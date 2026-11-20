//! Work that outlasts the request that asked for it.
//!
//! Installing Cobalt takes a minute and then waits for the reader to come
//! back on Wi-Fi; sending a comic takes as long as the comic is large. No
//! request can be held open for that, so the request starts a job and the
//! page asks after it.
//!
//! Most jobs are this same program run again as a child with the flags it
//! already has for running unattended. That is deliberate. `kobo setup` and
//! `kobo send` are several hundred lines of orchestration that say what they
//! are doing as they do it, in sentences already written for the owner, and
//! the honest way to put a window on them is to run them and show what they
//! say. The child's output is the progress, and its exit status is the
//! outcome.

use std::collections::VecDeque;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

/// The most output a job keeps.
///
/// Bounds what a child that never stops printing costs this process. The
/// longest real output, a full setup with its closing notes, is under eight
/// kilobytes, so nothing an owner would read is ever dropped; past the bound
/// the oldest text goes first, because the end is where the outcome is.
const MOST_TEXT: usize = 256 * 1024;

/// How many finished jobs are remembered.
///
/// A page that is reloaded asks again about the job it was showing, so the
/// last few are kept. A handful is enough: nothing looks further back than
/// the one job each place on the page last started.
const MOST_FINISHED: usize = 8;

/// What a job works on, which decides what may run beside it.
///
/// Two jobs writing to the same reader would interleave two transactions on
/// one volume, and two sends would race for one connection to the reader, so
/// each kind runs one at a time. A send and an install do not touch the same
/// thing and may overlap.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    /// Writes to a reader mounted over USB.
    Reader,
    /// Sends something to a reader over Wi-Fi, or to the queue.
    Send,
}

impl Kind {
    const fn word(self) -> &'static str {
        match self {
            Self::Reader => "reader",
            Self::Send => "send",
        }
    }
}

/// How a job ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Finish {
    /// Whether it did what was asked.
    pub ok: bool,
    /// The child's exit status, when the job was a child and it exited
    /// rather than being killed.
    pub code: Option<i32>,
}

#[derive(Debug, Default)]
struct Progress {
    text: String,
    /// Bytes dropped from the front of `text`, so offsets the page holds
    /// keep meaning the same place.
    dropped: usize,
    finish: Option<Finish>,
}

/// One job: what it is, what it has said so far, and how it ended.
#[derive(Debug)]
pub struct Job {
    pub id: u64,
    pub kind: Kind,
    /// What the job is doing, in the page's words: "Installing Cobalt".
    pub title: String,
    /// The mounted reader the job is writing to, when it is writing to one.
    /// A front end holds this against the readers still plugged in to tell
    /// a job that failed from one whose reader was pulled out from under it.
    pub volume: Option<String>,
    progress: Mutex<Progress>,
}

impl Job {
    /// Appends to what the job has said.
    pub fn say(&self, text: &str) {
        let Ok(mut progress) = self.progress.lock() else {
            return;
        };
        progress.text.push_str(text);
        if progress.text.len() > MOST_TEXT {
            let mut cut = progress.text.len() - MOST_TEXT;
            while !progress.text.is_char_boundary(cut) {
                cut += 1;
            }
            progress.text.drain(..cut);
            progress.dropped += cut;
        }
    }

    fn finish(&self, finish: Finish) {
        if let Ok(mut progress) = self.progress.lock() {
            progress.finish = Some(finish);
        }
    }

    /// Whether the job is still going.
    #[must_use]
    pub fn running(&self) -> bool {
        self.progress
            .lock()
            .is_ok_and(|progress| progress.finish.is_none())
    }

    /// The job as the page polls it: whether it is running, how it ended,
    /// and what it has said since offset `from`.
    ///
    /// `next` is the offset to ask from next time. Offsets count from the
    /// first byte the job ever said, so a page that fell behind a dropped
    /// stretch simply resumes at the oldest text still held.
    #[must_use]
    pub fn report(&self, from: usize) -> Value {
        let Ok(progress) = self.progress.lock() else {
            return json!({ "error": "the job's record could not be read" });
        };
        let mut start = from
            .saturating_sub(progress.dropped)
            .min(progress.text.len());
        while !progress.text.is_char_boundary(start) {
            start += 1;
        }
        json!({
            "id": self.id,
            "kind": self.kind.word(),
            "title": self.title,
            "volume": self.volume,
            "running": progress.finish.is_none(),
            "ok": progress.finish.map(|finish| finish.ok),
            "code": progress.finish.and_then(|finish| finish.code),
            "text": &progress.text[start..],
            "next": progress.dropped + progress.text.len(),
        })
    }

    /// The job without its text, for the list a reloaded page starts from.
    #[must_use]
    pub fn summary(&self) -> Value {
        let finish = self
            .progress
            .lock()
            .ok()
            .and_then(|progress| progress.finish);
        json!({
            "id": self.id,
            "kind": self.kind.word(),
            "title": self.title,
            "volume": self.volume,
            "running": finish.is_none(),
            "ok": finish.map(|finish| finish.ok),
        })
    }
}

/// Every job this run has started.
#[derive(Debug, Default)]
pub struct Jobs {
    list: Mutex<(u64, VecDeque<Arc<Job>>)>,
}

impl Jobs {
    /// Starts `work` on a thread of its own and returns the job's id.
    ///
    /// `work` says what it is doing through the job it is given and returns
    /// how it ended. `after` runs once it has, whatever the outcome, which
    /// is where a job puts away what it was given to work on.
    ///
    /// # Errors
    ///
    /// The title of the job already running, when one of the same kind is.
    pub fn start(
        self: &Arc<Self>,
        kind: Kind,
        title: &str,
        volume: Option<&Path>,
        work: impl FnOnce(&Job) -> Finish + Send + 'static,
        after: impl FnOnce() + Send + 'static,
    ) -> Result<u64, String> {
        let job = {
            let Ok(mut list) = self.list.lock() else {
                return Err("the job list".to_owned());
            };
            if let Some(busy) = list.1.iter().find(|job| job.kind == kind && job.running()) {
                return Err(busy.title.clone());
            }
            list.0 += 1;
            let job = Arc::new(Job {
                id: list.0,
                kind,
                title: title.to_owned(),
                volume: volume.map(|volume| volume.to_string_lossy().into_owned()),
                progress: Mutex::default(),
            });
            list.1.push_back(Arc::clone(&job));
            // Only finished jobs are forgotten. A running one is somebody's
            // install, and its record is how the page finds out it ended.
            while list.1.len() > MOST_FINISHED {
                let Some(oldest) = list.1.iter().position(|job| !job.running()) else {
                    break;
                };
                list.1.remove(oldest);
            }
            job
        };
        let id = job.id;
        std::thread::spawn(move || {
            let finish = work(&job);
            after();
            job.finish(finish);
        });
        Ok(id)
    }

    /// One job by id.
    #[must_use]
    pub fn find(&self, id: u64) -> Option<Arc<Job>> {
        self.list
            .lock()
            .ok()?
            .1
            .iter()
            .find(|job| job.id == id)
            .cloned()
    }

    /// The title of the running job of `kind`, when there is one.
    #[must_use]
    pub fn busy(&self, kind: Kind) -> Option<String> {
        self.list
            .lock()
            .ok()?
            .1
            .iter()
            .find(|job| job.kind == kind && job.running())
            .map(|job| job.title.clone())
    }

    /// Whether anything at all is still running.
    #[must_use]
    pub fn any_running(&self) -> bool {
        self.list
            .lock()
            .is_ok_and(|list| list.1.iter().any(|job| job.running()))
    }

    /// Every remembered job, oldest first, without its text.
    #[must_use]
    pub fn summaries(&self) -> Vec<Value> {
        self.list.lock().map_or_else(
            |_| Vec::new(),
            |list| list.1.iter().map(|job| job.summary()).collect(),
        )
    }
}

/// Runs a program to its end, saying everything it prints as it prints it.
///
/// Standard input is closed, so a child that would have asked a question
/// takes the branch it has for a pipe, which for every command run from here
/// is to say what flag answers it. Output and errors are read on a thread
/// each and said in the order they arrive, because a child whose pipe fills
/// stops, and one stream left unread while the other is drained is how that
/// happens.
///
/// Text is said in the chunks the child wrote it in, not line by line: setup
/// prints a dot at a time while it waits, and holding those back until a
/// newline would show nothing for five minutes.
pub fn run_child(
    program: &Path,
    arguments: &[String],
    directory: Option<&Path>,
    job: &Job,
) -> Finish {
    let mut command = Command::new(program);
    command
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(directory) = directory {
        command.current_dir(directory);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            job.say(&format!("Could not start {}: {error}\n", program.display()));
            return Finish {
                ok: false,
                code: None,
            };
        }
    };
    std::thread::scope(|scope| {
        if let Some(output) = child.stdout.take() {
            scope.spawn(|| relay(output, job));
        }
        if let Some(errors) = child.stderr.take() {
            scope.spawn(|| relay(errors, job));
        }
    });
    match child.wait() {
        Ok(status) => Finish {
            ok: status.success(),
            code: status.code(),
        },
        Err(error) => {
            job.say(&format!(
                "Could not wait for the command to finish: {error}\n"
            ));
            Finish {
                ok: false,
                code: None,
            }
        }
    }
}

/// Copies a child's stream into the job as text.
///
/// A read can end in the middle of a character, so bytes that do not yet
/// make one are held for the next read instead of being shown as a
/// replacement mark and then again as the rest of the character.
fn relay(mut stream: impl Read, job: &Job) {
    let mut held: Vec<u8> = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => held.extend_from_slice(&chunk[..read]),
        }
        let whole = match std::str::from_utf8(&held) {
            Ok(text) => text.len(),
            Err(error) if error.error_len().is_none() => error.valid_up_to(),
            // Bytes that will never be text. Said as what they are rather
            // than held forever waiting to become a character.
            Err(_) => held.len(),
        };
        job.say(&String::from_utf8_lossy(&held[..whole]));
        held.drain(..whole);
    }
    if !held.is_empty() {
        job.say(&String::from_utf8_lossy(&held));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn wait_for(jobs: &Jobs, id: u64) -> Arc<Job> {
        let job = jobs.find(id).expect("the job is remembered");
        let deadline = Instant::now() + Duration::from_secs(20);
        while job.running() {
            assert!(Instant::now() < deadline, "the job never finished");
            std::thread::sleep(Duration::from_millis(5));
        }
        job
    }

    fn done(ok: bool) -> Finish {
        Finish { ok, code: None }
    }

    #[test]
    fn a_job_says_what_it_did_and_then_how_it_ended() {
        let jobs = Arc::new(Jobs::default());
        let id = jobs
            .start(
                Kind::Reader,
                "Installing Cobalt",
                None,
                |job| {
                    job.say("one\n");
                    job.say("two\n");
                    done(true)
                },
                || {},
            )
            .expect("started");
        let report = wait_for(&jobs, id).report(0);
        assert_eq!(report["title"], "Installing Cobalt");
        assert_eq!(report["kind"], "reader");
        assert_eq!(report["running"], false);
        assert_eq!(report["ok"], true);
        assert_eq!(report["text"], "one\ntwo\n");
        assert_eq!(report["next"], 8);
    }

    #[test]
    fn a_page_that_has_read_some_of_the_text_is_given_only_the_rest() {
        let jobs = Arc::new(Jobs::default());
        let id = jobs
            .start(
                Kind::Send,
                "Sending",
                None,
                |job| {
                    job.say("caf\u{e9} one\n");
                    done(false)
                },
                || {},
            )
            .expect("started");
        let job = wait_for(&jobs, id);
        assert_eq!(job.report(6)["text"], "one\n");
        // An offset inside the two-byte letter moves on to the next whole
        // character instead of cutting it in half.
        assert_eq!(job.report(4)["text"], " one\n");
        assert_eq!(job.report(10_000)["text"], "");
        assert_eq!(job.report(0)["ok"], false);
    }

    #[test]
    fn a_job_that_never_stops_talking_keeps_only_the_newest_text() {
        let jobs = Arc::new(Jobs::default());
        let id = jobs
            .start(
                Kind::Send,
                "Talking",
                None,
                |job| {
                    for _ in 0..(MOST_TEXT / 1000 + 50) {
                        job.say(&"x".repeat(1000));
                    }
                    job.say("the end");
                    done(true)
                },
                || {},
            )
            .expect("started");
        let job = wait_for(&jobs, id);
        let report = job.report(0);
        let text = report["text"].as_str().expect("text");
        assert!(text.len() <= MOST_TEXT);
        assert!(text.ends_with("the end"));
        let total = (MOST_TEXT / 1000 + 50) * 1000 + "the end".len();
        assert_eq!(report["next"], total);
        assert_eq!(job.report(total - 7)["text"], "the end");
    }

    #[test]
    fn a_second_job_of_the_same_kind_is_refused_while_the_first_runs_and_another_kind_is_not() {
        let jobs = Arc::new(Jobs::default());
        let (release, held) = std::sync::mpsc::channel::<()>();
        let first = jobs
            .start(
                Kind::Reader,
                "Installing Cobalt",
                None,
                move |_| {
                    let _ = held.recv();
                    done(true)
                },
                || {},
            )
            .expect("started");
        assert_eq!(
            jobs.start(Kind::Reader, "Removing Cobalt", None, |_| done(true), || {})
                .expect_err("the reader is busy"),
            "Installing Cobalt"
        );
        assert_eq!(
            jobs.busy(Kind::Reader).as_deref(),
            Some("Installing Cobalt")
        );
        assert!(jobs.any_running());
        let other = jobs
            .start(Kind::Send, "Sending", None, |_| done(true), || {})
            .expect("a send does not wait for an install");
        wait_for(&jobs, other);
        release.send(()).expect("release the first job");
        wait_for(&jobs, first);
        assert!(!jobs.any_running());
        assert_eq!(jobs.busy(Kind::Reader), None);
        jobs.start(Kind::Reader, "Removing Cobalt", None, |_| done(true), || {})
            .expect("the reader is free again");
    }

    #[test]
    fn what_a_job_was_given_is_put_away_before_it_is_called_finished() {
        let jobs = Arc::new(Jobs::default());
        let put_away = Arc::new(Mutex::new(false));
        let seen = Arc::clone(&put_away);
        let id = jobs
            .start(
                Kind::Send,
                "Sending",
                None,
                |_| done(false),
                move || *seen.lock().expect("flag") = true,
            )
            .expect("started");
        wait_for(&jobs, id);
        assert!(*put_away.lock().expect("flag"));
    }

    #[test]
    fn old_finished_jobs_are_forgotten_and_an_unknown_id_finds_nothing() {
        let jobs = Arc::new(Jobs::default());
        let mut ids = Vec::new();
        for _ in 0..MOST_FINISHED + 3 {
            let id = jobs
                .start(Kind::Send, "Sending", None, |_| done(true), || {})
                .expect("started");
            wait_for(&jobs, id);
            ids.push(id);
        }
        assert!(jobs.find(ids[0]).is_none());
        assert!(jobs.find(*ids.last().expect("an id")).is_some());
        assert!(jobs.summaries().len() <= MOST_FINISHED);
        assert!(jobs.find(9_999).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn a_child_is_run_to_its_end_with_both_streams_shown_and_its_status_kept() {
        let jobs = Arc::new(Jobs::default());
        let id = jobs
            .start(Kind::Reader, "Running", None,
                |job| {
                    run_child(
                        Path::new("/bin/sh"),
                        &[
                            "-c".to_owned(),
                            "printf 'out '; printf 'err\\n' >&2; read answer || echo 'no input'; exit 3"
                                .to_owned(),
                        ],
                        None,
                        job,
                    )
                },
                || {},
            )
            .expect("started");
        let report = wait_for(&jobs, id).report(0);
        let text = report["text"].as_str().expect("text");
        assert!(text.contains("out "));
        assert!(text.contains("err\n"));
        // Standard input is closed, so a question is answered by its
        // absence instead of holding the job open forever.
        assert!(text.contains("no input"));
        assert_eq!(report["ok"], false);
        assert_eq!(report["code"], 3);
    }

    #[cfg(unix)]
    #[test]
    fn a_child_runs_in_the_folder_it_is_given() {
        let folder = std::env::temp_dir().join(format!("kobo-ui-jobs-{}", std::process::id()));
        std::fs::create_dir_all(&folder).expect("folder");
        std::fs::write(folder.join("only-here"), b"").expect("file");
        let jobs = Arc::new(Jobs::default());
        let cwd = folder.clone();
        let id = jobs
            .start(
                Kind::Send,
                "Listing",
                None,
                move |job| run_child(Path::new("/bin/ls"), &[], Some(&cwd), job),
                || {},
            )
            .expect("started");
        let report = wait_for(&jobs, id).report(0);
        assert_eq!(report["text"], "only-here\n");
        assert_eq!(report["ok"], true);
        let _ = std::fs::remove_dir_all(folder);
    }

    #[test]
    fn a_program_that_cannot_be_started_is_a_failed_job_that_says_why() {
        let jobs = Arc::new(Jobs::default());
        let id = jobs
            .start(
                Kind::Send,
                "Running",
                None,
                |job| run_child(Path::new("/no/such/program/anywhere"), &[], None, job),
                || {},
            )
            .expect("started");
        let report = wait_for(&jobs, id).report(0);
        assert_eq!(report["ok"], false);
        assert!(report["text"]
            .as_str()
            .expect("text")
            .starts_with("Could not start /no/such/program/anywhere"));
    }

    #[test]
    fn a_character_split_across_two_reads_is_shown_once_and_whole() {
        struct Halves(Vec<Vec<u8>>);
        impl Read for Halves {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                if self.0.is_empty() {
                    return Ok(0);
                }
                let next = self.0.remove(0);
                buffer[..next.len()].copy_from_slice(&next);
                Ok(next.len())
            }
        }
        let job = Job {
            id: 1,
            kind: Kind::Send,
            title: String::new(),
            volume: None,
            progress: Mutex::default(),
        };
        // "é" is 0xC3 0xA9; the reads split it, and a stray 0xFF follows.
        relay(
            Halves(vec![b"caf\xC3".to_vec(), b"\xA9 \xFF.".to_vec()]),
            &job,
        );
        assert_eq!(job.report(0)["text"], "caf\u{e9} \u{fffd}.");
    }
}

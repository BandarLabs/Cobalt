//! Sync on the reader's Articles shelf, answered from this computer.
//!
//! Books asks this helper, over the pairing Sidekick and Deck already use,
//! for any articles clipped here while the reader was away. The queue and
//! the copying belong to the companion, which owns the reader's SSH key and
//! the transport `kobo send` uses, so this runs `kobo clip sync` back to the
//! address the question came from and hands Books the one line it prints.
//!
//! The address the question came from is the reader as it is right now,
//! which is better than any saved address: a reader takes a new one from
//! the router every time it wakes. A question from this computer itself can
//! only be the simulator, so that is where those articles go.

use std::net::IpAddr;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Where `kobo sidekick run` says the companion is.
const COMPANION_VARIABLE: &str = "KOBO_COMPANION";

/// How long the companion may take. It gives every copy only what is left
/// of its own hundred seconds, so it has finished, and let go of the
/// queue, well before this; the runtime gives Books' request three minutes.
const SYNC_PATIENCE: Duration = Duration::from_secs(150);

/// The most the companion's answer is allowed to be.
const MOST_ANSWER: usize = 64 * 1024;

/// The target flags for the reader at `peer`.
#[must_use]
pub fn target_for(peer: IpAddr) -> Vec<String> {
    if peer.is_loopback() {
        vec!["--sim".to_owned()]
    } else {
        vec!["--device".to_owned(), peer.to_string()]
    }
}

/// Runs the companion's sync for the reader at `peer` and returns what to
/// tell Books.
///
/// Always a report, never an error status: a reader shows what went wrong
/// only if it is told in words, and a failed request carries none.
#[must_use]
pub fn sync_for(peer: IpAddr) -> String {
    let mut command = Command::new(companion());
    command
        .args(["clip", "sync"])
        .args(target_for(peer))
        .arg("--json")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    run(command).unwrap_or_else(problem)
}

/// The companion's last line, when it is the JSON object Books reads.
fn run(mut command: Command) -> Result<String, &'static str> {
    let mut child = command
        .spawn()
        .map_err(|_| "This computer has no kobo command beside its Sidekick helper.")?;
    let mut stdout = child.stdout.take().ok_or(UNFINISHED)?;
    let reader = std::thread::spawn(move || {
        let mut text = Vec::new();
        let _ = std::io::Read::read_to_end(
            &mut std::io::Read::take(&mut stdout, MOST_ANSWER as u64),
            &mut text,
        );
        text
    });
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < SYNC_PATIENCE => {
                std::thread::sleep(Duration::from_millis(100));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(UNFINISHED);
            }
        }
    }
    let text = reader.join().map_err(|_| UNFINISHED)?;
    last_object(&String::from_utf8_lossy(&text)).ok_or(UNFINISHED)
}

const UNFINISHED: &str = "The kobo command on this computer could not send the articles.";

/// The last line that parses as a JSON object, so a sentence the companion
/// printed first, such as which reader it chose, never reaches Books.
fn last_object(text: &str) -> Option<String> {
    text.lines()
        .rev()
        .map(str::trim)
        .find(|line| {
            line.starts_with('{')
                && kobo_json::parse(line).is_ok_and(|value| value.get("waiting").is_some())
        })
        .map(str::to_owned)
}

/// The answer while Sidekick runs as a sample, which runs nothing.
#[must_use]
pub fn not_in_the_sample() -> String {
    problem("Sidekick is running as a sample, which sends no articles. Start kobo sidekick run to use Sync.")
}

fn problem(reason: &str) -> String {
    kobo_json::ObjectBuilder::new()
        .set("delivered", 0_u32)
        .set("waiting", 0_u32)
        .set("problem", reason)
        .build()
        .to_json()
}

/// The companion `kobo sidekick run` named, then the one beside this helper,
/// as a release unpacks them, then whatever is on PATH.
fn companion() -> PathBuf {
    let named = std::env::var_os(COMPANION_VARIABLE).map(PathBuf::from);
    let beside = std::env::current_exe()
        .ok()
        .and_then(|here| here.parent().map(|folder| folder.join("kobo")));
    named
        .into_iter()
        .chain(beside)
        .find(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from("kobo"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reader_on_the_network_is_reached_at_the_address_it_asked_from() {
        let peer: IpAddr = "192.168.1.23".parse().expect("an address");
        assert_eq!(target_for(peer), ["--device", "192.168.1.23"]);
    }

    #[test]
    fn a_question_from_this_computer_is_the_simulator() {
        assert_eq!(target_for(IpAddr::from([127, 0, 0, 1])), ["--sim"]);
        assert_eq!(target_for("::1".parse().expect("loopback")), ["--sim"]);
    }

    #[test]
    fn only_the_companions_closing_report_reaches_books() {
        let printed = "Using your saved reader \"study\".\n\
                       {\"delivered\":1,\"waiting\":0}\n";
        assert_eq!(
            last_object(printed).as_deref(),
            Some("{\"delivered\":1,\"waiting\":0}")
        );
        assert_eq!(last_object("error: no reader is saved\n"), None);
    }

    #[test]
    fn a_problem_is_a_report_books_can_read() {
        let body = problem("No kobo command.");
        let value = kobo_json::parse(&body).expect("json");
        assert_eq!(
            value.get("problem").and_then(kobo_json::Value::as_str),
            Some("No kobo command.")
        );
        assert_eq!(
            value.get("waiting").and_then(kobo_json::Value::as_i64),
            Some(0)
        );
    }
}

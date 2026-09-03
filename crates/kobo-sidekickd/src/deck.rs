use command_group::{CommandGroup, GroupChild, Signal, UnixChildExt};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const CONFIG_POLL: Duration = Duration::from_secs(2);
const RESULT_LINGER: Duration = Duration::from_secs(30);
const RUN_LIMIT: Duration = Duration::from_secs(10 * 60);
const KILL_GRACE: Duration = Duration::from_secs(10);
const MAX_RUNNING: usize = 4;
const MAX_OUTPUT: usize = 2 * 1024;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeckFile {
    page: Vec<PageFile>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PageFile {
    name: String,
    key: Vec<KeyFile>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyFile {
    label: String,
    #[serde(default)]
    detail: String,
    run: String,
    #[serde(default)]
    confirm: bool,
}

#[derive(Clone, Debug)]
struct Page {
    name: String,
    keys: Vec<Key>,
}

#[derive(Clone, Debug)]
struct Key {
    id: String,
    label: String,
    detail: String,
    run: String,
    confirm: bool,
}

#[derive(Clone, Debug)]
struct ResultRecord {
    status: &'static str,
    exit: i32,
    tail: String,
    finished_at: u64,
    finished: Instant,
}

#[derive(Clone, Debug)]
enum RunState {
    Running,
    Finished(ResultRecord),
}

struct Inner {
    version: u64,
    source: Option<String>,
    available: bool,
    pages: Vec<Page>,
    error: Option<String>,
    runs: HashMap<String, RunState>,
    running: usize,
    last_config_check: Instant,
}

pub struct Deck {
    path: PathBuf,
    home: PathBuf,
    inner: Mutex<Inner>,
    changed: Condvar,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PressOutcome {
    NeedsConfirm,
    Started,
    Busy,
    Gone,
    Unavailable,
}

impl PressOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NeedsConfirm => "needs-confirm",
            Self::Started => "started",
            Self::Busy => "busy",
            Self::Gone => "gone",
            Self::Unavailable => "unavailable",
        }
    }
}

impl Deck {
    pub fn new(path: PathBuf, home: PathBuf) -> Arc<Self> {
        let now = Instant::now();
        let deck = Arc::new(Self {
            path,
            home,
            inner: Mutex::new(Inner {
                version: 0,
                source: None,
                available: false,
                pages: Vec::new(),
                error: None,
                runs: HashMap::new(),
                running: 0,
                last_config_check: now.checked_sub(CONFIG_POLL).unwrap_or(now),
            }),
            changed: Condvar::new(),
        });
        deck.refresh(true);
        deck
    }

    pub fn available(&self) -> bool {
        self.refresh(false);
        self.lock().available
    }

    pub fn snapshot(&self, known: u64, wait: Duration) -> Option<String> {
        let deadline = Instant::now() + wait;
        loop {
            self.refresh(false);
            let mut inner = self.lock();
            expire_results(&mut inner);
            if !inner.available {
                return None;
            }
            if inner.version != known || Instant::now() >= deadline {
                return Some(snapshot_json(&inner));
            }
            let until_config = inner.last_config_check + CONFIG_POLL;
            let wake = deadline.min(until_config);
            let duration = wake.saturating_duration_since(Instant::now());
            let (next, _) = self
                .changed
                .wait_timeout(inner, duration)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            inner = next;
            drop(inner);
        }
    }

    pub fn press(
        self: &Arc<Self>,
        id: &str,
        confirmed: bool,
        limit: Duration,
        grace: Duration,
    ) -> PressOutcome {
        self.refresh(false);
        let command = {
            let mut inner = self.lock();
            if !inner.available {
                return PressOutcome::Unavailable;
            }
            let Some(key) = inner
                .pages
                .iter()
                .flat_map(|page| page.keys.iter())
                .find(|key| key.id == id)
                .cloned()
            else {
                return PressOutcome::Gone;
            };
            if key.confirm && !confirmed {
                return PressOutcome::NeedsConfirm;
            }
            if matches!(inner.runs.get(id), Some(RunState::Running)) || inner.running >= MAX_RUNNING
            {
                return PressOutcome::Busy;
            }
            inner.runs.insert(id.to_owned(), RunState::Running);
            inner.running += 1;
            bump(&mut inner);
            key.run
        };
        self.changed.notify_all();
        let deck = Arc::clone(self);
        let id = id.to_owned();
        let home = self.home.clone();
        std::thread::spawn(move || {
            let result = run_command(&command, &home, limit, grace);
            let mut inner = deck.lock();
            inner.running = inner.running.saturating_sub(1);
            inner.runs.insert(id, RunState::Finished(result));
            bump(&mut inner);
            deck.changed.notify_all();
        });
        PressOutcome::Started
    }

    pub fn press_default(self: &Arc<Self>, id: &str, confirmed: bool) -> PressOutcome {
        self.press(id, confirmed, RUN_LIMIT, KILL_GRACE)
    }

    pub fn result(&self, id: &str) -> Option<String> {
        let mut inner = self.lock();
        expire_results(&mut inner);
        let state = inner.runs.get(id)?;
        let value = match state {
            RunState::Running => kobo_json::ObjectBuilder::new()
                .set("status", "running")
                .build(),
            RunState::Finished(result) => kobo_json::ObjectBuilder::new()
                .set("status", result.status)
                .set("exit", result.exit)
                .set("tail", result.tail.as_str())
                .set("finished_at", result.finished_at.to_string())
                .build(),
        };
        Some(value.to_json())
    }

    fn refresh(&self, force: bool) {
        let should_check = {
            let inner = self.lock();
            force || inner.last_config_check.elapsed() >= CONFIG_POLL
        };
        if !should_check {
            return;
        }
        let source = fs::read_to_string(&self.path);
        let mut inner = self.lock();
        inner.last_config_check = Instant::now();
        match source {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if inner.available || inner.source.is_some() {
                    inner.available = false;
                    inner.source = None;
                    inner.error = None;
                    bump(&mut inner);
                    self.changed.notify_all();
                }
            }
            Err(error) => {
                let message = format!("Can't read deck.toml: {error}");
                if inner.error.as_deref() != Some(&message) {
                    eprintln!("sidekick: {message}");
                    inner.error = Some(message);
                    bump(&mut inner);
                    self.changed.notify_all();
                }
            }
            Ok(source) => {
                inner.available = true;
                if inner.source.as_deref() == Some(&source) {
                    return;
                }
                inner.source = Some(source.clone());
                match parse_config(&source) {
                    Ok(pages) => {
                        inner.pages = pages;
                        inner.error = None;
                    }
                    Err(error) => {
                        eprintln!("sidekick: {error}");
                        inner.error = Some(error);
                    }
                }
                bump(&mut inner);
                self.changed.notify_all();
            }
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn parse_config(source: &str) -> Result<Vec<Page>, String> {
    let parsed: DeckFile = toml::from_str(source).map_err(|error: toml::de::Error| {
        let line = error
            .span()
            .map_or(1, |span| source[..span.start].lines().count().max(1));
        format!(
            "Deck file has a mistake on line {line}: {}",
            error.message()
        )
    })?;
    if !(1..=6).contains(&parsed.page.len()) {
        return Err("deck.toml needs between 1 and 6 pages".to_owned());
    }
    let mut pages = Vec::with_capacity(parsed.page.len());
    for page in parsed.page {
        let name = page.name.trim();
        if name.is_empty() || name.chars().count() > 24 {
            return Err("each page name must be 1 to 24 characters".to_owned());
        }
        if !(1..=12).contains(&page.key.len()) {
            return Err(format!("page '{name}' needs between 1 and 12 keys"));
        }
        let mut keys = Vec::with_capacity(page.key.len());
        for key in page.key {
            let label = key.label.trim();
            let detail = key.detail.trim();
            let run = key.run.trim();
            if label.is_empty() || label.chars().count() > 16 {
                return Err(format!(
                    "a key on page '{name}' has a label outside 1 to 16 characters"
                ));
            }
            if detail.chars().count() > 40 {
                return Err(format!(
                    "key '{label}' on page '{name}' has detail longer than 40 characters"
                ));
            }
            if run.is_empty() {
                return Err(format!("key '{label}' on page '{name}' has no command"));
            }
            keys.push(Key {
                id: stable_id(name, label, run),
                label: label.to_owned(),
                detail: detail.to_owned(),
                run: run.to_owned(),
                confirm: key.confirm,
            });
        }
        pages.push(Page {
            name: name.to_owned(),
            keys,
        });
    }
    Ok(pages)
}

fn stable_id(page: &str, label: &str, run: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(page.as_bytes());
    digest.update([0]);
    digest.update(label.as_bytes());
    digest.update([0]);
    digest.update(run.as_bytes());
    format!("{:x}", digest.finalize())
}

fn snapshot_json(inner: &Inner) -> String {
    let pages = inner
        .pages
        .iter()
        .map(|page| {
            let keys = page
                .keys
                .iter()
                .map(|key| {
                    let state = match inner.runs.get(&key.id) {
                        Some(RunState::Running) => "running",
                        Some(RunState::Finished(result)) => result.status,
                        None => "idle",
                    };
                    kobo_json::ObjectBuilder::new()
                        .set("id", key.id.as_str())
                        .set("label", key.label.as_str())
                        .set("detail", key.detail.as_str())
                        .set("confirm", key.confirm)
                        .set("state", state)
                        .build()
                })
                .collect();
            kobo_json::ObjectBuilder::new()
                .set("name", page.name.as_str())
                .set("keys", kobo_json::Value::Array(keys))
                .build()
        })
        .collect();
    let mut response = kobo_json::ObjectBuilder::new()
        .set("version", inner.version.to_string())
        .set("pages", kobo_json::Value::Array(pages));
    if let Some(error) = &inner.error {
        response = response.set("error", error.as_str());
    }
    response.build().to_json()
}

fn expire_results(inner: &mut Inner) {
    let before = inner.runs.len();
    inner.runs.retain(|_, state| match state {
        RunState::Running => true,
        RunState::Finished(result) => result.finished.elapsed() < RESULT_LINGER,
    });
    if inner.runs.len() != before {
        bump(inner);
    }
}

fn bump(inner: &mut Inner) {
    inner.version = inner.version.wrapping_add(1).max(1);
}

fn run_command(command: &str, home: &Path, limit: Duration, grace: Duration) -> ResultRecord {
    let mut process = Command::new("/bin/sh");
    process
        .args(["-c", command])
        .current_dir(home)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let Ok(mut child) = process.group_spawn() else {
        return finished("failed", -1, "Could not start the command.".to_owned());
    };
    let stdout = child.inner().stdout.take().map(read_in_thread);
    let stderr = child.inner().stderr.take().map(read_in_thread);
    let deadline = Instant::now() + limit;
    let (status, timed_out) = loop {
        match child.try_wait() {
            Ok(Some(status)) => break (Some(status), false),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(None) => {
                terminate(&mut child, grace);
                break (child.wait().ok(), true);
            }
            Err(_) => break (None, false),
        }
    };
    let mut output = Vec::new();
    if let Some(reader) = stdout {
        output.extend(reader.join().unwrap_or_default());
    }
    if let Some(reader) = stderr {
        if !output.is_empty() {
            output.push(b'\n');
        }
        output.extend(reader.join().unwrap_or_default());
    }
    let mut tail = clean_output(&output);
    if timed_out {
        if !tail.is_empty() {
            tail.push('\n');
        }
        tail.push_str("Killed after 10 minutes.");
    }
    let exit = status
        .as_ref()
        .and_then(std::process::ExitStatus::code)
        .unwrap_or(-1);
    let outcome = if !timed_out && status.is_some_and(|status| status.success()) {
        "ok"
    } else {
        "failed"
    };
    finished(outcome, exit, tail)
}

fn read_in_thread(mut reader: impl Read + Send + 'static) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut tail = Vec::new();
        let mut chunk = [0_u8; 512];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(read) => append_tail(&mut tail, &chunk[..read]),
            }
        }
        tail
    })
}

fn append_tail(tail: &mut Vec<u8>, bytes: &[u8]) {
    if bytes.len() >= MAX_OUTPUT {
        tail.clear();
        tail.extend_from_slice(&bytes[bytes.len() - MAX_OUTPUT..]);
        return;
    }
    let excess = tail
        .len()
        .saturating_add(bytes.len())
        .saturating_sub(MAX_OUTPUT);
    if excess > 0 {
        tail.drain(..excess);
    }
    tail.extend_from_slice(bytes);
}

fn terminate(child: &mut GroupChild, grace: Duration) {
    let _ = child.signal(Signal::SIGTERM);
    let deadline = Instant::now() + grace;
    while Instant::now() < deadline {
        if child.try_wait().ok().flatten().is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    let _ = child.kill();
}

fn clean_output(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut clean = String::new();
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '\u{1b}' {
            if characters.next_if_eq(&'[').is_some() {
                for code in characters.by_ref() {
                    if ('@'..='~').contains(&code) {
                        break;
                    }
                }
            } else {
                let _ = characters.next();
            }
        } else if character == '\n' || character == '\t' || !character.is_control() {
            clean.push(character);
        }
    }
    while clean.len() > MAX_OUTPUT {
        let first = clean
            .char_indices()
            .nth(1)
            .map_or(clean.len(), |(index, _)| index);
        clean.drain(..first);
    }
    clean
}

fn finished(status: &'static str, exit: i32, tail: String) -> ResultRecord {
    ResultRecord {
        status,
        exit,
        tail,
        finished_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        finished: Instant::now(),
    }
}

#[cfg(test)]
mod tests {
    use super::{clean_output, parse_config, stable_id, Deck, PressOutcome};
    use std::fs;
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn directory() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "cobalt-deck-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn sample(run: &str, confirm: bool) -> String {
        format!(
            r#"[[page]]
name = "Build"
[[page.key]]
label = "Test"
detail = "cargo test"
run = "{run}"
confirm = {confirm}
"#
        )
    }

    #[test]
    fn config_rejects_empty_and_oversized_decks() {
        assert!(parse_config("").is_err());
        assert!(parse_config("[[page]]\nname='x'\n").is_err());
        assert!(parse_config(&sample("true", false)).is_ok());
        assert!(parse_config(&sample("true", false).replace("Test", "12345678901234567")).is_err());
    }

    #[test]
    fn ids_are_stable_and_commands_are_part_of_the_identity() {
        assert_eq!(
            stable_id("Build", "Test", "true"),
            stable_id("Build", "Test", "true")
        );
        assert_ne!(
            stable_id("Build", "Test", "true"),
            stable_id("Build", "Test", "false")
        );
    }

    #[test]
    fn malformed_edits_keep_the_last_good_grid_and_surface_the_error() {
        let directory = directory();
        let path = directory.join("deck.toml");
        fs::write(&path, sample("true", false)).unwrap();
        let deck = Deck::new(path.clone(), directory.clone());
        let first = deck.snapshot(0, Duration::ZERO).unwrap();
        fs::write(&path, "not toml = [").unwrap();
        deck.refresh(true);
        let second = deck.snapshot(0, Duration::ZERO).unwrap();
        assert!(first.contains("\"label\":\"Test\""), "{first}");
        assert!(second.contains("\"label\":\"Test\""), "{second}");
        assert!(second.contains("\"error\":"), "{second}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn confirmation_busy_result_and_output_tail_are_enforced() {
        let directory = directory();
        let path = directory.join("deck.toml");
        fs::write(&path, sample("printf secret; sleep 0.2; false", true)).unwrap();
        let deck = Deck::new(path, directory.clone());
        let id = stable_id("Build", "Test", "printf secret; sleep 0.2; false");
        assert_eq!(
            deck.press(&id, false, Duration::from_secs(2), Duration::ZERO),
            PressOutcome::NeedsConfirm
        );
        assert_eq!(
            deck.press(&id, true, Duration::from_secs(2), Duration::ZERO),
            PressOutcome::Started
        );
        assert_eq!(
            deck.press(&id, true, Duration::from_secs(2), Duration::ZERO),
            PressOutcome::Busy
        );
        for _ in 0..100 {
            if let Some(result) = deck.result(&id) {
                if result.contains("\"status\":\"failed\"") {
                    assert!(result.contains("secret"), "{result}");
                    fs::remove_dir_all(directory).unwrap();
                    return;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("command did not finish");
    }

    #[test]
    fn terminal_escape_sequences_are_removed_from_results() {
        assert_eq!(clean_output(b"\x1b[31mred\x1b[0m\nplain"), "red\nplain");
    }

    #[test]
    fn timed_out_commands_are_killed_and_reported() {
        let directory = directory();
        let path = directory.join("deck.toml");
        fs::write(&path, sample("sleep 5", false)).unwrap();
        let deck = Deck::new(path, directory.clone());
        let id = stable_id("Build", "Test", "sleep 5");
        assert_eq!(
            deck.press(
                &id,
                false,
                Duration::from_millis(50),
                Duration::from_millis(20)
            ),
            PressOutcome::Started
        );
        for _ in 0..100 {
            if let Some(result) = deck.result(&id) {
                if result.contains("\"status\":\"failed\"") {
                    assert!(result.contains("Killed after 10 minutes."), "{result}");
                    fs::remove_dir_all(directory).unwrap();
                    return;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out command did not finish");
    }
}

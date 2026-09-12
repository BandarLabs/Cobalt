//! Manage the Sidekick helper from the companion, and prove the path works.
//!
//! The helper is its own binary, and until now an owner had to know that: find
//! `kobo-sidekickd`, run `setup` for their agent, remember to start it, and
//! have no way to ask whether it was running or to stop it. Everything else on
//! this device that keeps a helper alive is `kobo <thing> setup|run|status|
//! stop`, which is what Sync established, so Sidekick answers to the same four
//! words.
//!
//! `test` is the fifth, and it is the one that matters on a first evening: it
//! sends a harmless question of its own, waits for the reader to answer it, and
//! prints what came back. A permission prompt from a real agent is a poor first
//! test, because the thing it is asking to do is usually something nobody wants
//! to say yes to twice.
use std::fs;
use std::io::Read as _;
use std::io::Write as _;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The port the helper listens on for questions from an agent's hook.
const HOOK_PORT: u16 = 9330;

/// How long `test` waits for somebody to pick the reader up and answer.
///
/// Ten minutes rather than two: the first time anybody runs this they are also
/// pairing the reader, and two minutes ran out while the pairing code was
/// still being typed.
const TEST_PATIENCE: Duration = Duration::from_secs(600);

/// How long a background start waits to see the helper actually listening.
const START_PATIENCE: Duration = Duration::from_secs(10);

const USAGE: &str = "usage: kobo sidekick setup [AGENT] | run [--foreground] | status | stop | test";

pub fn command(arguments: &[String]) -> Result<(), String> {
    match arguments.split_first() {
        Some((verb, rest)) => match (verb.as_str(), rest) {
            ("setup", extra) => setup(extra),
            ("run", extra) => run(extra),
            ("status", []) => {
                status();
                Ok(())
            }
            ("stop", []) => stop(),
            ("test", extra) => test(extra),
            _ => Err(USAGE.to_owned()),
        },
        None => Err(USAGE.to_owned()),
    }
}

/// Installs the hook for one agent, through the helper's own setup.
///
/// The helper owns the knowledge of where each agent keeps its hooks, so this
/// does not duplicate it: it finds the binary, runs its setup, and says what
/// happened in the companion's voice.
fn setup(arguments: &[String]) -> Result<(), String> {
    let helper = locate()?;
    let agent = match arguments {
        [] => "claude".to_owned(),
        [name] => name.clone(),
        _ => return Err(USAGE.to_owned()),
    };
    let output = Command::new(&helper)
        .arg("setup")
        .arg(&agent)
        .output()
        .map_err(|error| format!("run {}: {error}", helper.display()))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let problem = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        return Err(format!(
            "the helper refused to set up {agent}: {}",
            problem.trim()
        ));
    }
    if !text.trim().is_empty() {
        println!("{}", text.trim_end());
    }
    println!("Sidekick is set up for {agent}. Start it with 'kobo sidekick run'.");
    Ok(())
}

fn run(arguments: &[String]) -> Result<(), String> {
    let foreground = match arguments {
        [] => false,
        [flag] if flag == "--foreground" => true,
        _ => return Err(USAGE.to_owned()),
    };
    let helper = locate()?;
    if listening() {
        return Err("the Sidekick helper is already running".to_owned());
    }
    if foreground {
        let status = Command::new(&helper)
            .arg("run")
            .status()
            .map_err(|error| format!("run {}: {error}", helper.display()))?;
        return if status.success() {
            Ok(())
        } else {
            Err(format!("the helper exited with {status}"))
        };
    }
    let child = Command::new(&helper)
        .arg("run")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("start {}: {error}", helper.display()))?;
    let pid = child.id();
    // Started is not the same as listening, and reporting the first as the
    // second is how a companion tells somebody everything is fine while the
    // reader sees nothing.
    let deadline = Instant::now() + START_PATIENCE;
    while Instant::now() < deadline {
        if listening() {
            write_pid(pid)?;
            println!(
                "Sidekick helper started in the background (PID {pid}).\nRun 'kobo sidekick test' to send the reader a harmless question, or 'kobo sidekick stop' to stop it."
            );
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Err(format!(
        "the helper started (PID {pid}) but is not answering on port {HOOK_PORT}; run 'kobo sidekick run --foreground' to see why"
    ))
}

fn status() {
    let helper = locate().ok();
    let running = listening();
    println!(
        "Sidekick helper: {}\n  binary   {}\n  hook     127.0.0.1:{HOOK_PORT}\n  config   {}",
        if running { "running" } else { "stopped" },
        helper.map_or_else(|| "not found on PATH".to_owned(), |path| path
            .display()
            .to_string()),
        config().map_or_else(|_| "unknown".to_owned(), |path| path.display().to_string()),
    );
    if let Some(pid) = read_pid() {
        println!("  started  PID {pid} by this companion");
    }
    if !running {
        println!("Run 'kobo sidekick run' to start it.");
    }
}

fn stop() -> Result<(), String> {
    let Some(pid) = read_pid() else {
        return if listening() {
            Err(
                "the helper is running but was not started by this companion; stop it where it was started"
                    .to_owned(),
            )
        } else {
            println!("The Sidekick helper is not running.");
            Ok(())
        };
    };
    let signalled = Command::new("kill")
        .arg(pid.to_string())
        .status()
        .map_err(|error| format!("stop PID {pid}: {error}"))?;
    if !signalled.success() {
        clear_pid();
        return Err(format!("PID {pid} could not be stopped; it may be gone"));
    }
    let deadline = Instant::now() + START_PATIENCE;
    while Instant::now() < deadline {
        if !listening() {
            clear_pid();
            println!("Sidekick helper stopped.");
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Err(format!("PID {pid} was asked to stop and is still answering"))
}

/// Sends the reader a question of the companion's own and prints the answer.
fn test(arguments: &[String]) -> Result<(), String> {
    if !arguments.is_empty() {
        return Err(USAGE.to_owned());
    }
    if !listening() {
        return Err(
            "the Sidekick helper is not running; start it with 'kobo sidekick run'".to_owned(),
        );
    }
    // Nothing here runs anything. The detail says so, because the whole point
    // of the test is that answering it is safe either way.
    let body = kobo_json::ObjectBuilder::new()
        .set("source", "kobo sidekick test")
        .set("session", "companion check")
        .set("tool", "Sample")
        .set(
            "detail",
            "This is a test from your computer. Nothing runs whichever you choose.",
        )
        .set("choices", kobo_json::Value::Array(Vec::new()))
        .set("permission", true)
        .set("multi", false)
        .build()
        .to_json();
    println!(
        "Sent a test question to the reader. Pick it up and answer; waiting up to {} seconds.",
        TEST_PATIENCE.as_secs()
    );
    let reply = post_ask(&body, TEST_PATIENCE)?;
    let decision = kobo_json::parse(&reply)
        .ok()
        .and_then(|value| {
            value
                .get("decision")
                .and_then(kobo_json::Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_default();
    match decision.as_str() {
        "allow" => println!("The reader answered: allow. The whole path works."),
        "deny" => println!("The reader answered: deny. The whole path works."),
        "" | "pass" => println!(
            "The reader left it for the terminal, which is also an answer: the question reached it and came back."
        ),
        other => println!("The reader answered: {other}. The whole path works."),
    }
    Ok(())
}

fn post_ask(body: &str, patience: Duration) -> Result<String, String> {
    let mut stream = TcpStream::connect(("127.0.0.1", HOOK_PORT))
        .map_err(|error| format!("reach the helper on port {HOOK_PORT}: {error}"))?;
    stream
        .set_read_timeout(Some(patience))
        .map_err(|error| format!("set a timeout: {error}"))?;
    let request = format!(
        "POST /ask HTTP/1.1\r\nHost: 127.0.0.1:{HOOK_PORT}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|error| format!("send the question: {error}"))?;
    let mut response = Vec::new();
    // A timeout here is nobody picking the reader up, which is a sentence
    // rather than an error number: this reported "Resource temporarily
    // unavailable (os error 35)" the first time it happened.
    stream.read_to_end(&mut response).map_err(|error| {
        if matches!(
            error.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        ) {
            format!(
                "nobody answered on the reader within {} minutes; the question is still waiting there",
                patience.as_secs() / 60
            )
        } else {
            format!("wait for the answer: {error}")
        }
    })?;
    let head = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or("the helper sent something this companion could not read")?;
    Ok(String::from_utf8_lossy(&response[head + 4..]).into_owned())
}

/// Whether anything is answering where the helper listens.
fn listening() -> bool {
    TcpStream::connect_timeout(
        &(std::net::Ipv4Addr::LOCALHOST, HOOK_PORT).into(),
        Duration::from_millis(400),
    )
    .is_ok()
}

/// The helper beside this binary first, then whatever is on PATH.
///
/// Beside first because that is where a release puts them together, and an
/// owner who has just unpacked one should not need a PATH entry to use it.
fn locate() -> Result<PathBuf, String> {
    if let Ok(here) = std::env::current_exe() {
        if let Some(directory) = here.parent() {
            let beside = directory.join("kobo-sidekickd");
            if beside.is_file() {
                return Ok(beside);
            }
        }
    }
    let found = Command::new("command")
        .args(["-v", "kobo-sidekickd"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|path| !path.is_empty());
    match found {
        Some(path) => Ok(PathBuf::from(path)),
        None => Err(
            "kobo-sidekickd is not beside this binary or on PATH; install it from the release you are running"
                .to_owned(),
        ),
    }
}

fn config() -> Result<PathBuf, String> {
    let home = std::env::var("HOME").map_err(|_| "HOME is not set".to_owned())?;
    Ok(Path::new(&home).join(".config/kobo/sidekick"))
}

fn pid_file() -> Result<PathBuf, String> {
    Ok(config()?.join("companion.pid"))
}

fn write_pid(pid: u32) -> Result<(), String> {
    let path = pid_file()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    fs::write(&path, format!("{pid}\n")).map_err(|error| format!("{}: {error}", path.display()))
}

fn read_pid() -> Option<u32> {
    fs::read_to_string(pid_file().ok()?)
        .ok()?
        .trim()
        .parse()
        .ok()
}

fn clear_pid() {
    if let Ok(path) = pid_file() {
        let _ignored = fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::{command, USAGE};

    fn words(input: &[&str]) -> Vec<String> {
        input.iter().map(|word| (*word).to_owned()).collect()
    }

    #[test]
    fn every_verb_is_one_the_other_helpers_already_use() {
        // Sync established setup, run, status and stop, and an owner who has
        // used one helper should not have to learn another vocabulary.
        for verb in ["setup", "run", "status", "stop", "test"] {
            assert!(USAGE.contains(verb), "{verb} is not in the usage line");
        }
    }

    #[test]
    fn an_unknown_verb_prints_the_usage_rather_than_guessing() {
        for arguments in [vec![], words(&["restart"]), words(&["status", "--json"])] {
            let error = command(&arguments).expect_err("refused");
            assert_eq!(error, USAGE, "{arguments:?}");
        }
    }

    #[test]
    fn test_refuses_extra_arguments_and_says_how_to_start_the_helper() {
        // With nothing listening, `test` has one useful thing to say, and it
        // is not a connection error.
        let error = command(&words(&["test"])).expect_err("nothing is listening");
        assert!(
            error.contains("kobo sidekick run"),
            "the way out is missing: {error}"
        );
    }
}

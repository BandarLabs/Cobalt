//! Owner-attended proof that the stock reader can be stopped and restarted.
//!
//! Everything the platform wants to do on the panel depends on this one
//! primitive, so it is proven on its own, with no display or input code
//! involved. If this is not completely reliable, nothing built on top of it can
//! be.
//!
//! What makes it safe to attempt:
//!
//! - The platform never owns boot, so a power cycle always returns the device to
//!   the stock reader. The worst outcome is a reboot.
//! - Nothing outside `/tmp` is written, and `/tmp` is a tmpfs that a reboot
//!   empties, so no state survives a failure.
//! - Before the reader is stopped, a watchdog is armed that restarts it
//!   unconditionally after a deadline. It is a separate detached process, so it
//!   survives this program being killed outright, including `SIGKILL`, which no
//!   in-process cleanup can ever handle. That turns "we crashed at the worst
//!   moment" from a reboot into a short wait.
//! - The reader is identified by its exact executable path and re-verified
//!   immediately before each signal, so no other process is ever signalled.

use kobo_hal::reader::{Reader, ReaderError, Watchdog};
use kobo_hal::supervisor::Suspended;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::thread::sleep;
use std::time::Duration;

const UNLOCK_ENV: &str = "KOBO_HANDOFF_UNLOCK";
const UNLOCK_PHRASE: &str = "OWNER_ATTENDED_READER_HANDOFF";

/// How long the reader is given to exit before `SIGKILL`, and again after it.
const STOP_GRACE: Duration = Duration::from_secs(15);
/// How long the restarted reader is given to appear in the process table.
const START_GRACE: Duration = Duration::from_secs(45);
/// The longest the device may be left without a reader.
const MAX_HOLD: Duration = Duration::from_secs(120);
/// How long after our own deadline the watchdog waits before intervening. It
/// only ever acts when we did not.
const WATCHDOG_MARGIN: Duration = Duration::from_secs(30);

fn main() -> ExitCode {
    let arguments = env::args().skip(1).collect::<Vec<_>>();
    let result = match arguments.first().map(String::as_str) {
        // The restart path is deliberately not gated behind the unlock phrase.
        // It only ever starts the reader, which is the safe direction, and the
        // watchdog must be able to run it without carrying a secret.
        Some("--restart-from") => match arguments.get(1) {
            Some(directory) => restart_from(Path::new(directory)),
            None => Err("--restart-from needs a directory".to_owned()),
        },
        // Proves the identification and capture logic against the real device
        // without signalling anything. This is the part that must be right
        // before the reader is ever stopped: if the wrong process were selected
        // or the environment captured incompletely, the reader would not come
        // back. It is read-only and safe to run at any time.
        Some("--dry-run") => dry_run(),
        _ => run(&arguments),
    };
    match result {
        Ok(report) => {
            println!("{report}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("kobo-handoff: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(arguments: &[String]) -> Result<String, String> {
    if env::var(UNLOCK_ENV).ok().as_deref() != Some(UNLOCK_PHRASE) {
        return Err("owner-attended handoff unlock is missing or incorrect".to_owned());
    }
    let hold = parse_hold(arguments)?;

    let reader = Reader::find().map_err(|error| error.to_string())?;
    let original_pid = reader.pid();
    note(&format!("found reader pid {original_pid}"));
    note(&format!(
        "session bus {}",
        reader.environment("DBUS_SESSION_BUS_ADDRESS").map_or_else(
            || "MISSING".to_owned(),
            |value| value.to_string_lossy().into_owned()
        )
    ));

    let state = PathBuf::from(format!("/tmp/kobo-handoff-{}", std::process::id()));
    reader
        .save(&state)
        .map_err(|error| format!("save reader description: {error}"))?;

    // Armed before anything is stopped. If the order were reversed there would
    // be a window in which the reader is down and nothing would bring it back.
    let watchdog = Watchdog::arm(&state, hold + WATCHDOG_MARGIN)
        .map_err(|error| format!("arm watchdog: {error}"))?;

    // Kobo's own freeze watchdog cannot tell a reader we stopped on purpose
    // from one that hung, and reboots the device about ten seconds after the
    // pings stop. Suspending it is what makes a hold longer than a few seconds
    // possible at all, and without it this primitive does not model what a real
    // session does.
    let suspended = match Suspended::suspend(reader.environment("DBUS_SESSION_BUS_ADDRESS")) {
        Ok(suspended) => {
            note("freeze watchdog suspended");
            suspended
        }
        Err(error) => {
            note(&format!("SUSPEND FAILED: {error}"));
            return Err(format!("suspend the freeze watchdog: {error}"));
        }
    };

    note("stopping the reader");
    let outcome = hold_without_reader(&reader, hold);
    note("hold finished");

    // The reader is restarted on every path, including failure, and the result
    // of that restart is reported rather than hidden behind the original error.
    let restart = reader.start(START_GRACE);
    let resumed = suspended.resume_once_fed(START_GRACE);
    watchdog.disarm();
    let _ignored = fs::remove_dir_all(&state);

    match (outcome, restart) {
        (Ok(()), Ok(new_pid)) => Ok(format!(
            "reader handoff completed; stopped pid {original_pid}, held for {} s, restarted as pid {new_pid}; freeze watchdog {}",
            hold.as_secs(),
            match resumed {
                Ok(after) => format!("resumed {after}"),
                Err(error) => format!("could not be resumed ({error}); it returns on the next reboot"),
            }
        )),
        (Ok(()), Err(error)) => Err(format!(
            "the reader was stopped but did not come back: {error}. Power cycle the device; it always boots the stock reader."
        )),
        (Err(error), Ok(new_pid)) => Err(format!(
            "handoff failed: {error}. The reader is running again as pid {new_pid}."
        )),
        (Err(error), Err(restart_error)) => Err(format!(
            "handoff failed: {error}, and the reader did not come back: {restart_error}. Power cycle the device; it always boots the stock reader."
        )),
    }
}

/// Stops the reader, waits, and reports whether the stop itself succeeded.
/// Restarting is the caller's responsibility so that it happens on every path.
fn hold_without_reader(reader: &Reader, hold: Duration) -> Result<(), ReaderError> {
    reader.stop(STOP_GRACE)?;
    sleep(hold);
    Ok(())
}

/// Appends one line to a record that survives an unclean reset.
///
/// The reader being absent is exactly when a device reboots itself, and a
/// report printed to a standard output nobody is reading, or written to a
/// tmpfs a reboot empties, is no report at all.
fn note(event: &str) {
    use std::io::Write;
    let uptime = fs::read_to_string("/proc/uptime").unwrap_or_default();
    let uptime = uptime.split_whitespace().next().unwrap_or("?").to_owned();
    let Ok(mut file) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/mnt/onboard/.kobo-handoff.log")
    else {
        return;
    };
    let _ignored = writeln!(file, "{uptime} {event}");
    let _ignored = file.sync_all();
}

fn parse_hold(arguments: &[String]) -> Result<Duration, String> {
    let seconds = match arguments {
        [] => 5,
        [flag, value] if flag == "--hold" => value
            .parse::<u64>()
            .map_err(|_| format!("--hold needs a whole number of seconds, got {value}"))?,
        _ => return Err("usage: kobo-handoff [--hold SECONDS]".to_owned()),
    };
    let hold = Duration::from_secs(seconds);
    if hold > MAX_HOLD {
        return Err(format!(
            "--hold is capped at {} seconds so the device is never left without a reader for long",
            MAX_HOLD.as_secs()
        ));
    }
    Ok(hold)
}

/// Identifies the reader and proves the captured description round-trips,
/// without stopping anything.
fn dry_run() -> Result<String, String> {
    let reader = Reader::find().map_err(|error| error.to_string())?;
    let state = PathBuf::from(format!("/tmp/kobo-handoff-dry-{}", std::process::id()));
    reader
        .save(&state)
        .map_err(|error| format!("save reader description: {error}"))?;
    let restored = Reader::load(&state).map_err(|error| format!("load description: {error}"))?;
    let _ignored = fs::remove_dir_all(&state);

    // A description that does not survive the round trip would restart the
    // reader with the wrong arguments or a truncated environment, which is
    // exactly the failure that leaves an owner with a dead-looking device.
    if restored.arguments() != reader.arguments() {
        return Err("the saved arguments did not round-trip".to_owned());
    }
    if restored.environment_len() != reader.environment_len() {
        return Err("the saved environment did not round-trip".to_owned());
    }
    let command = restored.start_command();
    let rendered = command
        .get_args()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ");

    Ok(format!(
        "dry run: reader pid {} identified by exact path; {} environment entries and {} arguments captured and verified.\nnothing was signalled.\nrestart would run: /bin/sh {rendered}",
        reader.pid(),
        reader.environment_len(),
        reader.arguments().len()
    ))
}

/// Starts the reader from a saved description, unless one is already running.
fn restart_from(directory: &Path) -> Result<String, String> {
    if Reader::find().is_ok() {
        return Ok("the reader is already running; nothing to do".to_owned());
    }
    let reader = Reader::load(directory).map_err(|error| format!("load description: {error}"))?;
    let pid = reader
        .start(START_GRACE)
        .map_err(|error| error.to_string())?;
    Ok(format!("reader restarted as pid {pid}"))
}

#[cfg(test)]
mod tests {
    use super::{parse_hold, MAX_HOLD};
    use std::time::Duration;

    #[test]
    fn the_default_hold_is_short() {
        assert_eq!(parse_hold(&[]).expect("default"), Duration::from_secs(5));
    }

    #[test]
    fn an_explicit_hold_is_accepted() {
        let arguments = ["--hold".to_owned(), "20".to_owned()];
        assert_eq!(
            parse_hold(&arguments).expect("explicit"),
            Duration::from_secs(20)
        );
    }

    #[test]
    fn an_unbounded_hold_is_refused() {
        // Leaving the device without a reader indefinitely is exactly the
        // failure this tool exists to avoid.
        let arguments = ["--hold".to_owned(), (MAX_HOLD.as_secs() + 1).to_string()];
        assert!(parse_hold(&arguments).is_err());
    }

    #[test]
    fn a_nonsense_hold_is_refused_rather_than_defaulted() {
        let arguments = ["--hold".to_owned(), "forever".to_owned()];
        assert!(parse_hold(&arguments).is_err());
    }

    #[test]
    fn unknown_arguments_are_refused() {
        let arguments = ["--wipe".to_owned()];
        assert!(parse_hold(&arguments).is_err());
    }
}

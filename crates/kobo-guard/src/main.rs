//! Crash cleanup and screen hand-back.
//!
//! The guardian captures the whole screen before a child runs, supervises that
//! child, and puts the screen back on every exit path: success, failure,
//! signal, or timeout. It is the mechanism that turns an application crash into
//! a cosmetic event rather than something the owner has to reboot out of.
//!
//! Deliberate limits in this phase:
//!
//! - It never stops or restarts the stock reader. Nickel stays alive, so
//!   sessions are short and owner-attended. Reader handoff is Phase 4.
//! - It never grabs touch, so the reader keeps receiving input.
//! - It signals exactly one child through the handle it created, and never
//!   searches for a process by name. Nothing else on the device is signalled.
//! - Restoration is explicit on every path rather than a `Drop` guard, because
//!   release builds abort on panic and would never run one.

use kobo_hal::surface::RegionSnapshot;
use kobo_hal::{
    DisplaySession, Rect, RefreshIntent, RefreshPlan, SurfaceGeometry, OWNER_UNLOCK_PHRASE,
};
use kobo_profile::CLARA_BW_391;
use std::env;
use std::fmt;
use std::path::Path;
use std::process::{Child, Command, ExitCode, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

/// The owner has to set this exactly. It is separate from the display unlock so
/// that supervising a child is its own deliberate decision.
const GUARD_UNLOCK_VARIABLE: &str = "KOBO_GUARD_UNLOCK";
const GUARD_UNLOCK_PHRASE: &str = "OWNER_ATTENDED_GUARDED_SESSION";

/// Bounds on how long a supervised child may run.
const DEFAULT_TIMEOUT_SECONDS: u64 = 30;
const MAXIMUM_TIMEOUT_SECONDS: u64 = 300;
/// How often the child is checked. Short enough to feel immediate, long enough
/// not to spin on a single-core device.
const POLL_INTERVAL: Duration = Duration::from_millis(50);
/// After a child is asked to stop, how long it is given before giving up on it.
const KILL_GRACE: Duration = Duration::from_secs(2);
/// Region the `--prove-restore` test aid deliberately damages, so a hardware
/// run has something real for restoration to undo. It stands in for an
/// application that scribbled on the screen and then died.
const PROVE_REGION: Rect = Rect {
    x: 408,
    y: 600,
    width: 256,
    height: 256,
};
const PROVE_RESTORE_FLAG: &str = "--prove-restore";

fn main() -> ExitCode {
    let arguments = env::args().skip(1).collect::<Vec<_>>();
    if arguments.iter().any(|argument| argument == "--help") {
        print_help();
        return ExitCode::SUCCESS;
    }
    match run(&arguments) {
        Ok(report) => {
            println!("{report}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("kobo-guard: {error}");
            ExitCode::from(3)
        }
    }
}

fn print_help() {
    println!("kobo-guard 0.1.0");
    println!(
        "Usage: kobo-guard --run <absolute-path> [--timeout-seconds <1-{MAXIMUM_TIMEOUT_SECONDS}>]"
    );
    println!("Requires {GUARD_UNLOCK_VARIABLE} to be set to the exact owner phrase.");
    println!("Captures the screen, supervises one child, and always restores the screen.");
}

fn run(arguments: &[String]) -> Result<String, String> {
    if env::var(GUARD_UNLOCK_VARIABLE).ok().as_deref() != Some(GUARD_UNLOCK_PHRASE) {
        return Err(format!(
            "{GUARD_UNLOCK_VARIABLE} is missing or incorrect; refusing to supervise anything"
        ));
    }
    let request = Request::parse(arguments)?;

    // The display session applies the profile, geometry and identity gates, so
    // a screen is only ever captured or written on exactly the known hardware.
    let session = DisplaySession::open(&CLARA_BW_391, Some(OWNER_UNLOCK_PHRASE))
        .map_err(|error| error.to_string())?;
    let geometry = session.geometry();
    let whole_screen = Rect {
        x: 0,
        y: 0,
        width: geometry.width,
        height: geometry.height,
    };
    let screen = session
        .capture(whole_screen)
        .map_err(|error| format!("capture whole screen: {error}"))?;
    let plan = RefreshPlan::new(
        whole_screen,
        RefreshIntent::QualityContent,
        false,
        geometry.width,
        geometry.height,
    )
    .ok_or_else(|| "the whole screen is not a valid refresh region".to_owned())?;

    // Past this point the child may have changed the screen, so every path has
    // to go through restoration before returning.
    let damaged = if request.prove_restore {
        damage(&session, geometry)
    } else {
        Ok(())
    };
    let outcome = supervise(&request);
    let restored = restore(&session, &screen, plan);

    damaged?;
    let outcome = outcome?;
    let bytes = restored?;
    Ok(format!(
        "guarded session finished: {outcome}; {bytes} screen bytes restored and verified"
    ))
}

/// Inverts a fixed region so a hardware run has real damage to undo.
///
/// This exists only to make restoration falsifiable. Without it a successful
/// run would prove nothing, because the screen would already be correct.
fn damage(session: &DisplaySession, geometry: SurfaceGeometry) -> Result<(), String> {
    let region = session
        .capture(PROVE_REGION)
        .map_err(|error| format!("capture region to damage: {error}"))?;
    let plan = RefreshPlan::new(
        PROVE_REGION,
        RefreshIntent::QualityContent,
        false,
        geometry.width,
        geometry.height,
    )
    .ok_or_else(|| "the proving region is not inside this screen".to_owned())?;
    session
        .restore(&region.inverted_rgb())
        .map_err(|error| format!("write damaged region: {error}"))?;
    session
        .refresh(plan)
        .map_err(|error| format!("refresh damaged region: {error}"))
}

/// Puts the captured screen back and proves it landed.
fn restore(
    session: &DisplaySession,
    screen: &RegionSnapshot,
    plan: RefreshPlan,
) -> Result<usize, String> {
    session
        .restore(screen)
        .map_err(|error| format!("restore whole screen: {error}"))?;
    session
        .refresh(plan)
        .map_err(|error| format!("refresh restored screen: {error}"))?;
    let verify = session
        .capture(screen.placement().region())
        .map_err(|error| format!("verify restored screen: {error}"))?;
    if verify.matches(screen) {
        Ok(screen.pixels().len())
    } else {
        Err("the restored screen does not match the captured bytes".to_owned())
    }
}

#[derive(Debug, Eq, PartialEq)]
struct Request {
    program: String,
    timeout: Duration,
    prove_restore: bool,
}

impl Request {
    fn parse(arguments: &[String]) -> Result<Self, String> {
        let prove_restore = arguments.iter().any(|value| value == PROVE_RESTORE_FLAG);
        let arguments = arguments
            .iter()
            .filter(|value| *value != PROVE_RESTORE_FLAG)
            .cloned()
            .collect::<Vec<_>>();
        let (program, seconds) = match arguments.as_slice() {
            [flag, program] if flag == "--run" => (program, DEFAULT_TIMEOUT_SECONDS),
            [flag, program, timeout_flag, value]
                if flag == "--run" && timeout_flag == "--timeout-seconds" =>
            {
                let seconds = value
                    .parse::<u64>()
                    .map_err(|_| "--timeout-seconds must be a whole number".to_owned())?;
                (program, seconds)
            }
            _ => {
                return Err(format!(
                    "usage: kobo-guard --run <absolute-path> \
                     [--timeout-seconds <1-{MAXIMUM_TIMEOUT_SECONDS}>]"
                ))
            }
        };
        if seconds == 0 || seconds > MAXIMUM_TIMEOUT_SECONDS {
            return Err(format!(
                "--timeout-seconds must be between 1 and {MAXIMUM_TIMEOUT_SECONDS}"
            ));
        }
        // An absolute path means the child is never resolved through PATH, so
        // what runs cannot depend on the environment it was launched from.
        let path = Path::new(program);
        if !path.is_absolute() {
            return Err("the supervised program must be an absolute path".to_owned());
        }
        if !path.is_file() {
            return Err(format!("{program} is not an existing file"));
        }
        Ok(Self {
            program: program.clone(),
            timeout: Duration::from_secs(seconds),
            prove_restore,
        })
    }
}

#[derive(Debug, Eq, PartialEq)]
enum Outcome {
    Exited(i32),
    Signalled,
    TimedOut,
}

impl fmt::Display for Outcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exited(0) => write!(formatter, "the child exited normally"),
            Self::Exited(code) => write!(formatter, "the child exited with status {code}"),
            Self::Signalled => write!(formatter, "the child was terminated by a signal"),
            Self::TimedOut => write!(formatter, "the child timed out and was stopped"),
        }
    }
}

/// Runs the child and waits for it, never longer than the requested bound.
///
/// The environment is cleared, so a supervised program cannot inherit the
/// unlock phrases and re-enter either the guard or the display path. Standard
/// input is detached as well, so a child can never consume the owner's SSH
/// session out from under the guardian.
fn supervise(request: &Request) -> Result<Outcome, String> {
    let mut child = start(&request.program)?;

    let deadline = Instant::now() + request.timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return Ok(status.code().map_or(Outcome::Signalled, Outcome::Exited))
            }
            Ok(None) => {}
            Err(error) => {
                stop(&mut child);
                return Err(format!("wait for {}: {error}", request.program));
            }
        }
        if Instant::now() >= deadline {
            stop(&mut child);
            return Ok(Outcome::TimedOut);
        }
        sleep(POLL_INTERVAL);
    }
}

/// Starts the program, riding out the one refusal that cures itself.
///
/// "Text file busy" means some other process still holds the executable open
/// for writing — an installer finishing a copy, or on a busy host a child
/// forked between another thread's write and its exec. The writer is done in
/// a moment, so a short retry is the difference between a supervisor that
/// works and one that fails by timing.
fn start(program: &str) -> Result<Child, String> {
    let deadline = Instant::now() + KILL_GRACE;
    loop {
        let refused = match Command::new(program)
            .env_clear()
            .stdin(Stdio::null())
            .spawn()
        {
            Ok(child) => return Ok(child),
            Err(error) => error,
        };
        let busy = refused.raw_os_error() == Some(TEXT_FILE_BUSY);
        if !busy || Instant::now() >= deadline {
            return Err(format!("start {program}: {refused}"));
        }
        sleep(POLL_INTERVAL);
    }
}

/// ETXTBSY, without pulling a crate in for one number. It is 26 on Linux,
/// where the guardian runs, and 26 on the host this is tested on.
const TEXT_FILE_BUSY: i32 = 26;

/// Stops exactly the child this process created.
///
/// `Child::kill` signals the process this handle owns. No name matching and no
/// process group is involved, so nothing else on the device can be affected.
fn stop(child: &mut Child) {
    let _ignored = child.kill();
    let deadline = Instant::now() + KILL_GRACE;
    while Instant::now() < deadline {
        if matches!(child.try_wait(), Ok(Some(_)) | Err(_)) {
            return;
        }
        sleep(POLL_INTERVAL);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        supervise, Outcome, Request, DEFAULT_TIMEOUT_SECONDS, GUARD_UNLOCK_PHRASE,
        MAXIMUM_TIMEOUT_SECONDS,
    };
    use std::time::{Duration, Instant};

    fn arguments(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| (*part).to_owned()).collect()
    }

    #[test]
    fn a_supervised_program_must_be_an_existing_absolute_path() {
        let request = Request::parse(&arguments(&["--run", "/bin/sh"])).expect("/bin/sh exists");
        assert_eq!(
            request,
            Request {
                program: "/bin/sh".to_owned(),
                timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECONDS),
                prove_restore: false,
            }
        );
        for rejected in [
            vec!["--run", "sh"],
            vec!["--run", "./sh"],
            vec!["--run", "/definitely/not/here"],
            vec!["--run", "/bin"],
            vec!["--run"],
            vec![],
        ] {
            assert!(
                Request::parse(&arguments(&rejected)).is_err(),
                "{rejected:?} must be refused"
            );
        }
    }

    #[test]
    fn the_child_timeout_is_bounded() {
        let request = Request::parse(&arguments(&["--run", "/bin/sh", "--timeout-seconds", "5"]))
            .expect("a bounded timeout is accepted");
        assert_eq!(request.timeout, Duration::from_secs(5));
        for rejected in [
            "0",
            &(MAXIMUM_TIMEOUT_SECONDS + 1).to_string(),
            "-1",
            "lots",
        ] {
            assert!(
                Request::parse(&arguments(&[
                    "--run",
                    "/bin/sh",
                    "--timeout-seconds",
                    rejected
                ]))
                .is_err(),
                "{rejected} must be refused"
            );
        }
    }

    #[test]
    fn the_proving_flag_is_opt_in_and_does_not_disturb_the_rest_of_the_parse() {
        let plain = Request::parse(&arguments(&["--run", "/bin/sh"])).expect("parses");
        assert!(!plain.prove_restore);
        let proving = Request::parse(&arguments(&[
            "--run",
            "/bin/sh",
            "--prove-restore",
            "--timeout-seconds",
            "5",
        ]))
        .expect("parses");
        assert!(proving.prove_restore);
        assert_eq!(proving.program, "/bin/sh");
        assert_eq!(proving.timeout, Duration::from_secs(5));
    }

    #[test]
    fn the_proving_region_is_inside_the_screen_and_not_the_whole_of_it() {
        use kobo_profile::CLARA_BW_391;
        let width = CLARA_BW_391.width;
        let height = CLARA_BW_391.height;
        assert!(super::PROVE_REGION.x + super::PROVE_REGION.width <= width);
        assert!(super::PROVE_REGION.y + super::PROVE_REGION.height <= height);
        assert!(super::PROVE_REGION.width < width);
        assert!(super::PROVE_REGION.height < height);
    }

    #[test]
    fn the_unlock_phrase_is_owner_attended_and_distinct_from_the_display_one() {
        assert_eq!(GUARD_UNLOCK_PHRASE, "OWNER_ATTENDED_GUARDED_SESSION");
        assert_ne!(GUARD_UNLOCK_PHRASE, kobo_hal::OWNER_UNLOCK_PHRASE);
    }

    #[test]
    fn every_outcome_reads_clearly() {
        assert_eq!(Outcome::Exited(0).to_string(), "the child exited normally");
        assert_eq!(
            Outcome::Exited(3).to_string(),
            "the child exited with status 3"
        );
        assert_eq!(
            Outcome::Signalled.to_string(),
            "the child was terminated by a signal"
        );
        assert_eq!(
            Outcome::TimedOut.to_string(),
            "the child timed out and was stopped"
        );
    }

    /// The three child outcomes the guardian has to survive, exercised for
    /// real rather than described. Restoration is not involved here because
    /// there is no framebuffer on the host; that part is proven on hardware.
    #[test]
    fn a_child_that_succeeds_fails_or_hangs_is_always_reported() {
        let scripts = TempScripts::new();
        let succeeds = scripts.write("succeeds", "exit 0\n");
        let fails = scripts.write("fails", "exit 3\n");
        let hangs = scripts.write("hangs", "while :; do sleep 1; done\n");

        assert_eq!(
            supervise(&Request {
                program: succeeds,
                timeout: Duration::from_secs(10),
                prove_restore: false,
            }),
            Ok(Outcome::Exited(0))
        );
        assert_eq!(
            supervise(&Request {
                program: fails,
                timeout: Duration::from_secs(10),
                prove_restore: false,
            }),
            Ok(Outcome::Exited(3))
        );

        let started = Instant::now();
        assert_eq!(
            supervise(&Request {
                program: hangs,
                timeout: Duration::from_secs(1),
                prove_restore: false,
            }),
            Ok(Outcome::TimedOut)
        );
        // The bound has to be honoured in both directions: it waits for it, and
        // it stops promptly afterwards instead of hanging on the child.
        assert!(started.elapsed() >= Duration::from_secs(1));
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    /// A child that ignores the polite stop still has to be stopped.
    #[test]
    fn a_child_that_ignores_termination_is_still_stopped_within_the_grace() {
        let scripts = TempScripts::new();
        let stubborn = scripts.write("stubborn", "trap '' TERM INT\nwhile :; do sleep 1; done\n");
        let started = Instant::now();
        assert_eq!(
            supervise(&Request {
                program: stubborn,
                timeout: Duration::from_secs(1),
                prove_restore: false,
            }),
            Ok(Outcome::TimedOut)
        );
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    struct TempScripts {
        root: std::path::PathBuf,
    }

    impl TempScripts {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "kobo-guard-test-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            std::fs::create_dir_all(&root).expect("create script directory");
            Self { root }
        }

        fn write(&self, name: &str, body: &str) -> String {
            use std::os::unix::fs::PermissionsExt;
            let path = self.root.join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}")).expect("write script");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
                .expect("make script executable");
            path.display().to_string()
        }
    }

    impl Drop for TempScripts {
        fn drop(&mut self) {
            let _ignored = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn a_child_that_cannot_start_is_an_error_not_a_silent_success() {
        let missing = Request {
            program: "/definitely/not/here".to_owned(),
            timeout: Duration::from_secs(1),
            prove_restore: false,
        };
        assert!(supervise(&missing).is_err());
    }
}

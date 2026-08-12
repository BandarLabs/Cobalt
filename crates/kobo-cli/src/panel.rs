//! Running one application on the reader's own panel.
//!
//! Everything else in this SDK either builds something or reads something.
//! This starts a session: it stops nothing, but it does take the panel away
//! from whatever is drawing on it, so it is behind `device-write` alongside
//! the other commands that can change what an owner is looking at.
//!
//! There was no subcommand for this at all until now, which meant the one
//! thing a developer does most often, put my app on the screen and look at it,
//! was the one thing they had to hand-write an `ssh` line for. Those lines
//! carried the unlock phrase, a `nohup setsid` incantation and a `timeout`,
//! and getting any of the three wrong left the reader with no panel and no
//! stock interface until it was power cycled.
//!
//! A session is always bounded. `timeout` on the device is what actually ends
//! it, so a session outlives neither the network nor this process: pull the
//! cable, close the laptop, lose Wi-Fi, and the reader still comes back on its
//! own. [`stop`] only exists to end one early.

use std::time::Duration;

use crate::{run_remote_shell, INSTALLED_PACKAGES, STORE_PACKAGES};

/// Where `deploy` and `package` both put the binaries.
const INSTALL_ROOT: &str = "/mnt/onboard/.adds/cobalt";

/// What the daemon requires before it will take the panel.
///
/// A developer starting a session at a keyboard is the attendance the gate is
/// asking about, exactly as an owner tapping a menu entry is in `start.sh`.
const PRESENT_UNLOCK: &str = "OWNER_ATTENDED_PANEL_SESSION";

/// How long a session runs when no length is given.
///
/// Long enough to look at a screen and tap through it, short enough that
/// forgetting about it costs a few minutes rather than a flat battery.
pub const DEFAULT_SECONDS: u64 = 240;

/// The longest session that can be asked for.
///
/// An hour is far past any reasonable look at a screen, and the point of the
/// ceiling is that the number is a bound rather than a promise: a session that
/// could be asked to run all day is a session that will one day be left
/// running all day.
pub const MAXIMUM_SECONDS: u64 = 60 * 60;

const START_TIMEOUT: Duration = Duration::from_secs(120);
const STOP_TIMEOUT: Duration = Duration::from_secs(75);

/// How long the daemon is given to hand the panel back before it is called a
/// failure.
///
/// Ending a session restarts the stock reader, and the package's own
/// instructions put that at twenty to thirty seconds. Ten was the first guess
/// and it was wrong: `stop` reported a daemon that would not die, and the very
/// next command found nothing running.
const SHUTDOWN_SECONDS: u64 = 45;

/// How many times a command will try to reach a reader before giving up.
pub(crate) const CONNECT_ATTEMPTS: u32 = 4;
/// How long to leave the radio to come back between attempts.
pub(crate) const WAKE_INTERVAL: Duration = Duration::from_secs(4);

/// How long the reader is given to settle after a session ends.
///
/// Ending one restarts the stock reader, and starting a new session while that
/// is happening fails in a way that looks exactly like the application
/// crashing. Measured at ten to twelve seconds on a Clara BW, so this is that
/// with a little room.
const SETTLE_SECONDS: u64 = 14;

pub const PRESENT_USAGE: &str =
    "usage: kobo present <app> --device IP [--seconds N] [--keep-running]";
pub const STOP_USAGE: &str = "usage: kobo stop --device IP";

#[derive(Debug, Eq, PartialEq)]
pub struct Present {
    pub app: String,
    pub host: String,
    pub seconds: u64,
    /// Whether an already running session should be left alone.
    pub keep_running: bool,
}

/// Reads the arguments for `kobo present`.
///
/// # Errors
///
/// When the application is missing or unknown, the host is missing or
/// unusable, or the length is not a number within the bound.
pub fn parse_present(arguments: &[String]) -> Result<Present, String> {
    let mut app: Option<String> = None;
    let mut host: Option<String> = None;
    let mut seconds = DEFAULT_SECONDS;
    let mut keep_running = false;
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            value if crate::is_device_flag(value) => {
                host = Some(
                    arguments
                        .get(index + 1)
                        .ok_or("--device needs a host")?
                        .clone(),
                );
                index += 1;
            }
            "--seconds" => {
                let value = arguments.get(index + 1).ok_or("--seconds needs a number")?;
                seconds = value
                    .parse::<u64>()
                    .map_err(|_| format!("'{value}' is not a number of seconds"))?;
                index += 1;
            }
            "--keep-running" => keep_running = true,
            other if other.starts_with('-') => {
                return Err(format!("unknown option '{other}'\n{PRESENT_USAGE}"))
            }
            other if app.is_none() => app = Some(other.to_owned()),
            other => return Err(format!("unexpected argument '{other}'\n{PRESENT_USAGE}")),
        }
        index += 1;
    }

    let app = app.ok_or_else(|| format!("{PRESENT_USAGE}\n\n{}", installed_list()))?;
    let app = resolve_app(&app)?;
    let host = host.ok_or_else(|| format!("present needs a device\n{PRESENT_USAGE}"))?;
    if !crate::valid_device_host(&host) {
        return Err(format!("'{host}' is not a usable device host"));
    }
    if seconds == 0 {
        return Err("--seconds 0 would start a session and end it at once".to_owned());
    }
    if seconds > MAXIMUM_SECONDS {
        return Err(format!(
            "--seconds {seconds} is longer than the {MAXIMUM_SECONDS} second ceiling"
        ));
    }
    Ok(Present {
        app,
        host,
        seconds,
        keep_running,
    })
}

/// Accepts either the packaged name or the short one a developer thinks in.
///
/// `kobo present todo` and `kobo present kobo-todo` are the same reader and
/// the same screen, and having to remember which one the CLI wants is the
/// sort of friction that gets a shell alias written instead.
fn resolve_app(name: &str) -> Result<String, String> {
    let prefixed = format!("kobo-{name}");
    for packaged in presentable() {
        if packaged == name || packaged == prefixed {
            return Ok(packaged.to_owned());
        }
    }
    if name == "kobod" || prefixed == "kobod" {
        return Err("kobod is the runtime, not an application to present".to_owned());
    }
    Err(format!(
        "'{name}' is not an installed application\n{}",
        installed_list()
    ))
}

/// Every application that can be put on the panel, by package name.
///
/// An application released through Store is installed to the same place and
/// started the same way as one built in, so leaving those out only meant that
/// the applications most in need of a look on real glass were the ones that
/// could not be given one.
fn presentable() -> impl Iterator<Item = &'static str> {
    INSTALLED_PACKAGES
        .iter()
        .map(|(name, _)| *name)
        .chain(STORE_PACKAGES.iter().copied())
        .filter(|name| *name != "kobod")
}

fn installed_list() -> String {
    let mut names: Vec<&str> = presentable()
        .map(|name| name.trim_start_matches("kobo-"))
        .collect();
    names.sort_unstable();
    names.dedup();
    format!("installed applications: {}", names.join(" "))
}

/// Runs a script on the device, tolerating a reader whose radio is dozing.
///
/// A Kobo powers its Wi-Fi down between packets and brings it back when
/// something arrives, so the first connection after an idle minute is often
/// refused while the later ones succeed. Measured on a Clara BW: port 22 shut
/// on the first probe and open on the next two, three seconds apart.
///
/// Only a failure to reach the device is retried. A script that ran and said
/// no is an answer, and repeating it would turn one clear refusal into three.
pub(crate) fn run_remote_shell_waking(
    remote: &str,
    script: &str,
    timeout: Duration,
) -> Result<crate::RemoteShellOutput, String> {
    let mut last = String::new();
    for attempt in 0..CONNECT_ATTEMPTS {
        match run_remote_shell(remote, script, timeout) {
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                if output.status.success() || !is_unreachable(&stderr) {
                    return Ok(output);
                }
                stderr.trim().clone_into(&mut last);
            }
            Err(error) => {
                if !is_unreachable(&error) {
                    return Err(error);
                }
                last = error;
            }
        }
        if attempt + 1 < CONNECT_ATTEMPTS {
            std::thread::sleep(WAKE_INTERVAL);
        }
    }
    Err(format!(
        "{remote} did not answer in {CONNECT_ATTEMPTS} attempts: {last}\n\
         'kobo session --device' with --wifi-always-on on stops the reader powering \
         its radio down while you work."
    ))
}

/// Whether a message describes never having reached the device.
///
/// Host key verification is deliberately absent: it is a real answer from a
/// real device, and retrying it three times only delays the same message.
pub(crate) fn is_unreachable(message: &str) -> bool {
    const SIGNS: &[&str] = &[
        "Operation timed out",
        "Connection refused",
        "No route to host",
        "Connection reset",
        "Network is unreachable",
        "Connection closed by remote host",
        "timed out",
    ];
    SIGNS.iter().any(|sign| message.contains(sign))
}

/// The script that takes the panel, and gives it back first if it has to.
///
/// This is one script rather than a stop followed by a start because ending a
/// session restarts the stock reader, and the reader's Wi-Fi goes away while
/// it does. Two round trips meant the second one landing in that window and
/// timing out, having already stopped what was on the screen. Sent as one
/// script, the device does the waiting locally and the network is only needed
/// at the start and the end.
fn start_script(options: &Present) -> String {
    let binary = format!("{INSTALL_ROOT}/bin/{}", options.app);
    let daemon = format!("{INSTALL_ROOT}/bin/kobod");
    let log = session_log(&options.app);
    let refuse = if options.keep_running {
        "  echo 'a session is already running, and --keep-running says not to disturb it' >&2\n  \
         exit 1\n"
    } else {
        ""
    };
    format!(
        "test -x '{binary}' || {{ echo 'not installed: {binary}' >&2; exit 1; }}\n\
         if pidof kobod > /dev/null 2>&1; then\n\
         {refuse}\
           for pid in $(pidof kobod); do kill \"$pid\" 2>/dev/null || true; done\n\
           waited=0\n\
           while [ \"$waited\" -lt {SHUTDOWN_SECONDS} ]; do\n\
             pidof kobod > /dev/null 2>&1 || break\n\
             sleep 1\n\
             waited=$((waited + 1))\n\
           done\n\
         {settle}\
         fi\n\
         nohup setsid env {UNLOCK_NAME}={PRESENT_UNLOCK} {BLACKBOX_NAME}=1 timeout {seconds} '{daemon}' \
           --present '{binary}' > '{log}' 2>&1 < /dev/null &\n\
         sleep 2\n\
         pidof kobod > /dev/null 2>&1 || {{ echo 'the session did not start' >&2; \
           tail -5 '{log}' >&2; exit 1; }}\n\
         echo started\n",
        seconds = options.seconds,
        settle = format_args!("  sleep {SETTLE_SECONDS}\n"),
        UNLOCK_NAME = "KOBO_PRESENT_UNLOCK",
        // The trace is off by default because it writes to the card once per
        // event, which is not a cost an owner reading a book should pay. A
        // session driven from a development machine is the opposite case: it
        // exists to be watched, and the one time anybody wants the record is
        // after something hung and took the reader with it.
        BLACKBOX_NAME = "KOBO_BLACKBOX",
    )
}

/// Starts one application on the panel and returns while it is still running.
///
/// # Errors
///
/// When the device cannot be reached, the application is not installed on it,
/// or a session is already running and `--keep-running` was not given.
pub fn present(arguments: &[String]) -> Result<(), String> {
    let options = parse_present(arguments)?;
    let remote = format!("root@{}", options.host);
    let output = run_remote_shell_waking(&remote, &start_script(&options), START_TIMEOUT)?;
    if !output.status.success() {
        return Err(format!(
            "start {} on {}: {}",
            options.app,
            options.host,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    println!(
        "{} is on the panel of {} for {} seconds",
        options.app, options.host, options.seconds
    );
    println!(
        "the reader hands the panel back by itself when the time is up, even if this \
         machine goes away. 'kobo stop --device {}' ends it now.",
        options.host
    );
    Ok(())
}

/// Ends a running session immediately.
///
/// # Errors
///
/// When the device cannot be reached.
pub fn stop(arguments: &[String]) -> Result<(), String> {
    let mut host: Option<String> = None;
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            value if crate::is_device_flag(value) => {
                host = Some(
                    arguments
                        .get(index + 1)
                        .ok_or("--device needs a host")?
                        .clone(),
                );
                index += 1;
            }
            other => return Err(format!("unknown option '{other}'\n{STOP_USAGE}")),
        }
        index += 1;
    }
    let host = host.ok_or_else(|| format!("stop needs a device\n{STOP_USAGE}"))?;
    if !crate::valid_device_host(&host) {
        return Err(format!("'{host}' is not a usable device host"));
    }
    let remote = format!("root@{host}");
    if session_pids(&remote)?.is_empty() {
        println!("no session is running on {host}");
        return Ok(());
    }
    stop_session(&remote)?;
    println!("the panel is back with the reader on {host}");
    Ok(())
}

/// Where a session's output goes, so `kobo logs` and a failed start can find it.
fn session_log(app: &str) -> String {
    format!("/tmp/{app}.log")
}

/// The daemon's process ids on the device, empty when nothing is running.
fn session_pids(remote: &str) -> Result<Vec<String>, String> {
    let output = run_remote_shell_waking(remote, "pidof kobod || true\n", STOP_TIMEOUT)?;
    if !output.status.success() {
        return Err(format!(
            "ask {remote} what is running: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .map(str::to_owned)
        .collect())
}

/// Ends the session and waits for the daemon to actually be gone.
///
/// `kill` returns as soon as the signal is delivered, not when the process has
/// handed the panel back. Returning before that is what made an immediate
/// second `present` fail.
fn stop_session(remote: &str) -> Result<(), String> {
    let script = format!(
        "for pid in $(pidof kobod); do kill \"$pid\" 2>/dev/null || true; done\n\
         waited=0\n\
         while [ \"$waited\" -lt {SHUTDOWN_SECONDS} ]; do\n\
           pidof kobod > /dev/null 2>&1 || exit 0\n\
           sleep 1\n\
           waited=$((waited + 1))\n\
         done\n\
         echo 'the daemon is still running after {SHUTDOWN_SECONDS} seconds' >&2\n\
         exit 1\n"
    );
    let script = script.as_str();
    let output = run_remote_shell_waking(remote, script, STOP_TIMEOUT)?;
    if !output.status.success() {
        return Err(format!(
            "end the session on {remote}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{parse_present, resolve_app, DEFAULT_SECONDS, MAXIMUM_SECONDS};

    fn arguments(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn an_application_is_named_either_way_round() {
        assert_eq!(resolve_app("todo").unwrap(), "kobo-todo");
        assert_eq!(resolve_app("kobo-todo").unwrap(), "kobo-todo");
    }

    #[test]
    fn the_runtime_is_not_an_application() {
        let error = resolve_app("kobod").unwrap_err();
        assert!(error.contains("runtime"), "{error}");
    }

    #[test]
    fn an_unknown_application_is_answered_with_the_list_of_real_ones() {
        let error = resolve_app("kobo-spreadsheet").unwrap_err();
        assert!(error.contains("not an installed application"), "{error}");
        assert!(error.contains("todo"), "{error}");
    }

    #[test]
    fn a_session_has_a_length_even_when_none_is_asked_for() {
        let parsed = parse_present(&arguments(&["todo", "--device", "192.168.1.5"])).unwrap();
        assert_eq!(parsed.seconds, DEFAULT_SECONDS);
        assert_eq!(parsed.app, "kobo-todo");
        assert_eq!(parsed.host, "192.168.1.5");
        assert!(!parsed.keep_running);
    }

    #[test]
    fn the_application_may_come_after_the_device() {
        let parsed = parse_present(&arguments(&["--device", "192.168.1.5", "hn"])).unwrap();
        assert_eq!(parsed.app, "kobo-hn");
    }

    #[test]
    fn a_session_longer_than_the_ceiling_is_refused() {
        let over = (MAXIMUM_SECONDS + 1).to_string();
        let error = parse_present(&arguments(&[
            "todo",
            "-s",
            "192.168.1.5",
            "--seconds",
            &over,
        ]))
        .unwrap_err();
        assert!(error.contains("ceiling"), "{error}");
    }

    #[test]
    fn a_session_of_no_length_is_refused_rather_than_started() {
        let error = parse_present(&arguments(&["todo", "-s", "192.168.1.5", "--seconds", "0"]))
            .unwrap_err();
        assert!(error.contains("at once"), "{error}");
    }

    #[test]
    fn a_present_without_a_device_says_so() {
        let error = parse_present(&arguments(&["todo"])).unwrap_err();
        assert!(error.contains("needs a device"), "{error}");
    }

    #[test]
    fn a_dozing_radio_is_worth_another_try() {
        for message in [
            "ssh: connect to host 192.168.1.5 port 22: Operation timed out",
            "ssh: connect to host 192.168.1.5 port 22: Connection refused",
            "ssh: connect to host 192.168.1.5 port 22: No route to host",
            "remote shell session timed out after 90 seconds",
        ] {
            assert!(super::is_unreachable(message), "{message}");
        }
    }

    #[test]
    fn a_real_answer_is_not_retried() {
        for message in [
            "Host key verification failed.",
            "Permission denied (publickey).",
            "not installed: /mnt/onboard/.adds/cobalt/bin/kobo-todo",
            "the session did not start",
        ] {
            assert!(!super::is_unreachable(message), "{message}");
        }
    }

    #[test]
    fn an_unusable_host_is_refused_before_anything_is_run() {
        let error =
            parse_present(&arguments(&["todo", "-s", "192.168.1.5; rm -rf /"])).unwrap_err();
        assert!(error.contains("not a usable device host"), "{error}");
    }
}

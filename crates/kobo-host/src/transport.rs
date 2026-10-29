//! Running a script on a reader over SSH.
//!
//! Every command that changes something on a reader over Wi-Fi does it by
//! piping a shell script into `ssh` and reading back what it printed. This is
//! that one path: the command line that reaches the reader, the session that
//! feeds it a script within a time limit, and the error that carries the
//! reader's side of a failure.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// How long `ssh` waits for a reader to answer before giving up.
pub const CONNECT_TIMEOUT_SECONDS: u64 = 10;

/// Whether the reader should be given a terminal.
///
/// Every other caller wants `No`: a script is piped in and read back, and a
/// terminal would echo the script into the output. `Yes` exists for the one
/// verb that hands the session to a person, where line editing, job control
/// and a prompt are the whole point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tty {
    No,
    Yes,
}

/// An `ssh` invocation that will offer the reader's key.
///
/// Without `-i` this offered only the default identities, and the reader's key
/// is deliberately not one of those, so every connection failed on a reader
/// that was set up correctly. `IdentitiesOnly` prevents a busy SSH agent from
/// exhausting the reader's authentication attempts before this key is tried.
#[must_use]
pub fn ssh_command(remote: &str, tty: Tty, key: Option<&Path>) -> Command {
    let mut command = Command::new("ssh");
    command
        .args([
            if tty == Tty::Yes { "-t" } else { "-T" },
            "-o",
            "BatchMode=yes",
            "-o",
        ])
        .arg(format!("ConnectTimeout={CONNECT_TIMEOUT_SECONDS}"));
    if let Some(key) = key {
        command.args(["-o", "IdentitiesOnly=yes", "-i"]).arg(key);
    }
    command.arg(remote);
    command
}

/// What a script printed on the reader, and how it ended.
#[derive(Debug)]
pub struct Output {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Runs `script` on the far side of an `ssh` command the caller has shaped,
/// and collects what it printed.
///
/// # Errors
///
/// When `ssh` cannot start, the session outlives `timeout`, or the script
/// cannot be written for a reason other than the far side closing its input.
pub fn run_session(
    mut command: Command,
    script: &str,
    timeout: Duration,
) -> Result<Output, String> {
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("start remote shell: {error}"))?;
    let stdin_handle = child.stdin.take();
    let stdin = take_pipe(&mut child, stdin_handle, "stdin")?;
    let stdout_handle = child.stdout.take();
    let stdout = take_pipe(&mut child, stdout_handle, "stdout")?;
    let stderr_handle = child.stderr.take();
    let stderr = take_pipe(&mut child, stderr_handle, "stderr")?;
    let script = script.as_bytes().to_vec();
    let writer = thread::spawn(move || -> std::io::Result<()> {
        let mut stdin = stdin;
        stdin.write_all(&script)?;
        stdin.flush()
    });
    let stdout_reader = thread::spawn(move || read_pipe(stdout));
    let stderr_reader = thread::spawn(move || read_pipe(stderr));
    let status = wait_for_child(&mut child, "remote shell session", timeout);
    let writer_result = writer
        .join()
        .map_err(|_| "remote shell stdin writer panicked".to_owned())?;
    let stdout = stdout_reader
        .join()
        .map_err(|_| "remote shell stdout reader panicked".to_owned())?
        .map_err(|error| format!("read remote stdout: {error}"))?;
    let stderr = stderr_reader
        .join()
        .map_err(|_| "remote shell stderr reader panicked".to_owned())?
        .map_err(|error| format!("read remote stderr: {error}"))?;
    let status = status.map_err(|error| session_error(error, &stdout, &stderr))?;
    if let Err(error) = writer_result {
        // A script that decides not to read the rest of its input (because it
        // refused, or because it ended in `exec`) closes the pipe under this
        // writer, and that is not a transport failure. The device answered;
        // its status and its stderr are what the caller has to be told, and
        // reporting a broken pipe instead buries a plain refusal under advice
        // about Wi-Fi and sleeping readers.
        if error.kind() != std::io::ErrorKind::BrokenPipe {
            return Err(session_error(
                format!("write remote script: {error}"),
                &stdout,
                &stderr,
            ));
        }
    }
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

/// `message`, followed by whatever the session printed, for an error that
/// has to show the reader's side of a failure.
#[must_use]
pub fn session_error(message: String, stdout: &[u8], stderr: &[u8]) -> String {
    let stdout = String::from_utf8_lossy(stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(stderr).trim().to_owned();
    let mut result = message;
    if !stdout.is_empty() {
        result.push_str("; stdout: ");
        result.push_str(&stdout);
    }
    if !stderr.is_empty() {
        result.push_str("; stderr: ");
        result.push_str(&stderr);
    }
    result
}

fn read_pipe(mut pipe: impl Read) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    pipe.read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// A pipe the child was started with, or an error that has already stopped
/// the child when the pipe was not captured.
///
/// # Errors
///
/// When `pipe` is `None`.
pub fn take_pipe<T>(child: &mut Child, pipe: Option<T>, name: &str) -> Result<T, String> {
    pipe.ok_or_else(|| {
        terminate(child);
        format!("start remote shell: {name} was not captured")
    })
}

/// Stops a child and reaps it, ignoring a child that has already gone.
pub fn terminate(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Waits for a child, stopping it once `timeout` has passed.
///
/// # Errors
///
/// When the child cannot be inspected, or outlives `timeout`.
pub fn wait_for_child(
    child: &mut Child,
    description: &str,
    timeout: Duration,
) -> Result<ExitStatus, String> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {}
            Err(error) => {
                terminate(child);
                return Err(format!("{description}: inspect child: {error}"));
            }
        }
        if Instant::now() >= deadline {
            terminate(child);
            return Err(format!(
                "{description} timed out after {} seconds",
                timeout.as_secs()
            ));
        }
        thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::run_session;
    use super::{ssh_command, Tty};
    use std::path::Path;
    #[cfg(unix)]
    use std::time::Duration;

    #[test]
    fn the_reader_key_is_offered_alone_and_only_when_there_is_one() {
        let arguments = |key: Option<&Path>| {
            ssh_command("root@reader", Tty::No, key)
                .get_args()
                .map(|argument| argument.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        };
        let keyed = arguments(Some(Path::new("/home/owner/.ssh/kobo_cobalt")));
        assert!(keyed
            .windows(2)
            .any(|pair| pair == ["-o", "IdentitiesOnly=yes"]));
        assert!(keyed.windows(2).any(|pair| pair[0] == "-i"));
        assert_eq!(keyed.last().map(String::as_str), Some("root@reader"));
        let unkeyed = arguments(None);
        assert!(!unkeyed.iter().any(|argument| argument == "-i"));
        assert_eq!(unkeyed.first().map(String::as_str), Some("-T"));
    }

    // The sessions below stand a local `sh` in for `ssh`, which reads the
    // script the same way: from its standard input.
    #[cfg(unix)]
    #[test]
    fn a_script_runs_and_its_output_and_status_come_back() {
        let output = run_session(
            std::process::Command::new("sh"),
            "echo out; echo err >&2; exit 3\n",
            Duration::from_secs(10),
        )
        .expect("a session");
        assert_eq!(output.status.code(), Some(3));
        assert_eq!(output.stdout, b"out\n");
        assert_eq!(output.stderr, b"err\n");
    }

    #[cfg(unix)]
    #[test]
    fn a_script_that_stops_reading_its_input_is_not_a_transport_failure() {
        let mut command = std::process::Command::new("sh");
        command.args(["-c", "echo refused >&2; exit 1"]);
        let unread = "true\n".repeat(200_000);
        let output =
            run_session(command, &unread, Duration::from_secs(10)).expect("the reader answered");
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(output.stderr, b"refused\n");
    }

    #[cfg(unix)]
    #[test]
    fn a_session_that_outlives_its_limit_is_stopped_and_says_so() {
        // `exec`, so the process stopped is the one holding the pipes, as
        // `ssh` is; a shell's own child would keep them open after it.
        let error = run_session(
            std::process::Command::new("sh"),
            "exec sleep 5\n",
            Duration::from_millis(100),
        )
        .expect_err("timed out");
        assert!(error.contains("timed out"), "{error}");
    }
}

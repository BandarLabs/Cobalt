//! Bounded private pipes for firmware Wi-Fi tools. No command text is logged.
use std::io::{self, Read, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const MAX_OUTPUT: usize = 64 * 1024;

struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// What a tool printed, and whether it exited successfully.
///
/// Kept together rather than turning a failed exit into an error, because
/// `wpa_cli` builds differ on whether a `FAIL-BUSY` reply exits non-zero, and
/// that reply means a scan is already running, which the caller must be able
/// to read whatever the exit status says.
pub(super) struct Reply {
    pub text: String,
    pub success: bool,
}

/// Drains the output pipe while polling the child, so a descendant that keeps
/// the pipe open cannot hold the caller past `timeout`.
pub(super) fn run(command: &mut Command, input: &[u8], timeout: Duration) -> io::Result<Reply> {
    let deadline = Instant::now() + timeout;
    let mut child = OwnedChild(
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?,
    );
    let mut stdin = child.0.stdin.take();
    let mut stdout = child
        .0
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("missing output pipe"))?;
    kobo_abi::set_nonblocking(&stdout)?;
    kobo_abi::set_nonblocking(
        stdin
            .as_ref()
            .ok_or_else(|| io::Error::other("missing input pipe"))?,
    )?;
    let mut sent = 0;
    let mut output = Vec::new();
    loop {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Wi-Fi tool deadline",
            ));
        }
        if sent < input.len() {
            match stdin
                .as_mut()
                .expect("input pipe retained until sent")
                .write(&input[sent..])
            {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "closed input pipe",
                    ))
                }
                Ok(count) => sent += count,
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) => {}
                Err(e) => return Err(e),
            }
        }
        if sent == input.len() {
            stdin.take();
        }
        // One bounded read per iteration prevents a noisy child starving the deadline.
        let mut buffer = [0; 4096];
        let eof = match stdout.read(&mut buffer) {
            Ok(0) => true,
            Ok(count) => {
                if output.len() + count > MAX_OUTPUT {
                    return Err(io::Error::other("Wi-Fi output limit"));
                }
                output.extend_from_slice(&buffer[..count]);
                false
            }
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) =>
            {
                false
            }
            Err(e) => return Err(e),
        };
        if let Some(status) = child.0.try_wait()? {
            if eof {
                return Ok(Reply {
                    text: String::from_utf8_lossy(&output).into_owned(),
                    success: status.success(),
                });
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn shell(script: &str) -> Command {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", script]);
        command
    }
    #[test]
    fn drains_output_and_sends_private_input() {
        let reply = run(&mut shell("cat"), b"private\n", Duration::from_secs(1)).unwrap();
        assert_eq!(reply.text, "private\n");
        assert!(reply.success);
    }
    #[test]
    fn bounds_stalled_input_and_retained_output() {
        for (script, input) in [
            ("sleep 1", vec![b'x'; 1024 * 1024]),
            ("sleep 1 & exit 0", Vec::new()),
        ] {
            let start = Instant::now();
            assert!(run(&mut shell(script), &input, Duration::from_millis(40)).is_err());
            assert!(start.elapsed() < Duration::from_millis(800));
        }
    }
    #[test]
    fn a_failed_exit_still_hands_back_what_the_tool_said() {
        let reply = run(
            &mut shell("printf 'FAIL-BUSY\\n'; exit 255"),
            b"",
            Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(reply.text, "FAIL-BUSY\n");
        assert!(!reply.success);
    }
    #[test]
    fn bounds_excess_output() {
        assert!(run(&mut shell("yes output"), b"", Duration::from_secs(1)).is_err());
    }
}

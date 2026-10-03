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

/// Drain pipes while polling the child, including when a descendant retains a
/// pipe. A monitor keeps stdin open until its completion predicate succeeds.
pub(super) fn run(
    command: &mut Command,
    input: &[u8],
    timeout: Duration,
    monitor: Option<fn(&str) -> bool>,
) -> io::Result<String> {
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
        if sent == input.len() && monitor.is_none() {
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
        let text = String::from_utf8_lossy(&output);
        if monitor.is_some_and(|done| done(&text)) {
            return Ok(text.into_owned());
        }
        if let Some(status) = child.0.try_wait()? {
            if eof {
                return if status.success() && monitor.is_none() {
                    Ok(text.into_owned())
                } else {
                    Err(io::Error::other("Wi-Fi tool failed"))
                };
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
        assert_eq!(
            run(
                &mut shell("cat"),
                b"private\n",
                Duration::from_secs(1),
                None
            )
            .unwrap(),
            "private\n"
        );
    }
    #[test]
    fn bounds_stalled_input_and_retained_output() {
        for (script, input) in [
            ("sleep 1", vec![b'x'; 1024 * 1024]),
            ("sleep 1 & exit 0", Vec::new()),
        ] {
            let start = Instant::now();
            assert!(run(&mut shell(script), &input, Duration::from_millis(40), None).is_err());
            assert!(start.elapsed() < Duration::from_millis(800));
        }
    }
    #[test]
    fn monitor_waits_for_completion_and_reaps_its_child() {
        let marker = std::env::temp_dir().join(format!("cobalt-wifi-child-{}", std::process::id()));
        let mut command = shell("echo $$ > \"$PID_FILE\"; read request; printf 'OK\\n'; sleep 0.03; printf '<3>CTRL-EVENT-SCAN-RESULTS\\n'; exec sleep 60");
        command.env("PID_FILE", &marker);
        let output = run(
            &mut command,
            b"scan\n",
            Duration::from_secs(1),
            Some(crate::wifi::scan_finished),
        )
        .unwrap();
        assert!(output.contains("CTRL-EVENT-SCAN-RESULTS"));
        let pid = std::fs::read_to_string(&marker).unwrap();
        assert!(!std::path::Path::new(&format!("/proc/{}", pid.trim())).exists());
        std::fs::remove_file(marker).unwrap();
    }

    #[test]
    fn bounds_excess_output_and_rejects_exit_failure() {
        assert!(run(&mut shell("yes output"), b"", Duration::from_secs(1), None).is_err());
        assert!(run(&mut shell("exit 1"), b"", Duration::from_secs(1), None).is_err());
    }
}

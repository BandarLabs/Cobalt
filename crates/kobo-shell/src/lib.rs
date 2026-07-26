//! One application's terminal, hosted by the runtime.
//!
//! The application never holds the pseudo-terminal, never forks a process and
//! never sees a file descriptor. It sends [`ShellRequest`] and receives
//! [`ShellEvent`], exactly the way it reaches the network and its own state.
//! That is not ceremony: it is the only place a refusal can be enforced, a
//! chunk can be bounded, and a program can be stopped when its application
//! goes away.
//!
//! The same code runs in the daemon and in the browser simulator, so a
//! terminal behaves the same on a development host as on the panel and a
//! difference between them cannot be introduced by having written it twice.
//!
//! # Why this is the dangerous one
//!
//! Every other capability in this platform is undone by a reboot. A shell on
//! this device is root on a writable root filesystem, so it is the first
//! feature capable of producing a device that a power cycle does not repair.
//! It is refused unless the application declared [`Capability::Shell`], and a
//! runtime that has no business offering one constructs [`Shells::refused`].

use kobo_abi::pty::{Pty, Wake};
use kobo_policy::Capability;
use kobo_protocol::{ShellError, ShellEvent, ShellRequest, MAX_SHELL_CHUNK};
use std::sync::mpsc::TryRecvError;

/// The program a terminal starts.
///
/// The stock shell, because it is the one that is certainly present on the
/// device and the one whose behaviour the owner can look up. Nothing is
/// shipped to the device to support it.
const PROGRAM: &str = "/bin/sh";

/// What the shell is told about the world.
///
/// The environment is built rather than inherited. The runtime's own
/// environment on this device came from the stock reader and carries its
/// session bus address among other things, none of which a shell should see.
///
/// `TERM` is `vt100` and not something richer because this device has no
/// terminfo database at all: a name nothing can look up leaves programs
/// guessing, and `vt100` is the one every program falls back to anyway. It is
/// also exactly the dialect the runtime's own parser implements, so a program
/// cannot ask for a capability the panel could not draw.
const ENVIRONMENT: [(&str, &str); 5] = [
    ("TERM", "vt100"),
    ("PATH", "/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"),
    ("HOME", "/root"),
    ("SHELL", PROGRAM),
    ("PS1", "$ "),
];

/// The largest grid a terminal may be opened with.
///
/// A guard against a nonsense request rather than a product decision: the
/// panel decides the real grid, and a program told it has 60,000 columns
/// allocates accordingly.
const MAX_GRID: u16 = 512;

/// One application's terminal, or its absence.
pub struct Shells {
    permitted: bool,
    open: Option<Pty>,
    wake: Option<Wake>,
}

impl Shells {
    /// A terminal host for an application holding `capabilities`.
    #[must_use]
    pub fn new(capabilities: &[Capability]) -> Self {
        Self {
            permitted: capabilities.contains(&Capability::Shell),
            open: None,
            wake: None,
        }
    }

    /// A host that refuses everything, for a runtime with no terminal to give.
    #[must_use]
    pub const fn refused() -> Self {
        Self {
            permitted: false,
            open: None,
            wake: None,
        }
    }

    /// Calls `wake` whenever the program has printed something.
    ///
    /// A runtime that drains only when it wakes for its own reasons would show
    /// a keystroke's echo at its next heartbeat, which for a terminal is the
    /// difference between typing and waiting. The hook is called from the
    /// reader thread and must only nudge the loop; the draining still happens
    /// in the loop, where the events can be delivered.
    #[must_use]
    pub fn waking(mut self, wake: Wake) -> Self {
        self.wake = Some(wake);
        self
    }

    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// Applies one request and returns anything the application must be told.
    ///
    /// A refusal is *always* reported, because an application that asked for
    /// something and heard nothing back cannot tell a denied request from a
    /// slow one and would wait forever for output that is never coming.
    /// Successful typing reports nothing: the answer to a keystroke is what
    /// the program prints, and an acknowledgement for every character would be
    /// a message per key for no information at all.
    pub fn handle(&mut self, request: ShellRequest) -> Option<ShellEvent> {
        if !self.permitted {
            return Some(ShellEvent::Refused(ShellError::NotPermitted));
        }
        match request {
            ShellRequest::Open { columns, rows } => Some(self.open(columns, rows)),
            ShellRequest::Input(bytes) => self.input(&bytes),
            ShellRequest::Resize { columns, rows } => self.resize(columns, rows),
            ShellRequest::Close => Some(self.close()),
        }
    }

    fn open(&mut self, columns: u16, rows: u16) -> ShellEvent {
        if self.open.is_some() {
            return ShellEvent::Refused(ShellError::AlreadyOpen);
        }
        let columns = columns.clamp(1, MAX_GRID);
        let rows = rows.clamp(1, MAX_GRID);
        match Pty::spawn_with_wake(PROGRAM, &[], &ENVIRONMENT, columns, rows, self.wake.clone()) {
            Ok(pty) => {
                self.open = Some(pty);
                ShellEvent::Opened
            }
            Err(_) => ShellEvent::Refused(ShellError::Failed),
        }
    }

    fn input(&mut self, bytes: &[u8]) -> Option<ShellEvent> {
        let Some(pty) = self.open.as_mut() else {
            return Some(ShellEvent::Refused(ShellError::NotOpen));
        };
        if bytes.len() > MAX_SHELL_CHUNK {
            return Some(ShellEvent::Refused(ShellError::Failed));
        }
        match pty.write(bytes) {
            Ok(()) => None,
            // A write that fails means the program has gone. Reporting it as
            // closed is the truth; reporting a failure would leave the
            // application waiting for output that can never arrive.
            Err(_) => Some(self.close()),
        }
    }

    fn resize(&mut self, columns: u16, rows: u16) -> Option<ShellEvent> {
        let Some(pty) = self.open.as_mut() else {
            return Some(ShellEvent::Refused(ShellError::NotOpen));
        };
        match pty.resize(columns.clamp(1, MAX_GRID), rows.clamp(1, MAX_GRID)) {
            Ok(()) => None,
            Err(_) => Some(ShellEvent::Refused(ShellError::Failed)),
        }
    }

    /// Ends the program, whether it wanted to end or not.
    ///
    /// Always reports closed rather than an error. Once this returns there is
    /// no terminal, which is exactly what the caller asked for, and a failure
    /// to reap something already dead is not news.
    pub fn close(&mut self) -> ShellEvent {
        let status = self.open.take().map_or(-1, |mut pty| {
            let status = pty.finished().ok().flatten();
            let _ignored = pty.close();
            status.unwrap_or(-1)
        });
        ShellEvent::Closed { status }
    }

    /// Collects everything the program has printed since the last call, and
    /// notices if it has finished.
    ///
    /// Never blocks. The runtime's event loop has a panel and other
    /// applications to serve, and a terminal that could stall it would make
    /// the whole device feel broken whenever a program stopped printing.
    #[must_use]
    pub fn drain(&mut self) -> Vec<ShellEvent> {
        let mut events = Vec::new();
        let Some(pty) = self.open.as_mut() else {
            return events;
        };
        let mut ended = false;
        // Everything already waiting becomes as few events as the bound allows.
        // A program printing a line at a time would otherwise produce an event
        // per line, and an application that repaints per event would then ask
        // the panel for a refresh per line, which on E Ink is the difference
        // between a screen that updates and a screen that flashes.
        let mut pending: Vec<u8> = Vec::new();
        loop {
            match pty.output().try_recv() {
                Ok(chunk) => pending.extend_from_slice(&chunk),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    ended = true;
                    break;
                }
            }
        }
        // Split rather than truncated. Losing the tail would corrupt an escape
        // sequence and leave the parser interpreting everything after it in
        // the wrong state.
        for piece in pending.chunks(MAX_SHELL_CHUNK) {
            events.push(ShellEvent::Output(piece.to_vec()));
        }
        if ended || matches!(pty.finished(), Ok(Some(_))) {
            events.push(self.close());
        }
        events
    }
}

impl Drop for Shells {
    /// An application that goes away takes its program with it.
    ///
    /// Without this, closing a terminal application would leave a root shell
    /// running on the device with nothing attached to it, which is precisely
    /// the kind of thing this platform exists not to do.
    fn drop(&mut self) {
        if self.open.is_some() {
            let _ignored = self.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Shells;
    use kobo_policy::Capability;
    use kobo_protocol::{ShellError, ShellEvent, ShellRequest};
    use std::time::{Duration, Instant};

    fn permitted() -> Shells {
        Shells::new(&[Capability::Shell])
    }

    /// Drains until `needle` shows up in the output, or patience runs out.
    fn wait_for(shells: &mut Shells, needle: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut seen = String::new();
        while Instant::now() < deadline && !seen.contains(needle) {
            for event in shells.drain() {
                if let ShellEvent::Output(bytes) = event {
                    seen.push_str(&String::from_utf8_lossy(&bytes));
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        seen
    }

    #[test]
    fn an_application_without_the_capability_is_refused_before_anything_starts() {
        // Refused at the request, not at the spawn: nothing must fork before
        // the permission has been checked.
        let mut shells = Shells::new(&[Capability::Network]);
        assert_eq!(
            shells.handle(ShellRequest::Open {
                columns: 53,
                rows: 20
            }),
            Some(ShellEvent::Refused(ShellError::NotPermitted))
        );
        assert!(!shells.is_open());
    }

    #[test]
    fn a_runtime_with_no_terminal_to_give_refuses_even_a_permitted_application() {
        let mut shells = Shells::refused();
        assert_eq!(
            shells.handle(ShellRequest::Close),
            Some(ShellEvent::Refused(ShellError::NotPermitted))
        );
    }

    #[test]
    fn typing_at_a_terminal_produces_what_the_program_printed() {
        let mut shells = permitted();
        assert_eq!(
            shells.handle(ShellRequest::Open {
                columns: 53,
                rows: 20
            }),
            Some(ShellEvent::Opened)
        );
        shells.handle(ShellRequest::Input(b"echo COBALT_SHELL\n".to_vec()));
        let seen = wait_for(&mut shells, "COBALT_SHELL");
        assert!(seen.contains("COBALT_SHELL"), "saw {seen:?}");
    }

    #[test]
    fn a_second_terminal_is_refused_rather_than_replacing_the_first() {
        // Replacing it would kill a program the reader is in the middle of
        // using, on nothing more than a duplicate message.
        let mut shells = permitted();
        shells.handle(ShellRequest::Open {
            columns: 53,
            rows: 20,
        });
        assert_eq!(
            shells.handle(ShellRequest::Open {
                columns: 53,
                rows: 20
            }),
            Some(ShellEvent::Refused(ShellError::AlreadyOpen))
        );
    }

    #[test]
    fn typing_before_there_is_a_terminal_is_refused() {
        let mut shells = permitted();
        assert_eq!(
            shells.handle(ShellRequest::Input(b"ls\n".to_vec())),
            Some(ShellEvent::Refused(ShellError::NotOpen))
        );
    }

    #[test]
    fn a_program_that_exits_is_reported_closed_exactly_once() {
        let mut shells = permitted();
        shells.handle(ShellRequest::Open {
            columns: 53,
            rows: 20,
        });
        shells.handle(ShellRequest::Input(b"exit 0\n".to_vec()));
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut closes = 0;
        while Instant::now() < deadline {
            for event in shells.drain() {
                if matches!(event, ShellEvent::Closed { .. }) {
                    closes += 1;
                }
            }
            if closes > 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(closes, 1);
        assert!(!shells.is_open());
        // And nothing further, because there is no terminal left to drain.
        assert!(shells.drain().is_empty());
    }

    #[test]
    fn the_program_is_told_the_grid_the_panel_actually_has() {
        // The whole reason the grid travels with the open request: a program
        // that assumes eighty columns draws off the side of this panel.
        let mut shells = permitted();
        shells.handle(ShellRequest::Open {
            columns: 53,
            rows: 37,
        });
        shells.handle(ShellRequest::Input(b"stty size\n".to_vec()));
        let seen = wait_for(&mut shells, "37 53");
        assert!(seen.contains("37 53"), "saw {seen:?}");
    }

    #[test]
    fn closing_twice_is_harmless() {
        let mut shells = permitted();
        shells.handle(ShellRequest::Open {
            columns: 53,
            rows: 20,
        });
        assert!(matches!(
            shells.handle(ShellRequest::Close),
            Some(ShellEvent::Closed { .. })
        ));
        assert!(matches!(
            shells.handle(ShellRequest::Close),
            Some(ShellEvent::Closed { .. })
        ));
    }
}

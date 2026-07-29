//! Kobo's freeze watchdog.
//!
//! `sickel` registers `com.kobo.watchdog.Sickel` on the session bus and expects
//! the reader to call `Ping` on it. When the pings stop it concludes the reader
//! has hung, kills it, writes `/mnt/onboard/.kobo/sickel_frozen`, and **reboots
//! the device**.
//!
//! From its point of view a reader we stopped deliberately and a reader that
//! crashed look exactly the same, so owning the panel for any length of time
//! means telling it first. This was found the hard way: a five second handoff
//! completed normally, and a ninety second session rebooted the device
//! mid-run.
//!
//! Rather than kill the watchdog, we use the interface Kobo already built for
//! this. The service exposes exactly three methods, `Suspend`, `Ping` and
//! `Resume`, and suspending is plainly the supported way to hold it off during
//! work that legitimately stops the reader. Killing it would leave the device
//! with no freeze protection at all and no way to restore it short of a reboot;
//! suspending is reversible by a single call.
//!
//! The session bus address is taken from the reader's own environment rather
//! than guessed, because the address contains a per-boot socket path.

use std::ffi::OsStr;
use std::fmt;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

/// The well-known name the watchdog registers.
const SERVICE: &str = "com.kobo.watchdog.Sickel";
/// The object path it exports. Confirmed by introspection on the device.
const OBJECT: &str = "/";
const SUSPEND: &str = "com.kobo.watchdog.Sickel.Suspend";
const RESUME: &str = "com.kobo.watchdog.Sickel.Resume";
const PING: &str = "com.kobo.watchdog.Sickel.Ping";
/// Present on the device; the platform ships no D-Bus client of its own.
const DBUS_SEND: &str = "/bin/dbus-send";
const DBUS_MONITOR: &str = "/bin/dbus-monitor";

#[derive(Debug)]
pub enum SupervisorError {
    /// The reader's environment carried no session bus address, so there is no
    /// way to reach the watchdog.
    NoSessionBus,
    /// The call was made but the watchdog did not accept it.
    CallFailed {
        method: &'static str,
        detail: String,
    },
}

impl fmt::Display for SupervisorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSessionBus => write!(
                formatter,
                "the reader's environment has no DBUS_SESSION_BUS_ADDRESS, so the freeze watchdog cannot be reached"
            ),
            Self::CallFailed { method, detail } => {
                write!(formatter, "{method} was refused: {detail}")
            }
        }
    }
}

impl std::error::Error for SupervisorError {}

/// A suspended freeze watchdog.
///
/// Holding this value means the device will not reboot itself because the
/// reader is missing. It must be resumed explicitly; release builds abort on
/// panic, so a `Drop` guard would not run when it mattered most.
pub struct Suspended {
    session_bus: String,
    /// Cleared to stop the keep-alive thread.
    keep_alive: Arc<AtomicBool>,
}

/// How often the suspension is re-asserted.
///
/// `Suspend` is `QTimer::stop()`, so one call ought to be enough forever. On
/// real hardware it is not: with the reader stopped and the watchdog suspended,
/// the device still rebooted itself, and the device's own syslog names the
/// culprit exactly.
///
/// ```text
/// 785.19  sickel: trying to kill nickel
/// 786.19  sickel: QProcess ... "/usr/bin/killall"
/// 795.20  sickel: rebooting
/// ```
///
/// Sixty-eight seconds after the reader was stopped, and then ten more, which
/// is precisely the `QProcess::waitForFinished(10000)` in `SickelService::
/// reboot`. So the timer was running again despite the suspension.
///
/// Watching the session bus through a restart says who restarts it, and it is
/// the reader. A reader we have just started calls `Suspend` and `Resume`
/// alternately about fifteen times inside a second and a half, and the last
/// call in that burst is always `Resume`:
///
/// ```text
/// [776.70] :1.7 Suspend    [776.72] :1.7 Resume
/// [776.74] :1.7 Suspend    [776.75] :1.7 Resume
/// ...
/// [778.05] :1.7 Suspend    [778.06] :1.7 Resume
/// ```
///
/// So the reader arms the freeze watchdog against itself while it is still
/// starting, and it has no idea anyone else suspended it. Nothing we can say
/// once makes that stop, which is why this is a rhythm and not a call.
///
/// The interval is the margin. `Resume` is `QTimer::start(10000)`, so a
/// `Resume` landing just after one of ours burns the interval out of a ten
/// second fuse before we stop the timer again. At the five seconds this used
/// to be, half the fuse was gone before we did anything, and one slow or
/// refused call spent the rest: the device idles at a load average around
/// five, and that is exactly the intermittency that made a session reboot the
/// reader some runs and not others.
const KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(2);

/// How long a single watchdog call may take before it is treated as failed.
///
/// Comfortably longer than the round trip on the device, comfortably shorter
/// than the interval between keep-alive calls, so a stuck call can never queue
/// up behind the next one.
const REPLY_TIMEOUT_MS: u32 = 1_500;

/// How long `Resume` gives the reader to feed the watchdog before it fires.
///
/// `Resume` is `QTimer::start(10000)` and `Ping` restarts that same timer, both
/// read out of `sickel`. Every margin in this file is measured against it.
const FUSE: Duration = Duration::from_secs(10);

/// How close together two pings must be to count as a heartbeat.
///
/// The reader pings every five seconds when it is healthy, so a second ping
/// inside the fuse is the weakest evidence that still distinguishes "being
/// fed" from "pinged once on the way up". Anything stricter would reject a
/// reader that is merely busy; anything looser would arm the watchdog against
/// a reader that cannot keep up with it, which is the failure this exists to
/// prevent.
const HEARTBEAT_WINDOW: Duration = FUSE;

impl Suspended {
    /// Asks the watchdog to stand down.
    ///
    /// `session_bus` is the `DBUS_SESSION_BUS_ADDRESS` value taken from the
    /// reader's environment.
    ///
    /// # Errors
    ///
    /// Returns an error when there is no session bus or the call is refused.
    /// Callers must treat that as fatal and not stop the reader, because the
    /// device would reboot partway through the session.
    pub fn suspend(session_bus: Option<&OsStr>) -> Result<Self, SupervisorError> {
        let session_bus = session_bus
            .and_then(|value| value.to_str())
            .filter(|value| !value.is_empty())
            .ok_or(SupervisorError::NoSessionBus)?
            .to_owned();
        call(&session_bus, SUSPEND).map_err(|detail| SupervisorError::CallFailed {
            method: "Suspend",
            detail,
        })?;
        // Only the first call is allowed to fail the session. A later one
        // failing is not worth aborting a running session for, and the session
        // watchdog already covers the case where everything stops working.
        let keep_alive = Arc::new(AtomicBool::new(true));
        let running = Arc::clone(&keep_alive);
        let bus = session_bus.clone();
        thread::spawn(move || {
            while running.load(Ordering::Relaxed) {
                thread::sleep(KEEP_ALIVE_INTERVAL);
                if !running.load(Ordering::Relaxed) {
                    return;
                }
                // Two calls, because they cover different failures and neither
                // covers both.
                //
                // `Suspend` stops the timer, which is the state we want. But
                // the reader re-arms it faster than any interval can close,
                // so between its `Resume` and our next `Suspend` there is
                // always a live fuse. `Ping` restarts that fuse, which is
                // exactly what the reader would be doing if it were up, and
                // standing in for the reader is precisely our job while we
                // are holding its panel.
                //
                // Pinging a suspended watchdog is harmless: the timer is
                // stopped, and `Ping` restarting a stopped timer does nothing.
                // So the pair is safe in either state, which is the point.
                let _ignored = call(&bus, SUSPEND);
                let _ignored = call(&bus, PING);
            }
        });
        Ok(Self {
            session_bus,
            keep_alive,
        })
    }

    /// Stops re-asserting the suspension.
    ///
    /// Called before resuming, so the keep-alive cannot re-suspend a watchdog
    /// we have just deliberately handed back.
    fn stop_keeping_alive(&self) {
        self.keep_alive.store(false, Ordering::Relaxed);
    }

    /// Puts the freeze watchdog back on duty.
    ///
    /// # Errors
    ///
    /// Returns an error when the call is refused. The device is then left
    /// without freeze protection until it is next rebooted, which is a
    /// degradation rather than a hazard, but it must be reported rather than
    /// swallowed.
    pub fn resume(&self) -> Result<(), SupervisorError> {
        self.stop_keeping_alive();
        call(&self.session_bus, RESUME).map_err(|detail| SupervisorError::CallFailed {
            method: "Resume",
            detail,
        })
    }

    /// Puts the watchdog back on duty, but not before the reader can feed it.
    ///
    /// Handing it back to a reader that is still starting is what caused the
    /// reboots at the end of a session, and the numbers say why. Disassembling
    /// `sickel` shows `Resume` is `QTimer::start(10000)` and `Ping` restarts
    /// that same ten second fuse, and watching the session bus shows the
    /// reader pings every five seconds. A reader we have just restarted takes
    /// far longer than ten seconds to reach its first ping, so resuming the
    /// moment the process exists lights a fuse nothing is feeding.
    ///
    /// Waiting is safe in the other direction, which is what makes this the
    /// right shape: `Suspend` is `QTimer::stop()`, so a suspended watchdog has
    /// no running timer and cannot fire at all. Dying during the wait leaves
    /// the device with no freeze protection until its next reboot, which is a
    /// degradation, rather than rebooting it, which is a failure.
    ///
    /// So rather than sleep for a guessed interval, this watches the bus for
    /// the reader's own ping and resumes as soon as it is feeding steadily.
    ///
    /// The watchdog is armed **only** on evidence that something is feeding it.
    ///
    /// This function used to arm it regardless, on the reasoning that if the
    /// reader was not running then "a reboot to stock is the correct outcome".
    /// That reasoning cost a reader their device. A freeze watchdog firing is
    /// not a reboot: it is an `SoC` reset with nothing flushed and no filesystem
    /// sync. Arm it against a reader that is not pinging and it fires roughly
    /// ten seconds later, every time, forever -- and some of those resets land
    /// while the reader is part-way through writing its library database.
    /// A corrupt `KoboReader.sqlite` is what makes the device erase itself and
    /// come up asking for a language, which is precisely what happened.
    ///
    /// So the choice is between leaving the device without freeze protection
    /// until its next reboot, and hard-resetting it on a ten second loop. This
    /// file already had the right principle written down one paragraph up --
    /// "a degradation, rather than rebooting it, which is a failure" -- and
    /// simply failed to apply it here.
    ///
    /// It then failed to apply it thoroughly enough. Arming on the reader's
    /// *first* ping is still arming against a reader that is not yet keeping
    /// up: a restarted reader pings while it is starting and then goes quiet
    /// for longer than the fuse while it scans its library, and the device
    /// rebooted about ten seconds after every session. The evidence has to be
    /// a rhythm, so `wait_for_heartbeat` wants two pings from the same sender
    /// inside the fuse before this arms anything.
    ///
    /// # Errors
    ///
    /// Returns an error when the resume call itself is refused.
    pub fn resume_once_fed(&self, wait: Duration) -> Result<ResumedAfter, SupervisorError> {
        let observed = wait_for_heartbeat(&self.session_bus, wait);
        // Stopped only now. Waiting for the reader's first ping is exactly the
        // window in which the suspension still has to hold.
        self.stop_keeping_alive();
        if observed.armed() {
            self.resume()?;
        }
        // Otherwise it stays suspended. `Suspend` is `QTimer::stop()`, so a
        // suspended watchdog has no running timer and cannot fire; the device
        // is merely unprotected against a freeze, and the next reboot -- or
        // the reader resuming it once it is genuinely up -- restores that.
        Ok(observed)
    }
}

/// What was decided about the freeze watchdog on the way out.
///
/// Only the first of these arms it. The other two leave it suspended, because
/// they are the cases where nothing is known to be feeding it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResumedAfter {
    /// The reader was seen feeding the watchdog, so it was safe to arm.
    ReaderPinged,
    /// No ping was seen in the time allowed, so it was left suspended.
    NoPingSeen,
    /// The bus could not be watched, so it was left suspended.
    NotObservable,
}

impl ResumedAfter {
    /// Whether the watchdog was actually armed.
    #[must_use]
    pub const fn armed(self) -> bool {
        matches!(self, Self::ReaderPinged)
    }
}

impl fmt::Display for ResumedAfter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReaderPinged => write!(formatter, "the reader was feeding it again"),
            Self::NoPingSeen => write!(
                formatter,
                "no ping was seen, so it was LEFT SUSPENDED rather than armed against a reader that is not feeding it; a reboot restores freeze protection"
            ),
            Self::NotObservable => write!(
                formatter,
                "the bus could not be watched, so it was left suspended; a reboot restores freeze protection"
            ),
        }
    }
}

/// Blocks until the reader is feeding the watchdog in a rhythm, or `wait`
/// elapses.
///
/// One ping is not a heartbeat. A reader that has just been started pings
/// while it is still coming up, long before it is sustainably feeding
/// anything, and arming a ten second fuse on the strength of that single ping
/// is what rebooted the device at the end of a session: the reader was still
/// scanning its library, the next ping was more than ten seconds away, and
/// `sickel` killed it and rebooted. So this waits for two pings from the same
/// sender close enough together to mean "being fed" rather than "happened
/// once".
///
/// Pings are attributed by sender because we are pinging too, and counting our
/// own keep-alive as evidence the reader is healthy would be circular. The
/// discriminator is free: each of our pings is a fresh `dbus-send` process
/// with a fresh connection, so it gets a new unique name every time and can
/// never produce two pings under one name. The reader holds one connection for
/// its lifetime, so all of its pings share a name.
fn wait_for_heartbeat(session_bus: &str, wait: Duration) -> ResumedAfter {
    let child = Command::new(DBUS_MONITOR)
        .arg("--session")
        .arg(format!(
            "type='method_call',interface='{SERVICE}',member='Ping'"
        ))
        .env("DBUS_SESSION_BUS_ADDRESS", session_bus)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else {
        thread::sleep(wait);
        return ResumedAfter::NotObservable;
    };
    let Some(output) = child.stdout.take() else {
        let _ignored = child.kill();
        let _ignored = child.wait();
        thread::sleep(wait);
        return ResumedAfter::NotObservable;
    };
    let (sender, seen) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(output).lines() {
            let Ok(line) = line else { return };
            // The monitor announces itself on the bus first, so only a real
            // method call counts.
            if !line.contains("member=Ping") {
                continue;
            }
            let Some(name) = sender_of(&line) else {
                continue;
            };
            if sender.send(name).is_err() {
                return;
            }
        }
    });

    let deadline = Instant::now() + wait;
    let mut last_seen: Vec<(String, Instant)> = Vec::new();
    let result = loop {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            break ResumedAfter::NoPingSeen;
        };
        let Ok(name) = seen.recv_timeout(remaining) else {
            break ResumedAfter::NoPingSeen;
        };
        let now = Instant::now();
        if let Some((_, previous)) = last_seen.iter_mut().find(|(known, _)| *known == name) {
            if now.duration_since(*previous) <= HEARTBEAT_WINDOW {
                break ResumedAfter::ReaderPinged;
            }
            // Too far apart to be a rhythm, but it is still the most recent
            // ping from this sender, so the next one is measured against it.
            *previous = now;
        } else {
            last_seen.push((name, now));
        }
    };
    let _ignored = child.kill();
    let _ignored = child.wait();
    result
}

/// The unique name a `dbus-monitor` line came from, as in `sender=:1.7`.
fn sender_of(line: &str) -> Option<String> {
    let rest = line.split_once("sender=")?.1;
    let name = rest
        .split(|character: char| character.is_ascii_whitespace())
        .next()?;
    (!name.is_empty()).then(|| name.to_owned())
}

/// Resumes a watchdog using a session bus address recovered from elsewhere.
///
/// This exists for the recovery path, where the process that suspended the
/// watchdog is gone and only its saved description remains.
///
/// # Errors
///
/// Returns an error when the call is refused.
pub fn resume_with(session_bus: Option<&OsStr>) -> Result<(), SupervisorError> {
    let session_bus = session_bus
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .ok_or(SupervisorError::NoSessionBus)?;
    call(session_bus, RESUME).map_err(|detail| SupervisorError::CallFailed {
        method: "Resume",
        detail,
    })
}

fn call(session_bus: &str, method: &str) -> Result<(), String> {
    let output = Command::new(DBUS_SEND)
        .arg("--session")
        // Without this `dbus-send` marks the message as expecting no reply and
        // exits zero the moment it has been written to the socket. Every way
        // the call can actually be refused (wrong service, wrong path, wrong
        // interface, a policy denial) then looks exactly like success, which
        // is how a watchdog everybody believed was suspended went on rebooting
        // the device. A method that returns nothing still returns a reply.
        .arg("--print-reply")
        // Waiting for the reply is only safe if the wait is bounded. The
        // default is twenty-five seconds, which is longer than the watchdog
        // takes to fire, so an unresponsive bus would turn a safety check into
        // the very hang it is meant to prevent.
        .arg(format!("--reply-timeout={REPLY_TIMEOUT_MS}"))
        .arg(format!("--dest={SERVICE}"))
        .arg(OBJECT)
        .arg(method)
        .env("DBUS_SESSION_BUS_ADDRESS", session_bus)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr)
            .trim()
            .replace(['\r', '\n'], " "))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        resume_with, sender_of, SupervisorError, Suspended, FUSE, HEARTBEAT_WINDOW,
        KEEP_ALIVE_INTERVAL, OBJECT, SERVICE,
    };
    use std::ffi::OsStr;

    /// Taken verbatim off the device, watching a reader restart.
    const RESTARTED_READER_PING: &str = "method call sender=:1.7 -> dest=com.kobo.watchdog.Sickel serial=13 path=/; interface=com.kobo.watchdog.Sickel; member=Ping";

    #[test]
    fn a_ping_is_attributed_to_the_connection_that_sent_it() {
        assert_eq!(sender_of(RESTARTED_READER_PING).as_deref(), Some(":1.7"));
    }

    /// Our own keep-alive pings must never be mistaken for the reader's.
    ///
    /// They are told apart by sender, so the parser has to find the sender on
    /// every line shape the monitor produces rather than on one of them.
    #[test]
    fn a_line_with_no_sender_names_nobody() {
        assert_eq!(sender_of("method call dest=com.kobo.watchdog.Sickel"), None);
        assert_eq!(sender_of("signal time=1 sender= -> dest=x"), None);
    }

    /// The margin the reader's own behaviour demands.
    ///
    /// A restarted reader calls `Resume` on the watchdog while it is still
    /// starting, so between its `Resume` and our next `Suspend` there is a
    /// live fuse. The interval is how much of that fuse is spent before we
    /// stop the timer again, and it has to leave room for a call or two to be
    /// refused on a device that idles at a load average around five.
    #[test]
    fn re_suspending_spends_only_a_fraction_of_the_fuse() {
        assert!(
            KEEP_ALIVE_INTERVAL.saturating_mul(3) < FUSE,
            "two refused calls in a row must still leave the fuse unspent"
        );
    }

    /// One ping is not a heartbeat.
    ///
    /// The window has to be wide enough to accept a reader pinging at its
    /// usual five seconds while it is busy, and no wider than the fuse, since
    /// pings further apart than the fuse are by definition not keeping the
    /// watchdog fed.
    #[test]
    fn a_heartbeat_is_two_pings_within_the_fuse() {
        assert!(HEARTBEAT_WINDOW >= Duration::from_secs(5));
        assert!(HEARTBEAT_WINDOW <= FUSE);
    }

    use std::time::Duration;

    /// The rule that a device was factory reset for want of.
    ///
    /// Arming the freeze watchdog is only ever safe when something has been
    /// seen feeding it. Every other outcome must leave it suspended: an armed
    /// watchdog nobody feeds hard-resets the `SoC` about every ten seconds with
    /// nothing synced, and eventually one of those resets lands in the middle
    /// of the reader writing its library database.
    #[test]
    fn the_watchdog_is_armed_only_when_the_reader_was_seen_feeding_it() {
        use super::ResumedAfter;
        assert!(ResumedAfter::ReaderPinged.armed());
        assert!(
            !ResumedAfter::NoPingSeen.armed(),
            "arming against a reader that is not pinging resets the device on a loop"
        );
        assert!(
            !ResumedAfter::NotObservable.armed(),
            "not being able to watch the bus is not evidence that anything is feeding it"
        );
    }

    /// Whatever the outcome is called, the log has to say what was actually
    /// done, because the previous wording said the device "may reboot back to
    /// stock shortly" -- which read as a caveat and was in fact a promise.
    #[test]
    fn an_unarmed_outcome_says_it_was_left_suspended() {
        use super::ResumedAfter;
        assert!(ResumedAfter::NoPingSeen.to_string().contains("SUSPENDED"));
        assert!(ResumedAfter::NotObservable
            .to_string()
            .contains("left suspended"));
    }

    #[test]
    fn a_missing_session_bus_is_refused_rather_than_guessed() {
        // The bus address contains a per-boot socket path, so there is no
        // sensible default. Guessing would silently fail to suspend the
        // watchdog and the device would reboot mid-session.
        assert!(matches!(
            Suspended::suspend(None),
            Err(SupervisorError::NoSessionBus)
        ));
    }

    #[test]
    fn an_empty_session_bus_is_refused() {
        assert!(matches!(
            Suspended::suspend(Some(OsStr::new(""))),
            Err(SupervisorError::NoSessionBus)
        ));
    }

    #[test]
    fn recovery_also_refuses_a_missing_session_bus() {
        assert!(matches!(
            resume_with(None),
            Err(SupervisorError::NoSessionBus)
        ));
    }

    #[test]
    fn the_service_details_match_what_the_device_exports() {
        // Confirmed by introspecting the running service: the object path is
        // the root, not a name-derived path.
        assert_eq!(SERVICE, "com.kobo.watchdog.Sickel");
        assert_eq!(OBJECT, "/");
    }

    #[test]
    fn a_failed_call_names_the_method() {
        let error = SupervisorError::CallFailed {
            method: "Suspend",
            detail: "no such service".to_owned(),
        };
        assert!(error.to_string().contains("Suspend"));
    }
}

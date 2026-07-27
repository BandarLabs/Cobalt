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
use std::time::Duration;

/// The well-known name the watchdog registers.
const SERVICE: &str = "com.kobo.watchdog.Sickel";
/// The object path it exports. Confirmed by introspection on the device.
const OBJECT: &str = "/";
const SUSPEND: &str = "com.kobo.watchdog.Sickel.Suspend";
const RESUME: &str = "com.kobo.watchdog.Sickel.Resume";
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
/// reboot`. So the timer was running again despite the suspension. Something
/// restarts it — the likeliest candidate is the reader itself calling `Resume`
/// on its way out, since `Suspend` is not reference counted and the reader has
/// no idea somebody else suspended it — and no amount of reading the
/// disassembly makes a one-shot call safe against that.
///
/// The reader solves the same problem by pinging every five seconds rather than
/// by reasoning about it, so this does the same. Re-asserting the suspension is
/// idempotent: `QTimer::stop()` on a stopped timer does nothing.
const KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(5);

/// How long a single watchdog call may take before it is treated as failed.
///
/// Comfortably longer than the round trip on the device, comfortably shorter
/// than the interval between keep-alive calls, so a stuck call can never queue
/// up behind the next one.
const REPLY_TIMEOUT_MS: u32 = 3_000;

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
                let _ignored = call(&bus, SUSPEND);
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
    /// the reader's own ping and resumes as soon as one arrives.
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
    /// # Errors
    ///
    /// Returns an error when the resume call itself is refused.
    pub fn resume_once_fed(&self, wait: Duration) -> Result<ResumedAfter, SupervisorError> {
        let observed = wait_for_ping(&self.session_bus, wait);
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

/// Blocks until the reader pings the watchdog, or `wait` elapses.
fn wait_for_ping(session_bus: &str, wait: Duration) -> ResumedAfter {
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
            if line.contains("member=Ping") && sender.send(()).is_err() {
                return;
            }
        }
    });
    let result = if seen.recv_timeout(wait).is_ok() {
        ResumedAfter::ReaderPinged
    } else {
        ResumedAfter::NoPingSeen
    };
    let _ignored = child.kill();
    let _ignored = child.wait();
    result
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
        // the call can actually be refused — wrong service, wrong path, wrong
        // interface, a policy denial — then looks exactly like success, which
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
    use super::{resume_with, SupervisorError, Suspended, OBJECT, SERVICE};
    use std::ffi::OsStr;

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

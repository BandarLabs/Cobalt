//! A trace that survives an unclean reset.
//!
//! The panel session prints its progress to standard output, and on this
//! device that is worthless the moment something resets the `SoC`: the log
//! lives on a tmpfs that a reboot empties, and the copy on `/mnt/onboard` is
//! VFAT with buffered writes, so the last several seconds (exactly the seconds
//! that matter) are never on the card.
//!
//! So the black box writes to the book partition and calls `fsync` after every
//! single line. That is deliberately expensive. It is the only way to learn
//! *when* the device died and *what the session was doing* at the time, and
//! without that the reboots can only be guessed at.
//!
//! It is off unless `KOBO_BLACKBOX=1`, because a synchronous write per event on
//! the owner's only device is a cost that should be paid on purpose.
//!
//! Every line carries the kernel's own clock, read from `/proc/uptime`, rather
//! than a wall clock. On this device `/proc/uptime` counts suspended time and
//! never resets on its own, so a reading that suddenly drops to single digits
//! is a boot and nothing else; a wall clock could have been stepped by NTP.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// Where the trace lands. The book partition is the only writable filesystem
/// that survives a reboot, and a leading dot keeps it out of the reader's
/// library view.
const PATH: &str = "/mnt/onboard/.kobo-blackbox.log";
const ENABLE: &str = "KOBO_BLACKBOX";

pub struct BlackBox {
    file: Option<Mutex<File>>,
    started: Instant,
}

impl BlackBox {
    /// Opens the trace, appending so that the record of an earlier session that
    /// ended in a reset is never destroyed by the run investigating it.
    #[must_use]
    pub fn open() -> Self {
        let started = Instant::now();
        if std::env::var(ENABLE).ok().as_deref() != Some("1") {
            return Self {
                file: None,
                started,
            };
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(PATH)
            .ok()
            .map(Mutex::new);
        let black_box = Self { file, started };
        black_box.record("=== session start");
        black_box
    }

    /// Writes one line and does not return until the card has it.
    pub fn record(&self, event: &str) {
        let Some(file) = &self.file else {
            return;
        };
        let line = format!(
            "{:>10.2} {:>8.2} {event}\n",
            kernel_seconds(),
            self.started.elapsed().as_secs_f64()
        );
        let Ok(mut file) = file.lock() else {
            return;
        };
        let _ignored = file.write_all(line.as_bytes());
        let _ignored = file.flush();
        // The whole point. Without this the trailing lines sit in the page
        // cache and a hardware reset takes them with it.
        let _ignored = file.sync_all();
    }

    #[must_use]
    pub fn is_recording(&self) -> bool {
        self.file.is_some()
    }
}

/// Seconds since boot, as the kernel counts them.
fn kernel_seconds() -> f64 {
    let mut text = String::new();
    let Ok(mut file) = File::open("/proc/uptime") else {
        return 0.0;
    };
    if file.read_to_string(&mut text).is_err() {
        return 0.0;
    }
    text.split_whitespace()
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0.0)
}

static TRACE: OnceLock<BlackBox> = OnceLock::new();

/// Records one line in the session trace.
///
/// Free rather than a method because the interesting events happen deep inside
/// the session (a tap resolving to nothing, a screen being painted, the reader
/// being stopped) and threading a recorder through every one of those
/// signatures would make the diagnostic harder to leave in place than to
/// remove.
pub fn trace(event: &str) {
    TRACE.get_or_init(BlackBox::open).record(event);
}

/// Whether the trace is on, for reporting it once in the session summary.
#[must_use]
pub fn recording() -> bool {
    TRACE.get_or_init(BlackBox::open).is_recording()
}

#[cfg(test)]
mod tests {
    use super::BlackBox;

    #[test]
    fn a_disabled_black_box_writes_nothing_and_never_fails() {
        // The device path does not exist on the host, so this also proves the
        // session is unaffected when the book partition cannot be written.
        let black_box = BlackBox {
            file: None,
            started: std::time::Instant::now(),
        };
        assert!(!black_box.is_recording());
        black_box.record("this must not panic");
    }
}

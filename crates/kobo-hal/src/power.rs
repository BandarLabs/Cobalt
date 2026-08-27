//! Putting the device to sleep while Cobalt still owns the session.
//!
//! The stock reader is stopped for the whole panel session, so the power
//! button has no other owner. The kernel still publishes the same two nodes
//! used for suspend-to-RAM: `/sys/power/state` lists the states it will
//! enter, and on `NTX` kernels `/sys/power/state-extended` flags subsystems so
//! a later `mem` write is real suspend rather than standby. Both are probed.
//! A missing node is skipped, never assumed.
//!
//! The hold-to-power-off classification lives here so it can be tested without
//! a panel session. The runtime decides when to call it.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long the power button must stay down before the press is a shutdown.
///
/// Matches the stock reader: a tap sleeps, a hold of a couple of seconds
/// powers off. Measured against Nickel's own timing on a Clara BW.
pub const HOLD_TO_POWER_OFF: Duration = Duration::from_secs(2);

/// What a power-button edge means, once the runtime says whether the session
/// is already asleep.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ButtonAction {
    Sleep,
    Wake,
    Shutdown,
}

/// Classifies press, hold and release of the power button.
#[derive(Clone, Debug, Default)]
pub struct PowerButton {
    pressed_at: Option<Instant>,
    shutdown_armed: bool,
}

impl PowerButton {
    /// The button went down.
    pub fn press(&mut self, now: Instant) {
        self.pressed_at = Some(now);
        self.shutdown_armed = false;
    }

    /// The button is still down. Returns [`ButtonAction::Shutdown`] once, when
    /// the hold crosses [`HOLD_TO_POWER_OFF`].
    pub fn poll_hold(&mut self, now: Instant) -> Option<ButtonAction> {
        let start = self.pressed_at?;
        if self.shutdown_armed || now.saturating_duration_since(start) < HOLD_TO_POWER_OFF {
            return None;
        }
        self.shutdown_armed = true;
        Some(ButtonAction::Shutdown)
    }

    /// The button came up. A hold that already armed shutdown is consumed
    /// silently so a release after power-off has started does not also sleep.
    pub fn release(&mut self, asleep: bool) -> Option<ButtonAction> {
        self.pressed_at = None;
        if self.shutdown_armed {
            self.shutdown_armed = false;
            return None;
        }
        Some(if asleep {
            ButtonAction::Wake
        } else {
            ButtonAction::Sleep
        })
    }

    #[must_use]
    pub const fn is_down(&self) -> bool {
        self.pressed_at.is_some()
    }
}

/// The kernel's suspend nodes, once they have been found.
#[derive(Clone, Debug)]
pub struct Power {
    state: PathBuf,
    extended: Option<PathBuf>,
}

impl Power {
    /// The live nodes, or nothing on a machine that does not publish them.
    #[must_use]
    pub fn open() -> Option<Self> {
        Self::open_in(Path::new("/sys/power"))
    }

    /// The same, against an arbitrary directory, so the writes are testable
    /// without touching a real power state.
    #[must_use]
    pub fn open_in(dir: &Path) -> Option<Self> {
        let state = dir.join("state");
        if !state.is_file() {
            return None;
        }
        let extended = {
            let path = dir.join("state-extended");
            path.is_file().then_some(path)
        };
        Some(Self { state, extended })
    }

    /// Whether the kernel lists `mem` among the states it will enter.
    #[must_use]
    pub fn allows_mem(&self) -> bool {
        fs::read_to_string(&self.state)
            .is_ok_and(|listed| listed.split_ascii_whitespace().any(|state| state == "mem"))
    }

    /// Flags subsystems for suspend. A device without `state-extended` is a
    /// successful no-op: there is nothing to flag.
    ///
    /// # Errors
    ///
    /// When the node exists and will not take the write. The caller must not
    /// then write `mem`, because that would be standby rather than suspend.
    pub fn flag_subsystems(&self) -> io::Result<()> {
        if let Some(path) = &self.extended {
            fs::write(path, b"1\n")?;
        }
        Ok(())
    }

    /// Clears the subsystem flag after a wake, or after a failed `mem` write.
    ///
    /// # Errors
    ///
    /// As [`Self::flag_subsystems`].
    pub fn unflag_subsystems(&self) -> io::Result<()> {
        if let Some(path) = &self.extended {
            fs::write(path, b"0\n")?;
        }
        Ok(())
    }

    /// Asks for suspend-to-RAM. Returns only when the kernel wakes the process.
    ///
    /// # Errors
    ///
    /// When the node refuses the write (`EBUSY` is the usual case: the `EPDC`
    /// or the touch panel still has work). The caller must unflag subsystems.
    pub fn enter_mem(&self) -> io::Result<()> {
        fs::write(&self.state, b"mem\n")
    }
}

#[cfg(test)]
mod tests {
    use super::{ButtonAction, Power, PowerButton, HOLD_TO_POWER_OFF};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    fn root(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("kobo-power-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("temp power dir");
        path
    }

    fn write(dir: &Path, name: &str, body: &str) {
        fs::write(dir.join(name), body).expect("write power node");
    }

    #[test]
    fn a_short_press_while_awake_sleeps() {
        let mut button = PowerButton::default();
        button.press(Instant::now());
        assert_eq!(button.release(false), Some(ButtonAction::Sleep));
    }

    #[test]
    fn a_short_press_while_asleep_wakes() {
        let mut button = PowerButton::default();
        button.press(Instant::now());
        assert_eq!(button.release(true), Some(ButtonAction::Wake));
    }

    #[test]
    fn a_hold_shuts_down_once_and_the_release_does_not_also_sleep() {
        let mut button = PowerButton::default();
        let start = Instant::now();
        button.press(start);
        assert_eq!(button.poll_hold(start), None);
        assert_eq!(
            button.poll_hold(start + HOLD_TO_POWER_OFF),
            Some(ButtonAction::Shutdown)
        );
        assert_eq!(
            button.poll_hold(start + HOLD_TO_POWER_OFF + Duration::from_secs(1)),
            None,
            "shutdown is armed once"
        );
        assert_eq!(button.release(false), None);
    }

    #[test]
    fn mem_is_detected_from_the_listed_states() {
        let dir = root("mem");
        write(&dir, "state", "freeze mem standby\n");
        let power = Power::open_in(&dir).expect("nodes");
        assert!(power.allows_mem());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_kernel_without_mem_is_not_told_it_has_it() {
        let dir = root("standby");
        write(&dir, "state", "standby\n");
        let power = Power::open_in(&dir).expect("nodes");
        assert!(!power.allows_mem());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn flag_then_failed_mem_clears_the_extended_node() {
        let dir = root("flag");
        write(&dir, "state", "mem\n");
        write(&dir, "state-extended", "0\n");
        let power = Power::open_in(&dir).expect("nodes");
        power.flag_subsystems().expect("flag");
        assert_eq!(
            fs::read_to_string(dir.join("state-extended"))
                .unwrap()
                .trim(),
            "1"
        );
        // A directory is not a file the kernel will suspend with, so this
        // write fails the way a busy `EPDC` fails: the node is there and will
        // not take `mem`.
        fs::remove_file(dir.join("state")).expect("remove state");
        fs::create_dir(dir.join("state")).expect("state as directory");
        assert!(power.enter_mem().is_err());
        power.unflag_subsystems().expect("unflag");
        assert_eq!(
            fs::read_to_string(dir.join("state-extended"))
                .unwrap()
                .trim(),
            "0"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_directory_without_state_is_not_a_power_backend() {
        let dir = root("empty");
        assert!(Power::open_in(&dir).is_none());
        let _ = fs::remove_dir_all(&dir);
    }
}

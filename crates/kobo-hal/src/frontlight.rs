//! Front light brightness.
//!
//! Behind `device-write`, because unlike the battery this changes something the
//! owner can see. It is still among the mildest things the platform does: one
//! small integer into one sysfs file, held in a register on the light driver
//! and nowhere else. A reboot restores whatever the stock reader last set, and
//! so does [`Frontlight::restore`], which is what the runtime calls when a
//! session ends.
//!
//! # Why the control is discovered rather than named
//!
//! The Clara BW publishes four backlight devices. Only one of them is the
//! control the owner thinks of as brightness:
//!
//! - `lm3630a_led` is the aggregate, `max_brightness` 100.
//! - `lm3630a_leda` and `lm3630a_ledb` are the two channels behind it, the cool
//!   and warm halves of ComfortLight, each `max_brightness` 255. Writing these
//!   individually sets a colour temperature as a side effect, which is not what
//!   an application asking for "brighter" means.
//! - `mxc_msp430.0` is a companion-chip control this device does not drive; it
//!   reads 0 and stays 0.
//!
//! Picking by name would be picking for one device. Instead the aggregate is
//! preferred where it exists and anything else is a fallback, and every path
//! is checked for the files it needs before it is used, a Kobo that names its
//! light something else still works, and one that publishes no light at all
//! reports [`None`] so the caller can refuse the capability rather than
//! pretending a write succeeded.
//!
//! # Why percentages
//!
//! Applications ask in percent and this scales to whatever the hardware counts
//! in. A device whose maximum is 255 and one whose maximum is 100 both take
//! `50` to mean half, so an application does not have to know which it is
//! talking to.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Where Linux publishes backlight controls.
const BACKLIGHTS: &str = "/sys/class/backlight";

/// The aggregate control, preferred when the device publishes one.
///
/// Named as a preference and never as a requirement: if it is absent, any other
/// usable control is taken instead.
const PREFERRED: &str = "lm3630a_led";

/// A front light that can be read and set.
///
/// Holds the reading taken when it was opened so that whatever the session does
/// can be undone exactly, rather than by guessing at a sensible default. A
/// runtime that restored "20 percent" would be leaving the owner's reader
/// changed by having run us, which is the one thing this project does not do.
#[derive(Clone, Debug)]
pub struct Frontlight {
    control: PathBuf,
    maximum: u32,
    original: u8,
}

impl Frontlight {
    /// Opens the device's front light, or returns `None` when it has none.
    #[must_use]
    pub fn open() -> Option<Self> {
        Self::open_in(Path::new(BACKLIGHTS))
    }

    /// The same, against an arbitrary root, so the discovery and the scaling are
    /// testable on a machine with no front light.
    #[must_use]
    pub fn open_in(backlights: &Path) -> Option<Self> {
        let control = find_control(backlights)?;
        let maximum = read_number(&control.join("max_brightness"))?;
        if maximum == 0 {
            return None;
        }
        let raw = read_number(&control.join("brightness"))?;
        let original = to_percent(raw, maximum);
        Some(Self {
            control,
            maximum,
            original,
        })
    }

    /// What the light is set to now, 0 to 100.
    ///
    /// Read back from the hardware rather than remembered, because the stock
    /// reader is still running and may have changed it underneath us.
    #[must_use]
    pub fn percent(&self) -> Option<u8> {
        let raw = read_number(&self.control.join("brightness"))?;
        Some(to_percent(raw, self.maximum))
    }

    /// What the light was set to before this process touched it.
    #[must_use]
    pub const fn original(&self) -> u8 {
        self.original
    }

    /// Sets the light, clamping to the range the hardware accepts.
    ///
    /// Returns what was actually set, which is not always what was asked for:
    /// a control counting to 100 cannot distinguish requests that scale to the
    /// same integer, and an application that redraws a slider from the returned
    /// value stays honest about it.
    ///
    /// # Errors
    ///
    /// When the control cannot be written, which on this device means the file
    /// vanished or the process is not root.
    pub fn set(&self, percent: u8) -> io::Result<u8> {
        let percent = percent.min(100);
        let raw = to_raw(percent, self.maximum);
        fs::write(self.control.join("brightness"), format!("{raw}\n"))?;
        Ok(to_percent(raw, self.maximum))
    }

    /// Puts the light back to where it was found.
    ///
    /// # Errors
    ///
    /// As [`Self::set`].
    pub fn restore(&self) -> io::Result<u8> {
        self.set(self.original)
    }
}

/// The best usable control under a backlight directory.
///
/// "Usable" means it publishes both of the files this module needs; a directory
/// that publishes only one is skipped rather than opened and failed on later.
/// Candidates are sorted so that a device with several equally usable controls
/// yields the same one on every call.
fn find_control(backlights: &Path) -> Option<PathBuf> {
    let mut candidates = fs::read_dir(backlights)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.join("brightness").is_file() && path.join("max_brightness").is_file())
        .collect::<Vec<_>>();
    candidates.sort();
    let preferred = candidates
        .iter()
        .position(|path| path.file_name().is_some_and(|name| name == PREFERRED));
    match preferred {
        Some(at) => Some(candidates.swap_remove(at)),
        None => candidates.into_iter().next(),
    }
}

/// A non-negative integer from a sysfs file, or `None` when it is missing or is
/// not one.
fn read_number(path: &Path) -> Option<u32> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// Scales a hardware reading to a percentage, rounding to nearest.
///
/// Rounding rather than truncating so that the maximum reads as 100 and not as
/// 99, and so that setting a value and reading it back agrees with itself.
fn to_percent(raw: u32, maximum: u32) -> u8 {
    if maximum == 0 {
        return 0;
    }
    let scaled = (u64::from(raw.min(maximum)) * 100 + u64::from(maximum) / 2) / u64::from(maximum);
    u8::try_from(scaled).unwrap_or(100)
}

/// Scales a percentage to a hardware value, rounding to nearest.
fn to_raw(percent: u8, maximum: u32) -> u32 {
    let percent = u64::from(percent.min(100));
    let scaled = (percent * u64::from(maximum) + 50) / 100;
    u32::try_from(scaled).unwrap_or(maximum).min(maximum)
}

#[cfg(test)]
mod tests {
    use super::{find_control, to_percent, to_raw, Frontlight};
    use std::fs;
    use std::path::Path;

    /// Builds a backlight directory the way the kernel publishes one.
    fn control(root: &Path, name: &str, brightness: &str, maximum: &str) {
        let dir = root.join(name);
        fs::create_dir_all(&dir).expect("create");
        fs::write(dir.join("brightness"), brightness).expect("write");
        fs::write(dir.join("max_brightness"), maximum).expect("write");
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("kobo-frontlight-{name}"));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create");
        path
    }

    #[test]
    fn the_aggregate_control_is_preferred_over_the_channels_behind_it() {
        // The Clara BW publishes all four of these. Writing a channel sets a
        // colour temperature as a side effect, which is not what an application
        // asking for "brighter" means.
        let root = scratch("aggregate");
        control(&root, "lm3630a_leda", "99", "255");
        control(&root, "lm3630a_ledb", "70", "255");
        control(&root, "lm3630a_led", "7", "100");
        control(&root, "mxc_msp430.0", "0", "100");
        let found = find_control(&root).expect("a control");
        assert_eq!(found.file_name().expect("name"), "lm3630a_led");
    }

    #[test]
    fn a_device_that_names_its_light_something_else_still_works() {
        // Never map unknown hardware onto a known name: a Kobo that calls its
        // light anything at all is better served by a control that works than
        // by a refusal.
        let root = scratch("unnamed");
        control(&root, "some_other_led", "50", "100");
        let found = find_control(&root).expect("a control");
        assert_eq!(found.file_name().expect("name"), "some_other_led");
    }

    #[test]
    fn a_control_missing_the_files_it_needs_is_not_offered() {
        let root = scratch("partial");
        fs::create_dir_all(root.join("broken")).expect("create");
        fs::write(root.join("broken").join("brightness"), "5").expect("write");
        assert!(find_control(&root).is_none());
    }

    #[test]
    fn a_device_with_no_light_at_all_reports_one_rather_than_inventing_it() {
        // The caller refuses the capability. An application told that it set
        // the brightness when nothing happened is worse off than one told no.
        let root = scratch("none");
        assert!(Frontlight::open_in(&root).is_none());
    }

    #[test]
    fn halfway_means_halfway_whatever_the_hardware_counts_in() {
        // The whole reason applications speak in percent: 50 is half on a
        // control that stops at 100 and on one that stops at 255.
        assert_eq!(to_raw(50, 100), 50);
        assert_eq!(to_raw(50, 255), 128);
        assert_eq!(to_percent(128, 255), 50);
        assert_eq!(to_percent(50, 100), 50);
    }

    #[test]
    fn the_top_of_the_range_reads_as_full_rather_than_as_ninety_nine() {
        // Truncating division gets this wrong, and a slider that cannot reach
        // its own end looks broken.
        assert_eq!(to_percent(255, 255), 100);
        assert_eq!(to_raw(100, 255), 255);
        assert_eq!(to_percent(0, 255), 0);
    }

    #[test]
    fn a_request_past_the_end_is_clamped_rather_than_wrapped() {
        assert_eq!(to_raw(u8::MAX, 100), 100);
        assert_eq!(to_percent(4000, 255), 100);
    }

    #[test]
    fn what_the_light_was_found_at_is_kept_so_it_can_be_put_back_exactly() {
        // Restoring a "sensible default" would leave the owner's reader changed
        // by having run us.
        let root = scratch("restore");
        control(&root, "lm3630a_led", "7", "100");
        let light = Frontlight::open_in(&root).expect("a light");
        assert_eq!(light.original(), 7);
        light.set(80).expect("set");
        assert_eq!(light.percent(), Some(80));
        light.restore().expect("restore");
        assert_eq!(light.percent(), Some(7));
    }
}

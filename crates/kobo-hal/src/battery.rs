//! Read-only battery observation.
//!
//! This is not behind `device-write`. Nothing here opens a device node, writes
//! a file, or changes any power state: it reads two small text files that the
//! kernel publishes for exactly this purpose, so it is safe while the stock
//! reader owns everything else.
//!
//! # Why the supply is discovered rather than named
//!
//! The Clara BW's gauge is `bd71827_bat`, but the charger on another Kobo is a
//! different part with a different name, and a hard-coded path would either
//! read the wrong supply or silently report nothing. The rule the whole project
//! uses applies here too: never map unknown hardware onto a known name. So the
//! supplies are enumerated and the one whose `type` is `Battery` is used, which
//! is a property of what the thing *is* rather than what it is called.
//!
//! A device with no battery supply at all returns [`None`], and the caller
//! refuses the capability rather than inventing a percentage. A made-up battery
//! reading is worse than no reading, because an application will act on it.

use std::fs;
use std::path::{Path, PathBuf};

/// Where Linux publishes power supplies.
const SUPPLIES: &str = "/sys/class/power_supply";

/// What the gauge currently says.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Battery {
    /// Charge remaining, 0 to 100.
    pub percent: u8,
    /// True while the device is taking power from something other than the
    /// battery. `Full` counts, because a device on a charger at 100 percent is
    /// not running the battery down.
    pub charging: bool,
}

/// Reads the battery, or returns `None` when this device does not publish one.
#[must_use]
pub fn read() -> Option<Battery> {
    read_from(Path::new(SUPPLIES))
}

/// The same, against an arbitrary root, so the parsing is testable without a
/// battery.
#[must_use]
pub fn read_from(supplies: &Path) -> Option<Battery> {
    let supply = find_battery(supplies)?;
    let percent = read_percent(&supply.join("capacity"))?;
    let charging = read_charging(&supply.join("status"));
    Some(Battery { percent, charging })
}

/// The first supply whose `type` file says `Battery`.
///
/// Entries are sorted, so a device with more than one battery reports the same
/// one on every call rather than whichever the directory happened to yield
/// first.
fn find_battery(supplies: &Path) -> Option<PathBuf> {
    let mut candidates = fs::read_dir(supplies)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            fs::read_to_string(path.join("type"))
                .is_ok_and(|kind| kind.trim().eq_ignore_ascii_case("Battery"))
        })
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.into_iter().next()
}

/// A percentage, or `None` when the file is missing or not a number.
///
/// Clamped rather than rejected above 100: some gauges report 101 briefly while
/// calibrating, and that is a full battery rather than a broken kernel.
fn read_percent(path: &Path) -> Option<u8> {
    let text = fs::read_to_string(path).ok()?;
    let value = text.trim().parse::<i64>().ok()?;
    Some(u8::try_from(value.clamp(0, 100)).unwrap_or(0))
}

/// Whether the device is on external power.
///
/// A missing or unreadable status is reported as not charging, which is the
/// conservative answer: policy withholds expensive capabilities on a low
/// battery unless it is charging, so guessing "charging" would hand out the
/// very grants the low-battery rule exists to withhold.
fn read_charging(path: &Path) -> bool {
    fs::read_to_string(path).is_ok_and(|status| {
        let status = status.trim();
        status.eq_ignore_ascii_case("Charging") || status.eq_ignore_ascii_case("Full")
    })
}

#[cfg(test)]
mod tests {
    use super::{read_from, Battery};
    use std::fs;
    use std::path::{Path, PathBuf};

    fn root(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("kobo-battery-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("a test directory");
        path
    }

    fn supply(root: &Path, name: &str, kind: &str, capacity: &str, status: &str) {
        let path = root.join(name);
        fs::create_dir_all(&path).expect("a supply directory");
        fs::write(path.join("type"), kind).expect("a type");
        if !capacity.is_empty() {
            fs::write(path.join("capacity"), capacity).expect("a capacity");
        }
        if !status.is_empty() {
            fs::write(path.join("status"), status).expect("a status");
        }
    }

    #[test]
    fn the_battery_is_found_by_type_rather_than_by_name() {
        // Exactly the shape of the real Clara BW: a mains supply that has no
        // capacity at all, beside the gauge.
        let root = root("by-type");
        supply(&root, "bd71827_ac", "Mains", "", "");
        supply(&root, "bd71827_bat", "Battery", "33", "Discharging");
        assert_eq!(
            read_from(&root),
            Some(Battery {
                percent: 33,
                charging: false,
            })
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_device_with_no_battery_reports_nothing_rather_than_a_default() {
        let root = root("none");
        supply(&root, "some_ac", "Mains", "", "");
        assert_eq!(read_from(&root), None);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_full_battery_on_a_charger_counts_as_charging() {
        let root = root("full");
        supply(&root, "bat", "Battery", "100", "Full");
        assert_eq!(
            read_from(&root),
            Some(Battery {
                percent: 100,
                charging: true,
            })
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn an_over_range_reading_is_clamped_rather_than_discarded() {
        let root = root("clamped");
        supply(&root, "bat", "Battery", "101", "Charging");
        assert_eq!(
            read_from(&root),
            Some(Battery {
                percent: 100,
                charging: true,
            })
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn an_unreadable_capacity_is_not_reported_as_empty() {
        // The dangerous failure: reporting zero would make an application
        // believe the device is about to die.
        let root = root("garbage");
        supply(&root, "bat", "Battery", "not a number", "Discharging");
        assert_eq!(read_from(&root), None);
        let _ = fs::remove_dir_all(&root);
    }
}

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
    let charging = cable_online(supplies).unwrap_or_else(|| read_charging(&supply.join("status")));
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

/// Everything the gauge publishes, defined once on the wire.
///
/// The screen that shows these numbers and the driver that reads them agree on
/// one type rather than two that have to be kept in step by hand.
pub use kobo_protocol::BatteryDetail as Detail;

/// Reads everything the gauge publishes, or `None` with no battery at all.
#[must_use]
pub fn detail() -> Option<Detail> {
    detail_from(Path::new(SUPPLIES))
}

/// The same, against an arbitrary root, so it is testable without a battery.
#[must_use]
pub fn detail_from(supplies: &Path) -> Option<Detail> {
    let supply = find_battery(supplies)?;
    let text = |name: &str| {
        fs::read_to_string(supply.join(name))
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty() && value != "N/A")
    };
    let number = |name: &str| text(name).and_then(|value| value.parse::<i32>().ok());
    Some(Detail {
        percent: read_percent(&supply.join("capacity")),
        status: text("status"),
        health: text("health"),
        technology: text("technology"),
        decidegrees: number("temp"),
        microvolts: number("voltage_now"),
        microamps: number("current_now"),
        charge_now: number("charge_now"),
        charge_full: number("charge_full"),
        charge_full_design: number("charge_full_design"),
    })
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

/// Whether a cable is delivering power, from the supply that carries it.
///
/// Asked before the battery's own status, because the battery's answer lags
/// the cable and sometimes never catches up. A gauge at a hundred percent
/// reports `Full` whether or not anything is plugged in, so a cable pulled
/// from a full reader left the charging mark on the panel indefinitely. The
/// USB or mains supply's `online` changes the moment the cable does.
///
/// `None` when no such supply publishes a readable `online`, so a device whose
/// driver exposes only the battery keeps the answer it had before. Also `None`
/// when one supply says `0` and another cannot be read: the unreadable one may
/// be the cable that is plugged in, and answering "unplugged" would override
/// a battery that reports it is charging.
fn cable_online(supplies: &Path) -> Option<bool> {
    let mut seen = None;
    let mut unknown = false;
    for entry in fs::read_dir(supplies).ok()?.filter_map(Result::ok).take(32) {
        let path = entry.path();
        let kind = fs::read_to_string(path.join("type")).unwrap_or_default();
        let kind = kind.trim();
        if !(kind == "USB" || kind.starts_with("USB_") || kind == "Mains" || kind == "Wireless") {
            continue;
        }
        match fs::read_to_string(path.join("online"))
            .as_deref()
            .map(str::trim)
        {
            Ok("1") => return Some(true),
            Ok("0") => seen = Some(false),
            _ => unknown = true,
        }
    }
    if unknown {
        None
    } else {
        seen
    }
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
    fn pulling_the_cable_from_a_full_battery_clears_the_charging_mark() {
        // The gauge says Full with or without a cable, so only the supply that
        // carries the cable can say it has gone.
        let root = root("cable-pulled-at-full");
        supply(&root, "battery", "Battery", "100", "Full");
        supply(&root, "usb", "USB", "", "");
        fs::write(root.join("usb/online"), "0\n").expect("online");
        assert!(!read_from(&root).expect("a battery").charging);
        fs::write(root.join("usb/online"), "1\n").expect("online");
        assert!(read_from(&root).expect("a battery").charging);
    }

    #[test]
    fn an_unreadable_cable_supply_does_not_overrule_a_charging_battery() {
        // One supply says unplugged and another cannot be read. The unreadable
        // one may be the live cable, so the battery's own status decides.
        let root = root("unreadable-cable-supply");
        supply(&root, "battery", "Battery", "60", "Charging");
        supply(&root, "usb", "USB", "", "");
        fs::write(root.join("usb/online"), "0\n").expect("online");
        supply(&root, "mains", "Mains", "", "");
        assert!(read_from(&root).expect("a battery").charging);
    }

    #[test]
    fn without_a_cable_supply_the_battery_status_still_decides() {
        let root = root("no-cable-supply");
        supply(&root, "battery", "Battery", "40", "Charging");
        assert!(read_from(&root).expect("a battery").charging);
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

//! Read-only external-power observations from the Linux power-supply ABI.
//! Unknown observations stay unknown; no device name implies a cable state.
use std::fs;
use std::io::Read;
use std::path::Path;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Observation {
    /// Any known battery charging/full or external supply online.
    pub external: Option<bool>,
    /// A supply explicitly identified as USB reports online.
    /// This is cable power, not a claim that USB storage is mounted.
    pub usb: Option<bool>,
}

#[must_use]
pub fn read() -> Observation {
    read_from(Path::new("/sys/class/power_supply"))
}

#[must_use]
pub fn read_from(root: &Path) -> Observation {
    let Ok(entries) = fs::read_dir(root) else {
        return Observation::default();
    };
    let mut external = Vec::new();
    let mut usb = Vec::new();
    for (index, entry) in entries.enumerate() {
        // A malformed fixture or driver must not turn polling into unbounded work.
        if index == 32 {
            return Observation::default();
        }
        let Ok(entry) = entry else {
            external.push(None);
            usb.push(None);
            continue;
        };
        let path = entry.path();
        match small_text(&path.join("type")).as_deref() {
            Some("Battery") => external.push(match small_text(&path.join("status")).as_deref() {
                Some("Charging" | "Full") => Some(true),
                Some("Discharging" | "Not charging") => Some(false),
                _ => None,
            }),
            Some(kind) if kind == "USB" || kind.starts_with("USB_") => {
                let online = online(&path);
                external.push(online);
                usb.push(online);
            }
            Some("Mains" | "Wireless") => external.push(online(&path)),
            // An unreadable or unfamiliar supply cannot prove absence.
            _ => {
                external.push(None);
                usb.push(None);
            }
        }
    }
    Observation {
        external: any_observed(&external),
        usb: any_observed(&usb),
    }
}

fn small_text(path: &Path) -> Option<String> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .ok()?
        .take(65)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > 64 {
        return None;
    }
    Some(String::from_utf8(bytes).ok()?.trim().to_owned())
}
fn online(path: &Path) -> Option<bool> {
    match small_text(&path.join("online")).as_deref() {
        Some("1") => Some(true),
        Some("0") => Some(false),
        _ => None,
    }
}
fn any_observed(values: &[Option<bool>]) -> Option<bool> {
    if values.contains(&Some(true)) {
        Some(true)
    } else if values.is_empty() || values.contains(&None) {
        None
    } else {
        Some(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn attachment_and_unknown_states_do_not_depend_on_driver_names() {
        let root = std::env::temp_dir().join(format!("cobalt-power-source-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        assert_eq!(read_from(&root), Observation::default());
        let battery = root.join("first");
        fs::create_dir(&battery).unwrap();
        fs::write(battery.join("type"), "Battery\n").unwrap();
        fs::write(battery.join("status"), "Discharging\n").unwrap();
        assert_eq!(
            read_from(&root),
            Observation {
                external: Some(false),
                usb: None
            }
        );
        let port = root.join("another");
        fs::create_dir(&port).unwrap();
        fs::write(port.join("type"), "USB\n").unwrap();
        for (online, expected) in [
            ("0", Some(false)),
            ("1", Some(true)),
            ("bad", None),
            ("0", Some(false)),
            ("1", Some(true)),
        ] {
            fs::write(port.join("online"), online).unwrap();
            assert_eq!(
                read_from(&root),
                Observation {
                    external: expected,
                    usb: expected
                }
            );
        }
        fs::write(port.join("type"), "Mains\n").unwrap();
        assert_eq!(
            read_from(&root),
            Observation {
                external: Some(true),
                usb: None
            }
        );
        fs::remove_dir_all(port).unwrap();
        fs::write(battery.join("status"), "Unknown\n").unwrap();
        assert_eq!(read_from(&root), Observation::default());
        fs::write(battery.join("status"), "Full\n").unwrap();
        assert_eq!(read_from(&root).external, Some(true));
        fs::write(battery.join("type"), "B".repeat(65)).unwrap();
        assert_eq!(read_from(&root), Observation::default());
        fs::remove_dir_all(root).unwrap();
    }
}

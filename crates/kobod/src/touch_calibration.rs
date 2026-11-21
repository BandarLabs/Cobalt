//! Remembering an owner's touch calibration between launches.
//!
//! A reader with no measured profile is asked to tap two dots before its first
//! real screen, and what the taps showed is kept here so the next launch does
//! not ask again. The file sits beside the consent ledger, under the
//! owner-data folder that survives an update, and holds one line per reader,
//! controller and pose. Losing it costs two taps, so a write that fails is
//! reported and otherwise ignored.

use kobo_profile::calibration::{find_in_ledger, ledger_key, ledger_with};
use kobo_profile::{DeviceSnapshot, TouchMapping};
use std::fs;
use std::path::{Path, PathBuf};

fn ledger(root: &Path) -> PathBuf {
    root.join("state").join("touch-calibration")
}

/// The calibration this reader was given on an earlier launch, if any.
#[must_use]
pub fn stored(root: &Path, snapshot: &DeviceSnapshot) -> Option<TouchMapping> {
    let key = ledger_key(snapshot)?;
    find_in_ledger(&fs::read_to_string(ledger(root)).ok()?, &key)
}

/// Records a calibration so later launches use it without asking.
///
/// # Errors
///
/// When the probe has no touch or framebuffer to key the record by, or the
/// state folder or ledger cannot be written.
pub fn remember(
    root: &Path,
    snapshot: &DeviceSnapshot,
    mapping: TouchMapping,
) -> Result<(), String> {
    let key =
        ledger_key(snapshot).ok_or_else(|| "the probe has no touch panel to key".to_owned())?;
    let path = ledger(root);
    let parent = path
        .parent()
        .ok_or_else(|| "the calibration ledger has no parent directory".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| format!("create {}: {error}", parent.display()))?;
    let existing = fs::read_to_string(&path).unwrap_or_default();
    fs::write(&path, ledger_with(&existing, &key, mapping))
        .map_err(|error| format!("write {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::{remember, stored};

    #[test]
    fn the_calibration_is_kept_under_a_name_no_application_can_take() {
        // An application that owned this record could decide where every tap
        // on the reader lands.
        let ledger = super::ledger(std::path::Path::new(""));
        let name = ledger
            .file_name()
            .and_then(|name| name.to_str())
            .expect("a name");
        assert!(kobo_app_store::is_public_reserved_app_id(name), "{name}");
    }
    use kobo_profile::calibration::ALL_MAPPINGS;
    use kobo_profile::{
        Bitfield, DeviceSnapshot, FramebufferSnapshot, IdentitySnapshot, TouchSnapshot,
    };
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn root() -> PathBuf {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "cobalt-calibration-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        path
    }

    fn snapshot() -> DeviceSnapshot {
        let byte = |offset| Bitfield {
            offset,
            length: 8,
            msb_right: 0,
        };
        DeviceSnapshot {
            compatible: vec!["fsl,imx6ull".to_owned()],
            model: None,
            framebuffer: Some(FramebufferSnapshot {
                id: "mxc_epdc_fb".to_owned(),
                width: 758,
                height: 1024,
                virtual_width: 768,
                virtual_height: 2048,
                x_offset: 0,
                y_offset: 0,
                bits_per_pixel: 32,
                grayscale: 0,
                stride: 3072,
                memory_length: 6_291_456,
                kind: 0,
                visual: 2,
                rotation: 3,
                red: byte(16),
                green: byte(8),
                blue: byte(0),
                alpha: byte(24),
            }),
            touch: Some(TouchSnapshot {
                path: "/dev/input/event1".to_owned(),
                name: "Some Touch".to_owned(),
                x_min: 0,
                x_max: 1023,
                y_min: 0,
                y_max: 757,
            }),
            identity: IdentitySnapshot {
                serial_prefix: Some("N306".into()),
                firmware_version: Some("4.38.23697".into()),
                kernel_release: None,
                device_code: Some(387),
            },
        }
    }

    #[test]
    fn a_reader_that_was_never_calibrated_has_nothing_stored() {
        assert_eq!(stored(&root(), &snapshot()), None);
    }

    #[test]
    fn a_calibration_is_there_on_the_next_launch_and_a_newer_one_replaces_it() {
        let root = root();
        remember(&root, &snapshot(), ALL_MAPPINGS[6]).expect("remember");
        assert_eq!(stored(&root, &snapshot()), Some(ALL_MAPPINGS[6]));
        remember(&root, &snapshot(), ALL_MAPPINGS[3]).expect("remember again");
        assert_eq!(stored(&root, &snapshot()), Some(ALL_MAPPINGS[3]));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_reader_turned_the_other_way_up_is_asked_again() {
        let root = root();
        remember(&root, &snapshot(), ALL_MAPPINGS[6]).expect("remember");
        let mut turned = snapshot();
        turned.framebuffer.as_mut().expect("framebuffer").rotation = 1;
        assert_eq!(stored(&root, &turned), None);
        let _ = std::fs::remove_dir_all(root);
    }
}

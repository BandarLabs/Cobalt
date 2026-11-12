//! The time zone the owner picked in Settings, kept on the card.
//!
//! One line, the zone's standard name, in the runtime's state folder. The
//! name is looked up on the list every time it is read, so a file edited by
//! hand, or written by a release whose list was longer, is simply no choice
//! rather than a rule nobody checked.
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use kobo_policy::zones::{self, Zone};
use kobo_protocol::DeviceError;

const STATE_FILE: &str = "time-zone";

fn state_file(root: &Path) -> PathBuf {
    root.join("state").join(STATE_FILE)
}

/// The zone the owner picked, if they picked one that is on the list.
#[must_use]
pub fn chosen(root: &Path) -> Option<&'static Zone> {
    let text = fs::read(state_file(root)).ok()?;
    if text.len() > zones::MAX_ZONE_ID_LEN + 2 {
        return None;
    }
    zones::find(std::str::from_utf8(&text).ok()?)
}

/// Keeps `zone` as the owner's choice, or forgets the choice for `None`.
///
/// # Errors
///
/// [`DeviceError::Backend`] when the card refuses the write.
pub fn choose(root: &Path, zone: Option<&Zone>) -> Result<(), DeviceError> {
    let path = state_file(root);
    let Some(zone) = zone else {
        return match fs::remove_file(&path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(DeviceError::Backend),
            _ => Ok(()),
        };
    };
    let parent = path.parent().ok_or(DeviceError::Backend)?;
    fs::create_dir_all(parent).map_err(|_| DeviceError::Backend)?;
    // Written beside and renamed over, so a power cut mid-write leaves the
    // old choice rather than half a name.
    let next = path.with_extension("next");
    let mut file = fs::File::create(&next).map_err(|_| DeviceError::Backend)?;
    file.write_all(zone.id.as_bytes())
        .and_then(|()| file.write_all(b"\n"))
        .and_then(|()| file.sync_all())
        .map_err(|_| DeviceError::Backend)?;
    fs::rename(&next, &path).map_err(|_| DeviceError::Backend)?;
    fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| DeviceError::Backend)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("kobod-zone-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        root
    }

    #[test]
    fn a_choice_is_kept_read_back_and_forgotten() {
        let root = scratch("kept");
        assert_eq!(chosen(&root), None);
        let kolkata = zones::find("Asia/Kolkata");
        choose(&root, kolkata).expect("kept");
        assert_eq!(chosen(&root), kolkata);
        choose(&root, None).expect("forgotten");
        assert_eq!(chosen(&root), None);
        choose(&root, None).expect("forgetting twice is not an error");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_file_that_names_no_city_on_the_list_is_no_choice() {
        let root = scratch("odd");
        fs::create_dir_all(root.join("state")).expect("state");
        fs::write(root.join("state").join(STATE_FILE), "Mars/Olympus_Mons\n").expect("written");
        assert_eq!(chosen(&root), None);
        fs::write(root.join("state").join(STATE_FILE), "x".repeat(4096)).expect("written");
        assert_eq!(chosen(&root), None);
        let _ = fs::remove_dir_all(&root);
    }
}

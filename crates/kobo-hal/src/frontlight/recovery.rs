//! Only a control name and bounded numeric values cross the recovery boundary.
use super::{to_percent, Balance, Frontlight, BACKLIGHTS};
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

const FILE: &str = "frontlight";
/// The record of the control a reader's profile chose, when that differs
/// from the one recorded before the profile was known. Crash recovery is armed
/// before the display is opened, so the first record is taken by the plain
/// rule; a profile that steers the light elsewhere adds this second one.
const PROFILE_FILE: &str = "frontlight-profile";
const LIMIT: u64 = 512;

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "invalid front light recovery record",
    )
}
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !matches!(name, "." | "..")
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

impl Frontlight {
    /// Record the exact pre-session setting before arming crash recovery or
    /// changing the light. The caller owns a private session directory. An
    /// existing record is refused rather than overwritten by a later capture.
    ///
    /// # Errors
    /// Refuses an invalid control name or a failed create/write/flush.
    pub fn save_recovery(&self, directory: &Path) -> io::Result<()> {
        self.save_record(directory, FILE)
    }

    /// Record the control the reader's profile chose, when it is not the one
    /// [`Self::save_recovery`] already recorded. Recovery puts back both.
    ///
    /// # Errors
    /// As [`Self::save_recovery`].
    pub fn save_profile_recovery(&self, directory: &Path) -> io::Result<()> {
        self.save_record(directory, PROFILE_FILE)
    }

    /// Whether two handles drive the same control.
    #[must_use]
    pub fn same_control(&self, other: &Self) -> bool {
        self.control == other.control
    }

    fn save_record(&self, directory: &Path, file: &str) -> io::Result<()> {
        let name = self
            .control
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| valid_name(name))
            .ok_or_else(invalid)?;
        let (colour_maximum, colour) = self.balance.map_or_else(
            || ("-".into(), "-".into()),
            |balance| (balance.maximum.to_string(), balance.original.to_string()),
        );
        let body = format!(
            "cobalt.frontlight.v1\n{name}\n{}\n{}\n{colour_maximum}\n{colour}\n",
            self.maximum, self.original_raw
        );
        let path = directory.join(file);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        file.write_all(body.as_bytes())?;
        file.sync_all()?;
        fs::File::open(directory)?.sync_all()
    }

    /// Restore a recorded light before restarting the stock reader. A legacy
    /// session without a record returns false; malformed records make no write.
    /// Apply the normal device/profile write gate before calling this method.
    ///
    /// # Errors
    /// Refuses malformed records, changed driver ranges and failed restoration.
    pub fn recover(directory: &Path) -> io::Result<bool> {
        Self::recover_in(Path::new(BACKLIGHTS), directory)
    }

    /// Recovery against an explicit backlight root, for host fixtures.
    ///
    /// # Errors
    /// As [`Self::recover`]. Record contents cannot select a different root.
    pub fn recover_in(backlights: &Path, directory: &Path) -> io::Result<bool> {
        let first = Self::recover_record(backlights, &directory.join(FILE))?;
        let profile = Self::recover_record(backlights, &directory.join(PROFILE_FILE))?;
        Ok(first || profile)
    }

    fn recover_record(backlights: &Path, record: &Path) -> io::Result<bool> {
        let file = match fs::File::open(record) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error),
        };
        let mut body = String::new();
        file.take(LIMIT + 1).read_to_string(&mut body)?;
        if u64::try_from(body.len()).unwrap_or(u64::MAX) > LIMIT {
            return Err(invalid());
        }
        let fields: Vec<_> = body.lines().collect();
        if fields.len() != 6 || fields[0] != "cobalt.frontlight.v1" || !valid_name(fields[1]) {
            return Err(invalid());
        }
        let number = |value: &str| value.parse::<u32>().map_err(|_| invalid());
        let maximum = number(fields[2])?;
        let original_raw = number(fields[3])?;
        if maximum == 0 || original_raw > maximum {
            return Err(invalid());
        }
        let balance = if fields[4] == "-" && fields[5] == "-" {
            None
        } else {
            let maximum = number(fields[4])?;
            let original = number(fields[5])?;
            if maximum < 2 || original > maximum {
                return Err(invalid());
            }
            Some(Balance { maximum, original })
        };
        let saved = Self {
            control: backlights.join(fields[1]),
            maximum,
            original: to_percent(original_raw, maximum),
            original_raw,
            balance,
        };
        saved.restore()?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontlight::tests::{banks, control, scratch};
    #[test]
    fn recovery_uses_the_original_capture_after_the_first_process_is_gone() {
        let root = scratch("journal");
        let journal = root.join("session");
        fs::create_dir(&journal).unwrap();
        control(&root, "light", "1", "255");
        banks(&root, "light", "3", "10");
        let light = Frontlight::open_in(&root).unwrap();
        light.save_recovery(&journal).unwrap();
        light.set(100).unwrap();
        drop(light);
        assert!(Frontlight::open_in(&root)
            .unwrap()
            .save_recovery(&journal)
            .is_err());
        assert!(Frontlight::recover_in(&root, &journal).unwrap());
        assert_eq!(
            super::super::read_number(&root.join("light/brightness")),
            Some(1)
        );
        assert_eq!(
            super::super::read_number(&root.join("light/color")),
            Some(3)
        );
        assert!(Frontlight::recover_in(&root, &journal).unwrap());
        fs::remove_dir_all(root).unwrap();
    }
    /// Crash recovery is armed before the display is opened and the reader's
    /// profile is known, so the light is recorded by the plain rule first;
    /// a profile that then steers the light to another control records that
    /// one too, and a crash puts back both.
    #[test]
    fn recovery_puts_back_the_control_a_profile_chose_after_the_plain_one() {
        let root = scratch("two-controls");
        let journal = root.join("session");
        fs::create_dir(&journal).unwrap();
        control(&root, "lm3630a_led", "40", "100");
        control(&root, "mxc_msp430.0", "30", "100");
        let plain = Frontlight::open_in(&root).unwrap();
        plain.save_recovery(&journal).unwrap();
        let chosen = Frontlight::open_in_layout(&root, kobo_profile::LightLayout::Imx).unwrap();
        assert!(!chosen.same_control(&plain));
        chosen.save_profile_recovery(&journal).unwrap();
        chosen.set(100).unwrap();
        plain.set(0).unwrap();
        drop((plain, chosen));
        assert!(Frontlight::recover_in(&root, &journal).unwrap());
        let read = |name: &str| super::super::read_number(&root.join(name).join("brightness"));
        assert_eq!(read("lm3630a_led"), Some(40));
        assert_eq!(read("mxc_msp430.0"), Some(30));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_records_cannot_change_a_control_or_escape_the_backlight_root() {
        let root = scratch("bad-journal");
        let journal = root.join("session");
        fs::create_dir(&journal).unwrap();
        control(&root, "light", "7", "100");
        assert!(!Frontlight::recover_in(&root, &journal).unwrap());
        for body in [
            "cobalt.frontlight.v1\n../light\n100\n20\n-\n-\n".into(),
            "cobalt.frontlight.v1\nlight\n100\n200\n-\n-\n".into(),
            "cobalt.frontlight.v1\nlight\n100\n20\n10\n11\n".into(),
            "x".repeat(513),
        ] {
            fs::write(journal.join(FILE), body).unwrap();
            assert!(Frontlight::recover_in(&root, &journal).is_err());
            assert_eq!(
                super::super::read_number(&root.join("light/brightness")),
                Some(7)
            );
        }
        fs::remove_dir_all(root).unwrap();
    }
}

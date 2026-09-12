//! Asking before running on hardware nobody has measured.
//!
//! Cobalt used to refuse any reader outside its table outright, and the refusal
//! reached a log file nobody had a reason to look in. From the owner's chair a
//! menu entry simply did nothing, which is the worst of both: no session, and
//! no way to find out why.
//!
//! It now runs, and asks first. The question is deliberately narrow. It states
//! what has not been tested about this exact device, says what could go wrong
//! in terms of what the owner would see, and offers to go back. Nothing here
//! claims the session is safe, because that is the one thing nobody has the
//! evidence to say.
//!
//! # Why acceptance is recorded and refusal is not
//!
//! An owner who accepts has decided something about their hardware that will
//! not change until the hardware or the firmware does, so asking again every
//! launch would be nagging. An owner who declines has decided nothing except
//! "not now", and the honest response to that is to ask again next time rather
//! than to remember a no and act on it forever.

use kobo_profile::{DeviceProfile, Standing};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

/// What the owner chose.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Decision {
    /// Run, and do not ask again for this device on this firmware branch.
    Accepted,
    /// Hand the reader back. Nothing is recorded, so the next launch asks.
    Declined,
}

/// The wording of one notice, resolved from what is actually unproven.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Notice {
    pub title: String,
    pub body: Vec<String>,
    /// Whether the panel's touch mapping is itself unverified, which decides
    /// if the owner is told how to answer without using it.
    pub touch_may_be_wrong: bool,
}

/// Where acceptances are kept, under the owner-data folder that survives an
/// update. A line per device and branch, so a reader that has been through two
/// firmware branches carries the record of both.
fn ledger(root: &Path) -> PathBuf {
    root.join("state").join("accepted-untested")
}

/// The key one acceptance is remembered by.
///
/// Model and firmware branch, and nothing narrower. A build number would ask
/// again on every wave Kobo ships, which is the nagging this is meant to
/// avoid, and a model alone would carry an answer about 4.45 silently forward
/// onto a 4.46 that changed something.
fn key(profile: &DeviceProfile, firmware: &str) -> Option<String> {
    let branch = kobo_profile::firmware_branch(firmware)?;
    Some(format!("{} {branch}", profile.serial_prefix))
}

/// Whether this device on this firmware branch has already been accepted.
#[must_use]
pub fn already_accepted(root: &Path, profile: &DeviceProfile, firmware: &str) -> bool {
    let Some(key) = key(profile, firmware) else {
        return false;
    };
    let Ok(ledger) = fs::read_to_string(ledger(root)) else {
        return false;
    };
    ledger.lines().any(|line| {
        line.split_whitespace()
            .take(2)
            .collect::<Vec<_>>()
            .join(" ")
            == key
    })
}

/// Records an acceptance durably.
///
/// The exact firmware build and the Cobalt version are written alongside the
/// key even though neither is matched on. What an owner agreed to is worth
/// being able to reconstruct later, and a ledger that only holds the question
/// cannot answer when it was asked.
///
/// # Errors
///
/// When the state folder cannot be created or the ledger cannot be written.
pub fn record(
    root: &Path,
    profile: &DeviceProfile,
    firmware: &str,
    cobalt_version: &str,
) -> Result<(), String> {
    let Some(key) = key(profile, firmware) else {
        return Err(format!("the firmware version {firmware} names no branch"));
    };
    let path = ledger(root);
    let parent = path
        .parent()
        .ok_or_else(|| "the consent ledger has no parent directory".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| format!("create {}: {error}", parent.display()))?;
    let mut existing = fs::read_to_string(&path).unwrap_or_default();
    if !existing.is_empty() && !existing.ends_with('\n') {
        existing.push('\n');
    }
    let _ = writeln!(existing, "{key} {firmware} cobalt-{cobalt_version}");
    fs::write(&path, existing).map_err(|error| format!("write {}: {error}", path.display()))
}

/// The notice to put in front of the owner, or `None` when the hardware and
/// the firmware are both covered by evidence and there is nothing to ask.
#[must_use]
pub fn notice(standing: Standing, profile: &DeviceProfile, firmware: &str) -> Option<Notice> {
    match standing {
        Standing::Measured => None,
        Standing::UntestedFirmware => Some(Notice {
            title: "Untested firmware".to_owned(),
            body: vec![
                format!(
                    "Cobalt is tested on {} builds of this Kobo. Yours is on {firmware}. Every hardware check passed.",
                    profile.firmware_branches().join(" and ")
                ),
                "Cobalt does not change how your Kobo starts up. Restart to return to the normal reader.".to_owned(),
                "Provided without warranty, at your own risk.".to_owned(),
            ],
            touch_may_be_wrong: false,
        }),
        Standing::Unmeasured => Some(Notice {
            title: "Untested device".to_owned(),
            body: vec![
                "Cobalt has not been tested on this Kobo. The screen may not draw correctly and taps may land in the wrong place.".to_owned(),
                "Cobalt does not change how your Kobo starts up. Restart to return to the normal reader.".to_owned(),
                "Provided without warranty, at your own risk. Not affiliated with Kobo.".to_owned(),
            ],
            touch_may_be_wrong: true,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::{already_accepted, key, notice, record, Decision, Standing};
    use kobo_profile::CLARA_BW_391;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A root of its own per test, so two ledgers never share a file.
    fn root() -> PathBuf {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "cobalt-consent-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        path
    }

    #[test]
    fn a_measured_device_on_a_measured_branch_is_asked_nothing() {
        assert!(notice(Standing::Measured, &CLARA_BW_391, "4.45.23697").is_none());
    }

    #[test]
    fn an_untested_branch_names_the_branch_that_was_tested_and_the_one_running() {
        let notice = notice(Standing::UntestedFirmware, &CLARA_BW_391, "4.46.23836")
            .expect("a notice is owed");
        assert_eq!(notice.title, "Untested firmware");
        assert!(notice.body[0].contains("4.45"));
        assert!(notice.body[0].contains("4.46.23836"));
        assert!(!notice.touch_may_be_wrong);
    }

    #[test]
    fn an_unmeasured_device_is_told_that_taps_may_land_in_the_wrong_place() {
        // The notice on unmeasured hardware has to survive being read on a
        // panel whose touch mapping is itself a guess, so the owner is told
        // that before they try to answer with it.
        let notice =
            notice(Standing::Unmeasured, &CLARA_BW_391, "4.28.17623").expect("a notice is owed");
        assert_eq!(notice.title, "Untested device");
        assert!(notice.touch_may_be_wrong);
        assert!(notice.body.iter().any(|line| line.contains("wrong place")));
    }

    #[test]
    fn every_notice_disclaims_warranty_without_claiming_the_session_is_safe() {
        for standing in [Standing::UntestedFirmware, Standing::Unmeasured] {
            let notice = notice(standing, &CLARA_BW_391, "4.46.23836").expect("a notice is owed");
            let text = notice.body.join(" ");
            assert!(text.contains("without warranty"), "{text}");
            assert!(text.contains("at your own risk"), "{text}");
            assert!(!text.contains("safe"), "{text}");
        }
    }

    #[test]
    fn an_acceptance_is_remembered_for_the_branch_it_was_given_on() {
        let root = root();
        assert!(!already_accepted(
            root.as_path(),
            &CLARA_BW_391,
            "4.45.23792"
        ));
        record(root.as_path(), &CLARA_BW_391, "4.45.23792", "0.3.12").expect("recorded");
        assert!(already_accepted(
            root.as_path(),
            &CLARA_BW_391,
            "4.45.23792"
        ));
    }

    #[test]
    fn a_later_build_on_an_accepted_branch_is_not_asked_about_again() {
        let root = root();
        record(root.as_path(), &CLARA_BW_391, "4.45.23792", "0.3.12").expect("recorded");
        assert!(already_accepted(
            root.as_path(),
            &CLARA_BW_391,
            "4.45.23999"
        ));
    }

    #[test]
    fn accepting_one_branch_says_nothing_about_the_next_one() {
        // A branch bump is where Kobo has historically changed something worth
        // looking at, so an answer about 4.45 must not carry onto 4.46.
        let root = root();
        record(root.as_path(), &CLARA_BW_391, "4.45.23792", "0.3.12").expect("recorded");
        assert!(!already_accepted(
            root.as_path(),
            &CLARA_BW_391,
            "4.46.23836"
        ));
    }

    #[test]
    fn declining_records_nothing_so_the_next_launch_asks_again() {
        // There is no call that writes a refusal, and this is the test that
        // keeps it that way: the ledger a decline leaves behind is empty.
        let root = root();
        assert_eq!(Decision::Declined, Decision::Declined);
        assert!(!already_accepted(
            root.as_path(),
            &CLARA_BW_391,
            "4.45.23792"
        ));
    }

    #[test]
    fn a_firmware_version_naming_no_branch_can_neither_be_recorded_nor_matched() {
        let root = root();
        assert!(key(&CLARA_BW_391, "unknown").is_none());
        assert!(record(root.as_path(), &CLARA_BW_391, "unknown", "0.3.12").is_err());
        assert!(!already_accepted(root.as_path(), &CLARA_BW_391, "unknown"));
    }

    #[test]
    fn a_second_acceptance_does_not_lose_the_first() {
        let root = root();
        record(root.as_path(), &CLARA_BW_391, "4.45.23792", "0.3.12").expect("recorded");
        record(root.as_path(), &CLARA_BW_391, "4.46.23836", "0.3.12").expect("recorded");
        assert!(already_accepted(
            root.as_path(),
            &CLARA_BW_391,
            "4.45.23792"
        ));
        assert!(already_accepted(
            root.as_path(),
            &CLARA_BW_391,
            "4.46.23836"
        ));
    }
}

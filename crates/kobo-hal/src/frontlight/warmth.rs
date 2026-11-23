//! Front light warmth.
//!
//! The balance between the two lamp banks, read and set as a setting in its
//! own right. The parent module moves the same file for a different reason,
//! to light both banks at the top of the brightness range, and never needs to
//! know which bank is the warm one. Setting a warmth does, and nothing on the
//! reader says: the file reads the same whichever way it runs. So every call
//! here takes the reader's [`Warmth`] from its profile, and a reader without
//! a mapping is refused before anything is read or written.
//!
//! # Why percentages, and why the steps are reported too
//!
//! Applications ask in percent for the reason they do for brightness: 0 is
//! the coolest light this reader makes and 100 the warmest, whatever the
//! control counts in and whichever way it runs. But the control is coarse,
//! eleven settings on every reader known, and a caller offering a plus and a
//! minus wants each tap to move the lamp exactly once. So a reading says how
//! many settings there are as well as where the lamp is among them.
//!
//! # Why this fails closed
//!
//! The driver keeps whatever number it is handed without checking it against
//! its own maximum, and once it holds one past the end every later brightness
//! write is accepted and changes nothing. A light that has stopped answering
//! is a worse outcome than a warmth that could not be set, so nothing is
//! written unless the range read when the light was opened is still the
//! range now, the value there is inside it, and the value going in is
//! clamped to it.

use super::{read_number, to_percent, to_raw, Frontlight, PREFERRED};
use kobo_profile::Warmth;
use std::io;

/// The longest scale a warmth is set on.
///
/// A percentage names 101 places, so on a scale no longer than this every
/// setting has a percentage of its own and asking for that percentage lands
/// on that setting. A longer one would have settings no request could reach,
/// and no reader known comes near it: theirs all end at 10.
const LONGEST_SCALE: u32 = 100;

/// Where the warmth stands, and how finely it can be set.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WarmthReading {
    /// 0 is the coolest light the reader makes and 100 the warmest.
    pub percent: u8,
    /// How many settings the lamp has, both ends counted: 11 on a control
    /// that runs from 0 to 10.
    pub steps: u8,
}

/// A warmth control that has passed every check, and the way it runs.
#[derive(Clone, Copy)]
struct Scale {
    maximum: u32,
    warmest_at_zero: bool,
}

impl Scale {
    /// Turns the driver's number into a place counted from the cool end, and
    /// a place back into the driver's number. One function for both, because
    /// turning a scale end for end twice is leaving it alone.
    const fn turned(self, value: u32) -> u32 {
        if self.warmest_at_zero {
            self.maximum - value
        } else {
            value
        }
    }

    fn reading(self, raw: u32) -> WarmthReading {
        WarmthReading {
            percent: to_percent(self.turned(raw), self.maximum),
            // Checked against `LONGEST_SCALE` before a scale is made.
            steps: u8::try_from(self.maximum + 1).unwrap_or(u8::MAX),
        }
    }
}

/// The refusal for a reader whose warmth this runtime does not set.
///
/// A kind of its own so the caller can tell "not on this reader" from "it
/// went wrong this time": the first is settled for the session, and the
/// second may work on the next try.
fn unsupported(why: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, why)
}

impl Frontlight {
    /// The warmth control, once the profile, the light that was opened and
    /// the files as they are now all agree it can be used.
    fn warmth_scale(&self, mapping: Warmth) -> io::Result<Scale> {
        let Warmth::Balance { warmest_at_zero } = mapping else {
            return Err(unsupported("this reader has no warmth mapping"));
        };
        // The mapping names the aggregate's own file. A light that was
        // opened on some other control may publish a `color` too, and what
        // that one means is not what the profile described.
        if self
            .control
            .file_name()
            .is_none_or(|name| name != PREFERRED)
        {
            return Err(unsupported("the light is not the mapped control"));
        }
        // What was not captured when the light was opened could not be put
        // back when the session ends, so it is not moved.
        let Some(balance) = self.balance else {
            return Err(unsupported("no warmth control was found with the light"));
        };
        if balance.maximum > LONGEST_SCALE {
            return Err(unsupported("the warmth scale is longer than is mapped"));
        }
        if read_number(&self.control.join("max_color")) != Some(balance.maximum) {
            return Err(io::Error::other("warmth range changed since capture"));
        }
        Ok(Scale {
            maximum: balance.maximum,
            warmest_at_zero,
        })
    }

    /// The driver's own number for the warmth, when it is one the scale has.
    fn warmth_raw(&self, scale: Scale) -> io::Result<u32> {
        let raw = read_number(&self.control.join("color"))
            .ok_or_else(|| io::Error::other("warmth control missing or unreadable"))?;
        if raw > scale.maximum {
            return Err(io::Error::other(
                "warmth control holds a value off its scale",
            ));
        }
        Ok(raw)
    }

    /// Where the warmth is set now.
    ///
    /// Read from the lamp and not remembered, for the reason the brightness
    /// is: the stock reader is still running, and a brightness set at the top
    /// of the range moves this file too until a warmth has been chosen.
    ///
    /// # Errors
    ///
    /// [`io::ErrorKind::Unsupported`] when `mapping` is
    /// [`Warmth::Unmapped`], when the light that was opened is not the
    /// control the mapping describes, or when that control had no usable
    /// warmth files when it was opened. Any other error is a control that
    /// has gone missing, changed its range or holds a value off its scale
    /// since.
    pub fn warmth(&self, mapping: Warmth) -> io::Result<WarmthReading> {
        let scale = self.warmth_scale(mapping)?;
        Ok(scale.reading(self.warmth_raw(scale)?))
    }

    /// Sets the warmth to the setting nearest `percent`, and reports where
    /// the lamp then is.
    ///
    /// Only the balance is written. The driver works the level it holds out
    /// again across the banks whenever the balance changes, so the
    /// brightness is left as it is, and a light that is off stays off.
    ///
    /// From this call until [`Self::restore`], [`Self::set`] leaves the
    /// balance alone at every level. What the session found is still what
    /// gets put back when it ends.
    ///
    /// # Errors
    ///
    /// As [`Self::warmth`], checked before anything is written, and when the
    /// write is refused or the control does not read back what was written.
    pub fn set_warmth(&self, mapping: Warmth, percent: u8) -> io::Result<WarmthReading> {
        // Held from the checks to the write, so a brightness set from
        // another handle cannot move the balance back in between.
        let mut chosen = self.chosen_warmth();
        let scale = self.warmth_scale(mapping)?;
        self.warmth_raw(scale)?;
        let raw = scale.turned(to_raw(percent, scale.maximum));
        self.balance(raw)?;
        // Remembered as soon as the file has been written, before the check
        // below: whatever the driver made of it, the balance is no longer
        // the one the brightness rule knows how to hand back.
        *chosen = Some(raw);
        if self.warmth_raw(scale)? != raw {
            return Err(io::Error::other("warmth control did not take the value"));
        }
        Ok(scale.reading(raw))
    }
}

#[cfg(test)]
mod tests {
    use super::{WarmthReading, LONGEST_SCALE};
    use crate::frontlight::tests::{banks, control, scratch};
    use crate::frontlight::{read_number, to_percent, to_raw, Frontlight, PREFERRED};
    use kobo_profile::{Warmth, CLARA_BW_391, MEDIATEK_BALANCE, SUPPORTED_PROFILES};
    use std::fs;
    use std::io;
    use std::path::Path;

    /// A reader whose control counts up towards warm.
    const WARMEST_AT_THE_TOP: Warmth = Warmth::Balance {
        warmest_at_zero: false,
    };
    /// A reader whose control counts down towards warm, as the mapped ones do.
    const WARMEST_AT_ZERO: Warmth = Warmth::Balance {
        warmest_at_zero: true,
    };

    /// A light at `brightness` out of 100 with its balance at `colour` out of
    /// 10, which is what every mapped reader publishes.
    fn light(name: &str, brightness: &str, colour: &str) -> (std::path::PathBuf, Frontlight) {
        let root = scratch(name);
        control(&root, PREFERRED, brightness, "100");
        banks(&root, PREFERRED, colour, "10");
        let light = Frontlight::open_in(&root).expect("a light");
        (root, light)
    }

    fn colour(root: &Path) -> Option<u32> {
        read_number(&root.join(PREFERRED).join("color"))
    }

    fn brightness(root: &Path) -> Option<u32> {
        read_number(&root.join(PREFERRED).join("brightness"))
    }

    const fn reading(percent: u8) -> WarmthReading {
        WarmthReading { percent, steps: 11 }
    }

    #[test]
    fn a_reading_counts_from_the_cool_end_whichever_way_the_control_runs() {
        for (raw, at_zero, at_the_top) in
            [("0", 100, 0), ("3", 70, 30), ("5", 50, 50), ("10", 0, 100)]
        {
            let (root, light) = light("warmth-read", "17", raw);
            assert_eq!(
                light.warmth(WARMEST_AT_ZERO).unwrap(),
                reading(at_zero),
                "{raw}"
            );
            assert_eq!(
                light.warmth(WARMEST_AT_THE_TOP).unwrap(),
                reading(at_the_top),
                "{raw}"
            );
            fs::remove_dir_all(root).unwrap();
        }
    }

    /// The fact the mapped profiles carry, read through a shipped profile and
    /// not a constant made up for the test, so turning the direction over in
    /// the profile table shows up here.
    #[test]
    fn a_mapped_reader_calls_zero_its_warmest_light() {
        assert_eq!(CLARA_BW_391.warmth, MEDIATEK_BALANCE);
        let (root, light) = light("warmth-shipped", "17", "0");
        assert_eq!(light.warmth(CLARA_BW_391.warmth).unwrap(), reading(100));
        assert_eq!(
            light.set_warmth(CLARA_BW_391.warmth, 0).unwrap(),
            reading(0)
        );
        assert_eq!(colour(&root), Some(10));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_warmth_lands_on_the_nearest_setting_and_never_past_either_end() {
        for (asked, set, cool_end_first) in [
            (0, 0, 0),
            (4, 0, 0),
            (5, 10, 1),
            (26, 30, 3),
            (50, 50, 5),
            (94, 90, 9),
            (95, 100, 10),
            (100, 100, 10),
            (u8::MAX, 100, 10),
        ] {
            let (root, light) = light("warmth-set", "17", "4");
            assert_eq!(
                light.set_warmth(WARMEST_AT_THE_TOP, asked).unwrap(),
                reading(set),
                "{asked}"
            );
            assert_eq!(colour(&root), Some(cool_end_first), "{asked}");
            assert_eq!(
                light.set_warmth(WARMEST_AT_ZERO, asked).unwrap(),
                reading(set),
                "{asked}"
            );
            assert_eq!(colour(&root), Some(10 - cool_end_first), "{asked}");
            assert_eq!(light.warmth(WARMEST_AT_ZERO).unwrap(), reading(set));
            fs::remove_dir_all(root).unwrap();
        }
    }

    /// A caller moving one setting at a time asks for the percentage a
    /// reading would give that setting, so that has to land on it, on every
    /// scale a warmth is set on.
    #[test]
    fn every_setting_of_every_scale_is_reached_by_its_own_percentage() {
        for maximum in 2..=LONGEST_SCALE {
            for step in 0..=maximum {
                assert_eq!(
                    to_raw(to_percent(step, maximum), maximum),
                    step,
                    "{step} of {maximum}"
                );
            }
        }
    }

    /// The driver applies a new balance to the level it already holds, so
    /// the level is not written again, and a light that is off is not lit
    /// by choosing its warmth.
    #[test]
    fn setting_the_warmth_leaves_the_brightness_as_it_was() {
        for level in ["0", "17", "100"] {
            let (root, light) = light("warmth-brightness", level, "4");
            light.set_warmth(WARMEST_AT_ZERO, 80).unwrap();
            assert_eq!(colour(&root), Some(2));
            assert_eq!(
                fs::read_to_string(root.join(PREFERRED).join("brightness")).unwrap(),
                level
            );
            fs::remove_dir_all(root).unwrap();
        }
    }

    /// Before a warmth is chosen the top of the range lights every bank, as
    /// it always has. Once one is chosen, no level moves it: not the top,
    /// and not the step back down that used to hand the old balance back.
    #[test]
    fn a_chosen_warmth_stays_where_it_was_put_at_every_brightness() {
        let (root, light) = light("warmth-kept", "17", "10");
        light.set(100).unwrap();
        assert_eq!(colour(&root), Some(5), "nothing chosen yet");
        light.set(40).unwrap();
        assert_eq!(colour(&root), Some(10), "nothing chosen yet");

        light.set_warmth(WARMEST_AT_ZERO, 80).unwrap();
        for level in [100, 99, 40, 0, 100] {
            assert_eq!(light.set(level).unwrap(), level);
            assert_eq!(colour(&root), Some(2), "at {level}");
            assert_eq!(brightness(&root), Some(u32::from(level)));
        }
        // A second choice replaces the first and is kept the same way.
        light.set_warmth(WARMEST_AT_ZERO, 0).unwrap();
        light.set(100).unwrap();
        assert_eq!(colour(&root), Some(10));
        fs::remove_dir_all(root).unwrap();
    }

    /// The session ends with the reader as it was found: the balance its
    /// owner had, not the one chosen here, by the ordinary way out and by
    /// the record a crashed session leaves behind.
    #[test]
    fn the_session_ends_with_the_warmth_and_brightness_it_found() {
        let (root, light) = light("warmth-restore", "17", "3");
        let journal = root.join("session");
        fs::create_dir(&journal).unwrap();
        light.save_recovery(&journal).unwrap();
        let record = fs::read(journal.join("frontlight")).unwrap();

        light.set_warmth(WARMEST_AT_ZERO, 100).unwrap();
        light.set(100).unwrap();
        assert_eq!((colour(&root), brightness(&root)), (Some(0), Some(100)));
        assert_eq!(light.restore().unwrap(), 17);
        assert_eq!((colour(&root), brightness(&root)), (Some(3), Some(17)));

        // The choice went with the session, so the top of the range lights
        // every bank again.
        light.set(100).unwrap();
        assert_eq!(colour(&root), Some(5));

        light.set_warmth(WARMEST_AT_ZERO, 0).unwrap();
        light.set(60).unwrap();
        assert_eq!(
            fs::read(journal.join("frontlight")).unwrap(),
            record,
            "what was found is recorded once and never rewritten"
        );
        drop(light);
        assert!(Frontlight::recover_in(&root, &journal).unwrap());
        assert_eq!((colour(&root), brightness(&root)), (Some(3), Some(17)));
        fs::remove_dir_all(root).unwrap();
    }

    /// The runtime hands clones of one light about. They are the same lamp,
    /// so a warmth chosen through one binds the brightness set through
    /// another.
    #[test]
    fn a_warmth_chosen_through_one_handle_is_kept_by_every_other() {
        let (root, light) = light("warmth-clones", "17", "10");
        let earlier = light.clone();
        light.set_warmth(WARMEST_AT_ZERO, 80).unwrap();
        earlier.set(100).unwrap();
        assert_eq!(colour(&root), Some(2));
        earlier.restore().unwrap();
        light.set(100).unwrap();
        assert_eq!(colour(&root), Some(5), "restoring through one frees all");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_reader_without_a_mapping_is_refused_and_nothing_is_written() {
        let (root, light) = light("warmth-unmapped", "17", "3");
        let unmapped: Vec<_> = SUPPORTED_PROFILES
            .iter()
            .filter(|profile| profile.warmth == Warmth::Unmapped)
            .collect();
        assert!(!unmapped.is_empty());
        for profile in unmapped {
            for refused in [
                light.warmth(profile.warmth),
                light.set_warmth(profile.warmth, 100),
            ] {
                assert_eq!(
                    refused.unwrap_err().kind(),
                    io::ErrorKind::Unsupported,
                    "{}",
                    profile.id
                );
            }
        }
        assert_eq!((colour(&root), brightness(&root)), (Some(3), Some(17)));
        // Refused, so nothing was chosen and the old rule still holds.
        light.set(100).unwrap();
        assert_eq!(colour(&root), Some(5));
        fs::remove_dir_all(root).unwrap();
    }

    /// A mapping describes one control. A light that was found under another
    /// name, or that had no usable warmth files when it was opened, is not
    /// that control, whatever turns up beside it later.
    #[test]
    fn a_light_that_is_not_the_mapped_control_is_refused_as_unsupported() {
        let unsupported = |light: &Frontlight| {
            for refused in [
                light.warmth(WARMEST_AT_ZERO),
                light.set_warmth(WARMEST_AT_ZERO, 100),
            ] {
                assert_eq!(refused.unwrap_err().kind(), io::ErrorKind::Unsupported);
            }
        };

        let root = scratch("warmth-other-name");
        control(&root, "some_other_led", "17", "100");
        banks(&root, "some_other_led", "3", "10");
        unsupported(&Frontlight::open_in(&root).unwrap());
        assert_eq!(
            read_number(&root.join("some_other_led/color")),
            Some(3),
            "left alone"
        );
        fs::remove_dir_all(root).unwrap();

        // No files at all, then files that arrive after the light was
        // opened and so were never captured for putting back.
        let root = scratch("warmth-no-files");
        control(&root, PREFERRED, "17", "100");
        let light = Frontlight::open_in(&root).unwrap();
        unsupported(&light);
        banks(&root, PREFERRED, "3", "10");
        unsupported(&light);
        assert_eq!(colour(&root), Some(3));
        fs::remove_dir_all(root).unwrap();

        // A range that is missing, is not a number, has no middle, or is
        // longer than a percentage can address.
        for maximum in [
            None,
            Some("ten"),
            Some("0"),
            Some("1"),
            Some("101"),
            Some("255"),
        ] {
            let root = scratch("warmth-range");
            control(&root, PREFERRED, "17", "100");
            banks(&root, PREFERRED, "0", maximum.unwrap_or("10"));
            if maximum.is_none() {
                fs::remove_file(root.join(PREFERRED).join("max_color")).unwrap();
            }
            unsupported(&Frontlight::open_in(&root).unwrap());
            assert_eq!(colour(&root), Some(0), "{maximum:?}");
            fs::remove_dir_all(root).unwrap();
        }

        // The i.MX layout never captures the file, so it is never set.
        let root = scratch("warmth-imx");
        control(&root, PREFERRED, "17", "100");
        banks(&root, PREFERRED, "3", "10");
        unsupported(&Frontlight::open_in_layout(&root, kobo_profile::LightLayout::Imx).unwrap());
        assert_eq!(colour(&root), Some(3));
        fs::remove_dir_all(root).unwrap();
    }

    /// The driver keeps a value past its maximum unchecked and then ignores
    /// brightness writes, so a control that no longer looks as it did when
    /// the light was opened is not written at all.
    #[test]
    fn a_control_that_went_missing_or_wrong_since_the_light_was_opened_fails_unwritten() {
        for (file, value) in [
            ("color", None),
            ("color", Some("warm")),
            ("color", Some("11")),
            ("color", Some("-1")),
            ("max_color", None),
            ("max_color", Some("ten")),
            ("max_color", Some("20")),
            ("max_color", Some("5")),
        ] {
            let (root, light) = light("warmth-changed", "17", "3");
            let path = root.join(PREFERRED).join(file);
            match value {
                Some(value) => fs::write(&path, value).unwrap(),
                None => fs::remove_file(&path).unwrap(),
            }
            let files = || {
                ["brightness", "max_brightness", "color", "max_color"]
                    .map(|file| fs::read_to_string(root.join(PREFERRED).join(file)).ok())
            };
            let before = files();
            for failed in [
                light.warmth(WARMEST_AT_ZERO),
                light.set_warmth(WARMEST_AT_ZERO, 100),
            ] {
                assert_ne!(
                    failed.unwrap_err().kind(),
                    io::ErrorKind::Unsupported,
                    "{file} = {value:?}: this is a failure, not a settled answer"
                );
            }
            assert_eq!(files(), before, "{file} = {value:?}");
            fs::remove_dir_all(root).unwrap();
        }
    }

    /// A write the control refuses is an error for the caller to report. It
    /// is never answered with the warmth that was asked for.
    #[test]
    fn a_warmth_the_control_refuses_is_an_error_and_not_the_value_asked_for() {
        let (root, light) = light("warmth-refused", "17", "3");
        let path = root.join(PREFERRED).join("color");
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&path, permissions).unwrap();
        // Root writes through a read-only mode, and some builders run as it.
        if fs::OpenOptions::new().write(true).open(&path).is_ok() {
            fs::remove_dir_all(root).unwrap();
            return;
        }
        assert!(light.set_warmth(WARMEST_AT_ZERO, 100).is_err());
        assert_eq!(colour(&root), Some(3));
        assert_eq!(light.warmth(WARMEST_AT_ZERO).unwrap(), reading(70));
        fs::remove_dir_all(root).unwrap();
    }
}

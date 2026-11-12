//! Working out how an unmeasured digitiser sits under its panel.
//!
//! A provisional profile can read almost everything about a reader from the
//! kernel, but not which way round the touch controller was mounted. Every
//! Kobo measured so far has its touch axes swapped, mirrored, or both,
//! relative to the display, so the identity mapping a provisional profile
//! used to start from put taps in the wrong place on all of them.
//!
//! Two taps settle it. The owner is shown a dot, taps it, is shown a second
//! one, taps that, and the raw positions the controller reported are compared
//! with where the dots were drawn. Of the eight ways the axes can be swapped
//! and mirrored, exactly one puts both taps near their dots.
//!
//! # Why the dots sit along one edge rather than at opposite corners
//!
//! Each raw axis is measured against its own range, so in that frame the panel
//! is a unit square, and the diagonal of a square is the line that swapping
//! the axes leaves where it is. Two dots at opposite corners therefore cannot
//! tell a swapped controller from one that is not: both report the same pair
//! of positions. Two dots at the ends of one edge can. Every swap or mirror
//! other than the true one moves at least one of them most of the way across
//! the panel.

use crate::{DeviceProfile, DeviceSnapshot, TouchMapping};

/// How far in from the panel's edges the dots are drawn, in percent.
///
/// Far enough in that a finger can land on the dot without catching the
/// bezel, and close enough to the corners that every wrong mapping moves at
/// least one dot by more than half the panel.
const INSET_PERCENT: u64 = 15;

/// How far a tap may land from its dot and still count, as a fraction of the
/// panel.
///
/// Any mapping other than the true one moves at least one dot most of the way
/// across the panel: the whole width less the inset at each end, which is
/// 0.7. A tolerance under half of that cannot accept two mappings at once.
/// This one leaves a finger plenty of room and stays well inside that limit.
const TOLERANCE: f64 = 0.25;

/// Why two taps did not produce a mapping.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CalibrationError {
    /// The profile's touch ranges cannot scale a tap.
    NoTouchRange,
    /// A tap was outside the range the controller says it reports.
    OutOfRange,
    /// No arrangement of the axes puts both taps near their dots, which
    /// usually means one of them missed.
    NoMatch,
}

impl std::fmt::Display for CalibrationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::NoTouchRange => "the touch controller reports no usable range",
            Self::OutOfRange => "a tap was outside the range the touch controller reports",
            Self::NoMatch => "the two taps did not land near the two dots",
        })
    }
}

impl std::error::Error for CalibrationError {}

/// Every way a digitiser can be swapped and mirrored against a panel.
pub const ALL_MAPPINGS: [TouchMapping; 8] = {
    let mut all = [TouchMapping {
        swap_axes: false,
        mirror_x: false,
        mirror_y: false,
    }; 8];
    let mut index = 0;
    while index < 8 {
        all[index] = TouchMapping {
            swap_axes: index & 4 != 0,
            mirror_x: index & 2 != 0,
            mirror_y: index & 1 != 0,
        };
        index += 1;
    }
    all
};

/// Where the two dots are drawn on a panel of this size, in display pixels.
///
/// The first near the top-left corner and the second near the top-right.
#[must_use]
pub fn targets(width: u32, height: u32) -> [(u32, u32); 2] {
    let inset = |extent: u32| -> u32 {
        let last = u64::from(extent.saturating_sub(1));
        u32::try_from((last * INSET_PERCENT + 50) / 100).unwrap_or(0)
    };
    let (x, y) = (inset(width), inset(height));
    [(x, y), (width.saturating_sub(1).saturating_sub(x), y)]
}

/// Finds the one mapping that puts both taps near their dots.
///
/// `taps` are the raw controller positions, in the order the dots were shown.
///
/// # Errors
///
/// When the profile's touch range is empty, a tap is outside it, or no
/// mapping explains both taps.
pub fn derive(
    profile: &DeviceProfile,
    width: u32,
    height: u32,
    taps: [(i32, i32); 2],
) -> Result<TouchMapping, CalibrationError> {
    if width < 2 || height < 2 {
        return Err(CalibrationError::NoTouchRange);
    }
    let raw = [
        normalise_raw(profile, taps[0])?,
        normalise_raw(profile, taps[1])?,
    ];
    let dots = targets(width, height).map(|(x, y)| {
        (
            f64::from(x) / f64::from(width - 1),
            f64::from(y) / f64::from(height - 1),
        )
    });
    let mut accepted = ALL_MAPPINGS.into_iter().filter(|mapping| {
        raw.iter().zip(dots).all(|(&tap, dot)| {
            let (x, y) = apply(*mapping, tap);
            (x - dot.0).hypot(y - dot.1) <= TOLERANCE
        })
    });
    match (accepted.next(), accepted.next()) {
        (Some(mapping), None) => Ok(mapping),
        _ => Err(CalibrationError::NoMatch),
    }
}

/// A raw position as a fraction of the range the controller reports.
fn normalise_raw(
    profile: &DeviceProfile,
    (x, y): (i32, i32),
) -> Result<(f64, f64), CalibrationError> {
    let axis = |value: i32, minimum: i32, maximum: i32| {
        if maximum <= minimum {
            return Err(CalibrationError::NoTouchRange);
        }
        if !(minimum..=maximum).contains(&value) {
            return Err(CalibrationError::OutOfRange);
        }
        Ok((f64::from(value) - f64::from(minimum)) / (f64::from(maximum) - f64::from(minimum)))
    };
    Ok((
        axis(x, profile.touch_x_min, profile.touch_x_max)?,
        axis(y, profile.touch_y_min, profile.touch_y_max)?,
    ))
}

/// Where a mapping puts a raw fraction on the display, as a fraction.
///
/// The same order of operations as [`crate::PanelPose::touch_to_display`]:
/// the swap picks which raw axis feeds which display axis, and the mirrors are
/// then applied in display space.
fn apply(mapping: TouchMapping, (u, v): (f64, f64)) -> (f64, f64) {
    let (mut x, mut y) = if mapping.swap_axes { (v, u) } else { (u, v) };
    if mapping.mirror_x {
        x = 1.0 - x;
    }
    if mapping.mirror_y {
        y = 1.0 - y;
    }
    (x, y)
}

/// The key a calibration is remembered by.
///
/// Everything a calibration depends on: which reader it is, which controller
/// and what ranges it reports, and the pose the dots were tapped in. A change
/// to any of them means the old answer may no longer hold, and asking for two
/// more taps is cheap next to touch that lands in the wrong place.
#[must_use]
pub fn ledger_key(snapshot: &DeviceSnapshot) -> Option<String> {
    let touch = snapshot.touch.as_ref()?;
    let framebuffer = snapshot.framebuffer.as_ref()?;
    let identity = &snapshot.identity;
    let word = |text: &str| -> String {
        text.chars()
            .map(|c| if c.is_whitespace() { '_' } else { c })
            .collect()
    };
    Some(format!(
        "{}:{}:{}:{}..{}:{}..{}:r{}",
        word(identity.serial_prefix.as_deref().unwrap_or("unknown")),
        identity.device_code.unwrap_or_default(),
        word(&touch.name),
        touch.x_min,
        touch.x_max,
        touch.y_min,
        touch.y_max,
        framebuffer.rotation
    ))
}

/// The calibration remembered under `key`, if there is one.
///
/// The last line for a key wins, so a reader calibrated twice uses the newer
/// answer even if the older line was never removed.
#[must_use]
pub fn find_in_ledger(ledger: &str, key: &str) -> Option<TouchMapping> {
    ledger.lines().rev().find_map(|line| {
        let mut fields = line.split_whitespace();
        if fields.next()? != key {
            return None;
        }
        let mut flag = |name: &str| -> Option<bool> {
            match fields.next()?.strip_prefix(name)?.strip_prefix('=')? {
                "0" => Some(false),
                "1" => Some(true),
                _ => None,
            }
        };
        Some(TouchMapping {
            swap_axes: flag("swap")?,
            mirror_x: flag("mirror_x")?,
            mirror_y: flag("mirror_y")?,
        })
    })
}

/// The ledger with `mapping` recorded under `key`, replacing any older line.
#[must_use]
pub fn ledger_with(ledger: &str, key: &str, mapping: TouchMapping) -> String {
    let mut lines = ledger
        .lines()
        .filter(|line| line.split_whitespace().next() != Some(key))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    lines.push(format!(
        "{key} swap={} mirror_x={} mirror_y={}",
        u8::from(mapping.swap_axes),
        u8::from(mapping.mirror_x),
        u8::from(mapping.mirror_y)
    ));
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::{
        derive, find_in_ledger, ledger_key, ledger_with, targets, CalibrationError, ALL_MAPPINGS,
    };
    use crate::{
        provisional, Bitfield, DeviceProfile, DeviceSnapshot, FramebufferSnapshot,
        IdentitySnapshot, PanelPose, TouchMapping, TouchSnapshot, TouchTransform,
    };

    fn byte(offset: u32) -> Bitfield {
        Bitfield {
            offset,
            length: 8,
            msb_right: 0,
        }
    }

    /// An unmeasured i.MX6SLL reader whose controller counts further on one
    /// axis than the other, as the real ones do.
    fn snapshot() -> DeviceSnapshot {
        DeviceSnapshot {
            compatible: vec!["fsl,imx6sll".to_owned()],
            model: Some("Synthetic reader".to_owned()),
            framebuffer: Some(FramebufferSnapshot {
                id: "mxc_epdc_fb".to_owned(),
                width: 1072,
                height: 1448,
                virtual_width: 1088,
                virtual_height: 1536,
                x_offset: 0,
                y_offset: 0,
                bits_per_pixel: 32,
                grayscale: 0,
                stride: 4352,
                memory_length: 6_782_976,
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
                name: "Some Touch Panel".to_owned(),
                x_min: 0,
                x_max: 1447,
                y_min: 0,
                y_max: 1071,
            }),
            identity: IdentitySnapshot {
                serial_prefix: Some("N999".into()),
                firmware_version: Some("4.38.23697".into()),
                kernel_release: Some("4.1.15".into()),
                device_code: Some(999),
            },
        }
    }

    fn profile_with(mapping: TouchMapping) -> &'static DeviceProfile {
        provisional::profile_from_probe(&snapshot(), TouchTransform::Calibrated(mapping))
            .expect("derivable")
    }

    /// The raw positions a controller mounted `mapping`-wise reports for the
    /// two dots, worked out by the same code that maps taps at runtime.
    fn taps_for(mapping: TouchMapping) -> [(i32, i32); 2] {
        let profile = profile_with(mapping);
        let pose = PanelPose::reference(profile);
        targets(profile.width, profile.height).map(|(x, y)| {
            pose.display_to_touch(x, y)
                .expect("a dot inside the panel has a raw position")
        })
    }

    #[test]
    fn every_one_of_the_eight_mountings_is_recovered_from_two_taps() {
        let neutral = profile_with(ALL_MAPPINGS[0]);
        for mapping in ALL_MAPPINGS {
            assert_eq!(
                derive(neutral, 1072, 1448, taps_for(mapping)),
                Ok(mapping),
                "{mapping:?}"
            );
        }
    }

    #[test]
    fn the_eight_mountings_are_all_different() {
        for (index, mapping) in ALL_MAPPINGS.iter().enumerate() {
            assert!(!ALL_MAPPINGS[index + 1..].contains(mapping));
        }
    }

    #[test]
    fn a_finger_that_lands_a_little_off_the_dot_still_counts() {
        let neutral = profile_with(ALL_MAPPINGS[0]);
        for mapping in ALL_MAPPINGS {
            let [first, second] = taps_for(mapping);
            // About seven percent of each axis, in opposite directions.
            let off = [
                (first.0 + 100, first.1 - 70),
                (second.0 - 100, second.1 + 70),
            ]
            .map(|(x, y)| (x.clamp(0, 1447), y.clamp(0, 1071)));
            assert_eq!(derive(neutral, 1072, 1448, off), Ok(mapping), "{mapping:?}");
        }
    }

    #[test]
    fn two_taps_in_the_middle_explain_nothing_and_are_refused() {
        let neutral = profile_with(ALL_MAPPINGS[0]);
        assert_eq!(
            derive(neutral, 1072, 1448, [(724, 536), (724, 536)]),
            Err(CalibrationError::NoMatch)
        );
    }

    #[test]
    fn the_same_corner_tapped_twice_is_refused() {
        let neutral = profile_with(ALL_MAPPINGS[0]);
        let [first, _] = taps_for(ALL_MAPPINGS[5]);
        assert_eq!(
            derive(neutral, 1072, 1448, [first, first]),
            Err(CalibrationError::NoMatch)
        );
    }

    #[test]
    fn a_tap_outside_the_reported_range_is_refused() {
        let neutral = profile_with(ALL_MAPPINGS[0]);
        assert_eq!(
            derive(neutral, 1072, 1448, [(-5, 10), (1400, 10)]),
            Err(CalibrationError::OutOfRange)
        );
    }

    #[test]
    fn opposite_corners_could_not_have_told_a_swap_apart() {
        // The reason the dots are not at opposite corners, kept as a test so
        // nobody moves them there. In the unit square the raw axes live in,
        // swapping the axes leaves the main diagonal where it is.
        let swapped = TouchMapping {
            swap_axes: true,
            mirror_x: false,
            mirror_y: false,
        };
        let corner = (0.15, 0.15);
        let opposite = (0.85, 0.85);
        assert_eq!(super::apply(swapped, corner), corner);
        assert_eq!(super::apply(swapped, opposite), opposite);
        let [first, second] = targets(1072, 1448);
        assert_eq!(first.1, second.1, "the dots share an edge");
    }

    #[test]
    fn the_dots_sit_inside_the_panel_near_its_top_corners() {
        let [first, second] = targets(1072, 1448);
        assert!(first.0 > 100 && first.0 < 200, "{first:?}");
        assert!(first.1 > 150 && first.1 < 250, "{first:?}");
        assert_eq!(second.0, 1071 - first.0);
        assert_eq!(targets(0, 0), [(0, 0), (0, 0)]);
    }

    #[test]
    fn a_calibration_is_found_again_under_the_same_key_and_the_newest_wins() {
        let key = ledger_key(&snapshot()).expect("a key");
        assert!(!key.contains(' '), "{key}");
        assert!(key.ends_with(":r3"), "{key}");
        let first = ALL_MAPPINGS[6];
        let second = ALL_MAPPINGS[5];
        let ledger = ledger_with("", &key, first);
        assert_eq!(find_in_ledger(&ledger, &key), Some(first));
        let ledger = ledger_with(&ledger, &key, second);
        assert_eq!(find_in_ledger(&ledger, &key), Some(second));
        assert_eq!(ledger.lines().count(), 1, "the older line is replaced");
        let appended = format!("{ledger}{key} swap=0 mirror_x=0 mirror_y=1\n");
        assert_eq!(find_in_ledger(&appended, &key), Some(ALL_MAPPINGS[1]));
    }

    #[test]
    fn a_calibration_taken_in_another_pose_or_on_another_controller_is_not_reused() {
        let key = ledger_key(&snapshot()).expect("a key");
        let ledger = ledger_with("", &key, ALL_MAPPINGS[6]);
        let mut turned = snapshot();
        turned.framebuffer.as_mut().expect("framebuffer").rotation = 1;
        let turned_key = ledger_key(&turned).expect("a key");
        assert_eq!(find_in_ledger(&ledger, &turned_key), None);
        let mut other = snapshot();
        other.touch.as_mut().expect("touch").x_max = 4095;
        let other_key = ledger_key(&other).expect("a key");
        assert_eq!(find_in_ledger(&ledger, &other_key), None);
    }

    #[test]
    fn a_damaged_ledger_line_is_ignored_rather_than_guessed_at() {
        let key = ledger_key(&snapshot()).expect("a key");
        for line in [
            format!("{key} swap=1 mirror_x=0"),
            format!("{key} swap=yes mirror_x=0 mirror_y=0"),
            format!("{key} mirror_x=0 swap=1 mirror_y=0"),
        ] {
            assert_eq!(find_in_ledger(&line, &key), None, "{line}");
        }
    }
}

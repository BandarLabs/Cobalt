//! Strict, fail-closed Kobo device profiles.

use std::fmt;

/// Which panel-controller interface the device's framebuffer speaks.
///
/// Declared on the profile rather than inferred from `framebuffer_id`, so
/// that adding a device is an explicit statement of which update ABI its
/// kernel implements.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FramebufferController {
    /// The `MediaTek` HWTCON interface (36-byte update struct).
    Hwtcon,
    /// The i.MX6 EPDC v2 interface (72-byte update struct).
    MxcfbV2,
}

/// How a device's touch controller reports position relative to the display.
///
/// This used to be inferred from the framebuffer's `rotation`. It cannot be:
/// the framebuffer says how the panel is scanned, and says nothing about which
/// way round the digitiser underneath it was soldered. Two devices reporting
/// `rotation: 1` were found to need different transforms, so the mapping is
/// declared per profile and has to be measured with `kobo touch-probe` rather
/// than derived.
///
/// # Why this is only safe at rotations in the verified set
///
/// A value here is a composition of two separate facts, and only one of them
/// belongs in a profile. How the digitiser sits under the panel is fixed in
/// the hardware and never changes. How display coordinates map onto physical
/// space is decided by the framebuffer's `rotation`, which is live state: a
/// Libra 2 held upside down reports `rotation: 3` where the same device
/// upright reports 1, with every geometry field unchanged, and it flips as the
/// reader is handled.
///
/// So a variant here is correct as written only at the rotation it was
/// measured at, named by `reference_rotation`. At any other rotation in the
/// profile's `verified_rotations` it is composed with the half-turn delta by
/// [`PanelPose`], which is what keeps this field honest now that `validate`
/// accepts more than one pose. Both halves were confirmed on the Libra 2
/// panel on 2026-08-22: the digitiser measured panel-fixed by corner at both
/// poses, and rendered marks at rotation 3 appearing diametrically opposite
/// their rotation 1 positions.
///
/// Anyone widening a profile's `verified_rotations` has to measure the pose
/// first, on the panel and on the digitiser both. Touch would not fail
/// loudly. It would land in the wrong place. As evidence that the
/// composition really is what is encoded here: the 180-degree case would need
/// a transpose with both axes mirrored, and there is deliberately no such
/// variant, because a profile has no business describing which way up someone
/// is holding the reader.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TouchTransform {
    /// Controller axes already agree with the display. No device here uses
    /// this, and it is the historical fallback for any rotation other than 1
    /// or 3.
    Direct,
    /// Axes exchanged, neither mirrored.
    ///
    /// Measured on the Kobo Libra 2 with three taps in an L, one leg per
    /// physical axis: moving down the left edge drives `raw_x` upward, moving
    /// right along the bottom drives `raw_y` upward, and neither runs
    /// backwards. Confirmed afterwards against the fixed transform, where a
    /// tap in each of three corners reported that corner.
    ///
    /// The reader was held with its page-turn buttons on the right throughout,
    /// which is the pose this device reports as `rotation: 1`. Buttons on the
    /// left is `rotation: 3`, a half turn away, which the profile accepts by
    /// composing this mapping with the delta — see [`PanelPose`]. Anyone
    /// repeating the measurement has to say which side the buttons are on:
    /// "portrait" alone names two orientations 180 degrees apart, and the
    /// digitiser tells them apart even when the screen does not.
    Transpose,
    /// Axes exchanged and Y mirrored.
    ///
    /// This is what every `rotation: 1` device did before the transform became
    /// explicit. It is preserved for the Elipsa 2E, where it is backed by a
    /// touch captured on the real device: see
    /// `elipsa_touch_transform_matches_a_physically_measured_touch`. The Libra
    /// 2 is also `rotation: 1`, also an Elan controller, and measured as a
    /// plain `Transpose`, so the two `rotation: 1` devices genuinely differ
    /// and neither can be inferred from the other.
    TransposeMirrorY,
    /// Axes exchanged and X mirrored.
    ///
    /// Confirmed on the Clara BW and the Clara HD, each by a physically
    /// captured touch.
    TransposeMirrorX,
}

impl TouchTransform {
    /// Lowers a declared transform into the composable form.
    ///
    /// Lossless in both directions for all four variants. The composed value
    /// this feeds is a superset: a transpose with both axes mirrored is
    /// reachable here and deliberately has no variant above, because it is a
    /// runtime fact about which way up the reader is being held rather than a
    /// hardware fact about the digitiser.
    #[must_use]
    pub const fn lower(self) -> TouchMapping {
        match self {
            Self::Direct => TouchMapping {
                swap_axes: false,
                mirror_x: false,
                mirror_y: false,
            },
            Self::Transpose => TouchMapping {
                swap_axes: true,
                mirror_x: false,
                mirror_y: false,
            },
            Self::TransposeMirrorY => TouchMapping {
                swap_axes: true,
                mirror_x: false,
                mirror_y: true,
            },
            Self::TransposeMirrorX => TouchMapping {
                swap_axes: true,
                mirror_x: true,
                mirror_y: false,
            },
        }
    }
}

/// How a profile's pose geometry follows from its panel dimensions.
///
/// The pose fields a framebuffer reports — virtual width, virtual height and
/// stride — are not independent hardware facts. They are computed by the
/// display driver from the visible geometry, and a profile that stores them as
/// constants is storing a photograph of one pose. This names the computation
/// instead, per profile, so that validation can derive the expectation for any
/// rotation in the verified set rather than matching one snapshot.
///
/// For every rotation a profile currently verifies the derived values are
/// numerically identical to the constants the profile also carries, and the
/// tests pin that. What changes is the kind of check: consistency with the
/// driver's rule rather than identity with one photograph. A firmware that
/// changed the driver's alignment would pass here where a constant would fail
/// loudly; the conformance harness in `tools/abi/check-mxcfb.sh` is where that
/// belongs instead.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GeometryRule {
    /// Pose fields are exact-matched constants.
    ///
    /// The `MediaTek` devices keep this, for the same reason the touch transform
    /// is per-profile: do not silently change a device nobody here can test.
    Fixed,
    /// The i.MX6 EPDC v2 derivation, read out of the vendor driver source
    /// (`mxc_epdc_v2_fb.c`) rather than fitted to observations:
    ///
    /// - `xres_virtual = ALIGN(xres, 32)` — the alignment is in pixels, not
    ///   bytes, per `:3567`; the driver's own comment says bytes and is wrong.
    /// - `yres_virtual = ALIGN(yres, 128) * num_screens / page_scale` per
    ///   `:3568`, where `page_scale` is `bits_per_pixel / 16` with 16 the
    ///   driver's `default_bpp`. The two factors cancel at 32 bpp and nowhere
    ///   else.
    /// - `line_length = xres_virtual * bits_per_pixel / 8` per `:3215`.
    ///
    /// `memory_length` is deliberately not derived. The driver fixes it at
    /// probe time from the panel's native mode, taking the larger product
    /// across orientations, and never recomputes it, so it stays an
    /// exact-matched constant on the profile.
    MxcEpdcV2 {
        /// The driver's `num_screens`, computed on the device from a
        /// device-tree memory size. A named assumption rather than a constant
        /// of the rule: two hardware revisions ship under the Libra 2's device
        /// code, and a revision with a different value would move the virtual
        /// height. Observation matches 2.
        num_screens: u32,
    },
}

/// `ALIGN(value, to)` as the kernel macro does it: round up to a multiple.
const fn align_up(value: u32, to: u32) -> u32 {
    value.div_ceil(to) * to
}

/// A touch mapping that can be composed with a live rotation.
///
/// # The frame these mirrors are written in
///
/// `mirror_x` and `mirror_y` are applied in **display pixel space**, after the
/// axis swap and after scaling. This has to be stated, because the raw-space
/// and display-space spellings of a single mirror are crossed: mirroring
/// display x is the same operation as mirroring raw y once the axes are
/// swapped. A reader who assumes the raw frame will map
/// [`TouchTransform::TransposeMirrorX`] onto `TransposeMirrorY`'s behaviour and
/// vice versa, which is a silent inversion on a device nobody here can test.
/// The Clara BW's measured-touch test is what would catch it.
///
/// Mirroring after scaling also matters: `width - 1 - scaled` and
/// `scale(maximum - raw)` round differently at the extremes, and the exact edge
/// assertions are what fail if the order is swapped.
///
/// Unlike [`TouchTransform`], this can express a transpose with both axes
/// mirrored, which is exactly what a 180 degree pose change produces and the
/// only reason this type exists.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TouchMapping {
    /// Controller axes are exchanged relative to the display.
    pub swap_axes: bool,
    /// Display x runs backwards.
    pub mirror_x: bool,
    /// Display y runs backwards.
    pub mirror_y: bool,
}

impl TouchMapping {
    /// The same mapping with the image turned through 180 degrees.
    ///
    /// The digitiser is glued to the glass and does not move when the reader is
    /// turned over. The image does: the driver hands `rotate` to the PXP, which
    /// rotates pixels on the way to the panel, and the `FB_ROTATE_UD` arm of
    /// `adjust_coordinates` maps a rectangle to `xres - (left + width)` and
    /// `yres - (top + height)`. So the same finger position lands at the
    /// diametrically opposite display point, which is both mirrors flipped with
    /// the swap unchanged.
    ///
    /// The 180 degree rotation is in the centre of the dihedral group, so it
    /// commutes with the swap and with either mirror. That is why this is
    /// correct regardless of which frame the mirrors are read in, and it is the
    /// one part of this composition that does not depend on the convention
    /// question below.
    #[must_use]
    pub const fn rotated_180(self) -> Self {
        Self {
            swap_axes: self.swap_axes,
            mirror_x: !self.mirror_x,
            mirror_y: !self.mirror_y,
        }
    }
}

pub const CLARA_BW_391: DeviceProfile = DeviceProfile {
    id: "clara-bw-391",
    model: "Kobo Clara BW",
    device_code: 391,
    device_tree_model: "MediaTek MT8110 board",
    compatible_fragments: &["mediatek,mt8110", "mediatek,mt8512"],
    framebuffer_id: "hwtcon",
    framebuffer_controller: FramebufferController::Hwtcon,
    width: 1072,
    height: 1448,
    pixels_per_inch: 300,
    virtual_width: 1072,
    virtual_height: 1448,
    x_offset: 0,
    y_offset: 0,
    bits_per_pixel: 32,
    grayscale: 0,
    stride: 4288,
    memory_length: 6_243_328,
    framebuffer_kind: 0,
    framebuffer_visual: 2,
    rotation: 3,
    red: Bitfield {
        offset: 0,
        length: 8,
        msb_right: 0,
    },
    green: Bitfield {
        offset: 8,
        length: 8,
        msb_right: 0,
    },
    blue: Bitfield {
        offset: 16,
        length: 8,
        msb_right: 0,
    },
    alpha: Bitfield {
        offset: 24,
        length: 8,
        msb_right: 0,
    },
    touch_transform: TouchTransform::TransposeMirrorX,
    reference_rotation: 3,
    verified_rotations: &[3],
    geometry_rule: GeometryRule::Fixed,
    touch_name: "cyttsp5_mt",
    touch_x_min: 0,
    touch_x_max: 1447,
    touch_y_min: 0,
    touch_y_max: 1071,
    serial_prefix: "N365",
    firmware_versions: &["4.45.23697"],
    kernel_release: "4.9.77",
    write_ready: true,
};

pub const CLARA_BW_395: DeviceProfile = DeviceProfile {
    id: "clara-bw-395",
    model: "Kobo Clara BW",
    device_code: 395,
    device_tree_model: "MediaTek MT8110 TPV board",
    compatible_fragments: &["mediatek,mt8110", "mediatek,mt8512"],
    framebuffer_id: "hwtcon",
    framebuffer_controller: FramebufferController::Hwtcon,
    width: 1072,
    height: 1448,
    pixels_per_inch: 300,
    virtual_width: 1072,
    virtual_height: 1448,
    x_offset: 0,
    y_offset: 0,
    bits_per_pixel: 32,
    grayscale: 0,
    stride: 4288,
    memory_length: 6_243_328,
    framebuffer_kind: 0,
    framebuffer_visual: 2,
    rotation: 3,
    red: Bitfield {
        offset: 0,
        length: 8,
        msb_right: 0,
    },
    green: Bitfield {
        offset: 8,
        length: 8,
        msb_right: 0,
    },
    blue: Bitfield {
        offset: 16,
        length: 8,
        msb_right: 0,
    },
    alpha: Bitfield {
        offset: 24,
        length: 8,
        msb_right: 0,
    },
    touch_transform: TouchTransform::TransposeMirrorX,
    reference_rotation: 3,
    verified_rotations: &[3],
    geometry_rule: GeometryRule::Fixed,
    touch_name: "cyttsp5_mt",
    touch_x_min: 0,
    touch_x_max: 1447,
    touch_y_min: 0,
    touch_y_max: 1071,
    serial_prefix: "P365",
    firmware_versions: &["4.45.23697"],
    kernel_release: "4.9.77",
    write_ready: true,
};

/// The Kobo Clara HD, added upstream without i.MX6 hardware to test on.
///
/// Its constants satisfy the [`GeometryRule::MxcEpdcV2`] derivation exactly
/// (`ALIGN(1072, 32) = 1088`, `ALIGN(1448, 128) * 2 / 2 = 1536`, stride
/// `1088 * 4 = 4352`), but the rule stays [`GeometryRule::Fixed`] and the
/// verified set stays one pose: do not silently widen a device nobody here
/// can test. Its rotation-3 touch mapping is the same measured transform as
/// the Clara BW's, on the same `cyttsp5_mt` controller with the same panel
/// dimensions.
pub const CLARA_HD_376: DeviceProfile = DeviceProfile {
    id: "clara-hd-376",
    model: "Kobo Clara HD",
    device_code: 376,
    device_tree_model: "Freescale i.MX6SLL NTX Board",
    compatible_fragments: &["fsl,imx6sll-lpddr3-arm2", "fsl,imx6sll"],
    framebuffer_id: "mxc_epdc_fb",
    framebuffer_controller: FramebufferController::MxcfbV2,
    width: 1072,
    height: 1448,
    pixels_per_inch: 300,
    virtual_width: 1088,
    virtual_height: 1536,
    x_offset: 0,
    y_offset: 0,
    bits_per_pixel: 32,
    grayscale: 0,
    stride: 4352,
    memory_length: 6_782_976,
    framebuffer_kind: 0,
    framebuffer_visual: 2,
    rotation: 3,
    red: Bitfield {
        offset: 16,
        length: 8,
        msb_right: 0,
    },
    green: Bitfield {
        offset: 8,
        length: 8,
        msb_right: 0,
    },
    blue: Bitfield {
        offset: 0,
        length: 8,
        msb_right: 0,
    },
    alpha: Bitfield {
        offset: 24,
        length: 8,
        msb_right: 0,
    },
    touch_transform: TouchTransform::TransposeMirrorX,
    reference_rotation: 3,
    verified_rotations: &[3],
    geometry_rule: GeometryRule::Fixed,
    touch_name: "cyttsp5_mt",
    touch_x_min: 0,
    touch_x_max: 1447,
    touch_y_min: 0,
    touch_y_max: 1071,
    serial_prefix: "N249",
    firmware_versions: &["4.38.23684", "4.38.23697"],
    kernel_release: "4.1.15-00136-g12655eaaef89",
    write_ready: true,
};

pub const ELIPSA_2E_389: DeviceProfile = DeviceProfile {
    id: "elipsa-2e-389",
    model: "Kobo Elipsa 2E",
    device_code: 389,
    device_tree_model: "MediaTek MT8110 board",
    compatible_fragments: &["mediatek,mt8110", "mediatek,mt8512"],
    framebuffer_id: "hwtcon",
    framebuffer_controller: FramebufferController::Hwtcon,
    width: 1404,
    height: 1872,
    pixels_per_inch: 227,
    virtual_width: 1404,
    virtual_height: 1872,
    x_offset: 0,
    y_offset: 0,
    bits_per_pixel: 32,
    grayscale: 0,
    stride: 5616,
    memory_length: 10_543_104,
    framebuffer_kind: 0,
    framebuffer_visual: 2,
    rotation: 1,
    red: Bitfield {
        offset: 0,
        length: 0,
        msb_right: 0,
    },
    green: Bitfield {
        offset: 0,
        length: 0,
        msb_right: 0,
    },
    blue: Bitfield {
        offset: 0,
        length: 0,
        msb_right: 0,
    },
    alpha: Bitfield {
        offset: 0,
        length: 0,
        msb_right: 0,
    },
    touch_transform: TouchTransform::TransposeMirrorY,
    reference_rotation: 1,
    verified_rotations: &[1],
    geometry_rule: GeometryRule::Fixed,
    touch_name: "Elan Touchscreen",
    touch_x_min: 0,
    touch_x_max: 1872,
    touch_y_min: 0,
    touch_y_max: 1404,
    serial_prefix: "N605",
    firmware_versions: &["4.38.23697"],
    kernel_release: "4.9.77",
    write_ready: true,
};

/// Kobo Libra 2, codename `io`, an i.MX6SLL Mark 7 device driven by
/// `mxc_epdc_fb` rather than by `hwtcon`.
///
/// Measured with `kobo doctor` against a device on firmware 4.38.23697, from a
/// cold boot into Nickel with nothing else launched.
///
/// The geometry below is the portrait state. Six of these fields move when the
/// reader is rotated: `width` and `height` exchange, `virtual_width` becomes
/// 1696 and `virtual_height` 1280, `stride` becomes 6784, and `rotation`
/// becomes 2. One rule covers both: the stride is the visible row rounded up
/// to 128 bytes, and the virtual width is that stride divided by four. So 1264
/// pixels is 5056 bytes, rounded to 5120, giving 1280; and 1680 pixels is 6720
/// bytes, rounded to 6784, giving 1696. The virtual height is the visible
/// height rounded up to 128 pixels. `memory_length` does not move, being
/// allocated once at the larger of the two products, and neither do the
/// bitfields or the touch ranges, since the controller reports in panel
/// coordinates whatever the screen is doing.
///
/// So this profile matches the device in portrait and rejects it in landscape.
/// Nothing about that is specific to the Libra 2, since every Kobo rotates and
/// nothing here sets the mode it wants: `FBIOGET_VSCREENINFO` is read and
/// `FBIOPUT_VSCREENINFO` is never sent, so whatever the previous application
/// left is what gets validated. Relaxing the comparison would not be a fix on
/// its own, because `rotation` also selects the touch transform in
/// `touch_to_display`, so a device accepted at the wrong rotation would have
/// its touch input placed wrongly.
pub const LIBRA_2_388: DeviceProfile = DeviceProfile {
    id: "libra-2-388",
    model: "Kobo Libra 2",
    device_code: 388,
    device_tree_model: "Freescale i.MX6SLL NTX Board",
    compatible_fragments: &["fsl,imx6sll"],
    framebuffer_id: "mxc_epdc_fb",
    framebuffer_controller: FramebufferController::MxcfbV2,
    width: 1264,
    height: 1680,
    pixels_per_inch: 300,
    virtual_width: 1280,
    virtual_height: 1792,
    x_offset: 0,
    y_offset: 0,
    bits_per_pixel: 32,
    grayscale: 0,
    stride: 5120,
    memory_length: 9_175_040,
    framebuffer_kind: 0,
    framebuffer_visual: 2,
    rotation: 1,
    red: Bitfield {
        offset: 16,
        length: 8,
        msb_right: 0,
    },
    green: Bitfield {
        offset: 8,
        length: 8,
        msb_right: 0,
    },
    blue: Bitfield {
        offset: 0,
        length: 8,
        msb_right: 0,
    },
    alpha: Bitfield {
        offset: 24,
        length: 8,
        msb_right: 0,
    },
    touch_transform: TouchTransform::Transpose,
    reference_rotation: 1,
    // Rotation 3 was verified on the device on 2026-08-22: the digitiser
    // measured panel-fixed by corner at both poses, and rendered marks at
    // rotation 3 appeared diametrically opposite their rotation 1 positions,
    // both exactly as the half-turn composition predicts.
    verified_rotations: &[1, 3],
    geometry_rule: GeometryRule::MxcEpdcV2 { num_screens: 2 },
    touch_name: "Elan Touchscreen",
    touch_x_min: 0,
    touch_x_max: 1680,
    touch_y_min: 0,
    touch_y_max: 1264,
    serial_prefix: "N418",
    firmware_versions: &["4.38.23697"],
    kernel_release: "4.1.15-00868-g58a2758be07",
    // Owner-attended display, touch, exit and recovery evidence was filmed on
    // the device and reviewed upstream before this was set.
    write_ready: true,
};

pub const SUPPORTED_PROFILES: &[&DeviceProfile] =
    &[&CLARA_BW_391, &CLARA_BW_395, &CLARA_HD_376, &ELIPSA_2E_389, &LIBRA_2_388];

pub const WRITE_EVIDENCE_PENDING: &str =
    "owner-attended display, touch, exit, and recovery evidence is incomplete";

/// Returns the exact supported profile authorized for ordinary device writes.
///
/// A hardware match alone deliberately is not enough. The profile must have
/// completed its attended evidence and every identity field must match the
/// reviewed device and firmware exactly.
///
/// # Errors
///
/// Returns every write blocker when no supported profile matches, the profile
/// is still awaiting attended evidence, or exact device identity is missing or
/// different.
pub fn write_ready_profile(
    snapshot: &DeviceSnapshot,
) -> Result<&'static DeviceProfile, Vec<String>> {
    let profile = identify_profile(snapshot)
        .ok_or_else(|| vec!["no supported hardware profile matched this device".to_owned()])?;
    let report = profile.validate(snapshot);
    if report.readiness == Readiness::WriteReady {
        Ok(profile)
    } else {
        Err(report.write_blockers)
    }
}

#[must_use]
pub fn identify_profile(snapshot: &DeviceSnapshot) -> Option<&'static DeviceProfile> {
    SUPPORTED_PROFILES
        .iter()
        .copied()
        .find(|profile| profile.validate(snapshot).readiness != Readiness::Rejected)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Bitfield {
    pub offset: u32,
    pub length: u32,
    pub msb_right: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FramebufferSnapshot {
    pub id: String,
    pub width: u32,
    pub height: u32,
    pub virtual_width: u32,
    pub virtual_height: u32,
    pub x_offset: u32,
    pub y_offset: u32,
    pub bits_per_pixel: u32,
    pub grayscale: u32,
    pub stride: u32,
    pub memory_length: u32,
    pub kind: u32,
    pub visual: u32,
    pub rotation: u32,
    pub red: Bitfield,
    pub green: Bitfield,
    pub blue: Bitfield,
    pub alpha: Bitfield,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TouchSnapshot {
    pub path: String,
    pub name: String,
    pub x_min: i32,
    pub x_max: i32,
    pub y_min: i32,
    pub y_max: i32,
}

/// Non-identifying device identity fields.
///
/// The device serial number is deliberately never captured. Only its model
/// prefix is retained, because the full serial is personal hardware data that
/// nothing in this project needs.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IdentitySnapshot {
    pub serial_prefix: Option<String>,
    pub firmware_version: Option<String>,
    pub kernel_release: Option<String>,
    pub device_code: Option<u16>,
}

impl IdentitySnapshot {
    /// Parses `/mnt/onboard/.kobo/version` and `/proc/sys/kernel/osrelease`.
    ///
    /// The version file is a comma separated list whose first field is the
    /// serial number, third field is the firmware version, and last field is a
    /// UUID whose trailing digits are the device code.
    #[must_use]
    pub fn parse(version_file: Option<&str>, kernel_release: Option<&str>) -> Self {
        let fields: Vec<&str> = version_file
            .map(|line| line.trim().split(',').collect())
            .unwrap_or_default();
        Self {
            serial_prefix: fields
                .first()
                .map(|serial| serial.chars().take(4).collect::<String>())
                .filter(|prefix| prefix.len() == 4),
            firmware_version: fields
                .get(2)
                .map(|value| (*value).trim().to_owned())
                .filter(|value| !value.is_empty()),
            kernel_release: kernel_release
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty()),
            device_code: fields.last().and_then(|uuid| {
                let digits: String = uuid
                    .rsplit('-')
                    .next()?
                    .trim_start_matches('0')
                    .chars()
                    .collect();
                digits.parse().ok()
            }),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DeviceSnapshot {
    pub compatible: Vec<String>,
    pub model: Option<String>,
    pub framebuffer: Option<FramebufferSnapshot>,
    pub touch: Option<TouchSnapshot>,
    pub identity: IdentitySnapshot,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeviceProfile {
    pub id: &'static str,
    pub model: &'static str,
    pub device_code: u16,
    pub device_tree_model: &'static str,
    pub compatible_fragments: &'static [&'static str],
    pub framebuffer_id: &'static str,
    /// Which update ABI this device's framebuffer driver implements.
    pub framebuffer_controller: FramebufferController,
    pub width: u32,
    pub height: u32,
    pub pixels_per_inch: u16,
    pub virtual_width: u32,
    pub virtual_height: u32,
    pub x_offset: u32,
    pub y_offset: u32,
    pub bits_per_pixel: u32,
    pub grayscale: u32,
    pub stride: u32,
    pub memory_length: u32,
    pub framebuffer_kind: u32,
    pub framebuffer_visual: u32,
    pub rotation: u32,
    pub red: Bitfield,
    pub green: Bitfield,
    pub blue: Bitfield,
    pub alpha: Bitfield,
    /// How the touch controller's axes relate to the visible display.
    ///
    /// Kept separate from `rotation` because the two answer different
    /// questions. `rotation` is read back from the framebuffer and is compared
    /// during validation, so it has to stay whatever the kernel reports. How
    /// the digitiser is mounted underneath the panel is a fact about the
    /// hardware that no framebuffer field records, and it has to be measured.
    pub touch_transform: TouchTransform,
    /// The orientation `touch_transform` is written in.
    ///
    /// `Transpose` on its own means nothing without "relative to what". The
    /// anchor is physical rather than numeric: the Libra 2's mapping was
    /// measured with the page-turn buttons on the right and the USB-C port on
    /// the top edge, which this device reports as `rotate: 1`.
    ///
    /// This constrains nothing at validation time. It names the frame the
    /// digitiser fact is expressed in, so that a live rotation can be composed
    /// against it. Recorded explicitly because a later reader will otherwise
    /// delete it as redundant with `rotation`, which it currently equals on
    /// every profile and which answers a different question: `rotation` is a
    /// pose snapshot that moves as the reader is handled, and this does not.
    pub reference_rotation: u32,
    /// The rotations this profile has been verified at, on the digitiser and
    /// on the panel both.
    ///
    /// Validation accepts a device only at a rotation in this set, and
    /// [`PanelPose::resolve`] refuses any other. Every entry must differ from
    /// `reference_rotation` by zero or a half turn, because that is all
    /// [`PanelPose`] can compose; a quarter turn entry would be refused at
    /// resolve time anyway, loudly. Widening this set is a measurement, not an
    /// edit: both the digitiser mapping and the image placement have to be
    /// confirmed on the physical device at the new pose first.
    pub verified_rotations: &'static [u32],
    /// How the pose geometry fields follow from the panel dimensions.
    pub geometry_rule: GeometryRule,
    pub touch_name: &'static str,
    pub touch_x_min: i32,
    pub touch_x_max: i32,
    pub touch_y_min: i32,
    pub touch_y_max: i32,
    pub serial_prefix: &'static str,
    /// Exact firmware releases covered by owner-attended evidence.
    pub firmware_versions: &'static [&'static str],
    pub kernel_release: &'static str,
    /// True only after owner-attended hardware evidence has been reviewed.
    pub write_ready: bool,
}

/// Pose geometry derived by a profile's [`GeometryRule`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExpectedGeometry {
    pub virtual_width: u32,
    pub virtual_height: u32,
    pub stride: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Readiness {
    Rejected,
    ReadOnlyMatched,
    WriteReady,
}

impl fmt::Display for Readiness {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rejected => formatter.write_str("rejected"),
            Self::ReadOnlyMatched => formatter.write_str("read-only matched"),
            Self::WriteReady => formatter.write_str("write ready"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidationReport {
    pub readiness: Readiness,
    pub mismatches: Vec<String>,
    pub write_blockers: Vec<String>,
}

impl DeviceProfile {
    #[must_use]
    pub fn validate(&self, snapshot: &DeviceSnapshot) -> ValidationReport {
        let mut mismatches = Vec::new();
        let mut blockers = Vec::new();

        for fragment in self.compatible_fragments {
            if !snapshot
                .compatible
                .iter()
                .any(|value| value.contains(fragment))
            {
                mismatches.push(format!("device tree does not contain {fragment}"));
            }
        }
        if snapshot.model.as_deref() != Some(self.device_tree_model) {
            mismatches.push(format!(
                "device tree model: expected {}, found {}",
                self.device_tree_model,
                snapshot.model.as_deref().unwrap_or("<missing>")
            ));
        }

        match &snapshot.framebuffer {
            Some(framebuffer) => {
                validate_framebuffer(self, framebuffer, &mut mismatches, &mut blockers);
            }
            None => mismatches.push("framebuffer probe unavailable".to_owned()),
        }

        match &snapshot.touch {
            Some(touch) => validate_touch(self, touch, &mut mismatches),
            None => mismatches.push("touch probe unavailable".to_owned()),
        }

        // The write gate and the verdict used to be strangers: an
        // unconditional "intentionally incomplete" blocker made WriteReady
        // unreachable for every profile, while the write path consulted
        // write_identity_blockers() separately and wrote anyway. The verdict
        // now reports the condition the write path actually enforces.
        blockers.extend(self.write_identity_blockers(snapshot));
        if !self.write_ready {
            blockers.push(WRITE_EVIDENCE_PENDING.to_owned());
        }

        let readiness = if !mismatches.is_empty() {
            Readiness::Rejected
        } else if blockers.is_empty() {
            Readiness::WriteReady
        } else {
            Readiness::ReadOnlyMatched
        };

        ValidationReport {
            readiness,
            mismatches,
            write_blockers: blockers,
        }
    }

    /// The pose geometry this profile expects a framebuffer to report.
    ///
    /// Derived from the panel dimensions by the profile's [`GeometryRule`],
    /// or the stored constants under [`GeometryRule::Fixed`]. Every rotation
    /// in the verified set is a half turn from the reference, which preserves
    /// width and height, so the visible dimensions are the profile's own at
    /// every rotation validation can accept.
    #[must_use]
    pub const fn expected_geometry(&self) -> ExpectedGeometry {
        match self.geometry_rule {
            GeometryRule::Fixed => ExpectedGeometry {
                virtual_width: self.virtual_width,
                virtual_height: self.virtual_height,
                stride: self.stride,
            },
            GeometryRule::MxcEpdcV2 { num_screens } => {
                let virtual_width = align_up(self.width, 32);
                let page_scale = self.bits_per_pixel / 16;
                ExpectedGeometry {
                    virtual_width,
                    virtual_height: align_up(self.height, 128) * num_screens / page_scale,
                    stride: virtual_width * self.bits_per_pixel / 8,
                }
            }
        }
    }

    /// Returns the reasons this device may not be written to.
    ///
    /// Hardware geometry alone is not proof of identity, because another device
    /// could report a compatible framebuffer. Any write path additionally
    /// requires the exact device code, firmware version, kernel release, and
    /// serial model prefix this profile was measured against. An empty result
    /// means every identity field matched exactly.
    #[must_use]
    pub fn write_identity_blockers(&self, snapshot: &DeviceSnapshot) -> Vec<String> {
        let mut blockers = Vec::new();
        let identity = &snapshot.identity;

        match identity.device_code {
            Some(code) if code == self.device_code => {}
            Some(code) => blockers.push(format!(
                "device code: expected {}, found {code}",
                self.device_code
            )),
            None => blockers.push("device code could not be read".to_owned()),
        }
        compare_identity(
            &mut blockers,
            "serial model prefix",
            self.serial_prefix,
            identity.serial_prefix.as_deref(),
        );
        compare_identity_one_of(
            &mut blockers,
            "firmware version",
            self.firmware_versions,
            identity.firmware_version.as_deref(),
        );
        compare_identity(
            &mut blockers,
            "kernel release",
            self.kernel_release,
            identity.kernel_release.as_deref(),
        );
        blockers
    }
}

/// Why a profile could not be resolved against the orientation a device is in.
///
/// A refusal here has to be as loud as a rejected profile. It cannot be
/// swallowed by a caller and it must never reach the touch decoder, where an
/// unresolvable pose would look like taps quietly going nowhere rather than
/// like a device Cobalt declines to drive.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PoseError {
    /// The device is in an orientation this profile has not been measured in.
    UnverifiedRotation {
        observed: u32,
        verified: &'static [u32],
    },
    /// The visible geometry disagrees with the profile at an orientation the
    /// profile does claim to describe.
    GeometryMismatch {
        field: &'static str,
        observed: u32,
        expected: u32,
    },
    /// The probe returned no framebuffer at all.
    FramebufferMissing,
    /// The orientation differs from the profile's reference frame by a quarter
    /// turn, which nobody has measured on this device.
    UnsupportedRotationDelta { observed: u32, reference: u32 },
}

impl fmt::Display for PoseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnverifiedRotation { observed, verified } => write!(
                formatter,
                "device is at rotation {observed}, and this profile is only verified at {verified:?}"
            ),
            Self::GeometryMismatch {
                field,
                observed,
                expected,
            } => write!(
                formatter,
                "{field}: expected {expected}, found {observed}"
            ),
            Self::FramebufferMissing => formatter.write_str("no framebuffer to resolve against"),
            Self::UnsupportedRotationDelta {
                observed,
                reference,
            } => write!(
                formatter,
                "rotation {observed} is a quarter turn from the reference rotation {reference}, \
                 and no quarter turn has been measured on this device"
            ),
        }
    }
}

/// A profile resolved against the orientation the device is actually in.
///
/// A [`DeviceProfile`] describes hardware. Which way up the reader is being
/// held is not hardware: the framebuffer's `rotation` flips between 1 and 3 as
/// a Libra 2 is handled, with every geometry field unchanged. A pose is the
/// pairing of the two, resolved once, at the point where a live framebuffer is
/// available.
///
/// # What this does
///
/// `resolve` accepts any rotation in the profile's verified set and composes
/// the digitiser mapping with the half-turn delta from the reference frame.
/// Composition landed before validation relaxed, in that order deliberately:
/// composing early buys nothing but is safe, where relaxing early would have
/// put taps in the wrong place without failing. Both poses were confirmed on
/// the Libra 2 panel before validation was widened.
///
/// This type is also the shape
/// live re-resolution would need, if the reader is ever to be turned over
/// mid-session, which is out of scope here.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PanelPose<'a> {
    profile: &'a DeviceProfile,
    rotation: u32,
    width: u32,
    height: u32,
    mapping: TouchMapping,
}

impl<'a> PanelPose<'a> {
    /// The pose the profile was measured in.
    ///
    /// For callers with no device in front of them. It asserts an orientation
    /// rather than observing one, so it is right for the simulator and for
    /// tests, and wrong for anything holding a framebuffer.
    #[must_use]
    pub const fn reference(profile: &'a DeviceProfile) -> Self {
        Self {
            profile,
            rotation: profile.rotation,
            width: profile.width,
            height: profile.height,
            mapping: profile.touch_transform.lower(),
        }
    }

    /// Composes the digitiser mapping against an orientation.
    ///
    /// The digitiser is fixed to the glass, so the profile's mapping is a
    /// constant expressed in the frame named by `reference_rotation`. What
    /// moves is the image. Composition is therefore by the difference between
    /// the live orientation and that frame.
    ///
    /// Only a difference of zero or a half turn is supported. A quarter turn is
    /// refused, and that refusal is load-bearing rather than cautious. The
    /// driver swaps the clockwise and anticlockwise numbering under a hardware
    /// configuration flag, `bDisplayBusWidth == 3`, which this board has not
    /// been read for. That swap is an involution on the two portrait values and
    /// fixes the other two, so it preserves a half turn exactly and inverts a
    /// quarter turn. The differences accepted here are precisely the ones the
    /// ambiguity cannot corrupt, and the ones refused are precisely the ones
    /// where it decides the answer.
    ///
    /// Anyone adding quarter turn support has to read `bDisplayBusWidth` off
    /// the device first. Without it this stops being fail-closed and becomes a
    /// coin flip.
    fn compose(profile: &DeviceProfile, rotation: u32) -> Result<TouchMapping, PoseError> {
        let mapping = profile.touch_transform.lower();
        match (rotation % 4 + 4 - profile.reference_rotation % 4) % 4 {
            0 => Ok(mapping),
            2 => Ok(mapping.rotated_180()),
            _ => Err(PoseError::UnsupportedRotationDelta {
                observed: rotation,
                reference: profile.reference_rotation,
            }),
        }
    }

    /// A pose at an arbitrary orientation, for tests that have no device.
    ///
    /// Deliberately private to this crate's tests. `resolve` stays the only way
    /// to build a pose from a device, and a public constructor taking any
    /// rotation would read as landscape support that nobody has measured.
    #[cfg(test)]
    fn for_test(profile: &'a DeviceProfile, rotation: u32) -> Result<Self, PoseError> {
        Ok(Self {
            profile,
            rotation,
            width: profile.width,
            height: profile.height,
            mapping: Self::compose(profile, rotation)?,
        })
    }

    /// Resolves a pose against a live framebuffer.
    ///
    /// Refuses any orientation outside the profile's verified set. The refusal
    /// is the point: an unresolvable pose has to stay a refusal rather than
    /// fall back on the reference, since the fallback would be a transform
    /// wrong by the height of the panel, and wrong silently.
    ///
    /// The geometry is compared rather than copied. Nothing stops a caller
    /// resolving against a snapshot that was never validated, and a pose built
    /// from dimensions the profile does not recognise would scale every tap
    /// against the wrong panel.
    ///
    /// # Errors
    ///
    /// Returns [`PoseError`] when the device is in an unverified orientation or
    /// when its visible geometry disagrees with the profile.
    pub fn resolve(
        profile: &'a DeviceProfile,
        framebuffer: &FramebufferSnapshot,
    ) -> Result<Self, PoseError> {
        if !profile.verified_rotations.contains(&framebuffer.rotation) {
            return Err(PoseError::UnverifiedRotation {
                observed: framebuffer.rotation,
                verified: profile.verified_rotations,
            });
        }
        for (field, observed, expected) in [
            ("width", framebuffer.width, profile.width),
            ("height", framebuffer.height, profile.height),
        ] {
            if observed != expected {
                return Err(PoseError::GeometryMismatch {
                    field,
                    observed,
                    expected,
                });
            }
        }
        Ok(Self {
            profile,
            rotation: framebuffer.rotation,
            width: framebuffer.width,
            height: framebuffer.height,
            mapping: Self::compose(profile, framebuffer.rotation)?,
        })
    }

    #[must_use]
    pub const fn profile(&self) -> &'a DeviceProfile {
        self.profile
    }

    #[must_use]
    pub const fn rotation(&self) -> u32 {
        self.rotation
    }

    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// The mapping that will actually run at this pose.
    ///
    /// Not the profile's declared [`TouchTransform`]. Reporting that instead
    /// would print a mapping the code does not use as soon as the reader is
    /// held the other way up, which is the shape of a defect this port has
    /// already fixed once in `doctor`.
    #[must_use]
    pub const fn touch_mapping(&self) -> TouchMapping {
        self.mapping
    }

    /// Maps a raw touch coordinate into display space at this pose.
    #[must_use]
    pub fn touch_to_display(&self, raw_x: i32, raw_y: i32) -> Option<(u32, u32)> {
        let profile = self.profile;
        if !(profile.touch_x_min..=profile.touch_x_max).contains(&raw_x)
            || !(profile.touch_y_min..=profile.touch_y_max).contains(&raw_y)
        {
            return None;
        }
        // The swap decides which raw axis feeds which display axis; the
        // mirrors are then applied in display space, after scaling. Doing the
        // mirrors before the scale rounds differently at the extremes.
        let (mut x, mut y) = if self.mapping.swap_axes {
            (
                scale_touch_axis(raw_y, profile.touch_y_min, profile.touch_y_max, self.width)?,
                scale_touch_axis(raw_x, profile.touch_x_min, profile.touch_x_max, self.height)?,
            )
        } else {
            (
                scale_touch_axis(raw_x, profile.touch_x_min, profile.touch_x_max, self.width)?,
                scale_touch_axis(raw_y, profile.touch_y_min, profile.touch_y_max, self.height)?,
            )
        };
        if self.mapping.mirror_x {
            x = self.width.checked_sub(1 + x)?;
        }
        if self.mapping.mirror_y {
            y = self.height.checked_sub(1 + y)?;
        }
        Some((x, y))
    }

    /// Converts a visible display coordinate back into raw touch space at this
    /// pose.
    #[must_use]
    pub fn display_to_touch(&self, x: u32, y: u32) -> Option<(i32, i32)> {
        let profile = self.profile;
        if x >= self.width || y >= self.height {
            return None;
        }
        // The exact inverse: undo the display-space mirrors first, then undo
        // the swap. Any other order is a different transform.
        let x = if self.mapping.mirror_x {
            self.width.checked_sub(1 + x)?
        } else {
            x
        };
        let y = if self.mapping.mirror_y {
            self.height.checked_sub(1 + y)?
        } else {
            y
        };
        let (raw_x, raw_y) = if self.mapping.swap_axes {
            (
                scale_display_axis(y, self.height, profile.touch_x_min, profile.touch_x_max)?,
                scale_display_axis(x, self.width, profile.touch_y_min, profile.touch_y_max)?,
            )
        } else {
            (
                scale_display_axis(x, self.width, profile.touch_x_min, profile.touch_x_max)?,
                scale_display_axis(y, self.height, profile.touch_y_min, profile.touch_y_max)?,
            )
        };
        if !(profile.touch_x_min..=profile.touch_x_max).contains(&raw_x)
            || !(profile.touch_y_min..=profile.touch_y_max).contains(&raw_y)
        {
            return None;
        }
        Some((raw_x, raw_y))
    }
}

fn scale_touch_axis(value: i32, minimum: i32, maximum: i32, pixels: u32) -> Option<u32> {
    if pixels == 0 || value < minimum || value > maximum || maximum <= minimum {
        return None;
    }
    if pixels == 1 {
        return Some(0);
    }
    let source = i64::from(maximum) - i64::from(minimum);
    let offset = i64::from(value) - i64::from(minimum);
    let target = i64::from(pixels - 1);
    u32::try_from((offset * target + source / 2) / source).ok()
}

fn scale_display_axis(value: u32, pixels: u32, minimum: i32, maximum: i32) -> Option<i32> {
    if pixels == 0 || value >= pixels || maximum <= minimum {
        return None;
    }
    if pixels == 1 {
        return Some(minimum);
    }
    let source = i64::from(pixels - 1);
    let target = i64::from(maximum) - i64::from(minimum);
    let scaled = (i64::from(value) * target + source / 2) / source;
    i32::try_from(i64::from(minimum) + scaled).ok()
}

fn validate_framebuffer(
    profile: &DeviceProfile,
    framebuffer: &FramebufferSnapshot,
    mismatches: &mut Vec<String>,
    blockers: &mut Vec<String>,
) {
    compare_str(
        mismatches,
        "framebuffer driver",
        &framebuffer.id,
        profile.framebuffer_id,
    );
    compare(mismatches, "width", framebuffer.width, profile.width);
    compare(mismatches, "height", framebuffer.height, profile.height);
    let expected = profile.expected_geometry();
    compare(
        mismatches,
        "virtual width",
        framebuffer.virtual_width,
        expected.virtual_width,
    );
    compare(
        mismatches,
        "virtual height",
        framebuffer.virtual_height,
        expected.virtual_height,
    );
    compare(
        mismatches,
        "X offset",
        framebuffer.x_offset,
        profile.x_offset,
    );
    compare(
        mismatches,
        "Y offset",
        framebuffer.y_offset,
        profile.y_offset,
    );
    validate_pixel_format(profile, framebuffer, mismatches, blockers);
    compare(mismatches, "stride", framebuffer.stride, expected.stride);
    compare(
        mismatches,
        "memory length",
        framebuffer.memory_length,
        profile.memory_length,
    );
    compare(
        mismatches,
        "framebuffer type",
        framebuffer.kind,
        profile.framebuffer_kind,
    );
    compare(
        mismatches,
        "framebuffer visual",
        framebuffer.visual,
        profile.framebuffer_visual,
    );
    if !profile.verified_rotations.contains(&framebuffer.rotation) {
        mismatches.push(format!(
            "rotation: {} is not in this profile's verified set {:?}",
            framebuffer.rotation, profile.verified_rotations
        ));
    }

    if framebuffer.virtual_width < framebuffer.width
        || framebuffer.virtual_height < framebuffer.height
    {
        mismatches.push("virtual framebuffer is smaller than visible geometry".to_owned());
    }
    let minimum = u64::from(framebuffer.stride) * u64::from(framebuffer.virtual_height);
    if u64::from(framebuffer.memory_length) < minimum {
        mismatches.push(format!(
            "framebuffer memory {} is smaller than required {}",
            framebuffer.memory_length, minimum
        ));
    }
}

fn validate_pixel_format(
    profile: &DeviceProfile,
    framebuffer: &FramebufferSnapshot,
    mismatches: &mut Vec<String>,
    blockers: &mut Vec<String>,
) {
    compare(
        mismatches,
        "bits_per_pixel",
        framebuffer.bits_per_pixel,
        profile.bits_per_pixel,
    );
    compare(
        mismatches,
        "grayscale",
        framebuffer.grayscale,
        profile.grayscale,
    );
    compare_debug(mismatches, "red bitfield", framebuffer.red, profile.red);
    compare_debug(
        mismatches,
        "green bitfield",
        framebuffer.green,
        profile.green,
    );
    compare_debug(mismatches, "blue bitfield", framebuffer.blue, profile.blue);
    compare_debug(
        mismatches,
        "alpha bitfield",
        framebuffer.alpha,
        profile.alpha,
    );
    let valid_length = |len| len == 8 || len == 0;
    if !valid_length(framebuffer.red.length)
        || !valid_length(framebuffer.green.length)
        || !valid_length(framebuffer.blue.length)
        || !valid_length(framebuffer.alpha.length)
    {
        blockers.push(format!(
            "unconfirmed framebuffer bitfields R{:?} G{:?} B{:?} A{:?}",
            framebuffer.red, framebuffer.green, framebuffer.blue, framebuffer.alpha
        ));
    }
}

fn validate_touch(profile: &DeviceProfile, touch: &TouchSnapshot, mismatches: &mut Vec<String>) {
    compare_str(mismatches, "touch name", &touch.name, profile.touch_name);
    compare(
        mismatches,
        "touch X minimum",
        touch.x_min,
        profile.touch_x_min,
    );
    compare(
        mismatches,
        "touch X maximum",
        touch.x_max,
        profile.touch_x_max,
    );
    compare(
        mismatches,
        "touch Y minimum",
        touch.y_min,
        profile.touch_y_min,
    );
    compare(
        mismatches,
        "touch Y maximum",
        touch.y_max,
        profile.touch_y_max,
    );
}

fn compare<T>(mismatches: &mut Vec<String>, name: &str, actual: T, expected: T)
where
    T: Copy + fmt::Display + PartialEq,
{
    if actual != expected {
        mismatches.push(format!("{name}: expected {expected}, found {actual}"));
    }
}

fn compare_str(mismatches: &mut Vec<String>, name: &str, actual: &str, expected: &str) {
    if actual != expected {
        mismatches.push(format!("{name}: expected {expected}, found {actual}"));
    }
}

fn compare_debug<T>(mismatches: &mut Vec<String>, name: &str, actual: T, expected: T)
where
    T: Copy + fmt::Debug + PartialEq,
{
    if actual != expected {
        mismatches.push(format!("{name}: expected {expected:?}, found {actual:?}"));
    }
}

fn compare_identity_one_of(
    blockers: &mut Vec<String>,
    name: &str,
    expected: &[&str],
    actual: Option<&str>,
) {
    match actual {
        Some(value) if expected.contains(&value) => {}
        Some(value) => {
            let wanted = match expected {
                [only] => (*only).to_owned(),
                choices => format!("one of {}", choices.join(", ")),
            };
            blockers.push(format!("{name}: expected {wanted}, found {value}"));
        }
        None => blockers.push(format!("{name} could not be read")),
    }
}

fn compare_identity(blockers: &mut Vec<String>, name: &str, expected: &str, actual: Option<&str>) {
    match actual {
        Some(value) if value == expected => {}
        Some(value) => blockers.push(format!("{name}: expected {expected}, found {value}")),
        None => blockers.push(format!("{name} could not be read")),
    }
}

#[cfg(test)]
mod tests {
    use super::{PanelPose, PoseError, TouchMapping, TouchTransform, SUPPORTED_PROFILES};

    /// The Libra 2 at the pose it was measured in: page-turn buttons on the
    /// right, which this device reports as rotation 1. Every display
    /// coordinate below is only meaningful at that pose.
    const LIBRA_2_POSE: PanelPose<'static> = PanelPose::reference(&LIBRA_2_388);
    const CLARA_BW_POSE: PanelPose<'static> = PanelPose::reference(&CLARA_BW_391);
    const ELIPSA_2E_POSE: PanelPose<'static> = PanelPose::reference(&ELIPSA_2E_389);
    const CLARA_HD_POSE: PanelPose<'static> = PanelPose::reference(&CLARA_HD_376);

    use super::{
        Bitfield, DeviceProfile, DeviceSnapshot, FramebufferSnapshot, IdentitySnapshot, Readiness,
        TouchSnapshot, CLARA_BW_391, CLARA_HD_376, ELIPSA_2E_389, LIBRA_2_388,
        WRITE_EVIDENCE_PENDING,
    };

    /// The Libra 2 as `kobo doctor` read it from a cold boot into Nickel, in
    /// portrait. `rotation` picks the orientation: 1 as measured in portrait,
    /// 2 as measured with the reader turned.
    fn measured_libra_2(rotation: u32) -> DeviceSnapshot {
        let red = Bitfield {
            offset: 16,
            length: 8,
            msb_right: 0,
        };
        let landscape = rotation == 2;
        DeviceSnapshot {
            compatible: vec!["fsl,imx6sll-lpddr3-arm2".into(), "fsl,imx6sll".into()],
            model: Some("Freescale i.MX6SLL NTX Board".into()),
            framebuffer: Some(FramebufferSnapshot {
                id: "mxc_epdc_fb".into(),
                width: if landscape { 1680 } else { 1264 },
                height: if landscape { 1264 } else { 1680 },
                virtual_width: if landscape { 1696 } else { 1280 },
                virtual_height: if landscape { 1280 } else { 1792 },
                x_offset: 0,
                y_offset: 0,
                bits_per_pixel: 32,
                grayscale: 0,
                stride: if landscape { 6784 } else { 5120 },
                memory_length: 9_175_040,
                kind: 0,
                visual: 2,
                rotation,
                red,
                green: Bitfield { offset: 8, ..red },
                blue: Bitfield { offset: 0, ..red },
                alpha: Bitfield { offset: 24, ..red },
            }),
            touch: Some(TouchSnapshot {
                path: "/dev/input/event1".into(),
                name: "Elan Touchscreen".into(),
                x_min: 0,
                x_max: 1680,
                y_min: 0,
                y_max: 1264,
            }),
            identity: IdentitySnapshot {
                serial_prefix: Some("N418".into()),
                firmware_version: Some("4.38.23697".into()),
                kernel_release: Some("4.1.15-00868-g58a2758be07".into()),
                device_code: Some(388),
            },
        }
    }

    #[test]
    fn libra_2_matches_the_measured_portrait_device() {
        let snapshot = measured_libra_2(1);
        let report = LIBRA_2_388.validate(&snapshot);
        assert!(report.mismatches.is_empty(), "{:?}", report.mismatches);
        // Identity is exact and the attended evidence has been reviewed.
        assert_eq!(report.readiness, Readiness::WriteReady);
        assert!(
            report.write_blockers.is_empty(),
            "{:?}",
            report.write_blockers
        );
        assert!(LIBRA_2_388.write_identity_blockers(&snapshot).is_empty());
        assert_eq!(
            super::identify_profile(&snapshot).map(|profile| profile.id),
            Some("libra-2-388")
        );
    }

    /// Rotating the reader moves six of the compared fields, so the same
    /// hardware stops matching. This is recorded rather than desired: the
    /// values come from the device, and the test exists so that the day
    /// somebody fixes it, they have to say so here.
    #[test]
    fn libra_2_rejects_the_same_device_in_landscape() {
        let snapshot = measured_libra_2(2);
        let report = LIBRA_2_388.validate(&snapshot);
        assert_eq!(report.readiness, Readiness::Rejected);
        assert_eq!(report.mismatches.len(), 6);
        assert!(super::identify_profile(&snapshot).is_none());
        assert!(
            LIBRA_2_388.write_identity_blockers(&snapshot).is_empty(),
            "identity is unaffected by orientation"
        );
    }

    /// The same hardware with the buttons on the left, which it reports as
    /// rotation 3 with every geometry field unchanged. Accepted since the pose
    /// was verified on the device on 2026-08-22: digitiser measured
    /// panel-fixed by corner, rendered marks landing diametrically opposite
    /// their rotation 1 positions.
    #[test]
    fn libra_2_matches_the_same_device_buttons_left() {
        let snapshot = measured_libra_2(3);
        let report = LIBRA_2_388.validate(&snapshot);
        assert!(report.mismatches.is_empty(), "{:?}", report.mismatches);
        // Identity is exact and the attended evidence has been reviewed.
        assert_eq!(report.readiness, Readiness::WriteReady);
        assert!(
            report.write_blockers.is_empty(),
            "{:?}",
            report.write_blockers
        );
        assert_eq!(
            super::identify_profile(&snapshot).map(|profile| profile.id),
            Some("libra-2-388")
        );
        let pose = PanelPose::resolve(
            &LIBRA_2_388,
            snapshot.framebuffer.as_ref().expect("a framebuffer"),
        )
        .expect("a verified pose resolves");
        assert_eq!(
            pose.touch_mapping(),
            TouchMapping {
                swap_axes: true,
                mirror_x: true,
                mirror_y: true,
            },
            "the resolved pose carries the composed mapping, not the reference one"
        );
    }

    /// `resolve` at an unverified rotation is a loud refusal, not a fallback
    /// to the reference mapping, which would be wrong by the height of the
    /// panel and wrong silently.
    #[test]
    fn resolve_refuses_an_unverified_rotation() {
        let snapshot = measured_libra_2(2);
        let error = PanelPose::resolve(
            &LIBRA_2_388,
            snapshot.framebuffer.as_ref().expect("a framebuffer"),
        )
        .expect_err("landscape has never been measured");
        assert_eq!(
            error,
            PoseError::UnverifiedRotation {
                observed: 2,
                verified: &[1, 3],
            }
        );
    }

    /// The derived geometry is numerically identical to the constants each
    /// profile also stores, so relaxing validation to the derived expectation
    /// accepted nothing the constants would have rejected.
    #[test]
    fn every_profile_derives_the_geometry_it_stores() {
        for profile in SUPPORTED_PROFILES {
            let expected = profile.expected_geometry();
            assert_eq!(
                (
                    expected.virtual_width,
                    expected.virtual_height,
                    expected.stride
                ),
                (
                    profile.virtual_width,
                    profile.virtual_height,
                    profile.stride
                ),
                "{} derives geometry it was not measured to have",
                profile.id
            );
        }
    }

    /// Every verified rotation must be composable, meaning a half turn or
    /// none from the reference frame. A quarter-turn entry would pass
    /// validation and then fail at resolve, which is loud but later than it
    /// needs to be.
    #[test]
    fn every_verified_rotation_is_composable_on_every_profile() {
        for profile in SUPPORTED_PROFILES {
            assert!(
                profile
                    .verified_rotations
                    .contains(&profile.reference_rotation),
                "{} does not verify its own reference frame",
                profile.id
            );
            for rotation in profile.verified_rotations {
                assert!(
                    (rotation % 4 + 4 - profile.reference_rotation % 4) % 4 % 2 == 0,
                    "{} verifies rotation {rotation}, a quarter turn from its reference",
                    profile.id
                );
            }
        }
    }

    /// Captured from three physical taps on the real Libra 2 with
    /// `kobo touch-probe`, read-only and ungrabbed, in an L so that each leg
    /// moves along exactly one physical axis.
    ///
    /// Corner assertions alone cannot catch a mirrored axis, because the four
    /// corners of a rectangle map onto themselves under any rotation, and a
    /// round trip through `display_to_touch` proves only that the inverse is
    /// an inverse. An earlier version of this test asserted both and passed
    /// while the transform was inverted by the full height of the panel.
    /// Every profile's declared mapping is written in the frame it was
    /// measured at, until `validate` accepts more than one pose. Without this,
    /// `reference_rotation` is a second copy of a fact with nothing checking
    /// the two agree, which is the drift this log keeps recording.
    #[test]
    fn reference_rotation_matches_the_measured_rotation_on_every_profile() {
        for profile in SUPPORTED_PROFILES {
            assert_eq!(
                profile.reference_rotation, profile.rotation,
                "{} declares a reference frame it was not measured in",
                profile.id
            );
        }
    }

    /// The composed mapping at the buttons-left pose, against the raw
    /// coordinates physically measured there.
    ///
    /// The expected values are hand-derived and written as literals: each is
    /// the rotate 1 answer for the same physical corner, turned through 180
    /// degrees. Nothing under test computed them. A fixture whose expectations
    /// come from running the composition asserts only that the code agrees with
    /// itself.
    ///
    /// Each tap is named by both anchors, buttons and port, rather than by its
    /// position in a table. Matching a rotate 3 tap against the wrong rotate 1
    /// tap produces a plausible answer that is wrong by most of the panel, and
    /// that mistake was made once while this fixture was being reviewed.
    ///
    /// Predicted before the panel was ever driven at this pose. The rendered
    /// mark experiment in the port log is what confirms the premise underneath
    /// it: the driver says the controller intends to rotate the image, and only
    /// the panel says it does.
    #[test]
    fn libra_2_composed_touch_at_the_buttons_left_pose() {
        let pose = PanelPose::for_test(&LIBRA_2_388, 3).expect("a half turn from the reference");
        assert_eq!(
            pose.touch_mapping(),
            TouchMapping {
                swap_axes: true,
                mirror_x: true,
                mirror_y: true,
            },
            "a half turn flips both mirrors and leaves the swap alone"
        );

        // Buttons left, port bottom edge. Raw values measured on the device.
        let far_from_buttons_away_from_port = pose
            .touch_to_display(1587, 72)
            .expect("measured tap maps to the display");
        let far_from_buttons_port_edge = pose
            .touch_to_display(96, 75)
            .expect("measured tap maps to the display");
        let near_buttons_port_edge = pose
            .touch_to_display(95, 1168)
            .expect("measured tap maps to the display");

        assert_eq!(far_from_buttons_away_from_port, (1191, 93));
        assert_eq!(far_from_buttons_port_edge, (1188, 1583));
        assert_eq!(near_buttons_port_edge, (96, 1584));

        // The assertion that actually carries this test. The digitiser does not
        // move, so one raw coordinate mapped at both poses must land at
        // diametrically opposite display points. If the composition were the
        // identity, which is the likeliest bug in this step, every corner
        // assertion and every round trip below still passes and this fails.
        let reference = PanelPose::reference(&LIBRA_2_388);
        for raw in [(1587, 72), (96, 75), (95, 1168), (800, 600)] {
            let upright = reference
                .touch_to_display(raw.0, raw.1)
                .expect("maps at the reference pose");
            let turned = pose
                .touch_to_display(raw.0, raw.1)
                .expect("maps at the turned pose");
            assert_eq!(
                upright.0 + turned.0,
                LIBRA_2_388.width - 1,
                "x is not diametrically opposite for raw {raw:?}"
            );
            assert_eq!(
                upright.1 + turned.1,
                LIBRA_2_388.height - 1,
                "y is not diametrically opposite for raw {raw:?}"
            );
        }

        // The L walks the other way round at this pose. Stated as gradients so
        // that a mirrored axis fails here even if the literals above are edited
        // to match a broken implementation.
        assert!(
            far_from_buttons_port_edge.1 > far_from_buttons_away_from_port.1,
            "increasing raw x must run up the display at this pose"
        );
        assert!(
            far_from_buttons_port_edge.0 > near_buttons_port_edge.0,
            "increasing raw y must run left across the display at this pose"
        );
    }

    /// A quarter turn is refused rather than guessed. The clockwise and
    /// anticlockwise numbering is board-dependent in the driver and this board
    /// has not been read for it, so a quarter turn is the case where the
    /// ambiguity decides the answer.
    #[test]
    fn a_quarter_turn_from_the_reference_is_refused() {
        for rotation in [0, 2] {
            assert_eq!(
                PanelPose::for_test(&LIBRA_2_388, rotation),
                Err(PoseError::UnsupportedRotationDelta {
                    observed: rotation,
                    reference: 1,
                })
            );
        }
    }

    /// All four declared transforms survive the round trip through the
    /// composable form, and a half turn is its own inverse.
    #[test]
    fn lowering_preserves_every_declared_transform() {
        let lowered = [
            (TouchTransform::Direct, (false, false, false)),
            (TouchTransform::Transpose, (true, false, false)),
            (TouchTransform::TransposeMirrorY, (true, false, true)),
            (TouchTransform::TransposeMirrorX, (true, true, false)),
        ];
        for (transform, (swap_axes, mirror_x, mirror_y)) in lowered {
            let mapping = transform.lower();
            assert_eq!(
                mapping,
                TouchMapping {
                    swap_axes,
                    mirror_x,
                    mirror_y
                },
                "{transform:?} lowered into the wrong frame"
            );
            assert_eq!(
                mapping.rotated_180().rotated_180(),
                mapping,
                "two half turns are not the identity for {transform:?}"
            );
            assert_ne!(
                mapping.rotated_180(),
                mapping,
                "a half turn changed nothing for {transform:?}"
            );
        }
    }

    #[test]
    fn libra_2_touch_matches_three_physically_measured_taps() {
        // Tap 1, top-left corner of the image.
        let top_left = LIBRA_2_POSE
            .touch_to_display(54, 62)
            .expect("measured tap maps to the display");
        // Tap 2, bottom-left: straight down the same edge, so only the
        // physical vertical changed, and only raw_x moved.
        let bottom_left = LIBRA_2_POSE
            .touch_to_display(1600, 66)
            .expect("measured tap maps to the display");
        // Tap 3, bottom-right: straight across the bottom, so only the
        // physical horizontal changed, and only raw_y moved.
        let bottom_right = LIBRA_2_POSE
            .touch_to_display(1596, 1172)
            .expect("measured tap maps to the display");

        assert_eq!(top_left, (62, 54));
        assert_eq!(bottom_left, (66, 1599));
        assert_eq!(bottom_right, (1171, 1595));

        // The shape of the L, stated independently of the exact numbers, so
        // that a mirrored axis fails here even if the values are edited.
        assert!(top_left.1 < bottom_left.1, "the left edge runs downward");
        assert!(
            top_left.0.abs_diff(bottom_left.0) < 32,
            "the left edge stays at one side"
        );
        assert!(
            bottom_left.0 < bottom_right.0,
            "the bottom edge runs rightward"
        );
        assert!(
            bottom_left.1.abs_diff(bottom_right.1) < 32,
            "the bottom edge stays at one end"
        );
    }

    #[test]
    fn libra_2_touch_edges_stay_inside_the_panel_and_round_trip() {
        for raw in [(0, 0), (0, 1264), (1680, 0), (1680, 1264)] {
            let display = LIBRA_2_POSE
                .touch_to_display(raw.0, raw.1)
                .expect("measured Libra 2 edge maps to the display");
            assert!(display.0 < LIBRA_2_388.width, "x escaped: {display:?}");
            assert!(display.1 < LIBRA_2_388.height, "y escaped: {display:?}");
        }
        assert_eq!(LIBRA_2_POSE.touch_to_display(0, 0), Some((0, 0)));
        assert_eq!(
            LIBRA_2_POSE.touch_to_display(1680, 1264),
            Some((1263, 1679))
        );
        for display in [(0, 0), (1263, 0), (0, 1679), (1263, 1679), (632, 840)] {
            let raw = LIBRA_2_POSE
                .display_to_touch(display.0, display.1)
                .expect("Libra 2 display point maps to the controller");
            assert_eq!(LIBRA_2_POSE.touch_to_display(raw.0, raw.1), Some(display));
        }
        assert_eq!(LIBRA_2_POSE.display_to_touch(1264, 0), None);
        assert_eq!(LIBRA_2_POSE.display_to_touch(0, 1680), None);
    }

    #[test]
    fn touch_transform_matches_measured_corners() {
        assert_eq!(CLARA_BW_POSE.touch_to_display(0, 1071), Some((0, 0)));
        assert_eq!(CLARA_BW_POSE.touch_to_display(0, 0), Some((1071, 0)));
        assert_eq!(CLARA_BW_POSE.touch_to_display(1447, 1071), Some((0, 1447)));
        assert_eq!(CLARA_BW_POSE.touch_to_display(1447, 0), Some((1071, 1447)));
        assert_eq!(CLARA_BW_POSE.touch_to_display(1448, 0), None);
    }

    #[test]
    fn display_and_touch_coordinates_round_trip_at_edges_and_inside() {
        for display in [(0, 0), (1071, 0), (0, 1447), (1071, 1447), (109, 110)] {
            let raw = CLARA_BW_POSE
                .display_to_touch(display.0, display.1)
                .expect("display point maps to controller");
            assert_eq!(CLARA_BW_POSE.touch_to_display(raw.0, raw.1), Some(display));
        }
        assert_eq!(CLARA_BW_POSE.display_to_touch(1072, 0), None);
        assert_eq!(CLARA_BW_POSE.display_to_touch(0, 1448), None);
    }

    #[test]
    fn elipsa_touch_edges_stay_inside_the_panel_and_display_points_round_trip() {
        for raw in [(0, 0), (0, 1404), (1872, 0), (1872, 1404)] {
            let display = ELIPSA_2E_POSE
                .touch_to_display(raw.0, raw.1)
                .expect("measured Elipsa edge maps to the display");
            assert!(display.0 < ELIPSA_2E_389.width, "x escaped: {display:?}");
            assert!(display.1 < ELIPSA_2E_389.height, "y escaped: {display:?}");
        }
        assert_eq!(ELIPSA_2E_POSE.touch_to_display(0, 0), Some((0, 1871)));
        assert_eq!(ELIPSA_2E_POSE.touch_to_display(1872, 1404), Some((1403, 0)));
        for display in [(0, 0), (1403, 0), (0, 1871), (1403, 1871), (702, 936)] {
            let raw = ELIPSA_2E_POSE
                .display_to_touch(display.0, display.1)
                .expect("Elipsa display point maps to the controller");
            assert_eq!(ELIPSA_2E_POSE.touch_to_display(raw.0, raw.1), Some(display));
        }
        assert_eq!(ELIPSA_2E_POSE.display_to_touch(1404, 0), None);
        assert_eq!(ELIPSA_2E_POSE.display_to_touch(0, 1872), None);
    }

    /// Captured from a physical touch on the real Clara BW with
    /// `kobo touch-probe`, read-only and ungrabbed.
    ///
    /// The corner ranges alone only prove the axes are swapped; they cannot
    /// prove which way each axis runs, because a flipped transform maps the
    /// corner set onto itself. This sample is the evidence for the direction:
    /// the owner touched roughly a centimetre in from the top-left edges, which
    /// is about 118 pixels at this panel's 300 pixels per inch, and the
    /// transform placed it at (109, 110).
    #[test]
    fn touch_transform_matches_a_physically_measured_touch() {
        let mapped = CLARA_BW_POSE
            .touch_to_display(110, 962)
            .expect("the measured raw sample is in range");
        assert_eq!(mapped, (109, 110));

        // A flip of either axis still lands inside the screen, so only distance
        // from the touched corner distinguishes them. Both are far away.
        let flipped_x = CLARA_BW_POSE
            .touch_to_display(110, 1071 - 962)
            .expect("in range");
        let flipped_y = CLARA_BW_POSE
            .touch_to_display(1447 - 110, 962)
            .expect("in range");
        assert_eq!(flipped_x, (962, 110));
        assert_eq!(flipped_y, (109, 1337));
    }

    /// Captured from a physical top-left touch on the real Elipsa 2E with
    /// `kobo touch-probe`, read-only and ungrabbed. The raw controller sample
    /// `(1838, 30)` mapped to display `(30, 34)`, matching where the owner
    /// touched rather than merely mapping the controller's corner set onto the
    /// panel's corner set.
    ///
    /// Carried over from the pre-`PanelPose` API unchanged: the expected
    /// values are the ones measured on that hardware, not values re-derived
    /// from the transform they are meant to pin.
    #[test]
    fn elipsa_touch_transform_matches_a_physically_measured_touch() {
        let mapped = ELIPSA_2E_POSE
            .touch_to_display(1838, 30)
            .expect("the measured raw Elipsa sample is in range");
        assert_eq!(mapped, (30, 34));

        // Either plausible reversed axis still produces an in-range point,
        // but places it far from the top-left location that was touched.
        let flipped_x = ELIPSA_2E_POSE
            .touch_to_display(1838, 1404 - 30)
            .expect("in range");
        let flipped_y = ELIPSA_2E_POSE
            .touch_to_display(1872 - 1838, 30)
            .expect("in range");
        assert_eq!(flipped_x, (1373, 34));
        assert_eq!(flipped_y, (30, 1837));
    }

    /// A doctor snapshot taken from a real Clara HD, matched against the
    /// strict profile across every reviewed firmware. This is the only pin on
    /// hardware none of us can re-measure.
    #[test]
    fn clara_hd_doctor_snapshot_matches_its_strict_profile() {
        let channel = Bitfield {
            offset: 16,
            length: 8,
            msb_right: 0,
        };
        let mut snapshot = DeviceSnapshot {
            compatible: vec!["fsl,imx6sll-lpddr3-arm2".into(), "fsl,imx6sll".into()],
            model: Some("Freescale i.MX6SLL NTX Board".into()),
            framebuffer: Some(FramebufferSnapshot {
                id: "mxc_epdc_fb".into(),
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
                red: channel,
                green: Bitfield {
                    offset: 8,
                    ..channel
                },
                blue: Bitfield {
                    offset: 0,
                    ..channel
                },
                alpha: Bitfield {
                    offset: 24,
                    ..channel
                },
            }),
            touch: Some(TouchSnapshot {
                path: "/dev/input/event1".into(),
                name: "cyttsp5_mt".into(),
                x_min: 0,
                x_max: 1447,
                y_min: 0,
                y_max: 1071,
            }),
            identity: IdentitySnapshot {
                serial_prefix: Some("N249".into()),
                firmware_version: Some("4.38.23684".into()),
                kernel_release: Some("4.1.15-00136-g12655eaaef89".into()),
                device_code: Some(376),
            },
        };
        for firmware in CLARA_HD_376.firmware_versions {
            snapshot.identity.firmware_version = Some((*firmware).into());
            let report = CLARA_HD_376.validate(&snapshot);
            assert_eq!(report.readiness, Readiness::WriteReady);
            assert!(report.mismatches.is_empty());
            assert!(report.write_blockers.is_empty());
            assert!(CLARA_HD_376.write_identity_blockers(&snapshot).is_empty());
        }
        assert_eq!(super::identify_profile(&snapshot), Some(&CLARA_HD_376));
    }

    #[test]
    fn clara_hd_touch_edges_map_inside_the_panel_and_round_trip() {
        for raw in [(0, 0), (0, 1071), (1447, 0), (1447, 1071)] {
            let display = CLARA_HD_POSE
                .touch_to_display(raw.0, raw.1)
                .expect("measured Clara HD edge maps to the display");
            assert!(display.0 < CLARA_HD_376.width, "x escaped: {display:?}");
            assert!(display.1 < CLARA_HD_376.height, "y escaped: {display:?}");
        }
        for display in [(0, 0), (1071, 0), (0, 1447), (1071, 1447), (536, 724)] {
            let raw = CLARA_HD_POSE
                .display_to_touch(display.0, display.1)
                .expect("Clara HD display point maps to the controller");
            assert_eq!(CLARA_HD_POSE.touch_to_display(raw.0, raw.1), Some(display));
        }
    }

    /// Captured from a physical touch about a centimetre in from the top-left
    /// of the Clara HD. The axis ranges prove that the controller is rotated;
    /// this sample proves the direction of both axes.
    #[test]
    fn clara_hd_touch_transform_matches_a_physically_measured_touch() {
        assert_eq!(CLARA_HD_POSE.touch_to_display(160, 909), Some((162, 160)));

        let flipped_x = CLARA_HD_POSE
            .touch_to_display(160, 1071 - 909)
            .expect("flipped sample remains in range");
        let flipped_y = CLARA_HD_POSE
            .touch_to_display(1447 - 160, 909)
            .expect("flipped sample remains in range");
        assert_eq!(flipped_x, (909, 160));
        assert_eq!(flipped_y, (162, 1287));
    }

    #[test]
    fn empty_snapshot_is_rejected() {
        let report = CLARA_BW_391.validate(&DeviceSnapshot::default());
        assert_eq!(report.readiness, Readiness::Rejected);
        assert!(!report.mismatches.is_empty());
    }

    #[test]
    fn a_fully_identified_device_is_write_ready_and_a_strange_firmware_is_not() {
        let red = Bitfield {
            offset: 0,
            length: 8,
            msb_right: 0,
        };
        let snapshot = DeviceSnapshot {
            compatible: vec!["mediatek,mt8110".into(), "mediatek,mt8512".into()],
            model: Some("MediaTek MT8110 board".into()),
            framebuffer: Some(FramebufferSnapshot {
                id: "hwtcon".into(),
                width: 1072,
                height: 1448,
                virtual_width: 1072,
                virtual_height: 1448,
                x_offset: 0,
                y_offset: 0,
                bits_per_pixel: 32,
                grayscale: 0,
                stride: 4288,
                memory_length: 6_243_328,
                kind: 0,
                visual: 2,
                rotation: 3,
                red,
                green: Bitfield { offset: 8, ..red },
                blue: Bitfield { offset: 16, ..red },
                alpha: Bitfield { offset: 24, ..red },
            }),
            touch: Some(TouchSnapshot {
                path: "/dev/input/event1".into(),
                name: "cyttsp5_mt".into(),
                x_min: 0,
                x_max: 1447,
                y_min: 0,
                y_max: 1071,
            }),
            identity: IdentitySnapshot {
                serial_prefix: Some("N365".into()),
                firmware_version: Some("4.45.23697".into()),
                kernel_release: Some("4.9.77".into()),
                device_code: Some(391),
            },
        };
        let report = CLARA_BW_391.validate(&snapshot);
        assert_eq!(report.readiness, Readiness::WriteReady);
        assert!(report.mismatches.is_empty());
        assert!(report.write_blockers.is_empty());
        assert!(CLARA_BW_391.write_identity_blockers(&snapshot).is_empty());

        // The same hardware on unreviewed firmware is still matched, but the
        // verdict drops back to read-only: geometry is not proof of identity.
        let mut updated = snapshot.clone();
        updated.identity.firmware_version = Some("4.99.99999".into());
        let report = CLARA_BW_391.validate(&updated);
        assert_eq!(report.readiness, Readiness::ReadOnlyMatched);
        assert!(report.mismatches.is_empty());
        assert_eq!(report.write_blockers.len(), 1);

        // A profile still awaiting owner-attended evidence is matched but
        // never write ready, however exact the identity.
        let candidate = DeviceProfile {
            write_ready: false,
            ..CLARA_BW_391
        };
        let report = candidate.validate(&snapshot);
        assert_eq!(report.readiness, Readiness::ReadOnlyMatched);
        assert!(report.mismatches.is_empty());
        assert_eq!(report.write_blockers, vec![WRITE_EVIDENCE_PENDING]);
    }

    #[test]
    fn parses_the_measured_version_file_without_retaining_the_serial() {
        let identity = IdentitySnapshot::parse(
            Some("N365410043013,4.9.77,4.45.23697,4.9.77,4.9.77,00000000-0000-0000-0000-000000000391"),
            Some("4.9.77\n"),
        );
        assert_eq!(identity.serial_prefix.as_deref(), Some("N365"));
        assert_eq!(identity.firmware_version.as_deref(), Some("4.45.23697"));
        assert_eq!(identity.kernel_release.as_deref(), Some("4.9.77"));
        assert_eq!(identity.device_code, Some(391));
    }

    #[test]
    fn missing_identity_blocks_every_write() {
        let blockers = CLARA_BW_391.write_identity_blockers(&DeviceSnapshot::default());
        assert_eq!(blockers.len(), 4);
    }
}

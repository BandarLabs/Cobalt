//! Strict, fail-closed Kobo device profiles.

use std::fmt;

/// How a device's touch controller reports position relative to the display.
///
/// This used to be inferred from the framebuffer's `rotation`. It cannot be:
/// the framebuffer says how the panel is scanned, and says nothing about which
/// way round the digitiser underneath it was soldered. Two devices reporting
/// `rotation: 1` were found to need different transforms, so the mapping is
/// declared per profile and has to be measured with `kobo touch-probe` rather
/// than derived.
///
/// # Why this is only safe while `rotation` is matched exactly
///
/// A value here is a composition of two separate facts, and only one of them
/// belongs in a profile. How the digitiser sits under the panel is fixed in
/// the hardware and never changes. How display coordinates map onto physical
/// space is decided by the framebuffer's `rotation`, which is live state: a
/// Libra 2 held upside down reports `rotation: 3` where the same device
/// upright reports 1, with every geometry field unchanged, and it flips as the
/// reader is handled.
///
/// So a variant here is correct only at the rotation it was measured at. That
/// holds today because `validate` compares `rotation` exactly, and a session
/// cannot open unless the device is in the state the profile was measured in.
/// The refusal is doing double duty: it is the thing that keeps this field
/// honest.
///
/// Anyone relaxing the `rotation` comparison, which the orientation behaviour
/// otherwise invites, has to replace this field at the same time. Touch would
/// not fail loudly. It would land in the wrong place. As evidence that the
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
    /// backwards.
    Transpose,
    /// Axes exchanged and Y mirrored.
    ///
    /// This is what every `rotation: 1` device did before the transform became
    /// explicit. It is preserved for the Elipsa 2E so that its behaviour does
    /// not change, but it has never been checked against a real finger: its
    /// test asserts only the arithmetic it was derived from. The Libra 2 is
    /// also `rotation: 1`, also an Elan controller, and measured as a plain
    /// `Transpose`, so this variant is the likeliest place for a latent bug on
    /// a device nobody here can test.
    TransposeMirrorY,
    /// Axes exchanged and X mirrored.
    ///
    /// Confirmed on the Clara BW, which is the only transform here backed by a
    /// physically captured touch.
    TransposeMirrorX,
}

pub const CLARA_BW_391: DeviceProfile = DeviceProfile {
    id: "clara-bw-391",
    model: "Kobo Clara BW",
    device_code: 391,
    device_tree_model: "MediaTek MT8110 board",
    compatible_fragments: &["mediatek,mt8110", "mediatek,mt8512"],
    framebuffer_id: "hwtcon",
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
    touch_name: "cyttsp5_mt",
    touch_x_min: 0,
    touch_x_max: 1447,
    touch_y_min: 0,
    touch_y_max: 1071,
    serial_prefix: "N365",
    firmware_version: "4.45.23697",
    kernel_release: "4.9.77",
};

pub const ELIPSA_2E_389: DeviceProfile = DeviceProfile {
    id: "elipsa-2e-389",
    model: "Kobo Elipsa 2E",
    device_code: 389,
    device_tree_model: "MediaTek MT8110 board",
    compatible_fragments: &["mediatek,mt8110", "mediatek,mt8512"],
    framebuffer_id: "hwtcon",
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
    touch_name: "Elan Touchscreen",
    touch_x_min: 0,
    touch_x_max: 1872,
    touch_y_min: 0,
    touch_y_max: 1404,
    serial_prefix: "N605",
    firmware_version: "4.38.23697",
    kernel_release: "4.9.77",
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
    touch_name: "Elan Touchscreen",
    touch_x_min: 0,
    touch_x_max: 1680,
    touch_y_min: 0,
    touch_y_max: 1264,
    serial_prefix: "N418",
    firmware_version: "4.38.23697",
    kernel_release: "4.1.15-00868-g58a2758be07",
};

pub const SUPPORTED_PROFILES: &[&DeviceProfile] = &[&CLARA_BW_391, &ELIPSA_2E_389, &LIBRA_2_388];

#[must_use]
pub fn identify_profile(snapshot: &DeviceSnapshot) -> Option<&'static DeviceProfile> {
    SUPPORTED_PROFILES
        .iter()
        .copied()
        .find(|profile| profile.validate(snapshot).readiness != Readiness::Rejected)
}

pub const READ_ONLY_WRITE_BLOCKER: &str =
    "write profile is intentionally incomplete until read-only doctor output is reviewed";

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
    pub touch_name: &'static str,
    pub touch_x_min: i32,
    pub touch_x_max: i32,
    pub touch_y_min: i32,
    pub touch_y_max: i32,
    pub serial_prefix: &'static str,
    pub firmware_version: &'static str,
    pub kernel_release: &'static str,
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

        blockers.push(READ_ONLY_WRITE_BLOCKER.to_owned());

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
        compare_identity(
            &mut blockers,
            "firmware version",
            self.firmware_version,
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

    #[must_use]
    pub fn touch_to_display(&self, raw_x: i32, raw_y: i32) -> Option<(u32, u32)> {
        if !(self.touch_x_min..=self.touch_x_max).contains(&raw_x)
            || !(self.touch_y_min..=self.touch_y_max).contains(&raw_y)
        {
            return None;
        }
        let horizontal = scale_touch_axis(raw_y, self.touch_y_min, self.touch_y_max, self.width)?;
        let vertical = scale_touch_axis(raw_x, self.touch_x_min, self.touch_x_max, self.height)?;
        match self.touch_transform {
            TouchTransform::Transpose => Some((horizontal, vertical)),
            TouchTransform::TransposeMirrorY => {
                Some((horizontal, self.height.checked_sub(1 + vertical)?))
            }
            TouchTransform::TransposeMirrorX => {
                Some((self.width.checked_sub(1 + horizontal)?, vertical))
            }
            TouchTransform::Direct => Some((
                scale_touch_axis(raw_x, self.touch_x_min, self.touch_x_max, self.width)?,
                scale_touch_axis(raw_y, self.touch_y_min, self.touch_y_max, self.height)?,
            )),
        }
    }

    /// Converts a visible display coordinate back to the touch controller's
    /// rotated coordinate space.
    ///
    /// The browser simulator deliberately takes this round trip before hit
    /// testing, so the same measured transform that gates the device is also
    /// exercised during ordinary application development.
    #[must_use]
    pub fn display_to_touch(&self, x: u32, y: u32) -> Option<(i32, i32)> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let (raw_x, raw_y) = match self.touch_transform {
            TouchTransform::Transpose => (
                scale_display_axis(y, self.height, self.touch_x_min, self.touch_x_max)?,
                scale_display_axis(x, self.width, self.touch_y_min, self.touch_y_max)?,
            ),
            TouchTransform::TransposeMirrorY => (
                scale_display_axis(
                    self.height.checked_sub(1 + y)?,
                    self.height,
                    self.touch_x_min,
                    self.touch_x_max,
                )?,
                scale_display_axis(x, self.width, self.touch_y_min, self.touch_y_max)?,
            ),
            TouchTransform::TransposeMirrorX => (
                scale_display_axis(y, self.height, self.touch_x_min, self.touch_x_max)?,
                scale_display_axis(
                    self.width.checked_sub(1 + x)?,
                    self.width,
                    self.touch_y_min,
                    self.touch_y_max,
                )?,
            ),
            TouchTransform::Direct => (
                scale_display_axis(x, self.width, self.touch_x_min, self.touch_x_max)?,
                scale_display_axis(y, self.height, self.touch_y_min, self.touch_y_max)?,
            ),
        };
        if !(self.touch_x_min..=self.touch_x_max).contains(&raw_x)
            || !(self.touch_y_min..=self.touch_y_max).contains(&raw_y)
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
    compare(
        mismatches,
        "virtual width",
        framebuffer.virtual_width,
        profile.virtual_width,
    );
    compare(
        mismatches,
        "virtual height",
        framebuffer.virtual_height,
        profile.virtual_height,
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
    compare(mismatches, "stride", framebuffer.stride, profile.stride);
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
    compare(
        mismatches,
        "rotation",
        framebuffer.rotation,
        profile.rotation,
    );

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

fn compare_identity(blockers: &mut Vec<String>, name: &str, expected: &str, actual: Option<&str>) {
    match actual {
        Some(value) if value == expected => {}
        Some(value) => blockers.push(format!("{name}: expected {expected}, found {value}")),
        None => blockers.push(format!("{name} could not be read")),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Bitfield, DeviceSnapshot, FramebufferSnapshot, IdentitySnapshot, Readiness, TouchSnapshot,
        CLARA_BW_391, ELIPSA_2E_389, LIBRA_2_388,
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
        assert_eq!(report.readiness, Readiness::ReadOnlyMatched);
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

    /// Captured from three physical taps on the real Libra 2 with
    /// `kobo touch-probe`, read-only and ungrabbed, in an L so that each leg
    /// moves along exactly one physical axis.
    ///
    /// Corner assertions alone cannot catch a mirrored axis, because the four
    /// corners of a rectangle map onto themselves under any rotation, and a
    /// round trip through `display_to_touch` proves only that the inverse is
    /// an inverse. An earlier version of this test asserted both and passed
    /// while the transform was inverted by the full height of the panel.
    #[test]
    fn libra_2_touch_matches_three_physically_measured_taps() {
        // Tap 1, top-left corner of the image.
        let top_left = LIBRA_2_388
            .touch_to_display(54, 62)
            .expect("measured tap maps to the display");
        // Tap 2, bottom-left: straight down the same edge, so only the
        // physical vertical changed, and only raw_x moved.
        let bottom_left = LIBRA_2_388
            .touch_to_display(1600, 66)
            .expect("measured tap maps to the display");
        // Tap 3, bottom-right: straight across the bottom, so only the
        // physical horizontal changed, and only raw_y moved.
        let bottom_right = LIBRA_2_388
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
            let display = LIBRA_2_388
                .touch_to_display(raw.0, raw.1)
                .expect("measured Libra 2 edge maps to the display");
            assert!(display.0 < LIBRA_2_388.width, "x escaped: {display:?}");
            assert!(display.1 < LIBRA_2_388.height, "y escaped: {display:?}");
        }
        assert_eq!(LIBRA_2_388.touch_to_display(0, 0), Some((0, 0)));
        assert_eq!(LIBRA_2_388.touch_to_display(1680, 1264), Some((1263, 1679)));
        for display in [(0, 0), (1263, 0), (0, 1679), (1263, 1679), (632, 840)] {
            let raw = LIBRA_2_388
                .display_to_touch(display.0, display.1)
                .expect("Libra 2 display point maps to the controller");
            assert_eq!(LIBRA_2_388.touch_to_display(raw.0, raw.1), Some(display));
        }
        assert_eq!(LIBRA_2_388.display_to_touch(1264, 0), None);
        assert_eq!(LIBRA_2_388.display_to_touch(0, 1680), None);
    }

    #[test]
    fn touch_transform_matches_measured_corners() {
        assert_eq!(CLARA_BW_391.touch_to_display(0, 1071), Some((0, 0)));
        assert_eq!(CLARA_BW_391.touch_to_display(0, 0), Some((1071, 0)));
        assert_eq!(CLARA_BW_391.touch_to_display(1447, 1071), Some((0, 1447)));
        assert_eq!(CLARA_BW_391.touch_to_display(1447, 0), Some((1071, 1447)));
        assert_eq!(CLARA_BW_391.touch_to_display(1448, 0), None);
    }

    #[test]
    fn display_and_touch_coordinates_round_trip_at_edges_and_inside() {
        for display in [(0, 0), (1071, 0), (0, 1447), (1071, 1447), (109, 110)] {
            let raw = CLARA_BW_391
                .display_to_touch(display.0, display.1)
                .expect("display point maps to controller");
            assert_eq!(CLARA_BW_391.touch_to_display(raw.0, raw.1), Some(display));
        }
        assert_eq!(CLARA_BW_391.display_to_touch(1072, 0), None);
        assert_eq!(CLARA_BW_391.display_to_touch(0, 1448), None);
    }

    #[test]
    fn elipsa_touch_edges_stay_inside_the_panel_and_display_points_round_trip() {
        for raw in [(0, 0), (0, 1404), (1872, 0), (1872, 1404)] {
            let display = ELIPSA_2E_389
                .touch_to_display(raw.0, raw.1)
                .expect("measured Elipsa edge maps to the display");
            assert!(display.0 < ELIPSA_2E_389.width, "x escaped: {display:?}");
            assert!(display.1 < ELIPSA_2E_389.height, "y escaped: {display:?}");
        }
        assert_eq!(ELIPSA_2E_389.touch_to_display(0, 0), Some((0, 1871)));
        assert_eq!(ELIPSA_2E_389.touch_to_display(1872, 1404), Some((1403, 0)));
        for display in [(0, 0), (1403, 0), (0, 1871), (1403, 1871), (702, 936)] {
            let raw = ELIPSA_2E_389
                .display_to_touch(display.0, display.1)
                .expect("Elipsa display point maps to the controller");
            assert_eq!(ELIPSA_2E_389.touch_to_display(raw.0, raw.1), Some(display));
        }
        assert_eq!(ELIPSA_2E_389.display_to_touch(1404, 0), None);
        assert_eq!(ELIPSA_2E_389.display_to_touch(0, 1872), None);
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
        let mapped = CLARA_BW_391
            .touch_to_display(110, 962)
            .expect("the measured raw sample is in range");
        assert_eq!(mapped, (109, 110));

        // A flip of either axis still lands inside the screen, so only distance
        // from the touched corner distinguishes them. Both are far away.
        let flipped_x = CLARA_BW_391
            .touch_to_display(110, 1071 - 962)
            .expect("in range");
        let flipped_y = CLARA_BW_391
            .touch_to_display(1447 - 110, 962)
            .expect("in range");
        assert_eq!(flipped_x, (962, 110));
        assert_eq!(flipped_y, (109, 1337));
    }

    #[test]
    fn empty_snapshot_is_rejected() {
        let report = CLARA_BW_391.validate(&DeviceSnapshot::default());
        assert_eq!(report.readiness, Readiness::Rejected);
        assert!(!report.mismatches.is_empty());
    }

    #[test]
    fn measured_snapshot_remains_read_only_until_reviewed() {
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
        assert_eq!(report.readiness, Readiness::ReadOnlyMatched);
        assert!(report.mismatches.is_empty());
        assert!(!report.write_blockers.is_empty());
        assert!(CLARA_BW_391.write_identity_blockers(&snapshot).is_empty());
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

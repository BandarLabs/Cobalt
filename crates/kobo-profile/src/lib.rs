//! Strict, fail-closed Kobo device profiles.

use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FramebufferController {
    Hwtcon,
    MxcfbV2,
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

pub const SUPPORTED_PROFILES: &[&DeviceProfile] = &[&CLARA_BW_391, &CLARA_HD_376, &ELIPSA_2E_389];

#[must_use]
pub fn identify_profile(snapshot: &DeviceSnapshot) -> Option<&'static DeviceProfile> {
    SUPPORTED_PROFILES
        .iter()
        .copied()
        .find(|profile| profile.validate(snapshot).readiness != Readiness::Rejected)
}

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

    #[must_use]
    pub fn touch_to_display(&self, raw_x: i32, raw_y: i32) -> Option<(u32, u32)> {
        if !(self.touch_x_min..=self.touch_x_max).contains(&raw_x)
            || !(self.touch_y_min..=self.touch_y_max).contains(&raw_y)
        {
            return None;
        }
        let horizontal = scale_touch_axis(raw_y, self.touch_y_min, self.touch_y_max, self.width)?;
        let vertical = scale_touch_axis(raw_x, self.touch_x_min, self.touch_x_max, self.height)?;
        match self.rotation {
            1 => Some((horizontal, self.height.checked_sub(1 + vertical)?)),
            3 => Some((self.width.checked_sub(1 + horizontal)?, vertical)),
            _ => Some((
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
        let (raw_x, raw_y) = match self.rotation {
            1 => (
                scale_display_axis(
                    self.height.checked_sub(1 + y)?,
                    self.height,
                    self.touch_x_min,
                    self.touch_x_max,
                )?,
                scale_display_axis(x, self.width, self.touch_y_min, self.touch_y_max)?,
            ),
            3 => (
                scale_display_axis(y, self.height, self.touch_x_min, self.touch_x_max)?,
                scale_display_axis(
                    self.width.checked_sub(1 + x)?,
                    self.width,
                    self.touch_y_min,
                    self.touch_y_max,
                )?,
            ),
            _ => (
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

#[cfg(test)]
mod tests {
    use super::{
        Bitfield, DeviceProfile, DeviceSnapshot, FramebufferSnapshot, IdentitySnapshot, Readiness,
        TouchSnapshot, CLARA_BW_391, CLARA_HD_376, ELIPSA_2E_389, WRITE_EVIDENCE_PENDING,
    };

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

    /// Captured from a physical top-left touch on the real Elipsa 2E with
    /// `kobo touch-probe`, read-only and ungrabbed. The raw controller sample
    /// `(1838, 30)` mapped to display `(30, 34)`, matching where the owner
    /// touched rather than merely mapping the controller's corner set onto the
    /// panel's corner set.
    #[test]
    fn elipsa_touch_transform_matches_a_physically_measured_touch() {
        let mapped = ELIPSA_2E_389
            .touch_to_display(1838, 30)
            .expect("the measured raw Elipsa sample is in range");
        assert_eq!(mapped, (30, 34));

        // Either plausible reversed axis still produces an in-range point,
        // but places it far from the top-left location that was touched.
        let flipped_x = ELIPSA_2E_389
            .touch_to_display(1838, 1404 - 30)
            .expect("in range");
        let flipped_y = ELIPSA_2E_389
            .touch_to_display(1872 - 1838, 30)
            .expect("in range");
        assert_eq!(flipped_x, (1373, 34));
        assert_eq!(flipped_y, (30, 1837));
    }

    #[test]
    fn empty_snapshot_is_rejected() {
        let report = CLARA_BW_391.validate(&DeviceSnapshot::default());
        assert_eq!(report.readiness, Readiness::Rejected);
        assert!(!report.mismatches.is_empty());
    }

    #[test]
    fn reviewed_profile_with_exact_identity_is_write_ready() {
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

        let candidate = DeviceProfile {
            write_ready: false,
            ..CLARA_BW_391
        };
        let report = candidate.validate(&snapshot);
        assert_eq!(report.readiness, Readiness::ReadOnlyMatched);
        assert_eq!(report.write_blockers, vec![WRITE_EVIDENCE_PENDING]);
    }

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
            let display = CLARA_HD_376
                .touch_to_display(raw.0, raw.1)
                .expect("measured Clara HD edge maps to the display");
            assert!(display.0 < CLARA_HD_376.width, "x escaped: {display:?}");
            assert!(display.1 < CLARA_HD_376.height, "y escaped: {display:?}");
        }
        for display in [(0, 0), (1071, 0), (0, 1447), (1071, 1447), (536, 724)] {
            let raw = CLARA_HD_376
                .display_to_touch(display.0, display.1)
                .expect("Clara HD display point maps to the controller");
            assert_eq!(CLARA_HD_376.touch_to_display(raw.0, raw.1), Some(display));
        }
    }

    /// Captured from a physical touch about a centimetre in from the top-left
    /// of the Clara HD. The axis ranges prove that the controller is rotated;
    /// this sample proves the direction of both axes.
    #[test]
    fn clara_hd_touch_transform_matches_a_physically_measured_touch() {
        assert_eq!(CLARA_HD_376.touch_to_display(160, 909), Some((162, 160)));

        let flipped_x = CLARA_HD_376
            .touch_to_display(160, 1071 - 909)
            .expect("flipped sample remains in range");
        let flipped_y = CLARA_HD_376
            .touch_to_display(1447 - 160, 909)
            .expect("flipped sample remains in range");
        assert_eq!(flipped_x, (909, 160));
        assert_eq!(flipped_y, (162, 1287));
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

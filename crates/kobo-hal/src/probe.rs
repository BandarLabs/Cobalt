use kobo_abi::{fb, input};
use kobo_profile::{Bitfield, DeviceSnapshot, FramebufferSnapshot, TouchSnapshot};
use std::error::Error;
use std::fmt;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct ProbeError {
    context: &'static str,
    source: io::Error,
}

impl ProbeError {
    fn new(context: &'static str, source: io::Error) -> Self {
        Self { context, source }
    }
}

impl fmt::Display for ProbeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.context, self.source)
    }
}

impl Error for ProbeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.source)
    }
}

/// Collects device identity using read-only files and query ioctls.
///
/// # Errors
///
/// Returns a framebuffer query error or an error querying a recognized touch
/// device without guessing a fallback device. The absence of a recognized
/// touch device remains part of the snapshot so the doctor can report unknown
/// hardware read-only.
pub fn probe_device() -> Result<DeviceSnapshot, ProbeError> {
    let compatible = read_nul_list(&[
        "/sys/firmware/devicetree/base/compatible",
        "/proc/device-tree/compatible",
    ])
    .unwrap_or_default();
    let model = read_nul_string(&[
        "/sys/firmware/devicetree/base/model",
        "/proc/device-tree/model",
        "/sys/devices/soc0/machine",
    ]);

    let framebuffer = probe_framebuffer(Path::new("/dev/fb0"))?;
    let touch = discover_touch_path("/proc/bus/input/devices")
        .map(|path| probe_touch(&path))
        .transpose()?;

    Ok(DeviceSnapshot {
        compatible,
        model,
        framebuffer: Some(framebuffer),
        touch,
        identity: probe_identity(),
    })
}

/// Reads the non-identifying parts of the Kobo version file and the kernel
/// release. Missing files yield empty fields rather than an error, so the
/// read-only doctor still works on an unknown device.
fn probe_identity() -> kobo_profile::IdentitySnapshot {
    let version = fs::read_to_string("/mnt/onboard/.kobo/version").ok();
    let kernel = fs::read_to_string("/proc/sys/kernel/osrelease").ok();
    kobo_profile::IdentitySnapshot::parse(version.as_deref(), kernel.as_deref())
}

fn probe_framebuffer(path: &Path) -> Result<FramebufferSnapshot, ProbeError> {
    let file =
        File::open(path).map_err(|error| ProbeError::new("open framebuffer read-only", error))?;
    let fixed = fb::fixed_screen_info(&file)
        .map_err(|error| ProbeError::new("query fixed screen info", error))?;
    let variable = fb::variable_screen_info(&file)
        .map_err(|error| ProbeError::new("query variable screen info", error))?;
    let id_end = fixed
        .id
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(fixed.id.len());

    Ok(FramebufferSnapshot {
        id: String::from_utf8_lossy(&fixed.id[..id_end]).into_owned(),
        width: variable.xres,
        height: variable.yres,
        virtual_width: variable.xres_virtual,
        virtual_height: variable.yres_virtual,
        x_offset: variable.xoffset,
        y_offset: variable.yoffset,
        bits_per_pixel: variable.bits_per_pixel,
        grayscale: variable.grayscale,
        stride: fixed.line_length,
        memory_length: fixed.smem_len,
        kind: fixed.kind,
        visual: fixed.visual,
        rotation: variable.rotate,
        red: bitfield(variable.red),
        green: bitfield(variable.green),
        blue: bitfield(variable.blue),
        alpha: bitfield(variable.transp),
    })
}

fn bitfield(value: fb::FbBitfield) -> Bitfield {
    Bitfield {
        offset: value.offset,
        length: value.length,
        msb_right: value.msb_right,
    }
}

fn probe_touch(path: &Path) -> Result<TouchSnapshot, ProbeError> {
    let file =
        File::open(path).map_err(|error| ProbeError::new("open touch input read-only", error))?;
    let name = input::device_name(&file)
        .map_err(|error| ProbeError::new("query touch device name", error))?;
    let x = input::absolute_axis(&file, input::ABS_MT_POSITION_X)
        .map_err(|error| ProbeError::new("query touch X range", error))?;
    let y = input::absolute_axis(&file, input::ABS_MT_POSITION_Y)
        .map_err(|error| ProbeError::new("query touch Y range", error))?;
    // The kernel answers a range query for an axis the device does not have
    // with zeros rather than an error. A single-touch controller reports on
    // `ABS_X` and `ABS_Y`, and those are the ranges its contacts are scaled
    // against, so an empty multi-touch range sends the query there instead.
    let (x, y) = if x.maximum > x.minimum && y.maximum > y.minimum {
        (x, y)
    } else {
        (
            input::absolute_axis(&file, input::ABS_X)
                .map_err(|error| ProbeError::new("query touch X range", error))?,
            input::absolute_axis(&file, input::ABS_Y)
                .map_err(|error| ProbeError::new("query touch Y range", error))?,
        )
    };
    Ok(TouchSnapshot {
        path: path.display().to_string(),
        name,
        x_min: x.minimum,
        x_max: x.maximum,
        y_min: y.minimum,
        y_max: y.maximum,
    })
}

fn read_nul_list(paths: &[&str]) -> Option<Vec<String>> {
    paths.iter().find_map(|path| {
        fs::read(path).ok().map(|bytes| {
            bytes
                .split(|byte| *byte == 0)
                .filter(|part| !part.is_empty())
                .map(|part| String::from_utf8_lossy(part).into_owned())
                .collect()
        })
    })
}

fn read_nul_string(paths: &[&str]) -> Option<String> {
    read_nul_list(paths).and_then(|values| values.into_iter().next())
}

fn discover_touch_path(input_devices_path: &str) -> Option<PathBuf> {
    let content = fs::read_to_string(input_devices_path).ok()?;
    discover_touch_path_from(&content, usize::BITS)
}

/// The controllers every measured Kobo uses, picked by name before anything
/// else so that a reader somebody has tested keeps the node it was tested on.
const NAMED_TOUCH_CONTROLLERS: [&str; 2] = ["cyttsp5_mt", "Elan Touchscreen"];

/// Name fragments of nodes that are never the touch panel, whatever axes
/// they declare. Buttons, the power key and the cover sensor live on these,
/// and the old NTX kernels publish their key node as `ntx_event0`.
const NOT_TOUCH: [&str; 6] = [
    "gpio-keys",
    "gpio_keys",
    "pwrkey",
    "power",
    "cover",
    "ntx_event",
];

/// One block of `/proc/bus/input/devices`, reduced to what selection reads.
struct InputBlock<'a> {
    name: &'a str,
    event: &'a str,
    absolute: Option<&'a str>,
    keys: Option<&'a str>,
}

impl<'a> InputBlock<'a> {
    fn parse(block: &'a str) -> Option<Self> {
        let field = |prefix: &str| block.lines().find_map(|line| line.strip_prefix(prefix));
        let name = field("N: Name=")?.trim_matches('"');
        let event = field("H: Handlers=")?.split_whitespace().find(|handler| {
            handler
                .strip_prefix("event")
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        })?;
        Some(Self {
            name,
            event,
            absolute: field("B: ABS="),
            keys: field("B: KEY="),
        })
    }

    fn has_axes(&self, x: u16, y: u16, word_bits: u32) -> bool {
        self.absolute.is_some_and(|bitmap| {
            crate::gpio::bitmap_has(bitmap, x, word_bits)
                && crate::gpio::bitmap_has(bitmap, y, word_bits)
        })
    }

    fn is_multitouch(&self, word_bits: u32) -> bool {
        self.has_axes(
            input::ABS_MT_POSITION_X,
            input::ABS_MT_POSITION_Y,
            word_bits,
        )
    }

    fn is_single_touch(&self, word_bits: u32) -> bool {
        self.has_axes(input::ABS_X, input::ABS_Y, word_bits)
            && self
                .keys
                .is_some_and(|bitmap| crate::gpio::bitmap_has(bitmap, input::BTN_TOUCH, word_bits))
    }

    fn is_excluded(&self) -> bool {
        let name = self.name.to_ascii_lowercase();
        NOT_TOUCH.iter().any(|fragment| name.contains(fragment))
    }

    fn path(&self) -> PathBuf {
        Path::new("/dev/input").join(self.event)
    }
}

/// Picks the touch panel out of the kernel's input device list.
///
/// A named controller wins, exactly as before this looked at anything else.
/// Failing that, the first node that declares both multi-touch position axes
/// is taken, and failing that the first that declares `ABS_X`, `ABS_Y` and
/// `BTN_TOUCH`, which is how a single-touch controller describes itself.
/// Nodes named for buttons, the power key or the cover are passed over even
/// if they declare axes, because a hall sensor or an accelerometer behind a
/// key node is not somewhere a tap can come from.
///
/// `word_bits` is the width of the kernel's `long`, which is how wide each
/// hex word of the capability bitmaps is.
fn discover_touch_path_from(content: &str, word_bits: u32) -> Option<PathBuf> {
    let blocks = content
        .split("\n\n")
        .filter_map(InputBlock::parse)
        .collect::<Vec<_>>();
    blocks
        .iter()
        .find(|block| {
            NAMED_TOUCH_CONTROLLERS
                .iter()
                .any(|name| block.name.contains(name))
        })
        .or_else(|| {
            blocks
                .iter()
                .filter(|block| !block.is_excluded())
                .find(|block| block.is_multitouch(word_bits))
        })
        .or_else(|| {
            blocks
                .iter()
                .filter(|block| !block.is_excluded())
                .find(|block| block.is_single_touch(word_bits))
        })
        .map(InputBlock::path)
}

/// Lists input evidence without selecting, grabbing or reading event streams.
/// Serial/unique IDs and physical paths are deliberately excluded.
#[must_use]
pub fn input_inventory() -> Vec<kobo_profile::observation::InputObservation> {
    let content = match fs::read_to_string("/proc/bus/input/devices") {
        Ok(content) => content,
        Err(error) => {
            return vec![kobo_profile::observation::InputObservation {
                name: String::new(),
                path: "/proc/bus/input/devices".into(),
                capabilities: Vec::new(),
                axes: Vec::new(),
                error: Some(format!("read inventory: {}", error.kind())),
            }]
        }
    };
    let mut devices = input_inventory_from(&content);
    for device in &mut devices {
        match File::open(&device.path) {
            Ok(file) => {
                for axis in [0, 1, input::ABS_MT_POSITION_X, input::ABS_MT_POSITION_Y] {
                    if let Ok(range) = input::absolute_axis(&file, axis) {
                        device
                            .axes
                            .push(format!("{axis}:{}..{}", range.minimum, range.maximum));
                    }
                }
            }
            Err(error) => device.error = Some(format!("open read-only: {}", error.kind())),
        }
    }
    devices
}

fn input_inventory_from(content: &str) -> Vec<kobo_profile::observation::InputObservation> {
    content
        .split("\n\n")
        .filter_map(|block| {
            let name = block
                .lines()
                .find_map(|line| line.strip_prefix("N: Name="))?;
            let handlers = block
                .lines()
                .find_map(|line| line.strip_prefix("H: Handlers="))?;
            let event = handlers.split_whitespace().find(|handler| {
                handler
                    .strip_prefix("event")
                    .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
            })?;
            Some(kobo_profile::observation::InputObservation {
                name: name
                    .trim_matches('"')
                    .chars()
                    .take(kobo_profile::observation::MAX_INPUT_TEXT)
                    .collect(),
                path: format!("/dev/input/{event}"),
                capabilities: block
                    .lines()
                    .filter(|line| {
                        ["B: EV=", "B: KEY=", "B: ABS=", "B: SW="]
                            .iter()
                            .any(|p| line.starts_with(p))
                    })
                    .take(4)
                    .map(|line| {
                        line.chars()
                            .take(kobo_profile::observation::MAX_INPUT_TEXT)
                            .collect()
                    })
                    .collect(),
                axes: Vec::new(),
                error: None,
            })
        })
        .take(kobo_profile::observation::MAX_INPUT_DEVICES)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::discover_touch_path_from;
    use std::path::Path;

    #[test]
    fn inventory_keeps_unknown_single_touch_and_separate_power_nodes() {
        let fixture = "N: Name=\"infrared touch\"\nH: Handlers=event4\nB: ABS=3\nU: Uniq=private\n\nN: Name=\"dedicated power\"\nH: Handlers=event5\nB: KEY=100000 0 0 0\n\nN: Name=\"bad\"\nH: Handlers=event../../bad\n";
        let devices = super::input_inventory_from(fixture);
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].capabilities, ["B: ABS=3"]);
        assert_eq!(devices[1].path, "/dev/input/event5");
        assert!(super::discover_touch_path_from(fixture, 32).is_none());
    }

    #[test]
    fn finds_cypress_touch_handler() {
        let fixture = "I: Bus=0018 Vendor=0000 Product=0000 Version=0000\n\
N: Name=\"cyttsp5_mt\"\n\
H: Handlers=event1 mouse0\n\
B: EV=b\n\n\
N: Name=\"gpio-keys\"\n\
H: Handlers=event0\n";
        assert_eq!(
            discover_touch_path_from(fixture, 32).as_deref(),
            Some(Path::new("/dev/input/event1"))
        );
    }

    #[test]
    fn finds_elan_touch_handler() {
        let fixture = "I: Bus=0018 Vendor=04f3 Product=0000 Version=0000\n\
N: Name=\"Elan Touchscreen\"\n\
H: Handlers=mouse0 event2\n\
B: EV=b\n\n\
N: Name=\"gpio-keys\"\n\
H: Handlers=event0\n";
        assert_eq!(
            discover_touch_path_from(fixture, 32).as_deref(),
            Some(Path::new("/dev/input/event2"))
        );
    }
    /// A multi-touch controller with no name anybody here has seen: pressure,
    /// touch major and minor, both position axes and a tracking id, printed
    /// in 32-bit words the way a 32-bit kernel prints them.
    const UNNAMED_MULTITOUCH: &str = "N: Name=\"zinitix_touch\"\n\
H: Handlers=event3\n\
B: EV=b\n\
B: KEY=400 0 0 0 0 0 0 0 0 0 0\n\
B: ABS=2630000 1000003\n";

    const GPIO_KEYS: &str = "N: Name=\"gpio-keys\"\n\
H: Handlers=kbd event0\n\
B: EV=3\n\
B: KEY=8000000 0\n";

    #[test]
    fn an_unnamed_controller_is_found_by_its_multitouch_axes() {
        let fixture = format!("{GPIO_KEYS}\n{UNNAMED_MULTITOUCH}");
        assert_eq!(
            discover_touch_path_from(&fixture, 32).as_deref(),
            Some(Path::new("/dev/input/event3"))
        );
    }

    #[test]
    fn the_same_bitmap_printed_by_a_64_bit_kernel_is_read_the_same_way() {
        let fixture = "N: Name=\"zinitix_touch\"\n\
H: Handlers=event3\n\
B: KEY=400 0 0 0 0 0\n\
B: ABS=263000001000003\n";
        assert_eq!(
            discover_touch_path_from(fixture, 64).as_deref(),
            Some(Path::new("/dev/input/event3"))
        );
    }

    #[test]
    fn a_single_touch_controller_is_found_by_its_axes_and_btn_touch() {
        let fixture = "N: Name=\"zforce-ir-touch\"\n\
H: Handlers=event1\n\
B: KEY=400 0 0 0 0 0 0 0 0 0 0\n\
B: ABS=3\n";
        assert_eq!(
            discover_touch_path_from(fixture, 32).as_deref(),
            Some(Path::new("/dev/input/event1"))
        );
    }

    #[test]
    fn button_power_and_cover_nodes_are_never_taken_for_the_panel() {
        // An accelerometer or a hall sensor can sit behind a key node and
        // declare axes. None of these is somewhere a tap comes from.
        for name in ["gpio-keys", "bd71828-pwrkey", "ntx_event0", "hall cover"] {
            let fixture = format!(
                "N: Name=\"{name}\"\nH: Handlers=event0\nB: KEY=400 0 0 0 0 0 0 0 0 0 0\nB: ABS=2630000 1000003\n"
            );
            assert!(
                discover_touch_path_from(&fixture, 32).is_none(),
                "{name} was taken for the touch panel"
            );
        }
    }

    #[test]
    fn a_named_controller_is_still_preferred_over_an_earlier_capability_match() {
        let fixture = format!(
            "{UNNAMED_MULTITOUCH}\nN: Name=\"cyttsp5_mt\"\nH: Handlers=event1\nB: ABS=2630000 1000003\n"
        );
        assert_eq!(
            discover_touch_path_from(&fixture, 32).as_deref(),
            Some(Path::new("/dev/input/event1"))
        );
    }

    #[test]
    fn a_multitouch_node_is_preferred_over_a_single_touch_one() {
        let fixture = format!(
            "N: Name=\"pointer\"\nH: Handlers=event2\nB: KEY=400 0 0 0 0 0 0 0 0 0 0\nB: ABS=3\n\n{UNNAMED_MULTITOUCH}"
        );
        assert_eq!(
            discover_touch_path_from(&fixture, 32).as_deref(),
            Some(Path::new("/dev/input/event3"))
        );
    }
}

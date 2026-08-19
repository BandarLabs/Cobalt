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
/// Returns the first framebuffer or touch discovery error without guessing a
/// fallback device.
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
    let touch =
        discover_touch_path("/proc/bus/input/devices").and_then(|path| probe_touch(&path).ok());

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
    discover_touch_path_from(&content)
}

fn discover_touch_path_from(content: &str) -> Option<PathBuf> {
    content.split("\n\n").find_map(|block| {
        let name_matches = block
            .lines()
            .find(|line| line.starts_with("N: Name="))
            .is_some_and(|line| line.contains("cyttsp5_mt") || line.contains("Elan Touchscreen"));
        if !name_matches {
            return None;
        }
        let handlers = block
            .lines()
            .find(|line| line.starts_with("H: Handlers="))?;
        let event = handlers
            .strip_prefix("H: Handlers=")?
            .split_whitespace()
            .find(|handler| handler.starts_with("event"))?;
        Some(Path::new("/dev/input").join(event))
    })
}

#[cfg(test)]
mod tests {
    use super::discover_touch_path_from;
    use std::path::Path;

    #[test]
    fn finds_cypress_touch_handler() {
        let fixture = "I: Bus=0018 Vendor=0000 Product=0000 Version=0000\n\
N: Name=\"cyttsp5_mt\"\n\
H: Handlers=event1 mouse0\n\
B: EV=b\n\n\
N: Name=\"gpio-keys\"\n\
H: Handlers=event0\n";
        assert_eq!(
            discover_touch_path_from(fixture).as_deref(),
            Some(Path::new("/dev/input/event1"))
        );
    }
}

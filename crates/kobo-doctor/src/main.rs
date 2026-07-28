use kobo_hal::observe::MAXIMUM_OBSERVE_SECONDS;
use kobo_hal::refresh::Rect;
use kobo_hal::surface::{read_region, SurfaceGeometry};
use kobo_hal::{observe_touch, probe_device};
use kobo_profile::{DeviceProfile, FramebufferSnapshot, Readiness, CLARA_BW_391};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

/// Opting into touch observation. It stays read-only: the device is opened
/// read-only and never grabbed, so the stock reader keeps every event.
const OBSERVE_TOUCH_VARIABLE: &str = "KOBO_DOCTOR_OBSERVE_TOUCH";

/// Opting into a screenshot of whatever is on the panel right now.
///
/// This lives in the doctor rather than in a tool of its own because it is
/// exactly what the doctor already is: a read-only look at the device. It
/// opens `/dev/fb0` for reading, copies it, and closes it. Nothing is grabbed,
/// nothing is refreshed and no pixel is written, so it is safe against the
/// stock reader or against one of our own applications, running or not --
/// which matters, because the screen worth photographing is usually the one
/// that has just gone wrong and must not be disturbed to be seen.
const CAPTURE_VARIABLE: &str = "KOBO_DOCTOR_CAPTURE";

/// The line the capture is announced with, so the host can find the picture in
/// a transcript that also carries the whole probe report.
const CAPTURE_HEADER: &str = "capture-begin";
const CAPTURE_FOOTER: &str = "capture-end";

/// Grey is worth the conversion cost here. The panel is 32-bit in memory and
/// single-channel in reality, so sending all four bytes would quadruple a
/// transfer that has to cross a USB-network link, to carry three copies of the
/// same number and an alpha byte that is always opaque.
fn grey_of(pixels: &[u8]) -> Vec<u8> {
    pixels.chunks_exact(4).map(|pixel| pixel[1]).collect()
}

fn main() -> ExitCode {
    println!("Kobo doctor 0.1.0");
    println!("mode: read-only (query ioctls only)");
    println!("profile: {} ({})", CLARA_BW_391.id, CLARA_BW_391.model);

    let snapshot = match probe_device() {
        Ok(snapshot) => snapshot,
        Err(error) => {
            eprintln!("probe failed: {error}");
            return ExitCode::FAILURE;
        }
    };

    println!("device-tree compatible: {}", snapshot.compatible.join(", "));
    if let Some(model) = &snapshot.model {
        println!("device-tree model: {model}");
    }
    if let Some(framebuffer) = &snapshot.framebuffer {
        println!(
            "framebuffer: id={} {}x{} virtual={}x{} offset={},{} bpp={} grayscale={} stride={} map={} type={} visual={} rotation={}",
            framebuffer.id,
            framebuffer.width,
            framebuffer.height,
            framebuffer.virtual_width,
            framebuffer.virtual_height,
            framebuffer.x_offset,
            framebuffer.y_offset,
            framebuffer.bits_per_pixel,
            framebuffer.grayscale,
            framebuffer.stride,
            framebuffer.memory_length,
            framebuffer.kind,
            framebuffer.visual,
            framebuffer.rotation
        );
        println!(
            "pixel fields: R{:?} G{:?} B{:?} A{:?}",
            framebuffer.red, framebuffer.green, framebuffer.blue, framebuffer.alpha
        );
    }
    // Identity is what gates every write. The full serial is deliberately never
    // read past its four-character model prefix.
    let identity = &snapshot.identity;
    println!(
        "identity: model={} firmware={} kernel={} device-code={}",
        identity.serial_prefix.as_deref().unwrap_or("<unknown>"),
        identity.firmware_version.as_deref().unwrap_or("<unknown>"),
        identity.kernel_release.as_deref().unwrap_or("<unknown>"),
        identity
            .device_code
            .map_or_else(|| "<unknown>".to_owned(), |code| code.to_string()),
    );
    if let Some(touch) = &snapshot.touch {
        println!(
            "touch: {} at {} X={}..{} Y={}..{}",
            touch.name, touch.path, touch.x_min, touch.x_max, touch.y_min, touch.y_max
        );
    }

    let report = CLARA_BW_391.validate(&snapshot);
    println!("result: {}", report.readiness);
    for mismatch in &report.mismatches {
        eprintln!("mismatch: {mismatch}");
    }
    for blocker in &report.write_blockers {
        println!("write blocker: {blocker}");
    }

    if report.readiness == Readiness::Rejected {
        return ExitCode::from(2);
    }

    // Observation is only offered once the profile matched, so events are never
    // interpreted with a transform that does not belong to this hardware.
    if let Some(request) = std::env::var_os(OBSERVE_TOUCH_VARIABLE) {
        let touch_path = snapshot.touch.as_ref().map(|touch| touch.path.clone());
        if let Err(error) = observe(
            &CLARA_BW_391,
            touch_path.as_deref(),
            &request.to_string_lossy(),
        ) {
            eprintln!("touch observation failed: {error}");
            return ExitCode::FAILURE;
        }
    }

    // Last, so that a capture failure still leaves the whole report behind.
    if std::env::var_os(CAPTURE_VARIABLE).is_some() {
        let Some(framebuffer) = snapshot.framebuffer.as_ref() else {
            eprintln!("capture failed: no framebuffer was discovered");
            return ExitCode::FAILURE;
        };
        if let Err(error) = capture(framebuffer) {
            eprintln!("capture failed: {error}");
            return ExitCode::FAILURE;
        }
    }

    ExitCode::SUCCESS
}

/// Prints the whole panel as base64 grey, one byte per pixel.
///
/// Base64 rather than raw because this comes home down an SSH pipe alongside
/// human-readable output, and a megabyte and a half of arbitrary bytes in the
/// middle of a transcript will find every terminal and every pipe that is not
/// binary-clean. The width and height are printed with it so the host never
/// has to assume a panel size it did not measure.
fn capture(framebuffer: &FramebufferSnapshot) -> Result<(), String> {
    let geometry = SurfaceGeometry {
        width: framebuffer.width,
        height: framebuffer.height,
        stride: framebuffer.stride,
        bits_per_pixel: framebuffer.bits_per_pixel,
        memory_length: u64::from(framebuffer.memory_length),
    };
    let file = OpenOptions::new()
        .read(true)
        .open("/dev/fb0")
        .map_err(|error| format!("open /dev/fb0 for reading: {error}"))?;
    let whole = Rect {
        x: 0,
        y: 0,
        width: framebuffer.width,
        height: framebuffer.height,
    };
    let snapshot =
        read_region(&file, geometry, whole).map_err(|error| format!("read the panel: {error}"))?;
    let grey = grey_of(snapshot.pixels());
    println!(
        "{CAPTURE_HEADER} {} {} {}",
        framebuffer.width,
        framebuffer.height,
        grey.len()
    );
    // Written straight out in chunks: one 2 MB String would be the largest
    // allocation this binary ever made, on a device with 512 MB and a reader
    // already in it.
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for chunk in grey.chunks(48) {
        let mut line = base64_line(chunk);
        line.push('\n');
        out.write_all(line.as_bytes())
            .map_err(|error| format!("write the capture: {error}"))?;
    }
    out.write_all(format!("{CAPTURE_FOOTER}\n").as_bytes())
        .map_err(|error| format!("write the capture: {error}"))?;
    Ok(())
}

/// Standard base64, padded.
fn base64_line(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for group in bytes.chunks(3) {
        let mut block = [0_u8; 3];
        block[..group.len()].copy_from_slice(group);
        let packed = (u32::from(block[0]) << 16) | (u32::from(block[1]) << 8) | u32::from(block[2]);
        for index in 0..4 {
            if index <= group.len() {
                let sextet = (packed >> (18 - index * 6)) & 0x3f;
                encoded.push(ALPHABET[sextet as usize] as char);
            } else {
                encoded.push('=');
            }
        }
    }
    encoded
}

fn observe(profile: &DeviceProfile, touch_path: Option<&str>, request: &str) -> Result<(), String> {
    let seconds: u64 = request
        .trim()
        .parse()
        .map_err(|_| format!("{OBSERVE_TOUCH_VARIABLE} must be a whole number of seconds"))?;
    if seconds == 0 || seconds > MAXIMUM_OBSERVE_SECONDS {
        return Err(format!(
            "{OBSERVE_TOUCH_VARIABLE} must be between 1 and {MAXIMUM_OBSERVE_SECONDS} seconds"
        ));
    }
    let path = touch_path.ok_or("no touch device was discovered")?;
    println!("touch observation: {seconds}s read-only on {path}, not grabbed");
    println!(
        "touch transform under test: display_x = {} - raw_y, display_y = raw_x",
        profile.touch_y_max
    );
    let reported = observe_touch(
        Path::new(path),
        profile,
        Duration::from_secs(seconds),
        |observation| println!("touch: {observation}"),
    )
    .map_err(|error| error.to_string())?;
    println!("touch observation complete: {reported} event(s)");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{base64_line, grey_of};

    #[test]
    fn the_encoder_agrees_with_the_standard_at_every_remainder() {
        assert_eq!(base64_line(b""), "");
        assert_eq!(base64_line(b"f"), "Zg==");
        assert_eq!(base64_line(b"fo"), "Zm8=");
        assert_eq!(base64_line(b"foo"), "Zm9v");
        assert_eq!(base64_line(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64_line(&[0x00, 0xff, 0x80]), "AP+A");
    }

    #[test]
    fn a_pixel_becomes_one_grey_byte_and_the_alpha_is_dropped() {
        let pixels = [10, 20, 30, 255, 40, 50, 60, 255];
        assert_eq!(
            grey_of(&pixels),
            vec![20, 50],
            "the panel is single-channel, so the three colour bytes agree and any one of them is the grey"
        );
    }
}

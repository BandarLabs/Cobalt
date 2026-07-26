use kobo_hal::observe::MAXIMUM_OBSERVE_SECONDS;
use kobo_hal::{observe_touch, probe_device};
use kobo_profile::{DeviceProfile, Readiness, CLARA_BW_391};
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

/// Opting into touch observation. It stays read-only: the device is opened
/// read-only and never grabbed, so the stock reader keeps every event.
const OBSERVE_TOUCH_VARIABLE: &str = "KOBO_DOCTOR_OBSERVE_TOUCH";

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

    ExitCode::SUCCESS
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

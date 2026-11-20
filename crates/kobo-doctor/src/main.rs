use kobo_hal::observe::MAXIMUM_OBSERVE_SECONDS;
use kobo_hal::refresh::Rect;
use kobo_hal::surface::{read_region, SurfaceGeometry};
use kobo_hal::{observe_touch, probe_device};
use kobo_profile::{
    identify_profile, DeviceProfile, DeviceSnapshot, FramebufferSnapshot, PanelPose, PoseError,
    Readiness,
};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::process::{Command, ExitCode};
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

/// Opting into a recording: `seconds:fps`.
///
/// The screenshot's sibling. A still says what the panel looked like; a
/// recording says how it got there, which is the question when a tap lands in
/// the wrong place or a screen flashes something before settling.
const RECORD_VARIABLE: &str = "KOBO_DOCTOR_RECORD";

/// Where the recording is written on the device.
const RECORD_PATH_VARIABLE: &str = "KOBO_DOCTOR_RECORD_PATH";
const DEFAULT_RECORD_PATH: &str = "/mnt/onboard/.kobo-record.bin";

/// Recorded frames are written to a file rather than down the SSH pipe.
///
/// A still is one and a half megabytes and can go home base64 in a transcript.
/// A recording is that many times over, and encoding it to text while the
/// panel is still moving would make this loop miss the frames it exists to
/// catch. So it lands on the device at full speed and the host fetches it
/// afterwards, when nothing is waiting.
const RECORD_MAGIC: &[u8; 8] = b"KOBOCST1";

/// The ceiling on a recording: a tool that watches the device must always stop
/// on its own.
///
/// Five minutes was the touch probe's number, borrowed. It turned out to be
/// the length of the tour rather than a bound on it, and a recording that
/// stops in the middle of the thing it was filming is worth nothing. Only
/// changed frames are kept, so ten minutes of a panel that mostly sits still
/// is not twice the file five minutes was.
const MAXIMUM_RECORD_SECONDS: u64 = 600;

/// Bounds on the sampling rate. E-ink settles in about a fifth of a second, so
/// past five frames a second there is nothing new to see and the loop only
/// competes with the reader for the memory bus.
const MAXIMUM_RECORD_FPS: u32 = 5;

/// Grey is worth the conversion cost here. The panel is 32-bit in memory and
/// single-channel in reality, so sending all four bytes would quadruple a
/// transfer that has to cross a USB-network link, to carry three copies of the
/// same number and an alpha byte that is always opaque.
fn grey_of(pixels: &[u8]) -> Vec<u8> {
    pixels.chunks_exact(4).map(|pixel| pixel[1]).collect()
}

fn announce_profile(profile: Option<&DeviceProfile>) -> Option<&DeviceProfile> {
    if let Some(profile) = profile {
        println!("profile: {} ({})", profile.id, profile.model);
    } else {
        println!("profile: unsupported");
    }
    profile
}

fn require_profile(
    profile: Option<&'static DeviceProfile>,
) -> Result<&'static DeviceProfile, ExitCode> {
    profile.ok_or_else(|| {
        println!("result: rejected");
        println!("write blocker: no supported hardware profile matched this device");
        ExitCode::from(2)
    })
}

fn main() -> ExitCode {
    if std::env::var_os("KOBO_DOCTOR_JSON").is_some() {
        return match json_probe() {
            Ok(json) => {
                println!("{json}");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("probe failed: {error}");
                ExitCode::FAILURE
            }
        };
    }
    human_probe()
}

fn human_probe() -> ExitCode {
    println!("Kobo doctor 0.1.0");
    println!("mode: read-only (query ioctls and hwconfig only)");

    for input in kobo_hal::probe::input_inventory() {
        println!("input evidence: {input:?}");
    }

    let snapshot = match probe_device() {
        Ok(snapshot) => snapshot,
        Err(error) => {
            eprintln!("probe failed: {error}");
            return ExitCode::FAILURE;
        }
    };

    let matched_profile = announce_profile(identify_profile(&snapshot));

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
    announce_identity(&snapshot.identity);
    announce_hwconfig_revision();
    if let Some(touch) = &snapshot.touch {
        println!(
            "touch: {} at {} X={}..{} Y={}..{}",
            touch.name, touch.path, touch.x_min, touch.x_max, touch.y_min, touch.y_max
        );
    }
    print_tailscale_readiness();

    let matched_profile = match require_profile(matched_profile) {
        Ok(profile) => profile,
        Err(exit) => return exit,
    };

    let report = matched_profile.validate(&snapshot);
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
    // interpreted with a transform that does not belong to this hardware. The
    // pose is resolved here too, and loudly, because the transform is only
    // correct at the orientation the reader is actually in.
    if let Some(request) = std::env::var_os(OBSERVE_TOUCH_VARIABLE) {
        let touch_path = snapshot.touch.as_ref().map(|touch| touch.path.clone());
        let request = request.to_string_lossy();
        if let Err(error) = observe(matched_profile, &snapshot, touch_path.as_deref(), &request) {
            eprintln!("touch observation failed: {error}");
            return ExitCode::FAILURE;
        }
    }

    if let Some(request) = std::env::var_os(RECORD_VARIABLE) {
        let Some(framebuffer) = snapshot.framebuffer.as_ref() else {
            eprintln!("record failed: no framebuffer was discovered");
            return ExitCode::FAILURE;
        };
        if let Err(error) = record(framebuffer, &request.to_string_lossy()) {
            eprintln!("record failed: {error}");
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

fn json_probe() -> Result<String, String> {
    let snapshot = probe_device().map_err(|error| error.to_string())?;
    let captured_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_secs();
    let mut observation = kobo_profile::observation::Observation::probe(snapshot, captured_at);
    observation.input_devices = kobo_hal::probe::input_inventory();
    observation.to_json()
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

/// Copies the panel repeatedly into a file, skipping frames that did not move.
///
/// Every pixel is kept. The panel is greyscale and the framebuffer is 32-bit,
/// so one byte per pixel is already a four-fold saving with nothing lost;
/// going further, to one bit, would throw away the anti-aliasing on every
/// glyph and the dithering on every cover, and a recording made to check how
/// something looks must not be the thing that changed how it looks.
///
/// What makes it small instead is that e-ink barely moves. A minute of a
/// reader looking at a page is one frame, not sixty, so identical frames are
/// dropped and only the changes are stored.
fn record(framebuffer: &FramebufferSnapshot, request: &str) -> Result<(), String> {
    let (seconds, fps) = parse_record_request(request)?;
    let geometry = SurfaceGeometry {
        width: framebuffer.width,
        height: framebuffer.height,
        stride: framebuffer.stride,
        bits_per_pixel: framebuffer.bits_per_pixel,
        memory_length: u64::from(framebuffer.memory_length),
    };
    let whole = Rect {
        x: 0,
        y: 0,
        width: framebuffer.width,
        height: framebuffer.height,
    };
    let path =
        std::env::var(RECORD_PATH_VARIABLE).unwrap_or_else(|_| DEFAULT_RECORD_PATH.to_owned());
    let file = OpenOptions::new()
        .read(true)
        .open("/dev/fb0")
        .map_err(|error| format!("open /dev/fb0 for reading: {error}"))?;
    let mut out = std::io::BufWriter::new(
        std::fs::File::create(&path).map_err(|error| format!("create {path}: {error}"))?,
    );
    out.write_all(RECORD_MAGIC)
        .and_then(|()| out.write_all(&framebuffer.width.to_le_bytes()))
        .and_then(|()| out.write_all(&framebuffer.height.to_le_bytes()))
        .map_err(|error| format!("write {path}: {error}"))?;

    // The shape of the panel as it was when this started. Something else can
    // reconfigure the framebuffer underneath a recording -- an application
    // taking the panel does exactly that -- and reading on at offsets computed
    // from the old shape would be reading the wrong memory, at speed, in a
    // loop. So it is checked every frame and the recording stops rather than
    // carries on against a panel that is no longer the one it measured.
    let shape = panel_shape();

    let interval = Duration::from_millis(1000 / u64::from(fps));
    let started = std::time::Instant::now();
    let deadline = Duration::from_secs(seconds);
    let mut previous: Option<Vec<u8>> = None;
    let mut kept = 0_u32;
    let mut looked = 0_u32;
    while started.elapsed() < deadline {
        let sampled = std::time::Instant::now();
        if panel_shape() != shape {
            eprintln!("the panel changed shape mid-recording; stopping here");
            break;
        }
        let pixels = read_whole_panel(&file, geometry, whole)
            .map_err(|error| format!("read the panel: {error}"))?;
        let grey = grey_of(&pixels);
        looked += 1;
        if previous.as_deref() != Some(grey.as_slice()) {
            let millis = u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX);
            out.write_all(&millis.to_le_bytes())
                .and_then(|()| out.write_all(&grey))
                .map_err(|error| format!("write {path}: {error}"))?;
            kept += 1;
            previous = Some(grey);
        }
        // Measured from the start of the read, so a slow read shortens the
        // wait rather than adding to it and the recording keeps its rate.
        if let Some(rest) = interval.checked_sub(sampled.elapsed()) {
            std::thread::sleep(rest);
        }
    }
    out.flush()
        .map_err(|error| format!("finish writing {path}: {error}"))?;
    println!(
        "record-written {path} {kept} {looked} {} {}",
        framebuffer.width, framebuffer.height
    );
    Ok(())
}

/// Copies the whole panel, in one read where the rows are contiguous.
///
/// On this panel they are: the stride is exactly the width in bytes, so the
/// visible pixels are one unbroken run. Reading them row by row, as the
/// general path must, costs 1448 reads from the display device every frame,
/// and a recording asks for that twice a second while an application is
/// drawing to the same device. One read of the same bytes is the same picture
/// with a thousandth of the traffic.
///
/// Falls back to the row-by-row path whenever the rows are not contiguous,
/// which is the only case that path exists for.
fn read_whole_panel(
    file: &std::fs::File,
    geometry: SurfaceGeometry,
    whole: Rect,
) -> Result<Vec<u8>, String> {
    let bytes_per_pixel = (geometry.bits_per_pixel / 8) as usize;
    let row_bytes = (geometry.width as usize).saturating_mul(bytes_per_pixel);
    let total = row_bytes.saturating_mul(geometry.height as usize);
    if bytes_per_pixel > 0
        && total > 0
        && u64::from(geometry.stride) == row_bytes as u64
        && geometry.memory_length >= total as u64
    {
        let mut pixels = vec![0_u8; total];
        read_exact_at(file, &mut pixels, 0).map_err(|error| format!("{error}"))?;
        return Ok(pixels);
    }
    read_region(file, geometry, whole)
        .map(|snapshot| snapshot.pixels().to_vec())
        .map_err(|error| format!("{error}"))
}

/// The panel's shape as the kernel currently reports it.
///
/// Read from sysfs rather than by ioctl because it costs two small file reads
/// and needs nothing held open, which matters when the question is being asked
/// on every frame.
fn panel_shape() -> Option<(String, String)> {
    let size = std::fs::read_to_string("/sys/class/graphics/fb0/virtual_size").ok()?;
    let depth = std::fs::read_to_string("/sys/class/graphics/fb0/bits_per_pixel").ok()?;
    Some((size.trim().to_owned(), depth.trim().to_owned()))
}

/// Reads `seconds:fps`, refusing anything that would run away with the device.
fn parse_record_request(request: &str) -> Result<(u64, u32), String> {
    let (seconds, fps) = request
        .split_once(':')
        .ok_or_else(|| format!("expected seconds:fps, got {request:?}"))?;
    let seconds: u64 = seconds
        .parse()
        .map_err(|_| format!("{seconds:?} is not a number of seconds"))?;
    let fps: u32 = fps.parse().map_err(|_| format!("{fps:?} is not a rate"))?;
    if seconds == 0 || seconds > MAXIMUM_RECORD_SECONDS {
        return Err(format!(
            "a recording runs between 1 and {MAXIMUM_RECORD_SECONDS} seconds, not {seconds}"
        ));
    }
    if fps == 0 || fps > MAXIMUM_RECORD_FPS {
        return Err(format!(
            "a recording samples between 1 and {MAXIMUM_RECORD_FPS} times a second, not {fps}"
        ));
    }
    Ok((seconds, fps))
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

/// The profile paired with the orientation the reader is actually in.
///
/// Separate from `main` only to keep it short. The refusal matters more than
/// the brevity: a touch transform is correct at one orientation, so observing
/// with an unresolved pose would report coordinates nobody can trust.
fn resolve_pose<'a>(
    profile: &'a DeviceProfile,
    snapshot: &DeviceSnapshot,
) -> Result<PanelPose<'a>, PoseError> {
    snapshot
        .framebuffer
        .as_ref()
        .ok_or(PoseError::FramebufferMissing)
        .and_then(|framebuffer| PanelPose::resolve(profile, framebuffer))
}

fn observe(
    profile: &DeviceProfile,
    snapshot: &DeviceSnapshot,
    touch_path: Option<&str>,
    request: &str,
) -> Result<(), String> {
    // Resolved before anything is read, and loudly. A touch transform is
    // correct at one orientation, so observing without a resolved pose would
    // print coordinates nobody can trust.
    let pose = &resolve_pose(profile, snapshot).map_err(|error| error.to_string())?;
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
    // This line used to be hardcoded to one device's transform and printed
    // regardless of the profile, so it described a mapping the code did not
    // use on every device but the Clara BW. It now names what will actually
    // run, including the orientation, since the transform is only right at one.
    println!(
        "touch mapping under test: {:?} at rotation {}",
        pose.touch_mapping(),
        pose.rotation()
    );
    let reported = observe_touch(
        Path::new(path),
        pose,
        Duration::from_secs(seconds),
        |observation| println!("touch: {observation}"),
    )
    .map_err(|error| error.to_string())?;
    println!("touch observation complete: {reported} event(s)");
    Ok(())
}

/// What can be read, without touching anything, about whether Tailscale
/// could run on this reader.
///
/// Every line is a read of a file the kernel publishes or of a path's
/// existence; nothing is created, opened for writing or brought up. That
/// limits what this can claim, and each line says which side of the limit it
/// is on. Whether the kernel has a tunnel driver is only half answered here:
/// a driver that is registered is certain, one that is not may still be a
/// module the kernel loads when the device node is first opened, and only
/// the supervisor in `crates/kobod/src/tailscale.rs` opens it. The device node
/// existing says nothing either way, since anybody can make one. What is
/// listening is the other thing worth reading before a tailnet is joined,
/// because the supervisor refuses to start while telnet or FTP are up or
/// while root can still log in over SSH with a password.
fn print_tailscale_readiness() {
    println!("tailscale readiness (read-only):");
    let misc = std::fs::read_to_string("/proc/misc").ok();
    println!(
        "  kernel tunnel driver: {}",
        match misc.as_deref().map(tunnel_driver_registered) {
            Some(true) => "registered",
            Some(false) =>
                "not registered (it may still load when the node is opened; starting Tailscale finds out)",
            None => "unknown (/proc/misc cannot be read)",
        }
    );
    println!(
        "  tunnel device node: {}",
        if Path::new("/dev/net/tun").exists() {
            "present (proves nothing about the driver)"
        } else {
            "absent (made on every start; /dev is rebuilt at boot)"
        }
    );
    let flags = std::fs::read_to_string("/sys/class/net/lo/flags").unwrap_or_default();
    println!(
        "  loopback: {}",
        match interface_is_up(&flags) {
            Some(true) => "up",
            Some(false) => "down (brought up on start)",
            None => "unknown",
        }
    );
    let listening = ["/proc/net/tcp", "/proc/net/tcp6"]
        .iter()
        .filter_map(|table| std::fs::read_to_string(table).ok())
        .flat_map(|table| listening_ports(&table))
        .collect::<Vec<_>>();
    for (port, name, consequence) in [
        (
            22,
            "ssh",
            "must refuse root a password before Tailscale starts",
        ),
        (23, "telnet", "Tailscale will not start while this is up"),
        (21, "ftp", "Tailscale will not start while this is up"),
    ] {
        if listening.contains(&port) {
            println!("  listening: {name} on {port} ({consequence})");
        }
    }
    let meminfo = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    for key in ["MemTotal", "MemAvailable"] {
        if let Some(line) = meminfo.lines().find(|line| line.starts_with(key)) {
            println!("  {line}");
        }
    }
    println!(
        "  packet filter tools: {}",
        if ["/sbin/iptables", "/usr/sbin/iptables"]
            .iter()
            .any(|path| Path::new(path).exists())
        {
            "present (unexpected; the daemon is still told to leave the firewall alone)"
        } else {
            "absent (expected; every listening port is reachable from a tailnet)"
        }
    );
    print_kernel_routing_support();
    let mounts = std::fs::read_to_string("/proc/mounts").unwrap_or_default();
    match mounts
        .lines()
        .find(|line| line.split_whitespace().nth(1) == Some("/mnt/onboard"))
    {
        Some(line) => println!("  /mnt/onboard: {line}"),
        None => println!("  /mnt/onboard: not mounted"),
    }
}

/// What the kernel offers for carrying other devices' traffic, the way a
/// Linux exit node or subnet router ordinarily does it.
///
/// The Tailscale service does not use any of this. It has the daemon carry
/// that traffic itself, exactly because nothing was known about these on any
/// reader. They are reported so that the next person knows: a reader whose
/// kernel has a packet filter with address translation could let the kernel
/// do the forwarding, which is faster than a daemon on one core.
///
/// Each line is as far as a read can go. The forwarding switch is a fact.
/// Connection tracking and the older tables are facts when present. The
/// newer packet filter has no file that announces it, so its line comes from
/// the kernel's own list of symbols: finding them proves the code is in the
/// running kernel, and not finding them does not prove a module could not
/// be loaded. Whether any of it works is something only using it shows.
fn print_kernel_routing_support() {
    let forwarding = std::fs::read_to_string("/proc/sys/net/ipv4/ip_forward");
    println!(
        "  kernel forwarding switch: {}",
        match forwarding.as_deref().map(str::trim) {
            Ok("0") => "off (Tailscale here does not need it on)",
            Ok(_) => "on",
            Err(_) => "unreadable",
        }
    );
    println!(
        "  connection tracking: {}",
        if Path::new("/proc/sys/net/netfilter/nf_conntrack_max").exists() {
            "present"
        } else {
            "not present"
        }
    );
    let tables = std::fs::read_to_string("/proc/net/ip_tables_names").unwrap_or_default();
    let tables = tables.split_whitespace().collect::<Vec<_>>();
    println!(
        "  older packet filter tables: {}",
        if tables.is_empty() {
            "none".to_owned()
        } else {
            tables.join(", ")
        }
    );
    match std::fs::read_to_string("/proc/kallsyms") {
        Ok(symbols) => {
            let found = packet_filter_in(&symbols);
            println!(
                "  newer packet filter in the running kernel: {}",
                if found.is_empty() {
                    "no symbols found (a module on disk would not show here)".to_owned()
                } else {
                    found.join(", ")
                }
            );
        }
        Err(_) => println!(
            "  newer packet filter in the running kernel: unknown (/proc/kallsyms cannot be read)"
        ),
    }
}

/// Which parts of the newer packet filter the kernel's symbol list shows:
/// the filter itself, address translation, and the rule that rewrites a
/// packet's source to the reader's own address.
fn packet_filter_in(symbols: &str) -> Vec<&'static str> {
    let has = |prefix: &str| {
        symbols.lines().any(|line| {
            line.split_whitespace()
                .nth(2)
                .is_some_and(|name| name.starts_with(prefix))
        })
    };
    [
        ("nf_tables_", "nf_tables"),
        ("nft_nat", "address translation"),
        ("nft_masq", "masquerade"),
        ("nft_ct", "connection tracking rules"),
    ]
    .into_iter()
    .filter(|(prefix, _)| has(prefix))
    .map(|(_, name)| name)
    .collect()
}

/// Whether the kernel's list of miscellaneous devices names the tunnel
/// driver, which registers as `tun` whatever its minor number.
fn tunnel_driver_registered(misc: &str) -> bool {
    misc.lines()
        .any(|line| line.split_whitespace().nth(1) == Some("tun"))
}

/// Whether an interface's flags say it is up.
///
/// The flags and not `operstate`, which reads `unknown` on a loopback that is
/// working, so a report built on it said the same thing on every reader.
fn interface_is_up(flags: &str) -> Option<bool> {
    u32::from_str_radix(flags.trim().trim_start_matches("0x"), 16)
        .ok()
        .map(|flags| flags & 1 == 1)
}

/// The TCP ports something is listening on at an address other machines can
/// reach, from the kernel's socket table.
fn listening_ports(table: &str) -> Vec<u16> {
    table
        .lines()
        .skip(1)
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let (address, port) = fields.nth(1)?.rsplit_once(':')?;
            let listening = fields.nth(1)? == "0A";
            let loopback = matches!(address, "0100007F" | "00000000000000000000000001000000");
            (listening && !loopback)
                .then(|| u16::from_str_radix(port, 16).ok())
                .flatten()
        })
        .collect()
}

/// Positional read: `FileExt` on Unix, the `seek_read` equivalent on Windows.
/// Doctor is a device tool, so the Windows arm exists to keep the workspace
/// compiling there; it is never exercised on a real panel.
#[cfg(unix)]
fn read_exact_at(file: &std::fs::File, buffer: &mut [u8], offset: u64) -> std::io::Result<()> {
    use std::os::unix::fs::FileExt as _;
    file.read_exact_at(buffer, offset)
}

#[cfg(windows)]
fn read_exact_at(file: &std::fs::File, buffer: &mut [u8], offset: u64) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt as _;
    let mut filled = 0;
    while filled < buffer.len() {
        let read = file.seek_read(&mut buffer[filled..], offset + filled as u64)?;
        if read == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "positional framebuffer read ended early",
            ));
        }
        filled += read;
    }
    Ok(())
}

/// Prints the identity that gates every write.
///
/// The full serial is deliberately never read past its four-character model
/// prefix.
fn announce_identity(identity: &kobo_profile::IdentitySnapshot) {
    println!(
        "identity: model={} firmware={} kernel={} device-code={}",
        identity.serial_prefix.as_deref().unwrap_or("<unknown>"),
        identity.firmware_version.as_deref().unwrap_or("<unknown>"),
        identity.kernel_release.as_deref().unwrap_or("<unknown>"),
        identity
            .device_code
            .map_or_else(|| "<unknown>".to_owned(), |code| code.to_string()),
    );
}

/// Prints the hardware-revision fields that can vary under one device code.
///
/// The Nia ships with either an RC5T619 or a BD71828 PMIC under device code
/// 382, and nothing in the identity line above tells them apart. `ntx_hwconfig
/// -s` is Kobo's read-only listing mode; keeping its exact field names and
/// values in the transcript lets a reviewer see which revision supplied the
/// evidence instead of assuming it was the one already measured.
fn announce_hwconfig_revision() {
    let fields = Command::new("/bin/ntx_hwconfig")
        .args(["-s", "/dev/mmcblk0"])
        .output()
        .ok()
        .map(|output| hwconfig_revision_fields(&String::from_utf8_lossy(&output.stdout)).join(", "))
        .filter(|fields| !fields.is_empty());
    println!(
        "hwconfig revision: {}",
        fields.as_deref().unwrap_or("unavailable")
    );
}

fn hwconfig_revision_fields(output: &str) -> Vec<&str> {
    const NAMES: [&str; 5] = ["PCB", "PCB_REV", "PCB_LVL", "PMIC", "FL_PWM"];
    output
        .lines()
        .filter_map(|line| line.split_once("] ").map(|(_, field)| field.trim()))
        .filter(|field| {
            field
                .split_once('=')
                .is_some_and(|(name, _)| NAMES.contains(&name))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_tunnel_driver_is_found_by_name_in_the_kernels_device_list() {
        assert!(super::tunnel_driver_registered(
            " 59 memory_bandwidth\n200 tun\n130 watchdog\n"
        ));
        // A name that only contains it, and a list without it.
        assert!(!super::tunnel_driver_registered(
            " 62 tunnel4\n130 watchdog\n"
        ));
        assert!(!super::tunnel_driver_registered(""));
    }

    #[test]
    fn the_newer_packet_filter_is_reported_part_by_part_from_the_kernels_symbols() {
        let symbols = "c0a1b2c0 T nf_tables_newtable\nc0a1b300 t nft_masq_ipv4_eval\tnft_masq_ipv4\nc0a1b400 t nft_nat_eval\nc0a1b500 T tun_get_socket\n";
        assert_eq!(
            super::packet_filter_in(symbols),
            ["nf_tables", "address translation", "masquerade"]
        );
        // A kernel with none of it, and a name that only contains one.
        assert!(super::packet_filter_in(
            "c0a1b500 T tun_get_socket\nc0a1b600 T my_nf_tables_helper\n"
        )
        .is_empty());
        assert!(super::packet_filter_in("").is_empty());
    }

    #[test]
    fn loopback_is_read_from_its_flags_not_its_operstate() {
        assert_eq!(super::interface_is_up("0x49\n"), Some(true));
        assert_eq!(super::interface_is_up("0x8\n"), Some(false));
        assert_eq!(super::interface_is_up("unknown\n"), None);
    }

    #[test]
    fn listening_ports_leave_out_loopback_and_open_connections() {
        let table = "  sl  local_address rem_address   st\n   0: 00000000:0016 00000000:0000 0A\n   1: 0100007F:20E0 00000000:0000 0A\n   2: 0A00000C:0016 0A000002:C1A2 01\n";
        assert_eq!(super::listening_ports(table), [22]);
    }

    #[test]
    fn a_recording_request_is_read_as_seconds_and_a_rate() {
        assert_eq!(super::parse_record_request("20:2"), Ok((20, 2)));
    }

    #[test]
    fn a_recording_that_would_never_stop_is_refused() {
        // A tool that watches the device has to stop on its own. This one runs
        // with the reader unattended, so an unbounded loop would flatten a
        // battery and hold the memory bus against the reader.
        assert!(super::parse_record_request("0:2").is_err());
        assert!(super::parse_record_request("100000:2").is_err());
        assert!(super::parse_record_request("20:0").is_err());
        assert!(super::parse_record_request("20:60").is_err());
        assert!(super::parse_record_request("20").is_err());
        assert!(super::parse_record_request("soon:2").is_err());
    }

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

    #[test]
    fn hwconfig_revision_keeps_the_tools_exact_field_names_and_values() {
        let fixture = "HW CONFIG v3.3 @1024 secno, size=72 bytes :\n\
[0] PCB='E60U20'\n\
[40] VCOM_10mV_LoByte=0x2A\n\
[41] PCB_REV=0x10\n\
[42] PCB_LVL='A'\n\
[44] PMIC='RC5T619'\n\
[45] FL_PWM='LM3630x1a'\n";
        assert_eq!(
            super::hwconfig_revision_fields(fixture),
            [
                "PCB='E60U20'",
                "PCB_REV=0x10",
                "PCB_LVL='A'",
                "PMIC='RC5T619'",
                "FL_PWM='LM3630x1a'",
            ]
        );
    }
}

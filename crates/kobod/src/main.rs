use kobo_policy::{DeviceServices, TaskRunner};

use kobo_protocol::{Frame, LogLevel, Message, TaskError, TaskOutcome};
use kobo_ui::{display_metrics_from_env, Screen, Surface};
use std::env;
use std::error::Error;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static VERIFIED_DEVICE_METRICS: OnceLock<kobo_ui::DisplayMetrics> = OnceLock::new();

fn metrics_for_profile(
    profile: &kobo_profile::DeviceProfile,
    mut metrics: kobo_ui::DisplayMetrics,
) -> Result<kobo_ui::DisplayMetrics, String> {
    metrics.width = i32::try_from(profile.width).map_err(|_| {
        format!(
            "profile {} width does not fit the layout engine",
            profile.id
        )
    })?;
    metrics.height = i32::try_from(profile.height).map_err(|_| {
        format!(
            "profile {} height does not fit the layout engine",
            profile.id
        )
    })?;
    metrics.pixels_per_inch = i32::from(profile.pixels_per_inch);
    Ok(metrics)
}

/// Returns immutable hardware metrics after a verified device session starts,
/// or the explicit host-simulation metrics before then.
pub fn device_metrics() -> kobo_ui::DisplayMetrics {
    VERIFIED_DEVICE_METRICS
        .get()
        .copied()
        .unwrap_or_else(display_metrics_from_env)
}

#[cfg(feature = "device-write")]
fn remember_device_profile(profile: &kobo_profile::DeviceProfile) -> Result<(), String> {
    let metrics = metrics_for_profile(profile, display_metrics_from_env())?;
    if let Some(remembered) = VERIFIED_DEVICE_METRICS.get() {
        if *remembered == metrics {
            return Ok(());
        }
        return Err("the verified device profile changed during this runtime".to_owned());
    }
    VERIFIED_DEVICE_METRICS
        .set(metrics)
        .map_err(|_| "the verified device metrics could not be retained".to_owned())
}

use std::process::ExitCode;

mod app_link;
mod app_store;
mod autoupdate;
#[cfg(feature = "device-write")]
mod blackbox;
#[cfg(feature = "device-write")]
mod device;
mod frame;
mod update;

fn main() -> ExitCode {
    let arguments = env::args().skip(1).collect::<Vec<_>>();
    match run(&arguments) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("kobod: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(arguments: &[String]) -> Result<(), Box<dyn Error>> {
    if arguments.is_empty() {
        print_safety_state();
        return Ok(());
    }
    if arguments.len() == 4 && arguments[0] == "--sim-socket" && arguments[2] == "--frame" {
        return serve_simulation(Path::new(&arguments[1]), Path::new(&arguments[3]));
    }
    #[cfg(feature = "device-write")]
    if arguments.len() == 2 && arguments[0] == "--present" {
        return present_on_panel(Path::new(&arguments[1]));
    }
    // The watchdog calls this after a session that never cleaned up. It only
    // ever starts the reader, so it is not gated behind the unlock phrase.
    #[cfg(feature = "device-write")]
    if arguments.len() == 2 && arguments[0] == "--restart-from" {
        return restart_reader(Path::new(&arguments[1]));
    }
    // Grabs the touch panel and reports what arrives, without stopping the
    // reader or touching the display. The kernel drops an EVIOCGRAB when the
    // holder dies, so the only lasting effect is that the reader sees no touch
    // for the duration.
    #[cfg(feature = "device-write")]
    if arguments.len() == 2 && arguments[0] == "--touch-test" {
        return touch_test(&arguments[1]);
    }
    // Reads the physical buttons and reports what arrives. This takes no
    // grab: the reader keeps seeing every press, so pages may turn in Nickel
    // underneath and the power button keeps its normal meaning. Purely a
    // capture, safe on a device in normal use.
    if arguments.len() == 2 && arguments[0] == "--key-test" {
        return key_test(&arguments[1]);
    }
    // Performs one real request and reports what happened. This touches no
    // hardware and does not go near the reader, so it is safe to run on a
    // device in normal use, which is the point: the network path has to be
    // provable without a handoff.
    if arguments.len() == 3 && arguments[0] == "--fetch" {
        return fetch_once(&arguments[1], &arguments[2]);
    }
    if arguments.len() == 2 && arguments[0] == "--app-link" {
        println!(
            "{}",
            app_link::maintenance(Path::new("/mnt/onboard/.adds/cobalt"), &arguments[1])
                .map_err(|error| format!("app link: {error}"))?
        );
        return Ok(());
    }
    Err("usage: kobod [--sim-socket PATH --frame PATH] [--present APP] [--fetch URL BYTES] [--key-test SECONDS] [--app-link status|unpair]".into())
}

#[cfg(feature = "device-write")]
fn touch_test(seconds: &str) -> Result<(), Box<dyn Error>> {
    use kobo_hal::input::TouchSession;
    use std::time::{Duration, Instant};

    let seconds: u64 = seconds.parse().unwrap_or(20).min(120);

    let snapshot = kobo_hal::probe_device()?;
    let profile = kobo_profile::write_ready_profile(&snapshot)
        .map_err(|blockers| format!("device write refused: {}", blockers.join("; ")))?;
    let touch_path = snapshot
        .touch
        .as_ref()
        .map(|t| t.path.clone())
        .ok_or_else(|| "touch probe was unavailable".to_owned())?;

    println!("touch device: {touch_path}");
    let framebuffer = snapshot
        .framebuffer
        .as_ref()
        .ok_or_else(|| "framebuffer probe was unavailable".to_owned())?;
    let pose = kobo_profile::PanelPose::resolve(profile, framebuffer)
        .map_err(|error| format!("touch refused: {error}"))?;
    let mut session = TouchSession::acquire(Path::new(&touch_path), pose)?;
    println!("grabbed; touch the panel");
    let events = session
        .take_events()
        .ok_or("the touch session produced no event channel")?;
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut count = 0_u32;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match events.recv_timeout(remaining) {
            Ok(event) => {
                count += 1;
                println!("event {count}: {event:?}");
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                println!("the reader thread stopped after {count} events");
                break;
            }
        }
    }
    session.release()?;
    println!("released after {count} events");
    Ok(())
}

/// Reads the button device without grabbing it and prints every event.
///
/// The point is to learn the keycodes the page-turn and power buttons emit
/// on hardware nobody has captured yet, at both poses. Numeric codes are
/// printed verbatim so nothing is lost to an incomplete name table.
fn key_test(seconds: &str) -> Result<(), Box<dyn Error>> {
    use kobo_hal::touch::InputEvent32;
    use std::io::Read;
    use std::time::{Duration, Instant};

    let seconds: u64 = seconds.parse().unwrap_or(20).min(120);

    let content = std::fs::read_to_string("/proc/bus/input/devices")?;
    let path = discover_key_path(&content)
        .ok_or_else(|| "no gpio-keys device found in /proc/bus/input/devices".to_owned())?;
    let mut device = std::fs::File::open(&path)?;
    let name = kobo_abi::input::device_name(&device)?;
    println!("button device: {} ({name})", path.display());
    println!("reading for {seconds}s, no grab; press each button, at both poses");

    let deadline = Instant::now() + Duration::from_secs(seconds);
    let started = Instant::now();
    let mut buffer = [0_u8; 16 * 64];
    let mut count = 0_u32;
    while Instant::now() < deadline {
        // A blocking read sits here until a press arrives; the deadline is
        // only checked between reads, so the capture ends on the first
        // event after time is up, or at the final press of Enter... it does
        // not, so the run simply overshoots by however long the last quiet
        // stretch lasts. Acceptable for an owner-attended probe.
        let read = device.read(&mut buffer)?;
        for chunk in buffer[..read].chunks_exact(16) {
            let Some(event) = InputEvent32::decode(chunk) else {
                continue;
            };
            count += 1;
            let at = started.elapsed().as_millis();
            let kind = match event.kind {
                0 => "SYN",
                1 => "KEY",
                _ => "???",
            };
            let action = match (event.kind, event.value) {
                (1, 0) => " up",
                (1, 1) => " down",
                (1, 2) => " repeat",
                _ => "",
            };
            println!(
                "{at:>7} ms  {kind} code={} value={}{action}",
                event.code, event.value
            );
        }
    }
    println!("captured {count} events");
    Ok(())
}

/// Finds the event node for the `gpio-keys` device.
fn discover_key_path(content: &str) -> Option<std::path::PathBuf> {
    content.split("\n\n").find_map(|block| {
        let name_matches = block
            .lines()
            .find(|line| line.starts_with("N: Name="))
            .is_some_and(|line| line.contains("gpio-keys"));
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
        Some(std::path::Path::new("/dev/input").join(event))
    })
}

/// Fetches one URL and prints a one line verdict.
fn fetch_once(url: &str, max_bytes: &str) -> Result<(), Box<dyn Error>> {
    let ceiling: u32 = max_bytes
        .parse()
        .map_err(|_| format!("byte ceiling must be a number, not {max_bytes:?}"))?;
    let started = std::time::Instant::now();
    match kobo_net::fetch(url, ceiling) {
        Ok(body) => {
            println!(
                "ok {url} -> {} bytes in {} ms",
                body.len(),
                started.elapsed().as_millis()
            );
            Ok(())
        }
        Err(error) => Err(format!("{url} -> {error}").into()),
    }
}

/// Stops the stock reader, gives the panel to one application, and puts
/// everything back afterwards.
#[cfg(feature = "device-write")]
fn present_on_panel(application: &Path) -> Result<(), Box<dyn Error>> {
    use std::time::Duration;
    const UNLOCK_ENV: &str = "KOBO_PRESENT_UNLOCK";
    const UNLOCK_PHRASE: &str = "OWNER_ATTENDED_PANEL_SESSION";
    if env::var(UNLOCK_ENV).ok().as_deref() != Some(UNLOCK_PHRASE) {
        return Err("owner-attended panel session unlock is missing or incorrect".into());
    }
    // The session used to end on a timer, which took the panel away from
    // somebody in the middle of using it. It now ends when the reader taps the
    // way out, or when the device has been left alone; the environment is only
    // an escape hatch for testing, and both values are clamped inside.
    let limits = device::Limits {
        idle: env::var("KOBO_IDLE_SECONDS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .map_or(device::Limits::default().idle, Duration::from_secs),
        ceiling: env::var("KOBO_SESSION_SECONDS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .map_or(device::Limits::default().ceiling, Duration::from_secs),
    };
    println!("{}", device::present(application, limits)?);
    Ok(())
}

/// Brings the stock reader back from a saved description.
#[cfg(feature = "device-write")]
fn restart_reader(state: &Path) -> Result<(), Box<dyn Error>> {
    use kobo_hal::reader::Reader;
    if Reader::find().is_ok() {
        println!("the reader is already running; nothing to do");
        return Ok(());
    }
    let reader = Reader::load(state)?;
    // The restart below is the operation that trips the SoC watchdog, and this
    // process is here precisely because the session that should have been
    // holding it off is dead. Slack first, then arm unconditionally once the
    // reader is back, which also repays the slack the dead session left.
    let watchdog = kobo_hal::soc_watchdog::SocWatchdog::default();
    let slack = watchdog.slacken();
    if let Err(error) = &slack {
        println!("the hardware watchdog could not be slackened ({error}); restarting anyway");
    }
    let pid = reader.start(std::time::Duration::from_secs(45))?;
    drop(slack);
    match watchdog.arm() {
        Ok(()) => {}
        Err(error) => println!(
            "the hardware watchdog could not be armed ({error}); it returns on the next reboot"
        ),
    }
    // A session that died without cleaning up also left the freeze watchdog
    // suspended. Putting it back is part of recovery, not an afterthought.
    match kobo_hal::supervisor::resume_with(reader.environment("DBUS_SESSION_BUS_ADDRESS")) {
        Ok(()) => println!("reader restarted as pid {pid}; freeze watchdog resumed"),
        Err(error) => println!(
            "reader restarted as pid {pid}, but the freeze watchdog could not be resumed ({error}); it returns on the next reboot"
        ),
    }
    println!("{}", clear_session_files(state));
    Ok(())
}

/// Removes what a session that died without cleaning up left behind.
///
/// Recovery is not finished when the reader is running again; it is finished
/// when the device looks like nothing happened. A killed session leaves its
/// state directory, its heartbeat and its socket in `/tmp`, and while tmpfs
/// clears them at the next boot, "no leftovers in `/tmp`" is one of the four
/// things checked after every session on this device, and a leftover heartbeat
/// is indistinguishable from a session still in progress.
///
/// Only paths derived from the directory the caller named are touched, and
/// only after the reader is confirmed running, so a failed restart leaves
/// everything in place for the next attempt.
/// Only reachable through the reader restart today, which is a device build,
/// but its tests run everywhere, so a default build compiles it unused.
#[cfg_attr(not(feature = "device-write"), allow(dead_code))]
fn clear_session_files(state: &Path) -> String {
    let mut removed = Vec::new();
    let mut failed = Vec::new();
    let mut discard = |path: PathBuf, directory: bool| {
        if !path.exists() {
            return;
        }
        let outcome = if directory {
            fs::remove_dir_all(&path)
        } else {
            fs::remove_file(&path)
        };
        match outcome {
            Ok(()) => removed.push(path),
            Err(error) => failed.push(format!("{}: {error}", path.display())),
        }
    };
    for suffix in ["beat", "sock", "cancel"] {
        let mut sidecar = state.as_os_str().to_owned();
        sidecar.push(format!(".{suffix}"));
        discard(PathBuf::from(sidecar), false);
    }
    discard(state.to_path_buf(), true);
    if failed.is_empty() {
        format!("cleared {} leftover session files", removed.len())
    } else {
        format!(
            "cleared {} leftover session files, but {} could not be removed ({}); \
             they are in tmpfs and go at the next reboot",
            removed.len(),
            failed.len(),
            failed.join(", ")
        )
    }
}

fn print_safety_state() {
    let write_unlocked = env::var_os("KOBO_DEVICE_WRITE_UNLOCK").is_some();
    println!("kobod {}", env!("CARGO_PKG_VERSION"));

    let profile_id = kobo_hal::probe_device()
        .ok()
        .and_then(|snapshot| kobo_profile::identify_profile(&snapshot))
        .map_or("unknown", |profile| profile.id);

    println!("profile: {profile_id}");
    println!("device-write compiled: {}", cfg!(feature = "device-write"));
    println!("device-write unlocked: {write_unlocked}");
    println!(
        "hardware ownership: {}",
        if cfg!(feature = "device-write") {
            "available with --present and the session unlock"
        } else {
            "disabled"
        }
    );
    if write_unlocked {
        eprintln!(
            "hardware writes remain blocked until physical recovery and smoke-test gates pass"
        );
    }
}

fn serve_simulation(socket_path: &Path, frame_path: &Path) -> Result<(), Box<dyn Error>> {
    validate_simulation_paths(socket_path, frame_path)?;
    // Without this the preview renders in the built-in bitmap fallback, which
    // is uppercase-only and fixed width, so every line break, page count and
    // paragraph height in the picture belongs to a panel nobody has. The
    // preview exists to be looked at; a preview drawn in the wrong face is
    // worse than none, because it is believed.
    let metrics = crate::device_metrics();
    let _ = kobo_text::install(metrics);
    // The same owner trust roots the device loads, from the host's own
    // directory, so an application developed against a local daemon works
    // here before it is ever staged on a reader.
    let trusted = kobo_net::trust_owner_roots_from_dir(&host_trust_directory());
    if trusted > 0 {
        println!("owner trust roots installed: {trusted}");
    }
    if socket_path.exists() {
        return Err(format!("socket already exists: {}", socket_path.display()).into());
    }
    let listener = UnixListener::bind(socket_path)?;
    fs::set_permissions(socket_path, fs::Permissions::from_mode(0o600))?;
    let _socket_guard = SocketGuard(socket_path.to_owned());
    println!("simulation socket ready: {}", socket_path.display());

    let (mut stream, _) = listener.accept()?;
    let hello = kobo_protocol::read_from(&mut stream)?;
    let Message::Hello { name } = hello.message else {
        return Err("first application message must be Hello".into());
    };
    println!("application connected: {name}");
    kobo_protocol::write_to(
        &mut stream,
        &Frame {
            request_id: hello.request_id,
            message: Message::Welcome {
                width: u16::try_from(metrics.width)?,
                height: u16::try_from(metrics.height)?,
                pixels_per_inch: u16::try_from(metrics.pixels_per_inch)?,
                text_scale: metrics.text_scale,
            },
        },
    )?;
    serve_application(&mut stream, frame_path, &name, metrics)
}

/// Where a host runtime looks for owner-installed TLS trust roots.
///
/// One fixed place, `~/.config/kobo/trust`, mirroring where the CLI keeps
/// host-side credentials, so `kobo-sidekick init` can drop a certificate in
/// and every host runtime finds it without being told.
fn host_trust_directory() -> PathBuf {
    env::var_os("HOME").map_or_else(
        || PathBuf::from(".kobo-trust"),
        |home| {
            PathBuf::from(home)
                .join(".config")
                .join("kobo")
                .join("trust")
        },
    )
}

fn host_dictionary_directory() -> PathBuf {
    env::var_os("HOME").map_or_else(
        || PathBuf::from(".kobo-dictionaries"),
        |home| {
            PathBuf::from(home)
                .join(".config")
                .join("kobo")
                .join("dictionaries")
        },
    )
}

#[allow(
    clippy::too_many_lines,
    reason = "one arm per message type; splitting the dispatch hides it"
)]
fn serve_application(
    stream: &mut UnixStream,
    frame_path: &Path,
    name: &str,
    metrics: kobo_ui::DisplayMetrics,
) -> Result<(), Box<dyn Error>> {
    // In simulation the daemon owns no hardware, so every hardware-touching
    // request is answered honestly rather than pretended.
    let mut services = DeviceServices::simulated();
    let dictionaries = services.load_dictionaries(&host_dictionary_directory());
    println!("offline dictionaries loaded: {dictionaries}");
    // There is no bezel here to hold a magnet against, so the state the hall
    // sensor reports is set on the way in. Without this the second half of
    // every cover-aware screen is unreachable off hardware.
    services.set_magnet(matches!(
        std::env::var("KOBO_MAGNET").as_deref(),
        Ok("1" | "present")
    ));
    // A real network backend, and the same placeholder grant the panel
    // runtime uses. Without the grant the backend could never run, so this
    // path claimed to be the real runtime while refusing every request an
    // application made.
    let tasks = std::sync::Arc::new(std::sync::Mutex::new(
        TaskRunner::simulated(std::env::temp_dir())
            .with_fetch(std::sync::Arc::new(kobo_net::fetch_from))
            .with_post(std::sync::Arc::new(kobo_net::post))
            .with_capabilities([kobo_policy::Capability::Network]),
    ));
    // Outcomes are delivered from their own thread. This loop blocks on the
    // application's socket, so draining after a message meant a request that
    // took two seconds arrived only when the developer next tapped something,
    // and an application that taps nothing while it waits, which is every
    // application that opens with a request, waited forever. Refusals were
    // instant, which is exactly why nothing noticed.
    let writer = std::sync::Arc::new(std::sync::Mutex::new(stream.try_clone()?));
    {
        let draining = std::sync::Arc::clone(&tasks);
        let writer = std::sync::Arc::clone(&writer);
        std::thread::spawn(move || deliver_outcomes(&draining, &writer));
    }
    let store = kobo_policy::store::Store::new(std::env::temp_dir().join("cobalt-host-state"));
    let shelf = kobo_policy::shelf::Shelf::new(std::env::temp_dir().join("cobalt-host-data"));
    let mut pictures = kobo_ui::PictureCache::default();
    loop {
        let frame = kobo_protocol::read_from(stream)?;
        match frame.message {
            Message::SetScreen(screen) => {
                // Per screen, as the device does it: a book is drawn
                // without a band and everything else with one.
                let chrome = simulated_chrome(name, &screen);
                write_screen(frame_path, screen, &chrome, name, &pictures)?;
            }
            Message::PutPicture {
                handle,
                width,
                height,
                grey,
            } => match pictures.put_report(handle, width, height, grey) {
                None => println!("picture {} refused", handle.0),
                Some(evicted) if !evicted.is_empty() => {
                    println!("picture {} evicted {evicted:?}", handle.0);
                }
                Some(_) => {}
            },
            Message::PutColorPicture {
                handle,
                width,
                height,
                grey,
                rgb,
            } => {
                if !pictures.put_color_with_fallback(handle, width, height, grey, rgb) {
                    println!("color picture {} refused", handle.0);
                }
            }
            Message::BeginPicture {
                handle,
                width,
                height,
            } => {
                if !pictures.begin_upload(handle, width, height) {
                    println!("picture {} upload refused", handle.0);
                }
            }
            Message::PictureChunk {
                handle,
                offset,
                grey,
            } => {
                if !pictures.upload_chunk(
                    handle,
                    usize::try_from(offset).unwrap_or(usize::MAX),
                    &grey,
                ) {
                    println!("picture {} chunk refused", handle.0);
                }
            }
            Message::CommitPicture { handle } => match pictures.commit_upload(handle) {
                None => println!("picture {} commit refused", handle.0),
                Some(evicted) if !evicted.is_empty() => {
                    println!("picture {} evicted {evicted:?}", handle.0);
                }
                Some(_) => {}
            },
            Message::DropPicture { handle } => pictures.remove(handle),
            Message::PutFont {
                handle,
                name,
                bytes,
            } => match kobo_text::BookFont::from_bytes(&bytes, &name, metrics) {
                Ok(font) => kobo_ui::put_book_typesetter(handle, Box::new(font)),
                Err(error) => println!("font {} refused: {error}", handle.0),
            },
            Message::DropFont { handle } => kobo_ui::drop_book_typesetter(handle),
            // This path renders one application to a file and owns no panel to
            // hand over, so the request is reported rather than performed.
            Message::Launch { name } => println!("launch requested: {name}"),
            Message::Log { level, message } => log_app(level, &message),
            Message::DeviceRequest(request) => {
                let result = services.handle(request.clone());
                println!("device request {request:?} -> {result:?}");
                write_shared(
                    &writer,
                    &Frame {
                        request_id: frame.request_id,
                        message: Message::DeviceResult(result),
                    },
                )?;
            }
            Message::Spawn { task, work } => {
                let submitted = tasks
                    .lock()
                    .map_err(|_| "the task lock was poisoned")?
                    .submit(task, work);
                if let Err(reason) = submitted {
                    println!("task {} refused: {reason:?}", task.0);
                    write_shared(
                        &writer,
                        &Frame {
                            request_id: frame.request_id,
                            message: Message::TaskOutcome {
                                task,
                                outcome: TaskOutcome::Failed(TaskError::Denied),
                            },
                        },
                    )?;
                }
            }
            Message::StoreRequest(request) => {
                let result = shelf
                    .handle(&request)
                    .unwrap_or_else(|| store.handle(&request));
                write_shared(
                    &writer,
                    &Frame {
                        request_id: frame.request_id,
                        message: Message::StoreResult(result),
                    },
                )?;
            }
            // This path renders to a file and has no reader at a keyboard, so
            // there is nothing a terminal could usefully be attached to. It is
            // refused rather than opened, because a build performs only what
            // it has a backend for and a silently ignored request would leave
            // the application waiting for output forever.
            Message::ShellRequest(_) => write_shared(
                &writer,
                &Frame {
                    request_id: frame.request_id,
                    message: Message::ShellEvent(kobo_protocol::ShellEvent::Refused(
                        kobo_protocol::ShellError::Unavailable,
                    )),
                },
            )?,
            Message::Cancel { task } => tasks
                .lock()
                .map_err(|_| "the task lock was poisoned")?
                .cancel(task),
            Message::Exit => {
                // Nothing an application started may outlive it.
                tasks
                    .lock()
                    .map_err(|_| "the task lock was poisoned")?
                    .shutdown();
                return Ok(());
            }
            Message::Hello { .. }
            | Message::Welcome { .. }
            | Message::Action { .. }
            | Message::TextHold { .. }
            | Message::TaskOutcome { .. }
            | Message::DeviceResult(_)
            | Message::StoreResult(_)
            | Message::Lifecycle(_)
            | Message::CoverChanged { .. }
            | Message::PageTurn { .. }
            | Message::ShellEvent(_) => {
                return Err("application sent a daemon-only message".into());
            }
        }
    }
}

/// Writes one frame to the application through the shared handle.
///
/// Every write goes through this, because the task thread and this loop both
/// write to the same socket and two frames interleaved on a stream protocol is
/// a stream that can never be read again.
fn write_shared(
    writer: &std::sync::Arc<std::sync::Mutex<UnixStream>>,
    frame: &Frame,
) -> Result<(), Box<dyn Error>> {
    let mut stream = writer.lock().map_err(|_| "the writer lock was poisoned")?;
    kobo_protocol::write_to(&mut *stream, frame)?;
    Ok(())
}

/// Hands every finished task to the application, from its own thread.
fn deliver_outcomes(
    tasks: &std::sync::Arc<std::sync::Mutex<TaskRunner>>,
    writer: &std::sync::Arc<std::sync::Mutex<UnixStream>>,
) {
    loop {
        let Ok(finished) = tasks.lock().map(|mut tasks| tasks.drain()) else {
            return;
        };
        for finished in finished {
            let Ok(mut stream) = writer.lock() else {
                return;
            };
            if kobo_protocol::write_to(
                &mut *stream,
                &Frame {
                    request_id: 0,
                    message: Message::TaskOutcome {
                        task: finished.task,
                        outcome: finished.outcome,
                    },
                },
            )
            .is_err()
            {
                return;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// The chrome an application would be given on a device, in simulation.
///
/// The launcher is home, so it has nowhere to go back to; everything else was
/// opened from it and does. The panel runtime decides this by comparing paths,
/// which a simulation running one application from a target directory has no
/// equivalent of, so it goes by the name the application introduced itself
/// with, the same name the launcher uses.
///
/// The band is here for the same reason the way back is: the device draws one
/// on every screen that is not a book, so a simulation without one is a
/// simulation of a screen that does not exist. It was missing, and it hid a
/// layout fault that put the first row of the launcher's grid underneath the
/// title on real hardware while every frame rendered here looked right.
fn simulated_chrome(name: &str, screen: &Screen) -> kobo_ui::Chrome {
    let chrome = kobo_ui::Chrome::with_back(name != HOME_APPLICATION);
    if screen.reading {
        return chrome;
    }
    chrome.with_status(simulated_status())
}

/// Everything the band shows, invented and fixed.
///
/// Fixed rather than read from the host, because a frame that changes with the
/// clock is a frame nobody can compare against the last one, and the point of
/// the band being here is that it occupies the room it occupies, not that it
/// says anything true. Deliberately not round numbers, so nobody mistakes a
/// simulated reading for a real one.
fn simulated_status() -> kobo_ui::Status {
    kobo_ui::Status {
        clock: "09:41".to_owned(),
        signal: kobo_ui::Signal::Strong,
        battery: Some(kobo_ui::Percent::new(72)),
        charging: false,
        // On, so the simulator draws the mark and a layout fault beside the
        // radio shows up here rather than only on hardware with headphones
        // paired.
        bluetooth: true,
    }
}

/// The application that is home, and so has no way back to draw.
const HOME_APPLICATION: &str = "launcher";

fn write_screen(
    path: &Path,
    screen: Screen,
    chrome: &kobo_ui::Chrome,
    name: &str,
    pictures: &dyn kobo_ui::Pictures,
) -> Result<(), Box<dyn Error>> {
    let mut surface = Surface::new(
        usize::try_from(crate::device_metrics().width)?,
        usize::try_from(crate::device_metrics().height)?,
    );
    // The same two steps the device takes, in the same order, because a
    // preview drawn with different chrome is a preview of a screen that will
    // never exist. Rendering with &Chrome::default() here meant the way back
    // was the one part of every screen that could not be looked at without a
    // reader, and it is the part that traps somebody when it is missing.
    let screen = kobo_ui::ensure_way_back(screen, chrome, name);
    let metrics = crate::device_metrics();
    // The same reason as on the device: the typeface sets at the ambient
    // scale, so a preview of a screen that asked for larger prose has to say
    // so or it is a preview of a screen nobody will see. The interface around
    // the prose keeps the reader's own size either way.
    kobo_ui::set_text_scale(metrics.text_scale);
    kobo_ui::set_reading_scale(screen.text_scale.unwrap_or(metrics.text_scale));
    kobo_ui::render_all(&screen, &metrics, chrome, pictures, &mut surface, None);

    let temporary = path.with_extension(format!("raw.tmp-{}", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let result = (|| -> std::io::Result<()> {
        file.write_all(&surface.pixels)?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result?;
    println!("rendered screen {} to {}", screen.id, path.display());
    Ok(())
}

fn log_app(level: LogLevel, message: &str) {
    let message = message.replace(['\r', '\n'], " ");
    println!("app {level:?}: {message}");
    // An application logs to explain itself, and the times it most needs to be
    // believed are the times it took the reader down with it. Standard output
    // does not survive that, so anything an application says goes to the black
    // box as well; it is a no-op unless the trace is on.
    #[cfg(feature = "device-write")]
    crate::blackbox::trace(&format!("app {level:?}: {message}"));
}

fn validate_simulation_paths(socket: &Path, frame: &Path) -> Result<(), Box<dyn Error>> {
    let socket_parent = socket.parent().ok_or("simulation socket needs a parent")?;
    let frame_parent = frame.parent().ok_or("simulation frame needs a parent")?;
    if socket_parent != frame_parent {
        return Err("simulation socket and frame must share a private directory".into());
    }
    let parent = socket_parent.canonicalize()?;
    let temporary_root = env::temp_dir().canonicalize()?;
    if !parent.starts_with(&temporary_root)
        || !parent
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("kobo-sim-"))
    {
        return Err("simulation directory must be a kobo-sim-* directory under temp".into());
    }
    let mode = fs::metadata(&parent)?.permissions().mode();
    if mode & 0o077 != 0 {
        return Err("simulation directory must not be accessible by group or others".into());
    }
    if frame.exists() {
        return Err(format!("frame already exists: {}", frame.display()).into());
    }
    Ok(())
}

struct SocketGuard(PathBuf);

impl Drop for SocketGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn an_elipsa_session_keeps_its_verified_metrics_without_another_probe() {
        let ambient = kobo_ui::DisplayMetrics {
            text_scale: kobo_ui::TextScale::Large,
            ..kobo_ui::CLARA_BW_METRICS
        };
        let metrics = super::metrics_for_profile(&kobo_profile::ELIPSA_2E_389, ambient)
            .expect("the supported profile fits layout coordinates");
        assert_eq!(metrics.width, 1404);
        assert_eq!(metrics.height, 1872);
        assert_eq!(metrics.pixels_per_inch, 227);
        assert_eq!(metrics.text_scale, kobo_ui::TextScale::Large);
    }

    fn plain() -> kobo_ui::Screen {
        kobo_ui::Screen::new(1, Vec::new())
    }

    fn book() -> kobo_ui::Screen {
        let mut screen = plain();
        screen.reading = true;
        screen
    }

    #[test]
    fn everything_but_the_home_screen_is_given_a_way_back() {
        // Without this the simulation drew every screen with no way back,
        // which is the one defect that leaves somebody stuck on a reader and
        // was the only part of a screen that could not be checked without one.
        assert!(!super::simulated_chrome("launcher", &plain()).back);
        for name in ["rss", "hn", "gutenbird", "todo", "terminal"] {
            assert!(
                super::simulated_chrome(name, &plain()).back,
                "{name} had no way back"
            );
        }
    }

    #[test]
    fn simulation_draws_the_band_the_device_draws() {
        // The simulator drew no band at all, so every frame checked here was a
        // frame of a screen the device never shows -- and a band of zero
        // height made a layout fault that only exists with one invisible. The
        // launcher's first row of tiles was drawn underneath its own title on
        // real hardware while this looked perfect.
        assert!(super::simulated_chrome("rss", &plain()).status.is_some());
        // Except over a book, which is where the device withholds it too.
        assert!(super::simulated_chrome("gutenbird", &book())
            .status
            .is_none());
    }

    #[test]
    fn an_application_with_no_bar_of_its_own_is_given_one_to_go_back_from() {
        let bare = kobo_ui::Screen::new(1, Vec::new());
        assert!(bare.top_bar.is_none());
        let chrome = super::simulated_chrome("rss", &bare);
        let fixed = kobo_ui::ensure_way_back(bare, &chrome, "Feeds");
        assert_eq!(
            fixed.top_bar.expect("a bar to hold the way back").title,
            "Feeds"
        );
    }

    #[test]
    fn the_home_screen_is_not_given_a_bar_it_did_not_ask_for() {
        let bare = kobo_ui::Screen::new(1, Vec::new());
        let chrome = super::simulated_chrome("launcher", &bare);
        let left = kobo_ui::ensure_way_back(bare, &chrome, "Cobalt");
        assert!(left.top_bar.is_none());
    }
    use super::validate_simulation_paths;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    /// Killing the daemon outright brought the reader back on hardware and
    /// left the session directory, its heartbeat and its socket in `/tmp`. A
    /// stale heartbeat is indistinguishable from a session in progress, so
    /// recovery has to sweep up after itself.
    #[cfg(feature = "device-write")]
    #[test]
    fn recovery_sweeps_up_what_a_killed_session_left_behind() {
        let state = std::env::temp_dir().join(format!("kobo-clear-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&state);
        fs::create_dir(&state).expect("create the session directory");
        fs::write(state.join("argv"), b"nickel").expect("write a session file");
        let sidecars = ["beat", "sock", "cancel"].map(|suffix| {
            let mut path = state.as_os_str().to_owned();
            path.push(format!(".{suffix}"));
            std::path::PathBuf::from(path)
        });
        for sidecar in &sidecars {
            fs::write(sidecar, b"1").expect("write a sidecar");
        }

        let report = super::clear_session_files(&state);

        assert!(!state.exists(), "the session directory survived: {report}");
        for sidecar in &sidecars {
            assert!(
                !sidecar.exists(),
                "{} survived: {report}",
                sidecar.display()
            );
        }
        assert!(report.contains("cleared 4"), "unexpected report: {report}");
        // Recovery runs on every abnormal exit, including ones that already
        // cleaned up, so a second sweep has to be silent rather than an error.
        assert!(super::clear_session_files(&state).contains("cleared 0"));
    }

    #[test]
    fn simulation_paths_require_private_temp_directory() {
        let root = std::env::temp_dir().join(format!("kobo-sim-test-{}", std::process::id()));
        fs::create_dir(&root).expect("create private directory");
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
            .expect("set private permissions");
        assert!(
            validate_simulation_paths(&root.join("kobod.sock"), &root.join("frame.raw")).is_ok()
        );
        assert!(validate_simulation_paths(
            &root.join("kobod.sock"),
            &std::env::temp_dir().join("other.raw")
        )
        .is_err());
        fs::remove_dir(root).expect("remove private directory");
    }
}

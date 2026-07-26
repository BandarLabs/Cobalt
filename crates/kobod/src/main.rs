use kobo_policy::{DeviceServices, TaskRunner};
use kobo_profile::CLARA_BW_391;
use kobo_protocol::{Frame, LogLevel, Message, TaskError, TaskOutcome};
use kobo_ui::{render, Screen, Surface, CLARA_BW_METRICS, DISPLAY_HEIGHT, DISPLAY_WIDTH};
use std::env;
use std::error::Error;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[cfg(feature = "device-write")]
mod blackbox;
#[cfg(feature = "device-write")]
mod device;

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
    // Performs one real request and reports what happened. This touches no
    // hardware and does not go near the reader, so it is safe to run on a
    // device in normal use, which is the point: the network path has to be
    // provable without a handoff.
    if arguments.len() == 3 && arguments[0] == "--fetch" {
        return fetch_once(&arguments[1], &arguments[2]);
    }
    Err("usage: kobod [--sim-socket PATH --frame PATH] [--present APP] [--fetch URL BYTES]".into())
}

#[cfg(feature = "device-write")]
fn touch_test(seconds: &str) -> Result<(), Box<dyn Error>> {
    use kobo_hal::input::TouchSession;
    use std::time::{Duration, Instant};

    let seconds: u64 = seconds.parse().unwrap_or(20).min(120);
    let profile = &kobo_profile::CLARA_BW_391;
    println!("touch device: {}", device::TOUCH_DEVICE);
    let mut session = TouchSession::acquire(Path::new(device::TOUCH_DEVICE), profile)?;
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
    let pid = reader.start(std::time::Duration::from_secs(45))?;
    // A session that died without cleaning up also left the freeze watchdog
    // suspended. Putting it back is part of recovery, not an afterthought.
    match kobo_hal::supervisor::resume_with(reader.environment("DBUS_SESSION_BUS_ADDRESS")) {
        Ok(()) => println!("reader restarted as pid {pid}; freeze watchdog resumed"),
        Err(error) => println!(
            "reader restarted as pid {pid}, but the freeze watchdog could not be resumed ({error}); it returns on the next reboot"
        ),
    }
    Ok(())
}

fn print_safety_state() {
    let write_unlocked = env::var_os("KOBO_DEVICE_WRITE_UNLOCK").is_some();
    println!("kobod 0.1.0");
    println!("profile: {}", CLARA_BW_391.id);
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
                width: u16::try_from(DISPLAY_WIDTH)?,
                height: u16::try_from(DISPLAY_HEIGHT)?,
                pixels_per_inch: u16::try_from(CLARA_BW_METRICS.pixels_per_inch)?,
            },
        },
    )?;
    serve_application(&mut stream, frame_path)
}

fn serve_application(stream: &mut UnixStream, frame_path: &Path) -> Result<(), Box<dyn Error>> {
    // In simulation the daemon owns no hardware, so every hardware-touching
    // request is answered honestly rather than pretended.
    let mut services = DeviceServices::simulated();
    // No network backend is supplied, so a fetch is refused rather than faked.
    let mut tasks = TaskRunner::simulated(std::env::temp_dir())
        .with_fetch(std::sync::Arc::new(kobo_net::fetch_from));
    let store = kobo_policy::store::Store::new(std::env::temp_dir().join("cobalt-host-state"));
    loop {
        let frame = kobo_protocol::read_from(stream)?;
        match frame.message {
            Message::SetScreen(screen) => write_screen(frame_path, &screen)?,
            // This path renders one application to a file and owns no panel to
            // hand over, so the request is reported rather than performed.
            Message::Launch { name } => println!("launch requested: {name}"),
            Message::Log { level, message } => log_app(level, &message),
            Message::DeviceRequest(request) => {
                let result = services.handle(request);
                println!("device request {request:?} -> {result:?}");
                kobo_protocol::write_to(
                    stream,
                    &Frame {
                        request_id: frame.request_id,
                        message: Message::DeviceResult(result),
                    },
                )?;
            }
            Message::Spawn { task, work } => {
                if let Err(reason) = tasks.submit(task, work) {
                    println!("task {} refused: {reason:?}", task.0);
                    kobo_protocol::write_to(
                        stream,
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
                let result = store.handle(&request);
                kobo_protocol::write_to(
                    stream,
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
            Message::ShellRequest(_) => kobo_protocol::write_to(
                stream,
                &Frame {
                    request_id: frame.request_id,
                    message: Message::ShellEvent(kobo_protocol::ShellEvent::Refused(
                        kobo_protocol::ShellError::Unavailable,
                    )),
                },
            )?,
            Message::Cancel { task } => tasks.cancel(task),
            Message::Exit => {
                // Nothing an application started may outlive it.
                tasks.shutdown();
                return Ok(());
            }
            Message::Hello { .. }
            | Message::Welcome { .. }
            | Message::Action { .. }
            | Message::TaskOutcome { .. }
            | Message::DeviceResult(_)
            | Message::StoreResult(_)
            | Message::Lifecycle(_)
            | Message::ShellEvent(_) => {
                return Err("application sent a daemon-only message".into());
            }
        }
        for finished in tasks.drain() {
            kobo_protocol::write_to(
                stream,
                &Frame {
                    request_id: 0,
                    message: Message::TaskOutcome {
                        task: finished.task,
                        outcome: finished.outcome,
                    },
                },
            )?;
        }
    }
}

fn write_screen(path: &Path, screen: &Screen) -> Result<(), Box<dyn Error>> {
    let mut surface = Surface::new(
        usize::try_from(DISPLAY_WIDTH)?,
        usize::try_from(DISPLAY_HEIGHT)?,
    );
    render(screen, &mut surface, None);

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
    println!("app {level:?}: {}", message.replace(['\r', '\n'], " "));
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
    use super::validate_simulation_paths;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

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

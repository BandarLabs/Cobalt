//! Running an application on the panel.
//!
//! This is the mode in which the platform actually owns the device: the stock
//! reader is stopped, the framebuffer and touch panel belong to us, and an
//! application's screens are what the owner sees.
//!
//! The ordering here is the whole safety argument, so it is written out rather
//! than left implicit.
//!
//! Acquire, in this order:
//!
//! 1. Find the reader and save how to restart it.
//! 2. Arm a watchdog that restarts it even if we are killed outright.
//! 3. Open the display, which validates the hardware profile exactly.
//! 4. Snapshot the whole screen.
//! 5. Take the touch panel.
//! 6. Suspend Kobo's freeze watchdog.
//! 7. Only now stop the reader.
//!
//! Step 6 is not optional and its position is not arbitrary. `sickel` reboots
//! the device when the reader stops pinging it, and it cannot tell a reader we
//! stopped on purpose from one that hung. Suspending it after stopping the
//! reader would leave a window in which the device could reboot underneath us.
//!
//! Nothing that can fail is left until after the reader is down. If the profile
//! does not match, or the panel is busy, or the screen cannot be captured, we
//! find out while the device is still completely untouched.
//!
//! Release runs in the exact reverse order and, critically, runs on *every*
//! path: normal exit, application crash, protocol violation, and deadline.
//! Release builds abort on panic, so no `Drop` implementation would run; the
//! unwinding is therefore explicit and centralised in one function rather than
//! spread across returns.

use crate::blackbox::{self, trace};
use kobo_hal::display::{DisplaySession, OWNER_UNLOCK_PHRASE};
use kobo_hal::input::TouchSession;
use kobo_hal::network::{Connection, Restored};
use kobo_hal::reader::{Reader, Watchdog, WATCHDOG_CHECK};
use kobo_hal::supervisor::Suspended;
use kobo_hal::touch::TouchEvent;
use kobo_hal::{Rect, RefreshIntent, RefreshPlan, RegionSnapshot};
use kobo_policy::{Backends, Capability, Declared, DeviceServices, PowerPolicy, TaskRunner};
use kobo_profile::{DeviceProfile, CLARA_BW_391};
use kobo_protocol::{Frame, Message, TaskError, TaskOutcome};
use kobo_ui::{render_with, ActionId, Chrome, Screen, Surface, CLARA_BW_METRICS};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// The touch panel on every device this supports so far.
pub const TOUCH_DEVICE: &str = "/dev/input/event1";
/// Where named credentials live.
///
/// On the book partition, because that is the one place the owner can reach
/// over USB without a shell, and because `/tmp` is a RAM disk that every
/// reboot empties. An application names a secret; only the runtime reads one.
const SECRETS: &str = "/mnt/onboard/.adds/cobalt/secrets";

/// Where each application's own keyed state lives, one directory per name.
const STATE_ROOT: &str = "/mnt/onboard/.adds/cobalt/state";
/// How long the reader is given to stop, and to come back.
const STOP_GRACE: Duration = Duration::from_secs(15);
const START_GRACE: Duration = Duration::from_secs(45);
/// The longest a session may own the device. A session that outlives this is
/// assumed to be wedged, and the reader is more valuable than the application.
///
/// This used to be half an hour and used to be the *only* way a session ended,
/// which meant the panel was taken away from somebody in the middle of using
/// it. It is now a backstop rather than a policy: a session ends when the
/// reader asks to go back, or when nothing has happened for [`IDLE_LIMIT`].
const MAX_SESSION: Duration = Duration::from_secs(2 * 60 * 60);
/// How long the panel may sit with nothing happening before the reader gets it
/// back.
///
/// Every tap and every repaint restarts this, so it measures genuine
/// abandonment rather than the pace of use. A device left on a screen nobody
/// is looking at should be an e-reader again, because that is what somebody
/// picking it up will expect it to be.
///
/// An hour rather than the fifteen minutes this started as. Fifteen sounds
/// generous and is not: a panel session is something the owner starts and then
/// puts down, and a session that had never been touched ended itself while its
/// owner was still deciding what to open. The point of this limit is a device
/// left behind, not a device being thought about.
const IDLE_LIMIT: Duration = Duration::from_secs(60 * 60);
/// The longest the loop waits between passes even when nothing is happening,
/// which bounds how stale the recovery watchdog's heartbeat can get.
const BEAT_INTERVAL: Duration = Duration::from_secs(10);

/// How stale a battery reading may be before it is taken again.
///
/// Read on demand rather than on a timer, so a session where nobody asks does
/// no file work at all, and rate limited so an application polling in a loop
/// cannot turn a read into a busy one. A gauge does not move meaningfully
/// inside half a minute, so nothing is lost.
const BATTERY_INTERVAL: Duration = Duration::from_secs(30);
/// How long to wait for the restarted reader to feed the freeze watchdog
/// before handing it back regardless.
///
/// The reader takes tens of seconds to reach its first ping, and the watchdog
/// reboots the device ten seconds after being resumed if nothing feeds it, so
/// this has to be generous. Waiting longer only delays the summary; waiting
/// too little reboots the device.
const WATCHDOG_HANDBACK: Duration = Duration::from_secs(90);

/// How long to wait for the connection to come back. Association plus a DHCP
/// lease takes several seconds on this radio, and the reader is already running
/// again by this point, so waiting costs nothing but the report.
const NETWORK_GRACE: Duration = Duration::from_secs(30);
/// How long the application is given to exit before it is killed.
const APP_STOP_GRACE: Duration = Duration::from_secs(3);

/// What the runtime is waiting for. Both sources feed one channel so the
/// runtime blocks rather than polling; a poll loop would keep the processor
/// awake between taps, which on a device that idles at zero power costs real
/// battery life.
enum Event {
    Touch(TouchEvent),
    App(Box<Frame>),
    /// The application's end of the socket closed.
    AppGone,
    /// A background task finished and its outcome is waiting to be drained.
    ///
    /// The loop otherwise only notices a finished task when something else
    /// wakes it, so an answer that had already arrived sat unread until the
    /// owner touched the panel. That reads as a hung application, and it is
    /// what made both a chat reply and a book download look stuck.
    TaskReady,
}

/// How long a session may run, and how long it may be ignored.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Limits {
    /// Ends the session when nothing has happened for this long.
    pub idle: Duration,
    /// Ends the session however busy it is.
    pub ceiling: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            idle: IDLE_LIMIT,
            ceiling: MAX_SESSION,
        }
    }
}

/// Runs `application` on the panel until it asks to leave, is left alone for
/// `limits.idle`, or reaches `limits.ceiling`.
///
/// Deliberately one function. Every step here takes something away from the
/// device and has to give it back in the exact reverse order, and that
/// argument is only checkable when the whole sequence is on one screen.
///
/// # Errors
///
/// Returns an error describing what failed and, always, what state the device
/// was left in.
#[allow(clippy::too_many_lines)]
pub fn present(application: &Path, limits: Limits) -> Result<String, String> {
    let limits = Limits {
        idle: limits.idle.min(MAX_SESSION),
        ceiling: limits.ceiling.min(MAX_SESSION),
    };
    let profile = &CLARA_BW_391;

    // Checked here, before anything is taken over. Stopping the reader costs
    // the owner half a minute and the network connection, so discovering only
    // afterwards that there was nothing to run is the worst possible order.
    // This is the likeliest failure of all: `/tmp` is a tmpfs, so every staged
    // application disappears on a reboot.
    preflight(application)?;

    // Installed before anything is taken over, because it reads a file and a
    // read can fail. A failure is not fatal: `kobo-ui` keeps its built-in
    // bitmap, so the worst case is ugly text rather than a dead session.
    let typeface = match kobo_text::install(CLARA_BW_METRICS) {
        Ok(path) => path.file_name().map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        ),
        Err(error) => format!("none ({error})"),
    };

    // A pulse every couple of seconds, so a trace that simply stops tells us
    // the device died at that instant rather than merely that nothing was
    // happening. The thread is deliberately never joined: the process exits at
    // the end of the session and takes it with it, and a heartbeat that stopped
    // early because of a tidy shutdown would be a heartbeat that lies.
    if blackbox::recording() {
        thread::spawn(|| loop {
            thread::sleep(Duration::from_secs(2));
            trace("alive");
        });
    }

    // Everything that can fail happens before the reader is stopped.
    let reader = Reader::find().map_err(|error| error.to_string())?;
    let state = PathBuf::from(format!("/tmp/kobo-session-{}", std::process::id()));
    reader
        .save(&state)
        .map_err(|error| format!("save reader description: {error}"))?;
    let watchdog = Arc::new(
        Watchdog::arm(&state, WATCHDOG_CHECK).map_err(|error| format!("arm watchdog: {error}"))?,
    );

    let display = DisplaySession::open(profile, Some(OWNER_UNLOCK_PHRASE))
        .map_err(|error| format!("open display: {error}"))?;
    let geometry = display.geometry();
    let whole_screen = Rect {
        x: 0,
        y: 0,
        width: geometry.width,
        height: geometry.height,
    };
    let backup = display
        .capture(whole_screen)
        .map_err(|error| format!("snapshot the screen: {error}"))?;
    let mut touch = TouchSession::acquire(Path::new(TOUCH_DEVICE), profile)
        .map_err(|error| format!("take the touch panel: {error}"))?;

    // Recorded while the daemons are still alive. Restarting the reader drops
    // the connection every time, and on a device managed over Wi-Fi that would
    // otherwise cost the very link this session was started through.
    let connection = Connection::capture();

    // Without this the device reboots itself partway through the session, so a
    // refusal here is fatal and the reader is left running.
    let suspended = Suspended::suspend(reader.environment("DBUS_SESSION_BUS_ADDRESS"))
        .map_err(|error| format!("suspend the freeze watchdog: {error}"))?;

    // The point of no return.
    trace("stopping the reader");
    reader
        .stop(STOP_GRACE)
        .map_err(|error| format!("stop the reader: {error}"))?;

    // One reader thread on the touch descriptor for the whole panel session,
    // started here rather than per application.
    let taps = TouchSink::default();
    pump_touch(&mut touch, &taps);

    let outcome = host_applications(
        application,
        &display,
        whole_screen,
        &taps,
        limits,
        profile,
        &watchdog,
    );
    trace("session finished, handing the panel back");
    println!("session finished, handing the panel back");

    // Teardown takes minutes in the worst case — the reader is given
    // forty-five seconds to come back, the network thirty, and the freeze
    // watchdog ninety more — and none of that runs the loop that normally
    // reports progress. Without this the recovery watchdog would conclude the
    // runtime had died and restart a reader that is already starting.
    let teardown = KeepBeating::start(&watchdog);
    // Reverse order, on every path.
    let restored = restore_screen(&display, &backup, whole_screen);
    let _ignored = touch.release();
    // The panel and the touch descriptor are given up *before* the reader is
    // started, not after it. Holding the display open while the reader brings
    // the EPD controller back up leaves two owners of one piece of hardware,
    // and the device then resets around thirty seconds later without syncing
    // anything, which is the SoC's own 31 second hardware watchdog rather than
    // any watchdog we can talk to. Ordering this correctly costs nothing.
    drop(touch);
    drop(display);
    trace("panel and touch released, restarting the reader");
    println!("panel released, restarting the reader");
    let restarted = reader.start(START_GRACE);
    let network = connection.restore(NETWORK_GRACE);
    trace("reader restart returned, waiting for it to feed the freeze watchdog");
    println!("waiting for the reader to feed the freeze watchdog");
    // Resumed only once the reader is feeding it again. Resuming the moment
    // the process exists lights a ten second fuse that a still-starting reader
    // cannot feed, which is what rebooted the device at the end of a session.
    let resumed = suspended.resume_once_fed(WATCHDOG_HANDBACK);
    watchdog.disarm();
    drop(teardown);
    let _ignored = fs::remove_dir_all(&state);

    let reader_state = match (restarted, resumed) {
        (Ok(pid), Ok(after)) => {
            format!("the reader is running again as pid {pid}, and the freeze watchdog was resumed {after}")
        }
        (Ok(pid), Err(error)) => format!(
            "the reader is running again as pid {pid}, but the freeze watchdog could not be resumed ({error}); it returns on the next reboot"
        ),
        (Err(error), _) => format!(
            "THE READER DID NOT COME BACK ({error}). Power cycle the device; it always boots the stock reader"
        ),
    };
    let reader_state = match network {
        Ok(Restored::Unaffected) => reader_state,
        Ok(Restored::Restarted) => format!("{reader_state}, and the network was reconnected"),
        Ok(Restored::StillDown) => format!(
            "{reader_state}, but the network did not come back; reconnect from the reader's own network screen"
        ),
        Err(error) => format!(
            "{reader_state}, but the network could not be restarted ({error}); reconnect from the reader's own network screen"
        ),
    };
    match (outcome, restored) {
        (Ok(summary), Ok(())) => Ok(format!("{summary}; typeface {typeface}; {reader_state}")),
        (Ok(summary), Err(error)) => Ok(format!(
            "{summary}; typeface {typeface}; the screen could not be restored ({error}), but {reader_state} and repaints its own screen"
        )),
        (Err(error), _) => Err(format!("{error}; {reader_state}")),
    }
}

/// Renders a duration the way the summary should read it.
///
/// Dividing by sixty reported a forty-five second session as a "0 minute"
/// limit, which reads as a bug in the session rather than a short one.
fn describe(limit: Duration) -> String {
    let seconds = limit.as_secs();
    if seconds < 60 {
        return format!("{seconds} second");
    }
    let minutes = seconds / 60;
    match seconds % 60 {
        0 => format!("{minutes} minute"),
        rest => format!("{minutes} minute {rest} second"),
    }
}

fn restore_screen(
    display: &DisplaySession,
    backup: &RegionSnapshot,
    whole_screen: Rect,
) -> Result<(), String> {
    display
        .restore(backup)
        .map_err(|error| format!("restore the screen: {error}"))?;
    let plan = RefreshPlan::new(
        whole_screen,
        RefreshIntent::QualityContent,
        false,
        whole_screen.width,
        whole_screen.height,
    )
    .ok_or_else(|| "the screen is not inside itself".to_owned())?;
    display
        .refresh(plan)
        .map_err(|error| format!("show the restored screen: {error}"))
}

/// Hosts one application after another on a panel that is already owned.
///
/// The display and the touch panel are taken once and held throughout, because
/// handing them back between applications would show the reader for a moment
/// and cost two full refreshes every time somebody opened something.
fn host_applications(
    application: &Path,
    display: &DisplaySession,
    whole_screen: Rect,
    touch: &TouchSink,
    limits: Limits,
    profile: &'static DeviceProfile,
    watchdog: &Arc<Watchdog>,
) -> Result<String, String> {
    // the touch panel are taken once and held throughout, because handing them
    // back between applications would show the reader for a moment and cost two
    // full refreshes every time somebody opened something.
    let catalogue = application
        .parent()
        .map_or_else(|| PathBuf::from("/tmp"), Path::to_path_buf);
    let deadline = Instant::now() + limits.ceiling;
    let home = application.to_path_buf();
    let mut current = home.clone();
    let mut visited = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break Ok(format!(
                "the session limit ended after {}",
                visited.join(", ")
            ));
        }
        // Only an application the launcher started can be closed back to it;
        // the first one has nowhere to go but the reader.
        let chrome = Chrome::with_back(current != home);
        match run_session(
            &current,
            display,
            whole_screen,
            touch,
            SessionOptions {
                limits: Limits {
                    idle: limits.idle,
                    ceiling: remaining,
                },
                profile,
                chrome,
                watchdog: Arc::clone(watchdog),
            },
        ) {
            Err(error) => break Err(error),
            Ok(session) => {
                visited.push(session.summary.clone());
                match session.next {
                    // An application that finishes hands the panel back to
                    // whatever started the session, so closing something
                    // returns to the launcher rather than to the reader. Only
                    // the first application ending finishes the session.
                    None if current == home => break Ok(visited.join("; ")),
                    None => current.clone_from(&home),
                    Some(name) => match resolve(&catalogue, &name) {
                        Ok(path) => current = path,
                        // A launch that cannot be satisfied returns to the
                        // launcher. Ending the session instead would show the
                        // reader again, cost the owner half a minute and the
                        // network, and take every other application down with
                        // it, all because one entry was missing.
                        Err(error) => {
                            visited.push(error);
                            current.clone_from(&home);
                        }
                    },
                }
            }
        }
    }
}

/// How a hosted application finished.
struct Outcome {
    summary: String,
    /// The application it asked the runtime to run next, if any.
    next: Option<String>,
}

/// Refuses a session that cannot possibly succeed, while it is still free.
///
/// The reader is still running when this returns an error, so the cost of a
/// mistake here is a message rather than a handoff.
fn preflight(application: &Path) -> Result<(), String> {
    let metadata = fs::metadata(application).map_err(|error| {
        format!(
            "there is nothing to run at {}: {error}. Nothing was changed on the device; note that /tmp is cleared by a reboot, so a staged application has to be uploaded again",
            application.display()
        )
    })?;
    if !metadata.is_file() {
        return Err(format!(
            "{} is not a file, so it cannot be run. Nothing was changed on the device",
            application.display()
        ));
    }
    if metadata.permissions().mode() & 0o111 == 0 {
        return Err(format!(
            "{} is not executable. Nothing was changed on the device",
            application.display()
        ));
    }
    Ok(())
}

/// Turns an application's name into the binary to run.
///
/// Names are validated rather than trusted. An application that could name a
/// path could start anything on the device, so the catalogue is a directory the
/// runtime chooses and the name may only select an entry within it.
fn resolve(catalogue: &Path, name: &str) -> Result<PathBuf, String> {
    let sane = !name.is_empty()
        && name.len() <= 32
        && name.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        });
    if !sane {
        return Err(format!("{name:?} is not a valid application name"));
    }
    let path = catalogue.join(format!("kobo-{name}"));
    if path.is_file() {
        Ok(path)
    } else {
        Err(format!("no application named {name} is installed"))
    }
}

fn run_session(
    application: &Path,
    display: &DisplaySession,
    whole_screen: Rect,
    touch: &TouchSink,
    options: SessionOptions,
) -> Result<Outcome, String> {
    let socket_path = PathBuf::from(format!("/tmp/kobo-session-{}.sock", std::process::id()));
    let _ignored = fs::remove_file(&socket_path);
    let listener = std::os::unix::net::UnixListener::bind(&socket_path)
        .map_err(|error| format!("bind application socket: {error}"))?;

    let spawned = Command::new(application)
        .env_clear()
        .env("KOBO_SOCKET", &socket_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        // The socket is removed here as well, because returning early would
        // otherwise leave a stale entry in /tmp for every failed start.
        Err(error) => {
            let _ignored = fs::remove_file(&socket_path);
            return Err(format!("start {}: {error}", application.display()));
        }
    };

    let result = converse(&listener, display, whole_screen, touch, options, &mut child);

    stop_application(&mut child);
    let _ignored = fs::remove_file(&socket_path);
    result
}

/// Ends the application, politely if it has already finished and firmly if not.
fn stop_application(child: &mut Child) {
    let deadline = Instant::now() + APP_STOP_GRACE;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) => {}
            Err(_) => break,
        }
        if Instant::now() >= deadline {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    // Nothing an application started may outlive the session.
    let _ignored = child.kill();
    let _ignored = child.wait();
}

#[allow(clippy::too_many_arguments)]
/// Resolves a touch to the action it activates, if any.
///
/// Activation happens on release rather than on contact, so a finger that lands
/// on the wrong control can be slid away from it without acting. That is what
/// every touch interface the owner already uses has taught them to expect.
/// The way back is drawn in the top bar, so an application that did not ask
/// for one would otherwise trap the reader. The runtime supplies it rather
/// than trusting every application to remember, titled with the application's
/// own name so nothing is invented.
fn ensure_way_back(mut screen: Screen, chrome: Chrome, name: &str) -> Screen {
    if chrome.back && screen.top_bar.is_none() {
        screen = screen.with_top_bar(kobo_ui::TopBar::new(kobo_ui::NodeId(0), name));
    }
    screen
}

fn action_for(event: TouchEvent, screen: Option<&Screen>, chrome: Chrome) -> Option<ActionId> {
    let TouchEvent::Up { x, y } = event else {
        return None;
    };
    let screen = screen?;
    // A touch outside the signed range cannot be on any control, so it is
    // dropped rather than wrapped into a bogus coordinate.
    let (Ok(x), Ok(y)) = (i32::try_from(x), i32::try_from(y)) else {
        return None;
    };
    // The same chrome the frame was drawn with. Laying out with a different
    // one would move every control away from where the reader can see it.
    let hit = screen.layout_with(&CLARA_BW_METRICS, chrome).hit_test(x, y);
    // Reported so a tap that lands on nothing stays distinguishable from a tap
    // that never arrived at all. Diagnosing the difference without this cost a
    // whole debugging session.
    trace(&format!("touch up ({x},{y}) -> {hit:?}"));
    println!("touch up ({x},{y}) -> {hit:?}");
    hit
}

/// Accepts the application and completes the opening exchange.
///
/// The application is told the panel size rather than discovering it, so an
/// application binary is not tied to one model.
fn greet(
    listener: &std::os::unix::net::UnixListener,
    whole_screen: Rect,
) -> Result<(std::os::unix::net::UnixStream, String), String> {
    let (mut stream, _) = listener
        .accept()
        .map_err(|error| format!("application never connected: {error}"))?;
    let hello =
        kobo_protocol::read_from(&mut stream).map_err(|error| format!("first message: {error}"))?;
    let Message::Hello { name } = hello.message else {
        return Err("the first application message must be Hello".to_owned());
    };
    kobo_protocol::write_to(
        &mut stream,
        &Frame {
            request_id: hello.request_id,
            message: Message::Welcome {
                width: u16::try_from(whole_screen.width).unwrap_or(u16::MAX),
                height: u16::try_from(whole_screen.height).unwrap_or(u16::MAX),
                // The panel this runtime renders for. An application that
                // measures text has to measure it for the same one, and pixel
                // counts alone do not say how large a pixel is.
                pixels_per_inch: u16::try_from(CLARA_BW_METRICS.pixels_per_inch)
                    .unwrap_or(u16::MAX),
            },
        },
    )
    .map_err(|error| format!("welcome: {error}"))?;
    Ok((stream, name))
}

/// What the runtime, rather than the application, decides about a session.
struct SessionOptions {
    limits: Limits,
    profile: &'static DeviceProfile,
    chrome: Chrome,
    watchdog: Arc<Watchdog>,
}

/// Keeps the recovery watchdog fed from a thread, for the stretches where the
/// session loop is not running.
///
/// Only used during teardown. Using it for the session itself would defeat the
/// point: a heartbeat coming from a thread says the process exists, while a
/// heartbeat coming from the loop says the runtime is still doing its job.
struct KeepBeating {
    running: Arc<AtomicBool>,
}

impl KeepBeating {
    fn start(watchdog: &Arc<Watchdog>) -> Self {
        let running = Arc::new(AtomicBool::new(true));
        let stop = Arc::clone(&running);
        let watchdog = Arc::clone(watchdog);
        thread::spawn(move || {
            while stop.load(AtomicOrdering::Relaxed) {
                watchdog.beat();
                thread::sleep(BEAT_INTERVAL);
            }
        });
        Self { running }
    }
}

impl Drop for KeepBeating {
    fn drop(&mut self) {
        self.running.store(false, AtomicOrdering::Relaxed);
    }
}

/// Routes one tap. Reports whether the reader asked to leave the application.
///
/// Going back is the runtime's affordance, not the application's. An
/// application cannot draw it, cannot remove it and never sees the tap, which
/// is what makes it reliable enough to be the way out of anything.
fn deliver_touch(
    stream: &mut std::os::unix::net::UnixStream,
    event: TouchEvent,
    current: Option<&Screen>,
    chrome: Chrome,
) -> Result<bool, String> {
    let Some(action) = action_for(event, current, chrome) else {
        return Ok(false);
    };
    if action == ActionId::BACK {
        return Ok(true);
    }
    kobo_protocol::write_to(
        stream,
        &Frame {
            request_id: 0,
            message: Message::Action { action },
        },
    )
    .map_err(|error| format!("deliver a tap: {error}"))?;
    Ok(false)
}

/// The session event loop.
///
/// It stays one flat match over the message kinds rather than being split up.
/// Every path that can end a session, and every message that must be refused,
/// is then visible together, and the alternative is a helper taking eleven
/// arguments that reads worse than what it replaces.
#[allow(clippy::too_many_lines)]
fn converse(
    listener: &std::os::unix::net::UnixListener,
    display: &DisplaySession,
    whole_screen: Rect,
    touch: &TouchSink,
    options: SessionOptions,
    child: &mut Child,
) -> Result<Outcome, String> {
    let SessionOptions {
        limits,
        profile,
        chrome,
        watchdog,
    } = options;
    let _ = (profile, child);
    let (mut stream, name) = greet(listener, whole_screen)?;

    let (sender, events) = mpsc::channel();
    touch.set(Some(sender.clone()));
    pump_application(&stream, &sender)?;

    let mut surface = Surface::new(whole_screen.width as usize, whole_screen.height as usize);
    let mut current: Option<Screen> = None;
    let mut painted = 0_u32;
    let mut panel = Painter::new(surface.pixels.len());
    // Not `simulated()`. On the real panel that answered every battery read
    // with the same invented 72 percent, which is worse than refusing: an
    // application cannot tell an invented number from a measured one, so it
    // acts on it. This build performs exactly what it has a proven backend
    // for, which today is the read-only battery gauge and nothing else.
    let mut services = DeviceServices::new(
        Declared::all(),
        PowerPolicy::DEFAULT,
        match kobo_hal::battery::read() {
            Some(_) => Backends::with([Capability::BatteryRead]),
            None => Backends::none(),
        },
    );
    // Deliberately already stale, so the first read an application makes is a
    // real measurement rather than the default the services were built with.
    let mut battery_read_at = Instant::now()
        .checked_sub(BATTERY_INTERVAL)
        .unwrap_or_else(Instant::now);
    // Keyed state lives beside the applications, on the book partition, because
    // that is the one place a Kobo is guaranteed to have room and the one place
    // a reinstall does not wipe. An application that never saves creates
    // nothing here.
    let store = kobo_policy::store::Store::new(Path::new(STATE_ROOT).join(&name));
    let waker = sender.clone();
    let mut tasks = TaskRunner::simulated(std::env::temp_dir())
        .with_fetch(Arc::new(kobo_net::fetch_from))
        .with_post(Arc::new(kobo_net::post))
        .with_secrets(SECRETS)
        .with_wake(Arc::new(move || {
            let _ = waker.send(Event::TaskReady);
        }))
        // Granted to everything for now. This is the placeholder for the
        // manifest: capabilities belong to an installed application, and until
        // applications are installed rather than staged in `/tmp` there is
        // nothing to read a declaration from. It is written here, once, rather
        // than being absent, so that the day manifests arrive there is exactly
        // one line to change.
        .with_capabilities([kobo_policy::Capability::Network]);
    let ceiling = Instant::now() + limits.ceiling;
    let mut last_activity = Instant::now();

    loop {
        let now = Instant::now();
        // Reported from the loop rather than from a thread, so this says the
        // runtime is still serving the panel rather than merely that the
        // process has not been reaped.
        watchdog.beat();
        if now >= ceiling {
            tasks.shutdown();
            return Ok(Outcome {
                summary: format!(
                    "{name} reached the {} session limit after {painted} screens",
                    describe(limits.ceiling)
                ),
                next: None,
            });
        }
        let idle_at = last_activity + limits.idle;
        if now >= idle_at {
            tasks.shutdown();
            return Ok(Outcome {
                summary: format!(
                    "{name} was left alone for {} after {painted} screens, so the reader has it back",
                    describe(limits.idle)
                ),
                next: None,
            });
        }
        // Whichever comes first, and never longer than one heartbeat, so a
        // session nobody is touching still proves it is alive.
        let wait = ceiling
            .saturating_duration_since(now)
            .min(idle_at.saturating_duration_since(now))
            .min(BEAT_INTERVAL);
        match events.recv_timeout(wait) {
            // Both fall through to the drain below rather than continuing. A
            // heartbeat is a second chance to deliver a result, and a wake is
            // the first: the drain is the only delivery path either way.
            Err(RecvTimeoutError::Timeout) | Ok(Event::TaskReady) => {}
            Err(RecvTimeoutError::Disconnected) => {
                tasks.shutdown();
                return Ok(Outcome {
                    summary: format!("{name} ended after {painted} screens"),
                    next: None,
                });
            }
            Ok(Event::AppGone) => {
                tasks.shutdown();
                return Ok(Outcome {
                    summary: format!("{name} exited after {painted} screens"),
                    next: None,
                });
            }
            Ok(Event::Touch(event)) => {
                last_activity = Instant::now();
                if deliver_touch(&mut stream, event, current.as_ref(), chrome)? {
                    tasks.shutdown();
                    return Ok(Outcome {
                        summary: format!("{name} was closed after {painted} screens"),
                        next: None,
                    });
                }
            }
            Ok(Event::App(frame)) => match frame.message {
                Message::SetScreen(screen) => {
                    last_activity = Instant::now();
                    trace(&format!("screen {} received", screen.id));
                    println!("screen {}", screen.id);
                    let screen = ensure_way_back(screen, chrome, &name);
                    render_with(&screen, &CLARA_BW_METRICS, chrome, &mut surface, None);
                    panel.paint(display, whole_screen, &surface)?;
                    trace(&format!("screen {} painted", screen.id));
                    painted += 1;
                    current = Some(screen);
                }
                Message::Log { .. } => {}
                Message::DeviceRequest(request) => {
                    if matches!(request, kobo_protocol::DeviceRequest::ReadBattery)
                        && battery_read_at.elapsed() >= BATTERY_INTERVAL
                    {
                        if let Some(battery) = kobo_hal::battery::read() {
                            services.observe_battery(battery.percent, battery.charging);
                        }
                        battery_read_at = Instant::now();
                    }
                    let result = services.handle(request);
                    kobo_protocol::write_to(
                        &mut stream,
                        &Frame {
                            request_id: frame.request_id,
                            message: Message::DeviceResult(result),
                        },
                    )
                    .map_err(|error| format!("answer a device request: {error}"))?;
                }
                Message::Spawn { task, work } => {
                    println!("task {} started", task.0);
                    if tasks.submit(task, work).is_err() {
                        kobo_protocol::write_to(
                            &mut stream,
                            &Frame {
                                request_id: frame.request_id,
                                message: Message::TaskOutcome {
                                    task,
                                    outcome: TaskOutcome::Failed(TaskError::Denied),
                                },
                            },
                        )
                        .map_err(|error| format!("refuse a task: {error}"))?;
                    }
                }
                Message::StoreRequest(request) => {
                    let result = store.handle(&request);
                    kobo_protocol::write_to(
                        &mut stream,
                        &Frame {
                            request_id: frame.request_id,
                            message: Message::StoreResult(result),
                        },
                    )
                    .map_err(|error| format!("answer a store request: {error}"))?;
                }
                Message::Cancel { task } => tasks.cancel(task),
                Message::Exit => {
                    tasks.shutdown();
                    return Ok(Outcome {
                        summary: format!("{name} exited after {painted} screens"),
                        next: None,
                    });
                }
                // The application stops so the next one can have the panel.
                // Running both at once would mean two owners of one screen.
                Message::Launch { name: wanted } => {
                    tasks.shutdown();
                    return Ok(Outcome {
                        summary: format!("{name} handed over after {painted} screens"),
                        next: Some(wanted),
                    });
                }
                Message::Hello { .. }
                | Message::Welcome { .. }
                | Message::Action { .. }
                | Message::TaskOutcome { .. }
                | Message::DeviceResult(_)
                | Message::StoreResult(_) => {
                    tasks.shutdown();
                    return Err(format!("{name} sent a runtime-only message"));
                }
            },
        }
        for finished in tasks.drain() {
            println!(
                "task {} finished: {}",
                finished.task.0,
                describe_outcome(&finished.outcome)
            );
            kobo_protocol::write_to(
                &mut stream,
                &Frame {
                    request_id: 0,
                    message: Message::TaskOutcome {
                        task: finished.task,
                        outcome: finished.outcome,
                    },
                },
            )
            .map_err(|error| format!("report a finished task: {error}"))?;
        }
    }
}

/// One short word for a task outcome, for the session log.
///
/// Deliberately says how much came back rather than what came back: a task
/// body can be a credentialed reply and the log is not a place for it.
fn describe_outcome(outcome: &TaskOutcome) -> String {
    match outcome {
        TaskOutcome::Completed(bytes) => format!("{} bytes", bytes.len()),
        TaskOutcome::Failed(error) => format!("failed ({error:?})"),
        TaskOutcome::Cancelled => "cancelled".to_string(),
    }
}

/// Decides how each frame reaches the panel.
///
/// # Why this is not simply "write the pixels"
///
/// E Ink has no single correct update. A two-level waveform is fast but cannot
/// show grey at all; a full sixteen-level update shows everything but flashes
/// the screen and takes several times as long. Choosing wrongly is not a small
/// penalty: driving antialiased text with a two-level waveform crushes every
/// edge pixel to black or white and leaves the previous screen behind as
/// residue, which reads as a dirty, smeared panel.
///
/// So the waveform is chosen from the pixels themselves rather than from how
/// important the caller believes the frame to be.
struct Painter {
    /// The last frame written, for working out what actually changed.
    previous: Vec<u8>,
    /// Frames drawn since the last de-ghosting pass.
    since_flash: u32,
    started: bool,
}

/// Non-flashing updates leave a faint residue that accumulates. Clearing it
/// costs one full update, so it is worth doing regularly and not constantly.
const FLASH_INTERVAL: u32 = 8;

impl Painter {
    fn new(pixels: usize) -> Self {
        Self {
            previous: vec![0; pixels],
            since_flash: 0,
            started: false,
        }
    }

    /// The smallest rectangle covering every pixel that changed.
    ///
    /// Refreshing only this is both faster and cleaner, because the panel is
    /// only disturbed where the screen actually differs.
    fn changed(&self, surface: &Surface, whole_screen: Rect) -> Option<Rect> {
        let width = usize::try_from(whole_screen.width).ok()?;
        let (mut left, mut right) = (usize::MAX, 0usize);
        let (mut top, mut bottom) = (usize::MAX, 0usize);
        for (index, _) in surface
            .pixels
            .iter()
            .zip(self.previous.iter())
            .enumerate()
            .filter(|(_, (current, previous))| current != previous)
        {
            let (x, y) = (index % width, index / width);
            left = left.min(x);
            right = right.max(x);
            top = top.min(y);
            bottom = bottom.max(y);
        }
        if left > right {
            return None;
        }
        Some(Rect {
            x: u32::try_from(left).ok()?,
            y: u32::try_from(top).ok()?,
            width: u32::try_from(right - left + 1).ok()?,
            height: u32::try_from(bottom - top + 1).ok()?,
        })
    }

    /// Whether a region holds any tone the two-level waveform cannot show.
    fn has_grey(surface: &Surface, region: Rect, whole_screen: Rect) -> bool {
        let width = whole_screen.width as usize;
        (region.y..region.y.saturating_add(region.height))
            .flat_map(|y| {
                let row = (y as usize).saturating_mul(width);
                let start = row.saturating_add(region.x as usize);
                let end = start.saturating_add(region.width as usize);
                surface.pixels.get(start..end).unwrap_or(&[])
            })
            .any(|tone| *tone != 0 && *tone != 255)
    }

    fn paint(
        &mut self,
        display: &DisplaySession,
        whole_screen: Rect,
        surface: &Surface,
    ) -> Result<(), String> {
        // The first frame replaces the reader's own screen, so it is always a
        // full clean update; anything less leaves the reader showing through.
        let (region, intent) = if self.started {
            let Some(region) = self.changed(surface, whole_screen) else {
                // Nothing moved. Refreshing anyway costs a visible flicker and
                // some battery to show exactly the same picture.
                return Ok(());
            };
            if self.since_flash >= FLASH_INTERVAL {
                (whole_screen, RefreshIntent::QualityContent)
            } else if Self::has_grey(surface, region, whole_screen) {
                (region, RefreshIntent::TextContent)
            } else {
                (region, RefreshIntent::FastFeedback)
            }
        } else {
            (whole_screen, RefreshIntent::QualityContent)
        };

        let frame =
            RegionSnapshot::from_grayscale(display.geometry(), whole_screen, &surface.pixels)
                .map_err(|error| format!("prepare the frame: {error}"))?;
        display
            .restore(&frame)
            .map_err(|error| format!("write the frame: {error}"))?;
        let plan = RefreshPlan::new(
            region,
            intent,
            intent == RefreshIntent::QualityContent,
            whole_screen.width,
            whole_screen.height,
        )
        .ok_or_else(|| "the refresh region is not inside the screen".to_owned())?;
        display
            .refresh(plan)
            .map_err(|error| format!("show the frame: {error}"))?;

        if intent == RefreshIntent::QualityContent {
            self.since_flash = 0;
        } else {
            self.since_flash = self.since_flash.saturating_add(1);
        }
        self.previous.clear();
        self.previous.extend_from_slice(&surface.pixels);
        self.started = true;
        Ok(())
    }
}

/// Where panel touches are delivered right now.
///
/// There is exactly one reader thread on the touch descriptor for the whole
/// panel session, however many applications come and go. A thread per
/// application does not work: the receiver can only be taken once, so the
/// second application would receive nothing at all, and if it could be taken
/// twice the two threads would split every report between them. So the thread
/// is started once and the destination is swapped as applications change.
#[derive(Clone, Default)]
struct TouchSink(Arc<Mutex<Option<Sender<Event>>>>);

impl TouchSink {
    fn set(&self, sender: Option<Sender<Event>>) {
        // A poisoned lock still holds a usable destination, and losing touch
        // is worse than continuing past a panic in a thread that is already
        // gone.
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = sender;
    }

    fn send(&self, event: TouchEvent) {
        let guard = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(sender) = guard.as_ref() {
            // Between applications there is no destination, and a tap then is
            // deliberately dropped rather than queued: a tap meant for the
            // application that just closed must not act on the next one.
            let _ignored = sender.send(Event::Touch(event));
        }
    }
}

fn pump_touch(touch: &mut TouchSession, sink: &TouchSink) {
    let Some(events) = touch.take_events() else {
        return;
    };
    let sink = sink.clone();
    thread::spawn(move || {
        while let Ok(event) = events.recv() {
            sink.send(event);
        }
    });
}

fn pump_application(
    stream: &std::os::unix::net::UnixStream,
    sender: &Sender<Event>,
) -> Result<(), String> {
    let mut reader = stream
        .try_clone()
        .map_err(|error| format!("watch the application: {error}"))?;
    let sender = sender.clone();
    thread::spawn(move || loop {
        let Ok(frame) = kobo_protocol::read_from(&mut reader) else {
            let _ignored = sender.send(Event::AppGone);
            return;
        };
        if sender.send(Event::App(Box::new(frame))).is_err() {
            return;
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hello() -> Screen {
        Screen::new(1, Vec::new()).with_top_bar(kobo_ui::TopBar::new(kobo_ui::NodeId(1), "Hello"))
    }

    #[test]
    fn a_session_is_refused_before_the_reader_is_stopped_when_there_is_nothing_to_run() {
        let missing = std::env::temp_dir().join("kobo-does-not-exist");
        let _ignored = fs::remove_file(&missing);
        let error = preflight(&missing).expect_err("a missing application is refused");
        assert!(error.contains("nothing to run"), "{error}");
        // The message has to say the device is untouched, because the whole
        // point of checking here is that nothing has happened yet.
        assert!(error.contains("Nothing was changed"), "{error}");

        let directory = std::env::temp_dir().join(format!("kobo-preflight-{}", std::process::id()));
        fs::create_dir_all(&directory).expect("make a directory");
        assert!(preflight(&directory)
            .expect_err("a directory is refused")
            .contains("not a file"));

        let unreadable = directory.join("not-executable");
        fs::write(&unreadable, b"#!/bin/sh\n").expect("write a file");
        fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o644))
            .expect("clear the executable bits");
        assert!(preflight(&unreadable)
            .expect_err("a file that cannot be executed is refused")
            .contains("not executable"));

        fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o755))
            .expect("set the executable bits");
        assert!(preflight(&unreadable).is_ok());
        let _ignored = fs::remove_dir_all(&directory);
    }

    #[test]
    fn touches_follow_the_application_that_is_on_the_panel_now() {
        // The defect this covers: the touch receiver can only be taken once,
        // so a pump started per application left the second one with no input
        // at all. Every application after the first ignored every tap.
        let sink = TouchSink::default();
        let (first, launcher) = mpsc::channel();
        sink.set(Some(first));
        sink.send(TouchEvent::Up { x: 1, y: 1 });
        assert!(matches!(
            launcher.try_recv(),
            Ok(Event::Touch(TouchEvent::Up { x: 1, y: 1 }))
        ));

        let (second, application) = mpsc::channel();
        sink.set(Some(second));
        sink.send(TouchEvent::Up { x: 2, y: 2 });
        assert!(matches!(
            application.try_recv(),
            Ok(Event::Touch(TouchEvent::Up { x: 2, y: 2 }))
        ));
        // and not to the application that just closed.
        assert!(launcher.try_recv().is_err());

        // Between applications a tap is dropped rather than queued, so it
        // cannot act on whatever opens next.
        sink.set(None);
        sink.send(TouchEvent::Up { x: 3, y: 3 });
        assert!(application.try_recv().is_err());
    }

    #[test]
    fn the_runtime_draws_a_way_back_only_for_a_launched_application() {
        let home = PathBuf::from("/tmp/kobo-launcher");
        assert!(!Chrome::with_back(*home.as_path() != *home.as_path()).back);
        assert!(Chrome::with_back(Path::new("/tmp/kobo-hello") != home).back);
    }

    #[test]
    fn an_application_that_forgot_a_top_bar_still_gets_a_way_back() {
        let bare = Screen::new(1, Vec::new());
        let fixed = ensure_way_back(bare, Chrome::with_back(true), "Hello");
        assert_eq!(
            fixed.top_bar.as_ref().map(|bar| bar.title.as_str()),
            Some("Hello")
        );
        // The launcher itself is not given one it did not ask for.
        assert!(
            ensure_way_back(Screen::new(1, Vec::new()), Chrome::default(), "Launcher")
                .top_bar
                .is_none()
        );
    }

    #[test]
    fn back_is_reported_from_the_chrome_the_frame_was_drawn_with() {
        let screen = hello();
        let chrome = Chrome::with_back(true);
        let back = screen
            .layout_with(&CLARA_BW_METRICS, chrome)
            .nodes
            .iter()
            .find(|node| node.kind == kobo_ui::LayoutKind::Back)
            .map(|node| node.rect)
            .expect("a back affordance");
        let hit = action_for(
            TouchEvent::Up {
                x: u32::try_from(back.x + back.width / 2).expect("inside the panel"),
                y: u32::try_from(back.y + back.height / 2).expect("inside the panel"),
            },
            Some(&screen),
            chrome,
        );
        assert_eq!(hit, Some(ActionId::BACK));
        // Laid out without the affordance, the same tap must not invent one.
        assert_ne!(
            action_for(
                TouchEvent::Up {
                    x: u32::try_from(back.x + back.width / 2).expect("inside the panel"),
                    y: u32::try_from(back.y + back.height / 2).expect("inside the panel"),
                },
                Some(&screen),
                Chrome::default(),
            ),
            Some(ActionId::BACK)
        );
    }

    fn catalogue() -> PathBuf {
        let directory = std::env::temp_dir().join(format!("kobo-catalogue-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("make a catalogue");
        std::fs::write(directory.join("kobo-hello"), b"#!/bin/sh\n").expect("install an app");
        directory
    }

    #[test]
    fn an_installed_name_resolves() {
        let directory = catalogue();
        assert_eq!(
            resolve(&directory, "hello").expect("hello is installed"),
            directory.join("kobo-hello")
        );
    }

    #[test]
    fn a_name_that_is_not_installed_is_refused() {
        assert!(resolve(&catalogue(), "nothing-here").is_err());
    }

    #[test]
    fn a_name_may_not_escape_the_catalogue() {
        // An application that could name a path could start anything on the
        // device, so traversal has to fail on the name and not on the lookup.
        let directory = catalogue();
        for attempt in [
            "../../bin/sh",
            "..",
            "/bin/sh",
            "hello/../../../bin/sh",
            "hello;reboot",
            "hello sh",
            "",
        ] {
            assert!(
                resolve(&directory, attempt).is_err(),
                "{attempt:?} was accepted"
            );
        }
    }

    #[test]
    fn a_name_is_bounded_in_length() {
        assert!(resolve(&catalogue(), &"a".repeat(33)).is_err());
    }

    fn surface(width: usize, height: usize, fill: u8) -> Surface {
        let mut surface = Surface::new(width, height);
        surface.clear(fill);
        surface
    }

    const SCREEN: Rect = Rect {
        x: 0,
        y: 0,
        width: 8,
        height: 4,
    };

    #[test]
    fn an_unchanged_screen_needs_no_refresh() {
        // Refreshing an identical picture costs a visible flicker and battery.
        let mut painter = Painter::new(32);
        let frame = surface(8, 4, 255);
        painter.previous.copy_from_slice(&frame.pixels);
        assert!(painter.changed(&frame, SCREEN).is_none());
    }

    #[test]
    fn only_the_changed_rectangle_is_reported() {
        let painter = Painter::new(32);
        let mut frame = surface(8, 4, 0);
        // `previous` starts black, so painting one pixel white is the change.
        frame.pixels[2 * 8 + 3] = 255;
        assert_eq!(
            painter.changed(&frame, SCREEN),
            Some(Rect {
                x: 3,
                y: 2,
                width: 1,
                height: 1
            })
        );
    }

    #[test]
    fn grey_is_detected_so_the_two_level_waveform_is_avoided() {
        // This is the defect that made real type look dirty: a two-level
        // waveform cannot show an antialiased edge at all.
        let mut frame = surface(8, 4, 255);
        assert!(!Painter::has_grey(&frame, SCREEN, SCREEN));
        frame.pixels[9] = 96;
        assert!(Painter::has_grey(&frame, SCREEN, SCREEN));
    }

    #[test]
    fn grey_outside_the_changed_region_does_not_force_a_slower_waveform() {
        let mut frame = surface(8, 4, 255);
        frame.pixels[3 * 8 + 7] = 128;
        let top_left = Rect {
            x: 0,
            y: 0,
            width: 2,
            height: 2,
        };
        assert!(!Painter::has_grey(&frame, top_left, SCREEN));
    }
}

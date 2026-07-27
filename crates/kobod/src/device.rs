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
use kobo_protocol::{Frame, Lifecycle, Message, TaskError, TaskOutcome};
use kobo_ui::{
    display_metrics_from_env, render_all, ActionId, Chrome, FramePlanner, PanelWaveform,
    PictureCache, Screen, Surface,
};
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
/// The panel metrics a screen is drawn and hit-tested with.
///
/// A screen may ask for a text size other than the reader's own — a reader
/// adjusting the size of a book is the case this exists for. Every place that
/// lays this screen out has to agree, because layout is what decides where the
/// controls are: rendering at one size and hit-testing at another moves every
/// control away from where it can be seen.
fn metrics_for(screen: &Screen) -> kobo_ui::DisplayMetrics {
    let mut metrics = display_metrics_from_env();
    if let Some(scale) = screen.text_scale {
        metrics.text_scale = scale;
    }
    metrics
}

/// Whether this session was asked to put the Wi-Fi connection back itself.
///
/// Off unless stated, because the restarted reader owns the radio and does not
/// know what we would be starting behind it. Set `KOBO_KEEP_NETWORK=1` when the
/// session is being driven over Wi-Fi and losing the link would lose the
/// session — which is a developer working remotely, and nobody else.
fn keep_network_requested() -> bool {
    std::env::var_os("KOBO_KEEP_NETWORK").is_some_and(|value| value == "1" || value == "true")
}

/// Puts the front light back to where the session found it, on the way out.
///
/// Holds a clone rather than a borrow so that the loop can go on using the
/// light for as long as it runs; both refer to the same sysfs file and the same
/// remembered original.
struct FrontlightGuard(Option<kobo_hal::frontlight::Frontlight>);

impl Drop for FrontlightGuard {
    fn drop(&mut self) {
        if let Some(light) = &self.0 {
            if let Err(error) = light.restore() {
                trace(&format!("frontlight not restored: {error}"));
            }
        }
    }
}

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
/// How long an application that asked for first refusal on Back is given to
/// answer it with a screen.
///
/// The reader owns the way out, and this is the whole of what an application
/// is allowed to do with it: draw something new, quickly, or be left behind.
/// An application that is wedged, or that claimed [`Screen::owns_back`] on a
/// screen it has nowhere to go back from, costs the reader this much and then
/// the launcher appears anyway. Two seconds is longer than a screen takes to
/// build and shorter than a reader waits before tapping again.
const BACK_GRACE: Duration = Duration::from_secs(2);
/// How often the stop watcher looks at the flag a signal handler sets.
///
/// Bounds how long the owner holds a device that has been asked to stop and
/// has not finished handing anything back. Ten times a second is far below
/// what a panel refresh costs and far above what anybody can perceive.
const POLL_FOR_STOP: Duration = Duration::from_millis(100);

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
    App(u64, Box<Frame>),
    /// An application's end of the socket closed.
    AppGone(u64),
    /// A background task finished and its outcome is waiting to be drained.
    ///
    /// The loop otherwise only notices a finished task when something else
    /// wakes it, so an answer that had already arrived sat unread until the
    /// owner touched the panel. That reads as a hung application, and it is
    /// what made both a chat reply and a book download look stuck.
    TaskReady,
    /// The process was asked to stop, and the session has to end the ordinary
    /// way so the panel, the touch device, the reader and the freeze watchdog
    /// all go back.
    Stopping(i32),
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
    let typeface = match kobo_text::install(display_metrics_from_env()) {
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
    // Only when someone asked for it. Putting the connection back means
    // starting a supplicant and a DHCP client on `wlan0`, and the reader that
    // has just been restarted drives that same radio itself, from inside
    // libnickel, with no way to be told what we did. Two owners of one radio is
    // the mistake the display is careful to avoid twelve lines above, and it
    // has the same shape here: the reader's own network panel then finds an
    // interface it did not configure, and stops being able to scan at all — not
    // merely disconnected, but unable to list a network it has known for
    // months. A reboot clears it, as it clears everything here, but a reboot is
    // a poor thing to owe someone who only opened an application.
    //
    // So this is now what it always really was: a convenience for working on a
    // device over Wi-Fi, where losing the link means losing the session that is
    // driving it. That case is exactly the one that already had to say out loud
    // that a human is present, so it is gated on the same statement rather than
    // on a new one.
    let network = if keep_network_requested() {
        connection.restore(NETWORK_GRACE)
    } else {
        Ok(kobo_hal::network::Restored::Unaffected)
    };
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

/// The most applications kept alive at once.
///
/// Not a memory budget: it is what a reader can plausibly be switching between.
/// Beyond it, the one left alone longest is stopped, because an application
/// nobody has looked at in a while is cheaper to start again than a device that
/// runs out of memory while its owner is reading.
const MAX_HOSTED: usize = 4;

/// One application the runtime is hosting.
///
/// Every one of these owns a live process, its own socket, its own store and
/// its own background work. Only one of them owns the panel.
struct Hosted {
    /// Identity that survives the list being reordered. An index would not:
    /// applications are removed from the middle when they end.
    id: u64,
    name: String,
    path: PathBuf,
    child: Child,
    stream: std::os::unix::net::UnixStream,
    store: kobo_policy::store::Store,
    tasks: TaskRunner,
    /// The terminal this application may run a program on, or a refusal.
    shells: kobo_shell::Shells,
    /// The last screen this application drew, foreground or not.
    ///
    /// Held for every application rather than only the front one, because that
    /// is what makes coming back instant: the panel is repainted from this
    /// rather than the application being asked to draw itself again.
    screen: Option<Screen>,
    /// The pictures this application handed over, bounded and private to it.
    ///
    /// Per application rather than shared so that one application filling the
    /// cache cannot evict another's covers, and so that everything is released
    /// together when it exits.
    pictures: PictureCache,
    painted: u32,
    /// When this was last on the panel, for deciding what to stop first.
    used: Instant,
}

impl Hosted {
    fn send(&mut self, message: Message) -> Result<(), String> {
        kobo_protocol::write_to(
            &mut self.stream,
            &Frame {
                request_id: 0,
                message,
            },
        )
        .map_err(|error| format!("send to {}: {error}", self.name))
    }
}

/// Hosts applications on a panel that is already owned.
///
/// The display and the touch panel are taken once and held throughout, because
/// handing them back between applications would show the reader for a moment
/// and cost two full refreshes every time somebody opened something.
///
/// # Why applications are not stopped when you leave them
///
/// Leaving an application used to end its process, and coming back started it
/// again from nothing: a fresh load, a fresh fetch, and whatever the reader was
/// in the middle of, gone. On a device where starting costs a full refresh and
/// a reload, that made switching something to avoid.
///
/// So an application that loses the panel keeps everything except the panel. It
/// is told, so it can save; its work in flight keeps running and its answers
/// keep arriving; and what it draws is kept rather than shown. Coming back is
/// one repaint of a screen the runtime already has.
#[allow(clippy::too_many_lines)]
fn host_applications(
    application: &Path,
    display: &DisplaySession,
    whole_screen: Rect,
    touch: &TouchSink,
    limits: Limits,
    profile: &'static DeviceProfile,
    watchdog: &Arc<Watchdog>,
) -> Result<String, String> {
    let _ = profile;
    let catalogue = application
        .parent()
        .map_or_else(|| PathBuf::from("/tmp"), Path::to_path_buf);
    let home = application.to_path_buf();
    let socket_path = PathBuf::from(format!("/tmp/kobo-session-{}.sock", std::process::id()));
    let _ignored = fs::remove_file(&socket_path);
    let listener = std::os::unix::net::UnixListener::bind(&socket_path)
        .map_err(|error| format!("bind application socket: {error}"))?;

    let (sender, events) = mpsc::channel();
    touch.set(Some(sender.clone()));

    let mut apps: Vec<Hosted> = Vec::new();
    let mut next_id = 1_u64;
    let mut surface = Surface::new(whole_screen.width as usize, whole_screen.height as usize);
    let mut panel = Painter::new(surface.width, surface.height);
    // Not `simulated()`. On the real panel that answered every battery read
    // with the same invented 72 percent, which is worse than refusing: an
    // application cannot tell an invented number from a measured one, so it
    // acts on it. This build performs exactly what it has a proven backend
    // for, which today is the read-only battery gauge and nothing else.
    // Opened once and held for the session, because what it holds is the
    // reading taken before anything was changed. Reopening per request would
    // capture whatever the last application set as though it were the owner's
    // own setting, and the light would never go back.
    let frontlight = kobo_hal::frontlight::Frontlight::open();
    let mut backends = Vec::new();
    if kobo_hal::battery::read().is_some() {
        backends.push(Capability::BatteryRead);
    }
    if frontlight.is_some() {
        backends.push(Capability::FrontlightControl);
    }
    let mut services = DeviceServices::new(
        Declared::all(),
        PowerPolicy::DEFAULT,
        Backends::with(backends),
    );
    if let Some(light) = &frontlight {
        if let Some(percent) = light.percent() {
            services.observe_frontlight(percent);
        }
    }
    // A guard rather than a line at the end of the loop, because the loop has
    // several exits — the session clock, an idle reader, a failed write to an
    // application — and a front light left bright by whichever path was taken
    // is exactly the kind of change a reboot should not have to fix.
    let _restore_light = FrontlightGuard(frontlight.clone());
    // Deliberately already stale, so the first read an application makes is a
    // real measurement rather than the default the services were built with.
    let mut battery_read_at = Instant::now()
        .checked_sub(BATTERY_INTERVAL)
        .unwrap_or_else(Instant::now);

    // Installed before anything is taken, so there is no window where the
    // process holds the panel and cannot be asked for it back. A failure here
    // is reported and not fatal: without a handler this behaves exactly as it
    // did before, and the recovery watchdog still covers it.
    match kobo_hal::stop::catch_requests() {
        Ok(()) => watch_for_stop_requests(&sender),
        Err(error) => {
            println!("stop requests will not be caught ({error}); kill needs the watchdog");
        }
    }

    let result = (|| -> Result<String, String> {
        let front = start_application(
            &mut apps,
            &mut next_id,
            &home,
            &listener,
            whole_screen,
            &sender,
        )?;
        let mut front = front;
        let mut visited: Vec<String> = Vec::new();
        let ceiling = Instant::now() + limits.ceiling;
        let mut last_activity = Instant::now();
        // Set when Back has been handed to an application that asked for it,
        // and cleared by the next screen that application draws. The reader's
        // way out is never left waiting on an application: if this is still
        // set when its grace expires, the launcher is shown regardless.
        let mut back_offered: Option<(u64, Instant)> = None;
        // The rectangle currently drawn inverted because a finger is on it.
        let mut pressed: Option<kobo_ui::Rect> = None;

        loop {
            let now = Instant::now();
            // Reported from the loop rather than from a thread, so this says
            // the runtime is still serving the panel rather than merely that
            // the process has not been reaped.
            watchdog.beat();
            if now >= ceiling {
                return Ok(finish(
                    &apps,
                    &visited,
                    &format!("the {} session limit was reached", describe(limits.ceiling)),
                ));
            }
            let idle_at = last_activity + limits.idle;
            if now >= idle_at {
                return Ok(finish(
                    &apps,
                    &visited,
                    &format!(
                        "nothing was touched for {}, so the reader has it back",
                        describe(limits.idle)
                    ),
                ));
            }
            // An application that was offered Back and drew nothing has had
            // its turn. This is what keeps the guarantee: the way out belongs
            // to the reader whatever the application does or fails to do.
            if let Some((id, offered_at)) = back_offered {
                if now.saturating_duration_since(offered_at) >= BACK_GRACE {
                    back_offered = None;
                    if id == front {
                        trace("the application did not answer back, leaving anyway");
                        let Some(home_id) = id_of_path(&apps, &home) else {
                            return Ok(finish(&apps, &visited, "the launcher is gone"));
                        };
                        front = switch_to(
                            &mut apps,
                            front,
                            home_id,
                            display,
                            whole_screen,
                            &mut surface,
                            &mut panel,
                            &home,
                        )?;
                    }
                }
            }
            // Whichever comes first, and never longer than one heartbeat, so a
            // session nobody is touching still proves it is alive.
            let wait = ceiling
                .saturating_duration_since(now)
                .min(idle_at.saturating_duration_since(now))
                .min(BEAT_INTERVAL)
                .min(back_offered.map_or(BEAT_INTERVAL, |(_, offered_at)| {
                    (offered_at + BACK_GRACE).saturating_duration_since(now)
                }));
            match events.recv_timeout(wait) {
                Ok(Event::Stopping(number)) => {
                    return Ok(finish(
                        &apps,
                        &visited,
                        &format!(
                            "{} arrived, so the panel and the reader go back the ordinary way",
                            kobo_hal::stop::name(number)
                        ),
                    ));
                }
                // Both fall through to the drain below rather than continuing.
                // A heartbeat is a second chance to deliver a result, and a
                // wake is the first: the drain is the only delivery path.
                Err(RecvTimeoutError::Timeout) | Ok(Event::TaskReady) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return Ok(finish(&apps, &visited, "the runtime ran out of work"));
                }
                Ok(Event::AppGone(id)) => {
                    let Some(index) = index_of(&apps, id) else {
                        continue;
                    };
                    let gone = apps.remove(index);
                    visited.push(format!(
                        "{} exited after {} screens",
                        gone.name, gone.painted
                    ));
                    stop_hosted(gone);
                    if id == front {
                        // The first application ending ends the session: there
                        // is nothing behind it but the reader.
                        let Some(home_id) = id_of_path(&apps, &home) else {
                            return Ok(finish(&apps, &visited, "the launcher exited"));
                        };
                        front = switch_to(
                            &mut apps,
                            front,
                            home_id,
                            display,
                            whole_screen,
                            &mut surface,
                            &mut panel,
                            &home,
                        )?;
                    }
                }
                Ok(Event::Touch(event)) => {
                    last_activity = Instant::now();
                    let Some(index) = index_of(&apps, front) else {
                        return Ok(finish(&apps, &visited, "nothing is on the panel"));
                    };
                    let chrome = Chrome::with_back(apps[index].path != home);
                    let screen = apps[index].screen.clone();
                    // A control shows that it has been touched, before anything
                    // it does can be seen. Without this the panel is simply
                    // still for as long as the application takes to answer —
                    // which for anything that reaches the network is seconds —
                    // and the reader, given no evidence their finger landed,
                    // reasonably concludes it did not and taps again. Drawn by
                    // inverting the finished surface, so the planner sees a
                    // change of pure black and white in one small rectangle and
                    // picks the fast waveform for it.
                    if let Some(current) = screen.as_ref() {
                        match event {
                            TouchEvent::Down { x, y } => {
                                if let (Ok(x), Ok(y)) = (i32::try_from(x), i32::try_from(y)) {
                                    if let Some(rect) = current
                                        .layout_with(&metrics_for(current), chrome)
                                        .pressed_control(x, y)
                                    {
                                        surface.invert_rect(rect);
                                        panel.paint(display, whole_screen, &surface)?;
                                        pressed = Some(rect);
                                    }
                                }
                            }
                            TouchEvent::Up { .. } => {
                                // Put back before the action is delivered, so
                                // that an application which repaints does so
                                // over a control in its resting state rather
                                // than over an inverted one.
                                if let Some(rect) = pressed.take() {
                                    surface.invert_rect(rect);
                                    panel.paint(display, whole_screen, &surface)?;
                                }
                            }
                            TouchEvent::Move { x, y } => {
                                // Slid off the control. Cancel the press the
                                // way every other platform does, so the reader
                                // can see that letting go here will do nothing.
                                let off = match (i32::try_from(x), i32::try_from(y)) {
                                    (Ok(x), Ok(y)) => {
                                        pressed.is_some_and(|rect| !rect.contains(x, y))
                                    }
                                    _ => true,
                                };
                                if off {
                                    if let Some(rect) = pressed.take() {
                                        surface.invert_rect(rect);
                                        panel.paint(display, whole_screen, &surface)?;
                                    }
                                }
                            }
                        }
                    }
                    match deliver_touch(&mut apps[index].stream, event, screen.as_ref(), chrome)? {
                        Tap::Handled => {}
                        Tap::OfferedBack => back_offered = Some((front, Instant::now())),
                        Tap::Leave => {
                            // Going back leaves the application running. It is
                            // put behind the launcher rather than ended, so
                            // coming back to it is a repaint, not a restart.
                            back_offered = None;
                            let Some(home_id) = id_of_path(&apps, &home) else {
                                return Ok(finish(&apps, &visited, "the launcher is gone"));
                            };
                            front = switch_to(
                                &mut apps,
                                front,
                                home_id,
                                display,
                                whole_screen,
                                &mut surface,
                                &mut panel,
                                &home,
                            )?;
                        }
                    }
                }
                Ok(Event::App(id, frame)) => {
                    let Some(index) = index_of(&apps, id) else {
                        // A frame from an application that has already gone.
                        // Dropped rather than treated as an error: the read
                        // thread and the exit race by nature.
                        continue;
                    };
                    match frame.message {
                        Message::SetScreen(screen) => {
                            let is_front = id == front;
                            if is_front {
                                last_activity = Instant::now();
                            }
                            // The answer to a Back that was handed over, if
                            // one was outstanding. Cleared on any screen from
                            // that application rather than a designated one:
                            // the application has drawn, which is all the
                            // runtime asked of it.
                            if back_offered.is_some_and(|(waiting, _)| waiting == id) {
                                back_offered = None;
                            }
                            let chrome = Chrome::with_back(apps[index].path != home);
                            let screen = ensure_way_back(screen, chrome, &apps[index].name);
                            if is_front {
                                trace(&format!("screen {} received", screen.id));
                                println!("screen {}", screen.id);
                                // The surface is about to be drawn afresh, so
                                // whatever was inverted on it is gone. Forget
                                // it, or releasing the finger would invert a
                                // rectangle of the new screen instead.
                                pressed = None;
                                render_all(
                                    &screen,
                                    &metrics_for(&screen),
                                    chrome,
                                    &apps[index].pictures,
                                    &mut surface,
                                    None,
                                );
                                panel.paint(display, whole_screen, &surface)?;
                                apps[index].painted += 1;
                            }
                            // Kept either way. A background application that
                            // finished its work has a finished screen waiting,
                            // rather than the reader watching it be rebuilt.
                            apps[index].screen = Some(screen);
                        }
                        Message::PutPicture {
                            handle,
                            width,
                            height,
                            grey,
                        } => match apps[index].pictures.put_report(handle, width, height, grey) {
                            None => trace(&format!("picture {} refused", handle.0)),
                            Some(evicted) => trace_picture_evictions(handle, &evicted),
                        },
                        Message::BeginPicture {
                            handle,
                            width,
                            height,
                        } => {
                            if !apps[index].pictures.begin_upload(handle, width, height) {
                                trace(&format!("picture {} upload refused", handle.0));
                            }
                        }
                        Message::PictureChunk {
                            handle,
                            offset,
                            grey,
                        } => {
                            if !apps[index].pictures.upload_chunk(
                                handle,
                                usize::try_from(offset).unwrap_or(usize::MAX),
                                &grey,
                            ) {
                                trace(&format!("picture {} chunk refused", handle.0));
                            }
                        }
                        Message::CommitPicture { handle } => {
                            match apps[index].pictures.commit_upload(handle) {
                                None => trace(&format!("picture {} commit refused", handle.0)),
                                Some(evicted) => trace_picture_evictions(handle, &evicted),
                            }
                        }
                        Message::DropPicture { handle } => apps[index].pictures.remove(handle),
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
                            // The light is driven before the policy answers,
                            // so what the application is told is what the
                            // hardware actually took. Percentages do not divide
                            // evenly into every control's range, and an
                            // application that redraws a slider from the reply
                            // would otherwise drift away from the panel.
                            if let Some(light) = &frontlight {
                                match request {
                                    kobo_protocol::DeviceRequest::SetFrontlight { percent }
                                        if services.may(Capability::FrontlightControl) =>
                                    {
                                        match light.set(percent) {
                                            Ok(set) => services.observe_frontlight(set),
                                            Err(error) => {
                                                trace(&format!("frontlight refused: {error}"));
                                            }
                                        }
                                    }
                                    kobo_protocol::DeviceRequest::ReadFrontlight => {
                                        if let Some(percent) = light.percent() {
                                            services.observe_frontlight(percent);
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            let result = services.handle(request);
                            reply(
                                &mut apps[index],
                                frame.request_id,
                                Message::DeviceResult(result),
                            )?;
                        }
                        Message::StoreRequest(request) => {
                            let result = apps[index].store.handle(&request);
                            reply(
                                &mut apps[index],
                                frame.request_id,
                                Message::StoreResult(result),
                            )?;
                        }
                        Message::ShellRequest(request) => {
                            if let Some(event) = apps[index].shells.handle(request) {
                                reply(
                                    &mut apps[index],
                                    frame.request_id,
                                    Message::ShellEvent(event),
                                )?;
                            }
                        }
                        Message::Spawn { task, work } => {
                            println!("task {} started for {}", task.0, apps[index].name);
                            if apps[index].tasks.submit(task, work).is_err() {
                                reply(
                                    &mut apps[index],
                                    frame.request_id,
                                    Message::TaskOutcome {
                                        task,
                                        outcome: TaskOutcome::Failed(TaskError::Denied),
                                    },
                                )?;
                            }
                        }
                        Message::Cancel { task } => apps[index].tasks.cancel(task),
                        Message::Exit => {
                            let gone = apps.remove(index);
                            let ending = gone.path == home;
                            visited.push(format!(
                                "{} closed after {} screens",
                                gone.name, gone.painted
                            ));
                            let was_front = gone.id == front;
                            stop_hosted(gone);
                            if ending {
                                return Ok(finish(&apps, &visited, "the launcher was closed"));
                            }
                            if was_front {
                                let Some(home_id) = id_of_path(&apps, &home) else {
                                    return Ok(finish(&apps, &visited, "the launcher is gone"));
                                };
                                front = switch_to(
                                    &mut apps,
                                    front,
                                    home_id,
                                    display,
                                    whole_screen,
                                    &mut surface,
                                    &mut panel,
                                    &home,
                                )?;
                            }
                        }
                        Message::Launch { name: wanted } => {
                            match open_application(
                                &mut apps,
                                &mut next_id,
                                &catalogue,
                                &wanted,
                                &listener,
                                whole_screen,
                                &sender,
                                front,
                            ) {
                                Ok(opened) => {
                                    front = switch_to(
                                        &mut apps,
                                        front,
                                        opened,
                                        display,
                                        whole_screen,
                                        &mut surface,
                                        &mut panel,
                                        &home,
                                    )?;
                                }
                                // A launch that cannot be satisfied leaves the
                                // panel where it is. Ending the session instead
                                // would show the reader again, cost the owner
                                // half a minute and the network, and take every
                                // other application down with it, all because
                                // one entry was missing.
                                Err(error) => {
                                    println!("launch refused: {error}");
                                    visited.push(error);
                                }
                            }
                        }
                        Message::Hello { .. }
                        | Message::Welcome { .. }
                        | Message::Action { .. }
                        | Message::TaskOutcome { .. }
                        | Message::Lifecycle(_)
                        | Message::DeviceResult(_)
                        | Message::StoreResult(_)
                        | Message::ShellEvent(_) => {
                            return Err(format!(
                                "{} sent a runtime-only message",
                                apps[index].name
                            ));
                        }
                    }
                }
            }
            // Every application's work, not just the one on the panel. That is
            // the point of a background application: the answer arrives whether
            // or not anybody is looking at it.
            for app in &mut apps {
                // A terminal keeps running in the background for the same
                // reason a download does: a build that finishes while the
                // reader is elsewhere should still have finished.
                for event in app.shells.drain() {
                    app.send(Message::ShellEvent(event))?;
                }
                let finished = app.tasks.drain();
                for done in finished {
                    println!(
                        "task {} finished for {}: {}",
                        done.task.0,
                        app.name,
                        describe_outcome(&done.outcome)
                    );
                    app.send(Message::TaskOutcome {
                        task: done.task,
                        outcome: done.outcome,
                    })?;
                }
            }
        }
    })();

    for app in apps {
        stop_hosted(app);
    }
    let _ignored = fs::remove_file(&socket_path);
    result
}

fn index_of(apps: &[Hosted], id: u64) -> Option<usize> {
    apps.iter().position(|app| app.id == id)
}

fn id_of_path(apps: &[Hosted], path: &Path) -> Option<u64> {
    apps.iter().find(|app| app.path == path).map(|app| app.id)
}

fn reply(app: &mut Hosted, request_id: u32, message: Message) -> Result<(), String> {
    kobo_protocol::write_to(
        &mut app.stream,
        &Frame {
            request_id,
            message,
        },
    )
    .map_err(|error| format!("answer {}: {error}", app.name))
}

/// One line describing how the session ended and what ran during it.
fn finish(apps: &[Hosted], visited: &[String], why: &str) -> String {
    let mut parts: Vec<String> = visited.to_vec();
    for app in apps {
        parts.push(format!("{} drew {} screens", app.name, app.painted));
    }
    if parts.is_empty() {
        why.to_owned()
    } else {
        format!("{why}; {}", parts.join(", "))
    }
}

/// Brings `wanted` to the panel, telling both applications what happened.
#[allow(clippy::too_many_arguments)]
fn switch_to(
    apps: &mut [Hosted],
    front: u64,
    wanted: u64,
    display: &DisplaySession,
    whole_screen: Rect,
    surface: &mut Surface,
    panel: &mut Painter,
    home: &Path,
) -> Result<u64, String> {
    if front == wanted {
        return Ok(front);
    }
    if let Some(index) = index_of(apps, front) {
        // Told before the panel changes, so an application that saves on
        // leaving has done it before anything else is drawn over it.
        apps[index].send(Message::Lifecycle(Lifecycle::Background))?;
    }
    let Some(index) = index_of(apps, wanted) else {
        return Ok(front);
    };
    apps[index].used = Instant::now();
    apps[index].send(Message::Lifecycle(Lifecycle::Foreground))?;
    // Painted from what the runtime already holds rather than waiting for the
    // application to draw itself again. An application with nothing drawn yet
    // is the only case where the panel keeps the previous image for a moment,
    // and that is a genuinely new application rather than a returning one.
    if let Some(screen) = apps[index].screen.clone() {
        let chrome = Chrome::with_back(apps[index].path != home);
        render_all(
            &screen,
            &metrics_for(&screen),
            chrome,
            &apps[index].pictures,
            surface,
            None,
        );
        panel.paint(display, whole_screen, surface)?;
        apps[index].painted += 1;
    }
    Ok(wanted)
}

/// Finds an application by name, starting it only if it is not already running.
#[allow(clippy::too_many_arguments)]
fn open_application(
    apps: &mut Vec<Hosted>,
    next_id: &mut u64,
    catalogue: &Path,
    name: &str,
    listener: &std::os::unix::net::UnixListener,
    whole_screen: Rect,
    sender: &Sender<Event>,
    front: u64,
) -> Result<u64, String> {
    let path = resolve(catalogue, name)?;
    if let Some(id) = id_of_path(apps, &path) {
        return Ok(id);
    }
    if apps.len() >= MAX_HOSTED {
        evict(apps, front);
    }
    start_application(apps, next_id, &path, listener, whole_screen, sender)
}

/// Stops whichever background application has been left alone longest.
///
/// Never the one on the panel, and never the last one: the alternative to
/// stopping something is refusing to open anything, which is worse.
fn evict(apps: &mut Vec<Hosted>, front: u64) {
    let seen: Vec<(u64, Instant)> = apps.iter().map(|app| (app.id, app.used)).collect();
    let Some(index) = coldest(&seen, front) else {
        return;
    };
    let gone = apps.remove(index);
    println!("stopped {} to make room", gone.name);
    stop_hosted(gone);
}

/// Which hosted application has been left alone longest, if any may go.
///
/// Separated from the eviction itself so the rule can be tested without
/// starting four processes: the one on the panel is never a candidate, and
/// neither is an empty list.
fn coldest(seen: &[(u64, Instant)], front: u64) -> Option<usize> {
    seen.iter()
        .enumerate()
        .filter(|(_, (id, _))| *id != front)
        .min_by_key(|(_, (_, used))| *used)
        .map(|(index, _)| index)
}

/// Starts one application and completes its opening exchange.
fn start_application(
    apps: &mut Vec<Hosted>,
    next_id: &mut u64,
    path: &Path,
    listener: &std::os::unix::net::UnixListener,
    whole_screen: Rect,
    sender: &Sender<Event>,
) -> Result<u64, String> {
    let socket_path = listener
        .local_addr()
        .ok()
        .and_then(|address| address.as_pathname().map(Path::to_path_buf))
        .ok_or_else(|| "the application socket has no path".to_owned())?;
    let mut child = Command::new(path)
        .env_clear()
        .env("KOBO_SOCKET", &socket_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("start {}: {error}", path.display()))?;
    let (stream, name) = match greet(listener, whole_screen) {
        Ok(greeting) => greeting,
        Err(error) => {
            let _ignored = child.kill();
            let _ignored = child.wait();
            return Err(error);
        }
    };
    let id = *next_id;
    *next_id += 1;
    if let Err(error) = pump_application(&stream, sender, id) {
        let _ignored = child.kill();
        let _ignored = child.wait();
        return Err(error);
    }
    let waker = sender.clone();
    let tasks = TaskRunner::simulated(std::env::temp_dir())
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
    apps.push(Hosted {
        id,
        // Named explicitly, and only here. A shell on this device is root on a
        // writable root filesystem, so it is the one capability that is never
        // granted by the same blanket line as the rest; when manifests arrive
        // this becomes a declaration, not a wider default.
        shells: kobo_shell::Shells::new(if name == "terminal" {
            &[kobo_policy::Capability::Shell]
        } else {
            &[]
        })
        .waking({
            let waker = sender.clone();
            Arc::new(move || {
                let _ = waker.send(Event::TaskReady);
            })
        }),
        // Keyed state lives beside the applications, on the book partition,
        // because that is the one place a Kobo is guaranteed to have room and
        // the one place a reinstall does not wipe. An application that never
        // saves creates nothing here.
        store: kobo_policy::store::Store::new(Path::new(STATE_ROOT).join(&name)),
        name,
        path: path.to_path_buf(),
        child,
        stream,
        tasks,
        screen: None,
        pictures: PictureCache::default(),
        painted: 0,
        used: Instant::now(),
    });
    Ok(id)
}

/// Ends one hosted application and everything it started.
fn stop_hosted(mut app: Hosted) {
    app.tasks.shutdown();
    stop_application(&mut app.child);
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
    let hit = screen
        .layout_with(&metrics_for(screen), chrome)
        .hit_test(x, y);
    // Reported so a tap that lands on nothing stays distinguishable from a tap
    // that never arrived at all. Diagnosing the difference without this cost a
    // whole debugging session.
    trace(&format!("touch up ({x},{y}) -> {hit:?}"));
    println!("touch up ({x},{y}) -> {hit:?}");
    hit
}

fn trace_picture_evictions(handle: kobo_ui::PictureHandle, evicted: &[kobo_ui::PictureHandle]) {
    if evicted.is_empty() {
        return;
    }
    let evicted = evicted
        .iter()
        .map(|picture| picture.0.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    trace(&format!("picture {} stored; evicted {evicted}", handle.0));
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
                pixels_per_inch: u16::try_from(display_metrics_from_env().pixels_per_inch)
                    .unwrap_or(u16::MAX),
                text_scale: display_metrics_from_env().text_scale,
            },
        },
    )
    .map_err(|error| format!("welcome: {error}"))?;
    Ok((stream, name))
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

/// What a tap turned out to mean, once the runtime has had its say.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Tap {
    /// Nothing the runtime has to act on. Either it hit nothing, or it was an
    /// ordinary action already on its way to the application.
    Handled,
    /// The reader asked to leave the application.
    Leave,
    /// The reader asked to go back and the application asked for first refusal
    /// on that, so the action was delivered instead. The runtime now waits for
    /// a screen, and leaves anyway if none arrives.
    OfferedBack,
}

/// Routes one tap. Reports what the runtime has to do about it.
///
/// Going back is the runtime's affordance, not the application's: an
/// application cannot draw it and cannot remove it, which is what makes it
/// reliable enough to be the way out of anything. A screen may ask for first
/// refusal on it — see [`Screen::owns_back`] — so that a screen reached from
/// inside an application goes back to where it was reached from rather than
/// out of the application. That is a delivery, not a transfer of ownership:
/// the caller still leaves if no new screen follows.
fn deliver_touch(
    stream: &mut std::os::unix::net::UnixStream,
    event: TouchEvent,
    current: Option<&Screen>,
    chrome: Chrome,
) -> Result<Tap, String> {
    let Some(action) = action_for(event, current, chrome) else {
        return Ok(Tap::Handled);
    };
    let offered = action == ActionId::BACK;
    if offered && !current.is_some_and(|screen| screen.owns_back) {
        return Ok(Tap::Leave);
    }
    kobo_protocol::write_to(
        stream,
        &Frame {
            request_id: 0,
            message: Message::Action { action },
        },
    )
    .map_err(|error| format!("deliver a tap: {error}"))?;
    Ok(if offered {
        Tap::OfferedBack
    } else {
        Tap::Handled
    })
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
    frames: FramePlanner,
}

impl Painter {
    fn new(width: usize, height: usize) -> Self {
        Self {
            frames: FramePlanner::new(width, height),
        }
    }

    fn paint(
        &mut self,
        display: &DisplaySession,
        whole_screen: Rect,
        surface: &Surface,
    ) -> Result<(), String> {
        let Some(transition) = self.frames.plan(surface) else {
            // Nothing moved. Refreshing anyway costs a visible flicker and
            // some battery to show exactly the same picture.
            return Ok(());
        };
        let region = Rect {
            x: u32::try_from(transition.region.x).unwrap_or(0),
            y: u32::try_from(transition.region.y).unwrap_or(0),
            width: u32::try_from(transition.region.width).unwrap_or(0),
            height: u32::try_from(transition.region.height).unwrap_or(0),
        };
        let intent = match transition.waveform {
            PanelWaveform::Du => RefreshIntent::FastFeedback,
            PanelWaveform::Gl16 => RefreshIntent::TextContent,
            PanelWaveform::Gc16 => RefreshIntent::QualityContent,
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
            transition.full,
            whole_screen.width,
            whole_screen.height,
        )
        .ok_or_else(|| "the refresh region is not inside the screen".to_owned())?;
        display
            .refresh(plan)
            .map_err(|error| format!("show the frame: {error}"))?;

        if !self.frames.commit(surface, transition) {
            return Err("the frame planner rejected a completed refresh".to_owned());
        }
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

/// Turns a caught signal into an ordinary loop event.
///
/// A signal handler may not lock, allocate or send on a channel, so it only
/// records a number; this thread is what carries it into the loop. Polling is
/// the right shape here despite the comment on [`Event`] about staying asleep:
/// a tenth of a second of an idle thread costs nothing measurable next to the
/// panel, and the alternative — a self pipe — buys latency that a session
/// giving four pieces of hardware back cannot use.
///
/// The thread ends when it has delivered, and otherwise when the process does,
/// which is immediately after the one session this process ever runs.
fn watch_for_stop_requests(sender: &Sender<Event>) {
    let sender = sender.clone();
    thread::spawn(move || loop {
        if let Some(number) = kobo_hal::stop::requested() {
            let _ignored = sender.send(Event::Stopping(number));
            return;
        }
        thread::sleep(POLL_FOR_STOP);
    });
}

fn pump_application(
    stream: &std::os::unix::net::UnixStream,
    sender: &Sender<Event>,
    id: u64,
) -> Result<(), String> {
    let mut reader = stream
        .try_clone()
        .map_err(|error| format!("watch the application: {error}"))?;
    let sender = sender.clone();
    thread::spawn(move || loop {
        let Ok(frame) = kobo_protocol::read_from(&mut reader) else {
            let _ignored = sender.send(Event::AppGone(id));
            return;
        };
        if sender.send(Event::App(id, Box::new(frame))).is_err() {
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
            .layout_with(&display_metrics_from_env(), chrome)
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

    #[test]
    fn a_screen_that_asked_for_back_is_given_it_and_one_that_did_not_is_left() {
        // The defect this covers: the runtime swallowed Back entirely, so an
        // application with screens of its own could not return to the one the
        // reader came from. Tapping out of a book dropped them at the launcher
        // and reopening the application showed the book again, because its
        // retained screen had never changed.
        let chrome = Chrome::with_back(true);
        let screen = hello();
        let back = screen
            .layout_with(&display_metrics_from_env(), chrome)
            .nodes
            .iter()
            .find(|node| node.kind == kobo_ui::LayoutKind::Back)
            .map(|node| node.rect)
            .expect("a back affordance");
        let tap = TouchEvent::Up {
            x: u32::try_from(back.x + back.width / 2).expect("inside the panel"),
            y: u32::try_from(back.y + back.height / 2).expect("inside the panel"),
        };

        let (mut runtime, mut app) =
            std::os::unix::net::UnixStream::pair().expect("a pair of sockets");
        assert_eq!(
            deliver_touch(&mut runtime, tap, Some(&screen), chrome).expect("route the tap"),
            Tap::Leave,
            "a screen that did not ask keeps the old behaviour"
        );

        let owning = screen.clone().with_own_back(true);
        assert_eq!(
            deliver_touch(&mut runtime, tap, Some(&owning), chrome).expect("route the tap"),
            Tap::OfferedBack
        );
        let frame = kobo_protocol::read_from(&mut app).expect("the application is told");
        assert!(matches!(
            frame.message,
            Message::Action {
                action: ActionId::BACK
            }
        ));
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
}

#[cfg(test)]
mod hosting_tests {
    use super::coldest;
    use std::time::{Duration, Instant};

    fn ago(seconds: u64) -> Instant {
        Instant::now()
            .checked_sub(Duration::from_secs(seconds))
            .unwrap_or_else(Instant::now)
    }

    #[test]
    fn the_application_left_alone_longest_is_the_one_that_goes() {
        let seen = [(1, ago(30)), (2, ago(300)), (3, ago(5))];
        assert_eq!(coldest(&seen, 3), Some(1));
    }

    #[test]
    fn the_one_on_the_panel_is_never_stopped_even_when_it_is_the_oldest() {
        // The front application is the oldest here by a wide margin, because
        // `used` records when it was last brought forward rather than when it
        // was last touched. Stopping it would close what the reader is looking
        // at in order to open something else.
        let seen = [(1, ago(900)), (2, ago(10))];
        assert_eq!(coldest(&seen, 1), Some(1));
    }

    #[test]
    fn nothing_is_stopped_when_the_only_application_is_the_front_one() {
        let seen = [(7, ago(60))];
        assert_eq!(coldest(&seen, 7), None);
    }
}

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
use kobo_hal::gpio::{self, GpioEvent, GpioSession};
use kobo_hal::input::TouchSession;
use kobo_hal::reader::{Reader, Watchdog, WATCHDOG_CHECK};
use kobo_hal::soc_watchdog::SocWatchdog;
use kobo_hal::supervisor::Suspended;
use kobo_hal::touch::TouchEvent;
use kobo_hal::{Rect, RefreshIntent, RefreshPlan, RegionSnapshot};
use kobo_policy::{Backends, Capability, Declared, DeviceServices, PowerPolicy, TaskRunner};
use kobo_protocol::{Frame, Lifecycle, Message, TaskError, TaskOutcome};
use kobo_ui::{
    ActionId, CellStyle, Chrome, FontHandle, FramePlanner, Layout, LayoutKind, PanelWaveform,
    PictureCache, Screen, Surface,
};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering as AtomicOrdering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const COBALT_ROOT: &str = "/mnt/onboard/.adds/cobalt";
/// Where owner-managed global and app-scoped credentials live.
///
/// On the book partition, because that is the one place the owner can reach
/// over USB without a shell, and because `/tmp` is a RAM disk that every
/// reboot empties. An application names a secret; only the runtime reads one.
const SECRETS: &str = "/mnt/onboard/.adds/cobalt/secrets";

/// Where owner-installed TLS trust roots live, beside the credentials and for
/// the same reasons. A certificate here lets the runtime verify a daemon on
/// the owner's own network exactly as it verifies a public host.
const TRUST: &str = "/mnt/onboard/.adds/cobalt/trust";
const DICTIONARIES: &str = "/mnt/onboard/.adds/cobalt/dictionaries";

/// Turns on the per-frame timing line on stderr.
const FRAME_TIMING: &str = "KOBO_FRAME_TIMING";

/// Whether the owner asked for per-frame timing, read once for the process.
///
/// Once rather than per frame because the point of the line is to measure the
/// paint path, and an environment lookup inside the thing being measured is
/// exactly the wrong place for it.
fn frame_timing_wanted() -> bool {
    static WANTED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *WANTED.get_or_init(|| std::env::var(FRAME_TIMING).ok().as_deref() == Some("1"))
}

/// The most publisher faces one application may hold in the runtime at once.
///
/// The protocol bounds a single font frame, not how many frames arrive. A book
/// needs a regular, an italic, a bold and a bold italic, and a handful more for
/// small caps or a display face; past that an application is accumulating
/// rather than typesetting, and every face held is parsed outlines and a glyph
/// cache inside the privileged runtime.
const MAX_APP_FONTS: usize = 16;

/// Where each application's own keyed state lives, one directory per name.
const STATE_ROOT: &str = "/mnt/onboard/.adds/cobalt/state";

/// Where large application data lives.
///
/// Beside the state and for the same reasons, but kept apart from it so the
/// two can be reasoned about separately: state is small, permanent and cheap
/// to keep, while a shelf holds books that can be fetched again and is the
/// first thing to clear when the card is tight. A single directory holding
/// both would make "delete the downloads" indistinguishable from "forget where
/// I was".
///
/// This is the USER partition -- the one that appears when the reader is
/// plugged in -- not one of the internal partitions the firmware lives on.
/// Nothing here can stop the device booting.
const DATA_ROOT: &str = "/mnt/onboard/.adds/cobalt/data";

/// The panel metrics a screen is drawn and hit-tested with.
///
/// A screen may ask for a text size other than the reader's own, a reader
/// adjusting the size of a book is the case this exists for. Every place that
/// lays this screen out has to agree, because layout is what decides where the
/// controls are: rendering at one size and hit-testing at another moves every
/// control away from where it can be seen.
///
/// What the screen asks for is the size of its *prose*. The interface keeps
/// the reader's own accessibility scale, so a book set larger does not also
/// grow the bar above it and take the room out of the page.
fn metrics_for(screen: &Screen) -> kobo_ui::DisplayMetrics {
    let metrics = crate::device_metrics();
    // The typeface is installed once and lives as long as the process, so the
    // size it sets at has to be told to it rather than carried in the metrics
    // it was built with. Set here, where the screen's own answer is known, so
    // that measuring and drawing this frame cannot disagree.
    kobo_ui::set_text_scale(metrics.text_scale);
    kobo_ui::set_reading_scale(screen.text_scale.unwrap_or(metrics.text_scale));
    metrics
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
/// How long a session runs before the background update checker first asks
/// what is newer. Long enough that opening Cobalt to do one thing never
/// competes with a download for the radio; short enough that a session used
/// for an evening still gets its updates.
const AUTO_UPDATE_FIRST_CHECK: Duration = Duration::from_secs(2 * 60);
/// How long after one check the next one happens. Releases are published on
/// the scale of weeks, so asking more often than this buys nothing but radio
/// time.
const AUTO_UPDATE_RECHECK: Duration = Duration::from_secs(6 * 60 * 60);
/// How long the panel must have been left alone before found updates are
/// applied. Applying blocks the loop the way a store install does, so it only
/// happens when nobody is mid-anything on the screen.
const AUTO_UPDATE_QUIET: Duration = Duration::from_secs(60);
/// Below this charge, background updates wait for a charger. A failed write
/// to the book partition costs more than a late update is worth.
const AUTO_UPDATE_MIN_BATTERY: u8 = 20;
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

/// The wireless interface every Kobo names the same thing.
const WIFI_LINK: &str = "wlan0";

/// How often the band is re-read.
///
/// Separate from how often it is allowed to *change*, which is the mistake
/// this pair of constants exists to undo. The band used to be re-read only
/// when a frame was already being drawn, so unplugging the charger left the
/// old mark on the panel until something else happened to redraw. Nothing was
/// stale in the reading; the reading was simply never taken.
///
/// Looking is cheap: four small files the kernel publishes. Drawing is not, so
/// the loop looks often and repaints only when the value it drew has actually
/// changed.
const STATUS_POLL: Duration = Duration::from_secs(2);

/// How stale a status may be before a frame that is already being drawn takes
/// a fresh one.
const STATUS_INTERVAL: Duration = Duration::from_secs(60);

/// Reads the clock, the radio and the gauge, no more often than it needs to.
///
/// Held by the session rather than read per frame. Every reading here is from
/// a file the kernel publishes, so none of it needs `device-write` and none of
/// it can disturb the stock reader.
struct StatusSource {
    last: kobo_ui::Status,
    taken: Option<Instant>,
    polled: Option<Instant>,
    /// Whether something is connected over Bluetooth.
    ///
    /// Not polled with the rest. The controller on this device is the vendor
    /// MTK stack behind D-Bus, where asking costs a round trip and, worse, has
    /// side effects: reading the adapter marks the stack as used and commits
    /// the session to the slow reboot on hand-back. So this is told rather
    /// than asked, from the replies the daemon is already carrying.
    ///
    /// The cost is that headphones which wander out of range on their own are
    /// noticed the next time something reads Bluetooth rather than within two
    /// seconds. That is the right trade against making every session reboot.
    bluetooth: bool,
}

impl StatusSource {
    fn new() -> Self {
        Self {
            last: kobo_ui::Status::default(),
            taken: None,
            polled: None,
            bluetooth: false,
        }
    }

    /// Records what the daemon just learned about Bluetooth.
    ///
    /// Returns whether this changed anything, so the caller can repaint on the
    /// same footing as [`StatusSource::poll`] without a second code path.
    fn observe_bluetooth(&mut self, connected: bool) -> bool {
        if self.bluetooth == connected {
            return false;
        }
        self.bluetooth = connected;
        self.last.bluetooth = connected;
        true
    }

    /// Takes a fresh reading, and says whether anything a reader can see moved.
    ///
    /// One comparison covers the clock, the radio, the gauge, the charging
    /// mark and Bluetooth, because they are five fields of one value. Adding a
    /// sixth mark to the band needs no change here at all, which is the point:
    /// the alternative is five timers and five remembered previous values that
    /// drift apart the first time one of them is forgotten.
    fn poll(&mut self) -> bool {
        if self
            .polled
            .is_some_and(|polled| polled.elapsed() < STATUS_POLL)
        {
            return false;
        }
        self.polled = Some(Instant::now());
        let fresh = self.read();
        if fresh == self.last {
            return false;
        }
        self.last = fresh;
        self.taken = Some(Instant::now());
        true
    }

    /// The current status, re-read only when it has gone stale.
    fn get(&mut self) -> &kobo_ui::Status {
        let stale = self
            .taken
            .is_none_or(|taken| taken.elapsed() >= STATUS_INTERVAL);
        if stale {
            self.last = self.read();
            self.taken = Some(Instant::now());
        }
        &self.last
    }

    fn read(&self) -> kobo_ui::Status {
        kobo_ui::Status {
            bluetooth: self.bluetooth,
            ..read_status()
        }
    }
}

/// The chrome a screen from `app` is drawn with.
///
/// The band is withheld from a reading screen. A book is a book: the stock
/// reader hides its own status bar the moment a page is opened, and a clock
/// ticking above a novel is both a distraction and a panel update per minute
/// for something nobody opened the book to see.
/// The back control is drawn for an application that is not the launcher, and
/// also for any screen that asked for Back itself. The second case used to be
/// missing, and it stranded people: a rooted application, which is what
/// `kobo present` and a single-application install both produce, is "at home",
/// so its sub-screens were drawn with no way back at all. Settings could open
/// Bluetooth and then had nothing to return to Settings with. The screen had
/// said `owns_back`, which is an application declaring it has somewhere of its
/// own to go, so drawing the control is exactly what it asked for.
fn chrome_for(screen: &Screen, at_home: bool, status: &mut StatusSource) -> Chrome {
    let chrome = Chrome::with_back(!at_home || screen.owns_back);
    if screen.reading {
        return chrome;
    }
    chrome.with_status(status.get().clone())
}

/// Assembles one reading of everything the band shows.
fn read_status() -> kobo_ui::Status {
    let battery = kobo_hal::battery::read();
    kobo_ui::Status {
        clock: clock(),
        // A radio with no default route is not a usable connection however
        // strong the association is, so reachability is checked before
        // strength. Showing three arcs on a device that cannot load a page is
        // the one thing this mark must never do.
        signal: if kobo_hal::network::is_online(WIFI_LINK) {
            kobo_hal::network::signal_dbm(WIFI_LINK)
                .map_or(kobo_ui::Signal::Weak, kobo_ui::Signal::from_dbm)
        } else {
            kobo_ui::Signal::Off
        },
        battery: battery.map(|battery| kobo_ui::Percent::new(battery.percent)),
        charging: battery.is_some_and(|battery| battery.charging),
        // Filled in by the caller, which is the only layer holding what the
        // daemon has been told about the controller.
        bluetooth: false,
    }
}

/// The wall clock as `HH:MM`, or empty when it cannot be read.
///
/// Computed from the system clock without pulling in a date library: the band
/// needs hours and minutes in local time and nothing else. The offset comes
/// from the `TZ` the firmware already sets, read once per call because a
/// reader who crosses a timezone should not have to restart anything.
fn clock() -> String {
    let Ok(since_epoch) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) else {
        // A clock before 1970 is a device whose time was never set. Blank is
        // the honest answer; a wrong time is worse than no time, because a
        // reader will believe it.
        return String::new();
    };
    // Seconds since 1970 does not reach i64 for another 292 billion years,
    // so the only way this conversion fails is a clock that is already wrong.
    let Ok(seconds) = i64::try_from(since_epoch.as_secs()) else {
        return String::new();
    };
    let seconds = seconds + local_offset_seconds();
    let day = seconds.rem_euclid(86_400);
    format!("{:02}:{:02}", day / 3600, (day % 3600) / 60)
}

/// Seconds to add to UTC for local time.
///
/// Read from `TZ` in the `<NAME><offset>` form that POSIX specifies and that
/// the firmware writes, where the sign is inverted from what everyone expects:
/// `EST5` is five hours *behind* UTC. Anything not understood is treated as
/// UTC rather than guessed at.
fn local_offset_seconds() -> i64 {
    let Ok(tz) = std::env::var("TZ") else {
        return 0;
    };
    let rest = tz.trim_start_matches(|character: char| character.is_ascii_alphabetic());
    let (sign, digits) = match rest.strip_prefix('-') {
        Some(digits) => (1, digits),
        None => (-1, rest.strip_prefix('+').unwrap_or(rest)),
    };
    let mut parts = digits.split(':');
    let Some(Ok(hours)) = parts.next().map(str::parse::<i64>) else {
        return 0;
    };
    let minutes = parts.next().and_then(|part| part.parse::<i64>().ok());
    sign * (hours * 3600 + minutes.unwrap_or(0) * 60)
}
/// How long to wait for the restarted reader to feed the freeze watchdog
/// before handing it back regardless.
///
/// The reader takes tens of seconds to reach its first ping, and the watchdog
/// reboots the device ten seconds after being resumed if nothing feeds it, so
/// this has to be generous. Waiting longer only delays the summary; waiting
/// too little reboots the device.
const WATCHDOG_HANDBACK: Duration = Duration::from_secs(90);

/// How long the application is given to exit before it is killed.
const APP_STOP_GRACE: Duration = Duration::from_secs(3);

/// What the runtime is waiting for. Both sources feed one channel so the
/// runtime blocks rather than polling; a poll loop would keep the processor
/// awake between taps, which on a device that idles at zero power costs real
/// battery life.
enum Event {
    Touch(TouchEvent),
    /// A button or orientation report from the `gpio-keys` node.
    Gpio(GpioEvent),
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
    /// The background checker found software newer than what is running.
    ///
    /// Carried into the loop rather than applied on the checker's thread,
    /// because applying replaces binaries and stops applications, and only
    /// the loop knows whether the panel is quiet enough for that.
    AutoUpdate(crate::autoupdate::Plan),
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

    // Checked here, before anything is taken over. Stopping the reader costs
    // the owner half a minute and the network connection, so discovering only
    // afterwards that there was nothing to run is the worst possible order.
    // This is the likeliest failure of all: `/tmp` is a tmpfs, so every staged
    // application disappears on a reboot.
    preflight(application)?;

    // Owner-installed trust roots, before the first request could build the
    // TLS configuration without them. Zero on almost every reader.
    let trusted = kobo_net::trust_owner_roots_from_dir(Path::new(TRUST));
    if trusted > 0 {
        trace(&format!("trust: {trusted} owner root(s) installed"));
    }

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
    let network = kobo_hal::network::Connection::capture();
    let state = PathBuf::from(format!("/tmp/kobo-session-{}", std::process::id()));
    reader
        .save(&state)
        .map_err(|error| format!("save reader description: {error}"))?;
    let watchdog = Arc::new(
        Watchdog::arm(&state, WATCHDOG_CHECK).map_err(|error| format!("arm watchdog: {error}"))?,
    );

    let display = DisplaySession::open(Some(OWNER_UNLOCK_PHRASE))
        .map_err(|error| format!("open display: {error}"))?;
    let profile = display.profile();
    crate::remember_device_profile(profile)?;

    // The display's exact profile is now retained for every later layout and
    // hit test. Installing a face may fail, but that is not fatal: `kobo-ui`
    // keeps its built-in bitmap, so the worst case is ugly text rather than a
    // dead session.
    let typeface = match kobo_text::install(crate::device_metrics()) {
        Ok(path) => path.file_name().map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        ),
        Err(error) => format!("none ({error})"),
    };

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

    let touch_path = display
        .snapshot()
        .touch
        .as_ref()
        .map(|t| t.path.clone())
        .ok_or_else(|| "touch probe was unavailable".to_owned())?;

    let framebuffer = display
        .snapshot()
        .framebuffer
        .as_ref()
        .ok_or_else(|| "framebuffer probe was unavailable".to_owned())?;
    // Resolved rather than assumed: the transform that places every tap is
    // only correct at the orientation it was measured at, so a reader held the
    // other way up has to refuse the session rather than mislocate touches.
    let pose = kobo_profile::PanelPose::resolve(profile, framebuffer)
        .map_err(|error| format!("take the touch panel: {error}"))?;
    let mut touch = TouchSession::acquire(Path::new(&touch_path), pose)
        .map_err(|error| format!("take the touch panel: {error}"))?;

    // Without this the device reboots itself partway through the session, so a
    // refusal here is fatal and the reader is left running.
    let suspended = Suspended::suspend(reader.environment("DBUS_SESSION_BUS_ADDRESS"))
        .map_err(|error| format!("suspend the freeze watchdog: {error}"))?;

    // The SoC's own counter gets the same treatment, and for the same span. It
    // is fed by a kernel thread every 28 seconds against a 31 second timeout,
    // and stopping and restarting the reader is the heaviest thing this device
    // ever does. Those three seconds are the entire margin, and a session
    // spends them: with the counter armed a session ended in a cold boot around
    // ten seconds after the panel went back, and with it slack the same session
    // ran through and kept going. `soc_watchdog` carries the measurements.
    //
    // A device that lacks the node is not a failure, so this only refuses when
    // the node is there and will not answer.
    let slack = SocWatchdog::default()
        .slacken()
        .map_err(|error| format!("slacken the hardware watchdog: {error}"))?;

    // The point of no return.
    trace("stopping the reader");
    reader
        .stop(STOP_GRACE)
        .map_err(|error| format!("stop the reader: {error}"))?;

    // Nickel owns Wi-Fi while it runs, but its supplicant and DHCP client are
    // detached processes. Capture them before the handoff and restore exactly
    // those processes if stopping Nickel drops the route. Cobalt never invents
    // a network or starts a daemon that was not already serving the owner.
    if network.was_online() {
        if let Some(wifi) = kobo_hal::wifi::Wifi::open() {
            let reconnected = wifi.set_enabled(true);
            trace(&format!(
                "asked the captured Wi-Fi owner to reconnect: {reconnected:?}"
            ));
        }
    }
    let session_network = network.restore_for_session(Duration::from_secs(30));
    trace(&format!(
        "session network: {:?}; uncertain captures: {:?}; start errors: {:?}",
        session_network.outcome(),
        session_network.uncertain_executables(),
        session_network.start_errors()
    ));

    // One reader thread on the touch descriptor for the whole panel session,
    // started here rather than per application.
    let taps = TouchSink::default();
    pump_touch(&mut touch, &taps);

    // The buttons and the orientation channel, on hardware that has them.
    // Absence is not a failure: not every supported device has page keys,
    // and a session without buttons is the state every session was in
    // before this existed.
    let mut buttons =
        gpio::discover_buttons_path().and_then(|path| match GpioSession::acquire(&path) {
            Ok(session) => Some(session),
            Err(error) => {
                trace(&format!("buttons unavailable: {error}"));
                None
            }
        });
    if let Some(session) = buttons.as_mut() {
        pump_gpio(session, &taps);
    }

    // Which page key means "forward" depends on how the reader is held. At
    // the profile's reference pose (buttons on the right, on the Libra 2)
    // key 194 pages forward and 193 pages back, read off the hardware: with
    // the buttons on the right the upper key is 193 and goes back, the lower
    // key is 194 and goes forward.
    //
    // The behaviour was checked the same way: a session paged as expected in
    // both portrait poses, and a half turn mid-session inverted it correctly
    // with no restart. Pose resolution has already refused anything but the
    // two portrait poses.
    let forward_is_194 = pose.rotation() % 4 == profile.reference_rotation % 4;

    let application_outcome = host_applications(
        application,
        &display,
        whole_screen,
        &taps,
        limits,
        forward_is_194,
        &watchdog,
    );
    // No panel work may outlive Cobalt's ownership of the display. This is an
    // explicit lifecycle fence rather than a timing assumption: the stock
    // reader can start drawing as soon as the session gives the descriptor
    // back.
    let panel_idle = display
        .finish_pending()
        .map_err(|error| format!("finish panel updates before handoff: {error}"));
    let outcome = match (application_outcome, panel_idle) {
        (Ok(summary), Ok(_)) => Ok(summary),
        (Err(error), _) | (_, Err(error)) => Err(error),
    };
    trace("session finished, handing the panel back");
    println!("session finished, handing the panel back");

    // Teardown takes minutes in the worst case (the reader is given forty-five
    // seconds to come back, the network thirty, and the freeze watchdog ninety
    // more) and none of that runs the loop that normally reports progress.
    // Without this the recovery watchdog would conclude the runtime had died
    // and restart a reader that is already starting.
    let teardown = KeepBeating::start(&watchdog);
    let captured_supplicant_release_failed = if profile.reap_nickel_supplicant {
        trace("stopping the captured leftover supplicant before the reader returns");
        if let Err(error) =
            network.release_captured(kobo_hal::network::SUPPLICANT_EXECUTABLE, STOP_GRACE)
        {
            trace(&format!("the leftover supplicant would not stop: {error}"));
            true
        } else {
            false
        }
    } else {
        false
    };
    let network_start_uncertain = !session_network.start_errors().is_empty()
        || !session_network.uncertain_executables().is_empty();
    let network_release_failed = match session_network.release(STOP_GRACE) {
        Ok(()) => false,
        Err(errors) => {
            trace(&format!(
                "session network would not stop cleanly: {errors:?}"
            ));
            true
        }
    };
    // Asked once, before the panel is given up, because the answer decides
    // whether the owner is owed an explanation and the display is gone by the
    // time the reboot itself is requested.
    let bluetooth_reboot = kobo_hal::bluetooth::requires_reboot_after_use();
    let reboot_reason = if bluetooth_reboot {
        Some((
            kobo_ui::Glyph::Bluetooth,
            "Bluetooth shares one radio with Wi-Fi here, and that radio can only be \
             started once per boot. Restarting is the only way to hand it back working. \
             This is expected, it is not a crash, and everything you have saved is \
             already on the disk.",
            "Bluetooth used the shared MediaTek radio",
        ))
    } else if network_start_uncertain
        || network_release_failed
        || captured_supplicant_release_failed
    {
        Some((
            kobo_ui::Glyph::Wifi,
            "Cobalt could not prove that Wi-Fi has exactly one owner. Restarting avoids \
             handing the radio to competing processes. This is expected, it is not a \
             crash, and everything you have saved is already on the disk.",
            "Wi-Fi ownership could not be handed back safely",
        ))
    } else {
        None
    };
    let rebooting = reboot_reason.is_some();
    if rebooting {
        let (glyph, summary, _) = reboot_reason.expect("reboot reason");
        if let Err(error) = announce_reboot(&display, whole_screen, glyph, summary) {
            // Not fatal. Failing to explain the reboot is worse than not
            // rebooting, but it is not a reason to leave the radio broken.
            trace(&format!("could not show the restart notice: {error}"));
        }
    }
    // Reverse order, on every path.
    let restored = restore_screen(&display, &backup, whole_screen);
    let _ignored = touch.release();
    // The panel and the touch descriptor are given up *before* the reader is
    // started, not after it. Holding the display open while the reader brings
    // the EPD controller back up leaves two owners of one piece of hardware.
    //
    // This ordering was originally credited with fixing a reset that happened
    // about thirty seconds later. That credit was misplaced: the reset was the
    // SoC watchdog, it happens with no display session open at all, and it is
    // handled by `slack` above. The ordering stays because two owners of one
    // controller is wrong on its own terms, not because it fixes that.
    drop(touch);
    drop(display);
    // The Clara BW's MediaTek Bluetooth driver cannot be initialised twice in
    // one boot. If Cobalt changed or scanned that stack, starting Nickel here
    // can panic the kernel inside wlan_drv_gen4m. A normal, synced reboot is
    // the only proven hand-back: it returns directly to the stock reader with
    // a pristine shared Wi-Fi/Bluetooth driver state.
    if rebooting {
        let (_, _, detail) = reboot_reason.expect("reboot reason");
        trace(&format!(
            "{detail}; rebooting cleanly instead of restarting the reader"
        ));
        println!("shared hardware needs a clean handoff; rebooting back to the reader");
        watchdog.disarm();
        drop(teardown);
        let _ignored = fs::remove_dir_all(&state);
        let summary = outcome.unwrap_or_else(|error| format!("application ended: {error}"));
        return request_clean_reboot().map(|()| {
            format!(
                "{summary}; typeface {typeface}; {detail}, so a clean reboot was requested before returning to the stock reader"
            )
        });
    }
    trace("panel and touch released, restarting the reader");
    println!("panel released, restarting the reader");
    let restarted = match reader.start(START_GRACE) {
        Ok(pid) => pid,
        Err(error) => {
            trace(&format!(
                "the reader did not restart ({error}); requesting a clean reboot"
            ));
            watchdog.disarm();
            drop(teardown);
            let _ignored = fs::remove_dir_all(&state);
            let summary = outcome.unwrap_or_else(|error| format!("application ended: {error}"));
            return request_clean_reboot().map(|()| {
                format!(
                    "{summary}; typeface {typeface}; the reader did not restart ({error}), so a clean reboot was requested"
                )
            });
        }
    };
    // Any daemon Cobalt started for the session was stopped by exact captured
    // identity above. A stop or capture uncertainty takes the clean-reboot
    // path, so Nickel is never started on top of an unproven network owner.
    trace("reader restart returned, waiting for it to feed the freeze watchdog");
    println!("waiting for the reader to feed the freeze watchdog");
    let reader_wifi = restore_reader_wifi(network.was_online(), Duration::from_secs(45));
    if !reader_wifi {
        trace("the reader did not complete Wi-Fi association; requesting a clean reboot");
        watchdog.disarm();
        drop(teardown);
        let _ignored = fs::remove_dir_all(&state);
        let summary = outcome.unwrap_or_else(|error| format!("application ended: {error}"));
        return request_clean_reboot().map(|()| {
            format!(
                "{summary}; typeface {typeface}; the reader did not reclaim Wi-Fi, so a clean reboot was requested"
            )
        });
    }
    // Resumed only once the reader is feeding it again. Resuming the moment
    // the process exists lights a ten second fuse that a still-starting reader
    // cannot feed, which is what rebooted the device at the end of a session.
    let resumed = suspended.resume_once_fed(WATCHDOG_HANDBACK);
    // Armed again only now. The reader has been given the panel back and has
    // proved it is feeding the freeze watchdog, which is the best evidence
    // available that it is far enough along to survive being timed. Dropping
    // this guard would arm it too, on any early return or panic above.
    let rearmed = slack.rearm();
    watchdog.disarm();
    drop(teardown);
    let _ignored = fs::remove_dir_all(&state);

    let reader_state = match resumed {
        Ok(after) => {
            format!("the reader is running again as pid {restarted}, and the freeze watchdog was resumed {after}")
        }
        Err(error) => format!(
            "the reader is running again as pid {restarted}, but the freeze watchdog could not be resumed ({error}); it returns on the next reboot"
        ),
    };
    let reader_state =
        format!("{reader_state}; the Wi-Fi connection is the reader's own again, so reconnect from its network screen if it does not return by itself");
    // Worth saying out loud rather than swallowing. The device is running
    // without its hardware watchdog until it is rebooted, which is a real loss
    // even though the kernel arms it again on the next boot.
    let reader_state = match rearmed {
        Ok(()) => reader_state,
        Err(error) => format!(
            "{reader_state}; the hardware watchdog could not be armed again ({error}), so it stays slack until the next reboot"
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

fn restore_reader_wifi(was_online: bool, within: Duration) -> bool {
    if !was_online {
        return true;
    }
    let deadline = Instant::now() + within;
    let mut retry_at = Instant::now();
    let mut healthy_since = None;
    loop {
        if let Some(wifi) = kobo_hal::wifi::Wifi::open() {
            let associated = wifi.associated().unwrap_or(false);
            let healthy =
                associated && kobo_hal::network::is_online(kobo_hal::network::WIRELESS_LINK);
            if healthy {
                let since = healthy_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= Duration::from_secs(10) {
                    return true;
                }
            } else {
                healthy_since = None;
                if !associated && Instant::now() >= retry_at {
                    if let Err(error) = wifi.recover_association() {
                        trace(&format!(
                            "the reader Wi-Fi recovery sequence was not accepted: {error:?}"
                        ));
                    }
                    retry_at = Instant::now() + Duration::from_secs(5);
                }
            }
        } else {
            healthy_since = None;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(500));
    }
}

/// Syncs user storage and requests the firmware's ordinary reboot path.
fn request_clean_reboot() -> Result<(), String> {
    let sync = Command::new("sync")
        .status()
        .map_err(|error| format!("start sync before Bluetooth reboot: {error}"))?;
    if !sync.success() {
        return Err("sync failed before Bluetooth reboot; power-cycle the reader".to_owned());
    }
    for tool in ["/sbin/reboot", "/bin/reboot", "/usr/sbin/reboot"] {
        if Path::new(tool).is_file() {
            return Command::new(tool)
                .status()
                .map_err(|error| format!("request reboot with {tool}: {error}"))
                .and_then(|status| {
                    status
                        .success()
                        .then_some(())
                        .ok_or_else(|| format!("{tool} refused the reboot; power-cycle the reader"))
                });
        }
    }
    Err("the firmware has no reboot command; power-cycle the reader".to_owned())
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

/// Tells the reader a restart is coming, and that it is not a fault.
///
/// Painted *before* the screen is restored rather than instead of it, so the
/// guarantee that a session always puts the reader's own screen back holds even
/// on the path that ends in a reboot. If the reboot then fails, the panel is
/// already back to normal and nothing has to undo this.
///
/// It exists because the reboot below was silent. The only warning was a line
/// on a developer's terminal, so from the owner's chair a Bluetooth connection
/// simply killed the device, which is exactly how it was reported.
fn announce_reboot(
    display: &DisplaySession,
    whole_screen: Rect,
    glyph: kobo_ui::Glyph,
    summary: &str,
) -> Result<(), String> {
    let screen = Screen::new(
        0,
        vec![kobo_ui::Node::Splash {
            id: kobo_ui::NodeId(1),
            glyph: Some(glyph),
            title: "Restarting your reader".to_owned(),
            summary: summary.to_owned(),
        }],
    );
    let mut surface = Surface::new(
        usize::try_from(whole_screen.width).unwrap_or(0),
        usize::try_from(whole_screen.height).unwrap_or(0),
    );
    kobo_ui::render_oriented(
        &screen,
        &metrics_for(&screen),
        &Chrome::with_back(false),
        &(),
        &mut surface,
        None,
        kobo_ui::Orientation::Portrait,
    );
    // A fresh planner, so its idea of what is already on the panel is blank and
    // the whole notice is drawn rather than diffed against the session's last
    // frame.
    Painter::new(surface.width, surface.height).paint(display, whole_screen, &surface)?;
    // Long enough to be read by someone who has just looked down at a reader
    // that appeared to be doing nothing. The reboot that follows costs far
    // more than this, so the wait is not what makes the wait long.
    thread::sleep(NOTICE_DWELL);
    Ok(())
}

/// How long the restart notice stays up before the screen is put back.
const NOTICE_DWELL: Duration = Duration::from_secs(5);

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
static NEXT_RUNTIME_FONT: AtomicU32 = AtomicU32::new(1);

/// One application the runtime is hosting.
///
/// Every one of these owns a live process, its own socket, its own store and
/// its own background work. Only one of them owns the panel.
struct Hosted {
    /// Identity that survives the list being reordered. An index would not:
    /// applications are removed from the middle when they end.
    id: u64,
    /// Selected at Hello and retained for responses to this application.
    version: u8,
    name: String,
    path: PathBuf,
    /// Root-owned filesystem visible to this application on the device.
    jail: Option<PathBuf>,
    child: ApplicationChild,
    stream: std::os::unix::net::UnixStream,
    store: kobo_policy::store::Store,
    shelf: kobo_policy::shelf::Shelf,
    tasks: TaskRunner,
    /// Capabilities declared by this installed application.
    declared: Declared,
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
    /// Application-local font handles mapped onto runtime-global handles.
    fonts: BTreeMap<FontHandle, FontHandle>,
    /// Logical direction requested by this application.
    orientation: kobo_ui::Orientation,
    /// Physical side selected by a verified orientation event, or the fixed
    /// fallback on readers without an orientation sensor.
    landscape_turn: kobo_ui::LandscapeTurn,
    painted: u32,
    /// When this was last on the panel, for deciding what to stop first.
    used: Instant,
}

/// An application process plus the legacy-kernel supervisor, when one is
/// needed. Ordinary process APIs may reap a child only after ptrace has
/// released its exit stop, so that ordering lives behind this type.
struct ApplicationChild {
    process: Child,
    #[cfg(all(target_os = "linux", target_arch = "arm"))]
    trace: Option<kobo_abi::sandbox::SyscallTrace>,
}

impl ApplicationChild {
    fn ordinary(process: Child) -> Self {
        Self {
            process,
            #[cfg(all(target_os = "linux", target_arch = "arm"))]
            trace: None,
        }
    }

    #[cfg(all(target_os = "linux", target_arch = "arm"))]
    fn traced(process: Child, trace: kobo_abi::sandbox::SyscallTrace) -> Self {
        Self {
            process,
            trace: Some(trace),
        }
    }

    fn id(&self) -> u32 {
        self.process.id()
    }

    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        #[cfg(all(target_os = "linux", target_arch = "arm"))]
        if self
            .trace
            .as_ref()
            .is_some_and(|trace| !trace.is_detached())
        {
            return Ok(None);
        }
        self.process.try_wait()
    }

    fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        #[cfg(all(target_os = "linux", target_arch = "arm"))]
        if let Some(trace) = &self.trace {
            trace.wait_until_detached(APP_STOP_GRACE);
        }
        self.process.wait()
    }

    fn trace_failure(&self) -> Option<String> {
        #[cfg(all(target_os = "linux", target_arch = "arm"))]
        if let Some(trace) = &self.trace {
            return trace.failure();
        }
        #[cfg(not(all(target_os = "linux", target_arch = "arm")))]
        let _ = self;
        None
    }
}

struct AppLaunch {
    listener: std::os::unix::net::UnixListener,
    socket_path: PathBuf,
    child_socket: PathBuf,
    program: PathBuf,
    jail: Option<PathBuf>,
    sandbox: Option<kobo_abi::sandbox::Sandbox>,
}

impl AppLaunch {
    fn prepare(path: &Path, id: u64) -> Result<Self, String> {
        if kobo_abi::sandbox::is_root() {
            return Self::sandboxed(path, id);
        }
        let socket_path =
            std::env::temp_dir().join(format!("kobo-session-{}-{id}.sock", std::process::id()));
        let _ignored = fs::remove_file(&socket_path);
        let listener = std::os::unix::net::UnixListener::bind(&socket_path)
            .map_err(|error| format!("bind application socket: {error}"))?;
        Ok(Self {
            listener,
            child_socket: socket_path.clone(),
            socket_path,
            program: path.to_path_buf(),
            jail: None,
            sandbox: None,
        })
    }

    fn sandboxed(path: &Path, id: u64) -> Result<Self, String> {
        if !kobo_abi::sandbox::network_boundary_available() {
            // Kobo's 4.1 i.MX6 kernels ship without CONFIG_SECCOMP and
            // CONFIG_NET_NS, so neither in-kernel network boundary exists
            // there. The chroot, the privilege ceiling and the identity drop
            // hold as everywhere, and on these kernels the runtime supervises
            // the application's syscalls over ptrace instead. Said once per
            // launch so a session transcript names which mechanism held.
            trace(
                "application network boundary enforced by ptrace supervision \
                 on this kernel; seccomp and network namespaces are absent",
            );
        }
        let root = std::env::temp_dir().join(format!("kobo-app-{}-{id}", std::process::id()));
        fs::create_dir(&root)
            .map_err(|error| format!("create application sandbox {}: {error}", root.display()))?;
        let prepared = (|| -> Result<Self, String> {
            fs::set_permissions(&root, fs::Permissions::from_mode(0o755))
                .map_err(|error| format!("protect application sandbox: {error}"))?;
            let program = root.join("app");
            fs::copy(path, &program)
                .map_err(|error| format!("copy {} into sandbox: {error}", path.display()))?;
            fs::set_permissions(&program, fs::Permissions::from_mode(0o555))
                .map_err(|error| format!("protect sandboxed application: {error}"))?;
            let socket_path = root.join("runtime.sock");
            let listener = std::os::unix::net::UnixListener::bind(&socket_path)
                .map_err(|error| format!("bind sandbox application socket: {error}"))?;
            fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o666))
                .map_err(|error| format!("make sandbox socket connectable: {error}"))?;
            let sandbox = kobo_abi::sandbox::Sandbox::new(&root)
                .map_err(|error| format!("prepare application sandbox: {error}"))?;
            Ok(Self {
                listener,
                socket_path,
                child_socket: PathBuf::from("/runtime.sock"),
                program: PathBuf::from("/app"),
                jail: Some(root.clone()),
                sandbox: Some(sandbox),
            })
        })();
        if prepared.is_err() {
            let _ignored = fs::remove_dir_all(&root);
        }
        prepared
    }
}

impl Hosted {
    fn send(&mut self, message: Message) -> Result<(), String> {
        kobo_protocol::write_to(
            &mut self.stream,
            &Frame {
                version: self.version,
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
    forward_is_194: bool,
    watchdog: &Arc<Watchdog>,
) -> Result<String, String> {
    // Kept current by the orientation channel: a reader flipped mid-session
    // keeps "forward" pointing forward even though the image does not rotate
    // yet.
    let mut forward_is_194 = forward_is_194;
    let catalogue = application
        .parent()
        .map_or_else(|| PathBuf::from("/tmp"), Path::to_path_buf);
    let home = application.to_path_buf();
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
    let bluetooth = kobo_hal::bluetooth::Bluetooth::open();
    if bluetooth.is_some() {
        backends.push(Capability::BluetoothControl);
    }
    let wifi = kobo_hal::wifi::Wifi::open();
    if wifi.is_some() {
        backends.push(Capability::WifiControl);
        backends.push(Capability::Network);
    }
    // Opened once for the whole session rather than per request, because the
    // sensor's value is in the edges and a reader that is only open while
    // somebody asks sees none of them.
    let mut cover = match kobo_hal::cover::CoverSensor::open() {
        Ok(cover) => {
            backends.push(Capability::CoverSensor);
            Some(cover)
        }
        Err(error) => {
            println!("no cover sensor ({error}); cover reads will be refused");
            None
        }
    };
    let audio_fetcher: kobo_hal::audio::StreamFetcher = Arc::new(|url, offset, max_bytes| {
        kobo_net::fetch_from(url, offset, max_bytes, None, &[]).map_err(|error| match error {
            // A reader with no route and a service that will not answer are
            // different things everywhere else, but `DeviceError` is the radio
            // vocabulary and has one word for both. Unreachable is the honest
            // one: from the player's point of view the bytes cannot be got.
            kobo_protocol::TaskError::Offline | kobo_protocol::TaskError::Unreachable => {
                kobo_protocol::DeviceError::Unreachable
            }
            kobo_protocol::TaskError::TimedOut => kobo_protocol::DeviceError::TimedOut,
            kobo_protocol::TaskError::NotFound => kobo_protocol::DeviceError::NotFound,
            kobo_protocol::TaskError::TooLarge => kobo_protocol::DeviceError::InvalidInput,
            kobo_protocol::TaskError::Denied => kobo_protocol::DeviceError::Backend,
            // Unreachable today: a plain fetch names no credential, so the
            // runtime never has one to be missing. Spelled out rather than
            // caught by a wildcard so that giving fetch a credential later
            // fails here instead of quietly reporting the wrong thing.
            //
            // A refusal to authenticate is not unreachable in the same way:
            // the host answered, and what it said was that this stream is not
            // for whoever asked.
            kobo_protocol::TaskError::NoCredential | kobo_protocol::TaskError::Unauthorized => {
                kobo_protocol::DeviceError::Authentication
            }
        })
    });
    let audio = kobo_hal::audio::Audio::open(Some(audio_fetcher));
    if audio.is_some() {
        backends.push(Capability::Audio);
        backends.push(Capability::BluetoothAudio);
    }
    let mut services = DeviceServices::new(
        Declared::all(),
        PowerPolicy::DEFAULT,
        Backends::with(backends),
    );
    let dictionaries = services.load_dictionaries(Path::new(DICTIONARIES));
    println!("offline dictionaries loaded: {dictionaries}");
    if let Some(light) = &frontlight {
        if let Some(percent) = light.percent() {
            services.observe_frontlight(percent);
        }
    }
    // A guard rather than a line at the end of the loop, because the loop has
    // several exits (the session clock, an idle reader, a failed write to an
    // application) and a front light left bright by whichever path was taken
    // is exactly the kind of change a reboot should not have to fix.
    let _restore_light = FrontlightGuard(frontlight.clone());
    // Deliberately already stale, so the first read an application makes is a
    // real measurement rather than the default the services were built with.
    let mut status = StatusSource::new();
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
    watch_for_stale_software(&sender);

    let result = (|| -> Result<String, String> {
        let front = start_application(&mut apps, &mut next_id, &home, whole_screen, &sender)?;
        let mut front = front;
        // Store installation stays bound to the channel whose catalog this
        // session last displayed. Settings may change while Store is
        // backgrounded; resolving the channel again at install time could
        // otherwise install a different version than the one the user saw.
        let mut store_channel = crate::autoupdate::preferences(Path::new(COBALT_ROOT)).channel;
        let mut visited: Vec<String> = Vec::new();
        let ceiling = Instant::now() + limits.ceiling;
        let mut last_activity = Instant::now();
        // Set when Back has been handed to an application that asked for it,
        // and cleared by the next screen that application draws. The reader's
        // way out is never left waiting on an application: if this is still
        // set when its grace expires, the launcher is shown regardless.
        let mut back_offered: Option<(u64, Instant)> = None;
        // The rectangle currently drawn inverted because a finger is on it.
        // The rectangle a finger is resting on, with the metrics its mark was
        // drawn against. Both, because the mark is undone by drawing it again
        // and that is only exact if nothing about it is recomputed.
        let mut pressed: Option<(kobo_ui::Rect, kobo_ui::DisplayMetrics, FeedbackKind)> = None;
        // A released control is restored with the application's answer when
        // that answer arrives promptly. If it does not, this deadline makes
        // the release visible on its own instead of leaving a key held down
        // while an application works or fails.
        let mut release_due: Option<Instant> = None;
        // When and where the finger landed, for telling a tap from a hold.
        let mut landed: Option<(Instant, i32, i32)> = None;
        // Updates the background checker found, held until the panel has been
        // quiet long enough to apply them. A newer report replaces an older
        // one outright: the newer one was computed against newer facts.
        let mut pending_updates: Option<crate::autoupdate::Plan> = None;

        loop {
            let now = Instant::now();
            // Reported from the loop rather than from a thread, so this says
            // the runtime is still serving the panel rather than merely that
            // the process has not been reaped.
            watchdog.beat();
            if release_due.is_some_and(|deadline| now >= deadline) {
                panel.paint(display, whole_screen, &surface)?;
                release_due = None;
            }
            // The band is the only thing on the panel that changes without
            // anybody touching it, so the loop has to notice it on its own.
            // Repainting is conditional on the reading having moved, and the
            // frame planner declines an identical frame anyway, so a session
            // sitting still costs nothing beyond reading four small files.
            if status.poll() {
                repaint(
                    &mut apps,
                    front,
                    display,
                    whole_screen,
                    &mut surface,
                    &mut panel,
                    &home,
                    &mut status,
                )?;
            }
            // Only the foreground application hears this. A magnet arriving
            // is a thing that happened in front of the reader, and a
            // background application has no standing to react to it.
            if let Some(sensor) = cover.as_mut() {
                if let Some(magnet) = sensor.poll() {
                    if let Some(index) = index_of(&apps, front) {
                        apps[index].send(kobo_protocol::Message::CoverChanged {
                            magnet_present: magnet == kobo_hal::cover::Magnet::Present,
                        })?;
                    }
                }
            }
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
            // Updates found in the background are applied only to a panel
            // nobody is using: enough quiet has passed since the last touch,
            // and the battery can afford the writes. Applying blocks this
            // loop exactly as a store install does, which is acceptable here
            // for the same reason it is there: nobody is waiting.
            if pending_updates.is_some()
                && now.saturating_duration_since(last_activity) >= AUTO_UPDATE_QUIET
                && auto_update_battery_permits()
            {
                if let Some(plan) = pending_updates.take() {
                    apply_auto_update(plan, &mut apps, front);
                }
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
                            &mut status,
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
                // So a charger pulled out while nobody is touching the panel
                // is noticed in seconds rather than at the next heartbeat.
                .min(STATUS_POLL)
                .min(back_offered.map_or(BEAT_INTERVAL, |(_, offered_at)| {
                    (offered_at + BACK_GRACE).saturating_duration_since(now)
                }))
                .min(release_due.map_or(BEAT_INTERVAL, |deadline| {
                    deadline.saturating_duration_since(now)
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
                Ok(Event::AutoUpdate(plan)) => {
                    pending_updates = Some(plan);
                }
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
                            &mut status,
                        )?;
                    }
                }
                Ok(Event::Gpio(event)) => {
                    last_activity = Instant::now();
                    match event {
                        // On the press, not the release: a page turn should
                        // not wait for a finger to lift. Only the foreground
                        // application hears it, for the same reason as taps
                        // and the cover: the press happened in front of the
                        // reader, and a background application has no
                        // standing to react to it.
                        //
                        // A screen that declares its page turns gets the
                        // declared action, exactly as if the side zone had
                        // been tapped, so a book, a shelf and a catalogue all
                        // page without knowing buttons exist. The layout is
                        // consulted rather than the screen because an overlay
                        // takes the page turns away, and a press while a
                        // dialog is up must not turn the page underneath it:
                        // the reader is answering the dialog. Only a screen
                        // that genuinely declares nothing receives the raw
                        // intent, as `Message::PageTurn`.
                        GpioEvent::Button {
                            button: button @ (gpio::Button::Page193 | gpio::Button::Page194),
                            pressed: true,
                        } => {
                            let forward = (button == gpio::Button::Page194) == forward_is_194;
                            if let Some(index) = index_of(&apps, front) {
                                let at_home = apps[index].path == home;
                                let message = match apps[index].screen.as_ref() {
                                    Some(current) => {
                                        // The chrome the frame was drawn with,
                                        // for the same reason `action_for`
                                        // uses it: laying out with a different
                                        // one resolves the press against a
                                        // screen the reader cannot see.
                                        let chrome = chrome_for(current, at_home, &mut status);
                                        page_key_message(current, &chrome, forward)
                                    }
                                    // Nothing painted yet, so there is nothing
                                    // to resolve against and the application
                                    // hears the raw intent.
                                    None => Some(kobo_protocol::Message::PageTurn { forward }),
                                };
                                if let Some(message) = message {
                                    apps[index].send(message)?;
                                }
                            }
                        }
                        GpioEvent::Button {
                            button: gpio::Button::Power,
                            pressed,
                        } => {
                            // Meaning arrives with the power sub-feature:
                            // short press sleep, long press shutdown. Until
                            // then the press is at least on the record.
                            trace(&format!("power button pressed={pressed}"));
                        }
                        // Sensor-equipped readers report which physical side
                        // is down. Readers without these events keep the fixed
                        // clockwise software-landscape fallback.
                        GpioEvent::Orientation(gpio::Orientation::PortraitUp) => {
                            forward_is_194 = true;
                        }
                        GpioEvent::Orientation(gpio::Orientation::PortraitDown) => {
                            forward_is_194 = false;
                        }
                        GpioEvent::Orientation(
                            orientation @ (gpio::Orientation::LandscapeLeft
                            | gpio::Orientation::LandscapeRight),
                        ) => {
                            let turn = match orientation {
                                gpio::Orientation::LandscapeLeft => {
                                    kobo_ui::LandscapeTurn::Clockwise
                                }
                                gpio::Orientation::LandscapeRight => {
                                    kobo_ui::LandscapeTurn::CounterClockwise
                                }
                                _ => unreachable!(),
                            };
                            let changed = apps.iter().any(|app| app.landscape_turn != turn);
                            for app in &mut apps {
                                app.landscape_turn = turn;
                            }
                            if changed {
                                repaint(
                                    &mut apps,
                                    front,
                                    display,
                                    whole_screen,
                                    &mut surface,
                                    &mut panel,
                                    &home,
                                    &mut status,
                                )?;
                            }
                        }
                        GpioEvent::Button { .. } | GpioEvent::Orientation(_) => {}
                    }
                }
                Ok(Event::Touch(event)) => {
                    last_activity = Instant::now();
                    let Some(index) = index_of(&apps, front) else {
                        return Ok(finish(&apps, &visited, "nothing is on the panel"));
                    };
                    let at_home = apps[index].path == home;
                    let screen = apps[index].screen.clone();
                    // The chrome the frame was drawn with, band and all: the
                    // band shifts everything below it down by its own height,
                    // so laying a tap out against chrome without one puts every
                    // hit rectangle five millimetres off.
                    let chrome = screen.as_ref().map_or_else(
                        || Chrome::with_back(!at_home),
                        |screen| chrome_for(screen, at_home, &mut status),
                    );
                    let orientation = apps[index].orientation;
                    let landscape_turn = apps[index].landscape_turn;
                    // A control shows that it has been touched, before
                    // anything it does can be seen. Without this the panel is
                    // simply still for as long as the application takes to
                    // answer (which for anything that reaches the network is
                    // seconds) and the reader, given no evidence their finger
                    // landed, reasonably concludes it did not and taps again.
                    // Drawn by inverting an inset, round-cornered patch of
                    // the finished surface, so the planner sees a change of
                    // pure black and white in one small rectangle and picks
                    // the fast waveform for it.
                    let mut released = None;
                    if let Some(current) = screen.as_ref() {
                        match event {
                            TouchEvent::Down { x, y } => {
                                if let (Ok(x), Ok(y)) = (i32::try_from(x), i32::try_from(y)) {
                                    landed = Some((Instant::now(), x, y));
                                    if orientation == kobo_ui::Orientation::Landscape {
                                        continue;
                                    }
                                    let layout =
                                        current.layout_with(&metrics_for(current), &chrome);
                                    if let Some(rect) = layout.pressed_control(x, y) {
                                        let metrics = metrics_for(current);
                                        surface.invert_press(rect, &metrics);
                                        panel.paint(display, whole_screen, &surface)?;
                                        // This feedback frame also carried any
                                        // older deferred release, so its
                                        // fallback is no longer needed. A press
                                        // outside every control paints nothing
                                        // and must leave the deadline armed.
                                        release_due = None;
                                        pressed =
                                            Some((rect, metrics, feedback_kind(&layout, rect)));
                                    }
                                }
                            }
                            TouchEvent::Up { .. } => {
                                // Restore the surface before the application
                                // draws, but do not submit the release yet.
                                // The action is delivered first and a prompt
                                // answer carries both the release and the new
                                // screen in one update.
                                if let Some((rect, metrics, class)) = pressed.take() {
                                    surface.invert_press(rect, &metrics);
                                    released = Some(class);
                                }
                            }
                            TouchEvent::Move { x, y } => {
                                // A finger that travels is a drag, not a hold.
                                // Without this, sliding across the page and
                                // pausing before letting go would mark a
                                // paragraph the reader never rested on.
                                if let (Some((_, from_x, from_y)), Ok(x), Ok(y)) =
                                    (landed, i32::try_from(x), i32::try_from(y))
                                {
                                    if (x - from_x).abs() > HOLD_SLIP
                                        || (y - from_y).abs() > HOLD_SLIP
                                    {
                                        landed = None;
                                    }
                                }
                                // Slid off the control. Cancel the press the
                                // way every other platform does, so the reader
                                // can see that letting go here will do nothing.
                                let off = match (i32::try_from(x), i32::try_from(y)) {
                                    (Ok(x), Ok(y)) => pressed
                                        .as_ref()
                                        .is_some_and(|(rect, _, _)| !rect.contains(x, y)),
                                    _ => true,
                                };
                                if off {
                                    if let Some((rect, metrics, _)) = pressed.take() {
                                        surface.invert_press(rect, &metrics);
                                        panel.paint(display, whole_screen, &surface)?;
                                    }
                                }
                            }
                        }
                    }
                    // Measured from contact to release, on the release, so a
                    // hold costs nothing until it has happened: no timer, no
                    // wake, and no gesture that fires while the finger is
                    // still down and cannot be taken back.
                    let held = match event {
                        TouchEvent::Up { .. } => landed
                            .take()
                            .is_some_and(|(at, _, _)| at.elapsed() >= HOLD_TIME),
                        _ => false,
                    };
                    let version = apps[index].version;
                    let disposition = deliver_touch(
                        &mut apps[index].stream,
                        version,
                        event,
                        screen.as_ref(),
                        &chrome,
                        held,
                        orientation,
                        landscape_turn,
                    )?;
                    match disposition {
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
                                &mut status,
                            )?;
                        }
                    }
                    if let Some(class) = released {
                        release_due = (disposition != Tap::Leave)
                            .then(|| Instant::now() + release_grace(class));
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
                        Message::SetScreen(mut screen) => {
                            if let Some(local) = screen.reading_font {
                                screen.reading_font = apps[index].fonts.get(&local).copied();
                            }
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
                            let chrome = chrome_for(&screen, apps[index].path == home, &mut status);
                            let screen =
                                kobo_ui::ensure_way_back(screen, &chrome, &apps[index].name);
                            if is_front {
                                trace(&format!("screen {} received", screen.id));
                                println!("screen {}", screen.id);
                                // The surface is about to be drawn afresh, so
                                // whatever was inverted on it is gone. Forget
                                // it, or releasing the finger would invert a
                                // rectangle of the new screen instead.
                                pressed = None;
                                release_due = None;
                                kobo_ui::render_oriented_with_turn(
                                    &screen,
                                    &metrics_for(&screen),
                                    &chrome,
                                    &apps[index].pictures,
                                    &mut surface,
                                    None,
                                    apps[index].orientation,
                                    apps[index].landscape_turn,
                                );
                                panel.paint(display, whole_screen, &surface)?;
                                apps[index].painted += 1;
                            }
                            // Kept either way. A background application that
                            // finished its work has a finished screen waiting,
                            // rather than the reader watching it be rebuilt.
                            apps[index].screen = Some(screen);
                        }
                        Message::SetOrientation(orientation) => {
                            apps[index].orientation = orientation;
                            if id == front {
                                if let Some(screen) = apps[index].screen.as_ref() {
                                    let chrome =
                                        chrome_for(screen, apps[index].path == home, &mut status);
                                    kobo_ui::render_oriented_with_turn(
                                        screen,
                                        &metrics_for(screen),
                                        &chrome,
                                        &apps[index].pictures,
                                        &mut surface,
                                        None,
                                        orientation,
                                        apps[index].landscape_turn,
                                    );
                                    panel.paint(display, whole_screen, &surface)?;
                                }
                            }
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
                        Message::PutFont {
                            handle,
                            name,
                            bytes,
                        } => {
                            // The map entry is only made once the face parses.
                            // Creating it first let a refused font hold a slot,
                            // and nothing bounded how many slots one
                            // application could take: a loop over fresh handles
                            // would grow the runtime until the device gave out.
                            let known = apps[index].fonts.contains_key(&handle);
                            if !known && apps[index].fonts.len() >= MAX_APP_FONTS {
                                trace(&format!(
                                    "font {} refused: {MAX_APP_FONTS} already held",
                                    handle.0
                                ));
                            } else {
                                match kobo_text::BookFont::from_bytes(
                                    &bytes,
                                    &name,
                                    crate::device_metrics(),
                                ) {
                                    Ok(book_font) => {
                                        let runtime_handle =
                                            *apps[index].fonts.entry(handle).or_insert_with(|| {
                                                FontHandle(
                                                    NEXT_RUNTIME_FONT
                                                        .fetch_add(1, AtomicOrdering::Relaxed),
                                                )
                                            });
                                        kobo_ui::put_book_typesetter(
                                            runtime_handle,
                                            Box::new(book_font),
                                        );
                                    }
                                    Err(error) => {
                                        trace(&format!("font {} refused: {error}", handle.0));
                                    }
                                }
                            }
                        }
                        Message::DropFont { handle } => {
                            if let Some(runtime_handle) = apps[index].fonts.remove(&handle) {
                                kobo_ui::drop_book_typesetter(runtime_handle);
                            }
                        }
                        // An application logs to explain itself, and the times
                        // it most needs to be believed are the times it took
                        // the reader down with it. Dropping the line here left
                        // the diagnostics an application had gone to the
                        // trouble of emitting with nowhere to arrive, on the
                        // one path that actually runs on the device.
                        Message::Log { level, message } => {
                            trace(&format!(
                                "app {} {level:?}: {}",
                                apps[index].name,
                                message.replace(['\r', '\n'], " ")
                            ));
                        }
                        Message::DeviceRequest(request) => {
                            let not_declared = !system_request_allowed(&apps[index].name, &request)
                                || kobo_policy::request_capability(&request).is_some_and(
                                    |capability| !apps[index].declared.holds(capability),
                                );
                            if !not_declared
                                && matches!(request, kobo_protocol::DeviceRequest::ReadBattery)
                                && battery_read_at.elapsed() >= BATTERY_INTERVAL
                            {
                                if let Some(battery) = kobo_hal::battery::read() {
                                    services.observe_battery(battery.percent, battery.charging);
                                }
                                battery_read_at = Instant::now();
                            }
                            // Once caller identity and declared capabilities
                            // pass, drive the light before forming the reply so
                            // the application is told what the hardware
                            // actually took. Percentages do not divide evenly
                            // into every control's range.
                            if !not_declared {
                                if let Some(light) = &frontlight {
                                    match request {
                                        kobo_protocol::DeviceRequest::SetFrontlight { percent }
                                            if apps[index]
                                                .declared
                                                .holds(Capability::FrontlightControl)
                                                && services.may(Capability::FrontlightControl) =>
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
                            }
                            let result = if not_declared {
                                kobo_protocol::DeviceResult::Denied(
                                    kobo_protocol::DenyReason::NotDeclared,
                                )
                            } else if let Some(reason) = services.refusal_for(&request) {
                                kobo_protocol::DeviceResult::Denied(reason)
                            } else {
                                match &request {
                                    kobo_protocol::DeviceRequest::ReadBluetooth => {
                                        bluetooth.as_ref().map_or(
                                            kobo_protocol::DeviceResult::Denied(
                                                kobo_protocol::DenyReason::Unsupported,
                                            ),
                                            kobo_hal::bluetooth::Bluetooth::state,
                                        )
                                    }

                                    kobo_protocol::DeviceRequest::SetBluetooth { enabled } => {
                                        // Kobo documents Bluetooth as sharing the wireless
                                        // radio with Wi-Fi. Bring Wi-Fi up first, exactly as
                                        // Nickel does, but do not fail Bluetooth merely because
                                        // association itself has not completed yet.
                                        if *enabled {
                                            if let Some(wifi) = &wifi {
                                                let _ignored = wifi.set_enabled(true);
                                            }
                                        }
                                        bluetooth.as_ref().map_or(
                                            kobo_protocol::DeviceResult::Denied(
                                                kobo_protocol::DenyReason::Unsupported,
                                            ),
                                            |bluetooth| bluetooth.set_enabled(*enabled),
                                        )
                                    }
                                    kobo_protocol::DeviceRequest::ScanBluetooth => {
                                        bluetooth.as_ref().map_or(
                                            kobo_protocol::DeviceResult::Denied(
                                                kobo_protocol::DenyReason::Unsupported,
                                            ),
                                            kobo_hal::bluetooth::Bluetooth::scan,
                                        )
                                    }
                                    kobo_protocol::DeviceRequest::PairBluetooth { address } => {
                                        bluetooth.as_ref().map_or(
                                            kobo_protocol::DeviceResult::Denied(
                                                kobo_protocol::DenyReason::Unsupported,
                                            ),
                                            |bluetooth| bluetooth.pair(address),
                                        )
                                    }
                                    kobo_protocol::DeviceRequest::ConnectBluetooth { address } => {
                                        bluetooth.as_ref().map_or(
                                            kobo_protocol::DeviceResult::Denied(
                                                kobo_protocol::DenyReason::Unsupported,
                                            ),
                                            |bluetooth| bluetooth.connect(address),
                                        )
                                    }
                                    kobo_protocol::DeviceRequest::DisconnectBluetooth {
                                        address,
                                    } => bluetooth.as_ref().map_or(
                                        kobo_protocol::DeviceResult::Denied(
                                            kobo_protocol::DenyReason::Unsupported,
                                        ),
                                        |bluetooth| bluetooth.disconnect(address),
                                    ),
                                    kobo_protocol::DeviceRequest::ForgetBluetooth { address } => {
                                        bluetooth.as_ref().map_or(
                                            kobo_protocol::DeviceResult::Denied(
                                                kobo_protocol::DenyReason::Unsupported,
                                            ),
                                            |bluetooth| bluetooth.forget(address),
                                        )
                                    }
                                    kobo_protocol::DeviceRequest::ReadWifi => wifi.as_ref().map_or(
                                        kobo_protocol::DeviceResult::Denied(
                                            kobo_protocol::DenyReason::Unsupported,
                                        ),
                                        kobo_hal::wifi::Wifi::state,
                                    ),
                                    kobo_protocol::DeviceRequest::SetWifi { enabled } => {
                                        wifi.as_ref().map_or(
                                            kobo_protocol::DeviceResult::Denied(
                                                kobo_protocol::DenyReason::Unsupported,
                                            ),
                                            |wifi| wifi.set_enabled(*enabled),
                                        )
                                    }
                                    kobo_protocol::DeviceRequest::ScanWifi => wifi.as_ref().map_or(
                                        kobo_protocol::DeviceResult::Denied(
                                            kobo_protocol::DenyReason::Unsupported,
                                        ),
                                        kobo_hal::wifi::Wifi::scan,
                                    ),
                                    kobo_protocol::DeviceRequest::JoinWifi { ssid, password } => {
                                        wifi.as_ref().map_or(
                                            kobo_protocol::DeviceResult::Denied(
                                                kobo_protocol::DenyReason::Unsupported,
                                            ),
                                            |wifi| wifi.join(ssid, password),
                                        )
                                    }
                                    kobo_protocol::DeviceRequest::DisconnectWifi => {
                                        wifi.as_ref().map_or(
                                            kobo_protocol::DeviceResult::Denied(
                                                kobo_protocol::DenyReason::Unsupported,
                                            ),
                                            kobo_hal::wifi::Wifi::disconnect,
                                        )
                                    }
                                    kobo_protocol::DeviceRequest::ReadAudio => {
                                        audio.as_ref().map_or(
                                            kobo_protocol::DeviceResult::Denied(
                                                kobo_protocol::DenyReason::Unsupported,
                                            ),
                                            kobo_hal::audio::Audio::state,
                                        )
                                    }
                                    kobo_protocol::DeviceRequest::LoadAudio { source } => {
                                        audio.as_ref().map_or(
                                            kobo_protocol::DeviceResult::Denied(
                                                kobo_protocol::DenyReason::Unsupported,
                                            ),
                                            |audio| match source {
                                                kobo_protocol::AudioSource::Shelf(name) => {
                                                    apps[index].shelf.published_path(name).map_or(
                                                        kobo_protocol::DeviceResult::Failed(
                                                            kobo_protocol::DeviceError::NotFound,
                                                        ),
                                                        |path| {
                                                            audio.load(
                                                                kobo_hal::audio::Source::File(path),
                                                            )
                                                        },
                                                    )
                                                }
                                                kobo_protocol::AudioSource::Stream(url) => {
                                                    if apps[index]
                                                        .declared
                                                        .holds(Capability::Network)
                                                        && services.may(Capability::Network)
                                                    {
                                                        audio.load(kobo_hal::audio::Source::Stream(
                                                            url.clone(),
                                                        ))
                                                    } else {
                                                        kobo_protocol::DeviceResult::Denied(
                                                            kobo_protocol::DenyReason::NotDeclared,
                                                        )
                                                    }
                                                }
                                            },
                                        )
                                    }
                                    kobo_protocol::DeviceRequest::PlayAudio => {
                                        audio.as_ref().map_or(
                                            kobo_protocol::DeviceResult::Denied(
                                                kobo_protocol::DenyReason::Unsupported,
                                            ),
                                            kobo_hal::audio::Audio::play,
                                        )
                                    }
                                    kobo_protocol::DeviceRequest::PauseAudio => {
                                        audio.as_ref().map_or(
                                            kobo_protocol::DeviceResult::Denied(
                                                kobo_protocol::DenyReason::Unsupported,
                                            ),
                                            kobo_hal::audio::Audio::pause,
                                        )
                                    }
                                    kobo_protocol::DeviceRequest::SeekAudio { position_ms } => {
                                        audio.as_ref().map_or(
                                            kobo_protocol::DeviceResult::Denied(
                                                kobo_protocol::DenyReason::Unsupported,
                                            ),
                                            |audio| audio.seek(*position_ms),
                                        )
                                    }
                                    kobo_protocol::DeviceRequest::StopAudio => {
                                        audio.as_ref().map_or(
                                            kobo_protocol::DeviceResult::Denied(
                                                kobo_protocol::DenyReason::Unsupported,
                                            ),
                                            kobo_hal::audio::Audio::stop,
                                        )
                                    }
                                    kobo_protocol::DeviceRequest::SetAudioVolume { percent } => {
                                        audio.as_ref().map_or(
                                            kobo_protocol::DeviceResult::Denied(
                                                kobo_protocol::DenyReason::Unsupported,
                                            ),
                                            |audio| audio.set_volume(*percent),
                                        )
                                    }
                                    // The gauge is read straight through
                                    // rather than from the cached percent the
                                    // band uses, because this is asked once
                                    // when somebody opens a battery screen
                                    // and they want today's numbers. When
                                    // there is no gauge to read, the policy's
                                    // own answer stands, which is what the
                                    // simulator runs on.
                                    kobo_protocol::DeviceRequest::ReadBatteryDetail => {
                                        kobo_hal::battery::detail().map_or_else(
                                            || services.handle(request.clone()),
                                            kobo_protocol::DeviceResult::BatteryDetail,
                                        )
                                    }
                                    // Answered from the sensor opened for the
                                    // session rather than by opening one here,
                                    // so the answer and the changes that
                                    // follow it come from the same reader and
                                    // cannot disagree.
                                    kobo_protocol::DeviceRequest::ReadCover => {
                                        cover.as_ref().map_or_else(
                                            || services.handle(request.clone()),
                                            |sensor| kobo_protocol::DeviceResult::Cover {
                                                available: true,
                                                magnet_present: sensor.magnet()
                                                    == kobo_hal::cover::Magnet::Present,
                                            },
                                        )
                                    }
                                    // Blocks the message loop like a
                                    // Bluetooth scan does. The application
                                    // paints its progress screen before
                                    // asking, and nothing else is served
                                    // while the installation is replaced,
                                    // which is exactly the quiet wanted.
                                    kobo_protocol::DeviceRequest::Update { url, sha256 } => {
                                        match crate::update::apply(url, sha256) {
                                            Ok(()) => kobo_protocol::DeviceResult::Done,
                                            Err(error) => {
                                                trace(&format!("update refused: {error}"));
                                                kobo_protocol::DeviceResult::Failed(error)
                                            }
                                        }
                                    }
                                    kobo_protocol::DeviceRequest::ListInstalledApps => {
                                        app_store_result(crate::app_store::installed(Path::new(
                                            COBALT_ROOT,
                                        )))
                                    }
                                    kobo_protocol::DeviceRequest::ReadAppCatalog => {
                                        let root = Path::new(COBALT_ROOT);
                                        let channel = crate::autoupdate::preferences(root).channel;
                                        let result = crate::app_store::catalog(root, channel);
                                        if result.is_ok() {
                                            store_channel = channel;
                                        }
                                        app_store_result(result)
                                    }
                                    kobo_protocol::DeviceRequest::RefreshAppCatalog => {
                                        let root = Path::new(COBALT_ROOT);
                                        let channel = crate::autoupdate::preferences(root).channel;
                                        let result = crate::app_store::refresh(root, channel);
                                        if result.is_ok() {
                                            store_channel = channel;
                                        }
                                        app_store_result(result)
                                    }
                                    kobo_protocol::DeviceRequest::InstallApp { id } => {
                                        let root = Path::new(COBALT_ROOT);
                                        let result =
                                            crate::app_store::install(root, id, store_channel);
                                        if result.is_ok() {
                                            stop_named_application(&mut apps, id);
                                        }
                                        app_store_done(result)
                                    }
                                    kobo_protocol::DeviceRequest::UninstallApp { id } => {
                                        let result =
                                            crate::app_store::uninstall(Path::new(COBALT_ROOT), id);
                                        if result.is_ok() {
                                            stop_named_application(&mut apps, id);
                                        }
                                        app_store_done(result)
                                    }
                                    kobo_protocol::DeviceRequest::ReadAppLink => app_link_result(
                                        crate::app_link::read(Path::new(COBALT_ROOT)),
                                    ),
                                    kobo_protocol::DeviceRequest::BeginAppLink => app_link_result(
                                        crate::app_link::begin(Path::new(COBALT_ROOT)),
                                    ),
                                    kobo_protocol::DeviceRequest::PollAppLink => {
                                        let result = crate::app_link::poll(Path::new(COBALT_ROOT));
                                        if let Ok(kobo_protocol::DeviceResult::RemoteInstall(
                                            outcome,
                                        )) = &result
                                        {
                                            if let Some(id) = remote_installed_id(outcome) {
                                                stop_named_application(&mut apps, id);
                                            }
                                        }
                                        app_link_result(result)
                                    }
                                    kobo_protocol::DeviceRequest::DisconnectAppLink => {
                                        app_link_result(crate::app_link::disconnect(Path::new(
                                            COBALT_ROOT,
                                        )))
                                    }
                                    kobo_protocol::DeviceRequest::ReadAutoUpdate => {
                                        auto_update_result(crate::autoupdate::preferences(
                                            Path::new(COBALT_ROOT),
                                        ))
                                    }
                                    kobo_protocol::DeviceRequest::SetAutoUpdate {
                                        cobalt,
                                        apps,
                                    } => {
                                        let current =
                                            crate::autoupdate::preferences(Path::new(COBALT_ROOT));
                                        let chosen = crate::autoupdate::Preferences {
                                            cobalt: *cobalt,
                                            apps: *apps,
                                            ..current
                                        };
                                        match crate::autoupdate::set_preferences(
                                            Path::new(COBALT_ROOT),
                                            chosen,
                                        ) {
                                            Ok(()) => auto_update_result(chosen),
                                            Err(error) => {
                                                kobo_protocol::DeviceResult::Failed(error)
                                            }
                                        }
                                    }
                                    kobo_protocol::DeviceRequest::ReadUpdateChannel => {
                                        update_channel_result(crate::autoupdate::preferences(
                                            Path::new(COBALT_ROOT),
                                        ))
                                    }
                                    kobo_protocol::DeviceRequest::SetUpdateChannel { channel } => {
                                        let current =
                                            crate::autoupdate::preferences(Path::new(COBALT_ROOT));
                                        let chosen = crate::autoupdate::Preferences {
                                            channel: *channel,
                                            ..current
                                        };
                                        match crate::autoupdate::set_preferences(
                                            Path::new(COBALT_ROOT),
                                            chosen,
                                        ) {
                                            Ok(()) => update_channel_result(chosen),
                                            Err(error) => {
                                                kobo_protocol::DeviceResult::Failed(error)
                                            }
                                        }
                                    }
                                    kobo_protocol::DeviceRequest::SetSecret { name, value } => {
                                        match kobo_policy::credentials::install_app_secret(
                                            Path::new(SECRETS),
                                            &apps[index].name,
                                            name,
                                            value.as_str(),
                                        ) {
                                            Ok(()) => kobo_protocol::DeviceResult::Done,
                                            Err(error) => {
                                                kobo_protocol::DeviceResult::Failed(error)
                                            }
                                        }
                                    }
                                    // Answered by probing the hardware again
                                    // rather than from anything cached, so
                                    // the screen built from it describes the
                                    // reader it is drawn on. That screen
                                    // exists to be photographed as evidence a
                                    // build ran here, and evidence read fresh
                                    // is worth more than evidence remembered.
                                    kobo_protocol::DeviceRequest::ReadIdentity => read_identity()
                                        .map_or_else(
                                            || services.handle(request.clone()),
                                            kobo_protocol::DeviceResult::Identity,
                                        ),
                                    _ => services.handle(request.clone()),
                                }
                            };
                            // Every Bluetooth reply passes through here, so
                            // this is the one place that has to know the band
                            // shows a Bluetooth mark. A reply that changes the
                            // answer repaints on the spot rather than waiting
                            // for the next screen.
                            if let kobo_protocol::DeviceResult::Bluetooth {
                                enabled, devices, ..
                            } = &result
                            {
                                let connected =
                                    *enabled && devices.iter().any(|device| device.connected);
                                if status.observe_bluetooth(connected) {
                                    repaint(
                                        &mut apps,
                                        front,
                                        display,
                                        whole_screen,
                                        &mut surface,
                                        &mut panel,
                                        &home,
                                        &mut status,
                                    )?;
                                }
                            }
                            reply(
                                &mut apps[index],
                                frame.request_id,
                                Message::DeviceResult(result),
                            )?;
                        }
                        Message::StoreRequest(request) => {
                            // The shelf is asked first and declines anything
                            // that is not its own, so neither side needs a
                            // list of which requests belong where.
                            let result = apps[index]
                                .shelf
                                .handle(&request)
                                .unwrap_or_else(|| apps[index].store.handle(&request));
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
                                    &mut status,
                                )?;
                            }
                        }
                        Message::Launch { name: wanted } => {
                            match open_application(
                                &mut apps,
                                &mut next_id,
                                &catalogue,
                                &wanted,
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
                                        &mut status,
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
                        | Message::TextHold { .. }
                        | Message::TaskOutcome { .. }
                        | Message::Lifecycle(_)
                        | Message::DeviceResult(_)
                        | Message::StoreResult(_)
                        | Message::CoverChanged { .. }
                        | Message::PageTurn { .. }
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
            version: app.version,
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
    status: &mut StatusSource,
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
    repaint(
        apps,
        wanted,
        display,
        whole_screen,
        surface,
        panel,
        home,
        status,
    )?;
    Ok(wanted)
}

/// Draws whatever the application on the panel last drew, with fresh chrome.
///
/// Shared by the application switch and the status poll, which want the same
/// thing for different reasons. Cheap when nothing moved: the frame planner
/// compares the rendered surface against what is on the panel and declines to
/// refresh an identical one, so a poll that finds a new battery reading
/// repaints the strip it changed and a poll that finds the same reading costs
/// one render and no panel update at all.
#[allow(clippy::too_many_arguments)]
fn repaint(
    apps: &mut [Hosted],
    id: u64,
    display: &DisplaySession,
    whole_screen: Rect,
    surface: &mut Surface,
    panel: &mut Painter,
    home: &Path,
    status: &mut StatusSource,
) -> Result<(), String> {
    let Some(index) = index_of(apps, id) else {
        return Ok(());
    };
    let Some(screen) = apps[index].screen.clone() else {
        return Ok(());
    };
    let chrome = chrome_for(&screen, apps[index].path == home, status);
    kobo_ui::render_oriented_with_turn(
        &screen,
        &metrics_for(&screen),
        &chrome,
        &apps[index].pictures,
        surface,
        None,
        apps[index].orientation,
        apps[index].landscape_turn,
    );
    panel.paint(display, whole_screen, surface)?;
    apps[index].painted += 1;
    Ok(())
}

/// Keeps platform replacement and app installation behind distinct built-in
/// identities even while both travel over the same bounded device channel.
fn system_request_allowed(app: &str, request: &kobo_protocol::DeviceRequest) -> bool {
    match request {
        kobo_protocol::DeviceRequest::Update { .. }
        | kobo_protocol::DeviceRequest::ReadAutoUpdate
        | kobo_protocol::DeviceRequest::SetAutoUpdate { .. }
        | kobo_protocol::DeviceRequest::ReadUpdateChannel
        | kobo_protocol::DeviceRequest::SetUpdateChannel { .. } => app == "settings",
        kobo_protocol::DeviceRequest::SetSecret { name, .. } => {
            kobo_policy::credentials::may_set(app, name)
        }
        kobo_protocol::DeviceRequest::ListInstalledApps => matches!(app, "launcher" | "store"),
        kobo_protocol::DeviceRequest::ReadAppCatalog
        | kobo_protocol::DeviceRequest::RefreshAppCatalog
        | kobo_protocol::DeviceRequest::InstallApp { .. }
        | kobo_protocol::DeviceRequest::UninstallApp { .. }
        | kobo_protocol::DeviceRequest::ReadAppLink
        | kobo_protocol::DeviceRequest::BeginAppLink
        | kobo_protocol::DeviceRequest::PollAppLink
        | kobo_protocol::DeviceRequest::DisconnectAppLink => app == "store",
        _ => true,
    }
}

fn app_link_result(
    result: Result<kobo_protocol::DeviceResult, kobo_protocol::DeviceError>,
) -> kobo_protocol::DeviceResult {
    result.unwrap_or_else(kobo_protocol::DeviceResult::Failed)
}

/// What this build is running on, read from the hardware right now.
///
/// `None` when the probe finds no matching profile, which is what happens on
/// a development host; the policy's simulator answer stands there, and its
/// profile name says plainly that it is not a reader. Firmware and kernel
/// come from the same files the startup identification read, so a value that
/// is missing here was missing there too and is shown as absent rather than
/// invented.
fn read_identity() -> Option<kobo_protocol::DeviceIdentity> {
    let snapshot = kobo_hal::probe_device().ok()?;
    let profile = kobo_profile::identify_profile(&snapshot)?;
    Some(kobo_protocol::DeviceIdentity {
        profile_id: profile.id.to_owned(),
        model: profile.model.to_owned(),
        device_code: profile.device_code,
        firmware: bounded(snapshot.identity.firmware_version.unwrap_or_default()),
        kernel: bounded(snapshot.identity.kernel_release.unwrap_or_default()),
        runtime_version: env!("CARGO_PKG_VERSION").to_owned(),
        panel_width: profile.width,
        panel_height: profile.height,
    })
}

/// Keeps a probed string inside the identity field bound so one overgrown
/// version file cannot make the whole answer unsendable. Cut once at the
/// bound, stepping back only as far as the nearest character boundary.
fn bounded(mut text: String) -> String {
    if text.len() > kobo_protocol::MAX_IDENTITY_FIELD_LEN {
        let mut end = kobo_protocol::MAX_IDENTITY_FIELD_LEN;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
}

fn remote_installed_id(outcome: &kobo_protocol::RemoteInstallOutcome) -> Option<&str> {
    match outcome {
        kobo_protocol::RemoteInstallOutcome::Installed { id }
        | kobo_protocol::RemoteInstallOutcome::Updated { id } => Some(id),
        kobo_protocol::RemoteInstallOutcome::None
        | kobo_protocol::RemoteInstallOutcome::AlreadyInstalled { .. }
        | kobo_protocol::RemoteInstallOutcome::Included { .. }
        | kobo_protocol::RemoteInstallOutcome::Unavailable { .. }
        | kobo_protocol::RemoteInstallOutcome::RequiresCobalt { .. } => None,
    }
}

fn app_store_result(
    result: Result<Vec<kobo_protocol::AppInfo>, kobo_protocol::DeviceError>,
) -> kobo_protocol::DeviceResult {
    match result {
        Ok(entries) => kobo_protocol::DeviceResult::Apps { entries },
        Err(error) => kobo_protocol::DeviceResult::Failed(error),
    }
}

fn app_store_done(result: Result<(), kobo_protocol::DeviceError>) -> kobo_protocol::DeviceResult {
    match result {
        Ok(()) => kobo_protocol::DeviceResult::Done,
        Err(error) => kobo_protocol::DeviceResult::Failed(error),
    }
}

fn stop_named_application(apps: &mut [Hosted], name: &str) {
    if let Some(app) = apps.iter_mut().find(|app| app.name == name) {
        app.tasks.shutdown();
        stop_application(&mut app.child, app.jail.as_deref());
        if let Some(root) = &app.jail {
            let _ignored = fs::remove_dir_all(root);
        }
    }
}

fn auto_update_result(chosen: crate::autoupdate::Preferences) -> kobo_protocol::DeviceResult {
    kobo_protocol::DeviceResult::AutoUpdate {
        cobalt: chosen.cobalt,
        apps: chosen.apps,
    }
}

fn update_channel_result(chosen: crate::autoupdate::Preferences) -> kobo_protocol::DeviceResult {
    kobo_protocol::DeviceResult::UpdateChannel(chosen.channel)
}

/// Whether the battery can afford background writes right now. A reader whose
/// gauge cannot be read is allowed, because refusing forever is worse than
/// trusting hardware that boots.
fn auto_update_battery_permits() -> bool {
    kobo_hal::battery::read()
        .is_none_or(|battery| battery.charging || battery.percent >= AUTO_UPDATE_MIN_BATTERY)
}

/// Applies what the background checker found, now that the panel is quiet.
///
/// The owner's choices are read again rather than trusted from the plan,
/// because the plan may be hours old and the settings screen may have moved a
/// switch since. The application currently on the panel is never replaced
/// under the reader; its turn comes with a later plan. A staged platform
/// release takes effect the next time Cobalt starts, exactly as one installed
/// from the settings screen does.
fn apply_auto_update(plan: crate::autoupdate::Plan, apps: &mut [Hosted], front: u64) {
    let root = Path::new(COBALT_ROOT);
    let chosen = crate::autoupdate::preferences(root);
    if chosen.channel != plan.channel {
        trace("the update channel changed, so the old background plan was discarded");
        return;
    }
    let showing = apps
        .iter()
        .find(|app| app.id == front)
        .map(|app| app.name.clone());
    if chosen.apps {
        for id in plan.apps {
            if showing.as_deref() == Some(id.as_str()) {
                trace(&format!("{id} is on the panel, so its update waits"));
                continue;
            }
            match crate::app_store::install(root, &id, chosen.channel) {
                Ok(()) => {
                    stop_named_application(apps, &id);
                    trace(&format!("{id} was updated in the background"));
                }
                Err(error) => trace(&format!("{id} background update failed: {error}")),
            }
        }
    }
    if !chosen.cobalt {
        return;
    }
    if let Some(update) = plan.platform {
        match crate::update::apply(&update.url, &update.sha256) {
            Ok(()) => trace(&format!(
                "Cobalt {} is staged and runs from the next start",
                update.version
            )),
            Err(error) => trace(&format!(
                "Cobalt {} background update failed: {error}",
                update.version
            )),
        }
    }
}

/// Finds an application by name, starting it only if it is not already running.
#[allow(clippy::too_many_arguments)]
fn open_application(
    apps: &mut Vec<Hosted>,
    next_id: &mut u64,
    catalogue: &Path,
    name: &str,
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
    start_application(apps, next_id, &path, whole_screen, sender)
}

/// Stops whichever background application has been left alone longest.
///
/// Never the one on the panel, and never the last one: the alternative to
/// stopping something is refusing to open anything, which is worse.
fn evict(apps: &mut Vec<Hosted>, front: u64) {
    let seen: Vec<(u64, Instant, bool)> = apps
        .iter()
        .map(|app| (app.id, app.used, app.tasks.in_flight() > 0))
        .collect();
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
///
/// An application with work in flight is not cold, however long ago it was
/// last on the panel. Somebody who starts a three minute audiobook and reads
/// the news while it writes has not abandoned it, and stopping it would throw
/// away minutes of work that was proceeding correctly, silently, and with
/// nothing on the panel to say so. Such an application is stopped only when
/// every other candidate is busy too, because refusing to open anything is
/// worse still.
fn coldest(seen: &[(u64, Instant, bool)], front: u64) -> Option<usize> {
    seen.iter()
        .enumerate()
        .filter(|(_, (id, _, _))| *id != front)
        .min_by_key(|(_, (_, used, busy))| (*busy, *used))
        .map(|(index, _)| index)
}

/// Starts one application and completes its opening exchange.
#[allow(
    clippy::too_many_lines,
    reason = "process setup, sandboxing and per-app services form one launch transaction"
)]
fn start_application(
    apps: &mut Vec<Hosted>,
    next_id: &mut u64,
    path: &Path,
    whole_screen: Rect,
    sender: &Sender<Event>,
) -> Result<u64, String> {
    let expected_name = installed_name(path)?;
    // Capabilities are part of admission, not post-launch setup. Reading them
    // before any process exists means a corrupt or concurrently removed
    // manifest cannot leave an unowned child and jail behind.
    let declared = if path.starts_with(Path::new(COBALT_ROOT).join("apps")) {
        crate::app_store::declared(Path::new(COBALT_ROOT), &expected_name)
            .ok_or_else(|| format!("read application manifest for {expected_name}"))?
    } else if let Some(declared) = crate::app_store::builtin_declared(&expected_name) {
        declared
    } else {
        Declared::all()
    };
    let launch = AppLaunch::prepare(path, *next_id)?;
    let AppLaunch {
        listener,
        socket_path,
        child_socket,
        program,
        jail,
        sandbox,
    } = launch;
    let mut command = Command::new(&program);
    command
        .env_clear()
        .env("KOBO_SOCKET", &child_socket)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        // An application that dies of something the protocol never hears about
        // -- a panic, a signal, a failed allocation -- says so on its standard
        // error and nowhere else. Sent to nothing, as this was, the only trace
        // left of a crash was the application no longer being there, and the
        // one question worth asking of a crash could not be answered at all.
        .stderr(Stdio::piped());
    let trace_syscalls = if let Some(sandbox) = sandbox {
        sandbox.configure(&mut command)
    } else {
        kobo_abi::process_group::configure(&mut command);
        false
    };
    let spawned = if trace_syscalls {
        #[cfg(all(target_os = "linux", target_arch = "arm"))]
        {
            kobo_abi::sandbox::SyscallTrace::spawn(command)
                .map(|(child, trace)| ApplicationChild::traced(child, trace))
        }
        #[cfg(not(all(target_os = "linux", target_arch = "arm")))]
        {
            unreachable!("syscall tracing is selected only on 32-bit ARM Linux")
        }
    } else {
        command.spawn().map(ApplicationChild::ordinary)
    };
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => {
            let _ignored = fs::remove_file(&socket_path);
            if let Some(root) = &jail {
                let _ignored = fs::remove_dir_all(root);
            }
            return Err(format!("start {}: {error}", path.display()));
        }
    };
    if let Some(stderr) = child.process.stderr.take() {
        report_what_an_application_says(expected_name.clone(), stderr);
    }
    let greeting = greet(&listener, whole_screen, &expected_name);
    drop(listener);
    let _ignored = fs::remove_file(&socket_path);
    let (stream, name, version) = match greeting {
        Ok(greeting) => greeting,
        Err(error) => {
            stop_application(&mut child, jail.as_deref());
            let error = with_trace_failure(error, &child);
            if let Some(root) = &jail {
                let _ignored = fs::remove_dir_all(root);
            }
            return Err(error);
        }
    };
    let id = *next_id;
    *next_id += 1;
    if let Err(error) = pump_application(&stream, sender, id, version) {
        stop_application(&mut child, jail.as_deref());
        let error = with_trace_failure(error, &child);
        if let Some(root) = &jail {
            let _ignored = fs::remove_dir_all(root);
        }
        return Err(error);
    }
    let waker = sender.clone();
    let credential_app = name.clone();
    let tasks = TaskRunner::simulated(std::env::temp_dir())
        .with_fetch(Arc::new(kobo_net::fetch_from_controlled))
        .with_post(Arc::new(kobo_net::post_controlled))
        .with_line_streams(Arc::new(kobo_net::LineStreams::default()))
        .with_app_secrets(SECRETS, &name)
        .with_credential_policy(Arc::new(
            move |credential, url, usage, body, content_type| {
                kobo_policy::credentials::allowed_request(
                    &credential_app,
                    credential,
                    url,
                    usage,
                    body,
                    content_type,
                )
            },
        ))
        .with_wake(Arc::new(move || {
            let _ = waker.send(Event::TaskReady);
        }))
        .with_capabilities(declared.iter());
    let shelf_root = if name == "audiobook" {
        // `.mp3z` is the firmware's sideloaded-audiobook container. Keeping
        // this one privileged shelf in a visible directory means Nickel finds
        // the finished archive after the panel session ends. Every other app
        // remains confined to its private Cobalt data directory.
        PathBuf::from("/mnt/onboard/Audiobooks")
    } else {
        Path::new(DATA_ROOT).join(&name)
    };
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
        shelf: kobo_policy::shelf::Shelf::new(shelf_root),
        name,
        version,
        path: path.to_path_buf(),
        jail,
        child,
        stream,
        tasks,
        declared,
        screen: None,
        pictures: PictureCache::default(),
        fonts: BTreeMap::new(),
        orientation: kobo_ui::Orientation::Portrait,
        landscape_turn: kobo_ui::LandscapeTurn::Clockwise,
        painted: 0,
        used: Instant::now(),
    });
    Ok(id)
}

/// Repeats an application's standard error into the trace, line by line.
///
/// On its own thread because the application writes when it has something to
/// say, which is rarely and then all at once, and the session cannot wait on
/// it either way.
fn report_what_an_application_says(name: String, stderr: std::process::ChildStderr) {
    std::thread::spawn(move || {
        use std::io::BufRead;
        for line in std::io::BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            if line.trim().is_empty() {
                continue;
            }
            trace(&format!("{name} said: {line}"));
            println!("{name} said: {line}");
        }
    });
}

/// Ends one hosted application and everything it started.
fn stop_hosted(mut app: Hosted) {
    // Said out loud, because an application that stopped on its own stopped
    // for a reason, and the number is the whole of what the system knows: a
    // signal says it was killed and which way, a status says it gave up.
    if let Ok(Some(status)) = app.child.try_wait() {
        trace(&format!("{} ended with {status}", app.name));
        println!("{} ended with {status}", app.name);
    }
    for (_, handle) in std::mem::take(&mut app.fonts) {
        kobo_ui::drop_book_typesetter(handle);
    }
    app.tasks.shutdown();
    stop_application(&mut app.child, app.jail.as_deref());
    if let Some(failure) = app.child.trace_failure() {
        trace(&format!(
            "{} syscall supervisor failed: {failure}",
            app.name
        ));
        println!("{} syscall supervisor failed: {failure}", app.name);
    }
    if let Some(root) = app.jail {
        let _ignored = fs::remove_dir_all(root);
    }
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
    if !valid_application_name(name) {
        return Err(format!("{name:?} is not a valid application name"));
    }
    if crate::app_store::manages_builtin(name) {
        return crate::app_store::resolve(Path::new(COBALT_ROOT), name);
    }
    prefer_installed(catalogue, name, || {
        crate::app_store::resolve(Path::new(COBALT_ROOT), name)
    })
}

/// Chooses between the copy the store installed and the binary sitting beside
/// the runtime, for an application the runtime does not carry itself.
///
/// The store is asked first and the binary beside the runtime answers only
/// when the store has none. Both can name the same application: the store
/// writes its copy by installing or updating one, while the binary beside the
/// runtime is whatever the last platform package left there. Asking in the
/// other order pins the application to that leftover for good, because no
/// update can replace it -- the store installs a newer copy, reports success,
/// and the reader goes on starting the old binary with nothing to say it is
/// happening. An application carried by an older package and dropped from a
/// later one is where this bites, because the file outlives the release that
/// put it there.
///
/// The lookup is a parameter so this decision can be tested: the real one
/// verifies a signature against the compiled-in release key, which a test
/// cannot produce.
fn prefer_installed<F>(catalogue: &Path, name: &str, installed: F) -> Result<PathBuf, String>
where
    F: FnOnce() -> Result<PathBuf, String>,
{
    installed().or_else(|_| {
        let path = catalogue.join(format!("kobo-{name}"));
        if path.is_file() {
            Ok(path)
        } else {
            Err(format!("no application named {name} is installed"))
        }
    })
}

fn valid_application_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
}

/// The identity attached to an installed binary by its catalogue entry.
fn installed_name(path: &Path) -> Result<String, String> {
    let file = path
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .ok_or_else(|| format!("{} has no UTF-8 file name", path.display()))?;
    let name = file
        .strip_prefix("kobo-")
        .filter(|name| valid_application_name(name))
        .ok_or_else(|| format!("{} is not an installed application name", path.display()))?;
    Ok(name.to_owned())
}

/// How long a finger must stay down for a touch to count as a hold.
///
/// Half a second, which is what every touch platform settled on: shorter and
/// an unhurried tap becomes a gesture nobody asked for, longer and the reader
/// concludes the panel is ignoring them and lifts off.
/// Long enough to distinguish a deliberate contextual hold from the immediate
/// tap that opens a destination, while remaining inside the observed 1–1.5 s
/// reader comfort range.
const HOLD_TIME: Duration = Duration::from_millis(1_100);

/// Briefly holds a release so an application's next screen can carry it.
///
/// These are far shorter than a panel transition and therefore add no visible
/// hold. A keyboard gets a little more room because one key commonly changes
/// both the key face and a text field at the opposite end of the screen.
const CONTROL_RELEASE_GRACE: Duration = Duration::from_millis(12);
const KEYBOARD_RELEASE_GRACE: Duration = Duration::from_millis(30);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FeedbackKind {
    Control,
    KeyboardKey,
}

fn feedback_kind(layout: &Layout, rect: kobo_ui::Rect) -> FeedbackKind {
    if layout
        .nodes
        .iter()
        .any(|node| node.rect == rect && matches!(node.kind, LayoutKind::Cell(_, CellStyle::Key)))
    {
        FeedbackKind::KeyboardKey
    } else {
        FeedbackKind::Control
    }
}

fn release_grace(class: FeedbackKind) -> Duration {
    match class {
        FeedbackKind::Control => CONTROL_RELEASE_GRACE,
        FeedbackKind::KeyboardKey => KEYBOARD_RELEASE_GRACE,
    }
}

/// How far the finger may wander and still be holding, in pixels.
///
/// A finger resting on glass is never still, and this panel reports every
/// tremor. Roughly three millimetres on a Clara, which is under the width of
/// the contact patch, so a hand that is not moving cannot cross it.
const HOLD_SLIP: i32 = 40;

/// Ends the application, politely if it has already finished and firmly if not.
fn stop_application(child: &mut ApplicationChild, jail: Option<&Path>) {
    let group = child.id();
    let _ignored = kobo_abi::process_group::signal(group, kobo_abi::process_group::SIGTERM);
    if let Some(root) = jail {
        let _ignored = kobo_abi::sandbox::signal(root, kobo_abi::process_group::SIGTERM);
    }
    let deadline = Instant::now() + APP_STOP_GRACE;
    while let Ok(None) = child.try_wait() {
        if Instant::now() >= deadline {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    // The process-group signal handles normal and development-host launches.
    // The chroot sweep is a second identity on device: it catches a descendant
    // even on a kernel too old to install the filter that prevents spawning it.
    let _ignored = kobo_abi::process_group::signal(group, kobo_abi::process_group::SIGKILL);
    if let Some(root) = jail {
        // Repeated because a legacy-kernel process could fork while the first
        // procfs scan was in progress. Current kernels deny clone in seccomp.
        for _ in 0..3 {
            if kobo_abi::sandbox::signal(root, kobo_abi::process_group::SIGKILL).ok() == Some(0) {
                break;
            }
            thread::yield_now();
        }
    }
    if child.try_wait().ok().flatten().is_none() {
        let _ignored = child.wait();
    }
}

fn with_trace_failure(error: String, child: &ApplicationChild) -> String {
    match child.trace_failure() {
        Some(failure) => format!("{error}; syscall supervisor failed: {failure}"),
        None => error,
    }
}

/// Resolves a touch to the action it activates, if any.
///
/// Activation happens on release rather than on contact, so a finger that lands
/// on the wrong control can be slid away from it without acting. That is what
/// every touch interface the owner already uses has taught them to expect.
#[cfg(test)]
fn action_for(
    event: TouchEvent,
    screen: Option<&Screen>,
    chrome: &Chrome,
    held: bool,
) -> Option<ActionId> {
    action_for_oriented(
        event,
        screen,
        chrome,
        held,
        kobo_ui::Orientation::Portrait,
        kobo_ui::LandscapeTurn::Clockwise,
    )
}

fn action_for_oriented(
    event: TouchEvent,
    screen: Option<&Screen>,
    chrome: &Chrome,
    held: bool,
    orientation: kobo_ui::Orientation,
    landscape_turn: kobo_ui::LandscapeTurn,
) -> Option<ActionId> {
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
    let physical = crate::device_metrics();
    let metrics = metrics_for(screen).oriented(orientation);
    let (x, y) = kobo_ui::logical_point_with_turn(
        orientation,
        landscape_turn,
        physical.width,
        physical.height,
        x,
        y,
    );
    let layout = screen.layout_with(&metrics, chrome);
    // A hold that the screen asked for wins over the tap the same pixels would
    // otherwise have been. Falls back rather than swallowing the touch: a
    // finger resting a moment too long on a page of a book that does not want
    // holds must still turn the page.
    let hit = if held {
        layout.hit_hold(x, y).or_else(|| layout.hit_test(x, y))
    } else {
        layout.hit_test(x, y)
    };
    // Reported so a tap that lands on nothing stays distinguishable from a tap
    // that never arrived at all. Diagnosing the difference without this cost a
    // whole debugging session.
    trace(&format!("touch up ({x},{y}) -> {hit:?}"));
    println!("touch up ({x},{y}) -> {hit:?}");
    hit
}

/// Resolves a page-key press to the message the application should hear.
///
/// The sibling of [`action_for`], and deliberately the same shape: a press and
/// a tap on a side zone mean the same thing, so they must not disagree about
/// what a screen currently allows.
///
/// `None` means the press is dropped. That is not the same as an application
/// choosing to ignore it, which is why the three states are matched rather
/// than collapsed: see [`kobo_ui::PagingState`].
fn page_key_message(
    screen: &Screen,
    chrome: &Chrome,
    forward: bool,
) -> Option<kobo_protocol::Message> {
    match screen.layout_with(&metrics_for(screen), chrome).page_turns {
        kobo_ui::PagingState::Declared(turns) => Some(kobo_protocol::Message::Action {
            action: if forward { turns.next } else { turns.previous },
        }),
        kobo_ui::PagingState::None => Some(kobo_protocol::Message::PageTurn { forward }),
        // Dropped, but not silently: a press that does nothing is
        // indistinguishable from a broken button, and this is the record that
        // says which it was. Both channels, as `action_for` does it, because
        // the black box is off unless somebody asked for it and this has to
        // be answerable in an ordinary session.
        kobo_ui::PagingState::SuppressedByOverlay => {
            trace("page key dropped: an overlay is up");
            println!("page key dropped: an overlay is up");
            None
        }
    }
}

fn text_hold_for_oriented(
    event: TouchEvent,
    screen: Option<&Screen>,
    chrome: &Chrome,
    held: bool,
    orientation: kobo_ui::Orientation,
) -> Option<(ActionId, kobo_ui::TextHit)> {
    if !held {
        return None;
    }
    let TouchEvent::Up { x, y } = event else {
        return None;
    };
    let screen = screen?;
    let (Ok(x), Ok(y)) = (i32::try_from(x), i32::try_from(y)) else {
        return None;
    };
    let layout = screen.layout_with(&metrics_for(screen).oriented(orientation), chrome);
    let action = layout.hold?;
    layout.hit_text(x, y).map(|hit| (action, hit))
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
    expected_name: &str,
) -> Result<(std::os::unix::net::UnixStream, String, u8), String> {
    let (mut stream, _) = listener
        .accept()
        .map_err(|error| format!("application never connected: {error}"))?;
    let hello =
        kobo_protocol::read_from(&mut stream).map_err(|error| format!("first message: {error}"))?;
    let Message::Hello { name } = hello.message else {
        return Err("the first application message must be Hello".to_owned());
    };
    if name != expected_name {
        return Err(format!(
            "application identity mismatch: launched {expected_name:?}, but it said {name:?}"
        ));
    }
    kobo_protocol::write_to(
        &mut stream,
        &Frame {
            version: hello.version,
            request_id: hello.request_id,
            message: Message::Welcome {
                width: u16::try_from(whole_screen.width).unwrap_or(u16::MAX),
                height: u16::try_from(whole_screen.height).unwrap_or(u16::MAX),
                // The panel this runtime renders for. An application that
                // measures text has to measure it for the same one, and pixel
                // counts alone do not say how large a pixel is.
                pixels_per_inch: u16::try_from(crate::device_metrics().pixels_per_inch)
                    .unwrap_or(u16::MAX),
                text_scale: crate::device_metrics().text_scale,
            },
        },
    )
    .map_err(|error| format!("welcome: {error}"))?;
    Ok((stream, name, hello.version))
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
/// refusal on it (see [`Screen::owns_back`]) so that a screen reached from
/// inside an application goes back to where it was reached from rather than
/// out of the application. That is a delivery, not a transfer of ownership:
/// the caller still leaves if no new screen follows.
#[allow(
    clippy::too_many_arguments,
    reason = "touch delivery needs the negotiated protocol, retained screen, and physical pose"
)]
fn deliver_touch(
    stream: &mut std::os::unix::net::UnixStream,
    version: u8,
    event: TouchEvent,
    current: Option<&Screen>,
    chrome: &Chrome,
    held: bool,
    orientation: kobo_ui::Orientation,
    landscape_turn: kobo_ui::LandscapeTurn,
) -> Result<Tap, String> {
    let logical_event = match event {
        TouchEvent::Up { x, y } => {
            let (Ok(x), Ok(y)) = (i32::try_from(x), i32::try_from(y)) else {
                return Ok(Tap::Handled);
            };
            let physical = crate::device_metrics();
            let (x, y) = kobo_ui::logical_point_with_turn(
                orientation,
                landscape_turn,
                physical.width,
                physical.height,
                x,
                y,
            );
            TouchEvent::Up {
                x: u32::try_from(x).map_err(|_| "logical touch x is negative")?,
                y: u32::try_from(y).map_err(|_| "logical touch y is negative")?,
            }
        }
        other => other,
    };
    if let Some((action, hit)) =
        text_hold_for_oriented(logical_event, current, chrome, held, orientation)
    {
        kobo_protocol::write_to(
            stream,
            &Frame {
                version,
                request_id: 0,
                message: Message::TextHold {
                    action,
                    context: hit.context,
                    start: hit.start,
                    end: hit.end,
                },
            },
        )
        .map_err(|error| format!("deliver a text hold: {error}"))?;
        return Ok(Tap::Handled);
    }
    let Some(action) =
        action_for_oriented(event, current, chrome, held, orientation, landscape_turn)
    else {
        return Ok(Tap::Handled);
    };
    let offered = action == ActionId::BACK;
    if offered && !current.is_some_and(|screen| screen.owns_back) {
        return Ok(Tap::Leave);
    }
    kobo_protocol::write_to(
        stream,
        &Frame {
            version,
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

        let started = Instant::now();
        // Convert and write only the rows the transition touches. The write
        // path runs at a few megabytes per second on the i.MX6's uncached
        // framebuffer, so writing the whole screen for every frame cost about
        // 1.6 seconds per tap regardless of how small the change was.
        let region_gray = {
            let out_of_surface = || "the transition region is not inside the surface".to_owned();
            let x = usize::try_from(transition.region.x).map_err(|_| out_of_surface())?;
            let y = usize::try_from(transition.region.y).map_err(|_| out_of_surface())?;
            let width = usize::try_from(transition.region.width).map_err(|_| out_of_surface())?;
            let height = usize::try_from(transition.region.height).map_err(|_| out_of_surface())?;
            let mut gray = Vec::with_capacity(width.saturating_mul(height));
            for row in 0..height {
                let start = (y + row) * surface.width + x;
                let end = start + width;
                gray.extend_from_slice(surface.pixels.get(start..end).ok_or_else(out_of_surface)?);
            }
            gray
        };
        let frame = RegionSnapshot::from_grayscale(display.geometry(), region, &region_gray)
            .map_err(|error| format!("prepare the frame: {error}"))?;
        let converted = started.elapsed();
        let fence = display
            .restore_timed(&frame)
            .map_err(|error| format!("write the frame: {error}"))?;
        let written = started.elapsed();
        let plan = RefreshPlan::new(
            region,
            intent,
            transition.full,
            whole_screen.width,
            whole_screen.height,
        )
        .ok_or_else(|| "the refresh region is not inside the screen".to_owned())?;
        let timing = display
            .refresh_deferred(plan)
            .map_err(|error| format!("show the frame: {error}"))?;
        // One line per frame: how long the grayscale conversion, the
        // framebuffer write and the two ioctls each took, and what was
        // refreshed with which waveform. This is what found the Libra 2 tap
        // delay, so it stays, but off by default and behind its own switch:
        // every frame on every device is the wrong place for unconditional
        // output. Stderr rather than the black box, which costs an fsync per
        // line; start.sh already captures stderr.
        if frame_timing_wanted() {
            eprintln!(
                "frame {}x{} wf={} convert={}ms write={}ms submit={}us fence={}ms completed={} pending={}",
                region.width,
                region.height,
                timing.submitted_waveform,
                converted.as_millis(),
                written.saturating_sub(converted).as_millis(),
                timing.submit.as_micros(),
                fence.wait.saturating_add(timing.prior.wait).as_millis(),
                fence.completed.saturating_add(timing.prior.completed),
                timing.unfinished,
            );
        }

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

    /// Same policy as taps: between applications a press is dropped, not
    /// queued for whatever comes up next.
    fn send_gpio(&self, event: GpioEvent) {
        let guard = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(sender) = guard.as_ref() {
            let _ignored = sender.send(Event::Gpio(event));
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

/// One reader thread on the button device for the whole panel session,
/// mirroring [`pump_touch`] for the same reason: the destination changes as
/// applications come and go, the thread does not.
fn pump_gpio(buttons: &mut GpioSession, sink: &TouchSink) {
    let Some(events) = buttons.take_events() else {
        return;
    };
    let sink = sink.clone();
    thread::spawn(move || {
        while let Ok(event) = events.recv() {
            sink.send_gpio(event);
        }
    });
}

/// Turns a caught signal into an ordinary loop event.
///
/// A signal handler may not lock, allocate or send on a channel, so it only
/// records a number; this thread is what carries it into the loop. Polling is
/// the right shape here despite the comment on [`Event`] about staying asleep:
/// a tenth of a second of an idle thread costs nothing measurable next to the
/// panel, and the alternative (a self pipe) buys latency that a session giving
/// four pieces of hardware back cannot use.
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

/// Asks, from its own thread, whether newer software has been published.
///
/// Only the asking happens here: what it finds is sent into the loop as
/// [`Event::AutoUpdate`] and applied when the panel is quiet, because
/// applying replaces binaries and stops applications and only the loop knows
/// whether that is safe right now. The owner's choices are read before every
/// check, so a switch turned off in settings stops the next check without a
/// restart. The thread ends with the process, which is right after the one
/// session this process ever runs.
fn watch_for_stale_software(sender: &Sender<Event>) {
    let sender = sender.clone();
    thread::spawn(move || {
        let mut delay = AUTO_UPDATE_FIRST_CHECK;
        loop {
            thread::sleep(delay);
            delay = AUTO_UPDATE_RECHECK;
            let root = Path::new(COBALT_ROOT);
            let chosen = crate::autoupdate::preferences(root);
            if !chosen.cobalt && !chosen.apps {
                continue;
            }
            let plan = crate::autoupdate::plan(root, chosen, env!("CARGO_PKG_VERSION"));
            if plan.is_empty() {
                continue;
            }
            if sender.send(Event::AutoUpdate(plan)).is_err() {
                return;
            }
        }
    });
}

fn pump_application(
    stream: &std::os::unix::net::UnixStream,
    sender: &Sender<Event>,
    id: u64,
    version: u8,
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
        if frame.version != version {
            let _ignored = sender.send(Event::AppGone(id));
            return;
        }
        if sender.send(Event::App(id, Box::new(frame))).is_err() {
            return;
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use kobo_policy::{Capability, TaskRunner};
    use kobo_protocol::{
        Credential, CredentialUse, Frame, Header, Message, Task, TaskId, TaskOutcome,
    };
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
    use rustls::{ServerConfig, ServerConnection, StreamOwned};
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{mpsc, Arc, Once};
    use std::thread;
    use std::time::Duration;

    const MOCK_CA: &[u8] = include_bytes!("../../kobo-net/tests/fixtures/localhost-ca.der");
    const MOCK_CERTIFICATE: &[u8] =
        include_bytes!("../../kobo-net/tests/fixtures/localhost-cert.der");
    const MOCK_PRIVATE_KEY: &[u8] =
        include_bytes!("../../kobo-net/tests/fixtures/localhost-key.der");
    const SEEK_BODY: &str = "rated=true&time=10&increment=0&variant=standard&color=random";
    const FORM: &str = "application/x-www-form-urlencoded";

    fn trust_mock_root() {
        static TRUST: Once = Once::new();
        TRUST.call_once(|| {
            kobo_net::trust_owner_root(MOCK_CA.to_vec()).expect("install mock root");
        });
    }

    fn mock_server_config() -> Arc<ServerConfig> {
        let certificate = CertificateDer::from(MOCK_CERTIFICATE.to_vec());
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(MOCK_PRIVATE_KEY.to_vec()));
        Arc::new(
            ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .expect("protocol versions")
                .with_no_client_auth()
                .with_single_cert(vec![certificate], key)
                .expect("mock certificate"),
        )
    }

    fn accept_mock(
        listener: &TcpListener,
        config: Arc<ServerConfig>,
    ) -> (StreamOwned<ServerConnection, TcpStream>, Vec<u8>) {
        let (socket, _) = listener.accept().expect("accept mock client");
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("read timeout");
        let connection = ServerConnection::new(config).expect("server connection");
        let mut stream = StreamOwned::new(connection, socket);
        let mut request = Vec::new();
        let mut buffer = [0_u8; 2048];
        loop {
            let read = stream.read(&mut buffer).expect("read request");
            assert!(read > 0, "client closed before request");
            request.extend_from_slice(&buffer[..read]);
            assert!(request.len() < 32 * 1024, "oversized test request");
            let Some(head_end) = request
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|position| position + 4)
            else {
                continue;
            };
            let head = std::str::from_utf8(&request[..head_end]).expect("request head");
            let content_length = head
                .split("\r\n")
                .find_map(|line| {
                    line.strip_prefix("Content-Length: ")
                        .and_then(|value| value.parse::<usize>().ok())
                })
                .unwrap_or(0);
            if request.len() >= head_end + content_length {
                return (stream, request);
            }
        }
    }

    fn has_header(request: &[u8], prefix: &str) -> bool {
        std::str::from_utf8(request)
            .is_ok_and(|request| request.split("\r\n").any(|line| line.starts_with(prefix)))
    }

    fn request_body(request: &[u8]) -> &[u8] {
        let start = request
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .expect("request separator")
            + 4;
        &request[start..]
    }

    fn test_root(name: &str) -> PathBuf {
        let target = std::env::var_os("CARGO_TARGET_DIR").map_or_else(
            || {
                std::env::current_dir()
                    .expect("current directory")
                    .join("target")
            },
            PathBuf::from,
        );
        target
            .join("kobod-test-state")
            .join(format!("{name}-{}", std::process::id()))
    }

    fn through_protocol_11(task: TaskId, work: Task) -> (TaskId, Task) {
        let mut encoded = Vec::new();
        kobo_protocol::write_to(
            &mut encoded,
            &Frame {
                version: kobo_protocol::LEGACY_VERSION,
                request_id: task.0,
                message: Message::Spawn { task, work },
            },
        )
        .expect("encode protocol-11 task");
        let decoded =
            kobo_protocol::read_from(&mut std::io::Cursor::new(encoded)).expect("decode task");
        assert_eq!(decoded.version, kobo_protocol::LEGACY_VERSION);
        assert_eq!(decoded.request_id, task.0);
        match decoded.message {
            Message::Spawn { task, work } => (task, work),
            other => panic!("decoded the wrong message: {other:?}"),
        }
    }

    fn stream_work(url: &str, action: &str) -> Task {
        Task::Fetch {
            url: url.to_owned(),
            offset: 0,
            max_bytes: 128 * 1024,
            credential: Some(Credential::bearer("lichess")),
            headers: vec![
                Header::new("Accept", "application/x-ndjson"),
                Header::new("X-Cobalt-Line-Stream", action),
                Header::new("X-Cobalt-Rate-Limit", "1"),
            ],
        }
    }

    struct EventMock {
        url: String,
        release: mpsc::Sender<()>,
        server: thread::JoinHandle<()>,
    }

    fn start_event_mock(config: Arc<ServerConfig>) -> EventMock {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind event mock");
        let port = listener.local_addr().expect("event address").port();
        let (release, wait) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut stream, request) = accept_mock(&listener, config);
            assert!(request.starts_with(b"GET /api/stream/event HTTP/1.1\r\n"));
            assert!(has_header(&request, "Authorization: Bearer "));
            assert!(has_header(&request, "Accept: application/x-ndjson"));
            assert!(!has_header(&request, "X-Cobalt-"));
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nTransfer-Encoding: chunked\r\nConnection: keep-alive\r\n\r\n",
                )
                .expect("event response head");
            stream.write_all(b"1\r\n\n\r\n").expect("event keepalive");
            let event = br#"{"type":"gameStart","game":{"id":"abcdEF12"}}"#;
            write!(stream, "{:x}\r\n", event.len() + 1).expect("event chunk size");
            stream.write_all(event).expect("event record");
            stream.write_all(b"\n\r\n").expect("event frame");
            stream.flush().expect("flush event");
            let _ = wait.recv_timeout(Duration::from_secs(5));
        });
        EventMock {
            url: format!("https://localhost:{port}/api/stream/event"),
            release,
            server,
        }
    }

    struct SeekMock {
        url: String,
        requests: Arc<AtomicUsize>,
        started: mpsc::Receiver<()>,
        release: mpsc::Sender<()>,
        server: thread::JoinHandle<()>,
    }

    fn start_seek_mock(config: Arc<ServerConfig>) -> SeekMock {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind seek mock");
        let port = listener.local_addr().expect("seek address").port();
        let requests = Arc::new(AtomicUsize::new(0));
        let observed_requests = Arc::clone(&requests);
        let (report_start, started) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut stream, request) = accept_mock(&listener, config);
            observed_requests.fetch_add(1, Ordering::SeqCst);
            assert!(request.starts_with(b"POST /api/board/seek HTTP/1.1\r\n"));
            assert!(has_header(
                &request,
                "Content-Type: application/x-www-form-urlencoded"
            ));
            assert!(has_header(&request, "Authorization: Bearer "));
            assert!(!has_header(&request, "X-Cobalt-"));
            assert_eq!(request_body(&request), SEEK_BODY.as_bytes());
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nTransfer-Encoding: chunked\r\nConnection: keep-alive\r\n\r\n1\r\n\n\r\n",
                )
                .expect("seek response");
            stream.flush().expect("flush seek response");
            report_start.send(()).expect("report seek");
            let _ = wait.recv_timeout(Duration::from_secs(5));
        });
        SeekMock {
            url: format!("https://localhost:{port}/api/board/seek"),
            requests,
            started,
            release,
            server,
        }
    }

    fn assert_outcome(runner: &mut TaskRunner, task: TaskId, expected: &TaskOutcome) {
        let finished = runner
            .wait(Duration::from_secs(5))
            .unwrap_or_else(|| panic!("task {} did not finish", task.0));
        assert_eq!(finished.task, task);
        assert_eq!(&finished.outcome, expected);
    }

    #[test]
    fn an_application_cannot_change_protocol_version_after_greeting() {
        let (runtime, mut application) =
            std::os::unix::net::UnixStream::pair().expect("socket pair");
        let (sender, receiver) = std::sync::mpsc::channel();
        super::pump_application(&runtime, &sender, 42, kobo_protocol::VERSION)
            .expect("start application pump");
        kobo_protocol::write_to(
            &mut application,
            &kobo_protocol::Frame {
                version: kobo_protocol::LEGACY_VERSION,
                request_id: 1,
                message: kobo_protocol::Message::Exit,
            },
        )
        .expect("send mixed-version frame");
        assert!(matches!(
            receiver
                .recv_timeout(std::time::Duration::from_secs(1))
                .expect("application rejection"),
            super::Event::AppGone(42)
        ));
    }

    #[test]
    fn protocol_11_lichess_consumes_host_secret_for_stream_and_cancellable_seek() {
        trust_mock_root();
        let config = mock_server_config();
        let event = start_event_mock(Arc::clone(&config));
        let seek_mock = start_seek_mock(config);

        let root = test_root("protocol-11-lichess");
        let _ignored = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("test secret root");
        std::fs::write(root.join("lichess"), "generated-test-credential").expect("test credential");
        assert!(!root.join("apps/lichess/lichess").exists());
        let allowed_event = event.url.clone();
        let allowed_seek = seek_mock.url.clone();
        let mut runner = TaskRunner::simulated(root.join("data"))
            .with_fetch(Arc::new(kobo_net::fetch_from_controlled))
            .with_post(Arc::new(kobo_net::post_controlled))
            .with_line_streams(Arc::new(kobo_net::LineStreams::default()))
            .with_app_secrets(&root, "lichess")
            .with_credential_policy(Arc::new(
                move |credential, url, usage, body, content_type| {
                    credential == &Credential::bearer("lichess")
                        && match usage {
                            CredentialUse::Fetch => {
                                url == allowed_event && body.is_none() && content_type.is_none()
                            }
                            CredentialUse::Post => {
                                url == allowed_seek
                                    && body == Some(SEEK_BODY)
                                    && content_type == Some(FORM)
                            }
                        }
                },
            ))
            .with_capabilities([Capability::Network]);

        for (id, action, expected) in [
            (TaskId(1), "open", TaskOutcome::Completed(Vec::new())),
            (
                TaskId(2),
                "next",
                TaskOutcome::Completed(
                    br#"{"type":"gameStart","game":{"id":"abcdEF12"}}"#.to_vec(),
                ),
            ),
        ] {
            let (task, work) = through_protocol_11(id, stream_work(&event.url, action));
            runner.submit(task, work).expect("submit stream task");
            assert_outcome(&mut runner, task, &expected);
        }

        let seek = TaskId(3);
        let (seek, work) = through_protocol_11(
            seek,
            Task::Post {
                url: seek_mock.url.clone(),
                body: SEEK_BODY.to_owned(),
                content_type: FORM.to_owned(),
                credential: Some(Credential::bearer("lichess")),
                headers: vec![
                    Header::new("X-Cobalt-Wait-Until-Cancelled", "1"),
                    Header::new("X-Cobalt-Rate-Limit", "1"),
                ],
                max_bytes: 4096,
            },
        );
        runner.submit(seek, work).expect("submit seek");
        seek_mock
            .started
            .recv_timeout(Duration::from_secs(5))
            .expect("seek reached mock");
        runner.cancel(seek);
        assert_outcome(&mut runner, seek, &TaskOutcome::Cancelled);
        assert_eq!(
            seek_mock.requests.load(Ordering::SeqCst),
            1,
            "the non-idempotent seek was replayed"
        );

        let close = TaskId(4);
        let (close, work) = through_protocol_11(close, stream_work(&event.url, "close"));
        runner.submit(close, work).expect("submit stream close");
        assert_outcome(&mut runner, close, &TaskOutcome::Completed(Vec::new()));

        seek_mock.release.send(()).expect("release seek mock");
        event.release.send(()).expect("release event mock");
        seek_mock.server.join().expect("seek mock");
        event.server.join().expect("event mock");
        drop(runner);
        std::fs::remove_dir_all(root).expect("remove test state");
    }

    /// `TZ` is read from the environment, which is process-global, so these
    /// are one test rather than several: two tests setting it at once would
    /// see each other's value.
    #[test]
    fn the_clock_reads_the_offset_posix_spells_backwards() {
        // POSIX inverts the sign everyone expects: EST5 is five hours *behind*
        // UTC, not ahead. Getting this backwards puts the band ten hours out.
        let cases = [
            ("UTC0", 0),
            ("EST5", -5 * 3600),
            ("IST-5:30", 5 * 3600 + 30 * 60),
            ("CET-1", 3600),
            ("NZST-12:45", 12 * 3600 + 45 * 60),
            // Nothing understood is UTC rather than a guess.
            ("", 0),
            ("Etc/Unknown", 0),
        ];
        for (tz, want) in cases {
            std::env::set_var("TZ", tz);
            assert_eq!(
                super::local_offset_seconds(),
                want,
                "TZ={tz} was read as the wrong offset"
            );
        }
        std::env::remove_var("TZ");
        assert_eq!(
            super::local_offset_seconds(),
            0,
            "a device with no TZ at all did not fall back to UTC"
        );
    }

    #[test]
    fn the_clock_is_two_fields_and_never_a_twenty_fifth_hour() {
        let now = super::clock();
        assert_eq!(now.len(), 5, "the clock was not HH:MM: {now}");
        let (hours, minutes) = now.split_once(':').expect("a separator");
        assert!(hours.parse::<u32>().expect("hours") < 24, "{now}");
        assert!(minutes.parse::<u32>().expect("minutes") < 60, "{now}");
    }

    #[test]
    fn a_book_is_drawn_without_the_band_and_everything_else_with_it() {
        let mut status = super::StatusSource::new();
        let reading = Screen::new(1, Vec::new()).with_reading(true);
        assert!(
            super::chrome_for(&reading, false, &mut status)
                .status
                .is_none(),
            "a clock was put above a book"
        );
        let listing = Screen::new(1, Vec::new());
        assert!(
            super::chrome_for(&listing, false, &mut status)
                .status
                .is_some(),
            "an ordinary screen lost its status band"
        );
        // Home has no way back, and the band is independent of that.
        let home = super::chrome_for(&listing, true, &mut status);
        assert!(!home.back, "the launcher was given a way out of itself");
        assert!(home.status.is_some(), "the launcher lost its status band");
    }

    /// A rooted application's sub-screen still gets a way back.
    ///
    /// `kobo present` runs one application as home, and every screen it opened
    /// from there was drawn with no back control, because back was decided
    /// purely by whether the application was the launcher. Settings could
    /// reach its Bluetooth pane and then had nothing to return with, on a
    /// device whose only other way out is the power button.
    #[test]
    fn a_rooted_applications_sub_screen_is_still_drawn_a_way_back() {
        let mut status = StatusSource::new();
        let pane = Screen::new(1, Vec::new())
            .with_top_bar(kobo_ui::TopBar::new(kobo_ui::NodeId(1), "Bluetooth"))
            .with_own_back(true);
        let chrome = super::chrome_for(&pane, true, &mut status);
        assert!(
            chrome.back,
            "a screen that owns back was left with no way to use it"
        );

        let rooted = Screen::new(1, Vec::new())
            .with_top_bar(kobo_ui::TopBar::new(kobo_ui::NodeId(1), "Settings"));
        assert!(
            !super::chrome_for(&rooted, true, &mut status).back,
            "an application's own root still has nowhere to go back to"
        );
    }

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
        assert!(!&Chrome::with_back(*home.as_path() != *home.as_path()).back);
        assert!(&Chrome::with_back(Path::new("/tmp/kobo-hello") != home).back);
    }

    #[test]
    fn an_application_that_forgot_a_top_bar_still_gets_a_way_back() {
        let bare = Screen::new(1, Vec::new());
        let fixed = kobo_ui::ensure_way_back(bare, &Chrome::with_back(true), "Hello");
        assert_eq!(
            fixed.top_bar.as_ref().map(|bar| bar.title.as_str()),
            Some("Hello")
        );
        // The launcher itself is not given one it did not ask for.
        assert!(kobo_ui::ensure_way_back(
            Screen::new(1, Vec::new()),
            &Chrome::default(),
            "Launcher"
        )
        .top_bar
        .is_none());
    }

    #[test]
    fn a_held_finger_reaches_the_hold_and_a_quick_one_turns_the_page() {
        // The two intents land on the same pixels, so the only thing telling
        // them apart is how long the finger stayed down. A reading page is
        // page turns from edge to edge: without this, marking a paragraph has
        // nowhere to be asked for, and with it done wrong every page turn
        // becomes a mark.
        let screen = Screen::new(
            1,
            vec![kobo_ui::Node::Text {
                id: kobo_ui::NodeId(1),
                text: "A page of a book.".to_owned(),
                links: Vec::new(),
            }],
        )
        .with_page_turns(ActionId(11), ActionId(12))
        .with_hold(ActionId(13));
        let chrome = Chrome::default();
        let metrics = metrics_for(&screen);
        let content = screen.layout_with(&metrics, &chrome).content;
        let touch = TouchEvent::Up {
            x: u32::try_from(metrics.width / 2).expect("inside the panel"),
            y: u32::try_from(content.y + content.height / 2).expect("inside the panel"),
        };
        assert_eq!(
            action_for(touch, Some(&screen), &chrome, true),
            Some(ActionId(13))
        );
        assert_eq!(
            action_for(touch, Some(&screen), &chrome, false),
            Some(ActionId(12))
        );
        // A screen that asked for no hold must still turn its pages, however
        // long the finger rested.
        let no_hold = Screen::new(1, Vec::new()).with_page_turns(ActionId(11), ActionId(12));
        assert_eq!(
            action_for(touch, Some(&no_hold), &chrome, true),
            Some(ActionId(12))
        );

        let covered = screen.clone().with_overlay(kobo_ui::Overlay::modal(
            kobo_ui::NodeId(9),
            "Question",
            Vec::new(),
        ));
        assert_eq!(
            text_hold_for_oriented(
                touch,
                Some(&covered),
                &chrome,
                true,
                kobo_ui::Orientation::Portrait,
            ),
            None,
            "covered text received a hold through the modal"
        );
        assert_ne!(
            action_for(touch, Some(&covered), &chrome, true),
            Some(ActionId(13)),
            "the modal leaked the page hold"
        );
    }

    #[test]
    fn landscape_taps_reach_the_same_control_from_either_physical_side() {
        let action = ActionId(77);
        let screen = Screen::new(
            1,
            vec![kobo_ui::Node::Button {
                id: kobo_ui::NodeId(1),
                action,
                label: "Open".to_owned(),
                state: kobo_ui::ControlState::Enabled,
                emphasis: kobo_ui::Emphasis::Primary,
            }],
        );
        let chrome = kobo_ui::Chrome::default();
        let physical = crate::device_metrics();
        let logical_metrics = physical.oriented(kobo_ui::Orientation::Landscape);
        let rect = screen
            .layout_with(&logical_metrics, &chrome)
            .rect_of_action(action)
            .expect("button");
        let logical_x = rect.x + rect.width / 2;
        let logical_y = rect.y + rect.height / 2;

        let clockwise = TouchEvent::Up {
            x: u32::try_from(physical.width - 1 - logical_y).expect("physical x"),
            y: u32::try_from(logical_x).expect("physical y"),
        };
        assert_eq!(
            super::action_for_oriented(
                clockwise,
                Some(&screen),
                &chrome,
                false,
                kobo_ui::Orientation::Landscape,
                kobo_ui::LandscapeTurn::Clockwise,
            ),
            Some(action)
        );

        let counter_clockwise = TouchEvent::Up {
            x: u32::try_from(logical_y).expect("physical x"),
            y: u32::try_from(physical.height - 1 - logical_x).expect("physical y"),
        };
        assert_eq!(
            super::action_for_oriented(
                counter_clockwise,
                Some(&screen),
                &chrome,
                false,
                kobo_ui::Orientation::Landscape,
                kobo_ui::LandscapeTurn::CounterClockwise,
            ),
            Some(action)
        );
    }

    /// The three states a page key can land in, and the one that used to be
    /// wrong: a press while a dialog is up is dropped, not passed on raw.
    #[test]
    fn a_page_key_is_dropped_while_an_overlay_is_up() {
        let chrome = Chrome::default();
        let page = || kobo_ui::Node::Text {
            id: kobo_ui::NodeId(1),
            text: "A page of a book.".to_owned(),
            links: Vec::new(),
        };

        // Declares turns: the press becomes the declared action, exactly as a
        // tap on the side zone would have.
        let declared = Screen::new(1, vec![page()]).with_page_turns(ActionId(11), ActionId(12));
        assert!(matches!(
            page_key_message(&declared, &chrome, true),
            Some(kobo_protocol::Message::Action {
                action: ActionId(12)
            })
        ));
        assert!(matches!(
            page_key_message(&declared, &chrome, false),
            Some(kobo_protocol::Message::Action {
                action: ActionId(11)
            })
        ));

        // Declares nothing: the application may make its own sense of the
        // press, so it hears the raw intent.
        let undeclared = Screen::new(1, vec![page()]);
        assert!(matches!(
            page_key_message(&undeclared, &chrome, true),
            Some(kobo_protocol::Message::PageTurn { forward: true })
        ));
        assert!(matches!(
            page_key_message(&undeclared, &chrome, false),
            Some(kobo_protocol::Message::PageTurn { forward: false })
        ));

        // Covered by a dialog: nothing is sent. Not the declared action, and
        // not the raw intent either -- an application that handled the raw
        // intent would have paged the content underneath the dialog, which is
        // the bug this state exists to make unrepresentable.
        let modal = kobo_ui::Overlay::modal(
            kobo_ui::NodeId(40),
            "Leave?",
            vec![kobo_ui::Node::Button {
                id: kobo_ui::NodeId(41),
                action: ActionId(6),
                label: "Leave".to_owned(),
                state: kobo_ui::ControlState::Enabled,
                emphasis: kobo_ui::Emphasis::Primary,
            }],
        );
        assert_eq!(
            page_key_message(&declared.clone().with_overlay(modal.clone()), &chrome, true),
            None
        );
        // Including when the screen never declared turns: the dialog is what
        // the reader is answering either way.
        assert_eq!(
            page_key_message(&undeclared.with_overlay(modal), &chrome, true),
            None
        );
    }

    #[test]
    fn back_is_reported_from_the_chrome_the_frame_was_drawn_with() {
        let screen = hello();
        let chrome = Chrome::with_back(true);
        let back = screen
            .layout_with(&crate::device_metrics(), &chrome)
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
            &chrome,
            false,
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
                &Chrome::default(),
                false,
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
            .layout_with(&crate::device_metrics(), &chrome)
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
            deliver_touch(
                &mut runtime,
                kobo_protocol::VERSION,
                tap,
                Some(&screen),
                &chrome,
                false,
                kobo_ui::Orientation::Portrait,
                kobo_ui::LandscapeTurn::Clockwise,
            )
            .expect("route the tap"),
            Tap::Leave,
            "a screen that did not ask keeps the old behaviour"
        );

        let owning = screen.clone().with_own_back(true);
        assert_eq!(
            deliver_touch(
                &mut runtime,
                kobo_protocol::VERSION,
                tap,
                Some(&owning),
                &chrome,
                false,
                kobo_ui::Orientation::Portrait,
                kobo_ui::LandscapeTurn::Clockwise,
            )
            .expect("route the tap"),
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
    fn a_binary_name_is_the_identity_the_handshake_must_use() {
        let path = catalogue().join("kobo-hello");
        assert_eq!(super::installed_name(&path).as_deref(), Ok("hello"));
        for path in ["hello", "kobo-Terminal", "kobo-../terminal", "kobo-"] {
            assert!(super::installed_name(std::path::Path::new(path)).is_err());
        }
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

    /// The update an application receives has to be the one that starts.
    ///
    /// A reader had arXiv left beside the runtime by a package old enough to
    /// predate the current wire protocol. The store updated the application,
    /// said so, and the reader went on starting the leftover, which the
    /// runtime then refused for speaking a protocol it no longer answers. The
    /// application was pinned to a binary no update could reach, and the only
    /// visible symptom was a screen that said Starting and stayed there.
    #[test]
    fn the_installed_copy_is_started_even_when_a_binary_sits_beside_the_runtime() {
        let directory = catalogue();
        let installed = directory.join("installed-elsewhere/kobo-hello");
        assert_eq!(
            super::prefer_installed(&directory, "hello", || Ok(installed.clone()))
                .expect("the installed copy answers"),
            installed,
            "the leftover beside the runtime shadowed the installed copy"
        );
    }

    #[test]
    fn a_binary_beside_the_runtime_answers_only_when_nothing_is_installed() {
        let directory = catalogue();
        assert_eq!(
            super::prefer_installed(&directory, "hello", || Err("nothing installed".to_owned()))
                .expect("the binary beside the runtime answers"),
            directory.join("kobo-hello")
        );
        assert!(super::prefer_installed(&directory, "nothing-here", || Err(
            "nothing installed".to_owned()
        ))
        .is_err());
    }
}

#[cfg(test)]
mod hosting_tests {
    use super::coldest;
    use std::time::{Duration, Instant};

    /// Reads better than a bare bool at every call site below.
    const BUSY: bool = true;
    const IDLE: bool = false;

    fn ago(seconds: u64) -> Instant {
        Instant::now()
            .checked_sub(Duration::from_secs(seconds))
            .unwrap_or_else(Instant::now)
    }

    #[test]
    fn the_application_left_alone_longest_is_the_one_that_goes() {
        let seen = [(1, ago(30), IDLE), (2, ago(300), IDLE), (3, ago(5), IDLE)];
        assert_eq!(coldest(&seen, 3), Some(1));
    }

    #[test]
    fn the_one_on_the_panel_is_never_stopped_even_when_it_is_the_oldest() {
        // The front application is the oldest here by a wide margin, because
        // `used` records when it was last brought forward rather than when it
        // was last touched. Stopping it would close what the reader is looking
        // at in order to open something else.
        let seen = [(1, ago(900), IDLE), (2, ago(10), IDLE)];
        assert_eq!(coldest(&seen, 1), Some(1));
    }

    #[test]
    fn nothing_is_stopped_when_the_only_application_is_the_front_one() {
        let seen = [(7, ago(60), IDLE)];
        assert_eq!(coldest(&seen, 7), None);
    }

    #[test]
    fn an_application_still_working_is_passed_over_for_a_newer_idle_one() {
        // The shape this exists for: an audiobook was started, took minutes to
        // write, and the owner read the news while it did. It is the oldest by
        // a long way and the only one with anything to lose, so the idle
        // application touched moments ago goes instead.
        let seen = [(1, ago(600), BUSY), (2, ago(20), IDLE), (3, ago(2), IDLE)];
        assert_eq!(coldest(&seen, 3), Some(1));
    }

    #[test]
    fn work_in_flight_outweighs_any_amount_of_idleness() {
        let seen = [(1, ago(86_400), BUSY), (2, ago(1), IDLE)];
        assert_eq!(coldest(&seen, 9), Some(1));
    }

    #[test]
    fn a_working_application_is_stopped_when_every_candidate_is_working() {
        // Refusing to open anything is worse than stopping something, so when
        // there is no idle candidate the oldest busy one still goes.
        let seen = [(1, ago(30), BUSY), (2, ago(300), BUSY), (3, ago(5), IDLE)];
        assert_eq!(coldest(&seen, 3), Some(1));
    }

    #[test]
    fn platform_and_app_updates_have_separate_builtin_authorities() {
        use kobo_protocol::DeviceRequest;

        let platform = DeviceRequest::Update {
            url: "https://example.test/Cobalt.tgz".to_owned(),
            sha256: "a".repeat(64),
        };
        assert!(super::system_request_allowed("settings", &platform));
        assert!(!super::system_request_allowed("store", &platform));
        assert!(!super::system_request_allowed("todo", &platform));

        for request in [
            DeviceRequest::ReadAutoUpdate,
            DeviceRequest::SetAutoUpdate {
                cobalt: true,
                apps: false,
            },
            DeviceRequest::ReadUpdateChannel,
            DeviceRequest::SetUpdateChannel {
                channel: kobo_protocol::UpdateChannel::Beta,
            },
        ] {
            assert!(super::system_request_allowed("settings", &request));
            assert!(!super::system_request_allowed("store", &request));
            assert!(!super::system_request_allowed("todo", &request));
        }

        let secret = DeviceRequest::SetSecret {
            name: "zotero".to_owned(),
            value: kobo_protocol::SecretValue::new("token"),
        };
        assert!(super::system_request_allowed("zotero-reader", &secret));
        assert!(!super::system_request_allowed("todo", &secret));

        let install = DeviceRequest::InstallApp {
            id: "word-count".to_owned(),
        };
        assert!(super::system_request_allowed("store", &install));
        assert!(!super::system_request_allowed("settings", &install));
        assert!(!super::system_request_allowed("todo", &install));

        assert!(super::system_request_allowed(
            "launcher",
            &DeviceRequest::ListInstalledApps
        ));
        assert!(!super::system_request_allowed(
            "launcher",
            &DeviceRequest::RefreshAppCatalog
        ));
        for request in [
            DeviceRequest::ReadAppLink,
            DeviceRequest::BeginAppLink,
            DeviceRequest::PollAppLink,
            DeviceRequest::DisconnectAppLink,
        ] {
            assert!(super::system_request_allowed("store", &request));
            assert!(!super::system_request_allowed("settings", &request));
            assert!(!super::system_request_allowed("todo", &request));
        }
    }

    #[test]
    fn app_secret_installation_is_scoped_private_and_replaceable() {
        use std::os::unix::fs::PermissionsExt;

        let directory =
            std::env::temp_dir().join(format!("kobo-app-secret-{}", std::process::id()));
        let _ignored = std::fs::remove_dir_all(&directory);
        kobo_policy::credentials::install_app_secret(
            &directory,
            "zotero-reader",
            "zotero",
            "first",
        )
        .expect("install credential");
        assert_eq!(
            std::fs::read(directory.join("apps/zotero-reader/zotero")).expect("read credential"),
            b"first"
        );
        assert_eq!(
            std::fs::metadata(&directory)
                .expect("secret directory")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(directory.join("apps/zotero-reader/zotero"))
                .expect("secret file")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        kobo_policy::credentials::install_app_secret(
            &directory,
            "zotero-reader",
            "zotero",
            "second",
        )
        .expect("replace credential");
        assert_eq!(
            std::fs::read(directory.join("apps/zotero-reader/zotero")).expect("read replacement"),
            b"second"
        );
        assert!(kobo_policy::credentials::install_app_secret(
            &directory,
            "zotero-reader",
            "openai",
            "not-authorized"
        )
        .is_err());
        assert!(!directory.join("apps/zotero-reader/openai").exists());
        let _ignored = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn keyboard_release_gets_one_short_app_response_window() {
        assert_eq!(
            super::release_grace(super::FeedbackKind::Control),
            super::CONTROL_RELEASE_GRACE
        );
        assert_eq!(
            super::release_grace(super::FeedbackKind::KeyboardKey),
            super::KEYBOARD_RELEASE_GRACE
        );
        assert!(super::KEYBOARD_RELEASE_GRACE > super::CONTROL_RELEASE_GRACE);
        assert!(super::KEYBOARD_RELEASE_GRACE < std::time::Duration::from_millis(50));
    }

    #[test]
    fn keyboard_cells_use_the_keyboard_feedback_window() {
        let screen = kobo_ui::Screen::new(
            1,
            vec![kobo_ui::Node::Grid {
                id: kobo_ui::NodeId(1),
                columns: 2,
                square: false,
                cells: vec![
                    kobo_ui::Cell::new(kobo_ui::ActionId(1), "A"),
                    kobo_ui::Cell::new(kobo_ui::ActionId(2), "B"),
                ],
            }],
        );
        let layout = screen.layout();
        let key = layout
            .nodes
            .iter()
            .find(|node| {
                matches!(
                    node.kind,
                    kobo_ui::LayoutKind::Cell(_, kobo_ui::CellStyle::Key)
                )
            })
            .expect("keyboard key")
            .rect;
        assert_eq!(
            super::feedback_kind(&layout, key),
            super::FeedbackKind::KeyboardKey
        );
    }
}

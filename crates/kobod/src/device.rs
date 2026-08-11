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
use kobo_hal::reader::{Reader, Watchdog, WATCHDOG_CHECK};
use kobo_hal::soc_watchdog::SocWatchdog;
use kobo_hal::supervisor::Suspended;
use kobo_hal::touch::TouchEvent;
use kobo_hal::{Rect, RefreshIntent, RefreshPlan, RegionSnapshot};
use kobo_policy::{Backends, Capability, Declared, DeviceServices, PowerPolicy, TaskRunner};
use kobo_profile::{DeviceProfile, CLARA_BW_391};
use kobo_protocol::{Frame, Lifecycle, Message, TaskError, TaskOutcome};
use kobo_ui::{
    display_metrics_from_env, render_all, ActionId, Chrome, FontHandle, FramePlanner,
    PanelWaveform, PictureCache, Screen, Surface,
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

/// The touch panel on every device this supports so far.
pub const TOUCH_DEVICE: &str = "/dev/input/event1";
const COBALT_ROOT: &str = "/mnt/onboard/.adds/cobalt";
/// Where named credentials live.
///
/// On the book partition, because that is the one place the owner can reach
/// over USB without a shell, and because `/tmp` is a RAM disk that every
/// reboot empties. An application names a secret; only the runtime reads one.
const SECRETS: &str = "/mnt/onboard/.adds/cobalt/secrets";

/// Where owner-installed TLS trust roots live, beside the credentials and for
/// the same reasons. A certificate here lets the runtime verify a daemon on
/// the owner's own network exactly as it verifies a public host.
const TRUST: &str = "/mnt/onboard/.adds/cobalt/trust";

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
fn metrics_for(screen: &Screen) -> kobo_ui::DisplayMetrics {
    let mut metrics = display_metrics_from_env();
    let scale = screen.text_scale.unwrap_or(metrics.text_scale);
    metrics.text_scale = scale;
    // The typeface is installed once and lives as long as the process, so the
    // size it sets at has to be told to it rather than carried in the metrics
    // it was built with. Set here, where the screen's own answer is known, so
    // that measuring and drawing this frame cannot disagree.
    kobo_ui::set_text_scale(scale);
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

    // Teardown takes minutes in the worst case (the reader is given forty-five
    // seconds to come back, the network thirty, and the freeze watchdog ninety
    // more) and none of that runs the loop that normally reports progress.
    // Without this the recovery watchdog would conclude the runtime had died
    // and restart a reader that is already starting.
    let teardown = KeepBeating::start(&watchdog);
    // Asked once, before the panel is given up, because the answer decides
    // whether the owner is owed an explanation and the display is gone by the
    // time the reboot itself is requested.
    let rebooting = kobo_hal::bluetooth::requires_reboot_after_use();
    if rebooting {
        if let Err(error) = announce_reboot(&display, whole_screen) {
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
        trace("MediaTek Bluetooth was used; rebooting cleanly instead of restarting the reader");
        println!("Bluetooth changed; rebooting cleanly back to the reader");
        watchdog.disarm();
        drop(teardown);
        let _ignored = fs::remove_dir_all(&state);
        let summary = outcome.unwrap_or_else(|error| format!("application ended: {error}"));
        return request_clean_reboot().map(|()| {
            format!(
                "{summary}; typeface {typeface}; Bluetooth used the shared MediaTek radio, so a clean reboot was requested before returning to the stock reader"
            )
        });
    }
    trace("panel and touch released, restarting the reader");
    println!("panel released, restarting the reader");
    let restarted = reader.start(START_GRACE);
    // The connection is never put back, and there is no longer a way to ask
    // for it. Restoring it meant starting a supplicant and a DHCP client on
    // `wlan0` while the reader we had just restarted drives that same radio
    // itself, from inside libnickel, with no way to be told what we did. Two
    // owners of one radio is the mistake the display is careful to avoid
    // twelve lines above, and it had the same shape here.
    //
    // It existed as a convenience for working on a device over Wi-Fi, where
    // losing the link costs a reboot. It is gone because that convenience was
    // the first link in the chain that erased a device: the reader came up
    // owning a radio it had not configured, never reached its first watchdog
    // ping, and the watchdog was armed against it anyway. The second link is
    // fixed in `resume_once_fed`, which now arms nothing without evidence, but
    // a developer's reboot was never worth being one fault away from a
    // stranger's library.
    trace("reader restart returned, waiting for it to feed the freeze watchdog");
    println!("waiting for the reader to feed the freeze watchdog");
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
fn announce_reboot(display: &DisplaySession, whole_screen: Rect) -> Result<(), String> {
    let screen = Screen::new(
        0,
        vec![kobo_ui::Node::Splash {
            id: kobo_ui::NodeId(1),
            glyph: Some(kobo_ui::Glyph::Bluetooth),
            title: "Restarting your reader".to_owned(),
            summary: "Bluetooth shares one radio with Wi-Fi here, and that radio can only be \
                      started once per boot. Restarting is the only way to hand it back working. \
                      This is expected, it is not a crash, and everything you have saved is \
                      already on the disk."
                .to_owned(),
        }],
    );
    let mut surface = Surface::new(
        usize::try_from(whole_screen.width).unwrap_or(0),
        usize::try_from(whole_screen.height).unwrap_or(0),
    );
    render_all(
        &screen,
        &metrics_for(&screen),
        &Chrome::with_back(false),
        &(),
        &mut surface,
        None,
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
    name: String,
    path: PathBuf,
    /// Root-owned filesystem visible to this application on the device.
    jail: Option<PathBuf>,
    child: Child,
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
    painted: u32,
    /// When this was last on the panel, for deciding what to stop first.
    used: Instant,
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
        kobo_net::fetch_from(url, offset, max_bytes, &[]).map_err(|error| match error {
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

    let result = (|| -> Result<String, String> {
        let front = start_application(&mut apps, &mut next_id, &home, whole_screen, &sender)?;
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
        // The rectangle a finger is resting on, with the metrics its mark was
        // drawn against. Both, because the mark is undone by drawing it again
        // and that is only exact if nothing about it is recomputed.
        let mut pressed: Option<(kobo_ui::Rect, kobo_ui::DisplayMetrics)> = None;
        // When and where the finger landed, for telling a tap from a hold.
        let mut landed: Option<(Instant, i32, i32)> = None;

        loop {
            let now = Instant::now();
            // Reported from the loop rather than from a thread, so this says
            // the runtime is still serving the panel rather than merely that
            // the process has not been reaped.
            watchdog.beat();
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
                            &mut status,
                        )?;
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
                    if let Some(current) = screen.as_ref() {
                        match event {
                            TouchEvent::Down { x, y } => {
                                if let (Ok(x), Ok(y)) = (i32::try_from(x), i32::try_from(y)) {
                                    landed = Some((Instant::now(), x, y));
                                    if let Some(rect) = current
                                        .layout_with(&metrics_for(current), &chrome)
                                        .pressed_control(x, y)
                                    {
                                        let metrics = metrics_for(current);
                                        surface.invert_press(rect, &metrics);
                                        panel.paint(display, whole_screen, &surface)?;
                                        pressed = Some((rect, metrics));
                                    }
                                }
                            }
                            TouchEvent::Up { .. } => {
                                // Put back before the action is delivered, so
                                // that an application which repaints does so
                                // over a control in its resting state rather
                                // than over an inverted one.
                                if let Some((rect, metrics)) = pressed.take() {
                                    surface.invert_press(rect, &metrics);
                                    panel.paint(display, whole_screen, &surface)?;
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
                                        .is_some_and(|(rect, _)| !rect.contains(x, y)),
                                    _ => true,
                                };
                                if off {
                                    if let Some((rect, metrics)) = pressed.take() {
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
                    match deliver_touch(
                        &mut apps[index].stream,
                        event,
                        screen.as_ref(),
                        &chrome,
                        held,
                    )? {
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
                                render_all(
                                    &screen,
                                    &metrics_for(&screen),
                                    &chrome,
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
                        Message::PutFont {
                            handle,
                            name,
                            bytes,
                        } => {
                            let runtime_handle =
                                *apps[index].fonts.entry(handle).or_insert_with(|| {
                                    FontHandle(
                                        NEXT_RUNTIME_FONT.fetch_add(1, AtomicOrdering::Relaxed),
                                    )
                                });
                            match kobo_text::BookFont::from_bytes(
                                &bytes,
                                &name,
                                display_metrics_from_env(),
                            ) {
                                Ok(font) => {
                                    kobo_ui::put_book_typesetter(runtime_handle, Box::new(font));
                                }
                                Err(error) => trace(&format!("font {} refused: {error}", handle.0)),
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
                                        app_store_result(crate::app_store::catalog(Path::new(
                                            COBALT_ROOT,
                                        )))
                                    }
                                    kobo_protocol::DeviceRequest::RefreshAppCatalog => {
                                        app_store_result(crate::app_store::refresh(Path::new(
                                            COBALT_ROOT,
                                        )))
                                    }
                                    kobo_protocol::DeviceRequest::InstallApp { id } => {
                                        let result =
                                            crate::app_store::install(Path::new(COBALT_ROOT), id);
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
    render_all(
        &screen,
        &metrics_for(&screen),
        &chrome,
        &apps[index].pictures,
        surface,
        None,
    );
    panel.paint(display, whole_screen, surface)?;
    apps[index].painted += 1;
    Ok(())
}

/// Keeps platform replacement and app installation behind distinct built-in
/// identities even while both travel over the same bounded device channel.
fn system_request_allowed(app: &str, request: &kobo_protocol::DeviceRequest) -> bool {
    match request {
        kobo_protocol::DeviceRequest::Update { .. } => app == "settings",
        kobo_protocol::DeviceRequest::ListInstalledApps => matches!(app, "launcher" | "store"),
        kobo_protocol::DeviceRequest::ReadAppCatalog
        | kobo_protocol::DeviceRequest::RefreshAppCatalog
        | kobo_protocol::DeviceRequest::InstallApp { .. }
        | kobo_protocol::DeviceRequest::UninstallApp { .. } => app == "store",
        _ => true,
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
        .stderr(Stdio::null());
    if let Some(sandbox) = sandbox {
        sandbox.configure(&mut command);
    } else {
        kobo_abi::process_group::configure(&mut command);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let _ignored = fs::remove_file(&socket_path);
            if let Some(root) = &jail {
                let _ignored = fs::remove_dir_all(root);
            }
            return Err(format!("start {}: {error}", path.display()));
        }
    };
    let greeting = greet(&listener, whole_screen, &expected_name);
    drop(listener);
    let _ignored = fs::remove_file(&socket_path);
    let (stream, name) = match greeting {
        Ok(greeting) => greeting,
        Err(error) => {
            stop_application(&mut child, jail.as_deref());
            if let Some(root) = &jail {
                let _ignored = fs::remove_dir_all(root);
            }
            return Err(error);
        }
    };
    let id = *next_id;
    *next_id += 1;
    if let Err(error) = pump_application(&stream, sender, id) {
        stop_application(&mut child, jail.as_deref());
        if let Some(root) = &jail {
            let _ignored = fs::remove_dir_all(root);
        }
        return Err(error);
    }
    let waker = sender.clone();
    let credential_app = name.clone();
    let declared = if path.starts_with(Path::new(COBALT_ROOT).join("apps")) {
        crate::app_store::declared(Path::new(COBALT_ROOT), &name)
            .ok_or_else(|| format!("read application manifest for {name}"))?
    } else if let Some(declared) = crate::app_store::builtin_declared(&name) {
        declared
    } else {
        Declared::all()
    };
    let tasks = TaskRunner::simulated(std::env::temp_dir())
        .with_fetch(Arc::new(kobo_net::fetch_from))
        .with_post(Arc::new(kobo_net::post))
        .with_secrets(SECRETS)
        .with_credential_policy(Arc::new(move |credential, url| {
            kobo_net::credential_allowed(&credential_app, credential, url)
        }))
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
        path: path.to_path_buf(),
        jail,
        child,
        stream,
        tasks,
        declared,
        screen: None,
        pictures: PictureCache::default(),
        fonts: BTreeMap::new(),
        painted: 0,
        used: Instant::now(),
    });
    Ok(id)
}

/// Ends one hosted application and everything it started.
fn stop_hosted(mut app: Hosted) {
    for (_, handle) in std::mem::take(&mut app.fonts) {
        kobo_ui::drop_book_typesetter(handle);
    }
    app.tasks.shutdown();
    stop_application(&mut app.child, app.jail.as_deref());
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
    let path = catalogue.join(format!("kobo-{name}"));
    if path.is_file() {
        Ok(path)
    } else {
        crate::app_store::resolve(Path::new(COBALT_ROOT), name)
            .map_err(|_| format!("no application named {name} is installed"))
    }
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
const HOLD_TIME: Duration = Duration::from_millis(500);

/// How far the finger may wander and still be holding, in pixels.
///
/// A finger resting on glass is never still, and this panel reports every
/// tremor. Roughly three millimetres on a Clara, which is under the width of
/// the contact patch, so a hand that is not moving cannot cross it.
const HOLD_SLIP: i32 = 40;

/// Ends the application, politely if it has already finished and firmly if not.
fn stop_application(child: &mut Child, jail: Option<&Path>) {
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

/// Resolves a touch to the action it activates, if any.
///
/// Activation happens on release rather than on contact, so a finger that lands
/// on the wrong control can be slid away from it without acting. That is what
/// every touch interface the owner already uses has taught them to expect.
fn action_for(
    event: TouchEvent,
    screen: Option<&Screen>,
    chrome: &Chrome,
    held: bool,
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
    let layout = screen.layout_with(&metrics_for(screen), chrome);
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

fn text_hold_for(
    event: TouchEvent,
    screen: Option<&Screen>,
    chrome: &Chrome,
    held: bool,
) -> Option<(ActionId, kobo_ui::TextHit)> {
    if !held {
        return None;
    }
    let TouchEvent::Up { x, y } = event else {
        return None;
    };
    let screen = screen?;
    let action = screen.hold?;
    let (Ok(x), Ok(y)) = (i32::try_from(x), i32::try_from(y)) else {
        return None;
    };
    let layout = screen.layout_with(&metrics_for(screen), chrome);
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
) -> Result<(std::os::unix::net::UnixStream, String), String> {
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
/// refusal on it (see [`Screen::owns_back`]) so that a screen reached from
/// inside an application goes back to where it was reached from rather than
/// out of the application. That is a delivery, not a transfer of ownership:
/// the caller still leaves if no new screen follows.
fn deliver_touch(
    stream: &mut std::os::unix::net::UnixStream,
    event: TouchEvent,
    current: Option<&Screen>,
    chrome: &Chrome,
    held: bool,
) -> Result<Tap, String> {
    if let Some((action, hit)) = text_hold_for(event, current, chrome, held) {
        kobo_protocol::write_to(
            stream,
            &Frame {
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
    let Some(action) = action_for(event, current, chrome, held) else {
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
    }

    #[test]
    fn back_is_reported_from_the_chrome_the_frame_was_drawn_with() {
        let screen = hello();
        let chrome = Chrome::with_back(true);
        let back = screen
            .layout_with(&display_metrics_from_env(), &chrome)
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
            .layout_with(&display_metrics_from_env(), &chrome)
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
            deliver_touch(&mut runtime, tap, Some(&screen), &chrome, false).expect("route the tap"),
            Tap::Leave,
            "a screen that did not ask keeps the old behaviour"
        );

        let owning = screen.clone().with_own_back(true);
        assert_eq!(
            deliver_touch(&mut runtime, tap, Some(&owning), &chrome, false).expect("route the tap"),
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
    }
}

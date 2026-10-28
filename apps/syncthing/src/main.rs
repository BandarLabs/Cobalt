mod supervisor;

use kobo_sdk::clock::{Clock, Snapshot, SystemClock};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Glyph, Heartbeat, KoboApp, Screen, ScreenBuilder,
    StoreResult, TaskId, TaskOutcome,
};
use std::process::ExitCode;
use std::time::Duration;
use supervisor::{Cadence, Config, FOLDERS};

const CONFIG: &str = "sync-config";
const STATUS: &str = "sync-status";
const INGEST: &str = "sync-ingest";
const META: &str = "sync-meta";
/// Saved to ask the runtime for a sync now. The runtime removes it as the sync
/// starts, which is how this app learns that it did.
const REQUEST: &str = "sync-request";
/// When the runtime last started a sync. The schedule counts from here, so
/// this is what "Next sync" is worked out from.
const LAST_ATTEMPT: &str = "sync-last-attempt";

/// How long a sync that was asked for may take to start before the screen
/// says it has not. The runtime answers a tap within a second or two; half a
/// minute of nothing means a runtime too old to start syncs at all.
const START_WAIT: Duration = Duration::from_secs(30);
/// How long the screen follows a sync. A sync stops itself after five
/// minutes, so a status still saying "running" after six is a stale file
/// rather than a sync, and polling it further would only cost battery.
const FOLLOW_LIMIT: Duration = Duration::from_secs(6 * 60);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum View {
    Status,
    Guide,
    Folders,
    About,
}

/// The supervisor's status, parsed line by line. Lines past the third come
/// from a newer supervisor; an older one leaves them unknown rather than
/// wrong.
#[derive(Clone, Debug, Default)]
struct WindowStatus {
    state: String,
    bytes: u64,
    message: String,
    peers: Option<u64>,
    last_success: u64,
    conflicts: Option<u64>,
}

impl WindowStatus {
    fn parse(text: &str) -> Self {
        let mut lines = text.lines();
        let state = lines.next().unwrap_or_default().to_owned();
        let bytes = lines
            .next()
            .and_then(|value| value.trim().parse().ok())
            .unwrap_or(0);
        let message = lines.next().unwrap_or_default().to_owned();
        let peers = lines.next().and_then(|value| value.trim().parse().ok());
        let last_success = lines
            .next()
            .and_then(|value| value.trim().parse().ok())
            .unwrap_or(0);
        let conflicts = lines.next().and_then(|value| value.trim().parse().ok());
        Self {
            state,
            bytes,
            message,
            peers,
            last_success,
            conflicts,
        }
    }

    /// Whether the runtime said something went wrong that the owner can act
    /// on, so its own words belong at the top of the screen.
    fn needs_the_owner(&self) -> bool {
        matches!(
            self.state.as_str(),
            "failed" | "timed-out" | "unpaired" | "offline" | "conflict"
        ) && !self.message.is_empty()
    }
}

/// Where a sync the owner asked for has got to before the runtime starts it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Asked {
    #[default]
    No,
    /// Saved, and the runtime has not started it yet.
    Waiting,
    /// Saved, and nothing started it within [`START_WAIT`].
    Stalled,
}

/// One line of the supervisor's import report: folder, files, bytes, when.
#[derive(Clone, Debug)]
struct Import {
    folder: String,
    files: u64,
    epoch: u64,
}

struct Sync {
    config: Config,
    view: View,
    status: WindowStatus,
    imports: Vec<Import>,
    last_attempt: u64,
    /// A sync the owner asked for that the runtime has not started yet.
    asked: Asked,
    /// Ticks while a sync is asked for or running, so the screen follows it
    /// without a refresh button.
    watch: Heartbeat,
    first_sync_seen: bool,
    first_sync_banner: bool,
}

impl Default for Sync {
    fn default() -> Self {
        Self {
            config: Config::default(),
            view: View::Status,
            status: WindowStatus::default(),
            imports: Vec::new(),
            last_attempt: 0,
            asked: Asked::No,
            watch: Heartbeat::every(5),
            first_sync_seen: false,
            first_sync_banner: false,
        }
    }
}

impl Sync {
    fn save(&self, context: &mut Context) {
        context.store().save(
            CONFIG,
            format!("{}\n{}", self.config.enabled, self.config.cadence as u8),
        );
        // The first line once held when this app last scheduled a wake. It is
        // kept, empty, so an older copy of the app still finds the second.
        context
            .store()
            .save(META, format!("0\n{}", u8::from(self.first_sync_seen)));
    }

    /// Asks the runtime to bring the app back when a scheduled sync is due,
    /// so its status is fresh when the reader next looks.
    ///
    /// The sync itself does not depend on this. The runtime starts it from the
    /// saved schedule whether or not this app is running.
    fn apply_schedule(&self, context: &mut Context) {
        if let (true, Some(seconds)) = (self.config.enabled, self.config.cadence.seconds()) {
            context
                .device()
                .schedule_wake(Duration::from_secs(u64::from(seconds)));
        } else {
            context.device().cancel_wake();
        }
    }

    fn reload(context: &mut Context) {
        for key in [STATUS, INGEST, REQUEST, LAST_ATTEMPT] {
            context.store().load(key);
        }
    }

    /// Saves the request the runtime answers by starting a sync.
    fn ask_for_a_sync(&mut self, context: &mut Context) {
        if self.status.state == "running" {
            return;
        }
        self.asked = Asked::Waiting;
        context
            .store()
            .save(REQUEST, now_seconds().to_string().into_bytes());
        self.follow(context);
    }

    /// Starts following a sync that is asked for or running, and stops when
    /// there is nothing left to follow.
    fn follow(&mut self, context: &mut Context) {
        let busy = self.asked == Asked::Waiting || self.running();
        if busy && self.status.state != "offline" {
            self.watch.start(context);
        } else {
            self.watch.stop(context);
        }
    }

    fn running(&self) -> bool {
        self.status.state == "running"
    }

    fn show(&self, context: &mut Context) {
        let screen = match self.view {
            View::Status => self.status_screen(),
            View::Guide => guide_screen(),
            View::Folders => folders_screen(),
            View::About => about_screen(),
        };
        // Supporting pages must receive the runtime Back event before it can
        // leave the app; the status screen remains the root exit.
        context.set_screen(screen.with_own_back(self.view != View::Status));
    }

    fn state_words(&self) -> String {
        if self.asked != Asked::No && !self.running() && self.status.state != "offline" {
            return if self.asked == Asked::Stalled {
                "Not started".to_owned()
            } else {
                "Starting".to_owned()
            };
        }
        transfer_state(&self.status)
    }

    fn status_facts(&self) -> Vec<(String, String)> {
        let mut facts = vec![("Status".to_owned(), self.state_words())];
        if self.running() {
            facts.push(("Left to move".to_owned(), format_bytes(self.status.bytes)));
        }
        facts.push((
            "Last sync".to_owned(),
            format_epoch(self.status.last_success),
        ));
        facts.push(("Next sync".to_owned(), self.next_sync()));
        if let Some(peers) = self.status.peers {
            facts.push(("Computers found".to_owned(), peers.to_string()));
        }
        if let Some(conflicts) = self.status.conflicts.filter(|count| *count > 0) {
            facts.push(("Files not synced".to_owned(), conflicts.to_string()));
        }
        for import in &self.imports {
            facts.push((
                format!("Added to {}", folder_label(&import.folder)),
                format!("{} files, {}", import.files, format_epoch(import.epoch)),
            ));
        }
        facts
    }

    fn banner(&self) -> Option<(BannerLevel, String)> {
        if !self.config.enabled {
            return Some((
                BannerLevel::Info,
                "Sync is off. Nothing moves until you turn it on.".to_owned(),
            ));
        }
        if self.first_sync_banner {
            return Some((
                BannerLevel::Info,
                "First sync finished. Notes and photos now arrive after each sync.".to_owned(),
            ));
        }
        if self.asked == Asked::Stalled {
            return Some((
                BannerLevel::Attention,
                "Sync has not started. Check Settings for a Cobalt update, then tap Sync now again."
                    .to_owned(),
            ));
        }
        if self.status.needs_the_owner() {
            return Some((BannerLevel::Attention, self.status.message.clone()));
        }
        if self.status.last_success == 0 && !self.running() && self.asked == Asked::No {
            return Some((
                BannerLevel::Info,
                "No sync has finished yet. Set up a computer, then tap Sync now.".to_owned(),
            ));
        }
        None
    }

    fn status_screen(&self) -> Screen {
        let facts = self.status_facts();
        let mut rows = Vec::with_capacity(4);
        if self.config.enabled {
            rows.push((
                "sync-now",
                "Sync now".to_owned(),
                if self.running() {
                    "Syncing. This takes up to five minutes.".to_owned()
                } else if self.asked == Asked::Waiting {
                    "Starting a sync.".to_owned()
                } else {
                    "Send and receive files now.".to_owned()
                },
                Glyph::Refresh,
            ));
        } else {
            rows.push((
                "toggle",
                "Turn on Sync".to_owned(),
                "Syncs once now, then on your schedule.".to_owned(),
                Glyph::Refresh,
            ));
        }
        rows.push((
            "cadence",
            self.config.cadence.label().to_owned(),
            match self.config.cadence.seconds() {
                Some(_) => "Syncs while Cobalt is open on Wi-Fi. Tap to change.".to_owned(),
                None => "Syncs only when you tap Sync now. Tap to change.".to_owned(),
            },
            Glyph::Clock,
        ));
        // Before the first sync the pairing guide earns the slot; afterwards
        // the folder list is the more useful destination.
        rows.push(if self.status.last_success == 0 {
            (
                "setup",
                "Set up Sync".to_owned(),
                "Pair a computer folder, step by step.".to_owned(),
                Glyph::Folder,
            )
        } else {
            (
                "folders",
                "Folders".to_owned(),
                "Vault, Frame, books and out.".to_owned(),
                Glyph::Folder,
            )
        });
        if self.config.enabled {
            rows.push((
                "toggle",
                "Turn off Sync".to_owned(),
                "Stops all syncing. Your files stay.".to_owned(),
                Glyph::Settings,
            ));
        }
        // The banner leads the screen: appended last it is the first node the
        // stack drops when a larger text size runs out of panel, which hid
        // "Sync is off." at exactly the sizes where the warning matters.
        let mut screen = ScreenBuilder::new("syncthing").top_bar("Sync");
        if let Some((level, text)) = self.banner() {
            screen = screen.banner(level, text);
        }
        screen
            .rows(rows)
            .facts(
                facts
                    .iter()
                    .map(|(key, value)| (key.as_str(), value.as_str())),
            )
            .build()
    }

    fn next_sync(&self) -> String {
        if !self.config.enabled {
            return "Off".to_owned();
        }
        let Some(seconds) = self.config.cadence.seconds() else {
            return "When you tap Sync now".to_owned();
        };
        let due = self.last_attempt.saturating_add(u64::from(seconds));
        if self.last_attempt == 0 || due <= now_seconds() {
            return "When on Wi-Fi".to_owned();
        }
        format_epoch(due)
    }

    fn load_status(&mut self, context: &mut Context, value: Option<&[u8]>) {
        self.status = value
            .map(|bytes| WindowStatus::parse(&String::from_utf8_lossy(bytes)))
            .unwrap_or_default();
        if self.status.last_success > 0 && !self.first_sync_seen {
            self.first_sync_seen = true;
            self.first_sync_banner = true;
            self.save(context);
        }
        self.follow(context);
    }
}

fn transfer_state(status: &WindowStatus) -> String {
    match status.state.as_str() {
        "idle" => "Up to date",
        "running" => "Syncing",
        "conflict" => "Some files did not sync",
        "paused" | "disabled" => "Off",
        "waiting" => "Waiting for Sync now",
        "timed-out" => "Did not finish",
        "failed" => "Could not start",
        "stopped" => "Stopped",
        "unpaired" => "No computer set up",
        "offline" => "Waiting for Wi-Fi",
        "" => "Not run yet",
        _ => "Unknown",
    }
    .to_owned()
}

fn folder_label(folder: &str) -> &'static str {
    match folder {
        "vault" => "Vault",
        "frame" => "Frame",
        _ => "folder",
    }
}

fn format_bytes(bytes: u64) -> String {
    // One decimal from integer arithmetic: whole units plus the first
    // fractional digit, so no float ever touches a byte count.
    if bytes >= 1024 * 1024 {
        format!(
            "{}.{} MB",
            bytes / (1024 * 1024),
            bytes % (1024 * 1024) * 10 / (1024 * 1024)
        )
    } else if bytes >= 1024 {
        format!("{}.{} KB", bytes / 1024, bytes % 1024 * 10 / 1024)
    } else {
        format!("{bytes} B")
    }
}

fn format_epoch(epoch: u64) -> String {
    if epoch == 0 {
        return "Never".to_owned();
    }
    let Some(snapshot) = snapshot_at(epoch) else {
        return "Unknown".to_owned();
    };
    let Some(date) = snapshot.date() else {
        return "Unknown".to_owned();
    };
    let day = date.day;
    let month = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ][usize::from(date.month.saturating_sub(1)) % 12];
    match snapshot.hour_minute() {
        Some((hour, minute)) => format!("{month} {day}, {hour:02}:{minute:02}"),
        None => format!("{month} {day}"),
    }
}

/// A moment on the reader's own clock, at the offset the runtime passed.
fn snapshot_at(epoch: u64) -> Option<Snapshot> {
    let snapshot = Snapshot {
        unix_millis: epoch.checked_mul(1000)?,
        monotonic_millis: 0,
        utc_offset_minutes: kobo_sdk::clock::reader_offset_minutes(),
    };
    snapshot.valid().then_some(snapshot)
}

fn now_seconds() -> u64 {
    SystemClock::reader()
        .now()
        .map_or(0, |snapshot| snapshot.unix_millis / 1000)
}

fn guide_screen() -> Screen {
    ScreenBuilder::new("syncthing")
        .top_bar("Set up Sync")
        .section("Pair one computer folder")
        .secondary("1. Install Syncthing on the computer with its package manager.")
        .secondary("2. With the reader awake on Wi-Fi, run this on the computer:")
        .text("kobo sync setup ~/Documents/notes --folder vault")
        .secondary("3. Run kobo sync run on the computer, then tap Turn on Sync here. The first sync starts at once.")
        .secondary("Files come to the reader in vault, frame and books, and changes made here never go back. Files in out go to the computer.")
        .build()
}

fn folders_screen() -> Screen {
    ScreenBuilder::new("syncthing")
        .top_bar("Sync folders")
        .top_bar_action("about", "About")
        .rows(
            FOLDERS
                .into_iter()
                .map(|(path, direction)| (path, path, direction, Glyph::Folder)),
        )
        .secondary("These four folders are fixed. Notes open in Vault and photos in Frame after each sync. Books stay as files.")
        .build()
}

fn about_screen() -> Screen {
    ScreenBuilder::new("syncthing")
        .top_bar("About Sync")
        .heading("Syncthing")
        .text("Sync uses Syncthing to move files between your computer and this reader. Syncthing is available under the MPL-2.0 license.")
        .secondary("The reader downloads the Syncthing engine the first time a computer is set up. Set one up with kobo sync setup.")
        .build()
}

impl KoboApp for Sync {
    fn on_start(&mut self, context: &mut Context) {
        context.store().load(CONFIG);
        context.store().load(META);
        Self::reload(context);
        self.show(context);
    }

    fn on_foreground(&mut self, context: &mut Context) {
        Self::reload(context);
    }

    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        let StoreResult::Loaded { key, value } = result else {
            return;
        };
        match key.as_str() {
            CONFIG => {
                if let Some(value) = value {
                    let saved = String::from_utf8_lossy(&value);
                    let mut fields = saved.lines();
                    self.config.enabled = fields.next() == Some("true");
                    self.config.cadence = match fields.next() {
                        Some("1") => Cadence::Hourly,
                        Some("2") => Cadence::FourHourly,
                        Some("3") => Cadence::Daily,
                        _ => Cadence::Manual,
                    };
                }
                self.apply_schedule(context);
            }
            META => {
                if let Some(value) = value {
                    self.first_sync_seen =
                        String::from_utf8_lossy(&value).lines().nth(1) == Some("1");
                }
            }
            STATUS => self.load_status(context, value.as_deref()),
            REQUEST => {
                // A request still saved from before a restart is followed
                // like a fresh one; a stalled one stays stalled.
                if value.is_none() {
                    self.asked = Asked::No;
                } else if self.asked == Asked::No {
                    self.asked = Asked::Waiting;
                }
                self.follow(context);
            }
            LAST_ATTEMPT => {
                self.last_attempt = value
                    .and_then(|bytes| String::from_utf8_lossy(&bytes).trim().parse().ok())
                    .unwrap_or(0);
            }
            INGEST => {
                self.imports = value
                    .as_deref()
                    .map(|bytes| {
                        String::from_utf8_lossy(bytes)
                            .lines()
                            .filter_map(|line| {
                                let mut fields = line.split('\t');
                                Some(Import {
                                    folder: fields.next()?.to_owned(),
                                    files: fields.next()?.parse().ok()?,
                                    epoch: fields.nth(1)?.parse().ok()?,
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
            }
            _ => return,
        }
        self.show(context);
    }

    fn on_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
        if !self.watch.on_task(context, task, &outcome) {
            return;
        }
        if self.asked == Asked::Waiting && !self.running() && self.watch.waited() >= START_WAIT {
            self.asked = Asked::Stalled;
            self.watch.stop(context);
            self.show(context);
            return;
        }
        if self.watch.waited() >= FOLLOW_LIMIT {
            self.watch.stop(context);
        }
        Self::reload(context);
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        self.first_sync_banner = false;
        if action == action_id("toggle") {
            self.config.enabled = !self.config.enabled;
            self.apply_schedule(context);
            self.save(context);
            // Turning Sync on is nearly always the last step of setting it
            // up, with the computer's side already running, so it syncs now
            // rather than leaving the owner to find a second button.
            if self.config.enabled {
                self.ask_for_a_sync(context);
            } else {
                self.asked = Asked::No;
                self.follow(context);
            }
        } else if action == action_id("sync-now") {
            self.ask_for_a_sync(context);
        } else if action == action_id("cadence") {
            self.config.cadence = self.config.cadence.next();
            self.apply_schedule(context);
            self.save(context);
        } else if action == action_id("folders") {
            self.view = View::Folders;
        } else if action == action_id("about") {
            self.view = View::About;
        } else if action == action_id("setup") {
            self.view = View::Guide;
        } else if action == action_id("back") || action == ActionId::BACK {
            self.view = View::Status;
        }
        self.show(context);
    }

    fn on_scheduled_wake(&mut self, context: &mut Context) {
        // The runtime starts scheduled syncs itself, from the saved schedule;
        // this wake only brings the status up to date for the next look.
        Self::reload(context);
        self.apply_schedule(context);
    }
}

fn main() -> ExitCode {
    kobo_sdk::run("syncthing", Sync::default()).map_or_else(
        |e| {
            eprintln!("syncthing: {e}");
            ExitCode::FAILURE
        },
        |()| ExitCode::SUCCESS,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobo_sdk::{Command, DeviceRequest, StoreRequest};
    use kobo_ui::{Chrome, CLARA_BW_METRICS};

    #[test]
    fn primary_sync_control_fits_clara_bw() {
        let app = Sync::default();
        let layout = app
            .status_screen()
            .layout_with(&CLARA_BW_METRICS, &Chrome::default());
        let toggle = layout
            .rect_of_action(action_id("toggle"))
            .expect("sync control");
        assert!(toggle.height >= CLARA_BW_METRICS.touch_target_minimum());
        assert_eq!(app.config.cadence, Cadence::Manual);
    }

    #[test]
    fn enabled_hourly_sync_requests_a_bounded_wake() {
        let app = Sync {
            config: Config {
                enabled: true,
                cadence: Cadence::Hourly,
            },
            ..Sync::default()
        };
        let mut context = Context::default();
        app.apply_schedule(&mut context);
        assert!(context.commands().iter().any(|command| {
            matches!(
                command,
                Command::Device(DeviceRequest::ScheduleWake { seconds: 3600 })
            )
        }));
    }

    fn saved_keys(commands: &[Command]) -> Vec<String> {
        commands
            .iter()
            .filter_map(|command| match command {
                Command::Store(StoreRequest::Save { key, .. }) => Some(key.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn sync_now_asks_the_runtime_and_turning_sync_on_syncs_at_once() {
        let mut app = Sync::default();
        let mut context = Context::default();
        app.on_action(&mut context, action_id("toggle"));
        let saved = saved_keys(context.commands());
        assert!(saved.contains(&CONFIG.to_owned()), "{saved:?}");
        assert!(saved.contains(&REQUEST.to_owned()), "{saved:?}");
        assert!(app.asked == Asked::Waiting && app.watch.is_running());

        // The runtime removing the request is how the app learns it started.
        let mut context = Context::default();
        app.on_store(
            &mut context,
            StoreResult::Loaded {
                key: STATUS.into(),
                value: Some(b"running\n2048\nSyncing folders.\n1\n0\n0".to_vec()),
            },
        );
        app.on_store(
            &mut context,
            StoreResult::Loaded {
                key: REQUEST.into(),
                value: None,
            },
        );
        assert!(
            app.asked == Asked::No && app.watch.is_running(),
            "a running sync is followed"
        );
        let mut context = Context::default();
        app.on_action(&mut context, action_id("sync-now"));
        assert!(
            saved_keys(context.commands()).is_empty(),
            "a second tap while syncing asks for nothing"
        );
        app.on_store(
            &mut context,
            StoreResult::Loaded {
                key: STATUS.into(),
                value: Some(b"idle\n0\nSync finished.\n1\n1790841600\n0".to_vec()),
            },
        );
        assert!(
            !app.watch.is_running(),
            "a finished sync is no longer polled"
        );
        assert!(app.first_sync_banner);
    }

    #[test]
    fn nothing_on_the_status_screen_uses_syncthing_jargon_or_pretends_to_have_run() {
        let fresh = Sync {
            config: Config {
                enabled: true,
                cadence: Cadence::Hourly,
            },
            ..Sync::default()
        };
        let text = format!("{:?}", fresh.status_screen());
        for jargon in [
            "window",
            "Window",
            "cadence",
            "receive-only",
            "Receive only",
            "Peers",
        ] {
            assert!(!text.contains(jargon), "{jargon} in {text}");
        }
        assert_eq!(fresh.state_words(), "Not run yet");
        assert_eq!(fresh.next_sync(), "When on Wi-Fi");
        assert!(fresh
            .banner()
            .is_some_and(|(_, words)| words.starts_with("No sync has finished yet")));
        for view in [guide_screen(), folders_screen(), about_screen()] {
            let text = format!("{view:?}");
            for jargon in ["window", "cadence", "receive-only", "Receive only"] {
                assert!(!text.contains(jargon), "{jargon} in {text}");
            }
        }
    }

    #[test]
    fn the_runtimes_reason_is_shown_when_a_sync_could_not_finish() {
        let app = Sync {
            config: Config {
                enabled: true,
                cadence: Cadence::Manual,
            },
            status: WindowStatus::parse(
                "timed-out\n0\nThe computer was not found. Run kobo sync run on it, keep both on the same network, then tap Sync now.\n0\n0\n0",
            ),
            ..Sync::default()
        };
        assert_eq!(app.state_words(), "Did not finish");
        let (level, words) = app.banner().expect("a banner");
        assert_eq!(level, BannerLevel::Attention);
        assert!(words.starts_with("The computer was not found"));
    }

    #[test]
    fn status_parses_old_and_new_supervisor_lines() {
        let old = WindowStatus::parse("idle\n0\nLast sync: complete.");
        assert_eq!(old.state, "idle");
        assert_eq!(old.peers, None);
        assert_eq!(old.last_success, 0);
        let new = WindowStatus::parse("conflict\n0\n2 file(s) could not sync.\n1\n1758000000\n2");
        assert_eq!(new.peers, Some(1));
        assert_eq!(new.last_success, 1_758_000_000);
        assert_eq!(new.conflicts, Some(2));
    }

    #[test]
    fn bytes_format_for_humans() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(2048), "2.0 KB");
        assert_eq!(format_bytes(5 * 1024 * 1024), "5.0 MB");
    }
}

#[cfg(test)]
mod ui_review_tests;

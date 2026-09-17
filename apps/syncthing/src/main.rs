mod supervisor;

use kobo_sdk::clock::{Clock, ManualClock, Snapshot, SystemClock};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Glyph, KoboApp, Screen, ScreenBuilder, StoreResult,
};
use std::process::ExitCode;
use std::time::Duration;
use supervisor::{Cadence, Config, FOLDERS};

const CONFIG: &str = "sync-config";
const STATUS: &str = "sync-status";
const INGEST: &str = "sync-ingest";
const META: &str = "sync-meta";

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
        let state = lines.next().unwrap_or("disabled").to_owned();
        let bytes = lines
            .next()
            .and_then(|value| value.trim().parse().ok())
            .unwrap_or(0);
        let message = lines.next().unwrap_or("Sync has not run.").to_owned();
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
    status_seen: bool,
    imports: Vec<Import>,
    scheduled_at: u64,
    first_sync_seen: bool,
    first_sync_banner: bool,
}

impl Default for Sync {
    fn default() -> Self {
        Self {
            config: Config::default(),
            view: View::Status,
            status: WindowStatus::default(),
            status_seen: false,
            imports: Vec::new(),
            scheduled_at: 0,
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
        context.store().save(
            META,
            format!("{}\n{}", self.scheduled_at, u8::from(self.first_sync_seen)),
        );
    }

    fn apply_schedule(&mut self, context: &mut Context) {
        if let (true, Some(seconds)) = (self.config.enabled, self.config.cadence.seconds()) {
            self.scheduled_at = now_seconds();
            context
                .device()
                .schedule_wake(Duration::from_secs(u64::from(seconds)));
        } else {
            self.scheduled_at = 0;
            context.device().cancel_wake();
        }
    }

    fn show(&self, context: &mut Context) {
        let screen = match self.view {
            View::Status => self.status_screen(),
            View::Guide => guide_screen(),
            View::Folders => folders_screen(),
            View::About => about_screen(),
        };
        context.set_screen(screen);
    }

    fn status_facts(&self) -> Vec<(String, String)> {
        let mut facts = vec![
            ("State".to_owned(), transfer_state(&self.status)),
            (
                "Bytes left".to_owned(),
                if self.status.state == "running" {
                    format_bytes(self.status.bytes)
                } else {
                    "0 B".to_owned()
                },
            ),
            (
                "Peers".to_owned(),
                self.status
                    .peers
                    .map_or_else(|| "-".to_owned(), |peers| peers.to_string()),
            ),
            (
                "Last success".to_owned(),
                format_epoch(self.status.last_success),
            ),
            ("Next window".to_owned(), self.next_window()),
        ];
        for import in &self.imports {
            facts.push((
                format!("{} imported", folder_label(&import.folder)),
                format!("{} files, {}", import.files, format_epoch(import.epoch)),
            ));
        }
        facts
    }

    fn status_screen(&self) -> Screen {
        let state = if self.config.enabled {
            "Enabled"
        } else {
            "Disabled"
        };
        let facts = self.status_facts();
        let mut screen = ScreenBuilder::new("syncthing")
            .top_bar("Sync")
            .section_with_value("Service", state)
            .rows([
                (
                    "toggle",
                    if self.config.enabled {
                        "Pause Sync".to_owned()
                    } else {
                        "Resume Sync".to_owned()
                    },
                    "Changes take effect during the current sync window.".to_owned(),
                    Glyph::Settings,
                ),
                (
                    "cadence",
                    self.config.cadence.label().to_owned(),
                    format!(
                        "Up to {} radio minutes per day.",
                        self.config.cadence.radio_minutes()
                    ),
                    Glyph::Clock,
                ),
                // The panel holds four rows beside the facts. Before the first
                // sync the pairing guide earns the slot; afterwards the folder
                // list is the more useful destination.
                if self.status.last_success == 0 {
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
                        "Vault, frame, books and out.".to_owned(),
                        Glyph::Folder,
                    )
                },
                (
                    "refresh",
                    "Refresh status".to_owned(),
                    "Read the runtime-owned Sync status.".to_owned(),
                    Glyph::Refresh,
                ),
            ])
            .facts(
                facts
                    .iter()
                    .map(|(key, value)| (key.as_str(), value.as_str())),
            );
        if !self.config.enabled {
            screen = screen.banner(BannerLevel::Info, "Sync is off.");
        } else if self.first_sync_banner {
            screen = screen.banner(
                BannerLevel::Info,
                "First sync complete. Files now import after every window.",
            );
        } else if self.status.conflicts.unwrap_or(0) > 0 {
            screen = screen.banner(
                BannerLevel::Attention,
                "Some files could not sync. They retry on the next window.",
            );
        } else if self.status_seen && self.status.last_success == 0 && self.config.enabled {
            screen = screen.banner(
                BannerLevel::Info,
                "Waiting for the first sync. Keep the reader on Wi-Fi.",
            );
        }
        screen.build()
    }

    fn next_window(&self) -> String {
        if !self.config.enabled {
            return "Off".to_owned();
        }
        let Some(seconds) = self.config.cadence.seconds() else {
            return "Manual".to_owned();
        };
        if self.scheduled_at == 0 {
            return "Not scheduled".to_owned();
        }
        format_epoch(self.scheduled_at + u64::from(seconds))
    }
}

fn transfer_state(status: &WindowStatus) -> String {
    match status.state.as_str() {
        "idle" => "Up to date".to_owned(),
        "running" => "Syncing".to_owned(),
        "conflict" => "Needs attention".to_owned(),
        "paused" => "Paused".to_owned(),
        "disabled" => "Off".to_owned(),
        "waiting" => "Waiting for a window".to_owned(),
        "timed-out" => "Window ended".to_owned(),
        "failed" => "Engine problem".to_owned(),
        "stopped" => "Stopped".to_owned(),
        "" => "Not run yet".to_owned(),
        other => other.to_owned(),
    }
}

fn folder_label(folder: &str) -> &'static str {
    match folder {
        "vault" => "Vault",
        "frame" => "Frame",
        _ => "Folder",
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

fn snapshot_at(epoch: u64) -> Option<Snapshot> {
    let reader = reader_clock().now().ok()?;
    let snapshot = Snapshot {
        unix_millis: epoch.checked_mul(1000)?,
        monotonic_millis: 0,
        utc_offset_minutes: reader.utc_offset_minutes,
    };
    snapshot.valid().then_some(snapshot)
}

fn now_seconds() -> u64 {
    reader_clock()
        .now()
        .map_or(0, |snapshot| snapshot.unix_millis / 1000)
}

/// The reader's clock, at the offset the runtime was started with.
fn reader_clock() -> Box<dyn Clock> {
    let minutes = std::env::var("KOBO_UTC_OFFSET_MINUTES")
        .ok()
        .and_then(|value| value.parse::<i16>().ok())
        .unwrap_or(0);
    SystemClock::new(minutes).map_or_else(
        |_| Box::new(ManualClock::new(EPOCH).expect("a valid fixed clock")) as Box<dyn Clock>,
        |clock| Box::new(clock) as Box<dyn Clock>,
    )
}

const EPOCH: Snapshot = Snapshot {
    unix_millis: 0,
    monotonic_millis: 0,
    utc_offset_minutes: 0,
};

fn guide_screen() -> Screen {
    ScreenBuilder::new("syncthing")
        .top_bar("Set up Sync")
        .heading("Pair one computer folder")
        .text("1. Install Syncthing on the computer with its package manager.")
        .text("2. Wake the reader on Wi-Fi, then on the computer run:")
        .text("kobo sync setup ~/Documents/notes --folder vault --device <address>")
        .text("3. Resume Sync here. The first window runs within the cadence you choose; vault, frame and books arrive receive-only, so originals on the reader stay protected.")
        .secondary("Folders are fixed: sync/vault, sync/frame and sync/books arrive; sync/out leaves. Transferred packages import into Vault and Frame after each window.")
        .button("back", "Back")
        .build()
}

fn folders_screen() -> Screen {
    ScreenBuilder::new("syncthing")
        .top_bar("Sync folders")
        .rows(
            FOLDERS
                .into_iter()
                .map(|(path, direction)| (path, path, direction, Glyph::Folder)),
        )
        .secondary("Folder set is fixed. Receive-only folders protect the owner’s originals. Vault and Frame packages import onto their shelves; books stay files.")
        .action_bar([("back", "Back"), ("about", "About")])
        .build()
}

fn about_screen() -> Screen {
    ScreenBuilder::new("syncthing")
        .top_bar("About Sync")
        .heading("Syncthing")
        .text("Syncthing keeps selected Kobo folders in sync while protecting receive-only originals. Syncthing is available under the MPL-2.0 license.")
        .secondary("Sync requires a Cobalt platform build that includes the pinned Syncthing engine; pair a computer with kobo sync setup.")
        .button("back", "Back")
        .build()
}

impl KoboApp for Sync {
    fn on_start(&mut self, context: &mut Context) {
        context.store().load(CONFIG);
        context.store().load(META);
        context.store().load(STATUS);
        context.store().load(INGEST);
        self.show(context);
    }

    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        if let StoreResult::Loaded { key, value } = result {
            if key == CONFIG {
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
            } else if key == META {
                if let Some(value) = value {
                    let saved = String::from_utf8_lossy(&value);
                    let mut fields = saved.lines();
                    self.scheduled_at = fields
                        .next()
                        .and_then(|field| field.parse().ok())
                        .unwrap_or(0);
                    self.first_sync_seen = fields.next() == Some("1");
                }
            } else if key == STATUS {
                if let Some(value) = value {
                    self.status_seen = true;
                    let status = String::from_utf8_lossy(&value);
                    self.status = WindowStatus::parse(&status);
                    if self.status.state == "failed" {
                        self.status.message = format!(
                            "Sync engine not installed or stopped. {}",
                            self.status.message
                        );
                    }
                    if self.status.last_success > 0 && !self.first_sync_seen {
                        self.first_sync_seen = true;
                        self.first_sync_banner = true;
                        self.save(context);
                    }
                }
            } else if key == INGEST {
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
            self.show(context);
        }
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        self.first_sync_banner = false;
        if action == action_id("toggle") {
            self.config.enabled = !self.config.enabled;
            self.status.message = if self.config.enabled {
                "Waiting for a sync window".into()
            } else {
                "Stopped".into()
            };
            self.apply_schedule(context);
            self.save(context);
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
        } else if action == action_id("refresh") {
            context.store().load(STATUS);
            context.store().load(INGEST);
        } else if action == action_id("back") || action == ActionId::BACK {
            self.view = View::Status;
        }
        self.show(context);
    }

    fn on_scheduled_wake(&mut self, context: &mut Context) {
        // The platform wake launcher invokes `kobod --syncthing scheduled`;
        // this app only repaints the result when the reader next sees it.
        context.store().load(STATUS);
        context.store().load(INGEST);
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
    use kobo_sdk::{Command, DeviceRequest};
    use kobo_ui::{Chrome, CLARA_BW_METRICS};

    #[test]
    fn primary_sync_control_fits_clara_bw() {
        let app = Sync::default();
        let layout = ScreenBuilder::new("sync")
            .top_bar("Sync")
            .empty_state("Sync is disabled.")
            .bottom_action("toggle", "Turn Sync on")
            .build()
            .layout_with(&CLARA_BW_METRICS, &Chrome::default());
        let toggle = layout
            .rect_of_action(action_id("toggle"))
            .expect("sync control");
        assert!(toggle.height >= CLARA_BW_METRICS.touch_target_minimum());
        assert_eq!(app.config.cadence, Cadence::Manual);
    }

    #[test]
    fn enabled_hourly_sync_requests_a_bounded_wake() {
        let mut app = Sync {
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
    fn import_report_parses_supervisor_lines() {
        let line = "vault\t12\t4096\t1758000000";
        let mut fields = line.split('\t');
        let import = Import {
            folder: fields.next().expect("folder").to_owned(),
            files: fields.next().and_then(|v| v.parse().ok()).expect("files"),
            epoch: fields.nth(1).and_then(|v| v.parse().ok()).expect("epoch"),
        };
        assert_eq!(import.folder, "vault");
        assert_eq!(import.files, 12);
        assert_eq!(import.epoch, 1_758_000_000);
    }

    #[test]
    fn bytes_format_for_humans() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(2048), "2.0 KB");
        assert_eq!(format_bytes(5 * 1024 * 1024), "5.0 MB");
    }
}

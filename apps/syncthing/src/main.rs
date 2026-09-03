mod supervisor;
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Glyph, KoboApp, ScreenBuilder, StoreResult,
};
use std::process::ExitCode;
use std::time::Duration;
use supervisor::{Cadence, Config, FOLDERS};

const CONFIG: &str = "sync-config";
const STATUS: &str = "sync-status";
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum View {
    Status,
    Folders,
    About,
}
struct Sync {
    config: Config,
    view: View,
    last: String,
    peers: u8,
    transferred: u64,
}
impl Default for Sync {
    fn default() -> Self {
        Self {
            config: Config::default(),
            view: View::Status,
            last: "Not run".into(),
            peers: 0,
            transferred: 0,
        }
    }
}
impl Sync {
    fn save(&self, context: &mut Context) {
        context.store().save(
            CONFIG,
            format!("{}\n{}", self.config.enabled, self.config.cadence as u8),
        );
    }
    fn apply_schedule(&self, context: &mut Context) {
        let seconds = match self.config.cadence {
            Cadence::Manual => None,
            Cadence::Hourly => Some(60 * 60),
            Cadence::FourHourly => Some(4 * 60 * 60),
            Cadence::Daily => Some(24 * 60 * 60),
        };
        match (self.config.enabled, seconds) {
            (true, Some(seconds)) => context.device().schedule_wake(Duration::from_secs(seconds)),
            _ => context.device().cancel_wake(),
        }
    }
    fn show(&self, context: &mut Context) {
        let screen = match self.view {
            View::Status => {
                let state = if self.config.enabled { "Enabled" } else { "Disabled" };
                let mut screen = ScreenBuilder::new("syncthing").top_bar("Sync").section_with_value("Service", state).rows([
                    ("toggle", if self.config.enabled { "Pause Sync".to_owned() } else { "Resume Sync".to_owned() }, "Changes take effect during the current sync window.".to_owned(), Glyph::Settings),
                    ("cadence", self.config.cadence.label().to_owned(), format!("Up to {} radio minutes per day.", self.config.cadence.radio_minutes()), Glyph::Clock),
                    ("folders", "Folders".to_owned(), "Vault, frame, books and out.".to_owned(), Glyph::Folder),
                    ("refresh", "Refresh status".to_owned(), "Read the runtime-owned Sync status.".to_owned(), Glyph::Refresh),
                    ("about", "About Sync".to_owned(), "How syncing works and software information.".to_owned(), Glyph::Settings),
                ]).facts([("Peers", self.peers.to_string()), ("Transferred", format!("{} bytes", self.transferred)), ("Last sync", self.last.clone())]);
                if !self.config.enabled { screen = screen.banner(BannerLevel::Info, "Sync is off."); }
                screen.build()
            }
            View::Folders => ScreenBuilder::new("syncthing").top_bar("Sync folders").rows(FOLDERS.into_iter().map(|(path, direction)| (path, path, direction, Glyph::Folder))).secondary("Folder set is fixed. Receive-only folders protect the owner’s originals.").button("back", "Back").build(),
            View::About => ScreenBuilder::new("syncthing").top_bar("About Sync").heading("Syncthing").text("Syncthing keeps selected Kobo folders in sync while protecting receive-only originals. Syncthing is available under the MPL-2.0 license.").secondary("Sync requires a Cobalt platform build that includes the pinned Syncthing engine; pair a computer with kobo sync setup.").button("back", "Back").build(),
        };
        context.set_screen(screen);
    }
}
impl KoboApp for Sync {
    fn on_start(&mut self, context: &mut Context) {
        context.store().load(CONFIG);
        context.store().load(STATUS);
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
            } else if key == STATUS {
                let status = value.as_deref().map_or_else(
                    || "disabled\n0\nSync has not run.".into(),
                    String::from_utf8_lossy,
                );
                let mut fields = status.lines();
                let state = fields.next().unwrap_or("disabled");
                self.transferred = fields
                    .next()
                    .and_then(|bytes| bytes.parse().ok())
                    .unwrap_or(0);
                fields
                    .next()
                    .unwrap_or("Sync has not run.")
                    .clone_into(&mut self.last);
                if state == "failed" {
                    self.last = format!("Sync engine not installed or stopped. {}", self.last);
                }
            }
            self.show(context);
        }
    }
    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if action == action_id("toggle") {
            self.config.enabled = !self.config.enabled;
            self.last = if self.config.enabled {
                "Waiting for a sync window".into()
            } else {
                "Stopped".into()
            };
            self.save(context);
            self.apply_schedule(context);
        } else if action == action_id("cadence") {
            self.config.cadence = self.config.cadence.next();
            self.save(context);
            self.apply_schedule(context);
        } else if action == action_id("folders") {
            self.view = View::Folders;
        } else if action == action_id("refresh") {
            context.store().load(STATUS);
        } else if action == action_id("about") {
            self.view = View::About;
        } else if action == action_id("back") || action == ActionId::BACK {
            self.view = View::Status;
        }
        self.show(context);
    }
    fn on_scheduled_wake(&mut self, context: &mut Context) {
        // The platform wake launcher invokes `kobod --syncthing scheduled`;
        // this app only repaints the result when the reader next sees it.
        context.store().load(STATUS);
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
}

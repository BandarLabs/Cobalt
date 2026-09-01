mod supervisor;
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Glyph, KoboApp, ScreenBuilder, StoreResult,
};
use std::process::ExitCode;
use supervisor::{Cadence, Config, FOLDERS};

const CONFIG: &str = "sync-config";
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
}
impl Default for Sync {
    fn default() -> Self {
        Self {
            config: Config::default(),
            view: View::Status,
            last: "Not run".into(),
            peers: 0,
        }
    }
}
impl Sync {
    fn save(&self, context: &mut Context) {
        context.store().save(
            CONFIG,
            format!(
                "{}\n{}\n{}",
                self.config.enabled, self.config.cadence as u8, self.config.device_id
            ),
        );
    }
    fn show(&self, context: &mut Context) {
        let screen = match self.view {
            View::Status => {
                let state = if self.config.enabled { "Enabled" } else { "Disabled" };
                let mut screen = ScreenBuilder::new("syncthing").top_bar("Sync").section_with_value("Service", state).rows([
                    ("toggle", if self.config.enabled { "Turn Sync off".to_owned() } else { "Turn Sync on".to_owned() }, "The default is off; disabled means no process and no listening ports.".to_owned(), Glyph::Settings),
                    ("cadence", self.config.cadence.label().to_owned(), format!("Up to {} radio minutes per day.", self.config.cadence.radio_minutes()), Glyph::Clock),
                    ("folders", "Folders".to_owned(), "Vault, frame, books and out.".to_owned(), Glyph::Folder),
                    ("about", "About Sync".to_owned(), "Syncthing MPL-2.0 attribution and engine status.".to_owned(), Glyph::Settings),
                ]).facts([("Peers", self.peers.to_string()), ("Last sync", self.last.clone())]);
                if !self.config.enabled { screen = screen.banner(BannerLevel::Info, "Sync is disabled. No engine is running."); }
                screen.build()
            }
            View::Folders => ScreenBuilder::new("syncthing").top_bar("Sync folders").rows(FOLDERS.into_iter().map(|(path, direction)| (path, path, direction, Glyph::Folder))).secondary("Folder set is fixed. Receive-only folders protect the owner’s originals.").button("back", "Back").build(),
            View::About => ScreenBuilder::new("syncthing").top_bar("About Sync").heading("Syncthing").text("The unmodified Syncthing engine is MPL-2.0. Source: github.com/syncthing/syncthing. kobod generates configuration, opens bounded windows, and keeps its REST API on 127.0.0.1.").secondary("Engine not installed until a platform update supplies the ARMv7 binary.").button("back", "Back").build(),
        };
        context.set_screen(screen);
    }
}
impl KoboApp for Sync {
    fn on_start(&mut self, context: &mut Context) {
        context.store().load(CONFIG);
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
                    self.config.device_id = fields.next().unwrap_or("KOBOSYNC-DEVICE-ID").into();
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
        } else if action == action_id("cadence") {
            self.config.cadence = self.config.cadence.next();
            self.save(context);
        } else if action == action_id("folders") {
            self.view = View::Folders;
        } else if action == action_id("about") {
            self.view = View::About;
        } else if action == action_id("back") || action == ActionId::BACK {
            self.view = View::Status;
        }
        self.show(context);
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
}

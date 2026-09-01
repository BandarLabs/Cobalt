mod ha;

use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Failure, Glyph, KoboApp, Screen, ScreenBuilder,
    Space, StoreResult, TaskId, TaskOutcome,
};
use std::process::ExitCode;

const BASE: &str = "base-url";
const TILES: &str = "tiles";
const STATES: &str = "states";
const REFRESH: &str = "refresh";
const SETTINGS: &str = "settings";
const ADD: &str = "add";
const BACK: &str = "back";
const MAX_TILES: usize = 12;

#[derive(Clone, Copy, Default, Eq, PartialEq)]
enum View {
    #[default]
    Opening,
    Setup,
    Grid,
    Settings,
    Add,
}

#[derive(Default)]
struct HomePanel {
    view: View,
    base: String,
    tiles: Vec<String>,
    states: Vec<(String, String)>,
    keyboard: Keyboard,
    task: Option<(TaskId, &'static str)>,
    banner: Option<String>,
}

impl HomePanel {
    fn show(&self, context: &mut Context) {
        context.set_screen(
            match self.view {
                View::Opening => ScreenBuilder::new("homepanel-opening")
                    .top_bar("Home Panel")
                    .activity("Opening", None)
                    .build(),
                View::Setup => self.setup(),
                View::Grid => self.grid(),
                View::Settings => self.settings(),
                View::Add => self.add(),
            }
            .with_own_back(matches!(self.view, View::Settings | View::Add)),
        );
    }
    fn setup(&self) -> Screen {
        let mut s = ScreenBuilder::new("homepanel-setup").top_bar("Home Panel")
            .heading("Connect Home Assistant")
            .text("Use an https URL. Install its long-lived token with kobo secret set homeassistant --device <ip>.")
            .field("home-url", self.keyboard.text(), "https://ha.example.net");
        if let Some(b) = &self.banner {
            s = s.banner(BannerLevel::Attention, b);
        }
        s.spacer(Space::Small)
            .keyboard(&self.keyboard, "Test connection")
            .build()
    }
    fn grid(&self) -> Screen {
        let mut s = ScreenBuilder::new("homepanel-grid").top_bar_action(SETTINGS, "Settings");
        s = if self.tiles.is_empty() {
            s.top_bar("Home Panel").splash(
                Some(Glyph::Light),
                "No tiles",
                "Add a Home Assistant entity in Settings.",
            )
        } else {
            s.top_bar("Home Panel").grid(
                2,
                false,
                self.tiles.iter().map(|id| {
                    let state = self
                        .states
                        .iter()
                        .find(|(known, _)| known == id)
                        .map_or("working…", |(_, value)| value.as_str());
                    (format!("tile.{id}"), format!("{}\n{}", title(id), state))
                }),
            )
        };
        if let Some(b) = &self.banner {
            s = s.banner(BannerLevel::Attention, b);
        }
        s.button(REFRESH, "Refresh now").build()
    }
    fn settings(&self) -> Screen {
        let mut s = ScreenBuilder::new("homepanel-settings")
            .top_bar("Settings")
            .section("Connection")
            .text(self.base.clone())
            .section("Tiles")
            .text(format!("{} of {MAX_TILES}", self.tiles.len()))
            .button(ADD, "Add a tile");
        if let Some(b) = &self.banner {
            s = s.banner(BannerLevel::Attention, b);
        }
        s.button(BACK, "Done").build()
    }
    fn add(&self) -> Screen {
        let mut s = ScreenBuilder::new("homepanel-add")
            .top_bar("Add a tile")
            .text("Type an entity id such as light.desk. Unsupported domains stay read-only.")
            .field("entity-id", self.keyboard.text(), "light.desk");
        if let Some(b) = &self.banner {
            s = s.banner(BannerLevel::Attention, b);
        }
        s.keyboard(&self.keyboard, "Add tile").build()
    }
    fn save_tiles(&self, context: &mut Context) {
        context
            .store()
            .save(TILES, self.tiles.join("\n").into_bytes());
    }
    fn save_states(&self, context: &mut Context) {
        let saved = self
            .states
            .iter()
            .map(|(id, state)| format!("{id}\t{state}"))
            .collect::<Vec<_>>()
            .join("\n");
        context.store().save(STATES, saved.into_bytes());
    }
    fn fetch(&mut self, context: &mut Context) {
        if self.base.is_empty() || self.tiles.is_empty() || self.task.is_some() {
            return;
        }
        if let Some(id) = context.spawn(ha::poll(&self.base, &self.tiles)) {
            self.task = Some((id, "poll"));
        }
    }
    fn test(&mut self, context: &mut Context) {
        if let Some(id) = context.spawn(ha::test_connection(&self.base)) {
            self.task = Some((id, "test"));
            self.banner = Some("Testing connection…".into());
            self.show(context);
        }
    }
}

fn title(id: &str) -> String {
    id.rsplit('.').next().unwrap_or(id).replace('_', " ")
}

impl KoboApp for HomePanel {
    fn on_start(&mut self, context: &mut Context) {
        context.store().load(BASE);
        context.store().load(TILES);
        context.store().load(STATES);
        self.show(context);
    }
    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        let StoreResult::Loaded { key, value } = result else {
            return;
        };
        let text = value
            .and_then(|b| String::from_utf8(b).ok())
            .unwrap_or_default();
        if key == BASE {
            self.base = text;
        } else if key == TILES {
            self.tiles = text.lines().map(str::to_owned).collect();
        } else if key == STATES {
            self.states = text
                .lines()
                .filter_map(|line| {
                    let (id, state) = line.split_once('\t')?;
                    Some((id.to_owned(), state.to_owned()))
                })
                .collect();
        }
        if self.view == View::Opening {
            self.view = if self.base.is_empty() {
                self.keyboard = Keyboard::with_text("https://");
                View::Setup
            } else {
                View::Grid
            };
            self.show(context);
            self.fetch(context);
        } else if key == TILES && self.view == View::Grid {
            self.fetch(context);
        }
    }
    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if action == ActionId::BACK {
            self.view = View::Grid;
            self.show(context);
            return;
        }
        if matches!(self.view, View::Setup | View::Add) {
            if let Some(key) = self.keyboard.press(action) {
                if matches!(key, Pressed::Edited | Pressed::Shifted) {
                    self.show(context);
                }
                if matches!(key, Pressed::Submitted) {
                    let text = self.keyboard.text().trim().to_owned();
                    if self.view == View::Setup {
                        if text.starts_with("https://") {
                            self.base = text;
                            context.store().save(BASE, self.base.clone().into_bytes());
                            self.test(context);
                        } else {
                            self.banner = Some("Use an https:// Home Assistant URL.".into());
                            self.show(context);
                        }
                    } else if text.contains('.') && self.tiles.len() < MAX_TILES {
                        self.tiles.push(text);
                        self.keyboard.clear();
                        self.save_tiles(context);
                        self.view = View::Grid;
                        self.banner = None;
                        self.show(context);
                        self.fetch(context);
                    } else {
                        self.banner = Some("Enter an entity id; the grid holds 12 tiles.".into());
                        self.show(context);
                    }
                }
                return;
            }
        }
        if action == action_id(SETTINGS) {
            self.view = View::Settings;
            self.show(context);
        } else if action == action_id(ADD) {
            self.keyboard.clear();
            self.view = View::Add;
            self.show(context);
        } else if action == action_id(BACK) {
            self.view = View::Grid;
            self.show(context);
            self.fetch(context);
        } else if action == action_id(REFRESH) {
            self.fetch(context);
        } else if self.view == View::Grid {
            if let Some(id) = self
                .tiles
                .iter()
                .find(|id| action == action_id(&format!("tile.{id}")))
                .cloned()
            {
                if let Some(task) = context.spawn(ha::service(&self.base, &id)) {
                    self.task = Some((task, "service"));
                    self.banner = Some(format!("{} pending", title(&id)));
                    self.show(context);
                }
            }
        }
    }
    fn on_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
        let Some((known, kind)) = self.task.take() else {
            return;
        };
        if known != task {
            return;
        }
        match (kind, outcome) {
            ("test", TaskOutcome::Completed(_)) => {
                self.banner = None;
                self.view = View::Grid;
                self.show(context);
                self.fetch(context);
            }
            ("poll", TaskOutcome::Completed(bytes)) => {
                let states = ha::state_rows(&bytes);
                if states != self.states {
                    self.states = states;
                    self.save_states(context);
                    self.banner = None;
                    self.show(context);
                }
            }
            ("service", TaskOutcome::Completed(_)) => {
                self.banner = Some("Sent; refreshing to confirm.".into());
                self.show(context);
                self.fetch(context);
            }
            (_, TaskOutcome::Failed(error)) => {
                self.banner = Some(format!(
                    "Off the air — {}. Check Wi-Fi or kobo secret set homeassistant.",
                    Failure::of(error).advice
                ));
                self.show(context);
            }
            _ => {}
        }
    }
}

fn main() -> ExitCode {
    kobo_sdk::run("homepanel", HomePanel::default())
        .map_or_else(|_| ExitCode::FAILURE, |()| ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grid_layout_fits_twelve_home_tiles() {
        let app = HomePanel {
            view: View::Grid,
            tiles: (0..12).map(|n| format!("light.{n}")).collect(),
            ..Default::default()
        };
        assert!(!app.grid().layout().nodes.is_empty());
    }
    #[test]
    fn title_is_an_entity_label_not_an_id() {
        assert_eq!(title("light.desk_lamp"), "desk lamp");
    }
}

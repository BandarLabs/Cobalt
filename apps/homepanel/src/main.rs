mod ha;

use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Glyph, Heartbeat, KoboApp, Screen, ScreenBuilder,
    Space, StoreResult, TaskError, TaskId, TaskOutcome,
};
use std::process::ExitCode;

const BASE: &str = "base-url";
const TILES: &str = "tiles";
const STATES: &str = "states";
const SETTINGS: &str = "settings";
const ADD: &str = "add";
const SEARCH: &str = "search";
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
    Search,
}

struct HomePanel {
    view: View,
    base: String,
    tiles: Vec<String>,
    states: Vec<(String, String)>,
    entities: Vec<ha::Entity>,
    query: String,
    keyboard: Keyboard,
    task: Option<(TaskId, &'static str)>,
    poll_clock: Heartbeat,
    banner: Option<String>,
}

impl Default for HomePanel {
    fn default() -> Self {
        Self {
            view: View::default(),
            base: String::new(),
            tiles: Vec::new(),
            states: Vec::new(),
            entities: Vec::new(),
            query: String::new(),
            keyboard: Keyboard::new(),
            task: None,
            poll_clock: Heartbeat::every(10),
            banner: None,
        }
    }
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
                View::Search => self.search(),
            }
            .with_own_back(matches!(
                self.view,
                View::Settings | View::Add | View::Search
            )),
        );
    }
    fn setup(&self) -> Screen {
        let mut s = ScreenBuilder::new("homepanel-setup")
            .top_bar("Home Panel")
            .heading("Connect Home Assistant")
            .text(
                "Enter your Home Assistant address after finishing account setup on your computer.",
            )
            .field("home-url", self.keyboard.text(), "https://ha.example.net");
        if let Some(b) = &self.banner {
            s = s.banner(BannerLevel::Attention, b);
        }
        s.spacer(Space::Small)
            .keyboard(&self.keyboard, "Test connection")
            .build()
    }
    fn grid(&self) -> Screen {
        let mut s = ScreenBuilder::new("homepanel-grid")
            .top_bar("Home Panel")
            .top_bar_glyph(SETTINGS, "Settings", Glyph::Settings)
            .top_bar_glyph(ADD, "Add tile", Glyph::Plus);
        s = if self.tiles.is_empty() {
            s.splash(
                Some(Glyph::Light),
                "No tiles",
                "Add a Home Assistant device.",
            )
        } else {
            s.grid(
                2,
                false,
                self.tiles.iter().map(|id| {
                    let state = self
                        .states
                        .iter()
                        .find(|(known, _)| known == id)
                        .map_or("Not connected", |(_, value)| value.as_str());
                    (format!("tile.{id}"), format!("{} · {}", title(id), state))
                }),
            )
        };
        if let Some(b) = &self.banner {
            s = s.banner(BannerLevel::Attention, b);
        }
        s.build()
    }
    fn settings(&self) -> Screen {
        let mut s = ScreenBuilder::new("homepanel-settings")
            .top_bar("Settings")
            .top_bar_glyph(ADD, "Add tile", Glyph::Plus)
            .section("Connection")
            .text(self.base.clone())
            .section("Tiles")
            .text(format!("{} of {MAX_TILES}", self.tiles.len()));
        if let Some(b) = &self.banner {
            s = s.banner(BannerLevel::Attention, b);
        }
        s.button(BACK, "Done").build()
    }
    fn add(&self) -> Screen {
        let mut s = ScreenBuilder::new("homepanel-add")
            .top_bar("Add a tile")
            .top_bar_glyph(SEARCH, "Search", Glyph::Search);
        if let Some(b) = &self.banner {
            s = s.banner(BannerLevel::Attention, b);
        }
        if self.task.is_some_and(|(_, kind)| kind == "entities") {
            return s.activity("Finding devices", None).build();
        }
        let words = self.query.to_ascii_lowercase();
        let mut rows = self
            .entities
            .iter()
            .filter(|entity| {
                words.is_empty()
                    || entity.name.to_ascii_lowercase().contains(&words)
                    || entity.id.to_ascii_lowercase().contains(&words)
            })
            .filter(|entity| !self.tiles.contains(&entity.id))
            .take(40)
            .map(|entity| {
                (
                    format!("entity.{}", entity.id),
                    entity.name.clone(),
                    format!("{} · {}", entity.state, entity.id),
                    entity_glyph(&entity.id),
                )
            })
            .collect::<Vec<_>>();
        if rows.is_empty() && valid_entity_id(&self.query) && !self.tiles.contains(&self.query) {
            rows.push((
                format!("manual.{}", self.query),
                title(&self.query),
                "Add anyway".to_owned(),
                entity_glyph(&self.query),
            ));
        }
        if rows.is_empty() {
            s.splash(
                Some(Glyph::Search),
                if self.query.is_empty() {
                    "No devices found"
                } else {
                    "No matches"
                },
                if self.query.is_empty() {
                    "Check Home Assistant and try again."
                } else {
                    "Try a different name."
                },
            )
            .build()
        } else {
            s.rows(rows).build()
        }
    }
    fn search(&self) -> Screen {
        ScreenBuilder::new("homepanel-search")
            .top_bar("Search devices")
            .typed(&self.keyboard, "Name or entity")
            .keyboard(&self.keyboard, "Search")
            .build()
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
    fn load_entities(&mut self, context: &mut Context) {
        self.view = View::Add;
        self.query.clear();
        self.banner = None;
        if let Some(id) = context.spawn(ha::entities(&self.base)) {
            self.task = Some((id, "entities"));
        } else {
            self.banner = Some("Device list is busy. Try again in a moment.".into());
        }
        self.show(context);
    }
}

fn title(id: &str) -> String {
    id.rsplit('.').next().unwrap_or(id).replace('_', " ")
}

fn valid_entity_id(id: &str) -> bool {
    let Some((domain, name)) = id.split_once('.') else {
        return false;
    };
    !domain.is_empty()
        && !name.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._".contains(&byte))
}

fn entity_glyph(id: &str) -> Glyph {
    match id.split('.').next().unwrap_or_default() {
        "light" => Glyph::Light,
        "switch" | "input_boolean" | "fan" | "scene" | "script" | "button" | "automation" => {
            Glyph::Power
        }
        "sensor" | "binary_sensor" => Glyph::Chart,
        "climate" => Glyph::Clock,
        "person" | "device_tracker" => Glyph::Person,
        "media_player" => Glyph::Play,
        _ => Glyph::Circle,
    }
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
            if self.view == View::Grid {
                self.poll_clock.start(context);
            }
        } else if key == TILES && self.view == View::Grid {
            self.fetch(context);
        }
    }
    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if action == ActionId::BACK {
            self.view = if self.view == View::Search {
                View::Add
            } else {
                View::Grid
            };
            self.show(context);
            return;
        }
        if matches!(self.view, View::Setup | View::Search) {
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
                    } else {
                        self.query = text;
                        self.view = View::Add;
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
            if self.tiles.len() >= MAX_TILES {
                self.banner = Some("Home Panel can show up to 12 tiles.".into());
                self.show(context);
            } else {
                self.load_entities(context);
            }
        } else if action == action_id(SEARCH) {
            self.keyboard = Keyboard::with_text(&self.query);
            self.view = View::Search;
            self.show(context);
        } else if action == action_id(BACK) {
            self.view = View::Grid;
            self.show(context);
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
                    self.banner = Some(format!("Updating {}…", title(&id)));
                    self.show(context);
                }
            }
        } else if self.view == View::Add {
            if action == action_id(&format!("manual.{}", self.query))
                && valid_entity_id(&self.query)
            {
                self.tiles.push(self.query.clone());
                self.save_tiles(context);
                self.view = View::Grid;
                self.banner = None;
                self.show(context);
            } else if let Some(entity) = self
                .entities
                .iter()
                .find(|entity| action == action_id(&format!("entity.{}", entity.id)))
            {
                self.tiles.push(entity.id.clone());
                self.states.push((entity.id.clone(), entity.state.clone()));
                self.save_tiles(context);
                self.save_states(context);
                self.view = View::Grid;
                self.banner = None;
                self.show(context);
            }
        }
    }
    fn on_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
        if self.poll_clock.on_task(context, task, &outcome) {
            if self.view == View::Grid {
                self.fetch(context);
            }
            return;
        }
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
                self.poll_clock.start(context);
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
            ("entities", TaskOutcome::Completed(bytes)) => {
                self.entities = ha::entity_rows(&bytes);
                self.banner = if self.entities.is_empty() {
                    Some("Home Assistant returned no devices.".into())
                } else {
                    None
                };
                self.show(context);
            }
            ("service", TaskOutcome::Completed(_)) => {
                self.banner = Some("Updated. Checking status…".into());
                self.show(context);
                self.fetch(context);
            }
            (_, TaskOutcome::Failed(TaskError::NoCredential)) => {
                self.banner = Some("Finish Home Assistant setup on your computer.".into());
                self.show(context);
            }
            ("service", TaskOutcome::Failed(_)) => {
                self.banner =
                    Some("Couldn't update that device. Check Home Assistant and Wi-Fi.".into());
                self.show(context);
            }
            ("entities" | "test", TaskOutcome::Failed(_)) => {
                self.banner =
                    Some("Couldn't load devices. Check Home Assistant setup and Wi-Fi.".into());
                self.show(context);
            }
            (_, TaskOutcome::Failed(_)) => {
                self.banner =
                    Some("Couldn't refresh devices. Check Home Assistant and Wi-Fi.".into());
                self.show(context);
            }
            _ => {}
        }
    }

    fn on_background(&mut self, context: &mut Context) {
        self.poll_clock.stop(context);
    }

    fn on_foreground(&mut self, context: &mut Context) {
        if self.view == View::Grid {
            self.poll_clock.start(context);
            self.fetch(context);
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

    #[test]
    fn empty_grid_offers_add_and_settings_as_header_icons() {
        let debug = format!("{:?}", HomePanel::default().grid());
        assert!(debug.contains("Plus"), "{debug}");
        assert!(debug.contains("Settings"), "{debug}");
        assert!(!debug.contains("Refresh now"), "{debug}");
    }

    #[test]
    fn picker_searches_names_and_entity_ids() {
        let app = HomePanel {
            view: View::Add,
            query: "kitchen".into(),
            entities: vec![
                ha::Entity {
                    id: "light.kitchen".into(),
                    name: "Ceiling lights".into(),
                    state: "on".into(),
                },
                ha::Entity {
                    id: "sensor.office".into(),
                    name: "Office temperature".into(),
                    state: "21".into(),
                },
            ],
            ..HomePanel::default()
        };
        let debug = format!("{:?}", app.add());
        assert!(debug.contains("Ceiling lights"), "{debug}");
        assert!(!debug.contains("Office temperature"), "{debug}");
    }

    #[test]
    fn exact_entity_ids_can_be_added_before_the_picker_connects() {
        let app = HomePanel {
            view: View::Add,
            query: "light.kitchen".into(),
            ..HomePanel::default()
        };
        let debug = format!("{:?}", app.add());
        assert!(debug.contains("Add anyway"), "{debug}");
        assert!(valid_entity_id("light.kitchen"));
        assert!(!valid_entity_id("Kitchen light"));
    }
}

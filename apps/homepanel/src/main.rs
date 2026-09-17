//! Home Panel keeps a handful of Home Assistant controls on the reader:
//! tiles that toggle, climate tiles that set a temperature, and honest
//! connection state for everything else.
mod ha;

use kobo_sdk::clock::{Clock, ManualClock, Snapshot, SystemClock};
use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Glyph, Heartbeat, KoboApp, Screen, ScreenBuilder,
    Space, StoreResult, TaskError, TaskId, TaskOutcome,
};
use std::process::ExitCode;

const BASE: &str = "base-url";
const TILES: &str = "tiles";
const STATES: &str = "states";
const CLIMATE: &str = "climate";
const SETTINGS: &str = "settings";
const LASTOK: &str = "last-ok";
const ADD: &str = "add";
const SEARCH: &str = "search";
const BACK: &str = "back";
const MAX_TILES: usize = 12;
const EDIT_PAGE: usize = 6;
const STALE_AFTER_MINUTES: i64 = 15;

#[derive(Clone, Default, PartialEq)]
enum View {
    #[default]
    Opening,
    Setup,
    Grid,
    Settings,
    Edit,
    Tile(usize),
    Add,
    Search,
    Climate(String),
}

struct HomePanel {
    view: View,
    base: String,
    tiles: Vec<String>,
    states: Vec<(String, String)>,
    climate: Vec<(String, ha::Climate)>,
    entities: Vec<ha::Entity>,
    query: String,
    keyboard: Keyboard,
    task: Option<(TaskId, &'static str)>,
    poll_clock: Heartbeat,
    banner: Option<String>,
    wall: bool,
    last_ok: Option<(i64, String)>,
    pending: Option<(String, String)>,
    edit_page: usize,
    poll_failed: bool,
}

impl Default for HomePanel {
    fn default() -> Self {
        Self {
            view: View::default(),
            base: String::new(),
            tiles: Vec::new(),
            states: Vec::new(),
            climate: Vec::new(),
            entities: Vec::new(),
            query: String::new(),
            keyboard: Keyboard::new(),
            task: None,
            poll_clock: Heartbeat::every(10),
            banner: None,
            wall: false,
            last_ok: None,
            pending: None,
            edit_page: 0,
            poll_failed: false,
        }
    }
}

fn reader_clock() -> Box<dyn Clock> {
    let minutes = std::env::var("KOBO_UTC_OFFSET_MINUTES")
        .ok()
        .and_then(|value| value.parse::<i16>().ok())
        .unwrap_or(0);
    SystemClock::new(minutes)
        .or_else(|_| SystemClock::new(0))
        .map_or_else(
            |_| {
                Box::new(
                    ManualClock::new(Snapshot {
                        unix_millis: 0,
                        monotonic_millis: 0,
                        utc_offset_minutes: 0,
                    })
                    .expect("a valid fixed clock"),
                ) as Box<dyn Clock>
            },
            |clock| Box::new(clock) as Box<dyn Clock>,
        )
}

fn now_minutes() -> Option<i64> {
    reader_clock()
        .now()
        .ok()
        .map(|snapshot| i64::try_from(snapshot.unix_millis / 60_000).unwrap_or(i64::MAX))
}

fn now_hhmm() -> String {
    reader_clock()
        .now()
        .ok()
        .and_then(Snapshot::hour_minute)
        .map_or_else(String::new, |(hour, minute)| {
            format!("{hour:02}:{minute:02}")
        })
}

/// What a failed task means for the person holding the reader, in the
/// words of the thing they can actually do about it.
fn failure_message(error: TaskError) -> String {
    match error {
        TaskError::NoCredential => {
            "The homeassistant token is not on this reader. Install it from your computer: kobo secret set homeassistant.".to_owned()
        }
        TaskError::Offline => "No network. Join Wi-Fi, then try again.".to_owned(),
        TaskError::Unreachable | TaskError::TimedOut => {
            "Home Assistant did not answer. Check the address and that it is running.".to_owned()
        }
        TaskError::Unauthorized => {
            "Home Assistant refused the token. Create a new long-lived access token and install it again.".to_owned()
        }
        TaskError::RateLimited(seconds) => {
            format!("Home Assistant asked us to wait {seconds}s. Trying again shortly.")
        }
        TaskError::TooLarge => {
            "Home Assistant answered with more than this panel can hold.".to_owned()
        }
        TaskError::NotFound => {
            "Home Assistant answered not found. Check the address ends at the server root."
                .to_owned()
        }
        TaskError::Denied => "The panel is not allowed to make that request.".to_owned(),
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

fn is_climate(id: &str) -> bool {
    id.split('.').next() == Some("climate")
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

fn format_degrees(value: f64) -> String {
    let rounded = (value * 2.0).round() / 2.0;
    if (rounded - rounded.round()).abs() < f64::EPSILON {
        format!("{rounded:.0}")
    } else {
        format!("{rounded:.1}")
    }
}

impl HomePanel {
    fn state_of(&self, id: &str) -> Option<&str> {
        self.states
            .iter()
            .find(|(known, _)| known == id)
            .map(|(_, value)| value.as_str())
    }

    fn climate_of(&self, id: &str) -> ha::Climate {
        self.climate
            .iter()
            .find(|(known, _)| known == id)
            .map_or(ha::Climate::default(), |(_, climate)| *climate)
    }

    fn tile_label(&self, id: &str) -> String {
        let climate = self.climate_of(id);
        if is_climate(id) {
            if let Some(current) = climate.current {
                return format!("{} · {}°", title(id), format_degrees(current));
            }
        }
        match self.state_of(id) {
            Some(state) => format!("{} · {state}", title(id)),
            None => format!("{} · Not connected", title(id)),
        }
    }

    fn freshness(&self) -> Option<String> {
        if self.tiles.is_empty() {
            return None;
        }
        match &self.last_ok {
            None => Some("Not connected yet.".to_owned()),
            Some((minutes, hhmm)) => {
                let stale = now_minutes().is_some_and(|now| now - minutes > STALE_AFTER_MINUTES);
                if stale {
                    Some(format!("Offline since {hhmm}. Readings may be stale."))
                } else {
                    Some(format!("Updated {hhmm}."))
                }
            }
        }
    }

    fn show(&self, context: &mut Context) {
        let screen = match &self.view {
            View::Opening => ScreenBuilder::new("homepanel-opening")
                .top_bar("Home Panel")
                .activity("Opening", None)
                .build(),
            View::Setup => self.setup(),
            View::Grid => self.grid(),
            View::Settings => self.settings(),
            View::Edit => self.edit(),
            View::Tile(index) => self.tile(*index),
            View::Add => self.add(),
            View::Search => self.search(),
            View::Climate(id) => self.climate_screen(id),
        };
        context.set_screen(screen.with_own_back(matches!(
            self.view,
            View::Settings
                | View::Edit
                | View::Tile(_)
                | View::Add
                | View::Search
                | View::Climate(_)
        )));
    }

    fn setup(&self) -> Screen {
        let mut s = ScreenBuilder::new("homepanel-setup")
            .top_bar("Home Panel")
            .heading("Connect Home Assistant")
            .text(
                "Enter your Home Assistant address after installing the token from your computer. The test below checks the address, the token, and the network.",
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
            if let Some(freshness) = self.freshness() {
                s = s.secondary(freshness);
            }
            s.grid(
                if self.wall { 1 } else { 2 },
                false,
                self.tiles
                    .iter()
                    .map(|id| (format!("tile.{id}"), self.tile_label(id))),
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
            .section("Connection")
            .text(self.base.clone())
            .button("change-url", "Change address")
            .section("Wall panel")
            .text(if self.wall {
                "One large tile per row. Best for a reader mounted by the door."
            } else {
                "Two tiles per row."
            })
            .button(
                "wall",
                if self.wall {
                    "Use two columns"
                } else {
                    "Use one large column"
                },
            )
            .section("Tiles")
            .text(format!("{} of {MAX_TILES}", self.tiles.len()));
        if let Some(b) = &self.banner {
            s = s.banner(BannerLevel::Attention, b);
        }
        s.button("edit", "Edit tiles").button(BACK, "Done").build()
    }

    fn edit(&self) -> Screen {
        let mut s = ScreenBuilder::new("homepanel-edit").top_bar("Edit tiles");
        if self.tiles.is_empty() {
            return s
                .splash(Some(Glyph::Light), "No tiles", "Add a tile first.")
                .button(BACK, "Done")
                .build();
        }
        let pages = self.tiles.len().div_ceil(EDIT_PAGE);
        let page = self.edit_page.min(pages - 1);
        let from = page * EDIT_PAGE;
        let to = (from + EDIT_PAGE).min(self.tiles.len());
        s = s.rows((from..to).map(|index| {
            (
                format!("edit.{index}"),
                title(&self.tiles[index]),
                self.state_of(&self.tiles[index])
                    .unwrap_or("Not connected")
                    .to_owned(),
                entity_glyph(&self.tiles[index]),
            )
        }));
        if let Some(b) = &self.banner {
            s = s.banner(BannerLevel::Attention, b);
        }
        if pages > 1 {
            s = s.button("more", format!("More tiles ({}/{pages})", page + 1));
        }
        s.button(BACK, "Done").build()
    }

    fn tile(&self, index: usize) -> Screen {
        let mut s = ScreenBuilder::new("homepanel-tile").top_bar("Tile");
        let Some(id) = self.tiles.get(index) else {
            return s
                .splash(None, "Tile is gone", "It was removed.")
                .button(BACK, "Done")
                .build();
        };
        s = s.heading(title(id)).text(id.clone()).text(format!(
            "State: {}",
            self.state_of(id).unwrap_or("Not connected")
        ));
        let climate = self.climate_of(id);
        if let Some(current) = climate.current {
            s = s.text(format!("Room: {}°", format_degrees(current)));
        }
        if let Some(target) = climate.target {
            s = s.text(format!("Target: {}°", format_degrees(target)));
        }
        let mut buttons = Vec::new();
        if index > 0 {
            buttons.push(("move-up", "Move up"));
        }
        buttons.push(("remove", "Remove"));
        buttons.push((BACK, "Done"));
        s.buttons(buttons).build()
    }

    fn climate_screen(&self, id: &str) -> Screen {
        let mut s = ScreenBuilder::new("homepanel-climate").top_bar(title(id));
        let climate = self.climate_of(id);
        s = s.text(format!(
            "State: {}",
            self.state_of(id).unwrap_or("Not connected")
        ));
        if let Some(current) = climate.current {
            s = s.text(format!("Room: {}°", format_degrees(current)));
        }
        if let Some(target) = climate.target {
            s = s.text(format!("Target: {}°", format_degrees(target)));
        }
        if let Some(b) = &self.banner {
            s = s.banner(BannerLevel::Attention, b);
        }
        s.buttons([("cool", "Cooler"), ("warm", "Warmer"), ("power", "Power")])
            .build()
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

    fn save_climate(&self, context: &mut Context) {
        let saved = self
            .climate
            .iter()
            .map(|(id, climate)| {
                format!(
                    "{id}\t{}\t{}",
                    climate
                        .current
                        .map_or_else(|| "-".to_owned(), |v| v.to_string()),
                    climate
                        .target
                        .map_or_else(|| "-".to_owned(), |v| v.to_string()),
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        context.store().save(CLIMATE, saved.into_bytes());
    }

    fn save_settings(&self, context: &mut Context) {
        let wall = if self.wall { "wall=1\n" } else { "" };
        context.store().save(SETTINGS, wall.as_bytes().to_vec());
    }

    fn save_last_ok(&self, context: &mut Context) {
        if let Some((minutes, hhmm)) = &self.last_ok {
            context
                .store()
                .save(LASTOK, format!("{minutes}|{hhmm}").into_bytes());
        }
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

    fn set_climate(&mut self, context: &mut Context, id: &str, delta: f64) {
        let climate = self.climate_of(id);
        let Some(target) = climate.target else {
            self.banner = Some("Home Assistant has not reported a target temperature yet.".into());
            self.show(context);
            return;
        };
        let next = ((target + delta) * 2.0).round() / 2.0;
        if let Some(task) = context.spawn(ha::set_temperature(&self.base, id, next)) {
            self.task = Some((task, "climate"));
            self.banner = Some(format!("Setting {}°…", format_degrees(next)));
            self.show(context);
        }
    }

    fn on_poll(&mut self, context: &mut Context, bytes: &[u8]) {
        self.states = ha::state_rows(bytes);
        self.climate = ha::climate_rows(bytes);
        self.last_ok = now_minutes().map(|minutes| (minutes, now_hhmm()));
        self.save_states(context);
        self.save_climate(context);
        self.save_last_ok(context);
        if let Some((id, before)) = self.pending.take() {
            let now = self.state_of(&id).unwrap_or("Not connected");
            self.banner = Some(if now == before {
                format!("{} did not change. Check Home Assistant.", title(&id))
            } else {
                format!("{} is now {now}.", title(&id))
            });
        } else if self.poll_failed {
            // A recovery clears the outage banner; a quiet poll never erases a
            // named acknowledgement such as "fan removed.".
            self.banner = None;
        }
        self.poll_failed = false;
        self.show(context);
    }
    /// Chrome actions available across views: settings, add, search, and the
    /// buttons living on the settings and edit screens.
    fn act_chrome(&mut self, context: &mut Context, action: ActionId) -> bool {
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
            self.view = match &self.view {
                View::Edit => View::Settings,
                View::Tile(_) => View::Edit,
                _ => View::Grid,
            };
            self.show(context);
            self.fetch(context);
        } else if action == action_id("change-url") {
            self.keyboard = Keyboard::with_text(&self.base);
            self.view = View::Setup;
            self.show(context);
        } else if action == action_id("wall") {
            self.wall = !self.wall;
            self.save_settings(context);
            self.show(context);
        } else if action == action_id("edit") {
            self.edit_page = 0;
            self.view = View::Edit;
            self.show(context);
        } else if action == action_id("more") {
            let pages = self.tiles.len().div_ceil(EDIT_PAGE).max(1);
            self.edit_page = (self.edit_page + 1) % pages;
            self.show(context);
        } else {
            return false;
        }
        true
    }

    fn act_tile(&mut self, context: &mut Context, index: usize, action: ActionId) {
        let Some(id) = self.tiles.get(index).cloned() else {
            self.view = View::Edit;
            self.show(context);
            return;
        };
        if action == action_id("move-up") && index > 0 {
            self.tiles.swap(index - 1, index);
            self.save_tiles(context);
            self.view = View::Edit;
            self.show(context);
        } else if action == action_id("remove") {
            self.tiles.remove(index);
            self.save_tiles(context);
            self.banner = Some(format!("{} removed.", title(&id)));
            self.view = View::Edit;
            self.show(context);
            self.fetch(context);
        }
    }

    fn act_climate(&mut self, context: &mut Context, id: &str, action: ActionId) {
        if action == action_id("cool") {
            self.set_climate(context, id, -0.5);
        } else if action == action_id("warm") {
            self.set_climate(context, id, 0.5);
        } else if action == action_id("power") {
            self.toggle_tile(context, id);
        }
    }

    fn act_grid(&mut self, context: &mut Context, action: ActionId) {
        if let Some(id) = self
            .tiles
            .iter()
            .find(|id| action == action_id(&format!("tile.{id}")))
            .cloned()
        {
            if is_climate(&id) {
                self.banner = None;
                self.view = View::Climate(id);
                self.show(context);
            } else {
                self.toggle_tile(context, &id);
            }
        }
    }

    fn act_edit(&mut self, context: &mut Context, action: ActionId) {
        if let Some(index) =
            (0..self.tiles.len()).find(|i| action == action_id(&format!("edit.{i}")))
        {
            self.view = View::Tile(index);
            self.show(context);
        }
    }

    fn act_add(&mut self, context: &mut Context, action: ActionId) {
        if action == action_id(&format!("manual.{}", self.query)) && valid_entity_id(&self.query) {
            self.tiles.push(self.query.clone());
            self.save_tiles(context);
            self.view = View::Grid;
            self.banner = None;
            self.show(context);
        } else if let Some(entity) = self
            .entities
            .iter()
            .find(|entity| action == action_id(&format!("entity.{}", entity.id)))
            .cloned()
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

    fn toggle_tile(&mut self, context: &mut Context, id: &str) {
        if let Some(task) = context.spawn(ha::service(&self.base, id)) {
            self.task = Some((task, "service"));
            let state = self.state_of(id).unwrap_or_default().to_owned();
            self.banner = Some(format!("Updating {}…", title(id)));
            self.pending = Some((id.to_owned(), state));
            self.show(context);
        }
    }
}

impl KoboApp for HomePanel {
    fn on_start(&mut self, context: &mut Context) {
        context.store().load(BASE);
        context.store().load(TILES);
        context.store().load(STATES);
        context.store().load(CLIMATE);
        context.store().load(SETTINGS);
        context.store().load(LASTOK);
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
        } else if key == CLIMATE {
            self.climate = text
                .lines()
                .filter_map(|line| {
                    let mut parts = line.split('\t');
                    let id = parts.next()?;
                    let current = parts.next().and_then(|v| v.parse::<f64>().ok());
                    let target = parts.next().and_then(|v| v.parse::<f64>().ok());
                    Some((id.to_owned(), ha::Climate { current, target }))
                })
                .collect();
        } else if key == SETTINGS {
            self.wall = text.lines().any(|line| line == "wall=1");
        } else if key == LASTOK {
            self.last_ok = text
                .split_once('|')
                .and_then(|(minutes, hhmm)| Some((minutes.parse::<i64>().ok()?, hhmm.to_owned())));
        }
        if self.view == View::Opening {
            if key == BASE {
                // Wait for the rest of the store before choosing a screen.
                return;
            }
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
            self.view = match &self.view {
                View::Search => View::Add,
                View::Tile(_) => View::Edit,
                View::Edit => View::Settings,
                _ => View::Grid,
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
        if self.act_chrome(context, action) {
            return;
        }
        match self.view.clone() {
            View::Tile(index) => self.act_tile(context, index, action),
            View::Climate(id) => self.act_climate(context, &id, action),
            View::Grid => self.act_grid(context, action),
            View::Edit => self.act_edit(context, action),
            View::Add => self.act_add(context, action),
            _ => {}
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
            ("test", TaskOutcome::Failed(error)) => {
                self.banner = Some(failure_message(error));
                self.show(context);
            }
            ("poll", TaskOutcome::Completed(bytes)) => {
                self.on_poll(context, &bytes);
            }
            ("poll", TaskOutcome::Failed(error)) => {
                self.poll_failed = true;
                let mut message = failure_message(error);
                if let Some((id, _)) = self.pending.take() {
                    message = format!("Couldn't confirm {}. {message}", title(&id));
                } else if let Some((_, hhmm)) = &self.last_ok {
                    message = format!("{message} Showing readings from {hhmm}.");
                }
                self.banner = Some(message);
                self.show(context);
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
            ("entities", TaskOutcome::Failed(error)) => {
                self.banner = Some(failure_message(error));
                self.view = View::Grid;
                self.show(context);
            }
            ("service", TaskOutcome::Completed(_)) => {
                self.banner = Some("Updated. Checking status…".into());
                self.show(context);
                self.fetch(context);
            }
            ("service", TaskOutcome::Failed(error)) => {
                let name = self
                    .pending
                    .take()
                    .map_or_else(|| "that device".to_owned(), |(id, _)| title(&id));
                self.banner = Some(format!(
                    "Couldn't update {name}. {}",
                    failure_message(error)
                ));
                self.show(context);
            }
            ("climate", TaskOutcome::Completed(_)) => {
                self.fetch(context);
            }
            ("climate", TaskOutcome::Failed(error)) => {
                self.banner = Some(format!(
                    "Couldn't set the temperature. {}",
                    failure_message(error)
                ));
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

    fn panel() -> HomePanel {
        HomePanel {
            view: View::Grid,
            base: "https://ha.example".into(),
            tiles: vec![
                "light.desk".into(),
                "switch.fan".into(),
                "climate.bedroom".into(),
            ],
            states: vec![
                ("light.desk".into(), "on".into()),
                ("switch.fan".into(), "off".into()),
                ("climate.bedroom".into(), "heat".into()),
            ],
            climate: vec![(
                "climate.bedroom".into(),
                ha::Climate {
                    current: Some(19.5),
                    target: Some(21.0),
                },
            )],
            ..Default::default()
        }
    }

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
    fn climate_tile_leads_with_the_room_temperature() {
        assert_eq!(panel().tile_label("climate.bedroom"), "bedroom · 19.5°");
        assert_eq!(panel().tile_label("light.desk"), "desk · on");
        assert_eq!(
            panel().tile_label("sensor.unknown"),
            "unknown · Not connected"
        );
    }

    #[test]
    fn freshness_says_never_fresh_or_stale() {
        let mut app = panel();
        assert_eq!(app.freshness().as_deref(), Some("Not connected yet."));
        app.last_ok = Some((now_minutes().unwrap_or(0), "22:18".to_owned()));
        let fresh = app.freshness().expect("freshness");
        assert!(fresh.starts_with("Updated "), "{fresh}");
        app.last_ok = Some((0, "22:18".to_owned()));
        let stale = app.freshness().expect("freshness");
        assert!(stale.starts_with("Offline since 22:18"), "{stale}");
    }

    #[test]
    fn failure_messages_name_the_fix() {
        assert!(failure_message(TaskError::NoCredential).contains("kobo secret set homeassistant"));
        assert!(failure_message(TaskError::Offline).contains("Wi-Fi"));
        assert!(failure_message(TaskError::Unreachable).contains("address"));
        assert!(failure_message(TaskError::Unauthorized).contains("long-lived access token"));
    }

    #[test]
    fn degrees_format_to_half_steps() {
        assert_eq!(format_degrees(21.0), "21");
        assert_eq!(format_degrees(19.5), "19.5");
        assert_eq!(format_degrees(20.26), "20.5");
    }

    #[test]
    fn edit_pages_six_tiles_at_a_time() {
        let app = HomePanel {
            view: View::Edit,
            tiles: (0..12).map(|n| format!("light.{n}")).collect(),
            edit_page: 1,
            ..Default::default()
        };
        let debug = format!("{:?}", app.edit());
        assert!(debug.contains("title: \"6\""), "{debug}");
        assert!(debug.contains("More tiles (2/2)"), "{debug}");
        assert!(!debug.contains("title: \"5\""), "{debug}");
    }

    #[test]
    fn wall_panel_is_one_column() {
        let mut app = panel();
        app.wall = true;
        let debug = format!("{:?}", app.grid());
        assert!(debug.contains("columns: 1"), "{debug}");
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

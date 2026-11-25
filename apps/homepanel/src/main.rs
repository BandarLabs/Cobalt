//! Home Panel keeps a handful of Home Assistant controls on the reader:
//! tiles that toggle, climate tiles that set a temperature, and honest
//! connection state for everything else.
mod ha;

use kobo_sdk::clock::{Clock, ManualClock, Snapshot, SystemClock};
use kobo_sdk::keyboard::{TextEntry, Typing};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, FieldKind, Glyph, Heartbeat, KoboApp, Position,
    Screen, ScreenBuilder, StoreResult, TaskError, TaskId, TaskOutcome,
};
use std::process::ExitCode;

const BASE: &str = "base-url";
const TILES: &str = "tiles";
const STATES: &str = "states";
const CLIMATE: &str = "climate";
/// Each tile's friendly name and unit, kept apart from the tile list so a
/// panel saved before names were kept still reads its tiles as it did.
const LABELS: &str = "labels";
const SETTINGS: &str = "settings";
const LASTOK: &str = "last-ok";
const ADD: &str = "add";
const SEARCH: &str = "search";
const BACK: &str = "back";
const CHECK_AGAIN: &str = "check-again";
const HOME_URL: &str = "home-url";
const PHONE: &str = "phone";
/// The one way out of the dialogue a failed action opens.
const CLOSE_DIALOG: &str = "close-dialog";
/// What Check again says when nothing has arrived. It is answered by checking
/// again, so the banner that says it carries that action.
const NO_ADDRESS_YET: &str =
    "No address has arrived from the computer yet. Run the command, then check again.";
/// Where a Home Assistant makes long-lived access tokens: the Security tab
/// of the owner's profile, under their own address.
const TOKEN_PAGE: &str = "/profile/security";
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
    /// A tile that is shown and not switched.
    Detail(String),
}

#[allow(
    clippy::struct_excessive_bools,
    reason = "independent facts about the panel, not the states of one machine"
)]
struct HomePanel {
    view: View,
    base: String,
    tiles: Vec<String>,
    states: Vec<(String, String)>,
    climate: Vec<(String, ha::Climate)>,
    /// What Home Assistant calls each tile and its reading's unit. A tile
    /// with no entry here, such as one saved before names were kept, is
    /// named from its entity ID until the next poll brings its name.
    labels: Vec<(String, ha::Label)>,
    entities: Vec<ha::Entity>,
    query: String,
    /// The address, typed on the runtime's keyboard.
    address: TextEntry,
    /// The address shown in the setup field until it is saved.
    draft: String,
    /// Why the typed address was refused, said under its field.
    address_error: Option<String>,
    /// Whether setup is showing the code for a phone.
    phone: bool,
    search_entry: TextEntry,
    task: Option<(TaskId, &'static str)>,
    poll_clock: Heartbeat,
    /// What the panel says about the last thing that happened, at the level
    /// its meaning asks for: a tile that changed is a success, a refused
    /// token an error, and a Home Assistant out of reach something to attend
    /// to, so the three never look alike.
    banner: Option<(BannerLevel, String)>,
    wall: bool,
    last_ok: Option<(i64, String)>,
    pending: Option<(String, String)>,
    edit_page: usize,
    grid_page: usize,
    picker_page: usize,
    /// The owner tapped Check again on setup and is owed an answer.
    checking: bool,
    /// Home Assistant could not list its devices, so the picker says how to
    /// add one by its entity ID instead.
    entities_failed: bool,
    /// What a tap on the grid or the climate screen came to, said in the
    /// status line in place of when the readings were taken.
    ///
    /// It used to be a banner over the tiles, so every tap pushed the grid
    /// down under the finger, and on the climate screen "Setting 21.5°…"
    /// landed between the readings and Cooler and Warmer mid-tapping.
    note: Option<String>,
    /// An action that did not happen, as a title and what went wrong. It
    /// needs reading, so it is a dialogue with one way out rather than a
    /// line that the next reading replaces.
    dialog: Option<(String, String)>,
    /// Why Home Assistant did not give the last request what it asked for,
    /// until a reading arrives. The status line says it beside the time of
    /// the readings on show, rather than "Updated 14:39." over a banner
    /// saying Home Assistant had stopped answering.
    trouble: Option<kobo_sdk::failure::Cause>,
}

impl Default for HomePanel {
    fn default() -> Self {
        Self {
            view: View::default(),
            base: String::new(),
            tiles: Vec::new(),
            states: Vec::new(),
            climate: Vec::new(),
            labels: Vec::new(),
            entities: Vec::new(),
            query: String::new(),
            address: TextEntry::new().address(),
            draft: String::new(),
            address_error: None,
            phone: false,
            search_entry: TextEntry::new(),
            task: None,
            poll_clock: Heartbeat::every(10),
            banner: None,
            wall: false,
            last_ok: None,
            pending: None,
            edit_page: 0,
            grid_page: 0,
            picker_page: 0,
            checking: false,
            entities_failed: false,
            note: None,
            dialog: None,
            trouble: None,
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

/// When a reading was taken, as the panel says it: the time alone today,
/// and with its day once it is older, so readings from last week do not pass
/// for this morning's.
fn reading_time(minutes: i64, hhmm: &str, now: Option<Snapshot>) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let Some(now) = now else {
        return hhmm.to_owned();
    };
    let then = Snapshot {
        unix_millis: u64::try_from(minutes.max(0))
            .unwrap_or(0)
            .saturating_mul(60_000),
        ..now
    };
    let (Some(day), Some(today), Some(date), Some(this)) =
        (then.local_day(), now.local_day(), then.date(), now.date())
    else {
        return hhmm.to_owned();
    };
    let month = MONTHS[usize::from(date.month.clamp(1, 12)) - 1];
    if day == today {
        hhmm.to_owned()
    } else if day + 1 == today {
        format!("yesterday at {hhmm}")
    } else if date.year == this.year {
        format!("{} {month} at {hhmm}", date.day)
    } else {
        format!("{} {month} {} at {hhmm}", date.day, date.year)
    }
}

/// What a failed task means for the person holding the reader, in the
/// words of the thing they can actually do about it. A Home Assistant on the
/// home network that does not answer is usually a panel that has left home,
/// so that is said before "check the address".
fn failure(base: &str, error: TaskError) -> kobo_sdk::failure::Problem {
    let service = kobo_sdk::failure::Service::new("Home Assistant").fixed_by(
        "Run kobo connect homepanel on your computer with a new long-lived access token.",
    );
    let service = if base.is_empty() {
        service
    } else {
        service.at(base)
    };
    service.explain(error)
}

/// How much of the connect screen the panel has room for, most first.
#[derive(Clone, Copy)]
enum Shape {
    Marked,
    Plain,
    Compact,
    /// Held sideways at the largest sizes with a banner up: the heading
    /// gives way and one sentence says what to do.
    Short,
    /// A long refusal held sideways at the largest sizes: the sentence and
    /// the field's label give way too, Use phone moves into the top bar, and
    /// the HTTPS rule is said under the field only while the address there
    /// does not already follow it.
    Bare,
}

/// What went wrong, in the few words the status line has room for. The
/// whole of it, with the fix, is the dialogue a failed tap opens.
const fn trouble_words(cause: kobo_sdk::failure::Cause) -> &'static str {
    use kobo_sdk::failure::Cause;
    match cause {
        Cause::Offline => "This reader is not on Wi-Fi.",
        Cause::AwayFromHome | Cause::Unreachable => "Home Assistant did not answer.",
        Cause::TimedOut => "Home Assistant was too slow.",
        Cause::Rejected => "Home Assistant refused your account.",
        Cause::Missing => "No account for Home Assistant yet.",
        Cause::Forbidden | Cause::Blocked => "Home Assistant refused the request.",
        Cause::NotFound => "Nothing found at that address.",
        Cause::RateLimited => "Home Assistant asked to slow down.",
        Cause::OutOfCredit | Cause::TooLarge => "Could not read Home Assistant.",
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

    fn label_of(&self, id: &str) -> Option<&ha::Label> {
        self.labels
            .iter()
            .find(|(known, _)| known == id)
            .map(|(_, label)| label)
    }

    /// The tile's name as Home Assistant gives it, "Desk lamp" rather than
    /// "desk", falling back to one made from the entity ID.
    fn name_of(&self, id: &str) -> String {
        self.label_of(id)
            .map(|label| label.name.as_str())
            .filter(|name| !name.is_empty())
            .map_or_else(|| title(id), str::to_owned)
    }

    /// The state with its unit, the way Home Assistant writes a reading:
    /// "12.3 °C", and "48%" with the percent sign set close. Only a number
    /// takes the unit, so "unavailable" never reads as "unavailable °C".
    fn reading_of(&self, id: &str) -> Option<String> {
        let state = self.state_of(id)?;
        let unit = self
            .label_of(id)
            .map(|label| label.unit.as_str())
            .unwrap_or_default();
        Some(if unit.is_empty() || state.trim().parse::<f64>().is_err() {
            state.to_owned()
        } else if unit == "%" || unit == "°" {
            format!("{state}{unit}")
        } else {
            format!("{state} {unit}")
        })
    }

    /// Learns names and units, keeping a name already known when Home
    /// Assistant has none to give, and only for tiles still on the panel.
    fn learn_labels(&mut self, rows: Vec<(String, ha::Label)>) -> bool {
        let before = self.labels.clone();
        for (id, label) in rows {
            if let Some((_, known)) = self.labels.iter_mut().find(|(known, _)| *known == id) {
                if !label.name.is_empty() {
                    known.name = label.name;
                }
                known.unit = label.unit;
            } else if !label.name.is_empty() || !label.unit.is_empty() {
                self.labels.push((id, label));
            }
        }
        let tiles = &self.tiles;
        self.labels.retain(|(id, _)| tiles.contains(id));
        self.labels != before
    }

    fn tile_label(&self, id: &str) -> String {
        let climate = self.climate_of(id);
        if is_climate(id) {
            if let Some(current) = climate.current {
                return format!("{} · {}°", self.name_of(id), format_degrees(current));
            }
        }
        match self.reading_of(id) {
            Some(reading) => format!("{} · {reading}", self.name_of(id)),
            None => format!("{} · Not connected", self.name_of(id)),
        }
    }

    /// When the readings on show were taken, and why they are not newer
    /// when Home Assistant did not answer, in one line.
    fn freshness(&self) -> Option<String> {
        if self.tiles.is_empty() {
            return None;
        }
        match (&self.last_ok, self.trouble.map(trouble_words)) {
            (None, None) => Some("Not connected yet.".to_owned()),
            (None, Some(trouble)) => Some(trouble.to_owned()),
            (Some((minutes, hhmm)), Some(trouble)) => {
                let when = reading_time(*minutes, hhmm, reader_clock().now().ok());
                Some(format!("{trouble} Readings from {when}."))
            }
            (Some((minutes, hhmm)), None) => {
                let stale = now_minutes().is_some_and(|now| now - minutes > STALE_AFTER_MINUTES);
                if stale {
                    let when = reading_time(*minutes, hhmm, reader_clock().now().ok());
                    Some(format!("Offline since {when}. Readings may be stale."))
                } else {
                    Some(format!("Updated {hhmm}."))
                }
            }
        }
    }

    /// The line under the bar on the grid and the climate screen: what the
    /// last tap came to, or else how fresh the readings are. It is there on
    /// every turn and keeps its place, so the tiles and buttons under it stay
    /// where the finger left them.
    fn status_line(&self) -> Option<String> {
        self.note.clone().or_else(|| self.freshness())
    }

    /// The dialogue for an action that did not happen, over whichever of
    /// the grid and the climate screen is showing.
    fn with_dialog(&self, screen: ScreenBuilder) -> ScreenBuilder {
        match &self.dialog {
            Some((title, text)) => screen.dialog(title, CLOSE_DIALOG, |dialog| {
                dialog.text(text).button(CLOSE_DIALOG, "Close")
            }),
            None => screen,
        }
    }

    /// Takes a tap that did not happen: the dialogue says what went wrong
    /// and the fix, and the status line what it means for the readings.
    fn action_failed(&mut self, title: String, error: TaskError) {
        let problem = failure(&self.base, error);
        self.trouble = Some(problem.cause);
        self.note = None;
        self.dialog = Some((title, problem.message));
    }

    /// The banner's words, whatever its level.
    fn banner_text(&self) -> Option<&str> {
        self.banner.as_ref().map(|(_, text)| text.as_str())
    }

    /// The banner on Settings and Edit tiles: only an acknowledgement of a
    /// change made there. An outage or a note about the grid is the grid's to
    /// say, and it used to follow the owner into every screen of settings.
    fn acknowledgement(&self) -> Option<&(BannerLevel, String)> {
        self.banner
            .as_ref()
            .filter(|(level, _)| *level == BannerLevel::Success)
    }

    /// The banner on a screen, at its own level.
    fn say(screen: ScreenBuilder, banner: Option<&(BannerLevel, String)>) -> ScreenBuilder {
        match banner {
            Some((level, text)) => screen.banner(*level, text),
            None => screen,
        }
    }

    /// Says what a failed request means, at the level the failure asks for,
    /// after `lead` when there is one.
    fn fail(&mut self, lead: Option<String>, error: TaskError, tail: Option<String>) {
        let problem = failure(&self.base, error);
        let level = problem.level();
        let mut message = problem.message;
        if let Some(lead) = lead {
            message = format!("{lead} {message}");
        }
        if let Some(tail) = tail {
            message = format!("{message} {tail}");
        }
        self.banner = Some((level, message));
    }

    fn show(&self, context: &mut Context) {
        let screen = match &self.view {
            View::Opening => ScreenBuilder::new("homepanel-opening")
                .top_bar("Home Panel")
                .activity("Opening", None)
                .build(),
            View::Setup => self.setup(context),
            View::Grid => self.grid(context),
            View::Settings => self.settings(),
            View::Edit => self.edit(),
            View::Tile(index) => self.tile(*index),
            View::Add => self.add(context),
            View::Search => self.search(),
            View::Climate(id) => self.climate_screen(id),
            View::Detail(id) => self.detail_screen(id),
        };
        context.set_screen(screen.with_own_back(
            (self.phone && self.view == View::Setup)
                || matches!(
                    self.view,
                    View::Settings
                        | View::Edit
                        | View::Tile(_)
                        | View::Add
                        | View::Search
                        | View::Climate(_)
                        | View::Detail(_)
                ),
        ));
    }

    /// The profile page where the owner makes a token, on the address in
    /// the field, while there is one that a phone can read off the panel.
    fn token_page(&self) -> Option<String> {
        let base = if self.draft.is_empty() {
            &self.base
        } else {
            &self.draft
        };
        let base = base.trim().trim_end_matches('/');
        (base.starts_with("https://") && base.len() > "https://".len())
            .then(|| format!("{base}{TOKEN_PAGE}"))
            .filter(|page| page.len() <= kobo_sdk::MAX_CODE_BYTES)
    }

    /// The first screen, until an address and a token are on the reader.
    ///
    /// Laid out as every connect screen is: what the owner needs to read
    /// first under the top bar, then what Home Panel needs and the one
    /// command that gives it, with Check again directly under the command.
    /// The token is named before anything has failed, because nothing works
    /// without it and the reader has no way to make one. Once a check has
    /// found nothing, Check again is the action of the banner that says so,
    /// and the button under the command goes, so it is offered once. Typing
    /// the address here comes after, for an owner without a computer to hand.
    /// Where the panel has no room, held sideways at the larger text sizes,
    /// the mark goes first, then the heading is set as a section, then the
    /// heading gives way to one sentence, and last, under a long refusal
    /// sideways at the largest sizes, see [`Shape::Bare`]; the command and
    /// every control stay.
    fn setup(&self, context: &Context) -> Screen {
        const TEXT: &str = "Run this on your computer and give it a long-lived access token. Home Assistant must be on HTTPS.";
        const SHORT: &str =
            "Run this on your computer with a long-lived token. Home Assistant must be on HTTPS.";
        if self.address.is_open() {
            return ScreenBuilder::new("homepanel-setup")
                .top_bar("Home Panel")
                .text_entry(&self.address, "Home Assistant address", "Test connection")
                .build();
        }
        if let Some(page) = self.token_page().filter(|_| self.phone) {
            return ScreenBuilder::new("homepanel-setup")
                .top_bar("Home Panel")
                .code(
                    page,
                    "Opens your Home Assistant profile, where long-lived access tokens are made.",
                )
                .bottom_action(HOME_URL, "Edit address")
                .build();
        }
        let checked = self.banner_text() == Some(NO_ADDRESS_YET);
        let bare = |shape: Shape| matches!(shape, Shape::Bare);
        let build = |shape: Shape| {
            let mut s = ScreenBuilder::new("homepanel-setup").top_bar("Home Panel");
            if bare(shape) && self.token_page().is_some() {
                s = s.top_bar_action(PHONE, "Use phone");
            }
            s = Self::say(s, self.banner.as_ref());
            if checked {
                s = s.banner_action(CHECK_AGAIN, "Check again");
            }
            s = match shape {
                Shape::Marked => s.splash(Some(Glyph::Light), "Connect Home Assistant", TEXT),
                Shape::Plain => s.splash(None, "Connect Home Assistant", TEXT),
                Shape::Compact => s.section("Connect Home Assistant").secondary(TEXT),
                Shape::Short => s.secondary(SHORT),
                Shape::Bare => s,
            }
            .command("kobo connect homepanel");
            if !checked {
                s = s.primary_button(CHECK_AGAIN, "Check again");
            }
            s = s.field(HOME_URL, &self.draft, "https://ha.example.net");
            if !bare(shape) {
                s = s.field_label("Home Assistant address");
            }
            s = s.field_kind(FieldKind::Address);
            if let Some(error) = &self.address_error {
                s = s.field_error(error);
            } else if bare(shape) && !self.draft.starts_with("https://") {
                s = s.field_hint("Home Assistant must be on HTTPS.");
            }
            if !bare(shape) && self.token_page().is_some() {
                s = s.button(PHONE, "Use phone");
            }
            s.build()
        };
        let metrics = context.metrics();
        [Shape::Marked, Shape::Plain, Shape::Compact, Shape::Short]
            .into_iter()
            .map(build)
            .find(|screen| {
                !screen
                    .diagnostics(&metrics, &kobo_sdk::Chrome::measuring(true))
                    .has_errors()
            })
            .unwrap_or_else(|| build(Shape::Bare))
    }

    /// The bar and the status line over the tiles. No banner: one coming
    /// and going above the grid moved every tile under the finger.
    fn grid_prefix(&self) -> ScreenBuilder {
        let mut screen = ScreenBuilder::new("homepanel-grid")
            .top_bar("Home Panel")
            .top_bar_glyph(SETTINGS, "Settings", Glyph::Settings)
            .top_bar_glyph(ADD, "Add tile", Glyph::Plus);
        if let Some(line) = self.status_line() {
            screen = screen.secondary(line);
        }
        self.with_dialog(screen)
    }

    fn page_controls(screen: ScreenBuilder, page: usize, pages: usize) -> ScreenBuilder {
        if pages <= 1 {
            return screen;
        }
        screen
            .page_turns("page-previous", "page-next")
            .page_position(
                u16::try_from(page + 1).unwrap_or(u16::MAX),
                u16::try_from(pages).unwrap_or(u16::MAX),
            )
    }

    fn grid_slice(&self, indices: &[usize], page: usize, pages: usize) -> Screen {
        let screen = self.grid_prefix().grid(
            if self.wall { 1 } else { 2 },
            false,
            indices.iter().filter_map(|&index| {
                let id = self.tiles.get(index)?;
                Some((format!("tile.{id}"), self.tile_label(id)))
            }),
        );
        Self::page_controls(screen, page, pages).build()
    }

    fn grid_pages(&self, context: &Context) -> Vec<Vec<usize>> {
        let all: Vec<usize> = (0..self.tiles.len()).collect();
        if self
            .grid_slice(&all, 0, 1)
            .diagnostics(&context.metrics(), &kobo_sdk::Chrome::measuring(true))
            .issues
            .is_empty()
        {
            return vec![all];
        }
        // A control grid has a different measure from a list of rows. Ask the
        // real layout whether each next tile fits, with page controls reserved.
        // There are at most twelve tiles, so this stays small even on a wall panel.
        let mut pages = Vec::new();
        let mut page = Vec::new();
        for index in all {
            page.push(index);
            if page.len() > 1
                && !self
                    .grid_slice(&page, 0, 2)
                    .diagnostics(&context.metrics(), &kobo_sdk::Chrome::measuring(true))
                    .issues
                    .is_empty()
            {
                page.pop();
                pages.push(std::mem::take(&mut page));
                page.push(index);
            }
        }
        if !page.is_empty() {
            pages.push(page);
        }
        pages
    }

    fn grid(&self, context: &Context) -> Screen {
        if self.tiles.is_empty() {
            return self
                .grid_prefix()
                .splash(
                    Some(Glyph::Light),
                    "No tiles",
                    "Add a Home Assistant device.",
                )
                .build();
        }
        let pages = self.grid_pages(context);
        let page = self.grid_page.min(pages.len().saturating_sub(1));
        self.grid_slice(&pages[page], page, pages.len())
    }

    /// Settings. The address is shown as the labelled field the setup screen
    /// uses, and tapping it opens setup with the address in it. Back is the
    /// way out; a Done beside it meant the same thing.
    fn settings(&self) -> Screen {
        // One setting with two states, so it is a switch, and its summary
        // says how the tiles are laid out now rather than what a tap does.
        let wall = if self.wall {
            "One large tile per row, for a reader by the door."
        } else {
            "Two tiles per row."
        };
        Self::say(
            ScreenBuilder::new("homepanel-settings").top_bar("Settings"),
            self.acknowledgement(),
        )
        .field("change-url", &self.base, "https://ha.example.net")
        .field_label("Home Assistant address")
        .field_kind(FieldKind::Address)
        .toggle("wall", "Wall panel", wall, self.wall)
        .section_with_value("Tiles", format!("{} of {MAX_TILES}", self.tiles.len()))
        .button("edit", "Edit tiles")
        .build()
    }

    /// The tiles in order, a page at a time. Back returns to settings.
    fn edit(&self) -> Screen {
        let mut s = ScreenBuilder::new("homepanel-edit").top_bar("Edit tiles");
        if self.tiles.is_empty() {
            return s
                .splash(Some(Glyph::Light), "No tiles", "Add a tile first.")
                .build();
        }
        let pages = self.tiles.len().div_ceil(EDIT_PAGE);
        let page = self.edit_page.min(pages - 1);
        let from = page * EDIT_PAGE;
        let to = (from + EDIT_PAGE).min(self.tiles.len());
        s = s.rows((from..to).map(|index| {
            (
                format!("edit.{index}"),
                self.name_of(&self.tiles[index]),
                self.reading_of(&self.tiles[index])
                    .unwrap_or_else(|| "Not connected".to_owned()),
                entity_glyph(&self.tiles[index]),
            )
        }));
        s = Self::say(s, self.acknowledgement());
        if pages > 1 {
            s = s.button("more", format!("More tiles ({}/{pages})", page + 1));
        }
        s.build()
    }

    fn tile(&self, index: usize) -> Screen {
        let mut s = ScreenBuilder::new("homepanel-tile").top_bar("Tile");
        let Some(id) = self.tiles.get(index) else {
            return s.splash(None, "Tile is gone", "It was removed.").build();
        };
        s = s.heading(self.name_of(id)).text(id.clone()).text(format!(
            "State: {}",
            self.reading_of(id)
                .unwrap_or_else(|| "Not connected".to_owned())
        ));
        let climate = self.climate_of(id);
        if let Some(current) = climate.current {
            s = s.text(format!("Room: {}°", format_degrees(current)));
        }
        if let Some(target) = climate.target {
            s = s.text(format!("Target: {}°", format_degrees(target)));
        }
        // Back is the way out. A Done beside Move up and Remove meant the
        // same thing, and at the largest sizes it squeezed the three into
        // thirds of the panel until "Remove" broke inside the word.
        if index > 0 {
            s.buttons([("move-up", "Move up"), ("remove", "Remove")])
                .build()
        } else {
            s.button("remove", "Remove").build()
        }
    }

    /// The thermostat, with the grid's status line above its readings, so
    /// "Setting 21.5°…" takes the place of "Updated 14:39." rather than
    /// pushing Cooler and Warmer down between two taps.
    fn climate_screen(&self, id: &str) -> Screen {
        let mut s = ScreenBuilder::new("homepanel-climate").top_bar(self.name_of(id));
        if let Some(line) = self.status_line() {
            s = s.secondary(line);
        }
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
        self.with_dialog(s)
            .buttons([("cool", "Cooler"), ("warm", "Warmer"), ("power", "Power")])
            .build()
    }

    fn detail_screen(&self, id: &str) -> Screen {
        let s = ScreenBuilder::new("homepanel-reading")
            .top_bar(self.name_of(id))
            .text(format!(
                "State: {}",
                self.reading_of(id)
                    .unwrap_or_else(|| "Not connected".to_owned())
            ))
            .text(id)
            .secondary("This tile only shows its reading. It is not switched from here.");
        Self::say(s, self.banner.as_ref()).build()
    }

    fn picker_prefix(&self) -> ScreenBuilder {
        let screen = ScreenBuilder::new("homepanel-add")
            .top_bar("Add a tile")
            .top_bar_glyph(SEARCH, "Search", Glyph::Search);
        Self::say(screen, self.banner.as_ref())
    }

    fn picker_rows(&self) -> Vec<(String, String, String, Glyph)> {
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
        rows
    }

    fn picker_pages(&self, context: &Context) -> Vec<Vec<usize>> {
        let rows = self.picker_rows();
        let borrowed = rows
            .iter()
            .map(|(_, title, detail, _)| (title.as_str(), detail.as_str()))
            .collect::<Vec<_>>();
        context.paginate_rows_under(
            &borrowed,
            false,
            Position::AtTheFoot,
            &self.picker_prefix().build(),
        )
    }

    fn add(&self, context: &Context) -> Screen {
        let screen = self.picker_prefix();
        if self.task.is_some_and(|(_, kind)| kind == "entities") {
            return screen.activity("Finding devices", None).build();
        }
        let rows = self.picker_rows();
        if rows.is_empty() && self.entities_failed && self.query.is_empty() {
            // The banner says why the list is missing. A device can still be
            // added by its entity ID, which is the way on from here.
            return screen
                .splash(
                    Some(Glyph::Search),
                    "Add a device by its ID",
                    "Tap Search and type its entity ID from Home Assistant, like light.kitchen.",
                )
                .build();
        }
        if rows.is_empty() && self.entities_failed {
            return screen
                .splash(
                    Some(Glyph::Search),
                    "No device with that ID",
                    "An entity ID is written with a dot, like light.kitchen.",
                )
                .build();
        }
        if rows.is_empty() {
            return screen
                .splash(
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
                .build();
        }
        let pages = self.picker_pages(context);
        let page = self.picker_page.min(pages.len().saturating_sub(1));
        Self::page_controls(
            screen.rows(pages[page].iter().map(|&index| rows[index].clone())),
            page,
            pages.len(),
        )
        .build()
    }

    fn search(&self) -> Screen {
        ScreenBuilder::new("homepanel-search")
            .top_bar("Search devices")
            .text_entry(&self.search_entry, "Name or entity", "Search")
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

    fn save_labels(&self, context: &mut Context) {
        let saved = self
            .labels
            .iter()
            .map(|(id, label)| format!("{id}\t{}\t{}", label.name, label.unit))
            .collect::<Vec<_>>()
            .join("\n");
        context.store().save(LABELS, saved.into_bytes());
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

    /// Takes the address typed on setup: saved and tested when it is HTTPS,
    /// and otherwise kept in the field with the reason under it.
    fn submit_address(&mut self, context: &mut Context, text: String) {
        if text.starts_with("https://") {
            self.base = text;
            self.draft.clone_from(&self.base);
            self.address_error = None;
            context.store().save(BASE, self.base.clone().into_bytes());
            self.test(context);
        } else {
            self.draft = text;
            // The field now says what is wrong, so a banner saying no address
            // has arrived is no longer the answer to anything.
            if self.banner_text() == Some(NO_ADDRESS_YET) {
                self.banner = None;
            }
            self.address_error = Some("Use an https:// Home Assistant URL.".into());
            self.show(context);
        }
    }

    fn test(&mut self, context: &mut Context) {
        if let Some(id) = context.spawn(ha::test_connection(&self.base)) {
            self.task = Some((id, "test"));
            self.banner = Some((BannerLevel::Info, "Testing connection…".into()));
            self.show(context);
        }
    }

    fn load_entities(&mut self, context: &mut Context) {
        self.view = View::Add;
        self.query.clear();
        self.picker_page = 0;
        self.banner = None;
        self.entities_failed = false;
        if let Some(id) = context.spawn(ha::entities(&self.base)) {
            self.task = Some((id, "entities"));
        } else {
            self.banner = Some((
                BannerLevel::Attention,
                "Device list is busy. Try again in a moment.".into(),
            ));
        }
        self.show(context);
    }

    fn set_climate(&mut self, context: &mut Context, id: &str, delta: f64) {
        let climate = self.climate_of(id);
        let Some(target) = climate.target else {
            self.note = Some("Home Assistant has not reported a target temperature yet.".into());
            self.show(context);
            return;
        };
        let next = ((target + delta) * 2.0).round() / 2.0;
        if let Some(task) = context.spawn(ha::set_temperature(&self.base, id, next)) {
            self.task = Some((task, "climate"));
            self.note = Some(format!("Setting {}°…", format_degrees(next)));
            self.show(context);
        }
    }

    fn on_poll(&mut self, context: &mut Context, bytes: &[u8]) {
        self.states = ha::state_rows(bytes);
        self.climate = ha::climate_rows(bytes);
        if self.learn_labels(ha::label_rows(bytes)) {
            self.save_labels(context);
        }
        self.last_ok = now_minutes().map(|minutes| (minutes, now_hhmm()));
        self.trouble = None;
        self.save_states(context);
        self.save_climate(context);
        self.save_last_ok(context);
        if let Some((id, before)) = self.pending.take() {
            let now = self.state_of(&id).unwrap_or("Not connected").to_owned();
            let name = self.name_of(&id);
            // A tile that moved is the tap done, said in the status line. One
            // that did not is a tap that did not happen, which the owner has
            // to look into in Home Assistant.
            if now == before {
                self.note = None;
                self.dialog = Some((
                    format!("{name} did not change"),
                    format!(
                        "Home Assistant answered, but {name} still reads {now}. Check it in Home Assistant."
                    ),
                ));
            } else {
                self.note = Some(format!("{name} is now {now}."));
            }
        } else {
            // A reading with nothing waiting on it: the line goes back to
            // saying when the readings were taken, so a panel by the door
            // does not say "Desk lamp is now off." for the rest of the day.
            self.note = None;
        }
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
                self.note = Some("Home Panel can show up to 12 tiles.".into());
                self.show(context);
            } else {
                self.load_entities(context);
            }
        } else if action == action_id(SEARCH) {
            self.search_entry.open_with(&self.query);
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
            self.draft.clone_from(&self.base);
            self.address_error = None;
            self.phone = false;
            self.view = View::Setup;
            self.show(context);
        } else if action == action_id("wall") {
            self.wall = !self.wall;
            self.grid_page = 0;
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
        let name = self.name_of(&id);
        if action == action_id("move-up") && index > 0 {
            self.tiles.swap(index - 1, index);
            self.save_tiles(context);
            self.view = View::Edit;
            self.show(context);
        } else if action == action_id("remove") {
            self.tiles.remove(index);
            self.save_tiles(context);
            self.labels.retain(|(known, _)| *known != id);
            self.save_labels(context);
            // No poll follows: nothing about the other tiles has changed, and
            // a poll that failed here used to replace this note with an outage.
            self.banner = Some((BannerLevel::Success, format!("{name} removed.")));
            self.view = View::Edit;
            self.show(context);
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
                self.note = None;
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
            // Discovery falls back to the entity ID for a device with no
            // friendly name; that is no name, and the tile makes its own.
            let name = if entity.name == entity.id {
                String::new()
            } else {
                entity.name.clone()
            };
            self.learn_labels(vec![(
                entity.id.clone(),
                ha::Label {
                    name,
                    unit: entity.unit.clone(),
                },
            )]);
            self.save_tiles(context);
            self.save_states(context);
            self.save_labels(context);
            self.view = View::Grid;
            self.banner = None;
            self.show(context);
        }
    }

    fn toggle_tile(&mut self, context: &mut Context, id: &str) {
        let Some(request) = ha::service(&self.base, id) else {
            self.banner = None;
            self.view = View::Detail(id.to_owned());
            self.show(context);
            return;
        };
        if let Some(task) = context.spawn(request) {
            self.task = Some((task, "service"));
            let state = self.state_of(id).unwrap_or_default().to_owned();
            self.note = Some(format!("Updating {}…", self.name_of(id)));
            self.pending = Some((id.to_owned(), state));
            self.show(context);
        }
    }
}

impl KoboApp for HomePanel {
    fn on_start(&mut self, context: &mut Context) {
        self.checking = false;
        context.store().load(BASE);
        context.store().load(TILES);
        context.store().load(STATES);
        context.store().load(CLIMATE);
        context.store().load(LABELS);
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
        } else if key == LABELS {
            self.labels = text
                .lines()
                .filter_map(|line| {
                    let mut parts = line.split('\t');
                    let id = parts.next().filter(|id| !id.is_empty())?;
                    let name = parts.next().unwrap_or_default().to_owned();
                    let unit = parts.next().unwrap_or_default().to_owned();
                    Some((id.to_owned(), ha::Label { name, unit }))
                })
                .collect();
        } else if key == SETTINGS {
            self.wall = text.lines().any(|line| line == "wall=1");
        } else if key == LASTOK {
            self.last_ok = text
                .split_once('|')
                .and_then(|(minutes, hhmm)| Some((minutes.parse::<i64>().ok()?, hhmm.to_owned())));
        }
        if key == BASE && self.view == View::Setup {
            // An address written by `kobo connect homepanel` while setup was
            // on screen: the panel opens without anything typed here.
            if self.base.is_empty() {
                if std::mem::take(&mut self.checking) {
                    self.banner = Some((BannerLevel::Attention, NO_ADDRESS_YET.into()));
                    self.show(context);
                }
            } else {
                self.checking = false;
                self.banner = None;
                self.view = View::Grid;
                self.show(context);
                self.fetch(context);
                self.poll_clock.start(context);
            }
            return;
        }
        if self.view == View::Opening {
            if key == BASE {
                // Wait for the rest of the store before choosing a screen.
                return;
            }
            self.view = if self.base.is_empty() {
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
        // The dialogue is closed before anything else, by its button or by
        // Back, which would otherwise leave the screen under it.
        if self.dialog.is_some() && (action == action_id(CLOSE_DIALOG) || action == ActionId::BACK)
        {
            self.dialog = None;
            self.show(context);
            return;
        }
        if action == ActionId::BACK && self.view == View::Setup && self.phone {
            self.phone = false;
            self.show(context);
            return;
        }
        if action == ActionId::BACK {
            self.back(context);
            return;
        }
        if self.view == View::Setup && action == action_id(CHECK_AGAIN) {
            self.banner = None;
            self.checking = true;
            context.store().load(BASE);
            return;
        }
        if self.view == View::Setup {
            if let Some(typing) = self.address.handle(action) {
                if let Typing::Submitted(text) = typing {
                    self.submit_address(context, text);
                } else {
                    self.show(context);
                }
                return;
            }
            if action == action_id(HOME_URL) {
                self.phone = false;
                // An empty field opens on the scheme every address needs.
                self.address.open_with(if self.draft.is_empty() {
                    "https://"
                } else {
                    &self.draft
                });
                self.show(context);
                return;
            }
            if action == action_id(PHONE) {
                self.phone = true;
                self.show(context);
                return;
            }
        }
        if self.view == View::Search {
            match self.search_entry.handle(action) {
                Some(Typing::Submitted(text)) => {
                    self.query = text;
                    self.picker_page = 0;
                    self.view = View::Add;
                    self.show(context);
                    return;
                }
                Some(Typing::Cancelled) => {
                    self.view = View::Add;
                    self.show(context);
                    return;
                }
                Some(_) => {
                    self.show(context);
                    return;
                }
                None => {}
            }
        }
        if action == action_id("page-previous") || action == action_id("page-next") {
            let (page, count) = match self.view {
                View::Grid => (self.grid_page, self.grid_pages(context).len()),
                View::Add => (self.picker_page, self.picker_pages(context).len()),
                _ => return,
            };
            let last = count.saturating_sub(1);
            let page = page.min(last);
            let next = if action == action_id("page-next") {
                page.saturating_add(1).min(last)
            } else {
                page.saturating_sub(1)
            };
            if self.view == View::Grid {
                self.grid_page = next;
            } else {
                self.picker_page = next;
            }
            self.show(context);
            return;
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
        let Some((known, kind)) = self.task else {
            return;
        };
        if known != task {
            return;
        }
        // A newer request may supersede the tracked UI operation while the
        // earlier request is still running. Its reply must not consume this one.
        self.task = None;
        match (kind, outcome) {
            ("test", TaskOutcome::Completed(_)) => {
                self.banner = None;
                self.view = View::Grid;
                self.show(context);
                self.fetch(context);
                self.poll_clock.start(context);
            }
            ("test", TaskOutcome::Failed(error)) => {
                self.fail(None, error, None);
                self.show(context);
            }
            ("poll", TaskOutcome::Completed(bytes)) => {
                self.on_poll(context, &bytes);
            }
            ("poll", TaskOutcome::Failed(error)) => {
                if let Some((id, _)) = self.pending.take() {
                    self.action_failed(format!("Couldn't confirm {}", self.name_of(&id)), error);
                } else {
                    // A look that nobody tapped for is said in the status
                    // line, beside the time of the readings still on show.
                    self.trouble = Some(failure(&self.base, error).cause);
                    self.note = None;
                }
                self.show(context);
            }
            ("entities", TaskOutcome::Completed(bytes)) => {
                self.entities = ha::entity_rows(&bytes);
                self.banner = if self.entities.is_empty() {
                    Some((
                        BannerLevel::Info,
                        "Home Assistant returned no devices.".into(),
                    ))
                } else {
                    None
                };
                self.show(context);
            }
            ("entities", TaskOutcome::Failed(error)) => {
                // The picker stays open: an entity ID typed into Search can
                // still be added, and going back to the grid hid that way.
                self.entities_failed = true;
                self.fail(None, error, None);
                self.show(context);
            }
            ("service", TaskOutcome::Completed(_)) => {
                self.note = Some("Updated. Checking status…".into());
                self.show(context);
                self.fetch(context);
            }
            ("service", TaskOutcome::Failed(error)) => {
                let name = self
                    .pending
                    .take()
                    .map_or_else(|| "that device".to_owned(), |(id, _)| self.name_of(&id));
                self.action_failed(format!("Couldn't update {name}"), error);
                self.show(context);
            }
            ("climate", TaskOutcome::Completed(_)) => {
                self.fetch(context);
            }
            ("climate", TaskOutcome::Failed(error)) => {
                self.action_failed("Couldn't set the temperature".into(), error);
                self.show(context);
            }
            _ => {}
        }
    }

    fn on_background(&mut self, context: &mut Context) {
        self.poll_clock.stop(context);
    }

    fn on_foreground(&mut self, context: &mut Context) {
        self.returned(context);
    }

    fn on_resume(&mut self, context: &mut Context) {
        self.returned(context);
    }
}

impl HomePanel {
    /// Back steps up one screen. Landing on the grid looks at Home Assistant
    /// again, as the grid's own way back does.
    fn back(&mut self, context: &mut Context) {
        self.view = match &self.view {
            View::Search => View::Add,
            View::Tile(_) => View::Edit,
            View::Edit => View::Settings,
            _ => View::Grid,
        };
        self.show(context);
        if self.view == View::Grid {
            self.fetch(context);
        }
    }

    /// Coming back to the front: the grid refreshes, and setup looks for an
    /// address `kobo connect homepanel` may have written meanwhile.
    fn returned(&mut self, context: &mut Context) {
        if self.view == View::Grid {
            self.poll_clock.start(context);
            self.fetch(context);
        } else if self.view == View::Setup {
            context.store().load(BASE);
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

    fn test_context() -> Context {
        kobo_sdk::AppRunner::new(HomePanel::default()).context()
    }

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
        assert!(!app.grid(&test_context()).layout().nodes.is_empty());
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

    /// Picked from the list as "Desk lamp", the tile used to read "desk",
    /// and a temperature sensor's tile "outside · 12.3" with no degrees.
    #[test]
    fn tiles_go_by_their_friendly_names_and_readings_carry_their_units() {
        let mut app = panel();
        app.tiles
            .extend(["sensor.outside".into(), "sensor.humidity".into()]);
        app.states.extend([
            ("sensor.outside".into(), "12.3".into()),
            ("sensor.humidity".into(), "48".into()),
        ]);
        app.on_poll(
            &mut test_context(),
            br#"[
                {"id":"light.desk","s":"on","a":{"unit":null,"name":"Desk lamp"}},
                {"id":"climate.bedroom","s":"heat","a":{"ct":19.5,"t":21,"name":"Bedroom"}},
                {"id":"sensor.outside","s":"12.3","a":{"unit":"\u00b0C","name":"Outside"}},
                {"id":"sensor.humidity","s":"unavailable","a":{"unit":"%","name":"Humidity"}}
            ]"#,
        );
        assert_eq!(app.tile_label("light.desk"), "Desk lamp · on");
        assert_eq!(app.tile_label("climate.bedroom"), "Bedroom · 19.5°");
        assert_eq!(app.tile_label("sensor.outside"), "Outside · 12.3 °C");
        assert_eq!(app.tile_label("sensor.humidity"), "Humidity · unavailable");
        app.states.retain(|(id, _)| id != "sensor.humidity");
        app.states.push(("sensor.humidity".into(), "48".into()));
        assert_eq!(app.tile_label("sensor.humidity"), "Humidity · 48%");
        assert!(format!("{:?}", app.detail_screen("sensor.outside")).contains("State: 12.3 °C"));
        // A poll from a Home Assistant that has no name for a tile keeps
        // the one it was added under.
        app.on_poll(
            &mut test_context(),
            br#"[{"id":"light.desk","s":"off","a":{"unit":null,"name":null}}]"#,
        );
        assert_eq!(app.tile_label("light.desk"), "Desk lamp · off");
    }

    #[test]
    fn a_device_picked_from_the_list_keeps_its_name_and_unit_from_the_first_draw() {
        let mut runner = kobo_sdk::AppRunner::new(HomePanel {
            view: View::Add,
            entities: vec![
                ha::Entity {
                    id: "sensor.outside".into(),
                    name: "Outside".into(),
                    state: "12.3".into(),
                    unit: "°C".into(),
                },
                ha::Entity {
                    id: "switch.no_name".into(),
                    name: "switch.no_name".into(),
                    state: "off".into(),
                    unit: String::new(),
                },
            ],
            ..panel()
        });
        runner.action(action_id("entity.sensor.outside"));
        assert_eq!(
            runner.app().tile_label("sensor.outside"),
            "Outside · 12.3 °C"
        );
        runner.app_mut().view = View::Add;
        runner.action(action_id("entity.switch.no_name"));
        assert_eq!(runner.app().tile_label("switch.no_name"), "no name · off");
    }

    /// A panel saved before names were kept has a tile list and no labels.
    /// Its tiles load as they always did and are named from their IDs.
    #[test]
    fn tiles_saved_before_names_were_kept_still_load_under_their_old_names() {
        let mut app = HomePanel::default();
        let mut context = test_context();
        for (key, value) in [
            (BASE, "https://ha.example"),
            (TILES, "light.desk\nsensor.outside"),
            (STATES, "light.desk\ton\nsensor.outside\t12.3"),
            (LABELS, ""),
        ] {
            app.on_store(
                &mut context,
                StoreResult::Loaded {
                    key: key.to_owned(),
                    value: Some(value.as_bytes().to_vec()),
                },
            );
        }
        assert_eq!(app.tiles.len(), 2);
        assert_eq!(app.tile_label("light.desk"), "desk · on");
        assert_eq!(app.tile_label("sensor.outside"), "outside · 12.3");
        app.on_store(
            &mut context,
            StoreResult::Loaded {
                key: LABELS.to_owned(),
                value: Some(
                    b"light.desk\tDesk lamp\t\nsensor.outside\tOutside\t\xc2\xb0C".to_vec(),
                ),
            },
        );
        assert_eq!(app.tile_label("light.desk"), "Desk lamp · on");
        assert_eq!(app.tile_label("sensor.outside"), "Outside · 12.3 °C");
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
        // A reading from another day names the day.
        assert!(
            stale.starts_with("Offline since 1 Jan 1970 at 22:18"),
            "{stale}"
        );
    }

    #[test]
    fn failure_messages_name_the_fix() {
        let base = "https://ha.example";
        assert!(failure(base, TaskError::NoCredential)
            .message
            .contains("kobo connect homepanel"));
        assert!(failure(base, TaskError::Offline).message.contains("Wi-Fi"));
        assert!(failure(base, TaskError::Unreachable)
            .message
            .contains("did not answer"));
        assert!(failure(base, TaskError::Unauthorized)
            .message
            .contains("long-lived access token"));
        assert!(!failure(base, TaskError::Unauthorized)
            .message
            .contains("address"));
        assert!(
            failure("https://homeassistant.local:8123", TaskError::Unreachable)
                .message
                .contains("home network")
        );
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
        let debug = format!("{:?}", app.grid(&test_context()));
        assert!(debug.contains("columns: 1"), "{debug}");
    }

    #[test]
    fn empty_grid_offers_add_and_settings_as_header_icons() {
        let debug = format!("{:?}", HomePanel::default().grid(&test_context()));
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
                    unit: String::new(),
                },
                ha::Entity {
                    id: "sensor.office".into(),
                    name: "Office temperature".into(),
                    state: "21".into(),
                    unit: String::new(),
                },
            ],
            ..HomePanel::default()
        };
        let debug = format!("{:?}", app.add(&test_context()));
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
        let debug = format!("{:?}", app.add(&test_context()));
        assert!(debug.contains("Add anyway"), "{debug}");
        assert!(valid_entity_id("light.kitchen"));
        assert!(!valid_entity_id("Kitchen light"));
    }

    #[test]
    fn a_device_list_that_fails_leaves_the_picker_open_on_adding_by_id() {
        let mut runner = kobo_sdk::AppRunner::new(panel());
        runner.action(action_id(ADD));
        let (task, _) = runner.app().task.expect("the device list was asked for");
        runner.task_outcome(task, TaskOutcome::Failed(TaskError::Unauthorized));
        assert!(runner.app().view == View::Add, "the picker closed");
        let drawn = format!("{:?}", runner.app().add(&test_context()));
        assert!(drawn.contains("entity ID"), "{drawn}");
        runner.app_mut().query = "light.kitchen".into();
        let drawn = format!("{:?}", runner.app().add(&test_context()));
        assert!(drawn.contains("Add anyway"), "{drawn}");
    }

    #[test]
    fn settings_and_edit_tiles_carry_only_a_note_about_a_change_made_there() {
        let mut app = panel();
        app.banner = Some((
            BannerLevel::Attention,
            "Home Assistant did not answer.".into(),
        ));
        assert!(!format!("{:?}", app.settings()).contains("did not answer"));
        assert!(!format!("{:?}", app.edit()).contains("did not answer"));
        app.banner = Some((BannerLevel::Success, "fan removed.".into()));
        assert!(format!("{:?}", app.edit()).contains("fan removed."));
    }

    #[test]
    fn a_removed_tile_is_still_acknowledged_when_a_poll_fails_off_the_grid() {
        let mut runner = kobo_sdk::AppRunner::new(HomePanel {
            view: View::Tile(1),
            ..panel()
        });
        let commands = runner.action(action_id("remove"));
        assert!(
            !commands
                .iter()
                .any(|command| matches!(command, kobo_sdk::Command::Spawn { .. })),
            "removing a tile polled Home Assistant"
        );
        assert_eq!(runner.app().banner_text(), Some("fan removed."));
        // A poll the grid started earlier fails while Edit tiles is showing.
        let mut context = test_context();
        runner.app_mut().fetch(&mut context);
        let (task, _) = runner.app().task.expect("a poll is in flight");
        runner.task_outcome(task, TaskOutcome::Failed(TaskError::Unreachable));
        assert_eq!(runner.app().banner_text(), Some("fan removed."));
    }

    /// Where each tile, or each of the climate screen's buttons, sits.
    fn places(screen: &Screen, actions: &[&str]) -> Vec<kobo_ui::Rect> {
        let layout = screen.layout_with(
            &kobo_sdk::CLARA_BW_METRICS,
            &kobo_sdk::Chrome::measuring(true),
        );
        actions
            .iter()
            .map(|action| layout.rect_of_action(action_id(action)).expect(action))
            .collect()
    }

    const TILES_ON_GRID: [&str; 3] = ["tile.light.desk", "tile.switch.fan", "tile.climate.bedroom"];

    /// Every tap used to push a banner over the grid, so the tiles jumped a
    /// banner's height under the finger. Each step of a tap is said in the
    /// status line instead, in one line, and no tile moves.
    #[test]
    fn a_tap_on_a_tile_is_said_in_the_status_line_and_no_tile_moves() {
        let mut runner = kobo_sdk::AppRunner::new(HomePanel {
            last_ok: Some((now_minutes().unwrap_or(0), "14:39".to_owned())),
            ..panel()
        });
        let context = test_context();
        let first = places(&runner.app().grid(&context), &TILES_ON_GRID);
        let mut said = vec![runner.app().status_line().expect("a status line")];
        runner.action(action_id("tile.light.desk"));
        said.push(runner.app().status_line().expect("a status line"));
        assert_eq!(places(&runner.app().grid(&context), &TILES_ON_GRID), first);
        let (service, _) = runner.app().task.expect("the switch went out");
        runner.task_outcome(service, TaskOutcome::Completed(Vec::new()));
        said.push(runner.app().status_line().expect("a status line"));
        let (poll, _) = runner.app().task.expect("the confirming look");
        runner.task_outcome(
            poll,
            TaskOutcome::Completed(br#"[{"id":"light.desk","s":"off"}]"#.to_vec()),
        );
        said.push(runner.app().status_line().expect("a status line"));
        assert_eq!(places(&runner.app().grid(&context), &TILES_ON_GRID), first);
        assert_eq!(
            said[1..],
            [
                "Updating desk…".to_owned(),
                "Updated. Checking status…".to_owned(),
                "desk is now off.".to_owned()
            ]
        );
        assert!(said[0].starts_with("Updated "), "{said:?}");
        assert_eq!(runner.app().banner, None);
        // The next look with nothing waiting on it says when it was taken.
        runner.app_mut().fetch(&mut test_context());
        let (poll, _) = runner.app().task.expect("a look");
        runner.task_outcome(
            poll,
            TaskOutcome::Completed(br#"[{"id":"light.desk","s":"off"}]"#.to_vec()),
        );
        let line = runner.app().status_line().expect("a status line");
        assert!(line.starts_with("Updated "), "{line}");
    }

    /// "Setting 21.5°…" landed between the readings and Cooler and Warmer,
    /// moving them between two taps. It takes the status line's place now.
    #[test]
    fn the_climate_screen_says_what_it_is_setting_without_moving_its_buttons() {
        let mut runner = kobo_sdk::AppRunner::new(HomePanel {
            last_ok: Some((now_minutes().unwrap_or(0), "14:39".to_owned())),
            ..panel()
        });
        runner.action(action_id("tile.climate.bedroom"));
        let buttons = ["cool", "warm", "power"];
        let before = places(&runner.app().climate_screen("climate.bedroom"), &buttons);
        let drawn = format!("{:?}", runner.app().climate_screen("climate.bedroom"));
        assert!(drawn.contains("Updated 14:39."), "{drawn}");
        runner.action(action_id("warm"));
        let screen = runner.app().climate_screen("climate.bedroom");
        assert!(format!("{screen:?}").contains("Setting 21.5°…"));
        assert!(!screen
            .nodes
            .iter()
            .any(|node| matches!(node, kobo_sdk::Node::Banner { .. })));
        assert_eq!(places(&screen, &buttons), before);
    }

    /// A tap that did not happen needs reading, so it is a dialogue with one
    /// button that closes it, over tiles that have not moved.
    #[test]
    fn a_failed_action_opens_a_dialogue_with_one_way_out() {
        let mut runner = kobo_sdk::AppRunner::new(HomePanel {
            last_ok: Some((now_minutes().unwrap_or(0), "14:39".to_owned())),
            ..panel()
        });
        let context = test_context();
        let first = places(&runner.app().grid(&context), &TILES_ON_GRID);
        runner.action(action_id("tile.light.desk"));
        let (service, _) = runner.app().task.expect("the switch went out");
        runner.task_outcome(service, TaskOutcome::Failed(TaskError::Unreachable));
        let screen = runner.app().grid(&context);
        let overlay = screen.overlay.as_ref().expect("a dialogue");
        assert_eq!(overlay.title, "Couldn't update desk");
        assert!(format!("{overlay:?}").contains("did not answer"));
        assert_eq!(places(&screen, &TILES_ON_GRID), first);
        // The status line agrees with it rather than saying "Updated".
        assert_eq!(
            runner.app().status_line().as_deref(),
            Some("Home Assistant did not answer. Readings from 14:39.")
        );
        // Back closes the dialogue and leaves the grid where it is.
        runner.action(ActionId::BACK);
        assert!(runner.app().dialog.is_none());
        assert!(runner.app().view == View::Grid);
        // A tile that did not change is a tap that did not happen too.
        runner.action(action_id("tile.switch.fan"));
        let (service, _) = runner.app().task.expect("the switch went out");
        runner.task_outcome(service, TaskOutcome::Completed(Vec::new()));
        let (poll, _) = runner.app().task.expect("the confirming look");
        runner.task_outcome(
            poll,
            TaskOutcome::Completed(br#"[{"id":"switch.fan","s":"off"}]"#.to_vec()),
        );
        let (title, _) = runner.app().dialog.clone().expect("a dialogue");
        assert_eq!(title, "fan did not change");
        runner.action(action_id(CLOSE_DIALOG));
        assert!(runner.app().dialog.is_none());
    }

    /// A Home Assistant that stopped answering used to be explained in a
    /// banner under "Updated 14:39.", which said the opposite. The status
    /// line says both: that it did not answer, and when the readings are from.
    #[test]
    fn a_home_assistant_that_stops_answering_is_said_beside_the_time_of_the_readings() {
        let mut runner = kobo_sdk::AppRunner::new(HomePanel {
            last_ok: Some((now_minutes().unwrap_or(0), "14:39".to_owned())),
            ..panel()
        });
        runner.app_mut().fetch(&mut test_context());
        let (poll, _) = runner.app().task.expect("a look");
        runner.task_outcome(poll, TaskOutcome::Failed(TaskError::Unreachable));
        let context = test_context();
        let drawn = format!("{:?}", runner.app().grid(&context));
        assert!(
            drawn.contains("Home Assistant did not answer. Readings from 14:39."),
            "{drawn}"
        );
        assert!(!drawn.contains("Updated 14:39."), "{drawn}");
        assert!(!drawn.contains("Banner"), "{drawn}");
        assert!(runner.app().dialog.is_none(), "nobody tapped anything");
        runner.app_mut().fetch(&mut test_context());
        let (poll, _) = runner.app().task.expect("a look");
        runner.task_outcome(
            poll,
            TaskOutcome::Completed(br#"[{"id":"light.desk","s":"on"}]"#.to_vec()),
        );
        let line = runner.app().status_line().expect("a status line");
        assert!(line.starts_with("Updated "), "{line}");
    }

    /// The status line is one line for everything it says at the size most
    /// people read at, so its changes never move the tiles.
    #[test]
    fn everything_the_status_line_says_fits_on_one_line() {
        let mut app = HomePanel {
            last_ok: Some((now_minutes().unwrap_or(0), "14:39".to_owned())),
            ..panel()
        };
        let mut lines = vec![
            "Updating living room ceiling light…".to_owned(),
            "Home Panel can show up to 12 tiles.".to_owned(),
            "Home Assistant has not reported a target temperature yet.".to_owned(),
        ];
        for error in [
            TaskError::Unreachable,
            TaskError::Offline,
            TaskError::TimedOut,
            TaskError::Unauthorized,
            TaskError::Denied,
            TaskError::RateLimited(60),
            TaskError::NoCredential,
            TaskError::NotFound,
            TaskError::TooLarge,
        ] {
            app.trouble = Some(failure(&app.base, error).cause);
            lines.push(app.freshness().expect("a line"));
        }
        for line in lines {
            let layout = ScreenBuilder::new("homepanel-grid")
                .top_bar("Home Panel")
                .secondary(&line)
                .build()
                .layout_with(
                    &kobo_sdk::CLARA_BW_METRICS,
                    &kobo_sdk::Chrome::measuring(true),
                );
            let wrapped = layout
                .nodes
                .iter()
                .find(|node| {
                    node.text_lines
                        .first()
                        .is_some_and(|first| line.starts_with(first.as_str()))
                })
                .map_or(0, |node| node.text_lines.len());
            assert_eq!(wrapped, 1, "{line}");
        }
    }

    #[test]
    fn a_refused_address_takes_the_no_address_yet_banner_down() {
        let mut runner = kobo_sdk::AppRunner::new(HomePanel {
            view: View::Setup,
            banner: Some((BannerLevel::Attention, NO_ADDRESS_YET.into())),
            ..HomePanel::default()
        });
        runner.action(action_id(HOME_URL));
        runner.type_text("http://ha.example.net");
        assert_eq!(runner.app().banner_text(), None);
        assert!(runner.app().address_error.is_some());
    }

    #[test]
    fn an_old_reading_is_dated_and_a_reading_from_today_is_not() {
        // 6 November 2026, 09:00 UTC.
        let now = Snapshot {
            unix_millis: 1_793_955_600_000,
            monotonic_millis: 0,
            utc_offset_minutes: 0,
        };
        let minutes = |unix_seconds: i64| unix_seconds / 60;
        assert_eq!(
            reading_time(minutes(1_793_948_400), "07:00", Some(now)),
            "07:00"
        );
        assert_eq!(
            reading_time(minutes(1_793_862_000), "07:00", Some(now)),
            "yesterday at 07:00"
        );
        assert_eq!(
            reading_time(minutes(1_788_244_800), "06:40", Some(now)),
            "1 Sep at 06:40"
        );
        assert_eq!(
            reading_time(minutes(1_756_708_800), "06:40", Some(now)),
            "1 Sep 2025 at 06:40"
        );
        assert_eq!(reading_time(0, "06:40", None), "06:40");
    }
}

#[cfg(test)]
mod ui_review_tests;

#[cfg(test)]
mod large_text_tests {
    use super::*;

    fn panels() -> impl Iterator<Item = kobo_sdk::DisplayMetrics> {
        [(1072, 1448, 300), (1264, 1680, 300), (1404, 1872, 227)]
            .into_iter()
            .flat_map(|(width, height, pixels_per_inch)| {
                [kobo_ui::TextScale::Default, kobo_ui::TextScale::Largest]
                    .into_iter()
                    .map(move |text_scale| kobo_sdk::DisplayMetrics {
                        width,
                        height,
                        pixels_per_inch,
                        text_scale,
                    })
            })
    }
    fn fits(screen: &Screen, metrics: kobo_sdk::DisplayMetrics) {
        let diagnostics = screen.diagnostics(&metrics, &kobo_sdk::Chrome::measuring(true));
        assert!(
            !diagnostics.has_errors(),
            "{metrics:?}: {:#?}",
            diagnostics.issues
        );
    }

    #[test]
    fn setup_instructions_and_keyboard_are_complete_at_large_text() {
        for metrics in panels() {
            let runner = kobo_sdk::AppRunner::with_metrics(HomePanel::default(), metrics);
            let screen = runner.app().setup(&runner.context());
            fits(&screen, metrics);
            let layout = screen.layout_with(&metrics, &kobo_sdk::Chrome::measuring(true));
            let text = layout
                .nodes
                .iter()
                .flat_map(|node| node.text_lines.iter())
                .cloned()
                .collect::<Vec<_>>()
                .join(" ");
            assert!(text.contains("kobo connect homepanel"), "{text}");
            assert!(text.contains("must be on HTTPS."), "{text}");
            assert!(layout.rect_of_action(action_id(HOME_URL)).is_some());
        }
    }

    #[test]
    fn every_setup_state_fits_at_every_size_held_either_way() {
        // Held sideways too, at every size: the connect screen gives up its
        // mark and then its heading before it would run off the panel.
        let clara =
            kobo_ui::TextScale::STEPS
                .into_iter()
                .map(|text_scale| kobo_sdk::DisplayMetrics {
                    text_scale,
                    ..kobo_sdk::CLARA_BW_METRICS
                });
        for portrait in panels().chain(clara) {
            let landscape = kobo_sdk::DisplayMetrics {
                width: portrait.height,
                height: portrait.width,
                ..portrait
            };
            for metrics in [portrait, landscape] {
                let runner = kobo_sdk::AppRunner::with_metrics(HomePanel::default(), metrics);
                let context = runner.context();
                for (draft, error, banner, phone) in [
                    ("https://ha.example.net", None, None, false),
                    (
                        "http://ha.example.net",
                        Some("Use an https:// Home Assistant URL."),
                        None,
                        false,
                    ),
                    (
                        "",
                        None,
                        Some((BannerLevel::Attention, NO_ADDRESS_YET.to_owned())),
                        false,
                    ),
                    (
                        "https://homeassistant.local:8123",
                        None,
                        Some((
                            BannerLevel::Error,
                            failure("https://homeassistant.local:8123", TaskError::Unauthorized)
                                .message,
                        )),
                        false,
                    ),
                    ("https://homeassistant.local:8123", None, None, true),
                ] {
                    let app = HomePanel {
                        draft: draft.into(),
                        address_error: error.map(Into::into),
                        banner,
                        phone,
                        ..HomePanel::default()
                    };
                    let screen = app.setup(&context).with_own_back(phone);
                    fits(&screen, metrics);
                    if phone {
                        continue;
                    }
                    let layout = screen.layout_with(&metrics, &kobo_sdk::Chrome::measuring(true));
                    let text = layout
                        .nodes
                        .iter()
                        .flat_map(|node| node.text_lines.iter())
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(" ");
                    assert!(
                        text.contains("kobo connect homepanel"),
                        "{metrics:?}: {text}"
                    );
                    assert!(
                        text.contains("must be on HTTPS.")
                            || error.is_some()
                            || draft.starts_with("https://"),
                        "{metrics:?}: {text}"
                    );
                    if app.token_page().is_some() {
                        assert!(layout.rect_of_action(action_id(PHONE)).is_some());
                    }
                    assert!(layout.rect_of_action(action_id(CHECK_AGAIN)).is_some());
                    assert!(layout.rect_of_action(action_id(HOME_URL)).is_some());
                }
            }
        }
    }

    /// Where each part of the connect screen is drawn, in reading order.
    fn connect_order(screen: &Screen) -> Vec<&'static str> {
        screen
            .nodes
            .iter()
            .filter_map(|node| match node {
                kobo_sdk::Node::Banner { .. } => Some("banner"),
                kobo_sdk::Node::Splash { .. } | kobo_sdk::Node::Section { .. } => Some("heading"),
                kobo_sdk::Node::Quote { .. } => Some("command"),
                kobo_sdk::Node::Button {
                    action,
                    emphasis: kobo_ui::Emphasis::Primary,
                    ..
                } if *action == action_id(CHECK_AGAIN) => Some("check again"),
                kobo_sdk::Node::Field { .. } => Some("field"),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_connect_screen_reads_banner_then_command_then_check_again_with_nothing_in_the_bar() {
        let context = kobo_sdk::AppRunner::new(HomePanel::default()).context();
        let first = HomePanel::default().setup(&context);
        assert_eq!(
            connect_order(&first),
            ["heading", "command", "check again", "field"],
            "{first:?}"
        );
        assert!(first
            .top_bar
            .as_ref()
            .is_some_and(|bar| bar.actions.is_empty()));
        let checked = HomePanel {
            banner: Some((BannerLevel::Attention, NO_ADDRESS_YET.into())),
            ..HomePanel::default()
        }
        .setup(&context);
        assert_eq!(
            connect_order(&checked),
            ["banner", "heading", "command", "field"],
            "Check again moves onto the banner: {checked:?}"
        );
        assert!(checked.nodes.iter().any(|node| matches!(
            node,
            kobo_sdk::Node::Banner { action: Some(action), .. }
                if action.action == action_id(CHECK_AGAIN)
        )));
    }

    #[test]
    fn the_address_is_typed_on_the_runtime_and_a_wrong_scheme_is_said_under_the_field() {
        let mut runner = kobo_sdk::AppRunner::new(HomePanel {
            view: View::Setup,
            ..HomePanel::default()
        });
        let commands = runner.action(action_id(HOME_URL));
        assert!(commands.iter().any(|command| matches!(
            command,
            kobo_sdk::Command::Device(kobo_sdk::DeviceRequest::TypeText {
                kind: kobo_sdk::FieldKind::Address,
                initial,
                submit,
                ..
            }) if initial == "https://" && submit == "Test connection"
        )));
        runner.type_text("http://ha.example.net");
        let screen = runner.app().setup(&runner.context());
        assert!(
            screen.nodes.iter().any(|node| matches!(
                node,
                kobo_sdk::Node::Field { value, error: Some(error), .. }
                    if value == "http://ha.example.net" && error.contains("https://")
            )),
            "{screen:?}"
        );
        assert!(
            !format!("{screen:?}").contains("Use phone"),
            "no code for an address the reader will not use"
        );
        runner.action(action_id(HOME_URL));
        let commands = runner.type_text("https://ha.example.net/");
        assert!(commands.iter().any(|command| matches!(
            command,
            kobo_sdk::Command::Store(kobo_sdk::StoreRequest::Save { key, .. }) if key == BASE
        )));
    }

    #[test]
    fn the_phone_code_opens_the_profile_page_where_tokens_are_made() {
        let mut runner = kobo_sdk::AppRunner::new(HomePanel {
            view: View::Setup,
            draft: "https://ha.example.net/".into(),
            ..HomePanel::default()
        });
        let commands = runner.action(action_id(PHONE));
        let drawn = format!("{commands:?}");
        assert!(
            drawn.contains("payload: \"https://ha.example.net/profile/security\""),
            "{drawn}"
        );
        runner.action(ActionId::BACK);
        assert!(!runner.app().phone);
        assert!(runner.app().view == View::Setup);
    }

    #[test]
    fn the_wall_panel_is_a_switch_that_flips_on_a_tap() {
        let mut runner = kobo_sdk::AppRunner::new(HomePanel {
            view: View::Settings,
            ..HomePanel::default()
        });
        let toggle = |screen: &Screen| {
            screen.nodes.iter().find_map(|node| match node {
                kobo_sdk::Node::Toggle { label, on, .. } if label == "Wall panel" => Some(*on),
                _ => None,
            })
        };
        assert_eq!(toggle(&runner.app().settings()), Some(false));
        runner.action(action_id("wall"));
        assert!(runner.app().wall);
        assert_eq!(toggle(&runner.app().settings()), Some(true));
    }

    #[test]
    fn settings_show_the_address_as_a_labelled_field_the_tile_count_beside_its_heading_and_no_done()
    {
        let mut runner = kobo_sdk::AppRunner::new(HomePanel {
            view: View::Settings,
            ..panel_for_settings()
        });
        let settings = runner.app().settings();
        assert!(
            settings.nodes.iter().any(|node| matches!(
                node,
                kobo_sdk::Node::Field { value, label: Some(label), kind: FieldKind::Address, .. }
                    if value == "https://ha.example" && label == "Home Assistant address"
            )),
            "{settings:?}"
        );
        assert!(
            settings.nodes.iter().any(|node| matches!(
                node,
                kobo_sdk::Node::Section { title, value: Some(value), .. }
                    if title == "Tiles" && value == "3 of 12"
            )),
            "{settings:?}"
        );
        assert!(
            !format!("{settings:?}").contains("\"Done\""),
            "Back is the way out of settings: {settings:?}"
        );
        runner.action(action_id("change-url"));
        assert!(runner.app().view == View::Setup);
        assert_eq!(runner.app().draft, "https://ha.example");
    }

    #[test]
    fn the_wall_panel_summary_says_how_the_tiles_are_laid_out_now() {
        let summary = |wall: bool| {
            HomePanel {
                wall,
                ..HomePanel::default()
            }
            .settings()
            .nodes
            .into_iter()
            .find_map(|node| match node {
                kobo_sdk::Node::Toggle { summary, .. } => Some(summary),
                _ => None,
            })
        };
        assert_eq!(summary(false).as_deref(), Some("Two tiles per row."));
        assert_eq!(
            summary(true).as_deref(),
            Some("One large tile per row, for a reader by the door.")
        );
    }

    #[test]
    fn settings_fit_a_clara_at_every_text_size_held_either_way_with_a_banner() {
        for text_scale in kobo_ui::TextScale::STEPS {
            for (width, height) in [(1072, 1448), (1448, 1072)] {
                let metrics = kobo_sdk::DisplayMetrics {
                    width,
                    height,
                    text_scale,
                    ..kobo_sdk::CLARA_BW_METRICS
                };
                let _runner = kobo_sdk::AppRunner::with_metrics(HomePanel::default(), metrics);
                for wall in [false, true] {
                    let app = HomePanel {
                        wall,
                        banner: Some((BannerLevel::Success, "fan removed.".into())),
                        ..panel_for_settings()
                    };
                    fits(&app.settings().with_own_back(true), metrics);
                }
            }
        }
    }

    #[test]
    fn a_finished_change_is_a_success_and_a_refused_token_an_error() {
        let mut context = kobo_sdk::AppRunner::new(HomePanel::default()).context();
        let mut app = panel_for_settings();
        app.task = Some((TaskId(4), "service"));
        app.on_task(&mut context, TaskId(4), TaskOutcome::Completed(Vec::new()));
        // A change on the grid is acknowledged in its status line, which
        // keeps its place, rather than a banner over the tiles.
        assert_eq!(app.note.as_deref(), Some("Updated. Checking status…"));
        assert_eq!(app.banner, None);
        app.task = Some((TaskId(5), "test"));
        app.on_task(
            &mut context,
            TaskId(5),
            TaskOutcome::Failed(TaskError::Unauthorized),
        );
        assert_eq!(
            app.banner.as_ref().map(|(level, _)| *level),
            Some(BannerLevel::Error)
        );
        app.task = Some((TaskId(6), "test"));
        app.on_task(
            &mut context,
            TaskId(6),
            TaskOutcome::Failed(TaskError::Offline),
        );
        assert_eq!(
            app.banner.as_ref().map(|(level, _)| *level),
            Some(BannerLevel::Attention),
            "a panel off the Wi-Fi may come right by itself"
        );
    }

    fn panel_for_settings() -> HomePanel {
        HomePanel {
            base: "https://ha.example".into(),
            tiles: vec![
                "light.desk".into(),
                "switch.fan".into(),
                "climate.bedroom".into(),
            ],
            ..HomePanel::default()
        }
    }

    #[test]
    fn the_tile_screen_keeps_move_up_and_remove_whole_with_back_as_the_way_out() {
        for text_scale in kobo_ui::TextScale::STEPS {
            for (width, height) in [(1072, 1448), (1448, 1072)] {
                let metrics = kobo_sdk::DisplayMetrics {
                    width,
                    height,
                    text_scale,
                    ..kobo_sdk::CLARA_BW_METRICS
                };
                let _runner = kobo_sdk::AppRunner::with_metrics(HomePanel::default(), metrics);
                let screen = panel_for_settings().tile(1);
                fits(&screen.clone().with_own_back(true), metrics);
                let layout = screen.layout_with(&metrics, &kobo_sdk::Chrome::measuring(true));
                let lines = layout
                    .nodes
                    .iter()
                    .flat_map(|node| node.text_lines.iter())
                    .cloned()
                    .collect::<Vec<_>>();
                for label in ["Move up", "Remove"] {
                    assert!(
                        lines.iter().any(|line| line == label),
                        "{text_scale:?} {width}x{height}: {label} broke: {lines:?}"
                    );
                }
                assert!(!lines.iter().any(|line| line == "Done"), "{lines:?}");
            }
        }
    }
}

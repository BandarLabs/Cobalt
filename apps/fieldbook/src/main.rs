//! Fieldbook keeps imported field packs, outing logs and a life list on the
//! reader. Packs arrive over the shelf from the `kobo fieldbook` companion;
//! sightings are recorded locally and never need a connection.
mod shelf;

use kobo_sdk::clock::{Clock, ManualClock, Snapshot, SystemClock};
use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Glyph, KoboApp, Screen, ScreenBuilder,
    ShelfDownload, ShelfProgress, StoreResult,
};
use shelf::{Pack, Species, MANIFEST, MAX_MANIFEST};
use std::process::ExitCode;

const OUTINGS: &str = "outings.v1";
const SIGHTINGS: &str = "sightings.v1";
const EXPORT: &str = "export-checklist.csv";
const MAX_STATE: usize = 256 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
struct Outing {
    id: u64,
    location: String,
    date: String,
    start: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Sighting {
    outing: u64,
    code: String,
    common: String,
    scientific: String,
    count: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum View {
    Home,
    Packs,
    Search,
    Outing,
    Sightings,
    Life,
    Export,
    Detail,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SaveState {
    Idle,
    Saving,
    Failed,
}

#[derive(Default)]
struct Loads {
    manifest: bool,
    state: bool,
    started: bool,
}

struct Fieldbook {
    view: View,
    packs: Vec<Pack>,
    pack_failures: Vec<(String, String)>,
    pack_notice: Option<String>,
    outings: Vec<Outing>,
    sightings: Vec<Sighting>,
    open_outing: Option<u64>,
    pack_pick: Option<usize>,
    detail: Option<Species>,
    deleted: Option<(usize, Sighting)>,
    search: Keyboard,
    query: String,
    location: Keyboard,
    naming_location: bool,
    save: SaveState,
    manifest_load: Option<ShelfDownload>,
    loads: Loads,
}

impl Default for Fieldbook {
    fn default() -> Self {
        Self {
            view: View::Home,
            packs: Vec::new(),
            pack_failures: Vec::new(),
            pack_notice: None,
            outings: Vec::new(),
            sightings: Vec::new(),
            open_outing: None,
            pack_pick: None,
            detail: None,
            deleted: None,
            search: Keyboard::new(),
            query: String::new(),
            location: Keyboard::new(),
            naming_location: false,
            save: SaveState::Idle,
            manifest_load: None,
            loads: Loads::default(),
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

/// Date as MM/DD/YYYY, the shape the checklist import expects.
fn today_string() -> String {
    reader_clock()
        .now()
        .ok()
        .and_then(Snapshot::date)
        .map_or(String::new(), |date| {
            format!("{:02}/{:02}/{:04}", date.month, date.day, date.year)
        })
}

fn now_string() -> String {
    reader_clock()
        .now()
        .ok()
        .and_then(Snapshot::hour_minute)
        .map_or(String::new(), |(hour, minute)| {
            format!("{hour:02}:{minute:02}")
        })
}

fn clean(text: &str) -> String {
    text.split(['|', '\n', '\r', ','])
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_owned()
}

fn species_line(species: &Species) -> String {
    match (species.code.is_empty(), species.scientific.is_empty()) {
        (false, false) => format!("{} · {}", species.code, species.scientific),
        (false, true) => species.code.clone(),
        (true, false) => species.scientific.clone(),
        (true, true) => String::new(),
    }
}

impl Fieldbook {
    fn start_when_ready(&mut self, context: &mut Context) {
        if self.loads.manifest && self.loads.state && !self.loads.started {
            self.loads.started = true;
            self.show(context);
        }
    }

    fn begin_manifest(&mut self, context: &mut Context) {
        let mut load = ShelfDownload::new(MANIFEST).at_most(MAX_MANIFEST);
        load.start(context);
        self.manifest_load = Some(load);
    }

    fn advance_manifest(&mut self, context: &mut Context, result: &StoreResult) -> bool {
        let Some(load) = &mut self.manifest_load else {
            return false;
        };
        match load.advance(context, result) {
            ShelfProgress::Done => {
                let bytes = self.manifest_load.take().expect("active manifest").take();
                match shelf::Shelf::decode(&bytes) {
                    Ok(decoded) => {
                        self.pack_failures = decoded
                            .failures
                            .iter()
                            .map(|failure| (failure.input.clone(), failure.reason.clone()))
                            .collect();
                        self.packs = decoded.packs;
                        self.pack_notice = None;
                    }
                    Err(error) => {
                        self.packs.clear();
                        self.pack_notice = Some(format!("Field packs need re-pushing: {error}"));
                    }
                }
                self.loads.manifest = true;
                self.start_when_ready(context);
                true
            }
            ShelfProgress::Failed(kobo_sdk::StoreError::Missing) => {
                self.manifest_load = None;
                self.loads.manifest = true;
                self.start_when_ready(context);
                true
            }
            ShelfProgress::Failed(_) => {
                self.manifest_load = None;
                self.loads.manifest = true;
                self.pack_notice = Some("The field pack shelf could not be opened.".to_owned());
                self.start_when_ready(context);
                true
            }
            ShelfProgress::Moving { .. } => true,
            ShelfProgress::Elsewhere => false,
        }
    }

    fn persist(&mut self, context: &mut Context) {
        self.save = SaveState::Saving;
        let outings = self
            .outings
            .iter()
            .map(|outing| {
                format!(
                    "{}|{}|{}|{}",
                    outing.id, outing.location, outing.date, outing.start
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        context.store().save(OUTINGS, outings.into_bytes());
        let sightings = self
            .sightings
            .iter()
            .map(|sighting| {
                format!(
                    "{}|{}|{}|{}|{}",
                    sighting.outing,
                    sighting.code,
                    sighting.common,
                    sighting.scientific,
                    sighting.count
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        context.store().save(SIGHTINGS, sightings.into_bytes());
    }

    fn open(&self) -> Option<&Outing> {
        self.open_outing
            .and_then(|id| self.outings.iter().find(|outing| outing.id == id))
    }

    fn outing_species(&self) -> Vec<Species> {
        if let Some(pick) = self.pack_pick {
            if let Some(pack) = self.packs.get(pick) {
                return pack.species.clone();
            }
        }
        let mut seen: Vec<Species> = Vec::new();
        for sighting in &self.sightings {
            if !seen.iter().any(|known| known.code == sighting.code) {
                seen.push(Species {
                    code: sighting.code.clone(),
                    common: sighting.common.clone(),
                    scientific: sighting.scientific.clone(),
                });
            }
        }
        seen
    }

    fn search_results(&self) -> Vec<Species> {
        let needle = self.query.to_lowercase();
        let mut found: Vec<Species> = Vec::new();
        if needle.is_empty() {
            return found;
        }
        for pack in &self.packs {
            for species in &pack.species {
                let matches = needle.is_empty()
                    || species.common.to_lowercase().contains(&needle)
                    || species.scientific.to_lowercase().contains(&needle)
                    || species.code.to_lowercase().contains(&needle);
                if matches && !found.iter().any(|known| known.code == species.code) {
                    found.push(species.clone());
                }
            }
        }
        found
    }

    fn outing_totals(&self, id: u64) -> (usize, u32) {
        let rows: Vec<&Sighting> = self.sightings.iter().filter(|s| s.outing == id).collect();
        (rows.len(), rows.iter().map(|s| u32::from(s.count)).sum())
    }

    fn log_sighting(&mut self, context: &mut Context, species: Species) {
        let Some(outing) = self.open_outing else {
            return;
        };
        if let Some(row) = self
            .sightings
            .iter_mut()
            .find(|s| s.outing == outing && s.code == species.code)
        {
            row.count = row.count.saturating_add(1);
        } else {
            self.sightings.push(Sighting {
                outing,
                code: species.code,
                common: species.common,
                scientific: species.scientific,
                count: 1,
            });
        }
        self.persist(context);
    }

    /// eBird Checklist Format: columns A and B stay empty for the first 14
    /// rows, one checklist per column starting at column C with effort data
    /// by position, species rows from row 15 with common name in A,
    /// scientific name in B, and a count per checklist column.
    fn checklist_csv(&self) -> String {
        let mut out = String::new();
        for row in 0..14 {
            let mut line = String::from(",");
            for outing in &self.outings {
                line.push(',');
                let cell = match row {
                    0 => outing.location.as_str(),
                    3 => outing.date.as_str(),
                    4 => outing.start.as_str(),
                    8 => "Incidental",
                    9 => "1",
                    11 => "Y",
                    _ => "",
                };
                line.push_str(cell);
            }
            out.push_str(&line);
            out.push('\n');
        }
        let mut commons: Vec<&str> = Vec::new();
        for sighting in &self.sightings {
            if !commons.contains(&sighting.common.as_str()) {
                commons.push(&sighting.common);
            }
        }
        for common in commons {
            let scientific = self
                .sightings
                .iter()
                .find(|s| s.common == common)
                .map_or("", |s| s.scientific.as_str());
            let mut line = format!("{common},{scientific}");
            for outing in &self.outings {
                line.push(',');
                let count: u32 = self
                    .sightings
                    .iter()
                    .filter(|s| s.outing == outing.id && s.common == common)
                    .map(|s| u32::from(s.count))
                    .sum();
                if count > 0 {
                    line.push_str(&count.to_string());
                }
            }
            out.push_str(&line);
            out.push('\n');
        }
        out
    }

    fn export(&mut self, context: &mut Context) {
        self.save = SaveState::Saving;
        context
            .store()
            .save(EXPORT, self.checklist_csv().into_bytes());
    }

    fn tab_bar() -> [(&'static str, &'static str); 5] {
        [
            ("home", "Today"),
            ("packs", "Packs"),
            ("search", "Search"),
            ("life", "Life list"),
            ("export", "Export"),
        ]
    }

    fn top(&self, title: &str) -> ScreenBuilder {
        let mut screen = ScreenBuilder::new("fieldbook").top_bar(title);
        if self.save == SaveState::Failed {
            screen = screen.banner(BannerLevel::Attention, "Not saved. The last write failed.");
        }
        screen
    }

    fn home_screen(&self) -> Screen {
        let mut s = self.top("Fieldbook");
        if let Some(notice) = &self.pack_notice {
            s = s.banner(BannerLevel::Attention, notice.clone());
        }
        if self.packs.is_empty() {
            s = s.secondary(
                "No field pack yet. Push one with `kobo fieldbook` from a computer; logging works without one.",
            );
        } else {
            s = s.secondary(format!(
                "{} pack{}, {} species.",
                self.packs.len(),
                if self.packs.len() == 1 { "" } else { "s" },
                self.packs.iter().map(|p| p.species.len()).sum::<usize>()
            ));
        }
        if let Some(outing) = self.open() {
            let (species, individuals) = self.outing_totals(outing.id);
            s = s.section(format!(
                "Open outing: {} · {} · {} · {} species, {} birds",
                outing.location, outing.date, outing.start, species, individuals
            ));
            s = s.buttons([("resume", "Resume tally"), ("finish", "Finish outing")]);
        } else {
            s = s.buttons([("new-outing", "Start an outing")]);
        }
        let recent: Vec<(String, String, String, Glyph)> = self
            .outings
            .iter()
            .rev()
            .take(6)
            .map(|outing| {
                let (species, _) = self.outing_totals(outing.id);
                (
                    format!("outing-{}", outing.id),
                    format!("{} · {}", outing.location, outing.date),
                    format!("{species} species"),
                    Glyph::Check,
                )
            })
            .collect();
        s.rows(recent)
            .nav_bar(
                Some(0),
                [
                    ("home", "Today"),
                    ("packs", "Packs"),
                    ("search", "Search"),
                    ("life", "Life list"),
                    ("export", "Export"),
                ],
            )
            .build()
    }

    fn packs_screen(&self) -> Screen {
        let mut s = self.top("Field packs");
        if self.packs.is_empty() {
            s = s.splash(
                Some(Glyph::Search),
                "No field pack yet",
                "Push a pack with `kobo fieldbook` from a computer. Sightings logged now stay on this reader and join any pack you import later.",
            );
            if let Some(notice) = &self.pack_notice {
                s = s.banner(BannerLevel::Attention, notice.clone());
            }
            return s.build();
        }
        for (input, reason) in &self.pack_failures {
            s = s.secondary(format!("{input}: {reason}"));
        }
        s.rows(self.packs.iter().take(6).enumerate().map(|(index, pack)| {
            (
                format!("pack-{index}"),
                pack.title.clone(),
                format!(
                    "{} · issued {} · {} species",
                    pack.region,
                    pack.issued,
                    pack.species.len()
                ),
                Glyph::Search,
            )
        }))
        .nav_bar(Some(1), Self::tab_bar())
        .build()
    }

    fn search_screen(&self) -> Screen {
        let mut s = self.top("Search packs");
        if self.packs.is_empty() && self.open_outing.is_none() {
            return s
                .splash(
                    Some(Glyph::Search),
                    "Nothing to search yet",
                    "Push a field pack with `kobo fieldbook` from a computer, then search it here.",
                )
                .build();
        }
        let results = self.search_results();
        if self.query.is_empty() {
            s = s.secondary(if self.packs.is_empty() {
                "Type the species you see.".to_owned()
            } else {
                "Type a name, a banding code, or a scientific name.".to_owned()
            });
        } else if results.is_empty() && !self.packs.is_empty() {
            s = s.secondary(format!(
                "No species in your packs matches “{}”.",
                self.query
            ));
        }
        let mut screen = s
            .typed(&self.search, "Name, code, or scientific name")
            .keyboard(&self.search, "Find")
            .rows(results.iter().take(3).enumerate().map(|(index, species)| {
                (
                    format!("found-{index}"),
                    species.common.clone(),
                    species_line(species),
                    Glyph::Search,
                )
            }));
        if !self.query.is_empty() && self.open_outing.is_some() {
            screen = screen.button("log-manual", format!("Log “{}” by name", self.query));
        }
        screen.nav_bar(Some(2), Self::tab_bar()).build()
    }

    fn outing_screen(&self) -> Screen {
        let mut s = self.top("Outing");
        if self.naming_location {
            return s
                .secondary("Name this place. Recent places are offered next time.")
                .typed(&self.location, "Location")
                .keyboard(&self.location, "Save")
                .build();
        }
        let Some(outing) = self.open() else {
            return s
                .splash(None, "No open outing", "Start an outing from Today.")
                .build();
        };
        let (species, individuals) = self.outing_totals(outing.id);
        s = s.secondary(format!(
            "{} · {} · {} — {} species, {} birds",
            outing.location, outing.date, outing.start, species, individuals
        ));
        let tally = self.outing_species();
        if tally.is_empty() {
            s = s.section("Log a species by name:");
        }
        let screen = s.rows(tally.iter().take(6).enumerate().map(|(index, bird)| {
            let logged: u32 = self
                .sightings
                .iter()
                .filter(|s| s.outing == outing.id && s.code == bird.code)
                .map(|s| u32::from(s.count))
                .sum();
            (
                format!("tally-{index}"),
                bird.common.clone(),
                if logged > 0 {
                    format!("{} · {logged} logged", bird.code)
                } else {
                    species_line(bird)
                },
                if logged > 0 {
                    Glyph::Check
                } else {
                    Glyph::Search
                },
            )
        }));
        screen
            .buttons([
                ("type-species", "Type a species"),
                ("sightings", "Review sightings"),
                ("finish", "Finish outing"),
            ])
            .build()
    }

    fn sightings_screen(&self) -> Screen {
        let mut s = self.top("Sightings");
        let Some(outing) = self.open() else {
            return s
                .splash(None, "No open outing", "Start an outing from Today.")
                .build();
        };
        if self.deleted.is_some() {
            s = s.banner(BannerLevel::Info, "Sighting deleted.");
        }
        let rows: Vec<(usize, &Sighting)> = self
            .sightings
            .iter()
            .enumerate()
            .filter(|(_, s)| s.outing == outing.id)
            .collect();
        if rows.is_empty() {
            let mut screen = s.splash(
                Some(Glyph::Search),
                "Nothing logged",
                "Tally a species on the outing screen.",
            );
            if self.deleted.is_some() {
                screen = screen.button("undo", "Undo delete");
            }
            return screen.build();
        }
        let mut screen = s.rows(rows.iter().take(6).map(|(index, sighting)| {
            (
                format!("sight-{index}"),
                format!("{} ×{}", sighting.common, sighting.count),
                format!("{} · {}", sighting.code, outing.date),
                Glyph::Check,
            )
        }));
        if self.deleted.is_some() {
            screen = screen.button("undo", "Undo delete");
        }
        screen.build()
    }

    fn life_screen(&self) -> Screen {
        let mut s = self.top("Life list");
        let mut totals: Vec<(&str, &str, u32)> = Vec::new();
        for sighting in &self.sightings {
            if let Some((_, _, total)) = totals
                .iter_mut()
                .find(|(_, code, _)| *code == sighting.code)
            {
                *total += u32::from(sighting.count);
            } else {
                totals.push((&sighting.common, &sighting.code, u32::from(sighting.count)));
            }
        }
        if totals.is_empty() {
            return s
                .splash(
                    Some(Glyph::Search),
                    "No birds logged",
                    "Start an outing from Today; every sighting joins this list.",
                )
                .build();
        }
        s = s.secondary(format!("{} species on this reader.", totals.len()));
        s.rows(
            totals
                .iter()
                .take(6)
                .enumerate()
                .map(|(index, (common, code, total))| {
                    (
                        format!("life-{index}"),
                        (*common).to_owned(),
                        if code.is_empty() {
                            format!("{total} birds")
                        } else {
                            format!("{code} · {total} birds")
                        },
                        Glyph::Check,
                    )
                }),
        )
        .nav_bar(Some(3), Self::tab_bar())
        .build()
    }

    fn export_screen(&self) -> Screen {
        let s = self.top("Export");
        if self.outings.is_empty() {
            return s
                .splash(
                    None,
                    "Nothing to export",
                    "Finished outings become an eBird Checklist Format CSV on this reader, ready for `kobo fieldbook` to fetch.",
                )
                .build();
        }
        s.secondary(format!(
            "{} outing{}, {} sighting{}. The file is written as {} and fetched with `kobo fieldbook`.",
            self.outings.len(),
            if self.outings.len() == 1 { "" } else { "s" },
            self.sightings.len(),
            if self.sightings.len() == 1 { "" } else { "s" },
            EXPORT
        ))
        .button("write-export", "Write checklist file")
        .nav_bar(Some(4), Self::tab_bar())
        .build()
    }

    fn detail_screen(&self) -> Screen {
        let mut s = self.top("Species");
        if let Some(species) = &self.detail {
            s = s.text(format!(
                "{}\n{}\nBanding code: {}",
                species.common, species.scientific, species.code
            ));
            if self.open_outing.is_some() {
                s = s.button("log-detail", "Log in the open outing");
            }
        }
        s.bottom_action("back", "Back").build()
    }

    fn screen(&self, _context: &Context) -> Screen {
        match self.view {
            View::Home => self.home_screen(),
            View::Packs => self.packs_screen(),
            View::Search => self.search_screen(),
            View::Outing => self.outing_screen(),
            View::Sightings => self.sightings_screen(),
            View::Life => self.life_screen(),
            View::Export => self.export_screen(),
            View::Detail => self.detail_screen(),
        }
    }

    fn show(&self, context: &mut Context) {
        context.set_screen(
            self.screen(context)
                .with_own_back(!matches!(self.view, View::Home)),
        );
    }

    fn open_outing_with(&mut self, context: &mut Context, location: String) {
        let id = reader_clock()
            .now()
            .map(|snap| snap.unix_millis)
            .unwrap_or(0);
        self.outings.push(Outing {
            id,
            location,
            date: today_string(),
            start: now_string(),
        });
        self.open_outing = Some(id);
        self.persist(context);
        self.view = View::Outing;
    }

    /// Keyboards consume their own key actions; true means the action is done.
    /// Every `Keyboard` answers any key action, so the view decides which
    /// one is actually on screen and listening.
    fn handle_keyboards(&mut self, context: &mut Context, action: ActionId) -> bool {
        if self.view == View::Search {
            match self.search.press(action) {
                Some(Pressed::Submitted) => {
                    self.query = self.search.take();
                    self.show(context);
                    return true;
                }
                Some(_) => {
                    self.show(context);
                    return true;
                }
                None => return false,
            }
        }
        if self.view == View::Outing && self.naming_location {
            match self.location.press(action) {
                Some(Pressed::Submitted) => {
                    let name = clean(&self.location.take());
                    self.naming_location = false;
                    if !name.is_empty() {
                        self.open_outing_with(context, name);
                    }
                    self.show(context);
                    return true;
                }
                Some(_) => {
                    self.show(context);
                    return true;
                }
                None => return false,
            }
        }
        false
    }
}

impl KoboApp for Fieldbook {
    fn on_start(&mut self, context: &mut Context) {
        self.begin_manifest(context);
        context.store().load(OUTINGS);
        context.store().load(SIGHTINGS);
    }

    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        if self.advance_manifest(context, &result) {
            return;
        }
        match result {
            StoreResult::Loaded { key, value } if key == OUTINGS => {
                let text = value
                    .filter(|bytes| bytes.len() <= MAX_STATE)
                    .and_then(|bytes| String::from_utf8(bytes).ok());
                if let Some(text) = text {
                    self.outings = text
                        .lines()
                        .filter_map(|line| {
                            let mut parts = line.split('|');
                            Some(Outing {
                                id: parts.next()?.parse().ok()?,
                                location: parts.next()?.to_owned(),
                                date: parts.next()?.to_owned(),
                                start: parts.next()?.to_owned(),
                            })
                        })
                        .collect();
                }
            }
            StoreResult::Loaded { key, value } if key == SIGHTINGS => {
                let text = value
                    .filter(|bytes| bytes.len() <= MAX_STATE)
                    .and_then(|bytes| String::from_utf8(bytes).ok());
                if let Some(text) = text {
                    self.sightings = text
                        .lines()
                        .filter_map(|line| {
                            let mut parts = line.split('|');
                            Some(Sighting {
                                outing: parts.next()?.parse().ok()?,
                                code: parts.next()?.to_owned(),
                                common: parts.next()?.to_owned(),
                                scientific: parts.next()?.to_owned(),
                                count: parts.next()?.parse().ok()?,
                            })
                        })
                        .collect();
                }
                self.loads.state = true;
                self.start_when_ready(context);
                return;
            }
            StoreResult::Saved { .. } => {
                self.save = SaveState::Idle;
            }
            StoreResult::Denied(_) => {
                self.save = SaveState::Failed;
            }
            _ => {}
        }
        if self.loads.started {
            self.show(context);
        }
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if self.handle_keyboards(context, action) {
            return;
        }
        if action == ActionId::BACK || action == action_id("back") || action == action_id("home") {
            self.view = View::Home;
        } else if action == action_id("packs") {
            self.view = View::Packs;
        } else if action == action_id("search") {
            self.view = View::Search;
        } else if action == action_id("life") {
            self.view = View::Life;
        } else if action == action_id("export") {
            self.view = View::Export;
        } else if action == action_id("new-outing") {
            self.naming_location = true;
            self.view = View::Outing;
            self.open_outing = None;
        } else if action == action_id("resume") {
            self.view = View::Outing;
        } else if action == action_id("finish") {
            self.open_outing = None;
            self.pack_pick = None;
            self.view = View::Home;
        } else if action == action_id("sightings") {
            self.view = View::Sightings;
        } else if action == action_id("type-species") {
            self.view = View::Search;
        } else if action == action_id("log-detail") {
            if let Some(species) = self.detail.clone() {
                self.log_sighting(context, species);
            }
            self.view = View::Outing;
        } else if action == action_id("log-manual") {
            let name = clean(&self.query);
            if !name.is_empty() {
                self.log_sighting(
                    context,
                    Species {
                        code: String::new(),
                        common: name,
                        scientific: String::new(),
                    },
                );
            }
            self.view = View::Outing;
        } else if action == action_id("write-export") {
            self.export(context);
        } else if let Some(index) =
            (0..self.packs.len()).find(|i| action == action_id(&format!("pack-{i}")))
        {
            self.pack_pick = Some(index);
            self.view = View::Search;
        } else if let Some(index) = (0..3).find(|i| action == action_id(&format!("found-{i}"))) {
            if let Some(species) = self.search_results().get(index).cloned() {
                self.detail = Some(species);
                self.view = View::Detail;
            }
        } else if let Some(index) = (0..6).find(|i| action == action_id(&format!("tally-{i}"))) {
            if let Some(species) = self.outing_species().get(index).cloned() {
                self.log_sighting(context, species);
            }
        } else if let Some(index) = (0..6).find(|i| {
            self.outings.len() > *i
                && action
                    == action_id(&format!(
                        "outing-{}",
                        self.outings[self.outings.len() - 1 - i].id
                    ))
        }) {
            let outing = self.outings[self.outings.len() - 1 - index].id;
            self.open_outing = Some(outing);
            self.view = View::Outing;
        } else if let Some(index) =
            (0..self.sightings.len()).find(|i| action == action_id(&format!("sight-{i}")))
        {
            if let Some(sighting) = self.sightings.get(index).cloned() {
                self.deleted = Some((index, sighting));
                self.sightings.remove(index);
                self.persist(context);
            }
        } else if action == action_id("undo") {
            if let Some((index, sighting)) = self.deleted.take() {
                self.sightings
                    .insert(index.min(self.sightings.len()), sighting);
                self.persist(context);
            }
        }
        self.show(context);
    }
}

fn main() -> ExitCode {
    kobo_sdk::run("fieldbook", Fieldbook::default()).map_or_else(
        |error| {
            eprintln!("fieldbook: {error}");
            ExitCode::FAILURE
        },
        |()| ExitCode::SUCCESS,
    )
}

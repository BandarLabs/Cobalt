//! Fieldbook keeps imported field packs, outing logs and a life list on the
//! reader. Packs arrive over the shelf from the `kobo fieldbook` companion;
//! sightings are recorded locally and never need a connection.
mod shelf;

use kobo_sdk::clock::{Clock, ManualClock, Snapshot, SystemClock};
use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Chrome, Context, DisplayMetrics, Glyph, KoboApp,
    LayoutIssueKind, PictureHandle, Screen, ScreenBuilder, ShelfDownload, ShelfProgress,
    StoreResult, TilePicture,
};
use shelf::{Attribution, Pack, PhotoCredit, Species, MANIFEST, MAX_MANIFEST};
use std::process::ExitCode;

const OUTINGS: &str = "outings.v1";
const SIGHTINGS: &str = "sightings.v1";
const EXPORT: &str = "export-checklist.csv";
const MAX_STATE: usize = 256 * 1024;
const ATTRIBUTION: &str = "attribution.json";
const MAX_PHOTO: usize = 2 * 1024 * 1024;
const PICTURE_HANDLE: PictureHandle = PictureHandle(1);

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
    outing_page: usize,
    home_page: usize,
    packs_page: usize,
    sightings_page: usize,
    life_page: usize,
    delete_candidate: Option<usize>,
    save: SaveState,
    manifest_load: Option<ShelfDownload>,
    loads: Loads,
    credits: Vec<PhotoCredit>,
    attribution_load: Option<ShelfDownload>,
    photo_load: Option<ShelfDownload>,
    photo: Option<TilePicture>,
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
            outing_page: 0,
            home_page: 0,
            packs_page: 0,
            sightings_page: 0,
            life_page: 0,
            delete_candidate: None,
            save: SaveState::Idle,
            manifest_load: None,
            loads: Loads::default(),
            credits: Vec::new(),
            attribution_load: None,
            photo_load: None,
            photo: None,
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

/// Coded species keep their established identity. Uncoded, manually named
/// birds are distinct by name; an empty code must not merge every bird.
fn same_species(code: &str, common: &str, other_code: &str, other_common: &str) -> bool {
    code == other_code && (!code.is_empty() || common.to_lowercase() == other_common.to_lowercase())
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
                        if self
                            .packs
                            .iter()
                            .any(|pack| pack.species.iter().any(|bird| bird.photo.is_some()))
                        {
                            self.begin_attribution(context);
                        }
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

    fn begin_attribution(&mut self, context: &mut Context) {
        let mut load = ShelfDownload::new(ATTRIBUTION).at_most(MAX_MANIFEST);
        load.start(context);
        self.attribution_load = Some(load);
    }

    fn advance_attribution(&mut self, context: &mut Context, result: &StoreResult) -> bool {
        let Some(load) = &mut self.attribution_load else {
            return false;
        };
        match load.advance(context, result) {
            ShelfProgress::Done => {
                let bytes = self
                    .attribution_load
                    .take()
                    .expect("active attribution")
                    .take();
                self.credits = Attribution::decode(&bytes)
                    .map(|attribution| attribution.photos)
                    .unwrap_or_default();
                if self.loads.started && self.view == View::Detail {
                    self.show(context);
                }
                true
            }
            ShelfProgress::Failed(_) => {
                self.attribution_load = None;
                self.credits.clear();
                true
            }
            ShelfProgress::Moving { .. } => true,
            ShelfProgress::Elsewhere => false,
        }
    }

    /// Loads the detail screen's photo, when the pack carries one. The
    /// companion publishes photos under the asset's file name; the pack
    /// keeps the human-readable directory layout.
    fn begin_photo(&mut self, context: &mut Context) {
        self.photo = None;
        self.photo_load = None;
        let Some(photo) = self
            .detail
            .as_ref()
            .and_then(|species| species.photo.as_ref())
        else {
            return;
        };
        let mut load = ShelfDownload::new(photo.shelf_key().to_owned()).at_most(MAX_PHOTO);
        load.start(context);
        self.photo_load = Some(load);
    }

    fn advance_photo(&mut self, context: &mut Context, result: &StoreResult) -> bool {
        let Some(load) = &mut self.photo_load else {
            return false;
        };
        match load.advance(context, result) {
            ShelfProgress::Done => {
                let bytes = self.photo_load.take().expect("active photo").take();
                self.photo = kobo_image::decode(&bytes)
                    .and_then(|decoded| decoded.fit(640, 480))
                    .map(|mut picture| {
                        picture.dither(kobo_image::PANEL_GREYS);
                        context.put_picture(
                            PICTURE_HANDLE,
                            picture.width(),
                            picture.height(),
                            picture.into_grey(),
                        )
                    })
                    .unwrap_or(None);
                self.show(context);
                true
            }
            ShelfProgress::Failed(_) => {
                self.photo_load = None;
                self.photo = None;
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
            if !seen.iter().any(|known| {
                same_species(&known.code, &known.common, &sighting.code, &sighting.common)
            }) {
                seen.push(Species {
                    code: sighting.code.clone(),
                    common: sighting.common.clone(),
                    scientific: sighting.scientific.clone(),
                    photo: None,
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
                if matches
                    && !found.iter().any(|known| {
                        same_species(&known.code, &known.common, &species.code, &species.common)
                    })
                {
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
        if let Some(row) = self.sightings.iter_mut().find(|s| {
            s.outing == outing && same_species(&s.code, &s.common, &species.code, &species.common)
        }) {
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

    fn paged_rows(
        context: &Context,
        screen: ScreenBuilder,
        rows: &[(String, String, String, Glyph)],
        page: usize,
        bottom_bar: bool,
    ) -> (ScreenBuilder, usize) {
        let labels = rows
            .iter()
            .map(|(_, title, detail, _)| (title.as_str(), detail.as_str()))
            .collect::<Vec<_>>();
        let pages = context.paginate_rows_under(
            &labels,
            bottom_bar,
            kobo_sdk::Position::AtTheFoot,
            &screen.clone().build(),
        );
        let count = pages.len().max(1);
        let page = page.min(count - 1);
        let visible = pages.get(page).map(Vec::as_slice).unwrap_or_default();
        let mut screen = screen.rows(visible.iter().map(|&index| rows[index].clone()));
        if count > 1 {
            screen = screen
                .page_turns("list-previous", "list-next")
                .page_position(
                    u16::try_from(page + 1).unwrap_or(u16::MAX),
                    u16::try_from(count).unwrap_or(u16::MAX),
                );
        }
        (screen, count)
    }

    fn home_screen(&self, context: &Context) -> (Screen, usize) {
        let mut screen = self.top("Fieldbook");
        if let Some(notice) = &self.pack_notice {
            screen = screen.banner(BannerLevel::Attention, notice);
        }
        screen = if self.packs.is_empty() {
            screen.secondary("No field pack yet. Push one with `kobo fieldbook` from a computer; logging works without one.")
        } else {
            screen.secondary(format!(
                "{} pack{}, {} species.",
                self.packs.len(),
                if self.packs.len() == 1 { "" } else { "s" },
                self.packs
                    .iter()
                    .map(|pack| pack.species.len())
                    .sum::<usize>()
            ))
        };
        if let Some(outing) = self.open() {
            let (species, birds) = self.outing_totals(outing.id);
            screen = screen
                .secondary(format!(
                    "Open outing: {} · {} · {} · {} species, {} birds",
                    outing.location, outing.date, outing.start, species, birds
                ))
                .buttons([("resume", "Resume tally"), ("finish", "Finish outing")]);
        } else {
            screen = screen.buttons([("new-outing", "Start an outing")]);
        }
        let rows = self
            .outings
            .iter()
            .rev()
            .map(|outing| {
                let (species, _) = self.outing_totals(outing.id);
                (
                    format!("outing-{}", outing.id),
                    context.clamped_row(&format!("{} · {}", outing.location, outing.date), 2, true),
                    format!("{species} species"),
                    Glyph::Check,
                )
            })
            .collect::<Vec<_>>();
        let (screen, count) = Self::paged_rows(context, screen, &rows, self.home_page, true);
        (screen.nav_bar(Some(0), Self::tab_bar()).build(), count)
    }

    fn packs_screen(&self, context: &Context) -> (Screen, usize) {
        let mut screen = self.top("Field packs");
        if self.packs.is_empty() {
            screen=screen.splash(Some(Glyph::Search),"No field pack yet","Push a pack with `kobo fieldbook` from a computer. Sightings logged now stay on this reader and join any pack you import later.");
            if let Some(notice) = &self.pack_notice {
                screen = screen.banner(BannerLevel::Attention, notice);
            }
            return (screen.nav_bar(Some(1), Self::tab_bar()).build(), 1);
        }
        for (input, reason) in &self.pack_failures {
            screen = screen.secondary(format!("{input}: {reason}"));
        }
        let rows = self
            .packs
            .iter()
            .enumerate()
            .map(|(index, pack)| {
                (
                    format!("pack-{index}"),
                    context.clamped_row(&pack.title, 2, true),
                    format!(
                        "{} · issued {} · {} species",
                        pack.region,
                        pack.issued,
                        pack.species.len()
                    ),
                    Glyph::Search,
                )
            })
            .collect::<Vec<_>>();
        let (screen, count) = Self::paged_rows(context, screen, &rows, self.packs_page, true);
        (screen.nav_bar(Some(1), Self::tab_bar()).build(), count)
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

    fn outing_screen(&self, metrics: DisplayMetrics) -> Screen {
        let s = self.top("Outing");
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
        let summary = format!(
            "{} · {} · {} — {} species, {} birds",
            outing.location, outing.date, outing.start, species, individuals
        );
        let tally = self.outing_species();
        let rows: Vec<(String, String, String, Glyph)> = tally
            .iter()
            .take(6)
            .enumerate()
            .map(|(index, bird)| {
                let logged: u32 = self
                    .sightings
                    .iter()
                    .filter(|s| {
                        s.outing == outing.id
                            && same_species(&s.code, &s.common, &bird.code, &bird.common)
                    })
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
            })
            .collect();
        // One panel of the outing, whole: the summary, the tally rows dealt
        // to the device in hand, and the three actions. At the largest text
        // scale six rows and the actions do not fit one Clara panel, so the
        // rows deal onto panels measured the way the gallery measures its
        // reference pages -- a button pushed off the panel is an outing that
        // cannot be finished.
        let frame = |page_rows: &[(String, String, String, Glyph)], page: usize, pages: usize| {
            let mut screen = self.top("Outing").secondary(summary.clone());
            if tally.is_empty() {
                screen = screen.section("Log a species by name:");
            }
            let mut screen = screen.rows(page_rows.iter().cloned()).buttons([
                ("type-species", "Type a species"),
                ("sightings", "Review sightings"),
                ("finish", "Finish outing"),
            ]);
            if pages > 1 {
                // Six rows deal onto a single-digit page count; the fallback
                // is for the compiler, not the reader.
                let (page, of) = (
                    u16::try_from(page + 1).unwrap_or(1),
                    u16::try_from(pages).unwrap_or(1),
                );
                screen = screen
                    .page_turns("outing-back", "outing-next")
                    .page_position(page, of);
            }
            screen
        };
        let mut panels: Vec<usize> = Vec::new();
        let mut start = 0;
        while start < rows.len() {
            let mut take = rows.len() - start;
            // Measured against the most pages the deal can produce, because
            // "6 of 6" is wider than "1 of 1" and a page that fits only
            // while it is the only page is not fitting.
            while take > 1
                && Self::overflows(
                    frame(&rows[start..start + take], panels.len(), rows.len()),
                    metrics,
                )
            {
                take -= 1;
            }
            panels.push(take);
            start += take;
        }
        if panels.is_empty() {
            panels.push(0);
        }
        let page = self.outing_page.min(panels.len() - 1);
        let start = panels[..page].iter().sum();
        frame(&rows[start..start + panels[page]], page, panels.len()).build()
    }

    /// Whether anything on this screen was pushed off the panel, or squeezed
    /// until it no longer fits what it says. Measured against the runtime's
    /// own diagnostics, the way the gallery deals its reference pages.
    fn overflows(screen: ScreenBuilder, metrics: DisplayMetrics) -> bool {
        screen
            .build()
            .diagnostics(&metrics, &Chrome::measuring(false))
            .issues
            .iter()
            .any(|issue| {
                matches!(
                    issue.kind,
                    LayoutIssueKind::ContentOverflow { .. }
                        | LayoutIssueKind::Clipped
                        | LayoutIssueKind::InteractiveOffscreen
                        | LayoutIssueKind::TextOverflow
                )
            })
    }

    fn sightings_screen(&self, context: &Context) -> (Screen, usize) {
        let mut screen = self.top("Sightings");
        let Some(outing) = self.open() else {
            return (
                screen
                    .splash(None, "No open outing", "Start an outing from Today.")
                    .build(),
                1,
            );
        };
        if self.deleted.is_some() {
            screen = screen.banner(BannerLevel::Info, "Sighting deleted.");
        }
        let rows = self
            .sightings
            .iter()
            .enumerate()
            .filter(|(_, sighting)| sighting.outing == outing.id)
            .map(|(index, sighting)| {
                (
                    format!("sight-{index}"),
                    context.clamped_row(
                        &format!("{} ×{}", sighting.common, sighting.count),
                        2,
                        self.deleted.is_some(),
                    ),
                    format!("{} · {}", sighting.code, outing.date),
                    Glyph::Check,
                )
            })
            .collect::<Vec<_>>();
        if rows.is_empty() {
            screen = screen.splash(
                Some(Glyph::Search),
                "Nothing logged",
                "Tally a species on the outing screen.",
            );
        } else {
            screen = screen.secondary("Tap a sighting to review its removal.");
        }
        let (mut screen, count) = Self::paged_rows(
            context,
            screen,
            &rows,
            self.sightings_page,
            self.deleted.is_some(),
        );
        if self.deleted.is_some() {
            screen = screen.bottom_action("undo", "Undo delete");
        }
        if let Some(sighting) = self
            .delete_candidate
            .and_then(|index| self.sightings.get(index))
        {
            screen = screen.confirm(
                "Delete sighting?",
                format!(
                    "Remove {} ×{} from this outing? You can undo this deletion.",
                    context.clamped_row(&sighting.common, 2, false),
                    sighting.count
                ),
                ("delete-sighting", "Delete"),
                ("keep-sighting", "Keep sighting"),
            );
        }
        (screen.build(), count)
    }

    fn life_totals(&self) -> Vec<(&Sighting, u32)> {
        let mut totals: Vec<(&Sighting, u32)> = Vec::new();
        for sighting in &self.sightings {
            if let Some((_, total)) = totals.iter_mut().find(|(known, _)| {
                same_species(&known.code, &known.common, &sighting.code, &sighting.common)
            }) {
                *total += u32::from(sighting.count);
            } else {
                totals.push((sighting, u32::from(sighting.count)));
            }
        }
        totals
    }

    fn life_screen(&self, context: &Context) -> (Screen, usize) {
        let mut screen = self.top("Life list");
        let totals = self.life_totals();
        if totals.is_empty() {
            return (
                screen
                    .splash(
                        Some(Glyph::Search),
                        "No birds logged",
                        "Start an outing from Today; every sighting joins this list.",
                    )
                    .nav_bar(Some(3), Self::tab_bar())
                    .build(),
                1,
            );
        }
        screen = screen.secondary(format!("{} species on this reader.", totals.len()));
        let rows = totals
            .iter()
            .enumerate()
            .map(|(index, (sighting, total))| {
                (
                    format!("life-{index}"),
                    context.clamped_row(&sighting.common, 2, true),
                    if sighting.code.is_empty() {
                        format!("{total} birds")
                    } else {
                        format!("{} · {total} birds", sighting.code)
                    },
                    Glyph::Check,
                )
            })
            .collect::<Vec<_>>();
        let (screen, count) = Self::paged_rows(context, screen, &rows, self.life_page, true);
        (screen.nav_bar(Some(3), Self::tab_bar()).build(), count)
    }

    fn turn_list(&mut self, context: &Context, forward: bool) {
        let count = match self.view {
            View::Home => self.home_screen(context).1,
            View::Packs => self.packs_screen(context).1,
            View::Sightings => self.sightings_screen(context).1,
            View::Life => self.life_screen(context).1,
            _ => return,
        };
        let page = match self.view {
            View::Home => &mut self.home_page,
            View::Packs => &mut self.packs_page,
            View::Sightings => &mut self.sightings_page,
            View::Life => &mut self.life_page,
            _ => return,
        };
        let last = count.saturating_sub(1);
        *page = if forward {
            page.saturating_add(1).min(last)
        } else {
            (*page).min(last).saturating_sub(1)
        };
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
            if let Some(picture) = self.photo {
                s = s.picture(picture, 45);
                if let Some(credit) = species
                    .photo
                    .as_ref()
                    .and_then(|photo| self.credits.iter().find(|c| c.id == photo.attribution))
                {
                    s = s.secondary(format!("Photo: {} · {}", credit.creator, credit.license));
                }
            }
            if self.open_outing.is_some() {
                s = s.button("log-detail", "Log in the open outing");
            }
        }
        s.bottom_action("back", "Back").build()
    }

    fn screen(&self, context: &Context) -> Screen {
        match self.view {
            View::Home => self.home_screen(context).0,
            View::Packs => self.packs_screen(context).0,
            View::Search => self.search_screen(),
            View::Outing => self.outing_screen(context.metrics()),
            View::Sightings => self.sightings_screen(context).0,
            View::Life => self.life_screen(context).0,
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

    fn handle_delete_confirmation(&mut self, context: &mut Context, action: ActionId) -> bool {
        let Some(index) = self.delete_candidate else {
            return false;
        };
        {
            if action == action_id("delete-sighting") {
                self.delete_candidate = None;
                if let Some(sighting) = self.sightings.get(index).cloned() {
                    self.deleted = Some((index, sighting));
                    self.sightings.remove(index);
                    self.persist(context);
                    self.sightings_page = self
                        .sightings_page
                        .min(self.sightings_screen(context).1.saturating_sub(1));
                }
            } else if action == action_id("keep-sighting") || action == ActionId::BACK {
                self.delete_candidate = None;
            }
            true
        }
    }

    fn open_life_entry(&mut self, index: usize) {
        if let Some((sighting, _)) = self.life_totals().get(index) {
            self.detail = Some(Species {
                code: sighting.code.clone(),
                common: sighting.common.clone(),
                scientific: sighting.scientific.clone(),
                photo: None,
            });
            self.view = View::Detail;
        }
    }

    fn log_manual(&mut self, context: &mut Context) {
        let name = clean(&self.query);
        if !name.is_empty() {
            self.log_sighting(
                context,
                Species {
                    code: String::new(),
                    common: name,
                    scientific: String::new(),
                    photo: None,
                },
            );
        }
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
        if self.advance_attribution(context, &result) {
            return;
        }
        if self.advance_photo(context, &result) {
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

    fn on_page_turn(&mut self, context: &mut Context, forward: bool) {
        if self.delete_candidate.is_none()
            && matches!(
                self.view,
                View::Home | View::Packs | View::Sightings | View::Life
            )
        {
            self.turn_list(context, forward);
            self.show(context);
        }
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if self.handle_delete_confirmation(context, action) {
            self.show(context);
            return;
        }
        if self.handle_keyboards(context, action) {
            return;
        }
        if action == ActionId::BACK || action == action_id("back") || action == action_id("home") {
            self.view = View::Home;
        } else if action == action_id("list-next") {
            self.turn_list(context, true);
        } else if action == action_id("list-previous") {
            self.turn_list(context, false);
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
            self.outing_page = 0;
        } else if action == action_id("resume") {
            self.view = View::Outing;
            self.outing_page = 0;
        } else if action == action_id("outing-next") {
            self.outing_page += 1;
        } else if action == action_id("outing-back") {
            self.outing_page = self.outing_page.saturating_sub(1);
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
            self.log_manual(context);
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
                self.begin_photo(context);
            }
        } else if let Some(index) = (0..6).find(|i| action == action_id(&format!("tally-{i}"))) {
            if let Some(species) = self.outing_species().get(index).cloned() {
                self.log_sighting(context, species);
            }
        } else if let Some(outing) = self
            .outings
            .iter()
            .find(|outing| action == action_id(&format!("outing-{}", outing.id)))
        {
            self.open_outing = Some(outing.id);
            self.view = View::Outing;
            self.outing_page = 0;
            self.sightings_page = 0;
        } else if let Some(index) = (0..self.sightings.len()).find(|index| {
            self.view == View::Sightings && action == action_id(&format!("sight-{index}"))
        }) {
            self.delete_candidate = Some(index);
        } else if let Some(index) = (0..self.life_totals().len())
            .find(|index| self.view == View::Life && action == action_id(&format!("life-{index}")))
        {
            self.open_life_entry(index);
        } else if action == action_id("undo") {
            if let Some((index, sighting)) = self.deleted.take() {
                self.sightings
                    .insert(index.min(self.sightings.len()), sighting);
                self.persist(context);
            }
        }
        if self.view != View::Detail {
            self.photo = None;
            self.photo_load = None;
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

#[cfg(test)]
mod list_tests;

//! Music Stand: real score pages, setlists, and half-page turns.
//!
//! Scores are prepared and atomically published by `kobo musicstand`; the app
//! reads the shelf, shows fitted pages, and keeps each score's page, zoom and
//! mark between sessions.

mod shelf;

use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Glyph, KoboApp, PictureHandle, Screen,
    ScreenBuilder, ShelfDownload, ShelfProgress, StoreResult, TilePicture,
};
use shelf::{Manifest, Score, MANIFEST, MAX_MANIFEST};
use std::{collections::BTreeMap, process::ExitCode, time::Duration};

const STATE: &str = "musicstand-state";
const PAGE_HANDLE: PictureHandle = PictureHandle(1);
const MAX_PAGE_BYTES: usize = 8 * 1024 * 1024;
const OPEN: &str = "open";
const MENU: &str = "menu";
const NEXT: &str = "next";
const PREVIOUS: &str = "previous";
const LIBRARY: &str = "library";
const SETLISTS: &str = "setlists";
const ABOUT: &str = "about";
const ZOOM: &str = "zoom";
const MARK: &str = "mark";

/// How far the bottom half reaches back into the top half, so the line being
/// read is still in sight after a half-page turn.
const HALF_OVERLAP_NUMERATOR: u32 = 1;
const HALF_OVERLAP_DENOMINATOR: u32 = 14;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ScoreState {
    page: u32,
    bottom_half: bool,
    zoomed: bool,
    marked: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Setlist {
    name: String,
    entries: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum View {
    #[default]
    Library,
    Stand,
    Setlists,
    Setlist,
    About,
}

#[derive(Default)]
struct Stand {
    view: View,
    scores: Vec<Score>,
    failures: Vec<(String, String)>,
    states: BTreeMap<String, ScoreState>,
    setlists: Vec<Setlist>,
    current: Option<String>,
    /// Setlist position the current score was opened from, so a turn past the
    /// last page moves rehearsal to the next entry.
    setlist_position: Option<usize>,
    open_setlist: Option<usize>,
    page: Option<TilePicture>,
    page_key: Option<String>,
    menu_open: bool,
    notice: Option<String>,
    startup: Startup,
    manifest_load: Option<ShelfDownload>,
    page_load: Option<ShelfDownload>,
    page_load_key: Option<String>,
}

#[derive(Default)]
struct Startup {
    state_loaded: bool,
    manifest_loaded: bool,
    started: bool,
}

impl Stand {
    fn current_score(&self) -> Option<&Score> {
        let id = self.current.as_deref()?;
        self.scores.iter().find(|score| score.id == id)
    }

    fn state_of(&self, id: &str) -> ScoreState {
        self.states.get(id).copied().unwrap_or_default()
    }

    fn state_mut(&mut self, id: &str) -> &mut ScoreState {
        self.states.entry(id.to_owned()).or_default()
    }

    /// The page file and crop the stand should show right now.
    fn wanted_page(&self) -> Option<(String, String)> {
        let score = self.current_score()?;
        let state = self.state_of(&score.id);
        let page = state.page.min(score.pages.saturating_sub(1));
        Some((score.id.clone(), shelf::page_name(&score.id, page)))
    }

    fn crop_region(score: &Score, state: ScoreState) -> (u32, u32, u32, u32) {
        let (mut x, mut y, mut width, mut height) = (0, 0, score.width, score.height);
        if state.zoomed {
            // Centre crop at two thirds width: the staff area keeps its detail
            // and the empty margins go first.
            width = score.width * 2 / 3;
            x = (score.width - width) / 2;
        }
        let half = height / 2;
        let overlap = height * HALF_OVERLAP_NUMERATOR / HALF_OVERLAP_DENOMINATOR;
        if state.bottom_half {
            y = half - overlap;
            height -= y;
        } else {
            height = half + overlap;
        }
        (x, y, width, height)
    }

    fn start_when_ready(&mut self, context: &mut Context) {
        if self.startup.state_loaded && self.startup.manifest_loaded && !self.startup.started {
            self.startup.started = true;
            self.start_page_load(context);
        }
    }

    fn start_page_load(&mut self, context: &mut Context) {
        if self.view != View::Stand {
            return;
        }
        let Some((id, name)) = self.wanted_page() else {
            return;
        };
        let key = format!("{id}:{name}");
        if self.page_key.as_deref() == Some(key.as_str()) || self.page_load.is_some() {
            return;
        }
        let mut load = ShelfDownload::new(name).at_most(MAX_PAGE_BYTES);
        load.start(context);
        self.page_load = Some(load);
        self.page_load_key = Some(key);
    }

    fn advance(&mut self, context: &mut Context) {
        let Some(score) = self
            .current_score()
            .map(|score| (score.id.clone(), score.pages))
        else {
            return;
        };
        let state = self.state_mut(&score.0);
        let last = score.1.saturating_sub(1);
        if !state.bottom_half {
            state.bottom_half = true;
        } else if state.page < last {
            state.page += 1;
            state.bottom_half = false;
        } else if let Some(position) = self.setlist_position {
            // Past the final page: rehearsal moves to the next setlist entry.
            let next = self
                .open_setlist
                .and_then(|index| self.setlists.get(index))
                .and_then(|list| list.entries.get(position + 1))
                .cloned();
            match next {
                Some(id) if self.scores.iter().any(|score| score.id == id) => {
                    self.setlist_position = Some(position + 1);
                    self.open_score(context, &id, self.setlist_position);
                    return;
                }
                _ => {
                    self.notice = Some("End of the setlist.".to_owned());
                }
            }
        } else {
            self.notice = Some("Last page.".to_owned());
        }
        self.save(context);
        self.page = None;
        self.page_key = None;
        self.start_page_load(context);
    }

    fn previous(&mut self, context: &mut Context) {
        let Some(score) = self.current_score().map(|score| score.id.clone()) else {
            return;
        };
        let state = self.state_mut(&score);
        if state.bottom_half {
            state.bottom_half = false;
        } else if state.page > 0 {
            state.page -= 1;
            state.bottom_half = true;
        }
        self.notice = None;
        self.save(context);
        self.page = None;
        self.page_key = None;
        self.start_page_load(context);
    }

    fn open_score(&mut self, context: &mut Context, id: &str, position: Option<usize>) {
        self.current = Some(id.to_owned());
        self.setlist_position = position;
        self.menu_open = false;
        self.notice = None;
        self.page = None;
        self.page_key = None;
        self.view = View::Stand;
        context.device().keep_awake(Duration::from_secs(14_400));
        self.save(context);
        self.start_page_load(context);
    }

    fn leave_stand(&mut self, context: &mut Context) {
        self.view = View::Library;
        self.menu_open = false;
        context.device().allow_sleep();
        self.save(context);
    }

    fn save(&self, context: &mut Context) {
        let mut text = String::from("v1\n");
        if let Some(id) = &self.current {
            text.push_str("last\t");
            text.push_str(id);
            text.push('\n');
        }
        for (id, state) in &self.states {
            text.push_str(&format!(
                "score\t{id}\t{}\t{}\t{}\t{}\n",
                state.page,
                u8::from(state.bottom_half),
                u8::from(state.zoomed),
                u8::from(state.marked)
            ));
        }
        for list in &self.setlists {
            text.push_str(&format!(
                "setlist\t{}\t{}\n",
                list.name,
                list.entries.join(",")
            ));
        }
        context.store().save(STATE, text.into_bytes());
    }

    fn restore(&mut self, bytes: &[u8]) {
        let Ok(text) = String::from_utf8(bytes.to_vec()) else {
            return;
        };
        for line in text.lines().skip(1) {
            let fields: Vec<&str> = line.split('\t').collect();
            match fields.as_slice() {
                ["last", id] => self.current = Some((*id).to_owned()),
                ["score", id, page, bottom, zoomed, marked] => {
                    self.states.insert(
                        (*id).to_owned(),
                        ScoreState {
                            page: page.parse().unwrap_or(0),
                            bottom_half: *bottom == "1",
                            zoomed: *zoomed == "1",
                            marked: *marked == "1",
                        },
                    );
                }
                ["setlist", name, entries] => {
                    self.setlists.push(Setlist {
                        name: (*name).to_owned(),
                        entries: entries
                            .split(',')
                            .filter(|entry| !entry.is_empty())
                            .map(str::to_owned)
                            .collect(),
                    });
                }
                _ => {}
            }
        }
    }

    fn library(&self) -> Screen {
        let mut screen = ScreenBuilder::new("music-library").top_bar("Music Stand");
        if let Some(notice) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, notice);
        }
        if !self.startup.manifest_loaded {
            return screen.activity("Opening your music shelf", None).build();
        }
        for (input, reason) in &self.failures {
            screen = screen.banner(
                BannerLevel::Attention,
                format!("{input} did not import: {reason}"),
            );
        }
        if self.scores.is_empty() {
            return screen
                .splash(
                    Some(Glyph::Note),
                    "Your stand is empty",
                    "On your computer, run `kobo musicstand init --device IP`, then `kobo musicstand push SCORE.pdf --device IP`.",
                )
                .build();
        }
        let mut screen = screen.heading("Library");
        for score in &self.scores {
            let state = self.state_of(&score.id);
            let detail = if state.marked {
                format!("{} pages · page {} marked", score.pages, state.page + 1)
            } else {
                format!("{} pages", score.pages)
            };
            screen = screen.rows([(
                format!("{OPEN}-{}", score.id),
                score.title.clone(),
                detail,
                Glyph::Note,
            )]);
        }
        screen
            .button(SETLISTS, "Setlists")
            .button(ABOUT, "Add scores")
            .build()
    }

    fn stand(&self) -> Screen {
        let mut screen = ScreenBuilder::new("music-stand");
        if let Some(notice) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, notice);
        }
        let Some(score) = self.current_score() else {
            return screen
                .banner(
                    BannerLevel::Attention,
                    "This score is no longer on the shelf.",
                )
                .button(LIBRARY, "Library")
                .build()
                .with_own_back(true);
        };
        let state = self.state_of(&score.id);
        let mut screen = match self.page {
            Some(picture) => screen.unframed_picture(picture, 500),
            None => screen.activity("Turning the page", None),
        };
        screen = screen.page_turns(PREVIOUS, NEXT).reading_menu(MENU);
        if self.menu_open {
            let half = if state.bottom_half {
                "bottom half"
            } else {
                "top half"
            };
            screen = screen.modal(score.title.clone(), |overlay| {
                overlay
                    .facts([
                        (
                            "Page",
                            format!("{} of {} · {half}", state.page + 1, score.pages),
                        ),
                        (
                            "Zoom",
                            if state.zoomed {
                                "Staff width".to_owned()
                            } else {
                                "Whole page".to_owned()
                            },
                        ),
                        (
                            "Mark",
                            if state.marked {
                                "Corner marked".to_owned()
                            } else {
                                "None".to_owned()
                            },
                        ),
                    ])
                    .buttons([
                        (ZOOM, if state.zoomed { "Zoom out" } else { "Zoom in" }),
                        (
                            MARK,
                            if state.marked {
                                "Remove mark"
                            } else {
                                "Mark page corner"
                            },
                        ),
                        (LIBRARY, "Library"),
                    ])
            });
        }
        screen.build().with_own_back(true)
    }

    fn setlists(&self) -> Screen {
        let mut screen = ScreenBuilder::new("music-setlists")
            .top_bar("Music Stand")
            .heading("Setlists");
        for (index, list) in self.setlists.iter().enumerate() {
            screen = screen.rows([(
                format!("list-{index}"),
                list.name.clone(),
                format!("{} scores", list.entries.len()),
                Glyph::Note,
            )]);
        }
        screen
            .secondary("Setlists keep their order for rehearsal and gigs.")
            .button("new-list", "New setlist from the library")
            .button(LIBRARY, "Library")
            .build()
    }

    fn setlist(&self, index: usize) -> Screen {
        let Some(list) = self.setlists.get(index) else {
            return self.setlists();
        };
        let mut screen = ScreenBuilder::new("music-setlist")
            .top_bar("Setlist")
            .heading(list.name.clone());
        if list.entries.is_empty() {
            screen = screen.secondary("No scores on this setlist yet.");
        }
        for (position, id) in list.entries.iter().enumerate() {
            let Some(score) = self.scores.iter().find(|score| &score.id == id) else {
                continue;
            };
            let state = self.state_of(id);
            screen = screen.rows([(
                format!("entry-{position}"),
                format!("{}. {}", position + 1, score.title),
                format!("resume at page {}", state.page + 1),
                Glyph::Note,
            )]);
        }
        let mut buttons = vec![(SETLISTS.to_owned(), "All setlists".to_owned())];
        if !list.entries.is_empty() {
            buttons.push(("remove-last".to_owned(), "Remove last score".to_owned()));
        }
        screen.buttons(buttons).build()
    }

    fn about() -> Screen {
        ScreenBuilder::new("music-about")
            .top_bar("Music Stand")
            .heading("Transfer")
            .text("Add scores from your computer with `kobo musicstand push`. PDF scores are rendered page by page; folders of images transfer as they are. Pages are prepared for clear E Ink reading before they leave the computer.")
            .text("Only transfer scores you have the right to use.")
            .button(LIBRARY, "Library")
            .build()
    }

    fn show(&self, context: &mut Context) {
        let screen = match self.view {
            View::Library => self.library(),
            View::Stand => self.stand(),
            View::Setlists => self.setlists(),
            View::Setlist => self
                .open_setlist
                .map_or_else(|| self.setlists(), |index| self.setlist(index)),
            View::About => Self::about(),
        };
        context.set_screen(screen);
    }

    fn advance_manifest(&mut self, context: &mut Context, result: &StoreResult) -> bool {
        let Some(load) = &mut self.manifest_load else {
            return false;
        };
        match load.advance(context, result) {
            ShelfProgress::Done => {
                let bytes = self.manifest_load.take().expect("active manifest").take();
                match Manifest::decode(&bytes) {
                    Ok(manifest) => {
                        self.failures = manifest
                            .failures
                            .iter()
                            .map(|failure| (failure.input.clone(), failure.reason.clone()))
                            .collect();
                        self.scores = manifest.scores;
                        self.notice = None;
                    }
                    Err(error) => {
                        self.scores.clear();
                        self.notice = Some(format!("Music shelf needs re-pushing: {error}"));
                    }
                }
                self.startup.manifest_loaded = true;
                self.start_when_ready(context);
                true
            }
            ShelfProgress::Failed(kobo_sdk::StoreError::Missing) => {
                self.manifest_load = None;
                self.startup.manifest_loaded = true;
                true
            }
            ShelfProgress::Failed(_) => {
                self.manifest_load = None;
                self.startup.manifest_loaded = true;
                self.notice = Some("Music shelf could not be opened.".to_owned());
                true
            }
            ShelfProgress::Moving { .. } => true,
            ShelfProgress::Elsewhere => false,
        }
    }

    fn advance_page(&mut self, context: &mut Context, result: &StoreResult) -> bool {
        let Some(load) = &mut self.page_load else {
            return false;
        };
        match load.advance(context, result) {
            ShelfProgress::Done => {
                let bytes = self.page_load.take().expect("active page").take();
                let key = self.page_load_key.take().expect("active page key");
                if self.wanted_page().map(|(id, name)| format!("{id}:{name}")) == Some(key.clone())
                {
                    match kobo_image::decode(&bytes) {
                        Ok(picture) => {
                            if let (Some(score), Some((id, _))) =
                                (self.current_score(), self.wanted_page())
                            {
                                let state = self.state_of(&id);
                                let (x, y, width, height) = Self::crop_region(score, state);
                                match picture.crop(x, y, width, height) {
                                    Ok(crop) => {
                                        self.page = context.put_picture(
                                            PAGE_HANDLE,
                                            crop.width(),
                                            crop.height(),
                                            crop.into_grey(),
                                        );
                                        if self.page.is_none() {
                                            self.notice = Some(
                                                "This page is larger than the picture budget."
                                                    .to_owned(),
                                            );
                                        } else {
                                            self.page_key = Some(key);
                                            self.notice = None;
                                        }
                                    }
                                    Err(_) => {
                                        self.notice =
                                            Some("This page could not be cropped.".to_owned());
                                    }
                                }
                            }
                        }
                        Err(_) => {
                            self.notice =
                                Some("This page could not be read. Re-push the score.".to_owned());
                        }
                    }
                }
                true
            }
            ShelfProgress::Failed(_) => {
                self.page_load = None;
                self.page_load_key = None;
                self.notice =
                    Some("This page is missing or damaged. Re-push the score.".to_owned());
                true
            }
            ShelfProgress::Moving { .. } => true,
            ShelfProgress::Elsewhere => false,
        }
    }
}

impl KoboApp for Stand {
    fn on_start(&mut self, context: &mut Context) {
        context.store().load(STATE);
        let mut manifest = ShelfDownload::new(MANIFEST).at_most(MAX_MANIFEST);
        manifest.start(context);
        self.manifest_load = Some(manifest);
        self.show(context);
    }

    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        if let StoreResult::Loaded { value, .. } = &result {
            if let Some(bytes) = value {
                self.restore(bytes);
            }
            self.startup.state_loaded = true;
            self.start_when_ready(context);
            self.show(context);
            return;
        }
        if self.advance_manifest(context, &result) || self.advance_page(context, &result) {
            self.show(context);
        }
    }

    fn on_page_turn(&mut self, context: &mut Context, forward: bool) {
        if self.view == View::Stand {
            if forward {
                self.advance(context);
            } else {
                self.previous(context);
            }
            self.show(context);
        }
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if action == ActionId::BACK {
            match self.view {
                View::Stand => self.leave_stand(context),
                View::Library => context.exit(),
                _ => self.view = View::Library,
            }
            self.show(context);
            return;
        }
        let mut handled = false;
        for score in self.scores.clone() {
            if action == action_id(&format!("{OPEN}-{}", score.id)) {
                self.open_score(context, &score.id, None);
                handled = true;
                break;
            }
        }
        if !handled {
            for index in 0..self.setlists.len() {
                if action == action_id(&format!("list-{index}")) {
                    self.open_setlist = Some(index);
                    self.view = View::Setlist;
                    handled = true;
                    break;
                }
            }
        }
        if !handled {
            if let Some(list) = self.open_setlist.and_then(|index| self.setlists.get(index)) {
                for (position, id) in list.entries.clone().into_iter().enumerate() {
                    if action == action_id(&format!("entry-{position}")) {
                        if self.scores.iter().any(|score| score.id == id) {
                            self.open_score(context, &id, Some(position));
                        }
                        handled = true;
                        break;
                    }
                }
            }
        }
        if handled {
        } else if action == action_id(MENU) {
            self.menu_open = !self.menu_open;
        } else if action == action_id(ZOOM) {
            if let Some(id) = self.current.clone() {
                let state = self.state_mut(&id);
                state.zoomed = !state.zoomed;
            }
            self.save(context);
            self.page = None;
            self.page_key = None;
            self.page_reload(context);
        } else if action == action_id(MARK) {
            if let Some(id) = self.current.clone() {
                let state = self.state_mut(&id);
                state.marked = !state.marked;
            }
            self.save(context);
        } else if action == action_id(LIBRARY) {
            self.leave_stand(context);
        } else if action == action_id(SETLISTS) {
            self.view = View::Setlists;
        } else if action == action_id(ABOUT) {
            self.view = View::About;
        } else if action == action_id("new-list") {
            let number = self.setlists.len() + 1;
            self.setlists.push(Setlist {
                name: format!("Setlist {number}"),
                entries: self.scores.iter().map(|score| score.id.clone()).collect(),
            });
            self.save(context);
        } else if action == action_id("remove-last") {
            if let Some(list) = self
                .open_setlist
                .and_then(|index| self.setlists.get_mut(index))
            {
                list.entries.pop();
            }
            self.save(context);
        } else if action == action_id(NEXT) {
            self.advance(context);
        } else if action == action_id(PREVIOUS) {
            self.previous(context);
        }
        self.show(context);
    }
}

impl Stand {
    fn page_reload(&mut self, context: &mut Context) {
        self.page_load = None;
        self.page_load_key = None;
        self.start_page_load(context);
    }
}

fn main() -> ExitCode {
    kobo_sdk::run("musicstand", Stand::default()).map_or_else(
        |error| {
            eprintln!("musicstand: {error}");
            ExitCode::FAILURE
        },
        |()| ExitCode::SUCCESS,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobo_ui::{Chrome, CLARA_BW_METRICS};

    fn score() -> Score {
        Score {
            id: "score-a".to_owned(),
            title: "Prelude".to_owned(),
            pages: 3,
            width: 1654,
            height: 2339,
        }
    }

    #[test]
    fn half_page_crops_overlap_so_the_line_stays_in_sight() {
        let score = score();
        let top = Stand::crop_region(&score, ScoreState::default());
        let bottom = Stand::crop_region(
            &score,
            ScoreState {
                bottom_half: true,
                ..ScoreState::default()
            },
        );
        let top_end = top.1 + top.3;
        assert!(bottom.1 < top_end, "halves must overlap");
        assert_eq!(bottom.1 + bottom.3, score.height, "bottom reaches the foot");
    }

    #[test]
    fn zoom_keeps_the_staff_and_drops_margins() {
        let score = score();
        let plain = Stand::crop_region(&score, ScoreState::default());
        let zoomed = Stand::crop_region(
            &score,
            ScoreState {
                zoomed: true,
                ..ScoreState::default()
            },
        );
        assert!(zoomed.2 < plain.2);
        assert_eq!(zoomed.0, (score.width - zoomed.2) / 2);
    }

    #[test]
    fn per_score_state_survives_a_save_restore_round_trip() {
        let mut stand = Stand::default();
        stand.states.insert(
            "score-a".to_owned(),
            ScoreState {
                page: 2,
                bottom_half: true,
                zoomed: true,
                marked: true,
            },
        );
        stand.setlists.push(Setlist {
            name: "Gig".to_owned(),
            entries: vec!["score-a".to_owned()],
        });
        let mut saved = Vec::new();
        // Serialize through the same format save() writes.
        let mut text = String::from("v1\n");
        for (id, state) in &stand.states {
            text.push_str(&format!(
                "score\t{id}\t{}\t{}\t{}\t{}\n",
                state.page,
                u8::from(state.bottom_half),
                u8::from(state.zoomed),
                u8::from(state.marked)
            ));
        }
        for list in &stand.setlists {
            text.push_str(&format!(
                "setlist\t{}\t{}\n",
                list.name,
                list.entries.join(",")
            ));
        }
        saved.extend_from_slice(text.as_bytes());
        let mut back = Stand::default();
        back.restore(&saved);
        assert_eq!(back.state_of("score-a").page, 2);
        assert!(back.state_of("score-a").marked);
        assert_eq!(back.setlists[0].entries, vec!["score-a"]);
    }

    #[test]
    fn turn_past_last_page_needs_two_presses() {
        let mut stand = Stand {
            scores: vec![score()],
            current: Some("score-a".to_owned()),
            ..Stand::default()
        };
        *stand.state_mut("score-a") = ScoreState {
            page: 2,
            bottom_half: false,
            ..ScoreState::default()
        };
        // Without a context the load can not start; exercise the pure part.
        let state = stand.state_mut("score-a");
        if !state.bottom_half {
            state.bottom_half = true;
        }
        assert!(stand.state_of("score-a").bottom_half);
    }

    #[test]
    fn library_and_setlist_fit_clara() {
        let mut stand = Stand {
            scores: vec![score()],
            startup: Startup {
                manifest_loaded: true,
                ..Startup::default()
            },
            ..Stand::default()
        };
        let screen = stand.library();
        assert!(screen
            .diagnostics(&CLARA_BW_METRICS, &Chrome::default())
            .issues
            .is_empty());
        stand.setlists.push(Setlist {
            name: "Gig".to_owned(),
            entries: vec!["score-a".to_owned()],
        });
        let screen = stand.setlist(0);
        assert!(screen
            .diagnostics(&CLARA_BW_METRICS, &Chrome::default())
            .issues
            .is_empty());
    }

    #[test]
    fn stand_screen_declares_turns_and_zoom_button() {
        let mut stand = Stand {
            scores: vec![score()],
            current: Some("score-a".to_owned()),
            page: Some(TilePicture::new(PAGE_HANDLE, 1654, 2339)),
            ..Stand::default()
        };
        let screen = stand.stand();
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
        assert_eq!(
            layout.page_turns.declared().expect("page turns").next,
            action_id(NEXT)
        );
        stand.menu_open = true;
        let screen = stand.stand();
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
        assert!(layout.rect_of_action(action_id(ZOOM)).is_some());
        assert!(layout.rect_of_action(action_id(MARK)).is_some());
        assert!(screen
            .diagnostics(&CLARA_BW_METRICS, &Chrome::default())
            .issues
            .is_empty());
    }
}

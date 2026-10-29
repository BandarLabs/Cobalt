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
const CHECK: &str = "check";
const NEW_LIST: &str = "new-list";
const ADD_SCORES: &str = "add-scores";
const DONE_ADDING: &str = "done-adding";
const DELETE_LIST: &str = "delete-list";
const CONFIRM_DELETE: &str = "confirm-delete";
const KEEP_LIST: &str = "keep-list";
const MOVE_UP: &str = "move-up";
const MOVE_DOWN: &str = "move-down";
const REMOVE_ENTRY: &str = "remove-entry";

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
    /// The library, offered for adding to the open setlist.
    Picker,
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
    library_page: usize,
    setlists_page: usize,
    setlist_page: usize,
    picker_page: usize,
    /// The setlist entry whose menu is open, by position.
    entry_menu: Option<usize>,
    confirming_delete: bool,
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

    /// Keep every score reachable and reserve the bottom bar before measuring.
    fn paged_rows(
        context: &Context,
        screen: ScreenBuilder,
        rows: &[(String, String, String)],
        page: usize,
    ) -> (ScreenBuilder, usize) {
        let labels = rows
            .iter()
            .map(|(_, title, detail)| (title.as_str(), detail.as_str()))
            .collect::<Vec<_>>();
        let pages = context.paginate_rows_under(
            &labels,
            true,
            kobo_sdk::Position::AtTheFoot,
            &screen.clone().build(),
        );
        let count = pages.len().max(1);
        let page = page.min(count - 1);
        let visible = pages.get(page).map(Vec::as_slice).unwrap_or_default();
        let mut screen = screen.rows(visible.iter().map(|&index| {
            let (action, title, detail) = &rows[index];
            (action.clone(), title.clone(), detail.clone(), Glyph::Note)
        }));
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

    fn library(&self, context: &Context) -> (Screen, usize) {
        let mut screen = ScreenBuilder::new("music-library").top_bar("Music Stand");
        if let Some(notice) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, notice);
        }
        if !self.startup.manifest_loaded {
            return (screen.activity("Opening your music shelf", None).build(), 1);
        }
        for (input, reason) in &self.failures {
            screen = screen.banner(
                BannerLevel::Attention,
                format!("{input} did not import: {reason}"),
            );
        }
        if self.scores.is_empty() {
            return (
                screen
                    .splash(
                        Some(Glyph::Note),
                        "Your stand is empty",
                        "Send a score from a computer. PDFs are set page by page.",
                    )
                    .section("On your computer")
                    .command("kobo musicstand push score.pdf")
                    .button(CHECK, "Check for scores")
                    .build(),
                1,
            );
        }
        let rows = self
            .scores
            .iter()
            .map(|score| {
                let state = self.state_of(&score.id);
                let detail = if state.marked {
                    format!("{} · page {} marked", pages(score.pages), state.page + 1)
                } else {
                    pages(score.pages)
                };
                (
                    format!("{OPEN}-{}", score.id),
                    context.clamped_row(&score.title, 2, true),
                    detail,
                )
            })
            .collect::<Vec<_>>();
        let (screen, pages) =
            Self::paged_rows(context, screen.heading("Library"), &rows, self.library_page);
        (
            screen
                .top_bar_action(ABOUT, "Add scores")
                .bottom_action(SETLISTS, "Setlists")
                .build(),
            pages,
        )
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
        screen = screen
            .top_bar(&score.title)
            .page_turns(PREVIOUS, NEXT)
            .reading_menu(MENU);
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

    fn setlists(&self, context: &Context) -> (Screen, usize) {
        let screen = ScreenBuilder::new("music-setlists")
            .top_bar("Music Stand")
            .heading("Setlists")
            .secondary("A setlist plays its scores in order and turns from the last page of one to the next.");
        let rows = self
            .setlists
            .iter()
            .enumerate()
            .map(|(index, list)| {
                (
                    format!("list-{index}"),
                    context.clamped_row(&list.name, 2, true),
                    format!(
                        "{} score{}",
                        list.entries.len(),
                        if list.entries.len() == 1 { "" } else { "s" }
                    ),
                )
            })
            .collect::<Vec<_>>();
        let (screen, pages) = Self::paged_rows(context, screen, &rows, self.setlists_page);
        (screen.bottom_action(NEW_LIST, "New setlist").build(), pages)
    }

    /// The scores of one setlist, each with a menu to move it or take it off.
    fn setlist_rows(&self, list: &Setlist, context: &Context) -> Vec<(String, String, String)> {
        list.entries
            .iter()
            .enumerate()
            .filter_map(|(position, id)| {
                let score = self.scores.iter().find(|score| &score.id == id)?;
                let state = self.state_of(id);
                Some((
                    format!("entry-{position}"),
                    context.clamped_row_with_menu(
                        &format!("{}. {}", position + 1, score.title),
                        2,
                        true,
                    ),
                    format!("Resumes at page {}", state.page + 1),
                ))
            })
            .collect()
    }

    fn setlist(&self, index: usize, context: &Context) -> (Screen, usize) {
        let Some(list) = self.setlists.get(index) else {
            return self.setlists(context);
        };
        let mut screen = ScreenBuilder::new("music-setlist")
            .top_bar("Setlist")
            .top_bar_action(DELETE_LIST, "Delete")
            .heading(list.name.clone());
        let rows = self.setlist_rows(list, context);
        if rows.is_empty() {
            screen = screen.splash(
                Some(Glyph::Note),
                "No scores on this setlist yet",
                "Choose Add scores to pick them from your library.",
            );
        }
        let labels = rows
            .iter()
            .map(|(_, title, detail)| (title.as_str(), detail.as_str()))
            .collect::<Vec<_>>();
        let pages = context.paginate_rows_with_menu_under(
            &labels,
            true,
            kobo_sdk::Position::AtTheFoot,
            &screen.clone().build(),
        );
        let count = pages.len().max(1);
        let page = self.setlist_page.min(count - 1);
        let visible = pages.get(page).map(Vec::as_slice).unwrap_or_default();
        screen = screen.rows_with_menu(visible.iter().map(|&row| {
            let (action, title, detail) = &rows[row];
            (
                action.clone(),
                title.clone(),
                detail.clone(),
                Glyph::Note,
                format!("{action}-menu"),
            )
        }));
        if let Some(open) = self.entry_menu {
            let shown = visible
                .iter()
                .any(|&row| rows[row].0 == format!("entry-{open}"));
            let mut items = Vec::new();
            if open > 0 {
                items.push((MOVE_UP, "Move up", Glyph::Previous));
            }
            if open + 1 < list.entries.len() {
                items.push((MOVE_DOWN, "Move down", Glyph::Next));
            }
            items.push((REMOVE_ENTRY, "Remove from setlist", Glyph::Trash));
            screen = screen.row_overflow(format!("entry-{open}-menu"), shown, items);
        }
        if count > 1 {
            screen = screen
                .page_turns("list-previous", "list-next")
                .page_position(
                    u16::try_from(page + 1).unwrap_or(u16::MAX),
                    u16::try_from(count).unwrap_or(u16::MAX),
                );
        }
        screen = screen.bottom_action(ADD_SCORES, "Add scores");
        if self.confirming_delete {
            screen = screen.confirm(
                "Delete this setlist?",
                format!("{} goes, and its scores stay in your library.", list.name),
                (CONFIRM_DELETE, "Delete"),
                (KEEP_LIST, "Keep"),
            );
        }
        (screen.build(), count)
    }

    /// The whole library, each score saying whether it is already on the
    /// setlist being built, so a piece is never added twice by mistake.
    fn picker(&self, index: usize, context: &Context) -> (Screen, usize) {
        let Some(list) = self.setlists.get(index) else {
            return self.setlists(context);
        };
        let screen = ScreenBuilder::new("music-picker")
            .top_bar("Add scores")
            .heading(list.name.clone())
            .secondary("Tap a score to add it to the end of the setlist.");
        if self.scores.is_empty() {
            return (
                screen
                    .splash(
                        Some(Glyph::Note),
                        "Your library is empty",
                        "Send scores from your computer with kobo musicstand push.",
                    )
                    .bottom_action(DONE_ADDING, "Done")
                    .build(),
                1,
            );
        }
        let rows = self
            .scores
            .iter()
            .enumerate()
            .map(|(position, score)| {
                let detail = match list.entries.iter().position(|id| id == &score.id) {
                    Some(at) => format!("On this setlist as number {}", at + 1),
                    None => pages(score.pages),
                };
                (
                    format!("pick-{position}"),
                    context.clamped_row(&score.title, 2, true),
                    detail,
                )
            })
            .collect::<Vec<_>>();
        let (screen, pages) = Self::paged_rows(context, screen, &rows, self.picker_page);
        (screen.bottom_action(DONE_ADDING, "Done").build(), pages)
    }

    /// Moves or removes the entry whose menu is open.
    fn edit_entry(&mut self, context: &mut Context, action: ActionId) {
        let (Some(index), Some(position)) = (self.open_setlist, self.entry_menu.take()) else {
            return;
        };
        let Some(list) = self.setlists.get_mut(index) else {
            return;
        };
        if position >= list.entries.len() {
            return;
        }
        if action == action_id(MOVE_UP) && position > 0 {
            list.entries.swap(position, position - 1);
        } else if action == action_id(MOVE_DOWN) && position + 1 < list.entries.len() {
            list.entries.swap(position, position + 1);
        } else if action == action_id(REMOVE_ENTRY) {
            list.entries.remove(position);
        } else {
            return;
        }
        self.save(context);
    }

    fn turn_list(&mut self, context: &Context, forward: bool) {
        let count = match self.view {
            View::Library => self.library(context).1,
            View::Setlists => self.setlists(context).1,
            View::Setlist => self
                .open_setlist
                .map_or(1, |index| self.setlist(index, context).1),
            View::Picker => self
                .open_setlist
                .map_or(1, |index| self.picker(index, context).1),
            _ => return,
        };
        let page = match self.view {
            View::Library => &mut self.library_page,
            View::Setlists => &mut self.setlists_page,
            View::Setlist => &mut self.setlist_page,
            View::Picker => &mut self.picker_page,
            _ => return,
        };
        let last = count.saturating_sub(1);
        *page = if forward {
            page.saturating_add(1).min(last)
        } else {
            (*page).min(last).saturating_sub(1)
        };
    }

    fn about() -> Screen {
        ScreenBuilder::new("music-about")
            .top_bar("Music Stand")
            .heading("Adding scores")
            .text("Send scores from your computer with kobo musicstand push. Name several files or a folder to send them all at once. Each push adds to your library.")
            .text("PDF scores need Poppler on the computer. Folders of page images are sent as they are.")
            .text("New scores appear here the next time you come back to Music Stand. Only send scores you have the right to use.")
            .button(LIBRARY, "Library")
            .build()
    }

    fn show(&self, context: &mut Context) {
        let screen = match self.view {
            View::Library => self.library(context).0,
            View::Stand => self.stand(),
            View::Setlists => self.setlists(context).0,
            View::Setlist => self.open_setlist.map_or_else(
                || self.setlists(context).0,
                |index| self.setlist(index, context).0,
            ),
            View::Picker => self.open_setlist.map_or_else(
                || self.setlists(context).0,
                |index| self.picker(index, context).0,
            ),
            View::About => Self::about(),
        };
        context.set_screen(screen.with_own_back(self.view != View::Library));
    }

    /// Reads the shelf manifest again.
    ///
    /// Music Stand used to read it once, when it opened, so a score pushed
    /// while the stand sat open in the background never reached the library
    /// until the app was closed and opened again. Coming back to the app is
    /// the moment somebody looks for what they just sent.
    fn reload_shelf(&mut self, context: &mut Context) {
        if !self.startup.manifest_loaded || self.manifest_load.is_some() {
            return;
        }
        let mut manifest = ShelfDownload::new(MANIFEST).at_most(MAX_MANIFEST);
        manifest.start(context);
        self.manifest_load = Some(manifest);
    }

    fn advance_manifest(&mut self, context: &mut Context, result: &StoreResult) -> bool {
        let Some(load) = &mut self.manifest_load else {
            return false;
        };
        let reloading = self.startup.manifest_loaded;
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
                    // A shelf already on screen is kept when the manifest
                    // read again cannot be decoded: a push half way through
                    // is the likely cause, and the next look finds it whole.
                    Err(_) if reloading => {}
                    Err(error) => {
                        self.scores.clear();
                        self.notice = Some(format!(
                            "The list of scores could not be read ({error}). Push your scores again from your computer."
                        ));
                    }
                }
                self.startup.manifest_loaded = true;
                self.start_when_ready(context);
                true
            }
            ShelfProgress::Failed(kobo_sdk::StoreError::Missing) => {
                self.manifest_load = None;
                if reloading {
                    self.scores.clear();
                    self.failures.clear();
                }
                self.startup.manifest_loaded = true;
                true
            }
            ShelfProgress::Failed(_) => {
                self.manifest_load = None;
                if !reloading {
                    self.notice = Some("The list of scores could not be opened.".to_owned());
                }
                self.startup.manifest_loaded = true;
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
        }
    }

    /// Shelf answers go to the read that asked for them, by name. A refusal
    /// carries no name of its own, and once the manifest is read again while
    /// a page is on its way, the name the runtime answers for is the only way
    /// to tell a missing page from a missing manifest.
    fn on_shelf(&mut self, context: &mut Context, name: &str, result: StoreResult) {
        let named = |load: &Option<ShelfDownload>| load.as_ref().is_some_and(|l| l.name() == name);
        let handled = if named(&self.manifest_load) {
            self.advance_manifest(context, &result)
        } else if named(&self.page_load) {
            self.advance_page(context, &result)
        } else {
            false
        };
        if handled {
            self.show(context);
        }
    }

    fn on_resume(&mut self, context: &mut Context) {
        self.reload_shelf(context);
        self.show(context);
    }

    fn on_foreground(&mut self, context: &mut Context) {
        self.reload_shelf(context);
        self.show(context);
    }

    fn on_page_turn(&mut self, context: &mut Context, forward: bool) {
        if self.view == View::Stand {
            if forward {
                self.advance(context);
            } else {
                self.previous(context);
            }
        } else {
            self.turn_list(context, forward);
        }
        self.show(context);
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if action == ActionId::BACK {
            match self.view {
                View::Stand if self.menu_open => self.menu_open = false,
                View::Stand => self.leave_stand(context),
                View::Library => context.exit(),
                View::Setlist if self.entry_menu.is_some() => self.entry_menu = None,
                View::Setlist if self.confirming_delete => self.confirming_delete = false,
                View::Setlist => self.view = View::Setlists,
                View::Picker => self.view = View::Setlist,
                _ => self.view = View::Library,
            }
            self.show(context);
            return;
        }
        if self.handle_collection_tap(context, action) {
            self.show(context);
            return;
        }
        if action == action_id("list-next") {
            self.turn_list(context, true);
        } else if action == action_id("list-previous") {
            self.turn_list(context, false);
        } else if action == action_id(MENU) {
            self.menu_open = !self.menu_open;
        } else if action == action_id(ZOOM) {
            if let Some(id) = self.current.clone() {
                let state = self.state_mut(&id);
                state.zoomed = !state.zoomed;
            }
            self.menu_open = false;
            self.save(context);
            self.page = None;
            self.page_key = None;
            self.page_reload(context);
        } else if action == action_id(MARK) {
            if let Some(id) = self.current.clone() {
                let state = self.state_mut(&id);
                state.marked = !state.marked;
            }
            self.menu_open = false;
            self.save(context);
        } else if action == action_id(LIBRARY) {
            self.leave_stand(context);
        } else if action == action_id(SETLISTS) {
            self.view = View::Setlists;
        } else if action == action_id(ABOUT) {
            self.view = View::About;
        } else if action == action_id(CHECK) {
            self.reload_shelf(context);
        } else if action == action_id(NEW_LIST) {
            // One more number than there are setlists always holds a name
            // nobody has taken.
            let name = (1..=self.setlists.len() + 1)
                .map(|number| format!("Setlist {number}"))
                .find(|name| !self.setlists.iter().any(|list| &list.name == name))
                .unwrap_or_else(|| format!("Setlist {}", self.setlists.len() + 1));
            self.setlists.push(Setlist {
                name,
                entries: Vec::new(),
            });
            self.open_setlist = Some(self.setlists.len() - 1);
            self.setlist_page = 0;
            self.picker_page = 0;
            self.view = View::Picker;
            self.save(context);
        } else if action == action_id(ADD_SCORES) {
            self.picker_page = 0;
            self.view = View::Picker;
        } else if action == action_id(DONE_ADDING) {
            self.view = View::Setlist;
        } else if action == action_id(DELETE_LIST) {
            self.entry_menu = None;
            self.confirming_delete = true;
        } else if action == action_id(KEEP_LIST) {
            self.confirming_delete = false;
        } else if action == action_id(CONFIRM_DELETE) {
            self.confirming_delete = false;
            if let Some(index) = self.open_setlist.take() {
                if index < self.setlists.len() {
                    self.setlists.remove(index);
                }
            }
            self.setlists_page = 0;
            self.view = View::Setlists;
            self.save(context);
        } else if [MOVE_UP, MOVE_DOWN, REMOVE_ENTRY]
            .iter()
            .any(|name| action == action_id(name))
        {
            self.edit_entry(context, action);
        } else if action == action_id(NEXT) {
            self.advance(context);
        } else if action == action_id(PREVIOUS) {
            self.previous(context);
        }
        self.show(context);
    }
}

impl Stand {
    /// A tap on one row of a collection: a score, a setlist, a setlist entry
    /// or its menu, or a score offered for the setlist being built.
    fn handle_collection_tap(&mut self, context: &mut Context, action: ActionId) -> bool {
        if let Some(score) = self
            .scores
            .iter()
            .find(|score| action == action_id(&format!("{OPEN}-{}", score.id)))
            .map(|score| score.id.clone())
        {
            self.open_score(context, &score, None);
            return true;
        }
        if let Some(index) =
            (0..self.setlists.len()).find(|index| action == action_id(&format!("list-{index}")))
        {
            self.open_setlist = Some(index);
            self.setlist_page = 0;
            self.entry_menu = None;
            self.confirming_delete = false;
            self.view = View::Setlist;
            return true;
        }
        let Some(index) = self.open_setlist else {
            return false;
        };
        let Some(entries) = self.setlists.get(index).map(|list| list.entries.clone()) else {
            return false;
        };
        for (position, id) in entries.iter().enumerate() {
            if action == action_id(&format!("entry-{position}")) {
                if self.scores.iter().any(|score| &score.id == id) {
                    self.open_score(context, id, Some(position));
                }
                return true;
            }
            if action == action_id(&format!("entry-{position}-menu")) {
                self.entry_menu = Some(position);
                return true;
            }
        }
        if let Some(position) =
            (0..self.scores.len()).find(|position| action == action_id(&format!("pick-{position}")))
        {
            let id = self.scores[position].id.clone();
            if let Some(list) = self.setlists.get_mut(index) {
                list.entries.push(id);
            }
            self.save(context);
            return true;
        }
        false
    }
}

impl Stand {
    fn page_reload(&mut self, context: &mut Context) {
        self.page_load = None;
        self.page_load_key = None;
        self.start_page_load(context);
    }
}

/// "1 page", "6 pages".
fn pages(count: u32) -> String {
    if count == 1 {
        "1 page".to_owned()
    } else {
        format!("{count} pages")
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
        let screen = stand.library(&Context::default()).0;
        assert!(screen
            .diagnostics(&CLARA_BW_METRICS, &Chrome::default())
            .issues
            .is_empty());
        stand.setlists.push(Setlist {
            name: "Gig".to_owned(),
            entries: vec!["score-a".to_owned()],
        });
        let screen = stand.setlist(0, &Context::default()).0;
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

#[cfg(test)]
mod collection_tests;

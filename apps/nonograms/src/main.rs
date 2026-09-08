//! Touch-first nonograms with a line-solver fairness invariant.

mod corpus;
mod photo;
#[cfg(test)]
mod quality_tests;
mod saved;
mod solver;
mod view;
use kobo_state::draft::{Draft, Status};

use corpus::Puzzle;
use kobo_image::Picture;
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, KoboApp, PictureHandle, Screen, ScreenBuilder,
    StoreResult, Task, TaskId, TaskOutcome, TilePicture,
};
use solver::{candidates, Cell};
use std::collections::BTreeSet;
use std::process::ExitCode;

const SOLVED: &str = "solved";
const PHOTO_FILE: &str = "photo.png";
const REVEAL: PictureHandle = PictureHandle(41);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Route {
    #[default]
    Browser,
    Play,
    Gate,
    Photo,
    PhotoHelp,
    Reveal,
    HowTo,
    Menu,
    Clue,
    Restart,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Pack {
    #[default]
    Pictures,
    Earlier,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Mark {
    #[default]
    Blank,
    Fill,
    Cross,
}

impl Mark {
    const fn next(self) -> Self {
        match self {
            Self::Blank => Self::Fill,
            Self::Fill => Self::Cross,
            Self::Cross => Self::Blank,
        }
    }

    const fn cell(self) -> Cell {
        match self {
            Self::Blank => Cell::Unknown,
            Self::Fill => Cell::Filled,
            Self::Cross => Cell::Empty,
        }
    }

    const fn stored(self) -> u8 {
        match self {
            Self::Blank => b'.',
            Self::Fill => b'#',
            Self::Cross => b'x',
        }
    }

    const fn read(value: u8) -> Option<Self> {
        match value {
            b'.' => Some(Self::Blank),
            b'#' => Some(Self::Fill),
            b'x' => Some(Self::Cross),
            _ => None,
        }
    }
}

#[allow(
    clippy::struct_excessive_bools,
    reason = "independent play preferences and storage lifecycle flags"
)]
struct Game {
    route: Route,
    pack: Pack,
    puzzles: Vec<Puzzle>,
    selected: Option<usize>,
    marks: Vec<Mark>,
    guided: bool,
    done: bool,
    solved: BTreeSet<String>,
    page: usize,
    notice: Option<String>,
    waiting: Option<TaskId>,
    photo_side: usize,
    reveal: Option<TilePicture>,
    photo_reveal: Option<(String, Picture)>,
    run_entry: bool,
    run_start: Option<usize>,
    focus: Option<usize>,
    viewport: Option<kobo_sdk::board::BoardViewport>,
    clue: Option<(String, String)>,
    help_page: usize,
    undo: std::collections::VecDeque<Vec<Mark>>,
    draft: Draft,
    active_save: Option<u64>,
    solved_draft: Draft,
    solved_save: Option<u64>,
    progress_loading: bool,
    load_error: Option<String>,
}

impl Default for Game {
    fn default() -> Self {
        Self {
            route: Route::Browser,
            pack: Pack::Pictures,
            puzzles: corpus::catalog(),
            selected: None,
            marks: Vec::new(),
            guided: false,
            done: false,
            solved: BTreeSet::new(),
            page: 0,
            notice: None,
            waiting: None,
            photo_side: 9,
            reveal: None,
            photo_reveal: None,
            run_entry: false,
            run_start: None,
            focus: None,
            viewport: None,
            clue: None,
            help_page: 0,
            undo: std::collections::VecDeque::new(),
            draft: Draft::restored(Vec::new(), saved::LIMIT).expect("empty draft"),
            active_save: None,
            solved_draft: Draft::restored(Vec::new(), 16 * 1024).expect("empty index"),
            solved_save: None,
            progress_loading: false,
            load_error: None,
        }
    }
}

impl Game {
    fn show(&self, context: &mut Context) {
        context.set_screen(
            self.screen(context)
                .with_own_back(self.route != Route::Browser),
        );
    }

    fn screen(&self, context: &Context) -> Screen {
        if self.progress_loading && self.route == Route::Play {
            let mut screen = ScreenBuilder::new("nonograms-loading").top_bar("Nonograms");
            if self.load_error.is_some() {
                screen = screen
                    .heading("Cannot open game")
                    .text("Your saved game was kept. Try reading it again.")
                    .button("retry-load", "Retry")
                    .button("back-browser", "Puzzles");
            } else {
                screen = screen.text("Opening game…");
            }
            return screen.build();
        }
        match self.route {
            Route::Browser => self.browser(context),
            Route::Play => self.play(context),
            Route::Gate => ScreenBuilder::new("nonograms-size-gate")
                .top_bar("Nonograms")
                .text("This grid is not supported. Choose a size from 5×5 to 25×25.")
                .button("back-browser", "Puzzles")
                .build(),
            Route::Photo => self.photo(),
            Route::Reveal => self.reveal_screen(context),
            Route::HowTo | Route::PhotoHelp => self.help(context),
            Route::Menu => self.menu(),
            Route::Clue => self.clue_screen(context),
            Route::Restart => ScreenBuilder::new("nonograms-restart")
                .top_bar("Restart puzzle?")
                .text("Clear every mark. You can undo this.")
                .buttons([("confirm-reset", "Restart"), ("resume", "Resume")])
                .build(),
        }
    }

    fn visible_puzzles(&self) -> Vec<usize> {
        self.puzzles
            .iter()
            .enumerate()
            .filter(|(_, puzzle)| puzzle.id.starts_with("pack-") == (self.pack == Pack::Earlier))
            .map(|(index, _)| index)
            .collect()
    }

    fn browser_detail(&self, index: usize) -> String {
        let puzzle = &self.puzzles[index];
        format!(
            "{} · {}×{} · {}",
            if self.solved.contains(&puzzle.id) {
                "Solved"
            } else {
                "Not started"
            },
            puzzle.side,
            puzzle.side,
            if puzzle.side > 9 {
                "Pan to play"
            } else {
                puzzle.difficulty()
            }
        )
    }

    fn browser_pages(&self, context: &Context) -> Vec<Vec<usize>> {
        let indices = self.visible_puzzles();
        let details: Vec<_> = indices
            .iter()
            .map(|&index| self.browser_detail(index))
            .collect();
        let rows: Vec<_> = indices
            .iter()
            .zip(&details)
            .map(|(&index, detail)| (self.puzzles[index].title.as_str(), detail.as_str(), ""))
            .collect();
        context
            .paginate_rows_below_section(&rows, true, kobo_sdk::Position::AtTheFoot, None)
            .into_iter()
            .map(|page| page.into_iter().map(|index| indices[index]).collect())
            .collect()
    }

    fn browser(&self, context: &Context) -> Screen {
        let pages = self.browser_pages(context);
        let page = self.page.min(pages.len().saturating_sub(1));
        let visible = pages.get(page).map(Vec::as_slice).unwrap_or_default();
        ScreenBuilder::new("nonograms-browser")
            .top_bar("Nonograms")
            .section_with_value(
                if self.pack == Pack::Pictures {
                    "Picture puzzles"
                } else {
                    "Earlier puzzles"
                },
                format!("{} puzzles", self.visible_puzzles().len()),
            )
            .rows(visible.iter().map(|&index| {
                let puzzle = &self.puzzles[index];
                (
                    format!("puzzle-{index}"),
                    puzzle.title.clone(),
                    self.browser_detail(index),
                    kobo_sdk::Glyph::Grid,
                )
            }))
            .page_turns("previous-page", "next-page")
            .page_position(
                u16::try_from(page + 1).unwrap_or(u16::MAX),
                u16::try_from(pages.len().max(1)).unwrap_or(u16::MAX),
            )
            .action_bar([
                ("photo", "Photos"),
                ("how-to-play", "Help"),
                (
                    "pack-toggle",
                    if self.pack == Pack::Pictures {
                        "Earlier"
                    } else {
                        "Pictures"
                    },
                ),
            ])
            .build()
    }

    fn photo(&self) -> Screen {
        let mut screen = ScreenBuilder::new("nonograms-photo")
            .top_bar("Photo puzzle")
            .text("Choose the size used on your computer.")
            .facts([("Grid", format!("{}×{}", self.photo_side, self.photo_side))]);
        if let Some(notice) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, notice);
        }
        screen
            .buttons([("photo-size", "Size"), ("photo-open", "Open")])
            .action_bar([("photo-help", "Help"), ("back-browser", "Puzzles")])
            .build()
    }

    fn reveal_screen(&self, context: &Context) -> Screen {
        let controls = |screen: ScreenBuilder| {
            screen
                .button_with_state(
                    "undo",
                    "Undo",
                    if self.undo.is_empty() {
                        kobo_sdk::ControlState::Disabled
                    } else {
                        kobo_sdk::ControlState::Enabled
                    },
                )
                .button("next-puzzle", "Puzzles")
        };
        let builder = || ScreenBuilder::new("nonograms-reveal").top_bar("Puzzle complete");
        let metrics = context.metrics();
        let skeleton = controls(builder()).build();
        let layout = skeleton.layout_with(&metrics, &kobo_sdk::Chrome::measuring(true));
        let available =
            layout.content.height - layout.flow_height - metrics.space(kobo_sdk::Space::Medium) * 2;
        let height_mm =
            u16::try_from((available * 254 / (metrics.pixels_per_inch * 10)).max(1)).unwrap_or(1);
        let screen = if let Some(reveal) = self.reveal {
            builder().unframed_picture(reveal, height_mm)
        } else {
            builder().text("Every square is complete.")
        };
        controls(screen).build()
    }

    fn puzzle(&self) -> Option<&Puzzle> {
        self.selected.and_then(|index| self.puzzles.get(index))
    }

    fn progress_key(&self) -> Option<String> {
        Some(format!("progress-{}", self.puzzle()?.id))
    }

    fn select(&mut self, context: &mut Context, index: usize) {
        let Some(puzzle) = self.puzzles.get(index) else {
            return;
        };
        self.pack = if puzzle.id.starts_with("pack-") {
            Pack::Earlier
        } else {
            Pack::Pictures
        };
        self.selected = Some(index);
        self.focus = Some(0);
        self.viewport = None;
        self.undo.clear();
        self.draft = Draft::restored(Vec::new(), saved::LIMIT).expect("empty draft");
        self.active_save = None;
        self.load_error = None;
        self.progress_loading = true;
        self.marks = vec![Mark::Blank; puzzle.side * puzzle.side];
        self.done = false;
        self.notice = None;
        if self
            .photo_reveal
            .as_ref()
            .is_some_and(|(id, _)| id != &puzzle.id)
        {
            self.photo_reveal = None;
        }
        self.run_start = None;
        if !(5..=25).contains(&puzzle.side) {
            self.route = Route::Gate;
            return;
        }
        self.route = Route::Play;
        if let Some(key) = self.progress_key() {
            context.store().load(key);
        }
    }

    fn save_progress(&mut self, context: &mut Context) {
        let bytes = saved::encode(self).expect("bounded game");
        self.draft.replace(bytes).expect("bounded draft");
        self.pump(context);
    }

    fn pump(&mut self, context: &mut Context) {
        if let Some(key) = self.progress_key() {
            if let Some(write) = self.draft.begin() {
                self.active_save = Some(write.revision);
                context.store().save(key, write.bytes);
            }
        }
        if let Some(write) = self.solved_draft.begin() {
            self.solved_save = Some(write.revision);
            context.store().save(SOLVED, write.bytes);
        }
    }

    fn restore_progress(&mut self, bytes: &[u8]) {
        if saved::restore(self, bytes).is_err() {
            self.load_error = Some("Saved game could not be read.".into());
        }
    }

    fn toggle(&mut self, context: &mut Context, cell: usize) {
        if self.done || cell >= self.marks.len() {
            return;
        }
        self.remember();
        self.focus = Some(cell);
        self.marks[cell] = self.marks[cell].next();
        self.finish_move(context);
    }

    fn enter_run(&mut self, context: &mut Context, cell: usize) {
        let Some(side) = self.puzzle().map(|puzzle| puzzle.side) else {
            return;
        };
        if self.done || cell >= self.marks.len() {
            return;
        }
        self.focus = Some(cell);
        let Some(start) = self.run_start.take() else {
            self.run_start = Some(cell);
            self.notice = Some(format!(
                "Run starts at row {}, column {}.",
                cell / side + 1,
                cell % side + 1
            ));
            return;
        };
        let (start_row, start_column) = (start / side, start % side);
        let (row, column) = (cell / side, cell % side);
        if start_row == row || start_column == column {
            self.remember();
        }
        if start_row == row {
            for column in start_column.min(column)..=start_column.max(column) {
                self.marks[row * side + column] = Mark::Fill;
            }
        } else if start_column == column {
            for row in start_row.min(row)..=start_row.max(row) {
                self.marks[row * side + column] = Mark::Fill;
            }
        } else {
            self.notice = Some("A run must stay in one row or column.".to_owned());
            return;
        }
        self.finish_move(context);
    }

    fn finish_move(&mut self, context: &mut Context) {
        self.notice = self.guided_contradiction();
        self.save_progress(context);
        if self.completed() {
            self.done = true;
            self.solved
                .insert(self.puzzle().expect("active puzzle").id.clone());
            self.solved_draft
                .replace(encode_solved(&self.solved))
                .expect("bounded solved index");
            self.pump(context);
            self.show_reveal(context);
        }
    }

    fn guided_contradiction(&self) -> Option<String> {
        if !self.guided {
            return None;
        }
        let puzzle = self.puzzle()?;
        for (row, clues) in puzzle.row_clues().iter().enumerate() {
            let line = (0..puzzle.side)
                .map(|column| self.marks[row * puzzle.side + column].cell())
                .collect::<Vec<_>>();
            if candidates(puzzle.side, clues, &line).is_empty() {
                return Some(format!("Contradiction in row {}.", row + 1));
            }
        }
        for (column, clues) in puzzle.column_clues().iter().enumerate() {
            let line = (0..puzzle.side)
                .map(|row| self.marks[row * puzzle.side + column].cell())
                .collect::<Vec<_>>();
            if candidates(puzzle.side, clues, &line).is_empty() {
                return Some(format!("Contradiction in column {}.", column + 1));
            }
        }
        None
    }

    fn completed(&self) -> bool {
        let Some(puzzle) = self.puzzle() else {
            return false;
        };
        self.marks.len() == puzzle.answer.len()
            && self
                .marks
                .iter()
                .zip(&puzzle.answer)
                .all(|(mark, answer)| *mark != Mark::Blank && (*mark == Mark::Fill) == *answer)
    }

    fn show_reveal(&mut self, context: &mut Context) {
        let picture = self
            .photo_reveal
            .as_ref()
            .filter(|(id, _)| self.puzzle().is_some_and(|puzzle| puzzle.id == *id))
            .map(|(_, picture)| picture.clone())
            .or_else(|| self.puzzle().and_then(reveal_for));
        self.reveal = picture.and_then(|picture| {
            context.put_picture(
                REVEAL,
                picture.width(),
                picture.height(),
                picture.grey().to_vec(),
            )
        });
        self.route = Route::Reveal;
    }

    fn open_photo(&mut self, context: &mut Context) {
        if !(5..=25).contains(&self.photo_side) {
            self.notice = Some("Choose a photo grid between 5×5 and 25×25.".to_owned());
            return;
        }
        self.cancel_photo_task(context);
        if let Some(task) = context.spawn(Task::ReadFile {
            path: PHOTO_FILE.to_owned(),
        }) {
            self.waiting = Some(task);
            self.notice = Some("Reading imported photo.".to_owned());
        }
    }

    fn cancel_photo_task(&mut self, context: &mut Context) {
        if let Some(task) = self.waiting.take() {
            context.cancel(task);
        }
    }

    fn load_photo(&mut self, context: &mut Context, bytes: &[u8]) {
        let id = photo_id(bytes, self.photo_side);
        match photo::from_photo(id, "Imported photo", bytes, self.photo_side) {
            Ok(photo) => {
                let id = photo.puzzle.id.clone();
                self.puzzles
                    .retain(|puzzle| !puzzle.id.starts_with("photo-"));
                self.puzzles.push(photo.puzzle);
                self.filter_solved();
                self.pack = Pack::Pictures;
                self.selected = Some(self.puzzles.len() - 1);
                self.focus = Some(0);
                self.viewport = None;
                self.undo.clear();
                self.draft = Draft::restored(Vec::new(), saved::LIMIT).expect("empty draft");
                self.active_save = None;
                self.load_error = None;
                self.progress_loading = true;
                self.marks = vec![Mark::Blank; self.photo_side * self.photo_side];
                self.done = false;
                self.notice = None;
                self.route = Route::Play;
                // The reveal remains in memory while the imported puzzle is
                // solved; its source never needs to become a credential or log.
                self.photo_reveal = Some((id, photo.reveal));
                if let Some(key) = self.progress_key() {
                    context.store().load(key);
                }
                context.store().load(SOLVED);
            }
            Err(error) => self.notice = Some(error.to_string()),
        }
    }

    fn filter_solved(&mut self) {
        self.solved
            .retain(|id| self.puzzles.iter().any(|puzzle| puzzle.id == *id));
    }
}

fn reveal_for(puzzle: &Puzzle) -> Option<Picture> {
    const WIDTH: u32 = 536;
    const HEIGHT: u32 = 536;
    let width = WIDTH as usize;
    let height = HEIGHT as usize;
    let grey = (0..height)
        .flat_map(|y| {
            (0..width).map(move |x| {
                let row = y * puzzle.side / height;
                let column = x * puzzle.side / width;
                // Show the actual completed grid without invented shading.
                if x * puzzle.side % width < puzzle.side || y * puzzle.side % height < puzzle.side {
                    170
                } else if puzzle.answer[row * puzzle.side + column] {
                    0
                } else {
                    255
                }
            })
        })
        .collect();
    Picture::from_grey(WIDTH, HEIGHT, grey).ok()
}

fn photo_id(bytes: &[u8], side: usize) -> String {
    let digest = kobo_net::sha256::hex_digest(bytes);
    format!("photo-{side}-{}", &digest[..24])
}

fn encode_solved(solved: &BTreeSet<String>) -> Vec<u8> {
    solved
        .iter()
        .cloned()
        .collect::<Vec<_>>()
        .join("\n")
        .into_bytes()
}

fn decode_solved(bytes: &[u8], puzzles: &[Puzzle]) -> BTreeSet<String> {
    std::str::from_utf8(bytes)
        .ok()
        .map(|text| {
            text.lines()
                .filter(|id| puzzles.iter().any(|puzzle| puzzle.id == *id))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

impl KoboApp for Game {
    fn on_start(&mut self, context: &mut Context) {
        context.store().load(SOLVED);
        self.show(context);
    }

    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        if let StoreResult::Loaded { key, value } = result {
            if key == SOLVED {
                // Do not let a late index read overwrite a completed puzzle
                // whose index replacement is already in flight.
                if matches!(self.solved_draft.status(), Status::Saved) {
                    self.solved = value
                        .as_deref()
                        .map(|bytes| decode_solved(bytes, &self.puzzles))
                        .unwrap_or_default();
                    self.solved_draft = Draft::restored(encode_solved(&self.solved), 16 * 1024)
                        .expect("bounded index");
                }
            } else if self.progress_key().as_deref() == Some(&key) {
                if !self.progress_loading {
                    return;
                }
                if let Some(value) = value {
                    self.restore_progress(&value);
                    if self.load_error.is_none() {
                        self.draft =
                            Draft::restored(value, saved::LIMIT).expect("validated record");
                    }
                }
                self.progress_loading = self.load_error.is_some();
                if !self.progress_loading && self.done {
                    self.show_reveal(context);
                }
            }
            self.show(context);
        }
    }

    fn on_load(&mut self, context: &mut Context, key: &str, result: StoreResult) {
        if matches!(result, StoreResult::Loaded { .. }) {
            self.on_store(context, result);
        } else if self.progress_key().as_deref() == Some(key) {
            self.load_error = Some("Storage could not be read.".into());
            self.show(context);
        }
    }

    fn on_save(&mut self, context: &mut Context, key: &str, result: StoreResult) {
        if key == SOLVED {
            if let Some(revision) = self.solved_save.take() {
                self.solved_draft.finish(
                    revision,
                    if matches!(result, StoreResult::Saved { .. }) {
                        Ok(())
                    } else {
                        Err("Storage could not be written.".into())
                    },
                );
                self.pump(context);
                self.show(context);
            }
            return;
        }
        if self.progress_key().as_deref() != Some(key) {
            return;
        }
        if let Some(revision) = self.active_save.take() {
            self.draft.finish(
                revision,
                if matches!(result, StoreResult::Saved { .. }) {
                    Ok(())
                } else {
                    Err("Storage could not be written.".into())
                },
            );
            self.pump(context);
            self.show(context);
        }
    }
    fn on_background(&mut self, context: &mut Context) {
        self.pump(context);
    }
    fn can_suspend(&self) -> bool {
        matches!(self.draft.status(), Status::Saved)
            && matches!(self.solved_draft.status(), Status::Saved)
    }

    #[allow(
        clippy::too_many_lines,
        reason = "explicit app navigation and editing actions"
    )]
    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if self.progress_loading && self.route == Route::Play {
            if action == action_id("retry-load") && self.load_error.take().is_some() {
                if let Some(key) = self.progress_key() {
                    context.store().load(key);
                }
            } else if action == ActionId::BACK || action == action_id("back-browser") {
                self.route = Route::Browser;
                self.progress_loading = false;
            }
            self.show(context);
            return;
        }
        let leaving = action == action_id("back-browser")
            || action == action_id("next-puzzle")
            || (action == ActionId::BACK && matches!(self.route, Route::Play | Route::Reveal));
        if leaving && !self.can_suspend() {
            self.route = Route::Menu;
            self.notice = Some("Save this game before opening another puzzle.".into());
            self.show(context);
            return;
        }
        if action == action_id("pack-toggle") && self.route == Route::Browser {
            self.pack = if self.pack == Pack::Pictures {
                Pack::Earlier
            } else {
                Pack::Pictures
            };
            self.page = 0;
        } else if action == action_id("photo-help") && self.route == Route::Photo {
            self.route = Route::PhotoHelp;
            self.help_page = 0;
        } else if action == ActionId::BACK && self.route == Route::PhotoHelp {
            self.route = Route::Photo;
        } else if action == action_id("retry-save") {
            self.draft.retry();
            self.solved_draft.retry();
            self.pump(context);
        } else if action == action_id("resume")
            || (action == ActionId::BACK
                && matches!(self.route, Route::Menu | Route::Clue | Route::Restart))
        {
            self.route = Route::Play;
        } else if action == action_id("more") && self.route == Route::Play {
            self.route = Route::Menu;
        } else if action == action_id("undo")
            && matches!(self.route, Route::Play | Route::Menu | Route::Reveal)
        {
            if let Some(marks) = self.undo.pop_back() {
                self.marks = marks;
                self.done = self.completed();
                self.run_start = None;
                self.notice = self.guided_contradiction();
                self.route = Route::Play;
                self.save_progress(context);
            }
        } else if action == action_id("help-next") {
            self.help_page =
                (self.help_page + 1).min(self.help_pages(context).len().saturating_sub(1));
        } else if action == action_id("help-previous") {
            self.help_page = self.help_page.saturating_sub(1);
        } else if action == ActionId::BACK
            || action == action_id("back-browser")
            || action == action_id("next-puzzle")
        {
            self.cancel_photo_task(context);
            self.route = Route::Browser;
            self.notice = None;
        } else if action == action_id("previous-page") && self.page > 0 {
            self.page -= 1;
        } else if action == action_id("next-page")
            && self.page + 1 < self.browser_pages(context).len()
        {
            self.page += 1;
        } else if action == action_id("photo") {
            self.route = Route::Photo;
            self.notice = None;
        } else if action == action_id("how-to-play") {
            self.route = Route::HowTo;
            self.help_page = 0;
            self.notice = None;
        } else if action == action_id("photo-size") {
            self.photo_side = match self.photo_side {
                5 => 7,
                7 => 9,
                _ => 5,
            };
            self.notice = None;
        } else if action == action_id("photo-open") {
            self.open_photo(context);
        } else if action == action_id("policy") {
            self.guided = !self.guided;
            self.notice = self.guided_contradiction();
            self.save_progress(context);
        } else if action == action_id("reset") {
            self.route = Route::Restart;
        } else if action == action_id("confirm-reset") && self.route == Route::Restart {
            self.remember();
            self.route = Route::Play;
            self.marks.fill(Mark::Blank);
            self.done = false;
            self.run_start = None;
            self.notice = None;
            self.save_progress(context);
        } else if action == action_id("run-entry") {
            self.run_entry = !self.run_entry;
            self.run_start = None;
            self.notice = None;
            self.save_progress(context);
        } else if let Some(index) =
            (0..self.puzzles.len()).find(|index| action == action_id(&format!("puzzle-{index}")))
        {
            self.select(context, index);
        } else if self.route == Route::Play {
            self.board_action(context, action);
        }
        self.show(context);
    }

    fn on_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
        if self.route != Route::Photo || self.waiting != Some(task) {
            return;
        }
        self.waiting = None;
        match outcome {
            TaskOutcome::Completed(bytes) => self.load_photo(context, &bytes),
            TaskOutcome::Failed(kobo_sdk::TaskError::NotFound) => {
                self.notice = Some(
                    "No imported photo found. Run kobo nonograms push IMAGE --size 9 --device READER."
                        .to_owned(),
                );
            }
            TaskOutcome::Failed(error) => {
                self.notice = Some(format!("The imported photo could not be read: {error}"));
            }
            TaskOutcome::Cancelled => {
                self.notice = Some("The photo import was cancelled.".to_owned());
            }
        }
        self.show(context);
    }
}

fn main() -> ExitCode {
    match kobo_sdk::run("nonograms", Game::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("nonograms: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{action_id, corpus, photo_id, Cell, Game, Mark, Route, SOLVED};
    use kobo_sdk::{Context, KoboApp, StoreResult, TaskOutcome};
    use kobo_ui::{Chrome, CLARA_BW_METRICS};

    #[test]
    fn every_bundled_puzzle_is_walked_by_the_real_solver() {
        for puzzle in corpus::bundled() {
            assert!(puzzle.is_line_solvable(), "{}", puzzle.id);
        }
    }

    #[test]
    fn browser_links_to_short_rules() {
        let mut game = Game::default();
        assert!(game
            .screen(&Context::default())
            .layout_with(&CLARA_BW_METRICS, &Chrome::default())
            .rect_of_action(action_id("how-to-play"))
            .is_some());
        game.route = Route::HowTo;
        assert!(game
            .screen(&Context::default())
            .diagnostics(&CLARA_BW_METRICS, &Chrome::measuring(true))
            .issues
            .is_empty());
    }

    #[test]
    fn every_browser_page_keeps_puzzles_and_tools_reachable() {
        for (width, height, pixels_per_inch) in
            [(1072, 1448, 300), (1448, 1072, 300), (758, 1024, 212)]
        {
            for text_scale in kobo_ui::TextScale::STEPS {
                let metrics = kobo_ui::DisplayMetrics {
                    width,
                    height,
                    pixels_per_inch,
                    text_scale,
                };
                let context = kobo_sdk::AppRunner::with_metrics(Game::default(), metrics).context();
                let mut game = Game::default();
                let pages = game.browser_pages(&context);
                assert_eq!(
                    pages.iter().flatten().copied().collect::<Vec<_>>(),
                    game.visible_puzzles()
                );
                for (page, indices) in pages.iter().enumerate() {
                    game.page = page;
                    let screen = game.browser(&context);
                    let chrome = Chrome::measuring(true);
                    let issues = screen.diagnostics(&metrics, &chrome).issues;
                    assert!(issues.is_empty(), "{metrics:?}: {issues:?}");
                    let layout = screen.layout_with(&metrics, &chrome);
                    for action in ["photo".to_owned(), "how-to-play".to_owned()]
                        .into_iter()
                        .chain(indices.iter().map(|index| format!("puzzle-{index}")))
                    {
                        assert!(
                            layout.rect_of_action(action_id(&action)).is_some(),
                            "{metrics:?}: {action}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn guided_mode_names_the_contradictory_line() {
        let mut game = Game::default();
        let mut context = Context::default();
        game.select(&mut context, 0);
        game.guided = true;
        let puzzle = game.puzzle().expect("puzzle");
        let empty_row = puzzle
            .row_clues()
            .iter()
            .position(Vec::is_empty)
            .expect("empty row");
        let cell = empty_row * puzzle.side;
        game.marks[cell] = Mark::Fill;
        assert_eq!(
            game.guided_contradiction(),
            Some(format!("Contradiction in row {}.", empty_row + 1))
        );
    }

    #[test]
    fn completion_requires_crosses_as_well_as_fills_then_reveals() {
        let mut game = Game::default();
        let mut context = Context::default();
        game.select(&mut context, 0);
        let answer = game.puzzle().expect("puzzle").answer.clone();
        game.marks = answer
            .iter()
            .map(|filled| if *filled { Mark::Fill } else { Mark::Blank })
            .collect();
        assert!(!game.completed());
        game.marks = answer
            .iter()
            .map(|filled| if *filled { Mark::Fill } else { Mark::Cross })
            .collect();
        assert!(game.completed());
        let final_cell = answer
            .iter()
            .position(|filled| *filled)
            .expect("filled cell");
        game.marks[final_cell] = Mark::Blank;
        game.toggle(&mut context, final_cell);
        assert_eq!(game.route, Route::Reveal);
    }

    #[test]
    fn run_entry_fills_one_line_and_saves_once_when_it_ends() {
        let mut game = Game::default();
        let mut context = Context::default();
        game.select(&mut context, 0);
        let side = game.puzzle().expect("puzzle").side;
        game.enter_run(&mut context, 0);
        assert_eq!(game.run_start, Some(0));
        game.enter_run(&mut context, side - 1);
        assert!((0..side).all(|cell| game.marks[cell] == Mark::Fill));
        assert!(context.commands().iter().any(|command| matches!(
            command,
            kobo_sdk::Command::Store(kobo_sdk::StoreRequest::Save { key, .. })
            if key == "progress-pack-00"
        )));
    }

    #[test]
    fn progress_round_trips_per_puzzle_without_becoming_a_global_save() {
        let mut game = Game::default();
        let mut context = Context::default();
        game.select(&mut context, 1);
        game.guided = true;
        game.marks[0] = Mark::Fill;
        game.save_progress(&mut context);
        let saved = context
            .commands()
            .iter()
            .find_map(|command| match command {
                kobo_sdk::Command::Store(kobo_sdk::StoreRequest::Save { key, value })
                    if key == "progress-pack-01" =>
                {
                    Some(value.clone())
                }
                _ => None,
            })
            .expect("per-puzzle state");
        let mut restored = Game::default();
        restored.select(&mut Context::default(), 1);
        restored.restore_progress(&saved);
        assert!(restored.guided);
        assert_eq!(restored.marks[0], Mark::Fill);
    }

    #[test]
    fn photo_identity_is_content_and_size_derived() {
        let black = photo_png(0);
        let white = photo_png(u8::MAX);
        assert_eq!(photo_id(&black, 5), photo_id(&black, 5));
        assert_ne!(photo_id(&black, 5), photo_id(&white, 5));
        assert_ne!(photo_id(&black, 5), photo_id(&black, 7));
    }

    #[test]
    fn photo_progress_keys_are_bounded_and_content_specific() {
        let black = photo_png(0);
        let white = photo_png(u8::MAX);
        let identical = format!("progress-{}", photo_id(&black, 5));
        let different = format!("progress-{}", photo_id(&white, 5));
        assert_eq!(identical, format!("progress-{}", photo_id(&black, 5)));
        assert_ne!(identical, different);
        assert!(identical.len() <= 64);
        assert!(different.len() <= 64);
    }

    #[test]
    fn photo_size_selector_only_cycles_playable_grids() {
        let mut game = Game::default();
        let mut context = Context::default();
        game.route = Route::Photo;
        game.photo_side = 5;
        for expected in [7, 9, 5] {
            game.on_action(&mut context, action_id("photo-size"));
            assert_eq!(game.photo_side, expected);
        }
    }

    #[test]
    fn replacing_a_photo_does_not_restore_its_progress_or_solved_state() {
        let mut game = Game::default();
        let mut context = Context::default();
        game.photo_side = 5;
        let black = photo_png(0);
        let white = photo_png(u8::MAX);
        game.load_photo(&mut context, &black);
        let first_id = game.puzzle().expect("first photo").id.clone();
        let first_progress = game.progress_key().expect("first progress");
        game.marks[0] = Mark::Fill;
        game.solved.insert(first_id.clone());

        game.load_photo(&mut context, &white);
        let second_id = game.puzzle().expect("replacement photo").id.clone();
        assert_ne!(first_id, second_id);
        assert!(game.marks.iter().all(|mark| *mark == Mark::Blank));
        assert!(!game.solved.contains(&first_id));

        game.on_store(
            &mut context,
            StoreResult::Loaded {
                key: first_progress,
                value: Some(photo_progress()),
            },
        );
        assert!(game.marks.iter().all(|mark| *mark == Mark::Blank));
    }

    #[test]
    fn solved_ids_are_limited_to_bundled_and_current_photo_puzzles() {
        let mut game = Game::default();
        let mut context = Context::default();
        game.on_store(
            &mut context,
            StoreResult::Loaded {
                key: SOLVED.to_owned(),
                value: Some(b"pack-00\nphoto-5-stale\nunknown".to_vec()),
            },
        );
        assert_eq!(game.solved.into_iter().collect::<Vec<_>>(), vec!["pack-00"]);
    }

    #[test]
    fn reimporting_identical_photo_reuses_its_progress_and_solved_identity() {
        let mut game = Game::default();
        let mut context = Context::default();
        game.photo_side = 5;
        let source = photo_png(0);
        game.load_photo(&mut context, &source);
        let id = game.puzzle().expect("photo").id.clone();
        let progress = game.progress_key().expect("progress");

        game.load_photo(&mut context, &source);
        assert_eq!(game.puzzle().expect("reimported photo").id, id);
        game.on_store(
            &mut context,
            StoreResult::Loaded {
                key: SOLVED.to_owned(),
                value: Some(format!("pack-00\n{id}").into_bytes()),
            },
        );
        game.on_store(
            &mut context,
            StoreResult::Loaded {
                key: progress,
                value: Some(photo_progress()),
            },
        );
        assert!(game.solved.contains(&id));
        assert!(game.marks.iter().all(|mark| *mark == Mark::Fill));
    }

    #[test]
    fn reselecting_a_photo_keeps_only_its_own_reveal_art() {
        let mut game = Game::default();
        let mut context = Context::default();
        game.photo_side = 5;
        let first = photo_png(0);
        game.load_photo(&mut context, &first);
        let selected = game.selected.expect("photo selection");
        let photo_id = game.puzzle().expect("photo").id.clone();
        let source = game.photo_reveal.as_ref().expect("stored reveal").1.clone();

        game.route = Route::Browser;
        game.select(&mut context, selected);
        assert_eq!(
            game.photo_reveal.as_ref().map(|(id, _)| id.as_str()),
            Some(photo_id.as_str())
        );
        let answer = game.puzzle().expect("reselected photo").answer.clone();
        game.marks = answer
            .iter()
            .map(|filled| if *filled { Mark::Fill } else { Mark::Cross })
            .collect();
        let last = answer
            .iter()
            .position(|filled| *filled)
            .expect("filled cell");
        game.marks[last] = Mark::Blank;
        game.toggle(&mut context, last);
        assert_eq!(game.route, Route::Reveal);
        assert!(context.commands().iter().any(|command| matches!(
            command,
            kobo_sdk::Command::PutPicture { pixels, .. } if pixels == source.grey()
        )));

        game.load_photo(&mut context, &photo_png(u8::MAX));
        assert_ne!(
            game.photo_reveal.as_ref().map(|(id, _)| id.as_str()),
            Some(photo_id.as_str())
        );
        game.select(&mut context, 0);
        assert!(game.photo_reveal.is_none());
    }

    #[test]
    fn first_board_window_has_a_square_clue_and_options() {
        let mut game = Game::default();
        let mut context = Context::default();
        game.select(&mut context, 0);
        let layout = game
            .play(&Context::default())
            .layout_with(&CLARA_BW_METRICS, &Chrome::default());
        for name in ["board.cell.0", "board.row.0", "board.column.0", "more"] {
            assert!(layout.rect_of_action(action_id(name)).is_some(), "{name}");
        }
        let diagnostics = game
            .play(&Context::default())
            .diagnostics(&CLARA_BW_METRICS, &Chrome::default());
        assert!(diagnostics.issues.is_empty(), "{:?}", diagnostics.issues);
        assert_eq!(Cell::Filled, Mark::Fill.cell());
    }

    #[test]
    fn every_notice_bearing_nine_by_nine_board_keeps_its_controls_on_panel() {
        let notices = [
            "Run starts at row 9, column 9.",
            "A run must stay in one row or column.",
            "Contradiction in row 9.",
            "Contradiction in column 9.",
        ];
        for notice in notices {
            let mut game = Game::default();
            game.select(&mut Context::default(), 24);
            assert_eq!(game.puzzle().map(|puzzle| puzzle.side), Some(9));
            game.guided = true;
            game.run_entry = true;
            game.notice = Some(notice.to_owned());

            let screen = game.play(&Context::default()).with_own_back(true);
            let diagnostics = screen.diagnostics(&CLARA_BW_METRICS, &Chrome::default());
            assert!(diagnostics.issues.is_empty(), "{notice}: {diagnostics:?}");
            let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
            {
                let control = "more";
                let rect = layout
                    .rect_of_action(action_id(control))
                    .unwrap_or_else(|| panic!("{notice}: {control} is unreachable"));
                assert!(
                    rect.y + rect.height <= CLARA_BW_METRICS.height,
                    "{notice}: {control} exceeds the panel: {rect:?}"
                );
            }
        }
    }

    #[test]
    fn leaving_photo_cancels_and_late_completion_cannot_change_routes() {
        let mut game = Game::default();
        let mut context = Context::default();
        game.route = Route::Photo;
        game.on_action(&mut context, action_id("photo-open"));
        let task = game.waiting.expect("photo task");
        let _ = context.take_commands();

        game.on_action(&mut context, action_id("back-browser"));
        assert_eq!(game.route, Route::Browser);
        assert_eq!(game.waiting, None);
        assert!(context
            .take_commands()
            .contains(&kobo_sdk::Command::Cancel(task)));

        game.on_task(&mut context, task, TaskOutcome::Completed(vec![0]));
        assert_eq!(game.route, Route::Browser);
        assert!(game.puzzles.iter().all(|puzzle| puzzle.id != "photo"));
        assert!(context.take_commands().is_empty());

        game.waiting = Some(task);
        game.route = Route::Play;
        game.on_task(&mut context, task, TaskOutcome::Completed(vec![0]));
        assert_eq!(game.route, Route::Play);
        assert!(game.puzzles.iter().all(|puzzle| puzzle.id != "photo"));
        assert!(context.take_commands().is_empty());
    }

    #[test]
    fn browser_and_photo_screens_have_no_layout_errors() {
        let game = Game::default();
        for screen in [game.browser(&Context::default()), game.photo()] {
            assert!(screen
                .diagnostics(&CLARA_BW_METRICS, &Chrome::default())
                .issues
                .is_empty());
        }
    }

    fn photo_png(grey: u8) -> Vec<u8> {
        let picture = kobo_image::Picture::from_grey(5, 5, vec![grey; 25]).expect("picture");
        kobo_image::encode_png_grey(picture.width(), picture.height(), picture.grey()).expect("png")
    }

    fn photo_progress() -> Vec<u8> {
        let mut progress = vec![b'g', b'\n'];
        progress.extend(std::iter::repeat_n(b'#', 25));
        progress
    }
}

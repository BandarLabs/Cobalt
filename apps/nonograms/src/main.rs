//! Touch-first nonograms with a line-solver fairness invariant.

mod corpus;
mod photo;
mod solver;

use corpus::{bundled, Puzzle};
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
const PER_PAGE: usize = 6;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Route {
    #[default]
    Browser,
    Play,
    Gate,
    Photo,
    Reveal,
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

    const fn label(self) -> &'static str {
        match self {
            Self::Blank => " ",
            Self::Fill => "■",
            Self::Cross => "×",
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

struct Game {
    route: Route,
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
}

impl Default for Game {
    fn default() -> Self {
        Self {
            route: Route::Browser,
            puzzles: bundled(),
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
        }
    }
}

impl Game {
    fn show(&self, context: &mut Context) {
        context.set_screen(self.screen().with_own_back(self.route != Route::Browser));
    }

    fn screen(&self) -> Screen {
        match self.route {
            Route::Browser => self.browser(),
            Route::Play => self.play(),
            Route::Gate => ScreenBuilder::new("nonograms-size-gate")
                .top_bar("Nonograms")
                .error_state("This puzzle needs a larger Cobalt panel grid. 5×5 through 9×9 fit this reader.")
                .button("back-browser", "Back to puzzles")
                .build(),
            Route::Photo => self.photo(),
            Route::Reveal => self.reveal_screen(),
        }
    }

    fn browser(&self) -> Screen {
        let pages = self.puzzles.len().div_ceil(PER_PAGE);
        let start = self.page * PER_PAGE;
        let mut screen = ScreenBuilder::new("nonograms-browser")
            .top_bar("Nonograms")
            .secondary("Every bundled puzzle is solvable by row and column logic alone.")
            .section_with_value(
                "Bundled puzzles",
                format!("{} of {}", self.solved.len(), self.puzzles.len()),
            )
            .rows(
                self.puzzles[start..(start + PER_PAGE).min(self.puzzles.len())]
                    .iter()
                    .enumerate()
                    .map(|(offset, puzzle)| {
                        let status = if self.solved.contains(&puzzle.id) {
                            "Solved"
                        } else {
                            "Not started"
                        };
                        (
                            format!("puzzle-{}", start + offset),
                            puzzle.title.clone(),
                            format!("{status} · {}×{}", puzzle.side, puzzle.side),
                            kobo_sdk::Glyph::Grid,
                        )
                    }),
            );
        if pages > 1 {
            screen = screen
                .page_turns("previous-page", "next-page")
                .page_position(
                    u16::try_from(self.page + 1).unwrap_or(u16::MAX),
                    u16::try_from(pages).unwrap_or(u16::MAX),
                );
        }
        screen.button("photo", "Make a photo puzzle").build()
    }

    fn play(&self) -> Screen {
        let Some(puzzle) = self.puzzle() else {
            return self.browser();
        };
        // A 9×9 board uses nearly all of the vertical budget. Put its notice
        // in the fixed top bar so the control band does not move or clip.
        let top_bar = if puzzle.side == 9 {
            self.notice.as_deref().unwrap_or("Nonograms").to_owned()
        } else {
            "Nonograms".to_owned()
        };
        let mut screen = ScreenBuilder::new("nonograms-play").top_bar(top_bar);
        if puzzle.side < 9 {
            screen = screen.secondary(format!(
                "{} · {} mode",
                puzzle.title,
                if self.guided { "guided" } else { "free" }
            ));
        }
        // Clues intentionally use commas inside a run and middle dots between
        // lines: a compact clue string remains readable on paper.
        screen = screen
            .secondary(format!("Rows: {}", clue_text(&puzzle.row_clues())))
            .secondary(format!("Columns: {}", clue_text(&puzzle.column_clues())));
        if let Some(notice) = self.notice.as_ref().filter(|_| puzzle.side < 9) {
            screen = screen.banner(BannerLevel::Attention, notice);
        }
        screen
            .board(
                u8::try_from(puzzle.side).expect("small panel puzzle"),
                self.marks
                    .iter()
                    .enumerate()
                    .map(|(cell, mark)| (cell_name(cell), mark.label(), None)),
            )
            .buttons([
                ("policy", if self.guided { "Guided" } else { "Free" }),
                ("reset", "Reset"),
                (
                    "run-entry",
                    if self.run_entry {
                        "Run entry: on"
                    } else {
                        "Run entry: off"
                    },
                ),
            ])
            .build()
    }

    fn photo(&self) -> Screen {
        let mut screen = ScreenBuilder::new("nonograms-photo")
            .top_bar("Photo puzzle")
            .text("Run kobo nonograms push IMAGE --size N --device READER (N is 5, 7, or 9), then choose the imported photo.")
            .facts([("Grid", format!("{}×{}", self.photo_side, self.photo_side))]);
        if let Some(notice) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, notice);
        }
        screen
            .buttons([
                ("photo-size", "Change grid"),
                ("photo-open", "Use imported photo"),
            ])
            .button("back-browser", "Back to puzzles")
            .build()
    }

    fn reveal_screen(&self) -> Screen {
        let mut screen = ScreenBuilder::new("nonograms-reveal").top_bar("Solved");
        if let Some(reveal) = self.reveal {
            screen = screen.unframed_picture(reveal, 130);
        } else {
            screen = screen
                .heading("Solved")
                .text("The reveal could not fit the picture cache.");
        }
        screen
            .secondary("The completed grid is replaced once by its 16-grey source drawing.")
            .primary_button("next-puzzle", "Back to puzzles")
            .build()
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
        self.selected = Some(index);
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
        if puzzle.side > 9 {
            self.route = Route::Gate;
            return;
        }
        self.route = Route::Play;
        if let Some(key) = self.progress_key() {
            context.store().load(key);
        }
    }

    fn save_progress(&self, context: &mut Context) {
        let Some(key) = self.progress_key() else {
            return;
        };
        let mut state = Vec::with_capacity(self.marks.len() + 2);
        state.push(if self.guided { b'g' } else { b'f' });
        state.push(b'\n');
        state.extend(self.marks.iter().map(|mark| mark.stored()));
        context.store().save(key, state);
    }

    fn restore_progress(&mut self, bytes: &[u8]) {
        let Some(puzzle) = self.puzzle() else { return };
        if bytes.len() != puzzle.side * puzzle.side + 2
            || !matches!(bytes[0], b'f' | b'g')
            || bytes[1] != b'\n'
        {
            return;
        }
        let Some(marks) = bytes[2..]
            .iter()
            .copied()
            .map(Mark::read)
            .collect::<Option<Vec<_>>>()
        else {
            return;
        };
        self.guided = bytes[0] == b'g';
        self.marks = marks;
        self.done = self.completed();
    }

    fn toggle(&mut self, context: &mut Context, cell: usize) {
        if self.done || cell >= self.marks.len() {
            return;
        }
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
            context.store().save(SOLVED, encode_solved(&self.solved));
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
        if self.photo_side > 9 {
            self.notice = Some(
                "This reader can play photo grids up to 9×9. Choose a smaller grid.".to_owned(),
            );
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
                self.selected = Some(self.puzzles.len() - 1);
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
    const HEIGHT: u32 = 724;
    let width = WIDTH as usize;
    let height = HEIGHT as usize;
    let grey = (0..height)
        .flat_map(|y| {
            (0..width).map(move |x| {
                let row = y * puzzle.side / height;
                let column = x * puzzle.side / width;
                let texture = u8::try_from((x / 11 + y / 11) % 8).unwrap_or(0);
                if puzzle.answer[row * puzzle.side + column] {
                    texture * 17
                } else {
                    (8 + texture) * 17
                }
            })
        })
        .collect();
    Picture::from_grey(WIDTH, HEIGHT, grey).ok()
}

fn clue_text(lines: &[Vec<u8>]) -> String {
    lines
        .iter()
        .map(|line| {
            if line.is_empty() {
                "0".to_owned()
            } else {
                line.iter().map(u8::to_string).collect::<Vec<_>>().join(",")
            }
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

fn cell_name(cell: usize) -> String {
    format!("cell-{cell}")
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
                self.solved = value
                    .as_deref()
                    .map(|bytes| decode_solved(bytes, &self.puzzles))
                    .unwrap_or_default();
            } else if self.progress_key().as_deref() == Some(&key) {
                if let Some(value) = value {
                    self.restore_progress(&value);
                }
            }
            self.show(context);
        }
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if action == ActionId::BACK
            || action == action_id("back-browser")
            || action == action_id("next-puzzle")
        {
            self.cancel_photo_task(context);
            self.route = Route::Browser;
            self.notice = None;
        } else if action == action_id("previous-page") && self.page > 0 {
            self.page -= 1;
        } else if action == action_id("next-page")
            && (self.page + 1) * PER_PAGE < self.puzzles.len()
        {
            self.page += 1;
        } else if action == action_id("photo") {
            self.route = Route::Photo;
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
            self.marks.fill(Mark::Blank);
            self.done = false;
            self.run_start = None;
            self.notice = None;
            self.save_progress(context);
        } else if action == action_id("run-entry") {
            self.run_entry = !self.run_entry;
            self.run_start = None;
            self.notice = None;
        } else if let Some(index) =
            (0..self.puzzles.len()).find(|index| action == action_id(&format!("puzzle-{index}")))
        {
            self.select(context, index);
        } else if let Some(cell) =
            (0..self.marks.len()).find(|cell| action == action_id(&cell_name(*cell)))
        {
            if self.run_entry {
                self.enter_run(context, cell);
            } else {
                self.toggle(context, cell);
            }
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
    use super::{action_id, clue_text, corpus, photo_id, Cell, Game, Mark, Route, SOLVED};
    use kobo_sdk::{Context, KoboApp, StoreResult, TaskOutcome};
    use kobo_ui::{Chrome, CLARA_BW_METRICS};

    #[test]
    fn clues_keep_comma_runs_and_middle_dot_line_separators() {
        assert_eq!(clue_text(&[vec![2, 1], vec![], vec![3]]), "2,1 · 0 · 3");
    }

    #[test]
    fn every_bundled_puzzle_is_walked_by_the_real_solver() {
        for puzzle in corpus::bundled() {
            assert!(puzzle.is_line_solvable(), "{}", puzzle.id);
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
            kobo_sdk::Command::PutPicture { grey, .. } if grey == source.grey()
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
    fn playable_board_fits_and_every_cell_is_reachable() {
        let mut game = Game::default();
        let mut context = Context::default();
        game.select(&mut context, 0);
        let layout = game
            .play()
            .layout_with(&CLARA_BW_METRICS, &Chrome::default());
        for cell in 0..game.marks.len() {
            assert!(layout
                .rect_of_action(action_id(&format!("cell-{cell}")))
                .is_some());
        }
        let diagnostics = game
            .play()
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

            let screen = game.play().with_own_back(true);
            let diagnostics = screen.diagnostics(&CLARA_BW_METRICS, &Chrome::default());
            assert!(diagnostics.issues.is_empty(), "{notice}: {diagnostics:?}");
            let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
            for control in ["policy", "reset", "run-entry"] {
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
        for screen in [game.browser(), game.photo()] {
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

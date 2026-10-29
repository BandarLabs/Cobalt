//! Offline Sudoku with acknowledged saves, optional checking and reversible edits.
mod game;
mod saved;
#[cfg(test)]
mod tests;
use game::{Game, Level, Puzzle, CELLS};
use kobo_sdk::game::{self as chrome, Menu, Tap};
use kobo_sdk::{
    action_id, ActionId, BandAlign, Context, DialogAction, KoboApp, PencilBoard, PencilMark,
    PencilMarkKind, Screen, ScreenBuilder, SlotWidth, StoreResult,
};
use kobo_state::draft::{Draft, Status};
use std::process::ExitCode;
type BuildSlot<'a> = Box<dyn FnOnce(ScreenBuilder) -> ScreenBuilder + 'a>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum View {
    #[default]
    Play,
    New,
    Restart,
    Help,
    Hint,
}
struct Sudoku {
    puzzles: Vec<Puzzle>,
    game: Game,
    view: View,
    menu: Menu,
    draft: Draft,
    active: Option<u64>,
    loaded: bool,
    load_error: Option<String>,
    notice: Option<String>,
    help_page: usize,
    sent_orientation: Option<kobo_sdk::Orientation>,
}
impl Default for Sudoku {
    fn default() -> Self {
        let puzzles = game::pack();
        Self {
            game: Game::new(0, &puzzles),
            puzzles,
            view: View::Play,
            menu: Menu::default(),
            draft: Draft::restored(Vec::new(), saved::LIMIT).expect("bounded empty draft"),
            active: None,
            loaded: false,
            load_error: None,
            notice: None,
            help_page: 0,
            sent_orientation: None,
        }
    }
}
fn cell_name(cell: usize) -> String {
    format!("cell-{cell}")
}
fn digit_name(digit: u8) -> String {
    format!("digit-{digit}")
}
impl Sudoku {
    fn save(&mut self, context: &mut Context) {
        let bytes =
            saved::encode(&self.game, &self.puzzles).expect("validated game fits record bound");
        self.draft.replace(bytes).expect("bounded game draft");
        self.pump(context);
    }
    fn pump(&mut self, context: &mut Context) {
        if let Some(write) = self.draft.begin() {
            self.active = Some(write.revision);
            context.store().save(saved::KEY, write.bytes);
        }
    }
    fn progress_label(&self) -> String {
        let left = (0..CELLS)
            .filter(|&cell| self.game.position.board[cell] == 0)
            .count();
        format!("{left} left")
    }
    fn save_failed(&self) -> bool {
        matches!(self.draft.status(), Status::Failed(_))
    }
    fn title(&self) -> String {
        let puzzle = &self.puzzles[self.game.puzzle];
        format!("Sudoku · {} {}", puzzle.level.name(), puzzle.number)
    }
    fn status(&self) -> String {
        if self.save_failed() {
            return "Not saved · Open More to retry.".into();
        }
        if self.game.solved(&self.puzzles) {
            // The hint count used to live on a menu screen that no longer
            // exists, and the end of a puzzle is the one moment it matters.
            // The way on is named too, since the board is otherwise a dead
            // end: the digits and Notes do nothing once it is full.
            return match self.game.position.hints {
                0 => "Puzzle complete. New game is in More.".into(),
                1 => "Puzzle complete with 1 hint. New game is in More.".into(),
                hints => format!("Puzzle complete with {hints} hints. New game is in More."),
            };
        }
        if let Some(notice) = &self.notice {
            return notice.clone();
        }
        if let Some(cell) = self.game.position.selected {
            let spec = &self.puzzles[self.game.puzzle];
            if spec.clues[cell] != 0 {
                return format!("{} · Given", self.progress_label());
            }
            if self.game.checking
                && self.game.position.board[cell] != 0
                && self.game.position.board[cell] != spec.solution[cell]
            {
                return format!("Wrong answer · {}", self.progress_label());
            }
            if self.game.pencil {
                format!("Notes · {}", self.progress_label())
            } else {
                self.progress_label()
            }
        } else {
            format!("Choose a square · {}", self.progress_label())
        }
    }
    fn board(&self, builder: ScreenBuilder) -> ScreenBuilder {
        let selected = self.game.position.selected;
        let spec = &self.puzzles[self.game.puzzle];
        let marks = (0..CELLS)
            .map(|cell| {
                let n = self.game.position.board[cell];
                let notes = self.game.position.notes[cell];
                let given = spec.clues[cell] != 0;
                let kind = if n != 0 {
                    PencilMarkKind::Digit { value: n, given }
                } else if notes != 0 {
                    PencilMarkKind::Candidates(notes)
                } else {
                    PencilMarkKind::Digit {
                        value: 0,
                        given: false,
                    }
                };
                // The cross and the 3x3 box around the target stay shaded, so
                // the houses that constrain a square are visible at a glance.
                let peer = selected.is_some_and(|chosen| {
                    chosen != cell
                        && (chosen / 9 == cell / 9
                            || chosen % 9 == cell % 9
                            || (chosen / 27 == cell / 27 && (chosen % 9) / 3 == (cell % 9) / 3))
                });
                PencilMark {
                    column: u8::try_from(cell % 9).expect("bounded board"),
                    row: u8::try_from(cell / 9).expect("bounded board"),
                    kind,
                    // Givens keep an action so they stay inspectable; entering
                    // a digit on one is refused below, as before.
                    action: Some(action_id(&cell_name(cell))),
                    selected: selected == Some(cell),
                    peer,
                }
            })
            .collect();
        builder.pencil_board(PencilBoard {
            columns: 9,
            rows: 9,
            cell_tenth_mm: 80,
            marks,
            edges: Vec::new(),
        })
    }
    fn digits(&self, builder: ScreenBuilder, columns: u8) -> ScreenBuilder {
        let notes = self
            .game
            .position
            .selected
            .map_or(0, |cell| self.game.position.notes[cell]);
        builder.grid_with_selection(
            columns,
            false,
            (1..=9).map(|digit| {
                (
                    digit_name(digit),
                    digit.to_string(),
                    self.game.pencil && notes & (1 << (digit - 1)) != 0,
                )
            }),
        )
    }
    /// The game's own entries in More, after New game.
    ///
    /// Retry save joins them only while a save has failed, next to New game
    /// at the top, because the status line sends the player here for it.
    fn more_items(&self) -> Vec<(&'static str, &'static str)> {
        let mut items = Vec::with_capacity(5);
        if self.save_failed() {
            items.push(("retry-save", "Retry save"));
        }
        items.push(("restart", "Restart puzzle"));
        items.push((
            "checking",
            if self.game.checking {
                "Hide mistakes"
            } else {
                "Show mistakes"
            },
        ));
        items.push(("hint", "Reveal square"));
        items.push((
            "rotate",
            if self.game.landscape {
                "Use portrait"
            } else {
                "Use landscape"
            },
        ));
        items
    }
    fn screen(&self, context: &Context) -> Screen {
        if !self.loaded {
            let builder = ScreenBuilder::new("sudoku").top_bar("Sudoku");
            return if let Some(error) = &self.load_error {
                builder
                    .heading("Cannot open game")
                    .text(error)
                    .text("Your saved game has been kept. Retry reading it before playing.")
                    .bottom_action("retry-load", "Retry")
                    .build()
            } else {
                builder.text("Opening your game…").build()
            };
        }
        // Every screen but the puzzle is a step away from it, so each has a
        // plain bar and Back returns to the board.
        let away = |title| ScreenBuilder::new("sudoku").top_bar(title).owns_back(true);
        match self.view {
            View::Play => self.play_screen(ScreenBuilder::new("sudoku").game_top_bar(
                self.title(),
                self.menu,
                self.more_items(),
            )),
            View::New => away("Sudoku")
                .heading(chrome::NEW_GAME_LABEL)
                .secondary(if self.has_progress() {
                    "Replaces this game and its history."
                } else {
                    "Choose a difficulty."
                })
                .button("new-easy", "Easy")
                .button("new-medium", "Medium")
                .button("new-hard", "Hard")
                .bottom_action("play", "Keep playing")
                .build(),
            View::Restart => away("Sudoku")
                .confirmation(
                    "Restart puzzle?",
                    "Clear answers, notes and hints. You can undo.",
                    DialogAction::new("confirm-restart", "Restart puzzle"),
                    DialogAction::new("play", "Keep playing"),
                )
                .build(),
            View::Hint => away("Sudoku")
                .confirmation(
                    "Reveal square?",
                    "Fill this square with its answer and count one hint. You can undo.",
                    DialogAction::new("confirm-hint", "Reveal square"),
                    DialogAction::new("play", "Keep playing"),
                )
                .build(),
            View::Help => {
                let pages = self.help_pages(context);
                let current = self.help_page.min(pages.len().saturating_sub(1));
                let mut b = away(chrome::RULES_LABEL)
                    .page_position(
                        u16::try_from(current + 1).expect("bounded help"),
                        u16::try_from(pages.len()).expect("bounded help"),
                    )
                    .action_bar([("help-previous", "Previous"), ("help-next", "Next")]);
                for paragraph in &pages[current] {
                    b = b.text(paragraph);
                }
                b.build()
            }
        }
    }
    fn play_screen(&self, builder: ScreenBuilder) -> Screen {
        let builder = if self.game.landscape {
            // The status line and a keypad of three rows sit beside the
            // board, so the board keeps every line of height the foot bar
            // leaves it: at the largest text a status line above it cost
            // the squares their last pixel of touch target.
            let slots: [(SlotWidth, BuildSlot<'_>); 2] = [
                (SlotWidth::Fill, Box::new(|b| self.board(b))),
                (
                    SlotWidth::Fixed(392),
                    Box::new(|b| self.digits(b.secondary(self.status()), 3)),
                ),
            ];
            builder.band(BandAlign::Top, slots)
        } else {
            self.digits(self.board(builder.secondary(self.status())), 9)
        };
        builder
            .game_actions(
                true,
                [
                    ("pencil", if self.game.pencil { "Digits" } else { "Notes" }),
                    ("erase", "Erase"),
                ],
            )
            .build()
    }
    /// Whether starting another puzzle would throw anything away.
    fn has_progress(&self) -> bool {
        !self.game.solved(&self.puzzles) && !self.game.undo.is_empty()
    }
    /// Why Erase did nothing, since the bar has no greyed-out state to show it.
    fn cannot_erase(&self) -> &'static str {
        match self.game.position.selected {
            None => "Choose a square to erase.",
            Some(cell) if !self.game.editable(cell, &self.puzzles) => {
                "Given numbers cannot change."
            }
            Some(_) => "This square is already empty.",
        }
    }
    /// Whether Reveal square has anything to fill, and if not, why.
    fn cannot_reveal(&self) -> Option<&'static str> {
        let Some(cell) = self.game.position.selected else {
            return Some("Choose a square to reveal.");
        };
        if !self.game.editable(cell, &self.puzzles) {
            Some("Given numbers cannot change.")
        } else if self.game.position.board[cell] == self.puzzles[self.game.puzzle].solution[cell] {
            Some("This square is already right.")
        } else {
            None
        }
    }
    fn help_pages(&self, context: &Context) -> Vec<Vec<String>> {
        let each = self.puzzles.len() / Level::ALL.len();
        context.paginate_oriented(&format!("Use 1–9 once in each row, column and 3×3 box. The shaded cross follows your selection; a bracketed number or small square marks your target. Given numbers cannot change.

Tap Notes, then numbers to add or remove notes. A dot marks a square with notes; its selected numbers have outlined keys. Tap Digits to enter answers. Erase clears the selected square, so notes can go where an answer was.

Mistakes are hidden at first. Show mistakes in More marks a wrong answer in the selected square without removing it, and Hide mistakes turns that off again. Reveal square in More fills the selected square after asking, and counts as a hint.

Your game saves after every edit. Undo takes back the last 64 edits, even after reopening. A full correct grid completes the puzzle. New game in More replaces the current game and its history.

The three dots at the top open More: New game, Restart puzzle, Show mistakes, Reveal square, and Use landscape, which shows the whole board with the digits beside it.

There are {each} original puzzles at each difficulty, and New game moves on to the next one. Easy needs only squares with one possible number. Medium also needs numbers with one possible place in a row, column or box. Hard also needs pairs and pointing: two squares in a row, column or box that share the same two possible numbers, or a number whose places in a box all lie along one line."), true, self.orientation())
    }
    /// A tap on the board, the digits, the foot bar or the game's own entries
    /// in More.
    fn play_action(&mut self, context: &mut Context, action: ActionId) {
        let is = |name: &str| action == action_id(name);
        if is("pencil") {
            self.game.pencil = !self.game.pencil;
        } else if is("erase") {
            if !self.game.erase(&self.puzzles) {
                self.notice = Some(self.cannot_erase().into());
            }
        } else if is("retry-save") {
            self.draft.retry();
            self.pump(context);
        } else if is("restart") {
            self.view = View::Restart;
        } else if is("checking") {
            self.game.checking = !self.game.checking;
        } else if is("hint") {
            match self.cannot_reveal() {
                Some(reason) => self.notice = Some(reason.into()),
                None => self.view = View::Hint,
            }
        } else if is("rotate") {
            self.game.landscape = !self.game.landscape;
        } else if let Some(cell) = (0..CELLS).find(|&c| is(&cell_name(c))) {
            self.game.position.selected = Some(cell);
        } else if let Some(digit) = (1..=9).find(|&d| is(&digit_name(d))) {
            if !self.game.enter(digit, &self.puzzles) && self.notes_need_erase() {
                self.notice = Some("Erase this answer to add notes.".into());
            }
        }
    }
    /// Whether a note was refused only because the square holds an answer.
    fn notes_need_erase(&self) -> bool {
        self.game.pencil
            && !self.game.solved(&self.puzzles)
            && self.game.position.selected.is_some_and(|cell| {
                self.game.editable(cell, &self.puzzles) && self.game.position.board[cell] != 0
            })
    }
    fn orientation(&self) -> kobo_sdk::Orientation {
        if self.game.landscape {
            kobo_sdk::Orientation::Landscape
        } else {
            kobo_sdk::Orientation::Portrait
        }
    }
    fn show(&mut self, context: &mut Context) {
        let orientation = self.orientation();
        if self.sent_orientation != Some(orientation) {
            context.set_orientation(orientation);
            self.sent_orientation = Some(orientation);
        }
        context.set_screen(self.screen(context));
    }
}
impl KoboApp for Sudoku {
    fn on_start(&mut self, context: &mut Context) {
        context.store().load(saved::KEY);
        self.show(context);
    }
    fn on_load(&mut self, context: &mut Context, key: &str, result: StoreResult) {
        if key != saved::KEY || self.loaded {
            return;
        }
        match result {
            StoreResult::Loaded {
                value: Some(bytes), ..
            } => match saved::decode(&bytes, &self.puzzles) {
                Ok(game) => {
                    self.game = game;
                    self.draft = Draft::restored(bytes, saved::LIMIT).expect("validated record");
                    self.loaded = true;
                    self.load_error = None;
                }
                Err(error) => self.load_error = Some(error.to_string()),
            },
            StoreResult::Loaded { value: None, .. } => {
                self.loaded = true;
                self.load_error = None;
                self.save(context);
            }
            _ => self.load_error = Some("Storage could not be read.".into()),
        }
        self.show(context);
    }
    fn on_save(&mut self, context: &mut Context, key: &str, result: StoreResult) {
        if key != saved::KEY {
            return;
        }
        if let Some(revision) = self.active.take() {
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
        !self.loaded || matches!(self.draft.status(), Status::Saved)
    }
    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        let is = |name: &str| action == action_id(name);
        if !self.loaded {
            if is("retry-load") && self.load_error.take().is_some() {
                context.store().load(saved::KEY);
                self.show(context);
            }
            return;
        }
        let before = self.game.clone();
        self.notice = None;
        match self.view {
            View::Play => match self.menu.read(action) {
                Some(Tap::Menu) => {}
                Some(Tap::Undo) => {
                    if !self.game.undo() {
                        self.notice = Some(chrome::NOTHING_TO_UNDO.into());
                    }
                }
                Some(Tap::Rules) => {
                    self.help_page = 0;
                    self.view = View::Help;
                }
                Some(Tap::NewGame) => self.view = View::New,
                None => self.play_action(context, action),
            },
            _ if is("play") || action == ActionId::BACK => self.view = View::Play,
            View::New => {
                if let Some(level) = Level::ALL
                    .into_iter()
                    .find(|l| is(&format!("new-{}", l.key())))
                {
                    self.game = self.game.start(level, &self.puzzles);
                    self.view = View::Play;
                }
            }
            View::Restart => {
                if is("confirm-restart") {
                    self.game.reset(&self.puzzles);
                    self.view = View::Play;
                }
            }
            View::Hint => {
                if is("confirm-hint") {
                    self.game.hint(&self.puzzles);
                    self.view = View::Play;
                }
            }
            View::Help => {
                if is("help-next") {
                    self.help_page =
                        (self.help_page + 1).min(self.help_pages(context).len().saturating_sub(1));
                } else if is("help-previous") {
                    self.help_page = self.help_page.saturating_sub(1);
                }
            }
        }
        if before != self.game {
            self.save(context);
        }
        self.show(context);
    }
}
fn main() -> ExitCode {
    match kobo_sdk::run("sudoku", Sudoku::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("sudoku: {error}");
            ExitCode::FAILURE
        }
    }
}

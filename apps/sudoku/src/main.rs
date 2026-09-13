//! Offline Sudoku with acknowledged saves, optional checking and reversible edits.
mod game;
mod saved;
#[cfg(test)]
mod tests;
use game::{Game, Level, Puzzle, CELLS};
use kobo_sdk::{
    action_id, ActionId, BandAlign, Context, ControlState, DialogAction, KoboApp, Screen,
    ScreenBuilder, SlotWidth, StoreResult,
};
use kobo_state::draft::{Draft, Status};
use std::process::ExitCode;
type BuildSlot<'a> = Box<dyn FnOnce(ScreenBuilder) -> ScreenBuilder + 'a>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum View {
    #[default]
    Play,
    Menu,
    New,
    Restart,
    Help,
    Hint,
    Display,
}
struct Sudoku {
    puzzles: Vec<Puzzle>,
    game: Game,
    view: View,
    draft: Draft,
    active: Option<u64>,
    loaded: bool,
    load_error: Option<String>,
    notice: Option<String>,
    help_page: usize,
    lower_rows: bool,
    sent_orientation: Option<kobo_sdk::Orientation>,
}
impl Default for Sudoku {
    fn default() -> Self {
        let puzzles = game::pack();
        Self {
            game: Game::new(0, &puzzles),
            puzzles,
            view: View::Play,
            draft: Draft::restored(Vec::new(), saved::LIMIT).expect("bounded empty draft"),
            active: None,
            loaded: false,
            load_error: None,
            notice: None,
            help_page: 0,
            lower_rows: false,
            sent_orientation: None,
        }
    }
}
fn state(enabled: bool) -> ControlState {
    if enabled {
        ControlState::Enabled
    } else {
        ControlState::Disabled
    }
}
fn cell_name(cell: usize) -> String {
    format!("cell-{cell}")
}
fn digit_name(digit: u8) -> String {
    format!("digit-{digit}")
}
impl Sudoku {
    fn reveal_selection(&mut self) {
        if let Some(cell) = self.game.position.selected {
            if cell < 27 {
                self.lower_rows = false;
            } else if cell >= 54 {
                self.lower_rows = true;
            }
        }
    }
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
    fn saved_label(&self) -> &'static str {
        match self.draft.status() {
            Status::Saved => "Saved",
            Status::Saving => "Saving…",
            Status::Unsaved | Status::Failed(_) => "Not saved",
        }
    }
    fn heading(&self) -> String {
        format!(
            "{} · {}/12",
            self.puzzles[self.game.puzzle].level.name(),
            self.game.puzzle % 12 + 1
        )
    }
    fn status(&self) -> String {
        if matches!(self.draft.status(), Status::Failed(_)) {
            return "Not saved · Open More to retry.".into();
        }
        if self.game.solved(&self.puzzles) {
            return format!("Puzzle complete · {}", self.saved_label());
        }
        if let Some(notice) = &self.notice {
            return notice.clone();
        }
        if let Some(cell) = self.game.position.selected {
            let spec = &self.puzzles[self.game.puzzle];
            if spec.clues[cell] != 0 {
                return format!("Row {} · Column {} · Given", cell / 9 + 1, cell % 9 + 1);
            }
            if self.game.checking
                && self.game.position.board[cell] != 0
                && self.game.position.board[cell] != spec.solution[cell]
            {
                return format!("Check this answer · {}", self.saved_label());
            }
            format!(
                "Row {} · Column {} · {}",
                cell / 9 + 1,
                cell % 9 + 1,
                if self.game.pencil {
                    "Notes"
                } else {
                    self.saved_label()
                }
            )
        } else {
            format!("Choose a square · {}", self.saved_label())
        }
    }
    fn board(&self, builder: ScreenBuilder, landscape: bool) -> ScreenBuilder {
        let first = if landscape && self.lower_rows { 27 } else { 0 };
        let end = if landscape { first + 54 } else { CELLS };
        builder.board_with_selection(
            9,
            (first..end).map(|cell| {
                let n = self.game.position.board[cell];
                let notes = self.game.position.notes[cell];
                let label = if n != 0 {
                    n.to_string()
                } else if notes == 0 {
                    " ".into()
                } else {
                    "·".into()
                };
                let selected = self.game.position.selected;
                // A light cross follows the selected row and column. A bracketed number or square identifies
                // the one editable target within that cross; givens remain inspectable.
                let focus = selected == Some(cell);
                let label = if focus {
                    if n != 0 {
                        format!("[{n}]")
                    } else if notes != 0 {
                        "⊙".into()
                    } else {
                        "□".into()
                    }
                } else {
                    label
                };
                let peer =
                    selected.is_some_and(|chosen| chosen / 9 == cell / 9 || chosen % 9 == cell % 9);
                (cell_name(cell), label, None, peer)
            }),
        )
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

    fn controls(&self, builder: ScreenBuilder) -> ScreenBuilder {
        builder.grid(
            3,
            false,
            [
                ("pencil", if self.game.pencil { "Digits" } else { "Notes" }),
                ("undo", "Undo"),
                ("more", "More"),
            ],
        )
    }
    fn screen(&self, context: &Context) -> Screen {
        let title = if self.loaded && self.view == View::Play {
            format!(
                "Sudoku · {} {}",
                self.puzzles[self.game.puzzle].level.name(),
                self.game.puzzle % 12 + 1
            )
        } else {
            "Sudoku".into()
        };
        let builder = ScreenBuilder::new("sudoku").top_bar(title);
        if !self.loaded {
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
        let builder = builder.owns_back(self.view != View::Play);
        match self.view {
            View::Play => self.play_screen(builder),
            View::Menu => self.menu_screen(builder),
            View::New => builder
                .heading("New puzzle")
                .secondary("Replaces this game and its history.")
                .button("new-easy", "Easy")
                .button("new-medium", "Medium")
                .button("new-hard", "Hard")
                .bottom_action("play", "Keep playing")
                .build(),
            View::Restart => builder
                .confirmation(
                    "Restart puzzle?",
                    "Clear answers, notes and hints. You can undo.",
                    DialogAction::new("confirm-restart", "Restart puzzle"),
                    DialogAction::new("play", "Keep playing"),
                )
                .build(),
            View::Hint => builder
                .confirmation(
                    "Reveal answer?",
                    "Fill this square and count one hint. You can undo.",
                    DialogAction::new("confirm-hint", "Reveal answer"),
                    DialogAction::new("play", "Keep playing"),
                )
                .build(),
            View::Display => builder
                .heading("View")
                .button(
                    "rotate",
                    if self.orientation() == kobo_sdk::Orientation::Landscape {
                        "Use portrait"
                    } else {
                        "Use landscape"
                    },
                )
                .button("how-to-play", "How to play")
                .bottom_action("play", "Return to puzzle")
                .build(),
            View::Help => {
                let pages = self.help_pages(context);
                let current = self.help_page.min(pages.len().saturating_sub(1));
                let mut b = builder
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
        let landscape = self.orientation() == kobo_sdk::Orientation::Landscape;
        let status = if landscape {
            format!(
                "Rows {} · {}",
                if self.lower_rows { "4–9" } else { "1–6" },
                self.status()
            )
        } else {
            self.status()
        };
        let builder = builder.secondary(status);
        if landscape {
            let slots: [(SlotWidth, BuildSlot<'_>); 2] = [
                (SlotWidth::Fill, Box::new(|b| self.board(b, true))),
                (
                    SlotWidth::Fixed(392),
                    Box::new(|b| {
                        self.digits(b, 5)
                            .button("pencil", if self.game.pencil { "Digits" } else { "Notes" })
                            .button_with_state("undo", "Undo", state(!self.game.undo.is_empty()))
                            .button("more", "More")
                            .button("rows", if self.lower_rows { "1–6" } else { "4–9" })
                    }),
                ),
            ];
            builder.band(BandAlign::Top, slots).build()
        } else {
            self.controls(self.digits(self.board(builder, false), 9))
                .build()
        }
    }
    fn menu_screen(&self, builder: ScreenBuilder) -> Screen {
        let mut b = if matches!(self.draft.status(), Status::Failed(_)) {
            builder
                .heading("Not saved")
                .secondary("Free some storage, then retry.")
                .button("retry-save", "Retry save")
        } else {
            builder
                .heading(if self.game.solved(&self.puzzles) {
                    "Puzzle complete"
                } else {
                    "This game"
                })
                .secondary(format!(
                    "{} · {} · {} hint{}",
                    self.heading(),
                    self.saved_label(),
                    self.game.position.hints,
                    if self.game.position.hints == 1 {
                        ""
                    } else {
                        "s"
                    }
                ))
        };
        let cell = self.game.position.selected;
        let editable = cell.is_some_and(|c| self.game.editable(c, &self.puzzles));
        let rows = [
            [
                (
                    "erase",
                    "Erase",
                    editable
                        && cell.is_some_and(|c| {
                            self.game.position.board[c] != 0 || self.game.position.notes[c] != 0
                        }),
                ),
                (
                    "checking",
                    if self.game.checking {
                        "Check on"
                    } else {
                        "Check off"
                    },
                    true,
                ),
            ],
            [
                (
                    "hint",
                    "Reveal",
                    editable
                        && cell.is_some_and(|c| {
                            self.game.position.board[c]
                                != self.puzzles[self.game.puzzle].solution[c]
                        }),
                ),
                ("new-game", "New", true),
            ],
            [("restart", "Restart", true), ("view", "View", true)],
        ];
        for row in rows {
            b = b.band(
                BandAlign::Middle,
                row.map(|(name, label, enabled)| {
                    (SlotWidth::Fill, move |b: ScreenBuilder| {
                        b.button_with_state(name, label, state(enabled))
                    })
                }),
            );
        }
        b.bottom_action("play", "Return to puzzle").build()
    }
    fn help_pages(&self, context: &Context) -> Vec<Vec<String>> {
        context.paginate_oriented("Use 1–9 once in each row, column and 3×3 box. The shaded cross follows your selection; a bracketed number or small square marks your target. Given numbers cannot change.

Tap Notes, then numbers to add or remove notes. A dot marks a square with notes; its selected numbers have outlined keys. Tap Digits to enter answers. Erase a filled square before adding notes.

More offers erase, checking and reveal. Checking starts off. Turn it on to flag a wrong selected answer without rejecting your entry. Revealing a square asks first and counts as a hint.

Your game saves after every edit. Undo restores the last 64 moves, even after reopening. A full correct grid completes the puzzle. New puzzle replaces the current game and its history.

Landscape shows rows 1–6 or 4–9. Use the range button to move between them.

There are 12 original puzzles per difficulty. Easy uses single candidates. Medium adds single locations in a row, column or box. Hard needs more advanced techniques.", true, self.orientation())
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
                    self.reveal_selection();
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
        if is("play") || action == ActionId::BACK {
            self.view = View::Play;
        } else {
            match self.view {
                View::Play => {
                    if is("rows") {
                        self.lower_rows = !self.lower_rows;
                    } else if is("more") {
                        self.view = View::Menu;
                    } else if is("pencil") {
                        self.game.pencil = !self.game.pencil;
                    } else if is("undo") {
                        if !self.game.undo() {
                            self.notice = Some("No moves to undo.".into());
                        }
                    } else if let Some(cell) = (0..CELLS).find(|&c| is(&cell_name(c))) {
                        self.game.position.selected = Some(cell);
                    } else if let Some(digit) = (1..=9).find(|&d| is(&digit_name(d))) {
                        self.game.enter(digit, &self.puzzles);
                    }
                }
                View::Menu => {
                    if is("retry-save") {
                        self.draft.retry();
                        self.pump(context);
                    } else if is("erase") {
                        self.game.erase(&self.puzzles);
                        self.view = View::Play;
                    } else if is("checking") {
                        self.game.checking = !self.game.checking;
                    } else if is("hint") {
                        self.view = View::Hint;
                    } else if is("new-game") {
                        self.view = View::New;
                    } else if is("restart") {
                        self.view = View::Restart;
                    } else if is("view") {
                        self.view = View::Display;
                    }
                }
                View::New => {
                    if let Some(level) = Level::ALL
                        .into_iter()
                        .find(|l| is(&format!("new-{}", l.key())))
                    {
                        let next = self.game.next(level, &self.puzzles);
                        let landscape = self.game.landscape;
                        self.game = Game::new(next, &self.puzzles);
                        self.game.landscape = landscape;
                        self.lower_rows = false;
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
                View::Display => {
                    if is("rotate") {
                        self.game.landscape = !self.game.landscape;
                        self.view = View::Play;
                    } else if is("how-to-play") {
                        self.help_page = 0;
                        self.view = View::Help;
                    }
                }
                View::Help => {
                    if is("help-next") {
                        self.help_page = (self.help_page + 1)
                            .min(self.help_pages(context).len().saturating_sub(1));
                    } else if is("help-previous") {
                        self.help_page = self.help_page.saturating_sub(1);
                    }
                }
            }
        }
        if before != self.game {
            self.reveal_selection();
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

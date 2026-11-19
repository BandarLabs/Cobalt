//! Offline Sudoku with acknowledged saves, optional checking and reversible edits.
mod game;
mod recover;
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

/// Where to put the digit keys so they sit under the board's nine columns:
/// how far in from the page's edge they start, and how wide they are, both in
/// tenths of a millimetre. `None` when the board is too narrow for nine keys
/// a finger can hit, and the keys keep the page's width.
fn keypad_under_board(probe: &Screen, metrics: &kobo_sdk::DisplayMetrics) -> Option<(u16, u16)> {
    let layout = probe.layout_with(metrics, &kobo_sdk::Chrome::measuring(true));
    let left = layout.rect_of_action(action_id(&cell_name(0)))?.x;
    let last = layout.rect_of_action(action_id(&cell_name(8)))?;
    let right = last.x + last.width;
    let page = layout.rect_of_action(action_id(&digit_name(1)))?.x;
    let gap = metrics.space(kobo_sdk::Space::Small);
    let tight = metrics.space(kobo_sdk::Space::Tight);
    let width = right - left;
    if width < metrics.touch_target_minimum() * 9 + tight * 8 || left - page < gap {
        return None;
    }
    // Rounded down, so the keys never run a pixel past the board.
    let tenths = |pixels: i32| u16::try_from(pixels * 254 / metrics.pixels_per_inch.max(1)).ok();
    Some((tenths(left - page - gap)?, tenths(width)?))
}

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
    set_aside: recover::SetAside,
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
            set_aside: recover::SetAside::default(),
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
    #[cfg(test)]
    fn status(&self) -> String {
        self.status_line(false)
    }
    /// The line over the board. `brief` drops the pointer to More where the
    /// whole sentence would take a second line and push the digit keys off
    /// a small panel at the largest text.
    fn status_line(&self, brief: bool) -> String {
        if self.save_failed() {
            return if brief {
                "Not saved · See More.".into()
            } else {
                "Not saved · Retry saving in More.".into()
            };
        }
        if self.game.solved(&self.puzzles) {
            // The hint count used to live on a menu screen that no longer
            // exists, and the end of a puzzle is the one moment it matters.
            // The way on is named too, since the board is otherwise a dead
            // end: the digits and Notes do nothing once it is full.
            let done = match self.game.position.hints {
                0 => "Puzzle complete.".to_owned(),
                1 => "Puzzle complete with 1 hint.".to_owned(),
                hints => format!("Puzzle complete with {hints} hints."),
            };
            return if brief {
                done
            } else {
                format!("{done} New game is in More.")
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
    /// Retry saving joins them only while a save has failed, next to New
    /// game at the top, because the status line sends the player here for it.
    /// A banner over the board would cost the squares the height the fit
    /// tests guard, so the status line names the entry instead.
    fn more_items(&self) -> Vec<(&'static str, &'static str)> {
        let mut items = Vec::with_capacity(5);
        if self.save_failed() {
            items.push(("retry-save", "Retry saving"));
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
            let head = || ScreenBuilder::new("sudoku").top_bar("Sudoku");
            // A splash with the fix directly under its reason, the shape every
            // other whole-screen failure takes. The heading and the Retry at
            // the foot of the panel left an empty page between the two.
            return if let Some(error) = &self.load_error {
                self.set_aside.screen(
                    head,
                    &context.metrics(),
                    error,
                    &recover::Wording {
                        title: "Cannot open game",
                        kept: "Your saved game has been kept.",
                        explain: "Your saved game stays on the Kobo as a separate copy, and Sudoku starts again on a new Easy puzzle.",
                    },
                )
            } else {
                head().text("Opening your game…").build()
            };
        }
        // Every screen but the puzzle is a step away from it, so each has a
        // plain bar and Back returns to the board.
        let away = |title| ScreenBuilder::new("sudoku").top_bar(title).owns_back(true);
        match self.view {
            View::Play => {
                let head = || {
                    ScreenBuilder::new("sudoku").game_top_bar(
                        self.title(),
                        self.menu,
                        self.more_items(),
                    )
                };
                // The keys are laid out the board's width when it is wide
                // enough for nine keys a finger can hit, which is measured
                // against a screen whose keys run the page's width.
                let metrics = context.metrics().oriented(self.orientation());
                let probe = self.play_screen(head(), None, false);
                let keypad = keypad_under_board(&probe, &metrics);
                let full = self.play_screen(head(), keypad, false);
                if full
                    .diagnostics(&metrics, &kobo_sdk::Chrome::measuring(true))
                    .has_errors()
                {
                    self.play_screen(head(), keypad, true)
                } else {
                    full
                }
            }
            View::New => self.new_game_screen(context),
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
    /// The three difficulties. Each deals a new puzzle, so they are three
    /// verbs side by side rather than a stack of full-width buttons that
    /// reads as a form. Where a third of the panel cannot hold "Medium" on
    /// one line, as upright at the largest sizes, they stack instead of
    /// breaking the word.
    fn new_game_screen(&self, context: &Context) -> Screen {
        const LEVELS: [(&str, &str); 3] = [
            ("new-easy", "Easy"),
            ("new-medium", "Medium"),
            ("new-hard", "Hard"),
        ];
        let build = |side_by_side: bool| {
            let screen = ScreenBuilder::new("sudoku")
                .top_bar("Sudoku")
                .owns_back(true)
                .heading(chrome::NEW_GAME_LABEL)
                .secondary(if self.has_progress() {
                    "Replaces this game and its history."
                } else {
                    "Choose a difficulty."
                });
            let screen = if side_by_side {
                screen.buttons(LEVELS)
            } else {
                LEVELS
                    .into_iter()
                    .fold(screen, |screen, (name, label)| screen.button(name, label))
            };
            screen.bottom_action("play", "Keep playing").build()
        };
        // A label broken across lines makes its button taller than the
        // one-line buttons of the stacked shape. The runtime names the panel
        // upright, so it is turned the way the game asked to be drawn.
        let metrics = context.metrics().oriented(self.orientation());
        let chrome = kobo_sdk::Chrome::measuring(true);
        let stacked = build(false);
        let one_line = stacked
            .layout_with(&metrics, &chrome)
            .rect_of_action(action_id(LEVELS[0].0))
            .map_or(i32::MAX, |rect| rect.height);
        let paired = build(true);
        let layout = paired.layout_with(&metrics, &chrome);
        let whole = LEVELS.iter().all(|(name, _)| {
            layout
                .rect_of_action(action_id(name))
                .is_some_and(|rect| rect.height <= one_line)
        });
        if whole {
            paired
        } else {
            stacked
        }
    }
    fn play_screen(
        &self,
        builder: ScreenBuilder,
        keypad: Option<(u16, u16)>,
        brief: bool,
    ) -> Screen {
        if self.game.landscape {
            // The status line, the keypad and the three verbs sit in a column
            // beside the board, so the board keeps the whole height under the
            // top bar. With the verbs in a bar along the foot, a Clara turned
            // on its side had squares a few pixels under a finger's width.
            let slots: [(SlotWidth, BuildSlot<'_>); 2] = [
                (SlotWidth::Fill, Box::new(|b| self.board(b))),
                (
                    SlotWidth::Fixed(392),
                    Box::new(|b| self.verbs(self.digits(b.secondary(self.status_line(brief)), 3))),
                ),
            ];
            return builder.band(BandAlign::Top, slots).build();
        }
        let builder = self.board(builder.secondary(self.status_line(brief)));
        let builder = match keypad {
            // The keys are the board's width and under its columns, so
            // the row of digits reads as part of the puzzle rather than
            // a row of buttons running past both of its edges.
            Some((indent, width)) => {
                let slots: [(SlotWidth, BuildSlot<'_>); 2] = [
                    (SlotWidth::Fixed(indent), Box::new(|b| b)),
                    (SlotWidth::Fixed(width), Box::new(|b| self.digits(b, 9))),
                ];
                builder.band(BandAlign::Top, slots)
            }
            None => self.digits(builder, 9),
        };
        // Notes is a switch that keeps its name, with a tick over it while it
        // is on. It used to change its word to Digits, so the bar named the
        // mode the player was not in, and read as either an order or a state.
        builder
            .action_bar_marked([
                (chrome::UNDO, chrome::UNDO_LABEL, None),
                (
                    "pencil",
                    "Notes",
                    self.game.pencil.then_some(kobo_sdk::Glyph::Check),
                ),
                ("erase", "Erase", None),
            ])
            .build()
    }
    /// Undo, Notes and Erase as keys under the digits, in the order the
    /// foot bar has them upright. Notes is outlined while it is on, as a
    /// digit key is while its note is set.
    fn verbs(&self, builder: ScreenBuilder) -> ScreenBuilder {
        builder.grid_with_selection(
            3,
            false,
            [
                (chrome::UNDO, chrome::UNDO_LABEL, false),
                ("pencil", "Notes", self.game.pencil),
                ("erase", "Erase", false),
            ],
        )
    }
    /// Whether starting another puzzle would throw anything away.
    fn has_progress(&self) -> bool {
        !self.game.solved(&self.puzzles) && !self.game.undo.is_empty()
    }
    /// Why Erase did nothing, since the bar has no greyed-out state to show it.
    fn cannot_erase(&self) -> &'static str {
        match self.game.position.selected {
            _ if self.game.solved(&self.puzzles) => "The puzzle is complete.",
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

Tap Notes, then numbers to add or remove notes; a tick over Notes shows they are on. Each note sits in its own place in the square, and the selected square's notes have outlined keys. Tap Notes again to enter answers, which are grey beside the printed numbers. Erase clears the selected square, so notes can go where an answer was.

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
                Err(error) => {
                    self.load_error = Some(error.to_string());
                    self.set_aside.hold(bytes);
                }
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
    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        if self.set_aside.on_keys(context, saved::KEY, result) {
            self.show(context);
        }
    }
    fn on_save(&mut self, context: &mut Context, key: &str, result: StoreResult) {
        if let Some(kept) = self.set_aside.on_save(key, &result) {
            // With the unreadable game copied, the fresh one is written at
            // once, so reopening finds it rather than the same failure.
            if kept {
                self.loaded = true;
                self.load_error = None;
                self.save(context);
            }
            self.show(context);
            return;
        }
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
            if self.set_aside.action(context, action) {
                self.show(context);
            } else if is("retry-load") && !self.set_aside.busy() && self.load_error.take().is_some()
            {
                self.set_aside.clear();
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

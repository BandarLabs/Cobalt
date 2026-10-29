//! Four hundred pencil puzzles with independent offline progress.

use kobo_sdk::game::{self, Menu, Tap};
use kobo_sdk::{
    action_id, ActionId, BandAlign, Context, DisplayMetrics, Glyph, KoboApp, LayoutIssueKind,
    Screen, ScreenBuilder, SlotWidth, StoreResult,
};
use kobo_state::draft::{Draft, Status};
use std::process::ExitCode;
mod boards;
mod collection;
mod saved;

const STATE: &str = "logicpack-state-v1";
#[cfg(test)]
const SLITHER_TARGET: u16 = 0b1011_0111_0011;
#[cfg(test)]
const KAKURO_SOLUTION: [u8; 3] = [3, 2, 4];
const INITIAL_MINES: u64 = (1 << 5) | (1 << 10) | (1 << 15);
type Slot = Box<dyn FnOnce(ScreenBuilder) -> ScreenBuilder>;
const NEW_GAME_RULE: &str = "New game opens the next puzzle of this kind you have not solved.";
const KINDS: [Kind; 4] = [Kind::Slither, Kind::Hashi, Kind::Kakuro, Kind::Mines];
/// What the play screen's menu offers after New game.
const MORE: [(&str, &str); 2] = [("restart", "Restart puzzle"), ("puzzles", "Puzzles")];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    Home,
    Slither,
    Hashi,
    Kakuro,
    Mines,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum View {
    #[default]
    Puzzle,
    Help,
    Restart,
    Collection(Kind),
    Entry,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Outcome {
    #[default]
    Playing,
    Lost,
    Solved,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Progress {
    New,
    Started,
    Solved,
}

impl Kind {
    const fn stored(self) -> u8 {
        match self {
            Self::Home => 0,
            Self::Slither => 1,
            Self::Hashi => 2,
            Self::Kakuro => 3,
            Self::Mines => 4,
        }
    }

    const fn read(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Home),
            1 => Some(Self::Slither),
            2 => Some(Self::Hashi),
            3 => Some(Self::Kakuro),
            4 => Some(Self::Mines),
            _ => None,
        }
    }

    const fn title(self) -> &'static str {
        match self {
            Self::Slither => "Slitherlink",
            Self::Hashi => "Hashi",
            Self::Kakuro => "Kakuro",
            Self::Mines => "Minesweeper",
            Self::Home => "Logic Pack",
        }
    }

    /// The action that opens this kind's puzzles from the first screen.
    const fn name(self) -> &'static str {
        match self {
            Self::Slither => "slither",
            Self::Hashi => "hashi",
            Self::Kakuro => "kakuro",
            Self::Mines => "mines",
            Self::Home => "home",
        }
    }

    const fn pitch(self) -> &'static str {
        match self {
            Self::Slither => "One loop",
            Self::Hashi => "Connect islands",
            Self::Kakuro => "Cross sums",
            Self::Mines => "Clear the field",
            Self::Home => "",
        }
    }
}

/// The puzzles of one kind in the order a player meets them: by grade, and
/// within a grade as the collection lists them.
fn ladder(kind: Kind) -> Vec<usize> {
    let mut order: Vec<usize> = collection::puzzles()
        .iter()
        .enumerate()
        .filter(|(_, p)| p.kind == kind)
        .map(|(i, _)| i)
        .collect();
    order.sort_by_key(|i| collection::puzzles()[*i].level());
    order
}

#[allow(
    clippy::struct_excessive_bools,
    reason = "each flag is a separate fact about the board or the session, and a state enum would make every restore check harder to read"
)]
struct Game {
    kind: Kind,
    cells: [u8; 64],
    notice: String,
    view: View,
    mines: u64,
    flagging: bool,
    /// Slitherlink taps mark where no line goes instead of drawing one.
    marking: bool,
    first_reveal: bool,
    outcome: Outcome,
    loaded: bool,
    load_error: Option<String>,
    draft: Draft,
    active: Option<u64>,
    games: Vec<saved::Run>,
    puzzle: usize,
    /// The grade the puzzle list is showing, an index into `LEVELS`.
    level: usize,
    page: usize,
    help_page: usize,
    entry: usize,
    undo: Vec<Vec<u8>>,
    menu: Menu,
    /// The panel as of the last callback, so the puzzle list can measure how
    /// many rows fit at the reader's text size.
    metrics: DisplayMetrics,
}

impl Default for Game {
    fn default() -> Self {
        Self {
            games: vec![saved::Run::default(); collection::puzzles().len()],
            ..Self::blank()
        }
    }
}

impl Game {
    /// A game with no progress table, for reading and writing one position.
    ///
    /// Opening a save checks every stored position by restoring it into a
    /// scratch game, and a table of four hundred empty records built for each
    /// of them was most of the work.
    fn blank() -> Self {
        Self {
            kind: Kind::Home,
            cells: [0; 64],
            notice: "Choose a puzzle.".into(),
            view: View::Puzzle,
            mines: INITIAL_MINES,
            flagging: false,
            marking: false,
            first_reveal: true,
            outcome: Outcome::Playing,
            loaded: false,
            load_error: None,
            draft: Draft::restored(Vec::new(), saved::LIMIT).expect("empty draft"),
            active: None,
            games: Vec::new(),
            puzzle: 0,
            level: 0,
            page: 0,
            help_page: 0,
            entry: 0,
            undo: Vec::new(),
            menu: Menu::default(),
            metrics: DisplayMetrics::default(),
        }
    }

    /// Files the open puzzle's progress. A puzzle that was only looked at is
    /// filed as never played, so it still reads as new in the list and costs
    /// nothing in the saved record.
    fn retain(&mut self) {
        if self.kind != Kind::Home {
            let position = self.encode();
            let mut fresh = Self::blank();
            fresh.fresh_puzzle(self.puzzle);
            self.games[self.puzzle] = if self.undo.is_empty() && position == fresh.encode() {
                saved::Run::default()
            } else {
                saved::Run {
                    position,
                    undo: self.undo.clone(),
                }
            };
        }
    }
    fn puzzle(&self) -> &'static collection::Puzzle {
        &collection::puzzles()[self.puzzle]
    }
    fn label(&self) -> String {
        format!("{} · {}", self.puzzle().difficulty, self.puzzle().title)
    }
    #[cfg(test)]
    fn select(&mut self, kind: Kind) {
        if kind == Kind::Home {
            self.retain();
            self.fresh(kind);
        } else {
            self.select_puzzle(usize::from(kind.stored() - 1));
        }
    }
    fn select_puzzle(&mut self, index: usize) {
        self.retain();
        self.fresh_puzzle(index);
        let run = self.games[index].clone();
        if !run.position.is_empty() {
            assert!(self.restore(&run.position), "validated run");
            self.undo = run.undo;
            // Saves from before puzzles judged themselves can hold a finished
            // board that nobody pressed Check on.
            if self.outcome == Outcome::Playing && self.kind != Kind::Mines {
                self.judge();
            }
            if self.outcome == Outcome::Playing {
                self.notice = self.label();
            }
        }
    }
    fn fresh_puzzle(&mut self, index: usize) {
        self.fresh(collection::puzzles()[index].kind);
        self.puzzle = index;
        if self.kind == Kind::Mines {
            self.mines = self.puzzle().mines();
            if let Some(open) = self.puzzle().opening() {
                self.first_reveal = false;
                self.reveal(open);
            }
        }
        self.notice = self.label();
    }
    fn remember(&mut self, before: Vec<u8>) {
        if self.encode() != before {
            if self.undo.len() == saved::HISTORY {
                self.undo.remove(0);
            }
            self.undo.push(before);
        }
    }
    fn save(&mut self, context: &mut Context) {
        self.retain();
        self.draft
            .replace(saved::encode(
                &self.games,
                if self.kind == Kind::Home {
                    None
                } else {
                    Some(self.puzzle)
                },
            ))
            .expect("bounded games");
        self.pump(context);
    }
    fn pump(&mut self, context: &mut Context) {
        if let Some(write) = self.draft.begin() {
            self.active = Some(write.revision);
            context.store().save(STATE, write.bytes);
        }
    }
    fn fresh(&mut self, kind: Kind) {
        self.undo.clear();
        self.kind = kind;
        self.puzzle = usize::from(kind.stored().saturating_sub(1));
        self.cells = [0; 64];
        self.mines = INITIAL_MINES;
        self.flagging = false;
        self.marking = false;
        self.first_reveal = true;
        self.outcome = Outcome::Playing;
        self.menu.close();
        self.notice = "Choose a puzzle.".into();
    }

    fn progress(&self, index: usize) -> Progress {
        let run = &self.games[index];
        if run.position.is_empty() {
            Progress::New
        } else if run.position.ends_with(b"|0|1") {
            Progress::Solved
        } else {
            Progress::Started
        }
    }

    /// The next puzzle of this kind not yet solved, after the open one and
    /// round to the start, so New game climbs the grades in order.
    fn next_unsolved(&self) -> Option<usize> {
        let order = ladder(self.kind);
        let at = order.iter().position(|i| *i == self.puzzle).unwrap_or(0);
        (1..order.len())
            .map(|step| order[(at + step) % order.len()])
            .find(|i| self.progress(*i) != Progress::Solved)
    }

    fn new_game(&mut self, context: &mut Context) {
        self.retain();
        if let Some(index) = self.next_unsolved() {
            self.select_puzzle(index);
            self.view = View::Puzzle;
            self.save(context);
        } else {
            self.notice = format!("Every {} puzzle is solved.", self.kind.title());
        }
    }

    /// The puzzles the list shows for `kind` at the chosen grade.
    fn listed(&self, kind: Kind) -> Vec<usize> {
        ladder(kind)
            .into_iter()
            .filter(|i| collection::puzzles()[*i].level() == self.level)
            .collect()
    }

    fn row(&self, index: usize, longest: bool) -> (String, String, String, Glyph) {
        let p = &collection::puzzles()[index];
        let progress = if longest {
            Progress::Started
        } else {
            self.progress(index)
        };
        let (state, glyph) = match progress {
            Progress::New => ("", Glyph::Grid),
            Progress::Started => (" · In progress", Glyph::Circle),
            Progress::Solved => (" · Solved", Glyph::Check),
        };
        (
            format!("puzzle-{index}"),
            p.title.clone(),
            format!("{}{state}", p.size()),
            glyph,
        )
    }

    /// One page of the puzzle list. The runtime draws a chevron beside the
    /// page count only on a side where there is a page, so the list needs no
    /// Previous and Next of its own, and its rows get the room instead.
    fn collection_page(
        &self,
        kind: Kind,
        listed: &[usize],
        (page, size, pages): (usize, usize, usize),
        longest: bool,
    ) -> Screen {
        let solved = listed
            .iter()
            .filter(|i| self.progress(**i) == Progress::Solved)
            .count();
        let rows = listed.iter().skip(page * size).take(size);
        let screen = ScreenBuilder::new("logicpack-collection")
            .top_bar(kind.title())
            .owns_back(true)
            .segmented(
                self.level,
                collection::LEVELS
                    .iter()
                    .enumerate()
                    .map(|(i, level)| (format!("level-{i}"), *level)),
            )
            .secondary(format!("{solved} of {} solved", listed.len()))
            .rows(rows.map(|i| self.row(*i, longest)));
        if pages > 1 {
            screen
                .page_turns("previous-puzzles", "next-puzzles")
                .page_position(
                    u16::try_from(page + 1).unwrap_or(u16::MAX),
                    u16::try_from(pages).unwrap_or(u16::MAX),
                )
                .build()
        } else {
            screen.build()
        }
    }

    fn fits(&self, screen: &Screen) -> bool {
        screen
            .diagnostics(&self.metrics, &kobo_sdk::Chrome::measuring(true))
            .issues
            .iter()
            .all(|issue| {
                !matches!(
                    issue.kind,
                    LayoutIssueKind::ContentOverflow { .. }
                        | LayoutIssueKind::TextOverflow
                        | LayoutIssueKind::Clipped
                        | LayoutIssueKind::InteractiveOffscreen
                )
            })
    }

    /// How many puzzles one page of the list holds on this panel.
    ///
    /// Measured rather than fixed, because the right answer is a dozen at the
    /// smallest text size and four at the largest, and a fixed three wasted
    /// most of the panel at every size. Every row is measured as though it
    /// were in progress, the longest a row gets, so a page never overflows
    /// once somebody starts one of its puzzles.
    fn page_size(&self, kind: Kind, listed: &[usize]) -> usize {
        let all = listed.len().max(1);
        if self.fits(&self.collection_page(kind, listed, (0, all, 1), true)) {
            return all;
        }
        let (mut low, mut high) = (1, listed.len().min(kobo_sdk::MAX_ROWS));
        while low < high {
            let middle = (low + high).div_ceil(2);
            if self.fits(&self.collection_page(kind, listed, (0, middle, 2), true)) {
                low = middle;
            } else {
                high = middle - 1;
            }
        }
        low
    }

    fn collection_screen(&self, kind: Kind) -> Screen {
        let listed = self.listed(kind);
        let size = self.page_size(kind, &listed);
        let pages = listed.len().div_ceil(size).max(1);
        let page = self.page.min(pages - 1);
        self.collection_page(kind, &listed, (page, size, pages), false)
    }

    /// Opens the list for `kind` at the grade and page of `focus`, or of the
    /// first puzzle not yet solved, so a player lands where they left off.
    fn open_collection(&mut self, kind: Kind, focus: Option<usize>) {
        let order = ladder(kind);
        let focus = focus
            .or_else(|| {
                order
                    .iter()
                    .copied()
                    .find(|i| self.progress(*i) != Progress::Solved)
            })
            .or_else(|| order.first().copied());
        self.view = View::Collection(kind);
        self.level = focus.map_or(0, |i| collection::puzzles()[i].level());
        let listed = self.listed(kind);
        let at = focus
            .and_then(|i| listed.iter().position(|s| *s == i))
            .unwrap_or(0);
        self.page = at / self.page_size(kind, &listed);
    }

    fn turn_page(&mut self, kind: Kind, forward: bool) {
        let listed = self.listed(kind);
        let pages = listed.len().div_ceil(self.page_size(kind, &listed)).max(1);
        let page = self.page.min(pages - 1);
        self.page = if forward {
            (page + 1).min(pages - 1)
        } else {
            page.saturating_sub(1)
        };
    }

    fn navigation_action(&mut self, context: &mut Context, action: ActionId) -> bool {
        if self.view == View::Entry {
            if action == ActionId::BACK || action == action_id("close-entry") {
                self.view = View::Puzzle;
            } else if let Some(value) =
                (0..=9_u8).find(|n| action == action_id(&format!("digit-{n}")))
            {
                let before = self.encode();
                if self.cells[self.entry] != value {
                    self.cells[self.entry] = value;
                    self.judge();
                    self.remember(before);
                    self.save(context);
                }
                self.view = View::Puzzle;
            }
            context.set_screen(self.screen());
            return true;
        }
        if self.kind == Kind::Kakuro && self.view == View::Puzzle {
            if let Some(cell) = (0..self.puzzle().cell_count())
                .find(|n| action == action_id(&format!("kakuro-{n}")))
            {
                self.entry = cell;
                self.view = View::Entry;
                context.set_screen(self.screen());
                return true;
            }
        }
        if let View::Collection(kind) = self.view {
            if action == ActionId::BACK {
                self.view = View::Puzzle;
                self.kind = Kind::Home;
                self.save(context);
            } else if action == action_id("next-puzzles") {
                self.turn_page(kind, true);
            } else if action == action_id("previous-puzzles") {
                self.turn_page(kind, false);
            } else if let Some(level) =
                (0..collection::LEVELS.len()).find(|i| action == action_id(&format!("level-{i}")))
            {
                self.level = level;
                self.page = 0;
            } else if let Some((index, _)) = collection::puzzles()
                .iter()
                .enumerate()
                .find(|(i, p)| p.kind == kind && action == action_id(&format!("puzzle-{i}")))
            {
                self.select_puzzle(index);
                self.view = View::Puzzle;
                self.save(context);
            }
            context.set_screen(self.screen());
            return true;
        }
        false
    }
    fn history_action(&mut self, context: &mut Context, action: ActionId) -> bool {
        let before = self.encode();
        if self.view == View::Restart {
            if action == action_id("confirm-restart") {
                let history = std::mem::take(&mut self.undo);
                self.fresh_puzzle(self.puzzle);
                self.undo = history;
                self.remember(before);
                self.save(context);
            } else if action != action_id("cancel-restart") && action != ActionId::BACK {
                return true;
            }
            self.view = View::Puzzle;
            context.set_screen(self.screen());
            return true;
        }
        if (action == action_id("restart") || action == action_id("restart-field"))
            && self.kind != Kind::Home
        {
            self.view = View::Restart;
            context.set_screen(self.screen());
            return true;
        }
        false
    }

    /// Answers a tap on the controls every game shares.
    fn shared(&mut self, context: &mut Context, tap: Tap) {
        match tap {
            Tap::Menu => {}
            Tap::Undo => {
                if let Some(position) = self.undo.pop() {
                    // The mode is how the player is tapping, not part of the
                    // move, so stepping back never switches it under them.
                    let flagging = self.flagging;
                    assert!(self.restore(&position));
                    self.flagging = flagging;
                    if self.outcome == Outcome::Playing {
                        self.notice = "Move undone.".into();
                    }
                    self.save(context);
                } else {
                    self.notice = game::NOTHING_TO_UNDO.into();
                }
            }
            Tap::Rules => {
                self.help_page = 0;
                self.view = View::Help;
            }
            Tap::NewGame => self.new_game(context),
        }
    }

    fn entry_screen(&self) -> Screen {
        let collection::Rules::CrossSum { runs, givens, .. } = &self.puzzle().rules else {
            return self.help_screen();
        };
        let full = givens
            .iter()
            .enumerate()
            .filter(|(_, n)| **n == 0)
            .nth(self.entry)
            .expect("editable square")
            .0;
        let across = runs
            .iter()
            .find(|r| !r.down && r.cells.contains(&full))
            .expect("across sum")
            .sum;
        let down = runs
            .iter()
            .find(|r| r.down && r.cells.contains(&full))
            .expect("down sum")
            .sum;
        ScreenBuilder::new("logicpack-digit")
            .top_bar(&self.puzzle().title)
            .owns_back(true)
            .heading("Enter a digit")
            .text(format!("Across {across} · Down {down}"))
            .secondary(format!(
                "Current: {}",
                if self.cells[self.entry] == 0 {
                    "blank".into()
                } else {
                    self.cells[self.entry].to_string()
                }
            ))
            .grid(
                3,
                false,
                (1..=9).map(|n| (format!("digit-{n}"), n.to_string())),
            )
            .grid(
                2,
                false,
                [("digit-0", "Clear square"), ("close-entry", "Board")],
            )
            .build()
    }
    fn screen(&self) -> Screen {
        if !self.loaded {
            let b = ScreenBuilder::new("logicpack-opening").top_bar("Logic Pack");
            return if let Some(error) = &self.load_error {
                b.heading("Cannot open progress")
                    .text(error)
                    .text("Your saved puzzles have been kept.")
                    .bottom_action("retry-load", "Retry")
                    .build()
            } else {
                b.text("Opening puzzles…").build()
            };
        }
        if matches!(self.draft.status(), Status::Failed(_)) {
            return ScreenBuilder::new("logicpack-save-failed")
                .top_bar("Logic Pack")
                .heading("Progress not saved")
                .text("Your latest moves are still here. Keep Logic Pack open and retry saving.")
                .bottom_action("retry-save", "Retry save")
                .build();
        }
        if let View::Collection(kind) = self.view {
            return self.collection_screen(kind);
        }
        if self.view == View::Entry {
            return self.entry_screen();
        }
        if self.view == View::Restart {
            return ScreenBuilder::new("logicpack-restart")
                .top_bar("Restart puzzle")
                .owns_back(true)
                .heading("Clear this puzzle?")
                .text("Other puzzles keep their progress. You can undo this restart.")
                .button("confirm-restart", "Restart")
                .button("cancel-restart", "Keep playing")
                .build();
        }
        if self.view == View::Help {
            return self.help_screen();
        }
        match self.kind {
            Kind::Home => ScreenBuilder::new("logicpack")
                .top_bar("Logic Pack")
                .rows(KINDS.map(|kind| {
                    let order = ladder(kind);
                    let solved = order
                        .iter()
                        .filter(|i| self.progress(**i) == Progress::Solved)
                        .count();
                    (
                        kind.name(),
                        kind.title(),
                        format!("{} · {solved} of {} solved", kind.pitch(), order.len()),
                        Glyph::Grid,
                    )
                }))
                .build(),
            Kind::Slither | Kind::Hashi | Kind::Kakuro => {
                self.play(|screen| screen.pencil_board(boards::pencil(self.puzzle(), &self.cells)))
            }
            Kind::Mines => self.mines_screen(),
        }
    }

    /// The rules, with what New game does on the first page wherever it
    /// fits. On a six-inch panel at the two largest text sizes it does not,
    /// and moves to the second page in place of the note about grades.
    fn help_screen(&self) -> Screen {
        let first = self.rules_page(true);
        let roomy = self.fits(&first);
        if self.help_page == 0 {
            return if roomy { first } else { self.rules_page(false) };
        }
        ScreenBuilder::new("logicpack-difficulty")
            .top_bar("Difficulty")
            .owns_back(true)
            .secondary(self.label())
            .text(&self.puzzle().guide)
            .text(if roomy {
                "A grade comes from the hardest step a puzzle needs, not from a timer."
            } else {
                NEW_GAME_RULE
            })
            .grid(
                2,
                false,
                [("previous-help", "Rules"), ("close-help", "Play")],
            )
            .build()
    }

    fn rules_page(&self, with_new_game: bool) -> Screen {
        let rules = match self.kind {
            Kind::Slither => [
                "Draw one loop with no branches or crossings.",
                "Each clue counts the loop edges around it.",
                "Tap between dots to draw a line, or in Cross mode to mark where none goes.",
            ],
            Kind::Hashi => [
                "Join islands with one or two straight bridges that never cross.",
                "Each island needs its number of bridges, and all must connect.",
                "Tap a gap to add a bridge. A third tap clears it.",
            ],
            Kind::Kakuro => [
                "Each run adds up to its sum: across at top right, down at bottom left.",
                "Use 1 to 9, with no digit twice in a run.",
                "Tap a white square to enter a digit. Printed digits are fixed.",
            ],
            Kind::Mines | Kind::Home => [
                "Reveal every safe square without opening a mine.",
                "A number counts the mines in the eight touching squares.",
                "In Flag mode, a tap marks a square you think holds a mine.",
            ],
        };
        let screen = ScreenBuilder::new("logicpack-help")
            .top_bar(self.kind.title())
            .owns_back(true)
            .text(rules[0])
            .text(rules[1])
            .text(rules[2]);
        if with_new_game {
            screen.text(NEW_GAME_RULE)
        } else {
            screen
        }
        .grid(
            2,
            false,
            [("next-help", "Difficulty"), ("close-help", "Play")],
        )
        .build()
    }

    /// A play screen: the shared top bar and foot around the game's board.
    ///
    /// The foot's second place is the way taps work, for the two games that
    /// have one, and becomes the obvious next step once a puzzle ends: the
    /// next puzzle after a win, a restart after a mine.
    fn play(&self, board: impl FnOnce(ScreenBuilder) -> ScreenBuilder) -> Screen {
        let saved = match self.draft.status() {
            Status::Saved => "Saved",
            Status::Saving | Status::Unsaved => "Saving…",
            Status::Failed(_) => "Not saved",
        };
        let status = if self.kind == Kind::Mines {
            // A cleared field shows every mine as a flag, so it counts them so.
            let flags: usize = if self.outcome == Outcome::Solved {
                self.mines.count_ones() as usize
            } else {
                self.cells[..self.puzzle().cell_count()]
                    .iter()
                    .map(|c| usize::from(*c == 2))
                    .sum()
            };
            format!("{flags} of {} flagged · {saved}", self.mines.count_ones())
        } else {
            saved.to_owned()
        };
        let verb = match (self.kind, self.outcome) {
            (_, Outcome::Solved) => Some(("next-puzzle", "Next puzzle")),
            (_, Outcome::Lost) => Some(("restart-field", "Restart")),
            (Kind::Slither, _) if self.marking => Some(("line-mode", "Mode: Cross")),
            (Kind::Slither, _) => Some(("line-mode", "Mode: Line")),
            (Kind::Mines, _) if self.flagging => Some(("mine-mode", "Mode: Flag")),
            (Kind::Mines, _) => Some(("mine-mode", "Mode: Reveal")),
            _ => None,
        };
        // The notice and the save state share one line above the board, so
        // the largest field still fits a six-inch panel at the largest text.
        let notice = self.notice.clone();
        let line: [(SlotWidth, Slot); 2] = [
            (
                SlotWidth::Fill,
                Box::new(move |slot| slot.secondary(notice)),
            ),
            (
                SlotWidth::Natural,
                Box::new(move |slot| slot.secondary(status)),
            ),
        ];
        let screen = ScreenBuilder::new("logicpack-play")
            .game_top_bar(self.kind.title(), self.menu, MORE)
            .owns_back(true)
            .band(BandAlign::Middle, line);
        board(screen).game_actions(true, verb).build()
    }

    /// Hidden squares are drawn filled, as untouched tiles, and open ones
    /// plain. A question mark on every hidden square read as a clue nobody
    /// could answer, and a blank hidden square looked exactly like an open
    /// square with no mines around it.
    fn mines_screen(&self) -> Screen {
        let side = self.puzzle().side();
        let cells = (0..self.puzzle().cell_count()).map(|cell| {
            let hidden = matches!(self.cells[cell], 0 | 2);
            let label = if self.outcome == Outcome::Solved && has_mine(self.mines, cell) {
                "⚑".to_owned()
            } else {
                match self.cells[cell] {
                    1 => digit_label(adjacent_mines(self.mines, cell, side)),
                    2 => "⚑".to_owned(),
                    3 => "✹".to_owned(),
                    _ => String::new(),
                }
            };
            (format!("mine-{cell}"), label, None, hidden)
        });
        self.play(|screen| {
            screen.board_with_selection(u8::try_from(side).expect("mine board"), cells)
        })
    }

    fn tap(&mut self, action: ActionId) -> bool {
        match self.kind {
            Kind::Slither => (0..self.puzzle().cell_count())
                .find(|edge| action == action_id(&format!("edge-{edge}")))
                .is_some_and(|edge| {
                    let mark = if self.marking { 2 } else { 1 };
                    self.cells[edge] = if self.cells[edge] == mark { 0 } else { mark };
                    true
                }),
            Kind::Hashi => (0..self.puzzle().cell_count())
                .find(|route| action == action_id(&format!("route-{route}")))
                .is_some_and(|route| {
                    self.cells[route] = (self.cells[route] + 1) % 3;
                    true
                }),
            Kind::Kakuro => (0..self.puzzle().cell_count())
                .find(|cell| action == action_id(&format!("kakuro-{cell}")))
                .is_some_and(|cell| {
                    self.cells[cell] = self.cells[cell] % 9 + 1;
                    true
                }),
            Kind::Mines => (0..self.puzzle().cell_count())
                .find(|cell| action == action_id(&format!("mine-{cell}")))
                .is_some_and(|cell| self.tap_mine(cell)),
            Kind::Home => false,
        }
    }

    fn tap_mine(&mut self, cell: usize) -> bool {
        if self.outcome != Outcome::Playing {
            return false;
        }
        if self.flagging {
            self.cells[cell] = match self.cells[cell] {
                0 => 2,
                2 => 0,
                _ => return false,
            };
            return true;
        }
        if self.cells[cell] != 0 {
            return false;
        }
        if self.first_reveal {
            self.first_reveal = false;
            if has_mine(self.mines, cell) {
                self.mines &= !(1 << cell);
                let replacement = (0..self.puzzle().cell_count())
                    .find(|candidate| *candidate != cell && !has_mine(self.mines, *candidate))
                    .expect("a replacement square");
                self.mines |= 1 << replacement;
            }
        }
        if has_mine(self.mines, cell) {
            self.cells[cell] = 3;
            self.outcome = Outcome::Lost;
            self.notice = "Mine opened. Undo the last move or restart.".into();
            return true;
        }
        self.reveal(cell);
        self.outcome = if (0..self.puzzle().cell_count())
            .all(|at| has_mine(self.mines, at) || self.cells[at] == 1)
        {
            Outcome::Solved
        } else {
            Outcome::Playing
        };
        self.notice = if self.outcome == Outcome::Solved {
            "Field cleared.".into()
        } else {
            "Safe. Keep going.".into()
        };
        true
    }

    fn reveal(&mut self, cell: usize) {
        if self.cells[cell] != 0 || has_mine(self.mines, cell) {
            return;
        }
        self.cells[cell] = 1;
        if adjacent_mines(self.mines, cell, self.puzzle().side()) != 0 {
            return;
        }
        for neighbour in neighbours(cell, self.puzzle().side()) {
            self.reveal(neighbour);
        }
    }

    /// Decides after every move whether a pencil puzzle is solved.
    ///
    /// Puzzles used to wait for a Check button, which only ever said "not
    /// yet" until the moment it would have said "solved" on its own. When the
    /// board looks finished and is not, the notice says which rule it breaks,
    /// still without giving away where.
    fn judge(&mut self) {
        if self.is_complete() {
            self.outcome = Outcome::Solved;
            self.notice = "Solved.".into();
            return;
        }
        self.outcome = Outcome::Playing;
        let count = self.puzzle().cell_count();
        self.notice = match &self.puzzle().rules {
            collection::Rules::CrossSum { .. } if self.cells[..count].iter().all(|n| *n != 0) => {
                "Every square is filled, but a sum is wrong or a digit repeats.".into()
            }
            collection::Rules::Loop { side, .. } => {
                let mut degree = vec![0; (side + 1) * (side + 1)];
                for (edge, (a, b)) in collection::loop_edges(*side).into_iter().enumerate() {
                    if self.cells[edge] == 1 {
                        degree[a] += 1;
                        degree[b] += 1;
                    }
                }
                if degree.iter().any(|d| *d != 0) && degree.iter().all(|d| *d == 0 || *d == 2) {
                    "The line is closed, but a clue does not match or there is more than one loop."
                        .into()
                } else {
                    self.label()
                }
            }
            collection::Rules::Bridges {
                islands, routes, ..
            } if islands.iter().enumerate().all(|(v, (_, _, need))| {
                routes
                    .iter()
                    .zip(&self.cells)
                    .filter(|((a, b), _)| *a == v || *b == v)
                    .map(|(_, n)| *n)
                    .sum::<u8>()
                    == *need
            }) =>
            {
                "Every island has its bridges, but some cross or they do not all connect.".into()
            }
            _ => self.label(),
        };
    }

    fn is_complete(&self) -> bool {
        self.kind != Kind::Home && self.puzzle().solved(&self.cells, self.mines)
    }

    fn encode(&self) -> Vec<u8> {
        format!(
            "{}|{}|{}|{}|{}|{}|{}",
            format_args!("p{}", self.puzzle),
            self.cells
                .iter()
                .map(u8::to_string)
                .collect::<Vec<_>>()
                .join(","),
            self.mines,
            u8::from(self.flagging),
            u8::from(self.first_reveal),
            u8::from(self.outcome == Outcome::Lost),
            u8::from(self.outcome == Outcome::Solved)
        )
        .into_bytes()
    }

    fn restore(&mut self, bytes: &[u8]) -> bool {
        let Ok(text) = std::str::from_utf8(bytes) else {
            return false;
        };
        let fields = text.split('|').collect::<Vec<_>>();
        if fields.len() != 7 {
            return false;
        }
        let Some((kind, index, cells, mines)) = saved::position_header(&fields) else {
            return false;
        };
        let puzzle = &collection::puzzles()[index];
        let flags = fields[3..]
            .iter()
            .map(|flag| match *flag {
                "0" => Some(false),
                "1" => Some(true),
                _ => None,
            })
            .collect::<Option<Vec<_>>>();
        let Some(flags) = flags else {
            return false;
        };
        let outcome = match (flags[2], flags[3]) {
            (false, false) => Outcome::Playing,
            (true, false) => Outcome::Lost,
            (false, true) => Outcome::Solved,
            (true, true) => return false,
        };
        let used = if kind == Kind::Home {
            0
        } else {
            puzzle.cell_count()
        };
        let maximum = match kind {
            Kind::Kakuro => 9,
            Kind::Mines => 3,
            _ => 2,
        };
        if cells[..used].iter().any(|c| *c > maximum) || cells[used..].iter().any(|c| *c != 0) {
            return false;
        }
        if kind != Kind::Mines
            && (outcome == Outcome::Lost || flags[0] || !flags[1] || mines != INITIAL_MINES)
        {
            return false;
        }
        if kind == Kind::Mines
            && (cells
                .iter()
                .enumerate()
                .any(|(i, c)| (*c == 1 && has_mine(mines, i)) || (*c == 3 && !has_mine(mines, i)))
                || (flags[1] && cells.iter().any(|c| *c == 1 || *c == 3))
                || (outcome == Outcome::Lost) != cells.contains(&3))
        {
            return false;
        }
        if outcome == Outcome::Solved && (kind == Kind::Home || !puzzle.solved(&cells, mines)) {
            return false;
        }
        self.kind = kind;
        self.puzzle = index;
        self.cells = cells;
        self.mines = mines;
        self.flagging = flags[0];
        self.first_reveal = flags[1];
        self.outcome = outcome;
        self.notice = match outcome {
            Outcome::Solved => "Solved.",
            Outcome::Lost => "Mine opened. Undo the last move or restart.",
            Outcome::Playing => "Continue your puzzle.",
        }
        .into();
        true
    }
}

fn digit_label(value: u8) -> String {
    if value == 0 {
        String::new()
    } else {
        value.to_string()
    }
}

const fn has_mine(mines: u64, cell: usize) -> bool {
    mines & (1 << cell) != 0
}

fn neighbours(cell: usize, side: usize) -> impl Iterator<Item = usize> {
    let row = cell / side;
    let column = cell % side;
    let mut adjacent = Vec::new();
    for next_row in row.saturating_sub(1)..=(row + 1).min(side - 1) {
        for next_column in column.saturating_sub(1)..=(column + 1).min(side - 1) {
            let next = next_row * side + next_column;
            if next != cell {
                adjacent.push(next);
            }
        }
    }
    adjacent.into_iter()
}

fn adjacent_mines(mines: u64, cell: usize, side: usize) -> u8 {
    u8::try_from(
        neighbours(cell, side)
            .filter(|at| has_mine(mines, *at))
            .count(),
    )
    .expect("at most eight neighbours")
}

impl KoboApp for Game {
    fn on_start(&mut self, context: &mut Context) {
        self.metrics = context.metrics();
        context.set_orientation(kobo_sdk::Orientation::Portrait);
        context.store().load(STATE);
        context.set_screen(self.screen());
    }

    fn on_load(&mut self, context: &mut Context, key: &str, result: StoreResult) {
        self.metrics = context.metrics();
        if key != STATE || self.loaded {
            return;
        }
        match result {
            StoreResult::Loaded {
                value: Some(bytes), ..
            } => match saved::decode(&bytes) {
                Some((games, current)) => {
                    self.games = games;
                    self.fresh(Kind::Home);
                    if let Some(index) = current {
                        self.select_puzzle(index);
                    }
                    self.draft = Draft::restored(bytes, saved::LIMIT).expect("validated bytes");
                    self.loaded = true;
                    self.load_error = None;
                }
                None => self.load_error = Some("This saved record could not be read.".into()),
            },
            StoreResult::Loaded { value: None, .. } => {
                self.loaded = true;
                self.load_error = None;
            }
            _ => self.load_error = Some("Storage could not be read.".into()),
        }
        context.set_screen(self.screen());
    }
    fn on_save(&mut self, context: &mut Context, key: &str, result: StoreResult) {
        self.metrics = context.metrics();
        if key != STATE {
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
            context.set_screen(self.screen());
        }
    }
    fn can_suspend(&self) -> bool {
        !self.loaded || matches!(self.draft.status(), Status::Saved)
    }
    fn on_background(&mut self, context: &mut Context) {
        self.pump(context);
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        self.metrics = context.metrics();
        if !self.loaded {
            if action == action_id("retry-load") && self.load_error.take().is_some() {
                context.store().load(STATE);
                context.set_screen(self.screen());
            }
            return;
        }
        if matches!(self.draft.status(), Status::Failed(_)) {
            if action == action_id("retry-save") {
                self.draft.retry();
                self.pump(context);
                context.set_screen(self.screen());
            }
            return;
        }
        if self.view == View::Puzzle && self.kind != Kind::Home {
            if let Some(tap) = self.menu.read(action) {
                self.shared(context, tap);
                context.set_screen(self.screen());
                return;
            }
        }
        if self.navigation_action(context, action) {
            return;
        }
        let before = self.encode();
        let mut changed = true;
        let mut save = false;
        if self.history_action(context, action) {
            return;
        }
        if self.view == View::Help {
            if action == action_id("next-help") {
                self.help_page = 1;
            } else if action == action_id("previous-help") {
                self.help_page = 0;
            } else if action == action_id("close-help") || action == ActionId::BACK {
                self.view = View::Puzzle;
            } else {
                return;
            }
        } else if self.kind == Kind::Home {
            match KINDS
                .into_iter()
                .find(|kind| action == action_id(kind.name()))
            {
                Some(kind) => self.open_collection(kind, None),
                None => return,
            }
        } else {
            match action {
                action if action == action_id("puzzles") || action == ActionId::BACK => {
                    self.retain();
                    self.open_collection(self.kind, Some(self.puzzle));
                }
                action if action == action_id("next-puzzle") => self.new_game(context),
                action if action == action_id("line-mode") && self.kind == Kind::Slither => {
                    self.marking = !self.marking;
                    self.notice = if self.marking {
                        "Cross mode. Tap between two dots to mark where no line goes."
                    } else {
                        "Line mode. Tap between two dots to draw a line."
                    }
                    .into();
                }
                action if action == action_id("mine-mode") && self.kind == Kind::Mines => {
                    self.flagging = !self.flagging;
                    self.notice = if self.flagging {
                        "Flag mode. Tap a hidden square to mark it."
                    } else {
                        "Reveal mode. Tap a hidden square to open it."
                    }
                    .into();
                    self.save(context);
                }
                _ => {
                    changed = self.tap(action);
                    if changed && self.kind != Kind::Mines {
                        self.judge();
                    }
                    save = changed;
                }
            }
        }
        if save {
            self.remember(before);
            self.save(context);
        }
        if changed {
            context.set_screen(self.screen());
        }
    }
}

fn main() -> ExitCode {
    match kobo_sdk::run("logicpack", Game::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("logicpack: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Measures with the shipped fonts at one panel, one test at a time.
///
/// Installing fonts is process-wide, so two layout tests running side by side
/// at different text sizes would each measure with the other's fonts.
#[cfg(test)]
fn fonts(
    metrics: DisplayMetrics,
) -> (
    std::sync::MutexGuard<'static, ()>,
    kobo_sdk::AppRunner<Game>,
) {
    static FONTS: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let guard = FONTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    (
        guard,
        kobo_sdk::AppRunner::with_metrics(Game::default(), metrics),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobo_ui::{Chrome, CLARA_BW_METRICS};

    fn ready() -> Game {
        Game {
            loaded: true,
            ..Game::default()
        }
    }

    #[test]
    fn all_four_puzzles_have_real_winning_positions() {
        let mut game = ready();
        game.select(Kind::Slither);
        for edge in 0..12 {
            game.cells[edge] = u8::from(SLITHER_TARGET & (1 << edge) != 0);
        }
        game.judge();
        assert_eq!(game.outcome, Outcome::Solved);

        game.select(Kind::Hashi);
        game.cells[..4].fill(1);
        game.judge();
        assert_eq!(game.outcome, Outcome::Solved);

        game.select(Kind::Kakuro);
        game.cells[..3].copy_from_slice(&KAKURO_SOLUTION);
        game.judge();
        assert_eq!(game.outcome, Outcome::Solved);
    }

    #[test]
    fn minesweeper_reseats_the_first_mine_and_can_lose_later() {
        let mut game = ready();
        game.select(Kind::Mines);
        assert!(has_mine(game.mines, 5));
        assert!(game.tap_mine(5));
        assert_ne!(game.outcome, Outcome::Lost);
        assert!(!has_mine(game.mines, 5));
        let mine = (0..16)
            .find(|cell| has_mine(game.mines, *cell))
            .expect("mine");
        assert!(game.tap_mine(mine));
        assert_eq!(game.outcome, Outcome::Lost);
    }

    #[test]
    fn progress_round_trips_and_malformed_state_is_refused() {
        let mut game = ready();
        game.select(Kind::Hashi);
        game.cells[1] = 2;
        let mut restored = Game::default();
        assert!(restored.restore(&game.encode()));
        assert_eq!(restored.kind, Kind::Hashi);
        assert_eq!(restored.cells[1], 2);
        assert!(!restored.restore(b"bad"));
    }

    #[test]
    fn every_play_screen_carries_rules_and_the_menu_on_top_and_undo_first_at_the_foot() {
        let _fonts = fonts(CLARA_BW_METRICS);
        for kind in KINDS {
            let mut game = ready();
            game.select(kind);
            let screen = game.screen();
            assert!(screen
                .diagnostics(&CLARA_BW_METRICS, &Chrome::default())
                .issues
                .is_empty());
            let bar = screen.top_bar.as_ref().expect("top bar");
            let labels: Vec<_> = bar.actions.iter().map(|a| a.label.as_str()).collect();
            assert_eq!(labels, [game::RULES_LABEL, "More"], "{kind:?}");
            let first = screen.nav_bar.as_ref().map_or_else(
                || screen.bottom_action.as_ref().unwrap().action.label.clone(),
                |bar| bar.destinations[0].label.clone(),
            );
            assert_eq!(first, game::UNDO_LABEL, "{kind:?}");
            let text = format!("{screen:?}");
            for gone in ["Help", "Check", "Undo last move"] {
                assert!(!text.contains(&format!("\"{gone}\"")), "{kind:?} {gone}");
            }
            game.on_action(&mut Context::default(), action_id(game::MORE));
            let menu = format!("{:?}", game.screen().overlay);
            assert!(menu.contains(game::NEW_GAME_LABEL) && menu.contains("Restart puzzle"));
            game.on_action(&mut Context::default(), action_id(game::RULES));
            assert_eq!(game.view, View::Help, "Rules is one tap from the board");
            game.view = View::Help;
            assert!(game
                .screen()
                .diagnostics(&CLARA_BW_METRICS, &Chrome::measuring(true))
                .issues
                .is_empty());
        }
    }

    #[test]
    fn hidden_minesweeper_squares_are_filled_tiles_with_no_question_marks() {
        let mut game = ready();
        game.select(Kind::Mines);
        game.cells[0] = 2;
        game.tap_mine(15);
        let kobo_ui::Node::Grid { cells, .. } = game
            .screen()
            .nodes
            .iter()
            .find(|n| matches!(n, kobo_ui::Node::Grid { .. }))
            .expect("field")
            .clone()
        else {
            unreachable!()
        };
        assert!(cells.iter().all(|c| c.label != "?"));
        assert_eq!(cells[0].label, "⚑");
        for (cell, square) in cells.iter().enumerate() {
            assert_eq!(square.selected, matches!(game.cells[cell], 0 | 2), "{cell}");
            if game.cells[cell] == 0 {
                assert!(square.label.is_empty());
            }
        }
    }
}

#[cfg(test)]
mod regression_tests {
    use super::*;
    #[test]
    fn a_lost_field_stays_lost_after_restore_until_restarted() {
        let mut game = Game {
            loaded: true,
            ..Game::default()
        };
        game.select(Kind::Mines);
        assert!(game.tap_mine(0));
        assert!(game.tap_mine(5));
        assert_eq!(game.outcome, Outcome::Lost);
        let mut restored = Game::default();
        assert!(restored.restore(&game.encode()));
        assert_eq!(restored.outcome, Outcome::Lost);
        assert!(!restored.tap_mine(1));
        restored.fresh(Kind::Mines);
        assert!(restored.tap_mine(1));
    }
}

#[cfg(test)]
mod help_layout_tests {
    use super::*;
    #[test]
    fn help_fits_supported_text_scales_and_geometries() {
        for (width, height, pixels_per_inch) in
            [(1072, 1448, 300), (758, 1024, 212), (1448, 1072, 300)]
        {
            for text_scale in kobo_ui::TextScale::STEPS {
                let metrics = kobo_sdk::DisplayMetrics {
                    width,
                    height,
                    pixels_per_inch,
                    text_scale,
                };
                let _fonts = fonts(metrics);
                for kind in KINDS {
                    let mut game = Game {
                        loaded: true,
                        metrics,
                        ..Game::default()
                    };
                    game.select(kind);
                    let mut said = false;
                    for page in [0, 1] {
                        game.help_page = page;
                        let screen = game.help_screen();
                        said |= format!("{screen:?}").contains(NEW_GAME_RULE);
                        let chrome = kobo_ui::Chrome::measuring(true);
                        let diagnostics = screen.diagnostics(&metrics, &chrome);
                        assert!(
                            diagnostics.issues.is_empty(),
                            "{kind:?} {page} {metrics:?}: {:?}",
                            diagnostics.issues
                        );
                        assert!(screen
                            .layout_with(&metrics, &chrome)
                            .rect_of_action(action_id("close-help"))
                            .is_some());
                    }
                    assert!(said, "{kind:?} {metrics:?} explains New game");
                }
            }
        }
    }
}

#[cfg(test)]
mod durable_tests {
    use super::*;
    fn ready() -> Game {
        Game {
            loaded: true,
            ..Game::default()
        }
    }
    fn back(g: &mut Game) {
        g.on_action(&mut Context::default(), ActionId::BACK);
    }
    fn act(g: &mut Game, name: &str) {
        if ["slither", "hashi", "kakuro", "mines"].contains(&name) {
            g.view = View::Puzzle;
            g.kind = Kind::Home;
            g.on_action(&mut Context::default(), action_id(name));
            let i = ["slither", "hashi", "kakuro", "mines"]
                .iter()
                .position(|n| *n == name)
                .unwrap();
            g.on_action(&mut Context::default(), action_id(&format!("puzzle-{i}")));
        } else {
            g.on_action(&mut Context::default(), action_id(name));
            if g.view == View::Entry && name.starts_with("kakuro-") {
                let n = g.cells[g.entry] % 9 + 1;
                g.on_action(&mut Context::default(), action_id(&format!("digit-{n}")));
            }
        }
    }
    #[test]
    fn switching_games_and_reopening_keeps_progress_and_undo() {
        let mut game = ready();
        act(&mut game, "hashi");
        act(&mut game, "route-0");
        act(&mut game, "puzzles");
        act(&mut game, "kakuro");
        act(&mut game, "kakuro-1");
        let bytes = saved::encode(&game.games, Some(game.puzzle));
        let mut reopened = Game::default();
        reopened.on_load(
            &mut Context::default(),
            STATE,
            StoreResult::Loaded {
                key: STATE.into(),
                value: Some(bytes),
            },
        );
        assert_eq!(reopened.kind, Kind::Kakuro);
        assert_eq!(reopened.cells[1], 1);
        back(&mut reopened);
        act(&mut reopened, "hashi");
        assert_eq!(reopened.cells[0], 1);
        act(&mut reopened, game::UNDO);
        assert_eq!(reopened.cells[0], 0);
        act(&mut reopened, "puzzles");
        act(&mut reopened, "kakuro");
        assert_eq!(reopened.cells[1], 1);
    }
    #[test]
    fn restart_requires_confirmation_and_is_one_undo_step() {
        let mut g = ready();
        act(&mut g, "hashi");
        act(&mut g, "route-0");
        act(&mut g, "route-1");
        let before = g.encode();
        act(&mut g, game::MORE);
        act(&mut g, "restart");
        act(&mut g, "cancel-restart");
        assert_eq!(g.encode(), before);
        act(&mut g, "restart");
        act(&mut g, "confirm-restart");
        assert_eq!(g.cells, [0; 64]);
        act(&mut g, game::UNDO);
        assert_eq!(g.encode(), before);
        act(&mut g, game::UNDO);
        assert_eq!(g.cells[1], 0);
    }
    #[test]
    fn undo_with_nothing_behind_it_says_so_in_place() {
        let mut g = ready();
        act(&mut g, "slither");
        act(&mut g, game::UNDO);
        assert_eq!(g.notice, game::NOTHING_TO_UNDO);
        assert_eq!(g.view, View::Puzzle);
    }
    #[test]
    fn old_write_acknowledgement_cannot_mark_newer_moves_saved() {
        let mut g = ready();
        act(&mut g, "hashi");
        act(&mut g, "route-0");
        let old = g.active.unwrap();
        act(&mut g, "route-0");
        let bytes = g.draft.bytes().to_vec();
        g.on_save(
            &mut Context::default(),
            STATE,
            StoreResult::Saved { key: STATE.into() },
        );
        assert!(g.active.unwrap() > old);
        assert_eq!(g.draft.status(), Status::Saving);
        g.on_save(
            &mut Context::default(),
            STATE,
            StoreResult::Denied(kobo_sdk::StoreError::Unwritable),
        );
        assert!(matches!(g.draft.status(), Status::Failed(_)));
        assert_eq!(g.draft.bytes(), bytes);
        g.on_background(&mut Context::default());
        assert!(g.active.is_none());
        act(&mut g, "retry-save");
        assert_eq!(g.draft.status(), Status::Saving);
        g.on_save(
            &mut Context::default(),
            STATE,
            StoreResult::Saved { key: STATE.into() },
        );
        assert_eq!(g.draft.status(), Status::Saved);
    }
    #[test]
    fn loss_and_flood_reveal_can_be_undone_together_with_mine_positions() {
        let mut g = ready();
        act(&mut g, "mines");
        let initial = g.encode();
        act(&mut g, "mine-5");
        assert!(!has_mine(g.mines, 5));
        act(&mut g, game::UNDO);
        assert_eq!(g.encode(), initial);
        act(&mut g, "mine-0");
        let safe = g.encode();
        act(&mut g, "mine-5");
        assert_eq!(g.outcome, Outcome::Lost);
        act(&mut g, game::UNDO);
        assert_eq!(g.encode(), safe);
    }
    #[test]
    fn invalid_and_future_records_are_preserved_without_partial_restore() {
        let mut g = ready();
        g.select(Kind::Hashi);
        g.cells[0] = 1;
        let before = g.encode();
        let bad = String::from_utf8(before.clone())
            .unwrap()
            .replace("|0|0", "|1|1");
        assert_ne!(bad.as_bytes(), before);
        assert!(!g.restore(bad.as_bytes()));
        assert_eq!(g.encode(), before);
        for bytes in [
            b"bad".to_vec(),
            vec![b'x'; saved::LIMIT + 1],
            b"1|9,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0|33824|0|1|0|0".to_vec(),
        ] {
            assert!(saved::decode(&bytes).is_none());
            let mut unopened = Game::default();
            unopened.on_load(
                &mut Context::default(),
                STATE,
                StoreResult::Loaded {
                    key: STATE.into(),
                    value: Some(bytes),
                },
            );
            assert!(!unopened.loaded);
            assert!(unopened.load_error.is_some());
            act(&mut unopened, "hashi");
            assert_eq!(unopened.kind, Kind::Home);
            assert!(unopened.active.is_none());
        }
        g.retain();
        let bytes = saved::encode(&g.games, Some(g.puzzle));
        let future = String::from_utf8(bytes)
            .unwrap()
            .replace("\"version\":3", "\"version\":99");
        assert!(saved::decode(future.as_bytes()).is_none());
    }
    #[test]
    fn legacy_record_migrates_and_bounded_history_round_trips() {
        let mut g = ready();
        g.select(Kind::Kakuro);
        g.cells[0] = 3;
        let (games, kind) =
            saved::decode(b"3|3,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0|33824|0|1|0|0").unwrap();
        assert_eq!(kind, Some(2));
        assert_eq!(games[2].position, g.encode());
        for _ in 0..100 {
            act(&mut g, "kakuro-1");
        }
        assert_eq!(g.undo.len(), saved::HISTORY);
        let bytes = saved::encode(&g.games, Some(g.puzzle));
        assert!(bytes.len() <= saved::LIMIT);
        assert_eq!(saved::decode(&bytes).unwrap().0, g.games);
    }
    #[test]
    fn new_game_opens_the_next_unsolved_puzzle_of_the_same_kind() {
        let mut g = ready();
        act(&mut g, "hashi");
        assert_eq!(g.puzzle, 1);
        g.cells[..4].fill(1);
        g.judge();
        g.retain();
        act(&mut g, game::NEW_GAME);
        let next = g.puzzle;
        assert_eq!(g.kind, Kind::Hashi);
        assert_ne!(next, 1);
        assert_eq!(collection::puzzles()[next].difficulty, "Starter");
        assert_eq!(g.progress(1), Progress::Solved);

        for index in ladder(Kind::Hashi) {
            g.games[index].position = b"|0|1".to_vec();
        }
        act(&mut g, game::NEW_GAME);
        assert_eq!(g.puzzle, next);
        assert_eq!(g.notice, "Every Hashi puzzle is solved.");
    }
    #[test]
    fn a_finished_board_is_solved_without_pressing_anything_and_offers_the_next() {
        let mut g = ready();
        act(&mut g, "hashi");
        for route in 0..4 {
            act(&mut g, &format!("route-{route}"));
        }
        assert_eq!(g.outcome, Outcome::Solved);
        assert!(format!("{:?}", g.screen()).contains("Next puzzle"));
        act(&mut g, "next-puzzle");
        assert_ne!(g.puzzle, 1);
        assert_eq!(g.outcome, Outcome::Playing);
    }
    #[test]
    fn slitherlink_draws_lines_or_crosses_by_mode_and_undo_keeps_the_mode() {
        let mut g = ready();
        act(&mut g, "slither");
        act(&mut g, "edge-0");
        assert_eq!(g.cells[0], 1);
        act(&mut g, "line-mode");
        act(&mut g, "edge-1");
        assert_eq!(g.cells[1], 2);
        act(&mut g, "edge-0");
        assert_eq!(g.cells[0], 2, "a cross replaces a line");
        act(&mut g, game::UNDO);
        assert_eq!(g.cells[0], 1);
        assert!(g.marking);
        act(&mut g, "line-mode");
        act(&mut g, "edge-0");
        assert_eq!(g.cells[0], 0, "a second tap clears a line");
    }
    #[test]
    fn new_minesweeper_fields_start_with_a_safe_patch_open() {
        let index = (20..collection::puzzles().len())
            .find(|i| collection::puzzles()[*i].kind == Kind::Mines)
            .expect("a new field");
        let mut g = ready();
        g.select_puzzle(index);
        assert!(!g.first_reveal);
        let open = g.puzzle().opening().unwrap();
        assert_eq!(g.cells[open], 1);
        assert!(g.cells.iter().map(|c| usize::from(*c == 1)).sum::<usize>() > 1);
        assert_eq!(
            g.games[index],
            saved::Run::default(),
            "looking is not playing"
        );
        g.retain();
        assert_eq!(g.games[index], saved::Run::default());
    }
    #[test]
    fn actual_fonts_keep_puzzle_controls_visible_at_every_text_size() {
        for (width, height, ppi) in [(1072, 1448, 300), (758, 1024, 212)] {
            for text_scale in kobo_ui::TextScale::STEPS {
                let metrics = kobo_sdk::DisplayMetrics {
                    width,
                    height,
                    pixels_per_inch: ppi,
                    text_scale,
                };
                let _fonts = fonts(metrics);
                for kind in [
                    Kind::Home,
                    Kind::Slither,
                    Kind::Hashi,
                    Kind::Kakuro,
                    Kind::Mines,
                ] {
                    let mut g = ready();
                    g.metrics = metrics;
                    g.select(kind);
                    for view in [View::Puzzle, View::Help, View::Restart] {
                        if kind == Kind::Home && view == View::Help {
                            continue;
                        }
                        g.view = view;
                        let chrome =
                            kobo_ui::Chrome::measuring(view != View::Puzzle || kind != Kind::Home);
                        let diagnostics = g.screen().diagnostics(&metrics, &chrome);
                        assert!(
                            diagnostics.issues.is_empty(),
                            "{kind:?} {view:?} {metrics:?}: {:?}",
                            diagnostics.issues
                        );
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod collection_integration_tests {
    use super::*;

    /// One puzzle for every board shape in the collection, which is what the
    /// layout depends on; checking all four hundred at every text size would
    /// prove the same thing a hundred times over.
    fn one_of_each_shape() -> Vec<usize> {
        let mut seen = std::collections::BTreeSet::new();
        (0..collection::puzzles().len())
            .filter(|i| {
                let p = &collection::puzzles()[*i];
                let shape = match &p.rules {
                    collection::Rules::Loop { side, .. } => format!("loop {side}"),
                    collection::Rules::Bridges { width, height, .. } => {
                        format!("bridges {width} {height}")
                    }
                    collection::Rules::CrossSum { mask, .. } => {
                        format!("sums {} {}", mask.len(), mask[0].len())
                    }
                    collection::Rules::Mines { side, .. } => format!("mines {side}"),
                };
                seen.insert(shape)
            })
            .collect()
    }

    #[test]
    fn every_board_shape_picker_and_help_page_fits_shipped_fonts() {
        for (width, height, ppi) in [(1072, 1448, 300), (758, 1024, 212)] {
            for text_scale in kobo_ui::TextScale::STEPS {
                let metrics = kobo_sdk::DisplayMetrics {
                    width,
                    height,
                    pixels_per_inch: ppi,
                    text_scale,
                };
                let _fonts = fonts(metrics);
                for index in one_of_each_shape() {
                    let mut g = Game {
                        loaded: true,
                        metrics,
                        ..Game::default()
                    };
                    g.select_puzzle(index);
                    if g.kind == Kind::Kakuro {
                        g.view = View::Entry;
                        g.entry = g.puzzle().cell_count() - 1;
                        let diagnostics = g
                            .screen()
                            .diagnostics(&metrics, &kobo_ui::Chrome::measuring(true));
                        assert!(
                            diagnostics.issues.is_empty(),
                            "{} entry {metrics:?}: {:?}",
                            g.puzzle().id,
                            diagnostics.issues
                        );
                    }
                    for (view, open) in [
                        (View::Puzzle, false),
                        (View::Puzzle, true),
                        (View::Help, false),
                    ] {
                        g.view = view;
                        g.menu = Menu::default();
                        if open {
                            g.menu.read(action_id(game::MORE));
                        }
                        for page in 0..=1 {
                            g.help_page = page;
                            let diagnostics = g
                                .screen()
                                .diagnostics(&metrics, &kobo_ui::Chrome::measuring(true));
                            assert!(
                                diagnostics.issues.is_empty(),
                                "{} {view:?} page{page} {metrics:?}: {:?}",
                                g.puzzle().id,
                                diagnostics.issues
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_list_page_holds_as_many_rows_as_fit_and_every_puzzle_can_be_reached() {
        for text_scale in [kobo_ui::TextScale::Default, kobo_ui::TextScale::Largest] {
            let metrics = kobo_sdk::DisplayMetrics {
                text_scale,
                ..kobo_sdk::DisplayMetrics::default()
            };
            let _fonts = fonts(metrics);
            for kind in KINDS {
                let mut g = Game {
                    loaded: true,
                    metrics,
                    ..Game::default()
                };
                g.on_action(&mut Context::default(), action_id(kind.name()));
                g.metrics = metrics;
                for level in 0..collection::LEVELS.len() {
                    g.on_action(
                        &mut Context::default(),
                        action_id(&format!("level-{level}")),
                    );
                    g.metrics = metrics;
                    let listed = g.listed(kind);
                    let mut reached = std::collections::BTreeSet::<usize>::new();
                    let pages = listed.len().div_ceil(g.page_size(kind, &listed));
                    for page in 0..pages {
                        let screen = g.collection_screen(kind);
                        let diagnostics =
                            screen.diagnostics(&metrics, &kobo_ui::Chrome::measuring(true));
                        assert!(
                            diagnostics.issues.is_empty(),
                            "{kind:?} {level} {text_scale:?}: {:?}",
                            diagnostics.issues
                        );
                        let layout =
                            screen.layout_with(&metrics, &kobo_ui::Chrome::measuring(true));
                        let shown: Vec<_> = listed
                            .iter()
                            .filter(|i| {
                                layout
                                    .rect_of_action(action_id(&format!("puzzle-{i}")))
                                    .is_some()
                            })
                            .collect();
                        if page + 1 < pages {
                            let least = if text_scale == kobo_ui::TextScale::Default {
                                6
                            } else {
                                3
                            };
                            assert!(
                                shown.len() >= least.min(listed.len()),
                                "{kind:?} {text_scale:?}: {}",
                                shown.len()
                            );
                        }
                        reached.extend(shown.into_iter().copied());
                        g.turn_page(kind, true);
                    }
                    assert_eq!(reached.len(), listed.len(), "{kind:?} level {level}");
                }
            }
        }
    }

    #[test]
    fn rows_say_which_puzzles_are_solved_or_under_way() {
        let mut g = Game {
            loaded: true,
            ..Game::default()
        };
        g.on_action(&mut Context::default(), action_id("hashi"));
        g.on_action(&mut Context::default(), action_id("puzzle-1"));
        for route in 0..4 {
            g.on_action(
                &mut Context::default(),
                action_id(&format!("route-{route}")),
            );
        }
        g.on_action(&mut Context::default(), action_id("next-puzzle"));
        g.on_action(&mut Context::default(), action_id("route-0"));
        let started = g.puzzle;
        g.on_action(&mut Context::default(), action_id("puzzles"));
        assert_eq!(g.view, View::Collection(Kind::Hashi));
        assert_eq!(g.row(1, false).2, "5 islands · Solved");
        assert!(g.row(started, false).2.ends_with(" · In progress"));
        assert_eq!(g.row(started, false).3, Glyph::Circle);
        assert_eq!(g.row(1, false).3, Glyph::Check);
    }

    #[test]
    fn full_collection_history_is_bounded_and_restore_keeps_every_identity() {
        let mut g = Game {
            loaded: true,
            ..Game::default()
        };
        for index in 0..collection::puzzles().len() {
            g.select_puzzle(index);
            if g.kind == Kind::Mines {
                for cell in 0..g.puzzle().cell_count() {
                    if !has_mine(g.mines, cell) {
                        g.tap_mine(cell);
                    }
                }
            } else {
                g.cells = g.puzzle().answer_cells();
                g.judge();
            }
            assert_eq!(g.outcome, Outcome::Solved, "{}", g.puzzle().id);
            let position = g.encode();
            g.undo = vec![position; saved::HISTORY];
            g.retain();
        }
        let bytes = saved::encode(&g.games, Some(g.puzzle));
        assert!(bytes.len() <= saved::LIMIT, "{}", bytes.len());
        let (games, current) = saved::decode(&bytes).expect("full collection restore");
        assert_eq!(current, Some(collection::puzzles().len() - 1));
        for (index, (run, kept)) in games.iter().zip(&g.games).enumerate() {
            assert_eq!(run.position, kept.position, "{index}");
            assert_eq!(g.progress(index), Progress::Solved);
        }
    }
}

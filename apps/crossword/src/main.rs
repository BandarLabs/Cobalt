//! Offline crossword play with an acknowledged, bounded save record.
mod game;
mod puzzles;
mod saved;
use game::{Progress, PUZZLES};
use kobo_sdk::game::{self as chrome, Menu, Tap};
use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{
    action_id, ActionId, Context, DialogAction, Glyph, KoboApp, Screen, ScreenBuilder, StoreResult,
};
use kobo_state::draft::{Draft, Status};
use std::process::ExitCode;

/// The order the shelf offers the puzzles in, which is not the order they are
/// stored in: the hand-written four lead with the easiest, and the generated
/// ones follow in the order they were made, where the two sizes alternate.
fn shelf_order() -> Vec<usize> {
    [3, 0, 1, 2].into_iter().chain(4..PUZZLES.len()).collect()
}

/// What the menu adds after New game, in the order a solver reaches for them.
const MENU: [(&str, &str); 5] = [
    ("check", "Check word"),
    ("reveal", "Reveal square"),
    ("clear", "Clear square"),
    ("restart", "Restart puzzle"),
    ("puzzles", "Puzzles"),
];

/// One line of the clue list: what a paper prints against a number, plus where
/// tapping it puts the reader.
struct ClueRow {
    heading: Option<&'static str>,
    clue: String,
    detail: String,
    first: usize,
    down: bool,
    number: u16,
}

const RULES: &str = "Fill the white squares\n\nRead a clue and fill in its answer. Small corner numbers match the clue list. The shaded row or column is the word you are solving, and its clue is above the grid. Some squares belong to one word only.\n\nTap a square to type in it. Enter one letter to move along the word, or the whole word to fill it from its first square. Tap another square in the word to move. Across or Down, at the foot of the grid and at the top while you type, switches direction.\n\nUndo, at the foot of the grid, takes back your last change, up to 32 steps per puzzle. Clues lists every clue; tap one to answer it.\n\nMore holds New game, which opens the next puzzle you have not started, then Check word, Reveal square, Clear square, Restart puzzle and Puzzles. Checking reports wrong and empty letters without changing them. Reveal and Restart ask first and can be undone.\n\nEach puzzle keeps its own letters, and stays marked Completed once solved, even after a restart.\n\nSaves are confirmed by storage. If a save fails, keep the app open and choose Retry saving. Unreadable records are kept intact.";
#[derive(Clone, Copy, Default, Eq, PartialEq)]
enum View {
    #[default]
    Puzzles,
    Board,
    Entry,
    Clues,
    Reveal,
    Restart,
    Rules,
    Result,
}
struct Crossword {
    games: Vec<Progress>,
    current: usize,
    view: View,
    keyboard: Keyboard,
    loaded: bool,
    load_error: Option<String>,
    draft: Draft,
    active: Option<u64>,
    notice: Option<String>,
    rules_page: usize,
    clue_page: usize,
    puzzle_page: usize,
    rules_from_puzzles: bool,
    menu: Menu,
}
impl Default for Crossword {
    fn default() -> Self {
        Self {
            games: PUZZLES.iter().map(Progress::new).collect(),
            current: 0,
            view: View::Puzzles,
            keyboard: Keyboard::new(),
            loaded: false,
            load_error: None,
            draft: Draft::restored(Vec::new(), saved::LIMIT).expect("empty draft"),
            active: None,
            notice: None,
            rules_page: 0,
            clue_page: 0,
            puzzle_page: 0,
            rules_from_puzzles: false,
            menu: Menu::default(),
        }
    }
}
impl Crossword {
    fn game(&self) -> &Progress {
        &self.games[self.current]
    }
    fn game_mut(&mut self) -> &mut Progress {
        &mut self.games[self.current]
    }
    fn show(&self, context: &mut Context) {
        context.set_screen(self.screen(context));
    }
    fn save(&mut self, context: &mut Context) {
        self.draft
            .replace(saved::encode(&self.games, self.current))
            .expect("bounded draft");
        self.pump(context);
    }
    fn pump(&mut self, context: &mut Context) {
        if let Some(write) = self.draft.begin() {
            self.active = Some(write.revision);
            context.store().save(saved::KEY, write.bytes);
        }
    }
    fn screen(&self, context: &Context) -> Screen {
        let mut b = ScreenBuilder::new("crossword").top_bar("Crossword");
        // Both failures fill the screen, so each is a splash with its fix
        // directly under the reason rather than a heading at the top and the
        // fix pinned to the foot with an empty panel between them.
        if !self.loaded {
            return if let Some(error) = &self.load_error {
                failure_screen(
                    || ScreenBuilder::new("crossword").top_bar("Crossword"),
                    &context.metrics(),
                    "Cannot open progress",
                    (&format!("{error} Your saved game has been kept."), error),
                    ("retry-load", "Try again"),
                )
            } else {
                b.text("Opening puzzles…").build()
            };
        }
        b = b.owns_back(self.view != View::Puzzles);
        let p = &PUZZLES[self.current];
        let g = self.game();
        let (label, _) = p.clue(g.position.selected, g.position.down);
        // The board ignores input until the save succeeds, so this replaces
        // it rather than sitting over it as a banner.
        if matches!(self.draft.status(), Status::Failed(_)) {
            let away = self.view != View::Puzzles;
            return failure_screen(
                || {
                    ScreenBuilder::new("crossword")
                        .top_bar("Crossword")
                        .owns_back(away)
                },
                &context.metrics(),
                "Progress is not saved",
                (
                    "Your latest letters are still here. Keep the app open while you make room on your reader, then retry.",
                    "Keep the app open while you make room on your reader, then retry.",
                ),
                ("retry-save", "Retry saving"),
            );
        }
        match self.view {
            View::Puzzles => self.puzzles_screen(b, context),
            View::Board => self.board_screen(b),
            View::Entry => self.entry_screen(b),
            View::Clues => self.clues_screen(b, context),
            View::Reveal => b
                .confirmation(
                    "Reveal square?",
                    "Adds one reveal.",
                    DialogAction::new("confirm-reveal", "Reveal"),
                    DialogAction::new("board", "Keep playing"),
                )
                .build(),
            View::Restart => b
                .confirmation(
                    "Restart puzzle?",
                    "Clear the letters.",
                    DialogAction::new("confirm-restart", "Restart"),
                    DialogAction::new("board", "Keep playing"),
                )
                .build(),
            View::Result => b
                .top_bar(label)
                .text(self.notice.as_deref().unwrap_or("Puzzle complete."))
                .bottom_action("board", "Back to puzzle")
                .build(),
            View::Rules => {
                let pages = context.paginate(RULES, true);
                let page = self.rules_page.min(pages.len().saturating_sub(1));
                b = b.top_bar(chrome::RULES_LABEL);
                for paragraph in &pages[page] {
                    b = b.text(paragraph);
                }
                b.page_position(
                    u16::try_from(page + 1).expect("bounded rules"),
                    u16::try_from(pages.len()).expect("bounded rules"),
                )
                .action_bar([("rules-previous", "Previous"), ("rules-next", "Next")])
                .build()
            }
        }
    }
    /// What each puzzle is, on one row: its name, and how far it has got.
    ///
    /// The list drew a half-width button per puzzle with its details hanging
    /// off to the left of it, two to a page, on a panel with room for every
    /// puzzle at once. A list of things to open is a list of rows on this
    /// device, and how many fit is measured rather than assumed.
    fn puzzle_rows(&self) -> Vec<(String, String)> {
        shelf_order()
            .iter()
            .map(|index| {
                let puzzle = &PUZZLES[*index];
                let game = &self.games[*index];
                let filled = game
                    .position
                    .letters
                    .iter()
                    .filter(|letter| **letter != b'.' && **letter != b'#')
                    .count();
                let squares = puzzle.answer.iter().filter(|cell| **cell != b'#').count();
                (
                    puzzle.title.to_owned(),
                    format!(
                        "{} · {}×{} · {}",
                        puzzle.level,
                        puzzle.side,
                        puzzle.side,
                        if game.solved_once {
                            "Completed".to_owned()
                        } else {
                            format!("{filled}/{squares} squares")
                        }
                    ),
                )
            })
            .collect()
    }

    fn puzzle_pages(&self, context: &Context) -> Vec<Vec<usize>> {
        let rows = self.puzzle_rows();
        let borrowed: Vec<(&str, &str)> = rows
            .iter()
            .map(|(title, detail)| (title.as_str(), detail.as_str()))
            .collect();
        let prefix = ScreenBuilder::new("crossword")
            .top_bar("Crossword")
            .top_bar_action(chrome::RULES, chrome::RULES_LABEL)
            .build();
        let pages =
            context.paginate_rows_under(&borrowed, true, kobo_sdk::Position::AtTheFoot, &prefix);
        if pages.is_empty() {
            vec![Vec::new()]
        } else {
            pages
        }
    }

    /// Shows the shelf at the page holding the puzzle just played, since with
    /// sixty puzzles the first page is rarely the one anybody wants.
    fn open_shelf(&mut self, context: &Context) {
        let order = shelf_order();
        let place = order.iter().position(|index| *index == self.current);
        self.puzzle_page = self
            .puzzle_pages(context)
            .iter()
            .position(|page| place.is_some_and(|place| page.contains(&place)))
            .unwrap_or(0);
        self.view = View::Puzzles;
    }

    /// The puzzle New game opens: the next one along the shelf nobody has
    /// touched, or failing that the next one not yet completed.
    fn next_puzzle(&self) -> Option<usize> {
        let order = shelf_order();
        let place = order
            .iter()
            .position(|index| *index == self.current)
            .unwrap_or(0);
        let after: Vec<usize> = (1..order.len())
            .map(|step| order[(place + step) % order.len()])
            .collect();
        after
            .iter()
            .find(|index| self.games[**index] == Progress::new(&PUZZLES[**index]))
            .or_else(|| after.iter().find(|index| !self.games[**index].solved_once))
            .copied()
    }

    fn puzzles_screen(&self, mut b: ScreenBuilder, context: &Context) -> Screen {
        b = b.top_bar_action(chrome::RULES, chrome::RULES_LABEL);
        let order = shelf_order();
        let rows = self.puzzle_rows();
        let pages = self.puzzle_pages(context);
        let page = self.puzzle_page.min(pages.len().saturating_sub(1));
        for index in &pages[page] {
            let (title, detail) = &rows[*index];
            b = b.rows([(
                format!("puzzle-{}", order[*index]),
                title.as_str(),
                detail.as_str(),
                if self.games[order[*index]].solved_once {
                    Glyph::Check
                } else {
                    Glyph::Grid
                },
            )]);
        }
        if pages.len() > 1 {
            b = b
                .page_position(
                    u16::try_from(page + 1).unwrap_or(1),
                    u16::try_from(pages.len()).unwrap_or(1),
                )
                .action_bar([("puzzles-previous", "Previous"), ("puzzles-next", "Next")]);
        }
        b.build()
    }

    fn board_screen(&self, mut b: ScreenBuilder) -> Screen {
        let p = &PUZZLES[self.current];
        let g = self.game();
        let (label, clue) = p.clue(g.position.selected, g.position.down);

        let word = p.word(g.position.selected, g.position.down);
        let line = if let Some(notice) = &self.notice {
            notice.clone()
        } else if g.solved(p) {
            completed_line(g)
        } else {
            format!("{label}: {clue}")
        };
        b = b.game_top_bar(p.title, self.menu, MENU).secondary(line);
        // Footer controls reserve their band before the square grid is measured.
        b.crossword_board(
            u8::try_from(p.side).expect("small puzzle"),
            (0..p.answer.len()).map(|cell| {
                (
                    format!("cell-{cell}"),
                    if g.position.letters[cell] == b'.' {
                        ' '
                    } else {
                        char::from(g.position.letters[cell])
                    },
                    p.number(cell).and_then(|n| u8::try_from(n).ok()),
                    !g.solved(p) && word.contains(&cell),
                )
            }),
        )
        // Undo, then the two things done most between letters: the clue list,
        // and turning the shaded word the other way.
        .game_actions(
            true,
            [
                ("clues", "Clues"),
                ("direction", direction_label(g.position.down)),
            ],
        )
        .build()
    }
    fn entry_screen(&self, b: ScreenBuilder) -> Screen {
        let p = &PUZZLES[self.current];
        let g = self.game();
        let (label, clue) = p.clue(g.position.selected, g.position.down);

        let word = p.word(g.position.selected, g.position.down);
        let mut entry = b
            .top_bar(label)
            // The direction it switches to, in words. A bare arrow in the bar
            // gave no hint that it was a control, or which way it would go.
            .top_bar_action("direction", direction_label(g.position.down))
            .text(clue)
            .grid_with_selection(
                u8::try_from(word.len()).expect("small word"),
                false,
                word.iter().map(|cell| {
                    (
                        format!("entry-{cell}"),
                        if g.position.letters[*cell] == b'.' {
                            " ".into()
                        } else {
                            char::from(g.position.letters[*cell]).to_string()
                        },
                        *cell == g.position.selected,
                    )
                }),
            )
            .typed(&self.keyboard, "Letter or whole word");
        if let Some(notice) = &self.notice {
            entry = entry.secondary(notice);
        }
        entry.keyboard(&self.keyboard, "Enter").build()
    }
    /// Every clue the puzzle has, the way a paper prints them: Across first,
    /// then Down, each against its own number.
    ///
    /// This listed two clues to a page, indexed by row and column, which only
    /// worked while a grid held one entry per row. On a grid with black
    /// squares row zero began on a black square, and opening the list took the
    /// application down with it.
    fn clue_rows(&self) -> Vec<ClueRow> {
        let puzzle = &PUZZLES[self.current];
        let game = self.game();
        let solving = puzzle.run(game.position.selected, game.position.down);
        let mut across_heading = true;
        let mut down_heading = true;
        let mut rows = Vec::new();
        for entry in puzzle.entries() {
            let heading = if entry.down {
                std::mem::take(&mut down_heading).then_some("Down")
            } else {
                std::mem::take(&mut across_heading).then_some("Across")
            };
            let length = puzzle.run(entry.first, entry.down).len();
            let here = entry.down == game.position.down && solving.first() == Some(&entry.first);
            rows.push(ClueRow {
                heading,
                clue: puzzle.clue_of(&entry).to_owned(),
                detail: if here {
                    "Solving now".to_owned()
                } else {
                    format!("{length} letters")
                },
                first: entry.first,
                down: entry.down,
                number: u16::try_from(entry.number).unwrap_or(u16::MAX),
            });
        }
        rows.sort_by_key(|row| (row.down, row.number));
        // Sorting moved the headings, so they are placed again on whichever
        // row now opens each direction.
        let mut across_heading = true;
        let mut down_heading = true;
        for row in &mut rows {
            row.heading = if row.down {
                std::mem::take(&mut down_heading).then_some("Down")
            } else {
                std::mem::take(&mut across_heading).then_some("Across")
            };
        }
        rows
    }

    fn clue_pages(&self, context: &Context) -> Vec<Vec<usize>> {
        let rows = self.clue_rows();
        let borrowed: Vec<(Option<&str>, &str, &str)> = rows
            .iter()
            .map(|row| (row.heading, row.clue.as_str(), row.detail.as_str()))
            .collect();
        let prefix = ScreenBuilder::new("crossword")
            .top_bar("Clues")
            .top_bar_action("board", "Grid")
            .build();
        let pages = context.paginate_rows_in_sections_under(
            &borrowed,
            true,
            kobo_sdk::Position::AtTheFoot,
            &prefix,
        );
        if pages.is_empty() {
            vec![Vec::new()]
        } else {
            pages
        }
    }

    fn clues_screen(&self, mut b: ScreenBuilder, context: &Context) -> Screen {
        let rows = self.clue_rows();
        let pages = self.clue_pages(context);
        let page = self.clue_page.min(pages.len().saturating_sub(1));
        b = b.top_bar("Clues").top_bar_action("board", "Grid");
        for index in &pages[page] {
            let row = &rows[*index];
            if let Some(heading) = row.heading {
                b = b.section(heading);
            }
            b = b.rows([(
                format!("clue-{index}"),
                row.clue.as_str(),
                row.detail.as_str(),
                kobo_sdk::RowLead::Number(row.number),
            )]);
        }
        if pages.len() > 1 {
            b = b
                .page_position(
                    u16::try_from(page + 1).unwrap_or(1),
                    u16::try_from(pages.len()).unwrap_or(1),
                )
                .action_bar([("clues-previous", "Previous"), ("clues-next", "Next")]);
        }
        b.build()
    }

    fn clues_action(&mut self, context: &Context, action: ActionId) {
        let is = |name: &str| action == action_id(name);

        if is("clues-previous") {
            self.clue_page = self.clue_page.saturating_sub(1);
        } else if is("clues-next") {
            self.clue_page =
                (self.clue_page + 1).min(self.clue_pages(context).len().saturating_sub(1));
        } else if is("board") {
            self.view = View::Board;
        } else {
            let rows = self.clue_rows();
            if let Some(row) = rows
                .iter()
                .enumerate()
                .find(|(index, _)| is(&format!("clue-{index}")))
                .map(|(_, row)| row)
            {
                self.game_mut().position.selected = row.first;
                self.game_mut().position.down = row.down;
                self.keyboard.clear();
                self.view = View::Entry;
            }
        }
    }
    fn entry_action(&mut self, action: ActionId) {
        let p = &PUZZLES[self.current];
        let is = |name: &str| action == action_id(name);

        if is("direction") {
            self.game_mut().position.down = !self.game().position.down;
            self.keyboard.clear();
        } else if let Some(cell) = p
            .word(self.game().position.selected, self.game().position.down)
            .into_iter()
            .find(|c| is(&format!("entry-{c}")))
        {
            self.game_mut().position.selected = cell;
            self.keyboard.clear();
        } else if let Some(pressed) = self.keyboard.press(action) {
            self.notice = None;
            if pressed == Pressed::Submitted {
                let text = self.keyboard.text().to_owned();
                match self.game_mut().enter(p, &text) {
                    Ok(()) => {
                        self.keyboard.clear();
                        self.notice = None;
                        if text.len() > 1 || self.game().solved(p) {
                            self.view = View::Board;
                        }
                    }
                    Err(message) => self.notice = Some(message.into()),
                }
            }
        }
    }
    /// The play screen's own taps: a square, the foot bar, and the menu
    /// entries the game added after New game.
    fn board_action(&mut self, context: &Context, action: ActionId) {
        let p = &PUZZLES[self.current];
        let is = |name: &str| action == action_id(name);

        if let Some(cell) = (0..p.answer.len())
            .filter(|i| p.answer[*i] != b'#')
            .find(|i| is(&format!("cell-{i}")))
        {
            self.game_mut().position.selected = cell;
            self.keyboard.clear();
            self.view = View::Entry;
        } else if is("clues") {
            self.clue_page = 0;
            self.view = View::Clues;
        } else if is("direction") {
            self.game_mut().position.down = !self.game().position.down;
        } else if is("clear") {
            let g = self.game_mut();
            if g.position.letters[g.position.selected] == b'.' {
                self.notice = Some("This square is already empty.".into());
            } else {
                g.remember();
                g.position.letters[g.position.selected] = b'.';
            }
        } else if is("check") {
            self.notice = Some(self.game_mut().check(p));
            self.view = View::Result;
        } else if is("reveal") {
            self.view = View::Reveal;
        } else if is("restart") {
            self.view = View::Restart;
        } else if is("puzzles") {
            self.open_shelf(context);
        }
    }
    /// Undo, Rules and New game, which sit in the same place in every game.
    fn shared_tap(&mut self, tap: Tap) {
        match tap {
            Tap::Menu => (),
            Tap::Undo => {
                if let Some(previous) = self.game_mut().undo.pop_back() {
                    self.game_mut().position = previous;
                } else {
                    self.notice = Some(chrome::NOTHING_TO_UNDO.into());
                }
            }
            Tap::Rules => {
                self.rules_page = 0;
                self.rules_from_puzzles = self.view == View::Puzzles;
                self.view = View::Rules;
            }
            Tap::NewGame => {
                // Every puzzle keeps its own letters, so moving to another
                // throws nothing away and needs no confirmation.
                if let Some(next) = self.next_puzzle() {
                    self.current = next;
                    self.keyboard.clear();
                    self.view = View::Board;
                } else {
                    self.notice = Some(
                        "You have completed every puzzle. Restart puzzle, in More, clears one to play again."
                            .into(),
                    );
                }
            }
        }
    }
    fn accept_key(&mut self, action: ActionId) -> bool {
        let p = &PUZZLES[self.current];
        let is = |name: &str| action == action_id(name);

        if let Some(ch) = self
            .keyboard
            .resolves(action)
            .or_else(|| is("kb.space").then_some(' '))
        {
            let length = p
                .word(self.game().position.selected, self.game().position.down)
                .len();
            if !ch.is_ascii_alphabetic() || self.keyboard.text().len() >= length {
                self.notice = Some(
                    if ch.is_ascii_alphabetic() {
                        "Use Delete to change the answer."
                    } else {
                        "Use letters A–Z."
                    }
                    .into(),
                );
                return false;
            }
        }
        true
    }
}
impl KoboApp for Crossword {
    fn on_start(&mut self, context: &mut Context) {
        context.set_orientation(kobo_sdk::Orientation::Portrait);
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
            } => match saved::decode(&bytes) {
                Ok((games, current)) => {
                    self.games = games;
                    self.current = current;
                    self.view = View::Board;
                    self.draft = Draft::restored(bytes, saved::LIMIT).expect("validated record");
                    self.loaded = true;
                    self.load_error = None;
                }
                Err(error) => self.load_error = Some(error.to_string()),
            },
            StoreResult::Loaded { value: None, .. } => {
                self.loaded = true;
                self.load_error = None;
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
        let is = |s: &str| action == action_id(s);
        if !self.loaded {
            if is("retry-load") && self.load_error.take().is_some() {
                context.store().load(saved::KEY);
                self.show(context);
            }
            return;
        }
        if matches!(self.draft.status(), Status::Failed(_)) {
            if is("retry-save") {
                self.draft.retry();
                self.pump(context);
                self.show(context);
            }
            return;
        }
        let before = self.games.clone();
        let old_current = self.current;
        let p = &PUZZLES[self.current];
        if self.view == View::Entry && !self.accept_key(action) {
            self.show(context);
            return;
        }
        // The shared controls live on the shelf and the play screen only, so
        // nothing a confirmation or the keyboard raises is read as one.
        let tap = if matches!(self.view, View::Board | View::Puzzles) {
            if self.view == View::Board {
                // A notice answers the tap before this one and goes with it.
                self.notice = None;
            }
            self.menu.read(action)
        } else {
            None
        };
        match tap {
            Some(tap) => self.shared_tap(tap),
            None if action == ActionId::BACK || is("board") => {
                self.keyboard.clear();
                self.notice = None;
                if self.view == View::Board || (self.view == View::Rules && self.rules_from_puzzles)
                {
                    self.open_shelf(context);
                } else {
                    self.view = View::Board;
                }
            }
            None => match self.view {
                View::Puzzles => {
                    if let Some(index) = (0..PUZZLES.len()).find(|i| is(&format!("puzzle-{i}"))) {
                        self.current = index;
                        self.view = View::Board;
                    } else if is("puzzles-previous") {
                        self.puzzle_page = self.puzzle_page.saturating_sub(1);
                    } else if is("puzzles-next") {
                        self.puzzle_page = (self.puzzle_page + 1)
                            .min(self.puzzle_pages(context).len().saturating_sub(1));
                    }
                }
                View::Board => self.board_action(context, action),
                View::Entry => self.entry_action(action),
                View::Clues => self.clues_action(context, action),
                View::Reveal if is("confirm-reveal") => {
                    self.game_mut().reveal(p);
                    self.view = View::Board;
                }
                View::Restart if is("confirm-restart") => {
                    let g = self.game_mut();
                    g.remember();
                    g.position.letters = Progress::new(p).position.letters;
                    self.view = View::Board;
                }
                View::Rules => {
                    if is("rules-previous") {
                        self.rules_page = self.rules_page.saturating_sub(1);
                    } else if is("rules-next") {
                        self.rules_page = (self.rules_page + 1)
                            .min(context.paginate(RULES, true).len().saturating_sub(1));
                    }
                }
                View::Reveal | View::Restart | View::Result => (),
            },
        }
        if before != self.games || old_current != self.current {
            self.save(context);
        }
        self.show(context);
    }
}
/// The direction switch names the way it turns the word, in words: a bare
/// arrow gave no hint that it was a control, or which way it would go.
const fn direction_label(down: bool) -> &'static str {
    if down {
        "Across"
    } else {
        "Down"
    }
}

/// The line above a solved grid, with the help it took, since the menu
/// screen that used to show the counts is gone.
fn completed_line(game: &Progress) -> String {
    let count = |n: u32, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    if game.checks == 0 && game.reveals == 0 {
        "Puzzle complete".into()
    } else {
        format!(
            "Puzzle complete · {} · {}",
            count(game.checks, "check", "checks"),
            count(game.reveals, "reveal", "reveals")
        )
    }
}

/// A whole-screen failure: a splash saying why, then its one fix directly
/// under it. On a small panel at the largest text sizes the mark gives way
/// first and then the longer sentence, so the reason and the fix are always
/// on the panel together.
fn failure_screen(
    head: impl Fn() -> ScreenBuilder,
    metrics: &kobo_sdk::DisplayMetrics,
    title: &str,
    (full, short): (&str, &str),
    (retry, label): (&str, &str),
) -> Screen {
    let build = |glyph, summary: &str| {
        head()
            .splash(glyph, title, summary)
            .primary_button(retry, label)
            .build()
    };
    [(Some(Glyph::Close), full), (None, full), (None, short)]
        .into_iter()
        .map(|(glyph, summary)| build(glyph, summary))
        .find(|screen| {
            !screen
                .diagnostics(metrics, &kobo_sdk::Chrome::measuring(true))
                .has_errors()
        })
        .unwrap_or_else(|| build(None, short))
}
fn main() -> ExitCode {
    match kobo_sdk::run("crossword", Crossword::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("crossword: {error}");
            ExitCode::FAILURE
        }
    }
}
#[cfg(test)]
mod tests;

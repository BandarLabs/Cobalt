//! Offline crossword play with an acknowledged, bounded save record.
mod game;
mod saved;
use game::{Progress, PUZZLES};
use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{
    action_id, ActionId, Context, DialogAction, KoboApp, Screen, ScreenBuilder, StoreResult,
};
use kobo_state::draft::{Draft, Status};
use std::process::ExitCode;

const HELP: &str = "Fill the white squares\n\nRead an Across or Down clue and tap a square to enter its answer. Small corner numbers match the clue list. The shaded row or column is the active word.\n\nEnter one letter or the whole word. One letter advances within the word; a whole word fills from its first square. The outlined square above the keyboard is your target. Tap another square in that word to move. The arrow changes between Across and Down.\n\nUse More for Undo, Clear square, Check word, Reveal square and Restart. Checking reports incorrect and empty letters without changing them. Reveal and Restart ask first and can be undone. Assistance counts are retained.\n\nPuzzles keeps separate progress for all four puzzles. Starter, Easy and Medium are editorial guides based on size and vocabulary. Answers read across and down; the clues differ. Odds and ends has black squares; the other three are word squares.\n\nSaves are confirmed by storage. If a save fails, keep the app open and choose Retry save. Unreadable records are kept intact. Completed means you have solved that puzzle at least once; undo can reopen it.";
#[derive(Clone, Copy, Default, Eq, PartialEq)]
enum View {
    #[default]
    Puzzles,
    Board,
    Entry,
    Clues,
    More,
    Reveal,
    Restart,
    Help,
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
    help_page: usize,
    clue_page: usize,
    puzzle_page: usize,
    help_from_puzzles: bool,
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
            help_page: 0,
            clue_page: 0,
            puzzle_page: 0,
            help_from_puzzles: false,
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
        if !self.loaded {
            return if let Some(error) = &self.load_error {
                b.heading("Cannot open progress")
                    .text(error)
                    .text("Your saved game has been kept.")
                    .bottom_action("retry-load", "Retry")
                    .build()
            } else {
                b.text("Opening puzzles…").build()
            };
        }
        b = b.owns_back(self.view != View::Puzzles);
        let p = &PUZZLES[self.current];
        let g = self.game();
        let (label, _) = p.clue(g.position.selected, g.position.down);
        if matches!(self.draft.status(), Status::Failed(_)) {
            return b.heading("Progress is not saved").text("Your latest letters are still here. Keep the app open while you make room on your reader, then retry.")
                .bottom_action("retry-save", "Retry save").build();
        }
        match self.view {
            View::Puzzles => self.puzzles_screen(b),
            View::Board => self.board_screen(b),
            View::Entry => self.entry_screen(b),
            View::Clues => self.clues_screen(b),
            View::More => self.more_screen(b),
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
            View::Help => {
                let pages = context.paginate(HELP, true);
                let page = self.help_page.min(pages.len().saturating_sub(1));
                for paragraph in &pages[page] {
                    b = b.text(paragraph);
                }
                b.page_position(
                    u16::try_from(page + 1).expect("bounded help"),
                    u16::try_from(pages.len()).expect("bounded help"),
                )
                .action_bar([("help-previous", "Previous"), ("help-next", "Next")])
                .build()
            }
        }
    }
    fn puzzles_screen(&self, mut b: ScreenBuilder) -> Screen {
        b = b.top_bar_action("help", "Help");
        for index in [3, 0, 1, 2].into_iter().skip(self.puzzle_page * 2).take(2) {
            let p = &PUZZLES[index];
            let game = &self.games[index];
            let filled = game
                .position
                .letters
                .iter()
                .filter(|b| **b != b'.' && **b != b'#')
                .count();
            b = b
                .button(format!("puzzle-{index}"), p.title)
                .secondary(format!(
                    "{} · {}×{} · {}",
                    p.level,
                    p.side,
                    p.side,
                    if game.solved_once {
                        "Completed".into()
                    } else {
                        format!(
                            "{filled}/{} squares",
                            p.answer.iter().filter(|b| **b != b'#').count()
                        )
                    }
                ));
        }
        b.action_bar([("puzzles-previous", "Previous"), ("puzzles-next", "Next")])
            .build()
    }
    fn board_screen(&self, mut b: ScreenBuilder) -> Screen {
        let p = &PUZZLES[self.current];
        let g = self.game();
        let (label, clue) = p.clue(g.position.selected, g.position.down);

        let word = p.word(g.position.selected, g.position.down);
        b = b.top_bar(p.title).secondary(if g.solved(p) {
            "Puzzle complete".into()
        } else {
            format!("{label}: {clue}")
        });
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
        .action_bar([("clues", "Clues"), ("more", "More")])
        .build()
    }
    fn entry_screen(&self, b: ScreenBuilder) -> Screen {
        let p = &PUZZLES[self.current];
        let g = self.game();
        let (label, clue) = p.clue(g.position.selected, g.position.down);

        let word = p.word(g.position.selected, g.position.down);
        let mut entry = b
            .top_bar(label)
            .top_bar_action("direction", if g.position.down { "→" } else { "↓" })
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
    fn clues_screen(&self, mut b: ScreenBuilder) -> Screen {
        let p = &PUZZLES[self.current];
        let g = self.game();

        let offset = self.clue_page * 2;
        b = b
            .top_bar(if g.position.down { "Down" } else { "Across" })
            .top_bar_action("direction", if g.position.down { "→" } else { "↓" });
        for n in offset..(offset + 2).min(p.side) {
            let cell = if g.position.down { n } else { n * p.side };
            let (title, text) = p.clue(cell, g.position.down);
            b = b.button(format!("clue-{n}"), title).text(text);
        }
        b.action_bar([("clues-previous", "Previous"), ("clues-next", "Next")])
            .build()
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
    fn clues_action(&mut self, action: ActionId) {
        let p = &PUZZLES[self.current];
        let is = |name: &str| action == action_id(name);

        if is("direction") {
            self.game_mut().position.down = !self.game().position.down;
            self.clue_page = 0;
        } else if is("clues-previous") {
            self.clue_page = self.clue_page.saturating_sub(1);
        } else if is("clues-next") {
            self.clue_page = (self.clue_page + 1).min((p.side - 1) / 2);
        } else if let Some(n) = (self.clue_page * 2..(self.clue_page * 2 + 2).min(p.side))
            .find(|n| is(&format!("clue-{n}")))
        {
            let first = if self.game().position.down {
                n
            } else {
                n * p.side
            };
            self.game_mut().position.selected = p.word(first, self.game().position.down)[0];
            self.keyboard.clear();
            self.view = View::Entry;
        }
    }
    fn more_action(&mut self, action: ActionId) {
        let p = &PUZZLES[self.current];
        let is = |name: &str| action == action_id(name);

        if is("undo") {
            if let Some(previous) = self.game_mut().undo.pop_back() {
                self.game_mut().position = previous;
            }
            self.view = View::Board;
        } else if is("clear") {
            let g = self.game_mut();
            if g.position.letters[g.position.selected] != b'.' {
                g.remember();
                g.position.letters[g.position.selected] = b'.';
            }
            self.view = View::Board;
        } else if is("check") {
            self.notice = Some(self.game_mut().check(p));
            self.view = View::Result;
        } else if is("reveal") {
            self.view = View::Reveal;
        } else if is("restart") {
            self.view = View::Restart;
        } else if is("puzzles") {
            self.view = View::Puzzles;
        } else if is("help") {
            self.help_page = 0;
            self.help_from_puzzles = false;
            self.view = View::Help;
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
    fn more_screen(&self, b: ScreenBuilder) -> Screen {
        let g = self.game();
        let mut b = b.top_bar("Puzzle options").secondary(format!(
            "{} checks · {} reveals · {}",
            g.checks,
            g.reveals,
            if matches!(self.draft.status(), Status::Saved) {
                "Saved"
            } else {
                "Saving…"
            }
        ));
        b = if g.undo.is_empty() {
            b.disabled_button("undo", "Undo")
        } else {
            b.button("undo", "Undo")
        };
        b = if g.position.letters[g.position.selected] == b'.' {
            b.disabled_button("clear", "Clear square")
        } else {
            b.button("clear", "Clear square")
        };
        b.grid(
            2,
            false,
            [
                ("check", "Check word"),
                ("reveal", "Reveal"),
                ("restart", "Restart"),
                ("puzzles", "Puzzles"),
            ],
        )
        .bottom_action("help", "How to play")
        .build()
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
        if action == ActionId::BACK || is("board") {
            self.keyboard.clear();
            self.notice = None;
            self.view = if self.view == View::Board
                || (self.view == View::Help && self.help_from_puzzles)
            {
                View::Puzzles
            } else {
                View::Board
            };
        } else {
            match self.view {
                View::Puzzles => {
                    if let Some(index) = (0..PUZZLES.len()).find(|i| is(&format!("puzzle-{i}"))) {
                        self.current = index;
                        self.view = View::Board;
                    } else if is("help") {
                        self.help_page = 0;
                        self.help_from_puzzles = true;
                        self.view = View::Help;
                    } else if is("puzzles-previous") {
                        self.puzzle_page = self.puzzle_page.saturating_sub(1);
                    } else if is("puzzles-next") {
                        self.puzzle_page = (self.puzzle_page + 1).min(1);
                    }
                }
                View::Board => {
                    if let Some(cell) = (0..p.answer.len())
                        .filter(|i| p.answer[*i] != b'#')
                        .find(|i| is(&format!("cell-{i}")))
                    {
                        self.game_mut().position.selected = cell;
                        self.keyboard.clear();
                        self.notice = None;
                        self.view = View::Entry;
                    } else if is("clues") {
                        self.clue_page = 0;
                        self.view = View::Clues;
                    } else if is("more") {
                        self.view = View::More;
                    }
                }
                View::Entry => self.entry_action(action),
                View::Clues => self.clues_action(action),
                View::More => self.more_action(action),
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
                View::Help => {
                    if is("help-previous") {
                        self.help_page = self.help_page.saturating_sub(1);
                    } else if is("help-next") {
                        self.help_page = (self.help_page + 1)
                            .min(context.paginate(HELP, true).len().saturating_sub(1));
                    }
                }
                View::Reveal | View::Restart | View::Result => (),
            }
        }
        if before != self.games || old_current != self.current {
            self.save(context);
        }
        self.show(context);
    }
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

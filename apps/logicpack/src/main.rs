//! Four small, deterministic pencil puzzles that work fully offline.

use kobo_sdk::{action_id, ActionId, Context, KoboApp, Screen, ScreenBuilder, StoreResult};
use kobo_state::draft::{Draft, Status};
use std::process::ExitCode;
mod boards;
mod saved;

const STATE: &str = "logicpack-state-v1";
const SLITHER_TARGET: u16 = 0b1011_0111_0011;
const KAKURO_SOLUTION: [u8; 3] = [3, 2, 4];
const INITIAL_MINES: u16 = (1 << 5) | (1 << 10) | (1 << 15);

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
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Outcome {
    #[default]
    Playing,
    Lost,
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
}

struct Game {
    kind: Kind,
    cells: [u8; 16],
    notice: String,
    view: View,
    mines: u16,
    flagging: bool,
    first_reveal: bool,
    outcome: Outcome,
    loaded: bool,
    load_error: Option<String>,
    draft: Draft,
    active: Option<u64>,
    games: [saved::Run; 4],
    undo: Vec<Vec<u8>>,
}

impl Default for Game {
    fn default() -> Self {
        Self {
            kind: Kind::Home,
            cells: [0; 16],
            notice: "Choose a puzzle.".into(),
            view: View::Puzzle,
            mines: INITIAL_MINES,
            flagging: false,
            first_reveal: true,
            outcome: Outcome::Playing,
            loaded: false,
            load_error: None,
            draft: Draft::restored(Vec::new(), saved::LIMIT).expect("empty draft"),
            active: None,
            games: std::array::from_fn(|_| saved::Run::default()),
            undo: Vec::new(),
        }
    }
}

impl Game {
    fn retain(&mut self) {
        if self.kind != Kind::Home {
            self.games[usize::from(self.kind.stored() - 1)] = saved::Run {
                position: self.encode(),
                undo: self.undo.clone(),
            };
        }
    }
    fn select(&mut self, kind: Kind) {
        self.retain();
        self.fresh(kind);
        if kind != Kind::Home {
            let run = self.games[usize::from(kind.stored() - 1)].clone();
            if !run.position.is_empty() {
                assert!(self.restore(&run.position), "validated run");
                self.undo = run.undo;
            }
        }
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
            .replace(saved::encode(&self.games, self.kind))
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
        self.cells = [0; 16];
        self.mines = INITIAL_MINES;
        self.flagging = false;
        self.first_reveal = true;
        self.outcome = Outcome::Playing;
        self.notice = match kind {
            Kind::Slither => "Draw one loop around the four 2 clues.",
            Kind::Hashi => "Join each outer island to the centre.",
            Kind::Kakuro => "Fill the three white cells from the sum clues.",
            Kind::Mines => "Reveal every safe square. Your first reveal is safe.",
            Kind::Home => "Choose a puzzle.",
        }
        .into();
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
        if self.view == View::Restart {
            return ScreenBuilder::new("logicpack-restart")
                .top_bar("Restart puzzle")
                .owns_back(true)
                .heading("Clear this puzzle?")
                .text("The other games will keep their progress. You can undo this restart.")
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
                .top_bar_action("how-to-play", "Help")
                .rows([
                    ("slither", "Slitherlink", "One loop", kobo_sdk::Glyph::Grid),
                    ("hashi", "Hashi", "Connect islands", kobo_sdk::Glyph::Grid),
                    ("kakuro", "Kakuro", "Cross sums", kobo_sdk::Glyph::Grid),
                    (
                        "mines",
                        "Minesweeper",
                        "Clear the field",
                        kobo_sdk::Glyph::Grid,
                    ),
                ])
                .build(),
            Kind::Slither => self.slither_screen(),
            Kind::Hashi => self.hashi_screen(),
            Kind::Kakuro => self.kakuro_screen(),
            Kind::Mines => self.mines_screen(),
        }
    }

    fn help_screen(&self) -> Screen {
        let (title, rules) = match self.kind {
            Kind::Slither => (
                "Slitherlink",
                [
                    "Make one closed loop with no branches or crossings.",
                    "A number tells how many of its four edges are in the loop.",
                    "Tap between two dots to cycle blank, line and ×.",
                ],
            ),
            Kind::Hashi => (
                "Hashi",
                [
                    "Join islands with one or two straight bridges.",
                    "Each island needs exactly its printed number of bridges.",
                    "Bridges cannot cross; every island must connect.",
                ],
            ),
            Kind::Kakuro => (
                "Kakuro",
                [
                    "Add each run to its sum: across at top right, down at bottom left.",
                    "Use 1–9; a digit cannot repeat within one run.",
                    "Tap to cycle digits; the 1 is fixed.",
                ],
            ),
            Kind::Mines => (
                "Minesweeper",
                [
                    "Reveal every safe square without opening a mine.",
                    "A number counts mines in the eight touching squares.",
                    "Switch to Flag, then mark squares that may hold mines.",
                ],
            ),
            Kind::Home => (
                "Logic Pack",
                [
                    "Choose one of four compact pencil puzzles.",
                    "Each puzzle has its own short rules screen.",
                    "Check never reveals the answer; it only judges your marks.",
                ],
            ),
        };
        ScreenBuilder::new("logicpack-help")
            .top_bar("How to play")
            .owns_back(true)
            .heading(title)
            .text(rules[0])
            .text(rules[1])
            .text(rules[2])
            .bottom_action("close-help", "Play")
            .build()
    }

    fn controls(&self, screen: ScreenBuilder) -> Screen {
        let mut b = screen.secondary(match self.draft.status() {
            Status::Saved => "Saved",
            Status::Saving | Status::Unsaved => "Saving…",
            Status::Failed(_) => "Not saved",
        });
        if self.outcome != Outcome::Playing {
            b = b.grid(2, false, [("back", "Puzzles")]);
        } else if self.kind == Kind::Mines {
            b = b.grid(
                2,
                false,
                [
                    (
                        "mine-mode",
                        if self.flagging {
                            "Mode: Flag"
                        } else {
                            "Mode: Reveal"
                        },
                    ),
                    ("check", "Check"),
                ],
            );
        } else {
            b = b.grid(2, false, [("check", "Check"), ("back", "Puzzles")]);
        }
        let mut actions = vec![("restart", "Restart")];
        if !self.undo.is_empty() {
            actions.insert(0, ("undo", "Undo"));
        }
        b.grid(2, false, actions).build()
    }

    fn pencil_screen(&self, title: &str, board: kobo_sdk::PencilBoard) -> Screen {
        self.controls(
            ScreenBuilder::new("logicpack-pencil")
                .top_bar(title)
                .owns_back(true)
                .top_bar_action("how-to-play", "Help")
                .secondary(&self.notice)
                .pencil_board(board),
        )
    }
    fn slither_screen(&self) -> Screen {
        self.pencil_screen("Slitherlink", boards::slither(&self.cells))
    }
    fn hashi_screen(&self) -> Screen {
        self.pencil_screen("Hashi", boards::hashi(&self.cells))
    }
    fn kakuro_screen(&self) -> Screen {
        self.pencil_screen("Kakuro", boards::kakuro(&self.cells))
    }

    fn mines_screen(&self) -> Screen {
        let cells = (0..16).map(|cell| {
            let label = if self.outcome == Outcome::Solved && has_mine(self.mines, cell) {
                "⚑".to_owned()
            } else {
                match self.cells[cell] {
                    1 => digit_label(adjacent_mines(self.mines, cell)),
                    2 => "⚑".to_owned(),
                    3 => "✹".to_owned(),
                    _ => "?".to_owned(),
                }
            };
            (format!("mine-{cell}"), label, None)
        });
        self.controls(
            ScreenBuilder::new("logicpack-mines")
                .top_bar("Minesweeper")
                .owns_back(true)
                .top_bar_action("how-to-play", "Help")
                .secondary(&self.notice)
                .board(4, cells),
        )
    }

    fn tap(&mut self, action: ActionId) -> bool {
        match self.kind {
            Kind::Slither => (0..12)
                .find(|edge| action == action_id(&format!("edge-{edge}")))
                .is_some_and(|edge| {
                    self.cells[edge] = (self.cells[edge] + 1) % 3;
                    true
                }),
            Kind::Hashi => (0..4)
                .find(|route| action == action_id(&format!("route-{route}")))
                .is_some_and(|route| {
                    self.cells[route] = (self.cells[route] + 1) % 3;
                    true
                }),
            Kind::Kakuro => (0..3)
                .find(|cell| action == action_id(&format!("kakuro-{cell}")))
                .is_some_and(|cell| {
                    self.cells[cell] = self.cells[cell] % 9 + 1;
                    true
                }),
            Kind::Mines => (0..16)
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
                let replacement = (0..16)
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
        self.outcome = if (0..16).all(|at| has_mine(self.mines, at) || self.cells[at] == 1) {
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
        if adjacent_mines(self.mines, cell) != 0 {
            return;
        }
        for neighbour in neighbours(cell) {
            self.reveal(neighbour);
        }
    }

    fn check(&mut self) {
        if self.kind == Kind::Mines && self.outcome != Outcome::Playing {
            return;
        }
        self.outcome = if self.is_complete() {
            Outcome::Solved
        } else {
            Outcome::Playing
        };
        self.notice = if self.outcome == Outcome::Solved {
            "Solved.".into()
        } else {
            "Not solved yet. Recheck your marks.".into()
        };
    }

    fn is_complete(&self) -> bool {
        match self.kind {
            Kind::Slither => {
                let mask = self.cells[..12]
                    .iter()
                    .enumerate()
                    .fold(0_u16, |mask, (edge, state)| {
                        mask | if *state == 1 { 1 << edge } else { 0 }
                    });
                mask == SLITHER_TARGET
            }
            Kind::Hashi => self.cells[..4].iter().all(|bridges| *bridges == 1),
            Kind::Kakuro => self.cells[..3] == KAKURO_SOLUTION,
            Kind::Mines => (0..16).all(|cell| has_mine(self.mines, cell) || self.cells[cell] == 1),
            Kind::Home => false,
        }
    }

    fn encode(&self) -> Vec<u8> {
        format!(
            "{}|{}|{}|{}|{}|{}|{}",
            self.kind.stored(),
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
        let Some(kind) = fields[0].parse().ok().and_then(Kind::read) else {
            return false;
        };
        let cells = fields[1]
            .split(',')
            .map(str::parse::<u8>)
            .collect::<Result<Vec<_>, _>>()
            .ok();
        let Some(cells): Option<[u8; 16]> = cells.and_then(|cells| cells.try_into().ok()) else {
            return false;
        };
        if cells.iter().any(|cell| *cell > 9) {
            return false;
        }
        let Some(mines) = fields[2]
            .parse::<u16>()
            .ok()
            .filter(|mines| mines.count_ones() == 3)
        else {
            return false;
        };
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
        let used = match kind {
            Kind::Home => 0,
            Kind::Slither => 12,
            Kind::Hashi => 4,
            Kind::Kakuro => 3,
            Kind::Mines => 16,
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
        let candidate = Self {
            kind,
            cells,
            mines,
            ..Self::default()
        };
        if outcome == Outcome::Solved && !candidate.is_complete() {
            return false;
        }
        self.kind = kind;
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
        " ".into()
    } else {
        value.to_string()
    }
}

const fn has_mine(mines: u16, cell: usize) -> bool {
    mines & (1 << cell) != 0
}

fn neighbours(cell: usize) -> impl Iterator<Item = usize> {
    let row = cell / 4;
    let column = cell % 4;
    let mut adjacent = Vec::new();
    for next_row in row.saturating_sub(1)..=(row + 1).min(3) {
        for next_column in column.saturating_sub(1)..=(column + 1).min(3) {
            let next = next_row * 4 + next_column;
            if next != cell {
                adjacent.push(next);
            }
        }
    }
    adjacent.into_iter()
}

fn adjacent_mines(mines: u16, cell: usize) -> u8 {
    u8::try_from(neighbours(cell).filter(|at| has_mine(mines, *at)).count())
        .expect("at most eight neighbours")
}

impl KoboApp for Game {
    fn on_start(&mut self, context: &mut Context) {
        context.set_orientation(kobo_sdk::Orientation::Portrait);
        context.store().load(STATE);
        context.set_screen(self.screen());
    }

    fn on_load(&mut self, context: &mut Context, key: &str, result: StoreResult) {
        if key != STATE || self.loaded {
            return;
        }
        match result {
            StoreResult::Loaded {
                value: Some(bytes), ..
            } => match saved::decode(&bytes) {
                Some((games, kind)) => {
                    self.games = games;
                    self.fresh(Kind::Home);
                    self.select(kind);
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
        let before = self.encode();
        let previous_kind = self.kind;
        let mut changed = true;
        let mut save = false;
        if self.view == View::Restart {
            if action == action_id("confirm-restart") {
                let history = std::mem::take(&mut self.undo);
                self.fresh(self.kind);
                self.undo = history;
                self.remember(before);
                self.save(context);
            } else if action != action_id("cancel-restart") && action != ActionId::BACK {
                return;
            }
            self.view = View::Puzzle;
            context.set_screen(self.screen());
            return;
        }
        if action == action_id("undo") && self.kind != Kind::Home {
            if let Some(position) = self.undo.pop() {
                assert!(self.restore(&position));
                self.save(context);
            }
            context.set_screen(self.screen());
            return;
        }
        if action == action_id("restart") && self.kind != Kind::Home {
            self.view = View::Restart;
            context.set_screen(self.screen());
            return;
        }
        if self.view == View::Help {
            if action == action_id("close-help") || action == ActionId::BACK {
                self.view = View::Puzzle;
            } else {
                return;
            }
        } else {
            match action {
                action if action == action_id("slither") => self.select(Kind::Slither),
                action if action == action_id("hashi") => self.select(Kind::Hashi),
                action if action == action_id("kakuro") => self.select(Kind::Kakuro),
                action if action == action_id("mines") => self.select(Kind::Mines),
                action if action == action_id("how-to-play") => self.view = View::Help,
                action if action == action_id("back") || action == ActionId::BACK => {
                    self.retain();
                    self.kind = Kind::Home;
                    self.notice = "Choose a puzzle.".into();
                }
                action if action == action_id("check") => {
                    self.check();
                    save = true;
                }
                action if action == action_id("mine-mode") && self.kind == Kind::Mines => {
                    self.flagging = !self.flagging;
                    self.notice = if self.flagging {
                        "Flag mode. Tap a hidden square to mark it."
                    } else {
                        "Reveal mode. Tap a hidden square to open it."
                    }
                    .into();
                    save = true;
                }
                _ => {
                    changed = self.tap(action);
                    if changed && self.kind != Kind::Mines {
                        self.outcome = Outcome::Playing;
                        self.notice = "Keep going. Check when you are ready.".into();
                    }
                    save = changed;
                }
            }
        }
        if save {
            self.remember(before);
            self.save(context);
        }
        if !save && self.kind != previous_kind {
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

#[cfg(test)]
mod tests {
    use super::*;
    use kobo_ui::{Chrome, CLARA_BW_METRICS};

    #[test]
    fn all_four_puzzles_have_real_winning_positions() {
        let mut game = Game {
            loaded: true,
            ..Game::default()
        };
        game.select(Kind::Slither);
        for edge in 0..12 {
            game.cells[edge] = u8::from(SLITHER_TARGET & (1 << edge) != 0);
        }
        game.check();
        assert_eq!(game.outcome, Outcome::Solved);

        game.select(Kind::Hashi);
        game.cells[..4].fill(1);
        game.check();
        assert_eq!(game.outcome, Outcome::Solved);

        game.select(Kind::Kakuro);
        game.cells[..3].copy_from_slice(&KAKURO_SOLUTION);
        game.check();
        assert_eq!(game.outcome, Outcome::Solved);
    }

    #[test]
    fn minesweeper_reseats_the_first_mine_and_can_lose_later() {
        let mut game = Game {
            loaded: true,
            ..Game::default()
        };
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
        let mut game = Game {
            loaded: true,
            ..Game::default()
        };
        game.select(Kind::Hashi);
        game.cells[1] = 2;
        let mut restored = Game::default();
        assert!(restored.restore(&game.encode()));
        assert_eq!(restored.kind, Kind::Hashi);
        assert_eq!(restored.cells[1], 2);
        assert!(!restored.restore(b"bad"));
    }

    #[test]
    fn every_screen_and_rules_page_fits_clara() {
        for kind in [
            Kind::Home,
            Kind::Slither,
            Kind::Hashi,
            Kind::Kakuro,
            Kind::Mines,
        ] {
            let mut game = Game {
                loaded: true,
                ..Game::default()
            };
            game.select(kind);
            assert!(game
                .screen()
                .diagnostics(&CLARA_BW_METRICS, &Chrome::default())
                .issues
                .is_empty());
            assert!(game
                .screen()
                .layout_with(&CLARA_BW_METRICS, &Chrome::default())
                .rect_of_action(action_id("how-to-play"))
                .is_some());
            game.view = View::Help;
            assert!(game
                .screen()
                .diagnostics(&CLARA_BW_METRICS, &Chrome::measuring(true))
                .issues
                .is_empty());
        }
    }
}

#[cfg(test)]
mod regression_tests {
    use super::*;
    #[test]
    fn checking_a_lost_field_preserves_the_loss_after_restore() {
        let mut game = Game {
            loaded: true,
            ..Game::default()
        };
        game.select(Kind::Mines);
        assert!(game.tap_mine(0));
        assert!(game.tap_mine(5));
        assert_eq!(game.outcome, Outcome::Lost);
        game.check();
        assert_eq!(game.outcome, Outcome::Lost);
        let mut restored = Game::default();
        assert!(restored.restore(&game.encode()));
        restored.check();
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
        let screens = [
            Kind::Home,
            Kind::Slither,
            Kind::Hashi,
            Kind::Kakuro,
            Kind::Mines,
        ]
        .map(|kind| {
            let game = Game {
                kind,
                loaded: true,
                ..Game::default()
            };
            game.help_screen()
        });
        for screen in screens {
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
                    let chrome = kobo_ui::Chrome::measuring(true);
                    let diagnostics = screen.diagnostics(&metrics, &chrome);
                    assert!(
                        diagnostics.issues.is_empty(),
                        "{metrics:?}: {:?}",
                        diagnostics.issues
                    );
                    assert!(screen
                        .layout_with(&metrics, &chrome)
                        .rect_of_action(action_id("close-help"))
                        .is_some());
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
    fn act(g: &mut Game, name: &str) {
        g.on_action(&mut Context::default(), action_id(name));
    }
    #[test]
    fn switching_games_and_reopening_keeps_progress_and_undo() {
        let mut game = ready();
        act(&mut game, "hashi");
        act(&mut game, "route-0");
        act(&mut game, "back");
        act(&mut game, "kakuro");
        act(&mut game, "kakuro-1");
        let bytes = saved::encode(&game.games, game.kind);
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
        act(&mut reopened, "back");
        act(&mut reopened, "hashi");
        assert_eq!(reopened.cells[0], 1);
        act(&mut reopened, "undo");
        assert_eq!(reopened.cells[0], 0);
        act(&mut reopened, "back");
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
        act(&mut g, "restart");
        act(&mut g, "cancel-restart");
        assert_eq!(g.encode(), before);
        act(&mut g, "restart");
        act(&mut g, "confirm-restart");
        assert_eq!(g.cells, [0; 16]);
        act(&mut g, "undo");
        assert_eq!(g.encode(), before);
        act(&mut g, "undo");
        assert_eq!(g.cells[1], 0);
    }
    #[test]
    fn old_write_acknowledgement_cannot_mark_newer_moves_saved() {
        let mut g = ready();
        act(&mut g, "hashi");
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
        act(&mut g, "undo");
        assert_eq!(g.encode(), initial);
        act(&mut g, "mine-0");
        let safe = g.encode();
        act(&mut g, "mine-5");
        assert_eq!(g.outcome, Outcome::Lost);
        act(&mut g, "undo");
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
        let bytes = saved::encode(&g.games, g.kind);
        let future = String::from_utf8(bytes)
            .unwrap()
            .replace("\"version\":1", "\"version\":99");
        assert!(saved::decode(future.as_bytes()).is_none());
    }
    #[test]
    fn legacy_record_migrates_and_bounded_history_round_trips() {
        let mut g = ready();
        g.select(Kind::Kakuro);
        g.cells[0] = 3;
        let (games, kind) = saved::decode(&g.encode()).unwrap();
        assert_eq!(kind, Kind::Kakuro);
        assert_eq!(games[2].position, g.encode());
        for _ in 0..100 {
            act(&mut g, "kakuro-1");
        }
        assert_eq!(g.undo.len(), saved::HISTORY);
        let bytes = saved::encode(&g.games, g.kind);
        assert!(bytes.len() <= saved::LIMIT);
        assert_eq!(saved::decode(&bytes).unwrap().0, g.games);
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
                let _runner = kobo_sdk::AppRunner::with_metrics(Game::default(), metrics);
                for kind in [
                    Kind::Home,
                    Kind::Slither,
                    Kind::Hashi,
                    Kind::Kakuro,
                    Kind::Mines,
                ] {
                    let mut g = ready();
                    g.select(kind);
                    for view in [View::Puzzle, View::Help, View::Restart] {
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

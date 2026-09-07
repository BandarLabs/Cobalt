//! Four small, deterministic pencil puzzles that work fully offline.

use kobo_sdk::{action_id, ActionId, Context, KoboApp, Screen, ScreenBuilder, StoreResult};
use std::process::ExitCode;

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
    edited: bool,
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
            edited: false,
        }
    }
}

impl Game {
    fn select(&mut self, kind: Kind) {
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
        if self.view == View::Help {
            return self.help_screen();
        }
        match self.kind {
            Kind::Home => ScreenBuilder::new("logicpack")
                .top_bar("Logic Pack")
                .heading("Four pencil puzzles")
                .secondary(&self.notice)
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
                .button("how-to-play", "How to play")
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
                    "Tap an edge to cycle blank, line and ×.",
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
                    "Fill each run so its digits add to the arrow clue.",
                    "Use 1–9; a digit cannot repeat within one run.",
                    "Tap a white square to cycle its digit.",
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

    fn controls(screen: ScreenBuilder) -> Screen {
        screen
            .grid(
                3,
                false,
                [
                    ("check", "Check"),
                    ("how-to-play", "How to play"),
                    ("back", "Puzzles"),
                ],
            )
            .build()
    }

    fn slither_screen(&self) -> Screen {
        let cells = (0..25).map(|place| {
            let row = place / 5;
            let column = place % 5;
            if row % 2 == 0 && column % 2 == 1 {
                let edge = row / 2 * 2 + (column - 1) / 2;
                (
                    format!("edge-{edge}"),
                    match self.cells[edge] {
                        1 => "━━",
                        2 => "×",
                        _ => " ",
                    }
                    .to_owned(),
                    None,
                )
            } else if row % 2 == 1 && column % 2 == 0 {
                let edge = 6 + (row - 1) / 2 * 3 + column / 2;
                (
                    format!("edge-{edge}"),
                    match self.cells[edge] {
                        1 => "┃",
                        2 => "×",
                        _ => " ",
                    }
                    .to_owned(),
                    None,
                )
            } else if row % 2 == 1 {
                (format!("fixed-{place}"), "2".to_owned(), None)
            } else {
                (format!("fixed-{place}"), "·".to_owned(), None)
            }
        });
        Self::controls(
            ScreenBuilder::new("logicpack-slither")
                .top_bar("Slitherlink")
                .secondary(&self.notice)
                .board(5, cells),
        )
    }

    fn hashi_screen(&self) -> Screen {
        let cells = (0..25).map(|place| {
            let (name, label) = match place {
                2 | 10 | 14 | 22 => (format!("fixed-{place}"), "1".to_owned()),
                12 => (format!("fixed-{place}"), "4".to_owned()),
                7 => ("route-0".to_owned(), bridge_label(self.cells[0], true)),
                11 => ("route-1".to_owned(), bridge_label(self.cells[1], false)),
                13 => ("route-2".to_owned(), bridge_label(self.cells[2], false)),
                17 => ("route-3".to_owned(), bridge_label(self.cells[3], true)),
                _ => (format!("fixed-{place}"), " ".to_owned()),
            };
            (name, label, None)
        });
        Self::controls(
            ScreenBuilder::new("logicpack-hashi")
                .top_bar("Hashi")
                .secondary(&self.notice)
                .board(5, cells),
        )
    }

    fn kakuro_screen(&self) -> Screen {
        let labels = [
            "■".to_owned(),
            "↓ 3".to_owned(),
            "↓ 7".to_owned(),
            "→ 4".to_owned(),
            "1".to_owned(),
            digit_label(self.cells[0]),
            "→ 6".to_owned(),
            digit_label(self.cells[1]),
            digit_label(self.cells[2]),
        ];
        let cells = labels.into_iter().enumerate().map(|(place, label)| {
            let name = match place {
                5 => "kakuro-0".to_owned(),
                7 => "kakuro-1".to_owned(),
                8 => "kakuro-2".to_owned(),
                _ => format!("fixed-{place}"),
            };
            (name, label, None)
        });
        Self::controls(
            ScreenBuilder::new("logicpack-kakuro")
                .top_bar("Kakuro")
                .secondary(&self.notice)
                .board(3, cells),
        )
    }

    fn mines_screen(&self) -> Screen {
        let cells = (0..16).map(|cell| {
            let label = match self.cells[cell] {
                1 => adjacent_mines(self.mines, cell).to_string(),
                2 => "⚑".to_owned(),
                3 => "✹".to_owned(),
                _ => "?".to_owned(),
            };
            (format!("mine-{cell}"), label, None)
        });
        ScreenBuilder::new("logicpack-mines")
            .top_bar("Minesweeper")
            .secondary(&self.notice)
            .board(4, cells)
            .grid(
                4,
                false,
                [
                    ("mine-mode", if self.flagging { "Flag" } else { "Reveal" }),
                    ("check", "Check"),
                    ("how-to-play", "How to play"),
                    ("back", "Puzzles"),
                ],
            )
            .build()
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
            self.notice = "Mine opened. Tap Puzzles to try a fresh field.".into();
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
        self.outcome = if match self.kind {
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
        } {
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
        self.kind = kind;
        self.cells = cells;
        self.mines = mines;
        self.flagging = flags[0];
        self.first_reveal = flags[1];
        self.outcome = match (flags[2], flags[3]) {
            (false, false) => Outcome::Playing,
            (true, false) => Outcome::Lost,
            (false, true) => Outcome::Solved,
            (true, true) => return false,
        };
        self.notice = "Saved puzzle restored.".into();
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

fn bridge_label(value: u8, vertical: bool) -> String {
    match (value, vertical) {
        (1, true) => "│",
        (2, true) => "║",
        (1, false) => "—",
        (2, false) => "═",
        _ => " ",
    }
    .into()
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
        context.store().load(STATE);
        context.set_screen(self.screen());
    }

    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        if let StoreResult::Loaded { key, value } = result {
            if key == STATE && !self.edited {
                if let Some(bytes) = value {
                    if !self.restore(&bytes) {
                        self.notice = "Saved puzzle was damaged and was ignored.".into();
                    }
                }
                context.set_screen(self.screen());
            }
        }
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        let mut changed = true;
        let mut save = false;
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
                    save = changed;
                }
            }
        }
        if save {
            self.edited = true;
            context.store().save(STATE, self.encode());
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
        let mut game = Game::default();
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
        let mut game = Game::default();
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
        let mut game = Game::default();
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
            let mut game = Game::default();
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
        let mut game = Game::default();
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
        restored.select(Kind::Mines);
        assert!(restored.tap_mine(1));
    }
}

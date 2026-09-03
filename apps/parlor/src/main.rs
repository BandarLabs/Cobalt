//! Pass-and-play classics with original rule engines and bounded house AI.
#![allow(
    clippy::cast_possible_truncation,
    clippy::needless_pass_by_value,
    clippy::struct_excessive_bools,
    clippy::unused_self
)]

mod engine;
mod games;

use engine::{choose_move, Game, Strength};
use games::{Draughts, Kalah, Morris, MorrisMove, Reversi, ReversiMove, Rules};
use kobo_sdk::{action_id, ActionId, Context, Glyph, KoboApp, Screen, ScreenBuilder, StoreResult};
use std::process::ExitCode;

const SAVE: &str = "parlor-autosave-v1";
const GAMES: [Title; 4] = [Title::Reversi, Title::Draughts, Title::Morris, Title::Kalah];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Title {
    Reversi,
    Draughts,
    Morris,
    Kalah,
}

impl Title {
    const fn name(self) -> &'static str {
        match self {
            Self::Reversi => "Reversi",
            Self::Draughts => "Draughts",
            Self::Morris => "Nine Men's Morris",
            Self::Kalah => "Kalah 6,4",
        }
    }
    const fn key(self) -> &'static str {
        match self {
            Self::Reversi => "reversi",
            Self::Draughts => "draughts",
            Self::Morris => "morris",
            Self::Kalah => "kalah",
        }
    }
    const fn detail(self) -> &'static str {
        match self {
            Self::Reversi => "Bracket discs in eight directions.",
            Self::Draughts => "Mandatory captures and complete capture chains.",
            Self::Morris => "Place, form mills, move, fly, and remove.",
            Self::Kalah => "Sow four stones from each of six pits.",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Mode {
    PassAndPlay,
    Solo,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum View {
    Menu,
    Setup(Title),
    Board,
    Record,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Position {
    Reversi(Reversi),
    Draughts(Draughts),
    Morris(Morris),
    Kalah(Kalah),
}

impl Position {
    fn title(&self) -> Title {
        match self {
            Self::Reversi(_) => Title::Reversi,
            Self::Draughts(_) => Title::Draughts,
            Self::Morris(_) => Title::Morris,
            Self::Kalah(_) => Title::Kalah,
        }
    }
    fn turn(&self) -> i8 {
        match self {
            Self::Reversi(g) => g.turn,
            Self::Draughts(g) => g.turn,
            Self::Morris(g) => g.turn,
            Self::Kalah(g) => g.turn,
        }
    }
    fn terminal(&self) -> Option<i32> {
        match self {
            Self::Reversi(g) => g.terminal_score(1),
            Self::Draughts(g) => g.terminal_score(1),
            Self::Morris(g) => g.terminal_score(1),
            Self::Kalah(g) => g.terminal_score(1),
        }
    }

    fn is_valid(&self) -> bool {
        match self {
            Self::Reversi(game) => {
                valid_side(game.turn) && game.board.iter().all(|piece| (-1..=1).contains(piece))
            }
            Self::Draughts(game) => {
                let expected = game.side() * game.side();
                valid_side(game.turn)
                    && game.board.len() == expected
                    && game.board.iter().all(|piece| (-2..=2).contains(piece))
                    && game
                        .forced
                        .is_none_or(|at| at < expected && game.board[at].signum() == game.turn)
                    && game
                        .captured
                        .iter()
                        .all(|&at| at < expected && game.board[at].signum() == -game.turn)
                    && (game.rules == Rules::International
                        || (game.forced.is_none() && game.captured.is_empty()))
                    && (game.forced.is_some() != game.captured.is_empty()
                        || game.rules == Rules::AngloAmerican)
                    && {
                        let mut captured = game.captured.clone();
                        captured.sort_unstable();
                        captured.dedup();
                        captured.len() == game.captured.len()
                    }
            }
            Self::Morris(game) => {
                valid_side(game.turn)
                    && game.placed.iter().all(|&placed| placed <= 9)
                    && game.board.iter().all(|piece| (-1..=1).contains(piece))
                    && game.count(1) <= usize::from(game.placed[0])
                    && game.count(-1) <= usize::from(game.placed[1])
            }
            Self::Kalah(game) => {
                valid_side(game.turn)
                    && game.pits.iter().all(|&pit| pit <= 48)
                    && game.pits.iter().map(|&pit| u16::from(pit)).sum::<u16>() == 48
            }
        }
    }

    fn selected_is_valid(&self, selected: Option<usize>) -> bool {
        selected.is_none_or(|at| match self {
            Self::Reversi(_) | Self::Kalah(_) => false,
            Self::Draughts(game) => {
                at < game.board.len()
                    && game.board[at].signum() == game.turn
                    && game.forced.is_none_or(|forced| forced == at)
            }
            Self::Morris(game) => at < 24 && game.board[at] == game.turn,
        })
    }
}

struct Parlor {
    view: View,
    position: Option<Position>,
    mode: Mode,
    rules: Rules,
    strength: Strength,
    human_side: i8,
    rotate: bool,
    selected: Option<usize>,
    notice: String,
    history: Vec<Position>,
    record: Vec<String>,
    undo_pending: bool,
    match_score: [u8; 2],
    scored: bool,
    loaded: bool,
}

impl Default for Parlor {
    fn default() -> Self {
        Self {
            view: View::Menu,
            position: None,
            mode: Mode::PassAndPlay,
            rules: Rules::AngloAmerican,
            strength: Strength::Club,
            human_side: 1,
            rotate: true,
            selected: None,
            notice: "Choose a table game. Pass-and-play is ready.".into(),
            history: Vec::new(),
            record: Vec::new(),
            undo_pending: false,
            match_score: [0, 0],
            scored: false,
            loaded: false,
        }
    }
}

fn player(side: i8) -> &'static str {
    if side == 1 {
        "Black / South"
    } else {
        "White / North"
    }
}

const fn valid_side(side: i8) -> bool {
    matches!(side, -1 | 1)
}

fn cell_name(at: usize) -> String {
    format!("cell-{at}")
}

impl Parlor {
    fn screen(&self) -> Screen {
        let screen = match self.view {
            View::Menu => self.menu_screen(),
            View::Setup(title) => self.setup_screen(title),
            View::Board => self.board_screen(),
            View::Record => self.record_screen(),
        };
        screen.with_own_back(self.view != View::Menu)
    }

    fn menu_screen(&self) -> Screen {
        let mut screen = ScreenBuilder::new("parlor-menu")
            .top_bar("Parlor")
            .heading("Pass-and-play classics")
            .secondary("Set the reader between two players, choose a game, and begin.")
            .rows(GAMES.map(|title| {
                (
                    title.key(),
                    title.name(),
                    title.detail(),
                    if title == Title::Draughts {
                        Glyph::Grid
                    } else {
                        Glyph::Circle
                    },
                )
            }));
        if self.position.is_some() {
            screen = screen.primary_button("resume", "Resume saved game");
        }
        screen.build()
    }

    fn setup_screen(&self, title: Title) -> Screen {
        let mode = match self.mode {
            Mode::PassAndPlay => "Pass-and-play",
            Mode::Solo => "Solo against the house",
        };
        let mut screen = ScreenBuilder::new("parlor-setup")
            .top_bar(title.name())
            .heading("Set the table")
            .rows([
                ("mode", "Mode", mode, Glyph::Person),
                (
                    "rotation",
                    "Board rotation",
                    if self.rotate {
                        "Faces the active player"
                    } else {
                        "Fixed for side-by-side play"
                    },
                    Glyph::Refresh,
                ),
            ]);
        if title == Title::Draughts {
            screen = screen.rows([("rules", "Rules", self.rules.name(), Glyph::Grid)]);
        }
        if self.mode == Mode::Solo {
            screen = screen.rows([
                (
                    "side",
                    "Your side",
                    if self.human_side == 1 {
                        "Black / South (first)"
                    } else {
                        "White / North (second)"
                    },
                    Glyph::Person,
                ),
                (
                    "strength",
                    "House strength",
                    self.strength.name(),
                    Glyph::Chart,
                ),
            ]);
        }
        if title == Title::Draughts && self.rules == Rules::International {
            screen = screen.error_state(
                "International draughts needs a 10×10 board. This reader's board surface is limited to 81 touch cells; choose Anglo-American 8×8.",
            );
            return screen.build();
        }
        screen
            .primary_button("start", "Start game")
            .button("back", "Games")
            .build()
    }

    fn board_screen(&self) -> Screen {
        let Some(position) = &self.position else {
            return self.menu_screen();
        };
        if matches!(
            position,
            Position::Draughts(Draughts {
                rules: Rules::International,
                ..
            })
        ) {
            return self.setup_screen(Title::Draughts);
        }
        let title = position.title();
        let rotated = self.mode == Mode::PassAndPlay && self.rotate && position.turn() == -1;
        let status = if let Some(score) = position.terminal() {
            match score.cmp(&0) {
                std::cmp::Ordering::Greater => "Black / South wins".to_owned(),
                std::cmp::Ordering::Less => "White / North wins".to_owned(),
                std::cmp::Ordering::Equal => "Draw".to_owned(),
            }
        } else if rotated {
            format!("{} · rotated", player(position.turn()))
        } else {
            format!("{} to move", player(position.turn()))
        };
        let base = ScreenBuilder::new("parlor-board")
            .top_bar(format!("{} — {status}", title.name()))
            .secondary(format!(
                "Best of 3 · Black/South {}–{} White/North · {}",
                self.match_score[0], self.match_score[1], self.notice
            ));
        let base = match position {
            Position::Reversi(game) => self.reversi_board(base, game, rotated),
            Position::Draughts(game) => self.draughts_board(base, game, rotated),
            Position::Morris(game) => self.morris_board(base, game, rotated),
            Position::Kalah(game) => self.kalah_board(base, game, rotated),
        };
        base.grid(
            3,
            false,
            [
                (
                    "undo",
                    if self.undo_pending {
                        "Agree undo"
                    } else {
                        "Undo"
                    },
                ),
                ("record", "Moves"),
                (
                    "new",
                    if position.terminal().is_some() {
                        "Next round"
                    } else {
                        "New game"
                    },
                ),
            ],
        )
        .button("back", "Games")
        .build()
    }

    fn reversi_board(&self, base: ScreenBuilder, game: &Reversi, rotated: bool) -> ScreenBuilder {
        let legal = game.placements();
        base.board(
            8,
            ordered(64, rotated).map(|at| {
                let (label, glyph) = match game.board[at] {
                    1 => ("", Some(Glyph::BlackDisc)),
                    -1 => ("", Some(Glyph::WhiteDisc)),
                    _ if legal.contains(&at) => ("◌", None),
                    _ => (" ", None),
                };
                (cell_name(at), label, glyph)
            }),
        )
    }

    fn draughts_board(&self, base: ScreenBuilder, game: &Draughts, rotated: bool) -> ScreenBuilder {
        let moves = game.moves();
        base.board(
            game.side() as u8,
            ordered(game.board.len(), rotated).map(|at| {
                let marked = self
                    .selected
                    .is_some_and(|from| moves.iter().any(|m| m.from == from && m.to == at));
                let (label, glyph) = if marked {
                    ("◌", None)
                } else if self.selected == Some(at) {
                    ("□", None)
                } else {
                    match game.board[at] {
                        1 => ("", Some(Glyph::BlackDraughtsMan)),
                        2 => ("", Some(Glyph::BlackDraughtsKing)),
                        -1 => ("", Some(Glyph::WhiteDraughtsMan)),
                        -2 => ("", Some(Glyph::WhiteDraughtsKing)),
                        _ => (" ", None),
                    }
                };
                (cell_name(at), label, glyph)
            }),
        )
    }

    fn morris_board(&self, base: ScreenBuilder, game: &Morris, rotated: bool) -> ScreenBuilder {
        let moves = game.moves();
        base.board(
            7,
            ordered(49, rotated).map(|grid| {
                let point = MORRIS_GRID.iter().position(|&g| g == grid);
                let (label, glyph) = point.map_or((" ", None), |at| {
                    let marked = moves.iter().any(|mv| match *mv {
                        MorrisMove::Place(to) | MorrisMove::Remove(to) => to == at,
                        MorrisMove::Slide(from, to) => self.selected == Some(from) && to == at,
                    });
                    if marked && game.board[at] == 0 {
                        ("Legal point", Some(Glyph::MorrisLegalPoint))
                    } else {
                        match game.board[at] {
                            1 => ("", Some(Glyph::BlackDisc)),
                            -1 => ("", Some(Glyph::WhiteDisc)),
                            _ => ("Open point", Some(Glyph::MorrisPoint)),
                        }
                    }
                });
                (cell_name(grid), label, glyph)
            }),
        )
    }

    fn kalah_board(&self, base: ScreenBuilder, game: &Kalah, rotated: bool) -> ScreenBuilder {
        base.secondary(format!(
            "Stores: South {} · North {}",
            game.pits[6], game.pits[13]
        ))
        .grid(
            7,
            false,
            ordered(14, rotated).map(|display| {
                let pit = KALAH_DISPLAY[display];
                let label = if pit == 6 {
                    format!("S {}", game.pits[pit])
                } else if pit == 13 {
                    format!("N {}", game.pits[pit])
                } else {
                    game.pits[pit].to_string()
                };
                (cell_name(pit), label)
            }),
        )
    }

    fn record_screen(&self) -> Screen {
        let text = if self.record.is_empty() {
            "No moves yet.".to_owned()
        } else {
            self.record
                .iter()
                .enumerate()
                .map(|(i, mv)| format!("{}. {mv}", i + 1))
                .collect::<Vec<_>>()
                .join("\n")
        };
        ScreenBuilder::new("parlor-record")
            .top_bar("Game record")
            .text(text)
            .button("board", "Board")
            .build()
    }

    fn start(&mut self, title: Title) {
        if title == Title::Draughts && self.rules == Rules::International {
            self.view = View::Setup(title);
            self.notice = "International 10×10 is unavailable on this reader.".into();
            return;
        }
        self.match_score = [0, 0];
        self.start_round(title);
    }

    fn start_round(&mut self, title: Title) {
        self.position = Some(match title {
            Title::Reversi => Position::Reversi(Reversi::default()),
            Title::Draughts => Position::Draughts(Draughts::new(self.rules)),
            Title::Morris => Position::Morris(Morris::default()),
            Title::Kalah => Position::Kalah(Kalah::default()),
        });
        self.view = View::Board;
        self.selected = None;
        self.history.clear();
        self.record.clear();
        self.undo_pending = false;
        self.scored = false;
        self.notice = match title {
            Title::Reversi => "Tap a hollow destination.".into(),
            Title::Draughts => format!(
                "{}. Tap a piece, then a hollow destination. Captures are mandatory.",
                self.rules.name()
            ),
            Title::Morris => {
                "Placement: tap a hollow point. Completing a mill earns one removal.".into()
            }
            Title::Kalah => "Tap one of your six non-empty pits to sow counter-clockwise.".into(),
        };
        self.run_ai_if_needed();
        self.update_outcome();
    }

    fn push_history(&mut self, description: String) {
        if let Some(position) = &self.position {
            self.history.push(position.clone());
        }
        self.record.push(description);
        self.undo_pending = false;
    }

    fn tap_cell(&mut self, at: usize) {
        let Some(position) = self.position.clone() else {
            return;
        };
        if position.terminal().is_some() {
            self.notice = "The game is over. Review the moves or start a new game.".into();
            return;
        }
        if self.mode == Mode::Solo && position.turn() != self.human_side {
            self.notice = "The house is choosing a move.".into();
            return;
        }
        match position {
            Position::Reversi(game) => self.tap_reversi(game, at),
            Position::Draughts(game) => self.tap_draughts(game, at),
            Position::Morris(game) => {
                if let Some(point) = MORRIS_GRID.iter().position(|&grid| grid == at) {
                    self.tap_morris(game, point);
                } else {
                    self.notice = "Tap one of the 24 marked intersections.".into();
                }
            }
            Position::Kalah(game) => self.tap_kalah(game, at),
        }
        self.run_ai_if_needed();
        self.update_outcome();
    }

    fn update_outcome(&mut self) {
        if self.scored {
            return;
        }
        let Some(score) = self.position.as_ref().and_then(Position::terminal) else {
            return;
        };
        if score > 0 {
            self.match_score[0] += 1;
        } else if score < 0 {
            self.match_score[1] += 1;
        }
        self.scored = true;
        if self.match_score.contains(&2) {
            self.notice = format!(
                "Match won, {}–{}. Start a new game to reset the card.",
                self.match_score[0], self.match_score[1]
            );
        }
    }

    fn tap_reversi(&mut self, game: Reversi, at: usize) {
        let mv = ReversiMove::Place(at);
        if !game.legal_moves().contains(&mv) {
            self.notice =
                "That square brackets no opposing discs. Tap a hollow destination.".into();
            return;
        }
        self.push_history(format!("{} to {}", player(game.turn), square_name(at, 8)));
        let mut next = game.apply(&mv);
        if matches!(next.legal_moves().as_slice(), [ReversiMove::Pass])
            && next.terminal_score(1).is_none()
        {
            let passed = next.turn;
            next = next.apply(&ReversiMove::Pass);
            self.notice = format!("{} has no legal move; the turn passes.", player(passed));
        } else {
            self.notice = format!("Pass the reader to {}.", player(next.turn));
        }
        self.position = Some(Position::Reversi(next));
    }

    fn tap_draughts(&mut self, game: Draughts, at: usize) {
        let moves = game.moves();
        if let Some(from) = self.selected {
            if let Some(mv) = moves.iter().find(|mv| mv.from == from && mv.to == at) {
                self.push_history(format!(
                    "{} {}–{}{}",
                    player(game.turn),
                    square_name(from, game.side()),
                    square_name(at, game.side()),
                    if mv.captured.is_some() {
                        " capture"
                    } else {
                        ""
                    }
                ));
                let next = game.apply_move(mv);
                self.selected = next.forced;
                self.notice = if next.forced.is_some() {
                    "Capture chain: the same piece must capture again.".into()
                } else {
                    format!("Pass the reader to {}.", player(next.turn))
                };
                self.position = Some(Position::Draughts(next));
                return;
            }
        }
        if moves.iter().any(|mv| mv.from == at) {
            self.selected = Some(at);
            self.notice = "Piece selected. Tap a hollow destination.".into();
        } else if moves.iter().any(|mv| mv.captured.is_some()) {
            self.notice = "Captures are mandatory. Select a piece that can capture.".into();
        } else {
            self.notice = "That piece has no legal move.".into();
        }
    }

    fn tap_morris(&mut self, game: Morris, at: usize) {
        let moves = game.moves();
        if game.removing {
            let mv = MorrisMove::Remove(at);
            if moves.contains(&mv) {
                self.push_history(format!("{} removes point {}", player(game.turn), at + 1));
                let next = game.apply_move(&mv);
                self.position = Some(Position::Morris(next.clone()));
                self.selected = None;
                self.notice = format!("Piece removed. Pass the reader to {}.", player(next.turn));
            } else {
                self.notice =
                    "Remove an opposing piece outside a mill, unless every piece is in a mill."
                        .into();
            }
            return;
        }
        if game.placed[usize::from(game.turn != 1)] < 9 {
            let mv = MorrisMove::Place(at);
            if moves.contains(&mv) {
                self.push_history(format!("{} places at point {}", player(game.turn), at + 1));
                let next = game.apply_move(&mv);
                self.notice = if next.removing {
                    "Mill formed. Remove one eligible opposing piece.".into()
                } else {
                    format!("Pass the reader to {}.", player(next.turn))
                };
                self.position = Some(Position::Morris(next));
            } else {
                self.notice = "Place on an empty marked intersection.".into();
            }
            return;
        }
        if let Some(from) = self.selected {
            let mv = MorrisMove::Slide(from, at);
            if moves.contains(&mv) {
                self.push_history(format!(
                    "{} moves point {}–{}",
                    player(game.turn),
                    from + 1,
                    at + 1
                ));
                let next = game.apply_move(&mv);
                self.notice = if next.removing {
                    "Mill formed. Remove one eligible opposing piece.".into()
                } else {
                    format!("Pass the reader to {}.", player(next.turn))
                };
                self.position = Some(Position::Morris(next));
                self.selected = None;
                return;
            }
        }
        if moves
            .iter()
            .any(|mv| matches!(mv, MorrisMove::Slide(from, _) if *from == at))
        {
            self.selected = Some(at);
            self.notice = if game.count(game.turn) == 3 {
                "Flying: choose any empty point."
            } else {
                "Piece selected. Choose an adjacent hollow point."
            }
            .into();
        } else {
            self.notice = "That piece cannot move.".into();
        }
    }

    fn tap_kalah(&mut self, game: Kalah, at: usize) {
        if !game.moves().contains(&at) {
            self.notice = "Choose a non-empty pit on your side. Stores cannot be selected.".into();
            return;
        }
        self.push_history(format!("{} sows pit {}", player(game.turn), at + 1));
        let next = game.apply_move(at);
        self.notice = if next.turn == game.turn && next.terminal_score(1).is_none() {
            "Last stone in your store: take the extra turn.".into()
        } else if next.terminal_score(1).is_some() {
            "One side is empty. Remaining stones moved to the stores.".into()
        } else {
            format!("Pass the reader to {}.", player(next.turn))
        };
        self.position = Some(Position::Kalah(next));
    }

    fn run_ai_if_needed(&mut self) {
        for _ in 0..32 {
            let Some(mut position) = self.position.clone() else {
                return;
            };
            let forced_pass = match &position {
                Position::Reversi(game)
                    if matches!(game.legal_moves().as_slice(), [ReversiMove::Pass])
                        && game.terminal_score(1).is_none() =>
                {
                    Some((game.turn, game.apply(&ReversiMove::Pass)))
                }
                _ => None,
            };
            if let Some((passed, next)) = forced_pass {
                self.history.push(position.clone());
                self.record.push(format!("{} passes", player(passed)));
                position = Position::Reversi(next);
                self.position = Some(position.clone());
                self.notice = format!("{} had no legal move; turn passes.", player(passed));
            }
            if self.mode != Mode::Solo
                || position.turn() == self.human_side
                || position.terminal().is_some()
            {
                return;
            }
            self.history.push(position.clone());
            let description = format!("House ({}) moves", self.strength.name());
            let next =
                match position {
                    Position::Reversi(game) => choose_move(&game, self.strength)
                        .map(|mv| Position::Reversi(game.apply(&mv))),
                    Position::Draughts(game) => choose_move(&game, self.strength)
                        .map(|mv| Position::Draughts(game.apply(&mv))),
                    Position::Morris(game) => choose_move(&game, self.strength)
                        .map(|mv| Position::Morris(game.apply(&mv))),
                    Position::Kalah(game) => {
                        choose_move(&game, self.strength).map(|mv| Position::Kalah(game.apply(&mv)))
                    }
                };
            let Some(next) = next else {
                return;
            };
            self.record.push(description);
            self.position = Some(next.clone());
            self.selected = match &next {
                Position::Draughts(g) => g.forced,
                _ => None,
            };
            self.notice = if next.turn() == self.human_side {
                format!("The house moved. {} to move.", player(self.human_side))
            } else {
                format!("The house continues ({})…", self.strength.name())
            };
        }
    }

    fn undo(&mut self) {
        if self.mode == Mode::PassAndPlay && !self.undo_pending {
            self.undo_pending = true;
            self.notice = "Undo requested. Opponent: tap Agree undo to confirm.".into();
            return;
        }
        if self.scored {
            if let Some(score) = self.position.as_ref().and_then(Position::terminal) {
                if score > 0 {
                    self.match_score[0] = self.match_score[0].saturating_sub(1);
                } else if score < 0 {
                    self.match_score[1] = self.match_score[1].saturating_sub(1);
                }
            }
            self.scored = false;
        }
        if let Some(previous) = self.history.pop() {
            self.position = Some(previous);
            self.record.pop();
            if self.mode == Mode::Solo {
                while self
                    .position
                    .as_ref()
                    .is_some_and(|position| position.turn() != self.human_side)
                {
                    let Some(previous) = self.history.pop() else {
                        break;
                    };
                    self.position = Some(previous);
                    self.record.pop();
                }
            }
            self.selected = None;
            self.notice = "Move undone.".into();
        } else {
            self.notice = "There is no move to undo.".into();
        }
        self.undo_pending = false;
    }

    fn save(&self, context: &mut Context) {
        if let Some(position) = &self.position {
            context
                .store()
                .save(SAVE, encode(self, position).into_bytes());
        }
    }

    fn handle_action(&mut self, action: ActionId) {
        if action == action_id("resume") {
            if matches!(
                self.position,
                Some(Position::Draughts(Draughts {
                    rules: Rules::International,
                    ..
                }))
            ) {
                self.view = View::Setup(Title::Draughts);
                self.notice = "International 10×10 is unavailable on this reader.".into();
            } else {
                self.view = View::Board;
                self.notice = "Game resumed.".into();
            }
        } else if let Some(title) = GAMES
            .iter()
            .find(|title| action == action_id(title.key()))
            .copied()
        {
            self.view = View::Setup(title);
        } else if action == action_id("mode") {
            self.mode = if self.mode == Mode::PassAndPlay {
                Mode::Solo
            } else {
                Mode::PassAndPlay
            };
        } else if action == action_id("rotation") {
            self.rotate = !self.rotate;
        } else if action == action_id("rules") {
            self.rules = if self.rules == Rules::AngloAmerican {
                Rules::International
            } else {
                Rules::AngloAmerican
            };
        } else if action == action_id("side") {
            self.human_side = -self.human_side;
        } else if action == action_id("strength") {
            self.strength = match self.strength {
                Strength::Casual => Strength::Club,
                Strength::Club => Strength::Strong,
                Strength::Strong => Strength::Casual,
            };
        } else if action == action_id("start") {
            if let View::Setup(title) = self.view {
                self.start(title);
            }
        } else if action == action_id("undo") {
            self.undo();
        } else if action == action_id("record") {
            self.view = View::Record;
        } else if action == action_id("board") {
            self.view = View::Board;
        } else if action == action_id("new") {
            if let Some(position) = &self.position {
                let title = position.title();
                if position.terminal().is_some() && !self.match_score.contains(&2) {
                    self.start_round(title);
                } else {
                    self.view = View::Setup(title);
                }
            }
        } else if action == action_id("back") || action == ActionId::BACK {
            self.view = View::Menu;
        } else if let Some(at) = (0..100).find(|at| action == action_id(&cell_name(*at))) {
            self.tap_cell(at);
        }
    }
}

impl KoboApp for Parlor {
    fn on_start(&mut self, context: &mut Context) {
        context.store().load(SAVE);
        context.set_screen(self.screen());
    }
    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        if let StoreResult::Loaded { value, .. } = result {
            if !self.loaded {
                if let Some(bytes) = value {
                    if let Ok(text) = String::from_utf8(bytes) {
                        if let Some(saved) = decode(&text) {
                            *self = saved;
                            self.notice = "Saved game ready. Choose Resume saved game.".into();
                            self.view = View::Menu;
                        }
                    }
                }
                self.loaded = true;
                context.set_screen(self.screen());
            }
        }
    }
    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        self.handle_action(action);
        self.save(context);
        context.set_screen(self.screen());
    }
}

fn ordered(length: usize, reverse: bool) -> impl Iterator<Item = usize> {
    let values: Vec<_> = if reverse {
        (0..length).rev().collect()
    } else {
        (0..length).collect()
    };
    values.into_iter()
}

fn square_name(at: usize, side: usize) -> String {
    format!("{}{}", (b'a' + (at % side) as u8) as char, side - at / side)
}

const MORRIS_GRID: [usize; 24] = [
    0, 3, 6, 8, 10, 12, 16, 17, 18, 21, 22, 23, 25, 26, 27, 30, 31, 32, 36, 38, 40, 42, 45, 48,
];
const KALAH_DISPLAY: [usize; 14] = [13, 12, 11, 10, 9, 8, 7, 0, 1, 2, 3, 4, 5, 6];

fn encode(app: &Parlor, position: &Position) -> String {
    let mode = u8::from(app.mode == Mode::Solo);
    let rules = u8::from(app.rules == Rules::International);
    let strength = match app.strength {
        Strength::Casual => 0,
        Strength::Club => 1,
        Strength::Strong => 2,
    };
    let header = format!(
        "{mode},{rules},{strength},{},{},{},{},{},{},{}",
        app.human_side,
        u8::from(app.rotate),
        position.turn(),
        app.match_score[0],
        app.match_score[1],
        u8::from(app.scored),
        app.selected.map_or_else(|| "-".into(), |at| at.to_string())
    );
    let body = match position {
        Position::Reversi(g) => format!(
            "R;{}",
            g.board
                .iter()
                .map(i8::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Position::Draughts(g) => format!(
            "D,{},{};{}",
            g.forced.map_or_else(|| "-".into(), |n| n.to_string()),
            encode_usizes(&g.captured),
            g.board
                .iter()
                .map(i8::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Position::Morris(g) => format!(
            "M,{},{},{};{}",
            g.placed[0],
            g.placed[1],
            u8::from(g.removing),
            g.board
                .iter()
                .map(i8::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Position::Kalah(g) => format!(
            "K;{}",
            g.pits
                .iter()
                .map(u8::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ),
    };
    let history = app
        .history
        .iter()
        .map(encode_snapshot)
        .collect::<Vec<_>>()
        .join("~");
    format!("{header};{body};{};{history}", app.record.join("|"))
}

struct SavedSettings {
    mode: Mode,
    rules: Rules,
    strength: Strength,
    human_side: i8,
    rotate: bool,
    turn: i8,
    match_score: [u8; 2],
    scored: bool,
    selected: Option<usize>,
}

fn decode_settings(fields: &[&str]) -> Option<SavedSettings> {
    if fields.len() != 10 {
        return None;
    }
    let mode = match *fields.first()? {
        "0" => Mode::PassAndPlay,
        "1" => Mode::Solo,
        _ => return None,
    };
    let rules = match *fields.get(1)? {
        "0" => Rules::AngloAmerican,
        "1" => Rules::International,
        _ => return None,
    };
    let strength = match *fields.get(2)? {
        "0" => Strength::Casual,
        "1" => Strength::Club,
        "2" => Strength::Strong,
        _ => return None,
    };
    let human_side = fields.get(3)?.parse().ok()?;
    let rotate = match *fields.get(4)? {
        "0" => false,
        "1" => true,
        _ => return None,
    };
    let turn = fields.get(5)?.parse().ok()?;
    let match_score = [fields.get(6)?.parse().ok()?, fields.get(7)?.parse().ok()?];
    let scored = match *fields.get(8)? {
        "0" => false,
        "1" => true,
        _ => return None,
    };
    Some(SavedSettings {
        mode,
        rules,
        strength,
        human_side,
        rotate,
        turn,
        match_score,
        scored,
        selected: parse_optional_usize(fields.get(9)?).ok()?,
    })
}

fn decode_position(kind: &[&str], board: &str, rules: Rules, turn: i8) -> Option<Position> {
    Some(match *kind.first()? {
        "R" if kind.len() == 1 => Position::Reversi(Reversi {
            board: parse_array(board)?,
            turn,
        }),
        "D" if kind.len() == 3 => Position::Draughts(Draughts {
            rules,
            board: parse_vec(board)?,
            turn,
            forced: parse_optional_usize(kind.get(1)?).ok()?,
            captured: parse_usizes(kind.get(2)?)?,
        }),
        "M" if kind.len() == 4 => Position::Morris(Morris {
            board: parse_array(board)?,
            turn,
            placed: [kind.get(1)?.parse().ok()?, kind.get(2)?.parse().ok()?],
            removing: match *kind.get(3)? {
                "0" => false,
                "1" => true,
                _ => return None,
            },
        }),
        "K" if kind.len() == 1 => Position::Kalah(Kalah {
            pits: parse_array(board)?,
            turn,
        }),
        _ => return None,
    })
}

fn decode(text: &str) -> Option<Parlor> {
    let mut parts = text.split(';');
    let settings = decode_settings(&parts.next()?.split(',').collect::<Vec<_>>())?;
    let kind = parts.next()?.split(',').collect::<Vec<_>>();
    let position = decode_position(&kind, parts.next()?, settings.rules, settings.turn)?;
    let record = match parts.next()? {
        "" => Vec::new(),
        moves => moves.split('|').map(str::to_owned).collect(),
    };
    let history = match parts.next()? {
        "" => Vec::new(),
        snapshots => snapshots
            .split('~')
            .map(decode_snapshot)
            .collect::<Option<Vec<_>>>()?,
    };
    if parts.next().is_some()
        || !valid_side(settings.human_side)
        || settings.match_score.iter().any(|&score| score > 2)
        || !position.is_valid()
        || !position.selected_is_valid(settings.selected)
        || record.len() != history.len()
        || history
            .iter()
            .any(|snapshot| snapshot.title() != position.title() || !snapshot.is_valid())
    {
        return None;
    }
    Some(Parlor {
        view: View::Menu,
        position: Some(position),
        mode: settings.mode,
        rules: settings.rules,
        strength: settings.strength,
        human_side: settings.human_side,
        rotate: settings.rotate,
        selected: settings.selected,
        notice: String::new(),
        history,
        record,
        undo_pending: false,
        match_score: settings.match_score,
        scored: settings.scored,
        loaded: true,
    })
}

fn parse_vec<T: std::str::FromStr>(text: &str) -> Option<Vec<T>> {
    text.split(',').map(|n| n.parse().ok()).collect()
}

fn parse_array<T: std::str::FromStr, const N: usize>(text: &str) -> Option<[T; N]> {
    parse_vec(text)?.try_into().ok()
}

fn parse_optional_usize(text: &str) -> Result<Option<usize>, std::num::ParseIntError> {
    if text == "-" {
        Ok(None)
    } else {
        text.parse().map(Some)
    }
}

fn encode_usizes(values: &[usize]) -> String {
    if values.is_empty() {
        "-".into()
    } else {
        values
            .iter()
            .map(usize::to_string)
            .collect::<Vec<_>>()
            .join(".")
    }
}

fn parse_usizes(text: &str) -> Option<Vec<usize>> {
    if text == "-" {
        Some(Vec::new())
    } else {
        text.split('.').map(|value| value.parse().ok()).collect()
    }
}

fn encode_snapshot(position: &Position) -> String {
    match position {
        Position::Reversi(game) => format!(
            "R:{}:{}",
            game.turn,
            game.board
                .iter()
                .map(i8::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Position::Draughts(game) => format!(
            "D:{}:{}:{}:{}:{}",
            u8::from(game.rules == Rules::International),
            game.turn,
            game.forced.map_or_else(|| "-".into(), |at| at.to_string()),
            encode_usizes(&game.captured),
            game.board
                .iter()
                .map(i8::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Position::Morris(game) => format!(
            "M:{}:{}:{}:{}:{}",
            game.turn,
            game.placed[0],
            game.placed[1],
            u8::from(game.removing),
            game.board
                .iter()
                .map(i8::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Position::Kalah(game) => format!(
            "K:{}:{}",
            game.turn,
            game.pits
                .iter()
                .map(u8::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

fn decode_snapshot(snapshot: &str) -> Option<Position> {
    let fields: Vec<_> = snapshot.split(':').collect();
    match *fields.first()? {
        "R" => Some(Position::Reversi(Reversi {
            turn: fields.get(1)?.parse().ok()?,
            board: parse_array(fields.get(2)?)?,
        })),
        "D" => Some(Position::Draughts(Draughts {
            rules: if fields.get(1)? == &"1" {
                Rules::International
            } else {
                Rules::AngloAmerican
            },
            turn: fields.get(2)?.parse().ok()?,
            forced: fields.get(3).and_then(|at| at.parse().ok()),
            captured: parse_usizes(fields.get(4)?)?,
            board: parse_vec(fields.get(5)?)?,
        })),
        "M" => Some(Position::Morris(Morris {
            turn: fields.get(1)?.parse().ok()?,
            placed: [fields.get(2)?.parse().ok()?, fields.get(3)?.parse().ok()?],
            removing: fields.get(4)? == &"1",
            board: parse_array(fields.get(5)?)?,
        })),
        "K" => Some(Position::Kalah(Kalah {
            turn: fields.get(1)?.parse().ok()?,
            pits: parse_array(fields.get(2)?)?,
        })),
        _ => None,
    }
}

fn main() -> ExitCode {
    kobo_sdk::run("parlor", Parlor::default()).map_or_else(
        |error| {
            eprintln!("parlor: {error}");
            ExitCode::FAILURE
        },
        |()| ExitCode::SUCCESS,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::perft;
    use crate::games::DraughtsMove;
    use kobo_ui::{Chrome, CLARA_BW_METRICS};

    #[test]
    fn reversi_opening_perft_matches_reference() {
        let game = Reversi::default();
        assert_eq!(perft(&game, 1), 4);
        assert_eq!(perft(&game, 2), 12);
        assert_eq!(perft(&game, 3), 56);
    }

    #[test]
    fn both_draughts_openings_and_mandatory_chain_are_pinned() {
        let anglo = Draughts::new(Rules::AngloAmerican);
        assert_eq!(perft(&anglo, 1), 7);
        assert_eq!(perft(&anglo, 2), 49);
        assert_eq!(perft(&anglo, 3), 302);
        let international = Draughts::new(Rules::International);
        assert_eq!(perft(&international, 1), 9);
        assert_eq!(perft(&international, 2), 81);
        assert_eq!(perft(&international, 3), 658);
        let mut game = Draughts::new(Rules::AngloAmerican);
        game.board.fill(0);
        game.board[42] = 1;
        game.board[33] = -1;
        game.board[17] = -1;
        let first = game.moves().into_iter().find(|m| m.from == 42).unwrap();
        let next = game.apply_move(&first);
        assert_eq!(next.turn, 1);
        assert_eq!(next.forced, Some(first.to));
        assert_eq!(next.board[33], 0);
        assert!(next.captured.is_empty());
        assert!(next.moves().iter().all(|m| m.from == first.to));
    }

    #[test]
    fn international_kings_fly_and_capture_over_distance() {
        let mut game = Draughts::new(Rules::International);
        game.board.fill(0);
        game.board[77] = 2;
        assert!(game.moves().iter().any(|m| m.to == 22));
        game.board[44] = -1;
        let captures = game.moves();
        assert!(captures.iter().all(|m| m.captured == Some(44)));
        assert!(captures.len() > 1);
    }

    #[test]
    fn draughts_capture_code_does_not_silently_hybridize_rules() {
        let mut anglo = Draughts::new(Rules::AngloAmerican);
        anglo.board.fill(0);
        anglo.board[17] = 1;
        anglo.board[26] = -1;
        assert!(anglo.moves().iter().all(|mv| mv.captured.is_none()));

        let mut international = Draughts::new(Rules::International);
        international.board.fill(0);
        international.board[77] = 2;
        international.board[66] = -1;
        international.board[22] = -1;
        let moves = international.moves();
        assert!(!moves.is_empty());
        assert!(moves.iter().all(|mv| international.capture_length(mv) == 2));
    }

    #[test]
    fn international_captured_piece_blocks_a_kings_return_path_until_turn_end() {
        let mut chain = Draughts::new(Rules::International);
        chain.board.fill(0);
        chain.board[77] = 2;
        chain.board[66] = -1;
        chain.board[46] = -1;
        let first = DraughtsMove {
            from: 77,
            to: 55,
            captured: Some(66),
        };
        assert!(chain.moves().contains(&first));
        let intermediate = chain.apply_move(&first);
        assert_eq!(intermediate.turn, 1);
        assert_eq!(intermediate.forced, Some(55));
        assert_eq!(intermediate.board[66], -1);
        assert_eq!(intermediate.captured, vec![66]);
        assert!(intermediate
            .moves()
            .iter()
            .all(|mv| mv.captured == Some(46)));
        let finished = intermediate.apply_move(&intermediate.moves()[0]);
        assert_eq!(finished.turn, -1);
        assert_eq!(finished.board[66], 0);
        assert_eq!(finished.board[46], 0);
        assert!(finished.captured.is_empty());

        let mut game = Draughts::new(Rules::International);
        game.board.fill(0);
        game.board[77] = 2;
        game.board[66] = -1;
        game.board[88] = -1;
        let first = DraughtsMove {
            from: 77,
            to: 55,
            captured: Some(66),
        };
        assert!(game.moves().contains(&first));
        let next = game.apply_move(&first);

        assert_eq!(next.turn, -1, "the blocker must end the capture sequence");
        assert_eq!(next.forced, None);
        assert!(next.captured.is_empty());
        assert_eq!(next.board[66], 0, "captured men leave after the sequence");
        assert_eq!(
            next.board[88], -1,
            "the king may not cross the captured man"
        );
    }

    #[test]
    fn morris_placement_mill_removal_and_flying_are_legal() {
        let game = Morris::default();
        assert_eq!(game.moves().len(), 24);
        let game = game.apply_move(&MorrisMove::Place(0));
        assert_eq!(game.moves().len(), 23);
        let mut mill = Morris::default();
        mill.board[0] = 1;
        mill.board[1] = 1;
        mill.placed = [2, 0];
        let mill = mill.apply_move(&MorrisMove::Place(2));
        assert!(mill.removing);
        let mut flying = Morris::default();
        flying.board[0] = 1;
        flying.board[1] = 1;
        flying.board[2] = 1;
        flying.board[21] = -1;
        flying.board[22] = -1;
        flying.board[23] = -1;
        flying.placed = [9, 9];
        assert!(flying.moves().contains(&MorrisMove::Slide(0, 20)));
    }

    #[test]
    fn kalah_sowing_capture_extra_turn_and_endgame_are_pinned() {
        let game = Kalah::default();
        assert_eq!(game.moves(), vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(game.apply_move(2).turn, 1);
        let mut capture = Kalah {
            pits: [0; 14],
            turn: 1,
        };
        capture.pits[0] = 1;
        capture.pits[7] = 1;
        capture.pits[11] = 3;
        capture.pits[5] = 1;
        let capture = capture.apply_move(0);
        assert_eq!(capture.pits[6], 4);
        let mut ending = Kalah {
            pits: [0; 14],
            turn: 1,
        };
        ending.pits[5] = 1;
        ending.pits[7] = 4;
        let ending = ending.apply_move(5);
        assert_eq!(ending.pits[13], 4);
        assert!(ending.pits[0..6].iter().all(|&n| n == 0));
        assert!(ending.pits[7..13].iter().all(|&n| n == 0));
    }

    #[test]
    fn autosave_round_trips_every_position_kind() {
        for title in GAMES {
            let mut app = Parlor::default();
            app.start(title);
            let text = encode(&app, app.position.as_ref().unwrap());
            let restored = decode(&text).unwrap();
            assert_eq!(restored.position, app.position);
        }
    }

    #[test]
    fn autosave_restores_history_needed_for_confirmed_undo() {
        let mut app = Parlor::default();
        app.start(Title::Reversi);
        let opening = app.position.clone();
        app.tap_reversi(Reversi::default(), 19);
        assert_eq!(app.history.len(), 1);

        let saved = encode(&app, app.position.as_ref().unwrap());
        let mut restored = decode(&saved).unwrap();
        assert_eq!(restored.history, app.history);
        restored.undo();
        assert!(restored.undo_pending);
        restored.undo();
        assert_eq!(restored.position, opening);
        assert!(restored.history.is_empty());
    }

    #[test]
    fn autosave_preserves_an_international_capture_in_progress() {
        let mut game = Draughts::new(Rules::International);
        game.board.fill(0);
        game.board[77] = 2;
        game.board[66] = -1;
        game.board[46] = -1;
        let game = game.apply_move(&DraughtsMove {
            from: 77,
            to: 55,
            captured: Some(66),
        });
        let app = Parlor {
            position: Some(Position::Draughts(game.clone())),
            selected: game.forced,
            rules: Rules::International,
            ..Parlor::default()
        };
        let saved = encode(&app, app.position.as_ref().unwrap());
        let restored = decode(&saved).unwrap();
        assert_eq!(restored.position, app.position);
        assert_eq!(restored.selected, Some(55));
    }

    #[test]
    fn malformed_autosaves_are_refused_before_they_can_reach_the_board() {
        let mut app = Parlor::default();
        app.start(Title::Reversi);
        let saved = encode(&app, app.position.as_ref().expect("position"));
        assert!(decode(&saved).is_some());

        let bad_turn = saved.replacen("0,0,1,1,1,1,0,0,0,-", "0,0,1,1,1,0,0,0,0,-", 1);
        assert!(decode(&bad_turn).is_none());
        assert!(decode("0,0,1,1,1,1,0,0,0,-;D,-,-;2;;").is_none());
        let bad_selected = saved.replacen("0,0,1,1,1,1,0,0,0,-", "0,0,1,1,1,1,0,0,0,99", 1);
        assert!(decode(&bad_selected).is_none());
    }

    #[test]
    fn deterministic_playouts_preserve_engine_invariants() {
        let mut reversi = Reversi::default();
        let mut occupied = 4;
        for _ in 0..80 {
            if reversi.terminal_score(1).is_some() {
                break;
            }
            let mv = reversi.legal_moves().remove(0);
            reversi = reversi.apply(&mv);
            let next_occupied = reversi.board.iter().filter(|&&piece| piece != 0).count();
            assert!(next_occupied >= occupied);
            assert!(reversi.board.iter().all(|piece| (-1..=1).contains(piece)));
            occupied = next_occupied;
        }
        assert!(reversi.terminal_score(1).is_some());

        for rules in [Rules::AngloAmerican, Rules::International] {
            let mut draughts = Draughts::new(rules);
            let mut pieces = draughts.board.iter().filter(|&&piece| piece != 0).count();
            for _ in 0..160 {
                if draughts.terminal_score(1).is_some() {
                    break;
                }
                let mv = draughts.legal_moves().remove(0);
                draughts = draughts.apply(&mv);
                let next_pieces = draughts.board.iter().filter(|&&piece| piece != 0).count();
                assert!(next_pieces <= pieces);
                assert!(draughts.board.iter().all(|piece| (-2..=2).contains(piece)));
                assert!(draughts.captured.iter().all(|&at| draughts.board[at] != 0));
                let mut unique = draughts.captured.clone();
                unique.sort_unstable();
                unique.dedup();
                assert_eq!(unique.len(), draughts.captured.len());
                pieces = next_pieces;
            }
        }

        let mut morris = Morris::default();
        for _ in 0..160 {
            if morris.terminal_score(1).is_some() {
                break;
            }
            let mv = morris.legal_moves().remove(0);
            morris = morris.apply(&mv);
            assert!(morris.board.iter().all(|piece| (-1..=1).contains(piece)));
            assert!(morris.placed.iter().all(|&placed| placed <= 9));
            assert!(morris.count(1) <= usize::from(morris.placed[0]));
            assert!(morris.count(-1) <= usize::from(morris.placed[1]));
        }

        let mut kalah = Kalah::default();
        for _ in 0..160 {
            if kalah.terminal_score(1).is_some() {
                break;
            }
            let mv = kalah.legal_moves().remove(0);
            kalah = kalah.apply(&mv);
            assert_eq!(
                kalah
                    .pits
                    .iter()
                    .map(|&seeds| u16::from(seeds))
                    .sum::<u16>(),
                48
            );
        }
        assert!(kalah.terminal_score(1).is_some());
    }

    #[test]
    fn house_ai_is_deterministic_at_each_named_strength() {
        let game = Reversi::default();
        for strength in [Strength::Casual, Strength::Club, Strength::Strong] {
            assert_eq!(
                choose_move(&game, strength),
                choose_move(&game, strength),
                "{}",
                strength.name()
            );
        }
    }

    #[test]
    fn all_primary_screens_fit_clara_bw() {
        let chrome = Chrome::default();
        let app = Parlor::default();
        let menu_issues = app.screen().diagnostics(&CLARA_BW_METRICS, &chrome).issues;
        assert!(menu_issues.is_empty(), "menu: {menu_issues:?}");
        for title in GAMES {
            let mut app = Parlor {
                view: View::Setup(title),
                ..Parlor::default()
            };
            assert!(
                app.screen()
                    .diagnostics(&CLARA_BW_METRICS, &chrome)
                    .issues
                    .is_empty(),
                "setup: {}",
                title.name()
            );
            app.start(title);
            let screen = app.screen();
            let diagnostics = screen.diagnostics(&CLARA_BW_METRICS, &chrome);
            assert!(
                diagnostics.issues.is_empty(),
                "{}: {:?}",
                title.name(),
                diagnostics.issues
            );
            assert!(screen
                .layout_with(&CLARA_BW_METRICS, &chrome)
                .rect_of_action(action_id("undo"))
                .is_some());
        }
        let solo_setup = Parlor {
            view: View::Setup(Title::Draughts),
            mode: Mode::Solo,
            rules: Rules::International,
            ..Parlor::default()
        };
        let issues = solo_setup
            .screen()
            .diagnostics(&CLARA_BW_METRICS, &chrome)
            .issues;
        assert!(issues.is_empty(), "solo setup: {issues:?}");
        let mut international = Parlor {
            rules: Rules::International,
            ..Parlor::default()
        };
        international.start(Title::Draughts);
        let issues = international
            .screen()
            .diagnostics(&CLARA_BW_METRICS, &chrome)
            .issues;
        assert!(issues.is_empty(), "international board: {issues:?}");
    }
}

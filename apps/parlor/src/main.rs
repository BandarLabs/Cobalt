//! Pass-and-play Reversi with a deliberately small, tested rule engine.
use kobo_sdk::{action_id, ActionId, Context, Glyph, KoboApp, Screen, ScreenBuilder};
use std::process::ExitCode;

const REVERSI: &str = "reversi";
const DRAUGHTS: &str = "draughts";
const MORRIS: &str = "morris";
const MANCALA: &str = "mancala";
const BACK: &str = "back";
const RESET: &str = "reset";
const SIDE: usize = 8;
const CELLS: usize = SIDE * SIDE;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Title {
    Reversi,
    Draughts,
    Morris,
    Mancala,
}
impl Title {
    fn name(self) -> &'static str {
        match self {
            Self::Reversi => "Reversi",
            Self::Draughts => "Draughts",
            Self::Morris => "Nine Men's Morris",
            Self::Mancala => "Mancala",
        }
    }
    fn detail(self) -> &'static str {
        match self {
            Self::Reversi => "Standard Reversi. Black moves first.",
            Self::Draughts => "International and Anglo-American rules are planned.",
            Self::Morris => "Nine Men's Morris placement board.",
            Self::Mancala => "Kalah (6,4).",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum View {
    Menu,
    Game(Title),
}

struct Parlor {
    view: View,
    board: [i8; CELLS],
    turn: i8,
    selected: Option<usize>,
    notice: String,
}

impl Default for Parlor {
    fn default() -> Self {
        Self {
            view: View::Menu,
            board: opening(),
            turn: 1,
            selected: None,
            notice: "Choose a game. Pass-and-play turns face the mover.".to_owned(),
        }
    }
}

fn opening() -> [i8; CELLS] {
    let mut board = [0; CELLS];
    board[27] = -1;
    board[28] = 1;
    board[35] = 1;
    board[36] = -1;
    board
}
fn cell_name(cell: usize) -> String {
    format!("square-{cell}")
}
fn player_name(turn: i8) -> &'static str {
    if turn == 1 {
        "Black"
    } else {
        "White"
    }
}
fn cell_label(value: i8, legal: bool, selected: bool) -> String {
    if selected {
        return "□".to_owned();
    }
    match value {
        1 => "●".to_owned(),
        -1 => "○".to_owned(),
        _ if legal => "·".to_owned(),
        _ => " ".to_owned(),
    }
}

fn flips(board: &[i8; CELLS], turn: i8, at: usize) -> Vec<usize> {
    if at >= CELLS || board[at] != 0 {
        return Vec::new();
    }
    let (row, col) = (at / SIDE, at % SIDE);
    let mut all = Vec::new();
    for (dr, dc) in [
        (-1isize, -1isize),
        (-1, 0),
        (-1, 1),
        (0, -1),
        (0, 1),
        (1, -1),
        (1, 0),
        (1, 1),
    ] {
        let mut run = Vec::new();
        let (mut r, mut c) = (row as isize + dr, col as isize + dc);
        while (0..SIDE as isize).contains(&r) && (0..SIDE as isize).contains(&c) {
            let value = board[r as usize * SIDE + c as usize];
            if value == -turn {
                run.push(r as usize * SIDE + c as usize);
            } else {
                if value == turn {
                    all.extend(run);
                }
                break;
            }
            r += dr;
            c += dc;
        }
    }
    all
}
fn legal(board: &[i8; CELLS], turn: i8, at: usize) -> bool {
    !flips(board, turn, at).is_empty()
}

impl Parlor {
    fn screen(&self) -> Screen {
        match self.view {
            View::Menu => ScreenBuilder::new("parlor-menu").top_bar("Parlor")
                .rows([
                    (REVERSI, "Reversi", Title::Reversi.detail(), Glyph::Circle),
                    (DRAUGHTS, "Draughts", Title::Draughts.detail(), Glyph::Grid),
                    (MORRIS, "Nine Men's Morris", Title::Morris.detail(), Glyph::Circle),
                    (MANCALA, "Mancala", Title::Mancala.detail(), Glyph::Circle),
                ]).build(),
            View::Game(Title::Reversi) => {
                let black = self.board.iter().filter(|&&piece| piece == 1).count();
                let white = self.board.iter().filter(|&&piece| piece == -1).count();
                ScreenBuilder::new("parlor-reversi").top_bar(format!("Reversi — {} to move", player_name(self.turn)))
                    .secondary(format!("{black} black · {white} white. {}", self.notice))
                    .board(8, (0..CELLS).map(|at| (cell_name(at), cell_label(self.board[at], legal(&self.board, self.turn, at), self.selected == Some(at)), None)))
                    .grid(2, false, [(RESET, "New game"), (BACK, "Games")]).build()
            }
            View::Game(title) => ScreenBuilder::new("parlor-game").top_bar(title.name())
                .heading(title.name()).text(title.detail())
                .text("This MVP's complete playable board is Reversi. The other titles remain visible rather than claiming rules they do not yet enforce.")
                .button(BACK, "Games").build(),
        }
    }
    fn show(&self, context: &mut Context) {
        context.set_screen(self.screen());
    }
    fn choose(&mut self, title: Title) {
        self.view = View::Game(title);
        self.board = opening();
        self.turn = 1;
        self.selected = None;
        self.notice = if title == Title::Reversi {
            "Tap a marked destination.".to_owned()
        } else {
            title.detail().to_owned()
        };
    }
    fn play(&mut self, at: usize) {
        let captured = flips(&self.board, self.turn, at);
        if captured.is_empty() {
            self.notice = "That square captures nothing. Tap a marked destination.".to_owned();
            return;
        }
        self.board[at] = self.turn;
        for square in captured {
            self.board[square] = self.turn;
        }
        self.turn = -self.turn;
        self.selected = None;
        if !(0..CELLS).any(|square| legal(&self.board, self.turn, square)) {
            self.turn = -self.turn;
            self.notice = format!("{} has no move; turn passes.", player_name(-self.turn));
        } else {
            self.notice = format!("Pass the reader to {}.", player_name(self.turn));
        }
    }
}

impl KoboApp for Parlor {
    fn on_start(&mut self, context: &mut Context) {
        self.show(context);
    }
    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        let chosen = [
            (REVERSI, Title::Reversi),
            (DRAUGHTS, Title::Draughts),
            (MORRIS, Title::Morris),
            (MANCALA, Title::Mancala),
        ]
        .iter()
        .find(|(name, _)| action == action_id(name))
        .map(|(_, title)| *title);
        if let Some(title) = chosen {
            self.choose(title);
            self.show(context);
            return;
        }
        if action == action_id(BACK) {
            self.view = View::Menu;
            self.show(context);
            return;
        }
        if action == action_id(RESET) {
            self.choose(Title::Reversi);
            self.show(context);
            return;
        }
        if matches!(self.view, View::Game(Title::Reversi)) {
            if let Some(square) = (0..CELLS).find(|square| action == action_id(&cell_name(*square)))
            {
                self.play(square);
                self.show(context);
            }
        }
    }
}

fn main() -> ExitCode {
    kobo_sdk::run("parlor", Parlor::default()).map_or_else(
        |error| {
            eprintln!("parlor: {error}");
            ExitCode::FAILURE
        },
        |_| ExitCode::SUCCESS,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobo_ui::{Chrome, CLARA_BW_METRICS};
    #[test]
    fn opening_has_four_legal_reversi_moves() {
        let board = opening();
        assert_eq!(
            (0..CELLS)
                .filter(|&square| legal(&board, 1, square))
                .count(),
            4
        );
        assert_eq!(flips(&board, 1, 19), vec![27]);
    }
    #[test]
    fn a_legal_move_flips_and_changes_turn() {
        let mut game = Parlor::default();
        game.choose(Title::Reversi);
        game.play(19);
        assert_eq!(game.board[19], 1);
        assert_eq!(game.board[27], 1);
        assert_eq!(game.turn, -1);
    }
    #[test]
    fn reversi_board_is_reachable_and_fits() {
        let mut game = Parlor::default();
        game.choose(Title::Reversi);
        let screen = game.screen();
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
        for square in 0..CELLS {
            assert!(layout
                .rect_of_action(action_id(&cell_name(square)))
                .is_some());
        }
        assert!(screen
            .diagnostics(&CLARA_BW_METRICS, &Chrome::default())
            .issues
            .is_empty());
    }
}

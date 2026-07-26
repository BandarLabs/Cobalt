//! Two players, one panel, three in a row.
//!
//! This exists to prove a point about the SDK as much as to be a game: it is
//! written entirely against the public builders, and the board is not a board
//! primitive. It is a `grid`, which is the same thing a keypad or an on-screen
//! keyboard is. If a game needs the framework to grow a new node type, the
//! framework is not general enough yet.
//!
//! The rules are the ones people actually play at a table: whoever is holding
//! the device taps, and the mark alternates. Nought goes first.

use kobo_sdk::{action_id, ActionId, Context, KoboApp, Screen, ScreenBuilder};
use std::process::ExitCode;

const SIZE: usize = 3;

/// The same board width, as the grid primitive wants it.
const COLUMNS: u8 = 3;
const CELLS: usize = SIZE * SIZE;
/// Every line that wins, as indices into the board.
const LINES: [[usize; SIZE]; 8] = [
    [0, 1, 2],
    [3, 4, 5],
    [6, 7, 8],
    [0, 3, 6],
    [1, 4, 7],
    [2, 5, 8],
    [0, 4, 8],
    [2, 4, 6],
];

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Mark {
    #[default]
    Empty,
    Nought,
    Cross,
}

impl Mark {
    /// What is drawn in the square. A space rather than an empty string,
    /// because an empty cell still has to occupy its place in the grid.
    const fn label(self) -> &'static str {
        match self {
            Self::Empty => " ",
            Self::Nought => "O",
            Self::Cross => "X",
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Empty => "nobody",
            Self::Nought => "O",
            Self::Cross => "X",
        }
    }

    const fn other(self) -> Self {
        match self {
            Self::Nought => Self::Cross,
            _ => Self::Nought,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Outcome {
    Playing,
    Won(Mark),
    Tie,
}

struct Game {
    board: [Mark; CELLS],
    turn: Mark,
    outcome: Outcome,
}

impl Default for Game {
    fn default() -> Self {
        Self {
            board: [Mark::Empty; CELLS],
            // Nought first, so the first tap of a fresh game is always an O.
            turn: Mark::Nought,
            outcome: Outcome::Playing,
        }
    }
}

impl Game {
    /// Applies a tap. Returns whether anything actually changed.
    ///
    /// A tap on an occupied square, or any tap after the game is over, is not
    /// a move. Silently ignoring it is deliberate: repainting an E Ink panel to
    /// say "you cannot do that" is slower and more annoying than doing nothing.
    fn play(&mut self, cell: usize) -> bool {
        if self.outcome != Outcome::Playing || cell >= CELLS || self.board[cell] != Mark::Empty {
            return false;
        }
        self.board[cell] = self.turn;
        self.outcome = self.settle();
        if self.outcome == Outcome::Playing {
            self.turn = self.turn.other();
        }
        true
    }

    fn settle(&self) -> Outcome {
        for line in LINES {
            let first = self.board[line[0]];
            if first != Mark::Empty && line.iter().all(|cell| self.board[*cell] == first) {
                return Outcome::Won(first);
            }
        }
        if self.board.iter().all(|cell| *cell != Mark::Empty) {
            Outcome::Tie
        } else {
            Outcome::Playing
        }
    }

    fn status(&self) -> String {
        match self.outcome {
            Outcome::Playing => format!("{} to play", self.turn.name()),
            Outcome::Won(mark) => format!("{} wins", mark.name()),
            Outcome::Tie => "A tie".to_owned(),
        }
    }
}

/// Cell names are fixed for the life of the application, so the action a square
/// carries never changes even as its mark does.
const NAMES: [&str; CELLS] = [
    "cell-0", "cell-1", "cell-2", "cell-3", "cell-4", "cell-5", "cell-6", "cell-7", "cell-8",
];

fn screen(game: &Game) -> Screen {
    let cells = NAMES
        .iter()
        .zip(game.board.iter())
        .map(|(name, mark)| (*name, mark.label()));
    ScreenBuilder::new("tictactoe")
        .top_bar("Tic-tac-toe")
        .heading(game.status())
        .grid(COLUMNS, true, cells)
        .button(
            "reset",
            if game.outcome == Outcome::Playing {
                "Reset game"
            } else {
                "Play again"
            },
        )
        .build()
}

impl KoboApp for Game {
    fn on_start(&mut self, context: &mut Context) {
        context.set_screen(screen(self));
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if action == action_id("reset") {
            *self = Self::default();
            context.set_screen(screen(self));
            return;
        }
        let Some(cell) = NAMES.iter().position(|name| action == action_id(name)) else {
            return;
        };
        // Only repaint when the board actually changed. On this panel an
        // unnecessary refresh is the most visible thing an application can do.
        if self.play(cell) {
            context.set_screen(screen(self));
        }
    }
}

fn main() -> ExitCode {
    match kobo_sdk::run("tictactoe", Game::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("tictactoe: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{screen, Game, Mark, Outcome, CELLS, NAMES, SIZE};
    use kobo_sdk::action_id;
    use kobo_ui::{Chrome, CLARA_BW_METRICS};

    #[test]
    fn the_first_tap_is_a_nought_and_marks_then_alternate() {
        let mut game = Game::default();
        assert!(game.play(0));
        assert_eq!(game.board[0], Mark::Nought);
        assert!(game.play(1));
        assert_eq!(game.board[1], Mark::Cross);
        assert!(game.play(2));
        assert_eq!(game.board[2], Mark::Nought);
    }

    #[test]
    fn an_occupied_square_is_not_a_move_and_does_not_pass_the_turn() {
        let mut game = Game::default();
        assert!(game.play(4));
        assert!(!game.play(4), "the same square was taken twice");
        assert_eq!(game.turn, Mark::Cross, "a rejected tap stole the turn");
    }

    #[test]
    fn every_winning_line_is_detected() {
        for line in super::LINES {
            let mut game = Game::default();
            for cell in line {
                game.board[cell] = Mark::Nought;
            }
            assert_eq!(game.settle(), Outcome::Won(Mark::Nought), "{line:?}");
        }
    }

    #[test]
    fn a_full_board_with_no_line_is_a_tie() {
        let mut game = Game::default();
        // O X O / O X X / X O O has no three in a row.
        for (cell, mark) in [
            Mark::Nought,
            Mark::Cross,
            Mark::Nought,
            Mark::Nought,
            Mark::Cross,
            Mark::Cross,
            Mark::Cross,
            Mark::Nought,
            Mark::Nought,
        ]
        .into_iter()
        .enumerate()
        {
            game.board[cell] = mark;
        }
        assert_eq!(game.settle(), Outcome::Tie);
    }

    #[test]
    fn the_game_stops_accepting_moves_once_it_is_won() {
        let mut game = Game::default();
        for cell in [0, 3, 1, 4, 2] {
            game.play(cell);
        }
        assert_eq!(game.outcome, Outcome::Won(Mark::Nought));
        assert!(!game.play(5), "a move was accepted after the game ended");
    }

    #[test]
    fn playing_again_clears_the_board_and_gives_nought_the_first_move() {
        let mut game = Game::default();
        for cell in [0, 3, 1, 4, 2] {
            game.play(cell);
        }
        game = Game::default();
        assert!(game.board.iter().all(|mark| *mark == Mark::Empty));
        assert_eq!(game.turn, Mark::Nought);
        assert_eq!(game.outcome, Outcome::Playing);
    }

    /// The board must be tappable, square by square, on the real panel.
    #[test]
    fn every_square_is_reachable_and_larger_than_a_finger() {
        let layout = screen(&Game::default()).layout_with(&CLARA_BW_METRICS, Chrome::default());
        for name in NAMES {
            let rect = layout
                .rect_of_action(action_id(name))
                .unwrap_or_else(|| panic!("{name} is not on the screen"));
            assert!(
                rect.height >= CLARA_BW_METRICS.touch_target_minimum(),
                "{name} is too small to tap: {rect:?}"
            );
            assert_eq!(rect.width, rect.height, "{name} is not square");
        }
    }

    /// A mark must not move the square it was placed in, or the next tap of a
    /// game played quickly lands somewhere else.
    #[test]
    fn the_board_does_not_move_as_it_fills() {
        let empty = screen(&Game::default()).layout_with(&CLARA_BW_METRICS, Chrome::default());
        let mut game = Game::default();
        game.play(0);
        game.play(4);
        let played = screen(&game).layout_with(&CLARA_BW_METRICS, Chrome::default());
        for name in NAMES {
            assert_eq!(
                empty.rect_of_action(action_id(name)),
                played.rect_of_action(action_id(name)),
                "{name} moved once the board had marks in it"
            );
        }
        assert_eq!(
            empty.rect_of_action(action_id("reset")),
            played.rect_of_action(action_id("reset")),
            "the reset button moved"
        );
    }

    #[test]
    fn the_whole_board_fits_on_the_panel() {
        let layout = screen(&Game::default()).layout_with(&CLARA_BW_METRICS, Chrome::default());
        let reset = layout
            .rect_of_action(action_id("reset"))
            .expect("a reset button");
        assert!(
            reset.y + reset.height <= CLARA_BW_METRICS.height,
            "the reset button is off the bottom of the panel"
        );
        assert_eq!(CELLS, SIZE * SIZE);
    }
}

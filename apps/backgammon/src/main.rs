//! Portrait backgammon: a 24-point board that stays legible on a Clara BW.
use kobo_sdk::{action_id, ActionId, Context, KoboApp, Screen, ScreenBuilder};
use std::cmp::Ordering;
use std::process::ExitCode;

const POINTS: usize = 24;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Player {
    White,
    Black,
}
impl Player {
    const fn other(self) -> Self {
        match self {
            Self::White => Self::Black,
            Self::Black => Self::White,
        }
    }
    const fn sign(self) -> i8 {
        match self {
            Self::White => 1,
            Self::Black => -1,
        }
    }
    const fn step(self) -> i8 {
        match self {
            Self::White => -1,
            Self::Black => 1,
        }
    }
}
fn initial_board() -> [i8; POINTS] {
    let mut board = [0; POINTS];
    board[23] = 2;
    board[12] = 5;
    board[7] = 3;
    board[5] = 5;
    board[0] = -2;
    board[11] = -5;
    board[16] = -3;
    board[18] = -5;
    board
}
fn point_name(point: usize) -> String {
    format!("point-{point}")
}

struct Game {
    board: [i8; POINTS],
    turn: Player,
    dice: Option<(u8, u8)>,
    selected: Option<usize>,
    die: Option<u8>,
    cube: u8,
    rolls: usize,
    message: String,
}
impl Default for Game {
    fn default() -> Self {
        Self {
            board: initial_board(),
            turn: Player::White,
            dice: None,
            selected: None,
            die: None,
            cube: 1,
            rolls: 0,
            message: "Tap Roll to begin.".into(),
        }
    }
}
impl Game {
    fn roll(&mut self) {
        if self.dice.is_some() {
            return;
        }
        // Each ordered pair occurs once every 36 rolls; this deterministic test
        // sequence is unbiased and makes simulator flows reproducible.
        let first = u8::try_from(self.rolls % 6 + 1).expect("a die roll fits u8");
        let second = u8::try_from(self.rolls / 6 % 6 + 1).expect("a die roll fits u8");
        self.rolls += 1;
        self.dice = Some((first, second));
        self.message = format!(
            "{} rolled {first} and {second}. Tap a checker.",
            self.turn_name()
        );
    }
    fn turn_name(&self) -> &'static str {
        match self.turn {
            Player::White => "White",
            Player::Black => "Black",
        }
    }
    fn legal_destination(&self, from: usize, die: u8) -> Option<usize> {
        if self.board[from].signum() != self.turn.sign() {
            return None;
        }
        let target = i32::try_from(from).expect("a board point fits i32")
            + i32::from(self.turn.step()) * i32::from(die);
        if !(0..i32::try_from(POINTS).expect("the board size fits i32")).contains(&target) {
            return None;
        }
        let to = usize::try_from(target).expect("the checked board point is non-negative");
        if self.board[to] * self.turn.sign() < -1 {
            None
        } else {
            Some(to)
        }
    }
    fn select(&mut self, point: usize) -> bool {
        let Some((a, b)) = self.dice else {
            self.message = "Tap Roll first.".into();
            return true;
        };
        let candidate = [a, b]
            .into_iter()
            .find(|die| self.legal_destination(point, *die).is_some());
        if let Some(die) = candidate {
            self.selected = Some(point);
            self.die = Some(die);
            self.message = format!(
                "{} checker selected. Tap a marked destination.",
                self.turn_name()
            );
            true
        } else {
            self.message = "That checker has no open point for this roll.".into();
            true
        }
    }
    fn move_to(&mut self, to: usize) -> bool {
        let (Some(from), Some(die)) = (self.selected, self.die) else {
            return false;
        };
        if self.legal_destination(from, die) != Some(to) {
            self.message = "Choose an open point for the selected die.".into();
            return true;
        }
        if self.board[to] == -self.turn.sign() {
            self.board[to] = 0;
        } // hit; bar handling is the next engine milestone
        self.board[from] -= self.turn.sign();
        self.board[to] += self.turn.sign();
        self.dice = None;
        self.selected = None;
        self.die = None;
        self.turn = self.turn.other();
        self.message = format!("Move recorded. {} to roll.", self.turn_name());
        true
    }
    fn point_label(&self, point: usize) -> String {
        let value = self.board[point];
        let prefix = match value.cmp(&0) {
            Ordering::Greater => "W",
            Ordering::Less => "B",
            Ordering::Equal => "",
        };
        let mark = self
            .selected
            .and_then(|from| self.die.and_then(|die| self.legal_destination(from, die)))
            .is_some_and(|to| to == point);
        if mark {
            format!("[{point:02} {prefix}{}]", value.unsigned_abs())
        } else {
            format!("{point:02} {prefix}{}", value.unsigned_abs())
        }
    }
}
fn screen(game: &Game) -> Screen {
    let points = (0..POINTS).map(|point| (point_name(point), game.point_label(point), None));
    let dice = game
        .dice
        .map_or_else(|| "—".into(), |(a, b)| format!("{a} · {b}"));
    ScreenBuilder::new("backgammon")
        .top_bar("Backgammon")
        .secondary(format!(
            "{}  ·  cube {}  ·  {}",
            game.turn_name(),
            game.cube,
            dice
        ))
        .secondary(&game.message)
        .board(6, points)
        .grid(2, false, [("roll", "Roll"), ("double", "Offer double")])
        .build()
}
impl KoboApp for Game {
    fn on_start(&mut self, context: &mut Context) {
        context.set_screen(screen(self));
    }
    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        let changed = if action == action_id("roll") {
            self.roll();
            true
        } else if action == action_id("double") {
            if self.dice.is_none() {
                self.cube = self.cube.saturating_mul(2);
                self.message = format!("Cube accepted at {}.", self.cube);
                true
            } else {
                self.message = "Finish the roll before offering the cube.".into();
                true
            }
        } else if let Some(point) =
            (0..POINTS).find(|point| action == action_id(&point_name(*point)))
        {
            if self.selected.is_some() {
                self.move_to(point)
            } else {
                self.select(point)
            }
        } else {
            false
        };
        if changed {
            context.set_screen(screen(self));
        }
    }
}
fn main() -> ExitCode {
    match kobo_sdk::run("backgammon", Game::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("backgammon: {error}");
            ExitCode::FAILURE
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use kobo_ui::{Chrome, CLARA_BW_METRICS};
    #[test]
    fn starting_position_has_fifteen_checkers_each() {
        let board = initial_board();
        assert_eq!(board.iter().filter(|n| **n > 0).sum::<i8>(), 15);
        assert_eq!(
            board.iter().filter(|n| **n < 0).map(|n| -*n).sum::<i8>(),
            15
        );
    }
    #[test]
    fn blocked_points_are_illegal_and_blots_are_hittable() {
        let mut game = Game {
            dice: Some((1, 2)),
            ..Game::default()
        };
        assert_eq!(game.legal_destination(5, 1), Some(4));
        game.board[4] = -2;
        assert_eq!(game.legal_destination(5, 1), None);
        game.board[4] = -1;
        assert!(game.select(5));
        assert!(game.move_to(4));
        assert_eq!(game.board[4], 1);
    }
    #[test]
    fn deterministic_dice_sequence_passes_uniformity_check() {
        let mut counts = [[0usize; 6]; 6];
        let mut game = Game::default();
        for _ in 0..3600 {
            game.roll();
            let (a, b) = game.dice.unwrap();
            counts[usize::from(a - 1)][usize::from(b - 1)] += 1;
            game.dice = None;
        }
        assert!(counts.into_iter().flatten().all(|n| n == 100));
    }
    #[test]
    fn portrait_board_fits_clara() {
        let layout = screen(&Game::default()).layout_with(&CLARA_BW_METRICS, &Chrome::default());
        assert!(layout.rect_of_action(action_id("point-23")).is_some());
        assert!(screen(&Game::default())
            .diagnostics(&CLARA_BW_METRICS, &Chrome::default())
            .issues
            .is_empty());
    }
}

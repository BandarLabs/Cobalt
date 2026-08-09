//! A touch-first Sudoku designed for the Clara BW's e-ink panel.

use kobo_sdk::{action_id, ActionId, Context, KoboApp, Screen, ScreenBuilder};
use std::process::ExitCode;

const SIDE: usize = 9;
const CELLS: usize = SIDE * SIDE;
const COLUMNS: u8 = 9;

const PUZZLE: [u8; CELLS] = [
    5, 3, 0, 0, 7, 0, 0, 0, 0, 6, 0, 0, 1, 9, 5, 0, 0, 0, 0, 9, 8, 0, 0, 0, 0, 6, 0, 8, 0, 0, 0, 6,
    0, 0, 0, 3, 4, 0, 0, 8, 0, 3, 0, 0, 1, 7, 0, 0, 0, 2, 0, 0, 0, 6, 0, 6, 0, 0, 0, 0, 2, 8, 0, 0,
    0, 0, 4, 1, 9, 0, 0, 5, 0, 0, 0, 0, 8, 0, 0, 7, 9,
];

const SOLUTION: [u8; CELLS] = [
    5, 3, 4, 6, 7, 8, 9, 1, 2, 6, 7, 2, 1, 9, 5, 3, 4, 8, 1, 9, 8, 3, 4, 2, 5, 6, 7, 8, 5, 9, 7, 6,
    1, 4, 2, 3, 4, 2, 6, 8, 5, 3, 7, 9, 1, 7, 1, 3, 9, 2, 4, 8, 5, 6, 9, 6, 1, 5, 3, 7, 2, 8, 4, 2,
    8, 7, 4, 1, 9, 6, 3, 5, 3, 4, 5, 2, 8, 6, 1, 7, 9,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Notice {
    Mistake,
    Hint,
}

struct Game {
    puzzle: [u8; CELLS],
    solution: [u8; CELLS],
    board: [u8; CELLS],
    selected: Option<usize>,
    mistakes: u16,
    hints: u16,
    variant: u8,
    notice: Option<Notice>,
}

impl Default for Game {
    fn default() -> Self {
        Self::new(0)
    }
}

impl Game {
    fn new(variant: u8) -> Self {
        let puzzle = shifted(PUZZLE, variant);
        Self {
            puzzle,
            solution: shifted(SOLUTION, variant),
            board: puzzle,
            selected: None,
            mistakes: 0,
            hints: 0,
            variant,
            notice: None,
        }
    }

    fn select(&mut self, cell: usize) -> bool {
        if cell >= CELLS || self.solved() || self.puzzle[cell] != 0 {
            return false;
        }
        let changed = self.selected != Some(cell) || self.notice.is_some();
        self.selected = Some(cell);
        self.notice = None;
        changed
    }

    fn enter(&mut self, digit: u8) -> bool {
        let Some(cell) = self.selected else {
            return false;
        };
        if !(1..=9).contains(&digit) || self.puzzle[cell] != 0 {
            return false;
        }
        if digit != self.solution[cell] {
            self.mistakes = self.mistakes.saturating_add(1);
            self.notice = Some(Notice::Mistake);
            return true;
        }
        self.board[cell] = digit;
        self.notice = None;
        if self.solved() {
            self.selected = None;
        } else {
            self.selected = self.next_empty(cell);
        }
        true
    }

    fn hint(&mut self) -> bool {
        if self.solved() {
            return false;
        }
        let cell = self
            .selected
            .filter(|cell| self.board[*cell] == 0)
            .or_else(|| self.board.iter().position(|digit| *digit == 0));
        let Some(cell) = cell else {
            return false;
        };
        self.board[cell] = self.solution[cell];
        self.hints = self.hints.saturating_add(1);
        self.notice = Some(Notice::Hint);
        self.selected = if self.solved() {
            None
        } else {
            self.next_empty(cell)
        };
        true
    }

    fn reset(&mut self) {
        *self = Self::new(self.variant);
    }

    fn new_game(&mut self) {
        *self = Self::new((self.variant + 1) % 9);
    }

    fn next_empty(&self, after: usize) -> Option<usize> {
        (1..=CELLS)
            .map(|step| (after + step) % CELLS)
            .find(|cell| self.board[*cell] == 0)
    }

    fn solved(&self) -> bool {
        self.board == self.solution
    }

    fn status(&self) -> String {
        if self.solved() {
            return format!(
                "Solved. {} mistake{}, {} hint{}.",
                self.mistakes,
                plural(self.mistakes),
                self.hints,
                plural(self.hints)
            );
        }
        match self.notice {
            Some(Notice::Mistake) => format!(
                "No fit. {} mistake{}.",
                self.mistakes,
                plural(self.mistakes)
            ),
            Some(Notice::Hint) => format!(
                "Hint placed. {} hint{} used.",
                self.hints,
                plural(self.hints)
            ),
            None => self.selected.map_or_else(
                || "Tap a blank square, then choose a number.".to_owned(),
                |cell| format!("R{} C{} selected.", cell / SIDE + 1, cell % SIDE + 1),
            ),
        }
    }

    fn cell_label(&self, cell: usize) -> String {
        let digit = self.board[cell];
        if self.selected == Some(cell) {
            if digit == 0 {
                "[]".to_owned()
            } else {
                format!("[{digit}]")
            }
        } else if digit == 0 {
            " ".to_owned()
        } else {
            digit.to_string()
        }
    }
}

const fn shifted<const N: usize>(mut values: [u8; N], shift: u8) -> [u8; N] {
    let mut index = 0;
    while index < N {
        if values[index] != 0 {
            values[index] = (values[index] - 1 + shift) % 9 + 1;
        }
        index += 1;
    }
    values
}

const fn plural(count: u16) -> &'static str {
    if count == 1 {
        ""
    } else {
        "s"
    }
}

fn cell_name(cell: usize) -> String {
    format!("cell-{cell}")
}

fn digit_name(digit: u8) -> String {
    format!("digit-{digit}")
}

fn screen(game: &Game) -> Screen {
    let cells = (0..CELLS).map(|cell| (cell_name(cell), game.cell_label(cell), None));
    let digits = (1..=9).map(|digit| (digit_name(digit), digit.to_string()));
    ScreenBuilder::new("sudoku")
        .top_bar("Sudoku")
        .secondary(game.status())
        .board(COLUMNS, cells)
        .grid(COLUMNS, false, digits)
        .grid(
            3,
            false,
            [
                ("hint", "Hint"),
                ("reset", "Reset"),
                ("new-game", "New game"),
            ],
        )
        .build()
}

impl KoboApp for Game {
    fn on_start(&mut self, context: &mut Context) {
        context.set_screen(screen(self));
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        let changed = if action == action_id("hint") {
            self.hint()
        } else if action == action_id("reset") {
            self.reset();
            true
        } else if action == action_id("new-game") {
            self.new_game();
            true
        } else if let Some(cell) = (0..CELLS).find(|cell| action == action_id(&cell_name(*cell))) {
            self.select(cell)
        } else if let Some(digit) = (1..=9).find(|digit| action == action_id(&digit_name(*digit))) {
            self.enter(digit)
        } else {
            false
        };
        if changed {
            context.set_screen(screen(self));
        }
    }
}

fn main() -> ExitCode {
    match kobo_sdk::run("sudoku", Game::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("sudoku: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobo_ui::{Chrome, LayoutKind, CLARA_BW_METRICS};

    fn valid_group(values: impl IntoIterator<Item = u8>) -> bool {
        values
            .into_iter()
            .fold(0_u16, |mask, digit| mask | (1_u16 << digit))
            == 0b11_1111_1110
    }

    #[test]
    fn puzzle_and_solution_are_valid() {
        assert!(PUZZLE
            .iter()
            .zip(SOLUTION)
            .all(|(clue, solved)| *clue == 0 || *clue == solved));
        assert_eq!(PUZZLE.iter().filter(|digit| **digit != 0).count(), 30);
        for row in 0..SIDE {
            assert!(valid_group(
                (0..SIDE).map(|column| SOLUTION[row * SIDE + column])
            ));
        }
        for column in 0..SIDE {
            assert!(valid_group(
                (0..SIDE).map(|row| SOLUTION[row * SIDE + column])
            ));
        }
        for block_row in 0..3 {
            for block_column in 0..3 {
                assert!(valid_group((0..3).flat_map(|row| {
                    (0..3).map(move |column| {
                        SOLUTION[(block_row * 3 + row) * SIDE + block_column * 3 + column]
                    })
                })));
            }
        }
    }

    #[test]
    fn clues_cannot_be_changed_and_mistakes_are_rejected() {
        let mut game = Game::default();
        assert!(!game.select(0));
        assert_eq!(game.board[0], 5);
        assert!(game.select(2));
        assert!(game.enter(1));
        assert_eq!(game.board[2], 0);
        assert_eq!(game.mistakes, 1);
        assert!(game.enter(4));
        assert_eq!(game.board[2], 4);
    }

    #[test]
    fn hint_reset_and_new_game_preserve_valid_rules() {
        let mut game = Game::default();
        assert!(game.select(2));
        assert!(game.hint());
        assert_eq!(game.board[2], 4);
        assert_eq!(game.hints, 1);
        game.reset();
        assert_eq!(game.board, PUZZLE);
        game.new_game();
        assert_eq!(game.variant, 1);
        assert_ne!(game.puzzle, PUZZLE);
        assert!(game
            .puzzle
            .iter()
            .zip(game.solution)
            .all(|(clue, solved)| *clue == 0 || *clue == solved));
    }

    #[test]
    fn completing_the_board_reaches_solved_state() {
        let mut game = Game::default();
        assert_eq!(game.status(), "Tap a blank square, then choose a number.");
        for cell in 0..CELLS {
            if game.puzzle[cell] == 0 {
                game.board[cell] = game.solution[cell];
            }
        }
        assert!(game.solved());
        assert!(game.status().starts_with("Solved"));
    }

    #[test]
    fn all_cells_and_digits_are_reachable_on_clara_bw() {
        let layout = screen(&Game::default()).layout_with(&CLARA_BW_METRICS, &Chrome::default());
        for cell in 0..CELLS {
            let rect = layout
                .rect_of_action(action_id(&cell_name(cell)))
                .unwrap_or_else(|| panic!("cell {cell} is missing"));
            assert_eq!(rect.width, rect.height);
            assert!(rect.width >= CLARA_BW_METRICS.touch_target_minimum());
        }
        for digit in 1..=9 {
            let rect = layout
                .rect_of_action(action_id(&digit_name(digit)))
                .unwrap_or_else(|| panic!("digit {digit} is missing"));
            assert!(rect.height >= CLARA_BW_METRICS.touch_target_minimum());
        }
    }

    #[test]
    fn three_by_three_blocks_have_clearer_gaps() {
        let layout = screen(&Game::default()).layout_with(&CLARA_BW_METRICS, &Chrome::default());
        let rect = |cell| {
            layout
                .rect_of_action(action_id(&cell_name(cell)))
                .expect("board cell")
        };
        let ordinary = rect(1).x - (rect(0).x + rect(0).width);
        let block = rect(3).x - (rect(2).x + rect(2).width);
        let ordinary_row = rect(9).y - (rect(0).y + rect(0).height);
        let block_row = rect(27).y - (rect(18).y + rect(18).height);
        assert!(block > ordinary);
        assert!(block_row > ordinary_row);
    }

    #[test]
    fn board_positions_are_stable_and_the_screen_fits() {
        let empty = screen(&Game::default()).layout_with(&CLARA_BW_METRICS, &Chrome::default());
        let mut game = Game::default();
        game.select(2);
        game.enter(4);
        let played = screen(&game).layout_with(&CLARA_BW_METRICS, &Chrome::default());
        for cell in 0..CELLS {
            let action = action_id(&cell_name(cell));
            assert_eq!(empty.rect_of_action(action), played.rect_of_action(action));
        }
        let bottom = played
            .nodes
            .iter()
            .filter(|node| matches!(node.kind, LayoutKind::Cell(..) | LayoutKind::Button(..)))
            .map(|node| node.rect.y + node.rect.height)
            .max()
            .expect("controls");
        assert!(bottom <= CLARA_BW_METRICS.height);
        let diagnostics = screen(&game).diagnostics(&CLARA_BW_METRICS, &Chrome::default());
        assert!(
            diagnostics.issues.is_empty(),
            "layout diagnostics: {:?}",
            diagnostics.issues
        );
    }
}

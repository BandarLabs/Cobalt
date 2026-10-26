//! Game rules and reversible owner edits. Persistence is acknowledged by the app.
use std::collections::VecDeque;

pub const CELLS: usize = 81;
pub const HISTORY: usize = 64;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Level {
    Easy,
    Medium,
    Hard,
}
impl Level {
    pub const ALL: [Self; 3] = [Self::Easy, Self::Medium, Self::Hard];
    pub const fn name(self) -> &'static str {
        match self {
            Self::Easy => "Easy",
            Self::Medium => "Medium",
            Self::Hard => "Hard",
        }
    }
    pub fn key(self) -> String {
        self.name().to_ascii_lowercase()
    }
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|level| level.key() == name)
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Puzzle {
    pub level: Level,
    pub clues: [u8; CELLS],
    pub solution: [u8; CELLS],
}
pub fn pack() -> Vec<Puzzle> {
    include_str!("../assets/puzzles.txt")
        .lines()
        .map(|line| {
            let parts: Vec<_> = line.split('|').collect();
            Puzzle {
                level: Level::parse(parts[0]).expect("validated bundled level"),
                clues: digits(parts[1]).expect("validated clues"),
                solution: digits(parts[2]).expect("validated solution"),
            }
        })
        .collect()
}
pub fn digits(value: &str) -> Option<[u8; CELLS]> {
    if value.len() != CELLS || !value.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    value
        .bytes()
        .map(|c| c - b'0')
        .collect::<Vec<_>>()
        .try_into()
        .ok()
}
pub fn digit_text(values: &[u8; CELLS]) -> String {
    values.iter().map(|n| char::from(b'0' + n)).collect()
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Position {
    pub board: [u8; CELLS],
    pub notes: [u16; CELLS],
    pub selected: Option<usize>,
    pub hints: u16,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Game {
    pub puzzle: usize,
    pub position: Position,
    pub undo: VecDeque<Position>,
    pub pencil: bool,
    pub checking: bool,
    pub landscape: bool,
}
impl Game {
    pub fn new(puzzle: usize, puzzles: &[Puzzle]) -> Self {
        Self {
            puzzle,
            position: Position {
                board: puzzles[puzzle].clues,
                notes: [0; CELLS],
                selected: None,
                hints: 0,
            },
            undo: VecDeque::new(),
            pencil: false,
            checking: false,
            landscape: false,
        }
    }
    pub fn solved(&self, puzzles: &[Puzzle]) -> bool {
        self.position.board == puzzles[self.puzzle].solution
    }
    pub fn editable(&self, cell: usize, puzzles: &[Puzzle]) -> bool {
        cell < CELLS && puzzles[self.puzzle].clues[cell] == 0
    }
    pub fn remember(&mut self) {
        self.undo.push_back(self.position.clone());
        if self.undo.len() > HISTORY {
            self.undo.pop_front();
        }
    }
    pub fn enter(&mut self, digit: u8, puzzles: &[Puzzle]) -> bool {
        let Some(cell) = self.position.selected else {
            return false;
        };
        if !(1..=9).contains(&digit) || !self.editable(cell, puzzles) || self.solved(puzzles) {
            return false;
        }
        if self.pencil {
            // Filled answers stay answers until the owner explicitly erases them.
            if self.position.board[cell] != 0 {
                return false;
            }
            self.remember();
            self.position.notes[cell] ^= 1 << (digit - 1);
        } else {
            if self.position.board[cell] == digit {
                return false;
            }
            self.remember();
            self.position.board[cell] = digit;
            self.position.notes[cell] = 0;
        }
        true
    }
    pub fn erase(&mut self, puzzles: &[Puzzle]) -> bool {
        let Some(cell) = self.position.selected else {
            return false;
        };
        if !self.editable(cell, puzzles)
            || (self.position.board[cell] == 0 && self.position.notes[cell] == 0)
        {
            return false;
        }
        self.remember();
        self.position.board[cell] = 0;
        self.position.notes[cell] = 0;
        true
    }
    pub fn hint(&mut self, puzzles: &[Puzzle]) -> bool {
        let Some(cell) = self.position.selected else {
            return false;
        };
        if !self.editable(cell, puzzles)
            || self.position.board[cell] == puzzles[self.puzzle].solution[cell]
        {
            return false;
        }
        self.remember();
        self.position.board[cell] = puzzles[self.puzzle].solution[cell];
        self.position.notes[cell] = 0;
        self.position.hints = self.position.hints.saturating_add(1);
        true
    }
    pub fn undo(&mut self) -> bool {
        if let Some(position) = self.undo.pop_back() {
            self.position = position;
            true
        } else {
            false
        }
    }
    pub fn reset(&mut self, puzzles: &[Puzzle]) {
        self.remember();
        self.position = Self::new(self.puzzle, puzzles).position;
    }
    pub fn next(&self, level: Level, puzzles: &[Puzzle]) -> usize {
        (1..=puzzles.len())
            .map(|step| (self.puzzle + step) % puzzles.len())
            .find(|&i| puzzles[i].level == level)
            .expect("pack has every level")
    }
}

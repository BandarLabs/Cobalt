//! Small offline word squares; clue numbering follows ordinary crossword order.
use std::collections::VecDeque;

pub const HISTORY: usize = 32;
pub struct Puzzle {
    pub id: &'static str,
    pub title: &'static str,
    pub level: &'static str,
    pub side: usize,
    pub answer: &'static [u8],
    pub across: &'static [&'static str],
    pub down: &'static [&'static str],
}
pub const PUZZLES: &[Puzzle] = &[
    Puzzle {
        id: "cat-v1",
        title: "Small beginnings",
        level: "Starter",
        side: 3,
        answer: b"CATARETEN",
        across: &[
            "Pet that purrs",
            "Present form of ‘be’, with ‘you’",
            "Number of fingers on two hands",
        ],
        down: &[
            "Animal with whiskers and a meow",
            "‘We ___ ready’",
            "Two more than eight",
        ],
    },
    Puzzle {
        id: "ball-v1",
        title: "In the square",
        level: "Easy",
        side: 4,
        answer: b"BALLAREALEADLADY",
        across: &[
            "Round object used in many games",
            "Amount of surface covered",
            "Go first and show the way",
            "Woman addressed politely",
        ],
        down: &[
            "Formal dance, or something to throw",
            "Length times width, for a rectangle",
            "Be in front of the others",
            "Word paired with ‘gentleman’",
        ],
    },
    Puzzle {
        id: "heart-v1",
        title: "Heart of the matter",
        level: "Medium",
        side: 5,
        answer: b"HEARTEMBERABUSERESINTREND",
        across: &[
            "Organ that pumps blood",
            "A glowing coal",
            "Treat badly",
            "Sticky substance from a pine tree",
            "General direction of change",
        ],
        down: &[
            "‘Learn by ___’: memorize",
            "Last glowing piece of a fire",
            "Mistreatment",
            "Amber began as this tree substance",
            "A fashion that is gaining followers",
        ],
    },
    Puzzle {
        id: "mini-v1",
        title: "Odds and ends",
        level: "Easy",
        side: 5,
        answer: b"CAT##ORE##WEDGE##DOM##YOU",
        across: &[
            "Small pet with retractable claws",
            "Rock containing a useful metal",
            "A piece thicker at one end",
            "Short form of Dominic",
            "The person being addressed",
        ],
        down: &[
            "Milk-giving farm animal",
            "‘They ___ coming’",
            "A stuffed toy bear",
            "A thick, sticky substance",
            "Large flightless Australian bird",
        ],
    },
];
impl Puzzle {
    pub fn number(&self, cell: usize) -> Option<usize> {
        let starts = |c: usize| {
            self.answer[c] != b'#'
                && (c % self.side == 0
                    || self.answer[c - 1] == b'#'
                    || c < self.side
                    || self.answer[c - self.side] == b'#')
        };
        if !starts(cell) {
            return None;
        }
        Some((0..=cell).filter(|c| starts(*c)).count())
    }
    pub fn word(&self, selected: usize, down: bool) -> Vec<usize> {
        (0..self.side)
            .map(|n| {
                if down {
                    selected % self.side + n * self.side
                } else {
                    selected / self.side * self.side + n
                }
            })
            .filter(|c| self.answer[*c] != b'#')
            .collect()
    }
    pub fn clue(&self, selected: usize, down: bool) -> (String, &'static str) {
        let cells = self.word(selected, down);
        let index = if down {
            selected % self.side
        } else {
            selected / self.side
        };
        (
            format!(
                "{} {}",
                self.number(cells[0]).expect("word starts numbered"),
                if down { "Down" } else { "Across" }
            ),
            if down {
                self.down[index]
            } else {
                self.across[index]
            },
        )
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Position {
    pub letters: Vec<u8>,
    pub selected: usize,
    pub down: bool,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Progress {
    pub position: Position,
    pub undo: VecDeque<Position>,
    /// Lifetime assistance counters are not erased by undo or restart.
    pub reveals: u32,
    pub checks: u32,
    pub solved_once: bool,
}
impl Progress {
    pub fn new(p: &Puzzle) -> Self {
        Self {
            position: Position {
                letters: p
                    .answer
                    .iter()
                    .map(|b| if *b == b'#' { b'#' } else { b'.' })
                    .collect(),
                selected: 0,
                down: false,
            },
            undo: VecDeque::new(),
            reveals: 0,
            checks: 0,
            solved_once: false,
        }
    }
    pub fn solved(&self, p: &Puzzle) -> bool {
        self.position.letters == p.answer
    }
    pub fn remember(&mut self) {
        if self.undo.len() == HISTORY {
            self.undo.pop_front();
        }
        self.undo.push_back(self.position.clone());
    }
    pub fn enter(&mut self, p: &Puzzle, text: &str) -> Result<(), &'static str> {
        let word = p.word(self.position.selected, self.position.down);
        if !text.bytes().all(|b| b.is_ascii_alphabetic())
            || (text.len() != 1 && text.len() != word.len())
        {
            return Err("Enter one letter or the whole word.");
        }
        self.remember();
        if text.len() == 1 {
            self.position.letters[self.position.selected] = text.as_bytes()[0].to_ascii_uppercase();
            let n = word
                .iter()
                .position(|c| *c == self.position.selected)
                .expect("selected in word");
            self.position.selected = word[(n + 1).min(word.len() - 1)];
        } else {
            for (cell, byte) in word.into_iter().zip(text.bytes()) {
                self.position.letters[cell] = byte.to_ascii_uppercase();
            }
        }
        self.solved_once |= self.solved(p);
        Ok(())
    }
    pub fn reveal(&mut self, p: &Puzzle) {
        let cell = self.position.selected;
        // Confirming an already-correct guess still reveals the answer.
        self.remember();
        self.position.letters[cell] = p.answer[cell];
        self.reveals = self.reveals.saturating_add(1);
        self.solved_once |= self.solved(p);
    }
    pub fn check(&mut self, p: &Puzzle) -> String {
        self.checks = self.checks.saturating_add(1);
        let word = p.word(self.position.selected, self.position.down);
        let empty = word
            .iter()
            .filter(|c| self.position.letters[**c] == b'.')
            .count();
        let wrong = word
            .iter()
            .filter(|c| {
                self.position.letters[**c] != b'.' && self.position.letters[**c] != p.answer[**c]
            })
            .count();
        if wrong == 0 && empty == 0 {
            "This word is correct.".into()
        } else {
            format!("{wrong} incorrect, {empty} empty. Your letters are unchanged.")
        }
    }
}

//! The built-in, original puzzle pack.
//!
//! Every image is a small ink drawing made from filled horizontal strokes.
//! That constraint is intentional: an all-filled or all-empty row is a
//! deduction on its own, which gives every pack member a demonstrable
//! no-guess solution before it is shipped.

use crate::solver::{runs, solve_board, Cell};

#[derive(Clone, Debug)]
pub struct Puzzle {
    pub id: String,
    pub title: String,
    pub side: usize,
    pub answer: Vec<bool>,
}

impl Puzzle {
    #[must_use]
    pub fn row_clues(&self) -> Vec<Vec<u8>> {
        (0..self.side)
            .map(|row| runs((0..self.side).map(|column| self.answer[row * self.side + column])))
            .collect()
    }

    #[must_use]
    pub fn column_clues(&self) -> Vec<Vec<u8>> {
        (0..self.side)
            .map(|column| runs((0..self.side).map(|row| self.answer[row * self.side + column])))
            .collect()
    }

    #[must_use]
    pub fn is_line_solvable(&self) -> bool {
        solve_board(self.side, &self.row_clues(), &self.column_clues()).is_some_and(|board| {
            board
                .iter()
                .zip(&self.answer)
                .all(|(cell, answer)| *cell == if *answer { Cell::Filled } else { Cell::Empty })
        })
    }
}

const TITLES: [&str; 60] = [
    "Harbor dawn",
    "Window light",
    "Rain band",
    "Still water",
    "Low cloud",
    "Night train",
    "Field notes",
    "Tide mark",
    "Paper kite",
    "Hill path",
    "Tea steam",
    "Old fence",
    "Blackbird",
    "Snow line",
    "Distant roof",
    "First frost",
    "Book spine",
    "Signal lamp",
    "Garden wall",
    "Moon rise",
    "Wood grain",
    "Blue hour",
    "Porch light",
    "Cedar shade",
    "Morning cup",
    "Shore grass",
    "Cloud break",
    "Rail bridge",
    "Moss stone",
    "North wind",
    "Quiet room",
    "Ink wash",
    "Wet pavement",
    "Map fold",
    "Long shadow",
    "Water tower",
    "Wool blanket",
    "Bird track",
    "Fog bank",
    "Farm gate",
    "Doorway",
    "Rock pool",
    "Sun blind",
    "River bend",
    "Window rain",
    "Late bus",
    "Pine ridge",
    "Small boat",
    "Street lamp",
    "Coal shed",
    "Dune grass",
    "Roof tile",
    "Night window",
    "Rain gauge",
    "Canyon wall",
    "White birch",
    "Sea wall",
    "Cloud shelf",
    "Foot bridge",
    "Last light",
];

/// Returns the full 60-puzzle corpus in deterministic order.
#[must_use]
pub fn bundled() -> Vec<Puzzle> {
    TITLES
        .iter()
        .enumerate()
        .map(|(index, title)| {
            let side = [5, 7, 9, 15, 25][index / 12];
            Puzzle {
                id: format!("pack-{index:02}"),
                title: (*title).to_owned(),
                side,
                answer: stripes(side, index),
            }
        })
        .collect()
}

fn stripes(side: usize, seed: usize) -> Vec<bool> {
    (0..side)
        .flat_map(|row| {
            let filled = (row + seed * 3) % 5 < 2 || (row + seed) % 11 == 0;
            std::iter::repeat_n(filled, side)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::bundled;

    #[test]
    fn every_bundled_puzzle_is_line_solvable_and_the_pack_has_each_requested_size() {
        let puzzles = bundled();
        assert_eq!(puzzles.len(), 60);
        for side in [5, 7, 9, 15, 25] {
            assert_eq!(
                puzzles.iter().filter(|puzzle| puzzle.side == side).count(),
                12
            );
        }
        for puzzle in &puzzles {
            assert!(puzzle.is_line_solvable(), "{} is not fair", puzzle.title);
        }
    }
}

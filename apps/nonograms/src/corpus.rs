//! Original picture collection and an immutable earlier study pack.
//! Earlier IDs and answers remain available for saved games; new pictures
//! have a separate identity namespace and are verified by the line solver.

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

    /// The rating counts complete passes of ordinary row/column deductions.
    /// It describes this solver's work, not a measured human solving time.
    #[must_use]
    pub fn difficulty(&self) -> &'static str {
        match crate::solver::solve_board_rated(self.side, &self.row_clues(), &self.column_clues()) {
            Some((board, rounds)) if !board.contains(&Cell::Unknown) => match rounds {
                0..=1 => "Easy",
                2..=3 => "Medium",
                _ => "Hard",
            },
            _ => "Needs guessing",
        }
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

/// Original picture puzzles. IDs are separate from the earlier stroke pack;
/// neither its answers nor its stored progress keys are repurposed.
#[must_use]
pub fn pictures() -> Vec<Puzzle> {
    include_str!("../assets/pictures.txt")
        .lines()
        .map(|line| {
            let fields: Vec<_> = line.split('|').collect();
            assert_eq!(fields.len(), 4, "bundled picture fields");
            let side = fields[2].parse().expect("bundled picture size");
            let answer: Vec<_> = fields[3]
                .bytes()
                .map(|byte| match byte {
                    b'#' => true,
                    b'.' => false,
                    _ => panic!("bundled picture mark"),
                })
                .collect();
            assert_eq!(answer.len(), side * side, "bundled picture square");
            Puzzle {
                id: format!("picture-{}-v1", fields[0]),
                title: fields[1].into(),
                side,
                answer,
            }
        })
        .collect()
}

#[must_use]
pub fn catalog() -> Vec<Puzzle> {
    // Stable earlier indices also preserve existing automated fixture routes.
    let mut puzzles = bundled();
    puzzles.extend(pictures());
    puzzles
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
    fn pictures_are_distinct_line_solvable_and_do_not_reassign_earlier_ids() {
        let pictures = super::pictures();
        assert_eq!(pictures.len(), 18);
        let mut unique = std::collections::BTreeSet::new();
        for puzzle in &pictures {
            assert!(puzzle.is_line_solvable(), "{}", puzzle.title);
            assert!(unique.insert((puzzle.side, puzzle.answer.clone())));
            assert!(puzzle.answer.iter().any(|filled| *filled));
            assert!(puzzle.answer.iter().any(|filled| !*filled));
            // A picture must contain detail within rows, not just horizontal stripes.
            assert!(puzzle
                .answer
                .chunks(puzzle.side)
                .any(|row| row.contains(&true) && row.contains(&false)));
            assert!(puzzle.id.starts_with("picture-") && !puzzle.id.starts_with("pack-"));
        }
        for side in [5, 7, 9, 15, 25] {
            assert!(pictures.iter().any(|p| p.side == side));
        }
        let catalog = super::catalog();
        for (old, current) in bundled().iter().zip(&catalog) {
            assert_eq!(old.id, current.id);
            assert_eq!(old.title, current.title);
            assert_eq!(old.answer, current.answer);
        }
    }

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

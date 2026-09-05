//! The bounded row/column solver used for both bundled and photo puzzles.
//!
//! It is deliberately a line solver: candidates are every arrangement that
//! fits a line's clues, filtered by marks already on the board. A cell is
//! deduced only when every remaining candidate agrees. This is the ordinary
//! nonogram technique, never a search over whole boards.

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub enum Cell {
    #[default]
    Unknown,
    Filled,
    Empty,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LineState {
    Contradiction,
    Possible(Vec<Cell>),
}

/// Consecutive filled runs in one line, represented as usable clues.
#[must_use]
pub fn runs(line: impl IntoIterator<Item = bool>) -> Vec<u8> {
    let mut runs = Vec::new();
    let mut count = 0_u8;
    for filled in line {
        if filled {
            count = count.saturating_add(1);
        } else if count != 0 {
            runs.push(count);
            count = 0;
        }
    }
    if count != 0 {
        runs.push(count);
    }
    runs
}

/// All line arrangements that satisfy `clues` and are compatible with `known`.
#[must_use]
pub fn candidates(length: usize, clues: &[u8], known: &[Cell]) -> Vec<Vec<Cell>> {
    if length != known.len() || clues.contains(&0) {
        return Vec::new();
    }
    let required =
        clues.iter().map(|run| usize::from(*run)).sum::<usize>() + clues.len().saturating_sub(1);
    if required > length {
        return Vec::new();
    }
    let mut output = Vec::new();
    let mut line = vec![Cell::Empty; length];
    place(0, 0, length, clues, known, &mut line, &mut output);
    output
}

fn place(
    clue: usize,
    cursor: usize,
    length: usize,
    clues: &[u8],
    known: &[Cell],
    line: &mut [Cell],
    output: &mut Vec<Vec<Cell>>,
) {
    if clue == clues.len() {
        if line[cursor..].iter().all(|cell| *cell != Cell::Filled) && compatible(line, known) {
            output.push(line.to_vec());
        }
        return;
    }
    let run = usize::from(clues[clue]);
    let later = clues[clue + 1..]
        .iter()
        .map(|value| usize::from(*value))
        .sum::<usize>()
        + clues.len().saturating_sub(clue + 2);
    let last_start = length.saturating_sub(run + later);
    for start in cursor..=last_start {
        let saved = line.to_vec();
        for cell in &mut line[cursor..start] {
            *cell = Cell::Empty;
        }
        for cell in &mut line[start..start + run] {
            *cell = Cell::Filled;
        }
        let next = start + run;
        if clue + 1 < clues.len() {
            line[next] = Cell::Empty;
            place(clue + 1, next + 1, length, clues, known, line, output);
        } else {
            place(clue + 1, next, length, clues, known, line, output);
        }
        line.copy_from_slice(&saved);
    }
}

fn compatible(candidate: &[Cell], known: &[Cell]) -> bool {
    candidate
        .iter()
        .zip(known)
        .all(|(candidate, known)| *known == Cell::Unknown || candidate == known)
}

/// Detects an impossible marked line and otherwise returns only forced cells.
#[must_use]
pub fn solve_line(clues: &[u8], known: &[Cell]) -> LineState {
    let candidates = candidates(known.len(), clues, known);
    let Some(first) = candidates.first() else {
        return LineState::Contradiction;
    };
    let mut forced = first.clone();
    for candidate in &candidates[1..] {
        for (forced, value) in forced.iter_mut().zip(candidate) {
            if *forced != *value {
                *forced = Cell::Unknown;
            }
        }
    }
    LineState::Possible(forced)
}

/// Applies lines until they stop changing. `None` means a contradiction.
#[must_use]
pub fn solve_board(
    side: usize,
    row_clues: &[Vec<u8>],
    column_clues: &[Vec<u8>],
) -> Option<Vec<Cell>> {
    if side == 0 || row_clues.len() != side || column_clues.len() != side {
        return None;
    }
    let mut board = vec![Cell::Unknown; side * side];
    loop {
        let mut changed = false;
        for row in 0..side {
            let cells = (0..side)
                .map(|column| board[row * side + column])
                .collect::<Vec<_>>();
            let LineState::Possible(forced) = solve_line(&row_clues[row], &cells) else {
                return None;
            };
            for (column, cell) in forced.into_iter().enumerate() {
                let at = row * side + column;
                if board[at] == Cell::Unknown && cell != Cell::Unknown {
                    board[at] = cell;
                    changed = true;
                }
            }
        }
        for column in 0..side {
            let cells = (0..side)
                .map(|row| board[row * side + column])
                .collect::<Vec<_>>();
            let LineState::Possible(forced) = solve_line(&column_clues[column], &cells) else {
                return None;
            };
            for (row, cell) in forced.into_iter().enumerate() {
                let at = row * side + column;
                if board[at] == Cell::Unknown && cell != Cell::Unknown {
                    board[at] = cell;
                    changed = true;
                }
            }
        }
        if !changed {
            return Some(board);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{candidates, runs, solve_line, Cell, LineState};

    fn brute(length: usize, clues: &[u8], known: &[Cell]) -> Vec<Vec<Cell>> {
        (0..(1_usize << length))
            .map(|bits| {
                (0..length)
                    .map(|index| {
                        if bits & (1 << index) == 0 {
                            Cell::Empty
                        } else {
                            Cell::Filled
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .filter(|line| {
                runs(line.iter().map(|cell| *cell == Cell::Filled)) == clues
                    && line
                        .iter()
                        .zip(known)
                        .all(|(cell, known)| *known == Cell::Unknown || cell == known)
            })
            .collect()
    }

    #[test]
    fn candidate_generation_agrees_with_brute_force_for_a_thousand_deterministic_lines() {
        let mut seed = 0x9e37_79b9_u64;
        for _ in 0..1_000 {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            let length = 1 + usize::try_from(seed % 15).expect("line length fits usize");
            let line = (0..length)
                .map(|index| {
                    seed.rotate_left(u32::try_from(index).expect("line index fits u32")) & 1 == 1
                })
                .collect::<Vec<_>>();
            let clues = runs(line.iter().copied());
            let known = (0..length)
                .map(|index| {
                    match seed
                        .rotate_right(u32::try_from(index * 3).expect("line rotation fits u32"))
                        % 3
                    {
                        1 if line[index] => Cell::Filled,
                        1 => Cell::Empty,
                        _ => Cell::Unknown,
                    }
                })
                .collect::<Vec<_>>();
            let mut actual = candidates(length, &clues, &known);
            let mut expected = brute(length, &clues, &known);
            actual.sort();
            expected.sort();
            assert_eq!(actual, expected, "length {length}, clues {clues:?}");
        }
    }

    #[test]
    fn impossible_mark_is_a_contradiction() {
        assert_eq!(
            solve_line(&[2], &[Cell::Empty, Cell::Empty, Cell::Unknown]),
            LineState::Contradiction
        );
    }
}

//! Local Open Sudoku SDM input. No file paths, XML, URLs or network requests.
use super::game::{digits, Level, Puzzle, CELLS};
use kobo_sdk::{
    action_id, ActionId, Context, Screen, ScreenBuilder, ShelfDownload, ShelfProgress, StoreResult,
};

pub const FILE: &str = "puzzles.sdm";
pub const BYTE_LIMIT: usize = 96 * 1024;
const PUZZLE_LIMIT: usize = 1000;
const SEARCH_LIMIT: usize = 100_000;

pub fn parse(bytes: &[u8]) -> Result<Vec<[u8; CELLS]>, String> {
    if bytes.len() > BYTE_LIMIT {
        return Err("File exceeds 96 KiB.".into());
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "SDM must contain ASCII digits.")?;
    let mut puzzles = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if puzzles.len() == PUZZLE_LIMIT {
            return Err("Use at most 1000 puzzles per file.".into());
        }
        puzzles.push(
            digits(line)
                .ok_or_else(|| format!("Line {} needs 81 digits (0 for blank).", index + 1))?,
        );
    }
    if puzzles.is_empty() {
        return Err("The SDM file is empty.".into());
    }
    Ok(puzzles)
}

fn houses(cell: usize) -> [usize; 3] {
    [cell / 9, 9 + cell % 9, 18 + cell / 27 * 3 + cell % 9 / 3]
}

/// Counts up to two solutions; even a single found solution is not accepted if
/// proving uniqueness exhausts the budget. Depth is at most 81, work is bounded.
struct Search {
    board: [u8; CELLS],
    used: [u16; 27],
    remaining: usize,
    solutions: usize,
    solution: [u8; CELLS],
}
impl Search {
    fn visit(&mut self) -> Result<(), String> {
        if self.remaining == 0 {
            return Err("Puzzle exceeds the validation work limit.".into());
        }
        self.remaining -= 1;
        let mut choice = None;
        for cell in 0..CELLS {
            if self.board[cell] != 0 {
                continue;
            }
            let used = houses(cell).iter().fold(0, |mask, &h| mask | self.used[h]);
            let available = 0x3fe_u16 & !used;
            if available == 0 {
                return Ok(());
            }
            if choice.is_none_or(|(_, old): (usize, u16)| available.count_ones() < old.count_ones())
            {
                choice = Some((cell, available));
            }
        }
        let Some((cell, available)) = choice else {
            self.solutions += 1;
            self.solution = self.board;
            return Ok(());
        };
        for digit in 1..=9 {
            let bit = 1 << digit;
            if available & bit == 0 {
                continue;
            }
            self.board[cell] = digit;
            for h in houses(cell) {
                self.used[h] |= bit;
            }
            self.visit()?;
            for h in houses(cell) {
                self.used[h] &= !bit;
            }
            self.board[cell] = 0;
            if self.solutions >= 2 {
                break;
            }
        }
        Ok(())
    }
}
pub fn validate(clues: [u8; CELLS]) -> Result<Puzzle, String> {
    validate_with_budget(clues, SEARCH_LIMIT)
}
fn validate_with_budget(clues: [u8; CELLS], budget: usize) -> Result<Puzzle, String> {
    let mut search = Search {
        board: clues,
        used: [0; 27],
        remaining: budget,
        solutions: 0,
        solution: [0; CELLS],
    };
    for (cell, &digit) in clues.iter().enumerate() {
        if digit > 9 {
            return Err("Clues must be digits from 0 to 9.".into());
        }
        if digit == 0 {
            continue;
        }
        let bit = 1 << digit;
        for h in houses(cell) {
            if search.used[h] & bit != 0 {
                return Err("Puzzle contains conflicting clues.".into());
            }
            search.used[h] |= bit;
        }
    }
    search.visit()?;
    match search.solutions {
        0 => Err("Puzzle has no solution.".into()),
        1 => Ok(Puzzle {
            clues,
            solution: search.solution,
            level: Level::Imported,
        }),
        _ => Err("Puzzle has more than one solution.".into()),
    }
}

#[derive(Default)]
pub struct Selection {
    download: Option<ShelfDownload>,
    cancelled: bool,
    puzzles: Vec<[u8; CELLS]>,
    index: usize,
    error: Option<String>,
}
impl Selection {
    pub fn screen(&self, builder: ScreenBuilder) -> Screen {
        let mut b = builder.section("Import SDM");
        if self.download.is_some() {
            b = b.secondary("Reading local file…");
        } else if self.puzzles.is_empty() {
            b = b
                .text("Copy puzzles.sdm by USB to .adds/cobalt/data/sudoku on the reader.")
                .secondary("81 digits per line. 0 means blank. Up to 1000 puzzles, 96 KiB.")
                .button("import-read", "Read file");
        } else {
            b = b
                .secondary(format!(
                    "Puzzle {} of {} · {} clues",
                    self.index + 1,
                    self.puzzles.len(),
                    self.puzzles[self.index].iter().filter(|&&n| n != 0).count()
                ))
                .secondary("Start replaces this game and its history.")
                .buttons([("import-previous", "Previous"), ("import-next", "Next")])
                .button("import-start", "Start puzzle")
                .button("import-read", "Reload file");
        }
        if let Some(error) = &self.error {
            b = b.secondary(error);
        }
        b.bottom_action("play", "Keep playing").build()
    }
    pub fn action(&mut self, context: &mut Context, action: ActionId) -> Option<Puzzle> {
        // Drain a cancelled read before starting another, so a late response
        // for the same shelf key cannot be attributed to a new transfer.
        if self.download.is_some() {
            return None;
        }
        if action == action_id("import-read") {
            self.error = None;
            self.puzzles.clear();
            self.index = 0;
            self.cancelled = false;
            let mut download = ShelfDownload::new(FILE).at_most(BYTE_LIMIT);
            download.start(context);
            self.download = Some(download);
        } else if action == action_id("import-previous") {
            self.index = self.index.saturating_sub(1);
            self.error = None;
        } else if action == action_id("import-next") {
            self.index = (self.index + 1).min(self.puzzles.len().saturating_sub(1));
            self.error = None;
        } else if action == action_id("import-start") {
            if let Some(&clues) = self.puzzles.get(self.index) {
                match validate(clues) {
                    Ok(puzzle) => return Some(puzzle),
                    Err(error) => self.error = Some(error),
                }
            }
        }
        None
    }
    pub fn cancel(&mut self) {
        self.cancelled = true;
        self.puzzles.clear();
        self.index = 0;
        self.error = None;
    }
    pub fn receive(&mut self, context: &mut Context, result: &StoreResult) -> bool {
        let Some(download) = &mut self.download else {
            return false;
        };
        if self.cancelled {
            self.download = None;
            return true;
        }
        match download.advance(context, result) {
            ShelfProgress::Done => {
                match parse(download.bytes()) {
                    Ok(puzzles) => self.puzzles = puzzles,
                    Err(error) => self.error = Some(error),
                }
                self.download = None;
            }
            ShelfProgress::Failed(_) => {
                self.download = None;
                self.error = Some(
                    "Cannot read puzzles.sdm. Check the USB copy and 96 KiB limit, then retry."
                        .into(),
                );
            }
            ShelfProgress::Moving { .. } | ShelfProgress::Elsewhere => {}
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_ambiguous_conflicting_and_exhausted_searches() {
        assert!(validate([0; CELLS]).unwrap_err().contains("more than one"));
        let mut clues = [0; CELLS];
        clues[0] = 1;
        clues[1] = 1;
        assert!(validate(clues).unwrap_err().contains("conflicting"));
        assert!(validate_with_budget(super::super::game::pack()[0].clues, 1)
            .unwrap_err()
            .contains("work limit"));
        // Exhaustion after finding one solution must not imply uniqueness.
        let mut ambiguous = super::super::game::pack()[0].solution;
        for n in &mut ambiguous {
            if *n <= 2 {
                *n = 0;
            }
        }
        assert!(validate_with_budget(ambiguous, 19)
            .unwrap_err()
            .contains("work limit"));
        clues[0] = 10;
        assert!(validate(clues).is_err());
    }
}

//! A compact, touch-first crossword with defensive `.puz` header parsing.
use kobo_sdk::{action_id, ActionId, Context, KoboApp, Screen, ScreenBuilder};
use std::process::ExitCode;

const WIDTH: usize = 7;
const GRID: &[u8] = b"CAT....A..#...R..#...E....#..#..D...........#....";
const CLUES: &[&str] = &["1 Across: Small pet", "4 Across: A reader's first letter", "1 Down: Feline companion", "2 Down: Use a clue list"];

#[derive(Debug, Eq, PartialEq)]
struct PuzHeader { width: u8, height: u8, clues: u16 }

/// Reads only the bounded `.puz` header before an importer accepts a payload.
fn parse_puz_header(bytes: &[u8]) -> Result<PuzHeader, &'static str> {
    const HEADER: usize = 0x34;
    if bytes.len() < HEADER { return Err("puzzle is shorter than its header"); }
    if &bytes[2..14] != b"ACROSS&DOWN\0" { return Err("not a .puz file"); }
    let width = bytes[0x2c]; let height = bytes[0x2d];
    if width == 0 || height == 0 || width > 25 || height > 25 { return Err("grid must be 1 to 25 cells per side"); }
    let cells = usize::from(width) * usize::from(height);
    if bytes.len() < HEADER + cells * 2 { return Err("puzzle grid is incomplete"); }
    Ok(PuzHeader { width, height, clues: u16::from_le_bytes([bytes[0x2e], bytes[0x2f]]) })
}
fn cell_name(cell: usize) -> String { format!("cell-{cell}") }

struct Game { letters: Vec<char>, selected: Option<usize>, candidate: char, down: bool, clue: usize, completed: bool }
impl Default for Game {
    fn default() -> Self {
        Self { letters: GRID.iter().map(|byte| if *byte == b'#' { '#' } else { ' ' }).collect(), selected: None, candidate: 'A', down: false, clue: 0, completed: false }
    }
}
impl Game {
    fn select(&mut self, cell: usize) -> bool {
        if cell >= GRID.len() || GRID[cell] == b'#' { return false; }
        if self.selected == Some(cell) { self.down = !self.down; } else { self.selected = Some(cell); }
        self.clue = (self.clue + 1) % CLUES.len(); true
    }
    fn enter(&mut self, letter: char) -> bool {
        let Some(selected) = self.selected else { return false; };
        if self.completed || GRID[selected] == b'#' { return false; }
        self.letters[selected] = letter;
        self.completed = GRID.iter().enumerate().filter(|(_, byte)| **byte != b'#').all(|(i, byte)| self.letters[i] == char::from(*byte));
        true
    }
    fn status(&self) -> String {
        if self.completed { "Solved. Check and reveal are no longer needed.".into() }
        else { format!("{} · {}", if self.down { "Down" } else { "Across" }, CLUES[self.clue]) }
    }
}
fn screen(game: &Game) -> Screen {
    let cells = (0..GRID.len()).map(|cell| {
        let label = if GRID[cell] == b'#' { format!("{cell:02} ■") } else if Some(cell) == game.selected { format!("{cell:02} [{}]", game.letters[cell]) } else { format!("{cell:02} {}", game.letters[cell]) };
        (cell_name(cell), label, None)
    });
    ScreenBuilder::new("crossword").top_bar("Crossword").secondary(game.status())
        .board(WIDTH as u8, cells)
        .grid(2, false, [("letter", format!("Letter: {}", game.candidate)), ("enter", "Enter letter".to_owned())])
        .grid(2, false, [("clue", "Next clue"), ("clear", "Clear cell")]).build()
}
impl KoboApp for Game {
    fn on_start(&mut self, context: &mut Context) { context.set_screen(screen(self)); }
    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        let changed = if action == action_id("clue") { self.clue = (self.clue + 1) % CLUES.len(); true }
        else if action == action_id("clear") { if let Some(selected) = self.selected { self.letters[selected] = ' '; true } else { false } }
        else if let Some(cell) = (0..GRID.len()).find(|cell| action == action_id(&cell_name(*cell))) { self.select(cell) }
        else if action == action_id("letter") { self.candidate = if self.candidate == 'Z' { 'A' } else { char::from_u32(self.candidate as u32 + 1).unwrap_or('A') }; true }
        else if action == action_id("enter") { self.enter(self.candidate) }
        else { false };
        if changed { context.set_screen(screen(self)); }
    }
}
fn main() -> ExitCode {
    match kobo_sdk::run("crossword", Game::default()) { Ok(()) => ExitCode::SUCCESS, Err(error) => { eprintln!("crossword: {error}"); ExitCode::FAILURE } }
}
#[cfg(test)]
mod tests {
    use super::*; use kobo_ui::{Chrome, CLARA_BW_METRICS};
    fn header(width: u8, height: u8) -> Vec<u8> { let mut bytes = vec![0; 0x34 + usize::from(width) * usize::from(height) * 2]; bytes[2..14].copy_from_slice(b"ACROSS&DOWN\0"); bytes[0x2c] = width; bytes[0x2d] = height; bytes }
    #[test] fn puz_header_refuses_truncated_and_oversize_inputs() { assert_eq!(parse_puz_header(&[]), Err("puzzle is shorter than its header")); assert_eq!(parse_puz_header(&header(26, 1)), Err("grid must be 1 to 25 cells per side")); assert_eq!(parse_puz_header(&header(7, 7)).unwrap().width, 7); }
    #[test] fn second_tap_changes_direction_and_letters_persist() { let mut game = Game::default(); assert!(game.select(0)); assert!(game.select(0)); assert!(game.down); assert!(game.enter('C')); assert_eq!(game.letters[0], 'C'); }
    #[test] fn board_and_picker_fit_clara_panel() { let layout = screen(&Game::default()).layout_with(&CLARA_BW_METRICS, &Chrome::default()); assert!(layout.rect_of_action(action_id("cell-0")).is_some()); assert!(layout.rect_of_action(action_id("enter")).is_some()); let diagnostics = screen(&Game::default()).diagnostics(&CLARA_BW_METRICS, &Chrome::default()); assert!(diagnostics.issues.is_empty(), "{:?}", diagnostics.issues); }
}

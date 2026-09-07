//! A compact, touch-first crossword with defensive `.puz` header parsing.
use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{action_id, ActionId, Context, KoboApp, Screen, ScreenBuilder, StoreResult};
use std::process::ExitCode;

const STATE: &str = "crossword-state-v1";
const WIDTH: u8 = 5;
const GRID: &[u8] = b"HEARTEMBERABUSERESINTREND";
const CLUES: &[&str] = &[
    "1 Across: Organ that pumps blood",
    "2 Across: A glowing coal",
    "3 Across: Treat badly",
    "4 Across: Fragrant tree secretion",
    "5 Across: General direction of change",
    "1 Down: Organ that pumps blood",
    "2 Down: A glowing coal",
    "3 Down: Treat badly",
    "4 Down: Fragrant tree secretion",
    "5 Down: General direction of change",
];

#[cfg(test)]
#[derive(Debug, Eq, PartialEq)]
struct PuzHeader {
    width: u8,
    height: u8,
    clues: u16,
}

/// Reads only the bounded `.puz` header before an importer accepts a payload.
#[cfg(test)]
fn parse_puz_header(bytes: &[u8]) -> Result<PuzHeader, &'static str> {
    const HEADER: usize = 0x34;
    if bytes.len() < HEADER {
        return Err("puzzle is shorter than its header");
    }
    if &bytes[2..14] != b"ACROSS&DOWN\0" {
        return Err("not a .puz file");
    }
    let width = bytes[0x2c];
    let height = bytes[0x2d];
    if width == 0 || height == 0 || width > 25 || height > 25 {
        return Err("grid must be 1 to 25 cells per side");
    }
    let cells = usize::from(width) * usize::from(height);
    if bytes.len() < HEADER + cells * 2 {
        return Err("puzzle grid is incomplete");
    }
    Ok(PuzHeader {
        width,
        height,
        clues: u16::from_le_bytes([bytes[0x2e], bytes[0x2f]]),
    })
}
fn cell_name(cell: usize) -> String {
    format!("cell-{cell}")
}

struct Game {
    letters: Vec<char>,
    selected: Option<usize>,
    down: bool,
    clue: usize,
    completed: bool,
    help: bool,
    clues: bool,
    typing: bool,
    keyboard: Keyboard,
    notice: Option<String>,
    edited: bool,
}
impl Default for Game {
    fn default() -> Self {
        Self {
            letters: GRID
                .iter()
                .map(|byte| if *byte == b'#' { '#' } else { ' ' })
                .collect(),
            selected: None,
            down: false,
            clue: 0,
            completed: false,
            help: false,
            clues: false,
            typing: false,
            keyboard: Keyboard::new(),
            notice: None,
            edited: false,
        }
    }
}
impl Game {
    fn select(&mut self, cell: usize) -> bool {
        if cell >= GRID.len() || GRID[cell] == b'#' {
            return false;
        }
        if self.selected == Some(cell) {
            self.down = !self.down;
        } else {
            self.selected = Some(cell);
        }
        let side = usize::from(WIDTH);
        self.clue = if self.down {
            side + cell % side
        } else {
            cell / side
        };
        true
    }
    fn enter(&mut self, letter: char) -> bool {
        let Some(selected) = self.selected else {
            return false;
        };
        if self.completed || GRID[selected] == b'#' {
            return false;
        }
        self.letters[selected] = letter;
        self.completed = GRID
            .iter()
            .enumerate()
            .filter(|(_, byte)| **byte != b'#')
            .all(|(i, byte)| self.letters[i] == char::from(*byte));
        self.notice = self.completed.then(|| "Solved.".to_owned());
        true
    }
    fn status(&self) -> String {
        if self.completed {
            "Solved.".into()
        } else {
            CLUES[self.clue].to_owned()
        }
    }

    fn encode(&self) -> Vec<u8> {
        let letters = self
            .letters
            .iter()
            .map(|letter| if *letter == ' ' { '.' } else { *letter })
            .collect::<String>();
        format!(
            "{letters};{};{}",
            self.selected
                .map_or_else(|| "-".to_owned(), |cell| cell.to_string()),
            u8::from(self.down)
        )
        .into_bytes()
    }

    fn restore(&mut self, value: &[u8]) -> bool {
        let Ok(text) = std::str::from_utf8(value) else {
            return false;
        };
        let mut fields = text.split(';');
        let (Some(letters), Some(selected), Some(down), None) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            return false;
        };
        if letters.chars().count() != GRID.len() {
            return false;
        }
        let restored = letters
            .chars()
            .enumerate()
            .map(|(index, letter)| {
                if GRID[index] == b'#' {
                    (letter == '#').then_some('#')
                } else if letter == '.' {
                    Some(' ')
                } else {
                    letter.is_ascii_uppercase().then_some(letter)
                }
            })
            .collect::<Option<Vec<_>>>();
        let Some(restored) = restored else {
            return false;
        };
        let selected = if selected == "-" {
            None
        } else {
            selected
                .parse::<usize>()
                .ok()
                .filter(|cell| *cell < GRID.len() && GRID[*cell] != b'#')
        };
        if selected.is_none() && text.split(';').nth(1) != Some("-") {
            return false;
        }
        let down = match down {
            "0" => false,
            "1" => true,
            _ => return false,
        };
        self.letters = restored;
        self.selected = selected;
        self.down = down;
        self.completed = GRID
            .iter()
            .enumerate()
            .all(|(index, byte)| self.letters[index] == char::from(*byte));
        if let Some(cell) = selected {
            let side = usize::from(WIDTH);
            self.clue = if down {
                side + cell % side
            } else {
                cell / side
            };
        }
        true
    }
}
fn screen(game: &Game) -> Screen {
    if game.help {
        return ScreenBuilder::new("crossword-help")
            .top_bar("How to play")
            .owns_back(true)
            .heading("Fill the white squares")
            .text("Read the clue, choose a square, then pick and enter a letter.")
            .text("Tap the same square again to switch between Across and Down.")
            .text("Next clue moves through the clue list. Clear cell erases one letter.")
            .bottom_action("close-help", "Play")
            .build();
    }
    if game.clues {
        let offset = if game.down { 5 } else { 0 };
        return ScreenBuilder::new("crossword-clues")
            .top_bar(if game.down {
                "Down clues"
            } else {
                "Across clues"
            })
            .owns_back(true)
            .rows((0..5).map(|index| {
                (
                    format!("clue-{index}"),
                    format!("{}", index + 1),
                    CLUES[offset + index]
                        .split_once(": ")
                        .map_or(CLUES[offset + index], |(_, clue)| clue),
                    kobo_sdk::Glyph::Note,
                )
            }))
            .buttons([("clue-direction", "Switch direction"), ("board", "Board")])
            .build();
    }
    if game.typing {
        return ScreenBuilder::new("crossword-entry")
            .top_bar("Crossword")
            .owns_back(true)
            .heading(game.status())
            .typed(&game.keyboard, "One letter")
            .keyboard(&game.keyboard, "Enter letter")
            .bottom_action("cancel", "Cancel")
            .build();
    }
    let cells = (0..GRID.len()).map(|cell| {
        let label = if GRID[cell] == b'#' {
            "■".to_owned()
        } else {
            game.letters[cell].to_string()
        };
        (cell_name(cell), label, None)
    });
    let mut screen = ScreenBuilder::new("crossword")
        .top_bar("Crossword")
        .secondary(game.status());
    if let Some(notice) = &game.notice {
        screen = screen.secondary(notice);
    }
    screen
        .board(WIDTH, cells)
        .grid(
            3,
            false,
            [
                ("clues", "Clues"),
                ("clear", "Clear cell"),
                ("how-to-play", "How to play"),
            ],
        )
        .build()
}
impl KoboApp for Game {
    fn on_start(&mut self, context: &mut Context) {
        context.store().load(STATE);
        context.set_screen(screen(self));
    }
    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        if let StoreResult::Loaded { key, value } = result {
            if key == STATE && !self.edited {
                if let Some(value) = value {
                    if !self.restore(&value) {
                        self.notice =
                            Some("Saved progress was damaged and was ignored.".to_owned());
                    }
                }
                context.set_screen(screen(self));
            }
        }
    }
    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        let mut save = false;
        let changed = if self.help {
            if action == action_id("close-help") || action == ActionId::BACK {
                self.help = false;
                true
            } else {
                false
            }
        } else if action == action_id("how-to-play") {
            self.help = true;
            true
        } else if self.clues {
            if action == action_id("board") || action == ActionId::BACK {
                self.clues = false;
                true
            } else if action == action_id("clue-direction") {
                self.down = !self.down;
                true
            } else if let Some(index) =
                (0..5).find(|index| action == action_id(&format!("clue-{index}")))
            {
                self.selected = Some(if self.down {
                    index
                } else {
                    index * usize::from(WIDTH)
                });
                self.clue = if self.down { 5 + index } else { index };
                self.clues = false;
                true
            } else {
                false
            }
        } else if self.typing {
            if action == action_id("cancel") || action == ActionId::BACK {
                self.keyboard.clear();
                self.typing = false;
                true
            } else if let Some(pressed) = self.keyboard.press(action) {
                if pressed == Pressed::Submitted {
                    let entered = self.keyboard.take();
                    let mut letters = entered.chars().filter(char::is_ascii_alphabetic);
                    if let (Some(letter), None) = (letters.next(), letters.next()) {
                        save = self.enter(letter.to_ascii_uppercase());
                        self.typing = false;
                    } else {
                        self.notice = Some("Enter one letter.".to_owned());
                    }
                }
                true
            } else {
                false
            }
        } else if action == action_id("clues") {
            self.clues = true;
            true
        } else if action == action_id("clear") {
            if let Some(selected) = self.selected {
                self.letters[selected] = ' ';
                self.completed = false;
                self.notice = None;
                save = true;
                true
            } else {
                false
            }
        } else if let Some(cell) =
            (0..GRID.len()).find(|cell| action == action_id(&cell_name(*cell)))
        {
            let selected = self.select(cell);
            if selected {
                self.keyboard.clear();
                self.typing = true;
            }
            selected
        } else {
            false
        };
        if save {
            self.edited = true;
            context.store().save(STATE, self.encode());
        }
        if changed {
            context.set_screen(screen(self));
        }
    }
}
fn main() -> ExitCode {
    match kobo_sdk::run("crossword", Game::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("crossword: {error}");
            ExitCode::FAILURE
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use kobo_ui::{Chrome, CLARA_BW_METRICS};
    fn header(width: u8, height: u8) -> Vec<u8> {
        let mut bytes = vec![0; 0x34 + usize::from(width) * usize::from(height) * 2];
        bytes[2..14].copy_from_slice(b"ACROSS&DOWN\0");
        bytes[0x2c] = width;
        bytes[0x2d] = height;
        bytes
    }
    #[test]
    fn puz_header_refuses_truncated_and_oversize_inputs() {
        assert_eq!(
            parse_puz_header(&[]),
            Err("puzzle is shorter than its header")
        );
        assert_eq!(
            parse_puz_header(&header(26, 1)),
            Err("grid must be 1 to 25 cells per side")
        );
        assert_eq!(parse_puz_header(&header(7, 7)).unwrap().width, 7);
    }
    #[test]
    fn second_tap_changes_direction_and_letters_persist() {
        let mut game = Game::default();
        assert!(game.select(0));
        assert!(game.select(0));
        assert!(game.down);
        assert!(game.enter('C'));
        assert_eq!(game.letters[0], 'C');
    }
    #[test]
    fn board_and_clue_controls_fit_clara_panel() {
        let layout = screen(&Game::default()).layout_with(&CLARA_BW_METRICS, &Chrome::default());
        assert!(layout.rect_of_action(action_id("cell-0")).is_some());
        assert!(layout.rect_of_action(action_id("clues")).is_some());
        let diagnostics =
            screen(&Game::default()).diagnostics(&CLARA_BW_METRICS, &Chrome::default());
        assert!(diagnostics.issues.is_empty(), "{:?}", diagnostics.issues);
    }

    #[test]
    fn how_to_play_is_short_and_reachable() {
        let mut game = Game::default();
        assert!(screen(&game)
            .layout_with(&CLARA_BW_METRICS, &Chrome::default())
            .rect_of_action(action_id("how-to-play"))
            .is_some());
        game.help = true;
        assert!(screen(&game)
            .diagnostics(&CLARA_BW_METRICS, &Chrome::measuring(true))
            .issues
            .is_empty());
    }

    #[test]
    fn board_cells_show_only_entered_letters() {
        let mut game = Game::default();
        game.select(0);
        game.enter('C');
        let screen = screen(&game);
        let labels = screen
            .nodes
            .iter()
            .find_map(|node| match node {
                kobo_ui::Node::Grid {
                    square: true,
                    cells,
                    ..
                } => Some(
                    cells
                        .iter()
                        .map(|cell| cell.label.as_str())
                        .collect::<Vec<_>>(),
                ),
                _ => None,
            })
            .expect("crossword board");

        assert_eq!(labels[0], "C");
        assert!(labels.iter().all(|label| !label.contains('[')));
        assert!(labels.iter().all(|label| !label.contains("00")));
    }

    #[test]
    fn progress_round_trips_and_refuses_malformed_state() {
        let mut game = Game::default();
        game.select(0);
        game.enter('H');
        game.down = true;
        let encoded = game.encode();
        let mut restored = Game::default();
        assert!(restored.restore(&encoded));
        assert_eq!(restored.letters, game.letters);
        assert_eq!(restored.selected, game.selected);
        assert!(restored.down);
        assert!(!restored.restore(b"too-short;0;1"));
    }

    #[test]
    fn bundled_grid_can_be_solved_with_letters() {
        let mut game = Game::default();
        for (cell, letter) in GRID.iter().copied().enumerate() {
            game.selected = Some(cell);
            assert!(game.enter(char::from(letter)));
        }
        assert!(game.completed);
    }
}

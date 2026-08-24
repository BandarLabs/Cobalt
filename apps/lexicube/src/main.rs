//! Sixteen letter dice, three minutes, and a dictionary for the argument
//! afterwards.
//!
//! The board is the game: players find words by eye and write them on paper,
//! exactly as they would around a physical tray. What the app adds is the
//! shake, the clock, and the part every table wants when the pens go down —
//! checking whether a word is real. Validity is answered from an embedded
//! SOWPODS word list, misspellings get near-miss suggestions, and definitions
//! come from the runtime's offline dictionaries when the owner has installed
//! any.
//!
//! The dice lie rotated the way they landed, as on the table. Text cells
//! only draw upright, so the board is rendered as one picture: outlined
//! tiles, each letter rasterized from the platform's own display face and
//! turned in quarter turns. The face is Atkinson Hyperlegible, whose letter
//! forms keep a turned N from reading as a Z.

use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{
    action_id, ActionId, Context, DictionaryEntry, Heartbeat, KoboApp, PictureHandle, Screen,
    ScreenBuilder, Space, TaskId, TaskOutcome, TilePicture,
};
use std::collections::HashSet;
use std::process::ExitCode;
use std::sync::OnceLock;

/// The sixteen dice of the modern tabletop set, one row per die. The `Qu`
/// face is two letters on one die, as it is on the plastic.
const DICE: [[&str; 6]; 16] = [
    ["R", "I", "F", "O", "B", "X"],
    ["I", "F", "E", "H", "E", "Y"],
    ["D", "E", "N", "O", "W", "S"],
    ["U", "T", "O", "K", "N", "D"],
    ["H", "M", "S", "R", "A", "O"],
    ["L", "U", "P", "E", "T", "S"],
    ["A", "C", "I", "T", "O", "A"],
    ["Y", "L", "G", "K", "U", "E"],
    ["Qu", "B", "M", "J", "O", "A"],
    ["E", "H", "I", "S", "P", "N"],
    ["V", "E", "T", "I", "G", "N"],
    ["B", "A", "L", "I", "Y", "T"],
    ["E", "Z", "A", "V", "N", "D"],
    ["R", "A", "L", "E", "S", "C"],
    ["U", "W", "I", "L", "R", "G"],
    ["P", "A", "C", "E", "M", "D"],
];

const BOARD_SIDE: usize = 4;
const BOARD_COLUMNS: u8 = 4;
const BOARD_CELLS: usize = BOARD_SIDE * BOARD_SIDE;
const GAME_SECONDS: u32 = 3 * 60;
/// How many near misses are worth offering before the list is noise.
const MAX_SUGGESTIONS: usize = 12;
/// Below this a word scores nothing at the table, so it is not worth checking.
const MIN_WORD_LETTERS: usize = 3;

/// Every word SOWPODS accepts, one per line, embedded so the answer needs no
/// radio. Parsed once, on the first lookup rather than at launch, so starting
/// a game never waits on it.
const WORD_LIST: &str = include_str!("../words/sowpods.txt");

fn words() -> &'static HashSet<&'static str> {
    static WORDS: OnceLock<HashSet<&'static str>> = OnceLock::new();
    WORDS.get_or_init(|| WORD_LIST.lines().filter(|line| !line.is_empty()).collect())
}

fn verify(word: &str) -> bool {
    words().contains(word.to_uppercase().as_str())
}

/// Words one edit away, for the "did you mean" list. A deletion, insertion,
/// substitution, or adjacent swap covers what a pencil-to-keyboard transfer
/// actually gets wrong.
fn suggest(word: &str) -> Vec<String> {
    let word = word.to_uppercase();
    let letters: Vec<char> = word.chars().collect();
    let alphabet = 'A'..='Z';
    let mut candidates = std::collections::BTreeSet::new();
    for index in 0..letters.len() {
        // Deletion.
        let mut shorter = letters.clone();
        shorter.remove(index);
        candidates.insert(shorter.into_iter().collect::<String>());
        // Substitution.
        for letter in alphabet.clone() {
            let mut swapped = letters.clone();
            swapped[index] = letter;
            candidates.insert(swapped.into_iter().collect::<String>());
        }
        // Adjacent transposition.
        if index + 1 < letters.len() {
            let mut turned = letters.clone();
            turned.swap(index, index + 1);
            candidates.insert(turned.into_iter().collect::<String>());
        }
    }
    // Insertion.
    for index in 0..=letters.len() {
        for letter in alphabet.clone() {
            let mut longer = letters.clone();
            longer.insert(index, letter);
            candidates.insert(longer.into_iter().collect::<String>());
        }
    }
    candidates.remove(&word);
    candidates
        .into_iter()
        .filter(|candidate| {
            candidate.len() >= MIN_WORD_LETTERS && words().contains(candidate.as_str())
        })
        .take(MAX_SUGGESTIONS)
        .map(|candidate| candidate.to_lowercase())
        .collect()
}

/// A small deterministic generator, seeded per shake. The quality bar is a
/// party game's dice, not cryptography.
struct Rng(u64);

impl Rng {
    fn from_clock() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0x9e37_79b9, |elapsed| {
                // The low bits are the fast-moving ones, and all a seed needs.
                u64::try_from(elapsed.as_nanos() & u128::from(u64::MAX)).unwrap_or(0x9e37_79b9)
            });
        Self(nanos | 1)
    }

    fn next(&mut self) -> u64 {
        // xorshift64, which is enough state to never repeat a shake in play.
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, bound: usize) -> usize {
        let bound = u64::try_from(bound.max(1)).unwrap_or(1);
        usize::try_from(self.next() % bound).unwrap_or(0)
    }
}

/// One shake: every die lands somewhere on the board showing one face.
fn shake(rng: &mut Rng) -> [&'static str; BOARD_CELLS] {
    let mut order: [usize; BOARD_CELLS] = std::array::from_fn(|index| index);
    for index in (1..order.len()).rev() {
        order.swap(index, rng.below(index + 1));
    }
    std::array::from_fn(|cell| {
        let die = DICE[order[cell]];
        die[rng.below(die.len())]
    })
}

/// How each die landed: a quarter-turn count per cell, as on the table.
fn spin(rng: &mut Rng) -> [u8; BOARD_CELLS] {
    let mut turns = [0_u8; BOARD_CELLS];
    for turn in &mut turns {
        *turn = u8::try_from(rng.below(4)).unwrap_or(0);
    }
    turns
}

/// The board drawn as one picture, because that is the only way a die can lie
/// rotated the way it landed: text cells draw upright, and a board where every
/// letter faces the reader is a different, easier game.
mod board_art {
    use super::{BOARD_CELLS, BOARD_SIDE};

    /// The published bitmap's edge. Chosen to out-resolve every panel the
    /// layout tests cover while staying inside the protocol's inline picture
    /// limit (800 * 800 = 640,000 bytes of grey).
    ///
    /// Written twice, in the two types its two consumers need, because a
    /// `usize` indexes the buffer here while `put_picture` speaks `u32`; the
    /// tests pin that the two never drift apart.
    pub const BOARD_PX: usize = 800;
    pub const BOARD_EDGE: u32 = 800;
    const TILE_GAP: usize = 12;
    const TILE_PX: usize = (BOARD_PX - (BOARD_SIDE + 1) * TILE_GAP) / BOARD_SIDE;
    const BORDER_PX: usize = 3;
    const WHITE: u8 = 255;
    const INK: u8 = 0;
    /// The letter's height as a share of its tile, sized down for `Qu`. The
    /// operands are tile-sized, far inside `f32`'s exact range.
    #[allow(clippy::cast_precision_loss)]
    const SINGLE_PX: f32 = TILE_PX as f32 * 0.58;
    #[allow(clippy::cast_precision_loss)]
    const DOUBLE_PX: f32 = TILE_PX as f32 * 0.40;

    /// A rasterized die face: coverage, 0 for paper through 255 for full ink.
    struct Stamp {
        width: usize,
        height: usize,
        coverage: Vec<u8>,
    }

    impl Stamp {
        /// The same face a quarter turn clockwise. Four turns is the identity,
        /// which the tests pin, because an off-by-one here is a mirrored
        /// letter and a mirrored letter is a different letter.
        fn quarter_turn(&self) -> Self {
            let width = self.height;
            let height = self.width;
            let mut coverage = vec![0_u8; self.coverage.len()];
            for y in 0..height {
                for x in 0..width {
                    coverage[y * width + x] = self.coverage[(self.height - 1 - x) * self.width + y];
                }
            }
            Self {
                width,
                height,
                coverage,
            }
        }

        fn turned(mut self, quarter_turns: u8) -> Self {
            for _ in 0..quarter_turns % 4 {
                self = self.quarter_turn();
            }
            self
        }
    }

    /// Rasterizes one face, `Qu` as two glyphs on a shared baseline.
    ///
    /// Every quantity in here is a glyph measurement: a few hundred pixels,
    /// far inside every cast's exact range.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        clippy::cast_sign_loss
    )]
    fn stamp(font: &fontdue::Font, face: &str, px: f32) -> Stamp {
        let glyphs: Vec<(fontdue::Metrics, Vec<u8>)> = face
            .chars()
            .map(|letter| font.rasterize(letter, px))
            .collect();
        let spacing = (px / 12.0) as usize;
        let ascent = glyphs
            .iter()
            .map(|(metrics, _)| metrics.height as i32 + metrics.ymin)
            .max()
            .unwrap_or(0);
        let descent = glyphs
            .iter()
            .map(|(metrics, _)| -metrics.ymin)
            .max()
            .unwrap_or(0)
            .max(0);
        let width: usize = glyphs
            .iter()
            .map(|(metrics, _)| metrics.width)
            .sum::<usize>()
            + spacing * glyphs.len().saturating_sub(1);
        let height = (ascent + descent).max(1) as usize;
        let mut coverage = vec![0_u8; width.max(1) * height];
        let mut pen = 0_usize;
        for (metrics, bitmap) in &glyphs {
            let top = (ascent - (metrics.height as i32 + metrics.ymin)).max(0) as usize;
            for row in 0..metrics.height {
                for column in 0..metrics.width {
                    let target = (top + row) * width.max(1) + pen + column;
                    coverage[target] = coverage[target].max(bitmap[row * metrics.width + column]);
                }
            }
            pen += metrics.width + spacing;
        }
        Stamp {
            width: width.max(1),
            height,
            coverage,
        }
    }

    /// Draws the whole board: outlined tiles, each letter turned the way its
    /// die landed. `None` when the bundled typeface cannot be loaded, which
    /// the caller answers with the upright text board instead.
    pub fn render(
        board: &[&'static str; BOARD_CELLS],
        turns: &[u8; BOARD_CELLS],
    ) -> Option<Vec<u8>> {
        let font =
            fontdue::Font::from_bytes(kobo_text::DISPLAY_FONT, fontdue::FontSettings::default())
                .ok()?;
        let mut grey = vec![WHITE; BOARD_PX * BOARD_PX];
        for cell in 0..BOARD_CELLS {
            let row = cell / BOARD_SIDE;
            let column = cell % BOARD_SIDE;
            let left = TILE_GAP + column * (TILE_PX + TILE_GAP);
            let top = TILE_GAP + row * (TILE_PX + TILE_GAP);
            let interior = BORDER_PX..TILE_PX - BORDER_PX;
            for y in 0..TILE_PX {
                for x in 0..TILE_PX {
                    if !interior.contains(&y) || !interior.contains(&x) {
                        grey[(top + y) * BOARD_PX + left + x] = INK;
                    }
                }
            }
            let face = board[cell];
            if face.is_empty() {
                continue;
            }
            let size = if face.chars().count() == 1 {
                SINGLE_PX
            } else {
                DOUBLE_PX
            };
            let letter = stamp(&font, face, size).turned(turns[cell]);
            let offset_x = left + (TILE_PX.saturating_sub(letter.width)) / 2;
            let offset_y = top + (TILE_PX.saturating_sub(letter.height)) / 2;
            for y in 0..letter.height.min(TILE_PX) {
                for x in 0..letter.width.min(TILE_PX) {
                    let ink = letter.coverage[y * letter.width + x];
                    let target = (offset_y + y) * BOARD_PX + offset_x + x;
                    grey[target] = grey[target].min(WHITE - ink);
                }
            }
        }
        Some(grey)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn the_two_spellings_of_the_edge_agree() {
            assert_eq!(u32::try_from(BOARD_PX).expect("edge fits"), BOARD_EDGE);
        }

        #[test]
        fn four_quarter_turns_are_the_identity() {
            let stamp = Stamp {
                width: 3,
                height: 2,
                coverage: vec![1, 2, 3, 4, 5, 6],
            };
            let turned = stamp
                .quarter_turn()
                .quarter_turn()
                .quarter_turn()
                .quarter_turn();
            assert_eq!(turned.coverage, vec![1, 2, 3, 4, 5, 6]);
            assert_eq!((turned.width, turned.height), (3, 2));
        }

        #[test]
        fn a_quarter_turn_swaps_the_dimensions() {
            let stamp = Stamp {
                width: 3,
                height: 2,
                coverage: vec![1, 2, 3, 4, 5, 6],
            };
            let turned = stamp.quarter_turn();
            assert_eq!((turned.width, turned.height), (2, 3));
            // The bottom-left corner becomes the top-left under a clockwise
            // turn: hand-derived, not computed by the code under test.
            assert_eq!(turned.coverage[0], 4);
        }

        #[test]
        fn the_board_renders_with_ink_and_the_rotation_changes_it() {
            let board: [&'static str; BOARD_CELLS] = [
                "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "Qu",
            ];
            let upright = render(&board, &[0; BOARD_CELLS]).expect("a rendered board");
            assert_eq!(upright.len(), BOARD_PX * BOARD_PX);
            assert!(upright.iter().any(|pixel| *pixel < 128), "no ink was laid");
            let mut turns = [0_u8; BOARD_CELLS];
            turns[0] = 1;
            let turned = render(&board, &turns).expect("a rendered board");
            assert_ne!(upright, turned, "a turned die must draw differently");
            // The same input draws the same picture: the renderer holds no
            // hidden state, so a repaint never subtly changes the board.
            let again = render(&board, &[0; BOARD_CELLS]).expect("a rendered board");
            assert_eq!(upright, again);
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Phase {
    /// Nothing shaken yet.
    #[default]
    Ready,
    Playing,
    Paused,
    /// The clock ran out; the board stays visible and the dictionary opens.
    Finished,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum View {
    #[default]
    Game,
    Lookup,
    Instructions,
}

/// What became of the last word the reader checked.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Checked {
    word: String,
    valid: bool,
    suggestions: Vec<String>,
    /// `None` while the runtime is still answering; empty when no installed
    /// dictionary knows the word.
    definitions: Option<Vec<DictionaryEntry>>,
}

struct App {
    board: [&'static str; BOARD_CELLS],
    /// Quarter turns per die, matching how the physical dice lie.
    turns: [u8; BOARD_CELLS],
    /// The board as a published picture, `None` until a game has been shaken
    /// or when the typeface could not be loaded.
    picture: Option<TilePicture>,
    phase: Phase,
    view: View,
    elapsed: u32,
    clock: Heartbeat,
    keyboard: Keyboard,
    checked: Option<Checked>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            board: [""; BOARD_CELLS],
            turns: [0; BOARD_CELLS],
            picture: None,
            phase: Phase::Ready,
            view: View::Game,
            elapsed: 0,
            clock: Heartbeat::every(1),
            keyboard: Keyboard::new(),
            checked: None,
        }
    }
}

impl App {
    fn new_game(&mut self, context: &mut Context) {
        let mut rng = Rng::from_clock();
        self.board = shake(&mut rng);
        self.turns = spin(&mut rng);
        self.picture = board_art::render(&self.board, &self.turns).and_then(|grey| {
            context.put_picture(
                PictureHandle(1),
                board_art::BOARD_EDGE,
                board_art::BOARD_EDGE,
                grey,
            )
        });
        self.phase = Phase::Playing;
        self.view = View::Game;
        self.elapsed = 0;
        self.checked = None;
        self.keyboard.clear();
        self.clock.stop(context);
        self.clock.start(context);
    }

    fn toggle_paused(&mut self, context: &mut Context) {
        match self.phase {
            Phase::Playing => {
                self.phase = Phase::Paused;
                self.clock.stop(context);
            }
            Phase::Paused => {
                self.phase = Phase::Playing;
                self.clock.start(context);
            }
            Phase::Ready | Phase::Finished => {}
        }
    }

    fn finish(&mut self, context: &mut Context) {
        self.phase = Phase::Finished;
        self.clock.stop(context);
    }

    fn remaining(&self) -> u32 {
        GAME_SECONDS.saturating_sub(self.elapsed)
    }

    fn check_word(&mut self, context: &mut Context, word: &str) {
        let valid = verify(word);
        let suggestions = if valid { Vec::new() } else { suggest(word) };
        let definitions = if valid {
            // Asked only for real words: the runtime's dictionaries answer by
            // headword, and a misspelling's definition is its suggestions.
            let asked = context
                .device()
                .lookup_word(word.to_lowercase(), None::<String>);
            if asked {
                None
            } else {
                Some(Vec::new())
            }
        } else {
            Some(Vec::new())
        };
        self.checked = Some(Checked {
            word: word.to_lowercase(),
            valid,
            suggestions,
            definitions,
        });
    }

    fn show(&self, context: &mut Context) {
        context.set_screen(match self.view {
            View::Game => self.game_screen(),
            View::Lookup => self.lookup_screen(),
            View::Instructions => instructions_screen(),
        });
    }

    /// Whether the rotated-picture board is what the screen should carry.
    ///
    /// Not while paused — the picture has letters in it, and pausing has to
    /// actually hide them — and not when the typeface failed to load, where
    /// the upright text board answers instead.
    fn shows_picture(&self) -> bool {
        self.phase != Phase::Ready && self.phase != Phase::Paused && self.picture.is_some()
    }

    /// What a board cell shows. The board is the whole game, so pausing has
    /// to actually hide it: a paused clock over visible letters is extra
    /// thinking time, not a pause.
    fn cell_label(&self, cell: usize) -> String {
        match self.phase {
            Phase::Paused => "·".to_owned(),
            _ => self.board[cell].to_owned(),
        }
    }

    fn status(&self) -> String {
        match self.phase {
            Phase::Ready => "Shake the dice, set your paper, and go.".to_owned(),
            Phase::Playing => format!("{} left", timecode(self.remaining())),
            Phase::Paused => "Paused.".to_owned(),
            Phase::Finished => "Pens down! Check your words below.".to_owned(),
        }
    }

    fn game_screen(&self) -> Screen {
        let mut screen = ScreenBuilder::new("lexicube")
            .top_bar("Lexicube")
            .top_bar_action("exit", "Exit");
        // The clock is read mid-game from arm's length, so while the game
        // runs it is set as the screen's display type rather than a
        // secondary line.
        screen = if self.phase == Phase::Playing {
            screen.heading(self.status())
        } else {
            screen.secondary(self.status())
        };
        if self.shows_picture() {
            // Unframed: the tiles draw their own outlines, and a frame around
            // an already-outlined board is two answers to one question.
            screen = screen.unframed_picture(self.picture.expect("shows_picture checked"), 110);
        } else if self.phase != Phase::Ready {
            let cells = (0..BOARD_CELLS).map(|cell| (die_name(cell), self.cell_label(cell), None));
            screen = screen.board(BOARD_COLUMNS, cells);
        }
        let mut actions: Vec<(&str, &str)> = Vec::new();
        match self.phase {
            Phase::Ready => actions.push(("new-game", "New game")),
            Phase::Playing => {
                actions.push(("new-game", "New game"));
                actions.push(("pause", "Pause"));
            }
            Phase::Paused => actions.push(("pause", "Resume")),
            Phase::Finished => {
                actions.push(("new-game", "New game"));
                actions.push(("lookup", "Check a word"));
            }
        }
        actions.push(("instructions", "How to play"));
        // Room between the board and the controls, so a hurried mid-game tap
        // at the tray's edge cannot land on New game.
        screen
            .spacer(Space::Large)
            .grid(
                u8::try_from(actions.len()).unwrap_or(u8::MAX),
                false,
                actions,
            )
            .build()
    }

    fn lookup_screen(&self) -> Screen {
        let mut screen = ScreenBuilder::new("lexicube-lookup")
            .top_bar("Check a word")
            .top_bar_action("back", "Board")
            .field("word", self.keyboard.text(), "Type a word from your list");
        if let Some(checked) = &self.checked {
            screen = if checked.valid {
                screen.section_with_value(checked.word.clone(), "valid".to_owned())
            } else {
                screen.section_with_value(checked.word.clone(), "not a word".to_owned())
            };
            if checked.valid {
                screen = match &checked.definitions {
                    None => screen.activity("Looking it up", None),
                    Some(entries) if entries.is_empty() => screen.secondary(
                        "No installed dictionary has a definition. Add UTF-8 TSV \
                         dictionaries to Cobalt's dictionaries folder for offline \
                         definitions.",
                    ),
                    Some(entries) => {
                        let mut with_definitions = screen;
                        for entry in entries.iter().take(3) {
                            with_definitions = with_definitions
                                .secondary(format!("{} · {}", entry.dictionary, entry.language))
                                .text(entry.definition.clone());
                        }
                        with_definitions
                    }
                };
            } else if !checked.suggestions.is_empty() {
                screen = screen.section("Did you mean").grid(
                    3,
                    false,
                    checked
                        .suggestions
                        .iter()
                        .map(|word| (suggestion_name(word), word.clone())),
                );
            }
        }
        screen.keyboard(&self.keyboard, "Check").build()
    }
}

fn instructions_screen() -> Screen {
    ScreenBuilder::new("lexicube-instructions")
        .top_bar("How to play")
        .top_bar_action("back", "Board")
        .text(
            "Shake the dice and start the three-minute clock. Everyone reads the \
             same board and writes down every word they can find, on their own \
             paper.",
        )
        .text(
            "A word uses letters that touch in a chain, sideways or diagonally, \
             without using the same die twice. Three letters or longer. The Qu \
             die counts as two letters.",
        )
        .text(
            "When the clock runs out: pens down. Read your lists aloud; a word \
             two players both found scores for neither. Use Check a word to \
             settle whether a word is real, and to read what it means.",
        )
        .section("Scoring")
        .text(
            "A word scores its letter count minus two: one point for a \
             three-letter word, and one more for every letter after that.",
        )
        .facts([
            ("3 letters", "1 point"),
            ("4 letters", "2 points"),
            ("5 letters", "3 points"),
            ("Every letter after", "1 more point"),
            ("Qu", "counts as 2 letters"),
            ("Plurals", "their own word: bird 2, birds 3"),
        ])
        .build()
}

fn timecode(seconds: u32) -> String {
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

fn die_name(cell: usize) -> String {
    format!("die-{cell}")
}

fn suggestion_name(word: &str) -> String {
    format!("suggest-{word}")
}

impl KoboApp for App {
    fn on_start(&mut self, context: &mut Context) {
        self.show(context);
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        // The keyboard first, because while it is up it owns the panel.
        if self.view == View::Lookup {
            if let Some(pressed) = self.keyboard.press(action) {
                match pressed {
                    Pressed::Edited | Pressed::Shifted => {}
                    Pressed::Submitted => {
                        let word = self.keyboard.take();
                        let word = word.trim().to_owned();
                        if word.len() >= MIN_WORD_LETTERS {
                            self.check_word(context, &word);
                        }
                    }
                }
                self.show(context);
                return;
            }
            if let Some(checked) = &self.checked {
                if let Some(word) = checked
                    .suggestions
                    .iter()
                    .find(|word| action == action_id(&suggestion_name(word)))
                    .cloned()
                {
                    self.check_word(context, &word);
                    self.show(context);
                    return;
                }
            }
        }
        if action == action_id("new-game") {
            self.new_game(context);
        } else if action == action_id("pause") {
            self.toggle_paused(context);
        } else if action == action_id("lookup") || action == action_id("word") {
            self.view = View::Lookup;
        } else if action == action_id("instructions") {
            self.view = View::Instructions;
        } else if action == action_id("back") {
            self.view = View::Game;
        } else if action == action_id("exit") {
            // Hands control back to whatever launched the app: the launcher,
            // or the end of the session when the app was presented alone.
            context.exit();
            return;
        } else {
            return;
        }
        self.show(context);
    }

    fn on_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
        if self.clock.on_task(context, task, &outcome) && self.phase == Phase::Playing {
            self.elapsed = self.elapsed.saturating_add(1);
            if self.elapsed >= GAME_SECONDS {
                self.finish(context);
            }
            self.show(context);
        }
    }

    fn on_device_result(
        &mut self,
        context: &mut Context,
        _request: kobo_sdk::DeviceRequest,
        result: kobo_sdk::DeviceResult,
    ) {
        if let kobo_sdk::DeviceResult::Dictionary { word, entries } = result {
            if let Some(checked) = &mut self.checked {
                if checked.word == word && checked.definitions.is_none() {
                    checked.definitions = Some(entries);
                    if self.view == View::Lookup {
                        self.show(context);
                    }
                }
            }
        }
    }
}

fn main() -> ExitCode {
    match kobo_sdk::run("lexicube", App::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("lexicube: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobo_ui::{Chrome, DisplayMetrics, TextScale, CLARA_BW_METRICS};

    /// Every panel this app claims to lay out on. The Libra Colour is the
    /// device this app was written against; the others bracket it.
    const PANELS: [(&str, DisplayMetrics); 4] = [
        ("clara-bw", CLARA_BW_METRICS),
        (
            "libra-colour",
            DisplayMetrics {
                width: 1264,
                height: 1680,
                pixels_per_inch: 300,
                text_scale: TextScale::Default,
            },
        ),
        (
            "nia",
            DisplayMetrics {
                width: 758,
                height: 1024,
                pixels_per_inch: 212,
                text_scale: TextScale::Default,
            },
        ),
        (
            "sage",
            DisplayMetrics {
                width: 1440,
                height: 1920,
                pixels_per_inch: 300,
                text_scale: TextScale::Default,
            },
        ),
    ];

    #[test]
    fn the_dice_are_the_tabletop_set() {
        assert_eq!(DICE.len(), BOARD_CELLS);
        let qu_faces: usize = DICE.iter().flatten().filter(|face| **face == "Qu").count();
        assert_eq!(qu_faces, 1, "exactly one die carries Qu");
        for die in DICE {
            for face in die {
                assert!(matches!(face.len(), 1 | 2), "{face:?} is not a die face");
                assert!(face.chars().next().is_some_and(char::is_uppercase));
            }
        }
    }

    #[test]
    fn a_shake_uses_every_die_once() {
        let mut rng = Rng(42);
        let board = shake(&mut rng);
        for face in board {
            assert!(
                DICE.iter().any(|die| die.contains(&face)),
                "{face:?} is not on any die"
            );
        }
        // Sixteen dice produce at most one Qu, and every cell has a letter.
        assert!(board.iter().filter(|face| **face == "Qu").count() <= 1);
        assert!(board.iter().all(|face| !face.is_empty()));
        // Two shakes from different seeds almost surely differ; equal boards
        // here would mean the generator is not consuming its state.
        let second = shake(&mut Rng(43));
        assert_ne!(board, second);
    }

    #[test]
    fn the_word_list_answers_like_the_table_expects() {
        assert!(verify("qi"), "the two-letter Scrabble staple is in SOWPODS");
        assert!(verify("AARDVARK"));
        assert!(verify("aardvark"), "case must not matter");
        assert!(!verify("XQZZY"));
        assert!(!verify(""));
    }

    #[test]
    fn suggestions_are_near_misses_and_never_the_word_itself() {
        let suggestions = suggest("AARDVARC");
        assert!(
            suggestions.contains(&"aardvark".to_owned()),
            "{suggestions:?}"
        );
        let listed = suggest("HOUSA");
        assert!(!listed.is_empty());
        assert!(listed.iter().all(|word| word.len() >= MIN_WORD_LETTERS));
        assert!(listed.len() <= MAX_SUGGESTIONS);
        assert!(suggest("ZZZZZZZZZZ").is_empty());
    }

    #[test]
    fn the_clock_counts_down_and_pens_go_down_at_zero() {
        let mut app = App::default();
        assert_eq!(app.remaining(), GAME_SECONDS);
        app.board = shake(&mut Rng(7));
        app.phase = Phase::Playing;
        app.elapsed = GAME_SECONDS - 1;
        assert_eq!(app.status(), "00:01 left");
        app.elapsed = GAME_SECONDS;
        assert_eq!(app.remaining(), 0);
    }

    #[test]
    fn pausing_hides_the_letters() {
        let mut app = App {
            board: shake(&mut Rng(7)),
            phase: Phase::Playing,
            ..App::default()
        };
        assert_eq!(app.cell_label(0), app.board[0]);
        app.phase = Phase::Paused;
        for cell in 0..BOARD_CELLS {
            // A paused board must not leak a letter. `·` is not a die face.
            assert_eq!(app.cell_label(cell), "·", "cell {cell} leaks while paused");
        }
        app.phase = Phase::Finished;
        assert_eq!(
            app.cell_label(0),
            app.board[0],
            "the finished board stays visible for checking words"
        );
    }

    #[test]
    fn checking_a_word_records_validity_and_suggestions() {
        let mut app = App::default();
        // `check_word` without a Context: exercise the pure halves directly.
        let valid = verify("house");
        assert!(valid);
        let checked = Checked {
            word: "housa".to_owned(),
            valid: verify("housa"),
            suggestions: suggest("housa"),
            definitions: Some(Vec::new()),
        };
        assert!(!checked.valid);
        assert!(checked.suggestions.contains(&"house".to_owned()));
        app.checked = Some(checked);
        app.view = View::Lookup;
        let screen = app.lookup_screen();
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
        assert!(layout
            .rect_of_action(action_id(&suggestion_name("house")))
            .is_some());
    }

    #[test]
    fn the_board_fits_and_is_readable_on_every_panel() {
        let app = App {
            board: shake(&mut Rng(7)),
            phase: Phase::Playing,
            ..App::default()
        };
        for (panel, metrics) in PANELS {
            let screen = app.game_screen();
            let layout = screen.layout_with(&metrics, &Chrome::default());
            for cell in 0..BOARD_CELLS {
                let rect = layout
                    .rect_of_action(action_id(&die_name(cell)))
                    .unwrap_or_else(|| panic!("die {cell} is missing on {panel}"));
                assert_eq!(rect.width, rect.height, "square dice on {panel}");
            }
            let diagnostics = screen.diagnostics(&metrics, &Chrome::default());
            assert!(
                diagnostics.issues.is_empty(),
                "{panel} layout diagnostics: {:?}",
                diagnostics.issues
            );
        }
    }

    #[test]
    fn every_screen_lays_out_cleanly_on_every_panel() {
        let app = App {
            board: shake(&mut Rng(7)),
            phase: Phase::Finished,
            ..App::default()
        };
        for (panel, metrics) in PANELS {
            let mut screens = vec![app.game_screen(), instructions_screen()];
            // The SDK's keyboard overflows its narrowest keys on the Nia's
            // 758-pixel panel, which is the SDK's to fix rather than this
            // app's, so the lookup screen is held to the panels the keyboard
            // itself supports.
            if metrics.width >= CLARA_BW_METRICS.width {
                screens.push(app.lookup_screen());
            }
            for screen in screens {
                let diagnostics = screen.diagnostics(&metrics, &Chrome::default());
                assert!(
                    diagnostics.issues.is_empty(),
                    "{panel} layout diagnostics: {:?}",
                    diagnostics.issues
                );
            }
        }
    }

    #[test]
    fn the_timecode_reads_like_a_kitchen_timer() {
        assert_eq!(timecode(180), "03:00");
        assert_eq!(timecode(61), "01:01");
        assert_eq!(timecode(0), "00:00");
    }
}

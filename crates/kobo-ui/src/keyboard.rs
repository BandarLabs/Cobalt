//! The keys of the on-screen keyboard, in the one place they are written down.
//!
//! From protocol 16 the runtime draws the keyboard for an application that
//! asks for text, so a key added here reaches every such application with the
//! next platform release. The few screens that still draw their own keys, a
//! terminal that sends each one the moment it is tapped and a game keypad,
//! read their rows from here too. Paperterm once kept a copy of the symbol
//! rows and was missing `_` and `#` from the day they were added to the
//! original, which is the whole case for there being exactly one.
//!
//! ## Why the keys are addressed by position
//!
//! A key's identity is where it is, not what it types. `Shift` and the symbol
//! layer change every label on the panel but no cell moves, so a tap resolves
//! to a position and this module decides what that position means right now.
//! Naming keys after their characters instead would need a different action id
//! per layer, which is four times the identifiers and one more thing to get
//! out of step with what is drawn. It is also what lets `kobo drive type` find
//! a key whether the runtime or an application drew it.
//!
//! ## Why there is no cursor
//!
//! Text is only ever appended to or deleted from the end. Moving a caret needs
//! a caret to be visible, which needs a repaint per tap on a panel that takes
//! tens of milliseconds a repaint, and the payoff is editing in the middle of
//! a search query.

use crate::{ActionId, Cell, FieldKind, Glyph};

/// The longest string the keyboard will accumulate, in characters.
///
/// A bound exists because the text is redrawn on the panel on every keystroke,
/// and an unbounded string would eventually take longer to lay out than to
/// type. It is generous enough for a search query, a chat message or an
/// access token.
pub const MAX_TEXT: usize = 512;

/// Letters, in the arrangement every reader already knows.
const LETTERS: [&str; 3] = ["qwertyuiop", "asdfghjkl", "zxcvbnm"];

/// The second layer. Chosen for what these applications actually need: digits,
/// the punctuation of a sentence, and the handful of symbols that appear in
/// search queries and addresses.
///
/// The hash is on it because applications on this device ask people to type
/// one. The list of things to do groups itself by whatever tags the owner
/// wrote into an item, and for as long as this layer had no `#` on it, that
/// was a feature only reachable by somebody who had typed their list
/// somewhere else and imported it.
///
/// The underscore is on it because access tokens contain one. Every Lichess
/// token starts `lip_`, and a token the keyboard cannot type is a token that
/// can only be installed from a computer. It sits at the end of its row so
/// no other key moves.
const SYMBOLS: [&str; 3] = ["1234567890", "-/:;()&@\"_", ".,?!'+=#"];

/// What the symbol layer shows while shift is held.
///
/// The symbol layer has no room left, and a Wi-Fi password, a web address or
/// a shell command can need any printable character, so the rest are under
/// shift, where a physical keyboard keeps them. The top row is what shift
/// makes of each digit on an American keyboard, so shift then 5 is a percent
/// sign, as anyone who has typed one on a computer would guess. The second
/// row is the brackets and marks no digit carries. The bottom row is the same
/// as unshifted, so the punctuation either side of a symbol stays put.
const SHIFTED_SYMBOLS: [&str; 3] = ["!@#$%^&*()", "[]{}<>\\|~`", ".,?!'+=#"];

pub const SHIFT: &str = "kb.shift";
pub const LAYER: &str = "kb.layer";
pub const SPACE: &str = "kb.space";
pub const BACKSPACE: &str = "kb.backspace";
pub const ENTER: &str = "kb.enter";
/// The way out of a field, drawn under the keys.
pub const CANCEL: &str = "kb.cancel";
/// The three keys an address keyboard has where the others have a space bar.
pub const AT: &str = "kb.at";
pub const SLASH: &str = "kb.slash";
pub const DOT: &str = "kb.dot";

/// The action name of the character key at `row` and `column`.
#[must_use]
pub fn key_name(row: usize, column: usize) -> String {
    format!("kb.r{row}c{column}")
}

/// What a tap on the keyboard meant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Pressed {
    /// The text changed. The caller should repaint.
    Edited,
    /// A modifier changed. The labels changed, so the caller should repaint,
    /// but the text did not.
    Shifted,
    /// The reader asked to accept what they typed.
    Submitted,
}

/// Which set of characters the keys are currently showing.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Layer {
    #[default]
    Letters,
    Symbols,
}

/// One key as it is drawn right now: the action it sends and what it says.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Key {
    pub name: String,
    pub label: String,
    pub glyph: Option<Glyph>,
    /// Drawn inverted, for a shift that is held.
    pub selected: bool,
}

impl Key {
    fn new(name: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            label: label.into(),
            glyph: None,
            selected: false,
        }
    }

    /// The grid cell this key is drawn as.
    #[must_use]
    pub fn cell(&self) -> Cell {
        let cell = Cell::new(ActionId::from_name(&self.name), self.label.clone())
            .with_selected(self.selected);
        match self.glyph {
            Some(glyph) => cell.with_glyph(glyph),
            None => cell,
        }
    }
}

/// An on-screen keyboard and the text typed on it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Keyboard {
    text: String,
    shift: bool,
    layer: Layer,
    kind: FieldKind,
}

impl Keyboard {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts on the digits rather than the letters.
    ///
    /// For a field whose answer is a number, such as an account's numeric user
    /// ID. Opening those on the letters made the reader find and tap `?123`
    /// before the first keystroke that could mean anything, and a reader who
    /// did not know the key was there saw no way to type a digit at all.
    #[must_use]
    pub fn numeric() -> Self {
        Self::for_kind(FieldKind::Number)
    }

    /// The keyboard a field of this kind opens on.
    ///
    /// A number opens on the digits. An address opens on the letters with
    /// `@`, `/` and `.` where the space bar would be, because an address never
    /// has a space in it and those three are in nearly every one. A secret
    /// opens on the letters like ordinary text: most tokens are mostly
    /// letters, and nothing about a password says which characters it uses.
    #[must_use]
    pub fn for_kind(kind: FieldKind) -> Self {
        Self {
            layer: if kind == FieldKind::Number {
                Layer::Symbols
            } else {
                Layer::Letters
            },
            kind,
            ..Self::default()
        }
    }

    /// Starts with text already typed, truncated to [`MAX_TEXT`].
    #[must_use]
    pub fn with_text(text: impl Into<String>) -> Self {
        Self::new().typed(text)
    }

    /// The same keyboard with `text` appended, truncated to [`MAX_TEXT`].
    #[must_use]
    pub fn typed(mut self, text: impl Into<String>) -> Self {
        for character in text.into().chars() {
            self.push(character);
        }
        self
    }

    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// How many characters have been typed, which is what a reader comparing
    /// a token against the length a service printed wants to know.
    #[must_use]
    pub fn count(&self) -> usize {
        self.text.chars().count()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    #[must_use]
    pub fn layer(&self) -> Layer {
        self.layer
    }

    #[must_use]
    pub fn kind(&self) -> FieldKind {
        self.kind
    }

    #[must_use]
    pub fn is_shifted(&self) -> bool {
        self.shift
    }

    pub fn clear(&mut self) {
        self.text.clear();
    }

    /// Takes the text, leaving the keyboard empty and ready for the next entry.
    pub fn take(&mut self) -> String {
        std::mem::take(&mut self.text)
    }

    /// Applies `action` if it belongs to the keyboard.
    ///
    /// Returns `None` for anything else, so a caller can pass every action it
    /// receives here first and handle its own afterwards without having to
    /// know which identifiers the keyboard claimed.
    pub fn press(&mut self, action: ActionId) -> Option<Pressed> {
        if action == ActionId::from_name(SHIFT) {
            self.shift = !self.shift;
            return Some(Pressed::Shifted);
        }
        if action == ActionId::from_name(LAYER) {
            self.layer = match self.layer {
                Layer::Letters => Layer::Symbols,
                Layer::Symbols => Layer::Letters,
            };
            // A shift is for the next key on the layer it was pressed on.
            // Carried across, a shift pressed for a capital and abandoned for
            // a digit would open the symbols on their shifted page.
            self.shift = false;
            return Some(Pressed::Shifted);
        }
        if action == ActionId::from_name(BACKSPACE) {
            // By character, not by byte. Truncating a multi-byte character
            // would leave the string invalid, and this is the one operation a
            // reader repeats without looking.
            self.text.pop();
            return Some(Pressed::Edited);
        }
        if action == ActionId::from_name(ENTER) {
            return Some(Pressed::Submitted);
        }
        let character = self.character_at(action)?;
        self.push(character);
        // A shift applies to exactly one letter, as on every phone. Leaving it
        // latched would mean reading the panel to find out what the next key
        // will do.
        if character != ' ' {
            self.shift = false;
        }
        Some(Pressed::Edited)
    }

    /// The character this key would type right now, without typing it.
    ///
    /// A terminal needs this: a keystroke there is sent to a program the
    /// instant it happens rather than accumulated into a field, so the layout
    /// and the modifier state are wanted but the text buffer is not.
    #[must_use]
    pub fn resolves(&self, action: ActionId) -> Option<char> {
        self.character_at(action)
    }

    fn character_at(&self, action: ActionId) -> Option<char> {
        if self.kind == FieldKind::Address {
            for (name, character) in [(AT, '@'), (SLASH, '/'), (DOT, '.')] {
                if action == ActionId::from_name(name) {
                    return Some(character);
                }
            }
        } else if action == ActionId::from_name(SPACE) {
            return Some(' ');
        }
        for (row, characters) in self.rows().iter().enumerate() {
            for (column, character) in characters.chars().enumerate() {
                if action == ActionId::from_name(&key_name(row, column)) {
                    return Some(if self.shift {
                        character.to_ascii_uppercase()
                    } else {
                        character
                    });
                }
            }
        }
        None
    }

    /// The characters the three rows of keys show right now, top row first.
    ///
    /// Public for an application that arranges its own keys around these, as
    /// Paperterm does. A copy of the rows kept anywhere else falls behind the
    /// moment a character is added here, and a key that is decoded but never
    /// drawn is a character nobody can type.
    #[must_use]
    pub fn rows(&self) -> [&'static str; 3] {
        match (self.layer, self.shift) {
            (Layer::Letters, _) => LETTERS,
            (Layer::Symbols, false) => SYMBOLS,
            (Layer::Symbols, true) => SHIFTED_SYMBOLS,
        }
    }

    /// Every key as it is drawn right now, as four rows top first: three of
    /// characters, then the row that changes layer and submits.
    ///
    /// Shift and backspace sit at either end of the bottom character row,
    /// where the reader's thumbs already are and where they are on every
    /// phone. The rows are drawn as separate grids rather than one, because
    /// the alphabet's rows are ten, nine and nine keys wide and a single grid
    /// would either leave a ragged edge or need padding cells that look like
    /// keys but do nothing.
    #[must_use]
    pub fn keys(&self, submit: &str) -> [Vec<Key>; 4] {
        let rows = self.rows();
        let mut drawn: [Vec<Key>; 4] = Default::default();
        for (index, characters) in rows.iter().enumerate() {
            let keys = &mut drawn[index];
            if index == 2 {
                keys.push(Key {
                    glyph: Some(Glyph::Shift),
                    selected: self.shift,
                    ..Key::new(SHIFT, if self.shift { "Shift on" } else { "Shift" })
                });
            }
            for (column, character) in characters.chars().enumerate() {
                let label = if self.shift {
                    character.to_ascii_uppercase()
                } else {
                    character
                };
                keys.push(Key::new(key_name(index, column), label.to_string()));
            }
            if index == 2 {
                keys.push(Key {
                    glyph: Some(Glyph::Backspace),
                    ..Key::new(BACKSPACE, "Delete character")
                });
            }
        }
        let layer = match self.layer {
            Layer::Letters => "?123",
            Layer::Symbols => "abc",
        };
        drawn[3].push(Key::new(LAYER, layer));
        if self.kind == FieldKind::Address {
            for (name, label) in [(AT, "@"), (SLASH, "/"), (DOT, ".")] {
                drawn[3].push(Key::new(name, label));
            }
        } else {
            drawn[3].push(Key::new(SPACE, "space"));
        }
        drawn[3].push(Key::new(ENTER, submit));
        drawn
    }

    fn push(&mut self, character: char) {
        // Counted in characters rather than bytes so the limit means the same
        // thing whatever alphabet the reader is using.
        if self.count() < MAX_TEXT {
            self.text.push(character);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Keyboard, Layer, Pressed, MAX_TEXT};
    use crate::{ActionId, FieldKind};

    /// The pages a reader can reach from a fresh keyboard, as the taps that
    /// reach them.
    const PAGES: [&[&str]; 4] = [&[], &["kb.shift"], &["kb.layer"], &["kb.layer", "kb.shift"]];

    fn tap(keyboard: &mut Keyboard, name: &str) -> Option<Pressed> {
        keyboard.press(ActionId::from_name(name))
    }

    #[test]
    fn every_printable_character_can_be_typed_on_the_keyboard() {
        // A control that asks for a character the keyboard cannot produce is
        // a control nobody can use. They went missing one at a time: the hash
        // a todo tag starts with, the underscore in every Lichess token, then
        // the percent sign a reader went looking for. A Wi-Fi password may be
        // any printable character and so may a line typed into a shell, so
        // this checks all of them rather than whichever was missed last.
        for kind in [
            FieldKind::Text,
            FieldKind::Secret,
            FieldKind::Number,
            FieldKind::Address,
        ] {
            let mut typed = String::new();
            for page in PAGES {
                let mut keyboard = Keyboard::for_kind(kind);
                for name in page {
                    tap(&mut keyboard, name);
                }
                for row in keyboard.keys("Done") {
                    for key in row {
                        let mut pressed = keyboard.clone();
                        if pressed.press(ActionId::from_name(&key.name)) == Some(Pressed::Edited) {
                            typed.push_str(pressed.text());
                        }
                    }
                }
            }
            for wanted in '!'..='~' {
                assert!(
                    typed.contains(wanted),
                    "a {kind:?} keyboard cannot type {wanted:?}"
                );
            }
        }
    }

    #[test]
    fn each_kind_opens_on_the_page_its_answer_starts_on() {
        assert_eq!(Keyboard::for_kind(FieldKind::Text).layer(), Layer::Letters);
        assert_eq!(
            Keyboard::for_kind(FieldKind::Secret).layer(),
            Layer::Letters
        );
        assert_eq!(
            Keyboard::for_kind(FieldKind::Number).layer(),
            Layer::Symbols
        );
        assert_eq!(Keyboard::numeric().layer(), Layer::Symbols);
        let address = Keyboard::for_kind(FieldKind::Address);
        assert_eq!(address.layer(), Layer::Letters);
        let bottom: Vec<_> = address.keys("Go")[3]
            .iter()
            .map(|key| key.label.clone())
            .collect();
        assert_eq!(bottom, ["?123", "@", "/", ".", "Go"]);
        let text: Vec<_> = Keyboard::new().keys("Add")[3]
            .iter()
            .map(|key| key.label.clone())
            .collect();
        assert_eq!(text, ["?123", "space", "Add"]);
    }

    #[test]
    fn an_address_types_its_marks_from_the_letters_and_has_no_space_to_tap() {
        // An address with a space in it is not an address, and the three
        // marks every address has were a layer away.
        let mut keyboard = Keyboard::for_kind(FieldKind::Address);
        for name in ["kb.r0c0", "kb.at", "kb.r0c1", "kb.dot", "kb.slash"] {
            tap(&mut keyboard, name);
        }
        assert_eq!(keyboard.text(), "q@w./");
        assert_eq!(tap(&mut keyboard, "kb.space"), None);
        assert_eq!(tap(&mut Keyboard::new(), "kb.at"), None);
    }

    #[test]
    fn shift_on_the_symbol_layer_types_what_shift_on_a_number_key_always_has() {
        // Shift and 5 is a percent sign on every physical keyboard, and it
        // releases after one symbol exactly as it does after one capital.
        let mut keyboard = Keyboard::new();
        tap(&mut keyboard, "kb.layer");
        tap(&mut keyboard, "kb.shift");
        tap(&mut keyboard, "kb.r0c4");
        tap(&mut keyboard, "kb.r0c4");
        assert_eq!(keyboard.text(), "%5");
    }

    #[test]
    fn changing_layer_releases_a_shift_meant_for_the_other_one() {
        let mut keyboard = Keyboard::new();
        tap(&mut keyboard, "kb.shift");
        tap(&mut keyboard, "kb.layer");
        assert!(!keyboard.is_shifted());
        tap(&mut keyboard, "kb.r0c0");
        assert_eq!(keyboard.text(), "1");
    }

    #[test]
    fn typing_a_word_produces_that_word() {
        let mut keyboard = Keyboard::new();
        for name in ["kb.r0c0", "kb.r1c0", "kb.r0c2"] {
            tap(&mut keyboard, name);
        }
        assert_eq!(keyboard.text(), "qae");
    }

    #[test]
    fn shift_applies_to_one_letter_and_then_releases() {
        // A latched shift would mean reading the panel before every key to
        // find out what it will do, which is exactly the cost this device
        // cannot afford.
        let mut keyboard = Keyboard::new();
        assert_eq!(tap(&mut keyboard, "kb.shift"), Some(Pressed::Shifted));
        tap(&mut keyboard, "kb.r0c0");
        tap(&mut keyboard, "kb.r0c1");
        assert_eq!(keyboard.text(), "Qw");
        assert!(!keyboard.is_shifted());
    }

    #[test]
    fn the_same_key_types_a_different_character_on_the_symbol_layer() {
        // The whole reason keys are addressed by position: the cell does not
        // move, so the action id must not change either.
        let mut keyboard = Keyboard::new();
        tap(&mut keyboard, "kb.r0c0");
        tap(&mut keyboard, "kb.layer");
        assert_eq!(keyboard.layer(), Layer::Symbols);
        tap(&mut keyboard, "kb.r0c0");
        assert_eq!(keyboard.text(), "q1");
    }

    #[test]
    fn backspace_removes_a_whole_character_rather_than_a_byte() {
        let mut keyboard = Keyboard::with_text("café");
        tap(&mut keyboard, "kb.backspace");
        assert_eq!(keyboard.text(), "caf");
        let mut empty = Keyboard::new();
        assert_eq!(tap(&mut empty, "kb.backspace"), Some(Pressed::Edited));
        assert!(empty.is_empty());
    }

    #[test]
    fn enter_submits_without_changing_the_text() {
        let mut keyboard = Keyboard::with_text("dickens");
        assert_eq!(tap(&mut keyboard, "kb.enter"), Some(Pressed::Submitted));
        assert_eq!(keyboard.text(), "dickens");
    }

    #[test]
    fn an_action_the_keyboard_does_not_own_is_handed_back() {
        // Callers pass every action here first, so claiming one that belongs
        // to them would silently break their own buttons.
        let mut keyboard = Keyboard::new();
        assert_eq!(tap(&mut keyboard, "search"), None);
        assert_eq!(keyboard.press(ActionId::BACK), None);
    }

    #[test]
    fn text_stops_growing_at_the_limit() {
        let mut keyboard = Keyboard::with_text("x".repeat(MAX_TEXT + 50));
        assert_eq!(keyboard.count(), MAX_TEXT);
        tap(&mut keyboard, "kb.r0c0");
        assert_eq!(keyboard.count(), MAX_TEXT);
        assert_eq!(keyboard.take().chars().count(), MAX_TEXT);
        assert!(keyboard.is_empty());
    }
}

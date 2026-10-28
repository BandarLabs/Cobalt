//! Typing, for the few things that genuinely cannot be tapped.
//!
//! This is a composite, not a primitive. The protocol has no keyboard node,
//! the layout engine has never heard of one, and the renderer draws it as what
//! it is: four grids of tappable cells. That was a deliberate decision, a
//! keyboard, a calculator, a board and a colour picker are all the same shape,
//! and adding a node for each would mean adding it to the wire format, the
//! layout engine, the renderer and the hit test before anybody could use it.
//! Everything here is built out of [`ScreenBuilder::grid`].
//!
//! ## Why the keys are addressed by position
//!
//! A key's identity is where it is, not what it types. `Shift` and the symbol
//! layer change every label on the panel but no cell moves, so a tap resolves
//! to a position and this module decides what that position means right now.
//! Naming keys after their characters instead would need a different action id
//! per layer, which is four times the identifiers and one more thing to get
//! out of step with what is drawn.
//!
//! ## Why there is no cursor
//!
//! Text is only ever appended to or deleted from the end. Moving a caret needs
//! a caret to be visible, which needs a repaint per tap on a panel that takes
//! tens of milliseconds a repaint, and the payoff is editing in the middle of
//! a search query. Applications that want prose should be asking a question
//! with [`ScreenBuilder::choose`] instead.

use crate::ScreenBuilder;
use kobo_ui::ActionId;

/// The longest string the keyboard will accumulate.
///
/// A bound exists because the text is redrawn on the panel on every keystroke,
/// and an unbounded string would eventually take longer to lay out than to
/// type. It is generous enough for a search query or a chat message.
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

/// An on-screen keyboard and the text typed on it.
///
/// The application owns one of these, hands it to
/// [`ScreenBuilder::keyboard`] when it draws, and gives it every action it
/// does not recognise.
#[derive(Clone, Debug, Default)]
pub struct Keyboard {
    text: String,
    shift: bool,
    layer: Layer,
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
        Self {
            layer: Layer::Symbols,
            ..Self::default()
        }
    }

    /// Starts with text already typed, truncated to [`MAX_TEXT`].
    #[must_use]
    pub fn with_text(text: impl Into<String>) -> Self {
        let mut keyboard = Self::default();
        for character in text.into().chars() {
            keyboard.push(character);
        }
        keyboard
    }

    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
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
    /// Returns `None` for anything else, so an application can pass every
    /// action it receives here first and handle its own afterwards without
    /// having to know which identifiers the keyboard claimed.
    pub fn press(&mut self, action: ActionId) -> Option<Pressed> {
        if action == crate::action_id(SHIFT) {
            self.shift = !self.shift;
            return Some(Pressed::Shifted);
        }
        if action == crate::action_id(LAYER) {
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
        if action == crate::action_id(SPACE) {
            self.push(' ');
            return Some(Pressed::Edited);
        }
        if action == crate::action_id(BACKSPACE) {
            // By character, not by byte. Truncating a multi-byte character
            // would leave the string invalid, and this is the one operation a
            // reader repeats without looking.
            self.text.pop();
            return Some(Pressed::Edited);
        }
        if action == crate::action_id(ENTER) {
            return Some(Pressed::Submitted);
        }
        let character = self.character_at(action)?;
        self.push(character);
        // A shift applies to exactly one letter, as on every phone. Leaving it
        // latched would mean reading the panel to find out what the next key
        // will do.
        self.shift = false;
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
        let rows = self.rows();
        for (row, characters) in rows.iter().enumerate() {
            for (column, character) in characters.chars().enumerate() {
                if action == crate::action_id(&key_name(row, column)) {
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

    fn push(&mut self, character: char) {
        // Counted in characters rather than bytes so the limit means the same
        // thing whatever alphabet the reader is using.
        if self.text.chars().count() < MAX_TEXT {
            self.text.push(character);
        }
    }
}

/// What a tap did to a [`TextEntry`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Typing {
    /// The field opened, changed, or a modifier moved. Repaint.
    Changed,
    /// The reader accepted what they typed. The text comes with it, and the
    /// field is closed and empty again, so the next entry starts clean.
    Submitted(String),
    /// The reader backed out. Nothing was entered.
    Cancelled,
}

/// A field that raises the keyboard when it is tapped, and puts it away again.
///
/// This exists because "tap the row, get a keyboard, get the text back" was
/// being written out by hand in every application, and every application got a
/// slightly different one. The row an application draws with
/// [`ScreenBuilder::or_type`] emits an action like any other; binding that
/// action here is what makes the row open the keyboard, rather than each
/// author remembering to switch a view enum.
///
/// The application still owns its screens: this decides *whether* the keyboard
/// is showing, and [`ScreenBuilder::text_entry`] draws it.
#[derive(Clone, Debug, Default)]
pub struct TextEntry {
    keyboard: Keyboard,
    open: bool,
    opens_on: Option<ActionId>,
    numeric: bool,
}

impl TextEntry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Binds the action that opens this field.
    ///
    /// Named rather than numbered, and resolved the same way every other
    /// action name is, so it is the same string passed to
    /// [`ScreenBuilder::or_type`].
    #[must_use]
    pub fn opened_by(mut self, name: &str) -> Self {
        self.opens_on = Some(crate::action_id(name));
        self
    }

    /// Opens this field on the digits every time, for a value that is a
    /// number. The letters stay one tap away on `abc`, because an ID pasted
    /// from a page can still carry a stray letter the reader needs to match.
    #[must_use]
    pub const fn numeric(mut self) -> Self {
        self.numeric = true;
        self
    }

    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.open
    }

    #[must_use]
    pub fn text(&self) -> &str {
        self.keyboard.text()
    }

    #[must_use]
    pub const fn keyboard(&self) -> &Keyboard {
        &self.keyboard
    }

    /// Opens the field with nothing in it, unshifted, on the letters or, for
    /// a [`Self::numeric`] field, on the digits.
    ///
    /// A new keyboard rather than an emptied one. Emptying kept the layer and
    /// the shift the last field was left on, so a field opened after a number
    /// had been typed into another came up on the symbols.
    pub fn open(&mut self) {
        self.keyboard = self.fresh();
        self.open = true;
    }

    /// Opens the field with `text` already in it, for editing something.
    pub fn open_with(&mut self, text: impl Into<String>) {
        let mut keyboard = self.fresh();
        for character in text.into().chars() {
            keyboard.push(character);
        }
        self.keyboard = keyboard;
        self.open = true;
    }

    /// Closes the field and throws away anything typed.
    pub fn close(&mut self) {
        self.keyboard = self.fresh();
        self.open = false;
    }

    fn fresh(&self) -> Keyboard {
        if self.numeric {
            Keyboard::numeric()
        } else {
            Keyboard::new()
        }
    }

    /// Applies `action` if it belongs to this field.
    ///
    /// Returns `None` for anything else, so an application passes every action
    /// here first and handles its own afterwards. While the field is open it
    /// claims the cancel action too, because a keyboard covering the panel is
    /// modal whether or not the author thought of it that way.
    pub fn handle(&mut self, action: ActionId) -> Option<Typing> {
        self.handle_text(action, true)
    }

    /// Handle private values without trimming meaningful password whitespace.
    /// Empty input cancels; a nonempty value is returned exactly as entered.
    pub fn handle_verbatim(&mut self, action: ActionId) -> Option<Typing> {
        self.handle_text(action, false)
    }

    fn handle_text(&mut self, action: ActionId, trim: bool) -> Option<Typing> {
        if !self.open {
            if self.opens_on == Some(action) {
                self.open();
                return Some(Typing::Changed);
            }
            return None;
        }
        if action == crate::action_id(CANCEL) {
            self.close();
            return Some(Typing::Cancelled);
        }
        match self.keyboard.press(action)? {
            Pressed::Edited | Pressed::Shifted => Some(Typing::Changed),
            Pressed::Submitted => {
                let text = self.keyboard.take();
                self.open = false;
                // Whitespace only is nothing. Returning it would make every
                // caller check, and half of them would forget.
                let text = if trim { text.trim().to_string() } else { text };
                if text.is_empty() {
                    Some(Typing::Cancelled)
                } else {
                    Some(Typing::Submitted(text))
                }
            }
        }
    }
}

const CANCEL: &str = "kb.cancel";

pub(crate) const SHIFT: &str = "kb.shift";
pub(crate) const LAYER: &str = "kb.layer";
pub(crate) const SPACE: &str = "kb.space";
pub(crate) const BACKSPACE: &str = "kb.backspace";
pub(crate) const ENTER: &str = "kb.enter";

pub(crate) fn key_name(row: usize, column: usize) -> String {
    format!("kb.r{row}c{column}")
}

impl ScreenBuilder {
    /// Draws `keyboard`, and the text typed on it so far.
    ///
    /// The rows are separate grids rather than one, because the alphabet's
    /// rows are ten, nine and nine keys wide and a single grid would either
    /// leave a ragged edge or need padding cells that look like keys but do
    /// nothing.
    #[must_use]
    pub fn keyboard(self, keyboard: &Keyboard, submit: &str) -> Self {
        let rows = keyboard.rows();
        // The keys go to the foot of the panel wherever they are added, which
        // is where thumbs are and where every other platform puts them. In
        // flow they landed wherever the prompt above them ended: the todo
        // application's compose screen drew its keyboard across the middle of
        // the panel with a third of a page of paper under it. A second `fill`
        // above this one, as `terminal_keys` has, changes nothing, because a
        // fill only ever pushes down.
        let mut screen = self.fill();
        for (index, characters) in rows.iter().enumerate() {
            let mut cells = Vec::new();
            // Shift and backspace sit on the bottom letter row, where the
            // reader's thumbs already are, and where they are on every phone.
            if index == 2 {
                cells.push((
                    SHIFT.to_string(),
                    if keyboard.is_shifted() {
                        "Shift on".to_string()
                    } else {
                        "Shift".to_string()
                    },
                ));
            }
            for (column, character) in characters.chars().enumerate() {
                let label = if keyboard.is_shifted() {
                    character.to_ascii_uppercase().to_string()
                } else {
                    character.to_string()
                };
                cells.push((key_name(index, column), label));
            }
            if index == 2 {
                cells.push((BACKSPACE.to_string(), "Delete character".to_string()));
            }
            let columns = u8::try_from(cells.len()).unwrap_or(u8::MAX);
            screen = screen.grid(columns, false, cells);
            if index == 2 {
                if let Some(kobo_ui::Node::Grid { cells, .. }) = screen.nodes.last_mut() {
                    if let Some(cell) = cells.first_mut() {
                        cell.glyph = Some(kobo_ui::Glyph::Shift);
                        cell.selected = keyboard.is_shifted();
                    }
                    if let Some(cell) = cells.last_mut() {
                        cell.glyph = Some(kobo_ui::Glyph::Backspace);
                    }
                }
            }
        }
        screen.grid(
            3,
            false,
            [
                (
                    LAYER.to_string(),
                    match keyboard.layer() {
                        Layer::Letters => "?123".to_string(),
                        Layer::Symbols => "abc".to_string(),
                    },
                ),
                (SPACE.to_string(), "space".to_string()),
                (ENTER.to_string(), submit.to_string()),
            ],
        )
    }

    /// Draws an open [`TextEntry`]: the prompt, the text, the keyboard and a
    /// way out.
    ///
    /// A way out is not optional. The keyboard fills the panel, so without it
    /// a reader who tapped the field by accident has no route back except
    /// submitting something they did not want.
    #[must_use]
    pub fn text_entry(self, entry: &TextEntry, prompt: &str, submit: &str) -> Self {
        self.heading(prompt)
            .typed(entry.keyboard(), "Type here")
            .divider()
            .keyboard(entry.keyboard(), submit)
            .button(CANCEL, "Cancel")
    }

    /// Draws a credential entry without putting the entered value on screen.
    #[must_use]
    pub fn secret_entry(self, entry: &TextEntry, prompt: &str, submit: &str) -> Self {
        self.heading(prompt)
            .secret_typed(entry.keyboard(), "Type the credential here")
            .divider()
            .keyboard(entry.keyboard(), submit)
            .button(CANCEL, "Cancel")
    }

    /// Indicates whether a credential has been entered without echoing it.
    #[must_use]
    pub fn secret_typed(self, keyboard: &Keyboard, placeholder: &str) -> Self {
        if keyboard.is_empty() {
            self.text(placeholder)
        } else {
            self.text("Credential entered")
        }
    }

    /// Shows what has been typed, or `placeholder` when nothing has been.
    ///
    /// An empty field draws nothing at all, which on a panel with no cursor
    /// reads as a broken screen rather than an empty one.
    #[must_use]
    pub fn typed(self, keyboard: &Keyboard, placeholder: &str) -> Self {
        if keyboard.is_empty() {
            self.text(placeholder)
        } else {
            self.text(keyboard.text())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Keyboard, Layer, Pressed, MAX_TEXT};
    use super::{TextEntry, Typing};
    use crate::{action_id, ScreenBuilder};

    /// The pages a reader can reach from a fresh keyboard, as the taps that
    /// reach them.
    const PAGES: [&[&str]; 4] = [&[], &["kb.shift"], &["kb.layer"], &["kb.layer", "kb.shift"]];

    #[test]
    fn every_printable_character_can_be_typed_on_the_keyboard() {
        // A control that asks for a character the keyboard cannot produce is
        // a control nobody can use. They went missing one at a time: the hash
        // a todo tag starts with, the underscore in every Lichess token, then
        // the percent sign a reader went looking for. A Wi-Fi password may be
        // any printable character and so may a line typed into a shell, so
        // this checks all of them rather than whichever was missed last.
        let mut typed = String::new();
        for page in PAGES {
            for row in 0..3 {
                for column in 0..16 {
                    let mut keyboard = Keyboard::new();
                    for name in page {
                        tap(&mut keyboard, name);
                    }
                    tap(&mut keyboard, &format!("kb.r{row}c{column}"));
                    typed.push_str(keyboard.text());
                }
            }
        }
        for wanted in '!'..='~' {
            assert!(
                typed.contains(wanted),
                "the keyboard cannot type {wanted:?}"
            );
        }
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
        // A shift pressed for a capital and abandoned for a digit would
        // otherwise open the symbol layer on its shifted page, and the reader
        // who wanted a 1 would get an exclamation mark.
        let mut keyboard = Keyboard::new();
        tap(&mut keyboard, "kb.shift");
        tap(&mut keyboard, "kb.layer");
        assert!(!keyboard.is_shifted());
        tap(&mut keyboard, "kb.r0c0");
        assert_eq!(keyboard.text(), "1");
    }

    #[test]
    fn a_field_opens_on_letters_whatever_the_last_one_was_left_on() {
        // Opening a field emptied the text but kept the layer and the shift,
        // so a field opened after a number had been typed into another one
        // came up on the symbol layer, with nothing on the panel to say why.
        let mut entry = TextEntry::new();
        entry.open();
        for name in ["kb.layer", "kb.r0c0", "kb.shift"] {
            entry.handle(action_id(name));
        }
        assert_eq!(
            entry.handle(action_id("kb.enter")),
            Some(Typing::Submitted("1".to_string()))
        );
        entry.open();
        assert_eq!(entry.keyboard().layer(), Layer::Letters);
        assert!(!entry.keyboard().is_shifted());

        entry.handle(action_id("kb.layer"));
        assert_eq!(
            entry.handle(action_id("kb.cancel")),
            Some(Typing::Cancelled)
        );
        entry.open();
        assert_eq!(entry.keyboard().layer(), Layer::Letters);
    }
    #[test]
    fn a_numeric_field_opens_on_the_digits_and_still_reaches_the_letters() {
        // A numeric account ID used to open on the letters, so the first tap
        // of every entry was a hunt for the key that shows the digits.
        let mut entry = TextEntry::new().numeric();
        entry.open();
        assert_eq!(entry.keyboard().layer(), Layer::Symbols);
        for name in ["kb.r0c0", "kb.r0c1", "kb.r0c2"] {
            entry.handle(action_id(name));
        }
        assert_eq!(entry.text(), "123");
        entry.handle(action_id("kb.layer"));
        entry.handle(action_id("kb.r0c0"));
        assert_eq!(entry.text(), "123q");
        entry.close();
        entry.open_with("42");
        assert_eq!(entry.keyboard().layer(), Layer::Symbols);
        assert_eq!(entry.text(), "42");
        assert_eq!(Keyboard::numeric().layer(), Layer::Symbols);
    }

    use kobo_ui::{Chrome, DisplayMetrics, LayoutKind, TextScale};

    fn panels() -> Vec<(String, DisplayMetrics)> {
        kobo_profile::SUPPORTED_PROFILES
            .iter()
            .flat_map(|profile| {
                let portrait = DisplayMetrics {
                    width: i32::try_from(profile.width).expect("profile width fits layout"),
                    height: i32::try_from(profile.height).expect("profile height fits layout"),
                    pixels_per_inch: i32::from(profile.pixels_per_inch),
                    text_scale: TextScale::Default,
                };
                let landscape = DisplayMetrics {
                    width: portrait.height,
                    height: portrait.width,
                    ..portrait
                };
                [
                    (format!("{} portrait", profile.id), portrait),
                    (format!("{} landscape", profile.id), landscape),
                ]
            })
            .collect()
    }

    fn tap(keyboard: &mut Keyboard, name: &str) -> Option<Pressed> {
        keyboard.press(action_id(name))
    }

    /// A keyboard belongs under the thumbs.
    ///
    /// It was placed in flow, so it started wherever the prompt above it
    /// finished: two lines of text put the keys across the middle of the panel
    /// with five hundred pixels of paper underneath them.
    #[test]
    fn a_keyboard_sits_at_the_foot_of_the_panel() {
        let mut entry = TextEntry::new();
        entry.open();
        let screen = ScreenBuilder::new("todo")
            .top_bar("Todo")
            .text_entry(&entry, "New item", "Add")
            .build();
        for (name, metrics) in panels() {
            let layout = screen.layout_with(&metrics, &Chrome::measuring(true));
            let bottom = layout
                .nodes
                .iter()
                .map(|node| node.rect.y + node.rect.height)
                .max()
                .unwrap_or(0);
            let content = layout.content;
            let spare = content.y + content.height - bottom;
            assert!(
                (0..=metrics.touch_target_default()).contains(&spare),
                "{name}: a compose screen left {spare} pixels of paper under its keyboard"
            );
        }
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
        // Truncating by byte would leave the string invalid, and backspace is
        // the one key a reader presses repeatedly without looking.
        let mut keyboard = Keyboard::with_text("café");
        tap(&mut keyboard, "kb.backspace");
        assert_eq!(keyboard.text(), "caf");
    }

    #[test]
    fn backspace_on_an_empty_field_is_harmless() {
        let mut keyboard = Keyboard::new();
        assert_eq!(tap(&mut keyboard, "kb.backspace"), Some(Pressed::Edited));
        assert!(keyboard.is_empty());
    }

    #[test]
    fn enter_submits_without_changing_the_text() {
        let mut keyboard = Keyboard::with_text("dickens");
        assert_eq!(tap(&mut keyboard, "kb.enter"), Some(Pressed::Submitted));
        assert_eq!(keyboard.text(), "dickens");
    }

    #[test]
    fn an_action_the_keyboard_does_not_own_is_handed_back() {
        // Applications pass every action here first, so claiming one that
        // belongs to the application would silently break its own buttons.
        let mut keyboard = Keyboard::new();
        assert_eq!(tap(&mut keyboard, "search"), None);
        assert_eq!(tap(&mut keyboard, "back"), None);
    }

    #[test]
    fn text_stops_growing_at_the_limit() {
        let mut keyboard = Keyboard::with_text("x".repeat(MAX_TEXT + 50));
        assert_eq!(keyboard.text().chars().count(), MAX_TEXT);
        tap(&mut keyboard, "kb.r0c0");
        assert_eq!(keyboard.text().chars().count(), MAX_TEXT);
    }

    #[test]
    fn taking_the_text_leaves_the_keyboard_ready_for_the_next_entry() {
        let mut keyboard = Keyboard::with_text("hello");
        assert_eq!(keyboard.take(), "hello");
        assert!(keyboard.is_empty());
    }

    #[test]
    fn every_drawn_key_is_reachable_by_a_tap_at_its_centre() {
        // The keyboard is drawn by one function and decoded by another, and
        // nothing but this test stops them drifting apart. It lays the real
        // screen out and taps the middle of every cell.
        for (name, metrics) in panels() {
            for page in PAGES {
                let mut drawn = Keyboard::new();
                for key in page {
                    drawn.press(action_id(key));
                }
                let screen = ScreenBuilder::new("keyboard")
                    .keyboard(&drawn, "Send")
                    .build();
                let layout = screen.layout_with(&metrics, &Chrome::default());
                let cells = layout
                    .nodes
                    .iter()
                    .filter_map(|node| match node.kind {
                        LayoutKind::Cell(action, ..) => Some((action, node.rect)),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                assert!(!cells.is_empty(), "{name}: the keyboard drew no keys");
                let mut typed = String::new();
                for (action, rect) in &cells {
                    assert!(
                        rect.width >= metrics.touch_target_minimum()
                            && rect.height >= metrics.touch_target_minimum(),
                        "{name}: key {action:?} is smaller than a touch target"
                    );
                    let x = rect.x + rect.width / 2;
                    let y = rect.y + rect.height / 2;
                    let hit = layout.hit_test(x, y).expect("a tap inside a key hits it");
                    assert_eq!(
                        hit, *action,
                        "{name}: a key was covered by something drawn after it"
                    );
                    let mut keyboard = drawn.clone();
                    let outcome = keyboard.press(hit).expect("every drawn key decodes");
                    if outcome == Pressed::Edited {
                        typed.push_str(keyboard.text());
                    }
                }
                // Every character of the page, plus the space the space bar
                // types. Read from the page rather than spelled out here,
                // because a copy spelled out here is how the underscore and
                // the hash were added to the keyboard and never checked.
                for character in drawn.rows().concat().chars() {
                    let character = if drawn.is_shifted() {
                        character.to_ascii_uppercase()
                    } else {
                        character
                    };
                    assert!(
                        typed.contains(character),
                        "{name}: {character} is drawn but cannot be typed"
                    );
                }
                assert!(typed.contains(' '), "{name}: the space bar types a space");
            }
        }
    }
}

//! Keys for a terminal, where a keystroke is sent the instant it happens.
//!
//! The text keyboard in [`crate::keyboard`] accumulates a string and hands it
//! over when the reader says so, which is right for a search query and wrong
//! for a shell: `Ctrl-C` has to arrive while the program is still running, and
//! a program reading a password is waiting on each byte. So this shares the
//! letters, the shift and the symbol layer, and sends rather than collects.
//!
//! It is a composite like the keyboard is: no node was added to the wire
//! format, the layout engine or the renderer. It is rows of tappable cells and
//! a small state machine deciding what each one means right now.

use crate::keyboard::Keyboard;
use crate::ScreenBuilder;
use kobo_ui::ActionId;

/// What a tap on the terminal keys produced.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Typed {
    /// Bytes for the program. Send them and expect output back.
    Send(Vec<u8>),
    /// A modifier moved. The labels changed, so repaint; send nothing.
    Changed,
}

/// The on-screen keys of a terminal.
#[derive(Clone, Debug, Default)]
pub struct TerminalKeys {
    keyboard: Keyboard,
    control: bool,
}

impl TerminalKeys {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub const fn is_control(&self) -> bool {
        self.control
    }

    #[must_use]
    pub const fn keyboard(&self) -> &Keyboard {
        &self.keyboard
    }

    /// Applies `action` if it belongs to the terminal keys.
    ///
    /// Returns `None` for anything else, so an application passes every action
    /// here first and handles its own afterwards without having to know which
    /// identifiers were claimed.
    pub fn press(&mut self, action: ActionId) -> Option<Typed> {
        if action == crate::action_id(CONTROL) {
            self.control = !self.control;
            return Some(Typed::Changed);
        }
        for (name, bytes) in SPECIALS {
            if action == crate::action_id(name) {
                return Some(Typed::Send(bytes.to_vec()));
            }
        }
        if let Some(character) = self.keyboard.resolves(action) {
            // A control modifier applies to one key and then releases, the
            // same as shift. Leaving it latched would mean reading the panel
            // before every key to find out what it is about to do, and the key
            // it is about to do it to might be `Ctrl-D`.
            let control = std::mem::take(&mut self.control);
            // Pressed for its effect on the modifiers, not for its text: the
            // shift has to release exactly as it does when typing.
            let _ignored = self.keyboard.press(action);
            self.keyboard.clear();
            return Some(Typed::Send(encode(character, control)));
        }
        // Shift and the symbol layer, which change what every key means
        // without sending anything themselves.
        let _moved = self.keyboard.press(action)?;
        self.keyboard.clear();
        Some(Typed::Changed)
    }
}

/// Turns one character into the bytes a terminal expects for it.
///
/// The control encoding is not a lookup table by accident: `Ctrl` clears the
/// two high bits of the character's code, which is why `Ctrl-C` is 3, `Ctrl-D`
/// is 4 and `Ctrl-[` is escape. Encoding it as arithmetic rather than a list
/// means every key gets the right answer, including the ones nobody thought to
/// put in the list.
fn encode(character: char, control: bool) -> Vec<u8> {
    if !control {
        let mut bytes = [0u8; 4];
        return character.encode_utf8(&mut bytes).as_bytes().to_vec();
    }
    let upper = character.to_ascii_uppercase();
    if upper.is_ascii() && (b'@'..=b'_').contains(&(upper as u8)) {
        return vec![(upper as u8) & 0x1f];
    }
    // A control combination this terminal has no code for. The plain character
    // is sent rather than nothing, because swallowing a keystroke silently
    // looks exactly like a device that has stopped responding.
    let mut bytes = [0u8; 4];
    character.encode_utf8(&mut bytes).as_bytes().to_vec()
}

const CONTROL: &str = "term.ctrl";

/// Keys that always mean one fixed sequence.
///
/// The arrows are the `vt100` cursor sequences rather than the application-mode
/// ones, matching the `TERM` the runtime sets. This device has no terminfo
/// database at all, so a program looks its keys up in whatever it compiled in
/// for `vt100` and these are what it will be expecting.
const SPECIALS: [(&str, &[u8]); 9] = [
    ("term.esc", b"\x1b"),
    ("term.tab", b"\t"),
    ("term.up", b"\x1b[A"),
    ("term.down", b"\x1b[B"),
    ("term.left", b"\x1b[D"),
    ("term.right", b"\x1b[C"),
    // The three keys the shared keyboard already draws, given terminal
    // meanings. Return is a carriage return and not a newline, because that is
    // what a terminal's return key sends; a shell given 0x0a is still waiting
    // for a line that never ended.
    ("kb.enter", b"\r"),
    ("kb.space", b" "),
    // Every terminal since the VT220 sends delete for the key above return. A
    // shell given an actual backspace moves the cursor left instead of
    // erasing, which reads as a keyboard that has stopped working.
    ("kb.backspace", b"\x7f"),
];

impl ScreenBuilder {
    /// Draws the keys of a terminal: the specials, then the usual letters.
    ///
    /// The specials are on their own row above the letters because they are
    /// the ones a reader hunts for. `Ctrl` shows its state in its label, since
    /// on a panel with no hover and no colour there is nowhere else to put it.
    #[must_use]
    pub fn terminal_keys(self, keys: &TerminalKeys) -> Self {
        let control = if keys.is_control() { "CTRL" } else { "ctrl" };
        self.grid(
            7,
            false,
            [
                ("term.esc".to_string(), "esc".to_string()),
                ("term.tab".to_string(), "tab".to_string()),
                (CONTROL.to_string(), control.to_string()),
                ("term.up".to_string(), "up".to_string()),
                ("term.down".to_string(), "down".to_string()),
                ("term.left".to_string(), "left".to_string()),
                ("term.right".to_string(), "right".to_string()),
            ],
        )
        .keyboard(keys.keyboard(), "enter")
    }
}

#[cfg(test)]
mod tests {
    use super::{TerminalKeys, Typed};
    use crate::action_id;

    fn tap(keys: &mut TerminalKeys, name: &str) -> Option<Typed> {
        keys.press(action_id(name))
    }

    #[test]
    fn a_letter_is_sent_the_moment_it_is_tapped() {
        // Not collected. A shell is waiting on the byte, not on a submission.
        let mut keys = TerminalKeys::new();
        assert_eq!(tap(&mut keys, "kb.r0c0"), Some(Typed::Send(b"q".to_vec())));
    }

    #[test]
    fn control_c_is_the_byte_a_program_is_actually_watching_for() {
        // The whole reason a terminal needs its own key layer: this has to
        // arrive while the program is still running.
        let mut keys = TerminalKeys::new();
        assert_eq!(tap(&mut keys, "term.ctrl"), Some(Typed::Changed));
        assert!(keys.is_control());
        // `c` is the third key of the bottom letter row.
        assert_eq!(tap(&mut keys, "kb.r2c2"), Some(Typed::Send(vec![0x03])));
    }

    #[test]
    fn control_applies_to_one_key_and_then_releases() {
        let mut keys = TerminalKeys::new();
        tap(&mut keys, "term.ctrl");
        tap(&mut keys, "kb.r2c2");
        assert!(!keys.is_control());
        assert_eq!(tap(&mut keys, "kb.r2c2"), Some(Typed::Send(b"c".to_vec())));
    }

    #[test]
    fn control_d_ends_a_shell_and_control_l_clears_it() {
        for (key, byte) in [("kb.r1c2", 0x04u8), ("kb.r1c8", 0x0c)] {
            let mut keys = TerminalKeys::new();
            tap(&mut keys, "term.ctrl");
            assert_eq!(tap(&mut keys, key), Some(Typed::Send(vec![byte])));
        }
    }

    #[test]
    fn return_sends_a_carriage_return_rather_than_a_newline() {
        // A terminal's return key is 0x0d. Sending 0x0a instead leaves a shell
        // waiting for a line that, as far as it is concerned, never ended.
        let mut keys = TerminalKeys::new();
        assert_eq!(
            tap(&mut keys, "kb.enter"),
            Some(Typed::Send(b"\r".to_vec()))
        );
    }

    #[test]
    fn backspace_sends_delete_because_that_is_what_terminals_send() {
        // Every terminal since the VT220 sends 0x7f for the key above return.
        // A shell given an actual 0x08 moves the cursor left instead of
        // erasing, which looks like a keyboard that has stopped working.
        let mut keys = TerminalKeys::new();
        assert_eq!(
            tap(&mut keys, "kb.backspace"),
            Some(Typed::Send(vec![0x7f]))
        );
    }

    #[test]
    fn a_modifier_changes_the_keys_without_sending_anything() {
        let mut keys = TerminalKeys::new();
        assert_eq!(tap(&mut keys, "kb.shift"), Some(Typed::Changed));
        assert_eq!(tap(&mut keys, "kb.layer"), Some(Typed::Changed));
    }

    /// The grid a terminal is given has to leave room for the keys under it.
    ///
    /// `terminal_grid_for` measured the leftover space with the layout engine,
    /// which was right, but with the default chrome, which was not: an
    /// application is never told the clock or the battery, and the status band
    /// is drawn above everything. So the terminal came back two rows taller
    /// than the panel could hold and the bottom key row -- space, enter, the
    /// layer switch -- was drawn off the edge of the screen.
    #[test]
    fn the_keys_under_a_terminal_stay_on_the_panel() {
        for metrics in [
            kobo_ui::CLARA_BW_METRICS,
            kobo_ui::DisplayMetrics::default(),
        ] {
            let keys = TerminalKeys::new();
            let compose = |rows: Vec<String>| {
                crate::ScreenBuilder::new("terminal")
                    .top_bar("Terminal")
                    .terminal(rows, None)
                    .terminal_keys(&keys)
                    .build()
            };
            let (columns, rows) = kobo_ui::terminal_grid_for(&compose(Vec::new()), &metrics);
            assert!(columns > 0 && rows > 0, "a terminal was given no grid");
            let full = compose(vec!["x".repeat(columns as usize); rows as usize]);
            let layout = full.layout_with(&metrics, &kobo_ui::Chrome::measuring(true));
            let overflow = layout
                .nodes
                .iter()
                .map(|node| node.rect.y + node.rect.height)
                .max()
                .unwrap_or(0);
            assert!(
                overflow <= metrics.height,
                "a full terminal pushed its keys {} past the bottom of a {} panel",
                overflow - metrics.height,
                metrics.height
            );
        }
    }

    #[test]
    fn the_arrows_send_the_vt100_sequences_the_runtime_promised() {
        let mut keys = TerminalKeys::new();
        assert_eq!(
            tap(&mut keys, "term.up"),
            Some(Typed::Send(b"\x1b[A".to_vec()))
        );
        assert_eq!(
            tap(&mut keys, "term.left"),
            Some(Typed::Send(b"\x1b[D".to_vec()))
        );
    }

    #[test]
    fn the_symbol_layer_reaches_the_characters_a_shell_needs() {
        let mut keys = TerminalKeys::new();
        tap(&mut keys, "kb.layer");
        assert_eq!(tap(&mut keys, "kb.r1c1"), Some(Typed::Send(b"/".to_vec())));
    }

    #[test]
    fn shift_reaches_capitals_here_too() {
        let mut keys = TerminalKeys::new();
        tap(&mut keys, "kb.shift");
        assert_eq!(tap(&mut keys, "kb.r0c0"), Some(Typed::Send(b"Q".to_vec())));
    }

    #[test]
    fn an_action_that_is_not_a_key_is_left_for_the_application() {
        let mut keys = TerminalKeys::new();
        assert_eq!(tap(&mut keys, "app.something.else"), None);
    }

    #[test]
    fn nothing_is_accumulated_between_keystrokes() {
        // The text keyboard's buffer would otherwise grow forever on a
        // terminal that is never submitted.
        let mut keys = TerminalKeys::new();
        for name in ["kb.r0c0", "kb.r0c1", "kb.r0c2"] {
            tap(&mut keys, name);
        }
        assert!(keys.keyboard().is_empty());
    }
}

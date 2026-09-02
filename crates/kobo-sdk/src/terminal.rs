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

use crate::keyboard::{key_name, Keyboard, Layer, BACKSPACE, ENTER, LAYER, SHIFT, SPACE};
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
    /// Draws a terminal keyboard with its fixed keys beside the usual letters.
    ///
    /// Keeping the function keys in the four familiar keyboard rows leaves the
    /// majority of a landscape panel to the terminal instead of spending a
    /// fifth row on a separate strip. `Ctrl` shows its state in its label,
    /// since on a panel with no hover and no colour there is nowhere else to
    /// put it.
    #[must_use]
    pub fn terminal_keys(self, keys: &TerminalKeys) -> Self {
        let control = if keys.is_control() { "CTRL" } else { "ctrl" };
        let keyboard = keys.keyboard();
        let shifted = keyboard.is_shifted();
        let row = |row, characters: &str| {
            characters
                .chars()
                .enumerate()
                .map(|(column, character)| {
                    (
                        key_name(row, column),
                        if shifted {
                            character.to_ascii_uppercase().to_string()
                        } else {
                            character.to_string()
                        },
                    )
                })
                .collect::<Vec<_>>()
        };
        let [top, home, bottom] = keyboard.rows();
        let top = row(0, top);
        let mut home = row(1, home);
        let bottom = row(2, bottom);
        home.push((CONTROL.to_string(), control.to_string()));

        let mut lower = Vec::with_capacity(bottom.len() + 3);
        lower.push((
            SHIFT.to_string(),
            if keys.keyboard().is_shifted() {
                "SHIFT".to_string()
            } else {
                "shift".to_string()
            },
        ));
        lower.extend(bottom);
        lower.push((BACKSPACE.to_string(), "back".to_string()));
        lower.push(("term.esc".to_string(), "esc".to_string()));

        self.fill()
            .grid(u8::try_from(top.len()).unwrap_or(u8::MAX), false, top)
            .grid(u8::try_from(home.len()).unwrap_or(u8::MAX), false, home)
            .grid(u8::try_from(lower.len()).unwrap_or(u8::MAX), false, lower)
            .grid(
                8,
                false,
                [
                    ("term.tab".to_string(), "tab".to_string()),
                    (
                        LAYER.to_string(),
                        match keys.keyboard().layer() {
                            Layer::Letters => "?123".to_string(),
                            Layer::Symbols => "abc".to_string(),
                        },
                    ),
                    (SPACE.to_string(), "space".to_string()),
                    (ENTER.to_string(), "enter".to_string()),
                    ("term.up".to_string(), "up".to_string()),
                    ("term.down".to_string(), "down".to_string()),
                    ("term.left".to_string(), "left".to_string()),
                    ("term.right".to_string(), "right".to_string()),
                ],
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{TerminalKeys, Typed};
    use crate::action_id;
    use kobo_ui::{
        mono_cell, render_with, terminal_grid, Caret, Chrome, ControlState, DisplayMetrics,
        FontSize, LayoutIssueKind, LayoutKind, Surface, TextScale,
    };
    use unicode_width::UnicodeWidthStr;

    fn tap(keys: &mut TerminalKeys, name: &str) -> Option<Typed> {
        keys.press(action_id(name))
    }

    fn paperterm_rows() -> Vec<String> {
        let mut rows = vec![String::new(); 35];
        for (index, line) in [
            "PAPERTERM PHYSICAL E2E",
            "status=LIVE",
            "pty=100x35",
            "tick=0095",
            "last_input=none",
            "Touch an arrow, Enter, then Ctrl-C.",
        ]
        .into_iter()
        .enumerate()
        {
            rows[index] = line.into();
        }
        rows
    }

    fn terminal_controls_screen(
        rows: &[String],
        cursor: Option<Caret>,
        legacy: bool,
    ) -> kobo_ui::Screen {
        crate::ScreenBuilder::new("paperterm")
            .top_bar("Paperterm")
            .top_bar_action("pairing", "Pairing")
            .terminal(rows.iter().cloned(), cursor)
            .fill()
            .secondary("The shell remains on your computer.")
            .grid(
                3,
                false,
                [("up", "↑"), ("enter", "Enter"), ("ctrl-c", "Ctrl-C")],
            )
            .build()
            .with_legacy_typography(legacy)
    }

    fn bordered(content: &str, columns: usize) -> String {
        let inner = columns.saturating_sub(2);
        let width = UnicodeWidthStr::width(content);
        assert!(width <= inner);
        format!("│{content}{}│", " ".repeat(inner - width))
    }

    fn claude_code_tui(columns: usize, rows: usize) -> (Vec<String>, usize) {
        assert!(columns >= 24 && rows >= 9);
        let inner = columns - 2;
        let wide_prefix = "Wide glyph: ";
        let wide_padding = inner - UnicodeWidthStr::width(wide_prefix) - 2;
        let wide_column = 1 + UnicodeWidthStr::width(wide_prefix) + wide_padding;
        let wide = format!("{wide_prefix}{}界", " ".repeat(wide_padding));
        let mut screen = vec![
            format!("╭{}╮", "─".repeat(inner)),
            bordered(" Claude Code", columns),
            bordered(" ❯ Implement responsive terminal layout", columns),
            format!("├{}┤", "─".repeat(inner)),
            bordered(" ⏺ Read(crates/kobo-ui/src/lib.rs)", columns),
            bordered(&wide, columns),
            format!("{}界", "x".repeat(columns - 1)),
            bordered(" ⎿ Updated SDK layout invariants", columns),
        ];
        while screen.len() + 1 < rows {
            screen.push(bordered("", columns));
        }
        screen.push(format!("╰{}╯", "─".repeat(inner)));
        screen.push("VERTICALLY CLIPPED".into());
        screen.push("ALSO CLIPPED".into());
        (screen, wide_column)
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
        for profile in kobo_profile::SUPPORTED_PROFILES {
            let portrait = kobo_ui::DisplayMetrics {
                width: i32::try_from(profile.width).expect("profile width fits layout"),
                height: i32::try_from(profile.height).expect("profile height fits layout"),
                pixels_per_inch: i32::from(profile.pixels_per_inch),
                text_scale: kobo_ui::TextScale::Default,
            };
            for (orientation, metrics) in [
                ("portrait", portrait),
                (
                    "landscape",
                    kobo_ui::DisplayMetrics {
                        width: portrait.height,
                        height: portrait.width,
                        ..portrait
                    },
                ),
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
                assert!(
                    columns > 0 && rows > 0,
                    "{} {orientation}: a terminal was given no grid",
                    profile.id
                );
                let full = compose(vec!["x".repeat(columns as usize); rows as usize]);
                let layout = full.layout_with(&metrics, &kobo_ui::Chrome::measuring(true));
                for node in &layout.nodes {
                    assert!(
                        node.rect.x >= 0
                            && node.rect.y >= 0
                            && node.rect.x + node.rect.width <= metrics.width
                            && node.rect.y + node.rect.height <= metrics.height,
                        "{} {orientation}: {:?} left the panel",
                        profile.id,
                        node.kind
                    );
                    if node.kind.acts_on().is_some() {
                        assert!(
                            node.rect.width >= metrics.touch_target_minimum()
                                && node.rect.height >= metrics.touch_target_minimum(),
                            "{} {orientation}: {:?} is smaller than a touch target",
                            profile.id,
                            node.kind
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn paperterm_status_and_controls_remain_visible_below_a_full_terminal() {
        let rows = paperterm_rows();

        for profile in kobo_profile::SUPPORTED_PROFILES {
            let portrait = DisplayMetrics {
                width: i32::try_from(profile.width).expect("profile width fits layout"),
                height: i32::try_from(profile.height).expect("profile height fits layout"),
                pixels_per_inch: i32::from(profile.pixels_per_inch),
                text_scale: TextScale::Default,
            };
            for (orientation, metrics) in [
                ("portrait", portrait),
                (
                    "landscape",
                    DisplayMetrics {
                        width: portrait.height,
                        height: portrait.width,
                        ..portrait
                    },
                ),
            ] {
                for legacy in [false, true] {
                    let screen = terminal_controls_screen(&rows, None, legacy);
                    let chrome = Chrome::measuring(false);
                    let diagnostics = screen.diagnostics(&metrics, &chrome);
                    assert!(
                        !diagnostics.issues.iter().any(|issue| matches!(
                            issue.kind,
                            LayoutIssueKind::InteractiveOffscreen
                                | LayoutIssueKind::Clipped
                                | LayoutIssueKind::TextOverflow
                        )),
                        "{} {orientation} legacy={legacy}: {:?}",
                        profile.id,
                        diagnostics.issues
                    );

                    let content = diagnostics.layout.content;
                    let controls = diagnostics
                        .layout
                        .nodes
                        .iter()
                        .filter(|node| matches!(node.kind, LayoutKind::Cell(..)))
                        .collect::<Vec<_>>();
                    assert_eq!(
                        controls.len(),
                        3,
                        "{} {orientation} legacy={legacy}",
                        profile.id
                    );
                    for control in controls {
                        assert!(
                            control.rect.x >= content.x
                                && control.rect.y >= content.y
                                && control.rect.x + control.rect.width <= content.x + content.width
                                && control.rect.y + control.rect.height
                                    <= content.y + content.height,
                            "{} {orientation} legacy={legacy}: {:?} is outside {:?}",
                            profile.id,
                            control.rect,
                            content
                        );
                        assert!(
                        control.rect.width >= metrics.touch_target_minimum()
                            && control.rect.height >= metrics.touch_target_minimum(),
                        "{} {orientation} legacy={legacy}: control is smaller than a touch target",
                        profile.id
                    );
                    }

                    for node in &diagnostics.layout.nodes {
                        let enabled = node.kind.acts_on().is_some()
                            && !matches!(
                                node.kind,
                                LayoutKind::Button(_, ControlState::Disabled, _)
                                    | LayoutKind::Tile(_, ControlState::Disabled)
                                    | LayoutKind::StepperControl(_, ControlState::Disabled, _)
                            );
                        if enabled {
                            assert!(
                                node.rect.x >= 0
                                    && node.rect.y >= 0
                                    && node.rect.x + node.rect.width <= metrics.width
                                    && node.rect.y + node.rect.height <= metrics.height,
                                "{} {orientation} legacy={legacy}: {:?} is offscreen at {:?}",
                                profile.id,
                                node.kind,
                                node.rect
                            );
                        }
                    }
                }
            }
        }
    }

    fn assert_terminal_controls_and_render(
        name: &str,
        metrics: DisplayMetrics,
        screen: &kobo_ui::Screen,
        chrome: &Chrome,
        layout: &kobo_ui::Layout,
        terminal: kobo_ui::Rect,
    ) {
        let controls = layout
            .nodes
            .iter()
            .filter(|node| matches!(node.kind, LayoutKind::Cell(..)))
            .collect::<Vec<_>>();
        assert_eq!(controls.len(), 3);
        for (index, control) in controls.iter().enumerate() {
            assert!(control.rect.x >= layout.content.x, "{name}");
            assert!(control.rect.y >= layout.content.y, "{name}");
            assert!(
                control.rect.x + control.rect.width <= layout.content.x + layout.content.width,
                "{name}"
            );
            assert!(
                control.rect.y + control.rect.height <= layout.content.y + layout.content.height,
                "{name}"
            );
            assert!(terminal.intersection(control.rect).is_none(), "{name}");
            for other in &controls[index + 1..] {
                assert!(control.rect.intersection(other.rect).is_none(), "{name}");
            }
        }
        for secondary in layout
            .nodes
            .iter()
            .filter(|node| node.kind == LayoutKind::Secondary)
        {
            assert!(terminal.intersection(secondary.rect).is_none(), "{name}");
            assert!(controls
                .iter()
                .all(|control| secondary.rect.intersection(control.rect).is_none()));
        }

        let mut surface = Surface::new(
            usize::try_from(metrics.width).expect("positive profile width"),
            usize::try_from(metrics.height).expect("positive profile height"),
        );
        render_with(screen, &metrics, chrome, &mut surface, None);
        assert!(surface
            .pixels
            .iter()
            .any(|pixel| *pixel != kobo_ui::tone::PAPER));
    }

    fn assert_claude_terminal(name: &str, metrics: DisplayMetrics) {
        let chrome = Chrome::measuring(false);
        let template = terminal_controls_screen(&[], None, false);
        let negotiated = kobo_ui::terminal_grid_for(&template, &metrics);
        assert!(negotiated.0 >= 24 && negotiated.1 >= 9, "{name}");
        let bare = crate::ScreenBuilder::new("terminal")
            .top_bar("Terminal")
            .terminal(Vec::<String>::new(), None)
            .build();
        let bare_grid = kobo_ui::terminal_grid_for(&bare, &metrics);
        assert_eq!(
            bare_grid.0, negotiated.0,
            "{name}: controls changed columns"
        );
        assert!(bare_grid.1 >= negotiated.1, "{name}: controls gained rows");
        if bare_grid.1 == negotiated.1 {
            assert_eq!(
                usize::from(negotiated.1),
                kobo_ui::MAX_TERMINAL_ROWS,
                "{name}: optional controls reserved no rows before the grid cap"
            );
        }

        let (source, wide_column) =
            claude_code_tui(usize::from(negotiated.0), usize::from(negotiated.1));
        let screen = terminal_controls_screen(
            &source,
            Some(Caret::new(
                5,
                u16::try_from(wide_column).expect("wide column"),
            )),
            false,
        );
        let diagnostics = screen.diagnostics(&metrics, &chrome);
        assert!(
            !diagnostics.issues.iter().any(|issue| matches!(
                issue.kind,
                LayoutIssueKind::InteractiveOffscreen
                    | LayoutIssueKind::Clipped
                    | LayoutIssueKind::TextOverflow
            )),
            "{name}: {:?}",
            diagnostics.issues
        );
        let layout = &diagnostics.layout;
        let terminal = layout
            .nodes
            .iter()
            .find(|node| node.kind == LayoutKind::TerminalGrid)
            .expect("terminal grid");
        let (cell_width, cell_height) = mono_cell(FontSize::Caption);
        assert_eq!(
            terminal_grid(terminal.rect.width, terminal.rect.height),
            negotiated,
            "{name}: negotiated and rendered grids differ"
        );
        assert_eq!(terminal.rect.width, i32::from(negotiated.0) * cell_width);
        assert_eq!(terminal.rect.height, i32::from(negotiated.1) * cell_height);
        assert_eq!(terminal.text_lines.len(), usize::from(negotiated.1));
        assert!(!terminal
            .text_lines
            .iter()
            .any(|line| line.contains("CLIPPED")));
        assert!(terminal
            .text_lines
            .iter()
            .all(|line| UnicodeWidthStr::width(line.as_str()) <= usize::from(negotiated.0)));
        assert_eq!(
            terminal.text_lines[6],
            "x".repeat(usize::from(negotiated.0) - 1),
            "{name}: a wide glyph was split across the right edge"
        );
        for &line in &[0, 1, 2, 3, 4, 5, 7, terminal.text_lines.len() - 1] {
            assert_eq!(
                UnicodeWidthStr::width(terminal.text_lines[line].as_str()),
                usize::from(negotiated.0),
                "{name}: box row {line} lost cell alignment"
            );
        }
        let cursor = layout
            .nodes
            .iter()
            .find(|node| node.kind == LayoutKind::TerminalCursor)
            .expect("wide cursor");
        let wide_column = i32::try_from(wide_column).expect("wide column fits layout");
        assert_eq!(cursor.rect.x, terminal.rect.x + wide_column * cell_width);
        assert_eq!(cursor.rect.width, 2 * cell_width);
        assert_eq!(cursor.rect.height, cell_height);
        assert_terminal_controls_and_render(name, metrics, &screen, &chrome, layout, terminal.rect);
    }

    #[test]
    fn rich_claude_terminal_keeps_its_grid_with_optional_controls() {
        for profile in kobo_profile::SUPPORTED_PROFILES {
            let portrait = DisplayMetrics {
                width: i32::try_from(profile.width).expect("profile width fits layout"),
                height: i32::try_from(profile.height).expect("profile height fits layout"),
                pixels_per_inch: i32::from(profile.pixels_per_inch),
                text_scale: TextScale::Default,
            };
            assert_claude_terminal(&format!("{} portrait", profile.id), portrait);
            assert_claude_terminal(
                &format!("{} landscape", profile.id),
                DisplayMetrics {
                    width: portrait.height,
                    height: portrait.width,
                    ..portrait
                },
            );
        }
    }

    fn assert_dense_terminal_keyboard(name: &str, metrics: DisplayMetrics, landscape: bool) {
        let keys = TerminalKeys::new();
        let compose = |rows: Vec<String>| {
            crate::ScreenBuilder::new("terminal")
                .top_bar("Terminal")
                .terminal(rows, None)
                .terminal_keys(&keys)
                .build()
        };
        let negotiated = kobo_ui::terminal_grid_for(&compose(Vec::new()), &metrics);
        let screen = compose(vec![
            "Claude Code responsive terminal".into();
            usize::from(negotiated.1)
        ]);
        let layout = screen.layout_with(&metrics, &Chrome::measuring(true));
        let terminal = layout
            .nodes
            .iter()
            .find(|node| node.kind == LayoutKind::TerminalGrid)
            .expect("terminal");
        let keys = layout
            .nodes
            .iter()
            .filter(|node| matches!(node.kind, LayoutKind::Cell(..)))
            .collect::<Vec<_>>();
        let mut row_tops = keys.iter().map(|key| key.rect.y).collect::<Vec<_>>();
        row_tops.sort_unstable();
        row_tops.dedup();
        assert_eq!(row_tops.len(), 4, "{name}: keyboard rows");
        let expected_height = if landscape {
            metrics.touch_target_minimum()
        } else {
            metrics.touch_target_default()
        };
        for (index, key) in keys.iter().enumerate() {
            assert_eq!(key.rect.height, expected_height, "{name}: key density");
            assert!(key.rect.x >= layout.content.x && key.rect.y >= layout.content.y);
            assert!(
                key.rect.x + key.rect.width <= layout.content.x + layout.content.width,
                "{name}: key left the content width"
            );
            assert!(
                key.rect.y + key.rect.height <= layout.content.y + layout.content.height,
                "{name}: key left the content height"
            );
            assert!(key.rect.width >= metrics.touch_target_minimum(), "{name}");
            assert_eq!(
                layout.hit_test(
                    key.rect.x + key.rect.width / 2,
                    key.rect.y + key.rect.height / 2
                ),
                key.kind.acts_on(),
                "{name}: key centre is not reachable"
            );
            assert!(terminal.rect.intersection(key.rect).is_none(), "{name}");
            for other in &keys[index + 1..] {
                assert!(key.rect.intersection(other.rect).is_none(), "{name}");
            }
        }
        if landscape {
            assert!(
                terminal.rect.height * 2 > layout.content.height,
                "{name}: terminal kept only {} of {} content pixels",
                terminal.rect.height,
                layout.content.height
            );
        }
    }

    #[test]
    fn terminal_keyboard_density_is_profile_and_orientation_driven() {
        for profile in kobo_profile::SUPPORTED_PROFILES {
            let portrait = DisplayMetrics {
                width: i32::try_from(profile.width).expect("profile width fits layout"),
                height: i32::try_from(profile.height).expect("profile height fits layout"),
                pixels_per_inch: i32::from(profile.pixels_per_inch),
                text_scale: TextScale::Default,
            };
            assert_dense_terminal_keyboard(&format!("{} portrait", profile.id), portrait, false);
            assert_dense_terminal_keyboard(
                &format!("{} landscape", profile.id),
                DisplayMetrics {
                    width: portrait.height,
                    height: portrait.width,
                    ..portrait
                },
                true,
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

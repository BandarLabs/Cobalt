//! What a program printed, turned into rows a panel can draw.
//!
//! A program does not print rows. It prints a stream containing text mixed
//! with escape sequences that move a cursor, clear a line, scroll a region and
//! change colour, and only a terminal emulator turns that back into a grid.
//! Writing one is a large job that is nearly all edge cases, so this crate
//! does not: it wraps [`vt100`], an MIT-licensed emulator, and translates its
//! screen into the terminal node this platform's renderer already draws.
//!
//! The dependency is deliberately quarantined here, exactly as the TLS stack
//! is quarantined in the networking crate. The UI layer, the protocol, the
//! runtime and the SDK stay free of it, so replacing the emulator changes one
//! crate and nothing else.
//!
//! # What is deliberately thrown away
//!
//! Colour, bold, italic and underline. This panel resolves sixteen greys, of
//! which five are usable, and mid greys ghost. A colour terminal rendered in
//! grey is less legible than a black one, not more. Inverse video is kept for
//! exactly one thing, the cursor, because that is the only mark a reader has
//! to find at a glance.

use kobo_ui::Caret;

/// How much history is kept above the visible grid.
///
/// Scrollback costs memory on a device with very little, and a reader cannot
/// select or copy from this panel, so its only use is looking back at what
/// scrolled past. A screen or two of that is the whole benefit.
const SCROLLBACK: usize = 100;

/// A running terminal's screen.
pub struct Terminal {
    parser: vt100::Parser,
    columns: u16,
    rows: u16,
}

impl Terminal {
    /// An empty screen of exactly this grid.
    #[must_use]
    pub fn new(columns: u16, rows: u16) -> Self {
        let columns = columns.max(1);
        let rows = rows.max(1);
        Self {
            parser: vt100::Parser::new(rows, columns, SCROLLBACK),
            columns,
            rows,
        }
    }

    /// Feeds bytes exactly as the program printed them.
    ///
    /// Chunk boundaries do not matter: an escape sequence split across two
    /// calls is resumed rather than mishandled, which is what makes it safe
    /// for the runtime to bound how much it carries in one message.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
    }

    /// Changes the grid, keeping what is on screen.
    pub fn resize(&mut self, columns: u16, rows: u16) {
        self.columns = columns.max(1);
        self.rows = rows.max(1);
        self.parser.screen_mut().set_size(self.rows, self.columns);
    }

    #[must_use]
    pub const fn grid(&self) -> (u16, u16) {
        (self.columns, self.rows)
    }

    /// The visible grid, one string per row.
    ///
    /// Always exactly as many rows as the grid has, including empty ones, so
    /// that the panel's layout does not move as output arrives. A screen that
    /// grew a row at a time would shift every control below it, which is the
    /// defect this project has already had to fix once.
    #[must_use]
    pub fn rows(&self) -> Vec<String> {
        let mut rows: Vec<String> = self
            .parser
            .screen()
            .rows(0, self.columns)
            .take(self.rows as usize)
            .collect();
        rows.resize(self.rows as usize, String::new());
        rows
    }

    /// Where the cursor is, or nothing when the program has hidden it.
    ///
    /// A hidden cursor is honoured rather than drawn anyway: a full-screen
    /// program that hides it has put something else where the reader should be
    /// looking, and an inverted cell in the middle of that is noise.
    #[must_use]
    pub fn cursor(&self) -> Option<Caret> {
        let screen = self.parser.screen();
        if screen.hide_cursor() {
            return None;
        }
        let (row, column) = screen.cursor_position();
        if row >= self.rows || column >= self.columns {
            return None;
        }
        Some(Caret::new(row, column))
    }

    /// Everything on the visible screen as one string, for tests and logs.
    #[must_use]
    pub fn text(&self) -> String {
        self.rows().join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::Terminal;
    use kobo_ui::Caret;

    #[test]
    fn plain_output_lands_in_the_rows_it_was_printed_on() {
        let mut terminal = Terminal::new(20, 4);
        terminal.feed(b"first\r\nsecond");
        let rows = terminal.rows();
        assert_eq!(rows[0], "first");
        assert_eq!(rows[1], "second");
    }

    #[test]
    fn the_grid_always_has_every_row_even_when_nothing_printed_on_it() {
        // Rows appearing one at a time would move everything below the
        // terminal down the panel as output arrived.
        let terminal = Terminal::new(20, 6);
        assert_eq!(terminal.rows().len(), 6);
    }

    #[test]
    fn an_escape_sequence_split_across_two_chunks_is_still_understood() {
        // The runtime bounds how much it carries in one message, so a
        // sequence *will* be split. Handling it in halves is not a nicety.
        let mut terminal = Terminal::new(20, 3);
        terminal.feed(b"abc\x1b");
        terminal.feed(b"[2J");
        assert_eq!(terminal.rows()[0], "");
    }

    #[test]
    fn the_cursor_follows_what_was_printed() {
        let mut terminal = Terminal::new(20, 3);
        terminal.feed(b"hi");
        assert_eq!(terminal.cursor(), Some(Caret::new(0, 2)));
    }

    #[test]
    fn a_program_that_hides_the_cursor_does_not_get_one_drawn_anyway() {
        let mut terminal = Terminal::new(20, 3);
        terminal.feed(b"\x1b[?25l");
        assert_eq!(terminal.cursor(), None);
    }

    #[test]
    fn moving_the_cursor_absolutely_puts_it_where_the_program_said() {
        let mut terminal = Terminal::new(20, 5);
        terminal.feed(b"\x1b[3;7H");
        assert_eq!(terminal.cursor(), Some(Caret::new(2, 6)));
    }

    #[test]
    fn output_past_the_last_row_scrolls_rather_than_growing_the_screen() {
        let mut terminal = Terminal::new(20, 3);
        terminal.feed(b"one\r\ntwo\r\nthree\r\nfour");
        let rows = terminal.rows();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[2], "four");
        assert_eq!(rows[0], "two");
    }

    #[test]
    fn a_line_longer_than_the_grid_wraps_the_way_the_program_expects() {
        let mut terminal = Terminal::new(5, 3);
        terminal.feed(b"abcdefgh");
        let rows = terminal.rows();
        assert_eq!(rows[0], "abcde");
        assert_eq!(rows[1], "fgh");
    }

    #[test]
    fn clearing_a_line_removes_what_was_on_it() {
        let mut terminal = Terminal::new(10, 2);
        terminal.feed(b"hello\r\x1b[K");
        assert_eq!(terminal.rows()[0], "");
    }

    #[test]
    fn resizing_keeps_the_screen_and_reports_the_new_grid() {
        let mut terminal = Terminal::new(20, 4);
        terminal.feed(b"kept");
        terminal.resize(40, 8);
        assert_eq!(terminal.grid(), (40, 8));
        assert_eq!(terminal.rows().len(), 8);
        assert_eq!(terminal.rows()[0], "kept");
    }

    #[test]
    fn colour_is_discarded_rather_than_drawn_as_grey() {
        // Five usable tones on this panel, and mid greys ghost. A red prompt
        // rendered in grey is less legible than a black one.
        let mut terminal = Terminal::new(20, 2);
        terminal.feed(b"\x1b[31mred\x1b[0m");
        assert_eq!(terminal.rows()[0], "red");
    }
}

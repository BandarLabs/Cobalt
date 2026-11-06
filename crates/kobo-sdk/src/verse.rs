//! Poems, set the way a printed book of verse sets them.
//!
//! The lines keep their own ragged ends and stand together as a block centred
//! on the page by its longest line, so a short lyric does not hang off the
//! left margin with half the panel empty beside it. The title is in the
//! reading face's italic and the poet beneath it in roman, both standing where
//! the lines stand, and the poem opens with its first letter set down into the
//! first two lines. Every page of a poem is set from one [`VerseSetting`],
//! worked out once for the whole poem, so turning a page does not move the
//! block.

use crate::{Context, ScreenBuilder};
use kobo_ui::{
    Chrome, Face, LayoutKind, ParagraphPresentation, RichTextSpan, TextPresentation, TextScale,
};

/// The air between two stanzas, in hundredths of an em: three quarters of a
/// line, which is what a printed book of verse leaves.
pub const STANZA_BREAK: u16 = 75;

/// Verse leading, as a percentage of the reading face's line height. The
/// reading face opens lines up for an hour of prose; a stanza set that way
/// reads almost double spaced.
pub const VERSE_LEADING: u16 = 88;

/// How many lines a poem's first letter is set down into.
pub const INITIAL_LINES: u8 = 2;

/// How one poem is set on this panel at this size.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct VerseSetting {
    /// Every line's indent from the column's start edge, in hundredths of an
    /// em.
    pub indent_em: u16,
}

impl VerseSetting {
    /// Centres `stanzas` as a block in the column a reading screen at `scale`
    /// has on this panel.
    #[must_use]
    pub fn measure(context: &Context, scale: TextScale, stanzas: &[&[&str]]) -> Self {
        let metrics = context.metrics();
        kobo_ui::with_text_scale(metrics.text_scale, || {
            kobo_ui::with_reading_scale(scale, || {
                // Read off a laid-out paragraph rather than worked out from the
                // margins: a reading screen keeps to a book's measure on a wide
                // panel, and that is the column the lines are set in.
                let probe = ScreenBuilder::new("verse-measure")
                    .reading(true)
                    .text_scale(scale)
                    .rich_text("M", Vec::new(), ParagraphPresentation::default())
                    .build();
                let column = probe
                    .layout_with(&metrics, &Chrome::measuring(true))
                    .nodes
                    .iter()
                    .find(|node| {
                        matches!(node.kind, LayoutKind::RichText(_)) && node.text_lines.is_empty()
                    })
                    .map_or(0, |node| node.rect.width);
                Self {
                    indent_em: kobo_ui::verse_indent(
                        stanzas,
                        column,
                        stanza_presentation(Self::default(), true, false),
                        Face::Reading,
                    ),
                }
            })
        })
    }
}

/// How one stanza is set.
fn stanza_presentation(
    setting: VerseSetting,
    opens: bool,
    after_break: bool,
) -> ParagraphPresentation {
    ParagraphPresentation {
        line_height_percent: VERSE_LEADING,
        margin_before_em: if after_break { STANZA_BREAK } else { 0 },
        start_indent_em: setting.indent_em,
        initial_lines: if opens { INITIAL_LINES } else { 0 },
        ..ParagraphPresentation::PLAIN
    }
}

/// The whole of `text` in the reading face's italic.
fn italic(text: &str) -> Vec<RichTextSpan> {
    vec![RichTextSpan {
        start: 0,
        end: text.len(),
        presentation: TextPresentation {
            emphasis: true,
            ..TextPresentation::default()
        },
    }]
}

impl ScreenBuilder {
    /// A poem's title, in italic, and its poet beneath it, standing where its
    /// lines will stand.
    #[must_use]
    pub fn verse_head(self, title: &str, poet: &str, setting: VerseSetting) -> Self {
        let line = ParagraphPresentation {
            start_indent_em: setting.indent_em,
            ..ParagraphPresentation::PLAIN
        };
        self.rich_text(title, italic(title), line).rich_text(
            poet,
            Vec::new(),
            ParagraphPresentation {
                // A line's air between the head and the poem, as a page of
                // verse leaves under its title.
                margin_after_em: 100,
                ..line
            },
        )
    }

    /// One stanza, its lines as written.
    ///
    /// `opens` sets the poem's first letter down into the stanza's first
    /// lines, for the stanza that begins the poem; `after_break` leaves a
    /// stanza break above it, for a stanza that follows another on the page.
    #[must_use]
    pub fn stanza(
        self,
        lines: &[&str],
        setting: VerseSetting,
        opens: bool,
        after_break: bool,
    ) -> Self {
        self.rich_text(
            lines.join("\n"),
            Vec::new(),
            stanza_presentation(setting, opens, after_break),
        )
    }

    /// What closes a poem, in italic under its last line: the book it came
    /// from, its year.
    #[must_use]
    pub fn verse_note(self, note: &str, setting: VerseSetting) -> Self {
        self.rich_text(
            note,
            italic(note),
            ParagraphPresentation {
                start_indent_em: setting.indent_em,
                margin_before_em: 100,
                ..ParagraphPresentation::PLAIN
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobo_ui::CLARA_BW_METRICS;

    const SONNET: [&str; 4] = [
        "That time of year thou mayst in me behold",
        "When yellow leaves, or none, or few, do hang",
        "Upon those boughs which shake against the cold,",
        "Bare ruin'd choirs, where late the sweet birds sang.",
    ];

    fn context() -> Context {
        let context = Context::default();
        assert_eq!(context.metrics(), CLARA_BW_METRICS);
        context
    }

    #[test]
    fn a_short_poem_is_centred_and_a_long_line_sets_it_from_the_margin() {
        let short: [&[&str]; 1] = [&[
            "Little Lamb, who made thee?",
            "Dost thou know who made thee?",
        ]];
        let centred = VerseSetting::measure(&context(), TextScale::default(), &short);
        assert!(centred.indent_em > 0, "{centred:?}");
        let long = "Bare ruin'd choirs ".repeat(8);
        let wide: [&[&str]; 1] = [&[long.as_str()]];
        let flush = VerseSetting::measure(&context(), TextScale::default(), &wide);
        assert_eq!(flush.indent_em, 0);
    }

    #[test]
    fn a_poem_set_from_its_setting_fits_and_keeps_every_line_whole() {
        let context = context();
        let stanzas: [&[&str]; 1] = [&SONNET];
        let setting = VerseSetting::measure(&context, TextScale::default(), &stanzas);
        let screen = ScreenBuilder::new("verse")
            .reading(true)
            .verse_head("Sonnet 73", "William Shakespeare", setting)
            .stanza(&SONNET, setting, true, false)
            .verse_note("Sonnets, 1609", setting)
            .build();
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::measuring(true));
        assert!(
            screen
                .diagnostics(&CLARA_BW_METRICS, &Chrome::measuring(true))
                .issues
                .is_empty(),
            "the poem fits"
        );
        let lines = layout
            .nodes
            .iter()
            .filter(|node| {
                matches!(node.kind, LayoutKind::RichText(_)) && !node.text_lines.is_empty()
            })
            .count();
        // Title, poet, four lines and the note: nothing wrapped.
        assert_eq!(lines, 7);
    }
}

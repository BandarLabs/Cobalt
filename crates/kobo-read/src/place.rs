//! Where the reader is, said at the foot of the page.
//!
//! # Why the page says it at all
//!
//! The place used to be given only when the controls were opened: a reader
//! who wanted to know how much of a chapter was left tapped the middle of the
//! page, read one line, and tapped again to put the panel away. That is two
//! repaints of an E Ink panel to learn a number, and it is the number a
//! reader looks for most often: whether there is time to finish the chapter.
//!
//! # Why it belongs to the bar and goes with it
//!
//! A book kept as the whole page is the words and nothing else, and a line
//! under them is not nothing. So the line is drawn for the reader who has
//! asked for the bar to stay, as the foot of the same furniture the bar is
//! the head of, and a reader who gives the page the whole panel gives it this
//! line of the panel too.
//!
//! # Why it is measured before the book is
//!
//! The line takes room, and room taken from under a page that has already
//! been set is the last line of that page. The room is taken off the area
//! the book is broken into, once, where the bar's own height is taken off
//! it, so a page is as tall as what is left and the line is drawn in what was
//! kept back. It is one line on every page or it is not drawn: a line that
//! wrapped to two would need twice the room, and only on some pages.

use kobo_sdk::ScreenBuilder;
use kobo_ui::{DisplayMetrics, Face, FontSize, ProseArea};

use crate::Reader;

/// How many of the layout's gaps the line keeps round itself.
///
/// One above and one below. The layout leaves a gap after everything it
/// places, the line included, so the lowest the line can sit is one gap
/// above the foot of the page. Keeping back that gap as well as the one
/// between the words and the line means there is always room for both, and
/// the line then sits at the same height on a page that is full and on one
/// that ends early, instead of rising and falling by a millimetre as the
/// pages turn.
const GAPS: i32 = 2;

impl Reader {
    /// The room the place line takes from the foot of every page, which is
    /// none when the page has the whole panel.
    pub(crate) fn foot_room(&self, panel: &DisplayMetrics, area: ProseArea) -> i32 {
        if self.memory.full_page {
            return 0;
        }
        // At the size the interface is set at, not the size of the book. The
        // line is furniture, like the bar above the page, and making the
        // type of a novel larger must not make its page number larger too.
        let line = kobo_ui::with_text_scale(panel.text_scale, || FontSize::Note.line_height());
        line.saturating_add(area.gap.saturating_mul(GAPS))
    }

    /// The pages a chapter starts on, in order.
    ///
    /// Worked out once for each time the book is broken into pages, because
    /// it is asked for on every page turn and a long book lists hundreds of
    /// chapters.
    pub(crate) fn chapter_pages(&self) -> Vec<usize> {
        let starts = crate::chapter_starts_of(&self.document);
        if starts.is_empty() {
            return Vec::new();
        }
        let mut found = std::collections::BTreeSet::new();
        let mut pages = Vec::new();
        for (number, page) in self.pages.iter().enumerate() {
            for piece in page {
                let block = usize::try_from(piece.block).unwrap_or(usize::MAX);
                if starts.contains(&block) && found.insert(block) && pages.last() != Some(&number) {
                    pages.push(number);
                }
            }
        }
        pages
    }

    /// How many pages of the chapter come after this one, when the book
    /// names its chapters.
    ///
    /// Counted to the next place the book's own contents point at, at any
    /// depth, because that is where the next page break with a title over it
    /// falls and so where a reader will feel the chapter has ended. The end
    /// of the book ends the last of them.
    fn pages_left_in_chapter(&self) -> Option<usize> {
        if self.chapter_starts.is_empty() {
            return None;
        }
        let next = self
            .chapter_starts
            .iter()
            .copied()
            .find(|start| *start > self.page)
            .unwrap_or(self.pages.len());
        Some(next.saturating_sub(self.page).saturating_sub(1))
    }

    /// What the foot of this page says, if it says anything.
    ///
    /// The fuller wording when it fits on one line and the page and its count
    /// when it does not, which is what the largest interface sizes on the
    /// narrowest panel come down to. A document of one page has no place
    /// worth stating and states none.
    pub(crate) fn place_line(&self) -> Option<String> {
        if self.foot == 0 {
            return None;
        }
        let (page, total) = self.place_in_book()?;
        let place = format!("Page {page} of {total}");
        let fuller = self.pages_left_in_chapter().map(|left| match left {
            0 => format!("{place} \u{b7} Last page of the chapter"),
            1 => format!("{place} \u{b7} 1 page left in the chapter"),
            left => format!("{place} \u{b7} {left} pages left in the chapter"),
        });
        fuller
            .into_iter()
            .chain(std::iter::once(place))
            .find(|line| self.fits_the_foot(line))
    }

    /// Whether `line` is drawn as a single line in the room kept for it.
    fn fits_the_foot(&self, line: &str) -> bool {
        kobo_ui::with_text_scale(self.panel.text_scale, || {
            kobo_ui::wrap_text_in(line, self.foot_width, FontSize::Note, Face::Text).len() == 1
        })
    }

    /// Sets the place at the foot of a page being drawn.
    ///
    /// Pushed to the foot rather than left under the last paragraph, so it is
    /// in the same spot on the page a chapter ends on as on a page of solid
    /// prose, where an eye that has learned to find it will look.
    pub(crate) fn with_place(&self, screen: ScreenBuilder) -> ScreenBuilder {
        match self.place_line() {
            Some(line) => screen.fill().secondary(line),
            None => screen,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{action, Memory};
    use kobo_doc::{Block, Contents, Document};
    use kobo_ui::Node;

    fn panel() -> DisplayMetrics {
        DisplayMetrics::default()
    }

    fn kept() -> Memory {
        Memory {
            full_page: false,
            ..Memory::default()
        }
    }

    fn prose(paragraphs: usize) -> Vec<Block> {
        (0..paragraphs)
            .map(|n| {
                Block::Paragraph(format!(
                    "Paragraph {n} of a letter that runs on for page after page about the river."
                ))
            })
            .collect()
    }

    /// Two chapters, each several pages long.
    fn chaptered() -> Document {
        let mut blocks = vec![Block::Heading {
            level: 1,
            text: "One".into(),
        }];
        blocks.extend(prose(60));
        let second = blocks.len();
        blocks.push(Block::Heading {
            level: 1,
            text: "Two".into(),
        });
        blocks.extend(prose(60));
        Document {
            contents: vec![
                Contents {
                    title: "One".into(),
                    block: 0,
                    depth: 0,
                },
                Contents {
                    title: "Two".into(),
                    block: second,
                    depth: 0,
                },
            ],
            blocks,
            ..Document::default()
        }
    }

    /// The last muted line on the page, which is where the place is set.
    fn foot(reader: &Reader) -> Option<String> {
        match reader.screen("A Book").nodes.last() {
            Some(Node::Secondary { text, .. }) => Some(text.clone()),
            _ => None,
        }
    }

    #[test]
    fn a_book_with_chapters_counts_down_the_pages_left_in_each_and_names_its_last() {
        let mut reader = Reader::open(chaptered(), kept(), &panel());
        let pages = reader.page_count();
        let second = reader.chapter_starts[1];
        assert!(
            second > 2 && pages > second + 2,
            "chapters of several pages"
        );
        for page in 0..pages {
            let end = if page < second { second } else { pages };
            let left = end - page - 1;
            let wanted = match left {
                0 => format!(
                    "Page {} of {pages} \u{b7} Last page of the chapter",
                    page + 1
                ),
                1 => format!(
                    "Page {} of {pages} \u{b7} 1 page left in the chapter",
                    page + 1
                ),
                left => format!(
                    "Page {} of {pages} \u{b7} {left} pages left in the chapter",
                    page + 1
                ),
            };
            assert_eq!(foot(&reader), Some(wanted));
            reader.act(action::FORWARD, &panel());
        }
    }

    #[test]
    fn a_document_with_no_chapters_says_the_page_and_the_count_and_nothing_it_cannot_know() {
        let document = Document {
            blocks: prose(80),
            ..Document::default()
        };
        let reader = Reader::open(document, kept(), &panel());
        assert_eq!(
            foot(&reader),
            Some(format!("Page 1 of {}", reader.page_count()))
        );
    }

    #[test]
    fn one_page_has_no_place_worth_stating() {
        let document = Document {
            blocks: prose(2),
            ..Document::default()
        };
        let reader = Reader::open(document, kept(), &panel());
        assert_eq!(reader.page_count(), 1);
        assert_eq!(foot(&reader), None);
    }

    #[test]
    fn the_whole_page_keeps_no_room_for_a_place_and_says_none() {
        let whole = Reader::open(chaptered(), Memory::default(), &panel());
        assert_eq!(whole.foot, 0);
        assert_eq!(foot(&whole), None);
        let bar_kept = Reader::open(chaptered(), kept(), &panel());
        assert!(bar_kept.foot > 0);
        assert!(bar_kept.page_count() >= whole.page_count());
    }

    #[test]
    fn a_panel_too_narrow_for_the_fuller_line_says_the_page_and_the_count_alone() {
        let narrow = DisplayMetrics {
            width: 420,
            ..panel()
        };
        let reader = Reader::open(chaptered(), kept(), &narrow);
        let said = foot(&reader).expect("a narrow page still says where it is");
        assert_eq!(said, format!("Page 1 of {}", reader.page_count()));
    }

    #[test]
    fn the_place_stays_under_a_notice_that_was_measured_for_and_gives_way_to_one_that_was_not() {
        let mut reader = Reader::open(chaptered(), kept(), &panel());
        // Turning back from the first page raises a notice the page is
        // measured again for, so the place is still under it.
        reader.act(action::BACK, &panel());
        let screen = reader.screen("A Book");
        assert!(screen
            .nodes
            .iter()
            .any(|node| matches!(node, Node::Banner { .. })));
        assert!(foot(&reader).is_some_and(|line| line.starts_with("Page 1 of ")));

        // A copy that stops short says so on its last page, in room nothing
        // kept for it, and the place is not drawn under that.
        reader.mark_truncated(true);
        while reader.can_go_forward() {
            reader.act(action::FORWARD, &panel());
        }
        let last = reader.screen("A Book");
        assert!(matches!(last.nodes.last(), Some(Node::Banner { .. })));
    }
}

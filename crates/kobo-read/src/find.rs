//! Finding words in a book, and going to where they are.
//!
//! # Why the book is searched a little at a time
//!
//! A search reads every block of the book, and a long novel is a megabyte of
//! them. On the development machine that is over before the screen is drawn;
//! on the reader, which is some twenty times slower and gives every callback a
//! quarter of a second, it is long enough for the watchdog to conclude the
//! application has hung. So the search is a thing that can be put down and
//! picked up: [`Reader::search_on`] reads blocks until the time it was given
//! runs out and says whether there is more to read, and whatever owns the
//! clock calls it again. Between calls the screen says it is searching, which
//! is true, and how far through the book it has got, which it knows.
//!
//! # Why a result is a place and not a page
//!
//! What is kept for each match is the block it is in and where in that block
//! the words start, the same two numbers every other remembered place is kept
//! as. The page a result is listed under is worked out when the list is
//! drawn, so a reader who makes the type larger and opens the results again
//! is given the pages as they are now.

use std::time::{Duration, Instant};

use kobo_sdk::keyboard::{TextEntry, Typing};
use kobo_sdk::{Screen, ScreenBuilder};
use kobo_ui::{DisplayMetrics, Face};

use crate::{
    action, highlighted_spans, Chrome, Entry, List, Locator, Outcome, Piece, Reader, SearchHit,
    MAX_SEARCH_QUERY_BYTES,
};

/// The most places one search lists.
///
/// Set by the list rather than by the search. Reading a megabyte of book for
/// a word takes three milliseconds on the development machine, which is some
/// sixty on the reader; cutting the results into pages that each show whole
/// takes a tenth of a millisecond a row, and it has to be done for the whole
/// list before the first page of it can say how many pages there are. Two
/// hundred and fifty-six rows came to half a second of the reader's time in
/// one callback that is allowed a quarter. Sixty-four is an eighth of a
/// second, and a search that finds more than that is one to narrow rather
/// than to page through.
pub(crate) const MAX_FOUND: usize = 64;

/// How often the searching screen is worth drawing again, in parts of the
/// book.
///
/// Quarters. The panel takes most of a second to repaint, so a bar that moved
/// with every pass would spend longer being drawn than the search spends
/// searching; four steps is enough to show a long book is being got through.
const PROGRESS_STEPS: usize = 4;

/// One search of the open book.
#[derive(Clone, Debug, Default)]
pub(crate) struct Find {
    /// The field the words are typed into, which the runtime draws.
    entry: TextEntry,
    /// The words being looked for, as they were typed.
    query: String,
    hits: Vec<SearchHit>,
    /// The first block the search has still to read.
    next: usize,
    /// How far the search has got.
    phase: Phase,
    /// The result the reader last went to, so the list opens on it again.
    chosen: usize,
}

/// What a search is doing.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Phase {
    /// Nothing: no search was asked for, or the last one is finished.
    #[default]
    Idle,
    /// Reading the book through for the words.
    Reading,
    /// Every block has been read and the results have still to be cut into
    /// pages.
    ///
    /// A step of its own because both halves take real time on the reader.
    /// Done in the same pass, the last of the reading and the whole of the
    /// measuring landed in one callback, and that is the callback with the
    /// least room to spare.
    Listing,
}

impl Find {
    pub(crate) fn count(&self) -> usize {
        self.hits.len()
    }

    /// Closes the field if it is open, keeping what was last found.
    pub(crate) fn put_away(&mut self) {
        if self.entry.is_open() {
            self.entry.close();
        }
    }
}

impl Reader {
    /// Whether a search of the book is under way and wants more time.
    ///
    /// What the owner of the clock asks before it arranges to be called
    /// again.
    #[must_use]
    pub fn is_searching(&self) -> bool {
        self.find.phase != Phase::Idle
    }

    /// Carries the search on for about `budget`, and answers whether the
    /// screen is worth drawing again.
    ///
    /// The clock is read after each block rather than during one, so a pass
    /// overruns by at most the block it was on, and a pass given no time at
    /// all still reads one and the search always ends. The reading stops at
    /// the end of the book or at the most results a list will hold, whichever
    /// is first, and the pass after that one cuts the list into pages.
    pub fn search_on(&mut self, budget: Duration, panel: &DisplayMetrics) -> bool {
        match self.find.phase {
            Phase::Idle => return false,
            Phase::Listing => {
                self.find.phase = Phase::Idle;
                if self.chrome == Chrome::Found {
                    self.open_list(panel);
                }
                return self.chrome == Chrome::Found;
            }
            Phase::Reading => {}
        }
        let query = self.find.query.to_lowercase();
        let blocks = self.document.blocks.len();
        let step_before = self.search_step();
        let until = Instant::now().checked_add(budget);
        while self.find.next < blocks && self.find.hits.len() < MAX_FOUND {
            if let Some(hit) = self.hit_in(self.find.next, &query) {
                self.find.hits.push(hit);
            }
            self.find.next += 1;
            if until.is_some_and(|until| Instant::now() >= until) {
                break;
            }
        }
        if self.find.next >= blocks || self.find.hits.len() >= MAX_FOUND {
            self.find.phase = Phase::Listing;
            return false;
        }
        self.chrome == Chrome::Found && self.search_step() != step_before
    }

    /// How far through the book the search is, in [`PROGRESS_STEPS`].
    fn search_step(&self) -> usize {
        let blocks = self.document.blocks.len().max(1);
        self.find.next.min(blocks) * PROGRESS_STEPS / blocks
    }

    /// Opens the search: what was last found if anything was asked for, the
    /// field to ask in if not.
    pub(crate) fn find_action(&mut self, name: &str, panel: &DisplayMetrics) -> Outcome {
        if name == action::FIND_AGAIN || self.find.query.is_empty() {
            // The words last looked for are put back in the field, so a
            // search that found too much is narrowed rather than retyped.
            self.find.entry.open_with(self.find.query.clone());
            self.set_chrome(Chrome::Find, panel);
        } else {
            self.set_chrome(Chrome::Found, panel);
        }
        Outcome::Repaint
    }

    /// Offers `action` to the search field while it is open.
    pub(crate) fn find_typed(
        &mut self,
        action: kobo_ui::ActionId,
        panel: &DisplayMetrics,
    ) -> Option<Outcome> {
        if self.chrome != Chrome::Find {
            return None;
        }
        match self.find.entry.handle(action)? {
            Typing::Changed => {}
            Typing::Submitted(query) => self.begin_search(&query, panel),
            // Backing out of the field goes back to what it was opened over:
            // the results of the last search when there was one, and the page
            // when there was not.
            _ if self.find.query.is_empty() => self.set_chrome(Chrome::Hidden, panel),
            _ => self.set_chrome(Chrome::Found, panel),
        }
        Some(Outcome::Repaint)
    }

    /// Starts reading the book through for `query`, from its first block.
    ///
    /// Nothing is read here. The first pass is taken when whatever owns the
    /// clock next calls [`Reader::search_on`], so the screen that says the
    /// book is being searched is on the panel before the searching starts.
    pub(crate) fn begin_search(&mut self, query: &str, panel: &DisplayMetrics) {
        self.find.put_away();
        self.find.query = bounded_query(query);
        self.find.hits.clear();
        self.find.next = 0;
        self.find.chosen = 0;
        self.find.phase = if self.find.query.is_empty() {
            Phase::Idle
        } else {
            Phase::Reading
        };
        self.set_chrome(Chrome::Found, panel);
    }

    /// Goes to one of the places found, leaving a way back to this page.
    pub(crate) fn go_to_found(&mut self, name: &str, panel: &DisplayMetrics) -> Outcome {
        let Some((index, hit)) = name
            .strip_prefix(action::FOUND)
            .and_then(|index| index.parse::<usize>().ok())
            .and_then(|index| Some((index, self.find.hits.get(index)?)))
        else {
            return Outcome::Elsewhere;
        };
        let (block, offset, length) = (hit.at, hit.offset, hit.length);
        self.find.chosen = index;
        self.jump_to(block, offset, panel);
        // Marked on the page it lands on, because a page is several hundred
        // words and the reader was promised a particular few of them.
        self.found = Some((block, offset, offset.saturating_add(length)));
        Outcome::Save
    }

    /// A piece's spans, with the words a search landed on marked in it.
    ///
    /// Worked out as the page is drawn rather than written into the pages, so
    /// it costs nothing to take off again when the page is turned. A mark is
    /// a tone behind the letters and moves none of them, which is why it can
    /// be laid over a page that has already been measured.
    pub(crate) fn spans_showing_found(
        &self,
        piece: &Piece,
        piece_from: u32,
    ) -> Vec<kobo_ui::RichTextSpan> {
        let Some((_, from, to)) = self.found.filter(|(block, ..)| *block == piece.block) else {
            return piece.spans.clone();
        };
        let start = usize::try_from(piece_from).unwrap_or(usize::MAX);
        let end = start.saturating_add(piece.text.len());
        let from = usize::try_from(from).unwrap_or(usize::MAX).max(start);
        let to = usize::try_from(to).unwrap_or(0).min(end);
        if from >= to {
            return piece.spans.clone();
        }
        highlighted_spans(&piece.spans, &piece.text, &[(from - start, to - start)])
    }

    /// The field the words to find are typed into.
    pub(crate) fn find_screen(&self, title: &str) -> Screen {
        ScreenBuilder::new("reader-find")
            .top_bar(kobo_ui::drawable_in(title, Face::Text))
            .text_entry_with_hint(
                &self.find.entry,
                "Find",
                "A word or a few words, as they are written here.",
                "Find",
            )
            .build()
    }

    /// What is drawn while the book is still being read through.
    pub(crate) fn searching_screen(&self, title: &str) -> Screen {
        let blocks = self.document.blocks.len().max(1);
        let read = self.find.next.min(blocks) * 100 / blocks;
        ScreenBuilder::new("reader-found")
            .top_bar(kobo_ui::drawable_in(title, Face::Text))
            .activity(
                format!("Searching for {}", quoted(&self.find.query)),
                Some(u8::try_from(read).unwrap_or(100)),
            )
            .action_bar(SEARCH_BAR)
            .build()
    }

    /// Every place the words were found, each a way to it.
    pub(crate) fn found_list(&self) -> List {
        let asked = quoted(&self.find.query);
        let count = self.find.hits.len();
        let intro = match count {
            0 => format!("Nothing here matches {asked}."),
            1 => format!("{asked} is in 1 passage."),
            // The reading stops when the list is as long as a list should
            // be, and saying "64 passages" of a word that is in two thousand
            // would be a count of the list passed off as a count of the book.
            _ if count >= MAX_FOUND => {
                format!("{asked} is in more than {count} passages. These are the first.")
            }
            _ => format!("{asked} is in {count} passages."),
        };
        let entries = self
            .find
            .hits
            .iter()
            .enumerate()
            .map(|(index, hit)| {
                Entry::row(
                    format!("{}{index}", action::FOUND),
                    &hit.excerpt,
                    &self.whereabouts(hit.at, hit.offset),
                    u16::try_from(index + 1).unwrap_or(u16::MAX),
                )
            })
            .collect();
        List {
            name: "reader-found",
            intro: Some(kobo_ui::drawable_in(&intro, Face::Text).into_owned().into()),
            entries,
            bar: SEARCH_BAR.to_vec(),
            current: self.find.chosen,
        }
    }

    /// Where a place is, said the way a reader would look for it: the page,
    /// and the chapter when the book names its chapters.
    fn whereabouts(&self, block: Locator, within: u32) -> String {
        let page = self.page_holding_place(block, within) + 1;
        let at = usize::try_from(block).unwrap_or(usize::MAX);
        match self
            .document
            .contents
            .iter()
            .rfind(|entry| entry.block <= at)
        {
            Some(chapter) => format!("Page {page} \u{b7} {}", chapter.title.trim()),
            None => format!("Page {page}"),
        }
    }
}

/// The verbs under a search, the same while it runs as when it has finished,
/// so the band does not change under a finger already on its way to it.
const SEARCH_BAR: [(&str, &str); 2] = [
    (action::BOOK, "Back to the book"),
    (action::FIND_AGAIN, "Search again"),
];

/// What was typed, cut to the longest query the scanner will run.
///
/// Cut rather than refused: a pasted sentence still finds its opening words,
/// and a search that silently found nothing because it was a few letters too
/// long would read as the words not being in the book.
fn bounded_query(query: &str) -> String {
    let mut end = query.len().min(MAX_SEARCH_QUERY_BYTES);
    while !query.is_char_boundary(end) {
        end -= 1;
    }
    query[..end].trim().to_owned()
}

/// The words looked for, set off the way speech is.
fn quoted(query: &str) -> String {
    format!("\u{201c}{query}\u{201d}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Memory;
    use kobo_doc::{Block, Contents, Document};
    use kobo_ui::Node;

    fn panel() -> DisplayMetrics {
        DisplayMetrics::default()
    }

    /// A book of two chapters, long enough that each runs to several pages,
    /// with one word that is only in the second.
    fn book() -> Reader {
        let mut blocks = vec![Block::Heading {
            level: 1,
            text: "The Weir".into(),
        }];
        blocks.extend((0..60).map(|n| {
            Block::Paragraph(format!(
                "Paragraph {n}: the river ran slow under the alders and nothing hurried it."
            ))
        }));
        blocks.push(Block::Heading {
            level: 1,
            text: "The Mill".into(),
        });
        blocks.extend((0..60).map(|n| {
            Block::Paragraph(format!(
                "Paragraph {n}: the wheel turned and the Heron watched it from the far bank."
            ))
        }));
        let document = Document {
            contents: vec![
                Contents {
                    title: "The Weir".into(),
                    block: 0,
                    depth: 0,
                },
                Contents {
                    title: "The Mill".into(),
                    block: 61,
                    depth: 0,
                },
            ],
            blocks,
            ..Document::default()
        };
        Reader::open(document, Memory::default(), &panel())
    }

    /// Opens the field and submits `query`, as the runtime's keyboard would.
    fn search(reader: &mut Reader, query: &str) {
        reader.act(action::FIND_AGAIN, &panel());
        assert_eq!(reader.chrome(), Chrome::Find);
        reader.begin_search(query, &panel());
    }

    fn finish(reader: &mut Reader) -> usize {
        let mut passes = 0;
        while reader.is_searching() {
            reader.search_on(Duration::ZERO, &panel());
            passes += 1;
            assert!(passes < 10_000, "the search never finished");
        }
        passes
    }

    fn rows(screen: &Screen) -> Vec<(String, String)> {
        screen
            .nodes
            .iter()
            .filter_map(|node| match node {
                Node::Rows { rows, .. } => Some(rows),
                _ => None,
            })
            .flatten()
            .map(|row| (row.title.clone(), row.summary.clone()))
            .collect()
    }

    fn said(screen: &Screen) -> Vec<String> {
        screen
            .nodes
            .iter()
            .filter_map(|node| match node {
                Node::Secondary { text, .. } => Some(text.clone()),
                Node::Activity { label, .. } => Some(label.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_controls_offer_a_search_and_the_first_one_opens_the_field() {
        let mut reader = book();
        reader.act(action::CONTROLS, &panel());
        assert_eq!(
            reader.act_on(kobo_sdk::action_id(action::FIND), &panel()),
            Outcome::Repaint
        );
        assert_eq!(reader.chrome(), Chrome::Find);
        let screen = reader.screen("A Book");
        assert!(matches!(
            screen.nodes.first(),
            Some(Node::Heading { text, .. }) if text == "Find"
        ));
        // Backing out with nothing ever searched for goes back to the page.
        reader.act_on(kobo_ui::ActionId::BACK, &panel());
        assert_eq!(reader.chrome(), Chrome::Hidden);
    }

    #[test]
    fn a_search_is_carried_across_many_passes_and_says_it_is_searching_until_the_last() {
        let mut reader = book();
        search(&mut reader, "heron");
        assert!(reader.is_searching());
        let waiting = reader.screen("A Book");
        assert_eq!(said(&waiting), ["Searching for \u{201c}heron\u{201d}"]);
        assert!(rows(&waiting).is_empty(), "nothing is listed half found");

        // With no time at all a pass still reads one block, so the search
        // always ends: a pass for every block, and one to cut the list.
        let passes = finish(&mut reader);
        assert_eq!(passes, reader.document().blocks.len() + 1);
        let found = reader.screen("A Book");
        assert_eq!(said(&found), ["\u{201c}heron\u{201d} is in 60 passages."]);
        assert!(!rows(&found).is_empty());
    }

    #[test]
    fn only_the_last_pass_and_each_quarter_of_the_book_ask_for_a_repaint() {
        let mut reader = book();
        search(&mut reader, "heron");
        let mut repaints = 0;
        while reader.is_searching() {
            if reader.search_on(Duration::ZERO, &panel()) {
                repaints += 1;
            }
        }
        assert_eq!(
            repaints, PROGRESS_STEPS,
            "three quarters passed and the end, and nothing between"
        );
    }

    #[test]
    fn each_result_shows_the_words_in_context_with_its_page_and_chapter() {
        let mut reader = book();
        search(&mut reader, "HERON");
        finish(&mut reader);
        let screen = reader.screen("A Book");
        let (context, place) = &rows(&screen)[0];
        assert!(
            context.contains("the Heron watched"),
            "the words are shown as the book writes them: {context}"
        );
        assert!(
            place.starts_with("Page ") && place.ends_with("\u{b7} The Mill"),
            "a result says where it is: {place}"
        );
        assert!(crate::shows_whole(&screen, &panel()));
    }

    #[test]
    fn going_to_a_result_marks_the_words_on_their_page_and_back_returns_to_the_page_left() {
        let mut reader = book();
        reader.act(action::FORWARD, &panel());
        let left = reader.page_number();
        search(&mut reader, "heron");
        finish(&mut reader);
        assert_eq!(
            reader.act_on(kobo_sdk::action_id("reader-found-3"), &panel()),
            Outcome::Save
        );
        assert_eq!(reader.chrome(), Chrome::Hidden);
        assert_ne!(reader.page_number(), left);
        let marked = |reader: &Reader| {
            reader
                .screen("A Book")
                .nodes
                .iter()
                .filter_map(|node| match node {
                    Node::RichText { text, spans, .. } => Some((text.clone(), spans.clone())),
                    _ => None,
                })
                .flat_map(|(text, spans)| {
                    spans
                        .into_iter()
                        .filter(|span| span.presentation.highlighted)
                        .map(move |span| text[span.start..span.end].to_owned())
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(marked(&reader), ["Heron"]);
        assert!(
            reader.annotations().is_empty(),
            "the mark is not a highlight"
        );

        // Turning the page takes the mark off; coming back does not put it
        // on again.
        reader.act(action::FORWARD, &panel());
        reader.act(action::BACK, &panel());
        assert!(marked(&reader).is_empty());

        assert_eq!(
            reader.act_on(kobo_ui::ActionId::BACK, &panel()),
            Outcome::Save
        );
        assert_eq!(reader.page_number(), left);
    }

    #[test]
    fn the_results_are_kept_and_open_again_on_the_one_last_gone_to() {
        let mut reader = book();
        search(&mut reader, "heron");
        finish(&mut reader);
        reader.act_on(kobo_sdk::action_id("reader-found-40"), &panel());
        reader.act(action::FIND, &panel());
        assert_eq!(reader.chrome(), Chrome::Found, "the last search is kept");
        let screen = reader.screen("A Book");
        let shown: Vec<u16> = screen
            .nodes
            .iter()
            .filter_map(|node| match node {
                Node::Rows { rows, .. } => Some(rows),
                _ => None,
            })
            .flatten()
            .filter_map(|row| match row.lead {
                kobo_ui::RowLead::Number(number) => Some(number),
                _ => None,
            })
            .collect();
        assert!(
            shown.contains(&41),
            "the list opened on another page of itself: {shown:?}"
        );
    }

    #[test]
    fn words_that_are_nowhere_in_the_book_are_said_to_be_nowhere() {
        let mut reader = book();
        search(&mut reader, "kingfisher");
        finish(&mut reader);
        let screen = reader.screen("A Book");
        assert_eq!(
            said(&screen),
            ["Nothing here matches \u{201c}kingfisher\u{201d}."]
        );
        assert!(rows(&screen).is_empty());
        let bar: Vec<String> = screen
            .nav_bar
            .as_ref()
            .map(|bar| {
                bar.destinations
                    .iter()
                    .map(|item| item.label.clone())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(bar, ["Back to the book", "Search again"]);
    }

    #[test]
    fn a_word_in_nearly_every_paragraph_stops_at_what_a_list_holds_and_says_there_are_more() {
        let document = Document {
            blocks: (0..400)
                .map(|n| Block::Paragraph(format!("Line {n} of the letter.")))
                .collect(),
            ..Document::default()
        };
        let mut reader = Reader::open(document, Memory::default(), &panel());
        search(&mut reader, "letter");
        finish(&mut reader);
        assert_eq!(reader.find.count(), MAX_FOUND);
        assert_eq!(
            said(&reader.screen("A Letter")),
            ["\u{201c}letter\u{201d} is in more than 64 passages. These are the first."]
        );
    }

    #[test]
    fn every_page_of_a_long_list_of_results_fits_the_panel_at_every_text_size() {
        for scale in kobo_ui::TextScale::STEPS {
            let mut metrics = panel();
            metrics.text_scale = scale;
            let mut reader = book();
            reader.restore(Memory::default(), &metrics);
            search(&mut reader, "heron");
            while reader.is_searching() {
                reader.search_on(Duration::ZERO, &metrics);
            }
            let mut pages = 0;
            loop {
                let screen = reader.screen("A Book");
                assert!(
                    crate::shows_whole(&screen, &metrics),
                    "a page of results ran off the panel at {}%",
                    scale.percent()
                );
                pages += 1;
                let before = reader.list_page;
                reader.act(action::LIST_NEXT, &metrics);
                if reader.list_page == before {
                    break;
                }
            }
            assert!(pages > 1, "sixty results are more than one page");
        }
    }

    #[test]
    fn a_pasted_sentence_is_cut_to_the_longest_query_rather_than_finding_nothing() {
        let long = "the river ran slow under the alders ".repeat(8);
        assert!(long.len() > MAX_SEARCH_QUERY_BYTES);
        let cut = bounded_query(&long);
        assert!(cut.len() <= MAX_SEARCH_QUERY_BYTES);
        assert!(long.starts_with(&cut));
    }
}

//! What a reader kept of a book, written out so it can leave the reader.
//!
//! # Why Markdown
//!
//! A marked passage is a quotation and a note is a line under it, which is
//! what a commonplace book has always been, and Markdown writes both the way
//! a person would by hand: a mark down the margin for the book's words and
//! nothing for one's own. It opens in anything that opens text, and the
//! programs that keep notes take it as it is.
//!
//! # Why a single word is quoted in its sentence
//!
//! A highlight made by holding a word is one word. Quoted alone it is a list
//! of vocabulary with nothing to say why any of it was kept, so the sentence
//! it stood in is quoted and the word set heavier inside it, which is how it
//! looked on the page when it was marked.
//!
//! # Why no page numbers
//!
//! A page here is a consequence of the type size, and the copy is read
//! somewhere that has neither. What places a passage outside the reader is
//! the chapter it is in, which the book states and which does not move.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::{first_words, Locator, Reader};

/// The longest sentence quoted whole round a highlighted word, in bytes.
///
/// Past this the quotation is a window round the word instead. A sentence in
/// a nineteenth-century novel can run to a page, and a page quoted for the
/// sake of one word buries the word.
const LONGEST_SENTENCE: usize = 400;

/// How much is kept either side of the word when its sentence is too long.
const WINDOW: usize = 120;

impl Reader {
    /// Whether the reader kept anything in this book: a marked passage, a
    /// highlighted word, a note or a bookmark.
    #[must_use]
    pub fn has_notes(&self) -> bool {
        !self.memory.highlights.is_empty()
            || !self.memory.annotations.is_empty()
            || !self.memory.bookmarks.is_empty()
    }

    /// Everything kept in this book as one Markdown document, or `None` when
    /// nothing was.
    ///
    /// `title` names the book when the file itself does not, which is what a
    /// plain text file opened from a shelf is known by. The passages come
    /// first, in the order they are in the book and under the chapter each is
    /// in, with a note set under the passage it was written beside; the
    /// bookmarks follow, each with its chapter and the words it sits on.
    #[must_use]
    pub fn notes_markdown(&self, title: &str) -> Option<String> {
        if !self.has_notes() {
            return None;
        }
        let mut out = String::new();
        let named = self
            .document
            .title
            .as_deref()
            .map(str::trim)
            .filter(|named| !named.is_empty())
            .unwrap_or_else(|| title.trim());
        let _ = writeln!(out, "# {}", escaped(named));
        if let Some(author) = self
            .document
            .author
            .as_deref()
            .map(str::trim)
            .filter(|author| !author.is_empty())
        {
            let _ = writeln!(out, "\n{}", escaped(author));
        }

        // Keyed by where each passage starts, so marked paragraphs and
        // highlighted words come out in the order a reader met them.
        let mut passages: BTreeMap<(Locator, u32, u64), (String, Option<&str>)> = BTreeMap::new();
        for block in &self.memory.highlights {
            if let Some(text) = self.block_text(*block) {
                passages.insert((*block, 0, 0), (quoted(&escaped(text)), None));
            }
        }
        for annotation in self.memory.annotations.values() {
            let range = annotation.range;
            let Some(quote) = self.quoted_in_its_sentence(range) else {
                continue;
            };
            passages.insert(
                (range.start.block, range.start.offset, annotation.id),
                (quote, annotation.note.as_deref()),
            );
        }
        let mut chapter = None;
        for ((block, ..), (quote, note)) in passages {
            let within = self.chapter_of(block);
            if within != chapter {
                if let Some(name) = within {
                    let _ = writeln!(out, "\n## {}", escaped(name.trim()));
                }
                chapter = within;
            }
            let _ = writeln!(out, "\n{quote}");
            if let Some(note) = note.map(str::trim).filter(|note| !note.is_empty()) {
                let _ = writeln!(out, "\n{}", escaped(note));
            }
        }

        if !self.memory.bookmarks.is_empty() {
            let _ = writeln!(out, "\n## Bookmarks\n");
            for block in &self.memory.bookmarks {
                let Some(text) = self.block_text(*block) else {
                    continue;
                };
                let words = escaped(&first_words(text));
                // A bookmark on the page a chapter opens with sits on the
                // chapter's own heading, and saying its name twice over tells
                // nobody anything the first did not.
                match self.chapter_of(*block).map(|name| escaped(name.trim())) {
                    Some(name) if name != words => {
                        let _ = writeln!(out, "- {name}: {words}");
                    }
                    _ => {
                        let _ = writeln!(out, "- {words}");
                    }
                }
            }
        }
        Some(out)
    }

    fn block_text(&self, block: Locator) -> Option<&str> {
        self.document
            .blocks
            .get(usize::try_from(block).ok()?)?
            .text()
    }

    /// The chapter a block is in, when the book names its chapters.
    fn chapter_of(&self, block: Locator) -> Option<&str> {
        let at = usize::try_from(block).ok()?;
        self.document
            .contents
            .iter()
            .rfind(|entry| entry.block <= at)
            .map(|entry| entry.title.as_str())
    }

    /// A highlighted range as a quotation: the sentence it is in, with the
    /// highlighted words set heavier.
    fn quoted_in_its_sentence(&self, range: crate::TextRange) -> Option<String> {
        if range.start.block != range.end.block {
            // More than one paragraph was marked, and each is its own line of
            // the quotation, whole.
            return Some(quoted(&escaped(&self.text_in(range)?)));
        }
        let text = self.block_text(range.start.block)?;
        let from = usize::try_from(range.start.offset).ok()?;
        let to = usize::try_from(range.end.offset).ok()?;
        let marked = text.get(from..to)?;
        let (start, end, cut_before, cut_after) = sentence_round(text, from, to);
        let mut line = String::new();
        if cut_before {
            line.push('\u{2026}');
        }
        line.push_str(&escaped(text.get(start..from)?));
        // Emphasis has to hug the words it marks, so the spaces a highlight
        // took in with them are left outside it.
        let inner = marked.trim();
        if inner.is_empty() {
            line.push_str(&escaped(marked));
        } else {
            let lead = marked.len() - marked.trim_start().len();
            line.push_str(&marked[..lead]);
            let _ = write!(line, "**{}**", escaped(inner));
            line.push_str(&marked[lead + inner.len()..]);
        }
        line.push_str(&escaped(text.get(to..end)?));
        if cut_after {
            line.push('\u{2026}');
        }
        Some(quoted(line.trim()))
    }
}

/// The bounds of the sentence round `from..to` in `text`, and whether either
/// end had to be cut short of the sentence's own.
fn sentence_round(text: &str, from: usize, to: usize) -> (usize, usize, bool, bool) {
    let ends_a_sentence = |at: usize, character: char| {
        matches!(character, '.' | '!' | '?' | '\n')
            && text[at + character.len_utf8()..]
                .chars()
                .next()
                .is_none_or(|next| next.is_whitespace() || matches!(next, '"' | '\u{201d}' | '\''))
    };
    let start = text[..from]
        .char_indices()
        .rev()
        .find(|(at, character)| ends_a_sentence(*at, *character))
        .map_or(0, |(at, character)| at + character.len_utf8());
    let end = text[to..]
        .char_indices()
        .find(|(at, character)| ends_a_sentence(to + *at, *character))
        .map_or(text.len(), |(at, character)| to + at + character.len_utf8());
    let start = start + (text[start..from].len() - text[start..from].trim_start().len());
    if end - start <= LONGEST_SENTENCE {
        return (start, end, false, false);
    }
    // A window round the words, opened and closed at a space so no word is
    // cut in half.
    let mut before = from.saturating_sub(WINDOW).max(start);
    while !text.is_char_boundary(before) {
        before += 1;
    }
    if before > start {
        before += text[before..from].find(' ').map_or(0, |space| space + 1);
    }
    let mut after = (to + WINDOW).min(end);
    while !text.is_char_boundary(after) {
        after -= 1;
    }
    if after < end {
        after = to + text[to..after].rfind(' ').unwrap_or(after - to);
    }
    (before, after, before > start, after < end)
}

/// `text` as a Markdown quotation, every line of it behind the mark.
fn quoted(text: &str) -> String {
    text.lines()
        .map(|line| {
            if line.trim().is_empty() {
                ">".to_owned()
            } else {
                format!("> {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `text` with the characters Markdown would read as instructions set down as
/// themselves.
///
/// Only the ones that change what is shown wherever they stand. A book's own
/// asterisk or underscore would otherwise turn half a sentence italic in
/// whatever opens the copy, and a square bracket would go looking for a link.
fn escaped(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        if matches!(character, '\\' | '*' | '_' | '`' | '[' | ']' | '<') {
            out.push('\\');
        }
        out.push(character);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{action, Memory, TextPosition, TextRange};
    use kobo_doc::{Block, Contents, Document};
    use kobo_ui::DisplayMetrics;

    fn panel() -> DisplayMetrics {
        DisplayMetrics::default()
    }

    fn book() -> Document {
        Document {
            title: Some("The Lock Keeper's Year".into()),
            author: Some("A. Keeper".into()),
            blocks: vec![
                Block::Heading {
                    level: 1,
                    text: "Spring".into(),
                },
                Block::Paragraph(
                    "The first boat came up on a Tuesday. The keeper walked out to meet it.".into(),
                ),
                Block::Paragraph(
                    "He had spent the winter mending what the frost had found.".into(),
                ),
                Block::Heading {
                    level: 1,
                    text: "Summer".into(),
                },
                Block::Paragraph(
                    "In June the level dropped a hand's width and stayed there.".into(),
                ),
            ],
            contents: vec![
                Contents {
                    title: "Spring".into(),
                    block: 0,
                    depth: 0,
                },
                Contents {
                    title: "Summer".into(),
                    block: 3,
                    depth: 0,
                },
            ],
            ..Document::default()
        }
    }

    fn word(block: Locator, text: &str, word: &str) -> TextRange {
        let from = u32::try_from(text.find(word).expect("the word is in the text")).unwrap();
        TextRange {
            start: TextPosition {
                block,
                offset: from,
            },
            end: TextPosition {
                block,
                offset: from + u32::try_from(word.len()).unwrap(),
            },
        }
    }

    #[test]
    fn a_book_with_nothing_kept_in_it_has_no_copy_to_make() {
        let reader = Reader::open(book(), Memory::default(), &panel());
        assert!(!reader.has_notes());
        assert_eq!(reader.notes_markdown("A Book"), None);
    }

    #[test]
    fn the_copy_is_the_title_the_author_each_passage_under_its_chapter_and_the_bookmarks() {
        let mut reader = Reader::open(book(), Memory::default(), &panel());
        // A marked paragraph, a highlighted word with a note, a highlighted
        // word without one, and a bookmark on the first page.
        reader.toggle_highlight(2, &panel());
        let keeper = word(
            1,
            "The first boat came up on a Tuesday. The keeper walked out to meet it.",
            "keeper",
        );
        reader
            .annotate(keeper, Some("the same man all year"), &panel())
            .unwrap();
        let level = word(
            4,
            "In June the level dropped a hand's width and stayed there.",
            "level",
        );
        reader.annotate(level, None, &panel()).unwrap();
        reader.go_to(0, &panel());
        reader.act(action::BOOKMARK, &panel());
        reader.memory.bookmarks.insert(4);

        let copy = reader.notes_markdown("ignored when the book names itself");
        assert_eq!(
            copy.as_deref(),
            Some(
                "# The Lock Keeper's Year\n\
                 \n\
                 A. Keeper\n\
                 \n\
                 ## Spring\n\
                 \n\
                 > The **keeper** walked out to meet it.\n\
                 \n\
                 the same man all year\n\
                 \n\
                 > He had spent the winter mending what the frost had found.\n\
                 \n\
                 ## Summer\n\
                 \n\
                 > In June the **level** dropped a hand's width and stayed there.\n\
                 \n\
                 ## Bookmarks\n\
                 \n\
                 - Spring\n\
                 - Summer: In June the level dropped a hand's width and stayed there.\n"
            )
        );
    }

    #[test]
    fn a_file_that_does_not_name_itself_is_called_what_its_shelf_calls_it() {
        let document = Document {
            blocks: vec![Block::Paragraph("One line of a letter.".into())],
            ..Document::default()
        };
        let mut reader = Reader::open(document, Memory::default(), &panel());
        reader.toggle_highlight(0, &panel());
        assert_eq!(
            reader.notes_markdown("A Letter Home").as_deref(),
            Some("# A Letter Home\n\n> One line of a letter.\n")
        );
    }

    #[test]
    fn the_books_own_asterisks_and_brackets_are_copied_as_themselves() {
        let document = Document {
            title: Some("Notes *and* [queries]".into()),
            blocks: vec![Block::Paragraph(
                "See note_3 and the * in [4] <here>.".into(),
            )],
            ..Document::default()
        };
        let mut reader = Reader::open(document, Memory::default(), &panel());
        reader.toggle_highlight(0, &panel());
        assert_eq!(
            reader.notes_markdown("x").as_deref(),
            Some(
                "# Notes \\*and\\* \\[queries\\]\n\n\
                 > See note\\_3 and the \\* in \\[4\\] \\<here>.\n"
            )
        );
    }

    #[test]
    fn a_word_in_a_sentence_as_long_as_a_page_is_quoted_in_a_window_cut_at_words() {
        let long = format!(
            "{} the heron stood {} and that was all.",
            "It was a long evening and".repeat(30),
            "in the shallows of the river".repeat(30)
        );
        let document = Document {
            blocks: vec![Block::Paragraph(long.clone())],
            ..Document::default()
        };
        let mut reader = Reader::open(document, Memory::default(), &panel());
        reader
            .annotate(word(0, &long, "heron"), None, &panel())
            .unwrap();
        let copy = reader.notes_markdown("A Book").unwrap();
        let quote = copy.lines().find(|line| line.starts_with('>')).unwrap();
        assert!(quote.contains("the **heron** stood"));
        assert!(quote.starts_with("> \u{2026}") && quote.ends_with('\u{2026}'));
        assert!(
            quote.len() < 2 * WINDOW + 40,
            "the window ran to {} bytes",
            quote.len()
        );
        let inside = quote
            .trim_start_matches("> \u{2026}")
            .trim_end_matches('\u{2026}');
        assert!(
            long.contains(&inside.replace("**", "")),
            "the window is the book's words, whole: {inside}"
        );
    }
}

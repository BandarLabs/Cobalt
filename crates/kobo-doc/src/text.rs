//! Reading a plain text file as a document.
//!
//! # What "plain text" actually means here
//!
//! Nobody sends a text file with no conventions in it. Project Gutenberg wraps
//! at seventy columns, separates paragraphs with a blank line, centres chapter
//! headings, indents verse, and wraps the whole thing in a licence header and
//! footer. A reader that treats all of that as one undifferentiated stream of
//! sentences produces exactly what gutenbird produces today: a wall.
//!
//! So this is not "split on blank lines". It recognises the handful of things
//! a text file uses to mean something, and each one is recognised by a rule
//! that would rather miss than guess wrong — a false heading in the middle of
//! a novel is far more jarring than a missed one.

use crate::{normalise_breaks, Block, Builder, Document, GUTENBERG_START};

/// A line no longer than this, alone between blank lines, may be a heading.
///
/// Headings in a text file are short. The ceiling is what stops an ordinary
/// one-sentence paragraph — which is also alone between blank lines — from
/// being promoted, and it is generous enough for "CHAPTER XVII. THE PIT AND
/// THE PENDULUM".
const HEADING_WIDTH: usize = 60;

/// How many lines of a block have to be indented before it is treated as
/// verse rather than as a paragraph somebody indented the first line of.
const VERSE_LINES: usize = 2;

/// The most groups of lines one bracketed aside may be split across.
///
/// Bounds the damage from an opening bracket that never closes: without it, a
/// stray `[` in the front matter swallows the first chapter.
const MAX_ASIDE_GROUPS: usize = 16;

/// Reads a plain text file.
#[must_use]
pub fn parse(source: &str) -> Document {
    let source = normalise_breaks(source);
    let mut builder = Builder::new();
    let body = read_gutenberg_header(&source, &mut builder);
    for group in asides(paragraphs(body)) {
        push_group(&mut builder, &group);
    }
    builder.finish()
}

/// Reads the header of a Gutenberg file, and hands back the rest.
///
/// Only the *header* is dealt with here, because the title and the author are
/// in it as `Field: value` lines and would otherwise be lost. Cutting the
/// licence itself happens later, on blocks, where it works for every format
/// rather than only this one.
fn read_gutenberg_header<'a>(source: &'a str, builder: &mut Builder) -> &'a str {
    let Some(start) = source.find(GUTENBERG_START) else {
        return source;
    };
    read_header(&source[..start], builder);
    source
}

/// Takes `Title:` and `Author:` out of a Gutenberg header.
///
/// The header is a list of `Field: value` lines. Only these two are wanted:
/// the rest are release dates, encodings and credits, which belong to the file
/// rather than to the book.
fn read_header(header: &str, builder: &mut Builder) {
    for line in header.lines() {
        if let Some(title) = line.strip_prefix("Title:") {
            builder.set_title(title);
        } else if let Some(author) = line.strip_prefix("Author:") {
            builder.set_author(author);
        }
    }
}

/// Splits the body into groups of lines separated by blank lines.
fn paragraphs(body: &str) -> Vec<Vec<&str>> {
    let mut groups = Vec::new();
    let mut group: Vec<&str> = Vec::new();
    for line in body.lines() {
        if line.trim().is_empty() {
            if !group.is_empty() {
                groups.push(std::mem::take(&mut group));
            }
            continue;
        }
        group.push(line);
    }
    if !group.is_empty() {
        groups.push(group);
    }
    groups
}

/// Rejoins a bracketed aside that blank lines split into several groups.
///
/// Project Gutenberg writes an illustration as `[Illustration: caption]`, and
/// the caption is often several centred lines with blank lines between them.
/// Read as ordinary groups that is four blocks — one of them the bare word
/// `[Illustration:`, another a lone `]` — scattered through the front matter
/// and, worse, through the chapter headings, where the stray bracket ends up
/// in the table of contents.
///
/// Only a group that *starts* with a bracket is joined, so a sentence
/// containing one is not merged with the paragraph after it.
fn asides(groups: Vec<Vec<&str>>) -> Vec<Vec<&str>> {
    let mut out: Vec<Vec<&str>> = Vec::with_capacity(groups.len());
    let mut waiting: Option<Vec<&str>> = None;
    let mut joined = 0;
    for group in groups {
        match &mut waiting {
            Some(open) => {
                open.extend(group);
                joined += 1;
                if balanced(open) || joined >= MAX_ASIDE_GROUPS {
                    out.push(waiting.take().unwrap_or_default());
                }
            }
            None => {
                if group
                    .first()
                    .is_some_and(|line| line.trim_start().starts_with('['))
                    && !balanced(&group)
                {
                    waiting = Some(group);
                    joined = 0;
                } else {
                    out.push(group);
                }
            }
        }
    }
    // An aside that never closed is still text somebody wrote.
    out.extend(waiting);
    out
}

/// Whether every bracket opened in these lines is also closed.
fn balanced(group: &[&str]) -> bool {
    let mut depth = 0i32;
    for line in group {
        for character in line.chars() {
            match character {
                '[' => depth += 1,
                ']' => depth -= 1,
                _ => {}
            }
        }
    }
    depth <= 0
}

/// Removes the underscores Project Gutenberg italicises with.
///
/// A plain text file has no italics, so Gutenberg writes `_thus_`, and left
/// alone they are drawn as underscores in the middle of the prose — the
/// running heads come out as `_Reading Jane\u{2019}s Letters._ _Chap 34._`.
///
/// An underscore between two alphanumeric characters is part of a word — an
/// identifier, a filename — and is kept. One at either edge of a word is a
/// marker and goes. That rule needs no matching pairs, so an unclosed marker
/// costs nothing, which matters because plenty of them are unclosed.
fn unmark(text: &str) -> String {
    if !text.contains('_') {
        return text.to_owned();
    }
    let characters: Vec<char> = text.chars().collect();
    characters
        .iter()
        .enumerate()
        .filter(|(at, character)| {
            if **character != '_' {
                return true;
            }
            let before = at.checked_sub(1).and_then(|at| characters.get(at));
            let after = characters.get(at + 1);
            let inside = |edge: Option<&char>| edge.is_some_and(|c| c.is_alphanumeric());
            inside(before) && inside(after)
        })
        .map(|(_, character)| *character)
        .collect()
}

/// Decides what one group of lines is, and adds it.
fn push_group(builder: &mut Builder, group: &[&str]) {
    if let Some(rule) = is_a_rule(group) {
        builder.push(rule);
        return;
    }
    if let Some(heading) = is_a_heading(group) {
        builder.push(match heading {
            Block::Heading { level, text } => Block::Heading {
                level,
                text: unmark(&text),
            },
            other => other,
        });
        return;
    }
    if is_an_aside(group) {
        push_aside(builder, group);
        return;
    }
    if is_verse(group) {
        // Kept line for line. Verse re-wrapped to the panel is prose with odd
        // capitalisation, and a table re-wrapped is nonsense. Left unmarked
        // too: an underscore here may be a rule somebody drew.
        builder.push(Block::Preformatted(group.join("\n")));
        return;
    }
    builder.push(Block::Paragraph(unmark(&group.join(" "))));
}

/// A line of nothing but punctuation is a divider.
///
/// Text files mark a scene break with `* * *` or a row of asterisks. Read as a
/// paragraph it is a line of asterisks in the middle of a novel.
fn is_a_rule(group: &[&str]) -> Option<Block> {
    let [line] = group else {
        return None;
    };
    let line = line.trim();
    // Three characters at least, so an ellipsis on a line of its own — which
    // is dialogue — is not mistaken for a divider.
    if line.len() >= 3
        && line
            .chars()
            .all(|character| matches!(character, '*' | '-' | '=' | '~' | '_' | ' ' | '.' | '#'))
        && line.chars().any(|character| character != '.')
    {
        return Some(Block::Rule);
    }
    None
}

/// Whether one group of lines is a heading, and at what level.
///
/// Three signals, and one alone is not enough except for the strongest:
///
/// * One line, short, and in capitals. This is how a text file writes a
///   chapter title and there is very little else it could be.
/// * One line, short, ending in no sentence punctuation, and beginning with a
///   word that names a division — "Chapter", "Book", "Part", "Canto".
///
/// A short line that is neither is left as a paragraph. A one-line paragraph
/// is common — a line of dialogue, an exclamation — and turning those into
/// headings would scatter chapter breaks through the middle of every scene.
fn is_a_heading(group: &[&str]) -> Option<Block> {
    let [line] = group else {
        return None;
    };
    let line = line.trim();
    if line.is_empty() || line.chars().count() > HEADING_WIDTH {
        return None;
    }
    let has_letters = line.chars().any(char::is_alphabetic);
    if !has_letters {
        return None;
    }
    let shouted = line
        .chars()
        .filter(|character| character.is_alphabetic())
        .all(char::is_uppercase);
    if shouted {
        return Some(Block::Heading {
            level: 1,
            text: line.to_owned(),
        });
    }
    // A full stop is allowed, and nothing else is. "Chapter I." and
    // "Chapter 17." are how most books write it, and rejecting them cost the
    // first chapter of a real book its heading. The other marks stay
    // disqualifying: a line ending in a comma or a question mark is a
    // sentence, and one ending in a quotation mark is dialogue.
    if line.ends_with(['!', '?', ',', ';', ':', '"']) {
        return None;
    }
    let words: Vec<&str> = line.split_whitespace().collect();
    // "Chapter four was the longest of them all." also begins with the word.
    // A real division heading is a name and a number.
    if words.len() > 4 {
        return None;
    }
    let names_a_division = [
        "chapter", "book", "part", "canto", "volume", "act", "scene", "letter",
    ]
    .iter()
    .any(|word| {
        words
            .first()
            .unwrap_or(&"")
            .trim_end_matches('.')
            .eq_ignore_ascii_case(word)
    });
    names_a_division.then(|| Block::Heading {
        level: 2,
        text: line.to_owned(),
    })
}

/// Adds a rejoined `[Illustration: …]`, and the chapter title inside it.
///
/// Its line breaks were the caption being centred, so they go. But Gutenberg
/// draws the opening of a chapter as an illustration whose *last* line is the
/// chapter title — `[Illustration: ·PRIDE AND PREJUDICE· … Chapter I.]` — and
/// folding that whole thing into one paragraph loses the boundary of the first
/// chapter of the book. So a title found on the last line is lifted back out
/// and the bracket is closed around what is left.
fn push_aside(builder: &mut Builder, group: &[&str]) {
    let lines: Vec<&str> = group
        .iter()
        .copied()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let last = lines.last().copied().unwrap_or_default();
    let bare = last.trim().trim_end_matches(']').trim();
    let Some(Block::Heading { level, text }) = is_a_heading(&[bare]) else {
        builder.push(Block::Paragraph(unmark(&lines.join(" "))));
        return;
    };
    // What is left had its closing bracket on the line just taken away.
    let rest = format!("{}]", lines[..lines.len() - 1].join(" ").trim_end());
    if rest.chars().any(char::is_alphanumeric) {
        builder.push(Block::Paragraph(unmark(&rest)));
    }
    builder.push(Block::Heading {
        level,
        text: unmark(&text),
    });
}

/// Whether a group is a bracketed aside spanning more than one line.
fn is_an_aside(group: &[&str]) -> bool {
    group.len() > 1
        && group
            .first()
            .is_some_and(|line| line.trim_start().starts_with('['))
        && group
            .last()
            .is_some_and(|line| line.trim_end().ends_with(']'))
}

/// Whether a group's line breaks are the author's rather than the wrapper's.
///
/// The signal is indentation that repeats. A wrapped paragraph has every line
/// hard against the margin except possibly the first; verse, a table and an
/// address all indent every line, or indent lines unevenly. One indented line
/// is the first line of a paragraph and means nothing.
///
/// Lines much shorter than the wrap width are the second signal: a wrapper
/// fills its lines, so a group of six lines none of which reaches halfway was
/// not wrapped by one.
fn is_verse(group: &[&str]) -> bool {
    if group.len() < VERSE_LINES {
        return false;
    }
    let indented = group
        .iter()
        .skip(1)
        .filter(|line| line.starts_with([' ', '\t']))
        .count();
    if indented >= VERSE_LINES {
        return true;
    }
    let longest = group
        .iter()
        .map(|line| line.trim_end().chars().count())
        .max()
        .unwrap_or_default();
    // Every line well short of the longest, and the longest itself short:
    // nothing wrapped this.
    longest < HEADING_WIDTH / 2 && group.len() > VERSE_LINES
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wrapped_paragraph_becomes_one_paragraph() {
        // Gutenberg hard-wraps at seventy columns. Honouring those breaks
        // gives a column of ragged short lines on a panel that is narrower
        // than seventy columns to begin with.
        let document = parse(
            "It is a truth universally acknowledged, that a single man in\n\
             possession of a good fortune, must be in want of a wife.\n\
             \n\
             However little known the feelings of such a man may be.",
        );
        assert_eq!(document.blocks.len(), 2);
        assert_eq!(
            document.blocks[0],
            Block::Paragraph(
                "It is a truth universally acknowledged, that a single man in possession \
                 of a good fortune, must be in want of a wife."
                    .to_owned()
            )
        );
    }

    #[test]
    fn a_shouted_line_of_its_own_is_a_chapter() {
        let document = parse("CHAPTER ONE\n\nIt began badly.");
        assert_eq!(
            document.blocks[0],
            Block::Heading {
                level: 1,
                text: "CHAPTER ONE".to_owned()
            }
        );
    }

    #[test]
    fn a_line_of_dialogue_is_not_promoted_to_a_chapter() {
        // A one-line paragraph is common. Treating every short line as a
        // heading scatters chapter breaks through the middle of every scene,
        // which is worse than missing one.
        for line in [
            "\"Get out!\"",
            "He said nothing.",
            "And then, silence.",
            "Yes.",
        ] {
            let document = parse(&format!("Before.\n\n{line}\n\nAfter."));
            assert_eq!(
                document.blocks[1],
                Block::Paragraph(crate::collapse(line)),
                "{line:?} was mistaken for a heading"
            );
        }
    }

    #[test]
    fn a_named_division_is_a_heading_even_in_mixed_case() {
        let document = parse("Chapter 17\n\nIt began badly.");
        assert_eq!(
            document.blocks[0],
            Block::Heading {
                level: 2,
                text: "Chapter 17".to_owned()
            }
        );
    }

    #[test]
    fn a_row_of_asterisks_is_a_break_and_not_a_paragraph() {
        let document = parse("Before.\n\n* * *\n\nAfter.");
        assert_eq!(document.blocks[1], Block::Rule);
    }

    #[test]
    fn an_ellipsis_on_its_own_line_is_still_words() {
        let document = parse("Before.\n\n...\n\nAfter.");
        assert_ne!(
            document.blocks[1],
            Block::Rule,
            "a trailing-off line was read as a scene break"
        );
    }

    #[test]
    fn verse_keeps_the_line_breaks_the_poet_chose() {
        let document = parse(
            "Words before.\n\n\
             \x20   Tyger Tyger, burning bright,\n\
             \x20   In the forests of the night;\n\
             \x20   What immortal hand or eye,\n\
             \x20   Could frame thy fearful symmetry?\n",
        );
        let Block::Preformatted(text) = &document.blocks[1] else {
            panic!("verse was re-wrapped as prose: {:?}", document.blocks[1]);
        };
        assert_eq!(text.lines().count(), 4);
    }

    #[test]
    fn one_indented_first_line_is_still_a_paragraph() {
        // Indenting the first line of a paragraph is ordinary typesetting.
        // Reading it as verse would freeze the wrapping of most of the book.
        let document = parse(
            "    It is a truth universally acknowledged, that a single man in\n\
             possession of a good fortune, must be in want of a wife.",
        );
        assert!(
            matches!(document.blocks[0], Block::Paragraph(_)),
            "an ordinary indented paragraph was read as verse: {:?}",
            document.blocks[0]
        );
    }

    fn a_gutenberg_file(body: &str) -> String {
        format!(
            "The Project Gutenberg eBook of Something\n\
             \n\
             Title: Something\n\
             Author: A Person\n\
             Release date: January 1, 2000\n\
             \n\
             *** START OF THE PROJECT GUTENBERG EBOOK SOMETHING ***\n\
             \n{body}\n\
             *** END OF THE PROJECT GUTENBERG EBOOK SOMETHING ***\n\
             \n\
             This eBook is for the use of anyone anywhere in the United States.\n\
             Redistribution is subject to the terms of the licence.\n"
        )
    }

    #[test]
    fn the_licence_around_a_gutenberg_book_is_not_part_of_the_book() {
        // Without this, page one of every Gutenberg book is the same page of
        // American copyright law.
        let document = parse(&a_gutenberg_file("It began badly.\n"));
        assert_eq!(
            document.blocks,
            vec![Block::Paragraph("It began badly.".to_owned())]
        );
    }

    #[test]
    fn a_gutenberg_header_says_what_the_book_is() {
        let document = parse(&a_gutenberg_file("It began badly.\n"));
        assert_eq!(document.title.as_deref(), Some("Something"));
        assert_eq!(document.author.as_deref(), Some("A Person"));
    }

    #[test]
    fn a_file_that_is_not_from_gutenberg_is_not_cut_about() {
        let document = parse("Just a note.\n\nAnd another.");
        assert_eq!(document.blocks.len(), 2);
        assert_eq!(document.title, None);
    }

    #[test]
    fn an_unterminated_gutenberg_file_still_yields_its_book() {
        // A download cut short loses the end marker. Refusing the whole thing
        // would turn a truncated book into no book.
        let source = a_gutenberg_file("It began badly.\n");
        let cut = &source[..source.find("*** END").expect("an end marker")];
        let document = parse(cut);
        assert_eq!(
            document.blocks,
            vec![Block::Paragraph("It began badly.".to_owned())]
        );
    }

    #[test]
    fn gutenbergs_italics_are_not_drawn_as_underscores() {
        // A plain text file has no italics, so Gutenberg writes `_thus_`. Left
        // alone, the running heads of a real book come out as
        // `_Reading Jane\u{2019}s Letters._ _Chap 34._`.
        let document = parse("She was _very_ clear about _that_.");
        assert_eq!(
            document.blocks[0],
            Block::Paragraph("She was very clear about that.".to_owned())
        );
    }

    #[test]
    fn an_underscore_inside_a_word_is_part_of_the_word() {
        let document = parse("The file is named read_me_first and nothing else.");
        assert_eq!(
            document.blocks[0].text(),
            Some("The file is named read_me_first and nothing else.")
        );
    }

    #[test]
    fn an_unclosed_italic_marker_costs_nothing() {
        // Plenty of them are unclosed. A rule needing matching pairs would
        // leave the rest of the paragraph italic, or leave every marker in it.
        let document = parse("She was _very clear about that.");
        assert_eq!(
            document.blocks[0].text(),
            Some("She was very clear about that.")
        );
    }

    #[test]
    fn an_illustration_split_by_blank_lines_is_one_thing() {
        // Read as ordinary groups this is four blocks, one of them the bare
        // word `[Illustration:` and another a lone `]`.
        let document = parse(
            "Before.\n\n\
             [Illustration:\n\n\
             \x20     GEORGE ALLEN PUBLISHER\n\n\
             \x20     RUSKIN HOUSE\n\
             \x20         ]\n\n\
             After.",
        );
        assert_eq!(
            document.blocks,
            vec![
                Block::Paragraph("Before.".to_owned()),
                Block::Paragraph("[Illustration: GEORGE ALLEN PUBLISHER RUSKIN HOUSE ]".to_owned()),
                Block::Paragraph("After.".to_owned()),
            ]
        );
    }

    #[test]
    fn a_chapter_title_drawn_inside_an_illustration_is_still_a_chapter() {
        // This is how Project Gutenberg draws the opening of chapter one, and
        // folding the whole aside into one paragraph loses the boundary.
        let document = parse(
            "[Illustration: \u{b7}PRIDE AND PREJUDICE\u{b7}\n\n\n\
             Chapter I.]\n\n\
             It is a truth universally acknowledged.",
        );
        assert_eq!(
            document.blocks,
            vec![
                Block::Paragraph("[Illustration: \u{b7}PRIDE AND PREJUDICE\u{b7}]".to_owned()),
                Block::Heading {
                    level: 2,
                    text: "Chapter I.".to_owned()
                },
                Block::Paragraph("It is a truth universally acknowledged.".to_owned()),
            ]
        );
    }

    #[test]
    fn a_division_heading_may_end_in_a_full_stop() {
        // "Chapter I." is how most books write it. Rejecting it cost a real
        // book the heading of its first chapter.
        for line in ["Chapter I.", "Chapter 17.", "Book II."] {
            let document = parse(&format!("{line}\n\nWords."));
            assert!(
                matches!(document.blocks[0], Block::Heading { .. }),
                "{line:?} was not read as a heading"
            );
        }
    }

    #[test]
    fn a_sentence_that_happens_to_start_with_the_word_chapter_is_not_a_heading() {
        let document = parse("Before.\n\nChapter four was the longest of them all.\n\nAfter.");
        assert!(
            matches!(document.blocks[1], Block::Paragraph(_)),
            "a sentence was promoted to a heading: {:?}",
            document.blocks[1]
        );
    }

    #[test]
    fn an_unclosed_bracket_cannot_swallow_the_chapter_after_it() {
        let mut source = String::from("[Illustration: never closed\n\n");
        for index in 0..40 {
            source.push_str("Paragraph ");
            source.push_str(&index.to_string());
            source.push_str(".\n\n");
        }
        let document = parse(&source);
        assert!(
            document.blocks.len() > 20,
            "an unclosed bracket ate the book: {} blocks",
            document.blocks.len()
        );
    }

    #[test]
    fn nothing_here_panics_on_anything() {
        for source in [
            "",
            "\n\n\n",
            "\u{0}\u{7}\u{1b}",
            "*** START OF TH",
            "*** START OF TH\n*** END OF TH",
            &"\u{e9}".repeat(10_000),
            &"a\n".repeat(10_000),
            &" ".repeat(10_000),
        ] {
            let _ = parse(source);
        }
    }
}

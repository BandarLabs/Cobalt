use pulldown_cmark::{html, Options, Parser};

/// The sole Markdown boundary. Normalised `CommonMark` becomes `HTML`, then
/// the platform's `HTML` text renderer, so a parser replacement stays in this
/// file. A note is a document somebody asked for by name, not a feed field,
/// so it converts against the shelf's own note ceiling rather than the
/// renderer's feed ceiling: a long note reaches its final sentence.
///
/// Mirrored from `shelf::MAX_NOTE_BYTES` (kept here so the path-included test
/// module compiles without the shelf codec).
const RENDER_CEILING: usize = 512 * 1024;
/// `[[target]]` and `[[target|label]]` read as their visible text; the
/// double-bracket syntax is an editing detail, not reading material.
fn wiki_links_to_text(markdown: &str) -> String {
    let mut out = String::with_capacity(markdown.len());
    let mut rest = markdown;
    while let Some(open) = rest.find("[[") {
        out.push_str(&rest[..open]);
        if let Some(close) = rest[open + 2..].find("]]") {
            let inner = &rest[open + 2..open + 2 + close];
            let visible = inner.rsplit('|').next().unwrap_or(inner);
            out.push_str(visible);
            rest = &rest[open + 2 + close + 2..];
        } else {
            out.push_str(&rest[open..]);
            rest = "";
        }
    }
    out.push_str(rest);
    out
}

/// One piece of a note as it is read: a heading, or a paragraph of prose with
/// the wiki links set into it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Block {
    Heading {
        level: u8,
        text: String,
    },
    Body {
        text: String,
        /// Each wiki link as its target and the byte range of its words.
        links: Vec<(String, usize, usize)>,
    },
}

/// The first code point of the range that marks where a wiki link starts.
///
/// A link's words have to survive Markdown parsing and the HTML renderer to
/// be found again in the text that comes out, and neither of them touches a
/// private-use character. The link's number rides in the marker itself, so a
/// link the renderer drops (one inside an HTML comment, say) cannot shift
/// every link after it onto the wrong note.
const LINK_BASE: u32 = 0xF_0000;
const LINK_LIMIT: u32 = 0xF_FFFD;
const LINK_END: char = '\u{E001}';

/// `[[target]]` and `[[target|label]]` become their label between markers,
/// and the targets are returned in the order the markers number them.
fn mark_wiki_links(markdown: &str) -> (String, Vec<String>) {
    let mut out = String::with_capacity(markdown.len());
    let mut targets = Vec::new();
    let mut rest = markdown;
    while let Some(open) = rest.find("[[") {
        out.push_str(&rest[..open]);
        let Some(close) = rest[open + 2..].find("]]") else {
            out.push_str(&rest[open..]);
            rest = "";
            break;
        };
        let inner = &rest[open + 2..open + 2 + close];
        let visible = inner.rsplit('|').next().unwrap_or(inner);
        let target = inner.split('|').next().unwrap_or(inner).trim();
        let marker = u32::try_from(targets.len())
            .ok()
            .and_then(|index| index.checked_add(LINK_BASE))
            .filter(|code| *code <= LINK_LIMIT)
            .and_then(char::from_u32);
        match marker {
            Some(marker) if !target.is_empty() && !visible.contains('\n') => {
                out.push(marker);
                out.push_str(visible);
                out.push(LINK_END);
                targets.push(target.to_owned());
            }
            _ => out.push_str(visible),
        }
        rest = &rest[open + 2 + close + 2..];
    }
    out.push_str(rest);
    (out, targets)
}

fn link_number(character: char) -> Option<usize> {
    let code = u32::from(character);
    (LINK_BASE..=LINK_LIMIT)
        .contains(&code)
        .then(|| usize::try_from(code - LINK_BASE).ok())
        .flatten()
}

/// Takes the markers out of one paragraph and says where each link's words
/// landed in what is left.
fn unmark(text: &str, targets: &[String]) -> (String, Vec<(String, usize, usize)>) {
    let mut clean = String::with_capacity(text.len());
    let mut links = Vec::new();
    let mut open: Option<(usize, usize)> = None;
    for character in text.chars() {
        if let Some(number) = link_number(character) {
            open = Some((number, clean.len()));
        } else if character == LINK_END {
            if let Some((number, start)) = open.take() {
                if let Some(target) = targets.get(number) {
                    if start < clean.len() {
                        links.push((target.clone(), start, clean.len()));
                    }
                }
            }
        } else {
            clean.push(character);
        }
    }
    (clean, links)
}

fn markdown_options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
}

/// What the text conversion would otherwise lose of a note's structure.
///
/// The conversion keeps an item's words and drops what marked it, keeps a
/// cell's words and drops the cell, and joins a code block's lines: numbered
/// steps arrived unnumbered, a table row ran its cells together into one
/// word, and a code block was one line.
#[derive(Default)]
struct Shaping {
    /// The number each open list is on, or `None` for a bulleted one.
    lists: Vec<Option<u64>>,
    /// Held until the item's first words, so a loose item, whose words sit in
    /// a paragraph of their own, keeps its marker on the same line as them.
    held_label: Option<String>,
    cells_in_row: usize,
    in_code: bool,
}

impl Shaping {
    fn push<'a>(
        &mut self,
        event: pulldown_cmark::Event<'a>,
        pending: &mut Vec<pulldown_cmark::Event<'a>>,
    ) {
        use pulldown_cmark::{Event, Tag, TagEnd};
        match event {
            Event::Start(Tag::List(start)) => {
                self.lists.push(start);
                pending.push(Event::Start(Tag::List(start)));
            }
            Event::End(TagEnd::List(ordered)) => {
                self.lists.pop();
                pending.push(Event::End(TagEnd::List(ordered)));
            }
            Event::Start(Tag::Item) => {
                pending.push(Event::Start(Tag::Item));
                self.held_label = Some(match self.lists.last_mut() {
                    Some(Some(number)) => {
                        let label = format!("{number}. ");
                        *number += 1;
                        label
                    }
                    _ => "\u{2022} ".to_owned(),
                });
            }
            Event::Start(Tag::TableHead | Tag::TableRow) => {
                self.cells_in_row = 0;
                pending.push(Event::Html("<p>".into()));
                pending.push(event);
            }
            Event::Start(Tag::TableCell) => {
                if self.cells_in_row > 0 {
                    pending.push(Event::Text(" \u{b7} ".into()));
                }
                self.cells_in_row += 1;
                pending.push(event);
            }
            Event::Start(Tag::CodeBlock(_)) => {
                self.in_code = true;
                pending.push(event);
            }
            Event::End(TagEnd::CodeBlock) => {
                self.in_code = false;
                pending.push(event);
            }
            Event::Text(text) if self.in_code => {
                for line in text.lines() {
                    pending.push(Event::Html("<p>".into()));
                    pending.push(Event::Text(line.to_owned().into()));
                }
            }
            Event::Text(_) | Event::Code(_) if self.held_label.is_some() => {
                if let Some(label) = self.held_label.take() {
                    pending.push(Event::Text(label.into()));
                }
                pending.push(event);
            }
            other => pending.push(other),
        }
    }
}

/// The note as headings and paragraphs.
///
/// Everything that is not a heading goes through the same HTML renderer as
/// before, so lists, tables and quotes read exactly as they did; only the
/// headings are lifted out, because a heading set as one more paragraph of
/// body text is a heading nobody can find on the page. The parser sees the
/// whole note at once, so a reference link defined at the foot of the note
/// still resolves in a paragraph at the top.
pub fn blocks(markdown: &str) -> Vec<Block> {
    use pulldown_cmark::{Event, Tag, TagEnd};
    let (marked, targets) = mark_wiki_links(markdown);
    let mut blocks = Vec::new();
    let mut pending: Vec<Event<'_>> = Vec::new();
    let mut heading: Option<(u8, String)> = None;
    let mut shaping = Shaping::default();
    let flush = |pending: &mut Vec<Event<'_>>, blocks: &mut Vec<Block>| {
        if pending.is_empty() {
            return;
        }
        let mut html_out = String::new();
        html::push_html(&mut html_out, pending.drain(..));
        let text = kobo_html::to_text_within(&html_out, RENDER_CEILING);
        for paragraph in text.split("\n\n") {
            let paragraph = paragraph.replace('\n', " ");
            let (clean, links) = unmark(paragraph.trim(), &targets);
            if !clean.trim().is_empty() {
                blocks.push(Block::Body { text: clean, links });
            }
        }
    };
    for event in Parser::new_ext(&marked, markdown_options()) {
        match event {
            Event::Start(Tag::Heading { level, .. }) if heading.is_none() => {
                flush(&mut pending, &mut blocks);
                heading = Some((level as u8, String::new()));
            }
            Event::End(TagEnd::Heading(_)) if heading.is_some() => {
                let (level, text) = heading.take().expect("open heading");
                let (clean, _) = unmark(text.trim(), &targets);
                if !clean.is_empty() {
                    blocks.push(Block::Heading { level, text: clean });
                }
            }
            Event::Text(text) | Event::Code(text) if heading.is_some() => {
                if let Some((_, words)) = &mut heading {
                    words.push_str(&text);
                }
            }
            Event::SoftBreak | Event::HardBreak if heading.is_some() => {
                if let Some((_, words)) = &mut heading {
                    words.push(' ');
                }
            }
            _ if heading.is_some() => {}
            other => shaping.push(other, &mut pending),
        }
    }
    flush(&mut pending, &mut blocks);
    blocks
}

pub fn render(markdown: &str) -> String {
    let markdown = wiki_links_to_text(markdown);
    let mut html_out = String::new();
    html::push_html(
        &mut html_out,
        Parser::new_ext(&markdown, markdown_options()),
    );
    kobo_html::to_text_within(&html_out, RENDER_CEILING)
}

#[cfg(test)]
mod tests {
    use super::{blocks, Block};

    #[test]
    fn a_list_keeps_its_bullets_and_its_numbers() {
        let found = blocks("- First\n- Second\n\n1. One\n2. Two\n\n- Loose one\n\n- Loose two\n");
        let text: Vec<String> = found
            .iter()
            .map(|block| match block {
                Block::Body { text, .. } | Block::Heading { text, .. } => text.clone(),
            })
            .collect();
        let all = text.join("|");
        for expected in [
            "\u{2022} First",
            "\u{2022} Second",
            "1. One",
            "2. Two",
            "\u{2022} Loose one",
            "\u{2022} Loose two",
        ] {
            assert!(all.contains(expected), "{expected:?} missing from {all:?}");
        }
    }

    #[test]
    fn a_table_keeps_its_cells_apart_and_a_code_block_its_lines() {
        let found = blocks(
            "| Character | Book |\n| --- | --- |\n| Emma | Emma |\n\n```\nfn main() {\n    run();\n}\n```\n",
        );
        let text: Vec<String> = found
            .iter()
            .map(|block| match block {
                Block::Body { text, .. } | Block::Heading { text, .. } => text.clone(),
            })
            .collect();
        assert!(
            text.contains(&"Character \u{b7} Book".to_owned()),
            "{text:?}"
        );
        assert!(text.contains(&"Emma \u{b7} Emma".to_owned()), "{text:?}");
        assert!(text.contains(&"fn main() {".to_owned()), "{text:?}");
        assert!(text.iter().any(|line| line.trim() == "run();"), "{text:?}");
    }

    #[test]
    fn headings_come_out_as_headings_at_their_own_level() {
        let found = blocks("# Plan\n\nFirst step.\n\n## Later\n\nSecond *step*.");
        assert_eq!(
            found,
            [
                Block::Heading {
                    level: 1,
                    text: "Plan".into()
                },
                Block::Body {
                    text: "First step.".into(),
                    links: Vec::new()
                },
                Block::Heading {
                    level: 2,
                    text: "Later".into()
                },
                Block::Body {
                    text: "Second step.".into(),
                    links: Vec::new()
                },
            ]
        );
    }

    #[test]
    fn a_wiki_link_keeps_its_words_and_says_which_note_they_open() {
        let found = blocks("See [[Projects/Alpha]] and [[Reading List|the list]].");
        let [Block::Body { text, links }] = found.as_slice() else {
            panic!("one paragraph: {found:?}");
        };
        assert_eq!(text, "See Projects/Alpha and the list.");
        assert_eq!(links.len(), 2);
        let (target, start, end) = &links[1];
        assert_eq!(target, "Reading List");
        assert_eq!(&text[*start..*end], "the list");
        assert_eq!(&text[links[0].1..links[0].2], "Projects/Alpha");
    }

    #[test]
    fn a_heading_that_holds_a_link_reads_as_its_words() {
        let found = blocks("## About [[Alpha]]");
        assert_eq!(
            found,
            [Block::Heading {
                level: 2,
                text: "About Alpha".into()
            }]
        );
    }

    #[test]
    fn a_reference_link_defined_after_a_heading_still_resolves_above_it() {
        let found = blocks("Read [the guide][g].\n\n# Notes\n\n[g]: https://example.com");
        assert!(matches!(&found[0], Block::Body { text, .. } if text == "Read the guide."));
    }

    #[test]
    fn wiki_links_read_as_their_visible_text() {
        let rendered = super::render("See [[Projects/Alpha]] and [[Reading List|the list]].");
        assert!(rendered.contains("See Projects/Alpha and the list."));
        assert!(!rendered.contains("[["));
    }
}

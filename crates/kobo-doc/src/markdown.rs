//! Reading Markdown as a document.
//!
//! # Why a library and not a scanner
//!
//! Markdown looks like it can be read with a handful of `starts_with` calls,
//! and every reader that tries discovers setext headings, lazy continuation
//! lines, fenced blocks containing hashes, list items containing blank lines,
//! and the several places where indentation changes meaning. `CommonMark` is a
//! specification with a test suite because none of that is guessable.
//!
//! So this is a translation layer over `pulldown-cmark` (MIT), not a parser.
//! What is written here is the part that is actually a judgement call: which
//! of Markdown's constructs mean something on a six-inch panel with one
//! column, one typeface and no colour, and what the rest should become.
//!
//! # What is deliberately dropped
//!
//! Links keep their words and lose their destination. A URL cannot be followed
//! from a page of a book, and printing it inline puts forty characters of
//! punctuation in the middle of a sentence. Emphasis is dropped for the moment
//! because a block carries one run of text and the renderer has no way to
//! carry a span; it is the obvious next thing to add and the shape here does
//! not stand in the way of it.

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::{Block, Builder, Document};

/// Reads a Markdown document.
#[must_use]
pub fn parse(source: &str) -> Document {
    let mut options = Options::empty();
    // Tables and strikethrough are not in the original specification but are
    // in essentially every Markdown file written since. Without them the
    // pipes and tildes are drawn literally.
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    // A note at the foot of the page has nowhere to go on a paginated panel,
    // so the definitions are collected where they were written, and the
    // reference keeps its number so the two can be matched up by eye.
    options.insert(Options::ENABLE_FOOTNOTES);
    // The front matter of a static-site page is metadata, not the first
    // paragraph of the article. Turning this on is what makes it a block
    // this parser can recognise and skip rather than a table it draws.
    options.insert(Options::ENABLE_YAML_STYLE_METADATA_BLOCKS);

    let mut state = State::new();
    for event in Parser::new_ext(source, options) {
        state.take(event);
    }
    state.finish()
}

/// What kind of block the events are currently inside.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Inside {
    Nothing,
    Paragraph,
    Heading(u8),
    Code,
    Item {
        ordered: bool,
    },
    /// A row of a table, which becomes one line of prose.
    Row,
    /// The front matter of a static-site page. Read and thrown away.
    Metadata,
}

struct State {
    builder: Builder,
    inside: Inside,
    text: String,
    /// How many block quotations deep the events are.
    ///
    /// A count rather than a flag because quotations nest, and a paragraph
    /// two deep is still somebody else's words — the panel has one inset, so
    /// the depth changes nothing about the drawing, but coming back out of the
    /// inner one must not end the outer.
    quoted: usize,
    /// Whether each open list is numbered, innermost last.
    lists: Vec<bool>,
    /// Cells of the table row being read.
    cells: Vec<String>,
    /// Words that belong at the front of the next block rather than in one of
    /// their own.
    ///
    /// A footnote's mark is written before the definition opens its paragraph.
    /// Pushed straight into `text` it would be flushed as a paragraph of its
    /// own the moment that paragraph started, leaving `[a]` sitting alone
    /// above the note it labels.
    prefix: String,
    /// Set when the metadata block at the top of the file has been seen, so a
    /// horizontal rule further down is not mistaken for more of it.
    seen_metadata: bool,
}

impl State {
    fn new() -> Self {
        Self {
            builder: Builder::new(),
            inside: Inside::Nothing,
            text: String::new(),
            quoted: 0,
            lists: Vec::new(),
            cells: Vec::new(),
            prefix: String::new(),
            seen_metadata: false,
        }
    }

    fn take(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) | Event::InlineMath(text) | Event::DisplayMath(text) => {
                self.text.push_str(&text);
            }
            // Inline code keeps its words. There is no second typeface to set
            // it in, and the alternative is losing the word entirely.
            Event::Code(code) => self.text.push_str(&code),
            // A break inside a paragraph is the author's line wrapping, not a
            // new paragraph; a hard break is meant, but a block holds one run
            // of text, so both become the space that would have been there.
            Event::SoftBreak | Event::HardBreak => self.text.push(' '),
            Event::Rule => {
                self.flush();
                self.builder.push(Block::Rule);
            }
            Event::FootnoteReference(name) => {
                self.text.push('[');
                self.text.push_str(&name);
                self.text.push(']');
            }
            Event::TaskListMarker(done) => {
                self.text.push_str(if done { "[x] " } else { "[ ] " });
            }
            // Raw HTML in a Markdown file is a `<br>`, a centred `<div>` or an
            // image. None of them survive as markup here, and printing the tag
            // itself would be worse than dropping it.
            Event::Html(_) | Event::InlineHtml(_) => {}
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => self.open(Inside::Paragraph),
            Tag::Heading { level, .. } => self.open(Inside::Heading(number(level))),
            Tag::BlockQuote(_) => {
                self.flush();
                self.quoted += 1;
            }
            Tag::CodeBlock(CodeBlockKind::Indented | CodeBlockKind::Fenced(_)) => {
                self.open(Inside::Code);
            }
            Tag::List(first) => {
                self.flush();
                self.lists.push(first.is_some());
            }
            Tag::Item => {
                let ordered = self.lists.last().copied().unwrap_or(false);
                self.open(Inside::Item { ordered });
            }
            Tag::TableHead | Tag::TableRow => {
                self.flush();
                self.cells.clear();
                self.inside = Inside::Row;
            }
            Tag::TableCell => self.text.clear(),
            Tag::MetadataBlock(_) => self.open(Inside::Metadata),
            Tag::FootnoteDefinition(name) => {
                self.flush();
                self.prefix = format!("[{name}] ");
            }
            // A destination that cannot be followed, or a picture that cannot
            // be fetched: in both cases the words inside the tag are the part
            // worth keeping, and they arrive as ordinary text events.
            Tag::Link { .. }
            | Tag::Image { .. }
            | Tag::Emphasis
            | Tag::Strong
            | Tag::Strikethrough
            | Tag::Superscript
            | Tag::Subscript
            | Tag::Table(_)
            | Tag::HtmlBlock
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::BlockQuote(_) => {
                self.flush();
                self.quoted = self.quoted.saturating_sub(1);
            }
            TagEnd::List(_) => {
                self.flush();
                self.lists.pop();
            }
            TagEnd::TableCell => {
                let cell = std::mem::take(&mut self.text);
                self.cells.push(crate::collapse(&cell));
            }
            TagEnd::TableHead | TagEnd::TableRow => self.flush_row(),
            TagEnd::FootnoteDefinition => {
                self.flush();
                // A definition with nothing in it must not leave its mark to
                // be picked up by whatever paragraph comes next.
                self.prefix.clear();
            }
            TagEnd::Paragraph
            | TagEnd::Heading(_)
            | TagEnd::CodeBlock
            | TagEnd::Item
            | TagEnd::MetadataBlock(_) => self.flush(),
            TagEnd::Table
            | TagEnd::Link
            | TagEnd::Image
            | TagEnd::Emphasis
            | TagEnd::Strong
            | TagEnd::Strikethrough
            | TagEnd::Superscript
            | TagEnd::Subscript
            | TagEnd::HtmlBlock
            | TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition => {}
        }
    }

    /// Starts a new block, closing whatever was open.
    ///
    /// Closing first matters because Markdown nests: a list item holds a
    /// paragraph, and without this the item's own words and the paragraph's
    /// would run together.
    fn open(&mut self, kind: Inside) {
        self.flush();
        self.inside = kind;
        if !self.prefix.is_empty() {
            let prefix = std::mem::take(&mut self.prefix);
            self.text.push_str(&prefix);
        }
    }

    /// Turns whatever has been collected into a block.
    fn flush(&mut self) {
        let text = std::mem::take(&mut self.text);
        let inside = std::mem::replace(&mut self.inside, Inside::Nothing);
        if text.trim().is_empty() && inside != Inside::Code {
            return;
        }
        match inside {
            Inside::Nothing => {}
            Inside::Metadata => {
                self.seen_metadata = true;
                // The title in the front matter is the page's title, and it is
                // more reliable than the first heading, which is often the
                // site's name rather than the article's.
                for line in text.lines() {
                    if let Some(title) = strip_field(line, "title") {
                        self.builder.set_title(title);
                    } else if let Some(author) = strip_field(line, "author") {
                        self.builder.set_author(author);
                    }
                }
            }
            Inside::Heading(level) => {
                // The one `#` at the top of a file is what the file is called
                // far more often than not, and a document with a title can say
                // so in its chrome. It stays a heading as well: it is still
                // where the first part begins.
                if level == 1 {
                    self.builder.set_title(&text);
                }
                self.builder.push(Block::Heading { level, text });
            }
            Inside::Code => self.builder.push(Block::Preformatted(text)),
            Inside::Item { ordered } => self.builder.push(Block::Item { ordered, text }),
            Inside::Paragraph | Inside::Row => {
                if self.quoted > 0 {
                    self.builder.push(Block::Quote(text));
                } else {
                    self.builder.push(Block::Paragraph(text));
                }
            }
        }
    }

    /// Turns a table row into one line.
    ///
    /// A table needs columns, and there is one column. Ruling it would give
    /// four characters of content between two rules on a panel this narrow, so
    /// the cells are run together with a separator that reads as a break
    /// rather than as punctuation inside a cell.
    fn flush_row(&mut self) {
        let cells: Vec<String> = self
            .cells
            .drain(..)
            .filter(|cell| !cell.is_empty())
            .collect();
        self.inside = Inside::Nothing;
        if cells.is_empty() {
            return;
        }
        self.builder
            .push(Block::Paragraph(cells.join(" \u{2014} ")));
    }

    fn finish(mut self) -> Document {
        self.flush();
        self.builder.finish()
    }
}

/// Pulls the value out of a `field: value` line of front matter.
fn strip_field<'a>(line: &'a str, field: &str) -> Option<&'a str> {
    let (name, value) = line.split_once(':')?;
    name.trim().eq_ignore_ascii_case(field).then(|| {
        value
            .trim()
            .trim_matches(|character| character == '"' || character == '\'')
    })
}

fn number(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_heading_is_a_heading_however_it_was_written() {
        // Setext headings are the reason this is not three `starts_with`
        // calls: the marker is on the line after the words.
        for source in ["# Title", "Title\n=====\n"] {
            let document = parse(source);
            assert_eq!(
                document.blocks[0],
                Block::Heading {
                    level: 1,
                    text: "Title".to_owned()
                },
                "{source:?}"
            );
        }
    }

    #[test]
    fn a_hash_inside_a_fence_is_not_a_heading() {
        let document = parse("```sh\n# not a heading\n```\n");
        assert_eq!(
            document.blocks,
            vec![Block::Preformatted("# not a heading".to_owned())]
        );
    }

    #[test]
    fn a_fenced_block_keeps_its_shape() {
        let document = parse("```rust\nfn main() {\n    ok();\n}\n```\n");
        let Block::Preformatted(text) = &document.blocks[0] else {
            panic!("code was not kept as code: {:?}", document.blocks[0]);
        };
        assert_eq!(text.lines().count(), 3);
        assert!(text.contains("    ok();"), "the indent was lost: {text:?}");
    }

    #[test]
    fn a_link_keeps_its_words_and_loses_its_destination() {
        // A URL cannot be followed from a page of a book, and printing it
        // puts forty characters of punctuation inside the sentence.
        let document = parse("See [the manual](https://example.invalid/a/very/long/path).");
        assert_eq!(
            document.blocks[0],
            Block::Paragraph("See the manual.".to_owned())
        );
    }

    #[test]
    fn a_paragraph_wrapped_by_an_editor_is_still_one_paragraph() {
        let document = parse("One line\nand its continuation.\n\nAnother.");
        assert_eq!(
            document.blocks[0],
            Block::Paragraph("One line and its continuation.".to_owned())
        );
    }

    #[test]
    fn a_quotation_is_marked_as_one_and_ends_where_it_ends() {
        let document = parse("> Quoted.\n\nNot quoted.");
        assert_eq!(
            document.blocks,
            vec![
                Block::Quote("Quoted.".to_owned()),
                Block::Paragraph("Not quoted.".to_owned()),
            ]
        );
    }

    #[test]
    fn coming_out_of_an_inner_quotation_does_not_end_the_outer_one() {
        let document = parse("> Outer.\n>\n> > Inner.\n>\n> Still outer.\n\nPlain.");
        let kinds: Vec<_> = document
            .blocks
            .iter()
            .map(|block| matches!(block, Block::Quote(_)))
            .collect();
        assert_eq!(kinds, vec![true, true, true, false]);
    }

    #[test]
    fn a_list_item_is_an_item_and_knows_whether_it_is_numbered() {
        let document = parse("- one\n- two\n\n1. first\n2. second\n");
        assert_eq!(
            document.blocks,
            vec![
                Block::Item {
                    ordered: false,
                    text: "one".to_owned()
                },
                Block::Item {
                    ordered: false,
                    text: "two".to_owned()
                },
                Block::Item {
                    ordered: true,
                    text: "first".to_owned()
                },
                Block::Item {
                    ordered: true,
                    text: "second".to_owned()
                },
            ]
        );
    }

    #[test]
    fn an_item_and_the_paragraph_inside_it_do_not_run_together() {
        // A loose list wraps each item's words in a paragraph. Without
        // closing the item first, the two runs of text are concatenated.
        let document = parse("- one\n\n- two\n");
        assert_eq!(document.blocks.len(), 2);
        assert_eq!(document.blocks[0].text(), Some("one"));
    }

    #[test]
    fn a_rule_survives_and_a_run_of_them_does_not_repeat() {
        let document = parse("One.\n\n---\n\nTwo.");
        assert_eq!(document.blocks[1], Block::Rule);
    }

    #[test]
    fn a_table_row_becomes_one_readable_line() {
        let document = parse("| a | b |\n|---|---|\n| 1 | 2 |\n");
        assert_eq!(
            document.blocks,
            vec![
                Block::Paragraph("a \u{2014} b".to_owned()),
                Block::Paragraph("1 \u{2014} 2".to_owned()),
            ]
        );
    }

    #[test]
    fn front_matter_says_what_the_page_is_rather_than_being_drawn() {
        let document = parse("---\ntitle: \"A Post\"\nauthor: A Person\n---\n\nWords.\n");
        assert_eq!(document.title.as_deref(), Some("A Post"));
        assert_eq!(document.author.as_deref(), Some("A Person"));
        assert_eq!(
            document.blocks,
            vec![Block::Paragraph("Words.".to_owned())],
            "the front matter was drawn as content"
        );
    }

    #[test]
    fn the_heading_at_the_top_names_the_document() {
        let document = parse("# The Manual\n\nWords.");
        assert_eq!(document.title.as_deref(), Some("The Manual"));
        assert!(
            matches!(document.blocks[0], Block::Heading { level: 1, .. }),
            "naming the document consumed the heading"
        );
    }

    #[test]
    fn front_matter_outranks_the_first_heading() {
        let document = parse("---\ntitle: The Real One\n---\n\n# The Site Name\n");
        assert_eq!(document.title.as_deref(), Some("The Real One"));
    }

    #[test]
    fn a_footnote_keeps_the_mark_that_leads_to_it() {
        let document = parse("Words.[^a]\n\n[^a]: The note.\n");
        assert_eq!(document.blocks[0], Block::Paragraph("Words.[a]".to_owned()));
        assert_eq!(
            document.blocks[1].text(),
            Some("[a] The note."),
            "the note lost the mark that leads back to it"
        );
    }

    #[test]
    fn raw_html_does_not_leak_its_tags_into_the_words() {
        let document = parse("A <b>bold</b> word.\n\n<div align=\"center\">Centred.</div>\n");
        for block in &document.blocks {
            let text = block.text().unwrap_or_default();
            assert!(!text.contains('<'), "a tag was drawn: {text:?}");
        }
        assert_eq!(
            document.blocks[0],
            Block::Paragraph("A bold word.".to_owned())
        );
    }

    #[test]
    fn a_task_list_says_which_things_are_done() {
        let document = parse("- [x] done\n- [ ] not\n");
        assert_eq!(document.blocks[0].text(), Some("[x] done"));
        assert_eq!(document.blocks[1].text(), Some("[ ] not"));
    }

    #[test]
    fn nothing_here_panics_on_anything() {
        for source in [
            "",
            "#",
            "```",
            "```\n",
            "> > > > > > > > > >",
            "| | | |",
            "---",
            "[^a]",
            &"- ".repeat(5_000),
            &"#".repeat(5_000),
            &"> ".repeat(5_000),
        ] {
            let _ = parse(source);
        }
    }
}

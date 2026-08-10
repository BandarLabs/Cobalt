//! Turning something somebody wants to read into something a panel can draw.
//!
//! # What this is for
//!
//! An application on this platform cannot open a file. Everything it reads
//! arrives as bytes (from a download, from the store, from a task) and
//! everything it draws is nodes. This crate is the middle: bytes of some
//! format in, a [`Document`] out, and the Reader turns a `Document` into pages
//! of nodes.
//!
//! # Why a document and not a string
//!
//! Plain text was enough while the only thing being read was Project
//! Gutenberg's plain text, and it is the reason the reader in gutenbird is a
//! wall of identical paragraphs. A heading is not a paragraph in bold; it is
//! where a chapter starts, which is what a table of contents is made of, what
//! "next chapter" moves between, and what a reader looks for when they come
//! back to a book after a week. Flattening it to a string throws that away at
//! the first step and no amount of care later gets it back.
//!
//! # The shape of the thing
//!
//! A document is a flat list of [`Block`]s. Not a tree: the panel draws one
//! column of blocks one after another, nothing here nests visually except a
//! quotation, and a tree would be a shape the renderer immediately flattens
//! again. Structure that matters (where a chapter begins) is a block, not a
//! level of nesting.
//!
//! # Limits
//!
//! Everything here is bounded. These parsers are pointed at bytes from the
//! open internet, on a device with 512 MB of memory shared with the stock
//! reader, and the failure the reader would see is the whole application being
//! killed rather than a message saying the book was odd. So a document has a
//! ceiling on blocks and on total text, and reaching it truncates rather than
//! fails: most of a book is worth more than an error.

#![forbid(unsafe_code)]

pub mod epub;
pub mod html;
pub mod markdown;
pub mod text;
pub mod zip;

use std::collections::BTreeMap;

/// What a file turned out to be.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Format {
    Text,
    Markdown,
    Html,
    Epub,
}

/// Works out what a file is, from its name and the bytes themselves.
///
/// The bytes are asked first where they can answer. A name is a hint somebody
/// typed: Gutenberg serves an EPUB from a URL ending in `.txt.utf-8`, and a
/// download saved as `book.epub` is often a plain text file the server had a
/// different opinion about. A zip's signature is not a hint.
#[must_use]
pub fn sniff(name: &str, bytes: &[u8]) -> Format {
    // "PK\x03\x04". Every EPUB is a zip, and nothing else here is.
    if bytes.starts_with(b"PK\x03\x04") {
        return Format::Epub;
    }
    let name = name.to_ascii_lowercase();
    let name = name.split(['?', '#']).next().unwrap_or(&name);
    for (suffix, format) in [
        (".epub", Format::Epub),
        (".md", Format::Markdown),
        (".markdown", Format::Markdown),
        (".htm", Format::Html),
        (".html", Format::Html),
        (".xhtml", Format::Html),
        (".txt", Format::Text),
    ] {
        if name.ends_with(suffix) {
            // An `.epub` that is not a zip is not an EPUB whatever it is
            // called, and trying to unpack it can only fail.
            return if format == Format::Epub {
                Format::Text
            } else {
                format
            };
        }
    }
    // No usable name. A file that opens with markup is markup; anything else
    // is prose, which is the reading that cannot mangle what it is given.
    let head = &bytes[..bytes.len().min(1024)];
    let head = String::from_utf8_lossy(head).to_ascii_lowercase();
    if head.contains("<html") || head.contains("<!doctype html") || head.contains("<body") {
        Format::Html
    } else {
        Format::Text
    }
}

/// Reads a file of any supported format.
///
/// # Errors
///
/// Only an EPUB can fail to be read at all: the other three formats have no
/// input they cannot interpret as *something*, which is deliberate, a book
/// that renders oddly can still be read, and one that refuses to open cannot.
pub fn read(name: &str, bytes: &[u8]) -> Result<Document, epub::Fault> {
    match sniff(name, bytes) {
        Format::Epub => epub::parse(bytes),
        Format::Markdown => Ok(markdown::parse(&String::from_utf8_lossy(bytes))),
        Format::Html => Ok(html::parse(&String::from_utf8_lossy(bytes))),
        Format::Text => Ok(text::parse(&String::from_utf8_lossy(bytes))),
    }
}

/// The most blocks one document may hold.
///
/// A long novel is a few thousand paragraphs. This is an order of magnitude
/// above that, and it is here so that a file which is one million empty list
/// items cannot become a million allocations.
pub const MAX_BLOCKS: usize = 60_000;

/// The most text one document may hold, in bytes.
///
/// Sixteen megabytes is far more than any book anybody reads on this device
/// (the largest thing in Project Gutenberg's top hundred is under three) and
/// far below the point at which holding it is a problem.
pub const MAX_TEXT: usize = 16 * 1024 * 1024;

/// The most named places one document may hold.
///
/// A heavily cross-referenced book names a few thousand: every footnote, every
/// verse, every entry in an index. This is above that and below the point
/// where a generated file whose every paragraph carries an identifier can make
/// the map cost more than the book.
pub const MAX_ANCHORS: usize = 20_000;

/// The most links one document may hold.
///
/// An annotated edition links every other sentence to a note. This is above
/// that and below the point where the list costs more than the book it is a
/// list of.
pub const MAX_LINKS: usize = 20_000;

/// The most text one block may hold.
///
/// A paragraph is a few hundred characters. A "paragraph" of a megabyte is a
/// file with no line breaks in it, and cutting it is better than handing the
/// layout engine a single block it will spend a second wrapping.
pub const MAX_BLOCK_TEXT: usize = 64 * 1024;

/// The deepest heading level that means anything.
///
/// HTML has six. Past three the distinction is invisible on a panel this size,
/// so deeper headings are clamped rather than dropped: an `<h5>` is still a
/// heading, it just does not get a fifth size nobody could tell from the
/// fourth.
pub const MAX_HEADING_LEVEL: u8 = 3;

/// One thing in a document, in the order it is read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Block {
    /// Where a chapter or a section starts. `level` is 1–[`MAX_HEADING_LEVEL`].
    Heading { level: u8, text: String },
    /// Ordinary prose.
    Paragraph(String),
    /// Somebody else's words, set in from the margin.
    Quote(String),
    /// Something whose line breaks and spaces are the point: a poem, a
    /// listing, an address. Never re-wrapped.
    Preformatted(String),
    /// One item of a list. `ordered` decides whether the marker is a number or
    /// a bullet; the number itself is the item's position, worked out when it
    /// is drawn, so that inserting an item cannot leave the list numbered
    /// wrongly.
    Item { ordered: bool, text: String },
    /// A picture the book set into its text.
    ///
    /// Carried as the name it was stored under rather than as pixels, because
    /// a block is copied and compared all over this crate and an illustrated
    /// book is twenty-five megabytes of them. The bytes live once, in
    /// [`Document::images`], and this says which.
    Picture {
        /// The key into [`Document::images`].
        name: String,
        /// What the book says the picture shows.
        ///
        /// Kept even when the picture is drawn: it is what a reader gets when
        /// the image will not decode, and on a panel that cannot show colour
        /// a caption is often the more useful of the two.
        alt: String,
    },
    /// A break between parts with no words on it.
    Rule,
    /// Where one file of a book ends and the next begins.
    ///
    /// Kept because an EPUB's chapters are separate files and a reader who
    /// asks for the next chapter means the next file, even when the author
    /// never wrote a heading at the top of it.
    Break,
}

impl Block {
    /// The words in this block, if it has any.
    #[must_use]
    pub fn text(&self) -> Option<&str> {
        match self {
            Self::Heading { text, .. }
            | Self::Paragraph(text)
            | Self::Quote(text)
            | Self::Preformatted(text)
            | Self::Item { text, .. } => Some(text),
            // A picture's description is not the words of the book: returning
            // it here would put a caption into a search, a highlight and the
            // count of what a page holds, all of which are about prose.
            Self::Picture { .. } | Self::Rule | Self::Break => None,
        }
    }

    /// Whether this block is where a reader would say a chapter starts.
    #[must_use]
    pub fn starts_a_part(&self) -> bool {
        matches!(self, Self::Heading { level: 1 | 2, .. } | Self::Break)
    }
}

/// Something to read, and what is known about it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Document {
    /// From the file itself, never from its name. A title guessed from a
    /// filename is wrong often enough to be worse than nothing.
    pub title: Option<String>,
    pub author: Option<String>,
    pub blocks: Vec<Block>,
    /// Whether something was left out because a limit was reached.
    ///
    /// Carried rather than swallowed so the Reader can say so. A book that
    /// silently stops two thirds of the way through looks like a book that
    /// ends abruptly, and the reader has no way to tell the difference.
    pub truncated: bool,
    /// The book's own table of contents, when it published one.
    ///
    /// Empty for a format that has no such thing, and for an EPUB whose
    /// contents could not be matched to anything in the spine. [`parts`] is
    /// the fallback and is always available, but it is a guess assembled from
    /// headings and file seams: it cannot tell a preface from chapter one, and
    /// it names a part after whatever heading happens to open it. A book that
    /// states its own contents is stating what its author called each part and
    /// in what order, which is not something to re-derive when it was given.
    ///
    /// [`parts`]: Document::parts
    pub contents: Vec<Contents>,
    /// Every named place in the book, and the block it names.
    ///
    /// A fragment is how one part of a book points at another: a table of
    /// contents entry, a footnote marker, a cross-reference. Gutenberg's
    /// EPUBs make this unavoidable rather than a nicety -- they put a whole
    /// novel's chapters in one file and tell them apart only by fragment, so
    /// a reader that resolves an href to a file lands every chapter of Pride
    /// and Prejudice on the same page.
    ///
    /// Keyed as the target is written after resolution, which for an EPUB is
    /// the archive name and the fragment together, because the same `id` is
    /// used in every file of most books.
    pub anchors: BTreeMap<String, usize>,
    /// The pictures the book set into its text, as they were stored.
    ///
    /// Undecoded on purpose. Turning bytes into pixels costs memory this
    /// device has to share with the reader it is pretending not to be, so the
    /// decision of how many to decode, and when, belongs to whatever is
    /// drawing them rather than to the parser. A book of four hundred
    /// engravings should not cost four hundred decodes to open.
    pub images: BTreeMap<String, Vec<u8>>,
    /// The links the book makes from one part of itself to another.
    ///
    /// A footnote marker, an endnote's way back, a cross-reference: all of
    /// them were flattened to plain text, so the words stayed and the going
    /// there did not. Kept in reading order.
    pub links: Vec<Link>,
}

/// Somewhere the book points, and the words it points from.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Link {
    /// The block the words sit in, as an index into [`Document::blocks`].
    pub block: usize,
    pub text: String,
    /// What it points at, in the same spelling [`Document::anchors`] uses.
    ///
    /// A target that is not in the anchors is one this book cannot reach: a
    /// link out to the web, or into a file the reading order left out. Kept
    /// rather than dropped, so a reader can be told the difference between a
    /// link that goes nowhere and a link that was never there.
    pub target: String,
}

impl Document {
    /// Where a link goes, when it goes somewhere in this book.
    #[must_use]
    pub fn destination(&self, link: &Link) -> Option<usize> {
        self.anchors.get(&link.target).copied()
    }
}

/// One line of a book's own table of contents.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Contents {
    pub title: String,
    /// Where it starts, as an index into [`Document::blocks`].
    ///
    /// Resolved when the book is read rather than followed later, because the
    /// href it came from is relative to the file it was written in and that
    /// file is inside an archive nobody keeps open. An entry whose target is
    /// not in the reading order is dropped rather than pointed somewhere else.
    pub block: usize,
    /// How deep this entry sits, zero for a top-level part.
    ///
    /// Kept so a contents screen can indent rather than present a flat list of
    /// two hundred sections, which is what a book with numbered subsections
    /// looks like when the nesting is thrown away.
    pub depth: u8,
}

impl Document {
    /// Where each part of the document starts, as indices into `blocks`.
    ///
    /// The first entry is always zero, even when the document opens with
    /// prose: everything belongs to some part, and a book whose first chapter
    /// has no heading would otherwise have a stretch at the front that "next
    /// chapter" could never reach.
    #[must_use]
    pub fn parts(&self) -> Vec<usize> {
        let mut parts = vec![0];
        for (index, block) in self.blocks.iter().enumerate().skip(1) {
            // A run of boundary blocks is one boundary. An EPUB chapter is a
            // file whose first element is its heading, so every seam in a book
            // is a `Break` immediately followed by a `Heading`; counting both
            // gives twice as many chapters as the book has, half of them one
            // block long.
            if block.starts_a_part() && !self.blocks[index - 1].starts_a_part() {
                parts.push(index);
            }
        }
        parts
    }

    /// The heading a part is known by, when it has one.
    #[must_use]
    pub fn part_title(&self, start: usize) -> Option<&str> {
        // A `Break` carries no words, so the name of that part is whatever
        // heading follows it, but only immediately, and only after a break.
        // Looking ahead unconditionally would give the unnamed stretch at the
        // front of a book the name of the chapter that follows it, which is
        // the one part that genuinely has no name.
        let at = match self.blocks.get(start)? {
            Block::Heading { text, .. } => return Some(text.as_str()),
            Block::Break => start + 1,
            _ => return None,
        };
        match self.blocks.get(at)? {
            Block::Heading { text, .. } => Some(text.as_str()),
            _ => None,
        }
    }
}

/// Collects blocks while keeping every limit in one place.
///
/// Each parser would otherwise have to remember to check three ceilings on
/// every push, and the one that forgot would be the one pointed at a hostile
/// file.
pub(crate) struct Builder {
    document: Document,
    text_used: usize,
}

impl Builder {
    pub(crate) fn new() -> Self {
        Self {
            document: Document::default(),
            text_used: 0,
        }
    }

    /// How many blocks have been kept so far.
    ///
    /// Asked while a book is being assembled, so that a format which knows
    /// where its own parts begin can record the position of each one as it
    /// goes. Counting afterwards would not do: blocks are dropped on the way
    /// in, so the number of blocks pushed and the number kept are different.
    pub(crate) fn len(&self) -> usize {
        self.document.blocks.len()
    }

    /// Records the book's own table of contents.
    pub(crate) fn set_contents(&mut self, contents: Vec<Contents>) {
        self.document.contents = contents;
    }

    /// Notes that a named place in the document starts here.
    ///
    /// Recorded against the block that is about to be pushed rather than the
    /// one just finished, because an `id` sits on the element whose words
    /// follow it. The first use of a name wins: a book that reuses one has
    /// already made it ambiguous, and picking the first keeps a link pointing
    /// backwards rather than to wherever the name was last repeated.
    pub(crate) fn mark_anchor(&mut self, id: &str) {
        if id.is_empty() || self.document.anchors.len() >= MAX_ANCHORS {
            return;
        }
        let at = self.document.blocks.len();
        self.document.anchors.entry(id.to_owned()).or_insert(at);
    }

    /// The blocks kept so far, for a format that needs to look at what it has
    /// assembled before it can say what else the book needs.
    pub(crate) fn blocks(&self) -> &[Block] {
        &self.document.blocks
    }

    /// Notes a link from the block being assembled.
    ///
    /// Recorded against the block it will become, the same way an anchor is,
    /// because the words of a link are part of a paragraph that has not been
    /// pushed yet.
    pub(crate) fn record_link(&mut self, text: &str, target: &str) {
        let text = collapse(text);
        if text.is_empty() || target.trim().is_empty() || self.document.links.len() >= MAX_LINKS {
            return;
        }
        self.document.links.push(Link {
            block: self.document.blocks.len(),
            text,
            target: target.to_owned(),
        });
    }

    /// Replaces the links, once a caller has resolved their targets.
    pub(crate) fn set_links(&mut self, links: Vec<Link>) {
        self.document.links = links;
    }

    /// Records the pictures the book refers to.
    pub(crate) fn set_images(&mut self, images: BTreeMap<String, Vec<u8>>) {
        self.document.images = images;
    }

    /// Replaces the anchors, once a caller has re-keyed them.
    pub(crate) fn set_anchors(&mut self, anchors: BTreeMap<String, usize>) {
        self.document.anchors = anchors;
    }

    /// Adds a block, trimming its text and dropping it if it says nothing.
    ///
    /// Blank blocks are dropped here rather than by each parser because every
    /// format produces them: a text file with three blank lines between
    /// paragraphs, a Markdown heading that is only hashes, an HTML `<p></p>`
    /// left behind by an editor.
    pub(crate) fn push(&mut self, block: Block) {
        let block = match block {
            Block::Preformatted(text) => {
                // Not trimmed on the inside, because the spaces are what it is
                // for. Only the blank lines around it go. Not trimmed on the
                // inside and not re-wrapped, but control characters still go:
                // a `\u{7}` has no drawing, and leaving one in makes every
                // renderer downstream decide what a bell looks like. Tabs and
                // newlines stay, they are the reason this block is
                // preformatted.
                let text: String = text
                    .trim_matches('\n')
                    .chars()
                    .filter(|c| *c == '\n' || *c == '\t' || !c.is_control())
                    .collect();
                if text.trim().is_empty() {
                    return;
                }
                Block::Preformatted(self.fit(text))
            }
            // A picture carries no words to trim and no ceiling to fit it to,
            // and an empty name is a picture nobody can find, so it is the one
            // block kept or dropped on its own terms.
            Block::Picture { ref name, .. } => {
                if name.trim().is_empty() {
                    return;
                }
                block
            }
            Block::Rule | Block::Break => {
                // Two rules in a row, or a break with nothing between it and
                // the last one, is a seam in the source rather than something
                // to draw twice.
                if matches!(
                    self.document.blocks.last(),
                    None | Some(Block::Rule | Block::Break)
                ) {
                    return;
                }
                block
            }
            other => {
                let Some(text) = other.text() else {
                    return;
                };
                let text = collapse(text);
                if text.is_empty() {
                    return;
                }
                let text = self.fit(text);
                match other {
                    Block::Heading { level, .. } => Block::Heading {
                        level: level.clamp(1, MAX_HEADING_LEVEL),
                        text,
                    },
                    Block::Paragraph(_) => Block::Paragraph(text),
                    Block::Quote(_) => Block::Quote(text),
                    Block::Item { ordered, .. } => Block::Item { ordered, text },
                    Block::Preformatted(_)
                    | Block::Picture { .. }
                    | Block::Rule
                    | Block::Break => return,
                }
            }
        };
        if self.document.blocks.len() >= MAX_BLOCKS {
            self.document.truncated = true;
            return;
        }
        self.document.blocks.push(block);
    }

    /// Cuts `text` to what is left of the ceilings.
    fn fit(&mut self, mut text: String) -> String {
        if text.len() > MAX_BLOCK_TEXT {
            truncate_to(&mut text, MAX_BLOCK_TEXT);
            self.document.truncated = true;
        }
        let room = MAX_TEXT.saturating_sub(self.text_used);
        if text.len() > room {
            truncate_to(&mut text, room);
            self.document.truncated = true;
        }
        self.text_used += text.len();
        text
    }

    pub(crate) fn set_title(&mut self, title: &str) {
        let title = collapse(title);
        if !title.is_empty() && self.document.title.is_none() {
            self.document.title = Some(title);
        }
    }

    pub(crate) fn set_author(&mut self, author: &str) {
        let author = collapse(author);
        if !author.is_empty() && self.document.author.is_none() {
            self.document.author = Some(author);
        }
    }

    pub(crate) fn finish(mut self) -> Document {
        // The contents were recorded against the blocks as they arrived, and
        // stripping the licence off the front of a Gutenberg book moves every
        // one of them. Shifting them here rather than resolving them later is
        // what keeps a chapter entry pointing at its own first words instead
        // of somewhere twenty paragraphs along.
        let before = self.document.blocks.len();
        strip_boilerplate(&mut self.document.blocks);
        let removed_from_front = before - self.document.blocks.len();
        // A document that ends on a rule or a break ends on a mark pointing at
        // nothing.
        while matches!(
            self.document.blocks.last(),
            Some(Block::Rule | Block::Break)
        ) {
            self.document.blocks.pop();
        }
        let kept = self.document.blocks.len();
        // An entry whose target was inside the licence, or past the end after
        // the trailing marks came off, no longer names anything in the book.
        // It is dropped rather than clamped to the nearest block, because a
        // contents line that silently goes to the wrong chapter is worse than
        // one that is not offered at all.
        self.document.contents.retain_mut(|entry| {
            let Some(block) = entry.block.checked_sub(removed_from_front) else {
                return false;
            };
            entry.block = block;
            block < kept
        });
        self.document.anchors = std::mem::take(&mut self.document.anchors)
            .into_iter()
            .filter_map(|(name, at)| {
                let at = at.checked_sub(removed_from_front)?;
                (at < kept).then_some((name, at))
            })
            .collect();
        self.document.links.retain_mut(|link| {
            let Some(block) = link.block.checked_sub(removed_from_front) else {
                return false;
            };
            link.block = block;
            block < kept
        });
        self.document
    }
}

/// Where Project Gutenberg's own text starts and stops.
///
/// The markers have been stable for twenty years. Matched on a prefix because
/// the line carries the book's title, which differs per book, and because some
/// editions write "THIS PROJECT GUTENBERG EBOOK" where others write "THE".
const GUTENBERG_START: &str = "*** START OF TH";
const GUTENBERG_END: &str = "*** END OF TH";

/// Drops the licence a Project Gutenberg book is wrapped in.
///
/// # Why this is here and not in the plain text parser
///
/// It was there, working on the raw file, and it only ever helped the one
/// format. Gutenberg serves the *same book* as EPUB and as HTML, wrapped in
/// the same thirty lines of header and five hundred of footer, and page one of
/// every one of those was a paragraph about redistribution in the United
/// States, identical for every book in the library. Doing it on blocks instead
/// means it works for every format there is and every format there will be,
/// because the markers survive into the blocks whatever parsed them.
///
/// A file with no markers is left exactly as it was. Guessing where somebody
/// else's front matter ends is not something this can do.
fn strip_boilerplate(blocks: &mut Vec<Block>) {
    let marked = |block: &Block, marker: &str| {
        block
            .text()
            .is_some_and(|text| text.trim_start().starts_with(marker))
    };
    if let Some(start) = blocks
        .iter()
        .position(|block| marked(block, GUTENBERG_START))
    {
        blocks.drain(..=start);
    }
    if let Some(end) = blocks.iter().position(|block| marked(block, GUTENBERG_END)) {
        blocks.truncate(end);
    }
}

/// Cuts a string to at most `limit` bytes without splitting a character.
pub(crate) fn truncate_to(text: &mut String, limit: usize) {
    if text.len() <= limit {
        return;
    }
    let mut at = limit;
    while at > 0 && !text.is_char_boundary(at) {
        at -= 1;
    }
    text.truncate(at);
}

/// Folds every run of whitespace into one space and trims the ends.
///
/// Every format arrives hard-wrapped by something: Gutenberg wraps at seventy
/// columns, HTML is indented by whoever wrote it, Markdown is wrapped by the
/// author's editor. Those line breaks belong to the file, not to the sentence,
/// and honouring them gives a column of ragged short lines on a panel that is
/// already narrow.
#[must_use]
pub fn collapse(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for character in text.chars() {
        if character.is_whitespace() {
            space = !out.is_empty();
            continue;
        }
        // Control characters are dropped rather than drawn. A renderer handed
        // a `\u{7}` has to decide what a bell looks like, and there is no
        // right answer.
        if character.is_control() {
            continue;
        }
        if space {
            out.push(' ');
            space = false;
        }
        out.push(character);
    }
    out
}

/// Folds every line-ending convention onto `\n`.
///
/// Gutenberg serves CRLF. Without this, a paragraph break that only matches
/// one convention is a paragraph break that usually does not match, and an
/// entire book parses as a single block.
#[must_use]
pub fn normalise_breaks(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bytes_outrank_the_name_they_arrived_under() {
        // Gutenberg serves an EPUB from a URL ending `.txt.utf-8`, and a
        // download saved as `book.epub` is often whatever the server felt like
        // sending.
        assert_eq!(sniff("book.txt", b"PK\x03\x04rest"), Format::Epub);
        assert_eq!(sniff("book.epub", b"Just some prose."), Format::Text);
    }

    #[test]
    fn a_name_decides_when_the_bytes_cannot() {
        assert_eq!(sniff("notes.md", b"# Notes"), Format::Markdown);
        assert_eq!(sniff("page.html", b"<p>hi"), Format::Html);
        assert_eq!(sniff("book.txt", b"words"), Format::Text);
        assert_eq!(sniff("page.html?v=2", b"<p>hi"), Format::Html);
    }

    #[test]
    fn markup_with_no_name_is_still_markup() {
        assert_eq!(sniff("", b"<!DOCTYPE html><html><body>hi"), Format::Html);
        assert_eq!(sniff("", b"Just some prose."), Format::Text);
    }

    #[test]
    fn reading_a_file_of_any_format_yields_the_same_kind_of_thing() {
        assert_eq!(
            read("a.md", b"# Title\n\nWords.")
                .expect("markdown always reads")
                .blocks[1],
            Block::Paragraph("Words.".to_owned())
        );
        assert_eq!(
            read("a.html", b"<p>Words.</p>")
                .expect("html always reads")
                .blocks[0],
            Block::Paragraph("Words.".to_owned())
        );
        assert_eq!(
            read("a.txt", b"Words.").expect("text always reads").blocks[0],
            Block::Paragraph("Words.".to_owned())
        );
    }

    #[test]
    fn gutenbergs_licence_is_stripped_whatever_format_it_arrived_in() {
        // Gutenberg serves the same book as text, as HTML and as EPUB, wrapped
        // in the same licence. Doing this on the raw text of one format left
        // page one of the other two as a paragraph about redistribution in the
        // United States, identical for every book in the library.
        let page = "<p>This eBook is for the use of anyone anywhere.</p>\
                    <p>*** START OF THE PROJECT GUTENBERG EBOOK SOMETHING ***</p>\
                    <p>It began badly.</p>\
                    <p>*** END OF THE PROJECT GUTENBERG EBOOK SOMETHING ***</p>\
                    <p>Redistribution is subject to the terms of the licence.</p>";
        assert_eq!(
            html::parse(page).blocks,
            vec![Block::Paragraph("It began badly.".to_owned())]
        );
    }

    #[test]
    fn a_document_with_no_markers_is_not_cut_about() {
        let page = "<p>One.</p><p>Two.</p>";
        assert_eq!(html::parse(page).blocks.len(), 2);
    }

    #[test]
    fn a_run_of_whitespace_is_one_space() {
        assert_eq!(collapse("  a\n\tb   c  "), "a b c");
        assert_eq!(collapse("\n\n\n"), "");
    }

    #[test]
    fn a_control_character_is_dropped_rather_than_drawn() {
        assert_eq!(collapse("a\u{7}b"), "ab");
    }

    #[test]
    fn every_line_ending_convention_folds_onto_one() {
        assert_eq!(normalise_breaks("a\r\nb\rc\nd"), "a\nb\nc\nd");
    }

    #[test]
    fn a_block_that_says_nothing_is_not_kept() {
        let mut builder = Builder::new();
        builder.push(Block::Paragraph("   \n  ".to_owned()));
        builder.push(Block::Heading {
            level: 1,
            text: String::new(),
        });
        assert!(builder.finish().blocks.is_empty());
    }

    #[test]
    fn a_heading_deeper_than_the_panel_can_show_is_clamped_not_dropped() {
        // An `<h5>` is still where a section starts. Dropping it would lose
        // the boundary; giving it a fifth size nobody can tell from the fourth
        // would be a lie about the hierarchy.
        let mut builder = Builder::new();
        builder.push(Block::Heading {
            level: 9,
            text: "Deep".to_owned(),
        });
        assert_eq!(
            builder.finish().blocks,
            vec![Block::Heading {
                level: MAX_HEADING_LEVEL,
                text: "Deep".to_owned()
            }]
        );
    }

    #[test]
    fn preformatted_text_keeps_the_spaces_that_are_the_point_of_it() {
        let mut builder = Builder::new();
        builder.push(Block::Preformatted(
            "\n    fn main() {\n        ok();\n    }\n".to_owned(),
        ));
        let blocks = builder.finish().blocks;
        let Some(Block::Preformatted(text)) = blocks.first() else {
            panic!("expected preformatted text, got {blocks:?}");
        };
        assert!(
            text.starts_with("    fn main"),
            "the indent was collapsed away: {text:?}"
        );
        assert!(text.contains('\n'), "the line breaks were collapsed away");
    }

    #[test]
    fn a_document_never_ends_on_a_mark_pointing_at_nothing() {
        let mut builder = Builder::new();
        builder.push(Block::Paragraph("Words.".to_owned()));
        builder.push(Block::Rule);
        builder.push(Block::Break);
        assert_eq!(
            builder.finish().blocks,
            vec![Block::Paragraph("Words.".to_owned())]
        );
    }

    #[test]
    fn a_run_of_marks_is_drawn_once() {
        let mut builder = Builder::new();
        builder.push(Block::Rule);
        builder.push(Block::Paragraph("One.".to_owned()));
        builder.push(Block::Rule);
        builder.push(Block::Rule);
        builder.push(Block::Paragraph("Two.".to_owned()));
        assert_eq!(
            builder.finish().blocks,
            vec![
                Block::Paragraph("One.".to_owned()),
                Block::Rule,
                Block::Paragraph("Two.".to_owned()),
            ]
        );
    }

    #[test]
    fn a_paragraph_longer_than_a_block_is_cut_and_says_so() {
        let mut builder = Builder::new();
        builder.push(Block::Paragraph("x".repeat(MAX_BLOCK_TEXT * 2)));
        let document = builder.finish();
        assert!(document.truncated, "the cut was made silently");
        assert_eq!(
            document.blocks[0].text().map(str::len),
            Some(MAX_BLOCK_TEXT)
        );
    }

    #[test]
    fn cutting_never_splits_a_character() {
        let mut text = "é".repeat(100);
        truncate_to(&mut text, 51);
        assert_eq!(text.len(), 50, "a two-byte character was cut in half");
    }

    #[test]
    fn everything_belongs_to_some_part_even_before_the_first_heading() {
        // A book that opens with a preface and only then reaches "Chapter One"
        // would otherwise have a stretch at the front that nothing could
        // navigate to.
        let document = Document {
            blocks: vec![
                Block::Paragraph("A preface.".to_owned()),
                Block::Heading {
                    level: 1,
                    text: "Chapter One".to_owned(),
                },
                Block::Paragraph("It began.".to_owned()),
            ],
            ..Document::default()
        };
        assert_eq!(document.parts(), vec![0, 1]);
        assert_eq!(document.part_title(1), Some("Chapter One"));
        assert_eq!(document.part_title(0), None, "the preface has no name");
    }

    #[test]
    fn a_part_that_begins_with_a_file_break_is_named_by_what_follows_it() {
        // An EPUB chapter is a file, and the heading is the first thing in it.
        let document = Document {
            blocks: vec![
                Block::Paragraph("The end of the last one.".to_owned()),
                Block::Break,
                Block::Heading {
                    level: 1,
                    text: "Chapter Two".to_owned(),
                },
            ],
            ..Document::default()
        };
        // Two boundary blocks in a row, one seam.
        assert_eq!(document.parts(), vec![0, 1]);
        assert_eq!(document.part_title(1), Some("Chapter Two"));
    }

    #[test]
    fn a_heading_well_inside_a_part_does_not_rename_it() {
        let document = Document {
            blocks: vec![
                Block::Break,
                Block::Paragraph("One.".to_owned()),
                Block::Paragraph("Two.".to_owned()),
                Block::Heading {
                    level: 3,
                    text: "A section".to_owned(),
                },
            ],
            ..Document::default()
        };
        assert_eq!(document.part_title(0), None);
    }
}

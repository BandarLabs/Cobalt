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

/// The most columns one table row is kept at.
///
/// A panel seven centimetres across cannot show a dozen columns of prose and
/// nothing is gained by carrying them: past this the row is not a table any
/// more, it is a spreadsheet somebody published as a web page. Cells past the
/// limit are dropped rather than merged, because merging them silently
/// changes which value sits under which heading.
pub const MAX_ROW_CELLS: usize = 12;
/// Styled runs retained for one block. Pathological tag-per-character XHTML
/// degrades into one final run rather than growing the render protocol.
const MAX_RICH_SPANS: usize = 256;

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
    /// Text belonging to the illustration immediately before it.
    Caption(String),
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
        /// Whether this is an illustration rather than a piece of the text.
        ///
        /// An illustration is a thing set into the page, and a reader gets a
        /// rule around it so that a pale sky is not mistaken for the margin.
        /// A formula set on its own line is not an illustration of the text,
        /// it *is* the text, drawn rather than written, and a box around it
        /// would read as oddly as a box around a sentence.
        illustration: bool,
    },
    /// One row of a table.
    ///
    /// A table is a run of these, and the run is the table: there is no
    /// enclosing block, because a document is a flat list and a nested one
    /// would have to be paginated, searched and highlighted through a second
    /// shape. Consecutive rows are laid out together, so the columns of a
    /// table line up without any block having to own the whole of it.
    ///
    /// Cells were previously joined into one paragraph with an em dash
    /// between them, which reads as a sentence made of fragments and loses
    /// the one thing a table is for: knowing which value belongs to which
    /// column.
    Row {
        /// Whether the row was written with `th` rather than `td`, and so
        /// names the columns rather than filling them.
        header: bool,
        /// Left to right, as written. Empty cells are kept: a gap in a table
        /// is where the alignment of everything after it comes from.
        cells: Vec<String>,
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

/// Inline emphasis retained from HTML/XHTML instead of flattened away.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct InlineStyle {
    pub strong: bool,
    pub emphasis: bool,
    pub underline: bool,
    pub superscript: bool,
    pub subscript: bool,
}

/// What a typeset formula's picture is named in [`Document::images`].
///
/// A formula is drawn at a fixed [`FORMULA_PICTURE_EM`] pixels to the em so
/// that it has detail to spare, and set on the page at the size of the text
/// it belongs to. An application has to be able to tell one apart from an
/// illustration to size it that way, and the name is how.
pub const FORMULA_PICTURE_PREFIX: &str = "formula:";

/// The pixels to the em a formula's picture was drawn at.
pub const FORMULA_PICTURE_EM: u32 = 48;

/// The most formulae one document will be typeset pictures for.
///
/// A survey paper can carry a thousand pieces of mathematics, and a reader
/// can only ever reserve room for a few dozen pictures at a time, so the
/// nine hundred and fiftieth of them was never going to be shown to anybody.
/// Drawing all one thousand and sixteen in one such paper took fifteen and a
/// half seconds on a Clara BW — five to typeset them and nine more to turn
/// them into something the panel could draw — and almost every picture that
/// bought was thrown away unlooked at.
///
/// Past this many, a formula is read as the line of text it has always fallen
/// back to. That is a worse-looking page than a drawn one, at the far end of
/// a paper nobody has scrolled to, and it is the difference between a paper
/// that opens and a paper that does not.
pub const MAX_FORMULA_PICTURES: usize = 64;

/// How long one document may spend drawing formulae.
///
/// The count above bounds how many pictures are worth keeping; it says
/// nothing about how long making them takes, and a reader with a slower
/// machine than the one this was written on should not be made to wait for a
/// count that was chosen elsewhere.
///
/// Measured on a Clara BW, reading a 1.4 MB paper carrying 1016 formulae:
/// the markup alone parses in about 290 ms, typesetting a formula costs
/// 4.7 ms, and turning its picture into something the panel can draw costs
/// 9.4 ms more. Opening that paper with every formula drawn takes fifteen
/// and a half seconds. With sixty-four it takes two and a half, against nine
/// and a half before any of this work — and sixty-four is already more
/// mathematics than a page of it can show.
///
/// So the budget is set above what the count costs on this device — it is
/// the count that should decide on hardware this work was measured on, and
/// the clock that should decide on anything slower. A machine that cannot
/// afford sixty-four draws what it can and reads the rest as the text they
/// have always fallen back to, without having to be told which machine it is.
pub const FORMULA_DRAWING_BUDGET: core::time::Duration = core::time::Duration::from_millis(500);

/// The same, for the renderer, which measures type in fractions of a pixel.
#[cfg(feature = "raster")]
pub(crate) const FORMULA_PICTURE_EM_F32: f32 = 48.0;

/// How big a formula's picture should be drawn, for a given reading em.
///
/// The picture was typeset at [`FORMULA_PICTURE_EM`] pixels to the em, so it
/// is already in the units type is measured in: setting it beside text of a
/// given em is a matter of scaling by the ratio of the two. The em rather
/// than the line height, because the line height is the em plus the space a
/// reader wants between lines, and a formula scaled to that comes out a fifth
/// larger than the letters either side of it.
#[must_use]
pub fn formula_size(source: (u32, u32), em: u32) -> (u32, u32) {
    let scaled = |side: u32| {
        (side.saturating_mul(em.max(1)))
            .div_ceil(FORMULA_PICTURE_EM)
            .max(1)
    };
    (scaled(source.0), scaled(source.1))
}

/// One styled run inside a block of prose.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InlineSpan {
    pub text: String,
    pub style: InlineStyle,
    /// The typeset picture to draw over this run, keyed into
    /// [`Document::images`], when the run is a formula.
    ///
    /// Mathematics does not survive being written out in a line: a fraction
    /// stacks and an index sits above its letter, and neither is a sequence
    /// of characters. So `text` holds the best linear reading of the formula
    /// -- which is what a search matches and what a reader without the
    /// picture gets -- and this says what to draw over it instead.
    pub formula: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TextAlignment {
    #[default]
    Start,
    Center,
    End,
    Justify,
}

/// Safe publisher styling that affects one reflowable block.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BlockStyle {
    pub alignment: TextAlignment,
    /// Percent of the selected face's natural line height.
    pub line_height_percent: u16,
    /// Before/after spacing in hundredths of an em.
    pub margin_before_em: u16,
    pub margin_after_em: u16,
    pub first_line_indent_em: i16,
    pub font_family: Option<String>,
    pub page_break_before: bool,
    pub page_break_after: bool,
}

impl Default for BlockStyle {
    fn default() -> Self {
        Self {
            alignment: TextAlignment::Start,
            line_height_percent: 100,
            margin_before_em: 0,
            margin_after_em: 0,
            first_line_indent_em: 0,
            font_family: None,
            page_break_before: false,
            page_break_after: false,
        }
    }
}

/// Structure and publisher styling retained for a text block.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RichBlock {
    pub spans: Vec<InlineSpan>,
    pub style: BlockStyle,
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
            | Self::Item { text, .. }
            | Self::Caption(text) => Some(text),
            // A picture's description is not the words of the book: returning
            // it here would put a caption into a search, a highlight and the
            // count of what a page holds, all of which are about prose.
            //
            // A row has words but no single string of them: joining the cells
            // to answer this is exactly the flattening this block exists to
            // stop. What a row reads as is [`Block::row_text`], which the
            // things that genuinely want prose ask for by name.
            Self::Picture { .. } | Self::Rule | Self::Break | Self::Row { .. } => None,
        }
    }

    /// A row read out as one line, for the things that can only take one.
    ///
    /// Search and the word count want the words; they do not want the
    /// columns. The cells are joined with a space rather than with a mark, so
    /// that a phrase which happens to straddle two cells is still found.
    #[must_use]
    pub fn row_text(&self) -> Option<String> {
        let Self::Row { cells, .. } = self else {
            return None;
        };
        Some(
            cells
                .iter()
                .filter(|cell| !cell.trim().is_empty())
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(" "),
        )
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
    /// Outline fonts embedded by the publisher, keyed by their resolved
    /// archive name.
    ///
    /// Kept as bounded, undecoded bytes for the same reason pictures are: the
    /// document parser has no panel and no glyph cache, while the reader can
    /// choose one face and hand it to the runtime only when publisher styling
    /// is enabled. WOFF/WOFF2 assets are retained for diagnostics but only
    /// OpenType/TrueType faces are currently usable by the rasterizer.
    pub fonts: BTreeMap<String, EmbeddedFont>,
    /// Rich XHTML representation for blocks that carried inline/CSS styling.
    /// Blocks absent from the map use their ordinary semantic presentation.
    pub rich: BTreeMap<usize, RichBlock>,
    /// The links the book makes from one part of itself to another.
    ///
    /// A footnote marker, an endnote's way back, a cross-reference: all of
    /// them were flattened to plain text, so the words stayed and the going
    /// there did not. Kept in reading order.
    pub links: Vec<Link>,
}

/// One font declared by an EPUB package.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EmbeddedFont {
    /// MIME type from the package manifest, when it supplied one.
    pub media_type: String,
    /// Family named by the publisher's `@font-face`, when one refers to this
    /// asset. This lets the reader choose the face the book actually asks for
    /// instead of whichever font filename sorts first.
    pub family: Option<String>,
    /// The bounded font bytes exactly as stored in the EPUB.
    pub bytes: Vec<u8>,
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

    /// Records publisher fonts that passed the EPUB resource bounds.
    pub(crate) fn set_fonts(&mut self, fonts: BTreeMap<String, EmbeddedFont>) {
        self.document.fonts = fonts;
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
            // A row is normalised cell by cell, because the thing that has
            // to be bounded is the row's width in columns as well as its
            // length in characters. A row of nothing but blanks is a spacer
            // somebody drew with a table and is not kept.
            Block::Row { header, cells } => {
                let mut kept: Vec<String> = cells
                    .into_iter()
                    .take(MAX_ROW_CELLS)
                    .map(|cell| self.fit(collapse(&cell)))
                    .collect();
                while matches!(kept.last(), Some(cell) if cell.is_empty()) {
                    kept.pop();
                }
                if kept.iter().all(String::is_empty) {
                    return;
                }
                Block::Row {
                    header,
                    cells: kept,
                }
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
                    Block::Caption(_) => Block::Caption(text),
                    Block::Preformatted(_)
                    | Block::Picture { .. }
                    | Block::Rule
                    | Block::Break
                    | Block::Row { .. } => return,
                }
            }
        };
        if self.document.blocks.len() >= MAX_BLOCKS {
            self.document.truncated = true;
            return;
        }
        self.document.blocks.push(block);
    }

    /// Adds a block while retaining its independently bounded XHTML styling.
    pub(crate) fn push_rich(&mut self, block: Block, mut rich: RichBlock) {
        let at = self.document.blocks.len();
        self.push(block);
        if self.document.blocks.len() == at + 1 && !rich.spans.is_empty() {
            let canonical = self.document.blocks[at].text().unwrap_or_default();
            bound_rich_spans(&mut rich.spans, canonical);
            self.document.rich.insert(at, rich);
        }
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
        let removed_from_front = strip_boilerplate(&mut self.document.blocks);
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
        self.document.rich = std::mem::take(&mut self.document.rich)
            .into_iter()
            .filter_map(|(at, rich)| {
                let at = at.checked_sub(removed_from_front)?;
                (at < kept).then_some((at, rich))
            })
            .collect();
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
/// Removes Project Gutenberg's licence from around the book.
///
/// Returns how many blocks came off the *front*, which is the only part that
/// moves everything after it. Truncating the end removes blocks too, so a
/// caller measuring the length before and against after would count both and
/// shift every recorded position by the size of the licence at the end as
/// well -- which is how a table of contents ends up pointing a chapter or two
/// early in exactly the books that have both markers.
fn strip_boilerplate(blocks: &mut Vec<Block>) -> usize {
    let marked = |block: &Block, marker: &str| {
        block
            .text()
            .is_some_and(|text| text.trim_start().starts_with(marker))
    };
    let mut removed_from_front = 0;
    if let Some(start) = blocks
        .iter()
        .position(|block| marked(block, GUTENBERG_START))
    {
        blocks.drain(..=start);
        removed_from_front = start + 1;
    }
    if let Some(end) = blocks.iter().position(|block| marked(block, GUTENBERG_END)) {
        blocks.truncate(end);
    }
    removed_from_front
}

/// Cuts a string to at most `limit` bytes without splitting a character.
fn bound_rich_spans(spans: &mut Vec<InlineSpan>, canonical: &str) {
    let joined = spans
        .iter()
        .map(|span| span.text.as_str())
        .collect::<String>();
    if !joined.starts_with(canonical) {
        *spans = vec![InlineSpan {
            text: canonical.to_owned(),
            style: InlineStyle::default(),
            formula: None,
        }];
        return;
    }
    let mut left = canonical.len();
    for span in spans.iter_mut() {
        if span.text.len() > left {
            truncate_to(&mut span.text, left);
        }
        left = left.saturating_sub(span.text.len());
    }
    spans.retain(|span| !span.text.is_empty());
    if spans.len() > MAX_RICH_SPANS {
        let tail = spans
            .drain(MAX_RICH_SPANS - 1..)
            .map(|span| span.text)
            .collect::<String>();
        spans.push(InlineSpan {
            text: tail,
            style: InlineStyle::default(),
            formula: None,
        });
    }
}

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

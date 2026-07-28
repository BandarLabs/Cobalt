//! A book on the panel, and everything a reader does to one.
//!
//! # Why a position is a place in the book and never a page number
//!
//! Page 47 is not a location. Make the type larger and page 47 is somewhere
//! else; the same book on the same device with the same reader is suddenly
//! forty pages further on than it was a moment ago. Every stock reader ever
//! made has had to learn this, and the ones that got it wrong are the ones
//! that lose your place when you change the font.
//!
//! So everything remembered here — where you are, what you marked, what you
//! bookmarked — is a [`Locator`], which is an index into the parsed document.
//! Pages are derived from the document and the current type size, and are
//! thrown away and rebuilt whenever either changes. Changing the type size
//! repaginates and then goes back to the block that was at the top of the
//! page, so the words under your thumb are still there afterwards.
//!
//! # Why this paginates rather than calling `paginate`
//!
//! [`kobo_ui::paginate`] sets everything at body size, because it takes a
//! string and a string has no structure. A book does: a chapter heading is
//! larger, a list item is indented, and each of those is a different height.
//! Measuring them all as body text puts the last lines of a page below the
//! bottom of the panel — the layout engine drops whatever does not fit,
//! silently, and the reader sees a page that stops mid-sentence.
//!
//! # Why a highlight is drawn as a quote
//!
//! There is no text selection on this panel and there should not be: selecting
//! a phrase needs a cursor a finger cannot place on a display that takes most
//! of a second to repaint. So the unit of highlighting is the paragraph, which
//! is what a tap can address, and a highlighted paragraph is set as a
//! depth-one quote — indented, with a rule down the left. That is what a
//! marked passage looks like in a printed book, it needs nothing new from the
//! renderer, and because the paragraph is *paginated* at that depth as well,
//! marking one never pushes the foot of the page off the bottom.

use std::collections::BTreeSet;

use kobo_doc::{Block, Document};
use kobo_sdk::{BannerLevel, Screen, ScreenBuilder};
use kobo_ui::TextScale;
use kobo_ui::{quote_offsets, wrap_text_in, DisplayMetrics, Face, FontSize, ProseArea};

/// Where something is in a book, independent of how the book is set.
///
/// An index into [`Document::blocks`]. Deliberately not a page, not a
/// character offset into a rendering, and not a percentage: those all move
/// when the type size does, and a bookmark that moves is not a bookmark.
pub type Locator = u32;

/// The depth a highlight's rule is set in. One, because a highlight is not a
/// reply to anything — it just needs a margin to put the mark in.
const HIGHLIGHT_DEPTH: u8 = 1;

/// The fewest lines of a paragraph worth leaving by itself at a page edge.
///
/// Two: the ordinary widow-and-orphan rule. A single line of a paragraph
/// stranded at the foot or the head of a page reads as something having gone
/// wrong rather than as prose continuing.
const MIN_KEEP_LINES: usize = 2;

/// The most pages a book is broken into.
///
/// A ceiling rather than a guess. Pagination allocates per page, and a
/// document that somehow produced millions of them would take the memory of a
/// device with 256 MB for everything.
const MAX_PAGES: usize = 16_384;

/// How much the front light moves per tap, out of a hundred.
///
/// Fine enough to find a comfortable level in a few taps, coarse enough that
/// finding it does not take twenty on a panel that flashes at every one.
const LIGHT_STEP: u8 = 10;

/// What the reader is looking at besides the book.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Chrome {
    /// The book and nothing else. The default, and where a reader spends
    /// essentially all of their time.
    #[default]
    Hidden,
    /// Type size, front light, and the mark controls.
    Controls,
    /// Everything marked in this book, in order, each one a way back to it.
    Highlights,
    /// The paragraphs on this page, to choose one to mark.
    ///
    /// A separate screen because there is no text selection on this panel: a
    /// finger cannot place a cursor on a display that takes most of a second
    /// to repaint, so the paragraph is picked from a list instead.
    Marking,
}

/// One piece of one block, as it lands on a page.
///
/// A long paragraph is cut across a page break, so a block can produce several
/// of these. `block` is the same for all of them, which is what lets a mark on
/// a paragraph survive being split.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Piece {
    pub block: Locator,
    pub text: String,
    kind: Kind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    Heading(u8),
    Body,
    Marked,
    Quote,
    Preformatted,
    Item,
    Rule,
    Break,
}

impl Kind {
    const fn size(self) -> FontSize {
        match self {
            Kind::Heading(1) => FontSize::Heading,
            // Level three is already small enough that the title size would
            // barely distinguish it from the text under it.
            Kind::Heading(_) => FontSize::Title,
            _ => FontSize::Body,
        }
    }

    const fn depth(self) -> u8 {
        match self {
            Kind::Marked => HIGHLIGHT_DEPTH,
            Kind::Quote | Kind::Item => 1,
            _ => 0,
        }
    }

    /// Whether this is something to look at rather than something to read.
    const fn is_furniture(self) -> bool {
        matches!(self, Kind::Rule | Kind::Break)
    }
}

/// Everything about one book that has to survive the application closing.
///
/// Small enough for the ordinary key-value store: a position, a type size, a
/// light level, and two sorted sets of block indices. A book with a thousand
/// marks in it is still only a few kilobytes.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Memory {
    /// The block that was at the top of the page.
    pub at: Locator,
    pub bookmarks: BTreeSet<Locator>,
    pub highlights: BTreeSet<Locator>,
    pub scale: TextScale,
    /// `None` means the reader has never set one here, so the device's own
    /// level is left alone. Zero is a real setting and is not the same thing.
    pub light: Option<u8>,
}

impl Memory {
    /// Writes this out for [`kobo_sdk::AppStore`].
    ///
    /// A line per field, not a struct dump. It is readable over the shell when
    /// somebody reports having lost their place, it survives a field being
    /// added, and a line that cannot be understood costs one field rather than
    /// the whole record — which matters, because the alternative to a partly
    /// understood record is a book that reopens at page one.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        use std::fmt::Write;
        let mut text = String::new();
        let _ = writeln!(text, "at {}", self.at);
        let _ = writeln!(text, "scale {}", self.scale.wire_value());
        if let Some(light) = self.light {
            let _ = writeln!(text, "light {light}");
        }
        for bookmark in &self.bookmarks {
            let _ = writeln!(text, "mark {bookmark}");
        }
        for highlight in &self.highlights {
            let _ = writeln!(text, "high {highlight}");
        }
        text.into_bytes()
    }

    /// Reads one back. Anything unrecognised is skipped rather than fatal.
    #[must_use]
    pub fn decode(bytes: &[u8]) -> Self {
        let mut memory = Self::default();
        let Ok(text) = std::str::from_utf8(bytes) else {
            return memory;
        };
        for line in text.lines() {
            let Some((field, value)) = line.split_once(' ') else {
                continue;
            };
            match field {
                "at" => memory.at = value.parse().unwrap_or(0),
                "scale" => {
                    if let Ok(wire) = value.parse::<u8>() {
                        memory.scale = TextScale::from_wire(wire).unwrap_or_default();
                    }
                }
                "light" => memory.light = value.parse().ok(),
                "mark" => {
                    if let Ok(at) = value.parse() {
                        memory.bookmarks.insert(at);
                    }
                }
                "high" => {
                    if let Ok(at) = value.parse() {
                        memory.highlights.insert(at);
                    }
                }
                _ => {}
            }
        }
        memory
    }
}

/// The names a reader answers to.
///
/// Names rather than raw identifiers, because [`kobo_sdk::ActionId`] is a hash
/// and an application that compared the wrong two would find out at a reader's
/// expense rather than at a compiler's.
pub mod action {
    pub const FORWARD: &str = "reader-forward";
    pub const BACK: &str = "reader-back";
    /// Shows the controls, or puts them away again.
    pub const CONTROLS: &str = "reader-controls";
    pub const CLOSE: &str = "reader-close";
    pub const LARGER: &str = "reader-larger";
    pub const SMALLER: &str = "reader-smaller";
    pub const BRIGHTER: &str = "reader-brighter";
    pub const DIMMER: &str = "reader-dimmer";
    pub const BOOKMARK: &str = "reader-bookmark";
    pub const HIGHLIGHTS: &str = "reader-highlights";
    /// Opens the list of paragraphs on this page, to mark one.
    pub const MARKING: &str = "reader-marking";
    /// One per markable paragraph on the page, suffixed with its block index.
    pub const MARK: &str = "reader-mark-";
    /// One per stored mark, suffixed with its block index.
    pub const GO: &str = "reader-go-";
}

/// What an application still has to do about an action the reader handled.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    /// Nothing here answers that action.
    Elsewhere,
    /// Handled; repaint.
    Repaint,
    /// Handled, and something worth keeping changed. Repaint and save.
    Save,
    /// Handled; repaint, save, and set the front light to this.
    Light(u8),
    /// The reader asked to leave the book.
    Close,
}

/// A book, open.
#[derive(Clone, Debug)]
pub struct Reader {
    document: Document,
    memory: Memory,
    pages: Vec<Vec<Piece>>,
    page: usize,
    chrome: Chrome,
    problem: Option<String>,
    /// Whether the last page is the end of the book or merely where it stopped.
    ///
    /// A copy that arrived cut short, or one so long that pagination hit its
    /// ceiling, both end in exactly the same way: a page, and then nothing.
    /// Silence there reads as "the end", which is the one thing it is not.
    cut: bool,
}

impl Reader {
    /// Opens a document wherever the last [`Memory`] left it.
    ///
    /// Pagination happens here, once: it is the expensive part, and doing it
    /// per repaint would put a whole novel through the wrapper every time
    /// somebody turned a page.
    #[must_use]
    pub fn open(document: Document, memory: Memory, panel: &DisplayMetrics) -> Self {
        let mut reader = Self {
            document,
            memory,
            pages: Vec::new(),
            page: 0,
            chrome: Chrome::Hidden,
            problem: None,
            cut: false,
        };
        reader.repaginate(panel);
        reader
    }

    /// Rebuilds the pages and goes back to the block that was at the top.
    ///
    /// Everything about type size and highlighting is built on this. Position
    /// first, pages second: the block index does not change when the setting
    /// does, which is the whole reason a position is stored the way it is.
    fn repaginate(&mut self, panel: &DisplayMetrics) {
        // The panel measured at the size this reader is set to. A page
        // measured at one size and drawn at another loses its last lines, and
        // the layout engine drops them without saying anything.
        let mut metrics = *panel;
        metrics.text_scale = self.memory.scale;
        let area = metrics.prose_area_in(true, self.chrome != Chrome::Hidden, Face::Reading);
        // Measured with the type at the size the screen will ask for. The
        // scale has to be ambient while this runs, because the wrapper and the
        // line height both read it -- and the screen carries the same value,
        // so what was measured here is what gets drawn.
        let (pages, capped) = kobo_ui::with_text_scale(self.memory.scale, || {
            paginate(&self.document, &self.memory.highlights, &metrics, area)
        });
        self.pages = pages;
        self.cut = capped || self.document.truncated;
        self.page = self.page_holding(self.memory.at);
    }

    /// The page a block lands on.
    ///
    /// Never a panic and never page one for want of an answer: both lose a
    /// reader's place in a way they cannot undo. A block past the end — a
    /// re-download of a shorter edition, say — falls back to the nearest
    /// earlier one.
    fn page_holding(&self, block: Locator) -> usize {
        self.pages
            .iter()
            .position(|page| page.iter().any(|piece| piece.block == block))
            .or_else(|| {
                self.pages
                    .iter()
                    .rposition(|page| page.iter().any(|piece| piece.block <= block))
            })
            .unwrap_or(0)
    }

    /// The block at the top of the current page, which is what gets kept.
    fn top(&self) -> Locator {
        self.pages
            .get(self.page)
            .and_then(|page| page.first())
            .map_or(0, |piece| piece.block)
    }

    fn remember_position(&mut self) {
        self.memory.at = self.top();
    }

    /// Whether there is a page after this one.
    #[must_use]
    pub fn can_go_forward(&self) -> bool {
        self.page + 1 < self.pages.len()
    }

    #[must_use]
    pub const fn can_go_back(&self) -> bool {
        self.page > 0
    }

    /// Turns forward. Returns whether anything moved.
    pub fn forward(&mut self) -> bool {
        if !self.can_go_forward() {
            return false;
        }
        self.page += 1;
        self.remember_position();
        true
    }

    /// Turns back. Returns whether anything moved.
    pub fn backward(&mut self) -> bool {
        if !self.can_go_back() {
            return false;
        }
        self.page -= 1;
        self.remember_position();
        true
    }

    /// Goes to a block, and puts away whatever was open over the book.
    ///
    /// This is what a tapped mark does. The chrome closes because the reader
    /// asked to be taken somewhere, and leaving the list up over the place
    /// they asked for would need a second tap to see it.
    pub fn go_to(&mut self, block: Locator, panel: &DisplayMetrics) {
        self.memory.at = block;
        self.chrome = Chrome::Hidden;
        self.repaginate(panel);
    }

    #[must_use]
    pub const fn page_number(&self) -> usize {
        self.page + 1
    }

    #[must_use]
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// One step larger, if there is one. Returns whether anything changed.
    pub fn larger(&mut self, panel: &DisplayMetrics) -> bool {
        let next = match self.memory.scale {
            TextScale::Default => TextScale::Large,
            TextScale::Large => TextScale::ExtraLarge,
            TextScale::ExtraLarge => return false,
        };
        self.set_scale(next, panel);
        true
    }

    /// One step smaller, if there is one. Returns whether anything changed.
    pub fn smaller(&mut self, panel: &DisplayMetrics) -> bool {
        let next = match self.memory.scale {
            TextScale::ExtraLarge => TextScale::Large,
            TextScale::Large => TextScale::Default,
            TextScale::Default => return false,
        };
        self.set_scale(next, panel);
        true
    }

    fn set_scale(&mut self, scale: TextScale, panel: &DisplayMetrics) {
        // The remembered block is deliberately *not* re-read from the page
        // first. It is already the finest answer there is to where the reader
        // is, and the top of the current page is a coarser one: taking it
        // would walk somebody backwards through the book a little on every
        // single adjustment, which is exactly when they are pressing the
        // button repeatedly.
        self.memory.scale = scale;
        self.repaginate(panel);
    }

    #[must_use]
    pub const fn scale(&self) -> TextScale {
        self.memory.scale
    }

    /// Brightens the front light by one step, stopping at full.
    pub fn brighter(&mut self) -> u8 {
        let level = self
            .memory
            .light
            .unwrap_or(0)
            .saturating_add(LIGHT_STEP)
            .min(100);
        self.memory.light = Some(level);
        level
    }

    /// Dims it by one step, stopping at off.
    pub fn dimmer(&mut self) -> u8 {
        let level = self.memory.light.unwrap_or(0).saturating_sub(LIGHT_STEP);
        self.memory.light = Some(level);
        level
    }

    #[must_use]
    pub const fn light(&self) -> Option<u8> {
        self.memory.light
    }

    /// Whether any of the words on this page are bookmarked.
    ///
    /// A mark sits on a *block*, not on a page, and asks whether this page is
    /// showing it -- rather than whether this page happens to *begin* with it.
    /// The difference is the whole feature: making the type larger moves a
    /// marked paragraph down into the middle of its page, and a mark that only
    /// counted at the top would silently come off exactly then. A reader would
    /// find their bookmarks gone and have no way to tell why.
    #[must_use]
    pub fn is_bookmarked(&self) -> bool {
        self.bookmark_here().is_some()
    }

    /// The bookmarked block on this page, if there is one.
    fn bookmark_here(&self) -> Option<Locator> {
        self.pages
            .get(self.page)?
            .iter()
            .map(|piece| piece.block)
            .find(|block| self.memory.bookmarks.contains(block))
    }

    /// Adds or removes this page's bookmark. Returns whether it is now on.
    ///
    /// Removing takes off whichever mark this page is showing, so the tap that
    /// lit the icon is the tap that puts it out -- even if the type size has
    /// changed since, and the mark is no longer on the first paragraph.
    pub fn toggle_bookmark(&mut self) -> bool {
        if let Some(existing) = self.bookmark_here() {
            self.memory.bookmarks.remove(&existing);
            false
        } else {
            let at = self.top();
            self.memory.bookmarks.insert(at);
            true
        }
    }

    /// Every bookmark, in reading order, with the words it sits on.
    #[must_use]
    pub fn bookmarks(&self) -> Vec<(Locator, String)> {
        self.opening_of(&self.memory.bookmarks)
    }

    /// Marks or unmarks one paragraph.
    ///
    /// Repaginates, because a marked paragraph is set narrower and so runs to
    /// more lines than it did a moment ago.
    pub fn toggle_highlight(&mut self, block: Locator, panel: &DisplayMetrics) -> bool {
        // The reader is anchored to the paragraph they just acted on, rather
        // than left where they were. Marking sets a paragraph narrower, so it
        // runs to more lines than it did a moment ago and can be pushed onto
        // the next page by its own mark -- taking with it both the
        // confirmation that anything happened and the only way to take the
        // mark off again.
        let on_screen = self.page().iter().any(|piece| piece.block == block);
        let on = if self.memory.highlights.remove(&block) {
            false
        } else {
            self.memory.highlights.insert(block);
            true
        };
        if on_screen {
            self.memory.at = block;
        }
        self.repaginate(panel);
        on
    }

    /// Every marked paragraph, in reading order, with the start of its text.
    #[must_use]
    pub fn highlights(&self) -> Vec<(Locator, String)> {
        self.opening_of(&self.memory.highlights)
    }

    fn opening_of(&self, blocks: &BTreeSet<Locator>) -> Vec<(Locator, String)> {
        blocks
            .iter()
            .filter_map(|block| {
                let text = self
                    .document
                    .blocks
                    .get(usize::try_from(*block).ok()?)?
                    .text()?;
                Some((*block, first_words(text)))
            })
            .collect()
    }

    /// The paragraphs on this page that can be marked, for building a picker.
    ///
    /// Furniture is left out: there is nothing to highlight about a rule, and
    /// offering it would put rows in the list that do nothing when tapped.
    #[must_use]
    pub fn markable(&self) -> Vec<(Locator, String)> {
        let mut seen: Vec<(Locator, String)> = Vec::new();
        for piece in self.page() {
            if piece.kind.is_furniture() || piece.text.trim().is_empty() {
                continue;
            }
            if seen.iter().any(|(block, _)| *block == piece.block) {
                continue;
            }
            seen.push((piece.block, first_words(&piece.text)));
        }
        seen
    }

    #[must_use]
    pub const fn chrome(&self) -> Chrome {
        self.chrome
    }

    /// Shows or puts away something over the book.
    ///
    /// Repaginates when the bar appears or goes, because the bar takes room
    /// off the page: without this the last lines would be drawn under it and
    /// silently dropped.
    pub fn set_chrome(&mut self, chrome: Chrome, panel: &DisplayMetrics) {
        if self.chrome == chrome {
            return;
        }
        // The position is left alone; see `set_scale`.
        let had_bar = self.chrome != Chrome::Hidden;
        self.chrome = chrome;
        if had_bar != (chrome != Chrome::Hidden) {
            self.repaginate(panel);
        }
    }

    /// Says something went wrong, on the next repaint.
    pub fn report(&mut self, problem: impl Into<String>) {
        self.problem = Some(problem.into());
    }

    #[must_use]
    pub const fn memory(&self) -> &Memory {
        &self.memory
    }

    /// Puts a kept memory back into an open book.
    ///
    /// For the case where the book and the place it was left arrive
    /// separately, which is the ordinary one: the text comes over the radio
    /// and the position comes out of the store, and neither waits for the
    /// other. Reopening the whole reader instead would throw away however much
    /// of the book had already arrived.
    pub fn restore(&mut self, memory: Memory, panel: &DisplayMetrics) {
        self.memory = memory;
        self.repaginate(panel);
    }

    #[must_use]
    pub const fn document(&self) -> &Document {
        &self.document
    }

    /// The current page, for an application that wants to draw it itself.
    #[must_use]
    pub fn page(&self) -> &[Piece] {
        self.pages.get(self.page).map_or(&[], Vec::as_slice)
    }

    /// Applies one named action.
    pub fn act(&mut self, name: &str, panel: &DisplayMetrics) -> Outcome {
        self.problem = None;
        match name {
            action::FORWARD => {
                if self.forward() {
                    Outcome::Save
                } else {
                    self.report("This is the end of the book.");
                    Outcome::Repaint
                }
            }
            action::BACK => {
                if self.backward() {
                    Outcome::Save
                } else {
                    self.report("This is the beginning of the book.");
                    Outcome::Repaint
                }
            }
            action::CONTROLS => {
                let next = if self.chrome == Chrome::Hidden {
                    Chrome::Controls
                } else {
                    Chrome::Hidden
                };
                self.set_chrome(next, panel);
                Outcome::Repaint
            }
            action::HIGHLIGHTS => {
                self.set_chrome(Chrome::Highlights, panel);
                Outcome::Repaint
            }
            action::MARKING => {
                self.set_chrome(Chrome::Marking, panel);
                Outcome::Repaint
            }
            action::LARGER => {
                let changed = self.larger(panel);
                self.resized(changed, "That is the largest size.")
            }
            action::SMALLER => {
                let changed = self.smaller(panel);
                self.resized(changed, "That is the smallest size.")
            }
            action::BRIGHTER => Outcome::Light(self.brighter()),
            action::DIMMER => Outcome::Light(self.dimmer()),
            action::BOOKMARK => {
                self.toggle_bookmark();
                Outcome::Save
            }
            action::CLOSE => Outcome::Close,
            other => {
                if let Some(block) = target_of(other) {
                    if other.starts_with(action::MARK) {
                        self.toggle_highlight(block, panel);
                    } else {
                        self.go_to(block, panel);
                    }
                    return Outcome::Save;
                }
                Outcome::Elsewhere
            }
        }
    }

    /// The same as [`Self::act`], for an application that is handed a hashed
    /// identifier rather than a name.
    ///
    /// Names are hashed on the way into a screen and the hash is what comes
    /// back, so there is no way to recover a name from one. The reader is the
    /// only thing that knows every name it might have put on a screen, so it
    /// is the only thing in a position to try them -- an application doing it
    /// would have to keep its own copy of that list in step, and the failure
    /// when it drifted would be a control that silently did nothing.
    pub fn act_on(&mut self, action: kobo_ui::ActionId, panel: &DisplayMetrics) -> Outcome {
        let mut names: Vec<String> = vec![
            action::FORWARD.into(),
            action::BACK.into(),
            action::CONTROLS.into(),
            action::CLOSE.into(),
            action::LARGER.into(),
            action::SMALLER.into(),
            action::BRIGHTER.into(),
            action::DIMMER.into(),
            action::BOOKMARK.into(),
            action::HIGHLIGHTS.into(),
            action::MARKING.into(),
        ];
        // Only the blocks that are actually on a screen right now, so this
        // stays a few dozen comparisons rather than one per block in a novel.
        for (block, _) in self.markable() {
            names.push(format!("{}{block}", action::MARK));
        }
        for block in self.memory.highlights.iter().chain(&self.memory.bookmarks) {
            names.push(format!("{}{block}", action::GO));
        }
        let Some(name) = names
            .into_iter()
            .find(|name| kobo_sdk::action_id(name) == action)
        else {
            return Outcome::Elsewhere;
        };
        self.act(&name, panel)
    }

    fn resized(&mut self, possible: bool, refusal: &str) -> Outcome {
        if possible {
            Outcome::Save
        } else {
            self.report(refusal);
            Outcome::Repaint
        }
    }

    /// Draws whatever the reader should be looking at.
    #[must_use]
    pub fn screen(&self, title: &str) -> Screen {
        match self.chrome {
            Chrome::Highlights => self.marks_screen(title),
            Chrome::Marking => self.marking_screen(title),
            Chrome::Controls | Chrome::Hidden => self.book_screen(title),
        }
    }

    fn book_screen(&self, title: &str) -> Screen {
        let mut screen = ScreenBuilder::new("reader")
            .reading(true)
            .text_scale(self.memory.scale)
            .top_bar(title);
        for piece in self.page() {
            screen = match piece.kind {
                Kind::Heading(_) => screen.heading(piece.text.clone()),
                Kind::Marked | Kind::Quote | Kind::Item => {
                    screen.quote(HIGHLIGHT_DEPTH, piece.text.clone())
                }
                Kind::Rule => screen.divider(),
                Kind::Break => screen.spacer(kobo_ui::Space::Small),
                Kind::Body | Kind::Preformatted => screen.text(piece.text.clone()),
            };
        }
        if let Some(problem) = &self.problem {
            screen = screen.banner(BannerLevel::Attention, problem.clone());
        } else if self.cut && !self.can_go_forward() {
            // Said on the last page, where somebody is deciding whether they
            // have finished the book. Anywhere earlier it is noise; here it is
            // the difference between an ending and a broken download.
            screen = screen.banner(
                BannerLevel::Attention,
                "This copy stops here rather than ending. Some of the book is missing.",
            );
        }
        // The gesture is what gets used; the bar is how anyone learns the
        // gesture is there. Tapping the side of the panel turns the page on
        // every Kobo ever made, and a reader holding one already knows that.
        screen = screen.page_turns(action::BACK, action::FORWARD);
        if self.chrome == Chrome::Hidden {
            // Nothing at the foot at all. This is the point of the reading
            // screen: a book, and the reader's own hands.
            return screen.build();
        }
        // Five, because five is what the panel physically carries: a bar is
        // clamped to what a finger can hit across the width of the display,
        // and the sixth control is not shrunk or wrapped, it is dropped. This
        // bar had six, and the one that vanished was the way to the marked
        // passages -- so every note anybody made was unreachable, on the
        // device and nowhere else. Bookmarking moved in beside them, which is
        // where the rest of the keeping lives anyway.
        screen
            .nav_bar(
                None,
                [
                    (action::SMALLER, "A-"),
                    (action::LARGER, "A+"),
                    (action::DIMMER, "Dim"),
                    (action::BRIGHTER, "Light"),
                    (action::HIGHLIGHTS, "Notes"),
                ],
            )
            .build()
    }

    fn marks_screen(&self, title: &str) -> Screen {
        let mut screen = ScreenBuilder::new("reader-marks").top_bar(title);
        let marks = self.highlights();
        let places = self.bookmarks();
        if marks.is_empty() && places.is_empty() {
            screen = screen.secondary(
                "Nothing is marked in this book yet. Mark a paragraph to keep the words, or bookmark a page to keep your place.",
            );
        }
        if !marks.is_empty() {
            screen = screen.heading("Marked passages");
            for (block, text) in marks {
                screen = screen.button(format!("{}{block}", action::GO), text);
            }
        }
        if !places.is_empty() {
            // Kept apart because they answer different questions: a passage is
            // something a reader wanted to keep, a bookmark is somewhere they
            // meant to come back to. Run together, the list of one buries the
            // other.
            screen = screen.heading("Bookmarks");
            for (block, text) in places {
                screen = screen.button(format!("{}{block}", action::GO), text);
            }
        }
        let mut bar = vec![
            (action::CONTROLS, "Back to the book"),
            (
                action::BOOKMARK,
                if self.is_bookmarked() {
                    "Remove bookmark"
                } else {
                    "Bookmark this page"
                },
            ),
        ];
        if !self.markable().is_empty() {
            // The only way in to marking a passage. It lives here rather than
            // on the reading bar, which is already as wide as the panel will
            // carry, and because somebody looking at their marks is exactly
            // the person about to make another.
            bar.push((action::MARKING, "Mark a paragraph"));
        }
        screen.nav_bar(None, bar).build()
    }

    /// The paragraphs on this page, each a tap away from being marked.
    fn marking_screen(&self, title: &str) -> Screen {
        let mut screen = ScreenBuilder::new("reader-marking")
            .top_bar(title)
            .secondary("Tap a paragraph to mark it, or to take the mark off.");
        for (block, text) in self.markable() {
            let marked = self.memory.highlights.contains(&block);
            // The state is in the row, because the list is the only place it
            // can be seen: the page underneath is not on screen while this is.
            // Said in words rather than with a tick: the reading face has no
            // check mark in it, and a screen carrying a character the face
            // cannot draw is refused outright -- correctly, because the
            // alternative is an empty box where the answer should be.
            let label = if marked {
                format!("Marked: {text}")
            } else {
                text
            };
            screen = screen.button(format!("{}{block}", action::MARK), label);
        }
        screen
            .nav_bar(
                None,
                [
                    (action::CONTROLS, "Back to the book"),
                    (action::HIGHLIGHTS, "Notes"),
                ],
            )
            .build()
    }
}

/// The block a `reader-mark-` or `reader-go-` action refers to.
#[must_use]
pub fn target_of(name: &str) -> Option<Locator> {
    name.strip_prefix(action::MARK)
        .or_else(|| name.strip_prefix(action::GO))
        .and_then(|rest| rest.parse().ok())
}

/// The opening of a paragraph, for a list that has to fit on one row.
fn first_words(text: &str) -> String {
    const MOST: usize = 60;
    let trimmed = text.trim();
    if trimmed.chars().count() <= MOST {
        return trimmed.to_owned();
    }
    let mut out: String = trimmed.chars().take(MOST).collect();
    // Cut at a word rather than mid-syllable, unless there is no space late
    // enough to cut at -- which is what a long URL looks like.
    if let Some(space) = out.rfind(' ') {
        if space > MOST / 2 {
            out.truncate(space);
        }
    }
    out.push('\u{2026}');
    out
}

/// Breaks a document into pages that fit, at their real sizes.
///
/// Returns the pages, and whether it stopped at the ceiling with book left --
/// which the caller has to say out loud rather than present as an ending.
fn paginate(
    document: &Document,
    highlights: &BTreeSet<Locator>,
    metrics: &kobo_ui::DisplayMetrics,
    area: ProseArea,
) -> (Vec<Vec<Piece>>, bool) {
    let mut pages: Vec<Vec<Piece>> = Vec::new();
    let mut page: Vec<Piece> = Vec::new();
    let mut used = 0;
    let gap = metrics.space(kobo_ui::Space::Small);
    if area.width <= 0 || area.height <= 0 {
        return (pages, !document.blocks.is_empty());
    }
    let mut capped = false;

    for (index, block) in document.blocks.iter().enumerate() {
        let Ok(index) = Locator::try_from(index) else {
            break;
        };
        if pages.len() >= MAX_PAGES {
            capped = true;
            break;
        }
        let kind = kind_of(block, highlights.contains(&index));
        let size = kind.size();
        let height = size.line_height_in(area.face);
        let (_, width) = quote_offsets(metrics, area.width, kind.depth());

        // Furniture takes a line's worth of room and carries no words, so it
        // is placed rather than wrapped -- and never left alone at the top of
        // a page, where a rule with nothing above it reads as a mistake.
        if kind.is_furniture() {
            if page.is_empty() {
                continue;
            }
            if used + gap + height > area.height {
                pages.push(std::mem::take(&mut page));
                used = 0;
                continue;
            }
            used += gap + height;
            page.push(Piece {
                block: index,
                text: String::new(),
                kind,
            });
            continue;
        }

        let Some(text) = block.text() else { continue };
        let lines = wrap_text_in(text, width, size, area.face);
        if lines.is_empty() {
            continue;
        }

        let mut placed = 0;
        while placed < lines.len() {
            let room = area.height - used - if page.is_empty() { 0 } else { gap };
            let fits = if room < height {
                0
            } else {
                usize::try_from(room / height).unwrap_or(usize::MAX)
            };
            let left = lines.len() - placed;
            // Either the rest fits, or enough of it fits to be worth breaking:
            // two lines here and two over the page. Anything less and the
            // whole paragraph goes over rather than leaving a widow behind.
            let take = if fits >= left {
                left
            } else if fits >= MIN_KEEP_LINES && left - fits >= MIN_KEEP_LINES {
                fits
            } else {
                0
            };
            if take == 0 {
                if page.is_empty() {
                    // One paragraph taller than the whole page. Cutting it
                    // anywhere beats looping forever, and the reader would far
                    // rather see the words than an empty panel.
                    let forced = usize::try_from(area.height / height).unwrap_or(1).max(1);
                    let end = (placed + forced).min(lines.len());
                    page.push(Piece {
                        block: index,
                        text: lines[placed..end].join(" "),
                        kind,
                    });
                    placed = end;
                }
                pages.push(std::mem::take(&mut page));
                used = 0;
                if pages.len() >= MAX_PAGES {
                    return (pages, true);
                }
                continue;
            }
            if !page.is_empty() {
                used += gap;
            }
            used += i32::try_from(take).unwrap_or(i32::MAX) * height;
            page.push(Piece {
                block: index,
                text: lines[placed..placed + take].join(" "),
                kind,
            });
            placed += take;
        }
    }
    if !page.is_empty() {
        pages.push(page);
    }
    (pages, capped)
}

fn kind_of(block: &Block, marked: bool) -> Kind {
    if marked && block.text().is_some() {
        return Kind::Marked;
    }
    match block {
        Block::Heading { level, .. } => Kind::Heading(*level),
        Block::Paragraph(_) => Kind::Body,
        Block::Quote(_) => Kind::Quote,
        Block::Preformatted(_) => Kind::Preformatted,
        Block::Item { .. } => Kind::Item,
        Block::Rule => Kind::Rule,
        Block::Break => Kind::Break,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The panel of a Clara BW, which is the device this is written for.
    fn panel() -> DisplayMetrics {
        DisplayMetrics::default()
    }

    pub(super) fn book(paragraphs: usize) -> Document {
        let mut blocks = vec![Block::Heading {
            level: 1,
            text: "Chapter I".into(),
        }];
        for index in 0..paragraphs {
            blocks.push(Block::Paragraph(format!(
                "Paragraph number {index}. It is a truth universally acknowledged, that a single \
                 man in possession of a good fortune, must be in want of a wife. However little \
                 known the feelings or views of such a man may be on his first entering a \
                 neighbourhood, this truth is so well fixed in the minds of the surrounding \
                 families, that he is considered as the rightful property of some one or other of \
                 their daughters."
            )));
        }
        Document {
            title: Some("Pride and Prejudice".into()),
            author: Some("Jane Austen".into()),
            blocks,
            truncated: false,
        }
    }

    fn reader(paragraphs: usize) -> Reader {
        Reader::open(book(paragraphs), Memory::default(), &panel())
    }

    #[test]
    fn a_book_breaks_into_more_than_one_page() {
        let reader = reader(40);
        assert!(
            reader.page_count() > 5,
            "forty long paragraphs fitted in {} pages",
            reader.page_count()
        );
        assert!(!reader.page().is_empty());
    }

    #[test]
    fn every_block_lands_on_exactly_one_stretch_of_pages() {
        // Nothing may be dropped and nothing repeated: a book missing a
        // paragraph reads as a book, which is why this is asserted rather
        // than eyeballed.
        let document = book(30);
        let reader = Reader::open(document.clone(), Memory::default(), &panel());
        let mut seen: Vec<Locator> = Vec::new();
        for page in 0..reader.page_count() {
            let mut at = reader.clone();
            at.page = page;
            for piece in at.page() {
                if seen.last() != Some(&piece.block) {
                    assert!(
                        !seen.contains(&piece.block),
                        "block {} came back after another block",
                        piece.block
                    );
                    seen.push(piece.block);
                }
            }
        }
        let expected: Vec<Locator> = (0..u32::try_from(document.blocks.len()).unwrap()).collect();
        assert_eq!(seen, expected, "a block was lost or reordered");
    }

    #[test]
    fn making_the_type_larger_keeps_the_reader_where_they_were() {
        // The whole reason a position is a block index. If this ever fails,
        // every reader loses their place the first time they touch A+.
        let mut reader = reader(60);
        for _ in 0..7 {
            reader.forward();
        }
        let before = reader.page().first().unwrap().block;
        let pages_before = reader.page_count();

        assert!(reader.larger(&panel()));
        assert!(
            reader.page().iter().any(|piece| piece.block == before),
            "the reader was moved off the words they were on by a change of type size"
        );
        assert!(
            reader.page_count() > pages_before,
            "larger type did not make more pages: {} then {}",
            pages_before,
            reader.page_count()
        );

        assert!(reader.smaller(&panel()));
        assert!(reader.page().iter().any(|piece| piece.block == before));
        assert_eq!(reader.page_count(), pages_before);
    }

    #[test]
    fn the_type_size_stops_at_both_ends_rather_than_wrapping() {
        let mut reader = reader(10);
        assert!(reader.larger(&panel()));
        assert!(reader.larger(&panel()));
        assert!(!reader.larger(&panel()), "there was a fourth size");
        assert_eq!(reader.scale(), TextScale::ExtraLarge);
        assert!(reader.smaller(&panel()));
        assert!(reader.smaller(&panel()));
        assert!(!reader.smaller(&panel()));
        assert_eq!(reader.scale(), TextScale::Default);
    }

    #[test]
    fn showing_the_controls_repaginates_because_they_take_room() {
        // Without this the last lines of the page are drawn under the bar and
        // dropped, with nothing on the panel to say they existed.
        let mut reader = reader(60);
        for _ in 0..4 {
            reader.forward();
        }
        let before = reader.page().first().unwrap().block;
        let pages_before = reader.page_count();
        reader.set_chrome(Chrome::Controls, &panel());
        assert!(
            reader.page_count() > pages_before,
            "the control bar took no room off the page"
        );
        assert!(
            reader.page().iter().any(|piece| piece.block == before),
            "opening the controls moved the reader off their words"
        );
    }

    #[test]
    fn a_bookmark_is_still_on_the_same_words_at_another_type_size() {
        let mut reader = reader(60);
        for _ in 0..6 {
            reader.forward();
        }
        assert!(reader.toggle_bookmark());
        assert!(reader.is_bookmarked());
        let marked = reader.page().first().unwrap().block;

        reader.larger(&panel());
        assert!(
            reader.is_bookmarked(),
            "the bookmark came off when the type changed"
        );
        assert!(reader.page().iter().any(|piece| piece.block == marked));

        assert!(!reader.toggle_bookmark());
        assert!(!reader.is_bookmarked());
    }

    #[test]
    fn a_highlight_sets_its_paragraph_narrower_and_still_holds_the_place() {
        let mut reader = reader(60);
        for _ in 0..5 {
            reader.forward();
        }
        let top = reader.page().first().unwrap().block;
        let target = reader.markable().last().expect("something to mark").0;

        assert!(reader.toggle_highlight(target, &panel()));
        assert!(
            reader.page().iter().any(|piece| piece.block == top),
            "marking a paragraph moved the reader off their words"
        );
        assert_eq!(
            reader.highlights().first().map(|(block, _)| *block),
            Some(target)
        );

        assert!(!reader.toggle_highlight(target, &panel()));
        assert!(reader.highlights().is_empty());
    }

    #[test]
    fn a_marked_paragraph_is_drawn_as_a_quote() {
        let mut reader = reader(20);
        let target = reader.markable().last().expect("something to mark").0;
        reader.toggle_highlight(target, &panel());
        let at = reader.page_holding(target);
        let mut showing = reader.clone();
        showing.page = at;
        assert!(
            showing
                .page()
                .iter()
                .any(|piece| piece.block == target && piece.kind == Kind::Marked),
            "the marked paragraph is set like every other one"
        );
    }

    #[test]
    fn tapping_a_mark_goes_to_it_and_puts_the_list_away() {
        let mut reader = reader(80);
        for _ in 0..12 {
            reader.forward();
        }
        let target = reader.markable().first().expect("something to mark").0;
        reader.toggle_highlight(target, &panel());
        for _ in 0..20 {
            reader.forward();
        }
        reader.set_chrome(Chrome::Highlights, &panel());

        let outcome = reader.act(&format!("{}{target}", action::GO), &panel());
        assert_eq!(outcome, Outcome::Save);
        assert_eq!(reader.chrome(), Chrome::Hidden);
        assert!(
            reader.page().iter().any(|piece| piece.block == target),
            "going to a mark did not land on it"
        );
    }

    #[test]
    fn the_reading_screen_carries_nothing_at_the_foot() {
        // A book and the reader's hands. The controls are a deliberate step
        // away from that, not the resting state.
        let mut reader = reader(20);
        let bare = reader.screen("Pride and Prejudice");
        assert!(bare.nav_bar.is_none(), "the plain reading page had a bar");
        reader.set_chrome(Chrome::Controls, &panel());
        assert!(reader.screen("Pride and Prejudice").nav_bar.is_some());
    }

    #[test]
    fn turning_past_either_end_says_so_rather_than_doing_nothing() {
        // A control that does nothing when tapped reads as a broken panel.
        let mut reader = reader(20);
        assert_eq!(reader.act(action::BACK, &panel()), Outcome::Repaint);
        assert!(reader.problem.is_some());
        while reader.forward() {}
        assert_eq!(reader.act(action::FORWARD, &panel()), Outcome::Repaint);
        assert!(reader.problem.is_some());
    }

    #[test]
    fn the_front_light_moves_in_steps_and_stops_at_both_ends() {
        let mut reader = reader(4);
        assert_eq!(reader.light(), None, "a light level was invented");
        assert_eq!(reader.act(action::BRIGHTER, &panel()), Outcome::Light(10));
        for _ in 0..20 {
            reader.brighter();
        }
        assert_eq!(reader.light(), Some(100));
        for _ in 0..20 {
            reader.dimmer();
        }
        assert_eq!(reader.light(), Some(0), "off is a setting, not an absence");
    }

    #[test]
    fn a_memory_survives_being_written_and_read() {
        let mut reader = reader(60);
        for _ in 0..9 {
            reader.forward();
        }
        reader.toggle_bookmark();
        let target = reader.markable().first().unwrap().0;
        reader.toggle_highlight(target, &panel());
        reader.larger(&panel());
        reader.larger(&panel());
        reader.brighter();

        let kept = Memory::decode(&reader.memory().encode());
        assert_eq!(&kept, reader.memory());

        // And reopening lands on the same words, which is what all of it is for.
        let reopened = Reader::open(book(60), kept, &panel());
        assert_eq!(
            reopened.page().first().unwrap().block,
            reader.page().first().unwrap().block
        );
        assert_eq!(reopened.scale(), TextScale::ExtraLarge);
        assert!(reopened.is_bookmarked());
    }

    /// Every word a screen puts on the panel, in order.
    fn texts(screen: &kobo_ui::Screen) -> Vec<String> {
        fn walk(nodes: &[kobo_ui::Node], out: &mut Vec<String>) {
            for node in nodes {
                match node {
                    kobo_ui::Node::Heading { text, .. }
                    | kobo_ui::Node::Text { text, .. }
                    | kobo_ui::Node::Secondary { text, .. }
                    | kobo_ui::Node::Quote { text, .. }
                    | kobo_ui::Node::Banner { text, .. } => out.push(text.clone()),
                    kobo_ui::Node::Button { label, .. } => out.push(label.clone()),
                    kobo_ui::Node::Card { children, .. } => walk(children, out),
                    _ => {}
                }
            }
        }
        let mut out = Vec::new();
        walk(&screen.nodes, &mut out);
        out
    }

    /// The action names a screen offers, buttons and bar alike.
    fn named_actions(screen: &kobo_ui::Screen) -> Vec<String> {
        // Names are hashed into identifiers on the way in, so the only way to
        // ask which action a row carries is to hash the name being looked for
        // and compare. Candidates are enumerated rather than guessed.
        let mut candidates: Vec<String> = vec![
            action::FORWARD.into(),
            action::BACK.into(),
            action::CONTROLS.into(),
            action::CLOSE.into(),
            action::LARGER.into(),
            action::SMALLER.into(),
            action::BRIGHTER.into(),
            action::DIMMER.into(),
            action::BOOKMARK.into(),
            action::HIGHLIGHTS.into(),
            action::MARKING.into(),
        ];
        for block in 0..200u32 {
            candidates.push(format!("{}{block}", action::MARK));
            candidates.push(format!("{}{block}", action::GO));
        }
        let mut present = Vec::new();
        for name in candidates {
            let wanted = kobo_sdk::action_id(&name);
            let on_bar = screen
                .nav_bar
                .as_ref()
                .is_some_and(|bar| bar.destinations.iter().any(|item| item.action == wanted));
            if on_bar || screen.nodes.iter().any(|node| holds(node, wanted)) {
                present.push(name);
            }
        }
        present
    }

    fn holds(node: &kobo_ui::Node, wanted: kobo_ui::ActionId) -> bool {
        match node {
            kobo_ui::Node::Button { action, .. } => *action == wanted,
            kobo_ui::Node::Card { children, .. } => {
                children.iter().any(|child| holds(child, wanted))
            }
            _ => false,
        }
    }

    #[test]
    fn a_paragraph_can_be_picked_off_a_list_because_a_finger_cannot_select_text() {
        let mut reader = reader(60);
        for _ in 0..3 {
            reader.forward();
        }
        assert_eq!(reader.act(action::HIGHLIGHTS, &panel()), Outcome::Repaint);
        assert_eq!(reader.act(action::MARKING, &panel()), Outcome::Repaint);
        // Read after the controls are up, because they take room off the page
        // and so change which paragraphs are on it. The list has to be of the
        // page the reader is actually looking at.
        let choices = reader.markable();
        assert!(!choices.is_empty(), "nothing on the page could be marked");
        let picker = reader.screen("Pride and Prejudice");
        let rows = named_actions(&picker);
        for (block, _) in &choices {
            assert!(
                rows.contains(&format!("{}{block}", action::MARK)),
                "paragraph {block} was on the page but not on the list"
            );
        }

        let target = choices.first().unwrap().0;
        assert_eq!(
            reader.act(&format!("{}{target}", action::MARK), &panel()),
            Outcome::Save
        );
        assert_eq!(
            reader.highlights().first().map(|(block, _)| *block),
            Some(target)
        );

        // And the list says so, so somebody can see what they have done
        // without going back to the page.
        let ticked = reader.screen("Pride and Prejudice");
        assert!(
            texts(&ticked)
                .iter()
                .any(|line| line.starts_with("Marked:")),
            "the marked paragraph was not shown as marked"
        );

        // And tapping it again takes the mark off, from the same row.
        assert_eq!(
            reader.act(&format!("{}{target}", action::MARK), &panel()),
            Outcome::Save
        );
        assert!(
            reader.highlights().is_empty(),
            "the mark could not be undone"
        );
    }

    #[test]
    fn the_notes_screen_keeps_passages_and_places_apart() {
        let mut reader = reader(60);
        for _ in 0..4 {
            reader.forward();
        }
        reader.toggle_bookmark();
        let marked = reader.markable().first().unwrap().0;
        reader.toggle_highlight(marked, &panel());

        reader.act(action::HIGHLIGHTS, &panel());
        let screen = reader.screen("Pride and Prejudice");
        let lines = texts(&screen);
        assert!(
            lines.iter().any(|line| line == "Marked passages"),
            "no heading for passages: {lines:?}"
        );
        assert!(
            lines.iter().any(|line| line == "Bookmarks"),
            "no heading for bookmarks: {lines:?}"
        );

        // Both are a way back into the book.
        let rows = named_actions(&screen);
        assert!(rows.iter().any(|name| name.starts_with(action::GO)));
    }

    #[test]
    fn an_empty_notes_screen_offers_no_way_to_mark_nothing() {
        // With no page under it there is nothing markable, and a button that
        // leads to an empty list is a dead end somebody has to back out of.
        let mut reader = Reader::open(
            Document {
                title: None,
                author: None,
                blocks: Vec::new(),
                truncated: false,
            },
            Memory::default(),
            &panel(),
        );
        reader.act(action::HIGHLIGHTS, &panel());
        let rows = named_actions(&reader.screen("Nothing"));
        assert!(!rows.iter().any(|name| name == action::MARKING));
    }

    #[test]
    fn every_control_the_reader_offers_reaches_the_panel() {
        // A bar is clamped to what the panel can physically carry and the rest
        // is dropped without a word, so a control that is declared is not
        // necessarily a control that exists. This is the check that the two
        // are the same number on every screen the reader draws.
        let mut reader = reader(60);
        reader.forward();
        let marked = reader.markable().first().unwrap().0;
        reader.toggle_highlight(marked, &panel());
        reader.toggle_bookmark();

        for chrome in [
            Chrome::Hidden,
            Chrome::Controls,
            Chrome::Highlights,
            Chrome::Marking,
        ] {
            reader.set_chrome(chrome, &panel());
            let screen = reader.screen("Pride and Prejudice");
            let Some(bar) = &screen.nav_bar else {
                continue;
            };
            assert_eq!(
                bar.visible(&panel()).len(),
                bar.destinations.len(),
                "{chrome:?} declared {} controls and the panel shows {}",
                bar.destinations.len(),
                bar.visible(&panel()).len()
            );
        }
    }

    #[test]
    fn the_marked_passages_can_be_reached_from_the_book() {
        // The defect this covers: the reading bar declared six controls, the
        // panel carries five, and the one dropped was the way to the notes.
        let mut reader = reader(60);
        reader.set_chrome(Chrome::Controls, &panel());
        let screen = reader.screen("Pride and Prejudice");
        let bar = screen.nav_bar.as_ref().expect("a bar");
        assert!(
            bar.visible(&panel())
                .iter()
                .any(|item| item.action == kobo_sdk::action_id(action::HIGHLIGHTS)),
            "there is no way from the book to the marks"
        );
    }

    #[test]
    fn a_book_that_arrived_cut_short_says_so_instead_of_ending() {
        let mut document = book(20);
        document.truncated = true;
        let mut reader = Reader::open(document, Memory::default(), &panel());

        // Not on the way there: a warning on every page is a warning nobody
        // reads by the time it matters.
        assert!(
            !texts(&reader.screen("Cut"))
                .iter()
                .any(|line| line.contains("Some of the book is missing")),
            "the warning was shown before the end"
        );

        while reader.forward() {}
        assert!(
            texts(&reader.screen("Cut"))
                .iter()
                .any(|line| line.contains("Some of the book is missing")),
            "a cut book ended in silence, which reads as the end"
        );
    }

    #[test]
    fn a_whole_book_ends_without_a_warning() {
        let mut reader = reader(20);
        while reader.forward() {}
        assert!(!texts(&reader.screen("Whole"))
            .iter()
            .any(|line| line.contains("Some of the book is missing")));
    }

    #[test]
    fn a_damaged_memory_costs_a_field_rather_than_the_book() {
        // A record that cannot be read at all means reopening at page one,
        // which is the failure this format exists to avoid.
        let kept = Memory::decode(b"at 42\nnonsense\nscale banana\nmark 7\nlight \nhigh 9\n");
        assert_eq!(kept.at, 42);
        assert_eq!(kept.scale, TextScale::Default);
        assert_eq!(kept.light, None);
        assert!(kept.bookmarks.contains(&7));
        assert!(kept.highlights.contains(&9));
    }

    #[test]
    fn a_position_past_the_end_lands_near_it_rather_than_at_the_beginning() {
        // A shorter edition of the same book. Sending the reader back to
        // page one would be the one thing they cannot undo.
        let memory = Memory {
            at: 5_000,
            ..Memory::default()
        };
        let reader = Reader::open(book(20), memory, &panel());
        assert_eq!(
            reader.page_number(),
            reader.page_count(),
            "a position past the end did not land on the last page"
        );
    }

    #[test]
    fn a_paragraph_taller_than_the_page_still_gets_drawn() {
        // Not hypothetical: a Gutenberg text with no blank lines in a section
        // arrives as one enormous block, and looping forever on it would hang
        // the application at the moment the book opened.
        let giant = "word ".repeat(20_000);
        let document = Document {
            title: None,
            author: None,
            blocks: vec![Block::Paragraph(giant)],
            truncated: false,
        };
        let reader = Reader::open(document, Memory::default(), &panel());
        assert!(reader.page_count() > 1);
        assert!(!reader.page().is_empty());
    }

    #[test]
    fn an_empty_document_opens_rather_than_panicking() {
        let document = Document {
            title: None,
            author: None,
            blocks: Vec::new(),
            truncated: false,
        };
        let mut reader = Reader::open(document, Memory::default(), &panel());
        assert_eq!(reader.page_count(), 0);
        assert!(reader.page().is_empty());
        assert!(!reader.forward());
        assert!(!reader.backward());
        assert!(reader.markable().is_empty());
        let _ = reader.screen("Nothing");
    }

    #[test]
    fn an_opening_is_cut_at_a_word_and_marked_as_cut() {
        let long = first_words(
            "It is a truth universally acknowledged, that a single man in possession of a good \
             fortune, must be in want of a wife.",
        );
        assert!(long.ends_with('\u{2026}'));
        assert!(long.chars().count() <= 61);
        assert!(
            !long.contains("acknowledge\u{2026}"),
            "cut mid-word: {long}"
        );

        // Nothing to cut at is not a reason to fail.
        let unbroken = first_words(&"x".repeat(200));
        assert!(unbroken.ends_with('\u{2026}'));

        // Short enough is left exactly alone.
        assert_eq!(first_words("  Chapter I  "), "Chapter I");
    }

    #[test]
    fn an_action_that_is_not_the_readers_is_left_for_the_application() {
        let mut reader = reader(4);
        assert_eq!(
            reader.act("gutenshelf-library", &panel()),
            Outcome::Elsewhere
        );
        assert_eq!(
            reader.act("reader-mark-notanumber", &panel()),
            Outcome::Elsewhere
        );
    }

    #[test]
    fn a_targets_block_is_read_back_off_its_action_name() {
        assert_eq!(target_of("reader-mark-17"), Some(17));
        assert_eq!(target_of("reader-go-17"), Some(17));
        assert_eq!(target_of("reader-forward"), None);
        assert_eq!(target_of("reader-go--3"), None);
    }
}

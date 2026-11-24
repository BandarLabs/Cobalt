//! The bundled library: documents already on this device.
//!
//! Books is an included application. It lists files on the card (`/mnt/onboard`,
//! `/mnt/sd`, the books Cobalt applications keep) and titles from the stock
//! Kobo library. EPUB, Markdown, HTML and plain text open in the shared reader,
//! at the page and text size they were closed at. PDF and Kobo Store books are
//! listed on a shelf of their own and say they open in the Kobo reader. A Kobo
//! Store title that has not been downloaded is listed and says so when opened.
//!
//! ## Why it opens on the book being read
//!
//! The stock reader's home is the book you are in the middle of, large, with
//! the way back into it one tap away, and the rest of the library as piles of
//! covers under it. Books used to open on a grid of every file in title order,
//! so the book somebody was halfway through was wherever its name sorted.
//! Now the last book opened is at the top with Continue reading, and the
//! library is three shelves under it: what opens here, what was opened
//! lately, and what opens in the Kobo reader.
//!
//! Web articles sent from the owner's computer have a shelf of their own,
//! Articles, kept apart from the books because they are read once and
//! thrown away. See [`articles`].
//!
//! ## Why it reads the books
//!
//! The runtime lists files; it does not open them. The covers, the real
//! titles of books kept under a blob's name, and the authors are inside the
//! books, so Books reads each one once, a few at a time while the shelf is on
//! the panel, and keeps what it learned. See [`covers`].

mod articles;
mod covers;
mod ranges;

use articles::{Kept, Said};
use covers::{Cover, Described, Recent, Thumb, DESCRIBED};
use kobo_bookview::positions::{Progress, KEY as READING};
use kobo_bookview::{BookView, Step};
use kobo_read::Outcome;
use kobo_sdk::clock::Clock as _;
use kobo_sdk::keyboard::{TextEntry, Typing};
use kobo_sdk::{
    action_id, document_preview, typographic_cover, ActionId, BandAlign, BannerLevel, Context,
    DeviceRequest, DeviceResult, Glyph, KoboApp, LibraryEntry, PictureHandle, Position, Screen,
    ScreenBuilder, SlotWidth, Space, StoreResult, Task, TaskId, TaskOutcome, Tile, TilePicture,
    TileShape, MAX_STORE_VALUE,
};
use ranges::{Arrival, Assembly, Fault};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::process::ExitCode;

/// How many books one shelf page holds.
///
/// Three columns of portrait tiles, two rows deep, matching gutenbird: that
/// is what fits whole between the bars on this panel.
#[cfg(test)]
const SHELF_PAGE: usize = 6;

const PREVIOUS: &str = "previous";
const NEXT: &str = "next";
const BACK: &str = "back";
const RELIST: &str = "relist";
const RETRY_SAVE: &str = "retry-save";
const CONTINUE: &str = "continue";
const SYNC: &str = "sync";
const ARCHIVED: &str = "archived";
const MARK_READ: &str = "article-read";
const MARK_UNREAD: &str = "article-unread";
const ARCHIVE: &str = "article-archive";
const RESTORE: &str = "article-restore";
const DELETE: &str = "article-delete";
const CONFIRM_DELETE: &str = "confirm-delete";
const KEEP_ARTICLE: &str = "keep-article";
const CANCEL_OPEN: &str = "cancel-open";
const OPEN_AGAIN: &str = "open-again";
const SEARCH: &str = "search";
const CLEAR_SEARCH: &str = "clear-search";

/// How long after one check Books leaves the computer alone when it comes
/// back to the panel. Opening Books asks; flicking in and out of it should
/// not ask every time.
const QUIET_CHECK_EVERY_MS: u64 = 5 * 60 * 1000;

/// Where to send an owner for anything Books cannot open itself.
const IN_KOBO_READER: &str = "To get there, tap Kobo reader at the foot of the Cobalt home screen.";

/// Store key for the books opened most recently, newest first.
const RECENT: &str = "recent-v1";
/// Store key for the order the owner keeps the Library shelf in.
const ORDER: &str = "library-order-v1";
/// How many recently opened books are remembered.
const RECENT_LIMIT: usize = 24;
/// How many covers are kept in the store.
///
/// An application's store holds 256 values. The rest are left for the
/// reading record, the two records here, and whatever comes next; a book
/// past this many has its cover read again when it is next on the panel.
const KEPT_COVERS: usize = 200;
/// How many decoded covers are held in memory at once.
const HELD_COVERS: usize = 48;

/// The largest book the runtime hands over in one answer: the wire's own
/// limit on a whole library document.
///
/// A book up to this size is asked for whole, in one round trip, as it always
/// was. It is also every book Books can open under a runtime that does not
/// answer ranges; see [`Ranges`].
const LARGEST_WHOLE: u32 = 4 * 1024 * 1024;

/// The largest book Books opens, read from the runtime a range at a time.
///
/// What this bounds is memory at the one moment a book costs the most. A
/// zip's directory is at its end, so the parser needs the whole file in hand
/// while it builds the document beside it. Out of any book, however large,
/// the parser keeps at most sixteen megabytes of text, eight of pictures and
/// four of publisher faces (the bounds in `kobo_doc`), so past a few
/// megabytes the only thing that still grows with the file is the file. At
/// this limit that moment is about sixty megabytes, and the file is let go
/// as soon as the first page is set. A larger book is listed, and says why it
/// does not open here rather than failing as if it were damaged.
///
/// Thirty-two is the most any application already holds as one file on this
/// hardware: the comic reader keeps an archive that size for a whole sitting,
/// where Books holds one for the moment it takes to open. The readers that
/// download their books stop at twelve and sixteen, and the runtime will not
/// list a document past sixty-four. The reader has under five hundred
/// megabytes for the runtime and every application it keeps alive, and the
/// cost of asking for more than is free is not a slow open: the kernel stops
/// the application that asked, and the owner is back at the launcher with
/// the book they were trying to read still closed.
///
/// It is also a wait. A megabyte is four round trips to the runtime, so a
/// book at the limit is a hundred and twenty-eight of them before its first
/// page, with the count on the panel. A book past it is nearly always
/// scanned pages, which the Kobo reader shows better than a reflowing one.
const LARGEST_OPENABLE: u32 = 32 * 1024 * 1024;

// The computer tells an owner whether a book will open before sending it,
// from the copy of this number the protocol carries.
const _: () = assert!(LARGEST_OPENABLE as usize == kobo_sdk::MAX_LIBRARY_OPENABLE_BYTES);

/// Whether the runtime under Books hands a book over a range at a time.
///
/// Books ships with the runtime, so the two are the same release except
/// between an update applied mid-session, which replaces Books on the card
/// while the old runtime is still running, and the next start. A runtime
/// from before ranges cannot read the request and ends the session of an
/// application that sends one, so Books never finds out by trying. It asks
/// the runtime which release it is, and uses ranges only under its own.
///
/// It asks the first time a listing holds a book too large to arrive whole,
/// because until then the answer changes nothing, and most libraries never
/// hold one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Ranges {
    /// No listing has held a book that needs them.
    Unasked,
    /// Asked, and the answer is on its way. A large book is shown as one
    /// that opens, since under the release Books shipped with it does, and a
    /// tap on one waits for the answer.
    Asked,
    /// The runtime is the release Books shipped with.
    Answered,
    /// Another release, or one that would not say. Books reads a book whole
    /// or not at all, as it did before ranges.
    Refused,
}

/// How wide the open book's cover is drawn at the top of Books.
const HERO_MM: u16 = 28;
/// Picture handles: the open book, the three shelf piles, then the tiles.
const HERO_HANDLE: u32 = 1;
const PILE_HANDLE: u32 = 2;
const TILE_HANDLE: u32 = 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    Epub,
    Markdown,
    Html,
    Text,
    Pdf,
    /// A Kobo Store book: listed with its real title, refused on a tap.
    Kepub,
}

impl Kind {
    const fn badge(self) -> &'static str {
        match self {
            Self::Epub => "epub",
            Self::Markdown => "md",
            Self::Html => "html",
            Self::Text => "txt",
            Self::Pdf => "pdf",
            Self::Kepub => "kepub",
        }
    }

    const fn is_readable(self) -> bool {
        !matches!(self, Self::Pdf | Self::Kepub)
    }

    /// Whether the book is a package with a cover and a title page to read.
    const fn is_packaged(self) -> bool {
        matches!(self, Self::Epub | Self::Kepub)
    }

    const fn filename(self) -> &'static str {
        match self {
            Self::Epub => "open-sea.epub",
            Self::Markdown => "notes.md",
            Self::Html => "streets.html",
            Self::Text => "letter.txt",
            Self::Pdf => "timetable.pdf",
            Self::Kepub => "purchase.kepub.epub",
        }
    }
}

#[derive(Clone, Debug)]
struct Document {
    id: String,
    title: String,
    /// What the runtime listed it as, before its own name replaced that.
    ///
    /// With its size, the only sign the file is still the one that was read:
    /// the listing says nothing else about it. A stock-database title or a
    /// package title changes with the book, so a file replaced by another of
    /// exactly the same size is still noticed.
    listed: String,
    /// From the book's own package, once it has been read.
    author: Option<String>,
    kind: Kind,
    /// Opening words, used as the tile for a document that is not a package.
    preview: String,
    on_card: bool,
    bytes: u32,
    /// Present only for in-process tests that never talk to the runtime.
    body: Option<String>,
}

impl Document {
    /// Whether it belongs on the shelf of things that open here, rather than
    /// the one of things that open in the Kobo reader.
    const fn belongs_here(&self) -> bool {
        self.on_card && self.kind.is_readable()
    }

    /// Whether tapping it opens it here rather than explaining why not,
    /// where `largest` is the most Books can open under this runtime.
    const fn opens_here(&self, largest: u32) -> bool {
        self.belongs_here() && self.bytes <= largest
    }

    /// Whether it has to be asked for a range at a time.
    const fn in_ranges(&self) -> bool {
        self.bytes > LARGEST_WHOLE
    }

    /// Whether Books can read it for a cover and a title.
    const fn describable(&self, largest: u32) -> bool {
        self.on_card && self.kind.is_packaged() && self.bytes <= largest && self.body.is_none()
    }
}

#[cfg(test)]
fn fixtures() -> Vec<Document> {
    let document = |id: &str, title: &str, kind, preview: &str, body: &str| Document {
        id: id.to_owned(),
        title: title.to_owned(),
        listed: title.to_owned(),
        author: None,
        kind,
        preview: preview.to_owned(),
        on_card: true,
        bytes: 0,
        body: Some(body.to_owned()),
    };
    vec![
        document(
            "test/open-sea.epub",
            "The Open Sea",
            Kind::Epub,
            "The tide was already turning when the boat left the harbour.",
            "The tide was already turning when the boat left the harbour.",
        ),
        document(
            "test/notes.md",
            "Notes on a Rainy Afternoon",
            Kind::Markdown,
            "The rain started before the kettle boiled.",
            "# Notes on a Rainy Afternoon\n\nThe rain started before the kettle boiled.",
        ),
        document(
            "test/timetable.pdf",
            "Timetable",
            Kind::Pdf,
            "Northbound 06:12",
            "",
        ),
        document(
            "test/letter.txt",
            "A Letter Home",
            Kind::Text,
            "Dear M,",
            "Dear M,\n\nThe hill behind the house is still green in September.",
        ),
        document(
            "test/streets.html",
            "Index of Streets",
            Kind::Html,
            "Market Street runs east from the harbour.",
            "<h1>Index of Streets</h1>\n<p>Market Street runs east from the harbour.</p>",
        ),
    ]
}

/// Where the record of described books stands against the store.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Record {
    /// Not read back yet, so nothing is described: a book read now might
    /// already be in the record and would be read twice.
    Unread,
    /// The same as the store holds.
    Kept,
    /// Holds something the store does not have yet.
    Changed,
}

/// One of the piles under the book being read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Shelf {
    /// Everything that opens here, recently opened first, but articles.
    Library,
    /// Web articles sent from the owner's computer, unread first.
    Articles,
    /// What was opened lately, newest first.
    Recent,
    /// Kobo Store books, PDFs and titles not yet downloaded.
    KoboReader,
    /// Articles put away, reached from the Articles shelf rather than home.
    Archive,
}

impl Shelf {
    /// The piles on the home screen, in the order they stand.
    const ALL: [Self; 4] = [
        Self::Library,
        Self::Articles,
        Self::Recent,
        Self::KoboReader,
    ];

    const fn name(self) -> &'static str {
        match self {
            Self::Library => "shelf-library",
            Self::Articles => "shelf-articles",
            Self::Recent => "shelf-recent",
            Self::KoboReader => "shelf-kobo-reader",
            Self::Archive => ARCHIVED,
        }
    }

    const fn title(self) -> &'static str {
        match self {
            Self::Library => "Library",
            Self::Articles => "Articles",
            Self::Recent => "Recent",
            Self::KoboReader => "Kobo reader",
            Self::Archive => "Archived articles",
        }
    }

    /// Whether the shelf is a list of articles rather than a grid of covers.
    const fn lists_articles(self) -> bool {
        matches!(self, Self::Articles | Self::Archive)
    }
}

/// The order the Library shelf stands in.
///
/// Recently opened first was the only one, and it is right for the handful
/// of books somebody is in the middle of. It is no way to find one book
/// among several hundred never opened, which stood in whatever order their
/// file names sorted. The choice is kept, because a shelf that forgets how
/// its owner arranged it is rearranged every morning.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Order {
    #[default]
    Recent,
    Title,
    Author,
}

impl Order {
    const ALL: [Self; 3] = [Self::Recent, Self::Title, Self::Author];

    /// The action that chooses it, which is also what is kept in the store.
    const fn name(self) -> &'static str {
        match self {
            Self::Recent => "order-recent",
            Self::Title => "order-title",
            Self::Author => "order-author",
        }
    }

    /// Short enough for the three to share one row, each a third of it:
    /// all three orders are on the shelf at once and the one in force is
    /// the filled one, so nothing has to be opened to see how it stands.
    const fn label(self) -> &'static str {
        match self {
            Self::Recent => "Recent",
            Self::Title => "Title",
            Self::Author => "Author",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum View {
    Home,
    Shelf(Shelf),
    Opening(usize),
    Reading,
    Unreadable(usize),
    /// Asking before an article is deleted, which cannot be undone.
    ConfirmDelete,
}

struct Books {
    view: View,
    /// Where Back from a book, or from a note about one, returns to.
    came_from: View,
    page: usize,
    documents: Vec<Document>,
    book: BookView,
    open_title: Option<String>,
    /// Where every book was left, and at what size.
    ///
    /// Every open used to start from a blank record and nothing was ever
    /// written back, so closing a book lost the page, the text size, the
    /// bookmarks and the highlights. The shared record is the one Feeds and
    /// the other readers keep, which also gives a book opened for the first
    /// time the text size last chosen here.
    progress: Progress,
    /// The record the open book is kept under.
    reading_id: Option<String>,
    /// The library identifier of the open book.
    opened: Option<String>,
    /// Whether the last attempt to list the library failed, which is not the
    /// same thing as a library with nothing in it.
    library_failed: bool,
    /// Whether the runtime stopped listing before it ran out of books.
    truncated: bool,
    /// What reading each book once told the shelf, by library identifier.
    described: BTreeMap<String, Described>,
    record: Record,
    /// How many store values that record was last read from or kept in, so
    /// a shorter record empties the ones it no longer needs.
    described_parts: usize,
    /// The books opened most recently, newest first, with how far into each.
    recent: Vec<Recent>,
    /// Real covers in memory, by library identifier.
    covers: BTreeMap<String, Thumb>,
    /// Lettered covers for books with none, so they are drawn once, with the
    /// name they were lettered with.
    lettered: BTreeMap<String, (String, Thumb)>,
    /// Books on the panel whose covers have not been read yet.
    wanted: VecDeque<String>,
    /// The book being read for its cover, if one is.
    describing: Option<String>,
    /// Kept covers asked of the store, by store key, with their books.
    loading: BTreeMap<String, String>,
    /// The books whose covers the panel showed last, so memory keeps those.
    on_panel: BTreeSet<String>,
    /// What each picture handle holds now, so an unchanged picture is not
    /// sent to the runtime again on every repaint.
    placed: BTreeMap<u32, (String, TilePicture)>,
    /// A document the runtime asked to have opened, by its library id, held
    /// until a listing that names it arrives. An article announced a moment
    /// ago is newer than any listing already held.
    asked_to_open: Option<String>,
    /// The Articles shelf: what is known about each article, and Sync.
    articles: articles::State,
    /// How the Library shelf is arranged and what it is narrowed to.
    library: Arranged,
    /// Whether this runtime hands a large book over in ranges.
    ranges: Ranges,
    /// The large book on its way in, if one is. One at a time, because each
    /// is tens of megabytes held until it is whole, and a book being opened
    /// takes the place of one being read for its cover.
    fetching: Option<Fetch>,
    /// Whether the book last tried was replaced on the card while it was
    /// arriving, so the note about it says that rather than calling it
    /// damaged.
    replaced: bool,
}

/// How the owner has the Library shelf: its order, and a search of it.
#[derive(Debug, Default)]
struct Arranged {
    order: Order,
    /// The field a search is typed into.
    search: TextEntry,
    /// What the shelf is narrowed to, or empty for all of it.
    query: String,
}

/// A large book being put together, and what for.
#[derive(Debug)]
struct Fetch {
    assembly: Assembly,
    /// To be opened, rather than read for its cover and name.
    opening: bool,
    /// The tenth of the book the panel last showed as arrived. The panel is
    /// redrawn when this moves and not for every piece: a piece lands in a
    /// few milliseconds and E Ink takes a few hundred to say so.
    shown: u32,
}

impl Default for Books {
    fn default() -> Self {
        Self::empty()
    }
}

impl Books {
    #[cfg(test)]
    fn seeded() -> Self {
        let mut books = Self::empty();
        books.take_documents(fixtures());
        books
    }

    fn empty() -> Self {
        Self {
            asked_to_open: None,
            view: View::Home,
            came_from: View::Home,
            page: 0,
            documents: Vec::new(),
            // Books passes on the answers a copy of the reader's notes waits
            // for, in `on_save` and `on_shelf`, so the notes may offer one.
            book: BookView::new().keeping_copies(),
            open_title: None,
            progress: Progress::default(),
            reading_id: None,
            opened: None,
            library_failed: false,
            truncated: false,
            described: BTreeMap::new(),
            record: Record::Unread,
            described_parts: 0,
            recent: Vec::new(),
            covers: BTreeMap::new(),
            lettered: BTreeMap::new(),
            wanted: VecDeque::new(),
            describing: None,
            loading: BTreeMap::new(),
            on_panel: BTreeSet::new(),
            placed: BTreeMap::new(),
            articles: articles::State::default(),
            library: Arranged::default(),
            ranges: Ranges::Unasked,
            fetching: None,
            replaced: false,
        }
    }

    /// The largest book that opens here under the runtime Books is on.
    const fn largest(&self) -> u32 {
        match self.ranges {
            Ranges::Refused => LARGEST_WHOLE,
            Ranges::Unasked | Ranges::Asked | Ranges::Answered => LARGEST_OPENABLE,
        }
    }

    /// Puts what can be read here first, and gives each book the name and
    /// author its own package gave it, where it has been read.
    fn take_documents(&mut self, mut documents: Vec<Document>) {
        documents.sort_by_key(|document| !document.belongs_here());
        for document in &mut documents {
            self.name(document);
        }
        self.documents = documents;
    }

    /// The name and author a book's package gave it, when Books has read this
    /// version of it.
    fn name(&self, document: &mut Document) {
        let Some(known) = self.described.get(&document.id) else {
            return;
        };
        if !known.is_of(document.bytes, &document.listed) {
            return;
        }
        if let Some(title) = &known.title {
            document.title = drawable(title);
        }
        document.author = known.author.as_deref().map(drawable);
    }

    fn show(&mut self, context: &mut Context) {
        self.on_panel.clear();
        let screen = match self.view {
            View::Home => self.home(context),
            View::Shelf(Shelf::Library) if self.library.search.is_open() => {
                ScreenBuilder::new("books-search")
                    .top_bar(Shelf::Library.title())
                    .text_entry_with_hint(
                        &self.library.search,
                        "Search the library",
                        "Part of a title or an author's name",
                        "Search",
                    )
                    .build()
            }
            View::Shelf(pile) => self.shelf(context, pile),
            View::Opening(index) if self.opening_in_ranges() => self.arriving(context, index),
            View::Opening(_) => match self.came_from {
                View::Shelf(pile) => self.shelf(context, pile),
                _ => self.home(context),
            },
            // The runtime keeps Back for itself on a screen that does not
            // claim it, and leaves the application. The page claimed it only
            // while a followed link had a way back, so Back from any other
            // page left Books with the book still open, and Books opened that
            // book again the next time it came forward: the shelves could not
            // be reached at all. Back from a page closes the book.
            View::Reading => self.reading().with_own_back(true),
            View::Unreadable(index) => self.unreadable(index),
            View::ConfirmDelete => self.confirm_delete(),
        };
        context.set_screen(screen);
        self.read_next(context);
    }

    /// Whether the book being opened is arriving a range at a time.
    fn opening_in_ranges(&self) -> bool {
        self.fetching.as_ref().is_some_and(|fetch| fetch.opening)
    }

    /// A large book on its way in: which book, how much of it has arrived,
    /// and a way to give up.
    ///
    /// A book that arrives whole is on the panel before a screen saying so
    /// could be drawn, so it keeps the shelf under it. One that takes a
    /// hundred round trips would leave the owner looking at a shelf that had
    /// ignored their tap.
    fn arriving(&mut self, context: &mut Context, index: usize) -> Screen {
        type Build = Box<dyn FnOnce(ScreenBuilder) -> ScreenBuilder>;
        let picture = self.cover(context, HERO_HANDLE, index);
        let (received, total) = self
            .fetching
            .as_ref()
            .map_or((0, None), |fetch| fetch.assembly.progress());
        let document = self.documents.get(index);
        // Until the first range says how long the file is, the listing's
        // word for it is the best there is.
        let total = total.or_else(|| document.map(|document| document.bytes));
        let title = document.map_or_else(String::new, |document| document.title.clone());
        let author = document.and_then(|document| document.author.clone());
        let about: Build = Box::new(move |slot: ScreenBuilder| {
            let slot = slot.heading_at_level(2, title);
            match author {
                Some(author) => slot.secondary(author),
                None => slot,
            }
        });
        let screen = ScreenBuilder::new("books-opening")
            .top_bar("Books")
            .owns_back(true);
        let screen = match picture {
            Some(picture) => {
                let cover: Build = Box::new(move |slot: ScreenBuilder| {
                    slot.picture(picture, HERO_MM.saturating_mul(2))
                });
                screen.band(
                    BandAlign::Top,
                    [
                        (SlotWidth::Fixed(HERO_MM.saturating_mul(10)), cover),
                        (SlotWidth::Fill, about),
                    ],
                )
            }
            None => about(screen),
        };
        screen
            .spacer(Space::Medium)
            .transfer("Opening", u64::from(received), total.map(u64::from))
            .cancellable(CANCEL_OPEN, "Cancel")
            .build()
    }

    /// The splash for a library that could not be listed or is empty.
    fn nothing_to_show(&self, screen: ScreenBuilder) -> Option<Screen> {
        if self.library_failed && self.documents.is_empty() {
            return Some(
                screen
                    .splash(
                        Some(Glyph::Book),
                        "The library could not be read",
                        "Books could not list the files on this reader. Try again.",
                    )
                    .button(RELIST, "Try again")
                    .build(),
            );
        }
        // A paired reader keeps its Articles shelf, and Sync on it, even
        // before anything else is on the card.
        if self.documents.is_empty() && self.articles.computer.is_none() {
            return Some(
                screen
                    .splash(
                        Some(Glyph::Book),
                        "No books yet",
                        "Plug the reader into a computer and copy EPUB, Markdown, HTML or text files onto it. They appear here the next time you open Books.",
                    )
                    .build(),
            );
        }
        None
    }

    /// The book being read, large, and the library as piles under it.
    fn home(&mut self, context: &mut Context) -> Screen {
        let screen = ScreenBuilder::new("books").top_bar("Books");
        if let Some(splash) = self.nothing_to_show(screen.clone()) {
            return splash;
        }
        let mut screen = self.notices(screen);
        let current = self.current();
        if let Some(index) = current {
            let picture = self.cover(context, HERO_HANDLE, index);
            screen = self.continuing(screen, index, picture);
        }
        let piles: Vec<_> = Shelf::ALL
            .into_iter()
            .enumerate()
            .filter_map(|(place, shelf)| {
                let members = self.members(shelf);
                let kept = shelf == Shelf::Articles
                    && (self.articles.computer.is_some()
                        || !self.members(Shelf::Archive).is_empty());
                if members.is_empty() && !kept {
                    return None;
                }
                let handle = PILE_HANDLE + u32::try_from(place).unwrap_or(0);
                let picture = if members.is_empty() {
                    None
                } else {
                    self.pile(context, handle, &members)
                };
                let count = if shelf == Shelf::Articles {
                    articles_word(&members, |index| self.is_unread(index))
                } else {
                    books_word(members.len())
                };
                Some((shelf, count, picture))
            })
            .collect();
        screen = screen.section("Your books").tile_grid(
            TileShape::Square,
            piles.into_iter().map(|(shelf, count, picture)| {
                (
                    shelf.name(),
                    shelf.title(),
                    if shelf == Shelf::Articles {
                        Glyph::News
                    } else {
                        Glyph::Book
                    },
                    move |tile: Tile| {
                        let tile = tile.with_subtitle(count);
                        match picture {
                            Some(picture) => tile.with_picture(picture),
                            None => tile,
                        }
                    },
                )
            }),
        );
        if current.is_none() {
            // Nothing has been opened yet, so there is no book to continue.
            // The first row of the library is the next best thing to offer.
            let columns = context.metrics().grid_columns(TileShape::Portrait);
            let first: Vec<_> = self
                .members(Shelf::Library)
                .into_iter()
                .take(columns)
                .collect();
            if !first.is_empty() {
                let tiles = self.tiles(context, &first);
                screen = screen
                    .section("Start reading")
                    .tile_grid(TileShape::Portrait, tiles);
            }
        }
        screen.build()
    }

    /// The book being read: its cover, and beside it how far in and the way
    /// back into it.
    ///
    /// Built from a band rather than the SDK's hero, because the hero's
    /// column holds a heading and facts and nothing else. A label reading
    /// "Read" against a value reading "Just started" was the closest it came
    /// to saying where the owner was, and the button went under the cover
    /// with the column beside it empty.
    fn continuing(
        &self,
        screen: ScreenBuilder,
        index: usize,
        picture: Option<TilePicture>,
    ) -> ScreenBuilder {
        type Build = Box<dyn FnOnce(ScreenBuilder) -> ScreenBuilder>;
        let document = &self.documents[index];
        let title = document.title.clone();
        let author = document.author.clone();
        let place = self
            .recent
            .iter()
            .find(|recent| recent.id == document.id)
            .cloned();
        let about: Build = Box::new(move |slot: ScreenBuilder| {
            // A level down from the page's own heading: the title of one book
            // beside its cover, not a headline over the whole page. At the
            // top level, a three-word title already took two lines.
            let slot = slot.heading_at_level(2, title);
            let slot = match author {
                Some(author) => slot.secondary(author),
                None => slot,
            };
            let slot = match place {
                Some(place) if place.page >= place.pages && place.pages > 0 => {
                    slot.secondary("Finished").progress(100)
                }
                Some(place) => slot
                    .secondary(format!("Page {} of {}", place.page, place.pages))
                    .progress(place.percent()),
                None => slot,
            };
            slot.spacer(Space::Medium)
                .button(CONTINUE, "Continue reading")
        });
        let Some(picture) = picture else {
            return about(screen);
        };
        let cover: Build =
            Box::new(move |slot: ScreenBuilder| slot.picture(picture, HERO_MM.saturating_mul(2)));
        screen.band(
            BandAlign::Top,
            [
                (SlotWidth::Fixed(HERO_MM.saturating_mul(10)), cover),
                (SlotWidth::Fill, about),
            ],
        )
    }

    /// One pile's books, as a grid of covers a page at a time.
    fn shelf(&mut self, context: &mut Context, pile: Shelf) -> Screen {
        if pile.lists_articles() {
            return self.article_shelf(context, pile);
        }
        let members = self.standing_on(pile);
        if members.is_empty() {
            let searched = pile == Shelf::Library && !self.library.query.is_empty();
            let bare = ScreenBuilder::new("books-shelf")
                .top_bar(pile.title())
                .owns_back(true);
            return if searched {
                ScreenBuilder::new("books-shelf")
                    .top_bar(format!("Search: {}", self.library.query))
                    .owns_back(true)
                    .top_bar_action(CLEAR_SEARCH, "Clear")
                    .splash(
                        Some(Glyph::Book),
                        "Nothing found",
                        format!(
                            "No title or author in the library has {} in it.",
                            self.library.query
                        ),
                    )
                    .build()
            } else {
                bare.splash(
                    Some(Glyph::Book),
                    "Nothing on this shelf",
                    "Books you add or open appear here.",
                )
                .build()
            };
        }
        let mut screen = self.shelf_prefix(pile);
        let pages = self.pages(context, pile, members.len());
        self.page = self.page.min(pages.len().saturating_sub(1));
        let page: Vec<usize> = pages
            .get(self.page)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|slot| members.get(slot).copied())
            .collect();
        let tiles = self.tiles(context, &page);
        screen = screen.tile_grid(TileShape::Portrait, tiles);
        let page_count = pages.len().max(1);
        if page_count > 1 {
            let page = u16::try_from(self.page + 1).unwrap_or(u16::MAX);
            let total = u16::try_from(page_count).unwrap_or(u16::MAX);
            screen = screen.page_turns(PREVIOUS, NEXT).page_position(page, total);
        }
        screen.build()
    }

    /// The bar of a shelf of covers and everything that stands above the
    /// covers, which is what a page of them is measured under.
    ///
    /// The Library shelf carries Search in its bar and the order it stands
    /// in under it. While it is narrowed by a search the bar says what was
    /// searched for and offers Clear in Search's place, because a shelf of
    /// four books with nothing to say why reads as a library that lost the
    /// rest. The bar says it rather than a line under it: a line there took
    /// a whole row of covers off every page of results.
    fn shelf_prefix(&self, pile: Shelf) -> ScreenBuilder {
        if pile != Shelf::Library {
            return self.notices(
                ScreenBuilder::new("books-shelf")
                    .top_bar(pile.title())
                    .owns_back(true),
            );
        }
        let screen = ScreenBuilder::new("books-shelf").owns_back(true);
        let screen = if self.library.query.is_empty() {
            self.notices(
                screen
                    .top_bar(pile.title())
                    .top_bar_action(SEARCH, "Search"),
            )
        } else {
            self.notices(
                screen
                    .top_bar(format!("Search: {}", self.library.query))
                    .top_bar_action(CLEAR_SEARCH, "Clear"),
            )
        };
        let chosen = Order::ALL
            .iter()
            .position(|order| *order == self.library.order)
            .unwrap_or_default();
        screen.segmented(
            chosen,
            Order::ALL.map(|order| (order.name(), order.label())),
        )
    }

    /// The documents a shelf is showing: all of it, or on the Library shelf
    /// the part a search narrowed it to.
    ///
    /// Kept apart from [`Self::members`], which the home screen counts and
    /// piles from: a search is of the shelf it was typed on and must not
    /// change how many books home says the library holds.
    fn standing_on(&self, pile: Shelf) -> Vec<usize> {
        let mut members = self.members(pile);
        if pile != Shelf::Library || self.library.query.is_empty() {
            return members;
        }
        let sought = folded(&self.library.query);
        let words: Vec<&str> = sought.split_whitespace().collect();
        members.retain(|&index| {
            let document = &self.documents[index];
            let about = folded(&format!(
                "{} {}",
                document.title,
                document.author.as_deref().unwrap_or_default()
            ));
            words.iter().all(|word| about.contains(word))
        });
        members
    }

    /// Book tiles for these documents: the cover, the author, how far in.
    fn tiles(
        &mut self,
        context: &mut Context,
        indices: &[usize],
    ) -> Vec<(String, String, Glyph, impl FnOnce(Tile) -> Tile)> {
        indices
            .iter()
            .enumerate()
            .filter_map(|(slot, &index)| {
                let handle = TILE_HANDLE + u32::try_from(slot).unwrap_or(0);
                let picture = self.cover(context, handle, index);
                let document = self.documents.get(index)?;
                let subtitle = if !document.on_card {
                    "Not downloaded".to_owned()
                } else if document.belongs_here() && !document.opens_here(self.largest()) {
                    "Too large to open here".to_owned()
                } else {
                    document.author.clone().unwrap_or_default()
                };
                let badge = self
                    .percent_read(&document.id)
                    .filter(|_| document.opens_here(self.largest()))
                    .map(|read| format!("{read}%"))
                    .unwrap_or_default();
                Some((
                    format!("book-{index}"),
                    document.title.clone(),
                    Glyph::Book,
                    move |tile: Tile| {
                        let tile = tile.with_subtitle(subtitle).with_badge(badge);
                        match picture {
                            Some(picture) => tile.with_picture(picture),
                            None => tile,
                        }
                    },
                ))
            })
            .collect()
    }

    /// The banners every list screen carries: a place not kept, and a list
    /// that stopped short.
    fn notices(&self, screen: ScreenBuilder) -> ScreenBuilder {
        let screen = self.place_notice(screen);
        if !self.truncated {
            return screen;
        }
        // The runtime stops for one of two reasons and says only that it
        // stopped. A listing as long as a listing can be stopped at the
        // count; a shorter one stopped at a folder it would not go into.
        let most = kobo_sdk::permissions::library::MAX_ENTRIES;
        if self.documents.len() >= most {
            screen.banner(
                BannerLevel::Info,
                format!(
                    "This reader holds more than the {} books Books lists. Some are not shown.",
                    grouped(most)
                ),
            )
        } else {
            screen.banner(
                BannerLevel::Info,
                "Books did not reach every folder on this reader. Some books are not shown.",
            )
        }
    }

    /// The documents on a shelf, in the order the shelf shows them.
    fn members(&self, pile: Shelf) -> Vec<usize> {
        let opened: Vec<usize> = self
            .recent
            .iter()
            .filter_map(|recent| {
                self.documents
                    .iter()
                    .position(|document| document.id == recent.id)
            })
            .collect();
        match pile {
            Shelf::Recent => opened,
            Shelf::Library => {
                let shelved = |index: usize| {
                    let document = &self.documents[index];
                    document.belongs_here() && !articles::is_article(&document.id)
                };
                // Marked off as they are placed rather than looked for among
                // the ones already placed: with two thousand books on the
                // card that search was two million comparisons, each time
                // the shelf was drawn or a page was turned.
                let mut placed = vec![false; self.documents.len()];
                let mut members = Vec::new();
                for index in opened.into_iter().chain(0..self.documents.len()) {
                    if shelved(index) && !std::mem::replace(&mut placed[index], true) {
                        members.push(index);
                    }
                }
                let title = |index: usize| folded(&self.documents[index].title);
                match self.library.order {
                    Order::Recent => {}
                    Order::Title => members.sort_by_cached_key(|&index| title(index)),
                    // A book that names no author, or has not been read for
                    // one yet, stands after those that do rather than
                    // before every A.
                    Order::Author => members.sort_by_cached_key(|&index| {
                        let author = self.documents[index].author.as_deref();
                        (
                            author.is_none(),
                            author.map(folded).unwrap_or_default(),
                            title(index),
                        )
                    }),
                }
                members
            }
            Shelf::Articles | Shelf::Archive => {
                let archived = pile == Shelf::Archive;
                let mut members: Vec<usize> = (0..self.documents.len())
                    .filter(|&index| {
                        let document = &self.documents[index];
                        articles::is_article(&document.id)
                            && self.kept(&document.id).archived == archived
                    })
                    .collect();
                members.sort_by_cached_key(|&index| {
                    let document = &self.documents[index];
                    articles::order(&self.kept(&document.id), &document.title)
                });
                members
            }
            Shelf::KoboReader => (0..self.documents.len())
                .filter(|&index| !self.documents[index].belongs_here())
                .collect(),
        }
    }

    /// What Books knows about one article, or nothing yet.
    fn kept(&self, id: &str) -> Kept {
        self.articles.kept.get(id).cloned().unwrap_or_default()
    }

    fn is_unread(&self, index: usize) -> bool {
        self.documents
            .get(index)
            .is_some_and(|document| !self.kept(&document.id).read)
    }

    /// The Articles shelf, or the archive under it, as a list: a title, the
    /// site and when it arrived, and a menu on each row.
    ///
    /// A list rather than covers, because an article's cover is usually
    /// lettered from its title, and the title, the site and the day are what
    /// tell one from the next.
    fn article_shelf(&mut self, context: &mut Context, pile: Shelf) -> Screen {
        let members = self.members(pile);
        let screen = self.article_prefix(pile);
        if members.is_empty() {
            return self.no_articles(screen, pile);
        }
        let now = kobo_sdk::clock::SystemClock::reader().now().ok();
        let rows: Vec<(String, String)> = members
            .iter()
            .map(|&index| {
                let document = &self.documents[index];
                let kept = self.kept(&document.id);
                let when = kept
                    .saved
                    .zip(now)
                    .and_then(|(saved, now)| kobo_sdk::when::spoken(saved, now));
                let about = match (kept.site.as_deref(), when) {
                    (Some(site), Some(when)) => format!("{} · {when}", drawable(site)),
                    (Some(site), None) => drawable(site),
                    (None, Some(when)) => when,
                    (None, None) => String::new(),
                };
                (
                    context.clamped_row_with_menu(&document.title, 2, false),
                    context.one_line_row_with_menu(&about, false),
                )
            })
            .collect();
        let borrowed: Vec<(&str, &str)> = rows
            .iter()
            .map(|(title, about)| (title.as_str(), about.as_str()))
            .collect();
        let pages = context.paginate_rows_with_menu_under(
            &borrowed,
            false,
            Position::AtTheFoot,
            &self.article_prefix(pile).build(),
        );
        let pages = if pages.is_empty() {
            vec![Vec::new()]
        } else {
            pages
        };
        self.page = self.page.min(pages.len() - 1);
        let shown = pages[self.page].clone();
        // Rows are named by their place on the shelf, which is what a test
        // or a route can see; what they act on is settled by id.
        let mut screen = screen.rows_with_menu(shown.iter().map(|&row| {
            let index = members[row];
            (
                format!("article-{row}"),
                rows[row].0.clone(),
                rows[row].1.clone(),
                if self.is_unread(index) {
                    Glyph::News
                } else {
                    Glyph::Check
                },
                format!("article-menu-{row}"),
            )
        }));
        // Only while its row is on the panel: a menu hung off a row on
        // another page would point at nothing.
        let open = shown.iter().copied().find(|&row| {
            self.articles.menu_open.as_deref() == Some(self.documents[members[row]].id.as_str())
        });
        if let Some(row) = open {
            let open = members[row];
            let mut items = vec![if self.is_unread(open) {
                (MARK_READ, "Mark read", Glyph::Check)
            } else {
                (MARK_UNREAD, "Mark unread", Glyph::News)
            }];
            items.push(if pile == Shelf::Archive {
                (RESTORE, "Move back to Articles", Glyph::Folder)
            } else {
                (ARCHIVE, "Archive", Glyph::Folder)
            });
            items.push((DELETE, "Delete", Glyph::Trash));
            screen = screen.row_overflow(format!("article-menu-{row}"), true, items);
        }
        if pages.len() > 1 {
            screen = screen.page_turns(PREVIOUS, NEXT).page_position(
                u16::try_from(self.page + 1).unwrap_or(u16::MAX),
                u16::try_from(pages.len()).unwrap_or(u16::MAX),
            );
        }
        screen.build()
    }

    /// An Articles shelf, or its archive, with nothing on it.
    fn no_articles(&self, screen: ScreenBuilder, pile: Shelf) -> Screen {
        if pile == Shelf::Archive {
            return screen
                .splash(
                    Some(Glyph::News),
                    "No archived articles",
                    "Articles you archive are kept here, out of the way.",
                )
                .build();
        }
        // One thing to type at a time: with a step already on the panel for
        // a sync that failed, how a first article is sent can wait until
        // that one is taken. The shelf still says it is empty, as a quiet
        // line: a heading with nothing under it read as a page that had
        // stopped half way.
        if self.says_what_to_run(pile) {
            return screen.secondary("No articles yet.").build();
        }
        let screen = screen.heading("No articles yet");
        // The shelf used to say that pages sent from a computer arrive here,
        // and not how one is sent, which left an owner who had just paired
        // looking at an empty shelf with nothing to do.
        screen
            .text("To send one, right-click a page in your computer's browser and choose Send to Kobo.")
            .text("If the browser has no Send to Kobo, run this once on the computer:")
            .command("kobo clip install --all")
            .build()
    }

    /// The bar and the line under it, which the list is measured beneath.
    fn article_prefix(&self, pile: Shelf) -> ScreenBuilder {
        let mut screen = ScreenBuilder::new("books-articles")
            .top_bar(pile.title())
            .owns_back(true);
        if pile == Shelf::Articles {
            screen = screen.top_bar_action(SYNC, "Sync");
            if !self.members(Shelf::Archive).is_empty() {
                screen = screen.top_bar_action(ARCHIVED, "Archived");
            }
        }
        if self.articles.delete_failed {
            screen = screen.banner(
                BannerLevel::Error,
                "That article could not be deleted. Try again.",
            );
        } else if let Some(said) = self
            .articles
            .said
            .as_ref()
            .filter(|_| pile == Shelf::Articles)
        {
            let line = said.line();
            screen = screen.banner(
                if line.trouble {
                    BannerLevel::Error
                } else {
                    BannerLevel::Info
                },
                line.words,
            );
            if let Some((lead, command)) = line.next {
                screen = screen.text(lead).command(command);
            }
        }
        screen
    }

    /// Whether the shelf is showing a step to take on the computer, which
    /// an empty shelf makes room for by saying less itself.
    fn says_what_to_run(&self, pile: Shelf) -> bool {
        pile == Shelf::Articles
            && !self.articles.delete_failed
            && self
                .articles
                .said
                .as_ref()
                .is_some_and(|said| said.line().next.is_some())
    }

    fn confirm_delete(&self) -> Screen {
        let title = self
            .articles
            .confirming
            .as_deref()
            .and_then(|id| self.documents.iter().find(|document| document.id == id))
            .map_or("This article", |document| document.title.as_str());
        ScreenBuilder::new("books-delete")
            .top_bar("Articles")
            .owns_back(true)
            .confirm(
                "Delete this article?",
                format!("{title} is removed from this reader."),
                (CONFIRM_DELETE, "Delete article"),
                (KEEP_ARTICLE, "Keep article"),
            )
            .build()
    }

    /// Asks the paired computer for articles waiting there.
    ///
    /// `quiet` is the check Books makes as it opens: nothing is said unless
    /// articles arrive, because a computer that is off is the usual case and
    /// not news.
    fn sync(&mut self, context: &mut Context, quiet: bool) {
        if self.articles.syncing.is_some() {
            if !quiet {
                // The owner asked while a quiet check was out; its answer
                // is now theirs.
                self.articles.syncing = self.articles.syncing.map(|(task, _)| (task, false));
                self.articles.said = Some(Said::Checking);
            }
            return;
        }
        let Some((paired, code)) = self.articles.computer.clone() else {
            if !quiet {
                self.articles.said = Some(Said::NotPaired);
            }
            return;
        };
        let address = match self.articles.hosts.current() {
            "" => paired,
            host => host.to_owned(),
        };
        let body = kobo_json::ObjectBuilder::new()
            .set("token", code.as_str())
            .build()
            .to_json();
        let task = context.spawn(Task::Post {
            url: format!("https://{address}/articles/sync"),
            body,
            content_type: "application/json".to_owned(),
            credential: None,
            headers: Vec::new(),
            max_bytes: 16 * 1024,
        });
        let Some(task) = task else {
            if !quiet {
                self.articles.said = Some(Said::NotAnswering);
            }
            return;
        };
        self.articles.syncing = Some((task, quiet));
        self.articles.checked_at = now_millis();
        if !quiet {
            self.articles.said = Some(Said::Checking);
        }
    }

    /// The computer's answer. Articles it sent are already on the card, so
    /// the card is listed again for them to appear.
    fn synced(&mut self, context: &mut Context, quiet: bool, outcome: TaskOutcome) {
        let said = match outcome {
            TaskOutcome::Completed(bytes) => {
                articles::reply(&bytes).map_or(Said::Outdated, |reply| articles::outcome(&reply))
            }
            TaskOutcome::Failed(error) => articles::failure(error),
            TaskOutcome::Cancelled => return,
        };
        if matches!(said, Said::Arrived(_)) {
            context.device().list_library();
        }
        // The runtime's own notice says that articles arrived, so a quiet
        // check adds nothing to it.
        if !quiet {
            self.articles.said = Some(said);
        }
        if matches!(self.view, View::Home | View::Shelf(_)) {
            self.show(context);
        }
    }

    /// Changes one article's marks and keeps them.
    fn mark(&mut self, context: &mut Context, index: usize, change: impl FnOnce(&mut Kept)) {
        let Some(document) = self.documents.get(index) else {
            return;
        };
        if !articles::is_article(&document.id) {
            return;
        }
        let bytes = document.bytes;
        let kept = self
            .articles
            .kept
            .entry(document.id.clone())
            .or_insert_with(|| Kept {
                bytes,
                ..Kept::default()
            });
        change(kept);
        self.save_articles(context);
    }

    fn save_articles(&mut self, context: &mut Context) {
        // A record too large to keep stays marked as changed, so the next
        // save tries again, rather than being dropped.
        let Some(parts) = articles::encode(&self.articles.kept, MAX_STORE_VALUE) else {
            self.articles.changed = true;
            return;
        };
        let count = parts.len();
        for (index, part) in parts.into_iter().enumerate() {
            context.store().save(articles::part_key(index), part);
        }
        // Parts a larger record used before are emptied, which is also where
        // reading stops.
        for index in count..self.articles.parts_stored {
            context.store().save(articles::part_key(index), Vec::new());
        }
        self.articles.parts_stored = count;
        self.articles.changed = false;
    }

    /// Takes a listing's articles into the record: new ones are added to be
    /// read for their site and date, and ones gone from the card are let go,
    /// unless the listing stopped short and cannot say what is gone.
    fn take_articles(&mut self, truncated: bool) {
        if !self.articles.loaded {
            return;
        }
        let listed: BTreeMap<&str, u32> = self
            .documents
            .iter()
            .filter(|document| articles::is_article(&document.id))
            .map(|document| (document.id.as_str(), document.bytes))
            .collect();
        let before = self.articles.kept.len();
        if !truncated {
            self.articles
                .kept
                .retain(|id, _| listed.contains_key(id.as_str()));
        }
        let mut changed = self.articles.kept.len() != before;
        for (id, bytes) in listed {
            let kept = self.articles.kept.entry(id.to_owned()).or_default();
            if kept.bytes != bytes {
                // Replaced since it was read, or new: read it again.
                kept.bytes = bytes;
                kept.described = false;
                changed = true;
            }
        }
        if changed {
            self.articles.changed = true;
        }
    }

    /// Queues every article not yet read for its site and date, wherever it
    /// is, because the shelf is sorted by them.
    fn want_articles(&mut self) {
        if !self.articles.loaded || self.record == Record::Unread {
            return;
        }
        let wanted: Vec<Document> = self
            .documents
            .iter()
            .filter(|document| {
                articles::is_article(&document.id) && !self.kept(&document.id).described
            })
            .cloned()
            .collect();
        for document in &wanted {
            self.queue(document);
        }
    }

    /// The book to offer to continue: the last one opened that still opens.
    fn current(&self) -> Option<usize> {
        self.recent.iter().find_map(|recent| {
            self.documents.iter().position(|document| {
                document.id == recent.id && document.opens_here(self.largest())
            })
        })
    }

    fn percent_read(&self, id: &str) -> Option<u8> {
        self.recent
            .iter()
            .find(|recent| recent.id == id)
            .map(Recent::percent)
    }

    fn reading(&self) -> Screen {
        let title = self.open_title.as_deref().unwrap_or("Reading");
        self.book.screen(title).unwrap_or_else(Self::missing_reader)
    }

    fn missing_reader() -> Screen {
        ScreenBuilder::new("books-unreadable")
            .top_bar("Books")
            .splash(
                Some(Glyph::Book),
                "This document",
                "This file could not be opened. It may be damaged, or written in a way Books cannot read.",
            )
            .button(BACK, "Back")
            .build()
    }

    /// Says where a book that does not open here can be read instead.
    ///
    /// "Not readable yet" left the owner holding a book with nowhere to take
    /// it. Each of these does open in the stock reader, so the screen says
    /// that, and how to get there.
    fn unreadable(&self, index: usize) -> Screen {
        let document = self.documents.get(index);
        let title = document.map_or("This document", |document| document.title.as_str());
        let reason = if document.is_some_and(|document| !document.on_card) {
            format!("This book is in your Kobo library but is not downloaded yet. Open it in the Kobo reader to download it. {IN_KOBO_READER}")
        } else if document.is_some_and(|document| document.kind == Kind::Kepub) {
            format!("Books from the Kobo Store open in the Kobo reader. {IN_KOBO_READER}")
        } else if document.is_some_and(|document| document.kind == Kind::Pdf) {
            format!(
                "PDFs open in the Kobo reader. Books cannot show PDF pages yet. {IN_KOBO_READER}"
            )
        } else if let Some(document) = document.filter(|document| document.bytes > self.largest()) {
            format!(
                "Books can open files up to {} MB, and this one is {}. Open it in the app it came from, or in the Kobo reader if you copied it on yourself.",
                self.largest() / (1024 * 1024),
                megabytes(document.bytes)
            )
        } else if self.replaced {
            "This file changed on the card while Books was opening it. Open it again.".to_owned()
        } else {
            "This file could not be opened. It may be damaged, or written in a way Books cannot read."
                .to_owned()
        };
        // The bar's own back arrow returns to the shelf too. Without owning
        // it, that arrow left Books altogether from a screen that is only a
        // note about one book.
        let screen = ScreenBuilder::new("books-unreadable")
            .top_bar("Books")
            .owns_back(true)
            .splash(Some(Glyph::Book), title, reason);
        if self.replaced {
            screen.button(OPEN_AGAIN, "Open again").build()
        } else {
            screen.build()
        }
    }

    /// Says on the shelf that the last place was not kept, with the fix on
    /// the same banner.
    ///
    /// On the shelf no book is open, so nothing would try the write again
    /// until a book was opened and paged; Retry saving does it now. A full
    /// record of places is not mended by trying again, so that one is said
    /// without the control.
    fn place_notice(&self, screen: ScreenBuilder) -> ScreenBuilder {
        if !self.progress.failed {
            return screen;
        }
        if self.progress.out_of_room() {
            return screen.banner(
                BannerLevel::Error,
                "Books has no room left to save your place in another book.",
            );
        }
        screen
            .banner(
                BannerLevel::Error,
                "Books could not save your place in the last book.",
            )
            .banner_action(RETRY_SAVE, "Retry saving")
    }

    fn pages(&self, context: &Context, pile: Shelf, count: usize) -> Vec<Vec<usize>> {
        // Measured under the banners, so a notice with its action line takes
        // room from the grid's page rather than squeezing every tile.
        let placed = self.shelf_prefix(pile).build();
        let pages = context.paginate_tiles_under(count, TileShape::Portrait, false, &placed);
        if pages.is_empty() {
            vec![Vec::new()]
        } else {
            pages
        }
    }

    /// A book's cover at `handle`: its own, once read, or a stand-in.
    ///
    /// Asking for it is also what queues the book to be read, so the covers
    /// that are read first are the ones on the panel.
    fn cover(&mut self, context: &mut Context, handle: u32, index: usize) -> Option<TilePicture> {
        let document = self.documents.get(index)?.clone();
        self.on_panel.insert(document.id.clone());
        self.want(context, &document);
        let (width, height) = tile_pixels(context);
        if let Some(thumb) = self.covers.get(&document.id) {
            let key = format!("cover:{}", document.id);
            let thumb = thumb.clone();
            return self.place(context, handle, key, || thumb);
        }
        if document.kind.is_packaged() || !document.on_card {
            let thumb = self.lettered(&document, width, height);
            let key = format!(
                "lettered:{}:{}:{}",
                document.id,
                document.title,
                document.author.as_deref().unwrap_or_default()
            );
            return self.place(context, handle, key, || thumb);
        }
        let key = format!("preview:{}", document.id);
        // The first line is left blank because the tile's percent read sits
        // in that corner, and drawn over the opening words it hid the start
        // of the title. Every preview tile leaves it, read or not, so their
        // first lines still line up across a shelf.
        let preview = format!("\n{}", document.preview);
        self.place(context, handle, key, || Thumb {
            width,
            height,
            grey: document_preview(&preview, document.kind.badge(), width, height),
        })
    }

    /// The covers of a shelf's first books, piled into one picture.
    fn pile(
        &mut self,
        context: &mut Context,
        handle: u32,
        members: &[usize],
    ) -> Option<TilePicture> {
        let (width, height) = context.metrics().tile_body(TileShape::Square);
        let (width, height) = (u32::try_from(width).ok()?, u32::try_from(height).ok()?);
        let (cover_width, cover_height) = tile_pixels(context);
        let mut thumbs = Vec::new();
        let mut key = format!("pile:{width}x{height}");
        for (place, &index) in members.iter().take(3).enumerate() {
            let document = self.documents[index].clone();
            self.on_panel.insert(document.id.clone());
            self.want(context, &document);
            let thumb = match self.covers.get(&document.id) {
                Some(thumb) => {
                    key.push_str(&format!(":c{}", document.id));
                    Some(thumb.clone())
                }
                None if place > 0 => {
                    key.push_str(":b");
                    None
                }
                None if document.kind.is_packaged() || !document.on_card => {
                    key.push_str(&format!(":l{}:{}", document.id, document.title));
                    Some(self.lettered(&document, cover_width, cover_height))
                }
                None => {
                    key.push_str(&format!(":p{}", document.id));
                    Some(Thumb {
                        width: cover_width,
                        height: cover_height,
                        grey: document_preview(
                            &document.preview,
                            document.kind.badge(),
                            cover_width,
                            cover_height,
                        ),
                    })
                }
            };
            thumbs.push(thumb);
        }
        let borrowed: Vec<Option<&Thumb>> = thumbs.iter().map(Option::as_ref).collect();
        let grey = covers::stack(&borrowed, width, height);
        self.place(context, handle, key, || Thumb {
            width,
            height,
            grey,
        })
    }

    /// A lettered cover for a book that has none, drawn once.
    ///
    /// Drawn again when the book's name changes, which it does once the book
    /// has been read: the first lettering is of the file's name.
    fn lettered(&mut self, document: &Document, width: u32, height: u32) -> Thumb {
        let name = format!(
            "{}\n{}",
            document.title,
            document.author.as_deref().unwrap_or_default()
        );
        let stale = self
            .lettered
            .get(&document.id)
            .is_none_or(|(lettered, thumb)| {
                *lettered != name || thumb.width != width || thumb.height != height
            });
        if stale {
            if self.lettered.len() >= HELD_COVERS {
                self.lettered.clear();
            }
            let thumb = Thumb {
                width,
                height,
                grey: typographic_cover(&document.title, document.author.as_deref(), width, height),
            };
            self.lettered.insert(document.id.clone(), (name, thumb));
        }
        self.lettered[&document.id].1.clone()
    }

    /// Sends a picture to the runtime unless that handle already holds it.
    fn place(
        &mut self,
        context: &mut Context,
        handle: u32,
        key: String,
        picture: impl FnOnce() -> Thumb,
    ) -> Option<TilePicture> {
        if let Some((held, reference)) = self.placed.get(&handle) {
            if *held == key {
                return Some(*reference);
            }
        }
        let thumb = picture();
        let reference =
            context.put_picture(PictureHandle(handle), thumb.width, thumb.height, thumb.grey)?;
        self.placed.insert(handle, (key, reference));
        Some(reference)
    }

    /// Queues a book on the panel to have its cover read or fetched.
    fn want(&mut self, context: &mut Context, document: &Document) {
        if self.covers.contains_key(&document.id) || self.record == Record::Unread {
            return;
        }
        match self.described.get(&document.id) {
            Some(known) if known.is_of(document.bytes, &document.listed) => match &known.cover {
                Cover::Kept(key) => {
                    if !self.loading.contains_key(key) {
                        self.loading.insert(key.clone(), document.id.clone());
                        context.store().load(key.clone());
                    }
                }
                Cover::Missing => {}
                Cover::Unkept => self.queue(document),
            },
            _ => self.queue(document),
        }
    }

    fn queue(&mut self, document: &Document) {
        if document.describable(self.largest())
            && self.describing.as_deref() != Some(document.id.as_str())
            && !self.wanted.contains(&document.id)
        {
            self.wanted.push_back(document.id.clone());
        }
    }

    /// Asks for the next queued book, one at a time and never while a book
    /// is open: reading a cover is the slowest thing Books does, and a page
    /// turn waiting behind it would be the owner's to notice.
    fn read_next(&mut self, context: &mut Context) {
        if self.describing.is_some() || matches!(self.view, View::Reading | View::Opening(_)) {
            return;
        }
        let largest = self.largest();
        while let Some(id) = self.wanted.pop_front() {
            // Articles are read wherever they are, because the shelf is
            // sorted by the date inside each one.
            if !self.on_panel.contains(&id) && !articles::is_article(&id) {
                continue;
            }
            let Some(document) = self
                .documents
                .iter()
                .find(|document| document.id == id && document.describable(largest))
            else {
                continue;
            };
            if !document.in_ranges() {
                self.describing = Some(id.clone());
                context.device().read_library(id);
                return;
            }
            // A large book waits for the runtime to say it answers ranges.
            // Drawing the shelf again queues it again, and the answer draws
            // the shelf.
            if self.ranges == Ranges::Answered {
                self.describing = Some(id.clone());
                self.fetching = Some(Fetch {
                    assembly: Assembly::begin(context, id, LARGEST_OPENABLE),
                    opening: false,
                    shown: 0,
                });
                return;
            }
        }
        if self.record == Record::Changed {
            self.save_described(context);
        }
        if self.articles.changed {
            self.save_articles(context);
        }
    }

    /// Reads a book's package for its name, author and cover, and keeps them.
    fn describe(&mut self, context: &mut Context, id: &str, bytes: &[u8]) {
        let Some(index) = self.documents.iter().position(|document| document.id == id) else {
            return;
        };
        let size = self.documents[index].bytes;
        let listed = self.documents[index].listed.clone();
        let description = kobo_doc::epub::describe(bytes).unwrap_or_default();
        if articles::is_article(id) {
            let kept = self.articles.kept.entry(id.to_owned()).or_default();
            kept.bytes = size;
            kept.site = description.publisher.as_deref().map(drawable);
            kept.saved = description
                .modified
                .as_deref()
                .and_then(kobo_sdk::when::stamp_millis);
            kept.described = true;
            self.articles.changed = true;
        }
        let (width, height) = tile_pixels(context);
        let thumb = description
            .cover
            .as_deref()
            .and_then(|cover| covers::thumbnail(cover, width, height));
        let kept = self
            .described
            .values()
            .filter(|book| matches!(book.cover, Cover::Kept(_)))
            .count();
        let cover = match &thumb {
            None => Cover::Missing,
            Some(thumb) => match covers::to_png(thumb)
                .filter(|png| kept < KEPT_COVERS && png.len() <= MAX_STORE_VALUE)
            {
                Some(png) => {
                    let key = covers::key(id, size, &listed);
                    context.store().save(key.clone(), png);
                    Cover::Kept(key)
                }
                None => Cover::Unkept,
            },
        };
        if let Some(thumb) = thumb {
            self.hold(id, thumb);
        }
        self.described.insert(
            id.to_owned(),
            Described {
                bytes: size,
                listed,
                title: description.title,
                author: description.author,
                cover,
            },
        );
        self.record = Record::Changed;
        let mut document = self.documents[index].clone();
        self.name(&mut document);
        self.documents[index] = document;
    }

    /// Keeps a decoded cover in memory, letting go of ones off the panel.
    fn hold(&mut self, id: &str, thumb: Thumb) {
        if self.covers.len() >= HELD_COVERS {
            let away: Vec<String> = self
                .covers
                .keys()
                .filter(|held| !self.on_panel.contains(*held))
                .cloned()
                .collect();
            for held in away {
                self.covers.remove(&held);
            }
        }
        self.covers.insert(id.to_owned(), thumb);
    }

    fn save_described(&mut self, context: &mut Context) {
        // A record past all its parts is left as the store has it. What was
        // learned since is read out of the books again on the next start.
        if let Some(parts) = covers::encode_described(&self.described, MAX_STORE_VALUE) {
            let count = parts.len();
            for (index, part) in parts.into_iter().enumerate() {
                context.store().save(covers::described_key(index), part);
            }
            // Parts a longer record used before are emptied, which is also
            // where reading stops.
            for index in count..self.described_parts {
                context
                    .store()
                    .save(covers::described_key(index), Vec::new());
            }
            self.described_parts = count;
        }
        self.record = Record::Kept;
    }

    /// Moves the open book to the front of the recent ones, with how far in.
    ///
    /// Written when the book is left rather than when it opens, so the page
    /// recorded is the one the owner stopped on.
    fn note_reading(&mut self, context: &mut Context) {
        let Some(id) = self.opened.clone() else {
            return;
        };
        let (page, pages) = self.book.reader_mut().map_or((1, 1), |reader| {
            (
                u32::try_from(reader.page_number()).unwrap_or(u32::MAX),
                u32::try_from(reader.page_count()).unwrap_or(u32::MAX),
            )
        });
        if articles::is_article(&id) && pages > 0 && page >= pages {
            // Read to the end: finished, so it moves down among the read.
            if let Some(index) = self.documents.iter().position(|document| document.id == id) {
                if self.is_unread(index) {
                    self.mark(context, index, |kept| kept.read = true);
                }
            }
        }
        self.recent.retain(|recent| recent.id != id);
        self.recent.insert(0, Recent { id, page, pages });
        self.recent.truncate(RECENT_LIMIT);
        context
            .store()
            .save(RECENT, covers::encode_recent(&self.recent));
    }

    fn open_document(&mut self, context: &mut Context, index: usize) {
        let Some(document) = self.documents.get(index).cloned() else {
            return;
        };
        if matches!(self.view, View::Home | View::Shelf(_)) {
            self.came_from = self.view;
        }
        self.replaced = false;
        if !document.opens_here(self.largest()) {
            self.book.close(context);
            self.open_title = None;
            self.view = View::Unreadable(index);
            self.show(context);
            return;
        }
        if let Some(body) = document.body.as_ref() {
            let bytes = fixture_bytes(&document, body);
            self.finish_open(context, index, &document, &bytes);
            return;
        }
        self.view = View::Opening(index);
        // A large book being read for its cover gives way: two of them in
        // hand at once is twice the most Books means to hold, and the owner
        // is waiting on this one.
        self.stop_fetching();
        if document.in_ranges() {
            match self.ranges {
                Ranges::Answered => {
                    self.fetching = Some(Fetch {
                        assembly: Assembly::begin(context, document.id, LARGEST_OPENABLE),
                        opening: true,
                        shown: 0,
                    });
                }
                // The answer opens the book when it lands; see `on_identity`.
                Ranges::Unasked => self.ask_about_ranges(context),
                Ranges::Asked | Ranges::Refused => {}
            }
            self.show(context);
            return;
        }
        self.show(context);
        context.device().read_library(document.id);
    }

    /// Asks the runtime which release it is, once. See [`Ranges`].
    fn ask_about_ranges(&mut self, context: &mut Context) {
        if self.ranges == Ranges::Unasked {
            self.ranges = Ranges::Asked;
            context.device().read_identity();
        }
    }

    /// The runtime said which release it is, or would not.
    fn on_identity(&mut self, context: &mut Context, result: &DeviceResult) {
        if self.ranges != Ranges::Asked {
            return;
        }
        let shipped_together = matches!(
            result,
            DeviceResult::Identity(identity)
                if identity.runtime_version == env!("CARGO_PKG_VERSION")
        );
        self.ranges = if shipped_together {
            Ranges::Answered
        } else {
            Ranges::Refused
        };
        match self.view {
            // A large book tapped before the answer came is opened now, or
            // said to be too large for this runtime.
            View::Opening(index)
                if self.fetching.is_none()
                    && self.documents.get(index).is_some_and(Document::in_ranges) =>
            {
                self.view = self.came_from;
                self.open_document(context, index);
            }
            View::Home | View::Shelf(_) => self.show(context),
            _ => {}
        }
    }

    /// Lets go of a large book that was arriving. One being read for its
    /// cover goes back to the front of the queue, to be read when the shelf
    /// is next on the panel with nothing open.
    fn stop_fetching(&mut self) {
        let Some(fetch) = self.fetching.take() else {
            return;
        };
        if !fetch.opening && self.describing.as_deref() == Some(fetch.assembly.id()) {
            if let Some(id) = self.describing.take() {
                self.wanted.push_front(id);
            }
        }
    }

    /// One answer to a range Books asked for.
    fn on_range(&mut self, context: &mut Context, request: &DeviceRequest, result: &DeviceResult) {
        let Some(fetch) = self.fetching.as_mut() else {
            return;
        };
        match fetch.assembly.advance(context, request, result) {
            Arrival::Elsewhere => {}
            Arrival::Moving { done, total } => {
                let tenth = u32::try_from(u64::from(done) * 10 / u64::from(total.max(1)))
                    .unwrap_or(u32::MAX);
                if fetch.opening && tenth != fetch.shown {
                    fetch.shown = tenth;
                    if matches!(self.view, View::Opening(_)) {
                        self.show(context);
                    }
                }
            }
            Arrival::Done => {
                if let Some(fetch) = self.fetching.take() {
                    let id = fetch.assembly.id().to_owned();
                    let bytes = fetch.assembly.take();
                    self.on_document(context, &id, &bytes);
                }
            }
            Arrival::Failed(fault) => {
                if let Some(fetch) = self.fetching.take() {
                    self.on_fetch_failed(context, &fetch, fault);
                }
            }
        }
    }

    /// A large book could not be put together.
    fn on_fetch_failed(&mut self, context: &mut Context, fetch: &Fetch, fault: Fault) {
        let id = fetch.assembly.id();
        if let Fault::TooLarge(size) = fault {
            // The listing was older than the file. The shelf and the note
            // both go by the length the runtime just gave.
            if let Some(document) = self.documents.iter_mut().find(|document| document.id == id) {
                document.bytes = size;
            }
        }
        if fetch.opening {
            self.replaced = fault == Fault::Changed;
        }
        self.on_document_refused(context, id);
        // A book the shelf offered to open is now one it says is too large.
        if matches!(fault, Fault::TooLarge(_)) && matches!(self.view, View::Home | View::Shelf(_)) {
            self.show(context);
        }
    }

    fn finish_open(
        &mut self,
        context: &mut Context,
        index: usize,
        document: &Document,
        bytes: &[u8],
    ) {
        // An EPUB is a zip. One that is not, a download cut short or a file
        // renamed by mistake, used to open as its own bytes set as prose.
        let not_a_book = document.kind == Kind::Epub && !bytes.starts_with(b"PK\x03\x04");
        if bytes.is_empty() || not_a_book {
            self.book.close(context);
            self.open_title = None;
            self.view = View::Unreadable(index);
            self.show(context);
            return;
        }
        let id = position_id(&document.id);
        let memory = self.progress.memory(&id);
        if self
            .book
            .open_bytes(context, document.kind.filename(), bytes, memory)
            .is_ok()
        {
            // A book whose package Books has not read yet, such as an article
            // opened from its notice the moment it arrived, is still known by
            // its file name, and the bar said "Electronic paper (3)". The
            // package names it, and reading only that much is cheap.
            let named = (document.kind == Kind::Epub)
                .then(|| kobo_doc::epub::names(bytes).ok()?.title)
                .flatten()
                .map(|title| drawable(&title));
            self.open_title = Some(named.unwrap_or_else(|| document.title.clone()));
            self.opened = Some(document.id.clone());
            self.reading_id = Some(id);
            self.view = View::Reading;
        } else {
            self.book.close(context);
            self.open_title = None;
            self.view = View::Unreadable(index);
        }
        self.show(context);
    }

    fn close_reader(&mut self, context: &mut Context) {
        self.keep_position(context);
        self.note_reading(context);
        self.book.close(context);
        self.open_title = None;
        self.opened = None;
        self.reading_id = None;
        self.view = self.came_from;
        self.show(context);
    }

    /// Writes down where the open book is, at what size, with its marks.
    ///
    /// A record that failed to save is tried again on the next page turn
    /// rather than given up on, because the usual cause is a write that
    /// collided with something else and will go through the next time.
    fn keep_position(&mut self, context: &mut Context) {
        let (Some(id), Some(memory)) = (&self.reading_id, self.book.memory()) else {
            return;
        };
        self.progress.keep(context, id.clone(), memory.clone());
        if self.progress.failed {
            self.progress.retry(context);
        }
    }

    fn read_action(&mut self, context: &mut Context, action: ActionId) -> bool {
        if !matches!(self.view, View::Reading) {
            return false;
        }
        if let Some(outcome) = self.book.act(context, action) {
            self.after_reader(context, outcome);
            return true;
        }
        if action == ActionId::BACK || action == action_id(BACK) {
            self.close_reader(context);
            return true;
        }
        false
    }

    /// Search and order on the Library shelf. Returns whether the action
    /// was one of theirs.
    fn library_action(&mut self, context: &mut Context, action: ActionId) -> bool {
        if self.view != View::Shelf(Shelf::Library) {
            return false;
        }
        match self.library.search.handle(action) {
            Some(Typing::Submitted(words)) => {
                self.library.query = words;
                self.page = 0;
                self.show(context);
                return true;
            }
            // Backing out of the field leaves the shelf as it was, narrowed
            // or not.
            Some(_) => {
                self.show(context);
                return true;
            }
            None => {}
        }
        if action == action_id(SEARCH) {
            self.library.search.open();
            self.show(context);
            return true;
        }
        if action == action_id(CLEAR_SEARCH) {
            self.library.query.clear();
            self.page = 0;
            self.show(context);
            return true;
        }
        if let Some(order) = Order::ALL
            .into_iter()
            .find(|order| action == action_id(order.name()))
        {
            if order != self.library.order {
                self.library.order = order;
                self.page = 0;
                context
                    .store()
                    .save(ORDER, order.name().as_bytes().to_vec());
            }
            self.show(context);
            return true;
        }
        // Back undoes the nearest thing first: the search, and only then
        // the shelf.
        if (action == action_id(BACK) || action == ActionId::BACK) && !self.library.query.is_empty()
        {
            self.library.query.clear();
            self.page = 0;
            self.show(context);
            return true;
        }
        false
    }

    /// Does what the reader left for Books to do about something it handled.
    fn after_reader(&mut self, context: &mut Context, outcome: Outcome) {
        match outcome {
            Outcome::Close => self.close_reader(context),
            Outcome::Light(level) => {
                context.device().set_frontlight(level);
                self.keep_position(context);
                self.show(context);
            }
            Outcome::Save => {
                self.keep_position(context);
                self.show(context);
            }
            Outcome::Elsewhere | Outcome::Repaint => self.show(context),
        }
    }

    /// The Articles shelf's taps. Returns whether the action was one.
    fn article_action(&mut self, context: &mut Context, action: ActionId) -> bool {
        let shelf_of = |books: &Self, id: Option<&str>| {
            let archived = id.is_some_and(|id| books.kept(id).archived);
            View::Shelf(if archived {
                Shelf::Archive
            } else {
                Shelf::Articles
            })
        };
        if action == action_id(SYNC) {
            self.articles.delete_failed = false;
            self.sync(context, false);
            self.show(context);
            return true;
        }
        if action == action_id(ARCHIVED) {
            self.page = 0;
            self.articles.menu_open = None;
            self.articles.delete_failed = false;
            self.view = View::Shelf(Shelf::Archive);
            self.show(context);
            return true;
        }
        if self.view == View::ConfirmDelete {
            let id = self.articles.confirming.clone();
            if action == action_id(CONFIRM_DELETE) {
                if let Some(id) = &id {
                    self.articles.deleting = Some(id.clone());
                    context.device().remove_article(id.clone());
                }
            } else if action != action_id(KEEP_ARTICLE) && action != ActionId::BACK {
                return false;
            }
            self.view = shelf_of(self, id.as_deref());
            self.articles.confirming = None;
            self.show(context);
            return true;
        }
        let View::Shelf(pile) = self.view else {
            return false;
        };
        if !pile.lists_articles() {
            return false;
        }
        if action == ActionId::BACK || action == action_id(BACK) {
            if self.articles.menu_open.take().is_none() {
                self.view = if pile == Shelf::Archive {
                    View::Shelf(Shelf::Articles)
                } else {
                    View::Home
                };
                self.page = 0;
                self.articles.delete_failed = false;
            }
            self.show(context);
            return true;
        }
        if self.menu_action(context, action) {
            return true;
        }
        let members = self.members(pile);
        let row_named = |name: &str| {
            (0..members.len()).find(|row| action == action_id(&format!("{name}-{row}")))
        };
        if let Some(row) = row_named("article-menu") {
            let id = self.documents[members[row]].id.clone();
            self.articles.menu_open = if self.articles.menu_open.as_deref() == Some(id.as_str()) {
                None
            } else {
                Some(id)
            };
            self.show(context);
            return true;
        }
        if let Some(row) = row_named("article") {
            self.articles.menu_open = None;
            self.open_document(context, members[row]);
            return true;
        }
        false
    }

    /// A choice from the menu open on one article's row.
    fn menu_action(&mut self, context: &mut Context, action: ActionId) -> bool {
        let Some(open) = self.articles.menu_open.clone() else {
            return false;
        };
        let change: Option<fn(&mut Kept)> = match action {
            action if action == action_id(MARK_READ) => Some(|kept| kept.read = true),
            action if action == action_id(MARK_UNREAD) => Some(|kept| kept.read = false),
            action if action == action_id(ARCHIVE) => Some(|kept| kept.archived = true),
            action if action == action_id(RESTORE) => Some(|kept| kept.archived = false),
            _ => None,
        };
        if let Some(change) = change {
            self.articles.menu_open = None;
            if let Some(index) = self
                .documents
                .iter()
                .position(|document| document.id == open)
            {
                self.mark(context, index, change);
            }
            self.show(context);
            return true;
        }
        if action == action_id(DELETE) {
            self.articles.menu_open = None;
            self.articles.confirming = Some(open);
            self.view = View::ConfirmDelete;
            self.show(context);
            return true;
        }
        false
    }

    /// The runtime's answer to a delete. The article leaves the shelf only
    /// once the file is gone, so a refusal never hides one that is still
    /// there.
    fn removed(&mut self, context: &mut Context, id: &str, result: &DeviceResult) {
        if self.articles.deleting.as_deref() == Some(id) {
            self.articles.deleting = None;
        }
        if matches!(result, DeviceResult::Done) {
            self.documents.retain(|document| document.id != id);
            self.recent.retain(|recent| recent.id != id);
            self.articles.kept.remove(id);
            self.save_articles(context);
            context
                .store()
                .save(RECENT, covers::encode_recent(&self.recent));
            self.articles.delete_failed = false;
        } else {
            self.articles.delete_failed = true;
        }
        if matches!(self.view, View::Home | View::Shelf(_)) {
            self.show(context);
        }
    }

    fn on_listing(&mut self, context: &mut Context, entries: Vec<LibraryEntry>, truncated: bool) {
        self.library_failed = false;
        self.truncated = truncated;
        self.take_documents(entries.into_iter().filter_map(from_entry).collect());
        if self.documents.iter().any(Document::in_ranges) {
            self.ask_about_ranges(context);
        }
        self.take_articles(truncated);
        self.want_articles();
        self.articles.menu_open = None;
        if let Some(id) = self.asked_to_open.take() {
            // An article opened from its notice closes onto its own shelf.
            if articles::is_article(&id) {
                self.view = View::Shelf(Shelf::Articles);
            }
            // Gone since it was announced: the shelf is the honest answer.
            if let Some(index) = self.documents.iter().position(|document| document.id == id) {
                self.open_document(context, index);
                return;
            }
        }
        if matches!(self.view, View::Home | View::Shelf(_)) {
            self.show(context);
        }
    }

    fn on_document(&mut self, context: &mut Context, id: &str, bytes: &[u8]) {
        if self.describing.as_deref() == Some(id) {
            self.describing = None;
            self.describe(context, id, bytes);
        }
        if let View::Opening(index) = self.view {
            if let Some(document) = self.documents.get(index).cloned() {
                if document.id == id {
                    self.finish_open(context, index, &document, bytes);
                    return;
                }
            }
        }
        if matches!(self.view, View::Home | View::Shelf(_)) {
            self.show(context);
        } else {
            self.read_next(context);
        }
    }

    /// The runtime would not hand over `id`, whole or in ranges.
    ///
    /// The refusal is matched to what it was for. A book can be on its way
    /// for its cover while another is being opened, and a refusal used to be
    /// taken for both: a cover that could not be read closed the book the
    /// owner had just tapped.
    fn on_document_refused(&mut self, context: &mut Context, id: &str) {
        if self.describing.as_deref() == Some(id) {
            self.describing = None;
            // Not read, and not worth asking about again on every repaint.
            let (bytes, listed) = self
                .documents
                .iter()
                .find(|document| document.id == id)
                .map_or((0, String::new()), |document| {
                    (document.bytes, document.listed.clone())
                });
            if let Some(kept) = self.articles.kept.get_mut(id) {
                // Not asked about again on every listing either.
                kept.described = true;
                self.articles.changed = true;
            }
            self.described.insert(
                id.to_owned(),
                Described {
                    bytes,
                    listed,
                    title: None,
                    author: None,
                    cover: Cover::Missing,
                },
            );
            self.record = Record::Changed;
        }
        if let View::Opening(index) = self.view {
            let opening = self
                .documents
                .get(index)
                .is_some_and(|document| document.id == id);
            if opening {
                self.book.close(context);
                self.open_title = None;
                self.view = View::Unreadable(index);
                self.show(context);
                return;
            }
        }
        self.read_next(context);
    }

    fn on_cover(&mut self, context: &mut Context, key: &str, result: StoreResult) {
        let Some(id) = self.loading.remove(key) else {
            return;
        };
        let thumb = match result {
            StoreResult::Loaded {
                value: Some(bytes), ..
            } => covers::from_png(&bytes),
            _ => None,
        };
        if let Some(thumb) = thumb {
            self.hold(&id, thumb);
        } else if let Some(known) = self.described.get_mut(&id) {
            // Gone from the store, or unreadable: read the book again.
            known.cover = Cover::Unkept;
        }
        if matches!(self.view, View::Home | View::Shelf(_)) {
            self.show(context);
        }
    }
}

impl KoboApp for Books {
    /// The owner tapped a notice announcing this document. Listed afresh
    /// first, because the document arrived after any listing Books holds.
    fn on_open_document(&mut self, context: &mut Context, id: String) {
        self.asked_to_open = Some(id);
        context.device().list_library();
    }

    /// The platform's own blue: this screen is Cobalt itself rather than an
    /// application running on it, and it wears the colour of the logo.
    fn accent(&self) -> Option<kobo_sdk::Accent> {
        Some(kobo_sdk::Accent::Cobalt)
    }

    fn on_start(&mut self, context: &mut Context) {
        context.store().load(READING);
        context.store().load(DESCRIBED);
        context.store().load(RECENT);
        context.store().load(articles::RECORD);
        // The hosts before the pairing, so they are in hand when it arrives
        // and the quiet check goes out.
        context.store().load(articles::HOSTS);
        context.store().load(articles::PAIRING);
        context.store().load(ORDER);
        if self.documents.is_empty() {
            context.device().list_library();
        }
        self.show(context);
    }

    fn on_load(&mut self, context: &mut Context, key: &str, result: StoreResult) {
        match key {
            READING => {
                self.progress.load(result);
                self.show(context);
            }
            key if covers::described_part(key).is_some() => {
                let index = covers::described_part(key).unwrap_or_default();
                if index != self.described_parts || self.record != Record::Unread {
                    return;
                }
                if let StoreResult::Loaded {
                    value: Some(bytes), ..
                } = result
                {
                    if !bytes.is_empty() {
                        self.described.extend(covers::decode_described(&bytes));
                        self.described_parts = index + 1;
                        // Parts are read one after another, and the first
                        // empty or missing one is the end.
                        if index + 1 < covers::DESCRIBED_PARTS {
                            context.store().load(covers::described_key(index + 1));
                            return;
                        }
                    }
                }
                self.record = Record::Kept;
                let documents = std::mem::take(&mut self.documents);
                self.take_documents(documents);
                if matches!(self.view, View::Home | View::Shelf(_)) {
                    self.show(context);
                }
            }
            ORDER => {
                if let StoreResult::Loaded {
                    value: Some(bytes), ..
                } = result
                {
                    // A name this build does not know, from a later one or a
                    // damaged value, leaves the shelf as it was.
                    if let Some(order) = Order::ALL
                        .into_iter()
                        .find(|order| order.name().as_bytes() == bytes.as_slice())
                    {
                        self.library.order = order;
                    }
                }
                if matches!(self.view, View::Home | View::Shelf(_)) {
                    self.show(context);
                }
            }
            RECENT => {
                if let StoreResult::Loaded {
                    value: Some(bytes), ..
                } = result
                {
                    self.recent = covers::decode_recent(&bytes);
                }
                if matches!(self.view, View::Home | View::Shelf(_)) {
                    self.show(context);
                }
            }
            key if articles::part_of(key).is_some() => {
                let index = articles::part_of(key).unwrap_or_default();
                if index != self.articles.parts_stored || self.articles.loaded {
                    return;
                }
                if let StoreResult::Loaded {
                    value: Some(bytes), ..
                } = result
                {
                    if !bytes.is_empty() {
                        self.articles.kept.extend(articles::decode(&bytes));
                        self.articles.parts_stored = index + 1;
                        // Parts are read one after another, and the first
                        // empty or missing one is the end.
                        if index + 1 < articles::PARTS {
                            context.store().load(articles::part_key(index + 1));
                            return;
                        }
                    }
                }
                self.articles.loaded = true;
                self.take_articles(self.truncated);
                self.want_articles();
                if matches!(self.view, View::Home | View::Shelf(_)) {
                    self.show(context);
                }
            }
            articles::HOSTS => self.articles.hosts_loaded(result),
            articles::PAIRING => {
                if let StoreResult::Loaded {
                    value: Some(bytes), ..
                } = result
                {
                    self.articles.computer = articles::pairing(&bytes);
                }
                self.articles.know_hosts();
                // Opening Books asks the computer, without a word unless
                // something arrives.
                if self.articles.checked_at.is_none() {
                    self.sync(context, true);
                }
                if matches!(self.view, View::Home | View::Shelf(_)) {
                    self.show(context);
                }
            }
            _ => self.on_cover(context, key, result),
        }
    }

    /// A copy of the open book's notes is written to the shelf before it is
    /// offered to a computer, and the answers about that file are the
    /// reader's.
    fn on_shelf(&mut self, context: &mut Context, name: &str, result: StoreResult) {
        if self.book.shelved(context, name, &result) && matches!(self.view, View::Reading) {
            self.show(context);
        }
    }

    fn on_save(&mut self, context: &mut Context, key: &str, result: StoreResult) {
        if self.book.saved(context, key, &result) {
            if matches!(self.view, View::Reading) {
                self.show(context);
            }
            return;
        }
        if key == READING {
            self.progress.saved(context, result);
            if matches!(self.view, View::Home | View::Shelf(_)) {
                self.show(context);
            }
            return;
        }
        if key.starts_with("cover-") && !matches!(result, StoreResult::Saved { .. }) {
            // The cover did not fit. It is read from the book again next time.
            for known in self.described.values_mut() {
                if known.cover == Cover::Kept(key.to_owned()) {
                    known.cover = Cover::Unkept;
                    self.record = Record::Changed;
                }
            }
        }
    }

    /// Lists the card again when the shelf comes back into view, so a book
    /// copied on while Books was in the background is there without a
    /// restart.
    fn on_foreground(&mut self, context: &mut Context) {
        if matches!(self.view, View::Home | View::Shelf(_)) {
            context.device().list_library();
        }
        // The pairing may have been written while Books was away, and the
        // computer may have something by now.
        context.store().load(articles::HOSTS);
        context.store().load(articles::PAIRING);
        let due = match (self.articles.checked_at, now_millis()) {
            (Some(then), Some(now)) => now.saturating_sub(then) >= QUIET_CHECK_EVERY_MS,
            _ => true,
        };
        if due {
            self.articles.checked_at = None;
        }
    }

    /// Keeps the place before the panel goes elsewhere, because an
    /// application in the background can be stopped without another word.
    fn on_background(&mut self, context: &mut Context) {
        self.keep_position(context);
        self.note_reading(context);
        if self.record == Record::Changed {
            self.save_described(context);
        }
        if self.articles.changed {
            self.save_articles(context);
        }
    }

    fn on_suspend(&mut self, context: &mut Context) {
        self.keep_position(context);
        self.note_reading(context);
    }

    fn on_exit(&mut self, context: &mut Context) {
        self.keep_position(context);
        self.note_reading(context);
        if self.record == Record::Changed {
            self.save_described(context);
        }
        if self.articles.changed {
            self.save_articles(context);
        }
    }

    /// A finger held on a word of the open book looks the word up, and from
    /// there it can be highlighted or given a note. The shared reader owns
    /// all of that; Books only hands the hold over.
    fn on_text_hold(&mut self, context: &mut Context, action: ActionId, hit: kobo_sdk::TextHit) {
        if matches!(self.view, View::Reading) {
            if let Some(outcome) = self.book.hold(context, action, hit) {
                self.after_reader(context, outcome);
                return;
            }
        }
        self.on_action(context, action);
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if self.read_action(context, action) {
            return;
        }
        if self.article_action(context, action) {
            return;
        }
        if self.library_action(context, action) {
            return;
        }
        if action == action_id(RELIST) {
            self.library_failed = false;
            context.device().list_library();
            return;
        }
        if action == action_id(RETRY_SAVE) {
            self.progress.retry(context);
            self.show(context);
            return;
        }
        if action == action_id(CONTINUE) {
            if let Some(index) = self.current() {
                self.open_document(context, index);
            }
            return;
        }
        if let Some(shelf) = Shelf::ALL
            .into_iter()
            .find(|shelf| action == action_id(shelf.name()))
        {
            self.page = 0;
            self.library.query.clear();
            self.view = View::Shelf(shelf);
            self.show(context);
            return;
        }
        if let View::Unreadable(index) = self.view {
            if action == action_id(OPEN_AGAIN) {
                self.view = self.came_from;
                self.open_document(context, index);
                return;
            }
        }
        // Giving up on a large book returns to where it was tapped, as Back
        // from the book itself would have.
        let giving_up = self.opening_in_ranges()
            && (action == action_id(CANCEL_OPEN)
                || action == action_id(BACK)
                || action == ActionId::BACK);
        if giving_up {
            self.stop_fetching();
            self.view = self.came_from;
            self.show(context);
            return;
        }
        if action == action_id(BACK) || action == ActionId::BACK {
            self.view = match self.view {
                View::Unreadable(_) => self.came_from,
                _ => View::Home,
            };
            self.show(context);
            return;
        }
        if let View::Shelf(shelf) = self.view {
            if action == action_id(NEXT) || action == action_id(PREVIOUS) {
                let count = self.standing_on(shelf).len();
                let pages = self.pages(context, shelf, count).len().max(1);
                self.page = if action == action_id(NEXT) {
                    (self.page + 1).min(pages - 1)
                } else {
                    self.page.saturating_sub(1)
                };
                self.show(context);
                return;
            }
        }
        if let Some(index) =
            (0..self.documents.len()).find(|index| action == action_id(&format!("book-{index}")))
        {
            self.open_document(context, index);
        }
    }

    fn on_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
        if let Some((syncing, quiet)) = self.articles.syncing {
            if syncing == task {
                self.articles.syncing = None;
                match &outcome {
                    // A computer not at this host may be at the other one,
                    // asked at once in the same voice.
                    TaskOutcome::Failed(error) if self.articles.hosts.failed(*error) => {
                        self.sync(context, quiet);
                        return;
                    }
                    TaskOutcome::Completed(_) => {
                        if let Some(record) = self.articles.hosts.answered() {
                            self.articles.hosts_record = Some(record.clone().into_bytes());
                            context.store().save(articles::HOSTS, record);
                        }
                    }
                    _ => {}
                }
                self.synced(context, quiet, outcome);
                return;
            }
        }
        match self.book.woke(context, task, &outcome) {
            Step::Repaint => self.show(context),
            Step::Quiet | Step::Elsewhere => {}
        }
    }

    fn on_device_result(
        &mut self,
        context: &mut Context,
        request: DeviceRequest,
        result: DeviceResult,
    ) {
        if request == DeviceRequest::ReadFrontlight {
            if let DeviceResult::Frontlight { percent } = result {
                if self.book.took_light(percent) {
                    self.show(context);
                }
            }
            return;
        }
        if let DeviceRequest::LookupWord { .. } = request {
            if self.book.took_definition(&request, &result) && matches!(self.view, View::Reading) {
                self.show(context);
            }
            return;
        }
        if request == DeviceRequest::ListLibrary {
            match result {
                DeviceResult::Library { entries, truncated } => {
                    self.on_listing(context, entries, truncated);
                }
                DeviceResult::Denied(_) | DeviceResult::Failed(_) => {
                    self.library_failed = true;
                    if matches!(self.view, View::Home | View::Shelf(_)) {
                        self.show(context);
                    }
                }
                _ => {}
            }
            return;
        }
        if let DeviceRequest::RemoveArticle { id } = request {
            self.removed(context, &id, &result);
            return;
        }
        if request == DeviceRequest::ReadIdentity {
            self.on_identity(context, &result);
            return;
        }
        if let DeviceRequest::ReadLibraryRange { .. } = request {
            self.on_range(context, &request, &result);
            return;
        }
        if let DeviceRequest::ReadLibrary { id: asked } = request {
            match result {
                DeviceResult::LibraryDocument { id, bytes } => {
                    self.on_document(context, &id, &bytes);
                }
                DeviceResult::Denied(_) | DeviceResult::Failed(_) => {
                    self.on_document_refused(context, &asked);
                }
                _ => {}
            }
        }
    }

    fn on_page_turn(&mut self, context: &mut Context, forward: bool) {
        if matches!(self.view, View::Reading) {
            let action = if forward {
                kobo_read::action::FORWARD
            } else {
                kobo_read::action::BACK
            };
            let _ = self.read_action(context, action_id(action));
        }
    }
}

fn books_word(count: usize) -> String {
    if count == 1 {
        "1 book".to_owned()
    } else {
        format!("{count} books")
    }
}

/// What the Articles pile says under its name: how many are unread, or how
/// many there are once all are read.
fn articles_word(members: &[usize], unread: impl Fn(usize) -> bool) -> String {
    let waiting = members.iter().filter(|&&index| unread(index)).count();
    match (members.len(), waiting) {
        (0, _) => "No articles".to_owned(),
        (_, 0) if members.len() == 1 => "1 article".to_owned(),
        (count, 0) => format!("{count} articles"),
        (_, waiting) => format!("{waiting} unread"),
    }
}

fn now_millis() -> Option<u64> {
    kobo_sdk::clock::SystemClock::reader()
        .now()
        .ok()
        .map(|now| now.unix_millis)
}

/// A name as a search compares it: in lower case and without its accents.
///
/// The reader's keyboard types plain letters far more easily than marked
/// ones, and nobody searching for Bronte should have to find the diaeresis
/// first. Covers the Latin letters European names are written in, as a
/// table, because that is what a title on this shelf and a query from that
/// keyboard are made of. A name in another script is compared as it is,
/// which is still an exact match. Marks written as separate characters
/// after their letter, as some files carry them, are dropped.
fn folded(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for letter in text.chars().flat_map(char::to_lowercase) {
        let plain = match letter {
            '\u{0300}'..='\u{036f}' => "",
            'à'..='å' | 'ā' | 'ă' | 'ą' => "a",
            'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => "c",
            'ď' | 'đ' | 'ð' => "d",
            'è'..='ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => "e",
            'ĝ' | 'ğ' | 'ġ' | 'ģ' => "g",
            'ĥ' | 'ħ' => "h",
            'ì'..='ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => "i",
            'ĵ' => "j",
            'ķ' => "k",
            'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => "l",
            'ñ' | 'ń' | 'ņ' | 'ň' => "n",
            'ò'..='ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => "o",
            'ŕ' | 'ŗ' | 'ř' => "r",
            'ś' | 'ŝ' | 'ş' | 'š' => "s",
            'ţ' | 'ť' | 'ŧ' => "t",
            'ù'..='ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => "u",
            'ŵ' => "w",
            'ý' | 'ÿ' | 'ŷ' => "y",
            'ź' | 'ż' | 'ž' => "z",
            'ß' => "ss",
            'æ' => "ae",
            'œ' => "oe",
            'þ' => "th",
            other => {
                out.push(other);
                continue;
            }
        };
        out.push_str(plain);
    }
    out
}

/// A count as it is written for a person: 2,000 rather than 2000.
fn grouped(count: usize) -> String {
    let digits = count.to_string();
    let mut out = String::new();
    for (place, digit) in digits.chars().enumerate() {
        if place > 0 && (digits.len() - place) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

fn megabytes(bytes: u32) -> String {
    format!("{:.1} MB", f64::from(bytes) / (1024.0 * 1024.0))
}

/// The record a book's place is kept under.
///
/// The shared record names an article by the SHA-256 of something that
/// identifies it, and only that, so a library path never reaches the file.
/// The prefix keeps a book from ever sharing a record with anything else
/// that might one day be kept in the same file.
/// A book's name as the shelf can draw it.
///
/// Titles and authors come from whoever made the file, in whatever script
/// they wrote in. A letter no installed face carries is put as a box, the way
/// the page draws it, rather than refusing the shelf that lists the book.
fn drawable(text: &str) -> String {
    kobo_sdk::drawable(text).into_owned()
}

fn position_id(document: &str) -> String {
    kobo_bookview::positions::identity(&format!("books:{document}"))
}

fn from_entry(entry: LibraryEntry) -> Option<Document> {
    let kind = match entry.kind {
        1 => Kind::Epub,
        2 => Kind::Markdown,
        3 => Kind::Html,
        4 => Kind::Text,
        5 => Kind::Pdf,
        6 => Kind::Kepub,
        _ => return None,
    };
    Some(Document {
        id: entry.id,
        preview: entry.title.clone(),
        listed: entry.title.clone(),
        title: drawable(&entry.title),
        author: None,
        kind,
        on_card: entry.on_card,
        bytes: entry.bytes,
        body: None,
    })
}

fn fixture_bytes(document: &Document, body: &str) -> Vec<u8> {
    match document.kind {
        Kind::Epub => kobo_doc::epub::write(
            &document.title,
            Some("A. Mariner"),
            &[kobo_doc::epub::Chapter {
                title: "Leaving harbour".to_owned(),
                body: body.to_owned(),
            }],
        )
        .unwrap_or_default(),
        // Neither is handed to a reader, so neither needs bytes.
        Kind::Pdf | Kind::Kepub => Vec::new(),
        Kind::Markdown | Kind::Html | Kind::Text => body.as_bytes().to_vec(),
    }
}

fn tile_pixels(context: &Context) -> (u32, u32) {
    let (width, height) = context.metrics().tile_body(TileShape::Portrait);
    (
        u32::try_from(width.max(1)).unwrap_or(1),
        u32::try_from(height.max(1)).unwrap_or(1),
    )
}

fn main() -> ExitCode {
    match kobo_sdk::run("books", Books::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("books: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        articles, covers, fixtures, Books, Document, Kind, Shelf, View, CONTINUE, DESCRIBED,
        READING, RECENT, SHELF_PAGE,
    };
    use kobo_sdk::prelude::*;
    use kobo_sdk::{
        document_preview, DeviceResult, LibraryEntry, StoreRequest, StoreResult, TaskId,
        TaskOutcome, CLARA_BW_METRICS,
    };
    use kobo_ui::{Chrome, LayoutKind, Tile, TileShape};

    fn shown(commands: Vec<Command>) -> Screen {
        commands
            .into_iter()
            .rev()
            .find_map(|command| match command {
                Command::SetScreen(screen) => Some(screen),
                _ => None,
            })
            .expect("a screen")
    }

    fn words(screen: &Screen) -> String {
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true));
        layout
            .nodes
            .iter()
            .flat_map(|node| &node.text_lines)
            .cloned()
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn portrait_tiles(screen: &Screen) -> Vec<Tile> {
        screen
            .nodes
            .iter()
            .find_map(|node| match node {
                Node::TileGrid {
                    shape: TileShape::Portrait,
                    tiles,
                    ..
                } => Some(tiles.clone()),
                _ => None,
            })
            .unwrap_or_default()
    }

    fn open(runner: &mut AppRunner<Books>, kind: Kind) -> Screen {
        let index = runner
            .app()
            .documents
            .iter()
            .position(|document| document.kind == kind)
            .expect("a document of that kind");
        shown(runner.action(action_id(&format!("book-{index}"))))
    }

    fn entry(id: &str, title: &str, kind: u8, bytes: u32, on_card: bool) -> LibraryEntry {
        LibraryEntry {
            id: id.to_owned(),
            title: title.to_owned(),
            kind,
            bytes,
            on_card,
        }
    }

    /// A plain text book long enough to turn several pages.
    fn long_book() -> Document {
        let body = (1..=400)
            .map(|line| format!("Line {line} of a letter that runs on for page after page."))
            .collect::<Vec<_>>()
            .join("\n\n");
        Document {
            id: "0/long-letter.txt".to_owned(),
            title: "A Long Letter".to_owned(),
            listed: "A Long Letter".to_owned(),
            author: None,
            kind: Kind::Text,
            preview: "Line 1".to_owned(),
            on_card: true,
            bytes: 0,
            body: Some(body),
        }
    }

    /// Answers the records Books asks for as it starts, in the order it
    /// asks: the store answers in order, whatever key a result names.
    fn answer_start(
        runner: &mut AppRunner<Books>,
        reading: Option<Vec<u8>>,
        described: Option<Vec<u8>>,
        recent: Option<Vec<u8>>,
    ) -> Vec<Command> {
        let mut commands = Vec::new();
        // A record that was kept is followed by a question about its next
        // part, which Books asks on reading the first and so is answered
        // after everything it asked as it started.
        let more = described.as_ref().is_some_and(|record| !record.is_empty());
        for (key, value) in [
            (READING, reading),
            (DESCRIBED, described),
            (RECENT, recent),
            (articles::RECORD, None),
            (articles::HOSTS, None),
            (articles::PAIRING, None),
            (super::ORDER, None),
        ] {
            commands.extend(runner.store_result(StoreResult::Loaded {
                key: key.into(),
                value,
            }));
        }
        if more {
            commands.extend(runner.store_result(StoreResult::Loaded {
                key: covers::described_key(1),
                value: None,
            }));
        }
        commands
    }

    /// Books started fresh, with its saved records handed back the way the
    /// store hands them back after a restart.
    fn started_with(record: Option<Vec<u8>>, recent: Option<Vec<u8>>) -> AppRunner<Books> {
        let mut books = Books::empty();
        books.take_documents(vec![long_book()]);
        let mut runner = AppRunner::new(books);
        runner.start();
        answer_start(&mut runner, record, None, recent);
        runner
    }

    fn saved(commands: &[Command], wanted: &str) -> Option<Vec<u8>> {
        commands.iter().find_map(|command| match command {
            Command::Store(StoreRequest::Save { key, value }) if key == wanted => {
                Some(value.clone())
            }
            _ => None,
        })
    }

    /// An EPUB whose package names a cover picture, as Standard Ebooks and
    /// most publishers write one.
    fn a_book_with_a_cover() -> Vec<u8> {
        a_book_with_a_cover_and(0)
    }

    /// The same book carrying `padding` bytes nothing in it refers to, so it
    /// takes several ranges to arrive without taking any longer to parse.
    fn a_book_with_a_cover_and(padding: usize) -> Vec<u8> {
        let cover =
            kobo_image::encode_png_grey(200, 300, &vec![0x20; 200 * 300]).expect("a cover picture");
        let members = [
            ("mimetype", b"application/epub+zip".to_vec()),
            (
                "META-INF/container.xml",
                br#"<container><rootfiles><rootfile full-path="OEBPS/book.opf"/></rootfiles></container>"#
                    .to_vec(),
            ),
            (
                "OEBPS/book.opf",
                br#"<package version="3.0"><metadata>
                    <dc:title>The Open Sea</dc:title><dc:creator>A. Mariner</dc:creator>
                  </metadata><manifest>
                    <item id="one" href="one.xhtml" media-type="application/xhtml+xml"/>
                    <item id="art" href="cover.png" media-type="image/png" properties="cover-image"/>
                  </manifest><spine><itemref idref="one"/></spine></package>"#
                    .to_vec(),
            ),
            ("OEBPS/one.xhtml", b"<p>The tide was turning.</p>".to_vec()),
            ("OEBPS/cover.png", cover),
        ];
        let mut members: Vec<(String, Vec<u8>)> = members
            .into_iter()
            .map(|(name, bytes)| (name.to_owned(), bytes))
            .collect();
        if padding > 0 {
            members.push(("OEBPS/plates.bin".to_owned(), vec![0x55; padding]));
        }
        kobo_doc::zip::stored(&members).expect("a book")
    }

    const ATLAS: &str = "0/Atlas of the Coast.epub";
    /// What the listing says the large book weighs: more than one answer
    /// carries. The file the tests serve is smaller, which a listing older
    /// than the file can be, and keeps them quick.
    const LISTED_LARGE: u32 = 6 * 1024 * 1024;

    /// Tells Books it is under the runtime it shipped with, and returns what
    /// it did about it.
    fn runner_identity(runner: &mut AppRunner<Books>) -> Vec<Command> {
        runner.device_result(this_runtime())
    }

    fn runtime(version: &str) -> DeviceResult {
        DeviceResult::Identity(kobo_sdk::DeviceIdentity {
            runtime_version: version.to_owned(),
            ..kobo_sdk::DeviceIdentity::default()
        })
    }

    fn this_runtime() -> DeviceResult {
        runtime(env!("CARGO_PKG_VERSION"))
    }

    fn requests(commands: &[Command]) -> Vec<DeviceRequest> {
        commands
            .iter()
            .filter_map(|command| match command {
                Command::Device(request) => Some(request.clone()),
                _ => None,
            })
            .collect()
    }

    fn ranges_asked(commands: &[Command]) -> Vec<u32> {
        requests(commands)
            .iter()
            .filter_map(|request| match request {
                DeviceRequest::ReadLibraryRange { offset, .. } => Some(*offset),
                _ => None,
            })
            .collect()
    }

    /// Answers every device request in `commands`, and every request those
    /// answers lead to, in the order they were made: ranges from `card`,
    /// which is asked for the file and its stamp each time so a test can
    /// change the file part-way, and the light a book asks about when it
    /// opens. Returns everything Books did along the way.
    fn serve(
        runner: &mut AppRunner<Books>,
        commands: Vec<Command>,
        card: &mut dyn FnMut(&DeviceRequest) -> DeviceResult,
    ) -> Vec<Command> {
        let mut seen = Vec::new();
        let mut waiting: std::collections::VecDeque<Command> = commands.into();
        let mut answered = 0;
        while let Some(command) = waiting.pop_front() {
            if let Command::Device(request) = &command {
                answered += 1;
                assert!(answered < 2_000, "Books never stopped asking");
                let result = match request {
                    DeviceRequest::ReadLibraryRange { .. } => card(request),
                    DeviceRequest::ReadFrontlight | DeviceRequest::SetFrontlight { .. } => {
                        DeviceResult::Frontlight { percent: 20 }
                    }
                    other => panic!("a request the test has no answer for: {other:?}"),
                };
                waiting.extend(runner.device_result(result));
            }
            seen.push(command);
        }
        seen
    }

    /// Books started on a card holding one large book, with the records it
    /// keeps answered and the listing in, up to the point where it has asked
    /// the runtime which release it is.
    fn started_with_a_large_book() -> (AppRunner<Books>, Vec<Command>) {
        let mut runner = AppRunner::new(Books::empty());
        runner.start();
        answer_start(&mut runner, None, None, None);
        let commands = runner.device_result(DeviceResult::Library {
            entries: vec![entry(ATLAS, "atlas", 1, LISTED_LARGE, true)],
            truncated: false,
        });
        (runner, commands)
    }

    #[test]
    fn the_first_screen_fits_a_clara() {
        let mut runner = AppRunner::new(Books::seeded());
        let screen = shown(runner.start());
        assert!(screen.validate(&CLARA_BW_METRICS).is_empty());
    }

    #[test]
    fn an_empty_library_says_so() {
        let mut runner = AppRunner::new(Books::empty());
        let screen = shown(runner.start());
        let text = words(&screen);
        assert!(
            text.contains("No books yet"),
            "an empty library did not say it was empty: {text}"
        );
        assert!(
            portrait_tiles(&screen).is_empty(),
            "an empty library still drew a shelf"
        );
    }

    #[test]
    fn a_card_listing_puts_each_title_on_the_shelf_it_opens_from() {
        let mut runner = AppRunner::new(Books::empty());
        runner.start();
        let home = shown(runner.device_result(DeviceResult::Library {
            entries: vec![
                entry("n/uuid-piranesi", "Piranesi", 1, 0, false),
                entry("0/notes.md", "Notes on a Rainy Afternoon", 2, 80, true),
            ],
            truncated: false,
        }));
        let text = words(&home);
        assert!(text.contains("Notes on a Rainy Afternoon"), "{text}");
        assert!(text.contains("Kobo reader"), "{text}");
        assert!(
            !text.contains("The Open Sea"),
            "the fixture shelf came back instead of the card"
        );
        let elsewhere = words(&shown(runner.action(action_id("shelf-kobo-reader"))));
        assert!(elsewhere.contains("Piranesi"), "{elsewhere}");
        assert!(elsewhere.contains("Not downloaded"), "{elsewhere}");
    }

    #[test]
    fn a_store_title_that_is_not_on_the_card_says_so() {
        let mut runner = AppRunner::new(Books::empty());
        runner.start();
        runner.device_result(DeviceResult::Library {
            entries: vec![entry("n/uuid-piranesi", "Piranesi", 1, 0, false)],
            truncated: false,
        });
        let screen = shown(runner.action(action_id("book-0")));
        let text = words(&screen);
        assert!(
            text.contains("not downloaded yet") && text.contains("Kobo reader"),
            "opening an undownloaded title did not say why: {text}"
        );
        assert!(
            matches!(runner.app().view, View::Unreadable(_)),
            "an undownloaded title was opened as if the file were here"
        );
    }

    #[test]
    fn the_library_shelf_is_portrait_covers_of_what_opens_here() {
        let mut runner = AppRunner::new(Books::seeded());
        runner.start();
        let screen = shown(runner.action(action_id("shelf-library")));
        let tiles = portrait_tiles(&screen);
        let here = fixtures()
            .iter()
            .filter(|document| document.belongs_here())
            .count();
        assert_eq!(tiles.len(), here.min(SHELF_PAGE));
        assert!(
            tiles.iter().all(|tile| tile.picture.is_some()),
            "a document was left on a generic glyph"
        );
        let text = words(&screen);
        assert!(text.contains("The Open Sea"), "{text}");
        assert!(text.contains("Notes on a Rainy Afternoon"), "{text}");
        assert!(
            !text.contains("Timetable"),
            "a PDF is on the Kobo reader's shelf"
        );
        let elsewhere = words(&shown(runner.action(action_id("shelf-kobo-reader"))));
        assert!(elsewhere.contains("Timetable"), "{elsewhere}");
    }

    #[test]
    fn a_coverless_tile_is_the_first_lines_not_a_glyph() {
        let notes = fixtures()
            .into_iter()
            .find(|document| document.kind == Kind::Markdown)
            .expect("a markdown document");
        let grey = document_preview(&notes.preview, notes.kind.badge(), 180, 280);
        assert!(
            grey.iter().any(|pixel| *pixel < 40),
            "the first lines left no ink"
        );
        assert!(
            notes.preview.contains("The rain started"),
            "the preview was not the document"
        );
        assert_eq!(notes.kind.badge(), "md");
    }

    #[test]
    fn a_book_on_the_panel_is_read_once_for_its_cover_name_and_author_and_they_are_kept() {
        let book = a_book_with_a_cover();
        let size = u32::try_from(book.len()).expect("a small book");
        let mut runner = AppRunner::new(Books::empty());
        runner.start();
        answer_start(&mut runner, None, None, None);
        let asked = runner.device_result(DeviceResult::Library {
            entries: vec![entry("0/sea.epub", "sea", 1, size, true)],
            truncated: false,
        });
        assert!(
            asked.iter().any(|command| matches!(
                command,
                Command::Device(DeviceRequest::ReadLibrary { id }) if id == "0/sea.epub"
            )),
            "the book on the panel was not read for its cover"
        );
        let commands = runner.device_result(DeviceResult::LibraryDocument {
            id: "0/sea.epub".to_owned(),
            bytes: book,
        });
        let key = covers::key("0/sea.epub", size, "sea");
        let kept = saved(&commands, &key).expect("the cover was kept in the store");
        assert!(
            covers::from_png(&kept).is_some(),
            "the kept cover reads back"
        );
        let text = words(&shown(commands.clone()));
        assert!(text.contains("The Open Sea"), "the package's title: {text}");
        assert!(text.contains("A. Mariner"), "the package's author: {text}");

        // What was learned is written down once nothing else is waiting to
        // be read, so the next start does not read the book again.
        let record = saved(&commands, DESCRIBED).expect("the record of described books");
        let mut runner = AppRunner::new(Books::empty());
        runner.start();
        answer_start(&mut runner, None, Some(record), None);
        let asked = runner.device_result(DeviceResult::Library {
            entries: vec![entry("0/sea.epub", "sea", 1, size, true)],
            truncated: false,
        });
        assert!(
            !asked.iter().any(|command| matches!(
                command,
                Command::Device(DeviceRequest::ReadLibrary { .. })
            )),
            "a described book was read again"
        );
        assert!(
            asked.iter().any(|command| matches!(
                command,
                Command::Store(StoreRequest::Load { key: asked }) if *asked == key
            )),
            "the kept cover was not asked for"
        );
    }

    #[test]
    fn a_book_replaced_by_another_of_the_same_size_is_read_again() {
        let mut described = std::collections::BTreeMap::new();
        described.insert(
            "0/book.epub".to_owned(),
            covers::Described {
                bytes: 4096,
                listed: "The First Book".to_owned(),
                title: Some("The First Book".to_owned()),
                author: None,
                cover: covers::Cover::Missing,
            },
        );
        let mut runner = AppRunner::new(Books::empty());
        runner.start();
        answer_start(
            &mut runner,
            None,
            covers::encode_described(&described, kobo_sdk::MAX_STORE_VALUE)
                .map(|mut parts| parts.remove(0)),
            None,
        );
        let asked = runner.device_result(DeviceResult::Library {
            entries: vec![entry("0/book.epub", "The Second Book", 1, 4096, true)],
            truncated: false,
        });
        assert!(
            asked.iter().any(|command| matches!(
                command,
                Command::Device(DeviceRequest::ReadLibrary { id }) if id == "0/book.epub"
            )),
            "a different book at the same path and size kept the first one's record"
        );
        assert_eq!(runner.app().documents[0].title, "The Second Book");
    }

    /// A book named in a script no installed face carries is still shelved.
    ///
    /// The name is the file's, not the application's, so the check that
    /// refuses a symbol an application chose for itself has no business
    /// refusing it: the shelf shows the letters it cannot draw as boxes, as
    /// the page does, and every other book on it stays in reach.
    #[test]
    fn a_book_named_in_a_script_no_face_carries_is_still_shelved() {
        let mut runner = AppRunner::new(Books::empty());
        runner.start();
        answer_start(&mut runner, None, None, None);
        assert!(
            kobo_ui::undrawable_in("\u{5d7}", kobo_ui::Face::Text).is_some(),
            "the face can draw Hebrew now, so this no longer tests anything"
        );
        runner.device_result(DeviceResult::Library {
            entries: vec![
                entry("0/whale.epub", "\u{5d7}\u{5d5}", 1, 4096, true),
                entry("0/pride.epub", "Pride and Prejudice", 1, 4096, true),
            ],
            truncated: false,
        });
        let titles: Vec<&str> = runner
            .app()
            .documents
            .iter()
            .map(|document| document.title.as_str())
            .collect();
        assert!(titles.contains(&"\u{25a1}\u{25a1}"), "{titles:?}");
        assert!(titles.contains(&"Pride and Prejudice"), "{titles:?}");
    }

    #[test]
    fn an_epub_opens_in_the_shared_reader() {
        let mut runner = AppRunner::new(Books::seeded());
        runner.start();
        let screen = open(&mut runner, Kind::Epub);
        assert_eq!(runner.app().view, View::Reading);
        assert!(
            runner.app().book.is_open(),
            "the EPUB tap did not hold a reader"
        );
        let text = words(&screen);
        assert!(
            text.contains("The tide was already turning"),
            "the shared reader did not show the EPUB: {text}"
        );
        assert!(
            portrait_tiles(&screen).is_empty(),
            "the EPUB tap left the shelf on the panel"
        );
        assert!(screen.validate(&CLARA_BW_METRICS).is_empty());
    }

    #[test]
    fn markdown_opens_in_the_shared_reader() {
        let mut runner = AppRunner::new(Books::seeded());
        runner.start();
        let screen = open(&mut runner, Kind::Markdown);
        assert_eq!(runner.app().view, View::Reading);
        assert!(runner.app().book.is_open());
        let text = words(&screen);
        assert!(
            text.contains("The rain started"),
            "the shared reader did not show the Markdown: {text}"
        );
    }

    #[test]
    fn plain_text_opens_in_the_shared_reader() {
        let mut runner = AppRunner::new(Books::seeded());
        runner.start();
        let screen = open(&mut runner, Kind::Text);
        assert_eq!(runner.app().view, View::Reading);
        assert!(runner.app().book.is_open());
        let text = words(&screen);
        assert!(
            text.contains("Dear M"),
            "the shared reader did not show the text file: {text}"
        );
    }

    #[test]
    fn html_opens_in_the_shared_reader() {
        let mut runner = AppRunner::new(Books::seeded());
        runner.start();
        let screen = open(&mut runner, Kind::Html);
        assert_eq!(runner.app().view, View::Reading);
        assert!(runner.app().book.is_open());
        let text = words(&screen);
        assert!(
            text.contains("Market Street"),
            "the shared reader did not show the HTML: {text}"
        );
    }

    /// A file called .epub that is not a zip is a download cut short or a
    /// file renamed by mistake. It says it could not be opened rather than
    /// showing its bytes set as prose.
    #[test]
    fn an_epub_that_is_not_a_zip_says_it_could_not_be_opened() {
        let mut runner = AppRunner::new(Books::empty());
        runner.start();
        answer_start(&mut runner, None, None, None);
        let bytes = b"Chapter one\nIt was not a book at all.".to_vec();
        let size = u32::try_from(bytes.len()).expect("a small file");
        let _listed = runner.device_result(DeviceResult::Library {
            entries: vec![entry("0/odd.epub", "odd", 1, size, true)],
            truncated: false,
        });
        let _described = runner.device_result(DeviceResult::LibraryDocument {
            id: "0/odd.epub".to_owned(),
            bytes: bytes.clone(),
        });
        let _opening = runner.action(action_id("book-0"));
        let screen = shown(runner.device_result(DeviceResult::LibraryDocument {
            id: "0/odd.epub".to_owned(),
            bytes,
        }));
        assert_eq!(runner.app().view, View::Unreadable(0));
        let text = words(&screen);
        assert!(text.contains("could not be opened"), "{text}");
        assert!(!text.contains("not a book at all"), "{text}");
    }

    /// Back to the book, from the marking list, puts the controls away too,
    /// so the next page key turns the page. It used to come back with the
    /// Aa panel open over the page.
    #[test]
    fn back_to_the_book_from_the_marking_list_leaves_the_page_bare() {
        let mut runner = AppRunner::with_metrics(Books::seeded(), CLARA_BW_METRICS);
        runner.start();
        let _reading = open(&mut runner, Kind::Epub);
        let _controls = runner.action(action_id(kobo_read::action::CONTROLS));
        let _marking = runner.action(action_id(kobo_read::action::MARKING));
        assert_eq!(
            runner.app().book.reader().map(kobo_read::Reader::chrome),
            Some(kobo_read::Chrome::Marking)
        );
        // The list's own Back to the book, which the shared reader names
        // apart from the control that opens the Aa panel.
        let _back = runner.action(action_id(kobo_read::action::BOOK));
        assert_eq!(
            runner.app().book.reader().map(kobo_read::Reader::chrome),
            Some(kobo_read::Chrome::Hidden),
            "Back to the book opened the controls"
        );
    }

    /// The first word the page would report a held finger on.
    fn a_word_on(screen: &Screen) -> kobo_sdk::TextHit {
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true));
        (0..CLARA_BW_METRICS.height)
            .step_by(8)
            .find_map(|y| layout.hit_text(CLARA_BW_METRICS.width / 4, y))
            .expect("a page of prose has a word under a finger")
    }

    #[test]
    fn holding_a_word_in_a_book_looks_it_up_and_a_highlight_made_there_is_kept_with_the_place() {
        let mut runner = started_with(None, None);
        let page = shown(runner.action(action_id("book-0")));
        let commands = runner.text_hold(action_id(kobo_read::action::MARKING), a_word_on(&page));
        let asked = commands
            .iter()
            .find_map(|command| match command {
                Command::Device(DeviceRequest::LookupWord { word, .. }) => Some(word.clone()),
                _ => None,
            })
            .expect("the held word was asked about");
        assert!(
            asked.chars().all(char::is_alphanumeric),
            "a whole word and nothing round it is looked up, not {asked:?}"
        );
        assert!(words(&shown(commands)).contains("Looking up"));

        // The light reading asked for when the book opened is answered first,
        // as the runtime answers in the order it was asked.
        runner.device_result(DeviceResult::Frontlight { percent: 20 });
        let defined = runner.device_result(DeviceResult::Dictionary {
            word: asked.clone(),
            entries: vec![kobo_sdk::DictionaryEntry {
                dictionary: "Pocket English".into(),
                language: "en".into(),
                headword: asked.clone(),
                definition: "What the word means.".into(),
            }],
        });
        let definition = shown(defined);
        assert!(words(&definition).contains("What the word means."));
        assert!(definition.validate(&CLARA_BW_METRICS).is_empty());

        let kept = runner.action(action_id(kobo_read::action::WORD_HIGHLIGHT));
        assert!(
            saved(&kept, READING).is_some(),
            "the highlight was not written down with the place"
        );
        let memory = runner.app().book.memory().cloned().expect("a book is open");
        assert_eq!(memory.annotations.len(), 1);
        assert_eq!(runner.app().view, View::Reading);
    }

    #[test]
    fn a_word_no_dictionary_has_says_how_to_add_one_and_back_returns_to_the_page() {
        let mut runner = started_with(None, None);
        let page = shown(runner.action(action_id("book-0")));
        runner.text_hold(action_id(kobo_read::action::MARKING), a_word_on(&page));
        runner.device_result(DeviceResult::Frontlight { percent: 20 });
        let missing =
            shown(runner.device_result(DeviceResult::Denied(kobo_sdk::DenyReason::Unsupported)));
        let text = words(&missing);
        assert!(text.contains("No dictionary on this reader has this word."));
        assert!(text.contains("kobo dict install"));
        assert!(missing.validate(&CLARA_BW_METRICS).is_empty());

        runner.action(ActionId::BACK);
        assert_eq!(
            runner.app().view,
            View::Reading,
            "Back from a definition closed the book"
        );
        assert_eq!(
            runner.app().book.reader().map(kobo_read::Reader::chrome),
            Some(kobo_read::Chrome::Hidden)
        );
    }

    /// Lets every task the application has asked for run to its end, in the
    /// order it asked, and gives back the last screen that came of it.
    fn run_tasks(runner: &mut AppRunner<Books>, mut commands: Vec<Command>) -> Option<Screen> {
        let mut last = None;
        for _ in 0..10_000 {
            let tasks: Vec<TaskId> = commands
                .iter()
                .filter_map(|command| match command {
                    Command::Spawn { task, .. } => Some(*task),
                    _ => None,
                })
                .collect();
            if tasks.is_empty() {
                return last;
            }
            commands = Vec::new();
            for task in tasks {
                let answered = runner.task_outcome(task, TaskOutcome::Completed(Vec::new()));
                if answered
                    .iter()
                    .any(|command| matches!(command, Command::SetScreen(_)))
                {
                    last = Some(shown(answered.clone()));
                }
                commands.extend(answered);
            }
        }
        panic!("the application never stopped asking for more time");
    }

    #[test]
    fn finding_words_in_a_book_lists_where_they_are_and_back_returns_from_one_of_them() {
        let mut runner = started_with(None, None);
        runner.action(action_id("book-0"));
        runner.page_turn(true);
        runner.store_result(StoreResult::Saved {
            key: READING.into(),
        });
        let left = runner
            .app()
            .book
            .reader()
            .map(kobo_read::Reader::page_number);

        runner.action(action_id(kobo_read::action::CONTROLS));
        let field = shown(runner.action(action_id(kobo_read::action::FIND)));
        assert!(words(&field).contains("Find"));
        let searching = runner.type_text("Line 250 ");
        assert!(
            words(&shown(searching.clone())).contains("Searching for"),
            "the book is read through after the screen says so, not before"
        );
        let found = run_tasks(&mut runner, searching).expect("the results were drawn");
        let text = words(&found);
        assert!(text.contains("is in 1 passage."), "{text}");
        assert!(text.contains("Line 250 of a letter"), "{text}");
        assert!(found.validate(&CLARA_BW_METRICS).is_empty());

        // The words found are marked on the page, so they are set as a run
        // of their own and the line is looked for by what follows them.
        let kept = runner.action(action_id("reader-found-0"));
        let page = words(&shown(kept.clone()));
        assert!(page.contains("250") && page.contains("Line 251 of a letter"));
        assert!(
            saved(&kept, READING).is_some(),
            "the place the search led to was not written down"
        );
        runner.action(ActionId::BACK);
        assert_eq!(
            runner.app().view,
            View::Reading,
            "Back from a place found closed the book"
        );
        assert_eq!(
            runner
                .app()
                .book
                .reader()
                .map(kobo_read::Reader::page_number),
            left,
            "Back did not return to the page the search was made from"
        );
    }

    #[test]
    fn a_page_typed_into_go_to_is_turned_to_and_kept_and_back_returns_from_it() {
        let mut runner = started_with(None, None);
        runner.action(action_id("book-0"));
        let pages = runner
            .app()
            .book
            .reader()
            .map(kobo_read::Reader::page_count)
            .expect("a book is open");
        assert!(pages > 6, "a letter of several pages");

        runner.action(action_id(kobo_read::action::CONTROLS));
        let screen = shown(runner.action(action_id(kobo_read::action::GO_TO)));
        assert!(words(&screen).contains(&format!("You are on page 1 of {pages}.")));
        assert!(screen.validate(&CLARA_BW_METRICS).is_empty());

        runner.action(action_id(kobo_read::action::GO_TO_TYPE));
        let refused = shown(runner.type_text(&format!("{}", pages + 1)));
        assert!(
            words(&refused).contains(&format!("The pages run from 1 to {pages}.")),
            "a page the book does not have was not said to be one"
        );
        assert_eq!(
            runner
                .app()
                .book
                .reader()
                .map(kobo_read::Reader::page_number),
            Some(1)
        );

        runner.action(action_id(kobo_read::action::GO_TO_TYPE));
        let kept = runner.type_text("5");
        assert_eq!(
            runner
                .app()
                .book
                .reader()
                .map(kobo_read::Reader::page_number),
            Some(5)
        );
        assert!(
            saved(&kept, READING).is_some(),
            "the page gone to was not written down as the place"
        );
        runner.action(ActionId::BACK);
        assert_eq!(runner.app().view, View::Reading);
        assert_eq!(
            runner
                .app()
                .book
                .reader()
                .map(kobo_read::Reader::page_number),
            Some(1),
            "Back did not return to the page that was left"
        );
    }

    /// Answers every storage request in `commands`, and in whatever those
    /// answers lead to, from a store and a shelf kept in `folder`, the way
    /// the runtime would. Gives back the last screen that came of it.
    fn store_everything(
        runner: &mut AppRunner<Books>,
        commands: Vec<Command>,
        folder: &std::path::Path,
    ) -> Option<Screen> {
        use kobo_sdk::permissions::{shelf::Shelf, store::Store};
        let shelf = Shelf::new(folder.join("data"));
        let store = Store::new(folder.join("state"));
        let mut waiting: std::collections::VecDeque<Command> = commands.into();
        let mut last = None;
        while let Some(command) = waiting.pop_front() {
            match command {
                Command::SetScreen(screen) => last = Some(screen),
                Command::Store(request) => {
                    let result = shelf
                        .handle(&request)
                        .unwrap_or_else(|| store.handle(&request));
                    waiting.extend(runner.store_result(result));
                }
                _ => {}
            }
        }
        last
    }

    #[test]
    fn a_copy_of_a_books_notes_is_offered_to_a_computer_as_markdown_once_the_reader_has_kept_it() {
        let folder = std::env::temp_dir().join(format!("books-notes-copy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).expect("a folder to keep the copy in");

        let mut runner = started_with(None, None);
        let opened = runner.action(action_id("book-0"));
        store_everything(&mut runner, opened, &folder);
        // With nothing kept there is nothing to copy, and no control for it.
        runner.action(action_id(kobo_read::action::CONTROLS));
        let empty = shown(runner.action(action_id(kobo_read::action::HIGHLIGHTS)));
        let offers = |screen: &Screen| {
            screen.top_bar.as_ref().is_some_and(|bar| {
                bar.actions
                    .iter()
                    .any(|action| action.label == "Save a copy")
            })
        };
        assert!(
            !offers(&empty),
            "an empty notes list offered a copy of itself"
        );

        runner.action(action_id(kobo_read::action::MARKING));
        let marked = runner.action(action_id("reader-mark-0"));
        store_everything(&mut runner, marked, &folder);
        let notes = shown(runner.action(action_id(kobo_read::action::HIGHLIGHTS)));
        assert!(offers(&notes), "the notes did not offer a copy");
        assert!(notes.validate(&CLARA_BW_METRICS).is_empty());

        let asked = runner.action(action_id(kobo_read::action::SAVE_COPY));
        assert!(
            words(&shown(asked.clone())).contains("Saving and checking the copy"),
            "the copy was not begun as soon as it was asked for"
        );
        let ready = store_everything(&mut runner, asked, &folder).expect("the copy's screen");
        let text = words(&ready);
        assert!(text.contains("A Long Letter notes"), "{text}");
        assert!(text.contains("Ready for your computer."), "{text}");

        let offer = kobo_sdk::exports::Offer::restore(
            &std::fs::read(folder.join("state").join(kobo_sdk::exports::OFFER_KEY))
                .expect("an offer was written"),
        )
        .expect("a readable offer");
        assert_eq!(offer.title, "A Long Letter notes");
        assert_eq!(offer.format, kobo_sdk::exports::Format::Markdown);
        let copy = std::fs::read_to_string(folder.join("data").join(&offer.digest))
            .expect("the copy is on the shelf");
        assert_eq!(
            copy,
            "# A Long Letter\n\n> Line 1 of a letter that runs on for page after page.\n"
        );

        // Back from the copy returns to the notes, not out of the book.
        let back = shown(runner.action(ActionId::BACK));
        assert!(words(&back).contains("Marked passages"));
        assert_eq!(runner.app().view, View::Reading);
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_pdf_is_listed_and_will_not_open() {
        let mut runner = AppRunner::new(Books::seeded());
        runner.start();
        runner.action(action_id("shelf-kobo-reader"));
        let screen = open(&mut runner, Kind::Pdf);
        let text = words(&screen);
        assert!(
            text.contains("PDFs open in the Kobo reader"),
            "opening a PDF did not say where to read it: {text}"
        );
        assert!(
            matches!(runner.app().view, View::Unreadable(_)),
            "a PDF tap left the shelf as if nothing happened"
        );
        assert!(
            !runner.app().book.is_open(),
            "a PDF was opened in the reader"
        );
        runner.action(ActionId::BACK);
        assert_eq!(
            runner.app().view,
            View::Shelf(super::Shelf::KoboReader),
            "the bar's back arrow did not return to the shelf it came from"
        );
    }

    #[test]
    fn the_two_limits_are_the_wires_and_sit_inside_what_the_runtime_lists() {
        assert_eq!(
            usize::try_from(super::LARGEST_WHOLE).ok(),
            Some(kobo_sdk::MAX_LIBRARY_DOCUMENT_BYTES),
            "a book this size is asked for whole, and the runtime must agree it can be"
        );
        assert!(
            u64::from(super::LARGEST_OPENABLE)
                <= kobo_sdk::permissions::library::MAX_DOCUMENT_BYTES,
            "Books would offer to open a book the runtime never lists"
        );
    }

    #[test]
    fn a_listing_with_a_large_book_asks_the_runtime_which_release_it_is_and_nothing_else() {
        let (mut runner, asked) = started_with_a_large_book();
        assert_eq!(requests(&asked), [DeviceRequest::ReadIdentity]);
        // A second listing does not ask a second time.
        let again = runner.device_result(this_runtime());
        assert!(!requests(&again).contains(&DeviceRequest::ReadIdentity));

        // A library with nothing that large never asks at all.
        let mut runner = AppRunner::new(Books::empty());
        runner.start();
        answer_start(&mut runner, None, None, None);
        let asked = runner.device_result(DeviceResult::Library {
            entries: vec![entry("0/notes.md", "Notes", 2, 80, true)],
            truncated: false,
        });
        assert!(requests(&asked).is_empty(), "{:?}", requests(&asked));
    }

    #[test]
    fn a_large_book_arrives_in_ranges_with_its_progress_on_the_panel_and_opens() {
        let book = a_book_with_a_cover_and(700_000);
        let (mut runner, _asked) = started_with_a_large_book();
        // The shelf reads the book for its cover first, in ranges as well.
        let answered = runner_identity(&mut runner);
        let described = serve(&mut runner, answered, &mut |request| {
            super::ranges::tests::answer(request, &book, 1)
        });
        assert_eq!(ranges_asked(&described), [0, 262_144, 524_288]);
        assert!(
            words(&shown(described)).contains("The Open Sea"),
            "the package's title was not read out of a book that arrived in ranges"
        );

        let opening = runner.action(action_id("book-0"));
        assert_eq!(
            ranges_asked(&opening),
            [0],
            "one range is asked for at a time"
        );
        assert!(
            !requests(&opening)
                .iter()
                .any(|request| matches!(request, DeviceRequest::ReadLibrary { .. })),
            "a book too large for one answer was asked for whole"
        );
        let first = shown(opening.clone());
        let text = words(&first);
        assert!(text.contains("Opening"), "{text}");
        assert!(text.contains("The Open Sea"), "{text}");
        assert!(text.contains("Cancel"), "{text}");
        assert!(first.validate(&CLARA_BW_METRICS).is_empty());

        let rest = serve(&mut runner, opening, &mut |request| {
            super::ranges::tests::answer(request, &book, 1)
        });
        assert_eq!(ranges_asked(&rest), [0, 262_144, 524_288]);
        let screens = rest
            .iter()
            .filter(|command| matches!(command, Command::SetScreen(_)))
            .count();
        assert!(
            (2..=12).contains(&screens),
            "{screens} screens for three pieces: progress is drawn by the tenth, not by the piece"
        );
        assert_eq!(runner.app().view, View::Reading);
        assert!(runner.app().fetching.is_none(), "the file was kept in hand");
        let page = words(&shown(rest));
        assert!(page.contains("The tide was turning"), "{page}");
    }

    #[test]
    fn under_another_release_of_the_runtime_books_reads_whole_or_not_at_all() {
        for other in [
            runtime("0.0.1"),
            DeviceResult::Denied(kobo_sdk::DenyReason::Unsupported),
            DeviceResult::Failed(kobo_sdk::DeviceError::Backend),
        ] {
            let small = a_book_with_a_cover();
            let size = u32::try_from(small.len()).expect("a small book");
            let mut runner = AppRunner::new(Books::empty());
            runner.start();
            answer_start(&mut runner, None, None, None);
            let asked = runner.device_result(DeviceResult::Library {
                entries: vec![
                    entry(ATLAS, "Atlas", 1, LISTED_LARGE, true),
                    entry("0/sea.epub", "sea", 1, size, true),
                ],
                truncated: false,
            });
            // The small book is read for its cover the way it always was,
            // and that request is ahead of the question in the queue.
            assert_eq!(
                requests(&asked),
                [
                    DeviceRequest::ReadIdentity,
                    DeviceRequest::ReadLibrary {
                        id: "0/sea.epub".to_owned()
                    }
                ]
            );
            let mut said = runner.device_result(other.clone());
            said.extend(runner.device_result(DeviceResult::LibraryDocument {
                id: "0/sea.epub".to_owned(),
                bytes: small.clone(),
            }));
            let shelf = words(&shown(runner.action(action_id("shelf-library"))));
            assert!(
                shelf.contains("Too large to open here"),
                "{other:?}: {shelf}"
            );

            let atlas = runner
                .app()
                .documents
                .iter()
                .position(|document| document.id == ATLAS)
                .expect("listed");
            let tapped = runner.action(action_id(&format!("book-{atlas}")));
            let text = words(&shown(tapped.clone()));
            assert!(text.contains("6.0 MB"), "{other:?}: {text}");
            assert!(text.contains("up to 4 MB"), "{other:?}: {text}");
            said.extend(tapped);
            runner.action(ActionId::BACK);

            // A book that fits one answer still opens, whole.
            let sea = runner
                .app()
                .documents
                .iter()
                .position(|document| document.id == "0/sea.epub")
                .expect("listed");
            let opening = runner.action(action_id(&format!("book-{sea}")));
            assert_eq!(
                requests(&opening),
                [DeviceRequest::ReadLibrary {
                    id: "0/sea.epub".to_owned()
                }]
            );
            said.extend(runner.device_result(DeviceResult::LibraryDocument {
                id: "0/sea.epub".to_owned(),
                bytes: small,
            }));
            assert_eq!(runner.app().view, View::Reading, "{other:?}");
            assert!(
                ranges_asked(&said).is_empty(),
                "{other:?}: a range was asked of a runtime that would end the session over it"
            );
        }
    }

    #[test]
    fn a_large_book_tapped_before_the_runtime_has_answered_opens_when_it_does() {
        let book = a_book_with_a_cover_and(300_000);
        let (mut runner, _asked) = started_with_a_large_book();
        let tapped = runner.action(action_id("book-0"));
        assert!(
            requests(&tapped).is_empty(),
            "asked for a range before knowing the runtime reads the request"
        );
        assert!(matches!(runner.app().view, View::Opening(_)));
        let answered = runner_identity(&mut runner);
        let opened = serve(&mut runner, answered, &mut |request| {
            super::ranges::tests::answer(request, &book, 1)
        });
        assert_eq!(ranges_asked(&opened), [0, 262_144]);
        assert_eq!(runner.app().view, View::Reading);

        // And under another release the same tap ends at the note.
        let (mut runner, _asked) = started_with_a_large_book();
        runner.action(action_id("book-0"));
        let text = words(&shown(runner.device_result(runtime("0.0.1"))));
        assert!(text.contains("up to 4 MB"), "{text}");
        assert!(matches!(runner.app().view, View::Unreadable(_)));
    }

    #[test]
    fn a_book_replaced_on_the_card_while_it_arrives_says_so_and_opens_on_a_second_try() {
        let first = a_book_with_a_cover_and(700_000);
        let second = a_book_with_a_cover_and(900_000);
        let (mut runner, _asked) = started_with_a_large_book();
        // Described under the first copy, so the shelf is quiet afterwards.
        let answered = runner_identity(&mut runner);
        serve(&mut runner, answered, &mut |request| {
            super::ranges::tests::answer(request, &first, 1)
        });
        let opening = runner.action(action_id("book-0"));
        let mut asked = 0;
        let failed = serve(&mut runner, opening, &mut |request| {
            asked += 1;
            if asked == 1 {
                super::ranges::tests::answer(request, &first, 1)
            } else {
                super::ranges::tests::answer(request, &second, 2)
            }
        });
        assert_eq!(asked, 2, "kept asking after the file changed");
        assert!(matches!(runner.app().view, View::Unreadable(_)));
        assert!(runner.app().fetching.is_none());
        let note = shown(failed);
        let text = words(&note);
        assert!(text.contains("changed on the card"), "{text}");
        assert!(text.contains("Open again"), "{text}");
        assert!(!text.contains("damaged"), "{text}");
        assert!(note.validate(&CLARA_BW_METRICS).is_empty());

        let again = runner.action(action_id(super::OPEN_AGAIN));
        assert_eq!(
            ranges_asked(&again),
            [0],
            "the second try began at the start"
        );
        let opened = serve(&mut runner, again, &mut |request| {
            super::ranges::tests::answer(request, &second, 2)
        });
        assert_eq!(runner.app().view, View::Reading);
        assert!(words(&shown(opened)).contains("The tide was turning"));
    }

    #[test]
    fn giving_up_on_a_large_book_returns_to_its_shelf_and_stops_asking() {
        let book = a_book_with_a_cover_and(700_000);
        for leave in [action_id(super::CANCEL_OPEN), ActionId::BACK] {
            let (mut runner, _asked) = started_with_a_large_book();
            let answered = runner_identity(&mut runner);
            serve(&mut runner, answered, &mut |request| {
                super::ranges::tests::answer(request, &book, 1)
            });
            runner.action(action_id("shelf-library"));
            let opening = runner.action(action_id("book-0"));
            let first = requests(&opening).remove(0);
            let left = runner.action(leave);
            assert_eq!(runner.app().view, View::Shelf(Shelf::Library));
            assert!(words(&shown(left)).contains("The Open Sea"));
            // The range that was already on its way lands on nothing.
            let late = runner.device_result(super::ranges::tests::answer(&first, &book, 1));
            assert!(ranges_asked(&late).is_empty(), "asked on after giving up");
            assert!(runner.app().fetching.is_none());
        }
    }

    #[test]
    fn a_file_that_turns_out_longer_than_books_opens_says_how_long_it_really_is() {
        let (mut runner, _asked) = started_with_a_large_book();
        let waiting = runner_identity(&mut runner);
        // The shelf's own read for a cover is told the same thing.
        let huge = |request: &DeviceRequest| {
            let DeviceRequest::ReadLibraryRange { id, offset, .. } = request else {
                panic!("not a range request");
            };
            DeviceResult::LibraryRange {
                id: id.clone(),
                offset: *offset,
                size: 40 * 1024 * 1024,
                stamp: 3,
                bytes: vec![0x50; 262_144],
            }
        };
        let described = serve(&mut runner, waiting, &mut |request| huge(request));
        assert_eq!(
            ranges_asked(&described),
            [0],
            "asked on for a book it will not hold"
        );
        let shelf = words(&shown(described));
        assert!(shelf.contains("Too large to open here"), "{shelf}");
        let text = words(&shown(runner.action(action_id("book-0"))));
        assert!(text.contains("up to 32 MB"), "{text}");
        assert!(text.contains("40.0 MB"), "{text}");
    }

    #[test]
    fn a_large_book_being_read_for_its_cover_gives_way_to_the_one_being_opened() {
        let book = a_book_with_a_cover_and(700_000);
        let (mut runner, _asked) = started_with_a_large_book();
        let describing = runner_identity(&mut runner);
        assert_eq!(ranges_asked(&describing), [0]);
        let stale = requests(&describing).remove(0);

        // Tapped while its cover is one range in.
        let opening = runner.action(action_id("book-0"));
        assert_eq!(ranges_asked(&opening), [0]);
        assert!(runner
            .app()
            .fetching
            .as_ref()
            .is_some_and(|fetch| fetch.opening));
        assert_eq!(runner.app().describing, None);
        assert_eq!(runner.app().wanted.front().map(String::as_str), Some(ATLAS));

        // Both first ranges are answered, the abandoned one ahead of the
        // other as the runtime would. They are the same piece of the same
        // file, so the first to land is kept, the second is let go, and one
        // book comes of it for one round trip more than it needed.
        let mut outstanding = vec![Command::Device(stale)];
        outstanding.extend(
            opening
                .into_iter()
                .filter(|command| matches!(command, Command::Device(_))),
        );
        let answers = serve(&mut runner, outstanding, &mut |request| {
            super::ranges::tests::answer(request, &book, 1)
        });
        assert_eq!(ranges_asked(&answers), [0, 0, 262_144, 524_288]);
        assert_eq!(runner.app().view, View::Reading);
        assert!(words(&shown(answers)).contains("The tide was turning"));
    }

    #[test]
    fn the_home_screen_is_tappable_on_a_clara() {
        let mut runner = AppRunner::new(Books::seeded());
        let screen = shown(runner.start());
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true));
        let controls: Vec<_> = layout
            .nodes
            .iter()
            .filter(|node| node.kind.acts_on().is_some())
            .collect();
        assert!(
            !controls.is_empty(),
            "the home screen offered nothing a finger could reach"
        );
        for control in &controls {
            assert!(
                control.rect.width >= CLARA_BW_METRICS.touch_target_minimum()
                    && control.rect.height >= CLARA_BW_METRICS.touch_target_minimum(),
                "{:?} is smaller than a touch target",
                control.kind
            );
        }
        assert!(
            layout
                .nodes
                .iter()
                .any(|node| matches!(node.kind, LayoutKind::FramedPicture(_))),
            "the home screen drew glyphs where the covers should be"
        );
    }

    #[test]
    fn the_book_last_read_is_offered_to_continue_at_its_page() {
        let mut runner = started_with(None, None);
        let first = words(&shown(runner.action(action_id("shelf-library"))));
        assert!(!first.contains("Continue reading"), "{first}");
        runner.action(action_id("book-0"));
        for _ in 0..3 {
            runner.page_turn(true);
            runner.store_result(StoreResult::Saved {
                key: READING.into(),
            });
        }
        let commands = runner.action(ActionId::BACK);
        assert_eq!(
            runner.app().view,
            View::Shelf(super::Shelf::Library),
            "closing a book returns to the shelf it was opened from"
        );
        let recent = saved(&commands, RECENT).expect("the book was written down as read");
        let home = shown(runner.action(ActionId::BACK));
        let text = words(&home);
        assert!(text.contains("Continue reading"), "{text}");
        assert!(text.contains("Page 4 of"), "{text}");
        let problems = home.validate(&CLARA_BW_METRICS);
        assert!(problems.is_empty(), "{problems:?}");

        // After a restart the same book is offered, and the button opens it
        // at the page it was left on.
        let record = saved(&commands, READING);
        let mut runner = started_with(record, Some(recent));
        runner.action(action_id(CONTINUE));
        assert_eq!(runner.app().view, View::Reading);
    }

    #[test]
    fn a_book_reopens_at_the_page_and_text_size_it_was_closed_at() {
        let mut runner = started_with(None, None);
        runner.action(action_id("book-0"));
        assert_eq!(runner.app().view, View::Reading);
        let mut record = None;
        for _ in 0..3 {
            record = saved(&runner.page_turn(true), READING).or(record);
            runner.store_result(StoreResult::Saved {
                key: READING.into(),
            });
        }
        record = saved(
            &runner.action(action_id(kobo_read::action::LARGER)),
            READING,
        )
        .or(record);
        runner.store_result(StoreResult::Saved {
            key: READING.into(),
        });
        let left = runner.app().book.memory().cloned().expect("a book is open");
        assert!(left.at > 0, "three page turns left the book at its start");
        assert_ne!(left.scale, kobo_read::Memory::default().scale);
        record = saved(&runner.action(ActionId::BACK), READING).or(record);
        assert_eq!(runner.app().view, View::Home);
        let record = record.expect("closing a book wrote its place down");

        // Books restarts with only what the store kept.
        let mut runner = started_with(Some(record), None);
        runner.action(action_id("book-0"));
        let reopened = runner
            .app()
            .book
            .memory()
            .cloned()
            .expect("the book opened again");
        assert_eq!(reopened.at, left.at, "the book reopened at another page");
        assert_eq!(
            reopened.scale, left.scale,
            "the book reopened at another size"
        );
    }

    #[test]
    fn quick_page_turns_and_leaving_still_keep_the_last_page() {
        let mut runner = started_with(None, None);
        runner.action(action_id("book-0"));
        runner.page_turn(true);
        runner.page_turn(true);
        runner.lifecycle(Lifecycle::Background);
        let left = runner.app().book.memory().map(|memory| memory.at);
        // The second turn waited for the first write; its answer releases it.
        let commands = runner.store_result(StoreResult::Saved {
            key: READING.into(),
        });
        let record = saved(&commands, READING)
            .expect("the last page was written once the first write was kept");
        let mut runner = started_with(Some(record), None);
        runner.action(action_id("book-0"));
        assert_eq!(runner.app().book.memory().map(|memory| memory.at), left);
    }

    #[test]
    fn a_place_that_was_not_saved_offers_retry_saving_on_the_shelf_and_writes_it_again() {
        let mut runner = started_with(None, None);
        runner.action(action_id("book-0"));
        runner.page_turn(true);
        runner.store_result(StoreResult::Denied(kobo_sdk::StoreError::Unwritable));
        // Leaving the book tries once more; when that fails too, the shelf
        // is where the owner learns of it.
        runner.action(ActionId::BACK);
        let shelf =
            shown(runner.store_result(StoreResult::Denied(kobo_sdk::StoreError::Unwritable)));
        // Leaving also wrote down which book was open; that write is kept.
        runner.store_result(StoreResult::Saved { key: RECENT.into() });
        let drawn = words(&shelf);
        assert!(drawn.contains("could not save your place"), "{drawn}");
        assert!(drawn.contains("Retry saving"), "{drawn}");
        // The fix is on the banner that explains it, and the shelf still fits.
        let layout = shelf.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true));
        assert!(layout
            .nodes
            .iter()
            .any(|node| { node.kind == LayoutKind::BannerAction(action_id("retry-save")) }));
        let problems = shelf.validate(&CLARA_BW_METRICS);
        assert!(problems.is_empty(), "{problems:?}");
        let commands = runner.action(action_id("retry-save"));
        assert!(
            saved(&commands, READING).is_some(),
            "Retry saving writes the place now"
        );
        let retried = shown(commands);
        // Once the write is kept the panel does not offer it again. An answer
        // that changes nothing sends no new screen, so the last one shown is
        // what is on the panel.
        let kept = runner.store_result(StoreResult::Saved {
            key: READING.into(),
        });
        let on_panel = kept
            .into_iter()
            .rev()
            .find_map(|command| match command {
                Command::SetScreen(screen) => Some(screen),
                _ => None,
            })
            .unwrap_or(retried);
        assert!(!words(&on_panel).contains("Retry saving"));
    }

    #[test]
    fn books_that_open_here_come_before_those_that_open_in_the_kobo_reader() {
        let mut runner = AppRunner::new(Books::empty());
        runner.start();
        runner.device_result(DeviceResult::Library {
            entries: vec![
                entry("0/timetable.pdf", "Timetable", 5, 80, true),
                entry("n/uuid-piranesi", "Piranesi", 1, 0, false),
                entry("0/notes.md", "Notes", 2, 80, true),
            ],
            truncated: false,
        });
        let order = runner
            .app()
            .documents
            .iter()
            .map(|document| document.title.as_str())
            .collect::<Vec<_>>();
        assert_eq!(order, ["Notes", "Timetable", "Piranesi"]);
    }

    /// A card of `count` documents filed the way a managed library files
    /// them, an author to a folder and a book to a folder inside it. Every
    /// seventh is a PDF, which is listed and opens elsewhere.
    fn a_card_of(count: u32) -> Vec<LibraryEntry> {
        (0..count)
            .map(|n| {
                entry(
                    &format!(
                        "0/Library/Author {n:04}/Title {n:04} ({n})/Title {n:04} - Author {n:04}.epub"
                    ),
                    &format!("Title {n:04}"),
                    if n % 7 == 0 { 5 } else { 1 },
                    900_000 + n,
                    true,
                )
            })
            .collect()
    }

    #[test]
    fn a_listing_that_stopped_short_says_which_way_it_stopped() {
        let mut runner = AppRunner::new(Books::empty());
        runner.start();
        let text = words(&shown(runner.device_result(DeviceResult::Library {
            entries: vec![entry("0/notes.md", "Notes", 2, 80, true)],
            truncated: true,
        })));
        assert!(text.contains("did not reach every folder"), "{text}");
        assert!(text.contains("Some books are not shown"), "{text}");

        let most = u32::try_from(kobo_sdk::permissions::library::MAX_ENTRIES).expect("a count");
        let mut runner = AppRunner::new(Books::empty());
        runner.start();
        let home = shown(runner.device_result(DeviceResult::Library {
            entries: a_card_of(most),
            truncated: true,
        }));
        let text = words(&home);
        assert!(
            text.contains("more than the 2,000 books Books lists"),
            "{text}"
        );
        assert!(home.validate(&CLARA_BW_METRICS).is_empty());
    }

    #[test]
    fn a_card_of_two_thousand_books_is_shelved_whole_and_paged_to_its_last_book() {
        let mut runner = AppRunner::new(Books::empty());
        runner.start();
        answer_start(&mut runner, None, None, None);
        let home = shown(runner.device_result(DeviceResult::Library {
            entries: a_card_of(2_000),
            truncated: false,
        }));
        let text = words(&home);
        // 286 of the 2,000 are PDFs and stand on the Kobo reader's shelf.
        assert!(text.contains("1714 books"), "{text}");
        assert!(text.contains("286 books"), "{text}");
        assert!(
            !text.contains("not shown"),
            "a whole listing was called partial: {text}"
        );
        assert!(home.validate(&CLARA_BW_METRICS).is_empty());

        let first = shown(runner.action(action_id("shelf-library")));
        assert_eq!(portrait_tiles(&first).len(), SHELF_PAGE);
        assert!(first.validate(&CLARA_BW_METRICS).is_empty());
        let second = words(&shown(runner.action(action_id(super::NEXT))));
        assert!(second.contains("Title 0008"), "{second}");

        // Straight to the far end, as three hundred taps on Next would.
        runner.app_mut().page = usize::MAX;
        let last = shown(runner.action(action_id(super::PREVIOUS)));
        let text = words(&last);
        assert!(text.contains("Title 1999"), "{text}");
        assert!(last.validate(&CLARA_BW_METRICS).is_empty());
        let final_book = runner
            .app()
            .documents
            .iter()
            .position(|document| document.title == "Title 1999")
            .expect("listed");
        let opening = runner.action(action_id(&format!("book-{final_book}")));
        assert!(
            requests(&opening)
                .iter()
                .any(|request| matches!(request, DeviceRequest::ReadLibrary { .. })),
            "the last book of a full card could not be tapped"
        );
    }

    #[test]
    fn what_books_learned_about_a_full_card_is_kept_in_parts_and_read_back_at_the_next_start() {
        let described: std::collections::BTreeMap<String, covers::Described> = a_card_of(2_000)
            .into_iter()
            .map(|entry| {
                (
                    entry.id,
                    covers::Described {
                        bytes: entry.bytes,
                        listed: entry.title.clone(),
                        title: Some(format!(
                            "{} and the rest of what its package calls it",
                            entry.title
                        )),
                        author: Some("An Author With A Name Of Ordinary Length".to_owned()),
                        cover: covers::Cover::Missing,
                    },
                )
            })
            .collect();
        let mut runner = AppRunner::new(Books::empty());
        runner.start();
        answer_start(&mut runner, None, None, None);
        runner.app_mut().described.clone_from(&described);
        runner.app_mut().record = super::Record::Changed;
        let kept: Vec<(String, Vec<u8>)> = runner
            .lifecycle(kobo_sdk::Lifecycle::Background)
            .into_iter()
            .filter_map(|command| match command {
                Command::Store(StoreRequest::Save { key, value })
                    if covers::described_part(&key).is_some() =>
                {
                    Some((key, value))
                }
                _ => None,
            })
            .collect();
        assert!(kept.len() > 1, "a record of 2,000 books fitted one value");
        assert!(kept
            .iter()
            .all(|(_, value)| value.len() <= kobo_sdk::MAX_STORE_VALUE));
        assert_eq!(kept[0].0, DESCRIBED);

        // The next start reads the first part with everything else, then
        // each further part on reading the one before, to the first that is
        // not there.
        let mut runner = AppRunner::new(Books::empty());
        runner.start();
        let mut parts = kept.into_iter().map(|(_, value)| value);
        for (key, value) in [
            (READING.to_owned(), None),
            (DESCRIBED.to_owned(), parts.next()),
            (RECENT.to_owned(), None),
            (articles::RECORD.to_owned(), None),
            (articles::HOSTS.to_owned(), None),
            (articles::PAIRING.to_owned(), None),
            (super::ORDER.to_owned(), None),
        ] {
            runner.store_result(StoreResult::Loaded { key, value });
        }
        assert_eq!(
            runner.app().record,
            super::Record::Unread,
            "read before it was whole"
        );
        for index in 1.. {
            let value = parts.next();
            let last = value.is_none();
            runner.store_result(StoreResult::Loaded {
                key: covers::described_key(index),
                value,
            });
            if last {
                break;
            }
        }
        assert_eq!(runner.app().record, super::Record::Kept);
        assert_eq!(runner.app().described, described);
    }

    /// Books on a card of four books, three of them read for their names
    /// and authors on an earlier start, with the Library shelf open.
    fn on_a_named_shelf(order: Option<&[u8]>) -> AppRunner<Books> {
        let books = [
            (
                "0/a.epub",
                "wuthering",
                "Wuthering Heights",
                "Emily Bront\u{eb}",
                1_000,
            ),
            (
                "0/b.epub",
                "jane",
                "Jane Eyre",
                "Charlotte Bront\u{eb}",
                1_001,
            ),
            ("0/c.epub", "sea", "The Open Sea", "A. Mariner", 1_002),
        ];
        let described: std::collections::BTreeMap<String, covers::Described> = books
            .iter()
            .map(|(id, listed, title, author, bytes)| {
                (
                    (*id).to_owned(),
                    covers::Described {
                        bytes: *bytes,
                        listed: (*listed).to_owned(),
                        title: Some((*title).to_owned()),
                        author: Some((*author).to_owned()),
                        cover: covers::Cover::Missing,
                    },
                )
            })
            .collect();
        let record = covers::encode_described(&described, kobo_sdk::MAX_STORE_VALUE)
            .map(|mut parts| parts.remove(0));
        let mut runner = AppRunner::new(Books::empty());
        runner.start();
        for (key, value) in [
            (READING, None),
            (DESCRIBED, record),
            (RECENT, None),
            (articles::RECORD, None),
            (articles::HOSTS, None),
            (articles::PAIRING, None),
            (super::ORDER, order.map(<[u8]>::to_vec)),
        ] {
            runner.store_result(StoreResult::Loaded {
                key: key.into(),
                value,
            });
        }
        runner.store_result(StoreResult::Loaded {
            key: covers::described_key(1),
            value: None,
        });
        let mut entries: Vec<LibraryEntry> = books
            .iter()
            .map(|(id, listed, _, _, bytes)| entry(id, listed, 1, *bytes, true))
            .collect();
        entries.push(entry("0/d.md", "\u{c9}mile Zola, notes", 2, 80, true));
        runner.device_result(DeviceResult::Library {
            entries,
            truncated: false,
        });
        runner.action(action_id("shelf-library"));
        runner
    }

    fn titles(screen: &Screen) -> Vec<String> {
        portrait_tiles(screen)
            .into_iter()
            .map(|tile| tile.label)
            .collect()
    }

    #[test]
    fn a_name_is_compared_without_its_case_or_its_accents() {
        assert_eq!(super::folded("Charlotte BRONT\u{cb}"), "charlotte bronte");
        assert_eq!(super::folded("\u{c9}mile Zola"), "emile zola");
        assert_eq!(
            super::folded("Stra\u{df}e, \u{c6}on, \u{152}uvre"),
            "strasse, aeon, oeuvre"
        );
        assert_eq!(
            super::folded("Dvo\u{159}\u{e1}k \u{141}\u{f3}d\u{17a}"),
            "dvorak lodz"
        );
        // The same name with its marks as characters of their own.
        assert_eq!(super::folded("Bronte\u{308}"), "bronte");
        // Another script is left as it is, which still matches itself.
        assert_eq!(super::folded("\u{6751}\u{4e0a}"), "\u{6751}\u{4e0a}");
    }

    #[test]
    fn searching_the_library_finds_a_title_or_an_author_whatever_its_case_or_accents() {
        let mut runner = on_a_named_shelf(None);
        let field = runner.action(action_id(super::SEARCH));
        assert!(
            requests(&field)
                .iter()
                .any(|request| matches!(request, DeviceRequest::TypeText { .. })),
            "Search did not ask the runtime for its keyboard"
        );
        let found = shown(runner.type_text("BRONTE"));
        assert_eq!(titles(&found), ["Wuthering Heights", "Jane Eyre"]);
        let text = words(&found);
        assert!(text.contains("Clear"), "{text}");
        assert_eq!(
            found.top_bar.as_ref().map(|bar| bar.title.as_str()),
            Some("Search: BRONTE")
        );
        assert!(found.validate(&CLARA_BW_METRICS).is_empty());
        // Home still counts the whole library while its shelf is narrowed.
        assert_eq!(runner.app().members(Shelf::Library).len(), 4);

        for (typed, expected) in [
            // A title carrying an accent, typed without one.
            ("emile", vec!["\u{c9}mile Zola, notes"]),
            // Words from the title and the author, in either order.
            ("charlotte eyre", vec!["Jane Eyre"]),
            ("  open   SEA ", vec!["The Open Sea"]),
            // An accent typed where the name has none still finds it.
            ("\u{e9}yre", vec!["Jane Eyre"]),
        ] {
            runner.action(action_id(super::CLEAR_SEARCH));
            runner.action(action_id(super::SEARCH));
            let found = shown(runner.type_text(typed));
            assert_eq!(titles(&found), expected, "{typed:?}");
        }

        runner.action(action_id(super::CLEAR_SEARCH));
        runner.action(action_id(super::SEARCH));
        let nothing = shown(runner.type_text("xyzzy"));
        let text = words(&nothing);
        assert!(text.contains("Nothing found"), "{text}");
        assert!(text.contains("has xyzzy in it"), "{text}");
        assert!(text.contains("Clear"), "{text}");
    }

    #[test]
    fn back_undoes_a_search_before_it_leaves_the_shelf_and_backing_out_of_the_field_changes_nothing(
    ) {
        let mut runner = on_a_named_shelf(None);
        runner.action(action_id(super::SEARCH));
        let all = shown(runner.cancel_text());
        assert_eq!(
            titles(&all).len(),
            4,
            "backing out of the field narrowed the shelf"
        );
        assert_eq!(runner.app().view, View::Shelf(Shelf::Library));

        runner.action(action_id(super::SEARCH));
        assert_eq!(titles(&shown(runner.type_text("sea"))), ["The Open Sea"]);
        let cleared = shown(runner.action(ActionId::BACK));
        assert_eq!(titles(&cleared).len(), 4);
        assert_eq!(runner.app().view, View::Shelf(Shelf::Library));
        runner.action(ActionId::BACK);
        assert_eq!(runner.app().view, View::Home);
    }

    #[test]
    fn the_library_stands_in_the_order_its_owner_chose_and_the_choice_is_kept() {
        let mut runner = on_a_named_shelf(None);
        let chosen = runner.action(action_id("order-title"));
        assert_eq!(
            saved(&chosen, super::ORDER).as_deref(),
            Some(&b"order-title"[..]),
            "the choice was not kept"
        );
        let by_title = shown(chosen);
        assert_eq!(
            titles(&by_title),
            [
                "\u{c9}mile Zola, notes",
                "Jane Eyre",
                "The Open Sea",
                "Wuthering Heights"
            ]
        );
        assert_eq!(runner.app().library.order, super::Order::Title);
        // All three orders stand on the shelf at once, and the screen they
        // share with the covers has nothing to remark on.
        let text = words(&by_title);
        for label in ["Recent", "Title", "Author"] {
            assert!(text.contains(label), "{label}: {text}");
        }
        let issues = by_title.validate(&CLARA_BW_METRICS);
        assert!(issues.is_empty(), "{issues:?}");

        let by_author = shown(runner.action(action_id("order-author")));
        // By the name as it is written, then the book with none.
        assert_eq!(
            titles(&by_author),
            [
                "The Open Sea",
                "Jane Eyre",
                "Wuthering Heights",
                "\u{c9}mile Zola, notes"
            ]
        );

        // The next start puts the shelf back the way it was left.
        let mut runner = on_a_named_shelf(Some(b"order-author"));
        assert_eq!(runner.app().library.order, super::Order::Author);
        runner.action(action_id("order-title"));
        let again = shown(runner.action(action_id("order-author")));
        assert_eq!(titles(&again)[0], "The Open Sea");
        // A value this build has no order for leaves the shelf alone.
        let runner = on_a_named_shelf(Some(b"order-by-colour"));
        assert_eq!(runner.app().library.order, super::Order::Recent);
    }

    #[test]
    fn a_search_keeps_the_order_the_shelf_stands_in() {
        let mut runner = on_a_named_shelf(Some(b"order-title"));

        runner.action(action_id(super::SEARCH));
        let found = shown(runner.type_text("e"));
        assert_eq!(
            titles(&found),
            [
                "\u{c9}mile Zola, notes",
                "Jane Eyre",
                "The Open Sea",
                "Wuthering Heights"
            ]
        );
    }

    #[test]
    fn an_empty_library_says_how_to_add_books() {
        let mut runner = AppRunner::new(Books::empty());
        let text = words(&shown(runner.start()));
        assert!(text.contains("copy EPUB"), "{text}");
    }

    #[test]
    fn a_library_that_could_not_be_listed_offers_to_try_again() {
        let mut runner = AppRunner::new(Books::empty());
        runner.start();
        let screen = shown(runner.device_result(DeviceResult::Failed(DeviceError::Backend)));
        let text = words(&screen);
        assert!(text.contains("could not be read"), "{text}");
        assert!(!text.contains("No books yet"), "{text}");
        let commands = runner.action(action_id("relist"));
        assert!(commands
            .iter()
            .any(|command| matches!(command, Command::Device(DeviceRequest::ListLibrary))));
    }

    #[test]
    fn an_article_a_notice_announced_opens_once_a_listing_that_names_it_arrives() {
        let mut runner = AppRunner::new(Books::empty());
        runner.start();
        let asked = runner.open_document("0/Articles/river.epub".to_owned());
        assert!(asked
            .iter()
            .any(|command| matches!(command, Command::Device(DeviceRequest::ListLibrary))));
        let opened = runner.device_result(DeviceResult::Library {
            entries: vec![
                entry("0/sea.epub", "sea", 1, 100, true),
                entry("0/Articles/river.epub", "river", 1, 100, true),
            ],
            truncated: false,
        });
        assert!(opened.iter().any(|command| matches!(
            command,
            Command::Device(DeviceRequest::ReadLibrary { id }) if id == "0/Articles/river.epub"
        )));
    }

    #[test]
    fn back_from_a_page_closes_the_book_rather_than_leaving_books() {
        let mut runner = started_with(None, None);
        runner.action(action_id("shelf-library"));
        let page = shown(runner.action(action_id("book-0")));
        assert_eq!(runner.app().view, View::Reading);
        // The device decides from the screen alone whether Back is offered to
        // Books or taken to the launcher, and one that does not claim it goes.
        assert!(
            page.owns_back,
            "Back from a page went to the launcher with the book still open"
        );
        runner.action(ActionId::BACK);
        assert_eq!(runner.app().view, View::Shelf(super::Shelf::Library));
    }

    #[test]
    fn an_article_opened_from_its_notice_is_titled_by_its_package_and_closes_onto_its_shelf() {
        let mut runner = AppRunner::new(Books::empty());
        runner.start();
        answer_start(&mut runner, None, None, None);
        let id = "0/Articles/Electronic paper (3).epub";
        runner.open_document(id.to_owned());
        runner.device_result(DeviceResult::Library {
            entries: vec![entry(id, "Electronic paper (3)", 1, 100, true)],
            truncated: false,
        });
        let bytes = kobo_doc::epub::write(
            "Electronic paper",
            None,
            &[kobo_doc::epub::Chapter {
                title: "Electronic paper".to_owned(),
                body: "<p>A display that reflects light like paper.</p>".to_owned(),
            }],
        )
        .expect("a book");
        // Books reads the new article once for the shelf and once to open
        // it; the second answer opens it.
        let mut page = None;
        for _ in 0..2 {
            let commands = runner.device_result(DeviceResult::LibraryDocument {
                id: id.to_owned(),
                bytes: bytes.clone(),
            });
            if runner.app().view == View::Reading {
                page = Some(shown(commands));
                break;
            }
        }
        let page = page.expect("the article opened");
        assert_eq!(runner.app().view, View::Reading);
        assert_eq!(runner.app().open_title.as_deref(), Some("Electronic paper"));
        assert!(page.owns_back);
        runner.action(ActionId::BACK);
        assert_eq!(runner.app().view, View::Shelf(Shelf::Articles));
    }

    #[test]
    fn an_article_deleted_after_its_notice_leaves_books_on_its_shelf() {
        let mut runner = AppRunner::new(Books::empty());
        runner.start();
        runner.open_document("0/Articles/gone.epub".to_owned());
        let shelf = runner.device_result(DeviceResult::Library {
            entries: vec![entry("0/sea.epub", "sea", 1, 100, true)],
            truncated: false,
        });
        assert!(!shelf
            .iter()
            .any(|command| matches!(command, Command::Device(DeviceRequest::ReadLibrary { id }) if id.contains("gone"))));
        assert_eq!(runner.app().view, View::Shelf(Shelf::Articles));
    }

    fn article(name: &str, title: &str) -> Document {
        Document {
            id: format!("0/Articles/{name}.epub"),
            title: title.to_owned(),
            listed: title.to_owned(),
            author: None,
            kind: Kind::Epub,
            preview: title.to_owned(),
            on_card: true,
            bytes: 100,
            body: Some(format!("{title}. The river found its way to the sea.")),
        }
    }

    fn kept(saved: u64, read: bool, archived: bool) -> articles::Kept {
        articles::Kept {
            bytes: 100,
            site: Some("example.org".to_owned()),
            saved: Some(saved),
            described: true,
            read,
            archived,
        }
    }

    /// Books with three articles and one book, every record already read.
    fn with_articles() -> Books {
        let mut books = Books::empty();
        let mut documents = vec![
            article("old", "An old unread one"),
            article("new", "A new unread one"),
            article("read", "One already read"),
        ];
        documents.extend(fixtures().into_iter().take(1));
        books.take_documents(documents);
        books.articles.loaded = true;
        books.record = super::Record::Kept;
        for (name, kept) in [
            ("old", kept(1_000, false, false)),
            ("new", kept(9_000, false, false)),
            ("read", kept(20_000, true, false)),
        ] {
            books
                .articles
                .kept
                .insert(format!("0/Articles/{name}.epub"), kept);
        }
        books
    }

    fn index_of(books: &Books, title: &str) -> usize {
        books
            .documents
            .iter()
            .position(|document| document.title == title)
            .expect("a document with that title")
    }

    /// An article's place on a shelf, which is what its row is named by.
    fn row_of(books: &Books, shelf: Shelf, title: &str) -> usize {
        titles_on(books, shelf)
            .iter()
            .position(|shown| shown == title)
            .expect("the article is on that shelf")
    }

    fn titles_on(books: &Books, shelf: Shelf) -> Vec<String> {
        books
            .members(shelf)
            .into_iter()
            .map(|index| books.documents[index].title.clone())
            .collect()
    }

    #[test]
    fn articles_have_a_shelf_of_their_own_with_unread_first_and_the_newest_first() {
        let books = with_articles();
        assert_eq!(
            titles_on(&books, Shelf::Articles),
            ["A new unread one", "An old unread one", "One already read"]
        );
        assert_eq!(titles_on(&books, Shelf::Library), ["The Open Sea"]);
    }

    #[test]
    fn the_articles_shelf_lists_each_with_its_site_and_offers_sync() {
        let mut books = with_articles();
        books.view = View::Shelf(Shelf::Articles);
        let mut runner = AppRunner::new(books);
        let screen = shown(runner.start());
        let said = words(&screen);
        assert!(said.contains("A new unread one"), "{said}");
        assert!(said.contains("example.org"), "{said}");
        assert!(said.contains("Sync"), "{said}");
        assert!(
            !said.contains("The Open Sea"),
            "a book is not an article: {said}"
        );
    }

    #[test]
    fn a_record_kept_in_parts_is_read_part_by_part_and_a_smaller_one_empties_the_rest() {
        let mut books = Books::empty();
        books.take_documents(vec![
            article("old", "An old unread one"),
            article("read", "One already read"),
        ]);
        books.record = super::Record::Kept;
        let mut runner = AppRunner::new(books);
        runner.start();
        let part = |name: &str, read: bool| {
            let one = std::collections::BTreeMap::from([(
                format!("0/Articles/{name}.epub"),
                kept(1_000, read, false),
            )]);
            articles::encode(&one, kobo_sdk::MAX_STORE_VALUE)
                .expect("fits")
                .remove(0)
        };
        let mut answer = |key: String, value: Option<Vec<u8>>| {
            runner.store_result(StoreResult::Loaded { key, value })
        };
        let asked = |commands: &[Command], wanted: &str| {
            commands.iter().any(|command| {
                matches!(command, Command::Store(StoreRequest::Load { key }) if key == wanted)
            })
        };
        // Replies come back in the order Books asked: its start-up records
        // first, then each part it asks for on reading the one before.
        for key in [READING, DESCRIBED, RECENT] {
            answer(key.to_owned(), None);
        }
        let commands = answer(articles::part_key(0), Some(part("old", false)));
        assert!(
            asked(&commands, &articles::part_key(1)),
            "the next part is read"
        );
        answer(articles::HOSTS.to_owned(), None);
        answer(articles::PAIRING.to_owned(), None);
        answer(super::ORDER.to_owned(), None);
        let commands = answer(articles::part_key(1), Some(part("read", true)));
        assert!(asked(&commands, &articles::part_key(2)));
        answer(articles::part_key(2), None);
        let app = runner.app();
        assert!(app.articles.loaded);
        assert_eq!(app.articles.parts_stored, 2);
        assert!(app.articles.kept["0/Articles/read.epub"].read);
        assert!(!app.articles.kept["0/Articles/old.epub"].read);

        let index = index_of(runner.app(), "An old unread one");
        runner.action(action_id(&format!("book-{index}")));
        let commands = runner.action(ActionId::BACK);
        assert!(saved(&commands, articles::RECORD).is_some());
        assert_eq!(
            saved(&commands, &articles::part_key(1)),
            Some(Vec::new()),
            "the second part, no longer needed, is emptied"
        );
    }

    #[test]
    fn an_article_read_to_its_end_moves_down_among_the_read() {
        let mut runner = AppRunner::new(with_articles());
        runner.start();
        let index = index_of(runner.app(), "A new unread one");
        runner.action(action_id(&format!("book-{index}")));
        assert_eq!(runner.app().view, View::Reading);
        runner.action(ActionId::BACK);
        assert!(runner.app().articles.kept["0/Articles/new.epub"].read);
        assert_eq!(
            titles_on(runner.app(), Shelf::Articles),
            ["An old unread one", "One already read", "A new unread one"]
        );
    }

    #[test]
    fn an_archived_article_leaves_the_shelf_for_the_archive_and_can_come_back() {
        let mut books = with_articles();
        books.view = View::Shelf(Shelf::Articles);
        let mut runner = AppRunner::new(books);
        runner.start();
        let row = row_of(runner.app(), Shelf::Articles, "An old unread one");
        let menu = shown(runner.action(action_id(&format!("article-menu-{row}"))));
        assert!(words(&menu).contains("Archive"));
        let commands = runner.action(action_id("article-archive"));
        assert!(
            saved(&commands, articles::RECORD).is_some(),
            "the archive is kept"
        );
        assert_eq!(
            titles_on(runner.app(), Shelf::Articles),
            ["A new unread one", "One already read"]
        );
        let archive = shown(runner.action(action_id("archived")));
        assert!(words(&archive).contains("An old unread one"));
        runner.action(action_id("article-menu-0"));
        runner.action(action_id("article-restore"));
        assert_eq!(
            titles_on(runner.app(), Shelf::Archive),
            Vec::<String>::new()
        );
        runner.action(ActionId::BACK);
        assert_eq!(runner.app().view, View::Shelf(Shelf::Articles));
    }

    #[test]
    fn delete_asks_first_and_the_article_goes_only_once_the_reader_let_it_go() {
        let mut books = with_articles();
        books.view = View::Shelf(Shelf::Articles);
        let mut runner = AppRunner::new(books);
        runner.start();
        let row = row_of(runner.app(), Shelf::Articles, "One already read");
        runner.action(action_id(&format!("article-menu-{row}")));
        let asked = shown(runner.action(action_id("article-delete")));
        assert!(words(&asked).contains("Delete this article?"));
        assert!(
            words(&asked).contains("One already read"),
            "it names what goes"
        );
        let kept = runner.action(action_id("keep-article"));
        assert!(!kept.iter().any(|command| matches!(
            command,
            Command::Device(DeviceRequest::RemoveArticle { .. })
        )));
        runner.action(action_id(&format!("article-menu-{row}")));
        runner.action(action_id("article-delete"));
        let confirmed = runner.action(action_id("confirm-delete"));
        assert!(confirmed.iter().any(|command| matches!(
            command,
            Command::Device(DeviceRequest::RemoveArticle { id }) if id == "0/Articles/read.epub"
        )));
        assert!(titles_on(runner.app(), Shelf::Articles).contains(&"One already read".to_owned()));
        runner.device_result(DeviceResult::Done);
        assert!(!titles_on(runner.app(), Shelf::Articles).contains(&"One already read".to_owned()));
        assert!(!runner
            .app()
            .articles
            .kept
            .contains_key("0/Articles/read.epub"));
    }

    #[test]
    fn a_listing_that_arrives_while_delete_is_asking_never_changes_what_is_deleted() {
        let mut books = with_articles();
        books.view = View::Shelf(Shelf::Articles);
        let mut runner = AppRunner::new(books);
        runner.start();
        let row = row_of(runner.app(), Shelf::Articles, "One already read");
        runner.action(action_id(&format!("article-menu-{row}")));
        runner.action(action_id("article-delete"));
        // A Sync lands another article meanwhile, and every place moves.
        let mut documents = vec![article("arrived", "Arrived just now")];
        documents.extend(runner.app().documents.clone());
        runner.app_mut().documents = documents;
        let confirmed = runner.action(action_id("confirm-delete"));
        assert!(confirmed.iter().any(|command| matches!(
            command,
            Command::Device(DeviceRequest::RemoveArticle { id }) if id == "0/Articles/read.epub"
        )));
    }

    #[test]
    fn a_delete_the_reader_refused_keeps_the_article_and_says_so() {
        let mut books = with_articles();
        books.view = View::Shelf(Shelf::Articles);
        let mut runner = AppRunner::new(books);
        runner.start();
        let row = row_of(runner.app(), Shelf::Articles, "One already read");
        runner.action(action_id(&format!("article-menu-{row}")));
        runner.action(action_id("article-delete"));
        runner.action(action_id("confirm-delete"));
        let said =
            shown(runner.device_result(DeviceResult::Failed(kobo_sdk::DeviceError::Backend)));
        assert!(words(&said).contains("could not be deleted"));
        assert!(titles_on(runner.app(), Shelf::Articles).contains(&"One already read".to_owned()));
    }

    fn post_of(commands: &[Command]) -> Option<(TaskId, String, String)> {
        commands.iter().find_map(|command| match command {
            Command::Spawn {
                task,
                work: kobo_sdk::Task::Post { url, body, .. },
            } => Some((*task, url.clone(), body.clone())),
            _ => None,
        })
    }

    fn paired_runner() -> (AppRunner<Books>, Vec<Command>) {
        let mut runner = AppRunner::new(with_articles());
        runner.start();
        let mut commands = Vec::new();
        for (key, value) in [
            (READING, None),
            (DESCRIBED, None),
            (RECENT, None),
            (articles::RECORD, None),
            (articles::HOSTS, None),
            (
                articles::PAIRING,
                Some(b"192.168.1.10:9331|abc234".to_vec()),
            ),
            (super::ORDER, None),
        ] {
            commands.extend(runner.store_result(StoreResult::Loaded {
                key: key.into(),
                value,
            }));
        }
        (runner, commands)
    }

    #[test]
    fn opening_books_asks_the_paired_computer_quietly() {
        let (mut runner, commands) = paired_runner();
        let (task, url, body) = post_of(&commands).expect("the computer is asked");
        assert_eq!(url, "https://192.168.1.10:9331/articles/sync");
        assert!(body.contains("abc234"));
        let shelf = shown(runner.action(action_id("shelf-articles")));
        assert!(
            !words(&shelf).contains("Checking"),
            "a quiet check says nothing"
        );
        runner.task_outcome(task, TaskOutcome::Failed(kobo_sdk::TaskError::Unreachable));
        assert_eq!(
            runner.app().articles.said,
            None,
            "a computer that is off is not news on opening"
        );
    }

    /// Books opened on what `kobo sidekick init` writes for a computer on a
    /// tailnet: the pairing, and the hosts record beside it.
    fn paired_at_both() -> (AppRunner<Books>, Vec<Command>) {
        let mut runner = AppRunner::new(with_articles());
        runner.start();
        let mut commands = Vec::new();
        for (key, value) in [
            (READING, None),
            (DESCRIBED, None),
            (RECENT, None),
            (articles::RECORD, None),
            (
                articles::HOSTS,
                Some(b"192.168.1.10:9331\nstudio.example.ts.net:9331".to_vec()),
            ),
            (
                articles::PAIRING,
                Some(b"192.168.1.10:9331|abc234".to_vec()),
            ),
            (super::ORDER, None),
        ] {
            commands.extend(runner.store_result(StoreResult::Loaded {
                key: key.into(),
                value,
            }));
        }
        (runner, commands)
    }

    #[test]
    fn a_computer_not_at_its_first_host_is_asked_at_the_other_and_first_from_then_on() {
        let (mut runner, commands) = paired_at_both();
        let (task, url, _) = post_of(&commands).expect("the computer is asked");
        assert_eq!(url, "https://192.168.1.10:9331/articles/sync");
        let commands =
            runner.task_outcome(task, TaskOutcome::Failed(kobo_sdk::TaskError::Unreachable));
        let (task, url, body) = post_of(&commands).expect("the other host is asked at once");
        assert_eq!(url, "https://studio.example.ts.net:9331/articles/sync");
        assert!(body.contains("abc234"));
        let commands = runner.task_outcome(
            task,
            TaskOutcome::Completed(br#"{"delivered":0,"waiting":0}"#.to_vec()),
        );
        assert!(commands.iter().any(|command| matches!(
            command,
            Command::Store(kobo_sdk::StoreRequest::Save { key, value })
                if key == articles::HOSTS
                    && value == b"studio.example.ts.net:9331\n192.168.1.10:9331"
        )));
        runner.action(action_id("shelf-articles"));
        let asked = runner.action(action_id("sync"));
        let (_, url, _) = post_of(&asked).expect("asked again");
        assert_eq!(url, "https://studio.example.ts.net:9331/articles/sync");
    }

    #[test]
    fn a_refused_pairing_is_said_and_not_taken_to_the_other_host() {
        let (mut runner, commands) = paired_at_both();
        runner.action(action_id("shelf-articles"));
        let (quiet, _, _) = post_of(&commands).expect("the quiet check");
        runner.task_outcome(
            quiet,
            TaskOutcome::Completed(br#"{"delivered":0,"waiting":0}"#.to_vec()),
        );
        let asked = runner.action(action_id("sync"));
        let (task, _, _) = post_of(&asked).expect("asked");
        let commands =
            runner.task_outcome(task, TaskOutcome::Failed(kobo_sdk::TaskError::Unauthorized));
        assert!(post_of(&commands).is_none());
        assert_eq!(runner.app().articles.said, Some(articles::Said::Refused));
    }

    #[test]
    fn sync_says_it_is_checking_then_how_many_arrived_and_lists_the_card_again() {
        let (mut runner, commands) = paired_runner();
        let (quiet, _, _) = post_of(&commands).expect("the quiet check");
        runner.task_outcome(
            quiet,
            TaskOutcome::Completed(br#"{"delivered":0,"waiting":0}"#.to_vec()),
        );
        runner.action(action_id("shelf-articles"));
        let asked = runner.action(action_id("sync"));
        assert!(words(&shown(asked.clone())).contains("Checking your computer"));
        let (task, _, _) = post_of(&asked).expect("the computer is asked again");
        let answered = runner.task_outcome(
            task,
            TaskOutcome::Completed(br#"{"delivered":2,"titles":["A","B"],"waiting":0}"#.to_vec()),
        );
        assert!(words(&shown(answered.clone())).contains("2 articles arrived"));
        assert!(answered
            .iter()
            .any(|command| matches!(command, Command::Device(DeviceRequest::ListLibrary))));
    }

    #[test]
    fn sync_with_nothing_waiting_or_no_computer_says_so_plainly() {
        let (mut runner, commands) = paired_runner();
        let (quiet, _, _) = post_of(&commands).expect("the quiet check");
        runner.task_outcome(
            quiet,
            TaskOutcome::Completed(br#"{"delivered":0,"waiting":0}"#.to_vec()),
        );
        runner.action(action_id("shelf-articles"));
        let (task, _, _) = post_of(&runner.action(action_id("sync"))).expect("asked");
        let said = shown(runner.task_outcome(
            task,
            TaskOutcome::Completed(br#"{"delivered":0,"waiting":0}"#.to_vec()),
        ));
        assert!(words(&said).contains("Nothing new"));
        let (task, _, _) = post_of(&runner.action(action_id("sync"))).expect("asked");
        let said =
            shown(runner.task_outcome(task, TaskOutcome::Failed(kobo_sdk::TaskError::Unreachable)));
        assert!(words(&said).contains("Your computer is not answering"));
    }

    #[test]
    fn sync_on_a_reader_that_was_never_paired_says_how_to_pair_it() {
        let mut books = with_articles();
        books.view = View::Shelf(Shelf::Articles);
        let mut runner = AppRunner::new(books);
        runner.start();
        let commands = runner.action(action_id("sync"));
        assert!(post_of(&commands).is_none());
        let screen = shown(commands);
        assert_eq!(commands_on(&screen), ["kobo sidekick init"]);
        let text = words(&screen);
        assert!(text.contains("Sync is not set up"), "{text}");
        assert!(text.contains("once on the computer:"), "{text}");
    }

    /// What a screen sets apart as something to type on a computer, as the
    /// owner would type it: the hyphens of a command travel as the kind
    /// that does not break a line.
    fn commands_on(screen: &Screen) -> Vec<String> {
        screen
            .nodes
            .iter()
            .filter_map(|node| match node {
                Node::Quote {
                    role: kobo_sdk::QuoteRole::Command,
                    text,
                    ..
                } => Some(text.replace('\u{2011}', "-")),
                _ => None,
            })
            .collect()
    }

    /// Everything a screen says that is not a command to type.
    fn sentences_on(screen: &Screen) -> String {
        screen
            .nodes
            .iter()
            .filter_map(|node| match node {
                Node::Banner { text, .. }
                | Node::Text { text, .. }
                | Node::Heading { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn every_way_sync_fails_fits_the_shelf_with_its_command_set_apart_from_its_sentence() {
        use articles::Said;
        for said in [
            Said::Waiting(1),
            Said::Waiting(7),
            Said::NotAnswering,
            Said::Offline,
            Said::Refused,
            Said::Outdated,
            Said::ComputerTrouble,
            Said::NotPaired,
        ] {
            let line = said.line();
            // On a shelf with articles on it, and on one with none.
            let mut full = with_articles();
            let mut empty = Books::empty();
            empty.articles.computer = Some(("192.168.1.10:9331".to_owned(), "abc234".to_owned()));
            empty.articles.loaded = true;
            for (books, holds_articles) in [(&mut full, true), (&mut empty, false)] {
                books.view = View::Shelf(Shelf::Articles);
                books.articles.said = Some(said.clone());
                let mut context = kobo_sdk::Context::default();
                let screen = books.shelf(&mut context, Shelf::Articles);
                let issues = screen.validate(&CLARA_BW_METRICS);
                assert!(issues.is_empty(), "{said:?}: {issues:?}");
                let sentences = sentences_on(&screen);
                assert!(sentences.contains(&line.words), "{said:?}: {sentences}");
                assert!(
                    !sentences.contains("kobo "),
                    "{said:?} put a command inside a sentence: {sentences}"
                );
                let expected: Vec<String> = line
                    .next
                    .iter()
                    .map(|(_, command)| (*command).to_owned())
                    .collect();
                if holds_articles || line.next.is_some() {
                    assert_eq!(commands_on(&screen), expected, "{said:?}");
                } else {
                    // Nothing to type for the failure, so the empty shelf
                    // still says how a first article is sent.
                    assert_eq!(commands_on(&screen), ["kobo clip install --all"]);
                }
                if holds_articles {
                    assert!(
                        words(&screen).contains("A new unread one"),
                        "{said:?} pushed every article off the first page"
                    );
                } else {
                    assert!(words(&screen).contains("No articles yet"), "{said:?}");
                }
            }
        }
    }

    #[test]
    fn an_empty_articles_shelf_says_how_a_first_article_is_sent() {
        let mut books = Books::empty();
        books.articles.computer = Some(("192.168.1.10:9331".to_owned(), "abc234".to_owned()));
        books.articles.loaded = true;
        books.view = View::Shelf(Shelf::Articles);
        let mut runner = AppRunner::new(books);
        let screen = shown(runner.start());
        let said = words(&screen);
        assert!(said.contains("No articles yet"), "{said}");
        assert!(said.contains("Sync"), "{said}");
        // How the first one gets here, in something an owner can do: the
        // menu in their browser, and the one command that puts it there.
        assert!(
            said.contains("right-click a page in your computer's browser and choose Send to Kobo"),
            "{said}"
        );
        assert_eq!(commands_on(&screen), ["kobo clip install --all"]);
        assert!(!sentences_on(&screen).contains("kobo "), "{said}");
        assert!(screen.validate(&CLARA_BW_METRICS).is_empty());
    }
}

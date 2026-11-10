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

use articles::{Kept, Said};
use covers::{Cover, Described, Recent, Thumb};
use kobo_bookview::positions::{Progress, KEY as READING};
use kobo_bookview::{BookView, Step};
use kobo_read::Outcome;
use kobo_sdk::clock::Clock as _;
use kobo_sdk::{
    action_id, document_preview, typographic_cover, ActionId, BandAlign, BannerLevel, Context,
    DeviceRequest, DeviceResult, Glyph, KoboApp, LibraryEntry, PictureHandle, Position, Screen,
    ScreenBuilder, SlotWidth, Space, StoreResult, Task, TaskId, TaskOutcome, Tile, TilePicture,
    TileShape, MAX_STORE_VALUE,
};
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

/// How long after one check Books leaves the computer alone when it comes
/// back to the panel. Opening Books asks; flicking in and out of it should
/// not ask every time.
const QUIET_CHECK_EVERY_MS: u64 = 5 * 60 * 1000;

/// Where to send an owner for anything Books cannot open itself.
const IN_KOBO_READER: &str = "To get there, tap Kobo reader at the foot of the Cobalt home screen.";

/// Store key for what reading each book told the shelf: name, author, cover.
const DESCRIBED: &str = "described-v1";
/// Store key for the books opened most recently, newest first.
const RECENT: &str = "recent-v1";
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

/// The largest book the runtime will hand over in one piece.
///
/// The wire's own limit on a library document. A larger book is listed, and
/// says why it does not open here rather than failing as if it were damaged.
const LARGEST_OPENABLE: u32 = 4 * 1024 * 1024;

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

    /// Whether tapping it opens it here rather than explaining why not.
    const fn opens_here(&self) -> bool {
        self.belongs_here() && !self.too_large()
    }

    const fn too_large(&self) -> bool {
        self.bytes > LARGEST_OPENABLE
    }

    /// Whether Books can read it for a cover and a title.
    const fn describable(&self) -> bool {
        self.on_card && self.kind.is_packaged() && !self.too_large() && self.body.is_none()
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
            book: BookView::new(),
            open_title: None,
            progress: Progress::default(),
            reading_id: None,
            opened: None,
            library_failed: false,
            truncated: false,
            described: BTreeMap::new(),
            record: Record::Unread,
            recent: Vec::new(),
            covers: BTreeMap::new(),
            lettered: BTreeMap::new(),
            wanted: VecDeque::new(),
            describing: None,
            loading: BTreeMap::new(),
            on_panel: BTreeSet::new(),
            placed: BTreeMap::new(),
            articles: articles::State::default(),
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
            View::Shelf(pile) => self.shelf(context, pile),
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
        let screen = ScreenBuilder::new("books-shelf")
            .top_bar(pile.title())
            .owns_back(true);
        let members = self.members(pile);
        if members.is_empty() {
            return screen
                .splash(
                    Some(Glyph::Book),
                    "Nothing on this shelf",
                    "Books you add or open appear here.",
                )
                .build();
        }
        let mut screen = self.notices(screen);
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
                } else if document.belongs_here() && document.too_large() {
                    "Too large to open here".to_owned()
                } else {
                    document.author.clone().unwrap_or_default()
                };
                let badge = self
                    .percent_read(&document.id)
                    .filter(|_| document.opens_here())
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
        if self.truncated {
            screen.banner(
                BannerLevel::Info,
                "This reader holds more books than Books can list. Some are not shown.",
            )
        } else {
            screen
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
                let mut members: Vec<usize> =
                    opened.into_iter().filter(|&index| shelved(index)).collect();
                for index in 0..self.documents.len() {
                    if shelved(index) && !members.contains(&index) {
                        members.push(index);
                    }
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
            let (heading, words) = if pile == Shelf::Archive {
                (
                    "No archived articles",
                    "Articles you archive are kept here, out of the way.",
                )
            } else {
                (
                    "No articles yet",
                    "Web pages you send from your computer arrive here. Sync asks it for any that are waiting.",
                )
            };
            return screen.splash(Some(Glyph::News), heading, words).build();
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
            let (words, trouble) = said.text();
            screen = screen.banner(
                if trouble {
                    BannerLevel::Error
                } else {
                    BannerLevel::Info
                },
                words,
            );
        }
        screen
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
        let Some((address, code)) = self.articles.computer.clone() else {
            if !quiet {
                self.articles.said = Some(Said::NotPaired);
            }
            return;
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
            self.documents
                .iter()
                .position(|document| document.id == recent.id && document.opens_here())
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
        } else if let Some(document) = document.filter(|document| document.too_large()) {
            format!(
                "Books can open files up to 4 MB, and this one is {}. Open it in the app it came from, or in the Kobo reader if you copied it on yourself.",
                megabytes(document.bytes)
            )
        } else {
            "This file could not be opened. It may be damaged, or written in a way Books cannot read."
                .to_owned()
        };
        // The bar's own back arrow returns to the shelf too. Without owning
        // it, that arrow left Books altogether from a screen that is only a
        // note about one book.
        ScreenBuilder::new("books-unreadable")
            .top_bar("Books")
            .owns_back(true)
            .splash(Some(Glyph::Book), title, reason)
            .build()
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
        let placed = self
            .notices(
                ScreenBuilder::new("books-shelf")
                    .top_bar(pile.title())
                    .owns_back(true),
            )
            .build();
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
        if document.describable()
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
        while let Some(id) = self.wanted.pop_front() {
            // Articles are read wherever they are, because the shelf is
            // sorted by the date inside each one.
            let still_wanted = (self.on_panel.contains(&id) || articles::is_article(&id))
                && self
                    .documents
                    .iter()
                    .any(|document| document.id == id && document.describable());
            if still_wanted {
                self.describing = Some(id.clone());
                context.device().read_library(id);
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
        let bytes = covers::encode_described(&self.described);
        if bytes.len() <= MAX_STORE_VALUE {
            context.store().save(DESCRIBED, bytes);
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
        if !document.opens_here() {
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
        self.show(context);
        context.device().read_library(document.id);
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
            self.open_title = Some(document.title.clone());
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
            return true;
        }
        if action == ActionId::BACK || action == action_id(BACK) {
            self.close_reader(context);
            return true;
        }
        false
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

    fn on_document_refused(&mut self, context: &mut Context) {
        if let Some(id) = self.describing.take() {
            // Not read, and not worth asking about again on every repaint.
            let (bytes, listed) = self
                .documents
                .iter()
                .find(|document| document.id == id)
                .map_or((0, String::new()), |document| {
                    (document.bytes, document.listed.clone())
                });
            if let Some(kept) = self.articles.kept.get_mut(&id) {
                // Not asked about again on every listing either.
                kept.described = true;
                self.articles.changed = true;
            }
            self.described.insert(
                id,
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
            self.book.close(context);
            self.open_title = None;
            self.view = View::Unreadable(index);
            self.show(context);
            return;
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
        context.store().load(articles::PAIRING);
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
            DESCRIBED => {
                if let StoreResult::Loaded {
                    value: Some(bytes), ..
                } = result
                {
                    self.described = covers::decode_described(&bytes);
                }
                self.record = Record::Kept;
                let documents = std::mem::take(&mut self.documents);
                self.take_documents(documents);
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
            articles::PAIRING => {
                if let StoreResult::Loaded {
                    value: Some(bytes), ..
                } = result
                {
                    self.articles.computer = articles::pairing(&bytes);
                }
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

    fn on_save(&mut self, context: &mut Context, key: &str, result: StoreResult) {
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

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if self.read_action(context, action) {
            return;
        }
        if self.article_action(context, action) {
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
            self.view = View::Shelf(shelf);
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
                let count = self.members(shelf).len();
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
        if let DeviceRequest::ReadLibrary { .. } = request {
            match result {
                DeviceResult::LibraryDocument { id, bytes } => {
                    self.on_document(context, &id, &bytes);
                }
                DeviceResult::Denied(_) | DeviceResult::Failed(_) => {
                    self.on_document_refused(context);
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
        for (key, value) in [
            (READING, reading),
            (DESCRIBED, described),
            (RECENT, recent),
            (articles::RECORD, None),
            (articles::PAIRING, None),
        ] {
            commands.extend(runner.store_result(StoreResult::Loaded {
                key: key.into(),
                value,
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
        let members: Vec<(String, Vec<u8>)> = members
            .into_iter()
            .map(|(name, bytes)| (name.to_owned(), bytes))
            .collect();
        kobo_doc::zip::stored(&members).expect("a book")
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
            Some(covers::encode_described(&described)),
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
    fn a_book_too_large_to_open_here_says_how_large_and_is_not_read_for_a_cover() {
        let mut runner = AppRunner::new(Books::empty());
        runner.start();
        answer_start(&mut runner, None, None, None);
        let asked = runner.device_result(DeviceResult::Library {
            entries: vec![entry(
                "1/gutenbird/book-1a2b3c4d",
                "Ulysses",
                1,
                6 * 1024 * 1024,
                true,
            )],
            truncated: false,
        });
        assert!(
            !asked.iter().any(|command| matches!(
                command,
                Command::Device(DeviceRequest::ReadLibrary { .. })
            )),
            "a book the runtime will not hand over was asked for"
        );
        let text = words(&shown(runner.action(action_id("book-0"))));
        assert!(text.contains("6.0 MB"), "{text}");
        assert!(text.contains("up to 4 MB"), "{text}");
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

    #[test]
    fn a_listing_that_stopped_short_says_so() {
        let mut runner = AppRunner::new(Books::empty());
        runner.start();
        let text = words(&shown(runner.device_result(DeviceResult::Library {
            entries: vec![entry("0/notes.md", "Notes", 2, 80, true)],
            truncated: true,
        })));
        assert!(text.contains("Some are not shown"), "{text}");
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
        answer(articles::PAIRING.to_owned(), None);
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
            (
                articles::PAIRING,
                Some(b"192.168.1.10:9331|abc234".to_vec()),
            ),
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
        assert!(words(&shown(commands)).contains("kobo sidekick init"));
    }

    #[test]
    fn an_empty_articles_shelf_says_where_articles_come_from() {
        let mut books = Books::empty();
        books.articles.computer = Some(("192.168.1.10:9331".to_owned(), "abc234".to_owned()));
        books.articles.loaded = true;
        books.view = View::Shelf(Shelf::Articles);
        let mut runner = AppRunner::new(books);
        let said = words(&shown(runner.start()));
        assert!(said.contains("No articles yet"), "{said}");
        assert!(said.contains("Sync"), "{said}");
    }
}

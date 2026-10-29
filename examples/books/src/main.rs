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
//! ## Why it reads the books
//!
//! The runtime lists files; it does not open them. The covers, the real
//! titles of books kept under a blob's name, and the authors are inside the
//! books, so Books reads each one once, a few at a time while the shelf is on
//! the panel, and keeps what it learned. See [`covers`].

mod covers;

use covers::{Cover, Described, Recent, Thumb};
use kobo_bookview::positions::{Progress, KEY as READING};
use kobo_bookview::{BookView, Step};
use kobo_read::Outcome;
use kobo_sdk::{
    action_id, document_preview, typographic_cover, ActionId, BandAlign, BannerLevel, Context,
    DeviceRequest, DeviceResult, Glyph, KoboApp, LibraryEntry, PictureHandle, Screen,
    ScreenBuilder, SlotWidth, Space, StoreResult, TaskId, TaskOutcome, Tile, TilePicture,
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
    /// Everything that opens here, recently opened first.
    Library,
    /// What was opened lately, newest first.
    Recent,
    /// Kobo Store books, PDFs and titles not yet downloaded.
    KoboReader,
}

impl Shelf {
    const ALL: [Self; 3] = [Self::Library, Self::Recent, Self::KoboReader];

    const fn name(self) -> &'static str {
        match self {
            Self::Library => "shelf-library",
            Self::Recent => "shelf-recent",
            Self::KoboReader => "shelf-kobo-reader",
        }
    }

    const fn title(self) -> &'static str {
        match self {
            Self::Library => "Library",
            Self::Recent => "Recent",
            Self::KoboReader => "Kobo reader",
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
            document.title.clone_from(title);
        }
        document.author.clone_from(&known.author);
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
            View::Reading => self.reading(),
            View::Unreadable(index) => self.unreadable(index),
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
        if self.documents.is_empty() {
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
                if members.is_empty() {
                    return None;
                }
                let handle = PILE_HANDLE + u32::try_from(place).unwrap_or(0);
                let picture = self.pile(context, handle, &members);
                Some((shelf, members.len(), picture))
            })
            .collect();
        screen = screen.section("Your books").tile_grid(
            TileShape::Square,
            piles.into_iter().map(|(shelf, count, picture)| {
                (
                    shelf.name(),
                    shelf.title(),
                    Glyph::Book,
                    move |tile: Tile| {
                        let tile = tile.with_subtitle(books_word(count));
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
                let mut members: Vec<usize> = opened
                    .into_iter()
                    .filter(|&index| self.documents[index].belongs_here())
                    .collect();
                for (index, document) in self.documents.iter().enumerate() {
                    if document.belongs_here() && !members.contains(&index) {
                        members.push(index);
                    }
                }
                members
            }
            Shelf::KoboReader => (0..self.documents.len())
                .filter(|&index| !self.documents[index].belongs_here())
                .collect(),
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
        self.place(context, handle, key, || Thumb {
            width,
            height,
            grey: document_preview(&document.preview, document.kind.badge(), width, height),
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
            let still_wanted = self.on_panel.contains(&id)
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
    }

    /// Reads a book's package for its name, author and cover, and keeps them.
    fn describe(&mut self, context: &mut Context, id: &str, bytes: &[u8]) {
        let Some(index) = self.documents.iter().position(|document| document.id == id) else {
            return;
        };
        let size = self.documents[index].bytes;
        let listed = self.documents[index].listed.clone();
        let description = kobo_doc::epub::describe(bytes).unwrap_or_default();
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
        if bytes.is_empty() {
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

    fn on_listing(&mut self, context: &mut Context, entries: Vec<LibraryEntry>, truncated: bool) {
        self.library_failed = false;
        self.truncated = truncated;
        self.take_documents(entries.into_iter().filter_map(from_entry).collect());
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
    /// The platform's own blue: this screen is Cobalt itself rather than an
    /// application running on it, and it wears the colour of the logo.
    fn accent(&self) -> Option<kobo_sdk::Accent> {
        Some(kobo_sdk::Accent::Cobalt)
    }

    fn on_start(&mut self, context: &mut Context) {
        context.store().load(READING);
        context.store().load(DESCRIBED);
        context.store().load(RECENT);
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
    }

    /// Keeps the place before the panel goes elsewhere, because an
    /// application in the background can be stopped without another word.
    fn on_background(&mut self, context: &mut Context) {
        self.keep_position(context);
        self.note_reading(context);
        if self.record == Record::Changed {
            self.save_described(context);
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
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if self.read_action(context, action) {
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

fn megabytes(bytes: u32) -> String {
    format!("{:.1} MB", f64::from(bytes) / (1024.0 * 1024.0))
}

/// The record a book's place is kept under.
///
/// The shared record names an article by the SHA-256 of something that
/// identifies it, and only that, so a library path never reaches the file.
/// The prefix keeps a book from ever sharing a record with anything else
/// that might one day be kept in the same file.
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
        title: entry.title,
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
        covers, fixtures, Books, Document, Kind, View, CONTINUE, DESCRIBED, READING, RECENT,
        SHELF_PAGE,
    };
    use kobo_sdk::prelude::*;
    use kobo_sdk::{
        document_preview, DeviceResult, LibraryEntry, StoreRequest, StoreResult, CLARA_BW_METRICS,
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

    /// Answers the three records Books asks for as it starts, in the order
    /// it asks: the store answers in order, whatever key a result names.
    fn answer_start(
        runner: &mut AppRunner<Books>,
        reading: Option<Vec<u8>>,
        described: Option<Vec<u8>>,
        recent: Option<Vec<u8>>,
    ) -> Vec<Command> {
        let mut commands = Vec::new();
        for (key, value) in [(READING, reading), (DESCRIBED, described), (RECENT, recent)] {
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
}

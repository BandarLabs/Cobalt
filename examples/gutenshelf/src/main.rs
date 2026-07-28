//! Gutenshelf: the Project Gutenberg library, on the device.
//!
//! Search sixty thousand public domain books, and read one without leaving the
//! application.
//!
//! ## Why plain text rather than EPUB
//!
//! Gutenberg publishes every book in several formats, and this reads the plain
//! text one. `kobo-doc` can read an EPUB now, so this is no longer a matter of
//! what can be parsed: it is that an EPUB is only useful whole, and a
//! half-downloaded zip is not a half-downloaded book. The plain text can be
//! read from the first byte, which is what lets the first page appear in about
//! a second on a radio this slow. What is lost is italics and a table of
//! contents.
//!
//! ## Why the reading screen is not built here
//!
//! It was, once: forty lines that turned pages and nothing else. Type size,
//! front light, bookmarks and marked passages are not gutenshelf's to invent
//! -- every application that shows a book wants the same ones, and a reader
//! who learns them in one should find them in the next. They live in
//! `kobo-read`.
//!
//! ## Why the book arrives in pieces
//!
//! The transport carries half a megabyte at most, and a Victorian novel is
//! several times that. Rather than refusing long books — which is most of the
//! interesting ones — this asks for the part it is about to need, using the
//! byte offset `Task::Fetch` carries. The first page therefore appears in
//! about a second, and the next piece is fetched while there are still pages
//! left to read.

use kobo_read::{Memory, Outcome, Reader};
use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Glyph, KoboApp, PictureHandle, ScreenBuilder,
    ShelfDownload, ShelfProgress, ShelfUpload, StoreResult, Task, TaskId, TaskOutcome, TilePicture,
    TileShape, MAX_STORE_VALUE,
};
use std::collections::BTreeMap;
use std::process::ExitCode;

/// The catalogue. Gutendex is a read-only JSON front end to Gutenberg's own
/// metadata, and is the only address this application chooses for itself; the
/// download host is whatever Gutendex names.
const CATALOGUE: &str = "https://gutendex.com/books";

/// How much of a catalogue response to accept. A page of results is around
/// thirty kilobytes; the rest is headroom for books with many editions.
const CATALOGUE_BYTES: u32 = 200 * 1024;

/// How much of a book to ask for at a time.
///
/// Around a hundred and fifty pages. Smaller would mean waiting mid-chapter;
/// larger risks the transport ceiling once headers are counted.
const CHUNK_BYTES: u32 = 256 * 1024;

/// How many placeholder rows stand in for the list while it is arriving.
///
/// Enough to look like the list that is coming, so the real rows land where
/// the eye is already looking rather than appearing to shift the screen.
const SKELETON_ROWS: u8 = 6;

/// The most books to keep from one response.
const MAX_RESULTS: usize = 16;

/// How much of a cover to accept.
///
/// Gutenberg's medium covers are around thirty kilobytes. The ceiling is what
/// stops a mis-typed URL pulling a megabyte down a slow radio for a thumbnail.
const COVER_BYTES: u32 = 512 * 1024;

/// How many books one shelf page holds.
///
/// Three columns of portrait tiles, two rows deep, which is what fits whole
/// between the bars on this panel. It used to be six in two columns of three,
/// and the third row was cut in half by the nav bar: the shelf showed four
/// books and a mistake. The grid itself is measured; this only decides where
/// the shelf is cut, so it has to agree with the grid or the same thing
/// happens again.
/// Cover fetches allowed at once.
///
/// One below the runtime's ceiling of four on purpose, so a shelf filling in
/// can never leave a search or a download with nowhere to go.
/// How tall the cover on a book's page is drawn.
///
/// A third of the panel: large enough to be the picture of the book rather
/// than a thumbnail of it, and small enough that the title, the author and the
/// Read button are all still on screen under it without scrolling.
const DETAILS_COVER_MM: u16 = 40;

const COVER_LANES: usize = 3;

/// Checked here rather than in a test, because the cost of getting it wrong is
/// a shelf that silently delays every search behind its own artwork, and that
/// is a mistake worth refusing to compile.
const _: () = assert!(COVER_LANES < kobo_sdk::MAX_TASKS_IN_FLIGHT);

/// Attempts spent on one cover before it is given up on.
const COVER_TRIES: u8 = 3;

const SHELF_PAGE: usize = 6;

/// How close to the end of what has been downloaded the reader may get before
/// the next piece is requested.
const TOP_UP_PAGES: usize = 2;

#[derive(Clone, Debug, Eq, PartialEq)]
struct Book {
    title: String,
    author: String,
    /// Where the plain text lives, when Gutenberg published one.
    text: Option<String>,
    /// Where the cover artwork lives, when Gutenberg published one.
    cover: Option<String>,
    /// The cover once it has been decoded and handed to the runtime.
    picture: Option<TilePicture>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum View {
    Results,
    Search,
    Details,
    Reading,
}

/// What the one exclusive request is for.
///
/// Covers are not in here. These two are the requests the reader is actually
/// waiting on, and only one can be outstanding: a catalogue and a book text
/// arriving together would need a screen that describes two kinds of waiting
/// at once. Cover fetches run alongside in their own lanes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Awaiting {
    Catalogue,
    Text,
}

struct Gutenshelf {
    view: View,
    keyboard: Keyboard,
    books: Vec<Book>,
    /// Which book is open, as an index into `books`.
    open: Option<usize>,
    query: Option<String>,
    /// The book being read, as far as it has been downloaded.
    text: String,
    /// The book, open, as far as it has been downloaded.
    ///
    /// Held rather than derived at draw time: the runtime states the panel
    /// during the handshake, so this cannot be built until the application is
    /// running, and repaginating a whole novel on every repaint would make
    /// every page turn slower than the one before it.
    reader: Option<Reader>,
    /// How many bytes of the book have been asked for so far.
    fetched: u32,
    /// Whether the download reached the end of the book.
    complete: bool,
    /// Which page of the shelf is showing.
    shelf: usize,
    /// Books whose covers are still to be fetched, most recent first.
    ///
    /// Only the covers for the shelf page being looked at are ever queued. A
    /// shelf of sixteen would otherwise hold the radio open for a dozen
    /// pictures the reader may never scroll to, and on a device that reads for
    /// weeks on a charge that is the difference between free and not.
    /// Covers still to fetch, each with the number of attempts already spent
    /// on it. Popped from the back, so a retry pushed to the front is tried
    /// last and one dead URL cannot starve the rest of the page.
    wanted: Vec<(usize, u8)>,
    /// Cover fetches in flight at once, which is the whole point: the panel
    /// spends its time waiting on the radio, not on the decoder.
    covers: Vec<(TaskId, usize, u8)>,
    task: Option<(TaskId, Awaiting)>,
    problem: Option<String>,
    /// Books already on the shelf, by blob name, with their size.
    ///
    /// A downloaded book is the expensive thing this application produces: it
    /// is a megabyte over a slow radio, and before this it was thrown away the
    /// moment somebody looked at another title. Held so a book can be opened
    /// again without the radio at all, and so the library can say which ones
    /// those are.
    stored: BTreeMap<String, u32>,
    /// Putting the open book onto the shelf, once it has all arrived.
    keeping: Option<ShelfUpload>,
    /// Taking a book back off the shelf, in place of downloading it.
    loading: Option<ShelfDownload>,
    /// Where the open book was last left, once the store has answered.
    ///
    /// Held rather than applied on arrival because the book and the place come
    /// from different places at different speeds: the text is on the radio and
    /// the position is on the card, and whichever lands second is the one that
    /// has to put them together.
    place: Option<Memory>,
    /// Covers the card is being asked about, by the key each answer will
    /// carry.
    ///
    /// Every cover is looked for here before it is looked for on the radio.
    /// Artwork does not change and a shelf page is the same six pictures every
    /// time it is opened, so fetching them again is seconds of somebody's life
    /// and a radio kept awake for something already on the device.
    looking: Vec<(String, usize)>,
}

impl Default for Gutenshelf {
    fn default() -> Self {
        Self {
            view: View::Results,
            keyboard: Keyboard::new(),
            books: Vec::new(),
            open: None,
            query: None,
            text: String::new(),
            reader: None,
            fetched: 0,
            complete: false,
            shelf: 0,
            wanted: Vec::new(),
            covers: Vec::new(),
            task: None,
            problem: None,
            stored: BTreeMap::new(),
            keeping: None,
            loading: None,
            place: None,
            looking: Vec::new(),
        }
    }
}

impl Gutenshelf {
    /// Whether the shelf itself is still on its way.
    ///
    /// Covers deliberately do not count. They arrive one at a time and the
    /// slowest of them decided how long the loading screen stayed up, so a
    /// shelf whose books had already arrived sat behind "Fetching the most
    /// popular books" until the last piece of artwork on the page resolved —
    /// and a cover that fails is silent, so it looked like nothing was
    /// happening at all.
    fn awaiting_catalogue(&self) -> bool {
        matches!(self.task, Some((_, Awaiting::Catalogue)))
    }

    fn awaiting_text(&self) -> bool {
        matches!(self.task, Some((_, Awaiting::Text)))
    }

    fn show(&self, context: &mut Context) {
        let screen = match self.view {
            View::Results => self.results(),
            View::Search => self.search(),
            View::Details => self.details(),
            View::Reading => self.reading(),
        };
        // Every view except the shelf was reached from another one, so Back
        // unwinds the application first and leaves it only from the shelf.
        // Without this the reader taps Back out of a book and lands at the
        // launcher, and coming back in shows the book again rather than the
        // shelf they were trying to reach.
        context.set_screen(screen.with_own_back(self.view != View::Results));
    }

    fn results(&self) -> kobo_sdk::Screen {
        let mut screen = ScreenBuilder::new("gutenshelf").top_bar(match &self.query {
            None => "Gutenshelf".to_owned(),
            Some(query) => format!("\u{201c}{query}\u{201d}"),
        });
        if let Some(problem) = &self.problem {
            screen = screen.banner(BannerLevel::Attention, problem.clone());
        }
        if self.awaiting_catalogue() {
            // A skeleton rather than only a label. The panel takes about a
            // second to repaint, so on a fast answer a one-line "loading"
            // message is on screen for less time than the refresh that draws
            // it, and reads as a flicker. A list-shaped placeholder in the
            // place the list will appear says what is coming, and the rows
            // that replace it land in the same position.
            return screen
                .divider()
                .activity(
                    match &self.query {
                        None => "Fetching the most popular books".to_owned(),
                        Some(query) => format!("Searching for {query}"),
                    },
                    None,
                )
                .skeleton(SKELETON_ROWS)
                .build();
        }
        if self.books.is_empty() {
            // The one screen where a full-width button belongs, because it is
            // the only thing on it and the only thing to do.
            return screen
                .text(
                    "Sixty thousand books, free and out of copyright. \
                     Search for an author or a title.",
                )
                .primary_button("search", "Search the library")
                .build();
        }
        let pages = self.shelf_pages();
        let first = self.shelf * SHELF_PAGE;
        let shown = self
            .books
            .iter()
            .enumerate()
            .skip(first)
            .take(SHELF_PAGE)
            .map(|(index, book)| {
                // A tick on a book already on the device. The tile is a cover
                // and a title with no room for a sentence, and the one thing
                // worth knowing before tapping is whether opening it costs a
                // download.
                // Said in words rather than with a tick. The reading face has
                // no check mark in it, and the platform refuses a screen
                // carrying a character it cannot draw -- which is the right
                // answer, because the alternative is a blank box on a shelf.
                let title = if self.is_kept(book) {
                    format!("{} (kept)", book.title)
                } else {
                    book.title.clone()
                };
                (format!("book-{index}"), title, Glyph::Book, book.picture)
            });
        screen = screen.picture_tiles(TileShape::Portrait, shown);
        if pages <= 1 {
            // Still reachable, just not at the cost of a whole row of covers.
            // Search used to be a full-width button above the shelf, which on
            // this panel was a third of the artwork the shelf exists to show.
            return screen.bottom_action("search", "Search").build();
        }
        // The same page controls the reader already uses inside a book, so a
        // shelf is turned the way a page is. Tapping the side of the panel
        // works here too, which is how every Kobo has always turned a page.
        screen
            .page_turns("shelf-back", "shelf-next")
            .nav_bar(
                None,
                [
                    ("shelf-back", "Back"),
                    ("search", "Search"),
                    ("shelf-next", "More"),
                ],
            )
            .build()
    }

    /// How many pages the shelf is cut into.
    fn shelf_pages(&self) -> usize {
        self.books.len().div_ceil(SHELF_PAGE).max(1)
    }

    fn search(&self) -> kobo_sdk::Screen {
        ScreenBuilder::new("gutenshelf-search")
            .top_bar("Search")
            .typed(&self.keyboard, "An author or a title")
            .keyboard(&self.keyboard, "Search")
            .build()
    }

    fn details(&self) -> kobo_sdk::Screen {
        let Some(book) = self.open.and_then(|index| self.books.get(index)) else {
            return self.results();
        };
        let mut screen = ScreenBuilder::new("gutenshelf-book")
            .top_bar(book.title.clone())
            .heading(book.title.clone())
            .secondary(book.author.clone());
        // The same cover the shelf already fetched, at a fixed height so every
        // book's page has the same shape whatever its artwork happens to be.
        // Given in millimetres rather than pixels, so it is a third of the
        // panel on any device rather than a third of one particular one.
        if let Some(picture) = book.picture {
            screen = screen.picture(picture, DETAILS_COVER_MM);
        }
        if let Some(problem) = &self.problem {
            screen = screen.banner(BannerLevel::Attention, problem.clone());
        }
        if self.awaiting_text() || self.loading.is_some() {
            return screen.activity("Downloading", None).build();
        }
        if self.is_kept(book) {
            // Worth saying plainly: it is the difference between opening now
            // and waiting on a megabyte over a radio, and it is the only way
            // to tell that this book costs nothing to open again.
            screen = screen.secondary("Already on this device.");
        }
        screen = if book.text.is_some() {
            // The one thing this page is for, so it is the one control filled.
            screen.primary_button("read", "Read")
        } else {
            // Said plainly rather than by disabling a button. A button that
            // does nothing when tapped reads as a broken panel.
            screen.secondary("Gutenberg has no plain text edition of this book.")
        };
        screen.button("results", "Back to the results").build()
    }

    fn reading(&self) -> kobo_sdk::Screen {
        let title = self
            .open
            .and_then(|index| self.books.get(index))
            .map_or_else(|| "Reading".to_owned(), |book| book.title.clone());
        let Some(reader) = &self.reader else {
            return ScreenBuilder::new("gutenshelf-reading")
                .reading(true)
                .top_bar(title)
                .activity("Downloading", None)
                .build();
        };
        reader.screen(&title)
    }

    fn ask_catalogue(&mut self, context: &mut Context, query: Option<&str>) {
        let url = match query {
            None => format!("{CATALOGUE}?sort=popular"),
            Some(query) => format!("{CATALOGUE}?search={}", encode_query(query)),
        };
        self.problem = None;
        match context.spawn(Task::Fetch {
            url,
            offset: 0,
            max_bytes: CATALOGUE_BYTES,
        }) {
            Some(task) => self.task = Some((task, Awaiting::Catalogue)),
            None => self.problem = Some("Too much is already in flight.".to_owned()),
        }
    }

    /// Queues the covers for the shelf page being looked at, then starts as
    /// many as the runtime will carry.
    ///
    /// Several at once, not one after another. The earlier version chained
    /// them to spend exactly one full refresh on the finished page, which is
    /// the right instinct on a panel that flashes when it repaints — but six
    /// covers fetched end to end meant six round trips over a slow radio, and
    /// the shelf sat empty for the whole of it. Fetching in parallel and still
    /// painting only when a batch lands keeps the refresh count to two while
    /// cutting the wait to a third.
    fn want_covers(&mut self, context: &mut Context) {
        let first = self.shelf * SHELF_PAGE;
        self.wanted.clear();
        self.looking = self
            .books
            .iter()
            .enumerate()
            .skip(first)
            .take(SHELF_PAGE)
            .filter(|(_, book)| book.picture.is_none())
            .filter_map(|(index, book)| Some((cover_key(book.cover.as_ref()?), index)))
            .rev()
            .collect();
        // The card first, every time, and the radio only for what it does not
        // have. Both answers come back through `on_store`, so the fetch starts
        // when the last lookup has answered rather than racing it.
        let keys = self
            .looking
            .iter()
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        for key in keys {
            context.store().load(key);
        }
        if self.looking.is_empty() {
            self.ask_cover(context);
        }
    }

    /// Takes one cover lookup off the list, and starts fetching when the last
    /// of them has answered.
    ///
    /// Nothing is fetched while a lookup is outstanding. Starting the radio
    /// per miss would work, but it would also paint the shelf once per cover:
    /// the batch is what keeps a page of artwork to two refreshes.
    fn looked_for_cover(&mut self, context: &mut Context, key: &str, found: Option<Vec<u8>>) {
        let Some(at) = self.looking.iter().position(|(held, _)| held == key) else {
            return;
        };
        let (_, index) = self.looking.remove(at);
        // A cached cover that will not decode is not a cover. Forgetting it
        // rather than keeping it means one bad write cannot make a book
        // pictureless for as long as the device lasts.
        let took = found.is_some_and(|bytes| self.took_cover(context, index, &bytes));
        if !took {
            if let Some(book) = self.books.get(index) {
                if let Some(key) = book.cover.as_ref().map(cover_key) {
                    context.store().forget(key);
                }
            }
            self.wanted.push((index, 0));
        }
        if !self.looking.is_empty() {
            return;
        }
        self.ask_cover(context);
        if self.covers.is_empty() {
            // Everything the page needed was already here. Nothing else is
            // going to arrive, so this is the paint.
            self.show(context);
        }
    }

    /// Fills the cover lanes from the queue.
    ///
    /// One fewer lane than the runtime allows, always. The spare is what lets
    /// a search or a download start straight away instead of queueing behind
    /// a shelf of artwork.
    fn ask_cover(&mut self, context: &mut Context) {
        while self.covers.len() < COVER_LANES {
            let Some((index, tries)) = self.wanted.pop() else {
                return;
            };
            let Some(url) = self.books.get(index).and_then(|book| book.cover.clone()) else {
                continue;
            };
            if let Some(task) = context.spawn(Task::Fetch {
                url,
                offset: 0,
                max_bytes: COVER_BYTES,
            }) {
                self.covers.push((task, index, tries));
                continue;
            }
            // Out of slots rather than out of covers. Put it back and let the
            // next arrival try again, so a busy moment loses nothing.
            self.wanted.push((index, tries));
            return;
        }
    }

    /// Takes a cover task off the in-flight list, if it is one of ours.
    fn finish_cover(&mut self, task: TaskId) -> Option<(usize, u8)> {
        let at = self.covers.iter().position(|(id, _, _)| *id == task)?;
        let (_, index, tries) = self.covers.remove(at);
        Some((index, tries))
    }

    /// Puts a cover that did not arrive back in the queue, up to a point.
    ///
    /// Gutendex serves these from a CDN that intermittently refuses, and the
    /// same URL asked again a moment later usually works. Retried quietly and
    /// a bounded number of times: a reader who cannot see a thumbnail is not
    /// helped by being told about it, and an unbounded retry would keep the
    /// radio awake for a cover that is genuinely gone.
    fn retry_cover(&mut self, index: usize, tries: u8) {
        if tries + 1 < COVER_TRIES {
            self.wanted.insert(0, (index, tries + 1));
        }
    }

    /// Decodes one cover and hands it to the runtime at the size it will be
    /// drawn.
    ///
    /// Fitting here rather than letting the renderer shrink it is what keeps
    /// the picture cache honest: a shelf of full-size covers is two megabytes
    /// of pixels that are averaged away on every paint.
    fn took_cover(&mut self, context: &mut Context, index: usize, bytes: &[u8]) -> bool {
        let (cell_width, cell_height) = context.metrics().tile_body(TileShape::Portrait);
        let Ok(cell_width) = u32::try_from(cell_width) else {
            return false;
        };
        let Ok(cell_height) = u32::try_from(cell_height) else {
            return false;
        };
        let Ok(picture) = kobo_image::decode(bytes) else {
            // A cover that will not decode is not worth telling the reader
            // about: the tile keeps its glyph and the book is still readable.
            return false;
        };
        // Enlarging rather than merely shrinking: Gutenberg publishes covers at
        // around 190 by 300, and a tile on this panel is more than twice that,
        // so fitting alone left every cover as a stamp in an empty cell.
        let Ok(mut picture) = picture.fit_enlarging(cell_width, cell_height) else {
            return false;
        };
        // Halftoned to the levels this panel actually resolves. Without it the
        // smooth gradients in cover art band into visible steps, which looks
        // like a decoding fault rather than a limitation of the display.
        picture.dither(kobo_image::PANEL_GREYS);
        let handle = PictureHandle(u32::try_from(index).unwrap_or(0));
        let (width, height) = (picture.width(), picture.height());
        let Some(reference) = context.put_picture(handle, width, height, picture.into_grey())
        else {
            return false;
        };
        if let Some(book) = self.books.get_mut(index) {
            book.picture = Some(reference);
        }
        true
    }

    /// Draws a cover that came off the radio, and keeps it for next time.
    ///
    /// Cached as it arrived rather than as it is drawn: the fetched bytes are
    /// a compressed JPEG of about thirty kilobytes and the drawn form is a
    /// quarter of a megabyte of pixels, and the expensive part was never the
    /// decode -- it was the round trip. Kept only if it drew, so a URL that
    /// answers with something that is not a picture is not cached as one.
    fn keep_cover(&mut self, context: &mut Context, index: usize, bytes: &[u8]) {
        if !self.took_cover(context, index, bytes) {
            return;
        }
        if bytes.len() > MAX_STORE_VALUE {
            return;
        }
        let Some(key) = self
            .books
            .get(index)
            .and_then(|book| book.cover.as_ref())
            .map(cover_key)
        else {
            return;
        };
        // Best effort and unwatched. A cache write that fails costs a fetch
        // next time and nothing else, and there is nothing a reader could do
        // about it if they were told.
        context.store().save(key, bytes.to_vec());
    }

    /// Refills the lanes, and repaints each time they all drain.
    ///
    /// Not on every arrival. Each repaint is a full panel refresh the reader
    /// watches happen, so painting per cover would flash six times for one
    /// shelf. Painting when a batch completes gives the shelf in two steps,
    /// which reads as filling in rather than as a fault.
    fn next_cover(&mut self, context: &mut Context) {
        self.ask_cover(context);
        if self.covers.is_empty() {
            self.show(context);
        }
    }

    /// Opens the book, from the device if it is here and the radio if not.
    fn get_text(&mut self, context: &mut Context) {
        let kept = self
            .open
            .and_then(|index| self.books.get(index))
            .is_some_and(|book| self.is_kept(book));
        if kept {
            if let Some((blob, _)) = self.open_names() {
                let mut download = ShelfDownload::new(blob);
                download.start(context);
                self.loading = Some(download);
                self.problem = None;
                return;
            }
        }
        self.ask_text(context);
    }

    fn ask_text(&mut self, context: &mut Context) {
        let Some(url) = self
            .open
            .and_then(|index| self.books.get(index))
            .and_then(|book| book.text.clone())
        else {
            self.problem = Some("This book has no plain text edition.".to_owned());
            return;
        };
        self.problem = None;
        match context.spawn(Task::Fetch {
            url,
            offset: self.fetched,
            max_bytes: CHUNK_BYTES,
        }) {
            Some(task) => self.task = Some((task, Awaiting::Text)),
            None => self.problem = Some("Too much is already in flight.".to_owned()),
        }
    }

    /// Fetches more of the book while there are still pages left to read.
    ///
    /// A request takes a second or two, and an e-reader that pauses at a page
    /// turn feels broken in a way that one which never pauses does not.
    fn top_up(&mut self, context: &mut Context) {
        if self.complete || self.task.is_some() {
            return;
        }
        let left = self.reader.as_ref().map_or(0, |reader| {
            reader.page_count().saturating_sub(reader.page_number())
        });
        if left <= TOP_UP_PAGES {
            self.ask_text(context);
        }
    }

    fn open_book(&mut self, context: &mut Context, index: usize) {
        self.open = Some(index);
        self.view = View::Details;
        // A different book, so nothing about the last one survives.
        self.text.clear();
        self.reader = None;
        self.place = None;
        self.loading = None;
        self.fetched = 0;
        self.complete = false;
        self.problem = None;
        if let Some((_, place)) = self.open_names() {
            // Asked now rather than when Read is tapped, so the position is
            // already here by the time the first page is.
            context.store().load(place);
        }
        self.show(context);
    }

    /// The blob name for a book, and the key its place is kept under.
    ///
    /// Derived from the text URL rather than from the title: two editions of
    /// the same novel are different files with different page breaks, and a
    /// place kept under a title would drop somebody into the wrong one. The
    /// URL is the only thing Gutenberg gives that names an edition.
    fn names(book: &Book) -> Option<(String, String)> {
        let url = book.text.as_ref()?;
        let stamp = stamp(url);
        Some((format!("book-{stamp:08x}"), format!("place-{stamp:08x}")))
    }

    fn open_names(&self) -> Option<(String, String)> {
        self.open
            .and_then(|index| self.books.get(index))
            .and_then(Self::names)
    }

    /// Whether this book is already on the device.
    fn is_kept(&self, book: &Book) -> bool {
        Self::names(book).is_some_and(|(blob, _)| self.stored.contains_key(&blob))
    }

    /// Offers an action to the open book, and says whether it was its.
    ///
    /// The reader owns every control on its own screen, so this is asked
    /// before any of the library's: a book is what is on the panel, and
    /// nothing else can be tapped from there.
    fn read_action(&mut self, context: &mut Context, action: ActionId) -> bool {
        let metrics = context.metrics();
        let Some(reader) = &mut self.reader else {
            return false;
        };
        match reader.act_on(action, &metrics) {
            Outcome::Elsewhere => return false,
            Outcome::Close => self.view = View::Details,
            Outcome::Light(level) => {
                context.device().set_frontlight(level);
                self.save_place(context);
            }
            Outcome::Save => {
                self.save_place(context);
                self.top_up(context);
            }
            Outcome::Repaint => {}
        }
        self.show(context);
        true
    }

    /// Keeps where the reader is, so the book opens there next time.
    fn save_place(&mut self, context: &mut Context) {
        let Some((_, place)) = self.open_names() else {
            return;
        };
        let Some(reader) = &self.reader else {
            return;
        };
        let memory = reader.memory().encode();
        context.store().save(place, memory);
    }

    /// Puts a finished book on the shelf, so it never has to be fetched again.
    fn keep_book(&mut self, context: &mut Context) {
        let Some((blob, _)) = self.open_names() else {
            return;
        };
        if self.stored.contains_key(&blob) || self.text.is_empty() {
            return;
        }
        let mut upload = ShelfUpload::new(blob, self.text.clone().into_bytes());
        upload.start(context);
        self.keeping = Some(upload);
    }

    /// Rebuilds the open book from everything downloaded so far.
    ///
    /// The reader's memory carries across rather than being rebuilt with it.
    /// A position is a block index, and appending to a book does not renumber
    /// what came before it -- so a chunk landing while somebody is reading
    /// chapter two leaves them in chapter two, which is the whole reason a
    /// position is stored that way.
    fn reopen(&mut self, context: &Context) {
        let memory = self.reader.as_ref().map_or_else(
            || self.place.clone().unwrap_or_default(),
            |reader| reader.memory().clone(),
        );
        // Named as text so it is read as text. The bytes are Gutenberg's plain
        // edition whatever the URL happened to end in.
        let cleaned = readable(&self.text);
        match kobo_doc::read("book.txt", cleaned.as_bytes()) {
            Ok(document) => {
                self.reader = Some(Reader::open(document, memory, &context.metrics()));
            }
            Err(_) => self.problem = Some("This book could not be read.".to_owned()),
        }
    }

    fn took_catalogue(&mut self, bytes: &[u8]) {
        match std::str::from_utf8(bytes)
            .ok()
            .and_then(|body| kobo_json::parse(body).ok())
        {
            None => self.problem = Some("Gutenberg's answer could not be read.".to_owned()),
            Some(value) => {
                self.books = books_from(&value);
                if self.books.is_empty() {
                    self.problem = Some("Nothing matched that search.".to_owned());
                }
            }
        }
        self.view = View::Results;
    }

    fn took_text(&mut self, context: &mut Context, bytes: &[u8]) {
        // Lossy on purpose. Gutenberg's plain text is usually UTF-8 but not
        // always, and a book that will not open because of one bad byte in
        // chapter forty is worse than a book with one odd character in it.
        let piece = String::from_utf8_lossy(bytes);
        // A short answer means the server had nothing more to give, which is
        // the only reliable end-of-book signal a ranged request produces.
        if bytes.len() < CHUNK_BYTES as usize {
            self.complete = true;
        }
        self.fetched = self
            .fetched
            .saturating_add(u32::try_from(bytes.len()).unwrap_or(CHUNK_BYTES));
        self.text.push_str(&piece);
        // Measured against the panel the runtime named, using its own wrapping
        // and line height, so a page that fits here is drawn whole. A page
        // that does not is not truncated on screen with a warning: the layout
        // simply stops, and the last paragraph is never drawn at all.
        //
        // Cleaned from the whole accumulated text rather than piece by piece,
        // because every marker this looks for can be split down the middle by
        // a chunk boundary.
        // Measured in the face it will be set in. A book is drawn in a serif
        // with book leading, which is wider and taller than the interface
        // face, so paginating with the default would run every page past the
        // bottom of the panel and lose its last lines.
        self.reopen(context);
        if self.complete {
            self.keep_book(context);
        }
        self.view = View::Reading;
    }
}

impl KoboApp for Gutenshelf {
    fn on_start(&mut self, context: &mut Context) {
        // Asked first so the shelf can mark what is already here by the time
        // the catalogue lands. A book already on the device opens instantly
        // and without the radio, and that is worth saying before somebody
        // waits on a download they did not need.
        context.shelf().list();
        self.ask_catalogue(context, None);
        self.show(context);
    }

    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        if let Some(upload) = &mut self.keeping {
            match upload.advance(context, &result) {
                ShelfProgress::Done => {
                    let name = upload.name().to_owned();
                    let size = u32::try_from(self.text.len()).unwrap_or(u32::MAX);
                    self.stored.insert(name, size);
                    self.keeping = None;
                    return;
                }
                // Quiet on purpose. Keeping a copy is a convenience; failing
                // at it does not stop anyone reading the book they have, and a
                // warning about the card being full over an open novel is an
                // interruption that helps nobody.
                ShelfProgress::Failed(_) => {
                    self.keeping = None;
                    return;
                }
                ShelfProgress::Moving { .. } => return,
                // Not this transfer's answer. Falling through matters: a book
                // being copied onto the shelf runs alongside the store answer
                // that says where the reader left it, and swallowing that here
                // dropped somebody back at page one of a book they were
                // halfway through.
                ShelfProgress::Elsewhere => {}
            }
        }
        if let Some(download) = &mut self.loading {
            match download.advance(context, &result) {
                ShelfProgress::Done => {
                    let bytes = self.loading.take().expect("a download in progress").take();
                    self.text = String::from_utf8_lossy(&bytes).into_owned();
                    self.fetched = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
                    self.complete = true;
                    self.reopen(context);
                    self.view = View::Reading;
                    self.show(context);
                    return;
                }
                ShelfProgress::Failed(_) => {
                    // The copy is gone or unreadable, so it is no longer a
                    // copy: forget it and fetch, rather than telling somebody
                    // their own device has lost their book.
                    if let Some((blob, _)) = self.open_names() {
                        self.stored.remove(&blob);
                        context.shelf().remove(blob);
                    }
                    self.loading = None;
                    self.ask_text(context);
                    self.show(context);
                    return;
                }
                ShelfProgress::Moving { .. } => return,
                ShelfProgress::Elsewhere => {}
            }
        }
        match result {
            StoreResult::Shelf(blobs) => {
                self.stored = blobs.into_iter().collect();
                self.show(context);
            }
            // Matched by key, not by shape. Both a cover and a reading
            // position come back as bytes under a key, and reading artwork as
            // a position would put somebody at a page chosen by a JPEG.
            StoreResult::Loaded { key, value }
                if self.looking.iter().any(|(held, _)| *held == key) =>
            {
                self.looked_for_cover(context, &key, value);
            }
            StoreResult::Loaded {
                value: Some(value), ..
            } => {
                // A place arriving for a book that is not open any more is
                // simply late; the reader it was meant for is gone.
                let memory = Memory::decode(&value);
                if let Some(reader) = &mut self.reader {
                    let metrics = context.metrics();
                    reader.restore(memory.clone(), &metrics);
                    self.show(context);
                }
                self.place = Some(memory);
            }
            _ => {}
        }
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if action == ActionId::BACK {
            // Delivered only on a screen that claimed it, so the shelf is
            // never reached here and Back still leaves the application from
            // there. Reading returns to the book it was opened from; a book
            // and the keyboard both return to the shelf.
            if self.view == View::Reading {
                // Back closes whatever is over the book before it closes the
                // book. Otherwise somebody who opened their notes to look
                // something up is thrown out of the book for asking to leave
                // the list.
                let metrics = context.metrics();
                if let Some(reader) = &mut self.reader {
                    if reader.chrome() != kobo_read::Chrome::Hidden {
                        reader.set_chrome(kobo_read::Chrome::Hidden, &metrics);
                        self.show(context);
                        return;
                    }
                }
            }
            self.view = match self.view {
                View::Reading if self.open.is_some() => View::Details,
                _ => View::Results,
            };
            self.problem = None;
            self.show(context);
            return;
        }
        if self.view == View::Search {
            match self.keyboard.press(action) {
                Some(Pressed::Submitted) => {
                    let query = self.keyboard.take();
                    let query = query.trim().to_owned();
                    if query.is_empty() {
                        self.view = View::Results;
                    } else {
                        self.query = Some(query.clone());
                        self.ask_catalogue(context, Some(&query));
                        self.view = View::Results;
                    }
                    self.show(context);
                    return;
                }
                Some(Pressed::Edited | Pressed::Shifted) => {
                    self.show(context);
                    return;
                }
                None => {}
            }
        }

        if action == action_id("search") {
            self.view = View::Search;
            self.show(context);
            return;
        }
        if action == action_id("results") {
            self.view = View::Results;
            self.show(context);
            return;
        }
        if action == action_id("read") {
            self.view = View::Reading;
            self.get_text(context);
            self.show(context);
            return;
        }
        if self.view == View::Reading && self.read_action(context, action) {
            return;
        }

        if action == action_id("shelf-next") || action == action_id("shelf-back") {
            let pages = self.shelf_pages();
            self.shelf = if action == action_id("shelf-next") {
                (self.shelf + 1).min(pages - 1)
            } else {
                self.shelf.saturating_sub(1)
            };
            // Painted before the covers are asked for, so turning the shelf is
            // immediate and the artwork arrives into a page that is already up.
            self.show(context);
            self.want_covers(context);
            return;
        }

        for index in 0..self.books.len() {
            if action == action_id(&format!("book-{index}")) {
                self.open_book(context, index);
                return;
            }
        }
    }

    fn on_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
        // Covers first, and separately. They arrive several at a time and out
        // of order, so they cannot share the single slot the catalogue and the
        // book text take turns in.
        if let Some((index, tries)) = self.finish_cover(task) {
            match outcome {
                TaskOutcome::Completed(bytes) => self.keep_cover(context, index, &bytes),
                // Silent on purpose. A missing cover leaves a tile with its
                // glyph, which is a shelf that still works; a banner about
                // artwork over a usable library is noise.
                TaskOutcome::Failed(_) => self.retry_cover(index, tries),
                TaskOutcome::Cancelled => {}
            }
            self.next_cover(context);
            return;
        }
        let Some((outstanding, awaiting)) = self.task else {
            return;
        };
        if outstanding != task {
            return;
        }
        self.task = None;
        match outcome {
            TaskOutcome::Completed(bytes) => match awaiting {
                Awaiting::Catalogue => {
                    self.took_catalogue(&bytes);
                    self.want_covers(context);
                }
                Awaiting::Text => self.took_text(context, &bytes),
            },
            TaskOutcome::Failed(error) => {
                // Named rather than summarised. "Not found" and "the network
                // could not be reached" call for completely different things
                // from the reader.
                self.problem = Some(format!("That did not work: {error}."));
                // A book that failed to download must not be shown as if it
                // had, and an empty reading screen is a dead end.
                if awaiting == Awaiting::Text && self.text.is_empty() {
                    self.view = View::Details;
                }
            }
            TaskOutcome::Cancelled => {
                self.problem = Some("Cancelled.".to_owned());
            }
        }
        self.show(context);
    }
}

/// Turns Project Gutenberg's plain text into something worth reading on a
/// panel.
///
/// The files are typeset for a 70 column terminal in 1971 and it shows. What
/// arrives is a licence, a title page laid out with runs of spaces, captions
/// for illustrations that are not in the file, and italics marked with
/// underscores. Handed straight to the typesetter that becomes twelve pages of
/// legal text before chapter one, lines like `PRIDE.        and PREJUDICE`
/// broken wherever the wrapping happened to land, and `_unhesitatingly_` with
/// its markup showing.
///
/// Nothing here is guesswork about prose. Every rule keys on a marker Gutenberg
/// actually writes, and each one leaves the text alone when its marker is
/// missing, so a file that does not follow the convention is passed through
/// rather than mangled.
/// A short, stable name for a URL.
///
/// Not a security hash and not required to be one: it names a file this
/// application wrote for itself, and the worst a collision does is offer the
/// wrong book, which the title on the details page makes obvious.
/// The cache key a cover is held under.
///
/// Keyed on the artwork's own URL, not on the book: two editions have
/// different covers, and a key that said "this book" would show one of them
/// under the other's title.
fn cover_key(url: impl AsRef<str>) -> String {
    kobo_sdk::cache_key(format!("cover.{:08x}", stamp(url.as_ref())))
}

fn stamp(url: &str) -> u32 {
    let mut hash = 0x811c_9dc5_u32;
    for byte in url.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

fn readable(raw: &str) -> String {
    let body = between_markers(raw);
    let mut out = String::with_capacity(body.len());
    for line in body.lines() {
        // Underscores are Gutenberg's italics and are not punctuation anybody
        // writes in a sentence, so they come out wherever they are. They are
        // frequently misplaced against the spaces around them — `for_ Pride
        // and Prejudice _unhesitatingly` — which is why matching them in pairs
        // would leave as many behind as it removed.
        let line = line.replace('_', "");
        // A run of spaces is a title page pretending to be a table. On a panel
        // that rewraps everything it is either a ragged gap in the middle of a
        // line or a line break in the wrong place, and one space says the same
        // thing without either.
        let collapsed = collapse_runs(&line);
        let trimmed = collapsed.trim();
        if trimmed.is_empty() {
            // Kept, because a blank line is the only paragraph mark the format
            // has. Collapsed to one so the page count is not padded out by the
            // four blank lines Gutenberg leaves around a chapter heading.
            if !out.ends_with("\n\n") && !out.is_empty() {
                out.push('\n');
            }
            continue;
        }
        out.push_str(trimmed);
        out.push('\n');
    }
    strip_illustrations(&out)
}

/// Narrows the file to the book, when it says where the book is.
///
/// Gutenberg brackets the text with `*** START OF ...` and `*** END OF ...`
/// lines. Everything outside them is the licence and the credits: worth
/// keeping in the file, not worth eleven pages of a reader's evening before
/// chapter one.
fn between_markers(raw: &str) -> &str {
    let mut body = raw;
    if let Some(start) = marker_line(body, "*** START") {
        body = &body[start..];
    }
    if let Some(end) = marker_line(body, "*** END") {
        body = &body[..end];
    }
    body
}

/// Finds where the line carrying a marker ends, or where it begins for an end
/// marker. Returns a byte offset that is always a line boundary.
fn marker_line(text: &str, marker: &str) -> Option<usize> {
    let mut offset = 0;
    for line in text.lines() {
        let upper = line.trim_start().to_uppercase();
        if upper.starts_with(marker) {
            return Some(if marker.starts_with("*** S") {
                // Past this line, so the marker itself is not read as prose.
                (offset + line.len() + 1).min(text.len())
            } else {
                offset
            });
        }
        offset += line.len() + 1;
    }
    None
}

/// Removes `[Illustration: ...]` captions, which describe pictures the plain
/// text edition does not contain.
///
/// Bracket counted rather than matched line by line, because the caption
/// regularly runs over several lines and can carry brackets of its own. An
/// unclosed bracket — a truncated download, most likely — puts the text back
/// exactly as it was rather than swallowing the rest of the book.
fn strip_illustrations(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = find_illustration(rest) {
        let (before, from) = rest.split_at(at);
        out.push_str(before);
        let mut depth = 0_u32;
        let mut end = None;
        for (index, character) in from.char_indices() {
            match character {
                '[' => depth += 1,
                ']' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(index + 1);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(end) = end else {
            out.push_str(from);
            return out;
        };
        rest = &from[end..];
    }
    out.push_str(rest);
    out
}

/// Where the next illustration caption opens, if there is one.
fn find_illustration(text: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(at) = text[from..].find('[') {
        let at = from + at;
        let after = &text[at + 1..];
        let head: String = after.chars().take("illustration".len()).collect();
        if head.eq_ignore_ascii_case("illustration") {
            return Some(at);
        }
        from = at + 1;
    }
    None
}

/// Squeezes runs of spaces and tabs down to one.
fn collapse_runs(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut in_run = false;
    for character in line.chars() {
        if character == ' ' || character == '\t' {
            if !in_run {
                out.push(' ');
                in_run = true;
            }
        } else {
            out.push(character);
            in_run = false;
        }
    }
    out
}

/// Percent-encodes a search term.
///
/// Everything outside the unreserved set is escaped, which is stricter than a
/// query string strictly needs. A search box is the one place a reader's text
/// goes straight into a URL, and being generous about what to leave alone
/// there is how a stray `&` turns into a parameter somebody did not ask for.
fn encode_query(query: &str) -> String {
    let mut encoded = String::new();
    for byte in query.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            const HEX: &[u8; 16] = b"0123456789ABCDEF";
            encoded.push('%');
            encoded.push(char::from(HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
    }
    encoded
}

/// Reads Gutendex's answer into the handful of fields this application shows.
fn books_from(value: &kobo_json::Value) -> Vec<Book> {
    let Some(results) = value.get("results").and_then(kobo_json::Value::as_array) else {
        return Vec::new();
    };
    results
        .iter()
        .take(MAX_RESULTS)
        .filter_map(|entry| {
            let title = entry.get("title").and_then(kobo_json::Value::as_str)?;
            Some(Book {
                title: title.to_owned(),
                author: authors_of(entry),
                text: plain_text_url(entry),
                cover: cover_url(entry),
                picture: None,
            })
        })
        .collect()
}

fn authors_of(entry: &kobo_json::Value) -> String {
    let names = entry
        .get("authors")
        .and_then(kobo_json::Value::as_array)
        .map(|authors| {
            authors
                .iter()
                .filter_map(|author| author.get("name").and_then(kobo_json::Value::as_str))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if names.is_empty() {
        // Gutenberg has a great many anonymous works, and a blank line under a
        // title reads as a rendering fault.
        "Unknown".to_owned()
    } else {
        names.join(", ")
    }
}

/// Picks the plain text edition, if there is one.
///
/// Gutenberg spells the format several ways — with a UTF-8 charset, with an
/// ASCII one, and with none — and also publishes zipped copies under names
/// that begin the same way. Matching on the prefix and then rejecting archives
/// is what covers all of it without listing every spelling.
fn plain_text_url(entry: &kobo_json::Value) -> Option<String> {
    let kobo_json::Value::Object(formats) = entry.get("formats")? else {
        return None;
    };
    formats
        .iter()
        .filter(|(kind, _)| kind.starts_with("text/plain"))
        .filter_map(|(_, url)| url.as_str())
        // Only `https`, because the runtime refuses anything else and a book
        // that fails at download time has already cost the reader a tap.
        .find(|url| !url.to_ascii_lowercase().ends_with(".zip") && url.starts_with("https://"))
        .map(str::to_owned)
}

/// Picks the cover artwork, if Gutenberg published one.
///
/// Gutendex lists two sizes under the same `image/jpeg` type — a small
/// thumbnail and a medium cover — so the URL is what distinguishes them. The
/// medium one is around 190 by 300, which is a little over half a tile on this
/// panel and the one worth enlarging; the small one is too coarse for it.
fn cover_url(entry: &kobo_json::Value) -> Option<String> {
    let kobo_json::Value::Object(formats) = entry.get("formats")? else {
        return None;
    };
    let covers = formats
        .iter()
        .filter(|(kind, _)| kind.starts_with("image/"))
        .filter_map(|(_, url)| url.as_str())
        .filter(|url| url.starts_with("https://"))
        .collect::<Vec<_>>();
    covers
        .iter()
        .find(|url| url.contains(".cover.medium."))
        .or_else(|| covers.first())
        .map(|url| (*url).to_owned())
}

fn main() -> ExitCode {
    match kobo_sdk::run("gutenshelf", Gutenshelf::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("gutenshelf: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        books_from, encode_query, plain_text_url, readable, Awaiting, Gutenshelf, Memory, Reader,
        View, COVER_TRIES,
    };
    use kobo_sdk::{action_id, AppRunner, Command, StoreRequest, StoreResult};
    use kobo_ui::{Chrome, LayoutKind, CLARA_BW_METRICS};

    /// Verbatim from the Pride and Prejudice file, which is what put twelve
    /// pages of licence and a table-of-contents-shaped title page in front of
    /// chapter one on the panel.
    const RAW: &str = "\
The Project Gutenberg eBook of Pride and Prejudice\n\
\n\
This eBook is for the use of anyone anywhere in the United States and\n\
most other parts of the world at no cost and with almost no restrictions\n\
whatsoever.\n\
\n\
Title: Pride and Prejudice\n\
\n\
*** START OF THE PROJECT GUTENBERG EBOOK PRIDE AND PREJUDICE ***\n\
[Illustration:\n\
\n\
 GEORGE ALLEN                    PUBLISHER\n\
\n\
        156 CHARING CROSS ROAD LONDON\n\
                                            ]\n\
\n\
PRIDE.                    and PREJUDICE\n\
\n\
It is a truth universally acknowledged, that a single man in\n\
possession of a good fortune, must be in want of a wife. I, for my\n\
part, declare for_ Pride and Prejudice _unhesitatingly.\n\
\n\
*** END OF THE PROJECT GUTENBERG EBOOK PRIDE AND PREJUDICE ***\n\
\n\
Please read this before you distribute or use this work.\n";

    #[test]
    fn gutenbergs_typesetting_for_a_1971_terminal_is_undone() {
        let clean = readable(RAW);

        // The licence at both ends is the reader's evening, not their book.
        assert!(
            !clean.contains("almost no restrictions"),
            "the header survived: {clean}"
        );
        assert!(
            !clean.contains("before you distribute"),
            "the footer survived: {clean}"
        );
        assert!(!clean.contains("*** START"), "the marker is not prose");
        assert!(!clean.contains("*** END"));

        // A caption for a picture that is not in a plain text file, spanning
        // lines and carrying its own layout.
        assert!(
            !clean.contains("GEORGE ALLEN") && !clean.contains("Illustration"),
            "the illustration survived: {clean}"
        );

        // Runs of spaces are a title page pretending to be a table.
        assert!(
            clean.contains("PRIDE. and PREJUDICE"),
            "the run was not collapsed: {clean}"
        );

        // Italics markup, including the misplaced pair that made the panel
        // read `for_ Pride and Prejudice _unhesitatingly`.
        assert!(!clean.contains('_'), "markup is showing: {clean}");
        assert!(clean.contains("declare for Pride and Prejudice unhesitatingly."));

        // The book itself is still all there.
        assert!(clean.contains("a truth universally acknowledged"));
        // And the paragraph mark it needs to be typeset is kept.
        assert!(clean.contains("\n\n"), "paragraphs were lost: {clean}");
    }

    #[test]
    fn a_file_that_follows_none_of_the_conventions_is_left_alone() {
        // The rules key on markers Gutenberg writes. A file without them is
        // somebody's book, and dropping it because it is unusual would be
        // very much worse than showing it as it came.
        let plain = "Chapter One\n\nIt was a bright cold day in April.\n";
        assert_eq!(readable(plain), plain);
    }

    #[test]
    fn an_unclosed_caption_never_swallows_the_rest_of_the_book() {
        // A truncated download ends mid-caption. Counting brackets to the end
        // of the file and deleting what it found would leave a blank screen.
        let truncated = "Chapter One\n\n[Illustration: the frontispiece\n";
        let clean = readable(truncated);
        assert!(clean.contains("Chapter One"), "{clean}");
    }

    const ANSWER: &str = r#"{
        "count": 2,
        "results": [
            {
                "title": "Pride and Prejudice",
                "authors": [{"name": "Austen, Jane"}],
                "formats": {
                    "image/jpeg": "https://www.gutenberg.org/cache/epub/1342/pg1342.cover.medium.jpg",
                    "text/html": "https://www.gutenberg.org/ebooks/1342.html",
                    "text/plain; charset=us-ascii": "https://www.gutenberg.org/files/1342/1342-0.txt",
                    "application/zip": "https://www.gutenberg.org/files/1342/1342.zip"
                }
            },
            {
                "title": "A Book Nobody Signed",
                "authors": [],
                "formats": {"application/epub+zip": "https://www.gutenberg.org/ebooks/2.epub"}
            }
        ]
    }"#;

    fn parsed() -> kobo_json::Value {
        kobo_json::parse(ANSWER).expect("the sample answer parses")
    }

    #[test]
    fn a_cover_that_did_not_arrive_is_asked_for_again_but_not_forever() {
        // Gutendex serves these from a CDN that intermittently refuses, and
        // the same URL a moment later usually works.
        let mut app = Gutenshelf::default();
        app.retry_cover(4, 0);
        assert_eq!(app.wanted, vec![(4, 1)]);

        // Retried at the front, which is the end taken last: one dead URL must
        // not be re-tried ahead of covers that have not been tried at all.
        app.wanted = vec![(7, 0)];
        app.retry_cover(4, 1);
        assert_eq!(app.wanted, vec![(4, 2), (7, 0)]);

        // And it does give up, or a cover that is genuinely gone would keep
        // the radio awake for as long as the shelf is open.
        let mut app = Gutenshelf::default();
        app.retry_cover(4, COVER_TRIES - 1);
        assert!(app.wanted.is_empty());
    }

    /// One transparent pixel, which is a picture the decoder accepts.
    ///
    /// Real cover art in a test would be a fixture nobody could check, and the
    /// thing under test is the caching, not the decoding.
    const PIXEL: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f,
        0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0xfc,
        0xcf, 0xc0, 0x50, 0x0f, 0x00, 0x04, 0x85, 0x01, 0x80, 0x84, 0xa9, 0x8c, 0x21, 0x00, 0x00,
        0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    /// The shelf, freshly arrived, with its covers still to find.
    fn shelved() -> AppRunner<Gutenshelf> {
        let mut runner = AppRunner::new(Gutenshelf {
            task: Some((kobo_sdk::TaskId(9), Awaiting::Catalogue)),
            ..Gutenshelf::default()
        });
        let _ignored = runner.task_outcome(
            kobo_sdk::TaskId(9),
            kobo_sdk::TaskOutcome::Completed(ANSWER.as_bytes().to_vec()),
        );
        runner
    }

    fn first_cover_key() -> String {
        let books = books_from(&parsed());
        super::cover_key(books[0].cover.as_ref().expect("a cover"))
    }

    #[test]
    fn a_cover_is_looked_for_on_the_device_before_it_is_asked_for_over_the_radio() {
        // Artwork does not change and a shelf page is the same six pictures
        // every time it is opened. Before this, every one of them was a round
        // trip over a slow radio on every launch.
        let mut runner = AppRunner::new(Gutenshelf {
            task: Some((kobo_sdk::TaskId(9), Awaiting::Catalogue)),
            ..Gutenshelf::default()
        });
        let commands = runner.task_outcome(
            kobo_sdk::TaskId(9),
            kobo_sdk::TaskOutcome::Completed(ANSWER.as_bytes().to_vec()),
        );
        assert!(
            commands.iter().any(|command| matches!(
                command,
                Command::Store(StoreRequest::Load { key }) if *key == first_cover_key()
            )),
            "the device was never asked whether it already had the cover"
        );
        assert!(
            !commands
                .iter()
                .any(|command| matches!(command, Command::Spawn { .. })),
            "the radio was used before the card had answered"
        );
    }

    #[test]
    fn a_cover_the_device_already_has_is_never_fetched() {
        let mut runner = shelved();
        let commands = runner.store_result(StoreResult::Loaded {
            key: first_cover_key(),
            value: Some(PIXEL.to_vec()),
        });
        assert!(
            !commands
                .iter()
                .any(|command| matches!(command, Command::Spawn { .. })),
            "a cover already on the device was fetched anyway"
        );
    }

    #[test]
    fn a_cover_the_device_does_not_have_is_fetched_once_every_lookup_has_answered() {
        let mut runner = shelved();
        let commands = runner.store_result(StoreResult::Loaded {
            key: first_cover_key(),
            value: None,
        });
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, Command::Spawn { .. })),
            "a cover the device did not have was never fetched"
        );
    }

    #[test]
    fn a_cached_cover_that_will_not_decode_is_thrown_away_rather_than_kept_forever() {
        // Without this, one bad write leaves a book pictureless for as long as
        // the device lasts: the cache answers, the answer is not a picture,
        // and nothing ever asks the network again.
        let mut runner = shelved();
        let commands = runner.store_result(StoreResult::Loaded {
            key: first_cover_key(),
            value: Some(b"404 Not Found".to_vec()),
        });
        assert!(
            commands.iter().any(|command| matches!(
                command,
                Command::Store(StoreRequest::Forget { key }) if *key == first_cover_key()
            )),
            "a cached value that is not a picture was kept"
        );
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, Command::Spawn { .. })),
            "nothing was fetched to replace it"
        );
    }

    #[test]
    fn a_cover_off_the_radio_is_kept_for_next_time() {
        let mut runner = shelved();
        let _ignored = runner.store_result(StoreResult::Loaded {
            key: first_cover_key(),
            value: None,
        });
        let task = runner
            .app_mut()
            .covers
            .first()
            .map(|(task, _, _)| *task)
            .expect("a cover fetch in flight");
        let commands = runner.task_outcome(task, kobo_sdk::TaskOutcome::Completed(PIXEL.to_vec()));
        assert!(
            commands.iter().any(|command| matches!(
                command,
                Command::Store(StoreRequest::Save { key, .. }) if *key == first_cover_key()
            )),
            "a cover that came off the radio was not kept"
        );
    }

    #[test]
    fn a_catalogue_answer_becomes_a_list_of_books() {
        let books = books_from(&parsed());
        assert_eq!(books.len(), 2);
        assert_eq!(books[0].title, "Pride and Prejudice");
        assert_eq!(books[0].author, "Austen, Jane");
        assert_eq!(
            books[0].text.as_deref(),
            Some("https://www.gutenberg.org/files/1342/1342-0.txt")
        );
    }

    #[test]
    fn a_book_with_no_author_still_has_something_under_its_title() {
        // Gutenberg holds a great many anonymous works, and a blank second
        // line reads as a rendering fault rather than as missing information.
        let books = books_from(&parsed());
        assert_eq!(books[1].author, "Unknown");
    }

    #[test]
    fn a_book_with_no_plain_text_edition_offers_no_download() {
        // Offering one would produce a tap that fails a second later, after
        // the panel has already repainted twice.
        let books = books_from(&parsed());
        assert_eq!(books[1].text, None);
    }

    #[test]
    fn a_zipped_or_insecure_edition_is_never_chosen() {
        // The runtime refuses plain http outright, and nothing here can unpack
        // an archive, so choosing either would be a download that cannot work.
        let entry = kobo_json::parse(
            r#"{"formats": {
                "text/plain; charset=utf-8": "https://example.org/book.txt.zip",
                "text/plain": "http://example.org/book.txt"
            }}"#,
        )
        .expect("parses");
        assert_eq!(plain_text_url(&entry), None);
    }

    #[test]
    fn a_search_term_cannot_add_parameters_of_its_own_to_the_url() {
        // The one place a reader's text goes straight into a URL.
        assert_eq!(
            encode_query("dickens&sort=popular"),
            "dickens%26sort%3Dpopular"
        );
        assert_eq!(encode_query("war and peace"), "war%20and%20peace");
        assert_eq!(encode_query("brontë"), "bront%C3%AB");
    }

    #[test]
    fn a_downloaded_chunk_is_broken_into_pages_that_fit_the_panel() {
        // Pages are measured against the panel the runtime named rather than a
        // character count, because layout stops at the bottom of the content
        // area and never draws the rest: a page that measured wrongly loses
        // its last paragraph on the device and nowhere else.
        let mut runner = AppRunner::new(Gutenshelf {
            view: View::Reading,
            open: Some(0),
            books: books_from(&parsed()),
            task: Some((kobo_sdk::TaskId(1), Awaiting::Text)),
            ..Gutenshelf::default()
        });
        let prose = "It is a truth universally acknowledged, that a single man in possession \
                     of a good fortune, must be in want of a wife.\n\n"
            .repeat(30);
        runner.task_outcome(
            kobo_sdk::TaskId(1),
            kobo_sdk::TaskOutcome::Completed(prose.clone().into_bytes()),
        );
        let application = runner.app_mut();
        assert!(
            application.reader.is_some(),
            "the chunk did not open as a book"
        );
        assert!(
            application
                .reader
                .as_ref()
                .is_some_and(|reader| reader.page_count() > 1),
            "the whole chunk fitted a page"
        );
        let metrics = CLARA_BW_METRICS;
        loop {
            let (page, expected) = {
                let reader = application.reader.as_ref().expect("a book");
                (reader.page_number(), reader.page().len())
            };
            let layout = application
                .reading()
                .layout_with(&metrics, &Chrome::with_back(true));
            let drawn = layout
                .nodes
                .iter()
                .filter(|node| node.kind == LayoutKind::Text)
                .count();
            assert_eq!(
                drawn, expected,
                "page {page} measured as {expected} paragraphs but drew {drawn}"
            );
            let reader = application.reader.as_mut().expect("a book");
            if !reader.forward() {
                break;
            }
        }
    }

    #[test]
    fn every_cover_on_a_shelf_page_is_drawn_whole_between_the_bars() {
        // The defect this covers: the shelf held six books in two columns,
        // which is three rows of a shape half again as tall as it is wide, and
        // the third row was drawn underneath the nav bar. The reader saw four
        // covers and half of two more, so a shelf of six looked like a shelf
        // of four and a rendering fault.
        //
        // Asserted against the layout rather than against the numbers that
        // produced it, because the two are set in different crates: SHELF_PAGE
        // decides where the shelf is cut and the grid decides how wide a cell
        // is, and nothing else makes them agree.
        let application = Gutenshelf {
            books: (0..super::SHELF_PAGE)
                .map(|index| super::Book {
                    title: format!("Book {index}"),
                    author: "Somebody".to_owned(),
                    text: None,
                    cover: None,
                    picture: None,
                })
                .collect(),
            ..Gutenshelf::default()
        };
        let chrome = Chrome::with_back(true);
        let screen = application.results();
        let layout = screen.layout_with(&CLARA_BW_METRICS, &chrome);
        let tiles = layout
            .nodes
            .iter()
            .filter(|node| matches!(node.kind, LayoutKind::Tile(_)))
            .collect::<Vec<_>>();
        assert_eq!(
            tiles.len(),
            super::SHELF_PAGE,
            "every book on the page has a tile"
        );
        let floor = CLARA_BW_METRICS.height - CLARA_BW_METRICS.nav_bar_height();
        for tile in &tiles {
            assert!(
                tile.rect.y + tile.rect.height <= floor,
                "a cover runs under the nav bar: {:?} against {floor}",
                tile.rect
            );
        }
        // And the screen itself agrees, which is the check an author gets.
        let issues = screen.validate(&CLARA_BW_METRICS);
        assert!(issues.is_empty(), "{issues:?}");
    }

    /// A book, open, at the panel the tests measure against.
    fn opened(text: &str) -> Reader {
        Reader::open(
            kobo_doc::read("book.txt", text.as_bytes()).expect("a readable book"),
            Memory::default(),
            &CLARA_BW_METRICS,
        )
    }

    #[test]
    fn the_page_controls_are_reachable_by_a_tap_at_their_centre() {
        // They are the pinned bar rather than the last thing in the flow, so
        // a long page loses its final words rather than its page turn.
        let mut reader = opened("A short book.");
        reader.act(kobo_read::action::CONTROLS, &CLARA_BW_METRICS);
        let application = Gutenshelf {
            view: View::Reading,
            reader: Some(reader),
            complete: true,
            ..Gutenshelf::default()
        };
        let layout = application
            .reading()
            .layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true));
        let controls = layout
            .nodes
            .iter()
            .filter_map(|node| match node.kind {
                LayoutKind::NavDestination(action) => Some((action, node.rect)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(controls.len(), 5, "the reading controls are not all there");
        for (action, rect) in controls {
            let hit = layout.hit_test(rect.x + rect.width / 2, rect.y + rect.height / 2);
            assert_eq!(hit, Some(action));
        }
    }

    #[test]
    fn a_failed_download_goes_back_to_the_book_rather_than_to_a_blank_page() {
        // An empty reading screen has no text and no way to retry, which on a
        // device whose only other control is "leave" is a dead end.
        let mut runner = AppRunner::new(Gutenshelf {
            view: View::Reading,
            open: Some(0),
            books: books_from(&parsed()),
            task: Some((kobo_sdk::TaskId(1), Awaiting::Text)),
            ..Gutenshelf::default()
        });
        runner.task_outcome(
            kobo_sdk::TaskId(1),
            kobo_sdk::TaskOutcome::Failed(kobo_sdk::TaskError::Unreachable),
        );
        assert_eq!(runner.app_mut().view, View::Details);
        assert!(runner.app_mut().problem.is_some());
    }

    #[test]
    fn a_short_answer_means_the_book_ended() {
        // A ranged request gives no other end-of-book signal, and a reader
        // that kept asking would request past the end of the file forever.
        let mut runner = AppRunner::new(Gutenshelf {
            task: Some((kobo_sdk::TaskId(1), Awaiting::Text)),
            ..Gutenshelf::default()
        });
        runner.task_outcome(
            kobo_sdk::TaskId(1),
            kobo_sdk::TaskOutcome::Completed(b"The whole of a very short book.".to_vec()),
        );
        assert!(runner.app_mut().complete);
    }

    #[test]
    fn opening_a_different_book_keeps_none_of_the_last_one() {
        // The pages are held as one string, and text left over from the
        // previous book would appear at the top of the new one.
        let mut runner = AppRunner::new(Gutenshelf {
            books: books_from(&parsed()),
            text: "Chapter forty of something else.".to_owned(),
            fetched: 4096,
            reader: Some(opened("Chapter forty of something else.")),
            complete: true,
            ..Gutenshelf::default()
        });
        runner.action(action_id("book-0"));
        let application = runner.app_mut();
        assert!(application.text.is_empty());
        assert_eq!(application.fetched, 0);
        assert!(application.reader.is_none(), "the last book is still open");
        assert!(!application.complete);
    }

    /// The blob name for the first book in the test catalogue.
    fn first_blob() -> String {
        let books = books_from(&parsed());
        Gutenshelf::names(&books[0]).expect("a text edition").0
    }

    #[test]
    fn a_book_already_on_the_device_is_read_from_it_rather_than_downloaded() {
        // The point of keeping a book at all. Before this, opening a novel a
        // second time fetched the whole megabyte again over the radio, which
        // on this device is the difference between free and not.
        let mut runner = AppRunner::new(Gutenshelf {
            books: books_from(&parsed()),
            ..Gutenshelf::default()
        });
        runner.store_result(StoreResult::Shelf(vec![(first_blob(), 4096)]));
        runner.action(action_id("book-0"));
        let commands = runner.action(action_id("read"));

        assert!(
            !commands
                .iter()
                .any(|command| matches!(command, Command::Spawn { .. })),
            "a book already on the device was fetched again"
        );
        assert!(
            commands.iter().any(|command| matches!(
                command,
                Command::Store(StoreRequest::ShelfRead { name, .. }) if *name == first_blob()
            )),
            "the copy on the device was not read"
        );
    }

    #[test]
    fn a_book_that_is_not_here_yet_is_still_fetched() {
        let mut runner = AppRunner::new(Gutenshelf {
            books: books_from(&parsed()),
            ..Gutenshelf::default()
        });
        runner.action(action_id("book-0"));
        let commands = runner.action(action_id("read"));
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, Command::Spawn { .. })),
            "a book that is not here was not fetched"
        );
    }

    #[test]
    fn a_finished_book_is_put_on_the_shelf() {
        let mut runner = AppRunner::new(Gutenshelf {
            view: View::Reading,
            open: Some(0),
            books: books_from(&parsed()),
            task: Some((kobo_sdk::TaskId(1), Awaiting::Text)),
            ..Gutenshelf::default()
        });
        // Short, so the answer is the end of the book.
        let commands = runner.task_outcome(
            kobo_sdk::TaskId(1),
            kobo_sdk::TaskOutcome::Completed(
                b"A whole short book.\n\nAnd its second part.".to_vec(),
            ),
        );
        assert!(
            commands.iter().any(|command| matches!(
                command,
                Command::Store(StoreRequest::ShelfWrite { name, last, .. })
                    if *name == first_blob() && *last
            )),
            "a finished book was not kept"
        );
    }

    #[test]
    fn opening_a_book_asks_where_it_was_left() {
        let mut runner = AppRunner::new(Gutenshelf {
            books: books_from(&parsed()),
            ..Gutenshelf::default()
        });
        let commands = runner.action(action_id("book-0"));
        assert!(
            commands.iter().any(|command| matches!(
                command,
                Command::Store(StoreRequest::Load { key }) if key.starts_with("place-")
            )),
            "the place this book was left was never asked for"
        );
    }

    #[test]
    fn a_kept_place_is_applied_to_the_book_when_both_have_arrived() {
        let mut runner = AppRunner::new(Gutenshelf {
            view: View::Reading,
            open: Some(0),
            books: books_from(&parsed()),
            task: Some((kobo_sdk::TaskId(1), Awaiting::Text)),
            ..Gutenshelf::default()
        });
        let prose = "It is a truth universally acknowledged, that a single man in possession \
                     of a good fortune, must be in want of a wife.\n\n"
            .repeat(30);
        runner.task_outcome(
            kobo_sdk::TaskId(1),
            kobo_sdk::TaskOutcome::Completed(prose.into_bytes()),
        );
        let place = Memory {
            at: 20,
            ..Memory::default()
        };
        runner.store_result(StoreResult::Loaded {
            key: "place-0".to_owned(),
            value: Some(place.encode()),
        });
        let reader = runner.app_mut().reader.as_ref().expect("a book");
        assert!(
            reader.page().iter().any(|piece| piece.block == 20),
            "the reader was not put back where they were left"
        );
    }

    #[test]
    fn typing_a_search_asks_the_catalogue_for_exactly_what_was_typed() {
        let mut runner = AppRunner::new(Gutenshelf::default());
        runner.action(action_id("search"));
        for key in ["kb.r0c9", "kb.r1c0", "kb.r0c1"] {
            runner.action(action_id(key));
        }
        let commands = runner.action(action_id("kb.enter"));
        let asked = commands.iter().find_map(|command| match command {
            Command::Spawn { work, .. } => Some(work.clone()),
            _ => None,
        });
        let Some(kobo_sdk::Task::Fetch { url, .. }) = asked else {
            panic!("no request was made");
        };
        assert!(url.ends_with("?search=paw"), "asked for {url}");
    }
}

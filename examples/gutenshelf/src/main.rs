//! Gutenshelf: the Project Gutenberg library, on the device.
//!
//! Search sixty thousand public domain books, and read one without leaving the
//! application.
//!
//! ## Why plain text rather than EPUB
//!
//! Gutenberg publishes every book in several formats, and this reads the plain
//! text one. An EPUB is a zip archive of XHTML that would need an unpacker, an
//! XML parser and a subset of CSS before a single word reached the panel, and
//! all three would be new dependencies in a workspace that has none. The plain
//! text is the same book. What is lost is italics and a table of contents;
//! what is gained is a reader that is a few hundred lines and cannot be
//! attacked by a malformed archive.
//!
//! ## Why the book arrives in pieces
//!
//! The transport carries half a megabyte at most, and a Victorian novel is
//! several times that. Rather than refusing long books — which is most of the
//! interesting ones — this asks for the part it is about to need, using the
//! byte offset `Task::Fetch` carries. The first page therefore appears in
//! about a second, and the next piece is fetched while there are still pages
//! left to read.

use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Glyph, KoboApp, PictureHandle, ScreenBuilder, Task,
    TaskId, TaskOutcome, TilePicture, TileShape,
};
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
    /// The downloaded text broken into pages that fit this panel.
    ///
    /// Held rather than derived at draw time: the runtime states the panel
    /// during the handshake, so this cannot be computed until the application
    /// is running, and recomputing a whole chunk on every repaint would make
    /// every page turn slower than the one before it.
    pages: Vec<Vec<String>>,
    /// How many bytes of the book have been asked for so far.
    fetched: u32,
    /// Whether the download reached the end of the book.
    complete: bool,
    page: usize,
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
            pages: Vec::new(),
            fetched: 0,
            complete: false,
            page: 0,
            shelf: 0,
            wanted: Vec::new(),
            covers: Vec::new(),
            task: None,
            problem: None,
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
                .button("search", "Search the library")
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
                (
                    format!("book-{index}"),
                    book.title.clone(),
                    Glyph::Book,
                    book.picture,
                )
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
            .text(book.author.clone());
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
        if self.awaiting_text() {
            return screen.activity("Downloading", None).build();
        }
        screen = if book.text.is_some() {
            screen.button("read", "Read")
        } else {
            // Said plainly rather than by disabling a button. A button that
            // does nothing when tapped reads as a broken panel.
            screen.text("Gutenberg has no plain text edition of this book.")
        };
        screen.button("results", "Back to the results").build()
    }

    fn reading(&self) -> kobo_sdk::Screen {
        let body = self.pages.get(self.page);
        let title = self
            .open
            .and_then(|index| self.books.get(index))
            .map_or_else(|| "Reading".to_owned(), |book| book.title.clone());
        let mut screen = ScreenBuilder::new("gutenshelf-reading").top_bar(title);
        let Some(body) = body else {
            return screen.activity("Downloading", None).build();
        };
        // One node per paragraph, which is how the page was measured. A single
        // node would lose every blank line in the book, because wrapping works
        // on words and cannot see where a paragraph ended.
        for paragraph in body {
            screen = screen.text(paragraph.clone());
        }
        if let Some(problem) = &self.problem {
            screen = screen.banner(BannerLevel::Attention, problem.clone());
        }
        // The page controls are the pinned bar rather than the last thing in
        // the flow. Content stops at the bar, so a page that runs long loses
        // its final words rather than its page turn: the layout engine drops
        // whatever does not fit, and in the flow the turn is what does not fit.
        // Nothing is "selected", because these are controls rather than a
        // location.
        // The bar stays, because a control you can see is how anyone learns
        // the gesture exists. The gesture is what gets used afterwards: every
        // Kobo turns the page when you tap the side of it, and a reader
        // holding one already knows that without being told.
        screen
            .page_turns("page-back", "page-next")
            .nav_bar(
                None,
                [
                    ("page-back", "Back"),
                    ("results", "Library"),
                    ("page-next", "Next"),
                ],
            )
            .build()
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
        self.wanted = self
            .books
            .iter()
            .enumerate()
            .skip(first)
            .take(SHELF_PAGE)
            .filter(|(_, book)| book.picture.is_none() && book.cover.is_some())
            .map(|(index, _)| (index, 0))
            .rev()
            .collect();
        self.ask_cover(context);
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
    fn took_cover(&mut self, context: &mut Context, index: usize, bytes: &[u8]) {
        let (cell_width, cell_height) = context.metrics().tile_body(TileShape::Portrait);
        let Ok(cell_width) = u32::try_from(cell_width) else {
            return;
        };
        let Ok(cell_height) = u32::try_from(cell_height) else {
            return;
        };
        let Ok(picture) = kobo_image::decode(bytes) else {
            // A cover that will not decode is not worth telling the reader
            // about: the tile keeps its glyph and the book is still readable.
            return;
        };
        // Enlarging rather than merely shrinking: Gutenberg publishes covers at
        // around 190 by 300, and a tile on this panel is more than twice that,
        // so fitting alone left every cover as a stamp in an empty cell.
        let Ok(mut picture) = picture.fit_enlarging(cell_width, cell_height) else {
            return;
        };
        // Halftoned to the levels this panel actually resolves. Without it the
        // smooth gradients in cover art band into visible steps, which looks
        // like a decoding fault rather than a limitation of the display.
        picture.dither(kobo_image::PANEL_GREYS);
        let handle = PictureHandle(u32::try_from(index).unwrap_or(0));
        let (width, height) = (picture.width(), picture.height());
        if let Some(reference) = context.put_picture(handle, width, height, picture.into_grey()) {
            if let Some(book) = self.books.get_mut(index) {
                book.picture = Some(reference);
            }
        }
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
        if self.page + TOP_UP_PAGES >= self.pages.len() {
            self.ask_text(context);
        }
    }

    fn open_book(&mut self, context: &mut Context, index: usize) {
        self.open = Some(index);
        self.view = View::Details;
        // A different book, so nothing about the last one survives.
        self.text.clear();
        self.pages.clear();
        self.fetched = 0;
        self.complete = false;
        self.page = 0;
        self.problem = None;
        self.show(context);
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

    fn took_text(&mut self, context: &Context, bytes: &[u8]) {
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
        self.pages = context.paginate(&readable(&self.text), true);
        self.view = View::Reading;
    }
}

impl KoboApp for Gutenshelf {
    fn on_start(&mut self, context: &mut Context) {
        self.ask_catalogue(context, None);
        self.show(context);
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if action == ActionId::BACK {
            // Delivered only on a screen that claimed it, so the shelf is
            // never reached here and Back still leaves the application from
            // there. Reading returns to the book it was opened from; a book
            // and the keyboard both return to the shelf.
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
            self.ask_text(context);
            self.show(context);
            return;
        }
        if action == action_id("page-next") {
            if self.page + 1 < self.pages.len() {
                self.page += 1;
            } else if self.complete {
                self.problem = Some("That is the end of the book.".to_owned());
            }
            self.top_up(context);
            self.show(context);
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
        if action == action_id("page-back") {
            self.page = self.page.saturating_sub(1);
            self.problem = None;
            self.show(context);
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
                TaskOutcome::Completed(bytes) => self.took_cover(context, index, &bytes),
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
        books_from, encode_query, plain_text_url, readable, Awaiting, Gutenshelf, View, COVER_TRIES,
    };
    use kobo_sdk::{action_id, AppRunner, Command};
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
        assert!(application.pages.len() > 1, "the whole chunk fitted a page");
        for page in 0..application.pages.len() {
            application.page = page;
            let layout = application
                .reading()
                .layout_with(&CLARA_BW_METRICS, Chrome::with_back(true));
            let drawn = layout
                .nodes
                .iter()
                .filter(|node| node.kind == LayoutKind::Text)
                .count();
            assert_eq!(
                drawn,
                application.pages[page].len(),
                "page {page} measured as {} paragraphs but drew {drawn}",
                application.pages[page].len()
            );
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
        let layout = screen.layout_with(&CLARA_BW_METRICS, chrome);
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

    #[test]
    fn the_page_controls_are_reachable_by_a_tap_at_their_centre() {
        // They are the pinned bar rather than the last thing in the flow, so
        // a long page loses its final words rather than its page turn.
        let application = Gutenshelf {
            view: View::Reading,
            pages: vec![vec!["A short book.".to_owned()]],
            complete: true,
            ..Gutenshelf::default()
        };
        let layout = application
            .reading()
            .layout_with(&CLARA_BW_METRICS, Chrome::with_back(true));
        let controls = layout
            .nodes
            .iter()
            .filter_map(|node| match node.kind {
                LayoutKind::NavDestination(action) => Some((action, node.rect)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(controls.len(), 3);
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
            page: 12,
            complete: true,
            ..Gutenshelf::default()
        });
        runner.action(action_id("book-0"));
        let application = runner.app_mut();
        assert!(application.text.is_empty());
        assert_eq!(application.fetched, 0);
        assert_eq!(application.page, 0);
        assert!(!application.complete);
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

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
    action_id, ActionId, BannerLevel, Context, Glyph, KoboApp, ScreenBuilder, Task, TaskId,
    TaskOutcome,
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

/// How close to the end of what has been downloaded the reader may get before
/// the next piece is requested.
const TOP_UP_PAGES: usize = 2;

#[derive(Clone, Debug, Eq, PartialEq)]
struct Book {
    title: String,
    author: String,
    /// Where the plain text lives, when Gutenberg published one.
    text: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum View {
    Results,
    Search,
    Details,
    Reading,
}

/// What the outstanding request is for.
///
/// Only one is ever in flight. A second would either race the first onto the
/// panel or need a screen that can describe two kinds of waiting at once.
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
            task: None,
            problem: None,
        }
    }
}

impl Gutenshelf {
    fn show(&self, context: &mut Context) {
        let screen = match self.view {
            View::Results => self.results(),
            View::Search => self.search(),
            View::Details => self.details(),
            View::Reading => self.reading(),
        };
        context.set_screen(screen);
    }

    fn results(&self) -> kobo_sdk::Screen {
        let mut screen = ScreenBuilder::new("gutenshelf").top_bar(match &self.query {
            None => "Gutenshelf".to_owned(),
            Some(query) => format!("\u{201c}{query}\u{201d}"),
        });
        if let Some(problem) = &self.problem {
            screen = screen.banner(BannerLevel::Attention, problem.clone());
        }
        screen = screen.button("search", "Search the library");
        if self.task.is_some() {
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
            return screen
                .text(
                    "Sixty thousand books, free and out of copyright. \
                     Search for an author or a title.",
                )
                .build();
        }
        screen
            .divider()
            .rows(self.books.iter().enumerate().map(|(index, book)| {
                (
                    format!("book-{index}"),
                    book.title.clone(),
                    book.author.clone(),
                    Glyph::Book,
                )
            }))
            .build()
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
        if let Some(problem) = &self.problem {
            screen = screen.banner(BannerLevel::Attention, problem.clone());
        }
        if self.task.is_some() {
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
        self.pages = context.paginate(&self.text, true);
        self.view = View::Reading;
    }
}

impl KoboApp for Gutenshelf {
    fn on_start(&mut self, context: &mut Context) {
        self.ask_catalogue(context, None);
        self.show(context);
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
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
        let Some((outstanding, awaiting)) = self.task else {
            return;
        };
        if outstanding != task {
            return;
        }
        self.task = None;
        match outcome {
            TaskOutcome::Completed(bytes) => match awaiting {
                Awaiting::Catalogue => self.took_catalogue(&bytes),
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
            TaskOutcome::Cancelled => self.problem = Some("Cancelled.".to_owned()),
        }
        self.show(context);
    }
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
    use super::{books_from, encode_query, plain_text_url, Awaiting, Gutenshelf, View};
    use kobo_sdk::{action_id, AppRunner, Command};
    use kobo_ui::{Chrome, LayoutKind, CLARA_BW_METRICS};

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

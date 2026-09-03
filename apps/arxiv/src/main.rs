//! arXiv, for reading rather than for downloading.
//!
//! Eight thousand preprints a week land on arXiv, and the way anybody finds
//! the handful worth their evening is by browsing a subject's newest listing
//! or searching for a phrase. Both are one query to the same export API, so
//! that is the whole of this application: a list of subjects, a search box,
//! and somewhere to actually read what comes back.
//!
//! ## Why the abstract is the document
//!
//! A paper on arXiv is a PDF, and this platform's reader does not read PDFs.
//! That could have made this a catalogue with nothing behind it -- a list that
//! ends at a title. It does not, because arXiv has served an HTML rendering of
//! every paper submitted since December 2023, and that is real prose the
//! reader can set. So a paper opens on its abstract, which always exists, and
//! offers the full text when arXiv has one to give. A paper too old for the
//! rendering says so plainly rather than opening an empty page.
//!
//! ## Why the listing is fetched a page at a time
//!
//! The API answers a query with as many entries as you ask for, and asking for
//! a thousand is one long fetch that fills memory with papers nobody scrolled
//! to. Twenty-five is about four screens of rows, which is as far as anybody
//! goes before either opening something or changing the query.

mod atom;

use atom::{Paper, Results};
use kobo_bookview::{BookView, Step};
use kobo_read::{Memory, Outcome};
use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Glyph, KoboApp, RowLead, Screen, ScreenBuilder,
    ShelfDownload, ShelfProgress, ShelfUpload, StoreResult, Task, TaskError, TaskId, TaskOutcome,
};
use std::fmt::Write as _;
use std::process::ExitCode;

/// The export API, which is the interface arXiv asks robots to use.
const QUERY: &str = "https://export.arxiv.org/api/query";

/// How many papers one listing fetch asks for.
const PAGE: usize = 25;

/// The ceiling on a listing. Twenty-five abstracts is well under this; the
/// margin is for a query that matches papers with long author lists.
const LISTING_BYTES: u32 = 512 * 1024;

/// The most papers the library will hold.
///
/// An application may hold [`kobo_sdk::MAX_STORE_KEYS`] durable keys, and this
/// library spends two of them on each paper -- the reading position and the
/// catalogue entry -- alongside a blob on the shelf. The ceiling is what is
/// left once the registry itself and a margin for the reading positions of
/// papers merely visited are taken out.
///
/// There is deliberately no eviction. A paper is in the library because
/// somebody put it there, and a library that quietly throws away the oldest
/// thing you kept is not a library. When it is full it says so and the reader
/// removes something.
const MAX_KEPT: usize = 96;

/// The ceiling on one paper's full text.
///
/// A rendered paper is mostly markup, and the ones that overrun this are
/// review articles with four hundred references. Truncation is reported on the
/// page rather than hidden, because a paper that simply stops is otherwise
/// indistinguishable from one that ended.
const FULL_TEXT_BYTES: u32 = 768 * 1024;

/// The subjects offered on the way in.
///
/// arXiv has upwards of a hundred and fifty categories and no reader wants to
/// scroll them. These are the ones a person holding an e-reader is plausibly
/// browsing, named as arXiv names them so the identifier on a paper's page
/// matches the list it was found in.
const SUBJECTS: &[(&str, &str)] = &[
    ("cs.AI", "Artificial Intelligence"),
    ("cs.LG", "Machine Learning"),
    ("cs.CL", "Computation and Language"),
    ("cs.CV", "Computer Vision"),
    ("cs.CR", "Cryptography and Security"),
    ("cs.DS", "Data Structures and Algorithms"),
    ("cs.SE", "Software Engineering"),
    ("cs.PL", "Programming Languages"),
    ("cs.DC", "Distributed and Parallel Computing"),
    ("cs.HC", "Human-Computer Interaction"),
    ("cs.OS", "Operating Systems"),
    ("math.CO", "Combinatorics"),
    ("math.NT", "Number Theory"),
    ("math.PR", "Probability"),
    ("stat.ML", "Machine Learning (Statistics)"),
    ("quant-ph", "Quantum Physics"),
    ("astro-ph.EP", "Earth and Planetary Astrophysics"),
    ("q-bio.NC", "Neurons and Cognition"),
    ("econ.GN", "General Economics"),
    ("physics.hist-ph", "History and Philosophy of Physics"),
];

/// Percent-encodes the parts of a query that are not safe in a URL.
///
/// Hand-rolled because the alternative is a dependency for twenty lines, and
/// because the runtime rejects a malformed URL rather than repairing it: a
/// search for `"deep learning"` with the quotes left raw is a request that
/// never leaves the device.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(byte));
            }
            _ => {
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}

/// Wraps a search in quotes when it is more than one word.
///
/// arXiv reads an unquoted space as `OR`: `all:machine learning` comes back
/// from the API as the query `all:machine OR all:learning`, which is every
/// paper containing either word and is not what anybody typing two words
/// meant. Quoting asks for the phrase, which is.
///
/// A quote somebody typed themselves is dropped rather than escaped. There is
/// no escape for it in the API's syntax, and an unbalanced one turns the rest
/// of the query into nonsense.
fn phrase(words: &str) -> String {
    let cleaned = words.replace('"', " ");
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.contains(' ') {
        format!("\"{cleaned}\"")
    } else {
        cleaned
    }
}

/// What a listing is a listing of.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Query {
    /// Everything newest-first in one subject.
    Subject { code: String, name: String },
    /// Whatever somebody typed.
    Words(String),
}

/// How far back a listing reaches.
///
/// arXiv publishes no measure of how widely a paper is read -- no citation
/// count, no download tally, nothing an application could sort by -- so the
/// only honest way to offer "what is worth reading" is to narrow the window
/// and let recency stand in for it. A week of one subject is a few hundred
/// papers rather than a few hundred thousand, which is the difference
/// between a list and an archive.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Window {
    #[default]
    Any,
    Week,
    Month,
}

impl Window {
    /// The next window in the cycle, for a control that has one button.
    const fn next(self) -> Self {
        match self {
            Self::Any => Self::Week,
            Self::Week => Self::Month,
            Self::Month => Self::Any,
        }
    }

    const fn days(self) -> u64 {
        match self {
            Self::Any => 0,
            Self::Week => 7,
            Self::Month => 30,
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Any => "Any time",
            Self::Week => "This week",
            Self::Month => "Last 30 days",
        }
    }

    /// The `submittedDate:[from TO to]` clause, or nothing for `Any`.
    ///
    /// `today` is a count of days since the Unix epoch, taken as an argument
    /// rather than read from the clock so that the shape of the clause can
    /// be tested against a date that will not move.
    fn clause(self, today: u64) -> Option<String> {
        if self == Self::Any {
            return None;
        }
        let from = stamp(today.saturating_sub(self.days()), false);
        let to = stamp(today, true);
        Some(format!("submittedDate:[{from} TO {to}]"))
    }
}

/// The `YYYYMMDDHHMM` stamp arXiv wants, at either end of a day.
fn stamp(day: u64, end: bool) -> String {
    let (year, month, date) = civil(day);
    let clock = if end { "2359" } else { "0000" };
    format!("{year:04}{month:02}{date:02}{clock}")
}

/// Calendar date from a count of days since 1970-01-01.
///
/// Howard Hinnant's `civil_from_days`, which reckons years as starting in
/// March so that the leap day lands at the end and needs no special case.
/// Written out here because pulling in a date library to format four
/// numbers would be a poor trade on a device this size.
fn civil(day: u64) -> (u64, u64, u64) {
    let era_day = day + 719_468;
    let era = era_day / 146_097;
    let day_of_era = era_day % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted = (5 * day_of_year + 2) / 153;
    let date = day_of_year - (153 * shifted + 2) / 5 + 1;
    let month = if shifted < 10 {
        shifted + 3
    } else {
        shifted - 9
    };
    (if month <= 2 { year + 1 } else { year }, month, date)
}

/// Today, as days since the Unix epoch.
fn today() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() / 86_400)
}

impl Query {
    /// The `search_query` arXiv expects.
    fn expression(&self, window: Window, today: u64) -> String {
        let subject = match self {
            Self::Subject { code, .. } => format!("cat:{}", escape(code)),
            Self::Words(words) => format!("all:{}", escape(&phrase(words))),
        };
        // A window narrows whichever question was asked, so it is an AND
        // against the term rather than a term of its own.
        // The whole expression goes into a query string, so the separator
        // has to be encoded along with the clause. An unencoded space here
        // is a malformed URL, and arXiv answers a malformed URL with an
        // empty feed rather than an error, which would look like a subject
        // that had simply stopped publishing.
        match window.clause(today) {
            Some(clause) => format!("{subject}{}{}", escape(" AND "), escape(&clause)),
            None => subject,
        }
    }

    fn title(&self) -> String {
        match self {
            Self::Subject { name, .. } => name.clone(),
            Self::Words(words) => format!("\u{201c}{words}\u{201d}"),
        }
    }
}

/// What the outstanding fetch will turn out to be.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Awaiting {
    Listing,
    FullText,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum View {
    #[default]
    Subjects,
    Search,
    Listing,
    Paper,
    FullText,
    /// The papers kept for reading without a network.
    Library,
}

/// One paper the reader kept, as the library lists it.
///
/// The title and authors are held here rather than read back out of the stored
/// rendering, because a library has to be listable with the card out of the
/// device and the shelf untouched: parsing ninety-six papers to draw one
/// screen of rows is the kind of thing that makes an application feel broken.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Kept {
    id: String,
    title: String,
    authors: String,
    /// How big the stored rendering is, so the library can say what it costs.
    bytes: u32,
}

#[derive(Debug, Default)]
struct Arxiv {
    view: View,
    keyboard: Keyboard,
    /// The subject list's own page, because twenty subjects is more than one
    /// screen of rows.
    subject_page: usize,
    query: Option<Query>,
    papers: Vec<Paper>,
    /// What arXiv says the query matches in total, so "25 of 1204" can be
    /// honest about being the first page of something much longer.
    total: u32,
    /// How far into the result set the papers in hand start.
    offset: usize,
    listing_page: usize,
    /// Which paper is open, as an index into `papers`.
    open: Option<usize>,
    /// The abstract, already broken into panel pages.
    ///
    /// A summary and its metadata, which is a card rather than a document. The
    /// paper itself is read through `book`.
    pages: Vec<Vec<String>>,
    page: usize,
    /// The paper's full text, open in the reader every other application on
    /// this device reads through.
    ///
    /// arXiv used to flatten the rendering to one long string and hand it to a
    /// line wrapper, which is why a paper had no headings, no emphasis, no
    /// figures, no captions, no equations set apart from the prose, and none
    /// of the marking, searching or dictionary a reader has everywhere else.
    /// The markup arXiv sends says all of that; nothing was reading it.
    book: BookView,
    /// Whether the full text arrived cut off at the byte ceiling.
    truncated: bool,
    task: Option<(TaskId, Awaiting)>,
    trouble: Option<String>,
    /// How far back listings reach.
    window: Window,
    /// Set when Keep was pressed from an abstract, so that the fetch it
    /// started ends in the library rather than only on the screen.
    keep_when_fetched: bool,
    /// Every paper kept for offline reading, newest first.
    library: Vec<Kept>,
    /// Whether the open paper was reached from the library rather than a
    /// listing, which is what Back has to know.
    from_library: bool,
    library_page: usize,
    /// The rendering of the open paper, held while it is on the panel so that
    /// keeping it does not mean fetching it a second time.
    ///
    /// Dropped with everything else when the paper closes: a stored copy is
    /// the point of keeping, and a copy of a paper nobody kept is a megabyte
    /// spent on nothing.
    fetched: Option<Vec<u8>>,
    /// A rendering on its way to or from the shelf.
    keeping: Option<ShelfUpload>,
    loading: Option<ShelfDownload>,
    /// Which paper the transfer in flight is for, since a shelf answer names
    /// only the blob.
    transferring: Option<String>,
    /// The reading position of the open paper, loaded before the paper is and
    /// held until the reader can be given it.
    place: Option<Memory>,
}

/// The store key a paper's reading position is written under.
fn place_key(id: &str) -> String {
    format!("place.{}", key_safe(id))
}

/// The shelf name a paper's kept rendering is written under.
fn blob_key(id: &str) -> String {
    format!("paper.{}", key_safe(id))
}

/// The store key the library's catalogue is written under.
const LIBRARY_KEY: &str = "library";

/// What a kept paper's row says under its title.
fn kept_summary(kept: &Kept) -> String {
    let size = kept.bytes / 1024;
    if kept.authors.is_empty() {
        return format!("{} \u{b7} {size} KB", kept.id);
    }
    format!("{} \u{b7} {} \u{b7} {size} KB", kept.id, kept.authors)
}

/// Writes the library catalogue out.
///
/// A line per paper, tab separated, for the same reason [`Memory::encode`] is
/// a line per field: it is readable over the shell when somebody reports
/// having lost a paper, and a line that cannot be understood costs one paper
/// rather than the whole library.
///
/// A tab is the separator because it is the one character a title, an author
/// list and an arXiv identifier all cannot contain -- and any that arrives
/// anyway is turned into a space on the way in, so a hostile title cannot
/// forge a field.
fn encode_library(library: &[Kept]) -> Vec<u8> {
    let mut text = String::new();
    for kept in library.iter().take(MAX_KEPT) {
        let _ = writeln!(
            text,
            "{}\t{}\t{}\t{}",
            untabbed(&kept.id),
            kept.bytes,
            untabbed(&kept.title),
            untabbed(&kept.authors)
        );
    }
    text.into_bytes()
}

fn decode_library(bytes: &[u8]) -> Vec<Kept> {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Vec::new();
    };
    let mut library = Vec::new();
    for line in text.lines().take(MAX_KEPT) {
        let mut fields = line.split('\t');
        let (Some(id), Some(bytes), Some(title)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        if id.is_empty() {
            continue;
        }
        library.push(Kept {
            id: id.to_owned(),
            title: title.to_owned(),
            authors: fields.next().unwrap_or_default().to_owned(),
            bytes: bytes.parse().unwrap_or(0),
        });
    }
    library
}

fn untabbed(text: &str) -> String {
    text.replace(['\t', '\n', '\r'], " ")
}

/// Narrows an arXiv identifier to what a store key may contain.
///
/// An identifier is `2401.00001v2` or, under the numbering arXiv used before
/// 2007, `math.CO/0601001`. The store takes lower case letters, digits, and
/// `.`, `-` and `_` only, and it refuses a key outside that set rather than
/// repairing it -- so the repair happens here, where it can be seen.
///
/// The slash becomes an underscore rather than a dash so that it stays
/// distinguishable from a dash that was already there, and the case is folded
/// because the store will not take `CO`.
fn key_safe(id: &str) -> String {
    id.chars()
        .map(|character| match character {
            'a'..='z' | '0'..='9' | '.' | '-' | '_' => character,
            'A'..='Z' => character.to_ascii_lowercase(),
            '/' => '_',
            _ => '-',
        })
        .collect()
}

const SEARCH: &str = "search";
const SUBJECTS_BACK: &str = "subjects-back";
const SUBJECTS_NEXT: &str = "subjects-next";
const LIST_BACK: &str = "list-back";
const LIST_NEXT: &str = "list-next";
const READ_BACK: &str = "read-back";
const READ_NEXT: &str = "read-next";
const MORE: &str = "more";
const FULL_TEXT: &str = "full-text";
const ABSTRACT: &str = "abstract";
const SUBJECT: &str = "subject-";
const PAPER: &str = "paper-";
const LIBRARY: &str = "library";
const LIB_BACK: &str = "library-back";
const LIB_NEXT: &str = "library-next";
const KEEP: &str = "keep";
const DISCARD: &str = "discard";
const KEPT: &str = "kept-";
const WINDOW: &str = "window";

impl Arxiv {
    fn paper(&self) -> Option<&Paper> {
        self.open.and_then(|index| self.papers.get(index))
    }

    /// Asks for a page of results.
    ///
    /// Newest first, always. A preprint server sorted by relevance is a search
    /// engine; sorted by date it is a periodical, which is what browsing a
    /// subject means.
    fn ask_listing(&mut self, context: &mut Context, query: Query, offset: usize) {
        let url = format!(
            "{QUERY}?search_query={}&start={offset}&max_results={PAGE}\
             &sortBy=submittedDate&sortOrder=descending",
            query.expression(self.window, today())
        );
        self.trouble = None;
        match context.spawn_retrying(Task::Fetch {
            url,
            offset: 0,
            max_bytes: LISTING_BYTES,
            credential: None,
            headers: Vec::new(),
        }) {
            Some(task) => {
                self.task = Some((task, Awaiting::Listing));
                self.query = Some(query);
                self.offset = offset;
            }
            None => self.trouble = Some("The device is busy. Try that again.".to_owned()),
        }
    }

    /// Asks for arXiv's HTML rendering of the open paper.
    fn ask_full_text(&mut self, context: &mut Context) {
        let Some(paper) = self.paper() else {
            return;
        };
        let url = format!("https://arxiv.org/html/{}", escape_path(&paper.id));
        // Asked for now rather than when the rendering lands, so that the
        // place is already in hand by the time there is a document to put
        // it into. The store is on the same machine and the paper is at the
        // other end of the internet, so the race is not close.
        let id = paper.id.clone();
        self.ask_place(context, &id);
        self.trouble = None;
        self.truncated = false;
        match context.spawn_retrying(Task::Fetch {
            url,
            offset: 0,
            max_bytes: FULL_TEXT_BYTES,
            credential: None,
            headers: Vec::new(),
        }) {
            Some(task) => self.task = Some((task, Awaiting::FullText)),
            None => self.trouble = Some("The device is busy. Try that again.".to_owned()),
        }
    }

    /// Lays the open paper's abstract out as pages.
    fn open_abstract(&mut self, context: &Context) {
        let Some(paper) = self.paper() else {
            return;
        };
        self.pages = context.paginate_reading(&abstract_text(paper), false);
        self.page = 0;
        self.truncated = false;
    }

    fn subjects(&self, context: &Context) -> Screen {
        let mut screen = ScreenBuilder::new("arxiv-subjects").top_bar("arXiv");
        if let Some(trouble) = &self.trouble {
            screen = screen.banner(BannerLevel::Attention, trouble.clone());
        }
        let rows: Vec<(&str, &str)> = SUBJECTS.iter().map(|(code, name)| (*name, *code)).collect();
        let pages = context.paginate_rows(&rows, true);
        let page = self.subject_page.min(pages.len().saturating_sub(1));
        let shown = pages.get(page).map(Vec::as_slice).unwrap_or_default();
        screen
            .rows(shown.iter().filter_map(|index| {
                SUBJECTS.get(*index).map(|(code, name)| {
                    (
                        format!("{SUBJECT}{index}"),
                        (*name).to_owned(),
                        (*code).to_owned(),
                        RowLead::Icon(Glyph::Note),
                    )
                })
            }))
            .top_bar_glyph(LIBRARY, "Library", Glyph::Bookmark)
            .top_bar_glyph(SEARCH, "Search arXiv", Glyph::Search)
            .page_turns(SUBJECTS_BACK, SUBJECTS_NEXT)
            .page_position(page_number(page), page_total(pages.len()))
            .build()
    }

    fn search(&self) -> Screen {
        let mut screen = ScreenBuilder::new("arxiv-search").top_bar("Search arXiv");
        if let Some(trouble) = &self.trouble {
            screen = screen.banner(BannerLevel::Attention, trouble.clone());
        }
        screen
            .typed(&self.keyboard, "A phrase, an author, a title")
            .keyboard(&self.keyboard, "Search")
            .build()
    }

    /// The papers kept for reading with no network.
    ///
    /// Reachable from the subject list rather than from a paper, because the
    /// question "what have I kept?" is asked on the way in, before there is
    /// any paper open to ask it from.
    fn library(&self, context: &Context) -> Screen {
        let mut screen = ScreenBuilder::new("arxiv-library").top_bar("Library");
        if let Some(trouble) = &self.trouble {
            screen = screen.banner(BannerLevel::Attention, trouble.clone());
        }
        if self.library.is_empty() {
            return screen
                .empty_state(
                    "Nothing kept yet. Open a paper's full text and keep it to read it later \
                     without a network.",
                )
                .build();
        }
        let rows: Vec<(String, String)> = self
            .library
            .iter()
            .map(|kept| (kept.title.clone(), kept_summary(kept)))
            .collect();
        let borrowed: Vec<(&str, &str)> = rows
            .iter()
            .map(|(title, summary)| (title.as_str(), summary.as_str()))
            .collect();
        let pages = context.paginate_rows(&borrowed, true);
        let page = self.library_page.min(pages.len().saturating_sub(1));
        let shown = pages.get(page).map(Vec::as_slice).unwrap_or_default();
        screen = screen.rows(shown.iter().filter_map(|index| {
            self.library.get(*index).map(|kept| {
                (
                    format!("{KEPT}{index}"),
                    kept.title.clone(),
                    kept_summary(kept),
                    RowLead::Icon(Glyph::Bookmark),
                )
            })
        }));
        screen
            .page_turns(LIB_BACK, LIB_NEXT)
            .page_position(page_number(page), page_total(pages.len()))
            .build()
    }

    fn listing(&self, context: &Context) -> Screen {
        let title = self
            .query
            .as_ref()
            .map_or_else(|| "arXiv".to_owned(), Query::title);
        let mut screen = ScreenBuilder::new("arxiv-listing").top_bar(title);
        if let Some(trouble) = &self.trouble {
            screen = screen.banner(BannerLevel::Attention, trouble.clone());
        }
        if self.waiting_for(Awaiting::Listing) {
            return screen.skeleton(6).build();
        }
        // The window control carries its own state as its label, so the
        // one button is both the way to change the reach of the listing and
        // the only place that says what the reach currently is.
        let narrowing = (WINDOW, self.window.label(), Glyph::Filter);
        if self.papers.is_empty() {
            return screen
                .empty_state("No papers here. Try a wider window or different words.")
                .bottom_action_marked(narrowing.0, narrowing.1, narrowing.2)
                .build();
        }
        let rows: Vec<(String, String)> = self
            .papers
            .iter()
            .map(|paper| (paper.title.clone(), row_summary(paper)))
            .collect();
        let borrowed: Vec<(&str, &str)> = rows
            .iter()
            .map(|(title, summary)| (title.as_str(), summary.as_str()))
            .collect();
        let pages = context.paginate_rows(&borrowed, true);
        let page = self.listing_page.min(pages.len().saturating_sub(1));
        let shown = pages.get(page).map(Vec::as_slice).unwrap_or_default();
        screen = screen.rows(shown.iter().filter_map(|index| {
            self.papers.get(*index).map(|paper| {
                (
                    format!("{PAPER}{index}"),
                    paper.title.clone(),
                    row_summary(paper),
                    RowLead::Number(u16::try_from(self.offset + index + 1).unwrap_or(u16::MAX)),
                )
            })
        }));
        // Offered only on the last page, and only when there is more behind
        // it. Anywhere else it is a control that fetches something the reader
        // has not finished looking at.
        let more_behind = self.offset + self.papers.len() < self.total as usize;
        if page + 1 == pages.len() && more_behind {
            screen = screen.bottom_action_marked(MORE, "Older papers", Glyph::Download);
        }
        screen
            .bottom_action_marked(narrowing.0, narrowing.1, narrowing.2)
            .page_turns(LIST_BACK, LIST_NEXT)
            .page_position(page_number(page), page_total(pages.len()))
            .build()
    }

    fn reading(&self) -> Screen {
        let Some(paper) = self.paper() else {
            return ScreenBuilder::new("arxiv-paper").top_bar("arXiv").build();
        };
        let mut screen = ScreenBuilder::new("arxiv-paper").top_bar(paper.id.clone());
        if let Some(trouble) = &self.trouble {
            screen = screen.banner(BannerLevel::Attention, trouble.clone());
        }
        if self.waiting_for(Awaiting::FullText) {
            return screen.activity("Fetching the full text", None).build();
        }
        if self.loading.is_some() {
            return screen.activity("Opening the kept paper", None).build();
        }
        if self.truncated {
            screen = screen.banner(
                BannerLevel::Attention,
                "This paper was longer than the reader will fetch, so it stops early.",
            );
        }
        let page = self.page.min(self.pages.len().saturating_sub(1));
        for line in self.pages.get(page).map(Vec::as_slice).unwrap_or_default() {
            screen = screen.text(line.clone());
        }
        // Keeping is offered from the paper rather than from the reader,
        // because the reader's bar belongs to reading and every application
        // sharing it has the same one. Whether this paper is kept is a fact
        // about this application's library, not about the page.
        let kept = self.paper().is_some_and(|paper| self.is_kept(&paper.id));
        screen = screen.fill();
        screen = if kept {
            screen.bottom_action_marked(DISCARD, "Remove from library", Glyph::Trash)
        } else {
            screen.bottom_action_marked(KEEP, "Keep for offline", Glyph::Download)
        };
        screen
            .bottom_action_marked(FULL_TEXT, "Full text", Glyph::Book)
            .page_turns(READ_BACK, READ_NEXT)
            .page_position(page_number(page), page_total(self.pages.len()))
            .build()
    }

    /// The paper itself, set by the reader the whole device shares.
    ///
    /// Its type size, its front light, its table of contents, its highlights
    /// and its dictionary are the ones somebody already learned in every other
    /// reading application here, and its Back closes the paper and returns to
    /// the abstract it was opened from.
    fn full_text(&self) -> Screen {
        let title = self
            .paper()
            .map_or_else(|| "arXiv".to_owned(), |paper| paper.id.clone());
        self.book.screen(&title).unwrap_or_else(|| self.reading())
    }

    fn waiting_for(&self, what: Awaiting) -> bool {
        self.task.is_some_and(|(_, awaiting)| awaiting == what)
    }

    fn show(&mut self, context: &mut Context) {
        let screen = match self.view {
            View::Subjects => self.subjects(context),
            View::Search => self.search(),
            View::Listing => self.listing(context),
            View::Paper => self.reading(),
            View::FullText => self.full_text(),
            View::Library => self.library(context),
        };
        // Every view but the subject list was reached from another one, so
        // Back has somewhere to go from all of them and nowhere to go from it.
        let screen = screen.with_own_back(self.view != View::Subjects);
        context.set_screen(screen);
    }

    /// Turns a page of whatever list the view is showing.
    fn turn(&mut self, context: &mut Context, forward: bool) {
        let page = match self.view {
            View::Subjects => &mut self.subject_page,
            View::Listing => &mut self.listing_page,
            View::Library => &mut self.library_page,
            View::Paper => &mut self.page,
            // The reader turns its own pages, and the taps that ask it to are
            // its own actions rather than this application's.
            View::FullText | View::Search => return,
        };
        if forward {
            *page += 1;
        } else {
            *page = page.saturating_sub(1);
        }
        self.show(context);
    }

    fn took_listing(&mut self, bytes: &[u8]) {
        let Ok(text) = std::str::from_utf8(bytes) else {
            self.trouble = Some("arXiv sent something unreadable.".to_owned());
            return;
        };
        let Results { papers, total } = atom::parse(text);
        if papers.is_empty() && self.offset > 0 {
            // Asked past the end. The papers already on screen are still the
            // right ones, so they stay rather than being replaced by nothing.
            self.trouble = Some("That is the end of this listing.".to_owned());
            return;
        }
        self.papers = papers;
        self.total = total;
        self.listing_page = 0;
    }

    fn took_full_text(&mut self, context: &mut Context, bytes: &[u8]) {
        let Ok(html) = std::str::from_utf8(bytes) else {
            self.trouble = Some("That rendering was not text.".to_owned());
            return;
        };
        let Some(paper) = self.paper().cloned() else {
            return;
        };
        let body = paper_body(html);
        // Handed to the reader whole. It parses the markup into the paper's
        // own structure -- its sections, its emphasis, its figures and their
        // captions -- and fetches the figures itself, one at a time, against
        // the address the rendering came from. None of that is arXiv's to
        // know: it is what reading a web page on this device means, and every
        // application that shows one gets the same answer.
        // The same address the rendering was fetched from, to the character.
        // A figure's address is joined against the document's own directory,
        // so a trailing slash here would make the paper's own directory the
        // base -- and arXiv writes its figures as "{id}/name.png", already
        // carrying the id. The paper's name appeared twice and every figure
        // came back 404, which is why a paper used to read with nothing but
        // "Refer to caption" where its plots belong.
        let origin = format!("https://arxiv.org/html/{}", escape_path(&paper.id));
        // Whatever this paper was left at, if it has been read before. The
        // load was asked for when the paper was opened, so by the time the
        // rendering is in hand the answer is usually already here; a paper
        // that outran it opens at the top, which is where it would have
        // opened anyway.
        let memory = self.place.take().unwrap_or_default();
        if !self.book.open_html(context, body, &origin, memory) {
            self.trouble =
                Some("arXiv has no readable rendering of this paper, only a PDF.".to_owned());
            return;
        }
        // A rendering cut off at the byte ceiling has no closing tag, which is
        // the honest signal: the fetched bytes are markup, and a paper can sit
        // at the transport limit with every word of it delivered.
        self.truncated = !body.contains("</article>");
        self.book.mark_truncated(self.truncated);
        self.page = 0;
        self.view = View::FullText;
        // Held so that keeping the paper does not fetch it a second time.
        // Only worth holding what could actually be kept: a rendering that
        // arrived truncated is not a paper, and storing one would make the
        // library quietly full of halves.
        self.fetched = if self.truncated {
            None
        } else {
            Some(bytes.to_vec())
        };
        if std::mem::take(&mut self.keep_when_fetched) {
            if self.fetched.is_some() {
                self.keep_paper(context);
            } else {
                // Half a paper is not worth a place in a library that exists
                // to be read without a network.
                self.trouble =
                    Some("Only part of this paper arrived, so it was not kept.".to_owned());
            }
        }
    }

    // ---- the library -------------------------------------------------

    /// Whether the open paper is already kept.
    fn is_kept(&self, id: &str) -> bool {
        self.library.iter().any(|kept| kept.id == id)
    }

    /// Writes the reading position of the open paper.
    ///
    /// Called on every save the reader asks for and again when the paper
    /// closes, because the two do not overlap: the reader asks after a mark or
    /// a page turn, and closing is the one that catches the paper somebody
    /// read to the end of and then left.
    fn save_place(&mut self, context: &mut Context) {
        let Some(id) = self.paper().map(|paper| paper.id.clone()) else {
            return;
        };
        let Some(memory) = self.book.memory() else {
            return;
        };
        context.store().save(place_key(&id), memory.encode());
    }

    /// Asks for the reading position of a paper about to be opened.
    fn ask_place(&mut self, context: &mut Context, id: &str) {
        self.place = None;
        context.store().load(place_key(id));
    }

    /// Keeps the open paper for reading with no network.
    fn keep_paper(&mut self, context: &mut Context) {
        let Some(paper) = self.paper().cloned() else {
            return;
        };
        if self.is_kept(&paper.id) {
            return;
        }
        if self.library.len() >= MAX_KEPT {
            self.trouble = Some(
                "The library is full. Remove a paper from it before keeping another.".to_owned(),
            );
            return;
        }
        let Some(bytes) = self.fetched.clone() else {
            // What gets kept is the rendering, and from the abstract there
            // is not one yet. Rather than telling somebody to press Full
            // text and then press Keep again, fetch it and keep it when it
            // lands: the two taps were always going to be the same errand.
            self.keep_when_fetched = true;
            self.ask_full_text(context);
            return;
        };
        let size = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
        let mut upload = ShelfUpload::new(blob_key(&paper.id), bytes);
        upload.start(context);
        self.keeping = Some(upload);
        self.transferring = Some(paper.id.clone());
        // Listed as soon as the transfer starts rather than when it finishes,
        // so the button answers the tap. A transfer that fails takes the entry
        // back out again, which is the only way round that never leaves a
        // paper kept with nothing behind it.
        self.library.insert(
            0,
            Kept {
                id: paper.id.clone(),
                title: paper.title.clone(),
                authors: paper.byline(),
                bytes: size,
            },
        );
        self.save_library(context);
    }

    /// Takes a paper back out of the library, and its rendering off the shelf.
    fn discard_paper(&mut self, context: &mut Context, id: &str) {
        self.library.retain(|kept| kept.id != id);
        context.shelf().remove(blob_key(id));
        self.save_library(context);
    }

    fn save_library(&mut self, context: &mut Context) {
        context
            .store()
            .save(LIBRARY_KEY, encode_library(&self.library));
    }

    /// Opens a kept paper from the shelf instead of the network.
    fn open_kept(&mut self, context: &mut Context, index: usize) {
        let Some(kept) = self.library.get(index).cloned() else {
            return;
        };
        // The library lists papers this application has never seen in a
        // listing, so the paper being opened is rebuilt from what was kept
        // beside it rather than looked up.
        self.papers = vec![Paper {
            id: kept.id.clone(),
            title: kept.title.clone(),
            authors: vec![kept.authors.clone()],
            ..Paper::default()
        }];
        self.open = Some(0);
        self.trouble = None;
        self.ask_place(context, &kept.id);
        let mut download = ShelfDownload::new(blob_key(&kept.id)).at_most(FULL_TEXT_BYTES as usize);
        download.start(context);
        self.loading = Some(download);
        self.transferring = Some(kept.id);
        self.view = View::Paper;
    }

    /// Gives back everything the open paper was costing.
    fn close_paper(&mut self, context: &mut Context) {
        // The place goes down before the document it points into goes away.
        self.save_place(context);
        self.book.close(context);
        self.truncated = false;
        self.fetched = None;
    }
}

/// Narrows a rendered paper to the paper.
///
/// arXiv's HTML rendering is a web page before it is a document. Ahead of the
/// paper sit a fundraising banner, a hidden "report an issue" form complete
/// with its own field labels, a row of site links and the whole table of
/// contents; behind it, a site footer. Converted wholesale that came out as a
/// full first page of furniture -- "Submit without GitHub", "Back to arXiv" --
/// before a word of the paper, which is exactly the failure this application
/// exists to avoid.
///
/// `LaTeXML` wraps the document itself in `<article class="ltx_document">` and
/// puts every one of those things outside it, so the article is the cut. A
/// rendering that does not have one is handed back whole rather than emptied:
/// furniture is worse than the paper, but nothing is worse than both.
fn paper_body(html: &str) -> &str {
    let Some(start) = html.find("<article") else {
        return html;
    };
    let body = &html[start..];
    // A paper cut off at the byte ceiling has no closing tag, and what did
    // arrive is still the paper.
    body.find("</article>")
        .map_or(body, |end| &body[..end + "</article>".len()])
}

/// The line under a paper's title in a list: who wrote it, when, and where it
/// sits. Three facts, because a row has one line for them.
fn row_summary(paper: &Paper) -> String {
    let mut parts = Vec::new();
    let byline = paper.byline();
    if !byline.is_empty() {
        parts.push(byline);
    }
    if !paper.published.is_empty() {
        parts.push(paper.published.clone());
    }
    if let Some(primary) = paper.categories.first() {
        parts.push(primary.clone());
    }
    parts.join(" \u{00b7} ")
}

/// The abstract, with the facts that only matter once you are considering
/// reading the thing set above it.
fn abstract_text(paper: &Paper) -> String {
    let mut facts = Vec::new();
    let byline = paper.byline();
    if !byline.is_empty() {
        facts.push(byline);
    }
    if !paper.categories.is_empty() {
        facts.push(paper.categories.join(", "));
    }
    if !paper.published.is_empty() {
        // Both dates, but only when they differ: a paper revised twice is a
        // different thing from the one first posted, and saying so costs a
        // line only for the papers where it is true.
        facts.push(
            if paper.updated.is_empty() || paper.updated == paper.published {
                format!("Submitted {}", paper.published)
            } else {
                format!("Submitted {}, revised {}", paper.published, paper.updated)
            },
        );
    }
    if !paper.journal.is_empty() {
        facts.push(format!("Published in {}", paper.journal));
    }
    if !paper.comment.is_empty() {
        facts.push(paper.comment.clone());
    }
    // Joined with a separator rather than newlines. The paginator treats a
    // single newline as a soft wrap, so a fact per line came out as one
    // run-on sentence -- "Lecheng Kong and 3 others cs.CL, cs.LG Submitted
    // 2026-08-10" -- where the reader could not tell where the authors ended
    // and the subjects began.
    format!(
        "{}\n\n{}\n\n{}",
        paper.title,
        facts.join(" \u{00b7} "),
        paper.summary
    )
}

/// The identifier as it goes in a path.
///
/// The old scheme has a slash in it -- `cond-mat/0703470` -- and that slash is
/// a real path separator in arXiv's URLs rather than something to encode.
fn escape_path(id: &str) -> String {
    id.split('/').map(escape).collect::<Vec<_>>().join("/")
}

/// Page numbers on the panel are counted from one, and a list with nothing in
/// it is still on page one of one rather than page zero of zero.
fn page_number(index: usize) -> u16 {
    u16::try_from(index + 1).unwrap_or(u16::MAX)
}

fn page_total(pages: usize) -> u16 {
    u16::try_from(pages.max(1)).unwrap_or(u16::MAX)
}

impl KoboApp for Arxiv {
    fn on_start(&mut self, context: &mut Context) {
        context.store().load(LIBRARY_KEY);
        self.show(context);
    }

    #[allow(clippy::too_many_lines)]
    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        // The keyboard first: while the search screen is up it owns the panel,
        // and every letter on it would otherwise fall through to the checks
        // below.
        if self.view == View::Search {
            match self.keyboard.press(action) {
                Some(Pressed::Submitted) => {
                    let typed = self.keyboard.text().trim().to_owned();
                    if typed.is_empty() {
                        return;
                    }
                    self.papers.clear();
                    self.view = View::Listing;
                    self.ask_listing(context, Query::Words(typed), 0);
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

        if action == ActionId::BACK {
            self.trouble = None;
            match self.view {
                View::Subjects => return,
                View::Search | View::Listing | View::Library => {
                    self.view = View::Subjects;
                    self.listing_page = 0;
                }
                View::Paper => {
                    // A paper opened out of the library goes back to the
                    // library, because the listing behind it is the one
                    // synthesised to hold it and has nothing else in it.
                    self.view = if self.from_library {
                        View::Library
                    } else {
                        View::Listing
                    };
                    self.open = None;
                }
                // The full text was reached from the abstract, so Back is the
                // abstract rather than the list two steps behind it. Leaving
                // is also the moment the paper stops costing anything: the
                // document, the picture handles the runtime is holding
                // against it and the figures still queued for it all go now,
                // rather than lingering for a paper nobody is reading.
                // A paper read out of the library has no abstract to go
                // back to -- only the rendering was kept -- so Back is the
                // library it was opened from.
                View::FullText if self.from_library => {
                    self.close_paper(context);
                    self.view = View::Library;
                    self.open = None;
                }
                View::FullText => {
                    self.close_paper(context);
                    self.view = View::Paper;
                    self.open_abstract(context);
                }
            }
            self.show(context);
            return;
        }

        // The reader first, while a paper is open in it: its page turns, its
        // type panel, its light, its contents, its marks and its dictionary
        // are all actions of its own, and none of them is this application's
        // to recognise.
        if self.view == View::FullText {
            if let Some(outcome) = self.book.act(context, action) {
                match outcome {
                    Outcome::Close if self.from_library => {
                        self.close_paper(context);
                        self.view = View::Library;
                        self.open = None;
                    }
                    Outcome::Close => {
                        self.close_paper(context);
                        self.view = View::Paper;
                        self.open_abstract(context);
                    }
                    Outcome::Light(level) => context.device().set_frontlight(level),
                    // The reader asks after anything worth keeping: a mark,
                    // a note, a page turn, a change of type size. Ignoring it
                    // is what used to lose every highlight in this
                    // application the moment a paper was closed.
                    Outcome::Save => self.save_place(context),
                    Outcome::Elsewhere | Outcome::Repaint => {}
                }
                self.show(context);
                return;
            }
        }

        if action == action_id(LIBRARY) {
            self.trouble = None;
            self.library_page = 0;
            self.view = View::Library;
            self.show(context);
            return;
        }

        if action == action_id(KEEP) {
            self.keep_paper(context);
            self.show(context);
            return;
        }

        if action == action_id(DISCARD) {
            if let Some(id) = self.paper().map(|paper| paper.id.clone()) {
                self.discard_paper(context, &id);
            }
            self.show(context);
            return;
        }

        if action == action_id(LIB_BACK) {
            self.turn(context, false);
            return;
        }
        if action == action_id(LIB_NEXT) {
            self.turn(context, true);
            return;
        }

        if action == action_id(SEARCH) {
            self.keyboard.clear();
            self.trouble = None;
            self.view = View::Search;
            self.show(context);
            return;
        }

        if action == action_id(SUBJECTS_BACK) || action == action_id(LIST_BACK) {
            self.turn(context, false);
            return;
        }
        if action == action_id(SUBJECTS_NEXT) || action == action_id(LIST_NEXT) {
            self.turn(context, true);
            return;
        }
        if action == action_id(READ_BACK) {
            self.turn(context, false);
            return;
        }
        if action == action_id(READ_NEXT) {
            self.turn(context, true);
            return;
        }

        if action == action_id(WINDOW) {
            self.window = self.window.next();
            // The window is part of the question, so changing it asks the
            // question again from the beginning rather than filtering what
            // is already on the screen.
            if let Some(query) = self.query.clone() {
                self.listing_page = 0;
                self.ask_listing(context, query, 0);
            }
            self.show(context);
            return;
        }

        if action == action_id(MORE) {
            if let Some(query) = self.query.clone() {
                let next = self.offset + self.papers.len();
                self.ask_listing(context, query, next);
                self.show(context);
            }
            return;
        }

        if action == action_id(FULL_TEXT) {
            self.ask_full_text(context);
            self.show(context);
            return;
        }

        if action == action_id(ABSTRACT) {
            self.close_paper(context);
            self.view = View::Paper;
            self.open_abstract(context);
            self.show(context);
            return;
        }

        for (index, (code, name)) in SUBJECTS.iter().enumerate() {
            if action == action_id(&format!("{SUBJECT}{index}")) {
                self.papers.clear();
                self.view = View::Listing;
                self.ask_listing(
                    context,
                    Query::Subject {
                        code: (*code).to_owned(),
                        name: (*name).to_owned(),
                    },
                    0,
                );
                self.show(context);
                return;
            }
        }

        for index in 0..self.library.len() {
            if action == action_id(&format!("{KEPT}{index}")) {
                self.close_paper(context);
                self.from_library = true;
                self.open_kept(context, index);
                self.show(context);
                return;
            }
        }

        for index in 0..self.papers.len() {
            if action == action_id(&format!("{PAPER}{index}")) {
                self.from_library = false;
                // Whatever the last paper was costing goes back before another
                // one starts costing anything.
                self.close_paper(context);
                self.open = Some(index);
                self.view = View::Paper;
                self.open_abstract(context);
                self.show(context);
                return;
            }
        }
    }

    /// The front light level the device is actually holding.
    ///
    /// Asked for by the reading surface itself when a paper is opened, so the
    /// panel's number and the lamp agree from the first tap rather than after
    /// it.
    fn on_device_result(
        &mut self,
        context: &mut Context,
        _request: kobo_sdk::DeviceRequest,
        result: kobo_sdk::DeviceResult,
    ) {
        if let kobo_sdk::DeviceResult::Frontlight { percent } = result {
            if self.book.took_light(percent) {
                self.show(context);
            }
        }
    }

    fn on_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
        // The reader's own sleep, which is what carries a figure from bytes to
        // pixels a half-step at a time.
        match self.book.woke(context, task, &outcome) {
            Step::Elsewhere => {}
            Step::Quiet => return,
            Step::Repaint => {
                self.show(context);
                return;
            }
        }

        let Some((waiting, awaiting)) = self.task else {
            return;
        };
        if waiting != task {
            return;
        }
        self.task = None;
        match outcome {
            TaskOutcome::Completed(bytes) => match awaiting {
                Awaiting::Listing => self.took_listing(&bytes),
                Awaiting::FullText => self.took_full_text(context, &bytes),
            },
            TaskOutcome::Failed(error) => self.trouble = Some(explain(error, awaiting)),
            TaskOutcome::Cancelled => {}
        }
        self.show(context);
    }

    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        // A rendering on its way onto the shelf. Nothing is drawn for it:
        // the paper is already listed, and a progress bar over a write that
        // takes two hundred milliseconds is noise.
        if let Some(upload) = &mut self.keeping {
            match upload.advance(context, &result) {
                ShelfProgress::Done => {
                    self.keeping = None;
                    self.transferring = None;
                    return;
                }
                ShelfProgress::Failed(_) => {
                    // The entry went in when the transfer started, so it has
                    // to come back out: a library naming a paper with nothing
                    // behind it is worse than one that failed loudly.
                    if let Some(id) = self.transferring.take() {
                        self.library.retain(|kept| kept.id != id);
                        self.save_library(context);
                    }
                    self.keeping = None;
                    self.trouble =
                        Some("That paper could not be kept. There may be no room.".to_owned());
                    self.show(context);
                    return;
                }
                ShelfProgress::Moving { .. } => return,
                ShelfProgress::Elsewhere => {}
            }
        }
        if let Some(download) = &mut self.loading {
            match download.advance(context, &result) {
                ShelfProgress::Done => {
                    let bytes = self.loading.take().expect("a download in progress").take();
                    self.transferring = None;
                    self.took_full_text(context, &bytes);
                    self.show(context);
                    return;
                }
                ShelfProgress::Failed(_) => {
                    self.loading = None;
                    self.transferring = None;
                    self.trouble =
                        Some("That kept paper could not be read back from the card.".to_owned());
                    self.show(context);
                    return;
                }
                ShelfProgress::Moving { .. } => return,
                ShelfProgress::Elsewhere => {}
            }
        }
        if let StoreResult::Loaded { key, value } = result {
            if key == LIBRARY_KEY {
                self.library = value.as_deref().map(decode_library).unwrap_or_default();
                self.show(context);
            } else if self
                .paper()
                .is_some_and(|paper| place_key(&paper.id) == key)
            {
                // A miss is the ordinary answer for a paper never opened
                // before, and `Memory::default` is exactly the right place to
                // start one.
                let place = value
                    .as_deref()
                    .map_or_else(Memory::default, Memory::decode);
                // Usually the place gets here first and is waiting when the
                // paper arrives. When the paper wins the race instead, it is
                // already open at page one, and putting it back is the whole
                // point of having stored the place at all.
                if self.book.restore(context, place.clone()) {
                    self.show(context);
                } else {
                    self.place = Some(place);
                }
            }
        }
    }
}

/// Says what went wrong in terms of what was being asked for.
///
/// A paper with no HTML rendering answers 404, and "not found" against a paper
/// that plainly exists reads as a broken application. It is not: it is arXiv
/// saying this one is only a PDF.
fn explain(error: TaskError, awaiting: Awaiting) -> String {
    match (error, awaiting) {
        (TaskError::NotFound, Awaiting::FullText) => {
            "arXiv has no readable rendering of this paper, only a PDF. \
             Renderings start with papers submitted in December 2023."
                .to_owned()
        }
        (TaskError::Denied, _) => "This application may not reach the network.".to_owned(),
        (TaskError::NotFound, Awaiting::Listing) => "arXiv had nothing at that address.".to_owned(),
        _ => "arXiv could not be reached. Check the connection and try again.".to_owned(),
    }
}

fn main() -> ExitCode {
    match kobo_sdk::run("arxiv", Arxiv::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("arxiv: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        abstract_text, blob_key, decode_library, encode_library, escape, escape_path, paper_body,
        place_key, row_summary, stamp, Arxiv, Kept, Query, View, Window, ABSTRACT, DISCARD,
        FULL_TEXT, KEEP, LIBRARY_KEY, SUBJECTS, WINDOW,
    };
    use crate::atom::Paper;
    use kobo_read::Memory;
    use kobo_sdk::StoreResult;
    use kobo_sdk::{action_id, is_valid_key, ActionId, AppRunner, Command, Task, TaskOutcome};

    fn paper() -> Paper {
        Paper {
            id: "2401.00001v2".into(),
            title: "Attention Is All You Need Again".into(),
            summary: "We revisit the transformer.".into(),
            authors: vec!["Ada Lovelace".into(), "Alan Turing".into()],
            published: "2024-01-01".into(),
            updated: "2024-01-09".into(),
            categories: vec!["cs.LG".into(), "cs.CL".into()],
            comment: "12 pages".into(),
            journal: String::new(),
        }
    }

    /// The runtime refuses a malformed URL rather than repairing it, so a
    /// search with a space in it would otherwise never leave the device.
    #[test]
    fn a_phrase_with_spaces_in_it_survives_the_journey_into_a_url() {
        assert_eq!(escape("deep learning"), "deep%20learning");
        assert_eq!(escape("\"exact phrase\""), "%22exact%20phrase%22");
        assert_eq!(escape("cs.LG"), "cs.LG");
    }

    /// The old identifiers have a slash in them, and that slash is a real path
    /// separator in arXiv's URLs rather than a character to encode.
    #[test]
    fn the_slash_in_an_old_identifier_stays_a_path_separator() {
        assert_eq!(escape_path("cond-mat/0703470"), "cond-mat/0703470");
        assert_eq!(escape_path("2401.00001v2"), "2401.00001v2");
    }

    #[test]
    fn a_subject_and_a_phrase_ask_arxiv_two_different_questions() {
        let subject = Query::Subject {
            code: "cs.LG".into(),
            name: "Machine Learning".into(),
        };
        assert_eq!(subject.expression(Window::Any, 20_000), "cat:cs.LG");
        assert_eq!(
            Query::Words("qubits".into()).expression(Window::Any, 20_000),
            "all:qubits"
        );
    }

    /// Browsing a subject means reading its newest, so the request has to say
    /// so: arXiv's default order is relevance, which for `cat:cs.LG` is
    /// arbitrary.
    #[test]
    fn a_listing_is_asked_for_newest_first() {
        let mut runner = AppRunner::new(Arxiv::default());
        runner.start();
        let commands = runner.action(action_id("subject-0"));
        let asked = commands.iter().find_map(|command| match command {
            Command::Spawn { work, .. } => Some(work.clone()),
            _ => None,
        });
        let Some(Task::Fetch { url, .. }) = asked else {
            panic!("no request was made");
        };
        assert!(url.contains("search_query=cat:cs.AI"), "{url}");
        assert!(url.contains("sortBy=submittedDate"), "{url}");
        assert!(url.contains("sortOrder=descending"), "{url}");
        assert!(url.starts_with("https://"), "{url}");
    }

    /// A subject drawn on the list but not handled would be a row that eats a
    /// tap. Each gets its own runner, because four unanswered fetches fill the
    /// in-flight allowance and the fifth would be refused for a reason that
    /// has nothing to do with the subject.
    #[test]
    fn every_subject_offered_is_one_that_can_be_opened() {
        for (index, (code, _)) in SUBJECTS.iter().enumerate() {
            let mut runner = AppRunner::new(Arxiv::default());
            runner.start();
            let commands = runner.action(action_id(&format!("subject-{index}")));
            let asked = commands.iter().find_map(|command| match command {
                Command::Spawn { work, .. } => Some(work.clone()),
                _ => None,
            });
            let Some(Task::Fetch { url, .. }) = asked else {
                panic!("subject {index} asked for nothing");
            };
            assert!(url.contains(&format!("search_query=cat:{code}")), "{url}");
        }
    }

    /// The facts above an abstract are the ones that decide whether to read
    /// it, so they have to be there and be right.
    #[test]
    fn an_abstract_is_set_under_the_facts_that_decide_whether_to_read_it() {
        let text = abstract_text(&paper());
        assert!(
            text.starts_with("Attention Is All You Need Again"),
            "{text}"
        );
        assert!(text.contains("Ada Lovelace, Alan Turing"), "{text}");
        assert!(text.contains("cs.LG, cs.CL"), "{text}");
        assert!(text.contains("Submitted 2024-01-01, revised 2024-01-09"));
        // The facts have to read as separate facts, not as one sentence.
        assert!(
            text.contains("Ada Lovelace, Alan Turing \u{00b7} cs.LG, cs.CL \u{00b7} Submitted"),
            "{text}"
        );
        assert!(text.ends_with("We revisit the transformer."), "{text}");
    }

    /// A revision date equal to the submission date is not news, and a line
    /// spent saying so is a line off every page of the abstract.
    #[test]
    fn a_paper_never_revised_is_not_described_as_revised() {
        let never = Paper {
            updated: "2024-01-01".into(),
            ..paper()
        };
        let text = abstract_text(&never);
        assert!(text.contains("Submitted 2024-01-01"), "{text}");
        assert!(!text.contains("revised"), "{text}");
    }

    /// Taken from the real shape of an arXiv rendering: banner and issue form
    /// ahead of the article, site footer behind it.
    const RENDERED: &str = "<html><body>\
        <div>arXiv is now an independent nonprofit! Learn more</div>\
        <div>Report GitHub Issue Title: Submit without GitHub</div>\
        <nav class=\"ltx_TOC\">Abstract 1 Introduction</nav>\
        <article class=\"ltx_document\"><h1>A Paper</h1><p>The first sentence.</p></article>\
        <footer>Site navigation About arXiv</footer></body></html>";

    /// The failure this catches is the one the simulator showed: a whole first
    /// page of "Submit without GitHub" and "Back to arXiv" before a word of
    /// the paper.
    #[test]
    fn a_rendered_paper_is_narrowed_to_the_paper() {
        let body = paper_body(RENDERED);
        let text = document_text(&kobo_doc::html::parse(body));
        assert!(text.contains("The first sentence."), "{text}");
        for furniture in [
            "independent nonprofit",
            "Submit without GitHub",
            "1 Introduction",
            "Site navigation",
        ] {
            assert!(!text.contains(furniture), "{furniture:?} survived: {text}");
        }
    }

    /// A paper cut off at the byte ceiling has no closing tag, and what did
    /// arrive is still the paper.
    #[test]
    fn a_rendering_cut_off_before_its_closing_tag_is_still_the_paper() {
        let cut = &RENDERED[..RENDERED.find("The first sentence.").unwrap() + 10];
        let text = document_text(&kobo_doc::html::parse(paper_body(cut)));
        assert!(text.contains("A Paper"), "{text}");
        assert!(!text.contains("Submit without GitHub"), "{text}");
    }

    /// Everything a parsed document would put on the panel, run together.
    fn document_text(document: &kobo_doc::Document) -> String {
        document
            .blocks
            .iter()
            .map(|block| match block {
                kobo_doc::Block::Heading { text, .. }
                | kobo_doc::Block::Item { text, .. }
                | kobo_doc::Block::Paragraph(text)
                | kobo_doc::Block::Quote(text)
                | kobo_doc::Block::Preformatted(text)
                | kobo_doc::Block::Caption(text) => text.clone(),
                kobo_doc::Block::Picture { alt, .. } => alt.clone(),
                _ => String::new(),
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Furniture is worse than the paper; nothing is worse than both.
    #[test]
    fn a_rendering_with_no_article_in_it_is_handed_back_whole() {
        let plain = "<html><body><p>Just prose.</p></body></html>";
        assert_eq!(paper_body(plain), plain);
    }

    #[test]
    fn a_row_says_who_wrote_it_when_and_where_it_sits() {
        assert_eq!(
            row_summary(&paper()),
            "Ada Lovelace, Alan Turing \u{00b7} 2024-01-01 \u{00b7} cs.LG"
        );
    }

    #[test]
    fn a_paper_missing_everything_but_a_title_still_has_a_row_that_reads() {
        let bare = Paper {
            title: "Untitled".into(),
            ..Paper::default()
        };
        assert_eq!(row_summary(&bare), "");
    }

    /// Back out of the full text lands on the abstract it was opened from, not
    /// on the list two steps behind it.
    #[test]
    fn leaving_the_full_text_returns_to_the_abstract_it_was_opened_from() {
        let mut app = Arxiv {
            view: View::FullText,
            papers: vec![paper()],
            open: Some(0),
            ..Arxiv::default()
        };
        let mut runner = AppRunner::new(std::mem::take(&mut app));
        runner.start();
        runner.action(kobo_sdk::ActionId::BACK);
        assert_eq!(runner.app_mut().view, View::Paper);
    }

    /// A paper opened for reading and the fetch that puts it there.
    fn opened_on(runner: &mut AppRunner<Arxiv>, rendering: &str) -> Vec<Command> {
        runner.app_mut().papers = vec![paper()];
        runner.app_mut().open = Some(0);
        runner.start();
        runner.action(action_id(FULL_TEXT));
        let task = runner
            .app()
            .task
            .expect("the full text was not asked for")
            .0;
        runner.task_outcome(task, TaskOutcome::Completed(rendering.as_bytes().to_vec()))
    }

    /// The point of all of this: a paper is a document, not a wall of text.
    ///
    /// It used to be flattened to one string and handed to a line wrapper, so
    /// a section heading, an emphasised term and a figure's caption all came
    /// out as the same undifferentiated prose -- and the figure itself did not
    /// come out at all.
    #[test]
    fn a_paper_is_read_as_a_document_rather_than_as_flattened_text() {
        let mut runner = AppRunner::new(Arxiv::default());
        let _ = opened_on(
            &mut runner,
            "<article><h2>1 Introduction</h2><p>The <em>first</em> sentence.</p>             <figure><img src=\"x1.png\" alt=\"A plot\"><figcaption>Figure 1.</figcaption>             </figure></article>",
        );

        assert_eq!(runner.app().view, View::FullText);
        let reader = runner.app().book.reader().expect("the paper is not open");
        let kinds: Vec<_> = reader
            .document()
            .blocks
            .iter()
            .map(std::mem::discriminant)
            .collect();
        assert!(
            kinds.contains(&std::mem::discriminant(&kobo_doc::Block::Heading {
                level: 2,
                text: String::new()
            })),
            "the section heading was flattened into the prose"
        );
        assert!(
            reader.pictures_wanted().contains(&"x1.png"),
            "the figure was dropped rather than drawn"
        );
    }

    /// And its figures, which live at addresses rather than in the file, are
    /// fetched against the address the paper itself was fetched from.
    #[test]
    fn a_figure_is_fetched_from_beside_the_paper_that_names_it() {
        let mut runner = AppRunner::new(Arxiv::default());
        let commands = opened_on(
            &mut runner,
            "<article><p>A paper.</p><img src=\"2401.00001v2/x1.png\" alt=\"A plot\"></article>",
        );

        let asked: Vec<String> = commands
            .iter()
            .filter_map(|command| match command {
                Command::Spawn {
                    work: Task::Fetch { url, .. },
                    ..
                } => Some(url.clone()),
                _ => None,
            })
            .collect();
        assert!(
            asked
                .iter()
                .any(|url| url == "https://arxiv.org/html/2401.00001v2/x1.png"),
            "the figure was not asked for beside its paper: {asked:?}. arXiv \
             writes a figure as \"{{id}}/name.png\", so the address it is \
             joined against is the document's own, not the paper's directory."
        );
    }

    /// Leaving a paper gives back what it was costing the device, by whichever
    /// of the two ways out of the reader the reader took.
    #[test]
    fn leaving_a_paper_gives_back_the_figures_it_was_holding() {
        for way_out in [ActionId::BACK, action_id(ABSTRACT)] {
            let mut runner = AppRunner::new(Arxiv::default());
            let _ = opened_on(
                &mut runner,
                "<article><p>A paper.</p><img src=\"x1.png\" alt=\"A plot\"></article>",
            );
            runner.action(way_out);

            assert_eq!(runner.app().view, View::Paper);
            assert!(
                !runner.app().book.is_open(),
                "the parsed paper was kept after leaving it"
            );
            assert!(
                runner.app().book.missing_pictures().is_empty(),
                "figures were still being fetched for a paper nobody is reading"
            );
        }
    }

    /// Two words meant both words, not either.
    ///
    /// arXiv reads an unquoted space as `OR` -- the API echoes the query back
    /// as `all:machine OR all:learning` -- so searching for two words used to
    /// return every paper containing either of them, which reads as search
    /// being broken.
    #[test]
    fn a_search_for_two_words_asks_for_the_phrase_rather_than_either_word() {
        let two = Query::Words("machine learning".into());
        assert_eq!(
            two.expression(Window::Any, 20_000),
            "all:%22machine%20learning%22"
        );
        // One word needs no quoting, and quoting it would only make the URL
        // longer and the query stricter than it was asked to be.
        assert_eq!(
            Query::Words("transformer".into()).expression(Window::Any, 20_000),
            "all:transformer"
        );
        // Whatever somebody typed, the query has to stay balanced.
        let hostile = Query::Words("say \"hello\" there".into()).expression(Window::Any, 20_000);
        assert_eq!(
            hostile.matches("%22").count() % 2,
            0,
            "unbalanced: {hostile}"
        );
        assert_eq!(hostile, "all:%22say%20hello%20there%22");
        // And a window still narrows a phrase.
        let narrowed = two.expression(Window::Week, 20_000);
        assert!(narrowed.starts_with("all:%22machine%20learning%22%20AND%20"));
    }

    #[test]
    fn a_library_survives_being_written_down_and_read_back() {
        let library = vec![
            Kept {
                id: "2401.00001v2".into(),
                title: "On the Convergence of Things".into(),
                authors: "A. Author and 3 others".into(),
                bytes: 91_234,
            },
            Kept {
                id: "math.CO/0601001".into(),
                title: "An Older Numbering Scheme".into(),
                authors: "B. Bourbaki".into(),
                bytes: 12,
            },
        ];
        let read_back = decode_library(&encode_library(&library));
        assert_eq!(read_back, library);
    }

    #[test]
    fn a_title_containing_a_tab_cannot_forge_a_field() {
        // The catalogue is tab separated, so a title with a tab in it would
        // otherwise arrive as a title and an author.
        let library = vec![Kept {
            id: "2401.00002".into(),
            title: "Before\tAfter".into(),
            authors: "C. Cantor".into(),
            bytes: 7,
        }];
        let read_back = decode_library(&encode_library(&library));
        assert_eq!(read_back.len(), 1, "the entry should still be one entry");
        assert_eq!(read_back[0].authors, "C. Cantor");
        assert!(
            !read_back[0].title.contains('\t'),
            "the tab should not have survived into the stored title"
        );
    }

    #[test]
    fn a_paper_identifier_with_a_slash_makes_a_key_the_store_accepts() {
        // The old arXiv numbering puts a slash in the identifier, and the
        // store refuses keys outside its character set rather than repairing
        // them, so the repair has to happen here.
        let key = blob_key("math.CO/0601001");
        assert_eq!(key, "paper.math.co_0601001");
        assert!(is_valid_key(&key), "the store would refuse {key}");
        assert!(
            is_valid_key(&place_key("math.CO/0601001")),
            "the store would refuse the place key too"
        );
        assert!(
            is_valid_key(&blob_key("2401.00001v2")),
            "the store would refuse a modern identifier"
        );
        assert_ne!(
            blob_key("math.CO/0601001"),
            blob_key("math.CO-0601001"),
            "a slash and a dash must not collapse onto one key"
        );
    }

    #[test]
    fn a_window_narrows_the_search_to_a_range_of_submission_dates() {
        // 20 000 days after the epoch is 2024-10-04.
        let subject = Query::Subject {
            code: "cs.AI".into(),
            name: "Artificial Intelligence".into(),
        };
        let week = subject.expression(Window::Week, 20_000);
        assert_eq!(
            week,
            "cat:cs.AI%20AND%20submittedDate%3A%5B202409270000%20TO%20202410042359%5D"
        );
        // A query string cannot carry a raw space or bracket, and arXiv
        // answers a malformed one with an empty feed rather than an error.
        assert!(
            !week.contains(' '),
            "the clause must be escaped, got {week}"
        );
        assert!(
            !week.contains('['),
            "the clause must be escaped, got {week}"
        );
    }

    #[test]
    fn the_dates_a_window_asks_for_are_the_dates_it_means() {
        assert_eq!(stamp(20_000, false), "202410040000");
        assert_eq!(stamp(20_000, true), "202410042359");
        // A month back from 2024-10-04 is 2024-09-04, across a month boundary.
        let month = Window::Month.clause(20_000).expect("a month has a clause");
        assert_eq!(month, "submittedDate:[202409040000 TO 202410042359]");
        // A week back from 2024-03-04 crosses a leap day.
        let leap = Window::Week.clause(19_786).expect("a week has a clause");
        assert_eq!(leap, "submittedDate:[202402260000 TO 202403042359]");
        assert_eq!(Window::Any.clause(20_000), None, "any time has no range");
    }

    #[test]
    fn the_window_control_cycles_back_round_to_any_time() {
        let mut window = Window::default();
        assert_eq!(window, Window::Any);
        window = window.next();
        assert_eq!(window, Window::Week);
        window = window.next();
        assert_eq!(window, Window::Month);
        window = window.next();
        assert_eq!(window, Window::Any, "the cycle has to close");
    }

    #[test]
    fn a_paper_kept_twice_is_kept_once() {
        let mut app = Arxiv::default();
        app.library.push(Kept {
            id: "2401.00003".into(),
            title: "A Paper".into(),
            authors: "D. Dedekind".into(),
            bytes: 5,
        });
        assert!(app.is_kept("2401.00003"));
        assert!(!app.is_kept("2401.00004"));
    }

    /// The whole point of the persistence work: a paper reopens where it was
    /// left, rather than at the top.
    ///
    /// This used to be dropped on the floor. The reader kept a place
    /// perfectly well and handed it back on request, but this application
    /// passed `Memory::default()` on every open and never once wrote one
    /// down, so every paper opened at page one however far into it somebody
    /// had got, and every highlight and note went with it.
    #[test]
    fn a_paper_reopens_at_the_place_it_was_left() {
        let long = format!(
            "<article>{}</article>",
            "<p>A paragraph of text.</p>".repeat(400)
        );

        // Read it once, turn some pages, and leave.
        let mut runner = AppRunner::new(Arxiv::default());
        let _ = opened_on(&mut runner, &long);
        for _ in 0..4 {
            runner.action(action_id("read-next"));
        }
        // Marginalia as well as a place, because the reader keeps all of it
        // in the one record and this application used to discard all of it.
        {
            let reader = runner
                .app_mut()
                .book
                .reader_mut()
                .expect("the paper is not open");
            let panel = kobo_sdk::CLARA_BW_METRICS;
            reader.toggle_bookmark();
            reader.toggle_highlight(2, &panel);
            reader
                .create_annotation(
                    1,
                    kobo_read::TextRange {
                        start: kobo_read::TextPosition {
                            block: 2,
                            offset: 0,
                        },
                        end: kobo_read::TextPosition {
                            block: 2,
                            offset: 5,
                        },
                    },
                    Some("Worth coming back to"),
                    &panel,
                )
                .expect("the annotation was refused");
        }
        let left_at = runner
            .app()
            .book
            .memory()
            .expect("the paper is not open")
            .clone();
        assert!(
            left_at.at > 0,
            "the test did not manage to turn a page, so it proves nothing"
        );
        let saved = runner
            .action(kobo_sdk::ActionId::BACK)
            .into_iter()
            .find_map(|command| match command {
                Command::Store(kobo_sdk::StoreRequest::Save { key, value }) => Some((key, value)),
                _ => None,
            })
            .expect("leaving the paper wrote nothing down");
        assert_eq!(saved.0, place_key(&paper().id), "saved under the wrong key");

        // Come back to it. The place comes out of the store before the paper
        // comes off the network, which is the ordinary order.
        let mut again = AppRunner::new(Arxiv::default());
        again.app_mut().papers = vec![paper()];
        again.app_mut().open = Some(0);
        again.start();
        again.action(action_id(FULL_TEXT));
        again.store_result(StoreResult::Loaded {
            key: saved.0,
            value: Some(saved.1),
        });
        let task = again.app().task.expect("no fetch").0;
        again.task_outcome(task, TaskOutcome::Completed(long.into_bytes()));

        let reopened = again.app().book.memory().expect("the paper is not open");
        assert_eq!(
            reopened.at, left_at.at,
            "the paper opened at the top instead of where it was left"
        );
        assert_eq!(
            reopened.bookmarks, left_at.bookmarks,
            "the bookmarks did not come back"
        );
        assert_eq!(
            reopened.highlights, left_at.highlights,
            "the highlights did not come back"
        );
        assert_eq!(
            reopened.annotations, left_at.annotations,
            "the notes did not come back"
        );
    }

    /// The same thing when the two arrive the other way round.
    #[test]
    fn a_place_that_arrives_after_the_paper_still_counts() {
        let long = format!(
            "<article>{}</article>",
            "<p>A paragraph of text.</p>".repeat(400)
        );
        let mut runner = AppRunner::new(Arxiv::default());
        let _ = opened_on(&mut runner, &long);
        assert_eq!(runner.app().book.memory().expect("open").at, 0);

        let place = Memory {
            at: 3,
            ..Memory::default()
        };
        runner.store_result(StoreResult::Loaded {
            key: place_key(&paper().id),
            value: Some(place.encode()),
        });
        assert_eq!(
            runner.app().book.memory().expect("open").at,
            3,
            "a place that lost the race was thrown away"
        );
    }

    /// Keeping a paper puts the rendering somewhere it can be read again with
    /// no network, and says so in the catalogue.
    #[test]
    fn keeping_a_paper_writes_it_to_the_shelf_and_lists_it() {
        let rendering = "<article><h2>1 Introduction</h2><p>Words.</p></article>";
        let mut runner = AppRunner::new(Arxiv::default());
        let _ = opened_on(&mut runner, rendering);

        let commands = runner.action(action_id(KEEP));
        assert!(
            runner.app().is_kept(&paper().id),
            "the paper was not listed in the library"
        );
        let wrote = commands.iter().any(|command| {
            matches!(
                command,
                Command::Store(kobo_sdk::StoreRequest::ShelfWrite { name, .. })
                    if *name == blob_key(&paper().id)
            )
        });
        assert!(wrote, "nothing was written to the shelf");

        // And the catalogue goes down too, or the library is empty next time.
        let listed = commands.iter().any(|command| {
            matches!(
                command,
                Command::Store(kobo_sdk::StoreRequest::Save { key, .. }) if key == LIBRARY_KEY
            )
        });
        assert!(listed, "the catalogue was not saved");
    }

    /// Discarding takes back the space as well as the entry. A library that
    /// forgets a paper but keeps its megabyte is a leak with a nice name.
    #[test]
    fn discarding_a_paper_takes_its_rendering_off_the_shelf_too() {
        let rendering = "<article><p>Words.</p></article>";
        let mut runner = AppRunner::new(Arxiv::default());
        let _ = opened_on(&mut runner, rendering);
        runner.action(action_id(KEEP));

        let commands = runner.action(action_id(DISCARD));
        assert!(!runner.app().is_kept(&paper().id), "still listed");
        let removed = commands.iter().any(|command| {
            matches!(
                command,
                Command::Store(kobo_sdk::StoreRequest::ShelfRemove { name })
                    if *name == blob_key(&paper().id)
            )
        });
        assert!(removed, "the rendering was left on the shelf");
    }

    /// Keeping from the abstract fetches the paper rather than refusing.
    #[test]
    fn keeping_from_an_abstract_fetches_the_paper_and_then_keeps_it() {
        let mut runner = AppRunner::new(Arxiv::default());
        runner.app_mut().papers = vec![paper()];
        runner.app_mut().open = Some(0);
        runner.app_mut().view = View::Paper;
        runner.start();

        let commands = runner.action(action_id(KEEP));
        let asked = commands.iter().any(|command| {
            matches!(command, Command::Spawn { work: Task::Fetch { url, .. }, .. }
                if url.contains("arxiv.org/html/"))
        });
        assert!(asked, "Keep from the abstract fetched nothing");
        assert!(!runner.app().is_kept(&paper().id), "kept before it arrived");

        let task = runner.app().task.expect("no fetch").0;
        let commands = runner.task_outcome(
            task,
            TaskOutcome::Completed(b"<article><p>Words.</p></article>".to_vec()),
        );
        assert!(
            runner.app().is_kept(&paper().id),
            "the fetch it started did not end in the library"
        );
        assert!(
            commands.iter().any(|command| matches!(
                command,
                Command::Store(kobo_sdk::StoreRequest::ShelfWrite { .. })
            )),
            "nothing reached the shelf"
        );
    }

    /// A paper that arrived in pieces is not a paper to keep.
    #[test]
    fn a_truncated_paper_is_not_put_in_the_library() {
        let mut runner = AppRunner::new(Arxiv::default());
        // No closing tag, which is how a rendering cut off at the byte
        // ceiling arrives.
        let _ = opened_on(&mut runner, "<article><p>Half of a pa");
        assert!(runner.app().truncated, "the test did not truncate anything");

        runner.action(action_id(KEEP));
        assert!(
            !runner.app().is_kept(&paper().id),
            "half a paper was put in a library meant for reading offline"
        );
    }

    /// Changing the window asks the question again rather than filtering what
    /// is already on the screen, because the window is part of the question.
    #[test]
    fn changing_the_window_asks_arxiv_again_from_the_first_result() {
        let mut runner = AppRunner::new(Arxiv::default());
        runner.start();
        runner.action(action_id("subject-0"));
        let first = runner.app().task.expect("no listing was asked for").0;
        runner.task_outcome(first, TaskOutcome::Completed(Vec::new()));

        let commands = runner.action(action_id(WINDOW));
        assert_eq!(runner.app().window, Window::Week);
        let asked = commands.iter().find_map(|command| match command {
            Command::Spawn {
                work: Task::Fetch { url, .. },
                ..
            } => Some(url.clone()),
            _ => None,
        });
        let url = asked.expect("changing the window asked for nothing");
        assert!(url.contains("submittedDate"), "{url}");
        assert!(
            url.contains("start=0"),
            "the window kept an old offset: {url}"
        );
    }
}

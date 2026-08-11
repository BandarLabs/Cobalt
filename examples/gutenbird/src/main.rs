//! Gutenbird: an OPDS client for the device.
//!
//! Read the Open Publication Distribution System -- the shape nearly every
//! ebook library on the open web answers in -- rather than one website's own
//! JSON. Project Gutenberg, Standard Ebooks, Open Library and the OPDS
//! conformance catalogs are built in; a reader may add any other by URL. See
//! `docs/OPDS.md` for why this changed and what the real catalogs actually do,
//! and `SPEC.md` for the shape this application follows.
//!
//! ## Why the interface never says which version answered
//!
//! OPDS comes in two incompatible wire formats -- Atom for 1.2, JSON for
//! 2.0 -- and `kobo_opds` reads both into one [`kobo_opds::Feed`]. Nothing on
//! this panel is allowed to ask which parser produced it: no badge, no screen
//! that exists for one version and not the other, no wording that changes
//! because a language arrived as `dcterms:language` rather than
//! `metadata.language`. A reader who adds a catalog should never be able to
//! tell which specification it implements.
//!
//! ## Why an EPUB is worth the wait
//!
//! This used to stream Project Gutenberg's plain text, because a zip archive
//! cannot be read until its last byte has arrived and the text could be shown
//! from the first one. That trade is no longer the right one: `kobo-doc` can
//! now read an EPUB, and throwing away every heading, every italic and the
//! table of contents in exchange for a first page a few seconds sooner is not
//! a trade most readers would choose if it were put to them plainly. An EPUB
//! is fetched in pieces, assembled whole, and only then opened; plain text
//! remains a fallback for the catalogs -- and there are real ones -- that
//! publish nothing else.
//!
//! ## Why the reading screen is not built here
//!
//! Type size, front light, bookmarks and marked passages are not gutenbird's
//! to invent -- every application that shows a book wants the same ones, and a
//! reader who learns them in one should find them in the next. They live in
//! `kobo-read`.

use kobo_opds::{AcquisitionKind, Category, Feed, ImageSource, Link, Publication, SearchTemplate};
use kobo_read::{Memory, Outcome, Reader};
use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Chrome, Context, DiagnosticSeverity, DisplayMetrics, Failure,
    FontHandle, Glyph, Header, KoboApp, LogLevel, PictureHandle, RowLead, ScreenBuilder,
    ShelfDownload, ShelfProgress, ShelfUpload, StoreResult, Task, TaskId, TaskOutcome, Tile,
    TilePicture, TileShape, TileState, MAX_STORE_VALUE,
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::process::ExitCode;

/// The catalogs built in, and the only place any of them is named.
///
/// Everything past this table treats a catalog as an address and a display
/// name; nothing downstream branches on which of these four it is, which is
/// what lets a reader's own addition behave exactly like one of these. Added
/// catalogs are stored on the device; these are rebuilt from this table on
/// every launch, so that changing the table changes what a reader sees
/// without a migration.
const CATALOGS: &[(&str, &str)] = &[
    (
        "Project Gutenberg",
        "https://www.gutenberg.org/ebooks.opds/",
    ),
    (
        "Standard Ebooks",
        "https://standardebooks.org/feeds/atom/new-releases",
    ),
    ("Open Library", "https://openlibrary.org/opds"),
    (
        "OPDS 2.0 Test Catalog",
        "https://test.opds.io/2.0/home.json",
    ),
];

/// How much of a feed page to accept.
///
/// The richest vendored fixture -- Open Library's root, with groups, facets
/// and a cover on every entry -- is under a hundred kilobytes. This is
/// generous past that and still far under `MAX_TASK_BYTES`.
const FEED_BYTES: u32 = 256 * 1024;

/// How much of a book to ask for at a time, whether it is an EPUB or the
/// plain text fallback.
///
/// Around a hundred and fifty pages of text, or a slice of a zip a book's
/// worth of pieces wide. Smaller would mean more round trips than the wait
/// buys back; larger risks the transport ceiling once headers are counted.
const CHUNK_BYTES: u32 = 256 * 1024;

/// How much of a cover to accept.
///
/// Real covers on the catalogs this was tested against run from thirty
/// kilobytes (Gutenberg) to a little over a hundred (Standard Ebooks). The
/// ceiling is what stops a mis-typed URL pulling a megabyte down a slow radio
/// for a thumbnail.
const COVER_BYTES: u32 = 512 * 1024;

/// How many placeholder rows stand in for a feed while it is arriving.
const SKELETON_ROWS: u8 = 6;

/// The most publications held on one shelf page at once, across every `next`
/// page followed.
///
/// A ceiling rather than none at all: a reader who holds "More" down should
/// not be able to make this application grow until the runtime kills it.
const MAX_PUBLICATIONS: usize = 320;

/// How many books one shelf page holds.
///
/// Three columns of portrait tiles, two rows deep, which is what fits whole
/// between the bars on this panel.
const SHELF_PAGE: usize = 6;

/// Cover fetches allowed at once.
///
/// One below the runtime's ceiling of four on purpose, so a shelf filling in
/// can never leave a search or a download with nowhere to go.
const COVER_LANES: usize = 3;

/// Checked here rather than in a test, because the cost of getting it wrong
/// is a shelf that silently delays every search behind its own artwork, and
/// that is a mistake worth refusing to compile.
const _: () = assert!(COVER_LANES < kobo_sdk::MAX_TASKS_IN_FLIGHT);

/// Attempts spent on one cover before it is given up on.
const COVER_TRIES: u8 = 3;

/// A cover shorter or narrower than this, once decoded, is a Gutenberg
/// category icon rather than art -- its navigation thumbnails are 22x22
/// pixels -- and is treated as no cover at all rather than stretched into
/// one, which would look like a decoding fault rather than a small picture.
const MIN_COVER_PX: u32 = 32;

/// The largest book this will take from a catalog, in bytes.
///
/// A zip cannot be read a piece at a time -- its directory is at the end --
/// so the whole of a book is in memory at once while it is parsed, alongside
/// the blocks and pictures that come out of it. This reader has 448 MB in
/// total and shares them with the firmware it is pretending not to be, and
/// Gutenberg's illustrated Pride and Prejudice, at twenty-four, took the
/// device down with it.
///
/// Twelve is comfortably more than any book of prose and enough for a
/// moderately illustrated one. A catalog offering nothing smaller is refused
/// on the page rather than part-way through the download, because a reader
/// who has watched a progress bar for four minutes has already been charged
/// for the failure.
const MAX_BOOK_BYTES: u64 = 12 * 1024 * 1024;

/// The most pictures decoded when a book opens.
///
/// Every one costs memory the reader shares with the firmware, and a novel's
/// illustrations run to a few dozen. This is above that and far below the
/// point where a book of photographs takes the device down with it.
const MAX_BOOK_PICTURES: usize = 64;

/// How large a picture inside a book is decoded to, in millimetres.
///
/// Fitted before it is handed over rather than after, so what the runtime
/// holds is the size it will be drawn: a plate scanned at print resolution is
/// several megabytes of pixels the panel has no way to show. The height
/// matches the ceiling `kobo-read` draws pictures at, and the width is the
/// column it has to fit inside.
const PICTURE_WIDTH_MM: u16 = 80;
const PICTURE_HEIGHT_MM: u16 = 90;

/// Where a book's picture handles start.
///
/// The shelf is holding cover handles numbered from zero at the same time, and
/// two pictures sharing a handle is the sort of fault that shows up as the
/// wrong illustration rather than as an error.
const PICTURE_HANDLE_BASE: u32 = 1_000;

/// Where the handles for navigation tile pictures start, and how many of them
/// are cycled through.
///
/// Kept apart from both the shelf's own cover handles and a book's pictures,
/// because two pictures sharing a handle shows up as the wrong illustration
/// rather than as an error. Cycled rather than grown without end: a reader
/// paging through a long catalog would otherwise ask the runtime to hold a
/// picture for every book they had passed.
const NAV_COVER_HANDLE_BASE: u32 = 2_000;
const NAV_COVER_HANDLES: u32 = 64;

/// How many rows are followed at once to find their pictures.
///
/// Two rather than the three the shelf's own covers get, and both together
/// stay under the runtime's ceiling of four, so a reader's tap always has a
/// lane waiting for it. Six tiles then fill in three rounds instead of six.
const FILL_LANES: usize = 2;

/// How wide the cover in a book's hero is drawn.
const DETAILS_COVER_MM: u16 = 30;

/// The box a book's own cover is always given, in pixels.
///
/// Always: the room is claimed before the bytes arrive and every cover is
/// padded out to exactly this, so the shape of what turns up cannot change
/// what is around it. A detail page drew with no cover, then redrew with one
/// above the title block, and everything under it -- Read included -- moved
/// about two hundred pixels down the panel. An owner reaching for Read as the
/// page appeared pressed the About text instead and turned the page. On a slow
/// catalog the gap between the two layouts is seconds rather than an instant.
///
/// Three by two, which is the shape of nearly every book cover ever printed, so
/// the padding is usually a few pixels of the paper it sits on.
const OPEN_COVER_PX: (u32, u32) = (DETAILS_COVER_MM as u32 * 8, DETAILS_COVER_MM as u32 * 12);

/// The handle the open book's cover is always held against.
const OPEN_COVER_HANDLE: PictureHandle = PictureHandle(u32::MAX);
const PUBLISHER_FONT_HANDLE: FontHandle = FontHandle(1);

/// How close to the end of what has been downloaded the reader may get before
/// the next piece of a plain-text fallback book is requested.
const TOP_UP_PAGES: usize = 2;

/// How many recent searches one catalog's search screen keeps.
const MAX_RECENT: usize = 6;

/// How many steps back the application remembers.
const MAX_TRAIL: usize = 8;

/// The store key the added-catalog registry is written under.
const REGISTRY_KEY: &str = "catalogs";
/// The store key naming which catalog was open when the reader last left.
const LAST_OPEN_KEY: &str = "catalog-open";

/// One catalog the reader can browse.
///
/// Whichever of OPDS's two versions it speaks is a fact this application
/// never asks after the root feed is parsed; see the module documentation.
#[derive(Clone, Debug)]
struct Catalog {
    name: String,
    root: String,
    /// Whether the reader added this one, which is what decides whether it
    /// is written back to the registry -- a built-in is rebuilt from
    /// [`CATALOGS`] every launch and would otherwise be written twice.
    added: bool,
    /// The search capability, resolved at most once per catalog per session
    /// and kept beside the catalog so an `OpenSearch` description document is
    /// fetched on the first search and never again.
    search: SearchState,
    /// Searches already run against this catalog this session, most recent
    /// first. A search of Gutenberg is not a search of anything else, so this
    /// travels with the catalog rather than living in one shared list.
    recent: Vec<String>,
}

impl Catalog {
    fn new(name: impl Into<String>, root: impl Into<String>, added: bool) -> Self {
        Self {
            name: name.into(),
            root: root.into(),
            added,
            search: SearchState::Unknown,
            recent: Vec::new(),
        }
    }
}

/// How this application reaches a catalog's search results, resolved once
/// and reused: either the feed's own `search` link, already usable as a
/// template, or one read out of the `OpenSearch` description document that
/// link pointed at. See `docs/OPDS.md`'s Search section for why OPDS 1.2 pays
/// a round trip 2.0 does not.
#[derive(Clone, Debug, PartialEq)]
enum SearchWay {
    Direct(Link),
    Template(SearchTemplate),
}

/// A catalog's search capability, discovered lazily: nothing is fetched
/// until a reader actually tries to search.
#[derive(Clone, Debug, Default, PartialEq)]
enum SearchState {
    #[default]
    Unknown,
    /// Asked after and found to offer none.
    None,
    Known(SearchWay),
}

impl SearchWay {
    fn expand(&self, query: &str) -> Option<String> {
        match self {
            Self::Direct(link) => kobo_opds::expand_search(link, query),
            Self::Template(template) => Some(template.expand(query)),
        }
    }
}

/// Whether a `search` link is already something to expand directly, or names
/// a document -- an `OpenSearch` description -- that has to be fetched first.
fn direct_search_way(link: &Link) -> Option<SearchWay> {
    let is_description = link
        .media_type
        .as_deref()
        .is_some_and(|value| value.to_ascii_lowercase().contains("opensearchdescription"));
    if is_description {
        None
    } else {
        Some(SearchWay::Direct(link.clone()))
    }
}

/// One followed feed, kept so Back can return to the page before this one
/// rather than leave the application. A catalog is a tree, and Back means
/// "the feed before this," which extends the existing rule that Back unwinds
/// the application before leaving it.
struct StackEntry {
    feed: Feed,
    /// The address this page was fetched from, so a `next` link is checked
    /// against the right host and a retry knows what to re-fetch.
    url: String,
    /// The next page, kept only when the catalog offered one and it stays on
    /// this catalog's own host.
    next: Option<String>,
    /// Which shelf or row page of this feed is showing.
    page: usize,
    /// Which catalog each of `feed.publications` came from, parallel to that
    /// list. Empty for an ordinary feed; filled in only for the one page
    /// that answers a search run against every catalog at once, so each row
    /// can say where it came from.
    sources: Vec<String>,
    /// Decoded covers for `feed.publications`, parallel to that list.
    /// Nothing is fetched for a page the reader has not turned to yet.
    covers: Vec<Option<TilePicture>>,
    /// The navigation entries already followed to see whether they are books.
    ///
    /// Held by address rather than by position, because a row that turns out
    /// to be a book leaves the navigation list and moves to the shelf, and
    /// every position after it shifts when it does.
    examined: BTreeSet<String>,
    /// Covers for `feed.navigation`, parallel to that list.
    ///
    /// A navigation entry is drawn as a tile like any other, so that a
    /// catalog which serves no shelf still looks like a shelf. The picture
    /// arrives later, or never, and only the picture changes: the tile is in
    /// place from the first draw, which is what stops the page moving under
    /// the reader.
    nav_covers: Vec<Option<TilePicture>>,
}

impl StackEntry {
    fn fresh(feed: Feed, url: String) -> Self {
        let next = feed
            .next()
            .map(|link| link.href.clone())
            .filter(|next| kobo_opds::same_origin(next, &url));
        let covers = vec![None; feed.publications.len()];
        let navigation = feed.navigation.len();
        Self {
            feed,
            url,
            next,
            page: 0,
            sources: Vec::new(),
            covers,
            nav_covers: vec![None; navigation],
            examined: BTreeSet::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum View {
    Catalogs,
    AddCatalog,
    Shelf,
    Search,
    Details,
    Reading,
    Note,
}

/// What a feed answer, once parsed, should become.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FeedPurpose {
    /// Replaces the whole stack: opening a catalog fresh.
    Root { catalog: usize },
    /// Pushed onto the stack: a navigation row was followed, or a search was
    /// run against the catalog already open. If the answer is a complete
    /// catalog entry document, this becomes an open book instead of a page.
    Push { catalog: usize },
    /// The next page of whatever is already on top of the stack, folded into
    /// it rather than replacing it.
    More,
    /// One catalog's share of a search run against every catalog at once.
    Federated { catalog: usize },
}

/// What one of the shelf's own background requests is for.
///
/// Kept apart from [`Awaiting`], which is the single request a reader is
/// actually waiting on. These run in their own lanes so that filling a shelf
/// in can never be the reason a tap does nothing.
#[derive(Clone, Debug, Eq, PartialEq)]
enum FillStage {
    /// Following a row to see whether it is a book, and where its picture is.
    ///
    /// Project Gutenberg never serves a shelf: every book in it, even in a
    /// bookshelf or an author's list, is a navigation entry pointing at that
    /// book's own document, carrying a twenty-two pixel icon and no cover. A
    /// catalog like that would draw as a page of identical rows, so the
    /// entries on the page being looked at are followed to see which of them
    /// are books, and the ones that are move to the shelf.
    Entry { href: String },
    /// Fetching that picture.
    Picture { href: String },
}

/// What a completed task should be applied to.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Awaiting {
    /// A feed page, and the address it was actually fetched from -- needed
    /// again once the bytes land, since every relative href inside them
    /// resolves against it.
    Feed(FeedPurpose, String),
    /// A catalog's root feed, fetched only because nothing about it has been
    /// fetched yet and a search needs to know whether it offers one at all.
    DiscoverRoot {
        catalog: usize,
        query: String,
        federated: bool,
    },
    /// An `OpenSearch` description document, read once to learn a 1.x
    /// catalog's search template.
    DiscoverDescription {
        catalog: usize,
        query: String,
        federated: bool,
        url: String,
    },
    /// A book's bytes, in pieces.
    Book,
}

/// One piece of a book's trailing description, so pages can be packed from
/// it without cutting a paragraph or a heading across a page turn.
#[derive(Clone, Debug)]
enum DetailBlock {
    Section(&'static str),
    Text(String),
    Categories(Vec<Category>),
    Facts(Vec<(String, String)>),
}

impl DetailBlock {
    fn add(&self, screen: ScreenBuilder) -> ScreenBuilder {
        match self {
            Self::Section(title) => screen.section(*title),
            Self::Text(text) => screen.text(text.clone()),
            // Rows rather than chips, because a category is a phrase rather
            // than a word. "Fiction -- England -- 19th century" in a chip is a
            // square box holding one line of small type with air above and
            // below it, and a screen of them showed four; the same phrases as
            // rows read as the list they are, several to a screen, with room
            // for the scheme they came from underneath.
            Self::Categories(categories) => {
                screen.rows(categories.iter().enumerate().map(|(index, category)| {
                    let label = category
                        .label
                        .clone()
                        .unwrap_or_else(|| category.term.clone());
                    // The term only when it says something the label does not.
                    // Most catalogs set both to the same string, and a row that
                    // repeats its own title underneath itself is noise.
                    let summary = if category.term == label {
                        String::new()
                    } else {
                        category.term.clone()
                    };
                    (
                        format!("category-{index}"),
                        label,
                        summary,
                        RowLead::Icon(Glyph::Search),
                    )
                }))
            }
            Self::Facts(facts) => screen.facts(facts.clone()),
        }
    }
}

/// What the Read button offers, worked out once from a publication's
/// acquisition links so the details page and the download both agree on it.
#[derive(Clone, Debug, PartialEq)]
enum ReadOffer {
    /// An EPUB or, failing that, plain text -- whichever `best_acquisition`
    /// picked.
    Read,
    /// A sample, never dressed up as the whole book.
    Sample,
    /// Buy, borrow or subscribe only: a price when the catalog stated one,
    /// and never a button that would fail.
    Unavailable(Option<kobo_opds::Price>),
    /// Nothing this application can offer at all -- including the case where
    /// every link on the entry was unsafe and the acquisition list survived
    /// empty by design.
    Nothing,
}

fn read_offer(publication: &Publication) -> ReadOffer {
    if let Some(acquisition) = publication.best_acquisition() {
        return if acquisition.kind == AcquisitionKind::Sample {
            ReadOffer::Sample
        } else {
            ReadOffer::Read
        };
    }
    if publication.acquisition.is_empty() {
        ReadOffer::Nothing
    } else {
        let price = publication
            .acquisition
            .iter()
            .find_map(|acquisition| acquisition.price.clone());
        ReadOffer::Unavailable(price)
    }
}

/// Which of the two formats a download turned out to be, decided from the
/// chosen acquisition's media type before a single byte arrives.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DownloadKind {
    Epub,
    Text,
}

fn download_kind(media_type: Option<&str>) -> DownloadKind {
    let base = media_type
        .and_then(|value| value.split(';').next())
        .unwrap_or_default();
    if base.eq_ignore_ascii_case("text/plain") {
        DownloadKind::Text
    } else {
        DownloadKind::Epub
    }
}

/// A book on its way in, whichever format it turned out to be.
struct Download {
    url: String,
    kind: DownloadKind,
    bytes: Vec<u8>,
    /// The acquisition's own `length`, when the catalog stated one -- what
    /// lets the progress bar show a real fraction rather than only bytes so
    /// far.
    total: Option<u64>,
}

#[allow(clippy::struct_excessive_bools)]
struct Gutenbird {
    view: View,
    /// The screens stepped through to reach the one showing, oldest first.
    ///
    /// Back used to name a fixed destination per screen, and every screen but
    /// one named the shelf. That left the catalog list reachable only through
    /// the globe, which does not look like navigation, and leaving it put the
    /// owner back on the catalog they had pressed the globe to get away from --
    /// a loop with no way out of the catalog they were already in. The root of
    /// a catalog had no Back control at all, because it was the one screen with
    /// nowhere fixed to go.
    ///
    /// Recording the step instead answers both: the catalog list is a screen
    /// above the shelf and Back reaches it, and Back from anywhere undoes the
    /// step that was taken rather than returning to a screen chosen in advance.
    ///
    /// Pages followed inside one catalog are deliberately absent: they are in
    /// [`Gutenbird::stack`], which Back unwinds first, because they are that
    /// catalog's own history rather than the application's.
    trail: Vec<View>,
    keyboard: Keyboard,
    pending_annotation: Option<kobo_read::AnnotationId>,

    catalogs: Vec<Catalog>,
    current: usize,
    stack: Vec<StackEntry>,

    /// Which navigation tile picture handle to use next.
    nav_cover_handle: u32,

    /// The book on the details or reading screen, if any.
    open: Option<Publication>,
    open_cover: Option<TilePicture>,
    open_cover_task: Option<(TaskId, u8)>,

    /// Whether the Search screen's toggle asks every catalog at once rather
    /// than only the one open.
    search_all: bool,
    /// Catalogs still to ask, in a federated search already under way.
    all_queue: VecDeque<usize>,
    /// The term a federated search is running, kept so each catalog in the
    /// queue is asked the same thing.
    federated_query: Option<String>,
    federating: bool,

    task: Option<(TaskId, Awaiting)>,
    problem: Option<String>,
    trouble: Option<Failure>,

    /// Publications on the current shelf page still to fetch a cover for,
    /// most recently queued first, with attempts already spent.
    wanted: Vec<(usize, u8)>,
    /// Cover fetches in flight at once.
    covers: Vec<(TaskId, usize, u8)>,
    /// Covers the store is being asked about, by the key each answer will
    /// carry.
    looking: Vec<(String, usize)>,
    /// Rows being followed to find their pictures, and what each request is
    /// for. Its own lanes rather than the one exclusive slot: a shelf filling
    /// itself in must never be the reason a tap does nothing.
    filling: Vec<(TaskId, FillStage)>,

    detail_page: usize,

    download: Option<Download>,
    fetched: u32,
    complete: bool,
    reader: Option<Reader>,
    /// The handles the runtime is holding plates against for the open book.
    ///
    /// Kept because a handle is the only thing that can give one back. The
    /// runtime frees every picture when an application exits, which is no help
    /// at all to an application that stays open: a book of plates was still
    /// costing the device several megabytes of decoded greyscale while its
    /// owner was back on the shelf looking at covers.
    book_pictures: Vec<PictureHandle>,
    /// The embedded face held for the open book, released with its pictures.
    book_font: Option<FontHandle>,
    /// The open book's plates still to be turned into pixels, in the order the
    /// text refers to them, each with the bytes it was stored as.
    ///
    /// Drained rather than iterated: a plate's source bytes are dropped the
    /// moment it has been decoded, so what an illustrated book costs while it
    /// opens falls as it goes rather than being both copies at once.
    plates: VecDeque<(String, Vec<u8>)>,
    /// A plate read from the book but not yet reduced to the panel's greys.
    ///
    /// The two halves of preparing a plate happen in separate callbacks
    /// because together they are longer than one is allowed to be, so the
    /// half-finished picture has to live somewhere in between.
    dithering: Option<(String, kobo_image::Picture)>,
    /// The sleep that will bring the next pass at the queue.
    plating: Option<TaskId>,
    place: Option<Memory>,
    keeping: Option<ShelfUpload>,
    loading: Option<ShelfDownload>,
    stored: BTreeMap<String, u32>,
    failed: Option<String>,
    retryable: bool,

    add_catalog_problem: Option<String>,
}

impl Default for Gutenbird {
    fn default() -> Self {
        Self {
            nav_cover_handle: 0,
            // The catalog list, not a shelf. Starting up opens the catalog
            // that was last open, which is a step taken from here, and that is
            // what puts a way back to the list under the first shelf drawn.
            view: View::Catalogs,
            trail: Vec::new(),
            keyboard: Keyboard::new(),
            pending_annotation: None,
            catalogs: CATALOGS
                .iter()
                .map(|(name, root)| Catalog::new(*name, *root, false))
                .collect(),
            current: 0,
            stack: Vec::new(),
            open: None,
            open_cover: None,
            open_cover_task: None,
            search_all: false,
            all_queue: VecDeque::new(),
            federated_query: None,
            federating: false,
            task: None,
            problem: None,
            trouble: None,
            wanted: Vec::new(),
            covers: Vec::new(),
            looking: Vec::new(),
            filling: Vec::new(),
            detail_page: 0,
            download: None,
            fetched: 0,
            complete: false,
            reader: None,
            book_pictures: Vec::new(),
            book_font: None,
            plates: VecDeque::new(),
            dithering: None,
            plating: None,
            place: None,
            keeping: None,
            loading: None,
            stored: BTreeMap::new(),
            failed: None,
            retryable: false,
            add_catalog_problem: None,
        }
    }
}

impl Gutenbird {
    fn awaiting_feed(&self) -> bool {
        matches!(
            self.task,
            Some((
                _,
                Awaiting::Feed(..)
                    | Awaiting::DiscoverRoot { .. }
                    | Awaiting::DiscoverDescription { .. }
            ))
        )
    }

    fn awaiting_book(&self) -> bool {
        matches!(self.task, Some((_, Awaiting::Book)))
    }

    fn current_catalog(&self) -> &Catalog {
        &self.catalogs[self.current]
    }

    fn show(&self, context: &mut Context) {
        let screen = match self.view {
            View::Catalogs => self.catalogs_screen(),
            View::AddCatalog => self.add_catalog_screen(),
            View::Shelf => self.shelf_screen(context),
            View::Search => self.search_screen(),
            View::Details => self.details_screen(context),
            View::Reading => self.reading_screen(context),
            View::Note => self.note_screen(),
        };
        // The application draws its own Back wherever it has somewhere to go,
        // which is every screen except the catalog list with nothing open over
        // it. Only there does Back mean leaving.
        context.set_screen(screen.with_own_back(self.can_go_back()));
    }

    /// Takes a step to another screen, remembering the one being left.
    ///
    /// For a step forward only. A screen returned to unwinds the trail through
    /// [`Self::back_to`] instead, or the trail grows by one every time an
    /// owner walks the same two screens.
    fn go(&mut self, view: View) {
        if self.view == view {
            return;
        }
        self.trail.push(self.view);
        // Bounded, because a reader who paces between a shelf and its search
        // for an hour is still only ever going to press Back a few times, and
        // an unbounded list of where they have been is a leak with a nice name.
        if self.trail.len() > MAX_TRAIL {
            self.trail.remove(0);
        }
        self.view = view;
    }

    /// Returns to a screen, undoing the step that left it.
    ///
    /// The step is only undone when it is the one on top: a screen that
    /// finished for its own reasons -- a download that failed back to the
    /// book page -- is arriving somewhere rather than retracing.
    fn back_to(&mut self, view: View) {
        if self.trail.last() == Some(&view) {
            self.trail.pop();
        }
        self.view = view;
    }

    /// The screen Back would return to, without taking the step.
    fn way_back(&self) -> Option<View> {
        self.trail
            .iter()
            .rev()
            .copied()
            .find(|previous| *previous != self.view)
    }

    /// Whether Back is the application's to answer rather than the runtime's.
    fn can_go_back(&self) -> bool {
        self.way_back().is_some() || (self.view == View::Shelf && self.stack.len() > 1)
    }

    /// Takes the step [`Self::way_back`] describes.
    ///
    /// Entries matching the screen already showing are dropped on the way
    /// past. A search answer replaces the shelf without being a step from it,
    /// so the shelf can be both where the trail points and where the owner
    /// already is, and Back that appears to do nothing is worse than Back that
    /// goes too far.
    fn step_back(&mut self) -> Option<View> {
        while let Some(previous) = self.trail.pop() {
            if previous != self.view {
                return Some(previous);
            }
        }
        None
    }

    // ---------------------------------------------------------------
    // Catalogs
    // ---------------------------------------------------------------

    fn catalogs_screen(&self) -> kobo_sdk::Screen {
        let mut screen = ScreenBuilder::new("gutenbird-catalogs")
            .top_bar("Catalogs")
            .top_bar_glyph("add-catalog", "Add a catalog", Glyph::Plus);
        if let Some(problem) = &self.problem {
            screen = screen.banner(BannerLevel::Attention, problem.clone());
        }
        let rows = self.catalogs.iter().enumerate().map(|(index, catalog)| {
            let trailing = if index == self.current { "Open" } else { "" };
            (
                format!("catalog-{index}"),
                catalog.name.clone(),
                catalog.root.clone(),
                RowLead::Icon(Glyph::Book),
                trailing.to_owned(),
            )
        });
        screen.rows_with_trailing(rows).build()
    }

    fn add_catalog_screen(&self) -> kobo_sdk::Screen {
        let mut screen = ScreenBuilder::new("gutenbird-add-catalog")
            .top_bar("Add a catalog")
            .field(
                "catalog-url",
                self.keyboard.text(),
                "https://example.org/opds",
            )
            .field_clear("catalog-url-clear");
        if let Some(problem) = &self.add_catalog_problem {
            screen = screen.banner(BannerLevel::Attention, problem.clone());
        }
        screen.keyboard(&self.keyboard, "Add").build()
    }

    /// Whether a reader-typed address is worth trying at all.
    ///
    /// Only `https`, because the runtime refuses anything else and a catalog
    /// that fails at the first fetch has already cost the reader a tap. Not a
    /// full URL grammar -- the fetch itself is the real test of that -- but
    /// enough to refuse an empty field, a bare word, or a scheme this device
    /// will never open before it is written to the registry at all.
    fn looks_like_a_catalog_url(text: &str) -> bool {
        let text = text.trim();
        text.len() > "https://".len()
            && text.to_ascii_lowercase().starts_with("https://")
            && !text.contains(char::is_whitespace)
    }

    fn submit_catalog(&mut self, context: &mut Context) {
        let url = self.keyboard.take();
        self.add_catalog(context, url.trim());
    }

    /// Validates and stores a catalog address, then opens it. Kept apart
    /// from [`Self::submit_catalog`] so the validation and storage this
    /// answers for can be exercised directly, without having to type an
    /// address key by key through the keyboard grid.
    fn add_catalog(&mut self, context: &mut Context, url: &str) {
        if !Self::looks_like_a_catalog_url(url) {
            self.add_catalog_problem =
                Some("That does not look like a catalog address.".to_owned());
            self.show(context);
            return;
        }
        self.add_catalog_problem = None;
        let name = catalog_display_name(url);
        self.catalogs.push(Catalog::new(name, url.to_owned(), true));
        self.save_registry(context);
        self.current = self.catalogs.len() - 1;
        self.open_catalog(context);
    }

    fn open_catalog(&mut self, context: &mut Context) {
        self.stop_federating();
        self.stack.clear();
        self.go(View::Shelf);
        let root = self.current_catalog().root.clone();
        self.save_last_open(context);
        self.spawn_feed(
            context,
            root,
            FeedPurpose::Root {
                catalog: self.current,
            },
        );
    }

    /// Writes the added-catalog registry back, refusing to write past
    /// [`MAX_STORE_VALUE`] -- 256 KiB, which a list of catalog addresses and
    /// names could only approach after several thousand additions, far past
    /// what a reader would ever type in by hand. A write that would exceed
    /// it is silently skipped rather than truncated: a half-written registry
    /// would forget catalogs from the middle of the list rather than only
    /// the newest one.
    fn save_registry(&mut self, context: &mut Context) {
        let bytes = encode_registry(&self.catalogs);
        if bytes.len() <= MAX_STORE_VALUE {
            context.store().save(REGISTRY_KEY, bytes);
        }
    }

    fn save_last_open(&mut self, context: &mut Context) {
        context.store().save(
            LAST_OPEN_KEY,
            self.current_catalog().root.clone().into_bytes(),
        );
    }

    // ---------------------------------------------------------------
    // Feed fetching
    // ---------------------------------------------------------------

    fn spawn_feed(&mut self, context: &mut Context, url: String, purpose: FeedPurpose) {
        self.problem = None;
        self.trouble = None;
        // The shelf's own work runs in lanes of its own and never holds the
        // slot this needs, but a page the reader has left is not worth
        // finishing, so its pictures are dropped rather than waited for.
        self.abandon_hydration(context);
        let headers = vec![Header::new("Accept", kobo_opds::ACCEPT)];
        if let Some(task) = context.spawn_retrying(Task::Fetch {
            url: url.clone(),
            offset: 0,
            max_bytes: FEED_BYTES,
            credential: None,
            headers,
        }) {
            context.log(LogLevel::Info, format!("feed {task:?} <- {url}"));
            self.task = Some((task, Awaiting::Feed(purpose, url)));
        } else {
            context.log(LogLevel::Warn, format!("feed refused, lanes full: {url}"));
            self.problem = Some("Too much is already in flight.".to_owned());
        }
    }

    /// Follows a navigation row, or the current catalog's own root: fetches
    /// it and pushes the result onto the stack once it lands.
    fn follow(&mut self, context: &mut Context, href: String) {
        self.spawn_feed(
            context,
            href,
            FeedPurpose::Push {
                catalog: self.current,
            },
        );
    }

    fn ask_more(&mut self, context: &mut Context) {
        if self.task.is_some() {
            return;
        }
        let Some(next) = self.stack.last().and_then(|entry| entry.next.clone()) else {
            return;
        };
        self.spawn_feed(context, next, FeedPurpose::More);
    }

    /// A feed with exactly one publication (or a few sharing one title, the
    /// shape Gutenberg's `.images`/`.noimages` pair leaves behind) and no
    /// navigation at all is a complete catalog entry document rather than a
    /// page to browse -- OPDS 1.2 section 5.1.2's distinction between a
    /// partial and a complete entry. This is what turns following one of
    /// Gutenberg's `subsection` links into opening a book, without this
    /// application ever having written Gutenberg's name to decide it.
    fn resolve_entry(feed: &Feed) -> Option<Publication> {
        if !feed.navigation.is_empty() {
            return None;
        }
        match feed.publications.len() {
            0 => None,
            1 => Some(feed.publications[0].clone()),
            _ => {
                let mut titles = feed
                    .publications
                    .iter()
                    .map(|publication| publication.title.as_str());
                let first = titles.next()?;
                if !titles.all(|title| title == first) {
                    return None;
                }
                // The illustrated edition, now that an illustration reaches
                // the panel. This was the smaller one for as long as pictures
                // were discarded on the way in, when Gutenberg's illustrated
                // Pride and Prejudice was twenty-five megabytes to draw the
                // same words as the five-hundred-kilobyte edition beside it.
                // It costs radio time, and the download says how much as it
                // goes, but a book's plates are part of the book.
                feed.publications
                    .iter()
                    .find(|publication| {
                        publication.acquisition.iter().any(|acquisition| {
                            acquisition.href.contains(".images") && affordable(acquisition)
                        })
                    })
                    .or_else(|| {
                        // Nothing illustrated small enough to read, so the
                        // plainest edition that will open. A book of words is
                        // better than a book that takes the reader down.
                        feed.publications
                            .iter()
                            .find(|publication| publication.acquisition.iter().any(affordable))
                    })
                    .or_else(|| feed.publications.first())
                    .cloned()
            }
        }
    }

    fn took_feed(
        &mut self,
        context: &mut Context,
        bytes: &[u8],
        purpose: FeedPurpose,
        base: String,
    ) {
        let Ok(mut feed) = kobo_opds::parse(bytes, &base) else {
            context.log(LogLevel::Warn, format!("unreadable answer from {base}"));
            self.problem = Some("That catalog's answer could not be read.".to_owned());
            self.view = View::Shelf;
            return;
        };
        fold_groups(&mut feed);
        match purpose {
            FeedPurpose::Root { catalog } => {
                self.seed_search(catalog, &feed);
                if let Some(publication) = Self::resolve_entry(&feed) {
                    self.open_publication(context, publication);
                } else {
                    self.stack = vec![StackEntry::fresh(feed, base)];
                    self.view = View::Shelf;
                    self.want_covers(context);
                    self.hydrate_visible(context);
                }
            }
            FeedPurpose::Push { catalog } => {
                self.seed_search(catalog, &feed);
                if let Some(publication) = Self::resolve_entry(&feed) {
                    self.open_publication(context, publication);
                } else {
                    self.stack.push(StackEntry::fresh(feed, base));
                    self.view = View::Shelf;
                    self.want_covers(context);
                    self.hydrate_visible(context);
                }
            }
            FeedPurpose::More => {
                if let Some(entry) = self.stack.last_mut() {
                    let origin = entry.url.clone();
                    let next = feed
                        .next()
                        .map(|link| link.href.clone())
                        .filter(|next| kobo_opds::same_origin(next, &origin));
                    let room = MAX_PUBLICATIONS.saturating_sub(entry.feed.publications.len());
                    entry.feed.navigation.extend(feed.navigation);
                    entry
                        .feed
                        .publications
                        .extend(feed.publications.into_iter().take(room));
                    entry.covers.resize(entry.feed.publications.len(), None);
                    entry.next = next;
                    let pages = shelf_pages(entry);
                    entry.page = (entry.page + 1).min(pages.saturating_sub(1));
                }
                self.view = View::Shelf;
                self.want_covers(context);
                self.hydrate_visible(context);
            }
            FeedPurpose::Federated { catalog } => {
                let name = self.catalogs[catalog].name.clone();
                if let Some(entry) = self.stack.last_mut() {
                    let room = MAX_PUBLICATIONS.saturating_sub(entry.feed.publications.len());
                    let taken: Vec<Publication> =
                        feed.publications.into_iter().take(room).collect();
                    entry.sources.extend(std::iter::repeat_n(name, taken.len()));
                    entry.feed.publications.extend(taken);
                    entry.covers.resize(entry.feed.publications.len(), None);
                }
                self.view = View::Shelf;
                self.advance_federated(context);
            }
        }
    }

    /// Opportunistically remembers a catalog's search capability from any
    /// feed of it this application already fetched, so a search on the
    /// catalog the reader is already looking at costs nothing extra.
    fn seed_search(&mut self, catalog: usize, feed: &Feed) {
        if self.catalogs[catalog].search != SearchState::Unknown {
            return;
        }
        let Some(link) = feed.search() else {
            self.catalogs[catalog].search = SearchState::None;
            return;
        };
        if let Some(way) = direct_search_way(link) {
            self.catalogs[catalog].search = SearchState::Known(way);
        }
        // Otherwise the link names an OpenSearch description document this
        // application has not fetched yet -- left unresolved until a search
        // is actually attempted, at which point `begin_search` fetches it.
    }

    fn open_publication(&mut self, context: &mut Context, publication: Publication) {
        self.open = Some(publication);
        // Given back, not merely forgotten. Every book's cover is held against
        // the same handle, and the frame this page reserves names that handle
        // before any bytes have arrived for it -- so the runtime, still
        // holding the last book's pixels, drew them. On the panel, opening The
        // Tale of Peter Rabbit showed Pride and Prejudice's peacock in the
        // space Beatrix Potter's cover was about to take.
        context.drop_picture(OPEN_COVER_HANDLE);
        self.open_cover = None;
        self.open_cover_task = None;
        self.detail_page = 0;
        self.go(View::Details);
        self.download = None;
        self.fetched = 0;
        self.complete = false;
        self.reader = None;
        // Unconditionally, unlike closing a book: arriving at another
        // publication abandons whatever was in flight, so there is nothing
        // left that could ask for these back later.
        for handle in self.book_pictures.drain(..) {
            context.drop_picture(handle);
        }
        self.place = None;
        self.loading = None;
        self.failed = None;
        self.problem = None;
        self.trouble = None;
        if let Some((_, place)) = self.open_keys() {
            context.store().load(place);
        }
        self.ask_open_cover(context);
    }

    // ---------------------------------------------------------------
    // Search
    // ---------------------------------------------------------------

    fn begin_search(
        &mut self,
        context: &mut Context,
        catalog: usize,
        query: String,
        federated: bool,
    ) {
        match self.catalogs[catalog].search.clone() {
            SearchState::Known(way) => self.issue_search(context, catalog, &query, &way, federated),
            SearchState::None => self.search_unavailable(context, federated),
            SearchState::Unknown => {
                let root = self.catalogs[catalog].root.clone();
                let headers = vec![Header::new("Accept", kobo_opds::ACCEPT)];
                match context.spawn_retrying(Task::Fetch {
                    url: root,
                    offset: 0,
                    max_bytes: FEED_BYTES,
                    credential: None,
                    headers,
                }) {
                    Some(task) => {
                        self.task = Some((
                            task,
                            Awaiting::DiscoverRoot {
                                catalog,
                                query,
                                federated,
                            },
                        ));
                    }
                    None => self.problem = Some("Too much is already in flight.".to_owned()),
                }
            }
        }
    }

    fn issue_search(
        &mut self,
        context: &mut Context,
        catalog: usize,
        query: &str,
        way: &SearchWay,
        federated: bool,
    ) {
        let Some(url) = way.expand(query) else {
            self.search_unavailable(context, federated);
            return;
        };
        self.push_recent(catalog, query);
        let purpose = if federated {
            FeedPurpose::Federated { catalog }
        } else {
            FeedPurpose::Push { catalog }
        };
        self.spawn_feed(context, url, purpose);
    }

    fn search_unavailable(&mut self, context: &mut Context, federated: bool) {
        if federated {
            self.advance_federated(context);
        } else {
            self.problem = Some("This catalog has no search.".to_owned());
            self.show(context);
        }
    }

    /// What a fetch of a catalog's root, made solely to discover its search
    /// capability, should do once it lands: read the `search` link straight
    /// off (or find that the catalog has none), or fetch the `OpenSearch`
    /// description document a 1.x link points at before a search can run at
    /// all.
    fn took_discover_root(
        &mut self,
        context: &mut Context,
        bytes: &[u8],
        root: &str,
        catalog: usize,
        query: String,
        federated: bool,
    ) {
        let Ok(feed) = kobo_opds::parse(bytes, root) else {
            self.catalogs[catalog].search = SearchState::None;
            self.search_unavailable(context, federated);
            return;
        };
        let Some(link) = feed.search() else {
            self.catalogs[catalog].search = SearchState::None;
            self.search_unavailable(context, federated);
            return;
        };
        if let Some(way) = direct_search_way(link) {
            self.catalogs[catalog].search = SearchState::Known(way.clone());
            self.issue_search(context, catalog, &query, &way, federated);
            return;
        }
        let url = link.href.clone();
        let headers = vec![Header::new(
            "Accept",
            "application/opensearchdescription+xml",
        )];
        match context.spawn_retrying(Task::Fetch {
            url: url.clone(),
            offset: 0,
            max_bytes: FEED_BYTES,
            credential: None,
            headers,
        }) {
            Some(id) => {
                self.task = Some((
                    id,
                    Awaiting::DiscoverDescription {
                        catalog,
                        query,
                        federated,
                        url,
                    },
                ));
            }
            None => self.search_unavailable(context, federated),
        }
    }

    fn push_recent(&mut self, catalog: usize, term: &str) {
        let catalog = &mut self.catalogs[catalog];
        catalog.recent.retain(|held| held != term);
        catalog.recent.insert(0, term.to_owned());
        catalog.recent.truncate(MAX_RECENT);
    }

    /// The search text a `category-N` chip on the open book's page stands
    /// for, resolved against the open book's own categories rather than a
    /// stored string, so a chip tapped after the book behind it has changed
    /// cannot launch a search for a category that book never had.
    fn category_for(&self, action: ActionId) -> Option<String> {
        let publication = self.open.as_ref()?;
        publication
            .categories
            .iter()
            .enumerate()
            .find(|(index, _)| action == action_id(&format!("category-{index}")))
            .map(|(_, category)| {
                category
                    .label
                    .clone()
                    .unwrap_or_else(|| category.term.clone())
            })
    }

    /// Starts a search against every catalog at once: catalogs are queued
    /// and asked one at a time, and each answer is appended to the shelf as
    /// it lands rather than waiting for the rest -- four searches at once
    /// would spend a quarter of a megabyte before the first row appeared.
    fn begin_federated_search(&mut self, context: &mut Context, query: String) {
        self.stop_federating();
        let synthetic = Feed {
            title: Some(format!("\u{201c}{query}\u{201d} \u{00b7} every catalog")),
            ..Feed::default()
        };
        self.stack.push(StackEntry::fresh(
            synthetic,
            self.current_catalog().root.clone(),
        ));
        self.view = View::Shelf;
        self.federated_query = Some(query);
        self.federating = true;
        self.all_queue = (0..self.catalogs.len()).collect();
        self.advance_federated(context);
    }

    fn advance_federated(&mut self, context: &mut Context) {
        if self.task.is_some() {
            return;
        }
        let Some(catalog) = self.all_queue.pop_front() else {
            self.federating = false;
            self.show(context);
            return;
        };
        let Some(query) = self.federated_query.clone() else {
            self.federating = false;
            return;
        };
        self.begin_search(context, catalog, query, true);
    }

    /// Stops asking catalogs that have not been sent yet. A request already
    /// in flight is left to finish and still lands on the shelf; only the
    /// queue behind it is cleared, which is why the queue is held here
    /// rather than handed to the runtime all at once.
    fn stop_federating(&mut self) {
        self.all_queue.clear();
        self.federating = false;
        self.federated_query = None;
    }

    // ---------------------------------------------------------------
    // Shelf drawing
    // ---------------------------------------------------------------

    fn shelf_screen(&self, context: &Context) -> kobo_sdk::Screen {
        let title = self
            .stack
            .last()
            .and_then(|entry| entry.feed.title.clone())
            .unwrap_or_else(|| self.current_catalog().name.clone());
        let mut screen = ScreenBuilder::new("gutenbird-shelf")
            .top_bar(title)
            .top_bar_glyph("search", "Search", Glyph::Search)
            .top_bar_glyph("catalogs", "Catalogs", Glyph::Globe);
        if let Some(problem) = &self.problem {
            screen = screen.banner(BannerLevel::Attention, problem.clone());
        }
        if self.awaiting_feed() {
            return screen
                .divider()
                .activity("Fetching the catalog", None)
                .skeleton(SKELETON_ROWS)
                .build();
        }
        let Some(entry) = self.stack.last() else {
            if let Some(failure) = self.trouble {
                return screen.failure_state(failure, "catalogs").build();
            }
            return screen
                .text("Nothing here yet.")
                .primary_button("catalogs", "Choose a catalog")
                .build();
        };
        if entry.feed.publications.is_empty() && entry.feed.navigation.is_empty() {
            if let Some(failure) = self.trouble {
                return screen.failure_state(failure, "catalogs").build();
            }
            return screen.text("Nothing here.").build();
        }
        // One grid, whatever the catalog sent. A feed of navigation entries
        // is still a shelf of things to open, and drawing it as a list while
        // a second list of books sat underneath gave the page two paginations
        // and left the books past the first screenful unreachable.
        let shown_screen = if entry.feed.publications.is_empty() {
            Self::navigation_grid(entry, screen)
        } else {
            self.publication_grid(context, entry, screen)
        };
        Self::paginated(entry, shown_screen)
    }

    /// Draws a catalog's navigation as tiles, so a catalog that serves no
    /// shelf still looks like one.
    ///
    /// Project Gutenberg is the case this exists for: every book in it is a
    /// navigation entry pointing at that book's own document. The tile is
    /// drawn from what the entry already says, and its picture is filled in
    /// afterwards if following it turns up a cover -- so the page is complete
    /// from the first draw and only gets better, rather than rearranging
    /// itself under the reader.
    fn navigation_grid(entry: &StackEntry, screen: ScreenBuilder) -> ScreenBuilder {
        let first = entry.page * SHELF_PAGE;
        let tiles = entry
            .feed
            .navigation
            .iter()
            .enumerate()
            .skip(first)
            .take(SHELF_PAGE)
            .map(|(index, navigation)| {
                let subtitle = navigation.summary.clone().unwrap_or_default();
                let picture = entry.nav_covers.get(index).copied().flatten();
                (
                    format!("nav-{index}"),
                    navigation.title.clone(),
                    Glyph::Book,
                    move |tile: Tile| {
                        let tile = tile.with_subtitle(subtitle);
                        match picture {
                            Some(picture) => tile.with_picture(picture),
                            None => tile,
                        }
                    },
                )
            });
        screen.tile_grid(TileShape::Portrait, tiles)
    }

    fn publication_grid(
        &self,
        _context: &Context,
        entry: &StackEntry,
        screen: ScreenBuilder,
    ) -> ScreenBuilder {
        let first = entry.page * SHELF_PAGE;
        let tiles = entry
            .feed
            .publications
            .iter()
            .zip(entry.covers.iter())
            .enumerate()
            .skip(first)
            .take(SHELF_PAGE)
            .map(|(index, (publication, picture))| {
                let state = if self.is_kept(publication) {
                    TileState::Held
                } else {
                    TileState::Normal
                };
                let author = publication.authors.first().cloned().unwrap_or_default();
                let picture = *picture;
                let source = entry.sources.get(index).cloned();
                (
                    format!("book-{index}"),
                    publication.title.clone(),
                    Glyph::Book,
                    move |tile: Tile| {
                        let subtitle = match source {
                            Some(source) if !author.is_empty() => {
                                format!("{author} \u{00b7} {source}")
                            }
                            Some(source) => source,
                            None => author,
                        };
                        let tile = tile.with_state(state).with_subtitle(subtitle);
                        match picture {
                            Some(picture) => tile.with_picture(picture),
                            None => tile,
                        }
                    },
                )
            });
        screen.tile_grid(TileShape::Portrait, tiles)
    }

    fn paginated(entry: &StackEntry, screen: ScreenBuilder) -> kobo_sdk::Screen {
        let pages = shelf_pages(entry);
        let more = entry.next.is_some() && entry.feed.publications.len() < MAX_PUBLICATIONS;
        if pages <= 1 && !more {
            return screen.build();
        }
        let page = u16::try_from(entry.page + 1).unwrap_or(u16::MAX);
        let total = u16::try_from(pages).unwrap_or(u16::MAX);
        screen
            .page_turns("shelf-back", "shelf-next")
            .page_position(page, total)
            .build()
    }

    fn is_kept(&self, publication: &Publication) -> bool {
        book_keys(publication).is_some_and(|(blob, _)| self.stored.contains_key(&blob))
    }

    // ---------------------------------------------------------------
    // Search screen
    // ---------------------------------------------------------------

    fn search_screen(&self) -> kobo_sdk::Screen {
        let catalog = self.current_catalog();
        let mut screen = ScreenBuilder::new("gutenbird-search")
            .top_bar("Search")
            .field("query", self.keyboard.text(), "An author or a title")
            .field_clear("query-clear");
        if let Some(problem) = &self.problem {
            screen = screen.banner(BannerLevel::Attention, problem.clone());
        }
        if !catalog.recent.is_empty() {
            screen = screen.section("Recent searches").chips(
                catalog
                    .recent
                    .iter()
                    .enumerate()
                    .map(|(index, term)| (format!("recent-{index}"), term.clone(), false)),
            );
        }
        screen = screen.chips([("search-all", "Search every catalog", self.search_all)]);
        match &catalog.search {
            SearchState::None => screen.secondary("This catalog has no search.").build(),
            SearchState::Unknown | SearchState::Known(_) => {
                screen.keyboard(&self.keyboard, "Search").build()
            }
        }
    }

    fn note_screen(&self) -> kobo_sdk::Screen {
        ScreenBuilder::new("gutenbird-note")
            .top_bar("Marginal note")
            .secondary(
                "Write beside the highlighted words. The highlight remains if the note is blank.",
            )
            .field("annotation-note", self.keyboard.text(), "Your note")
            .field_clear("annotation-note-clear")
            .keyboard(&self.keyboard, "Save note")
            .build()
    }

    // ---------------------------------------------------------------
    // Details
    // ---------------------------------------------------------------

    fn details_screen(&self, context: &Context) -> kobo_sdk::Screen {
        let Some(publication) = &self.open else {
            return self.shelf_screen(context);
        };
        let bare = ScreenBuilder::new("gutenbird-book")
            .top_bar(publication.title.clone())
            .hero(
                self.open_cover,
                DETAILS_COVER_MM,
                publication.title.clone(),
                (!publication.authors.is_empty()).then(|| publication.authors.join(", ")),
                Vec::<(String, String)>::new(),
            );
        let bare = match &self.problem {
            Some(problem) => bare.banner(BannerLevel::Attention, problem.clone()),
            None => bare,
        };
        if let Some(reason) = &self.failed {
            let bare = bare
                .transfer("Download stopped", u64::from(self.fetched), None)
                .transfer_failed(reason.clone(), self.retryable);
            return if self.retryable {
                bare.transfer_retry("read", "Try again").build()
            } else {
                bare.build()
            };
        }
        if self.awaiting_book() || self.loading.is_some() {
            let total = self.download.as_ref().and_then(|download| download.total);
            return bare
                .transfer("Downloading", u64::from(self.fetched), total)
                .build();
        }
        let blocks = Self::detail_blocks(publication);
        let pages = self.detail_pagination(context, publication, &blocks);
        let page = self.detail_page.min(pages.len().saturating_sub(1));
        let showing = pages.get(page).map_or(&[][..], Vec::as_slice);
        let mut screen = self.detail_head(publication, page == 0);
        for block in showing {
            screen = block.add(screen);
        }
        if pages.len() <= 1 {
            return screen.build();
        }
        screen
            .page_turns("about-back", "about-next")
            .page_position(
                u16::try_from(page + 1).unwrap_or(u16::MAX),
                u16::try_from(pages.len()).unwrap_or(u16::MAX),
            )
            .build()
    }

    fn detail_blocks(publication: &Publication) -> Vec<DetailBlock> {
        let mut blocks = Vec::new();
        if let Some(summary) = &publication.summary {
            blocks.push(DetailBlock::Section("About"));
            blocks.push(DetailBlock::Text(summary.clone()));
        }
        if !publication.categories.is_empty() {
            blocks.push(DetailBlock::Section("Categories"));
            blocks.push(DetailBlock::Categories(publication.categories.clone()));
        }
        if let Some(rights) = &publication.rights {
            blocks.push(DetailBlock::Section("Rights"));
            blocks.push(DetailBlock::Text(rights.clone()));
        }
        let facts = detail_facts(publication);
        if !facts.is_empty() {
            blocks.push(DetailBlock::Section("Details"));
            blocks.push(DetailBlock::Facts(facts));
        }
        blocks
    }

    fn detail_pagination(
        &self,
        context: &Context,
        publication: &Publication,
        blocks: &[DetailBlock],
    ) -> Vec<Vec<DetailBlock>> {
        let mut pages: Vec<Vec<DetailBlock>> = Vec::new();
        let mut current: Vec<DetailBlock> = Vec::new();
        let mut queue: VecDeque<DetailBlock> = blocks.iter().cloned().collect();
        while let Some(block) = queue.pop_front() {
            let mut candidate = current.clone();
            candidate.push(block.clone());
            if self.detail_fits(context, publication, pages.is_empty(), &candidate) {
                current = candidate;
                continue;
            }
            if let DetailBlock::Text(text) = &block {
                if let Some((head, tail)) =
                    self.split_summary(context, publication, pages.is_empty(), &current, text)
                {
                    current.push(DetailBlock::Text(head));
                    pages.push(std::mem::take(&mut current));
                    queue.push_front(DetailBlock::Text(tail));
                    continue;
                }
            }
            // A list of categories is cut between two of them, on the same
            // reasoning as a summary. Pride and Prejudice carries eleven, and
            // as one block they were placed whole whatever the room: on the
            // panel the last of them was drawn through the "3 of 4" beneath it.
            if let DetailBlock::Categories(categories) = &block {
                if let Some((head, tail)) = self.split_categories(
                    context,
                    publication,
                    pages.is_empty(),
                    &current,
                    categories,
                ) {
                    current.push(DetailBlock::Categories(head));
                    pages.push(std::mem::take(&mut current));
                    queue.push_front(DetailBlock::Categories(tail));
                    continue;
                }
            }
            if current.is_empty() {
                current = candidate;
                continue;
            }
            pages.push(std::mem::take(&mut current));
            queue.push_front(block);
        }
        pages.push(current);
        for index in 0..pages.len().saturating_sub(1) {
            if pages[index].len() > 1
                && matches!(pages[index].last(), Some(DetailBlock::Section(_)))
            {
                let orphan = pages[index].pop().expect("just matched");
                pages[index + 1].insert(0, orphan);
            }
        }
        pages.retain(|page| !page.is_empty());
        if pages.is_empty() {
            pages.push(Vec::new());
        }
        pages
    }

    fn split_summary(
        &self,
        context: &Context,
        publication: &Publication,
        first_page: bool,
        current: &[DetailBlock],
        text: &str,
    ) -> Option<(String, String)> {
        let words: Vec<&str> = text.split_whitespace().collect();
        if words.len() < 2 {
            return None;
        }
        let fits = |count: usize| {
            let mut blocks = current.to_vec();
            blocks.push(DetailBlock::Text(words[..count].join(" ")));
            self.detail_fits(context, publication, first_page, &blocks)
        };
        if !fits(1) {
            return None;
        }
        let (mut low, mut high) = (1usize, words.len() - 1);
        while low < high {
            let middle = low + (high - low).div_ceil(2);
            if fits(middle) {
                low = middle;
            } else {
                high = middle - 1;
            }
        }
        Some((words[..low].join(" "), words[low..].join(" ")))
    }

    /// How many categories fit here, and the ones that do not.
    ///
    /// The same binary search [`Self::split_summary`] runs over words, run
    /// over rows instead. `None` when there is nothing to cut -- one category,
    /// or a page with no room for even one -- and the caller then does what it
    /// does for any block that will not fit: starts a page with it.
    fn split_categories(
        &self,
        context: &Context,
        publication: &Publication,
        first_page: bool,
        current: &[DetailBlock],
        categories: &[Category],
    ) -> Option<(Vec<Category>, Vec<Category>)> {
        if categories.len() < 2 {
            return None;
        }
        let fits = |count: usize| {
            let mut blocks = current.to_vec();
            blocks.push(DetailBlock::Categories(categories[..count].to_vec()));
            self.detail_fits(context, publication, first_page, &blocks)
        };
        if !fits(1) {
            return None;
        }
        let (mut low, mut high) = (1usize, categories.len() - 1);
        while low < high {
            let middle = low + (high - low).div_ceil(2);
            if fits(middle) {
                low = middle;
            } else {
                high = middle - 1;
            }
        }
        Some((categories[..low].to_vec(), categories[low..].to_vec()))
    }

    fn detail_fits(
        &self,
        context: &Context,
        publication: &Publication,
        first_page: bool,
        blocks: &[DetailBlock],
    ) -> bool {
        let mut screen = self.detail_head(publication, first_page);
        for block in blocks {
            screen = block.add(screen);
        }
        screen
            .page_turns("about-back", "about-next")
            .page_position(1, 2)
            .build()
            .diagnostics(&context.metrics(), &Chrome::measuring(true))
            .issues
            .iter()
            .all(|issue| issue.severity != DiagnosticSeverity::Error)
    }

    fn detail_head(&self, publication: &Publication, first_page: bool) -> ScreenBuilder {
        let screen = ScreenBuilder::new("gutenbird-book").top_bar(publication.title.clone());
        if !first_page {
            return screen;
        }
        let subtitle = (!publication.authors.is_empty()).then(|| publication.authors.join(", "));
        let screen = screen.hero(
            self.open_cover,
            DETAILS_COVER_MM,
            publication.title.clone(),
            subtitle,
            Vec::<(String, String)>::new(),
        );
        let screen = match &self.problem {
            Some(problem) => screen.banner(BannerLevel::Attention, problem.clone()),
            None => screen,
        };
        let screen = if self.is_kept(publication) {
            screen.secondary("Already on this device.")
        } else {
            screen
        };
        match read_offer(publication) {
            ReadOffer::Read => screen.primary_button("read", "Read"),
            ReadOffer::Sample => screen.primary_button("read", "Read sample"),
            ReadOffer::Unavailable(price) => {
                let label = match price {
                    Some(price) => format!(
                        "Not available here \u{2014} {}{}",
                        price.value,
                        price.currency.map(|c| format!(" {c}")).unwrap_or_default()
                    ),
                    None => "Not available here.".to_owned(),
                };
                screen.secondary(label)
            }
            ReadOffer::Nothing => screen.secondary("Nothing here can be read on this device."),
        }
    }

    // ---------------------------------------------------------------
    // Reading
    // ---------------------------------------------------------------

    fn reading_screen(&self, context: &Context) -> kobo_sdk::Screen {
        let title = self.open.as_ref().map_or_else(
            || "Reading".to_owned(),
            |publication| publication.title.clone(),
        );
        let Some(reader) = &self.reader else {
            return self.details_screen(context);
        };
        reader.screen(&title)
    }

    // ---------------------------------------------------------------
    // Covers -- the shelf's publications
    // ---------------------------------------------------------------

    /// Follows the navigation entries on this page to find the books among
    /// them.
    ///
    /// One at a time, because the cover lanes are for covers and a page of
    /// six books would otherwise take every task the runtime allows. Only the
    /// page being looked at: a reader who turns pages quickly must not leave
    /// a trail of requests behind them.
    fn hydrate_visible(&mut self, context: &mut Context) {
        // Never on a screen that is not the shelf: a book downloading wants
        // every lane there is, and a picture for a tile nobody is looking at
        // is not worth taking one.
        if !matches!(self.view, View::Shelf) {
            return;
        }
        while self.filling.len() < FILL_LANES {
            let Some(entry) = self.stack.last() else {
                return;
            };
            let first = entry.page * SHELF_PAGE;
            let Some(href) = entry
                .feed
                .navigation
                .iter()
                .skip(first)
                .take(SHELF_PAGE)
                .map(|navigation| navigation.href.clone())
                .find(|href| !entry.examined.contains(href))
            else {
                return;
            };
            // Marked before the answer arrives, so a row that fails to load
            // is not asked for again on every redraw, and so that the lane
            // beside this one picks a different row.
            if let Some(entry) = self.stack.last_mut() {
                entry.examined.insert(href.clone());
            }
            let headers = vec![Header::new("Accept", kobo_opds::ACCEPT)];
            let Some(task) = context.spawn_retrying(Task::Fetch {
                url: href.clone(),
                offset: 0,
                max_bytes: FEED_BYTES,
                credential: None,
                headers,
            }) else {
                return;
            };
            context.log(LogLevel::Debug, format!("fill {task:?} <- {href}"));
            self.filling.push((task, FillStage::Entry { href }));
        }
    }

    /// Takes one of the shelf's own answers, if this task was one.
    fn finish_filling(&mut self, task: TaskId) -> Option<FillStage> {
        let at = self.filling.iter().position(|(id, _)| *id == task)?;
        Some(self.filling.remove(at).1)
    }

    /// Takes a followed row, and moves it to the shelf if it was a book.
    ///
    /// A row that turns out to be another feed is left exactly where it was:
    /// "Authors" and "Subjects" sit among the books in a Gutenberg search
    /// answer, and there is nothing in the address to tell them apart, which
    /// is why this looks rather than guesses.
    fn took_hydration(&mut self, context: &mut Context, bytes: &[u8], href: &str) {
        let cover = kobo_opds::parse(bytes, href)
            .ok()
            .as_ref()
            .and_then(Self::resolve_entry)
            .as_ref()
            .and_then(Publication::cover)
            .map(|image| image.href.clone());
        if let Some(kobo_opds::ImageSource::Url(url)) = cover {
            self.ask_nav_cover(context, href.to_owned(), url);
        } else {
            self.hydrate_visible(context);
        }
    }

    /// Asks for the picture a followed navigation entry named.
    /// Drops whatever the shelf was fetching for itself.
    ///
    /// Called before anything the reader asked for. A tile without its
    /// picture is a tile; a tap that does nothing is a broken application.
    fn abandon_hydration(&mut self, context: &mut Context) {
        for (task, _) in std::mem::take(&mut self.filling) {
            context.cancel(task);
        }
    }

    fn ask_nav_cover(&mut self, context: &mut Context, href: String, url: String) {
        let Some(task) = context.spawn_retrying(Task::Fetch {
            url,
            offset: 0,
            max_bytes: COVER_BYTES,
            credential: None,
            headers: Vec::new(),
        }) else {
            self.hydrate_visible(context);
            return;
        };
        context.log(LogLevel::Debug, format!("cover {task:?} <- {href}"));
        self.filling.push((task, FillStage::Picture { href }));
    }

    /// Puts a picture into the tile that asked for it.
    ///
    /// Found by address rather than by position, because the page may have
    /// turned while the picture was in the air and the tile at that index is
    /// then a different book.
    fn took_nav_cover(&mut self, context: &mut Context, bytes: &[u8], href: &str) {
        let (cell_width, cell_height) = context.metrics().tile_body(TileShape::Portrait);
        if let (Ok(width), Ok(height)) = (u32::try_from(cell_width), u32::try_from(cell_height)) {
            if let Ok(picture) = kobo_image::decode(bytes) {
                if let Ok(mut picture) = picture.fit_enlarging(width, height) {
                    picture.dither(kobo_image::PANEL_GREYS);
                    let handle = PictureHandle(NAV_COVER_HANDLE_BASE + self.nav_cover_handle);
                    self.nav_cover_handle =
                        self.nav_cover_handle.wrapping_add(1) % NAV_COVER_HANDLES;
                    let (drawn_width, drawn_height) = (picture.width(), picture.height());
                    if let Some(reference) =
                        context.put_picture(handle, drawn_width, drawn_height, picture.into_grey())
                    {
                        if let Some(entry) = self.stack.last_mut() {
                            if let Some(at) = entry
                                .feed
                                .navigation
                                .iter()
                                .position(|navigation| navigation.href == href)
                            {
                                if let Some(slot) = entry.nav_covers.get_mut(at) {
                                    *slot = Some(reference);
                                }
                            }
                        }
                    }
                }
            }
        }
        self.show(context);
        self.hydrate_visible(context);
    }

    fn want_covers(&mut self, context: &mut Context) {
        self.wanted.clear();
        let Some(entry) = self.stack.last() else {
            return;
        };
        let first = entry.page * SHELF_PAGE;
        let (cell_width, cell_height) = context.metrics().tile_body(TileShape::Portrait);
        let Ok(cell_width) = u32::try_from(cell_width) else {
            return;
        };
        let Ok(cell_height) = u32::try_from(cell_height) else {
            return;
        };
        let shown: Vec<usize> =
            (first..(first + SHELF_PAGE).min(entry.feed.publications.len())).collect();

        // A publication with no image at all is set from its title now,
        // rather than left as a hole nothing will ever fill.
        let coverless: Vec<usize> = shown
            .iter()
            .copied()
            .filter(|&index| {
                entry.covers[index].is_none() && entry.feed.publications[index].cover().is_none()
            })
            .collect();
        for index in coverless {
            self.set_a_cover(context, index, cell_width, cell_height);
        }

        self.looking.clear();
        let mut inline_queue: Vec<(usize, String, Vec<u8>)> = Vec::new();
        for index in shown {
            if self
                .stack
                .last()
                .is_none_or(|entry| entry.covers[index].is_some())
            {
                continue;
            }
            let Some(image) = self.stack.last().unwrap().feed.publications[index]
                .cover()
                .cloned()
            else {
                continue;
            };
            match image.href {
                ImageSource::Inline { bytes, .. } => {
                    inline_queue.push((index, String::new(), bytes));
                }
                ImageSource::Url(url) => self.looking.push((cover_key(&url), index)),
            }
        }
        for (index, _, bytes) in inline_queue {
            self.took_cover(context, index, &bytes);
        }
        let keys: Vec<String> = self.looking.iter().map(|(key, _)| key.clone()).collect();
        for key in keys {
            context.store().load(key);
        }
        if self.looking.is_empty() {
            self.ask_cover(context);
        }
    }

    fn looked_for_cover(&mut self, context: &mut Context, key: &str, found: Option<Vec<u8>>) {
        let Some(at) = self.looking.iter().position(|(held, _)| held == key) else {
            return;
        };
        let (_, index) = self.looking.remove(at);
        let took = found.is_some_and(|bytes| self.took_cover(context, index, &bytes));
        if !took {
            context.store().forget(key.to_owned());
            self.wanted.push((index, 0));
        }
        if !self.looking.is_empty() {
            return;
        }
        self.ask_cover(context);
        if self.covers.is_empty() {
            self.show(context);
        }
    }

    fn ask_cover(&mut self, context: &mut Context) {
        while self.covers.len() < COVER_LANES {
            let Some((index, tries)) = self.wanted.pop() else {
                return;
            };
            let Some(image) = self
                .stack
                .last()
                .and_then(|entry| entry.feed.publications.get(index))
                .and_then(Publication::cover)
            else {
                continue;
            };
            let ImageSource::Url(url) = image.href.clone() else {
                continue;
            };
            if let Some(task) = context.spawn_retrying(Task::Fetch {
                url,
                offset: 0,
                max_bytes: COVER_BYTES,
                credential: None,
                headers: Vec::new(),
            }) {
                self.covers.push((task, index, tries));
                continue;
            }
            self.wanted.push((index, tries));
            return;
        }
    }

    fn finish_cover(&mut self, task: TaskId) -> Option<(usize, u8)> {
        let at = self.covers.iter().position(|(id, _, _)| *id == task)?;
        let (_, index, tries) = self.covers.remove(at);
        Some((index, tries))
    }

    fn retry_cover(&mut self, index: usize, tries: u8) {
        if tries + 1 < COVER_TRIES {
            self.wanted.insert(0, (index, tries + 1));
        }
    }

    /// Decodes cover bytes and hands them to the runtime, refusing anything
    /// too small to be real art -- Gutenberg's own 22x22 category icons,
    /// specifically, which travel as `data:` thumbnails on navigation
    /// entries and must never reach a publication's own cover slot enlarged
    /// into a blur.
    fn took_cover(&mut self, context: &mut Context, index: usize, bytes: &[u8]) -> bool {
        let (cell_width, cell_height) = context.metrics().tile_body(TileShape::Portrait);
        let Ok(cell_width) = u32::try_from(cell_width) else {
            return false;
        };
        let Ok(cell_height) = u32::try_from(cell_height) else {
            return false;
        };
        let Ok(picture) = kobo_image::decode(bytes) else {
            self.set_a_cover(context, index, cell_width, cell_height);
            return false;
        };
        if picture.width() < MIN_COVER_PX || picture.height() < MIN_COVER_PX {
            return false;
        }
        let Ok(mut picture) = picture.fit_enlarging(cell_width, cell_height) else {
            return false;
        };
        picture.dither(kobo_image::PANEL_GREYS);
        let handle = PictureHandle(u32::try_from(index).unwrap_or(0));
        let (width, height) = (picture.width(), picture.height());
        let Some(reference) = context.put_picture(handle, width, height, picture.into_grey())
        else {
            return false;
        };
        if let Some(entry) = self.stack.last_mut() {
            if let Some(slot) = entry.covers.get_mut(index) {
                *slot = Some(reference);
            }
        }
        true
    }

    fn set_a_cover(
        &mut self,
        context: &mut Context,
        index: usize,
        width: u32,
        height: u32,
    ) -> bool {
        let Some(publication) = self
            .stack
            .last()
            .and_then(|entry| entry.feed.publications.get(index))
        else {
            return false;
        };
        let author = publication.authors.first().map(String::as_str);
        let grey = kobo_sdk::typographic_cover(&publication.title, author, width, height);
        if grey.is_empty() {
            return false;
        }
        let handle = PictureHandle(u32::try_from(index).unwrap_or(0));
        let Some(reference) = context.put_picture(handle, width, height, grey) else {
            return false;
        };
        if let Some(entry) = self.stack.last_mut() {
            if let Some(slot) = entry.covers.get_mut(index) {
                *slot = Some(reference);
            }
        }
        true
    }

    fn keep_cover(&mut self, context: &mut Context, index: usize, bytes: &[u8]) {
        if !self.took_cover(context, index, bytes) {
            return;
        }
        if bytes.len() > MAX_STORE_VALUE {
            return;
        }
        let Some(url) = self
            .stack
            .last()
            .and_then(|entry| entry.feed.publications.get(index))
            .and_then(Publication::cover)
            .and_then(|image| match &image.href {
                ImageSource::Url(url) => Some(url.clone()),
                ImageSource::Inline { .. } => None,
            })
        else {
            return;
        };
        context.store().save(cover_key(&url), bytes.to_vec());
    }

    fn next_cover(&mut self, context: &mut Context) {
        self.ask_cover(context);
        if self.covers.is_empty() {
            self.show(context);
        }
    }

    // ---------------------------------------------------------------
    // The open book's own cover
    // ---------------------------------------------------------------

    fn ask_open_cover(&mut self, context: &mut Context) {
        let Some(image) = self.open.as_ref().and_then(Publication::cover).cloned() else {
            // Nothing is coming, so nothing is reserved. An empty frame that
            // will never be filled is not a placeholder, it is a hole.
            return;
        };
        // Claimed now, drawn as an outlined box until the bytes land. A
        // publication that says it has a cover is going to have one, and the
        // room it takes is decided here rather than by what turns up.
        self.open_cover = Some(TilePicture::new(
            OPEN_COVER_HANDLE,
            OPEN_COVER_PX.0,
            OPEN_COVER_PX.1,
        ));
        match image.href {
            ImageSource::Inline { bytes, .. } => {
                if !self.took_open_cover(context, &bytes) {
                    self.letter_open_cover(context);
                }
            }
            ImageSource::Url(url) => {
                if let Some(task) = context.spawn_retrying(Task::Fetch {
                    url,
                    offset: 0,
                    max_bytes: COVER_BYTES,
                    credential: None,
                    headers: Vec::new(),
                }) {
                    self.open_cover_task = Some((task, 0));
                }
            }
        }
    }

    fn took_open_cover(&mut self, context: &mut Context, bytes: &[u8]) -> bool {
        let (width, height) = OPEN_COVER_PX;
        let Ok(picture) = kobo_image::decode(bytes) else {
            return false;
        };
        if picture.width() < MIN_COVER_PX || picture.height() < MIN_COVER_PX {
            return false;
        }
        let Ok(mut picture) = picture.fit_enlarging(width, height) else {
            return false;
        };
        picture.dither(kobo_image::PANEL_GREYS);
        // Padded out to the reserved box rather than handed over at whatever
        // size it came back. A cover an eighth taller than three by two would
        // otherwise be an eighth taller on the page than the frame that was
        // standing in for it, and the block underneath would move by that much
        // at the moment it arrived -- which is the whole thing being fixed.
        let padded = on_paper(&picture, width, height);
        let Some(reference) = context.put_picture(OPEN_COVER_HANDLE, width, height, padded) else {
            return false;
        };
        self.open_cover = Some(reference);
        true
    }

    /// Sets the book's own title in the room its cover was going to take.
    ///
    /// For a cover that was promised and did not arrive, or arrived and would
    /// not decode. Emptying the frame instead would move everything under it
    /// back up the panel, which is the reflow this reservation exists to stop,
    /// and leaving it empty says only that something is missing. The shelf
    /// already letters a tile whose cover will not decode; this is the same
    /// answer one page in.
    fn letter_open_cover(&mut self, context: &mut Context) {
        // Nothing was reserved, so nothing was promised: this book has no
        // cover at all and its metadata already has the whole width.
        if self.open_cover.is_none() {
            return;
        }
        let Some(publication) = self.open.as_ref() else {
            return;
        };
        let (width, height) = OPEN_COVER_PX;
        let grey = kobo_sdk::typographic_cover(
            &publication.title,
            publication.authors.first().map(String::as_str),
            width,
            height,
        );
        if grey.is_empty() {
            return;
        }
        if let Some(reference) = context.put_picture(OPEN_COVER_HANDLE, width, height, grey) {
            self.open_cover = Some(reference);
        }
    }

    // ---------------------------------------------------------------
    // Getting the book
    // ---------------------------------------------------------------

    fn get_book(&mut self, context: &mut Context) {
        let kept = self
            .open
            .as_ref()
            .is_some_and(|publication| self.is_kept(publication));
        if kept {
            if let Some((blob, _)) = self.open_keys() {
                let mut download = ShelfDownload::new(blob);
                download.start(context);
                self.loading = Some(download);
                self.problem = None;
                self.trouble = None;
                self.failed = None;
                return;
            }
        }
        self.ask_book(context);
    }

    fn ask_book(&mut self, context: &mut Context) {
        let Some(acquisition) = self
            .open
            .as_ref()
            .and_then(Publication::best_acquisition)
            .cloned()
        else {
            self.problem = Some("Nothing here can be read on this device.".to_owned());
            return;
        };
        let kind = download_kind(acquisition.media_type.as_deref());
        if self
            .download
            .as_ref()
            .is_none_or(|download| download.url != acquisition.href)
        {
            self.download = Some(Download {
                url: acquisition.href.clone(),
                kind,
                bytes: Vec::new(),
                total: acquisition.length,
            });
            self.fetched = 0;
            self.complete = false;
        }
        self.problem = None;
        self.trouble = None;
        self.failed = None;
        let headers = vec![Header::new("Accept", kobo_opds::ACCEPT)];
        match context.spawn_retrying(Task::Fetch {
            url: acquisition.href,
            offset: self.fetched,
            max_bytes: CHUNK_BYTES,
            credential: None,
            headers,
        }) {
            Some(task) => {
                context.log(LogLevel::Info, format!("book {task:?} at {}", self.fetched));
                self.task = Some((task, Awaiting::Book));
            }
            None => self.problem = Some("Too much is already in flight.".to_owned()),
        }
    }

    fn top_up(&mut self, context: &mut Context) {
        if self.complete || self.task.is_some() {
            return;
        }
        // An EPUB is fetched straight through rather than a few pages ahead
        // of the reader, because there is no reader yet: a zip cannot be
        // opened until its last byte lands, so there is no reading position
        // to stay ahead of. Waiting for one meant the first chunk arrived,
        // nothing asked for the second, and a five-hundred-kilobyte book sat
        // at forty-five per cent forever.
        if self.download.as_ref().map(|download| download.kind) == Some(DownloadKind::Epub) {
            self.ask_book(context);
            return;
        }
        if self.download.as_ref().map(|download| download.kind) != Some(DownloadKind::Text) {
            return;
        }
        let left = self.reader.as_ref().map_or(0, |reader| {
            reader.page_count().saturating_sub(reader.page_number())
        });
        if left <= TOP_UP_PAGES {
            self.ask_book(context);
            if self.awaiting_book() {
                if let Some(reader) = &mut self.reader {
                    reader.expect_more(true);
                }
            }
        }
    }

    fn open_keys(&self) -> Option<(String, String)> {
        self.open.as_ref().and_then(book_keys)
    }

    fn read_action(&mut self, context: &mut Context, action: ActionId) -> bool {
        let metrics = context.metrics();
        let Some(reader) = &mut self.reader else {
            return false;
        };
        match reader.act_on(action, &metrics) {
            Outcome::Elsewhere => return false,
            Outcome::Close => self.back_to(View::Details),
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
        // A page turn is also the moment a plate on the page turned to is
        // wanted. Nothing to do while the queue is already going round; this
        // is here for the case where the runtime had no room for the sleep
        // that would have carried it, and the plates would otherwise stop
        // arriving with no way to start again.
        if self.plating.is_none() && (!self.plates.is_empty() || self.dithering.is_some()) {
            self.decode_more_plates(context);
        }
        self.show(context);
        true
    }

    fn save_place(&mut self, context: &mut Context) {
        let Some((_, place)) = self.open_keys() else {
            return;
        };
        let Some(reader) = &self.reader else {
            return;
        };
        let memory = reader.memory().encode();
        context.store().save(place, memory);
    }

    fn keep_book(&mut self, context: &mut Context) {
        let Some((blob, _)) = self.open_keys() else {
            return;
        };
        let Some(download) = &self.download else {
            return;
        };
        if self.stored.contains_key(&blob) || download.bytes.is_empty() {
            return;
        }
        // Moved rather than copied. Cloning meant a twenty-four megabyte
        // book was two of them at once, at the exact moment the parse was
        // about to want a third, which is how this took the device down. The
        // reader is opened from the shelf copy afterwards, so nothing here
        // needs the bytes again.
        let Some(download) = self.download.take() else {
            return;
        };
        let mut upload = ShelfUpload::new(blob, download.bytes);
        upload.start(context);
        self.keeping = Some(upload);
    }

    /// Rebuilds the open book from everything downloaded so far, throwing a
    /// download away rather than keeping it forever if it will not parse --
    /// the one way an EPUB fetch can fail after every byte checked out as
    /// real.
    fn reopen(&mut self, context: &mut Context) {
        let Some(download) = &self.download else {
            return;
        };
        // A zip keeps its central directory at the end, so an EPUB is not a
        // book until its last byte has arrived: asking `kobo-doc` to read a
        // partial one does not return a partial book, it returns an error.
        // Text is the opposite and is rebuilt on every chunk, which is what
        // lets a reader start on page one while the rest is still coming.
        // Reading an EPUB early was worse than useless -- Gutenberg's
        // illustrated Pride and Prejudice is twenty-five megabytes, so the
        // first of a hundred chunks failed to parse and the screen said the
        // book could not be read while it was still arriving perfectly.
        if download.kind == DownloadKind::Epub && !self.complete {
            return;
        }
        let memory = self.reader.as_ref().map_or_else(
            || self.place.clone().unwrap_or_default(),
            |reader| reader.memory().clone(),
        );
        // Opening a book is the one thing this application does that has ever
        // blocked the panel for seconds at a time, and which of the three
        // stages costs that is not guessable from a screenshot: on a
        // development machine all three together are under thirty
        // milliseconds. So each is timed and the timings go to the log, which
        // means the black box, which means they survive the hang.
        let started = std::time::Instant::now();
        let (name, result) = match download.kind {
            DownloadKind::Text => {
                let cleaned = readable(&String::from_utf8_lossy(&download.bytes));
                ("book.txt", kobo_doc::read("book.txt", cleaned.as_bytes()))
            }
            DownloadKind::Epub => ("book.epub", kobo_doc::read("book.epub", &download.bytes)),
        };
        let read_ms = started.elapsed().as_millis();
        let downloaded = download.bytes.len();
        let _ = name;
        if let Ok(mut document) = result {
            let blocks = document.blocks.len();
            let started = std::time::Instant::now();
            // Whatever the last reopen handed over is about to be replaced, so
            // it goes back first. A book read from the shelf reaches here a
            // second time, and without this the first set stayed decoded in
            // the runtime with nothing left that could name it.
            for handle in self.book_pictures.drain(..) {
                context.drop_picture(handle);
            }
            if let Some(handle) = self.book_font.take() {
                context.drop_font(handle);
            }
            // Taken off the document rather than left on it: the reader owns
            // the document from here, and holding the scanned bytes as well as
            // the decoded greyscale meant every plate was paid for twice.
            let images = std::mem::take(&mut document.images);
            let pictures_ms = started.elapsed().as_millis();
            let started = std::time::Instant::now();
            let mut reader = Reader::open(document, memory, &context.metrics());
            if let Some((name, bytes)) = reader
                .preferred_publisher_font()
                .map(|(name, bytes)| (name.to_owned(), bytes.to_vec()))
            {
                if let Some(handle) = context.put_font(PUBLISHER_FONT_HANDLE, name, bytes) {
                    reader.set_publisher_font(Some(handle), &context.metrics());
                    self.book_font = Some(handle);
                }
            }
            // Before the count is read, because the sizes are what an
            // illustrated book is measured at, and it is that count -- the one
            // it will actually be read at -- that the timing line is worth
            // having.
            let reserved = self.reserve_plates(&reader, &images, &context.metrics());
            reader.set_pictures(reserved, &context.metrics());
            let paginate_ms = started.elapsed().as_millis();
            context.log(
                LogLevel::Info,
                format!(
                    "opened {} bytes in {read_ms} ms read, {pictures_ms} ms pictures, \
                     {paginate_ms} ms paginate ({blocks} blocks, {} pages, {} plates queued)",
                    downloaded,
                    reader.page_count(),
                    self.plates.len()
                ),
            );
            self.reader = Some(reader);
            // Handed to the runtime rather than started here. This callback
            // has already spent its parse and its pagination; a plate on top
            // of that is what put it over the deadline.
            if !self.plates.is_empty() {
                self.plating = context.spawn(Task::Sleep { seconds: 0 });
            }
        } else {
            self.problem = Some("This book could not be read.".to_owned());
            if self.complete {
                self.discard_broken_book(context);
            }
        }
    }

    /// Gives back everything the open book was costing.
    ///
    /// Leaving the reader used to release nothing: the `Reader` stayed, and
    /// with it the whole parsed document; the downloaded bytes stayed; and
    /// every plate stayed decoded inside the runtime. All of it was only
    /// dropped when some *other* book was opened, so an owner who read one
    /// illustrated book and went back to browsing carried it for the rest of
    /// the session. Peter Rabbit is nine megabytes held that way, Alice with
    /// the Tenniel plates fourteen, on a device with four hundred and forty.
    ///
    /// Nothing here needs the bytes again. A book already on the device is
    /// re-read from the shelf when its Read control is pressed, which is the
    /// same path that opened it the first time.
    fn close_book(&mut self, context: &mut Context) {
        // A book still arriving is a book still being appended to, and the
        // chunk that lands next expects to find what came before it.
        if self.download.is_some() && !self.complete {
            return;
        }
        for handle in self.book_pictures.drain(..) {
            context.drop_picture(handle);
        }
        if let Some(handle) = self.book_font.take() {
            context.drop_font(handle);
        }
        // Whatever is still queued is source bytes for a book nobody is
        // reading any more. The sleep already in flight is left to land and be
        // ignored: cancelling it would cost a message to save nothing.
        self.plates.clear();
        self.dithering = None;
        self.reader = None;
        self.download = None;
        self.fetched = 0;
        self.complete = false;
    }

    /// Claims the room every plate in the book will take, decoding none of it.
    ///
    /// Opening a book used to decode, fit and dither every image the container
    /// held before it returned, inside a lifecycle callback with a two hundred
    /// and fifty millisecond deadline. The Tale of Peter Rabbit is twenty-eight
    /// plates, and on a Clara BW that was two thousand seven hundred and
    /// eighty-four milliseconds: parsing the book took ten and paginating it
    /// thirty-two, so the images were ninety-nine per cent of a stall during
    /// which nothing drew, no control answered, and three watchdogs counted.
    ///
    /// A picture's header states its size in its first few dozen bytes, and
    /// size is the only thing pagination needs. Reading the headers costs
    /// microseconds and gives the reader the same page count it would have got
    /// from the pixels, so the book opens measured and correct and the plates
    /// are drawn into the room already reserved for them as they decode. Until
    /// each one does the panel draws an empty frame at exactly the right size,
    /// which is what stops a page from resizing under somebody reading it.
    ///
    /// Only the pictures the text actually refers to, in the order it refers to
    /// them, and at most `MAX_BOOK_PICTURES` of those: a container's spare
    /// artwork is not something to spend a decode on, and the order is what
    /// lets the plates arrive roughly as fast as they are read past.
    fn reserve_plates(
        &mut self,
        reader: &Reader,
        images: &BTreeMap<String, Vec<u8>>,
        metrics: &DisplayMetrics,
    ) -> BTreeMap<String, TilePicture> {
        self.plates.clear();
        self.book_pictures.clear();
        let mut reserved = BTreeMap::new();
        let Some((width, height)) = plate_box(metrics) else {
            return reserved;
        };
        for (index, name) in reader
            .pictures_wanted()
            .into_iter()
            .take(MAX_BOOK_PICTURES)
            .enumerate()
        {
            let Some(bytes) = images.get(name) else {
                continue;
            };
            // A header that will not parse is a plate that will not decode, so
            // it is left out here and the page reads its description instead --
            // which is what a book with a broken plate should look like.
            let Ok(source) = kobo_image::size(bytes) else {
                continue;
            };
            let (drawn_width, drawn_height) = kobo_image::fitted_size(source, width, height);
            if drawn_width == 0 || drawn_height == 0 {
                continue;
            }
            // Numbered from a base that cannot collide with the cover handles
            // the shelf is holding at the same time.
            let handle = PictureHandle(PICTURE_HANDLE_BASE + u32::try_from(index).unwrap_or(0));
            reserved.insert(
                name.to_owned(),
                TilePicture::new(handle, drawn_width, drawn_height),
            );
            self.book_pictures.push(handle);
            self.plates.push_back((name.to_owned(), bytes.clone()));
        }
        reserved
    }

    /// Carries one plate one step further towards the panel, and asks to be
    /// called again while any step is left.
    ///
    /// One step, not as many as fit in a time budget. Nothing here can be
    /// interrupted half way, so a budget can only be checked before starting
    /// something, and the pass then runs for the budget plus however long that
    /// something takes: on the panel a hundred and twenty millisecond budget
    /// produced callbacks of 272, 293 and 311 milliseconds against a deadline
    /// of 250. One whole plate per pass was no better -- 250 to 311 -- because
    /// one whole plate is itself over the deadline on this processor.
    ///
    /// So a plate is taken in its two natural halves. Reading the file and
    /// fitting it is one; reducing a million pixels to the sixteen greys the
    /// panel can hold is the other, and on a development machine the two are
    /// two milliseconds and three, which on the reader is about a hundred and
    /// about a hundred and seventy. Either alone is comfortably inside the
    /// deadline. There is no third half, and the honest fix is a runtime that
    /// can decode off this thread at all; until then this is where the seam is.
    ///
    /// The page being looked at is decoded first. Everything else can arrive
    /// while it is being read.
    fn decode_more_plates(&mut self, context: &mut Context) {
        let metrics = context.metrics();
        let Some((width, height)) = plate_box(&metrics) else {
            self.plates.clear();
            self.dithering = None;
            return;
        };
        let showing: Vec<String> = self.reader.as_ref().map_or_else(Vec::new, |reader| {
            reader
                .pictures_on_page()
                .into_iter()
                .map(str::to_owned)
                .collect()
        });
        let mut on_this_page = false;
        if let Some((name, mut picture)) = self.dithering.take() {
            // The second half: the greys.
            picture.dither(kobo_image::PANEL_GREYS);
            let (drawn_width, drawn_height) = (picture.width(), picture.height());
            if let Some(reserved) = self.handed_plate(&name) {
                if context
                    .put_picture(reserved, drawn_width, drawn_height, picture.into_grey())
                    .is_some()
                    && showing.contains(&name)
                {
                    on_this_page = true;
                }
            }
        } else {
            self.plates_first(&showing);
            // The first half: the file. A plate that will not read costs
            // nothing, so this goes past it rather than spending a whole round
            // trip discovering there was nothing to draw, and stops on the
            // first one that works.
            while let Some((name, bytes)) = self.plates.pop_front() {
                if self.handed_plate(&name).is_none() {
                    continue;
                }
                let Ok(picture) = kobo_image::decode(&bytes) else {
                    continue;
                };
                let Ok(picture) = picture.fit(width, height) else {
                    continue;
                };
                self.dithering = Some((name, picture));
                break;
            }
        }
        // Only when the page in front of somebody actually changed. A plate
        // twenty pages ahead landing is not a reason to flash an E Ink panel,
        // and there are two dozen of them in an illustrated book.
        if on_this_page {
            self.show(context);
        }
        if self.plates.is_empty() && self.dithering.is_none() {
            self.plating = None;
            return;
        }
        self.plating = context.spawn(Task::Sleep { seconds: 0 });
    }

    /// Moves the named plates to the front of the queue, keeping their order.
    fn plates_first(&mut self, names: &[String]) {
        if names.is_empty() {
            return;
        }
        let (wanted, rest): (VecDeque<_>, VecDeque<_>) = self
            .plates
            .drain(..)
            .partition(|(name, _)| names.contains(name));
        self.plates = wanted.into_iter().chain(rest).collect();
    }

    /// The handle a plate was reserved against, if it was.
    fn handed_plate(&self, name: &str) -> Option<PictureHandle> {
        self.reader
            .as_ref()
            .and_then(|reader| reader.picture_named(name))
            .map(|picture| picture.handle)
    }

    /// A blob that arrived and will not parse is forgotten rather than kept,
    /// on the same reasoning the cover cache already follows: a cached
    /// answer that turns out not to be usable must not persuade the device
    /// it has something it does not.
    fn discard_broken_book(&mut self, context: &mut Context) {
        self.download = None;
        self.complete = false;
        if let Some((blob, _)) = self.open_keys() {
            self.stored.remove(&blob);
            context.shelf().remove(blob);
        }
    }

    fn ask_light(&mut self, context: &mut Context) {
        if self
            .reader
            .as_ref()
            .is_some_and(|reader| reader.light().is_none())
        {
            context.device().read_frontlight();
        }
    }

    fn took_book(&mut self, context: &mut Context, bytes: &[u8]) {
        let Some(download) = &mut self.download else {
            return;
        };
        // The bytes, not the status: a supposed EPUB whose first chunk does
        // not begin with a zip's local file signature is refused before it
        // costs the reader a page of raw markup. Open Library publishes
        // open-access links that answer `200` with exactly that.
        if download.bytes.is_empty()
            && download.kind == DownloadKind::Epub
            && !bytes.starts_with(b"PK\x03\x04")
        {
            self.download = None;
            context.log(
                LogLevel::Warn,
                "book refused: the first bytes are not a zip".to_owned(),
            );
            self.failed = Some("This book did not arrive.".to_owned());
            self.retryable = false;
            self.back_to(View::Details);
            return;
        }
        // The ceiling applied to the bytes rather than to what the catalog
        // said about them. A stated length is a claim; this is the arrival.
        if download.bytes.len().saturating_add(bytes.len()) as u64 > MAX_BOOK_BYTES {
            let reached = download.bytes.len().saturating_add(bytes.len());
            self.download = None;
            context.log(
                LogLevel::Warn,
                format!("book refused at {reached} bytes, ceiling {MAX_BOOK_BYTES}"),
            );
            self.failed = Some("This book is too large to read on this device.".to_owned());
            self.retryable = false;
            self.view = View::Details;
            return;
        }
        let short = bytes.len() < CHUNK_BYTES as usize;
        download.bytes.extend_from_slice(bytes);
        self.fetched = u32::try_from(download.bytes.len()).unwrap_or(u32::MAX);
        let reached_total = download
            .total
            .is_some_and(|total| u64::from(self.fetched) >= total);
        self.complete = short || reached_total;
        self.reopen(context);
        self.ask_light(context);
        if self.complete && self.download.is_some() {
            self.keep_book(context);
        }
        if self.reader.is_some() {
            self.go(View::Reading);
        }
        // A chunk that lands is what asks for the one after it. Nothing else
        // can: the reader drives the plain text path by turning pages, and an
        // EPUB has no reader until the whole file is here.
        self.top_up(context);
    }
}

/// Which shelf page a stack entry's publications are cut into.
/// Brings an OPDS 2.0 feed's groups into the two collections the shelf draws.
///
/// A 2.0 catalog may put its publications inside groups rather than beside its
/// navigation, and a shelf that reads only the top level shows a rich home
/// feed as an empty one: Open Library's root carries fifty-four books that way
/// and drew none of them.
///
/// A group that names where its full collection lives becomes a row leading
/// there, which is what the group is for -- "Trending Books" is a place to go,
/// not a heading to print over a handful of covers. A group that names nowhere
/// has only what it is carrying, so that is folded onto the shelf, since the
/// alternative is discarding books the catalog took the trouble to send.
fn fold_groups(feed: &mut kobo_opds::Feed) {
    for group in std::mem::take(&mut feed.groups) {
        if let Some(href) = group.href {
            feed.navigation.push(kobo_opds::Navigation {
                title: group.title,
                href,
                summary: None,
                kind: None,
                rel: None,
                thumbnail: None,
            });
        } else {
            feed.navigation.extend(group.navigation);
            feed.publications.extend(group.publications);
        }
    }
}

/// Whether a book is small enough that reading it will not take the device.
///
/// An acquisition that states no length is allowed through: an unstated size
/// is unknown rather than enormous, and refusing every catalog that declines
/// to measure itself would refuse most of them. The ceiling still applies as
/// the bytes actually arrive, which is the check that cannot be lied to.
fn affordable(acquisition: &kobo_opds::Acquisition) -> bool {
    acquisition
        .length
        .is_none_or(|length| length <= MAX_BOOK_BYTES)
}

/// How an outcome is written in the trace.
///
/// A word rather than the whole value: a completed fetch carries the bytes,
/// and a book in the log is a log nobody can read.
fn outcome_name(outcome: &TaskOutcome) -> String {
    match outcome {
        TaskOutcome::Completed(bytes) => format!("ok {} bytes", bytes.len()),
        TaskOutcome::Failed(error) => format!("failed {error}"),
        TaskOutcome::Cancelled => "cancelled".to_owned(),
    }
}

fn shelf_pages(entry: &StackEntry) -> usize {
    let shown = if entry.feed.publications.is_empty() {
        entry.feed.navigation.len()
    } else {
        entry.feed.publications.len()
    };
    shown.div_ceil(SHELF_PAGE).max(1)
}

/// The blob name a book is kept under, and the key its reading place is kept
/// under -- both derived from the acquisition's own URL rather than the
/// title, so two catalogs' editions of the same book, or two editions within
/// one catalog, never collide over the same stored place.
fn book_keys(publication: &Publication) -> Option<(String, String)> {
    let acquisition = publication.best_acquisition()?;
    let stamp = stamp(&acquisition.href);
    Some((format!("book-{stamp:08x}"), format!("place-{stamp:08x}")))
}

fn cover_key(url: &str) -> String {
    kobo_sdk::cache_key(format!("cover.{:08x}", stamp(url)))
}

/// The box an illustration inside a book is fitted into, in pixels.
///
/// `None` on a panel so small the box has no area, which is not a device this
/// runs on but is a shape the arithmetic can produce.
fn plate_box(metrics: &DisplayMetrics) -> Option<(u32, u32)> {
    let width = metrics.tenth_mm(i32::from(PICTURE_WIDTH_MM) * 10);
    let height = metrics.tenth_mm(i32::from(PICTURE_HEIGHT_MM) * 10);
    match (u32::try_from(width), u32::try_from(height)) {
        (Ok(width), Ok(height)) if width > 0 && height > 0 => Some((width, height)),
        _ => None,
    }
}

/// Centres a picture on a sheet of exactly `width` by `height`.
///
/// The renderer draws a picture at the shape it was handed, so a cover handed
/// over at its own shape has a height decided by its publisher -- and a page
/// cannot reserve room for a number it does not know yet. Padding moves that
/// variation into a margin of white, which on white paper is nothing to look
/// at, and leaves everything below the cover exactly where it was first drawn.
///
/// A picture larger than the sheet in either direction is cropped rather than
/// scaled: callers fit before they get here, and silently resampling would
/// disagree with what was measured.
fn on_paper(picture: &kobo_image::Picture, width: u32, height: u32) -> Vec<u8> {
    let sheet_width = usize::try_from(width).unwrap_or(0);
    let sheet_height = usize::try_from(height).unwrap_or(0);
    let mut sheet = vec![u8::MAX; sheet_width * sheet_height];
    let source_width = usize::try_from(picture.width()).unwrap_or(0);
    let copied_width = source_width.min(sheet_width);
    let copied_height = usize::try_from(picture.height())
        .unwrap_or(0)
        .min(sheet_height);
    let left = (sheet_width - copied_width) / 2;
    let top = (sheet_height - copied_height) / 2;
    let grey = picture.grey();
    for row in 0..copied_height {
        let from = row * source_width;
        let to = (top + row) * sheet_width + left;
        let (Some(source), Some(target)) = (
            grey.get(from..from + copied_width),
            sheet.get_mut(to..to + copied_width),
        ) else {
            break;
        };
        target.copy_from_slice(source);
    }
    sheet
}

fn stamp(url: &str) -> u32 {
    let mut hash = 0x811c_9dc5_u32;
    for byte in url.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// A display name for a catalog a reader added by URL, since OPDS has no
/// convention for naming a catalog before its root feed is fetched.
fn catalog_display_name(url: &str) -> String {
    let without_scheme = url.trim_start_matches("https://");
    let host = without_scheme.split('/').next().unwrap_or(without_scheme);
    host.trim_start_matches("www.").to_owned()
}

/// The added-catalog registry, as bytes: one catalog per line, root and name
/// tab-separated. Chosen over a structured format because the data is two
/// strings with no nesting to describe, and because a list somebody can read
/// in a hex dump is a list somebody can recover by hand if this application
/// ever writes it wrongly.
fn encode_registry(catalogs: &[Catalog]) -> Vec<u8> {
    let mut out = String::new();
    for catalog in catalogs.iter().filter(|catalog| catalog.added) {
        out.push_str(&clean_field(&catalog.root));
        out.push('\t');
        out.push_str(&clean_field(&catalog.name));
        out.push('\n');
    }
    out.into_bytes()
}

fn clean_field(field: &str) -> String {
    field.replace(['\t', '\n', '\r'], " ").trim().to_owned()
}

fn decode_registry(bytes: &[u8]) -> Vec<Catalog> {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let root = fields.next().unwrap_or_default().trim();
            if !Gutenbird::looks_like_a_catalog_url(root) {
                return None;
            }
            let name = fields.next().unwrap_or_default().trim();
            let name = if name.is_empty() {
                catalog_display_name(root)
            } else {
                name.to_owned()
            };
            Some(Catalog::new(name, root.to_owned(), true))
        })
        .collect()
}

/// The flat facts for the details page, said only when the catalog actually
/// stated them. No invented reading time, no identifier relabelled as though
/// it belonged to one catalog when the field is generic across all of them.
fn detail_facts(publication: &Publication) -> Vec<(String, String)> {
    let mut facts = Vec::new();
    if let Some(language) = &publication.language {
        facts.push(("Language".to_owned(), language.clone()));
    }
    if let Some(issued) = publication
        .issued
        .as_ref()
        .or(publication.published.as_ref())
    {
        facts.push(("Published".to_owned(), issued.clone()));
    }
    if let Some(publisher) = &publication.publisher {
        facts.push(("Publisher".to_owned(), publisher.clone()));
    }
    if let Some(series) = &publication.series {
        let value = match series.position {
            Some(position) => format!("{} \u{00b7} {position}", series.name),
            None => series.name.clone(),
        };
        facts.push(("Series".to_owned(), value));
    }
    if let Some(identifier) = &publication.identifier {
        facts.push(("Identifier".to_owned(), identifier.clone()));
    }
    facts
}

// =====================================================================
// Project Gutenberg's plain text, for the catalogs that publish nothing else
// =====================================================================

/// Turns Project Gutenberg-style plain text into something worth reading on a
/// panel: the license trimmed from both ends, illustration captions for
/// pictures the plain edition does not contain removed, runs of spaces from a
/// title page laid out for a 1971 terminal collapsed, and underscore italics
/// markup stripped. Every rule keys on a marker this convention actually
/// writes, and each one leaves the text alone when its marker is missing, so
/// a file that does not follow the convention is passed through rather than
/// mangled.
fn readable(raw: &str) -> String {
    let body = between_markers(raw);
    let mut out = String::with_capacity(body.len());
    for line in body.lines() {
        let line = line.replace('_', "");
        let collapsed = collapse_runs(&line);
        let trimmed = collapsed.trim();
        if trimmed.is_empty() {
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

fn marker_line(text: &str, marker: &str) -> Option<usize> {
    let mut offset = 0;
    for line in text.lines() {
        let upper = line.trim_start().to_uppercase();
        if upper.starts_with(marker) {
            return Some(if marker.starts_with("*** S") {
                (offset + line.len() + 1).min(text.len())
            } else {
                offset
            });
        }
        offset += line.len() + 1;
    }
    None
}

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

fn main() -> ExitCode {
    match kobo_sdk::run("gutenbird", Gutenbird::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("gutenbird: {error}");
            ExitCode::FAILURE
        }
    }
}

impl KoboApp for Gutenbird {
    fn on_start(&mut self, context: &mut Context) {
        context.shelf().list();
        context.store().load(REGISTRY_KEY);
        context.store().load(LAST_OPEN_KEY);
        self.open_catalog(context);
    }

    #[allow(clippy::too_many_lines)]
    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        if let Some(upload) = &mut self.keeping {
            match upload.advance(context, &result) {
                ShelfProgress::Done => {
                    let name = upload.name().to_owned();
                    let size = self.download.as_ref().map_or(0, |download| {
                        u32::try_from(download.bytes.len()).unwrap_or(u32::MAX)
                    });
                    self.stored.insert(name, size);
                    self.keeping = None;
                    return;
                }
                ShelfProgress::Failed(_) => {
                    self.keeping = None;
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
                    let kind = self
                        .open
                        .as_ref()
                        .and_then(Publication::best_acquisition)
                        .map_or(DownloadKind::Text, |acquisition| {
                            download_kind(acquisition.media_type.as_deref())
                        });
                    let total = self
                        .open
                        .as_ref()
                        .and_then(Publication::best_acquisition)
                        .and_then(|acquisition| acquisition.length);
                    self.fetched = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
                    self.complete = true;
                    self.download = Some(Download {
                        url: self
                            .open
                            .as_ref()
                            .and_then(Publication::best_acquisition)
                            .map_or_else(String::new, |acquisition| acquisition.href.clone()),
                        kind,
                        bytes,
                        total,
                    });
                    self.reopen(context);
                    if self.reader.is_some() {
                        self.go(View::Reading);
                    }
                    self.show(context);
                    return;
                }
                ShelfProgress::Failed(_) => {
                    if let Some((blob, _)) = self.open_keys() {
                        self.stored.remove(&blob);
                        context.shelf().remove(blob);
                    }
                    self.loading = None;
                    self.ask_book(context);
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
            StoreResult::Loaded { key, value } if key == REGISTRY_KEY => {
                let mut added = value
                    .map(|bytes| decode_registry(&bytes))
                    .unwrap_or_default();
                self.catalogs.append(&mut added);
            }
            StoreResult::Loaded {
                key,
                value: Some(value),
            } if key == LAST_OPEN_KEY => {
                let root = String::from_utf8_lossy(&value).into_owned();
                if let Some(index) = self
                    .catalogs
                    .iter()
                    .position(|catalog| catalog.root == root)
                {
                    self.current = index;
                    self.open_catalog(context);
                }
            }
            StoreResult::Loaded { key, value }
                if self.looking.iter().any(|(held, _)| *held == key) =>
            {
                self.looked_for_cover(context, &key, value);
            }
            StoreResult::Loaded {
                value: Some(value), ..
            } => {
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

    fn on_device_result(
        &mut self,
        context: &mut Context,
        _request: kobo_sdk::DeviceRequest,
        result: kobo_sdk::DeviceResult,
    ) {
        let kobo_sdk::DeviceResult::Frontlight { percent } = result else {
            return;
        };
        if self
            .reader
            .as_mut()
            .is_some_and(|reader| reader.seed_light(percent))
        {
            self.show(context);
        }
    }

    fn on_text_hold(&mut self, context: &mut Context, action: ActionId, hit: kobo_sdk::TextHit) {
        if self.view != View::Reading || action != kobo_sdk::action_id(kobo_read::action::MARKING) {
            self.on_action(context, action);
            return;
        }
        let Ok(block) = u32::try_from(hit.context) else {
            return;
        };
        let range = kobo_read::TextRange {
            start: kobo_read::TextPosition {
                block,
                offset: hit.start,
            },
            end: kobo_read::TextPosition {
                block,
                offset: hit.end,
            },
        };
        let metrics = context.metrics();
        let created = self
            .reader
            .as_mut()
            .and_then(|reader| reader.annotate(range, None, &metrics).ok());
        if let Some(id) = created {
            self.pending_annotation = Some(id);
            self.keyboard.clear();
            self.save_place(context);
            self.go(View::Note);
            self.show(context);
        }
    }

    #[allow(clippy::too_many_lines)]
    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if action == ActionId::BACK {
            if self.view == View::Reading {
                let metrics = context.metrics();
                if let Some(reader) = &mut self.reader {
                    if reader.chrome() != kobo_read::Chrome::Hidden {
                        reader.set_chrome(kobo_read::Chrome::Hidden, &metrics);
                        self.show(context);
                        return;
                    }
                }
            }
            // A shelf deeper than its own root unwinds itself first. The pages
            // followed inside one catalog belong to that catalog rather than
            // to the application, and leaving the catalog while standing three
            // pages inside it is not what Back was asked for.
            if self.view == View::Shelf && self.stack.len() > 1 {
                self.stop_federating();
                self.stack.pop();
                self.want_covers(context);
                self.hydrate_visible(context);
            } else {
                if self.view == View::Reading && self.open.is_some() {
                    self.close_book(context);
                }
                if let Some(previous) = self.step_back() {
                    self.stop_federating();
                    self.view = previous;
                }
                // Nothing to step back to means the runtime was told this
                // screen has no Back of its own, so this call did not happen.
            }
            self.problem = None;
            self.trouble = None;
            self.show(context);
            return;
        }

        if self.view == View::Search {
            match self.keyboard.press(action) {
                Some(Pressed::Submitted) => {
                    let query = self.keyboard.take();
                    let query = query.trim().to_owned();
                    if !query.is_empty() {
                        if self.search_all {
                            self.begin_federated_search(context, query);
                        } else {
                            self.begin_search(context, self.current, query, false);
                        }
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
        if self.view == View::Note {
            match self.keyboard.press(action) {
                Some(Pressed::Submitted) => {
                    let note = self.keyboard.take();
                    if let (Some(id), Some(reader)) =
                        (self.pending_annotation.take(), self.reader.as_mut())
                    {
                        let note = (!note.trim().is_empty()).then_some(note.trim());
                        let _ = reader.edit_annotation_note(id, note);
                        self.save_place(context);
                    }
                    self.back_to(View::Reading);
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
        if self.view == View::AddCatalog {
            match self.keyboard.press(action) {
                Some(Pressed::Submitted) => {
                    self.submit_catalog(context);
                    return;
                }
                Some(Pressed::Edited | Pressed::Shifted) => {
                    self.show(context);
                    return;
                }
                None => {}
            }
        }

        if action == action_id("catalogs") {
            self.stop_federating();
            self.go(View::Catalogs);
            self.show(context);
            return;
        }
        if action == action_id("add-catalog") {
            self.keyboard.clear();
            self.add_catalog_problem = None;
            self.go(View::AddCatalog);
            self.show(context);
            return;
        }
        if action == action_id("search") {
            self.problem = None;
            self.go(View::Search);
            self.show(context);
            return;
        }
        if action == action_id("search-all") {
            self.search_all = !self.search_all;
            self.show(context);
            return;
        }
        if action == action_id("query-clear") {
            self.keyboard.clear();
            self.show(context);
            return;
        }
        if action == action_id("catalog-url-clear") {
            self.keyboard.clear();
            self.show(context);
            return;
        }
        if action == action_id("annotation-note-clear") {
            self.keyboard.clear();
            self.show(context);
            return;
        }
        if action == action_id("read") {
            self.get_book(context);
            self.show(context);
            return;
        }

        for (index, term) in self
            .current_catalog()
            .recent
            .clone()
            .into_iter()
            .enumerate()
        {
            if action == action_id(&format!("recent-{index}")) {
                self.go(View::Search);
                self.begin_search(context, self.current, term, false);
                self.show(context);
                return;
            }
        }

        for index in 0..self.catalogs.len() {
            if action == action_id(&format!("catalog-{index}")) {
                self.current = index;
                self.open_catalog(context);
                return;
            }
        }

        if let Some(category) = self.category_for(action) {
            self.go(View::Search);
            self.begin_search(context, self.current, category, false);
            self.show(context);
            return;
        }

        if self.view == View::Reading && self.read_action(context, action) {
            return;
        }

        if action == action_id("about-next") || action == action_id("about-back") {
            if action == action_id("about-next") {
                self.detail_page = self.detail_page.saturating_add(1);
            } else {
                self.detail_page = self.detail_page.saturating_sub(1);
            }
            self.show(context);
            return;
        }

        if action == action_id("shelf-next") || action == action_id("shelf-back") {
            let Some(entry) = self.stack.last_mut() else {
                return;
            };
            let pages = shelf_pages(entry);
            if action == action_id("shelf-next") {
                if entry.page + 1 >= pages {
                    self.ask_more(context);
                    self.show(context);
                    return;
                }
                entry.page += 1;
            } else {
                entry.page = entry.page.saturating_sub(1);
            }
            self.show(context);
            self.want_covers(context);
            self.hydrate_visible(context);
            return;
        }

        if let Some(entry) = self.stack.last() {
            for (index, navigation) in entry.feed.navigation.iter().enumerate() {
                if action == action_id(&format!("nav-{index}")) {
                    let href = navigation.href.clone();
                    self.follow(context, href);
                    return;
                }
            }
            for index in 0..entry.feed.publications.len() {
                if action == action_id(&format!("book-{index}")) {
                    let publication = entry.feed.publications[index].clone();
                    self.open_publication(context, publication);
                    return;
                }
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    fn on_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
        if let Some(stage) = self.finish_filling(task) {
            context.log(
                LogLevel::Debug,
                format!(
                    "fill {task:?} {} ({} lanes busy)",
                    outcome_name(&outcome),
                    self.filling.len()
                ),
            );
            if let TaskOutcome::Completed(bytes) = outcome {
                match stage {
                    FillStage::Entry { href } => self.took_hydration(context, &bytes, &href),
                    FillStage::Picture { href } => self.took_nav_cover(context, &bytes, &href),
                }
            } else {
                // A row that will not load stays a tile with a glyph, which
                // is what it already was.
                self.hydrate_visible(context);
            }
            return;
        }
        if let Some((index, tries)) = self.finish_cover(task) {
            match outcome {
                TaskOutcome::Completed(bytes) => self.keep_cover(context, index, &bytes),
                TaskOutcome::Failed(_) => self.retry_cover(index, tries),
                TaskOutcome::Cancelled => {}
            }
            self.next_cover(context);
            return;
        }
        if self.plating == Some(task) {
            self.plating = None;
            // Only while a book is open. Leaving one empties the queue, and a
            // sleep that was already in flight when it did lands here.
            if self.reader.is_some() {
                self.decode_more_plates(context);
            }
            return;
        }
        if self.open_cover_task.is_some_and(|(id, _)| id == task) {
            let (_, tries) = self.open_cover_task.take().expect("just checked");
            match outcome {
                TaskOutcome::Completed(bytes) => {
                    if !self.took_open_cover(context, &bytes) {
                        self.letter_open_cover(context);
                    }
                    self.show(context);
                }
                TaskOutcome::Failed(_) if tries + 1 < COVER_TRIES => {
                    if let Some(image) = self.open.as_ref().and_then(Publication::cover) {
                        if let ImageSource::Url(url) = image.href.clone() {
                            if let Some(new_task) = context.spawn_retrying(Task::Fetch {
                                url,
                                offset: 0,
                                max_bytes: COVER_BYTES,
                                credential: None,
                                headers: Vec::new(),
                            }) {
                                self.open_cover_task = Some((new_task, tries + 1));
                            }
                        }
                    }
                }
                // Out of attempts. The frame stays where it is and takes the
                // book's own title, rather than emptying and letting the page
                // move under whoever is looking at it.
                TaskOutcome::Failed(_) => {
                    self.letter_open_cover(context);
                    self.show(context);
                }
                TaskOutcome::Cancelled => {}
            }
            return;
        }

        let Some((outstanding, awaiting)) = self.task.clone() else {
            context.log(
                LogLevel::Debug,
                format!(
                    "{task:?} {} arrived with nothing waiting",
                    outcome_name(&outcome)
                ),
            );
            return;
        };
        if outstanding != task {
            context.log(
                LogLevel::Debug,
                format!("{task:?} answered after {outstanding:?} replaced it"),
            );
            return;
        }
        self.task = None;
        context.log(
            LogLevel::Info,
            format!("{task:?} {} for {awaiting:?}", outcome_name(&outcome)),
        );
        match outcome {
            TaskOutcome::Completed(bytes) => match awaiting {
                Awaiting::Feed(purpose, base) => self.took_feed(context, &bytes, purpose, base),
                Awaiting::DiscoverRoot {
                    catalog,
                    query,
                    federated,
                } => {
                    let root = self.catalogs[catalog].root.clone();
                    self.took_discover_root(context, &bytes, &root, catalog, query, federated);
                }
                Awaiting::DiscoverDescription {
                    catalog,
                    query,
                    federated,
                    url,
                } => {
                    if let Some(template) = kobo_opds::parse_opensearch(&bytes, &url) {
                        let way = SearchWay::Template(template);
                        self.catalogs[catalog].search = SearchState::Known(way.clone());
                        self.issue_search(context, catalog, &query, &way, federated);
                    } else {
                        self.catalogs[catalog].search = SearchState::None;
                        self.search_unavailable(context, federated);
                    }
                }
                Awaiting::Book => self.took_book(context, &bytes),
            },
            TaskOutcome::Failed(error) => match awaiting {
                Awaiting::Book => {
                    if self
                        .download
                        .as_ref()
                        .is_none_or(|download| download.bytes.is_empty())
                    {
                        let failure = Failure::of(error);
                        self.failed = Some(failure.advice.to_owned());
                        self.retryable = failure.retryable;
                        self.back_to(View::Details);
                    } else if let Some(reader) = &mut self.reader {
                        reader.expect_more(false);
                    }
                }
                Awaiting::DiscoverRoot {
                    catalog, federated, ..
                }
                | Awaiting::DiscoverDescription {
                    catalog, federated, ..
                } => {
                    self.catalogs[catalog].search = SearchState::None;
                    self.search_unavailable(context, federated);
                }
                Awaiting::Feed(FeedPurpose::Federated { .. }, _) => {
                    self.advance_federated(context);
                }
                // A row that could not be followed stays a row. It is already
                // marked as examined, so the next page turn does not spend
                // the radio asking again.
                Awaiting::Feed(..) => {
                    let failure = Failure::of(error);
                    self.trouble = Some(failure);
                    self.problem = Some(failure.advice.to_owned());
                }
            },
            TaskOutcome::Cancelled => {
                self.problem = Some("Cancelled.".to_owned());
            }
        }
        self.show(context);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        book_keys, catalog_display_name, decode_registry, download_kind, encode_registry,
        plate_box, read_offer, readable, Awaiting, BTreeMap, BTreeSet, Catalog, DetailBlock,
        Download, DownloadKind, FeedPurpose, FillStage, Gutenbird, Memory, ReadOffer, Reader,
        SearchState, SearchWay, StackEntry, View, COVER_TRIES, OPEN_COVER_HANDLE,
    };
    use kobo_opds::{
        Acquisition, AcquisitionKind, Category, Feed, Image, ImageSource, Link, Navigation,
        Publication, Relation,
    };
    use kobo_sdk::{
        action_id, AppRunner, Command, Context, DiagnosticSeverity, StoreRequest, StoreResult,
        Task, TaskError, TaskId, TaskOutcome,
    };
    use kobo_ui::{Chrome, LayoutKind, PictureHandle, TilePicture, CLARA_BW_METRICS};

    // -----------------------------------------------------------------
    // Fixture builders
    // -----------------------------------------------------------------

    fn publication(title: &str, acquisition: Vec<Acquisition>) -> Publication {
        Publication {
            title: title.to_owned(),
            identifier: None,
            authors: vec!["Some Author".to_owned()],
            summary: None,
            language: None,
            issued: None,
            published: None,
            updated: None,
            publisher: None,
            rights: None,
            extent: None,
            categories: Vec::new(),
            series: None,
            images: Vec::new(),
            acquisition,
            links: Vec::new(),
        }
    }

    fn acquisition(kind: AcquisitionKind, href: &str, media_type: &str) -> Acquisition {
        Acquisition {
            kind,
            href: href.to_owned(),
            media_type: Some(media_type.to_owned()),
            title: None,
            length: None,
            price: None,
            indirect: Vec::new(),
            available: true,
        }
    }

    fn epub_acquisition(href: &str) -> Acquisition {
        acquisition(AcquisitionKind::OpenAccess, href, "application/epub+zip")
    }

    /// An EPUB that states how big it is, which is how a catalog lets a
    /// reader's device choose between two editions of the same words.
    fn sized_epub(href: &str, length: u64) -> Acquisition {
        Acquisition {
            length: Some(length),
            ..epub_acquisition(href)
        }
    }

    fn text_acquisition(href: &str) -> Acquisition {
        acquisition(AcquisitionKind::OpenAccess, href, "text/plain")
    }

    fn cover_url(url: &str) -> Image {
        Image {
            href: ImageSource::Url(url.to_owned()),
            media_type: Some("image/jpeg".to_owned()),
            width: None,
            height: None,
            thumbnail: false,
        }
    }

    fn cover_inline(bytes: Vec<u8>) -> Image {
        Image {
            href: ImageSource::Inline {
                media_type: "image/png".to_owned(),
                bytes,
            },
            media_type: Some("image/png".to_owned()),
            width: None,
            height: None,
            thumbnail: false,
        }
    }

    fn navigation(title: &str, href: &str) -> Navigation {
        Navigation {
            title: title.to_owned(),
            href: href.to_owned(),
            summary: None,
            kind: None,
            rel: None,
            thumbnail: None,
        }
    }

    fn search_link(href: &str) -> Link {
        Link {
            rel: vec![Relation::Search],
            href: href.to_owned(),
            media_type: Some("application/atom+xml".to_owned()),
            title: None,
        }
    }

    /// One transparent pixel, which is a picture the decoder accepts.
    const PIXEL: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f,
        0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0xfc,
        0xcf, 0xc0, 0x50, 0x0f, 0x00, 0x04, 0x85, 0x01, 0x80, 0x84, 0xa9, 0x8c, 0x21, 0x00, 0x00,
        0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    /// A solid-colour 40x40 PNG -- big enough to clear `MIN_COVER_PX`, unlike
    /// [`PIXEL`], which exists to be *under* it.
    const LARGE_PIXEL: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x28, 0x00, 0x00, 0x00, 0x28, 0x08, 0x02, 0x00, 0x00, 0x00, 0x03,
        0x9c, 0x2f, 0x3a, 0x00, 0x00, 0x00, 0x30, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0xed, 0xcd,
        0x41, 0x09, 0x00, 0x00, 0x08, 0x04, 0xb0, 0x0b, 0x66, 0x12, 0x93, 0x18, 0xdf, 0x12, 0x82,
        0x9f, 0xc1, 0xfe, 0xcb, 0x74, 0xbd, 0x88, 0x58, 0x2c, 0x16, 0x8b, 0xc5, 0x62, 0xb1, 0x58,
        0x2c, 0x16, 0x8b, 0xc5, 0x62, 0xb1, 0x58, 0x7c, 0x67, 0x01, 0x56, 0xe3, 0x97, 0xdb, 0xe1,
        0x1b, 0x8a, 0x73, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    const BASE: &str = "https://example.org/catalog";

    /// A feed of two publications, one carrying a cover and one not -- the
    /// shape the cover pipeline tests exercise, mirroring what a real
    /// acquisition feed (Standard Ebooks, the 2.0 catalog) looks like.
    fn two_publication_feed() -> Feed {
        let mut with_cover = publication(
            "A Journey",
            vec![epub_acquisition("https://example.org/journey.epub")],
        );
        with_cover.images = vec![cover_url("https://example.org/covers/journey.jpg")];
        let bare = publication(
            "A Bare Book",
            vec![epub_acquisition("https://example.org/bare.epub")],
        );
        Feed {
            title: Some("Test Catalog".to_owned()),
            publications: vec![with_cover, bare],
            ..Feed::default()
        }
    }

    fn first_cover_key() -> String {
        super::cover_key("https://example.org/covers/journey.jpg")
    }

    /// The shelf, freshly arrived, with its cover still to find.
    fn shelved() -> AppRunner<Gutenbird> {
        let mut runner = AppRunner::new(Gutenbird {
            task: Some((
                TaskId(9),
                Awaiting::Feed(FeedPurpose::Root { catalog: 0 }, BASE.to_owned()),
            )),
            ..Gutenbird::default()
        });
        let _ignored = runner.task_outcome(
            TaskId(9),
            TaskOutcome::Completed(two_publication_feed_json().into_bytes()),
        );
        runner
    }

    /// The same two-publication feed, as the OPDS 2.0 JSON body a real feed
    /// fetch would answer with -- used wherever a test needs to go through
    /// `kobo_opds::parse` rather than constructing a [`Feed`] by hand.
    fn two_publication_feed_json() -> String {
        r#"{
            "metadata": {"title": "Test Catalog"},
            "publications": [
                {
                    "metadata": {"title": "A Journey", "author": "Jules Verne"},
                    "links": [{"rel": "http://opds-spec.org/acquisition/open-access", "href": "https://example.org/journey.epub", "type": "application/epub+zip"}],
                    "images": [{"href": "https://example.org/covers/journey.jpg", "type": "image/jpeg"}]
                },
                {
                    "metadata": {"title": "A Bare Book"},
                    "links": [{"rel": "http://opds-spec.org/acquisition/open-access", "href": "https://example.org/bare.epub", "type": "application/epub+zip"}]
                }
            ]
        }"#
        .to_owned()
    }

    fn app_with_stack(feed: Feed) -> Gutenbird {
        Gutenbird {
            stack: vec![StackEntry::fresh(feed, BASE.to_owned())],
            ..Gutenbird::default()
        }
    }

    // -----------------------------------------------------------------
    // Gutenberg-style plain text cleanup
    // -----------------------------------------------------------------

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
        assert!(
            !clean.contains("GEORGE ALLEN") && !clean.contains("Illustration"),
            "the illustration survived: {clean}"
        );
        assert!(
            clean.contains("PRIDE. and PREJUDICE"),
            "the run was not collapsed: {clean}"
        );
        assert!(!clean.contains('_'), "markup is showing: {clean}");
        assert!(clean.contains("declare for Pride and Prejudice unhesitatingly."));
        assert!(clean.contains("a truth universally acknowledged"));
        assert!(clean.contains("\n\n"), "paragraphs were lost: {clean}");
    }

    #[test]
    fn a_file_that_follows_none_of_the_conventions_is_left_alone() {
        let plain = "Chapter One\n\nIt was a bright cold day in April.\n";
        assert_eq!(readable(plain), plain);
    }

    #[test]
    fn an_unclosed_caption_never_swallows_the_rest_of_the_book() {
        let truncated = "Chapter One\n\n[Illustration: the frontispiece\n";
        let clean = readable(truncated);
        assert!(clean.contains("Chapter One"), "{clean}");
    }

    // -----------------------------------------------------------------
    // The feed stack, navigation and entry documents
    // -----------------------------------------------------------------

    #[test]
    fn a_catalog_that_answers_a_navigation_feed_lists_rows_rather_than_an_empty_shelf() {
        let feed = Feed {
            title: Some("Browse".to_owned()),
            navigation: vec![navigation("Fiction", "https://example.org/fiction")],
            ..Feed::default()
        };
        let app = app_with_stack(feed);
        let text = screen_text(&app.shelf_screen(&Context::default()));
        assert!(text.iter().any(|line| line.contains("Fiction")), "{text:?}");
    }

    #[test]
    fn following_a_book_in_a_navigation_feed_opens_its_details_rather_than_another_list() {
        // The rule that makes Gutenberg work without a line of code about
        // Gutenberg: a `subsection` link is followed, the document behind it
        // holds one publication and no navigation, and that goes straight to
        // Details.
        let mut runner = AppRunner::new(app_with_stack(Feed {
            navigation: vec![navigation("Moby-Dick", "https://gutenberg.example/entry/1")],
            ..Feed::default()
        }));
        runner.action(action_id("nav-0"));
        let entry = r#"{
            "metadata": {"title": "Moby-Dick"},
            "publications": [{
                "metadata": {"title": "Moby-Dick"},
                "links": [{"rel": "http://opds-spec.org/acquisition/open-access", "href": "https://gutenberg.example/moby.epub", "type": "application/epub+zip"}]
            }]
        }"#;
        let task = runner
            .app_mut()
            .task
            .as_ref()
            .map(|(id, _)| *id)
            .expect("a fetch was started");
        runner.task_outcome(task, TaskOutcome::Completed(entry.as_bytes().to_vec()));
        assert_eq!(runner.app().view, View::Details);
        assert_eq!(
            runner.app().open.as_ref().map(|p| p.title.as_str()),
            Some("Moby-Dick")
        );
    }

    #[test]
    fn a_feed_holding_one_publication_and_no_navigation_is_taken_as_an_entry_document() {
        let feed = Feed {
            publications: vec![publication(
                "Solo",
                vec![epub_acquisition("https://x/solo.epub")],
            )],
            ..Feed::default()
        };
        assert!(Gutenbird::resolve_entry(&feed).is_some());
    }

    #[test]
    fn a_feed_with_navigation_and_one_publication_is_not_taken_as_an_entry_document() {
        let feed = Feed {
            navigation: vec![navigation("Elsewhere", "https://x/elsewhere")],
            publications: vec![publication(
                "Solo",
                vec![epub_acquisition("https://x/solo.epub")],
            )],
            ..Feed::default()
        };
        assert!(Gutenbird::resolve_entry(&feed).is_none());
    }

    #[test]
    fn gutenbergs_images_and_noimages_editions_of_one_entry_collapse_to_one_book() {
        // Gutenberg's per-book entry document answers with two publications
        // sharing a title -- the `.images` and `.noimages` editions -- and
        // one book is shown rather than the same title twice.
        let feed = Feed {
            publications: vec![
                publication(
                    "Emma",
                    vec![sized_epub(
                        "https://gutenberg.example/1.noimages.epub",
                        558_547,
                    )],
                ),
                publication(
                    "Emma",
                    vec![sized_epub(
                        "https://gutenberg.example/1.images.epub",
                        24_846_294,
                    )],
                ),
            ],
            ..Feed::default()
        };
        let resolved = Gutenbird::resolve_entry(&feed).expect("collapses to one book");
        // The illustrated edition of this one is twenty-four megabytes, which
        // is past what this device can parse, so the plain one is what opens.
        assert!(resolved.acquisition[0].href.contains(".noimages"));
    }

    #[test]
    fn editions_that_name_no_pictures_fall_back_to_the_first() {
        // Not every catalog spells its editions the way Gutenberg does, and
        // one of two identical-looking books is better than neither.
        let feed = Feed {
            publications: vec![
                publication(
                    "Emma",
                    vec![sized_epub("https://elsewhere.example/a.epub", 900_000)],
                ),
                publication(
                    "Emma",
                    vec![sized_epub("https://elsewhere.example/b.epub", 100_000)],
                ),
            ],
            ..Feed::default()
        };
        let resolved = Gutenbird::resolve_entry(&feed).expect("collapses to one book");
        assert_eq!(resolved.acquisition[0].length, Some(900_000));
    }

    #[test]
    fn a_group_that_says_where_it_lives_becomes_somewhere_to_go() {
        // Open Library's root carries its books this way. A group naming its
        // own collection is a place to go rather than a heading to print over
        // whichever few covers happened to be sent inline.
        let mut feed = Feed {
            groups: vec![kobo_opds::Group {
                title: "Trending Books".to_owned(),
                href: Some("https://catalog.example/trending".to_owned()),
                navigation: Vec::new(),
                publications: vec![publication(
                    "Roots",
                    vec![epub_acquisition("https://x/1.epub")],
                )],
            }],
            ..Feed::default()
        };
        super::fold_groups(&mut feed);
        assert_eq!(feed.navigation.len(), 1);
        assert_eq!(feed.navigation[0].title, "Trending Books");
        assert_eq!(feed.navigation[0].href, "https://catalog.example/trending");
        assert!(
            feed.publications.is_empty(),
            "the books were folded onto the shelf as well as linked"
        );
    }

    #[test]
    fn a_group_that_says_nowhere_gives_up_the_books_it_is_carrying() {
        // The alternative is discarding books the catalog took the trouble to
        // send, which is what drawing only the top level did.
        let mut feed = Feed {
            groups: vec![kobo_opds::Group {
                title: "Classic Books".to_owned(),
                href: None,
                navigation: Vec::new(),
                publications: vec![
                    publication("Emma", vec![epub_acquisition("https://x/1.epub")]),
                    publication("Persuasion", vec![epub_acquisition("https://x/2.epub")]),
                ],
            }],
            ..Feed::default()
        };
        super::fold_groups(&mut feed);
        assert_eq!(feed.publications.len(), 2);
        assert!(feed.navigation.is_empty());
    }

    #[test]
    fn a_catalogue_cannot_send_this_device_to_another_host() {
        let feed = Feed {
            links: vec![Link {
                rel: vec![Relation::Next],
                href: "https://elsewhere.example/page2".to_owned(),
                media_type: None,
                title: None,
            }],
            publications: vec![publication(
                "One",
                vec![epub_acquisition("https://x/1.epub")],
            )],
            ..Feed::default()
        };
        let entry = StackEntry::fresh(feed, "https://example.org/catalog".to_owned());
        assert!(entry.next.is_none(), "an off-site next was followed");
    }

    #[test]
    fn a_same_host_next_link_is_kept_and_followed() {
        let feed = Feed {
            links: vec![Link {
                rel: vec![Relation::Next],
                href: "https://example.org/page2".to_owned(),
                media_type: None,
                title: None,
            }],
            publications: vec![publication(
                "One",
                vec![epub_acquisition("https://x/1.epub")],
            )],
            ..Feed::default()
        };
        let entry = StackEntry::fresh(feed, "https://example.org/catalog".to_owned());
        assert_eq!(entry.next.as_deref(), Some("https://example.org/page2"));
    }

    // -----------------------------------------------------------------
    // Getting the book: format choice and the button it draws
    // -----------------------------------------------------------------

    #[test]
    fn an_epub_is_preferred_over_plain_text_when_a_catalog_offers_both() {
        let book = publication(
            "Two Editions",
            vec![
                text_acquisition("https://x/book.txt"),
                epub_acquisition("https://x/book.epub"),
            ],
        );
        assert_eq!(read_offer(&book), ReadOffer::Read);
        let chosen = book.best_acquisition().expect("a download");
        assert!(chosen.href.to_ascii_lowercase().ends_with(".epub"));
    }

    #[test]
    fn a_book_offered_only_for_sale_says_so_instead_of_offering_a_download_that_fails() {
        let book = publication(
            "For Sale",
            vec![acquisition(
                AcquisitionKind::Buy,
                "https://x/buy",
                "application/epub+zip",
            )],
        );
        match read_offer(&book) {
            ReadOffer::Unavailable(_) => {}
            other => panic!("expected Unavailable, got {other:?}"),
        }
        let app = Gutenbird {
            open: Some(book),
            ..Gutenbird::default()
        };
        let text = screen_text(&app.details_screen(&Context::default()));
        assert!(
            text.iter().any(|line| line.contains("Not available here")),
            "{text:?}"
        );
        assert!(!text.iter().any(|line| line == "Read"), "{text:?}");
    }

    #[test]
    fn a_sample_is_never_presented_as_the_whole_book() {
        let book = publication(
            "A Taste",
            vec![acquisition(
                AcquisitionKind::Sample,
                "https://x/sample.epub",
                "application/epub+zip",
            )],
        );
        assert_eq!(read_offer(&book), ReadOffer::Sample);
        let app = Gutenbird {
            open: Some(book),
            ..Gutenbird::default()
        };
        let text = screen_text(&app.details_screen(&Context::default()));
        assert!(text.iter().any(|line| line == "Read sample"), "{text:?}");
        assert!(!text.iter().any(|line| line == "Read"), "{text:?}");
    }

    #[test]
    fn an_entry_whose_links_are_all_unsafe_survives_with_nothing_to_read() {
        let book = publication("Nothing Safe", Vec::new());
        assert_eq!(read_offer(&book), ReadOffer::Nothing);
    }

    // -----------------------------------------------------------------
    // The EPUB path
    // -----------------------------------------------------------------

    #[test]
    fn an_epub_arrives_in_pieces_and_is_not_opened_until_the_last_one_lands() {
        let book = publication("Piecemeal", vec![epub_acquisition("https://x/book.epub")]);
        let mut runner = AppRunner::new(Gutenbird {
            open: Some(book),
            view: View::Details,
            task: Some((TaskId(1), Awaiting::Book)),
            download: Some(Download {
                url: "https://x/book.epub".to_owned(),
                kind: DownloadKind::Epub,
                bytes: Vec::new(),
                total: None,
            }),
            ..Gutenbird::default()
        });
        // A first, partial chunk beginning with the zip signature: not
        // enough to be a whole EPUB, and correctly not opened.
        runner.task_outcome(
            TaskId(1),
            TaskOutcome::Completed(b"PK\x03\x04not a whole book".to_vec()),
        );
        assert!(runner.app().reader.is_none(), "a partial EPUB was opened");
        assert!(!runner.app().complete);
    }

    #[test]
    fn a_download_that_does_not_begin_with_the_zip_signature_is_refused_rather_than_shown_as_a_book(
    ) {
        // Open Library publishes open-access links that answer 200 with an
        // HTML page. The status was fine; the bytes were not a book.
        let book = publication("Mislabelled", vec![epub_acquisition("https://x/book.epub")]);
        let mut runner = AppRunner::new(Gutenbird {
            open: Some(book),
            view: View::Details,
            task: Some((TaskId(1), Awaiting::Book)),
            download: Some(Download {
                url: "https://x/book.epub".to_owned(),
                kind: DownloadKind::Epub,
                bytes: Vec::new(),
                total: None,
            }),
            ..Gutenbird::default()
        });
        runner.task_outcome(
            TaskId(1),
            TaskOutcome::Completed(b"<html>not a book</html>".to_vec()),
        );
        assert_eq!(
            runner.app().failed.as_deref(),
            Some("This book did not arrive.")
        );
        assert!(runner.app().reader.is_none());
    }

    fn epub_bytes() -> Vec<u8> {
        kobo_doc::epub::write(
            "A Test Book",
            Some("Some Author"),
            &[kobo_doc::epub::Chapter {
                title: "Chapter One".to_owned(),
                body: "It begins.".to_owned(),
            }],
        )
        .expect("a small synthetic EPUB")
    }

    #[test]
    fn a_whole_epub_is_opened_once_the_last_chunk_lands() {
        let book = publication("Whole", vec![epub_acquisition("https://x/book.epub")]);
        let mut runner = AppRunner::new(Gutenbird {
            open: Some(book),
            view: View::Details,
            task: Some((TaskId(1), Awaiting::Book)),
            download: Some(Download {
                url: "https://x/book.epub".to_owned(),
                kind: DownloadKind::Epub,
                bytes: Vec::new(),
                total: None,
            }),
            ..Gutenbird::default()
        });
        let bytes = epub_bytes();
        assert!(
            bytes.len() < super::CHUNK_BYTES as usize,
            "fixture too big for one chunk"
        );
        runner.task_outcome(TaskId(1), TaskOutcome::Completed(bytes));
        assert!(
            runner.app().reader.is_some(),
            "the finished EPUB was not opened"
        );
        assert_eq!(runner.app().view, View::Reading);
    }

    #[test]
    fn an_epub_already_on_the_shelf_is_read_from_it_rather_than_downloaded_again() {
        let book = publication("Kept", vec![epub_acquisition("https://x/kept.epub")]);
        let (blob, _) = book_keys(&book).expect("keys");
        let mut runner = AppRunner::new(Gutenbird {
            open: Some(book),
            stored: [(blob.clone(), 4096)].into_iter().collect(),
            ..Gutenbird::default()
        });
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
                Command::Store(StoreRequest::ShelfRead { name, .. }) if *name == blob
            )),
            "the copy on the device was not read"
        );
    }

    #[test]
    fn an_epub_that_will_not_parse_is_thrown_away_rather_than_kept_forever() {
        let book = publication("Corrupt", vec![epub_acquisition("https://x/corrupt.epub")]);
        let (blob, _) = book_keys(&book).expect("keys");
        let mut runner = AppRunner::new(Gutenbird {
            open: Some(book),
            stored: [(blob.clone(), 4096)].into_iter().collect(),
            ..Gutenbird::default()
        });
        runner.action(action_id("read"));
        let mut bytes = b"PK\x03\x04".to_vec();
        bytes.extend_from_slice(&[0u8; 32]); // a zip header with nothing usable behind it
        let commands = runner.store_result(StoreResult::ShelfRead {
            name: blob.clone(),
            offset: 0,
            bytes: bytes.clone(),
            size: u32::try_from(bytes.len()).unwrap(),
        });
        assert!(
            commands.iter().any(|command| matches!(
                command,
                Command::Store(StoreRequest::ShelfRemove { name }) if *name == blob
            )),
            "a book that will not parse was kept forever: {commands:?}"
        );
        assert!(!runner.app().stored.contains_key(&blob));
    }

    // -----------------------------------------------------------------
    // Covers
    // -----------------------------------------------------------------

    #[test]
    fn a_cover_is_looked_for_on_the_device_before_it_is_asked_for_over_the_radio() {
        let mut runner = AppRunner::new(Gutenbird {
            task: Some((
                TaskId(9),
                Awaiting::Feed(FeedPurpose::Root { catalog: 0 }, BASE.to_owned()),
            )),
            ..Gutenbird::default()
        });
        let commands = runner.task_outcome(
            TaskId(9),
            TaskOutcome::Completed(two_publication_feed_json().into_bytes()),
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
            value: Some(LARGE_PIXEL.to_vec()),
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
        let commands = runner.task_outcome(task, TaskOutcome::Completed(LARGE_PIXEL.to_vec()));
        assert!(
            commands.iter().any(|command| matches!(
                command,
                Command::Store(StoreRequest::Save { key, .. }) if *key == first_cover_key()
            )),
            "a cover that came off the radio was not kept"
        );
    }

    #[test]
    fn a_cover_that_did_not_arrive_is_asked_for_again_but_not_forever() {
        let mut app = Gutenbird::default();
        app.retry_cover(4, 0);
        assert_eq!(app.wanted, vec![(4, 1)]);
        app.wanted = vec![(7, 0)];
        app.retry_cover(4, 1);
        assert_eq!(app.wanted, vec![(4, 2), (7, 0)]);
        let mut app = Gutenbird::default();
        app.retry_cover(4, COVER_TRIES - 1);
        assert!(app.wanted.is_empty());
    }

    #[test]
    fn a_data_uri_thumbnail_is_decoded_rather_than_fetched() {
        let mut book = publication("Inline Cover", vec![epub_acquisition("https://x/1.epub")]);
        book.images = vec![cover_inline(LARGE_PIXEL.to_vec())];
        let mut app = app_with_stack(Feed {
            publications: vec![book],
            ..Feed::default()
        });
        let mut context = Context::default();
        app.want_covers(&mut context);
        let commands = context.take_commands();
        assert!(
            !commands
                .iter()
                .any(|command| matches!(command, Command::Spawn { .. })),
            "an inline cover was fetched over the radio: {commands:?}"
        );
        assert!(
            app.stack[0].covers[0].is_some(),
            "the inline cover was never decoded"
        );
    }

    #[test]
    fn an_icon_too_small_to_be_a_cover_is_not_enlarged_into_one() {
        // A tiny picture the decoder still accepts. Gutenberg's own
        // navigation icons are 22x22; this fixture is 1x1, well under
        // `MIN_COVER_PX` either way.
        let mut book = publication("Tiny Icon", vec![epub_acquisition("https://x/1.epub")]);
        book.images = vec![cover_inline(PIXEL.to_vec())];
        let mut app = app_with_stack(Feed {
            publications: vec![book],
            ..Feed::default()
        });
        let mut context = Context::default();
        app.want_covers(&mut context);
        assert!(
            app.stack[0].covers[0].is_none(),
            "a 1x1 icon was enlarged into a cover"
        );
    }

    #[test]
    fn covers_are_filled_in_only_for_the_shelf_page_being_looked_at() {
        let publications: Vec<Publication> = (0..8)
            .map(|index| {
                let mut book = publication(
                    &format!("Book {index}"),
                    vec![epub_acquisition(&format!("https://x/{index}.epub"))],
                );
                book.images = vec![cover_url(&format!("https://x/covers/{index}.jpg"))];
                book
            })
            .collect();
        let mut app = app_with_stack(Feed {
            publications,
            ..Feed::default()
        });
        let mut context = Context::default();
        app.want_covers(&mut context);
        // Only the first SHELF_PAGE (six) covers are looked up; the two on
        // the next page are left alone until the reader turns to them.
        assert_eq!(app.looking.len(), super::SHELF_PAGE);
    }

    // -----------------------------------------------------------------
    // Search
    // -----------------------------------------------------------------

    #[test]
    fn a_catalog_with_no_search_template_offers_no_keyboard() {
        let mut app = Gutenbird::default();
        app.catalogs[0].search = SearchState::None;
        app.view = View::Search;
        let text = screen_text(&app.search_screen());
        assert!(
            text.iter().any(|line| line.contains("no search")),
            "{text:?}"
        );
    }

    #[test]
    fn a_search_term_cannot_add_parameters_of_its_own_to_the_url() {
        let way = SearchWay::Direct(search_link("https://example.org/search{?query}"));
        assert_eq!(
            way.expand("dickens&sort=popular").as_deref(),
            Some("https://example.org/search?query=dickens%26sort%3Dpopular")
        );
    }

    fn known_search(catalog: &mut Catalog, href: &str) {
        catalog.search = SearchState::Known(SearchWay::Direct(search_link(href)));
    }

    #[test]
    fn typing_a_search_asks_the_catalogue_for_exactly_what_was_typed() {
        let mut app = Gutenbird::default();
        known_search(&mut app.catalogs[0], "https://example.org/search{?query}");
        let mut runner = AppRunner::new(app);
        runner.action(action_id("search"));
        for key in ["kb.r0c9", "kb.r1c0", "kb.r0c1"] {
            runner.action(action_id(key));
        }
        let commands = runner.action(action_id("kb.enter"));
        let asked = commands.iter().find_map(|command| match command {
            Command::Spawn { work, .. } => Some(work.clone()),
            _ => None,
        });
        let Some(Task::Fetch { url, .. }) = asked else {
            panic!("no request was made");
        };
        assert!(url.ends_with("?query=paw"), "asked for {url}");
    }

    #[test]
    fn a_search_is_remembered_so_it_need_not_be_typed_twice() {
        let mut app = Gutenbird::default();
        known_search(&mut app.catalogs[0], "https://example.org/search{?query}");
        let mut runner = AppRunner::new(app);
        runner.action(action_id("search"));
        for key in ["kb.r0c9", "kb.r1c0", "kb.r0c1"] {
            runner.action(action_id(key));
        }
        runner.action(action_id("kb.enter"));
        assert_eq!(runner.app().catalogs[0].recent, vec!["paw".to_owned()]);
    }

    #[test]
    fn a_recent_search_belongs_to_the_catalog_it_was_typed_into() {
        let mut app = Gutenbird::default();
        known_search(&mut app.catalogs[0], "https://a.example/search{?query}");
        known_search(&mut app.catalogs[1], "https://b.example/search{?query}");
        let mut context = Context::default();
        app.push_recent(0, "dickens");
        let _ = &mut context;
        assert_eq!(app.catalogs[0].recent, vec!["dickens".to_owned()]);
        assert!(
            app.catalogs[1].recent.is_empty(),
            "a search leaked into another catalog"
        );
    }

    #[test]
    fn a_search_template_is_discovered_once_and_reused_rather_than_refetched() {
        let mut runner = AppRunner::new(Gutenbird::default());
        let commands = {
            let app = runner.app_mut();
            let mut context = Context::default();
            app.begin_search(&mut context, 0, "first".to_owned(), false);
            context.take_commands()
        };
        let root_fetch = commands.iter().any(|command| matches!(
            command,
            Command::Spawn { work: Task::Fetch { url, .. }, .. } if *url == runner.app().catalogs[0].root
        ));
        assert!(
            root_fetch,
            "the first search did not have to discover the catalog's search link"
        );
        assert!(runner.app().task.is_some());

        // Answer the root with a feed carrying a direct search link.
        let task = runner.app().task.as_ref().map(|(id, _)| *id).unwrap();
        let root_body = r#"{"metadata": {"title": "T"}, "links": [{"rel": "search", "href": "https://example.org/search{?query}", "type": "application/opds+json", "templated": true}]}"#.to_string();
        runner.task_outcome(task, TaskOutcome::Completed(root_body.into_bytes()));
        assert!(matches!(
            runner.app().catalogs[0].search,
            SearchState::Known(_)
        ));

        // A second search against the same catalog goes straight to the
        // search URL -- no second fetch of the root.
        let task = runner.app().task.as_ref().map(|(id, _)| *id).unwrap();
        runner.task_outcome(
            task,
            TaskOutcome::Completed(b"{\"publications\":[]}".to_vec()),
        );
        let commands = {
            let app = runner.app_mut();
            let mut context = Context::default();
            app.begin_search(&mut context, 0, "second".to_owned(), false);
            context.take_commands()
        };
        let asked = commands.iter().find_map(|command| match command {
            Command::Spawn {
                work: Task::Fetch { url, .. },
                ..
            } => Some(url.clone()),
            _ => None,
        });
        assert_eq!(
            asked.as_deref(),
            Some("https://example.org/search?query=second")
        );
    }

    #[test]
    fn an_opensearch_description_yields_the_atom_url_rather_than_the_html_one() {
        let description = br#"<?xml version="1.0"?>
            <OpenSearchDescription xmlns="http://a9.com/-/spec/opensearch/1.1/">
              <Url type="text/html" template="https://example.org/search.html?q={searchTerms}"/>
              <Url type="application/atom+xml" template="https://example.org/search.atom?q={searchTerms}"/>
            </OpenSearchDescription>"#;
        let template =
            kobo_opds::parse_opensearch(description, "https://example.org/opensearch.xml")
                .expect("a template");
        assert_eq!(
            template.expand("dickens"),
            "https://example.org/search.atom?q=dickens"
        );
    }

    #[test]
    fn an_http_search_template_is_upgraded_to_https_rather_than_refused() {
        let description = br#"<?xml version="1.0"?>
            <OpenSearchDescription xmlns="http://a9.com/-/spec/opensearch/1.1/">
              <Url type="application/atom+xml" template="http://example.org/search?q={searchTerms}"/>
            </OpenSearchDescription>"#;
        let template =
            kobo_opds::parse_opensearch(description, "https://example.org/opensearch.xml")
                .expect("a template");
        assert!(
            template.expand("x").starts_with("https://"),
            "{}",
            template.expand("x")
        );
    }

    #[test]
    fn a_search_template_on_another_host_is_followed_but_a_paging_link_on_another_host_is_not() {
        let way = SearchWay::Direct(search_link("https://m.example.org/search{?query}"));
        assert_eq!(
            way.expand("x").as_deref(),
            Some("https://m.example.org/search?query=x")
        );

        let feed = Feed {
            links: vec![Link {
                rel: vec![Relation::Next],
                href: "https://m.example.org/page2".to_owned(),
                media_type: None,
                title: None,
            }],
            publications: vec![publication(
                "One",
                vec![epub_acquisition("https://x/1.epub")],
            )],
            ..Feed::default()
        };
        let entry = StackEntry::fresh(feed, "https://example.org/catalog".to_owned());
        assert!(
            entry.next.is_none(),
            "paging followed a host the reader never asked for"
        );
    }

    #[test]
    fn searching_every_catalog_appends_each_answer_as_it_arrives_rather_than_waiting_for_all_of_them(
    ) {
        let mut app = Gutenbird::default();
        known_search(&mut app.catalogs[0], "https://a.example/search{?query}");
        known_search(&mut app.catalogs[1], "https://b.example/search{?query}");
        app.search_all = true;
        app.view = View::Search;
        let mut runner = AppRunner::new(app);
        runner.action(action_id("search"));
        for key in ["kb.r0c9", "kb.r1c0", "kb.r0c1"] {
            runner.action(action_id(key));
        }
        runner.action(action_id("kb.enter"));
        assert!(runner.app().federating, "a federated search did not start");
        assert_eq!(
            runner.app().all_queue.len(),
            runner.app().catalogs.len() - 1
        );

        let task = runner
            .app()
            .task
            .as_ref()
            .map(|(id, _)| *id)
            .expect("the first catalog was asked");
        let answer = r#"{"publications": [{"metadata": {"title": "From A"}, "links": [{"rel": "http://opds-spec.org/acquisition/open-access", "href": "https://a.example/1.epub", "type": "application/epub+zip"}]}]}"#;
        runner.task_outcome(task, TaskOutcome::Completed(answer.as_bytes().to_vec()));

        // The first catalog's book is on the shelf while the second is still
        // being asked.
        assert_eq!(
            runner.app().stack.last().unwrap().feed.publications.len(),
            1
        );
        assert!(
            runner.app().task.is_some(),
            "the second catalog was not asked yet"
        );
    }

    #[test]
    fn leaving_the_search_screen_stops_the_catalogs_that_have_not_been_asked_yet() {
        let mut app = Gutenbird::default();
        known_search(&mut app.catalogs[0], "https://a.example/search{?query}");
        known_search(&mut app.catalogs[1], "https://b.example/search{?query}");
        known_search(&mut app.catalogs[2], "https://c.example/search{?query}");
        // A root already on the stack, as it always is by the time a reader
        // reaches Search in the running application -- otherwise the
        // federated results page would be the only thing on the stack and
        // Back would have nothing to pop it back to.
        app.stack = vec![StackEntry::fresh(Feed::default(), BASE.to_owned())];
        app.search_all = true;
        app.view = View::Search;
        app.trail = vec![View::Catalogs, View::Shelf];
        let mut runner = AppRunner::new(app);
        runner.action(action_id("search"));
        for key in ["kb.r0c9", "kb.r1c0", "kb.r0c1"] {
            runner.action(action_id(key));
        }
        runner.action(action_id("kb.enter"));
        assert!(!runner.app().all_queue.is_empty());

        // Leaving before the results have finished arriving: Back pops the
        // federated results page off the stack.
        runner.action(kobo_sdk::ActionId::BACK);
        assert!(
            runner.app().all_queue.is_empty(),
            "catalogs not yet asked were left queued"
        );
        assert!(!runner.app().federating);
    }

    // -----------------------------------------------------------------
    // Catalog registry
    // -----------------------------------------------------------------

    #[test]
    fn adding_a_catalog_by_url_keeps_it_and_a_malformed_url_is_refused_before_it_is_stored() {
        let mut app = Gutenbird {
            view: View::AddCatalog,
            ..Gutenbird::default()
        };
        let before = app.catalogs.len();

        // A malformed address.
        app.add_catalog(&mut Context::default(), "not a url");
        assert_eq!(
            app.catalogs.len(),
            before,
            "a malformed url was stored anyway"
        );
        assert!(app.add_catalog_problem.is_some());

        // A well formed one.
        app.add_catalog(&mut Context::default(), "https://example.org/opds");
        assert_eq!(app.catalogs.len(), before + 1);
        assert_eq!(
            app.catalogs.last().unwrap().root,
            "https://example.org/opds"
        );
        assert!(app.catalogs.last().unwrap().added);
        assert!(app.add_catalog_problem.is_none());
    }

    #[test]
    fn the_added_catalog_registry_round_trips_through_storage() {
        let catalogs = vec![Catalog::new("Mine", "https://example.org/opds", true)];
        let bytes = encode_registry(&catalogs);
        let decoded = decode_registry(&bytes);
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].root, "https://example.org/opds");
        assert_eq!(decoded[0].name, "Mine");
    }

    #[test]
    fn a_line_in_the_registry_with_a_malformed_url_is_dropped_rather_than_kept() {
        let decoded = decode_registry(b"not a url\tSomething\nhttps://good.example/opds\tGood\n");
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].root, "https://good.example/opds");
    }

    #[test]
    fn a_catalog_display_name_falls_back_to_the_host() {
        assert_eq!(
            catalog_display_name("https://www.example.org/opds/root"),
            "example.org"
        );
    }

    // -----------------------------------------------------------------
    // Details page facts
    // -----------------------------------------------------------------

    #[test]
    fn rights_is_shown_verbatim_when_the_catalog_states_it_and_not_at_all_when_it_does_not() {
        let mut with_rights = publication("Has Rights", Vec::new());
        with_rights.rights = Some("Public domain in the USA.".to_owned());
        let blocks = Gutenbird::detail_blocks(&with_rights);
        assert!(blocks.iter().any(
            |block| matches!(block, DetailBlock::Text(text) if text == "Public domain in the USA.")
        ));

        let without = publication("No Rights", Vec::new());
        let blocks = Gutenbird::detail_blocks(&without);
        assert!(!blocks
            .iter()
            .any(|block| matches!(block, DetailBlock::Section("Rights"))));
    }

    #[test]
    fn categories_become_chips_that_run_a_search() {
        let mut book = publication("Categorised", Vec::new());
        book.categories = vec![Category {
            term: "sf".to_owned(),
            label: Some("Science Fiction".to_owned()),
            scheme: None,
        }];
        let app = Gutenbird {
            open: Some(book),
            ..Gutenbird::default()
        };
        let action = app.category_for(action_id("category-0"));
        assert_eq!(action.as_deref(), Some("Science Fiction"));
    }

    #[test]
    fn download_kind_prefers_epub_media_types_over_plain_text() {
        assert_eq!(
            download_kind(Some("application/epub+zip")),
            DownloadKind::Epub
        );
        assert_eq!(
            download_kind(Some("application/kepub+zip")),
            DownloadKind::Epub
        );
        assert_eq!(download_kind(Some("text/plain")), DownloadKind::Text);
        assert_eq!(
            download_kind(Some("text/plain; charset=utf-8")),
            DownloadKind::Text
        );
    }

    #[test]
    fn book_keys_differ_between_two_editions_and_never_come_from_the_title() {
        let a = publication(
            "Same Title",
            vec![epub_acquisition("https://catalog-a.example/book.epub")],
        );
        let b = publication(
            "Same Title",
            vec![epub_acquisition("https://catalog-b.example/book.epub")],
        );
        assert_ne!(book_keys(&a), book_keys(&b));
    }

    // -----------------------------------------------------------------
    // Plain text fallback and pagination (unchanged machinery, now driven
    // by an OPDS acquisition rather than a Gutendex URL)
    // -----------------------------------------------------------------

    #[test]
    fn a_downloaded_chunk_is_broken_into_pages_that_fit_the_panel() {
        let book = publication("Prose", vec![text_acquisition("https://x/book.txt")]);
        let mut runner = AppRunner::new(Gutenbird {
            view: View::Reading,
            open: Some(book),
            task: Some((TaskId(1), Awaiting::Book)),
            download: Some(Download {
                url: "https://x/book.txt".to_owned(),
                kind: DownloadKind::Text,
                bytes: Vec::new(),
                total: None,
            }),
            ..Gutenbird::default()
        });
        let prose = "It is a truth universally acknowledged, that a single man in possession \
                     of a good fortune, must be in want of a wife.\n\n"
            .repeat(30);
        runner.task_outcome(
            TaskId(1),
            TaskOutcome::Completed(prose.clone().into_bytes()),
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
                .reading_screen(&kobo_sdk::Context::default())
                .layout_with(&metrics, &Chrome::with_back(true));
            let drawn = layout
                .nodes
                .iter()
                .filter(|node| matches!(node.kind, LayoutKind::Text | LayoutKind::RichText(_)))
                .map(|node| node.id)
                .collect::<BTreeSet<_>>()
                .len();
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

    fn opened(text: &str) -> Reader {
        Reader::open(
            kobo_doc::read("book.txt", text.as_bytes()).expect("a readable book"),
            Memory::default(),
            &CLARA_BW_METRICS,
        )
    }

    /// A plate is measured from its header and decoded later.
    ///
    /// Opening The Tale of Peter Rabbit on the panel decoded twenty-eight
    /// images inside a lifecycle callback and took 2,784 milliseconds doing
    /// it, against a deadline of 250. Reading the headers costs microseconds
    /// and answers the only question pagination has, so the book opens at the
    /// page count it will be read at and the pixels arrive afterwards.
    #[test]
    fn a_book_of_plates_is_measured_from_its_headers_and_decoded_afterwards() {
        let plate = kobo_image::encode_png_grey(1200, 2000, &vec![128; 1200 * 2000])
            .expect("a png of a plate");
        let images: BTreeMap<String, Vec<u8>> = [("plate.png".to_owned(), plate.clone())]
            .into_iter()
            .collect();
        // Enough prose to fill the page it opens on, so that the room the
        // plate takes is the difference between one page and two.
        let mut blocks: Vec<kobo_doc::Block> = (0..12)
            .map(|_| {
                kobo_doc::Block::Paragraph(
                    "Once upon a time there were four little rabbits, and their names were \
                     Flopsy, Mopsy, Cotton-tail and Peter."
                        .to_owned(),
                )
            })
            .collect();
        blocks.push(kobo_doc::Block::Picture {
            name: "plate.png".to_owned(),
            alt: "Four rabbits".to_owned(),
        });
        let document = kobo_doc::Document {
            blocks,
            ..kobo_doc::Document::default()
        };
        let mut reader = Reader::open(document, Memory::default(), &CLARA_BW_METRICS);
        let without = reader.page_count();

        let mut app = Gutenbird::default();
        let reserved = app.reserve_plates(&reader, &images, &CLARA_BW_METRICS);

        // The room, from the header alone, and the same room decoding would
        // have asked for.
        let (width, height) = plate_box(&CLARA_BW_METRICS).expect("a panel with room on it");
        let decoded = kobo_image::decode(&plate)
            .expect("decode")
            .fit(width, height)
            .expect("fit");
        assert_eq!(
            reserved.get("plate.png").map(|picture| picture.source),
            Some((decoded.width(), decoded.height())),
            "the size claimed from the header is not the size the decoder produces"
        );
        assert_eq!(
            app.plates.len(),
            1,
            "the plate should be queued rather than decoded"
        );

        reader.set_pictures(reserved, &CLARA_BW_METRICS);
        assert!(
            reader.page_count() > without,
            "the book was not measured around the room the plate takes"
        );
    }

    /// And the pixels, when they come, go into the room already reserved.
    #[test]
    fn a_queued_plate_is_handed_over_against_the_handle_it_was_measured_at() {
        let plate = kobo_image::encode_png_grey(1200, 2000, &vec![128; 1200 * 2000])
            .expect("a png of a plate");
        let images: BTreeMap<String, Vec<u8>> =
            [("plate.png".to_owned(), plate)].into_iter().collect();
        let document = kobo_doc::Document {
            blocks: vec![kobo_doc::Block::Picture {
                name: "plate.png".to_owned(),
                alt: "Four rabbits".to_owned(),
            }],
            ..kobo_doc::Document::default()
        };
        let mut reader = Reader::open(document, Memory::default(), &CLARA_BW_METRICS);
        let mut app = Gutenbird::default();
        let reserved = app.reserve_plates(&reader, &images, &CLARA_BW_METRICS);
        let claimed = reserved
            .get("plate.png")
            .copied()
            .expect("the plate was reserved");
        reader.set_pictures(reserved, &CLARA_BW_METRICS);
        app.reader = Some(reader);
        // The sleep that carries the queue round, answered.
        app.plating = Some(TaskId(1));

        let mut runner = AppRunner::new(app);
        // The first pass reads the plate and asks for another; the second
        // reduces it to the panel's greys and hands it over. Neither half is
        // allowed to take as long as both would.
        let first = runner.task_outcome(TaskId(1), TaskOutcome::Completed(Vec::new()));
        assert!(
            handed_pictures(&first).is_empty(),
            "a plate was decoded and dithered in one callback"
        );
        assert!(
            runner.app().plates.is_empty() && runner.app().dithering.is_some(),
            "the plate was not read out of the book"
        );
        let plating = runner
            .app()
            .plating
            .expect("another pass was not asked for");

        let second = runner.task_outcome(plating, TaskOutcome::Completed(Vec::new()));
        assert_eq!(
            handed_pictures(&second),
            vec![(claimed.handle, claimed.source.0, claimed.source.1)],
            "the plate did not fill the frame that was standing in for it"
        );
        assert!(
            runner.app().dithering.is_none(),
            "the decoded plate was kept after it was drawn"
        );
    }

    fn handed_pictures(commands: &[kobo_sdk::Command]) -> Vec<(PictureHandle, u32, u32)> {
        commands
            .iter()
            .filter_map(|command| match command {
                kobo_sdk::Command::PutPicture {
                    handle,
                    width,
                    height,
                    ..
                } => Some((*handle, *width, *height)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn leaving_a_book_gives_back_everything_it_was_costing() {
        // The device this was found on had been reading an illustrated book,
        // gone back to the shelf, and then answered a tap five seconds later
        // until it stopped answering at all. Nothing was released on the way
        // out: the parsed document, the downloaded bytes and every decoded
        // plate all stayed, and were only dropped when some other book was
        // opened -- which an owner browsing covers never does.
        let book = publication("Illustrated", vec![epub_acquisition("https://x/book.epub")]);
        let mut runner = AppRunner::new(Gutenbird {
            open: Some(book),
            view: View::Reading,
            // The way an owner actually arrives at a page of a book: a
            // catalog, a shelf, the book's own page, and then reading it.
            trail: vec![View::Catalogs, View::Shelf, View::Details],
            complete: true,
            reader: Some(opened("A book with plates in it.")),
            download: Some(Download {
                url: "https://x/book.epub".to_owned(),
                kind: DownloadKind::Epub,
                bytes: vec![0; 1_510_370],
                total: None,
            }),
            book_pictures: vec![PictureHandle(1000), PictureHandle(1001)],
            ..Gutenbird::default()
        });
        let commands = runner.action(kobo_sdk::ActionId::BACK);
        let given_back: Vec<PictureHandle> = commands
            .iter()
            .filter_map(|command| match command {
                kobo_sdk::Command::DropPicture(handle) => Some(*handle),
                _ => None,
            })
            .collect();
        assert_eq!(
            given_back,
            vec![PictureHandle(1000), PictureHandle(1001)],
            "the runtime was left holding the plates"
        );
        assert!(runner.app().reader.is_none(), "the parsed book was kept");
        assert!(
            runner.app().download.is_none(),
            "the downloaded bytes were kept"
        );
        assert_eq!(runner.app().view, View::Details);
    }

    /// The root of a catalog is not the end of the road.
    ///
    /// On the device, Project Gutenberg's own first screen drew no Back at
    /// all, so the only way to the catalog list was the globe -- which does
    /// not look like navigation -- and the list's Back went straight back to
    /// Project Gutenberg. There was no way out of the catalog you were in.
    #[test]
    fn the_first_screen_of_a_catalog_leads_back_to_the_list_of_catalogs() {
        let mut runner = AppRunner::new(Gutenbird::default());
        runner.start();
        assert_eq!(
            runner.app().view,
            View::Shelf,
            "starting up should open the catalog that was last open"
        );
        assert!(
            runner.app().can_go_back(),
            "the root of a catalog drew no way back to the catalog list"
        );

        runner.action(kobo_sdk::ActionId::BACK);
        assert_eq!(runner.app().view, View::Catalogs);
        assert!(
            !runner.app().can_go_back(),
            "the catalog list is the root, and Back there means leaving"
        );
    }

    /// Back undoes the step that was taken, rather than naming a screen.
    ///
    /// Pressing the globe from a shelf and then Back used to land on the
    /// shelf by coincidence -- Back on the catalog list was hard-coded to the
    /// shelf whatever had come before it. From the Add screen, which is only
    /// ever reached from the list, the same rule threw the owner two screens.
    #[test]
    fn back_returns_to_the_screen_the_step_was_taken_from() {
        let mut runner = AppRunner::new(Gutenbird::default());
        runner.start();
        runner.action(action_id("catalogs"));
        assert_eq!(runner.app().view, View::Catalogs);
        runner.action(action_id("add-catalog"));
        assert_eq!(runner.app().view, View::AddCatalog);

        runner.action(kobo_sdk::ActionId::BACK);
        assert_eq!(
            runner.app().view,
            View::Catalogs,
            "leaving the Add screen should return to the list it was opened from"
        );
        runner.action(kobo_sdk::ActionId::BACK);
        assert_eq!(
            runner.app().view,
            View::Shelf,
            "leaving the list should return to the shelf the globe was pressed on"
        );
    }

    /// One book's cover must not be drawn in the frame the next book reserved.
    ///
    /// Every book's cover is held against a single handle. The detail page
    /// claims the room for a cover before the bytes arrive, and names that
    /// handle to do it -- so with the last book's pixels still against it, the
    /// runtime drew them. On the panel, The Tale of Peter Rabbit opened with
    /// Pride and Prejudice's peacock where Beatrix Potter's cover belonged.
    #[test]
    fn arriving_at_a_book_gives_back_the_cover_the_last_one_left_behind() {
        let mut app = Gutenbird::default();
        let mut context = kobo_sdk::Context::default();
        app.open_publication(
            &mut context,
            publication("The Tale of Peter Rabbit", Vec::new()),
        );
        assert!(
            context.commands().iter().any(|command| matches!(
                command,
                kobo_sdk::Command::DropPicture(handle) if *handle == OPEN_COVER_HANDLE
            )),
            "the runtime was left holding the cover of whatever was open before"
        );
    }

    #[test]
    fn a_book_still_arriving_is_not_released_out_from_under_the_chunk_that_is_coming() {
        // The guard the release needs: a text book is readable while the rest
        // of it is still downloading, and the next chunk expects to find what
        // came before it.
        let book = publication("Streaming", vec![epub_acquisition("https://x/book.txt")]);
        let mut runner = AppRunner::new(Gutenbird {
            open: Some(book),
            view: View::Reading,
            trail: vec![View::Catalogs, View::Shelf, View::Details],
            complete: false,
            reader: Some(opened("The first chunk of a longer book.")),
            download: Some(Download {
                url: "https://x/book.txt".to_owned(),
                kind: DownloadKind::Text,
                bytes: b"The first chunk of a longer book.".to_vec(),
                total: Some(9_000),
            }),
            ..Gutenbird::default()
        });
        runner.action(kobo_sdk::ActionId::BACK);
        assert!(
            runner.app().download.is_some(),
            "a download still in flight was thrown away"
        );
    }

    #[test]
    fn the_page_controls_are_reachable_by_a_tap_at_their_centre() {
        let mut reader = opened("A short book.");
        reader.act(kobo_read::action::CONTROLS, &CLARA_BW_METRICS);
        let application = Gutenbird {
            view: View::Reading,
            reader: Some(reader),
            complete: true,
            ..Gutenbird::default()
        };
        let layout = application
            .reading_screen(&kobo_sdk::Context::default())
            .layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true));
        let controls = layout
            .nodes
            .iter()
            .filter_map(|node| match node.kind {
                LayoutKind::Button(action, kobo_ui::ControlState::Enabled, _)
                | LayoutKind::ChoiceOption(action, _) => Some((action, node.rect)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            controls.len() >= 5,
            "the reading controls are not all there: {controls:?}"
        );
        for (action, rect) in controls {
            let hit = layout.hit_test(rect.x + rect.width / 2, rect.y + rect.height / 2);
            assert_eq!(hit, Some(action));
        }
    }

    #[test]
    fn holding_a_word_opens_marginalia_and_saves_the_note_on_that_exact_word() {
        let mut runner = AppRunner::new(Gutenbird {
            view: View::Reading,
            reader: Some(opened("café novel")),
            complete: true,
            ..Gutenbird::default()
        });
        runner.text_hold(
            action_id(kobo_read::action::MARKING),
            kobo_sdk::TextHit {
                context: 0,
                start: 0,
                end: 5,
            },
        );
        assert_eq!(runner.app().view, View::Note);
        assert_eq!(runner.app().reader.as_ref().unwrap().annotations().len(), 1);

        runner.action(action_id("kb.r0c0"));
        runner.action(action_id("kb.enter"));
        assert_eq!(runner.app().view, View::Reading);
        let annotation = &runner.app().reader.as_ref().unwrap().annotations()[0];
        assert_eq!(annotation.note.as_deref(), Some("q"));
    }

    #[test]
    fn the_front_light_has_a_control_of_its_own() {
        let mut reader = opened("A short book.");
        reader.act(kobo_read::action::LIGHT, &CLARA_BW_METRICS);
        let application = Gutenbird {
            view: View::Reading,
            reader: Some(reader),
            complete: true,
            ..Gutenbird::default()
        };
        let screen = application.reading_screen(&kobo_sdk::Context::default());
        let bar = screen.top_bar.as_ref().expect("a top bar");
        assert!(
            bar.actions
                .iter()
                .any(|action| action.glyph == Some(kobo_ui::Glyph::Light)),
            "there is no light control in the bar"
        );
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true));
        let steps = layout
            .nodes
            .iter()
            .filter(|node| matches!(node.kind, LayoutKind::ChoiceOption(..)))
            .count();
        assert_eq!(
            steps, 2,
            "the light panel is not the two steps and nothing else"
        );
    }

    #[test]
    fn a_failed_download_stays_on_the_book_with_a_way_to_try_again() {
        let book = publication("Failing", vec![text_acquisition("https://x/book.txt")]);
        let mut runner = AppRunner::new(Gutenbird {
            view: View::Reading,
            open: Some(book),
            task: Some((TaskId(1), Awaiting::Book)),
            download: Some(Download {
                url: "https://x/book.txt".to_owned(),
                kind: DownloadKind::Text,
                bytes: Vec::new(),
                total: None,
            }),
            ..Gutenbird::default()
        });
        runner.task_outcome(TaskId(1), TaskOutcome::Failed(TaskError::Unreachable));
        assert_eq!(runner.app_mut().view, View::Details);
        assert!(runner.app_mut().failed.is_some());
        assert!(runner.app_mut().problem.is_none());
    }

    #[test]
    fn a_failure_that_retrying_cannot_help_offers_no_retry() {
        let book = publication("Denied", vec![text_acquisition("https://x/book.txt")]);
        let mut runner = AppRunner::new(Gutenbird {
            view: View::Reading,
            open: Some(book),
            task: Some((TaskId(1), Awaiting::Book)),
            download: Some(Download {
                url: "https://x/book.txt".to_owned(),
                kind: DownloadKind::Text,
                bytes: Vec::new(),
                total: None,
            }),
            ..Gutenbird::default()
        });
        runner.task_outcome(TaskId(1), TaskOutcome::Failed(TaskError::Denied));
        assert!(!runner.app_mut().retryable);
        let screen = runner
            .app_mut()
            .details_screen(&kobo_sdk::Context::default());
        assert!(
            !screen_text(&screen)
                .iter()
                .any(|line| line.contains("Try again")),
            "a retry is offered for a failure that retrying cannot help"
        );
    }

    fn screen_text(screen: &kobo_sdk::Screen) -> Vec<String> {
        screen
            .layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true))
            .nodes
            .iter()
            .flat_map(|node| node.text_lines.clone())
            .collect()
    }

    #[test]
    fn a_short_answer_means_the_book_ended() {
        let book = publication("Short", vec![text_acquisition("https://x/short.txt")]);
        let mut runner = AppRunner::new(Gutenbird {
            open: Some(book),
            task: Some((TaskId(1), Awaiting::Book)),
            download: Some(Download {
                url: "https://x/short.txt".to_owned(),
                kind: DownloadKind::Text,
                bytes: Vec::new(),
                total: None,
            }),
            ..Gutenbird::default()
        });
        runner.task_outcome(
            TaskId(1),
            TaskOutcome::Completed(b"The whole of a very short book.".to_vec()),
        );
        assert!(runner.app_mut().complete);
    }

    #[test]
    fn opening_a_different_book_keeps_none_of_the_last_one() {
        let feed = two_publication_feed();
        let mut runner = AppRunner::new(Gutenbird {
            stack: vec![StackEntry::fresh(feed, BASE.to_owned())],
            open: Some(publication(
                "Something Else",
                vec![text_acquisition("https://x/else.txt")],
            )),
            reader: Some(opened("Chapter forty of something else.")),
            complete: true,
            ..Gutenbird::default()
        });
        runner.action(action_id("book-0"));
        let application = runner.app_mut();
        assert!(application.download.is_none());
        assert_eq!(application.fetched, 0);
        assert!(application.reader.is_none(), "the last book is still open");
        assert!(!application.complete);
        assert_eq!(
            application.open.as_ref().map(|p| p.title.as_str()),
            Some("A Journey")
        );
    }

    #[test]
    fn a_book_that_is_not_here_yet_is_still_fetched() {
        let book = publication("Fresh", vec![epub_acquisition("https://x/fresh.epub")]);
        let mut runner = AppRunner::new(Gutenbird {
            open: Some(book),
            ..Gutenbird::default()
        });
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
        let book = publication("Finishing", vec![text_acquisition("https://x/finish.txt")]);
        let (blob, _) = book_keys(&book).expect("keys");
        let mut runner = AppRunner::new(Gutenbird {
            view: View::Reading,
            open: Some(book),
            task: Some((TaskId(1), Awaiting::Book)),
            download: Some(Download {
                url: "https://x/finish.txt".to_owned(),
                kind: DownloadKind::Text,
                bytes: Vec::new(),
                total: None,
            }),
            ..Gutenbird::default()
        });
        let commands = runner.task_outcome(
            TaskId(1),
            TaskOutcome::Completed(b"A whole short book.\n\nAnd its second part.".to_vec()),
        );
        assert!(
            commands.iter().any(|command| matches!(
                command,
                Command::Store(StoreRequest::ShelfWrite { name, last, .. }) if *name == blob && *last
            )),
            "a finished book was not kept"
        );
    }

    #[test]
    fn opening_a_book_asks_where_it_was_left() {
        let feed = two_publication_feed();
        let mut runner = AppRunner::new(Gutenbird {
            stack: vec![StackEntry::fresh(feed, BASE.to_owned())],
            ..Gutenbird::default()
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
        let book = publication("Placed", vec![text_acquisition("https://x/placed.txt")]);
        let (_, place_key) = book_keys(&book).expect("keys");
        let mut runner = AppRunner::new(Gutenbird {
            view: View::Reading,
            open: Some(book),
            task: Some((TaskId(1), Awaiting::Book)),
            download: Some(Download {
                url: "https://x/placed.txt".to_owned(),
                kind: DownloadKind::Text,
                bytes: Vec::new(),
                total: None,
            }),
            ..Gutenbird::default()
        });
        let prose = "It is a truth universally acknowledged, that a single man in possession \
                     of a good fortune, must be in want of a wife.\n\n"
            .repeat(30);
        runner.task_outcome(TaskId(1), TaskOutcome::Completed(prose.into_bytes()));
        let place = Memory {
            at: 20,
            ..Memory::default()
        };
        runner.store_result(StoreResult::Loaded {
            key: place_key,
            value: Some(place.encode()),
        });
        let reader = runner.app_mut().reader.as_ref().expect("a book");
        assert!(
            reader.page().iter().any(|piece| piece.block == 20),
            "the reader was not put back where they were left"
        );
    }

    #[test]
    fn a_download_runs_on_the_books_own_page_not_a_bare_screen() {
        let book = publication(
            "On Its Own Page",
            vec![text_acquisition("https://x/own.txt")],
        );
        let mut runner = AppRunner::new(Gutenbird {
            view: View::Details,
            open: Some(book),
            ..Gutenbird::default()
        });
        runner.action(action_id("read"));
        assert_eq!(
            runner.app().view,
            View::Details,
            "reading left the book page before there was a book to show"
        );
        assert!(runner.app().awaiting_book(), "the download was not started");
    }

    // -----------------------------------------------------------------
    // Details page pagination (unchanged engine, now over a Publication)
    // -----------------------------------------------------------------

    #[test]
    fn a_book_with_a_long_summary_is_paged_rather_than_cut_off() {
        let long = "\"Pride and Prejudice\" by Jane Austen is a novel published in 1813. \
             It follows Elizabeth Bennet, who must learn to see past first impressions \
             and hasty judgments. With five daughters and an estate that can only pass \
             to male heirs, the Bennet family faces financial pressure to marry well. \
             When wealthy Mr. Darcy arrives in their countryside neighborhood, his pride \
             and Elizabeth's prejudice set the stage for misunderstandings, hidden \
             truths, and unexpected revelations about character and love. (This is an \
             automatically generated summary.)"
            .repeat(2);
        let mut book = publication(
            "Pride and Prejudice",
            vec![text_acquisition("https://x/pp.txt")],
        );
        book.summary = Some(long.clone());
        book.language = Some("en".to_owned());
        book.issued = Some("1813".to_owned());
        book.publisher = Some("A Publisher".to_owned());
        book.categories = vec![
            Category {
                term: "courtship".to_owned(),
                label: Some("Courtship -- Fiction".to_owned()),
                scheme: None,
            },
            Category {
                term: "domestic".to_owned(),
                label: Some("Domestic fiction".to_owned()),
                scheme: None,
            },
            Category {
                term: "england".to_owned(),
                label: Some("England -- Fiction".to_owned()),
                scheme: None,
            },
            Category {
                term: "love".to_owned(),
                label: Some("Love stories".to_owned()),
                scheme: None,
            },
            Category {
                term: "sisters".to_owned(),
                label: Some("Sisters -- Fiction".to_owned()),
                scheme: None,
            },
        ];
        let mut app = Gutenbird {
            view: View::Details,
            open: Some(book),
            complete: true,
            ..Gutenbird::default()
        };
        let context = kobo_sdk::Context::default();
        let publication = app.open.clone().unwrap();
        let blocks = Gutenbird::detail_blocks(&publication);
        let pages = app.detail_pagination(&context, &publication, &blocks);
        assert!(
            pages.len() > 1,
            "a summary this long should have cost a page turn"
        );
        assert_eq!(
            summary_words(pages.iter().flatten()),
            summary_words(blocks.iter()),
            "paging dropped part of the book"
        );

        for page in 0..pages.len() {
            app.detail_page = page;
            let screen = app.details_screen(&context);
            let errors: Vec<_> = screen
                .diagnostics(&context.metrics(), &Chrome::with_back(true))
                .issues
                .into_iter()
                .filter(|issue| issue.severity == DiagnosticSeverity::Error)
                .collect();
            assert!(errors.is_empty(), "page {page} does not fit: {errors:?}");
        }
    }

    fn summary_words<'a>(blocks: impl IntoIterator<Item = &'a DetailBlock>) -> Vec<String> {
        blocks
            .into_iter()
            .filter_map(|block| match block {
                DetailBlock::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .flat_map(str::split_whitespace)
            .map(str::to_owned)
            .collect()
    }

    /// A long list of categories is cut between two of them.
    ///
    /// Drawn as rows they take a finger-height band each, and Pride and
    /// Prejudice carries eleven. Placed whole -- which is what a block that
    /// fits nowhere used to get -- the last of them was drawn on the panel
    /// through the "3 of 4" beneath it.
    #[test]
    fn a_long_list_of_categories_is_divided_rather_than_drawn_over_the_page_position() {
        let mut book = publication(
            "Pride and Prejudice",
            vec![text_acquisition("https://x/pp")],
        );
        book.categories = [
            "England -- Fiction",
            "Young women -- Fiction",
            "Love stories",
            "Sisters -- Fiction",
            "Domestic fiction",
            "Courtship -- Fiction",
            "Social classes -- Fiction",
            "Language and Literatures: English literature",
            "Regency fiction",
            "Married people -- Fiction",
            "Class differences -- Fiction",
        ]
        .into_iter()
        .map(|label| Category {
            term: label.to_owned(),
            label: Some(label.to_owned()),
            scheme: None,
        })
        .collect();
        let mut app = Gutenbird {
            view: View::Details,
            open: Some(book),
            complete: true,
            ..Gutenbird::default()
        };
        let context = kobo_sdk::Context::default();
        let publication = app.open.clone().unwrap();
        let blocks = Gutenbird::detail_blocks(&publication);
        let pages = app.detail_pagination(&context, &publication, &blocks);

        let listed: Vec<String> = pages
            .iter()
            .flatten()
            .filter_map(|block| match block {
                DetailBlock::Categories(categories) => Some(categories.clone()),
                _ => None,
            })
            .flatten()
            .map(|category| category.term)
            .collect();
        assert_eq!(
            listed.len(),
            publication.categories.len(),
            "dividing the list lost categories"
        );

        for page in 0..pages.len() {
            app.detail_page = page;
            let errors: Vec<_> = app
                .details_screen(&context)
                .diagnostics(&context.metrics(), &Chrome::with_back(true))
                .issues
                .into_iter()
                .filter(|issue| issue.severity == DiagnosticSeverity::Error)
                .collect();
            assert!(errors.is_empty(), "page {page} does not fit: {errors:?}");
        }
    }

    #[test]
    fn a_summary_under_a_cover_is_divided_rather_than_drawn_off_the_panel() {
        let long = "This is the summary of a book, written by whoever catalogued it, at \
                    whatever length they felt the book deserved. "
            .repeat(12);
        let mut book = publication(
            "Moby Dick; Or, The Whale",
            vec![text_acquisition("https://x/moby.txt")],
        );
        book.summary = Some(long);
        let mut app = Gutenbird {
            view: View::Details,
            open: Some(book),
            complete: true,
            ..Gutenbird::default()
        };
        app.open_cover = Some(TilePicture::new(PictureHandle(0), 306, 484));
        let context = kobo_sdk::Context::default();
        let publication = app.open.clone().unwrap();
        let blocks = Gutenbird::detail_blocks(&publication);
        let pages = app.detail_pagination(&context, &publication, &blocks);
        assert!(pages.len() > 1, "a summary this long should have paged");
        assert!(
            !pages[0].is_empty(),
            "the first page emptied, so the second inherited the cover"
        );
        assert_eq!(
            summary_words(pages.iter().flatten()),
            summary_words(blocks.iter()),
            "dividing the summary lost words"
        );
        for page in 0..pages.len() {
            app.detail_page = page;
            let errors: Vec<_> = app
                .details_screen(&context)
                .diagnostics(&context.metrics(), &Chrome::measuring(true))
                .issues
                .into_iter()
                .filter(|issue| issue.severity == DiagnosticSeverity::Error)
                .collect();
            assert!(errors.is_empty(), "page {page} does not fit: {errors:?}");
        }
    }

    #[test]
    fn every_cover_on_a_shelf_page_is_drawn_whole_between_the_bars() {
        let publications: Vec<Publication> = (0..super::SHELF_PAGE)
            .map(|index| {
                publication(
                    &format!("Book {index}"),
                    vec![epub_acquisition(&format!("https://x/{index}.epub"))],
                )
            })
            .collect();
        let application = app_with_stack(Feed {
            publications,
            ..Feed::default()
        });
        let chrome = Chrome::with_back(true);
        let screen = application.shelf_screen(&kobo_sdk::Context::default());
        let layout = screen.layout_with(&CLARA_BW_METRICS, &chrome);
        let tiles = layout
            .nodes
            .iter()
            .filter(|node| matches!(node.kind, LayoutKind::Tile(..)))
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
        let issues = screen.validate(&CLARA_BW_METRICS);
        assert!(issues.is_empty(), "{issues:?}");
    }

    #[test]
    fn more_keeps_meaning_more_until_the_catalog_runs_out() {
        let page_one = Feed {
            links: vec![Link {
                rel: vec![Relation::Next],
                href: "https://example.org/catalog?page=2".to_owned(),
                media_type: None,
                title: None,
            }],
            publications: vec![publication(
                "One",
                vec![epub_acquisition("https://x/1.epub")],
            )],
            ..Feed::default()
        };
        let mut runner = AppRunner::new(app_with_stack(page_one));
        let commands = runner.action(action_id("shelf-next"));
        let asked = commands.iter().find_map(|command| match command {
            Command::Spawn {
                work: Task::Fetch { url, .. },
                ..
            } => Some(url.clone()),
            _ => None,
        });
        assert_eq!(asked.as_deref(), Some("https://example.org/catalog?page=2"));

        let task = runner.app().task.as_ref().map(|(id, _)| *id).unwrap();
        let page_two = r#"{"publications": [{"metadata": {"title": "Two"}, "links": [{"rel": "http://opds-spec.org/acquisition/open-access", "href": "https://x/2.epub", "type": "application/epub+zip"}]}]}"#;
        runner.task_outcome(task, TaskOutcome::Completed(page_two.as_bytes().to_vec()));
        let entry = runner.app().stack.last().unwrap();
        assert_eq!(
            entry.feed.publications.len(),
            2,
            "the second page did not add to the first"
        );
        assert_eq!(entry.feed.publications[0].title, "One");
        assert_eq!(entry.feed.publications[1].title, "Two");
        assert!(
            entry.next.is_none(),
            "a catalog with no next page still says there is more"
        );
    }

    // -----------------------------------------------------------------
    // Parity: the same catalog, twice
    // -----------------------------------------------------------------

    const PARITY_ATOM: &str = include_str!("../tests/fixtures/parity-1.2.xml");
    const PARITY_JSON: &str = include_str!("../tests/fixtures/parity-2.0.json");

    fn parity_app(source: &str) -> Gutenbird {
        let feed = kobo_opds::parse(source.as_bytes(), "https://parity.example/catalog")
            .expect("both fixtures parse");
        app_with_stack(feed)
    }

    #[test]
    fn the_shelf_drawn_from_a_1_2_catalog_and_a_2_0_catalog_reads_the_same() {
        let atom = parity_app(PARITY_ATOM);
        let json = parity_app(PARITY_JSON);
        let atom_text = screen_text(&atom.shelf_screen(&Context::default()));
        let json_text = screen_text(&json.shelf_screen(&Context::default()));
        assert_eq!(atom_text, json_text);
    }

    #[test]
    fn a_books_details_drawn_from_either_version_read_the_same() {
        let atom = parity_app(PARITY_ATOM);
        let json = parity_app(PARITY_JSON);
        let atom_book = Gutenbird {
            open: atom.stack[0].feed.publications.first().cloned(),
            complete: true,
            ..Gutenbird::default()
        };
        let json_book = Gutenbird {
            open: json.stack[0].feed.publications.first().cloned(),
            complete: true,
            ..Gutenbird::default()
        };
        let context = Context::default();
        assert_eq!(
            screen_text(&atom_book.details_screen(&context)),
            screen_text(&json_book.details_screen(&context))
        );
    }

    #[test]
    fn the_search_screen_offers_the_same_keyboard_whichever_version_supplied_the_template() {
        let atom_feed = kobo_opds::parse(PARITY_ATOM.as_bytes(), BASE).expect("parses");
        let json_feed = kobo_opds::parse(PARITY_JSON.as_bytes(), BASE).expect("parses");
        let atom_link = atom_feed.search().cloned().expect("a search link");
        let json_link = json_feed.search().cloned().expect("a search link");
        let atom_way = SearchWay::Direct(atom_link);
        let json_way = SearchWay::Direct(json_link);
        assert_eq!(atom_way.expand("dickens"), json_way.expand("dickens"));

        let mut atom_app = Gutenbird::default();
        atom_app.catalogs[0].search = SearchState::Known(atom_way);
        atom_app.view = View::Search;
        let mut json_app = Gutenbird::default();
        json_app.catalogs[0].search = SearchState::Known(json_way);
        json_app.view = View::Search;
        assert_eq!(
            screen_text(&atom_app.search_screen()),
            screen_text(&json_app.search_screen())
        );
    }

    #[test]
    fn no_screen_anywhere_names_the_version_it_is_talking_to() {
        for source in [PARITY_ATOM, PARITY_JSON] {
            let app = parity_app(source);
            let text = screen_text(&app.shelf_screen(&Context::default())).join(" ");
            assert!(
                !text.contains("OPDS"),
                "the shelf mentioned the protocol: {text}"
            );
            assert!(
                !text.contains("1.2") && !text.contains("2.0"),
                "the shelf named a version: {text}"
            );
        }
    }
    #[test]
    fn a_book_too_large_for_this_device_is_refused_before_it_is_downloaded() {
        // Gutenberg's illustrated Pride and Prejudice is twenty-four
        // megabytes, and taking it took the device down: a zip is parsed
        // whole, so the book, the blocks and the pictures are all in memory
        // at once on a reader with 448 MB shared with the firmware.
        let feed = Feed {
            publications: vec![
                publication(
                    "Pride and Prejudice",
                    vec![sized_epub(
                        "https://gutenberg.example/1342.epub3.images",
                        24_835_612,
                    )],
                ),
                publication(
                    "Pride and Prejudice",
                    vec![sized_epub(
                        "https://gutenberg.example/1342.epub.noimages",
                        558_547,
                    )],
                ),
            ],
            ..Feed::default()
        };
        let resolved = Gutenbird::resolve_entry(&feed).expect("collapses to one book");
        assert_eq!(
            resolved.acquisition[0].length,
            Some(558_547),
            "took the edition that crashed the reader"
        );
    }

    #[test]
    fn an_illustrated_edition_small_enough_to_read_is_still_preferred() {
        // The ceiling is not a preference for plainness. A book whose plates
        // fit is still the better book.
        let feed = Feed {
            publications: vec![
                publication(
                    "Emma",
                    vec![sized_epub(
                        "https://gutenberg.example/1.noimages.epub",
                        500_000,
                    )],
                ),
                publication(
                    "Emma",
                    vec![sized_epub(
                        "https://gutenberg.example/1.images.epub",
                        3_000_000,
                    )],
                ),
            ],
            ..Feed::default()
        };
        let resolved = Gutenbird::resolve_entry(&feed).expect("collapses to one book");
        assert!(resolved.acquisition[0].href.contains(".images"));
    }

    #[test]
    fn an_edition_that_states_no_size_is_still_offered() {
        // An unstated size is unknown rather than enormous, and most catalogs
        // decline to measure themselves. The bytes are still counted as they
        // arrive, which is the check that cannot be lied to.
        let feed = Feed {
            publications: vec![publication(
                "Emma",
                vec![epub_acquisition("https://catalog.example/emma.epub")],
            )],
            ..Feed::default()
        };
        assert!(Gutenbird::resolve_entry(&feed).is_some());
    }

    #[test]
    fn a_tile_keeps_its_place_while_its_picture_is_looked_for() {
        // The tile is drawn from what the entry already said and stays
        // exactly where it is. Moving books into a second collection gave the
        // page two paginations, drew every row whether it fitted or not, and
        // left everything past the first screenful unreachable on a panel
        // that does not scroll.
        let mut runner = AppRunner::new(Gutenbird::default());
        let feed = Feed {
            navigation: vec![
                navigation("Pride and Prejudice", "https://gutenberg.example/1342.opds"),
                navigation("Moby Dick", "https://gutenberg.example/2701.opds"),
            ],
            ..Feed::default()
        };
        runner.app_mut().stack = vec![StackEntry::fresh(
            feed,
            "https://gutenberg.example/".to_owned(),
        )];
        let mut context = runner.context();
        runner.app_mut().took_hydration(
            &mut context,
            ENTRY_DOCUMENT.as_bytes(),
            "https://gutenberg.example/1342.opds",
        );
        let entry = runner.app().stack.last().expect("a shelf");
        assert_eq!(entry.feed.navigation.len(), 2, "the tile left the grid");
        assert!(
            entry.feed.publications.is_empty(),
            "a second collection appeared"
        );
        assert_eq!(
            entry.nav_covers.len(),
            2,
            "no room was kept for the pictures"
        );
    }

    #[test]
    fn a_navigation_feed_is_paged_by_what_it_actually_holds() {
        // The page count came from the publications, so a feed of twenty
        // navigation entries and no publications said "1 of 1" and drew all
        // twenty.
        let feed = Feed {
            navigation: (0..20)
                .map(|n| navigation(&format!("Book {n}"), &format!("https://x/{n}.opds")))
                .collect(),
            ..Feed::default()
        };
        let entry = StackEntry::fresh(feed, "https://x/".to_owned());
        assert_eq!(
            super::shelf_pages(&entry),
            4,
            "twenty entries, six to a page"
        );
    }

    #[test]
    fn a_tile_that_is_a_list_rather_than_a_book_gets_no_picture() {
        // "Authors" and "Subjects" sit among the books in a Gutenberg search
        // answer. They stay tiles and keep their glyph, because there is
        // nothing in the address to tell them apart and following them is how
        // this finds out.
        let mut runner = AppRunner::new(Gutenbird::default());
        let feed = Feed {
            navigation: vec![navigation(
                "Authors",
                "https://gutenberg.example/authors.opds",
            )],
            ..Feed::default()
        };
        runner.app_mut().stack = vec![StackEntry::fresh(
            feed,
            "https://gutenberg.example/".to_owned(),
        )];
        let mut context = runner.context();
        runner.app_mut().took_hydration(
            &mut context,
            NAVIGATION_DOCUMENT.as_bytes(),
            "https://gutenberg.example/authors.opds",
        );
        let entry = runner.app().stack.last().expect("a shelf");
        assert_eq!(entry.feed.navigation.len(), 1);
        assert!(entry.nav_covers[0].is_none(), "a list was given a cover");
    }

    #[test]
    fn a_tap_outranks_the_shelf_filling_its_own_tiles() {
        // Filling tiles occupies a lane, and on a catalog whose entries are
        // slow it occupies one continuously. Every tap was then refused with
        // "too much is already in flight", so the application looked frozen
        // while it was busy on the reader's behalf and ignoring them.
        let mut runner = AppRunner::new(Gutenbird::default());
        runner.app_mut().filling.push((
            kobo_sdk::TaskId(77),
            FillStage::Entry {
                href: "https://gutenberg.example/1342.opds".to_owned(),
            },
        ));
        let mut context = runner.context();
        runner.app_mut().follow(
            &mut context,
            "https://gutenberg.example/latest.opds".to_owned(),
        );
        let awaiting = runner.app().task.clone();
        assert!(
            matches!(awaiting, Some((_, Awaiting::Feed(..)))),
            "the shelf's own work kept the lane: {awaiting:?}"
        );
        assert!(
            runner.app().filling.is_empty(),
            "a page the reader has left was still being filled in"
        );
    }

    #[test]
    fn the_shelf_does_not_fill_tiles_while_a_book_is_downloading() {
        // A book wants every lane there is, and a picture for a tile nobody
        // is looking at is not worth one of them.
        let mut runner = AppRunner::new(Gutenbird::default());
        runner.app_mut().view = View::Details;
        let feed = Feed {
            navigation: vec![navigation("A Book", "https://gutenberg.example/1.opds")],
            ..Feed::default()
        };
        runner.app_mut().stack = vec![StackEntry::fresh(
            feed,
            "https://gutenberg.example/".to_owned(),
        )];
        let mut context = runner.context();
        runner.app_mut().hydrate_visible(&mut context);
        assert!(runner.app().filling.is_empty(), "it went looking anyway");
    }

    /// One book, as a catalog's own entry document states it.
    const ENTRY_DOCUMENT: &str = r#"<?xml version="1.0"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Pride and Prejudice</title>
  <entry>
    <title>Pride and Prejudice</title>
    <author><name>Austen, Jane</name></author>
    <link rel="http://opds-spec.org/acquisition" type="application/epub+zip"
          href="https://gutenberg.example/1342.epub"/>
    <link rel="http://opds-spec.org/image" type="image/jpeg"
          href="https://gutenberg.example/1342.cover.jpg"/>
  </entry>
</feed>"#;

    /// A list of somewhere else to go, which is not a book.
    const NAVIGATION_DOCUMENT: &str = r#"<?xml version="1.0"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Authors</title>
  <entry>
    <title>Austen, Jane</title>
    <link rel="subsection" type="application/atom+xml;profile=opds-catalog"
          href="https://gutenberg.example/author/68.opds"/>
  </entry>
  <entry>
    <title>Dickens, Charles</title>
    <link rel="subsection" type="application/atom+xml;profile=opds-catalog"
          href="https://gutenberg.example/author/37.opds"/>
  </entry>
</feed>"#;
}

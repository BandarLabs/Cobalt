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
use kobo_bookview::offline::{Heard, Keeper, NO_ROOM};
use kobo_bookview::{BookView, Step};
use kobo_read::{Memory, Outcome};
use kobo_sdk::keyboard::{TextEntry, Typing};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Glyph, KoboApp, RowLead, Screen, ScreenBuilder,
    ShelfDownload, ShelfProgress, StoreResult, Task, TaskError, TaskId, TaskOutcome,
};
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::process::ExitCode;

/// The export API, which is the interface arXiv asks robots to use.
/// Where the Atom API lives, overridable so the simulator harness can point
/// the app at a local fixture.
fn api_base() -> String {
    std::env::var("ARXIV_API_BASE")
        .unwrap_or_else(|_| "https://export.arxiv.org/api/query".to_owned())
}

/// Where HTML renderings live, overridable for the same reason. A paper's
/// figures resolve against this origin, so the override carries them too.
fn html_base() -> String {
    std::env::var("ARXIV_HTML_BASE").unwrap_or_else(|_| "https://arxiv.org/html".to_owned())
}

/// How many papers one listing fetch asks for.
const PAGE: usize = 25;

/// The ceiling on a listing. Twenty-five abstracts is well under this; the
/// margin is for a query that matches papers with long author lists.
const LISTING_BYTES: u32 = 512 * 1024;

/// The most papers the library will hold.
///
/// An application may hold [`kobo_sdk::MAX_STORE_KEYS`] durable keys. The
/// catalogue, the index of what is on the shelf, the saved searches and the
/// followed subjects are one key each, and every paper read costs one more for
/// its reading position, kept or not; the paper itself and its figures are one
/// file on the shelf and cost no key at all. The ceiling is what is left once
/// those four and a margin for the reading positions of papers merely visited
/// are taken out.
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

/// The headings arXiv's subjects are offered under, in arXiv's own order.
///
/// arXiv files its hundred and fifty-odd categories under eight subjects, and
/// Physics under thirteen archives of its own. Physics is opened out here, one
/// heading per archive, because fifty-one categories behind one heading is six
/// screens of turning to find Optics, and an archive name like Astrophysics or
/// Condensed Matter is already the word a physicist is looking for.
const GROUPS: &[&str] = &[
    "Computer Science",
    "Economics",
    "Electrical Engineering and Systems Science",
    "Mathematics",
    "Astrophysics",
    "Condensed Matter",
    "General Relativity and Quantum Cosmology",
    "High Energy Physics",
    "Mathematical Physics",
    "Nonlinear Sciences",
    "Nuclear Physics",
    "Physics",
    "Quantum Physics",
    "Quantitative Biology",
    "Quantitative Finance",
    "Statistics",
];

/// Every arXiv category, with the heading it is offered under.
///
/// The whole of arXiv's taxonomy rather than a selection. Twenty hand-picked
/// subjects meant that somebody who reads Fluid Dynamics or Algebraic Geometry
/// found nothing for them and no way to ask, because a category
/// cannot be typed into the search box. Named exactly as arXiv names them, so
/// the identifier under each matches the one printed on a paper.
const SUBJECTS: &[(&str, &str, usize)] = &[
    ("cs.AI", "Artificial Intelligence", 0),
    ("cs.AR", "Hardware Architecture", 0),
    ("cs.CC", "Computational Complexity", 0),
    (
        "cs.CE",
        "Computational Engineering, Finance, and Science",
        0,
    ),
    ("cs.CG", "Computational Geometry", 0),
    ("cs.CL", "Computation and Language", 0),
    ("cs.CR", "Cryptography and Security", 0),
    ("cs.CV", "Computer Vision and Pattern Recognition", 0),
    ("cs.CY", "Computers and Society", 0),
    ("cs.DB", "Databases", 0),
    ("cs.DC", "Distributed, Parallel, and Cluster Computing", 0),
    ("cs.DL", "Digital Libraries", 0),
    ("cs.DM", "Discrete Mathematics", 0),
    ("cs.DS", "Data Structures and Algorithms", 0),
    ("cs.ET", "Emerging Technologies", 0),
    ("cs.FL", "Formal Languages and Automata Theory", 0),
    ("cs.GL", "General Literature", 0),
    ("cs.GR", "Graphics", 0),
    ("cs.GT", "Computer Science and Game Theory", 0),
    ("cs.HC", "Human-Computer Interaction", 0),
    ("cs.IR", "Information Retrieval", 0),
    ("cs.IT", "Information Theory", 0),
    ("cs.LG", "Machine Learning", 0),
    ("cs.LO", "Logic in Computer Science", 0),
    ("cs.MA", "Multiagent Systems", 0),
    ("cs.MM", "Multimedia", 0),
    ("cs.MS", "Mathematical Software", 0),
    ("cs.NA", "Numerical Analysis", 0),
    ("cs.NE", "Neural and Evolutionary Computing", 0),
    ("cs.NI", "Networking and Internet Architecture", 0),
    ("cs.OH", "Other Computer Science", 0),
    ("cs.OS", "Operating Systems", 0),
    ("cs.PF", "Performance", 0),
    ("cs.PL", "Programming Languages", 0),
    ("cs.RO", "Robotics", 0),
    ("cs.SC", "Symbolic Computation", 0),
    ("cs.SD", "Sound", 0),
    ("cs.SE", "Software Engineering", 0),
    ("cs.SI", "Social and Information Networks", 0),
    ("cs.SY", "Systems and Control", 0),
    ("econ.EM", "Econometrics", 1),
    ("econ.GN", "General Economics", 1),
    ("econ.TH", "Theoretical Economics", 1),
    ("eess.AS", "Audio and Speech Processing", 2),
    ("eess.IV", "Image and Video Processing", 2),
    ("eess.SP", "Signal Processing", 2),
    ("eess.SY", "Systems and Control", 2),
    ("math.AC", "Commutative Algebra", 3),
    ("math.AG", "Algebraic Geometry", 3),
    ("math.AP", "Analysis of PDEs", 3),
    ("math.AT", "Algebraic Topology", 3),
    ("math.CA", "Classical Analysis and ODEs", 3),
    ("math.CO", "Combinatorics", 3),
    ("math.CT", "Category Theory", 3),
    ("math.CV", "Complex Variables", 3),
    ("math.DG", "Differential Geometry", 3),
    ("math.DS", "Dynamical Systems", 3),
    ("math.FA", "Functional Analysis", 3),
    ("math.GM", "General Mathematics", 3),
    ("math.GN", "General Topology", 3),
    ("math.GR", "Group Theory", 3),
    ("math.GT", "Geometric Topology", 3),
    ("math.HO", "History and Overview", 3),
    ("math.IT", "Information Theory", 3),
    ("math.KT", "K-Theory and Homology", 3),
    ("math.LO", "Logic", 3),
    ("math.MG", "Metric Geometry", 3),
    ("math.MP", "Mathematical Physics", 3),
    ("math.NA", "Numerical Analysis", 3),
    ("math.NT", "Number Theory", 3),
    ("math.OA", "Operator Algebras", 3),
    ("math.OC", "Optimization and Control", 3),
    ("math.PR", "Probability", 3),
    ("math.QA", "Quantum Algebra", 3),
    ("math.RA", "Rings and Algebras", 3),
    ("math.RT", "Representation Theory", 3),
    ("math.SG", "Symplectic Geometry", 3),
    ("math.SP", "Spectral Theory", 3),
    ("math.ST", "Statistics Theory", 3),
    ("astro-ph.CO", "Cosmology and Nongalactic Astrophysics", 4),
    ("astro-ph.EP", "Earth and Planetary Astrophysics", 4),
    ("astro-ph.GA", "Astrophysics of Galaxies", 4),
    ("astro-ph.HE", "High Energy Astrophysical Phenomena", 4),
    (
        "astro-ph.IM",
        "Instrumentation and Methods for Astrophysics",
        4,
    ),
    ("astro-ph.SR", "Solar and Stellar Astrophysics", 4),
    (
        "cond-mat.dis-nn",
        "Disordered Systems and Neural Networks",
        5,
    ),
    ("cond-mat.mes-hall", "Mesoscale and Nanoscale Physics", 5),
    ("cond-mat.mtrl-sci", "Materials Science", 5),
    ("cond-mat.other", "Other Condensed Matter", 5),
    ("cond-mat.quant-gas", "Quantum Gases", 5),
    ("cond-mat.soft", "Soft Condensed Matter", 5),
    ("cond-mat.stat-mech", "Statistical Mechanics", 5),
    ("cond-mat.str-el", "Strongly Correlated Electrons", 5),
    ("cond-mat.supr-con", "Superconductivity", 5),
    ("gr-qc", "General Relativity and Quantum Cosmology", 6),
    ("hep-ex", "High Energy Physics - Experiment", 7),
    ("hep-lat", "High Energy Physics - Lattice", 7),
    ("hep-ph", "High Energy Physics - Phenomenology", 7),
    ("hep-th", "High Energy Physics - Theory", 7),
    ("math-ph", "Mathematical Physics", 8),
    ("nlin.AO", "Adaptation and Self-Organizing Systems", 9),
    ("nlin.CD", "Chaotic Dynamics", 9),
    ("nlin.CG", "Cellular Automata and Lattice Gases", 9),
    ("nlin.PS", "Pattern Formation and Solitons", 9),
    ("nlin.SI", "Exactly Solvable and Integrable Systems", 9),
    ("nucl-ex", "Nuclear Experiment", 10),
    ("nucl-th", "Nuclear Theory", 10),
    ("physics.acc-ph", "Accelerator Physics", 11),
    ("physics.ao-ph", "Atmospheric and Oceanic Physics", 11),
    ("physics.app-ph", "Applied Physics", 11),
    ("physics.atm-clus", "Atomic and Molecular Clusters", 11),
    ("physics.atom-ph", "Atomic Physics", 11),
    ("physics.bio-ph", "Biological Physics", 11),
    ("physics.chem-ph", "Chemical Physics", 11),
    ("physics.class-ph", "Classical Physics", 11),
    ("physics.comp-ph", "Computational Physics", 11),
    (
        "physics.data-an",
        "Data Analysis, Statistics and Probability",
        11,
    ),
    ("physics.ed-ph", "Physics Education", 11),
    ("physics.flu-dyn", "Fluid Dynamics", 11),
    ("physics.gen-ph", "General Physics", 11),
    ("physics.geo-ph", "Geophysics", 11),
    ("physics.hist-ph", "History and Philosophy of Physics", 11),
    ("physics.ins-det", "Instrumentation and Detectors", 11),
    ("physics.med-ph", "Medical Physics", 11),
    ("physics.optics", "Optics", 11),
    ("physics.plasm-ph", "Plasma Physics", 11),
    ("physics.pop-ph", "Popular Physics", 11),
    ("physics.soc-ph", "Physics and Society", 11),
    ("physics.space-ph", "Space Physics", 11),
    ("quant-ph", "Quantum Physics", 12),
    ("q-bio.BM", "Biomolecules", 13),
    ("q-bio.CB", "Cell Behavior", 13),
    ("q-bio.GN", "Genomics", 13),
    ("q-bio.MN", "Molecular Networks", 13),
    ("q-bio.NC", "Neurons and Cognition", 13),
    ("q-bio.OT", "Other Quantitative Biology", 13),
    ("q-bio.PE", "Populations and Evolution", 13),
    ("q-bio.QM", "Quantitative Methods", 13),
    ("q-bio.SC", "Subcellular Processes", 13),
    ("q-bio.TO", "Tissues and Organs", 13),
    ("q-fin.CP", "Computational Finance", 14),
    ("q-fin.EC", "Economics", 14),
    ("q-fin.GN", "General Finance", 14),
    ("q-fin.MF", "Mathematical Finance", 14),
    ("q-fin.PM", "Portfolio Management", 14),
    ("q-fin.PR", "Pricing of Securities", 14),
    ("q-fin.RM", "Risk Management", 14),
    ("q-fin.ST", "Statistical Finance", 14),
    ("q-fin.TR", "Trading and Market Microstructure", 14),
    ("stat.AP", "Applications", 15),
    ("stat.CO", "Computation", 15),
    ("stat.ME", "Methodology", 15),
    ("stat.ML", "Machine Learning", 15),
    ("stat.OT", "Other Statistics", 15),
    ("stat.TH", "Statistics Theory", 15),
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
    /// The categories under one heading of the subject list.
    Group,
    Search,
    Listing,
    Paper,
    FullText,
    /// The papers kept for reading without a network.
    Library,
    /// The searches saved and the subjects followed, so either can be run
    /// again without being typed or found a second time.
    Saved,
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
    /// How far through anybody has read, as a percentage of the paper's
    /// blocks, written down each time the reader saves a place. `None` means
    /// kept but never opened.
    progress: Option<u8>,
}

#[derive(Clone, Debug)]
enum AbstractBlock {
    Title(String),
    Fact(String),
    Body(String),
}

impl AbstractBlock {
    fn text(&self) -> &str {
        match self {
            Self::Title(text) | Self::Fact(text) | Self::Body(text) => text,
        }
    }
    fn with_text(&self, text: String) -> Self {
        match self {
            Self::Title(_) => Self::Title(text),
            Self::Fact(_) => Self::Fact(text),
            Self::Body(_) => Self::Body(text),
        }
    }
    fn add(&self, screen: ScreenBuilder) -> ScreenBuilder {
        match self {
            Self::Title(text) => screen.heading(text.clone()),
            Self::Fact(text) => screen.secondary(text.clone()),
            Self::Body(text) => screen.text(text.clone()),
        }
    }
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Default)]
struct Arxiv {
    view: View,
    /// The words of a search, typed on the reader's own keyboard.
    entry: TextEntry,
    /// The subject list's own page, because the headings and followed
    /// subjects together are more than one screen of rows.
    subject_page: usize,
    /// The heading whose categories are showing, or were showing when a
    /// listing was opened from it, so Back from that listing goes back to
    /// the heading rather than all the way to the start.
    group: Option<usize>,
    group_page: usize,
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
    /// The title, metadata and abstract, broken into complete panel pages.
    /// The paper itself is read through `book`.
    pages: Vec<Vec<AbstractBlock>>,
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
    /// Some formulas in the open paper were left as text.
    formulae_as_text: bool,
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
    /// Word searches the reader saved to run again, newest first.
    saved: Vec<String>,
    /// Subject codes the reader follows, pinned to the top of the subject
    /// list in the order they were followed.
    followed: Vec<String>,
    saved_page: usize,
    /// Whether the saved list is removing rather than running: Manage turns
    /// every row into the removal of itself, and Done turns them back.
    managing: bool,
    /// The rendering of the open paper, cut to the paper, held while it is on
    /// the panel so that keeping it does not mean fetching it a second time.
    ///
    /// Dropped with everything else when the paper closes: a stored copy is
    /// the point of keeping, and a copy of a paper nobody kept is a megabyte
    /// spent on nothing.
    fetched: Option<String>,
    /// Kept papers, each one file on the shelf holding the rendering and its
    /// figures.
    ///
    /// The figures used to be left behind. A kept paper was its markup alone,
    /// so on the train every plot was its caption, and online every figure was
    /// fetched again on every reading.
    keeper: Keeper,
    /// Papers whose file is still being written, figures and all.
    ///
    /// Listed in the library from the tap, so the control answers it, but not
    /// called offline until the device has acknowledged the file and the
    /// index naming it: a paper marked offline is one that opens on a train.
    pending: BTreeSet<String>,
    /// Papers kept before figures were, being rewritten with them. Their old
    /// rendering stays on the shelf until the new file is acknowledged.
    upgrading: BTreeSet<String>,
    /// The kept paper being read back from the shelf, by identifier.
    opening: Option<String>,
    /// A rendering kept before figures were, on its way off the shelf.
    loading: Option<ShelfDownload>,
    /// Whether the rendering being read came from that older kind of file,
    /// which is the moment to rewrite it with its figures.
    from_legacy: bool,
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

/// The store keys the saved searches and the followed subjects are written
/// under.
const SEARCHES_KEY: &str = "searches";
const FOLLOWED_KEY: &str = "followed";

/// How many saved searches and followed subjects each list holds. A saved
/// list is a shortcut, not an archive: past a screenful or two of rows the
/// answer to "which one was it" is the search box, not more scrolling.
const MAX_SAVED: usize = 24;

/// Writes a list of short strings out, one to a line.
///
/// The same shape as the library catalogue, for the same reasons: readable
/// over the shell when somebody reports a lost entry, and a line that cannot
/// be understood costs one entry rather than the whole list. Tabs are
/// scrubbed on the way in, so nothing can forge a second field or a second
/// line.
fn encode_list(list: &[String]) -> Vec<u8> {
    let mut text = String::new();
    for entry in list.iter().take(MAX_SAVED) {
        let _ = writeln!(text, "{}", untabbed(entry));
    }
    text.into_bytes()
}

fn decode_list(bytes: &[u8]) -> Vec<String> {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Vec::new();
    };
    text.lines()
        .take(MAX_SAVED)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

/// What a kept paper's row says under its title.
fn kept_summary(kept: &Kept) -> String {
    // Rounded the way the library's line about everything kept is, so one
    // paper on its own does not read as a different size in the row and in
    // the total above it.
    let size = kobo_bookview::offline::size(u64::from(kept.bytes));
    let facts = if kept.authors.is_empty() {
        format!("{} \u{b7} {size}", kept.id)
    } else {
        format!("{} \u{b7} {} \u{b7} {size}", kept.id, kept.authors)
    };
    // Progress leads, for the same reason the offline badge does: the
    // row clamps to one line, and the tail is what the clamp eats.
    // "How far through am I" is the fact a library row exists to show.
    if let Some(progress) = kept.progress.filter(|progress| *progress > 0) {
        format!("{progress}% \u{b7} {facts}")
    } else {
        facts
    }
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
            "{}\t{}\t{}\t{}\t{}",
            untabbed(&kept.id),
            kept.bytes,
            untabbed(&kept.title),
            untabbed(&kept.authors),
            kept.progress
                .map(|progress| progress.to_string())
                .unwrap_or_default()
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
        let authors = fields.next().unwrap_or_default();
        let progress = fields
            .next()
            .filter(|field| !field.is_empty())
            .and_then(|field| field.parse().ok());
        library.push(Kept {
            id: id.to_owned(),
            title: title.to_owned(),
            authors: authors.to_owned(),
            bytes: bytes.parse().unwrap_or(0),
            progress,
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

/// The catalogue indices of every category under one heading, in arXiv's
/// order.
fn group_subjects(group: usize) -> impl Iterator<Item = usize> {
    SUBJECTS
        .iter()
        .enumerate()
        .filter_map(move |(index, (_, _, under))| (*under == group).then_some(index))
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
const GROUP: &str = "group-";
const GROUP_BACK: &str = "group-back";
const GROUP_NEXT: &str = "group-next";
const PAPER: &str = "paper-";
const LIBRARY: &str = "library";
const LIB_BACK: &str = "library-back";
const LIB_NEXT: &str = "library-next";
const KEEP: &str = "keep";
const DISCARD: &str = "discard";
const KEPT: &str = "kept-";
const WINDOW: &str = "window";
const SAVED: &str = "saved";
const MANAGE: &str = "manage";
const DONE: &str = "done";
const SAVE_SEARCH: &str = "save-search";
const FOLLOW: &str = "follow";
const UNFOLLOW: &str = "unfollow";
const SSEARCH: &str = "ssearch-";
const FROW: &str = "frow-";
const SAVED_BACK: &str = "saved-back";
const SAVED_NEXT: &str = "saved-next";
const RETRY_KEPT: &str = "retry-kept";
const RETRY_LISTING: &str = "retry-listing";

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
            "{}?search_query={}&start={offset}&max_results={PAGE}\
             &sortBy=submittedDate&sortOrder=descending",
            api_base(),
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
            None => self.trouble = Some("Another arXiv search is still loading.".to_owned()),
        }
    }

    /// Asks for arXiv's HTML rendering of the open paper.
    fn ask_full_text(&mut self, context: &mut Context) {
        let Some(paper) = self.paper() else {
            return;
        };
        let url = format!("{}/{}", html_base(), escape_path(&paper.id));
        // Asked for now rather than when the rendering lands, so that the
        // place is already in hand by the time there is a document to put
        // it into. The store is on the same machine and the paper is at the
        // other end of the internet, so the race is not close.
        let id = paper.id.clone();
        self.ask_place(context, &id);
        self.trouble = None;
        self.truncated = false;
        self.formulae_as_text = false;
        match context.spawn_retrying(Task::Fetch {
            url,
            offset: 0,
            max_bytes: FULL_TEXT_BYTES,
            credential: None,
            headers: Vec::new(),
        }) {
            Some(task) => self.task = Some((task, Awaiting::FullText)),
            None => self.trouble = Some("This paper is already loading.".to_owned()),
        }
    }

    /// Opens the abstract with its title and facts kept in their own type levels.
    fn open_abstract(&mut self, context: &Context) {
        self.page = 0;
        self.truncated = false;
        self.formulae_as_text = false;
        self.reflow_abstract(context);
    }

    fn reflow_abstract(&mut self, context: &Context) {
        let Some(paper) = self.paper() else { return };
        let blocks = std::iter::once(AbstractBlock::Title(paper.title.clone()))
            .chain(fact_lines(paper).into_iter().map(AbstractBlock::Fact))
            .chain(
                paper
                    .summary
                    .split("\n\n")
                    .filter(|text| !text.trim().is_empty())
                    .map(|text| AbstractBlock::Body(text.to_owned())),
            )
            .collect::<Vec<_>>();
        let mut queue = std::collections::VecDeque::from(blocks);
        let mut pages = Vec::new();
        let mut current = Vec::new();
        while let Some(block) = queue.pop_front() {
            let mut candidate = current.clone();
            candidate.push(block.clone());
            if self.abstract_fits(context, &candidate) {
                current = candidate;
                continue;
            }
            // Keep facts whole when possible. Long paragraphs use the space
            // left on this page; a title or fact that fills a page can split too.
            if current.is_empty() || matches!(block, AbstractBlock::Body(_)) {
                if let Some((head, tail)) = self.split_abstract_block(context, &current, &block) {
                    current.push(head);
                    pages.push(std::mem::take(&mut current));
                    queue.push_front(tail);
                    continue;
                }
            }
            if current.is_empty() {
                current.push(block);
            } else {
                pages.push(std::mem::take(&mut current));
                queue.push_front(block);
            }
        }
        if !current.is_empty() {
            pages.push(current);
        }
        self.pages = pages;
        self.page = self.page.min(self.pages.len().saturating_sub(1));
    }

    fn abstract_fits(&self, context: &Context, blocks: &[AbstractBlock]) -> bool {
        let mut screen = self.abstract_prefix();
        for block in blocks {
            screen = block.add(screen);
        }
        screen
            .bottom_action_marked(FULL_TEXT, "Full text", Glyph::Book)
            .page_turns(READ_BACK, READ_NEXT)
            .page_position(1, 2)
            .build()
            .diagnostics(&context.metrics(), &kobo_sdk::Chrome::measuring(true))
            .issues
            .iter()
            .all(|issue| issue.severity != kobo_sdk::DiagnosticSeverity::Error)
    }

    fn split_abstract_block(
        &self,
        context: &Context,
        current: &[AbstractBlock],
        block: &AbstractBlock,
    ) -> Option<(AbstractBlock, AbstractBlock)> {
        let text = block.text();
        let words = text
            .char_indices()
            .filter_map(|(index, ch)| (index > 0 && ch.is_whitespace()).then_some(index))
            .collect::<Vec<_>>();
        let mut split = self.fitting_abstract_prefix(context, current, block, &words);
        if split == 0 {
            let characters = text
                .char_indices()
                .skip(1)
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            split = self.fitting_abstract_prefix(context, current, block, &characters);
        }
        if split == 0 {
            return None;
        }
        Some((
            block.with_text(text[..split].trim_end().to_owned()),
            block.with_text(text[split..].trim_start().to_owned()),
        ))
    }

    fn fitting_abstract_prefix(
        &self,
        context: &Context,
        current: &[AbstractBlock],
        block: &AbstractBlock,
        boundaries: &[usize],
    ) -> usize {
        let (mut low, mut high) = (0, boundaries.len());
        while low < high {
            let middle = (low + high).div_ceil(2);
            let mut candidate = current.to_vec();
            candidate.push(
                block.with_text(block.text()[..boundaries[middle - 1]].trim_end().to_owned()),
            );
            if self.abstract_fits(context, &candidate) {
                low = middle;
            } else {
                high = middle - 1;
            }
        }
        low.checked_sub(1).map_or(0, |index| boundaries[index])
    }

    fn abstract_prefix(&self) -> ScreenBuilder {
        // The title rather than the identifier. "2401.00001v2" says nothing to
        // somebody who put the reader down mid-paper and picked it up again,
        // and the bar ends a long title with an ellipsis on its own.
        let mut screen = ScreenBuilder::new("arxiv-paper")
            .top_bar(self.paper().map_or("", |paper| paper.title.as_str()));
        if let Some(trouble) = &self.trouble {
            screen = screen.banner(BannerLevel::Attention, trouble.clone());
        }
        if self.truncated {
            screen = screen.banner(
                BannerLevel::Attention,
                "This paper is too long to open completely, so only the beginning is shown.",
            );
        } else if self.formulae_as_text {
            screen=screen.banner(BannerLevel::Info,"Some formulas in the full text are shown as text. This paper has more mathematics than the reader can draw.");
        }
        screen
    }

    fn subjects(&self, context: &Context) -> Screen {
        let mut screen = ScreenBuilder::new("arxiv-subjects").top_bar("Preprints");
        if let Some(trouble) = &self.trouble {
            screen = screen.banner(BannerLevel::Attention, trouble.clone());
        }
        // Followed subjects lead, in the order they were followed, and open
        // their listing in one tap; arXiv's headings follow them. The row ids
        // name the catalogue index, so the action that opens a subject does
        // not care where the row stood.
        let followed: Vec<usize> = self
            .followed
            .iter()
            .filter_map(|code| SUBJECTS.iter().position(|(known, _, _)| known == code))
            .collect();
        let mut rows: Vec<(String, String, String, Glyph)> = followed
            .iter()
            .map(|index| {
                let (code, name, _) = SUBJECTS[*index];
                (
                    format!("{SUBJECT}{index}"),
                    context.one_line_row(name, false),
                    code.to_owned(),
                    Glyph::Heart,
                )
            })
            .collect();
        rows.extend(GROUPS.iter().enumerate().map(|(group, name)| {
            let inside: Vec<usize> = group_subjects(group).collect();
            // A heading with one category in it is that category, so its row
            // opens the listing and says which category it is, rather than
            // leading to a list of one.
            let detail = match inside.as_slice() {
                [only] => SUBJECTS[*only].0.to_owned(),
                _ => format!("{} subjects", inside.len()),
            };
            (
                format!("{GROUP}{group}"),
                context.one_line_row(name, false),
                detail,
                Glyph::Folder,
            )
        }));
        let borrowed: Vec<(&str, &str)> = rows
            .iter()
            .map(|(_, title, detail, _)| (title.as_str(), detail.as_str()))
            .collect();
        let pages = context.paginate_rows_under(
            &borrowed,
            true,
            kobo_sdk::Position::AtTheFoot,
            &screen.clone().build(),
        );
        let page = self.subject_page.min(pages.len().saturating_sub(1));
        let shown = pages.get(page).map(Vec::as_slice).unwrap_or_default();
        screen
            .rows(shown.iter().filter_map(|position| {
                let (id, title, detail, glyph) = rows.get(*position)?;
                Some((
                    id.clone(),
                    title.clone(),
                    detail.clone(),
                    RowLead::Icon(*glyph),
                ))
            }))
            .top_bar_glyph(LIBRARY, "Library", Glyph::Bookmark)
            .top_bar_glyph(SAVED, "Saved", Glyph::Heart)
            .page_turns(SUBJECTS_BACK, SUBJECTS_NEXT)
            .page_position(page_number(page), page_total(pages.len()))
            .bottom_action_marked(SEARCH, "Search arXiv", Glyph::Search)
            .build()
    }

    /// The categories under one of arXiv's headings.
    fn group_screen(&self, context: &Context) -> Screen {
        let group = self.group.unwrap_or_default();
        let title = GROUPS.get(group).copied().unwrap_or("Preprints");
        let mut screen = ScreenBuilder::new("arxiv-group").top_bar(title);
        if let Some(trouble) = &self.trouble {
            screen = screen.banner(BannerLevel::Attention, trouble.clone());
        }
        let inside: Vec<usize> = group_subjects(group).collect();
        let rows: Vec<(String, &str)> = inside
            .iter()
            .map(|index| {
                let (code, name, _) = SUBJECTS[*index];
                (context.one_line_row(name, false), code)
            })
            .collect();
        let borrowed: Vec<(&str, &str)> = rows
            .iter()
            .map(|(title, code)| (title.as_str(), *code))
            .collect();
        let pages = context.paginate_rows_under(
            &borrowed,
            true,
            kobo_sdk::Position::AtTheFoot,
            &screen.clone().build(),
        );
        let page = self.group_page.min(pages.len().saturating_sub(1));
        let shown = pages.get(page).map(Vec::as_slice).unwrap_or_default();
        screen
            .rows(shown.iter().filter_map(|position| {
                let index = *inside.get(*position)?;
                let (title, code) = rows.get(*position)?;
                let (known, _, _) = SUBJECTS[index];
                let lead = if self.followed.iter().any(|followed| followed == known) {
                    Glyph::Heart
                } else {
                    Glyph::Note
                };
                Some((
                    format!("{SUBJECT}{index}"),
                    title.clone(),
                    (*code).to_owned(),
                    RowLead::Icon(lead),
                ))
            }))
            .page_turns(GROUP_BACK, GROUP_NEXT)
            .page_position(page_number(page), page_total(pages.len()))
            .build()
    }

    /// The search field, which the runtime draws over this screen.
    ///
    /// The runtime keeps this screen's title in its top bar and puts the
    /// prompt under it, so the title names the application and the prompt
    /// says what is being asked, rather than both saying "Search arXiv".
    fn search(&self) -> Screen {
        ScreenBuilder::new("arxiv-search")
            .top_bar("Preprints")
            .text_entry_with_hint(
                &self.entry,
                "Search arXiv",
                "A phrase, an author, a title",
                "Search",
            )
            .build()
    }

    /// The papers kept for reading with no network.
    ///
    /// Reachable from the subject list rather than from a paper, because the
    /// question "what have I kept?" is asked on the way in, before there is
    /// any paper open to ask it from.
    fn library(&self, context: &Context) -> Screen {
        let mut screen = ScreenBuilder::new("arxiv-library").top_bar("Library");
        if self.keeper.failed {
            // The way out of the failure sits on the banner that names it,
            // where it used to be a top bar mark the words had to point at.
            screen = screen
                .banner(
                    BannerLevel::Error,
                    "The list of papers kept on this reader could not be read or saved.",
                )
                .banner_action(RETRY_KEPT, "Try again");
        }
        if let Some(trouble) = &self.trouble {
            screen = screen.banner(BannerLevel::Attention, trouble.clone());
        }
        if self.library.is_empty() {
            return screen
                .splash(
                    Some(Glyph::Bookmark),
                    "No papers kept yet",
                    "Open a paper and tap Keep for offline. It waits here with its \
                     figures, ready to read without a connection.",
                )
                .build();
        }
        // What is on the reader, said the way every reading application on
        // the device says it, above the papers it counts.
        screen = screen.secondary(self.ready_offline());
        // Clamped here rather than left to the renderer: a real title runs
        // to two dozen words, and a row's text that does not fit is a screen
        // the renderer refuses outright. Two lines for the title, one for
        // the facts, measured against the same width the layout uses.
        let rows: Vec<(String, String)> = self
            .library
            .iter()
            .map(|kept| {
                // Still being written says so first, where the one-line clamp
                // cannot eat it: a paper that is not on the reader yet must
                // not look like one that is.
                let summary = if self.pending.contains(&kept.id) {
                    format!("Downloading for offline \u{b7} {}", kept_summary(kept))
                } else {
                    kept_summary(kept)
                };
                (
                    context.clamped_row(&kept.title, 2, false),
                    context.one_line_row(&summary, false),
                )
            })
            .collect();
        let borrowed: Vec<(&str, &str)> = rows
            .iter()
            .map(|(title, summary)| (title.as_str(), summary.as_str()))
            .collect();
        let pages = context.paginate_rows_under(
            &borrowed,
            false,
            kobo_sdk::Position::AtTheFoot,
            &screen.clone().build(),
        );
        let page = self.library_page.min(pages.len().saturating_sub(1));
        let shown = pages.get(page).map(Vec::as_slice).unwrap_or_default();
        screen = screen.rows(shown.iter().filter_map(|index| {
            self.library
                .get(*index)
                .zip(rows.get(*index))
                .map(|(_kept, (title, summary))| {
                    (
                        format!("{KEPT}{index}"),
                        title.clone(),
                        summary.clone(),
                        RowLead::Icon(Glyph::Bookmark),
                    )
                })
        }));
        screen
            .page_turns(LIB_BACK, LIB_NEXT)
            .page_position(page_number(page), page_total(pages.len()))
            .build()
    }

    /// The searches saved and the subjects followed, in one list.
    ///
    /// Reachable from the subject list beside the library, because both
    /// answer "where are the things I set aside" before any listing exists
    /// to ask it from.
    fn saved(&self, context: &Context) -> Screen {
        let mut screen = ScreenBuilder::new("arxiv-saved").top_bar("Saved");
        if let Some(trouble) = &self.trouble {
            screen = screen.banner(BannerLevel::Attention, trouble.clone());
        }
        if self.saved.is_empty() && self.followed.is_empty() {
            return screen
                .splash(
                    Some(Glyph::Heart),
                    "Nothing saved",
                    "Save a search or follow a subject to see it here.",
                )
                .build();
        }
        // Searches first: they are the more specific shortcut. A search
        // phrase is free text, so the rows are clamped here for the usual
        // reason - a row that does not fit is a refused screen.
        let rows: Vec<(String, String)> = self
            .saved
            .iter()
            .map(|words| {
                (
                    context.clamped_row(&format!("\u{201c}{words}\u{201d}"), 2, false),
                    "Saved search".to_owned(),
                )
            })
            .chain(self.followed.iter().filter_map(|code| {
                SUBJECTS
                    .iter()
                    .find(|(known, _, _)| known == code)
                    .map(|(code, name, _)| (context.one_line_row(name, false), (*code).to_owned()))
            }))
            .collect();
        let borrowed: Vec<(&str, &str)> = rows
            .iter()
            .map(|(title, summary)| (title.as_str(), summary.as_str()))
            .collect();
        let pages = context.paginate_rows_under(
            &borrowed,
            true,
            kobo_sdk::Position::AtTheFoot,
            &screen.clone().build(),
        );
        let page = self.saved_page.min(pages.len().saturating_sub(1));
        let shown = pages.get(page).map(Vec::as_slice).unwrap_or_default();
        let searches = self.saved.len();
        screen = screen.rows(shown.iter().filter_map(|position| {
            let (title, summary) = rows.get(*position)?;
            let (id, lead) = if *position < searches {
                (format!("{SSEARCH}{position}"), Glyph::Search)
            } else {
                (format!("{FROW}{}", position - searches), Glyph::Heart)
            };
            let lead = if self.managing { Glyph::Trash } else { lead };
            Some((id, title.clone(), summary.clone(), RowLead::Icon(lead)))
        }));
        // Manage turns every row into the removal of itself. There is no
        // confirm: a removal costs one tap to undo, because saving and
        // following are one tap each to redo.
        let managing = if self.managing {
            (DONE, "Done", Glyph::Check)
        } else {
            (MANAGE, "Manage", Glyph::Settings)
        };
        screen
            .page_turns(SAVED_BACK, SAVED_NEXT)
            .page_position(page_number(page), page_total(pages.len()))
            .bottom_action_marked(managing.0, managing.1, managing.2)
            .build()
    }

    fn listing(&self, context: &Context) -> Screen {
        let title = self
            .query
            .as_ref()
            .map_or_else(|| "Preprints".to_owned(), Query::title);
        let mut screen = ScreenBuilder::new("arxiv-listing").top_bar(title);
        let failed = self.papers.is_empty() && self.trouble.is_some();
        if let Some(trouble) = &self.trouble {
            let level = if failed {
                BannerLevel::Error
            } else {
                BannerLevel::Attention
            };
            screen = screen.banner(level, trouble.clone());
        }
        if self.waiting_for(Awaiting::Listing) {
            return placeholder(context, &screen);
        }
        if failed {
            // The one thing that can change a listing that never arrived goes
            // on the banner saying why, rather than in a button of its own
            // under it.
            screen = screen.banner_action(RETRY_LISTING, "Try again");
        }
        // The window control carries its own state as its label, so the
        // one button is both the way to change the reach of the listing and
        // the only place that says what the reach currently is.
        let narrowing = (WINDOW, self.window.label(), Glyph::Filter);
        if failed {
            // A listing that never arrived is not a listing with nothing in
            // it. The banner says why and carries the retry, rather than
            // advice about search words that were never the problem.
            return screen
                .bottom_action_marked(narrowing.0, narrowing.1, narrowing.2)
                .build();
        }
        if self.papers.is_empty() {
            return screen
                .splash(
                    Some(Glyph::Search),
                    "No papers found",
                    "Change the date range or search words.",
                )
                .bottom_action_marked(narrowing.0, narrowing.1, narrowing.2)
                .build();
        }
        // Clamped for the same reason the library's rows are: live titles
        // and bylines are far longer than anything a fixture needs, and an
        // overflowing row is a refused screen.
        let rows: Vec<(String, String)> = self
            .papers
            .iter()
            .map(|paper| {
                (
                    context.clamped_row(&paper.title, 2, false),
                    context.one_line_row(&self.listing_summary(paper), false),
                )
            })
            .collect();
        let borrowed: Vec<(&str, &str)> = rows
            .iter()
            .map(|(title, summary)| (title.as_str(), summary.as_str()))
            .collect();
        let pages = context.paginate_ranked_rows_under(
            &borrowed,
            true,
            u16::try_from(self.offset + self.papers.len()).unwrap_or(u16::MAX),
            kobo_sdk::Position::AtTheFoot,
            &screen.clone().build(),
        );
        let page = self.listing_page.min(pages.len().saturating_sub(1));
        let shown = pages.get(page).map(Vec::as_slice).unwrap_or_default();
        screen = screen.rows(shown.iter().filter_map(|index| {
            self.papers
                .get(*index)
                .zip(rows.get(*index))
                .map(|(_paper, (title, summary))| {
                    (
                        format!("{PAPER}{index}"),
                        title.clone(),
                        summary.clone(),
                        RowLead::Number(u16::try_from(self.offset + index + 1).unwrap_or(u16::MAX)),
                    )
                })
        }));
        // Offered only on the last page, and only when there is more behind
        // it. Anywhere else it is a control that fetches something the reader
        // has not finished looking at. It goes in the top bar: the bottom
        // band holds one control, and that one is the window the listing is
        // narrowed to.
        let more_behind = self.offset + self.papers.len() < self.total as usize;
        if page + 1 == pages.len() && more_behind {
            screen = screen.top_bar_action(MORE, "Older papers");
        }
        // Saving and following are offered where the thing they keep is on
        // screen: "run this again" is a fact about the listing behind the
        // rows, and the listing is the only place that says what it is. The
        // bar holds two actions and "Older papers" can be one, so this is
        // the other.
        match &self.query {
            Some(Query::Words(words)) if !self.saved.contains(words) => {
                screen = screen.top_bar_glyph(SAVE_SEARCH, "Save this search", Glyph::Bookmark);
            }
            Some(Query::Subject { code, .. }) => {
                screen = if self.followed.contains(code) {
                    screen.top_bar_glyph(UNFOLLOW, "Stop following", Glyph::Check)
                } else {
                    screen.top_bar_glyph(FOLLOW, "Follow this subject", Glyph::Heart)
                };
            }
            _ => {}
        }
        screen
            .bottom_action_marked(narrowing.0, narrowing.1, narrowing.2)
            .page_turns(LIST_BACK, LIST_NEXT)
            .page_position(page_number(page), page_total(pages.len()))
            .build()
    }

    fn reading(&self) -> Screen {
        if self.paper().is_none() {
            return ScreenBuilder::new("arxiv-paper")
                .top_bar("Preprints")
                .build();
        }
        let mut screen = self.abstract_prefix();
        if self.waiting_for(Awaiting::FullText) {
            return screen.activity("Fetching the full text", None).build();
        }
        if self.loading.is_some() || self.opening.is_some() {
            return screen.activity("Opening the kept paper", None).build();
        }
        let page = self.page.min(self.pages.len().saturating_sub(1));
        for block in self.pages.get(page).into_iter().flatten() {
            screen = block.add(screen);
        }
        // Keeping is offered from the paper rather than from the reader,
        // because the reader's bar belongs to reading and every application
        // sharing it has the same one. Whether this paper is kept is a fact
        // about this application's library, not about the page. It sits in
        // the top bar: the bottom band holds one control, and that one is
        // the way into the full text.
        let kept = self.paper().is_some_and(|paper| self.is_kept(&paper.id));
        screen = screen.fill();
        screen = if kept {
            screen.top_bar_glyph(DISCARD, "Remove from library", Glyph::Trash)
        } else {
            screen.top_bar_glyph(KEEP, "Keep for offline", Glyph::Download)
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
            .map_or_else(|| "Preprints".to_owned(), |paper| paper.title.clone());
        self.book.screen(&title).unwrap_or_else(|| self.reading())
    }

    fn waiting_for(&self, what: Awaiting) -> bool {
        self.task.is_some_and(|(_, awaiting)| awaiting == what)
    }

    fn show(&mut self, context: &mut Context) {
        if self.view == View::Paper
            && !self.waiting_for(Awaiting::FullText)
            && self.loading.is_none()
            && self.opening.is_none()
        {
            self.reflow_abstract(context);
        }
        let screen = match self.view {
            View::Subjects => self.subjects(context),
            View::Group => self.group_screen(context),
            View::Search => self.search(),
            View::Listing => self.listing(context),
            View::Paper => self.reading(),
            View::FullText => self.full_text(),
            View::Library => self.library(context),
            View::Saved => self.saved(context),
        };
        // Every view but the subject list was reached from another one, so
        // Back has somewhere to go from all of them and nowhere to go from it.
        let screen = screen.with_own_back(self.view != View::Subjects);
        context.set_screen(screen);
    }

    /// Turns a page of whatever list the view is showing.
    fn turn(&mut self, context: &mut Context, forward: bool) {
        let screen = match self.view {
            View::Subjects => self.subjects(context),
            View::Group => self.group_screen(context),
            View::Listing => self.listing(context),
            View::Library => self.library(context),
            View::Saved => self.saved(context),
            View::Paper => self.reading(),
            View::FullText | View::Search => return,
        };
        let last = screen
            .page_turns
            .and_then(|turns| turns.position)
            .map_or(0, |(_, total)| usize::from(total).saturating_sub(1));
        let page = match self.view {
            View::Subjects => &mut self.subject_page,
            View::Group => &mut self.group_page,
            View::Listing => &mut self.listing_page,
            View::Library => &mut self.library_page,
            View::Saved => &mut self.saved_page,
            View::Paper => &mut self.page,
            View::FullText | View::Search => return,
        };
        *page = if forward {
            (*page).min(last).saturating_add(1).min(last)
        } else {
            (*page).min(last).saturating_sub(1)
        };
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
        let legacy = std::mem::take(&mut self.from_legacy);
        // Cut by the same rule every reader of web pages on this device uses,
        // which for arXiv is the `<article>` LaTeXML wraps the paper in.
        // Whether the paper arrived whole is still read from that tag: a
        // rendering cut off at the byte ceiling has no closing one.
        let body = kobo_html::article::main_content(html);
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
        let origin = format!("{}/{}", html_base(), escape_path(&paper.id));
        // Whatever this paper was left at, if it has been read before. The
        // load was asked for when the paper was opened, so by the time the
        // rendering is in hand the answer is usually already here; a paper
        // that outran it opens at the top, which is where it would have
        // opened anyway.
        let memory = self.place.take().unwrap_or_default();
        if !self.book.open_html(context, &body, &origin, memory) {
            self.formulae_as_text = false;
            self.trouble =
                Some("arXiv has no readable rendering of this paper, only a PDF.".to_owned());
            return;
        }
        self.formulae_as_text = self.book.formulae_as_text();
        // A rendering cut off at the byte ceiling has no closing tag, which is
        // the honest signal: the fetched bytes are markup, and a paper can sit
        // at the transport limit with every word of it delivered.
        self.truncated = !paper_body(html).contains("</article>");
        self.book.mark_truncated(self.truncated);
        self.page = 0;
        self.view = View::FullText;
        // Held so that keeping the paper does not fetch it a second time.
        // Only worth holding what could actually be kept: a rendering that
        // arrived truncated is not a paper, and storing one would make the
        // library quietly full of halves.
        self.fetched = if self.truncated { None } else { Some(body) };
        // A paper kept before figures were is rewritten with them the first
        // time it is read, which is the one moment its rendering is already
        // in hand. Online, the figures come with it; offline, the new file is
        // no worse than the old one.
        if legacy && !self.truncated && self.keep_on_device(context, &paper.id) {
            self.upgrading.insert(paper.id.clone());
        }
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

    /// A listing row's second line: the facts that place the paper, and
    /// whether it is already on the shelf for reading without a network.
    fn listing_summary(&self, paper: &Paper) -> String {
        // The badge leads so that clamping a live-length byline to
        // one line can never eat it: an "offline" the row no longer
        // shows is a kept paper the reader cannot find again without
        // a network.
        if self.is_offline(&paper.id) {
            format!("offline \u{b7} {}", row_summary(paper))
        } else {
            row_summary(paper)
        }
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
        // The library row says how far through the paper anybody has read.
        // Blocks, not pages: a block is content and stays put when the type
        // size changes, which a page number does not.
        let progress = self.book.reader_mut().and_then(|reader| {
            let total = reader.document().blocks.len();
            let at = reader.memory().at as usize;
            let percent = u8::try_from((((at * 100) + (total / 2)) / total).min(100)).ok();
            (total > 0).then_some(percent)?
        });
        if let Some(progress) = progress {
            if let Some(kept) = self.library.iter_mut().find(|kept| kept.id == id) {
                if kept.progress != Some(progress) {
                    kept.progress = Some(progress);
                    self.save_library(context);
                }
            }
        }
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
        if self.library.len() >= MAX_KEPT || self.keeper.is_full() {
            self.trouble = Some(
                "The library is full. Remove a paper from it before keeping another.".to_owned(),
            );
            return;
        }
        if self.keeper.failed {
            self.trouble = Some(
                "Nothing new can be kept until the library is saved. Open Library and tap \
                 Try again."
                    .to_owned(),
            );
            return;
        }
        let Some(size) = self.fetched.as_ref().map(String::len) else {
            // What gets kept is the rendering, and from the abstract there
            // is not one yet. Rather than telling somebody to press Full
            // text and then press Keep again, fetch it and keep it when it
            // lands: the two taps were always going to be the same errand.
            self.keep_when_fetched = true;
            self.ask_full_text(context);
            return;
        };
        if !self.keep_on_device(context, &paper.id) {
            // Refused because the reader is full, the one reason left by the
            // time the checks above have passed.
            self.trouble = Some(NO_ROOM.to_owned());
            return;
        }
        self.pending.insert(paper.id.clone());
        // Listed as soon as the work starts rather than when it finishes, so
        // the button answers the tap. The row says it is still downloading
        // until the file is acknowledged, and a keep that fails takes the
        // entry back out again, so the library never names a paper with
        // nothing behind it.
        self.library.insert(
            0,
            Kept {
                id: paper.id.clone(),
                title: paper.title.clone(),
                authors: paper.byline(),
                bytes: u32::try_from(size).unwrap_or(u32::MAX),
                progress: None,
            },
        );
        self.save_library(context);
    }

    /// Hands the open paper's rendering to the keeper, which fetches its
    /// figures beside it and writes the lot to the shelf as one file.
    fn keep_on_device(&mut self, context: &mut Context, id: &str) -> bool {
        let Some(body) = self.fetched.as_deref() else {
            return false;
        };
        // The same address the rendering was read from, so the figures it
        // names resolve exactly as they do when the paper is open.
        let origin = format!("{}/{}", html_base(), escape_path(id));
        self.keeper.keep_markup(context, id, &origin, body)
    }

    /// Whether a kept paper opens with no network.
    ///
    /// A paper still being written does not, and nor does one whose file was
    /// refused. A paper kept before figures were does, without its figures.
    fn is_offline(&self, id: &str) -> bool {
        self.is_kept(id) && !self.pending.contains(id)
    }

    /// The library's line about what is on the reader.
    ///
    /// Counted from the library rather than the keeper alone, because a paper
    /// kept before figures were is on the reader too and opens offline.
    fn ready_offline(&self) -> String {
        let mut count = 0;
        let mut bytes = self.keeper.bytes();
        for kept in &self.library {
            if !self.is_offline(&kept.id) {
                continue;
            }
            count += 1;
            if !self.keeper.is_kept(&kept.id) {
                bytes += u64::from(kept.bytes);
            }
        }
        kobo_bookview::offline::ready_offline(count, bytes, "paper", "papers")
    }

    /// Takes a paper back out of the library, and its file off the shelf.
    fn discard_paper(&mut self, context: &mut Context, id: &str) {
        self.library.retain(|kept| kept.id != id);
        self.pending.remove(id);
        self.upgrading.remove(id);
        self.keeper.remove(context, id);
        // The rendering a paper kept before figures were is stored under. A
        // paper kept since has none, and removing what is not there costs a
        // refusal nobody sees.
        context.shelf().remove(blob_key(id));
        self.save_library(context);
    }

    fn save_library(&mut self, context: &mut Context) {
        context
            .store()
            .save(LIBRARY_KEY, encode_library(&self.library));
    }

    /// Saves the listing's word search so Saved can run it again.
    fn save_search(&mut self, context: &mut Context) {
        let Some(Query::Words(words)) = &self.query else {
            return;
        };
        if self.saved.contains(words) {
            return;
        }
        if self.saved.len() >= MAX_SAVED {
            self.trouble = Some(format!(
                "Saved searches hold {MAX_SAVED}. Remove one in Saved."
            ));
            return;
        }
        self.saved.insert(0, words.clone());
        context.store().save(SEARCHES_KEY, encode_list(&self.saved));
    }

    /// Follows the listing's subject, pinning it to the top of the list.
    fn follow_subject(&mut self, context: &mut Context) {
        let Some(Query::Subject { code, .. }) = &self.query else {
            return;
        };
        if self.followed.contains(code) {
            return;
        }
        if self.followed.len() >= MAX_SAVED {
            self.trouble = Some(format!(
                "Followed subjects hold {MAX_SAVED}. Remove one in Saved."
            ));
            return;
        }
        self.followed.push(code.clone());
        context
            .store()
            .save(FOLLOWED_KEY, encode_list(&self.followed));
    }

    fn unfollow_subject(&mut self, context: &mut Context) {
        let Some(Query::Subject { code, .. }) = &self.query else {
            return;
        };
        self.followed.retain(|followed| followed != code);
        context
            .store()
            .save(FOLLOWED_KEY, encode_list(&self.followed));
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
        self.view = View::Paper;
        if self.keeper.open(context, &kept.id) {
            self.opening = Some(kept.id);
        } else {
            self.open_legacy(context, &kept.id);
        }
    }

    /// Reads a paper kept before figures were, which is its rendering alone.
    fn open_legacy(&mut self, context: &mut Context, id: &str) {
        let mut download = ShelfDownload::new(blob_key(id)).at_most(FULL_TEXT_BYTES as usize);
        download.start(context);
        self.loading = Some(download);
        self.transferring = Some(id.to_owned());
        self.from_legacy = true;
    }

    /// Gives back everything the open paper was costing.
    fn close_paper(&mut self, context: &mut Context) {
        // The place goes down before the document it points into goes away.
        self.save_place(context);
        self.book.close(context);
        self.opening = None;
        self.keeper.stop_opening();
        self.truncated = false;
        self.formulae_as_text = false;
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

/// The facts that decide whether a paper is worth reading, one per line, in
/// the order a reader asks for them: who wrote it, where it sits, when it
/// came, and anything the authors thought to add.
fn fact_lines(paper: &Paper) -> Vec<String> {
    let mut lines = Vec::new();
    let byline = paper.byline();
    if !byline.is_empty() {
        lines.push(byline);
    }
    if !paper.categories.is_empty() {
        lines.push(paper.categories.join(", "));
    }
    if !paper.published.is_empty() {
        // Both dates, but only when they differ: a paper revised twice is a
        // different thing from the one first posted, and saying so costs a
        // line only for the papers where it is true.
        lines.push(
            if paper.updated.is_empty() || paper.updated == paper.published {
                format!("Submitted {}", paper.published)
            } else {
                format!("Submitted {}, revised {}", paper.published, paper.updated)
            },
        );
    }
    if !paper.journal.is_empty() {
        lines.push(format!("Published in {}", paper.journal));
    }
    if !paper.comment.is_empty() {
        lines.push(paper.comment.clone());
    }
    lines
}

/// The identifier as it goes in a path.
///
/// The old scheme has a slash in it -- `cond-mat/0703470` -- and that slash is
/// a real path separator in arXiv's URLs rather than something to encode.
fn escape_path(id: &str) -> String {
    id.split('/').map(escape).collect::<Vec<_>>().join("/")
}

/// A listing on its way: the lines that stand in for its rows.
///
/// As many as the panel has room for under what is already there, up to six.
/// A fixed six ran under the foot of the panel at the largest text sizes, and
/// the renderer refused the whole screen.
fn placeholder(context: &Context, screen: &ScreenBuilder) -> Screen {
    let metrics = context.metrics();
    (1..=6)
        .rev()
        .map(|lines| screen.clone().skeleton(lines).build())
        .find(|candidate| {
            !candidate
                .diagnostics(&metrics, &kobo_sdk::Chrome::measuring(true))
                .has_errors()
        })
        .unwrap_or_else(|| screen.clone().build())
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
    /// arXiv's wordmark is red, and so is every page of the site's own
    /// listing; the bar borrows it so the reader knows which archive this is.
    fn accent(&self) -> Option<kobo_sdk::Accent> {
        Some(kobo_sdk::Accent::Red)
    }

    fn on_start(&mut self, context: &mut Context) {
        context.store().load(LIBRARY_KEY);
        context.store().load(SEARCHES_KEY);
        context.store().load(FOLLOWED_KEY);
        self.keeper.start(context);
        self.show(context);
    }

    #[allow(clippy::too_many_lines)]
    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        // The search field first: while it is open the runtime holds the
        // panel, and its answer arrives as an action like any other.
        if let Some(typing) = self.entry.handle(action) {
            match typing {
                Typing::Submitted(typed) => {
                    self.papers.clear();
                    self.group = None;
                    self.view = View::Listing;
                    self.ask_listing(context, Query::Words(typed), 0);
                }
                // Backing out of the keyboard is Back from the search, which
                // has only ever led to the subject list.
                Typing::Cancelled | Typing::Saved | Typing::Refused(_)
                    if self.view == View::Search =>
                {
                    self.view = View::Subjects;
                }
                _ => {}
            }
            self.show(context);
            return;
        }

        if action == ActionId::BACK {
            self.trouble = None;
            match self.view {
                View::Subjects => return,
                // A listing opened from one of arXiv's headings goes back to
                // that heading, at the page it was left on, rather than all
                // the way back to the start.
                View::Listing if self.group.is_some() => {
                    self.view = View::Group;
                    self.listing_page = 0;
                }
                View::Search | View::Listing | View::Library | View::Saved | View::Group => {
                    self.view = View::Subjects;
                    self.listing_page = 0;
                    self.group = None;
                }
                View::Paper => {
                    self.opening = None;
                    self.keeper.stop_opening();
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
            self.entry.open();
            self.trouble = None;
            self.view = View::Search;
            self.show(context);
            return;
        }

        if action == action_id(SAVED) {
            self.saved_page = 0;
            self.managing = false;
            self.view = View::Saved;
            self.show(context);
            return;
        }

        if action == action_id(MANAGE) {
            self.managing = true;
            self.show(context);
            return;
        }

        if action == action_id(DONE) {
            self.managing = false;
            self.show(context);
            return;
        }

        if action == action_id(SAVE_SEARCH) {
            self.save_search(context);
            self.show(context);
            return;
        }

        if action == action_id(FOLLOW) {
            self.follow_subject(context);
            self.show(context);
            return;
        }

        if action == action_id(UNFOLLOW) {
            self.unfollow_subject(context);
            self.show(context);
            return;
        }

        if action == action_id(RETRY_LISTING) {
            if let Some(query) = self.query.clone() {
                self.listing_page = 0;
                self.ask_listing(context, query, self.offset);
            }
            self.show(context);
            return;
        }

        if action == action_id(RETRY_KEPT) {
            self.keeper.retry(context);
            self.show(context);
            return;
        }

        if action == action_id(SUBJECTS_BACK)
            || action == action_id(GROUP_BACK)
            || action == action_id(LIST_BACK)
            || action == action_id(SAVED_BACK)
        {
            self.turn(context, false);
            return;
        }
        if action == action_id(SUBJECTS_NEXT)
            || action == action_id(GROUP_NEXT)
            || action == action_id(LIST_NEXT)
            || action == action_id(SAVED_NEXT)
        {
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

        for group in 0..GROUPS.len() {
            if action == action_id(&format!("{GROUP}{group}")) {
                let inside: Vec<usize> = group_subjects(group).collect();
                self.group = Some(group);
                if let [only] = inside.as_slice() {
                    let (code, name, _) = SUBJECTS[*only];
                    self.papers.clear();
                    self.listing_page = 0;
                    self.view = View::Listing;
                    // Back from a heading of one goes to the start, not to a
                    // list of one it never showed.
                    self.group = None;
                    self.ask_listing(
                        context,
                        Query::Subject {
                            code: code.to_owned(),
                            name: name.to_owned(),
                        },
                        0,
                    );
                } else {
                    self.group_page = 0;
                    self.view = View::Group;
                }
                self.trouble = None;
                self.show(context);
                return;
            }
        }

        for (index, (code, name, _)) in SUBJECTS.iter().enumerate() {
            if action == action_id(&format!("{SUBJECT}{index}")) {
                // Remembered only when the subject was chosen under its
                // heading; a followed subject on the first screen goes back
                // to the first screen.
                if self.view != View::Group {
                    self.group = None;
                }
                self.papers.clear();
                self.listing_page = 0;
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

        for index in 0..self.saved.len() {
            if action == action_id(&format!("{SSEARCH}{index}")) {
                if self.managing {
                    self.saved.remove(index);
                    context.store().save(SEARCHES_KEY, encode_list(&self.saved));
                    self.show(context);
                    return;
                }
                let words = self.saved[index].clone();
                self.papers.clear();
                self.group = None;
                self.listing_page = 0;
                self.managing = false;
                self.view = View::Listing;
                self.ask_listing(context, Query::Words(words), 0);
                self.show(context);
                return;
            }
        }

        for index in 0..self.followed.len() {
            if action == action_id(&format!("{FROW}{index}")) {
                if self.managing {
                    self.followed.remove(index);
                    context
                        .store()
                        .save(FOLLOWED_KEY, encode_list(&self.followed));
                    self.show(context);
                    return;
                }
                let code = self.followed[index].clone();
                let Some((code, name)) = SUBJECTS
                    .iter()
                    .find(|(known, _, _)| *known == code)
                    .map(|(code, name, _)| ((*code).to_owned(), (*name).to_owned()))
                else {
                    return;
                };
                self.group = None;
                self.papers.clear();
                self.listing_page = 0;
                self.managing = false;
                self.view = View::Listing;
                self.ask_listing(context, Query::Subject { code, name }, 0);
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
        let heard = self.keeper.task(context, task, &outcome);
        if !matches!(heard, Heard::Elsewhere) {
            self.heard(context, heard);
            return;
        }
        self.took_task(context, task, outcome);
        // A lane the keeper could not have a moment ago may be free now.
        self.keeper.pump(context);
    }

    fn on_load(&mut self, context: &mut Context, key: &str, result: StoreResult) {
        self.answered(context, Asked::Load, key, result);
    }

    fn on_save(&mut self, context: &mut Context, key: &str, result: StoreResult) {
        self.answered(context, Asked::Save, key, result);
    }

    fn on_shelf(&mut self, context: &mut Context, name: &str, result: StoreResult) {
        self.answered(context, Asked::Shelf, name, result);
    }

    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
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
                    self.from_legacy = false;
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
            } else if key == SEARCHES_KEY {
                self.saved = value.as_deref().map(decode_list).unwrap_or_default();
                self.show(context);
            } else if key == FOLLOWED_KEY {
                self.followed = value.as_deref().map(decode_list).unwrap_or_default();
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

/// Which kind of request a store answer is answering.
#[derive(Clone, Copy)]
enum Asked {
    Load,
    Save,
    Shelf,
}

impl Arxiv {
    /// Offers a store answer to the keeper, and to the rest of the
    /// application when it is not the keeper's.
    ///
    /// Routed on what the answer itself names wherever it names anything,
    /// and on what was asked only for a refusal, which names nothing.
    fn answered(
        &mut self,
        context: &mut Context,
        asked: Asked,
        asked_for: &str,
        result: StoreResult,
    ) {
        let heard = match (&result, asked) {
            (StoreResult::Loaded { key, .. }, _) => self.keeper.loaded(context, key, &result),
            (StoreResult::Saved { key }, _) => self.keeper.saved(context, key, &result),
            (
                StoreResult::ShelfWritten { name, .. }
                | StoreResult::ShelfRead { name, .. }
                | StoreResult::ShelfRemoved { name },
                _,
            ) => self.keeper.shelf(context, name, &result),
            (StoreResult::Denied(_), Asked::Load) => {
                self.keeper.loaded(context, asked_for, &result)
            }
            (StoreResult::Denied(_), Asked::Save) => self.keeper.saved(context, asked_for, &result),
            (StoreResult::Denied(_), Asked::Shelf) => {
                self.keeper.shelf(context, asked_for, &result)
            }
            _ => Heard::Elsewhere,
        };
        if matches!(heard, Heard::Elsewhere) {
            self.on_store(context, result);
        } else {
            self.heard(context, heard);
        }
    }

    /// Acts on what the keeper made of an answer.
    fn heard(&mut self, context: &mut Context, heard: Heard) {
        match heard {
            Heard::Elsewhere | Heard::Quiet => return,
            Heard::Changed => self.kept_changed(context),
            Heard::Opened(id, kept) => {
                if self.opening.as_deref() != Some(id.as_str())
                    || self.paper().is_none_or(|paper| paper.id != id)
                {
                    return;
                }
                self.opening = None;
                let memory = self.place.take().unwrap_or_default();
                if self.book.open_kept(context, kept, memory) {
                    self.truncated = false;
                    self.formulae_as_text = self.book.formulae_as_text();
                    self.page = 0;
                    self.view = View::FullText;
                } else {
                    self.trouble =
                        Some("That kept paper has nothing in it that can be read.".to_owned());
                }
            }
            Heard::Unreadable(id) => {
                if self.opening.as_deref() == Some(id.as_str()) {
                    self.opening = None;
                    // A paper kept before figures were may still have its
                    // rendering on the shelf under its older name.
                    self.open_legacy(context, &id);
                }
            }
            Heard::Failed(id, why) => {
                self.upgrading.remove(&id);
                if self.pending.remove(&id) {
                    self.library.retain(|kept| kept.id != id);
                    self.save_library(context);
                    self.trouble = Some(if why == NO_ROOM {
                        NO_ROOM.to_owned()
                    } else {
                        format!("That paper could not be kept. {why}")
                    });
                }
            }
        }
        self.show(context);
    }

    /// Catches the library up with what the keeper has acknowledged.
    fn kept_changed(&mut self, context: &mut Context) {
        let landed: Vec<String> = self
            .pending
            .iter()
            .chain(&self.upgrading)
            .filter(|id| self.keeper.is_kept(id))
            .cloned()
            .collect();
        let mut rewrite = false;
        for id in landed {
            // The row says what the paper costs with its figures, which is
            // what removing it would give back.
            if let (Some(size), Some(kept)) = (
                self.keeper.size(&id),
                self.library.iter_mut().find(|kept| kept.id == id),
            ) {
                kept.bytes = u32::try_from(size).unwrap_or(u32::MAX);
                rewrite = true;
            }
            self.pending.remove(&id);
            if self.upgrading.remove(&id) {
                // The figures are on the shelf with the paper now, so the
                // older file holding the rendering alone can go.
                context.shelf().remove(blob_key(&id));
            }
        }
        // A reader that filled up while papers waited lets the waiting ones
        // go without a word. Each is taken back out of the library here, with
        // the reason, rather than left saying it is downloading for ever.
        if self.keeper.is_full() && !self.keeper.is_busy() {
            let dropped: Vec<String> = self
                .pending
                .iter()
                .filter(|id| !self.keeper.is_kept(id))
                .cloned()
                .collect();
            for id in dropped {
                self.pending.remove(&id);
                self.library.retain(|kept| kept.id != id);
                rewrite = true;
                self.trouble = Some(NO_ROOM.to_owned());
            }
            self.upgrading.retain(|id| self.keeper.is_kept(id));
        }
        if rewrite {
            self.save_library(context);
        }
    }

    fn took_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
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
        // No network at all is the reader's to fix, not arXiv's, and the SDK
        // says so in the words every other application uses.
        (TaskError::Offline, _) => kobo_sdk::Failure::of(error).advice.to_owned(),
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
        blob_key, decode_library, decode_list, encode_library, encode_list, escape, escape_path,
        fact_lines, kept_summary, paper_body, place_key, row_summary, stamp, Arxiv, Kept, Query,
        View, Window, ABSTRACT, DISCARD, DONE, FOLLOW, FOLLOWED_KEY, FULL_TEXT, KEEP, LIBRARY_KEY,
        MANAGE, MAX_SAVED, SAVED, SAVE_SEARCH, SEARCHES_KEY, SSEARCH, SUBJECTS, UNFOLLOW, WINDOW,
    };
    use super::{group_subjects, GROUP, GROUPS, LIBRARY, SUBJECT};
    use crate::atom::Paper;
    use kobo_bookview::offline::{Kept as KeptFile, INDEX_KEY};
    use kobo_read::Memory;
    use kobo_sdk::StoreResult;
    use kobo_sdk::{
        action_id, is_valid_key, ActionId, AppRunner, Command, StoreRequest, Task, TaskError,
        TaskId, TaskOutcome,
    };

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
    fn text_size_metrics() -> impl Iterator<Item = kobo_sdk::DisplayMetrics> {
        let mut smallest = kobo_sdk::CLARA_BW_METRICS;
        while let Some(scale) = smallest.text_scale.smaller() {
            smallest.text_scale = scale;
        }
        std::iter::successors(Some(smallest), |metrics| {
            Some(kobo_sdk::DisplayMetrics {
                text_scale: metrics.text_scale.larger()?,
                ..*metrics
            })
        })
    }

    #[test]
    fn notices_leave_every_subject_paper_and_saved_item_reachable() {
        for metrics in text_size_metrics() {
            let text_scale = metrics.text_scale;
            let context = AppRunner::with_metrics(Arxiv::default(), metrics).context();
            let mut app = Arxiv {
                trouble: Some(
                    "The archive could not be reached. Saved items remain available.".into(),
                ),
                ..Arxiv::default()
            };
            app.papers = (0..12)
                .map(|index| Paper {
                    title: format!("Paper {index}: Measurements beside the river"),
                    ..paper()
                })
                .collect();
            app.total = 12;
            app.library = (0..12)
                .map(|index| Kept {
                    id: format!("2609.{index:05}"),
                    title: format!("Paper {index}: Measurements beside the river"),
                    authors: "Ada Lovelace".into(),
                    bytes: 4096,
                    ..Kept::default()
                })
                .collect();
            app.saved = (0..12)
                .map(|index| format!("river measurements {index}"))
                .collect();
            // The library's own failure and its retry go above the trouble
            // notice, so the kept papers are paged under both.
            app.keeper.failed = true;
            for (view, prefix, count) in [
                (View::Subjects, GROUP, GROUPS.len()),
                (View::Listing, super::PAPER, 12),
                (View::Library, super::KEPT, 12),
                (View::Saved, SSEARCH, 12),
            ] {
                app.view = view;
                app.subject_page = 0;
                app.listing_page = 0;
                app.library_page = 0;
                app.saved_page = 0;
                let screen = |app: &Arxiv| match view {
                    View::Subjects => app.subjects(&context),
                    View::Listing => app.listing(&context),
                    View::Library => app.library(&context),
                    View::Saved => app.saved(&context),
                    _ => unreachable!(),
                };
                let pages = screen(&app)
                    .page_turns
                    .and_then(|turns| turns.position)
                    .unwrap()
                    .1;
                let mut seen = vec![0; count];
                for page in 0..usize::from(pages) {
                    app.subject_page = page;
                    app.listing_page = page;
                    app.library_page = page;
                    app.saved_page = page;
                    let diagnostics =
                        screen(&app).diagnostics(&metrics, &kobo_sdk::Chrome::measuring(true));
                    assert!(
                        diagnostics.issues.iter().all(|issue| {
                            // “Machine Learning (Statistics)” is a taxonomy
                            // qualifier, not an application-state suffix.
                            view == View::Subjects
                                && issue.kind == kobo_sdk::LayoutIssueKind::StateInLabel
                        }),
                        "{text_scale:?}, {view:?}, page {page}: {:?}",
                        diagnostics.issues
                    );
                    for (index, times) in seen.iter_mut().enumerate() {
                        let action = action_id(&format!("{prefix}{index}"));
                        if let Some(rect) = diagnostics.layout.rect_of_action(action) {
                            assert_eq!(
                                diagnostics
                                    .layout
                                    .hit_test(rect.x + rect.width / 2, rect.y + rect.height / 2),
                                Some(action)
                            );
                            *times += 1;
                        }
                    }
                }
                assert_eq!(seen, vec![1; count]);
            }
        }
    }

    #[test]
    fn abstract_pages_keep_all_title_facts_and_prose_under_a_recovery_notice() {
        for metrics in text_size_metrics() {
            let text_scale = metrics.text_scale;
            let mut context = AppRunner::with_metrics(Arxiv::default(), metrics).context();
            let title = "A study of language models and river measurements across several seasons "
                .repeat(4);
            let summary="The researchers measured the river and compared their observations with the surrounding fields. ".repeat(12);
            let paper = Paper {
                title: title.trim().to_owned(),
                summary: summary.clone(),
                ..paper()
            };
            let facts = fact_lines(&paper).join(" ");
            let mut app = Arxiv {
                view: View::Paper,
                open: Some(0),
                papers: vec![paper],
                trouble: Some("This paper could not be saved. Try keeping it again.".into()),
                ..Arxiv::default()
            };
            app.open_abstract(&context);
            let all = app.pages.iter().flatten().collect::<Vec<_>>();
            for (kind, expected) in [
                (0, title.as_str()),
                (1, facts.as_str()),
                (2, summary.as_str()),
            ] {
                let text = all
                    .iter()
                    .filter_map(|block| match (kind, block) {
                        (0, super::AbstractBlock::Title(t))
                        | (1, super::AbstractBlock::Fact(t))
                        | (2, super::AbstractBlock::Body(t)) => Some(t.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                assert_eq!(
                    text.split_whitespace().collect::<Vec<_>>(),
                    expected.split_whitespace().collect::<Vec<_>>()
                );
            }
            for page in 0..app.pages.len() {
                app.page = page;
                let diagnostics = app
                    .reading()
                    .diagnostics(&metrics, &kobo_sdk::Chrome::measuring(true));
                assert!(
                    diagnostics.issues.is_empty(),
                    "{text_scale:?}, page {page}: {:?}",
                    diagnostics.issues
                );
            }
            let last = app.pages.len() - 1;
            app.turn(&mut context, true);
            app.turn(&mut context, true);
            assert_eq!(app.page, last);
            app.turn(&mut context, false);
            assert_eq!(app.page, last.saturating_sub(1));
        }
    }

    #[test]
    fn unusually_long_unbroken_titles_split_at_utf8_boundaries() {
        let metrics = text_size_metrics().last().unwrap();
        let context = AppRunner::with_metrics(Arxiv::default(), metrics).context();
        let title = "étude".repeat(120);
        let mut app = Arxiv {
            open: Some(0),
            papers: vec![Paper {
                title: title.clone(),
                ..paper()
            }],
            ..Arxiv::default()
        };
        app.open_abstract(&context);
        let reconstructed = app
            .pages
            .iter()
            .flatten()
            .filter_map(|block| {
                if let super::AbstractBlock::Title(text) = block {
                    Some(text.as_str())
                } else {
                    None
                }
            })
            .collect::<String>();
        assert_eq!(reconstructed, title);
        for page in 0..app.pages.len() {
            app.page = page;
            assert!(app
                .reading()
                .diagnostics(&metrics, &kobo_sdk::Chrome::measuring(true))
                .issues
                .is_empty());
        }
    }

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
        for (index, (code, _, _)) in SUBJECTS.iter().enumerate() {
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

    /// The whole taxonomy, each category once, each under a heading that
    /// exists and no heading left empty.
    #[test]
    fn every_arxiv_category_is_offered_once_under_a_heading() {
        let codes: std::collections::BTreeSet<_> =
            SUBJECTS.iter().map(|(code, _, _)| *code).collect();
        assert_eq!(codes.len(), SUBJECTS.len(), "a category is offered twice");
        assert!(SUBJECTS.len() >= 150, "only {} categories", SUBJECTS.len());
        for (group, name) in GROUPS.iter().enumerate() {
            assert!(group_subjects(group).count() > 0, "{name} is empty");
        }
        assert!(SUBJECTS.iter().all(|(_, _, group)| *group < GROUPS.len()));
        for code in [
            "cs.AI",
            "math.AG",
            "physics.flu-dyn",
            "q-fin.TR",
            "eess.SP",
            "hep-th",
        ] {
            assert!(codes.contains(code), "{code} is missing");
        }
    }

    /// Every category is on exactly one page of its heading, tappable where
    /// it is drawn, at every text size the reader offers.
    #[test]
    fn every_category_is_reachable_on_its_headings_pages_at_every_text_size() {
        for metrics in text_size_metrics() {
            let context = AppRunner::with_metrics(Arxiv::default(), metrics).context();
            for (group, heading) in GROUPS.iter().enumerate() {
                let inside: Vec<usize> = group_subjects(group).collect();
                if inside.len() < 2 {
                    continue;
                }
                let mut app = Arxiv {
                    view: View::Group,
                    group: Some(group),
                    ..Arxiv::default()
                };
                let pages = app
                    .group_screen(&context)
                    .page_turns
                    .and_then(|turns| turns.position)
                    .map_or(1, |(_, total)| usize::from(total));
                let mut seen = vec![0; inside.len()];
                for page in 0..pages {
                    app.group_page = page;
                    let screen = app.group_screen(&context);
                    assert_eq!(
                        screen.top_bar.as_ref().map(|bar| bar.title.as_str()),
                        Some(*heading)
                    );
                    let diagnostics =
                        screen.diagnostics(&metrics, &kobo_sdk::Chrome::measuring(true));
                    assert!(
                        diagnostics.issues.is_empty(),
                        "{:?} {}: {:?}",
                        metrics.text_scale,
                        heading,
                        diagnostics.issues
                    );
                    for (at, index) in inside.iter().enumerate() {
                        let action = action_id(&format!("{SUBJECT}{index}"));
                        if diagnostics.layout.rect_of_action(action).is_some() {
                            seen[at] += 1;
                        }
                    }
                }
                assert_eq!(seen, vec![1; inside.len()], "{heading}");
            }
        }
    }

    /// A heading opens its categories, a category opens its listing, and Back
    /// retraces those steps rather than jumping to the start.
    #[test]
    fn a_heading_leads_to_its_categories_and_back_retraces_the_way() {
        let mut runner = AppRunner::new(Arxiv::default());
        runner.start();
        runner.action(action_id(&format!("{GROUP}3")));
        assert_eq!(runner.app().view, View::Group);
        let first = group_subjects(3)
            .next()
            .expect("Mathematics has categories");
        let commands = runner.action(action_id(&format!("{SUBJECT}{first}")));
        assert_eq!(runner.app().view, View::Listing);
        assert!(commands.iter().any(|command| matches!(
            command,
            Command::Spawn { work: Task::Fetch { url, .. }, .. } if url.contains("cat:math.AC")
        )));
        runner.action(ActionId::BACK);
        assert_eq!(runner.app().view, View::Group);
        runner.action(ActionId::BACK);
        assert_eq!(runner.app().view, View::Subjects);
    }

    /// A heading that holds a single category is that category.
    #[test]
    fn a_heading_of_one_category_opens_its_listing_directly() {
        let only = (0..GROUPS.len())
            .find(|group| group_subjects(*group).count() == 1)
            .expect("arXiv has archives of one category");
        let mut runner = AppRunner::new(Arxiv::default());
        runner.start();
        let commands = runner.action(action_id(&format!("{GROUP}{only}")));
        assert_eq!(runner.app().view, View::Listing);
        let code = SUBJECTS[group_subjects(only).next().expect("one")].0;
        assert!(commands.iter().any(|command| matches!(
            command,
            Command::Spawn { work: Task::Fetch { url, .. }, .. }
                if url.contains(&format!("cat:{}", escape(code)))
        )));
        runner.action(ActionId::BACK);
        assert_eq!(runner.app().view, View::Subjects);
    }

    /// A listing that could not be fetched offers to try again, and does not
    /// pretend the subject published nothing.
    #[test]
    fn a_listing_that_never_arrived_offers_to_try_again_rather_than_no_papers() {
        let mut runner = AppRunner::new(Arxiv::default());
        runner.start();
        runner.action(action_id("subject-0"));
        // The request is retried before it is given up on, so it fails as
        // many times as it is asked.
        for _ in 0..8 {
            let Some((task, _)) = runner.app().task else {
                break;
            };
            runner.task_outcome(task, TaskOutcome::Failed(TaskError::Offline));
        }
        assert!(runner.app().task.is_none(), "the listing never gave up");
        let context = runner.context();
        let rendered = format!("{:?}", runner.app().listing(&context));
        assert!(rendered.contains("not on a network"), "{rendered}");
        assert!(rendered.contains("Try again"), "{rendered}");
        assert!(!rendered.contains("No papers found"), "{rendered}");
        let commands = runner.action(action_id(super::RETRY_LISTING));
        assert!(commands.iter().any(|command| matches!(
            command,
            Command::Spawn { work: Task::Fetch { url, .. }, .. } if url.contains("cat:cs.AI")
        )));
    }

    /// A search is typed on the reader's own keyboard, and what comes back
    /// is the question arXiv is asked. Backing out is Back from the search.
    #[test]
    fn a_search_is_typed_on_the_readers_keyboard_and_backing_out_returns_to_the_subjects() {
        let mut runner = AppRunner::new(Arxiv::default());
        runner.start();
        let commands = runner.action(action_id(super::SEARCH));
        let Some(kobo_sdk::DeviceRequest::TypeText {
            prompt,
            hint,
            submit,
            ..
        }) = kobo_sdk::keyboard::testing::asked(&commands).cloned()
        else {
            panic!("the reader's keyboard was not asked for: {commands:?}")
        };
        assert_eq!(
            (prompt.as_str(), hint.as_str(), submit.as_str()),
            ("Search arXiv", "A phrase, an author, a title", "Search")
        );
        let commands = runner.type_text("  neural fields ");
        assert_eq!(runner.app().view, View::Listing);
        assert!(commands.iter().any(|command| matches!(
            command,
            Command::Spawn { work: Task::Fetch { url, .. }, .. } if url.contains("neural")
        )));

        runner.action(ActionId::BACK);
        runner.action(action_id(super::SEARCH));
        runner.cancel_text();
        assert_eq!(runner.app().view, View::Subjects);
    }

    /// The retry for a library that could not be read or saved is on the
    /// banner that says so, not in the top bar away from it.
    #[test]
    fn a_library_that_could_not_be_saved_offers_its_retry_on_the_banner() {
        let mut runner = AppRunner::new(Arxiv::default());
        runner.start();
        runner.app_mut().keeper.failed = true;
        runner.app_mut().view = View::Library;
        let context = runner.context();
        let screen = runner.app().library(&context);
        let retry = action_id(super::RETRY_KEPT);
        assert!(screen.nodes.iter().any(|node| matches!(
            node,
            kobo_sdk::Node::Banner { level: kobo_sdk::BannerLevel::Error, action: Some(action), .. }
                if action.action == retry
        )));
        assert!(
            screen
                .top_bar
                .as_ref()
                .is_none_or(|bar| bar.actions.iter().all(|a| a.action != retry)),
            "the retry is still in the top bar as well"
        );
    }

    /// While a listing is on its way its placeholder fits the panel at every
    /// text size held either way, with or without a notice above it.
    #[test]
    fn a_listing_on_its_way_fits_its_placeholder_at_every_text_size_held_either_way() {
        for metrics in text_size_metrics() {
            for orientation in [
                kobo_sdk::Orientation::Portrait,
                kobo_sdk::Orientation::Landscape,
            ] {
                let metrics = metrics.oriented(orientation);
                let context = AppRunner::with_metrics(Arxiv::default(), metrics).context();
                for trouble in [None, Some("The archive is slow to answer. Still asking.")] {
                    let app = Arxiv {
                        view: View::Listing,
                        query: Some(Query::Words("neural fields".into())),
                        trouble: trouble.map(str::to_owned),
                        task: Some((TaskId(1), super::Awaiting::Listing)),
                        ..Arxiv::default()
                    };
                    let screen = app.listing(&context);
                    let diagnostics =
                        screen.diagnostics(&metrics, &kobo_sdk::Chrome::measuring(true));
                    assert!(
                        diagnostics.issues.is_empty(),
                        "{metrics:?} {trouble:?}: {:?}",
                        diagnostics.issues
                    );
                    assert!(
                        screen
                            .nodes
                            .iter()
                            .any(|node| matches!(node, kobo_sdk::Node::Skeleton { .. })),
                        "{metrics:?}: no placeholder"
                    );
                }
            }
        }
    }

    /// A listing that never arrived says so with its retry on the banner,
    /// and the banner, its action and the date window fit at every size.
    #[test]
    fn a_listing_that_never_arrived_fits_its_retry_at_every_text_size() {
        for metrics in text_size_metrics() {
            let context = AppRunner::with_metrics(Arxiv::default(), metrics).context();
            let app = Arxiv {
                view: View::Listing,
                query: Some(Query::Words("neural fields".into())),
                trouble: Some(
                    "This reader is not on a network. Connect to Wi-Fi and try again.".into(),
                ),
                ..Arxiv::default()
            };
            let diagnostics = app
                .listing(&context)
                .diagnostics(&metrics, &kobo_sdk::Chrome::measuring(true));
            assert!(
                diagnostics.issues.is_empty(),
                "{:?}: {:?}",
                metrics.text_scale,
                diagnostics.issues
            );
            let retry = action_id(super::RETRY_LISTING);
            let rect = diagnostics
                .layout
                .rect_of_action(retry)
                .expect("the retry is drawn");
            assert_eq!(
                diagnostics
                    .layout
                    .hit_test(rect.x + rect.width / 2, rect.y + rect.height / 2),
                Some(retry)
            );
        }
    }

    /// The bar over a paper says which paper it is in words.
    #[test]
    fn the_top_bar_names_the_paper_rather_than_its_number() {
        let mut runner = AppRunner::new(Arxiv::default());
        runner.app_mut().papers = vec![paper()];
        runner.start();
        runner.action(action_id("paper-0"));
        let screen = runner.app().reading();
        assert_eq!(
            screen.top_bar.map(|bar| bar.title).as_deref(),
            Some(paper().title.as_str())
        );
    }

    /// While the full text is in the air, the paper screen says so.
    #[test]
    fn fetching_the_full_text_shows_a_fetching_state() {
        let mut runner = AppRunner::new(Arxiv::default());
        runner.app_mut().papers = vec![paper()];
        runner.start();
        runner.action(action_id("paper-0"));
        runner.action(action_id(FULL_TEXT));
        let rendered = format!("{:?}", runner.app().reading());
        assert!(rendered.contains("Fetching the full text"), "{rendered}");
    }

    /// Both ways off the abstract stay reachable: the bottom band is a
    /// single slot, so Keep lives in the top bar and Full text in the band -
    /// a second bottom control would silently replace the first.
    #[test]
    fn the_paper_screen_keeps_keep_and_full_text_reachable() {
        let mut runner = AppRunner::new(Arxiv::default());
        runner.app_mut().papers = vec![paper()];
        runner.start();
        runner.action(action_id("paper-0"));
        let rendered = format!("{:?}", runner.app().reading());
        assert!(rendered.contains("Keep for offline"), "{rendered}");
        assert!(rendered.contains("Full text"), "{rendered}");

        runner.app_mut().library.push(Kept {
            id: paper().id.clone(),
            title: paper().title.clone(),
            authors: String::new(),
            bytes: 1,
            progress: None,
        });
        let rendered = format!("{:?}", runner.app().reading());
        assert!(rendered.contains("Remove from library"), "{rendered}");
        assert!(!rendered.contains("Keep for offline"), "{rendered}");
    }

    /// Older papers stay reachable from the listing's last page, from the
    /// top bar, without costing the window control its band.
    #[test]
    fn the_listing_keeps_older_papers_and_the_window_reachable() {
        let app = Arxiv {
            papers: vec![paper()],
            total: 50,
            ..Arxiv::default()
        };
        let rendered = format!("{:?}", app.listing(&kobo_sdk::Context::default()));
        assert!(rendered.contains("Older papers"), "{rendered}");
        assert!(rendered.contains("Any time"), "{rendered}");
    }

    /// The facts above an abstract are the ones that decide whether to read
    /// it, so they have to be there and be right.
    #[test]
    fn an_abstract_is_set_under_the_facts_that_decide_whether_to_read_it() {
        let lines = fact_lines(&paper());
        assert_eq!(
            lines,
            [
                "Ada Lovelace, Alan Turing",
                "cs.LG, cs.CL",
                "Submitted 2024-01-01, revised 2024-01-09",
                "12 pages",
            ]
        );
    }

    /// Title, facts and abstract are three kinds of text and are set as
    /// three: the title a heading, each fact a muted line of its own, and the
    /// abstract prose paginated beneath them - never one run-on paragraph.
    #[test]
    fn the_first_page_separates_title_facts_and_prose_visually() {
        let mut runner = AppRunner::new(Arxiv::default());
        runner.app_mut().papers = vec![paper()];
        runner.start();
        runner.action(action_id("paper-0"));
        let app = runner.app();
        // The paginated prose carries the abstract and nothing else: the
        // title and every fact live in the header above it.
        let body = app
            .pages
            .iter()
            .flatten()
            .filter_map(|block| {
                if let super::AbstractBlock::Body(text) = block {
                    Some(text.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(body.contains("We revisit the transformer."), "{body}");
        assert!(!body.contains("Attention Is All You Need Again"), "{body}");
        assert!(!body.contains("Submitted 2024-01-01"), "{body}");
        // And the first page - heading, fact lines and prose together - fits
        // the smallest panel the application ships to.
        let issues = app
            .reading()
            .diagnostics(&kobo_sdk::CLARA_BW_METRICS, &kobo_sdk::Chrome::default())
            .issues;
        assert!(issues.is_empty(), "{issues:?}");
    }

    /// A revision date equal to the submission date is not news, and a line
    /// spent saying so is a line off every page of the abstract.
    #[test]
    fn a_paper_never_revised_is_not_described_as_revised() {
        let never = Paper {
            updated: "2024-01-01".into(),
            ..paper()
        };
        let lines = fact_lines(&never);
        assert!(lines.iter().any(|line| line == "Submitted 2024-01-01"));
        assert!(!lines.iter().any(|line| line.contains("revised")));
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

    /// A long paper keeps its structure the whole way through: tables stay
    /// tables, a displayed formula is typeset and handed to the panel as a
    /// picture of itself, and a figure is fetched rather than dropped.
    ///
    /// The parser hands a formula over as its LaTeX source and the book view
    /// typesets it a pass at a time after the first page is already showing,
    /// so the test answers the runtime's wake tasks the way the runtime would
    /// until the pipeline runs dry.
    #[test]
    fn a_long_paper_keeps_its_formulas_tables_and_figures() {
        let mut runner = AppRunner::new(Arxiv::default());
        let mut commands = opened_on(
            &mut runner,
            "<article><h2>1 Introduction</h2><p>The union over every set.</p>             <math display=\"block\" alttext=\"\\bigcup_{i=1}^{n} A_i\"><mo>\u{22c3}</mo></math>             <table><tr><th>Model</th><th>Accuracy</th></tr>             <tr><td>Fixture A</td><td>91.2</td></tr></table>             <figure><img src=\"2609.00077v1/x1.png\" alt=\"A fixture plot\">             <figcaption>Figure 1.</figcaption></figure></article>",
        );
        let fetched: Vec<String> = commands
            .iter()
            .filter_map(|command| match command {
                Command::Spawn {
                    work: Task::Fetch { url, .. },
                    ..
                } => Some(url.clone()),
                _ => None,
            })
            .collect();
        let mut pictures_put = 0;
        for _ in 0..16 {
            let wakes: Vec<TaskId> = commands
                .iter()
                .filter_map(|command| match command {
                    Command::Spawn {
                        task,
                        work: Task::Sleep { .. },
                    } => Some(*task),
                    _ => None,
                })
                .collect();
            if wakes.is_empty() {
                break;
            }
            commands = wakes
                .into_iter()
                .flat_map(|wake| runner.task_outcome(wake, TaskOutcome::Completed(Vec::new())))
                .collect();
            pictures_put += commands
                .iter()
                .filter(|command| matches!(command, Command::PutPicture { .. }))
                .count();
        }
        let reader = runner.app().book.reader().expect("the paper is not open");
        let blocks = &reader.document().blocks;

        assert!(
            blocks
                .iter()
                .any(|block| matches!(block, kobo_doc::Block::Row { header: true, .. })),
            "the table's heading row was flattened into prose"
        );
        assert!(
            blocks.iter().any(|block| matches!(
                block,
                kobo_doc::Block::Row { cells, .. } if cells.iter().any(|cell| cell == "Fixture A")
            )),
            "the table's body was flattened into prose"
        );
        assert!(
            blocks.iter().any(|block| matches!(
                block,
                kobo_doc::Block::Picture { name, .. } if name == "formula:0"
            )),
            "the displayed formula lost its place in the text"
        );
        assert!(
            pictures_put > 0,
            "the displayed formula was never typeset and handed to the panel"
        );
        assert!(
            fetched
                .iter()
                .any(|url| url == "https://arxiv.org/html/2609.00077v1/x1.png"),
            "the figure was not fetched from beside its paper: {fetched:?}"
        );
    }

    /// A figure that will not fetch costs the paper nothing: the page reads
    /// on past it, no error is raised, and the reader's place still saves on
    /// the way out.
    ///
    /// The reader draws what the caption said the figure shows, which is what
    /// a document with a missing plate should look like; the failure belongs
    /// to the figure, not to the paper around it.
    #[test]
    fn a_failed_figure_fetch_leaves_the_paper_readable_and_its_place_saved() {
        let mut runner = AppRunner::new(Arxiv::default());
        let commands = opened_on(
            &mut runner,
            "<article><p>Before the figure.</p>             <figure><img src=\"2609.00077v1/x2.png\" alt=\"A missing plot\">             <figcaption>Figure 2.</figcaption></figure>             <p>After the figure.</p></article>",
        );
        let fetch = commands
            .iter()
            .find_map(|command| match command {
                Command::Spawn {
                    task,
                    work: Task::Fetch { url, .. },
                } if url.ends_with("x2.png") => Some(*task),
                _ => None,
            })
            .expect("the figure was never asked for");
        let _ = runner.task_outcome(fetch, TaskOutcome::Failed(TaskError::NotFound));

        assert!(
            runner.app().trouble.is_none(),
            "a missing figure was raised as an error"
        );
        let reader = runner.app().book.reader().expect("the paper is not open");
        assert!(
            reader.document().blocks.iter().any(|block| matches!(
                block,
                kobo_doc::Block::Paragraph(text) if text == "After the figure."
            )),
            "the paper stopped reading at the missing figure"
        );

        let commands = runner.action(ActionId::BACK);
        assert!(
            commands.iter().any(|command| matches!(
                command,
                Command::Store(StoreRequest::Save { key, .. }) if key.starts_with("place.")
            )),
            "leaving the paper saved no place"
        );
    }

    /// Reproduces the first live run's refused screen: a real title wraps
    /// the heading to several lines and a real abstract fills every page,
    /// and the first page still has to fit exactly.
    #[test]
    fn a_long_live_title_paginates_the_abstract_without_overflow() {
        let mut runner = AppRunner::new(Arxiv::default());
        let mut live = paper();
        live.title = "Objective vs. Search: Decomposing What Makes a Good Tokeniser".into();
        live.authors = vec![
            "Ahmetcan Yavuz".into(),
            "Clara Meister".into(),
            "Tiago Pimentel".into(),
        ];
        live.categories = vec!["cs.CL".into(), "cs.AI".into()];
        live.published = "2026-09-16".into();
        live.comment = "Accepted at EMNLP 2026. 20 pages, 4 figures, 10 tables. Code: https://github.com/Ahmetcanyvz/comp-vs-like".into();
        live.summary =
            "Two dominant tokenisation algorithms are used by modern language models: byte-pair encoding (BPE) and UnigramLM. These differ along two orthogonal axes: their optimisation objective (compression vs. log-likelihood) and their search procedure (bottom-up merging vs. top-down pruning). Existing comparisons confound these axes, making it unclear whether their observed differences stem from what is being optimised vs. how it is being optimised. We disentangle the two by introducing two new tokenisation algorithms that complete this 2x2 design space: BottomUpLL, a bottom-up likelihood-based tokeniser, and TopDownComp, a top-down compression-based tokeniser. The remainder of the abstract carries the evaluation and the conclusions at the same length as the real paper's. ".into();
        runner.app_mut().papers = vec![live];
        runner.app_mut().open = Some(0);
        runner.app_mut().view = View::Paper;
        let context = runner.context();
        runner.app_mut().open_abstract(&context);
        let screen = runner.app().reading();
        let issues = screen.validate(&kobo_sdk::CLARA_BW_METRICS);
        assert!(
            !issues
                .iter()
                .any(|issue| matches!(issue.kind, kobo_sdk::LayoutIssueKind::TextOverflow)),
            "the abstract page overflowed under a live-length title: {issues:?}"
        );
    }

    /// Live titles and bylines run far longer than anything a fixture needs,
    /// and a row whose text does not fit is a screen the renderer refuses
    /// outright -- which is what the first run against the real arXiv feed
    /// did. Rows clamp to the width the layout engine measures, two lines
    /// for a title and one for the facts beneath it.
    #[test]
    fn rows_clamp_live_length_titles_and_bylines() {
        let mut runner = AppRunner::new(Arxiv::default());
        let long_title = "Transformers Are Secretly ".repeat(12);
        let long_authors = vec![
            "Bartholomew Featherstonehaugh".to_owned(),
            "Alexandrina Konstantinopoulos".to_owned(),
            "Wolfgang Amadeus".to_owned(),
        ];
        let mut live = paper();
        live.title = long_title.clone();
        live.authors = long_authors.clone();
        runner.app_mut().papers = vec![live];
        runner.app_mut().view = View::Listing;
        runner.app_mut().library = vec![Kept {
            id: "2609.00099v1".into(),
            title: long_title.clone(),
            authors: long_authors.join(", "),
            bytes: 4096,
            progress: None,
        }];
        let context = runner.context();
        let screen = runner.app().listing(&context);
        // The badge surviving the clamp is guaranteed by construction:
        // listing_summary leads with it and one_line_row ellipsizes the
        // tail. That ordering is pinned by the listing_summary tests.
        let issues = screen.validate(&kobo_sdk::CLARA_BW_METRICS);
        assert!(
            !issues
                .iter()
                .any(|issue| matches!(issue.kind, kobo_sdk::LayoutIssueKind::TextOverflow)),
            "a live-length listing row still overflowed: {issues:?}"
        );

        runner.app_mut().library = vec![Kept {
            id: "2609.00099v1".into(),
            title: long_title,
            authors: long_authors.join(", "),
            bytes: 4096,
            progress: Some(74),
        }];
        let screen = runner.app().library(&context);
        let issues = screen.validate(&kobo_sdk::CLARA_BW_METRICS);
        assert!(
            !issues
                .iter()
                .any(|issue| matches!(issue.kind, kobo_sdk::LayoutIssueKind::TextOverflow)),
            "a live-length library row still overflowed: {issues:?}"
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
                progress: Some(42),
            },
            Kept {
                id: "math.CO/0601001".into(),
                title: "An Older Numbering Scheme".into(),
                authors: "B. Bourbaki".into(),
                bytes: 12,
                progress: None,
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
            progress: None,
        }];
        let read_back = decode_library(&encode_library(&library));
        assert_eq!(read_back.len(), 1, "the entry should still be one entry");
        assert_eq!(read_back[0].authors, "C. Cantor");
        assert!(
            !read_back[0].title.contains('\t'),
            "the tab should not have survived into the stored title"
        );
    }

    /// A catalogue written before progress was kept still reads, with every
    /// paper simply never-opened.
    #[test]
    fn a_library_from_before_progress_was_kept_still_reads() {
        let legacy = "2401.00001v2\t91234\tOn the Convergence of Things\tA. Author\n";
        let read_back = decode_library(legacy.as_bytes());
        assert_eq!(read_back.len(), 1);
        assert_eq!(read_back[0].progress, None);
        assert_eq!(read_back[0].title, "On the Convergence of Things");
    }

    /// The library row says what is on the shelf and how far it was read.
    #[test]
    fn a_kept_row_shows_size_and_reading_progress() {
        let kept = Kept {
            id: "2401.00001v2".into(),
            title: "On the Convergence of Things".into(),
            authors: "A. Author".into(),
            bytes: 91_234,
            progress: Some(42),
        };
        let summary = kept_summary(&kept);
        assert!(summary.contains("2401.00001v2"), "{summary}");
        assert!(summary.contains("90 KB"), "{summary}");
        assert!(summary.starts_with("42% \u{b7} "), "{summary}");
        // Never opened says nothing, rather than claiming nought percent.
        let unread = Kept {
            progress: None,
            ..kept
        };
        assert!(!kept_summary(&unread).contains('%'));
    }

    /// A listing row says when the paper is already on the shelf.
    #[test]
    fn a_listing_row_says_when_the_paper_is_kept() {
        let mut app = Arxiv::default();
        assert!(!app.listing_summary(&paper()).contains("offline"));
        app.library.push(Kept {
            id: paper().id.clone(),
            title: paper().title.clone(),
            authors: String::new(),
            bytes: 1,
            progress: None,
        });
        let summary = app.listing_summary(&paper());
        assert!(summary.starts_with("offline \u{b7} "), "{summary}");
        // And the facts that place the paper are still there ahead of it.
        assert!(summary.contains("Ada Lovelace, Alan Turing"), "{summary}");
    }

    /// Reading a kept paper moves its library row along.
    #[test]
    fn reading_a_kept_paper_records_progress_in_the_library() {
        let long = format!(
            "<article>{}</article>",
            "<p>A paragraph of text.</p>".repeat(400)
        );
        let mut runner = AppRunner::new(Arxiv::default());
        let _ = opened_on(&mut runner, &long);
        runner.action(action_id(KEEP));
        for _ in 0..4 {
            runner.action(action_id(kobo_read::action::FORWARD));
        }
        runner.action(kobo_sdk::ActionId::BACK);
        let kept = runner
            .app()
            .library
            .iter()
            .find(|kept| kept.id == paper().id)
            .expect("the paper is not in the library");
        let progress = kept.progress.expect("no progress was recorded");
        assert!(
            progress > 0 && progress < 100,
            "four pages into a long paper read {progress}%"
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
            progress: None,
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

    /// Answers the keeper's request for its index as a reader with nothing
    /// kept yet would.
    fn keeper_ready(runner: &mut AppRunner<Arxiv>) {
        runner.store_result(StoreResult::Loaded {
            key: INDEX_KEY.into(),
            value: None,
        });
    }

    /// The last file written to the shelf, by name and bytes.
    fn written(commands: &[Command]) -> Option<(String, Vec<u8>)> {
        commands.iter().rev().find_map(|command| match command {
            Command::Store(StoreRequest::ShelfWrite { name, bytes, .. }) => {
                Some((name.clone(), bytes.clone()))
            }
            _ => None,
        })
    }

    /// Lets the shelf and the store acknowledge a kept paper's file, and
    /// answers with what the application asked for as they did.
    fn acknowledge(runner: &mut AppRunner<Arxiv>, name: &str, bytes: &[u8]) -> Vec<Command> {
        let mut commands = runner.store_result(StoreResult::ShelfWritten {
            name: name.to_owned(),
            size: u32::try_from(bytes.len()).expect("a small file"),
        });
        commands.extend(runner.store_result(StoreResult::Saved {
            key: INDEX_KEY.into(),
        }));
        commands
    }

    /// One grey pixel, as a whole PNG file.
    fn png() -> Vec<u8> {
        vec![
            137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1,
            8, 0, 0, 0, 0, 58, 126, 155, 85, 0, 0, 0, 10, 73, 68, 65, 84, 120, 156, 99, 104, 0, 0,
            0, 130, 0, 129, 119, 205, 114, 182, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
        ]
    }

    /// Keeping a paper puts the rendering somewhere it can be read again with
    /// no network, and says so in the catalogue, but calls it offline only
    /// once the device has acknowledged the file.
    #[test]
    fn keeping_a_paper_writes_it_to_the_shelf_and_lists_it() {
        let rendering = "<article><h2>1 Introduction</h2><p>Words.</p></article>";
        let mut runner = AppRunner::new(Arxiv::default());
        let _ = opened_on(&mut runner, rendering);
        keeper_ready(&mut runner);

        let commands = runner.action(action_id(KEEP));
        assert!(
            runner.app().is_kept(&paper().id),
            "the paper was not listed in the library"
        );
        assert!(
            !runner.app().is_offline(&paper().id),
            "the paper was called offline before anything was written"
        );
        let (name, bytes) = written(&commands).expect("nothing was written to the shelf");
        let kept = KeptFile::decode(&bytes).expect("the file is a whole paper");
        assert!(kept.html.contains("Words."));

        // And the catalogue goes down too, or the library is empty next time.
        let listed = commands.iter().any(|command| {
            matches!(
                command,
                Command::Store(kobo_sdk::StoreRequest::Save { key, .. }) if key == LIBRARY_KEY
            )
        });
        assert!(listed, "the catalogue was not saved");

        acknowledge(&mut runner, &name, &bytes);
        assert!(runner.app().is_offline(&paper().id));
        assert!(runner
            .app()
            .listing_summary(&paper())
            .starts_with("offline"));
    }

    /// The figures go onto the reader with the paper, and a kept paper opens
    /// with them from the file, asking the network for nothing.
    #[test]
    fn a_kept_paper_keeps_its_figures_and_opens_with_them_offline() {
        let rendering = "<article><p>Before the figure.</p>\
            <figure><img src=\"2401.00001v2/x1.png\" alt=\"A plot\">\
            <figcaption>Figure 1.</figcaption></figure><p>After.</p></article>";
        let mut runner = AppRunner::new(Arxiv::default());
        let _ = opened_on(&mut runner, rendering);
        keeper_ready(&mut runner);
        let commands = runner.action(action_id(KEEP));
        let figure = commands
            .iter()
            .find_map(|command| match command {
                Command::Spawn {
                    task,
                    work: Task::Fetch { url, .. },
                } if url == "https://arxiv.org/html/2401.00001v2/x1.png" => Some(*task),
                _ => None,
            })
            .expect("keeping the paper did not fetch its figure");
        let commands = runner.task_outcome(figure, TaskOutcome::Completed(png()));
        let (name, bytes) = written(&commands).expect("the paper was not written");
        let kept = KeptFile::decode(&bytes).expect("the file is a whole paper");
        assert_eq!(
            kept.pictures,
            vec![("2401.00001v2/x1.png".to_owned(), png())],
            "the figure was left behind"
        );
        acknowledge(&mut runner, &name, &bytes);
        let library = runner.app().library.clone();
        assert_eq!(
            u64::from(library[0].bytes),
            bytes.len() as u64,
            "the library row does not say what the kept file weighs"
        );

        // A fresh start, with the network gone.
        let mut again = AppRunner::new(Arxiv::default());
        again.start();
        again.store_result(StoreResult::Loaded {
            key: LIBRARY_KEY.into(),
            value: Some(super::encode_library(&library)),
        });
        let index = format!("kept-index-v1\n{}\t{}\n", paper().id, bytes.len());
        again.store_result(StoreResult::Loaded {
            key: INDEX_KEY.into(),
            value: Some(index.into_bytes()),
        });
        let context = again.context();
        let line = format!("{:?}", again.app().library(&context));
        assert!(line.contains("Ready offline: 1 paper"), "{line}");
        again.action(action_id(LIBRARY));
        let commands = again.action(action_id("kept-0"));
        let read = commands
            .iter()
            .any(|command| matches!(command, Command::Store(StoreRequest::ShelfRead { name: read, .. }) if *read == name));
        assert!(read, "the kept file was not read back");
        let size = u32::try_from(bytes.len()).expect("a small file");
        let commands = again.store_result(StoreResult::ShelfRead {
            name: name.clone(),
            offset: 0,
            bytes: bytes.clone(),
            size,
        });
        assert_eq!(again.app().view, View::FullText);
        assert!(
            !commands.iter().any(|command| matches!(
                command,
                Command::Spawn {
                    work: Task::Fetch { .. },
                    ..
                }
            )),
            "a kept figure was asked of the network"
        );
        assert!(
            again.app().book.missing_pictures().is_empty(),
            "the figure did not come out of the kept file"
        );
        let full = again.app().full_text();
        assert_eq!(
            full.top_bar.map(|bar| bar.title).as_deref(),
            Some(paper().title.as_str())
        );
    }

    /// A paper kept before figures were still opens from its old file, and
    /// reading it is the moment it is rewritten with its figures.
    #[test]
    fn a_paper_kept_before_figures_were_still_opens_and_is_rewritten_with_them() {
        let mut runner = AppRunner::new(Arxiv::default());
        runner.start();
        keeper_ready(&mut runner);
        runner.app_mut().library = vec![Kept {
            id: paper().id.clone(),
            title: paper().title.clone(),
            authors: "Ada Lovelace".into(),
            bytes: 40,
            progress: None,
        }];
        runner.action(action_id(LIBRARY));
        let commands = runner.action(action_id("kept-0"));
        assert!(commands.iter().any(|command| matches!(
            command,
            Command::Store(StoreRequest::ShelfRead { name, .. }) if *name == blob_key(&paper().id)
        )));
        let old = b"<article><p>Kept long ago.</p></article>".to_vec();
        let commands = runner.store_result(StoreResult::ShelfRead {
            name: blob_key(&paper().id),
            offset: 0,
            size: u32::try_from(old.len()).expect("small"),
            bytes: old,
        });
        assert_eq!(runner.app().view, View::FullText);
        let (name, bytes) = written(&commands).expect("the old paper was not rewritten");
        let commands = acknowledge(&mut runner, &name, &bytes);
        assert!(runner.app().keeper.is_kept(&paper().id));
        assert!(
            commands.iter().any(|command| matches!(
                command,
                Command::Store(StoreRequest::ShelfRemove { name }) if *name == blob_key(&paper().id)
            )),
            "the old file was left on the shelf beside the new one"
        );
    }

    /// Discarding takes back the space as well as the entry. A library that
    /// forgets a paper but keeps its megabyte is a leak with a nice name.
    #[test]
    fn discarding_a_paper_takes_its_file_off_the_shelf_too() {
        let rendering = "<article><p>Words.</p></article>";
        let mut runner = AppRunner::new(Arxiv::default());
        let _ = opened_on(&mut runner, rendering);
        keeper_ready(&mut runner);
        let commands = runner.action(action_id(KEEP));
        let (name, bytes) = written(&commands).expect("nothing was written");
        acknowledge(&mut runner, &name, &bytes);

        let commands = runner.action(action_id(DISCARD));
        assert!(!runner.app().is_kept(&paper().id), "still listed");
        let removed: Vec<&str> = commands
            .iter()
            .filter_map(|command| match command {
                Command::Store(kobo_sdk::StoreRequest::ShelfRemove { name }) => Some(name.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            removed.contains(&name.as_str()),
            "the kept file was left on the shelf"
        );
    }

    /// Keeping from the abstract fetches the paper rather than refusing.
    #[test]
    fn keeping_from_an_abstract_fetches_the_paper_and_then_keeps_it() {
        let mut runner = AppRunner::new(Arxiv::default());
        runner.app_mut().papers = vec![paper()];
        runner.app_mut().open = Some(0);
        runner.app_mut().view = View::Paper;
        runner.start();
        keeper_ready(&mut runner);

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
        assert!(written(&commands).is_some(), "nothing reached the shelf");
    }

    /// A file the reader refuses takes the paper back out of the library
    /// and says why, rather than leaving a row with nothing behind it.
    #[test]
    fn a_paper_the_reader_has_no_room_for_leaves_the_library_and_says_so() {
        let mut runner = AppRunner::new(Arxiv::default());
        let _ = opened_on(&mut runner, "<article><p>Words.</p></article>");
        keeper_ready(&mut runner);
        let commands = runner.action(action_id(KEEP));
        let (name, _) = written(&commands).expect("nothing was written");
        let mut context = runner.context();
        runner.app_mut().answered(
            &mut context,
            super::Asked::Shelf,
            &name,
            StoreResult::Denied(kobo_sdk::StoreError::NoRoom),
        );
        assert!(!runner.app().is_kept(&paper().id));
        let trouble = runner.app().trouble.clone().unwrap_or_default();
        assert!(trouble.contains("no room"), "{trouble}");
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
    /// The titles of a screen's rows, top to bottom.
    fn row_titles(screen: &kobo_sdk::Screen) -> Vec<String> {
        let mut titles = Vec::new();
        for node in &screen.nodes {
            if let kobo_sdk::Node::Rows { rows, .. } = node {
                titles.extend(rows.iter().map(|row| row.title.clone()));
            }
        }
        titles
    }

    /// Saved searches and followed subjects ride the store the way the
    /// library does: a line each, tabs scrubbed, the cap honest.
    #[test]
    fn saved_lists_survive_the_round_trip_through_the_store() {
        let saved = vec!["deep learning".to_owned(), "attention".to_owned()];
        assert_eq!(decode_list(&encode_list(&saved)), saved);

        let over: Vec<String> = (0..MAX_SAVED + 4).map(|n| format!("search {n}")).collect();
        assert_eq!(decode_list(&encode_list(&over)).len(), MAX_SAVED);

        // A tab in a phrase would forge a second line on the way back, so it
        // is a space before it ever reaches the store.
        let tabbed = vec!["a\tphrase\twith\ttabs".to_owned()];
        assert_eq!(
            decode_list(&encode_list(&tabbed)),
            vec!["a phrase with tabs".to_owned()]
        );
    }

    /// Both lists come back when the store answers, which is the whole point
    /// of writing them down.
    #[test]
    fn saved_lists_come_back_when_the_store_answers() {
        let mut runner = AppRunner::new(Arxiv::default());
        runner.start();
        runner.store_result(StoreResult::Loaded {
            key: SEARCHES_KEY.to_owned(),
            value: Some(encode_list(&["deep learning".to_owned()])),
        });
        runner.store_result(StoreResult::Loaded {
            key: FOLLOWED_KEY.to_owned(),
            value: Some(encode_list(&["cs.LG".to_owned()])),
        });
        assert_eq!(runner.app().saved, vec!["deep learning".to_owned()]);
        assert_eq!(runner.app().followed, vec!["cs.LG".to_owned()]);
    }

    /// Saving the listing's search writes it to the store once, and Saved
    /// runs it again without the keyboard.
    #[test]
    fn a_saved_search_is_stored_once_and_runs_again_from_saved() {
        let mut runner = AppRunner::new(Arxiv::default());
        runner.start();
        runner.app_mut().view = View::Listing;
        runner.app_mut().papers = vec![paper()];
        runner.app_mut().query = Some(Query::Words("deep learning".into()));

        let commands = runner.action(action_id(SAVE_SEARCH));
        assert_eq!(runner.app().saved, vec!["deep learning".to_owned()]);
        assert!(
            commands.iter().any(|command| matches!(
                command,
                Command::Store(StoreRequest::Save { key, value })
                    if key == SEARCHES_KEY && value == &encode_list(&["deep learning".to_owned()])
            )),
            "the saved list was not written out"
        );

        // Saving what is already saved keeps one of it.
        let _ = runner.action(action_id(SAVE_SEARCH));
        assert_eq!(runner.app().saved.len(), 1);

        let _ = runner.action(action_id(SAVED));
        assert_eq!(runner.app().view, View::Saved);
        let commands = runner.action(action_id(&format!("{SSEARCH}0")));
        assert_eq!(runner.app().view, View::Listing);
        assert!(
            matches!(&runner.app().query, Some(Query::Words(words)) if words == "deep learning"),
            "the saved search did not come back as the query"
        );
        assert!(
            commands.iter().any(|command| matches!(
                command,
                Command::Spawn {
                    work: Task::Fetch { url, .. },
                    ..
                } if url.contains("deep%20learning")
            )),
            "the saved search asked arXiv for nothing"
        );
    }

    /// Following pins the subject to the top of the list with its mark, and
    /// unfollowing puts the catalogue's order back.
    #[test]
    fn a_followed_subject_leads_the_subject_list() {
        let last = SUBJECTS.len() - 1;
        let (code, name, _) = SUBJECTS[last];
        let mut runner = AppRunner::new(Arxiv::default());
        runner.start();
        runner.app_mut().view = View::Listing;
        runner.app_mut().papers = vec![paper()];
        runner.app_mut().query = Some(Query::Subject {
            code: code.into(),
            name: name.into(),
        });

        let commands = runner.action(action_id(FOLLOW));
        assert_eq!(runner.app().followed, vec![code.to_owned()]);
        assert!(
            commands.iter().any(|command| matches!(
                command,
                Command::Store(StoreRequest::Save { key, .. }) if key == FOLLOWED_KEY
            )),
            "the followed list was not written out"
        );

        let _ = runner.action(ActionId::BACK);
        assert_eq!(runner.app().view, View::Subjects);
        let context = runner.context();
        let screen = runner.app().subjects(&context);
        assert_eq!(
            row_titles(&screen).first().map(String::as_str),
            Some(name),
            "the followed subject was not pinned to the top"
        );

        let _ = runner.action(action_id(UNFOLLOW));
        assert!(runner.app().followed.is_empty());
        let context = runner.context();
        let screen = runner.app().subjects(&context);
        assert_eq!(
            row_titles(&screen).first().map(String::as_str),
            Some(GROUPS[0]),
            "the headings did not come back to the top"
        );
    }

    /// A full list says so rather than dropping the oldest or growing past
    /// what the rows can hold.
    #[test]
    fn a_full_saved_list_says_so_instead_of_growing_quietly() {
        let mut runner = AppRunner::new(Arxiv::default());
        runner.start();
        runner.app_mut().saved = (0..MAX_SAVED).map(|n| format!("search {n}")).collect();
        runner.app_mut().view = View::Listing;
        runner.app_mut().papers = vec![paper()];
        runner.app_mut().query = Some(Query::Words("one more".into()));

        let _ = runner.action(action_id(SAVE_SEARCH));
        assert_eq!(runner.app().saved.len(), MAX_SAVED);
        assert!(
            runner
                .app()
                .trouble
                .as_deref()
                .is_some_and(|trouble| trouble.contains("hold")),
            "the cap was hit without a word"
        );
    }

    /// Manage turns a row into the removal of itself, and Done turns the
    /// rows back into shortcuts.
    #[test]
    fn manage_removes_and_done_restores() {
        let mut runner = AppRunner::new(Arxiv::default());
        runner.start();
        runner.app_mut().saved = vec!["alpha".to_owned(), "beta".to_owned()];

        let _ = runner.action(action_id(SAVED));
        let _ = runner.action(action_id(MANAGE));
        assert!(runner.app().managing);
        let commands = runner.action(action_id(&format!("{SSEARCH}0")));
        assert_eq!(runner.app().saved, vec!["beta".to_owned()]);
        assert!(
            commands.iter().any(|command| matches!(
                command,
                Command::Store(StoreRequest::Save { key, value })
                    if key == SEARCHES_KEY && value == &encode_list(&["beta".to_owned()])
            )),
            "the removal was not written out"
        );
        let _ = runner.action(action_id(DONE));
        assert!(!runner.app().managing);
    }

    /// Live search phrases run long, and a row whose text does not fit is a
    /// screen the renderer refuses outright -- so the saved list is checked
    /// at live length, not fixture length.
    #[test]
    fn a_live_length_saved_search_does_not_overflow_its_row() {
        let mut runner = AppRunner::new(Arxiv::default());
        runner.app_mut().saved = vec![
            "Comparative Analysis of Transformer-Based Language Models and Bayesian Deep Learning at Scale"
                .to_owned(),
        ];
        runner.app_mut().followed = vec![SUBJECTS[0].0.to_owned()];
        let context = runner.context();
        let screen = runner.app().saved(&context);
        let issues = screen.validate(&kobo_sdk::CLARA_BW_METRICS);
        assert!(
            !issues
                .iter()
                .any(|issue| matches!(issue.kind, kobo_sdk::LayoutIssueKind::TextOverflow)),
            "the saved rows overflowed under a live-length phrase: {issues:?}"
        );

        let context = runner.context();
        let screen = runner.app().subjects(&context);
        let issues = screen.validate(&kobo_sdk::CLARA_BW_METRICS);
        assert!(
            !issues
                .iter()
                .any(|issue| matches!(issue.kind, kobo_sdk::LayoutIssueKind::TextOverflow)),
            "the subject list overflowed with a followed subject pinned: {issues:?}"
        );
    }
}

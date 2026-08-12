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
    Task, TaskError, TaskId, TaskOutcome,
};
use std::collections::VecDeque;
use std::fmt::Write as _;
use std::process::ExitCode;

/// The export API, which is the interface arXiv asks robots to use.
const QUERY: &str = "https://export.arxiv.org/api/query";

/// How many papers one listing fetch asks for.
const PAGE: usize = 25;

/// The ceiling on a listing. Twenty-five abstracts is well under this; the
/// margin is for a query that matches papers with long author lists.
const LISTING_BYTES: u32 = 512 * 1024;

/// The ceiling on one paper's full text.
///
/// A rendered paper is mostly markup, and the ones that overrun this are
/// review articles with four hundred references. Truncation is reported on the
/// page rather than hidden, because a paper that simply stops is otherwise
/// indistinguishable from one that ended.
const FULL_TEXT_BYTES: u32 = 768 * 1024;

/// The most figures fetched for one paper.
///
/// A rendered paper's figures are separate files, so each one is a fetch of
/// its own over a connection that is somebody's home wifi at best. A typical
/// paper has half a dozen; a survey with every experiment plotted has thirty,
/// and the reader is past the ones that matter long before the last arrives.
const MAX_FIGURES: usize = 24;

/// The ceiling on one figure.
///
/// A plot is tens of kilobytes. The ones that overrun this are photographs at
/// print resolution, which the panel cannot show anyway: it is sixteen greys
/// on a screen narrower than the figure's own caption.
const FIGURE_BYTES: u32 = 512 * 1024;

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

/// What a listing is a listing of.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Query {
    /// Everything newest-first in one subject.
    Subject { code: String, name: String },
    /// Whatever somebody typed.
    Words(String),
}

impl Query {
    /// The `search_query` arXiv expects.
    fn expression(&self) -> String {
        match self {
            Self::Subject { code, .. } => format!("cat:{}", escape(code)),
            Self::Words(words) => format!("all:{}", escape(words)),
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
    /// Figures named by the rendering and not yet fetched, in the order the
    /// paper refers to them.
    ///
    /// A web page carries no pictures, only the addresses of some. An EPUB
    /// hands its plates over with the text and `kobo-doc` will not touch the
    /// network, so fetching these is this application's job and nobody
    /// else's.
    figures: VecDeque<String>,
    /// The figure fetch in flight, and the name it will fill.
    ///
    /// Its own slot rather than the one every other fetch shares: a figure
    /// arriving must never be the reason a listing or a paper is lost.
    figure: Option<(TaskId, String)>,
    /// Whether the full text arrived cut off at the byte ceiling.
    truncated: bool,
    task: Option<(TaskId, Awaiting)>,
    trouble: Option<String>,
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
            query.expression()
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
            .page_turns(SUBJECTS_BACK, SUBJECTS_NEXT)
            .page_position(page_number(page), page_total(pages.len()))
            .bottom_action_marked(SEARCH, "Search arXiv", Glyph::Search)
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
        if self.papers.is_empty() {
            return screen
                .empty_state("No papers here. Try different words.")
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
        screen
            .fill()
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
        let body = paper_body(html);
        // Parsed rather than flattened. What comes back is the paper's own
        // structure -- its sections, its emphasis, its figures and their
        // captions -- which is what the reader needs in order to set it as a
        // document rather than as one long paragraph.
        let mut document = kobo_doc::html::parse(body);
        if document.blocks.is_empty() {
            self.trouble =
                Some("arXiv has no readable rendering of this paper, only a PDF.".to_owned());
            return;
        }
        // A rendering cut off at the byte ceiling has no closing tag, which is
        // the honest signal: the fetched bytes are markup, and a paper can sit
        // at the transport limit with every word of it delivered.
        //
        // Told to the document rather than kept here, because the reader
        // already says this on the last page and says it better: a paper that
        // simply stops is otherwise indistinguishable from one that ended.
        self.truncated = !body.contains("</article>");
        document.truncated |= self.truncated;
        self.book.open(context, document, Memory::default());
        self.page = 0;
        self.view = View::FullText;
        // The addresses of the figures, which arrived as addresses and not as
        // pictures. The paper is readable now; these fill in behind it.
        self.figures = self
            .book
            .missing_pictures()
            .into_iter()
            .take(MAX_FIGURES)
            .collect();
        self.next_figure(context);
    }

    /// Asks for the next figure the paper named.
    ///
    /// One at a time. A paper with thirty plots is thirty fetches, and asking
    /// for all of them at once is how an application uses up every lane the
    /// runtime has and leaves nothing for the next listing.
    fn next_figure(&mut self, context: &mut Context) {
        if self.figure.is_some() {
            return;
        }
        let Some(paper) = self.paper() else {
            return;
        };
        let base = format!("https://arxiv.org/html/{}/", escape_path(&paper.id));
        while let Some(name) = self.figures.pop_front() {
            let Some(url) = figure_url(&base, &name) else {
                continue;
            };
            // Not retrying: a figure is worth one attempt. The paper is
            // already readable without it, and a plot that will not come is
            // not a reason to spend the connection on it three times.
            if let Some(task) = context.spawn(Task::Fetch {
                url,
                offset: 0,
                max_bytes: FIGURE_BYTES,
                credential: None,
                headers: Vec::new(),
            }) {
                self.figure = Some((task, name));
                return;
            }
            // No lane free. What is left stays queued, and the next page turn
            // asks again.
            self.figures.push_front(name);
            return;
        }
        // Nothing left to ask for, so whatever did arrive is measured in.
        self.settle_figures(context);
    }

    /// Measures every figure that arrived, once.
    ///
    /// Once rather than on each arrival: measuring is what decides where the
    /// pages break, and doing it twelve times as twelve plots land would move
    /// the text under somebody who is reading it. They keep their place across
    /// it either way -- the reader remembers a paragraph, not a page number --
    /// but the paragraph should not walk up the screen twelve times.
    fn settle_figures(&mut self, context: &mut Context) {
        if self.book.settle_pictures(context) {
            self.show(context);
        }
    }

    fn took_figure(&mut self, context: &mut Context, name: &str, bytes: Vec<u8>) {
        self.book.provide_picture(name, bytes);
        self.next_figure(context);
    }

    /// Gives back everything the open paper was costing.
    fn close_paper(&mut self, context: &mut Context) {
        self.book.close(context);
        self.figures.clear();
        // The fetch in flight is left to land and be ignored: cancelling it
        // would cost a message to save nothing, and `provide_picture` refuses
        // a name no open document mentions.
        self.figure = None;
        self.truncated = false;
    }
}

/// Resolves a figure's address against the paper it was named in.
///
/// `LaTeXML` writes figures as `x1.png`, relative to the rendering's own
/// address. A rendering is a web page before it is a paper, and its author
/// wrote it: a figure that names another host would have the device announce
/// itself to whoever the author chose, or knock on whatever else answers on
/// the reader's own network. So an absolute address is taken only when it is
/// arXiv's own, and everything else has to be a name inside the paper.
fn figure_url(base: &str, name: &str) -> Option<String> {
    if name.is_empty() || name.len() > 512 {
        return None;
    }
    if let Some(rest) = name.strip_prefix("https://") {
        let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
        // A port or credentials before the host are ways of writing an
        // address that reads as arXiv's and is not, so neither is allowed.
        let arxiv = host == "arxiv.org" || host.ends_with(".arxiv.org");
        return if arxiv { Some(name.to_owned()) } else { None };
    }
    // Not http, not a data URI, not a protocol-relative address: this
    // application fetches over TLS and nothing else, and a figure is not worth
    // making an exception for.
    if name.contains("://") || name.starts_with("//") || name.starts_with("data:") {
        return None;
    }
    // A rooted path leaves the paper's own directory, which no `LaTeXML`
    // rendering does, and a walk upwards out of it is a thing only a hostile
    // page tries. The walk is also refused when it is spelled in percent
    // escapes or with a backslash, because the address is joined as text here
    // and resolved as a path somewhere else.
    let escaped = name.to_ascii_lowercase();
    if name.starts_with('/')
        || name.contains('\\')
        || escaped.contains("%2e")
        || escaped.contains("%2f")
        || name.split('/').any(|part| part == "..")
    {
        return None;
    }
    Some(format!("{base}{name}"))
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
                View::Search | View::Listing => {
                    self.view = View::Subjects;
                    self.listing_page = 0;
                }
                View::Paper => {
                    self.view = View::Listing;
                    self.open = None;
                }
                // The full text was reached from the abstract, so Back is the
                // abstract rather than the list two steps behind it. Leaving
                // is also the moment the paper stops costing anything: the
                // document, the picture handles the runtime is holding
                // against it and the figures still queued for it all go now,
                // rather than lingering for a paper nobody is reading.
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
                    Outcome::Close => {
                        self.close_paper(context);
                        self.view = View::Paper;
                        self.open_abstract(context);
                    }
                    Outcome::Light(level) => context.device().set_frontlight(level),
                    Outcome::Elsewhere | Outcome::Repaint | Outcome::Save => {}
                }
                // A page turn is also the moment the figures on the page
                // turned to are wanted, and a lane may have come free since
                // the last one was asked for.
                self.next_figure(context);
                self.show(context);
                return;
            }
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

        for index in 0..self.papers.len() {
            if action == action_id(&format!("{PAPER}{index}")) {
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
        match self.book.woke(context, task) {
            Step::Elsewhere => {}
            Step::Quiet => return,
            Step::Repaint => {
                self.show(context);
                return;
            }
        }

        if self.figure.as_ref().is_some_and(|(id, _)| *id == task) {
            let (_, name) = self.figure.take().expect("just checked");
            match outcome {
                TaskOutcome::Completed(bytes) => self.took_figure(context, &name, bytes),
                // A figure that will not come is a figure the paper reads
                // without: the reader draws what the caption said it shows,
                // which is what a document with a missing plate should look
                // like. Nothing is said on screen about it.
                TaskOutcome::Failed(_) | TaskOutcome::Cancelled => self.next_figure(context),
            }
            return;
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
        abstract_text, escape, escape_path, figure_url, paper_body, row_summary, Arxiv, Query,
        View, ABSTRACT, FULL_TEXT, SUBJECTS,
    };
    use crate::atom::Paper;
    use kobo_sdk::{action_id, ActionId, AppRunner, Command, Task, TaskOutcome};

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
        assert_eq!(subject.expression(), "cat:cs.LG");
        assert_eq!(Query::Words("qubits".into()).expression(), "all:qubits");
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
            "<article><p>A paper.</p><img src=\"x1.png\" alt=\"A plot\"></article>",
        );

        let (_, name) = runner
            .app()
            .figure
            .clone()
            .expect("the figure was never asked for");
        assert_eq!(name, "x1.png");
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
            "the figure was not asked for beside its paper: {asked:?}"
        );
    }

    /// A rendering can name anything at all, and most of it is not a figure
    /// this application is willing to go and get.
    #[test]
    fn a_figure_address_that_leaves_the_paper_is_refused() {
        let base = "https://arxiv.org/html/2401.00001v2/";
        assert_eq!(
            figure_url(base, "x1.png").as_deref(),
            Some("https://arxiv.org/html/2401.00001v2/x1.png")
        );
        assert_eq!(
            figure_url(base, "https://arxiv.org/html/2401.00001v2/x1.png").as_deref(),
            Some("https://arxiv.org/html/2401.00001v2/x1.png")
        );
        for hostile in [
            "",
            "/etc/passwd",
            "../../../secrets.png",
            "..%2f..%2fsecrets.png",
            "%2e%2e/secrets.png",
            "..\\secrets.png",
            "http://example.org/x1.png",
            "//example.org/x1.png",
            "data:image/png;base64,AAAA",
            // Another host is another host however it is spelled, and a
            // rendering is written by whoever wrote the paper.
            "https://example.org/x1.png",
            "https://arxiv.org.example.org/x1.png",
            "https://evil.org/?a=arxiv.org",
        ] {
            assert!(
                figure_url(base, hostile).is_none(),
                "{hostile:?} was allowed"
            );
        }
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
                runner.app().figures.is_empty() && runner.app().figure.is_none(),
                "figures were still being fetched for a paper nobody is reading"
            );
        }
    }
}

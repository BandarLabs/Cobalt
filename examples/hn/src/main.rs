//! Hacker News, on a panel with no scrollbar and no keyboard.
//!
//! Four tabs along the bottom — Top, New, Ask, Show — and a comment thread
//! behind every story. Nothing animates, nothing scrolls, and nothing moves
//! under a finger that is already reaching for it.
//!
//! ## Why Algolia rather than the official API
//!
//! Hacker News' own Firebase API returns one item per request. A story with
//! four hundred replies is four hundred and one requests, which on a device
//! whose radio is the largest single draw on the battery is not a design, it
//! is a way to flatten a charge. Algolia's `items/:id` returns the entire
//! thread, nested, in one. That single fact is the reason this application is
//! possible at all.
//!
//! ## What happens to a thread that does not fit
//!
//! The transport carries half a megabyte — `MAX_TASK_BYTES_U32` in
//! `kobo-protocol`, well under the 1 MiB `MAX_FRAME_LEN` that carries it — and
//! a busy thread is comfortably more. A real one measured while writing this
//! was 734 KB for 925 comments. Algolia ignores `Range`, so the trick that
//! lets Gutenshelf read a novel in pieces does not work here: asking for the
//! second half returns the whole document again and the ceiling rejects it.
//!
//! So the request comes back [`TaskError::TooLarge`], and rather than showing
//! a dead end this asks a different question — `search_by_date` over that
//! story's comments, thirty at a time, which is bounded by construction. The
//! nesting is gone in that answer, so the screen *says* the nesting is gone.
//! What never happens is a thread that silently stops halfway, or one that
//! reads as complete when a third of it is missing.

mod html;
mod model;

use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, KoboApp, Screen, ScreenBuilder, Task, TaskError,
    TaskId, TaskOutcome,
};
use model::{Comment, Story};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

/// The read-only front end to Hacker News' own index.
const API: &str = "https://hn.algolia.com/api/v1";

/// How many stories a tab asks for.
///
/// One screenful is six or seven rows, so this is four or five page turns
/// deep. More would be a longer wait on the radio for pages nobody reaches.
const HITS: u32 = 30;

/// How far back the ranked tabs look, in seconds.
///
/// A week. Ask HN and Show HN are ranked rather than chronological, so without
/// a window they return the best of all time and the tab becomes a museum. A
/// day is too short — a good Ask HN thread collects answers for longer than
/// that, and a quiet Tuesday would leave the tab half empty — and a month is
/// long enough that the top of the list stops changing.
const RECENT_SECONDS: i64 = 7 * 24 * 60 * 60;

/// How many lines of a list row a headline may take.
///
/// Measured against a real front page: at one line, three of thirty headlines
/// ended in an ellipsis; at two lines, none did. So the cost is three rows in
/// thirty standing a line taller than their neighbours, and the gain is three
/// headlines that say what they are about. Pagination measures every row
/// individually, so a ragged column costs nothing but the raggedness.
const TITLE_LINES: usize = 2;

/// How much of a list response to accept.
///
/// A thirty-hit list is around 31 KB, and an Ask HN list is larger because
/// each hit carries the whole question. This is generous headroom over both
/// and still a small fraction of the ceiling.
const LIST_BYTES: u32 = 256 * 1024;

/// How much of a thread to accept.
///
/// The transport ceiling exactly. Asking for more is silently clamped to it by
/// the protocol, and asking for less would refuse threads that do in fact fit.
const THREAD_BYTES: u32 = 512 * 1024;

/// How many placeholder rows stand in for a list while it is arriving.
const SKELETON_ROWS: u8 = 6;

/// The bottom bar on a thread. Identical while the comments are arriving and
/// after they have, so nothing under the reader's finger moves.
const THREAD_BAR: [(&str, &str); 3] = [
    ("thread-back", "Back"),
    ("stories", "Stories"),
    ("thread-next", "Next"),
];

/// The bottom bar. Fixed, in this order, on every screen that has a list.
const TABS: [(&str, &str); 4] = [
    ("tab-top", "Top"),
    ("tab-new", "New"),
    ("tab-ask", "Ask"),
    ("tab-show", "Show"),
];

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Tab {
    #[default]
    Top,
    New,
    Ask,
    Show,
}

impl Tab {
    const ALL: [Self; 4] = [Self::Top, Self::New, Self::Ask, Self::Show];

    const fn index(self) -> usize {
        match self {
            Self::Top => 0,
            Self::New => 1,
            Self::Ask => 2,
            Self::Show => 3,
        }
    }

    const fn label(self) -> &'static str {
        TABS[self.index()].1
    }

    const fn action(self) -> &'static str {
        TABS[self.index()].0
    }

    /// What is being waited for, said as a sentence rather than as a noun.
    const fn waiting(self) -> &'static str {
        match self {
            Self::Top => "Fetching the front page",
            Self::New => "Fetching the newest stories",
            Self::Ask => "Fetching Ask HN",
            Self::Show => "Fetching Show HN",
        }
    }

    /// Where this tab's stories come from, asked as of `now`.
    ///
    /// The time window is the whole reason this takes an argument. Algolia's
    /// ranked search is ranked over *all of Hacker News, forever*, so asking
    /// it for `ask_hn` returns the best Ask HN threads of the last fifteen
    /// years: this application shipped with a ten-year-old post at the top of
    /// its Ask tab and a nine-year-old one below it, which is a perfectly good
    /// answer to a question nobody asked. Hacker News' own Ask and Show pages
    /// rank recent submissions, so these do too.
    ///
    /// `Top` needs no window because `front_page` is by definition what is on
    /// the front page now, and `New` needs none because it is ordered by date
    /// rather than ranked.
    fn url(self, now: i64) -> String {
        let tags = match self {
            Self::Top => "front_page",
            Self::New => "story",
            Self::Ask => "ask_hn",
            Self::Show => "show_hn",
        };
        // `New` is the only one that wants chronological order; the rest are
        // ranked, which is what "front page" means.
        let endpoint = if matches!(self, Self::New) {
            "search_by_date"
        } else {
            "search"
        };
        let window = match self {
            Self::Ask | Self::Show => {
                let since = now.saturating_sub(RECENT_SECONDS);
                format!("&numericFilters=created_at_i%3E{since}")
            }
            Self::Top | Self::New => String::new(),
        };
        format!("{API}/{endpoint}?tags={tags}&hitsPerPage={HITS}{window}")
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum View {
    #[default]
    List,
    Thread,
}

/// What the outstanding request is for.
///
/// Only ever one. A tab tapped twice while the first answer is in the air
/// would otherwise land two lists on the panel in an order decided by the
/// network.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Awaiting {
    List(Tab),
    /// The whole thread, nested, in one request.
    Thread,
    /// One page of the flat fallback, for a thread too large to fetch whole.
    FlatPage,
}

#[derive(Default)]
struct Hn {
    tab: Tab,
    view: View,
    stories: Vec<Story>,
    /// Which story rows belong on each page, measured against this panel.
    pages: Vec<Vec<usize>>,
    page: usize,
    /// The open story, as an index into `stories`.
    open: Option<usize>,
    comments: Vec<Comment>,
    /// The thread broken into pages of paragraphs that fit this panel.
    thread_pages: Vec<Vec<(u8, String)>>,
    /// Each story's title cut to the one line a row can show, measured against
    /// this panel rather than guessed at by character count.
    titles: Vec<String>,
    thread_page: usize,
    /// Set once a thread has been found too large to fetch whole.
    flat: bool,
    /// How many pages of flat comments have already been taken.
    flat_taken: u32,
    /// How many the fallback says there are.
    flat_pages: u32,
    /// What the screen has to admit about what it is showing.
    note: Option<String>,
    /// The device clock at the last answer, for relative ages.
    now: i64,
    task: Option<(TaskId, Awaiting)>,
    problem: Option<String>,
}

impl Hn {
    fn show(&self, context: &mut Context) {
        let screen = match self.view {
            View::List => self.list(),
            View::Thread => self.thread(),
        };
        context.set_screen(screen);
    }

    /// The story list for the tab that is showing.
    fn list(&self) -> Screen {
        let mut screen = ScreenBuilder::new("hn").top_bar(self.list_title());
        if let Some(problem) = &self.problem {
            screen = screen.banner(BannerLevel::Attention, problem.clone());
        }
        if matches!(self.task, Some((_, Awaiting::List(_)))) {
            // A skeleton rather than a spinner, because there are no spinners
            // here: every frame is a full panel refresh. A list-shaped
            // placeholder puts the rows where the eye is already looking.
            return self
                .with_tabs(
                    screen
                        .activity(self.tab.waiting(), None)
                        .skeleton(SKELETON_ROWS),
                )
                .build();
        }
        let Some(indices) = self.pages.get(self.page) else {
            return self
                .with_tabs(
                    screen
                        .text(
                            "Nothing came back for this tab. That is usually the network \
                             rather than Hacker News.",
                        )
                        .button("retry", "Try again"),
                )
                .build();
        };
        let rows = indices.iter().filter_map(|index| {
            let story = self.stories.get(*index)?;
            Some((
                format!("story-{index}"),
                self.titles.get(*index).cloned().unwrap_or_default(),
                model::summary(story, self.now),
                // The story's position, which is what Hacker News itself puts
                // here. An icon would be the same icon thirty times over: a
                // list of stories does not need to be told it is a list of
                // stories, and the rank says where you are in the tab and how
                // far down the page turns have carried you.
                u16::try_from(index + 1).unwrap_or(u16::MAX),
            ))
        });
        // Tapping the side of the panel turns the page, which is how every
        // Kobo has always worked. The bottom bar is spent on the tabs — those
        // are places, and places outrank controls for that bar — so the
        // visible page control is the one action the top bar allows.
        self.with_tabs(screen.rows(rows).page_turns("list-back", "list-next"))
            .build()
    }

    fn list_title(&self) -> String {
        if self.pages.len() > 1 {
            format!(
                "{} \u{b7} {} of {}",
                self.tab.label(),
                self.page + 1,
                self.pages.len()
            )
        } else {
            self.tab.label().to_owned()
        }
    }

    /// Adds the bottom bar, and the forward page control the bar has no room
    /// for.
    fn with_tabs(&self, screen: ScreenBuilder) -> ScreenBuilder {
        let screen = if self.pages.len() > 1 {
            screen.top_bar_action("list-next", "Next")
        } else {
            screen
        };
        screen.nav_bar(Some(self.tab.index()), TABS)
    }

    /// The open story and its comments, one measured page at a time.
    fn thread(&self) -> Screen {
        let Some(story) = self.open.and_then(|index| self.stories.get(index)) else {
            return self.list();
        };
        let mut screen = ScreenBuilder::new("hn-thread").top_bar(story.title.clone());
        if matches!(self.task, Some((_, Awaiting::Thread | Awaiting::FlatPage)))
            && self.comments.is_empty()
        {
            // The same bar as the loaded thread, rather than a smaller one
            // that grows when the comments land: a control that moves out from
            // under a finger on a panel this slow is a tap the reader watches
            // miss. A bar of one destination is also refused by the wire,
            // which is how this was found.
            return screen
                .activity("Fetching the comments", None)
                .skeleton(SKELETON_ROWS)
                .nav_bar(None, THREAD_BAR)
                .build();
        }
        for (depth, paragraph) in self
            .thread_pages
            .get(self.thread_page)
            .into_iter()
            .flatten()
        {
            screen = screen.quote(*depth, paragraph.clone());
        }
        if let Some(problem) = &self.problem {
            screen = screen.banner(BannerLevel::Attention, problem.clone());
        }
        // Pinned rather than at the end of the flow: content stops at the bar,
        // so a page that runs long loses its last sentence instead of the only
        // way off the page.
        screen
            .page_turns("thread-back", "thread-next")
            .nav_bar(None, THREAD_BAR)
            .build()
    }

    /// Everything the thread screen draws, as paragraphs carrying their depth.
    ///
    /// Built as prose rather than as nodes so that the runtime's own wrapping
    /// and line height decide where the folds are. Anything drawn outside this
    /// — a banner, say — is not measured, and on this panel what is not
    /// measured is what silently falls off the bottom.
    ///
    /// Depth travels with each paragraph because an indented paragraph has a
    /// narrower measure: a thread paginated flat and then drawn indented would
    /// lose the bottom of nearly every page.
    fn thread_paragraphs(&self) -> Vec<(u8, String)> {
        let Some(story) = self.open.and_then(|index| self.stories.get(index)) else {
            return Vec::new();
        };
        let mut paragraphs = vec![
            (0, story.title.clone()),
            (0, model::summary(story, self.now)),
        ];
        if let Some(note) = &self.note {
            // Inside the flow, not in a banner. A banner is chrome the
            // paginator never measured, so it would push the last paragraph of
            // every page off the panel.
            paragraphs.push((0, note.clone()));
        }
        if let Some(body) = &story.text {
            paragraphs.push((0, body.clone()));
        }
        if self.comments.is_empty() {
            paragraphs.push((0, "No comments yet.".to_owned()));
            return paragraphs;
        }
        for comment in &self.comments {
            let indent = comment.indent();
            paragraphs.push((indent, comment.byline(self.now)));
            for body in comment.body.split("\n\n") {
                if !body.trim().is_empty() {
                    paragraphs.push((indent, body.to_owned()));
                }
            }
        }
        paragraphs
    }

    /// Drops whatever is already on its way.
    ///
    /// Exactly one request is ever outstanding. A reader who taps three tabs
    /// while the first is in the air is asking for the third one, and letting
    /// all three run would land three lists on the panel in an order the
    /// network chose — each one a full refresh the reader watches happen.
    fn cancel_outstanding(&mut self, context: &mut Context) {
        if let Some((task, _)) = self.task.take() {
            context.cancel(task);
        }
    }

    fn ask_list(&mut self, context: &mut Context) {
        self.cancel_outstanding(context);
        self.problem = None;
        match context.spawn(Task::Fetch {
            url: self.tab.url(self.now),
            offset: 0,
            max_bytes: LIST_BYTES,
        }) {
            Some(task) => self.task = Some((task, Awaiting::List(self.tab))),
            None => self.problem = Some("Too much is already in flight.".to_owned()),
        }
    }

    /// Asks for one whole thread, nested, in a single request.
    fn ask_thread(&mut self, context: &mut Context) {
        let Some(id) = self.open_id().map(str::to_owned) else {
            self.problem = Some("That story has no thread to open.".to_owned());
            return;
        };
        self.cancel_outstanding(context);
        self.problem = None;
        match context.spawn(Task::Fetch {
            url: format!("{API}/items/{id}"),
            offset: 0,
            max_bytes: THREAD_BYTES,
        }) {
            Some(task) => self.task = Some((task, Awaiting::Thread)),
            None => self.problem = Some("Too much is already in flight.".to_owned()),
        }
    }

    /// Asks for the next page of the flat fallback.
    fn ask_flat_page(&mut self, context: &mut Context) {
        let Some(id) = self.open_id().map(str::to_owned) else {
            return;
        };
        self.cancel_outstanding(context);
        let page = self.flat_taken;
        match context.spawn(Task::Fetch {
            url: format!(
                "{API}/search_by_date?tags=comment,story_{id}&hitsPerPage={HITS}&page={page}"
            ),
            offset: 0,
            max_bytes: LIST_BYTES,
        }) {
            Some(task) => self.task = Some((task, Awaiting::FlatPage)),
            None => self.problem = Some("Too much is already in flight.".to_owned()),
        }
    }

    /// The open story's identifier, refused unless it is a plain number.
    ///
    /// It arrives from the network and goes straight into a URL, which is the
    /// one place a value from a stranger becomes a request this device makes.
    /// Hacker News item numbers are integers; anything else is somebody else's
    /// idea of what this application should fetch.
    fn open_id(&self) -> Option<&str> {
        let story = self.open.and_then(|index| self.stories.get(index))?;
        let id = story.id.as_str();
        (!id.is_empty() && id.len() <= 20 && id.bytes().all(|byte| byte.is_ascii_digit()))
            .then_some(id)
    }

    fn open_story(&mut self, context: &mut Context, index: usize) {
        self.open = Some(index);
        self.view = View::Thread;
        // A different story, so nothing about the last one survives. A thread
        // left in place would be drawn under the new title for the second it
        // takes the request to come back.
        self.comments.clear();
        self.thread_pages.clear();
        self.thread_page = 0;
        self.flat = false;
        self.flat_taken = 0;
        self.flat_pages = 0;
        self.note = None;
        self.problem = None;
        self.ask_thread(context);
        self.show(context);
    }

    fn took_list(&mut self, context: &Context, bytes: &[u8], tab: Tab) {
        let Some(value) = std::str::from_utf8(bytes)
            .ok()
            .and_then(|body| kobo_json::parse(body).ok())
        else {
            self.problem = Some("Hacker News' answer could not be read.".to_owned());
            return;
        };
        self.tab = tab;
        self.now = unix_now();
        self.stories = model::stories_from(&value);
        self.page = 0;
        self.repaginate_list(context);
        if self.stories.is_empty() {
            self.problem = Some("That tab came back empty.".to_owned());
        }
    }

    /// Measures the rows against the panel to find where the folds are.
    fn repaginate_list(&mut self, context: &Context) {
        self.titles = self
            .stories
            .iter()
            .map(|story| context.clamped_row(&story.title, TITLE_LINES, true))
            .collect();
        let summaries = self
            .stories
            .iter()
            .map(|story| model::summary(story, self.now))
            .collect::<Vec<_>>();
        let rows = self
            .titles
            .iter()
            .zip(&summaries)
            .map(|(title, summary)| (title.as_str(), summary.as_str()))
            .collect::<Vec<_>>();
        self.pages = context.paginate_rows(&rows, true);
        self.page = self.page.min(self.pages.len().saturating_sub(1));
    }

    /// Reads a whole nested thread, or falls back if it cannot.
    fn took_thread(&mut self, context: &Context, bytes: &[u8]) {
        let parsed = std::str::from_utf8(bytes)
            .ok()
            .and_then(|body| kobo_json::parse(body).ok());
        let Some(value) = parsed else {
            // Almost always the parser's own nesting ceiling: a thread with
            // more than thirty levels of reply in it. The fallback reads the
            // same comments without the nesting, so this is recoverable
            // rather than fatal.
            self.begin_fallback("This thread is nested deeper than it can be read whole.");
            return;
        };
        self.comments = model::flatten(&value);
        let expected = self
            .open
            .and_then(|index| self.stories.get(index))
            .map_or(0, |story| story.comments);
        if self.comments.len() >= model::MAX_COMMENTS {
            self.note = Some(format!(
                "This thread has {expected} comments. The first {} are shown; the rest \
                 are more than this device will hold at once.",
                model::MAX_COMMENTS
            ));
        }
        self.repaginate_thread(context);
    }

    /// Switches to the flat fallback and says why on the screen.
    ///
    /// Deliberately not silent, and deliberately not a truncation. The reader
    /// is told the nesting is gone, told the order has changed, and given the
    /// rest of the thread a page at a time. The alternative — showing the
    /// first 512 KB of a JSON document as if it were the whole conversation —
    /// produces a thread that ends mid-sentence and looks correct.
    fn begin_fallback(&mut self, because: &str) {
        self.flat = true;
        self.flat_taken = 0;
        self.flat_pages = 1;
        self.comments.clear();
        self.note = Some(format!(
            "{because} It is shown flat instead: newest first, without replies \
             threaded under what they answer. Tap Next past the last page for more."
        ));
    }

    fn took_flat_page(&mut self, context: &Context, bytes: &[u8]) {
        let Some(value) = std::str::from_utf8(bytes)
            .ok()
            .and_then(|body| kobo_json::parse(body).ok())
        else {
            self.problem = Some("Hacker News' answer could not be read.".to_owned());
            return;
        };
        self.flat_pages = model::pages_of(&value);
        self.flat_taken = self.flat_taken.saturating_add(1);
        let room = model::MAX_COMMENTS.saturating_sub(self.comments.len());
        self.comments
            .extend(model::flat_comments_from(&value).into_iter().take(room));
        self.repaginate_thread(context);
    }

    fn repaginate_thread(&mut self, context: &Context) {
        let paragraphs = self.thread_paragraphs();
        let borrowed = paragraphs
            .iter()
            .map(|(depth, text)| (*depth, text.as_str()))
            .collect::<Vec<_>>();
        self.thread_pages = context.paginate_quoted(&borrowed, true);
        self.thread_page = self
            .thread_page
            .min(self.thread_pages.len().saturating_sub(1));
    }

    /// Whether there is more of a fallback thread to ask for.
    fn more_to_take(&self) -> bool {
        self.flat && self.flat_taken < self.flat_pages && self.comments.len() < model::MAX_COMMENTS
    }

    fn turn_list(&mut self, forwards: bool) {
        let last = self.pages.len().saturating_sub(1);
        self.page = if forwards {
            // Wrapping, because the only visible forward control is in the top
            // bar: a reader who reaches the end with no way back to the start
            // would have to leave the tab and come back.
            if self.page >= last {
                0
            } else {
                self.page + 1
            }
        } else {
            self.page.saturating_sub(1)
        };
        self.problem = None;
    }

    fn switch_tab(&mut self, context: &mut Context, tab: Tab) {
        if self.tab == tab && !self.stories.is_empty() && self.view == View::List {
            return;
        }
        self.view = View::List;
        if self.tab == tab && !self.stories.is_empty() {
            // Coming back from a thread to the tab that is already loaded.
            // Asking again would cost a second of radio for a list that has
            // not changed since the reader tapped into it.
            self.show(context);
            return;
        }
        self.tab = tab;
        self.stories.clear();
        self.titles.clear();
        self.pages.clear();
        self.page = 0;
        self.ask_list(context);
        self.show(context);
    }
}

impl KoboApp for Hn {
    fn on_start(&mut self, context: &mut Context) {
        self.now = unix_now();
        self.ask_list(context);
        self.show(context);
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        for tab in Tab::ALL {
            if action == action_id(tab.action()) {
                self.switch_tab(context, tab);
                return;
            }
        }
        if action == action_id("stories") {
            self.view = View::List;
            self.problem = None;
            self.show(context);
            return;
        }
        if action == action_id("retry") {
            self.ask_list(context);
            self.show(context);
            return;
        }
        if action == action_id("list-next") || action == action_id("list-back") {
            self.turn_list(action == action_id("list-next"));
            self.show(context);
            return;
        }
        if action == action_id("thread-next") {
            self.turn_thread(context);
            return;
        }
        if action == action_id("thread-back") {
            self.thread_page = self.thread_page.saturating_sub(1);
            self.problem = None;
            self.show(context);
            return;
        }
        for index in 0..self.stories.len() {
            if action == action_id(&format!("story-{index}")) {
                self.open_story(context, index);
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
                Awaiting::List(tab) => self.took_list(context, &bytes, tab),
                Awaiting::Thread => self.took_thread(context, &bytes),
                Awaiting::FlatPage => self.took_flat_page(context, &bytes),
            },
            TaskOutcome::Failed(TaskError::TooLarge) if awaiting == Awaiting::Thread => {
                // The case this application was written around. Not an error
                // the reader caused and not one they can do anything about, so
                // it is handled rather than reported.
                self.begin_fallback("This thread is larger than this device can fetch at once.");
                self.ask_flat_page(context);
                self.show(context);
                return;
            }
            TaskOutcome::Failed(error) => {
                // Named rather than summarised: "not found" and "the network
                // could not be reached" call for entirely different things.
                self.problem = Some(format!("That did not work: {error}."));
                if awaiting == Awaiting::Thread && self.comments.is_empty() {
                    self.repaginate_thread(context);
                }
            }
            TaskOutcome::Cancelled => self.problem = Some("Cancelled.".to_owned()),
        }
        self.show(context);
    }
}

impl Hn {
    /// Turns the thread forward, fetching more of a fallback at the end.
    fn turn_thread(&mut self, context: &mut Context) {
        if self.thread_page + 1 < self.thread_pages.len() {
            self.thread_page += 1;
            self.problem = None;
        } else if self.more_to_take() && self.task.is_none() {
            self.ask_flat_page(context);
        } else if self.flat {
            self.problem = Some("That is everything this thread will give.".to_owned());
        } else {
            self.problem = Some("That is the end of the thread.".to_owned());
        }
        self.show(context);
    }
}

/// The device clock, in seconds since the epoch.
///
/// Only ever used to write "4h ago". A Kobo that has been asleep for a week
/// can come back with a clock that disagrees with the server, which is why
/// [`model::age`] treats a negative interval as "just now" rather than as a
/// comment from the future.
fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|since| i64::try_from(since.as_secs()).ok())
        .unwrap_or_default()
}

fn main() -> ExitCode {
    match kobo_sdk::run("hn", Hn::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("hn: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{model, Awaiting, Hn, Tab, View, TABS, TITLE_LINES};
    use kobo_sdk::{action_id, AppRunner, Command, Task, TaskError, TaskId, TaskOutcome};
    use kobo_ui::{Chrome, LayoutKind, Rect, CLARA_BW_METRICS};

    const FRONT_PAGE: &str = include_str!("../tests/front_page.json");
    const THREAD: &str = include_str!("../tests/thread.json");
    const COMMENT_PAGE: &str = include_str!("../tests/comment_page.json");

    /// An application with a real front page already loaded.
    fn loaded() -> AppRunner<Hn> {
        let mut runner = AppRunner::new(Hn::default());
        runner.start();
        let task = spawned(&runner);
        runner.task_outcome(task, TaskOutcome::Completed(FRONT_PAGE.as_bytes().to_vec()));
        runner
    }

    fn spawned(runner: &AppRunner<Hn>) -> TaskId {
        runner
            .app()
            .task
            .map(|(task, _)| task)
            .expect("a request is in flight")
    }

    fn asked(commands: &[Command]) -> Option<String> {
        commands.iter().find_map(|command| match command {
            Command::Spawn {
                work: Task::Fetch { url, .. },
                ..
            } => Some(url.clone()),
            _ => None,
        })
    }

    fn tab_rects(screen: &kobo_sdk::Screen) -> Vec<Rect> {
        screen
            .layout_with(&CLARA_BW_METRICS, Chrome::default())
            .nodes
            .iter()
            .filter(|node| {
                matches!(
                    node.kind,
                    LayoutKind::NavDestination(_) | LayoutKind::NavDestinationSelected(_)
                )
            })
            .map(|node| node.rect)
            .collect()
    }

    #[test]
    fn the_four_tabs_never_move_however_the_content_reflows() {
        // The defect this system has already been bitten by: a control that
        // walks down the panel as the text above it grows, so the finger that
        // was aimed at it lands on whatever took its place. A tab bar is the
        // worst place for it, because tabs are what a reader taps without
        // looking. Asserted as rectangles, not as intention.
        let mut runner = AppRunner::new(Hn::default());
        runner.start();
        let loading = tab_rects(&runner.app().list());
        assert_eq!(loading.len(), TABS.len());

        let task = spawned(&runner);
        runner.task_outcome(task, TaskOutcome::Completed(FRONT_PAGE.as_bytes().to_vec()));
        let full = tab_rects(&runner.app().list());
        assert_eq!(loading, full, "the tabs moved when the stories arrived");

        runner.app_mut().problem = Some(
            "A banner long enough to wrap onto a second line of a panel that is only a \
             few inches across, which is exactly when a bar walks."
                .to_owned(),
        );
        let with_banner = tab_rects(&runner.app().list());
        assert_eq!(loading, with_banner, "the tabs moved under an error");

        runner.app_mut().stories.clear();
        runner.app_mut().pages.clear();
        runner.app_mut().problem = None;
        let empty = tab_rects(&runner.app().list());
        assert_eq!(loading, empty, "the tabs moved on an empty tab");

        for rect in full {
            assert!(
                rect.height >= CLARA_BW_METRICS.touch_target_minimum(),
                "a tab too small to tap: {rect:?}"
            );
        }
    }

    #[test]
    fn every_tab_asks_the_endpoint_that_actually_serves_it() {
        // Four tabs, three endpoints and four tag sets. Getting one wrong
        // gives a tab that quietly shows the front page under another name.
        let mut runner = loaded();
        for (tab, expected) in [
            (Tab::New, "search_by_date?tags=story&hitsPerPage=30"),
            (Tab::Ask, "search?tags=ask_hn&hitsPerPage=30"),
            (Tab::Show, "search?tags=show_hn&hitsPerPage=30"),
            (Tab::Top, "search?tags=front_page&hitsPerPage=30"),
        ] {
            let commands = runner.action(action_id(tab.action()));
            let url = asked(&commands).expect("the tab asked for something");
            let (base, _) = url.split_once("&numericFilters").unwrap_or((&url, ""));
            assert_eq!(base, format!("https://hn.algolia.com/api/v1/{expected}"));
            let task = spawned(&runner);
            runner.task_outcome(task, TaskOutcome::Completed(FRONT_PAGE.as_bytes().to_vec()));
        }
    }

    #[test]
    fn the_ranked_tabs_only_ask_for_stories_from_this_week() {
        // Algolia ranks `search` over all of Hacker News forever, so without a
        // window Ask HN answers with the best thread of 2013. This is what
        // shipped, and what the owner noticed.
        let now = 1_700_000_000;
        let week = 7 * 24 * 60 * 60;
        for tab in [Tab::Ask, Tab::Show] {
            let url = tab.url(now);
            assert!(
                url.contains(&format!("numericFilters=created_at_i%3E{}", now - week)),
                "{tab:?} asked without a window: {url}"
            );
        }
        for tab in [Tab::Top, Tab::New] {
            // `front_page` is by definition current and `search_by_date` is
            // ordered by date, so a window there would only hide stories.
            assert!(
                !tab.url(now).contains("numericFilters"),
                "{tab:?} does not need a window"
            );
        }
    }

    #[test]
    fn a_real_front_page_pages_into_screens_that_are_all_drawn() {
        // The layout engine stops at the bottom of the content area and drops
        // the rest in silence, so a page measured wrongly is a story that
        // exists in memory and nowhere on the device.
        let runner = loaded();
        let application = runner.app();
        assert!(!application.pages.is_empty());
        let counted = application.pages.iter().map(Vec::len).sum::<usize>();
        assert_eq!(
            counted,
            application.stories.len(),
            "a story fell off a page"
        );
        for page in 0..application.pages.len() {
            let mut showing = Hn {
                page,
                ..Hn::default()
            };
            showing.stories.clone_from(&application.stories);
            showing.titles.clone_from(&application.titles);
            showing.pages.clone_from(&application.pages);
            let layout = showing
                .list()
                .layout_with(&CLARA_BW_METRICS, Chrome::default());
            let drawn = layout
                .nodes
                .iter()
                .filter(|node| matches!(node.kind, LayoutKind::Row(_)))
                .count();
            assert_eq!(
                drawn,
                application.pages[page].len(),
                "page {page} measured as {} rows but drew {drawn}",
                application.pages[page].len()
            );
        }
    }

    #[test]
    fn probe_hit_test() {
        let runner = loaded();
        let application = runner.app();
        let mut showing = Hn::default();
        showing.stories.clone_from(&application.stories);
        showing.titles.clone_from(&application.titles);
        showing.pages.clone_from(&application.pages);
        let screen = showing.list();
        for y in [200, 400, 1390] {
            println!("hit {y} -> {:?}", screen.hit_test(400, y));
        }
        let layout = screen.layout();
        for node in &layout.nodes {
            println!("{:?} {:?}", node.kind, node.rect);
        }
    }

    #[test]
    fn a_headline_gets_two_lines_and_no_more() {
        // Rows used to be cut to one line so the list was a stack of equal
        // bands. Against real headlines that meant most of them stopped
        // mid-sentence, so the allowance is two now. The ceiling still has to
        // hold, or one enormous title takes a page to itself.
        let runner = loaded();
        let application = runner.app();
        assert!(
            application
                .stories
                .iter()
                .any(|story| story.title.chars().count() > 60),
            "no headline was long enough for this to prove anything"
        );
        let mut showing = Hn::default();
        showing.stories.clone_from(&application.stories);
        showing.titles.clone_from(&application.titles);
        showing.pages.clone_from(&application.pages);
        let layout = showing
            .list()
            .layout_with(&CLARA_BW_METRICS, Chrome::default());
        let lines = layout
            .nodes
            .iter()
            .filter(|node| matches!(node.kind, LayoutKind::RowTitle))
            .map(|node| node.text_lines.len())
            .collect::<Vec<_>>();
        assert!(lines.len() > 3, "too few rows to compare");
        assert!(
            lines.iter().all(|count| *count <= TITLE_LINES),
            "a headline ran past its allowance: {lines:?}"
        );

        // And the allowance has to earn itself: one line would truncate most
        // of a real front page, two truncates almost none of it.
        let cut = |allowance| {
            application
                .stories
                .iter()
                .filter(|story| {
                    runner
                        .context()
                        .clamped_row(&story.title, allowance, true)
                        .ends_with('\u{2026}')
                })
                .count()
        };
        let (one, two) = (cut(1), cut(TITLE_LINES));
        println!("headlines truncated: one line {one}, two lines {two}");
        assert!(one > 0, "no headline was truncated at one line either way");
        assert!(
            two < one,
            "two lines saved nothing worth the ragged column: one line cut \
             {one} of {}, two lines cut {two}",
            application.stories.len()
        );
    }

    #[test]
    fn a_reply_is_drawn_further_in_than_what_it_answers() {
        // Depth used to be drawn with chevrons in the text because no node
        // took an offset. It is real indentation now, so this asserts the
        // pixels rather than the characters.
        let mut runner = loaded();
        runner.action(action_id("story-0"));
        let task = spawned(&runner);
        runner.task_outcome(task, TaskOutcome::Completed(THREAD.as_bytes().to_vec()));
        let application = runner.app();
        assert!(
            application.comments.iter().any(|comment| comment.depth > 0),
            "the fixture has no replies, so this proves nothing"
        );
        let mut lefts = Vec::new();
        for page in 0..application.thread_pages.len() {
            let mut showing = Hn {
                thread_page: page,
                open: application.open,
                ..Hn::default()
            };
            showing.stories.clone_from(&application.stories);
            showing.thread_pages.clone_from(&application.thread_pages);
            let layout = showing
                .thread()
                .layout_with(&CLARA_BW_METRICS, Chrome::default());
            for node in &layout.nodes {
                if let LayoutKind::Quote(depth) = node.kind {
                    lefts.push((depth, node.rect.x, node.rect.width));
                }
            }
        }
        let root = lefts
            .iter()
            .find(|(depth, _, _)| *depth == 0)
            .expect("nothing at the top level");
        let reply = lefts
            .iter()
            .find(|(depth, _, _)| *depth > 0)
            .expect("nothing indented");
        assert!(
            reply.1 > root.1,
            "a reply started at {} against {} for the top level",
            reply.1,
            root.1
        );
        assert!(
            reply.2 < root.2,
            "an indented reply was not narrower than the top level"
        );
    }

    #[test]
    fn a_thread_pages_into_screens_whose_paragraphs_are_all_drawn() {
        // The same property for prose. A comment thread that measured wrongly
        // loses its last paragraph on every page, and nothing on the panel
        // says so.
        let mut runner = loaded();
        runner.action(action_id("story-0"));
        let task = spawned(&runner);
        runner.task_outcome(task, TaskOutcome::Completed(THREAD.as_bytes().to_vec()));
        let application = runner.app();
        assert_eq!(application.view, View::Thread);
        assert!(
            application.thread_pages.len() > 1,
            "a whole thread fitted one page, so this proves nothing"
        );
        for page in 0..application.thread_pages.len() {
            let mut showing = Hn {
                thread_page: page,
                open: application.open,
                ..Hn::default()
            };
            showing.stories.clone_from(&application.stories);
            showing.thread_pages.clone_from(&application.thread_pages);
            let layout = showing
                .thread()
                .layout_with(&CLARA_BW_METRICS, Chrome::default());
            let drawn = layout
                .nodes
                .iter()
                .filter(|node| matches!(node.kind, LayoutKind::Quote(_)))
                .count();
            assert_eq!(
                drawn,
                application.thread_pages[page].len(),
                "thread page {page} measured as {} paragraphs but drew {drawn}",
                application.thread_pages[page].len()
            );
        }
    }

    #[test]
    fn the_page_controls_on_a_thread_are_reachable_at_their_centres() {
        // They are the pinned bar rather than the end of the flow, so a long
        // page loses its final sentence rather than its way forward.
        let mut runner = loaded();
        runner.action(action_id("story-0"));
        let task = spawned(&runner);
        runner.task_outcome(task, TaskOutcome::Completed(THREAD.as_bytes().to_vec()));
        let layout = runner
            .app()
            .thread()
            .layout_with(&CLARA_BW_METRICS, Chrome::default());
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
    fn a_thread_too_large_to_fetch_falls_back_instead_of_dead_ending() {
        // The centre of this application. Algolia ignores `Range`, so there is
        // no second chunk to ask for: the only alternatives are a flat
        // fallback or a story whose comments cannot be read at all.
        let mut runner = loaded();
        runner.action(action_id("story-0"));
        let task = spawned(&runner);
        let commands = runner.task_outcome(task, TaskOutcome::Failed(TaskError::TooLarge));
        let url = asked(&commands).expect("nothing was asked for after the ceiling");
        assert_eq!(
            url,
            "https://hn.algolia.com/api/v1/search_by_date?tags=comment,story_49057175\
             &hitsPerPage=30&page=0"
        );
        assert!(runner.app().flat);

        let task = spawned(&runner);
        runner.task_outcome(
            task,
            TaskOutcome::Completed(COMMENT_PAGE.as_bytes().to_vec()),
        );
        let application = runner.app();
        assert_eq!(application.comments.len(), 5);
        let note = application.note.as_deref().expect("nothing was admitted");
        assert!(
            note.contains("flat") && note.contains("newest first"),
            "the fallback did not say what it changed: {note}"
        );
        // And the admission is inside the measured flow, so it cannot push the
        // last paragraph of the page off the bottom of the panel.
        assert!(
            application.thread_pages[0]
                .iter()
                .any(|(_, paragraph)| paragraph.contains("flat")),
            "the note was not on the first page"
        );
    }

    #[test]
    fn a_thread_nested_past_the_parser_falls_back_rather_than_showing_nothing() {
        // The other way a thread can be unreadable. A parse failure that
        // reported an error would leave a story with a comment count and no
        // comments, which reads as a broken application.
        let mut runner = loaded();
        runner.action(action_id("story-0"));
        let task = spawned(&runner);
        let deep = format!("{}1{}", "[".repeat(200), "]".repeat(200));
        let commands = runner.task_outcome(task, TaskOutcome::Completed(deep.into_bytes()));
        assert!(runner.app().flat);
        assert!(asked(&commands).is_none(), "the fallback raced the repaint");
        assert!(runner
            .app()
            .note
            .as_deref()
            .is_some_and(|note| note.contains("nested")));
    }

    #[test]
    fn the_fallback_stops_asking_at_the_ceiling_rather_than_growing_forever() {
        // A thread of forty thousand comments is thirteen hundred pages. The
        // cap is what stops "tap Next" being a way to fill the device's memory
        // one page at a time.
        let mut runner = loaded();
        runner.action(action_id("story-0"));
        runner.app_mut().flat = true;
        runner.app_mut().flat_pages = 5000;
        runner.app_mut().flat_taken = 1;
        runner.app_mut().task = None;
        assert!(runner.app().more_to_take());
        runner.app_mut().comments = vec![model::Comment::default(); model::MAX_COMMENTS];
        assert!(
            !runner.app().more_to_take(),
            "the fallback would keep asking past the ceiling"
        );
    }

    #[test]
    fn only_one_request_is_ever_in_flight() {
        // A reader who taps three tabs while the first answer is in the air is
        // asking for the third. Letting all three run lands three lists on the
        // panel in an order the network chose, and each one is a full refresh
        // the reader sits through.
        let mut runner = AppRunner::new(Hn::default());
        runner.start();
        let first = spawned(&runner);
        let mut spawns = 1;
        let mut cancels = 0;
        for tab in ["tab-new", "tab-ask", "tab-show"] {
            for command in runner.action(action_id(tab)) {
                match command {
                    Command::Spawn { .. } => spawns += 1,
                    Command::Cancel(_) => cancels += 1,
                    _ => {}
                }
            }
        }
        assert_eq!(spawns, 4, "a tap did not ask for its tab");
        assert_eq!(
            spawns - cancels,
            1,
            "more than one answer is on its way to the same panel"
        );
        assert!(matches!(runner.app().task, Some((_, Awaiting::List(_)))));
        assert_ne!(runner.app().task.map(|(task, _)| task), Some(first));
    }

    #[test]
    fn an_identifier_that_is_not_a_number_never_becomes_a_url() {
        // `objectID` arrives from the network and goes straight into a request
        // this device makes. Anything but digits is somebody else naming the
        // address.
        let mut runner = loaded();
        runner.app_mut().stories[0].id = "1/../../evil?x=".to_owned();
        runner.app_mut().open = Some(0);
        assert_eq!(runner.app().open_id(), None);
        let commands = runner.action(action_id("story-0"));
        assert!(
            asked(&commands).is_none(),
            "a crafted identifier reached the network"
        );
        assert!(
            runner.app().problem.is_some(),
            "and nothing was said about it"
        );
    }

    #[test]
    fn coming_back_from_a_thread_does_not_ask_for_the_list_again() {
        // A second of radio for a list that has not changed since the reader
        // tapped into it, on a device that reads for weeks on a charge.
        let mut runner = loaded();
        runner.action(action_id("story-0"));
        let task = spawned(&runner);
        runner.task_outcome(task, TaskOutcome::Completed(THREAD.as_bytes().to_vec()));
        let commands = runner.action(action_id("tab-top"));
        assert!(asked(&commands).is_none(), "the list was fetched twice");
        assert_eq!(runner.app().view, View::List);
    }

    #[test]
    fn opening_a_second_story_keeps_nothing_from_the_first() {
        // The comments are held as one list. Leaving them in place draws the
        // last story's thread under the new story's title for as long as the
        // request takes.
        let mut runner = loaded();
        runner.action(action_id("story-0"));
        let task = spawned(&runner);
        runner.task_outcome(task, TaskOutcome::Completed(THREAD.as_bytes().to_vec()));
        assert!(!runner.app().comments.is_empty());
        runner.action(action_id("stories"));
        runner.action(action_id("story-1"));
        let application = runner.app();
        assert!(application.comments.is_empty());
        assert_eq!(application.thread_page, 0);
        assert!(!application.flat);
        assert_eq!(application.note, None);
    }
}

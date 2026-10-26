//! A brief that is ready before you open it.
//!
//! This exists to demonstrate the one lifecycle e-readers actually need, and
//! the one a mobile framework would call backgrounding. It is not a feed
//! reader with extra steps: the whole point is what happens when you *leave*.
//!
//! ## What it demonstrates
//!
//! Tap Refresh and it starts fetching. Go back to the launcher and open
//! something else. The fetch keeps running, because leaving an application no
//! longer stops it: the runtime keeps the process, the work in flight and the
//! memory, and tells the application it is no longer being looked at. Come back
//! and the brief is finished and drawn, with no reload and no second fetch.
//!
//! Under the previous design, leaving killed the process and returning started
//! it again from nothing, so this application could not have existed.
//!
//! ## Why it saves the moment it goes to the background
//!
//! [`KoboApp::on_background`] is the last certain moment. A reader closes an
//! e-reader by shutting a cover and may not open it for a week, and the device
//! may run its battery flat in between. So the brief is written then, and on
//! every arrival, rather than on the way out.
//!
//! ## Why the cached copy is shown before the fetch finishes
//!
//! The panel holds an image at zero power, so there is nothing to cover and no
//! reason to show a spinner. Yesterday's brief with an honest "as of" line is
//! more use than a blank screen, and it means the application is readable with
//! no network at all.

use kobo_bookview::illustrations::Illustrations;
use kobo_bookview::BookView;
use kobo_json::Value;
use kobo_sdk::snapshot::{Snapshot, SnapshotEvent};
use kobo_sdk::{
    action_id, ActionId, BandAlign, Context, Failure, Glyph, KoboApp, LogLevel, Screen,
    ScreenBuilder, SlotWidth, Space, StoreResult, Task, TaskId, TaskOutcome,
};
use std::process::ExitCode;

/// Which list of stories the brief is drawn from.
///
/// All four are the same public index in different moods, and a reader who
/// wants the day's arguments rather than the day's releases should not have to
/// take what they are given. Nothing is fetched until one is chosen or the
/// brief is refreshed.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Source {
    #[default]
    Top,
    Best,
    Ask,
    Show,
}

/// Where the index and the stories are read from.
///
/// Hacker News, unless `KOBO_BRIEF_ORIGIN` names somewhere else. That exists
/// for fixtures and captures, which need a server they own; it is read once
/// per request and never written down, so a brief saved under a fixture cannot
/// quietly become a brief from somewhere the reader did not choose.
fn origin() -> String {
    std::env::var("KOBO_BRIEF_ORIGIN")
        .ok()
        .filter(|origin| origin.starts_with("https://"))
        .unwrap_or_else(|| "https://hacker-news.firebaseio.com".to_owned())
}

impl Source {
    const ALL: [Self; 4] = [Self::Top, Self::Best, Self::Ask, Self::Show];

    fn index(self) -> String {
        let list = match self {
            Self::Top => "topstories",
            Self::Best => "beststories",
            Self::Ask => "askstories",
            Self::Show => "showstories",
        };
        format!("{}/v0/{list}.json", origin())
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Top => "Top stories",
            Self::Best => "Best of the week",
            Self::Ask => "Ask Hacker News",
            Self::Show => "Show Hacker News",
        }
    }

    const fn detail(self) -> &'static str {
        match self {
            Self::Top => "What the front page has now",
            Self::Best => "What held up over several days",
            Self::Ask => "Questions put to the site",
            Self::Show => "Things people built",
        }
    }

    const fn action(self) -> &'static str {
        match self {
            Self::Top => "source-top",
            Self::Best => "source-best",
            Self::Ask => "source-ask",
            Self::Show => "source-show",
        }
    }

    fn saved(self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::Best => "best",
            Self::Ask => "ask",
            Self::Show => "show",
        }
    }

    fn from_saved(text: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|source| source.saved() == text)
            .unwrap_or_default()
    }
}

/// How many stories a brief holds.
///
/// One panel's worth. A brief that needs a page turn is a feed, and a feed is
/// something a reader has to manage rather than glance at.
const STORIES: usize = 6;

/// The largest reply worth reading for either request.
///
/// The index is a few thousand ids and an item is a few hundred bytes. Asking
/// for less than the runtime's ceiling is what keeps a background refresh from
/// costing radio time nobody asked for.
const CEILING: u32 = 64 * 1024;

const REFRESH: &str = "refresh";
/// The brief, its source and when it was fetched.
const STORED: &str = "brief-v2";
/// The brief written before any of that was kept. Carried forward once.
const OLD_STORED: &str = "brief";
/// The most of one article this reads. A news page is a few dozen kilobytes;
/// past this it is a page of advertising with an article in it somewhere.
const ARTICLE_CEILING: u32 = 512 * 1024;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Story {
    title: String,
    site: String,
    /// Where the story itself is, when it is somewhere other than the index.
    url: String,
    /// What the poster wrote, for the questions and the show-and-tell, which
    /// have no address of their own.
    text: String,
}

/// What the application is waiting for, if anything.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Fetching {
    Nothing,
    /// The list of ids.
    Index(TaskId),
    /// One story. The position is where it goes in the brief, so replies that
    /// arrive out of order still land in the right place.
    Story(TaskId, usize),
    /// The page behind a story somebody is opening to read.
    Article(TaskId),
}

/// What the panel is showing.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum View {
    #[default]
    Brief,
    Sources,
    Reading,
}

struct Brief {
    stories: Vec<Story>,
    /// Which page of the brief is drawn, for the larger text sizes where six
    /// headlines do not fit one panel.
    page: usize,
    /// Which list the brief is drawn from.
    source: Source,
    /// When the stories on the panel were fetched, as the device read the
    /// clock at the time. A brief with no time on it is a brief nobody can
    /// tell from this morning's.
    fetched: Option<String>,
    /// Why the last refresh did not happen, if it did not. Kept apart from
    /// `note` so a failure survives until something replaces it.
    failure: Option<String>,
    view: View,
    /// The story being read, as a position in the brief.
    reading: Option<usize>,
    book: BookView,
    illustrations: Illustrations,
    /// The saved copy of the article being read.
    article: Option<Snapshot>,
    /// Ids still to be fetched, in order.
    queue: Vec<u64>,
    /// The brief being assembled. Kept apart from `stories` so a failed refresh
    /// leaves the previous brief on the panel rather than half of a new one.
    building: Vec<Option<Story>>,
    fetching: Fetching,
    /// Set while the reader is not looking. Nothing is drawn then, because
    /// nothing drawn would be seen, and a repaint the reader cannot see is a
    /// refresh charged to the battery for nothing.
    background: bool,
    loaded: bool,
    note: Option<String>,
}

impl Default for Brief {
    fn default() -> Self {
        Self {
            stories: Vec::new(),
            page: 0,
            source: Source::default(),
            fetched: None,
            failure: None,
            view: View::default(),
            reading: None,
            book: BookView::new(),
            illustrations: Illustrations::default(),
            article: None,
            queue: Vec::new(),
            building: Vec::new(),
            fetching: Fetching::Nothing,
            background: false,
            loaded: false,
            note: None,
        }
    }
}

/// Encodes the brief for the store: one story a line, title and site tabbed.
///
/// A tab is used rather than a comma because a title can contain a comma and
/// cannot contain a tab; anything that arrives with one has it replaced when the
/// story is built, so this cannot be ambiguous.
fn encode(source: Source, fetched: Option<&str>, stories: &[Story]) -> Vec<u8> {
    let mut out = format!("brief-v2\t{}\t{}\n", source.saved(), fetched.unwrap_or(""));
    for story in stories {
        out.push_str(&story.title);
        out.push('\t');
        out.push_str(&story.site);
        out.push('\t');
        out.push_str(&story.url);
        out.push('\t');
        out.push_str(&story.text);
        out.push('\n');
    }
    out.into_bytes()
}

/// What was written down: the stories, where they came from and when.
#[derive(Debug, Default, Eq, PartialEq)]
struct Saved {
    source: Source,
    fetched: Option<String>,
    stories: Vec<Story>,
}

fn decode(bytes: &[u8]) -> Saved {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Saved::default();
    };
    let mut lines = text.lines();
    let Some(header) = lines.next() else {
        return Saved::default();
    };
    let mut saved = Saved::default();
    let rest: Box<dyn Iterator<Item = &str>> = match header.split('\t').collect::<Vec<_>>()[..] {
        ["brief-v2", source, fetched] => {
            saved.source = Source::from_saved(source);
            saved.fetched = (!fetched.is_empty()).then(|| fetched.to_owned());
            Box::new(lines)
        }
        // The first version wrote titles and sites and nothing else. A brief
        // written by it still reads; it simply cannot say when it was fetched.
        _ => Box::new(std::iter::once(header).chain(lines)),
    };
    saved.stories = rest
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let title = fields.next()?;
            if title.is_empty() {
                return None;
            }
            Some(Story {
                title: title.to_owned(),
                site: fields.next().unwrap_or_default().to_owned(),
                url: fields.next().unwrap_or_default().to_owned(),
                text: fields.next().unwrap_or_default().to_owned(),
            })
        })
        .take(STORIES)
        .collect();
    saved
}

/// The device clock, as a line a reader can check against their own watch.
fn timestamp() -> Option<String> {
    use kobo_sdk::clock::{Clock, SystemClock};
    let now = SystemClock::new(0).ok()?.now().ok()?;
    let (hour, minute) = now.hour_minute()?;
    Some(format!("{} {hour:02}:{minute:02} UTC", now.date()?))
}

/// The host part of a URL, which is as much of a link as this panel can use.
///
/// Deliberately not the whole address: there is no browser to open it in, so
/// the only question a reader has is where the story is from.
fn site_of(url: &str) -> String {
    let host = url
        .split_once("://")
        .map_or(url, |(_, rest)| rest)
        .split('/')
        .next()
        .unwrap_or_default();
    host.strip_prefix("www.").unwrap_or(host).to_string()
}

fn clean(text: &str) -> String {
    text.replace(['\t', '\n', '\r'], " ")
}

impl Brief {
    fn show(&self, context: &mut Context) {
        if self.background {
            return;
        }
        let drawn = self.screen(context);
        context.set_screen(drawn);
    }

    /// Everything above the stories, which is what the list is measured under.
    fn brief_prefix(&self, context: &Context) -> ScreenBuilder {
        let mut screen = ScreenBuilder::new("brief")
            .top_bar("Daily brief")
            .top_bar_action("sources", "Source")
            .secondary(self.as_of());
        if let Some(failure) = &self.failure {
            screen = screen.banner(kobo_sdk::BannerLevel::Attention, failure.clone());
        }
        if let Some(note) = &self.note {
            screen = screen.text(note.clone());
        }
        let stories = self.stories.len().to_string();
        let sources = self.sources().to_string();
        screen = screen.band(
            BandAlign::Top,
            [
                (
                    SlotWidth::Fill,
                    Box::new(move |slot: ScreenBuilder| slot.facts([("Stories", stories)]))
                        as Box<dyn FnOnce(ScreenBuilder) -> ScreenBuilder>,
                ),
                (
                    SlotWidth::Fill,
                    Box::new(move |slot: ScreenBuilder| slot.facts([("Sources", sources)])),
                ),
            ],
        );
        let _ = context;
        screen.section(self.source.label())
    }

    /// How the brief's stories divide into pages.
    ///
    /// Six headlines are one panel at the sizes most readers use and two at
    /// the largest, which is their choice of type rather than this becoming a
    /// feed: every story stays reachable either way.
    fn pages(&self, context: &Context) -> Vec<Vec<usize>> {
        let rows: Vec<(String, String)> = self
            .stories
            .iter()
            .map(|story| (story.title.clone(), story.site.clone()))
            .collect();
        let borrowed: Vec<(&str, &str)> = rows
            .iter()
            .map(|(title, site)| (title.as_str(), site.as_str()))
            .collect();
        let highest = u16::try_from(self.stories.len()).unwrap_or(u16::MAX);
        let pages = context.paginate_ranked_rows_under(
            &borrowed,
            true,
            highest,
            kobo_sdk::Position::AtTheFoot,
            &self.brief_prefix(context).build(),
        );
        if pages.is_empty() {
            vec![Vec::new()]
        } else {
            pages
        }
    }

    fn screen(&self, context: &Context) -> Screen {
        if self.view == View::Reading {
            let title = self
                .reading
                .and_then(|index| self.stories.get(index))
                .map_or_else(String::new, |story| story.title.clone());
            return self.book.screen(&title).unwrap_or_else(|| {
                ScreenBuilder::new("brief-reading")
                    .top_bar(title)
                    .empty_state("This story arrived empty.")
                    .bottom_action("close-story", "Brief")
                    .build()
            });
        }
        if self.view == View::Sources {
            return self.sources_screen();
        }
        if !self.loaded {
            return ScreenBuilder::new("brief")
                .top_bar("Daily brief")
                .skeleton(5)
                .build();
        }
        let mut screen = self.brief_prefix(context);
        if self.stories.is_empty() && self.fetching == Fetching::Nothing {
            // Centred in what is left rather than stacked at the top, so a
            // brief that has not been fetched yet reads as a page waiting for
            // a tap instead of a page that failed to load.
            screen = screen.splash(
                Some(Glyph::News),
                "Nothing yet",
                "Tap Refresh once the device is online.",
            );
        } else if !self.stories.is_empty() {
            // Numbered rather than illustrated: the same note icon beside
            // every headline is decoration, and a briefing is ordered, so the
            // position is the one thing the well can usefully say.
            let pages = self.pages(context);
            let page = self.page.min(pages.len().saturating_sub(1));
            let shown = pages.get(page).cloned().unwrap_or_default();
            screen = screen.rows(shown.iter().map(|&index| {
                (
                    format!("story-{index}"),
                    self.stories[index].title.clone(),
                    self.stories[index].site.clone(),
                    u16::try_from(index + 1).unwrap_or(u16::MAX),
                )
            }));
            if pages.len() > 1 {
                screen = screen.page_turns("previous", "next").page_position(
                    u16::try_from(page + 1).unwrap_or(u16::MAX),
                    u16::try_from(pages.len()).unwrap_or(u16::MAX),
                );
            }
        }

        if self.fetching == Fetching::Nothing {
            // Pinned to the foot of the panel rather than set after the last
            // story. Placed inline it was drawn wherever the list happened to
            // end, and with a full brief that was past the bottom edge: the
            // one control on the screen, off the screen.
            screen = screen.bottom_action_marked(
                REFRESH,
                if self.failure.is_some() {
                    "Try again"
                } else {
                    "Refresh"
                },
                Glyph::Refresh,
            );
        } else {
            // A bar against a known total, not a spinner: the count of stories
            // is fixed, so an indeterminate animation would be claiming the end
            // is unknowable when it is six. Every frame of movement is a panel
            // refresh besides, so the bar is redrawn only as each story lands.
            let done = u64::try_from(self.building.iter().filter(|slot| slot.is_some()).count())
                .unwrap_or(0);
            // A count, not a byte count: `transfer` captions itself in bytes
            // and would print "3 B of 6 B" for three stories out of six.
            let percent = u8::try_from(done.saturating_mul(100) / STORIES as u64).unwrap_or(100);
            screen = screen
                .spacer(Space::Medium)
                .activity(
                    format!("Collecting stories, {done} of {STORIES}"),
                    Some(percent),
                )
                .text("You can leave this open. It keeps going.");
        }
        screen.build()
    }

    /// When this brief was fetched and where from, in one line.
    fn as_of(&self) -> String {
        match (&self.fetched, self.failure.is_some()) {
            (Some(fetched), true) => {
                format!("{} · fetched {fetched}", self.source.label())
            }
            (Some(fetched), false) => format!("{} · as of {fetched}", self.source.label()),
            (None, _) => format!("{} · not fetched yet", self.source.label()),
        }
    }

    fn sources_screen(&self) -> Screen {
        ScreenBuilder::new("brief-sources")
            .top_bar("Source")
            .owns_back(true)
            .secondary(
                "All four are public Hacker News lists. Nothing is fetched until you refresh.",
            )
            .rows(Source::ALL.map(|source| {
                (
                    source.action(),
                    source.label(),
                    if source == self.source {
                        "Showing this now"
                    } else {
                        source.detail()
                    },
                    if source == self.source {
                        Glyph::Check
                    } else {
                        Glyph::News
                    },
                )
            }))
            .bottom_action("close-sources", "Brief")
            .build()
    }

    /// How many distinct sites the brief drew from.
    ///
    /// Counted off the stories on hand rather than tracked as the fetch runs,
    /// so it can never disagree with the sites actually printed under the
    /// titles.
    fn sources(&self) -> usize {
        let mut seen: Vec<&str> = Vec::new();
        for story in &self.stories {
            if !seen.contains(&story.site.as_str()) {
                seen.push(&story.site);
            }
        }
        seen.len()
    }

    fn start_refresh(&mut self, context: &mut Context) {
        if self.fetching != Fetching::Nothing {
            return;
        }
        self.note = None;
        self.building = vec![None; STORIES];
        self.queue.clear();
        match context.spawn(Task::Fetch {
            url: self.source.index(),
            offset: 0,
            max_bytes: CEILING,
            credential: None,
            headers: Vec::new(),
        }) {
            Some(task) => self.fetching = Fetching::Index(task),
            None => self.note = Some("Too much already in flight.".to_owned()),
        }
    }

    /// Starts the next story, or finishes the brief when there are none left.
    fn advance(&mut self, context: &mut Context) {
        let position = self.building.iter().position(Option::is_none);
        let (Some(position), Some(id)) = (position, self.queue.first().copied()) else {
            self.complete(context);
            return;
        };
        self.queue.remove(0);
        let url = format!("{}/v0/item/{id}.json", origin());
        if let Some(task) = context.spawn(Task::Fetch {
            url,
            offset: 0,
            max_bytes: CEILING,
            credential: None,
            headers: Vec::new(),
        }) {
            self.fetching = Fetching::Story(task, position);
        } else {
            self.note = Some("Too much already in flight.".to_owned());
            self.complete(context);
        }
    }

    /// Publishes whatever was collected and writes it down.
    fn complete(&mut self, context: &mut Context) {
        self.fetching = Fetching::Nothing;
        let collected: Vec<Story> = self.building.iter().flatten().cloned().collect();
        if collected.is_empty() {
            if self.note.is_none() {
                self.note = Some("The refresh brought back nothing.".to_owned());
            }
        } else {
            self.stories = collected;
            self.fetched = timestamp();
            self.failure = None;
            self.keep(context);
        }
        self.building.clear();
        self.queue.clear();
    }

    /// Writes the brief down, with where it came from and when.
    fn keep(&self, context: &mut Context) {
        context.store().save(
            STORED,
            encode(self.source, self.fetched.as_deref(), &self.stories),
        );
    }

    /// Takes whatever the store had: the brief, the one the previous version
    /// wrote, or a saved article.
    fn opened(&mut self, context: &mut Context, key: &str, value: Option<Vec<u8>>) {
        if self.article_result(
            context,
            key,
            &StoreResult::Loaded {
                key: key.to_owned(),
                value: value.clone(),
            },
        ) {
            return;
        }
        if key == STORED {
            if let Some(saved) = value.map(|bytes| decode(&bytes)) {
                if !saved.stories.is_empty() {
                    self.stories = saved.stories;
                    self.source = saved.source;
                    self.fetched = saved.fetched;
                    self.loaded = true;
                    self.show(context);
                    return;
                }
            }
            // Nothing under the new name: the previous version may have left
            // a brief under the old one.
            context.store().load(OLD_STORED);
            return;
        }
        if key == OLD_STORED {
            if let Some(saved) = value.map(|bytes| decode(&bytes)) {
                self.stories = saved.stories;
            }
            self.loaded = true;
            self.show(context);
            return;
        }
        self.loaded = true;
        self.show(context);
    }

    /// Routes a store answer that belongs to the article being read.
    fn article_result(&mut self, context: &mut Context, key: &str, result: &StoreResult) -> bool {
        if self
            .illustrations
            .store(context, &mut self.book, key, result, false)
        {
            self.show(context);
            return true;
        }
        let Some(article) = self.article.as_mut().filter(|saved| saved.key == key) else {
            return false;
        };
        let event = article.stored(context, result);
        self.article_event(context, event);
        true
    }

    fn article_event(&mut self, context: &mut Context, event: Option<SnapshotEvent>) {
        match event {
            Some(SnapshotEvent::Loaded) => {
                let saved = self.article.as_ref().and_then(|saved| saved.bytes.clone());
                match saved {
                    // Read from the copy on the device: no request, and it
                    // works with the radio off.
                    Some(bytes) => self.read_article(context, &bytes),
                    None => self.fetch_article(context),
                }
            }
            Some(SnapshotEvent::Failed) => {
                self.note = Some("This story could not be saved for offline reading.".to_owned());
                self.show(context);
            }
            Some(SnapshotEvent::Saved) | None => self.show(context),
        }
    }

    /// Puts an article in front of the reader.
    fn read_article(&mut self, context: &mut Context, body: &[u8]) {
        let story = self
            .reading
            .and_then(|index| self.stories.get(index))
            .cloned();
        let Some(story) = story else {
            return;
        };
        let source = String::from_utf8_lossy(body).into_owned();
        self.book.close(context);
        self.illustrations.close(context);
        self.book.open(
            context,
            kobo_doc::html::parse(&source),
            kobo_read::Memory::default(),
        );
        self.illustrations.open(context, &mut self.book, &story.url);
        self.view = View::Reading;
        self.show(context);
    }

    fn fetch_article(&mut self, context: &mut Context) {
        let Some(story) = self.reading.and_then(|index| self.stories.get(index)) else {
            return;
        };
        if story.url.is_empty() {
            return;
        }
        if let Some(task) = context.spawn(Task::Fetch {
            url: story.url.clone(),
            offset: 0,
            max_bytes: ARTICLE_CEILING,
            credential: None,
            headers: Vec::new(),
        }) {
            self.fetching = Fetching::Article(task);
        } else {
            self.note = Some("Too much already in flight.".to_owned());
            self.show(context);
        }
    }

    /// Opens one story: the saved copy first, the site only if there is none.
    fn open_story(&mut self, context: &mut Context, index: usize) {
        let Some(story) = self.stories.get(index).cloned() else {
            return;
        };
        self.reading = Some(index);
        self.note = None;
        if story.url.is_empty() {
            // A question or a show-and-tell is its own text, and there is
            // nothing to fetch or to save.
            let body = format!("<h2>{}</h2>{}", story.title, story.text);
            self.read_article(context, body.as_bytes());
            return;
        }
        let saved = Snapshot::new(&format!("brief-article:{}", story.url))
            .at_most(ARTICLE_CEILING as usize);
        saved.start(context);
        self.article = Some(saved);
        self.note = Some("Opening the story…".to_owned());
        self.show(context);
    }

    fn close_story(&mut self, context: &mut Context) {
        self.illustrations.close(context);
        self.book.close(context);
        self.reading = None;
        self.view = View::Brief;
    }

    fn on_index(&mut self, context: &mut Context, body: &[u8]) {
        let Ok(text) = std::str::from_utf8(body) else {
            self.fail(context, "The index was not text.");
            return;
        };
        let Ok(value) = kobo_json::parse(text) else {
            self.fail(context, "The index could not be read.");
            return;
        };
        let Some(ids) = value.as_array() else {
            self.fail(context, "The index was not a list.");
            return;
        };
        self.queue = ids
            .iter()
            .filter_map(Value::as_i64)
            .map(u64::try_from)
            .filter_map(Result::ok)
            .take(STORIES)
            .collect();
        if self.queue.is_empty() {
            self.fail(context, "The index was empty.");
            return;
        }
        self.building = vec![None; self.queue.len().min(STORIES)];
        self.advance(context);
    }

    fn on_story(&mut self, context: &mut Context, position: usize, body: &[u8]) {
        if let Some(story) = parse_story(body) {
            if let Some(slot) = self.building.get_mut(position) {
                *slot = Some(story);
            }
        }
        self.advance(context);
    }

    fn fail(&mut self, context: &mut Context, why: &str) {
        // The brief on the panel stays exactly as it was. What changes is that
        // the screen now says the refresh did not happen and offers another.
        self.failure = Some(if self.stories.is_empty() {
            format!("{why} There is nothing saved to fall back on.")
        } else {
            format!("{why} The brief below is the one already saved.")
        });
        self.complete(context);
    }
}

fn parse_story(body: &[u8]) -> Option<Story> {
    let text = std::str::from_utf8(body).ok()?;
    let value = kobo_json::parse(text).ok()?;
    let title = value.get("title")?.as_str()?;
    if title.is_empty() {
        return None;
    }
    let url = value.get("url").and_then(Value::as_str).unwrap_or_default();
    let site = if url.is_empty() {
        "news.ycombinator.com".to_owned()
    } else {
        site_of(url)
    };
    Some(Story {
        title: clean(title),
        site: clean(&site),
        url: clean(url),
        // A question or a show-and-tell has no address of its own; what the
        // poster wrote is the whole of it.
        text: value
            .get("text")
            .and_then(Value::as_str)
            .map(clean)
            .unwrap_or_default(),
    })
}

impl KoboApp for Brief {
    fn on_start(&mut self, context: &mut Context) {
        context.store().load(STORED);
        self.show(context);
    }

    fn on_save(&mut self, context: &mut Context, key: &str, result: StoreResult) {
        self.article_result(context, key, &result);
    }

    fn on_shelf(&mut self, context: &mut Context, name: &str, result: StoreResult) {
        if self
            .illustrations
            .store(context, &mut self.book, name, &result, true)
        {
            self.show(context);
            return;
        }
        if let Some(article) = self.article.as_mut().filter(|saved| saved.owns_file(name)) {
            let event = article.shelf(context, &result);
            self.article_event(context, event);
        }
    }

    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        match result {
            StoreResult::Loaded { key, value } => self.opened(context, &key, value),
            StoreResult::Denied(reason) => {
                self.loaded = true;
                context.log(
                    LogLevel::Warn,
                    format!("the brief could not be kept: {reason}"),
                );
                self.show(context);
            }
            // Listed rather than wildcarded, so adding a store answer to the
            // protocol makes every application decide what it means here.
            // This one keeps nothing on the shelf.
            StoreResult::Saved { .. }
            | StoreResult::Forgotten { .. }
            | StoreResult::Keys(_)
            | StoreResult::ShelfWritten { .. }
            | StoreResult::ShelfRead { .. }
            | StoreResult::ShelfRemoved { .. }
            | StoreResult::Shelf(_) => {}
        }
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        // The open story answers its own page turns and controls first.
        if self.view == View::Reading && self.book.memory().is_some() {
            match self.book.act(context, action) {
                Some(kobo_read::Outcome::Close) => self.close_story(context),
                Some(kobo_read::Outcome::Light(level)) => context.device().set_frontlight(level),
                None if action == ActionId::BACK || action == action_id("close-story") => {
                    self.close_story(context);
                }
                _ => {}
            }
            self.show(context);
            return;
        }
        if action == action_id(REFRESH) {
            self.start_refresh(context);
        } else if action == action_id("previous") {
            self.page = self.page.saturating_sub(1);
        } else if action == action_id("next") {
            self.page = (self.page + 1).min(self.pages(context).len().saturating_sub(1));
        } else if action == action_id("sources") {
            self.view = View::Sources;
        } else if action == action_id("close-sources") || action == ActionId::BACK {
            self.view = View::Brief;
        } else if let Some(chosen) = Source::ALL
            .into_iter()
            .find(|source| action == action_id(source.action()))
        {
            self.view = View::Brief;
            if chosen != self.source {
                self.source = chosen;
                // The brief on the panel came from somewhere else, so it is
                // no longer what this source says. Saying so beats leaving
                // yesterday's other list under a new heading.
                self.stories.clear();
                self.fetched = None;
                self.failure = None;
                self.note = Some(format!("{} selected. Refresh to fetch it.", chosen.label()));
                self.keep(context);
            }
        } else if let Some(index) =
            (0..self.stories.len()).find(|index| action == action_id(&format!("story-{index}")))
        {
            self.open_story(context, index);
            return;
        }
        self.show(context);
    }

    fn on_background(&mut self, context: &mut Context) {
        self.background = true;
        // Written now, because this is the last certain moment. Nothing here
        // stops: the fetch in flight keeps running and its answer will still
        // arrive.
        if !self.stories.is_empty() {
            self.keep(context);
        }
    }

    fn on_foreground(&mut self, context: &mut Context) {
        self.background = false;
        // Whatever arrived while nobody was looking is drawn now, in one
        // refresh rather than one per story.
        self.show(context);
    }

    fn on_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
        // Only the one thing this application is actually waiting for. An
        // outcome for anything else is ignored rather than mistaken for the
        // answer to what is outstanding.
        if self
            .illustrations
            .task(context, &mut self.book, task, &outcome)
        {
            self.show(context);
            return;
        }
        if self.book.woke(context, task, &outcome) != kobo_bookview::Step::Elsewhere {
            if self.view == View::Reading {
                self.show(context);
            }
            return;
        }
        let stage = self.fetching;
        let waiting = match stage {
            Fetching::Nothing => return,
            Fetching::Index(waiting) | Fetching::Story(waiting, _) | Fetching::Article(waiting) => {
                waiting
            }
        };
        if waiting != task {
            return;
        }
        match outcome {
            TaskOutcome::Completed(body) => match stage {
                Fetching::Index(_) => self.on_index(context, &body),
                Fetching::Story(_, position) => self.on_story(context, position, &body),
                Fetching::Article(_) => {
                    self.fetching = Fetching::Nothing;
                    self.note = None;
                    // Saved before it is read, so the second opening costs
                    // nothing and works with no network at all.
                    if !self
                        .article
                        .as_mut()
                        .is_some_and(|saved| saved.save(context, body.clone()))
                    {
                        self.note = Some(
                            "This story is open but not saved for offline reading.".to_owned(),
                        );
                    }
                    self.read_article(context, &body);
                }
                Fetching::Nothing => {}
            },
            TaskOutcome::Failed(error) => {
                // The SDK owns the wording, so every application says the
                // same thing about the same failure and a new TaskError
                // variant does not need five edits.
                let why = Failure::of(error).advice;
                // A story that fails is skipped; an index that fails ends the
                // refresh, because there is nothing to skip to.
                if matches!(stage, Fetching::Article(_)) {
                    self.fetching = Fetching::Nothing;
                    self.note = Some(format!("This story could not be opened. {why}"));
                    self.reading = None;
                } else if matches!(stage, Fetching::Story(_, _)) {
                    self.note = Some(why.to_owned());
                    self.advance(context);
                } else {
                    self.fail(context, why);
                }
            }
            TaskOutcome::Cancelled => self.fail(context, "The refresh was stopped."),
        }
        self.show(context);
    }
}

fn main() -> ExitCode {
    match kobo_sdk::run("brief", Brief::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("brief: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        decode, encode, parse_story, site_of, Brief, Fetching, Source, Story, View, REFRESH,
        STORED, STORIES,
    };
    use kobo_sdk::{action_id, Command, Context, KoboApp, Task, TaskId, TaskOutcome};

    fn spawned(commands: &[Command]) -> TaskId {
        commands
            .iter()
            .find_map(|command| match command {
                Command::Spawn { task, .. } => Some(*task),
                _ => None,
            })
            .expect("nothing was started")
    }

    fn url_of(commands: &[Command]) -> String {
        commands
            .iter()
            .find_map(|command| match command {
                Command::Spawn {
                    work: Task::Fetch { url, .. },
                    ..
                } => Some(url.clone()),
                _ => None,
            })
            .expect("nothing was fetched")
    }

    fn ready() -> Brief {
        Brief {
            loaded: true,
            ..Brief::default()
        }
    }

    fn refreshing(context: &mut Context) -> (Brief, TaskId, Vec<Command>) {
        let mut brief = ready();
        brief.on_action(context, action_id(REFRESH));
        let commands = context.take_commands();
        let task = spawned(&commands);
        (brief, task, commands)
    }

    #[test]
    fn a_brief_survives_being_written_and_read_back() {
        let stories = vec![Story {
            title: "A title, with a comma".into(),
            site: "example.org".into(),
            url: "https://example.org/a-title".into(),
            text: String::new(),
        }];
        let saved = decode(&encode(Source::Ask, Some("2026-09-11 08:30 UTC"), &stories));
        assert_eq!(saved.stories, stories);
        assert_eq!(saved.source, Source::Ask);
        assert_eq!(saved.fetched.as_deref(), Some("2026-09-11 08:30 UTC"));
    }

    /// A brief written by the previous version still opens. It simply cannot
    /// say when it was fetched, because nothing wrote that down.
    #[test]
    fn a_brief_written_by_the_previous_version_still_opens() {
        let saved = decode(b"A title\texample.org\nAnother\telsewhere.test\n");
        assert_eq!(saved.stories.len(), 2);
        assert_eq!(saved.stories[0].site, "example.org");
        assert!(saved.stories[0].url.is_empty());
        assert_eq!(saved.fetched, None);
        assert_eq!(saved.source, Source::Top);
    }

    #[test]
    fn a_story_reports_where_it_is_from_rather_than_its_whole_address() {
        assert_eq!(site_of("https://www.bbc.co.uk/news/1234"), "bbc.co.uk");
        assert_eq!(site_of("https://example.org"), "example.org");
    }

    #[test]
    fn a_story_with_no_link_is_attributed_to_the_site_itself() {
        let story = parse_story(br#"{"title":"Ask HN: anything","by":"someone"}"#)
            .expect("the story was dropped");
        assert_eq!(story.site, "news.ycombinator.com");
    }

    #[test]
    fn a_tab_in_a_title_cannot_break_the_stored_format() {
        // Otherwise a title containing a tab would split into a title and a
        // site on the way back in, and the brief would be quietly wrong.
        let story =
            parse_story(b"{\"title\":\"one\\ttwo\",\"url\":\"https://e.org/\"}").expect("dropped");
        assert_eq!(
            decode(&encode(Source::Top, None, &[story])).stories.len(),
            1
        );
    }

    #[test]
    fn the_index_is_read_and_the_first_story_is_asked_for() {
        let mut context = Context::default();
        let (mut brief, task, started) = refreshing(&mut context);
        assert_eq!(url_of(&started), Source::Top.index());
        brief.on_task(
            &mut context,
            task,
            TaskOutcome::Completed(b"[8863,8864,8865,8866,8867,8868,8869]".to_vec()),
        );
        let commands = context.take_commands();
        assert_eq!(
            url_of(&commands),
            format!("{}/v0/item/8863.json", super::origin())
        );
        assert_eq!(brief.building.len(), STORIES);
    }

    #[test]
    fn work_started_before_leaving_still_finishes_afterwards() {
        // This is the whole point of the example. Nothing about being in the
        // background stops the fetch, and the answer is taken exactly as it
        // would have been.
        let mut context = Context::default();
        let (mut brief, task, _started) = refreshing(&mut context);
        brief.on_background(&mut context);
        let _ignored = context.take_commands();
        brief.on_task(
            &mut context,
            task,
            TaskOutcome::Completed(b"[8863]".to_vec()),
        );
        assert!(
            matches!(brief.fetching, Fetching::Story(_, 0)),
            "leaving stopped the work"
        );
        assert!(
            context
                .take_commands()
                .iter()
                .all(|command| !matches!(command, Command::SetScreen(_))),
            "a background application drew to a panel nobody was looking at"
        );
    }

    #[test]
    fn coming_back_draws_what_arrived_while_nobody_was_looking() {
        let mut context = Context::default();
        let (mut brief, task, _started) = refreshing(&mut context);
        brief.on_background(&mut context);
        brief.on_task(
            &mut context,
            task,
            TaskOutcome::Completed(b"[8863]".to_vec()),
        );
        let story = spawned(&context.take_commands());
        brief.on_task(
            &mut context,
            story,
            TaskOutcome::Completed(
                br#"{"title":"Something happened","url":"https://e.org/x"}"#.to_vec(),
            ),
        );
        let _ignored = context.take_commands();
        brief.on_foreground(&mut context);
        let commands = context.take_commands();
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, Command::SetScreen(_))),
            "coming back drew nothing"
        );
        assert_eq!(brief.stories.len(), 1);
        assert_eq!(brief.stories[0].title, "Something happened");
    }

    #[test]
    fn a_finished_brief_is_written_down_without_being_asked() {
        let mut context = Context::default();
        let (mut brief, task, _started) = refreshing(&mut context);
        brief.on_task(
            &mut context,
            task,
            TaskOutcome::Completed(b"[8863]".to_vec()),
        );
        let story = spawned(&context.take_commands());
        brief.on_task(
            &mut context,
            story,
            TaskOutcome::Completed(br#"{"title":"Kept","url":"https://e.org/x"}"#.to_vec()),
        );
        let saved = context.take_commands().iter().any(|command| {
            matches!(
                command,
                Command::Store(kobo_sdk::StoreRequest::Save { key, .. }) if key == super::STORED
            )
        });
        assert!(saved, "the brief was not kept");
    }

    #[test]
    fn a_failed_refresh_leaves_the_previous_brief_alone() {
        let mut context = Context::default();
        let (mut brief, task, _started) = refreshing(&mut context);
        brief.stories = vec![Story {
            title: "yesterday".into(),
            site: "e.org".into(),
            ..Story::default()
        }];
        brief.fetched = Some("2026-09-10 07:00 UTC".into());
        brief.on_task(
            &mut context,
            task,
            TaskOutcome::Failed(kobo_sdk::TaskError::Unreachable),
        );
        assert_eq!(brief.stories.len(), 1, "a failure emptied the brief");
        let failure = brief.failure.clone().expect("a failure said nothing");
        assert!(failure.contains("already saved"), "{failure}");
        // The line above the stories still says when they were fetched, and
        // the one control on the screen offers another attempt.
        assert!(
            brief.as_of().contains("2026-09-10 07:00 UTC"),
            "{}",
            brief.as_of()
        );
        let drawn = brief
            .screen(&Context::default())
            .layout_with(
                &kobo_ui::CLARA_BW_METRICS,
                &kobo_ui::Chrome::measuring(true),
            )
            .nodes
            .iter()
            .flat_map(|node| node.text_lines.clone())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(drawn.contains("Try again"), "{drawn}");
    }

    /// Choosing another list does not leave the last one's stories under the
    /// new heading, and says what to do about it.
    #[test]
    fn choosing_another_source_clears_a_brief_that_came_from_the_old_one() {
        let mut context = Context::default();
        let mut brief = ready();
        brief.stories = vec![Story {
            title: "from the front page".into(),
            site: "e.org".into(),
            ..Story::default()
        }];
        brief.fetched = Some("2026-09-10 07:00 UTC".into());
        brief.on_action(&mut context, action_id("sources"));
        assert_eq!(brief.view, View::Sources);
        brief.on_action(&mut context, action_id(Source::Ask.action()));
        assert_eq!(brief.view, View::Brief);
        assert_eq!(brief.source, Source::Ask);
        assert!(
            brief.stories.is_empty(),
            "another list kept the old stories"
        );
        assert_eq!(brief.fetched, None);
        assert!(
            brief.as_of().contains("Ask Hacker News"),
            "{}",
            brief.as_of()
        );
        let written = context
            .take_commands()
            .into_iter()
            .find_map(|command| match command {
                Command::Store(kobo_sdk::StoreRequest::Save { key, value }) if key == STORED => {
                    Some(value)
                }
                _ => None,
            });
        assert_eq!(
            decode(&written.expect("the choice was written down")).source,
            Source::Ask
        );
    }

    /// Every screen this application draws, at every size the interface has.
    #[test]
    fn every_screen_fits_each_supported_text_size() {
        for text_scale in kobo_ui::TextScale::STEPS {
            let metrics = kobo_ui::DisplayMetrics {
                text_scale,
                ..kobo_ui::CLARA_BW_METRICS
            };
            let mut brief = ready();
            brief.source = Source::Best;
            brief.fetched = Some("2026-09-11 08:30 UTC".into());
            brief.failure =
                Some("The reader is offline. The brief below is the one already saved.".into());
            brief.stories = (0..STORIES)
                .map(|index| Story {
                    title: format!("A headline {index} long enough to wrap on a narrow panel"),
                    site: "example.org".into(),
                    url: format!("https://example.org/{index}"),
                    text: String::new(),
                })
                .collect();
            for view in [View::Brief, View::Sources] {
                brief.view = view;
                let diagnostics = brief
                    .screen(&kobo_sdk::AppRunner::with_metrics(Brief::default(), metrics).context())
                    .diagnostics(&metrics, &kobo_ui::Chrome::measuring(true));
                assert!(
                    diagnostics.issues.is_empty(),
                    "{text_scale:?} {view:?}: {:?}",
                    diagnostics.issues
                );
            }
        }
    }

    /// A linked story is fetched once, saved, and afterwards opened from the
    /// copy on the device.
    #[test]
    fn a_linked_story_is_saved_and_reopened_without_another_request() {
        let mut context = Context::default();
        let mut brief = ready();
        brief.stories = vec![Story {
            title: "A morning by the river".into(),
            site: "example.org".into(),
            url: "https://example.org/river".into(),
            text: String::new(),
        }];
        brief.on_action(&mut context, action_id("story-0"));
        let key = brief
            .article
            .as_ref()
            .expect("a saved copy was opened")
            .key
            .clone();
        // Nothing is asked of the network until the store has answered.
        assert!(!context
            .take_commands()
            .iter()
            .any(|command| matches!(command, Command::Spawn { .. })));
        brief.on_load(
            &mut context,
            &key,
            kobo_sdk::StoreResult::Loaded {
                key: key.clone(),
                value: None,
            },
        );
        let fetch = context.take_commands();
        assert!(fetch.iter().any(
            |command| matches!(command, Command::Spawn { work: Task::Fetch { url, .. }, .. }
                if url == "https://example.org/river")
        ));
        let Fetching::Article(task) = brief.fetching else {
            panic!("the story was not being fetched")
        };
        brief.on_task(
            &mut context,
            task,
            TaskOutcome::Completed(b"<h1>A morning</h1><p>The first light.</p>".to_vec()),
        );
        assert_eq!(brief.view, View::Reading);
        let written = context.take_commands();
        assert!(
            written.iter().any(|command| matches!(
                command,
                Command::Store(kobo_sdk::StoreRequest::ShelfWrite { .. })
            )),
            "the story was read but not saved"
        );
    }

    /// A story with nothing but its own text is read without asking the
    /// network for anything.
    #[test]
    fn a_question_is_read_from_what_the_poster_wrote() {
        let mut context = Context::default();
        let mut brief = ready();
        brief.stories = vec![Story {
            title: "Ask HN: what do you read on a Kobo?".into(),
            site: "news.ycombinator.com".into(),
            url: String::new(),
            text: "<p>Something without a link of its own.</p>".into(),
        }];
        brief.on_action(&mut context, action_id("story-0"));
        assert_eq!(brief.view, View::Reading);
        assert!(
            !context
                .take_commands()
                .iter()
                .any(|command| matches!(command, Command::Spawn { .. })),
            "reading a question asked the network for something"
        );
        let drawn = brief
            .screen(&Context::default())
            .layout_with(&kobo_ui::CLARA_BW_METRICS, &kobo_ui::Chrome::default())
            .nodes
            .iter()
            .flat_map(|node| node.text_lines.clone())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(drawn.contains("Something without a link"), "{drawn}");
    }
}

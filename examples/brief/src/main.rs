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
//!
//! ## Why it refreshes itself
//!
//! A brief that waits for a tap is a brief that is always yesterday's. Once an
//! owner has refreshed it once, it asks the runtime to wake it each morning and
//! refreshes then, and it refreshes again whenever it is opened more than six
//! hours after the last one. Each story behind a headline is kept on the
//! device as it arrives, with its pictures, so the morning's brief reads on a
//! train with the radio off.

use kobo_bookview::offline::{Heard, Keeper};
use kobo_bookview::positions::{self, Progress};
use kobo_bookview::BookView;
use kobo_json::Value;
use kobo_sdk::{
    action_id, ActionId, Context, Failure, Glyph, KoboApp, LogLevel, Screen, ScreenBuilder, Space,
    StoreResult, Task, TaskId, TaskOutcome,
};
use std::process::ExitCode;
use std::time::Duration;

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
/// Six was one panel's worth, and it was too few: a morning's brief that
/// runs out before the coffee does sends an owner back to a browser. Twelve
/// is two panels at the size most people read at, which is still a brief and
/// not a feed to be managed.
const STORIES: usize = 12;

/// How old a brief may be before opening the application fetches a new one.
///
/// Long enough that flicking in and out of the application during a morning
/// costs no radio, short enough that the evening's brief is not the morning's.
const STALE_SECONDS: u64 = 6 * 60 * 60;

/// The local hour the morning wake is asked for.
///
/// Early enough that the brief is ready before anybody reads it, and late
/// enough that the night's stories have reached the front page.
const WAKE_HOUR: u64 = 6;

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
    /// The item number on Hacker News, which is what the story is kept on the
    /// device under. Empty for a brief written before it was recorded.
    id: String,
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
    /// The same moment in seconds since 1970, which is what deciding whether
    /// the brief is stale needs. `None` for a brief written before it was
    /// recorded, which counts as stale.
    fetched_at: Option<u64>,
    /// Why the last refresh did not happen, if it did not. Kept apart from
    /// `note` so a failure survives until something replaces it.
    failure: Option<String>,
    view: View,
    /// The story being read, as a position in the brief.
    reading: Option<usize>,
    book: BookView,
    /// The stories behind the headlines, kept whole on the device.
    keeper: Keeper,
    /// Where each story was left, and the text size last chosen.
    progress: Progress,
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
    /// The refresh being finished is finishing because it failed.
    failed_now: bool,
}

impl Default for Brief {
    fn default() -> Self {
        Self {
            stories: Vec::new(),
            page: 0,
            source: Source::default(),
            fetched: None,
            fetched_at: None,
            failure: None,
            view: View::default(),
            reading: None,
            book: BookView::new(),
            keeper: Keeper::default(),
            progress: Progress::default(),
            queue: Vec::new(),
            building: Vec::new(),
            fetching: Fetching::Nothing,
            background: false,
            loaded: false,
            note: None,
            failed_now: false,
        }
    }
}

/// Encodes the brief for the store: one story a line, title and site tabbed.
///
/// A tab is used rather than a comma because a title can contain a comma and
/// cannot contain a tab; anything that arrives with one has it replaced when the
/// story is built, so this cannot be ambiguous.
fn encode(source: Source, fetched: Option<&str>, at: Option<u64>, stories: &[Story]) -> Vec<u8> {
    let mut out = format!(
        "brief-v2\t{}\t{}\t{}\n",
        source.saved(),
        fetched.unwrap_or(""),
        at.map(|at| at.to_string()).unwrap_or_default()
    );
    for story in stories {
        out.push_str(&story.title);
        out.push('\t');
        out.push_str(&story.site);
        out.push('\t');
        out.push_str(&story.url);
        out.push('\t');
        out.push_str(&story.text);
        // Last, so a version that reads four fields reads this brief too.
        out.push('\t');
        out.push_str(&story.id);
        out.push('\n');
    }
    out.into_bytes()
}

/// What was written down: the stories, where they came from and when.
#[derive(Debug, Default, Eq, PartialEq)]
struct Saved {
    source: Source,
    fetched: Option<String>,
    fetched_at: Option<u64>,
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
        ["brief-v2", source, fetched, at] => {
            saved.source = Source::from_saved(source);
            saved.fetched = (!fetched.is_empty()).then(|| fetched.to_owned());
            saved.fetched_at = at.parse().ok();
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
                id: fields
                    .next()
                    .filter(|id| id.bytes().all(|byte| byte.is_ascii_digit()))
                    .unwrap_or_default()
                    .to_owned(),
            })
        })
        .take(STORIES)
        .collect();
    saved
}

/// The reader's clock, read once.
///
/// At the reader's own offset from UTC. This used to read the clock at UTC and
/// print "UTC" after it, which put a brief fetched over breakfast in New York
/// at "10:30 UTC" and one fetched in the evening in Sydney on yesterday's date.
fn now() -> Option<kobo_sdk::clock::Snapshot> {
    use kobo_sdk::clock::{Clock, SystemClock};
    SystemClock::reader().now().ok()
}

/// A moment as a line a reader can check against their own watch.
fn timestamp(at: kobo_sdk::clock::Snapshot) -> Option<String> {
    let (hour, minute) = at.hour_minute()?;
    Some(format!("{} {hour:02}:{minute:02}", at.date()?))
}

/// How long until the next morning wake, from `at`.
fn until_morning(at: kobo_sdk::clock::Snapshot) -> Duration {
    let local =
        (at.unix_millis / 1000).saturating_add_signed(i64::from(at.utc_offset_minutes) * 60);
    let into_day = local % 86_400;
    let target = WAKE_HOUR * 3600;
    let wait = if into_day < target {
        target - into_day
    } else {
        86_400 - into_day + target
    };
    Duration::from_secs(wait.max(60))
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
        if !self.stories.is_empty() {
            screen = screen.secondary(self.offline_line());
        }
        if let Some(failure) = &self.failure {
            screen = screen.banner(kobo_sdk::BannerLevel::Attention, failure.clone());
        }
        if let Some(note) = &self.note {
            screen = screen.text(note.clone());
        }
        // No count of stories or sites and no heading over the list. The line
        // above already names the list and the numbers down the side count the
        // stories; a band of facts and a heading saying the same again cost a
        // whole headline on every page.
        let _ = context;
        screen
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
        if self.reading.is_some()
            && (self.keeper.is_opening() || matches!(self.fetching, Fetching::Article(_)))
        {
            let title = self
                .reading
                .and_then(|index| self.stories.get(index))
                .map_or_else(String::new, |story| story.title.clone());
            return ScreenBuilder::new("brief-opening")
                .top_bar(title)
                .owns_back(true)
                .activity(
                    if self.keeper.is_opening() {
                        "Opening the saved story"
                    } else {
                        "Fetching the story"
                    },
                    None,
                )
                .build();
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
                "Tap Refresh once the reader is on Wi-Fi. After that the brief \
                 refreshes itself each morning.",
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
            let total = self.building.len().max(1) as u64;
            let percent = u8::try_from(done.saturating_mul(100) / total).unwrap_or(100);
            screen = screen
                .spacer(Space::Medium)
                .activity(
                    format!("Collecting stories, {done} of {total}"),
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
                "All four are public Hacker News lists. Nothing is fetched until you \
                 refresh once; after that the brief refreshes itself each morning.",
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

    /// What is on the device, in the words every application uses for it.
    fn offline_line(&self) -> String {
        let line = self.keeper.summary("story", "stories");
        if self.keeper.is_busy() {
            format!(
                "{}. Saving stories for offline reading.",
                line.trim_end_matches('.')
            )
        } else {
            line
        }
    }

    /// Whether the brief on hand is old enough that opening the application
    /// should fetch another.
    ///
    /// Only once there is a brief at all. The first fetch is the owner's to
    /// ask for, because until then nothing says which list they want or that
    /// this reader has Wi-Fi to spend on it.
    fn is_stale(&self, now: Option<u64>) -> bool {
        if self.stories.is_empty() && self.fetched.is_none() {
            return false;
        }
        match (self.fetched_at, now) {
            (Some(at), Some(now)) => now.saturating_sub(at) >= STALE_SECONDS || at > now,
            _ => true,
        }
    }

    /// Fetches a new brief if this one has gone stale.
    fn refresh_if_stale(&mut self, context: &mut Context) {
        let seconds = now().map(|at| at.unix_millis / 1000);
        if self.loaded && self.fetching == Fetching::Nothing && self.is_stale(seconds) {
            self.start_refresh(context);
        }
    }

    /// Asks the runtime to wake this application tomorrow morning.
    ///
    /// Asked again after every refresh rather than once, because a wake is
    /// one occurrence and the runtime may move it to fit others.
    fn schedule_morning(context: &mut Context) {
        if let Some(at) = now() {
            context.device().schedule_wake(until_morning(at));
        }
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
            // Not said under a failure that already says why, which put two
            // sentences about the same nothing one above the other.
            if self.note.is_none() && !self.failed_now {
                self.note = Some("The refresh brought back nothing.".to_owned());
            }
        } else {
            self.stories = collected;
            let at = now();
            self.fetched = at.and_then(timestamp);
            self.fetched_at = at.map(|at| at.unix_millis / 1000);
            self.failure = None;
            self.page = 0;
            self.keep(context);
            self.keep_stories(context);
            Self::schedule_morning(context);
        }
        self.building.clear();
        self.queue.clear();
        self.failed_now = false;
    }

    /// Keeps every linked story of this brief on the device, and lets go of
    /// the ones from briefs before it.
    fn keep_stories(&mut self, context: &mut Context) {
        let current: Vec<String> = self.stories.iter().map(|story| story.id.clone()).collect();
        self.keeper
            .retain(context, |id| current.iter().any(|kept| kept == id));
        for story in &self.stories {
            if !story.url.is_empty() && !story.id.is_empty() {
                self.keeper.keep_page(context, &story.id, &story.url);
            }
        }
    }

    /// Writes the brief down, with where it came from and when.
    fn keep(&self, context: &mut Context) {
        context.store().save(
            STORED,
            encode(
                self.source,
                self.fetched.as_deref(),
                self.fetched_at,
                &self.stories,
            ),
        );
    }

    /// Takes whatever the store had: the brief, or the one the previous
    /// version wrote.
    fn opened(&mut self, context: &mut Context, key: &str, value: Option<Vec<u8>>) {
        if key == STORED {
            if let Some(saved) = value.map(|bytes| decode(&bytes)) {
                if !saved.stories.is_empty() {
                    self.stories = saved.stories;
                    self.source = saved.source;
                    self.fetched = saved.fetched;
                    self.fetched_at = saved.fetched_at;
                    self.loaded = true;
                    self.arrived(context);
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
        }
        self.loaded = true;
        self.arrived(context);
    }

    /// The saved brief is on hand. A stale one is refreshed straight away and
    /// the morning wake is asked for again, since a wake the runtime dropped
    /// while the application was closed is otherwise never asked for.
    fn arrived(&mut self, context: &mut Context) {
        if !self.stories.is_empty() {
            Self::schedule_morning(context);
        }
        self.refresh_if_stale(context);
        self.show(context);
    }

    /// What the keeper of stories said, acted on.
    fn heard(&mut self, context: &mut Context, heard: Heard) {
        match heard {
            Heard::Elsewhere | Heard::Quiet => return,
            Heard::Changed | Heard::Failed(..) => {
                // One repaint when the last story is put away, not one per
                // story: twelve full refreshes of a panel somebody is reading
                // a headline on is twelve flashes for one changed line.
                if self.keeper.is_busy() && self.view == View::Brief {
                    return;
                }
            }
            Heard::Opened(id, kept) => {
                if self.reading_id() == Some(id.as_str()) && self.view != View::Reading {
                    let memory = self.memory();
                    if self.book.open_kept(context, kept, memory) {
                        self.view = View::Reading;
                    } else {
                        self.fetch_article(context);
                    }
                }
            }
            Heard::Unreadable(id) => {
                // The copy on the device would not read. The keeper has
                // already stopped claiming it is there.
                if self.reading_id() == Some(id.as_str()) && self.view != View::Reading {
                    self.fetch_article(context);
                }
            }
        }
        self.show(context);
    }

    /// The item number of the story being opened or read.
    fn reading_id(&self) -> Option<&str> {
        self.reading
            .and_then(|index| self.stories.get(index))
            .map(|story| story.id.as_str())
            .filter(|id| !id.is_empty())
    }

    /// The identity the open story's place is kept under.
    fn place(&self) -> Option<String> {
        let story = self.reading.and_then(|index| self.stories.get(index))?;
        Some(if story.url.is_empty() {
            positions::identity(&format!("brief-story:{}:{}", story.id, story.title))
        } else {
            positions::identity(&format!("brief-article:{}", story.url))
        })
    }

    /// Where the open story was left, or its start at the size last chosen.
    fn memory(&self) -> kobo_read::Memory {
        self.place()
            .map(|id| self.progress.memory(&id))
            .unwrap_or_default()
    }

    /// Writes down where the open story has got to.
    fn keep_position(&mut self, context: &mut Context) {
        if let (Some(id), Some(memory)) = (self.place(), self.book.memory()) {
            self.progress.keep(context, id, memory.clone());
        }
    }

    fn fetch_article(&mut self, context: &mut Context) {
        self.keeper.stop_opening();
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
            self.reading = None;
        }
    }

    /// A fetched story, cut to the article and put in front of the reader.
    ///
    /// Kept as it is opened when the keeper has not got to it yet, so the
    /// second opening needs no radio.
    fn read_article(&mut self, context: &mut Context, body: &[u8]) {
        let Some(story) = self
            .reading
            .and_then(|index| self.stories.get(index))
            .cloned()
        else {
            return;
        };
        let source = String::from_utf8_lossy(body).into_owned();
        if !story.id.is_empty()
            && !self.keeper.is_kept(&story.id)
            && !self.keeper.is_keeping(&story.id)
        {
            self.keeper.keep_markup(
                context,
                &story.id,
                &story.url,
                &kobo_doc::web::main_content(&source),
            );
        }
        let memory = self.memory();
        if self.book.open_article(context, &source, &story.url, memory) {
            self.view = View::Reading;
        } else {
            self.note = Some("This story arrived empty.".to_owned());
            self.reading = None;
        }
    }

    /// Opens one story: the copy on the device first, the site only if there
    /// is none.
    fn open_story(&mut self, context: &mut Context, index: usize) {
        let Some(story) = self.stories.get(index).cloned() else {
            return;
        };
        self.reading = Some(index);
        self.note = None;
        if story.url.is_empty() {
            // A question or a show-and-tell is its own text, and there is
            // nothing to fetch or to keep.
            let body = format!("<h2>{}</h2>{}", story.title, story.text);
            let memory = self.memory();
            if self
                .book
                .open_bytes(context, "story.html", body.as_bytes(), memory)
                .is_ok()
            {
                self.view = View::Reading;
            }
        } else if !self.keeper.open(context, &story.id) {
            self.fetch_article(context);
        }
        self.show(context);
    }

    fn close_story(&mut self, context: &mut Context) {
        // The place goes down before the document it points into goes away.
        self.keep_position(context);
        self.book.close(context);
        self.keeper.stop_opening();
        if let Fetching::Article(task) = self.fetching {
            context.cancel(task);
            self.fetching = Fetching::Nothing;
        }
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
        self.failed_now = true;
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
        id: value
            .get("id")
            .and_then(Value::as_i64)
            .filter(|id| *id > 0)
            .map(|id| id.to_string())
            .unwrap_or_default(),
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
        context.store().load(positions::KEY);
        self.keeper.start(context);
        // Earlier versions kept a copy of every story ever opened, and of
        // every picture in it, each under a store key of its own, and never
        // let any of them go. Those copies are found and removed once, here.
        context.store().list();
        self.show(context);
    }

    fn on_load(&mut self, context: &mut Context, key: &str, result: StoreResult) {
        if key == positions::KEY {
            self.progress.load(result);
            return;
        }
        let heard = self.keeper.loaded(context, key, &result);
        if !matches!(heard, Heard::Elsewhere) {
            // The index has arrived, so stories from briefs before this one
            // can be let go of now rather than at the next refresh.
            if !self.stories.is_empty() && self.fetching == Fetching::Nothing {
                self.keep_stories(context);
            }
            self.heard(context, heard);
            return;
        }
        self.on_store(context, result);
    }

    fn on_save(&mut self, context: &mut Context, key: &str, result: StoreResult) {
        if key == positions::KEY {
            self.progress.saved(context, result);
            return;
        }
        let heard = self.keeper.saved(context, key, &result);
        self.heard(context, heard);
    }

    fn on_shelf(&mut self, context: &mut Context, name: &str, result: StoreResult) {
        let heard = self.keeper.shelf(context, name, &result);
        self.heard(context, heard);
    }

    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        match result {
            StoreResult::Loaded { key, value } => self.opened(context, &key, value),
            StoreResult::Keys(keys) => {
                for key in keys.iter().filter(|key| is_old_copy(key)) {
                    context.store().forget(key.clone());
                    for slot in 0..2 {
                        context.shelf().remove(format!("{}.{slot}", &key[..60]));
                    }
                }
            }
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
            StoreResult::Saved { .. }
            | StoreResult::Forgotten { .. }
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
                Some(kobo_read::Outcome::Light(level)) => {
                    context.device().set_frontlight(level);
                    self.keep_position(context);
                }
                Some(kobo_read::Outcome::Save) => self.keep_position(context),
                None if action == ActionId::BACK || action == action_id("close-story") => {
                    self.close_story(context);
                }
                _ => {}
            }
            self.show(context);
            return;
        }
        if self.reading.is_some() && action == ActionId::BACK {
            // Back while a story is still on its way abandons it.
            self.close_story(context);
        } else if action == action_id(REFRESH) {
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
                // A refresh already on its way is for the old list, and left
                // to finish it would put that list under the new heading.
                if let Fetching::Index(task) | Fetching::Story(task, _) = self.fetching {
                    context.cancel(task);
                    self.fetching = Fetching::Nothing;
                    self.building.clear();
                    self.queue.clear();
                }
                self.source = chosen;
                // The brief on the panel came from somewhere else, so it is
                // no longer what this source says. Saying so beats leaving
                // yesterday's other list under a new heading.
                self.stories.clear();
                self.fetched = None;
                self.fetched_at = None;
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
        self.keep_position(context);
    }

    fn on_foreground(&mut self, context: &mut Context) {
        self.background = false;
        // A brief left open overnight is this morning's the moment it is
        // looked at again, not when somebody remembers to tap Refresh.
        if self.view == View::Brief {
            self.refresh_if_stale(context);
        }
        // Whatever arrived while nobody was looking is drawn now, in one
        // refresh rather than one per story.
        self.show(context);
    }

    fn on_scheduled_wake(&mut self, context: &mut Context) {
        // The morning wake. The brief is fetched whether or not it looks
        // stale: the point of waking is that it is fresh when the cover opens.
        if self.loaded && self.fetching == Fetching::Nothing && !self.stories.is_empty() {
            self.start_refresh(context);
        } else {
            Self::schedule_morning(context);
        }
        self.show(context);
    }

    fn on_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
        self.took_task(context, task, outcome);
        // A lane the keeper could not have a moment ago may be free now.
        self.keeper.pump(context);
    }
}

impl Brief {
    fn took_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
        let heard = self.keeper.task(context, task, &outcome);
        if !matches!(heard, Heard::Elsewhere) {
            self.heard(context, heard);
            return;
        }
        if self.book.woke(context, task, &outcome) != kobo_bookview::Step::Elsewhere {
            if self.view == View::Reading {
                self.show(context);
            }
            return;
        }
        // Only the one thing this application is actually waiting for. An
        // outcome for anything else is ignored rather than mistaken for the
        // answer to what is outstanding.
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
            TaskOutcome::Cancelled => {
                if matches!(stage, Fetching::Article(_)) {
                    self.fetching = Fetching::Nothing;
                } else {
                    self.fail(context, "The refresh was stopped.");
                }
            }
        }
        self.show(context);
    }
}

/// Whether a store key is a copy an earlier version kept of a story or a
/// picture.
///
/// Those were named by a SHA-256 digest and nothing else this application
/// writes is, so the shape alone tells them apart.
fn is_old_copy(key: &str) -> bool {
    key.len() == 64
        && key
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
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
    use kobo_sdk::{
        action_id, AppRunner, Command, Context, KoboApp, StoreError, StoreRequest, StoreResult,
        Task, TaskId, TaskOutcome,
    };
    use std::collections::BTreeMap;

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
            id: "8863".into(),
            title: "A title, with a comma".into(),
            site: "example.org".into(),
            url: "https://example.org/a-title".into(),
            text: String::new(),
        }];
        let saved = decode(&encode(
            Source::Ask,
            Some("2026-09-11 08:30"),
            Some(1_789_115_400),
            &stories,
        ));
        assert_eq!(saved.stories, stories);
        assert_eq!(saved.source, Source::Ask);
        assert_eq!(saved.fetched.as_deref(), Some("2026-09-11 08:30"));
        assert_eq!(saved.fetched_at, Some(1_789_115_400));
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
            decode(&encode(Source::Top, None, None, &[story]))
                .stories
                .len(),
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
            TaskOutcome::Completed(
                b"[8863,8864,8865,8866,8867,8868,8869,8870,8871,8872,8873,8874,8875]".to_vec(),
            ),
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
        assert_eq!(brief.note, None, "the failure was said twice");
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
                    id: (8863 + index).to_string(),
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

    /// A linked story opened before the keeper got to it is fetched once,
    /// cut to the article, and kept as it opens.
    #[test]
    fn a_linked_story_opened_before_it_was_kept_is_kept_as_it_opens() {
        let mut context = Context::default();
        let mut brief = ready();
        brief.stories = vec![Story {
            id: "8863".into(),
            title: "A morning by the river".into(),
            site: "example.org".into(),
            url: "https://example.org/river".into(),
            text: String::new(),
        }];
        brief.on_action(&mut context, action_id("story-0"));
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
            TaskOutcome::Completed(
                b"<nav>Menu</nav><article><h1>A morning</h1><p>The first light.</p></article>"
                    .to_vec(),
            ),
        );
        assert_eq!(brief.view, View::Reading);
        assert!(brief.keeper.is_keeping("8863"), "the story was not kept");
        let drawn = format!("{:?}", brief.book.screen("t").expect("the story is open"));
        assert!(drawn.contains("The first light."), "{drawn}");
        assert!(!drawn.contains("Menu"), "{drawn}");
    }

    /// A story with nothing but its own text is read without asking the
    /// network for anything.
    #[test]
    fn a_question_is_read_from_what_the_poster_wrote() {
        let mut context = Context::default();
        let mut brief = ready();
        brief.stories = vec![Story {
            id: "8864".into(),
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

    /// A reader's storage, answering every request the way a healthy device
    /// does, so a test can follow a story from the morning's refresh to the
    /// train.
    #[derive(Default)]
    struct Device {
        values: BTreeMap<String, Vec<u8>>,
        shelf: BTreeMap<String, Vec<u8>>,
        writing: BTreeMap<String, Vec<u8>>,
    }

    impl Device {
        /// Answers every store request in `commands`, and every request those
        /// answers lead to, in the order the application made them.
        fn settle(
            &mut self,
            runner: &mut AppRunner<Brief>,
            commands: Vec<Command>,
        ) -> Vec<Command> {
            let mut seen = Vec::new();
            let mut queue = std::collections::VecDeque::new();
            for command in commands {
                if let Command::Store(request) = &command {
                    queue.push_back(request.clone());
                }
                seen.push(command);
            }
            while let Some(request) = queue.pop_front() {
                let answer = self.answer(request);
                for command in runner.store_result(answer) {
                    if let Command::Store(request) = &command {
                        queue.push_back(request.clone());
                    }
                    seen.push(command);
                }
            }
            seen
        }

        fn answer(&mut self, request: StoreRequest) -> StoreResult {
            match request {
                StoreRequest::Save { key, value } => {
                    self.values.insert(key.clone(), value);
                    StoreResult::Saved { key }
                }
                StoreRequest::Load { key } => StoreResult::Loaded {
                    value: self.values.get(&key).cloned(),
                    key,
                },
                StoreRequest::Forget { key } => {
                    self.values.remove(&key);
                    StoreResult::Forgotten { key }
                }
                StoreRequest::List => StoreResult::Keys(self.values.keys().cloned().collect()),
                StoreRequest::ShelfWrite {
                    name,
                    offset,
                    bytes,
                    last,
                } => {
                    let blob = self.writing.entry(name.clone()).or_default();
                    blob.truncate(offset as usize);
                    blob.extend_from_slice(&bytes);
                    let size = u32::try_from(blob.len()).unwrap();
                    if last {
                        let whole = self.writing.remove(&name).unwrap_or_default();
                        self.shelf.insert(name.clone(), whole);
                    }
                    StoreResult::ShelfWritten { name, size }
                }
                StoreRequest::ShelfRead {
                    name,
                    offset,
                    length,
                } => match self.shelf.get(&name) {
                    Some(blob) => {
                        let from = (offset as usize).min(blob.len());
                        let to = (from + length as usize).min(blob.len());
                        StoreResult::ShelfRead {
                            bytes: blob[from..to].to_vec(),
                            size: u32::try_from(blob.len()).unwrap(),
                            name,
                            offset,
                        }
                    }
                    None => StoreResult::Denied(StoreError::Missing),
                },
                StoreRequest::ShelfRemove { name } => {
                    self.shelf.remove(&name);
                    StoreResult::ShelfRemoved { name }
                }
                StoreRequest::ShelfList => StoreResult::Shelf(
                    self.shelf
                        .iter()
                        .map(|(name, blob)| (name.clone(), u32::try_from(blob.len()).unwrap()))
                        .collect(),
                ),
            }
        }
    }

    /// The fetch the application has in flight for `url`.
    fn fetch_for(commands: &[Command], url: &str) -> Option<TaskId> {
        commands.iter().find_map(|command| match command {
            Command::Spawn {
                task,
                work: Task::Fetch { url: asked, .. },
            } if asked == url => Some(*task),
            _ => None,
        })
    }

    /// A brief of one linked story, refreshed on `device` with every store
    /// request answered and the story's page fetched for keeping.
    fn refreshed_on(device: &mut Device) -> AppRunner<Brief> {
        let mut runner = AppRunner::new(Brief::default());
        let commands = runner.start();
        device.settle(&mut runner, commands);
        let commands = runner.action(action_id(REFRESH));
        let index = fetch_for(&commands, &Source::Top.index()).expect("the index was asked for");
        let commands = runner.task_outcome(index, TaskOutcome::Completed(b"[8863]".to_vec()));
        let story = fetch_for(&commands, &format!("{}/v0/item/8863.json", super::origin()))
            .expect("the story was asked for");
        let commands = runner.task_outcome(
            story,
            TaskOutcome::Completed(
                br#"{"id":8863,"title":"A morning by the river","url":"https://example.org/river"}"#
                    .to_vec(),
            ),
        );
        let commands = device.settle(&mut runner, commands);
        let page = fetch_for(&commands, "https://example.org/river")
            .expect("the story behind the headline was fetched to keep");
        let commands = runner.task_outcome(
            page,
            TaskOutcome::Completed(
                b"<nav>Menu</nav><article><h1>A morning</h1><p>The first light.</p></article>"
                    .to_vec(),
            ),
        );
        device.settle(&mut runner, commands);
        runner
    }

    #[test]
    fn a_refresh_keeps_every_linked_story_on_the_device() {
        let mut device = Device::default();
        let runner = refreshed_on(&mut device);
        assert!(runner.app().keeper.is_kept("8863"));
        assert!(runner
            .app()
            .offline_line()
            .starts_with("Ready offline: 1 story, "));
        let drawn = format!("{:?}", runner.app().screen(&runner.context()));
        assert!(drawn.contains("Ready offline: 1 story"), "{drawn}");
    }

    #[test]
    fn a_kept_story_opens_with_the_radio_off() {
        let mut device = Device::default();
        let mut runner = refreshed_on(&mut device);
        let commands = runner.action(action_id("story-0"));
        let commands = device.settle(&mut runner, commands);
        assert!(
            fetch_for(&commands, "https://example.org/river").is_none(),
            "a kept story went to the network"
        );
        assert_eq!(runner.app().view, View::Reading);
        let drawn = format!("{:?}", runner.app().book.screen("t").unwrap());
        assert!(drawn.contains("The first light."), "{drawn}");
    }

    #[test]
    fn a_story_that_leaves_the_brief_leaves_the_device() {
        let mut device = Device::default();
        let mut runner = refreshed_on(&mut device);
        assert_eq!(device.shelf.len(), 1);
        runner.app_mut().stories[0].id = "9999".into();
        let mut context = Context::default();
        runner.app_mut().keep_stories(&mut context);
        let removed = context.take_commands().iter().any(|command| {
            matches!(
                command,
                Command::Store(StoreRequest::ShelfRemove { name }) if device.shelf.contains_key(name)
            )
        });
        assert!(removed, "yesterday's story stayed on the device");
    }

    #[test]
    fn a_brief_older_than_six_hours_refreshes_itself_and_a_fresh_one_does_not() {
        let mut brief = ready();
        assert!(
            !brief.is_stale(Some(1_000_000)),
            "a brief nobody has fetched yet was fetched without being asked"
        );
        brief.stories = vec![Story {
            title: "yesterday".into(),
            ..Story::default()
        }];
        brief.fetched = Some("2026-09-10 07:00".into());
        brief.fetched_at = Some(1_000_000);
        assert!(!brief.is_stale(Some(1_000_000 + 60 * 60)));
        assert!(brief.is_stale(Some(1_000_000 + 7 * 60 * 60)));
        brief.fetched_at = None;
        assert!(brief.is_stale(Some(1_000_000)), "an undated brief is stale");
    }

    #[test]
    fn opening_the_application_on_a_stale_brief_fetches_a_new_one() {
        let mut device = Device::default();
        device.values.insert(
            STORED.into(),
            encode(
                Source::Top,
                Some("2020-01-01 07:00"),
                Some(1_577_862_000),
                &[Story {
                    id: "1".into(),
                    title: "Old news".into(),
                    site: "example.org".into(),
                    ..Story::default()
                }],
            ),
        );
        let mut runner = AppRunner::new(Brief::default());
        let commands = runner.start();
        let commands = device.settle(&mut runner, commands);
        assert!(
            fetch_for(&commands, &Source::Top.index()).is_some(),
            "a brief from years ago was left as it was"
        );
        assert!(
            commands.iter().any(|command| matches!(
                command,
                Command::Device(kobo_sdk::DeviceRequest::ScheduleWake { .. })
            )),
            "no morning wake was asked for"
        );
    }

    #[test]
    fn the_morning_wake_fetches_a_new_brief() {
        let mut device = Device::default();
        let mut runner = refreshed_on(&mut device);
        let commands = runner.scheduled_wake();
        assert!(fetch_for(&commands, &Source::Top.index()).is_some());
    }

    #[test]
    fn the_morning_wake_is_asked_for_six_in_the_morning_on_the_readers_clock() {
        let snapshot = |hour: u64, offset: i16| kobo_sdk::clock::Snapshot {
            unix_millis: (20_000 * 86_400 + hour * 3600)
                .saturating_add_signed(-i64::from(offset) * 60)
                * 1000,
            monotonic_millis: 0,
            utc_offset_minutes: offset,
        };
        assert_eq!(
            super::until_morning(snapshot(5, 600)),
            std::time::Duration::from_secs(3600)
        );
        assert_eq!(
            super::until_morning(snapshot(22, -300)),
            std::time::Duration::from_secs(8 * 3600)
        );
    }

    #[test]
    fn the_time_a_brief_was_fetched_is_written_on_the_readers_clock() {
        let evening_in_sydney = kobo_sdk::clock::Snapshot {
            unix_millis: (20_000 * 86_400 + 10 * 3600) * 1000,
            monotonic_millis: 0,
            utc_offset_minutes: 600,
        };
        let stamped = super::timestamp(evening_in_sydney).unwrap();
        assert!(stamped.ends_with("20:00"), "{stamped}");
        assert!(!stamped.contains("UTC"), "{stamped}");
    }

    #[test]
    fn choosing_another_source_mid_refresh_does_not_put_the_old_list_under_the_new_name() {
        let mut context = Context::default();
        let (mut brief, task, _started) = refreshing(&mut context);
        brief.on_action(&mut context, action_id(Source::Ask.action()));
        assert!(context
            .take_commands()
            .iter()
            .any(|command| matches!(command, Command::Cancel(cancelled) if *cancelled == task)));
        brief.on_task(
            &mut context,
            task,
            TaskOutcome::Completed(b"[8863]".to_vec()),
        );
        assert_eq!(brief.fetching, Fetching::Nothing);
        assert!(brief.stories.is_empty());
    }

    #[test]
    fn copies_kept_by_earlier_versions_are_removed_at_start() {
        let old = kobo_bookview::positions::identity("brief-article:https://example.org/");
        let mut device = Device::default();
        device.values.insert(old.clone(), b"0:abc".to_vec());
        device
            .shelf
            .insert(format!("{}.0", &old[..60]), b"<p>old</p>".to_vec());
        let mut runner = AppRunner::new(Brief::default());
        let commands = runner.start();
        device.settle(&mut runner, commands);
        assert!(!device.values.contains_key(&old));
        assert!(device.shelf.is_empty());
    }
}

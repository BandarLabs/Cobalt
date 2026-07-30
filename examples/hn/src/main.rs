//! Hacker News, on a panel with no scrollbar and no keyboard.
//!
//! Four tabs along the bottom (Top, New, Ask, Show) and a comment thread
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
//! The transport carries half a megabyte (`MAX_TASK_BYTES_U32` in
//! `kobo-protocol`, well under the 1 MiB `MAX_FRAME_LEN` that carries it) and
//! a busy thread is comfortably more. A real one measured while writing this
//! was 734 KB for 925 comments. Algolia ignores `Range`, so the trick that
//! lets Gutenbird read a novel in pieces does not work here: asking for the
//! second half returns the whole document again and the ceiling rejects it.
//!
//! So the request comes back `TaskError::TooLarge`, and rather than showing
//! a dead end this asks a different question, `search_by_date` over that
//! story's comments, thirty at a time, which is bounded by construction. The
//! nesting is gone in that answer, so the screen *says* the nesting is gone.
//! What never happens is a thread that silently stops halfway, or one that
//! reads as complete when a third of it is missing.

mod model;

use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Failure, Glyph, KoboApp, QuoteRole, Screen,
    ScreenBuilder, Task, TaskId, TaskOutcome,
};
use model::{Comment, Story};
use std::collections::HashSet;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

/// Hacker News' own API, which is the authority on what is on the site.
///
/// Everything a reader sees comes from here. It answers one item per request,
/// which is more round trips than an index does, and it is worth every one of
/// them: it is the site's own record, so a story submitted a minute ago is in
/// the list, every score is the score on the page, and `kids` is the order the
/// site draws replies in. That ordering cannot be recomputed by a client, and
/// it is the reason this is not a search index reading.
const HN_API: &str = "https://hacker-news.firebaseio.com/v0";

/// How many item fetches run at once.
///
/// One below the runtime's ceiling of four on purpose, so a thread filling in
/// can never leave the list with nowhere to go.
const LANES: usize = 3;

const _: () = assert!(LANES < kobo_sdk::MAX_TASKS_IN_FLIGHT);

/// How many comments are fetched before the panel is repainted.
///
/// The panel takes most of a second to redraw and flashes while it does, so a
/// repaint per comment would be unusable. A run of this many is a few screens
/// of reading and a handful of seconds on the radio.
const CHUNK: usize = 24;

/// How much of one item to accept.
///
/// A comment is a paragraph or two of text plus a list of reply numbers. The
/// longest ones on the site are a few kilobytes; this is generous headroom.
const ITEM_BYTES: u32 = 64 * 1024;

/// One item, by number, from Hacker News itself.
fn item_url(id: i64) -> String {
    format!("{HN_API}/item/{id}.json")
}

/// How many stories a tab asks for.
///
/// One screenful is six or seven rows, so this is four or five page turns
/// deep. More would be a longer wait on the radio for pages nobody reaches.
const HITS: u32 = 30;

/// How much of a ranking to accept.
///
/// Thirty item numbers, sliced server-side. The unsliced list is five hundred
/// of them; this ceiling is generous against the slice and would refuse the
/// whole list, which is the point.
const RANKING_BYTES: u32 = 8 * 1024;

/// How many lines of a list row a headline may take.
///
/// Measured against a real front page: at one line, three of thirty headlines
/// ended in an ellipsis; at two lines, none did. So the cost is three rows in
/// thirty standing a line taller than their neighbours, and the gain is three
/// headlines that say what they are about. Pagination measures every row
/// individually, so a ragged column costs nothing but the raggedness.
const TITLE_LINES: usize = 2;

/// How many placeholder rows stand in for a list while it is arriving.
const SKELETON_ROWS: u8 = 6;

/// The tag on the paragraphs that stand in for the story's facts.
///
/// The thread is measured as prose so the runtime's own wrapping decides where
/// each page ends, but the facts at the top of the first page are a labelled
/// block, not prose. So a paragraph is set aside for each fact -- measured, and
/// so counted against the page -- and swapped for the real `facts` block when
/// the page is drawn. `u32::MAX` is the tag because comment tags are one-based
/// indices into a list capped far below it, so this can never be one of them.
const FACT_TAG: u32 = u32::MAX;

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

    /// Hacker News' own list for this tab: item numbers, in the site's order.
    ///
    /// This is the whole reason the application talks to two services. These
    /// four endpoints are the pages themselves (`topstories` *is* the front
    /// page, `askstories` *is* Ask HN) so there is no ranking to approximate
    /// and no recency window to guess at. The previous version asked Algolia's
    /// ranked search instead, and Algolia ranks over all of Hacker News
    /// forever, so Ask HN opened on a question from 2013.
    ///
    /// Sliced server-side with Firebase's own query parameters: the unsliced
    /// answer is five hundred numbers and 4.5 KB, the slice is thirty and
    /// under three hundred bytes, and the radio is the expensive part of this
    /// device.
    fn ranking_url(self) -> String {
        let list = match self {
            Self::Top => "topstories",
            Self::New => "newstories",
            Self::Ask => "askstories",
            Self::Show => "showstories",
        };
        format!("{HN_API}/{list}.json?orderBy=%22%24key%22&limitToFirst={HITS}")
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
/// Only ever one of these at a time. A tab tapped twice while the first answer
/// is in the air would otherwise land two lists on the panel in an order
/// decided by the network. Individual items are not in here: they run several
/// at a time in their own lanes and are matched by the item number they carry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Awaiting {
    /// Which stories this tab holds, and in what order.
    Ranking(Tab),
}

/// One place in a thread, whether or not its comment has arrived yet.
///
/// The thread is held as a flat list in the order the site draws it, with a
/// slot for every reply that is known to exist. A slot with nothing in it is a
/// comment that has been named by its parent and not yet fetched; it takes up
/// no room on the panel, and when it lands it appears exactly where it belongs
/// rather than at the end.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Slot {
    id: i64,
    depth: u16,
    comment: Option<Comment>,
}

#[derive(Default)]
struct Hn {
    tab: Tab,
    view: View,
    stories: Vec<Story>,
    /// The item numbers Hacker News gave for the showing tab, in its order.
    ranking: Vec<i64>,
    /// Which story rows belong on each page, measured against this panel.
    pages: Vec<Vec<usize>>,
    page: usize,
    /// The open story, as an index into `stories`.
    open: Option<usize>,
    comments: Vec<Comment>,
    /// The thread broken into pages of paragraphs that fit this panel. Each
    /// paragraph carries the comment it came from, one-based, so a byline on a
    /// page can be folded without having to be found again.
    thread_pages: Vec<Vec<(u32, u8, QuoteRole, String)>>,
    /// Which comments are folded shut, by index into `comments`.
    collapsed: HashSet<usize>,
    /// Each story's title cut to the one line a row can show, measured against
    /// this panel rather than guessed at by character count.
    titles: Vec<String>,
    thread_page: usize,
    /// Every reply the site says exists, in the site's own order.
    ///
    /// Held alongside `comments` rather than instead of it: `comments` is what
    /// is drawable now, and this is the shape of the whole conversation
    /// including the parts still on their way.
    slots: Vec<Slot>,
    /// Item fetches running at once, each with the item number it will answer.
    ///
    /// Matched by number rather than by position, because a comment landing
    /// splices its own replies into the middle of the list and every position
    /// after it moves.
    lanes: Vec<(TaskId, i64)>,
    /// How many comments are worth having on hand before the panel repaints.
    ///
    /// Grown when the reader reaches the end of what has arrived. Fetching one
    /// comment per request is what buys exact ordering, and repainting on each
    /// arrival would be one full refresh per comment; the panel is left alone
    /// until a whole chunk has landed.
    want: usize,
    /// Story details still to fetch, and the lanes fetching them.
    ///
    /// The tab's ranking arrives as bare item numbers, so each row's title and
    /// score is a request of its own. The first screenful is fetched before
    /// anything is drawn and the rest follows behind it.
    story_lanes: Vec<(TaskId, i64)>,
    /// What the screen has to admit about what it is showing.
    note: Option<String>,
    /// The device clock at the last answer, for relative ages.
    now: i64,
    task: Option<(TaskId, Awaiting)>,
    problem: Option<String>,
    /// The last task failure, kept as the SDK read it rather than as a
    /// sentence, because an empty list wants the whole-screen version of the
    /// same thing and a list with rows on it wants the banner.
    trouble: Option<Failure>,
}

impl Hn {
    fn show(&self, context: &mut Context) {
        let screen = match self.view {
            View::List => self.list(),
            View::Thread => self.thread(),
        };
        // A thread was reached from the list, so Back belongs to the list
        // first and to the launcher second. Claimed only when a thread is
        // genuinely open, so the fallback back to the list does not cost the
        // reader a tap that redraws what they are already looking at.
        let owns_back = self.view == View::Thread && self.open.is_some();
        context.set_screen(screen.with_own_back(owns_back));
    }

    /// The story list for the tab that is showing.
    fn list(&self) -> Screen {
        let mut screen = ScreenBuilder::new("hn").top_bar(self.list_title());
        if let Some(problem) = &self.problem {
            screen = screen.banner(BannerLevel::Attention, problem.clone());
        }
        if matches!(self.task, Some((_, Awaiting::Ranking(_))))
            || self.stories.is_empty() && !self.story_lanes.is_empty()
        {
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
            // A failure with an empty list is the whole screen, not a banner
            // over nothing. `standard_state` centres it and names it the same
            // way every other application does.
            if let Some(failure) = self.trouble {
                let screen = ScreenBuilder::new("hn")
                    .top_bar(self.list_title())
                    .failure_state(failure, "retry");
                return self.with_tabs(screen).build();
            }
            return self
                .with_tabs(
                    screen
                        .text(
                            "Nothing came back for this tab. That is usually the network \
                             rather than Hacker News.",
                        )
                        .primary_button("retry", "Try again"),
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
                // The score, in a column of its own at the trailing edge. It
                // was the fourth run-on clause of the summary line, which is
                // the last place an eye scanning for the popular story would
                // find it.
                model::score(story),
            ))
        });
        // Tapping the side of the panel turns the page, which is how every
        // Kobo has always worked. The bottom bar is spent on the tabs (those
        // are places, and places outrank controls for that bar) so the visible
        // page control is the one action the top bar allows.
        // No page position under the list. Which page of how many is already
        // in the top bar, next to the controls that change it, and a caption
        // repeating it at the foot of the panel was a second answer to a
        // question that had been answered once.
        let turning = screen
            .rows_with_trailing(rows)
            .page_turns("list-back", "list-next");
        self.with_tabs(turning).build()
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
        // Each direction is offered only where there is a page on that side.
        // The bar used to say Next on every page including the last, where it
        // promised stories that did not exist, and never said Previous at all,
        // so the only way back was a side tap nothing on the panel mentions.
        //
        // Chevrons rather than words: this is the one pair of controls whose
        // picture every reader already knows, and two words in a bar that also
        // has to hold the title is most of the title gone. The labels are
        // still set, because they are what the control is called everywhere
        // that is not the panel.
        let screen = if self.page > 0 {
            screen.top_bar_glyph("list-back", "Previous", Glyph::Previous)
        } else {
            screen
        };
        let screen = if self.page + 1 < self.pages.len() {
            screen.top_bar_glyph("list-next", "Next", Glyph::Next)
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
        if !self.lanes.is_empty() && self.comments.is_empty() {
            // The same bar as the loaded thread, rather than a smaller one
            // that grows when the comments land: a control that moves out from
            // under a finger on a panel this slow is a tap the reader watches
            // miss. A bar of one destination is also refused by the wire,
            // which is how this was found.
            return screen
                .activity("Fetching the comments", None)
                .skeleton(SKELETON_ROWS)
                .action_bar(THREAD_BAR)
                .build();
        }
        // A byline is only made foldable once, on the page where its comment
        // begins. The copy repeated at the top of a continuation is a
        // reminder of who is speaking, and hanging a control off it would put
        // two plus signs for the same comment in front of the reader.
        let mut folded_here: Option<u32> = None;
        let mut facts_drawn = false;
        for (tag, depth, role, paragraph) in self
            .thread_pages
            .get(self.thread_page)
            .into_iter()
            .flatten()
        {
            if *tag == FACT_TAG {
                // The labelled block, drawn once where its first reserved
                // paragraph fell; the rest of the run is the room it stands in.
                if !facts_drawn {
                    facts_drawn = true;
                    screen = screen.facts(story_facts(story, self.now));
                }
                continue;
            }
            let index = (*tag as usize).checked_sub(1);
            match (*role, index) {
                (QuoteRole::Byline, Some(index)) if folded_here != Some(*tag) => {
                    folded_here = Some(*tag);
                    let collapsed = self.collapsed.contains(&index);
                    let replies = self.replies_to(index);
                    screen = folding_text(
                        screen,
                        *depth,
                        paragraph.clone(),
                        &format!("fold-{index}"),
                        collapsed,
                        u16::try_from(replies).unwrap_or(u16::MAX),
                    );
                }
                _ => screen = screen.quote_as(*depth, *role, paragraph.clone()),
            }
        }
        if let Some(problem) = &self.problem {
            screen = screen.banner(BannerLevel::Attention, problem.clone());
        }
        // Pinned rather than at the end of the flow: content stops at the bar,
        // so a page that runs long loses its last sentence instead of the only
        // way off the page.
        screen
            .page_turns("thread-back", "thread-next")
            .action_bar(THREAD_BAR)
            .build()
    }

    /// Everything the thread screen draws, as paragraphs carrying their depth.
    ///
    /// Built as prose rather than as nodes so that the runtime's own wrapping
    /// and line height decide where the folds are. Anything drawn outside this
    /// (a banner, say) is not measured, and on this panel what is not measured
    /// is what silently falls off the bottom.
    ///
    /// Depth travels with each paragraph because an indented paragraph has a
    /// narrower measure: a thread paginated flat and then drawn indented would
    /// lose the bottom of nearly every page.
    fn thread_paragraphs(&self) -> Vec<(u32, u8, QuoteRole, String)> {
        let Some(story) = self.open.and_then(|index| self.stories.get(index)) else {
            return Vec::new();
        };
        let mut paragraphs = vec![(0, 0, QuoteRole::Body, story.title.clone())];
        // One paragraph per fact, tagged so the draw pass swaps the run for a
        // real facts block. They are here, in the measured flow, rather than
        // hung off the top of the screen as chrome, because chrome the
        // paginator never sees is chrome that pushes the last line of the page
        // off the panel. A byline of the value is a hair taller than the fact
        // row that replaces it, so the block always fits the room reserved.
        for (_, value) in story_facts(story, self.now) {
            paragraphs.push((FACT_TAG, 0, QuoteRole::Byline, value));
        }
        if let Some(note) = &self.note {
            // Inside the flow, not in a banner. A banner is chrome the
            // paginator never measured, so it would push the last paragraph of
            // every page off the panel.
            paragraphs.push((0, 0, QuoteRole::Body, note.clone()));
        }
        if let Some(body) = &story.text {
            paragraphs.push((0, 0, QuoteRole::Body, body.clone()));
        }
        if self.comments.is_empty() {
            paragraphs.push((0, 0, QuoteRole::Body, "No comments yet.".to_owned()));
            return paragraphs;
        }
        let mut index = 0;
        while index < self.comments.len() {
            let comment = &self.comments[index];
            let indent = comment.indent();
            // One-based, because zero is the tag everything that is not a
            // comment carries. A thread this long cannot be fetched -- the
            // ceiling is `model::MAX_COMMENTS` -- so the conversion cannot
            // narrow, and saturating is still better than wrapping onto
            // somebody else's comment.
            let tag = u32::try_from(index + 1).unwrap_or(u32::MAX);
            paragraphs.push((tag, indent, QuoteRole::Byline, comment.byline(self.now)));
            if self.collapsed.contains(&index) {
                // The comment's own words go too. Folding a comment that left
                // its text on the page would only hide the replies, and the
                // reader who tapped it was hiding the whole thing.
                index += self.replies_to(index) + 1;
                continue;
            }
            for body in comment.body.split("\n\n") {
                if !body.trim().is_empty() {
                    paragraphs.push((tag, indent, QuoteRole::Body, body.to_owned()));
                }
            }
            index += 1;
        }
        paragraphs
    }

    /// How many comments are underneath the one at `index`.
    ///
    /// The list is in pre-order, so a comment's replies are exactly the run
    /// that follows it while the depth stays greater than its own. Counted on
    /// the real `depth` rather than on `indent`, which is clamped: past the
    /// indent cap every reply is drawn at the same offset, and counting on
    /// that would fold a comment's siblings away with it.
    fn replies_to(&self, index: usize) -> usize {
        let Some(parent) = self.comments.get(index) else {
            return 0;
        };
        self.comments[index + 1..]
            .iter()
            .take_while(|comment| comment.depth > parent.depth)
            .count()
    }

    /// Drops the ranking request that is already on its way.
    ///
    /// Only one of these is ever outstanding. A reader who taps three tabs
    /// while the first is in the air is asking for the third one, and letting
    /// all three run would land three lists on the panel in an order the
    /// network chose, each one a full refresh the reader watches happen.
    fn cancel_outstanding(&mut self, context: &mut Context) {
        if let Some((task, _)) = self.task.take() {
            context.cancel(task);
        }
    }

    /// Asks Hacker News which stories this tab holds. The substance follows.
    fn ask_list(&mut self, context: &mut Context) {
        self.cancel_outstanding(context);
        self.drop_lanes(context);
        self.problem = None;
        self.trouble = None;
        self.ranking.clear();
        self.stories.clear();
        self.pages.clear();
        match context.spawn_retrying(Task::Fetch {
            url: self.tab.ranking_url(),
            offset: 0,
            max_bytes: RANKING_BYTES,
        }) {
            Some(task) => self.task = Some((task, Awaiting::Ranking(self.tab))),
            None => self.problem = Some("Too much is already in flight.".to_owned()),
        }
    }

    /// Starts as many story fetches as there are free lanes.
    ///
    /// One request per story, because that is the only way to see what the site
    /// sees. The search index that answered thirty at once is minutes behind,
    /// which on a front page that turns over in minutes is a different front
    /// page: a story submitted five minutes ago was simply missing, and every
    /// score was slightly wrong.
    fn pump_stories(&mut self, context: &mut Context) {
        while self.story_lanes.len() < LANES {
            let Some(id) = self.next_story() else { break };
            let Some(task) = context.spawn_retrying(Task::Fetch {
                url: item_url(id),
                offset: 0,
                max_bytes: ITEM_BYTES,
            }) else {
                break;
            };
            self.story_lanes.push((task, id));
        }
    }

    /// The next story number with neither an answer nor a lane on it.
    fn next_story(&self) -> Option<i64> {
        self.ranking
            .iter()
            .copied()
            .take(HITS as usize)
            .find(|id| {
                !self.story_lanes.iter().any(|(_, wanted)| wanted == id)
                    && !self.stories.iter().any(|story| story.id == id.to_string())
            })
            .filter(|_| self.stories.len() < model::MAX_STORIES)
    }

    /// Asks for the story item itself, which names its replies in site order.
    fn ask_thread(&mut self, context: &mut Context) {
        let Some(id) = self.open_id().and_then(|id| id.parse::<i64>().ok()) else {
            self.problem = Some("That story has no thread to open.".to_owned());
            return;
        };
        self.cancel_outstanding(context);
        self.drop_lanes(context);
        self.problem = None;
        self.trouble = None;
        self.want = CHUNK;
        match context.spawn_retrying(Task::Fetch {
            url: item_url(id),
            offset: 0,
            max_bytes: ITEM_BYTES,
        }) {
            Some(task) => self.lanes.push((task, id)),
            None => self.problem = Some("Too much is already in flight.".to_owned()),
        }
    }

    /// Starts as many comment fetches as there are free lanes and appetite.
    fn pump_thread(&mut self, context: &mut Context) {
        while self.lanes.len() < LANES
            && self.loaded() + self.lanes.len() < self.want
            && self.loaded() < model::MAX_COMMENTS
        {
            let Some(id) = self.next_slot() else { break };
            let Some(task) = context.spawn_retrying(Task::Fetch {
                url: item_url(id),
                offset: 0,
                max_bytes: ITEM_BYTES,
            }) else {
                break;
            };
            self.lanes.push((task, id));
        }
    }

    /// The next empty slot with no lane already on it, in reading order.
    fn next_slot(&self) -> Option<i64> {
        self.slots
            .iter()
            .find(|slot| slot.comment.is_none() && !self.lanes.iter().any(|(_, id)| *id == slot.id))
            .map(|slot| slot.id)
    }

    /// How many comments have actually arrived.
    fn loaded(&self) -> usize {
        self.slots
            .iter()
            .filter(|slot| slot.comment.is_some())
            .count()
    }

    /// Drops every item fetch on the floor.
    fn drop_lanes(&mut self, context: &mut Context) {
        for (task, _) in self.lanes.drain(..).chain(self.story_lanes.drain(..)) {
            context.cancel(task);
        }
    }

    /// Places one comment in its slot and opens slots for its own replies.
    fn place(&mut self, item: &model::Item) {
        let Some(at) = self.slots.iter().position(|slot| slot.id == item.id) else {
            return;
        };
        let depth = self.slots[at].depth;
        self.slots[at].comment = item.comment(depth);
        if self.slots[at].comment.is_none() {
            // Nothing to draw and nothing underneath it. The slot goes rather
            // than sitting there forever as a hole the fetcher keeps skipping.
            self.slots.remove(at);
            return;
        }
        let room = model::MAX_COMMENTS.saturating_sub(self.slots.len());
        let kids = item.kids.iter().take(room).enumerate().map(|(step, id)| {
            (
                step,
                Slot {
                    id: *id,
                    depth: depth.saturating_add(1),
                    comment: None,
                },
            )
        });
        for (step, slot) in kids.collect::<Vec<_>>() {
            self.slots.insert(at + 1 + step, slot);
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
        self.slots.clear();
        // Folds are positions in the list that is being thrown away. Left
        // behind, they would shut whichever comments of the new story happened
        // to land on the same indices.
        self.collapsed.clear();
        self.thread_pages.clear();
        self.thread_page = 0;
        self.note = None;
        self.problem = None;
        self.trouble = None;
        self.ask_thread(context);
        self.show(context);
    }

    /// Takes the tab's item numbers and goes after what they say.
    fn took_ranking(&mut self, context: &mut Context, bytes: &[u8], tab: Tab) {
        let numbers: Option<Vec<i64>> = std::str::from_utf8(bytes)
            .ok()
            .and_then(|body| kobo_json::parse(body).ok())
            .and_then(|value| {
                value
                    .as_array()
                    .map(|items| items.iter().filter_map(kobo_json::Value::as_i64).collect())
            });
        self.tab = tab;
        let Some(numbers) = numbers else {
            self.stories.clear();
            self.pages.clear();
            self.problem = Some("Hacker News' list of stories could not be read.".to_owned());
            return;
        };
        self.ranking = numbers;
        if self.ranking.is_empty() {
            self.stories.clear();
            self.pages.clear();
            self.problem = Some("That tab came back empty.".to_owned());
            return;
        }
        self.now = unix_now();
        self.pump_stories(context);
    }

    /// Places one story in the tab's own order and asks for the next.
    ///
    /// Sorted by the ranking rather than by arrival, because the lanes finish
    /// out of order and the order is the whole point: `topstories` *is* the
    /// front page.
    fn took_story(&mut self, context: &mut Context, bytes: &[u8], id: i64) {
        let taken = std::str::from_utf8(bytes)
            .ok()
            .and_then(|body| kobo_json::parse(body).ok())
            .as_ref()
            .and_then(model::item_from)
            .as_ref()
            .and_then(model::Item::story);
        match taken {
            Some(story) => {
                self.stories.push(story);
                let ranking = std::mem::take(&mut self.ranking);
                self.stories.sort_by_key(|story| {
                    story
                        .id
                        .parse::<i64>()
                        .ok()
                        .and_then(|id| ranking.iter().position(|listed| *listed == id))
                        .unwrap_or(usize::MAX)
                });
                self.ranking = ranking;
            }
            // A number the site no longer answers for, or an item a list
            // cannot show. It leaves the ranking, because otherwise the lane
            // that was on it picks it straight back up and asks forever.
            None => self.ranking.retain(|listed| *listed != id),
        }
        self.pump_stories(context);
    }

    /// Places one comment, opens slots for its replies, and asks for the next.
    fn took_item(&mut self, context: &mut Context, bytes: &[u8], id: i64) {
        let item = std::str::from_utf8(bytes)
            .ok()
            .and_then(|body| kobo_json::parse(body).ok())
            .as_ref()
            .and_then(model::item_from);
        match item {
            Some(item) if item.id == id => {
                if self.slots.is_empty() {
                    // The story itself, which is what names the top-level
                    // replies. Its own text is drawn from the list row.
                    self.slots = item
                        .kids
                        .iter()
                        .take(model::MAX_COMMENTS)
                        .map(|id| Slot {
                            id: *id,
                            depth: 0,
                            comment: None,
                        })
                        .collect();
                } else {
                    self.place(&item);
                }
            }
            // An item number the site no longer answers for. The slot goes, so
            // the fetcher moves on instead of asking for it again forever.
            _ => self.slots.retain(|slot| slot.id != id),
        }
        self.pump_thread(context);
    }

    /// Measures the rows against the panel to find where the folds are.
    fn repaginate_list(&mut self, context: &Context) {
        self.titles = self
            .stories
            .iter()
            .map(|story| {
                context.clamped_row_beside(&story.title, &model::score(story), TITLE_LINES, true)
            })
            .collect();
        let summaries = self
            .stories
            .iter()
            .map(|story| model::summary(story, self.now))
            .collect::<Vec<_>>();
        // Measured with the score, because the score keeps a column at the
        // trailing edge and the title and summary wrap inside what is left.
        // Paginated as though the rows were full width, the last one on every
        // page was drawn under the tab bar and clipped away.
        let scores = self.stories.iter().map(model::score).collect::<Vec<_>>();
        let rows = self
            .titles
            .iter()
            .zip(&summaries)
            .zip(&scores)
            .map(|((title, summary), score)| (title.as_str(), summary.as_str(), score.as_str()))
            .collect::<Vec<_>>();
        self.pages = context.paginate_rows_with_trailing(&rows, true);
        self.page = self.page.min(self.pages.len().saturating_sub(1));
    }

    /// Rebuilds the drawable thread from whatever has arrived.
    ///
    /// The slots are already in the site's order, so this is a filter rather
    /// than a sort. A slot still on its way contributes nothing and takes up
    /// no room, which is what lets the top of a conversation be read while the
    /// bottom of it is still being fetched.
    fn rebuild_thread(&mut self, context: &Context) {
        self.comments = self
            .slots
            .iter()
            .filter_map(|slot| slot.comment.clone())
            .collect();
        self.repaginate_thread(context);
    }

    /// Hides or shows the replies under the comment at `index`.
    ///
    /// The reader is kept where they tapped. Folding a long subtree away
    /// shortens the thread by pages, so holding the page *number* would leave
    /// them somewhere they never asked to be -- often past the end. Holding
    /// the *comment* instead means the byline they just touched is still under
    /// their finger, with whatever follows it now pulled up into view, which
    /// is the whole point of folding it.
    fn fold(&mut self, context: &mut Context, index: usize) {
        if !self.collapsed.remove(&index) {
            self.collapsed.insert(index);
        }
        self.repaginate_thread(context);
        let tag = u32::try_from(index + 1).unwrap_or(u32::MAX);
        if let Some(page) = self
            .thread_pages
            .iter()
            .position(|page| page.iter().any(|(carried, ..)| *carried == tag))
        {
            self.thread_page = page;
        }
        self.problem = None;
        self.trouble = None;
        self.show(context);
    }

    fn repaginate_thread(&mut self, context: &Context) {
        let paragraphs = self.thread_paragraphs();
        let borrowed = paragraphs
            .iter()
            .map(|(tag, depth, role, text)| (*tag, *depth, *role, text.as_str()))
            .collect::<Vec<_>>();
        self.thread_pages = context.paginate_tagged(&borrowed, true);
        self.thread_page = self
            .thread_page
            .min(self.thread_pages.len().saturating_sub(1));
    }

    /// Whether there is more of the thread still to fetch.
    fn more_to_take(&self) -> bool {
        self.comments.len() < model::MAX_COMMENTS
            && self.slots.iter().any(|slot| slot.comment.is_none())
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
        self.trouble = None;
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
        if action == ActionId::BACK {
            // Only ever delivered on a screen that asked for it, so this is
            // always a thread returning to the list it was opened from.
            self.view = View::List;
            self.problem = None;
            self.trouble = None;
            self.show(context);
            return;
        }
        for tab in Tab::ALL {
            if action == action_id(tab.action()) {
                self.switch_tab(context, tab);
                return;
            }
        }
        if action == action_id("stories") {
            self.view = View::List;
            self.problem = None;
            self.trouble = None;
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
        for index in 0..self.comments.len() {
            if action == action_id(&format!("fold-{index}")) {
                self.fold(context, index);
                return;
            }
        }
        if action == action_id("thread-next") {
            self.turn_thread(context);
            return;
        }
        if action == action_id("thread-back") {
            self.thread_page = self.thread_page.saturating_sub(1);
            self.problem = None;
            self.trouble = None;
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
        if let Some(at) = self.lanes.iter().position(|(lane, _)| *lane == task) {
            let (_, id) = self.lanes.remove(at);
            match outcome {
                TaskOutcome::Completed(bytes) => self.took_item(context, &bytes, id),
                TaskOutcome::Failed(_) => {
                    // One comment that will not come is one comment missing,
                    // not a broken thread. Its slot goes so the rest carries
                    // on, and its own replies go with it because there is
                    // nothing left to hang them under.
                    self.slots.retain(|slot| slot.id != id);
                    self.pump_thread(context);
                }
                TaskOutcome::Cancelled => return,
            }
            // Repainted only once a whole run has landed. Fetching one comment
            // per request is what buys the site's exact ordering; repainting
            // on each arrival would be one full panel refresh per comment.
            if self.lanes.is_empty() {
                self.rebuild_thread(context);
                if self.comments.is_empty() && !self.more_to_take() {
                    self.problem = Some("This thread came back empty.".to_owned());
                }
                self.show(context);
            }
            return;
        }
        if let Some(at) = self.story_lanes.iter().position(|(lane, _)| *lane == task) {
            let (_, id) = self.story_lanes.remove(at);
            match outcome {
                TaskOutcome::Completed(bytes) => self.took_story(context, &bytes, id),
                TaskOutcome::Failed(_) => {
                    // One story that will not come is one row missing, not a
                    // broken list. It leaves the ranking so the lane moves on
                    // rather than asking for it again forever.
                    self.ranking.retain(|listed| *listed != id);
                    self.pump_stories(context);
                }
                TaskOutcome::Cancelled => return,
            }
            if self.story_lanes.is_empty() {
                self.page = self.page.min(self.stories.len());
                self.repaginate_list(context);
                if self.stories.is_empty() {
                    self.problem = Some("That tab came back empty.".to_owned());
                }
                self.show(context);
            }
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
                Awaiting::Ranking(tab) => {
                    self.took_ranking(context, &bytes, tab);
                    // The stories themselves are already in the air, so the
                    // skeleton stays up rather than flashing an empty list.
                    if !self.story_lanes.is_empty() {
                        self.show(context);
                        return;
                    }
                }
            },
            TaskOutcome::Failed(error) => {
                // The SDK owns the wording, so every application says the same
                // thing about the same failure and a new TaskError variant does
                // not need an edit here.
                let failure = Failure::of(error);
                self.trouble = Some(failure);
                self.problem = Some(failure.advice.to_owned());
            }
            TaskOutcome::Cancelled => self.problem = Some("Cancelled.".to_owned()),
        }
        self.show(context);
    }
}

impl Hn {
    /// Turns the thread forward, going after more of it at the end.
    ///
    /// Reaching the end of what has arrived is an appetite for more, not a
    /// dead end: the rest of the conversation is known to exist and is being
    /// fetched a run at a time.
    fn turn_thread(&mut self, context: &mut Context) {
        if self.thread_page + 1 < self.thread_pages.len() {
            self.thread_page += 1;
            self.problem = None;
            self.trouble = None;
        } else if self.more_to_take() {
            self.want = self.loaded().saturating_add(CHUNK);
            self.pump_thread(context);
            self.problem = None;
            self.trouble = None;
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

/// What Hacker News says about a story that is not its title.
///
/// The domain, score, comment count and age, as a labelled block. They used to
/// share one byline, four clauses run together with middots, which reads as a
/// single long word and buries the score a reader is scanning for. The domain
/// is dropped for a self-post, which has none: a fact with an empty value is a
/// label pointing at a gap.
fn story_facts(story: &Story, now: i64) -> Vec<(&'static str, String)> {
    let mut facts = Vec::new();
    if let Some(site) = &story.site {
        facts.push(("Domain", site.clone()));
    }
    facts.push(("Score", model::score(story)));
    facts.push(("Comments", story.comments.to_string()));
    facts.push(("Age", model::age(now, story.created)));
    facts
}

/// A line that folds what is under it, or a plain one where nothing is.
///
/// [`ScreenBuilder::folding_byline`] always draws the little control, so a line
/// with nothing beneath it gets a plus sign that, tapped, redraws the identical
/// page -- the thread grew one on every childless comment before this decision
/// was made once, here, rather than slightly differently everywhere a fold is
/// wanted. Written against a text rather than a byline so that a grouped list or
/// a chat log can reach for the same rule.
fn folding_text(
    screen: ScreenBuilder,
    depth: u8,
    text: String,
    name: &str,
    collapsed: bool,
    hidden: u16,
) -> ScreenBuilder {
    if hidden == 0 && !collapsed {
        screen.quote_as(depth, QuoteRole::Byline, text)
    } else {
        screen.folding_byline(depth, text, name, collapsed, hidden)
    }
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
    use super::{model, Awaiting, Hn, Slot, Tab, View, CHUNK, FACT_TAG, LANES, TABS, TITLE_LINES};
    use kobo_sdk::{action_id, AppRunner, Command, Task, TaskError, TaskId, TaskOutcome};
    use kobo_ui::{Chrome, LayoutKind, Rect, CLARA_BW_METRICS};
    use std::collections::BTreeMap;

    /// Hacker News' own answer for the front page: item numbers, in order.
    ///
    /// Deliberately not in ascending order. `topstories` *is* the front page,
    /// and if the application ever sorts the answer itself the tests below say
    /// so, because the reader would then be shown a front page that is not the
    /// front page.
    const RANKING: &str = include_str!("../tests/ranking.json");

    /// The five stories that ranking names, one captured item per line.
    const FRONT_PAGE: &str = include_str!("../tests/front_page.jsonl");

    /// A whole captured conversation: the story item and every comment under
    /// it, one item per line, exactly as `item/<number>.json` answered.
    const THREAD: &str = include_str!("../tests/thread.jsonl");

    /// The story the captured thread belongs to.
    const THREAD_STORY: i64 = 49_079_727;

    /// Every captured item in a fixture, by item number.
    fn fixture(body: &str) -> BTreeMap<i64, String> {
        body.lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                let id = kobo_json::parse(line)
                    .expect("a captured item parses")
                    .get("id")
                    .and_then(kobo_json::Value::as_i64)
                    .expect("a captured item is numbered");
                (id, line.to_owned())
            })
            .collect()
    }

    /// The item numbers in a ranking fixture, in the order it lists them.
    fn ranking_of(body: &str) -> Vec<i64> {
        kobo_json::parse(body)
            .expect("the fixture parses")
            .as_array()
            .expect("an array of item numbers")
            .iter()
            .filter_map(kobo_json::Value::as_i64)
            .collect()
    }

    /// An application with a real front page already loaded.
    fn loaded() -> AppRunner<Hn> {
        let mut runner = AppRunner::new(Hn::default());
        runner.start();
        answer_list(&mut runner);
        runner
    }

    /// Answers the ranking request, then every story item it leads to.
    fn answer_list(runner: &mut AppRunner<Hn>) {
        let task = spawned(runner);
        runner.task_outcome(task, TaskOutcome::Completed(RANKING.as_bytes().to_vec()));
        answer_lanes(runner, &fixture(FRONT_PAGE));
    }

    /// Opens the captured thread and answers every item it asks for.
    fn opened_thread() -> (AppRunner<Hn>, usize) {
        let mut runner = loaded();
        let index = runner
            .app()
            .stories
            .iter()
            .position(|story| story.id == THREAD_STORY.to_string())
            .expect("the captured thread's story is on the captured front page");
        runner.action(action_id(&format!("story-{index}")));
        answer_lanes(&mut runner, &fixture(THREAD));
        (runner, index)
    }

    /// Answers every outstanding item fetch from `bodies` until none is left.
    ///
    /// An item number the fixture does not hold is answered `null`, which is
    /// what the site itself says about a number that is not an item.
    fn answer_lanes(runner: &mut AppRunner<Hn>, bodies: &BTreeMap<i64, String>) -> usize {
        let mut repaints = 0;
        for _ in 0..2000 {
            let Some((task, id)) = runner
                .app()
                .story_lanes
                .first()
                .or_else(|| runner.app().lanes.first())
                .copied()
            else {
                return repaints;
            };
            let body = bodies
                .get(&id)
                .cloned()
                .unwrap_or_else(|| "null".to_owned());
            let commands = runner.task_outcome(task, TaskOutcome::Completed(body.into_bytes()));
            repaints += commands
                .iter()
                .filter(|command| matches!(command, Command::SetScreen(_)))
                .count();
        }
        panic!("the fetcher never ran out of work");
    }

    fn spawned(runner: &AppRunner<Hn>) -> TaskId {
        runner
            .app()
            .task
            .map(|(task, _)| task)
            .expect("a request is in flight")
    }

    #[test]
    fn a_failed_tab_with_no_stories_is_the_whole_screen_not_a_banner_over_nothing() {
        // A banner needs something to sit above. With no rows, the failure is
        // the screen, and `standard_state` centres it and names it the same
        // way every other application does.
        let mut runner = AppRunner::new(Hn {
            task: Some((TaskId(1), Awaiting::Ranking(Tab::Top))),
            ..Hn::default()
        });
        runner.task_outcome(TaskId(1), TaskOutcome::Failed(TaskError::Offline));
        let screen = runner.app_mut().list();
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true));
        assert!(
            layout
                .nodes
                .iter()
                .any(|node| matches!(node.kind, LayoutKind::SplashTitle)),
            "the offline failure did not become a splash"
        );
        let text: Vec<String> = layout
            .nodes
            .iter()
            .flat_map(|node| node.text_lines.clone())
            .collect();
        assert!(
            text.iter().any(|line| line.contains("not on a network")),
            "the SDK's wording is not on the screen: {text:?}"
        );
    }

    #[test]
    fn every_failure_is_worded_by_the_sdk() {
        // The wording lives in one place so a new TaskError variant does not
        // need an edit in every application that can see it.
        for error in [
            TaskError::Offline,
            TaskError::Unreachable,
            TaskError::Denied,
        ] {
            let mut runner = AppRunner::new(Hn {
                task: Some((TaskId(1), Awaiting::Ranking(Tab::Top))),
                ..Hn::default()
            });
            runner.task_outcome(TaskId(1), TaskOutcome::Failed(error));
            assert_eq!(
                runner.app_mut().problem.clone().unwrap_or_default(),
                kobo_sdk::Failure::of(error).advice
            );
        }
    }

    #[test]
    fn a_failure_that_retrying_cannot_help_offers_no_retry() {
        // A refused permission will refuse again. The control is left off
        // rather than offered and disappointing.
        let mut runner = AppRunner::new(Hn {
            task: Some((TaskId(1), Awaiting::Ranking(Tab::Top))),
            ..Hn::default()
        });
        runner.task_outcome(TaskId(1), TaskOutcome::Failed(TaskError::Denied));
        let screen = runner.app_mut().list();
        let text: Vec<String> = screen
            .layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true))
            .nodes
            .iter()
            .flat_map(|node| node.text_lines.clone())
            .collect();
        assert!(
            !text.iter().any(|line| line.contains("Try again")),
            "a retry is offered for a failure retrying cannot help: {text:?}"
        );
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

    /// Every URL a batch of commands asked the network for.
    fn all_asked(commands: &[Command]) -> Vec<String> {
        commands
            .iter()
            .filter_map(|command| match command {
                Command::Spawn {
                    work: Task::Fetch { url, .. },
                    ..
                } => Some(url.clone()),
                _ => None,
            })
            .collect()
    }

    fn tab_rects(screen: &kobo_sdk::Screen) -> Vec<Rect> {
        screen
            .layout_with(&CLARA_BW_METRICS, &Chrome::default())
            .nodes
            .iter()
            .filter(|node| {
                matches!(
                    node.kind,
                    LayoutKind::NavDestination(..) | LayoutKind::NavDestinationSelected(..)
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
    fn every_tab_asks_hacker_news_for_the_page_it_is_named_after() {
        // These four endpoints are the pages themselves: `topstories` is what
        // /news serves, `askstories` is what /ask serves, and so on, verified
        // against the live site position for position. Getting one wrong gives
        // a tab that quietly shows some other page under this one's name, and
        // asking anything else gives a Hacker News of our own invention.
        let mut runner = loaded();
        for (tab, expected) in [
            (Tab::New, "newstories"),
            (Tab::Ask, "askstories"),
            (Tab::Show, "showstories"),
            (Tab::Top, "topstories"),
        ] {
            let commands = runner.action(action_id(tab.action()));
            let url = asked(&commands).expect("the tab asked for something");
            assert_eq!(
                url,
                format!(
                    "https://hacker-news.firebaseio.com/v0/{expected}.json\
                     ?orderBy=%22%24key%22&limitToFirst=30"
                ),
                "{tab:?} asked the wrong page"
            );
            answer_list(&mut runner);
        }
    }

    #[test]
    fn nothing_ranks_the_stories_except_hacker_news() {
        // The reason the list is fetched an item at a time. The lanes finish in
        // whatever order the radio gives them back, and taking that order gave
        // a front page shuffled by network timing. `topstories` is the front
        // page; its order is imposed on the answers however they arrive.
        let runner = loaded();
        let listed = ranking_of(RANKING)
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>();
        let shown = runner
            .app()
            .stories
            .iter()
            .map(|story| story.id.clone())
            .collect::<Vec<_>>();
        assert_eq!(shown, listed, "the stories were not in Hacker News' order");
    }

    #[test]
    fn every_story_is_asked_for_by_its_own_item_number() {
        // The site's own record of a story is the only source that is never
        // behind. The search index this application used to read lags by
        // minutes, and on a front page that turns over in minutes that meant a
        // story could be on the site and missing from the list, with every
        // score and comment count slightly wrong.
        let mut runner = AppRunner::new(Hn::default());
        runner.start();
        let task = spawned(&runner);
        let commands =
            runner.task_outcome(task, TaskOutcome::Completed(RANKING.as_bytes().to_vec()));
        let urls = all_asked(&commands);
        assert_eq!(urls.len(), LANES, "the lanes were not all filled at once");
        let ranking = ranking_of(RANKING);
        for (url, id) in urls.iter().zip(&ranking) {
            assert_eq!(
                *url,
                format!("https://hacker-news.firebaseio.com/v0/item/{id}.json"),
                "a lane asked for something other than the next story"
            );
        }
        answer_lanes(&mut runner, &fixture(FRONT_PAGE));
        assert_eq!(runner.app().stories.len(), ranking.len());
    }

    #[test]
    fn the_list_repaints_once_the_lanes_are_done_rather_than_once_per_story() {
        // Every repaint is a full panel refresh. Thirty of them for one list
        // would be a list that flickers for half a minute before it settles.
        let mut runner = AppRunner::new(Hn::default());
        runner.start();
        let task = spawned(&runner);
        runner.task_outcome(task, TaskOutcome::Completed(RANKING.as_bytes().to_vec()));
        let repaints = answer_lanes(&mut runner, &fixture(FRONT_PAGE));
        assert_eq!(
            repaints, 1,
            "the list repainted {repaints} times for one page of stories"
        );
    }

    #[test]
    fn a_story_the_site_no_longer_answers_for_does_not_wedge_the_list() {
        // Items are deleted, and a number in the ranking that answers `null`
        // has to move the lane on rather than hold it forever.
        let mut runner = AppRunner::new(Hn::default());
        runner.start();
        let task = spawned(&runner);
        runner.task_outcome(task, TaskOutcome::Completed(RANKING.as_bytes().to_vec()));
        let mut bodies = fixture(FRONT_PAGE);
        let gone = ranking_of(RANKING)[1];
        bodies.remove(&gone);
        answer_lanes(&mut runner, &bodies);
        assert_eq!(
            runner.app().stories.len(),
            4,
            "the missing story took the others with it"
        );
        assert!(
            runner
                .app()
                .stories
                .iter()
                .all(|story| story.id != gone.to_string()),
            "a story the site does not have was drawn anyway"
        );
    }

    #[test]
    fn an_empty_ranking_is_said_out_loud_rather_than_shown_as_an_empty_list() {
        // A tab with nothing in it and a tab whose request failed look the
        // same on a panel unless one of them says so.
        let mut runner = AppRunner::new(Hn::default());
        runner.start();
        let task = spawned(&runner);
        runner.task_outcome(task, TaskOutcome::Completed(b"[]".to_vec()));
        assert!(runner.app().task.is_none(), "it went on asking anyway");
        assert!(runner.app().problem.is_some(), "nothing was said about it");
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
                .layout_with(&CLARA_BW_METRICS, &Chrome::default());
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
            .layout_with(&CLARA_BW_METRICS, &Chrome::default());
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
        let (runner, _) = opened_thread();
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
                .layout_with(&CLARA_BW_METRICS, &Chrome::default());
            for node in &layout.nodes {
                if let LayoutKind::Quote(depth, _) = node.kind {
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
        let (runner, _) = opened_thread();
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
                .layout_with(&CLARA_BW_METRICS, &Chrome::default());
            // A paragraph is drawn either as a quote or, where it stood in for
            // one of the story's facts, as a value in the facts block; one
            // value per reserved fact line, so the two together still have to
            // account for every paragraph the page was measured to hold.
            let drawn = layout
                .nodes
                .iter()
                .filter(|node| matches!(node.kind, LayoutKind::Quote(..) | LayoutKind::FactValue))
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
        let (runner, _) = opened_thread();
        let layout = runner
            .app()
            .thread()
            .layout_with(&CLARA_BW_METRICS, &Chrome::default());
        let controls = layout
            .nodes
            .iter()
            .filter_map(|node| match node.kind {
                LayoutKind::NavDestination(action, ..) => Some((action, node.rect)),
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
    fn a_thread_is_drawn_in_the_order_the_site_draws_it() {
        // The single reason this application asks for one item at a time. The
        // site ranks siblings by its own scoring, which is neither
        // chronological nor anything a client can recompute; `kids` is that
        // ranking, and the captured fixture proves it is not a sort, because
        // its first reply has a higher item number than its second and its
        // last is the oldest of the four.
        let (runner, _) = opened_thread();
        let bodies = fixture(THREAD);
        let root = kobo_json::parse(&bodies[&THREAD_STORY]).expect("the story parses");
        let kids = model::item_from(&root).expect("the story reads").kids;
        assert!(
            kids.windows(2).any(|pair| pair[0] > pair[1]),
            "the fixture is in ascending order, so it proves nothing"
        );
        let top = runner
            .app()
            .slots
            .iter()
            .filter(|slot| slot.depth == 0)
            .map(|slot| slot.id)
            .collect::<Vec<_>>();
        // The fixture's last reply is flagged with nothing under it, which is
        // a comment a reader of the site does not see either.
        let drawn = kids
            .iter()
            .copied()
            .filter(|id| *id != 49_079_728)
            .collect::<Vec<_>>();
        assert_eq!(top, drawn, "the top level was not in the site's order");
    }

    #[test]
    fn a_reply_lands_under_what_it_answers_rather_than_at_the_end() {
        // Replies arrive out of order because the lanes finish out of order.
        // Appending them would give a conversation where every answer is at
        // the bottom, which is the shape of a chat log and not of a thread.
        let (runner, _) = opened_thread();
        let slots = &runner.app().slots;
        let parent = slots
            .iter()
            .position(|slot| slot.id == 49_080_373)
            .expect("the fixture's first reply is in the thread");
        assert_eq!(slots[parent].depth, 0);
        assert_eq!(
            slots[parent + 1].id,
            49_080_510,
            "the reply did not follow the comment it answers"
        );
        assert_eq!(slots[parent + 1].depth, 1);
        assert_eq!(slots[parent + 2].id, 49_080_628);
        assert_eq!(slots[parent + 2].depth, 2);
        assert_eq!(slots[parent + 3].id, 49_080_734);
        assert_eq!(slots[parent + 3].depth, 3);
    }

    #[test]
    fn a_flagged_comment_with_nothing_under_it_is_not_drawn_at_all() {
        // The fixture holds one, and a reader of the site does not see it
        // either. Drawing its text would put on the panel the one thing the
        // site took off it.
        let (runner, _) = opened_thread();
        assert!(
            runner
                .app()
                .comments
                .iter()
                .all(|comment| !comment.body.contains("[flagged]")),
            "a flagged comment reached the panel"
        );
        assert!(
            runner.app().slots.iter().all(|slot| slot.id != 49_079_728),
            "a flagged comment kept a slot the fetcher would keep filling"
        );
    }

    #[test]
    fn a_comment_the_site_no_longer_answers_for_does_not_wedge_the_thread() {
        // An item number in `kids` that answers `null` has to take its slot
        // with it, or the fetcher asks for it again forever and the thread
        // never finishes loading.
        let mut runner = loaded();
        let index = runner
            .app()
            .stories
            .iter()
            .position(|story| story.id == THREAD_STORY.to_string())
            .expect("the captured story is on the captured front page");
        runner.action(action_id(&format!("story-{index}")));
        let mut bodies = fixture(THREAD);
        bodies.remove(&49_080_366);
        answer_lanes(&mut runner, &bodies);
        let application = runner.app();
        assert!(
            application.slots.iter().all(|slot| slot.id != 49_080_366),
            "a comment the site does not have kept its slot"
        );
        assert!(
            application.comments.len() >= 5,
            "the missing comment took the thread with it: {} left",
            application.comments.len()
        );
    }

    #[test]
    fn a_request_that_fails_costs_one_comment_and_not_the_thread() {
        // A radio that drops one answer out of a hundred is an ordinary radio.
        let mut runner = loaded();
        let index = runner
            .app()
            .stories
            .iter()
            .position(|story| story.id == THREAD_STORY.to_string())
            .expect("the captured story is on the captured front page");
        runner.action(action_id(&format!("story-{index}")));
        let (task, id) = runner.app().lanes[0];
        assert_eq!(id, THREAD_STORY);
        let bodies = fixture(THREAD);
        runner.task_outcome(
            task,
            TaskOutcome::Completed(bodies[&id].clone().into_bytes()),
        );
        let (task, dropped) = runner.app().lanes[0];
        runner.task_outcome(task, TaskOutcome::Failed(TaskError::TooLarge));
        answer_lanes(&mut runner, &bodies);
        assert!(
            runner.app().slots.iter().all(|slot| slot.id != dropped),
            "a dropped comment kept its slot"
        );
        assert!(
            !runner.app().comments.is_empty(),
            "one dropped answer emptied the thread"
        );
    }

    #[test]
    fn the_thread_repaints_once_a_run_has_landed_rather_than_once_per_comment() {
        // Every repaint is a full panel refresh, so one per comment would be a
        // thread that flashes for a minute before it can be read. The lanes
        // are refilled before the panel is asked to redraw, so they only ever
        // empty at the end of a run.
        let mut runner = loaded();
        let index = runner
            .app()
            .stories
            .iter()
            .position(|story| story.id == THREAD_STORY.to_string())
            .expect("the captured story is on the captured front page");
        runner.action(action_id(&format!("story-{index}")));
        let repaints = answer_lanes(&mut runner, &fixture(THREAD));
        let comments = runner.app().comments.len();
        assert!(comments > 4, "too few comments to prove anything");
        assert!(
            repaints <= 1 + comments / CHUNK,
            "the thread repainted {repaints} times for {comments} comments"
        );
    }

    #[test]
    fn reaching_the_end_of_what_arrived_asks_for_more_rather_than_dead_ending() {
        // The rest of the conversation is known to exist. Saying "that is the
        // end of the thread" when it is only the end of what has been fetched
        // is the application lying about the site.
        let mut runner = loaded();
        let index = runner
            .app()
            .stories
            .iter()
            .position(|story| story.id == THREAD_STORY.to_string())
            .expect("the captured story is on the captured front page");
        runner.action(action_id(&format!("story-{index}")));
        let bodies = fixture(THREAD);
        let (task, id) = runner.app().lanes[0];
        runner.task_outcome(
            task,
            TaskOutcome::Completed(bodies[&id].clone().into_bytes()),
        );
        // Nothing else answered, so every slot below the top is still empty
        // and the reader is looking at the end of what arrived.
        runner.app_mut().lanes.clear();
        runner.app_mut().want = 0;
        assert!(runner.app().more_to_take());
        let commands = runner.action(action_id("thread-next"));
        assert!(
            asked(&commands).is_some(),
            "the end of the loaded part was reported as the end of the thread"
        );
        assert_eq!(runner.app().problem, None);
    }

    #[test]
    fn a_thread_stops_asking_at_the_ceiling_rather_than_growing_forever() {
        // A thread of forty thousand comments is one request each. The cap is
        // what stops "tap Next" being a way to fill the device's memory, and a
        // panel that turns a page a second is nobody's way through it.
        let (mut runner, _) = opened_thread();
        runner.app_mut().slots = (1..=10)
            .map(|id| Slot {
                id,
                depth: 0,
                comment: None,
            })
            .collect();
        runner.app_mut().comments.clear();
        assert!(runner.app().more_to_take());
        runner.app_mut().comments = vec![model::Comment::default(); model::MAX_COMMENTS];
        assert!(
            !runner.app().more_to_take(),
            "the fetcher would keep asking past the ceiling"
        );
    }

    #[test]
    fn a_thread_larger_than_the_old_ceiling_says_nothing_about_it() {
        let mut runner = loaded();
        let index = runner
            .app()
            .stories
            .iter()
            .position(|story| story.id == THREAD_STORY.to_string())
            .expect("the captured story is on the captured front page");
        runner.action(action_id(&format!("story-{index}")));
        let (task, _) = runner.app().lanes[0];
        let huge = format!(
            r#"{{"id": {THREAD_STORY}, "type": "story", "by": "a", "title": "T",
                 "descendants": 40000, "kids": [1, 2, 3]}}"#
        );
        runner.task_outcome(task, TaskOutcome::Completed(huge.into_bytes()));
        assert!(
            runner.app().note.is_none(),
            "a popular story was explained away instead of being shown: {:?}",
            runner.app().note
        );
        assert_eq!(
            runner.app().slots.len(),
            3,
            "the site named three replies, so three is what there is to read"
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
        assert!(matches!(runner.app().task, Some((_, Awaiting::Ranking(_)))));
        assert_ne!(runner.app().task.map(|(task, _)| task), Some(first));
    }

    #[test]
    fn an_identifier_that_is_not_a_number_never_becomes_a_url() {
        // The identifier arrives from the network and goes straight into a
        // request this device makes. Anything but digits is somebody else
        // naming the address.
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
        let (mut runner, _) = opened_thread();
        let commands = runner.action(action_id("tab-top"));
        assert!(asked(&commands).is_none(), "the list was fetched twice");
        assert_eq!(runner.app().view, View::List);
    }

    #[test]
    fn opening_a_second_story_keeps_nothing_from_the_first() {
        // The comments are held as one list. Leaving them in place draws the
        // last story's thread under the new story's title for as long as the
        // request takes.
        let (mut runner, _) = opened_thread();
        assert!(!runner.app().comments.is_empty());
        runner.action(action_id("stories"));
        runner.action(action_id("story-1"));
        let application = runner.app();
        assert!(application.comments.is_empty());
        assert!(application.slots.is_empty());
        assert_eq!(application.thread_page, 0);
        assert_eq!(application.note, None);
    }

    fn a_thread_of(depths: &[u16]) -> Hn {
        Hn {
            stories: vec![model::Story {
                id: "1".to_owned(),
                title: "A story".to_owned(),
                author: "someone".to_owned(),
                points: 10,
                comments: u32::try_from(depths.len()).unwrap_or(u32::MAX),
                created: 0,
                text: None,
                site: None,
            }],
            open: Some(0),
            comments: depths
                .iter()
                .enumerate()
                .map(|(index, depth)| model::Comment {
                    author: format!("author{index}"),
                    created: 0,
                    depth: *depth,
                    body: format!("Comment number {index}."),
                })
                .collect(),
            ..Hn::default()
        }
    }

    #[test]
    fn folding_a_comment_takes_its_replies_and_not_its_siblings() {
        // The list is pre-order, so a comment's replies are the run after it
        // while the depth stays greater. Counting on the drawn indent instead
        // would be wrong past the indent cap, where every level looks alike.
        let application = a_thread_of(&[0, 1, 2, 1, 0]);
        assert_eq!(application.replies_to(0), 3, "the first comment's subtree");
        assert_eq!(application.replies_to(1), 1);
        assert_eq!(application.replies_to(3), 0);
        assert_eq!(application.replies_to(4), 0, "the last comment has none");

        // Past the indent cap every level is drawn at the same offset. A
        // subtree measured on what is drawn rather than on what is true would
        // swallow the sibling at 4 along with the reply at 5.
        let capped = u16::from(model::MAX_INDENT);
        let deep = a_thread_of(&[capped, capped + 1, capped, capped + 1]);
        assert_eq!(
            deep.replies_to(0),
            1,
            "a comment past the indent cap took its own sibling with it"
        );
        assert_eq!(deep.replies_to(2), 1);
    }

    #[test]
    fn a_folded_comment_takes_its_own_words_with_it() {
        // Hiding only the replies would leave the comment's text on the page,
        // and a reader who folded it away was folding the whole thing away.
        let mut application = a_thread_of(&[0, 1, 2, 0]);
        let open = application.thread_paragraphs();
        application.collapsed.insert(0);
        let shut = application.thread_paragraphs();
        let bodies = |paragraphs: &[(u32, u8, kobo_sdk::QuoteRole, String)]| {
            paragraphs
                .iter()
                .filter(|(_, _, role, _)| *role == kobo_sdk::QuoteRole::Body)
                .map(|(_, _, _, text)| text.clone())
                .collect::<Vec<_>>()
        };
        assert!(bodies(&open).iter().any(|text| text.contains("number 0")));
        assert!(
            !bodies(&shut).iter().any(|text| text.contains("number 0")),
            "the folded comment's own words stayed on the page"
        );
        for hidden in ["number 1", "number 2"] {
            assert!(
                !bodies(&shut).iter().any(|text| text.contains(hidden)),
                "{hidden} was a reply to the folded comment and should have gone with it"
            );
        }
        assert!(
            bodies(&shut).iter().any(|text| text.contains("number 3")),
            "a sibling of the folded comment was folded away too"
        );
    }

    #[test]
    fn a_folded_byline_stays_on_the_page_so_it_can_be_opened_again() {
        // A fold that removed its own handle would be a comment the reader
        // could hide and then never get back.
        let mut application = a_thread_of(&[0, 1, 0]);
        application.collapsed.insert(0);
        let paragraphs = application.thread_paragraphs();
        assert!(
            paragraphs
                .iter()
                .any(|(_, _, role, text)| *role == kobo_sdk::QuoteRole::Byline
                    && text.contains("author0")),
            "the folded comment's byline went with it"
        );
    }

    #[test]
    fn the_tag_leads_back_to_the_comment_a_paragraph_came_from() {
        // What makes the fold work at all: pagination splits paragraphs and
        // repeats bylines, so counting is not a way back and the identity has
        // to be carried.
        let application = a_thread_of(&[0, 1, 0]);
        let paragraphs = application.thread_paragraphs();
        for (tag, _, _, text) in &paragraphs {
            if *tag == FACT_TAG {
                // A reserved fact line, which belongs to the story rather than
                // to any comment, so its tag leads nowhere and is meant to.
                continue;
            }
            if let Some(index) = (*tag as usize).checked_sub(1) {
                let author = &application.comments[index].author;
                assert!(
                    text.contains(author) || text.contains(&format!("number {index}")),
                    "{text:?} carried tag {tag}, which is {author}'s"
                );
            }
        }
    }
}

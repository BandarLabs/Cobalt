//! Hacker News, on a panel with no scrollbar and no keyboard.
//!
//! Five destinations along the bottom. Four are the site's own lists (Top,
//! New, Ask, Show) and the fifth is what this reader put aside, which is the
//! one that works with the radio off. Behind every story is its discussion,
//! and behind a story with a link is the article itself, read here rather
//! than left on a site nobody can reach from this device.
//!
//! ## One item per request, on purpose
//!
//! Hacker News' own API answers one item at a time, which is more round trips
//! than a search index needs. It is worth every one of them: it is the site's
//! own record, so a story submitted a minute ago is in the list, every score
//! is the score on the page, and `kids` is the order the site draws replies
//! in. A client cannot recompute that ordering, and a ranked search index
//! answering thirty at once got Ask HN wrong by thirteen years.
//!
//! Comments are fetched as the reader pages into them, so the radio a thread
//! costs tracks how far it was actually read rather than how popular it is.
//!
//! ## What the device remembers
//!
//! Which stories have been opened, and which were put aside. The site keeps
//! both for a logged-in reader and will keep neither for an application, and
//! the alternative to keeping them here is asking somebody for their Hacker
//! News password so that a list can be grey where they have already been.

mod model;

use kobo_bookview::illustrations::Illustrations;
use kobo_bookview::BookView;
use kobo_sdk::snapshot::{Snapshot, SnapshotEvent};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Failure, Glyph, KoboApp, Position, QuoteRole,
    RowLead, Screen, ScreenBuilder, StoreResult, Task, TaskId, TaskOutcome,
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

/// Where the lists and the items are actually read from.
///
/// Hacker News, unless `KOBO_HN_ORIGIN` names somewhere else. That exists for
/// fixtures and captures, which need a server they own; it is read once per
/// request and never written down, so a story kept under a fixture cannot
/// quietly become a story from somewhere the reader did not choose. Only
/// HTTPS, because everything else this application asks for is.
fn api() -> String {
    std::env::var("KOBO_HN_ORIGIN")
        .ok()
        .filter(|origin| origin.starts_with("https://"))
        .map_or_else(|| HN_API.to_owned(), |origin| format!("{origin}/v0"))
}

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
    format!("{}/item/{id}.json", api())
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

/// How many placeholder rows stand in for a list while it is arriving, when
/// the panel has not been measured yet.
///
/// A floor rather than a count. Six fits a Clara BW at the default text size
/// and runs off the bottom of it at 170%, where the renderer refuses the whole
/// screen rather than draw through the edge: a reader who turned the type up
/// got a blank panel while any list was loading. What is drawn now is what
/// fits, measured the same way the list under it is measured.
const SKELETON_ROWS: u8 = 3;

/// Where the marks are kept on the device.
///
/// One small file rather than one per story: the whole of it is read at start
/// and written when it changes, and a reader with three hundred read stories
/// still costs one read and one write.
const MARKS: &str = "hn-marks";

/// The bottom bar. Fixed, in this order, on every screen that has a list.
///
/// Saved is last because it is the only one that is not Hacker News: the four
/// before it are the site's own pages, in the site's own order, and this one
/// is the reader's.
const TABS: [(&str, &str); 5] = [
    ("tab-top", "Top"),
    ("tab-new", "New"),
    ("tab-ask", "Ask"),
    ("tab-show", "Show"),
    ("tab-saved", "Saved"),
];

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Tab {
    #[default]
    Top,
    New,
    Ask,
    Show,
    /// What this reader put aside, which is on the device rather than on the
    /// site and is the one list that needs no radio.
    Saved,
}

impl Tab {
    const ALL: [Self; 5] = [Self::Top, Self::New, Self::Ask, Self::Show, Self::Saved];

    const fn index(self) -> usize {
        match self {
            Self::Top => 0,
            Self::New => 1,
            Self::Ask => 2,
            Self::Show => 3,
            Self::Saved => 4,
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
            // Never drawn: the saved list is already on the device, so there
            // is no moment between asking for it and having it.
            Self::Saved => "Opening what you put aside",
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
    fn ranking_url(self) -> Option<String> {
        let list = match self {
            Self::Top => "topstories",
            Self::New => "newstories",
            Self::Ask => "askstories",
            Self::Show => "showstories",
            // Not a list Hacker News holds. Nothing to ask anybody for.
            Self::Saved => return None,
        };
        Some(format!(
            "{}/{list}.json?orderBy=%22%24key%22&limitToFirst={HITS}",
            api()
        ))
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum View {
    #[default]
    List,
    Thread,
    /// The article behind a story, read here rather than on a site this
    /// device cannot reach.
    Reading,
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
    /// How many placeholder rows this panel holds, once it has been measured.
    skeleton: u8,
    /// What this reader has already opened, and what they put aside.
    marks: model::Marks,
    /// Whether the marks have been read back from the device yet.
    ///
    /// Until they have, nothing is written: a save that lands before the file
    /// has been read would be written over the top of everything saved on
    /// every previous day.
    marks_known: bool,
    /// Which saved story's menu is open, as an index into `stories`.
    menu: Option<usize>,
    /// The article being read, when one is.
    book: BookView,
    /// The pictures in that article.
    illustrations: Illustrations,
    /// The copy of the article kept on the device, so it reopens with the
    /// radio off.
    article: Option<Snapshot>,
    /// The request fetching an article, when the device has no copy yet.
    fetching: Option<TaskId>,
}

impl Hn {
    fn show(&self, context: &mut Context) {
        if self.view == View::Reading {
            // The shared reader draws its own screen, including the page
            // turns and the controls in the middle column, so nothing here
            // draws over the top of it. Its own Back is the way out.
            let title = self
                .open
                .and_then(|index| self.stories.get(index))
                .map_or_else(String::new, |story| story.title.clone());
            let screen = self.book.screen(&title).unwrap_or_else(|| {
                ScreenBuilder::new("hn-article")
                    .top_bar(title)
                    .empty_state("This article arrived empty.")
                    .bottom_action("close-article", "Discussion")
                    .build()
            });
            context.set_screen(screen.with_own_back(true));
            return;
        }
        let screen = match self.view {
            View::List => self.list(),
            View::Thread | View::Reading => self.thread(),
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
        if self.tab == Tab::Saved && self.stories.is_empty() {
            return self.nothing_saved(screen);
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
                        .skeleton(self.skeleton.max(1)),
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
        if self.tab == Tab::Saved {
            return self.saved_list(screen, indices);
        }
        let rows = indices.iter().filter_map(|index| {
            let story = self.stories.get(*index)?;
            Some((
                format!("story-{index}"),
                self.titles.get(*index).cloned().unwrap_or_default(),
                self.row_summary(story),
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

    /// The saved list: the same stories, with the one verb that only applies
    /// here.
    ///
    /// No score and no rank. Both were true when the story was put aside and
    /// neither has been true since, and a number that looks live and is a day
    /// old is worse than no number. What the row carries instead is the mark
    /// that takes it off this list, which is the only thing a reader wants to
    /// do to a saved story that is not opening it.
    fn saved_list(&self, screen: ScreenBuilder, indices: &[usize]) -> Screen {
        let rows = indices.iter().filter_map(|index| {
            let story = self.stories.get(*index)?;
            Some((
                format!("story-{index}"),
                self.titles.get(*index).cloned().unwrap_or_default(),
                self.row_summary(story),
                RowLead::from(Glyph::Bookmark),
                format!("saved-menu-{index}"),
            ))
        });
        let screen = screen
            .rows_with_menu(rows)
            .page_turns("list-back", "list-next");
        let screen = match self.menu {
            Some(index) if indices.contains(&index) => screen.row_overflow(
                format!("saved-menu-{index}"),
                true,
                [("saved-forget", "Take off this list", Glyph::Trash)],
            ),
            _ => screen,
        };
        self.with_tabs(screen).build()
    }

    /// Nothing saved, said where the saved stories would be.
    ///
    /// Not the failure state the other tabs use. An empty saved list is not a
    /// network problem and there is nothing to try again: it is a list that
    /// has never been added to, so it says how stories get onto it.
    fn nothing_saved(&self, screen: ScreenBuilder) -> Screen {
        self.with_tabs(screen.empty_state(
            "Nothing saved yet. Open a story and tap Save, and it waits here \
             with its article, radio or no radio.",
        ))
        .build()
    }

    /// The second line of a row, with what this reader has already done with
    /// the story said before anything the site says about it.
    ///
    /// At the front rather than appended: a reader scanning a list runs their
    /// eye down the left edge, and a mark at the end of a line that is
    /// sometimes three clauses long is a mark nobody finds. Only the few rows
    /// that have been touched carry one at all.
    fn row_summary(&self, story: &Story) -> String {
        let summary = model::summary(story, self.now);
        let read = self.marks.was_read(&story.id);
        // Not on the saved list itself, where every row is saved and the word
        // would be the same word thirty times.
        let saved = self.tab != Tab::Saved && self.marks.was_saved(&story.id);
        match (read, saved) {
            (true, true) => format!("Read, saved \u{b7} {summary}"),
            (true, false) => format!("Read \u{b7} {summary}"),
            (false, true) => format!("Saved \u{b7} {summary}"),
            (false, false) => summary,
        }
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
        // The headline is in the bar and nowhere else. It used to be in both
        // the bar and the first paragraph, which is the same words twice on
        // one panel with one of them cut short, and it cost the first page a
        // headline's worth of comments. The bar is where it has to stay: it
        // is the only part of the screen that survives paging into the
        // replies, and a reader four pages down a thread should not have to
        // page back to find out whose story they are arguing about.
        let mut screen = ScreenBuilder::new("hn-thread").top_bar(story.title.clone());
        screen = if self.marks.was_saved(&story.id) {
            screen.top_bar_action("save", "Saved")
        } else {
            screen.top_bar_action("save", "Save")
        };
        if story.link.is_some() {
            screen = screen.top_bar_glyph("read-article", "Read the article", Glyph::Book);
        }
        if !self.lanes.is_empty() && self.comments.is_empty() {
            // No bar, which is what the loaded thread has too: nothing under
            // the reader's finger moves when the comments land.
            return screen
                .activity("Fetching the comments", None)
                .skeleton(self.skeleton.max(1))
                .build();
        }
        // What the site says about the story, above the first page of the
        // discussion and above no other. The pagination was measured under
        // exactly this block, so what is drawn here is room the comments
        // never had rather than room taken from them.
        if self.thread_page == 0 {
            screen = screen.facts(story_facts(story, self.now));
        }
        // A byline is only made foldable once, on the page where its comment
        // begins. The copy repeated at the top of a continuation is a
        // reminder of who is speaking, and hanging a control off it would put
        // two plus signs for the same comment in front of the reader.
        let mut folded_here: Option<u32> = None;
        for (tag, depth, role, paragraph) in self
            .thread_pages
            .get(self.thread_page)
            .into_iter()
            .flatten()
        {
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
        // No banner, and no bottom bar. A banner is chrome the paginator
        // never measured: on a full page at the larger text settings the
        // renderer refused the whole screen rather than draw it through the
        // panel edge, and a reader who turned the type up got nothing at all.
        // Everything this screen has to say is said inside the flow, which is
        // what was measured, so it has somewhere to be.
        // No bottom bar. It said "Back / Stories / Next", of which Stories is
        // the chevron already in the top bar and the other two are the page
        // turns the position strip draws for a third of the room. The bar was
        // three labels for two things the reader already had.
        let mut turning = screen.page_turns("thread-back", "thread-next");
        let pages = self.thread_pages.len();
        if pages > 1 {
            turning = turning.page_position(
                u16::try_from(self.thread_page + 1).unwrap_or(u16::MAX),
                u16::try_from(pages).unwrap_or(u16::MAX),
            );
        }
        turning.build()
    }

    /// The block of facts that stands above the first page of a discussion.
    ///
    /// Built as a screen of its own so it can be measured exactly as it will
    /// be drawn, which is what the pagination under it needs.
    fn facts_block(&self) -> Screen {
        let Some(story) = self.open.and_then(|index| self.stories.get(index)) else {
            return ScreenBuilder::new("hn-facts").build();
        };
        ScreenBuilder::new("hn-facts")
            .facts(story_facts(story, self.now))
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
        let mut paragraphs = Vec::new();
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
        let ended = !self.more_to_take();
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
        if ended {
            // The end of a conversation is a fact about it, so it is written
            // at the end of it rather than raised as a message over the top
            // of a page that is already full.
            paragraphs.push((0, 0, QuoteRole::Byline, "End of the thread.".to_owned()));
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
        self.measure_skeleton(&*context);
        self.cancel_outstanding(context);
        self.drop_lanes(context);
        self.problem = None;
        self.trouble = None;
        self.ranking.clear();
        self.stories.clear();
        self.pages.clear();
        let Some(url) = self.tab.ranking_url() else {
            self.take_saved(context);
            return;
        };
        match context.spawn_retrying(Task::Fetch {
            url,
            offset: 0,
            max_bytes: RANKING_BYTES,
            credential: None,
            headers: Vec::new(),
        }) {
            Some(task) => self.task = Some((task, Awaiting::Ranking(self.tab))),
            None => self.problem = Some("Too much is already in flight.".to_owned()),
        }
    }

    /// How many rows of a list this panel holds.
    ///
    /// Measured with the engine that will lay the list out, against rows of
    /// the shape this application actually draws, so the placeholder is the
    /// same height as the list that replaces it and nothing jumps when the
    /// stories land.
    fn measure_skeleton(&mut self, context: &Context) {
        const SHAPE: (&str, &str, &str) = (
            "A headline of about the length a story on this site is given",
            "example.com \u{b7} 12 comments \u{b7} 3h ago",
            "214 points",
        );
        let rows = vec![SHAPE; 12];
        let pages =
            context.paginate_ranked_rows_with_trailing(&rows, true, 12, Position::Elsewhere);
        let fits = pages.first().map_or(0, Vec::len);
        self.skeleton = u8::try_from(fits).unwrap_or(SKELETON_ROWS).max(1);
    }

    /// Puts the saved list on the panel, which costs no radio at all.
    ///
    /// The stories were written down whole when they were saved, so this is
    /// the one list that is complete the moment it is asked for, and the only
    /// one that works on a train.
    fn take_saved(&mut self, context: &mut Context) {
        self.stories = self.marks.saved.clone();
        self.now = unix_now();
        self.page = 0;
        self.repaginate_list(&*context);
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
                credential: None,
                headers: Vec::new(),
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
            credential: None,
            headers: Vec::new(),
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
                credential: None,
                headers: Vec::new(),
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

    /// How much of an article is worth carrying onto this device.
    ///
    /// Half a megabyte of markup is a long feature with its pictures still to
    /// come. Past that is a page that is mostly somebody else's javascript,
    /// and the reader is told rather than made to wait for it.
    const ARTICLE_BYTES: u32 = 512 * 1024;

    /// Opens the article behind the story, from the device if it is there.
    ///
    /// A saved copy is read without asking anybody for anything, which is the
    /// whole reason a story is saved at all.
    fn read_link(&mut self, context: &mut Context) {
        let Some(story) = self.open.and_then(|index| self.stories.get(index)) else {
            return;
        };
        let Some(link) = story.link.clone() else {
            // A question or a show-and-tell is its own text, and the text is
            // already on the discussion screen. Nothing to fetch.
            self.say_in_thread(
                context,
                "This story is its own text, which is on this screen.".to_owned(),
            );
            return;
        };
        self.problem = None;
        let saved =
            Snapshot::new(&format!("hn-article:{link}")).at_most(Self::ARTICLE_BYTES as usize);
        saved.start(context);
        self.article = Some(saved);
        self.show(context);
    }

    /// A store answer that belongs to the article being read, or does not.
    ///
    /// Returns whether it was one of those. The pictures are asked first
    /// because they arrive under their own names and answer for themselves.
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

    /// What the device said about its copy of the article.
    fn article_event(&mut self, context: &mut Context, event: Option<SnapshotEvent>) {
        match event {
            Some(SnapshotEvent::Loaded) => {
                let saved = self.article.as_ref().and_then(|saved| saved.bytes.clone());
                match saved {
                    Some(bytes) => self.open_article(context, &bytes),
                    None => self.fetch_article(context),
                }
            }
            Some(SnapshotEvent::Failed) => {
                self.problem = Some("This article could not be kept on the device.".to_owned());
                self.show(context);
            }
            Some(SnapshotEvent::Saved) | None => self.show(context),
        }
    }

    /// Asks the site the story points at for the article itself.
    fn fetch_article(&mut self, context: &mut Context) {
        let Some(link) = self
            .open
            .and_then(|index| self.stories.get(index))
            .and_then(|story| story.link.clone())
        else {
            return;
        };
        match context.spawn(Task::Fetch {
            url: link,
            offset: 0,
            max_bytes: Self::ARTICLE_BYTES,
            credential: None,
            headers: Vec::new(),
        }) {
            Some(task) => self.fetching = Some(task),
            None => self.problem = Some("Too much is already in flight.".to_owned()),
        }
        self.show(context);
    }

    /// Puts the article in front of the reader.
    fn open_article(&mut self, context: &mut Context, body: &[u8]) {
        let link = self
            .open
            .and_then(|index| self.stories.get(index))
            .and_then(|story| story.link.clone())
            .unwrap_or_default();
        let source = String::from_utf8_lossy(body).into_owned();
        self.book.close(context);
        self.illustrations.close(context);
        self.book.open(
            context,
            kobo_doc::html::parse(&source),
            kobo_read::Memory::default(),
        );
        self.illustrations.open(context, &mut self.book, &link);
        self.view = View::Reading;
        self.problem = None;
        self.show(context);
    }

    /// What came back from the site the story points at.
    fn took_article(&mut self, context: &mut Context, outcome: TaskOutcome) {
        match outcome {
            TaskOutcome::Completed(body) => {
                // Kept before it is read, so the second opening costs nothing
                // and works with the radio off.
                if !self
                    .article
                    .as_mut()
                    .is_some_and(|saved| saved.save(context, body.clone()))
                {
                    self.problem =
                        Some("This article is open but not kept on the device.".to_owned());
                }
                self.open_article(context, &body);
            }
            TaskOutcome::Failed(error) => {
                // The article, not the discussion. A site that will not answer
                // says nothing about the thread, which is still here and is
                // what this screen is for. The advice is the SDK's own words
                // for that failure, so a reader is told the same thing here as
                // everywhere else.
                self.say_in_thread(
                    context,
                    format!(
                        "{} The discussion is still here.",
                        Failure::of(error).advice
                    ),
                );
            }
            TaskOutcome::Cancelled => self.show(context),
        }
    }

    /// Closes the article and goes back to the discussion it was reached from.
    fn close_article(&mut self, context: &mut Context) {
        self.illustrations.close(context);
        self.book.close(context);
        self.article = None;
        self.fetching = None;
        self.view = View::Thread;
        self.show(context);
    }

    /// Puts the open story on the saved list, or takes it off again.
    fn save_open(&mut self, context: &mut Context) {
        let Some(story) = self.open.and_then(|index| self.stories.get(index)).cloned() else {
            return;
        };
        self.marks.toggle_saved(&story);
        self.remember(context);
        // No message. The control that was tapped says what happened: it
        // reads Save before and Saved after, which is the acknowledgement,
        // and a banner saying the same thing on a page that is already full
        // is a banner drawn through the edge of the panel.
        self.show(context);
    }

    /// Says something on the discussion screen, where there is room for it.
    ///
    /// In the measured flow rather than over the top of it, and the reader is
    /// carried back to the first page so that what was said is on the page
    /// they are looking at.
    fn say_in_thread(&mut self, context: &mut Context, said: String) {
        self.note = Some(said);
        self.thread_page = 0;
        self.repaginate_thread(&*context);
        self.show(context);
    }

    /// Writes the marks down, once they are known to be the whole of them.
    fn remember(&mut self, context: &mut Context) {
        if !self.marks_known {
            return;
        }
        context.store().save(MARKS, self.marks.encode());
    }

    /// Takes the story whose menu is open off the saved list.
    fn forget_saved(&mut self, context: &mut Context) {
        let Some(story) = self.menu.and_then(|index| self.stories.get(index)).cloned() else {
            return;
        };
        if self.marks.was_saved(&story.id) {
            self.marks.toggle_saved(&story);
            self.remember(context);
        }
        self.menu = None;
        // The list being shown is the saved list, so the row goes with it.
        self.take_saved(context);
        self.show(context);
    }

    fn open_story(&mut self, context: &mut Context, index: usize) {
        self.open = Some(index);
        self.menu = None;
        // Opened is read. Not "read to the end", which nothing here can know,
        // but the same thing the site means when it greys a visited link: you
        // have been here.
        if let Some(story) = self.stories.get(index) {
            let id = story.id.clone();
            self.marks.mark_read(&id);
            self.remember(context);
        }
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
        if self.tab == Tab::Saved {
            self.repaginate_saved(context);
            return;
        }
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
        // The count is in the top bar beside the controls that change it, so
        // no strip is reserved under the list and the page keeps the row that
        // strip was holding. Ranked, because the digits down the left are
        // narrower than the mark column every other list pays for.
        let highest = u16::try_from(rows.len()).unwrap_or(u16::MAX);
        self.pages =
            context.paginate_ranked_rows_with_trailing(&rows, true, highest, Position::Elsewhere);
        self.page = self.page.min(self.pages.len().saturating_sub(1));
    }

    /// The same, for the saved list, whose rows carry a mark instead of a
    /// score.
    ///
    /// Measured with the mark, because it takes a finger's width out of every
    /// title: a saved list paginated as though the rows were full width wraps
    /// its last title and draws it through the tab bar.
    fn repaginate_saved(&mut self, context: &Context) {
        self.titles = self
            .stories
            .iter()
            .map(|story| context.clamped_row_with_menu(&story.title, TITLE_LINES, true))
            .collect();
        let summaries = self
            .stories
            .iter()
            .map(|story| self.row_summary(story))
            .collect::<Vec<_>>();
        let rows = self
            .titles
            .iter()
            .zip(&summaries)
            .map(|(title, summary)| (title.as_str(), summary.as_str()))
            .collect::<Vec<_>>();
        self.pages = context.paginate_rows_with_menu(&rows, true);
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
        // Measured under the facts, which stand on the first page and on no
        // other. They used to be measured as a run of paragraphs of roughly
        // the same height, swapped for the real block while drawing, and
        // roughly the same height is a dozen pixels out over four facts,
        // which is one line too many at the foot of the first page.
        self.thread_pages = context.paginate_tagged_under(&borrowed, false, &self.facts_block());
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
        self.menu = None;
        if tab == Tab::Saved {
            // Nothing to ask anybody for, and nothing to keep from the tab
            // being left: the saved list is on the device and is rebuilt
            // whole every time it is opened, so a story saved from a thread
            // is on it the moment the reader gets back.
            //
            // What is already in the air has to go, though. A ranking that
            // lands after the reader has moved on puts its own tab back on
            // the panel, so tapping Saved while the front page was still
            // arriving showed the saved list for a second and then the front
            // page.
            self.cancel_outstanding(context);
            self.drop_lanes(context);
            self.tab = tab;
            self.problem = None;
            self.trouble = None;
            self.take_saved(context);
            self.show(context);
            return;
        }
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
        // Asked for before the network is: what has been read is what makes
        // the first list drawn look different from a stranger's.
        context.store().load(MARKS);
        self.ask_list(context);
        self.show(context);
    }

    fn on_save(&mut self, context: &mut Context, key: &str, result: StoreResult) {
        self.article_result(context, key, &result);
    }

    /// The piecework of moving an article on or off the device.
    ///
    /// A state file travels in one message; an article does not, so it goes
    /// through the shelf a piece at a time and the snapshot is told about each
    /// piece. Without this the copy is started and never finished, and the
    /// story that was put aside for the train is a story that is not there.
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
        if let StoreResult::Loaded { key, value } = &result {
            if key == MARKS {
                if let Some(text) = value
                    .as_ref()
                    .and_then(|bytes| std::str::from_utf8(bytes).ok())
                {
                    self.marks = model::Marks::decode(text);
                }
                self.marks_known = true;
                if self.tab == Tab::Saved {
                    // The saved list was drawn empty while the file was being
                    // read. Now it is the list.
                    self.take_saved(context);
                }
                self.show(context);
                return;
            }
        }
        // Anything else belongs to the copy of an article kept on the device.
        let key = match &result {
            StoreResult::Loaded { key, .. } | StoreResult::Saved { key } => key.clone(),
            _ => String::new(),
        };
        self.article_result(context, &key, &result);
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        // An open article answers its own page turns and its own controls
        // before anything else looks at the tap.
        if self.view == View::Reading && self.book.memory().is_some() {
            match self.book.act(context, action) {
                Some(kobo_read::Outcome::Close) => self.close_article(context),
                Some(kobo_read::Outcome::Light(level)) => context.device().set_frontlight(level),
                None if action == ActionId::BACK || action == action_id("close-article") => {
                    self.close_article(context);
                }
                _ => {}
            }
            self.show(context);
            return;
        }
        if action == ActionId::BACK {
            // Only ever delivered on a screen that asked for it, so this is
            // always a thread returning to the list it was opened from.
            self.view = View::List;
            self.problem = None;
            self.trouble = None;
            self.menu = None;
            self.show(context);
            return;
        }
        if action == action_id("save") {
            self.save_open(context);
            return;
        }
        if action == action_id("read-article") || action == action_id("close-article") {
            if self.view == View::Reading {
                self.close_article(context);
            } else {
                self.read_link(context);
            }
            return;
        }
        if action == action_id("saved-forget") {
            self.forget_saved(context);
            return;
        }
        for index in 0..self.stories.len() {
            if action == action_id(&format!("saved-menu-{index}")) {
                self.menu = Some(index);
                self.show(context);
                return;
            }
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
        // The open article, its pictures, and the request that fetched it.
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
        if self.fetching == Some(task) {
            self.fetching = None;
            self.took_article(context, outcome);
            return;
        }
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
    /// fetched a run at a time. Where there is nothing more, the last page
    /// already says so in its own last line, so a tap on a page that cannot
    /// turn does nothing rather than raising a message about it.
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
    use super::{model, Awaiting, Hn, Slot, Tab, View, CHUNK, LANES, TABS, TITLE_LINES};
    use kobo_sdk::{action_id, ActionId, AppRunner, Command, Task, TaskError, TaskId, TaskOutcome};
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
    fn a_ranked_list_that_counts_itself_in_the_bar_fills_the_page_it_is_drawn_on() {
        // Two measures were wrong in the same direction and both cost whole
        // rows: the page position lives in the top bar, so the strip under the
        // list is never reserved, and the rows lead with digits, which are
        // narrower than the mark column every other list pays for. Measured as
        // a marked list with a strip, the white under the last story was a
        // story's worth. A page is full when the first story of the next one
        // would not have fitted on it.
        let runner = loaded();
        let context = runner.context();
        let mut application = Hn::default();
        application.now = runner.app().now;
        // Short headlines on purpose: a list of one-line rows is where a row's
        // worth of white at the foot of the page is unmistakable.
        application.stories = (0..30)
            .map(|index| model::Story {
                id: index.to_string(),
                title: format!("A short headline number {index}"),
                author: "someone".into(),
                points: 40 + index,
                comments: 7,
                created: application.now - 3600,
                text: None,
                link: Some(format!("https://example.com/{index}")),
                site: Some("example.com".into()),
            })
            .collect();
        application.repaginate_list(&context);
        assert!(
            application.pages.len() > 2,
            "too few pages to prove anything"
        );

        let drawn = |page: usize| {
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
            let rows = layout
                .nodes
                .iter()
                .filter(|node| matches!(node.kind, LayoutKind::Row(_)))
                .map(|node| (node.rect.y, node.rect.height))
                .collect::<Vec<_>>();
            (layout.content, rows)
        };

        for page in 0..application.pages.len() - 1 {
            let (bounds, rows) = drawn(page);
            assert_eq!(
                rows.len(),
                application.pages[page].len(),
                "page {page} measured {} rows and drew {}",
                application.pages[page].len(),
                rows.len()
            );
            let bottom = rows.last().map_or(0, |(y, height)| y + height);
            // The runtime draws the status strip over the top of the panel and
            // never tells the application, so the page really ends there.
            let floor = bounds.y + bounds.height - CLARA_BW_METRICS.status_band_height();
            let (_, next) = drawn(page + 1);
            let following = next.first().map_or(0, |(_, height)| *height);
            let gap = CLARA_BW_METRICS.space(kobo_ui::Space::Tight) * 2;
            assert!(
                bottom + gap + following > floor,
                "page {page} left room for the story that starts page {}: \
                 {bottom} + {gap} + {following} against {floor}",
                page + 1
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
            // Every paragraph the page was measured to hold is drawn as a
            // quote. The facts are not paragraphs: they are the block the
            // first page was measured under, and they are counted below with
            // everything else that must stay above the foot of the panel.
            let drawn = layout
                .nodes
                .iter()
                .filter(|node| matches!(node.kind, LayoutKind::Quote(..)))
                .count();
            assert_eq!(
                drawn,
                application.thread_pages[page].len(),
                "thread page {page} measured as {} paragraphs but drew {drawn}",
                application.thread_pages[page].len()
            );

            // Drawn is not the same as read. The engine places a paragraph
            // that starts above the fold and lets the rest of it run under the
            // bar, so a page can hold every paragraph it was measured for and
            // still cut the last one off mid-sentence.
            let floor =
                layout.content.y + layout.content.height - CLARA_BW_METRICS.status_band_height();
            let spilling = layout
                .nodes
                .iter()
                .filter(|node| matches!(node.kind, LayoutKind::Quote(..) | LayoutKind::FactValue))
                .filter(|node| node.rect.y + node.rect.height > floor)
                .count();
            assert_eq!(
                spilling, 0,
                "thread page {page} ran {spilling} paragraphs under the bar"
            );
        }
    }

    #[test]
    fn a_thread_turns_its_pages_from_the_strip_that_says_which_one_it_is_on() {
        // The bar said "Back / Stories / Next": Stories is the chevron already
        // in the top bar, and the other two are the page turns. Three labels
        // for two things the reader had, across a bar's worth of the page.
        let (runner, _) = opened_thread();
        let mut application = Hn::default();
        application.stories.clone_from(&runner.app().stories);
        application
            .thread_pages
            .clone_from(&runner.app().thread_pages);
        application.open = runner.app().open;
        assert!(
            application.thread_pages.len() > 1,
            "a whole thread fitted one page, so this proves nothing"
        );

        let controls = |page: usize| {
            let showing = Hn {
                thread_page: page,
                open: application.open,
                stories: application.stories.clone(),
                thread_pages: application.thread_pages.clone(),
                ..Hn::default()
            };
            let layout = showing
                .thread()
                .layout_with(&CLARA_BW_METRICS, &Chrome::default());
            assert!(
                !layout
                    .nodes
                    .iter()
                    .any(|node| matches!(node.kind, LayoutKind::NavDestination(..))),
                "the thread grew a bottom bar again"
            );
            let mut reachable = Vec::new();
            for node in &layout.nodes {
                if let LayoutKind::PagePrevious(action) | LayoutKind::PageNext(action) = node.kind {
                    let hit = layout.hit_test(
                        node.rect.x + node.rect.width / 2,
                        node.rect.y + node.rect.height / 2,
                    );
                    assert_eq!(hit, Some(action), "a page turn missed its own centre");
                    reachable.push(action);
                }
            }
            reachable
        };

        // The first page offers only the way forward, the last only the way
        // back, and every page in between offers both.
        let last = application.thread_pages.len() - 1;
        assert_eq!(controls(0), vec![action_id("thread-next")]);
        assert_eq!(controls(last), vec![action_id("thread-back")]);
        if last > 1 {
            assert_eq!(
                controls(1),
                vec![action_id("thread-back"), action_id("thread-next")]
            );
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
                link: None,
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
    fn a_reply_deeper_than_the_gutter_can_show_says_how_deep_it_is() {
        // Past the indent cap every level is drawn at the same offset, so a
        // reader eight replies in sees the same gutter as one two replies in.
        // The byline is where the difference goes, because it costs no width.
        let capped = u16::from(model::MAX_INDENT);
        let deep = a_thread_of(&[0, capped, capped + 3]);
        let bylines = deep
            .thread_paragraphs()
            .into_iter()
            .filter(|(_, _, role, _)| *role == kobo_sdk::QuoteRole::Byline)
            .map(|(_, depth, _, text)| (depth, text))
            .collect::<Vec<_>>();
        let (shallow_depth, shallow) = &bylines[0];
        let (capped_depth, at_the_cap) = &bylines[1];
        let (past_depth, past_the_cap) = &bylines[2];
        assert_eq!(*shallow_depth, 0);
        assert!(!shallow.contains("deep"), "{shallow:?}");
        assert!(!at_the_cap.contains("deep"), "{at_the_cap:?}");
        assert_eq!(
            capped_depth, past_depth,
            "past the cap the drawn indent stops moving, which is the point"
        );
        assert!(
            past_the_cap.contains(&format!("reply {} deep", capped + 3)),
            "a reply past the indent cap did not say how deep it was: {past_the_cap:?}"
        );
    }

    #[test]
    fn a_folded_comment_can_be_opened_again_and_brings_its_replies_back() {
        // The round trip, rather than the two halves of it separately: a fold
        // that cannot be undone is a comment the reader has thrown away.
        // Not started: `on_start` goes after the front page, which would
        // throw this thread away before the first tap.
        let mut runner = AppRunner::new(a_thread_of(&[0, 1, 2, 0]));
        let whole = runner.app().thread_paragraphs().len();
        runner.action(action_id("fold-0"));
        let folded = runner.app().thread_paragraphs().len();
        assert!(
            folded < whole,
            "folding the first comment hid nothing: {folded} of {whole}"
        );
        assert!(runner.app().collapsed.contains(&0));
        runner.action(action_id("fold-0"));
        assert_eq!(
            runner.app().thread_paragraphs().len(),
            whole,
            "opening a folded comment did not bring its replies back"
        );
        assert!(runner.app().collapsed.is_empty());
    }

    #[test]
    fn the_discussion_says_the_headline_once() {
        // The headline used to be in the top bar and in the first paragraph,
        // which is the same words twice on one panel with one of them cut
        // short, and it cost the first page a headline's worth of comments.
        let (runner, _) = opened_thread();
        let application = runner.app();
        let title = application
            .open
            .and_then(|index| application.stories.get(index))
            .map(|story| story.title.clone())
            .expect("a story is open");
        let repeated = application
            .thread_paragraphs()
            .into_iter()
            .filter(|(_, _, _, text)| text.contains(&title))
            .count();
        assert_eq!(repeated, 0, "the headline is in the bar, and only there");
        let screen = application.thread();
        assert_eq!(
            screen.top_bar.as_ref().map(|bar| bar.title.clone()),
            Some(title),
            "the bar is where the headline has to be: it survives page turns"
        );
    }

    #[test]
    fn a_story_that_was_opened_is_marked_read_on_the_list_it_was_opened_from() {
        let mut runner = loaded();
        let page = runner.app().page;
        let id = runner.app().stories[0].id.clone();
        runner.action(action_id("story-0"));
        assert!(runner.app().marks.was_read(&id));
        // Back to the list, on the page it was left on. A reader four pages
        // into the front page who opens a story and comes back to the top has
        // lost their place, which on a list of thirty is most of it.
        runner.action(ActionId::BACK);
        assert_eq!(runner.app().view, View::List);
        assert_eq!(runner.app().page, page);
        let summary = runner.app().row_summary(&runner.app().stories[0]);
        assert!(summary.starts_with("Read \u{b7} "), "{summary:?}");
    }

    #[test]
    fn saving_a_story_puts_it_on_the_saved_list_and_saying_so_again_takes_it_off() {
        let mut runner = loaded();
        runner.action(action_id("story-0"));
        let id = runner.app().stories[0].id.clone();
        runner.action(action_id("save"));
        assert!(runner.app().marks.was_saved(&id));
        runner.action(action_id("save"));
        assert!(!runner.app().marks.was_saved(&id));
    }

    #[test]
    fn a_list_that_lands_after_the_reader_moved_on_does_not_put_itself_back() {
        // Tapping Saved while the front page was still arriving showed the
        // saved list, and then the front page a second later when the ranking
        // landed and set its own tab back.
        let mut runner = AppRunner::new(Hn::default());
        runner.start();
        let task = spawned(&runner);
        runner.action(action_id("tab-saved"));
        runner.task_outcome(task, TaskOutcome::Completed(RANKING.as_bytes().to_vec()));
        assert_eq!(runner.app().tab, Tab::Saved);
        assert!(runner.app().stories.is_empty());
    }

    #[test]
    fn the_saved_list_needs_no_network_at_all() {
        // The whole reason for saving: a list that is already on the device
        // is a list that works on a train. Nothing may be asked for.
        let mut runner = loaded();
        runner.action(action_id("story-0"));
        runner.action(action_id("save"));
        let before = runner.app().stories[0].title.clone();
        let commands = runner.action(action_id("tab-saved"));
        assert!(
            asked(&commands).is_none(),
            "opening the saved list asked the network for something"
        );
        assert_eq!(runner.app().tab, Tab::Saved);
        assert_eq!(runner.app().stories.len(), 1);
        assert_eq!(runner.app().stories[0].title, before);
    }

    #[test]
    fn a_story_with_no_link_of_its_own_says_so_rather_than_fetching_nothing() {
        // An Ask HN post is its own text. Offering to fetch an article that
        // does not exist is a request that can only fail, so the screen says
        // what is true instead and the discussion stays where it is.
        let mut application = a_thread_of(&[0]);
        application.stories[0].link = None;
        application.view = View::Thread;
        let mut runner = AppRunner::new(application);
        let commands = runner.action(action_id("read-article"));
        assert!(asked(&commands).is_none(), "it went looking for an article");
        assert_eq!(runner.app().view, View::Thread);
        let said = runner.app().note.clone().unwrap_or_default();
        assert!(said.contains("its own text"), "{said:?}");
        // And the bar does not offer what it cannot do.
        let screen = runner.app().thread();
        let bar = format!("{:?}", screen.top_bar);
        assert!(!bar.contains("read-article"), "{bar}");
    }

    #[test]
    fn a_long_headline_is_cut_to_the_rows_it_has_and_kept_whole_everywhere_else() {
        let runner = loaded();
        let application = runner.app();
        let context = runner.context();
        let long = "A headline of the length the site allows, which is eighty characters \
                    and change, and rather more than a six inch panel will hold on two lines";
        let clamped = context.clamped_row_beside(long, "214 points", TITLE_LINES, true);
        assert!(clamped.len() < long.len(), "{clamped:?}");
        assert!(clamped.ends_with('\u{2026}'), "{clamped:?}");
        // Cut on the list, whole in the bar of its own screen, where the
        // runtime does the cutting and the reader can still see the rest by
        // opening the article.
        assert!(application.titles.iter().all(|title| !title.contains('\n')));
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

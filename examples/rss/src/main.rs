//! Feeds: the sites you read, on the device.
//!
//! Type an address, pick the feed it finds, and read the articles without
//! leaving the application.
//!
//! ## Why a search service rather than guessing the address
//!
//! Almost nobody knows the address of a site's feed. They know the address of
//! the site. Turning one into the other means fetching the page, parsing its
//! HTML, reading `<link rel="alternate">`, then trying `/feed`, `/rss.xml`,
//! `/atom.xml` and a dozen more: several round trips over a radio that costs
//! battery, and an HTML parser aimed at whole pages rather than fragments.
//!
//! [Feedsearch](https://feedsearch.dev) does that work once, server-side, and
//! has done it before for most sites anybody types. One request returns every
//! feed a domain has, already ranked. That is the whole reason this
//! application can be a few hundred lines rather than a browser.
//!
//! Their terms ask for a visible attribution wherever their results are shown,
//! which is on both the search screen and the results screen below.
//!
//! ## Why the articles are read from the feed and not from the site
//!
//! Because the feed is the readable copy. Most publishers put the whole post
//! in `content:encoded`, and the ones that do not put a summary there. Either
//! way it is prose with a little markup, which is exactly what an E Ink panel
//! wants. Following the link instead would mean fetching a modern web page, a
//! megabyte of layout, script and advertising wrapped around the same words
//! this application already has.
//!
//! ## Why subscriptions are stored and articles are not
//!
//! A subscription is a hundred bytes and is the thing the reader chose. A
//! feed's articles are tens of kilobytes, are replaced by the publisher
//! whenever they like, and are cheap to fetch again. Storing the first costs
//! nothing and loses nothing; storing the second would spend the store's whole
//! budget on a copy that is wrong by the next morning.

mod feed;
mod search;

use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Failure, Glyph, KoboApp, LogLevel, Screen,
    ScreenBuilder, StoreResult, Task, TaskId, TaskOutcome,
};
use std::process::ExitCode;

/// The most feeds one reader may follow.
///
/// Not a storage limit, the whole list is one value of a few kilobytes. It is
/// a limit on how long a list can get before finding anything in it means
/// turning pages, at which point the application needs folders, and folders
/// are a different application.
const MAX_FEEDS: usize = 40;

/// The key the subscription list is stored under.
const FEEDS: &str = "feeds";

/// How much of a search answer to accept.
///
/// This was set at a dozen feeds and twenty kilobytes, which is what a blog
/// or a magazine answers with. A national newspaper is not that shape: the
/// New York Times publishes a feed per section and answers in a hundred and
/// fifty kilobytes across two hundred of them, so the cap refused the one
/// site most people would try first.
///
/// So it is the runtime's own ceiling now, the same one a feed itself gets.
/// There is nothing to be gained by refusing an answer the runtime was
/// willing to carry.
const SEARCH_BYTES: u32 = 512 * 1024;

/// How much of a feed to accept.
///
/// A feed carrying fifty full articles is a few hundred kilobytes at the top
/// end, and the largest this can ask for either way is the runtime's own
/// [`kobo_sdk::MAX_TASK_BYTES`].
///
/// Past this the answer is truncated rather than refused, and what that costs
/// depends on the format. A cut XML feed keeps every item that arrived whole
/// (which is the recent ones, because feeds are written newest first) and that
/// is measured, not assumed. A cut JSON feed yields nothing at all: half a
/// JSON document is not a JSON document, and there is no prefix of one to
/// recover. So a feed that will not parse at exactly this length is reported
/// as too large rather than as not a feed.
const FEED_BYTES: u32 = 512 * 1024;

/// Whether an answer arrived at its budget, and so was probably cut short.
///
/// A body that is exactly the number of bytes asked for is one the far end had
/// more of. It could be a feed that happens to be that length to the byte,
/// which is why this only ever changes the wording of a failure and never
/// discards an answer that parsed.
fn truncated(bytes: &[u8], budget: u32) -> bool {
    bytes.len() >= budget as usize
}

/// The attribution Feedsearch's terms ask for, on the screen where there is
/// room for the whole sentence.
///
/// The results screen carries it in its top bar instead. Both screens show
/// their results because of Feedsearch, and both have to say so.
const ATTRIBUTION: &str = "Feed search powered by feedsearch.dev";

/// A feed the reader has chosen to follow.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Subscription {
    url: String,
    title: String,
    site: String,
}

/// Which screen is in front of the reader.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum View {
    /// The feeds being followed.
    #[default]
    Shelf,
    /// Typing an address.
    Search,
    /// What the search found.
    Found,
    /// One feed's articles.
    Items,
    /// One article.
    Reading,
}

/// What the one outstanding request is for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Awaiting {
    Search,
    Feed,
}

#[derive(Default)]
struct Feeds {
    view: View,
    /// The subscription list, as stored.
    subscriptions: Vec<Subscription>,
    /// False until the store has answered once, so that an empty list is not
    /// mistaken for a reader who follows nothing.
    loaded: bool,
    keyboard: Keyboard,
    /// What was typed, kept to caption the results screen.
    query: String,
    /// What the search found, best first.
    found: Vec<search::Found>,
    /// Which subscription is open.
    open: Option<usize>,
    /// The open feed's articles.
    items: Vec<feed::Item>,
    /// Which article is being read.
    article: Option<usize>,
    /// The article, cut into pages that fit the panel.
    pages: Vec<Vec<String>>,
    page: usize,
    /// Which page of a list is showing. Shared by the shelf and the articles,
    /// because only one of them is ever on screen.
    list_page: usize,
    task: Option<(TaskId, Awaiting)>,
    problem: Option<String>,
    /// The last task failure as the SDK read it. An empty article list wants
    /// the whole-screen version of it; a list with articles wants the banner.
    trouble: Option<Failure>,
    /// Which feed's overflow menu is open, if any. An index into
    /// `subscriptions` rather than a page position, so turning a page or
    /// removing an earlier feed cannot leave it pointing at the wrong one.
    menu_open: Option<usize>,
}

impl Feeds {
    fn awaiting(&self, what: Awaiting) -> bool {
        matches!(self.task, Some((_, outstanding)) if outstanding == what)
    }

    /// Writes the subscription list back. Called after every change.
    fn save(&mut self, context: &mut Context) {
        let bytes = encode(&self.subscriptions);
        context.store().save(FEEDS, bytes);
    }

    /// Asks Feedsearch what feeds an address has.
    fn ask_search(&mut self, context: &mut Context, url: &str) {
        self.found.clear();
        self.problem = None;
        self.trouble = None;
        let request = search::request(url);
        match context.spawn_retrying(Task::Fetch {
            url: request,
            offset: 0,
            max_bytes: SEARCH_BYTES,
            headers: Vec::new(),
        }) {
            Some(task) => self.task = Some((task, Awaiting::Search)),
            None => self.problem = Some("The device is busy. Try that again.".to_owned()),
        }
    }

    /// Fetches the open feed.
    fn ask_feed(&mut self, context: &mut Context) {
        let Some(subscription) = self.open.and_then(|index| self.subscriptions.get(index)) else {
            return;
        };
        let url = subscription.url.clone();
        self.items.clear();
        self.problem = None;
        self.trouble = None;
        match context.spawn_retrying(Task::Fetch {
            url,
            offset: 0,
            max_bytes: FEED_BYTES,
            headers: Vec::new(),
        }) {
            Some(task) => self.task = Some((task, Awaiting::Feed)),
            None => self.problem = Some("The device is busy. Try that again.".to_owned()),
        }
    }

    /// Follows a feed, unless it is already followed.
    ///
    /// Returns where it sits in the list either way, so that choosing
    /// something already subscribed opens it rather than refusing.
    fn subscribe(&mut self, found: &search::Found) -> Option<usize> {
        if let Some(index) = self
            .subscriptions
            .iter()
            .position(|feed| feed.url == found.url)
        {
            return Some(index);
        }
        if self.subscriptions.len() >= MAX_FEEDS {
            self.problem = Some(format!(
                "That is {MAX_FEEDS} feeds, which is as many as this holds. \
                 Remove one first."
            ));
            return None;
        }
        self.subscriptions.push(Subscription {
            url: found.url.clone(),
            title: found.title.clone(),
            site: found.site.clone(),
        });
        Some(self.subscriptions.len() - 1)
    }

    /// Cuts the open article into pages that fit the panel.
    fn lay_out(&mut self, context: &Context) {
        let Some(item) = self.article.and_then(|index| self.items.get(index)) else {
            self.pages = Vec::new();
            return;
        };
        // No bar: a reading page carries nothing at its foot but the place it
        // is at. Reserving one leaves a hand's width of white above the
        // position and takes four lines off every page.
        self.pages = context.paginate_reading(&article_text(item), false);
        self.page = 0;
    }

    fn show(&mut self, context: &mut Context) {
        let screen = match self.view {
            View::Shelf => self.shelf(context),
            View::Search => self.search(),
            View::Found => self.results(context),
            View::Items => self.articles(context),
            View::Reading => self.reading(),
        };
        // Every view except the shelf was reached from another one, so Back
        // unwinds this application first and leaves it only from the shelf.
        // Without this, Back out of an article lands at the launcher.
        context
            .set_screen(screen.with_own_back(self.view != View::Shelf || self.menu_open.is_some()));
    }

    fn shelf(&self, context: &Context) -> Screen {
        let mut screen = ScreenBuilder::new("rss-shelf").top_bar("Feeds");
        if let Some(problem) = &self.problem {
            screen = screen.banner(BannerLevel::Attention, problem.clone());
        }
        if !self.loaded {
            return screen.activity("Opening your feeds", None).build();
        }
        if self.subscriptions.is_empty() {
            // Centred under a mark rather than ranged left at the top: this
            // is the first screen anybody sees, and a lone paragraph in the
            // corner of a 1448-pixel panel reads as a page that failed.
            return screen
                .splash(
                    Some(Glyph::Rss),
                    "No feeds yet",
                    "Follow a site and its new articles arrive here, \
                     ready to read without a browser.",
                )
                .primary_button("add", "Add a feed")
                .build();
        }
        // Clamped against the narrower column the overflow mark leaves, or
        // the longest titles run under the dots.
        let rows: Vec<(String, String)> = self
            .subscriptions
            .iter()
            .map(|feed| {
                let title = context.one_line_row_with_menu(&feed.title, true);
                let summary =
                    context.one_line_row_with_menu(&pretty_host(&feed.site, &feed.url), true);
                (title, summary)
            })
            .collect();
        let pages = page_groups(context, &rows, true, true);
        let page = self.list_page.min(pages.len().saturating_sub(1));
        let shown = pages.get(page).cloned().unwrap_or_default();
        screen = screen.rows_with_menu(shown.iter().map(|index| {
            (
                format!("feed-{index}"),
                rows[*index].0.clone(),
                rows[*index].1.clone(),
                Glyph::Rss,
                format!("feed-menu-{index}"),
            )
        }));
        // The menu hangs off the mark that opened it, and only while that mark
        // is on the panel: a page turn with one open would anchor a popover to
        // a control that is no longer drawn.
        if let Some(open) = self.menu_open.filter(|open| shown.contains(open)) {
            screen = screen.row_overflow(
                format!("feed-menu-{open}"),
                true,
                [("feed-forget", "Delete", Glyph::Trash)],
            );
        }
        if pages.len() <= 1 {
            return screen
                .bottom_action_marked("add", "Add a feed", Glyph::Plus)
                .build();
        }
        // Adding a feed is the verb; the page turns are the sides of the panel,
        // not two more buttons beside it. They rode in an action bar together
        // before, which read as three things to do when one of them was a place
        // to do it and the other two were only how to reach the rest of it.
        screen
            .page_turns("list-back", "list-next")
            .page_position(page_number(page), page_total(pages.len()))
            .bottom_action_marked("add", "Add a feed", Glyph::Plus)
            .build()
    }

    fn search(&self) -> Screen {
        let mut screen = ScreenBuilder::new("rss-search").top_bar("Add a feed");
        if let Some(problem) = &self.problem {
            screen = screen.banner(BannerLevel::Attention, problem.clone());
        }
        screen
            .typed(&self.keyboard, "A site, such as arstechnica.com")
            .secondary(ATTRIBUTION)
            .keyboard(&self.keyboard, "Search")
            .build()
    }

    fn results(&self, context: &Context) -> Screen {
        // The attribution lives in the top bar rather than under the list.
        // Feedsearch's terms ask for it to be visible wherever their results
        // are shown, and anything in the flow below a full page of rows is the
        // first thing the panel drops, silently, so the one element that is
        // not optional would be the one element missing. The bar is drawn
        // before the content and cannot be pushed off it.
        let mut screen = ScreenBuilder::new("rss-found").top_bar("Feeds via feedsearch.dev");
        if let Some(problem) = &self.problem {
            screen = screen.banner(BannerLevel::Attention, problem.clone());
        }
        if self.awaiting(Awaiting::Search) {
            return screen
                .divider()
                .activity(format!("Looking for feeds at {}", self.query), None)
                .skeleton(4)
                .build();
        }
        if self.found.is_empty() {
            return screen
                .empty_state(
                    "No feeds there. Some sites publish one at a different \
                     address, so it is worth trying the exact page you read.",
                )
                .primary_button("add", "Try another address")
                .build();
        }
        let rows: Vec<(String, String)> = self
            .found
            .iter()
            .map(|found| {
                (
                    context.one_line_row(&found.title, true),
                    context.one_line_row(&found.summary, true),
                )
            })
            .collect();
        let pages = page_groups(context, &rows, false, true);
        let page = self.list_page.min(pages.len().saturating_sub(1));
        let shown = pages.get(page).cloned().unwrap_or_default();
        screen = screen.rows(shown.iter().map(|index| {
            (
                format!("found-{index}"),
                rows[*index].0.clone(),
                rows[*index].1.clone(),
                Glyph::Rss,
            )
        }));
        if pages.len() <= 1 {
            return screen.build();
        }
        // The page turns are the sides of the panel, and the bar carries the
        // one verb: a bar reading Back, Search, More was two page turns
        // dressed as somewhere to go.
        screen
            .page_turns("list-back", "list-next")
            .page_position(page_number(page), page_total(pages.len()))
            .bottom_action_marked("add", "Search", Glyph::Search)
            .build()
    }

    fn articles(&self, context: &Context) -> Screen {
        let title = self
            .open
            .and_then(|index| self.subscriptions.get(index))
            .map_or_else(|| "Feed".to_owned(), |feed| feed.title.clone());
        let mut screen = ScreenBuilder::new("rss-items")
            .top_bar(context.one_line_row(&title, false))
            .top_bar_glyph("remove", "Unfollow", Glyph::Trash)
            // Fetching again is the one thing done here often enough to earn a
            // glyph rather than a word: the feed is read on demand, so a reader
            // catching up taps this on every feed they open. The two arrows say
            // it in the width a caption of "Refresh" wanted, which is what left
            // room for it to sit beside Unfollow inside the bar's two places.
            .top_bar_glyph("refresh", "Refresh", Glyph::Refresh);
        if let Some(problem) = &self.problem {
            screen = screen.banner(BannerLevel::Attention, problem.clone());
        }
        if self.awaiting(Awaiting::Feed) {
            return screen
                .divider()
                .activity("Fetching the latest articles", None)
                .skeleton(6)
                .build();
        }
        if self.items.is_empty() {
            // A feed that failed and a feed that published nothing are not the
            // same thing, and saying "Nothing published yet" about a reader who
            // is simply offline is a lie the SDK can avoid.
            if let Some(failure) = self.trouble {
                return screen.failure_state(failure, "refresh").build();
            }
            return screen
                .empty_state("Nothing published yet.")
                .primary_button("refresh", "Check again")
                .build();
        }
        let rows: Vec<(String, String)> = self
            .items
            .iter()
            .map(|item| {
                (
                    context.clamped_row(&item.title, 2, true),
                    context.one_line_row(&byline(item), true),
                )
            })
            .collect();
        let pages = page_groups(context, &rows, false, false);
        let page = self.list_page.min(pages.len().saturating_sub(1));
        let shown = pages.get(page).cloned().unwrap_or_default();
        screen = screen.rows(shown.iter().map(|index| {
            (
                format!("item-{index}"),
                rows[*index].0.clone(),
                rows[*index].1.clone(),
                // Numbered rather than a glyph: forty identical marks down the
                // side of a list say nothing, and the number is how somebody
                // finds their place again after putting the device down.
                u16::try_from(index + 1).unwrap_or(u16::MAX),
            )
        }));
        if pages.len() <= 1 {
            return screen.build();
        }
        // Paging is the sides of the panel, not a row of buttons: the refresh
        // verb moved to the top bar, and Back and More were only ever the page
        // turns wearing an action bar's clothes -- which is the confusion this
        // application was asked to stop making, a bar of verbs is not a bar of
        // somewhere-to-go.
        screen
            .page_turns("list-back", "list-next")
            .page_position(page_number(page), page_total(pages.len()))
            .build()
    }

    fn reading(&self) -> Screen {
        let title = self
            .article
            .and_then(|index| self.items.get(index))
            .map_or_else(String::new, |item| item.title.clone());
        let mut screen = ScreenBuilder::new("rss-reading")
            .top_bar(title)
            .reading(true);
        if self.pages.is_empty() {
            return screen.empty_state("This article arrived empty.").build();
        }
        let page = self.page.min(self.pages.len() - 1);
        for paragraph in &self.pages[page] {
            screen = screen.text(paragraph.clone());
        }
        screen
            .page_turns("page-back", "page-next")
            .page_position(page_number(page), page_total(self.pages.len()))
            .build()
    }
}

/// A page number the position band can carry, one based and clamped.
fn page_number(page: usize) -> u16 {
    u16::try_from(page.saturating_add(1)).unwrap_or(u16::MAX)
}

/// How many pages there are, clamped. Not `page_number`: a count is already
/// one based, and putting a page through the wrong one of these says "1 of 3"
/// about a list of two pages.
fn page_total(pages: usize) -> u16 {
    u16::try_from(pages).unwrap_or(u16::MAX)
}

/// How a list of rows is grouped into pages.
///
/// `menu` and `nav_bar` have to say what the screen actually draws. Measuring
/// a list of plain rows as though every one of them carried an overflow mark
/// takes a finger's width off the title column, which wraps titles that would
/// not have wrapped and makes every row taller than the one drawn: the article
/// list came back four rows to a page with a third of the panel left white
/// under them. Reserving a bottom bar that is not there costs another row the
/// same way.
fn page_groups(
    context: &Context,
    rows: &[(String, String)],
    menu: bool,
    nav_bar: bool,
) -> Vec<Vec<usize>> {
    let borrowed: Vec<(&str, &str)> = rows
        .iter()
        .map(|(title, summary)| (title.as_str(), summary.as_str()))
        .collect();
    let pages = if menu {
        context.paginate_rows_with_menu(&borrowed, nav_bar)
    } else {
        context.paginate_rows(&borrowed, nav_bar)
    };
    if pages.is_empty() {
        vec![Vec::new()]
    } else {
        pages
    }
}

/// The line under an article's title.
fn byline(item: &feed::Item) -> String {
    let date = item.short_date();
    match (item.author.trim(), date.as_str()) {
        ("", "") => first_words(&item.body),
        ("", date) => date.to_owned(),
        (author, "") => author.to_owned(),
        (author, date) => format!("{author} \u{00b7} {date}"),
    }
}

/// The opening of a body, for an item that says nothing else about itself.
fn first_words(body: &str) -> String {
    body.split_whitespace()
        .take(14)
        .collect::<Vec<_>>()
        .join(" ")
}

/// The whole article as one piece of prose, ready to be cut into pages.
fn article_text(item: &feed::Item) -> String {
    let mut text = String::new();
    let byline = byline(item);
    if !byline.is_empty() {
        text.push_str(&byline);
        text.push_str("\n\n");
    }
    text.push_str(item.body.trim());
    if !item.link.trim().is_empty() {
        // The address, plainly, at the end. There is no browser to hand it to,
        // but somebody reading on the sofa often wants to open it on a phone,
        // and a link they cannot see is a link they cannot type.
        text.push_str("\n\n");
        text.push_str(item.link.trim());
    }
    text
}

/// The host, for a line under a feed's name.
///
/// Falls back to the feed's own address when it did not name its site, and to
/// the raw string when there is no host to find, because something recognisable
/// is worth more here than something well-formed.
fn pretty_host(site: &str, url: &str) -> String {
    let source = if site.trim().is_empty() { url } else { site };
    let trimmed = source
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("www.");
    trimmed
        .split('/')
        .next()
        .filter(|host| !host.is_empty())
        .unwrap_or(trimmed)
        .to_owned()
}

/// The subscription list, as bytes.
///
/// One feed per line, three tab-separated fields. Chosen over JSON because the
/// data is three strings with no structure to describe, and over a binary
/// format because a list somebody can read in a hex dump is a list somebody
/// can recover by hand if this application ever writes it wrongly.
fn encode(feeds: &[Subscription]) -> Vec<u8> {
    let mut out = String::new();
    for feed in feeds {
        // Separators are removed rather than escaped. A tab inside a feed title
        // is a typographical accident, and losing it is invisible; a scheme for
        // escaping it would be code that runs for every reader to preserve
        // something no reader would notice.
        out.push_str(&clean(&feed.url));
        out.push('\t');
        out.push_str(&clean(&feed.title));
        out.push('\t');
        out.push_str(&clean(&feed.site));
        out.push('\n');
    }
    out.into_bytes()
}

fn clean(field: &str) -> String {
    field.replace(['\t', '\n', '\r'], " ").trim().to_owned()
}

/// Reads the subscription list back, keeping whatever lines make sense.
fn decode(bytes: &[u8]) -> Vec<Subscription> {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let url = fields.next().unwrap_or_default().trim();
            if url.is_empty() {
                return None;
            }
            let title = fields.next().unwrap_or_default().trim();
            let site = fields.next().unwrap_or_default().trim();
            Some(Subscription {
                url: url.to_owned(),
                title: if title.is_empty() {
                    pretty_host(site, url)
                } else {
                    title.to_owned()
                },
                site: site.to_owned(),
            })
        })
        .take(MAX_FEEDS)
        .collect()
}

/// The index in a `prefix-N` action name, if that is what this is.
fn indexed(action: ActionId, prefix: &str, count: usize) -> Option<usize> {
    (0..count).find(|index| action_id(&format!("{prefix}-{index}")) == action)
}

impl KoboApp for Feeds {
    fn on_start(&mut self, context: &mut Context) {
        context.store().load(FEEDS);
        self.show(context);
    }

    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        match result {
            StoreResult::Loaded { value, .. } => {
                self.subscriptions = value.map(|bytes| decode(&bytes)).unwrap_or_default();
                self.loaded = true;
                self.show(context);
            }
            // A list that could not be written is a list the reader will lose,
            // and they should hear about it while they can still write it down.
            StoreResult::Denied(reason) => {
                self.loaded = true;
                context.log(
                    LogLevel::Warn,
                    format!("the feed list could not be saved: {reason}"),
                );
                self.problem = Some("Your feeds could not be saved.".to_owned());
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

    #[allow(clippy::too_many_lines)]
    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        // The keyboard first: while the search screen is up, it owns the panel.
        if self.view == View::Search {
            match self.keyboard.press(action) {
                Some(Pressed::Submitted) => {
                    let typed = self.keyboard.take().trim().to_owned();
                    if typed.is_empty() {
                        return;
                    }
                    self.query.clone_from(&typed);
                    self.view = View::Found;
                    self.list_page = 0;
                    self.ask_search(context, &typed);
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

        // An open menu takes Back before the view does: the scrim beside a
        // popover sends Back, and on the shelf that would otherwise leave the
        // application entirely.
        if action == ActionId::BACK && self.menu_open.is_some() {
            self.menu_open = None;
            self.show(context);
            return;
        }

        if action == ActionId::BACK {
            self.problem = None;
            self.trouble = None;
            self.menu_open = None;
            match self.view {
                View::Shelf => {}
                View::Search | View::Items => {
                    self.view = View::Shelf;
                    self.list_page = 0;
                }
                View::Found => self.view = View::Search,
                View::Reading => {
                    self.view = View::Items;
                    self.article = None;
                }
            }
            self.show(context);
            return;
        }

        if action == action_id("add") {
            self.keyboard.clear();
            self.problem = None;
            self.trouble = None;
            self.view = View::Search;
            self.show(context);
            return;
        }

        if action == action_id("feed-forget") {
            if let Some(index) = self.menu_open.take() {
                if index < self.subscriptions.len() {
                    self.subscriptions.remove(index);
                    self.save(context);
                }
                // The open feed is named by position, so removing one before
                // it would leave it pointing at its neighbour.
                self.open = match self.open {
                    Some(open) if open == index => None,
                    Some(open) if open > index => Some(open - 1),
                    open => open,
                };
                self.list_page = 0;
            }
            self.show(context);
            return;
        }

        if action == action_id("refresh") {
            self.list_page = 0;
            self.ask_feed(context);
            self.show(context);
            return;
        }

        if action == action_id("remove") {
            if let Some(index) = self.open.take() {
                if index < self.subscriptions.len() {
                    self.subscriptions.remove(index);
                    self.save(context);
                }
            }
            self.items.clear();
            self.list_page = 0;
            self.view = View::Shelf;
            self.show(context);
            return;
        }

        if action == action_id("list-back") {
            self.list_page = self.list_page.saturating_sub(1);
            self.show(context);
            return;
        }

        if action == action_id("list-next") {
            self.list_page += 1;
            self.show(context);
            return;
        }

        if action == action_id("page-back") {
            self.page = self.page.saturating_sub(1);
            self.show(context);
            return;
        }

        if action == action_id("page-next") {
            if self.page + 1 < self.pages.len() {
                self.page += 1;
            }
            self.show(context);
            return;
        }

        if self.view == View::Found {
            if let Some(index) = indexed(action, "found", self.found.len()) {
                let Some(found) = self.found.get(index).cloned() else {
                    return;
                };
                if let Some(position) = self.subscribe(&found) {
                    self.save(context);
                    self.open = Some(position);
                    self.list_page = 0;
                    self.view = View::Items;
                    self.ask_feed(context);
                }
                self.show(context);
                return;
            }
        }

        if self.view == View::Shelf {
            if let Some(index) = indexed(action, "feed-menu", self.subscriptions.len()) {
                self.menu_open = Some(index);
                self.show(context);
                return;
            }
            if let Some(index) = indexed(action, "feed", self.subscriptions.len()) {
                self.menu_open = None;
                self.open = Some(index);
                self.list_page = 0;
                self.view = View::Items;
                self.ask_feed(context);
                self.show(context);
                return;
            }
        }

        if self.view == View::Items {
            if let Some(index) = indexed(action, "item", self.items.len()) {
                self.article = Some(index);
                self.view = View::Reading;
                self.lay_out(context);
                self.show(context);
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
                Awaiting::Search => {
                    self.found = search::results(&bytes);
                    if self.found.is_empty() {
                        // A search answer is JSON, and JSON that stops halfway
                        // is not JSON at all, so a cut answer yields nothing
                        // and looks exactly like a site with no feeds.
                        self.problem = truncated(&bytes, SEARCH_BYTES)
                            .then(|| "That site's answer was too large to read.".to_owned());
                    }
                }
                Awaiting::Feed => match feed::parse(&bytes) {
                    Some(parsed) => {
                        self.items = parsed.items;
                        // A feed usually names itself better than a search
                        // result does, so the shelf takes the better name once
                        // it has been read.
                        if let Some(subscription) = self
                            .open
                            .and_then(|index| self.subscriptions.get_mut(index))
                        {
                            if !parsed.title.trim().is_empty() && subscription.title != parsed.title
                            {
                                subscription.title = parsed.title;
                                let bytes = encode(&self.subscriptions);
                                context.store().save(FEEDS, bytes);
                            }
                        }
                    }
                    None => {
                        // It did answer with a feed; the feed did not fit.
                        // Saying it was not a feed sends somebody looking for
                        // a different address, which will not help.
                        self.problem = Some(if truncated(&bytes, FEED_BYTES) {
                            "That feed is larger than this can read.".to_owned()
                        } else {
                            "That address did not answer with a feed.".to_owned()
                        });
                    }
                },
            },
            TaskOutcome::Failed(error) => {
                // The SDK owns the wording. Five applications wrote five
                // different sentences for the same failure before this existed.
                let failure = Failure::of(error);
                self.trouble = Some(failure);
                self.problem = Some(failure.advice.to_owned());
            }
            TaskOutcome::Cancelled => self.problem = Some("Cancelled.".to_owned()),
        }
        self.show(context);
    }
}

fn main() -> ExitCode {
    match kobo_sdk::run("rss", Feeds::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("rss: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        article_text, byline, decode, encode, pretty_host, search, Awaiting, Feeds, Subscription,
        View, FEED_BYTES, MAX_FEEDS, SEARCH_BYTES,
    };
    use kobo_sdk::{action_id, AppRunner, Command, TaskId, TaskOutcome};
    use kobo_ui::{Chrome, Glyph, LayoutKind, CLARA_BW_METRICS};

    const ATOM: &str = "<feed><title>A Journal</title>\
        <entry><title>First post</title><link href=\"https://example.com/1\"/>\
        <published>2019-07-05T16:00:30Z</published><author><name>A Writer</name></author>\
        <content>The body of the first post.</content></entry></feed>";

    fn following() -> Vec<Subscription> {
        vec![Subscription {
            url: "https://example.com/feed.xml".to_owned(),
            title: "A Journal".to_owned(),
            site: "https://example.com/".to_owned(),
        }]
    }

    #[test]
    fn typing_an_address_asks_feedsearch_for_exactly_that_address() {
        let mut runner = AppRunner::new(Feeds {
            loaded: true,
            ..Feeds::default()
        });
        runner.action(action_id("add"));
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
        assert_eq!(
            url,
            "https://feedsearch.dev/api/v1/search?url=paw&favicon=false"
        );
    }

    #[test]
    fn choosing_a_result_follows_it_and_fetches_it() {
        let mut runner = AppRunner::new(Feeds {
            loaded: true,
            view: View::Found,
            found: vec![search::Found {
                url: "https://example.com/feed.xml".to_owned(),
                title: "A Journal".to_owned(),
                site: "https://example.com/".to_owned(),
                summary: "20 articles".to_owned(),
            }],
            ..Feeds::default()
        });
        let commands = runner.action(action_id("found-0"));
        let application = runner.app_mut();
        assert_eq!(application.subscriptions.len(), 1);
        assert_eq!(application.view, View::Items);
        assert!(application.awaiting(Awaiting::Feed));
        let saved = commands
            .iter()
            .any(|command| matches!(command, Command::Store(kobo_sdk::StoreRequest::Save { .. })));
        assert!(saved, "the new subscription was not written");
    }

    #[test]
    fn following_something_already_followed_opens_it_rather_than_repeating_it() {
        let mut runner = AppRunner::new(Feeds {
            loaded: true,
            view: View::Found,
            subscriptions: following(),
            found: vec![search::Found {
                url: "https://example.com/feed.xml".to_owned(),
                title: "A Journal".to_owned(),
                site: String::new(),
                summary: String::new(),
            }],
            ..Feeds::default()
        });
        runner.action(action_id("found-0"));
        let application = runner.app_mut();
        assert_eq!(application.subscriptions.len(), 1);
        assert_eq!(application.open, Some(0));
    }

    #[test]
    fn a_fetched_feed_becomes_articles_and_corrects_the_stored_name() {
        let mut runner = AppRunner::new(Feeds {
            loaded: true,
            view: View::Items,
            open: Some(0),
            subscriptions: vec![Subscription {
                url: "https://example.com/feed.xml".to_owned(),
                title: "example.com".to_owned(),
                site: "https://example.com/".to_owned(),
            }],
            task: Some((TaskId(1), Awaiting::Feed)),
            ..Feeds::default()
        });
        runner.task_outcome(TaskId(1), TaskOutcome::Completed(ATOM.as_bytes().to_vec()));
        let application = runner.app_mut();
        assert_eq!(application.items.len(), 1);
        assert_eq!(application.items[0].title, "First post");
        assert_eq!(application.subscriptions[0].title, "A Journal");
    }

    #[test]
    fn the_verbs_over_a_feed_are_marks_in_the_bar_rather_than_words() {
        // The verb used to be a caption, "Refresh", spelled into a bottom
        // button that shared its bar with the two page turns -- three controls
        // that read as three things to do when two of them were only how to
        // reach the rest of the list. It is a glyph in the top bar now, so the
        // bottom of the panel is the page turns and nothing else.
        let mut runner = AppRunner::new(Feeds {
            loaded: true,
            view: View::Items,
            open: Some(0),
            subscriptions: following(),
            task: Some((TaskId(1), Awaiting::Feed)),
            ..Feeds::default()
        });
        let commands =
            runner.task_outcome(TaskId(1), TaskOutcome::Completed(ATOM.as_bytes().to_vec()));
        let layout = screen_of(&commands).layout_with(&CLARA_BW_METRICS, &Chrome::default());
        let refresh = layout.nodes.iter().find_map(|node| match node.kind {
            LayoutKind::BarGlyph(id, Glyph::Refresh) => Some(id),
            _ => None,
        });
        assert_eq!(
            refresh,
            Some(action_id("refresh")),
            "the feed's refresh verb was not drawn as its glyph"
        );
        let unfollow = layout.nodes.iter().find_map(|node| match node.kind {
            LayoutKind::BarGlyph(id, Glyph::Trash) => Some(id),
            _ => None,
        });
        assert_eq!(
            unfollow,
            Some(action_id("remove")),
            "unfollowing a feed was not drawn as the bin the shelf uses for it"
        );
        assert!(
            layout
                .nodes
                .iter()
                .filter(|node| matches!(node.kind, LayoutKind::BarAction(_)))
                .count()
                == 0,
            "both verbs in this bar have a picture, so neither should be a word"
        );
    }

    /// Removing a feed used to mean opening it first, which meant fetching a
    /// feed you had already decided you did not want. The mark on the row is
    /// the short way, and it must not be mistaken for the row itself.
    #[test]
    fn the_mark_on_a_feed_opens_a_menu_rather_than_the_feed() {
        let mut runner = AppRunner::new(Feeds {
            loaded: true,
            subscriptions: following(),
            ..Feeds::default()
        });
        let commands = runner.action(action_id("feed-menu-0"));
        let screen = screen_of(&commands);
        assert!(
            !commands
                .iter()
                .any(|command| matches!(command, Command::Spawn { .. })),
            "the mark fetched the feed, so it was read as a tap on the row"
        );
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
        assert!(
            layout
                .nodes
                .iter()
                .any(|node| matches!(node.kind, LayoutKind::Scrim { .. })),
            "no menu opened"
        );
        assert!(
            text_of(&screen).iter().any(|line| line == "Delete"),
            "the menu did not offer to remove the feed"
        );
    }

    #[test]
    fn stopping_following_removes_the_feed_and_writes_the_list_back() {
        let mut runner = AppRunner::new(Feeds {
            loaded: true,
            subscriptions: following(),
            ..Feeds::default()
        });
        runner.action(action_id("feed-menu-0"));
        let commands = runner.action(action_id("feed-forget"));
        assert!(
            commands.iter().any(|command| matches!(
                command,
                Command::Store(kobo_sdk::StoreRequest::Save { .. })
            )),
            "the shorter list was never written back"
        );
        let screen = screen_of(&commands);
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
        assert!(
            !layout
                .nodes
                .iter()
                .any(|node| matches!(node.kind, LayoutKind::Scrim { .. })),
            "the menu stayed open over a feed that no longer exists"
        );
        assert!(
            text_of(&screen)
                .iter()
                .any(|line| line.contains("No feeds yet")),
            "the last feed was removed and the shelf still listed it"
        );
    }

    /// A popover is dismissed by a tap beside it, which arrives as Back. On
    /// the shelf Back otherwise leaves the application, so an open menu has to
    /// claim it first or putting the menu away closes Feeds.
    #[test]
    fn putting_the_menu_away_does_not_leave_the_application() {
        let mut runner = AppRunner::new(Feeds {
            loaded: true,
            subscriptions: following(),
            ..Feeds::default()
        });
        let opened = screen_of(&runner.action(action_id("feed-menu-0")));
        assert!(
            opened.owns_back,
            "the shelf did not claim Back while its menu was open, so the tap \
             beside the menu would have left Feeds"
        );
        let commands = runner.action(kobo_sdk::ActionId::BACK);
        let screen = screen_of(&commands);
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
        assert!(
            !layout
                .nodes
                .iter()
                .any(|node| matches!(node.kind, LayoutKind::Scrim { .. })),
            "the menu did not close"
        );
        assert!(
            text_of(&screen)
                .iter()
                .any(|line| line.contains("A Journal")),
            "closing the menu also removed the feed or left the shelf"
        );
    }

    #[test]
    fn something_that_is_not_a_feed_says_so_rather_than_showing_an_empty_list() {
        let mut runner = AppRunner::new(Feeds {
            loaded: true,
            view: View::Items,
            open: Some(0),
            subscriptions: following(),
            task: Some((TaskId(1), Awaiting::Feed)),
            ..Feeds::default()
        });
        runner.task_outcome(
            TaskId(1),
            TaskOutcome::Completed(b"<html><body>a web page</body></html>".to_vec()),
        );
        assert!(runner.app_mut().problem.is_some());
    }

    #[test]
    fn opening_an_article_cuts_it_into_pages_that_fit_the_panel() {
        let mut runner = AppRunner::new(Feeds {
            loaded: true,
            view: View::Items,
            open: Some(0),
            subscriptions: following(),
            task: Some((TaskId(1), Awaiting::Feed)),
            ..Feeds::default()
        });
        let long = "Some prose about the state of the world, at length. ".repeat(80);
        let source =
            format!("<rss><channel><title>A Journal</title><item><title>Long</title><description>{long}</description></item></channel></rss>");
        runner.task_outcome(TaskId(1), TaskOutcome::Completed(source.into_bytes()));
        runner.action(action_id("item-0"));
        let application = runner.app_mut();
        assert_eq!(application.view, View::Reading);
        assert!(application.pages.len() > 1, "the article fitted one page");
        assert_eq!(application.page, 0);
    }

    #[test]
    fn a_page_of_an_article_is_as_full_as_the_page_it_is_drawn_on() {
        // The reading screen carries nothing at its foot but the place it is
        // at, and it sets its prose in the reading face. Measured with a
        // bottom bar reserved and in the interface face, a page came back four
        // lines short and the article stopped in a field of white. A page is
        // full when one more line would not have fitted on it.
        let mut runner = AppRunner::new(Feeds {
            loaded: true,
            view: View::Items,
            open: Some(0),
            subscriptions: following(),
            task: Some((TaskId(1), Awaiting::Feed)),
            ..Feeds::default()
        });
        let long = "Some prose about the state of the world, at length. ".repeat(120);
        let source = format!(
            "<rss><channel><title>A Journal</title><item><title>Long</title>\
             <description>{long}</description></item></channel></rss>"
        );
        runner.task_outcome(TaskId(1), TaskOutcome::Completed(source.into_bytes()));
        runner.action(action_id("item-0"));
        let total = runner.app_mut().pages.len();
        assert!(total > 2, "too few pages to prove anything");

        for page in 0..total - 1 {
            runner.app_mut().page = page;
            let layout = runner
                .app_mut()
                .reading()
                .layout_with(&CLARA_BW_METRICS, &Chrome::default());
            let bottom = layout
                .nodes
                .iter()
                .filter(|node| matches!(node.kind, LayoutKind::Text))
                .map(|node| node.rect.y + node.rect.height)
                .max()
                .unwrap_or(0);
            // The runtime draws the status strip over the top of the panel and
            // the layout engine takes the position band out before it places
            // anything, so the page really ends above both.
            let floor =
                layout.content.y + layout.content.height - CLARA_BW_METRICS.status_band_height();
            let line = kobo_ui::FontSize::Body.line_height_in(kobo_ui::Face::Reading);
            assert!(bottom <= floor, "page {page} was set under the strip");
            assert!(
                bottom + line > floor,
                "page {page} left a line of room: {bottom} + {line} against {floor}"
            );
        }
    }

    #[test]
    fn back_unwinds_this_application_before_it_leaves_it() {
        let mut runner = AppRunner::new(Feeds {
            loaded: true,
            view: View::Reading,
            open: Some(0),
            article: Some(0),
            subscriptions: following(),
            ..Feeds::default()
        });
        runner.action(kobo_sdk::ActionId::BACK);
        assert_eq!(runner.app_mut().view, View::Items);
        runner.action(kobo_sdk::ActionId::BACK);
        assert_eq!(runner.app_mut().view, View::Shelf);
    }

    #[test]
    fn unfollowing_removes_the_feed_and_returns_to_the_shelf() {
        let mut runner = AppRunner::new(Feeds {
            loaded: true,
            view: View::Items,
            open: Some(0),
            subscriptions: following(),
            ..Feeds::default()
        });
        runner.action(action_id("remove"));
        let application = runner.app_mut();
        assert!(application.subscriptions.is_empty());
        assert_eq!(application.view, View::Shelf);
    }

    #[test]
    fn a_stored_list_survives_a_round_trip() {
        let feeds = vec![
            Subscription {
                url: "https://example.com/feed.xml".to_owned(),
                title: "A Journal".to_owned(),
                site: "https://example.com/".to_owned(),
            },
            Subscription {
                url: "https://other.example/atom".to_owned(),
                title: "Another\tone\nentirely".to_owned(),
                site: String::new(),
            },
        ];
        let read = decode(&encode(&feeds));
        assert_eq!(read.len(), 2);
        assert_eq!(read[0], feeds[0]);
        assert_eq!(read[1].url, feeds[1].url);
        assert_eq!(read[1].title, "Another one entirely");
    }

    #[test]
    fn a_damaged_list_keeps_the_lines_that_still_make_sense() {
        let read = decode(b"\n\thttps://a.example/feed\nhttps://b.example/feed\t\t\n\n");
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].url, "https://b.example/feed");
        assert_eq!(read[0].title, "b.example");
    }

    #[test]
    fn a_list_longer_than_the_application_holds_is_cut_rather_than_refused() {
        let feeds: Vec<Subscription> = (0..MAX_FEEDS + 10)
            .map(|index| Subscription {
                url: format!("https://example.com/{index}"),
                title: format!("Feed {index}"),
                site: String::new(),
            })
            .collect();
        assert_eq!(decode(&encode(&feeds)).len(), MAX_FEEDS);
    }

    #[test]
    fn a_host_is_shown_the_way_somebody_would_say_it() {
        assert_eq!(pretty_host("https://www.example.com/", ""), "example.com");
        assert_eq!(
            pretty_host("", "http://example.com/feed.xml"),
            "example.com"
        );
        assert_eq!(pretty_host("", ""), "");
    }

    #[test]
    fn an_article_carries_its_byline_and_its_address() {
        let item = super::feed::Item {
            title: "First post".to_owned(),
            link: "https://example.com/1".to_owned(),
            stamp: "2019-07-05T16:00:30Z".to_owned(),
            author: "A Writer".to_owned(),
            body: "The body.".to_owned(),
        };
        assert_eq!(byline(&item), "A Writer \u{00b7} 05 Jul");
        let text = article_text(&item);
        assert!(text.contains("A Writer"));
        assert!(text.contains("The body."));
        assert!(text.contains("https://example.com/1"));
    }

    #[test]
    fn an_item_that_says_nothing_about_itself_still_gets_a_line() {
        let item = super::feed::Item {
            title: "Untitled".to_owned(),
            body: "A few words of the body stand in for the byline.".to_owned(),
            ..super::feed::Item::default()
        };
        assert!(byline(&item).starts_with("A few words"));
    }

    /// The last screen an action produced.
    #[test]
    fn an_empty_feed_after_a_failure_says_the_failure_rather_than_nothing_published() {
        // "Nothing published yet" is a statement about the feed. Saying it to a
        // reader who is simply offline is a lie the SDK already knows better
        // than, and it sends them back to a publisher who did nothing wrong.
        let mut runner = AppRunner::new(Feeds {
            loaded: true,
            view: View::Items,
            open: Some(0),
            subscriptions: following(),
            task: Some((TaskId(1), Awaiting::Feed)),
            ..Feeds::default()
        });
        let commands =
            runner.task_outcome(TaskId(1), TaskOutcome::Failed(kobo_sdk::TaskError::Offline));
        let text = text_of(&screen_of(&commands));
        assert!(
            text.iter().any(|line| line.contains("not on a network")),
            "the offline advice is not on the article list: {text:?}"
        );
        assert!(
            !text.iter().any(|line| line.contains("Nothing published")),
            "an offline reader is still told the feed published nothing: {text:?}"
        );
    }

    #[test]
    fn every_failure_is_worded_by_the_sdk() {
        // Five applications wrote five sentences for one failure before
        // `Failure` existed. This is the assertion that keeps rss on it.
        for (error, expected) in [
            (kobo_sdk::TaskError::Offline, "not on a network"),
            (kobo_sdk::TaskError::Unreachable, "did not answer"),
            (kobo_sdk::TaskError::TimedOut, "too slow"),
        ] {
            let mut runner = AppRunner::new(Feeds {
                loaded: true,
                view: View::Items,
                open: Some(0),
                subscriptions: following(),
                task: Some((TaskId(1), Awaiting::Feed)),
                ..Feeds::default()
            });
            runner.task_outcome(TaskId(1), TaskOutcome::Failed(error));
            let said = runner.app_mut().problem.clone().unwrap_or_default();
            assert_eq!(said, kobo_sdk::Failure::of(error).advice);
            assert!(said.contains(expected), "{error:?} was worded as {said:?}");
        }
    }

    /// Every string a screen would draw, flattened.
    fn text_of(screen: &kobo_sdk::Screen) -> Vec<String> {
        screen
            .layout_with(
                &kobo_sdk::CLARA_BW_METRICS,
                &kobo_sdk::Chrome::with_back(true),
            )
            .nodes
            .iter()
            .flat_map(|node| node.text_lines.clone())
            .collect()
    }

    fn screen_of(commands: &[Command]) -> kobo_sdk::Screen {
        commands
            .iter()
            .rev()
            .find_map(|command| match command {
                Command::SetScreen(screen) => Some(screen.clone()),
                _ => None,
            })
            .expect("the action drew a screen")
    }

    /// Every screen has to fit the panel it is drawn on.
    ///
    /// Asserted against the layout rather than against the numbers that
    /// produced it. Rows are cut into pages by the runtime's own measurement,
    /// but the things placed around them (the attribution the search service
    /// requires, a keyboard, a nav bar) are placed by this application, and
    /// nothing but the layout makes the two agree. A screen that overflows
    /// loses its last element silently, and on hardware that reads as a
    /// missing button rather than as a bug.
    fn fits_the_panel(screen: &kobo_sdk::Screen, what: &str) {
        let issues = screen.validate(&CLARA_BW_METRICS);
        assert!(
            issues.is_empty(),
            "{what} does not fit the panel: {issues:?}"
        );
    }

    #[test]
    fn every_screen_in_the_whole_journey_fits_the_panel() {
        let mut runner = AppRunner::new(Feeds {
            loaded: true,
            ..Feeds::default()
        });

        // An empty shelf, which is the first thing a new reader sees.
        fits_the_panel(
            &screen_of(&runner.action(kobo_sdk::ActionId::BACK)),
            "the empty shelf",
        );

        // Typing an address. The keyboard takes most of the panel, and the
        // attribution has to fit above it.
        fits_the_panel(
            &screen_of(&runner.action(action_id("add"))),
            "the search screen",
        );
        for key in ["kb.r0c9", "kb.r1c0", "kb.r0c1"] {
            fits_the_panel(
                &screen_of(&runner.action(action_id(key))),
                "the search screen mid-typing",
            );
        }
        fits_the_panel(
            &screen_of(&runner.action(action_id("kb.enter"))),
            "the search in flight",
        );

        // A full page of results, each with the longest title and summary the
        // service is allowed to return, plus the attribution underneath.
        let entries: Vec<String> = (0..12)
            .map(|index| {
                format!(
                    r#"{{"url":"https://example.com/feed/{index}","title":"{}","description":"{}","item_count":20,"score":{index}}}"#,
                    "A Publication With A Very Long Name Indeed ".repeat(4),
                    "A description that runs on at some length. ".repeat(4)
                )
            })
            .collect();
        let answer = format!("[{}]", entries.join(","));
        let commands = runner.task_outcome(TaskId(1), TaskOutcome::Completed(answer.into_bytes()));
        fits_the_panel(&screen_of(&commands), "a full page of results");

        // Choosing one, then a feed of long articles.
        fits_the_panel(
            &screen_of(&runner.action(action_id("found-0"))),
            "the feed loading",
        );
        let items: Vec<String> = (0..20)
            .map(|index| {
                format!(
                    "<item><title>An article with a headline of the length \
                     publishers actually use, number {index}</title>\
                     <author>A Writer With A Long Name</author>\
                     <pubDate>Fri, 05 Jul 2019 16:00:30 +0000</pubDate>\
                     <description>{}</description></item>",
                    "Some prose about the state of the world, at length. ".repeat(40)
                )
            })
            .collect();
        let source = format!(
            "<rss><channel><title>A Journal</title>{}</channel></rss>",
            items.join("")
        );
        let commands = runner.task_outcome(TaskId(2), TaskOutcome::Completed(source.into_bytes()));
        fits_the_panel(&screen_of(&commands), "a page of articles");

        // Every page of the article list, then every page of one article.
        fits_the_panel(
            &screen_of(&runner.action(action_id("list-next"))),
            "a later page of articles",
        );
        fits_the_panel(
            &screen_of(&runner.action(action_id("list-back"))),
            "back to the first page",
        );

        let commands = runner.action(action_id("item-0"));
        fits_the_panel(&screen_of(&commands), "the first page of an article");
        let pages = runner.app_mut().pages.len();
        assert!(pages > 1, "the long article fitted a single page");
        for page in 1..pages {
            let commands = runner.action(action_id("page-next"));
            fits_the_panel(&screen_of(&commands), &format!("article page {page}"));
        }
    }

    #[test]
    fn the_screens_that_say_nothing_happened_fit_too() {
        // Empty and error states are the ones nobody looks at until they
        // appear on a device in front of somebody.
        let mut runner = AppRunner::new(Feeds {
            loaded: true,
            view: View::Items,
            open: Some(0),
            subscriptions: following(),
            task: Some((TaskId(1), Awaiting::Feed)),
            ..Feeds::default()
        });
        let commands =
            runner.task_outcome(TaskId(1), TaskOutcome::Completed(b"<html></html>".to_vec()));
        fits_the_panel(&screen_of(&commands), "a feed that was not a feed");

        let mut runner = AppRunner::new(Feeds {
            loaded: true,
            view: View::Found,
            query: "example.com".to_owned(),
            task: Some((TaskId(1), Awaiting::Search)),
            ..Feeds::default()
        });
        let commands = runner.task_outcome(TaskId(1), TaskOutcome::Completed(b"[]".to_vec()));
        fits_the_panel(&screen_of(&commands), "a search that found nothing");

        let mut runner = AppRunner::new(Feeds {
            loaded: true,
            view: View::Items,
            open: Some(0),
            subscriptions: following(),
            task: Some((TaskId(1), Awaiting::Feed)),
            ..Feeds::default()
        });
        let commands = runner.task_outcome(
            TaskId(1),
            TaskOutcome::Failed(kobo_sdk::TaskError::Unreachable),
        );
        fits_the_panel(&screen_of(&commands), "a feed that could not be reached");
    }

    #[test]
    fn feedsearch_is_credited_on_both_screens_that_show_its_results() {
        // A licensing obligation, not a preference: their terms ask for an
        // attribution visible to the reader on the search and results screens.
        // It has already been lost once, to a full page of results pushing it
        // off the panel, which is why it is asserted rather than trusted.
        let mut runner = AppRunner::new(Feeds {
            loaded: true,
            ..Feeds::default()
        });
        let search = screen_of(&runner.action(action_id("add")));
        assert!(
            format!("{search:?}").contains("feedsearch.dev"),
            "the search screen does not credit Feedsearch"
        );

        for key in ["kb.r0c9", "kb.r1c0", "kb.r0c1"] {
            runner.action(action_id(key));
        }
        // The results screen, while the search is still in flight.
        let waiting = screen_of(&runner.action(action_id("kb.enter")));
        assert!(
            format!("{waiting:?}").contains("feedsearch.dev"),
            "the results screen does not credit Feedsearch while loading"
        );

        let mut runner = AppRunner::new(Feeds {
            loaded: true,
            view: View::Found,
            query: "example.com".to_owned(),
            task: Some((TaskId(1), Awaiting::Search)),
            ..Feeds::default()
        });
        let answer = br#"[{"url":"https://example.com/rss","title":"Example","score":1}]"#;
        let results =
            screen_of(&runner.task_outcome(TaskId(1), TaskOutcome::Completed(answer.to_vec())));
        assert!(
            format!("{results:?}").contains("feedsearch.dev"),
            "the results screen does not credit Feedsearch"
        );
    }

    #[test]
    fn a_shelf_of_the_most_feeds_this_holds_is_still_turnable() {
        let subscriptions: Vec<Subscription> = (0..MAX_FEEDS)
            .map(|index| Subscription {
                url: format!("https://example.com/{index}"),
                title: format!("A Publication With A Long Name, number {index}"),
                site: format!("https://a-fairly-long-hostname-{index}.example.com/"),
            })
            .collect();
        let mut runner = AppRunner::new(Feeds {
            loaded: true,
            subscriptions,
            ..Feeds::default()
        });
        let commands = runner.action(kobo_sdk::ActionId::BACK);
        let mut screen = screen_of(&commands);
        fits_the_panel(&screen, "a full shelf");
        // And every later page of it. A page that turns back onto itself
        // sends nothing at all, because the runner drops a screen identical to
        // the one already showing, so the last screen stands.
        for page in 1..8 {
            let commands = runner.action(action_id("list-next"));
            if let Some(next) = commands.iter().rev().find_map(|command| match command {
                Command::SetScreen(screen) => Some(screen.clone()),
                _ => None,
            }) {
                screen = next;
            }
            fits_the_panel(&screen, &format!("shelf page {page}"));
        }
    }

    #[test]
    fn a_feed_too_large_to_read_is_not_reported_as_not_a_feed() {
        // Sending somebody to look for a different address does not help when
        // the address was right and the feed was simply bigger than the
        // budget. The two failures read identically before this.
        let mut runner = AppRunner::new(Feeds {
            loaded: true,
            view: View::Items,
            open: Some(0),
            subscriptions: following(),
            task: Some((TaskId(1), Awaiting::Feed)),
            ..Feeds::default()
        });
        let cut = vec![b'{'; FEED_BYTES as usize];
        let commands = runner.task_outcome(TaskId(1), TaskOutcome::Completed(cut));
        let screen = screen_of(&commands);
        assert!(format!("{screen:?}").contains("larger than this can read"));
        fits_the_panel(&screen, "a feed that was too large");
    }

    #[test]
    fn a_short_answer_that_is_not_a_feed_still_says_so() {
        let mut runner = AppRunner::new(Feeds {
            loaded: true,
            view: View::Items,
            open: Some(0),
            subscriptions: following(),
            task: Some((TaskId(1), Awaiting::Feed)),
            ..Feeds::default()
        });
        let commands =
            runner.task_outcome(TaskId(1), TaskOutcome::Completed(b"<html></html>".to_vec()));
        let screen = screen_of(&commands);
        assert!(format!("{screen:?}").contains("did not answer with a feed"));
    }

    #[test]
    fn a_search_answer_that_was_cut_short_says_so_rather_than_finding_nothing() {
        let mut runner = AppRunner::new(Feeds {
            loaded: true,
            view: View::Found,
            query: "example.com".to_owned(),
            task: Some((TaskId(1), Awaiting::Search)),
            ..Feeds::default()
        });
        let cut = vec![b'['; SEARCH_BYTES as usize];
        let commands = runner.task_outcome(TaskId(1), TaskOutcome::Completed(cut));
        let screen = screen_of(&commands);
        assert!(format!("{screen:?}").contains("too large to read"));
        fits_the_panel(&screen, "a search answer that was cut short");
    }

    #[test]
    fn a_site_with_no_feeds_is_not_accused_of_answering_too_much() {
        let mut runner = AppRunner::new(Feeds {
            loaded: true,
            view: View::Found,
            query: "example.com".to_owned(),
            task: Some((TaskId(1), Awaiting::Search)),
            ..Feeds::default()
        });
        let commands = runner.task_outcome(TaskId(1), TaskOutcome::Completed(b"[]".to_vec()));
        let screen = screen_of(&commands);
        assert!(!format!("{screen:?}").contains("too large"));
    }
}

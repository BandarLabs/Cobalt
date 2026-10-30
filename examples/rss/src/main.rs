//! Feeds: the sites you read, on the device.
//!
//! Type an address, pick the feed it finds, and read the articles without
//! leaving the application.
//!
//! Website names use feed discovery; full HTTPS feed addresses are fetched
//! directly without disclosure to the discovery service. Articles currently
//! survive refresh failures and are saved in verified local snapshots.
//!
//! Refresh all reads every feed in turn, and the newest articles that carry
//! pictures are kept whole with their pictures, so the shelf's "Ready
//! offline" line is a promise about what opens with the radio off rather
//! than about which words happen to be in a feed file.

mod cache;
mod feed;
mod illustrations;
mod opml;
mod progress;
mod search;
mod status;

use kobo_bookview::offline::{Heard, Keeper};
use kobo_sdk::keyboard::{TextEntry, Typing};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, BannerShape, Context, Failure, Glyph, KoboApp, LogLevel,
    RowEdge, RowState, Screen, ScreenBuilder, StoreResult, Task, TaskId, TaskOutcome,
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

/// Public feeds, opened only after the reader chooses one.
///
/// Both carry whole articles with their pictures. The first starter used to be
/// a news service whose feed carries one sentence per story, so the first
/// article anybody opened in Feeds was a sentence and a link they could not
/// follow, and the application looked broken on its first screen. The titles
/// are the ones each feed gives itself, so the shelf does not rename a feed
/// the moment it is first read.
const STARTER_FEEDS: &[(&str, &str, &str)] = &[
    (
        "NASA Science",
        "Space, Earth and discoveries from NASA, whole articles",
        "https://science.nasa.gov/feed/",
    ),
    (
        "Science + Tech \u{2013} The Conversation",
        "Researchers explain the news, whole articles",
        "https://theconversation.com/us/technology/articles.atom",
    ),
];

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

/// How many of each feed's newest articles are kept whole with their
/// pictures.
///
/// A feed of fifty illustrated articles is tens of megabytes of pictures, and
/// the radio and the card both pay for every one. The newest ten are what
/// somebody reads on the train; older ones still open offline as words, and
/// fetch their pictures when they are opened online.
const KEEP_NEWEST: usize = 10;

/// The attribution Feedsearch's terms ask for, on the screen where there is
/// room for the whole sentence.
///
/// The results screen carries it in its top bar instead. Both screens show
/// their results because of Feedsearch, and both have to say so.
const ATTRIBUTION: &str = "Feed search powered by feedsearch.dev";

/// The address field on the screen that adds a feed.
const FEED_ADDRESS: &str = "feed-address";

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
    /// What searching every saved subscription found.
    SavedResults,
    ArticleSearch,
    Import,
    Starters,
    /// One article.
    Reading,
}

/// One article found by searching every saved subscription.
#[derive(Clone, Debug, Eq, PartialEq)]
struct SavedHit {
    feed: usize,
    item: usize,
    /// The title as it was read, so drawing the row parses nothing.
    title: String,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum ArticleSearch {
    #[default]
    ThisFeed,
    EverySaved,
}

/// What the one outstanding request is for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Awaiting {
    Probe,
    Search,
    Feed,
}

#[derive(Default)]
struct Feeds {
    view: View,
    caches: std::collections::HashMap<String, cache::Cache>,
    reader: kobo_bookview::BookView,
    progress: progress::Progress,
    reading_id: Option<String>,
    import_files: Vec<String>,
    import_listing: bool,
    import_read: Option<kobo_sdk::ShelfDownload>,
    import_preview: Option<opml::Import>,
    import_excluded: std::collections::BTreeSet<usize>,
    import_pending: Option<Vec<Subscription>>,
    subscription_save: SubscriptionSave,
    statuses: status::Statuses,

    illustrations: illustrations::Illustrations,
    /// The subscription list, as stored.
    subscriptions: Vec<Subscription>,
    /// False until the store has answered once, so that an empty list is not
    /// mistaken for a reader who follows nothing.
    loaded: bool,
    /// The address of a site or feed to add, typed on the reader's keyboard.
    entry: TextEntry,
    /// An address refused before anything was fetched, kept in the field so
    /// it can be corrected rather than typed again.
    address: String,
    /// Why `address` was refused, drawn as the field's error.
    address_error: Option<&'static str>,
    /// Words to look for in saved articles, typed on the reader's keyboard.
    search_entry: TextEntry,
    /// What was typed, kept to caption the results screen.
    query: String,
    article_query: String,
    /// Search every saved subscription, not only the feed that is open.
    article_search: ArticleSearch,
    /// What searching every saved subscription found, in the order found.
    ///
    /// Kept rather than searched for again on every repaint: the search reads
    /// every saved feed, and the screen it draws turns pages.
    saved_hits: Vec<SavedHit>,
    /// What the search found, best first.
    found: Vec<search::Found>,
    direct: bool,
    /// Which subscription is open.
    open: Option<usize>,
    /// The open feed's articles.
    items: Vec<feed::Item>,
    /// Which article is being read.
    article: Option<usize>,
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
    /// Articles kept whole with their pictures.
    keeper: Keeper,
    /// Refresh all: the feed being fetched now, and the ones still to go.
    refreshing: Option<(TaskId, String)>,
    refresh_queue: std::collections::VecDeque<String>,
    /// How many feeds refresh all has finished, and how many of them failed.
    refreshed: (usize, usize),
    /// Feeds fetched by refresh all whose saved copy had not been read back
    /// yet, held until it has so the new copy is written over the right slot.
    refresh_pending: std::collections::HashMap<String, Vec<u8>>,
    /// Articles and bytes in each saved feed, counted once as it arrives.
    offline: std::collections::HashMap<String, (usize, usize)>,
}

#[derive(Default)]
struct SubscriptionSave {
    writing: Option<Vec<u8>>,
    queued: Option<Vec<u8>>,
    failed: bool,
    load_failed: bool,
}

impl Feeds {
    fn awaiting(&self, what: Awaiting) -> bool {
        matches!(self.task, Some((_, outstanding)) if outstanding == what)
    }

    /// Writes the subscription list back. Called after every change.
    fn save(&mut self, context: &mut Context) {
        self.save_subscriptions(context, encode(&self.subscriptions));
    }

    fn save_subscriptions(&mut self, context: &mut Context, bytes: Vec<u8>) {
        if self.subscription_save.load_failed {
            return;
        }
        if self.subscription_save.writing.is_some() {
            self.subscription_save.queued = Some(bytes);
        } else {
            self.subscription_save.writing = Some(bytes.clone());
            context.store().save(FEEDS, bytes);
        }
    }

    /// Full HTTPS addresses are fetched directly, without disclosing them to discovery.
    fn find_feed(&mut self, context: &mut Context, address: &str) {
        if !address.contains("://") {
            self.ask_search(context, address);
            return;
        }
        self.direct = true;
        self.found.clear();
        self.problem = None;
        self.trouble = None;
        if let Some(refused) = refused_address(address) {
            self.problem = Some(refused.to_owned());
            return;
        }
        match context.spawn_retrying(Task::Fetch {
            url: address.to_owned(),
            offset: 0,
            max_bytes: FEED_BYTES,
            credential: None,
            headers: Vec::new(),
        }) {
            Some(task) => self.task = Some((task, Awaiting::Probe)),
            None => self.problem = Some("The device is busy. Try again.".to_owned()),
        }
    }

    /// Asks Feedsearch what feeds an address has.
    fn ask_search(&mut self, context: &mut Context, url: &str) {
        self.direct = false;
        self.found.clear();
        self.problem = None;
        self.trouble = None;
        let request = search::request(url);
        match context.spawn_retrying(Task::Fetch {
            url: request,
            offset: 0,
            max_bytes: SEARCH_BYTES,
            credential: None,
            headers: Vec::new(),
        }) {
            Some(task) => self.task = Some((task, Awaiting::Search)),
            None => self.problem = Some("The device is busy. Try that again.".to_owned()),
        }
    }

    fn open_cached(&mut self, context: &mut Context) {
        let Some(url) = self
            .open
            .and_then(|index| self.subscriptions.get(index))
            .map(|feed| feed.url.clone())
        else {
            return;
        };
        if let Some(cached) = self.caches.get(&url) {
            if let Some(parsed) = cached.bytes.as_deref().and_then(feed::parse) {
                self.items = parsed.items;
                self.update_unread_summaries(context);
                return;
            }
            if cached.busy() {
                return;
            }
            self.ask_feed(context);
        } else {
            let cached = cache::Cache::new(&url);
            cached.start(context);
            self.caches.insert(url, cached);
        }
    }

    fn cache_event(&mut self, context: &mut Context, url: &str, event: Option<cache::Event>) {
        if event == Some(cache::Event::Loaded) {
            if let Some(bytes) = self.refresh_pending.remove(url) {
                if let Some(cached) = self.caches.get_mut(url) {
                    cached.save(context, bytes);
                }
            }
        }
        if matches!(event, Some(cache::Event::Loaded | cache::Event::Saved)) {
            self.count_offline(url);
        }
        let current = self
            .open
            .and_then(|index| self.subscriptions.get(index))
            .is_some_and(|feed| feed.url == url);
        if !current {
            // A search of everything saved is waiting on every feed, not on
            // the open one, so its results grow as the copies arrive.
            if self.view == View::SavedResults && event == Some(cache::Event::Loaded) {
                self.find_saved_hits();
                self.show(context);
            }
            // The shelf counts what is on the device, so it is redrawn once
            // the saved feeds have all been read back. Once, not per feed:
            // forty feeds arriving one by one would be forty full refreshes
            // of the panel while somebody is trying to tap one of them.
            if self.view == View::Shelf
                && matches!(event, Some(cache::Event::Loaded | cache::Event::Saved))
                && !self.saved_copies_arriving()
            {
                self.show(context);
            }
            return;
        }
        match event {
            Some(cache::Event::Loaded) => self.open_cached(context),
            Some(cache::Event::Failed) => self.problem = Some("Saved articles could not be read or updated. Your open articles are still available.".to_owned()),
            Some(cache::Event::Saved) => {
                if self.problem.as_deref().is_some_and(|problem| problem.starts_with("Saved articles could not") || problem.starts_with("These articles could not")) { self.problem = None; }
                self.update_unread_summaries(context);
            },
            None => {},
        }
        self.show(context);
    }

    /// Fetches the open feed.
    fn ask_feed(&mut self, context: &mut Context) {
        let Some(subscription) = self.open.and_then(|index| self.subscriptions.get(index)) else {
            return;
        };
        let url = subscription.url.clone();
        if self.caches.get(&url).is_some_and(cache::Cache::busy) {
            return;
        }
        self.problem = None;
        self.trouble = None;
        match context.spawn_retrying(Task::Fetch {
            url,
            offset: 0,
            max_bytes: FEED_BYTES,
            credential: None,
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

    fn note_feed_outcome(
        &mut self,
        context: &mut Context,
        awaiting: Awaiting,
        feed_succeeded: bool,
    ) {
        if awaiting == Awaiting::Feed {
            if let Some(feed) = self.open.and_then(|index| self.subscriptions.get(index)) {
                if feed_succeeded {
                    self.statuses.note(
                        context,
                        &feed.url,
                        Ok(status::timestamp().unwrap_or_else(|| "time unavailable".into())),
                    );
                } else if let Some(problem) = &self.problem {
                    self.statuses.note(context, &feed.url, Err(problem.clone()));
                }
            }
        }
    }

    fn update_unread_summaries(&mut self, context: &mut Context) {
        for (url, cached) in &self.caches {
            if !self.subscriptions.iter().any(|feed| &feed.url == url) {
                continue;
            }
            let Some(parsed) = cached.bytes.as_deref().and_then(feed::parse) else {
                continue;
            };
            let read: Option<Vec<_>> = parsed
                .items
                .iter()
                .map(|item| self.progress.has_saved_read(&article_id(item, url)))
                .collect();
            if let Some(read) = read {
                self.statuses.unread(
                    context,
                    url,
                    read.iter().filter(|read| !**read).count(),
                    read.len(),
                );
            }
        }
    }

    /// Counts what one saved feed holds, for the shelf's offline line.
    fn count_offline(&mut self, url: &str) {
        if let Some(bytes) = self
            .caches
            .get(url)
            .and_then(|cached| cached.bytes.as_ref())
        {
            let articles = feed::parse(bytes).map_or(0, |parsed| parsed.items.len());
            self.offline.insert(url.to_owned(), (articles, bytes.len()));
        }
    }

    /// What the shelf says is on the device, in the words every application
    /// uses for it.
    fn offline_line(&self) -> String {
        let (articles, bytes) = self
            .subscriptions
            .iter()
            .filter_map(|feed| self.offline.get(&feed.url))
            .fold((0, 0), |(articles, bytes), (more, size)| {
                (articles + more, bytes + size)
            });
        let line = kobo_bookview::offline::ready_offline(
            articles,
            bytes as u64 + self.keeper.bytes(),
            "article",
            "articles",
        );
        let mut said = vec![line.trim_end_matches('.').to_owned()];
        if self.refreshing.is_some() || !self.refresh_queue.is_empty() {
            let total = self.refreshed.0 + self.refresh_queue.len() + 1;
            said.insert(
                0,
                format!(
                    "Refreshing {} of {total} feeds",
                    (self.refreshed.0 + 1).min(total)
                ),
            );
        } else if self.refreshed.1 > 0 {
            said.push(format!(
                "{} of {} feeds could not be refreshed",
                self.refreshed.1, self.refreshed.0
            ));
        }
        if self.keeper.is_busy() {
            said.push("Saving pictures for offline reading".to_owned());
        }
        let mut joined = said.join(". ");
        if said.len() > 1 || line.ends_with('.') {
            joined.push('.');
        }
        joined
    }

    /// Fetches every feed, one after another.
    ///
    /// One at a time, because forty feeds at once is forty requests on a
    /// radio that runs four, and the open article's pictures would wait
    /// behind all of them.
    fn refresh_all(&mut self, context: &mut Context) {
        if self.refreshing.is_some() || self.subscriptions.is_empty() {
            return;
        }
        self.load_saved_caches(context);
        self.refresh_queue = self
            .subscriptions
            .iter()
            .map(|feed| feed.url.clone())
            .collect();
        self.refreshed = (0, 0);
        self.problem = None;
        self.refresh_next(context);
    }

    fn refresh_next(&mut self, context: &mut Context) {
        if self.refreshing.is_some() {
            return;
        }
        let Some(url) = self.refresh_queue.pop_front() else {
            self.finish_refresh(context);
            return;
        };
        match context.spawn_retrying(Task::Fetch {
            url: url.clone(),
            offset: 0,
            max_bytes: FEED_BYTES,
            credential: None,
            headers: Vec::new(),
        }) {
            Some(task) => self.refreshing = Some((task, url)),
            // No lane free. It goes back to the front and the next task to
            // finish starts it.
            None => self.refresh_queue.push_front(url),
        }
    }

    /// One feed of refresh all, answered.
    fn took_refresh(&mut self, context: &mut Context, url: &str, outcome: TaskOutcome) {
        let result = match outcome {
            TaskOutcome::Completed(bytes) => match feed::parse(&bytes) {
                Some(parsed) => {
                    match self.caches.get_mut(url) {
                        Some(cached) if !cached.busy() => {
                            if !cached.save(context, bytes.clone()) {
                                self.refresh_pending.insert(url.to_owned(), bytes);
                            }
                        }
                        _ => {
                            self.refresh_pending.insert(url.to_owned(), bytes);
                        }
                    }
                    if let Some(index) = self.subscriptions.iter().position(|feed| feed.url == url)
                    {
                        if !parsed.title.trim().is_empty()
                            && self.subscriptions[index].title != parsed.title
                        {
                            self.subscriptions[index].title.clone_from(&parsed.title);
                            self.save(context);
                        }
                        if self.open == Some(index) {
                            self.items.clone_from(&parsed.items);
                        }
                    }
                    self.keep_pictures(context, url, &parsed.items);
                    Ok(status::timestamp().unwrap_or_else(|| "time unavailable".into()))
                }
                None => Err(if truncated(&bytes, FEED_BYTES) {
                    "That feed is larger than this can read.".to_owned()
                } else {
                    "That address did not answer with a feed.".to_owned()
                }),
            },
            TaskOutcome::Failed(error) => Err(Failure::of(error).advice.to_owned()),
            TaskOutcome::Cancelled => Err("Cancelled.".to_owned()),
        };
        self.refreshed.0 += 1;
        if result.is_err() {
            self.refreshed.1 += 1;
        }
        self.statuses.note(context, url, result);
        self.refresh_next(context);
    }

    /// Everything has been asked for. The unread counts are worked out again,
    /// and the kept articles that no feed carries any more are let go.
    fn finish_refresh(&mut self, context: &mut Context) {
        self.update_unread_summaries(context);
        let every_feed_read = self.subscriptions.iter().all(|feed| {
            self.caches
                .get(&feed.url)
                .is_some_and(|cached| cached.bytes.is_some())
        });
        // Only once every saved feed has been read back. A feed whose copy is
        // still on its way would otherwise look empty, and its kept articles
        // would be thrown away for it.
        if every_feed_read {
            let current: std::collections::HashSet<String> = self
                .subscriptions
                .iter()
                .filter_map(|feed| {
                    let parsed = feed::parse(self.caches[&feed.url].bytes.as_deref()?)?;
                    Some(
                        parsed
                            .items
                            .iter()
                            .map(|item| article_id(item, &feed.url))
                            .collect::<Vec<_>>(),
                    )
                })
                .flatten()
                .collect();
            self.keeper.retain(context, |id| current.contains(id));
        }
    }

    /// Keeps the newest illustrated articles of a feed whole, with their
    /// pictures, so they open with the radio off.
    ///
    /// Only for Refresh all, which is the owner getting ready to be away from
    /// Wi-Fi; opening one feed is reading it now, and its pictures come as
    /// each article opens. An article already opened is left out too: its
    /// pictures were saved when it was read, and fetching them again would
    /// spend the radio on a second copy.
    fn keep_pictures(&mut self, context: &mut Context, feed_url: &str, items: &[feed::Item]) {
        for item in items
            .iter()
            .filter(|item| !item.html.is_empty() && item.html.contains("<img"))
            .filter(|item| self.progress.has_read(&article_id(item, feed_url)) != Some(true))
            .take(KEEP_NEWEST)
        {
            let origin = if item.link.is_empty() {
                feed_url
            } else {
                item.link.as_str()
            };
            self.keeper
                .keep_markup(context, &article_id(item, feed_url), origin, &item.html);
        }
    }

    /// What the keeper of whole articles said, acted on.
    fn heard(&mut self, context: &mut Context, heard: Heard) {
        match heard {
            Heard::Elsewhere | Heard::Quiet => return,
            Heard::Changed | Heard::Failed(..) => {}
            Heard::Opened(id, kept) => {
                if self.view == View::Reading
                    && self.reading_id.as_deref() == Some(id.as_str())
                    && !self.reader.is_open()
                {
                    let memory = self.progress.memory(&id);
                    if !self.reader.open_kept(context, kept, memory) {
                        self.open_from_feed(context);
                    }
                }
            }
            Heard::Unreadable(id) => {
                if self.view == View::Reading && self.reading_id.as_deref() == Some(id.as_str()) {
                    self.open_from_feed(context);
                }
            }
        }
        self.show(context);
    }

    fn keep_position(&mut self, context: &mut Context) {
        if let (Some(id), Some(memory)) = (&self.reading_id, self.reader.memory()) {
            self.progress.keep(context, id.clone(), memory.clone());
        }
    }

    fn show(&mut self, context: &mut Context) {
        let screen = match self.view {
            View::Shelf => self.shelf(context),
            View::Search => self.search(),
            View::Starters => self.starters(context),
            View::Found => self.results(context),
            View::SavedResults => self.saved_results(context),
            View::Items => self.articles(context),
            View::ArticleSearch => self.article_search(),
            View::Import => self.import_screen(context),
            View::Reading => self.reading(),
        };
        // Every view except the shelf was reached from another one, so Back
        // unwinds this application first and leaves it only from the shelf.
        // Without this, Back out of an article lands at the launcher.
        context
            .set_screen(screen.with_own_back(self.view != View::Shelf || self.menu_open.is_some()));
    }

    /// The banner over the shelf: what it says, how loudly, and the retry
    /// it carries.
    ///
    /// A write that failed is the owner's to fix, so it is an Error with
    /// Retry saving on it. That retry was the Refresh arrows in the top bar,
    /// the same picture that means Refresh all in the same place, under words
    /// naming a control the screen never showed.
    fn shelf_notice(&self) -> Option<(BannerLevel, String, Option<&'static str>)> {
        if self.subscription_save.failed {
            Some((
                BannerLevel::Error,
                "Subscriptions were not saved. Retry saving before closing Feeds.".to_owned(),
                Some("retry-subscriptions"),
            ))
        } else if self.statuses.failed {
            Some((
                BannerLevel::Error,
                "Refresh history is not saved.".to_owned(),
                Some("retry-status"),
            ))
        } else if let Some(problem) = &self.problem {
            Some((BannerLevel::Attention, problem.clone(), None))
        } else {
            // What is on the device is said whenever nothing more urgent is,
            // and only once there is something to follow.
            (self.loaded && !self.subscriptions.is_empty())
                .then(|| (BannerLevel::Info, self.offline_line(), None))
        }
    }

    /// What stands at a feed row's right edge: the menu mark, and the count
    /// of unread articles when there are any.
    fn feed_edge(unread: u32) -> RowEdge<'static> {
        if unread > 0 {
            RowEdge::new().with_menu().with_badge()
        } else {
            RowEdge::new().with_menu()
        }
    }

    /// Each feed's title and summary, clamped against the narrower column
    /// the overflow mark and the unread badge leave, or the longest titles
    /// run under them; and each feed's unread count.
    fn shelf_rows(&self, context: &Context) -> (Vec<(String, String)>, Vec<u32>) {
        let unread: Vec<u32> = self
            .subscriptions
            .iter()
            .map(|feed| self.statuses.unread_of(&feed.url).unwrap_or(0))
            .collect();
        let rows: Vec<(String, String)> = self
            .subscriptions
            .iter()
            .zip(&unread)
            .map(|(feed, unread)| {
                let edge = Self::feed_edge(*unread);
                let title = context.clamped_row_with_edge(&feed.title, 1, true, edge);
                let summary = context.clamped_row_with_edge(
                    &self
                        .statuses
                        .summary(&feed.url)
                        .unwrap_or_else(|| pretty_host(&feed.site, &feed.url)),
                    2,
                    true,
                    edge,
                );
                (title, summary)
            })
            .collect();
        (rows, unread)
    }

    fn shelf(&self, context: &Context) -> Screen {
        let mut screen = ScreenBuilder::new("rss-shelf").top_bar("Feeds");
        if self.subscription_save.load_failed {
            return screen
                .splash(None, "Could not open feeds", "Your saved subscriptions could not be opened. The saved file has been left unchanged.")
                .primary_button("retry-load-subscriptions", "Try again")
                .build();
        }
        if self.loaded && !self.subscriptions.is_empty() {
            screen = screen.top_bar_glyph("refresh-all", "Refresh all", Glyph::Refresh);
        }
        let notice = self.shelf_notice();
        if let Some((level, said, retry)) = &notice {
            screen = screen.banner(*level, said.clone());
            if let Some(retry) = retry {
                screen = screen.banner_action(*retry, "Retry saving");
            }
        }
        if !self.loaded {
            return screen.activity("Opening your feeds", None).build();
        }
        screen = screen.top_bar_glyph("search-saved", "Search saved", Glyph::Search);
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
        let (rows, unread) = self.shelf_rows(context);
        let measured: Vec<_> = rows
            .iter()
            .zip(&unread)
            .map(|((title, summary), unread)| {
                (title.as_str(), summary.as_str(), Self::feed_edge(*unread))
            })
            .collect();
        // Measured under the banner as it is drawn, with its mark and the
        // line its Retry saving takes.
        let shape = notice.as_ref().map(|(level, said, retry)| {
            let shape = BannerShape::new(*level, said);
            if retry.is_some() {
                shape.with_action()
            } else {
                shape
            }
        });
        let pages = context.paginate_rows_with_edges_below_banner(&measured, true, shape);
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
        // The count of what is new sits at the right edge, where an eye
        // scanning the shelf for it looks, and is left off at zero.
        for index in &shown {
            if unread[*index] > 0 {
                screen = screen.row_badge(format!("feed-{index}"), unread[*index]);
            }
        }
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

    fn starters(&self, context: &Context) -> Screen {
        let rows: Vec<_> = STARTER_FEEDS
            .iter()
            .map(|(title, description, _)| (*title, *description))
            .collect();
        let pages = context.paginate_rows(&rows, false);
        let page = self.list_page.min(pages.len().saturating_sub(1));
        let mut screen = ScreenBuilder::new("rss-starters")
            .top_bar("Browse feeds")
            .rows(pages.get(page).into_iter().flatten().map(|index| {
                (
                    format!("starter-{index}"),
                    rows[*index].0,
                    rows[*index].1,
                    Glyph::Rss,
                )
            }));
        if pages.len() > 1 {
            screen = screen
                .page_turns("list-back", "list-next")
                .page_position(page_number(page), page_total(pages.len()));
        }
        screen.build()
    }

    /// Where a feed is added from: an address, an OPML file or the bundled
    /// list.
    ///
    /// The address field raises the reader's keyboard when it is tapped
    /// rather than with the screen, because the keyboard covers the panel and
    /// Import OPML and Browse would be out of reach behind it.
    fn search(&self) -> Screen {
        let screen = ScreenBuilder::new("rss-search").top_bar("Add a feed");
        if self.entry.is_open() {
            return screen
                .text_entry_with_hint(
                    &self.entry,
                    "Website or HTTPS feed address",
                    ATTRIBUTION,
                    "Search",
                )
                .build();
        }
        let mut screen = screen
            .top_bar_action("import-opml", "Import OPML")
            .top_bar_action("browse-feeds", "Browse");
        if let Some(problem) = &self.problem {
            screen = screen.banner(BannerLevel::Attention, problem.clone());
        }
        // A refused address comes back in the field with the reason under
        // it, in the attribution's place, so it is corrected rather than
        // typed again. It used to go on to a results screen whose Try again
        // asked for the same refused address.
        let screen = screen
            .field(FEED_ADDRESS, &self.address, "Website or HTTPS feed address")
            .field_kind(kobo_sdk::FieldKind::Address)
            .field_hint(ATTRIBUTION);
        match self.address_error {
            Some(error) => screen.field_error(error),
            None => screen,
        }
        .build()
    }

    /// Raises the reader's keyboard for a feed address, holding `text` to
    /// correct rather than retype.
    fn type_address(&mut self, text: &str) {
        self.entry = TextEntry::new().address();
        self.entry.open_with(text);
    }

    fn results(&self, context: &Context) -> Screen {
        // The attribution lives in the top bar rather than under the list.
        // Feedsearch's terms ask for it to be visible wherever their results
        // are shown, and anything in the flow below a full page of rows is the
        // first thing the panel drops, silently, so the one element that is
        // not optional would be the one element missing. The bar is drawn
        // before the content and cannot be pushed off it.
        let mut screen = ScreenBuilder::new("rss-found").top_bar(if self.direct {
            "Feed preview"
        } else {
            "Feeds via feedsearch.dev"
        });
        if self.awaiting(Awaiting::Search) || self.awaiting(Awaiting::Probe) {
            return screen
                .divider()
                .activity(format!("Looking for feeds at {}", self.query), None)
                .skeleton(4)
                .build();
        }
        // A fetch that failed keeps this screen, with its fix on the banner
        // that says why: Join Wi-Fi off the network, Try again where asking
        // again can help, nothing where it cannot. Change address is a
        // different job and stays a button.
        if let Some(problem) = &self.problem {
            let screen = screen.banner(BannerLevel::Attention, problem.clone());
            let screen = match self.trouble {
                Some(failure) if failure.state == kobo_sdk::StandardState::Offline => {
                    screen.banner_action(kobo_sdk::JOIN_WIFI, "Join Wi-Fi")
                }
                Some(failure) if !failure.retryable => screen,
                _ => screen.banner_action("search-retry", "Try again"),
            };
            return screen.button("add", "Change address").build();
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

    fn import_screen(&self, context: &Context) -> Screen {
        let mut screen = ScreenBuilder::new("rss-import").top_bar("Import subscriptions");
        if let Some(problem) = &self.problem {
            screen = screen.banner(BannerLevel::Attention, problem);
        }
        if self.import_listing {
            return screen.activity("Looking for OPML files", None).build();
        }
        if self.import_pending.is_some() {
            return screen.activity("Saving subscriptions", None).build();
        }
        if self.import_read.is_some() {
            return screen.activity("Reading OPML file", None).build();
        }
        if let Some(preview) = &self.import_preview {
            return self.import_preview_screen(context, preview);
        }
        if self.import_files.is_empty() {
            return screen
                .empty_state("No OPML files are available in the Feeds app folder.")
                .build();
        }
        let rows: Vec<_> = self
            .import_files
            .iter()
            .map(|name| (name.as_str(), "OPML subscription list"))
            .collect();
        let pages = context.paginate_rows_below_notice(&rows, false, self.problem.as_deref());
        let page = self.list_page.min(pages.len().saturating_sub(1));
        screen = screen.rows(pages.get(page).into_iter().flatten().map(|index| {
            (
                format!("import-file-{index}"),
                rows[*index].0,
                rows[*index].1,
                Glyph::Rss,
            )
        }));
        if pages.len() > 1 {
            screen = screen
                .page_turns("list-back", "list-next")
                .page_position(page_number(page), page_total(pages.len()));
        }
        screen.build()
    }

    fn import_preview_screen(&self, context: &Context, preview: &opml::Import) -> Screen {
        let fresh: Vec<_> = preview
            .feeds
            .iter()
            .enumerate()
            .filter(|(_, feed)| !self.subscriptions.iter().any(|saved| saved.url == feed.url))
            .collect();
        let selected = fresh
            .iter()
            .filter(|(index, _)| !self.import_excluded.contains(index))
            .count();
        let skipped = preview.skipped + preview.feeds.len() - fresh.len();
        // A count of nothing skipped is a sentence somebody has to read to
        // learn that nothing happened, so it is only said when it is not zero.
        let skipped = match skipped {
            0 => String::new(),
            1 => "1 duplicate or unsupported entry skipped. ".to_owned(),
            _ => format!("{skipped} duplicate or unsupported entries skipped. "),
        };
        let mut notice = format!(
            "{} new {}. {skipped}Tap a feed to include or leave it out.",
            fresh.len(),
            if fresh.len() == 1 { "feed" } else { "feeds" },
        );
        if let Some(problem) = &self.problem {
            notice = format!("{problem} {notice}");
        }
        let room = MAX_FEEDS.saturating_sub(self.subscriptions.len());
        if selected > room {
            notice = format!("Choose up to {room} feeds. {notice}");
        }
        let mut screen = ScreenBuilder::new("rss-import")
            .top_bar("Choose feeds")
            .banner(
                if self.problem.is_some() || selected > room {
                    BannerLevel::Attention
                } else {
                    BannerLevel::Info
                },
                &notice,
            );
        if fresh.is_empty() {
            return screen.text("There are no new feeds to add.").build();
        }
        let rows: Vec<_> = fresh
            .iter()
            .map(|(_, feed)| {
                (
                    feed.title.clone(),
                    // The site, the way somebody would say it. The whole
                    // address wrapped onto a second line for most feeds and
                    // said nothing the host did not.
                    pretty_host(&feed.site, &feed.url),
                )
            })
            .collect();
        let measured: Vec<_> = rows
            .iter()
            .map(|(title, detail)| (title.as_str(), detail.as_str()))
            .collect();
        // Paged as though a bar were under the list, because the selection
        // bar is: it takes the room a bar would.
        let pages = context.paginate_rows_below_notice(&measured, true, Some(&notice));
        let page = self.list_page.min(pages.len().saturating_sub(1));
        // Each feed is ticked in the runtime's own box, and the bar under the
        // list counts them and adds them, so what will be added is read off
        // the boxes rather than off a word in every row.
        screen = screen.checked_rows(pages.get(page).into_iter().flatten().map(|index| {
            let (feed, _) = fresh[*index];
            (
                format!("import-toggle-{feed}"),
                rows[*index].0.as_str(),
                rows[*index].1.as_str(),
                Glyph::Rss,
                if self.import_excluded.contains(&feed) {
                    RowState::Unchecked
                } else {
                    RowState::Checked
                },
            )
        }));
        let add = (selected > 0 && selected <= room).then_some(("import-confirm", "Add"));
        screen = screen
            .fill()
            .selection_bar(u32::try_from(selected).unwrap_or(u32::MAX), add);
        if pages.len() > 1 {
            screen = screen
                .page_turns("list-back", "list-next")
                .page_position(page_number(page), page_total(pages.len()));
        }
        screen.build()
    }

    fn article_search(&self) -> Screen {
        ScreenBuilder::new("rss-article-search")
            .top_bar(if self.article_search == ArticleSearch::EverySaved {
                "Search all saved articles"
            } else {
                "Search saved articles"
            })
            .text_entry(
                &self.search_entry,
                "Words in the title, author or article",
                "Search",
            )
            .build()
    }

    /// Reads every saved feed once and keeps what matched.
    ///
    /// Called when the query changes and when a saved copy arrives, never
    /// while drawing: each feed parsed here is a whole file of articles turned
    /// into text, and the results screen turns pages.
    fn find_saved_hits(&mut self) {
        self.saved_hits.clear();
        let query = self.article_query.to_lowercase();
        let words: Vec<_> = query.split_whitespace().collect();
        if words.is_empty() {
            return;
        }
        for (feed, subscription) in self.subscriptions.iter().enumerate() {
            let Some(parsed) = self
                .caches
                .get(&subscription.url)
                .and_then(|cached| cached.bytes.as_deref())
                .and_then(feed::parse)
            else {
                continue;
            };
            for (item, article) in parsed.items.iter().enumerate() {
                if matches_words(article, &words) {
                    self.saved_hits.push(SavedHit {
                        feed,
                        item,
                        title: article.title.clone(),
                    });
                }
            }
        }
    }

    /// Opens the saved copy of every subscription that has not been read yet.
    ///
    /// Searching what is saved has to mean everything saved. The caches are
    /// otherwise filled one feed at a time as feeds are opened, so a search
    /// made before opening anything would have looked at nothing and said the
    /// words appear nowhere.
    fn load_saved_caches(&mut self, context: &mut Context) {
        let urls: Vec<String> = self
            .subscriptions
            .iter()
            .map(|feed| feed.url.clone())
            .filter(|url| !self.caches.contains_key(url))
            .collect();
        for url in urls {
            let cached = cache::Cache::new(&url);
            cached.start(context);
            self.caches.insert(url, cached);
        }
    }

    /// Whether any saved copy is still being read.
    fn saved_copies_arriving(&self) -> bool {
        self.subscriptions.iter().any(|feed| {
            self.caches
                .get(&feed.url)
                .is_some_and(|cached| cached.bytes.is_none() && cached.busy())
        })
    }

    fn saved_results(&self, context: &Context) -> Screen {
        let mut screen = ScreenBuilder::new("rss-saved-search")
            .top_bar(format!("Results for {}", self.article_query))
            .top_bar_action("clear-search", "Clear");
        let arriving = self.saved_copies_arriving();
        if self.saved_hits.is_empty() {
            return if arriving {
                screen.activity("Reading the saved articles", None).build()
            } else {
                screen
                    .empty_state("No saved articles match this search.")
                    .build()
            };
        }
        if arriving {
            // Saying so beats a list that silently grows under the reader.
            screen = screen.banner(BannerLevel::Info, "Still reading the saved articles.");
        }
        let rows: Vec<(String, String, String)> = self
            .saved_hits
            .iter()
            .map(|hit| {
                let title = self
                    .subscriptions
                    .get(hit.feed)
                    .map_or("Feed", |feed| feed.title.as_str());
                (
                    format!("hit-{}-{}", hit.feed, hit.item),
                    context.clamped_row(&hit.title, 2, true),
                    context.one_line_row(title, true),
                )
            })
            .collect();
        let borrowed: Vec<_> = rows
            .iter()
            .map(|(_, title, summary)| (title.as_str(), summary.as_str()))
            .collect();
        let pages = context.paginate_rows(&borrowed, false);
        let page = self.list_page.min(pages.len().saturating_sub(1));
        let shown = pages.get(page).cloned().unwrap_or_default();
        screen = screen.rows(shown.iter().map(|index| {
            (
                rows[*index].0.clone(),
                rows[*index].1.clone(),
                rows[*index].2.clone(),
                u16::try_from(index + 1).unwrap_or(u16::MAX),
            )
        }));
        if pages.len() > 1 {
            screen = screen
                .page_turns("list-back", "list-next")
                .page_position(page_number(page), page_total(pages.len()));
        }
        screen.build()
    }

    /// Returns from an article to the list it was opened from.
    ///
    /// An article reached from a search of everything saved goes back to those
    /// results, not to the articles of whichever feed happened to hold it. The
    /// reader is closed by three separate paths, so the answer lives here
    /// rather than being remembered correctly at each of them.
    fn close_article(&mut self) {
        self.article = None;
        if self.article_search == ArticleSearch::EverySaved {
            self.view = View::SavedResults;
            self.open = None;
            self.items.clear();
        } else {
            self.view = View::Items;
        }
    }

    /// Opens an article of the open feed in the reader.
    ///
    /// Both lists that lead to an article come through here, so a saved-search
    /// result is read on exactly the terms a result in its own feed is.
    fn open_article(&mut self, context: &mut Context, index: usize) {
        let Some(item) = self.items.get(index) else {
            return;
        };
        self.article = Some(index);
        self.view = View::Reading;
        self.reader.close(context);
        let feed_url = self
            .open
            .and_then(|i| self.subscriptions.get(i))
            .map_or(String::new(), |feed| feed.url.clone());
        let origin = if item.link.is_empty() {
            feed_url
        } else {
            item.link.clone()
        };
        let id = article_id(item, &origin);
        self.reading_id = Some(id.clone());
        // A copy kept with its pictures opens from the device. The feed's own
        // words are the fallback, and they are always here too.
        if !self.keeper.open(context, &id) {
            self.open_from_feed(context);
        }
    }

    /// Opens the article being read from the feed's own copy of it, fetching
    /// its pictures as it goes.
    fn open_from_feed(&mut self, context: &mut Context) {
        self.keeper.stop_opening();
        let Some(item) = self
            .article
            .and_then(|index| self.items.get(index))
            .cloned()
        else {
            return;
        };
        let origin = if item.link.is_empty() {
            self.open
                .and_then(|i| self.subscriptions.get(i))
                .map_or(String::new(), |feed| feed.url.clone())
        } else {
            item.link.clone()
        };
        let id = article_id(&item, &origin);
        let memory = self.progress.memory(&id);
        self.reading_id = Some(id);
        if item.html.is_empty() {
            if self
                .reader
                .open_bytes(
                    context,
                    "article.txt",
                    article_text(&item).as_bytes(),
                    memory,
                )
                .is_err()
            {
                self.problem = Some("This article could not be opened.".to_owned());
            }
        } else {
            self.reader
                .open(context, kobo_doc::html::parse(&item.html), memory);
            self.illustrations.open(context, &mut self.reader, &origin);
        }
        self.keep_position(context);
    }

    fn matching_items(&self) -> Vec<usize> {
        let query = self.article_query.to_lowercase();
        let words: Vec<_> = query.split_whitespace().collect();
        self.items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| matches_words(item, &words).then_some(index))
            .collect()
    }

    fn feed_save_failed(&self) -> bool {
        self.open
            .and_then(|index| self.subscriptions.get(index))
            .and_then(|feed| self.caches.get(&feed.url))
            .is_some_and(cache::Cache::retryable)
    }

    fn article_was_read(&self, item: &feed::Item) -> Option<bool> {
        let url = self
            .open
            .and_then(|index| self.subscriptions.get(index))
            .map_or("", |feed| feed.url.as_str());
        self.progress.has_read(&article_id(item, url))
    }

    fn article_notice(&self) -> Option<String> {
        let unread = self
            .items
            .iter()
            .filter_map(|item| self.article_was_read(item))
            .filter(|read| !read)
            .count();
        let reading_count = format!(
            "{unread} unread of {} {}.",
            self.items.len(),
            if self.items.len() == 1 {
                "article"
            } else {
                "articles"
            }
        );
        let mut notices = Vec::new();
        if self.feed_save_failed() {
            notices.push("Saved articles could not be read or updated. Try saving again.");
        }
        if self
            .items
            .first()
            .is_some_and(|item| self.article_was_read(item).is_some())
        {
            notices.push(reading_count.as_str());
        }
        if self.subscription_save.failed {
            notices.push("Subscriptions were not saved. Go back to Feeds to retry saving.");
        }
        if self.awaiting(Awaiting::Feed) && !self.items.is_empty() {
            notices.push("Checking for new articles.");
        }
        if let Some(problem) = &self.problem {
            // The cache warning above already covers this failure. Preserve
            // independent network errors without repeating the save warning.
            if !self.feed_save_failed()
                || !(problem.starts_with("Saved articles could not")
                    || problem.starts_with("These articles could not"))
            {
                notices.push(problem.as_str());
            }
        } else if let Some(reason) = self
            .open
            .and_then(|index| self.subscriptions.get(index))
            .and_then(|feed| self.statuses.detail(&feed.url))
        {
            notices.push(reason);
        }
        if self.progress.failed && self.illustrations.can_retry() {
            notices.push("Some images and reading progress are not saved.");
        } else if self.progress.failed {
            notices.push("Reading progress is not saved.");
        } else if self.illustrations.can_retry() {
            notices.push("Some images are not saved.");
        } else if self.illustrations.failed {
            notices.push("Some article images could not be loaded.");
        }
        (!notices.is_empty()).then(|| notices.join(" "))
    }

    fn article_header(&self, context: &Context, notice: Option<&str>) -> ScreenBuilder {
        let title = self
            .open
            .and_then(|index| self.subscriptions.get(index))
            .map_or_else(|| "Feed".to_owned(), |feed| feed.title.clone());
        let heading = if self.article_query.is_empty() {
            title
        } else {
            format!("Results for {}", self.article_query)
        };
        let mut screen = ScreenBuilder::new("rss-items")
            .top_bar(context.one_line_row(&heading, false))
            .top_bar_glyph("remove", "Unfollow", Glyph::Trash)
            // Fetching again is the one thing done here often enough to earn a
            // glyph rather than a word: the feed is read on demand, so a reader
            // catching up taps this on every feed they open. The two arrows say
            // it in the width a caption of "Refresh" wanted, which is what left
            // room for it to sit beside Unfollow inside the bar's two places.
            .top_bar_glyph("refresh", "Refresh", Glyph::Refresh);
        let retry =
            self.progress.failed || self.illustrations.can_retry() || self.feed_save_failed();
        if let Some(notice) = notice {
            screen = screen.banner(
                if self.problem.is_some()
                    || self.progress.failed
                    || self.illustrations.failed
                    || self.subscription_save.failed
                {
                    BannerLevel::Attention
                } else {
                    BannerLevel::Info
                },
                notice,
            );
        }
        if retry {
            screen = screen.bottom_action_marked("retry-save", "Retry saving", Glyph::Refresh);
        } else if !self.items.is_empty() {
            screen = screen.bottom_action_marked(
                "search-articles",
                if self.article_query.is_empty() {
                    "Search articles"
                } else {
                    "Change search"
                },
                Glyph::Search,
            );
        }
        screen
    }

    fn articles(&self, context: &Context) -> Screen {
        let notice = self.article_notice();
        let mut screen = self.article_header(context, notice.as_deref());
        if self.items.is_empty()
            && (self.awaiting(Awaiting::Feed)
                || self
                    .open
                    .and_then(|index| self.subscriptions.get(index))
                    .and_then(|feed| self.caches.get(&feed.url))
                    .is_some_and(cache::Cache::busy))
        {
            return screen
                .divider()
                .activity(
                    if self.awaiting(Awaiting::Feed) {
                        "Fetching the latest articles"
                    } else {
                        "Opening saved articles"
                    },
                    None,
                )
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
        let matches = self.matching_items();
        if matches.is_empty() {
            return screen
                .empty_state("No saved articles match this search.")
                .build();
        }
        let rows: Vec<(String, String)> = matches
            .iter()
            .map(|index| &self.items[*index])
            .map(|item| {
                (
                    context.clamped_row(&item.title, 2, true),
                    context.one_line_row(
                        &format!(
                            "{}{}",
                            if self.article_was_read(item) == Some(false) {
                                "Unread · "
                            } else {
                                ""
                            },
                            byline(item)
                        ),
                        true,
                    ),
                )
            })
            .collect();
        let borrowed: Vec<_> = rows
            .iter()
            .map(|(title, summary)| (title.as_str(), summary.as_str()))
            .collect();
        let pages = context.paginate_rows_below_notice(&borrowed, true, notice.as_deref());
        let page = self.list_page.min(pages.len().saturating_sub(1));
        let shown = pages.get(page).cloned().unwrap_or_default();
        screen = screen.rows(shown.iter().map(|index| {
            (
                format!("item-{}", matches[*index]),
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
        if let Some(screen) = self.reader.screen(&title) {
            return screen;
        }
        if self.keeper.is_opening() {
            return ScreenBuilder::new("rss-reading")
                .top_bar(title)
                .activity("Opening the saved article", None)
                .build();
        }
        ScreenBuilder::new("rss-reading")
            .top_bar(title)
            .empty_state("This article arrived empty.")
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

/// Stable identity shared by reading position and unread status.
fn article_id(item: &feed::Item, feed_url: &str) -> String {
    let origin = if item.link.trim().is_empty() {
        feed_url
    } else {
        item.link.as_str()
    };
    let body = if item.html.is_empty() {
        article_text(item)
    } else {
        item.html.clone()
    };
    kobo_net::sha256::hex_digest(format!("{origin}\n{}\n{body}", item.title).as_bytes())
}

/// The whole article as one piece of prose, ready to be cut into pages.
/// Whether an article holds every word, ignoring case.
///
/// One rule, so that searching the open feed and searching everything saved
/// cannot come to answer the same words differently.
fn matches_words(item: &feed::Item, words: &[&str]) -> bool {
    let text = format!("{} {} {}", item.title, item.author, item.body).to_lowercase();
    words.iter().all(|word| text.contains(word))
}

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

/// Reads a whole subscription record or refuses it without discarding entries.
fn decode(bytes: &[u8]) -> Result<Vec<Subscription>, ()> {
    let text = std::str::from_utf8(bytes).map_err(|_| ())?;
    let mut feeds = Vec::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let fields: Vec<_> = line.split('\t').collect();
        let [url, title, site] = fields.as_slice() else {
            return Err(());
        };
        let url = url.trim();
        if url.is_empty()
            || !(url.starts_with("https://") || url.starts_with("http://"))
            || feeds.len() >= MAX_FEEDS
        {
            return Err(());
        }
        let title = title.trim();
        let site = site.trim();
        feeds.push(Subscription {
            url: url.to_owned(),
            title: if title.is_empty() {
                pretty_host(site, url)
            } else {
                title.to_owned()
            },
            site: site.to_owned(),
        });
    }
    Ok(feeds)
}

/// The index in a `prefix-N` action name, if that is what this is.
fn indexed(action: ActionId, prefix: &str, count: usize) -> Option<usize> {
    (0..count).find(|index| action_id(&format!("{prefix}-{index}")) == action)
}

impl KoboApp for Feeds {
    /// The feed icon is orange, and has been since Firefox introduced it in
    /// 2004.
    fn accent(&self) -> Option<kobo_sdk::Accent> {
        Some(kobo_sdk::Accent::Orange)
    }

    fn on_start(&mut self, context: &mut Context) {
        context.store().load(FEEDS);
        context.store().load(progress::KEY);
        context.store().load(status::KEY);
        self.keeper.start(context);
        self.show(context);
    }

    fn on_load(&mut self, context: &mut Context, key: &str, result: StoreResult) {
        let heard = self.keeper.loaded(context, key, &result);
        if !matches!(heard, Heard::Elsewhere) {
            self.heard(context, heard);
            return;
        }
        if key == status::KEY {
            self.statuses.load(context, result);
            if self.loaded
                && !self.subscription_save.load_failed
                && !self.subscription_save.failed
                && self.subscription_save.writing.is_none()
            {
                self.statuses.retain(
                    context,
                    self.subscriptions.iter().map(|feed| feed.url.clone()),
                );
            }
            self.show(context);
            return;
        }
        if self
            .illustrations
            .store(context, &mut self.reader, key, &result, false)
        {
            self.show(context);
            return;
        }
        if key == progress::KEY {
            self.progress.load(result);
            self.update_unread_summaries(context);
            self.show(context);
            return;
        }
        if key == FEEDS {
            self.on_store(context, result);
            return;
        }
        if let Some((url, cached)) = self.caches.iter_mut().find(|(_, cached)| cached.key == key) {
            let url = url.clone();
            let event = cached.stored(context, &result);
            self.cache_event(context, &url, event);
        }
    }

    fn on_save(&mut self, context: &mut Context, key: &str, result: StoreResult) {
        let heard = self.keeper.saved(context, key, &result);
        if !matches!(heard, Heard::Elsewhere) {
            self.heard(context, heard);
            return;
        }
        if key == status::KEY {
            self.statuses.saved(context, &result);
            self.show(context);
            return;
        }
        if self
            .illustrations
            .store(context, &mut self.reader, key, &result, false)
        {
            self.show(context);
            return;
        }
        if key == progress::KEY {
            self.progress.saved(context, result);
            self.update_unread_summaries(context);
            if self.progress.failed {
                if let Some(reader) = self.reader.reader_mut() {
                    reader.report("Reading progress is not saved. Go back to retry.");
                }
            }
            self.show(context);
            return;
        }
        if key == FEEDS {
            let Some(written) = self.subscription_save.writing.take() else {
                return;
            };
            if matches!(result, StoreResult::Saved { .. }) {
                if let Some(next) = self.subscription_save.queued.take() {
                    self.save_subscriptions(context, next);
                } else {
                    self.subscription_save.failed = false;
                    self.statuses.retain(
                        context,
                        self.subscriptions.iter().map(|feed| feed.url.clone()),
                    );
                    if let Some(candidate) = self.import_pending.take() {
                        if encode(&candidate) == written {
                            self.subscriptions = candidate;
                            self.import_preview = None;
                            self.view = View::Shelf;
                        }
                    }
                    if self
                        .problem
                        .as_deref()
                        .is_some_and(|text| text.starts_with("Subscriptions were not saved."))
                    {
                        self.problem = None;
                    }
                }
            } else {
                self.subscription_save.queued = None;
                self.subscription_save.failed = true;
                self.import_pending = None;
                self.problem =
                    Some("Subscriptions were not saved. Retry saving before closing Feeds.".into());
            }
            self.show(context);
            return;
        }
        if let Some((url, cached)) = self.caches.iter_mut().find(|(_, cached)| cached.key == key) {
            let url = url.clone();
            let event = cached.stored(context, &result);
            self.cache_event(context, &url, event);
        }
    }

    fn on_shelf(&mut self, context: &mut Context, name: &str, result: StoreResult) {
        let heard = self.keeper.shelf(context, name, &result);
        if !matches!(heard, Heard::Elsewhere) {
            self.heard(context, heard);
            return;
        }
        if let Some(read) = &mut self.import_read {
            if read.name() == name {
                match read.advance(context, &result) {
                    kobo_sdk::ShelfProgress::Done => {
                        let bytes = self.import_read.take().expect("active OPML read").take();
                        match opml::parse(&bytes) {
                            Ok(preview) => {
                                self.import_preview = Some(preview);
                                self.import_excluded.clear();
                                self.list_page = 0;
                            }
                            Err(problem) => self.problem = Some(problem.to_owned()),
                        }
                    }
                    kobo_sdk::ShelfProgress::Failed(_) => {
                        self.import_read = None;
                        self.problem = Some("This OPML file could not be read.".into());
                    }
                    _ => {}
                }
                self.show(context);
                return;
            }
        }
        if self
            .illustrations
            .store(context, &mut self.reader, name, &result, true)
        {
            self.show(context);
            return;
        }
        if let Some((url, cached)) = self
            .caches
            .iter_mut()
            .find(|(_, cached)| cached.owns_file(name))
        {
            let url = url.clone();
            let event = cached.shelf(context, &result);
            self.cache_event(context, &url, event);
        }
    }

    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        if self.view == View::Import {
            if matches!(result, StoreResult::Denied(_)) {
                self.import_listing = false;
                self.problem = Some("OPML files could not be listed.".into());
                self.show(context);
                return;
            }
            if let StoreResult::Shelf(files) = &result {
                self.import_listing = false;
                self.import_files = files
                    .iter()
                    .filter(|(name, _)| name.to_ascii_lowercase().ends_with(".opml"))
                    .map(|(name, _)| name.clone())
                    .collect();
                self.import_files.sort();
                self.show(context);
                return;
            }
        }
        match result {
            StoreResult::Loaded { value, .. } => {
                let decoded = value.as_deref().map_or_else(|| Ok(Vec::new()), decode);
                match decoded {
                    Ok(subscriptions) => {
                        self.subscriptions = subscriptions;
                        self.subscription_save.load_failed = false;
                        // Every saved feed is read back now rather than as each
                        // is opened, so the shelf can say what is on the device.
                        self.load_saved_caches(context);
                        self.statuses.retain(
                            context,
                            self.subscriptions.iter().map(|feed| feed.url.clone()),
                        );
                        self.problem = None;
                    }
                    Err(()) => self.subscription_save.load_failed = true,
                }
                self.loaded = true;
                self.show(context);
            }
            // A failed load must never become an empty list that later edits replace.
            StoreResult::Denied(reason) => {
                self.subscription_save.load_failed = true;
                self.loaded = true;
                context.log(
                    LogLevel::Warn,
                    format!("the feed list could not be opened: {reason}"),
                );
                self.problem = Some("Your saved subscriptions could not be opened.".to_owned());
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
        if self.subscription_save.load_failed {
            if action == action_id("retry-load-subscriptions") {
                context.store().load(FEEDS);
            }
            self.show(context);
            return;
        }
        if self.import_pending.is_some() {
            return;
        }
        if action == action_id("retry-status") {
            self.statuses.retry(context);
            self.show(context);
            return;
        }
        if action == action_id("retry-subscriptions") {
            self.save(context);
            self.show(context);
            return;
        }
        if action == action_id("browse-feeds") {
            self.view = View::Starters;
            self.list_page = 0;
            self.problem = None;
            self.show(context);
            return;
        }
        if self.view == View::Starters {
            if let Some(index) = indexed(action, "starter", STARTER_FEEDS.len()) {
                let url = STARTER_FEEDS[index].2;
                url.clone_into(&mut self.query);
                self.view = View::Found;
                self.find_feed(context, url);
                self.show(context);
                return;
            }
        }
        if action == action_id("import-opml") {
            self.view = View::Import;
            self.import_preview = None;
            self.import_files.clear();
            self.import_listing = true;
            self.problem = None;
            self.list_page = 0;
            context.shelf().list();
            self.show(context);
            return;
        }
        if self.view == View::Import {
            if let Some(preview) = &self.import_preview {
                if let Some(index) = indexed(action, "import-toggle", preview.feeds.len()) {
                    if !self.import_excluded.remove(&index) {
                        self.import_excluded.insert(index);
                    }
                    self.show(context);
                    return;
                }
            }
            if action == action_id("import-confirm") {
                if let Some(preview) = &self.import_preview {
                    let mut candidate = self.subscriptions.clone();
                    for (index, feed) in preview.feeds.iter().enumerate() {
                        if !self.import_excluded.contains(&index)
                            && !candidate.iter().any(|saved| saved.url == feed.url)
                        {
                            candidate.push(feed.clone());
                        }
                    }
                    if candidate.len() > self.subscriptions.len() && candidate.len() <= MAX_FEEDS {
                        self.save_subscriptions(context, encode(&candidate));
                        self.import_pending = Some(candidate);
                    }
                }
                self.show(context);
                return;
            }
            if let Some(index) = indexed(action, "import-file", self.import_files.len()) {
                let mut read =
                    kobo_sdk::ShelfDownload::new(&self.import_files[index]).at_most(opml::LIMIT);
                read.start(context);
                self.import_read = Some(read);
                self.problem = None;
                self.show(context);
                return;
            }
        }
        if action == action_id("refresh-all") && self.view == View::Shelf {
            self.refresh_all(context);
            self.show(context);
            return;
        }
        if action == action_id("search-saved") && self.view == View::Shelf {
            self.article_search = ArticleSearch::EverySaved;
            self.open = None;
            self.article_query.clear();
            self.saved_hits.clear();
            self.search_entry.open();
            self.list_page = 0;
            self.view = View::ArticleSearch;
            // The saved copies are read one feed at a time as feeds are
            // opened. A search of everything saved asks for the rest now.
            self.load_saved_caches(context);
            self.show(context);
            return;
        }
        if action == action_id("search-articles") && self.view == View::Items {
            self.article_search = ArticleSearch::ThisFeed;
            self.search_entry.open_with(self.article_query.clone());
            self.view = View::ArticleSearch;
            self.show(context);
            return;
        }
        if action == action_id("clear-search")
            && matches!(
                self.view,
                View::ArticleSearch | View::Items | View::SavedResults
            )
        {
            let everywhere = self.view == View::SavedResults
                || (self.view == View::ArticleSearch
                    && self.article_search == ArticleSearch::EverySaved);
            self.article_query.clear();
            self.saved_hits.clear();
            self.article_search = ArticleSearch::ThisFeed;
            self.list_page = 0;
            self.view = if everywhere { View::Shelf } else { View::Items };
            self.show(context);
            return;
        }
        if self.view == View::ArticleSearch {
            match self.search_entry.handle(action) {
                Some(Typing::Submitted(words)) => {
                    self.article_query = words;
                    self.list_page = 0;
                    if self.article_search == ArticleSearch::EverySaved {
                        self.view = View::SavedResults;
                        self.find_saved_hits();
                    } else {
                        self.view = View::Items;
                    }
                    self.show(context);
                    return;
                }
                Some(Typing::Changed) => {
                    self.show(context);
                    return;
                }
                // Backing out returns to the list the search was opened
                // from, as Back always did here.
                Some(_) => {
                    self.view = if self.article_search == ArticleSearch::EverySaved {
                        View::Shelf
                    } else {
                        View::Items
                    };
                    self.show(context);
                    return;
                }
                None => {}
            }
        }
        if action == action_id("retry-save") {
            if let Some(cached) = self
                .open
                .and_then(|index| self.subscriptions.get(index))
                .and_then(|feed| self.caches.get_mut(&feed.url))
            {
                cached.retry(context);
            }
            self.illustrations.retry(context);
            if self.progress.failed {
                self.progress.retry(context);
            }
            self.show(context);
            return;
        }
        if action == action_id("retry-images") {
            self.illustrations.retry(context);
            self.show(context);
            return;
        }
        if action == action_id("retry-progress") {
            self.progress.retry(context);
            self.keep_position(context);
            self.show(context);
            return;
        }
        if self.view == View::Reading && self.reader.memory().is_some() {
            match self.reader.act(context, action) {
                Some(kobo_read::Outcome::Close) | None if action == ActionId::BACK => {
                    self.keep_position(context);
                    self.illustrations.close(context);
                    self.reader.close(context);
                    self.close_article();
                }
                Some(kobo_read::Outcome::Close) => {
                    self.keep_position(context);
                    self.illustrations.close(context);
                    self.reader.close(context);
                    self.close_article();
                }
                Some(kobo_read::Outcome::Save) => self.keep_position(context),
                Some(kobo_read::Outcome::Light(level)) => {
                    context.device().set_frontlight(level);
                    self.keep_position(context);
                }
                _ => {}
            }
            self.show(context);
            return;
        }
        // The keyboard first: while it is up, it owns the panel.
        if self.view == View::Search && self.entry.is_open() {
            if action == ActionId::BACK {
                // Back from the keyboard puts it away and keeps Add a feed.
                self.entry.close();
                self.show(context);
                return;
            }
            if let Some(typing) = self.entry.handle(action) {
                if let Typing::Submitted(typed) = typing {
                    if let Some(error) = refused_address(&typed) {
                        // Nothing to fetch, so nothing to show results for:
                        // the address stays in the field with the reason.
                        self.address = typed;
                        self.address_error = Some(error);
                    } else {
                        self.address.clear();
                        self.address_error = None;
                        self.query.clone_from(&typed);
                        self.view = View::Found;
                        self.list_page = 0;
                        self.find_feed(context, &typed);
                    }
                }
                self.show(context);
                return;
            }
        }
        if self.view == View::Search && action == action_id(FEED_ADDRESS) {
            self.problem = None;
            let typed = self.address.clone();
            self.type_address(&typed);
            self.show(context);
            return;
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
            if let Some((task, _)) = self.task.take() {
                context.cancel(task);
            }
            self.problem = None;
            self.trouble = None;
            self.menu_open = None;
            match self.view {
                View::Shelf => {}
                View::SavedResults => {
                    self.article_search = ArticleSearch::ThisFeed;
                    self.article_query.clear();
                    self.saved_hits.clear();
                    self.view = View::Shelf;
                    self.list_page = 0;
                }
                // A list narrowed by a search widens again first. The
                // search screen used to carry Clear, and the reader's own
                // keyboard has no room for it.
                View::Items if !self.article_query.is_empty() => {
                    self.article_query.clear();
                    self.list_page = 0;
                }
                View::Search | View::Items => {
                    self.view = View::Shelf;
                    self.list_page = 0;
                    self.address.clear();
                    self.address_error = None;
                }
                View::Found | View::Starters => self.view = View::Search,
                View::ArticleSearch => {
                    self.search_entry.close();
                    self.view = if self.article_search == ArticleSearch::EverySaved {
                        View::Shelf
                    } else {
                        View::Items
                    };
                }
                View::Import => {
                    self.view = View::Search;
                    self.import_read = None;
                    self.import_preview = None;
                }
                View::Reading => self.close_article(),
            }
            self.show(context);
            return;
        }

        if action == action_id("add") {
            // From the results, "Change address" means typing one, so the
            // keyboard comes straight up; from the shelf the screen comes
            // first, with Import OPML and Browse beside the field.
            let typing = self.view == View::Found;
            self.problem = None;
            self.trouble = None;
            self.view = View::Search;
            self.address.clear();
            self.address_error = None;
            if typing {
                // The address that found nothing, to correct.
                let query = self.query.clone();
                self.type_address(&query);
            } else {
                self.entry.close();
            }
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

        if action == action_id("search-retry") && self.view == View::Found {
            let query = self.query.clone();
            self.find_feed(context, &query);
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

        if self.view == View::Found {
            if let Some(index) = indexed(action, "found", self.found.len()) {
                let Some(found) = self.found.get(index).cloned() else {
                    return;
                };
                if let Some(position) = self.subscribe(&found) {
                    self.save(context);
                    if self.open != Some(position) {
                        self.items.clear();
                    }
                    self.article_query.clear();
                    self.article_search = ArticleSearch::ThisFeed;
                    self.open = Some(position);
                    self.list_page = 0;
                    self.view = View::Items;
                    self.open_cached(context);
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
                if self.open != Some(index) {
                    self.items.clear();
                }
                self.article_query.clear();
                self.article_search = ArticleSearch::ThisFeed;
                self.open = Some(index);
                self.list_page = 0;
                self.view = View::Items;
                self.open_cached(context);
                self.show(context);
                return;
            }
        }

        if self.view == View::SavedResults {
            if let Some(hit) = self
                .saved_hits
                .iter()
                .find(|hit| action == action_id(&format!("hit-{}-{}", hit.feed, hit.item)))
                .cloned()
            {
                self.open = Some(hit.feed);
                self.open_cached(context);
                self.open_article(context, hit.item);
                self.show(context);
                return;
            }
        }

        if self.view == View::Items {
            if let Some(index) = indexed(action, "item", self.items.len()) {
                self.open_article(context, index);
                self.show(context);
            }
        }
    }

    fn on_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
        self.took_task(context, task, outcome);
        // A lane that was busy a moment ago may be free now.
        self.refresh_next(context);
        self.keeper.pump(context);
    }
}

impl Feeds {
    #[allow(
        clippy::too_many_lines,
        reason = "one dispatch for every task the app starts keeps the order they are tried in visible"
    )]
    fn took_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
        let heard = self.keeper.task(context, task, &outcome);
        if !matches!(heard, Heard::Elsewhere) {
            self.heard(context, heard);
            return;
        }
        if let Some((_, url)) = self.refreshing.take_if(|(id, _)| *id == task) {
            self.took_refresh(context, &url, outcome);
            self.show(context);
            return;
        }
        if self
            .illustrations
            .task(context, &mut self.reader, task, &outcome)
        {
            self.show(context);
            return;
        }
        if self.reader.woke(context, task, &outcome) != kobo_bookview::Step::Elsewhere {
            if self.view == View::Reading {
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
        let mut feed_succeeded = false;
        match outcome {
            TaskOutcome::Completed(bytes) => {
                match awaiting {
                    Awaiting::Probe => {
                        if let Some(parsed) = feed::parse(&bytes) {
                            self.found = vec![search::Found {
                                url: self.query.clone(),
                                title: if parsed.title.trim().is_empty() {
                                    pretty_host(&parsed.site, &self.query)
                                } else {
                                    parsed.title
                                },
                                site: parsed.site,
                                summary: format!("{} articles · Select to add", parsed.items.len()),
                            }];
                        } else {
                            self.problem = Some(if truncated(&bytes, FEED_BYTES) {
                            "The feed response was too large to read."
                        } else {
                            "This address did not return a feed. Enter its feed address, or search using the website name."
                        }.to_owned());
                        }
                    }
                    Awaiting::Search => match search::results(&bytes) {
                        Ok(found) => self.found = found,
                        Err(_) => {
                            self.problem = Some(if truncated(&bytes, SEARCH_BYTES) {
                                "The search response was too large. Try a more specific address."
                            } else {
                                "The search service returned an unreadable response. Try again."
                            }.to_owned());
                        }
                    },
                    Awaiting::Feed => match feed::parse(&bytes) {
                        Some(parsed) => {
                            feed_succeeded = true;
                            self.items = parsed.items;
                            if let Some(url) = self
                                .open
                                .and_then(|index| self.subscriptions.get(index))
                                .map(|feed| feed.url.clone())
                            {
                                if let Some(cached) = self.caches.get_mut(&url) {
                                    if !cached.save(context, bytes.clone()) {
                                        self.problem = Some("These articles could not be saved for offline reading.".to_owned());
                                    }
                                }
                            }
                            // A feed usually names itself better than a search
                            // result does, so the shelf takes the better name once
                            // it has been read.
                            if let Some(subscription) = self
                                .open
                                .and_then(|index| self.subscriptions.get_mut(index))
                            {
                                if !parsed.title.trim().is_empty()
                                    && subscription.title != parsed.title
                                {
                                    subscription.title = parsed.title;
                                    self.save(context);
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
                }
            }
            TaskOutcome::Failed(error) => {
                // The SDK owns the wording. Five applications wrote five
                // different sentences for the same failure before this existed.
                let failure = Failure::of(error);
                self.trouble = Some(failure);
                self.problem = Some(failure.advice.to_owned());
            }
            TaskOutcome::Cancelled => self.problem = Some("Cancelled.".to_owned()),
        }
        self.note_feed_outcome(context, awaiting, feed_succeeded);
        self.show(context);
    }
}

/// Why an address cannot be fetched as typed, before anything is asked of
/// the network: anything with a scheme other than HTTPS. A website name with
/// no scheme goes to Feedsearch instead.
fn refused_address(address: &str) -> Option<&'static str> {
    (address.contains("://") && !address.starts_with("https://"))
        .then_some("Use an HTTPS feed address.")
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
        article_text, byline, decode, encode, opml, pretty_host, search, Awaiting, Feeds,
        Subscription, View, FEEDS, FEED_BYTES, MAX_FEEDS, SEARCH_BYTES,
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
    fn failed_save_and_search_keep_the_shelf_title_visible_at_every_text_size() {
        for text_scale in kobo_ui::TextScale::STEPS {
            let metrics = kobo_sdk::DisplayMetrics {
                text_scale,
                ..CLARA_BW_METRICS
            };
            let context = AppRunner::with_metrics(Feeds::default(), metrics).context();
            for subscription_failed in [true, false] {
                let mut app = Feeds {
                    loaded: true,
                    ..Feeds::default()
                };
                app.subscription_save.failed = subscription_failed;
                app.statuses.failed = !subscription_failed;
                let screen = app.shelf(&context);
                let diagnostics = screen.diagnostics(&metrics, &kobo_sdk::Chrome::measuring(true));
                assert!(
                    diagnostics.issues.is_empty(),
                    "{text_scale:?}: {:?}",
                    diagnostics.issues
                );
                for action in [
                    if subscription_failed {
                        "retry-subscriptions"
                    } else {
                        "retry-status"
                    },
                    "search-saved",
                ] {
                    let action = action_id(action);
                    let rect = diagnostics
                        .layout
                        .rect_of_action(action)
                        .expect("the recovery action remains visible");
                    assert_eq!(
                        diagnostics
                            .layout
                            .hit_test(rect.x + rect.width / 2, rect.y + rect.height / 2),
                        Some(action)
                    );
                }
                assert!(diagnostics
                    .layout
                    .nodes
                    .iter()
                    .flat_map(|node| &node.text_lines)
                    .any(|line| line == "Feeds"));
            }
        }
    }

    /// `count` followed feeds, the first and every third one with unread
    /// articles counted, the rest read to the end.
    fn shelf_of(count: usize, context: &mut kobo_sdk::Context) -> Feeds {
        let mut app = Feeds {
            loaded: true,
            subscriptions: (0..count)
                .map(|index| Subscription {
                    url: format!("https://example.com/{index}.xml"),
                    title: format!("A journal of long and careful writing, number {index}"),
                    site: "https://example.com/".to_owned(),
                })
                .collect(),
            ..Feeds::default()
        };
        app.statuses.load(
            context,
            kobo_sdk::StoreResult::Loaded {
                key: super::status::KEY.into(),
                value: None,
            },
        );
        for (index, feed) in app.subscriptions.iter().enumerate() {
            app.statuses
                .note(context, &feed.url, Ok("2026-09-09 08:30".into()));
            let unread = if index % 3 == 0 { 12 } else { 0 };
            app.statuses.unread(context, &feed.url, unread, 20);
        }
        app
    }

    fn rows_of(screen: &kobo_sdk::Screen) -> Vec<kobo_ui::Row> {
        screen
            .nodes
            .iter()
            .find_map(|node| match node {
                kobo_sdk::Node::Rows { rows, .. } => Some(rows.clone()),
                _ => None,
            })
            .unwrap_or_default()
    }

    fn banners_of(
        screen: &kobo_sdk::Screen,
    ) -> Vec<(kobo_sdk::BannerLevel, Option<(kobo_sdk::ActionId, String)>)> {
        screen
            .nodes
            .iter()
            .filter_map(|node| match node {
                kobo_sdk::Node::Banner { level, action, .. } => Some((
                    *level,
                    action
                        .as_ref()
                        .map(|action| (action.action, action.label.clone())),
                )),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_feeds_unread_count_is_a_badge_at_the_rows_edge_and_not_words_in_its_summary() {
        let mut context = kobo_sdk::Context::default();
        let app = shelf_of(2, &mut context);
        let rows = rows_of(&app.shelf(&context));
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].badge, Some(12));
        assert_eq!(rows[1].badge, None, "no badge for a feed read to the end");
        for row in &rows {
            assert!(!row.summary.contains("unread"), "{:?}", row.summary);
            assert!(row.summary.contains("Updated"), "{:?}", row.summary);
        }
    }

    #[test]
    fn a_failed_write_carries_retry_saving_on_its_banner_and_the_arrows_only_refresh_all() {
        let mut context = kobo_sdk::Context::default();
        for subscriptions_failed in [true, false] {
            let mut app = shelf_of(2, &mut context);
            app.subscription_save.failed = subscriptions_failed;
            app.statuses.failed = !subscriptions_failed;
            let retry = if subscriptions_failed {
                "retry-subscriptions"
            } else {
                "retry-status"
            };
            let screen = app.shelf(&context);
            assert_eq!(
                banners_of(&screen),
                [(
                    kobo_sdk::BannerLevel::Error,
                    Some((action_id(retry), "Retry saving".to_owned()))
                )]
            );
            let bar = format!("{:?}", screen.top_bar);
            assert!(bar.contains("Refresh all"), "{bar}");
            assert!(!bar.contains("Retry saving"), "{bar}");
        }
    }

    #[test]
    fn an_http_address_comes_back_in_the_field_with_its_reason_and_fetches_nothing() {
        let mut runner = AppRunner::new(Feeds {
            loaded: true,
            ..Feeds::default()
        });
        runner.action(action_id("add"));
        runner.action(action_id(super::FEED_ADDRESS));
        let commands = runner.type_text("http://example.com/feed.xml");
        assert!(!commands
            .iter()
            .any(|command| matches!(command, Command::Spawn { .. })));
        assert_eq!(runner.app().view, View::Search);
        let screen = screen_of(&commands);
        let field = screen
            .nodes
            .iter()
            .find_map(|node| match node {
                kobo_sdk::Node::Field {
                    value, hint, error, ..
                } => Some((value.clone(), hint.clone(), error.clone())),
                _ => None,
            })
            .expect("the Add screen has its field");
        assert_eq!(
            field,
            (
                "http://example.com/feed.xml".to_owned(),
                Some(super::ATTRIBUTION.to_owned()),
                Some("Use an HTTPS feed address.".to_owned())
            )
        );
        // Tapping the field opens it on the refused address, to correct.
        runner.action(action_id(super::FEED_ADDRESS));
        assert_eq!(runner.app().entry.text(), "http://example.com/feed.xml");
        let commands = runner.type_text("https://example.com/feed.xml");
        assert_eq!(runner.app().view, View::Found);
        assert!(commands
            .iter()
            .any(|command| matches!(command, Command::Spawn { .. })));
    }

    #[test]
    fn a_failed_fetch_keeps_the_results_with_its_fix_on_the_banner() {
        let mut runner = AppRunner::new(Feeds {
            loaded: true,
            view: View::Found,
            query: "example.com".to_owned(),
            task: Some((TaskId(1), Awaiting::Search)),
            ..Feeds::default()
        });
        let commands = runner.task_outcome(
            TaskId(1),
            TaskOutcome::Failed(kobo_sdk::TaskError::Unreachable),
        );
        let screen = screen_of(&commands);
        assert_eq!(
            banners_of(&screen),
            [(
                kobo_sdk::BannerLevel::Attention,
                Some((action_id("search-retry"), "Try again".to_owned()))
            )]
        );
        assert!(!screen.nodes.iter().any(|node| matches!(
            node,
            kobo_sdk::Node::Button { action, .. } if *action == action_id("search-retry")
        )));
        // Off the network the fix is the Wi-Fi, not asking again.
        let offline = kobo_sdk::Failure::of(kobo_sdk::TaskError::Offline);
        let app = Feeds {
            loaded: true,
            view: View::Found,
            query: "example.com".to_owned(),
            problem: Some(offline.advice.to_owned()),
            trouble: Some(offline),
            ..Feeds::default()
        };
        assert_eq!(
            banners_of(&app.results(&kobo_sdk::Context::default())),
            [(
                kobo_sdk::BannerLevel::Attention,
                Some((action_id(kobo_sdk::JOIN_WIFI), "Join Wi-Fi".to_owned()))
            )]
        );
    }

    /// The shelf under each banner it can carry, with badges on some rows,
    /// and the Add screen with a refused address, on a Clara at every text
    /// size held either way. Every page of the shelf draws every row it was
    /// measured for, and none runs under the bar.
    #[test]
    fn the_shelf_and_a_refused_address_fit_a_clara_at_every_text_size_held_either_way() {
        for text_scale in kobo_ui::TextScale::STEPS {
            for orientation in [
                kobo_ui::Orientation::Portrait,
                kobo_ui::Orientation::Landscape,
            ] {
                let metrics = kobo_sdk::DisplayMetrics {
                    text_scale,
                    ..CLARA_BW_METRICS
                }
                .oriented(orientation);
                let runner = AppRunner::with_metrics(Feeds::default(), metrics);
                let mut context = runner.context();
                for banner in 0..4 {
                    let mut app = shelf_of(9, &mut context);
                    app.subscription_save.failed = banner == 0;
                    app.statuses.failed = banner == 1;
                    if banner == 2 {
                        app.problem = Some(
                            "Saved articles could not be read or updated. Your open articles \
                             are still available."
                                .to_owned(),
                        );
                    }
                    let mut page = 0;
                    let mut seen = 0;
                    loop {
                        app.list_page = page;
                        let screen = app.shelf(&context);
                        let diagnostics = screen.diagnostics(&metrics, &Chrome::measuring(true));
                        assert!(
                            diagnostics.issues.is_empty(),
                            "banner {banner} page {page} {text_scale:?} {orientation:?}: {:?}",
                            diagnostics.issues
                        );
                        let rows = rows_of(&screen).len();
                        let drawn = diagnostics
                            .layout
                            .nodes
                            .iter()
                            .filter(|node| matches!(node.kind, LayoutKind::Row(_)))
                            .count();
                        assert_eq!(drawn, rows, "banner {banner} page {page}");
                        seen += rows;
                        if seen >= app.subscriptions.len() || rows == 0 {
                            break;
                        }
                        page += 1;
                    }
                    assert_eq!(seen, app.subscriptions.len(), "every feed is on a page");
                }
                let add = Feeds {
                    loaded: true,
                    view: View::Search,
                    address: "http://example.com/a/rather/long/path/to/the/feed.xml".to_owned(),
                    address_error: Some("Use an HTTPS feed address."),
                    ..Feeds::default()
                };
                let diagnostics = add.search().diagnostics(&metrics, &Chrome::measuring(true));
                assert!(
                    diagnostics.issues.is_empty(),
                    "add {text_scale:?} {orientation:?}: {:?}",
                    diagnostics.issues
                );
            }
        }
    }

    #[test]
    fn starter_feeds_require_a_choice_then_a_preview_before_subscribing() {
        for (index, (_, _, url)) in super::STARTER_FEEDS.iter().enumerate() {
            let mut runner = AppRunner::new(Feeds {
                loaded: true,
                ..Feeds::default()
            });
            let browsing = runner.action(action_id("browse-feeds"));
            assert!(!browsing
                .iter()
                .any(|command| matches!(command, Command::Spawn { .. })));
            fits_the_panel(&screen_of(&browsing), "starter feeds");
            let commands = runner.action(action_id(&format!("starter-{index}")));
            assert!(commands.iter().any(|command| matches!(command, Command::Spawn { work: kobo_sdk::Task::Fetch { url: target, credential: None, .. }, .. } if target == url)));
            assert!(runner.app().subscriptions.is_empty());
            let task = runner.app().task.unwrap().0;
            runner.task_outcome(task, TaskOutcome::Completed(ATOM.as_bytes().to_vec()));
            assert!(runner.app().subscriptions.is_empty());
            assert_eq!(runner.app().found.len(), 1);
            runner.action(action_id("found-0"));
            assert_eq!(runner.app().subscriptions[0].url, *url);
        }
    }

    #[test]
    fn failed_feed_snapshot_offers_a_retry_that_reloads_the_published_pointer() {
        use kobo_sdk::{StoreError, StoreResult};
        let mut runner = AppRunner::new(Feeds {
            loaded: true,
            subscriptions: following(),
            ..Feeds::default()
        });
        runner.action(action_id("feed-0"));
        let key = kobo_net::sha256::hex_digest(following()[0].url.as_bytes());
        runner.store_result(StoreResult::Loaded {
            key: key.clone(),
            value: None,
        });
        let task = runner.app().task.unwrap().0;
        runner.task_outcome(task, TaskOutcome::Completed(ATOM.as_bytes().to_vec()));
        let failure = runner.store_result(StoreResult::Denied(StoreError::NoRoom));
        assert!(runner.app().feed_save_failed());
        assert!(format!("{:?}", screen_of(&failure)).contains("Retry saving"));
        let retry = runner.action(action_id("retry-save"));
        assert!(retry.iter().any(|command| matches!(command, Command::Store(kobo_sdk::StoreRequest::Load { key: retry_key }) if retry_key == &key)));
        assert!(!retry.iter().any(|command| matches!(
            command,
            Command::Spawn {
                work: kobo_sdk::Task::Fetch { .. },
                ..
            }
        )));
    }

    #[test]
    fn direct_feed_addresses_are_probed_without_discovery_or_early_subscription() {
        for body in [ATOM.as_bytes(),
            br"<rss><channel><title>Journal</title><item><title>Post</title><description>Body</description></item></channel></rss>",
            br#"{"version":"https://jsonfeed.org/version/1.1","title":"Journal","items":[{"id":"1","title":"Post","content_text":"Body"}]}"#] {
            let address = "https://example.com/private/feed?key=sample";
            let mut runner = AppRunner::new(Feeds {
                loaded: true, view: View::Found, query: address.to_owned(),
                ..Feeds::default()
            });
            let commands = runner.action(action_id("search-retry"));
            assert!(commands.iter().any(|command| matches!(command,
                Command::Spawn { work: kobo_sdk::Task::Fetch { url, .. }, .. } if url == address)));
            let task = runner.app_mut().task.unwrap().0;
            let commands = runner.task_outcome(task, TaskOutcome::Completed(body.to_vec()));
            assert!(runner.app_mut().subscriptions.is_empty());
            assert_eq!(runner.app_mut().found.len(), 1);
            assert_eq!(runner.app_mut().found[0].url, address);
            let screen = screen_of(&commands);
            assert!(!format!("{screen:?}").contains("feedsearch.dev"));
            fits_the_panel(&screen, "direct feed preview");
            runner.action(action_id("found-0"));
            assert_eq!(runner.app_mut().subscriptions.len(), 1);
        }
    }

    #[test]
    fn leaving_discovery_cancels_the_request_and_ignores_its_late_answer() {
        let mut runner = AppRunner::new(Feeds {
            loaded: true,
            view: View::Found,
            query: "https://example.com/feed".to_owned(),
            ..Feeds::default()
        });
        runner.action(action_id("search-retry"));
        let task = runner.app_mut().task.unwrap().0;
        runner.action(kobo_sdk::ActionId::BACK);
        assert!(runner.app_mut().task.is_none());
        runner.task_outcome(task, TaskOutcome::Completed(ATOM.as_bytes().to_vec()));
        assert!(runner.app_mut().found.is_empty());
        assert_eq!(runner.app_mut().view, View::Search);
    }

    #[test]
    fn typing_an_address_asks_feedsearch_for_exactly_that_address() {
        let mut runner = AppRunner::new(Feeds {
            loaded: true,
            ..Feeds::default()
        });
        runner.action(action_id("add"));
        runner.action(action_id(super::FEED_ADDRESS));
        let commands = runner.type_text("paw");
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

    fn acknowledge_new_feed(runner: &mut AppRunner<Feeds>) {
        runner.store_result(kobo_sdk::StoreResult::Saved {
            key: FEEDS.to_owned(),
        });
        let key = runner.app_mut().caches.values().next().unwrap().key.clone();
        runner.store_result(kobo_sdk::StoreResult::Loaded { key, value: None });
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
        acknowledge_new_feed(&mut runner);
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
    fn failed_refresh_keeps_articles_but_switching_feeds_clears_them() {
        let mut subscriptions = following();
        subscriptions.push(Subscription {
            url: "https://other.example/feed".to_owned(),
            title: "Other feed".to_owned(),
            site: String::new(),
        });
        let mut runner = AppRunner::new(Feeds {
            loaded: true,
            view: View::Items,
            open: Some(0),
            subscriptions,
            items: super::feed::parse(ATOM.as_bytes()).unwrap().items,
            ..Feeds::default()
        });
        runner.action(action_id("refresh"));
        assert_eq!(runner.app_mut().items.len(), 1);
        let task = runner.app_mut().task.unwrap().0;
        runner.task_outcome(task, TaskOutcome::Failed(kobo_sdk::TaskError::Offline));
        assert_eq!(runner.app_mut().items.len(), 1);
        runner.action(kobo_sdk::ActionId::BACK);
        runner.action(action_id("feed-1"));
        assert!(runner.app_mut().items.is_empty());
    }

    #[test]
    fn failed_discovery_offers_retry_without_claiming_no_feeds() {
        let mut runner = AppRunner::new(Feeds {
            loaded: true,
            view: View::Found,
            query: "example.com".to_owned(),
            task: Some((TaskId(1), Awaiting::Search)),
            ..Feeds::default()
        });
        let commands = runner.task_outcome(
            TaskId(1),
            TaskOutcome::Completed(b"<html>Service unavailable</html>".to_vec()),
        );
        let text = text_of(&screen_of(&commands));
        assert!(
            text.iter().any(|line| line.contains("Try again")),
            "{text:?}"
        );
        assert!(
            !text.iter().any(|line| line.contains("No feeds there")),
            "{text:?}"
        );
        runner.action(action_id("search-retry"));
        assert!(runner.app_mut().awaiting(Awaiting::Search));
        assert!(runner.app_mut().problem.is_none());
        assert_eq!(runner.app_mut().query, "example.com");
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
    fn html_articles_request_images_and_cancel_them_when_closed() {
        let mut runner = AppRunner::new(Feeds {
            loaded: true, view: View::Items, open: Some(0), subscriptions: following(),
            items: vec![super::feed::Item {
                title: "An illustrated article".to_owned(),
                link: "https://example.com/story/".to_owned(),
                html: r#"<p>Story text.</p><figure><img src="/photo.png" alt="A mountain"/><figcaption>Morning light</figcaption></figure>"#.to_owned(),
                body: "Story text.".to_owned(), ..super::feed::Item::default()
            }], ..Feeds::default()
        });
        let commands = runner.action(action_id("item-0"));
        assert!(!commands.iter().any(|command| matches!(
            command,
            Command::Spawn {
                work: kobo_sdk::Task::Fetch { .. },
                ..
            }
        )));
        let key = kobo_net::sha256::hex_digest(b"rss-image:https://example.com/photo.png");
        let commands = runner.store_result(kobo_sdk::StoreResult::Loaded { key, value: None });
        let task = commands
            .iter()
            .find_map(|command| match command {
                Command::Spawn {
                    task,
                    work:
                        kobo_sdk::Task::Fetch {
                            url, credential, ..
                        },
                } if url == "https://example.com/photo.png" && credential.is_none() => Some(*task),
                _ => None,
            })
            .expect("article image requested without credentials");
        assert!(runner.app_mut().reader.memory().is_some());
        let commands = runner.action(kobo_sdk::ActionId::BACK);
        assert!(commands
            .iter()
            .any(|command| matches!(command, Command::Cancel(id) if *id == task)));
        assert_eq!(runner.app_mut().view, View::Items);
        assert!(runner.app_mut().reader.memory().is_none());
        runner.task_outcome(task, TaskOutcome::Completed(Vec::new()));
        assert_eq!(runner.app_mut().view, View::Items);
    }

    #[test]
    fn long_reading_sessions_continue_loading_new_images_and_can_revisit_old_ones() {
        let mut runner = AppRunner::new(Feeds {
            loaded: true,
            view: View::Items,
            open: Some(0),
            subscriptions: following(),
            ..Feeds::default()
        });
        for index in (0..128).chain(std::iter::once(0)) {
            let url = format!("https://example.com/image-{index}.png");
            runner.app_mut().items = vec![super::feed::Item {
                title: format!("Article {index}"),
                link: "https://example.com/story/".to_owned(),
                html: format!("<p>Article text.</p><img src=\"{url}\" alt=\"A landscape\"/>"),
                ..super::feed::Item::default()
            }];
            let commands = runner.action(action_id("item-0"));
            let key = kobo_net::sha256::hex_digest(format!("rss-image:{url}").as_bytes());
            assert!(
                commands.iter().any(|command| matches!(command,
                Command::Store(kobo_sdk::StoreRequest::Load { key: loaded }) if loaded == &key)),
                "image {index} did not check its saved copy"
            );
            let commands = runner.store_result(kobo_sdk::StoreResult::Loaded { key, value: None });
            assert!(commands.iter().any(|command| matches!(command,
                Command::Spawn { work: kobo_sdk::Task::Fetch { url: requested, .. }, .. } if requested == &url)),
                "image {index} could not be fetched");
            let closing = runner.action(kobo_sdk::ActionId::BACK);
            for command in closing {
                if let Command::Cancel(task) = command {
                    runner.task_outcome(task, TaskOutcome::Cancelled);
                }
            }
        }
    }

    #[test]
    fn a_verified_saved_image_is_opened_without_a_network_request() {
        let mut runner = AppRunner::new(Feeds {
            loaded: true,
            view: View::Items,
            items: vec![super::feed::Item {
                title: "Illustrated".into(),
                link: "https://example.com/story".into(),
                html: r#"<p>Story.</p><img src="/photo.png" alt="A ridge"/>"#.into(),
                ..super::feed::Item::default()
            }],
            ..Feeds::default()
        });
        let bytes = kobo_image::encode_png_grey(2, 2, &[0, 255, 255, 0]).unwrap();
        let key = kobo_net::sha256::hex_digest(b"rss-image:https://example.com/photo.png");
        let commands = runner.action(action_id("item-0"));
        assert!(!commands.iter().any(|c| matches!(
            c,
            Command::Spawn {
                work: kobo_sdk::Task::Fetch { .. },
                ..
            }
        )));
        runner.store_result(kobo_sdk::StoreResult::Loaded {
            key: key.clone(),
            value: Some(format!("1:{}", kobo_net::sha256::hex_digest(&bytes)).into_bytes()),
        });
        let commands = runner.store_result(kobo_sdk::StoreResult::ShelfRead {
            name: format!("{}.1", &key[..60]),
            offset: 0,
            size: u32::try_from(bytes.len()).unwrap(),
            bytes,
        });
        assert!(!commands.iter().any(|c| matches!(
            c,
            Command::Spawn {
                work: kobo_sdk::Task::Fetch { .. },
                ..
            }
        )));
        assert!(!runner.app_mut().illustrations.failed);
        assert!(runner.app_mut().reader.memory().is_some());
    }

    #[test]
    fn a_full_article_list_reserves_space_for_save_recovery() {
        let mut app = Feeds {
            loaded: true,
            view: View::Items,
            open: Some(0),
            subscriptions: following(),
            items: (0..50)
                .map(|index| super::feed::Item {
                    title: format!(
                        "A longer article headline about a walk by the river, number {index}"
                    ),
                    body: "An introductory paragraph about the article and its subject.".into(),
                    ..super::feed::Item::default()
                })
                .collect(),
            ..Feeds::default()
        };
        app.progress.failed = true;
        let mut runner = AppRunner::new(app);
        let screen = screen_of(&runner.action(action_id("list-back")));
        fits_the_panel(&screen, "full list with save recovery");
        assert!(format!("{screen:?}").contains("Retry saving"));
        assert!(screen.page_turns.is_some());
    }

    #[test]
    fn unreadable_subscriptions_block_writes_until_a_successful_reload() {
        use kobo_sdk::{StoreError, StoreResult};
        for result in [
            StoreResult::Loaded {
                key: FEEDS.into(),
                value: Some(vec![255]),
            },
            StoreResult::Denied(StoreError::Unwritable),
        ] {
            let mut runner = AppRunner::new(Feeds::default());
            runner.start();
            runner.store_result(result);
            runner.store_result(StoreResult::Loaded {
                key: super::progress::KEY.into(),
                value: None,
            });
            runner.store_result(StoreResult::Loaded {
                key: super::status::KEY.into(),
                value: None,
            });
            runner.store_result(StoreResult::Loaded {
                key: kobo_bookview::offline::INDEX_KEY.into(),
                value: None,
            });
            assert!(runner.app().subscription_save.load_failed);
            for action in ["add", "retry-subscriptions", "import-confirm"] {
                let commands = runner.action(action_id(action));
                assert!(!commands.iter().any(|command| matches!(
                    command,
                    Command::Store(kobo_sdk::StoreRequest::Save { .. })
                )));
            }
            let retry = runner.action(action_id("retry-load-subscriptions"));
            assert!(retry.iter().any(|command| matches!(command, Command::Store(kobo_sdk::StoreRequest::Load { key }) if key == FEEDS)));
            runner.store_result(StoreResult::Loaded {
                key: FEEDS.into(),
                value: Some(encode(&following())),
            });
            assert!(!runner.app().subscription_save.load_failed);
            assert_eq!(runner.app().subscriptions, following());
        }
    }

    #[test]
    fn subscription_writes_are_serial_and_retry_the_latest_list() {
        use kobo_sdk::{Context, KoboApp, StoreError, StoreResult};
        let mut app = Feeds {
            loaded: true,
            subscriptions: following(),
            ..Feeds::default()
        };
        let mut context = Context::default();
        app.save(&mut context);
        let first = app.subscription_save.writing.clone();
        app.subscriptions[0].title = "Revised title".into();
        app.save(&mut context);
        app.subscriptions.push(Subscription {
            url: "https://example.com/new.xml".into(),
            title: "New journal".into(),
            site: String::new(),
        });
        app.save(&mut context);
        let latest = encode(&app.subscriptions);
        assert_eq!(app.subscription_save.writing, first);
        assert_eq!(app.subscription_save.queued.as_ref(), Some(&latest));
        app.on_save(
            &mut context,
            FEEDS,
            StoreResult::Saved { key: FEEDS.into() },
        );
        assert_eq!(app.subscription_save.writing.as_ref(), Some(&latest));
        assert!(app.subscription_save.queued.is_none());
        app.on_save(
            &mut context,
            FEEDS,
            StoreResult::Denied(StoreError::TooFull),
        );
        assert!(app.subscription_save.failed);
        assert!(app.subscription_save.writing.is_none());
        app.on_action(&mut context, action_id("retry-subscriptions"));
        assert_eq!(app.subscription_save.writing.as_ref(), Some(&latest));
        app.on_save(
            &mut context,
            FEEDS,
            StoreResult::Saved { key: FEEDS.into() },
        );
        assert!(!app.subscription_save.failed);
    }

    #[test]
    fn an_earlier_save_acknowledgement_cannot_complete_an_import() {
        use kobo_sdk::{Context, KoboApp, StoreResult};
        let mut app = Feeds {
            loaded: true,
            view: View::Import,
            ..Feeds::default()
        };
        let mut context = Context::default();
        app.save(&mut context);
        let candidate = following();
        app.save_subscriptions(&mut context, encode(&candidate));
        app.import_pending = Some(candidate.clone());
        app.on_save(
            &mut context,
            FEEDS,
            StoreResult::Saved { key: FEEDS.into() },
        );
        assert!(app.subscriptions.is_empty());
        assert_eq!(app.import_pending.as_ref(), Some(&candidate));
        app.on_save(
            &mut context,
            FEEDS,
            StoreResult::Saved { key: FEEDS.into() },
        );
        assert_eq!(app.subscriptions, candidate);
        assert!(app.import_pending.is_none());
    }

    #[test]
    fn opml_selection_limits_the_saved_list_and_waits_for_acknowledgement() {
        let mut runner = AppRunner::new(Feeds {
            view: View::Import,
            loaded: true,
            import_preview: Some(opml::Import {
                feeds: (0..41)
                    .map(|i| Subscription {
                        title: format!("Journal {i}"),
                        url: format!("https://example.com/feed-{i}.xml"),
                        site: String::new(),
                    })
                    .collect(),
                skipped: 0,
            }),
            ..Feeds::default()
        });
        runner.action(action_id("import-confirm"));
        assert!(runner.app().import_pending.is_none());
        let preview = screen_of(&runner.action(action_id("import-toggle-0")));
        fits_the_panel(&preview, "OPML selection");
        runner.action(action_id("import-confirm"));
        let pending = runner
            .app()
            .import_pending
            .as_ref()
            .expect("selected subscriptions queued");
        assert_eq!(pending.len(), 40);
        assert!(pending.iter().all(|feed| !feed.url.ends_with("feed-0.xml")));
        assert!(runner.app().subscriptions.is_empty());
    }

    /// Forty-one feeds from a file with room for forty: every page fits at
    /// every text size with the bar under it, and the bar offers Add only
    /// once what is ticked fits.
    #[test]
    fn feeds_to_import_are_ticked_in_boxes_and_counted_by_the_bar_that_adds_them() {
        let preview = || opml::Import {
            feeds: (0..41)
                .map(|i| Subscription {
                    title: format!("A journal with a long enough name to wrap {i}"),
                    url: format!("https://example.com/feed-{i}.xml"),
                    site: format!("https://journal-{i}.example.com/"),
                })
                .collect(),
            skipped: 0,
        };
        let bar = |screen: &kobo_sdk::Screen| {
            screen
                .nodes
                .iter()
                .find_map(|node| match node {
                    kobo_sdk::Node::SelectionBar { count, actions, .. } => {
                        Some((*count, actions.len()))
                    }
                    _ => None,
                })
                .expect("a selection bar")
        };
        for text_scale in kobo_ui::TextScale::STEPS {
            let metrics = kobo_sdk::DisplayMetrics {
                text_scale,
                ..CLARA_BW_METRICS
            };
            let mut runner = AppRunner::with_metrics(
                Feeds {
                    view: View::Import,
                    loaded: true,
                    import_preview: Some(preview()),
                    ..Feeds::default()
                },
                metrics,
            );
            let context = runner.context();
            let first = runner.app().import_screen(&context);
            assert_eq!(bar(&first), (41, 0), "Add was offered past the limit");
            let after = screen_of(&runner.action(action_id("import-toggle-0")));
            assert_eq!(bar(&after), (40, 1));
            let states: Vec<_> = after
                .nodes
                .iter()
                .find_map(|node| match node {
                    kobo_sdk::Node::Rows { rows, .. } => {
                        Some(rows.iter().map(|row| row.state).collect::<Vec<_>>())
                    }
                    _ => None,
                })
                .expect("the feeds");
            assert_eq!(states[0], kobo_sdk::RowState::Unchecked);
            assert!(states[1..]
                .iter()
                .all(|state| *state == kobo_sdk::RowState::Checked));
            // A page past the last is drawn as the last, so every page is
            // reached by counting to the number of feeds.
            for page in 0..41 {
                runner.app_mut().list_page = page;
                let screen = runner.app().import_screen(&context);
                let issues = screen.validate(&metrics);
                assert!(issues.is_empty(), "{text_scale:?} page {page}: {issues:?}");
            }
        }
    }

    #[test]
    fn an_empty_opml_selection_does_not_write_subscriptions() {
        let mut runner = AppRunner::new(Feeds {
            view: View::Import,
            loaded: true,
            import_preview: Some(opml::Import {
                feeds: following(),
                skipped: 0,
            }),
            ..Feeds::default()
        });
        for index in 0..following().len() {
            runner.action(action_id(&format!("import-toggle-{index}")));
        }
        let commands = runner.action(action_id("import-confirm"));
        assert!(runner.app().import_pending.is_none());
        assert!(!commands
            .iter()
            .any(|command| matches!(command, Command::Store(kobo_sdk::StoreRequest::Save { .. }))));
    }

    #[test]
    fn opening_an_article_persists_read_status_and_new_content_is_unread() {
        let items = super::feed::parse(ATOM.as_bytes()).unwrap().items;
        let mut app = Feeds {
            loaded: true,
            view: View::Items,
            open: Some(0),
            subscriptions: following(),
            items: items.clone(),
            ..Feeds::default()
        };
        app.progress.load(kobo_sdk::StoreResult::Loaded {
            key: super::progress::KEY.into(),
            value: None,
        });
        assert_eq!(app.article_was_read(&items[0]), Some(false));
        let mut runner = AppRunner::new(app);
        let commands = runner.action(action_id("item-0"));
        assert_eq!(runner.app().article_was_read(&items[0]), Some(true));
        let bytes = commands
            .iter()
            .find_map(|command| match command {
                Command::Store(kobo_sdk::StoreRequest::Save { key, value })
                    if key == super::progress::KEY =>
                {
                    Some(value.clone())
                }
                _ => None,
            })
            .expect("opening the article saves its read status");
        let mut restored = Feeds {
            open: Some(0),
            subscriptions: following(),
            ..Feeds::default()
        };
        restored.progress.load(kobo_sdk::StoreResult::Loaded {
            key: super::progress::KEY.into(),
            value: Some(bytes),
        });
        assert_eq!(restored.article_was_read(&items[0]), Some(true));
        let mut revised = items[0].clone();
        revised.body.push_str(" A correction.");
        revised.html.clear();
        assert_eq!(restored.article_was_read(&revised), Some(false));
    }

    /// Searching what is saved means every subscription, not the opened ones.
    ///
    /// The saved copies are read one feed at a time as feeds are opened, so a
    /// search made from the shelf before opening anything was looking at an
    /// empty set and answering that the words appear nowhere.
    ///
    /// Back from one of its results returns to the results. The reader is
    /// closed by a path that only knew about a feed's own article list, so it
    /// used to land on the articles of whichever feed happened to hold the
    /// result, with every other feed's matches gone.
    #[test]
    fn searching_every_saved_subscription_covers_feeds_that_were_never_opened() {
        let feed = |title: &str, body: &str| {
            format!(
                "<rss><channel><title>{title}</title><item><title>{title} piece</title>\
                 <description>{body}</description></item></channel></rss>"
            )
            .into_bytes()
        };
        let mut app = Feeds {
            loaded: true,
            subscriptions: vec![
                Subscription {
                    url: "https://one.example/feed".into(),
                    title: "One".into(),
                    site: "one.example".into(),
                },
                Subscription {
                    url: "https://two.example/feed".into(),
                    title: "Two".into(),
                    site: "two.example".into(),
                },
            ],
            ..Feeds::default()
        };
        for (url, body) in [
            ("https://one.example/feed", feed("One", "nothing here")),
            (
                "https://two.example/feed",
                feed("Two", "a word about quartz"),
            ),
        ] {
            let mut cache = super::cache::Cache::new(url);
            cache.bytes = Some(body);
            app.caches.insert(url.to_owned(), cache);
        }
        let mut runner = AppRunner::new(app);
        runner.action(action_id("search-saved"));
        let commands = runner.type_text("quartz");
        assert_eq!(runner.app_mut().view, View::SavedResults);
        assert_eq!(
            runner
                .app_mut()
                .saved_hits
                .iter()
                .map(|hit| hit.feed)
                .collect::<Vec<_>>(),
            vec![1],
            "a subscription nobody had opened was left out of the search"
        );
        fits_the_panel(&screen_of(&commands), "saved search results");
        runner.action(action_id("hit-1-0"));
        assert_eq!(runner.app_mut().view, View::Reading);
        runner.action(kobo_sdk::ActionId::BACK);
        assert_eq!(
            runner.app_mut().view,
            View::SavedResults,
            "back from a result left the search it came from"
        );
        assert_eq!(runner.app_mut().saved_hits.len(), 1);
    }

    #[test]
    fn saved_article_search_matches_text_and_keeps_original_indices() {
        let mut runner = AppRunner::new(Feeds {
            loaded: true,
            view: View::Items,
            items: vec![
                super::feed::Item {
                    title: "Tea".into(),
                    ..super::feed::Item::default()
                },
                super::feed::Item {
                    title: "A walk".into(),
                    author: "Jo".into(),
                    body: "Over the old BRIDGE.".into(),
                    ..super::feed::Item::default()
                },
            ],
            ..Feeds::default()
        });
        runner.action(action_id("search-articles"));
        let commands = runner.type_text(" bridge JO ");
        assert_eq!(runner.app_mut().matching_items(), vec![1]);
        assert!(!commands
            .iter()
            .any(|command| matches!(command, Command::Spawn { .. })));
        let screen = screen_of(&commands);
        fits_the_panel(&screen, "saved search results");
        runner.action(action_id("item-1"));
        assert_eq!(runner.app_mut().article, Some(1));
        runner.action(kobo_sdk::ActionId::BACK);
        assert_eq!(runner.app_mut().article_query, "bridge JO");
        runner.action(action_id("search-articles"));
        let commands = runner.type_text("quartz");
        assert!(runner.app_mut().matching_items().is_empty());
        assert!(format!("{:?}", screen_of(&commands)).contains("No saved articles match"));
        // Back widens a narrowed list before it leaves the feed.
        runner.action(kobo_sdk::ActionId::BACK);
        assert_eq!(runner.app_mut().view, View::Items);
        assert_eq!(runner.app_mut().matching_items(), vec![0, 1]);
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
        let source = format!(
            "<rss><channel><title>A Journal</title><item><title>Long</title><description>{long}</description></item></channel></rss>"
        );
        runner.task_outcome(TaskId(1), TaskOutcome::Completed(source.into_bytes()));
        runner.action(action_id("item-0"));
        let application = runner.app_mut();
        assert_eq!(application.view, View::Reading);
        assert!(
            application.reader.reader().unwrap().page_count() > 1,
            "the article fitted one page"
        );
        assert_eq!(application.reader.memory().unwrap().at, 0);
    }

    #[test]
    fn plain_text_articles_use_shared_pagination_and_restore_their_position() {
        let mut runner = AppRunner::new(Feeds {
            loaded: true,
            view: View::Items,
            open: Some(0),
            subscriptions: following(),
            items: vec![super::feed::Item {
                title: "Plain text".to_owned(),
                body: "Write <img> literally, then keep reading. ".repeat(180),
                ..super::feed::Item::default()
            }],
            ..Feeds::default()
        });
        runner
            .app_mut()
            .progress
            .load(kobo_sdk::StoreResult::Loaded {
                key: super::progress::KEY.into(),
                value: None,
            });
        runner.action(action_id("item-0"));
        assert!(runner.app_mut().reader.reader().unwrap().page_count() > 2);
        assert!(runner
            .app_mut()
            .reader
            .reader()
            .unwrap()
            .page()
            .iter()
            .any(|piece| piece.text.contains("<img>")));
        runner.action(action_id(kobo_read::action::FORWARD));
        let at = runner.app_mut().reader.memory().unwrap().at;
        assert!(at > 0);
        runner.store_result(kobo_sdk::StoreResult::Saved {
            key: super::progress::KEY.into(),
        });
        runner.action(kobo_sdk::ActionId::BACK);
        runner.action(action_id("item-0"));
        assert_eq!(runner.app_mut().reader.memory().unwrap().at, at);
        fits_the_panel(&runner.app_mut().reading(), "restored plain-text article");
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
        let read = decode(&encode(&feeds)).unwrap();
        assert_eq!(read.len(), 2);
        assert_eq!(read[0], feeds[0]);
        assert_eq!(read[1].url, feeds[1].url);
        assert_eq!(read[1].title, "Another one entirely");
    }

    #[test]
    fn a_damaged_list_is_refused_without_discarding_entries() {
        let read = decode(b"\n\thttps://a.example/feed\nhttps://b.example/feed\t\t\n\n");
        assert!(read.is_err());
        let valid = decode(b"\nhttps://b.example/feed\t\t\n\n").unwrap();
        assert_eq!(valid[0].title, "b.example");
    }

    #[test]
    fn an_oversized_subscription_list_is_not_silently_truncated() {
        let feeds: Vec<Subscription> = (0..MAX_FEEDS + 10)
            .map(|index| Subscription {
                url: format!("https://example.com/{index}"),
                title: format!("Feed {index}"),
                site: String::new(),
            })
            .collect();
        assert!(decode(&encode(&feeds)).is_err());
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
            html: String::new(),
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
            (kobo_sdk::TaskError::TimedOut, "took too long"),
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

        // Adding a feed: the field with Import OPML and Browse beside it,
        // and the attribution under it.
        fits_the_panel(
            &screen_of(&runner.action(action_id("add"))),
            "the search screen",
        );
        runner.action(action_id(super::FEED_ADDRESS));
        fits_the_panel(&screen_of(&runner.type_text("paw")), "the search in flight");

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
        acknowledge_new_feed(&mut runner);
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
        let pages = runner.app_mut().reader.reader_mut().unwrap().page_count();
        assert!(pages > 1, "the long article fitted a single page");
        for page in 1..pages {
            let commands = runner.action(action_id(kobo_read::action::FORWARD));
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

        runner.action(action_id(super::FEED_ADDRESS));
        // The results screen, while the search is still in flight.
        let waiting = screen_of(&runner.type_text("paw"));
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
        assert!(format!("{screen:?}").contains("search response was too large"));
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

    const ILLUSTRATED: &str = "<feed><title>A Journal</title>\
        <entry><title>With a plot</title><link href=\"https://example.com/2\"/>\
        <content type=\"html\">&lt;p&gt;Words.&lt;/p&gt;&lt;img src=\"/plot.png\" alt=\"A plot\"&gt;</content>\
        </entry></feed>";

    fn two_feeds() -> Vec<Subscription> {
        vec![
            Subscription {
                url: "https://example.com/feed.xml".to_owned(),
                title: "A Journal".to_owned(),
                site: "https://example.com/".to_owned(),
            },
            Subscription {
                url: "https://example.org/feed.xml".to_owned(),
                title: "Another".to_owned(),
                site: "https://example.org/".to_owned(),
            },
        ]
    }

    fn fetched(commands: &[Command]) -> Vec<(TaskId, String)> {
        commands
            .iter()
            .filter_map(|command| match command {
                Command::Spawn {
                    task,
                    work: kobo_sdk::Task::Fetch { url, .. },
                } => Some((*task, url.clone())),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn refresh_all_reads_every_feed_in_turn_and_says_how_far_it_has_got() {
        let mut runner = AppRunner::new(Feeds {
            loaded: true,
            subscriptions: two_feeds(),
            ..Feeds::default()
        });
        let context = runner.context();
        let shelf = format!("{:?}", runner.app().shelf(&context));
        assert!(shelf.contains("Refresh all"), "{shelf}");
        let first = fetched(&runner.action(action_id("refresh-all")));
        assert_eq!(first.len(), 1, "every feed was asked for at once");
        assert_eq!(first[0].1, "https://example.com/feed.xml");
        let second = fetched(&runner.task_outcome(
            first[0].0,
            TaskOutcome::Completed(ILLUSTRATED.as_bytes().to_vec()),
        ));
        assert_eq!(second[0].1, "https://example.org/feed.xml");
        assert!(runner
            .app()
            .offline_line()
            .starts_with("Refreshing 2 of 2 feeds."));
        runner.task_outcome(
            second[0].0,
            TaskOutcome::Completed(b"<html>not a feed</html>".to_vec()),
        );
        let line = runner.app().offline_line();
        assert!(
            line.contains("1 of 2 feeds could not be refreshed."),
            "{line}"
        );
    }

    #[test]
    fn a_refreshed_article_with_pictures_is_kept_whole_for_offline_reading() {
        let mut runner = AppRunner::new(Feeds {
            loaded: true,
            subscriptions: two_feeds(),
            ..Feeds::default()
        });
        let first = fetched(&runner.action(action_id("refresh-all")));
        runner.task_outcome(
            first[0].0,
            TaskOutcome::Completed(ILLUSTRATED.as_bytes().to_vec()),
        );
        let item = &super::feed::parse(ILLUSTRATED.as_bytes()).unwrap().items[0];
        let id = super::article_id(item, "https://example.com/feed.xml");
        assert!(runner.app().keeper.is_keeping(&id));
    }

    #[test]
    fn the_shelf_says_what_is_ready_offline() {
        let mut app = Feeds {
            loaded: true,
            subscriptions: two_feeds(),
            ..Feeds::default()
        };
        app.offline
            .insert("https://example.com/feed.xml".into(), (12, 4 * 1024 * 1024));
        let context = AppRunner::new(Feeds::default()).context();
        let screen = format!("{:?}", app.shelf(&context));
        assert!(
            screen.contains("Ready offline: 12 articles, 4.0 MB"),
            "{screen}"
        );
    }
}

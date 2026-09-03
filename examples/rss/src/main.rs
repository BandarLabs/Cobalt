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
//! ## Why subscriptions and a bounded article cache are stored
//!
//! A subscription is the thing the reader chose, and a small cache is what
//! keeps its latest articles readable on a train or after Wi-Fi drops. Each
//! feed cache is capped below the store's 256 KiB value ceiling; a later sync
//! replaces it with the publisher's latest copy instead of growing forever.

mod feed;
mod miniflux;
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
const CONFIG: &str = "config";
const ITEM_STATES: &str = "item-states";
const CACHE_BYTES: usize = 240 * 1024;
const MAX_ITEM_STATES: usize = 2_000;
const MAX_FULL_ARTICLES: usize = 8;
const FULL_CONTENT_BYTES: usize = 224 * 1024;

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
    /// Shared backend settings.
    Settings,
    /// Entering a web site for Miniflux discovery.
    FluxDiscover,
    /// Choosing a discovered Miniflux feed.
    FluxFound,
    /// A Miniflux entry list.
    FluxShelf,
    /// A Miniflux entry.
    FluxArticle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TaskKind {
    Search,
    Feed,
    FluxEntries,
    FluxDiscover,
    FluxSubscribe,
    FluxFull,
    FluxMutation,
}

/// The immutable destination a running request will update.
///
/// UI indices are intentionally absent: an item can move when a fresh list
/// arrives, but a response may only update the stable target it was created
/// for.
#[derive(Clone, Debug, Eq, PartialEq)]
enum TaskTarget {
    Search,
    Feed {
        subscription_url: String,
    },
    FluxEntries {
        server: String,
        mode: miniflux::ListMode,
    },
    FluxDiscover {
        server: String,
        website: String,
    },
    FluxSubscribe {
        server: String,
        feed_url: String,
    },
    FluxFull {
        server: String,
        mode: miniflux::ListMode,
        entry_id: u64,
    },
    FluxMutation {
        server: String,
        mutation: miniflux::Mutation,
    },
}

impl TaskTarget {
    const fn kind(&self) -> TaskKind {
        match self {
            Self::Search => TaskKind::Search,
            Self::Feed { .. } => TaskKind::Feed,
            Self::FluxEntries { .. } => TaskKind::FluxEntries,
            Self::FluxDiscover { .. } => TaskKind::FluxDiscover,
            Self::FluxSubscribe { .. } => TaskKind::FluxSubscribe,
            Self::FluxFull { .. } => TaskKind::FluxFull,
            Self::FluxMutation { .. } => TaskKind::FluxMutation,
        }
    }

    fn miniflux_server(&self) -> Option<&str> {
        match self {
            Self::FluxEntries { server, .. }
            | Self::FluxDiscover { server, .. }
            | Self::FluxSubscribe { server, .. }
            | Self::FluxFull { server, .. }
            | Self::FluxMutation { server, .. } => Some(server),
            Self::Search | Self::Feed { .. } => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PendingTask {
    id: TaskId,
    target: TaskTarget,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Backend {
    #[default]
    Standalone,
    Miniflux,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Setting {
    Server,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ItemState {
    key: String,
    read: bool,
    starred: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FullContent {
    id: u64,
    content: String,
}

#[derive(Default)]
struct Feeds {
    view: View,
    backend: Backend,
    server: String,
    editing: Option<Setting>,
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
    task: Option<PendingTask>,
    problem: Option<String>,
    /// The last task failure as the SDK read it. An empty article list wants
    /// the whole-screen version of it; a list with articles wants the banner.
    trouble: Option<Failure>,
    /// Which feed's overflow menu is open, if any. An index into
    /// `subscriptions` rather than a page position, so turning a page or
    /// removing an earlier feed cannot leave it pointing at the wrong one.
    menu_open: Option<usize>,
    /// State is independent of a feed's current response, keyed by its GUID,
    /// id, or safe canonical link.
    item_states: Vec<ItemState>,
    /// A cache response may arrive after a successful network response; the
    /// latter is authoritative even when it legitimately contains no items.
    live_cache: [bool; 2],
    /// Miniflux entry caches are isolated by selected list mode.
    flux_mode: miniflux::ListMode,
    flux_caches: [Vec<miniflux::Article>; 3],
    flux_live_cache: [bool; 3],
    /// The open article is stable across a list refresh, unlike a row index.
    flux_open: Option<u64>,
    flux_discovered: Vec<miniflux::Discovered>,
    flux_pending: Vec<miniflux::Mutation>,
    /// Kept apart from the coalescible queue and durably requeued on restart.
    flux_in_flight: Option<miniflux::Mutation>,
    full_content: Vec<FullContent>,
    flux_menu_open: bool,
    flux_pages: Vec<Vec<String>>,
    flux_page: usize,
}

impl Feeds {
    fn awaiting(&self, kind: TaskKind) -> bool {
        self.task
            .as_ref()
            .is_some_and(|task| task.target.kind() == kind)
    }

    /// Writes the subscription list back. Called after every change.
    fn save(&mut self, context: &mut Context) {
        let bytes = encode(&self.subscriptions);
        context.store().save(FEEDS, bytes);
    }

    fn save_config(&self, context: &mut Context) {
        let backend = match self.backend {
            Backend::Standalone => "standalone",
            Backend::Miniflux => "miniflux",
        };
        context
            .store()
            .save(CONFIG, format!("{backend}\n{}", self.server));
    }

    fn miniflux_configured(&self) -> bool {
        miniflux::configured_server(&self.server)
    }

    /// Cancels work for one Miniflux server and activates another server's
    /// durable namespace without touching the old namespace on disk.
    fn change_flux_server(&mut self, context: &mut Context, server: &str) {
        let server = miniflux::canonical_server(server)
            .unwrap_or_else(|| server.trim().trim_end_matches('/').to_owned());
        if miniflux::canonical_server(&self.server).as_deref() == Some(server.as_str()) {
            return;
        }
        if let Some(task) = self.task.take() {
            context.cancel(task.id);
        }
        if let Some(in_flight) = self.flux_in_flight.take() {
            self.flux_pending.insert(0, in_flight);
        }
        // Save before changing `self.server`: queued mutations stay with the
        // host they were created for and can never be sent to the new one.
        self.save_flux_actions(context);
        self.flux_pending.clear();
        self.flux_caches = Default::default();
        self.flux_live_cache = [false; 3];
        self.full_content.clear();
        self.flux_open = None;
        self.flux_discovered.clear();
        self.flux_menu_open = false;
        self.flux_pages.clear();
        self.flux_page = 0;
        self.list_page = 0;
        self.server = server;
        self.load_flux_namespace(context);
    }

    fn load_flux_namespace(&self, context: &mut Context) {
        let Some(actions) = miniflux::actions_key(&self.server) else {
            return;
        };
        context.store().load(actions);
        if let Some(index) = miniflux::full_index_key(&self.server) {
            context.store().load(index);
        }
        for mode in [
            miniflux::ListMode::Unread,
            miniflux::ListMode::Starred,
            miniflux::ListMode::History,
        ] {
            if let Some(key) = miniflux::cache_key(&self.server, mode) {
                context.store().load(key);
            }
        }
    }

    fn flux_entries(&self, mode: miniflux::ListMode) -> &[miniflux::Article] {
        &self.flux_caches[mode.cache_index()]
    }

    fn flux_entries_mut(&mut self, mode: miniflux::ListMode) -> &mut Vec<miniflux::Article> {
        &mut self.flux_caches[mode.cache_index()]
    }

    fn selected_flux_entries(&self) -> &[miniflux::Article] {
        self.flux_entries(self.flux_mode)
    }

    fn selected_flux_entries_mut(&mut self) -> &mut Vec<miniflux::Article> {
        self.flux_entries_mut(self.flux_mode)
    }

    fn save_flux_cache(&self, context: &mut Context, mode: miniflux::ListMode) {
        if let Some(key) = miniflux::cache_key(&self.server, mode) {
            context
                .store()
                .save(key, encode_flux_cache(self.flux_entries(mode)));
        }
    }

    fn save_flux_actions(&self, context: &mut Context) {
        if let Some(key) = miniflux::actions_key(&self.server) {
            context.store().save(
                key,
                encode_flux_actions(self.flux_in_flight.iter().chain(self.flux_pending.iter())),
            );
        }
    }

    fn load_full_content(&self, context: &mut Context, mode: miniflux::ListMode) {
        for article in self.flux_entries(mode) {
            if let Some(key) = miniflux::full_content_key(&self.server, article.id) {
                context.store().load(key);
            }
        }
    }

    fn remember_full_content(&mut self, context: &mut Context, id: u64, full_text: String) {
        self.full_content.retain(|saved| saved.id != id);
        self.full_content.push(FullContent {
            id,
            content: full_text,
        });
        while self.full_content.len() > MAX_FULL_ARTICLES {
            let removed = self.full_content.remove(0);
            if let Some(key) = miniflux::full_content_key(&self.server, removed.id) {
                context.store().forget(key);
            }
        }
        let saved = self
            .full_content
            .iter()
            .find(|saved| saved.id == id)
            .expect("just inserted");
        if let Some(key) = miniflux::full_content_key(&self.server, id) {
            context.store().save(key, saved.content.as_bytes().to_vec());
        }
        if let Some(index) = miniflux::full_index_key(&self.server) {
            context
                .store()
                .save(index, encode_full_content_index(&self.full_content));
        }
    }

    fn restore_flux_mutation(&mut self, context: &mut Context, target: &TaskTarget) {
        let TaskTarget::FluxMutation { server, mutation } = target else {
            return;
        };
        if server == &self.server && self.flux_in_flight.as_ref() == Some(mutation) {
            self.flux_in_flight = None;
            self.flux_pending.insert(0, mutation.clone());
            self.save_flux_actions(context);
        }
    }

    fn cache_key(&self) -> Option<String> {
        self.open
            .and_then(|index| self.subscriptions.get(index))
            .map(|subscription| feed_cache_key(&subscription.url))
    }

    fn load_open_cache(&self, context: &mut Context) {
        if let Some(key) = self.cache_key() {
            context.store().load(key);
        }
    }

    fn standalone_state_key(&self, item: &feed::Item) -> Option<String> {
        let stable = item.id.trim();
        (!stable.is_empty()).then(|| {
            let feed = self
                .open
                .and_then(|index| self.subscriptions.get(index))
                .map_or(0, |subscription| stable_hash(&subscription.url));
            format!("{feed:016x}:{stable}")
        })
    }

    fn item_state(&self, item: &feed::Item) -> ItemState {
        self.standalone_state_key(item)
            .and_then(|key| {
                self.item_states
                    .iter()
                    .find(|state| state.key == key)
                    .cloned()
            })
            .unwrap_or_else(|| ItemState {
                key: self.standalone_state_key(item).unwrap_or_default(),
                read: false,
                starred: false,
            })
    }

    fn set_item_state(
        &mut self,
        context: &mut Context,
        item: &feed::Item,
        read: Option<bool>,
        starred: Option<bool>,
    ) {
        let Some(key) = self.standalone_state_key(item) else {
            self.problem = Some("This entry has no stable ID or safe link.".to_owned());
            return;
        };
        if let Some(state) = self.item_states.iter_mut().find(|state| state.key == key) {
            if let Some(read) = read {
                state.read = read;
            }
            if let Some(starred) = starred {
                state.starred = starred;
            }
        } else if self.item_states.len() < MAX_ITEM_STATES {
            let candidate = ItemState {
                key,
                read: read.unwrap_or(false),
                starred: starred.unwrap_or(false),
            };
            let mut next = self.item_states.clone();
            next.push(candidate.clone());
            if encode_item_states(&next).len() > CACHE_BYTES {
                self.problem = Some("Saved article state is full.".to_owned());
                return;
            }
            self.item_states.push(candidate);
        } else {
            self.problem = Some("Saved article state is full.".to_owned());
            return;
        }
        context
            .store()
            .save(ITEM_STATES, encode_item_states(&self.item_states));
    }

    /// Asks Feedsearch what feeds an address has.
    fn ask_search(&mut self, context: &mut Context, url: &str) {
        if self.task.is_some() {
            self.problem = Some("A request is already in progress.".to_owned());
            return;
        }
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
            Some(task) => {
                self.task = Some(PendingTask {
                    id: task,
                    target: TaskTarget::Search,
                });
            }
            None => self.problem = Some("The device is busy. Try that again.".to_owned()),
        }
    }

    /// Fetches the open feed.
    fn ask_feed(&mut self, context: &mut Context) {
        if self.task.is_some() {
            self.problem = Some("A request is already in progress.".to_owned());
            return;
        }
        let Some(subscription) = self.open.and_then(|index| self.subscriptions.get(index)) else {
            return;
        };
        let url = subscription.url.clone();
        self.items.clear();
        self.live_cache[0] = false;
        self.load_open_cache(context);
        self.problem = None;
        self.trouble = None;
        match context.spawn_retrying(Task::Fetch {
            url,
            offset: 0,
            max_bytes: FEED_BYTES,
            credential: None,
            headers: Vec::new(),
        }) {
            Some(task) => {
                self.task = Some(PendingTask {
                    id: task,
                    target: TaskTarget::Feed {
                        subscription_url: subscription.url.clone(),
                    },
                });
            }
            None => self.problem = Some("The device is busy. Try that again.".to_owned()),
        }
    }

    /// Miniflux operations are intentionally one-shot: the service may apply
    /// a PUT before a connection drops, so retrying a mutation would be less
    /// honest than retaining it in the durable queue for the next sync.
    fn start_flux(&mut self, context: &mut Context, target: TaskTarget, work: Task) -> bool {
        if self.task.is_some() {
            self.problem = Some("A Miniflux request is already in progress.".to_owned());
            return false;
        }
        if let Some(task) = context.spawn(work) {
            self.task = Some(PendingTask { id: task, target });
            true
        } else {
            self.problem = Some("The device is busy. Try that again.".to_owned());
            false
        }
    }

    fn request_flux_entries(&mut self, context: &mut Context) {
        let mode = self.flux_mode;
        let server = self.server.clone();
        self.start_flux(
            context,
            TaskTarget::FluxEntries {
                server: server.clone(),
                mode,
            },
            miniflux::entries(&server, mode),
        );
    }

    fn send_next_flux_mutation(&mut self, context: &mut Context) {
        if self.task.is_some() || self.flux_in_flight.is_some() {
            return;
        }
        let Some(mutation) = self.flux_pending.first().cloned() else {
            self.request_flux_entries(context);
            return;
        };
        self.flux_pending.remove(0);
        self.flux_in_flight = Some(mutation.clone());
        let server = self.server.clone();
        if !self.start_flux(
            context,
            TaskTarget::FluxMutation {
                server: server.clone(),
                mutation: mutation.clone(),
            },
            miniflux::mutate(&server, &mutation),
        ) {
            self.flux_in_flight = None;
            self.flux_pending.insert(0, mutation);
        }
        self.save_flux_actions(context);
    }

    fn sync_flux(&mut self, context: &mut Context) {
        if !self.miniflux_configured() {
            self.problem = Some("Set a Miniflux HTTPS server in Settings.".to_owned());
            self.view = View::Settings;
            return;
        }
        if self.task.is_some() {
            self.problem = Some("A Miniflux request is already in progress.".to_owned());
            return;
        }
        self.problem = None;
        self.trouble = None;
        self.flux_live_cache[self.flux_mode.cache_index()] = false;
        if self.flux_pending.is_empty() {
            self.request_flux_entries(context);
        } else {
            self.send_next_flux_mutation(context);
        }
    }

    fn queue_flux_mutation(&mut self, context: &mut Context, mutation: &miniflux::Mutation) {
        match mutation {
            miniflux::Mutation::Read(id) => {
                let mutation = miniflux::Mutation::Read(*id);
                if self.flux_in_flight.as_ref() != Some(&mutation)
                    && !self.flux_pending.contains(&mutation)
                {
                    self.flux_pending.push(mutation);
                }
            }
            miniflux::Mutation::Star { id, starred } => {
                self.flux_pending.retain(|pending| {
                    !matches!(pending, miniflux::Mutation::Star { id: pending_id, .. } if pending_id == id)
                });
                self.flux_pending.push(miniflux::Mutation::Star {
                    id: *id,
                    starred: *starred,
                });
            }
        }
        self.save_flux_actions(context);
    }

    fn lay_out_flux(&mut self, context: &Context) {
        let Some(article) = self.flux_open.and_then(|id| {
            self.selected_flux_entries()
                .iter()
                .find(|article| article.id == id)
        }) else {
            self.flux_pages.clear();
            return;
        };
        let status = if article.starred {
            format!("{} · Starred", article.status)
        } else {
            article.status.clone()
        };
        self.flux_pages = context.paginate_reading(
            &format!("{status} · {}\n\n{}", article.feed, article.content),
            false,
        );
        self.flux_page = 0;
    }

    fn select_flux_mode(&mut self, context: &mut Context, mode: miniflux::ListMode) {
        if self.task.is_some() {
            self.problem = Some("A Miniflux request is already in progress.".to_owned());
            return;
        }
        self.flux_mode = mode;
        self.flux_open = None;
        self.flux_pages.clear();
        self.list_page = 0;
        self.sync_flux(context);
    }

    #[allow(clippy::too_many_lines)]
    fn on_flux_action(&mut self, context: &mut Context, action: ActionId) {
        if self.view == View::FluxDiscover {
            match self.keyboard.press(action) {
                Some(Pressed::Submitted) => {
                    let website = self.keyboard.take().trim().to_owned();
                    if website.is_empty() {
                        return;
                    }
                    if self.miniflux_configured() {
                        self.problem = None;
                        if self.start_flux(
                            context,
                            TaskTarget::FluxDiscover {
                                server: self.server.clone(),
                                website: website.clone(),
                            },
                            miniflux::discover(&self.server, &website),
                        ) {
                            self.flux_discovered.clear();
                            self.view = View::FluxFound;
                        }
                    } else {
                        self.problem = Some("Set a Miniflux HTTPS server in Settings.".to_owned());
                        self.view = View::Settings;
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

        if action == ActionId::BACK && self.flux_menu_open {
            self.flux_menu_open = false;
            self.show(context);
            return;
        }
        if action == ActionId::BACK {
            self.problem = None;
            self.trouble = None;
            self.view = match self.view {
                View::FluxFound => View::FluxDiscover,
                _ => View::FluxShelf,
            };
            self.show(context);
            return;
        }

        if action == action_id("flux-more") && self.task.is_none() {
            self.flux_menu_open = true;
        } else if action == action_id("flux-sync") {
            self.sync_flux(context);
        } else if action == action_id("flux-discover") {
            if self.miniflux_configured() {
                self.keyboard.clear();
                self.problem = None;
                self.view = View::FluxDiscover;
            } else {
                self.problem = Some("Set a Miniflux HTTPS server in Settings.".to_owned());
                self.view = View::Settings;
            }
        } else if action == action_id("flux-unread") {
            self.select_flux_mode(context, miniflux::ListMode::Unread);
        } else if action == action_id("flux-starred") {
            self.select_flux_mode(context, miniflux::ListMode::Starred);
        } else if action == action_id("flux-history") {
            self.select_flux_mode(context, miniflux::ListMode::History);
        } else if action == action_id("list-back") {
            self.list_page = self.list_page.saturating_sub(1);
        } else if action == action_id("list-next") {
            self.list_page += 1;
        } else if action == action_id("flux-page-back") {
            self.flux_page = self.flux_page.saturating_sub(1);
        } else if action == action_id("flux-page-next") {
            if self.flux_page + 1 < self.flux_pages.len() {
                self.flux_page += 1;
            }
        } else if action == action_id("flux-full") {
            self.flux_menu_open = false;
            if let Some(entry_id) = self.flux_open {
                let mode = self.flux_mode;
                self.start_flux(
                    context,
                    TaskTarget::FluxFull {
                        server: self.server.clone(),
                        mode,
                        entry_id,
                    },
                    miniflux::full_content(&self.server, entry_id),
                );
            }
        } else if action == action_id("flux-toggle-read") {
            if let Some(id) = self.flux_open {
                let mode = self.flux_mode;
                let marked = self
                    .selected_flux_entries_mut()
                    .iter_mut()
                    .find(|article| article.id == id)
                    .and_then(|article| {
                        if article.status == "read" {
                            None
                        } else {
                            "read".clone_into(&mut article.status);
                            Some(article.id)
                        }
                    });
                if let Some(id) = marked {
                    self.queue_flux_mutation(context, &miniflux::Mutation::Read(id));
                    self.save_flux_cache(context, mode);
                    self.lay_out_flux(context);
                } else {
                    self.problem = Some("This entry is already read.".to_owned());
                }
            }
        } else if action == action_id("flux-toggle-star") {
            if let Some(id) = self.flux_open {
                let mode = self.flux_mode;
                let toggled = self
                    .selected_flux_entries_mut()
                    .iter_mut()
                    .find(|article| article.id == id)
                    .map(|article| {
                        article.starred = !article.starred;
                        (article.id, article.starred)
                    });
                if let Some((id, starred)) = toggled {
                    self.queue_flux_mutation(context, &miniflux::Mutation::Star { id, starred });
                    self.save_flux_cache(context, mode);
                    self.lay_out_flux(context);
                }
            }
        } else if let Some(index) = indexed(action, "flux-found", self.flux_discovered.len()) {
            if let Some(found) = self.flux_discovered.get(index).cloned() {
                self.problem = Some("Adding the feed…".to_owned());
                self.start_flux(
                    context,
                    TaskTarget::FluxSubscribe {
                        server: self.server.clone(),
                        feed_url: found.url.clone(),
                    },
                    miniflux::subscribe(&self.server, &found.url),
                );
            }
        } else if let Some(index) =
            indexed(action, "flux-entry", self.selected_flux_entries().len())
        {
            if let Some(article) = self.selected_flux_entries().get(index) {
                self.flux_open = Some(article.id);
                self.flux_menu_open = false;
                self.view = View::FluxArticle;
                self.lay_out_flux(context);
            }
        }
        self.show(context);
    }

    fn on_flux_task(&mut self, context: &mut Context, target: TaskTarget, outcome: TaskOutcome) {
        match outcome {
            TaskOutcome::Completed(bytes) => match target {
                TaskTarget::FluxEntries { mode, .. } => {
                    if let Some(mut entries) = miniflux::parse_entries(&bytes) {
                        overlay_full_content(&self.full_content, &mut entries);
                        self.flux_caches[mode.cache_index()] = entries;
                        self.flux_live_cache[mode.cache_index()] = true;
                        self.save_flux_cache(context, mode);
                        self.load_full_content(context, mode);
                        self.problem = Some(format!(
                            "Synced {} {} entries.",
                            self.flux_entries(mode).len(),
                            mode.label().to_lowercase()
                        ));
                    } else {
                        self.problem = Some(
                            "Miniflux entries were invalid; cached entries remain readable."
                                .to_owned(),
                        );
                    }
                }
                TaskTarget::FluxDiscover { .. } => {
                    self.flux_discovered = miniflux::parse_discoveries(&bytes);
                    self.problem = self
                        .flux_discovered
                        .is_empty()
                        .then(|| "Miniflux did not find a feed there.".to_owned());
                }
                TaskTarget::FluxSubscribe { .. } => {
                    self.problem = Some("Feed added. Syncing entries…".to_owned());
                    self.view = View::FluxShelf;
                    self.request_flux_entries(context);
                }
                TaskTarget::FluxFull { mode, entry_id, .. } => {
                    let full_text = miniflux::parse_full_content(&bytes, FULL_CONTENT_BYTES);
                    if let Some(full_text) = full_text {
                        if full_text.len() > FULL_CONTENT_BYTES {
                            self.problem = Some(
                                "Full article is too large to save; existing offline copy remains."
                                    .to_owned(),
                            );
                        } else {
                            self.remember_full_content(context, entry_id, full_text.clone());
                            if let Some(article) = self
                                .flux_entries_mut(mode)
                                .iter_mut()
                                .find(|article| article.id == entry_id)
                            {
                                article.content = full_text;
                            }
                            self.save_flux_cache(context, mode);
                            if self.flux_mode == mode && self.flux_open == Some(entry_id) {
                                self.lay_out_flux(context);
                            }
                            self.problem =
                                Some("Full article saved for offline reading.".to_owned());
                        }
                    } else {
                        self.problem = Some("Miniflux did not return article text.".to_owned());
                    }
                }
                TaskTarget::FluxMutation { mutation, .. } => {
                    if self.flux_in_flight.as_ref() == Some(&mutation) {
                        self.flux_in_flight = None;
                    }
                    self.save_flux_actions(context);
                    if self.flux_pending.is_empty() {
                        self.request_flux_entries(context);
                    } else {
                        self.send_next_flux_mutation(context);
                    }
                }
                TaskTarget::Search | TaskTarget::Feed { .. } => {
                    unreachable!("standalone task")
                }
            },
            TaskOutcome::Failed(error) => {
                self.restore_flux_mutation(context, &target);
                self.problem = Some(miniflux_failure(error));
            }
            TaskOutcome::Cancelled => {
                self.restore_flux_mutation(context, &target);
                self.problem =
                    Some("Miniflux request cancelled. Cached entries remain readable.".to_owned());
            }
        }
    }

    /// Follows a feed, unless it is already followed.
    ///
    /// Returns where it sits in the list either way, so that choosing
    /// something already subscribed opens it rather than refusing.
    fn subscribe(&mut self, found: &search::Found) -> Option<usize> {
        let Some(url) = kobo_net::resolve_https_url("", &found.url) else {
            self.problem = Some("Feedsearch did not return an HTTPS feed.".to_owned());
            return None;
        };
        if let Some(index) = self.subscriptions.iter().position(|feed| feed.url == url) {
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
            url,
            title: clamp_bytes(&found.title, 512),
            site: clamp_bytes(&found.site, 2_048),
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
        let state = self.item_state(item);
        let status = match (state.read, state.starred) {
            (false, false) => "Unread",
            (false, true) => "Unread · Starred",
            (true, false) => "Read",
            (true, true) => "Read · Starred",
        };
        self.pages =
            context.paginate_reading(&format!("{status}\n\n{}", article_text(item)), false);
        self.page = 0;
    }

    fn settings(&self) -> Screen {
        let mode = match self.backend {
            Backend::Standalone => "Standalone",
            Backend::Miniflux => "Miniflux",
        };
        let mut screen = ScreenBuilder::new("rss-settings")
            .top_bar("Feeds settings")
            .field("mode", mode, "Standalone or Miniflux")
            .secondary("Standalone uses Feedsearch. Miniflux uses its dedicated API token.");
        if self.backend == Backend::Miniflux {
            screen = screen
                .field("server", &self.server, "https://miniflux.example")
                .field("flux-discover", "Add a Miniflux feed", "")
                .secondary("Install secret miniflux; kobod sends it as X-Auth-Token.");
        }
        screen.build()
    }

    fn editing(&self) -> Screen {
        let prompt = match self.editing {
            Some(Setting::Server) => "Miniflux HTTPS server",
            None => unreachable!("only called while editing"),
        };
        ScreenBuilder::new("rss-settings")
            .top_bar("Feeds settings")
            .typed(&self.keyboard, prompt)
            .keyboard(&self.keyboard, "Save")
            .build()
    }

    fn flux_shelf(&self, context: &Context) -> Screen {
        if !self.miniflux_configured() {
            return ScreenBuilder::new("rss-flux")
                .top_bar("Feeds")
                .heading("Miniflux setup needed")
                .text("Set an HTTPS server in Settings.")
                .secondary("Install the token with kobo secret set miniflux.")
                .buttons([("settings", "Settings"), ("flux-discover", "Add a feed")])
                .build();
        }
        let selected = match self.flux_mode {
            miniflux::ListMode::Unread => 0,
            miniflux::ListMode::Starred => 1,
            miniflux::ListMode::History => 2,
        };
        let mut screen = ScreenBuilder::new("rss-flux")
            .top_bar("Feeds")
            .top_bar_action("settings", "Settings")
            .tabs(
                selected,
                [
                    ("flux-unread", "Unread"),
                    ("flux-starred", "Starred"),
                    ("flux-history", "History"),
                ],
            );
        if self.task.is_none() {
            screen = screen.top_bar_glyph("flux-sync", "Sync", Glyph::Refresh);
        }
        if let Some(problem) = &self.problem {
            screen = screen.banner(BannerLevel::Attention, problem.clone());
        }
        if self.awaiting(TaskKind::FluxEntries) {
            return screen
                .activity(format!("Syncing {} entries", self.flux_mode.label()), None)
                .skeleton(6)
                .build();
        }
        if self.selected_flux_entries().is_empty()
            && !self.flux_live_cache[self.flux_mode.cache_index()]
        {
            return screen
                .empty_state(format!(
                    "No {} entries cached. Sync when Miniflux is reachable.",
                    self.flux_mode.label().to_lowercase()
                ))
                .buttons([("flux-sync", "Sync"), ("flux-discover", "Add a feed")])
                .build();
        }
        let rows: Vec<(String, String)> = self
            .selected_flux_entries()
            .iter()
            .map(|article| {
                let status = if article.starred {
                    format!("{} · Starred", article.status)
                } else {
                    article.status.clone()
                };
                (
                    context.one_line_row(&article.title, true),
                    context.one_line_row(&format!("{} · {status}", article.feed), true),
                )
            })
            .collect();
        // Tabs and the cache status occupy panel space outside the generic
        // row paginator. Three compact rows leave room for both as well as
        // page position on Clara-sized screens.
        let pages = flux_page_groups(rows.len());
        let page = self.list_page.min(pages.len().saturating_sub(1));
        let shown = pages.get(page).cloned().unwrap_or_default();
        screen = screen.rows(shown.iter().map(|index| {
            (
                format!("flux-entry-{index}"),
                rows[*index].0.clone(),
                rows[*index].1.clone(),
                if self.selected_flux_entries()[*index].starred {
                    Glyph::Bookmark
                } else {
                    Glyph::Rss
                },
            )
        }));
        let pending = self.flux_pending.len();
        screen = screen.secondary(if pending == 0 {
            "Cached for offline reading.".to_owned()
        } else {
            format!(
                "{pending} change{} pending sync.",
                if pending == 1 { "" } else { "s" }
            )
        });
        if pages.len() > 1 {
            screen = screen
                .page_turns("list-back", "list-next")
                .page_position(page_number(page), page_total(pages.len()));
        }
        screen.build()
    }

    fn flux_discover(&self) -> Screen {
        let mut screen = ScreenBuilder::new("rss-flux-discover").top_bar("Add a feed");
        if let Some(problem) = &self.problem {
            screen = screen.banner(BannerLevel::Attention, problem.clone());
        }
        screen
            .typed(&self.keyboard, "Website address")
            .secondary("Miniflux finds the feeds for this site.")
            .keyboard(&self.keyboard, "Discover")
            .build()
    }

    fn flux_found(&self, context: &Context) -> Screen {
        let mut screen = ScreenBuilder::new("rss-flux-found").top_bar("Choose a feed");
        if let Some(problem) = &self.problem {
            screen = screen.banner(BannerLevel::Attention, problem.clone());
        }
        if self.awaiting(TaskKind::FluxDiscover) {
            return screen.activity("Finding feeds", None).skeleton(4).build();
        }
        if self.awaiting(TaskKind::FluxSubscribe) {
            return screen.activity("Adding feed", None).build();
        }
        if self.flux_discovered.is_empty() {
            return screen
                .empty_state("Miniflux did not find a feed there.")
                .button("flux-discover", "Try another website")
                .build();
        }
        screen
            .rows(
                self.flux_discovered
                    .iter()
                    .enumerate()
                    .map(|(index, found)| {
                        (
                            format!("flux-found-{index}"),
                            context.one_line_row(&found.title, true),
                            context.one_line_row(&found.kind, true),
                            Glyph::Rss,
                        )
                    }),
            )
            .build()
    }

    fn flux_article(&self) -> Screen {
        let Some(article) = self.flux_open.and_then(|id| {
            self.selected_flux_entries()
                .iter()
                .find(|article| article.id == id)
        }) else {
            return ScreenBuilder::new("rss-flux-reading")
                .top_bar("Feeds")
                .empty_state("Choose an entry from Miniflux.")
                .build();
        };
        let mut screen = ScreenBuilder::new("rss-flux-reading")
            .top_bar(&article.title)
            .top_bar_glyph("flux-toggle-read", "Mark read", Glyph::Check)
            .top_bar_glyph(
                "flux-toggle-star",
                if article.starred { "Unstar" } else { "Star" },
                Glyph::Bookmark,
            )
            .reading(true);
        if self.task.is_none() {
            screen = screen.top_bar_overflow(
                "flux-more",
                self.flux_menu_open,
                [("flux-full", "Load full article")],
            );
        }
        if self.flux_pages.is_empty() {
            screen = screen.text(format!(
                "{} · {}",
                article.feed,
                if article.starred {
                    "Starred"
                } else {
                    &article.status
                }
            ));
            screen = screen.text(if self.awaiting(TaskKind::FluxFull) {
                "Loading full article…"
            } else {
                "No cached article text. Load the full article when online."
            });
        } else {
            let page = self.flux_page.min(self.flux_pages.len() - 1);
            for paragraph in &self.flux_pages[page] {
                screen = screen.text(paragraph.clone());
            }
            screen = screen
                .page_turns("flux-page-back", "flux-page-next")
                .page_position(page_number(page), page_total(self.flux_pages.len()));
        }
        screen.build()
    }

    fn show(&mut self, context: &mut Context) {
        if self.editing.is_some() {
            context.set_screen(self.editing().with_own_back(true));
            return;
        }
        let screen = if self.view == View::Settings {
            self.settings()
        } else if self.backend == Backend::Miniflux {
            match self.view {
                View::FluxDiscover => self.flux_discover(),
                View::FluxFound => self.flux_found(context),
                View::FluxArticle => self.flux_article(),
                _ => self.flux_shelf(context),
            }
        } else {
            match self.view {
                View::Shelf => self.shelf(context),
                View::Search => self.search(),
                View::Found => self.results(context),
                View::Items => self.articles(context),
                View::Reading => self.reading(),
                View::Settings
                | View::FluxDiscover
                | View::FluxFound
                | View::FluxShelf
                | View::FluxArticle => {
                    unreachable!("the route was handled above")
                }
            }
        };
        // Every view except the shelf was reached from another one, so Back
        // unwinds this application first and leaves it only from the shelf.
        // Without this, Back out of an article lands at the launcher.
        context.set_screen(screen.with_own_back(
            !matches!(self.view, View::Shelf | View::FluxShelf)
                || self.menu_open.is_some()
                || self.flux_menu_open,
        ));
    }

    fn shelf(&self, context: &Context) -> Screen {
        let mut screen = ScreenBuilder::new("rss-shelf")
            .top_bar("Feeds")
            .top_bar_action("settings", "Settings");
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
        if self.awaiting(TaskKind::Search) {
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
        if self.awaiting(TaskKind::Feed) {
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
                let state = self.item_state(item);
                let status = match (state.read, state.starred) {
                    (false, false) => "Unread".to_owned(),
                    (false, true) => "Unread · Starred".to_owned(),
                    (true, false) => "Read".to_owned(),
                    (true, true) => "Read · Starred".to_owned(),
                };
                let byline = byline(item);
                (
                    context.clamped_row(&item.title, 2, true),
                    context.one_line_row(
                        &if byline.is_empty() {
                            status
                        } else {
                            format!("{status} · {byline}")
                        },
                        true,
                    ),
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
        let item = self.article.and_then(|index| self.items.get(index));
        let title = item.map_or_else(String::new, |item| item.title.clone());
        let state = item.map_or(
            ItemState {
                key: String::new(),
                read: false,
                starred: false,
            },
            |item| self.item_state(item),
        );
        let mut screen = ScreenBuilder::new("rss-reading")
            .top_bar(title)
            .top_bar_glyph(
                "toggle-read",
                if state.read {
                    "Mark unread"
                } else {
                    "Mark read"
                },
                Glyph::Check,
            )
            .top_bar_glyph(
                "toggle-star",
                if state.starred { "Unstar" } else { "Star" },
                Glyph::Bookmark,
            )
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

fn flux_page_groups(entries: usize) -> Vec<Vec<usize>> {
    let pages: Vec<Vec<usize>> = (0..entries)
        .collect::<Vec<_>>()
        .chunks(3)
        .map(<[usize]>::to_vec)
        .collect();
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
        out.push_str(&clamp_bytes(&clean(&feed.url), 2_048));
        out.push('\t');
        out.push_str(&clamp_bytes(&clean(&feed.title), 512));
        out.push('\t');
        out.push_str(&clamp_bytes(&clean(&feed.site), 2_048));
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

fn decode_config(bytes: &[u8]) -> (Backend, String) {
    let stored = String::from_utf8_lossy(bytes);
    let mut fields = stored.lines();
    let backend = match fields.next().unwrap_or_default() {
        "miniflux" => Backend::Miniflux,
        _ => Backend::Standalone,
    };
    let server = fields.next().unwrap_or_default().trim().to_owned();
    (backend, server)
}

/// Store strings are escaped rather than split on an article's prose.
fn escape_field(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

fn unescape_field(value: &str) -> String {
    let mut decoded = String::with_capacity(value.len());
    let mut characters = value.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            decoded.push(character);
            continue;
        }
        match characters.next() {
            Some('t') => decoded.push('\t'),
            Some('n') => decoded.push('\n'),
            Some('r') => decoded.push('\r'),
            Some('\\') | None => decoded.push('\\'),
            Some(other) => {
                decoded.push('\\');
                decoded.push(other);
            }
        }
    }
    decoded
}

/// Takes a UTF-8 prefix that will fit in one stored field.
fn clamp_bytes(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        return value.to_owned();
    }
    let mut end = limit;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

fn stable_hash(value: &str) -> u64 {
    value.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

fn feed_cache_key(url: &str) -> String {
    format!("feed-cache-{:016x}", stable_hash(url))
}

fn encode_feed_cache(feed: &feed::Feed) -> Vec<u8> {
    let mut stored = format!(
        "{}\t{}\n",
        escape_field(&clamp_bytes(&feed.title, 256)),
        escape_field(&clamp_bytes(&feed.site, 512))
    );
    for item in &feed.items {
        let line = format!(
            "{}\t{}\t{}\t{}\t{}\t{}\n",
            escape_field(&clamp_bytes(&item.id, 512)),
            escape_field(&clamp_bytes(&item.title, 256)),
            escape_field(&clamp_bytes(&item.link, 512)),
            escape_field(&clamp_bytes(&item.stamp, 128)),
            escape_field(&clamp_bytes(&item.author, 256)),
            escape_field(&clamp_bytes(&item.body, 3_500)),
        );
        if stored.len() + line.len() > CACHE_BYTES {
            break;
        }
        stored.push_str(&line);
    }
    stored.into_bytes()
}

fn decode_feed_cache(bytes: &[u8]) -> Option<feed::Feed> {
    let stored = String::from_utf8_lossy(bytes);
    let mut lines = stored.lines();
    let header = lines.next()?;
    let mut header = header.split('\t').map(unescape_field);
    let mut cached = feed::Feed {
        title: header.next().unwrap_or_default(),
        site: header.next().unwrap_or_default(),
        items: Vec::new(),
    };
    for line in lines.take(feed::MAX_ITEMS) {
        let mut fields = line.split('\t').map(unescape_field);
        let item = feed::Item {
            id: fields.next().unwrap_or_default(),
            title: fields.next().unwrap_or_default(),
            link: fields.next().unwrap_or_default(),
            stamp: fields.next().unwrap_or_default(),
            author: fields.next().unwrap_or_default(),
            body: fields.next().unwrap_or_default(),
        };
        if !item.title.trim().is_empty()
            || !item.body.trim().is_empty()
            || !item.id.trim().is_empty()
        {
            cached.items.push(item);
        }
    }
    Some(cached)
}

fn encode_item_states(states: &[ItemState]) -> Vec<u8> {
    use std::fmt::Write as _;

    let mut stored = String::new();
    for state in states.iter().take(MAX_ITEM_STATES) {
        let _ = writeln!(
            stored,
            "{}\t{}\t{}",
            escape_field(&state.key),
            state.read,
            state.starred
        );
    }
    stored.into_bytes()
}

fn decode_item_states(bytes: &[u8]) -> Vec<ItemState> {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let key = unescape_field(fields.next()?);
            let read = fields.next()? == "true";
            let starred = fields.next()? == "true";
            (!key.is_empty()).then_some(ItemState { key, read, starred })
        })
        .take(MAX_ITEM_STATES)
        .collect()
}

fn encode_flux_actions<'a>(actions: impl IntoIterator<Item = &'a miniflux::Mutation>) -> Vec<u8> {
    actions
        .into_iter()
        .take(MAX_ITEM_STATES)
        .map(|action| match action {
            miniflux::Mutation::Read(id) => format!("r\t{id}\n"),
            miniflux::Mutation::Star { id, starred } => format!("s\t{id}\t{starred}\n"),
        })
        .collect::<String>()
        .into_bytes()
}

fn encode_full_content_index(contents: &[FullContent]) -> Vec<u8> {
    contents
        .iter()
        .map(|content| content.id.to_string())
        .collect::<Vec<_>>()
        .join("\n")
        .into_bytes()
}

fn decode_full_content_index(bytes: &[u8]) -> Vec<u64> {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter_map(|id| id.parse().ok())
        .take(MAX_FULL_ARTICLES)
        .collect()
}

fn overlay_full_content(contents: &[FullContent], entries: &mut [miniflux::Article]) {
    for article in entries {
        if let Some(saved) = contents.iter().find(|saved| saved.id == article.id) {
            article.content.clone_from(&saved.content);
        }
    }
}

fn decode_flux_actions(bytes: &[u8]) -> Vec<miniflux::Mutation> {
    let mut actions = Vec::new();
    for mutation in String::from_utf8_lossy(bytes).lines().filter_map(|line| {
        let mut fields = line.split('\t');
        match fields.next()? {
            "r" => fields.next()?.parse().ok().map(miniflux::Mutation::Read),
            "s" => Some(miniflux::Mutation::Star {
                id: fields.next()?.parse().ok()?,
                starred: fields.next()? == "true",
            }),
            _ => None,
        }
    }) {
        match mutation {
            miniflux::Mutation::Read(id) if actions.contains(&miniflux::Mutation::Read(id)) => {}
            miniflux::Mutation::Star { id, .. } => {
                actions.retain(
                    |queued| !matches!(queued, miniflux::Mutation::Star { id: queued_id, .. } if *queued_id == id),
                );
                actions.push(mutation);
            }
            miniflux::Mutation::Read(_) => actions.push(mutation),
        }
        if actions.len() == MAX_ITEM_STATES {
            break;
        }
    }
    actions
}

fn encode_flux_cache(entries: &[miniflux::Article]) -> Vec<u8> {
    let mut stored = String::new();
    for article in entries {
        let line = format!(
            "{}\t{}\t{}\t{}\t{}\t{}\n",
            article.id,
            escape_field(&clamp_bytes(&article.title, 256)),
            escape_field(&clamp_bytes(&article.feed, 256)),
            escape_field(&clamp_bytes(&article.content, 3_500)),
            escape_field(&clamp_bytes(&article.status, 32)),
            article.starred,
        );
        if stored.len() + line.len() > CACHE_BYTES {
            break;
        }
        stored.push_str(&line);
    }
    stored.into_bytes()
}

fn decode_flux_cache(bytes: &[u8]) -> Vec<miniflux::Article> {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t').map(unescape_field);
            Some(miniflux::Article {
                id: fields.next()?.parse().ok()?,
                title: fields.next()?,
                feed: fields.next()?,
                content: fields.next()?,
                status: fields.next()?,
                starred: fields.next()? == "true",
            })
        })
        .take(100)
        .collect()
}

fn miniflux_failure(error: kobo_sdk::TaskError) -> String {
    match error {
        kobo_sdk::TaskError::Unauthorized => {
            "Miniflux rejected secret miniflux (401 Unauthorized).".to_owned()
        }
        kobo_sdk::TaskError::NoCredential => {
            "Miniflux needs secret miniflux. Run kobo secret set miniflux.".to_owned()
        }
        kobo_sdk::TaskError::Offline => {
            "Offline. Cached entries remain readable; join Wi-Fi to sync.".to_owned()
        }
        kobo_sdk::TaskError::Unreachable => {
            "Miniflux did not answer. Cached entries remain readable.".to_owned()
        }
        other => kobo_sdk::Failure::of(other).advice.to_owned(),
    }
}

/// The index in a `prefix-N` action name, if that is what this is.
fn indexed(action: ActionId, prefix: &str, count: usize) -> Option<usize> {
    (0..count).find(|index| action_id(&format!("{prefix}-{index}")) == action)
}

impl KoboApp for Feeds {
    fn on_start(&mut self, context: &mut Context) {
        context.store().load(FEEDS);
        context.store().load(CONFIG);
        context.store().load(ITEM_STATES);
        self.show(context);
    }

    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        match result {
            StoreResult::Loaded { key, value } => {
                if key == FEEDS {
                    self.subscriptions = value.map(|bytes| decode(&bytes)).unwrap_or_default();
                    self.loaded = true;
                } else if key == CONFIG {
                    if let Some(value) = value {
                        let (backend, server) = decode_config(&value);
                        self.backend = backend;
                        self.change_flux_server(context, &server);
                        if self.backend == Backend::Miniflux {
                            self.view = View::FluxShelf;
                        }
                    }
                } else if key == ITEM_STATES {
                    self.item_states = value.as_deref().map(decode_item_states).unwrap_or_default();
                } else if miniflux::actions_key(&self.server).as_deref() == Some(key.as_str()) {
                    self.flux_pending = value
                        .as_deref()
                        .map(decode_flux_actions)
                        .unwrap_or_default();
                } else if miniflux::full_index_key(&self.server).as_deref() == Some(key.as_str()) {
                    for id in value
                        .as_deref()
                        .map(decode_full_content_index)
                        .unwrap_or_default()
                    {
                        if let Some(key) = miniflux::full_content_key(&self.server, id) {
                            context.store().load(key);
                        }
                    }
                } else if let Some(id) = miniflux::full_content_id(&self.server, &key) {
                    if let Some(content) = value
                        .and_then(|content| String::from_utf8(content).ok())
                        .filter(|content| content.len() <= FULL_CONTENT_BYTES)
                    {
                        self.full_content.retain(|saved| saved.id != id);
                        if self.full_content.len() < MAX_FULL_ARTICLES {
                            self.full_content.push(FullContent { id, content });
                        }
                        for entries in &mut self.flux_caches {
                            overlay_full_content(&self.full_content, entries);
                        }
                        if self.flux_open.is_some() {
                            self.lay_out_flux(context);
                        }
                    }
                } else if let Some(mode) = [
                    miniflux::ListMode::Unread,
                    miniflux::ListMode::Starred,
                    miniflux::ListMode::History,
                ]
                .into_iter()
                .find(|mode| {
                    miniflux::cache_key(&self.server, *mode).as_deref() == Some(key.as_str())
                }) {
                    if self.flux_entries(mode).is_empty() {
                        self.flux_caches[mode.cache_index()] =
                            value.as_deref().map(decode_flux_cache).unwrap_or_default();
                        self.load_full_content(context, mode);
                    }
                } else if key == self.cache_key().unwrap_or_default()
                    && self.items.is_empty()
                    && !self.live_cache[0]
                {
                    if let Some(cached) = value.as_deref().and_then(decode_feed_cache) {
                        self.items = cached.items;
                    }
                }
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
        if let Some(setting) = self.editing {
            match self.keyboard.press(action) {
                Some(Pressed::Submitted) => {
                    let value = self.keyboard.take().trim().to_owned();
                    if !value.is_empty() {
                        match setting {
                            Setting::Server => self.change_flux_server(context, &value),
                        }
                        self.save_config(context);
                    }
                    self.editing = None;
                }
                Some(Pressed::Edited | Pressed::Shifted) => {
                    self.show(context);
                    return;
                }
                None if action == ActionId::BACK => self.editing = None,
                None => {}
            }
            self.show(context);
            return;
        }

        if action == action_id("settings") {
            self.view = View::Settings;
            self.show(context);
            return;
        }
        if self.view == View::Settings {
            if action == action_id("mode") {
                self.backend = match self.backend {
                    Backend::Standalone => Backend::Miniflux,
                    Backend::Miniflux => Backend::Standalone,
                };
                self.save_config(context);
            } else if action == action_id("server") && self.backend == Backend::Miniflux {
                self.keyboard = Keyboard::with_text(if self.server.is_empty() {
                    "https://"
                } else {
                    &self.server
                });
                self.editing = Some(Setting::Server);
            } else if action == action_id("flux-discover") && self.backend == Backend::Miniflux {
                self.keyboard.clear();
                self.problem = None;
                self.view = View::FluxDiscover;
            } else if action == action_id("back") || action == ActionId::BACK {
                self.view = match self.backend {
                    Backend::Standalone => View::Shelf,
                    Backend::Miniflux => View::FluxShelf,
                };
            }
            self.show(context);
            return;
        }
        if self.backend == Backend::Miniflux {
            self.on_flux_action(context, action);
            return;
        }

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
                View::Settings
                | View::FluxDiscover
                | View::FluxFound
                | View::FluxShelf
                | View::FluxArticle => unreachable!("Miniflux routes are handled above"),
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

        if action == action_id("toggle-read") {
            if let Some(item) = self
                .article
                .and_then(|index| self.items.get(index))
                .cloned()
            {
                let state = self.item_state(&item);
                self.set_item_state(context, &item, Some(!state.read), None);
                self.lay_out(context);
            }
            self.show(context);
            return;
        }

        if action == action_id("toggle-star") {
            if let Some(item) = self
                .article
                .and_then(|index| self.items.get(index))
                .cloned()
            {
                let state = self.item_state(&item);
                self.set_item_state(context, &item, None, Some(!state.starred));
                self.lay_out(context);
            }
            self.show(context);
            return;
        }

        if action == action_id("feed-forget") {
            if let Some(index) = self.menu_open.take() {
                if index < self.subscriptions.len() {
                    context
                        .store()
                        .forget(feed_cache_key(&self.subscriptions[index].url));
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
                    context
                        .store()
                        .forget(feed_cache_key(&self.subscriptions[index].url));
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
        let Some(pending) = self.task.clone() else {
            return;
        };
        if pending.id != task {
            return;
        }
        self.task = None;
        if pending
            .target
            .miniflux_server()
            .is_some_and(|server| server != self.server)
        {
            // A server edit cancels and detaches its old request. If a late
            // outcome still reaches the app, it cannot populate or mutate the
            // newly selected host's namespace.
            return;
        }
        if matches!(
            pending.target,
            TaskTarget::FluxEntries { .. }
                | TaskTarget::FluxDiscover { .. }
                | TaskTarget::FluxSubscribe { .. }
                | TaskTarget::FluxFull { .. }
                | TaskTarget::FluxMutation { .. }
        ) {
            self.on_flux_task(context, pending.target, outcome);
            self.show(context);
            return;
        }
        match outcome {
            TaskOutcome::Completed(bytes) => match pending.target {
                TaskTarget::Search => {
                    self.found = search::results(&bytes);
                    if self.found.is_empty() {
                        // A search answer is JSON, and JSON that stops halfway
                        // is not JSON at all, so a cut answer yields nothing
                        // and looks exactly like a site with no feeds.
                        self.problem = truncated(&bytes, SEARCH_BYTES)
                            .then(|| "That site's answer was too large to read.".to_owned());
                    }
                }
                TaskTarget::Feed { subscription_url } => {
                    match feed::parse_at(&bytes, &subscription_url) {
                        Some(parsed) => {
                            self.live_cache[0] = true;
                            context.store().save(
                                feed_cache_key(&subscription_url),
                                encode_feed_cache(&parsed),
                            );
                            if self
                                .open
                                .and_then(|index| self.subscriptions.get(index))
                                .is_some_and(|subscription| subscription.url == subscription_url)
                            {
                                self.items.clone_from(&parsed.items);
                            }
                            // A feed usually names itself better than a search
                            // result does, so the shelf takes the better name once
                            // it has been read.
                            if let Some(subscription) = self
                                .subscriptions
                                .iter_mut()
                                .find(|subscription| subscription.url == subscription_url)
                            {
                                if !parsed.title.trim().is_empty()
                                    && subscription.title != parsed.title
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
                    }
                }
                TaskTarget::FluxEntries { .. }
                | TaskTarget::FluxDiscover { .. }
                | TaskTarget::FluxSubscribe { .. }
                | TaskTarget::FluxFull { .. }
                | TaskTarget::FluxMutation { .. } => unreachable!("handled above"),
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
        article_text, byline, decode, encode, encode_feed_cache, encode_flux_cache, feed_cache_key,
        miniflux, pretty_host, search, Backend, Feeds, PendingTask, Setting, Subscription,
        TaskKind, TaskTarget, View, CONFIG, FEED_BYTES, FULL_CONTENT_BYTES, ITEM_STATES, MAX_FEEDS,
        SEARCH_BYTES,
    };
    use kobo_sdk::{
        action_id, AppRunner, Command, Credential, StoreResult, Task, TaskError, TaskId,
        TaskOutcome,
    };
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

    fn flux_reader() -> Feeds {
        Feeds {
            loaded: true,
            backend: Backend::Miniflux,
            server: "https://flux.example".to_owned(),
            view: View::FluxShelf,
            ..Feeds::default()
        }
    }

    fn flux_actions(server: &str) -> String {
        miniflux::actions_key(server).expect("valid test server")
    }

    fn flux_full_index(server: &str) -> String {
        miniflux::full_index_key(server).expect("valid test server")
    }

    fn flux_full_content(server: &str, id: u64) -> String {
        miniflux::full_content_key(server, id).expect("valid test server")
    }

    fn pending_feed() -> PendingTask {
        PendingTask {
            id: TaskId(1),
            target: TaskTarget::Feed {
                subscription_url: "https://example.com/feed.xml".to_owned(),
            },
        }
    }

    fn pending_search() -> PendingTask {
        PendingTask {
            id: TaskId(1),
            target: TaskTarget::Search,
        }
    }

    fn spawned(commands: &[Command]) -> (TaskId, Task) {
        commands
            .iter()
            .find_map(|command| match command {
                Command::Spawn { task, work } => Some((*task, work.clone())),
                _ => None,
            })
            .expect("the app spawned its requested work")
    }

    #[test]
    fn standalone_uses_the_requested_url_then_caches_and_persists_entry_state() {
        let mut runner = AppRunner::new(Feeds {
            loaded: true,
            view: View::Items,
            open: Some(0),
            subscriptions: following(),
            task: Some(pending_feed()),
            ..Feeds::default()
        });
        let source = br"<rss><channel><title>Journal</title><item><guid>entry-1</guid><title>Relative</title><link>stories/one</link><description>Readable offline.</description></item></channel></rss>";
        let commands = runner.task_outcome(TaskId(1), TaskOutcome::Completed(source.to_vec()));
        assert_eq!(
            runner.app().items[0].link,
            "https://example.com/stories/one",
            "the exact subscription URL was the relative-link base"
        );
        assert_eq!(runner.app().items[0].id, "entry-1");
        assert!(
            commands.iter().any(|command| matches!(
                command,
                Command::Store(kobo_sdk::StoreRequest::Save { key, value })
                    if key.starts_with("feed-cache-") && value.len() <= kobo_sdk::MAX_STORE_VALUE
            )),
            "a successful standalone fetch was not made readable offline"
        );

        runner.action(action_id("item-0"));
        let commands = runner.action(action_id("toggle-read"));
        assert!(runner.app().item_states[0].read);
        assert!(commands.iter().any(|command| matches!(
            command,
            Command::Store(kobo_sdk::StoreRequest::Save { key, .. }) if key == ITEM_STATES
        )));
        runner.action(action_id("toggle-star"));
        assert!(runner.app().item_states[0].starred);
    }

    #[test]
    fn standalone_cache_stays_available_when_a_refresh_is_offline() {
        let cached = super::feed::Feed {
            title: "Journal".to_owned(),
            site: "https://example.com/".to_owned(),
            items: vec![super::feed::Item {
                id: "entry-1".to_owned(),
                title: "Cached".to_owned(),
                link: "https://example.com/one".to_owned(),
                body: "Still here.".to_owned(),
                ..super::feed::Item::default()
            }],
        };
        let mut runner = AppRunner::new(Feeds {
            loaded: true,
            view: View::Items,
            open: Some(0),
            subscriptions: following(),
            task: Some(pending_feed()),
            ..Feeds::default()
        });
        runner.store_result(StoreResult::Loaded {
            key: feed_cache_key("https://example.com/feed.xml"),
            value: Some(encode_feed_cache(&cached)),
        });
        assert_eq!(runner.app().items[0].title, "Cached");
        runner.task_outcome(TaskId(1), TaskOutcome::Failed(TaskError::Offline));
        assert_eq!(runner.app().items[0].body, "Still here.");
    }

    #[test]
    fn settings_switch_the_single_app_to_the_dedicated_miniflux_credential() {
        let mut runner = AppRunner::new(Feeds {
            loaded: true,
            ..Feeds::default()
        });
        runner.action(action_id("settings"));
        let commands = runner.action(action_id("mode"));
        assert_eq!(runner.app().backend, Backend::Miniflux);
        assert!(commands.iter().any(|command| matches!(
            command,
            Command::Store(kobo_sdk::StoreRequest::Save { key, value })
                if key == "config"
                    && !String::from_utf8_lossy(value).contains("token")
                    && String::from_utf8_lossy(value).lines().count() == 1
        )));
        assert_eq!(
            super::decode_config(b"miniflux\nhttps://feeds.example\npersonal-miniflux"),
            (Backend::Miniflux, "https://feeds.example".to_owned()),
            "an old development-only credential field must not broaden the new boundary"
        );
    }

    #[test]
    fn miniflux_sync_drains_durable_read_actions_then_fetches_the_selected_mode() {
        let mut runner = AppRunner::new(flux_reader());
        let commands = runner.action(action_id("flux-sync"));
        let (entries_task, work) = spawned(&commands);
        let Task::Fetch {
            url, credential, ..
        } = work
        else {
            panic!("sync must fetch entries");
        };
        assert_eq!(
            url,
            "https://flux.example/v1/entries?status=unread&limit=100&order=published_at&direction=desc"
        );
        assert_eq!(
            credential,
            Some(Credential::in_header("miniflux", "X-Auth-Token"))
        );
        runner.task_outcome(
            entries_task,
            TaskOutcome::Completed(
                br#"{"entries":[{"id":8,"title":"Story","feed":{"title":"Paper"},"content":"<p>Cached text</p>","status":"unread","starred":false}]}"#
                    .to_vec(),
            ),
        );
        runner.action(action_id("flux-entry-0"));
        let commands = runner.action(action_id("flux-toggle-read"));
        assert_eq!(runner.app().selected_flux_entries()[0].status, "read");
        assert!(runner
            .app()
            .flux_pending
            .contains(&miniflux::Mutation::Read(8)));
        assert!(commands.iter().any(|command| matches!(
            command,
            Command::Store(kobo_sdk::StoreRequest::Save { key, .. })
                if key == &flux_actions("https://flux.example")
        )));
        runner.action(action_id("flux-toggle-star"));
        assert!(runner.app().selected_flux_entries()[0].starred);
        assert!(runner
            .app()
            .flux_pending
            .contains(&miniflux::Mutation::Star {
                id: 8,
                starred: true,
            }));

        let commands = runner.action(action_id("flux-sync"));
        let (write_task, work) = spawned(&commands);
        let Task::Put {
            body, credential, ..
        } = work
        else {
            panic!("the queued read must use PUT before fetching");
        };
        assert_eq!(body, r#"{"entry_ids":[8],"status":"read"}"#);
        assert_eq!(
            credential,
            Some(Credential::in_header("miniflux", "X-Auth-Token"))
        );
        let commands = runner.task_outcome(write_task, TaskOutcome::Completed(Vec::new()));
        let (star_task, next) = spawned(&commands);
        let Task::Put { body, .. } = next else {
            panic!("the queued star must use PUT after the queued read");
        };
        assert_eq!(body, r#"{"entry_ids":[8],"starred":true}"#);
        let commands = runner.task_outcome(star_task, TaskOutcome::Completed(Vec::new()));
        assert!(runner.app().flux_pending.is_empty());
        let (_, next) = spawned(&commands);
        assert!(matches!(next, Task::Fetch { .. }));
    }

    #[test]
    fn miniflux_discovery_subscription_and_full_article_use_documented_routes() {
        let mut reader = flux_reader();
        reader.view = View::FluxDiscover;
        reader.keyboard = kobo_sdk::keyboard::Keyboard::with_text("https://example.org");
        let mut runner = AppRunner::new(reader);
        let commands = runner.action(action_id("kb.enter"));
        let (discover_task, work) = spawned(&commands);
        let Task::Post { url, body, .. } = work else {
            panic!("discovery must POST");
        };
        assert_eq!(url, "https://flux.example/v1/discover");
        assert_eq!(body, r#"{"url":"https://example.org"}"#);
        runner.task_outcome(
            discover_task,
            TaskOutcome::Completed(
                br#"[{"url":"https://example.org/feed.xml","title":"Example","type":"rss"}]"#
                    .to_vec(),
            ),
        );
        let commands = runner.action(action_id("flux-found-0"));
        let (subscribe_task, work) = spawned(&commands);
        let Task::Post { url, body, .. } = work else {
            panic!("subscription must POST");
        };
        assert_eq!(url, "https://flux.example/v1/feeds");
        assert_eq!(body, r#"{"feed_url":"https://example.org/feed.xml"}"#);

        let commands = runner.task_outcome(subscribe_task, TaskOutcome::Completed(Vec::new()));
        let (entries_task, _) = spawned(&commands);
        runner.task_outcome(
            entries_task,
            TaskOutcome::Completed(
                br#"{"entries":[{"id":9,"title":"Long","feed":{"title":"Example"},"content":"short","status":"unread","starred":false}]}"#
                    .to_vec(),
            ),
        );
        runner.action(action_id("flux-entry-0"));
        runner.action(action_id("flux-more"));
        let commands = runner.action(action_id("flux-full"));
        let (full_task, work) = spawned(&commands);
        let Task::Fetch { url, .. } = work else {
            panic!("full content must fetch");
        };
        assert_eq!(url, "https://flux.example/v1/entries/9/fetch-content");
        runner.task_outcome(
            full_task,
            TaskOutcome::Completed(br#"{"content":"<p>Full cached story</p>"}"#.to_vec()),
        );
        assert_eq!(
            runner.app().selected_flux_entries()[0].content,
            "Full cached story"
        );
    }

    #[test]
    fn miniflux_star_queue_coalesces_three_toggles_without_losing_an_in_flight_write() {
        let mut reader = flux_reader();
        reader.flux_caches[miniflux::ListMode::Unread.cache_index()] = vec![miniflux::Article {
            id: 8,
            title: "Story".to_owned(),
            feed: "Paper".to_owned(),
            content: "Cached".to_owned(),
            status: "unread".to_owned(),
            starred: false,
        }];
        reader.flux_open = Some(8);
        reader.view = View::FluxArticle;
        let mut runner = AppRunner::new(reader);

        runner.action(action_id("flux-toggle-star"));
        runner.action(action_id("flux-toggle-star"));
        runner.action(action_id("flux-toggle-star"));
        assert_eq!(
            runner.app().flux_pending,
            vec![miniflux::Mutation::Star {
                id: 8,
                starred: true
            }],
            "three offline toggles must retain only their newest desired state"
        );

        let commands = runner.action(action_id("flux-sync"));
        let (task, work) = spawned(&commands);
        assert!(matches!(
            work,
            Task::Put {
                body,
                ..
            } if body == r#"{"entry_ids":[8],"starred":true}"#
        ));
        assert_eq!(
            runner.app().flux_in_flight,
            Some(miniflux::Mutation::Star {
                id: 8,
                starred: true
            })
        );
        assert!(runner.app().flux_pending.is_empty());

        runner.action(action_id("flux-toggle-star"));
        assert_eq!(
            runner.app().flux_in_flight,
            Some(miniflux::Mutation::Star {
                id: 8,
                starred: true
            }),
            "a new desired state must not overwrite the PUT already sent"
        );
        assert_eq!(
            runner.app().flux_pending,
            vec![miniflux::Mutation::Star {
                id: 8,
                starred: false
            }]
        );
        runner.task_outcome(task, TaskOutcome::Completed(Vec::new()));
    }

    #[test]
    fn miniflux_duplicate_subscribe_and_full_content_actions_keep_their_original_targets() {
        let mut reader = flux_reader();
        reader.view = View::FluxDiscover;
        reader.flux_discovered.push(miniflux::Discovered {
            url: "https://example.org/feed.xml".to_owned(),
            title: "Example".to_owned(),
            kind: "rss".to_owned(),
        });
        let mut runner = AppRunner::new(reader);
        let commands = runner.action(action_id("flux-found-0"));
        let (subscribe_task, _) = spawned(&commands);
        let duplicate = runner.action(action_id("flux-found-0"));
        assert!(
            !duplicate
                .iter()
                .any(|command| matches!(command, Command::Spawn { .. })),
            "a second feed tap must not replace the in-flight subscription"
        );
        assert_eq!(
            runner.app().task,
            Some(PendingTask {
                id: subscribe_task,
                target: TaskTarget::FluxSubscribe {
                    server: "https://flux.example".to_owned(),
                    feed_url: "https://example.org/feed.xml".to_owned()
                }
            })
        );

        let mut reader = flux_reader();
        reader.flux_caches[miniflux::ListMode::Unread.cache_index()] = vec![miniflux::Article {
            id: 9,
            title: "First".to_owned(),
            feed: "Example".to_owned(),
            content: "Summary".to_owned(),
            status: "unread".to_owned(),
            starred: false,
        }];
        reader.flux_open = Some(9);
        reader.view = View::FluxArticle;
        let mut runner = AppRunner::new(reader);
        runner.action(action_id("flux-more"));
        let commands = runner.action(action_id("flux-full"));
        let (full_task, _) = spawned(&commands);
        let duplicate = runner.action(action_id("flux-full"));
        assert!(
            !duplicate
                .iter()
                .any(|command| matches!(command, Command::Spawn { .. })),
            "a second full-content action must not replace the first"
        );
        runner.app_mut().flux_caches[miniflux::ListMode::Unread.cache_index()].insert(
            0,
            miniflux::Article {
                id: 10,
                title: "Newer".to_owned(),
                feed: "Example".to_owned(),
                content: "Other".to_owned(),
                status: "unread".to_owned(),
                starred: false,
            },
        );
        runner.task_outcome(
            full_task,
            TaskOutcome::Completed(br#"{"content":"<p>Exact full text</p>"}"#.to_vec()),
        );
        assert_eq!(
            runner
                .app()
                .selected_flux_entries()
                .iter()
                .find(|article| article.id == 9)
                .map(|article| article.content.as_str()),
            Some("Exact full text"),
            "the full response must update its immutable entry ID, not row zero"
        );
        assert_eq!(
            runner.app().selected_flux_entries()[0].content,
            "Other",
            "a reordered row must not receive another entry's content"
        );
    }

    #[test]
    fn miniflux_cache_isolated_by_mode_and_full_content_survives_a_refresh_and_restart() {
        let mut reader = flux_reader();
        reader.flux_caches[miniflux::ListMode::Unread.cache_index()] = vec![miniflux::Article {
            id: 1,
            title: "Unread cache".to_owned(),
            feed: "Paper".to_owned(),
            content: "Unread".to_owned(),
            status: "unread".to_owned(),
            starred: false,
        }];
        reader.flux_caches[miniflux::ListMode::Starred.cache_index()] = vec![miniflux::Article {
            id: 2,
            title: "Starred cache".to_owned(),
            feed: "Paper".to_owned(),
            content: "Starred".to_owned(),
            status: "read".to_owned(),
            starred: true,
        }];
        let mut runner = AppRunner::new(reader);
        let commands = runner.action(action_id("flux-starred"));
        let (starred_task, _) = spawned(&commands);
        assert_eq!(
            runner.app().selected_flux_entries()[0].title,
            "Starred cache",
            "switching tabs must never show the unread cache under Starred"
        );
        let commands = runner.task_outcome(
            starred_task,
            TaskOutcome::Completed(
                br#"{"entries":[{"id":9,"title":"From network","feed":{"title":"Paper"},"content":"short","status":"read","starred":true}]}"#
                    .to_vec(),
            ),
        );
        assert!(commands.iter().any(|command| matches!(
            command,
            Command::Store(kobo_sdk::StoreRequest::Save { key, .. })
                if key == &miniflux::cache_key("https://flux.example", miniflux::ListMode::Starred)
                    .expect("valid test server")
        )));

        let mut initial = flux_reader();
        initial.flux_caches[miniflux::ListMode::Unread.cache_index()] = vec![miniflux::Article {
            id: 9,
            title: "Story".to_owned(),
            feed: "Paper".to_owned(),
            content: "Summary".to_owned(),
            status: "unread".to_owned(),
            starred: false,
        }];
        initial.flux_open = Some(9);
        initial.view = View::FluxArticle;
        let mut runner = AppRunner::new(initial);
        runner.action(action_id("flux-more"));
        let commands = runner.action(action_id("flux-full"));
        let (full_task, _) = spawned(&commands);
        let full = "The complete article is kept without truncation.";
        let commands = runner.task_outcome(
            full_task,
            TaskOutcome::Completed(format!(r#"{{"content":"<p>{full}</p>"}}"#).into_bytes()),
        );
        assert!(commands.iter().any(|command| matches!(
            command,
            Command::Store(kobo_sdk::StoreRequest::Save { key, value })
                if key == &flux_full_content("https://flux.example", 9) && value == full.as_bytes()
        )));

        let mut restarted = AppRunner::new(flux_reader());
        restarted.store_result(StoreResult::Loaded {
            key: flux_full_index("https://flux.example"),
            value: Some(b"9".to_vec()),
        });
        restarted.store_result(StoreResult::Loaded {
            key: flux_full_content("https://flux.example", 9),
            value: Some(full.as_bytes().to_vec()),
        });
        let commands = restarted.action(action_id("flux-sync"));
        let (entries_task, _) = spawned(&commands);
        restarted.task_outcome(
            entries_task,
            TaskOutcome::Completed(
                br#"{"entries":[{"id":9,"title":"Story","feed":{"title":"Paper"},"content":"summary again","status":"unread","starred":false}]}"#
                    .to_vec(),
            ),
        );
        assert_eq!(
            restarted.app().selected_flux_entries()[0].content,
            full,
            "a refreshed list must overlay the exact stable-ID full-content cache"
        );
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "one test follows the complete two-server persistence boundary"
    )]
    fn miniflux_server_namespaces_isolate_same_entry_ids_and_survive_restart() {
        let first = "https://one.example/reader";
        let second = "https://two.example/reader/";
        let first_article = miniflux::Article {
            id: 7,
            title: "One's entry".to_owned(),
            feed: "One".to_owned(),
            content: "One's content".to_owned(),
            status: "unread".to_owned(),
            starred: false,
        };
        let second_article = miniflux::Article {
            id: 7,
            title: "Two's entry".to_owned(),
            feed: "Two".to_owned(),
            content: "Two's content".to_owned(),
            status: "unread".to_owned(),
            starred: false,
        };
        let mut reader = flux_reader();
        reader.server = first.to_owned();
        reader.flux_caches[miniflux::ListMode::Unread.cache_index()] = vec![first_article.clone()];
        reader.flux_pending.push(miniflux::Mutation::Read(7));
        reader.flux_in_flight = Some(miniflux::Mutation::Star {
            id: 7,
            starred: true,
        });
        reader.task = Some(PendingTask {
            id: TaskId(44),
            target: TaskTarget::FluxEntries {
                server: first.to_owned(),
                mode: miniflux::ListMode::Unread,
            },
        });
        reader.editing = Some(Setting::Server);
        reader.keyboard = kobo_sdk::keyboard::Keyboard::with_text(second);
        let mut runner = AppRunner::new(reader);
        let commands = runner.action(action_id("kb.enter"));
        assert_eq!(runner.app().server, "https://two.example/reader");
        assert!(runner.app().task.is_none());
        assert!(runner.app().flux_pending.is_empty());
        assert!(runner.app().flux_in_flight.is_none());
        assert!(runner.app().selected_flux_entries().is_empty());
        assert!(commands
            .iter()
            .any(|command| matches!(command, Command::Cancel(TaskId(44)))));
        assert!(commands.iter().any(|command| matches!(
            command,
            Command::Store(kobo_sdk::StoreRequest::Save { key, value })
                if key == &flux_actions(first)
                    && String::from_utf8_lossy(value).contains("r\t7")
                    && String::from_utf8_lossy(value).contains("s\t7\ttrue")
        )));
        assert!(commands.iter().any(|command| matches!(
            command,
            Command::Store(kobo_sdk::StoreRequest::Load { key })
                if key == &miniflux::cache_key(second, miniflux::ListMode::Unread)
                    .expect("valid test server")
        )));
        assert!(!commands.iter().any(|command| matches!(
            command,
            Command::Store(kobo_sdk::StoreRequest::Forget { key })
                if key.starts_with("miniflux.")
        )));

        // A delayed response from the old namespace must not repopulate the
        // new host, even though both servers use entry ID 7.
        runner.store_result(StoreResult::Loaded {
            key: miniflux::cache_key(first, miniflux::ListMode::Unread).expect("valid test server"),
            value: Some(encode_flux_cache(&[first_article])),
        });
        assert!(runner.app().selected_flux_entries().is_empty());
        runner.store_result(StoreResult::Loaded {
            key: miniflux::cache_key(second, miniflux::ListMode::Unread)
                .expect("valid test server"),
            value: Some(encode_flux_cache(std::slice::from_ref(&second_article))),
        });
        assert_eq!(
            runner.app().selected_flux_entries(),
            std::slice::from_ref(&second_article)
        );

        let mut restarted = AppRunner::new(Feeds::default());
        let startup = restarted.start();
        assert!(startup.iter().any(|command| matches!(
            command,
            Command::Store(kobo_sdk::StoreRequest::Load { key }) if key == CONFIG
        )));
        let startup = restarted.store_result(StoreResult::Loaded {
            key: CONFIG.to_owned(),
            value: Some(b"miniflux\nhttps://two.example/reader/".to_vec()),
        });
        assert!(startup.iter().any(|command| matches!(
            command,
            Command::Store(kobo_sdk::StoreRequest::Load { key })
                if key == &miniflux::cache_key(second, miniflux::ListMode::Unread)
                    .expect("valid test server")
        )));
        restarted.store_result(StoreResult::Loaded {
            key: miniflux::cache_key(first, miniflux::ListMode::Unread).expect("valid test server"),
            value: Some(encode_flux_cache(&[miniflux::Article {
                id: 7,
                title: "Stale".to_owned(),
                ..second_article.clone()
            }])),
        });
        restarted.store_result(StoreResult::Loaded {
            key: miniflux::cache_key(second, miniflux::ListMode::Unread)
                .expect("valid test server"),
            value: Some(encode_flux_cache(std::slice::from_ref(&second_article))),
        });
        assert_eq!(restarted.app().selected_flux_entries(), &[second_article]);
    }

    #[test]
    fn equivalent_server_edits_keep_the_same_namespace_and_pending_work() {
        let raw = "https://FLUX.example:443/reader/";
        let mut reader = flux_reader();
        reader.server = raw.to_owned();
        reader.flux_pending.push(miniflux::Mutation::Read(7));
        reader.task = Some(PendingTask {
            id: TaskId(44),
            target: TaskTarget::FluxEntries {
                server: raw.to_owned(),
                mode: miniflux::ListMode::Unread,
            },
        });
        reader.editing = Some(Setting::Server);
        reader.keyboard = kobo_sdk::keyboard::Keyboard::with_text("https://flux.example/reader");
        let mut runner = AppRunner::new(reader);

        let commands = runner.action(action_id("kb.enter"));

        assert_eq!(runner.app().server, raw);
        assert_eq!(runner.app().flux_pending, vec![miniflux::Mutation::Read(7)]);
        assert_eq!(
            runner.app().task.as_ref().map(|task| task.id),
            Some(TaskId(44))
        );
        assert!(
            !commands
                .iter()
                .any(|command| matches!(command, Command::Cancel(TaskId(44)))),
            "equivalent settings must not detach their existing request"
        );
    }

    #[test]
    fn oversized_full_content_keeps_the_existing_exact_article_and_is_not_saved() {
        let mut reader = flux_reader();
        reader.flux_caches[miniflux::ListMode::Unread.cache_index()] = vec![miniflux::Article {
            id: 9,
            title: "Story".to_owned(),
            feed: "Paper".to_owned(),
            content: "Previously saved full article.".to_owned(),
            status: "unread".to_owned(),
            starred: false,
        }];
        reader.flux_open = Some(9);
        reader.view = View::FluxArticle;
        let mut runner = AppRunner::new(reader);
        runner.action(action_id("flux-more"));
        let commands = runner.action(action_id("flux-full"));
        let (task, _) = spawned(&commands);
        let content = "x".repeat(FULL_CONTENT_BYTES * 2);
        let commands = runner.task_outcome(
            task,
            TaskOutcome::Completed(format!(r#"{{"content":"<p>{content}</p>"}}"#).into_bytes()),
        );
        assert_eq!(
            runner.app().selected_flux_entries()[0].content,
            "Previously saved full article."
        );
        assert!(
            !commands.iter().any(|command| matches!(
                command,
                Command::Store(kobo_sdk::StoreRequest::Save { key, .. })
                    if key == &flux_full_content("https://flux.example", 9)
            )),
            "a truncated full article must never replace the exact stored value"
        );
        assert!(runner
            .app()
            .problem
            .as_deref()
            .is_some_and(|problem| problem.contains("too large to save")));
    }

    #[test]
    fn miniflux_modes_and_failures_keep_cached_entries_and_explain_the_problem() {
        let mut reader = flux_reader();
        reader.flux_caches[miniflux::ListMode::History.cache_index()] = vec![miniflux::Article {
            id: 3,
            title: "Cached".to_owned(),
            feed: "Paper".to_owned(),
            content: "Readable".to_owned(),
            status: "unread".to_owned(),
            starred: true,
        }];
        let mut runner = AppRunner::new(reader);
        let commands = runner.action(action_id("flux-history"));
        let (task, work) = spawned(&commands);
        let Task::Fetch { url, .. } = work else {
            panic!("history must fetch");
        };
        assert!(url.contains("status=read"), "{url}");
        runner.task_outcome(task, TaskOutcome::Failed(TaskError::Unauthorized));
        assert_eq!(runner.app().selected_flux_entries()[0].title, "Cached");
        assert!(runner
            .app()
            .problem
            .as_deref()
            .is_some_and(|message| message.contains("401 Unauthorized")));

        let mut reader = flux_reader();
        reader.flux_caches[miniflux::ListMode::Starred.cache_index()] = vec![miniflux::Article {
            id: 4,
            title: "Offline cache".to_owned(),
            feed: "Paper".to_owned(),
            content: "Readable".to_owned(),
            status: "unread".to_owned(),
            starred: false,
        }];
        let mut runner = AppRunner::new(reader);
        let commands = runner.action(action_id("flux-starred"));
        let (task, work) = spawned(&commands);
        let Task::Fetch { url, .. } = work else {
            panic!("starred must fetch");
        };
        assert!(url.contains("starred=true"), "{url}");
        runner.task_outcome(task, TaskOutcome::Failed(TaskError::Offline));
        assert_eq!(
            runner.app().selected_flux_entries()[0].title,
            "Offline cache"
        );
        assert!(runner
            .app()
            .problem
            .as_deref()
            .is_some_and(|message| message.starts_with("Offline")));
    }

    #[test]
    fn invalid_completed_miniflux_entries_keep_ram_and_persisted_cache() {
        let mode = miniflux::ListMode::Unread;
        let mut reader = flux_reader();
        reader.flux_caches[mode.cache_index()] = vec![miniflux::Article {
            id: 4,
            title: "Cached".to_owned(),
            feed: "Paper".to_owned(),
            content: "Readable".to_owned(),
            status: "unread".to_owned(),
            starred: false,
        }];
        let mut runner = AppRunner::new(reader);
        let (task, _) = spawned(&runner.action(action_id("flux-sync")));

        let commands = runner.task_outcome(task, TaskOutcome::Completed(b"{not JSON".to_vec()));

        assert_eq!(runner.app().selected_flux_entries()[0].title, "Cached");
        assert!(runner
            .app()
            .problem
            .as_deref()
            .is_some_and(|message| message.contains("invalid")));
        assert!(
            !commands.iter().any(|command| matches!(
                command,
                Command::Store(kobo_sdk::StoreRequest::Save { key, .. })
                    if key == &miniflux::cache_key("https://flux.example", mode)
                        .expect("valid test server")
            )),
            "an invalid completed response must not overwrite the persisted cache"
        );
    }

    #[test]
    fn empty_completed_miniflux_entries_replace_the_cached_mode() {
        let mode = miniflux::ListMode::Unread;
        let mut reader = flux_reader();
        reader.flux_caches[mode.cache_index()] = vec![miniflux::Article {
            id: 4,
            title: "Cached".to_owned(),
            feed: "Paper".to_owned(),
            content: "Readable".to_owned(),
            status: "unread".to_owned(),
            starred: false,
        }];
        let mut runner = AppRunner::new(reader);
        let (task, _) = spawned(&runner.action(action_id("flux-sync")));

        let commands =
            runner.task_outcome(task, TaskOutcome::Completed(br#"{"entries":[]}"#.to_vec()));

        assert!(runner.app().selected_flux_entries().is_empty());
        assert!(runner.app().flux_live_cache[mode.cache_index()]);
        assert!(commands.iter().any(|command| matches!(
            command,
            Command::Store(kobo_sdk::StoreRequest::Save { key, .. })
                if key == &miniflux::cache_key("https://flux.example", mode)
                    .expect("valid test server")
        )));
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
        assert!(application.awaiting(TaskKind::Feed));
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
            task: Some(pending_feed()),
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
            task: Some(pending_feed()),
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
            task: Some(pending_feed()),
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
            task: Some(pending_feed()),
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
            task: Some(pending_feed()),
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
            id: "first-post".to_owned(),
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
            task: Some(pending_feed()),
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
                task: Some(pending_feed()),
                ..Feeds::default()
            });
            runner.task_outcome(TaskId(1), TaskOutcome::Failed(error));
            let said = runner.app_mut().problem.clone().unwrap_or_default();
            assert_eq!(said, kobo_sdk::Failure::of(error).advice);
            assert!(said.contains(expected), "{error:?} was worded as {said:?}");
        }
    }

    #[test]
    fn miniflux_lists_and_articles_fit_the_clara_panel() {
        let mut reader = flux_reader();
        reader.flux_caches[miniflux::ListMode::Unread.cache_index()] = (0..30)
            .map(|id| miniflux::Article {
                id,
                title: format!("A deliberately long Miniflux article title number {id}"),
                feed: "A publication with a long enough name to be measured".to_owned(),
                content: "A cached full article remains readable when the network is unavailable. "
                    .repeat(30),
                status: "unread".to_owned(),
                starred: id == 0,
            })
            .collect();
        let mut runner = AppRunner::new(reader);
        let list = screen_of(&runner.action(action_id("list-next")));
        fits_the_panel(&list, "a Miniflux entry list");
        let article = screen_of(&runner.action(action_id("flux-entry-0")));
        fits_the_panel(&article, "a Miniflux article");

        let mut settings = AppRunner::new(flux_reader());
        let settings = screen_of(&settings.action(action_id("settings")));
        fits_the_panel(&settings, "Miniflux settings");
    }

    #[test]
    fn miniflux_more_menu_claims_back_without_leaving_the_article() {
        let mut reader = flux_reader();
        reader.flux_caches[miniflux::ListMode::Unread.cache_index()] = vec![miniflux::Article {
            id: 1,
            title: "Entry".to_owned(),
            feed: "Feed".to_owned(),
            content: "Cached text.".to_owned(),
            status: "unread".to_owned(),
            starred: false,
        }];
        let mut runner = AppRunner::new(reader);
        runner.action(action_id("flux-entry-0"));
        let more = screen_of(&runner.action(action_id("flux-more")));
        assert!(more.owns_back);
        runner.action(kobo_sdk::ActionId::BACK);
        assert_eq!(runner.app().view, View::FluxArticle);
        assert!(!runner.app().flux_menu_open);
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
            task: Some(pending_feed()),
            ..Feeds::default()
        });
        let commands =
            runner.task_outcome(TaskId(1), TaskOutcome::Completed(b"<html></html>".to_vec()));
        fits_the_panel(&screen_of(&commands), "a feed that was not a feed");

        let mut runner = AppRunner::new(Feeds {
            loaded: true,
            view: View::Found,
            query: "example.com".to_owned(),
            task: Some(pending_search()),
            ..Feeds::default()
        });
        let commands = runner.task_outcome(TaskId(1), TaskOutcome::Completed(b"[]".to_vec()));
        fits_the_panel(&screen_of(&commands), "a search that found nothing");

        let mut runner = AppRunner::new(Feeds {
            loaded: true,
            view: View::Items,
            open: Some(0),
            subscriptions: following(),
            task: Some(pending_feed()),
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
            task: Some(pending_search()),
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
            task: Some(pending_feed()),
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
            task: Some(pending_feed()),
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
            task: Some(pending_search()),
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
            task: Some(pending_search()),
            ..Feeds::default()
        });
        let commands = runner.task_outcome(TaskId(1), TaskOutcome::Completed(b"[]".to_vec()));
        let screen = screen_of(&commands);
        assert!(!format!("{screen:?}").contains("too large"));
    }
}

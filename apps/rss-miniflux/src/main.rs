mod miniflux;
mod pending;

use kobo_bookview::illustrations::Illustrations;
use kobo_bookview::positions::{Progress, KEY as READING};
use kobo_bookview::BookView;
use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::snapshot::{Snapshot, SnapshotEvent};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Glyph, KoboApp, Position, Screen, ScreenBuilder,
    StoreResult, TaskId, TaskOutcome,
};
use miniflux::{Article, Status};
use pending::{Change, Pending};
use std::process::ExitCode;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum View {
    Shelf,
    Reading,
    Settings,
    Directory,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Setting {
    Server,
    Credential,
}

/// Which articles the shelf is showing.
///
/// Three views of one downloaded batch rather than three requests. A reader on
/// a train who taps Starred is not owed a spinner: everything the tabs draw
/// arrived together and is already on the device.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Tab {
    #[default]
    Unread,
    Starred,
    History,
}

impl Tab {
    const ORDER: [Self; 3] = [Self::Unread, Self::Starred, Self::History];

    const fn index(self) -> usize {
        match self {
            Self::Unread => 0,
            Self::Starred => 1,
            Self::History => 2,
        }
    }

    const fn action(self) -> &'static str {
        match self {
            Self::Unread => "unread",
            Self::Starred => "starred",
            Self::History => "history",
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Unread => "Unread",
            Self::Starred => "Starred",
            Self::History => "History",
        }
    }

    const fn nothing_here(self) -> (&'static str, &'static str) {
        match self {
            Self::Unread => ("No unread articles", "Sync to check for new ones."),
            Self::Starred => (
                "No starred articles",
                "Star an article and it is kept here.",
            ),
            Self::History => (
                "Nothing read yet",
                "Articles you have read appear here after they are opened.",
            ),
        }
    }
}

/// What the outstanding request will answer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Awaiting {
    /// One of the three lists a sync is made of, by its place in the order.
    Sync(usize),
    /// The page behind a summary-only article.
    Full(u64),
    /// The change at the head of the queue.
    Change(u64, Change),
    /// Where a chosen feed can be filed.
    Categories(usize),
    /// Following a chosen feed.
    Subscribe(usize),
}

/// Public feeds, opened only after the reader chooses one.
const STARTER_FEEDS: &[(&str, &str, &str)] = &[
    (
        "BBC Science & Environment",
        "Science and environmental reporting",
        "https://feeds.bbci.co.uk/news/science_and_environment/rss.xml",
    ),
    (
        "NASA Science",
        "Space, Earth and discoveries from NASA",
        "https://science.nasa.gov/feed/",
    ),
];

const CONFIG: &str = "config";

#[derive(Default)]
struct Reader {
    server: String,
    credential: String,
    articles: Vec<Article>,
    /// The lists of a sync in progress, merged as they arrive.
    arriving: Vec<Article>,
    tab: Tab,
    page: usize,
    open: Option<usize>,
    menu_open: Option<usize>,
    view: Option<View>,
    snapshot: Option<Snapshot>,
    book: BookView,
    illustrations: Illustrations,
    progress: Progress,
    reading_id: Option<String>,
    changes: Pending,
    /// Whether the last change sent came back unanswered.
    unreachable: bool,
    task: Option<(TaskId, Awaiting)>,
    notice: Option<String>,
    keyboard: Keyboard,
    editing: Option<Setting>,
}

impl Reader {
    fn configured(&self) -> bool {
        self.server.starts_with("https://") && !self.credential.is_empty()
    }

    fn open_snapshot(&mut self, context: &mut Context) {
        if !self.configured() {
            return;
        }
        let snapshot = Snapshot::new(&format!(
            "miniflux-articles-v1:{}\n{}",
            self.server, self.credential
        ))
        .at_most(miniflux::MAX_RESPONSE);
        snapshot.start(context);
        self.snapshot = Some(snapshot);
    }

    fn snapshot_event(&mut self, event: Option<SnapshotEvent>) {
        match event {
            Some(SnapshotEvent::Loaded) => {
                if let Some(bytes) = self.snapshot.as_ref().and_then(|s| s.bytes.as_deref()) {
                    match miniflux::parse_entries(bytes) {
                        Ok(articles) => {
                            self.articles = articles;
                            self.page = 0;
                        }
                        Err(_) => {
                            self.notice = Some(
                                "Saved articles could not be read. Sync to download them again."
                                    .into(),
                            );
                        }
                    }
                }
            }
            Some(SnapshotEvent::Failed) => {
                self.notice = Some(
                    "Articles could not be saved or opened. Retry saving before closing.".into(),
                );
            }
            // A save that landed says nothing: the articles were already on
            // screen, and a line congratulating the store would push the
            // reader's list down a row for no news at all.
            Some(SnapshotEvent::Saved) | None => {}
        }
    }

    /// Writes the batch back out, including anything fetched since the sync.
    fn keep_articles(&mut self, context: &mut Context) {
        let bytes = miniflux::encode_entries(&self.articles);
        if !self
            .snapshot
            .as_mut()
            .is_some_and(|snapshot| snapshot.save(context, bytes))
        {
            self.notice = Some("New articles are not saved. Retry saving before closing.".into());
        }
    }

    // What the reader sees, which is the downloaded article with every local
    // change laid over it. A change that has not reached Miniflux is still
    // something this reader did, and a star that disappeared until the next
    // sync would read as the tap having missed.

    fn shown_status(&self, article: &Article) -> Status {
        self.changes
            .changes_to(article.id)
            .fold(article.status, |status, change| match change {
                Change::Read => Status::Read,
                Change::Unread => Status::Unread,
                Change::Archive => Status::Removed,
                Change::Star | Change::Unstar => status,
            })
    }

    fn shown_starred(&self, article: &Article) -> bool {
        self.changes
            .changes_to(article.id)
            .fold(article.starred, |starred, change| match change {
                Change::Star => true,
                Change::Unstar => false,
                _ => starred,
            })
    }

    /// The articles this tab draws, as indices into the downloaded batch.
    fn listed(&self) -> Vec<usize> {
        self.articles
            .iter()
            .enumerate()
            .filter(|(_, article)| {
                let status = self.shown_status(article);
                match self.tab {
                    _ if status == Status::Removed => false,
                    Tab::Unread => status == Status::Unread,
                    Tab::Starred => self.shown_starred(article),
                    Tab::History => status == Status::Read,
                }
            })
            .map(|(index, _)| index)
            .collect()
    }

    fn retryable(&self) -> bool {
        self.snapshot.as_ref().is_some_and(Snapshot::retryable)
            || self.changes.failed
            || self.progress.failed
            || self.illustrations.can_retry()
    }

    /// The one line the shelf says about itself.
    ///
    /// Ordered by what a reader can do something about: something that needs
    /// retrying comes before a count of changes still to go out, which is only
    /// news.
    fn notice(&self) -> Option<String> {
        if let Some(notice) = &self.notice {
            return Some(notice.clone());
        }
        if self.changes.failed {
            return Some("Your changes are not saved on this Kobo yet.".into());
        }
        if self.progress.failed {
            return Some("Reading positions are not saved.".into());
        }
        if self.illustrations.can_retry() {
            return Some("Some article images are not saved.".into());
        }
        // One sentence for both halves of the same fact. A reader whose train
        // went into a tunnel is owed "it did not go, and it is not lost",
        // which two separate lines said less clearly than one does.
        let waiting = self.changes.len();
        match (waiting, self.unreachable) {
            (0, _) => None,
            (1, false) => Some("1 change waiting for Miniflux.".into()),
            (waiting, false) => Some(format!("{waiting} changes waiting for Miniflux.")),
            (1, true) => Some(
                "Miniflux did not answer. 1 change is kept and goes out on the next sync.".into(),
            ),
            (waiting, true) => Some(format!(
                "Miniflux did not answer. {waiting} changes are kept and go out on the next sync."
            )),
        }
    }

    fn shelf_prefix(&self, notice: Option<&str>) -> ScreenBuilder {
        let mut screen = ScreenBuilder::new("rss-miniflux")
            .top_bar("RSS Reader")
            .top_bar_glyph("sync", "Sync", Glyph::Refresh)
            .top_bar_glyph("settings", "Settings", Glyph::Settings)
            .tabs(
                self.tab.index(),
                Tab::ORDER.map(|tab| (tab.action(), tab.label())),
            );
        if let Some(notice) = notice {
            screen = screen.banner(
                if self.retryable() {
                    BannerLevel::Attention
                } else {
                    BannerLevel::Info
                },
                notice,
            );
        }
        if self.retryable() {
            screen = screen.bottom_action_marked("retry-save", "Retry saving", Glyph::Refresh);
        }
        screen
    }

    fn shelf_rows(&self, context: &Context, listed: &[usize]) -> Vec<(String, String)> {
        listed
            .iter()
            .map(|&index| {
                let article = &self.articles[index];
                let mut summary = article.feed.clone();
                if self.shown_starred(article) {
                    summary.push_str(" · Starred");
                }
                if self.tab != Tab::Unread && self.shown_status(article) == Status::Unread {
                    summary.push_str(" · Unread");
                }
                (
                    context.clamped_row_with_menu(&article.title, 2, self.nav_bar()),
                    context.one_line_row_with_menu(&summary, self.nav_bar()),
                )
            })
            .collect()
    }

    fn nav_bar(&self) -> bool {
        self.retryable()
    }

    fn shelf_pages(
        &self,
        context: &Context,
        rows: &[(String, String)],
        notice: Option<&str>,
    ) -> Vec<Vec<usize>> {
        let borrowed = rows
            .iter()
            .map(|(title, summary)| (title.as_str(), summary.as_str()))
            .collect::<Vec<_>>();
        let pages = context.paginate_rows_with_menu_under(
            &borrowed,
            self.nav_bar(),
            Position::AtTheFoot,
            &self.shelf_prefix(notice).build(),
        );
        if pages.is_empty() {
            vec![Vec::new()]
        } else {
            pages
        }
    }

    fn shelf(&self, context: &Context) -> Screen {
        let notice = self.notice();
        let screen = self.shelf_prefix(notice.as_deref());
        let listed = self.listed();
        if listed.is_empty() {
            let (heading, detail) = self.tab.nothing_here();
            return screen.splash(Some(Glyph::Rss), heading, detail).build();
        }
        let rows = self.shelf_rows(context, &listed);
        let pages = self.shelf_pages(context, &rows, notice.as_deref());
        let page = self.page.min(pages.len().saturating_sub(1));
        let shown = pages.get(page).cloned().unwrap_or_default();
        let mut screen = screen.rows_with_menu(shown.iter().map(|&row| {
            let index = listed[row];
            (
                format!("article-{index}"),
                rows[row].0.clone(),
                rows[row].1.clone(),
                if self.shown_starred(&self.articles[index]) {
                    Glyph::Bookmark
                } else {
                    Glyph::Rss
                },
                format!("article-menu-{index}"),
            )
        }));
        // The menu belongs to the row that opened it, and only while that row
        // is on the panel: a page turn with one open would hang a popover off
        // a control that is no longer drawn.
        if let Some(open) = self
            .menu_open
            .filter(|open| shown.iter().any(|&row| listed[row] == *open))
        {
            let article = &self.articles[open];
            let mut items = vec![if self.shown_starred(article) {
                ("unstar", "Remove star", Glyph::Bookmark)
            } else {
                ("star", "Star", Glyph::Bookmark)
            }];
            items.push(if self.shown_status(article) == Status::Unread {
                ("mark-read", "Mark read", Glyph::Check)
            } else {
                ("keep-unread", "Keep unread", Glyph::Circle)
            });
            items.push(("full-article", "Load full article", Glyph::Download));
            items.push(("archive", "Archive", Glyph::Trash));
            screen = screen.row_overflow(format!("article-menu-{open}"), true, items);
        }
        if pages.len() > 1 {
            screen = screen.page_turns("previous", "next").page_position(
                u16::try_from(page + 1).unwrap_or(u16::MAX),
                u16::try_from(pages.len()).unwrap_or(u16::MAX),
            );
        }
        screen.build()
    }

    fn settings(&self) -> Screen {
        ScreenBuilder::new("rss-miniflux")
            .top_bar("RSS Reader settings")
            .field("server", &self.server, "https://miniflux.example")
            .field("credential", &self.credential, "miniflux")
            .secondary(
                "Install the API token on your computer with \
                 `kobo secret set miniflux`. The token stays in the runtime; \
                 this Kobo only knows its name.",
            )
            .button("directory", "Suggested feeds")
            .button("back", "Back")
            .build()
    }

    fn directory(&self, context: &Context) -> Screen {
        let rows: Vec<_> = STARTER_FEEDS
            .iter()
            .map(|(title, description, _)| (*title, *description))
            .collect();
        let pages = context.paginate_rows(&rows, false);
        let page = self.page.min(pages.len().saturating_sub(1));
        let mut screen = ScreenBuilder::new("rss-miniflux").top_bar("Suggested feeds");
        if let Some(notice) = &self.notice {
            screen = screen.banner(BannerLevel::Info, notice.clone());
        }
        screen = screen.rows(pages.get(page).into_iter().flatten().map(|&index| {
            (
                format!("starter-{index}"),
                rows[index].0,
                rows[index].1,
                Glyph::Rss,
            )
        }));
        screen
            .secondary("A feed is added to your Miniflux account after you choose it.")
            .button("back", "Back")
            .build()
    }

    fn reading(&self) -> Screen {
        let title = self
            .open
            .and_then(|index| self.articles.get(index))
            .map_or_else(String::new, |article| article.title.clone());
        self.book.screen(&title).unwrap_or_else(|| {
            ScreenBuilder::new("rss-miniflux")
                .top_bar(title)
                .empty_state("This article arrived empty. Load the full article to read it.")
                .build()
        })
    }

    fn show(&mut self, context: &mut Context) {
        let view = self.view.unwrap_or(View::Shelf);
        if let Some(setting) = self.editing {
            let prompt = match setting {
                Setting::Server => "Miniflux HTTPS server",
                Setting::Credential => "Credential name",
            };
            context.set_screen(
                ScreenBuilder::new("rss-miniflux")
                    .top_bar("RSS Reader settings")
                    .typed(&self.keyboard, prompt)
                    .keyboard(&self.keyboard, "Save")
                    .build()
                    .with_own_back(true),
            );
            return;
        }
        let screen = match view {
            View::Shelf if !self.configured() => ScreenBuilder::new("rss-miniflux")
                .top_bar("RSS Reader")
                .splash(
                    Some(Glyph::Rss),
                    "Connect Miniflux",
                    "On your computer run `kobo secret set miniflux`, then add the HTTPS address here.",
                )
                .primary_button("settings", "Add address")
                .button("directory", "Suggested feeds")
                .build(),
            View::Shelf => self.shelf(context),
            View::Reading => self.reading(),
            View::Settings => self.settings(),
            View::Directory => self.directory(context),
        };
        // Every view except the shelf was reached from another one, so Back
        // unwinds this application first and leaves it only from the shelf.
        context.set_screen(screen.with_own_back(view != View::Shelf || self.menu_open.is_some()));
    }

    fn spawn(&mut self, context: &mut Context, task: kobo_sdk::Task, awaiting: Awaiting) -> bool {
        if self.task.is_some() {
            return false;
        }
        // A change is sent once even through the retrying helper, because a
        // lost reply can follow an applied change.
        let started = context.spawn_retrying(task);
        if let Some(id) = started {
            self.task = Some((id, awaiting));
            return true;
        }
        self.notice = Some("This Kobo could not start the request. Try again.".into());
        false
    }

    fn sync(&mut self, context: &mut Context) {
        if !self.configured() {
            self.view = Some(View::Settings);
            return;
        }
        if self.task.is_some() {
            return;
        }
        if self.snapshot.is_none() {
            self.open_snapshot(context);
            self.notice = Some("Opening saved articles. Choose Sync when they are ready.".into());
            return;
        }
        if self.snapshot.as_ref().is_some_and(Snapshot::busy) {
            self.notice = Some(
                "Saved articles are still being opened or updated. Try Sync again shortly.".into(),
            );
            return;
        }
        // Changes first. A sync that ran before them would download the state
        // they are about to change and show the reader their own tap undone.
        if self.flush_changes(context) {
            self.notice = Some("Sending your changes to Miniflux…".into());
            return;
        }
        self.notice = Some("Syncing articles…".into());
        self.unreachable = false;
        self.arriving.clear();
        self.fetch_part(context, 0);
    }

    /// Asks for one of the three lists a sync is made of.
    fn fetch_part(&mut self, context: &mut Context, at: usize) {
        let Some((part, depth)) = miniflux::Part::ORDER.get(at).copied() else {
            self.batch_arrived(context);
            return;
        };
        let task = miniflux::entries(&self.server, &self.credential, part, depth);
        self.spawn(context, task, Awaiting::Sync(at));
    }

    /// Sends the change at the head of the queue, if there is one to send.
    fn flush_changes(&mut self, context: &mut Context) -> bool {
        if !self.configured() || self.task.is_some() {
            return false;
        }
        let Some((id, change)) = self.changes.next_change() else {
            return false;
        };
        let task = match change {
            Change::Read => miniflux::set_status(&self.server, &self.credential, id, Status::Read),
            Change::Unread => {
                miniflux::set_status(&self.server, &self.credential, id, Status::Unread)
            }
            Change::Archive => {
                miniflux::set_status(&self.server, &self.credential, id, Status::Removed)
            }
            Change::Star => miniflux::set_starred(&self.server, &self.credential, id, true),
            Change::Unstar => miniflux::set_starred(&self.server, &self.credential, id, false),
        };
        if self.spawn(context, task, Awaiting::Change(id, change)) {
            self.changes.begin();
            return true;
        }
        false
    }

    fn change(&mut self, context: &mut Context, index: usize, change: Change) {
        let Some(article) = self.articles.get(index) else {
            return;
        };
        let id = article.id;
        if !self.changes.push(context, id, change) {
            self.notice = Some("This Kobo cannot hold more unsent changes. Sync first.".into());
            return;
        }
        self.flush_changes(context);
    }

    fn open_article(&mut self, context: &mut Context, index: usize) {
        let Some(article) = self.articles.get(index) else {
            return;
        };
        self.menu_open = None;
        self.open = Some(index);
        self.view = Some(View::Reading);
        self.book.close(context);
        self.illustrations.close(context);
        let id = kobo_net::sha256::hex_digest(
            format!("miniflux:{}:{}", self.server, article.id).as_bytes(),
        );
        let memory = self.progress.memory(&id);
        self.reading_id = Some(id);
        let body = article.content.clone();
        let origin = article.url.clone();
        let unread = self.shown_status(article) == Status::Unread;
        if body.trim().is_empty() {
            self.notice = Some(
                "This feed supplied a summary only. Choose Load full article to fetch the rest."
                    .into(),
            );
        }
        self.book
            .open(context, kobo_doc::html::parse(&body), memory);
        self.illustrations.open(context, &mut self.book, &origin);
        self.keep_position(context);
        // Opening an article is what marks it read, the way it does in every
        // feed reader. Doing it here rather than on the way out means an
        // article read on a train is already queued when the Wi-Fi returns.
        if unread {
            self.change(context, index, Change::Read);
        }
    }

    fn close_article(&mut self, context: &mut Context) {
        self.keep_position(context);
        self.illustrations.close(context);
        self.book.close(context);
        self.view = Some(View::Shelf);
        self.open = None;
        self.reading_id = None;
    }

    fn keep_position(&mut self, context: &mut Context) {
        if let (Some(id), Some(memory)) = (&self.reading_id, self.book.memory()) {
            self.progress.keep(context, id.clone(), memory.clone());
        }
    }

    fn persist_config(&self, context: &mut Context) {
        context
            .store()
            .save(CONFIG, format!("{}\n{}", self.server, self.credential));
    }

    fn settings_are_safe_to_change(&self) -> bool {
        !self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.busy() || snapshot.retryable())
            && self.changes.len() == 0
    }
}

impl KoboApp for Reader {
    fn on_start(&mut self, context: &mut Context) {
        self.credential = "miniflux".into();
        context.store().load(CONFIG);
        context.store().load(pending::KEY);
        context.store().load(READING);
        self.show(context);
    }

    fn on_load(&mut self, context: &mut Context, key: &str, result: StoreResult) {
        if self
            .illustrations
            .store(context, &mut self.book, key, &result, false)
        {
            self.show(context);
            return;
        }
        if key == READING {
            self.progress.load(result);
            self.show(context);
            return;
        }
        if key == pending::KEY {
            self.changes.load(&result);
            self.flush_changes(context);
            self.show(context);
            return;
        }
        if let Some(snapshot) = self.snapshot.as_mut().filter(|s| s.key == key) {
            let event = snapshot.stored(context, &result);
            self.snapshot_event(event);
            self.show(context);
            return;
        }
        self.on_store(context, result);
    }

    fn on_save(&mut self, context: &mut Context, key: &str, result: StoreResult) {
        if self
            .illustrations
            .store(context, &mut self.book, key, &result, false)
        {
            self.show(context);
            return;
        }
        if key == READING {
            self.progress.saved(context, result);
            self.show(context);
            return;
        }
        if key == pending::KEY {
            self.changes.stored(context, &result);
            self.show(context);
            return;
        }
        if let Some(snapshot) = self.snapshot.as_mut().filter(|s| s.key == key) {
            let event = snapshot.stored(context, &result);
            self.snapshot_event(event);
            self.show(context);
        }
    }

    fn on_shelf(&mut self, context: &mut Context, name: &str, result: StoreResult) {
        if self
            .illustrations
            .store(context, &mut self.book, name, &result, true)
        {
            self.show(context);
            return;
        }
        if let Some(snapshot) = self.snapshot.as_mut().filter(|s| s.owns_file(name)) {
            let event = snapshot.shelf(context, &result);
            self.snapshot_event(event);
            self.show(context);
        }
    }

    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        if let StoreResult::Loaded { key, value } = result {
            if key == CONFIG {
                if let Some(value) = &value {
                    let saved = String::from_utf8_lossy(value);
                    let mut values = saved.lines();
                    values
                        .next()
                        .unwrap_or_default()
                        .clone_into(&mut self.server);
                    values
                        .next()
                        .unwrap_or("miniflux")
                        .clone_into(&mut self.credential);
                }
                self.open_snapshot(context);
            }
            self.show(context);
        }
    }

    #[allow(clippy::too_many_lines)]
    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if let Some(setting) = self.editing {
            self.edit(context, setting, action);
            self.show(context);
            return;
        }
        // The open document answers its own page turns and controls first.
        if self.view == Some(View::Reading) && self.book.memory().is_some() {
            match self.book.act(context, action) {
                Some(kobo_read::Outcome::Close) => self.close_article(context),
                Some(kobo_read::Outcome::Save) => self.keep_position(context),
                Some(kobo_read::Outcome::Light(level)) => {
                    context.device().set_frontlight(level);
                    self.keep_position(context);
                }
                None if action == ActionId::BACK => self.close_article(context),
                _ => {}
            }
            self.show(context);
            return;
        }
        // An open menu takes Back before the view does, or the tap beside the
        // popover leaves the application instead of closing the menu.
        if action == ActionId::BACK && self.menu_open.is_some() {
            self.menu_open = None;
        } else if action == action_id("settings") {
            self.view = Some(View::Settings);
        } else if action == action_id("server") {
            self.keyboard = Keyboard::with_text(if self.server.is_empty() {
                "https://"
            } else {
                &self.server
            });
            self.editing = Some(Setting::Server);
        } else if action == action_id("credential") {
            self.keyboard = Keyboard::with_text(&self.credential);
            self.editing = Some(Setting::Credential);
        } else if action == action_id("directory") {
            self.page = 0;
            self.view = Some(View::Directory);
        } else if action == action_id("back") || action == ActionId::BACK {
            self.page = 0;
            self.view = Some(View::Shelf);
        } else if action == action_id("previous") {
            self.page = self.page.saturating_sub(1);
        } else if action == action_id("next") {
            let listed = self.listed();
            let rows = self.shelf_rows(context, &listed);
            let pages = self.shelf_pages(context, &rows, self.notice().as_deref());
            self.page = self
                .page
                .saturating_add(1)
                .min(pages.len().saturating_sub(1));
        } else if let Some(tab) = Tab::ORDER
            .into_iter()
            .find(|tab| action == action_id(tab.action()))
        {
            self.tab = tab;
            self.page = 0;
            self.menu_open = None;
        } else if action == action_id("retry-save") {
            self.retry(context);
        } else if action == action_id("sync") {
            self.notice = None;
            self.sync(context);
        } else if let Some(index) = indexed(action, "starter", STARTER_FEEDS.len()) {
            self.follow(context, index);
        } else if let Some(index) = indexed(action, "article-menu", self.articles.len()) {
            self.menu_open = Some(index);
        } else if let Some(index) = indexed(action, "article", self.articles.len()) {
            self.open_article(context, index);
        } else if let Some(index) = self.menu_open {
            self.menu_action(context, index, action);
        }
        self.show(context);
    }

    fn on_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
        if self
            .illustrations
            .task(context, &mut self.book, task, &outcome)
        {
            self.show(context);
            return;
        }
        if self.book.woke(context, task, &outcome) != kobo_bookview::Step::Elsewhere {
            if self.view == Some(View::Reading) {
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
        match awaiting {
            Awaiting::Sync(at) => self.synced(context, at, outcome),
            Awaiting::Change(id, change) => self.changed(context, id, change, &outcome),
            Awaiting::Full(id) => self.fetched(context, id, &outcome),
            Awaiting::Categories(index) => self.categorised(context, index, &outcome),
            Awaiting::Subscribe(index) => self.subscribed(index, &outcome),
        }
        self.show(context);
    }
}

impl Reader {
    fn edit(&mut self, context: &mut Context, setting: Setting, action: ActionId) {
        match self.keyboard.press(action) {
            Some(Pressed::Submitted) => {
                if !self.settings_are_safe_to_change() {
                    self.notice =
                        Some("Finish saving and syncing before changing accounts.".into());
                    self.editing = None;
                    self.view = Some(View::Shelf);
                    return;
                }
                if let Some((task, _)) = self.task.take() {
                    context.cancel(task);
                }
                let value = self.keyboard.take().trim().to_owned();
                if !value.is_empty() {
                    match setting {
                        Setting::Server => self.server = value,
                        Setting::Credential => self.credential = value,
                    }
                    self.articles.clear();
                    self.open = None;
                    self.snapshot = None;
                    self.open_snapshot(context);
                    self.persist_config(context);
                }
                self.editing = None;
            }
            Some(Pressed::Edited | Pressed::Shifted) => {}
            None => {
                if action == ActionId::BACK {
                    self.editing = None;
                }
            }
        }
    }

    fn menu_action(&mut self, context: &mut Context, index: usize, action: ActionId) {
        let choice = [
            ("star", Some(Change::Star)),
            ("unstar", Some(Change::Unstar)),
            ("mark-read", Some(Change::Read)),
            ("keep-unread", Some(Change::Unread)),
            ("archive", Some(Change::Archive)),
            ("full-article", None),
        ]
        .into_iter()
        .find(|(name, _)| action == action_id(name));
        let Some((_, change)) = choice else {
            return;
        };
        self.menu_open = None;
        match change {
            Some(change) => self.change(context, index, change),
            None => self.load_full_article(context, index),
        }
    }

    fn load_full_article(&mut self, context: &mut Context, index: usize) {
        let Some(article) = self.articles.get(index) else {
            return;
        };
        if !self.configured() {
            self.view = Some(View::Settings);
            return;
        }
        let id = article.id;
        let task = miniflux::full_content(&self.server, &self.credential, id);
        if self.spawn(context, task, Awaiting::Full(id)) {
            self.notice = Some("Asking Miniflux for the full article…".into());
        } else {
            self.notice = Some("Finish the request in progress, then try again.".into());
        }
    }

    fn retry(&mut self, context: &mut Context) {
        if let Some(snapshot) = &mut self.snapshot {
            snapshot.retry(context);
        }
        if self.changes.failed {
            self.changes.retry(context);
        }
        if self.progress.failed {
            self.progress.retry(context);
            self.keep_position(context);
        }
        if self.illustrations.can_retry() {
            self.illustrations.retry(context);
        }
    }

    fn follow(&mut self, context: &mut Context, index: usize) {
        if !self.configured() {
            self.view = Some(View::Settings);
            return;
        }
        let task = miniflux::categories(&self.server, &self.credential);
        if self.spawn(context, task, Awaiting::Categories(index)) {
            self.notice = Some(format!("Adding {} to Miniflux…", STARTER_FEEDS[index].0));
        }
    }

    /// Takes one of the three lists and asks for the next.
    ///
    /// A part that fails ends the sync rather than saving half a batch over a
    /// whole one: the saved copy a reader already has is worth more than a
    /// fresher copy missing its Starred tab.
    fn synced(&mut self, context: &mut Context, at: usize, outcome: TaskOutcome) {
        match outcome {
            TaskOutcome::Completed(bytes) => match miniflux::parse_entries(&bytes) {
                Ok(articles) => {
                    for article in articles {
                        if !self
                            .arriving
                            .iter()
                            .any(|held: &Article| held.id == article.id)
                        {
                            self.arriving.push(article);
                        }
                    }
                    self.fetch_part(context, at + 1);
                }
                Err(error) => {
                    self.arriving.clear();
                    self.notice = Some(error.message().into());
                }
            },
            TaskOutcome::Failed(_) => {
                self.arriving.clear();
                self.notice = Some(
                    "Could not sync. Your saved articles are still here; connect to Wi-Fi and retry."
                        .into(),
                );
            }
            TaskOutcome::Cancelled => {
                self.arriving.clear();
                self.notice = Some("Sync cancelled.".into());
            }
        }
    }

    /// Publishes the merged batch once all three lists have arrived.
    fn batch_arrived(&mut self, context: &mut Context) {
        self.articles = std::mem::take(&mut self.arriving);
        self.page = 0;
        self.notice = Some(match self.articles.len() {
            1 => "Synced 1 article.".to_owned(),
            count => format!("Synced {count} articles."),
        });
        self.keep_articles(context);
    }

    fn changed(&mut self, context: &mut Context, id: u64, change: Change, outcome: &TaskOutcome) {
        match outcome {
            TaskOutcome::Completed(_) => {
                self.changes.applied(context);
                self.unreachable = false;
                if let Some(article) = self.articles.iter_mut().find(|a| a.id == id) {
                    match change {
                        Change::Read => article.status = Status::Read,
                        Change::Unread => article.status = Status::Unread,
                        Change::Archive => article.status = Status::Removed,
                        Change::Star => article.starred = true,
                        Change::Unstar => article.starred = false,
                    }
                }
                self.keep_articles(context);
                self.notice = None;
                self.flush_changes(context);
            }
            TaskOutcome::Cancelled => self.changes.unresolved(),
            TaskOutcome::Failed(_) => {
                self.changes.unresolved();
                // Said by the count of waiting changes rather than here, so
                // the shelf carries one sentence about them instead of two.
                self.unreachable = true;
                self.notice = None;
            }
        }
    }

    fn fetched(&mut self, context: &mut Context, id: u64, outcome: &TaskOutcome) {
        let TaskOutcome::Completed(bytes) = outcome else {
            self.notice =
                Some("The full article could not be fetched. The summary is kept.".into());
            return;
        };
        match miniflux::parse_content(bytes) {
            Ok(body) if !body.trim().is_empty() => {
                let Some(index) = self.articles.iter().position(|a| a.id == id) else {
                    return;
                };
                self.articles[index].content = body;
                self.keep_articles(context);
                self.notice = Some("The full article is here.".into());
                if self.open == Some(index) {
                    self.open_article(context, index);
                }
            }
            Ok(_) => {
                self.notice =
                    Some("Miniflux fetched the page and found no article text in it.".into());
            }
            Err(error) => self.notice = Some(error.message().into()),
        }
    }

    fn categorised(&mut self, context: &mut Context, index: usize, outcome: &TaskOutcome) {
        let TaskOutcome::Completed(bytes) = outcome else {
            self.notice = Some("Could not reach Miniflux to add this feed.".into());
            return;
        };
        match miniflux::parse_categories(bytes) {
            Ok(categories) => {
                let Some((category, _)) = categories.first() else {
                    self.notice =
                        Some("Your Miniflux account has no category to file a feed under.".into());
                    return;
                };
                let task = miniflux::subscribe(
                    &self.server,
                    &self.credential,
                    STARTER_FEEDS[index].2,
                    *category,
                );
                self.spawn(context, task, Awaiting::Subscribe(index));
            }
            Err(error) => self.notice = Some(error.message().into()),
        }
    }

    fn subscribed(&mut self, index: usize, outcome: &TaskOutcome) {
        self.notice = Some(match outcome {
            TaskOutcome::Completed(_) => format!(
                "{} was added to your Miniflux account. Sync to read it.",
                STARTER_FEEDS[index].0
            ),
            _ => format!(
                "{} was not added. Check the feed in Miniflux before trying again.",
                STARTER_FEEDS[index].0
            ),
        });
    }
}

/// The index in `name-<index>`, when the action is one of those.
fn indexed(action: ActionId, name: &str, count: usize) -> Option<usize> {
    (0..count).find(|index| action == action_id(&format!("{name}-{index}")))
}

fn main() -> ExitCode {
    kobo_sdk::run("rss-miniflux", Reader::default()).map_or_else(
        |e| {
            eprintln!("rss-miniflux: {e}");
            ExitCode::FAILURE
        },
        |()| ExitCode::SUCCESS,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobo_sdk::{AppRunner, Command, StoreRequest, Task};
    use kobo_ui::{Chrome, CLARA_BW_METRICS};

    const SAVED_ARTICLE: &[u8] = br#"{"entries":[{"id":7,"title":"A saved article","content":"<h2>A heading</h2><p>The complete article body.</p>","starred":true,"status":"unread","url":"https://example.com/story","feed":{"title":"Journal"}}]}"#;
    const NOTHING: &[u8] = br#"{"entries":[]}"#;

    fn connected_with(queue: Option<Vec<u8>>) -> AppRunner<Reader> {
        let mut runner = AppRunner::new(Reader::default());
        runner.start();
        runner.store_result(StoreResult::Loaded {
            key: CONFIG.into(),
            value: Some(b"https://flux.example\nminiflux".to_vec()),
        });
        runner.store_result(StoreResult::Loaded {
            key: pending::KEY.into(),
            value: queue,
        });
        runner.store_result(StoreResult::Loaded {
            key: READING.into(),
            value: None,
        });
        runner
    }

    fn connected() -> AppRunner<Reader> {
        connected_with(None)
    }

    /// Answers the snapshot load with the empty store a new reader has.
    fn opened(runner: &mut AppRunner<Reader>) {
        let key = runner.app().snapshot.as_ref().unwrap().key.clone();
        runner.store_result(StoreResult::Loaded { key, value: None });
    }

    fn saved(commands: &[Command], key: &str) -> Option<Vec<u8>> {
        commands.iter().find_map(|command| match command {
            Command::Store(StoreRequest::Save { key: named, value }) if named == key => {
                Some(value.clone())
            }
            _ => None,
        })
    }

    fn sent(commands: &[Command]) -> Option<Task> {
        commands.iter().find_map(|command| match command {
            Command::Spawn { work, .. } => Some(work.clone()),
            _ => None,
        })
    }

    fn written(commands: &[Command]) -> Option<(String, Vec<u8>)> {
        commands.iter().find_map(|command| match command {
            Command::Store(StoreRequest::ShelfWrite { name, bytes, .. }) => {
                Some((name.clone(), bytes.clone()))
            }
            _ => None,
        })
    }

    /// Answers the outstanding request, whatever it is.
    fn answer(runner: &mut AppRunner<Reader>, body: &[u8]) -> Vec<Command> {
        let (task, _) = runner.app().task.expect("a request was made");
        runner.task_outcome(task, TaskOutcome::Completed(body.to_vec()))
    }

    /// Publishes whatever the application just wrote to the shelf.
    fn acknowledge(runner: &mut AppRunner<Reader>, commands: &[Command]) -> (String, Vec<u8>) {
        let (name, bytes) = written(commands).expect("the batch is written to the shelf");
        let key = runner.app().snapshot.as_ref().unwrap().key.clone();
        let published = runner.store_result(StoreResult::ShelfWritten {
            name: name.clone(),
            size: u32::try_from(bytes.len()).unwrap(),
        });
        let pointer = saved(&published, &key).expect("the pointer is published");
        runner.store_result(StoreResult::Saved { key });
        (name, pointer)
    }

    /// Drives a whole sync: the three lists, then the two acknowledgements
    /// that publish the saved batch. Answers with what a restart would find.
    fn synced(runner: &mut AppRunner<Reader>, unread: &[u8]) -> (String, Vec<u8>, Vec<u8>) {
        runner.action(action_id("sync"));
        answer(runner, unread);
        answer(runner, NOTHING);
        let writes = answer(runner, NOTHING);
        let (_, bytes) = written(&writes).expect("the merged batch is written");
        let (name, pointer) = acknowledge(runner, &writes);
        (name, pointer, bytes)
    }

    /// Reopens a saved batch the way a restart does: pointer, then file.
    fn reopened(runner: &mut AppRunner<Reader>, name: &str, pointer: Vec<u8>, body: &[u8]) {
        let key = runner.app().snapshot.as_ref().unwrap().key.clone();
        runner.store_result(StoreResult::Loaded {
            key,
            value: Some(pointer),
        });
        runner.store_result(StoreResult::ShelfRead {
            name: name.to_owned(),
            offset: 0,
            bytes: body.to_vec(),
            size: u32::try_from(body.len()).unwrap(),
        });
    }

    #[test]
    fn every_article_is_reachable_through_measured_pages_at_all_text_sizes() {
        for scale in kobo_ui::TextScale::STEPS {
            let mut metrics = CLARA_BW_METRICS;
            metrics.text_scale = scale;
            let article = miniflux::parse_entries(SAVED_ARTICLE).unwrap().remove(0);
            let articles = (0..100)
                .map(|i| Article {
                    id: i + 1,
                    title: format!(
                        "Article {i}: a long headline about the places and people along the river"
                    ),
                    starred: false,
                    ..article.clone()
                })
                .collect();
            let mut runner = AppRunner::with_metrics(
                Reader {
                    server: "https://flux.example".into(),
                    credential: "miniflux".into(),
                    articles,
                    notice: Some("Could not sync. Saved articles are still available.".into()),
                    ..Reader::default()
                },
                metrics,
            );
            let mut seen = std::collections::BTreeSet::new();
            let mut commands = runner.action(action_id("previous"));
            for _ in 0..101 {
                for command in &commands {
                    if let Command::SetScreen(screen) = command {
                        let diagnostics = screen.diagnostics(&metrics, &Chrome::default());
                        assert!(
                            !diagnostics
                                .issues
                                .iter()
                                .any(|issue| issue.severity == kobo_ui::DiagnosticSeverity::Error),
                            "{scale:?}: {:?}",
                            diagnostics.issues
                        );
                        let layout = screen.layout_with(&metrics, &Chrome::default());
                        for i in 0..100 {
                            if layout
                                .rect_of_action(action_id(&format!("article-{i}")))
                                .is_some()
                            {
                                seen.insert(i);
                            }
                        }
                    }
                }
                let page = runner.app().page;
                commands = runner.action(action_id("next"));
                if runner.app().page == page {
                    break;
                }
            }
            assert_eq!(seen.len(), 100, "{scale:?}: not all articles reached");
        }
    }

    #[test]
    fn a_sync_collects_the_three_lists_the_tabs_are_drawn_from() {
        let mut runner = connected();
        opened(&mut runner);
        let asking = runner.action(action_id("sync"));
        let Some(Task::Fetch { url, .. }) = sent(&asking) else {
            panic!("nothing was requested")
        };
        assert!(url.contains("status=unread"), "{url}");
        let asking = answer(&mut runner, SAVED_ARTICLE);
        let Some(Task::Fetch { url, .. }) = sent(&asking) else {
            panic!("the starred list was not requested")
        };
        assert!(url.contains("starred=true"), "{url}");
        let asking = answer(&mut runner, NOTHING);
        let Some(Task::Fetch { url, .. }) = sent(&asking) else {
            panic!("the read list was not requested")
        };
        assert!(url.contains("status=read"), "{url}");
        let writes = answer(&mut runner, br#"{"entries":[{"id":9,"title":"Read elsewhere","content":"<p>Body</p>","starred":false,"status":"read"}]}"#);
        let (_, bytes) = written(&writes).expect("the merged batch is written");
        let batch = miniflux::parse_entries(&bytes).unwrap();
        assert_eq!(batch.len(), 2, "the lists were not merged");
        assert!(runner.app().task.is_none(), "a fourth request was made");
        assert_eq!(
            runner.app().listed(),
            vec![0],
            "Unread showed a read article"
        );
        runner.action(action_id("history"));
        assert_eq!(runner.app().listed(), vec![1]);
    }

    #[test]
    fn the_saved_batch_reopens_after_a_restart_without_a_single_request() {
        let mut runner = connected();
        opened(&mut runner);
        let (name, pointer, bytes) = synced(&mut runner, SAVED_ARTICLE);

        let mut restarted = connected();
        let first = restarted.store_result(StoreResult::Loaded {
            key: restarted.app().snapshot.as_ref().unwrap().key.clone(),
            value: Some(pointer),
        });
        let second = restarted.store_result(StoreResult::ShelfRead {
            name,
            offset: 0,
            bytes: bytes.clone(),
            size: u32::try_from(bytes.len()).unwrap(),
        });
        assert!(!first
            .iter()
            .chain(&second)
            .any(|command| matches!(command, Command::Spawn { .. })));
        assert_eq!(restarted.app().articles, runner.app().articles);
        assert!(restarted.app().articles[0]
            .content
            .contains("complete article body"));
    }

    #[test]
    fn a_failed_save_keeps_the_articles_and_retry_does_not_download_them_again() {
        let mut runner = connected();
        let key = runner.app().snapshot.as_ref().unwrap().key.clone();
        opened(&mut runner);
        runner.action(action_id("sync"));
        answer(&mut runner, SAVED_ARTICLE);
        answer(&mut runner, NOTHING);
        answer(&mut runner, NOTHING);
        runner.store_result(StoreResult::Denied(kobo_sdk::StoreError::NoRoom));
        assert!(runner.app().snapshot.as_ref().unwrap().retryable());
        assert_eq!(runner.app().articles.len(), 1);
        let retry = runner.action(action_id("retry-save"));
        assert!(retry.iter().any(
            |c| matches!(c, Command::Store(StoreRequest::Load { key: loaded }) if loaded == &key)
        ));
        assert!(!retry.iter().any(|c| matches!(c, Command::Spawn { .. })));
    }

    #[test]
    fn a_malformed_answer_preserves_the_previous_articles_and_reports_why() {
        let mut runner = connected();
        opened(&mut runner);
        synced(&mut runner, SAVED_ARTICLE);
        let previous = runner.app().articles.clone();
        runner.action(action_id("sync"));
        answer(&mut runner, b"<html>Log in</html>");
        assert_eq!(runner.app().articles, previous);
        assert!(
            runner.app().task.is_none(),
            "the sync carried on regardless"
        );
        assert!(runner
            .app()
            .notice
            .as_deref()
            .unwrap()
            .contains("valid Miniflux response"));
    }

    #[test]
    fn a_part_that_fails_leaves_the_saved_batch_alone() {
        let mut runner = connected();
        opened(&mut runner);
        synced(&mut runner, SAVED_ARTICLE);
        let previous = runner.app().articles.clone();
        runner.action(action_id("sync"));
        answer(&mut runner, NOTHING);
        let (task, _) = runner.app().task.unwrap();
        let after = runner.task_outcome(task, TaskOutcome::Failed(kobo_sdk::TaskError::Offline));
        assert!(
            written(&after).is_none(),
            "half a sync was written over a whole one"
        );
        assert_eq!(runner.app().articles, previous);
    }

    #[test]
    fn opening_an_article_marks_it_read_once_and_moves_it_out_of_unread() {
        let mut runner = connected();
        opened(&mut runner);
        synced(&mut runner, SAVED_ARTICLE);
        let sending = runner.action(action_id("article-0"));
        assert_eq!(runner.app().view, Some(View::Reading));
        let Some(Task::Update { url, body, .. }) = sent(&sending) else {
            panic!("opening an article did not send a read mark")
        };
        assert!(url.ends_with("/v1/entries"));
        assert_eq!(body, r#"{"entry_ids":[7],"status":"read"}"#);
        assert_eq!(runner.app().changes.len(), 1);
        assert!(
            runner.app().listed().is_empty(),
            "a read article stayed in Unread"
        );
        runner.action(ActionId::BACK);
        assert_eq!(runner.app().view, Some(View::Shelf));
        runner.action(action_id("history"));
        assert_eq!(runner.app().listed(), vec![0]);

        let landed = answer(&mut runner, b"{}");
        assert_eq!(runner.app().changes.len(), 0);
        assert_eq!(runner.app().articles[0].status, Status::Read);
        assert!(
            written(&landed).is_some(),
            "the acknowledged change was not saved for offline reading"
        );
        let again = runner.action(action_id("article-0"));
        assert!(
            sent(&again).is_none(),
            "reopening queued a second read mark"
        );
    }

    #[test]
    fn a_star_made_offline_is_shown_and_still_queued_after_a_restart() {
        let mut runner = connected();
        opened(&mut runner);
        let (name, pointer, bytes) = synced(&mut runner, SAVED_ARTICLE);
        // This article arrives starred, so the reader takes the star off.
        runner.action(action_id("article-menu-0"));
        let queued = runner.action(action_id("unstar"));
        let written_queue = saved(&queued, pending::KEY).expect("the change is written down");
        let Some(Task::Update { body, .. }) = sent(&queued) else {
            panic!("the star was not sent")
        };
        assert_eq!(body, r#"{"entry_ids":[7],"starred":false}"#);
        assert!(!runner.app().shown_starred(&runner.app().articles[0]));
        assert!(
            runner.app().articles[0].starred,
            "the downloaded batch was rewritten before Miniflux answered"
        );

        let mut restarted = connected_with(Some(written_queue));
        reopened(&mut restarted, &name, pointer, &bytes);
        assert_eq!(
            restarted.app().changes.len(),
            1,
            "the unsent star did not survive the restart"
        );
        assert!(!restarted.app().shown_starred(&restarted.app().articles[0]));
        restarted.action(action_id("starred"));
        assert!(
            restarted.app().listed().is_empty(),
            "an unstarred article stayed in Starred"
        );
    }

    #[test]
    fn a_change_whose_reply_never_arrived_goes_out_again_on_the_next_sync() {
        let mut runner = connected();
        opened(&mut runner);
        synced(&mut runner, SAVED_ARTICLE);
        runner.action(action_id("article-menu-0"));
        runner.action(action_id("unstar"));
        let (task, _) = runner.app().task.expect("the star was sent");
        runner.task_outcome(task, TaskOutcome::Failed(kobo_sdk::TaskError::Unreachable));
        assert_eq!(runner.app().changes.len(), 1);
        assert!(runner
            .app()
            .notice()
            .unwrap()
            .contains("goes out on the next sync"));

        let again = runner.action(action_id("sync"));
        assert!(
            matches!(sent(&again), Some(Task::Update { body, .. })
                if body == r#"{"entry_ids":[7],"starred":false}"#),
            "the unsent change was not tried again before the sync"
        );
        answer(&mut runner, b"{}");
        assert_eq!(runner.app().changes.len(), 0);
    }

    #[test]
    fn a_full_article_replaces_the_summary_and_is_saved_for_offline_reading() {
        const SUMMARY: &[u8] = br#"{"entries":[{"id":7,"title":"A summary","content":"<p>Only the first line.</p>","starred":false,"status":"unread","url":"https://example.com/story","feed":{"title":"Journal"}}]}"#;
        let mut runner = connected();
        opened(&mut runner);
        synced(&mut runner, SUMMARY);
        runner.action(action_id("article-menu-0"));
        let asking = runner.action(action_id("full-article"));
        let Some(Task::Fetch { url, .. }) = sent(&asking) else {
            panic!("the full article was not requested")
        };
        assert!(url.ends_with("/v1/entries/7/fetch-content?update_content=false"));
        let writes = answer(
            &mut runner,
            br#"{"content":"<h2>All of it</h2><p>Every paragraph of the page.</p>"}"#,
        );
        assert!(runner.app().articles[0].content.contains("Every paragraph"));
        let (_, bytes) = written(&writes).expect("the upgraded article was not saved");
        let restored = miniflux::parse_entries(&bytes).expect("the saved batch reads back");
        assert!(restored[0].content.contains("Every paragraph"));
        assert_eq!(restored[0].title, "A summary");
    }

    #[test]
    fn the_account_cannot_be_changed_while_changes_are_still_unsent() {
        let mut runner = connected();
        opened(&mut runner);
        synced(&mut runner, SAVED_ARTICLE);
        runner.action(action_id("article-menu-0"));
        runner.action(action_id("unstar"));
        assert_eq!(runner.app().changes.len(), 1);
        runner.action(action_id("settings"));
        runner.action(action_id("server"));
        runner.app_mut().keyboard = Keyboard::with_text("https://other.example");
        let refused = runner.action(action_id("kb.enter"));
        assert_eq!(runner.app().server, "https://flux.example");
        assert!(saved(&refused, CONFIG).is_none(), "the account was changed");
        assert!(runner
            .app()
            .notice
            .as_deref()
            .unwrap()
            .contains("Finish saving and syncing"));
    }

    #[test]
    fn a_chosen_feed_is_filed_under_an_existing_category() {
        let mut runner = connected();
        opened(&mut runner);
        runner.action(action_id("directory"));
        let asking = runner.action(action_id("starter-0"));
        let Some(Task::Fetch { url, .. }) = sent(&asking) else {
            panic!("the categories were not read first")
        };
        assert!(url.ends_with("/v1/categories"));
        let following = answer(&mut runner, br#"[{"id":3,"title":"News"}]"#);
        let Some(Task::Post { url, body, .. }) = sent(&following) else {
            panic!("the feed was not added")
        };
        assert!(url.ends_with("/v1/feeds"));
        assert!(body.contains(STARTER_FEEDS[0].2));
        assert!(body.contains("\"category_id\":3"));
    }

    #[test]
    fn setup_actions_are_reachable_before_an_account_is_added() {
        let mut runner = AppRunner::new(Reader::default());
        let commands = runner.start();
        let Some(Command::SetScreen(screen)) = commands
            .iter()
            .rev()
            .find(|command| matches!(command, Command::SetScreen(_)))
            .cloned()
        else {
            panic!("nothing was drawn")
        };
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
        assert!(layout.rect_of_action(action_id("settings")).is_some());
        assert!(layout.rect_of_action(action_id("directory")).is_some());
    }
}

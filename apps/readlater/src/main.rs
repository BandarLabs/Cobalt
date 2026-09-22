mod cache;
mod session;
mod wallabag;

use kobo_sdk::snapshot::{Snapshot, SnapshotEvent};

use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Credential, DeviceRequest, DeviceResult, Glyph,
    KoboApp, ScreenBuilder, StoreResult, Task, TaskError, TaskId, TaskOutcome, UpdateMethod,
};
use std::process::ExitCode;
use wallabag::Entry;

const CONFIG: &str = "config";
const ACTIONS: &str = "actions";
/// The one credential name this application is policy-authorized to use.
const CREDENTIAL: &str = "wallabag";
const CACHE_ERROR: &str = "Articles could not be saved or opened. Retry saving before closing.";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum View {
    Queue,
    Article,
    Settings,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Setting {
    Server,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PendingTask {
    Queue,
    Article(usize),
    /// The outbox entry at index zero is in flight.
    Outbox,
    /// A token refresh; `resume` holds what to replay when it lands.
    Refresh,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Tab {
    #[default]
    Unread,
    Starred,
    Archive,
}

impl Tab {
    fn params(self) -> (bool, bool) {
        match self {
            Tab::Unread => (false, false),
            Tab::Starred => (true, false),
            Tab::Archive => (false, true),
        }
    }

    fn shows(self, entry: &Entry) -> bool {
        match self {
            Tab::Unread => !entry.archived,
            Tab::Starred => entry.starred && !entry.archived,
            Tab::Archive => entry.archived,
        }
    }

    fn index(self) -> usize {
        match self {
            Tab::Unread => 0,
            Tab::Starred => 1,
            Tab::Archive => 2,
        }
    }
}

/// One local change waiting for the server: archive, or star either way.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct OutboxAction {
    id: u64,
    kind: OutboxKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OutboxKind {
    Archive,
    Star,
    Unstar,
}

impl OutboxAction {
    fn encode(self) -> String {
        let tag = match self.kind {
            OutboxKind::Archive => "a",
            OutboxKind::Star => "s",
            OutboxKind::Unstar => "u",
        };
        format!("{tag}:{}", self.id)
    }

    fn decode(token: &str) -> Option<Self> {
        let (tag, id) = match token.split_once(':') {
            Some((tag, id)) => (tag, id),
            // The pre-outbox format stored bare ids, all archives.
            None => ("a", token),
        };
        let kind = match tag {
            "a" => OutboxKind::Archive,
            "s" => OutboxKind::Star,
            "u" => OutboxKind::Unstar,
            _ => return None,
        };
        Some(Self {
            id: id.parse().ok()?,
            kind,
        })
    }

    fn body(self) -> String {
        match self.kind {
            OutboxKind::Archive => wallabag::archive_body(true),
            OutboxKind::Star => wallabag::star_body(true),
            OutboxKind::Unstar => wallabag::star_body(false),
        }
    }
}

#[derive(Default)]
struct ReadLater {
    snapshot: Option<Snapshot>,
    cache_dirty: bool,
    server: String,
    session: Option<session::Session>,
    depth: u16,
    tab: Tab,
    entries: Vec<Entry>,
    entries_origin: Option<String>,
    task_origin: Option<String>,
    open: Option<usize>,
    view: Option<View>,
    pending: Vec<OutboxAction>,
    task: Option<(TaskId, PendingTask)>,
    resume: Option<PendingTask>,
    refreshing: bool,
    /// Consecutive refreshes without one succeeding request; caps the
    /// refresh-refused-refresh loop at a single renewal.
    refresh_attempts: u8,
    notice: Option<String>,
    keyboard: Keyboard,
    editing: Option<Setting>,
}

impl ReadLater {
    fn open_cache(&mut self, context: &mut Context) {
        if !self.ready() {
            return;
        }
        let snapshot = Snapshot::new(&format!("readlater-v1:{}\n{CREDENTIAL}", self.server()))
            .at_most(cache::LIMIT);
        snapshot.start(context);
        self.snapshot = Some(snapshot);
        self.cache_dirty = false;
    }

    fn keep_articles(&mut self, context: &mut Context) {
        self.cache_dirty = true;
        self.flush_cache(context);
    }

    fn flush_cache(&mut self, context: &mut Context) {
        if !self.cache_dirty {
            return;
        }
        let Some(snapshot) = &mut self.snapshot else {
            return;
        };
        if snapshot.busy() {
            return;
        }
        let Some(bytes) = cache::encode(&self.entries) else {
            self.notice = Some("These articles exceed the offline storage limit. Keep fewer articles and sync again.".into());
            return;
        };
        if snapshot.save(context, bytes) {
            self.cache_dirty = false;
        }
    }

    fn cache_event(&mut self, context: &mut Context, event: Option<SnapshotEvent>) {
        match event {
            Some(SnapshotEvent::Loaded) => {
                if let Some(bytes) = self
                    .snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.bytes.as_deref())
                {
                    if let Some(mut entries) = cache::decode(bytes) {
                        if self.cache_dirty {
                            for entry in &mut self.entries {
                                if entry.content.is_empty() {
                                    if let Some(saved) =
                                        entries.iter_mut().find(|saved| saved.id == entry.id)
                                    {
                                        entry.content = std::mem::take(&mut saved.content);
                                        entry.position = saved.position;
                                    }
                                }
                            }
                        } else {
                            self.entries = entries;
                            self.entries_origin = Some(self.server());
                        }
                    } else {
                        self.notice = Some(
                            "Saved articles could not be opened. Sync to download them again."
                                .into(),
                        );
                    }
                }
            }
            Some(SnapshotEvent::Failed) => {
                self.notice = Some(CACHE_ERROR.into());
            }
            Some(SnapshotEvent::Saved) if self.notice.as_deref() == Some(CACHE_ERROR) => {
                self.notice = None;
            }
            _ => {}
        }
        self.flush_cache(context);
        self.show(context);
    }

    fn ready(&self) -> bool {
        self.session.is_some() || self.server.starts_with("https://")
    }

    /// The server every request and cache is scoped to: the signed-in account's
    /// own, or the address typed in settings when no session is installed.
    fn server(&self) -> String {
        self.session
            .as_ref()
            .map_or_else(|| self.server.clone(), |session| session.server.clone())
    }
    fn show(&self, context: &mut Context) {
        let view = self.view.unwrap_or(View::Queue);
        if let Some(setting) = self.editing {
            let prompt = match setting {
                Setting::Server => "Wallabag HTTPS server",
            };
            context.set_screen(
                ScreenBuilder::new("readlater")
                    .top_bar("Read Later settings")
                    .typed(&self.keyboard, prompt)
                    .keyboard(&self.keyboard, "Save")
                    .build(),
            );
            return;
        }
        let screen = match view {
            View::Queue => self.queue_screen(),
            View::Article => {
                let entry = self.open.and_then(|i| self.entries.get(i));
                let loading = matches!(self.task, Some((_, PendingTask::Article(_))));
                match entry {
                    Some(e) if !e.content.is_empty() => article_screen(context, e),
                    Some(e) if loading => ScreenBuilder::new("readlater").top_bar("Read Later").heading(&e.title).secondary(&e.site).text("Loading article…").button("back", "Back").build(),
                    Some(e) => ScreenBuilder::new("readlater").top_bar("Read Later").heading(&e.title).text("Wallabag couldn't extract this one. Open the original URL in Wallabag.").action_bar([("archive", "Archive"), ("back", "Back")]).build(),
                    None => ScreenBuilder::new("readlater").top_bar("Read Later").splash(Some(Glyph::Bookmark), "Choose an article", "Open one from your reading list.").build(),
                }
            }
            View::Settings => {
                let page = ScreenBuilder::new("readlater").top_bar("Read Later settings");
                let page = if let Some(session) = &self.session {
                    page.secondary(format!(
                        "Signed in to {}. Run `kobo readlater login` on your computer to switch accounts.",
                        session.server
                    ))
                } else {
                    page.field("server", &self.server, "https://wallabag.example")
                        .secondary("Finish Wallabag setup on your computer.")
                };
                page.choose(
                    "Articles to keep",
                    [
                        ("depth-20", "20 newest"),
                        ("depth-50", "50 newest"),
                        ("depth-100", "100 newest"),
                    ],
                )
                .chosen(match self.depth {
                    100 => 2,
                    50 => 1,
                    _ => 0,
                })
                .button("back", "Back")
                .build()
            }
        };
        context.set_screen(screen);
    }
    fn queue_screen(&self) -> kobo_sdk::Screen {
        if !self.ready() {
            return ScreenBuilder::new("readlater")
                .top_bar("Read Later")
                .splash(
                    Some(Glyph::Bookmark),
                    "Connect Wallabag",
                    "On your computer run `kobo readlater login`, or install a credential named wallabag and add the HTTPS address here.",
                )
                .primary_button("settings", "Add address")
                .build();
        }
        let mut page = ScreenBuilder::new("readlater")
            .top_bar("Read Later")
            .top_bar_action("sync", "Sync")
            .top_bar_glyph("settings", "Settings", Glyph::Settings)
            .tabs(
                self.tab.index(),
                [
                    ("unread", "Unread"),
                    ("starred", "Starred"),
                    ("archive-tab", "Archive"),
                ],
            );
        if let Some(note) = &self.notice {
            page = page.banner(BannerLevel::Attention, note);
        }
        if self.snapshot.as_ref().is_some_and(Snapshot::retryable) || self.cache_dirty {
            page = page.button("retry-save", "Retry saving");
        }
        let visible: Vec<(usize, &Entry)> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| self.tab.shows(entry))
            .collect();
        if visible.is_empty() {
            page.splash(
                Some(Glyph::Bookmark),
                match self.tab {
                    Tab::Unread => "No saved articles",
                    Tab::Starred => "No starred articles",
                    Tab::Archive => "No archived articles",
                },
                "Sync Wallabag to add some.",
            )
            .build()
        } else {
            page.rows(visible.into_iter().map(|(i, e)| {
                (
                    format!("entry-{i}"),
                    e.title.clone(),
                    format!("{} · {} min", e.site, e.reading_time),
                    if e.starred {
                        Glyph::Heart
                    } else {
                        Glyph::Bookmark
                    },
                )
            }))
            .secondary(format!(
                "{} action{} pending sync",
                self.pending.len(),
                if self.pending.len() == 1 { "" } else { "s" }
            ))
            .build()
        }
    }

    fn sync(&mut self, context: &mut Context) {
        if !self.ready() {
            self.view = Some(View::Settings);
            return;
        }
        if self.task.is_some() {
            return;
        }
        // Local changes reach Wallabag before the list is read back, so an
        // archived article is gone from the fresh list rather than reappearing.
        if !self.pending.is_empty() {
            self.post_outbox(context);
            return;
        }
        self.fetch_queue(context);
    }

    fn switch_tab(&mut self, context: &mut Context, tab: Tab) {
        if self.tab == tab {
            return;
        }
        self.tab = tab;
        self.open = None;
        self.view = Some(View::Queue);
        self.sync(context);
    }

    fn fetch_queue(&mut self, context: &mut Context) {
        self.notice = Some("Syncing articles…".to_owned());
        let (starred, archived) = self.tab.params();
        if let Some(id) = context.spawn_retrying(Task::Fetch {
            url: wallabag::queue_url(&self.server(), self.depth.max(20), starred, archived),
            offset: 0,
            max_bytes: 512 * 1024,
            credential: Some(Credential::bearer(CREDENTIAL)),
            headers: Vec::new(),
        }) {
            self.task = Some((id, PendingTask::Queue));
            self.task_origin = Some(self.server());
        }
    }

    /// The server list is the truth for the current tab; bodies and reading
    /// places carry over only within one library, never across servers by id.
    fn queue_completed(&mut self, context: &mut Context, origin: &str, bytes: &[u8]) {
        let Some(mut entries) = wallabag::parse_entries(bytes) else {
            self.notice =
                Some("Couldn't load the reading list. Current articles are unchanged.".into());
            return;
        };
        if self.entries_origin.as_deref() == Some(origin) {
            for entry in &mut entries {
                if let Some(previous) = self.entries.iter_mut().find(|old| old.id == entry.id) {
                    entry.position = previous.position;
                    if entry.content.is_empty() {
                        entry.content = std::mem::take(&mut previous.content);
                    }
                }
            }
        }
        let tab = self.tab;
        self.entries
            .retain(|entry| !tab.shows(entry) || entries.iter().any(|e| e.id == entry.id));
        for entry in entries {
            if let Some(slot) = self.entries.iter_mut().find(|old| old.id == entry.id) {
                *slot = entry;
            } else {
                self.entries.push(entry);
            }
        }
        self.entries_origin = Some(origin.to_owned());
        self.notice = Some("Reading list synced.".to_owned());
        self.keep_articles(context);
    }

    fn post_outbox(&mut self, context: &mut Context) {
        let Some(action) = self.pending.first().copied() else {
            return;
        };
        self.notice = Some(format!(
            "Syncing {} change{}…",
            self.pending.len(),
            if self.pending.len() == 1 { "" } else { "s" }
        ));
        // Wallabag applies entry flags by PATCH; POST is refused there. The
        // answer echoes the whole entry, body included, so the ceiling is the
        // fetch ceiling rather than a flag's size.
        if let Some(id) = context.spawn_retrying(Task::Update {
            method: UpdateMethod::Patch,
            url: wallabag::entry_url(&self.server(), action.id),
            body: action.body(),
            content_type: "application/json".to_owned(),
            credential: Some(Credential::bearer(CREDENTIAL)),
            headers: Vec::new(),
            max_bytes: 512 * 1024,
        }) {
            self.task = Some((id, PendingTask::Outbox));
            self.task_origin = Some(self.server());
        }
    }

    /// Archive is optimistic: the article leaves the list now and the server
    /// hears about it from the outbox on the next sync.
    fn archive_open(&mut self, context: &mut Context) {
        let Some(index) = self.open else { return };
        let Some(entry) = self.entries.get_mut(index) else {
            return;
        };
        entry.archived = true;
        self.pending.push(OutboxAction {
            id: entry.id,
            kind: OutboxKind::Archive,
        });
        self.persist_actions(context);
        self.keep_articles(context);
        self.notice = Some("Archived; sending on the next sync.".to_owned());
        self.view = Some(View::Queue);
    }

    fn star_open(&mut self, context: &mut Context) {
        let Some(index) = self.open else { return };
        let Some(entry) = self.entries.get_mut(index) else {
            return;
        };
        entry.starred = !entry.starred;
        let action = OutboxAction {
            id: entry.id,
            kind: if entry.starred {
                OutboxKind::Star
            } else {
                OutboxKind::Unstar
            },
        };
        self.pending.push(action);
        self.persist_actions(context);
        self.keep_articles(context);
        self.notice = Some(if action.kind == OutboxKind::Star {
            "Starred; sending on the next sync.".to_owned()
        } else {
            "Unstarred; sending on the next sync.".to_owned()
        });
    }

    /// An expired token is not the end of a sync: refresh once, install the
    /// replacement through the runtime, and replay what was refused.
    fn refresh(&mut self, context: &mut Context, resume: PendingTask) {
        let Some(session) = &self.session else {
            return;
        };
        if self.refreshing || self.refresh_attempts > 0 {
            return;
        }
        self.refreshing = true;
        self.refresh_attempts += 1;
        self.resume = Some(resume);
        self.notice = Some("Refreshing the Wallabag sign-in…".to_owned());
        if let Some(id) = context.spawn_retrying(Task::Post {
            url: session.token_url(),
            body: session.refresh_body(),
            content_type: "application/x-www-form-urlencoded".to_owned(),
            credential: None,
            headers: Vec::new(),
            max_bytes: 16 * 1024,
        }) {
            self.task = Some((id, PendingTask::Refresh));
            self.task_origin = None;
        }
    }
    fn open_article(&mut self, context: &mut Context, index: usize) {
        self.open = Some(index);
        self.view = Some(View::Article);
        let Some(entry) = self.entries.get(index) else {
            return;
        };
        if !entry.content.is_empty() || !self.ready() {
            return;
        }
        if let Some(id) = context.spawn_retrying(Task::Fetch {
            url: wallabag::entry_url(&self.server(), entry.id),
            offset: 0,
            max_bytes: 512 * 1024,
            credential: Some(Credential::bearer(CREDENTIAL)),
            headers: Vec::new(),
        }) {
            self.task = Some((id, PendingTask::Article(index)));
            self.task_origin = Some(self.server());
        }
    }
    fn persist_actions(&self, context: &mut Context) {
        context.store().save(
            ACTIONS,
            self.pending
                .iter()
                .map(|action| action.encode())
                .collect::<Vec<_>>()
                .join(","),
        );
    }
    fn persist_config(&self, context: &mut Context) {
        context
            .store()
            .save(CONFIG, format!("{}\n{CREDENTIAL}", self.server));
    }
}

impl KoboApp for ReadLater {
    fn on_start(&mut self, context: &mut Context) {
        self.depth = 50;
        context.store().load(CONFIG);
        context.store().load(ACTIONS);
        context.store().load(session::STORE_KEY);
        context.shelf().read(
            session::SHELF_FILE,
            0,
            u32::try_from(session::LIMIT).unwrap_or(u32::MAX),
        );
        self.show(context);
    }
    fn on_load(&mut self, context: &mut Context, key: &str, result: StoreResult) {
        if let Some(snapshot) = self
            .snapshot
            .as_mut()
            .filter(|snapshot| snapshot.key == key)
        {
            let event = snapshot.stored(context, &result);
            self.cache_event(context, event);
        } else {
            self.on_store(context, result);
        }
    }

    fn on_save(&mut self, context: &mut Context, key: &str, result: StoreResult) {
        if let Some(snapshot) = self
            .snapshot
            .as_mut()
            .filter(|snapshot| snapshot.key == key)
        {
            let event = snapshot.stored(context, &result);
            self.cache_event(context, event);
        }
    }

    fn on_shelf(&mut self, context: &mut Context, name: &str, result: StoreResult) {
        if name == session::SHELF_FILE {
            if let StoreResult::ShelfRead { bytes, .. } = result {
                match session::decode_import(&bytes) {
                    Some(import) => {
                        let server = import.session.server.clone();
                        let access = import.access_token.clone();
                        self.session = Some(import.session);
                        context.store().save(
                            session::STORE_KEY,
                            session::encode(self.session.as_ref().unwrap()),
                        );
                        context.secrets().set_server(CREDENTIAL, &server, access);
                        context.shelf().remove(session::SHELF_FILE);
                        self.entries_origin = None;
                        self.open_cache(context);
                        self.notice = Some(format!("Signed in to {server}."));
                    }
                    None => {
                        self.notice = Some(
                            "The session file could not be read. Run `kobo readlater login` again."
                                .into(),
                        );
                    }
                }
            }
        } else if let Some(snapshot) = self
            .snapshot
            .as_mut()
            .filter(|snapshot| snapshot.owns_file(name))
        {
            let event = snapshot.shelf(context, &result);
            self.cache_event(context, event);
        }
        self.show(context);
    }

    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        if let StoreResult::Loaded { key, value } = result {
            if key == CONFIG {
                if let Some(value) = &value {
                    let text = String::from_utf8_lossy(value);
                    // The second line named a credential; the one authorized
                    // name is fixed now, so only the server is read back.
                    self.server
                        .clone_from(&text.lines().next().unwrap_or_default().to_owned());
                }
            }
            if key == session::STORE_KEY {
                if let Some(value) = &value {
                    self.session = session::decode(value);
                }
                // CONFIG and the session load concurrently; whichever arrives
                // last opens the cache.
                if self.snapshot.is_none() {
                    self.open_cache(context);
                }
            }
            if key == CONFIG {
                self.open_cache(context);
            }
            if key == ACTIONS {
                self.pending = value
                    .as_ref()
                    .map(|v| {
                        String::from_utf8_lossy(v)
                            .split(',')
                            .filter(|token| !token.is_empty())
                            .filter_map(OutboxAction::decode)
                            .collect()
                    })
                    .unwrap_or_default();
            }
            self.show(context);
        }
    }
    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if let Some(setting) = self.editing {
            match self.keyboard.press(action) {
                Some(Pressed::Submitted) => {
                    let value = self.keyboard.take().trim().to_owned();
                    if !value.is_empty() {
                        if self
                            .snapshot
                            .as_ref()
                            .is_some_and(|snapshot| snapshot.busy() || snapshot.retryable())
                        {
                            self.notice = Some("Finish saving articles before changing the server. Retry saving if needed.".into());
                            self.editing = None;
                            self.view = Some(View::Queue);
                            self.show(context);
                            return;
                        }
                        match setting {
                            Setting::Server => self.server = value,
                        }
                        self.persist_config(context);
                        self.entries.clear();
                        self.entries_origin = None;
                        self.open_cache(context);
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
        } else if action == action_id("settings") {
            self.view = Some(View::Settings);
        } else if action == action_id("server") {
            self.keyboard = Keyboard::with_text(if self.server.is_empty() {
                "https://"
            } else {
                &self.server
            });
            self.editing = Some(Setting::Server);
        } else if action == action_id("retry-save") {
            if let Some(snapshot) = &mut self.snapshot {
                snapshot.retry(context);
            }
            self.flush_cache(context);
        } else if action == action_id("sync") {
            self.sync(context);
        } else if action == action_id("unread") {
            self.switch_tab(context, Tab::Unread);
        } else if action == action_id("starred") {
            self.switch_tab(context, Tab::Starred);
        } else if action == action_id("archive-tab") {
            self.switch_tab(context, Tab::Archive);
        } else if action == action_id("back") || action == ActionId::BACK {
            self.view = Some(View::Queue);
        } else if action == action_id("depth-20") {
            self.depth = 20;
        } else if action == action_id("depth-50") {
            self.depth = 50;
        } else if action == action_id("depth-100") {
            self.depth = 100;
        } else if self.view == Some(View::Article)
            && (action == action_id("previous-page") || action == action_id("next-page"))
        {
            if let Some(entry) = self.open.and_then(|index| self.entries.get_mut(index)) {
                let pages = context.paginate_reading(&entry.content, true);
                let current = article_page(&pages, entry.position);
                let next = if action == action_id("next-page") {
                    (current + 1).min(pages.len().saturating_sub(1))
                } else {
                    current.saturating_sub(1)
                };
                if next != current {
                    entry.position = pages.iter().take(next).map(|page| page_words(page)).sum();
                    self.keep_articles(context);
                }
            }
        } else if action == action_id("archive") {
            self.archive_open(context);
        } else if action == action_id("star") {
            self.star_open(context);
        } else if let Some(index) =
            (0..self.entries.len()).find(|i| action == action_id(&format!("entry-{i}")))
        {
            self.open_article(context, index);
        }
        self.show(context);
    }
    fn on_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
        let Some((id, kind)) = self.task else {
            return;
        };
        if id != task {
            return;
        }
        self.task = None;
        let origin = self.server();
        if kind == PendingTask::Refresh {
            self.task_origin = None;
        } else if self
            .task_origin
            .take()
            .is_some_and(|requested| requested != origin)
        {
            return;
        }
        match (kind, outcome) {
            (PendingTask::Queue, TaskOutcome::Completed(bytes)) => {
                self.refreshing = false;
                self.refresh_attempts = 0;
                self.queue_completed(context, &origin, &bytes);
            }
            (PendingTask::Article(index), TaskOutcome::Completed(bytes)) => {
                self.refreshing = false;
                self.refresh_attempts = 0;
                if let Some(entry) = wallabag::parse_entry_document(&bytes) {
                    if let Some(slot) = self.entries.get_mut(index) {
                        if slot.id == entry.id {
                            let position = slot.position;
                            let (starred, archived) = (slot.starred, slot.archived);
                            *slot = entry;
                            slot.position = position;
                            slot.starred = starred;
                            slot.archived = archived;
                            self.keep_articles(context);
                        }
                    }
                }
            }
            (PendingTask::Outbox, TaskOutcome::Completed(_)) => {
                self.refreshing = false;
                self.refresh_attempts = 0;
                if !self.pending.is_empty() {
                    self.pending.remove(0);
                    self.persist_actions(context);
                }
                // The rest of the outbox goes first; a fresh list follows it.
                self.sync(context);
            }
            (PendingTask::Refresh, TaskOutcome::Completed(bytes)) => {
                if let Some((access, rolled)) = session::parse_token(&bytes) {
                    let server = self.server();
                    if let Some(session) = &mut self.session {
                        if let Some(rolled) = rolled {
                            session.refresh_token = rolled;
                        }
                        let stored = session::encode(session);
                        context.store().save(session::STORE_KEY, stored);
                    }
                    context.secrets().set_server(CREDENTIAL, server, access);
                    // The refused request replays when the runtime
                    // acknowledges the replacement token.
                } else {
                    self.resume = None;
                    self.notice = Some(
                        "The Wallabag sign-in could not be renewed. Run `kobo readlater login` again."
                            .into(),
                    );
                }
            }
            (PendingTask::Refresh, TaskOutcome::Failed(_) | TaskOutcome::Cancelled) => {
                self.resume = None;
                self.notice = Some(
                    "The Wallabag sign-in could not be renewed. Run `kobo readlater login` again."
                        .into(),
                );
            }
            (kind, TaskOutcome::Failed(TaskError::Unauthorized)) => {
                // An expired token: refresh once and replay, unless the
                // refusal came from a request made after a refresh.
                if self.session.is_some() && !self.refreshing {
                    self.refresh(context, kind);
                } else {
                    self.notice = Some(
                        "Wallabag refused the installed credential. Run `kobo readlater login` again."
                            .into(),
                    );
                }
            }
            (_, TaskOutcome::Failed(_)) => {
                self.notice = Some(if self.entries.is_empty() {
                    "No network. Join Wi-Fi, then sync.".to_owned()
                } else {
                    "Off the air. Saved articles stay readable. Join Wi-Fi to sync.".to_owned()
                });
            }
            (_, TaskOutcome::Cancelled) => self.notice = Some("Sync cancelled.".to_owned()),
        }
        self.show(context);
    }

    fn on_device_result(
        &mut self,
        context: &mut Context,
        request: DeviceRequest,
        result: DeviceResult,
    ) {
        if matches!(
            request,
            DeviceRequest::SetServerSecret { ref name, .. } if name == CREDENTIAL
        ) {
            if result == DeviceResult::Done {
                self.refreshing = false;
                if let Some(resume) = self.resume.take() {
                    match resume {
                        PendingTask::Queue => self.fetch_queue(context),
                        PendingTask::Article(index) => {
                            self.open_article(context, index);
                        }
                        PendingTask::Outbox => self.post_outbox(context),
                        PendingTask::Refresh => {}
                    }
                }
            } else {
                self.resume = None;
                self.notice =
                    Some("The refreshed sign-in could not be installed on this reader.".into());
            }
            self.show(context);
        }
    }
}

/// One page of an article, with the place it was left on.
///
/// The whole article used to be handed to the renderer as a single block, so
/// anything longer than the panel ran off the bottom of it and the rest was
/// unreachable. It is paginated against the panel it is drawn on, and the page
/// count in the bar is what tells a reader there is more.
fn article_screen(context: &mut Context, entry: &Entry) -> kobo_sdk::Screen {
    let pages = context.paginate_reading(&entry.content, true);
    let index = article_page(&pages, entry.position);
    let mut page = ScreenBuilder::new("readlater")
        .top_bar(&entry.title)
        .top_bar_action(
            "archive",
            if entry.archived {
                "Archived"
            } else {
                "Archive"
            },
        )
        .top_bar_glyph(
            "star",
            if entry.starred { "Starred" } else { "Star" },
            Glyph::Heart,
        )
        .reading(true)
        .page_position(
            u16::try_from(index + 1).unwrap_or(u16::MAX),
            u16::try_from(pages.len()).unwrap_or(u16::MAX),
        )
        .action_bar([
            ("previous-page", "Previous"),
            ("back", "Reading list"),
            ("next-page", "Next"),
        ]);
    if let Some(paragraphs) = pages.get(index) {
        for paragraph in paragraphs {
            page = page.text(paragraph);
        }
    }
    page.build()
}

fn page_words(page: &[String]) -> usize {
    page.iter()
        .map(|paragraph| paragraph.split_whitespace().count())
        .sum()
}

// A word offset survives display-size changes; old snapshots start at the beginning.
fn article_page(pages: &[Vec<String>], position: usize) -> usize {
    let mut end = 0;
    for (index, page) in pages.iter().enumerate() {
        end += page_words(page);
        if position < end {
            return index;
        }
    }
    pages.len().saturating_sub(1)
}

fn main() -> ExitCode {
    kobo_sdk::run("readlater", ReadLater::default()).map_or_else(
        |error| {
            eprintln!("readlater: {error}");
            ExitCode::FAILURE
        },
        |()| ExitCode::SUCCESS,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobo_ui::{Chrome, CLARA_BW_METRICS};
    #[test]
    fn long_articles_page_without_losing_text_and_resume_after_reflow() {
        use kobo_sdk::{AppRunner, Command};
        for scale in [kobo_ui::TextScale::Default, kobo_ui::TextScale::Largest] {
            let metrics = kobo_sdk::DisplayMetrics {
                text_scale: scale,
                ..CLARA_BW_METRICS
            };
            let mut context = AppRunner::with_metrics(ReadLater::default(), metrics).context();
            let article = (0..60).map(|n| format!("Paragraph {n}. Walking beside the river, we watched the light change on the water. A narrow path followed the bank beneath the trees.")).collect::<Vec<_>>().join("\n\n");
            let mut app = ReadLater {
                view: Some(View::Article),
                open: Some(0),
                entries: vec![Entry {
                    id: 7,
                    title: "Walking beside the river".into(),
                    site: "example.org".into(),
                    reading_time: 8,
                    content: article.clone(),
                    position: 0,
                    starred: false,
                    archived: false,
                }],
                ..ReadLater::default()
            };
            let pages = context.paginate_reading(&article, true);
            assert!(pages.len() > 3);
            assert_eq!(
                pages
                    .iter()
                    .flatten()
                    .flat_map(|p| p.split_whitespace())
                    .collect::<Vec<_>>(),
                article.split_whitespace().collect::<Vec<_>>()
            );
            app.on_action(&mut context, action_id("next-page"));
            assert_eq!(app.entries[0].position, page_words(&pages[0]));
            let saved = cache::encode(&app.entries).unwrap();
            app.entries = cache::decode(&saved).unwrap();
            assert_eq!(article_page(&pages, app.entries[0].position), 1);
            app.show(&mut context);
            let screen = context
                .commands()
                .iter()
                .rev()
                .find_map(|c| match c {
                    Command::SetScreen(s) => Some(s),
                    _ => None,
                })
                .unwrap();
            assert!(screen
                .diagnostics(&metrics, &Chrome::measuring(true))
                .issues
                .is_empty());
            if scale == kobo_ui::TextScale::Default {
                capture_queue(&app, "article-page.png");
            }
            app.on_action(&mut context, action_id("previous-page"));
            assert_eq!(app.entries[0].position, 0);
            app.on_action(&mut context, action_id("previous-page"));
            assert_eq!(app.entries[0].position, 0);
            let changed = Context::default().paginate_reading(&article, true);
            let offset = page_words(&pages[0]);
            let target = article_page(&changed, offset);
            let start: usize = changed.iter().take(target).map(|p| page_words(p)).sum();
            assert!(start <= offset && start + page_words(&changed[target]) > offset);
        }
    }

    #[test]
    fn acknowledged_article_snapshot_restores_in_a_fresh_app() {
        use kobo_sdk::{Command, StoreRequest};
        let mut app = ReadLater {
            server: "https://bag.example".into(),
            ..ReadLater::default()
        };
        let mut context = Context::default();
        app.open_cache(&mut context);
        let key = app.snapshot.as_ref().unwrap().key.clone();
        app.on_load(
            &mut context,
            &key,
            StoreResult::Loaded {
                key: key.clone(),
                value: None,
            },
        );
        let _ = context.take_commands();
        app.entries = vec![Entry {
            id: 7,
            title: "An article".into(),
            site: "example.org".into(),
            reading_time: 2,
            position: 0,
            content: "First paragraph.\n\nUse <section> literally.".into(),
            starred: false,
            archived: false,
        }];
        app.keep_articles(&mut context);
        let (name, bytes) = context
            .take_commands()
            .into_iter()
            .find_map(|command| match command {
                Command::Store(StoreRequest::ShelfWrite {
                    name,
                    bytes,
                    last: true,
                    ..
                }) => Some((name, bytes)),
                _ => None,
            })
            .unwrap();
        assert!(app.snapshot.as_ref().unwrap().bytes.is_none());
        app.on_shelf(
            &mut context,
            &name,
            StoreResult::ShelfWritten {
                name: name.clone(),
                size: u32::try_from(bytes.len()).unwrap(),
            },
        );
        let pointer = context
            .take_commands()
            .into_iter()
            .find_map(|command| match command {
                Command::Store(StoreRequest::Save { value, .. }) => Some(value),
                _ => None,
            })
            .unwrap();
        assert!(app.snapshot.as_ref().unwrap().bytes.is_none());
        app.on_save(&mut context, &key, StoreResult::Saved { key: key.clone() });
        assert!(app.snapshot.as_ref().unwrap().bytes.is_some());
        let mut reopened = ReadLater {
            server: app.server.clone(),
            ..ReadLater::default()
        };
        reopened.open_cache(&mut context);
        reopened.on_load(
            &mut context,
            &key,
            StoreResult::Loaded {
                key: key.clone(),
                value: Some(pointer),
            },
        );
        reopened.on_shelf(
            &mut context,
            &name,
            StoreResult::ShelfRead {
                name: name.clone(),
                offset: 0,
                size: u32::try_from(bytes.len()).unwrap(),
                bytes,
            },
        );
        assert_eq!(reopened.entries, app.entries);
        assert_eq!(reopened.entries_origin, Some(app.server.clone()));
        failed_save_preserves_snapshot(&mut app, &mut context);
    }

    #[test]
    fn session_loaded_after_config_still_opens_the_cache() {
        let mut app = ReadLater::default();
        let mut context = Context::default();
        app.on_load(
            &mut context,
            CONFIG,
            StoreResult::Loaded {
                key: CONFIG.into(),
                value: None,
            },
        );
        assert!(app.snapshot.is_none());
        let session = session::Session {
            server: "https://bag.example".into(),
            client_id: "id".into(),
            client_secret: "secret".into(),
            refresh_token: "refresh".into(),
        };
        app.on_load(
            &mut context,
            session::STORE_KEY,
            StoreResult::Loaded {
                key: session::STORE_KEY.into(),
                value: Some(session::encode(&session)),
            },
        );
        assert!(app.snapshot.is_some());
    }

    fn failed_save_preserves_snapshot(app: &mut ReadLater, context: &mut Context) {
        use kobo_sdk::{Command, StoreRequest};
        app.entries[0].content = "Changed article".into();
        app.keep_articles(context);
        let pending_name = context
            .take_commands()
            .into_iter()
            .find_map(|command| match command {
                Command::Store(StoreRequest::ShelfWrite { name, .. }) => Some(name),
                _ => None,
            })
            .unwrap();
        app.on_shelf(
            context,
            &pending_name,
            StoreResult::Denied(kobo_sdk::StoreError::TooFull),
        );
        assert!(app.snapshot.as_ref().unwrap().retryable());
        assert_ne!(
            cache::decode(app.snapshot.as_ref().unwrap().bytes.as_deref().unwrap()).unwrap(),
            app.entries
        );
        assert!(app.notice.as_deref().unwrap().contains("Retry saving"));
        capture_queue(app, "save-failed.png");
    }

    #[test]
    fn late_snapshot_fills_bodies_without_overwriting_fresh_metadata() {
        let saved = Entry {
            id: 7,
            title: "Old title".into(),
            site: "example.org".into(),
            reading_time: 2,
            position: 0,
            content: "Saved full body".into(),
            starred: false,
            archived: false,
        };
        let mut app = ReadLater {
            server: "https://bag.example".into(),
            ..ReadLater::default()
        };
        let mut context = Context::default();
        app.open_cache(&mut context);
        app.snapshot.as_mut().unwrap().bytes = cache::encode(&[saved]);
        app.entries = vec![Entry {
            id: 7,
            title: "New title".into(),
            site: "example.org".into(),
            reading_time: 3,
            position: 0,
            content: String::new(),
            starred: false,
            archived: false,
        }];
        app.cache_dirty = true;
        app.cache_event(&mut context, Some(SnapshotEvent::Loaded));
        assert_eq!(app.entries[0].title, "New title");
        assert_eq!(app.entries[0].reading_time, 3);
        assert_eq!(app.entries[0].content, "Saved full body");
    }

    #[test]
    fn refresh_keeps_fetched_bodies_and_rejects_invalid_lists() {
        let origin = "https://bag.example".to_owned();
        let mut app = ReadLater {
            server: origin.clone(),
            entries_origin: Some(origin.clone()),
            entries: vec![Entry {
                id: 7,
                title: "Old title".into(),
                site: "example.org".into(),
                reading_time: 2,
                position: 0,
                content: "An original saved article body.".into(),
                starred: false,
                archived: false,
            }],
            ..ReadLater::default()
        };
        let metadata = br#"{"items":[{"id":7,"title":"Updated title"}]}"#;
        let mut context = Context::default();
        app.task = Some((TaskId(1), PendingTask::Queue));
        app.task_origin = Some(origin.clone());
        app.on_task(
            &mut context,
            TaskId(1),
            TaskOutcome::Completed(metadata.to_vec()),
        );
        assert_eq!(app.entries[0].title, "Updated title");
        assert_eq!(app.entries[0].content, "An original saved article body.");
        app.task = Some((TaskId(2), PendingTask::Queue));
        app.task_origin = Some(origin.clone());
        app.on_task(
            &mut context,
            TaskId(2),
            TaskOutcome::Completed(b"not JSON".to_vec()),
        );
        assert_eq!(app.entries[0].content, "An original saved article body.");
        assert!(app.notice.as_deref().unwrap().contains("unchanged"));
        capture_queue(&app, "refresh-failed.png");
        app.server = "https://another.example".into();
        app.task = Some((TaskId(3), PendingTask::Queue));
        app.task_origin = Some(origin);
        app.on_task(
            &mut context,
            TaskId(3),
            TaskOutcome::Completed(br#"{"items":[]}"#.to_vec()),
        );
        assert_eq!(
            app.entries.len(),
            1,
            "late old-server reply replaced the list"
        );
        app.task = Some((TaskId(4), PendingTask::Queue));
        app.task_origin = Some(app.server.clone());
        app.on_task(
            &mut context,
            TaskId(4),
            TaskOutcome::Completed(metadata.to_vec()),
        );
        assert!(
            app.entries[0].content.is_empty(),
            "old-server content crossed into a new library"
        );
    }

    #[test]
    fn refused_token_refreshes_once_and_replays() {
        let mut app = ReadLater {
            session: Some(session::Session {
                server: "https://bag.example".into(),
                client_id: "1_abc".into(),
                client_secret: "secret".into(),
                refresh_token: "ref".into(),
            }),
            ..ReadLater::default()
        };
        let mut context = Context::default();
        app.task = Some((TaskId(1), PendingTask::Queue));
        app.task_origin = Some(app.server());
        app.on_task(
            &mut context,
            TaskId(1),
            TaskOutcome::Failed(TaskError::Unauthorized),
        );
        assert!(app.refreshing);
        assert_eq!(app.resume, Some(PendingTask::Queue));
        let posted = context.commands().iter().any(|command| {
            matches!(
                command,
                kobo_sdk::Command::Spawn {
                    work: Task::Post { url, credential: None, body, .. },
                    ..
                }
                if url == "https://bag.example/oauth/v2/token" && body.contains("refresh_token=ref")
            )
        });
        assert!(posted, "an uncredentialed refresh was spawned");
        // A second refusal while refreshing must not spawn another.
        app.on_task(
            &mut context,
            TaskId(2),
            TaskOutcome::Failed(TaskError::Unauthorized),
        );
        assert_eq!(app.refresh_attempts, 1);
    }

    fn capture_queue(app: &ReadLater, name: &str) {
        let Ok(directory) = std::env::var("KOBO_QUALITY_CAPTURE_DIR") else {
            return;
        };
        kobo_text::install(CLARA_BW_METRICS).unwrap();
        let mut context = Context::default();
        app.show(&mut context);
        let screen = context
            .commands()
            .iter()
            .find_map(|command| match command {
                kobo_sdk::Command::SetScreen(screen) => Some(screen),
                _ => None,
            })
            .unwrap();
        let metrics = context.metrics();
        let mut surface = kobo_ui::Surface::new(
            usize::try_from(metrics.width).unwrap(),
            usize::try_from(metrics.height).unwrap(),
        );
        kobo_ui::render(screen, &mut surface, None);
        let png = kobo_image::encode_png_grey(
            u32::try_from(metrics.width).unwrap(),
            u32::try_from(metrics.height).unwrap(),
            &surface.pixels,
        )
        .unwrap();
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(std::path::Path::new(&directory).join(name), png).unwrap();
    }

    #[test]
    fn setup_and_queue_fit_the_clara_panel() {
        for app in [
            ReadLater::default(),
            ReadLater {
                server: "https://bag.example".into(),
                depth: 50,
                ..ReadLater::default()
            },
        ] {
            assert!(app.sync_rect().width >= CLARA_BW_METRICS.touch_target_minimum());
        }
    }
    #[test]
    fn article_screens_fit_the_clara_panel() {
        let with_body = ScreenBuilder::new("readlater")
            .top_bar("Read Later")
            .heading("Why the Borrow Checker Exists")
            .secondary("example.com")
            .text("ownership is not optional")
            .action_bar([
                ("archive", "Archive"),
                ("star", "Star"),
                ("delete", "Delete"),
            ])
            .build();
        let loading = ScreenBuilder::new("readlater")
            .top_bar("Read Later")
            .heading("Why the Borrow Checker Exists")
            .secondary("example.com")
            .text("Loading article…")
            .button("back", "Back")
            .build();
        for screen in [with_body, loading] {
            let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
            assert!(
                layout.nodes.iter().any(|node| !node.text_lines.is_empty()),
                "article screen must draw"
            );
        }
    }

    #[test]
    fn archive_is_queued_without_a_secret() {
        let mut app = ReadLater {
            entries: vec![Entry {
                id: 1,
                title: "T".into(),
                site: "S".into(),
                reading_time: 1,
                position: 0,
                content: String::new(),
                starred: false,
                archived: false,
            }],
            open: Some(0),
            ..ReadLater::default()
        };
        app.pending.push(OutboxAction {
            id: app.entries[0].id,
            kind: OutboxKind::Archive,
        });
        assert_eq!(
            app.pending,
            [OutboxAction {
                id: 1,
                kind: OutboxKind::Archive
            }]
        );
        assert_eq!(OutboxAction::decode("a:1"), Some(app.pending[0]));
        assert_eq!(OutboxAction::decode("1"), Some(app.pending[0]));
        assert_eq!(
            OutboxAction::decode("s:9"),
            Some(OutboxAction {
                id: 9,
                kind: OutboxKind::Star
            })
        );
        assert_eq!(OutboxAction::decode("x:1"), None);
    }
    impl ReadLater {
        fn sync_rect(&self) -> kobo_ui::Rect {
            let screen = if self.ready() {
                ScreenBuilder::new("test")
                    .top_bar("Read Later")
                    .splash(
                        Some(Glyph::Bookmark),
                        "No saved articles",
                        "Sync Wallabag to add some.",
                    )
                    .button("sync", "Sync")
                    .build()
            } else {
                ScreenBuilder::new("test")
                    .top_bar("Read Later")
                    .splash(
                        Some(Glyph::Bookmark),
                        "Connect Wallabag",
                        "On your computer run `kobo secret set wallabag`, then add the HTTPS address here.",
                    )
                    .primary_button("settings", "Add address")
                    .build()
            };
            screen
                .layout_with(&CLARA_BW_METRICS, &Chrome::default())
                .rect_of_action(action_id(if self.ready() { "sync" } else { "settings" }))
                .expect("primary action must be reachable")
        }
    }
}

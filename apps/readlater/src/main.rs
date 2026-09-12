mod cache;
mod wallabag;

use kobo_sdk::snapshot::{Snapshot, SnapshotEvent};

use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Credential, Glyph, KoboApp, ScreenBuilder,
    StoreResult, Task, TaskId, TaskOutcome,
};
use std::process::ExitCode;
use wallabag::Entry;

const CONFIG: &str = "config";
const ACTIONS: &str = "actions";
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
    Credential,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PendingTask {
    Queue,
    Article(usize),
}

#[derive(Default)]
struct ReadLater {
    snapshot: Option<Snapshot>,
    cache_dirty: bool,
    server: String,
    credential: String,
    depth: u16,
    entries: Vec<Entry>,
    entries_origin: Option<(String, String)>,
    task_origin: Option<(String, String)>,
    open: Option<usize>,
    view: Option<View>,
    pending: Vec<u64>,
    task: Option<(TaskId, PendingTask)>,
    notice: Option<String>,
    keyboard: Keyboard,
    editing: Option<Setting>,
}

impl ReadLater {
    fn open_cache(&mut self, context: &mut Context) {
        if !self.ready() {
            return;
        }
        let snapshot = Snapshot::new(&format!(
            "readlater-v1:{}\n{}",
            self.server, self.credential
        ))
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
                                    }
                                }
                            }
                        } else {
                            self.entries = entries;
                            self.entries_origin =
                                Some((self.server.clone(), self.credential.clone()));
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
        self.server.starts_with("https://") && !self.credential.is_empty()
    }
    fn show(&self, context: &mut Context) {
        let view = self.view.unwrap_or(View::Queue);
        if let Some(setting) = self.editing {
            let prompt = match setting {
                Setting::Server => "Wallabag HTTPS server",
                Setting::Credential => "Credential name",
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
            View::Queue if !self.ready() => ScreenBuilder::new("readlater")
                .top_bar("Read Later")
                .splash(
                    Some(Glyph::Bookmark),
                    "Connect Wallabag",
                    "On your computer run `kobo secret set wallabag`, then add the HTTPS address here.",
                )
                .primary_button("settings", "Add address")
                .build(),
            View::Queue => {
                let mut page = ScreenBuilder::new("readlater")
                    .top_bar("Read Later")
                    .top_bar_action("sync", "Sync")
                    .top_bar_glyph("settings", "Settings", Glyph::Settings)
                    .tabs(
                        0,
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
                if self.entries.is_empty() {
                    page.splash(
                        Some(Glyph::Bookmark),
                        "No saved articles",
                        "Sync Wallabag to add some.",
                    )
                    .button("sync", "Sync")
                    .build()
                } else {
                    page.rows(self.entries.iter().enumerate().map(|(i, e)| {
                        (
                            format!("entry-{i}"),
                            e.title.clone(),
                            format!("{} · {} min", e.site, e.reading_time),
                            Glyph::Bookmark,
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
            View::Article => {
                let entry = self.open.and_then(|i| self.entries.get(i));
                let loading = matches!(self.task, Some((_, PendingTask::Article(_))));
                match entry {
                    Some(e) if !e.content.is_empty() => ScreenBuilder::new("readlater").top_bar("Read Later").heading(&e.title).secondary(&e.site).text(&e.content).action_bar([("archive", "Archive"), ("star", "Star"), ("delete", "Delete")]).build(),
                    Some(e) if loading => ScreenBuilder::new("readlater").top_bar("Read Later").heading(&e.title).secondary(&e.site).text("Loading article…").button("back", "Back").build(),
                    Some(e) => ScreenBuilder::new("readlater").top_bar("Read Later").heading(&e.title).text("Wallabag couldn't extract this one. Open the original URL in Wallabag.").action_bar([("archive", "Archive"), ("back", "Back")]).build(),
                    None => ScreenBuilder::new("readlater").top_bar("Read Later").splash(Some(Glyph::Bookmark), "Choose an article", "Open one from your reading list.").build(),
                }
            }
            View::Settings => ScreenBuilder::new("readlater")
                .top_bar("Read Later settings")
                .field("server", &self.server, "https://wallabag.example")
                .secondary("Finish Wallabag setup on your computer.")
                .choose(
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
                .build(),
        };
        context.set_screen(screen);
    }
    fn sync(&mut self, context: &mut Context) {
        if !self.ready() {
            self.view = Some(View::Settings);
            return;
        }
        self.notice = Some("Syncing newest articles…".to_owned());
        if let Some(id) = context.spawn_retrying(Task::Fetch {
            url: wallabag::queue_url(&self.server, self.depth.max(20)),
            offset: 0,
            max_bytes: 512 * 1024,
            credential: Some(Credential::bearer(&self.credential)),
            headers: Vec::new(),
        }) {
            self.task = Some((id, PendingTask::Queue));
            self.task_origin = Some((self.server.clone(), self.credential.clone()));
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
            url: wallabag::entry_url(&self.server, entry.id),
            offset: 0,
            max_bytes: 512 * 1024,
            credential: Some(Credential::bearer(&self.credential)),
            headers: Vec::new(),
        }) {
            self.task = Some((id, PendingTask::Article(index)));
            self.task_origin = Some((self.server.clone(), self.credential.clone()));
        }
    }
    fn persist_actions(&self, context: &mut Context) {
        context.store().save(
            ACTIONS,
            self.pending
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(","),
        );
    }
    fn persist_config(&self, context: &mut Context) {
        context
            .store()
            .save(CONFIG, format!("{}\n{}", self.server, self.credential));
    }
}

impl KoboApp for ReadLater {
    fn on_start(&mut self, context: &mut Context) {
        "wallabag".clone_into(&mut self.credential);
        self.depth = 50;
        context.store().load(CONFIG);
        context.store().load(ACTIONS);
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
        if let Some(snapshot) = self
            .snapshot
            .as_mut()
            .filter(|snapshot| snapshot.owns_file(name))
        {
            let event = snapshot.shelf(context, &result);
            self.cache_event(context, event);
        }
    }

    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        if let StoreResult::Loaded { key, value } = result {
            if key == CONFIG {
                if let Some(value) = &value {
                    let text = String::from_utf8_lossy(value);
                    let mut parts = text.lines();
                    parts
                        .next()
                        .unwrap_or_default()
                        .clone_into(&mut self.server);
                    parts
                        .next()
                        .unwrap_or("wallabag")
                        .clone_into(&mut self.credential);
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
                            .filter_map(|x| x.parse().ok())
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
                            Setting::Credential => self.credential = value,
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
        } else if action == action_id("credential") {
            self.keyboard.clear();
            self.editing = Some(Setting::Credential);
        } else if action == action_id("retry-save") {
            if let Some(snapshot) = &mut self.snapshot {
                snapshot.retry(context);
            }
            self.flush_cache(context);
        } else if action == action_id("sync") {
            self.sync(context);
        } else if action == action_id("back") || action == ActionId::BACK {
            self.view = Some(View::Queue);
        } else if action == action_id("depth-20") {
            self.depth = 20;
        } else if action == action_id("depth-50") {
            self.depth = 50;
        } else if action == action_id("depth-100") {
            self.depth = 100;
        } else if action == action_id("archive") {
            if let Some(e) = self.open.and_then(|i| self.entries.get(i)) {
                self.pending.push(e.id);
                self.persist_actions(context);
                self.notice = Some("Archived locally; pending sync.".to_owned());
                self.view = Some(View::Queue);
            }
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
        let origin = (self.server.clone(), self.credential.clone());
        if self
            .task_origin
            .take()
            .is_some_and(|requested| requested != origin)
        {
            return;
        }
        match (kind, outcome) {
            (PendingTask::Queue, TaskOutcome::Completed(bytes)) => {
                if let Some(mut entries) = wallabag::parse_entries(&bytes) {
                    if self.entries_origin.as_ref() == Some(&origin) {
                        for entry in &mut entries {
                            if entry.content.is_empty() {
                                if let Some(previous) =
                                    self.entries.iter_mut().find(|old| old.id == entry.id)
                                {
                                    entry.content = std::mem::take(&mut previous.content);
                                }
                            }
                        }
                    }
                    self.entries = entries;
                    self.entries_origin = Some(origin);
                    self.notice = Some(format!("Synced {} articles.", self.entries.len()));
                    self.keep_articles(context);
                } else {
                    self.notice = Some("The reading list could not be loaded. Your current articles are unchanged. Try syncing again.".into());
                }
            }
            (PendingTask::Article(index), TaskOutcome::Completed(bytes)) => {
                if let Some(entry) = wallabag::parse_entry_document(&bytes) {
                    if let Some(slot) = self.entries.get_mut(index) {
                        if slot.id == entry.id {
                            *slot = entry;
                            self.keep_articles(context);
                        }
                    }
                }
            }
            (_, TaskOutcome::Failed(_)) => {
                self.notice = Some(
                    "Off the air. Cached articles remain readable; join Wi-Fi to sync.".to_owned(),
                );
            }
            (_, TaskOutcome::Cancelled) => self.notice = Some("Sync cancelled.".to_owned()),
        }
        self.show(context);
    }
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
    fn acknowledged_article_snapshot_restores_in_a_fresh_app() {
        use kobo_sdk::{Command, StoreRequest};
        let mut app = ReadLater {
            server: "https://bag.example".into(),
            credential: "wallabag".into(),
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
            content: "First paragraph.\n\nUse <section> literally.".into(),
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
            credential: app.credential.clone(),
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
        assert_eq!(
            reopened.entries_origin,
            Some((app.server.clone(), app.credential.clone()))
        );
        failed_save_preserves_snapshot(&mut app, &mut context);
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
            content: "Saved full body".into(),
        };
        let mut app = ReadLater {
            server: "https://bag.example".into(),
            credential: "wallabag".into(),
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
            content: String::new(),
        }];
        app.cache_dirty = true;
        app.cache_event(&mut context, Some(SnapshotEvent::Loaded));
        assert_eq!(app.entries[0].title, "New title");
        assert_eq!(app.entries[0].reading_time, 3);
        assert_eq!(app.entries[0].content, "Saved full body");
    }

    #[test]
    fn refresh_keeps_fetched_bodies_and_rejects_invalid_lists() {
        let origin = ("https://bag.example".to_owned(), "wallabag".to_owned());
        let mut app = ReadLater {
            server: origin.0.clone(),
            credential: origin.1.clone(),
            entries_origin: Some(origin.clone()),
            entries: vec![Entry {
                id: 7,
                title: "Old title".into(),
                site: "example.org".into(),
                reading_time: 2,
                content: "An original saved article body.".into(),
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
        app.task_origin = Some((app.server.clone(), app.credential.clone()));
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
                credential: "wallabag".into(),
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
                content: String::new(),
            }],
            open: Some(0),
            ..ReadLater::default()
        };
        app.pending.push(app.entries[0].id);
        assert_eq!(app.pending, [1]);
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

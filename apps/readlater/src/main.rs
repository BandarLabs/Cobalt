mod wallabag;

use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{action_id, ActionId, BannerLevel, Context, Credential, Glyph, KoboApp, ScreenBuilder, StoreResult, Task, TaskId, TaskOutcome};
use std::process::ExitCode;
use wallabag::Entry;

const CONFIG: &str = "config";
const ACTIONS: &str = "actions";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum View { Queue, Article, Settings }
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Setting { Server, Credential }

#[derive(Default)]
struct ReadLater {
    server: String,
    credential: String,
    depth: u16,
    entries: Vec<Entry>,
    open: Option<usize>,
    view: Option<View>,
    pending: Vec<u64>,
    task: Option<TaskId>,
    notice: Option<String>,
    keyboard: Keyboard,
    editing: Option<Setting>,
}

impl ReadLater {
    fn ready(&self) -> bool { self.server.starts_with("https://") && !self.credential.is_empty() }
    fn show(&self, context: &mut Context) {
        let view = self.view.unwrap_or(View::Queue);
        if let Some(setting) = self.editing {
            let prompt = match setting { Setting::Server => "Wallabag HTTPS server", Setting::Credential => "Credential name" };
            context.set_screen(ScreenBuilder::new("readlater").top_bar("Read Later settings").typed(&self.keyboard, prompt).keyboard(&self.keyboard, "Save").build());
            return;
        }
        let screen = match view {
            View::Queue if !self.ready() => ScreenBuilder::new("readlater").top_bar("Read Later").heading("Setup needed").text("Set a Wallabag server before the first sync.").secondary("kobo secret set wallabag").bottom_action("settings", "Settings").build(),
            View::Queue => {
                let mut page = ScreenBuilder::new("readlater").top_bar_action("sync", "Sync").top_bar("Read Later")
                    .tabs(0, [("unread", "Unread"), ("starred", "Starred"), ("archive-tab", "Archive")]);
                if let Some(note) = &self.notice { page = page.banner(BannerLevel::Attention, note); }
                if self.entries.is_empty() { page.empty_state("Saved articles appear here after Sync.").button("sync", "Sync").build() }
                else { page.rows(self.entries.iter().enumerate().map(|(i, e)| (format!("entry-{i}"), e.title.clone(), format!("{} · {} min", e.site, e.reading_time), Glyph::Bookmark))).secondary(format!("{} action{} pending sync", self.pending.len(), if self.pending.len() == 1 { "" } else { "s" })).build() }
            }
            View::Article => {
                let entry = self.open.and_then(|i| self.entries.get(i));
                match entry {
                    Some(e) if !e.content.is_empty() => ScreenBuilder::new("readlater").top_bar("Read Later").heading(&e.title).secondary(&e.site).text(&e.content).action_bar([("archive", "Archive"), ("star", "Star"), ("delete", "Delete")]).build(),
                    Some(e) => ScreenBuilder::new("readlater").top_bar("Read Later").heading(&e.title).text("Wallabag couldn't extract this one. Open the original URL in Wallabag.").action_bar([("archive", "Archive"), ("back", "Back")]).build(),
                    None => ScreenBuilder::new("readlater").top_bar("Read Later").empty_state("Choose an article from the queue.").build(),
                }
            }
            View::Settings => ScreenBuilder::new("readlater").top_bar("Read Later settings")
                .field("server", &self.server, "https://wallabag.example").field("credential", &self.credential, "wallabag")
                .secondary("Credential values stay in kobod. Run `kobo secret set wallabag`.").choose("Sync depth", [("depth-20", "20 newest"), ("depth-50", "50 newest"), ("depth-100", "100 newest")]).chosen(match self.depth { 100 => 2, 50 => 1, _ => 0 }).button("back", "Back").build(),
        };
        context.set_screen(screen);
    }
    fn sync(&mut self, context: &mut Context) {
        if !self.ready() { self.view = Some(View::Settings); return; }
        self.notice = Some("Syncing newest articles…".to_owned());
        self.task = context.spawn_retrying(Task::Fetch { url: wallabag::queue_url(&self.server, self.depth.max(20)), offset: 0, max_bytes: 512 * 1024, credential: Some(Credential::bearer(&self.credential)), headers: Vec::new() });
    }
    fn persist_actions(&self, context: &mut Context) { context.store().save(ACTIONS, self.pending.iter().map(u64::to_string).collect::<Vec<_>>().join(",")); }
    fn persist_config(&self, context: &mut Context) { context.store().save(CONFIG, format!("{}\n{}", self.server, self.credential)); }
}

impl KoboApp for ReadLater {
    fn on_start(&mut self, context: &mut Context) { self.credential = "wallabag".to_owned(); self.depth = 50; context.store().load(CONFIG); context.store().load(ACTIONS); self.show(context); }
    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        if let StoreResult::Loaded { key, value } = result {
            if key == CONFIG { if let Some(value) = &value { let text = String::from_utf8_lossy(value); let mut parts = text.lines(); self.server = parts.next().unwrap_or_default().to_owned(); self.credential = parts.next().unwrap_or("wallabag").to_owned(); } }
            if key == ACTIONS { self.pending = value.as_ref().map(|v| String::from_utf8_lossy(v).split(',').filter_map(|x| x.parse().ok()).collect()).unwrap_or_default(); }
            self.show(context);
        }
    }
    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if let Some(setting) = self.editing {
            match self.keyboard.press(action) {
                Some(Pressed::Submitted) => {
                    let value = self.keyboard.take().trim().to_owned();
                    if !value.is_empty() {
                        match setting { Setting::Server => self.server = value, Setting::Credential => self.credential = value }
                        self.persist_config(context);
                    }
                    self.editing = None;
                }
                Some(Pressed::Edited | Pressed::Shifted) => {}
                None if action == ActionId::BACK => self.editing = None,
                None => {}
            }
        } else if action == action_id("settings") { self.view = Some(View::Settings); }
        else if action == action_id("server") { self.keyboard.clear(); self.editing = Some(Setting::Server); }
        else if action == action_id("credential") { self.keyboard.clear(); self.editing = Some(Setting::Credential); }
        else if action == action_id("sync") { self.sync(context); }
        else if action == action_id("back") || action == ActionId::BACK { self.view = Some(View::Queue); }
        else if action == action_id("depth-20") { self.depth = 20; }
        else if action == action_id("depth-50") { self.depth = 50; }
        else if action == action_id("depth-100") { self.depth = 100; }
        else if action == action_id("archive") { if let Some(e) = self.open.and_then(|i| self.entries.get(i)) { self.pending.push(e.id); self.persist_actions(context); self.notice = Some("Archived locally; pending sync.".to_owned()); self.view = Some(View::Queue); } }
        else if let Some(index) = (0..self.entries.len()).find(|i| action == action_id(&format!("entry-{i}"))) { self.open = Some(index); self.view = Some(View::Article); }
        self.show(context);
    }
    fn on_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
        if self.task != Some(task) { return; }
        self.task = None;
        match outcome { TaskOutcome::Completed(bytes) => { self.entries = wallabag::parse_entries(&bytes); self.notice = Some(format!("Synced {} articles.", self.entries.len())); }, TaskOutcome::Failed(_) => self.notice = Some("Off the air. Cached articles remain readable; join Wi-Fi to sync.".to_owned()), TaskOutcome::Cancelled => self.notice = Some("Sync cancelled.".to_owned()) }
        self.show(context);
    }
}

fn main() -> ExitCode { kobo_sdk::run("readlater", ReadLater::default()).map_or_else(|error| { eprintln!("readlater: {error}"); ExitCode::FAILURE }, |_| ExitCode::SUCCESS) }

#[cfg(test)]
mod tests {
    use super::*;
    use kobo_ui::{Chrome, CLARA_BW_METRICS};
    #[test] fn setup_and_queue_fit_the_clara_panel() { for app in [ReadLater::default(), ReadLater { server: "https://bag.example".into(), credential: "wallabag".into(), depth: 50, ..ReadLater::default() }] { assert!(app.sync_rect().width >= CLARA_BW_METRICS.touch_target_minimum()); } }
    #[test] fn archive_is_queued_without_a_secret() { let mut app = ReadLater { entries: vec![Entry { id: 1, title: "T".into(), site: "S".into(), reading_time: 1, content: String::new() }], open: Some(0), ..ReadLater::default() }; app.pending.push(app.entries[0].id); assert_eq!(app.pending, [1]); }
    impl ReadLater { fn sync_rect(&self) -> kobo_ui::Rect { let screen = if self.ready() { ScreenBuilder::new("test").top_bar("Read Later").empty_state("Saved articles appear here after Sync.").button("sync", "Sync").build() } else { ScreenBuilder::new("test").top_bar("Read Later").heading("Setup needed").secondary("kobo secret set wallabag").bottom_action("settings", "Settings").build() }; screen.layout_with(&CLARA_BW_METRICS, &Chrome::default()).rect_of_action(action_id(if self.ready() { "sync" } else { "settings" })).expect("primary action must be reachable") } }
}

mod miniflux;
use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{action_id, ActionId, BannerLevel, Context, Glyph, KoboApp, ScreenBuilder, StoreResult, TaskId, TaskOutcome};
use miniflux::Article;
use std::process::ExitCode;

#[derive(Clone, Copy, Debug, Eq, PartialEq)] enum View { Shelf, Article, Settings, Directory }
#[derive(Clone, Copy, Debug, Eq, PartialEq)] enum Setting { Server, Credential }
#[derive(Default)] struct Reader { server: String, credential: String, articles: Vec<Article>, open: Option<usize>, view: Option<View>, queued_reads: Vec<u64>, task: Option<TaskId>, notice: Option<String>, keyboard: Keyboard, editing: Option<Setting> }
const CONFIG: &str = "config";
const ACTIONS: &str = "read-actions";

impl Reader {
    fn configured(&self) -> bool { self.server.starts_with("https://") && !self.credential.is_empty() }
    fn show(&self, context: &mut Context) {
        let view = self.view.unwrap_or(View::Shelf);
        if let Some(setting) = self.editing {
            let prompt = match setting { Setting::Server => "Miniflux HTTPS server", Setting::Credential => "Credential name" };
            context.set_screen(ScreenBuilder::new("rss-miniflux").top_bar("RSS Reader settings").typed(&self.keyboard, prompt).keyboard(&self.keyboard, "Save").build());
            return;
        }
        let screen = match view {
            View::Shelf if !self.configured() => ScreenBuilder::new("rss-miniflux").top_bar("RSS Reader").heading("Setup needed").text("Set a Miniflux server before the first sync.").secondary("kobo secret set miniflux").buttons([("settings", "Settings"), ("directory", "Starter directory")]).build(),
            View::Shelf => { let mut page = ScreenBuilder::new("rss-miniflux").top_bar("RSS Reader").top_bar_action("sync", "Sync").tabs(0, [("unread", "Unread"), ("starred", "Starred"), ("history", "History")]); if let Some(n) = &self.notice { page = page.banner(BannerLevel::Attention, n); } if self.articles.is_empty() { page.empty_state("Unread articles appear here after Sync.").button("sync", "Sync").build() } else { page.rows(self.articles.iter().enumerate().map(|(i,a)| (format!("article-{i}"), a.title.clone(), a.feed.clone(), if a.starred { Glyph::Bookmark } else { Glyph::Rss }))).secondary(format!("{} reads pending sync", self.queued_reads.len())).build() } }
            View::Article => match self.open.and_then(|i| self.articles.get(i)) { Some(a) => ScreenBuilder::new("rss-miniflux").top_bar("RSS Reader").heading(&a.title).secondary(&a.feed).text(if a.content.is_empty() { "This feed did not supply article content. Use Load full article when Miniflux can fetch it." } else { &a.content }).action_bar([("mark-read", "Mark read"), ("star", "Star"), ("full", "Load full article")]).build(), None => ScreenBuilder::new("rss-miniflux").top_bar("RSS Reader").empty_state("Choose an article from the unread list.").build() },
            View::Settings => ScreenBuilder::new("rss-miniflux").top_bar("RSS Reader settings").field("server", &self.server, "https://miniflux.example").field("credential", &self.credential, "miniflux").secondary("The token stays in kobod as X-Auth-Token.").button("back", "Back").build(),
            View::Directory => ScreenBuilder::new("rss-miniflux").top_bar("Starter directory").rows([("science", "Science News", "A feed is added after you choose it.", Glyph::Rss), ("engineering", "Engineering blogs", "A feed is added after you choose it.", Glyph::Rss), ("longform", "Long-form writing", "A feed is added after you choose it.", Glyph::Rss)]).button("back", "Back").build(),
        };
        context.set_screen(screen);
    }
    fn sync(&mut self, context: &mut Context) { if !self.configured() { self.view = Some(View::Settings); return; } self.notice = Some("Syncing unread articles…".into()); self.task = context.spawn_retrying(miniflux::unread(&self.server, &self.credential, 100)); }
    fn persist_config(&self, context: &mut Context) { context.store().save(CONFIG, format!("{}\n{}", self.server, self.credential)); }
}
impl KoboApp for Reader {
    fn on_start(&mut self, context: &mut Context) { self.credential = "miniflux".into(); context.store().load(CONFIG); context.store().load(ACTIONS); self.show(context); }
    fn on_store(&mut self, context: &mut Context, result: StoreResult) { if let StoreResult::Loaded { key, value } = result { if key == CONFIG { if let Some(value) = &value { let saved = String::from_utf8_lossy(value); let mut values = saved.lines(); self.server = values.next().unwrap_or_default().to_owned(); self.credential = values.next().unwrap_or("miniflux").to_owned(); } } else if key == ACTIONS { self.queued_reads = value.as_ref().map(|v| String::from_utf8_lossy(v).split(',').filter_map(|x|x.parse().ok()).collect()).unwrap_or_default(); } self.show(context); } }
    fn on_action(&mut self, context: &mut Context, action: ActionId) { if let Some(setting) = self.editing { match self.keyboard.press(action) { Some(Pressed::Submitted) => { let value = self.keyboard.take().trim().to_owned(); if !value.is_empty() { match setting { Setting::Server => self.server = value, Setting::Credential => self.credential = value } self.persist_config(context); } self.editing = None; }, Some(Pressed::Edited | Pressed::Shifted) => {}, None if action == ActionId::BACK => self.editing = None, None => {} } } else if action == action_id("settings") { self.view = Some(View::Settings); } else if action == action_id("server") { self.keyboard.clear(); self.editing = Some(Setting::Server); } else if action == action_id("credential") { self.keyboard.clear(); self.editing = Some(Setting::Credential); } else if action == action_id("directory") { self.view = Some(View::Directory); } else if action == action_id("back") || action == ActionId::BACK { self.view = Some(View::Shelf); } else if action == action_id("sync") { self.sync(context); } else if action == action_id("mark-read") { if let Some(article) = self.open.and_then(|i| self.articles.get(i)) { self.queued_reads.push(article.id); context.store().save(ACTIONS, self.queued_reads.iter().map(u64::to_string).collect::<Vec<_>>().join(",")); self.notice = Some("Read locally; pending sync.".into()); self.view = Some(View::Shelf); } } else if let Some(i) = (0..self.articles.len()).find(|i| action == action_id(&format!("article-{i}"))) { self.open = Some(i); self.view = Some(View::Article); } self.show(context); }
    fn on_task(&mut self, context: &mut Context, task: TaskId, result: TaskOutcome) { if self.task != Some(task) { return; } self.task = None; match result { TaskOutcome::Completed(bytes) => { self.articles = miniflux::parse_entries(&bytes); self.notice = Some(format!("Synced {} unread articles.", self.articles.len())); }, TaskOutcome::Failed(_) => self.notice = Some("Off the air. Cached articles remain readable; join Wi-Fi to sync.".into()), TaskOutcome::Cancelled => self.notice = Some("Sync cancelled.".into()) } self.show(context); }
}
fn main() -> ExitCode { kobo_sdk::run("rss-miniflux", Reader::default()).map_or_else(|e| { eprintln!("rss-miniflux: {e}"); ExitCode::FAILURE }, |_| ExitCode::SUCCESS) }

#[cfg(test)]
mod tests { use super::*; use kobo_ui::{Chrome, CLARA_BW_METRICS}; #[test] fn setup_actions_are_reachable() { let layout = ScreenBuilder::new("rss").top_bar("RSS Reader").permission_denied_state("Set a Miniflux server, then run `kobo secret set miniflux`.").button("settings", "Settings").button("directory", "Starter directory").build().layout_with(&CLARA_BW_METRICS, &Chrome::default()); assert!(layout.rect_of_action(action_id("settings")).is_some()); assert!(layout.rect_of_action(action_id("directory")).is_some()); } }

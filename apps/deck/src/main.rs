mod model;
use kobo_sdk::keyboard::{TextEntry, Typing};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Glyph, KoboApp, Screen, ScreenBuilder, StoreResult,
    Task, TaskId, TaskOutcome,
};
use model::{decode, Deck};
use std::process::ExitCode;
const PAIRED: &str = "paired";
const CACHE: &str = "deck-cache";
#[derive(Clone, Copy, PartialEq)]
enum View {
    Opening,
    Address,
    Code,
    Grid,
}
struct App {
    view: View,
    address: String,
    code: String,
    entry: TextEntry,
    deck: Deck,
    page: usize,
    notice: Option<String>,
    task: Option<TaskId>,
    pressed: Option<String>,
    confirming: Option<String>,
}
impl Default for App {
    fn default() -> Self {
        Self {
            view: View::Opening,
            address: String::new(),
            code: String::new(),
            entry: TextEntry::new(),
            deck: Deck::fallback(),
            page: 0,
            notice: None,
            task: None,
            pressed: None,
            confirming: None,
        }
    }
}
impl App {
    fn show(&self, cx: &mut Context) {
        cx.set_screen(self.screen());
    }
    fn screen(&self) -> Screen {
        match self.view{View::Opening=>ScreenBuilder::new("deck-opening").top_bar("Deck").activity("Opening",None).build(),View::Address=>ScreenBuilder::new("deck-address").top_bar("Deck").heading("Pair with your computer").text("Run kobo-sidekickd init, then type its address. Deck uses the existing Sidekick pairing.").text_entry(&self.entry,"Address","Next").build(),View::Code=>ScreenBuilder::new("deck-code").top_bar("Deck").heading("Now the pairing code").text("Type the six characters printed by kobo-sidekickd init.").text_entry(&self.entry,"Pairing code","Pair").build(),View::Grid=>self.grid()}
    }
    fn grid(&self) -> Screen {
        let page = self
            .deck
            .pages
            .get(self.page)
            .unwrap_or(&self.deck.pages[0]);
        let tabs = self
            .deck
            .pages
            .iter()
            .map(|p| (format!("page-{}", p.name), p.name.clone()))
            .collect::<Vec<_>>();
        let mut screen = ScreenBuilder::new("deck-grid")
            .top_bar("Deck")
            .tabs(self.page, tabs);
        if let Some(note) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, note);
        }
        if page.keys.is_empty() {
            return screen
                .splash(
                    Some(Glyph::Grid),
                    "No keys",
                    "Create ~/.config/kobo/sidekick/deck.toml, then leave this screen open.",
                )
                .build();
        }
        screen = screen.grid(
            2,
            false,
            page.keys.iter().map(|key| {
                let status = match key.state.as_str() {
                    "running" => "working…",
                    "ok" => "done",
                    "failed" => "failed",
                    _ => "",
                };
                (
                    format!("press-{}", key.id),
                    if key.detail.is_empty() {
                        format!("{} {status}", key.label)
                    } else {
                        format!("{}\n{}\n{status}", key.label, key.detail)
                    },
                )
            }),
        );
        if let Some(id) = &self.confirming {
            if let Some(key) = page.keys.iter().find(|key| &key.id == id) {
                screen = screen.confirm(
                    key.label.clone(),
                    "Run this on the paired computer?",
                    ("confirm-run", "Run"),
                    ("cancel-run", "Cancel"),
                );
            }
        }
        screen.build()
    }
    fn poll(&mut self, cx: &mut Context) {
        if self.task.is_none() {
            self.task = cx.spawn(Task::Fetch {
                url: format!(
                    "https://{}/deck?version={}&token={}",
                    self.address, self.deck.version, self.code
                ),
                offset: 0,
                max_bytes: 64 * 1024,
                credential: None,
                headers: vec![],
            });
        }
    }
    fn press(&mut self, cx: &mut Context, id: &str, confirmed: bool) {
        self.pressed = Some(id.to_owned());
        self.task = cx.spawn(Task::Post {
            url: format!("https://{}/deck/press?token={}", self.address, self.code),
            body: format!("{{\"key\":\"{id}\",\"confirmed\":{confirmed}}}"),
            content_type: "application/json".into(),
            credential: None,
            headers: vec![],
            max_bytes: 4096,
        });
    }
}
impl KoboApp for App {
    fn on_start(&mut self, cx: &mut Context) {
        cx.store().load(PAIRED);
        cx.store().load_cached(CACHE);
        self.show(cx);
    }
    fn on_store(&mut self, cx: &mut Context, result: StoreResult) {
        if let StoreResult::Loaded { key, value } = result {
            if key == PAIRED {
                if let Some(raw) = value.and_then(|v| String::from_utf8(v).ok()) {
                    if let Some((address, code)) = raw.split_once('|') {
                        self.address = address.into();
                        self.code = code.into();
                        self.view = View::Grid;
                        self.poll(cx);
                    } else {
                        self.view = View::Address;
                        self.entry.open();
                    }
                } else {
                    self.view = View::Address;
                    self.entry.open();
                }
            } else if key == format!("cache:{CACHE}") {
                if let Some(raw) = value.and_then(|v| String::from_utf8(v).ok()) {
                    if let Some(deck) = decode(&raw) {
                        self.deck = deck;
                    }
                }
            }
            self.show(cx);
        }
    }
    fn on_task(&mut self, cx: &mut Context, task: TaskId, outcome: TaskOutcome) {
        if self.task != Some(task) {
            return;
        }
        self.task = None;
        match outcome {
            TaskOutcome::Completed(bytes) => {
                let raw = String::from_utf8_lossy(&bytes).into_owned();
                if self.pressed.take().is_some() {
                    if raw.contains("needs-confirm") {
                        self.notice = Some("This command needs confirmation.".into());
                    } else if raw.contains("started") {
                        self.notice = Some("Command started.".into());
                        self.poll(cx);
                    } else {
                        self.notice = Some("That key changed. Loading the fresh deck.".into());
                        self.poll(cx);
                    }
                } else if let Some(deck) = decode(&raw) {
                    self.deck = deck;
                    cx.store().cache(CACHE, raw);
                }
            }
            TaskOutcome::Failed(_) => {
                self.notice = Some("off the air — is kobo-sidekickd running?".into());
            }
            TaskOutcome::Cancelled => {}
        }
        self.show(cx);
    }
    fn on_action(&mut self, cx: &mut Context, a: ActionId) {
        if matches!(self.view, View::Address | View::Code) {
            if let Some(event) = self.entry.handle(a) {
                if let Typing::Submitted(text) = event {
                    if self.view == View::Address {
                        self.address = text;
                        self.view = View::Code;
                        self.entry.open();
                    } else {
                        self.code = text;
                        cx.store()
                            .save(PAIRED, format!("{}|{}", self.address, self.code));
                        self.view = View::Grid;
                        self.poll(cx);
                    }
                }
                self.show(cx);
                return;
            }
        }
        if a == action_id("confirm-run") {
            if let Some(id) = self.confirming.take() {
                self.press(cx, &id, true);
            }
            self.show(cx);
            return;
        }
        if a == action_id("cancel-run") {
            self.confirming = None;
            self.show(cx);
            return;
        }
        for (index, page) in self.deck.pages.iter().enumerate() {
            if a == action_id(&format!("page-{}", page.name)) {
                self.page = index;
                self.show(cx);
                return;
            }
            for key in &page.keys {
                if a == action_id(&format!("press-{}", key.id)) {
                    if key.confirm {
                        self.confirming = Some(key.id.clone());
                    } else {
                        let id = key.id.clone();
                        self.press(cx, &id, false);
                    }
                    self.show(cx);
                    return;
                }
            }
        }
    }
}
fn main() -> ExitCode {
    match kobo_sdk::run("deck", App::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("deck: {error}");
            ExitCode::FAILURE
        }
    }
}

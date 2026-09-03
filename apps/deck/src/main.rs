mod model;
use kobo_sdk::keyboard::{TextEntry, Typing};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Glyph, KoboApp, Screen, ScreenBuilder, StoreResult,
    Task, TaskId, TaskOutcome,
};
use model::{decode, decode_result, Deck, RunResult};
use std::process::ExitCode;
const PAIRED: &str = "paired";
const CACHE: &str = "deck-cache";
#[derive(Clone, Copy, PartialEq)]
enum View {
    Opening,
    Address,
    Code,
    Grid,
    Result,
}
#[derive(Clone, Debug, Eq, PartialEq)]
enum Pending {
    Poll,
    Press(String),
    Result(String),
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
    pending: Option<Pending>,
    confirming: Option<String>,
    result: Option<(String, RunResult)>,
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
            pending: None,
            confirming: None,
            result: None,
        }
    }
}
impl App {
    fn show(&self, cx: &mut Context) {
        cx.set_screen(self.screen());
    }
    fn screen(&self) -> Screen {
        match self.view {
            View::Opening => ScreenBuilder::new("deck-opening")
                .top_bar("Deck")
                .activity("Opening", None)
                .build(),
            View::Address => ScreenBuilder::new("deck-address")
                .top_bar("Deck")
                .heading("Pair with your computer")
                .text("Start Sidekick on your computer, then enter the address it shows.")
                .text_entry(&self.entry, "Computer address", "Next")
                .build(),
            View::Code => ScreenBuilder::new("deck-code")
                .top_bar("Deck")
                .heading("Now the pairing code")
                .text("Enter the six-character code shown on your computer.")
                .text_entry(&self.entry, "Pairing code", "Pair")
                .build(),
            View::Grid => self.grid(),
            View::Result => self.result_screen(),
        }
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
        if self.task.is_none() {
            screen = screen.button("retry", "Refresh");
        }
        if page.keys.is_empty() {
            return screen
                .splash(
                    Some(Glyph::Grid),
                    "No controls yet",
                    "Add controls in Deck on your computer, then keep this screen open.",
                )
                .build();
        }
        screen = screen.grid(
            2,
            false,
            page.keys.iter().map(|key| {
                let status = match key.state.as_str() {
                    "running" => "Running…",
                    "ok" => "✓",
                    "failed" => "×",
                    _ => "",
                };
                (
                    format!("press-{}", key.id),
                    if key.detail.is_empty() {
                        format!("{} {status}", key.label)
                    } else {
                        format!("{} · {} {status}", key.label, key.detail)
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
    fn result_screen(&self) -> Screen {
        let Some((label, result)) = &self.result else {
            return ScreenBuilder::new("deck-result")
                .top_bar("Deck")
                .heading("No result")
                .button("back", "Back to controls")
                .build();
        };
        let status = match (result.status.as_str(), result.exit) {
            ("running", _) => "Still running".to_owned(),
            ("ok", Some(exit)) => format!("Finished · exit {exit}"),
            ("failed", Some(exit)) => format!("Failed · exit {exit}"),
            ("ok", None) => "Finished".to_owned(),
            _ => "Failed".to_owned(),
        };
        ScreenBuilder::new("deck-result")
            .top_bar("Deck")
            .heading(label)
            .secondary(status)
            .text(if result.tail.is_empty() {
                "This command produced no output."
            } else {
                result.tail.as_str()
            })
            .button("back", "Back to controls")
            .build()
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
            self.pending = Some(Pending::Poll);
        }
    }
    fn press(&mut self, cx: &mut Context, id: &str, confirmed: bool) {
        if let Some(key) = self
            .deck
            .pages
            .iter_mut()
            .flat_map(|page| page.keys.iter_mut())
            .find(|key| key.id == id)
        {
            "running".clone_into(&mut key.state);
        }
        self.task = cx.spawn(Task::Post {
            url: format!("https://{}/deck/press?token={}", self.address, self.code),
            body: format!("{{\"key\":\"{id}\",\"confirmed\":{confirmed}}}"),
            content_type: "application/json".into(),
            credential: None,
            headers: vec![],
            max_bytes: 4096,
        });
        self.pending = Some(Pending::Press(id.to_owned()));
    }
    fn result(&mut self, cx: &mut Context, id: &str) {
        self.task = cx.spawn(Task::Fetch {
            url: format!(
                "https://{}/deck/result?key={id}&token={}",
                self.address, self.code
            ),
            offset: 0,
            max_bytes: 4096,
            credential: None,
            headers: vec![],
        });
        self.pending = Some(Pending::Result(id.to_owned()));
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
        let pending = self.pending.take();
        match outcome {
            TaskOutcome::Completed(bytes) => {
                let raw = String::from_utf8_lossy(&bytes).into_owned();
                match pending {
                    Some(Pending::Poll) => {
                        if let Some(deck) = decode(&raw) {
                            self.notice = deck.error.clone();
                            self.deck = deck;
                            self.page = self.page.min(self.deck.pages.len().saturating_sub(1));
                            cx.store().cache(CACHE, raw);
                        }
                        self.poll(cx);
                    }
                    Some(Pending::Press(id)) => {
                        let outcome = kobo_json::parse(&raw)
                            .ok()
                            .and_then(|value| {
                                value
                                    .get("outcome")
                                    .and_then(kobo_json::Value::as_str)
                                    .map(str::to_owned)
                            })
                            .unwrap_or_else(|| "gone".to_owned());
                        match outcome.as_str() {
                            "started" => self.notice = None,
                            "needs-confirm" => self.confirming = Some(id),
                            "busy" => self.notice = Some("That command is still running.".into()),
                            _ => {
                                self.notice =
                                    Some("That control changed. Refreshing the deck.".into());
                            }
                        }
                        self.poll(cx);
                    }
                    Some(Pending::Result(id)) => {
                        if let Some(result) = decode_result(&raw) {
                            let label = self
                                .deck
                                .pages
                                .iter()
                                .flat_map(|page| page.keys.iter())
                                .find(|key| key.id == id)
                                .map_or_else(|| "Command".to_owned(), |key| key.label.clone());
                            self.result = Some((label, result));
                            self.view = View::Result;
                        } else {
                            self.notice = Some("That result is no longer available.".into());
                        }
                    }
                    None => {}
                }
            }
            TaskOutcome::Failed(_) => {
                if let Some(Pending::Press(id)) = pending {
                    if let Some(key) = self
                        .deck
                        .pages
                        .iter_mut()
                        .flat_map(|page| page.keys.iter_mut())
                        .find(|key| key.id == id)
                    {
                        "idle".clone_into(&mut key.state);
                    }
                }
                self.notice =
                    Some("Can't reach your computer. Check that Sidekick is open.".into());
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
        if a == action_id("retry") {
            self.notice = None;
            self.poll(cx);
            self.show(cx);
            return;
        }
        if a == action_id("back") {
            self.view = View::Grid;
            self.result = None;
            self.poll(cx);
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
                    if key.state == "running" {
                        self.notice = Some("That command is still running.".into());
                    } else if key.state == "failed" {
                        let id = key.id.clone();
                        self.result(cx, &id);
                    } else if key.confirm {
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

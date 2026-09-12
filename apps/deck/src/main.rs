//! A command deck: fifteen places for the things you do on your computer
//! most often, on a panel that is already on the desk.
//!
//! The reader owns nothing here except which key to press. What each key runs
//! is decided on the computer with `kobo deck set`, pushed to the reader, and
//! run by the computer that owns the shell: a deck that could invent its own
//! commands would be a remote shell with a friendly face on it.
//!
//! ## What the panel has to say
//!
//! Which layout is showing, whether the computer is answering, and what the
//! last thing pressed did. All three are things the reader cannot find out any
//! other way: the computer is in another room, and a key that has been pressed
//! looks exactly like a key that has not.

mod model;
use kobo_sdk::keyboard::{TextEntry, Typing};
use kobo_sdk::{
    action_id, cache_key, ActionId, BannerLevel, Context, Glyph, KoboApp, Screen, ScreenBuilder,
    StoreResult, Task, TaskId, TaskOutcome,
};
use model::{decode, decode_result, pad_cells, Deck, RunResult};
use std::process::ExitCode;
const PAIRED: &str = "paired";
const CACHE: &str = "deck-cache";
#[derive(Clone, Copy, Debug, PartialEq)]
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
    /// Waiting between one look at the computer and the next.
    Wait,
    Press(String),
    Result(String),
}

/// How long to leave the computer alone between looks.
///
/// The deck used to ask again the moment an answer landed, which on a device
/// whose radio is the largest draw on the battery is a way to flatten a charge
/// while nothing at all is happening: three hundred and seventy requests went
/// out in the ten seconds it took to notice. Long enough to be kind, short
/// enough that a key somebody pressed on the computer shows up here while they
/// are still looking at it.
const BETWEEN_LOOKS: u32 = 5;
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
    /// Whether the computer answered the last thing it was asked.
    ///
    /// A deck whose computer has gone away looks exactly like a deck whose
    /// computer is idle, and the difference is the whole question a reader has
    /// when a key does nothing.
    reachable: bool,
    /// What the last key that finished did, for the line under the deck.
    ///
    /// Kept beside the full result rather than instead of it: this is the one
    /// line the grid can show, and the result screen is where the output is.
    last: Option<String>,
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
            reachable: true,
            last: None,
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
            // The explanation and the keyboard are two screens, not one. A
            // raised keyboard takes the bottom half of the panel, so a
            // heading and a paragraph above it fit at the default text size
            // and are refused outright at 170%, which left a reader who had
            // turned the type up with a blank panel and no way to pair.
            View::Address if self.entry.is_open() => ScreenBuilder::new("deck-address")
                .text_entry(&self.entry, "Computer address", "Next")
                .build(),
            View::Address => ScreenBuilder::new("deck-address")
                .top_bar("Deck")
                .heading("Pair with your computer")
                .text("Start Sidekick on your computer. It shows an address and a code.")
                .primary_button("enter-address", "Enter the address")
                .build(),
            View::Code if self.entry.is_open() => ScreenBuilder::new("deck-code")
                .text_entry(&self.entry, "Pairing code", "Pair")
                .build(),
            View::Code => ScreenBuilder::new("deck-code")
                .top_bar("Deck")
                .heading("Now the pairing code")
                .text("The six characters Sidekick is showing beside the address.")
                .primary_button("enter-code", "Enter the code")
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
        // The bar says which deck this is rather than that it is a deck. With
        // one word on it, a reader with a Build page and a Home page had to
        // read the keys to find out which one they were looking at.
        let mut screen = ScreenBuilder::new("deck-grid").top_bar(page.name.clone());
        if self.deck.pages.len() > 1 {
            let tabs = self
                .deck
                .pages
                .iter()
                .map(|p| (format!("page-{}", p.name), p.name.clone()))
                .collect::<Vec<_>>();
            screen = screen.tabs(self.page, tabs);
        }
        if self.address == "local" {
            screen = screen.top_bar_action("pair-preview", "Pair");
        } else {
            screen = screen.top_bar_glyph("retry", "Refresh", Glyph::Refresh);
        }
        if let Some(note) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, note);
        }
        // Who this deck is talking to, said once, above the keys. It is the
        // difference between a key that did nothing and a computer that is no
        // longer listening, which is not a difference a reader can see in a
        // grid of squares.
        screen = screen.secondary(self.connection());
        screen = screen.pads(pad_cells(page));
        if let Some(last) = &self.last {
            // What the last key that finished did. The whole output is on the
            // result screen; this is the line that says there is one.
            screen = screen.secondary(last.clone());
        }
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
    /// What this deck is connected to, as a sentence.
    fn connection(&self) -> String {
        if self.address == "local" {
            return "Preview only. Pair to run commands.".to_owned();
        }
        if self.address.is_empty() {
            return "Not paired with a computer yet.".to_owned();
        }
        if self.reachable {
            format!("Paired with {}.", self.address)
        } else {
            format!("{} is not answering.", self.address)
        }
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
    /// Leaves the computer alone for a moment, then looks again.
    fn wait(&mut self, cx: &mut Context) {
        if self.address == "local" || self.task.is_some() {
            return;
        }
        self.task = cx.spawn(Task::Sleep {
            seconds: BETWEEN_LOOKS,
        });
        self.pending = Some(Pending::Wait);
    }

    fn poll(&mut self, cx: &mut Context) {
        if self.address == "local" {
            return;
        }
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
        if self.address == "local" {
            self.notice = Some("Preview only. Pair with your computer to use these pads.".into());
            return;
        }
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
                    }
                } else {
                    self.view = View::Address;
                }
            } else if key == cache_key(CACHE) {
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
                self.reachable = true;
                let raw = String::from_utf8_lossy(&bytes).into_owned();
                match pending {
                    Some(Pending::Poll) => {
                        if let Some(deck) = decode(&raw) {
                            self.notice = deck.error.clone();
                            self.deck = deck;
                            self.page = self.page.min(self.deck.pages.len().saturating_sub(1));
                            cx.store().cache(CACHE, raw);
                        }
                        self.wait(cx);
                    }
                    Some(Pending::Wait) => self.poll(cx),
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
                            self.last = Some(summarise(&label, &result));
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
                self.reachable = false;
                // Still worth looking again, but not immediately: a computer
                // that is not there answers a request a second just as fast
                // as it answers none.
                if matches!(pending, Some(Pending::Poll | Pending::Wait)) {
                    self.wait(cx);
                }
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
        if a == action_id("pair-preview") && self.address == "local" {
            self.view = View::Address;
            self.notice = None;
            self.show(cx);
            return;
        }
        if matches!(self.view, View::Address | View::Code) {
            if let Some(event) = self.entry.handle(a) {
                if let Typing::Submitted(text) = event {
                    if self.view == View::Address {
                        self.address = text;
                        self.view = View::Code;
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
        if a == action_id("enter-address") || a == action_id("enter-code") {
            self.entry.open();
            self.show(cx);
            return;
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
                    } else if key.state == "failed" || key.state == "ok" {
                        // A key that has already run opens what it said. It
                        // used to do that only when it had failed, so the
                        // output of everything that worked was unreachable.
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
/// One line about what a command did, for the deck to carry under its keys.
fn summarise(label: &str, result: &RunResult) -> String {
    match (result.status.as_str(), result.exit) {
        ("running", _) => format!("{label} is still running."),
        ("ok", Some(0) | None) => format!("{label} finished."),
        ("ok", Some(exit)) => format!("{label} finished, exit {exit}."),
        (_, Some(exit)) => format!("{label} failed, exit {exit}. Tap it to read the output."),
        _ => format!("{label} failed. Tap it to read the output."),
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

#[cfg(test)]
mod tests {
    use super::{summarise, App, Pending, View, BETWEEN_LOOKS, PAIRED};
    use crate::model::{decode, RunResult};
    use kobo_sdk::{action_id, AppRunner, Command, StoreResult, Task, TaskId, TaskOutcome};

    const LAYOUT: &str = r#"{"version":"2","pages":[{"name":"Build","keys":[
        {"id":"test","label":"Test","detail":"cargo test","confirm":false,"state":"idle"},
        {"id":"deploy","label":"Deploy","detail":"ship it","confirm":true,"state":"idle"}]}]}"#;

    #[test]
    fn static_preview_does_not_dispatch_a_command_or_claim_it_is_running() {
        let mut app = App {
            address: "local".into(),
            ..App::default()
        };
        let mut context = kobo_sdk::Context::default();
        app.press(&mut context, "fixture-pad", false);
        assert!(context.take_commands().is_empty());
        assert!(app.task.is_none());
        assert!(app.pending.is_none());
        assert!(app.notice.as_deref().unwrap().contains("Preview only"));
    }

    /// A deck paired with a computer, with the layout that computer sent.
    fn paired() -> (AppRunner<App>, Vec<Command>) {
        let mut runner = AppRunner::new(App::default());
        runner.start();
        runner.store_result(StoreResult::Loaded {
            key: PAIRED.into(),
            value: Some(b"192.168.1.5:8080|code".to_vec()),
        });
        let task = runner.app().task.expect("the deck asked for its keys");
        let commands =
            runner.task_outcome(task, TaskOutcome::Completed(LAYOUT.as_bytes().to_vec()));
        (runner, commands)
    }

    /// What a batch of commands started, if it started anything.
    fn started(commands: &[Command]) -> Option<Task> {
        commands.iter().rev().find_map(|command| match command {
            Command::Spawn { work, .. } => Some(work.clone()),
            _ => None,
        })
    }

    /// The identifier of whatever is in flight.
    fn in_flight(runner: &AppRunner<App>) -> Option<TaskId> {
        runner.app().task
    }

    #[test]
    fn the_deck_says_which_computer_it_is_talking_to_and_whether_it_is_answering() {
        let (runner, _) = paired();
        let said = runner.app().connection();
        assert!(said.contains("192.168.1.5:8080"), "{said}");
        assert!(said.starts_with("Paired"), "{said}");

        let unreachable = App {
            address: "192.168.1.5:8080".into(),
            reachable: false,
            ..App::default()
        };
        assert!(unreachable.connection().contains("not answering"));

        let local = App {
            address: "local".into(),
            ..App::default()
        };
        assert!(local.connection().contains("Preview only"));
        assert!(App::default().connection().contains("Not paired"));
    }

    #[test]
    fn the_deck_waits_between_looks_rather_than_asking_again_at_once() {
        // It used to ask the moment an answer landed: three hundred and
        // seventy requests went out in the ten seconds it took to notice,
        // which on a device whose radio is the largest draw on the battery is
        // a way to flatten a charge while nothing is happening.
        let (runner, commands) = paired();
        let waiting = started(&commands).expect("something was started after the deck arrived");
        assert_eq!(
            waiting,
            Task::Sleep {
                seconds: BETWEEN_LOOKS
            },
            "the deck asked the computer again immediately"
        );
        assert_eq!(runner.app().pending, Some(Pending::Wait));
    }

    #[test]
    fn a_key_that_asks_first_does_not_run_until_it_is_answered() {
        let (mut runner, _) = paired();
        let asked = runner.action(action_id("press-deploy"));
        assert_eq!(runner.app().confirming.as_deref(), Some("deploy"));
        assert!(
            !matches!(started(&asked), Some(Task::Post { .. })),
            "a command that asks first was sent before it was answered"
        );
        let cancelled = runner.action(action_id("cancel-run"));
        assert_eq!(runner.app().confirming, None);
        assert!(
            !matches!(started(&cancelled), Some(Task::Post { .. })),
            "cancelling ran it anyway"
        );

        runner.action(action_id("press-deploy"));
        let confirmed = runner.action(action_id("confirm-run"));
        assert!(
            matches!(started(&confirmed), Some(Task::Post { .. })),
            "answering the question did not run it"
        );
    }

    #[test]
    fn a_key_that_has_run_opens_what_it_said() {
        // It used to do that only when the command had failed, so everything
        // that worked said nothing at all.
        let (mut runner, _) = paired();
        for key in runner
            .app_mut()
            .deck
            .pages
            .iter_mut()
            .flat_map(|page| page.keys.iter_mut())
        {
            "ok".clone_into(&mut key.state);
        }
        runner.action(action_id("press-test"));
        let task = in_flight(&runner).expect("the deck asked what the command said");
        runner.task_outcome(
            task,
            TaskOutcome::Completed(
                br#"{"status":"ok","exit":0,"tail":"14 tests passed"}"#.to_vec(),
            ),
        );
        assert_eq!(runner.app().view, View::Result);
        assert_eq!(runner.app().last.as_deref(), Some("Test finished."));
        let drawn = format!("{:?}", runner.app().screen());
        assert!(drawn.contains("14 tests passed"), "{drawn}");
    }

    #[test]
    fn what_a_command_did_is_said_in_one_line() {
        let ran = RunResult {
            status: "ok".into(),
            exit: Some(0),
            tail: String::new(),
        };
        assert_eq!(summarise("Test", &ran), "Test finished.");
        let failed = RunResult {
            status: "failed".into(),
            exit: Some(7),
            tail: String::new(),
        };
        assert!(summarise("Deploy", &failed).contains("exit 7"));
        assert!(summarise("Deploy", &failed).contains("read the output"));
    }

    #[test]
    fn the_deck_the_computer_sent_is_the_deck_that_is_drawn() {
        let deck = decode(LAYOUT).expect("layout");
        assert_eq!(deck.pages[0].keys.len(), 2);
        let app = App {
            deck,
            address: "192.168.1.5:8080".into(),
            view: View::Grid,
            ..App::default()
        };
        let drawn = format!("{:?}", app.screen());
        assert!(
            drawn.contains("Test") && drawn.contains("Deploy"),
            "{drawn}"
        );
        // The rest of the deck is places for keys nobody has assigned, and
        // there are as many of them as the panel draws.
        assert_eq!(
            drawn.matches("label: \"\"").count(),
            crate::model::PAD_COUNT - 2,
            "{drawn}"
        );
    }
}

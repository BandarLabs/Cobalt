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
    StoreResult, Task, TaskError, TaskId, TaskOutcome,
};
use model::PAD_COLUMNS;
use model::{decode, decode_result, pad_cells, Deck, RunResult};
use std::process::ExitCode;
const PAIRED: &str = "paired";
const CACHE: &str = "deck-cache";

/// The port `kobo sidekick run` listens on for the reader. Filled in when an
/// address is typed without one, because the address the computer prints is
/// the part people copy and the port is the part they leave off: a bare
/// address used to be dialled on the HTTPS default and never connected.
const DEFAULT_PORT: &str = "9331";

/// How many characters `kobo sidekick init` puts in a pairing code.
const CODE_LENGTH: usize = 6;

/// Why the computer did not give the deck what it asked for.
///
/// One cause, one sentence. The deck used to say "Check that Sidekick is
/// open" for everything, which sent somebody with a stale pairing code to
/// restart a helper that was running perfectly well.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Trouble {
    /// This reader has no network.
    Offline,
    /// Nothing answered at the address, or the answer could not be trusted.
    Unreachable,
    /// The computer answered too slowly.
    Slow,
    /// The computer answered, and refused the pairing code.
    Refused,
    /// The computer answered, and has no deck.toml.
    NoDeck,
    /// Anything else the computer said that the deck could not use.
    Unreadable,
}

impl Trouble {
    const fn of(error: TaskError) -> Self {
        match error {
            TaskError::Offline => Self::Offline,
            TaskError::Unreachable => Self::Unreachable,
            TaskError::TimedOut => Self::Slow,
            TaskError::Unauthorized => Self::Refused,
            TaskError::NotFound => Self::NoDeck,
            _ => Self::Unreadable,
        }
    }

    /// The banner: what happened and what fixes it.
    const fn advice(self) -> &'static str {
        match self {
            Self::Offline => "This reader is not on Wi-Fi. Join a network in Cobalt Settings, then tap Refresh.",
            Self::Unreachable => "Your computer is not answering. Start kobo sidekick run on it. After a first pairing, reopen Cobalt once.",
            Self::Slow => "Your computer took too long to answer. It may be asleep. Wake it, then tap Refresh.",
            Self::Refused => "Your computer did not accept this pairing code. Run kobo sidekick init on it, or tap Pairing to enter the code again.",
            Self::NoDeck => NO_DECK,
            Self::Unreadable => "Deck could not use your computer's answer. Tap Refresh to try again.",
        }
    }
}

/// Said both for a failed look at the deck and for a press the computer
/// could not place, because they are the same missing file.
const NO_DECK: &str =
    "Your computer has no deck yet. In your project folder on it, run kobo deck init --preset build.";
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
    /// Reading what a watched key said, only for the line under the deck.
    Note(String),
}

/// Said when the runtime has no room for one more request just now.
const STILL_TALKING: &str = "Deck is still waiting on your computer. Tap again in a moment.";

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
    /// The key this reader started, until its finish is acknowledged.
    watching: Option<String>,
    result: Option<(String, RunResult)>,
    /// The key whose result is on screen, so it can be run again from there.
    result_key: Option<String>,
    result_page: usize,
    /// Why the computer did not answer the last thing it was asked, if it
    /// did not.
    ///
    /// A deck whose computer has gone away looks exactly like a deck whose
    /// computer is idle, and the difference is the whole question a reader has
    /// when a key does nothing.
    trouble: Option<Trouble>,
    /// The pairing in use before Change pairing was chosen, so backing out of
    /// the typing returns to a deck that still works.
    previous: Option<(String, String)>,
    /// Why the last typed address or code was not accepted.
    entry_error: Option<&'static str>,
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
            watching: None,
            result: None,
            result_key: None,
            result_page: 0,
            trouble: None,
            previous: None,
            entry_error: None,
            last: None,
        }
    }
}
impl App {
    fn show(&self, cx: &mut Context) {
        cx.set_screen(self.screen(cx));
    }
    fn screen(&self, context: &Context) -> Screen {
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
            View::Address => {
                let mut screen = ScreenBuilder::new("deck-address")
                    .top_bar("Deck")
                    .owns_back(self.previous.is_some())
                    .heading("Pair with your computer");
                if let Some(error) = self.entry_error {
                    screen = screen.banner(BannerLevel::Attention, error);
                }
                screen
                    .text("On your computer, run these two.")
                    .command("kobo sidekick init")
                    .command("kobo sidekick run")
                    .secondary("If the first cannot reach this reader, enter the address and code it prints.")
                    .primary_button("enter-address", "Enter the address")
                    .build()
            }
            View::Code if self.entry.is_open() => ScreenBuilder::new("deck-code")
                .text_entry(&self.entry, "Pairing code", "Pair")
                .build(),
            View::Code => {
                let mut screen = ScreenBuilder::new("deck-code")
                    .top_bar("Deck")
                    .owns_back(true)
                    .heading("Now the pairing code");
                if let Some(error) = self.entry_error {
                    screen = screen.banner(BannerLevel::Attention, error);
                }
                screen
                    .text("The six characters kobo sidekick init printed beside the address.")
                    .primary_button("enter-code", "Enter the code")
                    .build()
            }
            View::Grid => self.grid(),
            View::Result => self.result_screen(context),
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
            // A pairing that has gone wrong needs a way out from the deck
            // itself, not only from a fresh install.
            screen = screen.top_bar_action("repair", "Pairing").top_bar_glyph(
                "retry",
                "Refresh",
                Glyph::Refresh,
            );
        }
        if let Some(note) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, note);
        }
        // Who this deck is talking to, said once, above the keys. It is the
        // difference between a key that did nothing and a computer that is no
        // longer listening, which is not a difference a reader can see in a
        // grid of squares.
        screen = screen.secondary(self.connection());
        // A five-column board rather than the pads shortcut: the shortcut
        // backfills every unassigned place with a blank key, and a reader
        // with three commands saw a wall of ruled boxes that do nothing.
        screen = screen.board_with_selection(PAD_COLUMNS, pad_cells(page));
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
        match self.trouble {
            None => format!("Paired with {}.", self.address),
            Some(Trouble::Offline) => "This reader is not on Wi-Fi.".to_owned(),
            Some(Trouble::Unreachable) => format!("{} is not answering.", self.address),
            Some(Trouble::Slow) => format!("{} is slow to answer.", self.address),
            Some(Trouble::Refused) => format!("{} refused this pairing.", self.address),
            Some(Trouble::NoDeck) => format!("{} has no deck yet.", self.address),
            Some(Trouble::Unreadable) => {
                format!("{} sent an answer Deck cannot use.", self.address)
            }
        }
    }

    /// The address as typed, cleaned, with the helper's port filled in when
    /// none was given. `None` when it could not be an address.
    fn accept_address(typed: &str) -> Option<String> {
        let typed: String = typed.split_whitespace().collect();
        let typed = typed
            .strip_prefix("https://")
            .unwrap_or(&typed)
            .trim_end_matches('/')
            .to_owned();
        if typed.is_empty()
            || !typed
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']'))
        {
            return None;
        }
        let has_port = if typed.starts_with('[') {
            typed.contains("]:")
        } else {
            typed.contains(':')
        };
        Some(if has_port {
            typed
        } else {
            format!("{typed}:{DEFAULT_PORT}")
        })
    }

    /// The code as typed, if it is one `kobo sidekick init` could have made.
    fn accept_code(typed: &str) -> Option<String> {
        let code = typed.trim().to_ascii_lowercase();
        (code.chars().count() == CODE_LENGTH && code.chars().all(|c| c.is_ascii_alphanumeric()))
            .then_some(code)
    }

    fn result_prefix(&self) -> ScreenBuilder {
        let screen = ScreenBuilder::new("deck-result")
            .top_bar("Deck")
            .owns_back(true);
        let Some((label, result)) = &self.result else {
            return screen.heading("No result");
        };
        let status = match (result.status.as_str(), result.exit) {
            ("running", _) => "Still running".to_owned(),
            ("ok", Some(exit)) => format!("Finished · exit {exit}"),
            ("failed", Some(exit)) => format!("Failed · exit {exit}"),
            ("ok", None) => "Finished".to_owned(),
            _ => "Failed".to_owned(),
        };
        screen.heading(label).secondary(status)
    }

    fn result_pages(&self, context: &Context) -> Vec<Vec<String>> {
        let Some((_, result)) = &self.result else {
            return Vec::new();
        };
        let text = if result.tail.is_empty() {
            "This command produced no output."
        } else {
            &result.tail
        };
        let paragraphs: Vec<_> = text
            .split('\n')
            .map(|paragraph| (0, 0, kobo_sdk::QuoteRole::Body, paragraph))
            .collect();
        context
            .paginate_tagged_under(&paragraphs, false, &self.result_prefix().build())
            .into_iter()
            .map(|page| page.into_iter().map(|(_, _, _, text)| text).collect())
            .collect()
    }

    fn result_screen(&self, context: &Context) -> Screen {
        let pages = self.result_pages(context);
        let page = self.result_page.min(pages.len().saturating_sub(1));
        // The heading and exit status occupy the first page; continuation
        // pages keep the command name in chrome, matching the SDK paginator.
        let mut screen = if page == 0 {
            self.result_prefix()
        } else {
            ScreenBuilder::new("deck-result")
                .top_bar(
                    self.result
                        .as_ref()
                        .map_or("Deck", |(label, _)| label.as_str()),
                )
                .owns_back(true)
        };
        if let Some(paragraphs) = pages.get(page) {
            for paragraph in paragraphs {
                screen = screen.text(paragraph);
            }
        }
        // Tapping a key that has just run opens what it said, so this is the
        // way to run it again from here rather than waiting out the result.
        if self.result_key.is_some() && self.address != "local" {
            screen = screen.button("run-again", "Run again");
        }
        if pages.len() > 1 {
            screen = screen
                .page_turns("result-previous", "result-next")
                .page_position(
                    u16::try_from(page + 1).unwrap_or(u16::MAX),
                    u16::try_from(pages.len()).unwrap_or(u16::MAX),
                );
        }
        screen.build()
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
        let task = cx.spawn(Task::Post {
            url: format!("https://{}/deck/press?token={}", self.address, self.code),
            body: format!("{{\"key\":\"{id}\",\"confirmed\":{confirmed}}}"),
            content_type: "application/json".into(),
            credential: None,
            headers: vec![],
            max_bytes: 4096,
        });
        // A tap either goes out or says why it did not. The key used to turn
        // to "running" and stay that way when the runtime had no room for
        // another request, with nothing sent at all.
        let Some(task) = task else {
            self.notice = Some(STILL_TALKING.into());
            return;
        };
        if let Some(key) = self
            .deck
            .pages
            .iter_mut()
            .flat_map(|page| page.keys.iter_mut())
            .find(|key| key.id == id)
        {
            "running".clone_into(&mut key.state);
        }
        self.notice = None;
        self.task = Some(task);
        self.pending = Some(Pending::Press(id.to_owned()));
    }
    fn result(&mut self, cx: &mut Context, id: &str) {
        let task = cx.spawn(Task::Fetch {
            url: format!(
                "https://{}/deck/result?key={id}&token={}",
                self.address, self.code
            ),
            offset: 0,
            max_bytes: 4096,
            credential: None,
            headers: vec![],
        });
        if task.is_none() {
            self.notice = Some(STILL_TALKING.into());
            return;
        }
        self.task = task;
        self.pending = Some(Pending::Result(id.to_owned()));
    }

    /// The pairing screens' taps, and Pairing in the deck's top bar.
    fn pairing_action(&mut self, cx: &mut Context, a: ActionId) -> bool {
        if matches!(self.view, View::Address | View::Code) {
            if let Some(event) = self.entry.handle(a) {
                if let Typing::Submitted(text) = event {
                    self.submitted(cx, &text);
                }
                self.show(cx);
                return true;
            }
            if a == ActionId::BACK {
                // Back steps out of the typing: code to address, and address
                // to the deck that was working before Pairing was chosen.
                if self.view == View::Code {
                    self.view = View::Address;
                } else if let Some((address, code)) = self.previous.take() {
                    self.address = address;
                    self.code = code;
                    self.view = View::Grid;
                    self.poll(cx);
                }
                self.entry_error = None;
                self.show(cx);
                return true;
            }
        }
        if a == action_id("enter-address") {
            // Opened with what is known, because the usual repair is a new
            // code for the same computer.
            let known = self
                .previous
                .as_ref()
                .map(|(address, _)| address.clone())
                .unwrap_or_default();
            self.entry.open_with(known);
            self.show(cx);
            return true;
        }
        if a == action_id("enter-code") {
            self.entry.open();
            self.show(cx);
            return true;
        }
        if a == action_id("repair") && self.view == View::Grid {
            self.change_pairing(cx);
            return true;
        }
        false
    }

    /// Run again under a result: the same press the key would have made,
    /// asking first when the key asks first.
    fn run_again(&mut self, cx: &mut Context, a: ActionId) -> bool {
        if a == action_id("run-again") && self.view == View::Result {
            if let Some(id) = self.result_key.take() {
                self.view = View::Grid;
                self.result = None;
                self.result_page = 0;
                let confirm = self
                    .deck
                    .pages
                    .iter()
                    .flat_map(|page| page.keys.iter())
                    .find(|key| key.id == id)
                    .map(|key| key.confirm);
                match confirm {
                    Some(true) => self.confirming = Some(id),
                    Some(false) => self.press(cx, &id, false),
                    None => {
                        self.notice = Some(
                            "That pad changed on your computer. The deck is refreshing.".into(),
                        );
                        self.poll(cx);
                    }
                }
            }
            self.show(cx);
            return true;
        }
        false
    }

    /// Starts typing a new pairing, keeping the one in use to come back to.
    fn change_pairing(&mut self, cx: &mut Context) {
        if !self.address.is_empty() && self.address != "local" {
            self.previous = Some((self.address.clone(), self.code.clone()));
        }
        self.entry.close();
        self.entry_error = None;
        self.notice = None;
        self.view = View::Address;
        self.show(cx);
    }

    /// Takes a typed address or code. A refused one stays on its own step with
    /// the reason above it, instead of being saved and failing later as an
    /// unreachable computer.
    fn submitted(&mut self, cx: &mut Context, text: &str) {
        if self.view == View::Address {
            if let Some(address) = Self::accept_address(text) {
                self.address = address;
                self.entry_error = None;
                self.view = View::Code;
            } else {
                self.entry_error = Some("That is not an address. Enter the one kobo sidekick init printed, like 192.168.1.20.");
            }
            return;
        }
        let Some(code) = Self::accept_code(text) else {
            self.entry_error = Some("A pairing code is six letters and digits. Enter the one kobo sidekick init printed.");
            return;
        };
        self.code = code;
        self.entry_error = None;
        self.previous = None;
        self.trouble = None;
        self.notice = None;
        // What the last key did on the old pairing says nothing about this one.
        self.last = None;
        cx.store()
            .save(PAIRED, format!("{}|{}", self.address, self.code));
        self.view = View::Grid;
        // Whatever was in flight belongs to the old pairing.
        self.task = None;
        self.pending = None;
        self.poll(cx);
    }

    fn poll_landed(&mut self, cx: &mut Context, raw: &str) {
        if let Some(deck) = decode(raw) {
            self.notice = deck.error.clone();
            self.deck = deck;
            self.page = self.page.min(self.deck.pages.len().saturating_sub(1));
            cx.store().cache(CACHE, raw);
        }
        // A key this reader started has finished: read what it
        // said so the line under the deck acknowledges it,
        // without dragging the reader to the result screen.
        if let Some(watched) = self.watching.take() {
            let finished = self
                .deck
                .pages
                .iter()
                .flat_map(|page| page.keys.iter())
                .any(|key| key.id == watched && (key.state == "ok" || key.state == "failed"));
            if finished {
                self.task = cx.spawn(Task::Fetch {
                    url: format!(
                        "https://{}/deck/result?key={watched}&token={}",
                        self.address, self.code
                    ),
                    offset: 0,
                    max_bytes: 4096,
                    credential: None,
                    headers: vec![],
                });
                self.pending = Some(Pending::Note(watched));
            } else {
                self.watching = Some(watched);
            }
        }
        self.wait(cx);
    }

    fn note_landed(&mut self, cx: &mut Context, id: &str, raw: &str) {
        if let Some(result) = decode_result(raw) {
            let label = self
                .deck
                .pages
                .iter()
                .flat_map(|page| page.keys.iter())
                .find(|key| key.id == id)
                .map_or_else(|| "Command".to_owned(), |key| key.label.clone());
            self.last = Some(summarise(&label, &result));
        }
        self.wait(cx);
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
                // Both halves are trimmed and a bare address gets the port,
                // so a pairing saved by an older Deck, or written by the
                // computer, opens the same way a freshly typed one does.
                let remembered = value
                    .and_then(|v| String::from_utf8(v).ok())
                    .and_then(|raw| {
                        let (address, code) = raw.split_once('|')?;
                        let code = code.trim();
                        if address.trim() == "local" {
                            return Some(("local".to_owned(), code.to_owned()));
                        }
                        Some((Self::accept_address(address)?, code.to_owned()))
                    })
                    .filter(|(_, code)| !code.is_empty());
                if let Some((address, code)) = remembered {
                    self.address = address;
                    self.code = code;
                    self.view = View::Grid;
                    self.poll(cx);
                } else if self.view == View::Opening {
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
    fn on_foreground(&mut self, cx: &mut Context) {
        match self.view {
            // A look that was cancelled while the panel was elsewhere would
            // otherwise leave a deck that never updates again.
            View::Grid if self.task.is_none() => self.poll(cx),
            // `kobo sidekick init` may have paired this reader while Deck
            // sat on its pairing screen. Nothing is lost by looking.
            View::Address if self.previous.is_none() && !self.entry.is_open() => {
                cx.store().load(PAIRED);
            }
            _ => {}
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
                if self.trouble.take().is_some() {
                    self.notice = None;
                }
                let raw = String::from_utf8_lossy(&bytes).into_owned();
                match pending {
                    Some(Pending::Poll) => {
                        self.poll_landed(cx, &raw);
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
                            "started" => {
                                self.notice = None;
                                self.watching = Some(id);
                            }
                            "needs-confirm" => self.confirming = Some(id),
                            "busy" => self.notice = Some("That command is still running.".into()),
                            "full" => {
                                self.notice = Some(
                                    "Four commands are already running. Tap again when one finishes."
                                        .into(),
                                );
                            }
                            "unavailable" => self.notice = Some(NO_DECK.into()),
                            _ => {
                                self.notice = Some(
                                    "That pad changed on your computer. The deck is refreshing."
                                        .into(),
                                );
                            }
                        }
                        self.poll(cx);
                    }
                    Some(Pending::Note(id)) => {
                        self.note_landed(cx, &id, &raw);
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
                            self.result_key = Some(id);
                            self.result_page = 0;
                            self.view = View::Result;
                        } else {
                            self.notice = Some("That result is no longer available.".into());
                        }
                    }
                    None => {}
                }
            }
            TaskOutcome::Failed(error) => {
                let trouble = Trouble::of(error);
                self.trouble = Some(trouble);
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
                self.notice = Some(trouble.advice().into());
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
        if self.pairing_action(cx, a) || self.run_again(cx, a) {
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
        if self.view == View::Result
            && (a == action_id("result-previous") || a == action_id("result-next"))
        {
            let last = self.result_pages(cx).len().saturating_sub(1);
            let current = self.result_page.min(last);
            self.result_page = if a == action_id("result-next") {
                current.saturating_add(1).min(last)
            } else {
                current.saturating_sub(1)
            };
            self.show(cx);
            return;
        }
        if a == action_id("back") || (a == ActionId::BACK && self.view == View::Result) {
            self.view = View::Grid;
            self.result = None;
            self.result_key = None;
            self.result_page = 0;
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
    use kobo_sdk::{
        action_id, ActionId, AppRunner, Command, StoreResult, Task, TaskError, TaskId, TaskOutcome,
    };

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
            trouble: Some(super::Trouble::Unreachable),
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
    fn a_started_key_is_acknowledged_under_the_deck_when_it_finishes() {
        let (mut runner, _) = paired();
        runner.action(action_id("press-test"));
        let post = in_flight(&runner).expect("the press went out");
        runner.task_outcome(
            post,
            TaskOutcome::Completed(br#"{"outcome":"started"}"#.to_vec()),
        );
        assert_eq!(runner.app().watching.as_deref(), Some("test"));
        let poll = in_flight(&runner).expect("the deck looked again after the start");
        let finished = r#"{"version":"3","pages":[{"name":"Build","keys":[
            {"id":"test","label":"Test","detail":"cargo test","confirm":false,"state":"ok"},
            {"id":"deploy","label":"Deploy","detail":"ship it","confirm":true,"state":"idle"}]}]}"#;
        let commands =
            runner.task_outcome(poll, TaskOutcome::Completed(finished.as_bytes().to_vec()));
        let note = started(&commands).expect("the deck asked what the key said");
        match note {
            Task::Fetch { url, .. } => assert!(url.contains("/deck/result?key=test"), "{url}"),
            other => panic!("expected a result fetch, started {other:?}"),
        }
        assert_eq!(runner.app().pending, Some(Pending::Note("test".into())));
        let note_task = in_flight(&runner).expect("the result read is in flight");
        runner.task_outcome(
            note_task,
            TaskOutcome::Completed(
                br#"{"status":"ok","exit":0,"tail":"deck companion check passed"}"#.to_vec(),
            ),
        );
        assert_eq!(runner.app().last.as_deref(), Some("Test finished."));
        assert_eq!(
            runner.app().view,
            View::Grid,
            "the note must not drag the reader off the grid"
        );
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
        let drawn = format!("{:?}", runner.app().screen(&kobo_sdk::Context::default()));
        assert!(drawn.contains("14 tests passed"), "{drawn}");
    }

    #[test]
    fn a_bare_address_is_dialled_on_the_helpers_port() {
        assert_eq!(
            App::accept_address(" 192.168.1.20 ").as_deref(),
            Some("192.168.1.20:9331")
        );
        assert_eq!(
            App::accept_address("192.168.1.20:9400").as_deref(),
            Some("192.168.1.20:9400")
        );
        assert_eq!(
            App::accept_address("https://studio.local/").as_deref(),
            Some("studio.local:9331")
        );
        assert_eq!(
            App::accept_address("[fe80::1]").as_deref(),
            Some("[fe80::1]:9331")
        );
        assert_eq!(App::accept_address("not an/address"), None);
        assert_eq!(App::accept_code(" ABC234 ").as_deref(), Some("abc234"));
        assert_eq!(App::accept_code("abc"), None);
    }

    #[test]
    fn a_pairing_saved_without_a_port_still_connects() {
        let mut runner = AppRunner::new(App::default());
        runner.start();
        let commands = runner.store_result(StoreResult::Loaded {
            key: PAIRED.into(),
            value: Some(b"192.168.1.5|abc234\n".to_vec()),
        });
        match started(&commands) {
            Some(Task::Fetch { url, .. }) => {
                assert!(url.starts_with("https://192.168.1.5:9331/deck?"), "{url}");
                assert!(url.ends_with("token=abc234"), "{url}");
            }
            other => panic!("expected the deck to be fetched, started {other:?}"),
        }
    }

    #[test]
    fn each_failure_names_its_own_cause_and_fix() {
        let (mut runner, _) = paired();
        let cases = [
            (TaskError::Unauthorized, "pairing code", "refused"),
            (TaskError::NotFound, "kobo deck init", "no deck"),
            (TaskError::Unreachable, "kobo sidekick run", "not answering"),
            (TaskError::Offline, "Wi-Fi", "not on Wi-Fi"),
            (TaskError::TimedOut, "asleep", "slow"),
        ];
        let mut seen = Vec::new();
        for (error, advice, line) in cases {
            runner.action(action_id("retry"));
            let task = in_flight(&runner).expect("a look is in flight");
            runner.task_outcome(task, TaskOutcome::Failed(error));
            let notice = runner.app().notice.clone().expect("a notice");
            assert!(notice.contains(advice), "{error:?}: {notice}");
            assert!(runner.app().connection().contains(line), "{error:?}");
            assert!(!seen.contains(&notice), "two causes share {notice}");
            seen.push(notice);
        }
        // A pairing problem is never blamed on the network or the address.
        assert!(!seen[0].contains("Wi-Fi") && !seen[0].contains("not answering"));
    }

    #[test]
    fn change_pairing_starts_from_the_deck_and_back_returns_to_it() {
        let (mut runner, _) = paired();
        runner.action(action_id("repair"));
        assert_eq!(runner.app().view, View::Address);
        runner.action(action_id("enter-address"));
        assert_eq!(runner.app().entry.text(), "192.168.1.5:8080");
        runner.cancel_text();
        runner.action(ActionId::BACK);
        assert_eq!(runner.app().view, View::Grid);
        assert_eq!(runner.app().code, "code");
    }

    #[test]
    fn a_new_code_typed_after_change_pairing_is_saved_and_used() {
        let (mut runner, _) = paired();
        runner.action(action_id("repair"));
        runner
            .app_mut()
            .submitted(&mut kobo_sdk::Context::default(), "192.168.1.9");
        assert_eq!(runner.app().view, View::Code);
        let mut context = kobo_sdk::Context::default();
        runner.app_mut().submitted(&mut context, "zz");
        assert!(runner.app().entry_error.is_some(), "a short code was taken");
        runner.app_mut().submitted(&mut context, "NEW234");
        assert_eq!(runner.app().view, View::Grid);
        let saved = context.take_commands().into_iter().any(|command| {
            matches!(
                command,
                Command::Store(kobo_sdk::StoreRequest::Save { key, value })
                    if key == PAIRED && value == b"192.168.1.9:9331|new234"
            )
        });
        assert!(saved, "the new pairing was not saved");
    }

    #[test]
    fn a_finished_key_can_be_run_again_from_its_result() {
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
        let task = in_flight(&runner).expect("the result read");
        runner.task_outcome(
            task,
            TaskOutcome::Completed(br#"{"status":"ok","exit":0,"tail":"done"}"#.to_vec()),
        );
        assert_eq!(runner.app().view, View::Result);
        let drawn = format!("{:?}", runner.app().screen(&kobo_sdk::Context::default()));
        assert!(drawn.contains("Run again"), "{drawn}");
        let commands = runner.action(action_id("run-again"));
        match started(&commands) {
            Some(Task::Post { url, body, .. }) => {
                assert!(url.contains("/deck/press"), "{url}");
                assert!(body.contains("\"confirmed\":false"), "{body}");
            }
            other => panic!("Run again did not press the key: {other:?}"),
        }
        assert_eq!(runner.app().view, View::Grid);
    }

    #[test]
    fn a_full_computer_is_told_apart_from_a_busy_key() {
        let (mut runner, _) = paired();
        runner.action(action_id("press-test"));
        let post = in_flight(&runner).expect("the press went out");
        runner.task_outcome(
            post,
            TaskOutcome::Completed(br#"{"outcome":"full"}"#.to_vec()),
        );
        let notice = runner.app().notice.clone().unwrap_or_default();
        assert!(notice.contains("Four commands"), "{notice}");
    }

    #[test]
    fn pairing_screens_fit_the_panel_at_every_text_size() {
        use kobo_ui::{Chrome, DisplayMetrics, TextScale, CLARA_BW_METRICS};
        for text_scale in TextScale::STEPS {
            let metrics = DisplayMetrics {
                text_scale,
                ..CLARA_BW_METRICS
            };
            // Built through a runner, as on a reader. The runner installs the
            // face the runtime measures with; without one, a run that reached
            // this test first measured in the bitmap fallback and passed a
            // screen that ran off the panel in the real face.
            let runner = kobo_sdk::AppRunner::with_metrics(App::default(), metrics);
            let context = runner.context();
            for (view, error) in [
                (View::Address, None),
                (View::Address, Some("That is not an address. Enter the one kobo sidekick init printed, like 192.168.1.20.")),
                (View::Code, Some("A pairing code is six letters and digits. Enter the one kobo sidekick init printed.")),
            ] {
                let app = App {
                    view,
                    entry_error: error,
                    previous: Some(("192.168.1.5:9331".into(), "abc234".into())),
                    ..App::default()
                };
                let screen = app.screen(&context);
                let errors: Vec<_> = screen
                    .diagnostics(&metrics, &Chrome::measuring(true))
                    .issues
                    .into_iter()
                    .filter(|issue| issue.severity == kobo_sdk::DiagnosticSeverity::Error)
                    .collect();
                assert!(errors.is_empty(), "{view:?} {text_scale:?}: {errors:#?}");
            }
        }
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
        let drawn = format!("{:?}", app.screen(&kobo_sdk::Context::default()));
        assert!(
            drawn.contains("Test") && drawn.contains("Deploy"),
            "{drawn}"
        );
        // Places nobody has assigned stay as paper: only the keys the
        // computer sent are drawn.
        assert_eq!(drawn.matches("label: \"\"").count(), 0, "{drawn}");
    }
}

#[cfg(test)]
mod ui_review_tests;

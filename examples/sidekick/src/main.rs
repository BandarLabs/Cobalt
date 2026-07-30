//! The permission prompt, moved to the armchair.
//!
//! A coding agent on the desk stops to ask "may I run this?" and the asking
//! goes wherever this reader is. The sidekick daemon on the computer catches
//! the question through the agent's own hook system and holds it; this
//! application collects it over one long-polled fetch, prints the command in
//! full, and offers exactly three answers under a thumb: Allow, Deny, and
//! leave it for the terminal.
//!
//! The design rule is that the panel earns its repaints. An empty poll asks
//! again without drawing anything; a question repaints once and then the
//! panel holds it at zero power for as long as the decision takes, which is
//! the one thing this screen does better than the phone it replaces.
//!
//! Pairing is typed once and remembered: the daemon's address, then the
//! six-character code `kobo-sidekickd init` printed. The code rides every
//! request so nobody else on the network can watch the questions or answer
//! them, and the connection is TLS against a root the owner installed with
//! `kobo trust set`.

use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Failure, Glyph, KoboApp, Screen, ScreenBuilder,
    Space, StoreResult, Task, TaskId, TaskOutcome,
};
use std::process::ExitCode;

const TITLE: &str = "Sidekick";
/// Where the address and code are remembered between sessions.
const PAIRED: &str = "paired";

const ALLOW: &str = "allow";
const DENY: &str = "deny";
const IGNORE: &str = "pass";
const REPAIR: &str = "repair";

/// The port `kobo-sidekick run` listens on, filled in when the owner types a
/// bare address, so the common case is typing one thing instead of two.
const DEFAULT_PORT: &str = "9331";

/// How long the daemon holds an empty poll before answering "nothing yet".
/// Well under the runtime's own request ceiling, so a quiet afternoon is a
/// steady heartbeat of short requests rather than a stack of timeouts.
const POLL_WAIT: &str = "25";

/// A question is a command line and change; a reply is smaller.
const MAX_REPLY: u32 = 16 * 1024;

/// How long to sleep after a failed poll before trying again. Long enough
/// not to spin on a dead network, short enough that the daemon coming back
/// is noticed before anyone walks over to check.
const NAP_SECONDS: u32 = 10;

/// The most characters of a command drawn on the panel. Enough to read
/// almost any real command whole; past this the reader should be at the
/// terminal anyway, and the tail is marked rather than silently missing.
const MAX_DETAIL: usize = 600;

/// One question, as the daemon sent it.
#[derive(Clone, Debug, PartialEq)]
struct Ask {
    id: u32,
    source: String,
    tool: String,
    detail: String,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum View {
    /// Waiting for the store to say whether a pairing exists.
    #[default]
    Opening,
    /// Typing the daemon's address.
    Address,
    /// Typing the pairing code.
    Code,
    /// Paired, polling, nothing to decide.
    Watching,
    /// A question is on the panel.
    Asking,
    /// An answer is on its way to the daemon.
    Sending,
}

#[derive(Default)]
struct Sidekick {
    view: View,
    keyboard: Keyboard,
    /// The daemon, as `host:port`.
    address: String,
    /// The pairing code, sent as the token on every request.
    code: String,
    /// The question on the panel, while there is one.
    ask: Option<Ask>,
    poll: Option<TaskId>,
    answer: Option<TaskId>,
    nap: Option<TaskId>,
    /// The last decision, stated on the watching screen so a glance says
    /// the tap counted even after the question has left the panel.
    last: Option<String>,
    trouble: Option<String>,
}

impl Sidekick {
    fn show(&self, context: &mut Context) {
        // Back retreats one step inside the flow -- code to address, and a
        // question to "leave it for the terminal" -- rather than leaving.
        let owns_back = matches!(self.view, View::Code | View::Asking);
        context.set_screen(self.screen().with_own_back(owns_back));
    }

    fn screen(&self) -> Screen {
        match self.view {
            View::Opening => ScreenBuilder::new("sidekick-opening")
                .top_bar(TITLE)
                .activity("Opening", None)
                .build(),
            View::Address => self.address_screen(),
            View::Code => self.code_screen(),
            View::Watching => self.watching(),
            View::Asking => self.asking(),
            View::Sending => ScreenBuilder::new("sidekick-sending")
                .top_bar(TITLE)
                .activity("Sending your answer", None)
                .build(),
        }
    }

    /// Step one of pairing: where the daemon is.
    fn address_screen(&self) -> Screen {
        let mut screen = ScreenBuilder::new("sidekick-address")
            .top_bar(TITLE)
            .heading("Pair with your computer")
            .text(
                "Run kobo-sidekickd init there, then type the address it \
                 prints. The port is filled in if you leave it off.",
            );
        if let Some(trouble) = &self.trouble {
            screen = screen.banner(BannerLevel::Attention, trouble.clone());
        }
        screen
            .typed(&self.keyboard, "192.168.1.20:9331")
            .spacer(Space::Small)
            .keyboard(&self.keyboard, "Next")
            .build()
    }

    /// Step two: the code that proves the reader is the owner's.
    fn code_screen(&self) -> Screen {
        let mut screen = ScreenBuilder::new("sidekick-code")
            .top_bar(TITLE)
            .heading("Now the pairing code")
            .text(format!(
                "The six characters kobo-sidekickd init printed beside \
                 {}. They keep anyone else on the network from answering \
                 for you.",
                self.address
            ));
        if let Some(trouble) = &self.trouble {
            screen = screen.banner(BannerLevel::Attention, trouble.clone());
        }
        screen
            .typed(&self.keyboard, "the pairing code")
            .spacer(Space::Small)
            .keyboard(&self.keyboard, "Pair")
            .build()
    }

    /// Paired and quiet. Painted when the state changes, never per poll.
    fn watching(&self) -> Screen {
        let mut screen = ScreenBuilder::new("sidekick-watching")
            .top_bar(TITLE)
            .splash(
                Some(Glyph::Circle),
                "Watching",
                "Questions from your coding agents appear here the moment \
                 they ask.",
            );
        if let Some(trouble) = &self.trouble {
            screen = screen.banner(BannerLevel::Attention, trouble.clone());
        }
        if let Some(last) = &self.last {
            screen = screen.section("Last answer").text(last.clone());
        }
        screen
            .section("Paired with")
            .text(self.address.clone())
            .spacer(Space::Small)
            .button(REPAIR, "Change pairing")
            .build()
    }

    /// The question, whole, over three answers.
    fn asking(&self) -> Screen {
        let Some(ask) = &self.ask else {
            return self.watching();
        };
        let mut screen = ScreenBuilder::new("sidekick-asking")
            .top_bar(TITLE)
            .heading(format!("{} asks", agent_name(&ask.source)))
            .byline(0, ask.tool.clone())
            .quote(0, trimmed(&ask.detail));
        if let Some(trouble) = &self.trouble {
            screen = screen.banner(BannerLevel::Attention, trouble.clone());
        }
        screen
            .spacer(Space::Small)
            .button(ALLOW, "Allow")
            .button(DENY, "Deny")
            .button(IGNORE, "Leave it for the terminal")
            .build()
    }

    /// Starts the next long poll. One in flight at a time, always.
    fn poll(&mut self, context: &mut Context) {
        if self.poll.is_some() {
            return;
        }
        let url = format!(
            "https://{}/pending?token={}&wait={POLL_WAIT}",
            self.address, self.code
        );
        self.poll = context.spawn(Task::Fetch {
            url,
            offset: 0,
            max_bytes: MAX_REPLY,
        });
    }

    /// Sends the tapped decision back to the daemon.
    fn decide(&mut self, context: &mut Context, choice: &str) {
        let Some(ask) = &self.ask else {
            return;
        };
        let body = kobo_json::ObjectBuilder::new()
            .set("token", self.code.as_str())
            .set("id", ask.id)
            .set("choice", choice)
            .build()
            .to_json();
        let work = Task::Post {
            url: format!("https://{}/answer", self.address),
            body,
            content_type: "application/json".to_owned(),
            credential: None,
            headers: Vec::new(),
            max_bytes: MAX_REPLY,
        };
        if let Some(task) = context.spawn(work) {
            self.answer = Some(task);
            self.last = Some(format!(
                "{} {} for {}.",
                decided(choice),
                trimmed_to(&ask.detail, 60),
                agent_name(&ask.source)
            ));
            self.view = View::Sending;
            self.trouble = None;
            self.show(context);
        } else {
            self.trouble = Some("Still sending the last answer.".to_owned());
            self.show(context);
        }
    }

    /// What came back from a poll.
    fn on_poll(&mut self, context: &mut Context, outcome: TaskOutcome) {
        if self.view != View::Watching {
            // The pairing screens are up mid-poll. Nothing is shown over the
            // typing and nothing spins the loop; a question stays queued on
            // its daemon, for the next poll of whatever pairing wins.
            return;
        }
        match outcome {
            TaskOutcome::Completed(bytes) => {
                let repaint = self.trouble.take().is_some();
                if let Some(ask) = read_ask(&bytes) {
                    self.ask = Some(ask);
                    self.view = View::Asking;
                    self.show(context);
                    // Deliberately no next poll: the daemon queues anything
                    // else that arrives until this question is decided.
                    return;
                }
                if repaint {
                    self.show(context);
                }
                self.poll(context);
            }
            TaskOutcome::Failed(error) => {
                self.trouble = Some(Failure::of(error).advice.to_owned());
                self.show(context);
                self.nap = context.spawn(Task::Sleep {
                    seconds: NAP_SECONDS,
                });
            }
            // The runtime withdrew the task; on_foreground restarts the loop.
            TaskOutcome::Cancelled => {}
        }
    }

    /// What came back from posting an answer.
    fn on_answer(&mut self, context: &mut Context, outcome: &TaskOutcome) {
        match outcome {
            TaskOutcome::Completed(bytes) => {
                if !answer_landed(bytes) {
                    // The daemon said no: the question was gone -- timed out
                    // or already collected -- before the tap arrived. Saying
                    // "Allowed" now would be claiming a decision nobody got.
                    self.last = Some("That question was gone before the answer arrived.".into());
                }
                self.ask = None;
                self.view = View::Watching;
                self.trouble = None;
                self.show(context);
                self.poll(context);
            }
            TaskOutcome::Failed(error) => {
                // The question is still on the daemon's board, so it comes
                // back to the panel with the three answers intact.
                self.view = View::Asking;
                self.last = None;
                self.trouble = Some(Failure::of(*error).advice.to_owned());
                self.show(context);
            }
            TaskOutcome::Cancelled => {
                self.view = View::Asking;
                self.last = None;
                self.show(context);
            }
        }
    }

    /// A submitted address, cleaned and given the default port if none was
    /// typed. `None` means it could not be an address, with the reason left
    /// in `trouble`.
    fn accept_address(&mut self, typed: &str) -> Option<String> {
        let typed: String = typed.split_whitespace().collect();
        if typed.is_empty() {
            return None;
        }
        let address = if typed.contains(':') {
            typed
        } else {
            format!("{typed}:{DEFAULT_PORT}")
        };
        let plausible = address
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']'));
        if plausible {
            self.trouble = None;
            Some(address)
        } else {
            self.trouble = Some("That does not look like host:port.".to_owned());
            None
        }
    }

    /// Handles a tap while a keyboard is up. Returns whether it was one.
    fn typing(&mut self, context: &mut Context, action: ActionId) -> bool {
        let Some(pressed) = self.keyboard.press(action) else {
            return false;
        };
        match pressed {
            Pressed::Edited | Pressed::Shifted => self.show(context),
            Pressed::Submitted => match self.view {
                View::Address => {
                    let typed = self.keyboard.text().to_owned();
                    if let Some(address) = self.accept_address(&typed) {
                        self.address = address;
                        self.keyboard.clear();
                        self.view = View::Code;
                    }
                    self.show(context);
                }
                View::Code => {
                    let code = self.keyboard.text().trim().to_owned();
                    if code.is_empty() {
                        return true;
                    }
                    self.code = code;
                    self.keyboard.clear();
                    let record = format!("{}\n{}", self.address, self.code);
                    context.store().save(PAIRED, record.into_bytes());
                    self.view = View::Watching;
                    self.trouble = None;
                    // A poll still in flight belongs to the old pairing.
                    // Forgetting its id makes whatever it brings back land on
                    // nothing, and clears the way for this pairing's poll.
                    self.poll = None;
                    self.show(context);
                    self.poll(context);
                }
                _ => {}
            },
        }
        true
    }
}

/// The agent's name as a person says it, not as its process does.
fn agent_name(source: &str) -> &str {
    match source {
        "codex" => "Codex",
        "claude" => "Claude Code",
        other => other,
    }
}

/// The verb for the watching screen's "last answer" line.
fn decided(choice: &str) -> &'static str {
    match choice {
        ALLOW => "Allowed",
        DENY => "Denied",
        _ => "Left at the terminal:",
    }
}

fn trimmed(detail: &str) -> String {
    trimmed_to(detail, MAX_DETAIL)
}

/// The front of a long command, with the cut marked. Counted in characters
/// rather than bytes so the mark never splits one in half.
fn trimmed_to(detail: &str, most: usize) -> String {
    if detail.chars().count() <= most {
        return detail.to_owned();
    }
    let mut kept: String = detail.chars().take(most).collect();
    kept.push('…');
    kept
}

/// Whether the daemon said the tap landed on a live question. Anything but
/// a clear yes is a no: an unreadable reply gets the same caution.
fn answer_landed(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|text| kobo_json::parse(text).ok())
        .and_then(|body| body.get("ok").and_then(kobo_json::Value::as_bool))
        == Some(true)
}

/// The question in a poll's body, if the body carries one.
fn read_ask(bytes: &[u8]) -> Option<Ask> {
    let text = std::str::from_utf8(bytes).ok()?;
    let body = kobo_json::parse(text).ok()?;
    let ask = body.get("ask")?;
    let id = u32::try_from(ask.get("id").and_then(kobo_json::Value::as_i64)?).ok()?;
    let field = |name: &str| {
        ask.get(name)
            .and_then(kobo_json::Value::as_str)
            .unwrap_or("")
            .to_owned()
    };
    Some(Ask {
        id,
        source: field("source"),
        tool: field("tool"),
        detail: field("detail"),
    })
}

impl KoboApp for Sidekick {
    fn on_start(&mut self, context: &mut Context) {
        context.store().load(PAIRED);
        self.show(context);
    }

    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        let StoreResult::Loaded { key, value } = result else {
            return;
        };
        if key != PAIRED || self.view != View::Opening {
            return;
        }
        let remembered = value
            .as_deref()
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
            .and_then(|text| {
                let (address, code) = text.split_once('\n')?;
                Some((address.to_owned(), code.to_owned()))
            });
        if let Some((address, code)) = remembered {
            self.address = address;
            self.code = code;
            self.view = View::Watching;
            self.show(context);
            self.poll(context);
        } else {
            self.view = View::Address;
            self.show(context);
        }
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if action == ActionId::BACK {
            match self.view {
                // A question dismissed is a question left for the terminal,
                // said out loud rather than left dangling on the daemon.
                View::Asking => self.decide(context, IGNORE),
                View::Code => {
                    self.keyboard = Keyboard::with_text(&self.address);
                    self.view = View::Address;
                    self.show(context);
                }
                _ => {}
            }
            return;
        }
        if matches!(self.view, View::Address | View::Code) && self.typing(context, action) {
            return;
        }
        if action == action_id(REPAIR) && self.view == View::Watching {
            self.keyboard = Keyboard::with_text(&self.address);
            self.view = View::Address;
            self.show(context);
            return;
        }
        if self.view == View::Asking {
            for choice in [ALLOW, DENY, IGNORE] {
                if action == action_id(choice) {
                    self.decide(context, choice);
                    return;
                }
            }
        }
    }

    fn on_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
        if self.poll == Some(task) {
            self.poll = None;
            self.on_poll(context, outcome);
        } else if self.answer == Some(task) {
            self.answer = None;
            self.on_answer(context, &outcome);
        } else if self.nap == Some(task) {
            self.nap = None;
            if self.view == View::Watching {
                self.poll(context);
            }
        }
    }

    fn on_foreground(&mut self, context: &mut Context) {
        // Whatever was in flight may have been cancelled while the panel was
        // elsewhere; a watching screen with no poll is a dead remote.
        if self.view == View::Watching && self.poll.is_none() && self.nap.is_none() {
            self.poll(context);
        }
    }
}

fn main() -> ExitCode {
    match kobo_sdk::run("sidekick", Sidekick::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("sidekick: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Sidekick, View, ALLOW, DENY, IGNORE, PAIRED, REPAIR};
    use kobo_sdk::keyboard::Keyboard;
    use kobo_sdk::{
        action_id, ActionId, Command, Context, KoboApp, Screen, StoreRequest, StoreResult, Task,
        TaskId, TaskOutcome,
    };
    use kobo_ui::{Chrome, CLARA_BW_METRICS};

    fn act(app: &mut Sidekick, action: ActionId) -> Vec<Command> {
        let mut context = Context::default();
        app.on_action(&mut context, action);
        context.take_commands()
    }

    /// A sidekick already paired and watching, with its first poll in flight.
    fn paired() -> (Sidekick, TaskId) {
        let mut app = Sidekick::default();
        let mut context = Context::default();
        app.on_start(&mut context);
        let _ = context.take_commands();
        app.on_store(
            &mut context,
            StoreResult::Loaded {
                key: PAIRED.to_owned(),
                value: Some(b"192.168.1.5:9331\nabc123".to_vec()),
            },
        );
        let poll = fetched(&context.take_commands()).expect("a poll starts").0;
        (app, poll)
    }

    /// The one `Task::Fetch` in a batch, as `(task, url)`.
    fn fetched(commands: &[Command]) -> Option<(TaskId, String)> {
        commands.iter().find_map(|command| match command {
            Command::Spawn {
                task,
                work: Task::Fetch { url, .. },
            } => Some((*task, url.clone())),
            _ => None,
        })
    }

    /// The one `Task::Post` in a batch, as `(task, url, body)`.
    fn posted(commands: &[Command]) -> Option<(TaskId, String, String)> {
        commands.iter().find_map(|command| match command {
            Command::Spawn {
                task,
                work: Task::Post { url, body, .. },
            } => Some((*task, url.clone(), body.clone())),
            _ => None,
        })
    }

    fn slept(commands: &[Command]) -> Option<TaskId> {
        commands.iter().find_map(|command| match command {
            Command::Spawn {
                task,
                work: Task::Sleep { .. },
            } => Some(*task),
            _ => None,
        })
    }

    fn painted(commands: &[Command]) -> Option<Screen> {
        commands.iter().rev().find_map(|command| match command {
            Command::SetScreen(screen) => Some(screen.clone()),
            _ => None,
        })
    }

    /// Every line of text the panel would show, in order.
    fn shown(screen: &Screen) -> Vec<String> {
        screen
            .layout_with(&CLARA_BW_METRICS, &Chrome::default())
            .nodes
            .iter()
            .flat_map(|node| node.text_lines.clone())
            .collect()
    }

    fn question(id: u32, detail: &str) -> TaskOutcome {
        TaskOutcome::Completed(
            format!(
                r#"{{"ask":{{"id":{id},"source":"codex","tool":"shell","detail":"{detail}"}}}}"#
            )
            .into_bytes(),
        )
    }

    #[test]
    fn a_first_run_asks_for_the_address_and_touches_no_network() {
        let mut app = Sidekick::default();
        let mut context = Context::default();
        app.on_start(&mut context);
        let _ = context.take_commands();
        app.on_store(
            &mut context,
            StoreResult::Loaded {
                key: PAIRED.to_owned(),
                value: None,
            },
        );
        let commands = context.take_commands();
        assert!(fetched(&commands).is_none(), "polled before pairing");
        let lines = shown(&painted(&commands).expect("a screen"));
        assert!(
            lines
                .iter()
                .any(|line| line.contains("kobo-sidekickd init")),
            "the screen never says where the address comes from: {lines:?}"
        );
    }

    #[test]
    fn a_remembered_pairing_goes_straight_to_watching_and_polls_with_its_token() {
        let mut app = Sidekick::default();
        let mut context = Context::default();
        app.on_start(&mut context);
        let _ = context.take_commands();
        app.on_store(
            &mut context,
            StoreResult::Loaded {
                key: PAIRED.to_owned(),
                value: Some(b"192.168.1.5:9331\nabc123".to_vec()),
            },
        );
        let commands = context.take_commands();
        let (_, url) = fetched(&commands).expect("watching starts a poll");
        assert_eq!(url, "https://192.168.1.5:9331/pending?token=abc123&wait=25");
        assert_eq!(app.view, View::Watching);
    }

    #[test]
    fn typing_the_address_and_code_saves_the_pairing_and_starts_watching() {
        let mut app = Sidekick::default();
        let mut context = Context::default();
        app.on_start(&mut context);
        app.on_store(
            &mut context,
            StoreResult::Loaded {
                key: PAIRED.to_owned(),
                value: None,
            },
        );
        let _ = context.take_commands();
        // A bare host gets the daemon's port; the reader types one thing.
        app.keyboard = Keyboard::with_text("192.168.1.9");
        act(&mut app, action_id("kb.enter"));
        assert_eq!(app.view, View::Code);
        app.keyboard = Keyboard::with_text("qk3mzp");
        let mut context = Context::default();
        app.on_action(&mut context, action_id("kb.enter"));
        let commands = context.take_commands();
        let saved = commands.iter().find_map(|command| match command {
            Command::Store(StoreRequest::Save { key, value }) => Some((key.clone(), value.clone())),
            _ => None,
        });
        assert_eq!(
            saved,
            Some((PAIRED.to_owned(), b"192.168.1.9:9331\nqk3mzp".to_vec()))
        );
        let (_, url) = fetched(&commands).expect("pairing starts the first poll");
        assert!(url.starts_with("https://192.168.1.9:9331/pending?token=qk3mzp"));
    }

    #[test]
    fn an_address_that_could_not_be_one_is_refused_with_a_reason() {
        let mut app = Sidekick {
            view: View::Address,
            keyboard: Keyboard::with_text("what even is this?"),
            ..Sidekick::default()
        };
        let commands = act(&mut app, action_id("kb.enter"));
        assert_eq!(app.view, View::Address, "a nonsense address moved on");
        let lines = shown(&painted(&commands).expect("a repaint with the reason"));
        assert!(
            lines.iter().any(|line| line.contains("host:port")),
            "no reason shown: {lines:?}"
        );
    }

    #[test]
    fn a_question_off_the_wire_paints_the_command_and_three_answers() {
        let (mut app, poll) = paired();
        let mut context = Context::default();
        app.on_task(&mut context, poll, question(4, "cargo test --workspace"));
        let commands = context.take_commands();
        assert!(
            fetched(&commands).is_none(),
            "kept polling with a question on the panel"
        );
        let screen = painted(&commands).expect("the question was painted");
        let lines = shown(&screen);
        assert!(
            lines.iter().any(|line| line.contains("cargo test")),
            "the command is not on the panel: {lines:?}"
        );
        assert!(lines.iter().any(|line| line.contains("Codex")), "{lines:?}");
        for label in ["Allow", "Deny", "Leave it for the terminal"] {
            assert!(
                lines.iter().any(|line| line.contains(label)),
                "no {label} on the panel: {lines:?}"
            );
        }
    }

    #[test]
    fn an_empty_poll_asks_again_without_repainting() {
        let (mut app, poll) = paired();
        let mut context = Context::default();
        app.on_task(&mut context, poll, TaskOutcome::Completed(b"{}".to_vec()));
        let commands = context.take_commands();
        assert!(fetched(&commands).is_some(), "the loop stopped");
        assert!(
            painted(&commands).is_none(),
            "an empty poll repainted an unchanged screen"
        );
    }

    #[test]
    fn allowing_posts_the_tap_and_watching_resumes_naming_the_outcome() {
        let (mut app, poll) = paired();
        let mut context = Context::default();
        app.on_task(&mut context, poll, question(4, "cargo test"));
        let _ = context.take_commands();
        let commands = act(&mut app, action_id(ALLOW));
        let (task, url, body) = posted(&commands).expect("the answer was sent");
        assert_eq!(url, "https://192.168.1.5:9331/answer");
        assert!(body.contains(r#""id":4"#), "{body}");
        assert!(body.contains(r#""choice":"allow""#), "{body}");
        assert!(body.contains(r#""token":"abc123""#), "{body}");
        let mut context = Context::default();
        app.on_task(
            &mut context,
            task,
            TaskOutcome::Completed(br#"{"ok":true}"#.to_vec()),
        );
        let commands = context.take_commands();
        assert!(fetched(&commands).is_some(), "watching never resumed");
        let lines = shown(&painted(&commands).expect("back to watching"));
        assert!(
            lines
                .iter()
                .any(|line| line.contains("Allowed") && line.contains("cargo test")),
            "the outcome is not stated: {lines:?}"
        );
    }

    #[test]
    fn back_on_a_question_leaves_it_for_the_terminal() {
        let (mut app, poll) = paired();
        let mut context = Context::default();
        app.on_task(&mut context, poll, question(7, "rm -rf ./build"));
        let _ = context.take_commands();
        let commands = act(&mut app, ActionId::BACK);
        let (_, _, body) = posted(&commands).expect("dismissal still answers");
        assert!(body.contains(r#""choice":"pass""#), "{body}");
    }

    #[test]
    fn a_dead_daemon_is_named_once_and_polling_resumes_after_a_nap() {
        let (mut app, poll) = paired();
        let mut context = Context::default();
        app.on_task(
            &mut context,
            poll,
            TaskOutcome::Failed(kobo_sdk::TaskError::Unreachable),
        );
        let commands = context.take_commands();
        let advice = kobo_sdk::Failure::of(kobo_sdk::TaskError::Unreachable).advice;
        let lines = shown(&painted(&commands).expect("the trouble was stated"));
        assert!(
            lines.iter().any(|line| line.contains(advice)),
            "no advice on the panel: {lines:?}"
        );
        let nap = slept(&commands).expect("no retry was scheduled");
        assert!(fetched(&commands).is_none(), "retried without the nap");
        let mut context = Context::default();
        app.on_task(&mut context, nap, TaskOutcome::Completed(Vec::new()));
        let commands = context.take_commands();
        assert!(fetched(&commands).is_some(), "the nap never woke the loop");
        // The poll that then succeeds takes the banner down with one repaint.
        let poll = fetched(&commands).expect("polling again").0;
        let mut context = Context::default();
        app.on_task(&mut context, poll, TaskOutcome::Completed(b"{}".to_vec()));
        let commands = context.take_commands();
        let lines = shown(&painted(&commands).expect("the recovery repaints once"));
        assert!(
            !lines.iter().any(|line| line.contains(advice)),
            "the trouble outlived it: {lines:?}"
        );
    }

    #[test]
    fn a_failed_answer_puts_the_question_back_with_the_reason() {
        let (mut app, poll) = paired();
        let mut context = Context::default();
        app.on_task(&mut context, poll, question(4, "cargo build"));
        let _ = context.take_commands();
        let commands = act(&mut app, action_id(DENY));
        let (task, _, _) = posted(&commands).expect("the answer was sent");
        let mut context = Context::default();
        app.on_task(
            &mut context,
            task,
            TaskOutcome::Failed(kobo_sdk::TaskError::Unreachable),
        );
        let lines = shown(&painted(&context.take_commands()).expect("a repaint"));
        assert!(
            lines.iter().any(|line| line.contains("cargo build")),
            "the question left the panel with nobody told: {lines:?}"
        );
        assert!(
            lines.iter().any(|line| line.contains("Deny")),
            "the answers left with it: {lines:?}"
        );
    }

    #[test]
    fn changing_the_pairing_starts_from_the_current_address() {
        let (mut app, _) = paired();
        let commands = act(&mut app, action_id(REPAIR));
        assert_eq!(app.view, View::Address);
        let lines = shown(&painted(&commands).expect("the pairing screen"));
        assert!(
            lines.iter().any(|line| line.contains("192.168.1.5:9331")),
            "the address must be edited from scratch: {lines:?}"
        );
    }

    #[test]
    fn a_command_too_long_for_the_panel_is_cut_and_the_cut_is_marked() {
        let long = "x".repeat(2000);
        assert_eq!(super::trimmed(&long).chars().count(), super::MAX_DETAIL + 1);
        assert!(super::trimmed(&long).ends_with('…'));
        assert_eq!(super::trimmed("cargo test"), "cargo test");
    }

    #[test]
    fn an_answer_the_daemon_rejects_is_not_reported_as_decided() {
        let (mut app, poll) = paired();
        let mut context = Context::default();
        app.on_task(&mut context, poll, question(4, "cargo publish"));
        let _ = context.take_commands();
        let commands = act(&mut app, action_id(ALLOW));
        let (task, _, _) = posted(&commands).expect("the answer was sent");
        let mut context = Context::default();
        app.on_task(
            &mut context,
            task,
            TaskOutcome::Completed(br#"{"ok":false}"#.to_vec()),
        );
        let commands = context.take_commands();
        assert!(fetched(&commands).is_some(), "watching never resumed");
        let lines = shown(&painted(&commands).expect("a repaint"));
        assert!(
            !lines.iter().any(|line| line.contains("Allowed")),
            "claimed a decision that never landed: {lines:?}"
        );
        assert!(
            lines.iter().any(|line| line.contains("gone before")),
            "the miss is not stated: {lines:?}"
        );
    }

    #[test]
    fn a_question_arriving_mid_repair_does_not_take_the_typing_screen() {
        let (mut app, poll) = paired();
        let _ = act(&mut app, action_id(REPAIR));
        assert_eq!(app.view, View::Address);
        let mut context = Context::default();
        app.on_task(&mut context, poll, question(6, "cargo run"));
        let commands = context.take_commands();
        assert_eq!(app.view, View::Address, "a question took the typing screen");
        assert!(fetched(&commands).is_none(), "polled while re-pairing");
    }

    #[test]
    fn a_poll_from_the_old_pairing_cannot_answer_into_the_new() {
        let (mut app, old_poll) = paired();
        let _ = act(&mut app, action_id(REPAIR));
        app.keyboard = Keyboard::with_text("192.168.1.77");
        let _ = act(&mut app, action_id("kb.enter"));
        app.keyboard = Keyboard::with_text("zzzzzz");
        let mut context = Context::default();
        // Real task numbers never repeat; a fresh test context restarts
        // them, so spend a few keeping the new poll's id off the old one's.
        for _ in 0..3 {
            let _ = context.spawn(Task::Sleep { seconds: 1 });
        }
        app.on_action(&mut context, action_id("kb.enter"));
        let commands = context.take_commands();
        let (new_poll, url) = fetched(&commands).expect("the new pairing polls");
        assert_ne!(new_poll, old_poll, "the old poll still speaks");
        assert!(url.contains("192.168.1.77"), "{url}");
        assert!(url.contains("token=zzzzzz"), "{url}");
        // The old poll comes back bearing a question: it lands on nothing.
        let mut context = Context::default();
        app.on_task(&mut context, old_poll, question(4, "rm -rf /"));
        let commands = context.take_commands();
        assert_eq!(app.view, View::Watching, "a stale poll took the panel");
        assert!(app.ask.is_none(), "a stale question was kept");
        assert!(fetched(&commands).is_none(), "a stale poll spun the loop");
    }

    #[test]
    fn ignoring_a_question_says_so_without_claiming_a_decision() {
        let (mut app, poll) = paired();
        let mut context = Context::default();
        app.on_task(&mut context, poll, question(9, "npx create-react-app"));
        let _ = context.take_commands();
        let commands = act(&mut app, action_id(IGNORE));
        let (task, _, body) = posted(&commands).expect("ignoring still answers");
        assert!(body.contains(r#""choice":"pass""#), "{body}");
        let mut context = Context::default();
        app.on_task(
            &mut context,
            task,
            TaskOutcome::Completed(br#"{"ok":true}"#.to_vec()),
        );
        let lines = shown(&painted(&context.take_commands()).expect("watching again"));
        assert!(
            lines
                .iter()
                .any(|line| line.contains("Left at the terminal")),
            "{lines:?}"
        );
    }
}

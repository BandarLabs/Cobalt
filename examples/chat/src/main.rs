//! A chat client for a device with no keyboard worth the name.
//!
//! Three screens and one rule. The rule is that the reader should have to
//! type as little as possible: typing here means hunting for keys on a panel
//! that takes tens of milliseconds a repaint, so the model is asked to offer
//! tappable answers wherever a question genuinely has them, and those answers
//! are drawn with the same [`ScreenBuilder::choose`] a native screen would
//! use. It is asked just as firmly not to do that every turn, because a
//! conversation that answers every remark with a menu is a form.
//!
//! ## The key
//!
//! This application never sees it. [`Task::Post`] carries the *name* of a
//! secret; the runtime resolves that against its own directory and attaches
//! the `Authorization` header itself. Nothing here reads it, holds it, logs
//! it, or could put it in a crash dump, and a test asserts the request body
//! contains nothing key-shaped.
//!
//! ## Why nothing moves
//!
//! There is no spinner and no animation, here or anywhere in this system.
//! Waiting is stated once with [`ScreenBuilder::activity`] and the panel then
//! holds that image at zero power until there is something new to say.

mod conversation;

use conversation::{Conversation, Provider, Reply, Role, Turn, PROVIDERS};
use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Glyph, KoboApp, LogLevel, Screen, ScreenBuilder,
    Space, StoreResult, Task, TaskError, TaskId, TaskOutcome,
};
use std::process::ExitCode;

/// Roughly how many characters of body text fit on one line of the panel.
///
/// The renderer wraps text itself, so this is not used to break lines. It is
/// used to guess how tall a message will be before it is drawn, which is what
/// decides how much of the transcript is kept. A guess is enough: being a few
/// characters out shows one message more or fewer, and measuring properly
/// would mean laying the screen out twice on every repaint.
const COLUMNS: usize = 48;

/// How many lines of transcript are drawn before older turns are dropped.
///
/// Conservative on purpose. The panel holds far more lines than this with the
/// built-in bitmap type, and roughly this many with the real typeface the
/// runtime installs, which is the one the reader will be looking at. Nothing
/// scrolls on an E Ink panel, so a transcript that overflows is not a
/// transcript the reader can reach: it is one whose newest line is off the
/// bottom of the screen, which is the only line that matters.
const TRANSCRIPT_LINES: usize = 18;

/// The longest option label drawn on a choice row.
const MAX_OPTION_LABEL: usize = 44;

const TYPE: &str = "type";
const TALK: &str = "talk";
const SERVICE: &str = "service";
/// Where the chosen provider is remembered between sessions.
const CHOSEN: &str = "provider";
const CHOICES: [&str; 3] = ["service-0", "service-1", "service-2"];
const CANCEL: &str = "cancel";
const RETRY: &str = "retry";
const OPTIONS: [&str; conversation::MAX_OPTIONS] = [
    "option-0", "option-1", "option-2", "option-3", "option-4", "option-5",
];

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum View {
    #[default]
    Talking,
    Composing,
    /// Which service to talk to. Its own screen rather than a sheet, because
    /// the answer changes what every subsequent request looks like and the
    /// reader should see it stated rather than glimpse it.
    Choosing,
    /// A request is in flight. The transcript stays on the panel underneath,
    /// because replacing it with a waiting screen would cost a full repaint
    /// to show less than was there before.
    Waiting,
}

/// What the application calls itself, in the one place it says so.
const TITLE: &str = "AI Command Center";

/// The three destinations, in one place so that no screen can disagree with
/// another about where the bar goes or what is on it.
const DESTINATIONS: [(&str, &str); 3] =
    [(TALK, "Conversation"), (TYPE, "Type"), (SERVICE, "Service")];

/// A provider's name, marked when it is the one in use.
///
/// The mark is a character rather than a tone: a chosen row drawn a shade
/// darker is invisible on a panel that resolves sixteen greys under a reading
/// light, and this is the only way to tell which key a request will use.
#[derive(Default)]
struct Chat {
    conversation: Conversation,
    keyboard: Keyboard,
    view: View,
    /// Which service the next request goes to.
    ///
    /// The application still never sees a key. This picks the endpoint, the
    /// body shape, and the *name* of the secret the runtime resolves; if the
    /// runtime holds no secret under that name the request is refused, which
    /// is the honest answer.
    provider: Provider,
    task: Option<TaskId>,
    /// What went wrong, if anything. Always recoverable: it is drawn as a
    /// banner above a screen that still has every control it had before.
    trouble: Option<String>,
}

impl Chat {
    fn show(&self, context: &mut Context) {
        // The keyboard and the service list are both destinations reached from
        // the transcript, so Back returns to the transcript before it leaves.
        let owns_back = matches!(self.view, View::Composing | View::Choosing);
        context.set_screen(self.screen().with_own_back(owns_back));
    }

    fn screen(&self) -> Screen {
        match self.view {
            View::Composing => self.compose(),
            View::Choosing => self.choosing(),
            View::Talking | View::Waiting => self.transcript(),
        }
    }

    /// The conversation, newest last, with whatever can be answered by tapping.
    fn transcript(&self) -> Screen {
        // The keyboard is a destination rather than a button in the flow. A
        // button underneath a transcript moves every time the transcript
        // grows, which on a panel this slow means the control walks out from
        // under a finger that is already on its way down.
        let mut screen = ScreenBuilder::new("chat")
            .top_bar(TITLE)
            .nav_bar(0, DESTINATIONS);
        if let Some(trouble) = &self.trouble {
            screen = screen.banner(BannerLevel::Attention, trouble.clone());
        }

        let turns = self.conversation.turns();
        let visible = visible_turns(turns, TRANSCRIPT_LINES);
        if turns.is_empty() {
            // Centred under a mark rather than ranged left at the top: this
            // is the first thing anybody sees, and a lone paragraph in the
            // corner of a 1448-pixel panel reads as a page that failed.
            screen = screen.splash(
                Some(Glyph::Chat),
                "Nothing said yet",
                "Tap Type to start. Answers you can tap appear as buttons, so most \
                 turns need no typing at all.",
            );
        } else if visible.len() < turns.len() {
            // Said plainly rather than hidden, because a transcript that
            // silently begins in the middle reads as a lost conversation.
            screen = screen.text("Earlier messages are no longer shown.");
        }

        for (position, turn) in visible.iter().enumerate() {
            if position > 0 {
                screen = screen.spacer(Space::Small);
            }
            screen = draw_turn(screen, turn, self.provider.label());
        }

        let offered = self.offered();
        match self.view {
            View::Waiting => {
                // A labelled state of the conversation, not a paragraph
                // trailing the last turn, so a reply that is on its way is not
                // mistaken for one that has arrived. An indeterminate activity
                // rather than a transfer: the model announces no length to
                // count towards, and a transfer captions itself in bytes,
                // which would print "0 B" for something that is not bytes.
                screen = screen
                    .section("Reply")
                    .activity("Reaching the model", None)
                    .cancellable(CANCEL, "Cancel");
            }
            _ if !offered.is_empty() => {
                // The whole point of the application: an answer that can be
                // tapped, with typing still one tap away for anything the
                // model did not think of. The choice's prompt is promoted to a
                // section so the answers read as a group under a rule rather
                // than a heading floating a hair above the first button.
                screen = screen
                    .section("Tap an answer")
                    .choose(
                        "",
                        offered
                            .iter()
                            .enumerate()
                            .map(|(index, option)| (OPTIONS[index], label(option))),
                    )
                    .or_type(TYPE, "Type something else...");
            }
            _ => {
                // A draft left in the keyboard is kept in sight here rather
                // than only on the keyboard screen. Before this, a message
                // half-typed and then navigated away from was gone, with
                // nothing on the panel to say it had ever been started; the
                // reader came back to a blank composer and retyped it.
                let draft = self.keyboard.text();
                if !draft.trim().is_empty() {
                    screen = screen
                        .section("Draft")
                        .field(TYPE, draft, "Tap to keep typing.");
                }
            }
        }

        if self.view != View::Waiting && self.can_retry() {
            screen = screen.button(RETRY, "Try again");
        }
        screen.build()
    }

    /// The keyboard, and what has been typed on it so far.
    fn compose(&self) -> Screen {
        ScreenBuilder::new("chat-compose")
            .top_bar("Type a message")
            .nav_bar(1, DESTINATIONS)
            .typed(&self.keyboard, "Your message appears here.")
            .spacer(Space::Small)
            .keyboard(&self.keyboard, "Send")
            .build()
    }

    /// The service chooser.
    fn choosing(&self) -> Screen {
        ScreenBuilder::new("chat-service")
            .top_bar("Service")
            .nav_bar(2, DESTINATIONS)
            .text(
                "The key itself is held by the runtime and never by this \
                 application. Choosing a service chooses which stored key it \
                 uses and which address the request goes to.",
            )
            .section("Talk to")
            .choose(
                "",
                PROVIDERS
                    .iter()
                    .enumerate()
                    .map(|(index, provider)| (CHOICES[index], provider.label())),
            )
            .chosen(
                PROVIDERS
                    .iter()
                    .position(|provider| *provider == self.provider)
                    .unwrap_or(0),
            )
            .build()
    }

    /// The answers the newest reply offered, if it offered any.
    ///
    /// Recomputed from the transcript rather than stored, so there is exactly
    /// one copy of the truth: what is drawn and what a tap sends are read out
    /// of the same string by the same parser.
    fn offered(&self) -> Vec<String> {
        match self.conversation.last() {
            Some(turn) if turn.role == Role::Assistant => Reply::read(&turn.text).options,
            _ => Vec::new(),
        }
    }

    /// Whether the last thing that happened was a question that never got an
    /// answer, which is the only situation where resending is what the reader
    /// means by trying again.
    fn can_retry(&self) -> bool {
        self.trouble.is_some()
            && self
                .conversation
                .last()
                .is_some_and(|turn| turn.role == Role::You)
    }

    fn say(&mut self, context: &mut Context, text: impl AsRef<str>) {
        self.conversation.push(Role::You, text);
        self.submit(context);
    }

    /// Hands the whole conversation to the runtime.
    ///
    /// The body is built here and the credential is not: `secret` is a name,
    /// and the runtime is what turns it into a header. There is no blocking
    /// alternative, which is the reason the screen stays live and the reader
    /// can still cancel.
    fn submit(&mut self, context: &mut Context) {
        let work = Task::Post {
            url: self.provider.endpoint().to_owned(),
            body: self.conversation.request_body(self.provider),
            content_type: "application/json".to_owned(),
            credential: Some(self.provider.credential()),
            headers: self.provider.headers(),
            max_bytes: conversation::MAX_REPLY_BYTES,
        };
        if let Some(task) = context.spawn(work) {
            self.task = Some(task);
            self.view = View::Waiting;
            self.trouble = None;
        } else {
            self.view = View::Talking;
            self.trouble = Some("Something else is still being sent.".to_owned());
        }
        self.show(context);
    }

    /// Handles a tap while the keyboard is up. Returns whether it was one.
    fn typing(&mut self, context: &mut Context, action: ActionId) -> bool {
        if action == action_id(TALK) {
            self.view = View::Talking;
            self.show(context);
            return true;
        }
        let Some(pressed) = self.keyboard.press(action) else {
            return false;
        };
        match pressed {
            Pressed::Edited | Pressed::Shifted => self.show(context),
            Pressed::Submitted => {
                let text = self.keyboard.text().trim().to_owned();
                // Nothing typed means nothing changed, so nothing is
                // repainted. A refresh that redraws the same pixels is the
                // most visible thing this application could do for no reason.
                if !text.is_empty() {
                    self.keyboard.clear();
                    self.view = View::Talking;
                    self.say(context, text);
                }
            }
        }
        true
    }
}

/// Draws one turn as a byline over its body, so the two sides of the
/// conversation are told apart by who is named above each block and by a small
/// indent on the reply, rather than by a "You:" glued to the front of a
/// sentence. The reader's own words sat at depth 0 under "You"; the reply sits
/// one level in under the service that wrote it, which is the only place the
/// panel says which key answered.
fn draw_turn(screen: ScreenBuilder, turn: &Turn, assistant: &str) -> ScreenBuilder {
    match turn.role {
        Role::You => screen.byline(0, "You").quote(0, turn.text.clone()),
        Role::Assistant => {
            let reply = Reply::read(&turn.text);
            let screen = screen.byline(1, assistant.to_owned());
            if reply.paragraphs.is_empty() {
                // A reply that was nothing but an options line still has to
                // occupy its place, or the transcript appears to skip a turn.
                return screen.quote(1, "(an answer to tap, below)");
            }
            reply.paragraphs.iter().fold(screen, |screen, paragraph| {
                screen.quote(1, paragraph.as_str())
            })
        }
    }
}

/// The turns that fit, newest last.
///
/// Trimmed from the front rather than the back: the newest message is the one
/// the reader is waiting for, and it is the one that must be on the panel.
/// The newest is kept even when it alone exceeds the budget, because showing
/// nothing at all would be worse than showing a message that runs long.
fn visible_turns(turns: &[Turn], budget: usize) -> &[Turn] {
    let mut used = 0;
    let mut first = turns.len();
    for (index, turn) in turns.iter().enumerate().rev() {
        let lines = turn_lines(turn);
        if index + 1 < turns.len() && used + lines > budget {
            break;
        }
        used += lines;
        first = index;
    }
    &turns[first..]
}

/// About how many lines a turn will occupy once the renderer has wrapped it.
///
/// Counts the byline as a line of its own, because a turn now carries one and a
/// budget that ignored it would keep one turn too many and push the newest off
/// the bottom -- the one failure this whole trim exists to prevent.
fn turn_lines(turn: &Turn) -> usize {
    let paragraphs = match turn.role {
        Role::You => vec![turn.text.clone()],
        Role::Assistant => Reply::read(&turn.text).paragraphs,
    };
    let body = paragraphs
        .iter()
        .map(|paragraph| paragraph.chars().count().div_ceil(COLUMNS).max(1))
        .sum::<usize>()
        .max(1);
    body + 1
}

/// Shortens an option so it stays one line on a choice row.
fn label(option: &str) -> String {
    if option.chars().count() <= MAX_OPTION_LABEL {
        return option.to_owned();
    }
    let mut short = option
        .chars()
        .take(MAX_OPTION_LABEL - 1)
        .collect::<String>();
    short.push('…');
    short
}

/// What to put in front of the reader when the runtime could not carry out the
/// request. Every one of these leaves the conversation intact and something to
/// tap, because a chat client that dead-ends on a flat battery or a lapsed key
/// is a chat client that has to be restarted to be used again.
const fn explain(error: TaskError) -> &'static str {
    match error {
        TaskError::Denied => {
            "No key is installed for this device, or the network was refused. Nothing was sent."
        }
        TaskError::Unreachable => "The network could not be reached. Wi-Fi may be asleep.",
        TaskError::TooLarge => "The reply was too long to read on this device.",
        TaskError::TimedOut => "The service took too long to answer.",
        TaskError::NotFound => "The service refused the request. The key may be wrong or spent.",
    }
}

impl KoboApp for Chat {
    fn on_start(&mut self, context: &mut Context) {
        context.store().load(CHOSEN);
        self.show(context);
    }

    /// Restores the remembered service.
    ///
    /// A first run, a cleared store and a refusal all land on the default,
    /// because none of them is a reason to put an error in front of someone
    /// who only wanted to ask a question.
    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        if let StoreResult::Loaded { key, value } = result {
            if key != CHOSEN {
                return;
            }
            let restored = value
                .as_deref()
                .and_then(|bytes| std::str::from_utf8(bytes).ok())
                .map(Provider::from_key)
                .unwrap_or_default();
            if restored != self.provider {
                self.provider = restored;
                self.show(context);
            }
        }
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if action == ActionId::BACK {
            // Only offered on the keyboard and the service list, both of which
            // return to the transcript they were opened from.
            self.view = View::Talking;
            self.show(context);
            return;
        }
        if self.view == View::Composing && self.typing(context, action) {
            return;
        }

        if action == action_id(TALK) {
            // Already here. Repainting would cost a refresh to show exactly
            // what is already on the panel.
            return;
        }

        if action == action_id(TYPE) {
            self.view = View::Composing;
            self.trouble = None;
            self.show(context);
            return;
        }

        if action == action_id(SERVICE) {
            self.view = View::Choosing;
            self.trouble = None;
            self.show(context);
            return;
        }

        if let Some(index) = CHOICES.iter().position(|name| action == action_id(name)) {
            if let Some(&provider) = PROVIDERS.get(index) {
                self.provider = provider;
                context
                    .store()
                    .save(CHOSEN, provider.key().as_bytes().to_vec());
                self.view = View::Talking;
                self.trouble = None;
                self.show(context);
            }
            return;
        }

        if action == action_id(CANCEL) {
            if let Some(task) = self.task {
                context.cancel(task);
            }
            return;
        }

        if action == action_id(RETRY) && self.can_retry() {
            self.submit(context);
            return;
        }

        if let Some(index) = OPTIONS.iter().position(|name| action == action_id(name)) {
            // Read back out of the transcript, so an option can only ever send
            // text the model actually offered.
            if let Some(option) = self.offered().get(index) {
                self.say(context, option.clone());
            }
        }
    }

    fn on_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
        if self.task != Some(task) {
            return;
        }
        self.task = None;
        self.view = View::Talking;
        match outcome {
            TaskOutcome::Completed(bytes) => {
                match conversation::read_completion(&bytes, self.provider) {
                    Ok(reply) => {
                        self.conversation.push(Role::Assistant, reply);
                        self.trouble = None;
                    }
                    Err(trouble) => self.trouble = Some(trouble),
                }
            }
            TaskOutcome::Failed(error) => {
                // The kind of failure, never the conversation: what the reader
                // said is theirs and has no business in the system log.
                context.log(LogLevel::Warn, format!("chat request failed: {error}"));
                self.trouble = Some(explain(error).to_owned());
            }
            TaskOutcome::Cancelled => {
                self.trouble = Some("That question was cancelled.".to_owned());
            }
        }
        self.show(context);
    }
}

fn main() -> ExitCode {
    match kobo_sdk::run("chat", Chat::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("chat: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        conversation::Provider,
        conversation::{Role, Turn},
        visible_turns, Chat, View, CHOICES, CHOSEN, COLUMNS, OPTIONS, SERVICE, TALK,
        TRANSCRIPT_LINES, TYPE,
    };
    use kobo_sdk::keyboard::Keyboard;
    use kobo_sdk::{
        action_id, Command, Context, KoboApp, Screen, StoreRequest, StoreResult, Task, TaskId,
        TaskOutcome,
    };
    use kobo_ui::{Chrome, LayoutKind, QuoteRole, CLARA_BW_METRICS};

    /// Runs one callback and hands back what the application asked for.
    fn act(chat: &mut Chat, action: &str) -> Vec<Command> {
        let mut context = Context::default();
        chat.on_action(&mut context, action_id(action));
        context.take_commands()
    }

    fn started() -> (Chat, Context) {
        let mut chat = Chat::default();
        let mut context = Context::default();
        chat.on_start(&mut context);
        (chat, context)
    }

    /// The one `Task::Post` in a batch of commands, if there is one.
    fn posted(commands: &[Command]) -> Option<(String, Option<String>)> {
        commands.iter().find_map(|command| match command {
            Command::Spawn {
                work: Task::Post {
                    body, credential, ..
                },
                ..
            } => Some((
                body.clone(),
                credential.as_ref().map(|held| held.secret.clone()),
            )),
            _ => None,
        })
    }

    fn last_screen(commands: &[Command]) -> Screen {
        commands
            .iter()
            .rev()
            .find_map(|command| match command {
                Command::SetScreen(screen) => Some(screen.clone()),
                _ => None,
            })
            .expect("the application painted something")
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

    fn reply(text: &str) -> TaskOutcome {
        let body = kobo_json::ObjectBuilder::new()
            .set(
                "choices",
                vec![kobo_json::ObjectBuilder::new().set(
                    "message",
                    kobo_json::ObjectBuilder::new()
                        .set("role", "assistant")
                        .set("content", text),
                )],
            )
            .build()
            .to_json();
        TaskOutcome::Completed(body.into_bytes())
    }

    /// Types `text` on the on-screen keyboard and taps send.
    fn type_and_send(chat: &mut Chat, text: &str) -> Vec<Command> {
        act(chat, TYPE);
        chat.keyboard = Keyboard::with_text(text);
        act(chat, "kb.enter")
    }

    #[test]
    fn typing_a_message_sends_the_whole_conversation_and_names_a_secret_it_never_reads() {
        let (mut chat, _) = started();
        let commands = type_and_send(&mut chat, "hello");
        let (body, secret) = posted(&commands).expect("the message was sent");
        assert_eq!(secret.as_deref(), Some("openai"));
        assert!(body.contains("hello"));
        // The promise the whole application rests on: a name went out, not a
        // key, and nothing key-shaped is anywhere near the body.
        assert!(!body.contains("sk-"), "{body}");
        assert!(!body.contains("Authorization"), "{body}");
    }

    #[test]
    fn a_draft_stays_in_sight_after_leaving_the_keyboard() {
        // The defect: a message half-typed and then navigated away from was
        // gone, with nothing on the panel to say it had ever been started, so
        // the reader came back to a blank composer and typed it a second time.
        let (mut chat, _) = started();
        act(&mut chat, TYPE);
        chat.keyboard = Keyboard::with_text("half a thought");
        act(&mut chat, TALK);
        assert_eq!(chat.view, View::Talking);
        let screen = chat.screen();
        assert!(
            screen
                .layout_with(&CLARA_BW_METRICS, &Chrome::default())
                .rect_of_action(action_id(TYPE))
                .is_some(),
            "the draft was not left tappable to keep typing"
        );
        assert!(
            shown(&screen)
                .iter()
                .any(|line| line.contains("half a thought")),
            "the draft vanished when the keyboard closed"
        );
    }

    #[test]
    fn a_reply_offering_options_puts_them_on_the_panel_as_taps() {
        let (mut chat, _) = started();
        let commands = type_and_send(&mut chat, "what next");
        let task = spawned(&commands);
        let mut context = Context::default();
        chat.on_task(
            &mut context,
            task,
            reply("Where shall we start?\n{\"options\":[\"The beginning\",\"The end\"]}"),
        );
        let screen = last_screen(&context.take_commands());
        let lines = shown(&screen);
        assert!(lines.iter().any(|line| line.contains("Where shall we")));
        assert!(lines.iter().any(|line| line.contains("The beginning")));
        // And nothing of the machinery that carried them.
        assert!(
            !lines.iter().any(|line| line.contains("options")),
            "{lines:?}"
        );
    }

    #[test]
    fn tapping_an_option_sends_it_as_the_next_message() {
        let (mut chat, _) = started();
        let commands = type_and_send(&mut chat, "what next");
        let task = spawned(&commands);
        let mut context = Context::default();
        chat.on_task(
            &mut context,
            task,
            reply("Where shall we start?\n{\"options\":[\"The beginning\",\"The end\"]}"),
        );
        let commands = act(&mut chat, OPTIONS[1]);
        let (body, _) = posted(&commands).expect("the tap sent a message");
        assert!(body.contains("The end"), "{body}");
        assert_eq!(
            chat.conversation.last().map(|turn| turn.role),
            Some(Role::You)
        );
    }

    #[test]
    fn a_reply_that_ignores_the_format_is_read_as_prose_and_never_as_json() {
        let (mut chat, _) = started();
        let commands = type_and_send(&mut chat, "tell me something");
        let task = spawned(&commands);
        let mut context = Context::default();
        chat.on_task(
            &mut context,
            task,
            reply("Bleak House was serialised in twenty parts.\n{\"opts\": broken"),
        );
        let lines = shown(&last_screen(&context.take_commands()));
        assert!(lines.iter().any(|line| line.contains("twenty parts")));
        assert!(
            !lines.iter().any(|line| line.contains('{')),
            "raw JSON reached the panel: {lines:?}"
        );
    }

    #[test]
    fn a_failed_request_leaves_a_banner_and_a_way_to_try_again() {
        // Every error in this application is recoverable. A chat client that
        // has to be restarted after one lapsed connection is one the reader
        // will not come back to.
        let (mut chat, _) = started();
        let commands = type_and_send(&mut chat, "hello");
        let task = spawned(&commands);
        let mut context = Context::default();
        chat.on_task(
            &mut context,
            task,
            TaskOutcome::Failed(kobo_sdk::TaskError::Unreachable),
        );
        let screen = last_screen(&context.take_commands());
        assert!(shown(&screen)
            .iter()
            .any(|line| line.contains("could not be reached")));
        let retry = screen
            .layout_with(&CLARA_BW_METRICS, &Chrome::default())
            .rect_of_action(action_id("retry"));
        assert!(retry.is_some(), "there is no way back from a failed send");
        let commands = act(&mut chat, "retry");
        assert!(posted(&commands).is_some(), "trying again sent nothing");
    }

    #[test]
    fn waiting_is_stated_once_and_never_animated() {
        // There are no spinners anywhere in this system: every frame of one
        // would be a full panel refresh.
        let (mut chat, _) = started();
        let commands = type_and_send(&mut chat, "hello");
        assert_eq!(chat.view, View::Waiting);
        let layout = last_screen(&commands).layout_with(&CLARA_BW_METRICS, &Chrome::default());
        assert!(layout
            .nodes
            .iter()
            .any(|node| node.kind == LayoutKind::ActivityLabel));
    }

    #[test]
    fn an_empty_message_is_not_sent_and_does_not_repaint_the_panel() {
        let (mut chat, _) = started();
        act(&mut chat, TYPE);
        chat.keyboard = Keyboard::with_text("   ");
        let commands = act(&mut chat, "kb.enter");
        assert!(posted(&commands).is_none());
        assert!(
            commands.is_empty(),
            "an empty send repainted the panel for nothing"
        );
    }

    #[test]
    fn the_newest_message_is_always_on_the_panel_however_long_the_conversation() {
        // Nothing scrolls on an E Ink panel, so a transcript that overflows
        // has pushed the only line the reader is waiting for off the bottom.
        let mut chat = Chat::default();
        for index in 0..40 {
            chat.conversation
                .push(Role::You, format!("question {index}"));
            chat.conversation.push(
                Role::Assistant,
                "A long answer that runs on for a while so that it certainly wraps onto \
                 more than one line of the panel, several times over.",
            );
        }
        chat.conversation.push(Role::You, "the newest question");
        let layout = chat
            .screen()
            .layout_with(&CLARA_BW_METRICS, &Chrome::default());
        let newest = layout
            .nodes
            .iter()
            .rfind(|node| {
                node.text_lines
                    .iter()
                    .any(|line| line.contains("the newest question"))
            })
            .expect("the newest message is drawn at all");
        assert!(
            newest.rect.y + newest.rect.height <= CLARA_BW_METRICS.height,
            "the newest message is off the bottom of the panel: {:?}",
            newest.rect
        );
    }

    #[test]
    fn a_reply_of_several_paragraphs_becomes_several_nodes() {
        // The renderer wraps words but treats a quote as one paragraph, so a
        // blank line in the model's answer would vanish and two paragraphs
        // would run together into a wall of text. Splitting them here is what
        // keeps a long answer readable on this panel.
        let mut chat = Chat::default();
        chat.conversation
            .push(Role::Assistant, "First paragraph.\n\nSecond paragraph.");
        let paragraphs = chat
            .screen()
            .layout_with(&CLARA_BW_METRICS, &Chrome::default())
            .nodes
            .iter()
            .filter(|node| matches!(node.kind, LayoutKind::Quote(_, QuoteRole::Body)))
            .filter(|node| {
                node.text_lines
                    .iter()
                    .any(|line| line.contains("paragraph."))
            })
            .count();
        assert_eq!(paragraphs, 2, "the two paragraphs ran together");
    }

    #[test]
    fn the_transcript_is_trimmed_to_a_budget_rather_than_grown_without_limit() {
        let turns = (0..30)
            .map(|index| Turn {
                role: Role::You,
                text: format!("message {index} {}", "x".repeat(COLUMNS)),
            })
            .collect::<Vec<_>>();
        let visible = visible_turns(&turns, TRANSCRIPT_LINES);
        assert!(visible.len() < turns.len(), "nothing was trimmed");
        assert_eq!(
            visible.last(),
            turns.last(),
            "the trim dropped the newest turn"
        );
    }

    #[test]
    fn one_enormous_turn_is_still_shown_rather_than_trimmed_away_entirely() {
        // Otherwise a single long reply would leave a blank screen, which
        // reads as a crash rather than as a long answer.
        let turns = vec![Turn {
            role: Role::You,
            text: "y".repeat(COLUMNS * TRANSCRIPT_LINES * 4),
        }];
        assert_eq!(visible_turns(&turns, TRANSCRIPT_LINES).len(), 1);
    }

    #[test]
    fn every_key_of_the_compose_screen_is_reachable_on_this_panel() {
        // The compose screen is the one that can run off the bottom, because
        // the keyboard is four rows of controls under whatever has been typed.
        let chat = Chat {
            view: View::Composing,
            keyboard: Keyboard::with_text(
                "a message long enough to wrap onto a second line of the panel, \
                 which is what a real one does",
            ),
            ..Chat::default()
        };
        let layout = chat
            .screen()
            .layout_with(&CLARA_BW_METRICS, &Chrome::default());
        let send = layout
            .rect_of_action(action_id("kb.enter"))
            .expect("a send key");
        assert!(
            send.y + send.height <= CLARA_BW_METRICS.height,
            "the send key is off the bottom of the panel: {send:?}"
        );
        assert!(
            send.height >= CLARA_BW_METRICS.touch_target_minimum(),
            "the send key is too small to tap: {send:?}"
        );
    }

    #[test]
    fn a_conversation_with_nothing_in_it_says_so_and_offers_the_keyboard() {
        let (chat, mut context) = started();
        let screen = last_screen(&context.take_commands());
        assert!(shown(&screen)
            .iter()
            .any(|line| line.contains("Nothing said yet")));
        assert!(screen
            .layout_with(&CLARA_BW_METRICS, &Chrome::default())
            .rect_of_action(action_id(TYPE))
            .is_some());
        assert!(chat.conversation.turns().is_empty());
    }

    #[test]
    fn the_way_to_the_keyboard_never_moves_however_long_the_conversation_gets() {
        // The original defect this system has already been bitten by: a
        // control below text that reflows walks down the panel as the text
        // grows, so the finger that was aimed at it lands on whatever took
        // its place. The keyboard is reached from a bar pinned to the panel
        // for exactly that reason, and this asserts the rectangle rather than
        // the intention.
        let (mut chat, mut context) = started();
        let empty = chat
            .screen()
            .layout_with(&CLARA_BW_METRICS, &Chrome::default())
            .rect_of_action(action_id(TYPE))
            .expect("the empty conversation offers the keyboard");
        for turn in 0..12 {
            chat.conversation.push(Role::You, format!("message {turn}"));
            chat.conversation.push(
                Role::Assistant,
                "A reply long enough to wrap onto more than one line of a panel \
                 that is only a few inches across, which is the whole point.",
            );
        }
        let full = chat
            .screen()
            .layout_with(&CLARA_BW_METRICS, &Chrome::default())
            .rect_of_action(action_id(TYPE))
            .expect("a full conversation still offers the keyboard");
        assert_eq!(empty, full, "the keyboard moved as the transcript grew");
        assert!(
            full.height >= CLARA_BW_METRICS.touch_target_minimum(),
            "the way to the keyboard is too small to tap: {full:?}"
        );
        let _ = &mut context;
    }

    #[test]
    fn the_keyboard_screen_offers_the_way_back_in_the_same_place() {
        let (mut chat, mut context) = started();
        let talking = chat
            .screen()
            .layout_with(&CLARA_BW_METRICS, &Chrome::default())
            .rect_of_action(action_id(TALK))
            .expect("the transcript names where it already is");
        act(&mut chat, TYPE);
        let composing = chat
            .screen()
            .layout_with(&CLARA_BW_METRICS, &Chrome::default())
            .rect_of_action(action_id(TALK))
            .expect("the keyboard offers the way back");
        assert_eq!(talking, composing);
        let _ = &mut context;
    }

    /// The identifier the application handed to `Context::spawn`.
    fn spawned(commands: &[Command]) -> TaskId {
        commands
            .iter()
            .find_map(|command| match command {
                Command::Spawn { task, .. } => Some(*task),
                _ => None,
            })
            .expect("a task was spawned")
    }

    #[test]
    fn an_outcome_for_some_other_task_is_ignored() {
        // Tasks report back exactly once, but nothing stops a stale outcome
        // arriving after a cancel, and mistaking one for a reply would put
        // another application's answer into this conversation.
        let (mut chat, _) = started();
        type_and_send(&mut chat, "hello");
        let mut context = Context::default();
        chat.on_task(&mut context, TaskId(999), reply("not for you"));
        assert!(context.take_commands().is_empty());
        assert_eq!(chat.view, View::Waiting);
    }

    #[test]
    fn the_conversation_survives_a_cancel_so_the_question_can_be_asked_again() {
        let (mut chat, _) = started();
        let commands = type_and_send(&mut chat, "hello");
        let task = spawned(&commands);
        let mut context = Context::default();
        chat.on_task(&mut context, task, TaskOutcome::Cancelled);
        assert!(chat.can_retry());
        assert_eq!(
            chat.conversation.turns().len(),
            1,
            "a cancel threw the question away"
        );
    }

    /// The whole point of naming the credential header: the request goes
    /// straight to the service the reader picked, with that service's key in
    /// that service's header and that service's body shape.
    #[test]
    fn choosing_a_service_changes_where_the_next_question_goes() {
        let (mut chat, _) = started();
        act(&mut chat, SERVICE);
        assert_eq!(chat.view, View::Choosing);

        let saved = act(&mut chat, CHOICES[1]);
        assert_eq!(chat.provider, Provider::Anthropic);
        assert!(
            saved.iter().any(|command| matches!(
                command,
                Command::Store(StoreRequest::Save { key, value })
                    if key == CHOSEN && value == Provider::Anthropic.key().as_bytes()
            )),
            "the choice was not remembered"
        );

        let commands = type_and_send(&mut chat, "hello");
        let request = commands
            .iter()
            .find_map(|command| match command {
                Command::Spawn {
                    work:
                        Task::Post {
                            url,
                            credential,
                            headers,
                            ..
                        },
                    ..
                } => Some((url.clone(), credential.clone(), headers.clone())),
                _ => None,
            })
            .expect("a question was asked");
        assert_eq!(request.0, Provider::Anthropic.endpoint());
        let credential = request.1.expect("a credential");
        assert_eq!(credential.header_name(), "x-api-key");
        assert_eq!(credential.secret, "anthropic");
        assert!(request
            .2
            .iter()
            .any(|header| header.name == "anthropic-version"));
    }

    /// The chooser has to say which service is already in use, and it has to
    /// say it with something the panel can actually draw: a tick character in
    /// the label rendered as a missing-glyph box on the device.
    #[test]
    fn the_service_in_use_is_marked_and_no_label_carries_a_symbol() {
        let (mut chat, _) = started();
        act(&mut chat, SERVICE);
        act(&mut chat, CHOICES[1]);
        act(&mut chat, SERVICE);
        let screen = chat.screen();
        let [.., kobo_sdk::Node::Choice {
            options, selected, ..
        }] = &screen.nodes[..]
        else {
            unreachable!("the chooser ends in a choice")
        };
        assert_eq!(*selected, Some(1));
        for option in options {
            assert!(
                option.label.is_ascii(),
                "a label carries a symbol the installed face may not have: {}",
                option.label
            );
        }
    }

    #[test]
    fn the_remembered_service_comes_back_and_a_stale_one_does_not() {
        let (mut chat, _) = started();
        let mut context = Context::default();
        chat.on_store(
            &mut context,
            StoreResult::Loaded {
                key: CHOSEN.to_owned(),
                value: Some(Provider::Gemini.key().as_bytes().to_vec()),
            },
        );
        assert_eq!(chat.provider, Provider::Gemini);

        // A value naming a service that no longer exists is not a reason to
        // put an error in front of someone who wanted to ask a question.
        let mut chat = Chat::default();
        let mut context = Context::default();
        chat.on_store(
            &mut context,
            StoreResult::Loaded {
                key: CHOSEN.to_owned(),
                value: Some(b"a-service-that-was-removed".to_vec()),
            },
        );
        assert_eq!(chat.provider, Provider::OpenAi);
    }

    /// The bar carries three destinations now, and it still must not move: a
    /// control that walks down the panel as the transcript grows is one that
    /// leaves from under a finger already on its way down.
    #[test]
    fn the_bar_is_in_the_same_place_however_long_the_conversation_is() {
        let bar = |chat: &Chat| {
            chat.screen()
                .layout_with(&CLARA_BW_METRICS, &Chrome::default())
                .nodes
                .iter()
                .filter(|node| {
                    matches!(
                        node.kind,
                        LayoutKind::NavDestination(_) | LayoutKind::NavDestinationSelected(_)
                    )
                })
                .map(|node| node.rect)
                .collect::<Vec<_>>()
        };
        let (mut chat, _) = started();
        let empty = bar(&chat);
        assert_eq!(empty.len(), 3, "three destinations");
        for index in 0..12 {
            chat.conversation
                .push(Role::You, format!("question {index}"));
            chat.conversation
                .push(Role::Assistant, "A reasonably long answer. ".repeat(6));
        }
        assert_eq!(bar(&chat), empty, "the bar moved as the transcript grew");
    }
}

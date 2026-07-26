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

use conversation::{Conversation, Reply, Role, Turn};
use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, KoboApp, LogLevel, Screen, ScreenBuilder, Space,
    Task, TaskError, TaskId, TaskOutcome,
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
    /// A request is in flight. The transcript stays on the panel underneath,
    /// because replacing it with a waiting screen would cost a full repaint
    /// to show less than was there before.
    Waiting,
}

/// What the application calls itself, in the one place it says so.
const TITLE: &str = "AI Command Center";

#[derive(Default)]
struct Chat {
    conversation: Conversation,
    keyboard: Keyboard,
    view: View,
    task: Option<TaskId>,
    /// What went wrong, if anything. Always recoverable: it is drawn as a
    /// banner above a screen that still has every control it had before.
    trouble: Option<String>,
}

impl Chat {
    fn show(&self, context: &mut Context) {
        context.set_screen(self.screen());
    }

    fn screen(&self) -> Screen {
        match self.view {
            View::Composing => self.compose(),
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
            .nav_bar(0, [(TALK, "Conversation"), (TYPE, "Type")]);
        if let Some(trouble) = &self.trouble {
            screen = screen.banner(BannerLevel::Attention, trouble.clone());
        }

        let turns = self.conversation.turns();
        let visible = visible_turns(turns, TRANSCRIPT_LINES);
        if turns.is_empty() {
            screen = screen.text(
                "Nothing said yet. Tap Type to start. Answers you can tap will appear as \
                 buttons, so most turns need no typing at all.",
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
            screen = draw_turn(screen, turn);
        }

        let offered = self.offered();
        match self.view {
            View::Waiting => {
                screen = screen
                    .spacer(Space::Small)
                    .activity("Waiting for a reply", None)
                    .cancellable(CANCEL, "Cancel");
            }
            _ if !offered.is_empty() => {
                // The whole point of the application: an answer that can be
                // tapped, with typing still one tap away for anything the
                // model did not think of.
                screen = screen
                    .spacer(Space::Small)
                    .choose(
                        "Tap an answer",
                        offered
                            .iter()
                            .enumerate()
                            .map(|(index, option)| (OPTIONS[index], label(option))),
                    )
                    .or_type(TYPE, "Type something else...");
            }
            _ => {}
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
            .nav_bar(1, [(TALK, "Conversation"), (TYPE, "Type")])
            .typed(&self.keyboard, "Your message appears here.")
            .spacer(Space::Small)
            .keyboard(&self.keyboard, "Send")
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
            url: conversation::ENDPOINT.to_owned(),
            body: self.conversation.request_body(),
            content_type: "application/json".to_owned(),
            secret: Some(conversation::SECRET.to_owned()),
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

/// Draws one turn, prefixing the reader's own words so the two sides of the
/// conversation can be told apart without colour, weight or indentation —
/// none of which this panel spends well.
fn draw_turn(screen: ScreenBuilder, turn: &Turn) -> ScreenBuilder {
    match turn.role {
        Role::You => screen.text(format!("You: {}", turn.text)),
        Role::Assistant => {
            let reply = Reply::read(&turn.text);
            if reply.paragraphs.is_empty() {
                // A reply that was nothing but an options line still has to
                // occupy its place, or the transcript appears to skip a turn.
                return screen.text("(an answer to tap, below)");
            }
            reply.paragraphs.iter().fold(screen, ScreenBuilder::text)
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
fn turn_lines(turn: &Turn) -> usize {
    let paragraphs = match turn.role {
        Role::You => vec![turn.text.clone()],
        Role::Assistant => Reply::read(&turn.text).paragraphs,
    };
    paragraphs
        .iter()
        .map(|paragraph| paragraph.chars().count().div_ceil(COLUMNS).max(1))
        .sum::<usize>()
        .max(1)
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
        self.show(context);
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
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
            TaskOutcome::Completed(bytes) => match conversation::read_completion(&bytes) {
                Ok(reply) => {
                    self.conversation.push(Role::Assistant, reply);
                    self.trouble = None;
                }
                Err(trouble) => self.trouble = Some(trouble),
            },
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
        conversation::{Role, Turn},
        visible_turns, Chat, View, COLUMNS, OPTIONS, TALK, TRANSCRIPT_LINES, TYPE,
    };
    use kobo_sdk::keyboard::Keyboard;
    use kobo_sdk::{action_id, Command, Context, KoboApp, Screen, Task, TaskId, TaskOutcome};
    use kobo_ui::{Chrome, LayoutKind, CLARA_BW_METRICS};

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
                work: Task::Post { body, secret, .. },
                ..
            } => Some((body.clone(), secret.clone())),
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
            .layout_with(&CLARA_BW_METRICS, Chrome::default())
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
            .layout_with(&CLARA_BW_METRICS, Chrome::default())
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
        let layout = last_screen(&commands).layout_with(&CLARA_BW_METRICS, Chrome::default());
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
            .layout_with(&CLARA_BW_METRICS, Chrome::default());
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
        // The renderer wraps words but treats a text node as one paragraph,
        // so a blank line in the model's answer would vanish and two
        // paragraphs would run together into a wall of text. Splitting them
        // here is what keeps a long answer readable on this panel.
        let mut chat = Chat::default();
        chat.conversation
            .push(Role::Assistant, "First paragraph.\n\nSecond paragraph.");
        let paragraphs = chat
            .screen()
            .layout_with(&CLARA_BW_METRICS, Chrome::default())
            .nodes
            .iter()
            .filter(|node| node.kind == LayoutKind::Text)
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
            .layout_with(&CLARA_BW_METRICS, Chrome::default());
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
            .layout_with(&CLARA_BW_METRICS, Chrome::default())
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
            .layout_with(&CLARA_BW_METRICS, Chrome::default())
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
            .layout_with(&CLARA_BW_METRICS, Chrome::default())
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
            .layout_with(&CLARA_BW_METRICS, Chrome::default())
            .rect_of_action(action_id(TALK))
            .expect("the transcript names where it already is");
        act(&mut chat, TYPE);
        let composing = chat
            .screen()
            .layout_with(&CLARA_BW_METRICS, Chrome::default())
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
}

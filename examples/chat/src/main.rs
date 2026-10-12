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
use kobo_sdk::exports::{Export, Format as ExportFormat};
use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Failure, Glyph, KoboApp, LogLevel, Screen,
    ScreenBuilder, Space, StoreResult, Task, TaskError, TaskId, TaskOutcome,
};
use std::process::ExitCode;
use unicode_segmentation::UnicodeSegmentation;

/// Display-only fragments; persisted/provider turns remain unchanged.
#[derive(Clone)]
struct TurnPart {
    turn: usize,
    text: String,
}

/// Rough line-width estimate used only to seed measured transcript pages.
const COLUMNS: usize = 48;

/// Initial page budget; complete screen measurement refines it and splits
/// oversized turns without altering stored conversation text.
const TRANSCRIPT_LINES: usize = 12;

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
const MORE: &str = "more";
const NEW: &str = "new";
const CONFIRM_NEW: &str = "confirm-new";
const KEEP: &str = "keep";
const EXPORT: &str = "export";
const EARLIER: &str = "earlier";
const LATER: &str = "later";
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
    /// Whether the transcript's own menu is open.
    menu_open: bool,
    /// Whether the reader has been asked to confirm starting over.
    confirming_new: bool,
    /// How many pages back from the newest the reader is reading. Zero while
    /// a conversation is underway; older pages are for reading back.
    pages_back: usize,
    /// A copy being prepared for the paired computer, when asked for one.
    export: Option<Export>,
}

impl Chat {
    fn show(&self, context: &mut Context) {
        // The keyboard and the service list are both destinations reached from
        // the transcript, so Back returns to the transcript before it leaves.
        // A menu, a confirmation and an export are layers over it, so Back
        // lifts them first.
        let owns_back = matches!(self.view, View::Composing | View::Choosing)
            || self.menu_open
            || self.confirming_new
            || self.export.is_some();
        context.set_screen(self.screen_for(context).with_own_back(owns_back));
    }

    #[cfg(test)]
    fn screen(&self) -> Screen {
        self.screen_for(&Context::default())
    }

    fn screen_for(&self, context: &Context) -> Screen {
        if let Some(export) = &self.export {
            return export.screen();
        }
        match self.view {
            View::Composing => self.compose(),
            View::Choosing => self.choosing(),
            View::Talking | View::Waiting => {
                let pages = self.measured_pages(context);
                self.transcript_page(
                    &pages,
                    if self.view == View::Waiting {
                        0
                    } else {
                        self.pages_back
                    },
                )
            }
        }
    }

    /// The conversation, newest last, with whatever can be answered by tapping.
    fn measured_pages(&self, context: &Context) -> Vec<Vec<TurnPart>> {
        let turns = self.conversation.turns();
        let metrics = context.metrics();
        let mut pages: Vec<Vec<TurnPart>> = transcript_pages(turns, self.page_budget())
            .into_iter()
            .map(|(first, end)| {
                (first..end)
                    .map(|turn| TurnPart {
                        turn,
                        text: display_text(&turns[turn]),
                    })
                    .collect()
            })
            .collect();
        let mut page = 0;
        while page < pages.len() {
            let screen = self.transcript_page(&pages, pages.len() - 1 - page);
            if screen
                .diagnostics(&metrics, &kobo_sdk::Chrome::measuring(true))
                .issues
                .iter()
                .any(|issue| {
                    matches!(
                        issue.kind,
                        kobo_sdk::LayoutIssueKind::ContentOverflow { .. }
                            | kobo_sdk::LayoutIssueKind::Clipped
                            | kobo_sdk::LayoutIssueKind::InteractiveOffscreen
                            | kobo_sdk::LayoutIssueKind::TextOverflow
                    )
                })
            {
                if pages[page].len() > 1 {
                    let rest = pages[page].split_off(1);
                    pages.insert(page + 1, rest);
                    continue;
                }
                let part = &mut pages[page][0];
                if let Some(split) = split_turn_text(&part.text) {
                    let next = TurnPart {
                        turn: part.turn,
                        text: part.text.split_off(split),
                    };
                    pages.insert(page + 1, vec![next]);
                    continue;
                }
            }
            page += 1;
        }
        pages
    }

    fn transcript_page(&self, pages: &[Vec<TurnPart>], pages_back: usize) -> Screen {
        // The keyboard is a destination rather than a button in the flow. A
        // button underneath a transcript moves every time the transcript
        // grows, which on a panel this slow means the control walks out from
        // under a finger that is already on its way down.
        let mut screen = ScreenBuilder::new("chat")
            .top_bar(TITLE)
            .nav_bar(0, DESTINATIONS);
        let turns = self.conversation.turns();
        if !turns.is_empty() {
            screen = screen.top_bar_overflow(
                MORE,
                self.menu_open,
                [(NEW, "New conversation"), (EXPORT, "Save a copy")],
            );
        }
        if let Some(trouble) = &self.trouble {
            screen = screen.banner(BannerLevel::Attention, trouble.clone());
        }

        // Nothing scrolls on this panel, so a long transcript is paged
        // rather than trimmed away: every turn stays reachable, and the
        // newest page is where the conversation happens.
        let latest = pages.len().saturating_sub(1);
        let page = latest.saturating_sub(pages_back.min(latest));
        let on_latest = page >= latest;
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
        }

        if let Some(parts) = pages.get(page) {
            for (position, part) in parts.iter().enumerate() {
                if position > 0 {
                    screen = screen.spacer(Space::Small);
                }
                screen = draw_turn(screen, &turns[part.turn], &part.text, self.provider.label());
            }
        }
        if pages.len() > 1 {
            screen = screen.page_turns(EARLIER, LATER).page_position(
                u16::try_from(page + 1).unwrap_or(u16::MAX),
                u16::try_from(pages.len()).unwrap_or(u16::MAX),
            );
        }

        let offered = self.offered();
        match self.view {
            _ if !on_latest => {}
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

        if on_latest && self.view != View::Waiting && self.can_retry() {
            screen = screen.button(RETRY, "Try again");
        }
        if self.confirming_new {
            screen = screen.confirm(
                "New conversation",
                "Start over? What was said so far is replaced.",
                (CONFIRM_NEW, "Start over"),
                (KEEP, "Keep talking"),
            );
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
            .secondary("Add a service key from your computer, for example kobo secret set openai.")
            .rows(PROVIDERS.iter().enumerate().map(|(index, provider)| {
                (
                    CHOICES[index],
                    provider.label(),
                    provider.model(),
                    if *provider == self.provider {
                        Glyph::Check
                    } else {
                        Glyph::Circle
                    },
                )
            }))
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
    /// How many estimated lines of transcript one page holds right now.
    ///
    /// The failure banner and its way back ride on the newest page and are
    /// not lines of transcript, so while trouble is on the panel each page
    /// carries fewer turns. Without this the banner and the Try again button
    /// pushed one another's neighbours off the panel at the larger text
    /// scales, and the way back was exactly what was no longer visible.
    fn page_budget(&self) -> usize {
        if self.trouble.is_some() {
            TRANSCRIPT_LINES.saturating_sub(5)
        } else {
            TRANSCRIPT_LINES
        }
    }

    fn can_retry(&self) -> bool {
        self.trouble.is_some()
            && self
                .conversation
                .last()
                .is_some_and(|turn| turn.role == Role::You)
    }

    fn say(&mut self, context: &mut Context, text: impl AsRef<str>) {
        self.conversation.push(Role::You, text);
        self.pages_back = 0;
        self.persist(context);
        self.submit(context);
    }

    /// Keeps the transcript where a restart can find it. Saving is silent:
    /// the panel already shows exactly what was said.
    fn persist(&self, context: &mut Context) {
        context
            .store()
            .save(conversation::STATE, self.conversation.encode());
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

    /// Handles a tap on the transcript's own layer: its menu, the
    /// start-over confirmation and the copy for the paired computer. Returns
    /// whether it was one.
    fn transcript_layer(&mut self, context: &mut Context, action: ActionId) -> bool {
        if action == action_id(NEW) {
            self.confirming_new = true;
        } else if action == action_id(CONFIRM_NEW) {
            self.confirming_new = false;
            self.conversation = Conversation::default();
            self.pages_back = 0;
            self.trouble = None;
            self.persist(context);
        } else if action == action_id(KEEP) {
            self.confirming_new = false;
        } else if action == action_id(EXPORT) {
            let text = self.conversation.transcript_text(self.provider.label());
            match Export::new("chat-conversation", ExportFormat::Text, text.into_bytes()) {
                Ok(mut export) => {
                    export.begin(context);
                    self.export = Some(export);
                }
                Err(error) => self.trouble = Some(error),
            }
        } else {
            return false;
        }
        self.show(context);
        true
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
/// Prefer a nearby word boundary, falling back to a complete grapheme for
/// long unbroken words. Retain every byte, including boundary whitespace.
fn split_turn_text(text: &str) -> Option<usize> {
    let count = text.graphemes(true).count();
    let (middle, _) = text.grapheme_indices(true).nth(count / 2)?;
    if middle == 0 {
        return None;
    }
    Some(
        text.grapheme_indices(true)
            .take(count / 2)
            .filter(|(_, grapheme)| grapheme.chars().all(char::is_whitespace))
            .map(|(offset, grapheme)| offset + grapheme.len())
            .filter(|offset| *offset >= middle / 2)
            .last()
            .unwrap_or(middle),
    )
}

fn display_text(turn: &Turn) -> String {
    match turn.role {
        Role::You => turn.text.clone(),
        Role::Assistant => {
            let paragraphs = Reply::read(&turn.text).paragraphs;
            if paragraphs.is_empty() {
                "(an answer to tap, below)".into()
            } else {
                paragraphs.join("\n\n")
            }
        }
    }
}

fn draw_turn(screen: ScreenBuilder, turn: &Turn, text: &str, assistant: &str) -> ScreenBuilder {
    match turn.role {
        Role::You => screen.byline(0, "You").quote(0, text),
        Role::Assistant => text.split("\n\n").fold(
            screen.byline(1, assistant.to_owned()),
            |screen, paragraph| screen.quote(1, paragraph),
        ),
    }
}

/// The transcript cut into pages that each fit the panel, oldest page first.
///
/// Seeded from newest to oldest. Measured pagination then splits oversized
/// turns into display-only fragments, including the newest turn when needed.
fn transcript_pages(turns: &[Turn], budget: usize) -> Vec<(usize, usize)> {
    let mut pages = Vec::new();
    let mut end = turns.len();
    let mut used = 0;
    let mut first = turns.len();
    for index in (0..turns.len()).rev() {
        let lines = turn_lines(&turns[index]);
        if first < end && used + lines > budget {
            pages.push((first, end));
            end = first;
            used = 0;
        }
        used += lines;
        first = index;
    }
    if first < end {
        pages.push((first, end));
    }
    pages.reverse();
    pages
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
fn explain(error: TaskError, provider: Provider) -> String {
    match error {
        // The one failure this application can say more about than the SDK
        // can: the service's key is not installed, and installing it is a
        // command the reader can run rather than a mystery to contemplate.
        TaskError::NoCredential => format!(
            "No key is installed for {}. Install one from your computer: kobo secret set {}.",
            provider.label(),
            provider.key()
        ),
        TaskError::NotFound => {
            "The service refused the request. The key may be wrong or spent.".to_owned()
        }
        other => Failure::of(other).advice.to_owned(),
    }
}

impl KoboApp for Chat {
    fn on_start(&mut self, context: &mut Context) {
        context.store().load(CHOSEN);
        context.store().load(conversation::STATE);
        self.show(context);
    }

    /// Restores the remembered service and the saved conversation.
    ///
    /// A first run, a cleared store and a refusal all land on the default,
    /// because none of them is a reason to put an error in front of someone
    /// who only wanted to ask a question.
    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        // A copy on its way out answers on its own keys; the transcript and
        // the chosen service are never handed to it.
        if let Some(export) = self.export.as_mut() {
            let key = match &result {
                StoreResult::Loaded { key, .. } | StoreResult::Saved { key } => key.as_str(),
                _ => "",
            };
            if key != CHOSEN && key != conversation::STATE && export.on_save(context, key, &result)
            {
                self.show(context);
                return;
            }
        }
        if let StoreResult::Loaded { key, value } = result {
            if key == CHOSEN {
                let restored = value
                    .as_deref()
                    .and_then(|bytes| std::str::from_utf8(bytes).ok())
                    .map(Provider::from_key)
                    .unwrap_or_default();
                if restored != self.provider {
                    self.provider = restored;
                    self.show(context);
                }
            } else if key == conversation::STATE {
                if let Some(restored) = value.as_deref().and_then(Conversation::restore) {
                    self.conversation = restored;
                    self.pages_back = 0;
                    self.show(context);
                }
            }
        }
    }

    fn on_shelf(&mut self, context: &mut Context, name: &str, result: StoreResult) {
        if let Some(export) = self.export.as_mut() {
            if export.on_shelf(context, name, &result) {
                self.show(context);
            }
        }
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if action == ActionId::BACK {
            // A menu, a confirmation and an export are layers over the
            // transcript; the keyboard and the service list are destinations
            // reached from it. Back lifts or leaves one layer at a time.
            if self.menu_open {
                self.menu_open = false;
            } else if self.confirming_new {
                self.confirming_new = false;
            } else if self.export.is_some() {
                self.export = None;
            } else {
                self.view = View::Talking;
            }
            self.show(context);
            return;
        }
        if self.view == View::Composing && self.typing(context, action) {
            return;
        }
        if action == action_id(MORE) {
            self.menu_open = !self.menu_open;
            self.show(context);
            return;
        }
        // A tap anywhere else closes the menu it came from.
        self.menu_open = false;

        if self.export.is_some() {
            if action == action_id("export-confirm") || action == action_id("export-retry") {
                if let Some(export) = self.export.as_mut() {
                    export.begin(context);
                }
                self.show(context);
            }
            return;
        }

        if self.transcript_layer(context, action) {
            return;
        }

        if action == action_id(TALK) {
            // The nav bar names where it already is; on the service list the
            // same destination is the way back to the transcript. Answering
            // it there cost nothing but the refresh the tap asked for.
            if self.view == View::Choosing || self.menu_open {
                self.menu_open = false;
                self.view = View::Talking;
                self.show(context);
            }
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

        if action == action_id(EARLIER) || action == action_id(LATER) {
            let count = self.measured_pages(context).len();
            if count > 1 {
                let latest = count - 1;
                let back = self.pages_back.min(latest);
                self.pages_back = if action == action_id(EARLIER) {
                    (back + 1).min(latest)
                } else {
                    back.saturating_sub(1)
                };
                self.show(context);
            }
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
                        self.pages_back = 0;
                        self.persist(context);
                        self.trouble = None;
                    }
                    Err(trouble) => self.trouble = Some(trouble),
                }
            }
            TaskOutcome::Failed(error) => {
                // The kind of failure, never the conversation: what the reader
                // said is theirs and has no business in the system log.
                context.log(LogLevel::Warn, format!("chat request failed: {error}"));
                self.trouble = Some(explain(error, self.provider));
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
        transcript_pages, Chat, View, CHOICES, CHOSEN, COLUMNS, EARLIER, OPTIONS, SERVICE, TALK,
        TRANSCRIPT_LINES, TYPE,
    };
    use super::{Glyph, PROVIDERS};
    use kobo_sdk::keyboard::Keyboard;
    use kobo_sdk::{
        action_id, ActionId, Command, Context, KoboApp, Screen, StoreRequest, StoreResult, Task,
        TaskId, TaskOutcome,
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
        // Asserted against the SDK's own wording rather than a copy of it, so
        // this test cannot pass while the reader is shown something else.
        let expected = kobo_sdk::Failure::of(kobo_sdk::TaskError::Unreachable).advice;
        assert!(
            shown(&screen).iter().any(|line| line.contains(expected)),
            "the banner did not carry {expected:?}"
        );
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
    fn a_long_transcript_is_paged_rather_than_trimmed_away() {
        let turns = (0..30)
            .map(|index| Turn {
                role: Role::You,
                text: format!("message {index} {}", "x".repeat(COLUMNS)),
            })
            .collect::<Vec<_>>();
        let pages = transcript_pages(&turns, TRANSCRIPT_LINES);
        assert!(pages.len() > 1, "nothing was paged");
        let (first, end) = pages[0];
        assert_eq!(first, 0, "the earliest turns are not on a page");
        assert!(end > first, "the first page is empty");
        let (_, end) = pages.last().copied().unwrap_or((0, 0));
        assert_eq!(end, turns.len(), "the newest turn is not on a page");
    }

    #[test]
    fn every_page_fits_its_panel_at_every_text_scale() {
        // The promise the pages make: no turn is dropped off the top of an
        // overfull page. The fullest page is the one to check, and the
        // estimate behind it is conservative enough that it is also the
        // oldest page by construction.
        let mut chat = Chat::default();
        for index in 0..24 {
            chat.conversation
                .push(Role::You, format!("question {index}"));
            chat.conversation.push(
                Role::Assistant,
                "A reply long enough to wrap onto more than one line of a panel \
                 that is only a few inches across, which is the whole point.",
            );
        }
        // The fullest page is the one carrying a failure: the banner and the
        // way to try again take room the turns budget has to give back.
        chat.conversation.push(Role::You, "question that failed");
        chat.trouble = Some(super::explain(
            kobo_sdk::TaskError::NoCredential,
            Provider::OpenAi,
        ));
        assert!(
            chat.can_retry(),
            "the worst page is the one with a way back"
        );
        let pages = transcript_pages(chat.conversation.turns(), chat.page_budget());
        for scale in [
            kobo_ui::TextScale::Default,
            kobo_ui::TextScale::Large,
            kobo_ui::TextScale::ExtraLarge,
        ] {
            let mut metrics = CLARA_BW_METRICS;
            metrics.text_scale = scale;
            for page in 0..pages.len() {
                chat.pages_back = pages.len() - 1 - page;
                let layout = chat
                    .screen()
                    .layout_with(&metrics, &Chrome::measuring(true));
                for node in &layout.nodes {
                    assert!(
                        node.rect.y + node.rect.height <= metrics.height,
                        "page {page} at {scale:?} overflows: {:?} past {}",
                        node.kind,
                        metrics.height
                    );
                }
            }
        }
    }

    #[test]
    fn one_enormous_turn_is_still_shown_rather_than_trimmed_away_entirely() {
        // Otherwise a single long reply would leave a blank screen, which
        // reads as a crash rather than as a long answer.
        let turns = vec![Turn {
            role: Role::You,
            text: "y".repeat(COLUMNS * TRANSCRIPT_LINES * 4),
        }];
        assert_eq!(transcript_pages(&turns, TRANSCRIPT_LINES).len(), 1);
    }

    #[test]
    fn paging_reaches_the_earliest_turns_and_a_reply_returns_to_the_end() {
        let mut chat = Chat::default();
        for index in 0..8 {
            chat.conversation
                .push(Role::You, format!("question {index}"));
            chat.conversation.push(
                Role::Assistant,
                "A reply long enough to wrap onto more than one line of a panel \
                 that is only a few inches across, which is the whole point.",
            );
        }
        let screen = chat.screen();
        assert_eq!(
            screen.page_turns.map(|turns| turns.previous),
            Some(action_id(EARLIER)),
            "a paged transcript offers the way back"
        );
        let mut context = Context::default();
        let mut lines = Vec::new();
        for _ in 0..14 {
            chat.on_action(&mut context, action_id(EARLIER));
            lines = shown(&last_screen(&context.take_commands()));
            if lines.iter().any(|line| line.contains("question 0")) {
                break;
            }
        }
        assert!(
            lines.iter().any(|line| line.contains("question 0")),
            "the earliest turn was never reached: {lines:?}"
        );
        // Asking something new lands the reader back on the newest page.
        let mut context = Context::default();
        chat.say(&mut context, "a fresh question");
        let lines = shown(&chat.screen());
        assert!(lines.iter().any(|line| line.contains("a fresh question")));
        assert!(
            !lines.iter().any(|line| line.contains("question 0")),
            "a new message did not return to the end"
        );
    }

    #[test]
    fn the_conversation_is_saved_and_comes_back_after_a_restart() {
        let (mut chat, _) = started();
        let commands = type_and_send(&mut chat, "hello");
        let task = spawned(&commands);
        let mut context = Context::default();
        chat.on_task(&mut context, task, reply("Hello to you too."));
        let saved = context
            .take_commands()
            .iter()
            .find_map(|command| match command {
                Command::Store(StoreRequest::Save { key, value })
                    if key == super::conversation::STATE =>
                {
                    Some(value.clone())
                }
                _ => None,
            })
            .expect("the reply was saved");
        // A restart is a fresh application handed the saved bytes.
        let mut restored = Chat::default();
        let mut context = Context::default();
        restored.on_store(
            &mut context,
            StoreResult::Loaded {
                key: super::conversation::STATE.to_owned(),
                value: Some(saved),
            },
        );
        let lines = shown(&restored.screen());
        assert!(lines.iter().any(|line| line.contains("hello")));
        assert!(lines.iter().any(|line| line.contains("Hello to you too.")));
    }

    #[test]
    fn a_cancelled_question_can_be_sent_again_with_one_tap() {
        let (mut chat, _) = started();
        let commands = type_and_send(&mut chat, "hello");
        let task = spawned(&commands);
        let mut context = Context::default();
        chat.on_task(&mut context, task, TaskOutcome::Cancelled);
        let screen = last_screen(&context.take_commands());
        assert!(shown(&screen).iter().any(|line| line.contains("cancelled")));
        assert!(screen
            .layout_with(&CLARA_BW_METRICS, &Chrome::default())
            .rect_of_action(action_id("retry"))
            .is_some());
        let commands = act(&mut chat, "retry");
        assert!(posted(&commands).is_some(), "the question was not resent");
    }

    #[test]
    fn a_missing_key_is_answered_with_how_to_install_one() {
        let (mut chat, _) = started();
        let commands = type_and_send(&mut chat, "hello");
        let task = spawned(&commands);
        let mut context = Context::default();
        chat.on_task(
            &mut context,
            task,
            TaskOutcome::Failed(kobo_sdk::TaskError::NoCredential),
        );
        let lines = shown(&last_screen(&context.take_commands()));
        assert!(
            lines.join(" ").contains("kobo secret set openai"),
            "the guidance did not name the command: {lines:?}"
        );
    }

    #[test]
    fn a_copy_of_the_conversation_is_prepared_for_the_computer() {
        let (mut chat, _) = started();
        let commands = type_and_send(&mut chat, "hello");
        let task = spawned(&commands);
        let mut context = Context::default();
        chat.on_task(&mut context, task, reply("Hi."));
        let commands = act(&mut chat, "more");
        let _ = commands;
        act(&mut chat, super::EXPORT);
        let export = chat.export.as_ref().expect("a copy was prepared");
        assert_eq!(export.offer().format, super::ExportFormat::Text);
        assert!(export.offer().bytes > 0);
        // Back lifts the export layer and returns the transcript.
        let mut context = Context::default();
        chat.on_action(&mut context, ActionId::BACK);
        assert!(chat.export.is_none());
        let lines = shown(&chat.screen());
        assert!(lines.iter().any(|line| line.contains("hello")));
    }

    #[test]
    fn starting_over_asks_first_and_then_clears() {
        let (mut chat, _) = started();
        type_and_send(&mut chat, "hello");
        act(&mut chat, super::NEW);
        assert!(chat.confirming_new, "no confirmation was asked");
        let commands = act(&mut chat, super::CONFIRM_NEW);
        assert!(chat.conversation.turns().is_empty());
        assert!(commands.iter().any(|command| matches!(
            command,
            Command::Store(StoreRequest::Save { key, .. })
                if key == super::conversation::STATE
        )));
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
    fn the_service_screen_s_conversation_destination_leads_back() {
        // The nav bar is on the chooser too, and the destination that names
        // the transcript was dead there: a tap on Conversation from Service
        // repainted nothing and went nowhere.
        let (mut chat, _) = started();
        act(&mut chat, SERVICE);
        assert_eq!(chat.view, View::Choosing);
        let commands = act(&mut chat, TALK);
        assert_eq!(chat.view, View::Talking);
        assert!(!commands.is_empty(), "the way back repainted nothing");
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
        let [.., kobo_sdk::Node::Rows { rows, .. }] = &screen.nodes[..] else {
            unreachable!("the chooser ends in service rows")
        };
        for (index, row) in rows.iter().enumerate() {
            assert_eq!(row.title, PROVIDERS[index].label());
            assert_eq!(row.summary, PROVIDERS[index].model());
            assert_eq!(
                row.lead,
                kobo_sdk::RowLead::from(if index == 1 {
                    Glyph::Check
                } else {
                    Glyph::Circle
                })
            );
            assert!(row.title.is_ascii());
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
                        LayoutKind::NavDestination(..) | LayoutKind::NavDestinationSelected(..)
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
    #[test]
    fn service_rows_fit_and_select_their_provider_at_every_text_size() {
        // Exercise the 300ppi panel geometries in portrait and logical landscape.
        // The Elipsa 227ppi profile is verified in its own native-renderer
        // process: an installed real-font typesetter retains its initial PPI.
        for (width, height, pixels_per_inch) in [
            (1072, 1448, 300),
            (1448, 1072, 300),
            (1264, 1680, 300),
            (1680, 1264, 300),
        ] {
            for scale in kobo_ui::TextScale::STEPS {
                let metrics = kobo_ui::DisplayMetrics {
                    text_scale: scale,
                    width,
                    height,
                    pixels_per_inch,
                };
                let mut runner = kobo_sdk::AppRunner::with_metrics(Chat::default(), metrics);
                for (index, provider) in PROVIDERS.iter().enumerate() {
                    runner.app_mut().view = View::Choosing;
                    let screen = runner.app().screen().with_own_back(true);
                    let chrome = kobo_ui::Chrome::for_screen(
                        &screen,
                        false,
                        kobo_ui::Chrome::measuring(true).status,
                    );
                    let diagnostics = screen.diagnostics(&metrics, &chrome);
                    assert!(
                        !diagnostics.has_errors(),
                        "{width}x{height} {pixels_per_inch}ppi {scale:?}: {:?}",
                        diagnostics.issues
                    );
                    let action = action_id(CHOICES[index]);
                    let rect = diagnostics
                        .layout
                        .rect_of_action(action)
                        .expect("provider row");
                    assert_eq!(
                        diagnostics
                            .layout
                            .hit_test(rect.x + rect.width / 2, rect.y + rect.height / 2),
                        Some(action)
                    );
                    let commands = runner.action(action);
                    assert_eq!(runner.app().provider, *provider);
                    assert!(commands.iter().any(|command| matches!(command,Command::Store(StoreRequest::Save{key,value}) if key==CHOSEN && value==provider.key().as_bytes())));
                }
            }
        }
    }
}

#[cfg(test)]
mod large_text_tests {
    use super::*;

    fn panels() -> impl Iterator<Item = kobo_sdk::DisplayMetrics> {
        [(1072, 1448, 300), (1264, 1680, 300), (1404, 1872, 227)]
            .into_iter()
            .flat_map(|(width, height, pixels_per_inch)| {
                [kobo_ui::TextScale::Default, kobo_ui::TextScale::Largest]
                    .into_iter()
                    .map(move |text_scale| kobo_sdk::DisplayMetrics {
                        width,
                        height,
                        pixels_per_inch,
                        text_scale,
                    })
            })
    }
    fn fits(screen: &Screen, metrics: kobo_sdk::DisplayMetrics) {
        let diagnostics = screen.diagnostics(&metrics, &kobo_sdk::Chrome::measuring(true));
        assert!(
            !diagnostics.has_errors(),
            "{metrics:?}: {:#?}",
            diagnostics.issues
        );
    }

    #[test]
    fn seeded_conversation_keeps_every_turn_on_measured_pages() {
        for metrics in panels() {
            let mut app = Chat::default();
            for index in 0..8 {
                app.conversation
                    .push(Role::You, format!("question {index}"));
                app.conversation.push(Role::Assistant,
                    "A reply long enough to wrap onto more than one line of a panel that is only a few inches across, which is the whole point.");
            }
            let runner = kobo_sdk::AppRunner::with_metrics(app, metrics);
            let pages = runner.app().measured_pages(&runner.context());
            assert_eq!(
                pages
                    .iter()
                    .flatten()
                    .map(|part| part.turn)
                    .collect::<Vec<_>>(),
                (0..16).collect::<Vec<_>>()
            );
            for page in 0..pages.len() {
                fits(&runner.app().transcript_page(&pages, page), metrics);
            }
        }
    }
}

#[cfg(test)]
mod long_turn_regression_tests {
    use super::*;
    #[test]
    fn long_single_turn_fits_every_page() {
        for role in [Role::You, Role::Assistant] {
            let metrics = kobo_sdk::DisplayMetrics {
                text_scale: kobo_ui::TextScale::Largest,
                ..kobo_ui::CLARA_BW_METRICS
            };
            let mut app = Chat::default();
            app.conversation.push(
                role,
                "A normal long answer with explanatory prose. ".repeat(40),
            );
            let runner = kobo_sdk::AppRunner::with_metrics(app, metrics);
            let pages = runner.app().measured_pages(&runner.context());
            assert!(pages.len() > 1);
            let expected = display_text(&runner.app().conversation.turns()[0]);
            let actual: String = pages
                .iter()
                .flatten()
                .map(|part| part.text.as_str())
                .collect();
            assert_eq!(
                actual, expected,
                "pagination must preserve the entire displayed turn"
            );
            let mut offset = 0;
            let boundaries: Vec<_> = expected
                .grapheme_indices(true)
                .map(|(index, _)| index)
                .chain([expected.len()])
                .collect();
            for part in pages.iter().flatten() {
                assert!(boundaries.contains(&offset));
                offset += part.text.len();
            }
            assert_eq!(offset, expected.len());
            for page in 0..pages.len() {
                let screen = runner.app().transcript_page(&pages, page);
                assert!(
                    !screen
                        .diagnostics(&metrics, &kobo_ui::Chrome::measuring(true))
                        .has_errors(),
                    "page {page}: {:?}",
                    screen
                        .diagnostics(&metrics, &kobo_ui::Chrome::measuring(true))
                        .issues
                );
            }
        }
    }

    #[test]
    fn long_turns_preserve_graphemes_and_latest_controls() {
        let metrics = kobo_sdk::DisplayMetrics {
            text_scale: kobo_ui::TextScale::Largest,
            ..kobo_ui::CLARA_BW_METRICS
        };
        for state in [View::Talking, View::Waiting] {
            let mut app = Chat {
                view: state,
                ..Chat::default()
            };
            app.conversation.push(
                Role::Assistant,
                format!(
                    "{}\n{{\"options\":[\"Continue\",\"Explain\"]}}",
                    "Words and e\u{301} and \u{1f469}\u{1f3fd}\u{200d}\u{1f4bb}. ".repeat(80)
                ),
            );
            let runner = kobo_sdk::AppRunner::with_metrics(app, metrics);
            let pages = runner.app().measured_pages(&runner.context());
            let expected = display_text(&runner.app().conversation.turns()[0]);
            assert_eq!(
                pages
                    .iter()
                    .flatten()
                    .map(|part| part.text.as_str())
                    .collect::<String>(),
                expected
            );
            let boundaries: Vec<_> = expected
                .grapheme_indices(true)
                .map(|(index, _)| index)
                .chain([expected.len()])
                .collect();
            let mut offset = 0;
            for part in pages.iter().flatten() {
                assert!(boundaries.contains(&offset));
                offset += part.text.len();
            }
            for back in 0..pages.len() {
                let screen = runner.app().transcript_page(&pages, back);
                let layout = screen.layout_with(&metrics, &kobo_ui::Chrome::measuring(true));
                let action = if state == View::Waiting {
                    CANCEL
                } else {
                    OPTIONS[0]
                };
                assert_eq!(
                    layout.rect_of_action(action_id(action)).is_some(),
                    back == 0
                );
                // Unsupported emoji remain explicit renderer diagnostics, not
                // a reason to split every grapheme into a separate page.
                assert!(!screen
                    .diagnostics(&metrics, &kobo_ui::Chrome::measuring(true))
                    .issues
                    .iter()
                    .any(|issue| matches!(
                        issue.kind,
                        kobo_sdk::LayoutIssueKind::Clipped
                            | kobo_sdk::LayoutIssueKind::TextOverflow
                            | kobo_sdk::LayoutIssueKind::ContentOverflow { .. }
                            | kobo_sdk::LayoutIssueKind::InteractiveOffscreen
                    )));
            }
            assert!(pages.len() < 80);
        }
    }
}

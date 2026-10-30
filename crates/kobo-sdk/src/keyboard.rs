//! Typing, for the few things that genuinely cannot be tapped.
//!
//! ## The runtime's keyboard
//!
//! From protocol 16 the runtime draws the keyboard. A [`TextEntry`] asks it
//! for text with [`DeviceRequest::TypeText`] when it is drawn open, and hands
//! the answer back through [`TextEntry::handle`], the same call an application
//! already makes for every action it receives. The runtime takes the panel,
//! draws the field and the keys in its own type, reads the taps itself and
//! answers once.
//!
//! That moved three things out of every application. A key added to the
//! keyboard reaches every application with the next platform release, where it
//! used to reach one Store release at a time. Keystrokes no longer travel to
//! the application and back as whole screens. And a secret field, set up with
//! [`TextEntry::saved_as`], is written straight into the application's own
//! secret namespace: the application hears that it was saved and never what it
//! was.
//!
//! ## The keyboard an application draws
//!
//! [`Keyboard`] and [`ScreenBuilder::keyboard`] still draw keys inside an
//! application, from the same rows the runtime uses, for a screen that has to
//! see every key as it is tapped: a terminal sends each keystroke to a program
//! the moment it happens, and a game keypad is part of its board. Anything that
//! only wants a line of text back should use a [`TextEntry`].

use crate::{action_id, ActionId, DeviceRequest, DeviceResult, ScreenBuilder};
use kobo_protocol::{SecretTarget, TextOutcome};
use kobo_ui::FieldKind;

pub(crate) use kobo_ui::keyboard::{key_name, BACKSPACE, ENTER, LAYER, SHIFT, SPACE};
pub use kobo_ui::keyboard::{Keyboard, Layer, Pressed, MAX_TEXT};

/// The action an answered field arrives as.
///
/// An application already hands every action to its fields first, so the
/// answer comes the same way rather than through a callback each application
/// would have had to add.
const ANSWERED: &str = "kb.answered";

/// The cancel action of the keyboard applications used to draw. Still
/// claimed, so an application that put a Cancel of its own under a field
/// keeps the behaviour it had.
const CANCEL: &str = kobo_ui::keyboard::CANCEL;

/// What a tap did to a [`TextEntry`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Typing {
    /// The field opened, changed, or a modifier moved. Repaint.
    Changed,
    /// The reader accepted what they typed. The text comes with it, and the
    /// field is closed and empty again, so the next entry starts clean.
    Submitted(String),
    /// The reader backed out. Nothing was entered.
    Cancelled,
    /// A secret was typed and the runtime saved it where
    /// [`TextEntry::saved_as`] said. What it was never reaches the
    /// application.
    Saved,
    /// The runtime would not keep the secret. The answer says why, as
    /// [`DeviceResult::Denied`] or [`DeviceResult::Failed`].
    Refused(DeviceResult),
}

/// A field that raises the keyboard when it is tapped, and puts it away again.
///
/// This exists because "tap the row, get a keyboard, get the text back" was
/// being written out by hand in every application, and every application got a
/// slightly different one. The row an application draws with
/// [`ScreenBuilder::or_type`] emits an action like any other; binding that
/// action here is what makes the row open the keyboard, rather than each
/// author remembering to switch a view enum.
///
/// The application still owns its screens: this decides *whether* the field
/// is open, and [`ScreenBuilder::text_entry`] asks the runtime for it.
#[derive(Clone, Debug, Default)]
pub struct TextEntry {
    keyboard: Keyboard,
    open: bool,
    opens_on: Option<ActionId>,
    kind: FieldKind,
    save_as: Option<SecretTarget>,
    /// Which opening of this field the runtime is answering. A new one per
    /// opening, so an answer meant for a field closed in the meantime is not
    /// taken by the next one.
    opening: u64,
}

impl TextEntry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Binds the action that opens this field.
    ///
    /// Named rather than numbered, and resolved the same way every other
    /// action name is, so it is the same string passed to
    /// [`ScreenBuilder::or_type`].
    #[must_use]
    pub fn opened_by(mut self, name: &str) -> Self {
        self.opens_on = Some(crate::action_id(name));
        self
    }

    /// Opens this field on the digits every time, for a value that is a
    /// number. The letters stay one tap away on `abc`, because an ID pasted
    /// from a page can still carry a stray letter the reader needs to match.
    #[must_use]
    pub fn numeric(self) -> Self {
        self.kind(FieldKind::Number)
    }

    /// Opens this field with `@`, `/` and `.` beside the letters and no space
    /// bar, for a web or mail address.
    #[must_use]
    pub fn address(self) -> Self {
        self.kind(FieldKind::Address)
    }

    /// Chooses the keys this field opens on. A secret needs somewhere to go
    /// as well, so it is set up with [`Self::saved_as`] instead.
    #[must_use]
    pub fn kind(mut self, kind: FieldKind) -> Self {
        if kind != FieldKind::Secret {
            self.kind = kind;
            self.save_as = None;
            self.keyboard = self.fresh();
        }
        self
    }

    /// Makes this a secret field whose value the runtime keeps under
    /// `target`, in this application's own namespace.
    ///
    /// The value is never handed back. The field answers
    /// [`Typing::Saved`], and requests that need the secret name it the way
    /// they name any other runtime-held credential. The name is held to the
    /// same review as [`crate::Context::secrets`], before the owner types a
    /// word of it.
    #[must_use]
    pub fn saved_as(mut self, target: SecretTarget) -> Self {
        self.set_target(target);
        self
    }

    /// The same as [`Self::saved_as`], for a field that is already built,
    /// such as a password whose username was typed on the screen before.
    pub fn set_target(&mut self, target: SecretTarget) {
        self.kind = FieldKind::Secret;
        self.save_as = Some(target);
        self.keyboard = self.fresh();
    }

    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.open
    }

    /// The text the field opened with. The runtime holds what is typed after
    /// that until the owner is done.
    #[must_use]
    pub fn text(&self) -> &str {
        self.keyboard.text()
    }

    #[must_use]
    pub const fn keyboard(&self) -> &Keyboard {
        &self.keyboard
    }

    /// Opens the field with nothing in it, on the letters or, for a
    /// [`Self::numeric`] field, on the digits.
    pub fn open(&mut self) {
        self.keyboard = self.fresh();
        self.open = true;
        self.opening = runtime::next_opening();
    }

    /// Opens the field with `text` already in it, for editing something.
    ///
    /// A secret field always opens empty: the application does not hold the
    /// secret, so it has nothing to put in it.
    pub fn open_with(&mut self, text: impl Into<String>) {
        self.open();
        if self.kind != FieldKind::Secret {
            self.keyboard = self.fresh().typed(text);
        }
    }

    /// Closes the field and throws away anything typed.
    pub fn close(&mut self) {
        runtime::forget(self.opening);
        self.keyboard = self.fresh();
        self.open = false;
    }

    fn fresh(&self) -> Keyboard {
        Keyboard::for_kind(self.kind)
    }

    /// Applies `action` if it belongs to this field.
    ///
    /// Returns `None` for anything else, so an application passes every action
    /// here first and handles its own afterwards. While the field is open it
    /// claims the cancel action too, because a field covering the panel is
    /// modal whether or not the author thought of it that way.
    pub fn handle(&mut self, action: ActionId) -> Option<Typing> {
        self.handle_text(action, true)
    }

    /// Handle private values without trimming meaningful password whitespace.
    /// Empty input cancels; a nonempty value is returned exactly as entered.
    pub fn handle_verbatim(&mut self, action: ActionId) -> Option<Typing> {
        self.handle_text(action, false)
    }

    fn handle_text(&mut self, action: ActionId, trim: bool) -> Option<Typing> {
        if !self.open {
            if self.opens_on == Some(action) {
                self.open();
                return Some(Typing::Changed);
            }
            return None;
        }
        if action == action_id(CANCEL) {
            self.close();
            return Some(Typing::Cancelled);
        }
        if action != action_id(ANSWERED) {
            return None;
        }
        let answer = runtime::collect(self.opening)?;
        self.keyboard = self.fresh();
        self.open = false;
        Some(match answer {
            TextOutcome::Typed(text) => {
                // Whitespace only is nothing. Returning it would make every
                // caller check, and half of them would forget.
                let text = if trim { text.trim().to_owned() } else { text };
                if text.is_empty() {
                    Typing::Cancelled
                } else {
                    Typing::Submitted(text)
                }
            }
            TextOutcome::Saved => Typing::Saved,
            TextOutcome::Cancelled => Typing::Cancelled,
            TextOutcome::Denied(reason) => Typing::Refused(DeviceResult::Denied(reason)),
            TextOutcome::Failed(error) => Typing::Refused(DeviceResult::Failed(error)),
        })
    }

    /// Asks the runtime for this field, if it is open, when a screen drawn
    /// with it is sent.
    fn ask(&self, prompt: &str, hint: &str, submit: &str) {
        if !self.open {
            return;
        }
        // A field the runtime refuses to draw is a field nobody can type
        // into, so what an application hands over is made fit to send rather
        // than sent as it is and refused.
        let prompt = bounded(prompt, kobo_protocol::MAX_TEXT_PROMPT_LEN);
        let submit = bounded(submit, kobo_protocol::MAX_TEXT_SUBMIT_LEN);
        runtime::draw(
            self.opening,
            DeviceRequest::TypeText {
                prompt: if prompt.trim().is_empty() {
                    "Type here".to_owned()
                } else {
                    prompt
                },
                hint: bounded(hint, kobo_protocol::MAX_TEXT_HINT_LEN),
                initial: bounded(self.keyboard.text(), kobo_protocol::MAX_TYPED_LEN),
                submit: if submit.trim().is_empty() {
                    "Done".to_owned()
                } else {
                    submit
                },
                kind: self.kind,
                save_as: self.save_as.clone(),
            },
        );
    }
}

/// `text` on one line and cut to `limit` bytes at a character boundary, so a
/// prompt written for a wider screen still asks for its field rather than
/// being refused by the wire.
fn bounded(text: &str, limit: usize) -> String {
    let mut line: String = text
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect();
    if line.len() > limit {
        let mut end = limit;
        while !line.is_char_boundary(end) {
            end -= 1;
        }
        line.truncate(end);
    }
    line
}

/// The fields drawn open and the runtime's answers to them.
///
/// Thread-local because the two halves meet nowhere else. A field asks when a
/// screen drawn with it is sent, through [`crate::Context::set_screen`], and
/// is answered through [`TextEntry::handle`]; neither call is handed anything
/// that could carry the request or the answer between them, and changing
/// either would have meant changing every application that types. An
/// application runs on one thread, so this is one application's fields.
pub(crate) mod runtime {
    use crate::DeviceRequest;
    use kobo_protocol::TextOutcome;
    use std::cell::RefCell;
    use std::collections::VecDeque;

    #[derive(Default)]
    struct Fields {
        next: u64,
        /// The newest field drawn open whose request has not been sent.
        drawn: Option<(u64, DeviceRequest)>,
        /// Fields the runtime holds a request for, oldest first. The runtime
        /// answers in the order it was asked, so the oldest is the one the
        /// next answer is for.
        asked: VecDeque<u64>,
        /// Answers waiting for their field to collect them.
        answered: Vec<(u64, TextOutcome)>,
        /// Fields closed while the runtime still held them. Their answers
        /// are dropped as they arrive rather than kept for nobody.
        closed: Vec<u64>,
    }

    thread_local! {
        static FIELDS: RefCell<Fields> = RefCell::new(Fields::default());
    }

    pub(super) fn next_opening() -> u64 {
        FIELDS.with_borrow_mut(|fields| {
            fields.next += 1;
            fields.next
        })
    }

    pub(super) fn draw(opening: u64, request: DeviceRequest) {
        FIELDS.with_borrow_mut(|fields| fields.drawn = Some((opening, request)));
    }

    /// The request for a field drawn open since the last screen was sent,
    /// unless the runtime already has it. Screens are drawn again on every
    /// change, and the field is asked for once.
    pub(crate) fn take_drawn() -> Option<DeviceRequest> {
        FIELDS.with_borrow_mut(|fields| {
            let (opening, request) = fields.drawn.take()?;
            if fields.asked.contains(&opening) {
                return None;
            }
            fields.asked.push_back(opening);
            Some(request)
        })
    }

    pub(super) fn forget(opening: u64) {
        FIELDS.with_borrow_mut(|fields| {
            if fields
                .drawn
                .as_ref()
                .is_some_and(|(drawn, _)| *drawn == opening)
            {
                fields.drawn = None;
            }
            fields.answered.retain(|(answered, _)| *answered != opening);
            if fields.asked.contains(&opening) && !fields.closed.contains(&opening) {
                fields.closed.push(opening);
            }
        });
    }

    /// Files the runtime's answer under the field that asked first. False
    /// when no field is waiting for one.
    pub(crate) fn answer(outcome: TextOutcome) -> bool {
        FIELDS.with_borrow_mut(|fields| {
            let Some(opening) = fields.asked.pop_front() else {
                return false;
            };
            if let Some(at) = fields.closed.iter().position(|closed| *closed == opening) {
                fields.closed.remove(at);
            } else {
                fields.answered.push((opening, outcome));
            }
            true
        })
    }

    /// The answer for the newest field, as the runtime gives it: every older
    /// field still waiting is answered as cancelled first, because the
    /// runtime cancels a field when a newer one replaces it. Says how many
    /// answers that made, none when no field is waiting.
    pub(crate) fn answer_newest(outcome: TextOutcome) -> usize {
        let waiting = FIELDS.with_borrow(|fields| fields.asked.len());
        for _ in 1..waiting {
            answer(TextOutcome::Cancelled);
        }
        if waiting > 0 {
            answer(outcome);
        }
        waiting
    }

    pub(super) fn collect(opening: u64) -> Option<TextOutcome> {
        FIELDS.with_borrow_mut(|fields| {
            let at = fields
                .answered
                .iter()
                .position(|(answered, _)| *answered == opening)?;
            Some(fields.answered.remove(at).1)
        })
    }
}

/// The action an answered field is delivered as.
pub(crate) fn answered() -> ActionId {
    action_id(ANSWERED)
}

impl ScreenBuilder {
    /// Draws `keyboard`, and the text typed on it so far, inside the
    /// application.
    ///
    /// For a screen that has to see every key, such as a terminal or a game
    /// keypad. A field that only wants text back is a [`TextEntry`], which
    /// the runtime draws.
    #[must_use]
    pub fn keyboard(self, keyboard: &Keyboard, submit: &str) -> Self {
        // The keys go to the foot of the panel wherever they are added, which
        // is where thumbs are and where every other platform puts them. In
        // flow they landed wherever the prompt above them ended: the todo
        // application's compose screen drew its keyboard across the middle of
        // the panel with a third of a page of paper under it. A second `fill`
        // above this one, as `terminal_keys` has, changes nothing, because a
        // fill only ever pushes down.
        let mut screen = self.fill();
        for row in keyboard.keys(submit) {
            let columns = u8::try_from(row.len()).unwrap_or(u8::MAX);
            screen = screen.grid_with_selection(
                columns,
                false,
                row.iter()
                    .map(|key| (key.name.as_str(), key.label.clone(), key.selected)),
            );
            if let Some(kobo_ui::Node::Grid { cells, .. }) = screen.nodes.last_mut() {
                for (cell, key) in cells.iter_mut().zip(&row) {
                    if key.glyph.is_some() {
                        cell.glyph = key.glyph;
                    }
                }
            }
        }
        screen
    }

    /// Draws an open [`TextEntry`], and asks the runtime for it when this
    /// screen is sent.
    ///
    /// What is drawn here is the application's own screen, under the
    /// runtime's field: the prompt and what the field opened with. The
    /// runtime keeps this screen's title in its top bar, so the owner can see
    /// which application is asking while they type.
    #[must_use]
    pub fn text_entry(self, entry: &TextEntry, prompt: &str, submit: &str) -> Self {
        self.text_entry_with_hint(entry, prompt, "", submit)
    }

    /// [`Self::text_entry`], with one line of guidance the runtime shows
    /// under the prompt while the owner types: where a token comes from, how
    /// long it is, or what the last entry did.
    #[must_use]
    pub fn text_entry_with_hint(
        self,
        entry: &TextEntry,
        prompt: &str,
        hint: &str,
        submit: &str,
    ) -> Self {
        entry.ask(prompt, hint, submit);
        let screen = self.heading(prompt);
        let screen = if hint.trim().is_empty() {
            screen
        } else {
            screen.secondary(hint)
        };
        if entry.kind == FieldKind::Secret {
            screen.text("Typing on the reader\u{2019}s keyboard.")
        } else {
            screen.typed(entry.keyboard(), "Type here")
        }
    }

    /// Draws a secret field. The same as [`Self::text_entry`] for an entry
    /// made with [`TextEntry::saved_as`], which is the only kind of entry a
    /// secret can be typed into.
    #[must_use]
    pub fn secret_entry(self, entry: &TextEntry, prompt: &str, submit: &str) -> Self {
        debug_assert!(
            entry.kind == FieldKind::Secret,
            "a secret field needs TextEntry::saved_as to say where the secret goes"
        );
        self.text_entry(entry, prompt, submit)
    }

    /// Indicates whether a credential has been entered without echoing it.
    #[must_use]
    pub fn secret_typed(self, keyboard: &Keyboard, placeholder: &str) -> Self {
        if keyboard.is_empty() {
            self.text(placeholder)
        } else {
            self.text("Credential entered")
        }
    }

    /// Shows what has been typed, or `placeholder` when nothing has been.
    ///
    /// An empty field draws nothing at all, which on a panel with no cursor
    /// reads as a broken screen rather than an empty one.
    #[must_use]
    pub fn typed(self, keyboard: &Keyboard, placeholder: &str) -> Self {
        if keyboard.is_empty() {
            self.text(placeholder)
        } else {
            self.text(keyboard.text())
        }
    }
}

/// Answering the runtime's text fields from a test.
///
/// An application's tests have no runtime to draw the field, so they answer
/// it here, as the runtime would have: the newest field the application
/// asked for gets the answer, delivered through
/// [`crate::KoboApp::on_action`] the way a real one is.
pub mod testing {
    use super::runtime;
    use crate::{Command, Context, DeviceRequest, KoboApp};
    use kobo_protocol::TextOutcome;

    /// Types `text` into the field the application has open and submits it.
    ///
    /// # Panics
    ///
    /// When the application has no field open, which is a test that typed
    /// before tapping whatever opens one.
    pub fn type_text<A: KoboApp>(app: &mut A, context: &mut Context, text: &str) {
        answer(app, context, TextOutcome::Typed(text.to_owned()));
    }

    /// Backs out of the field the application has open.
    ///
    /// # Panics
    ///
    /// When the application has no field open.
    pub fn cancel_text<A: KoboApp>(app: &mut A, context: &mut Context) {
        answer(app, context, TextOutcome::Cancelled);
    }

    /// Answers the field the application has open as the runtime would,
    /// including [`TextOutcome::Saved`] for a secret.
    ///
    /// # Panics
    ///
    /// When the application has no field open.
    pub fn answer<A: KoboApp>(app: &mut A, context: &mut Context, outcome: TextOutcome) {
        let waiting = runtime::answer_newest(outcome);
        assert!(waiting > 0, "no text field is open to answer");
        for _ in 0..waiting {
            app.on_action(context, super::answered());
        }
    }

    /// The text field the application asked for most recently in these
    /// commands, if it asked for one.
    #[must_use]
    pub fn asked(commands: &[Command]) -> Option<&DeviceRequest> {
        commands.iter().rev().find_map(|command| match command {
            Command::Device(request @ DeviceRequest::TypeText { .. }) => Some(request),
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::testing;
    use super::{Keyboard, Pressed, TextEntry, Typing};
    use crate::{action_id, Command, Context, DeviceRequest, DeviceResult, KoboApp, ScreenBuilder};
    use kobo_protocol::{DenyReason, SecretTarget, TextOutcome};
    use kobo_ui::{Chrome, DisplayMetrics, FieldKind, LayoutKind, TextScale};

    /// The pages a reader can reach from a fresh keyboard, as the taps that
    /// reach them.
    const PAGES: [&[&str]; 4] = [&[], &["kb.shift"], &["kb.layer"], &["kb.layer", "kb.shift"]];

    fn panels() -> Vec<(String, DisplayMetrics)> {
        kobo_profile::SUPPORTED_PROFILES
            .iter()
            .flat_map(|profile| {
                let portrait = DisplayMetrics {
                    width: i32::try_from(profile.width).expect("profile width fits layout"),
                    height: i32::try_from(profile.height).expect("profile height fits layout"),
                    pixels_per_inch: i32::from(profile.pixels_per_inch),
                    text_scale: TextScale::Default,
                };
                let landscape = DisplayMetrics {
                    width: portrait.height,
                    height: portrait.width,
                    ..portrait
                };
                [
                    (format!("{} portrait", profile.id), portrait),
                    (format!("{} landscape", profile.id), landscape),
                ]
            })
            .collect()
    }

    /// The smallest application that types: a field opened by a row, and
    /// whatever came back from it.
    #[derive(Default)]
    struct Notes {
        entry: TextEntry,
        heard: Vec<Typing>,
    }

    impl KoboApp for Notes {
        fn on_start(&mut self, _: &mut Context) {}

        fn on_action(&mut self, context: &mut Context, action: crate::ActionId) {
            if let Some(typing) = self.entry.handle(action) {
                self.heard.push(typing);
            }
            let screen = ScreenBuilder::new("notes").top_bar("Notes");
            let screen = if self.entry.is_open() {
                screen.text_entry(&self.entry, "New note", "Add")
            } else {
                screen.button("add", "Add a note")
            };
            context.set_screen(screen.build());
        }
    }

    fn notes() -> Notes {
        Notes {
            entry: TextEntry::new().opened_by("add"),
            heard: Vec::new(),
        }
    }

    #[test]
    fn an_open_field_asks_the_runtime_once_however_often_it_is_drawn() {
        let mut app = notes();
        let mut context = Context::default();
        app.on_action(&mut context, action_id("add"));
        let commands = context.take_commands();
        let Some(DeviceRequest::TypeText {
            prompt,
            submit,
            kind,
            save_as,
            ..
        }) = testing::asked(&commands)
        else {
            panic!("opening the field asks for it: {commands:?}")
        };
        assert_eq!((prompt.as_str(), submit.as_str()), ("New note", "Add"));
        assert_eq!((*kind, save_as.clone()), (FieldKind::Text, None));
        assert!(
            matches!(commands.first(), Some(Command::Device(_))),
            "asked before its screen is sent, so the runtime never paints the screen under it"
        );

        app.on_action(&mut context, action_id("something-else"));
        assert!(
            testing::asked(&context.take_commands()).is_none(),
            "drawn again, not asked again"
        );

        testing::type_text(&mut app, &mut context, "  milk  ");
        assert_eq!(app.heard.last(), Some(&Typing::Submitted("milk".into())));
        assert!(!app.entry.is_open());
    }

    #[test]
    fn backing_out_of_the_runtimes_field_cancels_and_reopening_asks_again() {
        let mut app = notes();
        let mut context = Context::default();
        app.on_action(&mut context, action_id("add"));
        testing::cancel_text(&mut app, &mut context);
        assert_eq!(app.heard.last(), Some(&Typing::Cancelled));
        let _ = context.take_commands();
        app.on_action(&mut context, action_id("add"));
        assert!(testing::asked(&context.take_commands()).is_some());
        testing::type_text(&mut app, &mut context, "   ");
        assert_eq!(
            app.heard.last(),
            Some(&Typing::Cancelled),
            "nothing but spaces is nothing"
        );
    }

    #[test]
    fn an_answer_for_a_field_closed_in_the_meantime_is_not_taken_by_the_next() {
        let mut app = notes();
        let mut context = Context::default();
        app.on_action(&mut context, action_id("add"));
        app.entry.close();
        app.entry.open();
        app.on_action(&mut context, action_id("redraw"));
        // The runtime cancels the first field when the second arrives, then
        // answers the second.
        testing::type_text(&mut app, &mut context, "second");
        assert_eq!(
            app.heard,
            [Typing::Changed, Typing::Submitted("second".into())],
            "the cancelled first field is not heard as the second one's answer"
        );
    }

    #[test]
    fn a_secret_field_names_where_it_goes_and_hears_only_that_it_was_saved() {
        let mut entry = TextEntry::new().saved_as(SecretTarget::named("lichess"));
        entry.open_with("an old token");
        assert_eq!(entry.text(), "", "a secret field opens empty");
        let mut context = Context::default();
        context.set_screen(
            ScreenBuilder::new("token")
                .secret_entry(&entry, "Account key", "Save")
                .build(),
        );
        let commands = context.take_commands();
        let Some(DeviceRequest::TypeText {
            kind,
            save_as,
            initial,
            ..
        }) = testing::asked(&commands)
        else {
            panic!("a secret field asks for one")
        };
        assert_eq!(*kind, FieldKind::Secret);
        assert_eq!(
            save_as.as_ref().map(|target| target.name.as_str()),
            Some("lichess")
        );
        assert!(initial.is_empty());
        assert!(super::runtime::answer(TextOutcome::Saved));
        assert_eq!(
            entry.handle_verbatim(super::answered()),
            Some(Typing::Saved)
        );

        entry.open();
        context.set_screen(
            ScreenBuilder::new("token")
                .secret_entry(&entry, "Account key", "Save")
                .build(),
        );
        super::runtime::answer(TextOutcome::Denied(DenyReason::NotDeclared));
        assert_eq!(
            entry.handle_verbatim(super::answered()),
            Some(Typing::Refused(DeviceResult::Denied(
                DenyReason::NotDeclared
            )))
        );
    }

    #[test]
    fn a_runner_answers_the_field_through_the_action_the_application_already_handles() {
        let mut runner = crate::AppRunner::new(notes());
        runner.start();
        let commands = runner.action(action_id("add"));
        assert!(testing::asked(&commands).is_some());
        assert_eq!(
            runner.outstanding_requests(),
            1,
            "the field is outstanding until it is answered"
        );
        let after = runner.type_text("eggs");
        assert_eq!(
            runner.app().heard.last(),
            Some(&Typing::Submitted("eggs".into()))
        );
        assert_eq!(runner.outstanding_requests(), 0);
        assert!(
            after
                .iter()
                .any(|command| matches!(command, Command::SetScreen(_))),
            "the screen after the field is sent even if it is the one from before"
        );
    }

    #[test]
    fn a_kind_chooses_the_keys_and_a_secret_cannot_be_chosen_without_a_home() {
        assert_eq!(
            TextEntry::new().numeric().keyboard().kind(),
            FieldKind::Number
        );
        assert_eq!(
            TextEntry::new().address().keyboard().kind(),
            FieldKind::Address
        );
        assert_eq!(
            TextEntry::new().kind(FieldKind::Secret).keyboard().kind(),
            FieldKind::Text,
            "a secret needs saved_as"
        );
    }

    #[test]
    fn a_prompt_too_long_for_the_wire_is_cut_rather_than_refused() {
        let long = "é".repeat(kobo_protocol::MAX_TEXT_PROMPT_LEN);
        let cut = super::bounded(&long, kobo_protocol::MAX_TEXT_PROMPT_LEN);
        assert!(cut.len() <= kobo_protocol::MAX_TEXT_PROMPT_LEN);
        assert!(cut.chars().all(|character| character == 'é'));
        assert_eq!(super::bounded("two\nlines", 64), "two lines");
    }

    /// A keyboard drawn inside an application belongs under the thumbs.
    #[test]
    fn an_application_drawn_keyboard_sits_at_the_foot_of_the_panel() {
        let screen = ScreenBuilder::new("terminal")
            .top_bar("Terminal")
            .text("$ ")
            .keyboard(&Keyboard::new(), "enter")
            .build();
        for (name, metrics) in panels() {
            let layout = screen.layout_with(&metrics, &Chrome::measuring(true));
            let bottom = layout
                .nodes
                .iter()
                .map(|node| node.rect.y + node.rect.height)
                .max()
                .unwrap_or(0);
            let content = layout.content;
            let spare = content.y + content.height - bottom;
            assert!(
                (0..=metrics.touch_target_default()).contains(&spare),
                "{name}: the keyboard left {spare} pixels of paper under it"
            );
        }
    }

    #[test]
    fn every_drawn_key_is_reachable_by_a_tap_at_its_centre() {
        // The keyboard is drawn by one function and decoded by another, and
        // nothing but this test stops them drifting apart. It lays the real
        // screen out and taps the middle of every cell.
        for (name, metrics) in panels() {
            for page in PAGES {
                let mut drawn = Keyboard::new();
                for key in page {
                    drawn.press(action_id(key));
                }
                let screen = ScreenBuilder::new("keyboard")
                    .keyboard(&drawn, "Send")
                    .build();
                let layout = screen.layout_with(&metrics, &Chrome::default());
                let cells = layout
                    .nodes
                    .iter()
                    .filter_map(|node| match node.kind {
                        LayoutKind::Cell(action, ..) => Some((action, node.rect)),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                assert!(!cells.is_empty(), "{name}: the keyboard drew no keys");
                let mut typed = String::new();
                for (action, rect) in &cells {
                    assert!(
                        rect.width >= metrics.touch_target_minimum()
                            && rect.height >= metrics.touch_target_minimum(),
                        "{name}: key {action:?} is smaller than a touch target"
                    );
                    let x = rect.x + rect.width / 2;
                    let y = rect.y + rect.height / 2;
                    let hit = layout.hit_test(x, y).expect("a tap inside a key hits it");
                    assert_eq!(
                        hit, *action,
                        "{name}: a key was covered by something drawn after it"
                    );
                    let mut keyboard = drawn.clone();
                    let outcome = keyboard.press(hit).expect("every drawn key decodes");
                    if outcome == Pressed::Edited {
                        typed.push_str(keyboard.text());
                    }
                }
                for character in drawn.rows().concat().chars() {
                    let character = if drawn.is_shifted() {
                        character.to_ascii_uppercase()
                    } else {
                        character
                    };
                    assert!(
                        typed.contains(character),
                        "{name}: {character} is drawn but cannot be typed"
                    );
                }
                assert!(typed.contains(' '), "{name}: the space bar types a space");
            }
        }
    }
}

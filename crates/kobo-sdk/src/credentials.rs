//! Consistent owner onboarding for runtime-held application credentials.
//!
//! The account key or password is typed on the runtime's keyboard, and the
//! runtime writes it into this application's own secret namespace. The flow
//! hears that it was saved and never what it was, so a Lichess token or a
//! server password passes through nothing but the runtime that keeps it. A
//! username is typed here like any other text, because it is shown on screen
//! anyway and the runtime stores it in front of the password.
//!
//! The prompt is drawn as a labelled field, so where the key comes from sits
//! under the field as its hint and a refused save sits there as its error,
//! the way every other field on the platform explains itself. An app that
//! knows the page where the key is made names it with
//! [`CredentialSetup::with_key_page`], and the flow offers a QR code for a
//! phone, so the owner does not type an address on e-ink to find it.

use crate::keyboard::{TextEntry, Typing};
use crate::{
    action_id, ActionId, Context, DeviceError, DeviceRequest, DeviceResult, Screen, ScreenBuilder,
};
use kobo_protocol::SecretTarget;
use kobo_ui::{BannerLevel, FieldKind};
use std::fmt;

const ENTER: &str = "credential.enter";
const CLI: &str = "credential.cli";
const CHECK: &str = "credential.check";
const PHONE: &str = "credential.phone";

/// Whether a presence check is out, and who wanted it.
///
/// A check the owner asked for reports "not there yet" on screen. One the app
/// sent by itself on coming back to the front stays quiet when the account is
/// still missing, because the owner may not have done anything yet and a
/// warning they did not ask for reads as something having gone wrong.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Checking {
    Idle,
    Quiet,
    Asked,
    /// The runtime said it saved what the owner typed, and this check carries
    /// that news to [`CredentialSetup::on_device_result`].
    Saved,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum View {
    Closed,
    Prompt,
    /// The runtime's secret field is up for an account key.
    Entry,
    Username,
    /// The runtime's secret field is up for the password that goes with the
    /// username typed before it.
    Password,
    Cli,
    /// A QR code for the page where the key is made, for a phone to open.
    Phone,
    /// Saved by the runtime, and waiting for the check that says so.
    Saving,
}

/// Outcome from handling a credential setup interaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CredentialEvent {
    Changed,
    Saved,
    Cancelled,
    /// The owner asked to check again, and the runtime would not say whether
    /// the account is there. The flow has closed so the app can make one of
    /// its own requests: a request is the truest check there is, and its
    /// failure, read through [`crate::failure`], says what is missing.
    TryAccount,
}

/// A modal flow that installs one app-authorized runtime credential.
pub struct CredentialSetup {
    name: String,
    service: String,
    view: View,
    entry: TextEntry,
    /// Why what was typed was not saved. Drawn as the field's error, because
    /// it is about what went into the field.
    problem: Option<String>,
    /// What a presence check found. Drawn as a banner that carries Check
    /// again, because the fix is to look again once the account is there.
    check: Option<String>,
    basic: bool,
    username: String,
    server: Option<String>,
    guidance: Option<String>,
    computer: Option<String>,
    /// The page where the owner makes the key, and what to say under its code.
    key_page: Option<(String, String)>,
    /// Where this sign-in sits in the app's connect flow: the step it is,
    /// counted from zero, and the name of every step.
    steps: Option<(u8, Vec<String>)>,
    checking: Checking,
}

impl fmt::Debug for CredentialSetup {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CredentialSetup")
            .field("name", &self.name)
            .field("service", &self.service)
            .field("view", &self.view)
            .field("problem", &self.problem)
            .field("check", &self.check)
            .finish_non_exhaustive()
    }
}

impl Default for CredentialSetup {
    fn default() -> Self {
        Self::new("", "")
    }
}

impl CredentialSetup {
    #[must_use]
    pub fn new(name: impl Into<String>, service: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            service: service.into(),
            view: View::Closed,
            entry: TextEntry::new(),
            problem: None,
            check: None,
            basic: false,
            username: String::new(),
            server: None,
            guidance: None,
            computer: None,
            key_page: None,
            steps: None,
            checking: Checking::Idle,
        }
    }

    /// Name the command that sets this account up from a computer, in place
    /// of the generic `kobo secret set NAME`.
    ///
    /// An account bound to a server needs the server too, and typing both on
    /// the computer is the whole point of going there, so an app that has a
    /// guided command says which one. Naming one also offers the computer
    /// route on a server-bound flow, which otherwise has none.
    #[must_use]
    pub fn with_computer_command(mut self, command: impl Into<String>) -> Self {
        self.computer = Some(command.into());
        self
    }

    /// Say where this service's key comes from, in place of the generic line.
    ///
    /// "Add the account key for this service" is enough when the owner already
    /// holds one. A token that has to be created on the provider's site first
    /// needs that said on the screen that asks for it, because the reader has
    /// no browser to look it up in. The same line stays under the field while
    /// the key is typed.
    #[must_use]
    pub fn with_guidance(mut self, guidance: impl Into<String>) -> Self {
        self.guidance = Some(guidance.into());
        self
    }

    /// Name the page where the owner makes this key, so the flow can offer a
    /// QR code for a phone to open it.
    ///
    /// Typing a long address into a phone is easy; finding it on a reader
    /// that has no browser is not, and typing it off the panel is error
    /// prone. `page` is a whole address, or a path starting with `/` that is
    /// joined to the server bound with [`Self::bind_server`], since a
    /// self-hosted service's token page lives on the owner's own server. The
    /// code is offered only while the address fits in
    /// [`crate::MAX_CODE_BYTES`]. Never put a secret in it: a code on a
    /// panel can be read by anyone who can see the panel.
    #[must_use]
    pub fn with_key_page(mut self, page: impl Into<String>, caption: impl Into<String>) -> Self {
        self.key_page = Some((page.into(), caption.into()));
        self
    }

    /// Show where this sign-in sits in a longer connect flow, as a step
    /// indicator over the prompt.
    ///
    /// An app that asks for the server address first, then the key, then
    /// checks both, says so on every screen of the flow, so the owner knows
    /// how much is left. `current` counts from zero and `labels` names every
    /// step.
    #[must_use]
    pub fn with_steps<I, L>(mut self, current: u8, labels: I) -> Self
    where
        I: IntoIterator<Item = L>,
        L: Into<String>,
    {
        self.steps = Some((current, labels.into_iter().map(Into::into).collect()));
        self
    }

    /// Collect HTTP Basic account details as separate username and password
    /// fields. The runtime still owns encoding and storage.
    #[must_use]
    pub const fn with_basic(mut self) -> Self {
        self.basic = true;
        self
    }

    /// Bind subsequent account entry to an owner-selected server.
    ///
    /// # Errors
    /// Refuses malformed addresses, and changes while the runtime's field is
    /// up: the field was asked for with the server it will save to.
    pub fn bind_server(&mut self, server: &str) -> Result<(), String> {
        if self.typing_secret() || !kobo_protocol::valid_secret_server(server) {
            return Err("Add a valid HTTPS server address first.".into());
        }
        self.close();
        self.server = Some(server.into());
        Ok(())
    }

    const fn typing_secret(&self) -> bool {
        matches!(self.view, View::Entry | View::Password | View::Saving)
    }

    /// Where the runtime keeps what the owner types for this account.
    fn target(&self) -> SecretTarget {
        SecretTarget {
            name: self.name.clone(),
            server: self.server.clone(),
            username: self.basic.then(|| self.username.clone()),
        }
    }

    /// Puts the runtime's field up for the secret itself.
    fn ask_for_secret(&mut self, view: View) {
        self.entry.close();
        self.entry.set_target(self.target());
        self.entry.open();
        self.view = view;
    }

    #[must_use]
    pub const fn is_open(&self) -> bool {
        !matches!(self.view, View::Closed)
    }

    pub fn open(&mut self) {
        self.problem = None;
        self.check = None;
        self.view = View::Prompt;
    }

    pub fn close(&mut self) {
        self.entry.close();
        self.username.clear();
        self.problem = None;
        self.check = None;
        self.checking = Checking::Idle;
        self.view = View::Closed;
    }

    /// Back to the prompt, with nothing half entered kept.
    fn back_to_prompt(&mut self) {
        self.entry.close();
        self.username.clear();
        self.view = View::Prompt;
    }

    /// Ask the runtime whether the account arrived from somewhere else.
    ///
    /// Call this when the app comes back to the front or the reader wakes.
    /// An owner who ran the computer command while this screen sat waiting
    /// should find the app connected when they look at it again, not be told
    /// to close and reopen it. Only a flow showing its prompt or its computer
    /// route asks, so typing is never interrupted.
    pub fn recheck(&mut self, context: &mut Context) {
        if matches!(self.view, View::Prompt | View::Cli) && self.checking == Checking::Idle {
            self.checking = Checking::Quiet;
            context.secrets().check(&[&self.name]);
        }
    }

    fn computer_command(&self) -> Option<String> {
        match (&self.computer, &self.server) {
            (Some(command), _) => Some(command.clone()),
            (None, None) => Some(format!("kobo secret set {}", self.name)),
            (None, Some(_)) => None,
        }
    }

    /// The address the phone code opens, while there is one that fits.
    fn key_page(&self) -> Option<(String, &str)> {
        let (page, caption) = self.key_page.as_ref()?;
        let address = if page.starts_with('/') {
            format!("{}{page}", self.server.as_deref()?.trim_end_matches('/'))
        } else {
            page.clone()
        };
        (!address.is_empty() && address.len() <= crate::MAX_CODE_BYTES)
            .then_some((address, caption.as_str()))
    }

    /// Puts the step indicator, when the app gave one, under the top bar.
    fn steps(&self, screen: ScreenBuilder) -> ScreenBuilder {
        match &self.steps {
            Some((current, labels)) if !labels.is_empty() => screen.steps(
                *current,
                u8::try_from(labels.len()).unwrap_or(u8::MAX),
                labels.iter().cloned(),
            ),
            _ => screen,
        }
    }

    /// The words on the control that starts typing.
    const fn enter_label(&self) -> &'static str {
        if self.basic {
            "Sign in"
        } else {
            "Enter account key"
        }
    }

    /// The server a field saves to, as a line under its prompt, so an owner
    /// with two libraries can see which one they are signing in to.
    fn server_line(&self) -> String {
        self.server
            .as_ref()
            .map(|server| format!("For {server}"))
            .unwrap_or_default()
    }

    /// The prompt: the key as a labelled field, and the other ways in.
    fn prompt_screen(&self, app_title: &str, computer: bool) -> Screen {
        let mut screen = ScreenBuilder::new("credential-required").top_bar(app_title);
        // Check again sits in the bar rather than among the buttons
        // so the prompt keeps its height, until a check has said the
        // account is not there: then it is the banner's own action,
        // beside the words it answers, and one control is enough.
        if self.check.is_none() {
            screen = screen.top_bar_action(CHECK, "Check again");
        }
        // A flow with steps names this one in its step line, so the
        // heading would say the same thing twice on the line under it.
        screen = if self.steps.is_some() {
            self.steps(screen)
        } else {
            screen.heading(format!("Connect {}", self.service))
        };
        screen = screen
            .field(ENTER, "", self.enter_label())
            .field_label(if self.basic {
                "Username and password"
            } else {
                "Account key"
            })
            // Nothing is in the field to hide: the runtime holds the
            // key, and the kind says what the field is for.
            .field_kind(if self.basic {
                FieldKind::Text
            } else {
                FieldKind::Secret
            });
        // While a check's banner is up, it is what the owner needs to
        // read, and the hint gives up its lines to it so the prompt
        // still fits a panel held on its side at the largest text.
        // The same guidance is under the runtime's field when typing.
        if self.check.is_none() {
            screen = screen.field_hint(self.guidance.as_deref().unwrap_or(if self.basic {
                "Use your server username and password."
            } else {
                "Add the account key for this service."
            }));
        }
        if let Some(problem) = &self.problem {
            screen = screen.field_error(problem);
        }
        if let Some(check) = &self.check {
            screen = screen
                .banner(BannerLevel::Attention, check)
                .banner_action(CHECK, "Check again");
        }
        // The other ways in share one line, so a prompt with a
        // refused save and a missing account still fits a panel held
        // on its side at the larger text sizes.
        screen = match (self.key_page().is_some(), computer) {
            (true, true) => screen.buttons([(PHONE, "Use phone"), (CLI, "Use computer")]),
            (true, false) => screen.button(PHONE, "Use phone"),
            (false, true) => screen.button(CLI, "Use computer"),
            (false, false) => screen,
        };
        screen.build()
    }

    #[must_use]
    pub fn screen(&self, app_title: &str) -> Screen {
        let command = self.computer_command();
        match self.view {
            View::Closed => ScreenBuilder::new("credential-closed")
                .top_bar(app_title)
                .build(),
            View::Prompt => self.prompt_screen(app_title, command.is_some()),
            // The runtime draws these three. What is built here is this
            // application's screen under its field, whose title the runtime
            // keeps in its top bar while the owner types.
            View::Username => ScreenBuilder::new("account-username")
                .top_bar(app_title)
                .owns_back(true)
                .text_entry_with_hint(&self.entry, "Username", &self.server_line(), "Next")
                .build(),
            View::Password => ScreenBuilder::new("account-private-entry")
                .top_bar(app_title)
                .owns_back(true)
                .text_entry_with_hint(&self.entry, "Password", &self.server_line(), "Save")
                .build(),
            View::Entry => ScreenBuilder::new("account-private-entry")
                .top_bar(app_title)
                .owns_back(true)
                .text_entry_with_hint(
                    &self.entry,
                    "Account key",
                    self.guidance.as_deref().unwrap_or_default(),
                    "Save",
                )
                .build(),
            // The top bar's back returns to the prompt. A second Back button
            // beside it would leave two controls that look alike and go to
            // different places when an app owns back for the whole flow.
            //
            // Laid out as every connect screen is: what the last check found
            // first, under the top bar, then what to do and the command, and
            // Check again directly under the command until a check has found
            // nothing. After that Check again is the banner's action, beside
            // the words it answers, rather than a second button further down.
            View::Cli => {
                let screen = ScreenBuilder::new("credential-cli")
                    .top_bar(app_title)
                    .owns_back(true);
                let screen = match &self.check {
                    Some(check) => screen
                        .banner(BannerLevel::Attention, check)
                        .banner_action(CHECK, "Check again"),
                    None => screen,
                }
                // A section rather than a heading: the top bar already names
                // the app, and a line of display type was what pushed Check
                // again off a small panel held on its side.
                .section("Set it from a computer")
                .secondary("Run this on your computer, then tap Check again.")
                .command(command.unwrap_or_default());
                if self.check.is_some() {
                    screen.build()
                } else {
                    screen.primary_button(CHECK, "Check again").build()
                }
            }
            // The code and the way on to typing, with nothing else, so the
            // code keeps its full size on a panel held either way at the
            // largest text.
            View::Phone => {
                let (address, caption) = self.key_page().unwrap_or_default();
                ScreenBuilder::new("credential-phone")
                    .top_bar(app_title)
                    .owns_back(true)
                    .code(address, caption)
                    .bottom_action(ENTER, self.enter_label())
                    .build()
            }
            View::Saving => ScreenBuilder::new("credential-saving")
                .top_bar(app_title)
                .heading("Saving account details")
                .text("Keep this page open until saving finishes.")
                .build(),
        }
    }

    pub fn on_action(
        &mut self,
        context: &mut Context,
        action: ActionId,
    ) -> Option<CredentialEvent> {
        if matches!(self.view, View::Username | View::Password | View::Entry) {
            if action == ActionId::BACK {
                self.back_to_prompt();
                return Some(CredentialEvent::Changed);
            }
            let typing = self.entry.handle_verbatim(action)?;
            return Some(self.typed(context, typing));
        }
        if self.view == View::Saving {
            return Some(CredentialEvent::Changed);
        }
        if matches!(self.view, View::Cli | View::Phone) && action == ActionId::BACK {
            self.view = View::Prompt;
            return Some(CredentialEvent::Changed);
        }
        if action == ActionId::BACK {
            self.close();
            return Some(CredentialEvent::Cancelled);
        }
        match (self.view, action) {
            (View::Prompt | View::Phone, value) if value == action_id(ENTER) => {
                self.problem = None;
                self.check = None;
                if self.basic {
                    self.entry.close();
                    self.entry = TextEntry::new();
                    self.entry.open();
                    self.view = View::Username;
                } else {
                    self.ask_for_secret(View::Entry);
                }
                Some(CredentialEvent::Changed)
            }
            (View::Prompt, value)
                if value == action_id(CLI) && self.computer_command().is_some() =>
            {
                self.problem = None;
                self.check = None;
                self.view = View::Cli;
                Some(CredentialEvent::Changed)
            }
            (View::Prompt, value) if value == action_id(PHONE) && self.key_page().is_some() => {
                self.problem = None;
                self.check = None;
                self.view = View::Phone;
                Some(CredentialEvent::Changed)
            }
            (View::Prompt | View::Cli, value) if value == action_id(CHECK) => {
                self.problem = None;
                self.check = None;
                if self.checking == Checking::Idle {
                    context.secrets().check(&[&self.name]);
                }
                self.checking = Checking::Asked;
                Some(CredentialEvent::Changed)
            }
            _ => None,
        }
    }

    /// What the runtime's field said, for whichever of the three is up.
    fn typed(&mut self, context: &mut Context, typing: Typing) -> CredentialEvent {
        match typing {
            Typing::Submitted(value) if self.view == View::Username => {
                if value.contains(':') || value.chars().any(char::is_control) {
                    self.problem = Some("Enter a username without a colon or line break.".into());
                    self.back_to_prompt();
                } else {
                    self.username = value;
                    self.ask_for_secret(View::Password);
                }
                CredentialEvent::Changed
            }
            // Reported through `on_device_result`, as a saved account always
            // has been, so an application written when the flow sent the
            // secret itself goes on connecting with no change. The check
            // that carries the news is a real one, and costs one answer.
            Typing::Saved => {
                self.entry.close();
                self.username.clear();
                self.view = View::Saving;
                self.checking = Checking::Saved;
                context.secrets().check(&[&self.name]);
                CredentialEvent::Changed
            }
            Typing::Refused(result) => {
                self.problem = Some(
                    match result {
                        DeviceResult::Denied(_) => {
                            "This app cannot save this account. Update Cobalt and try again."
                        }
                        DeviceResult::Failed(DeviceError::InvalidInput) => {
                            "These account details are too long or contain a line break. Check them and try again."
                        }
                        DeviceResult::Failed(_) => {
                            "Account details were not saved. Check the reader\u{2019}s free space and try again."
                        }
                        _ => "Saving could not be confirmed. Try again.",
                    }
                    .to_owned(),
                );
                self.back_to_prompt();
                CredentialEvent::Changed
            }
            Typing::Cancelled => {
                self.back_to_prompt();
                CredentialEvent::Changed
            }
            // A secret is never handed back, so a submitted value here is not
            // one this flow asked for.
            Typing::Submitted(_) | Typing::Changed => CredentialEvent::Changed,
        }
    }

    /// The answer to [`Self::recheck`] or to a Check again tap.
    fn on_checked(&mut self, result: &DeviceResult) -> Option<CredentialEvent> {
        // The runtime already said this one was saved. Whatever the check
        // found, the account went where the runtime keeps it.
        if self.checking == Checking::Saved && self.view == View::Saving {
            self.close();
            return Some(CredentialEvent::Saved);
        }
        let asked = self.checking == Checking::Asked;
        self.checking = Checking::Idle;
        if !matches!(self.view, View::Prompt | View::Cli) {
            return None;
        }
        match result {
            DeviceResult::Secrets { present } if present.contains(&self.name) => {
                self.close();
                Some(CredentialEvent::Saved)
            }
            _ if !asked => Some(CredentialEvent::Changed),
            DeviceResult::Denied(_) => {
                self.close();
                Some(CredentialEvent::TryAccount)
            }
            DeviceResult::Secrets { .. } => {
                self.check = Some(format!(
                    "No {} account is on this reader yet.",
                    self.service
                ));
                Some(CredentialEvent::Changed)
            }
            _ => {
                self.check = Some("The reader could not check. Try again.".into());
                Some(CredentialEvent::Changed)
            }
        }
    }

    /// Takes the answer to a presence check this flow sent, including the
    /// one that reports an account typed on the reader as saved.
    pub fn on_device_result(
        &mut self,
        request: &DeviceRequest,
        result: &DeviceResult,
    ) -> Option<CredentialEvent> {
        let DeviceRequest::CheckSecrets { names } = request else {
            return None;
        };
        if self.checking != Checking::Idle && names.len() == 1 && names[0] == self.name {
            self.on_checked(result)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CredentialEvent, CredentialSetup};
    use crate::keyboard::testing;
    use crate::{action_id, Command, Context, DeviceRequest, DeviceResult};
    use kobo_protocol::{DenyReason, DeviceError, SecretTarget, TextOutcome};
    use kobo_ui::FieldKind;

    /// Sends the flow's screen, as an application does after every event,
    /// and returns the text field that screen asked the runtime for.
    fn asked(setup: &CredentialSetup, context: &mut Context) -> Option<DeviceRequest> {
        context.set_screen(setup.screen("Komga"));
        testing::asked(&context.take_commands()).cloned()
    }

    /// Answers the runtime's field as the runtime would.
    fn answer(
        setup: &mut CredentialSetup,
        context: &mut Context,
        outcome: TextOutcome,
    ) -> Option<CredentialEvent> {
        crate::keyboard::runtime::answer_newest(outcome);
        setup.on_action(context, crate::keyboard::answered())
    }

    /// The presence check a saved field sends, answered as the runtime
    /// would, and what the flow made of it.
    fn confirm(setup: &mut CredentialSetup, context: &mut Context) -> Option<CredentialEvent> {
        let commands = context.take_commands();
        let Some(Command::Device(request @ DeviceRequest::CheckSecrets { names })) = commands
            .iter()
            .find(|command| matches!(command, Command::Device(DeviceRequest::CheckSecrets { .. })))
        else {
            panic!("a saved account is confirmed: {commands:?}")
        };
        setup.on_device_result(
            request,
            &DeviceResult::Secrets {
                present: names.clone(),
            },
        )
    }

    fn secret_of(request: Option<&DeviceRequest>) -> Option<SecretTarget> {
        match request {
            Some(DeviceRequest::TypeText {
                kind: FieldKind::Secret,
                save_as,
                initial,
                ..
            }) => {
                assert!(initial.is_empty(), "a secret field opens empty");
                save_as.clone()
            }
            _ => None,
        }
    }

    #[test]
    fn an_account_key_is_typed_on_the_runtime_and_saved_without_reaching_the_app() {
        let mut setup = CredentialSetup::new("zotero", "Zotero")
            .with_guidance("Create a key at zotero.org/settings/keys first.");
        let mut context = Context::default();
        setup.open();
        setup.on_action(&mut context, action_id("credential.enter"));
        let request = asked(&setup, &mut context);
        assert_eq!(
            secret_of(request.as_ref()),
            Some(SecretTarget::named("zotero"))
        );
        let Some(DeviceRequest::TypeText { hint, .. }) = &request else {
            unreachable!()
        };
        assert!(hint.contains("zotero.org/settings/keys"), "{hint}");
        assert!(
            asked(&setup, &mut context).is_none(),
            "a screen drawn again does not ask again"
        );
        assert_eq!(
            answer(&mut setup, &mut context, TextOutcome::Saved),
            Some(CredentialEvent::Changed)
        );
        assert!(format!("{:?}", setup.screen("Zotero")).contains("Saving account details"));
        assert!(
            context.commands().iter().all(|command| !matches!(
                command,
                Command::Device(DeviceRequest::SetSecret { .. })
            )),
            "the runtime saved it, so the app sends nothing to save"
        );
        assert_eq!(
            setup.on_action(&mut context, action_id("credential.enter")),
            Some(CredentialEvent::Changed),
            "nothing starts again while the save is being confirmed"
        );
        assert_eq!(
            confirm(&mut setup, &mut context),
            Some(CredentialEvent::Saved),
            "saved is heard where applications have always listened for it"
        );
        assert!(!setup.is_open());
    }

    #[test]
    fn a_basic_account_types_its_username_here_and_its_password_on_the_runtime() {
        let mut setup = CredentialSetup::new("komga", "Komga").with_basic();
        let mut context = Context::default();
        setup.bind_server("https://books.example").unwrap();
        setup.open();
        setup.on_action(&mut context, action_id("credential.enter"));
        let username = asked(&setup, &mut context);
        assert!(
            matches!(&username, Some(DeviceRequest::TypeText { kind: FieldKind::Text, save_as: None, hint, .. }) if hint == "For https://books.example"),
            "{username:?}"
        );
        assert_eq!(
            answer(
                &mut setup,
                &mut context,
                TextOutcome::Typed("reader".into())
            ),
            Some(CredentialEvent::Changed)
        );
        assert_eq!(
            secret_of(asked(&setup, &mut context).as_ref()),
            Some(SecretTarget {
                name: "komga".into(),
                server: Some("https://books.example".into()),
                username: Some("reader".into()),
            })
        );
        assert!(
            setup.bind_server("https://another.example").is_err(),
            "the field was asked for with the server it saves to"
        );
        assert_eq!(
            answer(&mut setup, &mut context, TextOutcome::Saved),
            Some(CredentialEvent::Changed)
        );
        assert!(setup.username.is_empty());
        assert_eq!(
            confirm(&mut setup, &mut context),
            Some(CredentialEvent::Saved)
        );
    }

    #[test]
    fn invalid_usernames_and_cancelled_passwords_never_save_an_account() {
        let mut setup = CredentialSetup::new("komga", "Komga").with_basic();
        let mut context = Context::default();
        setup.open();
        setup.on_action(&mut context, action_id("credential.enter"));
        asked(&setup, &mut context);
        answer(
            &mut setup,
            &mut context,
            TextOutcome::Typed("user:password".into()),
        );
        assert!(setup.problem.is_some());
        assert_eq!(setup.view, super::View::Prompt);
        assert!(secret_of(asked(&setup, &mut context).as_ref()).is_none());

        setup.on_action(&mut context, action_id("credential.enter"));
        asked(&setup, &mut context);
        answer(
            &mut setup,
            &mut context,
            TextOutcome::Typed("reader".into()),
        );
        asked(&setup, &mut context);
        answer(&mut setup, &mut context, TextOutcome::Cancelled);
        assert_eq!(setup.view, super::View::Prompt);
        assert!(setup.username.is_empty());
    }

    #[test]
    fn a_save_the_runtime_refuses_says_why_and_offers_the_field_again() {
        for (refusal, says) in [
            (
                TextOutcome::Denied(DenyReason::NotDeclared),
                "This app cannot save this account",
            ),
            (TextOutcome::Failed(DeviceError::Backend), "free space"),
            (TextOutcome::Failed(DeviceError::InvalidInput), "too long"),
        ] {
            let mut setup = CredentialSetup::new("zotero", "Zotero");
            let mut context = Context::default();
            setup.open();
            setup.on_action(&mut context, action_id("credential.enter"));
            asked(&setup, &mut context);
            assert_eq!(
                answer(&mut setup, &mut context, refusal),
                Some(CredentialEvent::Changed)
            );
            let drawn = format!("{:?}", setup.screen("Zotero"));
            assert!(drawn.contains(says), "{drawn}");
            assert!(drawn.contains("Enter account key"), "{drawn}");
        }
    }

    #[test]
    fn guidance_tells_the_owner_where_the_key_comes_from_instead_of_the_generic_line() {
        let mut setup = CredentialSetup::new("lichess", "Lichess")
            .with_guidance("Create a token at lichess.org first.");
        setup.open();
        let drawn = format!("{:?}", setup.screen("Lichess"));
        assert!(
            drawn.contains("Create a token at lichess.org first."),
            "{drawn}"
        );
        assert!(!drawn.contains("Add the account key"), "{drawn}");
        assert!(drawn.contains("Enter account key"), "{drawn}");
    }

    #[test]
    fn the_computer_route_has_one_way_back_and_it_returns_to_the_prompt() {
        let mut setup = CredentialSetup::new("zotero", "Zotero");
        let mut context = Context::default();
        setup.open();
        setup.on_action(&mut context, action_id("credential.cli"));
        let screen = setup.screen("Zotero");
        assert!(screen.owns_back, "{screen:?}");
        assert!(
            !format!("{screen:?}").contains("label: \"Back\""),
            "{screen:?}"
        );

        assert_eq!(
            setup.on_action(&mut context, crate::ActionId::BACK),
            Some(CredentialEvent::Changed)
        );
        assert!(setup.is_open());
        assert!(format!("{:?}", setup.screen("Zotero")).contains("Use computer"));
    }

    #[test]
    fn account_fields_and_their_prompts_fit_all_interface_sizes() {
        let panels =
            [kobo_ui::CLARA_BW_METRICS]
                .into_iter()
                .chain(kobo_profile::SUPPORTED_PROFILES.iter().map(|profile| {
                    crate::DisplayMetrics {
                        width: i32::try_from(profile.width).unwrap(),
                        height: i32::try_from(profile.height).unwrap(),
                        pixels_per_inch: i32::from(profile.pixels_per_inch),
                        text_scale: kobo_ui::TextScale::Default,
                    }
                }))
                .chain([crate::DisplayMetrics {
                    width: 758,
                    height: 1024,
                    pixels_per_inch: 212,
                    text_scale: kobo_ui::TextScale::Default,
                }]);
        let panels = panels.flat_map(|portrait| {
            [
                portrait,
                crate::DisplayMetrics {
                    width: portrait.height,
                    height: portrait.width,
                    ..portrait
                },
            ]
        });
        for mut metrics in panels {
            for scale in kobo_ui::TextScale::STEPS {
                metrics.text_scale = scale;
                #[cfg(feature = "text")]
                kobo_text::install(metrics).unwrap();
                let mut setup = CredentialSetup::new("komga", "Komga")
                    .with_basic()
                    .with_computer_command("kobo connect komga")
                    .with_key_page(
                        "/settings/tokens",
                        "Opens the page on your server where account keys are made.",
                    )
                    .with_steps(1, ["Address", "Account", "Check"]);
                setup.bind_server("https://books.example").unwrap();
                // A refused save and a missing account are never shown at
                // once: Check again clears the one, and typing clears the
                // other. Each is checked at its longest.
                let refused = "These account details are too long or contain a line break. Check them and try again.";
                let missing = "No Komga account is on this reader yet.";
                for (view, problem, check) in [
                    (super::View::Prompt, Some(refused), None),
                    (super::View::Prompt, None, Some(missing)),
                    (super::View::Username, None, None),
                    (super::View::Password, None, None),
                    (super::View::Entry, None, None),
                    (super::View::Cli, None, None),
                    (super::View::Cli, None, Some(missing)),
                    (super::View::Phone, None, None),
                    (super::View::Saving, None, None),
                ] {
                    // Held on its side, the prompt, the computer route and
                    // the phone code are checked: they are what this flow
                    // draws with the new controls. The typing screens are
                    // the runtime's.
                    let sideways = metrics.width > metrics.height;
                    if sideways
                        && !matches!(
                            view,
                            super::View::Prompt | super::View::Cli | super::View::Phone
                        )
                    {
                        continue;
                    }
                    setup.view = view;
                    setup.problem = problem.map(str::to_owned);
                    setup.check = check.map(str::to_owned);
                    setup.entry.open();
                    let screen = setup.screen("Komga").with_own_back(true);
                    let chrome = kobo_ui::Chrome::for_screen(&screen, false, None);
                    let errors = screen
                        .diagnostics(&metrics, &chrome)
                        .issues
                        .into_iter()
                        .filter(|issue| issue.severity == kobo_ui::DiagnosticSeverity::Error)
                        .collect::<Vec<_>>();
                    assert!(errors.is_empty(), "{metrics:?} {view:?}: {errors:?}");
                }
            }
        }
    }

    #[test]
    fn check_again_connects_an_account_that_arrived_from_the_computer() {
        let mut setup = CredentialSetup::new("zotero", "Zotero");
        let mut context = Context::default();
        setup.open();
        setup.on_action(&mut context, action_id("credential.cli"));
        let route = format!("{:?}", setup.screen("Zotero"));
        assert!(route.contains("kobo secret set zotero"), "{route}");
        assert!(!route.contains("<address>"), "{route}");
        assert!(route.contains("Check again"), "{route}");
        assert!(!route.contains("close and reopen"), "{route}");

        setup.on_action(&mut context, action_id("credential.check"));
        let commands = context.take_commands();
        let [Command::Device(request @ DeviceRequest::CheckSecrets { names })] =
            commands.as_slice()
        else {
            panic!("Check again asks the runtime: {commands:?}")
        };
        assert_eq!(names, &["zotero".to_owned()]);
        assert_eq!(
            setup.on_device_result(
                request,
                &DeviceResult::Secrets {
                    present: Vec::new()
                }
            ),
            Some(CredentialEvent::Changed)
        );
        let drawn = format!("{:?}", setup.screen("Zotero"));
        assert!(
            drawn.contains("No Zotero account is on this reader yet"),
            "{drawn}"
        );

        setup.on_action(&mut context, action_id("credential.check"));
        let commands = context.take_commands();
        let [Command::Device(request)] = commands.as_slice() else {
            panic!("a second check is sent")
        };
        assert_eq!(
            setup.on_device_result(
                request,
                &DeviceResult::Secrets {
                    present: vec!["zotero".to_owned()]
                }
            ),
            Some(CredentialEvent::Saved)
        );
        assert!(!setup.is_open());
    }

    #[test]
    fn a_check_the_runtime_will_not_answer_hands_the_question_to_the_app() {
        let mut setup = CredentialSetup::new("readeck", "Readeck");
        let mut context = Context::default();
        setup.bind_server("https://read.example").unwrap();
        setup.open();
        setup.recheck(&mut context);
        let commands = context.take_commands();
        let Command::Device(request) = &commands[0] else {
            panic!("a device request")
        };
        let refused = DeviceResult::Denied(kobo_protocol::DenyReason::PolicyRejected);
        assert_eq!(
            setup.on_device_result(request, &refused),
            Some(CredentialEvent::Changed),
            "a quiet check stays quiet"
        );
        assert!(setup.is_open());
        setup.on_action(&mut context, action_id("credential.check"));
        let commands = context.take_commands();
        let Command::Device(request) = &commands[0] else {
            panic!("a device request")
        };
        assert_eq!(
            setup.on_device_result(request, &refused),
            Some(CredentialEvent::TryAccount)
        );
        assert!(!setup.is_open());
    }

    #[test]
    fn a_quiet_recheck_on_return_connects_without_warning_when_nothing_changed() {
        let mut setup = CredentialSetup::new("zotero", "Zotero");
        let mut context = Context::default();
        setup.recheck(&mut context);
        assert!(
            context.take_commands().is_empty(),
            "a closed flow asks nothing"
        );
        setup.open();
        setup.recheck(&mut context);
        setup.recheck(&mut context);
        let commands = context.take_commands();
        assert_eq!(commands.len(), 1, "one check at a time: {commands:?}");
        let Command::Device(request) = &commands[0] else {
            panic!("a device request")
        };
        setup.on_device_result(
            request,
            &DeviceResult::Secrets {
                present: Vec::new(),
            },
        );
        assert!(!format!("{:?}", setup.screen("Zotero"))
            .contains("No Zotero account is on this reader yet"));
        setup.recheck(&mut context);
        let commands = context.take_commands();
        let Command::Device(request) = &commands[0] else {
            panic!("a device request")
        };
        assert_eq!(
            setup.on_device_result(
                request,
                &DeviceResult::Secrets {
                    present: vec!["zotero".to_owned()]
                }
            ),
            Some(CredentialEvent::Saved)
        );
        let typing = &mut CredentialSetup::new("zotero", "Zotero");
        typing.open();
        typing.on_action(&mut context, action_id("credential.enter"));
        typing.recheck(&mut context);
        assert!(
            context.take_commands().is_empty(),
            "typing is never interrupted"
        );
    }

    #[test]
    fn a_server_bound_flow_offers_the_computer_only_with_a_command_that_carries_the_server() {
        let mut bare = CredentialSetup::new("readeck", "Readeck");
        let mut context = Context::default();
        bare.bind_server("https://read.example").unwrap();
        bare.open();
        assert!(!format!("{:?}", bare.screen("Readeck")).contains("Use computer"));
        assert!(format!("{:?}", bare.screen("Readeck")).contains("Check again"));
        assert_eq!(
            bare.on_action(&mut context, action_id("credential.cli")),
            None
        );

        let mut guided = CredentialSetup::new("readeck", "Readeck")
            .with_computer_command("kobo connect readeck");
        guided.bind_server("https://read.example").unwrap();
        guided.open();
        guided.on_action(&mut context, action_id("credential.cli"));
        assert!(format!("{:?}", guided.screen("Readeck")).contains("kobo connect readeck"));
    }

    fn checks_in_bar(screen: &crate::Screen) -> bool {
        screen.top_bar.as_ref().is_some_and(|bar| {
            bar.actions
                .iter()
                .any(|action| action.label == "Check again")
        })
    }

    #[test]
    fn a_refused_save_is_the_fields_error_and_a_missing_account_is_a_banner_that_checks_again() {
        let mut setup = CredentialSetup::new("zotero", "Zotero");
        let mut context = Context::default();
        setup.open();
        let prompt = setup.screen("Zotero");
        let field = prompt.nodes.iter().find_map(|node| match node {
            kobo_ui::Node::Field {
                label, hint, kind, ..
            } => Some((label.clone(), hint.clone(), *kind)),
            _ => None,
        });
        assert_eq!(
            field,
            Some((
                Some("Account key".to_owned()),
                Some("Add the account key for this service.".to_owned()),
                FieldKind::Secret
            )),
            "{prompt:?}"
        );
        assert!(checks_in_bar(&prompt), "{prompt:?}");

        setup.on_action(&mut context, action_id("credential.enter"));
        asked(&setup, &mut context);
        answer(
            &mut setup,
            &mut context,
            TextOutcome::Failed(DeviceError::InvalidInput),
        );
        let refused = setup.screen("Zotero");
        assert!(
            refused.nodes.iter().any(|node| matches!(
                node,
                kobo_ui::Node::Field { error: Some(error), .. } if error.contains("too long")
            )),
            "{refused:?}"
        );
        assert!(
            !refused
                .nodes
                .iter()
                .any(|node| matches!(node, kobo_ui::Node::Banner { .. })),
            "a refused save is said once, under the field: {refused:?}"
        );

        setup.on_action(&mut context, action_id("credential.check"));
        let commands = context.take_commands();
        let Some(Command::Device(request)) = commands.last() else {
            panic!("Check again asks the runtime: {commands:?}")
        };
        setup.on_device_result(
            request,
            &DeviceResult::Secrets {
                present: Vec::new(),
            },
        );
        let missing = setup.screen("Zotero");
        assert!(
            missing.nodes.iter().any(|node| matches!(
                node,
                kobo_ui::Node::Banner { text, action: Some(action), .. }
                    if text.contains("No Zotero account") && action.label == "Check again"
                        && action.action == action_id("credential.check")
            )),
            "{missing:?}"
        );
        assert!(
            !checks_in_bar(&missing),
            "Check again is offered once: {missing:?}"
        );
    }

    #[test]
    fn a_key_page_on_the_owners_server_is_offered_as_a_code_for_a_phone() {
        let mut setup = CredentialSetup::new("readeck", "Readeck").with_key_page(
            "/profile/tokens",
            "Opens Readeck's token page on your server.",
        );
        let mut context = Context::default();
        setup.open();
        assert!(
            !format!("{:?}", setup.screen("Readeck")).contains("Use phone"),
            "no code before the server is known"
        );
        assert_eq!(
            setup.on_action(&mut context, action_id("credential.phone")),
            None
        );
        setup.bind_server("https://read.example/").unwrap();
        setup.open();
        assert!(format!("{:?}", setup.screen("Readeck")).contains("Use phone"));
        setup.on_action(&mut context, action_id("credential.phone"));
        let phone = setup.screen("Readeck");
        assert!(
            phone.nodes.iter().any(|node| matches!(
                node,
                kobo_ui::Node::Code { payload, caption, .. }
                    if payload == "https://read.example/profile/tokens" && caption.contains("token page")
            )),
            "{phone:?}"
        );
        assert!(phone.owns_back, "{phone:?}");
        setup.on_action(&mut context, crate::ActionId::BACK);
        assert!(format!("{:?}", setup.screen("Readeck")).contains("Use phone"));

        setup.on_action(&mut context, action_id("credential.phone"));
        setup.on_action(&mut context, action_id("credential.enter"));
        assert!(
            secret_of(asked(&setup, &mut context).as_ref()).is_some(),
            "the phone screen leads straight on to typing the key"
        );

        let long = CredentialSetup::new("lichess", "Lichess").with_key_page(
            format!("https://lichess.org/{}", "a".repeat(crate::MAX_CODE_BYTES)),
            "Too long to scan.",
        );
        assert!(
            long.key_page().is_none(),
            "a code that cannot be read is not offered"
        );
    }

    #[test]
    fn a_sign_in_inside_a_longer_flow_says_which_step_it_is() {
        let mut setup = CredentialSetup::new("readeck", "Readeck")
            .with_computer_command("kobo connect readeck")
            .with_steps(1, ["Address", "Token", "Check"]);
        let mut context = Context::default();
        setup.open();
        let steps = |screen: &crate::Screen| {
            screen.nodes.iter().find_map(|node| match node {
                kobo_ui::Node::Steps {
                    current,
                    total,
                    labels,
                    ..
                } => Some((*current, *total, labels.clone())),
                _ => None,
            })
        };
        let expected = Some((1, 3, vec!["Address".into(), "Token".into(), "Check".into()]));
        assert_eq!(steps(&setup.screen("Readeck")), expected);
        setup.on_action(&mut context, action_id("credential.cli"));
        assert_eq!(
            steps(&setup.screen("Readeck")),
            None,
            "the computer route is a way through the same step, not a step of its own"
        );
        assert_eq!(
            steps(&CredentialSetup::new("zotero", "Zotero").screen("Zotero")),
            None
        );
    }
}

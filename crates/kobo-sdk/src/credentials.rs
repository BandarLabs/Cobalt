//! Consistent owner onboarding for runtime-held application credentials.
//!
//! The account key or password is typed on the runtime's keyboard, and the
//! runtime writes it into this application's own secret namespace. The flow
//! hears that it was saved and never what it was, so a Lichess token or a
//! server password passes through nothing but the runtime that keeps it. A
//! username is typed here like any other text, because it is shown on screen
//! anyway and the runtime stores it in front of the password.

use crate::keyboard::{TextEntry, Typing};
use crate::{
    action_id, ActionId, Context, DeviceError, DeviceRequest, DeviceResult, Screen, ScreenBuilder,
};
use kobo_protocol::SecretTarget;
use std::fmt;

const ENTER: &str = "credential.enter";
const CLI: &str = "credential.cli";
const CHECK: &str = "credential.check";

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
    problem: Option<String>,
    basic: bool,
    username: String,
    server: Option<String>,
    guidance: Option<String>,
    computer: Option<String>,
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
            basic: false,
            username: String::new(),
            server: None,
            guidance: None,
            computer: None,
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
        self.view = View::Prompt;
    }

    pub fn close(&mut self) {
        self.entry.close();
        self.username.clear();
        self.problem = None;
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

    /// The server a field saves to, as a line under its prompt, so an owner
    /// with two libraries can see which one they are signing in to.
    fn server_line(&self) -> String {
        self.server
            .as_ref()
            .map(|server| format!("For {server}"))
            .unwrap_or_default()
    }

    #[must_use]
    pub fn screen(&self, app_title: &str) -> Screen {
        let command = self.computer_command();
        match self.view {
            View::Closed => ScreenBuilder::new("credential-closed")
                .top_bar(app_title)
                .build(),
            View::Prompt => {
                let mut screen = ScreenBuilder::new("credential-required")
                    .top_bar(app_title)
                    .heading(format!("Connect {}", self.service))
                    .text(self.guidance.as_deref().unwrap_or(if self.basic {
                        "Use your server username and password."
                    } else {
                        "Add the account key for this service."
                    }));
                if let Some(problem) = &self.problem {
                    screen = screen.banner(crate::BannerLevel::Attention, problem);
                }
                // Check again sits in the bar rather than among the buttons
                // so the prompt keeps its height with a problem shown: the
                // smallest panel at the largest text has no room for a third
                // control under the banner.
                screen = screen.top_bar_action(CHECK, "Check again").primary_button(
                    ENTER,
                    if self.basic {
                        "Sign in"
                    } else {
                        "Enter account key"
                    },
                );
                if command.is_some() {
                    screen = screen.button(CLI, "Use computer");
                }
                screen.build()
            }
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
            View::Cli => {
                let mut screen = ScreenBuilder::new("credential-cli")
                    .top_bar(app_title)
                    .owns_back(true)
                    .heading("Set it from a computer");
                if let Some(problem) = &self.problem {
                    screen = screen.banner(crate::BannerLevel::Attention, problem);
                }
                screen
                    .text(command.unwrap_or_default())
                    .secondary("Run this on your computer, then tap Check again.")
                    .primary_button(CHECK, "Check again")
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
        if self.view == View::Cli && action == ActionId::BACK {
            self.view = View::Prompt;
            return Some(CredentialEvent::Changed);
        }
        if action == ActionId::BACK {
            self.close();
            return Some(CredentialEvent::Cancelled);
        }
        match (self.view, action) {
            (View::Prompt, value) if value == action_id(ENTER) => {
                self.problem = None;
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
                self.view = View::Cli;
                Some(CredentialEvent::Changed)
            }
            (View::Prompt | View::Cli, value) if value == action_id(CHECK) => {
                self.problem = None;
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
                self.problem = Some(format!(
                    "No {} account is on this reader yet.",
                    self.service
                ));
                Some(CredentialEvent::Changed)
            }
            _ => {
                self.problem = Some("The reader could not check. Try again.".into());
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
        for mut metrics in panels {
            for scale in kobo_ui::TextScale::STEPS {
                metrics.text_scale = scale;
                #[cfg(feature = "text")]
                kobo_text::install(metrics).unwrap();
                let mut setup = CredentialSetup::new("komga", "Komga").with_basic();
                setup.bind_server("https://books.example").unwrap();
                setup.problem = Some("No Komga account is on this reader yet.".into());
                for view in [
                    super::View::Prompt,
                    super::View::Username,
                    super::View::Password,
                    super::View::Entry,
                    super::View::Cli,
                    super::View::Saving,
                ] {
                    setup.view = view;
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
}

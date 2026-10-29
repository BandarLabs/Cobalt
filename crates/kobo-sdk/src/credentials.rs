//! Consistent owner onboarding for runtime-held application credentials.

use crate::keyboard::{TextEntry, Typing};
use crate::{action_id, ActionId, Context, DeviceRequest, DeviceResult, Screen, ScreenBuilder};
use std::fmt;

const ENTER: &str = "credential.enter";
const CLI: &str = "credential.cli";
const CHECK: &str = "credential.check";
const REVEAL: &str = "credential.reveal";

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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum View {
    Closed,
    Prompt,
    Entry,
    Username,
    Password,
    Cli,
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
    revealed: bool,
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
            revealed: false,
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
    /// no browser to look it up in.
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
    /// Refuses malformed addresses or changes while a save is pending.
    pub fn bind_server(&mut self, server: &str) -> Result<(), String> {
        if self.view == View::Saving || !kobo_protocol::valid_secret_server(server) {
            return Err("Add a valid HTTPS server address first.".into());
        }
        self.close();
        self.server = Some(server.into());
        Ok(())
    }

    fn submit(&mut self, context: &mut Context, value: String) {
        self.username.clear();
        self.entry.close();
        self.revealed = false;
        if value.is_empty()
            || value.len() > kobo_protocol::MAX_APP_SECRET_BYTES
            || value.chars().any(char::is_control)
        {
            self.problem = Some("These account details are too long or contain a line break. Check them and try again.".into());
            self.view = View::Prompt;
            return;
        }
        if let Some(server) = &self.server {
            context.secrets().set_server(&self.name, server, value);
        } else {
            context.secrets().set(&self.name, value);
        }
        self.view = View::Saving;
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
        self.revealed = false;
        self.checking = Checking::Idle;
        self.view = View::Closed;
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
            View::Username => ScreenBuilder::new("account-username")
                .top_bar("Username")
                .owns_back(true)
                .typed(self.entry.keyboard(), "Type your username")
                .keyboard(self.entry.keyboard(), "Next")
                .build(),
            View::Password => self.private_entry("Password", "Type your password"),
            View::Entry => self.private_entry("Account key", "Type your account key"),
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

    /// The typed value stays off the panel unless the owner asks for it, and
    /// its length is always shown.
    ///
    /// Typing blind with no count meant a 24-character token copied by eye
    /// from a phone failed on one mis-tap with nothing on screen to say which.
    /// The count catches a dropped or doubled key against the length the
    /// service printed, and Show lets the owner read it back once, somewhere
    /// nobody is looking over their shoulder.
    fn private_entry(&self, title: &str, empty: &str) -> Screen {
        let typed = self.entry.text();
        let count = typed.chars().count();
        let length = if count == 1 {
            "1 character entered".to_owned()
        } else {
            format!("{count} characters entered")
        };
        let screen = ScreenBuilder::new("account-private-entry")
            .top_bar(title)
            .top_bar_action(REVEAL, if self.revealed { "Hide" } else { "Show" })
            .owns_back(true);
        let screen = if typed.is_empty() {
            screen.text(empty)
        } else if self.revealed {
            screen.text(typed).secondary(length)
        } else {
            screen.text(length)
        };
        screen.keyboard(self.entry.keyboard(), "Save").build()
    }

    pub fn on_action(
        &mut self,
        context: &mut Context,
        action: ActionId,
    ) -> Option<CredentialEvent> {
        if matches!(self.view, View::Password | View::Entry) && action == action_id(REVEAL) {
            self.revealed = !self.revealed;
            return Some(CredentialEvent::Changed);
        }
        if matches!(self.view, View::Username | View::Password) {
            if action == ActionId::BACK {
                self.entry.close();
                self.username.clear();
                self.revealed = false;
                self.view = View::Prompt;
                return Some(CredentialEvent::Changed);
            }
            let typing = if self.view == View::Password {
                self.entry.handle_verbatim(action)
            } else {
                self.entry.handle(action)
            }?;
            match typing {
                Typing::Submitted(value) if self.view == View::Username => {
                    if value.contains(':') || value.chars().any(char::is_control) {
                        self.problem =
                            Some("Enter a username without a colon or line break.".into());
                        self.view = View::Prompt;
                    } else {
                        self.username = value;
                        self.entry.open();
                        self.view = View::Password;
                    }
                }
                Typing::Submitted(password) => {
                    self.submit(context, format!("{}:{password}", self.username));
                }
                Typing::Cancelled => {
                    self.revealed = false;
                    self.view = View::Prompt;
                }
                Typing::Changed => {}
            }
            return Some(CredentialEvent::Changed);
        }
        if self.view == View::Entry {
            if action == ActionId::BACK {
                self.entry.close();
                self.revealed = false;
                self.view = View::Prompt;
                return Some(CredentialEvent::Changed);
            }
            return match self.entry.handle_verbatim(action) {
                Some(Typing::Submitted(value)) => {
                    self.submit(context, value);
                    Some(CredentialEvent::Changed)
                }
                Some(Typing::Cancelled) => {
                    self.revealed = false;
                    self.view = View::Prompt;
                    Some(CredentialEvent::Changed)
                }
                Some(Typing::Changed) => Some(CredentialEvent::Changed),
                None => None,
            };
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
                self.entry.open();
                self.view = if self.basic {
                    View::Username
                } else {
                    View::Entry
                };
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

    /// The answer to [`Self::recheck`] or to a Check again tap.
    fn on_checked(&mut self, result: &DeviceResult) -> Option<CredentialEvent> {
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

    pub fn on_device_result(
        &mut self,
        request: &DeviceRequest,
        result: &DeviceResult,
    ) -> Option<CredentialEvent> {
        if let DeviceRequest::CheckSecrets { names } = request {
            return if self.checking != Checking::Idle && names.len() == 1 && names[0] == self.name {
                self.on_checked(result)
            } else {
                None
            };
        }
        let name = match request {
            DeviceRequest::SetSecret { name, .. } if self.server.is_none() => name,
            DeviceRequest::SetServerSecret { name, server, .. }
                if self.server.as_ref() == Some(server) =>
            {
                name
            }
            _ => return None,
        };
        if name != &self.name || self.view != View::Saving {
            return None;
        }
        match result {
            DeviceResult::Done => {
                self.close();
                Some(CredentialEvent::Saved)
            }
            DeviceResult::Denied(_) => {
                self.problem =
                    Some("This app cannot save this account. Update Cobalt and try again.".into());
                self.view = View::Prompt;
                Some(CredentialEvent::Changed)
            }
            DeviceResult::Failed(_) => {
                self.problem = Some(
                    "Account details were not saved. Check the reader’s free space and try again."
                        .into(),
                );
                self.view = View::Prompt;
                Some(CredentialEvent::Changed)
            }
            _ => {
                self.problem = Some("Saving could not be confirmed. Try again.".to_owned());
                self.view = View::Prompt;
                Some(CredentialEvent::Changed)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CredentialEvent, CredentialSetup};
    use crate::{action_id, Command, Context, DeviceRequest, DeviceResult};

    #[test]
    fn server_account_waits_for_matching_ack_and_refuses_oversized_pairs() {
        let mut setup = CredentialSetup::new("komga", "Komga").with_basic();
        let mut context = Context::default();
        setup.bind_server("https://books.example").unwrap();
        setup.open();
        setup.on_action(&mut context, action_id("credential.enter"));
        setup.entry.open_with("reader");
        setup.on_action(&mut context, action_id("kb.enter"));
        setup.entry.open_with("  private  ");
        setup.on_action(&mut context, action_id("kb.enter"));
        let commands = context.take_commands();
        let [Command::Device(
            request @ DeviceRequest::SetServerSecret {
                name,
                server,
                value,
            },
        )] = commands.as_slice()
        else {
            panic!("missing bound save")
        };
        assert_eq!(name, "komga");
        assert_eq!(server, "https://books.example");
        assert_eq!(value.as_str(), "reader:  private  ");
        setup.on_action(&mut context, crate::ActionId::BACK);
        assert_eq!(setup.view, super::View::Saving);
        assert!(setup.bind_server("https://another.example").is_err());
        assert_eq!(
            setup.on_device_result(
                &DeviceRequest::SetSecret {
                    name: "komga".into(),
                    value: kobo_protocol::SecretValue::new("x")
                },
                &DeviceResult::Done
            ),
            None
        );
        assert_eq!(
            setup.on_device_result(request, &DeviceResult::Done),
            Some(CredentialEvent::Saved)
        );
        setup.open();
        setup.on_action(&mut context, action_id("credential.enter"));
        setup.entry.open_with("u".repeat(300));
        setup.on_action(&mut context, action_id("kb.enter"));
        setup.entry.open_with("p".repeat(300));
        setup.on_action(&mut context, action_id("kb.enter"));
        assert!(context.take_commands().is_empty());
        assert_eq!(setup.view, super::View::Prompt);
        assert!(setup.problem.is_some());
    }

    #[test]
    fn entered_secret_is_sent_to_the_runtime_and_never_drawn() {
        let mut setup = CredentialSetup::new("zotero", "Zotero");
        let mut context = Context::default();
        setup.open();
        setup.on_action(&mut context, action_id("credential.enter"));
        setup.entry.open_with("visible-secret");
        let screen = setup.screen("Zotero Reader");
        assert!(!format!("{screen:?}").contains("visible-secret"));
        assert!(!format!("{setup:?}").contains("visible-secret"));
        setup.on_action(&mut context, action_id("kb.enter"));
        let commands = context.take_commands();
        assert!(commands.iter().any(|command| matches!(
            command,
            Command::Device(DeviceRequest::SetSecret { name, value })
                if name == "zotero" && value.as_str() == "visible-secret"
        )));
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
    fn basic_account_fields_preserve_password_spaces_without_showing_them() {
        for password in ["  private pass  ", " "] {
            let mut setup = CredentialSetup::new("komga", "Komga").with_basic();
            let mut context = Context::default();
            setup.open();
            setup.on_action(&mut context, action_id("credential.enter"));
            setup.entry.open_with("reader");
            setup.on_action(&mut context, action_id("kb.enter"));
            assert_eq!(setup.view, super::View::Password);
            setup.entry.open_with(password);
            assert!(!format!("{:?}", setup.screen("Komga")).contains("private pass"));
            assert!(!format!("{setup:?}").contains("reader"));
            setup.on_action(&mut context, action_id("kb.enter"));
            let expected = format!("reader:{password}");
            assert!(
                matches!(context.take_commands().as_slice(), [Command::Device(DeviceRequest::SetSecret { name, value })] if name == "komga" && value.as_str() == expected)
            );
            assert!(setup.username.is_empty());
            assert!(setup.entry.text().is_empty());
        }
    }

    #[test]
    fn invalid_usernames_and_cancelled_passwords_never_save_an_account() {
        let mut setup = CredentialSetup::new("komga", "Komga").with_basic();
        let mut context = Context::default();
        setup.open();
        setup.on_action(&mut context, action_id("credential.enter"));
        setup.entry.open_with("user:password");
        setup.on_action(&mut context, action_id("kb.enter"));
        assert!(context.take_commands().is_empty());
        assert!(setup.problem.is_some());
        setup.on_action(&mut context, action_id("credential.enter"));
        setup.entry.open_with("reader");
        setup.on_action(&mut context, action_id("kb.enter"));
        setup.entry.open_with("private password");
        setup.on_action(&mut context, crate::ActionId::BACK);
        setup.close();
        assert!(setup.entry.text().is_empty());
        assert!(setup.username.is_empty());
        assert!(context.take_commands().is_empty());
    }

    #[test]
    fn account_fields_and_save_feedback_fit_all_interface_sizes() {
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
                setup.problem = Some("No Komga account is on this reader yet.".into());
                for (view, revealed) in [
                    (super::View::Prompt, false),
                    (super::View::Username, false),
                    (super::View::Password, false),
                    (super::View::Password, true),
                    (super::View::Entry, false),
                    (super::View::Entry, true),
                    (super::View::Cli, false),
                    (super::View::Saving, false),
                ] {
                    setup.view = view;
                    setup.revealed = revealed;
                    setup.entry.open_with("fake-token-abcdefghijklm");
                    let screen = setup.screen("Komga").with_own_back(true);
                    let chrome = kobo_ui::Chrome::for_screen(&screen, false, None);
                    let errors = screen
                        .diagnostics(&metrics, &chrome)
                        .issues
                        .into_iter()
                        .filter(|issue| issue.severity == kobo_ui::DiagnosticSeverity::Error)
                        .collect::<Vec<_>>();
                    assert!(
                        errors.is_empty(),
                        "{metrics:?} {view:?} shown {revealed}: {errors:?}"
                    );
                }
            }
        }
    }

    fn drawn(setup: &CredentialSetup) -> String {
        format!("{:?}", setup.screen("Lichess"))
    }

    #[test]
    fn a_typed_key_shows_its_length_and_shows_itself_only_when_asked() {
        let mut setup = CredentialSetup::new("lichess", "Lichess");
        let mut context = Context::default();
        setup.open();
        setup.on_action(&mut context, action_id("credential.enter"));
        assert!(drawn(&setup).contains("Type your account key"));
        setup.entry.open_with("fake-token-abcdefghijklm");
        let hidden = drawn(&setup);
        assert!(hidden.contains("24 characters entered"), "{hidden}");
        assert!(hidden.contains("Show"), "{hidden}");
        assert!(!hidden.contains("fake-token-abcdefghijklm"), "{hidden}");

        setup.on_action(&mut context, action_id("credential.reveal"));
        let shown = drawn(&setup);
        assert!(shown.contains("fake-token-abcdefghijklm"), "{shown}");
        assert!(shown.contains("Hide"), "{shown}");
        assert!(context.take_commands().is_empty(), "showing saves nothing");

        setup.on_action(&mut context, action_id("credential.reveal"));
        assert!(!drawn(&setup).contains("fake-token-abcdefghijklm"));

        setup.on_action(&mut context, action_id("credential.reveal"));
        setup.on_action(&mut context, crate::ActionId::BACK);
        setup.on_action(&mut context, action_id("credential.enter"));
        setup.entry.open_with("x");
        let reopened = drawn(&setup);
        assert!(reopened.contains("1 character entered"), "{reopened}");
        assert!(
            !reopened.contains("text: \"x\""),
            "a key shown once is hidden again on the next entry: {reopened}"
        );
    }

    #[test]
    fn a_password_can_be_shown_but_a_username_screen_has_no_reveal_to_press() {
        let mut setup = CredentialSetup::new("calibre", "calibre-web").with_basic();
        let mut context = Context::default();
        setup.open();
        setup.on_action(&mut context, action_id("credential.enter"));
        assert_eq!(
            setup.on_action(&mut context, action_id("credential.reveal")),
            None
        );
        setup.entry.open_with("reader");
        setup.on_action(&mut context, action_id("kb.enter"));
        setup.entry.open_with("open sesame");
        assert!(drawn(&setup).contains("11 characters entered"));
        setup.on_action(&mut context, action_id("credential.reveal"));
        assert!(drawn(&setup).contains("open sesame"));
    }

    #[test]
    fn check_again_connects_an_account_that_arrived_from_the_computer() {
        let mut setup = CredentialSetup::new("zotero", "Zotero");
        let mut context = Context::default();
        setup.open();
        setup.on_action(&mut context, action_id("credential.cli"));
        let route = drawn(&setup);
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
        assert!(
            drawn(&setup).contains("No Zotero account is on this reader yet"),
            "{}",
            drawn(&setup)
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
        assert!(!drawn(&setup).contains("No Zotero account is on this reader yet"));
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
        assert!(!drawn(&bare).contains("Use computer"));
        assert!(drawn(&bare).contains("Check again"));
        assert_eq!(
            bare.on_action(&mut context, action_id("credential.cli")),
            None
        );

        let mut guided = CredentialSetup::new("readeck", "Readeck")
            .with_computer_command("kobo connect readeck");
        guided.bind_server("https://read.example").unwrap();
        guided.open();
        guided.on_action(&mut context, action_id("credential.cli"));
        assert!(drawn(&guided).contains("kobo connect readeck"));
    }

    #[test]
    fn successful_install_closes_the_flow() {
        let mut setup = CredentialSetup::new("zotero", "Zotero");
        setup.open();
        setup.view = super::View::Saving;
        assert_eq!(
            setup.on_device_result(
                &DeviceRequest::SetSecret {
                    name: "zotero".to_owned(),
                    value: kobo_protocol::SecretValue::new("hidden"),
                },
                &DeviceResult::Done,
            ),
            Some(CredentialEvent::Saved)
        );
        assert!(!setup.is_open());
    }
}

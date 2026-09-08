//! Consistent owner onboarding for runtime-held application credentials.

use crate::keyboard::{TextEntry, Typing};
use crate::{action_id, ActionId, Context, DeviceRequest, DeviceResult, Screen, ScreenBuilder};
use std::fmt;

const ENTER: &str = "credential.enter";
const CLI: &str = "credential.cli";
const BACK: &str = "credential.back";

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
        }
    }

    /// Collect HTTP Basic account details as separate username and password
    /// fields. The runtime still owns encoding and storage.
    #[must_use]
    pub const fn with_basic(mut self) -> Self {
        self.basic = true;
        self
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
        self.view = View::Closed;
    }

    #[must_use]
    pub fn screen(&self, app_title: &str) -> Screen {
        let command = format!("kobo secret set {} --device <address>", self.name);
        match self.view {
            View::Closed => ScreenBuilder::new("credential-closed")
                .top_bar(app_title)
                .build(),
            View::Prompt => {
                let mut screen = ScreenBuilder::new("credential-required")
                    .top_bar(app_title)
                    .heading(format!("Connect {}", self.service))
                    .text(if self.basic {
                        "Use your server username and password."
                    } else {
                        "Add the account key for this service."
                    });
                if let Some(problem) = &self.problem {
                    screen = screen.banner(crate::BannerLevel::Attention, problem);
                }
                screen
                    .button(
                        ENTER,
                        if self.basic {
                            "Sign in"
                        } else {
                            "Enter account key"
                        },
                    )
                    .button(CLI, "Use computer")
                    .build()
            }
            View::Username => ScreenBuilder::new("account-username")
                .top_bar("Username")
                .owns_back(true)
                .typed(self.entry.keyboard(), "Type your username")
                .keyboard(self.entry.keyboard(), "Next")
                .build(),
            View::Password => {
                self.private_entry("Password", "Type your password", "Password entered")
            }
            View::Entry => self.private_entry(
                "Account key",
                "Type your account key",
                "Account key entered",
            ),
            View::Cli => ScreenBuilder::new("credential-cli")
                .top_bar(app_title)
                .heading("Set it from a computer")
                .text(command)
                .secondary("After the command succeeds, close and reopen this app.")
                .button(BACK, "Back")
                .build(),
            View::Saving => ScreenBuilder::new("credential-saving")
                .top_bar(app_title)
                .heading("Saving account details")
                .text("Keep this page open until saving finishes.")
                .build(),
        }
    }

    fn private_entry(&self, title: &str, empty: &str, entered: &str) -> Screen {
        ScreenBuilder::new("account-private-entry")
            .top_bar(title)
            .owns_back(true)
            .text(if self.entry.text().is_empty() {
                empty
            } else {
                entered
            })
            .keyboard(self.entry.keyboard(), "Save")
            .build()
    }

    pub fn on_action(
        &mut self,
        context: &mut Context,
        action: ActionId,
    ) -> Option<CredentialEvent> {
        if matches!(self.view, View::Username | View::Password) {
            if action == ActionId::BACK {
                self.entry.close();
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
                    context
                        .secrets()
                        .set(self.name.clone(), format!("{}:{password}", self.username));
                    self.username.clear();
                    self.view = View::Saving;
                }
                Typing::Cancelled => self.view = View::Prompt,
                Typing::Changed => {}
            }
            return Some(CredentialEvent::Changed);
        }
        if self.view == View::Entry {
            if action == ActionId::BACK {
                self.view = View::Prompt;
                return Some(CredentialEvent::Changed);
            }
            return match self.entry.handle_verbatim(action) {
                Some(Typing::Submitted(value)) => {
                    context.secrets().set(self.name.clone(), value);
                    self.view = View::Saving;
                    Some(CredentialEvent::Changed)
                }
                Some(Typing::Cancelled) => {
                    self.view = View::Prompt;
                    Some(CredentialEvent::Changed)
                }
                Some(Typing::Changed) => Some(CredentialEvent::Changed),
                None => None,
            };
        }
        if action == ActionId::BACK {
            self.close();
            return Some(CredentialEvent::Cancelled);
        }
        match (self.view, action) {
            (View::Prompt, value) if value == action_id(ENTER) => {
                self.entry.open();
                self.view = if self.basic {
                    View::Username
                } else {
                    View::Entry
                };
                Some(CredentialEvent::Changed)
            }
            (View::Prompt, value) if value == action_id(CLI) => {
                self.view = View::Cli;
                Some(CredentialEvent::Changed)
            }
            (View::Cli, value) if value == action_id(BACK) => {
                self.view = View::Prompt;
                Some(CredentialEvent::Changed)
            }
            _ => None,
        }
    }

    pub fn on_device_result(
        &mut self,
        request: &DeviceRequest,
        result: &DeviceResult,
    ) -> Option<CredentialEvent> {
        let DeviceRequest::SetSecret { name, .. } = request else {
            return None;
        };
        if name != &self.name || self.view != View::Saving {
            return None;
        }
        match result {
            DeviceResult::Done => {
                self.close();
                Some(CredentialEvent::Saved)
            }
            DeviceResult::Denied(reason) => {
                self.problem = Some(format!("The runtime refused this credential: {reason}."));
                self.view = View::Prompt;
                Some(CredentialEvent::Changed)
            }
            DeviceResult::Failed(error) => {
                self.problem = Some(format!("The credential could not be saved: {error}."));
                self.view = View::Prompt;
                Some(CredentialEvent::Changed)
            }
            _ => {
                self.problem = Some("The runtime returned an unexpected answer.".to_owned());
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
        let panels = kobo_profile::SUPPORTED_PROFILES
            .iter()
            .map(|profile| crate::DisplayMetrics {
                width: i32::try_from(profile.width).unwrap(),
                height: i32::try_from(profile.height).unwrap(),
                pixels_per_inch: i32::from(profile.pixels_per_inch),
                text_scale: kobo_ui::TextScale::Default,
            })
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
                for view in [
                    super::View::Prompt,
                    super::View::Username,
                    super::View::Password,
                    super::View::Entry,
                    super::View::Saving,
                ] {
                    setup.view = view;
                    setup.entry.open_with("private password");
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

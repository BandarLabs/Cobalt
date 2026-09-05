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
        }
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
                    .text("This app needs a private credential before it can connect.");
                if let Some(problem) = &self.problem {
                    screen = screen.banner(crate::BannerLevel::Attention, problem);
                }
                screen
                    .buttons([(ENTER, "Enter credential"), (CLI, "Use computer")])
                    .build()
            }
            View::Entry => ScreenBuilder::new("credential-entry")
                .top_bar(app_title)
                .secret_entry(
                    &self.entry,
                    &format!("Enter the {} credential", self.service),
                    "Save",
                )
                .build(),
            View::Cli => ScreenBuilder::new("credential-cli")
                .top_bar(app_title)
                .heading("Set it from a computer")
                .text(command)
                .secondary("After the command succeeds, close and reopen this app.")
                .button(BACK, "Back")
                .build(),
            View::Saving => ScreenBuilder::new("credential-saving")
                .top_bar(app_title)
                .heading("Saving credential")
                .text("The runtime is storing it privately.")
                .build(),
        }
    }

    pub fn on_action(
        &mut self,
        context: &mut Context,
        action: ActionId,
    ) -> Option<CredentialEvent> {
        if self.view == View::Entry {
            return match self.entry.handle(action) {
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
                self.view = View::Entry;
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

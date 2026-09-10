//! Provider setup composed from the shared address, credential and task flows.
//! The app validates its provider response and acknowledges saving the address.

use crate::credentials::{CredentialEvent, CredentialSetup};
use crate::keyboard::{TextEntry, Typing};
use crate::{
    action_id, ActionId, BannerLevel, Context, Credential, DeviceRequest, DeviceResult, Failure,
    Screen, ScreenBuilder, Task, TaskId, TaskOutcome,
};

pub const ADDRESS: &str = "provider.address";
pub const ACCOUNT: &str = "provider.account";
pub const TEST: &str = "provider.test";
pub const SAMPLE: &str = "provider.sample";
pub const CANCEL: &str = "provider.cancel";

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Event {
    Changed,
    /// Persist through the app's acknowledged configuration store.
    AddressChanged(String),
    /// HTTP success is not proof of a valid provider. Parse this bounded body
    /// before calling `verified` or `invalid_response`.
    Response(Vec<u8>),
    Sample,
    Closed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Connection {
    Unchecked,
    AwaitingValidation,
    Verified,
}

pub struct ProviderSetup {
    service: String,
    address: String,
    credential: Credential,
    probe: String,
    credentials: CredentialSetup,
    entry: TextEntry,
    task: Option<TaskId>,
    connection: Connection,
    advice: Option<String>,
    needs_wifi: bool,
    sample: bool,
    server_accounts: bool,
    samples: Option<crate::samples::Collection>,
}

impl std::fmt::Debug for ProviderSetup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderSetup")
            .field("service", &self.service)
            .field("connection", &self.connection)
            .field("task", &self.task)
            .finish_non_exhaustive()
    }
}
impl ProviderSetup {
    /// `probe` is a fixed provider API path relative to the configured base.
    /// Secrets remain runtime-owned and are never part of the saved address.
    ///
    /// # Errors
    /// Rejects malformed service metadata or a probe that could change origin.
    pub fn new(service: &str, secret: &str, probe: &str, sample: bool) -> Result<Self, String> {
        if service.is_empty()
            || service.len() > 64
            || service.chars().any(char::is_control)
            || secret.is_empty()
            || secret.len() > 64
            || !secret
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
            || !probe.starts_with('/')
            || probe.starts_with("//")
            || probe.len() > 512
            || probe.contains(['#', '\\'])
            || probe.chars().any(char::is_whitespace)
        {
            return Err("Invalid provider setup configuration.".into());
        }
        Ok(Self {
            service: service.into(),
            address: String::new(),
            credential: Credential::bearer(secret),
            probe: probe.into(),
            credentials: CredentialSetup::new(secret, service),
            entry: TextEntry::new(),
            task: None,
            connection: Connection::Unchecked,
            advice: None,
            needs_wifi: false,
            sample,
            server_accounts: false,
            samples: None,
        })
    }

    /// Offer an original offline collection through the existing sample action.
    /// The app opens a separate sample session on `Event::Sample`; no provider
    /// response, credentials or durable owner data are synthesized here.
    #[must_use]
    pub fn with_samples(mut self, collection: crate::samples::Collection) -> Self {
        self.sample = true;
        self.samples = Some(collection);
        self
    }

    #[must_use]
    pub const fn samples(&self) -> Option<crate::samples::Collection> {
        self.samples
    }

    /// Select the provider's supported header convention without exposing its
    /// value. Runtime destination/credential policy remains authoritative.
    ///
    /// # Errors
    /// Refuses malformed header names before the connection request is built.
    pub fn with_authentication(mut self, header: crate::SecretHeader) -> Result<Self, String> {
        let credentials = CredentialSetup::new(&self.credential.secret, &self.service);
        self.credentials = if header == crate::SecretHeader::Basic {
            credentials.with_basic()
        } else {
            credentials
        };
        self.credential.header = header;
        if !self.credential.is_well_formed() {
            return Err("Invalid account header configuration.".into());
        }
        Ok(self)
    }

    /// Bind entered accounts to the configured server. Enable only for a
    /// provider supported by the runtime's server account policy.
    #[must_use]
    pub const fn with_server_accounts(mut self) -> Self {
        self.server_accounts = true;
        self
    }

    /// Validate before replacing the previous address; never silently downgrade
    /// HTTPS or accept credentials/query tokens in a base URL.
    ///
    /// # Errors
    /// Returns owner-facing guidance without echoing entered credentials.
    pub fn restore_address(&mut self, address: &str) -> Result<(), String> {
        let address = normalize_address(address)?;
        self.address = address;
        self.task = None;
        self.connection = Connection::Unchecked;
        Ok(())
    }
    #[must_use]
    pub fn address(&self) -> &str {
        &self.address
    }
    #[must_use]
    pub const fn is_connected(&self) -> bool {
        matches!(self.connection, Connection::Verified)
    }

    /// Only acknowledge a current response after parsing the expected provider
    /// schema. Late acknowledgements after an address change cannot connect it.
    pub fn verified(&mut self) -> bool {
        if self.connection != Connection::AwaitingValidation {
            return false;
        }
        self.connection = Connection::Verified;
        self.advice = None;
        true
    }
    pub fn invalid_response(&mut self) {
        if self.connection == Connection::AwaitingValidation {
            self.connection = Connection::Unchecked;
            self.advice = Some(
                "This address did not return the expected content. Check it and try again.".into(),
            );
        }
    }
    fn cancel(&mut self, context: &mut Context) {
        if self.connection == Connection::AwaitingValidation {
            self.connection = Connection::Unchecked;
        }
        if let Some(task) = self.task.take() {
            context.cancel(task);
        }
    }

    #[must_use]
    pub fn screen(&self) -> Screen {
        if self.credentials.is_open() {
            return self.credentials.screen(&self.service);
        }
        if self.entry.is_open() {
            return ScreenBuilder::new("provider-address")
                .top_bar("Server address")
                .owns_back(true)
                .text_entry(&self.entry, "HTTPS address", "Use address")
                .build();
        }
        let mut screen = ScreenBuilder::new("provider-setup")
            .top_bar(format!("Connect {}", self.service))
            .owns_back(true);
        if self.needs_wifi {
            screen = screen.top_bar_action(crate::JOIN_WIFI, "Wi-Fi");
        }
        if let Some(advice) = &self.advice {
            screen = screen.banner(BannerLevel::Attention, advice);
        }
        screen = screen
            .field(ADDRESS, &self.address, "Add server address")
            .button(ACCOUNT, "Account details");
        screen = if self.task.is_some() || self.connection == Connection::AwaitingValidation {
            screen
                .secondary("Checking connection…")
                .button(CANCEL, "Cancel check")
        } else {
            screen
                .secondary(if self.connection == Connection::Verified {
                    "Connection checked"
                } else {
                    "Check the connection before loading your library."
                })
                .button(TEST, "Check connection")
        };
        if self.sample {
            screen = screen.bottom_action(SAMPLE, "Try a sample");
        }
        screen.build()
    }

    pub fn on_action(&mut self, context: &mut Context, action: ActionId) -> Option<Event> {
        if self.credentials.is_open() {
            return self
                .credentials
                .on_action(context, action)
                .map(|_| Event::Changed);
        }
        if self.entry.is_open() {
            if action == ActionId::BACK {
                self.entry.close();
                return Some(Event::Changed);
            }
            return self.entry.handle(action).map(|typing| match typing {
                Typing::Submitted(value) => match self.restore_address(&value) {
                    Ok(()) => {
                        self.advice = None;
                        Event::AddressChanged(self.address.clone())
                    }
                    Err(reason) => {
                        self.advice = Some(reason);
                        Event::Changed
                    }
                },
                _ => Event::Changed,
            });
        }
        if action == ActionId::BACK {
            self.cancel(context);
            return Some(Event::Closed);
        }
        if action == action_id(ADDRESS) {
            self.cancel(context);
            self.entry.open_with(&self.address);
        } else if action == action_id(ACCOUNT) {
            self.cancel(context);
            self.connection = Connection::Unchecked;
            if self.server_accounts {
                if let Err(reason) = self.credentials.bind_server(&self.address) {
                    self.advice = Some(reason);
                    return Some(Event::Changed);
                }
            }
            self.credentials.open();
        } else if action == action_id(TEST) {
            self.cancel(context);
            self.connection = Connection::Unchecked;
            self.advice = None;
            self.needs_wifi = false;
            if let Err(reason) = normalize_address(&self.address) {
                self.advice = Some(reason);
            } else {
                self.task = context.spawn(Task::Fetch {
                    url: format!("{}{}", self.address, self.probe),
                    offset: 0,
                    max_bytes: 64 * 1024,
                    credential: Some(self.credential.clone()),
                    headers: Vec::new(),
                });
                if self.task.is_none() {
                    self.advice =
                        Some("The reader is busy. Try the connection check again.".into());
                }
            }
        } else if action == action_id(CANCEL) {
            self.cancel(context);
        } else if action == action_id(SAMPLE) && self.sample {
            self.cancel(context);
            return Some(Event::Sample);
        } else {
            return None;
        }
        Some(Event::Changed)
    }

    pub fn on_task(&mut self, id: TaskId, outcome: &TaskOutcome) -> Option<Event> {
        if self.task != Some(id) {
            return None;
        }
        self.task = None;
        match outcome {
            TaskOutcome::Completed(bytes) => {
                self.connection = Connection::AwaitingValidation;
                Some(Event::Response(bytes.clone()))
            }
            TaskOutcome::Failed(error) => {
                self.needs_wifi = *error == crate::TaskError::Offline;
                self.advice = Some(match error {
                    crate::TaskError::NoCredential => "Add account details, then check the connection again.",
                    crate::TaskError::Unauthorized => "The server did not accept your account details. Update them and check again.",
                    _ => Failure::of(*error).advice,
                }.into());
                Some(Event::Changed)
            }
            TaskOutcome::Cancelled => Some(Event::Changed),
        }
    }
    pub fn on_device_result(
        &mut self,
        request: &DeviceRequest,
        result: &DeviceResult,
    ) -> Option<Event> {
        self.credentials
            .on_device_result(request, result)
            .map(|event| {
                if event == CredentialEvent::Saved {
                    self.connection = Connection::Unchecked;
                    self.advice =
                        Some("Account details saved. Check the connection to continue.".into());
                }
                Event::Changed
            })
    }
}

fn normalize_address(address: &str) -> Result<String, String> {
    let address = address.trim().trim_end_matches('/');
    if address.len() > 512
        || address.contains(['?', '#', '\\'])
        || address.chars().any(char::is_whitespace)
    {
        return Err("Use the server's HTTPS address without a sign-in link or query.".into());
    }
    kobo_net::parse(address).map_err(|_| {
        "Enter an HTTPS server address. Keep account details in Account details.".to_string()
    })?;
    Ok(address.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bad_addresses_do_not_replace_a_working_server_or_echo_private_values() {
        let mut setup = ProviderSetup::new("Articles", "articles", "/api/account", true).unwrap();
        setup
            .restore_address(" https://library.example/books/ ")
            .unwrap();
        for address in [
            "http://library.example",
            "https://private:password@library.example",
            "https://library.example?token=private",
            "https://library.example/#private",
            "not a URL",
        ] {
            let error = setup.restore_address(address).unwrap_err();
            assert!(!error.contains("private"));
            assert_eq!(setup.address(), "https://library.example/books");
        }
    }
    #[test]
    fn success_requires_provider_validation_and_stale_results_are_ignored() {
        let mut setup = ProviderSetup::new("Articles", "articles", "/api/account", false).unwrap();
        setup.restore_address("https://library.example").unwrap();
        let mut context = Context::default();
        setup.on_action(&mut context, action_id(TEST));
        let first = setup.task.unwrap();
        assert_eq!(
            setup.on_task(first, &TaskOutcome::Completed(b"not account JSON".to_vec())),
            Some(Event::Response(b"not account JSON".to_vec()))
        );
        assert!(!setup.is_connected());
        setup.invalid_response();
        assert!(!setup.verified());
        setup.on_action(&mut context, action_id(TEST));
        let second = setup.task.unwrap();
        setup.on_action(&mut context, action_id(CANCEL));
        assert_eq!(setup.on_task(second, &TaskOutcome::Completed(vec![])), None);
        assert!(!setup.verified());
        setup.on_action(&mut context, action_id(TEST));
        let third = setup.task.unwrap();
        setup.on_task(
            third,
            &TaskOutcome::Completed(b"valid provider fixture".to_vec()),
        );
        assert!(setup.verified());
        assert!(setup.is_connected());
        setup.restore_address("https://another.example").unwrap();
        assert!(!setup.is_connected());
    }
    #[test]
    fn production_failure_mapping_does_not_claim_connected() {
        let mut setup = ProviderSetup::new("Articles", "articles", "/api/account", false).unwrap();
        setup.restore_address("https://library.example").unwrap();
        let mut context = Context::default();
        for error in [
            crate::TaskError::Offline,
            crate::TaskError::NoCredential,
            crate::TaskError::Unauthorized,
            crate::TaskError::TimedOut,
        ] {
            setup.on_action(&mut context, action_id(TEST));
            setup.on_task(setup.task.unwrap(), &TaskOutcome::Failed(error));
            assert!(setup.advice.is_some());
            assert_eq!(setup.needs_wifi, error == crate::TaskError::Offline);
            assert!(!setup.is_connected());
        }
    }
    #[test]
    fn connection_probe_uses_the_providers_runtime_credential_convention() {
        for header in [
            crate::SecretHeader::Bearer,
            crate::SecretHeader::Basic,
            crate::SecretHeader::Named("X-Auth-Token".into()),
        ] {
            let mut setup = ProviderSetup::new("Articles", "articles", "/api/account", false)
                .unwrap()
                .with_authentication(header.clone())
                .unwrap();
            setup.restore_address("https://library.example").unwrap();
            let mut context = Context::default();
            setup.on_action(&mut context, action_id(TEST));
            assert!(context.commands().iter().any(|command| matches!(command, crate::Command::Spawn { work: Task::Fetch { credential: Some(credential), .. }, .. } if credential.header == header && credential.secret == "articles")));
        }
        assert!(
            ProviderSetup::new("Articles", "articles", "/api/account", false)
                .unwrap()
                .with_authentication(crate::SecretHeader::Named("bad\r\nheader".into()))
                .is_err()
        );
    }

    #[test]
    fn sample_choice_exposes_original_data_without_contacting_or_verifying_an_account() {
        let mut setup = ProviderSetup::new("Notes", "notes", "/api/account", false)
            .unwrap()
            .with_samples(crate::samples::Collection::Notes);
        let mut context = Context::default();
        assert_eq!(
            setup.on_action(&mut context, action_id(SAMPLE)),
            Some(Event::Sample)
        );
        assert_eq!(setup.samples().unwrap().items().len(), 12);
        assert!(context.commands().is_empty());
        assert_eq!(setup.connection, Connection::Unchecked);
        assert!(setup.address.is_empty());
    }

    #[test]
    fn provider_setup_controls_fit_profiles_and_large_text() {
        for profile in kobo_profile::SUPPORTED_PROFILES {
            for scale in [
                kobo_ui::TextScale::Default,
                kobo_ui::TextScale::Large,
                kobo_ui::TextScale::ExtraLarge,
            ] {
                let metrics = crate::DisplayMetrics {
                    width: i32::try_from(profile.width).unwrap(),
                    height: i32::try_from(profile.height).unwrap(),
                    pixels_per_inch: i32::from(profile.pixels_per_inch),
                    text_scale: scale,
                };
                #[cfg(feature = "text")]
                kobo_text::install(metrics).unwrap();
                let mut setup = ProviderSetup::new("Miniflux", "miniflux", "/v1/me", true).unwrap();
                setup
                    .restore_address("https://library.example/reader")
                    .unwrap();
                let mut context = Context::default();
                for action in [TEST, CANCEL, ACCOUNT, "credential.cli", "credential.back"] {
                    setup.on_action(&mut context, action_id(action));
                    let screen = setup.screen();
                    let chrome = kobo_ui::Chrome::for_screen(&screen, false, None);
                    let diagnostics = screen.diagnostics(&metrics, &chrome);
                    let errors = diagnostics
                        .issues
                        .iter()
                        .filter(|issue| issue.severity == kobo_ui::DiagnosticSeverity::Error)
                        .collect::<Vec<_>>();
                    assert!(
                        errors.is_empty(),
                        "{} {scale:?} {action}: {errors:?}",
                        profile.id
                    );
                }
                setup.credentials.close();
                setup.on_action(&mut context, action_id(TEST));
                setup.on_task(
                    setup.task.unwrap(),
                    &TaskOutcome::Failed(crate::TaskError::Offline),
                );
                let screen = setup.screen();
                let chrome = kobo_ui::Chrome::for_screen(&screen, false, None);
                assert!(
                    screen
                        .diagnostics(&metrics, &chrome)
                        .issues
                        .iter()
                        .all(|issue| issue.severity != kobo_ui::DiagnosticSeverity::Error),
                    "{} {scale:?} offline",
                    profile.id
                );
                setup
                    .entry
                    .open_with("https://private:password@library.example");
                assert!(!format!("{setup:?}").contains("password"));
            }
        }
    }
}

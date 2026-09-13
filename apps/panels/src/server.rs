//! Acknowledged server settings around the shared account and connection UI.
use kobo_json::{ObjectBuilder, Value};
use kobo_sdk::provider::{Event, ProviderSetup};
use kobo_sdk::{action_id, ActionId, Context, Screen, ScreenBuilder, SecretHeader, StoreResult};
use kobo_state::draft::{Draft, Status};
use kobo_state::record::{Error, Schema};

pub const KEY: &str = "server";
pub const CATALOG_PATH: &str = "/opds/v1.2/catalog";
const MAX_BYTES: usize = 2048;

pub struct Server {
    pub setup: ProviderSetup,
    draft: Option<Draft>,
    active: Option<u64>,
    error: Option<String>,
}
impl Default for Server {
    fn default() -> Self {
        Self {
            setup: ProviderSetup::new("Komga", "komga", CATALOG_PATH, false)
                .expect("fixed provider")
                .with_authentication(SecretHeader::Basic)
                .expect("basic authentication")
                .with_server_accounts(),
            draft: None,
            active: None,
            error: None,
        }
    }
}
fn schema() -> Schema {
    Schema::new("panels.server", 1, MAX_BYTES).expect("fixed schema")
}
fn encode(address: &str) -> Result<Vec<u8>, Error> {
    schema().encode(&ObjectBuilder::new().set("address", address).build())
}
impl Server {
    pub fn load(&mut self, result: &StoreResult) {
        if self.draft.is_some() {
            return;
        }
        let StoreResult::Loaded { key, value } = result else {
            self.error = Some(
                "The server settings could not be read. Try again when storage is available."
                    .into(),
            );
            return;
        };
        if key != KEY {
            return;
        }
        let restored = schema()
            .restore(value.as_deref(), |_, _| Err(Error::MigrationUnavailable))
            .and_then(|record| match record {
                None => Ok(String::new()),
                Some(record) => record
                    .payload
                    .get("address")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .ok_or(Error::Corrupt),
            });
        let address = match restored {
            Ok(address) => address,
            Err(error) => {
                self.error = Some(error.to_string());
                return;
            }
        };
        if !address.is_empty() {
            if let Err(error) = self.setup.restore_address(&address) {
                self.error = Some(error);
                return;
            }
        }
        self.draft = Some(
            Draft::restored(
                value
                    .clone()
                    .unwrap_or_else(|| encode("").expect("empty settings")),
                MAX_BYTES,
            )
            .expect("validated settings"),
        );
        self.error = None;
    }
    pub fn can_suspend(&self) -> bool {
        self.draft
            .as_ref()
            .is_none_or(|draft| draft.status() == Status::Saved)
    }
    pub fn is_saved(&self) -> bool {
        self.draft
            .as_ref()
            .is_some_and(|draft| draft.status() == Status::Saved)
    }
    pub fn catalog_url(&self) -> String {
        format!("{}{CATALOG_PATH}", self.setup.address())
    }
    pub fn screen(&self) -> Screen {
        let base = ScreenBuilder::new("panels-server-storage")
            .top_bar("Connect Komga")
            .owns_back(true);
        if let Some(error) = &self.error {
            return base
                .heading("Server settings could not be opened")
                .text(error)
                .bottom_action("server-retry", "Try again")
                .build();
        }
        match self.draft.as_ref().map(Draft::status) {
            None => base.activity("Opening server settings…", None).build(),
            Some(Status::Saved) => self.setup.screen(),
            Some(Status::Failed(error)) => base
                .heading("Address not saved")
                .text(error)
                .bottom_action("server-retry", "Retry saving")
                .build(),
            Some(Status::Saving | Status::Unsaved) => {
                base.activity("Saving server address…", None).build()
            }
        }
    }
    pub fn on_action(&mut self, context: &mut Context, action: ActionId) -> Option<Event> {
        if action == ActionId::BACK && !self.is_saved() {
            return Some(Event::Closed);
        }
        if action == action_id("server-retry") {
            if let Some(draft) = &mut self.draft {
                draft.retry();
                self.pump(context);
            } else {
                context.store().load(KEY);
            }
            return Some(Event::Changed);
        }
        if !self.is_saved() {
            return None;
        }
        let event = self.setup.on_action(context, action)?;
        if let Event::AddressChanged(address) = &event {
            if let Err(error) = self.save_address(context, address) {
                self.error = Some(error);
            }
        }
        Some(event)
    }
    fn save_address(&mut self, context: &mut Context, address: &str) -> Result<(), String> {
        let bytes = encode(address).map_err(|error| error.to_string())?;
        let draft = self
            .draft
            .as_mut()
            .ok_or("Open the server settings before changing them.")?;
        draft
            .replace(bytes)
            .map_err(|_| "The server address could not be kept. Try again.".to_owned())?;
        self.pump(context);
        Ok(())
    }
    pub fn pump(&mut self, context: &mut Context) {
        if let Some(write) = self.draft.as_mut().and_then(Draft::begin) {
            self.active = Some(write.revision);
            context.store().save(KEY, write.bytes);
        }
    }
    pub fn saved(&mut self, context: &mut Context, result: &StoreResult) {
        let Some(revision) = self.active else {
            return;
        };
        let outcome = match result {
            StoreResult::Saved { key } if key == KEY => Ok(()),
            StoreResult::Denied(_) => {
                Err("The server address was not saved. Free some space, then retry.".into())
            }
            _ => return,
        };
        self.active = None;
        if let Some(draft) = &mut self.draft {
            draft.finish(revision, outcome);
        }
        self.pump(context);
    }
}

#[cfg(test)]
mod tests {
    use super::{encode, Server, KEY};
    use kobo_sdk::{action_id, Command, Context, StoreError, StoreRequest, StoreResult};

    #[test]
    fn address_save_failure_retries_before_connection_can_be_checked() {
        let mut server = Server::default();
        let mut context = Context::default();
        server.load(&StoreResult::Loaded {
            key: KEY.into(),
            value: None,
        });
        server
            .setup
            .restore_address("https://library.example/comics/")
            .unwrap();
        server
            .save_address(&mut context, server.setup.address().to_owned().as_str())
            .unwrap();
        assert!(!server.is_saved());
        assert!(!server.can_suspend());
        assert!(
            matches!(context.take_commands().as_slice(), [Command::Store(StoreRequest::Save { key, .. })] if key == KEY)
        );
        server.saved(&mut context, &StoreResult::Denied(StoreError::NoRoom));
        server.on_action(&mut context, action_id(kobo_sdk::provider::TEST));
        assert!(
            context.take_commands().is_empty(),
            "connection must wait for a saved address"
        );
        server.on_action(&mut context, action_id("server-retry"));
        let commands = context.take_commands();
        let [Command::Store(StoreRequest::Save { value, .. })] = commands.as_slice() else {
            panic!("only retry address save");
        };
        let mut restored = Server::default();
        restored.load(&StoreResult::Loaded {
            key: KEY.into(),
            value: Some(value.clone()),
        });
        assert_eq!(
            restored.catalog_url(),
            "https://library.example/comics/opds/v1.2/catalog"
        );
        assert!(!restored.setup.is_connected());
        server.saved(&mut context, &StoreResult::Saved { key: KEY.into() });
        assert!(server.can_suspend());
        server.on_action(&mut context, action_id(kobo_sdk::provider::TEST));
        assert!(context
            .take_commands()
            .iter()
            .any(|command| matches!(command, Command::Spawn { .. })));
    }

    #[test]
    fn unreadable_and_unrecognized_settings_cannot_be_overwritten_by_setup() {
        for result in [
            StoreResult::Denied(StoreError::NoRoom),
            StoreResult::Loaded {
                key: KEY.into(),
                value: Some(b"broken".to_vec()),
            },
            StoreResult::Loaded {
                key: KEY.into(),
                value: Some(encode("https://user:password@example.com").unwrap()),
            },
        ] {
            let mut server = Server::default();
            let mut context = Context::default();
            server.load(&result);
            assert!(server.error.is_some());
            assert!(server.draft.is_none());
            server.on_action(&mut context, action_id(kobo_sdk::provider::ADDRESS));
            server.on_action(&mut context, action_id(kobo_sdk::provider::TEST));
            assert!(context.take_commands().is_empty());
            server.on_action(&mut context, action_id("server-retry"));
            assert!(
                matches!(context.take_commands().as_slice(), [Command::Store(StoreRequest::Load { key })] if key == KEY)
            );
        }
    }
}

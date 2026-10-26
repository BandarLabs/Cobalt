//! Owner-initiated exports, verified before being offered to a paired computer.
//!
//! Route `begin` from the owner's Export action, and forward `on_shelf` and
//! `on_save` callbacks. One export may be pending per app. `kobo export` reads
//! the acknowledged offer over the existing authenticated reader connection;
//! preparing an offer does not send content or change the owner's original.
use crate::imports::{Format as ImportFormat, Import};
use crate::{Context, Screen, ScreenBuilder, StoreResult};
use kobo_json::{ObjectBuilder, Value};

pub const OFFER_KEY: &str = "cobalt-export";
pub const MAX_OFFER_BYTES: usize = 4096;
pub const MAX_EXPORT_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Format {
    Text,
    Markdown,
    Png,
    Jpeg,
}
impl Format {
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Text => "txt",
            Self::Markdown => "md",
            Self::Png => "png",
            Self::Jpeg => "jpg",
        }
    }
    const fn import(self) -> ImportFormat {
        match self {
            Self::Text => ImportFormat::Text,
            Self::Markdown => ImportFormat::Markdown,
            Self::Png | Self::Jpeg => ImportFormat::Image,
        }
    }
}

/// Metadata only. The digest is the sole shelf path a receiver may read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Offer {
    pub title: String,
    pub format: Format,
    pub digest: String,
    pub bytes: usize,
}
fn schema() -> kobo_state::record::Schema {
    kobo_state::record::Schema::new("cobalt.export", 1, MAX_OFFER_BYTES)
        .expect("fixed export schema")
}
impl Offer {
    fn encode(&self) -> Result<Vec<u8>, String> {
        schema()
            .encode(
                &ObjectBuilder::new()
                    .set("title", self.title.as_str())
                    .set("format", self.format.extension())
                    .set("sha256", self.digest.as_str())
                    .set("bytes", self.bytes.to_string())
                    .build(),
            )
            .map_err(|error| error.to_string())
    }
    /// Read metadata without accepting any path or filename from the app.
    ///
    /// # Errors
    /// Refuses corrupt, newer, oversized or incomplete offers.
    pub fn restore(bytes: &[u8]) -> Result<Self, String> {
        let restored = schema()
            .restore(Some(bytes), |_, _| {
                Err(kobo_state::record::Error::MigrationUnavailable)
            })
            .map_err(|error| error.to_string())?
            .ok_or("Choose Export in the app first.")?;
        let field = |key| {
            restored
                .payload
                .get(key)
                .and_then(Value::as_str)
                .ok_or("This export offer is incomplete.")
        };
        let format = match field("format")? {
            "txt" => Format::Text,
            "md" => Format::Markdown,
            "png" => Format::Png,
            "jpg" => Format::Jpeg,
            _ => return Err("Update Cobalt to receive this file format.".into()),
        };
        let offer = Self {
            title: field("title")?.into(),
            format,
            digest: field("sha256")?.into(),
            bytes: field("bytes")?
                .parse()
                .map_err(|_| "The export size is invalid.")?,
        };
        if offer.title.is_empty()
            || offer.title.len() > 256
            || offer.title.chars().any(char::is_control)
            || offer.digest.len() != 64
            || !offer
                .digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || offer.bytes == 0
            || offer.bytes > MAX_EXPORT_BYTES
        {
            return Err("This export offer is invalid.".into());
        }
        Ok(offer)
    }
    /// A receiver must verify all bytes before publishing a completed file.
    #[must_use]
    pub fn matches(&self, bytes: &[u8]) -> bool {
        bytes.len() == self.bytes && kobo_net::sha256::hex_digest(bytes) == self.digest
    }
}

pub struct Export {
    copy: Import,
    offer: Offer,
    publishing: bool,
    ready: bool,
    problem: Option<String>,
}
impl std::fmt::Debug for Export {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Export")
            .field("ready", &self.ready)
            .field("publishing", &self.publishing)
            .finish_non_exhaustive()
    }
}
impl Export {
    /// Prepare app-validated content without writing or transferring anything.
    /// Text must be UTF-8; image bytes must already be validated by the app's
    /// image encoder/decoder. This checks their container signatures only.
    ///
    /// # Errors
    /// Refuses an invalid title, empty/oversized content or a format mismatch.
    pub fn new(title: &str, format: Format, bytes: Vec<u8>) -> Result<Self, String> {
        let valid = match format {
            Format::Text | Format::Markdown => std::str::from_utf8(&bytes).is_ok(),
            Format::Png => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
            Format::Jpeg => bytes.starts_with(&[0xff, 0xd8, 0xff]),
        };
        if !valid {
            return Err("This file does not match the selected export format.".into());
        }
        let copy = Import::new(title.trim(), format.import(), bytes)?;
        let receipt = copy.receipt();
        let offer = Offer {
            title: receipt.title.clone(),
            format,
            digest: receipt.digest.clone(),
            bytes: receipt.bytes,
        };
        Ok(Self {
            copy,
            offer,
            publishing: false,
            ready: false,
            problem: None,
        })
    }
    /// Call only after the owner chooses Export. Retry keeps the same bytes.
    pub fn begin(&mut self, context: &mut Context) -> bool {
        if self.publishing || self.ready {
            return false;
        }
        self.problem = None;
        if self.copy.is_available() {
            self.publish(context);
            true
        } else {
            self.copy.begin(context)
        }
    }
    fn publish(&mut self, context: &mut Context) {
        match self.offer.encode() {
            Ok(bytes) => {
                context.store().save(OFFER_KEY, bytes);
                self.publishing = true;
            }
            Err(error) => self.problem = Some(error),
        }
    }
    pub fn on_shelf(&mut self, context: &mut Context, name: &str, result: &StoreResult) -> bool {
        self.copy.on_shelf(context, name, result)
    }
    pub fn on_save(&mut self, context: &mut Context, key: &str, result: &StoreResult) -> bool {
        if key == OFFER_KEY && self.publishing {
            self.publishing = false;
            self.ready = matches!(result, StoreResult::Saved { key } if key == OFFER_KEY);
            if !self.ready {
                self.problem = Some(
                    "The copy was saved, but it is not ready for your computer. Try again.".into(),
                );
            }
            return true;
        }
        let handled = self.copy.on_save(key, result);
        if handled && self.copy.is_available() && !self.publishing && !self.ready {
            self.publish(context);
        }
        handled
    }
    #[must_use]
    pub const fn is_ready(&self) -> bool {
        self.ready
    }
    #[must_use]
    pub fn offer(&self) -> &Offer {
        &self.offer
    }
    #[must_use]
    pub fn screen(&self) -> Screen {
        let mut screen = ScreenBuilder::new("export")
            .top_bar("Save a copy")
            .owns_back(true)
            .heading(if self.offer.title.chars().count() > 64 {
                format!("{}…", self.offer.title.chars().take(63).collect::<String>())
            } else {
                self.offer.title.clone()
            })
            .secondary(format!(
                "{} · {}",
                self.offer.format.extension().to_uppercase(),
                crate::imports::display_size(self.offer.bytes)
            ));
        if self.ready {
            screen = screen.text("Ready for your computer.").secondary("Use Cobalt on your paired computer to receive this copy. The original stays on your reader.");
        } else if let Some(problem) = self.problem.as_deref().or_else(|| self.copy.failure()) {
            screen = screen
                .text(problem)
                .bottom_action("export-retry", "Try again");
        } else if self.publishing {
            screen = screen.text("Preparing the saved copy…");
        } else {
            screen = match self.copy.stage() {
                crate::imports::Stage::Preview => screen
                    .text("Keep a copy to receive on your computer.")
                    .bottom_action("export-confirm", "Save copy"),
                _ => screen.text("Saving and checking the copy…"),
            };
        }
        screen.build()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Command, StoreError, StoreRequest};

    #[test]
    fn only_an_acknowledged_verified_copy_is_offered_to_the_computer() {
        let root = std::env::temp_dir().join(format!("cobalt-sdk-export-{}", std::process::id()));
        fs_create(&root);
        let shelf = kobo_policy::shelf::Shelf::new(root.join("data"));
        let store = kobo_policy::store::Store::new(root.join("state"));
        let bytes = b"Garden notes\nWater the mint after sunset.\n".to_vec();
        let mut export = Export::new("Garden notes", Format::Text, bytes.clone()).unwrap();
        let mut context = Context::default();
        assert!(context.take_commands().is_empty());
        assert!(!export.is_ready());
        assert!(!format!("{export:?}").contains("Water the mint"));
        export.begin(&mut context);
        let mut refused = false;
        for _ in 0..20 {
            let commands = context.take_commands();
            if commands.is_empty() {
                break;
            }
            for command in commands {
                let Command::Store(request) = command else {
                    panic!("export performed non-storage work")
                };
                let result = if matches!(&request, StoreRequest::Save { key, .. } if key == OFFER_KEY)
                    && !refused
                {
                    refused = true;
                    StoreResult::Denied(StoreError::NoRoom)
                } else {
                    shelf
                        .handle(&request)
                        .unwrap_or_else(|| store.handle(&request))
                };
                match request {
                    StoreRequest::Save { key, .. } => {
                        export.on_save(&mut context, &key, &result);
                    }
                    StoreRequest::ShelfRead { name, .. }
                    | StoreRequest::ShelfWrite { name, .. } => {
                        export.on_shelf(&mut context, &name, &result);
                    }
                    _ => panic!("unexpected export request"),
                }
            }
        }
        assert!(refused);
        assert!(!export.is_ready());
        assert!(export.begin(&mut context));
        let commands = context.take_commands();
        let [Command::Store(request @ StoreRequest::Save { key, .. })] = commands.as_slice() else {
            panic!("offer retry copied content again")
        };
        assert_eq!(key, OFFER_KEY);
        export.on_save(&mut context, key, &store.handle(request));
        assert!(export.is_ready());
        let offer =
            Offer::restore(&std::fs::read(root.join("state").join(OFFER_KEY)).unwrap()).unwrap();
        assert!(offer.matches(&bytes));
        assert!(!offer.matches(b"partial"));
        assert_eq!(
            std::fs::read(root.join("data").join(&offer.digest)).unwrap(),
            bytes
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    fn fs_create(root: &std::path::Path) {
        let _ignored = std::fs::remove_dir_all(root);
        std::fs::create_dir_all(root).unwrap();
    }
    #[test]
    fn an_offer_cannot_select_an_arbitrary_reader_file() {
        let export = Export::new("Note", Format::Markdown, b"# Note\n".to_vec()).unwrap();
        let encoded = export.offer.encode().unwrap();
        assert_eq!(Offer::restore(&encoded).unwrap(), export.offer);
        let text = String::from_utf8(encoded).unwrap();
        assert!(Offer::restore(
            text.replace(&export.offer.digest, "../../secrets")
                .as_bytes()
        )
        .is_err());
        assert!(Offer::restore(text.replace("\"version\":1", "\"version\":2").as_bytes()).is_err());
        assert!(Export::new("Photo", Format::Png, b"not a photo".to_vec()).is_err());
        assert!(Export::new("Text", Format::Text, vec![0xff]).is_err());
    }
}

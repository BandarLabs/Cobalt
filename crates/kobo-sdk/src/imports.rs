//! Acknowledged, read-back-verified local imports. The app inspects the document
//! format before constructing an import; this module verifies transfer identity.

use crate::{
    Context, Failure, Screen, ScreenBuilder, ShelfDownload, ShelfProgress, ShelfUpload,
    StoreResult, MAX_SHELF_DOWNLOAD,
};
use kobo_json::{ObjectBuilder, Value};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Format {
    Text,
    Markdown,
    Html,
    Cbz,
    Image,
    Pdf,
}
impl Format {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Text => "Text",
            Self::Markdown => "Markdown",
            Self::Html => "HTML",
            Self::Cbz => "CBZ",
            Self::Image => "Image",
            Self::Pdf => "PDF",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Receipt {
    pub title: String,
    pub format: Format,
    pub bytes: usize,
    /// Both the shelf name and receipt key. Namespaces are separate; a full
    /// digest prevents a same-name import from overwriting different content.
    pub digest: String,
}
impl Receipt {
    fn schema() -> kobo_state::record::Schema {
        kobo_state::record::Schema::new("cobalt.import-receipt", 1, 4096)
            .expect("fixed receipt schema")
    }
    fn encode(&self) -> Result<Vec<u8>, String> {
        Self::schema()
            .encode(
                &ObjectBuilder::new()
                    .set("title", self.title.as_str())
                    .set("format", self.format.name())
                    .set("bytes", self.bytes.to_string())
                    .set("sha256", self.digest.as_str())
                    .build(),
            )
            .map_err(|error| error.to_string())
    }
    /// A receipt alone does not prove current availability. Call
    /// `Import::verify_existing` to re-check its shelf bytes after reopening.
    ///
    /// # Errors
    /// Preserves missing, corrupt and future-version distinctions.
    pub fn restore(bytes: Option<&[u8]>) -> Result<Option<Self>, String> {
        let Some(record) = Self::schema()
            .restore(bytes, |_, _| {
                Err(kobo_state::record::Error::MigrationUnavailable)
            })
            .map_err(|error| error.to_string())?
        else {
            return Ok(None);
        };
        let text = |key| {
            record
                .payload
                .get(key)
                .and_then(Value::as_str)
                .ok_or("The import receipt is incomplete.")
        };
        let title = text("title")?.to_owned();
        let format = match text("format")? {
            "Text" => Format::Text,
            "Markdown" => Format::Markdown,
            "HTML" => Format::Html,
            "CBZ" => Format::Cbz,
            "Image" => Format::Image,
            "PDF" => Format::Pdf,
            _ => return Err("Update the app to open this imported format.".into()),
        };
        let bytes = text("bytes")?
            .parse()
            .map_err(|_| "The imported size could not be read.")?;
        let digest = text("sha256")?.to_owned();
        if title.is_empty()
            || title.len() > 256
            || title.chars().any(char::is_control)
            || bytes == 0
            || bytes > MAX_SHELF_DOWNLOAD
            || digest.len() != 64
            || !digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("The import receipt is not valid.".into());
        }
        Ok(Some(Self {
            title,
            format,
            bytes,
            digest,
        }))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stage {
    Preview,
    Writing,
    Verifying,
    SavingReceipt,
    Ready,
    Cancelled,
}

#[derive(Debug)]
pub struct Import {
    receipt: Receipt,
    stage: Stage,
    upload: Option<ShelfUpload>,
    download: Option<ShelfDownload>,
    failure: Option<String>,
    retryable: bool,
    progress: u32,
    previously_saved: bool,
}
impl Import {
    /// Construct a preview from app-validated, owned content. No write occurs
    /// until `begin`; the original byte buffer moves into the upload.
    ///
    /// # Errors
    /// Refuses empty/oversized content and invalid display metadata.
    pub fn new(title: &str, format: Format, bytes: Vec<u8>) -> Result<Self, String> {
        if title.is_empty()
            || title.len() > 256
            || title.chars().any(char::is_control)
            || bytes.is_empty()
            || bytes.len() > MAX_SHELF_DOWNLOAD
        {
            return Err("Choose a supported file up to 32 MB with a readable title.".into());
        }
        let receipt = Receipt {
            title: title.into(),
            format,
            bytes: bytes.len(),
            digest: kobo_net::sha256::hex_digest(&bytes),
        };
        let upload = Some(ShelfUpload::new(&receipt.digest, bytes));
        Ok(Self {
            receipt,
            stage: Stage::Preview,
            upload,
            download: None,
            failure: None,
            retryable: true,
            progress: 0,
            previously_saved: false,
        })
    }
    #[must_use]
    pub fn receipt(&self) -> &Receipt {
        &self.receipt
    }
    #[must_use]
    pub const fn stage(&self) -> Stage {
        self.stage
    }
    #[must_use]
    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }
    #[must_use]
    pub fn is_available(&self) -> bool {
        self.stage == Stage::Ready && self.failure.is_none()
    }

    /// Reopening never trusts a receipt without checking the current bytes.
    #[must_use]
    pub fn verify_existing(context: &mut Context, receipt: Receipt) -> Self {
        let mut import = Self {
            receipt,
            stage: Stage::Verifying,
            upload: None,
            download: None,
            failure: None,
            retryable: true,
            progress: 0,
            previously_saved: true,
        };
        import.start_verification(context);
        import
    }
    /// Confirm the preview, or retry the failed stage. Active operations are
    /// not duplicated by another press. Retrying a receipt never uploads again.
    pub fn begin(&mut self, context: &mut Context) -> bool {
        if !self.retryable || (self.stage != Stage::Preview && self.failure.is_none()) {
            return false;
        }
        self.failure = None;
        self.progress = 0;
        match self.stage {
            Stage::Preview | Stage::Writing => {
                self.stage = Stage::Writing;
                if let Some(upload) = &mut self.upload {
                    upload.start(context);
                }
            }
            Stage::Verifying => self.start_verification(context),
            Stage::SavingReceipt => self.save_receipt(context),
            Stage::Ready | Stage::Cancelled => return false,
        }
        true
    }
    /// Stop issuing more chunks. A pending answer can still arrive and is
    /// ignored. Never delete a previously imported identical blob on cancel.
    pub fn cancel(&mut self) {
        self.stage = Stage::Cancelled;
        self.upload = None;
        self.download = None;
        self.failure = None;
    }
    fn start_verification(&mut self, context: &mut Context) {
        self.stage = Stage::Verifying;
        let mut download = ShelfDownload::new(&self.receipt.digest).at_most(self.receipt.bytes);
        download.start(context);
        self.download = Some(download);
    }
    fn save_receipt(&mut self, context: &mut Context) {
        self.stage = Stage::SavingReceipt;
        match self.receipt.encode() {
            Ok(bytes) => context.store().save(&self.receipt.digest, bytes),
            Err(error) => self.failure = Some(error),
        }
    }
    /// Route only the corresponding `KoboApp::on_shelf` answer here. The SDK
    /// supplies the requested name even for refusals without a name on wire.
    pub fn on_shelf(&mut self, context: &mut Context, name: &str, result: &StoreResult) -> bool {
        if name != self.receipt.digest || self.failure.is_some() {
            return false;
        }
        let progress = match self.stage {
            Stage::Writing => self
                .upload
                .as_mut()
                .map(|transfer| transfer.advance(context, result)),
            Stage::Verifying => self
                .download
                .as_mut()
                .map(|transfer| transfer.advance(context, result)),
            _ => None,
        };
        match progress {
            None | Some(ShelfProgress::Elsewhere) => return false,
            Some(ShelfProgress::Moving { done, .. }) => self.progress = done,
            Some(ShelfProgress::Failed(error)) => {
                self.failure = Some(if self.stage == Stage::Writing {
                    Failure::storing(error).advice.into()
                } else {
                    "The copied file could not be checked. Retry the check; if it still fails, import the original again.".into()
                });
            }
            Some(ShelfProgress::Done) if self.stage == Stage::Writing => {
                self.upload = None;
                self.progress = 0;
                self.start_verification(context);
            }
            Some(ShelfProgress::Done) => {
                let Some(download) = self.download.take() else {
                    self.failure = Some("The file check was interrupted. Retry the check.".into());
                    return true;
                };
                if download.bytes().len() != self.receipt.bytes
                    || kobo_net::sha256::hex_digest(download.bytes()) != self.receipt.digest
                {
                    self.retryable = false;
                    self.failure = Some(
                        "The copied file does not match the original. Import the original again."
                            .into(),
                    );
                } else if self.previously_saved {
                    self.stage = Stage::Ready;
                } else {
                    self.save_receipt(context);
                }
            }
        }
        true
    }
    /// Availability requires both a verified shelf and this successful save.
    pub fn on_save(&mut self, key: &str, result: &StoreResult) -> bool {
        if key != self.receipt.digest
            || self.stage != Stage::SavingReceipt
            || self.failure.is_some()
        {
            return false;
        }
        match result {
            StoreResult::Saved { key } if key == &self.receipt.digest => self.stage = Stage::Ready,
            StoreResult::Denied(_) => {
                self.failure = Some(
                    "The file was copied, but its library entry was not saved. Retry saving."
                        .into(),
                );
            }
            _ => return false,
        }
        true
    }
    #[must_use]
    pub fn screen(&self) -> Screen {
        let mut screen = ScreenBuilder::new("import")
            .top_bar("Add to library")
            .owns_back(true)
            .heading(&self.receipt.title)
            .secondary(format!(
                "{} · {}",
                self.receipt.format.name(),
                display_size(self.receipt.bytes)
            ));
        if let Some(failure) = &self.failure {
            return screen
                .text(failure)
                .bottom_action(
                    if self.retryable {
                        "import-retry"
                    } else {
                        "import-replace"
                    },
                    if self.retryable {
                        "Retry"
                    } else {
                        "Choose file"
                    },
                )
                .build();
        }
        screen = match self.stage {
            Stage::Preview => screen
                .text("Keep a copy on this reader for offline use.")
                .bottom_action("import-confirm", "Add to library"),
            Stage::Writing | Stage::Verifying => screen
                .transfer(
                    if self.stage == Stage::Writing {
                        "Copying file"
                    } else {
                        "Checking copied file"
                    },
                    u64::from(self.progress),
                    Some(self.receipt.bytes as u64),
                )
                .bottom_action("import-cancel", "Cancel"),
            Stage::SavingReceipt => screen.text("Saving the library entry…"),
            Stage::Ready => screen
                .text("Available on this reader.")
                .bottom_action("import-open", "Open"),
            Stage::Cancelled => screen.text("Import cancelled."),
        };
        screen.build()
    }
}

fn display_size(bytes: usize) -> String {
    if bytes >= 1_000_000 {
        format!("{}.{} MB", bytes / 1_000_000, bytes % 1_000_000 / 100_000)
    } else {
        format!("{} KB", bytes.div_ceil(1000))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Command, StoreError, StoreRequest};
    fn answer(
        import: &mut Import,
        context: &mut Context,
        request: &StoreRequest,
        result: &StoreResult,
    ) {
        match request {
            StoreRequest::Save { key, .. } => {
                import.on_save(key, result);
            }
            StoreRequest::ShelfWrite { name, .. } | StoreRequest::ShelfRead { name, .. } => {
                import.on_shelf(context, name, result);
            }
            _ => panic!("unexpected import request"),
        }
    }
    #[test]
    fn receipt_failure_retries_without_reupload_and_reopen_checks_actual_bytes() {
        let root =
            std::env::temp_dir().join(format!("cobalt-import-receipt-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let shelf = kobo_policy::shelf::Shelf::new(root.join("shelf"));
        let store = kobo_policy::store::Store::new(root.join("state"));
        let mut import = Import::new(
            "A walk home",
            Format::Text,
            b"Original fixture paragraph.".to_vec(),
        )
        .unwrap();
        let mut context = Context::default();
        assert!(!import.is_available());
        assert!(import.begin(&mut context));
        assert!(!import.begin(&mut context));
        let mut refused = false;
        for _ in 0..20 {
            for command in context.take_commands() {
                let Command::Store(request) = command else {
                    panic!("unexpected command")
                };
                let result = if matches!(request, StoreRequest::Save { .. }) && !refused {
                    refused = true;
                    StoreResult::Denied(StoreError::NoRoom)
                } else {
                    shelf
                        .handle(&request)
                        .unwrap_or_else(|| store.handle(&request))
                };
                answer(&mut import, &mut context, &request, &result);
            }
            if import.failure().is_some() {
                break;
            }
        }
        assert!(refused);
        assert!(!import.is_available());
        assert_eq!(import.stage(), Stage::SavingReceipt);
        assert!(import.begin(&mut context));
        let commands = context.take_commands();
        assert_eq!(commands.len(), 1);
        let Command::Store(request @ StoreRequest::Save { .. }) = &commands[0] else {
            panic!("retry must only save receipt")
        };
        answer(&mut import, &mut context, request, &store.handle(request));
        assert!(import.is_available());
        let restored = Receipt::restore(Some(&import.receipt.encode().unwrap()))
            .unwrap()
            .unwrap();
        let mut reopened = Import::verify_existing(&mut context, restored.clone());
        assert!(!reopened.is_available());
        for command in context.take_commands() {
            let Command::Store(request) = command else {
                unreachable!()
            };
            answer(
                &mut reopened,
                &mut context,
                &request,
                &shelf.handle(&request).unwrap(),
            );
        }
        assert!(reopened.is_available());
        assert!(matches!(
            shelf.handle(&StoreRequest::ShelfRemove {
                name: restored.digest.clone(),
            }),
            Some(StoreResult::ShelfRemoved { .. })
        ));
        let mut missing = Import::verify_existing(&mut context, restored);
        for command in context.take_commands() {
            let Command::Store(request) = command else {
                unreachable!()
            };
            answer(
                &mut missing,
                &mut context,
                &request,
                &shelf.handle(&request).unwrap(),
            );
        }
        assert!(!missing.is_available());
        assert!(missing.failure().is_some());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn corruption_and_cancel_never_make_an_item_available() {
        let mut context = Context::default();
        let mut import = Import::new("A walk home", Format::Text, b"one".to_vec()).unwrap();
        import.begin(&mut context);
        let name = import.receipt.digest.clone();
        import.on_shelf(
            &mut context,
            &name,
            &StoreResult::ShelfWritten {
                name: name.clone(),
                size: 3,
            },
        );
        import.on_shelf(
            &mut context,
            &name,
            &StoreResult::ShelfRead {
                name: name.clone(),
                offset: 0,
                bytes: b"two".to_vec(),
                size: 3,
            },
        );
        assert!(import.failure().unwrap().contains("does not match"));
        assert!(!import.begin(&mut context));
        assert!(!import.is_available());
        import.cancel();
        assert!(!import.on_save(&name, &StoreResult::Saved { key: name.clone() }));
        assert!(!import.is_available());
        assert_eq!(Receipt::restore(None), Ok(None));
        assert!(Receipt::restore(Some(b"broken")).is_err());
    }
    #[test]
    fn import_preview_progress_and_failure_fit_supported_screens() {
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
                let mut import =
                    Import::new("A walk home", Format::Cbz, b"original fixture".to_vec()).unwrap();
                for stage in [
                    Stage::Preview,
                    Stage::Writing,
                    Stage::Verifying,
                    Stage::SavingReceipt,
                    Stage::Ready,
                    Stage::Cancelled,
                ] {
                    import.stage = stage;
                    let screen = import.screen();
                    let chrome = kobo_ui::Chrome::for_screen(&screen, false, None);
                    let errors = screen
                        .diagnostics(&metrics, &chrome)
                        .issues
                        .into_iter()
                        .filter(|issue| issue.severity == kobo_ui::DiagnosticSeverity::Error)
                        .collect::<Vec<_>>();
                    assert!(
                        errors.is_empty(),
                        "{} {scale:?} {stage:?}: {errors:?}",
                        profile.id
                    );
                }
                import.failure = Some(
                    "The file was copied, but its library entry was not saved. Retry saving."
                        .into(),
                );
                let screen = import.screen();
                let chrome = kobo_ui::Chrome::for_screen(&screen, false, None);
                assert!(
                    screen
                        .diagnostics(&metrics, &chrome)
                        .issues
                        .iter()
                        .all(|issue| issue.severity != kobo_ui::DiagnosticSeverity::Error),
                    "{} {scale:?} failure",
                    profile.id
                );
            }
        }
    }
}

//! Local comic preview, verification and cancellation.

use super::{
    archive, transfer, Context, Failure, Format, Import, ImportStage, Kept, Panels, Route,
    ShelfDownload, ShelfProgress, StoreResult, SIDELOAD,
};

impl Panels {
    pub(super) fn import_help_screen(&self) -> kobo_sdk::Screen {
        let screen = kobo_sdk::ScreenBuilder::new("panels-import-help")
            .top_bar("Add comic")
            .owns_back(true)
            .page_position(u16::try_from(self.import_help_page + 1).unwrap_or(1), 4);
        match self.import_help_page {
            0 => screen.heading("Connect by USB")
                .text("Choose Connect on your reader. Open the reader drive on your computer.")
                .action_bar([("sample-comic", "Sample"), ("import-help-next", "Next")]).build(),
            1 => screen.heading("Open this folder")
                .text(".adds/cobalt/data/panels")
                .text("Show hidden folders. Create panels if needed.")
                .bottom_action("import-help-next", "Next").build(),
            2 => screen.heading("Copy your CBZ file")
                .text("Copy your comic into the folder. Name the copy volume.cbz.")
                .text("Use a CBZ file up to 32 MB.")
                .bottom_action("import-help-next", "Next").build(),
            _ => screen.heading("Return to Panels")
                .text("Eject the drive safely. Unplug the cable and reopen Panels. Choose Add comic to preview your file.")
                .bottom_action("load-sideload", "Check for comic").build(),
        }
    }

    pub(super) fn load_sample(&mut self) {
        if self.library.is_none()
            || self.local_load.is_some()
            || self.import.is_some()
            || self.pending_open.is_some()
            || self.shelf_load.is_some()
        {
            return;
        }
        self.route = Route::Import;
        self.notice = None;
        self.preview_comic(include_bytes!("../assets/a-small-garden.cbz").to_vec());
    }

    fn preview_comic(&mut self, bytes: Vec<u8>) {
        let preview = archive::inspect(&bytes)
            .map_err(|error| error.to_string())
            .and_then(|comic| {
                let thumbnail = super::previews::from_comic(&bytes, &comic);
                let import = Import::new(
                    comic.metadata.title.as_deref().unwrap_or("Added comic"),
                    Format::Cbz,
                    bytes,
                )?;
                let entry = Kept {
                    key: import.receipt().digest.clone(),
                    title: import.receipt().title.clone(),
                    pages: comic.pages.len(),
                    rtl: comic.metadata.right_to_left.unwrap_or(false),
                };
                Ok((import, entry, thumbnail))
            });
        match preview {
            Ok((import, entry, thumbnail)) => {
                self.previews.staged = thumbnail.map(|bytes| (entry.key.clone(), bytes));
                self.import = Some(import);
                self.import_entry = Some(entry);
            }
            Err(error) => {
                self.notice = Some(error);
                self.route = Route::Library;
            }
        }
    }

    pub(super) fn load_sideload(&mut self, context: &mut Context) {
        if self.library.is_none()
            || self.local_load.is_some()
            || self.import.is_some()
            || self.pending_open.is_some()
            || self.shelf_load.is_some()
        {
            return;
        }
        self.route = Route::Import;
        self.notice = None;
        let mut load = ShelfDownload::new(SIDELOAD).at_most(transfer::MAX_COMIC);
        load.start(context);
        self.local_load = Some(load);
    }

    pub(super) fn cancel_import(&mut self) {
        if let Some(import) = &mut self.import {
            let pending = import.failure().is_none()
                && matches!(
                    import.stage(),
                    ImportStage::CheckingExisting
                        | ImportStage::Writing
                        | ImportStage::Verifying
                        | ImportStage::SavingReceipt
                );
            import.cancel();
            if !pending {
                self.import = None;
            }
        }
        self.import_entry = None;
        self.previews.staged = None;
        // Drain an outstanding read before allowing another import of the same file.
        self.route = Route::Library;
    }

    pub(super) fn advance_local_load(&mut self, context: &mut Context, result: &StoreResult) {
        if self.route != Route::Import {
            self.local_load = None;
            return;
        }
        let Some(load) = &mut self.local_load else {
            return;
        };
        match load.advance(context, result) {
            ShelfProgress::Done => {
                let bytes = self.local_load.take().expect("active local load").take();
                self.preview_comic(bytes);
            }
            ShelfProgress::Failed(error) => {
                self.local_load = None;
                if error == kobo_sdk::StoreError::Missing {
                    self.import_help_page = 0;
                    self.route = Route::ImportHelp;
                } else {
                    self.route = Route::Library;
                    self.notice = Some(Failure::storing(error).advice.into());
                }
            }
            ShelfProgress::Moving { .. } | ShelfProgress::Elsewhere => {}
        }
    }
}

//! Local comic preview, verification and cancellation.

use super::{
    archive, transfer, Context, Failure, Format, Import, ImportStage, Kept, Panels, Route,
    ShelfDownload, ShelfProgress, StoreResult, SIDELOAD,
};

impl Panels {
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
                let preview = archive::inspect(&bytes)
                    .map_err(|error| error.to_string())
                    .and_then(|comic| {
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
                        Ok((import, entry))
                    });
                match preview {
                    Ok((import, entry)) => {
                        self.import = Some(import);
                        self.import_entry = Some(entry);
                    }
                    Err(error) => {
                        self.notice = Some(error);
                        self.route = Route::Library;
                    }
                }
            }
            ShelfProgress::Failed(error) => {
                self.local_load = None;
                self.route = Route::Library;
                self.notice = Some(if error == kobo_sdk::StoreError::Missing {
                    "Send a CBZ file to Panels from your computer, then choose Add comic again."
                        .into()
                } else {
                    Failure::storing(error).advice.into()
                });
            }
            ShelfProgress::Moving { .. } | ShelfProgress::Elsewhere => {}
        }
    }
}

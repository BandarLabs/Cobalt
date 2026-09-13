//! Serialized download checkpoints, recovery and removal.
use super::{
    archive, komga, shelf_key,
    transfer::{self, Checkpoint},
    Awaiting, Context, Credential, Format, Import, Kept, Panels, Pending, Route, Saving,
    ShelfDownload, ShelfProgress, ShelfUpload, StoreResult, Task, PARTIAL_BLOB, PARTIAL_META,
};

#[derive(Clone)]
pub(super) enum Metadata {
    Save(Checkpoint),
    Clear,
}

#[derive(Default)]
pub(super) struct Recovery {
    pub checkpoint: Option<Checkpoint>,
    pub metadata_write: Option<Metadata>,
    pub metadata_retry: Option<Metadata>,
    pub metadata_loading: bool,
    pub loaded: bool,
    pub save_retry: Option<Saving>,
    pub discard_requested: bool,
    pub cleanup: std::collections::BTreeMap<String, bool>,
}

impl Panels {
    pub(super) fn recovery_busy(&self) -> bool {
        self.recovery.metadata_write.is_some()
            || self.recovery.metadata_retry.is_some()
            || self.upload.is_some()
            || self.recovery.save_retry.is_some()
            || self.partial_load.is_some()
            || !self.recovery.cleanup.is_empty()
            || self.recovery.discard_requested
    }
    fn write_metadata(&mut self, context: &mut Context, operation: Metadata) {
        if self.recovery.metadata_write.is_some() {
            return;
        }
        let bytes = match &operation {
            Metadata::Save(checkpoint) => {
                let Ok(bytes) = checkpoint.encode() else {
                    self.paused = true;
                    self.notice =
                        Some("This download could not be saved. Remove it and try again.".into());
                    self.recovery.metadata_retry = Some(operation);
                    return;
                };
                bytes
            }
            Metadata::Clear => Vec::new(),
        };
        context.store().save(PARTIAL_META, bytes);
        self.recovery.metadata_write = Some(operation);
    }
    pub(super) fn saved_metadata(&mut self, context: &mut Context, result: &StoreResult) {
        let Some(operation) = self.recovery.metadata_write.take() else {
            return;
        };
        if !matches!(result, StoreResult::Saved { key } if key == PARTIAL_META) {
            self.recovery.metadata_retry = Some(operation);
            self.paused = true;
            self.notice =
                Some("Download progress was not saved. Free some space, then retry.".into());
            return;
        }
        match operation {
            Metadata::Save(checkpoint) => {
                let complete = checkpoint.complete;
                self.recovery.checkpoint = Some(checkpoint);
                if self.recovery.discard_requested {
                    self.drain_removal(context);
                } else if complete && !self.paused {
                    self.finish_download(context);
                } else {
                    self.fetch_next_chunk(context);
                }
            }
            Metadata::Clear => {
                self.recovery.checkpoint = None;
                self.pending = None;
                self.transfer = None;
                self.recovery.save_retry = None;
                self.recovery.metadata_retry = None;
                self.recovery.loaded = true;
                self.completed_cleanup = None;
                for name in transfer::BLOBS.into_iter().chain([PARTIAL_BLOB]) {
                    self.recovery.cleanup.insert(name.into(), true);
                    context.shelf().remove(name);
                }
            }
        }
    }
    pub(super) fn removed_blob(&mut self, name: &str, result: &StoreResult) -> bool {
        let Some(waiting) = self.recovery.cleanup.get_mut(name) else {
            return false;
        };
        if !*waiting {
            return true;
        }
        if matches!(result, StoreResult::ShelfRemoved { name: removed } if removed == name)
            || matches!(result, StoreResult::Denied(kobo_sdk::StoreError::Missing))
        {
            self.recovery.cleanup.remove(name);
        } else {
            *waiting = false;
            self.notice = Some(
                "Some download files could not be removed. Retry when storage is available.".into(),
            );
        }
        if self.recovery.cleanup.is_empty() {
            self.recovery.discard_requested = false;
            if self.route == Route::Download {
                self.route = Route::Library;
            }
            self.notice = None;
        }
        true
    }
    pub(super) fn restore_download(&mut self, context: &mut Context, value: Option<&[u8]>) {
        self.recovery.metadata_loading = false;
        self.recovery.loaded = true;
        let Some(bytes) = value.filter(|bytes| !bytes.is_empty()) else {
            if self.route == Route::Download {
                self.route = Route::Library;
                self.notice = None;
            }
            return;
        };
        self.route = Route::Download;
        self.paused = true;
        if let Ok(checkpoint) = Checkpoint::restore(bytes) {
            self.pending = Some(checkpoint.pending.clone());
            self.recovery.checkpoint = Some(checkpoint);
            self.load_checkpoint(context);
        } else {
            // Old, corrupt and future records remain until explicit removal.
            self.recovery.loaded = false;
            self.notice = Some("This paused download could not be opened. Retry, or remove it and download the comic again.".into());
        }
    }

    fn load_checkpoint(&mut self, context: &mut Context) {
        if self.partial_load.is_some() {
            return;
        }
        let Some(checkpoint) = &self.recovery.checkpoint else {
            return;
        };
        if checkpoint.bytes == 0 {
            if !checkpoint.matches(&[]) {
                self.notice = Some(
                    "This paused download could not be verified. Remove it and try again.".into(),
                );
                return;
            }
            self.transfer = Some(transfer::Download::new(
                checkpoint.pending.url.clone(),
                vec![],
            ));
            self.notice = Some("Download paused. Continue when your server is available.".into());
            return;
        }
        let mut load =
            ShelfDownload::new(transfer::BLOBS[checkpoint.slot]).at_most(transfer::MAX_COMIC);
        load.start(context);
        self.partial_load = Some(load);
        self.notice = Some("Checking the saved download.".into());
    }
    pub(super) fn begin_download(&mut self, context: &mut Context) {
        if !self.recovery.loaded || self.pending.is_some() || self.recovery_busy() {
            self.route = Route::Download;
            self.notice = Some("Continue or remove your previous download first.".into());
            return;
        }
        if self.library.is_none() || self.completed_cleanup.is_some() || self.import.is_some() {
            self.notice = Some("Finish adding your comic before starting another download.".into());
            return;
        }
        let Some(publication) = self.selected.as_ref() else {
            return;
        };
        let Some(url) = komga::cbz(publication) else {
            self.notice = Some("This volume has no downloadable comic file.".into());
            return;
        };
        let pending = Pending {
            key: shelf_key(publication.identifier.as_deref().unwrap_or(&url)),
            title: publication.title.clone(),
            url,
        };
        let checkpoint = Checkpoint::new(pending.clone(), 0, &[], false);
        if checkpoint.encode().is_err() {
            self.notice = Some("This comic's download details could not be read.".into());
            return;
        }
        if let Some((task, _)) = self.task.take() {
            context.cancel(task);
        }
        self.transfer = Some(transfer::Download::new(pending.url.clone(), vec![]));
        self.pending = Some(pending);
        self.paused = false;
        self.notice = None;
        self.route = Route::Download;
        self.write_metadata(context, Metadata::Save(checkpoint));
    }
    pub(super) fn fetch_next_chunk(&mut self, context: &mut Context) {
        if self.paused || self.task.is_some() || self.recovery_busy() {
            return;
        }
        let Some(download) = &self.transfer else {
            return;
        };
        if download.complete {
            self.finish_download(context);
            return;
        }
        self.task = context
            .spawn(Task::Fetch {
                url: download.url.clone(),
                offset: download.offset(),
                max_bytes: u32::try_from(transfer::CHUNK).expect("bounded chunk"),
                credential: Some(Credential::basic("komga")),
                headers: vec![],
            })
            .map(|task| (task, Awaiting::Comic));
        if self.task.is_none() {
            self.paused = true;
            self.notice = Some("The download could not start. Try again.".into());
        }
    }
    pub(super) fn save_transfer(&mut self, context: &mut Context, complete: bool) {
        if self.upload.is_some() || self.recovery.metadata_write.is_some() {
            return;
        }
        let Some(download) = &self.transfer else {
            return;
        };
        let slot = self
            .recovery
            .checkpoint
            .as_ref()
            .map_or(0, |checkpoint| 1 - checkpoint.slot);
        let mut upload = ShelfUpload::new(transfer::BLOBS[slot], download.received.clone());
        upload.start(context);
        self.recovery.save_retry = None;
        self.upload = Some((
            upload,
            if complete {
                Saving::Complete
            } else {
                Saving::Partial
            },
        ));
    }
    pub(super) fn advance_upload(&mut self, context: &mut Context, result: &StoreResult) {
        if self.recovery.discard_requested {
            // One shelf write is outstanding. Consume its answer without issuing another piece.
            self.upload = None;
            self.drain_removal(context);
            return;
        }
        let Some((upload, saving)) = &mut self.upload else {
            return;
        };
        match upload.advance(context, result) {
            ShelfProgress::Done => {
                let complete = *saving == Saving::Complete;
                let slot = transfer::BLOBS
                    .iter()
                    .position(|name| *name == upload.name())
                    .expect("checkpoint slot");
                self.upload = None;
                if let (Some(pending), Some(download)) = (&self.pending, &self.transfer) {
                    self.write_metadata(
                        context,
                        Metadata::Save(Checkpoint::new(
                            pending.clone(),
                            slot,
                            &download.received,
                            complete,
                        )),
                    );
                }
            }
            ShelfProgress::Failed(_) => {
                self.recovery.save_retry = Some(*saving);
                self.upload = None;
                self.paused = true;
                self.notice =
                    Some("The download could not be saved. Free some space, then retry.".into());
            }
            ShelfProgress::Moving { .. } | ShelfProgress::Elsewhere => {}
        }
    }
    pub(super) fn cancel_download(&mut self, context: &mut Context, remove: bool) {
        if let Some((task, _)) = self.task.take() {
            context.cancel(task);
        }
        self.paused = true;
        if let Some(download) = &mut self.transfer {
            download.recheck();
        }
        if remove {
            self.recovery.discard_requested = true;
            self.notice = Some("Removing download.".into());
            self.drain_removal(context);
        }
    }
    pub(super) fn drain_removal(&mut self, context: &mut Context) {
        if self.upload.is_some()
            || self.partial_load.is_some()
            || self.recovery.metadata_write.is_some()
            || self.recovery.metadata_loading
            || !self.recovery.cleanup.is_empty()
        {
            return;
        }
        self.recovery.metadata_retry = None;
        self.recovery.save_retry = None;
        self.write_metadata(context, Metadata::Clear);
    }
    pub(super) fn retry_download(&mut self, context: &mut Context) {
        if self.upload.is_some()
            || self.partial_load.is_some()
            || self.recovery.metadata_write.is_some()
            || self.recovery.metadata_loading
        {
            return;
        }
        self.notice = None;
        if !self.recovery.cleanup.is_empty() {
            for (name, waiting) in &mut self.recovery.cleanup {
                if !*waiting {
                    *waiting = true;
                    context.shelf().remove(name.clone());
                }
            }
            return;
        }
        if self.recovery.discard_requested {
            self.drain_removal(context);
            return;
        }
        self.paused = false;
        if let Some(operation) = self.recovery.metadata_retry.take() {
            self.write_metadata(context, operation);
        } else if let Some(saving) = self.recovery.save_retry.take() {
            self.save_transfer(context, saving == Saving::Complete);
        } else if let Some(download) = &mut self.transfer {
            download.recheck();
            self.fetch_next_chunk(context);
        } else if self.recovery.checkpoint.is_some() {
            self.load_checkpoint(context);
        } else {
            self.recovery.metadata_loading = true;
            context.store().load(PARTIAL_META);
        }
    }
    pub(super) fn finish_download(&mut self, context: &mut Context) {
        if self.import.is_some() {
            return;
        }
        let (Some(pending), Some(download)) = (&self.pending, &self.transfer) else {
            return;
        };
        let result = archive::inspect(&download.received)
            .map_err(|error| error.to_string())
            .and_then(|comic| {
                let thumbnail = super::previews::from_comic(&download.received, &comic);
                let import = Import::new(&pending.title, Format::Cbz, download.received.clone())?;
                let kept = Kept {
                    key: import.receipt().digest.clone(),
                    title: import.receipt().title.clone(),
                    pages: comic.pages.len(),
                    rtl: comic.metadata.right_to_left.unwrap_or(self.rtl),
                };
                Ok((import, kept, thumbnail))
            });
        match result {
            Ok((mut import, kept, thumbnail)) => {
                self.previews.staged = thumbnail.map(|bytes| (kept.key.clone(), bytes));
                self.completed_cleanup = Some(kept.key.clone());
                self.import_entry = Some(kept);
                // Download was the owner's confirmation. Reuse the verified copy and receipt flow.
                import.begin(context);
                self.import = Some(import);
                self.transfer = None;
                self.route = Route::Import;
            }
            Err(error) => {
                self.paused = true;
                self.notice = Some(error);
            }
        }
    }
    pub(super) fn advance_partial_load(&mut self, context: &mut Context, result: &StoreResult) {
        if self.recovery.discard_requested {
            self.partial_load = None;
            self.drain_removal(context);
            return;
        }
        let Some(load) = &mut self.partial_load else {
            return;
        };
        match load.advance(context, result) {
            ShelfProgress::Done => {
                let bytes = self.partial_load.take().expect("active read").take();
                if let Some(checkpoint) = &self.recovery.checkpoint {
                    if checkpoint.matches(&bytes) {
                        let mut download =
                            transfer::Download::new(checkpoint.pending.url.clone(), bytes);
                        download.complete = checkpoint.complete;
                        self.transfer = Some(download);
                        self.notice = Some(
                            if checkpoint.complete {
                                "Download complete. Add it to your shelf, even while offline."
                            } else {
                                "Download paused. Continue when your server is available."
                            }
                            .into(),
                        );
                    } else {
                        self.notice = Some("This saved download could not be verified. Remove it and download the comic again.".into());
                    }
                }
                self.paused = true;
            }
            ShelfProgress::Failed(_) => {
                self.partial_load = None;
                self.paused = true;
                self.notice = Some("The saved download could not be read. Retry when storage is available, or remove it.".into());
            }
            ShelfProgress::Moving { .. } | ShelfProgress::Elsewhere => {}
        }
    }
}

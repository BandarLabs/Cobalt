//! Real SDK callback routing and filesystem policy, with original network fixture bytes.
use super::*;
use kobo_sdk::{AppRunner, Command, StoreError, StoreRequest};
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);

struct Harness {
    root: PathBuf,
    runner: AppRunner<Panels>,
    queue: VecDeque<Command>,
    fixture: Vec<u8>,
    offsets: Vec<u32>,
    fail: Option<String>,
}
impl Harness {
    fn new(fixture: Vec<u8>) -> Self {
        let root = std::env::temp_dir().join(format!(
            "panels-recovery-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        let mut harness = Self {
            root,
            runner: AppRunner::new(Panels::default()),
            queue: VecDeque::new(),
            fixture,
            offsets: vec![],
            fail: None,
        };
        harness.restart();
        harness.pump();
        harness
    }
    fn restart(&mut self) {
        self.runner = AppRunner::new(Panels::default());
        self.queue = self.runner.start().into();
    }
    fn action(&mut self, name: &str) {
        self.queue.extend(self.runner.action(action_id(name)));
    }
    fn begin(&mut self) {
        let feed = komga::parse(br#"<feed xmlns="http://www.w3.org/2005/Atom"><title>Comics</title><entry><title>Garden</title><id>garden</id><link rel="http://opds-spec.org/acquisition/open-access" href="garden.cbz" type="application/x-cbz"/></entry></feed>"#, "https://example.com/library").unwrap();
        self.runner.app_mut().selected = Some(feed.publications[0].clone());
        self.action("download");
    }
    fn stored(&self) -> Vec<u8> {
        match kobo_policy::store::Store::new(self.root.join("state")).handle(&StoreRequest::Load {
            key: PARTIAL_META.into(),
        }) {
            StoreResult::Loaded { value, .. } => value.unwrap_or_default(),
            result => panic!("record read: {result:?}"),
        }
    }
    fn checkpoint(&self) -> transfer::Checkpoint {
        transfer::Checkpoint::restore(&self.stored()).unwrap()
    }
    fn tick(&mut self) -> bool {
        let Some(command) = self.queue.pop_front() else {
            return false;
        };
        let replies = match command {
            Command::Store(request) => {
                let key = match &request {
                    StoreRequest::Save { key, .. } => Some(key.as_str()),
                    StoreRequest::ShelfWrite { name, .. } => Some(name.as_str()),
                    _ => None,
                };
                let fault = if key.is_some() && key == self.fail.as_deref() {
                    self.fail = None;
                    Some(kobo_policy::WriteFault::NoRoom)
                } else {
                    None
                };
                let result = kobo_policy::shelf::Shelf::new(&self.root)
                    .handle_with_write_fault(&request, fault)
                    .unwrap_or_else(|| {
                        kobo_policy::store::Store::new(self.root.join("state"))
                            .handle_with_write_fault(&request, fault)
                    });
                self.runner.store_result(result)
            }
            Command::Spawn {
                task,
                work:
                    Task::Fetch {
                        offset,
                        max_bytes,
                        credential,
                        ..
                    },
            } => {
                assert_eq!(credential, Some(Credential::basic("komga")));
                self.offsets.push(offset);
                let from = (offset as usize).min(self.fixture.len());
                let to = (from + max_bytes as usize).min(self.fixture.len());
                self.runner.task_outcome(
                    task,
                    TaskOutcome::Completed(self.fixture[from..to].to_vec()),
                )
            }
            Command::Cancel(task) => self.runner.task_outcome(task, TaskOutcome::Cancelled),
            _ => vec![],
        };
        self.queue.extend(replies);
        true
    }
    fn pump(&mut self) {
        for _ in 0..2000 {
            if !self.tick() {
                return;
            }
        }
        panic!("callback loop did not settle");
    }
    fn until(&mut self, check: impl Fn(&Command) -> bool) {
        for _ in 0..2000 {
            if self.queue.front().is_some_and(&check) {
                return;
            }
            assert!(self.tick(), "expected callback was never queued");
        }
        panic!("callback not reached");
    }
}
impl Drop for Harness {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}
fn metadata(command: &Command) -> bool {
    matches!(command, Command::Store(StoreRequest::Save { key, .. }) if key == PARTIAL_META)
}
fn network(command: &Command) -> bool {
    matches!(command, Command::Spawn { .. })
}

#[test]
fn initial_metadata_failure_prevents_fetch_and_retry_releases_it() {
    let mut h = Harness::new(vec![0; transfer::CHUNK]);
    h.fail = Some(PARTIAL_META.into());
    h.begin();
    h.pump();
    assert!(h.offsets.is_empty());
    assert!(h.runner.app_mut().recovery.metadata_retry.is_some());
    assert!(!h.runner.app_mut().can_suspend());
    h.action("retry");
    h.until(network);
    assert_eq!(h.checkpoint().bytes, 0);
}

#[test]
fn crash_between_blob_and_metadata_keeps_previous_checkpoint_and_rechecks_server() {
    let mut h = Harness::new(vec![3; transfer::CHUNK * 3]);
    h.begin();
    h.until(metadata);
    h.tick(); // initial metadata
    h.until(metadata);
    h.tick(); // first chunk committed
    assert_eq!(h.checkpoint().bytes, transfer::CHUNK);
    let previous = h.stored();
    h.until(metadata); // second blob complete; manifest has not been written
    assert_eq!(h.stored(), previous);
    h.restart();
    h.pump();
    assert_eq!(
        h.runner.app_mut().transfer.as_ref().unwrap().received.len(),
        transfer::CHUNK
    );
    assert_eq!(h.runner.app_mut().transfer.as_ref().unwrap().offset(), 0);
    h.fixture[0] = 9;
    h.action("retry");
    h.pump();
    assert!(h
        .runner
        .app_mut()
        .notice
        .as_deref()
        .unwrap()
        .contains("changed"));
    assert_eq!(h.stored(), previous);
    assert_eq!(h.offsets.last(), Some(&0));
}

#[test]
fn failed_chunk_save_retries_storage_before_fetching_more_and_cancel_drains_writes() {
    let mut h = Harness::new(vec![3; transfer::CHUNK * 2]);
    h.fail = Some(transfer::BLOBS[1].into());
    h.begin();
    h.pump();
    assert_eq!(h.offsets, [0]);
    assert!(h.runner.app_mut().recovery.save_retry.is_some());
    h.action("retry");
    h.until(|command| matches!(command, Command::Store(StoreRequest::ShelfWrite { .. })));
    h.action("cancel-download");
    assert!(h.runner.app_mut().upload.is_some());
    h.action("download");
    h.pump();
    assert_eq!(h.offsets, [0]);
    assert!(h.stored().is_empty());
    assert!(h.runner.app_mut().pending.is_none());
    assert!(!h.runner.app_mut().recovery_busy());
    for name in transfer::BLOBS {
        assert!(!h.root.join(name).exists());
    }
}

#[test]
fn complete_checkpoint_imports_offline_and_survives_receipt_and_library_save_failures() {
    let bytes = include_bytes!("../assets/a-small-garden.cbz").to_vec();
    let digest = kobo_net::sha256::hex_digest(&bytes);
    let mut h = Harness::new(bytes.clone());
    h.begin();
    h.until(metadata);
    h.tick();
    h.until(metadata);
    h.tick(); // complete manifest acknowledged, import read queued
    assert!(h.checkpoint().complete);
    h.restart();
    h.pump();
    let network_count = h.offsets.len();
    h.fail = Some(digest.clone()); // fail final copy, then retry
    h.action("retry");
    h.pump();
    assert!(h
        .runner
        .app_mut()
        .import
        .as_ref()
        .unwrap()
        .failure()
        .is_some());
    assert!(h.checkpoint().complete);
    h.action("import-retry");
    h.until(|command| matches!(command, Command::Store(StoreRequest::Save { key, .. }) if key == &digest));
    h.fail = Some(digest.clone());
    h.pump(); // fail receipt, preserving verified bytes
    assert!(
        std::fs::read(h.root.join(&digest)).unwrap() == bytes,
        "verified comic bytes must survive"
    );
    h.fail = Some(LIBRARY.into());
    h.action("import-retry");
    h.pump();
    assert!(!h.stored().is_empty());
    assert!(matches!(
        h.runner.app_mut().library.as_ref().unwrap().status(),
        DraftStatus::Failed(_)
    ));
    assert!(!h.runner.app_mut().can_suspend());
    h.action("retry-library");
    h.pump();
    assert_eq!(
        h.offsets.len(),
        network_count,
        "offline completion must not request the server"
    );
    assert!(h.stored().is_empty());
    assert_eq!(h.runner.app_mut().library_entries()[0].key, digest);
    assert!(h.runner.app_mut().import.as_ref().unwrap().is_available());
    assert!(
        std::fs::read(h.root.join(&digest)).unwrap() == bytes,
        "verified comic bytes must survive"
    );
}

#[test]
fn corrupt_and_legacy_recovery_are_preserved_until_explicit_removal() {
    let mut h = Harness::new(vec![]);
    for bytes in [
        b"fixture.cbz\tGarden\thttps://example.com/comic.cbz".as_slice(),
        b"{}",
    ] {
        let store = kobo_policy::store::Store::new(h.root.join("state"));
        assert!(matches!(
            store.handle(&StoreRequest::Save {
                key: PARTIAL_META.into(),
                value: bytes.to_vec()
            }),
            StoreResult::Saved { .. }
        ));
        h.restart();
        h.pump();
        assert_eq!(h.stored(), bytes);
        h.begin();
        h.pump();
        assert!(h.offsets.is_empty());
        assert_eq!(h.stored(), bytes);
        h.action("cancel-download");
        h.pump();
        assert!(h.stored().is_empty());
    }
}

#[test]
fn damaged_blob_does_not_become_an_empty_download_or_modify_its_checkpoint() {
    let mut h = Harness::new(vec![3; transfer::CHUNK * 2]);
    h.begin();
    h.until(metadata);
    h.tick();
    h.until(metadata);
    h.tick();
    let checkpoint = h.checkpoint();
    let saved = h.stored();
    std::fs::write(h.root.join(transfer::BLOBS[checkpoint.slot]), b"damaged").unwrap();
    h.restart();
    h.pump();
    assert!(h.runner.app_mut().transfer.is_none());
    assert_eq!(h.stored(), saved);
    assert!(h
        .runner
        .app_mut()
        .notice
        .as_deref()
        .unwrap()
        .contains("verified"));
    h.action("retry");
    h.pump();
    assert_eq!(h.stored(), saved);
    assert!(h.runner.app_mut().transfer.is_none());
}

#[test]
fn failed_clear_and_failed_remove_remain_retryable_without_touching_saved_comics() {
    let mut h = Harness::new(vec![0; transfer::CHUNK]);
    h.begin();
    h.until(metadata);
    h.tick();
    h.until(network);
    h.action("cancel-download");
    h.fail = Some(PARTIAL_META.into());
    h.pump();
    assert!(h.runner.app_mut().recovery.discard_requested);
    assert!(!h.stored().is_empty());
    h.action("retry");
    h.until(|command| matches!(command, Command::Store(StoreRequest::ShelfRemove { .. })));
    let Command::Store(StoreRequest::ShelfRemove { name }) = h.queue.pop_front().unwrap() else {
        panic!()
    };
    h.queue.extend(
        h.runner
            .store_result(StoreResult::Denied(StoreError::Unwritable)),
    );
    h.pump();
    assert_eq!(h.runner.app_mut().recovery.cleanup.get(&name), Some(&false));
    h.action("retry");
    h.pump();
    assert!(!h.runner.app_mut().recovery_busy());
}

#[test]
fn resumed_matching_prefix_advances_only_after_a_checkpoint_ack_and_suspend_keeps_it() {
    let mut h = Harness::new(vec![3; transfer::CHUNK * 3]);
    h.begin();
    h.until(metadata);
    h.tick();
    h.until(metadata);
    h.tick();
    let previous = h.stored();
    h.restart();
    h.pump();
    let count = h.offsets.len();
    h.action("retry");
    h.until(metadata); // recheck did not rewrite the old prefix; next chunk is now ready
    assert_eq!(
        &h.offsets[count..],
        &[0, u32::try_from(transfer::CHUNK).unwrap()]
    );
    assert_eq!(h.stored(), previous);
    h.fail = Some(PARTIAL_META.into());
    h.tick();
    h.pump();
    assert_eq!(h.stored(), previous);
    assert!(h.runner.app_mut().recovery.metadata_retry.is_some());
    h.action("retry");
    h.queue.extend(h.runner.suspend());
    h.pump();
    assert_eq!(h.checkpoint().bytes, transfer::CHUNK * 2);
    assert!(h.runner.app_mut().paused);
    assert!(h.runner.app_mut().task.is_none());
    h.restart();
    h.pump();
    assert_eq!(
        h.runner.app_mut().transfer.as_ref().unwrap().received.len(),
        transfer::CHUNK * 2
    );
}

#[test]
fn downloading_changed_content_from_the_same_url_keeps_both_verified_comics() {
    let first = include_bytes!("../assets/a-small-garden.cbz").to_vec();
    let second = include_bytes!("../../../docs/quality/fixtures/original-pages.cbz").to_vec();
    let mut h = Harness::new(first.clone());
    h.begin();
    h.pump();
    h.queue.extend(h.runner.action(ActionId::BACK));
    h.pump();
    h.fixture = second.clone();
    h.begin();
    h.pump();
    assert_eq!(h.runner.app_mut().library_entries().len(), 2);
    h.queue.extend(h.runner.action(ActionId::BACK));
    h.pump();
    h.begin(); // Identical copy and unchanged index: no extra library save is needed.
    h.pump();
    assert_eq!(h.runner.app_mut().library_entries().len(), 2);
    assert!(
        h.stored().is_empty(),
        "reused copies must also finish recovery cleanup"
    );
    assert!(!h.runner.app_mut().recovery_busy());
    for bytes in [first, second] {
        let key = kobo_net::sha256::hex_digest(&bytes);
        assert!(std::fs::read(h.root.join(key)).unwrap() == bytes);
    }
}

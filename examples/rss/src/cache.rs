//! Two-slot feed snapshots. The published slot is never overwritten while saving.
use kobo_net::sha256::hex_digest;
use kobo_sdk::{Context, ShelfDownload, ShelfProgress, ShelfUpload, StoreError, StoreResult};

pub const LIMIT: usize = 512 * 1024;

#[derive(Default)]
enum Phase {
    #[default]
    Loading,
    Reading(ShelfDownload),
    Ready,
    Writing(ShelfUpload),
    Committing,
    Failed,
}

pub struct Cache {
    pub key: String,
    stem: String,
    slot: u8,
    digest: String,
    phase: Phase,
    pub bytes: Option<Vec<u8>>,
    pending: Option<Vec<u8>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Event {
    Loaded,
    Saved,
    Failed,
}

impl Cache {
    pub fn new(url: &str) -> Self {
        // SHA-256 identifies the URL. Filenames use its first 240 bits so
        // both slots fit the shelf name limit. Reads verify content digests.
        let stem = hex_digest(url.as_bytes());
        Self {
            key: stem.clone(),
            stem,
            slot: 0,
            digest: String::new(),
            phase: Phase::Loading,
            bytes: None,
            pending: None,
        }
    }

    pub fn start(&self, context: &mut Context) {
        context.store().load(&self.key);
    }

    fn file(&self) -> String {
        // URL digest occupies the filename; slot zero/one use distinct suffixes.
        format!("{}.{}", &self.stem[..60], self.slot)
    }

    pub fn owns_file(&self, name: &str) -> bool {
        self.file() == name
    }

    pub fn busy(&self) -> bool {
        matches!(
            self.phase,
            Phase::Loading | Phase::Reading(_) | Phase::Writing(_) | Phase::Committing
        )
    }

    pub fn save(&mut self, context: &mut Context, bytes: Vec<u8>) -> bool {
        if bytes.len() > LIMIT {
            return false;
        }
        if matches!(self.phase, Phase::Failed) {
            // Keep the latest refresh, but do not write until retry has checked
            // which slot the previous acknowledgement may have published.
            self.pending = Some(bytes);
            return false;
        }
        if !matches!(self.phase, Phase::Ready) {
            return false;
        }
        self.slot ^= 1;
        self.digest = hex_digest(&bytes);
        let mut upload = ShelfUpload::new(self.file(), bytes.clone());
        self.pending = Some(bytes);
        upload.start(context);
        self.phase = Phase::Writing(upload);
        true
    }

    pub fn retryable(&self) -> bool {
        matches!(self.phase, Phase::Failed)
    }

    pub fn retry(&mut self, context: &mut Context) {
        if self.retryable() {
            self.phase = Phase::Loading;
            context.store().load(&self.key);
        }
    }

    pub fn stored(&mut self, context: &mut Context, result: &StoreResult) -> Option<Event> {
        match (&self.phase, result) {
            (Phase::Loading, StoreResult::Loaded { value, .. }) => {
                if let Some(value) = value {
                    let Some((slot, digest)) = pointer(value) else {
                        return Some(self.fail());
                    };
                    self.slot = slot;
                    self.digest = digest;
                } else {
                    self.slot = 0;
                    self.digest.clear();
                }
                if let Some(bytes) = self.pending.take() {
                    self.phase = Phase::Ready;
                    self.save(context, bytes);
                    return None;
                }
                if value.is_none() {
                    self.phase = Phase::Ready;
                    return Some(Event::Loaded);
                }
                let mut read = ShelfDownload::new(self.file()).at_most(LIMIT);
                read.start(context);
                self.phase = Phase::Reading(read);
                None
            }
            (Phase::Committing, StoreResult::Saved { .. }) => {
                self.bytes = self.pending.take();
                self.phase = Phase::Ready;
                Some(Event::Saved)
            }
            (_, StoreResult::Denied(_)) => Some(self.fail()),
            _ => None,
        }
    }

    pub fn shelf(&mut self, context: &mut Context, result: &StoreResult) -> Option<Event> {
        let progress = match &mut self.phase {
            Phase::Reading(read) => read.advance(context, result),
            Phase::Writing(write) => write.advance(context, result),
            _ => return None,
        };
        match progress {
            ShelfProgress::Done => match std::mem::replace(&mut self.phase, Phase::Ready) {
                Phase::Reading(read) => {
                    let bytes = read.take();
                    if hex_digest(&bytes) != self.digest {
                        return Some(self.unavailable());
                    }
                    self.bytes = Some(bytes);
                    Some(Event::Loaded)
                }
                Phase::Writing(_) => {
                    context.store().save(
                        &self.key,
                        format!("{}:{}", self.slot, self.digest).into_bytes(),
                    );
                    self.phase = Phase::Committing;
                    None
                }
                _ => unreachable!(),
            },
            ShelfProgress::Failed(StoreError::Missing | StoreError::TooFull)
                if matches!(self.phase, Phase::Reading(_)) =>
            {
                Some(self.unavailable())
            }
            ShelfProgress::Failed(_) => Some(self.fail()),
            ShelfProgress::Moving { .. } | ShelfProgress::Elsewhere => None,
        }
    }

    fn unavailable(&mut self) -> Event {
        // The pointer was read successfully, so the published slot is known.
        // Keep it intact and allow a replacement download into the other slot.
        // This applies only to absent/invalid content, never uncertain writes.
        self.bytes = None;
        self.phase = Phase::Ready;
        Event::Loaded
    }

    fn fail(&mut self) -> Event {
        // A failed acknowledgement can still have committed the pointer. Do
        // not reuse either slot until a retry has reread the published pointer.
        self.phase = Phase::Failed;
        Event::Failed
    }
}

fn pointer(bytes: &[u8]) -> Option<(u8, String)> {
    let value = std::str::from_utf8(bytes).ok()?;
    let (slot, digest) = value.split_once(':')?;
    let slot = match slot {
        "0" => 0,
        "1" => 1,
        _ => return None,
    };
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return None;
    }
    Some((slot, digest.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobo_sdk::{ActionId, AppRunner, Command, KoboApp, StoreError, StoreRequest};

    struct Harness {
        cache: Cache,
        event: Option<Event>,
    }
    impl KoboApp for Harness {
        fn on_start(&mut self, context: &mut Context) {
            self.cache.start(context);
        }
        fn on_action(&mut self, context: &mut Context, action: ActionId) {
            if action == ActionId(2) {
                self.cache.retry(context);
                return;
            }
            self.cache.save(context, b"new article".to_vec());
        }
        fn on_store(&mut self, context: &mut Context, result: StoreResult) {
            self.event = if matches!(self.cache.phase, Phase::Reading(_) | Phase::Writing(_)) {
                self.cache.shelf(context, &result)
            } else {
                self.cache.stored(context, &result)
            };
        }
    }
    fn runner() -> AppRunner<Harness> {
        AppRunner::new(Harness {
            cache: Cache::new("https://example.com/feed"),
            event: None,
        })
    }
    fn load(runner: &mut AppRunner<Harness>, value: Option<Vec<u8>>) {
        runner.start();
        let key = runner.app_mut().cache.key.clone();
        runner.store_result(StoreResult::Loaded { key, value });
    }
    #[test]
    fn publication_waits_for_both_file_and_pointer_acknowledgements() {
        let mut runner = runner();
        load(&mut runner, None);
        let commands = runner.action(ActionId(1));
        assert!(!commands
            .iter()
            .any(|c| matches!(c, Command::Store(StoreRequest::Save { .. }))));
        assert!(runner.app_mut().cache.bytes.is_none());
        let name = runner.app_mut().cache.file();
        let commands = runner.store_result(StoreResult::ShelfWritten { name, size: 11 });
        assert!(commands
            .iter()
            .any(|c| matches!(c, Command::Store(StoreRequest::Save { .. }))));
        assert!(runner.app_mut().cache.bytes.is_none());
        let key = runner.app_mut().cache.key.clone();
        runner.store_result(StoreResult::Saved { key });
        assert_eq!(
            runner.app_mut().cache.bytes.as_deref(),
            Some(b"new article".as_slice())
        );
        assert_eq!(runner.app_mut().event, Some(Event::Saved));
    }
    #[test]
    fn restart_reads_and_verifies_the_published_snapshot() {
        let mut runner = runner();
        load(
            &mut runner,
            Some(format!("1:{}", hex_digest(b"saved article")).into_bytes()),
        );
        let name = runner.app_mut().cache.file();
        let commands = runner.store_result(StoreResult::ShelfRead {
            name,
            offset: 0,
            bytes: b"saved article".to_vec(),
            size: 13,
        });
        assert_eq!(
            runner.app_mut().cache.bytes.as_deref(),
            Some(b"saved article".as_slice())
        );
        assert!(!commands.iter().any(|c| matches!(c, Command::Spawn { .. })));
        assert_eq!(runner.app_mut().event, Some(Event::Loaded));
    }
    #[test]
    fn corrupted_snapshot_is_not_exposed_as_saved_articles() {
        let mut runner = runner();
        load(
            &mut runner,
            Some(format!("0:{}", hex_digest(b"saved article")).into_bytes()),
        );
        let name = runner.app_mut().cache.file();
        runner.store_result(StoreResult::ShelfRead {
            name,
            offset: 0,
            bytes: b"other article".to_vec(),
            size: 13,
        });
        assert!(runner.app_mut().cache.bytes.is_none());
        assert_eq!(runner.app_mut().event, Some(Event::Loaded));
        runner.action(ActionId(1));
        assert!(runner.app_mut().cache.file().ends_with(".1"));
        assert!(matches!(runner.app_mut().cache.phase, Phase::Writing(_)));
    }

    #[test]
    fn missing_snapshot_can_be_replaced_without_overwriting_its_slot() {
        let mut runner = runner();
        load(
            &mut runner,
            Some(format!("1:{}", hex_digest(b"old")).into_bytes()),
        );
        runner.store_result(StoreResult::Denied(StoreError::Missing));
        assert_eq!(runner.app_mut().event, Some(Event::Loaded));
        assert!(runner.app_mut().cache.bytes.is_none());
        runner.action(ActionId(1));
        assert!(runner.app_mut().cache.file().ends_with(".0"));
        assert!(matches!(runner.app_mut().cache.phase, Phase::Writing(_)));
    }

    #[test]
    fn retry_rereads_the_pointer_before_reusing_a_slot() {
        let mut runner = runner();
        load(&mut runner, None);
        runner.action(ActionId(1));
        let name = runner.app_mut().cache.file();
        runner.store_result(StoreResult::ShelfWritten { name, size: 11 });
        runner.store_result(StoreResult::Denied(StoreError::TooFull));
        let commands = runner.action(ActionId(2));
        assert!(commands
            .iter()
            .any(|command| matches!(command, Command::Store(StoreRequest::Load { .. }))));
        assert!(matches!(runner.app_mut().cache.phase, Phase::Loading));
        // The failed acknowledgement actually committed slot 1. Retry must
        // preserve that slot and write the retained candidate into slot 0.
        let key = runner.app_mut().cache.key.clone();
        runner.store_result(StoreResult::Loaded {
            key,
            value: Some(format!("1:{}", hex_digest(b"new article")).into_bytes()),
        });
        assert!(runner.app_mut().cache.file().ends_with(".0"));
        assert!(matches!(runner.app_mut().cache.phase, Phase::Writing(_)));
        let name = runner.app_mut().cache.file();
        runner.store_result(StoreResult::ShelfWritten { name, size: 11 });
        let key = runner.app_mut().cache.key.clone();
        runner.store_result(StoreResult::Saved { key });
        assert_eq!(
            runner.app_mut().cache.bytes.as_deref(),
            Some(b"new article".as_slice())
        );
    }

    #[test]
    fn a_new_refresh_replaces_the_unsaved_candidate_without_overwriting_a_slot() {
        let mut cache = Cache::new("https://example.com/feed");
        let mut context = Context::default();
        cache.phase = Phase::Failed;
        cache.pending = Some(b"older refresh".to_vec());
        assert!(!cache.save(&mut context, b"latest refresh".to_vec()));
        assert!(matches!(cache.phase, Phase::Failed));
        cache.retry(&mut context);
        let key = cache.key.clone();
        cache.stored(&mut context, &StoreResult::Loaded { key, value: None });
        assert!(matches!(cache.phase, Phase::Writing(_)));
        assert_eq!(cache.pending.as_deref(), Some(b"latest refresh".as_slice()));
        assert_eq!(cache.digest, hex_digest(b"latest refresh"));
    }

    #[test]
    fn uncertain_commit_cannot_overwrite_the_published_slot_on_retry() {
        let mut runner = runner();
        load(&mut runner, None);
        runner.action(ActionId(1));
        let name = runner.app_mut().cache.file();
        runner.store_result(StoreResult::ShelfWritten { name, size: 11 });
        runner.store_result(StoreResult::Denied(StoreError::TooFull));
        let commands = runner.action(ActionId(1));
        assert!(!commands.iter().any(|c| matches!(c, Command::Store(_))));
        assert!(matches!(runner.app_mut().cache.phase, Phase::Failed));
    }
}

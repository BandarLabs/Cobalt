//! A bounded, acknowledged set of article reading states.
//!
//! Shared by every reader of web articles. Where a book keeps one position, a
//! feed reader keeps hundreds of small ones, so the record has to be bounded
//! and it has to be honest: a position counts as saved only once the store has
//! acknowledged the write that carried it, and a failed write keeps the file a
//! reader already has rather than replacing it with a shorter one.
use kobo_read::Memory;
use kobo_sdk::{Context, StoreResult};
use kobo_ui::TextScale;
use std::collections::BTreeMap;

pub const KEY: &str = "reading-v1";
const LIMIT: usize = 192 * 1024;
const MAX_ARTICLES: usize = 1000;

/// Where the text size last chosen in this application is kept.
///
/// Every article opened for the first time used to start at 100%, so a reader
/// who needs 140% set it again on every story, every day. The size they chose
/// last is kept as one more record in the same file, under an identity no
/// article can have: an article's identity is a SHA-256 digest, and no input
/// digests to all zeros. A version of the application from before this was
/// added reads the record as an article nobody ever opens, so the file stays
/// readable in both directions.
const SIZE: &str = "0000000000000000000000000000000000000000000000000000000000000000";

#[derive(Default)]
pub struct Progress {
    records: BTreeMap<String, Memory>,
    saved: Vec<u8>,
    acknowledged: std::collections::BTreeSet<String>,
    writing: Option<Vec<u8>>,
    loaded: bool,
    pub failed: bool,
    capacity_failed: bool,
}

/// The identity a record is kept under: a SHA-256 digest of whatever the
/// application uses to name an article, such as its address.
///
/// Records are keyed by digest so a file of them has one fixed shape however
/// long the addresses are, and so nothing in it says which sites were read.
#[must_use]
pub fn identity(name: &str) -> String {
    kobo_net::sha256::hex_digest(name.as_bytes())
}

impl Progress {
    pub fn load(&mut self, result: StoreResult) {
        if let StoreResult::Loaded { value, .. } = result {
            let bytes = value.unwrap_or_default();
            if let Some(records) = decode(&bytes) {
                self.records = records;
                self.saved = encode(&self.records);
                self.acknowledged = self.records.keys().cloned().collect();
                self.loaded = true;
                self.failed = false;
                return;
            }
        }
        self.failed = true;
    }

    /// Whether this article has been opened, once the record has been read.
    #[must_use]
    pub fn has_read(&self, id: &str) -> Option<bool> {
        self.loaded.then(|| self.records.contains_key(id))
    }

    /// The same, counting only what the store has acknowledged keeping.
    #[must_use]
    pub fn has_saved_read(&self, id: &str) -> Option<bool> {
        self.loaded.then(|| self.acknowledged.contains(id))
    }

    /// Where this article was left, or the start of it at the text size
    /// last chosen in this application.
    #[must_use]
    pub fn memory(&self, id: &str) -> Memory {
        self.records.get(id).cloned().unwrap_or_else(|| Memory {
            scale: self.text_scale(),
            ..Memory::default()
        })
    }

    /// The text size last chosen in this application, which is what an
    /// article opened for the first time starts at.
    #[must_use]
    pub fn text_scale(&self) -> TextScale {
        self.records
            .get(SIZE)
            .map(|record| record.scale)
            .unwrap_or_default()
    }

    pub fn keep(&mut self, context: &mut Context, id: String, memory: Memory) {
        if !self.loaded {
            self.failed = true;
            return;
        }
        let mut next = self.records.clone();
        if memory.scale != self.text_scale() {
            next.insert(
                SIZE.to_owned(),
                Memory {
                    scale: memory.scale,
                    ..Memory::default()
                },
            );
        }
        next.insert(id, memory);
        if next.len() > MAX_ARTICLES || encode(&next).len() > LIMIT {
            self.failed = true;
            self.capacity_failed = true;
            return;
        }
        self.capacity_failed = false;
        self.records = next;
        if !self.failed {
            self.flush(context);
        }
    }

    /// Whether the last place could not be kept because the record of places
    /// is full, rather than because a write failed.
    ///
    /// A retry cannot mend that, so an application offers Retry saving only
    /// when this is false; offering it otherwise is a control that does
    /// nothing every time it is tapped.
    #[must_use]
    pub const fn out_of_room(&self) -> bool {
        self.capacity_failed
    }

    pub fn retry(&mut self, context: &mut Context) {
        if self.capacity_failed {
            return;
        }
        if !self.loaded {
            context.store().load(KEY);
            return;
        }
        self.failed = false;
        self.flush(context);
    }

    fn flush(&mut self, context: &mut Context) {
        let bytes = encode(&self.records);
        if self.writing.is_none() && bytes != self.saved {
            context.store().save(KEY, bytes.clone());
            self.writing = Some(bytes);
        }
    }

    pub fn saved(&mut self, context: &mut Context, result: StoreResult) {
        let Some(bytes) = self.writing.take() else {
            return;
        };
        if matches!(result, StoreResult::Saved { key } if key == KEY) {
            // What was written is what this decodes, so the fallback is never
            // reached; it is here because a reader losing its place is not
            // worth ending the application over.
            self.acknowledged = decode(&bytes).unwrap_or_default().into_keys().collect();
            self.saved = bytes;
            self.flush(context);
        } else {
            self.failed = true;
        }
    }
}

fn encode(records: &BTreeMap<String, Memory>) -> Vec<u8> {
    // The format names one application because Feeds wrote it first. Keeping
    // the name is what lets an upgrade open the positions a reader already has.
    let mut out = String::from("rss-reading-v1\n");
    for (id, memory) in records {
        out.push_str(id);
        out.push('\t');
        kobo_json::escape_into(
            &String::from_utf8(memory.encode()).expect("reader memory is UTF-8"),
            &mut out,
        );
        out.push('\n');
    }
    out.into_bytes()
}

fn decode(bytes: &[u8]) -> Option<BTreeMap<String, Memory>> {
    if bytes.is_empty() {
        return Some(BTreeMap::new());
    }
    if bytes.len() > LIMIT {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?;
    let mut lines = text.strip_prefix("rss-reading-v1\n")?.lines();
    let mut records = BTreeMap::new();
    for line in &mut lines {
        let (id, encoded) = line.split_once('\t')?;
        if id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()) || records.contains_key(id)
        {
            return None;
        }
        let value = kobo_json::parse(encoded).ok()?;
        let memory_text = value.as_str()?;
        let memory = Memory::decode(memory_text.as_bytes());
        if memory.encode() != memory_text.as_bytes() {
            return None;
        }
        records.insert(id.to_owned(), memory);
        if records.len() > MAX_ARTICLES {
            return None;
        }
    }
    Some(records)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobo_sdk::{ActionId, AppRunner, Command, KoboApp, StoreError, StoreRequest};
    #[derive(Default)]
    struct Harness {
        progress: Progress,
        at: u32,
    }
    impl KoboApp for Harness {
        fn on_start(&mut self, context: &mut Context) {
            context.store().load(KEY);
        }
        fn on_load(&mut self, _: &mut Context, _: &str, result: StoreResult) {
            self.progress.load(result);
        }
        fn on_save(&mut self, context: &mut Context, _: &str, result: StoreResult) {
            self.progress.saved(context, result);
        }
        fn on_action(&mut self, context: &mut Context, action: ActionId) {
            if action == ActionId(2) {
                self.progress.retry(context);
                return;
            }
            self.at += 1;
            self.progress.keep(
                context,
                "a".repeat(64),
                Memory {
                    at: self.at,
                    ..Memory::default()
                },
            );
        }
    }
    fn save(commands: &[Command]) -> Option<Vec<u8>> {
        commands.iter().find_map(|command| match command {
            Command::Store(StoreRequest::Save { value, .. }) => Some(value.clone()),
            _ => None,
        })
    }
    #[test]
    fn saved_read_status_waits_for_the_write_acknowledgement() {
        let mut progress = super::Progress::default();
        let mut context = kobo_sdk::Context::default();
        let id = "a".repeat(64);
        assert_eq!(progress.has_saved_read(&id), None);
        progress.load(StoreResult::Loaded {
            key: KEY.into(),
            value: None,
        });
        progress.keep(&mut context, id.clone(), kobo_read::Memory::default());
        assert_eq!(progress.has_read(&id), Some(true));
        assert_eq!(progress.has_saved_read(&id), Some(false));
        progress.saved(&mut context, StoreResult::Denied(StoreError::TooFull));
        assert_eq!(progress.has_saved_read(&id), Some(false));
        progress.retry(&mut context);
        progress.saved(&mut context, StoreResult::Saved { key: KEY.into() });
        assert_eq!(progress.has_saved_read(&id), Some(true));
    }

    #[test]
    fn newer_position_waits_for_the_previous_write_acknowledgement() {
        let mut runner = AppRunner::new(Harness::default());
        runner.start();
        runner.store_result(StoreResult::Loaded {
            key: KEY.into(),
            value: None,
        });
        let first = save(&runner.action(ActionId(1))).unwrap();
        assert!(save(&runner.action(ActionId(1))).is_none());
        let second = save(&runner.store_result(StoreResult::Saved { key: KEY.into() })).unwrap();
        assert_ne!(first, second);
        assert_eq!(decode(&second).unwrap()[&"a".repeat(64)].at, 2);
        runner.store_result(StoreResult::Denied(StoreError::TooFull));
        assert!(runner.app_mut().progress.failed);
        assert_eq!(save(&runner.action(ActionId(2))), Some(second));
    }
    #[test]
    fn unreadable_progress_is_preserved_and_cannot_be_overwritten() {
        let mut runner = AppRunner::new(Harness::default());
        runner.start();
        runner.store_result(StoreResult::Loaded {
            key: KEY.into(),
            value: Some(b"future format".to_vec()),
        });
        assert!(save(&runner.action(ActionId(1))).is_none());
        assert!(runner.app_mut().progress.failed);
    }
    #[test]
    fn a_new_article_opens_at_the_size_last_chosen_and_the_choice_survives_a_restart() {
        let mut progress = super::Progress::default();
        let mut context = kobo_sdk::Context::default();
        progress.load(StoreResult::Loaded {
            key: KEY.into(),
            value: None,
        });
        assert_eq!(progress.memory(&"b".repeat(64)).scale, TextScale::default());
        let larger = TextScale::default()
            .larger()
            .expect("there is a larger size");
        progress.keep(
            &mut context,
            "a".repeat(64),
            Memory {
                scale: larger,
                ..Memory::default()
            },
        );
        assert_eq!(progress.memory(&"b".repeat(64)).scale, larger);
        let mut reopened = super::Progress::default();
        reopened.load(StoreResult::Loaded {
            key: KEY.into(),
            value: Some(encode(&progress.records)),
        });
        assert_eq!(reopened.memory(&"c".repeat(64)).scale, larger);
        assert_eq!(reopened.memory(&"a".repeat(64)).scale, larger);
    }

    #[test]
    fn reading_memory_round_trips_and_rejects_truncated_records() {
        let memory = Memory {
            at: 25,
            ..Memory::default()
        };
        let records = BTreeMap::from([("a".repeat(64), memory)]);
        let bytes = encode(&records);
        assert_eq!(decode(&bytes), Some(records));
        assert!(decode(&bytes[..bytes.len() - 4]).is_none());
    }
}

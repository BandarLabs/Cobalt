//! Acknowledged refresh history, independent of article read status.
use kobo_sdk::{Context, StoreResult};
use std::collections::BTreeMap;

pub const KEY: &str = "feed-status-v1";
const LIMIT: usize = 32 * 1024;
const MAX_FEEDS: usize = 40;

#[derive(Clone, Default, Eq, PartialEq)]
struct Record {
    success: String,
    failure: String,
    unread: Option<(u8, u8)>,
}

enum Update {
    Refresh(Result<String, String>),
    Unread(u8, u8),
}

#[derive(Default)]
pub struct Statuses {
    records: BTreeMap<String, Record>,
    pending: Vec<(String, Update)>,
    saved: Vec<u8>,
    writing: Option<Vec<u8>>,
    loaded: bool,
    pub failed: bool,
}

impl Statuses {
    pub fn load(&mut self, context: &mut Context, result: StoreResult) {
        if let StoreResult::Loaded { value, .. } = result {
            if let Some(records) = decode(&value.unwrap_or_default()) {
                self.records = records;
                self.loaded = true;
                self.failed = false;
                self.saved = encode(&self.records);
                for (key, update) in std::mem::take(&mut self.pending) {
                    self.apply(key, update);
                }
                self.flush(context);
                return;
            }
        }
        self.failed = true;
    }

    pub fn note(&mut self, context: &mut Context, url: &str, result: Result<String, String>) {
        self.update(context, url, Update::Refresh(result));
    }

    pub fn unread(&mut self, context: &mut Context, url: &str, unread: usize, total: usize) {
        if unread > total || total > super::feed::MAX_ITEMS {
            self.failed = true;
            return;
        }
        self.update(
            context,
            url,
            Update::Unread(
                u8::try_from(unread).expect("bounded unread count"),
                u8::try_from(total).expect("bounded article count"),
            ),
        );
    }

    fn update(&mut self, context: &mut Context, url: &str, result: Update) {
        let key = kobo_net::sha256::hex_digest(url.as_bytes());
        if self.loaded {
            self.apply(key, result);
            self.flush(context);
        } else if self.pending.len() < MAX_FEEDS * 4 {
            self.pending.push((key, result));
        } else {
            self.failed = true;
        }
    }

    fn apply(&mut self, key: String, result: Update) {
        if !self.records.contains_key(&key) && self.records.len() >= MAX_FEEDS {
            self.failed = true;
            return;
        }
        let record = self.records.entry(key).or_default();
        match result {
            Update::Refresh(Ok(stamp)) => {
                record.success = stamp;
                record.failure.clear();
            }
            Update::Refresh(Err(reason)) => record.failure = reason,
            Update::Unread(unread, total) => record.unread = Some((unread, total)),
        }
    }

    pub fn summary(&self, url: &str) -> Option<String> {
        let record = self
            .records
            .get(&kobo_net::sha256::hex_digest(url.as_bytes()))?;
        let refreshed = if record.success.is_empty() {
            "Not refreshed yet".into()
        } else {
            format!("Updated {}", record.success)
        };
        let refreshed = match record.unread {
            Some((unread, _)) => format!("{unread} unread · {refreshed}"),
            None => refreshed,
        };
        Some(if record.failure.is_empty() {
            refreshed
        } else {
            format!("Refresh failed · {refreshed}")
        })
    }

    pub fn detail(&self, url: &str) -> Option<&str> {
        self.records
            .get(&kobo_net::sha256::hex_digest(url.as_bytes()))
            .map(|record| record.failure.as_str())
            .filter(|reason| !reason.is_empty())
    }

    pub fn retain(&mut self, context: &mut Context, urls: impl Iterator<Item = String>) {
        if !self.loaded {
            return;
        }
        let keys: std::collections::BTreeSet<_> = urls
            .map(|url| kobo_net::sha256::hex_digest(url.as_bytes()))
            .collect();
        self.records.retain(|key, _| keys.contains(key));
        self.flush(context);
    }

    pub fn retry(&mut self, context: &mut Context) {
        self.failed = false;
        if self.loaded {
            self.flush(context);
        } else {
            context.store().load(KEY);
        }
    }

    fn flush(&mut self, context: &mut Context) {
        if !self.loaded || self.failed || self.writing.is_some() {
            return;
        }
        let bytes = encode(&self.records);
        if bytes.len() > LIMIT {
            self.failed = true;
            return;
        }
        if bytes != self.saved {
            context.store().save(KEY, bytes.clone());
            self.writing = Some(bytes);
        }
    }

    pub fn saved(&mut self, context: &mut Context, result: &StoreResult) {
        let Some(bytes) = self.writing.take() else {
            return;
        };
        if matches!(result, StoreResult::Saved { .. }) {
            self.saved = bytes;
            self.flush(context);
        } else {
            self.failed = true;
        }
    }
}

fn encode(records: &BTreeMap<String, Record>) -> Vec<u8> {
    let mut out = String::from("rss-status-v1\n");
    for (key, record) in records {
        out.push_str(key);
        out.push('\t');
        kobo_json::escape_into(&record.success, &mut out);
        out.push('\t');
        kobo_json::escape_into(&record.failure, &mut out);
        out.push('\t');
        out.push_str(
            &record
                .unread
                .map_or_else(|| "-".into(), |(unread, total)| format!("{unread}/{total}")),
        );
        out.push('\n');
    }
    out.into_bytes()
}

fn decode(bytes: &[u8]) -> Option<BTreeMap<String, Record>> {
    if bytes.is_empty() {
        return Some(BTreeMap::new());
    }
    if bytes.len() > LIMIT {
        return None;
    }
    let text = std::str::from_utf8(bytes)
        .ok()?
        .strip_prefix("rss-status-v1\n")?;
    let mut records = BTreeMap::new();
    for line in text.lines() {
        let fields: Vec<_> = line.split('\t').collect();
        let (key, success, failure, count) = match fields.as_slice() {
            [key, success, failure] => (*key, *success, *failure, "-"),
            [key, success, failure, count] => (*key, *success, *failure, *count),
            _ => return None,
        };
        let unread = if count == "-" {
            None
        } else {
            let (unread, total) = count.split_once('/')?;
            let unread: u8 = unread.parse().ok()?;
            let total: u8 = total.parse().ok()?;
            if unread > total || usize::from(total) > super::feed::MAX_ITEMS {
                return None;
            }
            Some((unread, total))
        };
        if key.len() != 64
            || !key.bytes().all(|b| b.is_ascii_hexdigit())
            || records.len() >= MAX_FEEDS
            || records.contains_key(key)
        {
            return None;
        }
        let success = kobo_json::parse(success).ok()?.as_str()?.to_owned();
        let failure = kobo_json::parse(failure).ok()?.as_str()?.to_owned();
        if success.len() > 40 || failure.len() > 512 {
            return None;
        }
        records.insert(
            key.to_owned(),
            Record {
                success,
                failure,
                unread,
            },
        );
    }
    Some(records)
}

pub fn timestamp() -> Option<String> {
    use kobo_sdk::clock::{Clock, SystemClock};
    let now = SystemClock::new(0).ok()?.now().ok()?;
    let (hour, minute) = now.hour_minute()?;
    Some(format!("{} {hour:02}:{minute:02} UTC", now.date()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    const URL: &str = "https://example.com/feed";
    fn loaded() -> (Statuses, Context) {
        let mut status = Statuses::default();
        let mut context = Context::default();
        status.load(
            &mut context,
            StoreResult::Loaded {
                key: KEY.into(),
                value: None,
            },
        );
        (status, context)
    }
    #[test]
    fn failure_retains_success_time_and_success_clears_failure() {
        let (mut status, mut context) = loaded();
        status.note(&mut context, URL, Ok("2026-09-09 08:30 UTC".into()));
        status.note(&mut context, URL, Err("Offline".into()));
        assert!(status
            .summary(URL)
            .unwrap()
            .contains("2026-09-09 08:30 UTC"));
        assert_eq!(status.detail(URL), Some("Offline"));
        status.note(&mut context, URL, Ok("2026-09-10 10:45 UTC".into()));
        assert_eq!(status.detail(URL), None);
        assert!(status.summary(URL).unwrap().contains("2026-09-10"));
    }
    #[test]
    fn pending_refresh_events_keep_their_order_when_the_file_loads() {
        let mut status = Statuses::default();
        let mut context = Context::default();
        status.note(&mut context, URL, Ok("2026-09-09 08:30 UTC".into()));
        status.note(&mut context, URL, Err("Offline".into()));
        status.load(
            &mut context,
            StoreResult::Loaded {
                key: KEY.into(),
                value: None,
            },
        );
        assert!(status.summary(URL).unwrap().contains("2026-09-09"));
        assert_eq!(status.detail(URL), Some("Offline"));
    }
    #[test]
    fn writes_wait_for_ack_and_retry_the_latest_history() {
        let (mut status, mut context) = loaded();
        status.note(&mut context, URL, Ok("2026-09-09 08:30 UTC".into()));
        let first = status.writing.clone();
        status.note(&mut context, URL, Err("Offline".into()));
        assert_eq!(status.writing, first);
        status.saved(&mut context, &StoreResult::Saved { key: KEY.into() });
        assert_ne!(status.writing, first);
        status.saved(
            &mut context,
            &StoreResult::Denied(kobo_sdk::StoreError::NoRoom),
        );
        assert!(status.failed);
        status.retry(&mut context);
        let bytes = status.writing.clone().unwrap();
        let mut reopened = Statuses::default();
        reopened.load(
            &mut context,
            StoreResult::Loaded {
                key: KEY.into(),
                value: Some(bytes),
            },
        );
        assert_eq!(reopened.detail(URL), Some("Offline"));
        assert!(reopened.summary(URL).unwrap().contains("2026-09-09"));
    }
    #[test]
    fn unreadable_history_is_not_overwritten_by_a_refresh() {
        let mut status = Statuses::default();
        let mut context = Context::default();
        status.load(
            &mut context,
            StoreResult::Loaded {
                key: KEY.into(),
                value: Some(b"future data".to_vec()),
            },
        );
        status.note(&mut context, URL, Err("Offline".into()));
        assert!(status.failed);
        assert!(status.writing.is_none());
    }
    #[test]
    fn removed_feeds_release_only_their_history() {
        let (mut status, mut context) = loaded();
        status.note(&mut context, URL, Err("Offline".into()));
        status.note(
            &mut context,
            "https://example.com/other",
            Err("Offline".into()),
        );
        status.retain(&mut context, [URL.to_owned()].into_iter());
        assert!(status.summary(URL).is_some());
        assert!(status.summary("https://example.com/other").is_none());
    }
    #[test]
    fn unread_summaries_survive_reload_without_changing_refresh_history() {
        let (mut status, mut context) = loaded();
        status.note(&mut context, URL, Ok("2026-09-09 08:30 UTC".into()));
        status.unread(&mut context, URL, 3, 5);
        status.saved(&mut context, &StoreResult::Saved { key: KEY.into() });
        let bytes = status.writing.clone().unwrap();
        let mut restored = Statuses::default();
        restored.load(
            &mut context,
            StoreResult::Loaded {
                key: KEY.into(),
                value: Some(bytes),
            },
        );
        assert!(restored.summary(URL).unwrap().starts_with("3 unread"));
        assert!(restored.summary(URL).unwrap().contains("2026-09-09"));
        restored.unread(&mut context, URL, 6, 5);
        assert!(restored.failed);
        assert!(restored.summary(URL).unwrap().starts_with("3 unread"));
    }

    #[test]
    fn malformed_and_duplicate_records_are_refused() {
        assert!(decode(b"rss-status-v1\ninvalid").is_none());
        let key = "a".repeat(64);
        let row = format!("{key}\t\"\"\t\"Offline\"\n");
        assert!(decode(format!("rss-status-v1\n{row}{row}").as_bytes()).is_none());
        assert!(decode(&vec![b'a'; LIMIT + 1]).is_none());
    }
}

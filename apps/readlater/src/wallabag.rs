//! The Wallabag boundary. The application names a credential; the runtime owns it.

use kobo_json::Value;
#[cfg(test)]
use kobo_sdk::{Credential, Task};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Entry {
    pub id: u64,
    pub title: String,
    pub site: String,
    pub reading_time: u64,
    pub content: String,
    pub position: usize,
    pub starred: bool,
    pub archived: bool,
}

/// The reading list for one tab: unread, starred, or archived articles.
pub fn queue_url(server: &str, depth: u16, starred: bool, archived: bool) -> String {
    format!(
        "{}/api/entries.json?detail=metadata&perPage={depth}&page=1&archive={}&starred={}",
        server.trim_end_matches('/'),
        u8::from(archived),
        u8::from(starred),
    )
}

/// The outbox writes Wallabag understands on an entry document.
pub fn archive_body(archived: bool) -> String {
    format!("{{\"archive\":{}}}", u8::from(archived))
}

pub fn star_body(starred: bool) -> String {
    format!("{{\"star\":{}}}", u8::from(starred))
}

pub fn entry_url(server: &str, id: u64) -> String {
    format!("{}/api/entries/{id}.json", server.trim_end_matches('/'))
}

#[cfg(test)]
pub fn archive(server: &str, credential: &str, id: u64) -> Task {
    Task::Post {
        url: entry_url(server, id),
        body: "{\"archive\":1}".to_owned(),
        content_type: "application/json".to_owned(),
        credential: Some(Credential::bearer(credential)),
        headers: Vec::new(),
        max_bytes: 16 * 1024,
    }
}

pub fn parse_entries(bytes: &[u8]) -> Option<Vec<Entry>> {
    let value = kobo_json::parse(std::str::from_utf8(bytes).ok()?).ok()?;
    let entries = value
        .get("_embedded")
        .and_then(|v| v.get("items"))
        .or_else(|| value.get("items"))
        .and_then(Value::as_array)?;
    let mut seen = std::collections::BTreeSet::new();
    entries
        .iter()
        .map(|value| {
            let entry = parse_entry(value)?;
            seen.insert(entry.id).then_some(entry)
        })
        .collect()
}

pub fn parse_entry_document(bytes: &[u8]) -> Option<Entry> {
    let value = kobo_json::parse(&String::from_utf8_lossy(bytes)).ok()?;
    parse_entry(&value)
}

pub fn parse_entry(value: &Value) -> Option<Entry> {
    Some(Entry {
        id: u64::try_from(value.get("id")?.as_i64()?).ok()?,
        title: text(value, "title", "Untitled"),
        site: text(value, "domain_name", "Wallabag"),
        reading_time: u64::try_from(
            value
                .get("reading_time")
                .and_then(Value::as_i64)
                .unwrap_or(0),
        )
        .unwrap_or(0),
        position: 0,
        starred: flag(value, "is_starred"),
        archived: flag(value, "is_archived"),
        content: kobo_html::to_text_within(
            value
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            // Whole articles, not excerpts: the reader is the offline copy.
            256 * 1024,
        ),
    })
}

fn flag(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_i64) == Some(1)
}

fn text(value: &Value, key: &str, fallback: &str) -> String {
    let found = value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    if found.is_empty() {
        fallback.to_owned()
    } else {
        found.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_is_bounded_and_credential_never_enters_a_body() {
        assert_eq!(
            queue_url("https://bag.example/", 50, false, false),
            "https://bag.example/api/entries.json?detail=metadata&perPage=50&page=1&archive=0&starred=0"
        );
        assert!(queue_url("https://bag.example/", 20, true, false).contains("starred=1"));
        assert!(queue_url("https://bag.example/", 100, false, true).contains("archive=1"));
        let Task::Post {
            body, credential, ..
        } = archive("https://bag.example", "wallabag", 9)
        else {
            panic!()
        };
        assert_eq!(body, "{\"archive\":1}");
        assert_eq!(credential, Some(Credential::bearer("wallabag")));
    }

    #[test]
    fn invalid_lists_are_distinct_from_a_valid_empty_queue() {
        assert_eq!(parse_entries(br#"{"items":[]}"#), Some(vec![]));
        for invalid in [
            b"not JSON".as_slice(),
            b"{}",
            br#"{"items":[{}]}"#,
            br#"{"items":[{"id":1},{"id":1}]}"#,
            &[255],
        ] {
            assert!(parse_entries(invalid).is_none());
        }
    }

    #[test]
    fn parses_wallabag_embedded_items() {
        let entries = parse_entries(br#"{"_embedded":{"items":[{"id":7,"title":"A piece","domain_name":"example.org","reading_time":4}]}}"#).unwrap();
        assert_eq!(entries[0].title, "A piece");
        assert_eq!(entries[0].reading_time, 4);
    }

    #[test]
    fn metadata_queue_omits_article_bodies() {
        let entries =
            parse_entries(include_bytes!("../tests/fixtures/entries-metadata.json")).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].title, "Why the Borrow Checker Exists");
        assert_eq!(entries[0].site, "example.com");
        assert_eq!(entries[1].title, "Paper Notes on Local-First Software");
        assert!(entries.iter().all(|entry| entry.content.is_empty()));
        assert_eq!(
            entry_url("https://bag.example/", 7),
            "https://bag.example/api/entries/7.json"
        );
    }

    #[test]
    fn flags_and_outbox_bodies_match_the_api() {
        let entry = parse_entry(
            &kobo_json::parse(r#"{"id":9,"title":"t","is_starred":1,"is_archived":0}"#).unwrap(),
        )
        .unwrap();
        assert!(entry.starred);
        assert!(!entry.archived);
        assert_eq!(archive_body(true), r#"{"archive":1}"#);
        assert_eq!(star_body(false), r#"{"star":0}"#);
    }

    #[test]
    fn entry_document_carries_extracted_html() {
        let entry = parse_entry_document(include_bytes!("../tests/fixtures/entry-7.json"))
            .expect("one Wallabag entry");
        assert_eq!(entry.id, 7);
        assert_eq!(entry.title, "Why the Borrow Checker Exists");
        assert!(entry.content.contains("ownership is not optional"));
    }
}

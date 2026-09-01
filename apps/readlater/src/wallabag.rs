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
}

pub fn queue_url(server: &str, depth: u16) -> String {
    format!(
        "{}/api/entries.json?detail=metadata&perPage={depth}&page=1&archive=0",
        server.trim_end_matches('/')
    )
}

#[cfg(test)]
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

pub fn parse_entries(bytes: &[u8]) -> Vec<Entry> {
    let Ok(value) = kobo_json::parse(&String::from_utf8_lossy(bytes)) else {
        return Vec::new();
    };
    let entries = value
        .get("_embedded")
        .and_then(|v| v.get("items"))
        .or_else(|| value.get("items"))
        .and_then(Value::as_array)
        .unwrap_or(&[]);
    entries.iter().filter_map(parse_entry).collect()
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
        content: value
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
    })
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
            queue_url("https://bag.example/", 50),
            "https://bag.example/api/entries.json?detail=metadata&perPage=50&page=1&archive=0"
        );
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
    fn parses_wallabag_embedded_items() {
        let entries = parse_entries(br#"{"_embedded":{"items":[{"id":7,"title":"A piece","domain_name":"example.org","reading_time":4}]}}"#);
        assert_eq!(entries[0].title, "A piece");
        assert_eq!(entries[0].reading_time, 4);
    }
}

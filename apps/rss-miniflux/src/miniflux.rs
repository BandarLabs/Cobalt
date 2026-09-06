//! Miniflux requests and parsing; secret tokens remain in the runtime.

use kobo_json::Value;
use kobo_sdk::{Credential, Task};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Article {
    pub id: u64,
    pub title: String,
    pub feed: String,
    pub content: String,
    pub starred: bool,
}

pub fn endpoint(server: &str, path: &str) -> String {
    format!("{}/v1/{path}", server.trim_end_matches('/'))
}
pub fn unread(server: &str, credential: &str, depth: u16) -> Task {
    Task::Fetch {
        url: endpoint(
            server,
            &format!("entries?status=unread&limit={depth}&order=published_at&direction=desc"),
        ),
        offset: 0,
        max_bytes: 768 * 1024,
        credential: Some(Credential::in_header(credential, "X-Auth-Token")),
        headers: Vec::new(),
    }
}
#[cfg(test)]
pub fn mark_read(server: &str, credential: &str, ids: &[u64]) -> Task {
    let ids = ids.iter().map(u64::to_string).collect::<Vec<_>>().join(",");
    Task::Post {
        url: endpoint(server, "entries"),
        body: format!("{{\"entry_ids\":[{ids}],\"status\":\"read\"}}"),
        content_type: "application/json".to_owned(),
        credential: Some(Credential::in_header(credential, "X-Auth-Token")),
        headers: Vec::new(),
        max_bytes: 16 * 1024,
    }
}
pub fn parse_entries(bytes: &[u8]) -> Vec<Article> {
    let Ok(value) = kobo_json::parse(&String::from_utf8_lossy(bytes)) else {
        return Vec::new();
    };
    value
        .get("entries")
        .and_then(Value::as_array)
        .unwrap_or(&[])
        .iter()
        .filter_map(|entry| {
            Some(Article {
                id: u64::try_from(entry.get("id")?.as_i64()?).ok()?,
                title: nonempty(entry.get("title").and_then(Value::as_str), "Untitled"),
                feed: nonempty(
                    entry
                        .get("feed")
                        .and_then(|f| f.get("title"))
                        .and_then(Value::as_str),
                    "Feed",
                ),
                content: entry
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                starred: entry
                    .get("starred")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            })
        })
        .collect()
}
fn nonempty(found: Option<&str>, fallback: &str) -> String {
    let value = found.unwrap_or_default().trim();
    if value.is_empty() {
        fallback.to_owned()
    } else {
        value.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn token_is_a_named_runtime_header_not_a_body_value() {
        let Task::Post {
            body, credential, ..
        } = mark_read("https://flux.example", "miniflux", &[2, 5])
        else {
            panic!()
        };
        assert_eq!(body, "{\"entry_ids\":[2,5],\"status\":\"read\"}");
        assert_eq!(
            credential,
            Some(Credential::in_header("miniflux", "X-Auth-Token"))
        );
    }
    #[test]
    fn parses_a_batch_response() {
        let values = parse_entries(br#"{"entries":[{"id":1,"title":"News","feed":{"title":"Paper"},"content":"Body","starred":true}]}"#);
        assert_eq!(
            values[0],
            Article {
                id: 1,
                title: "News".into(),
                feed: "Paper".into(),
                content: "Body".into(),
                starred: true
            }
        );
    }
}

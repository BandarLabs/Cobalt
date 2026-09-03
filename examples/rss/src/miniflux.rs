//! Narrow Miniflux request builders and response readers.
//!
//! `kobod` resolves the dedicated `miniflux` secret into `X-Auth-Token` while
//! executing a task, so token bytes never enter this module, the UI, or the
//! store.

use kobo_html::{to_text, to_text_within};
use kobo_json::Value;
use kobo_sdk::{Credential, Task};

const ENTRY_BYTES: u32 = 512 * 1024;
const SMALL_RESPONSE_BYTES: u32 = 32 * 1024;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ListMode {
    #[default]
    Unread,
    Starred,
    History,
}

impl ListMode {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Unread => "Unread",
            Self::Starred => "Starred",
            Self::History => "History",
        }
    }

    #[must_use]
    pub const fn cache_index(self) -> usize {
        match self {
            Self::Unread => 0,
            Self::Starred => 1,
            Self::History => 2,
        }
    }

    #[must_use]
    const fn cache_name(self) -> &'static str {
        match self {
            Self::Unread => "unread",
            Self::Starred => "starred",
            Self::History => "history",
        }
    }

    fn query(self) -> &'static str {
        match self {
            Self::Unread => "status=unread&limit=100&order=published_at&direction=desc",
            Self::Starred => "starred=true&limit=100&order=published_at&direction=desc",
            Self::History => "status=read&limit=100&order=published_at&direction=desc",
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Article {
    pub id: u64,
    pub title: String,
    pub feed: String,
    pub content: String,
    pub status: String,
    pub starred: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Discovered {
    pub url: String,
    pub title: String,
    pub kind: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Mutation {
    Read(u64),
    Star { id: u64, starred: bool },
}

/// Returns the canonical HTTPS server and optional reverse-proxy prefix.
///
/// The result removes redundant trailing slashes, folds host case, and folds
/// the default HTTPS port so equivalent settings share one durable namespace.
/// A path prefix remains part of the value because it is part of the service
/// selected by a reverse proxy.
#[must_use]
pub fn canonical_server(server: &str) -> Option<String> {
    let server = server.trim();
    if server.is_empty() || server.contains(['?', '#', '\\', '%']) {
        return None;
    }
    let address = kobo_net::parse(server).ok()?;
    let path = address.path.trim_end_matches('/');
    if !path.is_empty()
        && (!path.starts_with('/')
            || path.starts_with("//")
            || path.split('/').any(|part| matches!(part, "." | "..")))
    {
        return None;
    }
    let host = address.host.to_ascii_lowercase();
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host
    };
    let authority = if address.port == 443 {
        host
    } else {
        format!("{host}:{}", address.port)
    };
    Some(format!("https://{authority}{path}"))
}

/// Returns the short durable-store namespace for one canonical server.
///
/// The origin and its intentional proxy prefix determine the identifier; no
/// credential or user-entered secret contributes to it.
#[must_use]
pub fn namespace(server: &str) -> Option<String> {
    canonical_server(server).map(|server| format!("{:016x}", stable_hash(&server)))
}

#[must_use]
pub fn cache_key(server: &str, mode: ListMode) -> Option<String> {
    namespace(server).map(|namespace| format!("miniflux.{namespace}.cache.{}", mode.cache_name()))
}

#[must_use]
pub fn actions_key(server: &str) -> Option<String> {
    namespace(server).map(|namespace| format!("miniflux.{namespace}.actions"))
}

#[must_use]
pub fn full_index_key(server: &str) -> Option<String> {
    namespace(server).map(|namespace| format!("miniflux.{namespace}.full-index"))
}

#[must_use]
pub fn full_content_key(server: &str, id: u64) -> Option<String> {
    namespace(server).map(|namespace| format!("miniflux.{namespace}.full-{id}"))
}

#[must_use]
pub fn full_content_id(server: &str, key: &str) -> Option<u64> {
    let namespace = namespace(server)?;
    key.strip_prefix(&format!("miniflux.{namespace}.full-"))?
        .parse()
        .ok()
}

/// Builds a v1 URL beneath the configured Miniflux server.
#[must_use]
pub fn endpoint(server: &str, path: &str) -> String {
    format!("{}/v1/{path}", server.trim().trim_end_matches('/'))
}

fn token() -> Credential {
    Credential::in_header("miniflux", "X-Auth-Token")
}

/// Validates the user-supplied server before the UI offers a sync.
///
/// Requests retain a configured path prefix (for reverse-proxy deployments),
/// but credentials can only ever be attached to the `/v1` endpoints built
/// below and approved by the platform policy.
#[must_use]
pub fn configured_server(server: &str) -> bool {
    canonical_server(server).is_some()
}

fn stable_hash(value: &str) -> u64 {
    value.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

#[must_use]
pub fn entries(server: &str, mode: ListMode) -> Task {
    Task::Fetch {
        url: endpoint(server, &format!("entries?{}", mode.query())),
        offset: 0,
        max_bytes: ENTRY_BYTES,
        credential: Some(token()),
        headers: Vec::new(),
    }
}

#[must_use]
pub fn discover(server: &str, website: &str) -> Task {
    post(
        server,
        "discover",
        format!(r#"{{"url":{}}}"#, json_string(website.trim())),
    )
}

/// Creates a feed from a documented discovery URL.
///
/// `category_id` is optional in supported current Miniflux releases. Leaving
/// it out lets Miniflux select its default category without pretending every
/// account's category IDs are the same.
#[must_use]
pub fn subscribe(server: &str, feed_url: &str) -> Task {
    post(
        server,
        "feeds",
        format!(r#"{{"feed_url":{}}}"#, json_string(feed_url)),
    )
}

#[must_use]
pub fn full_content(server: &str, id: u64) -> Task {
    Task::Fetch {
        url: endpoint(server, &format!("entries/{id}/fetch-content")),
        offset: 0,
        max_bytes: ENTRY_BYTES,
        credential: Some(token()),
        headers: Vec::new(),
    }
}

#[must_use]
pub fn mutate(server: &str, mutation: &Mutation) -> Task {
    let body = match mutation {
        Mutation::Read(id) => format!(r#"{{"entry_ids":[{id}],"status":"read"}}"#),
        Mutation::Star { id, starred } => {
            format!(r#"{{"entry_ids":[{id}],"starred":{starred}}}"#)
        }
    };
    Task::Put {
        url: endpoint(server, "entries"),
        body,
        content_type: "application/json".to_owned(),
        credential: Some(token()),
        headers: Vec::new(),
        max_bytes: SMALL_RESPONSE_BYTES,
    }
}

fn post(server: &str, path: &str, body: String) -> Task {
    Task::Post {
        url: endpoint(server, path),
        body,
        content_type: "application/json".to_owned(),
        credential: Some(token()),
        headers: Vec::new(),
        max_bytes: SMALL_RESPONSE_BYTES,
    }
}

/// Reads the `/v1/entries` response, distinguishing an empty list from an
/// invalid payload so a completed malformed response cannot erase a cache.
#[must_use]
pub fn parse_entries(bytes: &[u8]) -> Option<Vec<Article>> {
    let value = kobo_json::parse(&String::from_utf8_lossy(bytes)).ok()?;
    Some(
        value
            .get("entries")?
            .as_array()?
            .iter()
            .filter_map(article)
            .collect(),
    )
}

#[must_use]
pub fn parse_discoveries(bytes: &[u8]) -> Vec<Discovered> {
    let Ok(value) = kobo_json::parse(&String::from_utf8_lossy(bytes)) else {
        return Vec::new();
    };
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let url = entry.get("url").and_then(Value::as_str)?.trim();
            (!url.is_empty()).then(|| Discovered {
                url: url.to_owned(),
                title: text(entry.get("title").and_then(Value::as_str), "Untitled feed"),
                kind: text(entry.get("type").and_then(Value::as_str), "feed"),
            })
        })
        .collect()
}

/// Reads the entry payload returned by `/fetch-content`.
#[must_use]
pub fn parse_full_content(bytes: &[u8], maximum_bytes: usize) -> Option<String> {
    let value = kobo_json::parse(&String::from_utf8_lossy(bytes)).ok()?;
    value
        .get("content")
        .and_then(Value::as_str)
        // One byte beyond the storable ceiling ensures `to_text_within`'s
        // explicit truncation marker makes an overlong article unmistakable
        // to the caller. A full article is either stored exactly or kept out
        // of the offline cache; it is never silently shortened and labelled
        // "saved".
        .map(|content| to_text_within(content, maximum_bytes.saturating_add(1)))
        .filter(|content| !content.trim().is_empty())
}

fn article(entry: &Value) -> Option<Article> {
    Some(Article {
        id: u64::try_from(entry.get("id")?.as_i64()?).ok()?,
        title: text(entry.get("title").and_then(Value::as_str), "Untitled"),
        feed: text(
            entry
                .get("feed")
                .and_then(|feed| feed.get("title"))
                .and_then(Value::as_str),
            "Feed",
        ),
        content: to_text(
            entry
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        ),
        status: text(entry.get("status").and_then(Value::as_str), "unread"),
        starred: entry
            .get("starred")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn text(found: Option<&str>, fallback: &str) -> String {
    let found = found.unwrap_or_default().trim();
    if found.is_empty() {
        fallback.to_owned()
    } else {
        found.to_owned()
    }
}

fn json_string(value: &str) -> String {
    let mut quoted = String::from("\"");
    for character in value.chars() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            character if character.is_control() => {
                use std::fmt::Write as _;
                let _ = write!(quoted, "\\u{:04x}", character as u32);
            }
            character => quoted.push(character),
        }
    }
    quoted.push('"');
    quoted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_use_the_named_runtime_token_and_documented_json() {
        let Task::Put {
            body, credential, ..
        } = mutate("https://flux.example", &Mutation::Read(42))
        else {
            panic!("expected PUT");
        };
        assert_eq!(body, r#"{"entry_ids":[42],"status":"read"}"#);
        assert_eq!(
            credential,
            Some(Credential::in_header("miniflux", "X-Auth-Token"))
        );

        let Task::Put { body, .. } = mutate(
            "https://flux.example",
            &Mutation::Star {
                id: 42,
                starred: true,
            },
        ) else {
            panic!("expected POST");
        };
        assert_eq!(body, r#"{"entry_ids":[42],"starred":true}"#);
    }

    #[test]
    fn modes_and_full_content_have_only_the_allowed_request_shapes() {
        let Task::Fetch { url, .. } = entries("https://flux.example", ListMode::History) else {
            panic!("expected fetch");
        };
        assert_eq!(
            url,
            "https://flux.example/v1/entries?status=read&limit=100&order=published_at&direction=desc"
        );
        let Task::Fetch { url, .. } = full_content("https://flux.example", 7) else {
            panic!("expected fetch");
        };
        assert_eq!(url, "https://flux.example/v1/entries/7/fetch-content");
    }

    #[test]
    fn discovery_and_entries_parse_without_leaking_markup() {
        let feeds = parse_discoveries(
            br#"[{"url":"https://example.test/atom","title":"Journal","type":"atom"}]"#,
        );
        assert_eq!(feeds[0].title, "Journal");
        let entries = parse_entries(br#"{"entries":[{"id":7,"title":"News","feed":{"title":"Paper"},"content":"<p>Body</p>","status":"read","starred":true}]}"#)
            .expect("valid entries response");
        assert_eq!(entries[0].content, "Body");
        assert_eq!(entries[0].status, "read");
        assert_eq!(
            parse_full_content(br#"{"content":"<p>Full story</p>"}"#, 1024).as_deref(),
            Some("Full story")
        );
    }

    #[test]
    fn server_must_be_a_plain_https_origin_or_prefix() {
        assert!(configured_server("https://flux.example"));
        assert!(configured_server("https://flux.example/miniflux"));
        assert!(!configured_server("http://flux.example"));
        assert!(!configured_server("https://flux.example/?bad"));
    }

    #[test]
    fn server_namespace_canonicalizes_only_equivalent_origins() {
        assert_eq!(
            canonical_server(" https://FLUX.example:443/reader/ "),
            Some("https://flux.example/reader".to_owned())
        );
        assert_eq!(
            namespace("https://FLUX.example:443/reader/"),
            namespace("https://flux.example/reader")
        );
        assert_ne!(
            namespace("https://flux.example/reader"),
            namespace("https://other.example/reader")
        );
    }

    #[test]
    fn entries_payload_requires_an_entries_array_but_allows_an_empty_one() {
        assert_eq!(parse_entries(b"{not JSON"), None);
        assert_eq!(parse_entries(br#"{"total":0}"#), None);
        assert_eq!(parse_entries(br#"{"entries":{}}"#), None);
        assert_eq!(parse_entries(br#"{"entries":[]}"#), Some(Vec::new()));
    }

    #[test]
    fn ipv6_servers_have_one_pair_of_authority_brackets() {
        assert_eq!(
            canonical_server("https://[::1]"),
            Some("https://[::1]".to_owned())
        );
        assert_eq!(
            canonical_server("https://[::1]:8443/reader/"),
            Some("https://[::1]:8443/reader".to_owned())
        );
    }
}

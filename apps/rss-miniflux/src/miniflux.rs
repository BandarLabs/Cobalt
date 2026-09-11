//! Miniflux requests and parsing; secret tokens remain in the runtime.

use kobo_json::Value;
use kobo_sdk::{Credential, Task, UpdateMethod};
use std::fmt::Write;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Article {
    pub id: u64,
    pub title: String,
    pub feed: String,
    pub content: String,
    pub starred: bool,
    pub url: String,
    pub status: Status,
}

pub fn endpoint(server: &str, path: &str) -> String {
    format!("{}/v1/{path}", server.trim_end_matches('/'))
}
fn token(credential: &str) -> Credential {
    Credential::in_header(credential, "X-Auth-Token")
}

/// What a reply may weigh. Miniflux answers a batch in a few hundred
/// kilobytes; anything past this is not a batch, and the runtime stops reading.
fn response_ceiling() -> u32 {
    u32::try_from(MAX_RESPONSE).unwrap_or(u32::MAX)
}

fn fetch(server: &str, credential: &str, path: &str) -> Task {
    Task::Fetch {
        url: endpoint(server, path),
        offset: 0,
        max_bytes: response_ceiling(),
        credential: Some(token(credential)),
        headers: Vec::new(),
    }
}

/// One of the three lists a sync collects.
///
/// A Miniflux query carries one status, so the three tabs are three requests
/// merged into one saved batch rather than one clever request. Fetching them
/// together is what lets Starred and History work with the radio off: an
/// article starred and then read belongs in two tabs, and a reader on a train
/// should find it in both without waiting for anything.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Part {
    Unread,
    Starred,
    Read,
}

impl Part {
    /// In order, with the depth each contributes to the saved batch.
    ///
    /// Unread is most of it, because that is what a feed reader is for. The
    /// depths add up to the batch ceiling so a merged sync cannot overflow the
    /// file it is about to be written into.
    pub const ORDER: [(Self, u16); 3] = [(Self::Unread, 60), (Self::Starred, 20), (Self::Read, 20)];

    fn query(self, depth: u16) -> String {
        let filter = match self {
            Self::Unread => "status=unread",
            Self::Starred => "starred=true",
            Self::Read => "status=read",
        };
        format!("entries?limit={depth}&{filter}&order=published_at&direction=desc")
    }
}

/// One of the lists the tabs are drawn from.
pub fn entries(server: &str, credential: &str, part: Part, depth: u16) -> Task {
    fetch(server, credential, &part.query(depth))
}

/// Asks Miniflux for the page behind a summary-only article.
///
/// `update_content=false` leaves the account's own copy alone: fetching the
/// rest of an article to read it here is not a reason to rewrite what the
/// reader sees in every other Miniflux client.
pub fn full_content(server: &str, credential: &str, id: u64) -> Task {
    fetch(
        server,
        credential,
        &format!("entries/{id}/fetch-content?update_content=false"),
    )
}

/// The categories a new feed can be filed under.
pub fn categories(server: &str, credential: &str) -> Task {
    fetch(server, credential, "categories")
}

/// Follows a feed, filed under an existing category.
pub fn subscribe(server: &str, credential: &str, feed_url: &str, category: u64) -> Task {
    let mut body = String::from("{\"feed_url\":");
    kobo_json::escape_into(feed_url, &mut body);
    let _ = write!(body, ",\"category_id\":{category}}}");
    Task::Post {
        url: endpoint(server, "feeds"),
        body,
        content_type: "application/json".into(),
        credential: Some(token(credential)),
        headers: Vec::new(),
        max_bytes: 16 * 1024,
    }
}

/// A change to one article, sent as the state it should end in.
///
/// Every change this application makes is an assignment rather than a toggle,
/// and that is deliberate. The runtime sends an update exactly once and never
/// replays it, because a reply can go missing after the change has been
/// applied. An assignment survives that: sending it again asks for the same
/// end state, so a queue of unsent changes can be flushed after a train
/// tunnel without anyone having to work out what arrived. A star sent as
/// "flip it" could not be, which is why the toggle endpoint is not used here.
fn update(server: &str, credential: &str, body: String) -> Task {
    Task::Update {
        method: UpdateMethod::Put,
        url: endpoint(server, "entries"),
        body,
        content_type: "application/json".into(),
        credential: Some(token(credential)),
        headers: Vec::new(),
        max_bytes: 16 * 1024,
    }
}

/// Sets the read state of one article.
pub fn set_status(server: &str, credential: &str, id: u64, status: Status) -> Task {
    update(
        server,
        credential,
        format!(
            "{{\"entry_ids\":[{id}],\"status\":\"{}\"}}",
            status.wire_name()
        ),
    )
}

/// Sets whether one article is starred.
pub fn set_starred(server: &str, credential: &str, id: u64, starred: bool) -> Task {
    update(
        server,
        credential,
        format!("{{\"entry_ids\":[{id}],\"starred\":{starred}}}"),
    )
}
pub const MAX_RESPONSE: usize = 768 * 1024;
pub const MAX_ARTICLES: usize = 100;
pub const MAX_BODY: usize = 256 * 1024;
pub const MAX_CATEGORIES: usize = 64;
/// The largest integer a JSON number carries exactly. An identifier past it
/// has already been rounded, and a rounded identifier names another article.
const SAFE_INTEGER: u64 = 9_007_199_254_740_991;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Status {
    Unread,
    Read,
    Removed,
}

impl Status {
    pub const fn wire_name(self) -> &'static str {
        match self {
            Self::Unread => "unread",
            Self::Read => "read",
            Self::Removed => "removed",
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParseError {
    TooLarge,
    Malformed,
    InvalidEntry,
}
impl ParseError {
    pub const fn message(self) -> &'static str {
        match self {
            Self::TooLarge => "The response is too large. Your previous articles are kept.",
            Self::Malformed => {
                "The server did not return a valid Miniflux response. Check its address."
            }
            Self::InvalidEntry => {
                "The response contains invalid article details. Your previous articles are kept."
            }
        }
    }
}
pub fn parse_entries(bytes: &[u8]) -> Result<Vec<Article>, ParseError> {
    if bytes.len() > MAX_RESPONSE {
        return Err(ParseError::TooLarge);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| ParseError::Malformed)?;
    let value = kobo_json::parse(text).map_err(|_| ParseError::Malformed)?;
    if !unique_fields(&value) {
        return Err(ParseError::Malformed);
    }
    let entries = value
        .get("entries")
        .and_then(Value::as_array)
        .ok_or(ParseError::Malformed)?;
    if entries.len() > MAX_ARTICLES {
        return Err(ParseError::TooLarge);
    }
    let mut articles = Vec::with_capacity(entries.len());
    for entry in entries {
        let article = article_from(entry)?;
        if articles.iter().any(|seen: &Article| seen.id == article.id) {
            return Err(ParseError::InvalidEntry);
        }
        articles.push(article);
    }
    Ok(articles)
}

/// The article body Miniflux fetched from the original page.
pub fn parse_content(bytes: &[u8]) -> Result<String, ParseError> {
    if bytes.len() > MAX_RESPONSE {
        return Err(ParseError::TooLarge);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| ParseError::Malformed)?;
    let value = kobo_json::parse(text).map_err(|_| ParseError::Malformed)?;
    if !unique_fields(&value) {
        return Err(ParseError::Malformed);
    }
    let content = value
        .get("content")
        .and_then(Value::as_str)
        .ok_or(ParseError::Malformed)?;
    if content.len() > MAX_BODY {
        return Err(ParseError::TooLarge);
    }
    Ok(content.to_owned())
}

/// The categories a feed may be filed under, nearest to first.
pub fn parse_categories(bytes: &[u8]) -> Result<Vec<(u64, String)>, ParseError> {
    if bytes.len() > MAX_RESPONSE {
        return Err(ParseError::TooLarge);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| ParseError::Malformed)?;
    let value = kobo_json::parse(text).map_err(|_| ParseError::Malformed)?;
    let listed = value.as_array().ok_or(ParseError::Malformed)?;
    let mut categories = Vec::with_capacity(listed.len().min(MAX_CATEGORIES));
    for category in listed.iter().take(MAX_CATEGORIES) {
        if !unique_fields(category) {
            return Err(ParseError::InvalidEntry);
        }
        let id = category
            .get("id")
            .and_then(Value::as_i64)
            .and_then(|id| u64::try_from(id).ok())
            .filter(|id| (1..=SAFE_INTEGER).contains(id))
            .ok_or(ParseError::InvalidEntry)?;
        let title = category.get("title").and_then(Value::as_str);
        categories.push((id, nonempty(title, "Feeds")));
    }
    Ok(categories)
}

/// The batch, written back out the way Miniflux sent it.
///
/// A full article fetched after the sync is part of what the reader now has,
/// and the saved copy is the only thing a restart opens. Re-encoding the batch
/// keeps one file as the whole truth rather than adding a second store of
/// upgraded bodies that could disagree with it.
pub fn encode_entries(articles: &[Article]) -> Vec<u8> {
    let mut out = String::from("{\"entries\":[");
    for (index, article) in articles.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        let _ = write!(out, "{{\"id\":{},\"title\":", article.id);
        kobo_json::escape_into(&article.title, &mut out);
        out.push_str(",\"content\":");
        kobo_json::escape_into(&article.content, &mut out);
        out.push_str(",\"url\":");
        kobo_json::escape_into(&article.url, &mut out);
        let _ = write!(
            out,
            ",\"starred\":{},\"status\":\"{}\",\"feed\":{{\"title\":",
            article.starred,
            article.status.wire_name()
        );
        kobo_json::escape_into(&article.feed, &mut out);
        out.push_str("}}");
    }
    out.push_str("]}");
    out.into_bytes()
}

fn article_from(entry: &Value) -> Result<Article, ParseError> {
    {
        if !unique_fields(entry) {
            return Err(ParseError::InvalidEntry);
        }
        let id = entry
            .get("id")
            .and_then(Value::as_i64)
            .and_then(|id| u64::try_from(id).ok())
            .filter(|id| (1..=SAFE_INTEGER).contains(id))
            .ok_or(ParseError::InvalidEntry)?;
        let title = entry
            .get("title")
            .and_then(Value::as_str)
            .ok_or(ParseError::InvalidEntry)?;
        let content = entry
            .get("content")
            .and_then(Value::as_str)
            .ok_or(ParseError::InvalidEntry)?;
        let url = match entry.get("url") {
            None => "",
            Some(Value::String(url)) => url,
            _ => return Err(ParseError::InvalidEntry),
        };
        let feed = entry
            .get("feed")
            .and_then(|f| f.get("title"))
            .and_then(Value::as_str);
        if title.len() > 4096
            || content.len() > MAX_BODY
            || url.len() > 4096
            || feed.is_some_and(|f| f.len() > 4096)
        {
            return Err(ParseError::TooLarge);
        }
        let status = match entry.get("status") {
            None => Status::Unread,
            Some(Value::String(status)) if status == "unread" => Status::Unread,
            Some(Value::String(status)) if status == "read" => Status::Read,
            Some(Value::String(status)) if status == "removed" => Status::Removed,
            _ => return Err(ParseError::InvalidEntry),
        };
        let starred = entry
            .get("starred")
            .and_then(Value::as_bool)
            .ok_or(ParseError::InvalidEntry)?;
        Ok(Article {
            id,
            title: nonempty(Some(title), "Untitled"),
            feed: nonempty(feed, "Feed"),
            content: content.into(),
            starred,
            url: url.into(),
            status,
        })
    }
}

fn unique_fields(value: &Value) -> bool {
    let Value::Object(fields) = value else {
        return false;
    };
    let mut names = std::collections::HashSet::with_capacity(fields.len());
    fields.iter().all(|(name, _)| names.insert(name))
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
    fn every_request_carries_a_runtime_token_and_a_bounded_response() {
        let Task::Fetch {
            url,
            credential,
            max_bytes,
            ..
        } = entries("https://flux.example", "miniflux", Part::Unread, 60)
        else {
            panic!("the batch is fetched")
        };
        assert!(
            url.ends_with("/v1/entries?limit=60&status=unread&order=published_at&direction=desc")
        );
        assert_eq!(max_bytes as usize, MAX_RESPONSE);
        let named = Some(Credential::in_header("miniflux", "X-Auth-Token"));
        assert_eq!(credential, named);
        for task in [
            entries("https://flux.example", "miniflux", Part::Starred, 20),
            entries("https://flux.example", "miniflux", Part::Read, 20),
            full_content("https://flux.example", "miniflux", 7),
            categories("https://flux.example", "miniflux"),
            subscribe("https://flux.example", "miniflux", "https://feed.test", 3),
            set_status("https://flux.example", "miniflux", 7, Status::Read),
            set_starred("https://flux.example", "miniflux", 7, true),
        ] {
            assert!(task.is_sendable(), "{task:?} does not fit the wire");
            let carried = match &task {
                Task::Fetch { credential, .. }
                | Task::Post { credential, .. }
                | Task::Update { credential, .. } => credential.clone(),
                _ => panic!("only requests are made"),
            };
            assert_eq!(carried, named, "{task:?} named no credential");
        }
    }

    #[test]
    fn every_change_names_one_article_and_the_state_it_should_end_in() {
        for (task, expected) in [
            (
                set_status("https://flux.example", "miniflux", 7, Status::Removed),
                r#"{"entry_ids":[7],"status":"removed"}"#,
            ),
            (
                set_starred("https://flux.example", "miniflux", 7, false),
                r#"{"entry_ids":[7],"starred":false}"#,
            ),
        ] {
            let Task::Update { url, body, .. } = task else {
                panic!("a change is an update")
            };
            assert!(url.ends_with("/v1/entries"), "{url}");
            assert_eq!(body, expected);
        }
    }

    #[test]
    fn a_chosen_feed_is_sent_as_escaped_json_under_its_category() {
        let Task::Post { url, body, .. } = subscribe(
            "https://flux.example/",
            "miniflux",
            "https://feed.test/a\"b",
            3,
        ) else {
            panic!("a feed is posted")
        };
        assert!(url.ends_with("/v1/feeds"));
        let parsed = kobo_json::parse(&body).expect("the body is JSON");
        assert_eq!(
            parsed.get("feed_url").and_then(Value::as_str),
            Some("https://feed.test/a\"b")
        );
        assert_eq!(parsed.get("category_id").and_then(Value::as_i64), Some(3));
    }

    #[test]
    fn a_fetched_body_and_a_category_list_are_read_as_strictly_as_a_batch() {
        assert_eq!(
            parse_content(br#"{"content":"<p>The page.</p>"}"#),
            Ok("<p>The page.</p>".to_owned())
        );
        assert!(parse_content(br#"{"content":12}"#).is_err());
        assert_eq!(
            parse_categories(br#"[{"id":3,"title":"News"},{"id":4,"title":""}]"#),
            Ok(vec![(3, "News".to_owned()), (4, "Feeds".to_owned())])
        );
        assert!(parse_categories(br#"{"id":3}"#).is_err());
    }

    #[test]
    fn a_batch_written_back_out_reads_as_the_same_articles() {
        let articles = parse_entries(
            br#"{"entries":[{"id":7,"title":"A \"quoted\" story","content":"<p>Body</p>","starred":true,"status":"read","url":"https://example.test/a","feed":{"title":"Journal"}},
                          {"id":8,"title":"Another","content":"","starred":false,"status":"unread"}]}"#,
        )
        .unwrap();
        let encoded = encode_entries(&articles);
        assert_eq!(parse_entries(&encoded), Ok(articles));
    }
    #[test]
    fn errors_are_distinct_from_an_empty_inbox_and_html_is_retained() {
        assert_eq!(parse_entries(br#"{"entries":[]}"#), Ok(Vec::new()));
        assert_eq!(
            parse_entries(br#"{"entries":[],"entries":[]}"#),
            Err(ParseError::Malformed)
        );
        for bytes in [
            b"<html>Sign in</html>".as_slice(),
            b"{}",
            b"{\"entries\":{}}",
            b"{\"entries\":[{}]}",
            b"\xff",
        ] {
            assert!(parse_entries(bytes).is_err());
        }
        let article = br#"{"id":8,"title":"Long read","content":"<h1>Heading</h1><p>First <em>paragraph</em>.</p>","starred":false,"status":"read","url":"https://example.test/story"}"#;
        let single = format!(
            "{{\"entries\":[{}]}}",
            std::str::from_utf8(article).unwrap()
        );
        let parsed = parse_entries(single.as_bytes()).unwrap();
        assert!(parsed[0].content.contains("<em>paragraph</em>"));
        assert_eq!(parsed[0].status, Status::Read);
        let duplicate = format!(
            "{{\"entries\":[{0},{0}]}}",
            std::str::from_utf8(article).unwrap()
        );
        assert_eq!(
            parse_entries(duplicate.as_bytes()),
            Err(ParseError::InvalidEntry)
        );
        assert_eq!(
            parse_entries(&vec![b' '; MAX_RESPONSE + 1]),
            Err(ParseError::TooLarge)
        );
    }
    #[test]
    fn invalid_status_url_and_imprecise_ids_cannot_become_pending_actions() {
        for entry in [
            r#"{"id":7,"id":8,"title":"Story","content":"Text","starred":false}"#,
            r#"{"id":7,"title":"Story","content":"Text","starred":false,"status":false}"#,
            r#"{"id":7,"title":"Story","content":"Text","starred":false,"status":null}"#,
            r#"{"id":7,"title":"Story","content":"Text","starred":false,"url":23}"#,
            r#"{"id":9007199254740993,"title":"Story","content":"Text","starred":false}"#,
        ] {
            let body = format!("{{\"entries\":[{entry}]}}");
            assert_eq!(
                parse_entries(body.as_bytes()),
                Err(ParseError::InvalidEntry)
            );
        }
    }
    #[test]
    fn parses_a_batch_response() {
        let values = parse_entries(br#"{"entries":[{"id":1,"title":"News","feed":{"title":"Paper"},"content":"Body","starred":true}]}"#).unwrap();
        assert_eq!(
            values[0],
            Article {
                id: 1,
                title: "News".into(),
                feed: "Paper".into(),
                content: "Body".into(),
                starred: true,
                url: String::new(),
                status: Status::Unread,
            }
        );
    }
}

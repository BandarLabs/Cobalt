//! The reviewed gateway contract. A gateway answers two routes under its
//! base address: paginated letter reads, and reply posts that carry a
//! client-chosen idempotency key so a retried send can never deliver twice.
use kobo_sdk::{Credential, Task};

pub const SECRET: &str = "hermes-post";
// Five rows leave room for the notice, the pending count and the More
// button on the smallest panel.
pub const PER_PAGE: usize = 5;
const MAX_REPLY: usize = 32 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Letter {
    pub id: String,
    pub title: String,
    pub body: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReplyState {
    Queued,
    Sending,
    Delivered,
    Rejected,
}

impl ReplyState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Queued => "Queued for the next connection",
            Self::Sending => "Sending",
            Self::Delivered => "Delivered",
            Self::Rejected => "Rejected by the gateway",
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Sending => "sending",
            Self::Delivered => "delivered",
            Self::Rejected => "rejected",
        }
    }
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "queued" => Self::Queued,
            "sending" => Self::Sending,
            "delivered" => Self::Delivered,
            "rejected" => Self::Rejected,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Reply {
    pub id: String,
    pub letter_id: String,
    pub body: String,
    pub state: ReplyState,
}

impl Reply {
    pub fn new(gateway: &str, letter_id: &str, body: &str) -> Option<Self> {
        if body.trim().is_empty() || body.len() > MAX_REPLY || letter_id.len() > 128 {
            return None;
        }
        Some(Self {
            id: kobo_net::sha256::hex_digest(
                format!("post-reply-v1\n{gateway}\n{letter_id}\n{body}").as_bytes(),
            ),
            letter_id: letter_id.to_owned(),
            body: body.to_owned(),
            state: ReplyState::Queued,
        })
    }
}

pub fn endpoint(gateway: &str, route: &str) -> String {
    format!(
        "{}/{}",
        gateway.trim_end_matches('/'),
        route.trim_start_matches('/')
    )
}

pub fn letters_page(gateway: &str, page: usize) -> Task {
    Task::Fetch {
        url: endpoint(
            gateway,
            &format!("/letters?page={page}&per_page={PER_PAGE}"),
        ),
        offset: 0,
        max_bytes: 512 * 1024,
        credential: Some(Credential::bearer(SECRET)),
        headers: Vec::new(),
    }
}

pub fn send_reply(gateway: &str, reply: &Reply) -> Task {
    Task::Post {
        url: endpoint(gateway, "/replies"),
        body: kobo_json::ObjectBuilder::new()
            .set("letter_id", reply.letter_id.as_str())
            .set("body", reply.body.as_str())
            .set("reply_id", reply.id.as_str())
            .build()
            .to_json(),
        content_type: "application/json".to_owned(),
        credential: Some(Credential::bearer(SECRET)),
        headers: Vec::new(),
        max_bytes: 4096,
    }
}

/// One page of letters. A malformed page is distinguished from a valid empty
/// one so a bad reply can never wipe the cached inbox.
pub fn page(bytes: &[u8]) -> Option<(usize, Vec<Letter>)> {
    let text = std::str::from_utf8(bytes).ok()?;
    let root = kobo_json::parse(text).ok()?;
    let total = usize::try_from(root.get("total")?.as_i64()?).ok()?;
    let items = root.get("items")?.as_array()?;
    let mut letters = Vec::new();
    let mut seen = Vec::new();
    for item in items {
        let letter = Letter {
            id: item.get("id")?.as_str()?.to_owned(),
            title: item.get("title")?.as_str()?.to_owned(),
            body: item.get("body")?.as_str()?.to_owned(),
        };
        if letter.id.is_empty() || seen.contains(&letter.id) {
            return None;
        }
        seen.push(letter.id.clone());
        letters.push(letter);
    }
    Some((total, letters))
}

/// The gateway's answer to a reply: accepted, or a duplicate of one it
/// already has (a retried send after an uncertain connection is still a
/// success). Anything else is malformed.
pub fn reply_outcome(bytes: &[u8]) -> Option<bool> {
    let text = std::str::from_utf8(bytes).ok()?;
    let root = kobo_json::parse(text).ok()?;
    match root.get("status")?.as_str()? {
        "accepted" => Some(false),
        "duplicate" => Some(true),
        _ => None,
    }
}

/// Store encoding: one letter per line, fields tab-separated, newlines in
/// bodies escaped. The first line fixes the format.
pub fn encode_cache(letters: &[Letter], places: &[(String, usize)]) -> Vec<u8> {
    let mut text = String::from("post-cache-v1\n");
    for letter in letters {
        let place = places
            .iter()
            .find(|(id, _)| *id == letter.id)
            .map_or(0, |(_, place)| *place);
        text.push_str(&format!(
            "{}\t{}\t{place}\t{}\n",
            field(&letter.id),
            field(&letter.title),
            field(&letter.body)
        ));
    }
    text.into_bytes()
}

pub type Places = Vec<(String, usize)>;

pub fn decode_cache(bytes: &[u8]) -> Option<(Vec<Letter>, Places)> {
    let text = std::str::from_utf8(bytes).ok()?;
    let lines = text.strip_prefix("post-cache-v1\n")?.lines();
    let mut letters = Vec::new();
    let mut places = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(4, '\t');
        let id = unfield(parts.next()?);
        let title = unfield(parts.next()?);
        let place = parts.next()?.parse().ok()?;
        let body = unfield(parts.next()?);
        places.push((id.clone(), place));
        letters.push(Letter { id, title, body });
    }
    Some((letters, places))
}

pub fn encode_drafts(drafts: &[(String, String)]) -> Vec<u8> {
    let mut text = String::from("post-drafts-v1\n");
    for (letter, body) in drafts {
        text.push_str(&format!("{}\t{}\n", field(letter), field(body)));
    }
    text.into_bytes()
}

pub fn decode_drafts(bytes: &[u8]) -> Option<Vec<(String, String)>> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut drafts = Vec::new();
    for line in text.strip_prefix("post-drafts-v1\n")?.lines() {
        if line.is_empty() {
            continue;
        }
        let (letter, body) = line.split_once('\t')?;
        drafts.push((unfield(letter), unfield(body)));
    }
    Some(drafts)
}

pub fn encode_outbox(replies: &[Reply]) -> Vec<u8> {
    let mut text = String::from("post-outbox-v1\n");
    for reply in replies {
        text.push_str(&format!(
            "{}\t{}\t{}\t{}\n",
            reply.id,
            field(&reply.letter_id),
            reply.state.name(),
            field(&reply.body)
        ));
    }
    text.into_bytes()
}

pub fn decode_outbox(bytes: &[u8]) -> Option<Vec<Reply>> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut replies = Vec::new();
    for line in text.strip_prefix("post-outbox-v1\n")?.lines() {
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(4, '\t');
        let id = parts.next()?.to_owned();
        let letter_id = unfield(parts.next()?);
        // A send interrupted mid-flight is unknown at the server: it rejoins
        // the queue, and the idempotency key keeps a repeat from doubling.
        let state = match ReplyState::parse(parts.next()?) {
            Some(ReplyState::Sending | ReplyState::Queued) => ReplyState::Queued,
            Some(state) => state,
            None => return None,
        };
        let body = unfield(parts.next()?);
        replies.push(Reply {
            id,
            letter_id,
            body,
            state,
        });
    }
    Some(replies)
}

fn field(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\t', "\\t")
}

fn unfield(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('\\') => out.push('\\'),
                _ => return out,
            }
        } else {
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_secret_is_named_not_embedded() {
        let Task::Fetch {
            credential: Some(secret),
            url,
            ..
        } = letters_page("https://gateway.example", 1)
        else {
            panic!("fetch")
        };
        assert_eq!(secret.secret, SECRET);
        assert!(!url.contains(SECRET));
        assert!(url.ends_with("/letters?page=1&per_page=5"));
    }

    #[test]
    fn pages_parse_and_reject_duplicates() {
        let (total, letters) = page(
            br#"{"total":3,"items":[{"id":"dawn","title":"Morning note","body":"Tea first."}]}"#,
        )
        .unwrap();
        assert_eq!(total, 3);
        assert_eq!(letters[0].title, "Morning note");
        assert!(page(br#"{"total":2,"items":[{"id":"a","title":"t","body":"b"},{"id":"a","title":"t","body":"b"}]}"#)
            .is_none());
        assert!(page(b"not json").is_none());
        assert!(page(br#"{"total":0,"items":[]}"#).is_some_and(|(t, i)| t == 0 && i.is_empty()));
    }

    #[test]
    fn reply_key_is_stable_for_the_same_letter_and_body() {
        let first = Reply::new("https://gateway.example", "dawn", "Tea first.").unwrap();
        let again = Reply::new("https://gateway.example", "dawn", "Tea first.").unwrap();
        let edited = Reply::new("https://gateway.example", "dawn", "Coffee first.").unwrap();
        let elsewhere = Reply::new("https://other.example", "dawn", "Tea first.").unwrap();
        assert_eq!(first.id, again.id);
        assert_eq!(first.id.len(), 64);
        assert_ne!(first.id, edited.id);
        assert_ne!(first.id, elsewhere.id);
        assert!(Reply::new("https://gateway.example", "dawn", "  ").is_none());
    }

    #[test]
    fn reply_outcome_reads_accepted_and_duplicate() {
        assert_eq!(reply_outcome(br#"{"status":"accepted"}"#), Some(false));
        assert_eq!(reply_outcome(br#"{"status":"duplicate"}"#), Some(true));
        assert_eq!(reply_outcome(br#"{"status":"pending"}"#), None);
    }

    #[test]
    fn store_codecs_round_trip_and_a_sending_reply_requeues() {
        let letters = vec![Letter {
            id: "dawn".into(),
            title: "Morning\tnote".into(),
            body: "Tea first.\n\nThen write.".into(),
        }];
        let (back, places) = decode_cache(&encode_cache(&letters, &[("dawn".into(), 12)])).unwrap();
        assert_eq!(back, letters);
        assert_eq!(places, vec![("dawn".to_string(), 12)]);

        let drafts = vec![("dawn".to_string(), "Line one\nLine two".to_string())];
        assert_eq!(decode_drafts(&encode_drafts(&drafts)), Some(drafts));

        let mut reply = Reply::new("https://gateway.example", "dawn", "Tea first.").unwrap();
        reply.state = ReplyState::Sending;
        let restored = decode_outbox(&encode_outbox(&[reply.clone()])).unwrap();
        assert_eq!(restored[0].state, ReplyState::Queued);
        assert_eq!(restored[0].id, reply.id);
        assert_eq!(restored[0].body, "Tea first.");
    }
}

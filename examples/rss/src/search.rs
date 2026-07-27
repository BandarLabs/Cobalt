//! Asking [Feedsearch](https://feedsearch.dev) what feeds an address has.
//!
//! One request, one JSON array, ranked by how well each feed matches what was
//! typed. The service's terms ask for a visible attribution wherever its
//! results appear, which the two screens that show them carry.

use kobo_json::Value;

/// The service's search endpoint.
const ENDPOINT: &str = "https://feedsearch.dev/api/v1/search";

/// The most results to keep.
///
/// A large site can publish a feed per section and answer with forty. Past a
/// screen or two nobody is choosing any more, they are scrolling, and the
/// ranking has already put the right answer at the top.
const MAX_RESULTS: usize = 12;

/// The longest title or summary kept, in characters.
const MAX_TEXT: usize = 200;

/// One feed the service found.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Found {
    /// The feed's own address, which is what gets subscribed to.
    pub url: String,
    pub title: String,
    /// The site the feed belongs to.
    pub site: String,
    /// A line about the feed, for under its name.
    pub summary: String,
}

/// The address to fetch for a typed search.
///
/// The favicon is declined explicitly. It is off by default today, but it
/// arrives as a base64 data URI inside the JSON — tens of kilobytes per result
/// for an image this application has nowhere to draw — and a default is a
/// thing that can change.
#[must_use]
pub fn request(typed: &str) -> String {
    format!("{ENDPOINT}?url={}&favicon=false", encode(typed.trim()))
}

/// Percent-encodes a query value.
///
/// Everything outside the unreserved set goes, rather than a list of the
/// characters known to cause trouble. A reader pastes what they have, and a
/// space or an accent in an address should come back as a search that failed,
/// not as a malformed request.
fn encode(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push('%');
            out.push(char::from(HEX[usize::from(byte >> 4)]));
            out.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
    }
    out
}

/// Reads the service's answer.
///
/// Anything that is not an array of objects with a `url` is no results rather
/// than an error: the screen already has to say "nothing found", and a reader
/// cannot act on the difference between a site with no feeds and a service
/// having a bad afternoon.
#[must_use]
pub fn results(bytes: &[u8]) -> Vec<Found> {
    let text = String::from_utf8_lossy(bytes);
    let Ok(value) = kobo_json::parse(&text) else {
        return Vec::new();
    };
    let Some(entries) = value.as_array() else {
        return Vec::new();
    };
    let mut found: Vec<(f64, Found)> = entries
        .iter()
        .filter_map(|entry| {
            let url = entry.get("url").and_then(Value::as_str)?.trim();
            if url.is_empty() {
                return None;
            }
            let site = entry
                .get("site_url")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let title = pick(
                [
                    entry.get("title").and_then(Value::as_str),
                    entry.get("site_name").and_then(Value::as_str),
                ],
                url,
            );
            let score = entry.get("score").and_then(Value::as_f64).unwrap_or(0.0);
            Some((
                score,
                Found {
                    url: url.to_owned(),
                    title,
                    site,
                    summary: summarise(entry),
                },
            ))
        })
        .collect();
    // Highest score first. The service computes it from how closely the feed's
    // address matches what was asked for, which is a better guess at what
    // somebody meant than the order a crawler happened to find things in.
    found.sort_by(|left, right| right.0.total_cmp(&left.0));
    found.truncate(MAX_RESULTS);
    found.into_iter().map(|(_, entry)| entry).collect()
}

/// The first of several fields that has anything in it.
fn pick<const N: usize>(candidates: [Option<&str>; N], fallback: &str) -> String {
    for candidate in candidates {
        let text = candidate.unwrap_or_default().trim();
        if !text.is_empty() {
            return clamp(text);
        }
    }
    clamp(fallback)
}

/// The line under a result's name.
///
/// Built from what the feed is rather than what it says, because every result
/// for one site tends to carry the same description, and a screen of identical
/// sentences is a screen nobody can choose from. How many articles it holds
/// and whether it is a podcast are the things that differ.
fn summarise(entry: &Value) -> String {
    let mut parts: Vec<String> = Vec::new();
    if entry.get("is_podcast").and_then(Value::as_bool) == Some(true) {
        parts.push("Podcast".to_owned());
    }
    match entry.get("item_count").and_then(Value::as_i64) {
        Some(1) => parts.push("1 article".to_owned()),
        Some(count) if count > 1 => parts.push(format!("{count} articles")),
        _ => {}
    }
    let description = entry
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    if parts.is_empty() && !description.is_empty() {
        return clamp(description);
    }
    if parts.is_empty() {
        // Nothing else to say, so say where it is. Two feeds from one site are
        // told apart by their path and nothing else.
        return clamp(entry.get("url").and_then(Value::as_str).unwrap_or_default());
    }
    clamp(&parts.join(" \u{00b7} "))
}

/// Cuts to a character count, never inside a character.
fn clamp(text: &str) -> String {
    match text.char_indices().nth(MAX_TEXT) {
        Some((index, _)) => text[..index].trim_end().to_owned(),
        None => text.trim().to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::{encode, request, results, MAX_RESULTS};

    const ANSWER: &str = r#"[
        {"url":"http://feeds.arstechnica.com/arstechnica/index","title":"Ars Technica",
         "site_url":"https://arstechnica.com/","site_name":"Ars Technica",
         "description":"Serving the Technologist.","item_count":20,"score":27,
         "is_podcast":false,"version":"rss20"},
        {"url":"https://arstechnica.com/feed/","title":"","site_url":"https://arstechnica.com/",
         "site_name":"Ars Technica","item_count":1,"score":40,"is_podcast":true}
    ]"#;

    #[test]
    fn a_search_asks_for_the_address_and_declines_the_favicon() {
        assert_eq!(
            request(" arstechnica.com "),
            "https://feedsearch.dev/api/v1/search?url=arstechnica.com&favicon=false"
        );
    }

    #[test]
    fn anything_that_is_not_plain_is_encoded_rather_than_sent() {
        assert_eq!(
            encode("https://a.example/x y"),
            "https%3A%2F%2Fa.example%2Fx%20y"
        );
        assert_eq!(encode("caf\u{e9}"), "caf%C3%A9");
        assert_eq!(encode("a-b_c.d~e"), "a-b_c.d~e");
        assert_eq!(encode("&url=evil"), "%26url%3Devil");
    }

    #[test]
    fn results_come_back_best_first() {
        let found = results(ANSWER.as_bytes());
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].url, "https://arstechnica.com/feed/");
        assert_eq!(
            found[1].url,
            "http://feeds.arstechnica.com/arstechnica/index"
        );
    }

    #[test]
    fn a_feed_with_no_title_of_its_own_borrows_the_sites() {
        let found = results(ANSWER.as_bytes());
        assert_eq!(found[0].title, "Ars Technica");
        assert_eq!(found[0].site, "https://arstechnica.com/");
    }

    #[test]
    fn a_summary_says_what_the_feed_is_rather_than_repeating_the_site() {
        let found = results(ANSWER.as_bytes());
        assert_eq!(found[0].summary, "Podcast \u{b7} 1 article");
        assert_eq!(found[1].summary, "20 articles");
    }

    #[test]
    fn a_feed_that_describes_nothing_still_gets_a_line() {
        let found = results(br#"[{"url":"https://a.example/rss"}]"#);
        assert_eq!(found[0].summary, "https://a.example/rss");
        assert_eq!(found[0].title, "https://a.example/rss");
    }

    #[test]
    fn a_bare_description_is_used_when_there_is_nothing_countable() {
        let found = results(br#"[{"url":"https://a.example/rss","description":"A weblog."}]"#);
        assert_eq!(found[0].summary, "A weblog.");
    }

    #[test]
    fn an_answer_that_is_not_results_is_no_results_rather_than_a_failure() {
        assert!(results(b"").is_empty());
        assert!(results(b"not json").is_empty());
        assert!(results(b"{}").is_empty());
        assert!(results(b"[]").is_empty());
        assert!(results(br#"[{"title":"no address"}]"#).is_empty());
        assert!(results(br#"[{"url":"   "}]"#).is_empty());
        assert!(results(br#"[null, 3, "text"]"#).is_empty());
    }

    #[test]
    fn a_site_that_publishes_a_feed_per_section_is_cut_to_a_choosable_list() {
        let entries: Vec<String> = (0..40)
            .map(|index| format!(r#"{{"url":"https://a.example/{index}","score":{index}}}"#))
            .collect();
        let found = results(format!("[{}]", entries.join(",")).as_bytes());
        assert_eq!(found.len(), MAX_RESULTS);
        assert_eq!(found[0].url, "https://a.example/39");
    }
}

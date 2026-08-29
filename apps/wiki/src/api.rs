//! Wikipedia's own `action=query` API: search, a random article, and one
//! article's plain-text extract.
//!
//! No API key, no rate-limit tier to reason about -- this is the same
//! anonymous endpoint `en.wikipedia.org` itself calls. `explaintext=1` asks
//! `MediaWiki` to strip wiki markup server-side, so the panel is handed prose
//! rather than a wikitext parser's homework.

use kobo_json::Value;

const API: &str = "https://en.wikipedia.org/w/api.php";

/// The most search results kept. A screen or two of titles; past that a
/// reader is scrolling, not choosing.
const MAX_RESULTS: usize = 20;

/// The address that asks for titles matching what was typed.
#[must_use]
pub fn search_url(query: &str) -> String {
    format!(
        "{API}?action=query&list=search&format=json&srlimit={MAX_RESULTS}&srsearch={}",
        encode(query.trim())
    )
}

/// The address that asks for one article title, chosen at random from the
/// encyclopedia's main namespace.
#[must_use]
pub fn random_url() -> String {
    format!("{API}?action=query&list=random&format=json&rnnamespace=0&rnlimit=1")
}

/// The address that asks for one article's plain-text body.
#[must_use]
pub fn extract_url(title: &str) -> String {
    format!(
        "{API}?action=query&format=json&prop=extracts&explaintext=1&exsectionformat=plain&titles={}",
        encode(title)
    )
}

/// Percent-encodes a query value.
///
/// Everything outside the unreserved set goes, rather than a list of the
/// characters known to cause trouble. A title with an accent or an ampersand
/// (`AT&T`, `Motorhead`) should come back as a search that failed, not as a
/// malformed request.
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

/// One title the search found, with the line `MediaWiki` drew under it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Hit {
    pub title: String,
    /// A snippet of the article with the match marked, HTML stripped.
    pub snippet: String,
}

/// Reads a search answer.
///
/// Anything that is not `query.search`, an array of objects with a `title`,
/// is no results rather than a failure: the screen already has to say
/// "nothing found", and a reader cannot act on the difference between that
/// and the service having a bad afternoon.
#[must_use]
pub fn search_results(bytes: &[u8]) -> Vec<Hit> {
    let text = String::from_utf8_lossy(bytes);
    let Ok(value) = kobo_json::parse(&text) else {
        return Vec::new();
    };
    let Some(entries) = value.get("query").and_then(|query| query.get("search")) else {
        return Vec::new();
    };
    let Some(entries) = entries.as_array() else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| {
            let title = entry.get("title").and_then(Value::as_str)?.trim();
            if title.is_empty() {
                return None;
            }
            let snippet = entry
                .get("snippet")
                .and_then(Value::as_str)
                .map(strip_html)
                .unwrap_or_default();
            Some(Hit {
                title: title.to_owned(),
                snippet,
            })
        })
        .take(MAX_RESULTS)
        .collect()
}

/// The title a random-article answer named, if it named one.
#[must_use]
pub fn random_title(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    let value = kobo_json::parse(&text).ok()?;
    let title = value
        .get("query")?
        .get("random")?
        .index(0)?
        .get("title")?
        .as_str()?
        .trim();
    (!title.is_empty()).then(|| title.to_owned())
}

/// One article's body, read back from an extract answer.
#[must_use]
pub fn extract(bytes: &[u8]) -> Option<(String, String)> {
    let text = String::from_utf8_lossy(bytes);
    let value = kobo_json::parse(&text).ok()?;
    let pages = value.get("query")?.get("pages")?;
    // Keyed by page id, and there is exactly one page in this answer because
    // exactly one title was asked for. The key itself is meaningless (and is
    // literally `"-1"` for a title that does not exist), so the first and
    // only field is taken rather than searched for by name.
    let Value::Object(fields) = pages else {
        return None;
    };
    let (_, page) = fields.first()?;
    if page.get("missing").is_some() {
        return None;
    }
    let title = page.get("title").and_then(Value::as_str)?.trim();
    let body = page.get("extract").and_then(Value::as_str)?.trim();
    if title.is_empty() || body.is_empty() {
        return None;
    }
    Some((title.to_owned(), body.to_owned()))
}

/// Drops HTML tags and decodes the handful of entities `MediaWiki`'s search
/// snippets actually carry.
///
/// A snippet is `<span class="searchmatch">` around the matched word and
/// occasionally `&quot;` or `&amp;` from the source text. It is never a full
/// document, so this is not a general HTML reader: anything that looks like a
/// tag is dropped outright rather than interpreted.
fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if in_tag => {}
            _ => out.push(ch),
        }
    }
    out.replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

#[cfg(test)]
mod tests {
    use super::{
        encode, extract, extract_url, random_title, random_url, search_results, search_url,
    };

    #[test]
    fn a_search_asks_for_titles_matching_what_was_typed() {
        assert_eq!(
            search_url(" Albert Einstein "),
            "https://en.wikipedia.org/w/api.php?action=query&list=search&format=json&srlimit=20&srsearch=Albert%20Einstein"
        );
    }

    #[test]
    fn a_title_with_reserved_characters_is_encoded_rather_than_sent() {
        assert_eq!(encode("AT&T"), "AT%26T");
        assert_eq!(encode("caf\u{e9}"), "caf%C3%A9");
        assert_eq!(encode("a-b_c.d~e"), "a-b_c.d~e");
    }

    #[test]
    fn random_asks_the_main_namespace_for_one_title() {
        assert_eq!(
            random_url(),
            "https://en.wikipedia.org/w/api.php?action=query&list=random&format=json&rnnamespace=0&rnlimit=1"
        );
    }

    #[test]
    fn an_extract_is_asked_for_by_exact_title() {
        assert_eq!(
            extract_url("E=mc²"),
            "https://en.wikipedia.org/w/api.php?action=query&format=json&prop=extracts&explaintext=1&exsectionformat=plain&titles=E%3Dmc%C2%B2"
        );
    }

    const SEARCH: &str = r#"{"query":{"search":[
        {"title":"Albert Einstein","snippet":"German-born theoretical <span class=\"searchmatch\">physicist</span>"},
        {"title":"Einstein family","snippet":"Relatives of <span class=\"searchmatch\">Einstein</span>"}
    ]}}"#;

    #[test]
    fn results_come_back_in_the_order_the_service_ranked_them() {
        let hits = search_results(SEARCH.as_bytes());
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].title, "Albert Einstein");
        assert_eq!(hits[0].snippet, "German-born theoretical physicist");
        assert_eq!(hits[1].title, "Einstein family");
    }

    #[test]
    fn something_that_is_not_search_results_is_no_results_rather_than_a_failure() {
        assert!(search_results(b"").is_empty());
        assert!(search_results(b"not json").is_empty());
        assert!(search_results(b"{}").is_empty());
        assert!(search_results(br#"{"query":{"search":[]}}"#).is_empty());
        assert!(search_results(br#"{"query":{"search":[{"snippet":"no title"}]}}"#).is_empty());
    }

    #[test]
    fn a_random_answer_names_its_title() {
        let body =
            br#"{"batchcomplete":"","query":{"random":[{"id":736,"ns":0,"title":"Bicycle"}]}}"#;
        assert_eq!(random_title(body), Some("Bicycle".to_owned()));
    }

    #[test]
    fn an_answer_with_no_random_title_is_read_as_none() {
        assert_eq!(random_title(b"{}"), None);
        assert_eq!(random_title(b"not json"), None);
    }

    #[test]
    fn an_extract_answer_yields_the_canonical_title_and_the_body() {
        let body = br#"{"query":{"pages":{"736":{"pageid":736,"ns":0,"title":"Bicycle",
            "extract":"A bicycle, also called a pedal cycle, is a human-powered vehicle."}}}}"#;
        let (title, text) = extract(body).expect("an extract was read");
        assert_eq!(title, "Bicycle");
        assert!(text.starts_with("A bicycle"));
    }

    #[test]
    fn a_title_wikipedia_does_not_have_is_read_as_no_extract() {
        let body = br#"{"query":{"pages":{"-1":{"ns":0,"title":"Nonexistentxyz","missing":""}}}}"#;
        assert_eq!(extract(body), None);
    }
}

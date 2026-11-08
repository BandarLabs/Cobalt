//! Web articles a computer delivered, and Sync, which asks it for more.
//!
//! An article is an EPUB in the card's `Articles` folder: the computer puts
//! it there and nothing else does, so the folder is how Books knows one.
//! It is listed and read like any other book. What makes it an article is
//! kept here: the site it came from and when it was saved, both read from
//! its package once, and whether the owner has read or archived it, which
//! only Books knows.
//!
//! Sync asks the computer paired with this reader for anything clipped while
//! the reader was away. The computer answers by copying those articles here
//! over its own connection to the reader, then says how many it sent, so
//! Books only has to list the card again. The pairing is the one Sidekick
//! and Deck use, written by `kobo sidekick init`.

use std::collections::BTreeMap;
use std::fmt::Write as _;

/// The identifier prefix of a file in the card's `Articles` folder.
pub const FOLDER: &str = "0/Articles/";

/// Store key for what Books knows about each article.
pub const RECORD: &str = "articles-v1";
/// Store key the computer writes the pairing under: `address|code`.
pub const PAIRING: &str = "computer";

/// Whether a library identifier names a delivered article: an EPUB directly
/// in the folder, which is all the computer ever writes there.
#[must_use]
pub fn is_article(id: &str) -> bool {
    id.strip_prefix(FOLDER).is_some_and(|name| {
        !name.is_empty()
            && !name.contains('/')
            && name.to_ascii_lowercase().ends_with(".epub")
            && !name.to_ascii_lowercase().ends_with(".kepub.epub")
    })
}

/// Everything the Articles shelf holds beside the listing.
#[derive(Debug, Default)]
pub struct State {
    /// What Books knows about each article, by library id.
    pub kept: BTreeMap<String, Kept>,
    /// Whether that record has been read back from the store yet.
    pub loaded: bool,
    /// Whether it holds something the store does not have yet.
    pub changed: bool,
    /// The computer Sync asks: its address and pairing code.
    pub computer: Option<(String, String)>,
    /// The request Sync has out, and whether it is the quiet one.
    pub syncing: Option<(kobo_sdk::TaskId, bool)>,
    /// When the computer was last asked, in milliseconds since 1970.
    pub checked_at: Option<u64>,
    /// The line under the shelf's bar.
    pub said: Option<Said>,
    /// The article whose menu is open, by library id. Ids rather than
    /// places, because a listing that arrives while the menu is open, from a
    /// Sync, moves every article along.
    pub menu_open: Option<String>,
    /// The article Delete is asking about, by library id.
    pub confirming: Option<String>,
    /// The article being deleted, until the runtime answers.
    pub deleting: Option<String>,
    /// Said on the shelf when a delete did not go through.
    pub delete_failed: bool,
}

/// What Books keeps about one article.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Kept {
    /// The file's size when its package was read, so a replaced file is
    /// read again.
    pub bytes: u32,
    /// The site it came from, from the package.
    pub site: Option<String>,
    /// When it was saved, in milliseconds since 1970, from the package.
    pub saved: Option<u64>,
    /// Whether the package has been read for the two above.
    pub described: bool,
    pub read: bool,
    pub archived: bool,
}

/// The record as one store value, a line an article.
#[must_use]
pub fn encode(articles: &BTreeMap<String, Kept>) -> Vec<u8> {
    let mut out = String::new();
    for (id, kept) in articles {
        let site = kept
            .site
            .as_deref()
            .unwrap_or_default()
            .replace(['\t', '\n', '\r'], " ");
        let _ = writeln!(
            out,
            "{id}\t{}\t{site}\t{}\t{}{}{}",
            kept.bytes,
            kept.saved
                .map(|saved| saved.to_string())
                .unwrap_or_default(),
            if kept.described { "d" } else { "" },
            if kept.read { "r" } else { "" },
            if kept.archived { "a" } else { "" },
        );
    }
    out.into_bytes()
}

/// The record read back. A line that does not parse is dropped, which costs
/// one article being read again and its marks, not the shelf.
#[must_use]
pub fn decode(bytes: &[u8]) -> BTreeMap<String, Kept> {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let id = fields.next()?.to_owned();
            let bytes = fields.next()?.parse().ok()?;
            let site = fields.next()?;
            let saved = fields.next()?;
            let marks = fields.next()?;
            Some((
                id,
                Kept {
                    bytes,
                    site: (!site.is_empty()).then(|| site.to_owned()),
                    saved: saved.parse().ok(),
                    described: marks.contains('d'),
                    read: marks.contains('r'),
                    archived: marks.contains('a'),
                },
            ))
        })
        .collect()
}

/// The order a shelf of articles is shown in: unread before read, and
/// newest first within each. One not yet described has just arrived, so it
/// goes at the top of its group rather than the bottom.
#[must_use]
pub fn order(kept: &Kept, title: &str) -> (bool, std::cmp::Reverse<u64>, String) {
    (
        kept.read,
        std::cmp::Reverse(kept.saved.unwrap_or(u64::MAX)),
        title.to_lowercase(),
    )
}

/// The computer this reader is paired with, from the record the computer
/// wrote: its address with the helper's port, and the code.
#[must_use]
pub fn pairing(raw: &[u8]) -> Option<(String, String)> {
    let text = std::str::from_utf8(raw).ok()?;
    let (address, code) = text.trim().split_once('|')?;
    let address = address.trim().trim_start_matches("https://");
    let address = address.trim_end_matches('/');
    let code = code.trim();
    let sound = !address.is_empty()
        && !code.is_empty()
        && address.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b':' | b'[' | b']')
        })
        && code.bytes().all(|byte| byte.is_ascii_alphanumeric());
    if !sound {
        return None;
    }
    let address = if address.contains(':') && !address.ends_with(']') {
        address.to_owned()
    } else {
        format!("{address}:9331")
    };
    Some((address, code.to_owned()))
}

/// What the computer said a sync did.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Reply {
    pub delivered: usize,
    pub waiting: usize,
    pub problem: Option<String>,
}

/// Reads the computer's answer. `None` for anything that is not one.
#[must_use]
pub fn reply(bytes: &[u8]) -> Option<Reply> {
    let value = kobo_json::parse(std::str::from_utf8(bytes).ok()?).ok()?;
    let count = |key: &str| {
        value
            .get(key)
            .and_then(kobo_json::Value::as_i64)
            .and_then(|count| usize::try_from(count).ok())
    };
    Some(Reply {
        delivered: count("delivered")?,
        waiting: count("waiting")?,
        problem: value
            .get("problem")
            .and_then(kobo_json::Value::as_str)
            .map(str::to_owned),
    })
}

/// Where Sync stands, in the words the shelf shows under its bar.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Said {
    Checking,
    Arrived(usize),
    NothingNew,
    /// The computer has articles it could not copy here.
    Waiting(usize),
    NotAnswering,
    Offline,
    /// The computer refused the pairing code.
    Refused,
    /// The computer's helper is older than Sync.
    Outdated,
    NotPaired,
}

impl Said {
    /// The line itself, and whether it reports a failure.
    #[must_use]
    pub fn text(&self) -> (String, bool) {
        match self {
            Self::Checking => ("Checking your computer".to_owned(), false),
            Self::Arrived(1) => ("1 article arrived".to_owned(), false),
            Self::Arrived(count) => (format!("{count} articles arrived"), false),
            Self::NothingNew => ("Nothing new".to_owned(), false),
            Self::Waiting(count) => (
                format!(
                    "{} waiting on your computer could not be copied to this reader. Run kobo clip sync on the computer to see why.",
                    if *count == 1 {
                        "1 article".to_owned()
                    } else {
                        format!("{count} articles")
                    }
                ),
                true,
            ),
            Self::NotAnswering => (
                "Your computer is not answering. Start kobo sidekick run on it, then try again.".to_owned(),
                true,
            ),
            Self::Offline => ("This reader is not on Wi-Fi.".to_owned(), true),
            Self::Refused => (
                "Your computer did not accept this reader. Run kobo sidekick init on it again.".to_owned(),
                true,
            ),
            Self::Outdated => (
                "Your computer's kobo command is older than Sync. Update it, then try again.".to_owned(),
                true,
            ),
            Self::NotPaired => (
                "Sync asks your computer for articles. Run kobo sidekick init on it once to pair this reader.".to_owned(),
                true,
            ),
        }
    }
}

/// What a finished sync means for the shelf.
#[must_use]
pub fn outcome(reply: &Reply) -> Said {
    if reply.delivered > 0 {
        Said::Arrived(reply.delivered)
    } else if reply.waiting > 0 {
        Said::Waiting(reply.waiting)
    } else {
        Said::NothingNew
    }
}

/// What a failed request means.
#[must_use]
pub fn failure(error: kobo_sdk::TaskError) -> Said {
    use kobo_sdk::TaskError;
    match error {
        TaskError::Offline => Said::Offline,
        TaskError::Unauthorized => Said::Refused,
        TaskError::NotFound => Said::Outdated,
        _ => Said::NotAnswering,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_epub_directly_in_the_articles_folder_is_an_article() {
        assert!(is_article("0/Articles/River.epub"));
        assert!(is_article("0/Articles/River (2).EPUB"));
        assert!(!is_article("0/River.epub"));
        assert!(!is_article("0/Articles/Saved/River.epub"));
        assert!(!is_article("0/Articles/notes.txt"));
        assert!(!is_article("1/Articles/River.epub"));
        assert!(!is_article("0/Articles/"));
    }

    #[test]
    fn what_books_knows_about_each_article_survives_the_store() {
        let mut articles = BTreeMap::new();
        articles.insert(
            "0/Articles/River.epub".to_owned(),
            Kept {
                bytes: 4096,
                site: Some("Field\tNotes".to_owned()),
                saved: Some(1_762_600_000_000),
                described: true,
                read: true,
                archived: false,
            },
        );
        articles.insert(
            "0/Articles/Sea.epub".to_owned(),
            Kept {
                archived: true,
                ..Kept::default()
            },
        );
        let back = decode(&encode(&articles));
        assert_eq!(
            back["0/Articles/River.epub"].site.as_deref(),
            Some("Field Notes")
        );
        assert_eq!(back["0/Articles/River.epub"].saved, Some(1_762_600_000_000));
        assert!(back["0/Articles/River.epub"].read);
        assert!(back["0/Articles/Sea.epub"].archived);
        assert!(!back["0/Articles/Sea.epub"].described);
    }

    #[test]
    fn unread_articles_come_first_and_the_newest_first_within_each() {
        let at = |saved: Option<u64>, read: bool| Kept {
            saved,
            read,
            ..Kept::default()
        };
        let mut shelf = [
            ("Old unread", at(Some(1), false)),
            ("New read", at(Some(9), true)),
            ("New unread", at(Some(5), false)),
            ("Just arrived", at(None, false)),
        ];
        shelf.sort_by_key(|(title, kept)| order(kept, title));
        let titles: Vec<_> = shelf.iter().map(|(title, _)| *title).collect();
        assert_eq!(
            titles,
            ["Just arrived", "New unread", "Old unread", "New read"]
        );
    }

    #[test]
    fn a_pairing_the_computer_wrote_is_read_with_or_without_its_port() {
        assert_eq!(
            pairing(b"192.168.1.10:9331|abc234\n"),
            Some(("192.168.1.10:9331".to_owned(), "abc234".to_owned()))
        );
        assert_eq!(
            pairing(b"studio.local|abc234"),
            Some(("studio.local:9331".to_owned(), "abc234".to_owned()))
        );
        assert_eq!(pairing(b"192.168.1.10:9331"), None);
        assert_eq!(pairing(b"evil.host/x?|abc"), None);
        assert_eq!(pairing(b"host|code with space"), None);
    }

    #[test]
    fn the_computers_answer_becomes_one_honest_line() {
        let read = |text: &str| reply(text.as_bytes()).map(|reply| outcome(&reply));
        assert_eq!(
            read(r#"{"delivered":2,"waiting":0}"#),
            Some(Said::Arrived(2))
        );
        assert_eq!(
            read(r#"{"delivered":0,"waiting":0}"#),
            Some(Said::NothingNew)
        );
        assert_eq!(
            read(r#"{"delivered":0,"waiting":3,"problem":"asleep"}"#),
            Some(Said::Waiting(3))
        );
        assert_eq!(read("<html>"), None);
        assert_eq!(Said::Arrived(2).text().0, "2 articles arrived");
        assert_eq!(Said::Arrived(1).text().0, "1 article arrived");
        assert_eq!(Said::Checking.text().0, "Checking your computer");
        assert!(Said::NotAnswering
            .text()
            .0
            .starts_with("Your computer is not answering"));
    }

    #[test]
    fn a_request_that_failed_says_what_the_owner_can_do() {
        use kobo_sdk::TaskError;
        assert_eq!(failure(TaskError::Unreachable), Said::NotAnswering);
        assert_eq!(failure(TaskError::TimedOut), Said::NotAnswering);
        assert_eq!(failure(TaskError::Offline), Said::Offline);
        assert_eq!(failure(TaskError::Unauthorized), Said::Refused);
        assert_eq!(failure(TaskError::NotFound), Said::Outdated);
    }
}

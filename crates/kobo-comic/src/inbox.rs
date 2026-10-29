//! The comics `kobo panels push` has sent to a reader and Panels has not yet
//! put on its shelf.
//!
//! A push used to land in one fixed file, so a second push overwrote the
//! first before anybody had opened Panels, and the comic arrived under no
//! name at all. Each sent comic now has a file of its own, named by its
//! content, and this list carries the title it was sent under. The computer
//! writes the list; Panels only reads it, and deletes a comic's file once
//! its own verified copy is on the shelf.

use kobo_json::{ObjectBuilder, Value};

/// The list's name on the Panels shelf.
pub const MANIFEST: &str = "inbox.v1";
pub const MAX_MANIFEST: usize = 64 * 1024;
/// How many comics may wait at once. Each can be the full archive limit, and
/// a reader's storage is shared with everything else on it.
pub const MAX_WAITING: usize = 16;
const FORMAT: &str = "panels-inbox";
const MAX_TITLE_BYTES: usize = 256;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Sent {
    /// The comic's file on the shelf, from [`file_name`].
    pub file: String,
    pub title: String,
    pub pages: usize,
    /// The SHA-256 of the archive, which is also the key Panels files its
    /// own copy under, so a comic already on the shelf is recognised.
    pub sha256: String,
}

/// The shelf file a comic with this digest is sent as.
#[must_use]
pub fn file_name(sha256: &str) -> String {
    format!("sent-{}.cbz", &sha256[..16.min(sha256.len())])
}

fn valid(sent: &Sent) -> bool {
    let hex = |text: &str| {
        text.bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    };
    sent.sha256.len() == 64
        && hex(&sent.sha256)
        && sent.file == file_name(&sent.sha256)
        && !sent.title.trim().is_empty()
        && sent.title.len() <= MAX_TITLE_BYTES
        && !sent.title.chars().any(char::is_control)
        && (1..=crate::MAX_ENTRIES).contains(&sent.pages)
}

#[must_use]
pub fn encode(comics: &[Sent]) -> Vec<u8> {
    let comics: Vec<Value> = comics
        .iter()
        .map(|sent| {
            ObjectBuilder::new()
                .set("file", sent.file.as_str())
                .set("title", sent.title.as_str())
                .set("pages", sent.pages.to_string())
                .set("sha256", sent.sha256.as_str())
                .build()
        })
        .collect();
    ObjectBuilder::new()
        .set("format", FORMAT)
        .set("version", "1")
        .set("comics", comics)
        .build()
        .to_json()
        .into_bytes()
}

/// Reads the list back.
///
/// # Errors
///
/// Refuses a list that is too large, is not this format, or names a comic
/// that could not have been written by `kobo panels push`.
pub fn decode(bytes: &[u8]) -> Result<Vec<Sent>, String> {
    if bytes.len() > MAX_MANIFEST {
        return Err("The list of sent comics is too large.".to_owned());
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| "The list of sent comics is not text.".to_owned())?;
    let root =
        kobo_json::parse(text).map_err(|_| "The list of sent comics is damaged.".to_owned())?;
    if root.get("format").and_then(Value::as_str) != Some(FORMAT)
        || root.get("version").and_then(Value::as_str) != Some("1")
    {
        return Err("The list of sent comics is from a newer Panels.".to_owned());
    }
    let mut comics = Vec::new();
    for entry in root
        .get("comics")
        .and_then(Value::as_array)
        .ok_or_else(|| "The list of sent comics is damaged.".to_owned())?
        .iter()
        .take(MAX_WAITING)
    {
        let text = |key: &str| entry.get(key).and_then(Value::as_str).map(str::to_owned);
        let sent = Sent {
            file: text("file").unwrap_or_default(),
            title: text("title").unwrap_or_default(),
            pages: text("pages")
                .and_then(|pages| pages.parse().ok())
                .unwrap_or(0),
            sha256: text("sha256").unwrap_or_default(),
        };
        if !valid(&sent) {
            return Err("The list of sent comics is damaged.".to_owned());
        }
        comics.push(sent);
    }
    Ok(comics)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sent(title: &str, digit: char) -> Sent {
        let sha256 = digit.to_string().repeat(64);
        Sent {
            file: file_name(&sha256),
            title: title.to_owned(),
            pages: 4,
            sha256,
        }
    }

    #[test]
    fn several_sent_comics_keep_their_titles_through_the_list() {
        let comics = vec![sent("A small garden", 'a'), sent("The lighthouse", 'b')];
        assert_eq!(decode(&encode(&comics)), Ok(comics));
    }

    #[test]
    fn a_list_naming_a_file_push_could_not_have_written_is_refused() {
        let mut bad = sent("Escape", 'c');
        bad.file = "../library".to_owned();
        assert!(decode(&encode(&[bad])).is_err());
        let mut untitled = sent("Fine", 'd');
        untitled.title = "\n".to_owned();
        assert!(decode(&encode(&[untitled])).is_err());
        assert!(decode(b"{\"format\":\"other\",\"version\":\"1\",\"comics\":[]}").is_err());
    }
}

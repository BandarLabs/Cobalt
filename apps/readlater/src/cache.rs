//! Versioned local article data. Bodies are already extracted text, not HTML.
use crate::wallabag::Entry;
use kobo_json::{ObjectBuilder, Value};

pub const LIMIT: usize = 8 * 1024 * 1024;

pub fn encode(entries: &[Entry]) -> Option<Vec<u8>> {
    let size = entries.iter().try_fold(0usize, |size, entry| {
        size.checked_add(entry.title.len())?
            .checked_add(entry.site.len())?
            .checked_add(entry.content.len())
    })?;
    if size > LIMIT {
        return None;
    }
    let items: Vec<_> = entries
        .iter()
        .map(|entry| {
            ObjectBuilder::new()
                .set("id", entry.id.to_string())
                .set("title", entry.title.clone())
                .set("site", entry.site.clone())
                .set("minutes", entry.reading_time.to_string())
                .set("text", entry.content.clone())
                .set("position", entry.position.to_string())
                .set("starred", if entry.starred { "1" } else { "0" })
                .set("archived", if entry.archived { "1" } else { "0" })
                .build()
        })
        .collect();
    let bytes = ObjectBuilder::new()
        .set("version", "2")
        .set("items", items)
        .build()
        .to_json()
        .into_bytes();
    (bytes.len() <= LIMIT).then_some(bytes)
}

pub fn decode(bytes: &[u8]) -> Option<Vec<Entry>> {
    if bytes.len() > LIMIT {
        return None;
    }
    let value = kobo_json::parse(std::str::from_utf8(bytes).ok()?).ok()?;
    // Version 1 predates flags; its articles load as unstarred and unread.
    let flags = match value.get("version")?.as_str()? {
        "1" => false,
        "2" => true,
        _ => return None,
    };
    let mut seen = std::collections::BTreeSet::new();
    value
        .get("items")?
        .as_array()?
        .iter()
        .map(|value| {
            let text = |name| value.get(name).and_then(Value::as_str);
            let id = text("id")?.parse().ok()?;
            if !seen.insert(id) {
                return None;
            }
            Some(Entry {
                id,
                title: text("title")?.into(),
                site: text("site")?.into(),
                reading_time: text("minutes")?.parse().ok()?,
                content: text("text")?.into(),
                position: match value.get("position") {
                    None => 0,
                    Some(value) => value.as_str()?.parse().ok()?,
                },
                starred: flags && text("starred")? == "1",
                archived: flags && text("archived")? == "1",
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extracted_text_round_trips_without_being_parsed_as_html_again() {
        let entries = vec![Entry {
            id: u64::MAX,
            title: "日本語".into(),
            site: "example.org".into(),
            reading_time: 5,
            position: 12,
            content: "Use <section> & preserve \"quotes\".\n\nSecond paragraph.".into(),
            starred: true,
            archived: false,
        }];
        assert_eq!(decode(&encode(&entries).unwrap()), Some(entries));
        assert!(decode(br#"{"version":"3","items":[]}"#).is_none());
        let legacy = br#"{"version":"1","items":[{"id":"7","title":"t","site":"s","minutes":"1","text":"x"}]}"#;
        let legacy = decode(legacy).unwrap();
        assert!(!legacy[0].starred && !legacy[0].archived);
        assert!(decode(b"broken").is_none());
    }
}

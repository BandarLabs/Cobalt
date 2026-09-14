use kobo_json::{ObjectBuilder, Value};

/// The reader's real storage, not the small app key-value store: notes are
/// pushed as files on this app's shelf (`kobo-sdk::Context::shelf`), the same
/// mechanism `apps/frame` uses for photos. [`MANIFEST`] is metadata only
/// (titles, tags, dates, read state) and stays small even for thousands of
/// notes; each note's body is its own shelf blob, fetched only when opened.
pub const MANIFEST: &str = "manifest.v1";
pub const MAX_MANIFEST: usize = 4 * 1024 * 1024;
pub const MAX_BODY: usize = 4 * 1024 * 1024;
pub const PENDING_KEY: &str = "clippings-pending-v1";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NoteMeta {
    pub id: String,
    pub path: String,
    pub title: String,
    pub read: bool,
    pub tags: Vec<String>,
    pub published: String,
    pub created: String,
}

/// Parses the manifest `kobo clippings push` publishes. Body text lives in a
/// separate shelf blob per note, named by `id`, and is fetched separately.
///
/// # Errors
/// Refuses anything that is not this exact, version-1 shape, an invalid or
/// duplicate note identity, or more entries than [`MAX_MANIFEST`] should ever
/// actually contain.
pub fn decode_manifest(bytes: &[u8]) -> Result<Vec<NoteMeta>, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "manifest is not UTF-8".to_owned())?;
    let root = kobo_json::parse(text).map_err(|_| "manifest is not valid JSON".to_owned())?;
    if root.get("version").and_then(Value::as_i64) != Some(1) {
        return Err("manifest version is unsupported".to_owned());
    }
    let rows = root
        .get("notes")
        .and_then(Value::as_array)
        .ok_or("manifest has no notes")?;
    let mut notes = Vec::with_capacity(rows.len());
    // Borrows straight from the parsed rows to check for duplicates, rather
    // than allocating an owned copy of every id just to put it in a set: this
    // walk runs once per manifest load against the real vault's ~3,500
    // entries, right after the JSON parse itself, in the same lifecycle
    // callback.
    let mut ids = std::collections::HashSet::with_capacity(rows.len());
    for row in rows {
        let id = row
            .get("id")
            .and_then(Value::as_str)
            .ok_or("manifest entry has no id")?;
        let path = row
            .get("path")
            .and_then(Value::as_str)
            .ok_or("manifest entry has no path")?;
        if !valid_id(id) || !ids.insert(id) {
            return Err("manifest has an invalid or duplicate note identity".to_owned());
        }
        let tags = row
            .get("tags")
            .and_then(Value::as_array)
            .map(|tags| {
                tags.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        notes.push(NoteMeta {
            id: id.to_owned(),
            path: path.to_owned(),
            title: row
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or(path)
                .to_owned(),
            read: row.get("read").and_then(Value::as_bool).unwrap_or(false),
            tags,
            published: row
                .get("published")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            created: row
                .get("created")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        });
    }
    Ok(notes)
}

#[must_use]
pub fn valid_id(id: &str) -> bool {
    id.strip_prefix("note-").is_some_and(|suffix| {
        suffix.len() <= 27 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Sort {
    Published,
    Created,
}
impl Sort {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Published => "Published date",
            Self::Created => "Created date",
        }
    }
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Published => Self::Created,
            Self::Created => Self::Published,
        }
    }
}

/// What Home's read state is narrowed to. Unread is the default: the whole
/// point of a reading queue is to work it down, and a freshly pushed vault
/// otherwise opens onto thousands of already-read imports.
///
/// Independent of any tag filter (see `Clippings::tag_filter`) rather than
/// combined into one cycling state with it: with the two combined, reaching
/// a tag meant cycling past every other tag already ahead of it in the list,
/// and there was no way back to Unread except cycling through all of them
/// again.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ReadFilter {
    #[default]
    Unread,
    Read,
    All,
}
impl ReadFilter {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Unread => "Unread",
            Self::Read => "Read",
            Self::All => "All",
        }
    }
}

/// Sorts a caller-chosen subset of `notes` (indices into it, already
/// filtered), newest first by the chosen date field. Notes missing that date
/// (Web Clipper sometimes can't find a source `published` date) sort after
/// every note that has one, rather than before (which a naive
/// string-empty comparison would otherwise do).
#[must_use]
pub fn sort_indices(notes: &[NoteMeta], sort: Sort, mut indices: Vec<usize>) -> Vec<usize> {
    fn key(note: &NoteMeta, sort: Sort) -> &str {
        match sort {
            Sort::Published => &note.published,
            Sort::Created => &note.created,
        }
    }
    indices.sort_by(|&a, &b| {
        let (ka, kb) = (key(&notes[a], sort), key(&notes[b], sort));
        match (ka.is_empty(), kb.is_empty()) {
            (true, true) => std::cmp::Ordering::Equal,
            (true, false) => std::cmp::Ordering::Greater,
            (false, true) => std::cmp::Ordering::Less,
            (false, false) => kb.cmp(ka),
        }
    });
    indices
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingEdit {
    pub path: String,
    pub read: bool,
    pub added_tags: Vec<String>,
}

pub fn encode_pending(edits: &[PendingEdit]) -> Vec<u8> {
    let items: Vec<Value> = edits
        .iter()
        .map(|edit| {
            ObjectBuilder::new()
                .set("path", edit.path.clone())
                .set("read", edit.read)
                .set("added_tags", edit.added_tags.clone())
                .build()
        })
        .collect();
    ObjectBuilder::new()
        .set("edits", items)
        .build()
        .to_json()
        .into_bytes()
}

#[must_use]
pub fn decode_pending(bytes: &[u8]) -> Vec<PendingEdit> {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Vec::new();
    };
    let Ok(root) = kobo_json::parse(text) else {
        return Vec::new();
    };
    let Some(items) = root.get("edits").and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let path = item.get("path")?.as_str()?.to_owned();
            let read = item.get("read").and_then(Value::as_bool).unwrap_or(false);
            let added_tags = item
                .get("added_tags")
                .and_then(Value::as_array)
                .map(|tags| {
                    tags.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            Some(PendingEdit {
                path,
                read,
                added_tags,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest_json(notes: &str) -> String {
        format!("{{\"version\":1,\"notes\":[{notes}]}}")
    }

    #[test]
    fn decodes_a_well_formed_manifest() {
        let bytes = manifest_json(
            r#"{"id":"note-0000000000000001","path":"A.md","title":"Alpha","published":"2026-01-01","created":"2026-01-02","tags":["one","two"],"read":false}"#,
        );
        let notes = decode_manifest(bytes.as_bytes()).expect("manifest parses");
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].title, "Alpha");
        assert_eq!(notes[0].tags, vec!["one", "two"]);
        assert!(!notes[0].read);
    }

    #[test]
    fn falls_back_to_the_path_when_a_title_is_missing() {
        let bytes = manifest_json(r#"{"id":"note-0000000000000001","path":"A.md"}"#);
        let notes = decode_manifest(bytes.as_bytes()).expect("manifest parses");
        assert_eq!(notes[0].title, "A.md");
    }

    #[test]
    fn rejects_duplicate_or_malformed_identities() {
        let duplicate = manifest_json(
            r#"{"id":"note-0000000000000001","path":"A.md"},{"id":"note-0000000000000001","path":"B.md"}"#,
        );
        assert!(decode_manifest(duplicate.as_bytes()).is_err());
        let malformed = manifest_json(r#"{"id":"../../secrets","path":"A.md"}"#);
        assert!(decode_manifest(malformed.as_bytes()).is_err());
    }

    #[test]
    fn rejects_an_unsupported_version() {
        assert!(decode_manifest(br#"{"version":2,"notes":[]}"#).is_err());
    }

    #[test]
    fn sorting_by_published_puts_missing_dates_last_and_newest_first() {
        let note = |id: &str, published: &str| NoteMeta {
            id: id.to_owned(),
            path: format!("{id}.md"),
            title: id.to_owned(),
            read: false,
            tags: vec![],
            published: published.to_owned(),
            created: String::new(),
        };
        let notes = vec![
            note("note-0000000000000001", ""),
            note("note-0000000000000002", "2020-01-01"),
            note("note-0000000000000003", "2026-01-01"),
        ];
        let order = sort_indices(&notes, Sort::Published, (0..notes.len()).collect());
        assert_eq!(
            order
                .iter()
                .map(|&i| notes[i].id.as_str())
                .collect::<Vec<_>>(),
            [
                "note-0000000000000003",
                "note-0000000000000002",
                "note-0000000000000001"
            ]
        );
    }

    #[test]
    fn sort_toggles_between_the_two_supported_fields() {
        assert_eq!(Sort::Published.next(), Sort::Created);
        assert_eq!(Sort::Created.next(), Sort::Published);
    }

    #[test]
    fn pending_edits_round_trip_through_json() {
        let edits = vec![
            PendingEdit {
                path: "A.md".to_owned(),
                read: true,
                added_tags: vec!["reviewed".to_owned()],
            },
            PendingEdit {
                path: "B.md".to_owned(),
                read: false,
                added_tags: vec![],
            },
        ];
        let decoded = decode_pending(&encode_pending(&edits));
        assert_eq!(decoded, edits);
    }
}

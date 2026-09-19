//! Decode-only mirror of the Vault shelf format the companion CLI publishes
//! (the frame pattern: the app never links the host crate, so the two sides
//! move on their own branches and meet at the format).

pub const MANIFEST: &str = "manifest.v1";
pub const SYNCED_MANIFEST: &str = "synced.v1";
pub const MAX_MANIFEST: usize = 1024 * 1024;
pub const MAX_NOTE_BYTES: usize = 512 * 1024;
const FORMAT: &str = "vault-shelf";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NoteEntry {
    pub id: String,
    pub path: String,
    pub title: String,
    pub tags: Vec<String>,
    pub links: Vec<String>,
    pub digest: String,
    pub bytes: u64,
    pub added: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportFailure {
    pub input: String,
    pub reason: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Manifest {
    pub notes: Vec<NoteEntry>,
    pub failures: Vec<ImportFailure>,
}

impl Manifest {
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_MANIFEST {
            return Err("the Vault shelf manifest is too large".to_owned());
        }
        let text =
            std::str::from_utf8(bytes).map_err(|_| "the shelf manifest is not UTF-8".to_owned())?;
        let root = kobo_json::parse(text).map_err(|error| format!("shelf manifest: {error}"))?;
        if root.get("format").and_then(kobo_json::Value::as_str) != Some(FORMAT) {
            return Err("the shelf manifest is not a Vault shelf".to_owned());
        }
        if root.get("version").and_then(kobo_json::Value::as_str) != Some("1") {
            return Err("the shelf manifest version is not supported".to_owned());
        }
        let mut notes = Vec::new();
        for entry in root
            .get("notes")
            .and_then(kobo_json::Value::as_array)
            .ok_or_else(|| "the shelf manifest has no note list".to_owned())?
        {
            let text = |key: &str| -> Result<String, String> {
                entry
                    .get(key)
                    .and_then(kobo_json::Value::as_str)
                    .map(str::to_owned)
                    .ok_or_else(|| format!("a shelf note has no {key}"))
            };
            let number = |key: &str| -> Result<u64, String> {
                text(key)?
                    .parse()
                    .map_err(|_| format!("a shelf note has an invalid {key}"))
            };
            let strings = |key: &str| -> Vec<String> {
                entry
                    .get(key)
                    .and_then(kobo_json::Value::as_array)
                    .map(|values| {
                        values
                            .iter()
                            .filter_map(|value| value.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default()
            };
            notes.push(NoteEntry {
                id: text("id")?,
                path: text("path")?,
                title: text("title")?,
                tags: strings("tags"),
                links: strings("links"),
                digest: text("digest")?,
                bytes: number("bytes")?,
                added: number("added")?,
            });
        }
        let mut failures = Vec::new();
        if let Some(entries) = root.get("failures").and_then(kobo_json::Value::as_array) {
            for entry in entries {
                let text = |key: &str| -> Result<String, String> {
                    entry
                        .get(key)
                        .and_then(kobo_json::Value::as_str)
                        .map(str::to_owned)
                        .ok_or_else(|| format!("a shelf failure has no {key}"))
                };
                failures.push(ImportFailure {
                    input: text("input")?,
                    reason: text("reason")?,
                });
            }
        }
        Ok(Self { notes, failures })
    }
}

pub fn note_name(note_id: &str) -> String {
    format!("{note_id}.md")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_a_pushed_manifest() {
        let raw = br#"{"format":"vault-shelf","version":"1","notes":[{"id":"note-abc","path":"Projects/Alpha.md","title":"Alpha","tags":["project"],"links":["Welcome"],"digest":"ff00","bytes":"42","added":"7"}],"failures":[{"input":"broken.md","reason":"not UTF-8"}]}"#;
        let manifest = Manifest::decode(raw).expect("decode");
        assert_eq!(manifest.notes.len(), 1);
        assert_eq!(manifest.notes[0].tags, vec!["project"]);
        assert_eq!(manifest.notes[0].links, vec!["Welcome"]);
        assert_eq!(manifest.failures.len(), 1);
        assert_eq!(note_name("note-abc"), "note-abc.md");
    }

    #[test]
    fn rejects_foreign_and_broken_shelves() {
        assert!(
            Manifest::decode(br#"{"format":"musicstand-shelf","version":"1","notes":[]}"#).is_err()
        );
        assert!(Manifest::decode(b"not json").is_err());
        assert!(Manifest::decode(&vec![b'x'; MAX_MANIFEST + 1]).is_err());
    }

    #[test]
    fn tolerates_entries_without_tags_or_links() {
        let raw = br#"{"format":"vault-shelf","version":"1","notes":[{"id":"note-a","path":"a.md","title":"a","digest":"d","bytes":"1","added":"1"}]}"#;
        let manifest = Manifest::decode(raw).expect("decode");
        assert!(manifest.notes[0].tags.is_empty());
        assert!(manifest.notes[0].links.is_empty());
    }
}

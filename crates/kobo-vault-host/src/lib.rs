//! Host-side vault preparation for Vault: the shelf manifest codec and note
//! packaging. The CLI walks folders of Markdown notes; this crate keeps the
//! on-device format honest and testable on any platform.

use kobo_json::{ObjectBuilder, Value};
use std::collections::{BTreeMap, BTreeSet};

pub const MANIFEST: &str = "manifest.v1";
/// The shelf the Sync ingestion writes: same codec, separate manifest, so a
/// synced drop never has to touch what `kobo vault push` published.
pub const SYNCED_MANIFEST: &str = "synced.v1";
/// Note ids on the synced shelf, distinct from pushed `note-` ids.
pub const SYNCED_PREFIX: &str = "synced-note-";
pub const MAX_MANIFEST: usize = 1024 * 1024;
pub const MAX_NOTES: usize = 2048;
pub const MAX_NOTE_BYTES: u64 = 512 * 1024;
const FORMAT: &str = "vault-shelf";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NoteEntry {
    pub id: String,
    pub path: String,
    pub title: String,
    /// Deduped `#tags` found in the note, so the reader can list and filter
    /// tags without opening every note.
    pub tags: Vec<String>,
    /// Deduped Markdown and wiki-link targets, so backlinks answer from the
    /// manifest alone.
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
    pub fn encode(&self) -> Vec<u8> {
        let notes: Vec<Value> = self
            .notes
            .iter()
            .map(|note| {
                ObjectBuilder::new()
                    .set("id", note.id.clone())
                    .set("path", note.path.clone())
                    .set("title", note.title.clone())
                    .set("tags", note.tags.clone())
                    .set("links", note.links.clone())
                    .set("digest", note.digest.clone())
                    .set("bytes", note.bytes.to_string())
                    .set("added", note.added.to_string())
                    .build()
            })
            .collect();
        let failures: Vec<Value> = self
            .failures
            .iter()
            .map(|failure| {
                ObjectBuilder::new()
                    .set("input", failure.input.clone())
                    .set("reason", failure.reason.clone())
                    .build()
            })
            .collect();
        let root = ObjectBuilder::new()
            .set("format", FORMAT)
            .set("version", "1")
            .set("notes", notes)
            .set("failures", failures)
            .build();
        root.to_json().into_bytes()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_MANIFEST {
            return Err("the Vault shelf manifest is too large".to_owned());
        }
        let text =
            std::str::from_utf8(bytes).map_err(|_| "the shelf manifest is not UTF-8".to_owned())?;
        let value = kobo_json::parse(text).map_err(|error| format!("shelf manifest: {error}"))?;
        let root = &value;
        if root.get("format").and_then(Value::as_str) != Some(FORMAT) {
            return Err("the shelf manifest is not a Vault shelf".to_owned());
        }
        if root.get("version").and_then(Value::as_str) != Some("1") {
            return Err("the shelf manifest version is not supported".to_owned());
        }
        let mut notes = Vec::new();
        for entry in root
            .get("notes")
            .and_then(Value::as_array)
            .ok_or_else(|| "the shelf manifest has no note list".to_owned())?
        {
            let text = |key: &str| -> Result<String, String> {
                entry
                    .get(key)
                    .and_then(Value::as_str)
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
                    .and_then(Value::as_array)
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
        if let Some(entries) = root.get("failures").and_then(Value::as_array) {
            for entry in entries {
                let text = |key: &str| -> Result<String, String> {
                    entry
                        .get(key)
                        .and_then(Value::as_str)
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

/// Content digest for change detection, matching the other shelves' scheme.
pub fn digest(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// The title a reader should see: the note's own heading when it has one,
/// otherwise a humanised file stem.
pub fn title_for(path: &str, body: &str) -> String {
    for line in body.lines() {
        let trimmed = line.trim_start();
        if let Some(heading) = trimmed.strip_prefix("# ") {
            let heading = heading.trim();
            if !heading.is_empty() {
                return heading.to_owned();
            }
        }
    }
    path.rsplit('/')
        .next()
        .unwrap_or(path)
        .trim_end_matches(".md")
        .replace(['-', '_'], " ")
}

/// The deduped `#tags` a note carries, in first-seen order. A tag keeps the
/// slashes and dashes a nested tag uses and drops trailing punctuation.
#[must_use]
pub fn tags_for(body: &str) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();
    for tag in body
        .split_whitespace()
        .filter_map(|word| word.strip_prefix('#'))
        .map(|tag| {
            tag.trim_matches(|c: char| !c.is_alphanumeric() && c != '/' && c != '-')
                .to_owned()
        })
        .filter(|tag| !tag.is_empty())
    {
        if !tags.contains(&tag) {
            tags.push(tag);
        }
    }
    tags
}

/// The deduped note targets a note links to, Markdown links and wiki-links
/// together, attachments and transclusions left out.
#[must_use]
pub fn links_for(body: &str) -> Vec<String> {
    let mut links: Vec<String> = Vec::new();
    let mut push = |target: &str| {
        if !target.is_empty()
            && !looks_like_attachment(target)
            && !links.iter().any(|link| link == target)
        {
            links.push(target.to_owned());
        }
    };
    for (start, _) in body.match_indices("](") {
        if let Some(link) = body[start + 2..].split(')').next() {
            if link.to_ascii_lowercase().ends_with(".md") {
                push(link);
            }
        }
    }
    let mut from = 0;
    while let Some(open) = body[from..].find("[[") {
        let rest = &body[from + open + 2..];
        let Some(close) = rest.find("]]") else {
            break;
        };
        let inner = &rest[..close];
        from += open + 2 + close + 2;
        if inner.starts_with('{') {
            continue;
        }
        let target = inner
            .split('|')
            .next()
            .unwrap_or(inner)
            .split('#')
            .next()
            .unwrap_or(inner)
            .trim();
        push(target);
    }
    links
}

fn looks_like_attachment(target: &str) -> bool {
    let Some((_, ext)) = target.rsplit_once('.') else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "pdf" | "mp3" | "mp4" | "canvas" | "svg"
    )
}

/// One note offered by the CLI after the folder walk.
#[derive(Clone)]
pub struct IncomingNote {
    pub path: String,
    pub title: String,
    pub tags: Vec<String>,
    pub links: Vec<String>,
    pub digest: String,
    pub added: u64,
    pub body: Vec<u8>,
}

pub struct PreparedNote {
    pub note_id: String,
    pub markdown: Vec<u8>,
}

/// A note whose content is unchanged but whose vault-relative path moved:
/// the shelf keeps the same note file and only the manifest entry changes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Rename {
    pub id: String,
    pub from: String,
    pub to: String,
}

pub struct Push {
    pub manifest: Manifest,
    /// Only the notes whose bytes are not already on the shelf.
    pub notes: Vec<PreparedNote>,
    pub removed: Vec<NoteEntry>,
    pub renamed: Vec<Rename>,
}

impl Push {
    pub fn note_name(note_id: &str) -> String {
        format!("{note_id}.md")
    }
}

/// Merge an incoming folder walk into the existing shelf with mirror
/// semantics: unchanged digests keep their files, moved paths become renames
/// rather than re-transfers, and notes no longer offered leave the shelf.
pub fn plan(
    existing: &Manifest,
    incoming: Vec<IncomingNote>,
    failures: Vec<ImportFailure>,
) -> Result<Push, String> {
    plan_prefixed(existing, incoming, failures, "note-")
}

/// The same plan against another id prefix, for the synced shelf.
pub fn plan_prefixed(
    existing: &Manifest,
    incoming: Vec<IncomingNote>,
    mut failures: Vec<ImportFailure>,
    id_prefix: &str,
) -> Result<Push, String> {
    let old_by_digest: BTreeMap<&str, &NoteEntry> = existing
        .notes
        .iter()
        .map(|note| (note.digest.as_str(), note))
        .collect();
    let mut used_ids: BTreeSet<String> =
        existing.notes.iter().map(|note| note.id.clone()).collect();
    let mut claimed: BTreeSet<String> = BTreeSet::new();
    let mut new_by_digest: BTreeMap<String, (String, u64)> = BTreeMap::new();
    let mut notes = Vec::new();
    let mut prepared = Vec::new();
    let mut renamed = Vec::new();
    for offer in incoming {
        let bytes = u64::try_from(offer.body.len()).unwrap_or(u64::MAX);
        if bytes > MAX_NOTE_BYTES {
            failures.push(ImportFailure {
                input: offer.path,
                reason: format!("larger than the {MAX_NOTE_BYTES} byte note limit"),
            });
            continue;
        }
        if let Some(old) = old_by_digest.get(offer.digest.as_str()) {
            if old.path != offer.path {
                renamed.push(Rename {
                    id: old.id.clone(),
                    from: old.path.clone(),
                    to: offer.path.clone(),
                });
            }
            claimed.insert(old.digest.clone());
            notes.push(NoteEntry {
                id: old.id.clone(),
                path: offer.path,
                title: offer.title,
                tags: offer.tags,
                links: offer.links,
                digest: offer.digest,
                bytes,
                added: old.added,
            });
            continue;
        }
        if let Some((id, first_added)) = new_by_digest.get(&offer.digest) {
            // A second path with identical content shares the one note file.
            claimed.insert(offer.digest.clone());
            notes.push(NoteEntry {
                id: id.clone(),
                path: offer.path,
                title: offer.title,
                tags: offer.tags,
                links: offer.links,
                digest: offer.digest,
                bytes,
                added: *first_added,
            });
            continue;
        }
        let base = format!("{id_prefix}{}", &offer.digest[..16.min(offer.digest.len())]);
        let mut id = base.clone();
        let mut suffix = 2_u32;
        while !used_ids.insert(id.clone()) {
            id = format!("{base}-{suffix}");
            suffix = suffix.saturating_add(1);
        }
        claimed.insert(offer.digest.clone());
        new_by_digest.insert(offer.digest.clone(), (id.clone(), offer.added));
        prepared.push(PreparedNote {
            note_id: id.clone(),
            markdown: offer.body,
        });
        notes.push(NoteEntry {
            id,
            path: offer.path,
            title: offer.title,
            tags: offer.tags,
            links: offer.links,
            digest: offer.digest,
            bytes,
            added: offer.added,
        });
    }
    let removed: Vec<NoteEntry> = existing
        .notes
        .iter()
        .filter(|note| !claimed.contains(&note.digest))
        .cloned()
        .collect();
    Ok(Push {
        manifest: Manifest { notes, failures },
        notes: prepared,
        removed,
        renamed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer(path: &str, body: &str, added: u64) -> IncomingNote {
        IncomingNote {
            path: path.to_owned(),
            title: title_for(path, body),
            tags: tags_for(body),
            links: links_for(body),
            digest: digest(body.as_bytes()),
            added,
            body: body.as_bytes().to_vec(),
        }
    }

    #[test]
    fn manifest_round_trips() {
        let manifest = Manifest {
            notes: vec![NoteEntry {
                id: "note-abc".to_owned(),
                path: "Projects/Alpha.md".to_owned(),
                title: "Alpha".to_owned(),
                tags: vec!["project".to_owned()],
                links: vec!["Welcome".to_owned()],
                digest: "ff00".to_owned(),
                bytes: 42,
                added: 7,
            }],
            failures: vec![ImportFailure {
                input: "broken.md".to_owned(),
                reason: "not UTF-8".to_owned(),
            }],
        };
        let decoded = Manifest::decode(&manifest.encode()).expect("decode");
        assert_eq!(decoded, manifest);
    }

    #[test]
    fn decode_rejects_a_foreign_shelf() {
        let wrong = br#"{"format":"musicstand-shelf","version":"1","notes":[]}"#;
        assert!(Manifest::decode(wrong).is_err());
        assert!(Manifest::decode(b"not json").is_err());
    }

    #[test]
    fn title_prefers_the_notes_heading() {
        assert_eq!(
            title_for("x/reading-list.md", "text\n# Reading List\n"),
            "Reading List"
        );
        assert_eq!(
            title_for("Projects/alpha-notes.md", "no heading"),
            "alpha notes"
        );
    }

    #[test]
    fn unchanged_notes_do_not_transfer_again() {
        let first = plan(
            &Manifest::default(),
            vec![offer("a.md", "# A\n", 1)],
            Vec::new(),
        )
        .expect("plan");
        assert_eq!(first.notes.len(), 1);
        let second =
            plan(&first.manifest, vec![offer("a.md", "# A\n", 1)], Vec::new()).expect("plan");
        assert!(second.notes.is_empty());
        assert!(second.removed.is_empty());
        assert_eq!(second.manifest.notes[0].id, first.manifest.notes[0].id);
    }

    #[test]
    fn a_rename_keeps_the_note_file_and_is_reported() {
        let first = plan(
            &Manifest::default(),
            vec![offer("old.md", "# Same\n", 1)],
            Vec::new(),
        )
        .expect("plan");
        let second = plan(
            &first.manifest,
            vec![offer("new.md", "# Same\n", 2)],
            Vec::new(),
        )
        .expect("plan");
        assert!(second.notes.is_empty(), "a rename must not re-transfer");
        assert!(second.removed.is_empty());
        assert_eq!(
            second.renamed,
            vec![Rename {
                id: first.manifest.notes[0].id.clone(),
                from: "old.md".to_owned(),
                to: "new.md".to_owned(),
            }]
        );
        assert_eq!(second.manifest.notes[0].path, "new.md");
    }

    #[test]
    fn notes_no_longer_offered_leave_the_shelf() {
        let first = plan(
            &Manifest::default(),
            vec![offer("a.md", "# A\n", 1), offer("b.md", "# B\n", 1)],
            Vec::new(),
        )
        .expect("plan");
        let second =
            plan(&first.manifest, vec![offer("a.md", "# A\n", 1)], Vec::new()).expect("plan");
        assert_eq!(second.removed.len(), 1);
        assert_eq!(second.removed[0].path, "b.md");
    }

    #[test]
    fn duplicate_digests_never_share_an_id() {
        let push = plan(
            &Manifest::default(),
            vec![offer("a.md", "# Same\n", 1), offer("b.md", "# Same\n", 1)],
            Vec::new(),
        )
        .expect("plan");
        assert_eq!(
            push.manifest.notes.len(),
            2,
            "both paths stay listed in the vault"
        );
        assert_eq!(push.notes.len(), 1, "identical content transfers once");
        assert_eq!(
            push.manifest.notes[0].id, push.manifest.notes[1].id,
            "identical content shares one note file"
        );
    }

    #[test]
    fn tags_and_links_come_out_deduped_and_clean() {
        let body = "See [[Welcome|the home note]] and [[sheet.pdf]]. #project #project, #area/work
                    [a note](Other.md) and [a picture](pic.png) and {{embed}}.";
        assert_eq!(tags_for(body), vec!["project", "area/work"]);
        assert_eq!(links_for(body), vec!["Other.md", "Welcome"]);
    }

    #[test]
    fn oversized_notes_fail_honestly() {
        let big = vec![b'x'; (MAX_NOTE_BYTES + 1) as usize];
        let offer = IncomingNote {
            path: "big.md".to_owned(),
            title: "big".to_owned(),
            tags: Vec::new(),
            links: Vec::new(),
            digest: digest(&big),
            added: 1,
            body: big,
        };
        let push = plan(&Manifest::default(), vec![offer], Vec::new()).expect("plan");
        assert_eq!(push.manifest.notes.len(), 0);
        assert_eq!(push.manifest.failures.len(), 1);
    }
}

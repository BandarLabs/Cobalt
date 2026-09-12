//! A bounded shelf index whose latest edit is retained until storage acknowledges it.
use kobo_json::{ObjectBuilder, Value};
use kobo_sdk::{Context, StoreResult};
use kobo_state::draft::{Draft, Status};
use kobo_state::record::{Error, Schema};
use std::collections::BTreeSet;

pub const MAX_BOOKS: usize = 64;
pub const MAX_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Kept {
    pub key: String,
    pub title: String,
    pub pages: usize,
    pub rtl: bool,
}

pub struct Library {
    entries: Vec<Kept>,
    draft: Draft,
    active: Option<u64>,
}

fn schema() -> Schema {
    Schema::new("panels.library", 1, MAX_BYTES).expect("fixed schema")
}

fn validate(entries: &[Kept]) -> Result<(), Error> {
    if entries.len() > MAX_BOOKS {
        return Err(Error::TooLarge);
    }
    let mut keys = BTreeSet::new();
    for entry in entries {
        if !kobo_sdk::is_valid_key(&entry.key)
            || !keys.insert(&entry.key)
            || entry.title.trim().is_empty()
            || entry.title.len() > 256
            || entry.title.chars().any(char::is_control)
            || !(1..=2048).contains(&entry.pages)
        {
            return Err(Error::Corrupt);
        }
    }
    Ok(())
}

fn encode(entries: &[Kept]) -> Result<Vec<u8>, Error> {
    validate(entries)?;
    schema().encode(&Value::Array(
        entries
            .iter()
            .map(|entry| {
                ObjectBuilder::new()
                    .set("key", entry.key.as_str())
                    .set("title", entry.title.as_str())
                    .set("pages", entry.pages.to_string())
                    .set("rtl", entry.rtl)
                    .build()
            })
            .collect(),
    ))
}

fn decode(bytes: &[u8]) -> Result<Vec<Kept>, Error> {
    if bytes.len() > MAX_BYTES {
        return Err(Error::TooLarge);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| Error::Corrupt)?;
    if text.is_empty() {
        return Err(Error::Corrupt);
    }
    let entries = if text.trim_start().starts_with('{') {
        let record = schema()
            .restore(Some(bytes), |_, _| Err(Error::MigrationUnavailable))?
            .ok_or(Error::Corrupt)?;
        let items = record.payload.as_array().ok_or(Error::Corrupt)?;
        if items.len() > MAX_BOOKS {
            return Err(Error::TooLarge);
        }
        items
            .iter()
            .map(|item| {
                let text = |key| item.get(key).and_then(Value::as_str).ok_or(Error::Corrupt);
                Ok(Kept {
                    key: text("key")?.into(),
                    title: text("title")?.into(),
                    pages: text("pages")?.parse().map_err(|_| Error::Corrupt)?,
                    rtl: item
                        .get("rtl")
                        .and_then(Value::as_bool)
                        .ok_or(Error::Corrupt)?,
                })
            })
            .collect::<Result<Vec<_>, Error>>()?
    } else {
        // The previous format was four tab-separated fields. Migrate it only
        // when every row is valid; a damaged row must not silently disappear.
        let mut entries = Vec::new();
        for line in text.lines() {
            if entries.len() == MAX_BOOKS {
                return Err(Error::TooLarge);
            }
            let fields = line.split('\t').collect::<Vec<_>>();
            if fields.len() != 4 || !matches!(fields[3], "0" | "1") {
                return Err(Error::Corrupt);
            }
            entries.push(Kept {
                key: fields[0].into(),
                title: fields[1].into(),
                pages: fields[2].parse().map_err(|_| Error::Corrupt)?,
                rtl: fields[3] == "1",
            });
        }
        entries
    };
    validate(&entries)?;
    Ok(entries)
}

impl Library {
    pub fn restore(bytes: Option<&[u8]>) -> Result<Self, Error> {
        let entries = bytes.map(decode).transpose()?.unwrap_or_default();
        let canonical = encode(&entries)?;
        let mut draft = Draft::restored(
            bytes.map_or_else(|| canonical.clone(), <[u8]>::to_vec),
            MAX_BYTES,
        )
        .map_err(|_| Error::TooLarge)?;
        draft.replace(canonical).map_err(|_| Error::TooLarge)?;
        Ok(Self {
            entries,
            draft,
            active: None,
        })
    }
    pub fn entries(&self) -> &[Kept] {
        &self.entries
    }
    pub fn status(&self) -> Status<'_> {
        self.draft.status()
    }
    pub fn remember(&mut self, entry: Kept) -> Result<(), Error> {
        let mut next = self.entries.clone();
        next.retain(|kept| kept.key != entry.key);
        next.insert(0, entry);
        let encoded = encode(&next)?;
        self.draft.replace(encoded).map_err(|_| Error::TooLarge)?;
        self.entries = next;
        Ok(())
    }
    pub fn pump(&mut self, context: &mut Context) {
        if let Some(write) = self.draft.begin() {
            self.active = Some(write.revision);
            context.store().save(super::LIBRARY, write.bytes);
        }
    }
    pub fn saved(&mut self, context: &mut Context, result: &StoreResult) -> bool {
        let Some(revision) = self.active else {
            return false;
        };
        let outcome = match result {
            StoreResult::Saved { key } if key == super::LIBRARY => Ok(()),
            StoreResult::Denied(_) => {
                Err("The comic list was not saved. Free some space, then retry.".into())
            }
            _ => return false,
        };
        self.active = None;
        self.draft.finish(revision, outcome);
        self.pump(context);
        true
    }
    pub fn retry(&mut self, context: &mut Context) {
        self.draft.retry();
        self.pump(context);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn book(key: &str) -> Kept {
        Kept {
            key: key.into(),
            title: "A small journey".into(),
            pages: 12,
            rtl: false,
        }
    }
    #[test]
    fn legacy_migration_is_complete_or_refused_and_does_not_mutate_the_original() {
        let bytes = b"one.cbz\tA small journey\t12\t0\n";
        let library = Library::restore(Some(bytes)).unwrap();
        assert_eq!(library.entries(), [book("one.cbz")]);
        assert_eq!(library.status(), Status::Unsaved);
        for bad in [
            b"".as_slice(),
            b"bad",
            b"one.cbz\tBook\t12\t2",
            b"../one.cbz\tBook\t12\t0",
            b"one.cbz\tBook\t0\t0",
            b"one.cbz\tBook\t12\t0\none.cbz\tBook\t12\t0",
        ] {
            let original = bad.to_vec();
            assert!(Library::restore(Some(bad)).is_err());
            assert_eq!(bad, original);
        }
    }
    #[test]
    fn failed_ack_keeps_newest_edit_and_only_retry_can_release_it() {
        let mut library = Library::restore(None).unwrap();
        let mut context = Context::default();
        library.remember(book("one.cbz")).unwrap();
        library.pump(&mut context);
        library.remember(book("two.cbz")).unwrap();
        assert!(library.saved(
            &mut context,
            &StoreResult::Denied(kobo_sdk::StoreError::TooFull)
        ));
        assert!(matches!(library.status(), Status::Failed(_)));
        assert_eq!(library.entries().len(), 2);
        library.retry(&mut context);
        library.saved(
            &mut context,
            &StoreResult::Saved {
                key: super::super::LIBRARY.into(),
            },
        );
        assert_eq!(library.status(), Status::Saved);
        assert_eq!(decode(library.draft.bytes()).unwrap(), library.entries());
    }
    #[test]
    fn oversized_updates_and_future_records_preserve_the_existing_library() {
        let mut library = Library::restore(None).unwrap();
        for index in 0..MAX_BOOKS {
            library.remember(book(&format!("book-{index}"))).unwrap();
        }
        let before = library.entries().to_vec();
        assert!(library.remember(book("one-too-many")).is_err());
        assert_eq!(library.entries(), before);
        let future = Schema::new("panels.library", 2, MAX_BYTES)
            .unwrap()
            .encode(&Value::Array(vec![]))
            .unwrap();
        assert!(matches!(
            Library::restore(Some(&future)),
            Err(Error::NewerVersion)
        ));
    }
}

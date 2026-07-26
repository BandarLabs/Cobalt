//! Where an application keeps what it needs to open where it closed.
//!
//! # Why a key-value store and not a directory
//!
//! An application that can name a path can name `../../../etc/init.d/rcS`, and
//! from then on every caller for the rest of time has to remember to sanitise
//! it. A key namespace deletes the class of mistake rather than defending
//! against it: there is no syntax here that can express somewhere else. The
//! validation in [`kobo_protocol::is_valid_key`] rejects rather than rewrites,
//! because a sanitiser that maps `a/b` and `a-b` onto the same file is a
//! sanitiser that lets one application quietly overwrite its own state.
//!
//! # Why every write is atomic
//!
//! This device loses power. It is a battery in a case with no shutdown button,
//! and the stock reader suspends it without asking. A state file caught
//! half-written is the specific failure that makes an application unopenable
//! for good, which is worse than losing the write: the reader cannot tell the
//! difference between a corrupt file and a broken application, and neither can
//! the application. So every value is written to a temporary file in the same
//! directory, flushed, and then renamed over the old one. A rename within a
//! directory either happened or did not, so a reader either sees the previous
//! value or the new one and never a splice of the two.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use kobo_protocol::{
    is_valid_key, StoreError, StoreRequest, StoreResult, MAX_STORE_KEYS, MAX_STORE_VALUE,
};

/// One application's own small state.
#[derive(Clone, Debug)]
pub struct Store {
    root: Option<PathBuf>,
}

impl Store {
    /// Opens the store an application keeps under `root`.
    ///
    /// The directory is created on the first write rather than here, so an
    /// application that never saves anything leaves nothing behind.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: Some(root.into()),
        }
    }

    /// A store that refuses everything, for a session with nowhere to write.
    ///
    /// Refusing is the honest answer. Accepting writes and dropping them would
    /// leave an application believing it had saved.
    #[must_use]
    pub const fn unavailable() -> Self {
        Self { root: None }
    }

    /// Answers exactly one request.
    #[must_use]
    pub fn handle(&self, request: &StoreRequest) -> StoreResult {
        let Some(root) = self.root.as_deref() else {
            return StoreResult::Denied(StoreError::Unwritable);
        };
        match request {
            StoreRequest::Save { key, value } => Self::save(root, key, value),
            StoreRequest::Load { key } => Self::load(root, key),
            StoreRequest::Forget { key } => Self::forget(root, key),
            StoreRequest::List => Self::list(root),
        }
    }

    fn save(root: &Path, key: &str, value: &[u8]) -> StoreResult {
        if !is_valid_key(key) {
            return StoreResult::Denied(StoreError::BadKey);
        }
        if value.len() > MAX_STORE_VALUE {
            return StoreResult::Denied(StoreError::TooFull);
        }
        // Counted before the write, and only for a key that is not already
        // there, so rewriting an existing value can never be refused for want
        // of room. An application that cannot overwrite its own state is an
        // application that cannot recover from being nearly full.
        if !root.join(key).exists() && Self::count(root) >= MAX_STORE_KEYS {
            return StoreResult::Denied(StoreError::TooFull);
        }
        if fs::create_dir_all(root).is_err() {
            return StoreResult::Denied(StoreError::Unwritable);
        }
        // The temporary name carries the key so two concurrent saves of
        // different keys cannot collide on one scratch file.
        let temporary = root.join(format!(".{key}.writing"));
        if write_then_rename(&temporary, &root.join(key), value).is_err() {
            let _ignored = fs::remove_file(&temporary);
            return StoreResult::Denied(StoreError::Unwritable);
        }
        StoreResult::Saved { key: key.into() }
    }

    fn load(root: &Path, key: &str) -> StoreResult {
        if !is_valid_key(key) {
            return StoreResult::Denied(StoreError::BadKey);
        }
        // A key that was never written and a key that cannot be read are the
        // same answer on purpose: both mean there is nothing to restore, and an
        // application that treated them differently would have two first-run
        // paths, only one of which ever gets tested.
        let value = fs::read(root.join(key))
            .ok()
            .filter(|value| value.len() <= MAX_STORE_VALUE);
        StoreResult::Loaded {
            key: key.into(),
            value,
        }
    }

    fn forget(root: &Path, key: &str) -> StoreResult {
        if !is_valid_key(key) {
            return StoreResult::Denied(StoreError::BadKey);
        }
        // Removing something that is not there is a success. The caller wanted
        // it gone, and it is gone.
        let _ignored = fs::remove_file(root.join(key));
        StoreResult::Forgotten { key: key.into() }
    }

    fn list(root: &Path) -> StoreResult {
        let mut keys: Vec<String> = fs::read_dir(root)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| entry.file_name().into_string().ok())
            // A half-finished write is not a key, and neither is anything else
            // that arrived here without going through `save`.
            .filter(|name| is_valid_key(name))
            .take(MAX_STORE_KEYS)
            .collect();
        keys.sort();
        StoreResult::Keys(keys)
    }

    fn count(root: &Path) -> usize {
        match Self::list(root) {
            StoreResult::Keys(keys) => keys.len(),
            _ => 0,
        }
    }
}

fn write_then_rename(temporary: &Path, final_path: &Path, value: &[u8]) -> std::io::Result<()> {
    {
        let mut file = fs::File::create(temporary)?;
        file.write_all(value)?;
        // Without this the rename can land before the bytes do, and a power
        // loss in that window leaves a correctly named file full of nothing.
        file.sync_all()?;
    }
    fs::rename(temporary, final_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_root() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "kobo-store-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ignored = fs::remove_dir_all(&root);
        root
    }

    #[test]
    fn a_value_survives_being_saved_and_loaded() {
        let store = Store::new(temporary_root());
        assert_eq!(
            store.handle(&StoreRequest::Save {
                key: "tasks".into(),
                value: b"milk".to_vec(),
            }),
            StoreResult::Saved {
                key: "tasks".into()
            }
        );
        assert_eq!(
            store.handle(&StoreRequest::Load {
                key: "tasks".into()
            }),
            StoreResult::Loaded {
                key: "tasks".into(),
                value: Some(b"milk".to_vec()),
            }
        );
    }

    #[test]
    fn a_key_that_was_never_written_loads_as_nothing() {
        let store = Store::new(temporary_root());
        assert_eq!(
            store.handle(&StoreRequest::Load {
                key: "never".into()
            }),
            StoreResult::Loaded {
                key: "never".into(),
                value: None,
            }
        );
    }

    #[test]
    fn an_empty_value_is_not_the_same_as_no_value() {
        // A cleared list is a real state. An application that could not tell it
        // from a first run would helpfully restore the deleted items.
        let store = Store::new(temporary_root());
        let _ignored = store.handle(&StoreRequest::Save {
            key: "list".into(),
            value: Vec::new(),
        });
        assert_eq!(
            store.handle(&StoreRequest::Load { key: "list".into() }),
            StoreResult::Loaded {
                key: "list".into(),
                value: Some(Vec::new()),
            }
        );
    }

    #[test]
    fn a_key_cannot_name_somewhere_else() {
        let root = temporary_root();
        let store = Store::new(&root);
        for escape in ["../escaped", "..", "/etc/passwd", ".hidden", "a/b"] {
            assert_eq!(
                store.handle(&StoreRequest::Save {
                    key: escape.into(),
                    value: b"x".to_vec(),
                }),
                StoreResult::Denied(StoreError::BadKey),
                "{escape} was accepted"
            );
        }
        // Nothing was created outside the store, and the store itself was never
        // brought into existence by a refused write.
        assert!(!root.exists());
    }

    #[test]
    fn an_oversized_value_is_refused_and_leaves_the_old_one() {
        let store = Store::new(temporary_root());
        let _ignored = store.handle(&StoreRequest::Save {
            key: "k".into(),
            value: b"original".to_vec(),
        });
        assert_eq!(
            store.handle(&StoreRequest::Save {
                key: "k".into(),
                value: vec![0; MAX_STORE_VALUE + 1],
            }),
            StoreResult::Denied(StoreError::TooFull)
        );
        assert_eq!(
            store.handle(&StoreRequest::Load { key: "k".into() }),
            StoreResult::Loaded {
                key: "k".into(),
                value: Some(b"original".to_vec()),
            }
        );
    }

    #[test]
    fn overwriting_still_works_when_the_store_is_full() {
        let root = temporary_root();
        let store = Store::new(&root);
        for index in 0..MAX_STORE_KEYS {
            let _ignored = store.handle(&StoreRequest::Save {
                key: format!("k{index}"),
                value: b"x".to_vec(),
            });
        }
        // A new key has nowhere to go...
        assert_eq!(
            store.handle(&StoreRequest::Save {
                key: "one-more".into(),
                value: b"x".to_vec(),
            }),
            StoreResult::Denied(StoreError::TooFull)
        );
        // ...but an application must always be able to update what it already
        // has, or it can never save its way back out of being full.
        assert_eq!(
            store.handle(&StoreRequest::Save {
                key: "k0".into(),
                value: b"updated".to_vec(),
            }),
            StoreResult::Saved { key: "k0".into() }
        );
    }

    #[test]
    fn forgetting_something_absent_is_a_success() {
        let store = Store::new(temporary_root());
        assert_eq!(
            store.handle(&StoreRequest::Forget {
                key: "absent".into()
            }),
            StoreResult::Forgotten {
                key: "absent".into()
            }
        );
    }

    #[test]
    fn listing_reports_only_finished_writes() {
        let root = temporary_root();
        let store = Store::new(&root);
        let _ignored = store.handle(&StoreRequest::Save {
            key: "real".into(),
            value: b"x".to_vec(),
        });
        // A scratch file left by a write that died mid-flight must not appear
        // as a key, or an application would try to load a half-written value.
        fs::write(root.join(".real.writing"), b"half").expect("scratch");
        assert_eq!(
            store.handle(&StoreRequest::List),
            StoreResult::Keys(vec!["real".into()])
        );
    }

    #[test]
    fn a_session_with_nowhere_to_write_refuses_rather_than_pretends() {
        let store = Store::unavailable();
        assert_eq!(
            store.handle(&StoreRequest::Save {
                key: "k".into(),
                value: b"x".to_vec(),
            }),
            StoreResult::Denied(StoreError::Unwritable)
        );
    }
}

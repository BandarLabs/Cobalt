//! Save acknowledgement for editable records. Never discard the latest draft on failure.

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Write {
    pub revision: u64,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Status<'a> {
    Saved,
    Unsaved,
    Saving,
    Failed(&'a str),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error {
    TooLarge,
    Exhausted,
}

/// At most one store write is released at a time. Further edits are coalesced
/// until that write is answered. The owner can always export `bytes()`.
#[derive(Clone, Debug)]
pub struct Draft {
    bytes: Vec<u8>,
    limit: usize,
    revision: u64,
    saved: u64,
    active: Option<u64>,
    failure: Option<String>,
}

impl Draft {
    /// Start from bytes already loaded successfully from storage.
    ///
    /// # Errors
    /// Refuses an initial value larger than the caller's storage bound.
    pub fn restored(bytes: Vec<u8>, limit: usize) -> Result<Self, Error> {
        if bytes.len() > limit {
            return Err(Error::TooLarge);
        }
        Ok(Self {
            bytes,
            limit,
            revision: 0,
            saved: 0,
            active: None,
            failure: None,
        })
    }

    /// Keep an edit in memory. An oversized edit leaves the prior draft intact.
    ///
    /// # Errors
    /// Returns a size/counter error without discarding the previous draft.
    pub fn replace(&mut self, bytes: Vec<u8>) -> Result<bool, Error> {
        if bytes.len() > self.limit {
            return Err(Error::TooLarge);
        }
        if bytes == self.bytes {
            return Ok(false);
        }
        let revision = self.revision.checked_add(1).ok_or(Error::Exhausted)?;
        self.bytes = bytes;
        self.revision = revision;
        Ok(true)
    }

    /// Latest owner text/data, including changes not yet saved. Use for export
    /// after a save failure; exporting must not mark the record as saved.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[must_use]
    pub fn status(&self) -> Status<'_> {
        if let Some(message) = &self.failure {
            Status::Failed(message)
        } else if self.active.is_some() {
            Status::Saving
        } else if self.revision == self.saved {
            Status::Saved
        } else {
            Status::Unsaved
        }
    }

    /// Release one write. A failure pauses writes until `retry`; a queued edit
    /// can begin after the previous response is acknowledged.
    pub fn begin(&mut self) -> Option<Write> {
        if self.failure.is_some() || self.active.is_some() || self.revision == self.saved {
            return None;
        }
        self.active = Some(self.revision);
        Some(Write {
            revision: self.revision,
            bytes: self.bytes.clone(),
        })
    }

    /// Associate the response with the exact released write, not the latest edit.
    /// Ignore unrelated/stale responses. Failures retain all owner changes.
    pub fn finish(&mut self, revision: u64, result: Result<(), String>) -> bool {
        if self.active != Some(revision) {
            return false;
        }
        self.active = None;
        match result {
            Ok(()) => self.saved = revision,
            Err(message) => self.failure = Some(message.chars().take(512).collect()),
        }
        true
    }

    /// Allow another explicit save attempt, retaining the latest draft.
    pub fn retry(&mut self) {
        self.failure = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn earlier_ack_does_not_label_a_later_edit_saved() {
        let mut draft = Draft::restored(b"old".to_vec(), 1024).unwrap();
        draft.replace(b"one".to_vec()).unwrap();
        let first = draft.begin().unwrap();
        draft.replace(b"two".to_vec()).unwrap();
        assert!(draft.begin().is_none());
        assert!(draft.finish(first.revision, Ok(())));
        assert_eq!(draft.status(), Status::Unsaved);
        let second = draft.begin().unwrap();
        assert_eq!(second.bytes, b"two");
        assert!(!draft.finish(first.revision, Ok(())));
        assert_eq!(draft.status(), Status::Saving);
        draft.finish(second.revision, Ok(()));
        assert_eq!(draft.status(), Status::Saved);
    }

    #[test]
    fn failed_save_keeps_the_latest_draft_for_retry_and_export() {
        let mut draft = Draft::restored(vec![], 1024).unwrap();
        draft.replace(b"first edit".to_vec()).unwrap();
        let write = draft.begin().unwrap();
        draft.replace(b"latest edit".to_vec()).unwrap();
        draft.finish(write.revision, Err("Storage is full".into()));
        assert_eq!(draft.bytes(), b"latest edit");
        assert_eq!(draft.status(), Status::Failed("Storage is full"));
        assert!(draft.begin().is_none());
        draft.retry();
        assert_eq!(draft.begin().unwrap().bytes, b"latest edit");
    }

    #[test]
    fn no_op_and_oversized_edits_leave_the_saved_record_alone() {
        let mut draft = Draft::restored(b"kept".to_vec(), 4).unwrap();
        assert!(!draft.replace(b"kept".to_vec()).unwrap());
        assert_eq!(draft.replace(b"too big".to_vec()), Err(Error::TooLarge));
        assert_eq!(draft.bytes(), b"kept");
        assert_eq!(draft.status(), Status::Saved);
        assert!(draft.begin().is_none());
    }
}

//! Save-before-send queue for idempotent provider mutations.
//!
//! Persist `checkpoint().bytes` through the app store, then call `saved` with
//! that checkpoint's revision only after the matching successful store response.
//! `begin` never releases work newer than the acknowledged snapshot. A failed
//! store write leaves it unavailable for sending and available for retry/export.
//!
//! Delivery is at least once: an acknowledged provider request can be replayed
//! if power is lost before the updated queue is saved. Use idempotent operations
//! such as setting read/star/archive state, or a provider idempotency key. Do not
//! use this queue for non-idempotent purchases or message creation.

use kobo_json::{ObjectBuilder, Value};
use std::collections::BTreeSet;

pub const MAX_MUTATIONS: usize = 64;
pub const MAX_VALUE_BYTES: usize = 16 * 1024;
pub const MAX_CHECKPOINT_BYTES: usize = 128 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Mutation {
    pub sequence: u64,
    pub key: String,
    pub value: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Failure {
    Retry(String),
    Conflict(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Checkpoint {
    pub revision: u64,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error {
    Full,
    InvalidKey,
    Corrupt,
    UnsupportedVersion,
    Exhausted,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Full => "There is no room for another pending change. Sync or export the pending changes first.",
            Self::InvalidKey => "A pending change needs a stable record key.",
            Self::Corrupt => "Pending changes could not be read. Keep a copy before resetting them.",
            Self::UnsupportedVersion => "These pending changes were saved by a newer version. Update the app to read them.",
            Self::Exhausted => "The pending-change counter is exhausted. Export the changes before resetting it.",
        })
    }
}
impl std::error::Error for Error {}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Outbox {
    entries: Vec<Mutation>,
    sequence: u64,
    revision: u64,
    durable: u64,
    active: Option<u64>,
    failure: Option<Failure>,
}

impl Outbox {
    #[must_use]
    pub fn entries(&self) -> &[Mutation] {
        &self.entries
    }
    #[must_use]
    pub const fn failure(&self) -> Option<&Failure> {
        self.failure.as_ref()
    }
    #[must_use]
    pub fn needs_save(&self) -> bool {
        self.revision != self.durable
    }

    /// Coalesce changes to an unsent record. An in-flight change is immutable;
    /// a newer value for its key is queued after it.
    ///
    /// # Errors
    /// Refuses invalid keys, exhausted counters and queue/byte limits atomically.
    pub fn enqueue(&mut self, key: String, value: String) -> Result<u64, Error> {
        if key.is_empty() || key.len() > 256 || key.chars().any(char::is_control) {
            return Err(Error::InvalidKey);
        }
        if value.len() > MAX_VALUE_BYTES {
            return Err(Error::Full);
        }
        let mut changed = self.clone();
        let sequence = if let Some(entry) = changed
            .entries
            .iter_mut()
            .rev()
            .find(|entry| entry.key == key && Some(entry.sequence) != changed.active)
        {
            if entry.value == value {
                return Ok(entry.sequence);
            }
            entry.value = value;
            entry.sequence
        } else {
            if changed.entries.len() >= MAX_MUTATIONS {
                return Err(Error::Full);
            }
            changed.sequence = changed.sequence.checked_add(1).ok_or(Error::Exhausted)?;
            changed.entries.push(Mutation {
                sequence: changed.sequence,
                key,
                value,
            });
            changed.sequence
        };
        changed.bump()?;
        // Reserve JSON space for a later failure explanation and growing counters.
        if changed.checkpoint()?.bytes.len() > MAX_CHECKPOINT_BYTES - 4096 {
            return Err(Error::Full);
        }
        *self = changed;
        Ok(sequence)
    }

    fn bump(&mut self) -> Result<(), Error> {
        self.revision = self.revision.checked_add(1).ok_or(Error::Exhausted)?;
        Ok(())
    }

    /// Encode a bounded versioned snapshot. Sequence counters are strings so
    /// JSON floating-point precision cannot change mutation identities.
    ///
    /// # Errors
    /// Returns `Full` if the serialized state exceeds the storage contract.
    pub fn checkpoint(&self) -> Result<Checkpoint, Error> {
        let entries = self
            .entries
            .iter()
            .map(|entry| {
                ObjectBuilder::new()
                    .set("sequence", entry.sequence.to_string())
                    .set("key", entry.key.clone())
                    .set("value", entry.value.clone())
                    .build()
            })
            .collect::<Vec<_>>();
        let (kind, message) = match &self.failure {
            None => ("", ""),
            Some(Failure::Retry(message)) => ("retry", message.as_str()),
            Some(Failure::Conflict(message)) => ("conflict", message.as_str()),
        };
        let bytes = ObjectBuilder::new()
            .set("version", 1_u32)
            .set("sequence", self.sequence.to_string())
            .set("revision", self.revision.to_string())
            .set("entries", entries)
            .set("failure", kind)
            .set("message", message)
            .build()
            .to_json()
            .into_bytes();
        if bytes.len() > MAX_CHECKPOINT_BYTES {
            return Err(Error::Full);
        }
        Ok(Checkpoint {
            revision: self.revision,
            bytes,
        })
    }

    /// Acknowledge only a snapshot actually handed to storage. Older successful
    /// writes cannot mark a later edit as saved. Ignore unexpected future acks.
    pub fn saved(&mut self, revision: u64) -> bool {
        if revision > self.revision || revision < self.durable {
            return false;
        }
        self.durable = revision;
        true
    }

    /// Release the first saved mutation for sending, with one request in flight.
    pub fn begin(&mut self) -> Option<Mutation> {
        if self.needs_save() || self.active.is_some() || self.failure.is_some() {
            return None;
        }
        let entry = self.entries.first()?.clone();
        self.active = Some(entry.sequence);
        Some(entry)
    }

    /// Acknowledge a matching successful provider response, or pause for retry
    /// or conflict resolution. Stale and duplicate responses leave state alone.
    ///
    /// # Errors
    /// Returns `Exhausted` rather than wrapping a revision counter.
    pub fn finish(&mut self, sequence: u64, result: Result<(), Failure>) -> Result<bool, Error> {
        if self.active != Some(sequence) {
            return Ok(false);
        }
        self.bump()?;
        self.active = None;
        match result {
            Ok(()) => self.entries.retain(|entry| entry.sequence != sequence),
            Err(failure) => {
                self.failure = Some(match failure {
                    Failure::Retry(message) => Failure::Retry(message.chars().take(512).collect()),
                    Failure::Conflict(message) => {
                        Failure::Conflict(message.chars().take(512).collect())
                    }
                });
            }
        }
        Ok(true)
    }

    /// Retry requires an explicit owner/app decision. Conflicts are retained
    /// until the app has reconciled the provider's current state.
    ///
    /// # Errors
    /// Returns `Exhausted` rather than wrapping a revision counter.
    pub fn retry(&mut self) -> Result<(), Error> {
        if self.failure.is_some() {
            self.bump()?;
            self.failure = None;
        }
        Ok(())
    }

    /// Load an acknowledged snapshot. A missing key is the expected first-run
    /// state; corrupt or future data is never silently replaced with an empty queue.
    ///
    /// # Errors
    /// Refuses invalid, oversized, unsupported or internally inconsistent state.
    pub fn restore(bytes: Option<&[u8]>) -> Result<Self, Error> {
        let Some(bytes) = bytes else {
            return Ok(Self::default());
        };
        if bytes.len() > MAX_CHECKPOINT_BYTES {
            return Err(Error::Full);
        }
        let value = kobo_json::parse(std::str::from_utf8(bytes).map_err(|_| Error::Corrupt)?)
            .map_err(|_| Error::Corrupt)?;
        if value.get("version").and_then(Value::as_i64) != Some(1) {
            return Err(Error::UnsupportedVersion);
        }
        let sequence = number(&value, "sequence")?;
        let revision = number(&value, "revision")?;
        if sequence > revision {
            return Err(Error::Corrupt);
        }
        let rows = value
            .get("entries")
            .and_then(Value::as_array)
            .ok_or(Error::Corrupt)?;
        if rows.len() > MAX_MUTATIONS {
            return Err(Error::Full);
        }
        let mut entries = Vec::new();
        let mut seen = BTreeSet::new();
        let mut previous = 0;
        for row in rows {
            let entry = Mutation {
                sequence: number(row, "sequence")?,
                key: string(row, "key")?.into(),
                value: string(row, "value")?.into(),
            };
            if entry.sequence == 0
                || entry.sequence > sequence
                || entry.sequence <= previous
                || !seen.insert(entry.sequence)
                || entry.key.is_empty()
                || entry.key.len() > 256
                || entry.key.chars().any(char::is_control)
                || entry.value.len() > MAX_VALUE_BYTES
            {
                return Err(Error::Corrupt);
            }
            previous = entry.sequence;
            entries.push(entry);
        }
        let message = string(&value, "message")?;
        if message.chars().count() > 512 {
            return Err(Error::Corrupt);
        }
        let failure = match string(&value, "failure")? {
            "" if message.is_empty() => None,
            "retry" => Some(Failure::Retry(message.into())),
            "conflict" => Some(Failure::Conflict(message.into())),
            _ => return Err(Error::Corrupt),
        };
        Ok(Self {
            entries,
            sequence,
            revision,
            durable: revision,
            active: None,
            failure,
        })
    }
}

fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str, Error> {
    value.get(key).and_then(Value::as_str).ok_or(Error::Corrupt)
}
fn number(value: &Value, key: &str) -> Result<u64, Error> {
    string(value, key)?.parse().map_err(|_| Error::Corrupt)
}

#[cfg(test)]
mod tests;

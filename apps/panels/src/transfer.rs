//! Bounded transfer state and verified local checkpoints.
use kobo_json::{ObjectBuilder, Value};
use kobo_state::record::{Error, Schema};

pub const CHUNK: usize = 256 * 1024;
pub const MAX_COMIC: usize = kobo_comic::MAX_ARCHIVE_BYTES;
const _: () = assert!(MAX_COMIC <= kobo_sdk::MAX_SHELF_DOWNLOAD);
pub const BLOBS: [&str; 2] = ["partial-a.cbz", "partial-b.cbz"];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Checkpoint {
    pub pending: super::Pending,
    pub slot: usize,
    pub bytes: usize,
    pub digest: String,
    pub complete: bool,
}
impl Checkpoint {
    pub fn new(pending: super::Pending, slot: usize, bytes: &[u8], complete: bool) -> Self {
        Self {
            pending,
            slot,
            bytes: bytes.len(),
            digest: kobo_net::sha256::hex_digest(bytes),
            complete,
        }
    }
    fn validate(&self) -> Result<(), Error> {
        if self.slot > 1
            || self.bytes > MAX_COMIC
            || (self.complete && self.bytes == 0)
            || (!self.complete && self.bytes % CHUNK != 0)
            || self.digest.len() != 64
            || !self
                .digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || !kobo_sdk::is_valid_key(&self.pending.key)
            || self.pending.title.trim().is_empty()
            || self.pending.title.len() > 256
            || self.pending.title.chars().any(char::is_control)
            || self.pending.url.len() > 2048
            || kobo_net::parse(&self.pending.url).is_err()
        {
            return Err(Error::Corrupt);
        }
        Ok(())
    }
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        self.validate()?;
        Schema::new("panels.download", 1, 4096)?.encode(
            &ObjectBuilder::new()
                .set("key", self.pending.key.as_str())
                .set("title", self.pending.title.as_str())
                .set("url", self.pending.url.as_str())
                .set("slot", self.slot.to_string())
                .set("bytes", self.bytes.to_string())
                .set("digest", self.digest.as_str())
                .set("complete", self.complete)
                .build(),
        )
    }
    pub fn restore(bytes: &[u8]) -> Result<Self, Error> {
        let record = Schema::new("panels.download", 1, 4096)?
            .restore(Some(bytes), |_, _| Err(Error::MigrationUnavailable))?
            .ok_or(Error::Corrupt)?;
        let text = |name| {
            record
                .payload
                .get(name)
                .and_then(Value::as_str)
                .ok_or(Error::Corrupt)
        };
        let value = Self {
            pending: super::Pending {
                key: text("key")?.into(),
                title: text("title")?.into(),
                url: text("url")?.into(),
            },
            slot: text("slot")?.parse().map_err(|_| Error::Corrupt)?,
            bytes: text("bytes")?.parse().map_err(|_| Error::Corrupt)?,
            digest: text("digest")?.into(),
            complete: record
                .payload
                .get("complete")
                .and_then(Value::as_bool)
                .ok_or(Error::Corrupt)?,
        };
        value.validate()?;
        Ok(value)
    }
    pub fn matches(&self, bytes: &[u8]) -> bool {
        self.bytes == bytes.len() && self.digest == kobo_net::sha256::hex_digest(bytes)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Download {
    pub url: String,
    pub received: Vec<u8>,
    pub failed: bool,
    pub complete: bool,
    checked: usize,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Step {
    Checked,
    Added,
    Complete,
}
impl Download {
    pub fn new(url: String, received: Vec<u8>) -> Self {
        Self {
            url,
            received,
            failed: false,
            complete: false,
            checked: 0,
        }
    }
    pub fn recheck(&mut self) {
        self.checked = 0;
    }
    pub fn checking(&self) -> bool {
        self.checked < self.received.len()
    }
    pub fn offset(&self) -> u32 {
        u32::try_from(self.checked).unwrap_or(u32::MAX)
    }
    pub fn append(&mut self, chunk: &[u8]) -> Result<Step, &'static str> {
        let result = self.accept(chunk);
        self.failed = result.is_err();
        result
    }
    fn accept(&mut self, chunk: &[u8]) -> Result<Step, &'static str> {
        if chunk.len() > CHUNK {
            return Err("The server returned an unexpected download. Remove it and try again.");
        }
        if self.checking() {
            let end = (self.checked + CHUNK).min(self.received.len());
            if chunk != &self.received[self.checked..end] {
                return Err(
                    "The comic on your server has changed. Remove this download and start again.",
                );
            }
            self.checked = end;
            return Ok(Step::Checked);
        }
        if self.received.len().saturating_add(chunk.len()) > MAX_COMIC {
            return Err("This comic is too large to keep on this reader.");
        }
        self.received.extend_from_slice(chunk);
        self.checked = self.received.len();
        self.complete = chunk.len() < CHUNK;
        Ok(if self.complete {
            Step::Complete
        } else {
            Step::Added
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pending() -> super::super::Pending {
        super::super::Pending {
            key: "comic.cbz".into(),
            title: "Garden".into(),
            url: "https://example.com/garden.cbz".into(),
        }
    }
    #[test]
    fn restored_prefix_must_match_before_new_bytes_are_added() {
        let mut transfer = Download::new(pending().url, vec![0; CHUNK]);
        assert_eq!(transfer.offset(), 0);
        assert!(transfer.append(&vec![1; CHUNK]).is_err());
        assert_eq!(transfer.received, vec![0; CHUNK]);
        assert_eq!(transfer.append(&vec![0; CHUNK]), Ok(Step::Checked));
        assert_eq!(transfer.offset() as usize, CHUNK);
        assert_eq!(transfer.append(&[1]), Ok(Step::Complete));
        assert!(transfer.complete);
    }
    #[test]
    fn refuses_oversized_and_shortened_responses_and_exact_boundary_eof_works() {
        let mut transfer = Download::new(pending().url, vec![]);
        assert!(transfer.append(&vec![1; CHUNK + 1]).is_err());
        transfer.received = vec![0; MAX_COMIC];
        assert!(transfer.append(&[]).is_err());
        transfer.checked = MAX_COMIC;
        assert!(transfer.append(&[1]).is_err());
        assert_eq!(transfer.append(&[]), Ok(Step::Complete));
    }
    #[test]
    fn checkpoint_binds_metadata_to_exact_bytes_and_refuses_newer_or_bad_records() {
        let checkpoint = Checkpoint::new(pending(), 0, &[1, 2], true);
        let encoded = checkpoint.encode().unwrap();
        assert_eq!(Checkpoint::restore(&encoded).unwrap(), checkpoint);
        assert!(checkpoint.matches(&[1, 2]));
        assert!(!checkpoint.matches(&[2, 1]));
        for bad in [
            b"comic.cbz\tGarden\thttps://example.com".as_slice(),
            b"{}",
            b"",
        ] {
            assert!(Checkpoint::restore(bad).is_err());
        }
        let future = Schema::new("panels.download", 2, 4096)
            .unwrap()
            .encode(&Value::Null)
            .unwrap();
        assert!(Checkpoint::restore(&future).is_err());
        let mut bad = checkpoint.clone();
        bad.slot = 2;
        assert!(bad.encode().is_err());
        bad = checkpoint;
        bad.complete = false;
        assert!(bad.encode().is_err());
    }
}

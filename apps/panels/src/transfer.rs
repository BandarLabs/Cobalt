//! Bounded, restartable transfer state. Shelf writes remain atomic.

pub const CHUNK: usize = 256 * 1024;
pub const MAX_COMIC: usize = 12 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Download {
    pub url: String,
    pub received: Vec<u8>,
    pub failed: bool,
}

impl Download {
    pub fn new(url: String, received: Vec<u8>) -> Self {
        Self {
            url,
            received,
            failed: false,
        }
    }

    pub fn offset(&self) -> u32 {
        u32::try_from(self.received.len()).unwrap_or(u32::MAX)
    }

    pub fn append(&mut self, chunk: &[u8]) -> Result<bool, ()> {
        if self.received.len().saturating_add(chunk.len()) > MAX_COMIC {
            self.failed = true;
            return Err(());
        }
        self.received.extend_from_slice(chunk);
        Ok(chunk.len() < CHUNK)
    }
}

#[cfg(test)]
mod tests {
    use super::{Download, CHUNK, MAX_COMIC};
    #[test]
    fn resumes_at_the_saved_offset_and_finishes_on_a_short_chunk() {
        let mut transfer = Download::new("https://library/one.cbz".into(), vec![0; CHUNK]);
        assert_eq!(
            transfer.offset(),
            u32::try_from(CHUNK).expect("transfer chunk fits u32")
        );
        assert!(transfer.append(&[1]).expect("bounded"));
    }
    #[test]
    fn refuses_an_oversized_file() {
        let mut transfer = Download::new("https://library/one.cbz".into(), vec![0; MAX_COMIC]);
        assert!(transfer.append(&[1]).is_err());
        assert!(transfer.failed);
    }
}

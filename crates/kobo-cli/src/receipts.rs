//! Receipts for what this computer sent, and where.
//!
//! A receipt is written only after the companion acknowledged the transfer -
//! Frame's verify pass, a staged file's presence - so "sent" always means
//! sent. Receipts answer "did this already go?" by content hash: sending the
//! same unchanged file to the same target is reported as already done instead
//! of pushed again, and an interrupted transfer leaves no receipt, which is
//! exactly what makes a re-send the resume rather than a duplicate. Same
//! `key = value` blocks as the reader and steps stores.

use std::path::{Path, PathBuf};

/// One acknowledged send.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Receipt {
    /// The file as named on the command line.
    pub file: String,
    /// The companion that took it.
    pub app: String,
    /// Where it went: `sim`, an address, or a saved name.
    pub target: String,
    /// The content hash, so a changed file is a new send, not a duplicate.
    pub sha256: String,
    /// Seconds since the epoch when the transfer was acknowledged.
    pub at: u64,
}

/// Every receipt, oldest first.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Receipts {
    /// Acknowledged sends in the order they completed.
    pub receipts: Vec<Receipt>,
}

impl Receipts {
    /// Reads a receipts file; unknown lines are ignored.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let mut receipts = Self::default();
        let mut receipt = Receipt::empty();
        let mut started = false;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                if started {
                    receipts.receipts.push(receipt.clone());
                    receipt = Receipt::empty();
                    started = false;
                }
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim();
            match key.trim() {
                "send" => {
                    if started {
                        receipts.receipts.push(receipt.clone());
                        receipt = Receipt::empty();
                    }
                    value.clone_into(&mut receipt.file);
                    started = true;
                }
                "app" if started => value.clone_into(&mut receipt.app),
                "target" if started => value.clone_into(&mut receipt.target),
                "sha256" if started => value.clone_into(&mut receipt.sha256),
                "at" if started => receipt.at = value.parse().unwrap_or(0),
                _ => {}
            }
        }
        if started {
            receipts.receipts.push(receipt);
        }
        receipts
    }

    /// Writes the receipts back out, in the same shape [`Receipts::parse`]
    /// reads.
    #[must_use]
    pub fn render(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        for receipt in &self.receipts {
            let _ = writeln!(out, "send = {}", receipt.file);
            let _ = writeln!(out, "app = {}", receipt.app);
            let _ = writeln!(out, "target = {}", receipt.target);
            let _ = writeln!(out, "sha256 = {}", receipt.sha256);
            let _ = writeln!(out, "at = {}", receipt.at);
            out.push('\n');
        }
        out
    }

    /// Loads the receipts at `path`; a missing file is no receipts.
    pub fn load(path: &Path) -> Result<Self, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(Self::parse(&text)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(format!("{} cannot be read: {error}", path.display())),
        }
    }

    /// Saves the receipts at `path`, making its folder when needed.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("{} cannot be made: {error}", parent.display()))?;
        }
        std::fs::write(path, self.render())
            .map_err(|error| format!("{} cannot be written: {error}", path.display()))
    }

    /// The receipt for exactly this content to exactly this target, when one
    /// exists. A changed file hashes differently and misses, which is what
    /// makes a re-send of edited content a new send.
    #[must_use]
    pub fn find(&self, app: &str, target: &str, sha256: &str) -> Option<&Receipt> {
        self.receipts.iter().rev().find(|receipt| {
            receipt.app == app && receipt.target == target && receipt.sha256 == sha256
        })
    }

    /// Records an acknowledged send.
    pub fn record(&mut self, receipt: Receipt) {
        self.receipts.push(receipt);
    }
}

impl Receipt {
    fn empty() -> Self {
        Self {
            file: String::new(),
            app: String::new(),
            target: String::new(),
            sha256: String::new(),
            at: 0,
        }
    }
}

/// Where receipts live: `$KOBO_CONFIG_DIR/receipts`, else
/// `~/.config/kobo/receipts`, beside the reader and steps stores.
#[must_use]
pub fn receipts_path() -> PathBuf {
    if let Some(value) = std::env::var_os("KOBO_CONFIG_DIR") {
        return PathBuf::from(value).join("receipts");
    }
    let home = std::env::var_os("HOME").unwrap_or_default();
    PathBuf::from(home)
        .join(".config")
        .join("kobo")
        .join("receipts")
}

/// The content hash of a file about to be sent.
pub fn hash_file(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path)
        .map_err(|error| format!("{} cannot be read: {error}", path.display()))?;
    Ok(crate::sha256::hex_digest(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt(file: &str, sha: &str, at: u64) -> Receipt {
        Receipt {
            file: file.to_owned(),
            app: "frame".to_owned(),
            target: "sim".to_owned(),
            sha256: sha.to_owned(),
            at,
        }
    }

    #[test]
    fn receipts_round_trip() {
        let mut receipts = Receipts::default();
        receipts.record(receipt("a.png", "abc", 10));
        let parsed = Receipts::parse(&receipts.render());
        assert_eq!(parsed, receipts);
        assert!(parsed.find("frame", "sim", "abc").is_some());
    }

    #[test]
    fn a_changed_file_or_a_different_target_is_a_new_send() {
        let mut receipts = Receipts::default();
        receipts.record(receipt("a.png", "abc", 10));
        assert!(
            receipts.find("frame", "sim", "def").is_none(),
            "edited content resends"
        );
        assert!(
            receipts.find("frame", "192.168.1.23", "abc").is_none(),
            "another reader is a new send"
        );
        assert!(
            receipts.find("feeds", "sim", "abc").is_none(),
            "another companion is a new send"
        );
    }

    #[test]
    fn an_interrupted_send_left_no_receipt() {
        let receipts = Receipts::parse("");
        assert!(receipts.find("frame", "sim", "abc").is_none());
    }
}

/// A send that did not finish: the selection and the prepared routing, kept
/// so a retry is one word rather than the whole command again.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pending {
    /// The file as named on the command line.
    pub file: String,
    /// The companion chosen for it, when the choice was settled.
    pub app: Option<String>,
    /// The target flags exactly as given, e.g. `--sim` or `--reader clara`.
    pub target: String,
}

impl Pending {
    /// Reads a pending-send file; unknown lines are ignored.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let mut pending = Pending {
            file: String::new(),
            app: None,
            target: String::new(),
        };
        for line in text.lines() {
            let Some((key, value)) = line.trim().split_once('=') else {
                continue;
            };
            let value = value.trim();
            match key.trim() {
                "send" => value.clone_into(&mut pending.file),
                "app" => pending.app = (!value.is_empty()).then(|| value.to_owned()),
                "target" => value.clone_into(&mut pending.target),
                _ => {}
            }
        }
        (!pending.file.is_empty() && !pending.target.is_empty()).then_some(pending)
    }

    /// Writes the pending send back out.
    #[must_use]
    pub fn render(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        let _ = writeln!(out, "send = {}", self.file);
        let _ = writeln!(out, "app = {}", self.app.as_deref().unwrap_or_default());
        let _ = writeln!(out, "target = {}", self.target);
        out
    }

    /// Loads the pending send at `path`; a missing file is nothing pending.
    pub fn load(path: &Path) -> Result<Option<Self>, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(Self::parse(&text)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!("{} cannot be read: {error}", path.display())),
        }
    }

    /// Saves the pending send at `path`, making its folder when needed.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("{} cannot be made: {error}", parent.display()))?;
        }
        std::fs::write(path, self.render())
            .map_err(|error| format!("{} cannot be written: {error}", path.display()))
    }

    /// Clears a pending send; clearing nothing is fine.
    pub fn clear(path: &Path) {
        let _ = std::fs::remove_file(path);
    }
}

/// Where a pending send lives: beside the receipts it never became.
#[must_use]
pub fn pending_path() -> PathBuf {
    if let Some(value) = std::env::var_os("KOBO_CONFIG_DIR") {
        return PathBuf::from(value).join("pending-send");
    }
    let home = std::env::var_os("HOME").unwrap_or_default();
    PathBuf::from(home)
        .join(".config")
        .join("kobo")
        .join("pending-send")
}

#[cfg(test)]
mod pending_tests {
    use super::*;

    #[test]
    fn a_pending_send_round_trips_and_clears() {
        let pending = Pending {
            file: "a.png".to_owned(),
            app: Some("frame".to_owned()),
            target: "--reader clara".to_owned(),
        };
        let parsed = Pending::parse(&pending.render()).expect("parses");
        assert_eq!(parsed, pending);
        let path = std::env::temp_dir().join(format!("kobo-pending-{}", std::process::id()));
        assert!(Pending::load(&path).expect("loads").is_none());
        pending.save(&path).expect("saves");
        assert!(Pending::load(&path).expect("loads").is_some());
        Pending::clear(&path);
        assert!(Pending::load(&path).expect("loads").is_none());
    }

    #[test]
    fn an_empty_or_half_written_pending_file_is_nothing_pending() {
        assert!(Pending::parse("").is_none());
        assert!(
            Pending::parse("send = a.png\n").is_none(),
            "no target, nothing to retry"
        );
    }
}

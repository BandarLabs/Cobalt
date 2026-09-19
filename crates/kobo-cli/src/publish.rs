//! Publishing content so a reader never sees half a file.
//!
//! Every companion's transfer ends here: the new bytes go to a `.writing`
//! sibling, are synced to storage, and only then renamed over the
//! destination. A rename is one directory operation, so whatever the owner
//! or the next run sees is the whole old file or the whole new one - a
//! crash, a full card or a pulled cable leaves the previous valid content
//! exactly where it was. A leftover partial is the visible remains of an
//! interrupted write: it blocks the next publish until somebody looks at it
//! rather than being silently written over, and the error says the shelf is
//! unchanged.

use std::fs;
use std::io::Write as _;
use std::path::Path;

/// Publishes `bytes` to `destination` atomically. `what` names the content
/// in owner language ("subscription list", "photo fit") for the errors.
pub fn atomically(destination: &Path, bytes: &[u8], what: &str) -> Result<(), String> {
    let name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("invalid {what} filename"))?;
    let partial = destination.with_file_name(format!(".{name}.writing"));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&partial)
        .map_err(|error| format!("prepare {what} (existing content unchanged): {error}"))?;
    let result = file
        .write_all(bytes)
        .and_then(|()| file.sync_all())
        .and_then(|()| {
            drop(file);
            fs::rename(&partial, destination)
        });
    if let Err(error) = result {
        let _ = fs::remove_file(&partial);
        return Err(format!(
            "could not publish {what}; previous content unchanged: {error}"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_publish_keeps_the_previous_file_and_cleans_up() {
        let root = std::env::temp_dir().join(format!("kobo-publish-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let destination = root.join("list.opml");
        fs::write(&destination, b"previous valid content").unwrap();
        // A directory where the partial must go blocks the write, not the
        // old file.
        let blocker = root.join(".list.opml.writing");
        fs::create_dir(&blocker).unwrap();
        assert!(atomically(&destination, b"new", "list").is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"previous valid content");
        fs::remove_dir(&blocker).unwrap();
        atomically(&destination, b"new", "list").unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"new");
        assert!(!root.join(".list.opml.writing").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_interrupted_write_blocks_until_it_is_seen() {
        let root = std::env::temp_dir().join(format!("kobo-publish-stale-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let destination = root.join("list.opml");
        fs::write(root.join(".list.opml.writing"), b"half a transfer").unwrap();
        let error = atomically(&destination, b"new", "list").unwrap_err();
        assert!(error.contains("unchanged"), "{error}");
        fs::remove_dir_all(root).unwrap();
    }
}

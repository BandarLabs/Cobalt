//! Filesystem acknowledgement boundary for app records and published shelf files.
//! A successful rename is visible; only a successful parent flush acknowledges
//! its durability. After a failed flush the new value may already be visible,
//! so callers retain pending state and can retry, never claim rollback.

use std::fs;
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::Path;

/// A write failure injected after request validation and before filesystem changes.
/// Reads and removal remain available for recovery.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WriteFault {
    NoRoom,
    Unwritable,
}

impl WriteFault {
    pub(crate) const fn error(self) -> kobo_protocol::StoreError {
        match self {
            Self::NoRoom => kobo_protocol::StoreError::NoRoom,
            Self::Unwritable => kobo_protocol::StoreError::Unwritable,
        }
    }
}

pub(crate) fn store_error(error: &io::Error) -> kobo_protocol::StoreError {
    if error.kind() == io::ErrorKind::StorageFull {
        kobo_protocol::StoreError::NoRoom
    } else {
        kobo_protocol::StoreError::Unwritable
    }
}

fn parent(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

pub(crate) fn sync_directory(path: &Path) -> io::Result<()> {
    fs::File::open(path)?.sync_all()
}

pub(crate) fn ensure_directory(path: &Path) -> io::Result<()> {
    ensure_directory_with(path, &mut sync_directory)
}

fn ensure_directory_with(
    path: &Path,
    sync: &mut impl FnMut(&Path) -> io::Result<()>,
) -> io::Result<()> {
    if path.is_dir() {
        // Also handles retry after this directory was created but its parent
        // flush failed. Ancestors are made durable before a child is created.
        return if path.parent().is_some() {
            sync(parent(path))
        } else {
            Ok(())
        };
    }
    if path.as_os_str().is_empty() {
        return Ok(());
    }
    let parent = parent(path);
    if parent != path {
        ensure_directory_with(parent, sync)?;
    }
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists && path.is_dir() => {}
        Err(error) => return Err(error),
    }
    sync(parent)
}

pub(crate) fn publish(partial: &Path, destination: &Path) -> io::Result<()> {
    publish_with(partial, destination, &mut sync_directory)
}

fn publish_with(
    partial: &Path,
    destination: &Path,
    sync: &mut impl FnMut(&Path) -> io::Result<()>,
) -> io::Result<()> {
    fs::File::open(partial)?.sync_all()?;
    fs::rename(partial, destination)?;
    sync(parent(destination))
}

pub(crate) fn remove(root: &Path, paths: &[&Path]) -> io::Result<()> {
    remove_with(root, paths, &mut sync_directory)
}

fn remove_with(
    root: &Path,
    paths: &[&Path],
    sync: &mut impl FnMut(&Path) -> io::Result<()>,
) -> io::Result<()> {
    for path in paths {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    match fs::metadata(root) {
        Ok(metadata) if metadata.is_dir() => sync(root),
        // An existing non-directory root is damaged/unwritable, not empty.
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            "app data root is not a directory",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    struct Scratch(PathBuf);
    impl Scratch {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "cobalt-durability-{}-{name}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
            Self(root)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn failed_sync(_: &Path) -> io::Result<()> {
        Err(io::Error::other("injected directory flush failure"))
    }

    #[test]
    fn a_visible_rename_is_not_acknowledged_when_its_directory_cannot_flush() {
        let root = Scratch::new("publish");
        let partial = root.0.join("partial");
        let final_path = root.0.join("note");
        fs::write(&final_path, "old").unwrap();
        fs::write(&partial, "new").unwrap();
        assert!(publish_with(&partial, &final_path, &mut failed_sync).is_err());
        assert_eq!(
            fs::read(&final_path).unwrap(),
            b"new",
            "no false promise of rollback"
        );
        fs::write(&partial, "new").unwrap();
        publish(&partial, &final_path).unwrap();
        assert_eq!(fs::read(&final_path).unwrap(), b"new");
    }

    #[test]
    fn failed_delete_flush_can_be_retried_even_when_the_name_is_already_absent() {
        let root = Scratch::new("remove");
        let file = root.0.join("note");
        fs::write(&file, "original").unwrap();
        assert!(remove_with(&root.0, &[&file], &mut failed_sync).is_err());
        assert!(!file.exists());
        let mut flushed = false;
        remove_with(&root.0, &[&file], &mut |path| {
            assert_eq!(path, root.0);
            flushed = true;
            sync_directory(path)
        })
        .unwrap();
        assert!(
            flushed,
            "idempotent retry must still flush the previous unlink"
        );
    }

    #[test]
    fn new_directory_ancestors_are_confirmed_before_a_child_is_created() {
        let root = Scratch::new("mkdir");
        let parent = root.0.join("state");
        let child = parent.join("app");
        let mut failed_once = false;
        assert!(ensure_directory_with(&child, &mut |path| {
            if path == root.0 && parent.is_dir() {
                failed_once = true;
                Err(io::Error::other("parent not durable"))
            } else {
                sync_directory(path)
            }
        })
        .is_err());
        assert!(failed_once && parent.is_dir());
        assert!(
            !child.exists(),
            "do not create a child under an unconfirmed parent"
        );
        ensure_directory(&child).unwrap();
        assert!(child.is_dir());
    }

    #[test]
    fn bad_roots_and_failed_unlinks_do_not_report_success() {
        let root = Scratch::new("bad-root");
        let directory = root.0.join("directory");
        fs::create_dir(&directory).unwrap();
        assert!(remove(&root.0, &[&directory]).is_err());
        let file = root.0.join("not-a-directory");
        fs::write(&file, "keep").unwrap();
        assert!(ensure_directory(&file).is_err());
        assert!(remove(&file, &[&file.join("record")]).is_err());
        assert_eq!(fs::read(file).unwrap(), b"keep");
    }
}

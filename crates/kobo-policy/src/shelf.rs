//! Where an application keeps something too big to hold in a message.
//!
//! # Why this is not just a bigger [`crate::store`]
//!
//! The store is for the handful of kilobytes an application needs to reopen
//! where it closed, and every operation there moves a whole value at once.
//! That is the right shape for a reading position and the wrong shape for the
//! book: a frame is capped at a megabyte, and a book is several. Raising the
//! cap would not help, because the ceiling is not arbitrary — it is what keeps
//! either end from being made to allocate an unbounded buffer by a peer that
//! merely said it was going to send one.
//!
//! So a blob moves in pieces, and this module is the thing that reassembles
//! them. Its whole job is that the reassembly is safe to interrupt.
//!
//! # Why a blob is invisible until it is finished
//!
//! A half-downloaded book that can be opened is worse than no book. It opens,
//! it reads correctly for a while, and then it stops in the middle of a
//! sentence with nothing to say why — and the application cannot tell that
//! from a book that was always like that. So pieces land in a file whose name
//! cannot be spelled as a blob name, and only the rename at the end publishes
//! it. Anything interrupted leaves a partial that no read can reach.
//!
//! # Why the card's free space is anybody's business here
//!
//! Cobalt's data sits on the same partition as `KoboReader.sqlite`, which is
//! the stock reader's entire library — every book, shelf, bookmark and
//! position. Fill that partition and the reader's own database cannot write.
//! Nothing about that failure points back at us, and the person holding the
//! device has no way to know that the thing which broke their library was an
//! application they installed. So a write that would take the card below
//! [`SHELF_RESERVE`] is refused, and a write that runs out of room part way
//! through takes the whole partial blob with it rather than leaving megabytes
//! of a book nobody can read.

use std::fs;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use kobo_protocol::{
    is_valid_key, StoreError, StoreRequest, StoreResult, MAX_SHELF_BLOBS, MAX_SHELF_BYTES,
    MAX_SHELF_CHUNK, SHELF_RESERVE,
};

/// One application's large data.
#[derive(Clone, Debug)]
pub struct Shelf {
    root: Option<PathBuf>,
}

impl Shelf {
    /// Opens the shelf an application keeps under `root`.
    ///
    /// The directory is created on the first write, so an application that
    /// never stores anything leaves nothing behind.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: Some(root.into()),
        }
    }

    /// A shelf that refuses everything, for a session with nowhere to write.
    #[must_use]
    pub const fn unavailable() -> Self {
        Self { root: None }
    }

    /// Answers exactly one request, or `None` if it was not a shelf request.
    ///
    /// Returning `None` rather than a denial keeps the ordinary store requests
    /// routable through the same call site without this module having to know
    /// what the store does with them.
    #[must_use]
    pub fn handle(&self, request: &StoreRequest) -> Option<StoreResult> {
        let root = match &self.root {
            Some(root) => root.as_path(),
            None => {
                return match request {
                    StoreRequest::ShelfWrite { .. }
                    | StoreRequest::ShelfRead { .. }
                    | StoreRequest::ShelfRemove { .. }
                    | StoreRequest::ShelfList => Some(StoreResult::Denied(StoreError::Unwritable)),
                    _ => None,
                }
            }
        };
        match request {
            StoreRequest::ShelfWrite {
                name,
                offset,
                bytes,
                last,
            } => Some(Self::write(root, name, *offset, bytes, *last)),
            StoreRequest::ShelfRead {
                name,
                offset,
                length,
            } => Some(Self::read(root, name, *offset, *length)),
            StoreRequest::ShelfRemove { name } => Some(Self::remove(root, name)),
            StoreRequest::ShelfList => Some(Self::list(root)),
            _ => None,
        }
    }

    fn write(root: &Path, name: &str, offset: u32, bytes: &[u8], last: bool) -> StoreResult {
        if !is_valid_key(name) {
            return StoreResult::Denied(StoreError::BadKey);
        }
        if bytes.len() > MAX_SHELF_CHUNK {
            return StoreResult::Denied(StoreError::TooFull);
        }
        if fs::create_dir_all(root).is_err() {
            return StoreResult::Denied(StoreError::Unwritable);
        }
        let partial = partial_path(root, name);
        let chunk = u64::try_from(bytes.len()).unwrap_or(u64::MAX);

        // Offset zero starts the blob over. This is the only way back from an
        // interrupted download: the application does not know how much of its
        // last piece landed before the connection went, and asking would need
        // a round trip it cannot make until it has reconnected anyway. So
        // beginning again is always allowed and always means the same thing.
        if offset == 0 {
            // Checked here rather than at the end, so an application that is
            // over its allowance finds out before it uploads a whole book.
            if !path_for(root, name).exists() && count(root) >= MAX_SHELF_BLOBS {
                return StoreResult::Denied(StoreError::TooFull);
            }
            let _ignored = fs::remove_file(&partial);
        }

        let so_far = size_of(&partial).unwrap_or(0);
        if u64::from(offset) != so_far {
            return StoreResult::Denied(StoreError::Missing);
        }

        // Counted against what this application already holds, not counting
        // the partial being replaced -- an application that cannot re-download
        // a book it already has is an application that cannot repair itself.
        if usage(root).saturating_add(chunk) > MAX_SHELF_BYTES {
            return StoreResult::Denied(StoreError::TooFull);
        }
        if !room_for(root, chunk) {
            return StoreResult::Denied(StoreError::NoRoom);
        }

        match append(&partial, offset, bytes) {
            Ok(()) => {}
            Err(error) => {
                // The blob goes, not just the piece. A book that stopped
                // half way through is bytes on a card nobody can spend and
                // nobody can read, and this is the exact moment we know it is
                // never going to be finished.
                let _ignored = fs::remove_file(&partial);
                return StoreResult::Denied(if error.kind() == std::io::ErrorKind::StorageFull {
                    StoreError::NoRoom
                } else {
                    StoreError::Unwritable
                });
            }
        }

        let size = so_far.saturating_add(chunk);
        if last && publish(&partial, &path_for(root, name)).is_err() {
            let _ignored = fs::remove_file(&partial);
            return StoreResult::Denied(StoreError::Unwritable);
        }
        StoreResult::ShelfWritten {
            name: name.into(),
            size: clamp_size(size),
        }
    }

    fn read(root: &Path, name: &str, offset: u32, length: u32) -> StoreResult {
        if !is_valid_key(name) {
            return StoreResult::Denied(StoreError::BadKey);
        }
        let path = path_for(root, name);
        let Some(size) = size_of(&path) else {
            return StoreResult::Denied(StoreError::Missing);
        };
        if u64::from(offset) > size {
            return StoreResult::Denied(StoreError::Missing);
        }
        // Reading at exactly the end is an empty answer rather than a refusal,
        // so a caller that walks a blob in fixed steps stops cleanly instead of
        // having to treat its last request as a special case.
        let wanted = usize::try_from(length)
            .unwrap_or(usize::MAX)
            .min(MAX_SHELF_CHUNK);
        let available =
            usize::try_from(size.saturating_sub(u64::from(offset))).unwrap_or(usize::MAX);
        let mut bytes = vec![0; wanted.min(available)];
        if !bytes.is_empty() {
            match read_at(&path, u64::from(offset), &mut bytes) {
                Ok(()) => {}
                Err(_) => return StoreResult::Denied(StoreError::Unwritable),
            }
        }
        StoreResult::ShelfRead {
            name: name.into(),
            offset,
            bytes,
            size: clamp_size(size),
        }
    }

    fn remove(root: &Path, name: &str) -> StoreResult {
        if !is_valid_key(name) {
            return StoreResult::Denied(StoreError::BadKey);
        }
        // Both copies. Leaving the partial behind would mean a caller that
        // asked for something to be gone is still paying for it, and cannot
        // see it to ask again.
        let _ignored = fs::remove_file(path_for(root, name));
        let _ignored = fs::remove_file(partial_path(root, name));
        StoreResult::ShelfRemoved { name: name.into() }
    }

    fn list(root: &Path) -> StoreResult {
        let mut blobs: Vec<(String, u32)> = finished(root)
            .map(|(name, size)| (name, clamp_size(size)))
            .take(MAX_SHELF_BLOBS)
            .collect();
        blobs.sort();
        StoreResult::Shelf(blobs)
    }
}

/// Everything finished on the shelf, with its size on disk.
fn finished(root: &Path) -> impl Iterator<Item = (String, u64)> + '_ {
    fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            // A partial cannot pass this, because its name begins with a dot
            // and a blob name may not. That is the whole publishing mechanism:
            // it is not a flag anybody has to remember to check.
            if !is_valid_key(&name) {
                return None;
            }
            let size = entry.metadata().ok()?.len();
            Some((name, size))
        })
}

fn count(root: &Path) -> usize {
    finished(root).count()
}

/// Everything this application is holding, finished or not.
fn usage(root: &Path) -> u64 {
    fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.metadata().ok())
        .filter(std::fs::Metadata::is_file)
        .map(|metadata| metadata.len())
        .fold(0, u64::saturating_add)
}

/// Whether the card can take `wanted` more bytes and still leave the stock
/// reader's library room to grow.
///
/// A filesystem that cannot be interrogated is treated as having room. The
/// alternative is refusing every write on a device whose `statvfs` is not what
/// we expected, which turns an unknown into an outage.
fn room_for(root: &Path, wanted: u64) -> bool {
    // Up to the nearest directory that exists. `root` is created by the first
    // write, and the first write is exactly when this check matters -- asking
    // about a path that is not there yet answers "cannot tell", which would
    // have quietly disabled the reserve for every application's first book.
    let mut here = Some(root);
    while let Some(path) = here {
        if let Some(free) = kobo_abi::free_space(path) {
            return free.saturating_sub(wanted) >= SHELF_RESERVE;
        }
        here = path.parent();
    }
    true
}

fn path_for(root: &Path, name: &str) -> PathBuf {
    root.join(name)
}

fn partial_path(root: &Path, name: &str) -> PathBuf {
    root.join(format!(".{name}.writing"))
}

/// A blob's size, or `None` if there is no such file.
fn size_of(path: &Path) -> Option<u64> {
    let metadata = fs::metadata(path).ok()?;
    metadata.is_file().then_some(metadata.len())
}

fn append(partial: &Path, offset: u32, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(partial)?;
    // Sought rather than appended, so a file that somehow grew past the offset
    // the caller believes it is at is cut back to that offset instead of
    // being written past. The two cannot disagree after this.
    file.seek(SeekFrom::Start(u64::from(offset)))?;
    file.write_all(bytes)?;
    Ok(())
}

fn publish(partial: &Path, final_path: &Path) -> std::io::Result<()> {
    {
        let file = fs::File::open(partial)?;
        // Before the rename, not after: a rename that lands before the bytes
        // do leaves a correctly named book full of zeroes, and this device is
        // reset by a hardware watchdog with nothing flushed.
        file.sync_all()?;
    }
    fs::rename(partial, final_path)?;
    if let Some(directory) = final_path.parent() {
        if let Ok(handle) = fs::File::open(directory) {
            let _ignored = handle.sync_all();
        }
    }
    Ok(())
}

fn read_at(path: &Path, offset: u64, into: &mut [u8]) -> std::io::Result<()> {
    use std::io::Read;
    let mut file = fs::File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    file.read_exact(into)
}

/// A size as it goes on the wire.
///
/// Saturating rather than wrapping: a blob larger than four gigabytes cannot
/// exist here — [`MAX_SHELF_BYTES`] is a quarter of that — but a size that
/// wrapped would report a huge file as a tiny one, and a caller would stop
/// reading it at the wrong place and believe it had the whole thing.
fn clamp_size(size: u64) -> u32 {
    u32::try_from(size).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_root() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "kobo-shelf-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ignored = fs::remove_dir_all(&root);
        root
    }

    fn write(shelf: &Shelf, name: &str, offset: u32, bytes: &[u8], last: bool) -> StoreResult {
        shelf
            .handle(&StoreRequest::ShelfWrite {
                name: name.into(),
                offset,
                bytes: bytes.to_vec(),
                last,
            })
            .expect("a shelf request")
    }

    fn read(shelf: &Shelf, name: &str, offset: u32, length: u32) -> StoreResult {
        shelf
            .handle(&StoreRequest::ShelfRead {
                name: name.into(),
                offset,
                length,
            })
            .expect("a shelf request")
    }

    #[test]
    fn a_blob_written_in_pieces_reads_back_whole() {
        let shelf = Shelf::new(temporary_root());
        write(&shelf, "book.txt", 0, b"It is a truth ", false);
        write(&shelf, "book.txt", 14, b"universally ", false);
        assert_eq!(
            write(&shelf, "book.txt", 26, b"acknowledged", true),
            StoreResult::ShelfWritten {
                name: "book.txt".into(),
                size: 38,
            }
        );
        assert_eq!(
            read(&shelf, "book.txt", 0, 1024),
            StoreResult::ShelfRead {
                name: "book.txt".into(),
                offset: 0,
                bytes: b"It is a truth universally acknowledged".to_vec(),
                size: 38,
            }
        );
    }

    #[test]
    fn an_unfinished_blob_cannot_be_read_at_all() {
        // The whole point of the partial name. A book that opens and stops in
        // the middle of a sentence is indistinguishable, to the person reading
        // it, from a book that was always like that.
        let shelf = Shelf::new(temporary_root());
        write(&shelf, "book.txt", 0, b"chapter one", false);
        assert_eq!(
            read(&shelf, "book.txt", 0, 1024),
            StoreResult::Denied(StoreError::Missing)
        );
        assert_eq!(
            shelf.handle(&StoreRequest::ShelfList),
            Some(StoreResult::Shelf(Vec::new()))
        );
    }

    #[test]
    fn a_piece_that_would_leave_a_hole_is_refused() {
        let shelf = Shelf::new(temporary_root());
        write(&shelf, "book.txt", 0, b"aaaa", false);
        assert_eq!(
            write(&shelf, "book.txt", 9_000, b"bbbb", false),
            StoreResult::Denied(StoreError::Missing)
        );
        // And the piece that does line up still works, so the refusal did not
        // damage what was already there.
        assert_eq!(
            write(&shelf, "book.txt", 4, b"bbbb", true),
            StoreResult::ShelfWritten {
                name: "book.txt".into(),
                size: 8,
            }
        );
    }

    #[test]
    fn beginning_again_at_zero_discards_what_was_there() {
        // The recovery path after a dropped connection: the application does
        // not know how much of its last piece landed, and this is how it stops
        // needing to know.
        let shelf = Shelf::new(temporary_root());
        write(&shelf, "book.txt", 0, b"the wrong download", false);
        write(&shelf, "book.txt", 0, b"the right one", true);
        assert_eq!(
            read(&shelf, "book.txt", 0, 1024),
            StoreResult::ShelfRead {
                name: "book.txt".into(),
                offset: 0,
                bytes: b"the right one".to_vec(),
                size: 13,
            }
        );
    }

    #[test]
    fn a_finished_blob_can_be_replaced_without_being_removed_first() {
        let shelf = Shelf::new(temporary_root());
        write(&shelf, "book.txt", 0, b"first edition", true);
        write(&shelf, "book.txt", 0, b"second edition", true);
        assert_eq!(
            shelf.handle(&StoreRequest::ShelfList),
            Some(StoreResult::Shelf(vec![("book.txt".into(), 14)]))
        );
    }

    #[test]
    fn reading_at_the_very_end_is_an_empty_answer_not_a_refusal() {
        // So a caller walking a blob in fixed steps stops cleanly rather than
        // having to special-case its own last request.
        let shelf = Shelf::new(temporary_root());
        write(&shelf, "book.txt", 0, b"abcd", true);
        assert_eq!(
            read(&shelf, "book.txt", 4, 16),
            StoreResult::ShelfRead {
                name: "book.txt".into(),
                offset: 4,
                bytes: Vec::new(),
                size: 4,
            }
        );
        assert_eq!(
            read(&shelf, "book.txt", 5, 16),
            StoreResult::Denied(StoreError::Missing)
        );
    }

    #[test]
    fn a_read_past_the_middle_returns_only_what_is_there() {
        let shelf = Shelf::new(temporary_root());
        write(&shelf, "book.txt", 0, b"abcdefgh", true);
        assert_eq!(
            read(&shelf, "book.txt", 6, 1024),
            StoreResult::ShelfRead {
                name: "book.txt".into(),
                offset: 6,
                bytes: b"gh".to_vec(),
                size: 8,
            }
        );
    }

    #[test]
    fn a_name_that_could_escape_the_directory_is_refused() {
        let shelf = Shelf::new(temporary_root());
        for name in ["../escape", "/etc/passwd", ".hidden", "", "Book.txt"] {
            assert_eq!(
                write(&shelf, name, 0, b"x", true),
                StoreResult::Denied(StoreError::BadKey),
                "{name:?} was accepted as a blob name"
            );
            assert_eq!(
                read(&shelf, name, 0, 1),
                StoreResult::Denied(StoreError::BadKey),
                "{name:?} was accepted as a blob name to read"
            );
        }
    }

    #[test]
    fn removing_takes_the_half_written_copy_too() {
        // Otherwise a caller who asked for something to be gone is still
        // paying for it and cannot see it to ask again.
        let root = temporary_root();
        let shelf = Shelf::new(&root);
        write(&shelf, "book.txt", 0, b"half a book", false);
        assert_eq!(
            shelf.handle(&StoreRequest::ShelfRemove {
                name: "book.txt".into()
            }),
            Some(StoreResult::ShelfRemoved {
                name: "book.txt".into()
            })
        );
        assert_eq!(usage(&root), 0, "a partial survived the removal");
    }

    #[test]
    fn removing_something_that_was_never_there_is_a_success() {
        let shelf = Shelf::new(temporary_root());
        assert_eq!(
            shelf.handle(&StoreRequest::ShelfRemove {
                name: "gone.txt".into()
            }),
            Some(StoreResult::ShelfRemoved {
                name: "gone.txt".into()
            })
        );
    }

    #[test]
    fn a_shelf_with_nowhere_to_write_refuses_rather_than_pretends() {
        let shelf = Shelf::unavailable();
        assert_eq!(
            write(&shelf, "book.txt", 0, b"x", true),
            StoreResult::Denied(StoreError::Unwritable)
        );
        assert_eq!(
            shelf.handle(&StoreRequest::ShelfList),
            Some(StoreResult::Denied(StoreError::Unwritable))
        );
    }

    #[test]
    fn an_ordinary_store_request_is_not_the_shelfs_to_answer() {
        // Including on an unavailable shelf, which is the case that would
        // otherwise swallow every ordinary request at the routing site.
        for shelf in [Shelf::new(temporary_root()), Shelf::unavailable()] {
            assert!(shelf.handle(&StoreRequest::List).is_none());
            assert!(shelf
                .handle(&StoreRequest::Load { key: "a".into() })
                .is_none());
        }
    }

    #[test]
    fn a_chunk_over_the_ceiling_is_refused() {
        let shelf = Shelf::new(temporary_root());
        assert_eq!(
            write(&shelf, "book.txt", 0, &vec![0; MAX_SHELF_CHUNK + 1], false),
            StoreResult::Denied(StoreError::TooFull)
        );
    }

    #[test]
    fn the_list_is_sorted_and_carries_sizes() {
        let shelf = Shelf::new(temporary_root());
        write(&shelf, "zeta.txt", 0, b"zz", true);
        write(&shelf, "alpha.txt", 0, b"a", true);
        write(&shelf, "midway.txt", 0, b"mmm", false);
        assert_eq!(
            shelf.handle(&StoreRequest::ShelfList),
            Some(StoreResult::Shelf(vec![
                ("alpha.txt".into(), 1),
                ("zeta.txt".into(), 2),
            ]))
        );
    }

    #[test]
    fn a_seek_past_the_end_cannot_be_used_to_claim_free_space() {
        // `append` seeks, and a seek past the end of a file makes a sparse
        // hole. The offset check is what stops that being reachable, so this
        // asserts the check rather than trusting it.
        let root = temporary_root();
        let shelf = Shelf::new(&root);
        write(&shelf, "book.txt", 0, b"a", false);
        assert_eq!(
            write(&shelf, "book.txt", u32::MAX, b"b", true),
            StoreResult::Denied(StoreError::Missing)
        );
        assert_eq!(usage(&root), 1);
    }

    #[test]
    fn the_card_keeps_a_reserve_the_shelf_cannot_spend() {
        // Not a mock: this asks the real filesystem the checkout is on, which
        // is the same call the device makes. Asking for more than the whole
        // card must be refused whatever card it is.
        let root = temporary_root();
        assert!(
            room_for(&root, 1),
            "one byte was refused on a filesystem with a checkout on it"
        );
        assert!(
            !room_for(&root, u64::MAX),
            "the reserve did not stop a write larger than the card"
        );
    }

    #[test]
    fn the_reserve_applies_before_the_directory_exists() {
        // The first write is what creates the directory, so a check that only
        // worked on an existing one would be off for every application's first
        // and largest download.
        let root = temporary_root();
        assert!(!root.exists(), "the fixture left a directory behind");
        assert!(
            !room_for(&root, u64::MAX),
            "the reserve was skipped because the directory was not there yet"
        );
    }

    #[test]
    fn a_filesystem_that_cannot_be_measured_at_all_is_not_treated_as_full() {
        // An unknown must not become an outage: refusing every write on a
        // device whose statvfs is unusual is worse than the problem. A NUL
        // cannot be passed to the kernel, and neither can its parent.
        assert!(room_for(Path::new("a\u{0}b"), 1));
    }
}

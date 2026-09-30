//! Directory durability: making a file's *name* survive a power cut.
//!
//! Writing and syncing a file is not enough for an atomic rename to be
//! durable; the directory holding the name must be flushed too. Unix does
//! this by opening the directory and fsyncing it. Windows refuses a plain
//! `File::open` on a directory, so the Windows arm opens it with
//! `FILE_FLAG_BACKUP_SEMANTICS` - the flag that exists exactly so tools can
//! hold a directory handle - and `FlushFileBuffers` on that handle flushes
//! the directory's metadata the way fsync does.
//!
//! One implementation, shared by kobo-policy and kobo-cli, so no caller
//! carries a quieter platform arm.

use std::fs;
use std::io;
use std::path::Path;

/// Opens a directory for flushing.
///
/// `FlushFileBuffers` refuses a read-only directory handle with
/// `ERROR_ACCESS_DENIED`, so the handle asks for write access as well.
#[cfg(windows)]
fn open_directory(path: &Path) -> io::Result<fs::File> {
    use std::os::windows::fs::OpenOptionsExt as _;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
}

/// Opens a directory for flushing.
#[cfg(unix)]
fn open_directory(path: &Path) -> io::Result<fs::File> {
    fs::File::open(path)
}

/// Flushes a directory's metadata, so names created or renamed inside it
/// survive a crash.
///
/// # Errors
///
/// Returns the error from opening or flushing the directory.
pub fn sync_directory(path: &Path) -> io::Result<()> {
    open_directory(path)?.sync_all()
}

/// Flushes an existing file's contents.
///
/// `FlushFileBuffers` refuses a read-only handle with `ERROR_ACCESS_DENIED`,
/// so the Windows arm opens the file for writing as well; nothing is
/// written, but the access right is what the flush call checks.
///
/// # Errors
///
/// Returns the error from opening or flushing the file.
pub fn sync_file(path: &Path) -> io::Result<()> {
    #[cfg(windows)]
    let file = fs::OpenOptions::new().read(true).write(true).open(path)?;
    #[cfg(unix)]
    let file = fs::File::open(path)?;
    file.sync_all()
}

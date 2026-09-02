//! Applies a published update to the installation on the book partition.
//!
//! The archive a release publishes is the same `KoboRoot.tgz` a person would
//! copy over USB, and every path in it lives under the installation folder on
//! the FAT32 book partition. Nothing here writes anywhere else, which is what
//! makes an update safe to apply on a running reader: the worst possible
//! outcome is a broken folder that the previous copy, kept beside it, undoes.
//!
//! The sequence is deliberate. The archive is fetched whole and verified
//! against its published digest before a single byte lands on disk. It is
//! then unpacked next to the installation, not over it, and only a complete
//! unpack is swapped in. Owner data is held outside all three versioned trees
//! while a durable direction journal makes every rename restartable.

use kobo_protocol::DeviceError;
use std::fs;
use std::io::Write;
use std::path::{Component, Path};

/// Where an update may write, as recorded inside the archive. The same
/// invariant the packager enforces on the way in is enforced again here on
/// the way out, so a doctored archive cannot reach the rest of the device.
const PREFIX: &str = "mnt/onboard/.adds/cobalt";

/// The folder that holds the installation, the staging copy and the previous
/// copy on a real reader.
#[cfg(feature = "device-write")]
const ADDS: &str = "/mnt/onboard/.adds";

/// The most compressed bytes a release is allowed to be. The real artifact is
/// a few megabytes; a reply ten times that size is not the artifact.
#[cfg(feature = "device-write")]
const ARCHIVE_LIMIT: u32 = 32 * 1024 * 1024;

/// The most the archive may expand to. The device has half a gigabyte of
/// memory in total, so the unpacked tree is held to a fraction of it.
const EXPANDED_LIMIT: u32 = 128 * 1024 * 1024;

/// One tar header or payload block.
const BLOCK: usize = 512;

/// Downloads a release archive, verifies it, and swaps it in.
///
/// # Errors
///
/// [`DeviceError::Integrity`] when the download does not match `sha256`,
/// transport errors translated as the audio streamer translates them, and
/// [`DeviceError::Backend`] when the book partition refuses a write.
#[cfg(feature = "device-write")]
pub fn apply(url: &str, sha256: &str) -> Result<(), DeviceError> {
    let archive = kobo_net::fetch(url, ARCHIVE_LIMIT).map_err(|error| match error {
        kobo_protocol::TaskError::Offline | kobo_protocol::TaskError::Unreachable => {
            DeviceError::Unreachable
        }
        kobo_protocol::TaskError::TimedOut => DeviceError::TimedOut,
        kobo_protocol::TaskError::NotFound => DeviceError::NotFound,
        kobo_protocol::TaskError::TooLarge | kobo_protocol::TaskError::Denied => {
            DeviceError::InvalidInput
        }
        kobo_protocol::TaskError::NoCredential | kobo_protocol::TaskError::Unauthorized => {
            DeviceError::Authentication
        }
    })?;
    install(&archive, sha256, Path::new(ADDS))
}

/// Verifies `archive` against `sha256` and installs it under `adds`.
///
/// The staging copy is written to `adds/cobalt.next`, and only after every
/// member has been written is it renamed into place. The copy it replaces is
/// kept at `adds/cobalt.prev` so a bad release can be undone by hand.
fn install(archive: &[u8], sha256: &str, adds: &Path) -> Result<(), DeviceError> {
    if kobo_net::sha256::hex_digest(archive) != sha256 {
        return Err(DeviceError::Integrity);
    }
    // The digest matched, so these bytes are exactly what was published. A
    // failure past this point means the release itself is malformed, which is
    // an input problem, not a transport or a disk problem.
    let tar =
        kobo_net::gzip::expand(archive, EXPANDED_LIMIT).map_err(|_| DeviceError::InvalidInput)?;
    recover_interrupted_update(adds)?;
    let staging = adds.join("cobalt.next");
    if staging.exists() {
        fs::remove_dir_all(&staging).map_err(|_| DeviceError::Backend)?;
    }
    let unpacked = unpack(&tar, &staging);
    if unpacked.is_err() {
        // A half-written staging folder is not left behind to be mistaken
        // for progress by the next attempt.
        let _ignored = fs::remove_dir_all(&staging);
    }
    unpacked?;
    if has_owner_folders(&staging) {
        let _ignored = fs::remove_dir_all(&staging);
        return Err(DeviceError::Backend);
    }
    swap(adds, &staging)
}

/// Writes every member of `tar` under `staging`, refusing anything that is
/// not a plain file or folder inside the installation prefix.
fn unpack(tar: &[u8], staging: &Path) -> Result<(), DeviceError> {
    let mut members = 0usize;
    let mut offset = 0usize;
    while offset + BLOCK <= tar.len() {
        let block = &tar[offset..offset + BLOCK];
        if block.iter().all(|&byte| byte == 0) {
            break;
        }
        verify_checksum(block)?;
        let path = read_string(&block[0..100]);
        let size = read_octal(&block[124..136])?;
        let size = usize::try_from(size).map_err(|_| DeviceError::InvalidInput)?;
        let kind = block[156];
        let relative = match installed_path(&path) {
            Some(relative) => relative,
            // A general-purpose packager describes the folders above the
            // install root too. They already exist on a reader and nothing
            // is written for them, but they are not grounds to refuse the
            // release either.
            None if kind == b'5' && names_folder_above_prefix(&path) => {
                offset += BLOCK;
                continue;
            }
            None => return Err(DeviceError::InvalidInput),
        };
        let payload = match kind {
            b'5' => 0,
            b'0' => size,
            // A symbolic link, a hard link or a device node has no business
            // in this archive, and unpacked as root they are exactly the
            // members an attacker would want.
            _ => return Err(DeviceError::InvalidInput),
        };
        let end = offset
            .checked_add(BLOCK)
            .and_then(|start| start.checked_add(payload))
            .ok_or(DeviceError::InvalidInput)?;
        if end > tar.len() {
            return Err(DeviceError::InvalidInput);
        }
        let destination = staging.join(relative);
        if kind == b'5' {
            fs::create_dir_all(&destination).map_err(|_| DeviceError::Backend)?;
        } else {
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent).map_err(|_| DeviceError::Backend)?;
            }
            fs::write(&destination, &tar[offset + BLOCK..end]).map_err(|_| DeviceError::Backend)?;
            set_mode(&destination, &block[100..108]);
        }
        members += 1;
        offset = end.div_ceil(BLOCK) * BLOCK;
    }
    if members == 0 {
        return Err(DeviceError::InvalidInput);
    }
    sync_tree(staging)?;
    Ok(())
}

/// Retires the current installation and moves the staged one into place.
///
/// Owner folders are first moved to a transaction holder beside every
/// versioned tree. A durable direction marker makes each following rename
/// restartable, including when power disappears between two owner folders.
fn swap(adds: &Path, staging: &Path) -> Result<(), DeviceError> {
    swap_with_fault(adds, staging, &mut |_| Ok(()))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TransactionStep {
    SetForward,
    HoldOwner(&'static str),
    RemovePrevious,
    RetireCurrent,
    PromoteStaging,
    RestoreOwner(&'static str),
    SetRollback,
    RollbackOwner(&'static str),
    RollbackNew,
    RollbackPrevious,
    RemoveHolder,
    ClearJournal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TransactionFailure {
    Backend,
    #[allow(dead_code, reason = "constructed by fault-injection tests")]
    Interrupted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Direction {
    Forward,
    Rollback,
}

const OWNER_HOLDER: &str = "cobalt.owner";
const JOURNAL: &str = ".cobalt-update-transaction";
const JOURNAL_TEMPORARY: &str = ".cobalt-update-transaction.new";

/// The complete update transaction with a power-loss boundary injected by
/// tests. An interruption deliberately skips in-process rollback: the next
/// normal startup must recover the durable state on disk.
fn swap_with_fault(
    adds: &Path,
    staging: &Path,
    step: &mut impl FnMut(TransactionStep) -> Result<(), TransactionFailure>,
) -> Result<(), DeviceError> {
    if staging != adds.join("cobalt.next") {
        return Err(DeviceError::InvalidInput);
    }
    if let Err(error) = write_direction(adds, Direction::Forward, step) {
        return Err(device_error(error));
    }
    match recover_forward(adds, step) {
        Ok(()) => Ok(()),
        Err(TransactionFailure::Interrupted) => Err(DeviceError::Backend),
        Err(TransactionFailure::Backend) => {
            write_direction(adds, Direction::Rollback, step).map_err(device_error)?;
            recover_rollback(adds, step).map_err(device_error)?;
            Err(DeviceError::Backend)
        }
    }
}

fn recover_forward(
    adds: &Path,
    step: &mut impl FnMut(TransactionStep) -> Result<(), TransactionFailure>,
) -> Result<(), TransactionFailure> {
    let current = adds.join("cobalt");
    let staging = adds.join("cobalt.next");
    let previous = adds.join("cobalt.prev");
    let holder = adds.join(OWNER_HOLDER);

    // No staging name means promotion already happened. Never collect from
    // current in that topology: those are the owner folders being restored.
    if !staging.exists() {
        restore_owner_folders(&holder, &current, TransactionStep::RestoreOwner, step)?;
        remove_empty_holder(adds, step)?;
        clear_journal(adds, step)?;
        return Ok(());
    }

    private_holder(&holder)?;
    if current.exists() {
        move_owner_folders(&current, &holder, TransactionStep::HoldOwner, step)?;
    }
    if previous.exists() && current.exists() {
        step(TransactionStep::RemovePrevious)?;
        remove_durable(&previous)?;
    }
    if current.exists() {
        step(TransactionStep::RetireCurrent)?;
        rename_durable(&current, &previous)?;
    }
    step(TransactionStep::PromoteStaging)?;
    rename_durable(&staging, &current)?;
    restore_owner_folders(&holder, &current, TransactionStep::RestoreOwner, step)?;
    remove_empty_holder(adds, step)?;
    clear_journal(adds, step)?;
    Ok(())
}

fn recover_rollback(
    adds: &Path,
    step: &mut impl FnMut(TransactionStep) -> Result<(), TransactionFailure>,
) -> Result<(), TransactionFailure> {
    let current = adds.join("cobalt");
    let staging = adds.join("cobalt.next");
    let previous = adds.join("cobalt.prev");
    let holder = adds.join(OWNER_HOLDER);

    if !previous.exists() && !staging.exists() {
        // A first installation has no older tree to restore. Finishing the
        // verified installation is the only rollback that leaves a launcher.
        write_direction(adds, Direction::Forward, step)?;
        return recover_forward(adds, step);
    }

    if !staging.exists() && current.exists() {
        private_holder(&holder)?;
        move_owner_folders(&current, &holder, TransactionStep::RollbackOwner, step)?;
        step(TransactionStep::RollbackNew)?;
        rename_durable(&current, &staging)?;
    }
    if !current.exists() {
        if !previous.exists() {
            return Err(TransactionFailure::Backend);
        }
        step(TransactionStep::RollbackPrevious)?;
        rename_durable(&previous, &current)?;
    }
    restore_owner_folders(&holder, &current, TransactionStep::RollbackOwner, step)?;
    remove_empty_holder(adds, step)?;
    clear_journal(adds, step)?;
    Ok(())
}

/// Recovers a durable OTA transaction before a runtime is allowed to launch.
///
/// This is called both before applying an update and by normal daemon startup.
pub fn recover_at_startup(adds: &Path) -> Result<(), DeviceError> {
    recover_interrupted_update(adds)
}

/// Recovers owner data from interrupted current/next/prev states before a
/// retry may discard staging or an old rollback tree.
fn recover_interrupted_update(adds: &Path) -> Result<(), DeviceError> {
    if !adds.exists() {
        return Ok(());
    }
    if let Some(direction) = read_direction(adds)? {
        let mut uninterrupted = |_| Ok(());
        return match direction {
            Direction::Forward => recover_forward(adds, &mut uninterrupted),
            Direction::Rollback => recover_rollback(adds, &mut uninterrupted),
        }
        .map_err(device_error);
    }

    // Pre-journal releases may have died after promotion and while moving
    // owner folders. Recover those layouts once, without deleting either side
    // of an ambiguous conflict.
    let current = adds.join("cobalt");
    let staging = adds.join("cobalt.next");
    let previous = adds.join("cobalt.prev");
    let holder = adds.join(OWNER_HOLDER);
    if !current.exists() && previous.exists() {
        rename_durable(&previous, &current).map_err(device_error)?;
    }
    if current.exists() {
        let mut uninterrupted = |_| Ok(());
        restore_owner_folders(
            &holder,
            &current,
            TransactionStep::RollbackOwner,
            &mut uninterrupted,
        )
        .map_err(device_error)?;
        restore_owner_folders(
            &previous,
            &current,
            TransactionStep::RestoreOwner,
            &mut uninterrupted,
        )
        .map_err(device_error)?;
        restore_owner_folders(
            &staging,
            &current,
            TransactionStep::RestoreOwner,
            &mut uninterrupted,
        )
        .map_err(device_error)?;
        remove_empty_holder(adds, &mut uninterrupted).map_err(device_error)?;
    } else if has_owner_folders(&staging) || has_owner_folders(&holder) {
        return Err(DeviceError::Backend);
    }
    Ok(())
}

fn has_owner_folders(path: &Path) -> bool {
    OWNER_FOLDERS
        .iter()
        .any(|folder| path.join(folder).exists())
}

/// What the owner put on the reader, as opposed to what a release ships:
/// installed trust roots, secrets, application state and application data. A
/// release archive never carries these folders, so an update carries them
/// forward or the reader forgets everything it was trusted with.
const OWNER_FOLDERS: [&str; 6] = ["secrets", "trust", "state", "data", "apps", "store"];

fn move_owner_folders(
    from: &Path,
    to: &Path,
    operation: fn(&'static str) -> TransactionStep,
    step: &mut impl FnMut(TransactionStep) -> Result<(), TransactionFailure>,
) -> Result<(), TransactionFailure> {
    for folder in OWNER_FOLDERS {
        let source = from.join(folder);
        if !source.exists() {
            continue;
        }
        let destination = to.join(folder);
        if destination.exists() {
            return Err(TransactionFailure::Backend);
        }
        step(operation(folder))?;
        rename_durable(&source, &destination)?;
    }
    Ok(())
}

fn restore_owner_folders(
    from: &Path,
    to: &Path,
    operation: fn(&'static str) -> TransactionStep,
    step: &mut impl FnMut(TransactionStep) -> Result<(), TransactionFailure>,
) -> Result<(), TransactionFailure> {
    if !from.exists() {
        return Ok(());
    }
    if !to.exists() {
        return Err(TransactionFailure::Backend);
    }
    for folder in OWNER_FOLDERS {
        let source = from.join(folder);
        if !source.exists() {
            continue;
        }
        let destination = to.join(folder);
        if destination.exists() {
            return Err(TransactionFailure::Backend);
        }
        step(operation(folder))?;
        rename_durable(&source, &destination)?;
    }
    Ok(())
}

fn private_holder(holder: &Path) -> Result<(), TransactionFailure> {
    match fs::symlink_metadata(holder) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(holder).map_err(|_| TransactionFailure::Backend)?;
            sync_directory(holder.parent().ok_or(TransactionFailure::Backend)?)
                .map_err(|_| TransactionFailure::Backend)
        }
        Ok(_) | Err(_) => Err(TransactionFailure::Backend),
    }
}

fn remove_empty_holder(
    adds: &Path,
    step: &mut impl FnMut(TransactionStep) -> Result<(), TransactionFailure>,
) -> Result<(), TransactionFailure> {
    let holder = adds.join(OWNER_HOLDER);
    if !holder.exists() {
        return Ok(());
    }
    if fs::read_dir(&holder)
        .map_err(|_| TransactionFailure::Backend)?
        .next()
        .is_some()
    {
        return Err(TransactionFailure::Backend);
    }
    step(TransactionStep::RemoveHolder)?;
    fs::remove_dir(&holder).map_err(|_| TransactionFailure::Backend)?;
    sync_directory(adds).map_err(|_| TransactionFailure::Backend)
}

fn rename_durable(from: &Path, to: &Path) -> Result<(), TransactionFailure> {
    fs::rename(from, to).map_err(|_| TransactionFailure::Backend)?;
    let from_parent = from.parent().ok_or(TransactionFailure::Backend)?;
    let to_parent = to.parent().ok_or(TransactionFailure::Backend)?;
    sync_directory(from_parent).map_err(|_| TransactionFailure::Backend)?;
    if to_parent != from_parent {
        sync_directory(to_parent).map_err(|_| TransactionFailure::Backend)?;
    }
    Ok(())
}

fn remove_durable(path: &Path) -> Result<(), TransactionFailure> {
    fs::remove_dir_all(path).map_err(|_| TransactionFailure::Backend)?;
    sync_directory(path.parent().ok_or(TransactionFailure::Backend)?)
        .map_err(|_| TransactionFailure::Backend)
}

fn write_direction(
    adds: &Path,
    direction: Direction,
    step: &mut impl FnMut(TransactionStep) -> Result<(), TransactionFailure>,
) -> Result<(), TransactionFailure> {
    step(match direction {
        Direction::Forward => TransactionStep::SetForward,
        Direction::Rollback => TransactionStep::SetRollback,
    })?;
    let temporary = adds.join(JOURNAL_TEMPORARY);
    let journal = adds.join(JOURNAL);
    if temporary.exists() {
        fs::remove_file(&temporary).map_err(|_| TransactionFailure::Backend)?;
    }
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(|_| TransactionFailure::Backend)?;
    file.write_all(match direction {
        Direction::Forward => b"forward\n",
        Direction::Rollback => b"rollback\n",
    })
    .map_err(|_| TransactionFailure::Backend)?;
    file.sync_all().map_err(|_| TransactionFailure::Backend)?;
    fs::rename(&temporary, &journal).map_err(|_| TransactionFailure::Backend)?;
    sync_directory(adds).map_err(|_| TransactionFailure::Backend)
}

fn read_direction(adds: &Path) -> Result<Option<Direction>, DeviceError> {
    let journal = adds.join(JOURNAL);
    let contents = match fs::read(&journal) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(DeviceError::Backend),
    };
    match contents.as_slice() {
        b"forward\n" => Ok(Some(Direction::Forward)),
        b"rollback\n" => Ok(Some(Direction::Rollback)),
        _ => Err(DeviceError::Backend),
    }
}

fn clear_journal(
    adds: &Path,
    step: &mut impl FnMut(TransactionStep) -> Result<(), TransactionFailure>,
) -> Result<(), TransactionFailure> {
    step(TransactionStep::ClearJournal)?;
    match fs::remove_file(adds.join(JOURNAL)) {
        Ok(()) => sync_directory(adds).map_err(|_| TransactionFailure::Backend),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(TransactionFailure::Backend),
    }
}

fn sync_directory(path: &Path) -> Result<(), DeviceError> {
    fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| DeviceError::Backend)
}

fn sync_tree(path: &Path) -> Result<(), DeviceError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| DeviceError::Backend)?;
    if metadata.file_type().is_symlink() {
        return Err(DeviceError::Backend);
    }
    if metadata.is_file() {
        return fs::File::open(path)
            .and_then(|file| file.sync_all())
            .map_err(|_| DeviceError::Backend);
    }
    for entry in fs::read_dir(path).map_err(|_| DeviceError::Backend)? {
        sync_tree(&entry.map_err(|_| DeviceError::Backend)?.path())?;
    }
    sync_directory(path)
}

fn device_error(_: TransactionFailure) -> DeviceError {
    DeviceError::Backend
}

/// Returns the path relative to the installation folder, or `None` for a
/// member that names anything outside it.
fn installed_path(path: &str) -> Option<&Path> {
    let rest = path.strip_prefix(PREFIX)?;
    // "cobalt-else/…" also survives the prefix strip; only the folder itself
    // or something inside it may pass.
    if !rest.is_empty() && !rest.starts_with('/') {
        return None;
    }
    let candidate = Path::new(rest.trim_start_matches('/'));
    // The prefix guarantees where the member claims to live; this guarantees
    // it cannot climb back out of it.
    if candidate
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        Some(candidate)
    } else {
        None
    }
}

/// Returns whether `path` names one of the folders the installation prefix
/// sits inside, such as `mnt/` or `mnt/onboard/.adds/`.
fn names_folder_above_prefix(path: &str) -> bool {
    let trimmed = path.trim_end_matches('/');
    !trimmed.is_empty()
        && PREFIX
            .strip_prefix(trimmed)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Applies the executable bits a member carries, where the filesystem has
/// them to apply. The book partition is FAT32 and has none, so failure here
/// is the expected case on a reader and is not reported. Only the plain
/// permission bits are taken: setuid, setgid and the sticky bit are nothing
/// an application archive has any business carrying.
fn set_mode(path: &Path, field: &[u8]) {
    #[cfg(unix)]
    if let Ok(mode) = read_octal(field) {
        use std::os::unix::fs::PermissionsExt;
        let mode = u32::try_from(mode & 0o777).unwrap_or(0o644);
        let _ignored = fs::set_permissions(path, fs::Permissions::from_mode(mode));
    }
    #[cfg(not(unix))]
    let _ignored = (path, field);
}

fn verify_checksum(block: &[u8]) -> Result<(), DeviceError> {
    let stated = read_octal(&block[148..156])?;
    let computed: u64 = block
        .iter()
        .enumerate()
        .map(|(index, &byte)| {
            if (148..156).contains(&index) {
                u64::from(b' ')
            } else {
                u64::from(byte)
            }
        })
        .sum();
    if computed == stated {
        Ok(())
    } else {
        Err(DeviceError::InvalidInput)
    }
}

fn read_string(field: &[u8]) -> String {
    let end = field
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).into_owned()
}

fn read_octal(field: &[u8]) -> Result<u64, DeviceError> {
    let text = read_string(field);
    let digits = text.trim_matches(|character: char| character == ' ' || character == '\0');
    if digits.is_empty() {
        return Ok(0);
    }
    u64::from_str_radix(digits, 8).map_err(|_| DeviceError::InvalidInput)
}

#[cfg(test)]
mod tests {
    use super::{
        install, recover_interrupted_update, swap_with_fault, TransactionFailure, TransactionStep,
        JOURNAL, OWNER_FOLDERS, PREFIX,
    };
    use kobo_protocol::DeviceError;
    use std::fs;

    /// A tar member for the archives these tests publish.
    struct Member<'a> {
        path: String,
        kind: u8,
        payload: &'a [u8],
    }

    fn folder(path: &str) -> Member<'static> {
        Member {
            path: format!("{PREFIX}/{path}"),
            kind: b'5',
            payload: &[],
        }
    }

    fn file<'a>(path: &str, payload: &'a [u8]) -> Member<'a> {
        Member {
            path: format!("{PREFIX}/{path}"),
            kind: b'0',
            payload,
        }
    }

    fn tar(members: &[Member<'_>]) -> Vec<u8> {
        let mut archive = Vec::new();
        for member in members {
            let mut block = [0u8; 512];
            block[..member.path.len()].copy_from_slice(member.path.as_bytes());
            block[100..107].copy_from_slice(b"0000755");
            let size = format!("{:011o}", member.payload.len());
            block[124..135].copy_from_slice(size.as_bytes());
            block[156] = member.kind;
            block[148..156].copy_from_slice(b"        ");
            let sum: u64 = block.iter().map(|&byte| u64::from(byte)).sum();
            let checksum = format!("{sum:06o}\0 ");
            block[148..156].copy_from_slice(checksum.as_bytes());
            archive.extend_from_slice(&block);
            archive.extend_from_slice(member.payload);
            let padding = member.payload.len().div_ceil(512) * 512 - member.payload.len();
            archive.extend(std::iter::repeat_n(0u8, padding));
        }
        archive.extend(std::iter::repeat_n(0u8, 1024));
        archive
    }

    /// Wraps bytes in a gzip container using stored deflate blocks, which is
    /// all the tests need and keeps them free of a compressor.
    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut container = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 0xff];
        let mut chunks = bytes.chunks(0xffff).peekable();
        while let Some(chunk) = chunks.next() {
            let length = u16::try_from(chunk.len()).expect("chunked to fit");
            container.push(u8::from(chunks.peek().is_none()));
            container.extend_from_slice(&length.to_le_bytes());
            container.extend_from_slice(&(!length).to_le_bytes());
            container.extend_from_slice(chunk);
        }
        // The reader stops at the final deflate block, so the trailer only
        // has to be present.
        container.extend_from_slice(&[0u8; 8]);
        container
    }

    fn published(members: &[Member<'_>]) -> (Vec<u8>, String) {
        let archive = gzip(&tar(members));
        let digest = kobo_net::sha256::hex_digest(&archive);
        (archive, digest)
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let folder =
            std::env::temp_dir().join(format!("kobod-update-{name}-{}", std::process::id()));
        let _ignored = fs::remove_dir_all(&folder);
        fs::create_dir_all(&folder).expect("scratch folder");
        folder
    }

    #[test]
    fn a_verified_archive_is_unpacked_and_swapped_in() {
        let adds = scratch("swap");
        let (archive, digest) = published(&[
            folder(""),
            folder("bin"),
            file("bin/kobod", b"new daemon"),
            file("start.sh", b"#!/bin/sh\n"),
        ]);
        install(&archive, &digest, &adds).expect("install succeeds");
        let read = |path: &str| fs::read(adds.join("cobalt").join(path)).expect("installed file");
        assert_eq!(read("bin/kobod"), b"new daemon");
        assert_eq!(read("start.sh"), b"#!/bin/sh\n");
        assert!(!adds.join("cobalt.next").exists());
        let _ignored = fs::remove_dir_all(&adds);
    }

    #[test]
    fn the_replaced_installation_is_kept_beside_the_new_one() {
        let adds = scratch("previous");
        fs::create_dir_all(adds.join("cobalt")).expect("current installation");
        fs::write(adds.join("cobalt/start.sh"), b"old").expect("current file");
        let (archive, digest) = published(&[file("start.sh", b"new")]);
        install(&archive, &digest, &adds).expect("install succeeds");
        assert_eq!(
            fs::read(adds.join("cobalt/start.sh")).expect("new file"),
            b"new"
        );
        assert_eq!(
            fs::read(adds.join("cobalt.prev/start.sh")).expect("kept file"),
            b"old"
        );
        let _ignored = fs::remove_dir_all(&adds);
    }

    #[test]
    fn the_owners_folders_survive_an_update() {
        let adds = scratch("carry");
        fs::create_dir_all(adds.join("cobalt/trust")).expect("current trust");
        fs::write(adds.join("cobalt/trust/sidekick.pem"), b"PEM").expect("trust root");
        fs::create_dir_all(adds.join("cobalt/secrets")).expect("current secrets");
        fs::write(adds.join("cobalt/secrets/hn"), b"token").expect("secret");
        fs::write(adds.join("cobalt/start.sh"), b"old").expect("current file");
        let (archive, digest) = published(&[file("start.sh", b"new")]);
        install(&archive, &digest, &adds).expect("install succeeds");
        // The release replaced its own files and carried the owner's.
        assert_eq!(
            fs::read(adds.join("cobalt/start.sh")).expect("new file"),
            b"new"
        );
        assert_eq!(
            fs::read(adds.join("cobalt/trust/sidekick.pem")).expect("carried trust root"),
            b"PEM"
        );
        assert_eq!(
            fs::read(adds.join("cobalt/secrets/hn")).expect("carried secret"),
            b"token"
        );
        assert!(!adds.join("cobalt.prev/trust").exists());
        assert!(!adds.join("cobalt.prev/secrets").exists());
        let _ignored = fs::remove_dir_all(&adds);
    }

    fn transaction_fixture(name: &str) -> std::path::PathBuf {
        let adds = scratch(name);
        let current = adds.join("cobalt");
        let staging = adds.join("cobalt.next");
        fs::create_dir_all(&current).expect("current");
        fs::write(current.join("start.sh"), b"old").expect("old release");
        fs::create_dir_all(&staging).expect("staging");
        fs::write(staging.join("start.sh"), b"new").expect("new release");
        fs::create_dir_all(adds.join("cobalt.prev")).expect("stale previous");
        fs::write(adds.join("cobalt.prev/start.sh"), b"older").expect("older release");
        for folder in OWNER_FOLDERS {
            fs::create_dir_all(current.join(folder)).expect("owner folder");
            fs::write(current.join(folder).join("kept"), folder).expect("owner data");
        }
        adds
    }

    fn assert_owner_data(adds: &std::path::Path, release: &[u8]) {
        assert_eq!(
            fs::read(adds.join("cobalt/start.sh")).expect("active release"),
            release
        );
        for folder in OWNER_FOLDERS {
            assert_eq!(
                fs::read_to_string(adds.join("cobalt").join(folder).join("kept"))
                    .expect("active owner data"),
                folder,
                "{folder} was lost or attached to the wrong release"
            );
            for inactive in ["cobalt.next", "cobalt.prev", "cobalt.owner"] {
                assert!(
                    !adds.join(inactive).join(folder).exists(),
                    "{folder} remained split into {inactive}"
                );
            }
        }
        assert!(!adds.join(JOURNAL).exists(), "journal was not cleared");
    }

    #[test]
    fn startup_recovers_every_forward_rename_and_phase_boundary() {
        let trace_root = transaction_fixture("forward-trace");
        let mut boundaries = Vec::new();
        swap_with_fault(&trace_root, &trace_root.join("cobalt.next"), &mut |step| {
            boundaries.push(step);
            Ok(())
        })
        .expect("trace transaction");
        assert_owner_data(&trace_root, b"new");
        let _ignored = fs::remove_dir_all(trace_root);

        for (failure, boundary) in boundaries.iter().copied().enumerate() {
            let adds = transaction_fixture(&format!("forward-boundary-{failure}"));
            let mut seen = 0usize;
            assert_eq!(
                swap_with_fault(&adds, &adds.join("cobalt.next"), &mut |step| {
                    assert_eq!(
                        step, boundaries[seen],
                        "transaction changed before {boundary:?}"
                    );
                    let interrupt = seen == failure;
                    seen += 1;
                    if interrupt {
                        Err(TransactionFailure::Interrupted)
                    } else {
                        Ok(())
                    }
                }),
                Err(DeviceError::Backend),
                "boundary {boundary:?} did not interrupt"
            );
            recover_interrupted_update(&adds).expect("normal startup recovery");
            if boundary == TransactionStep::SetForward {
                assert_owner_data(&adds, b"old");
            } else {
                assert_owner_data(&adds, b"new");
            }
            let _ignored = fs::remove_dir_all(adds);
        }
    }

    #[test]
    fn startup_recovers_every_atomic_rollback_boundary() {
        let trace_root = transaction_fixture("rollback-trace");
        let mut rollback = false;
        let mut rollback_boundaries = Vec::new();
        assert_eq!(
            swap_with_fault(&trace_root, &trace_root.join("cobalt.next"), &mut |step| {
                if step == TransactionStep::RestoreOwner("secrets") && !rollback {
                    return Err(TransactionFailure::Backend);
                }
                if step == TransactionStep::SetRollback {
                    rollback = true;
                }
                if rollback {
                    rollback_boundaries.push(step);
                }
                Ok(())
            }),
            Err(DeviceError::Backend)
        );
        assert_owner_data(&trace_root, b"old");
        let _ignored = fs::remove_dir_all(trace_root);

        for (failure, boundary) in rollback_boundaries.iter().copied().enumerate() {
            let adds = transaction_fixture(&format!("rollback-boundary-{failure}"));
            let mut rollback = false;
            let mut seen = 0usize;
            assert_eq!(
                swap_with_fault(&adds, &adds.join("cobalt.next"), &mut |step| {
                    if step == TransactionStep::RestoreOwner("secrets") && !rollback {
                        return Err(TransactionFailure::Backend);
                    }
                    if step == TransactionStep::SetRollback {
                        rollback = true;
                    }
                    if rollback {
                        let interrupt = seen == failure;
                        seen += 1;
                        if interrupt {
                            return Err(TransactionFailure::Interrupted);
                        }
                    }
                    Ok(())
                }),
                Err(DeviceError::Backend),
                "rollback boundary {boundary:?} did not interrupt"
            );
            recover_interrupted_update(&adds).expect("normal startup rollback recovery");
            if boundary == TransactionStep::SetRollback {
                assert_owner_data(&adds, b"new");
            } else {
                assert_owner_data(&adds, b"old");
            }
            let _ignored = fs::remove_dir_all(adds);
        }
    }

    #[test]
    fn retry_restores_a_retired_installation_before_discarding_staging() {
        let adds = scratch("recover-retired");
        fs::create_dir_all(adds.join("cobalt.prev/state")).expect("retired state");
        fs::write(adds.join("cobalt.prev/state/session"), b"kept").expect("state");
        fs::write(adds.join("cobalt.prev/start.sh"), b"old").expect("old release");
        fs::create_dir_all(adds.join("cobalt.next")).expect("staging");
        fs::write(adds.join("cobalt.next/start.sh"), b"new").expect("new release");

        recover_interrupted_update(&adds).expect("recover interrupted retirement");

        assert_eq!(
            fs::read(adds.join("cobalt/state/session")).expect("active state"),
            b"kept"
        );
        assert_eq!(
            fs::read(adds.join("cobalt/start.sh")).expect("active release"),
            b"old"
        );
        assert!(adds.join("cobalt.next").exists());
        assert!(!adds.join("cobalt.prev").exists());
        let _ignored = fs::remove_dir_all(&adds);
    }

    #[test]
    fn retry_finishes_owner_transfer_after_promotion() {
        let adds = scratch("recover-promoted");
        fs::create_dir_all(adds.join("cobalt")).expect("new release");
        fs::write(adds.join("cobalt/start.sh"), b"new").expect("new file");
        fs::create_dir_all(adds.join("cobalt.prev/state")).expect("retired state");
        fs::write(adds.join("cobalt.prev/state/session"), b"kept").expect("state");

        recover_interrupted_update(&adds).expect("finish owner transfer");

        assert_eq!(
            fs::read(adds.join("cobalt/state/session")).expect("active state"),
            b"kept"
        );
        assert!(!adds.join("cobalt.prev/state").exists());
        let _ignored = fs::remove_dir_all(&adds);
    }

    #[test]
    fn retry_finishes_a_partially_completed_owner_transfer() {
        let adds = scratch("recover-partial-owner");
        fs::create_dir_all(adds.join("cobalt/state")).expect("moved state");
        fs::write(adds.join("cobalt/state/session"), b"state").expect("state");
        fs::create_dir_all(adds.join("cobalt.prev/data")).expect("retired data");
        fs::write(adds.join("cobalt.prev/data/cache"), b"data").expect("data");

        recover_interrupted_update(&adds).expect("finish partial transfer");

        assert_eq!(
            fs::read(adds.join("cobalt/state/session")).expect("state remains"),
            b"state"
        );
        assert_eq!(
            fs::read(adds.join("cobalt/data/cache")).expect("data restored"),
            b"data"
        );
        let _ignored = fs::remove_dir_all(&adds);
    }

    #[test]
    fn retry_recovers_owner_data_from_legacy_staging_before_cleanup() {
        let adds = scratch("recover-staged-owner");
        fs::create_dir_all(adds.join("cobalt")).expect("active release");
        fs::create_dir_all(adds.join("cobalt.next/store")).expect("staged store");
        fs::write(adds.join("cobalt.next/store/catalog"), b"kept").expect("store");

        recover_interrupted_update(&adds).expect("recover staged owner data");

        assert_eq!(
            fs::read(adds.join("cobalt/store/catalog")).expect("active store"),
            b"kept"
        );
        assert!(!adds.join("cobalt.next/store").exists());
        let _ignored = fs::remove_dir_all(&adds);
    }

    #[test]
    fn owner_data_without_a_recoverable_installation_is_never_deleted() {
        let adds = scratch("recover-orphaned-owner");
        fs::create_dir_all(adds.join("cobalt.next/secrets")).expect("staged secrets");
        fs::write(adds.join("cobalt.next/secrets/token"), b"kept").expect("secret");

        assert_eq!(recover_interrupted_update(&adds), Err(DeviceError::Backend));
        assert_eq!(
            fs::read(adds.join("cobalt.next/secrets/token")).expect("preserved secret"),
            b"kept"
        );
        let _ignored = fs::remove_dir_all(&adds);
    }

    #[test]
    fn a_release_cannot_replace_an_owner_folder_or_report_success() {
        let adds = scratch("owner-conflict");
        fs::create_dir_all(adds.join("cobalt/secrets")).expect("current secrets");
        fs::write(adds.join("cobalt/secrets/token"), b"kept").expect("secret");
        fs::write(adds.join("cobalt/start.sh"), b"old").expect("current release");
        let (archive, digest) = published(&[file("start.sh", b"new"), folder("secrets")]);
        assert_eq!(install(&archive, &digest, &adds), Err(DeviceError::Backend));
        assert_eq!(
            fs::read(adds.join("cobalt/secrets/token")).expect("current secret"),
            b"kept"
        );
        assert_eq!(
            fs::read(adds.join("cobalt/start.sh")).expect("current release"),
            b"old"
        );
        assert!(!adds.join("cobalt.prev").exists());
        let _ignored = fs::remove_dir_all(&adds);
    }

    #[test]
    fn a_download_that_does_not_match_its_digest_writes_nothing() {
        let adds = scratch("digest");
        let (archive, _) = published(&[file("start.sh", b"payload")]);
        let wrong = kobo_net::sha256::hex_digest(b"something else");
        assert_eq!(
            install(&archive, &wrong, &adds),
            Err(DeviceError::Integrity)
        );
        assert!(!adds.join("cobalt").exists());
        assert!(!adds.join("cobalt.next").exists());
        let _ignored = fs::remove_dir_all(&adds);
    }

    #[test]
    fn the_folders_above_the_prefix_are_tolerated_but_never_written() {
        let adds = scratch("above");
        // The shape `kobo package` used to publish: every ancestor folder
        // described before the payload.
        let above = |path: &str| Member {
            path: path.to_owned(),
            kind: b'5',
            payload: &[],
        };
        let (archive, digest) = published(&[
            above("mnt/"),
            above("mnt/onboard/"),
            above("mnt/onboard/.adds/"),
            folder(""),
            file("start.sh", b"#!/bin/sh\n"),
        ]);
        install(&archive, &digest, &adds).expect("install succeeds");
        assert_eq!(
            fs::read(adds.join("cobalt/start.sh")).expect("installed file"),
            b"#!/bin/sh\n"
        );
        // Tolerated means skipped: nothing above the prefix appears in the
        // staging area or beside it.
        assert!(!adds.join("mnt").exists());
        assert!(!adds.join("onboard").exists());
        let _ignored = fs::remove_dir_all(&adds);
    }

    #[test]
    fn a_file_above_the_prefix_is_still_refused() {
        let adds = scratch("above-file");
        let stray = Member {
            path: "mnt/onboard/.adds/".to_owned(),
            kind: b'0',
            payload: b"tampered",
        };
        let (archive, digest) = published(&[file("start.sh", b"fine"), stray]);
        assert_eq!(
            install(&archive, &digest, &adds),
            Err(DeviceError::InvalidInput)
        );
        assert!(!adds.join("cobalt").exists());
        let _ignored = fs::remove_dir_all(&adds);
    }

    #[test]
    fn a_member_outside_the_installation_prefix_is_refused() {
        let adds = scratch("outside");
        let stray = Member {
            path: "mnt/onboard/.kobo/Kobo/Kobo eReader.conf".to_owned(),
            kind: b'0',
            payload: b"tampered",
        };
        let (archive, digest) = published(&[file("start.sh", b"fine"), stray]);
        assert_eq!(
            install(&archive, &digest, &adds),
            Err(DeviceError::InvalidInput)
        );
        assert!(!adds.join("cobalt").exists());
        assert!(!adds.join("cobalt.next").exists());
        let _ignored = fs::remove_dir_all(&adds);
    }

    #[test]
    fn a_member_that_climbs_out_with_dot_dot_is_refused() {
        let adds = scratch("climb");
        let climbing = Member {
            path: format!("{PREFIX}/../escape"),
            kind: b'0',
            payload: b"tampered",
        };
        let (archive, digest) = published(&[climbing]);
        assert_eq!(
            install(&archive, &digest, &adds),
            Err(DeviceError::InvalidInput)
        );
        let _ignored = fs::remove_dir_all(&adds);
    }

    #[test]
    fn a_symbolic_link_is_refused() {
        let adds = scratch("symlink");
        let link = Member {
            path: format!("{PREFIX}/link"),
            kind: b'2',
            payload: &[],
        };
        let (archive, digest) = published(&[link]);
        assert_eq!(
            install(&archive, &digest, &adds),
            Err(DeviceError::InvalidInput)
        );
        let _ignored = fs::remove_dir_all(&adds);
    }

    #[test]
    fn a_sibling_folder_sharing_the_prefix_spelling_is_refused() {
        let adds = scratch("sibling");
        let sibling = Member {
            path: format!("{PREFIX}-else/file"),
            kind: b'0',
            payload: b"tampered",
        };
        let (archive, digest) = published(&[sibling]);
        assert_eq!(
            install(&archive, &digest, &adds),
            Err(DeviceError::InvalidInput)
        );
        let _ignored = fs::remove_dir_all(&adds);
    }

    #[test]
    fn an_empty_archive_is_refused() {
        let adds = scratch("empty");
        let archive = gzip(&[0u8; 1024]);
        let digest = kobo_net::sha256::hex_digest(&archive);
        assert_eq!(
            install(&archive, &digest, &adds),
            Err(DeviceError::InvalidInput)
        );
        let _ignored = fs::remove_dir_all(&adds);
    }
}

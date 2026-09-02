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
//! unpack is swapped in. The swap itself is two renames on one filesystem,
//! which is as small as that window gets.

use kobo_protocol::DeviceError;
use std::fs;
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
    Ok(())
}

/// Retires the current installation and moves the staged one into place.
///
/// Owner folders stay in the retired installation until the new tree is live.
/// `cobalt.next` is disposable by design, so it must never be their only home.
fn swap(adds: &Path, staging: &Path) -> Result<(), DeviceError> {
    swap_with_rename(adds, staging, |from, to| {
        fs::rename(from, to).map_err(|_| DeviceError::Backend)
    })
}

/// The same staged swap with rename injected for failure-path verification.
fn swap_with_rename(
    adds: &Path,
    staging: &Path,
    rename: impl FnMut(&Path, &Path) -> Result<(), DeviceError>,
) -> Result<(), DeviceError> {
    swap_with_operations(adds, staging, rename, |path| {
        fs::remove_dir_all(path).map_err(|_| DeviceError::Backend)
    })
}

/// The complete update transaction with filesystem fault injection for tests.
fn swap_with_operations(
    adds: &Path,
    staging: &Path,
    mut rename: impl FnMut(&Path, &Path) -> Result<(), DeviceError>,
    mut remove: impl FnMut(&Path) -> Result<(), DeviceError>,
) -> Result<(), DeviceError> {
    let current = adds.join("cobalt");
    let previous = adds.join("cobalt.prev");
    let mut retired = false;
    if current.exists() {
        if previous.exists() {
            remove(&previous)?;
        }
        rename(&current, &previous)?;
        retired = true;
    }
    if rename(staging, &current).is_err() {
        // The old installation was already stepped aside, and leaving the
        // launcher pointing at nothing is the one outcome worse than any
        // failure. Put it back; both names are on the same filesystem, so
        // this rename is as likely to work as the one that just did.
        if retired {
            rename(&previous, &current)?;
        }
        return Err(DeviceError::Backend);
    }
    if retired && carry_owner_folders(&previous, &current).is_err() {
        rename(&current, staging)?;
        rename(&previous, &current)?;
        return Err(DeviceError::Backend);
    }
    Ok(())
}

/// Recovers owner data from interrupted current/next/prev states before a
/// retry may discard staging or an old rollback tree.
fn recover_interrupted_update(adds: &Path) -> Result<(), DeviceError> {
    let current = adds.join("cobalt");
    let staging = adds.join("cobalt.next");
    let previous = adds.join("cobalt.prev");
    if !current.exists() && previous.exists() {
        fs::rename(&previous, &current).map_err(|_| DeviceError::Backend)?;
    }
    if current.exists() && previous.exists() {
        resume_owner_folders(&previous, &current)?;
    }
    if current.exists() && staging.exists() {
        resume_owner_folders(&staging, &current)?;
    } else if staging.exists() && has_owner_folders(&staging) {
        return Err(DeviceError::Backend);
    }
    Ok(())
}

fn has_owner_folders(path: &Path) -> bool {
    OWNER_FOLDERS
        .iter()
        .any(|folder| path.join(folder).exists())
}

/// Finishes an owner-folder transfer interrupted between atomic renames.
///
/// A folder already present at both locations is ambiguous and therefore
/// fails closed. The updater never deletes either copy in that state.
fn resume_owner_folders(from: &Path, to: &Path) -> Result<(), DeviceError> {
    for folder in OWNER_FOLDERS {
        if from.join(folder).exists() && to.join(folder).exists() {
            return Err(DeviceError::Backend);
        }
    }
    for folder in OWNER_FOLDERS {
        let source = from.join(folder);
        if !source.exists() {
            continue;
        }
        let destination = to.join(folder);
        fs::rename(source, destination).map_err(|_| DeviceError::Backend)?;
    }
    Ok(())
}

/// What the owner put on the reader, as opposed to what a release ships:
/// installed trust roots, secrets, application state and application data. A
/// release archive never carries these folders, so an update carries them
/// forward or the reader forgets everything it was trusted with.
const OWNER_FOLDERS: [&str; 6] = ["secrets", "trust", "state", "data", "apps", "store"];

/// Moves all owner folders as one transaction.
///
/// Destination folders are rejected rather than overwritten: a published
/// release has no authority to supply secrets or state. On failure, every
/// already-moved folder is moved back before returning an error.
fn carry_owner_folders(from: &Path, to: &Path) -> Result<(), DeviceError> {
    carry_owner_folders_with(from, to, |from, to| fs::rename(from, to).map_err(|_| ()))
        .map_err(|()| DeviceError::Backend)
}

fn carry_owner_folders_with(
    from: &Path,
    to: &Path,
    mut rename: impl FnMut(&Path, &Path) -> Result<(), ()>,
) -> Result<(), ()> {
    let mut moved = Vec::new();
    for folder in OWNER_FOLDERS {
        let source = from.join(folder);
        let destination = to.join(folder);
        if !source.exists() {
            continue;
        }
        if destination.exists() || rename(&source, &destination).is_err() {
            for moved_folder in moved.into_iter().rev() {
                let _ignored = rename(&to.join(moved_folder), &from.join(moved_folder));
            }
            return Err(());
        }
        moved.push(folder);
    }
    Ok(())
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
        carry_owner_folders_with, install, recover_interrupted_update, swap_with_operations,
        swap_with_rename, OWNER_FOLDERS, PREFIX,
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

    #[test]
    fn every_owner_folder_moves_into_the_verified_staging_installation() {
        let root = scratch("all-owner-folders");
        let current = root.join("cobalt");
        let staging = root.join("cobalt.next");
        fs::create_dir_all(&current).expect("current");
        fs::create_dir_all(&staging).expect("staging");
        for folder in OWNER_FOLDERS {
            fs::create_dir_all(current.join(folder)).expect("owner folder");
            fs::write(current.join(folder).join("kept"), folder).expect("owner data");
        }
        carry_owner_folders_with(&current, &staging, |from, to| {
            fs::rename(from, to).map_err(|_| ())
        })
        .expect("all owner data transferred");
        for folder in OWNER_FOLDERS {
            assert!(!current.join(folder).exists(), "{folder} remained behind");
            assert_eq!(
                fs::read_to_string(staging.join(folder).join("kept")).expect("staged owner data"),
                folder
            );
        }
        let _ignored = fs::remove_dir_all(&root);
    }

    #[test]
    fn an_injected_owner_move_failure_rolls_every_prior_move_back() {
        let root = scratch("owner-rollback");
        let current = root.join("cobalt");
        let staging = root.join("cobalt.next");
        fs::create_dir_all(&current).expect("current");
        fs::create_dir_all(&staging).expect("staging");
        for folder in OWNER_FOLDERS {
            fs::create_dir_all(current.join(folder)).expect("owner folder");
            fs::write(current.join(folder).join("kept"), folder).expect("owner data");
        }
        assert_eq!(
            carry_owner_folders_with(&current, &staging, |from, to| {
                if to.starts_with(&staging) && from.file_name().is_some_and(|name| name == "data") {
                    Err(())
                } else {
                    fs::rename(from, to).map_err(|_| ())
                }
            }),
            Err(())
        );
        for folder in OWNER_FOLDERS {
            assert_eq!(
                fs::read_to_string(current.join(folder).join("kept")).expect("restored owner data"),
                folder,
                "{folder} was stranded outside the current installation"
            );
            assert!(
                !staging.join(folder).exists(),
                "{folder} leaked into staging"
            );
        }
        let _ignored = fs::remove_dir_all(&root);
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
    fn a_failed_live_rename_restores_the_previous_installation_and_owner_data() {
        let adds = scratch("promotion-rollback");
        let current = adds.join("cobalt");
        let staging = adds.join("cobalt.next");
        fs::create_dir_all(current.join("state")).expect("current state");
        fs::write(current.join("state/session"), b"kept").expect("state");
        fs::write(current.join("start.sh"), b"old").expect("current release");
        fs::create_dir_all(&staging).expect("staging");
        fs::write(staging.join("start.sh"), b"new").expect("staged release");
        assert_eq!(
            swap_with_rename(&adds, &staging, |from, to| {
                if from == staging && to == current {
                    Err(DeviceError::Backend)
                } else {
                    fs::rename(from, to).map_err(|_| DeviceError::Backend)
                }
            }),
            Err(DeviceError::Backend)
        );
        assert_eq!(
            fs::read(current.join("start.sh")).expect("previous release restored"),
            b"old"
        );
        assert_eq!(
            fs::read(current.join("state/session")).expect("owner state restored"),
            b"kept"
        );
        assert!(!adds.join("cobalt.prev").exists());
        let _ignored = fs::remove_dir_all(&adds);
    }

    #[test]
    fn an_injected_previous_delete_failure_restores_owner_data_before_returning() {
        let adds = scratch("previous-delete-rollback");
        let current = adds.join("cobalt");
        let staging = adds.join("cobalt.next");
        let previous = adds.join("cobalt.prev");
        fs::create_dir_all(current.join("store")).expect("current store");
        fs::write(current.join("store/catalog"), b"kept").expect("store");
        fs::write(current.join("start.sh"), b"old").expect("current release");
        fs::create_dir_all(&staging).expect("staging");
        fs::create_dir_all(&previous).expect("previous");
        fs::write(previous.join("start.sh"), b"older").expect("previous release");
        assert_eq!(
            swap_with_operations(
                &adds,
                &staging,
                |from, to| fs::rename(from, to).map_err(|_| DeviceError::Backend),
                |_| Err(DeviceError::Backend),
            ),
            Err(DeviceError::Backend)
        );
        assert_eq!(
            fs::read(current.join("store/catalog")).expect("restored owner store"),
            b"kept"
        );
        assert!(previous.exists(), "the old backup was deleted on failure");
        let _ignored = fs::remove_dir_all(&adds);
    }

    #[test]
    fn an_injected_retirement_failure_restores_staged_owner_data_to_current() {
        let adds = scratch("retirement-rollback");
        let current = adds.join("cobalt");
        let staging = adds.join("cobalt.next");
        fs::create_dir_all(current.join("data")).expect("current data");
        fs::write(current.join("data/cache"), b"kept").expect("data");
        fs::write(current.join("start.sh"), b"old").expect("current release");
        fs::create_dir_all(&staging).expect("staging");
        assert_eq!(
            swap_with_rename(&adds, &staging, |from, to| {
                if from == current && to.ends_with("cobalt.prev") {
                    Err(DeviceError::Backend)
                } else {
                    fs::rename(from, to).map_err(|_| DeviceError::Backend)
                }
            }),
            Err(DeviceError::Backend)
        );
        assert_eq!(
            fs::read(current.join("data/cache")).expect("restored owner data"),
            b"kept"
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

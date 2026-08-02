//! Building the one file a Kobo owner has to copy.
//!
//! # Why this exists at all
//!
//! Everything else in this project is reached over SSH, which is fine for the
//! person writing it and useless for anyone else. A Kobo owner should not have
//! to enable a hidden developer setting, find an IP address and learn that this
//! device's SSH server ignores remote arguments. They should copy one file and
//! reboot.
//!
//! # Why it writes nothing to the root filesystem
//!
//! `/etc/init.d/rcS` extracts `/mnt/onboard/.kobo/KoboRoot.tgz` as root at
//! boot, so a tarball could put a file anywhere. This one deliberately cannot:
//! every path is checked to sit under `mnt/onboard/.adds/cobalt/`, which is the
//! book partition the owner already sees over USB.
//!
//! That is possible because of something measured on the device rather than
//! assumed: `/mnt/onboard` is mounted `rw,noatime,nodiratime,fmask=0022` with
//! **no `noexec`**, so a binary copied there is mode 0755 and runs. A script
//! written and executed on the device confirmed it. So the platform needs no
//! root, owns no boot script, and uninstalling is deleting a folder from a file
//! manager on any computer.
//!
//! The vendor's installer is used anyway, for two reasons. It extracts the
//! exact layout, where a human copying a folder tree onto a case-insensitive
//! filesystem may not. And it is bracketed by u-boot environment writes that
//! point at recovery before extraction and back at normal boot afterwards, so
//! even losing power halfway through lands somewhere recoverable.
//!
//! # The gate that makes installs "silently do nothing"
//!
//! `rcS` only looks at the tarball if `pickel can-upgrade` agrees, and
//! `pickel-mtk` contains exactly one interesting string:
//! `/sys/class/power_supply/bd71827_bat/capacity`. It is a battery check. An
//! owner installing on a flat device sees nothing happen and no explanation,
//! so the instructions say to charge first.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Where everything this platform installs lives.
///
/// Relative, because a tar member path must be, and because a leading slash is
/// exactly the thing that would let a tarball escape.
pub const INSTALL_ROOT: &str = "mnt/onboard/.adds/cobalt";

/// The largest member this builder will write.
///
/// A guard against packaging something that is not a device binary at all. The
/// binaries are around a megabyte each.
const MAX_MEMBER: u64 = 32 * 1024 * 1024;

const BLOCK: usize = 512;

/// One file in the package.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Member {
    pub path: String,
    pub bytes: Vec<u8>,
    /// Whether the file is a program. On the book partition every file is 0755
    /// whatever this says, but the archive should still be honest, and the
    /// same list is used to write a plain folder.
    pub program: bool,
}

/// Checks a member list before anything is written.
///
/// Refusals rather than fixes: a path that had to be corrected is a path
/// somebody did not mean to write, and this archive is extracted as root.
///
/// # Errors
///
/// Returns the first path that is absolute, escapes the install root, is
/// empty, is duplicated, or is larger than [`MAX_MEMBER`].
pub fn check(members: &[Member]) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for member in members {
        let path = member.path.as_str();
        if path.is_empty() || path.starts_with('/') || path.starts_with("./") {
            return Err(format!("{path:?} is not a relative path"));
        }
        if path.split('/').any(|part| part == ".." || part.is_empty()) {
            return Err(format!("{path:?} escapes the archive"));
        }
        if !path.starts_with(&format!("{INSTALL_ROOT}/")) {
            return Err(format!(
                "{path:?} is outside {INSTALL_ROOT}; this package never writes to the root filesystem"
            ));
        }
        if member.bytes.len() as u64 > MAX_MEMBER {
            return Err(format!("{path:?} is larger than this builder will package"));
        }
        if !seen.insert(path.to_owned()) {
            return Err(format!("{path:?} appears twice"));
        }
    }
    Ok(())
}

/// Writes a deterministic ustar archive.
///
/// Deterministic on purpose: the same inputs produce the same bytes, so the
/// checksum an owner is told to compare means something. Timestamps, uid, gid
/// and ordering are all fixed rather than taken from the machine that happened
/// to build it.
///
/// # Errors
///
/// Returns an error if [`check`] refuses the member list.
pub fn tar(members: &[Member]) -> Result<Vec<u8>, String> {
    check(members)?;
    let mut archive = Vec::new();
    for directory in directories(members) {
        archive.extend_from_slice(&header(&format!("{directory}/"), 0, b'5', 0o755));
    }
    for member in members {
        let mode = if member.program { 0o755 } else { 0o644 };
        archive.extend_from_slice(&header(&member.path, member.bytes.len(), b'0', mode));
        archive.extend_from_slice(&member.bytes);
        let padding = (BLOCK - member.bytes.len() % BLOCK) % BLOCK;
        archive.extend(std::iter::repeat_n(0u8, padding));
    }
    // Two empty blocks end an archive, and BusyBox tar wants them.
    archive.extend(std::iter::repeat_n(0u8, BLOCK * 2));
    Ok(archive)
}

/// Writes a ustar archive from entries given exactly, without [`check`].
///
/// [`tar`] is the only way to build the Cobalt payload, and it refuses any
/// path outside the install root because that payload must never touch the
/// root filesystem. This is the primitive underneath it, and it is
/// `pub(crate)` rather than `pub` so that the one caller which does need to
/// write elsewhere, [`crate::authorize`], has to be a named module in this
/// crate rather than anything that happens to link against it.
///
/// Directories are listed explicitly rather than inferred from the file paths.
/// Inferring them meant an archive holding `root/.ssh/authorized_keys` also
/// carried an entry for `root/`, and an archive extracted as root should
/// create the one directory it needs rather than reach for the mode of a
/// directory that was already there.
pub(crate) fn archive(folders: &[(&str, u32)], files: &[(String, Vec<u8>, u32)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (folder, mode) in folders {
        out.extend_from_slice(&header(&format!("{folder}/"), 0, b'5', *mode));
    }
    for (path, bytes, mode) in files {
        out.extend_from_slice(&header(path, bytes.len(), b'0', *mode));
        out.extend_from_slice(bytes);
        let padding = (BLOCK - bytes.len() % BLOCK) % BLOCK;
        out.extend(std::iter::repeat_n(0u8, padding));
    }
    out.extend(std::iter::repeat_n(0u8, BLOCK * 2));
    out
}

/// Appends entries to an archive that has already been terminated.
///
/// The single `KoboRoot.tgz` slot is the reason this exists. The firmware
/// extracts exactly one archive, so a run that wants to install both
/// NickelMenu and this machine's key cannot stage two: it has to hand over one
/// archive carrying both. Concatenating the two would not do, because a reader
/// stops at the end-of-archive marker and would never see the second.
///
/// `pub(crate)` for the same reason [`archive`] is: only [`crate::authorize`]
/// needs it.
///
/// # Errors
///
/// When `base` is not a whole number of blocks, or does not end with the two
/// zero blocks that terminate a tar. Both mean the input is not an archive
/// this can safely reopen.
pub(crate) fn extend(
    base: &[u8],
    folders: &[(&str, u32)],
    files: &[(String, Vec<u8>, u32)],
) -> Result<Vec<u8>, String> {
    if base.is_empty() || base.len() % BLOCK != 0 {
        return Err(format!(
            "this is not a tar archive: {} bytes is not a whole number of {BLOCK}-byte blocks",
            base.len()
        ));
    }
    let mut end = base.len();
    while end >= BLOCK && base[end - BLOCK..end].iter().all(|&byte| byte == 0) {
        end -= BLOCK;
    }
    // Padding to the blocking factor means there may be many trailing zero
    // blocks, but there must be at least the two that end an archive.
    if base.len() - end < BLOCK * 2 {
        return Err("this tar archive does not end, so nothing can be added to it".to_owned());
    }
    let mut out = base[..end].to_vec();
    out.extend_from_slice(&archive(folders, files));
    Ok(out)
}

/// Every directory the members live in, parents first.
fn directories(members: &[Member]) -> Vec<String> {
    let mut all = BTreeSet::new();
    for member in members {
        let mut parts: Vec<&str> = member.path.split('/').collect();
        parts.pop();
        for end in 1..=parts.len() {
            all.insert(parts[..end].join("/"));
        }
    }
    // Folders above the install root are not this package's to describe:
    // they already exist on any reader, and an updater that rightly refuses
    // members outside the install root would refuse the whole archive over
    // them. tar creates missing parents on its own.
    all.retain(|path| path == INSTALL_ROOT || path.starts_with(&format!("{INSTALL_ROOT}/")));
    let mut ordered: Vec<String> = all.into_iter().collect();
    ordered.sort_by_key(|path| (path.matches('/').count(), path.clone()));
    ordered
}

fn header(path: &str, size: usize, kind: u8, mode: u32) -> [u8; BLOCK] {
    let mut block = [0u8; BLOCK];
    write_field(&mut block[0..100], path.as_bytes());
    write_octal(&mut block[100..108], u64::from(mode), 7);
    write_octal(&mut block[108..116], 0, 7);
    write_octal(&mut block[116..124], 0, 7);
    write_octal(&mut block[124..136], size as u64, 11);
    // A fixed modification time, because the point is reproducibility and a
    // build clock is the one input nobody can reproduce.
    write_octal(&mut block[136..148], 0, 11);
    block[148..156].fill(b' ');
    block[156] = kind;
    write_field(&mut block[257..263], b"ustar");
    block[263..265].copy_from_slice(b"00");
    write_field(&mut block[265..297], b"root");
    write_field(&mut block[297..329], b"root");
    let checksum: u32 = block.iter().map(|&byte| u32::from(byte)).sum();
    write_octal(&mut block[148..156], u64::from(checksum), 6);
    block[154] = 0;
    block[155] = b' ';
    block
}

fn write_field(field: &mut [u8], value: &[u8]) {
    let length = value.len().min(field.len() - 1);
    field[..length].copy_from_slice(&value[..length]);
}

fn write_octal(field: &mut [u8], value: u64, digits: usize) {
    let text = format!("{value:0digits$o}");
    field[..digits].copy_from_slice(&text.as_bytes()[..digits]);
    field[digits] = 0;
}

/// One entry read back out of an archive.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Listed {
    pub path: String,
    pub size: usize,
    pub mode: u32,
    pub kind: u8,
}

fn safe_archive_path(path: &str) -> bool {
    let path = path.strip_suffix('/').unwrap_or(path);
    !path.is_empty()
        && path
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-'))
        && !path
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
}

/// Reads an archive back, so a package can be inspected rather than trusted.
///
/// # Errors
///
/// Returns an error for a truncated archive, a bad header checksum, or a
/// member type this builder never writes. A symbolic link, a hard link and a
/// device node are all refused here, because on extraction as root they are
/// the interesting ones.
pub fn list(archive: &[u8]) -> Result<Vec<Listed>, String> {
    let mut entries = Vec::new();
    let mut offset = 0usize;
    while offset + BLOCK <= archive.len() {
        let block = &archive[offset..offset + BLOCK];
        if block.iter().all(|&byte| byte == 0) {
            break;
        }
        verify_checksum(block)?;
        let path = read_string(&block[0..100]);
        if !safe_archive_path(&path) {
            return Err(format!("{path:?} is not a safe relative archive path"));
        }
        let mode = u32::try_from(read_octal(&block[100..108])?)
            .map_err(|_| format!("{path:?} has an implausible mode"))?;
        let size = usize::try_from(read_octal(&block[124..136])?)
            .map_err(|_| format!("{path:?} is larger than this machine can address"))?;
        let kind = block[156];
        if kind != b'0' && kind != b'5' {
            return Err(format!("{path:?} is not a plain file or directory"));
        }
        entries.push(Listed {
            path: path.clone(),
            size,
            mode,
            kind,
        });
        let payload = if kind == b'5' { 0 } else { size };
        let occupied = payload
            .div_ceil(BLOCK)
            .checked_mul(BLOCK)
            .and_then(|payload| BLOCK.checked_add(payload))
            .ok_or_else(|| format!("{path:?} has an overflowing size"))?;
        offset = offset
            .checked_add(occupied)
            .ok_or_else(|| format!("{path:?} has an overflowing offset"))?;
        if offset > archive.len() {
            return Err(format!("{path:?} is truncated"));
        }
    }
    if entries.is_empty() {
        return Err("the archive contains nothing".to_owned());
    }
    Ok(entries)
}

fn verify_checksum(block: &[u8]) -> Result<(), String> {
    let stated = read_octal(&block[148..156])?;
    let computed: u32 = block
        .iter()
        .enumerate()
        .map(|(index, &byte)| {
            if (148..156).contains(&index) {
                u32::from(b' ')
            } else {
                u32::from(byte)
            }
        })
        .sum();
    if u64::from(computed) == stated {
        Ok(())
    } else {
        Err("a header checksum does not match; the archive is damaged".to_owned())
    }
}

fn read_string(field: &[u8]) -> String {
    let end = field
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).into_owned()
}

fn read_octal(field: &[u8]) -> Result<u64, String> {
    let text = read_string(field);
    let digits = text.trim_matches(|character: char| character == ' ' || character == '\0');
    if digits.is_empty() {
        return Ok(0);
    }
    u64::from_str_radix(digits, 8).map_err(|_| format!("{digits:?} is not an octal field"))
}

/// Writes the same payload as a plain folder, for an owner who would rather
/// copy files than trust a tarball.
///
/// # Errors
///
/// Returns any filesystem error, and refuses a member list [`check`] rejects.
pub fn write_folder(members: &[Member], root: &Path) -> Result<(), String> {
    check(members)?;
    let prefix = format!("{INSTALL_ROOT}/");
    for member in members {
        let relative = member
            .path
            .strip_prefix(&prefix)
            .ok_or_else(|| format!("{:?} is outside the install root", member.path))?;
        let destination = root.join(relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
        }
        fs::write(&destination, &member.bytes)
            .map_err(|error| format!("{}: {error}", destination.display()))?;
        if member.program {
            set_executable(&destination)?;
        }
    }
    Ok(())
}

fn set_executable(path: &PathBuf) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path)
        .map_err(|error| format!("{}: {error}", path.display()))?
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).map_err(|error| format!("{}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::{check, header, list, tar, write_folder, Member, BLOCK, INSTALL_ROOT};

    fn member(name: &str, bytes: &[u8]) -> Member {
        Member {
            path: format!("{INSTALL_ROOT}/{name}"),
            bytes: bytes.to_vec(),
            program: name.starts_with("bin/"),
        }
    }

    #[test]
    fn an_archive_reads_back_as_what_went_in() {
        let members = vec![
            member("bin/kobod", b"ELF..."),
            member("README.txt", b"how to remove this"),
        ];
        let archive = tar(&members).expect("a valid package");
        let listed = list(&archive).expect("it reads back");
        let files: Vec<&super::Listed> = listed.iter().filter(|entry| entry.kind == b'0').collect();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, format!("{INSTALL_ROOT}/bin/kobod"));
        assert_eq!(files[0].size, 6);
        assert_eq!(files[0].mode, 0o755);
        assert_eq!(files[1].mode, 0o644);
    }

    /// The property the whole design rests on.
    ///
    /// This archive is extracted as root by the device's own boot script. If a
    /// path outside the install root could ever get in, everything else in this
    /// project's safety argument is void.
    #[test]
    fn nothing_outside_the_install_root_can_be_packaged() {
        for path in [
            "etc/init.d/rcS",
            "usr/local/Kobo/nickel",
            "/mnt/onboard/.adds/cobalt/bin/x",
            "mnt/onboard/.adds/cobalt/../../../etc/passwd",
            "mnt/onboard/.adds/nm/cobalt",
        ] {
            let members = vec![Member {
                path: path.to_owned(),
                bytes: b"x".to_vec(),
                program: false,
            }];
            assert!(check(&members).is_err(), "{path} was accepted");
            assert!(tar(&members).is_err(), "{path} was packaged");
        }
    }

    #[test]
    fn an_archive_read_from_disk_cannot_hide_traversal_beneath_the_install_prefix() {
        let path = format!("{INSTALL_ROOT}/../../../../etc/passwd");
        let mut archive = header(&path, 1, b'0', 0o644).to_vec();
        archive.extend_from_slice(b"x");
        archive.resize(BLOCK * 2, 0);
        assert!(list(&archive).is_err());
    }

    #[test]
    fn an_archive_member_must_be_complete() {
        let path = format!("{INSTALL_ROOT}/large");
        let archive = header(&path, BLOCK * 4, b'0', 0o644);
        assert!(list(&archive).is_err());
    }

    #[test]
    fn the_same_input_always_produces_the_same_bytes() {
        // A checksum an owner is asked to compare is worthless if the build
        // machine's clock is one of the inputs.
        let members = vec![member("bin/kobod", b"ELF...")];
        assert_eq!(tar(&members), tar(&members));
    }

    #[test]
    fn a_duplicate_path_is_refused_rather_than_overwritten() {
        let members = vec![member("bin/kobod", b"one"), member("bin/kobod", b"two")];
        assert!(check(&members).is_err());
    }

    #[test]
    fn parent_directories_are_created_before_what_is_in_them() {
        let members = vec![member("bin/kobod", b"ELF...")];
        let archive = tar(&members).expect("a valid package");
        let listed = list(&archive).expect("it reads back");
        let directories: Vec<&str> = listed
            .iter()
            .filter(|entry| entry.kind == b'5')
            .map(|entry| entry.path.as_str())
            .collect();
        // Nothing above the install root: those folders exist on every
        // reader, and the on-device updater refuses members outside it.
        assert_eq!(
            directories,
            vec!["mnt/onboard/.adds/cobalt/", "mnt/onboard/.adds/cobalt/bin/",]
        );
    }

    #[test]
    fn a_damaged_header_is_reported_rather_than_read() {
        let members = vec![member("bin/kobod", b"ELF...")];
        let mut archive = tar(&members).expect("a valid package");
        archive[600] ^= 0xff;
        assert!(list(&archive).is_err());
    }

    #[test]
    fn the_folder_form_holds_the_same_files() {
        let root = std::env::temp_dir().join(format!("cobalt-package-{}", std::process::id()));
        let _ignored = std::fs::remove_dir_all(&root);
        let members = vec![
            member("bin/kobod", b"ELF..."),
            member("README.txt", b"how to remove this"),
        ];
        write_folder(&members, &root).expect("writing a folder");
        assert_eq!(
            std::fs::read(root.join("bin/kobod")).expect("the binary"),
            b"ELF..."
        );
        assert!(root.join("README.txt").is_file());
        let _ignored = std::fs::remove_dir_all(&root);
    }
}

//! The tailnet names this runtime keeps in the reader's hosts file.
//!
//! The reader declines `MagicDNS`, because it has no DNS manager and its
//! resolver file belongs to the stock firmware. Applications still need to
//! reach a tailnet computer by name, so the names live in a marked block in
//! the hosts file instead, and only while the daemon is running.
//!
//! That file is the stock reader's, on the root filesystem, and a mistake in
//! it outlives Cobalt. So the edit is made on bytes rather than on lines of
//! text: everything outside the two markers is carried through exactly as it
//! was found, and the file is replaced by a rename or not at all.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::net::IpAddr;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

const BEGIN: &[u8] = b"# >>> cobalt-tailscale >>>";
const END: &[u8] = b"# <<< cobalt-tailscale <<<";

/// The largest hosts file this will edit.
///
/// A stock reader's is under a kilobyte. One past this size was not written
/// by anything this runtime understands, and reading an unbounded file into
/// memory to append a few lines to it is how a small feature takes a device
/// with half a gigabyte of memory down.
const LARGEST_FILE: u64 = 256 * 1024;

/// The most tailnet names written. A tailnet with more computers than this
/// is an organisation's, and the reader needs the handful its owner dials.
const MOST_NAMES: usize = 64;

/// One computer on the tailnet, as the daemon's status describes it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Node {
    pub address: String,
    pub name: String,
}

/// The lines to write for `nodes`, sorted so the same tailnet always produces
/// the same block and an unchanged tailnet never rewrites the file.
///
/// A name or address that is not exactly what it claims to be is left out.
/// Both arrive from the network inside a JSON string, which can hold a line
/// break as easily as a letter, and a line break written here would be a line
/// of the attacker's choosing in the file every program on the reader
/// resolves names through.
pub fn block(nodes: &[Node]) -> Vec<String> {
    let mut lines = nodes
        .iter()
        .filter_map(|node| {
            let address = node.address.parse::<IpAddr>().ok()?;
            let name = node.name.trim_end_matches('.');
            if !is_host_name(name) {
                return None;
            }
            let short = name.split('.').next().unwrap_or_default();
            Some(if short == name {
                format!("{address}\t{name}")
            } else {
                format!("{address}\t{name} {short}")
            })
        })
        .collect::<Vec<_>>();
    lines.sort();
    lines.dedup();
    lines.truncate(MOST_NAMES);
    lines
}

fn is_host_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 253
        && name.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
}

/// `existing` with every earlier block removed and `lines` appended as the
/// new one, or with no block at all when `lines` is empty.
///
/// A marker with no partner is removed on its own and nothing beside it is
/// touched. Nothing this module writes can leave one, because its writes are
/// whole files, so a lone marker is somebody's hand edit, and guessing where
/// they meant the block to end could delete lines that were never ours.
pub fn splice(existing: &[u8], lines: &[String]) -> Vec<u8> {
    let mut kept = Vec::with_capacity(existing.len() + 64 * lines.len());
    let mut rest = existing;
    while !rest.is_empty() {
        let (line, after) = split_line(rest);
        if content(line) == BEGIN {
            if let Some(resume) = after_end_marker(after) {
                rest = resume;
                continue;
            }
        } else if content(line) != END {
            kept.extend_from_slice(line);
        }
        rest = after;
    }
    if lines.is_empty() {
        return kept;
    }
    // The one change outside the markers: a file whose last line has no line
    // break gains one, or the marker would be glued to the owner's last entry.
    if kept.last().is_some_and(|byte| *byte != b'\n') {
        kept.push(b'\n');
    }
    kept.extend_from_slice(BEGIN);
    kept.push(b'\n');
    for line in lines {
        kept.extend_from_slice(line.as_bytes());
        kept.push(b'\n');
    }
    kept.extend_from_slice(END);
    kept.push(b'\n');
    kept
}

/// The first line of `bytes` with its line break, and what follows it.
fn split_line(bytes: &[u8]) -> (&[u8], &[u8]) {
    let end = bytes
        .iter()
        .position(|byte| *byte == b'\n')
        .map_or(bytes.len(), |index| index + 1);
    bytes.split_at(end)
}

/// A line without its line break, in either convention.
fn content(line: &[u8]) -> &[u8] {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    line.strip_suffix(b"\r").unwrap_or(line)
}

/// What follows the end marker, when one closes the block that `bytes` is
/// the inside of before another block begins.
fn after_end_marker(bytes: &[u8]) -> Option<&[u8]> {
    let mut rest = bytes;
    while !rest.is_empty() {
        let (line, after) = split_line(rest);
        if content(line) == END {
            return Some(after);
        }
        if content(line) == BEGIN {
            return None;
        }
        rest = after;
    }
    None
}

/// Makes the block in the hosts file at `path` hold exactly `lines`.
///
/// Returns without writing when the file already says so, which is every
/// minute of an ordinary session: the root filesystem is flash, and a rewrite
/// per status check would wear it for nothing.
///
/// # Errors
///
/// When the file cannot be read, is too large to be a hosts file, or cannot
/// be replaced. In each case it is left exactly as it was found.
pub fn write(path: &Path, lines: &[String]) -> Result<(), String> {
    // A hosts file that is a link is replaced where it points, so the link
    // the firmware set up is still a link afterwards.
    let path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let (existing, mode) = match read(&path) {
        Ok(found) => found,
        // No hosts file and nothing to add is already the wanted state.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && lines.is_empty() => {
            return Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (Vec::new(), 0o644),
        // Anything else is a file that exists and could not be read. Writing
        // the block alone over it would delete every entry it held.
        Err(error) => return Err(format!("read {}: {error}", path.display())),
    };
    let next = splice(&existing, lines);
    if next == existing {
        return Ok(());
    }
    replace(&path, &next, mode)
}

fn read(path: &Path) -> std::io::Result<(Vec<u8>, u32)> {
    let file = fs::File::open(path)?;
    let metadata = file.metadata()?;
    if metadata.len() > LARGEST_FILE {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "larger than any hosts file this edits",
        ));
    }
    let mut bytes = Vec::new();
    // Bounded again while reading, because the size was asked a moment ago
    // and the answer can be out of date.
    file.take(LARGEST_FILE + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > LARGEST_FILE {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "larger than any hosts file this edits",
        ));
    }
    Ok((bytes, metadata.permissions().mode() & 0o777))
}

/// Writes `bytes` beside `path` and renames the result over it.
///
/// A reader that loses power halfway is left with the old file and a
/// leftover beside it, never with half a hosts file: the rename is the only
/// moment the name changes hands, and it happens after the bytes are on the
/// flash.
fn replace(path: &Path, bytes: &[u8], mode: u32) -> Result<(), String> {
    let temporary = beside(path);
    match fs::remove_file(&temporary) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("remove stale {}: {error}", temporary.display())),
    }
    let written = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&temporary)
        .and_then(|mut file| {
            file.write_all(bytes)?;
            file.sync_all()
        })
        .and_then(|()| fs::rename(&temporary, path));
    if let Err(error) = written {
        let _ignored = fs::remove_file(&temporary);
        return Err(format!("replace {}: {error}", path.display()));
    }
    if let Some(folder) = path.parent() {
        let _ignored = kobo_protocol::durability::sync_directory(folder);
    }
    Ok(())
}

fn beside(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".cobalt-new");
    path.with_file_name(name)
}

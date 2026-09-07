//! Shared, bounded CBZ inspection and lazy page decoding. Nothing is extracted.

use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::io::{Cursor, Read};
use zip::{CompressionMethod, ZipArchive};

pub const MAX_ARCHIVE_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_ENTRIES: usize = 2048;
pub const MAX_DIRECTORY_BYTES: usize = 1024 * 1024;
pub const MAX_EXPANDED_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Comic {
    pub pages: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ComicError {
    CbrUnsupported,
    UnknownFormat,
    Archive(String),
    Unsupported(String),
    UnsafeEntry,
    Limit(&'static str),
    Empty,
    PageUnavailable,
    Image(String),
}

impl std::fmt::Display for ComicError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CbrUnsupported => {
                f.write_str("CBR is not supported yet. Add a CBZ copy of this comic.")
            }
            Self::UnknownFormat => f.write_str(
                "This file is not a CBZ archive. Export the comic as CBZ and add it again.",
            ),
            Self::Archive(reason) => write!(
                f,
                "The CBZ is damaged: {reason}. Download or export a new copy."
            ),
            Self::Unsupported(reason) => write!(
                f,
                "This CBZ uses {reason}. Export an unencrypted CBZ with standard ZIP compression."
            ),
            Self::UnsafeEntry => f.write_str(
                "The CBZ contains unsafe or duplicate file names. Export a new CBZ copy.",
            ),
            Self::Limit(limit) => write!(
                f,
                "This comic exceeds the {limit} limit. Split the volume or export smaller pages."
            ),
            Self::Empty => {
                f.write_str("The CBZ contains no PNG or JPEG pages. Export pages as PNG or JPEG.")
            }
            Self::PageUnavailable => {
                f.write_str("This page is no longer in the comic. Reopen the volume.")
            }
            Self::Image(reason) => write!(
                f,
                "This page could not be opened: {reason}. Try the next page or add a new copy."
            ),
        }
    }
}
impl std::error::Error for ComicError {}

fn damaged(error: impl std::fmt::Display) -> ComicError {
    ComicError::Archive(error.to_string())
}

fn u16_at(bytes: &[u8], offset: usize) -> Result<u16, ComicError> {
    let value = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| damaged("truncated directory"))?;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}
fn u32_at(bytes: &[u8], offset: usize) -> Result<u32, ComicError> {
    let value = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| damaged("truncated directory"))?;
    Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

// Admission checks only: the ZIP dependency parses and decompresses accepted files.
// Check before constructing it, so a hostile directory cannot request unbounded allocation.
fn preflight(bytes: &[u8]) -> Result<(), ComicError> {
    if bytes.starts_with(b"Rar!\x1a\x07") {
        return Err(ComicError::CbrUnsupported);
    }
    if bytes.len() > MAX_ARCHIVE_BYTES {
        return Err(ComicError::Limit("64 MiB archive"));
    }
    if !bytes.starts_with(b"PK\x03\x04") && !bytes.starts_with(b"PK\x05\x06") {
        return Err(ComicError::UnknownFormat);
    }
    let start = bytes.len().saturating_sub(22 + usize::from(u16::MAX));
    let end = bytes
        .len()
        .checked_sub(22)
        .ok_or_else(|| damaged("missing directory"))?;
    let footer = (start..=end)
        .rev()
        .find(|&offset| {
            bytes.get(offset..offset + 4) == Some(b"PK\x05\x06")
                && u16_at(bytes, offset + 20)
                    .is_ok_and(|length| offset + 22 + usize::from(length) == bytes.len())
        })
        .ok_or_else(|| damaged("missing directory"))?;
    let entries = usize::from(u16_at(bytes, footer + 10)?);
    let directory_size = u32_at(bytes, footer + 12)? as usize;
    let directory_start = u32_at(bytes, footer + 16)? as usize;
    if entries == usize::from(u16::MAX)
        || directory_size == u32::MAX as usize
        || directory_start == u32::MAX as usize
    {
        return Err(ComicError::Unsupported("ZIP64".into()));
    }
    if u16_at(bytes, footer + 4)? != 0
        || u16_at(bytes, footer + 6)? != 0
        || usize::from(u16_at(bytes, footer + 8)?) != entries
    {
        return Err(ComicError::Unsupported("multiple ZIP volumes".into()));
    }
    if entries > MAX_ENTRIES || directory_size > MAX_DIRECTORY_BYTES {
        return Err(ComicError::Limit("2,048 files / 1 MiB directory"));
    }
    if directory_start.checked_add(directory_size) != Some(footer) {
        return Err(damaged("inconsistent directory bounds"));
    }
    let mut cursor = directory_start;
    let mut names = BTreeSet::new();
    for _ in 0..entries {
        if bytes.get(cursor..cursor + 4) != Some(b"PK\x01\x02") {
            return Err(damaged("invalid directory entry"));
        }
        let name_length = usize::from(u16_at(bytes, cursor + 28)?);
        let extra_length = usize::from(u16_at(bytes, cursor + 30)?);
        let comment_length = usize::from(u16_at(bytes, cursor + 32)?);
        let name_start = cursor + 46;
        let next = name_start + name_length + extra_length + comment_length;
        if next > footer {
            return Err(damaged("truncated directory entry"));
        }
        let name = bytes
            .get(name_start..name_start + name_length)
            .ok_or_else(|| damaged("truncated name"))?;
        if !names.insert(name) {
            return Err(ComicError::UnsafeEntry);
        }
        cursor = next;
    }
    if cursor != footer {
        return Err(damaged("inconsistent file count"));
    }
    Ok(())
}

fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('/')
        && !name.contains(['\\', ':', '\0'])
        && !name.chars().any(char::is_control)
        && name
            .trim_end_matches('/')
            .split('/')
            .all(|part| !matches!(part, "" | "." | ".."))
}

fn page_name(name: &str) -> bool {
    !name
        .split('/')
        .any(|part| part.starts_with('.') || part == "__MACOSX")
        && name.rsplit_once('.').is_some_and(|(_, extension)| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg"
            )
        })
}

fn open(bytes: &[u8]) -> Result<ZipArchive<Cursor<&[u8]>>, ComicError> {
    preflight(bytes)?;
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(damaged)?;
    let mut total = 0_u64;
    let mut names = BTreeSet::new();
    for index in 0..archive.len() {
        let file = archive.by_index_raw(index).map_err(damaged)?;
        if !safe_name(file.name())
            || !names.insert(file.name().to_owned())
            || file.is_symlink()
            || file
                .unix_mode()
                .is_some_and(|mode| !matches!(mode & 0o170_000, 0 | 0o100_000 | 0o040_000))
        {
            return Err(ComicError::UnsafeEntry);
        }
        if file.encrypted() {
            return Err(ComicError::Unsupported("encryption".into()));
        }
        if !matches!(
            file.compression(),
            CompressionMethod::Stored | CompressionMethod::Deflated
        ) {
            return Err(ComicError::Unsupported(
                "a different ZIP compression method".into(),
            ));
        }
        total = total
            .checked_add(file.size())
            .ok_or(ComicError::Limit("expanded size"))?;
        if total > MAX_EXPANDED_BYTES {
            return Err(ComicError::Limit("512 MiB expanded size"));
        }
        if page_name(file.name()) && file.size() > kobo_image::MAX_SOURCE_BYTES as u64 {
            return Err(ComicError::Limit("4 MiB per page"));
        }
    }
    Ok(archive)
}

/// Compare numbered paths without converting arbitrarily long digit runs to integers.
fn natural_order(left: &str, right: &str) -> Ordering {
    let (mut a, mut b) = (left.as_bytes(), right.as_bytes());
    while let (Some(&x), Some(&y)) = (a.first(), b.first()) {
        if x.is_ascii_digit() && y.is_ascii_digit() {
            let an = a
                .iter()
                .position(|c| !c.is_ascii_digit())
                .unwrap_or(a.len());
            let bn = b
                .iter()
                .position(|c| !c.is_ascii_digit())
                .unwrap_or(b.len());
            let aa = &a[..an];
            let bb = &b[..bn];
            let aa = &aa[aa.iter().position(|&c| c != b'0').unwrap_or(aa.len())..];
            let bb = &bb[bb.iter().position(|&c| c != b'0').unwrap_or(bb.len())..];
            let ordering = aa.len().cmp(&bb.len()).then_with(|| aa.cmp(bb));
            if ordering != Ordering::Equal {
                return ordering;
            }
            a = &a[an..];
            b = &b[bn..];
        } else {
            let ordering = x.to_ascii_lowercase().cmp(&y.to_ascii_lowercase());
            if ordering != Ordering::Equal {
                return ordering;
            }
            a = &a[1..];
            b = &b[1..];
        }
    }
    a.len().cmp(&b.len()).then_with(|| left.cmp(right))
}

/// Inspect structure and page limits without decompressing images.
///
/// # Errors
/// Returns a specific archive, format, safety or resource-limit error.
pub fn inspect(bytes: &[u8]) -> Result<Comic, ComicError> {
    let archive = open(bytes)?;
    let mut pages = archive
        .file_names()
        .filter(|name| page_name(name))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    pages.sort_by(|a, b| natural_order(a, b));
    if pages.is_empty() {
        Err(ComicError::Empty)
    } else {
        Ok(Comic { pages })
    }
}

/// Decode one page, checking the ZIP CRC and image limits. No page cache is retained here.
///
/// # Errors
/// Returns an archive, missing-page or image decoding error.
pub fn page(bytes: &[u8], comic: &Comic, index: usize) -> Result<kobo_image::Picture, ComicError> {
    let name = comic.pages.get(index).ok_or(ComicError::PageUnavailable)?;
    let mut archive = open(bytes)?;
    let mut file = archive.by_name(name).map_err(damaged)?;
    if !page_name(name) || file.size() > kobo_image::MAX_SOURCE_BYTES as u64 {
        return Err(ComicError::PageUnavailable);
    }
    let mut encoded = Vec::new();
    (&mut file)
        .take(kobo_image::MAX_SOURCE_BYTES as u64 + 1)
        .read_to_end(&mut encoded)
        .map_err(damaged)?;
    if encoded.len() > kobo_image::MAX_SOURCE_BYTES {
        return Err(ComicError::Limit("4 MiB per page"));
    }
    kobo_image::decode(&encoded).map_err(|error| ComicError::Image(error.to_string()))
}

#[cfg(test)]
mod tests;

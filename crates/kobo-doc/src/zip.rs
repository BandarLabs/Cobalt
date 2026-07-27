//! Reading a zip archive that arrived as bytes.
//!
//! # Why this is here rather than a crate
//!
//! Every zip crate on the registry is built around `Read + Seek`, because
//! every zip crate expects a file. An application on this platform cannot open
//! a file — an EPUB arrives as a `Vec<u8>` from a download or from the store —
//! and a whole archive already in memory does not need seeking. What is left
//! once the file handling is gone is the central directory format, which is a
//! table with fixed offsets.
//!
//! # What it does not do
//!
//! No Zip64, no encryption, no multi-disk archives, no data descriptors, no
//! compression method but *stored* and *deflate*. An EPUB is a zip written by
//! a tool from a specification that requires exactly those two methods, and
//! everything else is refused rather than half-supported.

use std::collections::BTreeMap;

/// Signature of an entry in the central directory.
const CENTRAL_HEADER: u32 = 0x0201_4b50;
/// Signature of the record that says where the central directory is.
const END_OF_DIRECTORY: u32 = 0x0605_4b50;
/// Signature of the header immediately before an entry's data.
const LOCAL_HEADER: u32 = 0x0403_4b50;

/// Fixed part of a central directory entry, before its name.
const CENTRAL_HEADER_LEN: usize = 46;
/// Fixed part of a local header, before its name.
const LOCAL_HEADER_LEN: usize = 30;
/// Fixed length of the end-of-directory record, without its comment.
const END_OF_DIRECTORY_LEN: usize = 22;

/// The most an archive may hold once unpacked.
///
/// A zip can claim any uncompressed size it likes, and a few hundred bytes of
/// deflate can claim gigabytes. On a device with 512 MB shared with the stock
/// reader, believing that claim is how an application is killed rather than
/// told the file was odd.
const MAX_UNPACKED: usize = 64 * 1024 * 1024;

/// The most entries an archive may hold.
const MAX_ENTRIES: usize = 8_192;

/// Compression method: kept as written.
const STORED: u16 = 0;
/// Compression method: deflate.
const DEFLATED: u16 = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Fault {
    /// No end-of-directory record: these bytes are not a zip archive.
    NotAnArchive,
    /// The structure is a zip's, but an offset or a length does not lead
    /// anywhere inside the file.
    Damaged,
    /// Stored with something other than *stored* or *deflate*.
    Unsupported,
    /// Claims to unpack to more than [`MAX_UNPACKED`].
    TooLarge,
}

impl std::fmt::Display for Fault {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str(match self {
            Self::NotAnArchive => "not an archive",
            Self::Damaged => "the archive is damaged",
            Self::Unsupported => "the archive is packed in a way this reader does not know",
            Self::TooLarge => "the archive is too large to unpack",
        })
    }
}

impl std::error::Error for Fault {}

#[derive(Clone, Copy)]
struct Entry {
    /// Offset of the *local* header, which is where the data can be found.
    at: usize,
    method: u16,
    packed: usize,
    unpacked: usize,
}

/// An archive held in memory, with its table of contents read.
pub struct Archive<'a> {
    bytes: &'a [u8],
    /// Sorted, so that looking for a member by a path that differs only in
    /// case — which EPUBs written on Windows do — has somewhere to start.
    entries: BTreeMap<String, Entry>,
}

impl<'a> Archive<'a> {
    /// Reads the table of contents. Nothing is unpacked here.
    ///
    /// # Errors
    ///
    /// [`Fault::NotAnArchive`] when there is no end-of-directory record, and
    /// [`Fault::Damaged`] when one is found but does not point at a directory
    /// inside the file.
    pub fn open(bytes: &'a [u8]) -> Result<Self, Fault> {
        let end = find_end_of_directory(bytes).ok_or(Fault::NotAnArchive)?;
        let count = u16_at(bytes, end + 10).ok_or(Fault::Damaged)? as usize;
        let mut at = u32_at(bytes, end + 16).ok_or(Fault::Damaged)? as usize;

        let mut entries = BTreeMap::new();
        for _ in 0..count.min(MAX_ENTRIES) {
            if u32_at(bytes, at) != Some(CENTRAL_HEADER) {
                // The count in the record is a claim like any other. Stopping
                // at the first thing that is not an entry keeps whatever was
                // read before it, which is how a truncated download still
                // yields the chapters that did arrive.
                break;
            }
            let method = u16_at(bytes, at + 10).ok_or(Fault::Damaged)?;
            let packed = u32_at(bytes, at + 20).ok_or(Fault::Damaged)? as usize;
            let unpacked = u32_at(bytes, at + 24).ok_or(Fault::Damaged)? as usize;
            let name_len = u16_at(bytes, at + 28).ok_or(Fault::Damaged)? as usize;
            let extra_len = u16_at(bytes, at + 30).ok_or(Fault::Damaged)? as usize;
            let comment_len = u16_at(bytes, at + 32).ok_or(Fault::Damaged)? as usize;
            let offset = u32_at(bytes, at + 42).ok_or(Fault::Damaged)? as usize;

            let name_at = at + CENTRAL_HEADER_LEN;
            let name = bytes
                .get(name_at..name_at + name_len)
                .ok_or(Fault::Damaged)?;
            // A member name is text in the archive's own encoding. It is only
            // ever compared against names from the archive's own manifest, so
            // an unrepresentable byte matters less than refusing the book.
            let name = String::from_utf8_lossy(name).into_owned();
            entries.insert(
                normalise(&name),
                Entry {
                    at: offset,
                    method,
                    packed,
                    unpacked,
                },
            );
            at = name_at + name_len + extra_len + comment_len;
        }
        Ok(Self { bytes, entries })
    }

    /// Every member's name, in order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    /// Unpacks one member.
    ///
    /// # Errors
    ///
    /// [`Fault::Damaged`] when the member is not there or its header does not
    /// lead to its data, [`Fault::Unsupported`] for a compression method this
    /// does not know, and [`Fault::TooLarge`] when it claims to unpack past
    /// [`MAX_UNPACKED`].
    pub fn read(&self, name: &str) -> Result<Vec<u8>, Fault> {
        let entry = *self.entries.get(&normalise(name)).ok_or(Fault::Damaged)?;
        if entry.unpacked > MAX_UNPACKED {
            return Err(Fault::TooLarge);
        }
        // The local header repeats the name and the extra field, and its
        // *lengths* are the authoritative ones: a writer is allowed to put
        // different extra fields in the two places, so the central directory's
        // lengths cannot be used to find the data.
        if u32_at(self.bytes, entry.at) != Some(LOCAL_HEADER) {
            return Err(Fault::Damaged);
        }
        let name_len = u16_at(self.bytes, entry.at + 26).ok_or(Fault::Damaged)? as usize;
        let extra_len = u16_at(self.bytes, entry.at + 28).ok_or(Fault::Damaged)? as usize;
        let from = entry.at + LOCAL_HEADER_LEN + name_len + extra_len;
        let packed = self
            .bytes
            .get(from..from.checked_add(entry.packed).ok_or(Fault::Damaged)?)
            .ok_or(Fault::Damaged)?;

        match entry.method {
            STORED => Ok(packed.to_vec()),
            DEFLATED => {
                // The limit is the smaller of what the entry claims and what
                // is allowed, so a member claiming four gigabytes cannot make
                // the decompressor reserve four gigabytes before failing.
                let limit = entry.unpacked.clamp(1, MAX_UNPACKED);
                miniz_oxide::inflate::decompress_to_vec_with_limit(packed, limit)
                    .map_err(|_| Fault::Damaged)
            }
            _ => Err(Fault::Unsupported),
        }
    }
}

/// Puts a member name into the one form used for lookups.
///
/// Separators are folded because an archive written on Windows may use
/// backslashes, and case is folded because a manifest written by hand often
/// disagrees with the archive about it. Both are wrong strictly, and both are
/// the difference between a book opening and not.
fn normalise(name: &str) -> String {
    name.replace('\\', "/")
        .trim_start_matches("./")
        .to_ascii_lowercase()
}

/// Finds the end-of-directory record, searching back from the end.
///
/// It is last in the file, but a variable-length comment may follow it, so its
/// position is not fixed. The comment length is sixteen bits, which bounds the
/// search: past sixty-five kilobytes from the end there is nothing to find.
fn find_end_of_directory(bytes: &[u8]) -> Option<usize> {
    let furthest = bytes.len().saturating_sub(END_OF_DIRECTORY_LEN + 0xffff);
    let mut at = bytes.len().checked_sub(END_OF_DIRECTORY_LEN)?;
    loop {
        if u32_at(bytes, at) == Some(END_OF_DIRECTORY) {
            return Some(at);
        }
        if at == furthest {
            return None;
        }
        at -= 1;
    }
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    let field = bytes.get(at..at.checked_add(2)?)?;
    Some(u16::from_le_bytes([field[0], field[1]]))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    let field = bytes.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes([field[0], field[1], field[2], field[3]]))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Builds a zip archive with every member stored uncompressed.
    ///
    /// Written here rather than pulled in so that the tests exercise the
    /// reader against bytes laid out by something that is not the reader.
    pub(crate) fn archive(members: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out: Vec<u8> = Vec::new();
        let mut directory: Vec<u8> = Vec::new();
        for (name, body) in members {
            let at = u32::try_from(out.len()).expect("a small test archive");
            let size = u32::try_from(body.len()).expect("a small test member");
            let name_len = u16::try_from(name.len()).expect("a short test name");

            // Offsets are spelled out because getting one wrong here produces
            // an archive the reader cannot read, which looks exactly like the
            // reader being broken.
            out.extend_from_slice(&LOCAL_HEADER.to_le_bytes()); // 0
            out.extend_from_slice(&20u16.to_le_bytes()); // 4  version needed
            out.extend_from_slice(&0u16.to_le_bytes()); // 6  flags
            out.extend_from_slice(&STORED.to_le_bytes()); // 8  method
            out.extend_from_slice(&[0; 4]); // 10 time and date
            out.extend_from_slice(&[0; 4]); // 14 crc, not checked here
            out.extend_from_slice(&size.to_le_bytes()); // 18 packed
            out.extend_from_slice(&size.to_le_bytes()); // 22 unpacked
            out.extend_from_slice(&name_len.to_le_bytes()); // 26
            out.extend_from_slice(&0u16.to_le_bytes()); // 28 extra
            out.extend_from_slice(name.as_bytes()); // 30
            out.extend_from_slice(body);

            directory.extend_from_slice(&CENTRAL_HEADER.to_le_bytes()); // 0
            directory.extend_from_slice(&20u16.to_le_bytes()); // 4  version made by
            directory.extend_from_slice(&20u16.to_le_bytes()); // 6  version needed
            directory.extend_from_slice(&0u16.to_le_bytes()); // 8  flags
            directory.extend_from_slice(&STORED.to_le_bytes()); // 10 method
            directory.extend_from_slice(&[0; 4]); // 12 time and date
            directory.extend_from_slice(&[0; 4]); // 16 crc
            directory.extend_from_slice(&size.to_le_bytes()); // 20 packed
            directory.extend_from_slice(&size.to_le_bytes()); // 24 unpacked
            directory.extend_from_slice(&name_len.to_le_bytes()); // 28
            directory.extend_from_slice(&0u16.to_le_bytes()); // 30 extra
            directory.extend_from_slice(&0u16.to_le_bytes()); // 32 comment
            directory.extend_from_slice(&0u16.to_le_bytes()); // 34 disk
            directory.extend_from_slice(&0u16.to_le_bytes()); // 36 internal attributes
            directory.extend_from_slice(&[0; 4]); // 38 external attributes
            directory.extend_from_slice(&at.to_le_bytes()); // 42
            directory.extend_from_slice(name.as_bytes()); // 46
        }
        let directory_at = u32::try_from(out.len()).expect("a small test archive");
        let directory_len = u32::try_from(directory.len()).expect("a small test directory");
        let count = u16::try_from(members.len()).expect("few test members");
        out.extend_from_slice(&directory);
        out.extend_from_slice(&END_OF_DIRECTORY.to_le_bytes()); // 0
        out.extend_from_slice(&[0; 4]); // 4  this disk, disk with the directory
        out.extend_from_slice(&count.to_le_bytes()); // 8  entries on this disk
        out.extend_from_slice(&count.to_le_bytes()); // 10 entries in total
        out.extend_from_slice(&directory_len.to_le_bytes()); // 12
        out.extend_from_slice(&directory_at.to_le_bytes()); // 16
        out.extend_from_slice(&0u16.to_le_bytes()); // 20 comment length
        out
    }

    #[test]
    fn a_member_comes_back_as_it_went_in() {
        let bytes = archive(&[("one.txt", b"first"), ("two.txt", b"second")]);
        let opened = Archive::open(&bytes).expect("a readable archive");
        assert_eq!(opened.read("one.txt").as_deref(), Ok(&b"first"[..]));
        assert_eq!(opened.read("two.txt").as_deref(), Ok(&b"second"[..]));
    }

    #[test]
    fn every_member_is_listed() {
        let bytes = archive(&[("a", b"1"), ("b", b"2")]);
        let opened = Archive::open(&bytes).expect("a readable archive");
        assert_eq!(opened.names().collect::<Vec<_>>(), vec!["a", "b"]);
    }

    #[test]
    fn a_name_written_the_other_way_round_still_finds_its_member() {
        // An EPUB's manifest and its archive disagree about case and about
        // separators often enough that being strict means the book does not
        // open at all.
        let bytes = archive(&[("OEBPS\\Chapter1.xhtml", b"words")]);
        let opened = Archive::open(&bytes).expect("a readable archive");
        assert_eq!(
            opened.read("OEBPS/chapter1.xhtml").as_deref(),
            Ok(&b"words"[..])
        );
    }

    #[test]
    fn bytes_that_are_not_an_archive_are_refused_rather_than_guessed_at() {
        for bytes in [&b""[..], &b"not a zip"[..], &[0u8; 4096][..]] {
            assert!(
                matches!(Archive::open(bytes), Err(Fault::NotAnArchive)),
                "{} bytes of nothing were read as an archive",
                bytes.len()
            );
        }
    }

    #[test]
    fn a_member_that_is_not_there_is_not_an_answer() {
        let bytes = archive(&[("a", b"1")]);
        let opened = Archive::open(&bytes).expect("a readable archive");
        assert_eq!(opened.read("b").unwrap_err(), Fault::Damaged);
    }

    #[test]
    fn an_archive_cut_short_still_yields_what_arrived() {
        // The count in the end record is a claim. Trusting it over what is
        // actually there turns a partial download into no download.
        let mut bytes = archive(&[("a", b"1"), ("b", b"2")]);
        let len = bytes.len();
        // Say there are eight entries when there are two.
        bytes[len - 12] = 8;
        let opened = Archive::open(&bytes).expect("a readable archive");
        assert_eq!(opened.names().count(), 2);
    }

    #[test]
    fn an_offset_that_leads_outside_the_file_is_damage_and_not_a_panic() {
        let mut bytes = archive(&[("a", b"1")]);
        let len = bytes.len();
        // Point the directory past the end of the file.
        bytes[len - 6..len - 2].copy_from_slice(&0xffff_fff0u32.to_le_bytes());
        let opened = Archive::open(&bytes).expect("an end record is still there");
        assert_eq!(opened.names().count(), 0);
    }

    #[test]
    fn an_unknown_compression_method_is_refused() {
        let mut bytes = archive(&[("a", b"1")]);
        // The method lives at offset 10 of the central header, which follows
        // the single local entry.
        let directory = bytes
            .windows(4)
            .position(|window| window == CENTRAL_HEADER.to_le_bytes())
            .expect("a central directory");
        bytes[directory + 10] = 99;
        let opened = Archive::open(&bytes).expect("a readable archive");
        assert_eq!(opened.read("a").unwrap_err(), Fault::Unsupported);
    }

    #[test]
    fn nothing_here_panics_on_anything() {
        let good = archive(&[("a", b"words"), ("b/c.xhtml", b"<p>more</p>")]);
        for cut in 0..good.len() {
            if let Ok(opened) = Archive::open(&good[..cut]) {
                for name in opened.names().map(str::to_owned).collect::<Vec<_>>() {
                    let _ = opened.read(&name);
                }
            }
        }
        // And every single-byte corruption of a whole archive.
        for at in 0..good.len() {
            let mut bytes = good.clone();
            bytes[at] ^= 0xff;
            if let Ok(opened) = Archive::open(&bytes) {
                for name in opened.names().map(str::to_owned).collect::<Vec<_>>() {
                    let _ = opened.read(&name);
                }
            }
        }
    }
}

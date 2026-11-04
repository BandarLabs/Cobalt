//! The audiobooks already on the reader, and how an application reaches one.
//!
//! This is [`crate::library`] for sound. The same roots are walked, under
//! the same bounds and the same rule that an application never sees a path:
//! what comes back is an opaque identifier and the few facts a shelf needs.
//! What differs is what a book is. A document is one file; an audiobook is
//! as often a folder of files, one per chapter or per disc, and listing
//! those as forty separate books would make a shelf of one book look like a
//! shelf of forty. So a folder whose audio files read as parts of one thing
//! is listed once, as a book with that many parts, and resolves to all of
//! them in playing order.
//!
//! Nothing is opened. A listing of two thousand files that read every tag
//! would take the reader's one core long enough that the shelf would seem
//! to have hung, so titles are file names, and an application learns the
//! rest when it opens a book.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::library::{skipped_directory, MAX_DEPTH, MAX_PER_DIRECTORY, ROOTS};

/// How many books one listing may contain.
pub const MAX_ENTRIES: usize = kobo_protocol::MAX_AUDIO_ENTRIES;
/// How many files a folder may contribute as parts of one book.
pub const MAX_PARTS: usize = 1_000;

/// What an audio file is, by its name.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    Mp3,
    Mp4,
    Ogg,
    Opus,
    Flac,
    Wav,
    Mp3z,
}

impl Kind {
    /// The kind a file name implies, if it implies one.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        let lowered = name.to_ascii_lowercase();
        let (_, suffix) = lowered.rsplit_once('.')?;
        match suffix {
            "mp3" => Some(Self::Mp3),
            "m4b" | "m4a" => Some(Self::Mp4),
            "ogg" | "oga" => Some(Self::Ogg),
            "opus" => Some(Self::Opus),
            "flac" => Some(Self::Flac),
            "wav" => Some(Self::Wav),
            "mp3z" => Some(Self::Mp3z),
            _ => None,
        }
    }

    /// The wire's number for the kind.
    #[must_use]
    pub const fn wire(self) -> u8 {
        match self {
            Self::Mp3 => 1,
            Self::Mp4 => 2,
            Self::Ogg => 3,
            Self::Opus => 4,
            Self::Flac => 5,
            Self::Wav => 6,
            Self::Mp3z => 7,
        }
    }
}

/// One audiobook the owner has.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Entry {
    /// The root's index and the path under it, which is what resolves.
    pub id: String,
    /// The file or folder name without the suffix that named its format.
    pub title: String,
    pub kind: Kind,
    /// How many files the book is made of.
    pub parts: u16,
    pub bytes: u64,
}

/// What a listing found, and whether it is the whole story.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Listing {
    pub entries: Vec<Entry>,
    pub truncated: bool,
}

/// Where an identifier leads: one file, or a folder's files in playing
/// order under the folder's name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Location {
    File(PathBuf),
    Parts { title: String, paths: Vec<PathBuf> },
}

/// Every audiobook under `roots`, in title order.
#[must_use]
pub fn list_in(roots: &[PathBuf]) -> Listing {
    let mut listing = Listing::default();
    let mut seen = BTreeSet::new();
    for (index, root) in roots.iter().enumerate() {
        if !root.is_dir() {
            continue;
        }
        walk(root, root, index, 0, &mut listing, &mut seen);
        if listing.truncated {
            break;
        }
    }
    listing.entries.sort_by(|left, right| {
        left.title
            .to_ascii_lowercase()
            .cmp(&right.title.to_ascii_lowercase())
            .then_with(|| left.id.cmp(&right.id))
    });
    listing
}

/// The audiobooks under the compiled-in roots.
#[must_use]
pub fn list() -> Listing {
    list_in(&ROOTS.iter().map(PathBuf::from).collect::<Vec<_>>())
}

/// Where an identifier from a listing leads, under `roots`, or nothing
/// for an identifier no listing could have produced.
#[must_use]
pub fn resolve_in(roots: &[PathBuf], id: &str) -> Option<Location> {
    let (index, relative) = id.split_once('/')?;
    let root = roots.get(index.parse::<usize>().ok()?)?;
    if relative.is_empty() || Path::new(relative).is_absolute() {
        return None;
    }
    if Path::new(relative)
        .components()
        .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return None;
    }
    let candidate = root.join(relative);
    let real = fs::canonicalize(&candidate).ok()?;
    let real_root = fs::canonicalize(root).ok()?;
    let parts = real
        .strip_prefix(&real_root)
        .ok()?
        .components()
        .map(|part| part.as_os_str().to_str())
        .collect::<Option<Vec<_>>>()?;
    let name = *parts.last()?;
    if parts.iter().any(|part| skipped_directory(part)) {
        return None;
    }
    if real.is_file() {
        return Kind::from_name(name).map(|_| Location::File(real));
    }
    if !real.is_dir() {
        return None;
    }
    let files = audio_files(&real)?;
    let names = files
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>();
    if !reads_as_parts(&names) {
        return None;
    }
    Some(Location::Parts {
        title: name.to_owned(),
        paths: files.into_iter().map(|(_, path)| path).collect(),
    })
}

/// Where an identifier leads under the compiled-in roots.
#[must_use]
pub fn resolve(id: &str) -> Option<Location> {
    resolve_in(&ROOTS.iter().map(PathBuf::from).collect::<Vec<_>>(), id)
}

/// The wire form of one entry.
#[must_use]
pub fn to_wire(entry: Entry) -> kobo_protocol::AudioEntry {
    kobo_protocol::AudioEntry {
        id: entry.id,
        title: crate::library::clip_title(&entry.title),
        kind: entry.kind.wire(),
        parts: entry.parts,
        bytes: entry.bytes,
    }
}

fn walk(
    root: &Path,
    directory: &Path,
    root_index: usize,
    depth: usize,
    listing: &mut Listing,
    seen: &mut BTreeSet<PathBuf>,
) {
    if depth > MAX_DEPTH {
        listing.truncated = true;
        return;
    }
    let Ok(real) = fs::canonicalize(directory) else {
        return;
    };
    if !seen.insert(real) {
        return;
    }
    let Ok(reader) = fs::read_dir(directory) else {
        return;
    };
    let mut files = Vec::new();
    let mut folders = Vec::new();
    let mut looked_at = 0;
    for entry in reader.flatten() {
        looked_at += 1;
        if looked_at > MAX_PER_DIRECTORY {
            listing.truncated = true;
            return;
        }
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if !skipped_directory(name) {
                folders.push(path);
            }
        } else if kind.is_file() {
            if let Some(audio) = Kind::from_name(name) {
                let bytes = entry.metadata().map_or(0, |data| data.len());
                if bytes > 0 {
                    files.push((name.to_owned(), path, audio, bytes));
                }
            }
        }
    }
    files.sort_by(|left, right| natural_order(&left.0, &right.0));
    let names = files
        .iter()
        .map(|(name, _, _, _)| name.as_str())
        .collect::<Vec<_>>();
    // The folder's own files first: as one book when they read as parts,
    // else one book each.
    if depth > 0 && reads_as_parts(&names) {
        if let Some(entry) = folder_entry(root, directory, root_index, &files) {
            if listing.entries.len() >= MAX_ENTRIES {
                listing.truncated = true;
                return;
            }
            listing.entries.push(entry);
        }
    } else {
        for (name, path, kind, bytes) in &files {
            if listing.entries.len() >= MAX_ENTRIES {
                listing.truncated = true;
                return;
            }
            let Some(id) = id_of(root, path, root_index) else {
                continue;
            };
            listing.entries.push(Entry {
                id,
                title: title_of(name),
                kind: *kind,
                parts: 1,
                bytes: *bytes,
            });
        }
    }
    folders.sort();
    for folder in folders {
        walk(root, &folder, root_index, depth + 1, listing, seen);
        if listing.truncated {
            return;
        }
    }
}

fn folder_entry(
    root: &Path,
    directory: &Path,
    root_index: usize,
    files: &[(String, PathBuf, Kind, u64)],
) -> Option<Entry> {
    let id = id_of(root, directory, root_index)?;
    let name = directory.file_name()?.to_str()?;
    let (_, _, kind, _) = files.first()?;
    Some(Entry {
        id,
        title: name.to_owned(),
        kind: *kind,
        parts: u16::try_from(files.len()).unwrap_or(u16::MAX),
        bytes: files.iter().map(|(_, _, _, bytes)| *bytes).sum(),
    })
}

fn id_of(root: &Path, path: &Path, root_index: usize) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?.to_str()?;
    Some(format!("{root_index}/{relative}"))
}

/// The audio files in a folder, in playing order, or nothing for a folder
/// with none or too many.
fn audio_files(directory: &Path) -> Option<Vec<(String, PathBuf)>> {
    let mut files = fs::read_dir(directory)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?.to_owned();
            let is_file = entry.file_type().ok()?.is_file();
            (is_file && Kind::from_name(&name).is_some()).then_some((name, path))
        })
        .collect::<Vec<_>>();
    if files.is_empty() || files.len() > MAX_PARTS {
        return None;
    }
    files.sort_by(|left, right| natural_order(&left.0, &right.0));
    Some(files)
}

/// Whether a folder's audio files are parts of one book rather than
/// several books that happen to share a folder.
///
/// Two or more files are parts when every one of them carries a number,
/// which is how discs and chapters are named ("01 - The Hobbit.mp3",
/// "Chapter 12.mp3", "Track03.mp3"), or when their names are the same once
/// the digits are taken out, which is how converters name them. A folder
/// of "Dune.m4b" and "Emma.m4b" is neither, and lists as two books. One
/// file alone is never a folder book: it lists under its own name.
#[must_use]
pub fn reads_as_parts(names: &[&str]) -> bool {
    if names.len() < 2 || names.len() > MAX_PARTS {
        return false;
    }
    let numbered = names
        .iter()
        .all(|name| title_of(name).bytes().any(|byte| byte.is_ascii_digit()));
    if numbered {
        return true;
    }
    let mut stems = names.iter().map(|name| {
        title_of(name)
            .chars()
            .filter(|character| !character.is_ascii_digit())
            .collect::<String>()
            .trim_matches(|character: char| !character.is_alphanumeric())
            .to_ascii_lowercase()
    });
    let first = stems.next().unwrap_or_default();
    !first.is_empty() && stems.all(|stem| stem == first)
}

/// The file name without the suffix that named its format.
fn title_of(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, suffix)) if Kind::from_name(&format!("x.{suffix}")).is_some() => {
            stem.to_owned()
        }
        _ => name.to_owned(),
    }
}

/// Orders names as a person reads them: "Part 2" before "Part 10".
fn natural_order(left: &str, right: &str) -> std::cmp::Ordering {
    let mut left_chunks = chunks(left).into_iter();
    let mut right_chunks = chunks(right).into_iter();
    loop {
        match (left_chunks.next(), right_chunks.next()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(a), Some(b)) => {
                let ordering = match (a.parse::<u64>(), b.parse::<u64>()) {
                    (Ok(a), Ok(b)) => a.cmp(&b),
                    _ => a.to_ascii_lowercase().cmp(&b.to_ascii_lowercase()),
                };
                if ordering != std::cmp::Ordering::Equal {
                    return ordering;
                }
            }
        }
    }
}

/// A name cut into runs of digits and runs of everything else.
fn chunks(name: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for character in name.chars() {
        let digit = character.is_ascii_digit();
        match out.last_mut() {
            Some(last) if last.chars().next().is_some_and(|c| c.is_ascii_digit()) == digit => {
                last.push(character);
            }
            _ => out.push(character.to_string()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{list_in, natural_order, reads_as_parts, resolve_in, Kind, Location};
    use std::fs;
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("kobo-audio-library-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("a scratch folder");
        path
    }

    #[test]
    fn numbered_files_and_same_named_files_are_parts_and_mixed_titles_are_not() {
        assert!(reads_as_parts(&["01 - Dune.mp3", "02 - Dune.mp3"]));
        assert!(reads_as_parts(&[
            "Chapter 1.m4a",
            "Chapter 2.m4a",
            "Chapter 3.m4a"
        ]));
        assert!(reads_as_parts(&["Track01.mp3", "Track02.mp3"]));
        assert!(reads_as_parts(&["dune_01.mp3", "dune_02.mp3"]));
        assert!(!reads_as_parts(&["Dune.m4b", "Emma.m4b"]));
        assert!(!reads_as_parts(&["Dune.m4b"]));
        assert!(!reads_as_parts(&[]));
        // One numbered and one not: two books, since nothing says otherwise.
        assert!(!reads_as_parts(&["Dune 1.m4b", "Emma.m4b"]));
    }

    #[test]
    fn names_are_ordered_the_way_a_person_reads_them() {
        let mut names = vec!["Part 10.mp3", "Part 2.mp3", "Part 1.mp3", "part 3.mp3"];
        names.sort_by(|left, right| natural_order(left, right));
        assert_eq!(
            names,
            ["Part 1.mp3", "Part 2.mp3", "part 3.mp3", "Part 10.mp3"]
        );
    }

    #[test]
    fn a_folder_of_parts_lists_once_and_resolves_to_its_files_in_order() {
        let root = scratch("parts");
        let dune = root.join("Audiobooks").join("Dune");
        fs::create_dir_all(&dune).expect("a book folder");
        for name in [
            "10 - End.mp3",
            "2 - Middle.mp3",
            "1 - Start.mp3",
            "cover.jpg",
        ] {
            fs::write(dune.join(name), b"bytes").expect("a part");
        }
        fs::write(root.join("Audiobooks").join("Emma.m4b"), b"book").expect("a single book");
        fs::write(root.join("Audiobooks").join("Notes.txt"), b"not sound").expect("a note");
        let roots = vec![root.clone()];
        let listing = list_in(&roots);
        let found = listing
            .entries
            .iter()
            .map(|entry| {
                (
                    entry.id.as_str(),
                    entry.title.as_str(),
                    entry.kind,
                    entry.parts,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            found,
            [
                ("0/Audiobooks/Dune", "Dune", Kind::Mp3, 3),
                ("0/Audiobooks/Emma.m4b", "Emma", Kind::Mp4, 1),
            ]
        );
        assert!(!listing.truncated);
        let Some(Location::Parts { title, paths }) = resolve_in(&roots, "0/Audiobooks/Dune") else {
            panic!("the folder resolves to its parts");
        };
        assert_eq!(title, "Dune");
        let names = paths
            .iter()
            .map(|path| path.file_name().unwrap().to_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(names, ["1 - Start.mp3", "2 - Middle.mp3", "10 - End.mp3"]);
        assert_eq!(
            resolve_in(&roots, "0/Audiobooks/Emma.m4b"),
            Some(Location::File(
                fs::canonicalize(root.join("Audiobooks").join("Emma.m4b")).unwrap()
            ))
        );
    }

    #[test]
    fn a_folder_of_separate_books_lists_each_and_resolves_to_none() {
        let root = scratch("separate");
        let shelf = root.join("Audiobooks");
        fs::create_dir_all(&shelf).expect("a folder");
        fs::write(shelf.join("Dune.m4b"), b"one").expect("a book");
        fs::write(shelf.join("Emma.mp3"), b"two").expect("a book");
        let roots = vec![root];
        let titles = list_in(&roots)
            .entries
            .iter()
            .map(|entry| entry.title.clone())
            .collect::<Vec<_>>();
        assert_eq!(titles, ["Dune", "Emma"]);
        assert_eq!(resolve_in(&roots, "0/Audiobooks"), None);
    }

    #[test]
    fn the_root_itself_never_reads_as_one_book() {
        // A card root holding "01.mp3" and "02.mp3" is two sideloaded files,
        // not a book called after the card.
        let root = scratch("root-files");
        fs::write(root.join("01.mp3"), b"one").expect("a file");
        fs::write(root.join("02.mp3"), b"two").expect("a file");
        let listing = list_in(&[root]);
        assert_eq!(listing.entries.len(), 2);
    }

    #[test]
    fn only_what_a_listing_could_name_resolves() {
        let root = scratch("confined");
        let hidden = root.join(".kobo");
        fs::create_dir_all(&hidden).expect("a hidden folder");
        fs::write(hidden.join("secret.mp3"), b"no").expect("a hidden file");
        fs::write(root.join("book.epub"), b"no").expect("a document");
        let roots = vec![root];
        assert_eq!(resolve_in(&roots, "0/.kobo/secret.mp3"), None);
        assert_eq!(resolve_in(&roots, "0/book.epub"), None);
        assert_eq!(resolve_in(&roots, "0/../book.mp3"), None);
        assert_eq!(resolve_in(&roots, "1/book.mp3"), None);
        assert!(list_in(&roots).entries.is_empty());
    }
}

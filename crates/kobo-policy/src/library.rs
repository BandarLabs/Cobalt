//! The documents already on the device, and how an application reaches one.
//!
//! # Why the runtime does the looking
//!
//! An application cannot open a file, and that is the point: a store app that
//! could read `KoboReader.sqlite` would be reading the owner's entire reading
//! history. But a library is exactly the case that rule is wrong for. The
//! books are already on the card, the owner put them there, and an
//! application that cannot see them is one that makes them download a book
//! they already own.
//!
//! So the search happens here instead. An application asks for the library
//! and gets back a list of entries; it never names a directory, never sees an
//! absolute path, and cannot ask for one. What comes back is an opaque
//! identifier and the few facts needed to draw a shelf.
//!
//! # Why the roots are a fixed list
//!
//! A configurable root is a filesystem grant with extra steps: whoever writes
//! the configuration decides what the capability means, and the answer stops
//! being reviewable. The roots here are the places books actually live on this
//! hardware, they are compiled in, and adding one is a change to this file
//! that somebody has to justify.
//!
//! `/mnt/onboard/.kobo` is deliberately absent. That is the stock reader's own
//! state -- its database, its covers, its kepub cache -- and none of it is a
//! book the owner chose to put somewhere.
//!
//! # Why a walk is bounded three ways
//!
//! The card is removable and its contents are not ours. A reader with a deep
//! Calibre tree, a symlink loop, or forty thousand text files must not be able
//! to make the runtime walk forever or answer with a list no panel can draw.
//! Depth, total entries and per-directory entries are all capped, and the walk
//! stops rather than truncating silently: a shelf that says it is showing
//! everything when it is not is worse than one that says it stopped.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Where books live on this hardware.
///
/// `/mnt/onboard` is the visible user partition: what appears when the reader
/// is plugged in by USB, and where both sideloaded books and the stock
/// reader's own downloads sit. Cobalt's data root is where applications
/// publish their shelves.
pub const ROOTS: &[&str] = &["/mnt/onboard", "/mnt/onboard/.adds/cobalt/data", "/mnt/sd"];

/// Directory names never descended into, matched at any depth.
///
/// The stock reader's state, our own program files, and the noise that
/// desktop operating systems leave on removable media.
const SKIPPED: &[&str] = &[
    ".kobo",
    ".kobo-images",
    ".adobe-digital-editions",
    ".Spotlight-V100",
    ".Trashes",
    ".fseventsd",
    "System Volume Information",
    "$RECYCLE.BIN",
];

/// How deep below a root the walk will go.
pub const MAX_DEPTH: usize = 6;

/// How many documents one listing may contain.
pub const MAX_ENTRIES: usize = 2_000;

/// How many directory entries one directory contributes before the walk gives
/// up on it.
pub const MAX_PER_DIRECTORY: usize = 4_000;

/// The largest document the runtime will hand to an application.
pub const MAX_DOCUMENT_BYTES: u64 = 64 * 1024 * 1024;

/// What a document is, as far as a shelf is concerned.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Kind {
    Epub,
    Markdown,
    Html,
    Text,
    /// Listed, and not openable on this device.
    ///
    /// Nothing here parses PDF, and the honest answer on a shelf is to show
    /// the book with the reason rather than to hide it or to open something
    /// that is not it.
    Pdf,
}

impl Kind {
    /// The short badge a tile draws in its corner.
    #[must_use]
    pub const fn badge(self) -> &'static str {
        match self {
            Self::Epub => "EPUB",
            Self::Markdown => "MD",
            Self::Html => "HTML",
            Self::Text => "TXT",
            Self::Pdf => "PDF",
        }
    }

    /// Whether the built-in reader can page this.
    #[must_use]
    pub const fn is_readable(self) -> bool {
        !matches!(self, Self::Pdf)
    }

    /// The kind a file name implies, if it implies one.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        let lowered = name.to_ascii_lowercase();
        for (suffix, kind) in [
            (".epub", Self::Epub),
            (".kepub.epub", Self::Epub),
            (".md", Self::Markdown),
            (".markdown", Self::Markdown),
            (".html", Self::Html),
            (".htm", Self::Html),
            (".xhtml", Self::Html),
            (".txt", Self::Text),
            (".pdf", Self::Pdf),
        ] {
            if lowered.ends_with(suffix) {
                return Some(kind);
            }
        }
        None
    }
}

/// One document the owner already has.
///
/// `id` is what an application passes back to read the document. It is the
/// path relative to its root, prefixed by the root's index, so it is stable
/// across listings, means nothing outside this module, and cannot be turned
/// into somewhere else by an application that edits it: resolving checks the
/// result is still inside the root it claims.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Entry {
    pub id: String,
    /// The file name without its suffix, which is the best title available
    /// without opening the document.
    pub title: String,
    pub kind: Kind,
    pub bytes: u64,
}

/// What a listing found, and whether it is the whole story.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Listing {
    pub entries: Vec<Entry>,
    /// True when a bound stopped the walk before it ran out of places to
    /// look. Said out loud so a shelf can tell the owner it is partial.
    pub truncated: bool,
}

/// Every document under `roots`, in title order.
///
/// Roots that do not exist are skipped rather than reported: a device without
/// an SD card is not an error.
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

/// The documents under the compiled-in [`ROOTS`].
#[must_use]
pub fn list() -> Listing {
    list_in(&ROOTS.iter().map(PathBuf::from).collect::<Vec<_>>())
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
    // A symlinked directory that points back up its own tree would otherwise
    // be walked until the depth cap caught it, once per way in.
    let Ok(real) = fs::canonicalize(directory) else {
        return;
    };
    if !seen.insert(real) {
        return;
    }
    let Ok(reader) = fs::read_dir(directory) else {
        return;
    };
    let mut looked_at = 0;
    for entry in reader.flatten() {
        looked_at += 1;
        if looked_at > MAX_PER_DIRECTORY {
            listing.truncated = true;
            return;
        }
        if listing.entries.len() >= MAX_ENTRIES {
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
            if SKIPPED.contains(&name) {
                continue;
            }
            walk(root, &path, root_index, depth + 1, listing, seen);
            if listing.truncated {
                return;
            }
            continue;
        }
        if !kind.is_file() {
            continue;
        }
        let Some(document) = Kind::from_name(name) else {
            continue;
        };
        let bytes = entry.metadata().map_or(0, |data| data.len());
        if bytes == 0 || bytes > MAX_DOCUMENT_BYTES {
            continue;
        }
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let Some(relative) = relative.to_str() else {
            continue;
        };
        listing.entries.push(Entry {
            id: format!("{root_index}/{relative}"),
            title: title_of(name),
            kind: document,
            bytes,
        });
    }
}

/// The file name without the suffix that named its format.
fn title_of(name: &str) -> String {
    let lowered = name.to_ascii_lowercase();
    for suffix in [
        ".kepub.epub",
        ".epub",
        ".markdown",
        ".md",
        ".xhtml",
        ".html",
        ".htm",
        ".txt",
        ".pdf",
    ] {
        if lowered.ends_with(suffix) {
            return name[..name.len() - suffix.len()].to_owned();
        }
    }
    name.to_owned()
}

/// The path an identifier names, if it names one inside its own root.
///
/// Every part of this is a refusal rather than a repair. An identifier that
/// does not parse, points at a root that does not exist, or escapes that root
/// once resolved is answered with nothing at all, because the only way to
/// produce one is to have edited it.
#[must_use]
pub fn resolve_in(roots: &[PathBuf], id: &str) -> Option<PathBuf> {
    let (index, relative) = id.split_once('/')?;
    let root = roots.get(index.parse::<usize>().ok()?)?;
    if relative.is_empty() || Path::new(relative).is_absolute() {
        return None;
    }
    // Rejected before touching the disk, so a traversal never becomes a read
    // that merely happened to fail.
    if Path::new(relative)
        .components()
        .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return None;
    }
    let candidate = root.join(relative);
    let real = fs::canonicalize(&candidate).ok()?;
    let real_root = fs::canonicalize(root).ok()?;
    if !real.starts_with(&real_root) || !real.is_file() {
        return None;
    }
    Some(real)
}

/// The path an identifier names under the compiled-in [`ROOTS`].
#[must_use]
pub fn resolve(id: &str) -> Option<PathBuf> {
    resolve_in(&ROOTS.iter().map(PathBuf::from).collect::<Vec<_>>(), id)
}

/// The bytes of a document, refusing anything too large to hand over.
#[must_use]
pub fn read_in(roots: &[PathBuf], id: &str) -> Option<Vec<u8>> {
    let path = resolve_in(roots, id)?;
    let bytes = fs::metadata(&path).ok()?.len();
    if bytes == 0 || bytes > MAX_DOCUMENT_BYTES {
        return None;
    }
    fs::read(path).ok()
}

#[cfg(test)]
mod tests {
    use super::{list_in, read_in, resolve_in, Kind, MAX_DEPTH};
    use std::fs;
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("cobalt-library-{name}"));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("the scratch root");
        root
    }

    #[test]
    fn a_listing_names_documents_and_ignores_everything_else() {
        let root = scratch("kinds");
        for name in [
            "Bleak House.epub",
            "notes.md",
            "page.html",
            "plain.txt",
            "scan.pdf",
            "cover.jpg",
            "KoboReader.sqlite",
        ] {
            fs::write(root.join(name), b"body").expect("the file");
        }
        let listing = list_in(&[root]);
        let found = listing
            .entries
            .iter()
            .map(|entry| (entry.title.as_str(), entry.kind))
            .collect::<Vec<_>>();
        assert_eq!(
            found,
            [
                ("Bleak House", Kind::Epub),
                ("notes", Kind::Markdown),
                ("page", Kind::Html),
                ("plain", Kind::Text),
                ("scan", Kind::Pdf),
            ]
        );
        assert!(!listing.truncated);
    }

    #[test]
    fn the_stock_readers_own_state_is_never_walked() {
        // .kobo holds the library database and the cover cache. Nothing in it
        // is a book somebody chose to put there, and all of it is private.
        let root = scratch("kobo-dir");
        fs::create_dir_all(root.join(".kobo")).expect("the directory");
        fs::write(root.join(".kobo/secret.epub"), b"body").expect("the file");
        fs::write(root.join("Real.epub"), b"body").expect("the file");
        let listing = list_in(&[root]);
        assert_eq!(listing.entries.len(), 1);
        assert_eq!(listing.entries[0].title, "Real");
    }

    #[test]
    fn an_identifier_cannot_be_edited_into_somewhere_else() {
        let root = scratch("escape");
        fs::write(root.join("Real.epub"), b"body").expect("the file");
        let outside = root.parent().expect("a parent").join("outside.epub");
        fs::write(&outside, b"body").expect("the file");
        let roots = vec![root];
        assert!(resolve_in(&roots, "0/Real.epub").is_some());
        for forged in [
            "0/../outside.epub",
            "0/./../outside.epub",
            "0//etc/passwd",
            "1/Real.epub",
            "Real.epub",
            "0/",
        ] {
            assert!(
                resolve_in(&roots, forged).is_none(),
                "{forged:?} resolved to a path"
            );
        }
        let _ = fs::remove_file(outside);
    }

    #[test]
    fn a_document_deeper_than_the_cap_stops_the_walk_and_says_so() {
        let root = scratch("depth");
        let mut deep = root.clone();
        for level in 0..=MAX_DEPTH + 1 {
            deep = deep.join(format!("level-{level}"));
        }
        fs::create_dir_all(&deep).expect("the directories");
        fs::write(deep.join("Deep.epub"), b"body").expect("the file");
        let listing = list_in(&[root]);
        assert!(listing.entries.is_empty());
        assert!(
            listing.truncated,
            "a walk that gave up must say the shelf is partial"
        );
    }

    #[test]
    fn reading_returns_the_bytes_and_refuses_an_unknown_identifier() {
        let root = scratch("read");
        fs::write(root.join("Real.txt"), b"the body").expect("the file");
        let roots = vec![root];
        assert_eq!(
            read_in(&roots, "0/Real.txt").as_deref(),
            Some(&b"the body"[..])
        );
        assert!(read_in(&roots, "0/Missing.txt").is_none());
    }

    #[test]
    fn pdfs_are_listed_and_declared_unreadable() {
        // Nothing on the device parses PDF. Hiding them would lose books the
        // owner can see over USB; claiming they open would be a lie.
        assert!(!Kind::Pdf.is_readable());
        assert_eq!(Kind::Pdf.badge(), "PDF");
        for kind in [Kind::Epub, Kind::Markdown, Kind::Html, Kind::Text] {
            assert!(kind.is_readable(), "{kind:?} should page in the reader");
        }
    }
}

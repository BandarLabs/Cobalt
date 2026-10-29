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
//! `/mnt/onboard/.kobo` is not walked. That directory holds the stock reader's
//! database, cover cache, and firmware chrome (the eLabel EPUBs). Sideloaded
//! files live at the card root and in ordinary folders; titles the stock
//! reader already knows — including Kepubs it downloaded, and Store books
//! that are in the library but not on the card — come from
//! `KoboReader.sqlite` rather than from walking that tree.
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
    ".adds",
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
    /// A Kobo Store book, listed and not openable yet.
    ///
    /// Kepub is EPUB with the stock reader's own spans threaded through the
    /// markup. Opening one as plain EPUB shows the spans, so it is listed with
    /// its real title from the database and refused on a tap, which is the
    /// same answer PDF already gives. These are the books most readers own
    /// most of, so hiding them would make the shelf look empty rather than
    /// honest.
    Kepub,
}

impl Kind {
    /// The short badge a tile draws in its corner.
    #[must_use]
    pub const fn badge(self) -> &'static str {
        match self {
            Self::Epub => "EPUB",
            Self::Kepub => "KEPUB",
            Self::Markdown => "MD",
            Self::Html => "HTML",
            Self::Text => "TXT",
            Self::Pdf => "PDF",
        }
    }

    /// Whether the built-in reader can page this.
    #[must_use]
    pub const fn is_readable(self) -> bool {
        !matches!(self, Self::Pdf | Self::Kepub)
    }

    /// The kind a file name implies, if it implies one.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        let lowered = name.to_ascii_lowercase();
        for (suffix, kind) in [
            (".kepub.epub", Self::Kepub),
            (".epub", Self::Epub),
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
    /// False when the stock library names a book whose file is not on the card.
    pub on_card: bool,
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

/// The documents under the compiled-in [`ROOTS`], plus titles from the stock
/// reader's library that are not already on the walk.
#[must_use]
pub fn list() -> Listing {
    let mut listing = list_in(&ROOTS.iter().map(PathBuf::from).collect::<Vec<_>>());
    nickel::merge(&mut listing);
    listing
}

/// The bytes of a document, refusing anything too large to hand over.
#[must_use]
pub fn read(id: &str) -> Option<Vec<u8>> {
    if let Some(bytes) = nickel::read(id) {
        return Some(bytes);
    }
    read_in(&ROOTS.iter().map(PathBuf::from).collect::<Vec<_>>(), id)
}

/// Where the stock reader keeps its database, relative to the card.
const STOCK_DATABASE: &str = ".kobo/KoboReader.sqlite";

/// The documents under `roots`, plus the stock library a copy of the card
/// holds at its first root.
///
/// The reader lists the stock reader's database from its card. A simulator's
/// library folder stands in for the card, so a database kept where the card
/// keeps one is read the same way, and the shelf of Kobo Store books can be
/// seen and tried without a reader.
#[must_use]
pub fn list_with_stock_in(roots: &[PathBuf]) -> Listing {
    let mut listing = list_in(roots);
    if let Some(card) = roots.first() {
        let database = card.join(STOCK_DATABASE);
        if database.is_file() {
            nickel::merge_from(&mut listing, &database, roots);
        }
    }
    listing
}

/// The bytes of a document [`list_with_stock_in`] listed.
///
/// A stock-library book is named by its path. On the reader that path comes
/// from the reader's own database; here the database is a copy anybody could
/// have written, so a book it names is read only from inside one of `roots`.
#[must_use]
pub fn read_with_stock_in(roots: &[PathBuf], id: &str) -> Option<Vec<u8>> {
    let Some(path) = id.strip_prefix("n/") else {
        return read_in(roots, id);
    };
    let real = Path::new(path).canonicalize().ok()?;
    let inside = roots
        .iter()
        .filter_map(|root| root.canonicalize().ok())
        .any(|root| real.starts_with(root));
    if inside {
        nickel::read(id)
    } else {
        None
    }
}

/// The wire form of one listing entry.
#[must_use]
pub fn to_wire(entry: Entry) -> kobo_protocol::LibraryEntry {
    kobo_protocol::LibraryEntry {
        id: entry.id,
        title: clip_title(&entry.title),
        kind: match entry.kind {
            Kind::Epub => 1,
            Kind::Markdown => 2,
            Kind::Html => 3,
            Kind::Text => 4,
            Kind::Pdf => 5,
            Kind::Kepub => 6,
        },
        bytes: u32::try_from(entry.bytes).unwrap_or(u32::MAX),
        on_card: entry.on_card,
    }
}

/// Truncates to the wire bound on a character boundary, never producing an
/// empty title: the encoder refuses those, and one overlong Japanese title
/// would otherwise drop the whole listing.
fn clip_title(title: &str) -> String {
    let mut out = String::new();
    for ch in title.chars() {
        let mut buf = [0; 4];
        let encoded = ch.encode_utf8(&mut buf);
        if out.len() + encoded.len() > kobo_protocol::MAX_LIBRARY_TITLE_LEN {
            break;
        }
        out.push(ch);
    }
    if out.is_empty() {
        "Untitled".to_owned()
    } else {
        out
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
        let named = Kind::from_name(name);
        let Some(document) = named.or_else(|| is_unnamed_epub(&path, name).then_some(Kind::Epub))
        else {
            continue;
        };
        let metadata = entry.metadata().ok();
        let bytes = metadata.as_ref().map_or(0, fs::Metadata::len);
        if bytes == 0 || bytes > MAX_DOCUMENT_BYTES {
            continue;
        }
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let Some(relative) = relative.to_str() else {
            continue;
        };
        let title = if named.is_some() {
            title_of(name)
        } else {
            let modified = metadata.and_then(|data| data.modified().ok());
            unnamed_title(&path, bytes, modified).unwrap_or_else(|| name.to_owned())
        };
        listing.entries.push(Entry {
            id: format!("{root_index}/{relative}"),
            title,
            kind: document,
            bytes,
            on_card: true,
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

/// Whether a file with no extension is an EPUB, by its first bytes.
///
/// Cobalt applications keep what they download as shelf blobs named
/// `book-1a2b3c4d`, with no suffix to list them by, so every book Gutenbird
/// downloaded was invisible here. An EPUB says what it is in its first member:
/// the format requires a `mimetype` file, stored uncompressed, holding
/// `application/epub+zip`. Only names without a dot are opened to check, so a
/// card of photos and archives is not read on the way past.
fn is_unnamed_epub(path: &Path, name: &str) -> bool {
    const SIGNATURE: &[u8] = b"application/epub+zip";
    if name.contains('.') {
        return false;
    }
    let mut head = [0_u8; 512];
    let Ok(read) = fs::File::open(path).and_then(|mut file| {
        use std::io::Read as _;
        file.read(&mut head)
    }) else {
        return false;
    };
    let head = &head[..read];
    let field = |at: usize| {
        head.get(at..at + 2)
            .map(|bytes| usize::from(u16::from_le_bytes([bytes[0], bytes[1]])))
    };
    let (Some(name_len), Some(extra_len)) = (field(26), field(28)) else {
        return false;
    };
    head.starts_with(b"PK\x03\x04")
        && head.get(30..30 + name_len) == Some(&b"mimetype"[..])
        && head
            .get(30 + name_len + extra_len..)
            .is_some_and(|content| content.starts_with(SIGNATURE))
}

/// The most an unnamed EPUB is read in full to find its title.
const MAX_TITLE_READ_BYTES: u64 = 16 * 1024 * 1024;

/// The title an unnamed EPUB gives itself, read once per version of the file.
///
/// A blob's name is a key, not a title, and a shelf of `book-1a2b3c4d` is not
/// a shelf anybody can choose from. The title is in the book's package, which
/// means reading the archive, so each answer is kept for as long as the
/// runtime runs and asked again only when the file's size or time changes.
fn unnamed_title(
    path: &Path,
    bytes: u64,
    modified: Option<std::time::SystemTime>,
) -> Option<String> {
    use std::collections::BTreeMap;
    use std::sync::{Mutex, PoisonError};
    type Key = (PathBuf, u64, Option<std::time::SystemTime>);
    static TITLES: Mutex<BTreeMap<Key, Option<String>>> = Mutex::new(BTreeMap::new());
    let key = (path.to_path_buf(), bytes, modified);
    if let Some(known) = TITLES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&key)
    {
        return known.clone();
    }
    let title = (bytes <= MAX_TITLE_READ_BYTES)
        .then(|| fs::read(path).ok())
        .flatten()
        .and_then(|book| kobo_doc::epub::names(&book).ok())
        .and_then(|description| description.title);
    let mut titles = TITLES.lock().unwrap_or_else(PoisonError::into_inner);
    if titles.len() >= MAX_ENTRIES {
        titles.clear();
    }
    titles.insert(key, title.clone());
    title
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

mod nickel {
    use super::{clip_title, Entry, Kind, Listing, MAX_DOCUMENT_BYTES, MAX_ENTRIES, ROOTS};
    use std::path::{Path, PathBuf};

    const DATABASE: &str = "/mnt/onboard/.kobo/KoboReader.sqlite";

    pub fn merge(listing: &mut Listing) {
        let roots: Vec<PathBuf> = ROOTS.iter().map(PathBuf::from).collect();
        merge_from(listing, Path::new(DATABASE), &roots);
    }

    /// Adds the stock library's titles to a walk of `roots`.
    pub fn merge_from(listing: &mut Listing, database: &Path, roots: &[PathBuf]) {
        let Ok(connection) = rusqlite::Connection::open_with_flags(
            database,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ) else {
            return;
        };
        let Ok(mut statement) = connection.prepare(
            "SELECT ContentID, Title, MimeType, IsDownloaded, ___FileSize
             FROM content
             WHERE ContentType = '6' AND Title IS NOT NULL AND Title != ''",
        ) else {
            return;
        };
        let Ok(rows) = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                downloaded(row)?,
                row.get::<_, i64>(4).unwrap_or(0),
            ))
        }) else {
            return;
        };
        for row in rows.flatten() {
            if listing.entries.len() >= MAX_ENTRIES {
                listing.truncated = true;
                break;
            }
            let (content_id, title, mime, downloaded, size) = row;
            if mime.to_ascii_lowercase().contains("mp3") {
                continue;
            }
            let named = content_id.strip_prefix("file://").map(Path::new);
            if named.is_some_and(skipped_internal) {
                continue;
            }
            let path = named.filter(|path| path.is_file());
            if let Some(listed) = path.and_then(|path| already_listed(listing, path, roots)) {
                // The walk named it after its file. The stock reader read the
                // book's own title when it was copied on, and a sideloaded
                // `austen_pride_v2` is "Pride and Prejudice" there.
                listing.entries[listed].title = clip_title(&title);
                continue;
            }
            let on_card = downloaded && path.is_some();
            let Some(kind) = kind_of(&mime, path) else {
                continue;
            };
            let id = if let Some(path) = path.and_then(Path::to_str) {
                format!("n/{path}")
            } else {
                format!("n/{content_id}")
            };
            if id.len() > kobo_protocol::MAX_LIBRARY_ID_LEN || !kobo_protocol_id_ok(&id) {
                continue;
            }
            listing.entries.push(Entry {
                id,
                title: clip_title(&title),
                kind,
                bytes: u64::try_from(size.max(0)).unwrap_or(0),
                on_card,
            });
        }
        listing.entries.sort_by(|left, right| {
            left.title
                .to_ascii_lowercase()
                .cmp(&right.title.to_ascii_lowercase())
                .then_with(|| left.id.cmp(&right.id))
        });
    }

    pub fn read(id: &str) -> Option<Vec<u8>> {
        let path = id.strip_prefix("n/")?;
        if path.contains("..") {
            return None;
        }
        let path = Path::new(path);
        if skipped_internal(path) || !path.is_file() {
            return None;
        }
        let bytes = std::fs::metadata(path).ok()?.len();
        if bytes == 0 || bytes > MAX_DOCUMENT_BYTES {
            return None;
        }
        std::fs::read(path).ok()
    }

    fn downloaded(row: &rusqlite::Row<'_>) -> rusqlite::Result<bool> {
        match row.get_ref(3)? {
            rusqlite::types::ValueRef::Text(text) => {
                let text = std::str::from_utf8(text).unwrap_or("");
                Ok(text.eq_ignore_ascii_case("true") || text == "1")
            }
            rusqlite::types::ValueRef::Integer(value) => Ok(value != 0),
            _ => Ok(false),
        }
    }

    /// Where the walk already listed this file, if it did.
    fn already_listed(listing: &Listing, path: &Path, roots: &[PathBuf]) -> Option<usize> {
        let real = path.canonicalize().ok()?;
        roots.iter().enumerate().find_map(|(index, root)| {
            let root = root.canonicalize().ok()?;
            let relative = real.strip_prefix(&root).ok()?.to_str()?;
            let id = format!("{index}/{relative}");
            listing.entries.iter().position(|entry| entry.id == id)
        })
    }

    fn kobo_protocol_id_ok(id: &str) -> bool {
        !id.is_empty()
            && !id.starts_with('/')
            && !id.chars().any(char::is_control)
            && !id.split('/').any(|part| part == "..")
    }

    /// Firmware chrome, cover cache, and Cobalt's own files are not books.
    /// Kepubs the stock reader downloaded under `.kobo/kepub` are.
    pub(super) fn skipped_internal(path: &Path) -> bool {
        let parts: Vec<&str> = path
            .components()
            .filter_map(|part| part.as_os_str().to_str())
            .collect();
        if parts
            .iter()
            .any(|part| matches!(*part, ".kobo-images" | ".adds" | ".adobe-digital-editions"))
        {
            return true;
        }
        match parts.iter().position(|part| *part == ".kobo") {
            Some(index) => parts.get(index + 1).copied() != Some("kepub"),
            None => false,
        }
    }

    fn kind_of(mime: &str, path: Option<&Path>) -> Option<Kind> {
        if let Some(name) = path.and_then(|path| path.file_name()?.to_str()) {
            if let Some(kind) = Kind::from_name(name) {
                return Some(kind);
            }
        }
        let mime = mime.to_ascii_lowercase();
        if mime.contains("kobo-epub") {
            Some(Kind::Kepub)
        } else if mime.contains("epub") || mime.contains("octet-stream") {
            Some(Kind::Epub)
        } else if mime.contains("pdf") {
            Some(Kind::Pdf)
        } else if mime.contains("html") {
            Some(Kind::Html)
        } else if mime.contains("markdown") {
            Some(Kind::Markdown)
        } else if mime.contains("text/plain") {
            Some(Kind::Text)
        } else {
            None
        }
    }
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

    // `.kepub.epub` also ends with `.epub`, so asking about `.epub` first made
    // the Kepub arm unreachable and every Kobo Store book look like an EPUB
    // the reader could open. It cannot: the stock reader threads its own spans
    // through the markup.
    #[test]
    fn a_kepub_is_not_mistaken_for_an_epub_it_cannot_open() {
        assert_eq!(Kind::from_name("Bleak House.kepub.epub"), Some(Kind::Kepub));
        assert_eq!(Kind::from_name("Bleak House.epub"), Some(Kind::Epub));
        assert_eq!(Kind::from_name("BLEAK HOUSE.KEPUB.EPUB"), Some(Kind::Kepub));

        assert!(!Kind::Kepub.is_readable(), "a Kepub is listed, not opened");
        assert!(Kind::Epub.is_readable());
        assert_eq!(Kind::Kepub.badge(), "KEPUB");
    }

    /// The wire is the seam this crate answers across, and a listing is
    /// encoded as one value. An entry the encoder refuses is not a missing
    /// tile: it fails the whole reply, and the host ends the session rather
    /// than deliver a half-written one. So every kind this module can produce
    /// is checked against the encoder, not just the one the shelf shows most.
    #[test]
    fn every_kind_a_listing_can_produce_survives_the_wire() {
        let root = scratch("wire");
        for name in [
            "Bleak House.epub",
            "Piranesi.kepub.epub",
            "notes.md",
            "page.html",
            "plain.txt",
            "scan.pdf",
        ] {
            fs::write(root.join(name), b"body").expect("the file");
        }
        let entries: Vec<_> = list_in(&[root])
            .entries
            .into_iter()
            .map(super::to_wire)
            .collect();
        assert_eq!(entries.len(), 6, "the walk did not find every kind");
        let frame = kobo_protocol::Frame {
            version: kobo_protocol::VERSION,
            request_id: 1,
            message: kobo_protocol::Message::DeviceResult(kobo_protocol::DeviceResult::Library {
                entries,
                truncated: false,
            }),
        };
        assert!(
            kobo_protocol::encode(&frame).is_ok(),
            "a kind this module lists cannot be sent, which ends the session showing the shelf"
        );
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

    #[test]
    fn cobalt_program_files_are_never_walked() {
        let root = scratch("adds-dir");
        fs::create_dir_all(root.join(".adds/cobalt")).expect("the directory");
        fs::write(root.join(".adds/cobalt/README.txt"), b"body").expect("the file");
        fs::write(root.join("Letter.txt"), b"body").expect("the file");
        let listing = list_in(&[root]);
        assert_eq!(listing.entries.len(), 1);
        assert_eq!(listing.entries[0].title, "Letter");
    }

    #[test]
    fn firmware_chrome_is_not_a_book_and_a_kepub_download_is() {
        use std::path::Path;
        assert!(super::nickel::skipped_internal(Path::new(
            "/mnt/onboard/.kobo/eLabel/sleep.epub"
        )));
        assert!(super::nickel::skipped_internal(Path::new(
            "/mnt/onboard/.adds/cobalt/README.txt"
        )));
        assert!(!super::nickel::skipped_internal(Path::new(
            "/mnt/onboard/.kobo/kepub/Piranesi.kepub.epub"
        )));
        assert!(!super::nickel::skipped_internal(Path::new(
            "/mnt/onboard/sideloaded.epub"
        )));
    }

    fn a_book_called(title: &str) -> Vec<u8> {
        kobo_doc::epub::write(
            title,
            Some("A. Writer"),
            &[kobo_doc::epub::Chapter {
                title: "One".to_owned(),
                body: "Words.".to_owned(),
            }],
        )
        .expect("a written book")
    }

    #[test]
    fn a_book_an_application_kept_without_an_extension_is_listed_by_its_own_title() {
        let root = scratch("unnamed");
        let shelf = root.join("gutenbird");
        fs::create_dir_all(&shelf).expect("an application's shelf");
        fs::write(
            shelf.join("book-1a2b3c4d"),
            a_book_called("The Time Machine"),
        )
        .expect("a blob");
        // A blob that is not a book, and one that only says it is.
        fs::write(shelf.join("place-1a2b3c4d"), b"position 12").expect("a record");
        fs::write(shelf.join("feed-cache"), b"PK\x03\x04 not an epub").expect("a zip");

        let listing = list_in(std::slice::from_ref(&root));
        let found: Vec<_> = listing
            .entries
            .iter()
            .map(|entry| (entry.id.as_str(), entry.title.as_str(), entry.kind))
            .collect();
        assert_eq!(
            found,
            [("0/gutenbird/book-1a2b3c4d", "The Time Machine", Kind::Epub)]
        );
        assert!(read_in(&[root], "0/gutenbird/book-1a2b3c4d").is_some());
    }

    #[test]
    fn a_sideloaded_book_takes_the_title_the_stock_reader_read_from_it() {
        let root = scratch("titled");
        let file = root.join("austen_pride_v2.epub");
        fs::write(&file, a_book_called("Pride and Prejudice")).expect("a sideloaded book");
        let database = scratch("titled-db").join("KoboReader.sqlite");
        let connection = rusqlite::Connection::open(&database).expect("a scratch library");
        connection
            .execute_batch(&format!(
                "CREATE TABLE content (ContentID TEXT, Title TEXT, MimeType TEXT,
                    IsDownloaded TEXT, ___FileSize INTEGER, ContentType TEXT);
                 INSERT INTO content VALUES ('file://{}', 'Pride and Prejudice',
                    'application/epub+zip', 'true', 12, '6');",
                file.canonicalize().expect("a real path").display()
            ))
            .expect("the row");
        drop(connection);
        let roots = [root];
        let mut listing = list_in(&roots);
        super::nickel::merge_from(&mut listing, &database, &roots);
        let titles: Vec<_> = listing
            .entries
            .iter()
            .map(|entry| entry.title.as_str())
            .collect();
        assert_eq!(titles, ["Pride and Prejudice"], "listed once, by its title");
    }

    #[test]
    fn a_copy_of_the_card_lists_and_reads_its_stock_library_but_only_inside_the_roots() {
        let card = scratch("stock-card");
        let kepubs = card.join(".kobo").join("kepub");
        fs::create_dir_all(&kepubs).expect("the stock reader's folders");
        let kepub = kepubs.join("1f7e9c2a");
        fs::write(&kepub, a_book_called("Piranesi")).expect("a store book");
        let elsewhere = scratch("stock-elsewhere").join("secret.epub");
        fs::write(&elsewhere, a_book_called("Not a card book")).expect("a file off the card");
        let database = card.join(".kobo").join("KoboReader.sqlite");
        let connection = rusqlite::Connection::open(&database).expect("a scratch library");
        connection
            .execute_batch(&format!(
                "CREATE TABLE content (ContentID TEXT, Title TEXT, MimeType TEXT,
                    IsDownloaded TEXT, ___FileSize INTEGER, ContentType TEXT);
                 INSERT INTO content VALUES
                    ('file://{}', 'Piranesi', 'application/x-kobo-epub+zip', 'true', 9, '6'),
                    ('file://{}', 'Outside', 'application/epub+zip', 'true', 9, '6'),
                    ('7a866127', 'Cloud Atlas', 'application/x-kobo-epub+zip', 'false', 0, '6');",
                kepub.canonicalize().expect("a real path").display(),
                elsewhere.canonicalize().expect("a real path").display(),
            ))
            .expect("the rows");
        drop(connection);
        let roots = [card];
        let listing = super::list_with_stock_in(&roots);
        let found: Vec<_> = listing
            .entries
            .iter()
            .map(|entry| (entry.title.as_str(), entry.kind, entry.on_card))
            .collect();
        assert_eq!(
            found,
            [
                ("Cloud Atlas", Kind::Kepub, false),
                ("Outside", Kind::Epub, true),
                ("Piranesi", Kind::Kepub, true),
            ]
        );
        let id = |title: &str| {
            listing
                .entries
                .iter()
                .find(|entry| entry.title == title)
                .map(|entry| entry.id.clone())
                .expect("listed")
        };
        assert!(super::read_with_stock_in(&roots, &id("Piranesi")).is_some());
        assert!(
            super::read_with_stock_in(&roots, &id("Outside")).is_none(),
            "a copied database named a file outside the card and it was read"
        );
    }

    #[test]
    fn the_stock_library_names_books_the_walk_cannot_see() {
        let database = scratch("nickel-db").join("KoboReader.sqlite");
        let connection = rusqlite::Connection::open(&database).expect("a scratch library");
        connection
            .execute_batch(
                "CREATE TABLE content (
                    ContentID TEXT PRIMARY KEY,
                    Title TEXT,
                    MimeType TEXT,
                    IsDownloaded TEXT,
                    ___FileSize INTEGER,
                    ContentType TEXT
                );
                INSERT INTO content VALUES
                ('7a866127-8d49-4541-ac08-4a279e0f07cd', 'Piranesi',
                 'application/x-kobo-epub+zip', 'false', 0, '6'),
                ('file:///mnt/onboard/.kobo/eLabel/sleep.epub', 'Sleep',
                 'application/epub+zip', 'true', 12, '6'),
                ('file:///mnt/onboard/.adds/cobalt/README.txt', 'README',
                 'text/plain', 'true', 12, '6'),
                ('file:///mnt/onboard/Audiobooks/comet.mp3z', 'comet',
                 'application/x-kobo-mp3z', 'true', 12, '6');",
            )
            .expect("the rows");
        drop(connection);
        let mut listing = super::Listing::default();
        super::nickel::merge_from(&mut listing, &database, &[]);
        let titles: Vec<_> = listing
            .entries
            .iter()
            .map(|entry| (entry.title.as_str(), entry.on_card))
            .collect();
        assert_eq!(titles, [("Piranesi", false)]);
    }
}

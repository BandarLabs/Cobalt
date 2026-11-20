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
///
/// [`SYNCED_BOOKS`] is the folder of books the owner shares from a computer.
/// It is a root of its own because it sits under `.adds`, which the walk of
/// the card never enters: the folder belongs to the sync engine, which keeps
/// its bookkeeping beside the books and would lose track of a file moved out
/// from under it, so the books stay where they land and the library comes to
/// them. Without it a book shared from a computer arrived on the reader and
/// appeared nowhere on it.
///
/// It is last, and a root added after it goes after it. An identifier starts
/// with its root's place in this list, and Books keeps each reading position
/// and cover under the identifier, so moving a root forgets the place in
/// every book under the roots that follow it.
pub const ROOTS: &[&str] = &[
    "/mnt/onboard",
    "/mnt/onboard/.adds/cobalt/data",
    "/mnt/sd",
    SYNCED_BOOKS,
];

/// Where the Sync app's `books` folder lands on the card.
///
/// Receive-only on the reader: what is in it is what the computer shared,
/// plus the engine's own marker and whatever it is part-way through writing,
/// all of it hidden, and a hidden file is never listed.
pub const SYNCED_BOOKS: &str = "/mnt/onboard/.adds/cobalt/sync/books";

/// Directory names never descended into, matched at any depth, besides every
/// hidden directory.
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
///
/// Also the most the wire carries in one listing,
/// [`kobo_protocol::MAX_LIBRARY_ENTRIES`], and the build holds this to that:
/// a listing longer than the wire allows is not sent short, it fails to
/// encode, and the session that asked for it ends.
pub const MAX_ENTRIES: usize = 2_000;
const _: () = assert!(MAX_ENTRIES <= kobo_protocol::MAX_LIBRARY_ENTRIES);

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
    whole(&locate(id)?)
}

/// One range of a document under the compiled-in [`ROOTS`] or in the stock
/// reader's library. See [`Range`].
///
/// # Errors
///
/// [`RangeFault::NotADocument`] for an identifier that names nothing a
/// listing could have, and [`RangeFault::Unreadable`] when the file is there
/// and the card would not give up the bytes.
pub fn read_range(id: &str, offset: u32, length: u32) -> Result<Range, RangeFault> {
    range_of(&locate(id).ok_or(RangeFault::NotADocument)?, offset, length)
}

/// The file an identifier names, on the reader: one the stock library handed
/// out, or one under the roots.
fn locate(id: &str) -> Option<PathBuf> {
    nickel::locate(id).or_else(|| resolve(id))
}

/// A whole document, unless it is empty or too large to hand over.
fn whole(path: &Path) -> Option<Vec<u8>> {
    let bytes = fs::metadata(path).ok()?.len();
    if bytes == 0 || bytes > MAX_DOCUMENT_BYTES {
        return None;
    }
    fs::read(path).ok()
}

/// Part of a document, and what the file was at the moment it was read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Range {
    /// The length of the whole file.
    pub size: u32,
    /// The file's modification time as a number, there to be compared and
    /// nothing else: it differs once the file has been written again, which
    /// is how whoever is putting the ranges together learns that two of them
    /// are not pieces of the same book. Zero when the card does not say.
    pub stamp: u64,
    pub bytes: Vec<u8>,
}

/// Why a range was not read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RangeFault {
    /// The identifier names nothing a listing could have named: no such
    /// file, one outside the roots, an empty one, or one past
    /// [`MAX_DOCUMENT_BYTES`].
    NotADocument,
    /// The file is there and could not be read, or changed length while it
    /// was being read.
    Unreadable,
}

/// Reads at most `length` bytes of `path` from `offset`, by seeking.
///
/// The file is opened for this one range and closed again, so nothing is held
/// between two requests and a book being replaced is never kept alive by a
/// reader half-way through the old one. The length and the bytes both come
/// from the open file rather than from its name, so they describe the same
/// file even when the name is given to another one in between.
fn range_of(path: &Path, offset: u32, length: u32) -> Result<Range, RangeFault> {
    use std::io::{Read as _, Seek as _, SeekFrom};
    let mut file = fs::File::open(path).map_err(|_| RangeFault::NotADocument)?;
    let metadata = file.metadata().map_err(|_| RangeFault::Unreadable)?;
    let size = match u32::try_from(metadata.len()) {
        Ok(size) if size > 0 && u64::from(size) <= MAX_DOCUMENT_BYTES => size,
        _ => return Err(RangeFault::NotADocument),
    };
    let stamp = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |since| {
            u64::try_from(since.as_nanos()).unwrap_or(u64::MAX)
        });
    // Clamped here as well as on the wire, because the answer is encoded as
    // one value and a piece the encoder refuses ends the session that asked.
    let wanted = usize::try_from(length.min(size.saturating_sub(offset)))
        .unwrap_or(usize::MAX)
        .min(kobo_protocol::MAX_LIBRARY_RANGE_BYTES);
    let mut bytes = Vec::with_capacity(wanted);
    if wanted > 0 {
        file.seek(SeekFrom::Start(u64::from(offset)))
            .map_err(|_| RangeFault::Unreadable)?;
        file.take(u64::try_from(wanted).unwrap_or(u64::MAX))
            .read_to_end(&mut bytes)
            .map_err(|_| RangeFault::Unreadable)?;
    }
    // Fewer than the file's own length promised: it was cut short between
    // being measured and being read, and a piece that stops early would be
    // taken for the end of the book.
    if bytes.len() != wanted {
        return Err(RangeFault::Unreadable);
    }
    Ok(Range { size, stamp, bytes })
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
            nickel::merge_from(&mut listing, &database, roots, Some(card));
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
    whole(&locate_with_stock_in(roots, id)?)
}

/// One range of a document [`list_with_stock_in`] listed, under the same
/// rule as [`read_with_stock_in`]. See [`Range`].
///
/// # Errors
///
/// As [`read_range`].
pub fn read_range_with_stock_in(
    roots: &[PathBuf],
    id: &str,
    offset: u32,
    length: u32,
) -> Result<Range, RangeFault> {
    let path = locate_with_stock_in(roots, id).ok_or(RangeFault::NotADocument)?;
    range_of(&path, offset, length)
}

fn locate_with_stock_in(roots: &[PathBuf], id: &str) -> Option<PathBuf> {
    let Some(path) = id.strip_prefix("n/") else {
        return resolve_in(roots, id);
    };
    let real = Path::new(path).canonicalize().ok()?;
    let inside = roots
        .iter()
        .filter_map(|root| root.canonicalize().ok())
        .any(|root| real.starts_with(root));
    if inside {
        nickel::locate(id)
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
pub(crate) fn clip_title(title: &str) -> String {
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
            if skipped_directory(name) {
                continue;
            }
            walk(root, &path, root_index, depth + 1, listing, seen);
            if listing.truncated {
                return;
            }
            continue;
        }
        if !kind.is_file() || hidden(name) {
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
        let Some(id) = wire_path(root, &path)
            .map(|relative| format!("{root_index}/{relative}"))
            .filter(|id| carried(id))
        else {
            continue;
        };
        let title = if named.is_some() {
            title_of(name)
        } else {
            let modified = metadata.and_then(|data| data.modified().ok());
            unnamed_title(&path, bytes, modified).unwrap_or_else(|| name.to_owned())
        };
        listing.entries.push(Entry {
            id,
            title,
            kind: document,
            bytes,
            on_card: true,
        });
    }
}

/// Whether the wire carries this identifier: no longer than it allows, and
/// with no control character in it.
///
/// A listing is encoded as one value, so an identifier the encoder refuses
/// does not cost one tile: the whole reply fails and the session showing the
/// shelf ends. A book in a deep folder of long names, which is how a library
/// in a script of three bytes to the letter is filed, is therefore left off
/// the listing, and never shortened to fit, because a shortened identifier
/// names no file.
fn carried(id: &str) -> bool {
    id.len() <= kobo_protocol::MAX_LIBRARY_ID_LEN && !id.chars().any(char::is_control)
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
    if !real.is_file() {
        return None;
    }
    // Only what a listing could have named. A root is a folder of books, but
    // it is not only books: Cobalt's data root holds every application's own
    // files, and a simulator's holds its secrets. Reading anything under the
    // root handed any of those to any application allowed to read a book.
    let mut parts = real
        .strip_prefix(&real_root)
        .ok()?
        .components()
        .map(|part| part.as_os_str().to_str())
        .collect::<Option<Vec<_>>>()?;
    let name = parts.pop()?;
    if hidden(name) || parts.iter().any(|part| skipped_directory(part)) {
        return None;
    }
    (Kind::from_name(name).is_some() || is_unnamed_epub(&real, name)).then_some(real)
}

/// The folder on the card a computer delivers clipped articles into.
///
/// Visible, at the top of the card, because that is where the stock reader
/// and Books both find books, and where an owner looking over USB expects
/// them.
pub const ARTICLES_FOLDER: &str = "Articles";

/// Why an article could not be deleted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Removal {
    /// The identifier names nothing deletable: no such file, or a file
    /// outside the articles folder.
    NotAnArticle,
    /// The file is there and the card would not let it go.
    Refused,
}

/// Deletes one delivered article, the only write the library allows.
///
/// Only an EPUB directly inside [`ARTICLES_FOLDER`] on the first root, the
/// card, and never through a link: the folder holds only what a computer
/// delivered, so nothing the owner copied on can be reached by a mistake in
/// Books or by an identifier somebody edited.
///
/// # Errors
///
/// [`Removal::NotAnArticle`] for anything else, [`Removal::Refused`] when
/// the file system will not remove it.
pub fn remove_article(roots: &[PathBuf], id: &str) -> Result<(), Removal> {
    let card = roots.first().ok_or(Removal::NotAnArticle)?;
    let name = id
        .strip_prefix("0/")
        .and_then(|relative| relative.strip_prefix(ARTICLES_FOLDER))
        .and_then(|rest| rest.strip_prefix('/'))
        .ok_or(Removal::NotAnArticle)?;
    if name.is_empty()
        || name.contains('/')
        || name.starts_with('.')
        || Kind::from_name(name) != Some(Kind::Epub)
    {
        return Err(Removal::NotAnArticle);
    }
    let folder = card.join(ARTICLES_FOLDER);
    let path = folder.join(name);
    let metadata = fs::symlink_metadata(&path).map_err(|_| Removal::NotAnArticle)?;
    let folder_metadata = fs::symlink_metadata(&folder).map_err(|_| Removal::NotAnArticle)?;
    if !metadata.file_type().is_file() || !folder_metadata.file_type().is_dir() {
        return Err(Removal::NotAnArticle);
    }
    fs::remove_file(&path).map_err(|_| Removal::Refused)
}

/// A path under a root as an identifier carries it: its parts joined by `/`
/// whatever the host's separator, so a listing made on Windows names the
/// same book as one made on the reader, and nothing for a name that is not
/// UTF-8.
pub(crate) fn wire_path(root: &Path, path: &Path) -> Option<String> {
    let parts = path
        .strip_prefix(root)
        .ok()?
        .components()
        .map(|part| part.as_os_str().to_str())
        .collect::<Option<Vec<_>>>()?;
    Some(parts.join("/"))
}

/// Whether the walk leaves a directory alone: every hidden one, and the
/// visible names in [`SKIPPED`].
pub(crate) fn skipped_directory(name: &str) -> bool {
    name.starts_with('.') || SKIPPED.contains(&name)
}

/// Whether a file is hidden, which nothing an owner put on the shelf is.
///
/// The sync engine writes an arriving book under a hidden temporary name and
/// gives it its real one only when every byte has landed. That name ends in
/// `.tmp` today, which no kind is listed by, but a book half arrived is a
/// file of the wrong length that would open as a damaged one, so it is kept
/// off the shelf by being hidden rather than by how its name happens to end.
/// The engine's ignore list is hidden the same way. So is the sidecar a Mac
/// leaves beside every file it copies, `._Bleak House.epub`, which was listed
/// as a second Bleak House that could not be opened.
fn hidden(name: &str) -> bool {
    name.starts_with('.')
}

/// The path an identifier names under the compiled-in [`ROOTS`].
#[must_use]
pub fn resolve(id: &str) -> Option<PathBuf> {
    resolve_in(&ROOTS.iter().map(PathBuf::from).collect::<Vec<_>>(), id)
}

/// The bytes of a document, refusing anything too large to hand over.
#[must_use]
pub fn read_in(roots: &[PathBuf], id: &str) -> Option<Vec<u8>> {
    whole(&resolve_in(roots, id)?)
}

mod nickel {
    use super::{clip_title, Entry, Kind, Listing, MAX_ENTRIES, ROOTS};
    use std::path::{Path, PathBuf};

    const DATABASE: &str = "/mnt/onboard/.kobo/KoboReader.sqlite";

    /// Where the reader mounts its card, and so where the database's paths
    /// begin.
    const CARD: &str = "/mnt/onboard/";

    pub fn merge(listing: &mut Listing) {
        let roots: Vec<PathBuf> = ROOTS.iter().map(PathBuf::from).collect();
        merge_from(listing, Path::new(DATABASE), &roots, None);
    }

    /// Adds the stock library's titles to a walk of `roots`.
    ///
    /// `card` is where a copy of the card is kept, when the database came off
    /// a reader: it names its files where the reader mounts the card, and in
    /// the copy they are under `card` instead. Without that every sideloaded
    /// book was listed twice, once by the walk and once as a Store title that
    /// was not downloaded.
    pub fn merge_from(
        listing: &mut Listing,
        database: &Path,
        roots: &[PathBuf],
        card: Option<&Path>,
    ) {
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
            let named = content_id.strip_prefix("file://").map(|path| {
                match (card, path.strip_prefix(CARD)) {
                    (Some(card), Some(inside)) => card.join(inside),
                    _ => PathBuf::from(path),
                }
            });
            if named.as_deref().is_some_and(skipped_internal) {
                continue;
            }
            let path = named.as_deref().filter(|path| path.is_file());
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
            // A sideloaded file the stock reader still remembers but that is
            // gone from the card, such as an article deleted in Books, is not
            // a title waiting to be downloaded: nothing could download it.
            if named.is_some() && path.is_none() && kind != Kind::Kepub {
                continue;
            }
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

    /// The file an identifier from the stock library names, if it is one
    /// this module would have listed.
    pub fn locate(id: &str) -> Option<PathBuf> {
        let path = id.strip_prefix("n/")?;
        if path.contains("..") {
            return None;
        }
        let path = Path::new(path);
        (!skipped_internal(path) && path.is_file()).then(|| path.to_path_buf())
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

    /// Bytes that say where they are, so a piece put back in the wrong place
    /// or read from the wrong offset is a different book.
    fn a_file_of(length: usize) -> Vec<u8> {
        (0..length)
            .map(|at| u8::try_from((at * 31 + at / 251) % 251).expect("under 251"))
            .collect()
    }

    const PIECE: u32 = 256 * 1024;

    #[test]
    fn a_document_read_in_ranges_is_the_file_and_no_range_is_more_than_a_piece() {
        assert_eq!(
            usize::try_from(PIECE).ok(),
            Some(kobo_protocol::MAX_LIBRARY_RANGE_BYTES)
        );
        let root = scratch("ranges");
        let book = a_file_of(2 * 262_144 + 1_000);
        fs::write(root.join("Atlas.epub"), &book).expect("the file");
        let roots = [root];
        let size = u32::try_from(book.len()).expect("a small file");

        let mut held = Vec::new();
        let mut stamps = Vec::new();
        let mut lengths = Vec::new();
        loop {
            let offset = u32::try_from(held.len()).expect("a small file");
            // Asked for far more than a piece every time, as an application
            // that ignored the bound would.
            let range = super::read_range_with_stock_in(&roots, "0/Atlas.epub", offset, u32::MAX)
                .expect("a range");
            assert_eq!(range.size, size);
            stamps.push(range.stamp);
            lengths.push(range.bytes.len());
            if range.bytes.is_empty() {
                break;
            }
            held.extend_from_slice(&range.bytes);
        }
        assert_eq!(held, book, "the ranges put together are not the file");
        assert_eq!(lengths, [262_144, 262_144, 1_000, 0]);
        assert!(
            stamps[0] != 0,
            "this file system keeps no modification time"
        );
        assert!(stamps.iter().all(|stamp| *stamp == stamps[0]));

        // A range from the middle is the middle, and one that reaches past
        // the end stops at the end rather than failing.
        let middle =
            super::read_range_with_stock_in(&roots, "0/Atlas.epub", 300_000, 10).expect("a range");
        assert_eq!(middle.bytes, book[300_000..300_010]);
        let tail = super::read_range_with_stock_in(&roots, "0/Atlas.epub", size - 4, 100)
            .expect("a range");
        assert_eq!(tail.bytes, book[book.len() - 4..]);
        let past = super::read_range_with_stock_in(&roots, "0/Atlas.epub", u32::MAX, PIECE)
            .expect("a range");
        assert_eq!((past.size, past.bytes.len()), (size, 0));
    }

    #[test]
    fn a_range_is_refused_for_everything_a_whole_read_is_refused_for() {
        let root = scratch("range-escape");
        let shelf = root.join("readlater");
        fs::create_dir_all(&shelf).expect("an application's folder");
        fs::create_dir_all(root.join(".secrets")).expect("a hidden folder");
        fs::write(root.join("Real.epub"), b"body").expect("the file");
        fs::write(shelf.join("token"), b"hunter2").expect("an app's own blob");
        fs::write(shelf.join("articles.json"), b"{}").expect("an app's own file");
        fs::write(root.join(".secrets/lichess.epub"), b"a secret").expect("a secret");
        fs::write(root.join(".Hidden.epub"), b"hidden").expect("a hidden file");
        fs::write(root.join("Empty.epub"), b"").expect("an empty file");
        let outside = root.parent().expect("a parent").join("range-outside.epub");
        fs::write(&outside, b"body").expect("the file");
        let roots = [root];

        assert!(super::read_range_with_stock_in(&roots, "0/Real.epub", 0, PIECE).is_ok());
        for forged in [
            "0/../range-outside.epub",
            "0/./../range-outside.epub",
            "0//etc/passwd",
            "1/Real.epub",
            "Real.epub",
            "0/",
            "0/Missing.epub",
            "0/readlater/token",
            "0/readlater/articles.json",
            "0/.secrets/lichess.epub",
            "0/.Hidden.epub",
            "0/Empty.epub",
            "n//etc/passwd",
        ] {
            for offset in [0, 2] {
                assert_eq!(
                    super::read_range_with_stock_in(&roots, forged, offset, PIECE),
                    Err(super::RangeFault::NotADocument),
                    "{forged:?} from {offset}"
                );
            }
        }
        let _ = fs::remove_file(outside);
    }

    #[cfg(unix)]
    #[test]
    fn a_range_does_not_follow_a_link_out_of_its_root() {
        let root = scratch("range-link");
        let outside = scratch("range-link-outside").join("Private.epub");
        fs::write(&outside, b"not on the card").expect("a file off the card");
        std::os::unix::fs::symlink(&outside, root.join("Linked.epub")).expect("a link");
        assert_eq!(
            super::read_range_with_stock_in(&[root], "0/Linked.epub", 0, PIECE),
            Err(super::RangeFault::NotADocument)
        );
    }

    #[test]
    fn a_file_that_shrank_or_grew_between_two_ranges_says_so_in_every_range_after() {
        let root = scratch("range-changed");
        let path = root.join("Atlas.epub");
        let first_copy = a_file_of(400_000);
        fs::write(&path, &first_copy).expect("the file");
        let written = |seconds: u64| {
            let file = fs::File::options()
                .write(true)
                .open(&path)
                .expect("the file");
            let when = std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds);
            file.set_modified(when).expect("a modification time");
        };
        written(1_700_000_000);
        let roots = [root.clone()];
        let read = |offset| super::read_range_with_stock_in(&roots, "0/Atlas.epub", offset, PIECE);

        let first = read(0).expect("the first range");
        assert_eq!((first.size, first.bytes.len()), (400_000, 262_144));

        // Replaced by a shorter copy before the second range is asked for:
        // the next offset is past its end.
        fs::write(&path, a_file_of(100_000)).expect("a shorter copy");
        written(1_700_000_100);
        let shrunk = read(262_144).expect("an answer");
        assert_eq!((shrunk.size, shrunk.bytes.len()), (100_000, 0));
        assert_ne!(shrunk.stamp, first.stamp);

        // Replaced by a longer one: there are bytes at the offset, and they
        // are not the book the first range came from.
        fs::write(&path, a_file_of(900_000)).expect("a longer copy");
        written(1_700_000_200);
        let grown = read(262_144).expect("an answer");
        assert_eq!((grown.size, grown.bytes.len()), (900_000, 262_144));
        assert_ne!(grown.stamp, first.stamp);

        // Replaced by another book of exactly the first one's length, which
        // only the stamp can tell from it.
        let mut other = first_copy;
        other.reverse();
        fs::write(&path, &other).expect("another book");
        written(1_700_000_300);
        let other_book = read(262_144).expect("an answer");
        assert_eq!(other_book.size, first.size);
        assert_ne!(other_book.stamp, first.stamp);

        // Gone altogether.
        fs::remove_file(&path).expect("removed");
        assert_eq!(read(262_144), Err(super::RangeFault::NotADocument));
    }

    #[test]
    fn a_file_past_the_largest_document_is_not_read_in_ranges_either() {
        let root = scratch("range-huge");
        let path = root.join("Scans.epub");
        let file = fs::File::create(&path).expect("the file");
        // A length without the bytes: no card is filled to find this out.
        file.set_len(super::MAX_DOCUMENT_BYTES + 1)
            .expect("a long empty file");
        drop(file);
        let roots = [root];
        assert!(
            list_in(&roots).entries.is_empty(),
            "a file too large to hand over was listed"
        );
        assert_eq!(
            super::read_range_with_stock_in(&roots, "0/Scans.epub", 0, PIECE),
            Err(super::RangeFault::NotADocument)
        );
        let file = fs::File::options()
            .write(true)
            .open(&path)
            .expect("the file");
        file.set_len(super::MAX_DOCUMENT_BYTES).expect("resized");
        drop(file);
        let last = u32::try_from(super::MAX_DOCUMENT_BYTES).expect("fits the wire") - 10;
        let range =
            super::read_range_with_stock_in(&roots, "0/Scans.epub", last, PIECE).expect("a range");
        assert_eq!(
            (u64::from(range.size), range.bytes.len()),
            (super::MAX_DOCUMENT_BYTES, 10)
        );
        fs::remove_file(path).expect("removed");
    }

    #[test]
    fn a_stock_library_book_is_read_in_ranges_only_from_inside_the_roots() {
        let card = scratch("range-stock-card");
        let kepubs = card.join(".kobo").join("kepub");
        fs::create_dir_all(&kepubs).expect("the stock reader's folders");
        let kepub = kepubs.join("1f7e9c2a");
        let book = a_book_called("Piranesi");
        fs::write(&kepub, &book).expect("a store book");
        let elsewhere = scratch("range-stock-elsewhere").join("secret.epub");
        fs::write(&elsewhere, a_book_called("Not a card book")).expect("a file off the card");
        let roots = [card];
        let id = |path: &std::path::Path| {
            format!("n/{}", path.canonicalize().expect("a real path").display())
        };
        let range = super::read_range_with_stock_in(&roots, &id(&kepub), 0, PIECE)
            .expect("a store book on the card");
        assert_eq!(range.bytes, book);
        assert_eq!(
            super::read_range_with_stock_in(&roots, &id(&elsewhere), 0, PIECE),
            Err(super::RangeFault::NotADocument),
            "a file outside the card was read through a stock identifier"
        );
    }

    #[test]
    fn a_book_whose_path_the_wire_cannot_carry_is_left_off_rather_than_ending_the_listing() {
        let root = scratch("long-paths");
        // Three bytes to the letter, so sixty letters a folder is 180 bytes
        // and two folders deep is past what an identifier may be.
        let folder = "\u{672c}".repeat(60);
        let deep = root.join(&folder).join(&folder);
        fs::create_dir_all(&deep).expect("the folders");
        fs::write(deep.join("Deep.epub"), b"body").expect("a deep book");
        fs::write(root.join(&folder).join("Near.epub"), b"body").expect("a nearer book");
        fs::write(root.join("Top.epub"), b"body").expect("a book at the top");
        let entries: Vec<_> = list_in(&[root])
            .entries
            .into_iter()
            .map(super::to_wire)
            .collect();
        let titles: Vec<_> = entries.iter().map(|entry| entry.title.as_str()).collect();
        assert_eq!(titles, ["Near", "Top"]);
        let frame = kobo_protocol::Frame {
            version: kobo_protocol::VERSION,
            request_id: 1,
            message: kobo_protocol::Message::DeviceResult(kobo_protocol::DeviceResult::Library {
                entries,
                truncated: false,
            }),
        };
        assert!(kobo_protocol::encode(&frame).is_ok());
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
    fn the_folder_of_synced_books_is_a_root_and_no_older_root_moved_to_make_room() {
        // An identifier begins with its root's place in the list, and every
        // kept reading position is filed under an identifier.
        assert_eq!(
            super::ROOTS[..3],
            ["/mnt/onboard", "/mnt/onboard/.adds/cobalt/data", "/mnt/sd"]
        );
        assert_eq!(super::ROOTS[3], super::SYNCED_BOOKS);
        assert_eq!(super::SYNCED_BOOKS, "/mnt/onboard/.adds/cobalt/sync/books");
    }

    #[test]
    fn a_synced_book_is_listed_and_one_still_arriving_is_not() {
        let card = scratch("sync-card");
        let synced = scratch("sync-books");
        fs::create_dir_all(synced.join(".stfolder")).expect("the engine's marker");
        fs::create_dir_all(synced.join(".stversions")).expect("the engine's old copies");
        fs::create_dir_all(synced.join("Essays")).expect("a folder the owner made");
        fs::write(synced.join("River.epub"), a_book_called("River")).expect("an arrived book");
        fs::write(synced.join("Essays/Tides.md"), b"# Tides").expect("an arrived note");
        for partial in [
            ".syncthing.Harbour.epub.tmp",
            "~syncthing~Harbour.epub.tmp",
            "Essays/.syncthing.Salt.md.tmp",
        ] {
            fs::write(synced.join(partial), b"PK\x03\x04 half a book").expect("a partial");
        }
        fs::write(synced.join(".stignore"), b"*.part").expect("the engine's ignore list");
        fs::write(synced.join(".stfolder/marker.txt"), b"").expect("a marker file");
        fs::write(synced.join(".stversions/River.epub"), b"an older River").expect("an old copy");
        fs::write(synced.join("._River.epub"), b"a sidecar").expect("a copying sidecar");

        let roots = [card, synced];
        let listed: Vec<_> = list_in(&roots)
            .entries
            .into_iter()
            .map(|entry| entry.id)
            .collect();
        assert_eq!(listed, ["1/River.epub", "1/Essays/Tides.md"]);
        assert!(read_in(&roots, "1/River.epub").is_some());
        for id in [
            "1/.syncthing.Harbour.epub.tmp",
            "1/~syncthing~Harbour.epub.tmp",
            "1/.stignore",
            "1/.stfolder/marker.txt",
            "1/.stversions/River.epub",
            "1/._River.epub",
        ] {
            assert!(read_in(&roots, id).is_none(), "{id} was readable");
        }
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
    fn only_what_a_listing_could_name_can_be_read() {
        let root = scratch("readable");
        let shelf = root.join("readlater");
        let secrets = root.join(".secrets");
        fs::create_dir_all(&shelf).expect("an application's folder");
        fs::create_dir_all(&secrets).expect("a hidden folder");
        fs::write(shelf.join("Notes.md"), b"# Notes").expect("a listed note");
        fs::write(shelf.join("book-1a2b3c4d"), a_book_called("Kept")).expect("a kept book");
        fs::write(shelf.join("articles.json"), b"{}").expect("an app's own file");
        fs::write(shelf.join("token"), b"hunter2").expect("an app's own blob");
        fs::write(secrets.join("lichess.epub"), b"not a book").expect("a secret");
        let roots = [root];

        let listed: Vec<_> = list_in(&roots)
            .entries
            .into_iter()
            .map(|entry| entry.id)
            .collect();
        assert_eq!(
            listed,
            ["0/readlater/book-1a2b3c4d", "0/readlater/Notes.md"]
        );
        for id in &listed {
            assert!(
                read_in(&roots, id).is_some(),
                "{id} was listed and not readable"
            );
        }
        for id in [
            "0/readlater/articles.json",
            "0/readlater/token",
            "0/.secrets/lichess.epub",
        ] {
            assert!(read_in(&roots, id).is_none(), "{id} was readable");
        }
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
        super::nickel::merge_from(&mut listing, &database, &roots, None);
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
    fn a_database_copied_off_a_reader_names_the_books_on_the_copy_of_its_card() {
        let card = scratch("copied-card");
        fs::write(
            card.join("austen_pride_v2.epub"),
            a_book_called("Pride and Prejudice"),
        )
        .expect("a sideloaded book");
        let kepubs = card.join(".kobo").join("kepub");
        fs::create_dir_all(&kepubs).expect("the stock reader's folders");
        fs::write(kepubs.join("1f7e9c2a"), a_book_called("Piranesi")).expect("a store book");
        let database = card.join(".kobo").join("KoboReader.sqlite");
        let connection = rusqlite::Connection::open(&database).expect("a scratch library");
        connection
            .execute_batch(
                "CREATE TABLE content (ContentID TEXT, Title TEXT, MimeType TEXT,
                    IsDownloaded TEXT, ___FileSize INTEGER, ContentType TEXT);
                 INSERT INTO content VALUES
                    ('file:///mnt/onboard/austen_pride_v2.epub', 'Pride and Prejudice',
                     'application/epub+zip', 'true', 9, '6'),
                    ('file:///mnt/onboard/.kobo/kepub/1f7e9c2a', 'Piranesi',
                     'application/x-kobo-epub+zip', 'true', 9, '6');",
            )
            .expect("the rows");
        drop(connection);
        let roots = [card];
        let listing = super::list_with_stock_in(&roots);
        let found: Vec<_> = listing
            .entries
            .iter()
            .map(|entry| (entry.title.as_str(), entry.on_card))
            .collect();
        assert_eq!(found, [("Piranesi", true), ("Pride and Prejudice", true)]);
        let piranesi = &listing.entries[0].id;
        assert!(super::read_with_stock_in(&roots, piranesi).is_some());
    }

    #[test]
    fn only_an_epub_directly_in_the_articles_folder_can_be_deleted() {
        let card = scratch("articles");
        let articles = card.join("Articles");
        fs::create_dir_all(articles.join("Kept")).expect("the articles folder");
        fs::write(articles.join("River.epub"), b"article").expect("an article");
        fs::write(articles.join("notes.txt"), b"notes").expect("a note");
        fs::write(articles.join("Kept").join("Deep.epub"), b"deeper").expect("a nested book");
        fs::write(card.join("Bleak House.epub"), b"a book").expect("a book");
        let roots = [card.clone()];
        for id in [
            "0/Bleak House.epub",
            "0/Articles/notes.txt",
            "0/Articles/Kept/Deep.epub",
            "0/Articles/../Bleak House.epub",
            "1/Articles/River.epub",
            "0/Articles/Missing.epub",
            "0/Articles/",
        ] {
            assert_eq!(
                super::remove_article(&roots, id),
                Err(super::Removal::NotAnArticle),
                "{id}"
            );
        }
        assert!(card.join("Bleak House.epub").exists());
        assert!(articles.join("Kept").join("Deep.epub").exists());
        assert_eq!(
            super::remove_article(&roots, "0/Articles/River.epub"),
            Ok(())
        );
        assert!(!articles.join("River.epub").exists());
    }

    #[cfg(unix)]
    #[test]
    fn an_article_that_is_a_link_to_a_book_elsewhere_is_not_followed() {
        let card = scratch("articles-link");
        fs::create_dir_all(card.join("Articles")).expect("the articles folder");
        fs::write(card.join("Bleak House.epub"), b"a book").expect("a book");
        std::os::unix::fs::symlink(
            card.join("Bleak House.epub"),
            card.join("Articles").join("Bleak House.epub"),
        )
        .expect("a link");
        assert_eq!(
            super::remove_article(&[card.clone()], "0/Articles/Bleak House.epub"),
            Err(super::Removal::NotAnArticle)
        );
        assert!(card.join("Bleak House.epub").exists());
    }

    #[test]
    fn a_sideloaded_book_gone_from_the_card_is_not_listed_as_waiting_to_download() {
        let card = scratch("gone-card");
        let database = card.join("KoboReader.sqlite");
        let connection = rusqlite::Connection::open(&database).expect("a scratch library");
        connection
            .execute_batch(&format!(
                "CREATE TABLE content (ContentID TEXT, Title TEXT, MimeType TEXT,
                    IsDownloaded TEXT, ___FileSize INTEGER, ContentType TEXT);
                 INSERT INTO content VALUES
                    ('file://{}/Articles/River.epub', 'River', 'application/epub+zip',
                     'true', 9, '6'),
                    ('7a866127', 'Cloud Atlas', 'application/x-kobo-epub+zip', 'false', 0, '6');",
                card.display()
            ))
            .expect("the rows");
        drop(connection);
        let mut listing = super::Listing::default();
        super::nickel::merge_from(&mut listing, &database, &[card], None);
        let titles: Vec<_> = listing
            .entries
            .iter()
            .map(|entry| entry.title.as_str())
            .collect();
        assert_eq!(titles, ["Cloud Atlas"]);
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
        super::nickel::merge_from(&mut listing, &database, &[], None);
        let titles: Vec<_> = listing
            .entries
            .iter()
            .map(|entry| (entry.title.as_str(), entry.on_card))
            .collect();
        assert_eq!(titles, [("Piranesi", false)]);
    }
}

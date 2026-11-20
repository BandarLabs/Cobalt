//! Sending a book or a document to the reader's card, where Books lists it.
//!
//! Books lists every EPUB, PDF, text, HTML and Markdown file it finds in an
//! ordinary folder on the card, so getting a book onto the reader is a copy
//! and nothing more: no app has to be told. `kobo send novel.epub` makes that
//! copy over Wi-Fi into the card's `Books` folder, the way a clipped article
//! reaches `Articles`, and with the same promises, kept by the same code in
//! [`super::clip_delivery`]: nothing half written is ever listed, a different
//! file of the same name is kept and the new one numbered, a book the reader
//! already holds is recognised and not copied again, and a notice says it
//! arrived.
//!
//! What differs from an article is size. An article is a few megabytes made
//! on this computer to fit; a book is whatever the owner has. So the card is
//! asked for room before anything is copied, the copy is checked against the
//! file's own hash when it lands, and the book travels in slices, so that
//! neither this computer nor the reader's shell ever holds all of it at once.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use super::clip_delivery::{self as delivery, Missed, Reader, Saved, BOOKS};
use super::console;
use kobo_policy::library::Kind;

/// Where books are saved on the reader.
const DEVICE_BOOKS: &str = "/mnt/onboard/Books";

/// The largest file a send copies.
///
/// Far more than Books opens, on purpose: a scanned PDF of a few hundred
/// megabytes is an ordinary thing to own, and the Kobo's own reader opens it
/// whatever Books makes of it. A bound all the same, because the file is held
/// in memory once to be hashed and sent, and it travels as one SSH session
/// that a reader going to sleep can cut: past this, a cable is the better
/// tool and the refusal says so.
pub const MAX_SEND_BYTES: u64 = 512 * 1024 * 1024;

/// The slowest copy still worth waiting for, in bytes a second.
///
/// A package install measured about 650 kilobytes a second through the same
/// pipe, so this is a fifth of that: slow enough that a reader at the far end
/// of the house is not given up on, fast enough that a connection that has
/// stopped is noticed while the owner is still there.
const SLOWEST_COPY: u64 = 128 * 1024;

/// The size from which a copy takes long enough that saying it has started
/// is kinder than silence.
const WORTH_A_LINE: u64 = 1024 * 1024;

/// How many files one folder send takes. A shelf of five hundred is more than
/// anybody reads from, and a bound keeps `kobo send ~` from walking a home
/// directory.
const MAX_FOLDER_FILES: usize = 500;

/// How deep below the folder named a send looks, the depth Books itself
/// walks the card to.
const MAX_FOLDER_DEPTH: usize = kobo_policy::library::MAX_DEPTH;

/// The suffixes Books lists a file by, longest first where one ends another,
/// so `.kepub.epub` is never read as `.epub`.
const SUFFIXES: [&str; 9] = [
    ".kepub.epub",
    ".epub",
    ".markdown",
    ".md",
    ".xhtml",
    ".html",
    ".htm",
    ".txt",
    ".pdf",
];

/// The limit a file of `bytes` is over, when it is larger than Books opens.
///
/// The one place this command asks whether Books will open a file. Books
/// reads a document from the runtime a range at a time, up to
/// [`kobo_protocol::MAX_LIBRARY_OPENABLE_BYTES`]; when that limit moves this
/// follows it, and every sentence that tells an owner a file is too large for
/// Books is worded from the number returned here. A larger file is sent
/// anyway, because the card is also where the Kobo's own reader looks.
pub(crate) fn larger_than_books_opens(bytes: u64) -> Option<u64> {
    let limit = u64::try_from(kobo_protocol::MAX_LIBRARY_OPENABLE_BYTES).unwrap_or(u64::MAX);
    (bytes > limit).then_some(limit)
}

/// The limit a file of `bytes` is over, when it is larger than Books lists.
///
/// The walk that builds the shelf passes over anything this large without a
/// word, so the owner has to be told here or they will look for a book that
/// is on the card and not on the shelf.
fn larger_than_books_lists(bytes: u64) -> Option<u64> {
    let limit = kobo_policy::library::MAX_DOCUMENT_BYTES;
    (bytes > limit).then_some(limit)
}

/// One file read and ready to go.
#[derive(Clone, Debug)]
pub(crate) struct Book {
    /// The name it is saved under before any number a clash adds, without
    /// its suffix. Never holds a character the card's file system refuses.
    stem: String,
    suffix: &'static str,
    kind: Kind,
    /// Shared rather than owned, so the thread that feeds the reader's shell
    /// reads the same copy instead of a second one.
    bytes: Arc<Vec<u8>>,
    /// The SHA-256 of the bytes, which is how the reader recognises a copy it
    /// already has and how it knows the one it was just sent arrived whole.
    digest: String,
}

impl Book {
    /// Reads `path`, refusing what Books does not list and what is too large
    /// to send, each with the reason.
    fn read(path: &Path) -> Result<Self, String> {
        let shown = path.display();
        let length = fs::metadata(path)
            .map_err(|error| format!("{shown} cannot be read: {error}"))?
            .len();
        if length == 0 {
            return Err(format!("{shown} is empty, so there is nothing to send."));
        }
        if length > MAX_SEND_BYTES {
            return Err(console::unsupported(format!(
                "{shown} is {}. Send copies files up to {}; copy a larger one over USB.",
                size_words(length),
                size_words(MAX_SEND_BYTES)
            )));
        }
        let bytes = fs::read(path).map_err(|error| format!("{shown} cannot be read: {error}"))?;
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let Some((stem, suffix, kind)) = named(&name, &bytes) else {
            return Err(console::unsupported(format!(
                "{shown} is not a file Books lists. Books lists EPUB, PDF, text, HTML and \
                 Markdown files."
            )));
        };
        Ok(Self {
            stem: card_stem(stem),
            suffix,
            kind,
            digest: super::sha256::hex_digest(&bytes),
            bytes: Arc::new(bytes),
        })
    }

    /// The name it takes on the card when no other file has it.
    fn wanted(&self) -> String {
        format!("{}{}", self.stem, self.suffix)
    }

    fn len(&self) -> u64 {
        u64::try_from(self.bytes.len()).unwrap_or(u64::MAX)
    }

    /// What the reader's notice calls it. A PDF or a page of text is not a
    /// book, and a notice that called it one would read as a mistake.
    fn heading(&self) -> &'static str {
        match self.kind {
            Kind::Epub | Kind::Kepub => "Book arrived",
            Kind::Pdf | Kind::Text | Kind::Html | Kind::Markdown => "Document arrived",
        }
    }
}

/// A file name split into the stem, the suffix Books lists it by, and what
/// that makes it.
///
/// The name decides when it can. A file with no suffix Books knows is looked
/// at instead, and an EPUB or a PDF is given the suffix it lacks, because the
/// Kobo's own reader goes by the name alone and would pass it over.
fn named<'a>(name: &'a str, bytes: &[u8]) -> Option<(&'a str, &'static str, Kind)> {
    let lowered = name.to_ascii_lowercase();
    for suffix in SUFFIXES {
        if lowered.ends_with(suffix) {
            let kind = Kind::from_name(suffix)?;
            return Some((&name[..name.len() - suffix.len()], suffix, kind));
        }
    }
    if super::detect::is_epub(bytes) {
        Some((name, ".epub", Kind::Epub))
    } else if bytes.starts_with(b"%PDF-") {
        Some((name, ".pdf", Kind::Pdf))
    } else {
        None
    }
}

/// A stem the reader's card accepts, by the rule a clipped article's name
/// follows.
///
/// The card is FAT, which refuses a colon, a question mark or a quote in a
/// name, and a book called `Why? A "life".epub` on this computer is common
/// enough. One rule for both folders also keeps the promise the notice
/// relies on: a saved name never holds a quote or a backslash, so it can be
/// written into the record as it is.
fn card_stem(stem: &str) -> String {
    // That rule answers "Article" for a name with nothing in it the card
    // keeps, which is the wrong word here.
    let kept = |character: char| {
        !(character.is_control() || character.is_whitespace() || "<>:\"/\\|?*.".contains(character))
    };
    if !stem.chars().any(kept) {
        return "Book".to_owned();
    }
    let named = kobo_article::book::file_name(stem);
    named.strip_suffix(".epub").unwrap_or(&named).to_owned()
}

/// One file and what the reader did with it.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Landed {
    /// The name it asked for, before any number.
    wanted: String,
    saved: Saved,
    kind: Kind,
    bytes: u64,
}

/// What a send to Books did, for the lines that end it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Outcome {
    /// Whether a folder was sent, which changes how the result is counted.
    folder: bool,
    landed: Vec<Landed>,
    /// Files in a folder that were not tried, each with the reason.
    left_out: Vec<String>,
}

impl Outcome {
    /// The lines that end a send: what went, what the reader already had,
    /// and anything the owner should know about reading it there.
    #[must_use]
    pub fn said(&self, file: &str, reader: &str) -> String {
        if self.folder {
            return self.said_of_a_folder(file, reader);
        }
        let Some(landed) = self.landed.first() else {
            return format!("Nothing in {file} was sent.");
        };
        match &landed.saved {
            Saved::Already(name) => format!(
                "{file} is already in the Books folder on {reader}{}, so nothing was sent.",
                if *name == landed.wanted {
                    String::new()
                } else {
                    format!(" as {name}")
                }
            ),
            Saved::New(name) => {
                let mut said = if *name == landed.wanted {
                    format!("Sent {file} to Books on {reader}.")
                } else {
                    format!(
                        "Sent {file} to Books on {reader} as {name}, because a different {} \
                         was already there.",
                        landed.wanted
                    )
                };
                if let Some(note) = reading_note(landed.kind, landed.bytes) {
                    said.push('\n');
                    said.push_str(&note);
                }
                said
            }
        }
    }

    fn said_of_a_folder(&self, file: &str, reader: &str) -> String {
        let new: Vec<&Landed> = self
            .landed
            .iter()
            .filter(|landed| matches!(landed.saved, Saved::New(_)))
            .collect();
        let already = self.landed.len() - new.len();
        let mut lines = Vec::new();
        if new.is_empty() {
            lines.push(format!(
                "Everything Books lists in {file} is already in the Books folder on {reader}, \
                 so nothing was sent."
            ));
        } else {
            lines.push(format!(
                "Sent {} from {file} to Books on {reader}.",
                files_word(new.len())
            ));
            if already > 0 {
                lines.push(format!(
                    "{} already there and not sent again.",
                    if already == 1 {
                        "1 was".to_owned()
                    } else {
                        format!("{already} were")
                    }
                ));
            }
        }
        for landed in &new {
            let Saved::New(name) = &landed.saved else {
                continue;
            };
            if *name != landed.wanted {
                lines.push(format!(
                    "{} was saved as {name}, because a different {} was already there.",
                    landed.wanted, landed.wanted
                ));
            }
            // Only a size is worth a line to itself: what happens to a PDF is
            // the same for every PDF, and is said once below.
            if let Some(note) = size_note(landed.kind, landed.bytes) {
                lines.push(format!("{name}: {note}"));
            }
        }
        if new.iter().any(|landed| !landed.kind.is_readable()) {
            lines.push(
                "Books lists the PDFs and Kobo EPUBs among them, and the Kobo's own reader \
                 opens those."
                    .to_owned(),
            );
        }
        if !self.left_out.is_empty() {
            lines.push("Not sent:".to_owned());
            lines.extend(self.left_out.iter().map(|why| format!("  {why}")));
        }
        lines.join("\n")
    }
}

/// What the owner should know about reading a file of this kind and size on
/// the reader, when the answer is not simply that Books opens it.
///
/// Said after the send rather than instead of it: the file is wanted on the
/// reader either way, and the Kobo's own reader finds it in the same folder.
fn reading_note(kind: Kind, bytes: u64) -> Option<String> {
    size_note(kind, bytes).or_else(|| {
        (!kind.is_readable())
            .then(|| "Books lists it, and the Kobo's own reader opens it.".to_owned())
    })
}

/// What a file's size costs it on the reader, when it costs anything.
///
/// A file Books never opens itself, a PDF or a Kobo EPUB, is only too large
/// once Books stops listing it; how much Books can open says nothing about a
/// file it hands to the Kobo's own reader.
fn size_note(kind: Kind, bytes: u64) -> Option<String> {
    if let Some(limit) = larger_than_books_lists(bytes) {
        return Some(format!(
            "It is {}, and Books lists files up to {}, so it will not appear in Books. It is \
             in the Books folder on the card.",
            size_words(bytes),
            size_words(limit)
        ));
    }
    let limit = larger_than_books_opens(bytes).filter(|_| kind.is_readable())?;
    Some(format!(
        "It is {}, and Books opens files up to {}, so Books lists it and cannot open it.",
        size_words(bytes),
        size_words(limit)
    ))
}

/// "1 file", "12 files".
fn files_word(count: usize) -> String {
    if count == 1 {
        "1 file".to_owned()
    } else {
        format!("{count} files")
    }
}

/// A size as an owner reads one: "812 KB", "9.2 MB", "120 MB".
pub(crate) fn size_words(bytes: u64) -> String {
    const MEGABYTE: u64 = 1024 * 1024;
    if bytes < MEGABYTE {
        format!("{} KB", bytes.div_ceil(1024).max(1))
    } else if bytes < 10 * MEGABYTE && bytes % MEGABYTE != 0 {
        let tenths = bytes * 10 / MEGABYTE;
        format!("{}.{} MB", tenths / 10, tenths % 10)
    } else {
        format!("{} MB", bytes / MEGABYTE)
    }
}

/// What a file is, in the words a preview uses.
fn kind_words(kind: Kind) -> &'static str {
    match kind {
        Kind::Epub => "an EPUB book",
        Kind::Kepub => "a Kobo EPUB book",
        Kind::Pdf => "a PDF",
        Kind::Text => "a text file",
        Kind::Html => "an HTML page",
        Kind::Markdown => "a Markdown file",
    }
}

/// Says what a send of `file` would do, and sends nothing.
pub fn preview(file: &str) -> Result<(), String> {
    let book = Book::read(Path::new(file))?;
    println!(
        "{file} is {} of {}. A send saves it as {} in the Books folder on the reader's card.",
        kind_words(book.kind),
        size_words(book.len()),
        book.wanted()
    );
    println!(
        "{}",
        reading_note(book.kind, book.len()).unwrap_or_else(|| "Books opens it.".to_owned())
    );
    Ok(())
}

/// Sends a book, or every file Books lists in a folder, to `reader`.
///
/// A folder goes one file at a time, each as a single book would, and all
/// into the one `Books` folder: Books shows a flat shelf, so the folders a
/// collection was kept in on this computer would only be depth for its walk
/// to spend. A reader that stops answering ends the send there, with what
/// already went said first, and the next send resumes it, because the reader
/// recognises every book it was given.
pub fn send(path: &Path, reader: &Reader) -> Result<Outcome, String> {
    if let Reader::Device(host) = reader {
        if !super::valid_device_host(host) {
            return Err(console::usage(format!(
                "{host:?} is not an address this can reach"
            )));
        }
    }
    let folder = path.is_dir();
    let (files, left_out) = if folder {
        gather(path)?
    } else {
        (vec![path.to_path_buf()], Vec::new())
    };
    let mut outcome = Outcome {
        folder,
        landed: Vec::new(),
        left_out,
    };
    let total = files.len();
    for file in files {
        let book = match Book::read(&file) {
            Ok(book) => book,
            Err(why) if folder => {
                outcome.left_out.push(console::display(&why).to_owned());
                continue;
            }
            Err(why) => return Err(why),
        };
        if folder || book.len() >= WORTH_A_LINE {
            console::Console::progress(&format!(
                "sending {} ({})",
                book.wanted(),
                size_words(book.len())
            ));
        }
        let saved = deliver(reader, &book).map_err(|missed| {
            let sent = outcome.landed.len();
            let so_far = if folder && sent > 0 {
                format!(" {sent} of {total} went before it.")
            } else {
                String::new()
            };
            match missed {
                Missed::Away(detail) => console::Console::with_details(
                    console::target(format!(
                        "{} did not answer, so {} was not sent. It may be asleep or away \
                         from Wi-Fi.{so_far}",
                        capitalised(&reader.label()),
                        file.display()
                    )),
                    &detail,
                ),
                Missed::Refused(why) => format!("{why}{so_far}"),
            }
        })?;
        outcome.landed.push(Landed {
            wanted: book.wanted(),
            saved,
            kind: book.kind,
            bytes: book.len(),
        });
    }
    if outcome.landed.is_empty() {
        return Err(format!(
            "Nothing in {} could be sent:\n  {}",
            path.display(),
            outcome.left_out.join("\n  ")
        ));
    }
    Ok(outcome)
}

/// A label that begins a sentence: "the simulator" as "The simulator".
fn capitalised(label: &str) -> String {
    let mut letters = label.chars();
    letters.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(letters).collect()
    })
}

/// The files Books lists under `folder`, in name order, and those passed
/// over for being too many.
///
/// Only the name is asked here. Looking inside every file of a folder to see
/// whether it might be a book would read a photo library on the way past,
/// which is the reason Books itself goes by the name.
fn gather(folder: &Path) -> Result<(Vec<PathBuf>, Vec<String>), String> {
    let mut found = Vec::new();
    let mut pending = vec![(folder.to_path_buf(), 0)];
    while let Some((directory, depth)) = pending.pop() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let path = entry.path();
            if path.is_dir() {
                if depth < MAX_FOLDER_DEPTH {
                    pending.push((path, depth + 1));
                }
            } else if Kind::from_name(&name).is_some() {
                found.push(path);
            }
        }
    }
    if found.is_empty() {
        return Err(console::unsupported(format!(
            "{} holds nothing Books lists. Books lists EPUB, PDF, text, HTML and Markdown \
             files.",
            folder.display()
        )));
    }
    found.sort();
    let mut left_out = Vec::new();
    if found.len() > MAX_FOLDER_FILES {
        left_out.push(format!(
            "{} more, because one send takes {MAX_FOLDER_FILES}. Send the folder again for \
             the next ones.",
            files_word(found.len() - MAX_FOLDER_FILES)
        ));
        found.truncate(MAX_FOLDER_FILES);
    }
    Ok((found, left_out))
}

fn deliver(reader: &Reader, book: &Book) -> Result<Saved, Missed> {
    match reader {
        Reader::Simulator => shelve_in_folders(
            &kobo_sim::simulated_library_root(),
            &kobo_sim::simulated_notices_folder().join("inbox"),
            book,
        )
        .map_err(Missed::Refused),
        Reader::Device(host) => shelve_on_device(host, book),
    }
}

/// The simulator's half: the card and the inbox are folders here.
fn shelve_in_folders(card: &Path, inbox: &Path, book: &Book) -> Result<Saved, String> {
    let folder = card.join(BOOKS);
    fs::create_dir_all(&folder)
        .and_then(|()| fs::create_dir_all(inbox))
        .map_err(|error| format!("could not make the books folder: {error}"))?;
    if let Some(held) = held_copy(&folder, &book.bytes) {
        return Ok(Saved::Already(held));
    }
    // A card that cannot be asked is taken to have room, as the runtime's own
    // shelves take it: refusing every send on a file system that will not
    // answer turns not knowing into not working.
    if let Some(free) = kobo_abi::free_space(&folder) {
        if free < needed(book.len()) {
            return Err(no_room(&book.wanted(), book.len(), free, "the simulator"));
        }
    }
    let name = delivery::place(&folder, &book.stem, book.suffix, &book.bytes, "book")?;
    delivery::announce(inbox, "send", &notice(book, &name), "book")?;
    Ok(Saved::New(name))
}

/// The name of a file in `folder` holding exactly `bytes`, if there is one.
///
/// Only a file of the same length is read, so a shelf of large books costs a
/// directory listing and nothing more.
fn held_copy(folder: &Path, bytes: &[u8]) -> Option<String> {
    let length = u64::try_from(bytes.len()).ok()?;
    fs::read_dir(folder)
        .ok()?
        .flatten()
        .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
        .filter(|entry| {
            entry
                .metadata()
                .is_ok_and(|metadata| metadata.is_file() && metadata.len() == length)
        })
        .find(|entry| fs::read(entry.path()).is_ok_and(|held| held == bytes))
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
}

/// The room a book of `bytes` needs: itself, and the margin every large
/// write to the card leaves.
///
/// The stock reader's whole library is a database on this partition, and one
/// with nowhere to write comes back empty with nothing pointing at the book
/// that filled the card. The runtime's own shelves keep
/// [`kobo_protocol::SHELF_RESERVE`] free for it, and so does a send.
pub(crate) fn needed(bytes: u64) -> u64 {
    bytes.saturating_add(kobo_protocol::SHELF_RESERVE)
}

/// The refusal for a card without room, with both numbers.
fn no_room(name: &str, bytes: u64, free: u64, reader: &str) -> String {
    format!(
        "The card on {reader} has {} free. {name} is {}, and a send leaves {} for the \
         Kobo's own library, so it was not sent. Make room on the card and send it again.",
        size_words(free),
        size_words(bytes),
        size_words(kobo_protocol::SHELF_RESERVE)
    )
}

fn shelve_on_device(host: &str, book: &Book) -> Result<Saved, Missed> {
    let (head, tail) = device_script(book, needed(book.len()).div_ceil(1024));
    let bytes = Arc::clone(&book.bytes);
    let output = super::run_remote_shell_fed(
        &format!("root@{host}"),
        move |shell| write_script(shell, &head, &bytes, &tail),
        patience_for(book.len()),
    )
    .map_err(Missed::Away)?;
    if let Some(refusal) = refusal_in(&output.stdout, book, &format!("the reader at {host}")) {
        return Err(Missed::Refused(refusal));
    }
    delivery::taken(host, &output, "book")
}

/// The reader's own reason for not keeping a book, when its script gave one.
fn refusal_in(stdout: &[u8], book: &Book, reader: &str) -> Option<String> {
    let said = String::from_utf8_lossy(stdout);
    if let Some(free) = said.lines().find_map(|line| line.strip_prefix("full=")) {
        let free = free.trim().parse::<u64>().unwrap_or(0).saturating_mul(1024);
        return Some(no_room(&book.wanted(), book.len(), free, reader));
    }
    said.lines().any(|line| line == "damaged").then(|| {
        format!(
            "The copy of {} that reached {reader} was not the file that was sent, so it \
             was removed. Send it again.",
            book.wanted()
        )
    })
}

/// How long one book's copy may take: a minute to connect and settle, and
/// the time its size needs at the slowest speed worth waiting for.
pub(crate) fn patience_for(bytes: u64) -> Duration {
    Duration::from_secs(60 + bytes / SLOWEST_COPY)
}

/// The script that saves a book on the reader, as the part before the book
/// itself and the part after it.
///
/// It begins, moves the finished file into place and announces it with the
/// steps an article's script uses. Around them is what a book needs and an
/// article does not. A copy the reader already has is found by size first
/// and hash second, so only a file of exactly this length is ever read, and
/// the listing is asked for the size because counting the bytes of every
/// book on the shelf would read the whole shelf. The card is asked for
/// `needed_kilobytes` before anything is written. What arrives is hashed
/// again before it is given a name, so a transfer cut short or altered never
/// becomes a book that will not open.
///
/// Everything that follows the book is a function defined before it, and the
/// part after is that function's name alone. A connection that drops near
/// the end then either runs the whole of the ending or none of it, where a
/// script cut mid-way could have saved the book and never said so.
pub(crate) fn device_script(book: &Book, needed_kilobytes: u64) -> (String, String) {
    let (before, after) = notice_halves(book);
    let head = format!(
        "{opening}\
         count='{count}'\n\
         digest='{digest}'\n\
         for held in \"$dir\"/*; do\n\
         [ -f \"$held\" ] || continue\n\
         set -- $(ls -l \"$held\")\n\
         [ \"${{5:-}}\" = \"$count\" ] || continue\n\
         set -- $(sha256sum \"$held\")\n\
         if [ \"$1\" = \"$digest\" ]; then printf 'already=%s\\n' \"${{held##*/}}\"; exit 0; fi\n\
         done\n\
         {room}\
         partial=\"$dir/.send-$$.writing\"\n\
         trap 'rm -f \"$partial\"' EXIT\n\
         trap 'exit 1' HUP INT TERM\n\
         finish() {{\n\
         set -- $(sha256sum \"$partial\")\n\
         if [ \"$1\" != \"$digest\" ]; then printf 'damaged\\n'; exit 0; fi\n\
         {settling}{announcing}\
         }}\n\
         : > \"$partial\"\n",
        opening = delivery::opening_script(DEVICE_BOOKS, &book.stem),
        count = book.bytes.len(),
        digest = book.digest,
        room = super::room_script(needed_kilobytes),
        settling = delivery::settling_script(book.suffix),
        announcing = delivery::announcing_script("send", &before, &after),
    );
    (head, "finish\n".to_owned())
}

/// Writes a book's script to the reader's shell: the head, the book in
/// slices, and the tail.
fn write_script(
    shell: &mut dyn Write,
    head: &str,
    bytes: &[u8],
    tail: &str,
) -> std::io::Result<()> {
    shell.write_all(head.as_bytes())?;
    super::append_in_slices(shell, bytes)?;
    shell.write_all(tail.as_bytes())
}

/// The notice record for a book saved as `name`.
fn notice(book: &Book, name: &str) -> String {
    let (before, after) = notice_halves(book);
    format!("{before}{name}{after}")
}

/// The notice record either side of the saved name.
///
/// The runtime words an article's notice itself and takes any other kind's
/// heading from the record, so a book is announced through the inbox as it
/// stands, with nothing new for the runtime to learn. The title is the name
/// without its suffix, which is what Books lists the file as.
fn notice_halves(book: &Book) -> (String, String) {
    let record = kobo_json::ObjectBuilder::new()
        .set("kind", "book")
        .set("heading", book.heading())
        .set("title", book.stem.as_str())
        .set("app", "books")
        .build();
    delivery::around_the_name(&record, BOOKS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let folder = std::env::temp_dir().join(format!("kobo-books-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&folder);
        fs::create_dir_all(&folder).expect("a scratch folder");
        folder
    }

    /// A book as `Book::read` would make it from a file called `name`.
    fn book(name: &str, bytes: &[u8]) -> Book {
        let (stem, suffix, kind) = named(name, bytes).expect("a file Books lists");
        Book {
            stem: card_stem(stem),
            suffix,
            kind,
            digest: crate::sha256::hex_digest(bytes),
            bytes: Arc::new(bytes.to_vec()),
        }
    }

    fn new(saved: Result<Saved, String>) -> String {
        match saved {
            Ok(Saved::New(name)) => name,
            other => panic!("not saved: {other:?}"),
        }
    }

    fn records(inbox: &Path) -> Vec<kobo_json::Value> {
        let mut paths: Vec<PathBuf> = fs::read_dir(inbox)
            .expect("the inbox")
            .flatten()
            .map(|entry| entry.path())
            .collect();
        paths.sort();
        paths
            .iter()
            .map(|path| {
                assert_eq!(
                    path.extension().and_then(|extension| extension.to_str()),
                    Some("json"),
                    "nothing half written is left in the inbox"
                );
                kobo_json::parse(&fs::read_to_string(path).expect("read")).expect("valid")
            })
            .collect()
    }

    #[test]
    fn a_book_lands_in_the_books_folder_and_is_announced_as_a_book() {
        let card = scratch("land");
        let inbox = card.join("inbox");
        let name = new(shelve_in_folders(
            &card,
            &inbox,
            &book("Bleak House.epub", b"a long novel"),
        ));
        assert_eq!(name, "Bleak House.epub");
        assert_eq!(
            fs::read(card.join("Books").join(&name)).expect("saved"),
            b"a long novel"
        );
        let records = records(&inbox);
        assert_eq!(records.len(), 1);
        let field = |key: &str| records[0].get(key).and_then(kobo_json::Value::as_str);
        assert_eq!(field("kind"), Some("book"));
        assert_eq!(field("heading"), Some("Book arrived"));
        assert_eq!(field("title"), Some("Bleak House"));
        assert_eq!(field("app"), Some("books"));
        assert_eq!(field("path"), Some("Books/Bleak House.epub"));
        let _ = fs::remove_dir_all(&card);
    }

    #[test]
    fn the_runtime_reads_a_book_notice_as_one_it_can_show_and_open() {
        // The record goes through the inbox the runtime already reads, so
        // this is the runtime's own reader deciding whether it is a notice.
        let card = scratch("runtime");
        let state = card.join("notices");
        let name = new(shelve_in_folders(
            &card,
            &state.join(kobod::notices::INBOX),
            &book("Timetable.pdf", b"%PDF-1.4 a timetable"),
        ));
        let mut notices = kobod::notices::Notices::open(&state);
        assert_eq!(
            notices.collect(1_000),
            (1, 0),
            "one arrived, none discarded"
        );
        let list = notices.list(&card, false, kobod::notices::builtin_mark);
        assert_eq!(list.entries.len(), 1);
        assert_eq!(list.entries[0].heading, "Document arrived");
        assert_eq!(list.entries[0].title, "Timetable");
        assert_eq!(list.entries[0].target, kobo_protocol::NoticeTarget::Ready);
        assert_eq!(
            notices.target(list.entries[0].id, &card),
            Some(("books".to_owned(), format!("0/Books/{name}")))
        );
        let _ = fs::remove_dir_all(&card);
    }

    #[test]
    fn the_same_book_sent_again_is_neither_written_nor_announced_twice() {
        let card = scratch("same");
        let inbox = card.join("inbox");
        let novel = book("Tides.epub", b"the same bytes");
        let first = new(shelve_in_folders(&card, &inbox, &novel));
        assert_eq!(
            shelve_in_folders(&card, &inbox, &novel),
            Ok(Saved::Already(first))
        );
        // The same bytes under another name on this computer are the same
        // book on the reader.
        assert_eq!(
            shelve_in_folders(&card, &inbox, &book("Tides copy.epub", b"the same bytes")),
            Ok(Saved::Already("Tides.epub".to_owned()))
        );
        assert_eq!(fs::read_dir(card.join("Books")).expect("listed").count(), 1);
        assert_eq!(records(&inbox).len(), 1);
        let _ = fs::remove_dir_all(&card);
    }

    #[test]
    fn a_different_book_with_the_same_name_is_numbered_and_the_first_is_kept() {
        let card = scratch("clash");
        let inbox = card.join("inbox");
        let first = new(shelve_in_folders(
            &card,
            &inbox,
            &book("Tides.epub", b"first edition"),
        ));
        let second = new(shelve_in_folders(
            &card,
            &inbox,
            &book("Tides.epub", b"second edition"),
        ));
        assert_eq!(
            (first.as_str(), second.as_str()),
            ("Tides.epub", "Tides (2).epub")
        );
        assert_eq!(
            fs::read(card.join("Books").join("Tides.epub")).expect("kept"),
            b"first edition"
        );
        // The number goes before the whole suffix, or a Kobo EPUB would stop
        // being one.
        new(shelve_in_folders(
            &card,
            &inbox,
            &book("Purchase.kepub.epub", b"one"),
        ));
        let again = new(shelve_in_folders(
            &card,
            &inbox,
            &book("Purchase.KEPUB.EPUB", b"two"),
        ));
        assert_eq!(again, "Purchase (2).kepub.epub");
        let hidden = fs::read_dir(card.join("Books"))
            .expect("listed")
            .flatten()
            .any(|entry| entry.file_name().to_string_lossy().starts_with('.'));
        assert!(!hidden, "no partial file is left behind");
        let _ = fs::remove_dir_all(&card);
    }

    /// Runs a book's script under this computer's shell, with the card and
    /// the inbox pointed at `card`.
    #[cfg(unix)]
    fn run_script(card: &Path, book: &Book, needed_kilobytes: u64) -> String {
        let (head, tail) = device_script(book, needed_kilobytes);
        let mut script = Vec::new();
        write_script(&mut script, &head, &book.bytes, &tail).expect("written");
        let script = String::from_utf8(script)
            .expect("a script is text")
            .replace(DEVICE_BOOKS, &card.join("Books").to_string_lossy())
            .replace(
                "/mnt/onboard/.adds/cobalt/state/notices/inbox",
                &card.join("inbox").to_string_lossy(),
            );
        // Read from standard input, as the reader's shell reads it, and
        // because a book is longer than an argument may be.
        let mut shell = std::process::Command::new("sh")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("sh runs");
        let mut input = shell.stdin.take().expect("a pipe");
        let writer = std::thread::spawn(move || {
            // A script that ends early closes the pipe, as the reader's does.
            let _ = input.write_all(script.as_bytes());
        });
        let output = shell.wait_with_output().expect("sh ends");
        writer.join().expect("the script was written");
        assert!(output.status.success(), "{output:?}");
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    #[cfg(unix)]
    #[test]
    fn the_reader_script_saves_a_book_once_numbers_a_clash_and_reports_the_copy_it_had() {
        let card = scratch("script");
        // Longer than two slices, so the pieces have to join exactly.
        let long: Vec<u8> = (0..2 * crate::UPLOAD_SLICE + 12_345)
            .map(|n| u8::try_from(n % 251).expect("under 251"))
            .collect();
        let novel = book("Bleak House.epub", &long);
        assert_eq!(run_script(&card, &novel, 1), "saved=Bleak House.epub");
        assert_eq!(
            fs::read(card.join("Books").join("Bleak House.epub")).expect("saved"),
            long
        );
        assert_eq!(run_script(&card, &novel, 1), "already=Bleak House.epub");
        assert_eq!(
            run_script(&card, &book("Bleak House.epub", b"another edition"), 1),
            "saved=Bleak House (2).epub"
        );
        // Two records written in the same second are ordered by the shell's
        // process number, so they are looked for rather than counted off.
        let mut announced: Vec<String> = records(&card.join("inbox"))
            .iter()
            .filter_map(|record| record.get("path").and_then(kobo_json::Value::as_str))
            .map(str::to_owned)
            .collect();
        announced.sort();
        assert_eq!(
            announced,
            ["Books/Bleak House (2).epub", "Books/Bleak House.epub"],
            "the copy the reader had was not announced again"
        );
        let hidden = fs::read_dir(card.join("Books"))
            .expect("listed")
            .flatten()
            .any(|entry| entry.file_name().to_string_lossy().starts_with('.'));
        assert!(!hidden, "no partial file is left behind");
        let _ = fs::remove_dir_all(&card);
    }

    #[cfg(unix)]
    #[test]
    fn the_reader_script_refuses_a_card_without_room_before_anything_is_copied() {
        let card = scratch("full");
        let novel = book("Atlas.pdf", b"%PDF-1.4 every map");
        // More kilobytes than any disk this runs on has free.
        let said = run_script(&card, &novel, 4_000_000_000_000_000);
        let free = said.strip_prefix("full=").expect("refused for room");
        assert!(free.parse::<u64>().is_ok(), "{said}");
        assert_eq!(fs::read_dir(card.join("Books")).expect("listed").count(), 0);
        assert!(!card.join("inbox").exists() || records(&card.join("inbox")).is_empty());
        let refusal =
            refusal_in(said.as_bytes(), &novel, "the reader at 192.168.1.9").expect("a refusal");
        assert!(
            refusal.contains("Atlas.pdf") && refusal.contains("free"),
            "{refusal}"
        );
        let _ = fs::remove_dir_all(&card);
    }

    #[cfg(unix)]
    #[test]
    fn a_copy_that_does_not_match_the_file_is_removed_and_never_named() {
        let card = scratch("damaged");
        let mut novel = book("Tides.epub", b"what was sent");
        novel.digest = crate::sha256::hex_digest(b"what was meant");
        assert_eq!(run_script(&card, &novel, 1), "damaged");
        assert_eq!(fs::read_dir(card.join("Books")).expect("listed").count(), 0);
        assert!(refusal_in(b"damaged\n", &novel, "the simulator")
            .is_some_and(|refusal| refusal.contains("Send it again")));
        let _ = fs::remove_dir_all(&card);
    }

    #[test]
    fn the_reader_script_never_lets_a_name_reach_the_shell() {
        let (head, tail) = device_script(&book("Robert'); rm -rf $HOME #.epub", b"x"), 1);
        let script = format!("{head}{tail}");
        assert!(!script.contains("rm -rf $HOME"), "a name reached the shell");
        assert!(script.contains("/mnt/onboard/Books"));
        let checked = head.find("df -Pk").expect("room is asked for");
        assert!(head.contains("full="), "a card without room is said so");
        let begun = head.find(": > \"$partial\"").expect("the copy is begun");
        assert!(checked < begun, "room is asked for before the copy");
        let saved = head.find("mv \"$partial\"").expect("moved into place");
        let announced = head.find("mv \"$record\"").expect("the notice is moved");
        assert!(saved < announced, "the notice must follow the book");
        assert_eq!(
            tail, "finish\n",
            "the ending is one word, so a dropped connection cannot run half of it"
        );
    }

    #[test]
    fn a_name_the_card_would_refuse_is_made_one_it_takes() {
        let odd = book("Why? A \"life\": part 1/2.epub", b"x");
        assert_eq!(odd.wanted(), "Why A life part 1 2.epub");
        assert_eq!(book("???.epub", b"x").wanted(), "Book.epub");
        assert_eq!(book("Article.epub", b"x").wanted(), "Article.epub");
        let record = notice(&odd, &odd.wanted());
        assert!(kobo_json::parse(&record).is_ok(), "{record}");
    }

    #[test]
    fn every_suffix_sent_is_one_books_lists_by() {
        for suffix in SUFFIXES {
            let name = format!("Named{suffix}");
            let (stem, found, kind) = named(&name, b"").expect("listed");
            assert_eq!((stem, found), ("Named", suffix));
            assert_eq!(Kind::from_name(&name), Some(kind), "{suffix}");
        }
        assert_eq!(
            named("Purchase.kepub.epub", b"").map(|(_, suffix, kind)| (suffix, kind)),
            Some((".kepub.epub", Kind::Kepub))
        );
    }

    #[test]
    fn a_file_with_no_suffix_is_named_for_what_its_bytes_say() {
        let mut epub = b"PK\x03\x04".to_vec();
        epub.resize(30, 0);
        epub.extend_from_slice(b"mimetypeapplication/epub+zip");
        assert_eq!(
            named("book-1a2b", &epub).map(|(stem, suffix, _)| (stem, suffix)),
            Some(("book-1a2b", ".epub"))
        );
        assert_eq!(
            named("scan", b"%PDF-1.7").map(|(_, suffix, kind)| (suffix, kind)),
            Some((".pdf", Kind::Pdf))
        );
        assert!(named("letter.docx", b"PK\x03\x04 a word document").is_none());
    }

    #[test]
    fn a_file_books_does_not_list_is_refused_with_what_it_does_list() {
        let folder = scratch("refused");
        let letter = folder.join("letter.docx");
        fs::write(&letter, b"not a book").expect("a file");
        let error = Book::read(&letter).expect_err("refused");
        assert_eq!(console::category_of(&error), console::EXIT_UNSUPPORTED);
        assert!(
            error.contains("EPUB, PDF, text, HTML and Markdown"),
            "{error}"
        );
        let empty = folder.join("empty.epub");
        fs::write(&empty, b"").expect("a file");
        assert!(Book::read(&empty)
            .expect_err("refused")
            .contains("is empty"));
        let _ = fs::remove_dir_all(&folder);
    }

    #[test]
    fn the_owner_is_told_when_a_file_is_larger_than_books_opens() {
        // Written against the limit itself, which another change is free to
        // move.
        let limit = u64::try_from(kobo_protocol::MAX_LIBRARY_OPENABLE_BYTES).expect("fits");
        assert_eq!(larger_than_books_opens(limit), None);
        assert_eq!(larger_than_books_opens(limit + 1), Some(limit));
        assert_eq!(reading_note(Kind::Epub, limit), None);
        let lists = kobo_policy::library::MAX_DOCUMENT_BYTES;
        if limit < lists {
            let note = reading_note(Kind::Epub, limit + 1).expect("a note");
            assert!(note.contains("lists it and cannot open it"), "{note}");
            assert!(note.contains(&size_words(limit)), "{note}");
        }
        let note = reading_note(Kind::Text, lists + 1).expect("a note");
        assert!(note.contains("will not appear in Books"), "{note}");
    }

    #[test]
    fn a_pdf_is_said_to_open_in_the_kobos_own_reader() {
        assert_eq!(
            reading_note(Kind::Pdf, 1_000).as_deref(),
            Some("Books lists it, and the Kobo's own reader opens it.")
        );
        assert!(reading_note(Kind::Kepub, 1_000).is_some());
    }

    #[test]
    fn a_send_ends_by_saying_what_the_reader_did_with_the_file() {
        let landed = |saved: Saved| Outcome {
            folder: false,
            landed: vec![Landed {
                wanted: "Tides.epub".to_owned(),
                saved,
                kind: Kind::Epub,
                bytes: 1_000,
            }],
            left_out: Vec::new(),
        };
        assert_eq!(
            landed(Saved::New("Tides.epub".to_owned())).said("Tides.epub", "\"desk\""),
            "Sent Tides.epub to Books on \"desk\"."
        );
        assert_eq!(
            landed(Saved::New("Tides (2).epub".to_owned())).said("Tides.epub", "the simulator"),
            "Sent Tides.epub to Books on the simulator as Tides (2).epub, because a \
             different Tides.epub was already there."
        );
        assert_eq!(
            landed(Saved::Already("Tides.epub".to_owned())).said("Tides.epub", "the simulator"),
            "Tides.epub is already in the Books folder on the simulator, so nothing was sent."
        );
        assert_eq!(
            landed(Saved::Already("Old name.epub".to_owned())).said("Tides.epub", "\"desk\""),
            "Tides.epub is already in the Books folder on \"desk\" as Old name.epub, so \
             nothing was sent."
        );
    }

    #[test]
    fn a_folder_sends_every_file_books_lists_and_only_those() {
        let shelf = scratch("folder");
        fs::create_dir_all(shelf.join("Dickens")).expect("a folder");
        fs::write(shelf.join("Dickens").join("Bleak House.epub"), b"one").expect("a book");
        fs::write(shelf.join("Timetable.pdf"), b"%PDF-1.4").expect("a pdf");
        fs::write(shelf.join("cover.jpg"), b"\xff\xd8").expect("a picture");
        fs::write(shelf.join(".hidden.epub"), b"hidden").expect("a hidden file");
        let (files, left_out) = gather(&shelf).expect("books");
        let names: Vec<String> = files
            .iter()
            .map(|path| {
                path.file_name()
                    .expect("named")
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(names, ["Bleak House.epub", "Timetable.pdf"]);
        assert!(left_out.is_empty());
        let photos = scratch("folder-photos");
        fs::write(photos.join("cover.jpg"), b"\xff\xd8").expect("a picture");
        let error = gather(&photos).expect_err("nothing to send");
        assert!(error.contains("holds nothing Books lists"), "{error}");
        let _ = fs::remove_dir_all(&shelf);
        let _ = fs::remove_dir_all(&photos);
    }

    #[test]
    fn a_folder_send_counts_what_went_and_what_was_already_there() {
        let outcome = Outcome {
            folder: true,
            landed: vec![
                Landed {
                    wanted: "One.epub".to_owned(),
                    saved: Saved::New("One.epub".to_owned()),
                    kind: Kind::Epub,
                    bytes: 1_000,
                },
                Landed {
                    wanted: "Two.epub".to_owned(),
                    saved: Saved::New("Two (2).epub".to_owned()),
                    kind: Kind::Epub,
                    bytes: 1_000,
                },
                Landed {
                    wanted: "Three.epub".to_owned(),
                    saved: Saved::Already("Three.epub".to_owned()),
                    kind: Kind::Epub,
                    bytes: 1_000,
                },
            ],
            left_out: Vec::new(),
        };
        assert_eq!(
            outcome.said("shelf", "\"desk\""),
            "Sent 2 files from shelf to Books on \"desk\".\n\
             1 was already there and not sent again.\n\
             Two.epub was saved as Two (2).epub, because a different Two.epub was already there."
        );
    }

    #[test]
    fn a_larger_book_is_given_longer_to_copy() {
        assert_eq!(patience_for(0), Duration::from_secs(60));
        assert!(patience_for(MAX_SEND_BYTES) > patience_for(MAX_SEND_BYTES / 2));
        assert!(
            patience_for(MAX_SEND_BYTES) < Duration::from_secs(2 * 60 * 60),
            "a copy that has stopped is given up on the same afternoon"
        );
    }

    #[test]
    fn sizes_read_the_way_an_owner_says_them() {
        assert_eq!(size_words(1), "1 KB");
        assert_eq!(size_words(812 * 1024), "812 KB");
        assert_eq!(size_words(4 * 1024 * 1024), "4 MB");
        assert_eq!(size_words(9 * 1024 * 1024 + 300 * 1024), "9.2 MB");
        assert_eq!(size_words(120 * 1024 * 1024 + 5), "120 MB");
    }
}

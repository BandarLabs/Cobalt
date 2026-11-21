//! Which companion a file belongs to, chosen by what the file is.
//!
//! A photo goes to Frame, a subscription list to Feeds, a comic archive to
//! Panels, a story file to Parser, a study deck to Flashcards, a book or a
//! plain document to Books and a Markdown note to Vault. A PDF is a score for
//! Music Stand, a knitting pattern for Needles or a document to read, and
//! nothing in the file says which, so it is offered as a choice. The owner
//! names the file; this module names the companion. Detection reads the
//! extension first and the file's own magic bytes when the extension says
//! nothing, and it says so plainly when more than one companion could take
//! the file: `send` asks, or `--to` answers, but nothing guesses.
//!
//! An EPUB used to be routed to Fanshelf, which downloads its own works and
//! takes no files, so the one thing an owner most often has to send was the
//! one thing send refused. Books lists whatever sits in an ordinary folder on
//! the card, so a book is sent there.
//!
//! The table used to send every PDF and every Markdown file to Needles, so a
//! score and a note both replaced the owner's knitting pattern. Routing by
//! the app whose job the file is, and asking when two apps share a format,
//! is what keeps a send from landing somewhere the owner did not mean.
//!
//! Extensions decide by convention; magic bytes decide by content. Neither
//! is a validation: the companion's own check still accepts or refuses the
//! file before anything is sent. An explicit `--to` is the owner overriding
//! detection, so a name send knows always wins over what the file looks like.

use std::path::Path;

/// A companion `send` can hand a file to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Companion {
    Frame,
    Feeds,
    Panels,
    Parser,
    Flashcards,
    Fanshelf,
    Needles,
    MusicStand,
    Vault,
    Books,
}

impl Companion {
    /// Every companion, in the order help and errors list them.
    pub const ALL: [Self; 10] = [
        Self::Frame,
        Self::Books,
        Self::MusicStand,
        Self::Vault,
        Self::Feeds,
        Self::Panels,
        Self::Parser,
        Self::Flashcards,
        Self::Fanshelf,
        Self::Needles,
    ];

    /// The name the owner types after `--to`, which is also the companion
    /// command's own name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Frame => "frame",
            Self::Feeds => "feeds",
            Self::Panels => "panels",
            Self::Parser => "parser",
            Self::Flashcards => "flashcards",
            Self::Fanshelf => "fanshelf",
            Self::Needles => "needles",
            Self::MusicStand => "musicstand",
            Self::Vault => "vault",
            Self::Books => "books",
        }
    }

    /// The app's name as the reader shows it.
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            Self::Frame => "Frame",
            Self::Feeds => "Feeds",
            Self::Panels => "Panels",
            Self::Parser => "Parser",
            Self::Flashcards => "Flashcards",
            Self::Fanshelf => "Fanshelf",
            Self::Needles => "Needles",
            Self::MusicStand => "Music Stand",
            Self::Vault => "Vault",
            Self::Books => "Books",
        }
    }

    /// What the app opens, for a choice the owner has to make.
    #[must_use]
    pub fn purpose(self) -> &'static str {
        match self {
            Self::Frame => "photos",
            Self::Feeds => "feed subscription lists",
            Self::Panels => "comics",
            Self::Parser => "interactive stories",
            Self::Flashcards => "study decks",
            Self::Fanshelf => "EPUB books",
            Self::Needles => "knitting patterns",
            Self::MusicStand => "sheet music",
            Self::Vault => "Markdown notes",
            Self::Books => "books and documents",
        }
    }

    /// What choosing this app means, for a file several apps open.
    ///
    /// Books is only ever one of several when a PDF is involved, and there
    /// the plain "for books and documents" would promise more than happens:
    /// Books shows the PDF on its shelf and sends the owner to the Kobo's own
    /// reader to turn its pages. The choice says that before it is made, not
    /// after the file has landed.
    #[must_use]
    pub fn offer(self) -> String {
        match self {
            Self::Books => "Books, to read as a document: Books lists a PDF and the Kobo's own \
                            reader opens it"
                .to_owned(),
            _ => format!("{}, for {}", self.title(), self.purpose()),
        }
    }

    /// What a send would replace, for an app that keeps one thing at a time.
    ///
    /// Panels keeps one comic, so a send to it overwrites whatever the owner
    /// was in the middle of. `send` asks first, or wants `--replace`, rather
    /// than finding out afterwards. Needles used to be the other one, and a
    /// stray send replaced the pattern an owner was knitting; it now keeps
    /// every pattern it is given.
    #[must_use]
    pub fn replaces(self) -> Option<&'static str> {
        match self {
            Self::Panels => Some("comic"),
            _ => None,
        }
    }

    /// Whether the app takes a whole folder in one send.
    ///
    /// Each of these adds what the folder holds to what is already on the
    /// reader and sends only what changed, so a folder send never removes
    /// anything.
    #[must_use]
    pub fn takes_folders(self) -> bool {
        matches!(
            self,
            Self::Frame | Self::MusicStand | Self::Vault | Self::Books
        )
    }

    /// The companion behind a typed name, when there is one.
    ///
    /// Case and spacing are forgiven, so `Music Stand`, `music-stand` and
    /// `musicstand` are the same app, the way an owner reads it on screen.
    #[must_use]
    pub fn named(word: &str) -> Option<Self> {
        let wanted = squeeze(word);
        Self::ALL
            .into_iter()
            .find(|companion| companion.name() == wanted || squeeze(companion.title()) == wanted)
    }
}

/// A name with case, spaces and punctuation taken out.
fn squeeze(word: &str) -> String {
    word.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|character| character.to_ascii_lowercase())
        .collect()
}

/// How choosing one companion can fail.
#[derive(Debug)]
pub enum ChooseError {
    /// More than one companion could take the file; the caller asks which.
    Ambiguous(Vec<Companion>),
    /// Anything else, already tagged for the console taxonomy.
    Message(String),
}

/// Every companion that could take `path`, most specific first.
///
/// Empty means nothing we know wants it. More than one means the file is a
/// format more than one companion reads (a PDF score, pattern or document, a
/// zip that could be a comic or a deck) which the caller resolves by asking,
/// never by picking the first.
#[must_use]
pub fn candidates(path: &Path) -> Vec<Companion> {
    if let Some(extension) = path.extension() {
        let extension = extension.to_ascii_lowercase();
        let companions: &[Companion] = match extension.to_str() {
            Some("png" | "jpg" | "jpeg" | "gif" | "bmp") => &[Companion::Frame],
            Some("opml") => &[Companion::Feeds],
            Some("cbz") => &[Companion::Panels],
            Some("z3" | "z4" | "z5" | "z8" | "zcode" | "zblorb" | "ulx" | "gblorb") => {
                &[Companion::Parser]
            }
            Some("apkg" | "colpkg") => &[Companion::Flashcards],
            // `.kepub.epub` ends in `.epub` too, and goes the same way: Books
            // lists it and the Kobo's own reader opens it.
            Some("epub" | "txt" | "html" | "htm" | "xhtml") => &[Companion::Books],
            Some("md" | "markdown") => &[Companion::Vault],
            Some("pdf") => &PDF,
            _ => &[],
        };
        if !companions.is_empty() {
            return companions.to_vec();
        }
    }
    magic_candidates(path)
}

/// The apps a PDF could be for. Music Stand and Needles each keep it and draw
/// its pages; Books lists it for the Kobo's own reader.
const PDF: [Companion; 3] = [Companion::MusicStand, Companion::Needles, Companion::Books];

/// The companions a folder could go to, from what it holds.
///
/// Notes make it a Vault folder, even with pictures or books beside them,
/// because an Obsidian vault keeps its attachments there. PDFs make it a set
/// of scores. EPUBs with no notes make it a folder of books, and with PDFs
/// beside them it is a question for the owner, as notes and PDFs are.
/// Pictures with none of those make it an album. Text and HTML files decide
/// only when nothing else does: a stray `README.txt` in an album or beside a
/// set of scores is not a library. Only the first few hundred entries are
/// looked at: enough to tell what a folder is, without walking a whole home
/// directory.
#[must_use]
pub fn folder_candidates(folder: &Path) -> Vec<Companion> {
    let (mut notes, mut scores, mut pictures) = (false, false, false);
    let (mut books, mut texts) = (false, false);
    let mut pending = vec![folder.to_path_buf()];
    let mut looked = 0;
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            looked += 1;
            if looked > 500 {
                break;
            }
            let path = entry.path();
            let hidden = entry.file_name().to_string_lossy().starts_with('.');
            if hidden {
                continue;
            }
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            match candidates(&path).as_slice() {
                [Companion::Vault] => notes = true,
                [Companion::Frame] => pictures = true,
                [Companion::MusicStand, Companion::Needles, Companion::Books] => scores = true,
                [Companion::Books] if is_page(&path) => texts = true,
                [Companion::Books] => books = true,
                _ => {}
            }
        }
    }
    let mut found = Vec::new();
    if notes {
        found.push(Companion::Vault);
    }
    if scores {
        found.push(Companion::MusicStand);
    }
    if books && !notes {
        found.push(Companion::Books);
    }
    if pictures && found.is_empty() {
        found.push(Companion::Frame);
    }
    if texts && found.is_empty() {
        found.push(Companion::Books);
    }
    found
}

/// Whether a file Books takes is a text or HTML page rather than a book.
///
/// Anything else that reached Books is an EPUB, by its name or by its bytes,
/// and an EPUB says what a folder is in a way a text file cannot.
fn is_page(path: &Path) -> bool {
    path.extension().is_some_and(|extension| {
        ["txt", "html", "htm", "xhtml"]
            .iter()
            .any(|page| extension.eq_ignore_ascii_case(page))
    })
}

/// What the first bytes say, when the name did not.
fn magic_candidates(path: &Path) -> Vec<Companion> {
    let mut head = [0u8; 58];
    let read = std::fs::File::open(path)
        .and_then(|mut file| {
            use std::io::Read as _;
            file.read(&mut head)
        })
        .unwrap_or(0);
    let head = &head[..read];
    if head.starts_with(b"\x89PNG")
        || head.starts_with(b"\xff\xd8")
        || head.starts_with(b"GIF8")
        || head.starts_with(b"BM")
    {
        vec![Companion::Frame]
    } else if head.starts_with(b"SQLite format 3\0") {
        vec![Companion::Flashcards]
    } else if head.starts_with(b"Glul") {
        vec![Companion::Parser]
    } else if head.starts_with(b"%PDF-") {
        PDF.to_vec()
    } else if head.starts_with(b"PK\x03\x04") {
        if is_epub(head) {
            vec![Companion::Books]
        } else {
            // A zip container: comic archives and deck packages are both zips.
            vec![Companion::Panels, Companion::Flashcards]
        }
    } else {
        Vec::new()
    }
}

/// An EPUB is a zip whose first entry is an uncompressed `mimetype` file
/// holding exactly `application/epub+zip`, both at fixed offsets.
pub(crate) fn is_epub(head: &[u8]) -> bool {
    head.len() >= 58 && &head[30..38] == b"mimetype" && &head[38..58] == b"application/epub+zip"
}

/// The choices for a file several companions read, one line each, ready to
/// print under the question.
#[must_use]
pub fn choices(offered: &[Companion]) -> String {
    let width = offered
        .iter()
        .map(|companion| companion.name().len())
        .max()
        .unwrap_or(0);
    offered
        .iter()
        .map(|companion| format!("  --to {:width$}  {}", companion.name(), companion.offer()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Resolves candidates to the one companion, honoring `--to`.
///
/// A named companion goes even when detection ruled it out: `--to` is the
/// owner overriding detection, and the companion's own check still refuses
/// a file it cannot take. A name send has never heard of is a usage error
/// naming the companions there are. Several candidates with no `--to` is the
/// caller's cue to ask; this returns the candidates for that.
pub fn choose(candidates: &[Companion], app: Option<&str>) -> Result<Companion, ChooseError> {
    if let Some(app) = app {
        return match Companion::named(app) {
            Some(companion) => Ok(companion),
            None => Err(ChooseError::Message(crate::console::usage(format!(
                "kobo send FILE --to APP\nNo app called {app} takes files from send. These do: {}.",
                Companion::ALL
                    .iter()
                    .map(|companion| companion.name())
                    .collect::<Vec<_>>()
                    .join(", ")
            )))),
        };
    }
    match candidates {
        [only] => Ok(*only),
        [] => Err(ChooseError::Message(crate::console::unsupported(
            "No app opens this kind of file. Send understands EPUB books, text and HTML \
             documents, photos, PDFs, Markdown notes, OPML subscription lists, CBZ comics, \
             story files and APKG or COLPKG decks. If you know which app should open it, \
             name it with --to APP.",
        ))),
        several => Err(ChooseError::Ambiguous(several.to_vec())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(name)
    }

    #[test]
    fn extensions_route_to_their_companions() {
        assert_eq!(candidates(&named_path("beach.PNG")), [Companion::Frame]);
        assert_eq!(candidates(&named_path("subs.opml")), [Companion::Feeds]);
        assert_eq!(candidates(&named_path("annual.cbz")), [Companion::Panels]);
        assert_eq!(candidates(&named_path("curses.z5")), [Companion::Parser]);
        assert_eq!(
            candidates(&named_path("deck.apkg")),
            [Companion::Flashcards]
        );
        assert_eq!(
            candidates(&named_path("deck.colpkg")),
            [Companion::Flashcards]
        );
        assert_eq!(candidates(&named_path("groceries.md")), [Companion::Vault]);
        assert_eq!(
            candidates(&named_path("Notes.Markdown")),
            [Companion::Vault]
        );
    }

    #[test]
    fn a_book_or_a_plain_document_goes_to_books_and_fanshelf_no_longer_claims_it() {
        for name in [
            "novel.epub",
            "Novel.EPUB",
            "purchase.kepub.epub",
            "letter.txt",
            "streets.html",
            "streets.htm",
            "chapter.xhtml",
        ] {
            assert_eq!(candidates(&named_path(name)), [Companion::Books], "{name}");
        }
        assert!(matches!(
            choose(&candidates(&named_path("novel.epub")), None),
            Ok(Companion::Books)
        ));
        assert!(Companion::Books.takes_folders());
        assert_eq!(Companion::Books.replaces(), None);
    }

    #[test]
    fn a_pdf_is_a_score_a_pattern_or_a_document_and_the_owner_is_asked_which() {
        assert_eq!(
            candidates(&named_path("nocturne.pdf")),
            [Companion::MusicStand, Companion::Needles, Companion::Books]
        );
        match choose(&candidates(&named_path("nocturne.pdf")), None) {
            Err(ChooseError::Ambiguous(offered)) => {
                let listed = choices(&offered);
                assert!(listed.contains("--to musicstand"), "{listed}");
                assert!(listed.contains("Music Stand, for sheet music"), "{listed}");
                assert!(listed.contains("--to needles"), "{listed}");
                assert!(
                    listed.contains("Needles, for knitting patterns"),
                    "{listed}"
                );
                assert!(listed.contains("--to books"), "{listed}");
                assert!(
                    listed.contains("Books lists a PDF and the Kobo's own reader opens it"),
                    "the Books choice has to say who turns the pages: {listed}"
                );
            }
            other => panic!("a PDF must not be guessed, got {other:?}"),
        }
    }

    #[test]
    fn a_markdown_note_still_goes_to_vault_and_to_books_only_when_asked() {
        assert_eq!(candidates(&named_path("note.md")), [Companion::Vault]);
        assert!(matches!(
            choose(&candidates(&named_path("note.md")), Some("books")),
            Ok(Companion::Books)
        ));
        assert!(matches!(
            choose(&candidates(&named_path("nocturne.pdf")), Some("Books")),
            Ok(Companion::Books)
        ));
    }

    #[test]
    fn a_markdown_note_never_goes_to_needles_unless_asked() {
        assert!(!candidates(&named_path("note.md")).contains(&Companion::Needles));
        assert!(matches!(
            choose(&candidates(&named_path("note.md")), Some("needles")),
            Ok(Companion::Needles)
        ));
    }

    #[test]
    fn a_file_nothing_opens_is_not_guessed_at() {
        assert!(candidates(&named_path("kobo-detect-unknown.docx")).is_empty());
    }

    #[test]
    fn an_app_can_be_named_the_way_the_reader_shows_it() {
        for spelling in ["musicstand", "Music Stand", "music-stand", "MUSICSTAND"] {
            assert_eq!(
                Companion::named(spelling),
                Some(Companion::MusicStand),
                "{spelling}"
            );
        }
        assert_eq!(Companion::named("Vault"), Some(Companion::Vault));
        assert_eq!(Companion::named("kindle"), None);
    }

    #[test]
    fn a_folder_goes_to_the_app_whose_files_it_holds() {
        let root = std::env::temp_dir().join(format!("kobo-detect-folders-{}", std::process::id()));
        let make = |name: &str, files: &[&str]| {
            let folder = root.join(name);
            std::fs::create_dir_all(folder.join("inner")).expect("folder");
            for file in files {
                std::fs::write(folder.join(file), b"x").expect("file");
            }
            folder
        };
        let album = make("album", &["a.jpg", "inner/b.png"]);
        assert_eq!(folder_candidates(&album), [Companion::Frame]);
        let vault = make("vault", &["today.md", "inner/diagram.png"]);
        assert_eq!(folder_candidates(&vault), [Companion::Vault]);
        let scores = make("scores", &["nocturne.pdf", "inner/etude.pdf"]);
        assert_eq!(folder_candidates(&scores), [Companion::MusicStand]);
        let mixed = make("mixed", &["notes.md", "score.pdf"]);
        assert_eq!(
            folder_candidates(&mixed),
            [Companion::Vault, Companion::MusicStand]
        );
        let empty = make("empty", &[]);
        assert!(folder_candidates(&empty).is_empty());
        assert!(Companion::Vault.takes_folders());
        assert!(!Companion::Needles.takes_folders());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_folder_of_books_goes_to_books_without_taking_folders_other_apps_had() {
        let root = std::env::temp_dir().join(format!("kobo-detect-books-{}", std::process::id()));
        let make = |name: &str, files: &[&str]| {
            let folder = root.join(name);
            std::fs::create_dir_all(folder.join("inner")).expect("folder");
            for file in files {
                std::fs::write(folder.join(file), b"x").expect("file");
            }
            folder
        };
        let shelf = make(
            "shelf",
            &["novel.epub", "inner/sequel.kepub.epub", "cover.jpg"],
        );
        assert_eq!(folder_candidates(&shelf), [Companion::Books]);
        let pages = make("pages", &["letter.txt", "inner/streets.html"]);
        assert_eq!(folder_candidates(&pages), [Companion::Books]);
        // A book beside PDFs could be a library or a set of scores.
        let library = make("library", &["novel.epub", "manual.pdf"]);
        assert_eq!(
            folder_candidates(&library),
            [Companion::MusicStand, Companion::Books]
        );
        // What these folders were before a text file sat in them is what they
        // still are: a stray README is not a library.
        let album = make("album", &["a.jpg", "README.txt"]);
        assert_eq!(folder_candidates(&album), [Companion::Frame]);
        let scores = make("scores", &["nocturne.pdf", "setlist.txt"]);
        assert_eq!(folder_candidates(&scores), [Companion::MusicStand]);
        let vault = make("vault", &["today.md", "inner/attached.epub", "export.html"]);
        assert_eq!(folder_candidates(&vault), [Companion::Vault]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_apps_that_keep_one_thing_say_what_a_send_replaces() {
        assert_eq!(Companion::Needles.replaces(), None);
        assert_eq!(Companion::Panels.replaces(), Some("comic"));
        assert_eq!(Companion::MusicStand.replaces(), None);
        assert_eq!(Companion::Frame.replaces(), None);
    }

    #[test]
    fn an_unknown_extension_falls_back_to_magic_bytes() {
        let photo = named_path("kobo-detect-photo");
        std::fs::write(&photo, b"\x89PNG\r\n\x1a\nrest").expect("a photo");
        assert_eq!(candidates(&photo), [Companion::Frame]);
        let pattern = named_path("kobo-detect-pattern");
        std::fs::write(&pattern, b"%PDF-1.4 rest").expect("a pattern");
        assert_eq!(
            candidates(&pattern),
            [Companion::MusicStand, Companion::Needles, Companion::Books]
        );
    }

    #[test]
    fn an_epub_zip_is_a_book_not_a_comic_or_deck() {
        let mut bytes = b"PK\x03\x04".to_vec();
        bytes.resize(30, 0);
        bytes.extend_from_slice(b"mimetype");
        bytes.extend_from_slice(b"application/epub+zip");
        let book = named_path("kobo-detect-book");
        std::fs::write(&book, &bytes).expect("a book");
        assert_eq!(candidates(&book), [Companion::Books]);
        // The same container without the EPUB mimetype stays ambiguous.
        let mut zip = b"PK\x03\x04".to_vec();
        zip.resize(64, 0);
        let archive = named_path("kobo-detect-archive");
        std::fs::write(&archive, &zip).expect("an archive");
        assert_eq!(
            candidates(&archive),
            [Companion::Panels, Companion::Flashcards]
        );
    }

    #[test]
    fn one_candidate_is_the_answer() {
        let chosen = choose(&[Companion::Frame], None);
        assert!(matches!(chosen, Ok(Companion::Frame)));
    }

    #[test]
    fn several_candidates_are_the_callers_cue_to_ask() {
        let chosen = choose(&[Companion::Panels, Companion::Flashcards], None);
        match chosen {
            Err(ChooseError::Ambiguous(offered)) => {
                assert_eq!(offered, [Companion::Panels, Companion::Flashcards]);
            }
            other => panic!("expected the offered companions, got {other:?}"),
        }
    }

    #[test]
    fn no_candidates_is_plainly_unsupported() {
        match choose(&[], None) {
            Err(ChooseError::Message(error)) => {
                assert_eq!(
                    crate::console::category_of(&error),
                    crate::console::EXIT_UNSUPPORTED
                );
            }
            other => panic!("expected a tagged unsupported error, got {other:?}"),
        }
    }

    #[test]
    fn an_explicit_app_overrides_detection() {
        let chosen = choose(&[Companion::Frame], Some("feeds"));
        assert!(matches!(chosen, Ok(Companion::Feeds)));
        let chosen = choose(&[], Some("needles"));
        assert!(matches!(chosen, Ok(Companion::Needles)));
    }

    #[test]
    fn an_unknown_app_names_the_companions_there_are() {
        match choose(&[Companion::Frame], Some("kindle")) {
            Err(ChooseError::Message(error)) => {
                assert_eq!(
                    crate::console::category_of(&error),
                    crate::console::EXIT_USAGE
                );
                assert!(error.contains("No app called kindle"));
                assert!(error.contains("fanshelf"));
                assert!(error.contains("musicstand"));
            }
            other => panic!("expected a tagged usage error, got {other:?}"),
        }
    }
}

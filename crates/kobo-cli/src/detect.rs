//! Which companion a file belongs to, chosen by what the file is.
//!
//! A photo goes to Frame, a subscription list to Feeds, a comic archive to
//! Panels, a story file to Parser, a study deck to Flashcards, an ebook to
//! Fanshelf, a knitting pattern to Needles. The owner names the file; this
//! module names the companion. Detection reads the extension first and the
//! file's own magic bytes when the extension says nothing, and it says so
//! plainly when more than one companion could take the file - `send` asks,
//! or `--app` answers, but nothing guesses.
//!
//! Extensions decide by convention; magic bytes decide by content. Neither
//! is a validation: the companion's own check still accepts or refuses the
//! file before anything is sent. An explicit `--app` is the owner
//! overriding detection, so a name send knows always wins over what the
//! file looks like.

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
}

impl Companion {
    /// Every companion, in the order help and errors list them.
    pub const ALL: [Self; 7] = [
        Self::Frame,
        Self::Feeds,
        Self::Panels,
        Self::Parser,
        Self::Flashcards,
        Self::Fanshelf,
        Self::Needles,
    ];

    /// The name the owner types after `--app`.
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
        }
    }

    /// The companion behind a typed name, when there is one.
    #[must_use]
    pub fn named(word: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|companion| companion.name() == word)
    }
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
/// container more than one companion reads - a zip with no extension could
/// be a comic archive or a deck package - which the caller resolves by
/// asking, never by picking the first.
#[must_use]
pub fn candidates(path: &Path) -> Vec<Companion> {
    if let Some(extension) = path.extension() {
        let extension = extension.to_ascii_lowercase();
        let companion = match extension.to_str() {
            Some("png" | "jpg" | "jpeg" | "gif" | "bmp") => Some(Companion::Frame),
            Some("opml") => Some(Companion::Feeds),
            Some("cbz") => Some(Companion::Panels),
            Some("z3" | "z4" | "z5" | "z8" | "zcode" | "zblorb" | "ulx" | "gblorb") => {
                Some(Companion::Parser)
            }
            Some("apkg" | "colpkg") => Some(Companion::Flashcards),
            Some("epub") => Some(Companion::Fanshelf),
            Some("md" | "markdown" | "txt" | "pdf") => Some(Companion::Needles),
            _ => None,
        };
        if let Some(companion) = companion {
            return vec![companion];
        }
    }
    magic_candidates(path)
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
        vec![Companion::Needles]
    } else if head.starts_with(b"PK\x03\x04") {
        if is_epub(head) {
            vec![Companion::Fanshelf]
        } else {
            // A zip container: comic archives and deck packages are both zips.
            vec![Companion::Panels, Companion::Flashcards]
        }
    } else {
        Vec::new()
    }
}

/// An EPUB is a zip whose first entry is an uncompressed `mimetype` file
/// holding exactly `application/epub+zip` - both at fixed offsets.
fn is_epub(head: &[u8]) -> bool {
    head.len() >= 58 && &head[30..38] == b"mimetype" && &head[38..58] == b"application/epub+zip"
}

/// Resolves candidates to the one companion, honoring `--app`.
///
/// A named companion goes even when detection ruled it out: `--app` is the
/// owner overriding detection, and the companion's own check still refuses
/// a file it cannot take. A name send has never heard of is a usage error
/// naming the companions there are. Several candidates with no `--app` is
/// the caller's cue to ask; this returns the candidates for that.
pub fn choose(candidates: &[Companion], app: Option<&str>) -> Result<Companion, ChooseError> {
    if let Some(app) = app {
        return match Companion::named(app) {
            Some(companion) => Ok(companion),
            None => Err(ChooseError::Message(crate::console::usage(format!(
                "no companion named {app}; companions are: {}",
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
            "no companion knows this file; photos, OPML subscription lists, CBZ comics, story files, EPUB books, Needles patterns and APKG/COLPKG decks are understood",
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
        assert_eq!(candidates(&named_path("novel.epub")), [Companion::Fanshelf]);
        assert_eq!(candidates(&named_path("scarf.md")), [Companion::Needles]);
        assert_eq!(candidates(&named_path("scarf.pdf")), [Companion::Needles]);
        assert_eq!(candidates(&named_path("scarf.txt")), [Companion::Needles]);
    }

    #[test]
    fn an_unknown_extension_falls_back_to_magic_bytes() {
        let photo = named_path("kobo-detect-photo");
        std::fs::write(&photo, b"\x89PNG\r\n\x1a\nrest").expect("a photo");
        assert_eq!(candidates(&photo), [Companion::Frame]);
        let pattern = named_path("kobo-detect-pattern");
        std::fs::write(&pattern, b"%PDF-1.4 rest").expect("a pattern");
        assert_eq!(candidates(&pattern), [Companion::Needles]);
    }

    #[test]
    fn an_epub_zip_is_a_book_not_a_comic_or_deck() {
        let mut bytes = b"PK\x03\x04".to_vec();
        bytes.resize(30, 0);
        bytes.extend_from_slice(b"mimetype");
        bytes.extend_from_slice(b"application/epub+zip");
        let book = named_path("kobo-detect-book");
        std::fs::write(&book, &bytes).expect("a book");
        assert_eq!(candidates(&book), [Companion::Fanshelf]);
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
                assert!(error.contains("no companion named kindle"));
                assert!(error.contains("fanshelf"));
            }
            other => panic!("expected a tagged usage error, got {other:?}"),
        }
    }
}

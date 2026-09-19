//! Which companion a file belongs to, chosen by what the file is.
//!
//! A photo goes to Frame, a subscription list to Feeds, a comic archive to
//! Panels, a story file to Parser, a study deck to Flashcards. The owner
//! names the file; this module names the companion. Detection reads the
//! extension first and the file's own magic bytes when the extension says
//! nothing, and it says so plainly when more than one companion could take
//! the file - `send` asks, or `--app` answers, but nothing guesses.
//!
//! Extensions decide by convention; magic bytes decide by content. Neither
//! is a validation: the companion's own check still accepts or refuses the
//! file before anything is sent.

use std::path::Path;

/// Every companion that could take `path`, most specific first.
///
/// Empty means nothing we know wants it. More than one means the file is a
/// container more than one companion reads - a zip with no extension could
/// be a comic archive or a deck package - which the caller resolves by
/// asking, never by picking the first.
#[must_use]
pub fn candidates(path: &Path) -> Vec<&'static str> {
    if let Some(extension) = path.extension() {
        let extension = extension.to_ascii_lowercase();
        let app = match extension.to_str() {
            Some("png" | "jpg" | "jpeg" | "gif" | "bmp") => Some("frame"),
            Some("opml") => Some("feeds"),
            Some("cbz") => Some("panels"),
            Some("z3" | "z4" | "z5" | "z8" | "zcode" | "zblorb" | "ulx" | "gblorb") => {
                Some("parser")
            }
            Some("apkg" | "colpkg") => Some("flashcards"),
            _ => None,
        };
        if let Some(app) = app {
            return vec![app];
        }
    }
    magic_candidates(path)
}

/// What the first bytes say, when the name did not.
fn magic_candidates(path: &Path) -> Vec<&'static str> {
    let mut head = [0u8; 16];
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
        vec!["frame"]
    } else if head.starts_with(b"SQLite format 3\0") {
        vec!["flashcards"]
    } else if head.starts_with(b"Glul") {
        vec!["parser"]
    } else if head.starts_with(b"PK\x03\x04") {
        // A zip container: comic archives and deck packages are both zips.
        vec!["panels", "flashcards"]
    } else {
        Vec::new()
    }
}

/// Resolves candidates to the one companion, honoring `--app`.
///
/// A named app that detection ruled out is a usage error saying so - sending
/// a photo to Feeds fails later and worse. Several candidates with no `--app`
/// is the caller's cue to ask; this returns the candidates for that.
pub fn choose<'a>(candidates: &'a [&'static str], app: Option<&str>) -> Result<&'a str, String> {
    if let Some(app) = app {
        if candidates.contains(&app) {
            return Ok(candidates
                .iter()
                .find(|candidate| **candidate == app)
                .unwrap());
        }
        return Err(crate::console::usage(format!(
            "--app {app} does not fit this file; it looks like: {}",
            candidates.join(", ")
        )));
    }
    match candidates {
        [only] => Ok(only),
        [] => Err(crate::console::unsupported(
            "no companion knows this file; photos, OPML subscription lists, CBZ comics, story files and APKG/COLPKG decks are understood",
        )),
        several => Err(format!("several companions could take it: {}", several.join(", "))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(name)
    }

    #[test]
    fn extensions_route_to_their_companions() {
        assert_eq!(candidates(&named("beach.PNG")), ["frame"]);
        assert_eq!(candidates(&named("subs.opml")), ["feeds"]);
        assert_eq!(candidates(&named("annual.cbz")), ["panels"]);
        assert_eq!(candidates(&named("curses.z5")), ["parser"]);
        assert_eq!(candidates(&named("deck.apkg")), ["flashcards"]);
        assert_eq!(candidates(&named("deck.colpkg")), ["flashcards"]);
    }

    #[test]
    fn an_unknown_extension_falls_back_to_magic_bytes() {
        let photo = named("kobo-detect-photo");
        std::fs::write(&photo, b"\x89PNG\r\n\x1a\nrest").expect("a photo");
        assert_eq!(candidates(&photo), ["frame"]);
        let deck = named("kobo-detect-deck");
        std::fs::write(&deck, b"SQLite format 3\0rest").expect("a deck");
        assert_eq!(candidates(&deck), ["flashcards"]);
        let container = named("kobo-detect-zip");
        std::fs::write(&container, b"PK\x03\x04rest").expect("a zip");
        assert_eq!(candidates(&container), ["panels", "flashcards"]);
        let _ = std::fs::remove_file(photo);
        let _ = std::fs::remove_file(deck);
        let _ = std::fs::remove_file(container);
    }

    #[test]
    fn nothing_known_is_unsupported_not_a_guess() {
        let error = choose(&[], None).unwrap_err();
        assert!(error.starts_with("unsupported: "), "{error}");
    }

    #[test]
    fn a_named_app_must_fit_the_file() {
        assert_eq!(choose(&["frame"], Some("frame")).unwrap(), "frame");
        let error = choose(&["frame"], Some("feeds")).unwrap_err();
        assert!(error.starts_with("usage: "), "{error}");
        assert!(error.contains("frame"), "{error}");
    }

    #[test]
    fn several_candidates_ask_rather_than_pick() {
        let error = choose(&["panels", "flashcards"], None).unwrap_err();
        assert!(error.contains("panels, flashcards"), "{error}");
        assert_eq!(
            choose(&["panels", "flashcards"], Some("flashcards")).unwrap(),
            "flashcards"
        );
    }
}

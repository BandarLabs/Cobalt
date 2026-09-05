//! Device-side half of the host-prepared transfer format.
//!
//! `SQLite` and archive parsing deliberately live on the computer that owns the
//! original Anki collection. The reader receives only a bounded, verified
//! review library and can therefore remain a static, socket-free binary.

use crate::model::{decode, Library};

const HEADER: &[u8] = b"cobalt-flashcards-transfer-v1\n";

#[derive(Debug, Eq, PartialEq)]
pub enum ImportError {
    NotPrepared,
    Damaged,
    NoCards,
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str(match self {
            Self::NotPrepared => {
                "That package was not produced by a compatible Flashcards host importer."
            }
            Self::Damaged => {
                "The transferred deck is damaged. Keep your Anki collection and try again when compatible import support ships."
            }
            Self::NoCards => "This deck has no reviewable cards.",
        })
    }
}

impl std::error::Error for ImportError {}

#[derive(Debug)]
pub struct ImportedLibrary {
    pub library: Library,
    pub replaces_collection: bool,
}

/// Validates the portable host transfer before it is allowed to replace data.
///
/// A `.colpkg` becomes a whole-library replacement on the host and carries
/// this marker. Ordinary `.apkg` imports retain the existing library and
/// merge cards by their stable Anki card id.
pub fn import(bytes: &[u8], _today: i32) -> Result<ImportedLibrary, ImportError> {
    let body = bytes.strip_prefix(HEADER).ok_or(ImportError::NotPrepared)?;
    let separator = body
        .iter()
        .position(|byte| *byte == b'\n')
        .ok_or(ImportError::Damaged)?;
    let (kind, body) = body.split_at(separator);
    let body = &body[1..];
    let replaces_collection = match kind {
        b"apkg" => false,
        b"colpkg" => true,
        _ => return Err(ImportError::Damaged),
    };
    let library = decode(body).ok_or(ImportError::Damaged)?;
    if library.cards.is_empty() {
        return Err(ImportError::NoCards);
    }
    Ok(ImportedLibrary {
        library,
        replaces_collection,
    })
}

#[cfg(test)]
mod tests {
    use super::{import, ImportError, HEADER};
    use crate::model::{encode, Card, CardState, Library};

    fn library() -> Library {
        Library {
            cards: vec![Card {
                id: 7,
                deck: "Language".into(),
                front: "犬".into(),
                back: "dog".into(),
                last_review_day: 1,
                due_day: 1,
                state: CardState::New,
                reps: 0,
                lapses: 0,
                stability: None,
                difficulty: None,
                media: 0,
            }],
            ..Library::default()
        }
    }

    #[test]
    fn prepared_apkg_library_round_trips_without_sqlite() {
        let mut bytes = HEADER.to_vec();
        bytes.extend_from_slice(b"apkg\n");
        bytes.extend(encode(&library()));
        let imported = import(&bytes, 1).expect("prepared transfer");
        assert!(!imported.replaces_collection);
        assert_eq!(imported.library.cards[0].front, "犬");
    }

    #[test]
    fn rejects_raw_archives_and_damaged_transfers_without_touching_library() {
        assert_eq!(
            import(b"PK\x03\x04", 1).unwrap_err(),
            ImportError::NotPrepared
        );
        let mut bytes = HEADER.to_vec();
        bytes.extend_from_slice(b"apkg\nnot a library");
        assert_eq!(import(&bytes, 1).unwrap_err(), ImportError::Damaged);
    }
}

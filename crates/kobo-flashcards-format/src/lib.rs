#![forbid(unsafe_code)]

//! The bounded, offline bundle consumed by the Flashcards application.
//!
//! This is deliberately not an Anki collection format. Host-side import
//! validates an Anki package and writes this pathless container; the device
//! only accepts a digest-checked manifest plus media addressed by validated
//! names. No HTML, script, path, or URL becomes executable on the reader.

use flate2::{read::ZlibDecoder, write::ZlibEncoder, Compression};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::{Read, Write};
use unicode_normalization::UnicodeNormalization;

pub const MAGIC: [u8; 8] = *b"CBFLASH\0";
pub const VERSION: u16 = 2;
pub const HEADER_BYTES: usize = MAGIC.len() + 2 + 4 + 8 + 4 + 8 + 32;
pub const MAX_BUNDLE_BYTES: u64 = 32 * 1024 * 1024;
pub const MAX_MANIFEST_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_PAYLOAD_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_MEDIA_BYTES: u64 = 4 * 1024 * 1024;
pub const MAX_MEDIA_ENTRIES: usize = 4_096;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BundleManifest {
    pub format_version: u16,
    pub source: Source,
    pub notetypes: Vec<NoteType>,
    pub decks: Vec<Deck>,
    pub deck_configurations: Vec<DeckConfiguration>,
    pub cards: Vec<Card>,
    pub revlog: Vec<ReviewLog>,
    pub media: Vec<Media>,
    pub diagnostics: Vec<Diagnostic>,
}

impl BundleManifest {
    #[must_use]
    pub fn empty(source: Source) -> Self {
        Self {
            format_version: VERSION,
            source,
            notetypes: Vec::new(),
            decks: Vec::new(),
            deck_configurations: Vec::new(),
            cards: Vec::new(),
            revlog: Vec::new(),
            media: Vec::new(),
            diagnostics: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Source {
    pub package_kind: String,
    pub collection_schema: i64,
    pub collection_modified: i64,
    pub note_count: usize,
    pub upstream_anki_revision: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NoteType {
    pub id: i64,
    pub name: String,
    /// The original model JSON is retained so host reconciliation does not
    /// lose scheduling/template fields that the Kobo renderer does not use.
    pub original_json: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Deck {
    pub id: i64,
    pub name: String,
    pub configuration_id: Option<i64>,
    pub original_json: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DeckConfiguration {
    pub id: i64,
    pub name: String,
    pub original_json: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Card {
    pub id: i64,
    pub note_id: i64,
    pub deck_id: i64,
    pub ordinal: i32,
    pub queue: i32,
    pub card_type: i32,
    pub due: i64,
    pub interval: i32,
    pub ease_factor: i32,
    pub repetitions: i32,
    pub lapses: i32,
    pub remaining_steps: i32,
    pub modified: i64,
    pub template_name: String,
    pub front: String,
    pub back: String,
    pub tags: Vec<String>,
    pub media_names: Vec<String>,
    pub attachments: Vec<Attachment>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Attachment {
    pub name: String,
    /// A host-generated PNG for a safe SVG source. The original source is
    /// still retained under `name` for lossless host reconciliation.
    pub rendered_name: Option<String>,
    pub mime: String,
    pub kind: AttachmentKind,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AttachmentKind {
    Image,
    Audio,
    Video,
    Other,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReviewLog {
    pub id: i64,
    pub card_id: i64,
    pub user_sequence: i32,
    pub ease: i32,
    pub interval: i32,
    pub last_interval: i32,
    pub ease_factor: i32,
    pub milliseconds: i32,
    pub review_kind: i32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Media {
    pub name: String,
    pub mime: String,
    pub offset: u64,
    pub length: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Diagnostic {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsedBundle {
    manifest: BundleManifest,
    payload: Vec<u8>,
}

impl ParsedBundle {
    #[must_use]
    pub const fn manifest(&self) -> &BundleManifest {
        &self.manifest
    }

    #[must_use]
    pub fn media(&self, name: &str) -> Option<&[u8]> {
        let name = canonical_media_name(name).ok()?;
        let media = self
            .manifest
            .media
            .iter()
            .find(|media| media.name == name)?;
        let start = usize::try_from(media.offset).ok()?;
        let end = start.checked_add(usize::try_from(media.length).ok()?)?;
        self.payload.get(start..end)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FormatError {
    TooShort,
    BadMagic,
    UnsupportedVersion(u16),
    ManifestTooLarge(usize),
    BundleTooLarge(u64),
    LengthMismatch,
    DigestMismatch,
    Compression(String),
    Json(String),
    InvalidMediaName(String),
    DuplicateMediaName(String),
    MediaLimit,
    MediaOutOfBounds(String),
    MediaDigestMismatch(String),
    NonDeterministicOrder,
}

impl fmt::Display for FormatError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooShort => write!(formatter, "bundle is truncated"),
            Self::BadMagic => write!(formatter, "not a Flashcards bundle"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported bundle version {version}")
            }
            Self::ManifestTooLarge(length) => {
                write!(formatter, "manifest is {length} bytes, above the limit")
            }
            Self::BundleTooLarge(length) => {
                write!(formatter, "bundle is {length} bytes, above the limit")
            }
            Self::LengthMismatch => write!(formatter, "bundle length does not match its header"),
            Self::DigestMismatch => write!(formatter, "bundle digest does not match its content"),
            Self::Compression(error) => {
                write!(formatter, "invalid compressed bundle content: {error}")
            }
            Self::Json(error) => write!(formatter, "invalid bundle manifest: {error}"),
            Self::InvalidMediaName(name) => write!(formatter, "invalid media name {name:?}"),
            Self::DuplicateMediaName(name) => write!(formatter, "duplicate media name {name:?}"),
            Self::MediaLimit => write!(formatter, "bundle has too many or too-large media files"),
            Self::MediaOutOfBounds(name) => {
                write!(formatter, "media {name:?} is outside the payload")
            }
            Self::MediaDigestMismatch(name) => {
                write!(formatter, "media {name:?} digest does not match")
            }
            Self::NonDeterministicOrder => {
                write!(formatter, "media entries are not in canonical order")
            }
        }
    }
}

impl std::error::Error for FormatError {}

/// Produces a stable, digest-protected bundle. `media` is keyed by the
/// original media name, but the output names are NFC-normalized and sorted.
///
/// # Errors
///
/// Returns an error when a media name, a size limit, or the manifest cannot be
/// represented safely.
pub fn encode(
    mut manifest: BundleManifest,
    media: BTreeMap<String, Vec<u8>>,
) -> Result<Vec<u8>, FormatError> {
    if media.len() > MAX_MEDIA_ENTRIES {
        return Err(FormatError::MediaLimit);
    }
    let mut normalized = BTreeMap::new();
    for (raw_name, bytes) in media {
        let name = canonical_media_name(&raw_name)?;
        if bytes.len() as u64 > MAX_MEDIA_BYTES {
            return Err(FormatError::MediaLimit);
        }
        if normalized.insert(name.clone(), bytes).is_some() {
            return Err(FormatError::DuplicateMediaName(name));
        }
    }
    let mut payload = Vec::new();
    let mut records = Vec::with_capacity(normalized.len());
    for (name, bytes) in normalized {
        let offset = u64::try_from(payload.len()).map_err(|_| FormatError::MediaLimit)?;
        let length = u64::try_from(bytes.len()).map_err(|_| FormatError::MediaLimit)?;
        let mime = media_type(&name).to_owned();
        records.push(Media {
            name,
            mime,
            offset,
            length,
            sha256: digest_hex(&bytes),
        });
        payload.extend_from_slice(&bytes);
    }
    manifest.format_version = VERSION;
    manifest.media = records;
    let manifest_bytes =
        serde_json::to_vec(&manifest).map_err(|error| FormatError::Json(error.to_string()))?;
    if manifest_bytes.len() > MAX_MANIFEST_BYTES {
        return Err(FormatError::ManifestTooLarge(manifest_bytes.len()));
    }
    if payload.len() > MAX_PAYLOAD_BYTES {
        return Err(FormatError::MediaLimit);
    }
    let compressed_manifest = compress(&manifest_bytes)?;
    let compressed_payload = compress(&payload)?;
    let total = HEADER_BYTES
        .checked_add(compressed_manifest.len())
        .and_then(|length| length.checked_add(compressed_payload.len()))
        .ok_or(FormatError::MediaLimit)?;
    let total_u64 = u64::try_from(total).map_err(|_| FormatError::MediaLimit)?;
    if total_u64 > MAX_BUNDLE_BYTES {
        return Err(FormatError::BundleTooLarge(total_u64));
    }
    let mut bytes = Vec::with_capacity(total);
    bytes.extend_from_slice(&MAGIC);
    bytes.extend_from_slice(&VERSION.to_le_bytes());
    bytes.extend_from_slice(
        &u32::try_from(compressed_manifest.len())
            .map_err(|_| FormatError::ManifestTooLarge(compressed_manifest.len()))?
            .to_le_bytes(),
    );
    bytes.extend_from_slice(
        &u64::try_from(compressed_payload.len())
            .map_err(|_| FormatError::MediaLimit)?
            .to_le_bytes(),
    );
    bytes.extend_from_slice(
        &u32::try_from(manifest_bytes.len())
            .map_err(|_| FormatError::ManifestTooLarge(manifest_bytes.len()))?
            .to_le_bytes(),
    );
    bytes.extend_from_slice(
        &u64::try_from(payload.len())
            .map_err(|_| FormatError::MediaLimit)?
            .to_le_bytes(),
    );
    let mut hasher = Sha256::new();
    hasher.update(&manifest_bytes);
    hasher.update(&payload);
    bytes.extend_from_slice(&hasher.finalize());
    bytes.extend_from_slice(&compressed_manifest);
    bytes.extend_from_slice(&compressed_payload);
    Ok(bytes)
}

/// Parses only fully verified bundles. The returned type never exposes a path.
///
/// # Errors
///
/// Returns an error when a header, compressed section, digest, manifest, or
/// media index is malformed or exceeds its bound.
pub fn decode(bytes: &[u8]) -> Result<ParsedBundle, FormatError> {
    if bytes.len() < HEADER_BYTES {
        return Err(FormatError::TooShort);
    }
    if bytes[..MAGIC.len()] != MAGIC {
        return Err(FormatError::BadMagic);
    }
    let version = u16::from_le_bytes([bytes[8], bytes[9]]);
    if version != VERSION {
        return Err(FormatError::UnsupportedVersion(version));
    }
    let compressed_manifest_length =
        u32::from_le_bytes([bytes[10], bytes[11], bytes[12], bytes[13]]) as usize;
    let compressed_payload_length = u64::from_le_bytes(
        bytes[14..22]
            .try_into()
            .map_err(|_| FormatError::TooShort)?,
    );
    let manifest_length = u32::from_le_bytes(
        bytes[22..26]
            .try_into()
            .map_err(|_| FormatError::TooShort)?,
    ) as usize;
    let payload_length = u64::from_le_bytes(
        bytes[26..34]
            .try_into()
            .map_err(|_| FormatError::TooShort)?,
    );
    if manifest_length > MAX_MANIFEST_BYTES {
        return Err(FormatError::ManifestTooLarge(manifest_length));
    }
    if payload_length > MAX_PAYLOAD_BYTES as u64 {
        return Err(FormatError::MediaLimit);
    }
    let expected = HEADER_BYTES
        .checked_add(compressed_manifest_length)
        .and_then(|length| length.checked_add(usize::try_from(compressed_payload_length).ok()?))
        .ok_or(FormatError::LengthMismatch)?;
    if expected != bytes.len() {
        return Err(FormatError::LengthMismatch);
    }
    if bytes.len() as u64 > MAX_BUNDLE_BYTES {
        return Err(FormatError::BundleTooLarge(bytes.len() as u64));
    }
    let payload_offset = HEADER_BYTES + compressed_manifest_length;
    let manifest_bytes = decompress(
        &bytes[HEADER_BYTES..payload_offset],
        manifest_length,
        "manifest",
    )?;
    let payload = decompress(
        &bytes[payload_offset..],
        usize::try_from(payload_length).map_err(|_| FormatError::MediaLimit)?,
        "media payload",
    )?;
    let mut hasher = Sha256::new();
    hasher.update(&manifest_bytes);
    hasher.update(&payload);
    let calculated = hasher.finalize();
    if calculated[..] != bytes[34..66] {
        return Err(FormatError::DigestMismatch);
    }
    let manifest: BundleManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|error| FormatError::Json(error.to_string()))?;
    if manifest.format_version != VERSION {
        return Err(FormatError::UnsupportedVersion(manifest.format_version));
    }
    validate_media(&manifest.media, &payload)?;
    Ok(ParsedBundle { manifest, payload })
}

fn compress(bytes: &[u8]) -> Result<Vec<u8>, FormatError> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
    encoder
        .write_all(bytes)
        .map_err(|error| FormatError::Compression(error.to_string()))?;
    encoder
        .finish()
        .map_err(|error| FormatError::Compression(error.to_string()))
}

fn decompress(bytes: &[u8], expected: usize, label: &str) -> Result<Vec<u8>, FormatError> {
    let mut reader = ZlibDecoder::new(bytes);
    let mut output = Vec::with_capacity(expected);
    reader
        .by_ref()
        .take(
            u64::try_from(expected)
                .unwrap_or(u64::MAX)
                .saturating_add(1),
        )
        .read_to_end(&mut output)
        .map_err(|error| FormatError::Compression(format!("{label}: {error}")))?;
    if output.len() != expected {
        return Err(FormatError::LengthMismatch);
    }
    Ok(output)
}

/// NFC-normalizes an Anki media name and rejects every path-like spelling.
///
/// # Errors
///
/// Returns an error when `name` is empty or contains a path component or
/// control byte. A valid result is NFC-normalized.
pub fn canonical_media_name(name: &str) -> Result<String, FormatError> {
    let normalized = name.nfc().collect::<String>();
    if normalized.is_empty()
        || normalized.len() > 255
        || normalized == "."
        || normalized == ".."
        || normalized.contains('/')
        || normalized.contains('\\')
        || normalized
            .bytes()
            .any(|byte| byte == 0 || byte.is_ascii_control())
    {
        return Err(FormatError::InvalidMediaName(name.to_owned()));
    }
    Ok(normalized)
}

#[must_use]
pub fn media_type(name: &str) -> &'static str {
    match name
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
    {
        Some(extension) if matches!(extension.as_str(), "jpg" | "jpeg") => "image/jpeg",
        Some(extension) if extension == "png" => "image/png",
        Some(extension) if extension == "gif" => "image/gif",
        Some(extension) if extension == "webp" => "image/webp",
        Some(extension) if extension == "svg" => "image/svg+xml",
        Some(extension) if matches!(extension.as_str(), "mp3" | "ogg" | "wav" | "m4a") => "audio/*",
        Some(extension) if matches!(extension.as_str(), "mp4" | "webm") => "video/*",
        _ => "application/octet-stream",
    }
}

#[must_use]
pub fn digest_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ignored = write!(encoded, "{byte:02x}");
    }
    encoded
}

fn validate_media(media: &[Media], payload: &[u8]) -> Result<(), FormatError> {
    if media.len() > MAX_MEDIA_ENTRIES {
        return Err(FormatError::MediaLimit);
    }
    let mut names = BTreeSet::new();
    let mut previous = None;
    for record in media {
        let name = canonical_media_name(&record.name)?;
        if name != record.name {
            return Err(FormatError::InvalidMediaName(record.name.clone()));
        }
        if !names.insert(name.clone()) {
            return Err(FormatError::DuplicateMediaName(name));
        }
        if previous
            .as_ref()
            .is_some_and(|prior: &String| prior >= &name)
        {
            return Err(FormatError::NonDeterministicOrder);
        }
        previous = Some(name.clone());
        if record.length > MAX_MEDIA_BYTES {
            return Err(FormatError::MediaLimit);
        }
        let start = usize::try_from(record.offset)
            .map_err(|_| FormatError::MediaOutOfBounds(name.clone()))?;
        let end = start
            .checked_add(
                usize::try_from(record.length)
                    .map_err(|_| FormatError::MediaOutOfBounds(name.clone()))?,
            )
            .ok_or_else(|| FormatError::MediaOutOfBounds(name.clone()))?;
        let content = payload
            .get(start..end)
            .ok_or_else(|| FormatError::MediaOutOfBounds(name.clone()))?;
        if digest_hex(content) != record.sha256 {
            return Err(FormatError::MediaDigestMismatch(name));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> Source {
        Source {
            package_kind: "apkg".to_owned(),
            collection_schema: 11,
            collection_modified: 1,
            note_count: 0,
            upstream_anki_revision: "pinned".to_owned(),
        }
    }

    #[test]
    fn bundle_round_trip_is_digest_checked_and_deterministic() {
        let mut media = BTreeMap::new();
        media.insert("two.png".to_owned(), b"two".to_vec());
        media.insert("one.png".to_owned(), b"one".to_vec());
        let first = encode(BundleManifest::empty(source()), media.clone()).expect("encode");
        let second = encode(BundleManifest::empty(source()), media).expect("encode");
        assert_eq!(first, second);
        let parsed = decode(&first).expect("decode");
        assert_eq!(parsed.media("one.png"), Some(&b"one"[..]));
        assert_eq!(parsed.media("two.png"), Some(&b"two"[..]));
    }

    #[test]
    fn path_and_normalization_conflicts_are_rejected() {
        for name in [
            "../escape.png",
            "/absolute.png",
            "a/b.png",
            "a\\b.png",
            "\0.png",
        ] {
            assert!(canonical_media_name(name).is_err(), "{name:?}");
        }
        let mut media = BTreeMap::new();
        media.insert("e\u{301}.png".to_owned(), b"first".to_vec());
        media.insert("é.png".to_owned(), b"second".to_vec());
        assert!(matches!(
            encode(BundleManifest::empty(source()), media),
            Err(FormatError::DuplicateMediaName(_))
        ));
    }

    #[test]
    fn corrupt_and_noncanonical_bundles_are_refused() {
        let encoded = encode(BundleManifest::empty(source()), BTreeMap::new()).expect("encode");
        assert!(matches!(decode(&encoded[..20]), Err(FormatError::TooShort)));
        let mut tampered = encoded;
        tampered[34] ^= 1;
        assert_eq!(decode(&tampered), Err(FormatError::DigestMismatch));
    }
}

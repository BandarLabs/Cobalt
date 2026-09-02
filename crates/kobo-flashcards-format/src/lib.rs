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

mod svg;
pub use svg::{rasterize_svg, validate_svg_source};

pub const MAGIC: [u8; 8] = *b"CBFLASH\0";
pub const VERSION: u16 = 3;
pub const HEADER_BYTES: usize = MAGIC.len() + 2 + 4 + 8 + 4 + 8 + 32;
pub const MAX_BUNDLE_BYTES: u64 = 32 * 1024 * 1024;
pub const MAX_MANIFEST_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_PAYLOAD_BYTES: usize = 48 * 1024 * 1024;
pub const MAX_MEDIA_BYTES: u64 = 4 * 1024 * 1024;
pub const MAX_MEDIA_ENTRIES: usize = 8_192;
pub const MAX_NOTES: usize = 100_000;
pub const MAX_CARDS: usize = 100_000;
pub const MAX_REVIEW_QUEUE_CARDS: usize = 512;
pub const MAX_REVLOG_ENTRIES: usize = 500_000;
pub const MAX_REVIEW_LOG_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_REVIEW_LOG_LINE_BYTES: usize = 512;

pub const COMPATIBILITY_NOTICE: &str = include_str!("../../../licenses/NOTICE-Flashcards-Anki.md");
pub const ANKI_LICENSE: &str = include_str!("../../../licenses/LICENSE-Anki.txt");
pub const ANKIDROID_LICENSE: &str = include_str!("../../../licenses/LICENSE-AnkiDroid.txt");
pub const RESVG_LICENSE: &str = include_str!("../../../licenses/LICENSE-resvg.txt");
pub const DEVICE_DEPENDENCY_LICENSES: &str =
    include_str!("../../../licenses/LICENSE-Flashcards-device-dependencies.txt");
pub const HOST_DEPENDENCY_LICENSES: &str =
    include_str!("../../../licenses/LICENSE-Flashcards-host-dependencies.txt");
pub const ATKINSON_LICENSE: &str =
    include_str!("../../kobo-text/fonts/LICENSE-AtkinsonHyperlegible.txt");
pub const DEJAVU_LICENSE: &str = include_str!("../../kobo-text/fonts/LICENSE-DejaVu.txt");

pub const DISTRIBUTION_DOCUMENTS: [(&str, &str); 7] = [
    ("Compatibility notice and source pins", COMPATIBILITY_NOTICE),
    ("Anki licence", ANKI_LICENSE),
    ("AnkiDroid licence", ANKIDROID_LICENSE),
    ("resvg licence", RESVG_LICENSE),
    (
        "Flashcards device dependency licences",
        DEVICE_DEPENDENCY_LICENSES,
    ),
    ("Atkinson Hyperlegible licence", ATKINSON_LICENSE),
    ("DejaVu licence", DEJAVU_LICENSE),
];

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BundleManifest {
    pub format_version: u16,
    pub sources: Vec<Source>,
    pub notes: Vec<Note>,
    pub notetypes: Vec<NoteType>,
    pub decks: Vec<Deck>,
    pub deck_configurations: Vec<DeckConfiguration>,
    pub cards: Vec<Card>,
    pub review_queue: ReviewQueue,
    pub revlog: Vec<ReviewLog>,
    pub graves: Vec<Grave>,
    pub media: Vec<Media>,
    pub diagnostics: Vec<Diagnostic>,
}

fn validate_single_side_image(names: &[String]) -> Result<(), FormatError> {
    if names
        .iter()
        .filter(|name| media_type(name).starts_with("image/"))
        .count()
        > 1
    {
        return Err(FormatError::InvalidManifest(
            "a card side contains more than one image".to_owned(),
        ));
    }
    Ok(())
}

impl BundleManifest {
    #[must_use]
    pub fn empty(source: Source) -> Self {
        Self {
            format_version: VERSION,
            sources: vec![source],
            notes: Vec::new(),
            notetypes: Vec::new(),
            decks: Vec::new(),
            deck_configurations: Vec::new(),
            cards: Vec::new(),
            review_queue: ReviewQueue::default(),
            revlog: Vec::new(),
            graves: Vec::new(),
            media: Vec::new(),
            diagnostics: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Source {
    pub package_kind: String,
    pub collection_member: String,
    pub collection_schema: i64,
    pub normalized_schema: i64,
    pub collection_id: i64,
    pub collection_created: i64,
    pub collection_modified: i64,
    pub schema_modified: i64,
    pub dirty: i64,
    pub user_sequence: i64,
    pub last_sync: i64,
    pub note_count: usize,
    pub card_count: usize,
    pub upstream_anki_revision: String,
    pub original_config_json: String,
    pub original_models_json: String,
    pub original_decks_json: String,
    pub original_deck_configurations_json: String,
    pub original_tags_json: String,
    pub normalized_config: Vec<CollectionConfig>,
    pub normalized_tags: Vec<CollectionTag>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CollectionConfig {
    pub key: String,
    pub user_sequence: i64,
    pub modified: i64,
    pub value_hex: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CollectionTag {
    pub name: String,
    pub user_sequence: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Note {
    pub id: i64,
    pub guid: String,
    pub notetype_id: i64,
    pub modified: i64,
    pub user_sequence: i64,
    pub tags: Vec<String>,
    pub fields: Vec<String>,
    pub sort_field: String,
    pub checksum: i64,
    pub flags: i64,
    pub data: String,
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
    pub user_sequence: i32,
    pub queue: i32,
    pub card_type: i32,
    pub due: i64,
    pub interval: i32,
    pub ease_factor: i32,
    pub repetitions: i32,
    pub lapses: i32,
    pub remaining_steps: i32,
    pub original_due: i64,
    pub original_deck_id: i64,
    pub flags: i32,
    pub data: String,
    pub modified: i64,
    pub template_name: String,
    pub front: String,
    pub back: String,
    pub tags: Vec<String>,
    pub question_media_names: Vec<String>,
    pub answer_media_names: Vec<String>,
    pub media_names: Vec<String>,
    pub attachments: Vec<Attachment>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReviewQueue {
    pub card_ids: Vec<i64>,
    pub new_count: usize,
    pub learning_count: usize,
    pub review_count: usize,
    pub decks: Vec<DeckQueue>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DeckQueue {
    pub source_index: usize,
    pub root_deck_id: i64,
    pub card_ids: Vec<i64>,
    pub new_count: usize,
    pub learning_count: usize,
    pub review_count: usize,
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
pub struct Grave {
    pub object_id: i64,
    pub object_kind: i32,
    pub user_sequence: i32,
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnerReviewRecord {
    format: u8,
    bundle_sha256: String,
    card_id: i64,
    grade: String,
    imported_due: i64,
    imported_reps: i64,
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

/// Re-rasterizes SVG attachments used by `card_ids` and requires the exact
/// digest-addressed PNG bytes stored in the bundle.
///
/// # Errors
///
/// Returns an error if a card/media reference is missing, an SVG is unsafe, or
/// its deterministic PNG differs from the bundled image.
pub fn verify_svg_bindings(bundle: &ParsedBundle, card_ids: &[i64]) -> Result<(), FormatError> {
    let mut verified = BTreeSet::new();
    for card_id in card_ids {
        let index = bundle
            .manifest
            .cards
            .binary_search_by_key(card_id, |card| card.id)
            .map_err(|_| {
                FormatError::InvalidManifest("SVG verification card is missing".to_owned())
            })?;
        for attachment in bundle.manifest.cards[index]
            .attachments
            .iter()
            .filter(|attachment| attachment.mime == "image/svg+xml")
        {
            if !verified.insert(attachment.name.clone()) {
                continue;
            }
            let source = bundle.media(&attachment.name).ok_or_else(|| {
                FormatError::InvalidManifest("retained SVG source is missing".to_owned())
            })?;
            let rendered_name = attachment.rendered_name.as_deref().ok_or_else(|| {
                FormatError::InvalidManifest("SVG rendered media is missing".to_owned())
            })?;
            let rendered = bundle.media(rendered_name).ok_or_else(|| {
                FormatError::InvalidManifest("SVG rendered bytes are missing".to_owned())
            })?;
            if rasterize_svg(source)? != rendered {
                return Err(FormatError::InvalidManifest(
                    "SVG rendered image does not match its retained source".to_owned(),
                ));
            }
        }
    }
    Ok(())
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
    InvalidManifest(String),
    InvalidReviewLog(String),
    InvalidSvg(String),
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
            Self::InvalidManifest(message) => {
                write!(formatter, "invalid bundle manifest: {message}")
            }
            Self::InvalidReviewLog(message) => {
                write!(formatter, "invalid Cobalt review log: {message}")
            }
            Self::InvalidSvg(message) => write!(formatter, "invalid SVG media: {message}"),
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
    validate_manifest(&manifest, &payload)?;
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
    validate_manifest(&manifest, &payload)?;
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
    let mut expected_offset = 0_u64;
    for record in media {
        let name = canonical_media_name(&record.name)?;
        if name != record.name {
            return Err(FormatError::InvalidMediaName(record.name.clone()));
        }
        if record.mime != media_type(&name) {
            return Err(FormatError::InvalidManifest(format!(
                "media {name:?} has a mismatched MIME type"
            )));
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
        if record.offset != expected_offset {
            return Err(FormatError::NonDeterministicOrder);
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
        expected_offset = record
            .offset
            .checked_add(record.length)
            .ok_or(FormatError::MediaLimit)?;
    }
    if expected_offset != payload.len() as u64 {
        return Err(FormatError::LengthMismatch);
    }
    Ok(())
}

fn attachment_kind(mime: &str) -> AttachmentKind {
    if mime.starts_with("image/") {
        AttachmentKind::Image
    } else if mime.starts_with("audio/") {
        AttachmentKind::Audio
    } else if mime.starts_with("video/") {
        AttachmentKind::Video
    } else {
        AttachmentKind::Other
    }
}

fn validate_manifest(manifest: &BundleManifest, payload: &[u8]) -> Result<(), FormatError> {
    validate_manifest_shape(manifest)?;
    validate_sources(&manifest.sources)?;
    let note_ids = manifest
        .notes
        .iter()
        .map(|note| note.id)
        .collect::<BTreeSet<_>>();
    let notetype_ids = manifest
        .notetypes
        .iter()
        .map(|notetype| notetype.id)
        .collect::<BTreeSet<_>>();
    let deck_ids = manifest
        .decks
        .iter()
        .map(|deck| deck.id)
        .collect::<BTreeSet<_>>();
    let deck_configuration_ids = manifest
        .deck_configurations
        .iter()
        .map(|configuration| configuration.id)
        .collect::<BTreeSet<_>>();
    let card_ids = manifest
        .cards
        .iter()
        .map(|card| card.id)
        .collect::<BTreeSet<_>>();
    let media_by_name = manifest
        .media
        .iter()
        .map(|media| (media.name.as_str(), media))
        .collect::<BTreeMap<_, _>>();

    validate_note_and_deck_references(manifest, &notetype_ids, &deck_configuration_ids)?;
    validate_card_references(manifest, &note_ids, &deck_ids, &media_by_name)?;
    validate_revlog_references(&manifest.revlog, &card_ids)?;
    validate_review_queue(
        &manifest.review_queue,
        &card_ids,
        &deck_ids,
        manifest.sources.len(),
    )?;
    if payload.len() > MAX_PAYLOAD_BYTES {
        return Err(FormatError::MediaLimit);
    }
    Ok(())
}

fn validate_manifest_shape(manifest: &BundleManifest) -> Result<(), FormatError> {
    if manifest.sources.is_empty() {
        return Err(FormatError::InvalidManifest(
            "at least one source collection is required".to_owned(),
        ));
    }
    if manifest.notes.len() > MAX_NOTES
        || manifest.cards.len() > MAX_CARDS
        || manifest.revlog.len() > MAX_REVLOG_ENTRIES
    {
        return Err(FormatError::InvalidManifest(
            "collection record count exceeds the device limit".to_owned(),
        ));
    }
    sorted_unique_by(&manifest.notes, |note| note.id, "notes")?;
    sorted_unique_by(&manifest.notetypes, |notetype| notetype.id, "notetypes")?;
    sorted_unique_by(&manifest.decks, |deck| deck.id, "decks")?;
    sorted_unique_by(
        &manifest.deck_configurations,
        |configuration| configuration.id,
        "deck configurations",
    )?;
    sorted_unique_by(&manifest.cards, |card| card.id, "cards")?;
    sorted_unique_by(&manifest.revlog, |review| review.id, "revlog")?;
    sorted_unique_by(
        &manifest.graves,
        |grave| (grave.object_id, grave.object_kind),
        "graves",
    )?;
    if manifest.notes.iter().any(|note| note.id <= 0)
        || manifest.notetypes.iter().any(|notetype| notetype.id <= 0)
        || manifest.decks.iter().any(|deck| deck.id <= 0)
        || manifest
            .deck_configurations
            .iter()
            .any(|configuration| configuration.id <= 0)
        || manifest.cards.iter().any(|card| {
            card.id <= 0
                || card.ordinal < 0
                || i32::try_from(card.due).is_err()
                || i32::try_from(card.original_due).is_err()
                || card.original_deck_id < 0
                || card.interval < 0
                || card.ease_factor < 0
                || card.repetitions < 0
                || card.lapses < 0
                || card.remaining_steps < 0
                || !(0..=3).contains(&card.card_type)
                || !(-3..=4).contains(&card.queue)
        })
        || manifest.revlog.iter().any(|review| review.id <= 0)
    {
        return Err(FormatError::InvalidManifest(
            "collection identifiers or scheduling fields are outside Anki's normalized bounds"
                .to_owned(),
        ));
    }
    Ok(())
}

fn validate_sources(sources: &[Source]) -> Result<(), FormatError> {
    for source in sources {
        if source.upstream_anki_revision.is_empty()
            || source.note_count > MAX_NOTES
            || source.card_count > MAX_CARDS
        {
            return Err(FormatError::InvalidManifest(
                "source metadata is incomplete or unbounded".to_owned(),
            ));
        }
        sorted_unique_by(
            &source.normalized_config,
            |entry| entry.key.clone(),
            "collection configuration",
        )?;
        sorted_unique_by(
            &source.normalized_tags,
            |entry| entry.name.clone(),
            "collection tags",
        )?;
    }
    Ok(())
}

fn validate_note_and_deck_references(
    manifest: &BundleManifest,
    notetype_ids: &BTreeSet<i64>,
    deck_configuration_ids: &BTreeSet<i64>,
) -> Result<(), FormatError> {
    for note in &manifest.notes {
        if !notetype_ids.contains(&note.notetype_id) {
            return Err(FormatError::InvalidManifest(format!(
                "note {} references missing notetype {}",
                note.id, note.notetype_id
            )));
        }
    }
    for deck in &manifest.decks {
        if deck
            .configuration_id
            .is_some_and(|id| !deck_configuration_ids.contains(&id))
        {
            return Err(FormatError::InvalidManifest(format!(
                "deck {} references a missing configuration",
                deck.id
            )));
        }
    }
    Ok(())
}

fn validate_card_references(
    manifest: &BundleManifest,
    note_ids: &BTreeSet<i64>,
    deck_ids: &BTreeSet<i64>,
    media_by_name: &BTreeMap<&str, &Media>,
) -> Result<(), FormatError> {
    for card in &manifest.cards {
        if !note_ids.contains(&card.note_id) || !deck_ids.contains(&card.deck_id) {
            return Err(FormatError::InvalidManifest(format!(
                "card {} has a missing note or deck",
                card.id
            )));
        }
        validate_sorted_media_names(&card.question_media_names)?;
        validate_sorted_media_names(&card.answer_media_names)?;
        validate_sorted_media_names(&card.media_names)?;
        validate_single_side_image(&card.question_media_names)?;
        validate_single_side_image(&card.answer_media_names)?;
        let expected = card
            .question_media_names
            .iter()
            .chain(&card.answer_media_names)
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if expected != card.media_names {
            return Err(FormatError::InvalidManifest(format!(
                "card {} has a non-canonical media union",
                card.id
            )));
        }
        let mut attachment_names = BTreeSet::new();
        for attachment in &card.attachments {
            let name = canonical_media_name(&attachment.name)?;
            if name != attachment.name
                || !attachment_names.insert(name.clone())
                || !card.media_names.contains(&name)
                || !media_by_name.contains_key(name.as_str())
                || attachment.mime != media_type(&name)
                || attachment.kind != attachment_kind(&attachment.mime)
                || attachment.kind == AttachmentKind::Other
            {
                return Err(FormatError::InvalidManifest(format!(
                    "card {} has an invalid attachment index",
                    card.id
                )));
            }
            if let Some(rendered) = &attachment.rendered_name {
                let rendered = canonical_media_name(rendered)?;
                let source = media_by_name
                    .get(name.as_str())
                    .expect("attachment source was checked");
                let expected = format!("cobalt-svg-{}.png", source.sha256);
                if attachment.mime != "image/svg+xml"
                    || attachment.kind != AttachmentKind::Image
                    || rendered != expected
                    || media_type(&rendered) != "image/png"
                    || !media_by_name.contains_key(rendered.as_str())
                {
                    return Err(FormatError::InvalidManifest(format!(
                        "card {} has an invalid rendered-media redirect",
                        card.id
                    )));
                }
            } else if attachment.mime == "image/svg+xml" {
                return Err(FormatError::InvalidManifest(format!(
                    "card {} has an SVG without a host-rendered bundle image",
                    card.id
                )));
            }
        }
        if attachment_names != card.media_names.iter().cloned().collect::<BTreeSet<_>>() {
            return Err(FormatError::InvalidManifest(format!(
                "card {} has referenced media without attachments",
                card.id
            )));
        }
    }
    Ok(())
}

fn validate_revlog_references(
    revlog: &[ReviewLog],
    card_ids: &BTreeSet<i64>,
) -> Result<(), FormatError> {
    for review in revlog {
        if !card_ids.contains(&review.card_id) {
            return Err(FormatError::InvalidManifest(format!(
                "revlog {} references missing card {}",
                review.id, review.card_id
            )));
        }
    }
    Ok(())
}

fn sorted_unique_by<T, K: Ord>(
    values: &[T],
    key: impl Fn(&T) -> K,
    label: &str,
) -> Result<(), FormatError> {
    let mut previous = None;
    for value in values {
        let current = key(value);
        if previous
            .as_ref()
            .is_some_and(|previous| previous >= &current)
        {
            return Err(FormatError::InvalidManifest(format!(
                "{label} are not sorted and unique"
            )));
        }
        previous = Some(current);
    }
    Ok(())
}

fn validate_sorted_media_names(names: &[String]) -> Result<(), FormatError> {
    let mut previous = None;
    for raw_name in names {
        let name = canonical_media_name(raw_name)?;
        if &name != raw_name || previous.as_ref().is_some_and(|prior| prior >= &name) {
            return Err(FormatError::InvalidManifest(
                "card media names are not canonical, sorted, and unique".to_owned(),
            ));
        }
        previous = Some(name);
    }
    Ok(())
}

fn validate_review_queue(
    queue: &ReviewQueue,
    card_ids: &BTreeSet<i64>,
    deck_ids: &BTreeSet<i64>,
    source_count: usize,
) -> Result<(), FormatError> {
    if queue.card_ids.len() > MAX_REVIEW_QUEUE_CARDS
        || queue.new_count + queue.learning_count + queue.review_count != queue.card_ids.len()
    {
        return Err(FormatError::InvalidManifest(
            "review queue counts do not match its cards".to_owned(),
        ));
    }
    let unique = queue.card_ids.iter().copied().collect::<BTreeSet<_>>();
    if unique.len() != queue.card_ids.len() || !unique.is_subset(card_ids) {
        return Err(FormatError::InvalidManifest(
            "review queue contains duplicate or missing cards".to_owned(),
        ));
    }
    let mut flattened = Vec::new();
    let mut new_count = 0_usize;
    let mut learning_count = 0_usize;
    let mut review_count = 0_usize;
    for deck in &queue.decks {
        if deck.source_index >= source_count
            || !deck_ids.contains(&deck.root_deck_id)
            || deck.new_count + deck.learning_count + deck.review_count != deck.card_ids.len()
        {
            return Err(FormatError::InvalidManifest(
                "deck review queue metadata is invalid".to_owned(),
            ));
        }
        flattened.extend_from_slice(&deck.card_ids);
        new_count = new_count.saturating_add(deck.new_count);
        learning_count = learning_count.saturating_add(deck.learning_count);
        review_count = review_count.saturating_add(deck.review_count);
    }
    if flattened != queue.card_ids
        || new_count != queue.new_count
        || learning_count != queue.learning_count
        || review_count != queue.review_count
    {
        return Err(FormatError::InvalidManifest(
            "aggregate and per-deck review queues differ".to_owned(),
        ));
    }
    Ok(())
}

/// Validates the append-only owner-local review log without changing its bytes.
///
/// # Errors
///
/// Returns an error for truncation, unknown fields, unsupported versions or
/// grades, invalid bundle digests, or records beyond the fixed bounds.
pub fn validate_review_log(bytes: &[u8]) -> Result<usize, FormatError> {
    if bytes.len() > MAX_REVIEW_LOG_BYTES {
        return Err(FormatError::InvalidReviewLog(
            "file exceeds the device export bound".to_owned(),
        ));
    }
    if !bytes.is_empty() && !bytes.ends_with(b"\n") {
        return Err(FormatError::InvalidReviewLog(
            "last record is not newline-terminated".to_owned(),
        ));
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| FormatError::InvalidReviewLog("file is not UTF-8".to_owned()))?;
    let mut records = 0_usize;
    for line in text.lines() {
        if line.is_empty() || line.len() > MAX_REVIEW_LOG_LINE_BYTES {
            return Err(FormatError::InvalidReviewLog(
                "record is empty or too large".to_owned(),
            ));
        }
        let record: OwnerReviewRecord = serde_json::from_str(line)
            .map_err(|error| FormatError::InvalidReviewLog(error.to_string()))?;
        if record.format != 2
            || record.card_id <= 0
            || i32::try_from(record.imported_due).is_err()
            || i32::try_from(record.imported_reps).is_err()
            || !matches!(record.grade.as_str(), "again" | "hard" | "good")
            || record.bundle_sha256.len() != 64
            || !record
                .bundle_sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(FormatError::InvalidReviewLog(
                "record has an unsupported shape".to_owned(),
            ));
        }
        records = records.saturating_add(1);
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> Source {
        Source {
            package_kind: "apkg".to_owned(),
            collection_member: "collection.anki2".to_owned(),
            collection_schema: 11,
            normalized_schema: 18,
            collection_id: 1,
            collection_created: 0,
            collection_modified: 1,
            schema_modified: 1,
            dirty: 0,
            user_sequence: 0,
            last_sync: 0,
            note_count: 0,
            card_count: 0,
            upstream_anki_revision: "pinned".to_owned(),
            original_config_json: "{}".to_owned(),
            original_models_json: "{}".to_owned(),
            original_decks_json: "{}".to_owned(),
            original_deck_configurations_json: "{}".to_owned(),
            original_tags_json: "{}".to_owned(),
            normalized_config: Vec::new(),
            normalized_tags: Vec::new(),
        }
    }

    fn single_card_manifest() -> BundleManifest {
        let mut manifest = BundleManifest::empty(source());
        manifest.sources[0].note_count = 1;
        manifest.sources[0].card_count = 1;
        manifest.notes.push(Note {
            id: 1,
            guid: "guid".to_owned(),
            notetype_id: 1,
            modified: 1,
            user_sequence: 0,
            tags: Vec::new(),
            fields: vec!["front".to_owned()],
            sort_field: "front".to_owned(),
            checksum: 1,
            flags: 0,
            data: String::new(),
        });
        manifest.notetypes.push(NoteType {
            id: 1,
            name: "Basic".to_owned(),
            original_json: "{}".to_owned(),
        });
        manifest.deck_configurations.push(DeckConfiguration {
            id: 1,
            name: "Default".to_owned(),
            original_json: "{}".to_owned(),
        });
        manifest.decks.push(Deck {
            id: 1,
            name: "Default".to_owned(),
            configuration_id: Some(1),
            original_json: "{}".to_owned(),
        });
        manifest.cards.push(Card {
            id: 1,
            note_id: 1,
            deck_id: 1,
            ordinal: 0,
            user_sequence: 0,
            queue: 0,
            card_type: 0,
            due: 1,
            interval: 0,
            ease_factor: 0,
            repetitions: 0,
            lapses: 0,
            remaining_steps: 0,
            original_due: 0,
            original_deck_id: 0,
            flags: 0,
            data: String::new(),
            modified: 1,
            template_name: "Card".to_owned(),
            front: "front".to_owned(),
            back: "back".to_owned(),
            tags: Vec::new(),
            question_media_names: Vec::new(),
            answer_media_names: Vec::new(),
            media_names: Vec::new(),
            attachments: Vec::new(),
            diagnostics: Vec::new(),
        });
        manifest.review_queue = ReviewQueue {
            card_ids: vec![1],
            new_count: 1,
            learning_count: 0,
            review_count: 0,
            decks: vec![DeckQueue {
                source_index: 0,
                root_deck_id: 1,
                card_ids: vec![1],
                new_count: 1,
                learning_count: 0,
                review_count: 0,
            }],
        };
        manifest
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

    #[test]
    fn owner_review_log_is_versioned_bounded_and_bundle_bound() {
        let valid = b"{\"format\":2,\"bundle_sha256\":\"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\",\"card_id\":7,\"grade\":\"good\",\"imported_due\":3,\"imported_reps\":2}\n";
        assert_eq!(validate_review_log(valid), Ok(1));
        assert!(validate_review_log(&valid[..valid.len() - 1]).is_err());
        assert!(validate_review_log(
            b"{\"format\":2,\"bundle_sha256\":\"bad\",\"card_id\":7,\"grade\":\"easy\",\"imported_due\":3,\"imported_reps\":2}\n"
        )
        .is_err());
        assert!(validate_review_log(
            b"{\"format\":2,\"format\":2,\"bundle_sha256\":\"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\",\"card_id\":7,\"grade\":\"good\",\"imported_due\":3,\"imported_reps\":2}\n"
        )
        .is_err());
        assert!(validate_review_log(
            b"{\"format\":2,\"bundle_sha256\":\"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\",\"card_id\":7,\"grade\":\"good\",\"imported_due\":3,\"imported_reps\":2147483648}\n"
        )
        .is_err());
    }

    #[test]
    fn bundle_validation_matches_review_log_scheduling_bounds() {
        let mut manifest = single_card_manifest();
        manifest.cards[0].due = i64::MAX;
        assert!(matches!(
            encode(manifest, BTreeMap::new()),
            Err(FormatError::InvalidManifest(_))
        ));
    }

    #[test]
    fn non_svg_attachments_cannot_redirect_displayed_bytes() {
        let mut manifest = single_card_manifest();
        manifest.cards[0].question_media_names = vec!["front.png".to_owned()];
        manifest.cards[0].answer_media_names = vec!["front.png".to_owned()];
        manifest.cards[0].media_names = vec!["front.png".to_owned()];
        manifest.cards[0].attachments = vec![Attachment {
            name: "front.png".to_owned(),
            rendered_name: Some("other.png".to_owned()),
            mime: "image/png".to_owned(),
            kind: AttachmentKind::Image,
        }];
        let media = BTreeMap::from([
            ("front.png".to_owned(), b"front".to_vec()),
            ("other.png".to_owned(), b"other".to_vec()),
        ]);
        assert!(matches!(
            encode(manifest, media),
            Err(FormatError::InvalidManifest(_))
        ));
    }

    #[test]
    fn prebuilt_bundles_cannot_bypass_single_image_sides() {
        let mut manifest = single_card_manifest();
        manifest.cards[0].question_media_names =
            vec!["first.png".to_owned(), "second.png".to_owned()];
        manifest.cards[0].answer_media_names =
            vec!["first.png".to_owned(), "second.png".to_owned()];
        manifest.cards[0].media_names = vec!["first.png".to_owned(), "second.png".to_owned()];
        manifest.cards[0].attachments = ["first.png", "second.png"]
            .into_iter()
            .map(|name| Attachment {
                name: name.to_owned(),
                rendered_name: None,
                mime: "image/png".to_owned(),
                kind: AttachmentKind::Image,
            })
            .collect();
        let media = BTreeMap::from([
            ("first.png".to_owned(), b"first".to_vec()),
            ("second.png".to_owned(), b"second".to_vec()),
        ]);
        assert!(matches!(
            encode(manifest, media),
            Err(FormatError::InvalidManifest(_))
        ));
    }
}

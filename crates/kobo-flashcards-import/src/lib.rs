#![forbid(unsafe_code)]

//! Host-only APKG/COLPKG ingestion for the Flashcards offline bundle.
//!
//! It intentionally treats every package member as hostile. The only
//! executable input Anki cards normally carry is HTML/JavaScript; rendered
//! device strings are plain text, and the source template is retained only as
//! inert data in the manifest.

use anki::collection::{Collection, CollectionBuilder};
use anki::deckconfig::{DeckConfSchema11, DeckConfigId};
use anki::decks::{DeckId, DeckSchema11};
use anki::notetype::NotetypeSchema11;
use anki::services::MediaService;
use anki_proto::generic;
use kobo_flashcards_format::{
    canonical_media_name, decode, digest_hex, encode, media_type, rasterize_svg,
    validate_review_log, validate_svg_source, verify_card_images, Attachment, AttachmentKind,
    BundleManifest, Card, CardTextSpan, CardTextStyle, CollectionConfig, CollectionTag, Deck,
    DeckConfiguration, DeckQueue, Diagnostic, FormatError, Grave, Note as BundleNote, NoteType,
    ReviewLog, ReviewQueue, Source, CONVERTER_REVISION, MAX_BUNDLE_BYTES, MAX_CARDS,
    MAX_CARD_TEXT_SPANS, MAX_MEDIA_BYTES, MAX_MEDIA_ENTRIES, MAX_PAYLOAD_BYTES,
    MAX_REVIEW_QUEUE_CARDS,
};
use rusqlite::Connection;
use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use unicase::UniCase;
use zip::{CompressionMethod, ZipArchive};

pub const UPSTREAM_ANKI_REVISION: &str = "9e32ad8849068510a82273889c21b22e1acf0949";
pub const MAX_ARCHIVE_ENTRIES: usize = 8_192;
pub const MAX_ARCHIVE_BYTES: u64 = MAX_BUNDLE_BYTES;
pub const MAX_COLLECTION_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_EXPANDED_ARCHIVE_BYTES: u64 =
    MAX_COLLECTION_BYTES + MAX_PAYLOAD_BYTES as u64 + 4 * 1024 * 1024;
pub const MAX_EXPANSION_RATIO: u64 = 100;
pub const TRANSFER_CHUNK_BYTES: usize = 256 * 1024;
pub const NORMALIZED_SCHEMA: i64 = 18;
pub const SUPPORTED_COLLECTION_SCHEMAS: [i64; 6] = [11, 14, 15, 16, 17, 18];
const TYPE_ANSWER_PLACEHOLDER: &str = "[Type answer is unavailable on Kobo]";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImportMode {
    /// Adds a legacy APKG to an optional existing Flashcards bundle.
    MergeApkg,
    /// Replaces an existing Flashcards bundle with a COLPKG collection.
    ReplaceColpkg,
}

#[derive(Clone, Debug)]
pub struct ImportOptions {
    pub mode: ImportMode,
    pub merge_into: Option<PathBuf>,
    /// Tests use this to prove a failed output cannot replace a prior bundle.
    pub fail_after_bytes: Option<usize>,
}

impl ImportOptions {
    #[must_use]
    pub const fn apkg() -> Self {
        Self {
            mode: ImportMode::MergeApkg,
            merge_into: None,
            fail_after_bytes: None,
        }
    }

    #[must_use]
    pub const fn colpkg() -> Self {
        Self {
            mode: ImportMode::ReplaceColpkg,
            merge_into: None,
            fail_after_bytes: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportReport {
    pub package_kind: String,
    pub notes: usize,
    pub active_cards: usize,
    pub new_cards: usize,
    pub learning_cards: usize,
    pub review_cards: usize,
    pub decks: usize,
    pub media_files: usize,
    pub media_bytes: u64,
    pub image_bearing_notes: usize,
    pub sound_bearing_notes: usize,
    pub diagnostics: Vec<Diagnostic>,
    pub bundle_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StageReport {
    pub bytes: u64,
    pub sha256: String,
    pub resumed_at: u64,
    pub destination: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReviewLogExportReport {
    pub records: usize,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug)]
pub enum ImportError {
    Io(io::Error),
    Zip(String),
    Sql(String),
    Format(FormatError),
    InvalidPackage(String),
    Interrupted,
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "{error}"),
            Self::Zip(error) => write!(formatter, "invalid package archive: {error}"),
            Self::Sql(error) => write!(formatter, "invalid collection database: {error}"),
            Self::Format(error) => write!(formatter, "{error}"),
            Self::InvalidPackage(error) => write!(formatter, "unsupported package: {error}"),
            Self::Interrupted => write!(formatter, "import interrupted before publication"),
        }
    }
}

impl std::error::Error for ImportError {}

impl From<io::Error> for ImportError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<FormatError> for ImportError {
    fn from(error: FormatError) -> Self {
        Self::Format(error)
    }
}

/// Imports a bounded legacy `SQLite` APKG/COLPKG, then atomically publishes one
/// fully verified device bundle. It never extracts a media pathname.
///
/// # Errors
///
/// Returns an error when archive validation, collection inspection, template
/// rendering, merging, bundle encoding, or atomic publication fails.
pub fn import(
    input: &Path,
    output: &Path,
    options: &ImportOptions,
) -> Result<ImportReport, ImportError> {
    let package_kind = package_kind(input, options.mode)?;
    let unpacked = read_archive(input, package_kind)?;
    let mut manifest = read_collection(
        &unpacked.collection,
        &unpacked.media,
        package_kind,
        &unpacked.collection_member,
        output,
    )?;
    let mut imported_media = unpacked.media;
    prepare_rendered_media(&mut manifest, &mut imported_media)?;
    let (manifest, media) = match options.mode {
        ImportMode::MergeApkg => {
            merge_existing(manifest, imported_media, options.merge_into.as_deref())?
        }
        ImportMode::ReplaceColpkg => (manifest, imported_media),
    };
    let encoded = encode(manifest.clone(), media)?;
    let parsed = verify_bundle_bytes(&encoded)?;
    let report = report_for(&parsed, package_kind.name(), &encoded);
    atomic_write(output, &encoded, options.fail_after_bytes)?;
    Ok(report)
}

/// Fully reads and digest-validates a published bundle, then proves that every
/// image attachment is addressed by bytes inside that same bundle.
///
/// # Errors
///
/// Returns an error when the bundle or any image-media reference fails
/// validation.
pub fn verify_bundle(path: &Path) -> Result<ImportReport, ImportError> {
    let bytes = fs::read(path)?;
    let parsed = verify_bundle_bytes(&bytes)?;
    let package_kind = if parsed.manifest().sources.len() == 1 {
        parsed.manifest().sources[0].package_kind.as_str()
    } else {
        "merged"
    };
    Ok(report_for(&parsed, package_kind, &bytes))
}

fn verify_bundle_bytes(bytes: &[u8]) -> Result<kobo_flashcards_format::ParsedBundle, ImportError> {
    let parsed = decode(bytes)?;
    let card_ids = parsed
        .manifest()
        .cards
        .iter()
        .map(|card| card.id)
        .collect::<Vec<_>>();
    verify_card_images(&parsed, &card_ids)?;
    Ok(parsed)
}

/// Stages a verified bundle to the fixed private Flashcards shelf on a mounted
/// Kobo volume. Each chunk is durable before its matching resume record is
/// published; the existing collection is replaced only after a complete
/// digest check and atomic rename.
///
/// # Errors
///
/// Returns an error when source verification, destination staging, resume
/// validation, or final publication fails.
pub fn stage_for_kobo(
    bundle: &Path,
    kobo_root: &Path,
    interrupt_after_chunks: Option<usize>,
) -> Result<StageReport, ImportError> {
    let _report = verify_bundle(bundle)?;
    let source = fs::read(bundle)?;
    let source_digest = digest_hex(&source);
    let destination = kobo_root
        .join(".adds")
        .join("cobalt")
        .join("data")
        .join("flashcards")
        .join("collection.cobfc");
    let parent = destination.parent().ok_or_else(|| {
        ImportError::InvalidPackage("fixed Flashcards destination has no parent".to_owned())
    })?;
    fs::create_dir_all(parent)?;
    let partial = parent.join(".collection.cobfc.writing");
    let journal = parent.join(".collection.cobfc.resume");
    let mut offset = resume_offset(&partial, &journal, &source, &source_digest);
    let resumed_at = u64::try_from(offset).map_err(|_| ImportError::Interrupted)?;
    if offset == 0 {
        let _ignored = fs::remove_file(&partial);
        let _ignored = fs::remove_file(&journal);
    }

    let mut partial_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&partial)?;
    let mut chunks = 0_usize;
    while offset < source.len() {
        let end = offset
            .saturating_add(TRANSFER_CHUNK_BYTES)
            .min(source.len());
        partial_file.write_all(&source[offset..end])?;
        partial_file.sync_data()?;
        offset = end;
        write_resume_record(&journal, &source_digest, offset)?;
        chunks = chunks.saturating_add(1);
        if interrupt_after_chunks.is_some_and(|limit| chunks >= limit) && offset < source.len() {
            return Err(ImportError::Interrupted);
        }
    }
    drop(partial_file);
    if digest_hex(&fs::read(&partial)?) != source_digest {
        return Err(ImportError::InvalidPackage(
            "staged collection digest changed before publication".to_owned(),
        ));
    }
    // `verify_bundle` also checks the decompressed format and all media
    // digests, not just the outer byte digest.
    let _report = verify_bundle(&partial)?;
    File::open(&partial)?.sync_all()?;
    fs::rename(&partial, &destination)?;
    let _ignored = fs::remove_file(&journal);
    File::open(parent)?.sync_all()?;
    Ok(StageReport {
        bytes: u64::try_from(source.len()).map_err(|_| ImportError::Interrupted)?,
        sha256: source_digest,
        resumed_at,
        destination,
    })
}

/// Copies the fixed, append-only Cobalt review log from a mounted Kobo volume
/// without converting it to Anki history. Each newline-delimited record is
/// structurally checked, while its original bytes are preserved exactly.
///
/// # Errors
///
/// Returns an error when the fixed device log is absent, too large, malformed,
/// or cannot be atomically copied to `output`.
pub fn export_local_review_log(
    kobo_root: &Path,
    output: &Path,
) -> Result<ReviewLogExportReport, ImportError> {
    let source = kobo_root
        .join(".adds")
        .join("cobalt")
        .join("data")
        .join("flashcards")
        .join("cobalt-review-log.ndjson");
    let bytes = fs::read(source)?;
    let records = validate_review_log(&bytes)?;
    atomic_write(output, &bytes, None)?;
    let published = fs::read(output)?;
    if published != bytes {
        return Err(ImportError::InvalidPackage(
            "published review log differs from the checked source".to_owned(),
        ));
    }
    Ok(ReviewLogExportReport {
        records,
        bytes: u64::try_from(bytes.len()).map_err(|_| ImportError::Interrupted)?,
        sha256: digest_hex(&bytes),
    })
}

fn resume_offset(partial: &Path, journal: &Path, source: &[u8], source_digest: &str) -> usize {
    let Ok(record) = fs::read_to_string(journal) else {
        return 0;
    };
    let mut fields = record.lines();
    let (Some(expected_source), Some(offset), None) = (fields.next(), fields.next(), fields.next())
    else {
        return 0;
    };
    let Ok(offset) = offset.parse::<usize>() else {
        return 0;
    };
    if expected_source != source_digest || offset > source.len() {
        return 0;
    }
    let Ok(bytes) = fs::read(partial) else {
        return 0;
    };
    if bytes.len() != offset || bytes != source[..offset] {
        return 0;
    }
    offset
}

fn write_resume_record(
    journal: &Path,
    source_digest: &str,
    offset: usize,
) -> Result<(), ImportError> {
    let writing = journal.with_extension("resume.writing");
    let _ignored = fs::remove_file(&writing);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&writing)?;
    writeln!(file, "{source_digest}\n{offset}")?;
    file.sync_all()?;
    fs::rename(writing, journal)?;
    if let Some(parent) = journal.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PackageKind {
    Apkg,
    Colpkg,
}

impl PackageKind {
    const fn name(self) -> &'static str {
        match self {
            Self::Apkg => "apkg",
            Self::Colpkg => "colpkg",
        }
    }
}

fn package_kind(path: &Path, mode: ImportMode) -> Result<PackageKind, ImportError> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    match (extension.as_deref(), mode) {
        (Some("apkg"), ImportMode::MergeApkg) => Ok(PackageKind::Apkg),
        (Some("colpkg"), ImportMode::ReplaceColpkg) => Ok(PackageKind::Colpkg),
        (Some("apkg"), _) => Err(ImportError::InvalidPackage(
            "APKG files use merge semantics; select --merge".to_owned(),
        )),
        (Some("colpkg"), _) => Err(ImportError::InvalidPackage(
            "COLPKG files use replacement semantics; select --replace".to_owned(),
        )),
        _ => Err(ImportError::InvalidPackage(
            "expected an .apkg or .colpkg pathname".to_owned(),
        )),
    }
}

#[derive(Debug)]
struct Unpacked {
    collection: Vec<u8>,
    collection_member: String,
    media: BTreeMap<String, Vec<u8>>,
}

struct UniqueMediaMap(BTreeMap<String, String>);

impl<'de> Deserialize<'de> for UniqueMediaMap {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct MapVisitor;

        impl<'de> Visitor<'de> for MapVisitor {
            type Value = UniqueMediaMap;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a media filename map with unique string keys")
            }

            fn visit_map<A>(self, mut access: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut values = BTreeMap::new();
                while let Some((key, value)) = access.next_entry::<String, String>()? {
                    if values.insert(key.clone(), value).is_some() {
                        return Err(serde::de::Error::custom(format!(
                            "duplicate media map key {key:?}"
                        )));
                    }
                }
                Ok(UniqueMediaMap(values))
            }
        }

        deserializer.deserialize_map(MapVisitor)
    }
}

fn read_archive(path: &Path, _kind: PackageKind) -> Result<Unpacked, ImportError> {
    unpack_legacy_members(read_zip_members(path)?)
}

fn read_zip_members(path: &Path) -> Result<BTreeMap<String, Vec<u8>>, ImportError> {
    let file = File::open(path)?;
    let archive_bytes = file.metadata()?.len();
    if archive_bytes > MAX_ARCHIVE_BYTES {
        return Err(ImportError::InvalidPackage(
            "package file exceeds the compressed archive limit".to_owned(),
        ));
    }
    let mut archive = ZipArchive::new(file).map_err(|error| ImportError::Zip(error.to_string()))?;
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err(ImportError::InvalidPackage(format!(
            "{} entries exceed the {MAX_ARCHIVE_ENTRIES} entry limit",
            archive.len()
        )));
    }
    let mut names = BTreeSet::new();
    let mut members = BTreeMap::new();
    let mut total = 0_u64;
    for index in 0..archive.len() {
        let mut member = archive
            .by_index(index)
            .map_err(|error| ImportError::Zip(error.to_string()))?;
        let name = member.name().to_owned();
        validate_member_name(&name)?;
        if member.encrypted() {
            return Err(ImportError::InvalidPackage(format!(
                "encrypted archive member {name:?} is not supported"
            )));
        }
        if !matches!(
            member.compression(),
            CompressionMethod::Stored | CompressionMethod::Deflated
        ) {
            return Err(ImportError::InvalidPackage(format!(
                "archive member {name:?} uses an unsupported compression method"
            )));
        }
        if member.is_dir()
            || member
                .unix_mode()
                .is_some_and(|mode| mode & 0o170_000 == 0o120_000)
        {
            return Err(ImportError::InvalidPackage(format!(
                "directory or symlink member {name:?} is not allowed"
            )));
        }
        if !names.insert(name.clone()) {
            return Err(ImportError::InvalidPackage(format!(
                "duplicate archive member {name:?}"
            )));
        }
        let size = member.size();
        let compressed = member.compressed_size().max(1);
        let member_limit = if matches!(
            name.as_str(),
            "collection.anki2" | "collection.anki21" | "collection.anki21b"
        ) {
            MAX_COLLECTION_BYTES
        } else {
            MAX_MEDIA_BYTES
        };
        if size > member_limit || size > compressed.saturating_mul(MAX_EXPANSION_RATIO) {
            return Err(ImportError::InvalidPackage(format!(
                "member {name:?} exceeds the decompression limit"
            )));
        }
        total = total
            .checked_add(size)
            .ok_or_else(|| ImportError::InvalidPackage("archive size overflow".to_owned()))?;
        if total > MAX_EXPANDED_ARCHIVE_BYTES {
            return Err(ImportError::InvalidPackage(
                "expanded archive exceeds the host import limit".to_owned(),
            ));
        }
        let mut bytes = Vec::with_capacity(usize::try_from(size).unwrap_or(0));
        member.read_to_end(&mut bytes)?;
        if bytes.len() as u64 != size {
            return Err(ImportError::InvalidPackage(format!(
                "truncated member {name:?}"
            )));
        }
        members.insert(name, bytes);
    }
    Ok(members)
}

fn unpack_legacy_members(mut members: BTreeMap<String, Vec<u8>>) -> Result<Unpacked, ImportError> {
    if members.contains_key("meta") || members.contains_key("collection.anki21b") {
        return Err(ImportError::InvalidPackage(
            "modern meta/collection.anki21b packages use the protobuf media map and zstd collection path, which this bounded legacy adapter does not accept"
                .to_owned(),
        ));
    }
    let collection_names = ["collection.anki2", "collection.anki21"]
        .into_iter()
        .filter(|name| members.contains_key(*name))
        .collect::<Vec<_>>();
    if collection_names.len() != 1 {
        return Err(ImportError::InvalidPackage(
            "expected exactly one legacy SQLite collection.anki2 or collection.anki21 member"
                .to_owned(),
        ));
    }
    let collection_member = collection_names[0].to_owned();
    let collection = members
        .remove(&collection_member)
        .expect("legacy collection member was checked");
    if collection.len() as u64 > MAX_COLLECTION_BYTES {
        return Err(ImportError::InvalidPackage(
            "collection database exceeds the limit".to_owned(),
        ));
    }
    let media_map = members
        .remove("media")
        .ok_or_else(|| ImportError::InvalidPackage("media map is missing".to_owned()))?;
    let map = serde_json::from_slice::<UniqueMediaMap>(&media_map)
        .map_err(|error| ImportError::InvalidPackage(format!("media map: {error}")))?
        .0;
    let mut media = BTreeMap::new();
    for (number, raw_name) in map {
        if !is_numeric_member(&number) {
            return Err(ImportError::InvalidPackage(format!(
                "invalid media map key {number:?}"
            )));
        }
        let bytes = members.remove(&number).ok_or_else(|| {
            ImportError::InvalidPackage(format!("media map member {number:?} is missing"))
        })?;
        let name = canonical_media_name(&raw_name)?;
        validate_media_bytes(&name, &bytes)?;
        if media.insert(name.clone(), bytes).is_some() {
            return Err(ImportError::InvalidPackage(format!(
                "media names collide after normalization: {name:?}"
            )));
        }
    }
    if !members.is_empty() {
        let unexpected = members.keys().next().expect("not empty");
        return Err(ImportError::InvalidPackage(format!(
            "unexpected package member {unexpected:?}"
        )));
    }
    Ok(Unpacked {
        collection,
        collection_member,
        media,
    })
}

fn validate_member_name(name: &str) -> Result<(), ImportError> {
    if name.is_empty()
        || name.starts_with('/')
        || name.contains('\\')
        || name.split('/').any(|part| matches!(part, "" | "." | ".."))
        || name
            .bytes()
            .any(|byte| byte == 0 || byte.is_ascii_control())
    {
        return Err(ImportError::InvalidPackage(format!(
            "unsafe archive member {name:?}"
        )));
    }
    Ok(())
}

fn is_numeric_member(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|byte| byte.is_ascii_digit())
}

fn validate_media_bytes(name: &str, bytes: &[u8]) -> Result<(), ImportError> {
    if bytes.len() as u64 > MAX_MEDIA_BYTES {
        return Err(ImportError::InvalidPackage(format!(
            "media {name:?} exceeds the file limit"
        )));
    }
    let extension = name
        .rsplit_once('.')
        .map_or("", |(_, extension)| extension)
        .to_ascii_lowercase();
    if matches!(
        extension.as_str(),
        "js" | "mjs" | "cjs" | "html" | "htm" | "wasm"
    ) {
        return Err(ImportError::InvalidPackage(format!(
            "executable add-on media {name:?} is not supported"
        )));
    }
    let expected = media_type(name);
    let detected = sniff_media(bytes);
    if matches!(detected, Some("image/gif" | "image/webp"))
        || matches!(expected, "image/gif" | "image/webp")
    {
        return Err(ImportError::InvalidPackage(
            "GIF/WebP media is not advertised because the Kobo decoder is built for bounded PNG/JPEG only"
                .to_owned(),
        ));
    }
    if detected.is_some_and(|detected| detected.starts_with("image/") && detected != expected) {
        return Err(ImportError::InvalidPackage(format!(
            "image bytes do not match a supported filename extension for {name:?}"
        )));
    }
    if expected == "image/svg+xml" {
        validate_svg_source(bytes)?;
        return Ok(());
    }
    if expected.starts_with("image/") && detected.is_none() {
        return Err(ImportError::InvalidPackage(format!(
            "image {name:?} has no recognized signature"
        )));
    }
    if matches!(expected, "image/png" | "image/jpeg") {
        kobo_image::decode(bytes).map_err(|error| {
            ImportError::InvalidPackage(format!(
                "image media is outside the bounded Kobo decode path: {error}"
            ))
        })?;
    }

    Ok(())
}

fn sniff_media(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else if std::str::from_utf8(bytes).is_ok_and(|text| {
        let text = text.trim_start_matches('\u{feff}').trim_start();
        text.starts_with("<svg") || (text.starts_with("<?xml") && text.contains("<svg"))
    }) {
        Some("image/svg+xml")
    } else if bytes.starts_with(b"OggS") {
        Some("audio/ogg")
    } else if bytes.starts_with(b"ID3") {
        Some("audio/mpeg")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WAVE") {
        Some("audio/wav")
    } else {
        None
    }
}

fn read_collection(
    bytes: &[u8],
    media: &BTreeMap<String, Vec<u8>>,
    kind: PackageKind,
    collection_member: &str,
    output: &Path,
) -> Result<BundleManifest, ImportError> {
    let scratch = collection_scratch_path(output)?;
    remove_sqlite_scratch(&scratch)?;
    let result = (|| {
        if let Some(parent) = scratch.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&scratch)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        read_collection_file(&scratch, media, kind, collection_member)
    })();
    let cleanup = remove_sqlite_scratch(&scratch);
    match (result, cleanup) {
        (Ok(manifest), Ok(())) => Ok(manifest),
        (Err(error), Ok(())) | (_, Err(error)) => Err(error),
    }
}

fn remove_sqlite_scratch(path: &Path) -> Result<(), ImportError> {
    for suffix in ["", "-wal", "-shm", "-journal"] {
        match fs::remove_file(sqlite_sidecar_path(path, suffix)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(ImportError::Io(error)),
        }
    }
    Ok(())
}

fn sqlite_sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    if suffix.is_empty() {
        path.to_path_buf()
    } else {
        let mut name = path.as_os_str().to_os_string();
        name.push(suffix);
        PathBuf::from(name)
    }
}

fn collection_scratch_path(output: &Path) -> Result<PathBuf, ImportError> {
    let parent = publication_parent(output)?;
    let name = output
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            ImportError::InvalidPackage("output filename is not valid Unicode".to_owned())
        })?;
    Ok(parent.join(format!(".{name}.collection-check")))
}

fn read_collection_file(
    path: &Path,
    media: &BTreeMap<String, Vec<u8>>,
    kind: PackageKind,
    collection_member: &str,
) -> Result<BundleManifest, ImportError> {
    let mut source = read_original_source(path, kind, collection_member)?;
    if !SUPPORTED_COLLECTION_SCHEMAS.contains(&source.collection_schema) {
        let detail = if matches!(source.collection_schema, 12 | 13) {
            "schemas 12 and 13 require a clean downgrade and are explicitly refused by the pinned rslib"
        } else {
            "the pinned rslib only opens schema 11 and schemas 14 through 18"
        };
        return Err(ImportError::InvalidPackage(format!(
            "collection schema {} is outside the supported range: {detail}",
            source.collection_schema
        )));
    }

    let mut builder = CollectionBuilder::new(path);
    builder.set_check_integrity(true);
    let collection = builder.build().map_err(|error| {
        ImportError::InvalidPackage(format!(
            "pinned Anki rslib rejected collection migration/normalization: {error}"
        ))
    })?;
    collection.close(None).map_err(anki_import_error)?;
    let normalized_schema: i64 = readonly_database(path)
        .map_err(|error| import_stage("normalized schema open", error))?
        .query_row("SELECT ver FROM col LIMIT 1", [], |row| row.get(0))
        .map_err(|error| ImportError::Sql(format!("normalized schema: {error}")))?;
    if normalized_schema != NORMALIZED_SCHEMA {
        return Err(ImportError::InvalidPackage(format!(
            "pinned Anki rslib normalized to unexpected schema {normalized_schema}"
        )));
    }
    source.normalized_schema = normalized_schema;
    let (normalized_config, normalized_tags) = read_normalized_collection_metadata(path)
        .map_err(|error| import_stage("metadata", error))?;
    source.normalized_config = normalized_config;
    source.normalized_tags = normalized_tags;

    let mut collection = CollectionBuilder::new(path)
        .build()
        .map_err(anki_import_error)?;
    let notes =
        read_notes(collection.storage.db()).map_err(|error| import_stage("notes", error))?;
    let notes_by_id = notes
        .iter()
        .map(|note| (note.id, note.notetype_id))
        .collect::<BTreeMap<_, _>>();
    let card_rows =
        read_card_rows(collection.storage.db()).map_err(|error| import_stage("cards", error))?;
    let mut used_templates = BTreeMap::<i64, BTreeSet<i32>>::new();
    for row in &card_rows {
        let notetype_id = notes_by_id.get(&row.note_id).ok_or_else(|| {
            ImportError::InvalidPackage(format!(
                "card {} references missing note {}",
                row.id, row.note_id
            ))
        })?;
        used_templates
            .entry(*notetype_id)
            .or_default()
            .insert(row.ordinal);
    }
    let notetypes = read_notetypes(&mut collection, &used_templates)?;
    let decks = read_decks(&mut collection)?;
    let deck_configurations = read_deck_configurations(&collection)
        .map_err(|error| import_stage("deck configurations", error))?;
    let review_queue = read_review_queue(&mut collection, &decks, card_rows.len())?;
    let cards = read_cards(&mut collection, card_rows, &notes, media)
        .map_err(|error| import_stage("cards", error))?;
    let revlog =
        read_revlog(collection.storage.db()).map_err(|error| import_stage("revlog", error))?;
    let graves =
        read_graves(collection.storage.db()).map_err(|error| import_stage("graves", error))?;
    source.note_count = notes.len();
    source.card_count = cards.len();

    let mut manifest = BundleManifest::empty(source);
    manifest.notes = notes;
    manifest.notetypes = notetypes;
    manifest.decks = decks;
    manifest.deck_configurations = deck_configurations;
    manifest.cards = cards;
    manifest.review_queue = review_queue;
    manifest.revlog = revlog;
    manifest.graves = graves;
    for card in &manifest.cards {
        manifest
            .diagnostics
            .extend(card.diagnostics.iter().cloned());
    }
    Ok(manifest)
}

fn readonly_database(path: &Path) -> Result<Connection, ImportError> {
    let database = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| ImportError::Sql(error.to_string()))?;
    database
        .create_collation("unicase", |left, right| {
            UniCase::new(left).cmp(&UniCase::new(right))
        })
        .map_err(|error| ImportError::Sql(error.to_string()))?;
    Ok(database)
}

fn read_original_source(
    path: &Path,
    kind: PackageKind,
    collection_member: &str,
) -> Result<Source, ImportError> {
    let database = readonly_database(path)?;
    let integrity: String = database
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(|error| ImportError::Sql(error.to_string()))?;
    if integrity != "ok" {
        return Err(ImportError::InvalidPackage(
            "SQLite integrity_check did not return ok".to_owned(),
        ));
    }
    let mut source = database
        .query_row(
            "SELECT id, crt, mod, scm, ver, dty, usn, ls, conf, models, decks, dconf, tags FROM col LIMIT 1",
            [],
            |row| {
                Ok(Source {
                    package_kind: kind.name().to_owned(),
                    collection_member: collection_member.to_owned(),
                    collection_schema: row.get(4)?,
                    normalized_schema: 0,
                    collection_id: row.get(0)?,
                    collection_created: row.get(1)?,
                    collection_modified: row.get(2)?,
                    schema_modified: row.get(3)?,
                    dirty: row.get(5)?,
                    user_sequence: row.get(6)?,
                    last_sync: row.get(7)?,
                    note_count: 0,
                    card_count: 0,
                    converter_revision: CONVERTER_REVISION.to_owned(),
                    original_config_json: row.get(8)?,
                    original_models_json: row.get(9)?,
                    original_decks_json: row.get(10)?,
                    original_deck_configurations_json: row.get(11)?,
                    original_tags_json: row.get(12)?,
                    normalized_config: Vec::new(),
                    normalized_tags: Vec::new(),
                })
            },
        )
        .map_err(|error| ImportError::Sql(error.to_string()))?;
    source.original_config_json =
        canonical_json_text(&source.original_config_json, "collection configuration")?;
    source.original_models_json = canonical_json_text(&source.original_models_json, "notetypes")?;
    source.original_decks_json = canonical_json_text(&source.original_decks_json, "decks")?;
    source.original_deck_configurations_json = canonical_json_text(
        &source.original_deck_configurations_json,
        "deck configurations",
    )?;
    source.original_tags_json = canonical_json_text(&source.original_tags_json, "tags")?;
    source.note_count = database
        .query_row("SELECT count(*) FROM notes", [], |row| row.get(0))
        .map_err(|error| ImportError::Sql(error.to_string()))?;
    source.card_count = database
        .query_row("SELECT count(*) FROM cards", [], |row| row.get(0))
        .map_err(|error| ImportError::Sql(error.to_string()))?;
    if source.note_count > MAX_CARDS || source.card_count > MAX_CARDS {
        return Err(ImportError::InvalidPackage(
            "collection has more notes or cards than the bundle limit".to_owned(),
        ));
    }
    Ok(source)
}

fn canonical_json_text(text: &str, label: &str) -> Result<String, ImportError> {
    if text.is_empty() {
        return Ok(String::new());
    }
    let value: Value = serde_json::from_str(text)
        .map_err(|error| ImportError::InvalidPackage(format!("{label}: {error}")))?;
    Ok(serde_json::to_string(&canonical_json_value(value))
        .expect("JSON value serialization cannot fail"))
}

fn canonical_json_value(value: Value) -> Value {
    match value {
        Value::Array(values) => {
            Value::Array(values.into_iter().map(canonical_json_value).collect())
        }
        Value::Object(values) => {
            let mut ordered = serde_json::Map::new();
            let mut values = values.into_iter().collect::<Vec<_>>();
            values.sort_by(|left, right| left.0.cmp(&right.0));
            for (key, value) in values {
                ordered.insert(key, canonical_json_value(value));
            }
            Value::Object(ordered)
        }
        other => other,
    }
}

fn read_normalized_collection_metadata(
    path: &Path,
) -> Result<(Vec<CollectionConfig>, Vec<CollectionTag>), ImportError> {
    let database = readonly_database(path)?;
    let mut config_statement = database
        .prepare("SELECT key, usn, mtime_secs, hex(val) FROM config ORDER BY key")
        .map_err(|error| ImportError::Sql(error.to_string()))?;
    let config = config_statement
        .query_map([], |row| {
            Ok(CollectionConfig {
                key: row.get(0)?,
                user_sequence: row.get(1)?,
                modified: row.get(2)?,
                value_hex: row.get(3)?,
            })
        })
        .map_err(|error| ImportError::Sql(error.to_string()))?
        .map(|row| row.map_err(|error| ImportError::Sql(error.to_string())))
        .collect::<Result<Vec<_>, _>>()?;
    drop(config_statement);
    let mut tag_statement = database
        .prepare("SELECT tag, usn FROM tags ORDER BY tag")
        .map_err(|error| ImportError::Sql(error.to_string()))?;
    let mut tags = tag_statement
        .query_map([], |row| {
            Ok(CollectionTag {
                name: row.get(0)?,
                user_sequence: row.get(1)?,
            })
        })
        .map_err(|error| ImportError::Sql(error.to_string()))?
        .map(|row| row.map_err(|error| ImportError::Sql(error.to_string())))
        .collect::<Result<Vec<_>, _>>()?;
    tags.sort_by(|left, right| left.name.cmp(&right.name));
    Ok((config, tags))
}

fn read_notetypes(
    collection: &mut Collection,
    used_templates: &BTreeMap<i64, BTreeSet<i32>>,
) -> Result<Vec<NoteType>, ImportError> {
    let mut records = collection
        .get_all_notetypes()
        .map_err(anki_import_error)?
        .into_iter()
        .map(|notetype| {
            if let Some(ordinals) = used_templates.get(&notetype.id.0) {
                for ordinal in ordinals {
                    let template = notetype
                        .get_template(u16::try_from(*ordinal).unwrap_or(u16::MAX))
                        .map_err(anki_import_error)?;
                    validate_card_template(template)?;
                }
            }
            let schema11: NotetypeSchema11 = (*notetype).clone().into();
            Ok(NoteType {
                id: notetype.id.0,
                name: notetype.name.clone(),
                original_json: canonical_json_text(
                    &serde_json::to_string(&schema11)
                        .expect("Anki schema-11 notetype serialization cannot fail"),
                    "normalized notetype",
                )?,
            })
        })
        .collect::<Result<Vec<_>, ImportError>>()?;
    records.sort_by_key(|record| record.id);
    Ok(records)
}

fn validate_card_template(template: &anki::notetype::CardTemplate) -> Result<(), ImportError> {
    for side in [&template.config.q_format, &template.config.a_format] {
        let lower = side.to_ascii_lowercase();
        if lower.contains("<script")
            || lower.contains("javascript:")
            || lower.contains("vbscript:")
            || lower.contains("<iframe")
            || lower.contains("<object")
            || lower.contains("<embed")
            || lower.contains("<link")
            || lower.contains("<meta http-equiv")
            || has_html_event_handler(&lower)
        {
            return Err(ImportError::InvalidPackage(
                "card templates containing JavaScript, active HTML, or event handlers are not supported"
                    .to_owned(),
            ));
        }
    }
    Ok(())
}

fn has_html_event_handler(lower: &str) -> bool {
    let mut remaining = lower;
    while let Some(start) = remaining.find('<') {
        let tag = &remaining[start + 1..];
        let Some(end) = html_tag_end(tag) else {
            return false;
        };
        let body = &tag[..end];
        if !body.starts_with("!--") && tag_contains_event_handler(body) {
            return true;
        }
        remaining = &tag[end + 1..];
    }
    false
}

fn html_tag_end(tag: &str) -> Option<usize> {
    let mut quote = None;
    for (offset, character) in tag.char_indices() {
        if let Some(open) = quote {
            if character == open {
                quote = None;
            }
        } else if matches!(character, '"' | '\'') {
            quote = Some(character);
        } else if character == '>' {
            return Some(offset);
        }
    }
    None
}

fn tag_contains_event_handler(tag: &str) -> bool {
    let bytes = tag.as_bytes();
    let mut index = 0;
    let mut quote = None;
    while index < bytes.len() {
        let byte = bytes[index];
        if let Some(open) = quote {
            if byte == open {
                quote = None;
            }
            index += 1;
            continue;
        }
        if matches!(byte, b'"' | b'\'') {
            quote = Some(byte);
            index += 1;
            continue;
        }
        let at_attribute = index > 0
            && bytes[index - 1].is_ascii_whitespace()
            && bytes.get(index..index + 2) == Some(b"on");
        if at_attribute {
            let mut end = index + 2;
            while bytes.get(end).is_some_and(u8::is_ascii_alphabetic) {
                end += 1;
            }
            if end > index + 2 {
                while bytes.get(end).is_some_and(u8::is_ascii_whitespace) {
                    end += 1;
                }
                if bytes.get(end) == Some(&b'=') {
                    return true;
                }
            }
        }
        index += 1;
    }
    false
}

fn read_decks(collection: &mut Collection) -> Result<Vec<Deck>, ImportError> {
    let names = collection
        .get_all_deck_names(false)
        .map_err(anki_import_error)?;
    let mut records = Vec::with_capacity(names.len());
    for (id, _) in names {
        let deck = collection
            .get_deck(id)
            .map_err(anki_import_error)?
            .ok_or_else(|| {
                ImportError::InvalidPackage(format!("pinned rslib lost deck {}", id.0))
            })?;
        let schema11: DeckSchema11 = (*deck).clone().into();
        records.push(Deck {
            id: deck.id.0,
            name: deck.name.human_name(),
            configuration_id: deck.config_id().map(|id| id.0),
            original_json: canonical_json_text(
                &serde_json::to_string(&schema11)
                    .expect("Anki schema-11 deck serialization cannot fail"),
                "normalized deck",
            )?,
        });
    }
    records.sort_by_key(|record| record.id);
    Ok(records)
}

fn read_deck_configurations(
    collection: &Collection,
) -> Result<Vec<DeckConfiguration>, ImportError> {
    let mut statement = collection
        .storage
        .db()
        .prepare("SELECT id FROM deck_config ORDER BY id")
        .map_err(|error| ImportError::Sql(error.to_string()))?;
    let ids = statement
        .query_map([], |row| row.get::<_, i64>(0))
        .map_err(|error| ImportError::Sql(error.to_string()))?
        .map(|row| row.map_err(|error| ImportError::Sql(error.to_string())))
        .collect::<Result<Vec<_>, _>>()?;
    let mut records = Vec::with_capacity(ids.len());
    for id in ids {
        let configuration = collection
            .get_deck_config(DeckConfigId(id), false)
            .map_err(anki_import_error)?
            .ok_or_else(|| {
                ImportError::InvalidPackage(format!("pinned rslib lost deck configuration {id}"))
            })?;
        let schema11: DeckConfSchema11 = configuration.clone().into();
        records.push(DeckConfiguration {
            id,
            name: configuration.name,
            original_json: canonical_json_text(
                &serde_json::to_string(&schema11)
                    .expect("Anki schema-11 deck configuration serialization cannot fail"),
                "normalized deck configuration",
            )?,
        });
    }
    Ok(records)
}

fn read_notes(database: &Connection) -> Result<Vec<BundleNote>, ImportError> {
    let mut statement = database
        .prepare(
            "SELECT id, guid, mid, mod, usn, tags, flds, sfld, csum, flags, data FROM notes ORDER BY id",
        )
        .map_err(|error| ImportError::Sql(error.to_string()))?;
    let rows = statement
        .query_map([], |row| {
            Ok(BundleNote {
                id: row.get(0)?,
                guid: row.get(1)?,
                notetype_id: row.get(2)?,
                modified: row.get(3)?,
                user_sequence: row.get(4)?,
                tags: row
                    .get::<_, String>(5)?
                    .split_whitespace()
                    .map(ToOwned::to_owned)
                    .collect(),
                fields: row
                    .get::<_, String>(6)?
                    .split('\u{1f}')
                    .map(ToOwned::to_owned)
                    .collect(),
                sort_field: sqlite_value_as_string(row.get_ref(7)?),
                checksum: row.get(8)?,
                flags: row.get(9)?,
                data: row.get(10)?,
            })
        })
        .map_err(|error| ImportError::Sql(error.to_string()))?;
    rows.map(|row| row.map_err(|error| ImportError::Sql(error.to_string())))
        .collect()
}

fn sqlite_value_as_string(value: rusqlite::types::ValueRef<'_>) -> String {
    match value {
        rusqlite::types::ValueRef::Null => String::new(),
        rusqlite::types::ValueRef::Integer(value) => value.to_string(),
        rusqlite::types::ValueRef::Real(value) => value.to_string(),
        rusqlite::types::ValueRef::Text(value) => String::from_utf8_lossy(value).into_owned(),
        rusqlite::types::ValueRef::Blob(value) => hex_bytes(value),
    }
}

fn read_review_queue(
    collection: &mut Collection,
    decks: &[Deck],
    total_cards: usize,
) -> Result<ReviewQueue, ImportError> {
    let deck_names = decks
        .iter()
        .map(|deck| deck.name.as_str())
        .collect::<BTreeSet<_>>();
    let root_ids = decks
        .iter()
        .filter(|deck| {
            deck.name
                .rsplit_once("::")
                .is_none_or(|(parent, _)| !deck_names.contains(parent))
        })
        .map(|deck| deck.id)
        .collect::<Vec<_>>();
    let mut queue = ReviewQueue::default();
    let mut seen = BTreeSet::new();
    for root_id in root_ids {
        let _changes = collection
            .set_current_deck(DeckId(root_id))
            .map_err(anki_import_error)?;
        let queued = anki::services::SchedulerService::get_queued_cards(
            collection,
            anki_proto::scheduler::GetQueuedCardsRequest {
                fetch_limit: u32::try_from(total_cards.min(MAX_CARDS)).unwrap_or(u32::MAX),
                intraday_learning_only: false,
            },
        )
        .map_err(anki_import_error)?;
        let ids = queued
            .cards
            .into_iter()
            .filter_map(|entry| entry.card.map(|card| card.id))
            .collect::<Vec<_>>();
        if ids.iter().any(|id| !seen.insert(*id)) {
            return Err(ImportError::InvalidPackage(
                "pinned rslib returned a card in more than one root-deck queue".to_owned(),
            ));
        }
        let new_count = usize::try_from(queued.new_count).unwrap_or(usize::MAX);
        let learning_count = usize::try_from(queued.learning_count).unwrap_or(usize::MAX);
        let review_count = usize::try_from(queued.review_count).unwrap_or(usize::MAX);
        if ids.is_empty() && new_count == 0 && learning_count == 0 && review_count == 0 {
            continue;
        }
        queue.card_ids.extend_from_slice(&ids);
        if queue.card_ids.len() > MAX_REVIEW_QUEUE_CARDS {
            return Err(ImportError::InvalidPackage(format!(
                "upstream due queue has more than {MAX_REVIEW_QUEUE_CARDS} cards; lower deck limits before export"
            )));
        }
        queue.new_count = queue.new_count.saturating_add(new_count);
        queue.learning_count = queue.learning_count.saturating_add(learning_count);
        queue.review_count = queue.review_count.saturating_add(review_count);
        queue.decks.push(DeckQueue {
            source_index: 0,
            root_deck_id: root_id,
            card_ids: ids,
            new_count,
            learning_count,
            review_count,
        });
    }
    Ok(queue)
}

struct CardRow {
    id: i64,
    note_id: i64,
    deck_id: i64,
    ordinal: i32,
    modified: i64,
    user_sequence: i32,
    card_type: i32,
    queue: i32,
    due: i64,
    interval: i32,
    ease_factor: i32,
    repetitions: i32,
    lapses: i32,
    remaining_steps: i32,
    original_due: i64,
    original_deck_id: i64,
    flags: i32,
    data: String,
}

fn read_card_rows(database: &Connection) -> Result<Vec<CardRow>, ImportError> {
    let mut statement = database
        .prepare(
            "SELECT id, nid, did, ord, mod, usn, type, queue, due, ivl, factor, reps, lapses, left, odue, odid, flags, data FROM cards ORDER BY id",
        )
        .map_err(|error| ImportError::Sql(error.to_string()))?;
    let rows = statement
        .query_map([], |row| {
            Ok(CardRow {
                id: row.get(0)?,
                note_id: row.get(1)?,
                deck_id: row.get(2)?,
                ordinal: row.get(3)?,
                modified: row.get(4)?,
                user_sequence: row.get(5)?,
                card_type: row.get(6)?,
                queue: row.get(7)?,
                due: row.get(8)?,
                interval: row.get(9)?,
                ease_factor: row.get(10)?,
                repetitions: row.get(11)?,
                lapses: row.get(12)?,
                remaining_steps: row.get(13)?,
                original_due: row.get(14)?,
                original_deck_id: row.get(15)?,
                flags: row.get(16)?,
                data: row.get(17)?,
            })
        })
        .map_err(|error| ImportError::Sql(error.to_string()))?;
    rows.map(|row| row.map_err(|error| ImportError::Sql(error.to_string())))
        .collect()
}

fn read_cards(
    collection: &mut Collection,
    rows: Vec<CardRow>,
    notes: &[BundleNote],
    media: &BTreeMap<String, Vec<u8>>,
) -> Result<Vec<Card>, ImportError> {
    let notes = notes
        .iter()
        .map(|note| (note.id, note))
        .collect::<BTreeMap<_, _>>();
    rows.into_iter()
        .map(|row| read_card(collection, row, &notes, media))
        .collect()
}

fn read_card(
    collection: &mut Collection,
    row: CardRow,
    notes: &BTreeMap<i64, &BundleNote>,
    media: &BTreeMap<String, Vec<u8>>,
) -> Result<Card, ImportError> {
    let note = notes.get(&row.note_id).ok_or_else(|| {
        ImportError::InvalidPackage(format!(
            "card {} references missing note {}",
            row.id, row.note_id
        ))
    })?;
    let notetype = collection
        .get_notetype(anki::notetype::NotetypeId(note.notetype_id))
        .map_err(anki_import_error)?
        .ok_or_else(|| {
            ImportError::InvalidPackage(format!("note {} has no usable notetype", row.note_id))
        })?;
    let template = notetype
        .get_template(u16::try_from(row.ordinal).unwrap_or(u16::MAX))
        .map_err(anki_import_error)?;
    let template_name = template.name.clone();
    let type_answer = template_uses_type_answer(&template.config.q_format)
        || template_uses_type_answer(&template.config.a_format);
    let mut diagnostics = Vec::new();
    let (front, back) = render_existing_with_rslib(
        collection,
        row.id,
        &notetype.config.css,
        type_answer,
        &mut diagnostics,
    )?;
    let question_references = front.media;
    let answer_references = back.media;
    ensure_single_image_reference(&question_references)?;
    ensure_single_image_reference(&answer_references)?;
    let question_media_names = sorted_unique_references(question_references);
    let answer_media_names = sorted_unique_references(answer_references);
    let media_names = question_media_names
        .iter()
        .chain(&answer_media_names)
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if media_names.iter().any(|name| !media.contains_key(name)) {
        return Err(ImportError::InvalidPackage(
            "a rendered card references media absent from the package".to_owned(),
        ));
    }
    let attachments = media_names
        .iter()
        .map(|name| attachment(name, &media[name]))
        .collect::<Vec<_>>();
    if attachments
        .iter()
        .any(|attachment| attachment.kind == AttachmentKind::Other)
    {
        return Err(ImportError::InvalidPackage(
            "a rendered card references media the Kobo cannot display or identify as a non-playing audio/video attachment"
                .to_owned(),
        ));
    }
    Ok(Card {
        id: row.id,
        note_id: row.note_id,
        deck_id: row.deck_id,
        ordinal: row.ordinal,
        user_sequence: row.user_sequence,
        queue: row.queue,
        card_type: row.card_type,
        due: row.due,
        interval: row.interval,
        ease_factor: row.ease_factor,
        repetitions: row.repetitions,
        lapses: row.lapses,
        remaining_steps: row.remaining_steps,
        original_due: row.original_due,
        original_deck_id: row.original_deck_id,
        flags: row.flags,
        data: row.data,
        modified: row.modified,
        template_name,
        front: front.text,
        back: back.text,
        front_spans: front.spans,
        back_spans: back.spans,
        tags: note.tags.clone(),
        question_media_names,
        answer_media_names,
        media_names,
        attachments,
        diagnostics,
    })
}

struct RenderedSide {
    text: String,
    spans: Vec<CardTextSpan>,
    media: Vec<String>,
}

fn render_existing_with_rslib(
    collection: &mut Collection,
    card_id: i64,
    css: &str,
    type_answer: bool,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<(RenderedSide, RenderedSide), ImportError> {
    let partial = anki::services::CardRenderingService::render_existing_card(
        collection,
        anki_proto::card_rendering::RenderExistingCardRequest {
            card_id,
            browser: false,
            partial_render: true,
        },
    )
    .map_err(anki_import_error)?;
    reject_unresolved_filters(&partial.question_nodes)?;
    reject_unresolved_filters(&partial.answer_nodes)?;

    let rendered = anki::services::CardRenderingService::render_existing_card(
        collection,
        anki_proto::card_rendering::RenderExistingCardRequest {
            card_id,
            browser: false,
            partial_render: false,
        },
    )
    .map_err(anki_import_error)?;
    let question = rendered_nodes_text(rendered.question_nodes)?;
    let answer = rendered_nodes_text(rendered.answer_nodes)?;
    if type_answer {
        diagnostics.push(diagnostic(
            "type-answer-noninteractive",
            "Anki type-answer markers are retained as noninteractive text.",
        ));
    }
    let question_media = upstream_media_references(collection, &question)?;
    let answer_media = upstream_media_references(collection, &answer)?;
    let question_text = kobo_html::to_text(&question);
    let answer_text = kobo_html::to_text(&answer);
    let question_spans = rendered_emphasis(&question, css, &question_text)?;
    let answer_spans = rendered_emphasis(&answer, css, &answer_text)?;
    Ok((
        RenderedSide {
            text: annotate_type_answer(&question_text, type_answer),
            spans: question_spans,
            media: question_media,
        },
        RenderedSide {
            text: annotate_type_answer(&answer_text, type_answer),
            spans: answer_spans,
            media: answer_media,
        },
    ))
}

fn rendered_emphasis(html: &str, css: &str, plain: &str) -> Result<Vec<CardTextSpan>, ImportError> {
    let document = kobo_doc::html::parse_with_css(html, css);
    if document.truncated {
        return Err(ImportError::InvalidPackage(
            "rendered card styling exceeds the bounded document parser".to_owned(),
        ));
    }
    let mut spans: Vec<CardTextSpan> = Vec::new();
    let mut cursor = 0;
    for rich in document.rich.values() {
        for source in &rich.spans {
            let source_text = source.text.trim();
            if source_text.is_empty() {
                continue;
            }
            let style = CardTextStyle {
                strong: source.style.strong,
                emphasis: source.style.emphasis,
                underline: source.style.underline,
                superscript: source.style.superscript,
                subscript: source.style.subscript,
            };
            let Some((start, end)) = find_without_whitespace(plain, cursor, source_text) else {
                if !style.is_plain() && !source.text.trim().is_empty() {
                    return Err(ImportError::InvalidPackage(
                        "rendered card emphasis cannot be mapped safely to device text".to_owned(),
                    ));
                }
                continue;
            };
            cursor = end;
            if style.is_plain() {
                continue;
            }
            if let Some(previous) = spans.last_mut() {
                if previous.end == start && previous.style == style {
                    previous.end = end;
                    continue;
                }
            }
            spans.push(CardTextSpan { start, end, style });
            if spans.len() > MAX_CARD_TEXT_SPANS {
                return Err(ImportError::InvalidPackage(
                    "rendered card has too many emphasis spans".to_owned(),
                ));
            }
        }
    }
    Ok(spans)
}

fn find_without_whitespace(haystack: &str, from: usize, needle: &str) -> Option<(usize, usize)> {
    let wanted = needle
        .char_indices()
        .filter_map(|(_, character)| (!character.is_whitespace()).then_some(character))
        .collect::<Vec<_>>();
    if wanted.is_empty() {
        return None;
    }
    let available = haystack
        .get(from..)?
        .char_indices()
        .filter_map(|(offset, character)| {
            (!character.is_whitespace()).then_some((
                character,
                from + offset,
                from + offset + character.len_utf8(),
            ))
        })
        .collect::<Vec<_>>();
    available
        .windows(wanted.len())
        .find(|window| {
            window
                .iter()
                .map(|(character, _, _)| *character)
                .eq(wanted.iter().copied())
        })
        .map(|window| {
            (
                window.first().expect("non-empty wanted").1,
                window.last().expect("non-empty wanted").2,
            )
        })
}

fn reject_unresolved_filters(
    nodes: &[anki_proto::card_rendering::RenderedTemplateNode],
) -> Result<(), ImportError> {
    for node in nodes {
        if let Some(anki_proto::card_rendering::rendered_template_node::Value::Replacement(
            replacement,
        )) = &node.value
        {
            if replacement.field_name != "FrontSide" || !replacement.filters.is_empty() {
                return Err(ImportError::InvalidPackage(
                    "card uses an add-on template filter that pinned Anki rslib leaves for external code"
                        .to_owned(),
                ));
            }
        }
    }
    Ok(())
}

fn rendered_nodes_text(
    nodes: Vec<anki_proto::card_rendering::RenderedTemplateNode>,
) -> Result<String, ImportError> {
    let mut text = String::new();
    for node in nodes {
        match node.value {
            Some(anki_proto::card_rendering::rendered_template_node::Value::Text(value)) => {
                text.push_str(&value);
            }
            Some(anki_proto::card_rendering::rendered_template_node::Value::Replacement(_)) => {
                return Err(ImportError::InvalidPackage(
                    "pinned Anki rslib did not fully render a card".to_owned(),
                ));
            }
            None => {
                return Err(ImportError::InvalidPackage(
                    "pinned Anki rslib returned an empty render node".to_owned(),
                ));
            }
        }
    }
    Ok(text)
}

fn upstream_media_references(
    collection: &mut Collection,
    html: &str,
) -> Result<Vec<String>, ImportError> {
    let references = MediaService::extract_media_files(
        collection,
        generic::String {
            val: html.to_owned(),
        },
    )
    .map_err(anki_import_error)?;
    references
        .vals
        .into_iter()
        .map(|name| canonical_media_name(&name).map_err(ImportError::from))
        .collect()
}

fn ensure_single_image_reference(references: &[String]) -> Result<(), ImportError> {
    if references
        .iter()
        .filter(|name| media_type(name).starts_with("image/"))
        .count()
        > 1
    {
        return Err(ImportError::InvalidPackage(
            "a card side renders more than one image; the single-image device layout refuses to drop or reorder content"
                .to_owned(),
        ));
    }
    Ok(())
}

fn sorted_unique_references(references: Vec<String>) -> Vec<String> {
    references
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn annotate_type_answer(text: &str, type_answer: bool) -> String {
    if type_answer {
        format!("{text}\n\n{TYPE_ANSWER_PLACEHOLDER}")
    } else {
        text.to_owned()
    }
}

fn template_uses_type_answer(template: &str) -> bool {
    let mut remaining = template;
    while let Some(start) = remaining.find("{{") {
        let after = &remaining[start + 2..];
        let Some(end) = after.find("}}") else {
            return false;
        };
        let token = after[..end].trim();
        if !token.starts_with(['#', '^', '/', '!']) {
            let mut parts = token.rsplit(':');
            let _field = parts.next();
            if parts.any(|filter| filter.trim() == "type") {
                return true;
            }
        }
        remaining = &after[end + 2..];
    }
    false
}

fn read_revlog(database: &Connection) -> Result<Vec<ReviewLog>, ImportError> {
    let exists: bool = database
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'revlog')",
            [],
            |row| row.get(0),
        )
        .map_err(|error| ImportError::Sql(error.to_string()))?;
    if !exists {
        return Ok(Vec::new());
    }
    let mut statement = database
        .prepare(
            "SELECT id, cid, usn, ease, ivl, lastIvl, factor, time, type FROM revlog ORDER BY id",
        )
        .map_err(|error| ImportError::Sql(error.to_string()))?;
    let rows = statement
        .query_map([], |row| {
            Ok(ReviewLog {
                id: row.get(0)?,
                card_id: row.get(1)?,
                user_sequence: row.get(2)?,
                ease: row.get(3)?,
                interval: row.get(4)?,
                last_interval: row.get(5)?,
                ease_factor: row.get(6)?,
                milliseconds: row.get(7)?,
                review_kind: row.get(8)?,
            })
        })
        .map_err(|error| ImportError::Sql(error.to_string()))?;
    rows.map(|row| row.map_err(|error| ImportError::Sql(error.to_string())))
        .collect()
}

fn read_graves(database: &Connection) -> Result<Vec<Grave>, ImportError> {
    let mut statement = database
        .prepare("SELECT oid, type, usn FROM graves ORDER BY oid, type")
        .map_err(|error| ImportError::Sql(error.to_string()))?;
    let rows = statement
        .query_map([], |row| {
            Ok(Grave {
                object_id: row.get(0)?,
                object_kind: row.get(1)?,
                user_sequence: row.get(2)?,
            })
        })
        .map_err(|error| ImportError::Sql(error.to_string()))?;
    rows.map(|row| row.map_err(|error| ImportError::Sql(error.to_string())))
        .collect()
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "the function is passed directly to Result::map_err"
)]
fn anki_import_error(error: anki::error::AnkiError) -> ImportError {
    ImportError::InvalidPackage(format!(
        "pinned Anki rslib rejected the collection: {error}"
    ))
}

fn import_stage(stage: &str, error: ImportError) -> ImportError {
    match error {
        ImportError::Sql(message) => ImportError::Sql(format!("{stage}: {message}")),
        other => other,
    }
}

fn hex_bytes(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ignored = write!(output, "{byte:02x}");
    }
    output
}

#[cfg(test)]
fn render_template(
    template: &str,
    fields: &BTreeMap<String, String>,
    tags: &[String],
    ordinal: i32,
    front: Option<&str>,
    show_answer: bool,
    diagnostics: &mut Vec<Diagnostic>,
) -> String {
    let conditional = render_conditionals(template, fields);
    let mut output = String::new();
    let mut remaining = conditional.as_str();
    while let Some(start) = remaining.find("{{") {
        output.push_str(&remaining[..start]);
        let after = &remaining[start + 2..];
        let Some(end) = after.find("}}") else {
            diagnostics.push(diagnostic(
                "template-unclosed",
                "template has an unclosed substitution",
            ));
            output.push_str(&remaining[start..]);
            remaining = "";
            break;
        };
        let token = after[..end].trim();
        output.push_str(&substitution(
            token,
            fields,
            tags,
            ordinal,
            front,
            show_answer,
            diagnostics,
        ));
        remaining = &after[end + 2..];
    }
    output.push_str(remaining);
    kobo_html::to_text(&output)
}

#[cfg(test)]
fn render_conditionals(template: &str, fields: &BTreeMap<String, String>) -> String {
    let mut result = template.to_owned();
    // A bounded pass handles ordinary nested conditionals while refusing a
    // deliberately pathological template to turn import into unbounded work.
    for _ in 0..16 {
        let Some(start) = result.rfind("{{#") else {
            break;
        };
        let Some(open_end) = result[start..].find("}}") else {
            break;
        };
        let name = result[start + 3..start + open_end].trim();
        let close = format!("{{{{/{name}}}}}");
        let after_open = start + open_end + 2;
        let Some(close_start) = result[after_open..]
            .find(&close)
            .map(|index| after_open + index)
        else {
            break;
        };
        let content = result[after_open..close_start].to_owned();
        let replacement = if fields
            .get(name)
            .is_some_and(|value| !value.trim().is_empty())
        {
            content
        } else {
            String::new()
        };
        result.replace_range(start..close_start + close.len(), &replacement);
    }
    for _ in 0..16 {
        let Some(start) = result.rfind("{{^") else {
            break;
        };
        let Some(open_end) = result[start..].find("}}") else {
            break;
        };
        let name = result[start + 3..start + open_end].trim();
        let close = format!("{{{{/{name}}}}}");
        let after_open = start + open_end + 2;
        let Some(close_start) = result[after_open..]
            .find(&close)
            .map(|index| after_open + index)
        else {
            break;
        };
        let content = result[after_open..close_start].to_owned();
        let replacement = if fields.get(name).is_none_or(|value| value.trim().is_empty()) {
            content
        } else {
            String::new()
        };
        result.replace_range(start..close_start + close.len(), &replacement);
    }
    result
}

#[cfg(test)]
fn substitution(
    token: &str,
    fields: &BTreeMap<String, String>,
    tags: &[String],
    ordinal: i32,
    front: Option<&str>,
    show_answer: bool,
    diagnostics: &mut Vec<Diagnostic>,
) -> String {
    if token == "FrontSide" {
        return front.unwrap_or_default().to_owned();
    }
    if token == "Tags" {
        return tags.join(" ");
    }
    let (filter, field) = token
        .split_once(':')
        .map_or(("", token), |(filter, field)| (filter, field));
    let value = fields.get(field.trim()).cloned().unwrap_or_default();
    match filter {
        "" => value,
        "text" => kobo_html::to_text(&value),
        "hint" => {
            if value.trim().is_empty() {
                String::new()
            } else {
                "[Show hint]".to_owned()
            }
        }
        "cloze" => render_cloze(&value, ordinal, show_answer),
        unsupported => {
            diagnostics.push(diagnostic(
                "unsupported-filter",
                &format!("Anki template filter {unsupported:?} was not rendered"),
            ));
            format!("[Unsupported filter: {unsupported}]")
        }
    }
}

#[cfg(test)]
fn render_cloze(value: &str, ordinal: i32, show_answer: bool) -> String {
    let marker = format!("{{{{c{}::", ordinal + 1);
    let mut rendered = String::new();
    let mut remaining = value;
    while let Some(start) = remaining.find("{{c") {
        rendered.push_str(&remaining[..start]);
        let rest = &remaining[start..];
        let Some(end) = rest.find("}}") else {
            rendered.push_str(rest);
            return rendered;
        };
        let body = &rest[2..end];
        let mut parts = body.splitn(3, "::");
        let index = parts.next().unwrap_or_default();
        let answer = parts.next().unwrap_or_default();
        let hint = parts.next().unwrap_or_default();
        let replacement = if rest.starts_with(&marker) && !show_answer {
            if hint.is_empty() {
                "[...]".to_owned()
            } else {
                format!("[{hint}]")
            }
        } else if index.starts_with('c') {
            answer.to_owned()
        } else {
            rest[..end + 2].to_owned()
        };
        rendered.push_str(&replacement);
        remaining = &rest[end + 2..];
    }
    rendered.push_str(remaining);
    rendered
}

#[cfg(test)]
fn referenced_media(fields: &BTreeMap<String, String>) -> Vec<String> {
    let mut names = BTreeSet::new();
    for value in fields.values() {
        for name in sound_references(value)
            .into_iter()
            .chain(image_references(value))
        {
            if let Ok(name) = canonical_media_name(&name) {
                names.insert(name);
            }
        }
    }
    names.into_iter().collect()
}

#[cfg(test)]
fn sound_references(value: &str) -> Vec<String> {
    let mut references = Vec::new();
    let mut remaining = value;
    while let Some(start) = remaining.find("[sound:") {
        let after = &remaining[start + 7..];
        let Some(end) = after.find(']') else {
            break;
        };
        references.push(after[..end].to_owned());
        remaining = &after[end + 1..];
    }
    references
}

#[cfg(test)]
fn image_references(value: &str) -> Vec<String> {
    let mut references = Vec::new();
    let lower = value.to_ascii_lowercase();
    let mut offset = 0;
    while let Some(start) = lower[offset..].find("<img") {
        let start = offset + start;
        let Some(end) = lower[start..].find('>').map(|index| start + index) else {
            break;
        };
        let original = &value[start..end];
        for quote in ['"', '\''] {
            let needle = format!("src={quote}");
            if let Some(attribute) = original.to_ascii_lowercase().find(&needle) {
                let from = attribute + needle.len();
                if let Some(close) = original[from..].find(quote) {
                    references.push(original[from..from + close].to_owned());
                }
                break;
            }
        }
        offset = end + 1;
    }
    references
}

fn attachment(name: &str, _bytes: &[u8]) -> Attachment {
    let mime = media_type(name).to_owned();
    let kind = if mime.starts_with("image/") {
        AttachmentKind::Image
    } else if mime.starts_with("audio/") {
        AttachmentKind::Audio
    } else if mime.starts_with("video/") {
        AttachmentKind::Video
    } else {
        AttachmentKind::Other
    };
    Attachment {
        name: name.to_owned(),
        rendered_name: None,
        mime,
        kind,
    }
}

fn prepare_rendered_media(
    manifest: &mut BundleManifest,
    media: &mut BTreeMap<String, Vec<u8>>,
) -> Result<(), ImportError> {
    let mut rendered: BTreeMap<String, String> = BTreeMap::new();
    for card in &mut manifest.cards {
        for attachment in &mut card.attachments {
            if attachment.mime != "image/svg+xml" {
                continue;
            }
            let rendered_name = if let Some(name) = rendered.get(&attachment.name) {
                name.clone()
            } else {
                let source = media.get(&attachment.name).ok_or_else(|| {
                    ImportError::InvalidPackage(
                        "referenced SVG disappeared before rasterization".to_owned(),
                    )
                })?;
                let png = rasterize_svg(source)?;
                validate_media_bytes("cobalt-rendered.png", &png)?;
                let name = format!("cobalt-svg-{}.png", digest_hex(source));
                if let Some(existing) = media.get(&name) {
                    if existing != &png {
                        return Err(ImportError::InvalidPackage(
                            "generated SVG raster name conflicts with package media".to_owned(),
                        ));
                    }
                } else {
                    media.insert(name.clone(), png);
                }
                rendered.insert(attachment.name.clone(), name.clone());
                name
            };
            attachment.rendered_name = Some(rendered_name);
        }
    }
    let total = media.values().try_fold(0_usize, |total, bytes| {
        total
            .checked_add(bytes.len())
            .ok_or_else(|| ImportError::InvalidPackage("media payload size overflow".to_owned()))
    })?;
    if media.len() > MAX_MEDIA_ENTRIES || total > MAX_PAYLOAD_BYTES {
        return Err(ImportError::InvalidPackage(format!(
            "rendered media payload has {} files and {total} bytes; limits are {MAX_MEDIA_ENTRIES} files and {MAX_PAYLOAD_BYTES} bytes",
            media.len()
        )));
    }
    Ok(())
}

fn diagnostic(code: &str, message: &str) -> Diagnostic {
    Diagnostic {
        code: code.to_owned(),
        message: message.to_owned(),
    }
}

fn merge_existing(
    mut incoming: BundleManifest,
    mut incoming_media: BTreeMap<String, Vec<u8>>,
    existing_path: Option<&Path>,
) -> Result<(BundleManifest, BTreeMap<String, Vec<u8>>), ImportError> {
    let Some(existing_path) = existing_path else {
        return Ok((incoming, incoming_media));
    };
    let existing_bytes = fs::read(existing_path)?;
    let existing = verify_bundle_bytes(&existing_bytes)?;
    let mut merged = existing.manifest().clone();
    let existing_card_ids = merged
        .cards
        .iter()
        .map(|card| card.id)
        .collect::<BTreeSet<_>>();
    if incoming
        .cards
        .iter()
        .any(|card| existing_card_ids.contains(&card.id))
    {
        return Err(ImportError::InvalidPackage(
            "APKG card identifier conflict: existing cards are never overwritten or deduplicated"
                .to_owned(),
        ));
    }

    let source_offset = merged.sources.len();
    for deck_queue in &mut incoming.review_queue.decks {
        deck_queue.source_index = deck_queue.source_index.saturating_add(source_offset);
    }
    merged.sources.append(&mut incoming.sources);
    merge_exact_records(&mut merged.notes, incoming.notes, |note| note.id, "note")?;
    merge_exact_records(
        &mut merged.notetypes,
        incoming.notetypes,
        |notetype| notetype.id,
        "notetype",
    )?;
    merge_exact_records(&mut merged.decks, incoming.decks, |deck| deck.id, "deck")?;
    merge_exact_records(
        &mut merged.deck_configurations,
        incoming.deck_configurations,
        |configuration| configuration.id,
        "deck configuration",
    )?;
    merged.cards.append(&mut incoming.cards);
    merged.cards.sort_by_key(|card| card.id);
    merge_exact_records(
        &mut merged.revlog,
        incoming.revlog,
        |review| review.id,
        "revlog",
    )?;
    merge_exact_records(
        &mut merged.graves,
        incoming.graves,
        |grave| (grave.object_id, grave.object_kind),
        "grave",
    )?;
    merged
        .review_queue
        .card_ids
        .append(&mut incoming.review_queue.card_ids);
    merged.review_queue.new_count = merged
        .review_queue
        .new_count
        .saturating_add(incoming.review_queue.new_count);
    merged.review_queue.learning_count = merged
        .review_queue
        .learning_count
        .saturating_add(incoming.review_queue.learning_count);
    merged.review_queue.review_count = merged
        .review_queue
        .review_count
        .saturating_add(incoming.review_queue.review_count);
    merged
        .review_queue
        .decks
        .append(&mut incoming.review_queue.decks);
    merged.diagnostics.append(&mut incoming.diagnostics);

    let mut media = BTreeMap::new();
    for record in &existing.manifest().media {
        let bytes = existing.media(&record.name).ok_or_else(|| {
            ImportError::InvalidPackage("existing bundle media index is invalid".to_owned())
        })?;
        media.insert(record.name.clone(), bytes.to_vec());
    }
    for (name, bytes) in incoming_media {
        if let Some(existing) = media.get(&name) {
            if existing != &bytes {
                return Err(ImportError::InvalidPackage(format!(
                    "media name conflict for {name:?}; files are not silently renamed"
                )));
            }
        } else {
            media.insert(name, bytes);
        }
    }
    incoming_media = media;
    Ok((merged, incoming_media))
}

fn merge_exact_records<T, K>(
    existing: &mut Vec<T>,
    incoming: Vec<T>,
    key: impl Fn(&T) -> K,
    label: &str,
) -> Result<(), ImportError>
where
    T: Clone + Eq,
    K: Ord,
{
    let mut records = BTreeMap::new();
    for record in existing.drain(..) {
        records.insert(key(&record), record);
    }
    for record in incoming {
        let record_key = key(&record);
        if let Some(prior) = records.get(&record_key) {
            if prior != &record {
                return Err(ImportError::InvalidPackage(format!(
                    "APKG {label} identifier/content conflict; neither record was overwritten"
                )));
            }
        } else {
            records.insert(record_key, record);
        }
    }
    *existing = records.into_values().collect();
    Ok(())
}

fn report_for(
    bundle: &kobo_flashcards_format::ParsedBundle,
    package_kind: &str,
    bytes: &[u8],
) -> ImportReport {
    let manifest = bundle.manifest();
    let active_cards = manifest.review_queue.card_ids.len();
    let image_bearing_notes = manifest
        .cards
        .iter()
        .filter(|card| {
            card.attachments
                .iter()
                .any(|attachment| attachment.kind == AttachmentKind::Image)
        })
        .map(|card| card.note_id)
        .collect::<BTreeSet<_>>()
        .len();
    let sound_bearing_notes = manifest
        .cards
        .iter()
        .filter(|card| {
            card.attachments
                .iter()
                .any(|attachment| attachment.kind == AttachmentKind::Audio)
        })
        .map(|card| card.note_id)
        .collect::<BTreeSet<_>>()
        .len();
    ImportReport {
        package_kind: package_kind.to_owned(),
        notes: manifest.notes.len(),
        active_cards,
        new_cards: manifest.review_queue.new_count,
        learning_cards: manifest.review_queue.learning_count,
        review_cards: manifest.review_queue.review_count,
        decks: manifest.decks.len(),
        media_files: manifest.media.len(),
        media_bytes: manifest.media.iter().map(|media| media.length).sum(),
        image_bearing_notes,
        sound_bearing_notes,
        diagnostics: manifest.diagnostics.clone(),
        bundle_sha256: digest_hex(bytes),
    }
}

fn atomic_write(path: &Path, bytes: &[u8], fail_after: Option<usize>) -> Result<(), ImportError> {
    let parent = publication_parent(path)?;
    fs::create_dir_all(parent)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            ImportError::InvalidPackage("output filename is not valid Unicode".to_owned())
        })?;
    let partial = parent.join(format!(".{name}.writing"));
    let _ignored = fs::remove_file(&partial);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&partial)?;
    let written = fail_after.unwrap_or(bytes.len()).min(bytes.len());
    file.write_all(&bytes[..written])?;
    file.sync_all()?;
    if written != bytes.len() {
        let _ignored = fs::remove_file(&partial);
        return Err(ImportError::Interrupted);
    }
    fs::rename(&partial, path)?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

fn publication_parent(path: &Path) -> Result<&Path, ImportError> {
    match path.parent() {
        Some(parent) if parent.as_os_str().is_empty() => Ok(Path::new(".")),
        Some(parent) => Ok(parent),
        None => Err(ImportError::InvalidPackage(
            "output must have a usable parent directory".to_owned(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;
    use std::time::{SystemTime, UNIX_EPOCH};
    use zip::{write::SimpleFileOptions, ZipWriter};

    fn unique(name: &str) -> PathBuf {
        let tick = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("target")
            .join("flashcards-import-tests")
            .join(format!("{name}-{}-{tick}", std::process::id()));
        fs::create_dir_all(path.parent().expect("target parent")).expect("test directory");
        path
    }

    fn fixture(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = ZipWriter::new(io::Cursor::new(Vec::new()));
        for (name, value) in entries {
            writer
                .start_file(*name, SimpleFileOptions::default())
                .expect("member");
            writer.write_all(value).expect("contents");
        }
        writer.finish().expect("finish").into_inner()
    }

    #[test]
    fn archive_paths_and_duplicates_are_refused_before_extraction() {
        let path = unique("traversal.apkg");
        fs::write(&path, fixture(&[("../collection.anki2", b"x")])).expect("write");
        let error = read_archive(&path, PackageKind::Apkg).expect_err("refused");
        assert!(error.to_string().contains("unsafe archive member"));
        let _ignored = fs::remove_file(path);
    }

    #[test]
    fn unrecognized_image_bytes_are_refused() {
        assert!(validate_media_bytes("picture.png", b"not a PNG").is_err());
        assert!(validate_media_bytes("sound.mp3", b"audio").is_ok());
        assert!(validate_media_bytes("hidden.bin", b"GIF89a").is_err());
        assert!(validate_media_bytes("hidden.bin", b"RIFF\0\0\0\0WEBP").is_err());
        let png = kobo_image::encode_png_grey(1, 1, &[0]).expect("tiny PNG");
        assert!(validate_media_bytes("extensionless", &png).is_err());
    }

    #[test]
    fn template_rendering_keeps_filters_honest_and_strips_html() {
        let fields = BTreeMap::from([
            ("Front".to_owned(), "<b>こんにちは</b>".to_owned()),
            ("Text".to_owned(), "{{c1::answer::hint}}".to_owned()),
        ]);
        let mut diagnostics = Vec::new();
        assert_eq!(
            render_template(
                "{{text:Front}} {{cloze:Text}}",
                &fields,
                &[],
                0,
                None,
                false,
                &mut diagnostics
            ),
            "こんにちは [hint]"
        );
        assert!(render_template(
            "{{type:Front}}",
            &fields,
            &[],
            0,
            None,
            false,
            &mut diagnostics
        )
        .contains("Unsupported filter"));
        assert_eq!(
            render_template(
                "{{cloze:Text}}",
                &fields,
                &[],
                0,
                Some("question"),
                true,
                &mut diagnostics
            ),
            "answer"
        );
        assert_eq!(diagnostics[0].code, "unsupported-filter");
        assert_eq!(
            annotate_type_answer("before [[type:Front]] after", false),
            "before [[type:Front]] after"
        );
        assert_eq!(
            annotate_type_answer("before [[type:Front]] after", true),
            format!("before [[type:Front]] after\n\n{TYPE_ANSWER_PLACEHOLDER}")
        );
        assert!(template_uses_type_answer("{{type:Front}}"));
        assert!(template_uses_type_answer("{{cloze:type:Text}}"));
        assert!(!template_uses_type_answer("literal [[type:Front]]"));
        assert!(has_html_event_handler(
            r#"<img src="x" onerror = "alert(1)">"#
        ));
        assert!(!has_html_event_handler(
            r#"<img alt="literal onerror = text">"#
        ));
    }

    #[test]
    fn rendered_semantic_and_bounded_css_emphasis_survives() {
        let html = r#"<div class="accent">plain <strong>bold</strong> <u>under</u></div>"#;
        let plain = kobo_html::to_text(html);
        let spans = rendered_emphasis(
            html,
            ".accent { font-style: italic; text-align: right; font-size: 90px; }",
            &plain,
        )
        .expect("bounded emphasis");
        assert_eq!(plain, "plain bold under");
        assert!(spans.iter().any(|span| span.style.emphasis));
        assert!(spans
            .iter()
            .any(|span| span.style.strong && span.style.emphasis));
        assert!(spans
            .iter()
            .any(|span| span.style.underline && span.style.emphasis));
        assert!(spans
            .iter()
            .all(|span| span.end <= plain.len() && span.start < span.end));
    }

    #[test]
    fn interrupted_write_keeps_the_previous_bundle() {
        let path = unique("atomic.cobfc");
        fs::write(&path, b"previous").expect("prior");
        assert!(matches!(
            atomic_write(&path, b"replacement", Some(3)),
            Err(ImportError::Interrupted)
        ));
        assert_eq!(fs::read(&path).expect("prior survives"), b"previous");
        let _ignored = fs::remove_file(path);
    }

    #[test]
    fn bare_output_filenames_publish_in_the_current_directory() {
        let name = format!(
            ".flashcards-bare-output-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        );
        let path = PathBuf::from(name);
        atomic_write(&path, b"published", None).expect("bare output");
        assert_eq!(fs::read(&path).expect("bare output bytes"), b"published");
        assert_eq!(
            publication_parent(&path).expect("bare parent"),
            Path::new(".")
        );
        let _ignored = fs::remove_file(path);
    }

    #[test]
    fn staging_is_chunked_resumable_and_never_replaces_a_partial_import() {
        let root = unique("stage");
        fs::create_dir_all(&root).expect("fixture root");
        let bundle = root.join("collection.cobfc");
        let source = kobo_flashcards_format::Source {
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
            converter_revision: CONVERTER_REVISION.to_owned(),
            original_config_json: "{}".to_owned(),
            original_models_json: "{}".to_owned(),
            original_decks_json: "{}".to_owned(),
            original_deck_configurations_json: "{}".to_owned(),
            original_tags_json: "{}".to_owned(),
            normalized_config: Vec::new(),
            normalized_tags: Vec::new(),
        };
        let mut media = BTreeMap::new();
        let mut state = 0x1234_5678_u32;
        let bytes = (0..TRANSFER_CHUNK_BYTES + 64)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 24) as u8
            })
            .collect::<Vec<_>>();
        media.insert("blob.mp3".to_owned(), bytes);
        fs::write(
            &bundle,
            encode(BundleManifest::empty(source), media).expect("bundle"),
        )
        .expect("bundle file");
        let mounted = root.join("kobo");
        let final_path = mounted.join(".adds/cobalt/data/flashcards/collection.cobfc");
        fs::create_dir_all(final_path.parent().expect("parent")).expect("destination parent");
        fs::write(&final_path, b"previous").expect("existing collection");
        let review_log = final_path
            .parent()
            .expect("flashcards data")
            .join("cobalt-review-log.ndjson");
        let review_bytes = b"{\"format\":2,\"bundle_sha256\":\"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\",\"card_id\":7,\"grade\":\"good\",\"imported_due\":3,\"imported_reps\":2}\n";
        fs::write(&review_log, review_bytes).expect("existing review log");
        assert!(matches!(
            stage_for_kobo(&bundle, &mounted, Some(1)),
            Err(ImportError::Interrupted)
        ));
        assert_eq!(
            fs::read(&final_path).expect("existing collection"),
            b"previous"
        );
        let staged = stage_for_kobo(&bundle, &mounted, None).expect("resume");
        assert!(staged.resumed_at > 0);
        assert_eq!(
            fs::read(&final_path).expect("staged bytes"),
            fs::read(&bundle).expect("source")
        );
        assert_eq!(
            fs::read(review_log).expect("preserved review log"),
            review_bytes
        );
        let _ignored = fs::remove_dir_all(root);
    }

    #[test]
    fn local_review_log_export_is_lossless_and_checked() {
        let root = unique("review-log");
        let mounted = root.join("kobo");
        let log = mounted.join(".adds/cobalt/data/flashcards/cobalt-review-log.ndjson");
        fs::create_dir_all(log.parent().expect("parent")).expect("log parent");
        let original = b"{\"format\":2,\"bundle_sha256\":\"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\",\"card_id\":7,\"grade\":\"good\",\"imported_due\":3,\"imported_reps\":2}\n";
        fs::write(&log, original).expect("log");
        let output = root.join("export.ndjson");
        let report = export_local_review_log(&mounted, &output).expect("export");
        assert_eq!(report.records, 1);
        assert_eq!(fs::read(output).expect("output"), original);
        let _ignored = fs::remove_dir_all(root);
    }

    #[test]
    fn media_references_cover_images_and_sound_tags() {
        let fields = BTreeMap::from([(
            "Field".to_owned(),
            r#"<img src="image.png"> [sound:audio.mp3]"#.to_owned(),
        )]);
        assert_eq!(referenced_media(&fields), vec!["audio.mp3", "image.png"]);
    }

    #[test]
    fn self_generated_fixture_preserves_cards_media_and_schedule() {
        let root = unique("complete-fixture");
        fs::create_dir_all(&root).expect("fixture root");
        let collection = root.join("collection.anki2");
        create_fixture_collection(&collection);
        let image = kobo_image::encode_png_grey(1, 1, &[0]).expect("tiny PNG");
        let database = fs::read(&collection).expect("database");
        let package = root.join("fixture.apkg");
        fs::write(
            &package,
            fixture(&[
                ("collection.anki2", &database),
                ("media", br#"{"0":"picture.png","1":"clip.mp3"}"#),
                ("0", &image),
                ("1", b"ID3audio"),
            ]),
        )
        .expect("package");
        let output = root.join("collection.cobfc");
        let options = ImportOptions::apkg();
        let report = import(&package, &output, &options).expect("import");
        let scratch = collection_scratch_path(&output).expect("scratch path");
        for suffix in ["", "-wal", "-shm", "-journal"] {
            assert!(!sqlite_sidecar_path(&scratch, suffix).exists());
        }
        assert_eq!(report.notes, 2);
        assert_eq!(report.active_cards, 3);
        assert_eq!(report.decks, 1);
        assert_eq!(report.media_files, 2);
        assert_eq!(report.image_bearing_notes, 1);
        assert_eq!(report.sound_bearing_notes, 1);
        let bundle_bytes = fs::read(&output).expect("bundle");
        let bundle = decode(&bundle_bytes).expect("verified bundle");
        let second_output = root.join("collection-second.cobfc");
        import(&package, &second_output, &options).expect("deterministic reimport");
        assert_eq!(
            bundle_bytes,
            fs::read(second_output).expect("second bundle")
        );
        assert_eq!(bundle.media("picture.png"), Some(image.as_slice()));
        verify_bundle(&output).expect("Kobo image verification");
        assert_eq!(bundle.manifest().revlog.len(), 1);
        assert!(bundle.manifest().notetypes[0]
            .original_json
            .contains("\"css\""));
        let _ignored = fs::remove_dir_all(root);
    }

    #[test]
    fn private_owner_deck_matches_pinned_rslib_aggregates_when_available() {
        let Ok(package) = std::env::var("COBALT_ANKI_EQUIVALENCE_APKG") else {
            return;
        };
        let package = PathBuf::from(package);
        if !package.is_file() {
            return;
        }
        let root = unique("private-equivalence");
        fs::create_dir_all(&root).expect("private test root");
        let output = root.join("collection.cobfc");
        import(&package, &output, &ImportOptions::apkg()).expect("private import");
        let bundle_bytes = fs::read(&output).expect("private bundle");
        let bundle = decode(&bundle_bytes).expect("private bundle decode");
        verify_bundle(&output).expect("private image verification");
        assert!(
            bundle
                .manifest()
                .cards
                .iter()
                .any(|card| !card.front_spans.is_empty() || !card.back_spans.is_empty()),
            "private aggregate contains no retained emphasis"
        );

        let unpacked = read_archive(&package, PackageKind::Apkg).expect("private archive");
        let collection_path = root.join("upstream.anki2");
        fs::write(&collection_path, unpacked.collection).expect("private collection scratch");
        let mut builder = CollectionBuilder::new(&collection_path);
        builder.set_check_integrity(true);
        builder
            .build()
            .expect("upstream migration")
            .close(None)
            .expect("upstream migration close");
        let mut collection = CollectionBuilder::new(&collection_path)
            .build()
            .expect("upstream normalized collection");
        let notes = read_notes(collection.storage.db()).expect("upstream notes");
        let decks = read_decks(&mut collection).expect("upstream decks");
        let rows = read_card_rows(collection.storage.db()).expect("upstream cards");
        let queue =
            read_review_queue(&mut collection, &decks, rows.len()).expect("upstream scheduler");

        let upstream_render = aggregate_upstream_render_identity(&mut collection, &rows);
        let bundle_render = aggregate_bundle_render_identity(bundle.manifest());
        assert!(
            upstream_render == bundle_render,
            "aggregate question/answer/media identity differs from pinned rslib"
        );
        assert!(
            queue.card_ids == bundle.manifest().review_queue.card_ids
                && queue.new_count == bundle.manifest().review_queue.new_count
                && queue.learning_count == bundle.manifest().review_queue.learning_count
                && queue.review_count == bundle.manifest().review_queue.review_count,
            "aggregate scheduling queue/counts differ from pinned rslib"
        );
        assert!(
            notes.len() == bundle.manifest().notes.len()
                && rows.len() == bundle.manifest().cards.len(),
            "aggregate collection counts differ from pinned rslib"
        );
        let _ignored = fs::remove_dir_all(root);
    }

    fn aggregate_upstream_render_identity(collection: &mut Collection, rows: &[CardRow]) -> String {
        let mut aggregate = Vec::new();
        for row in rows {
            let rendered = anki::services::CardRenderingService::render_existing_card(
                collection,
                anki_proto::card_rendering::RenderExistingCardRequest {
                    card_id: row.id,
                    browser: false,
                    partial_render: false,
                },
            )
            .expect("upstream render");
            let card = collection
                .storage
                .get_card(anki::card::CardId(row.id))
                .expect("upstream card")
                .expect("upstream card exists");
            let note = collection
                .storage
                .get_note(card.note_id())
                .expect("upstream note")
                .expect("upstream note exists");
            let notetype = collection
                .get_notetype(note.notetype_id)
                .expect("upstream notetype")
                .expect("upstream notetype exists");
            let template = notetype
                .get_template(card.template_idx())
                .expect("upstream template");
            let type_answer = template_uses_type_answer(&template.config.q_format)
                || template_uses_type_answer(&template.config.a_format);
            let question = rendered_nodes_text(rendered.question_nodes).expect("question");
            let answer = rendered_nodes_text(rendered.answer_nodes).expect("answer");
            let question_media = sorted_unique_references(
                upstream_media_references(collection, &question).expect("question media"),
            );
            let answer_media = sorted_unique_references(
                upstream_media_references(collection, &answer).expect("answer media"),
            );
            let question_plain = kobo_html::to_text(&question);
            let answer_plain = kobo_html::to_text(&answer);
            let question_spans =
                rendered_emphasis(&question, &notetype.config.css, &question_plain)
                    .expect("question emphasis");
            let answer_spans = rendered_emphasis(&answer, &notetype.config.css, &answer_plain)
                .expect("answer emphasis");
            let question_text = annotate_type_answer(&question_plain, type_answer);
            let answer_text = annotate_type_answer(&answer_plain, type_answer);
            append_identity_field(&mut aggregate, &row.id.to_le_bytes());
            append_identity_field(&mut aggregate, question_text.as_bytes());
            append_text_span_identity(&mut aggregate, &question_spans);
            append_identity_field(&mut aggregate, answer_text.as_bytes());
            append_text_span_identity(&mut aggregate, &answer_spans);
            for name in question_media {
                append_identity_field(&mut aggregate, name.as_bytes());
            }
            aggregate.push(0xff);
            for name in answer_media {
                append_identity_field(&mut aggregate, name.as_bytes());
            }
            aggregate.push(0xfe);
        }
        digest_hex(&aggregate)
    }

    fn aggregate_bundle_render_identity(manifest: &BundleManifest) -> String {
        let mut aggregate = Vec::new();
        for card in &manifest.cards {
            append_identity_field(&mut aggregate, &card.id.to_le_bytes());
            append_identity_field(&mut aggregate, card.front.as_bytes());
            append_text_span_identity(&mut aggregate, &card.front_spans);
            append_identity_field(&mut aggregate, card.back.as_bytes());
            append_text_span_identity(&mut aggregate, &card.back_spans);
            for name in &card.question_media_names {
                append_identity_field(&mut aggregate, name.as_bytes());
            }
            aggregate.push(0xff);
            for name in &card.answer_media_names {
                append_identity_field(&mut aggregate, name.as_bytes());
            }
            aggregate.push(0xfe);
        }
        digest_hex(&aggregate)
    }

    fn append_text_span_identity(aggregate: &mut Vec<u8>, spans: &[CardTextSpan]) {
        for span in spans {
            append_identity_field(
                aggregate,
                &u64::try_from(span.start)
                    .expect("span start fits")
                    .to_le_bytes(),
            );
            append_identity_field(
                aggregate,
                &u64::try_from(span.end)
                    .expect("span end fits")
                    .to_le_bytes(),
            );
            aggregate.extend_from_slice(&[
                u8::from(span.style.strong),
                u8::from(span.style.emphasis),
                u8::from(span.style.underline),
                u8::from(span.style.superscript),
                u8::from(span.style.subscript),
            ]);
        }
        aggregate.push(0xfd);
    }

    fn append_identity_field(output: &mut Vec<u8>, field: &[u8]) {
        output.extend_from_slice(
            &u64::try_from(field.len())
                .expect("aggregate field length")
                .to_le_bytes(),
        );
        output.extend_from_slice(field);
    }

    #[test]
    fn scheduler_queue_excludes_suspended_buried_and_non_due_cards() {
        let root = unique("scheduler-state");
        fs::create_dir_all(&root).expect("scheduler root");
        let collection_path = root.join("collection.anki2");
        let mut collection = CollectionBuilder::new(&collection_path)
            .build()
            .expect("rslib collection");
        let basic = collection
            .get_notetype_by_name("Basic")
            .expect("basic lookup")
            .expect("basic notetype");
        for index in 0..4 {
            let mut note = basic.new_note();
            note.set_field(0, format!("front {index}")).expect("front");
            note.set_field(1, format!("back {index}")).expect("back");
            collection
                .add_note(&mut note, DeckId(1))
                .expect("basic note");
        }
        let ids = collection
            .storage
            .db()
            .prepare("SELECT id FROM cards ORDER BY id")
            .expect("card ids")
            .query_map([], |row| row.get::<_, i64>(0))
            .expect("card rows")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("card ids");
        let _timing = anki::services::SchedulerService::sched_timing_today(&mut collection)
            .expect("initialize day");
        for (id, mode) in [
            (
                ids[0],
                anki_proto::scheduler::bury_or_suspend_cards_request::Mode::Suspend,
            ),
            (
                ids[1],
                anki_proto::scheduler::bury_or_suspend_cards_request::Mode::BuryUser,
            ),
        ] {
            let _changes = anki::services::SchedulerService::bury_or_suspend_cards(
                &mut collection,
                anki_proto::scheduler::BuryOrSuspendCardsRequest {
                    card_ids: vec![id],
                    note_ids: Vec::new(),
                    mode: mode as i32,
                },
            )
            .expect("bury or suspend");
        }
        collection
            .storage
            .db()
            .execute(
                "UPDATE cards SET type = 2, queue = 2, due = 2147483647 WHERE id = ?",
                [ids[2]],
            )
            .expect("future review");
        collection
            .close(Some(anki::storage::SchemaVersion::V11))
            .expect("schema-11 package");
        let package = root.join("scheduler.apkg");
        write_package(&package, &collection_path, "collection.anki2", &[]);
        let output = root.join("scheduler.cobfc");
        import(&package, &output, &ImportOptions::apkg()).expect("scheduler import");
        let bytes = fs::read(output).expect("scheduler bundle");
        let bundle = decode(&bytes).expect("scheduler decode");
        assert_eq!(bundle.manifest().cards.len(), 4);
        assert_eq!(bundle.manifest().review_queue.card_ids, vec![ids[3]]);
        assert_eq!(bundle.manifest().review_queue.new_count, 1);
        assert_eq!(bundle.manifest().review_queue.learning_count, 0);
        assert_eq!(bundle.manifest().review_queue.review_count, 0);
        let _ignored = fs::remove_dir_all(root);
    }

    #[test]
    fn multi_cloze_ordinals_are_generated_and_rendered_by_pinned_rslib() {
        let root = unique("multi-cloze");
        fs::create_dir_all(&root).expect("cloze root");
        let collection_path = root.join("collection.anki2");
        let mut collection = CollectionBuilder::new(&collection_path)
            .build()
            .expect("rslib collection");
        let cloze = collection
            .get_notetype_by_name("Cloze")
            .expect("cloze lookup")
            .expect("cloze notetype");
        let mut note = cloze.new_note();
        note.set_field(0, "{{c1::one}} {{c2::two}} {{c1,2::both}}")
            .expect("multi cloze");
        collection
            .add_note(&mut note, DeckId(1))
            .expect("cloze note");
        collection
            .close(Some(anki::storage::SchemaVersion::V11))
            .expect("schema-11 package");
        let package = root.join("cloze.apkg");
        write_package(&package, &collection_path, "collection.anki2", &[]);
        let output = root.join("cloze.cobfc");
        import(&package, &output, &ImportOptions::apkg()).expect("cloze import");
        let bytes = fs::read(output).expect("cloze bundle");
        let bundle = decode(&bytes).expect("cloze decode");
        let mut ordinals = bundle
            .manifest()
            .cards
            .iter()
            .map(|card| card.ordinal)
            .collect::<Vec<_>>();
        ordinals.sort_unstable();
        assert_eq!(ordinals, vec![0, 1]);
        assert_ne!(
            bundle.manifest().cards[0].front,
            bundle.manifest().cards[1].front
        );
        assert_eq!(bundle.manifest().review_queue.card_ids.len(), 2);
        let _ignored = fs::remove_dir_all(root);
    }

    #[test]
    fn both_legacy_collection_members_and_colpkg_replacement_are_supported() {
        let root = unique("legacy-members");
        fs::create_dir_all(&root).expect("legacy root");
        let collection = root.join("collection.anki2");
        create_fixture_collection(&collection);

        let anki21 = root.join("legacy2.apkg");
        write_fixture_media_package(&anki21, &collection, "collection.anki21");
        let anki21_output = root.join("legacy2.cobfc");
        import(&anki21, &anki21_output, &ImportOptions::apkg()).expect("legacy2 APKG");
        let bytes = fs::read(anki21_output).expect("legacy2 bundle");
        let bundle = decode(&bytes).expect("legacy2 decode");
        assert_eq!(
            bundle.manifest().sources[0].collection_member,
            "collection.anki21"
        );

        let colpkg = root.join("legacy.colpkg");
        write_fixture_media_package(&colpkg, &collection, "collection.anki2");
        let colpkg_output = root.join("legacy-colpkg.cobfc");
        import(&colpkg, &colpkg_output, &ImportOptions::colpkg()).expect("legacy COLPKG");
        let bytes = fs::read(colpkg_output).expect("COLPKG bundle");
        let bundle = decode(&bytes).expect("COLPKG decode");
        assert_eq!(bundle.manifest().sources.len(), 1);
        assert_eq!(bundle.manifest().sources[0].package_kind, "colpkg");

        let schema18 = root.join("schema18.anki2");
        let mut latest = CollectionBuilder::new(&schema18)
            .build()
            .expect("schema-18 collection");
        let basic = latest
            .get_notetype_by_name("Basic")
            .expect("basic lookup")
            .expect("basic notetype");
        let mut note = basic.new_note();
        note.set_field(0, "front").expect("front");
        note.set_field(1, "back").expect("back");
        latest.add_note(&mut note, DeckId(1)).expect("latest note");
        latest.close(None).expect("latest close");
        let latest_package = root.join("schema18.apkg");
        write_package(&latest_package, &schema18, "collection.anki2", &[]);
        let latest_output = root.join("schema18.cobfc");
        import(&latest_package, &latest_output, &ImportOptions::apkg()).expect("schema-18 APKG");
        let bytes = fs::read(latest_output).expect("schema-18 bundle");
        let bundle = decode(&bytes).expect("schema-18 decode");
        assert_eq!(bundle.manifest().sources[0].collection_schema, 18);
        assert_eq!(bundle.manifest().sources[0].normalized_schema, 18);
        let _ignored = fs::remove_dir_all(root);
    }

    #[test]
    fn modern_schema_gap_javascript_addons_and_animated_images_fail_closed() {
        let root = unique("unsupported-boundaries");
        fs::create_dir_all(&root).expect("boundary root");
        let modern = root.join("modern.apkg");
        fs::write(
            &modern,
            fixture(&[
                ("collection.anki21b", b"modern"),
                ("meta", b"protobuf"),
                ("media", b"protobuf"),
            ]),
        )
        .expect("modern package");
        assert!(read_archive(&modern, PackageKind::Apkg)
            .expect_err("modern refused")
            .to_string()
            .contains("modern"));

        let collection_path = root.join("collection.anki2");
        create_fixture_collection(&collection_path);
        Connection::open(&collection_path)
            .expect("schema gap collection")
            .execute("UPDATE col SET ver = 12", [])
            .expect("schema gap");
        let schema_gap = root.join("schema-gap.apkg");
        write_fixture_media_package(&schema_gap, &collection_path, "collection.anki2");
        assert!(import(
            &schema_gap,
            &root.join("schema-gap.cobfc"),
            &ImportOptions::apkg()
        )
        .expect_err("schema gap refused")
        .to_string()
        .contains("schemas 12 and 13"));

        assert!(validate_media_bytes("animated.gif", b"GIF89a").is_err());
        assert!(validate_media_bytes("animated.webp", b"RIFF\0\0\0\0WEBP").is_err());
        assert!(validate_media_bytes("addon.js", b"console.log(1)").is_err());

        let js_collection = root.join("javascript.anki2");
        let mut collection = CollectionBuilder::new(&js_collection)
            .build()
            .expect("javascript collection");
        let basic = collection
            .get_notetype_by_name("Basic")
            .expect("basic lookup")
            .expect("basic notetype");
        let mut changed = (*basic).clone();
        changed.templates[0].config.q_format = "<script>alert(1)</script>{{Front}}".to_owned();
        collection
            .update_notetype(&mut changed, false)
            .expect("javascript template");
        let mut note = changed.new_note();
        note.set_field(0, "front").expect("front");
        note.set_field(1, "back").expect("back");
        collection
            .add_note(&mut note, DeckId(1))
            .expect("javascript note");
        collection
            .close(Some(anki::storage::SchemaVersion::V11))
            .expect("javascript package");
        let js_package = root.join("javascript.apkg");
        write_package(&js_package, &js_collection, "collection.anki2", &[]);
        assert!(import(
            &js_package,
            &root.join("javascript.cobfc"),
            &ImportOptions::apkg()
        )
        .expect_err("javascript refused")
        .to_string()
        .contains("JavaScript"));

        let addon_collection = root.join("addon-filter.anki2");
        let mut collection = CollectionBuilder::new(&addon_collection)
            .build()
            .expect("add-on collection");
        let basic = collection
            .get_notetype_by_name("Basic")
            .expect("basic lookup")
            .expect("basic notetype");
        let mut changed = (*basic).clone();
        changed.templates[0].config.q_format = "{{third_party_addon:Front}}".to_owned();
        collection
            .update_notetype(&mut changed, false)
            .expect("add-on template");
        let mut note = changed.new_note();
        note.set_field(0, "front").expect("front");
        note.set_field(1, "back").expect("back");
        collection
            .add_note(&mut note, DeckId(1))
            .expect("add-on note");
        collection
            .close(Some(anki::storage::SchemaVersion::V11))
            .expect("add-on package");
        let addon_package = root.join("addon-filter.apkg");
        write_package(&addon_package, &addon_collection, "collection.anki2", &[]);
        assert!(import(
            &addon_package,
            &root.join("addon-filter.cobfc"),
            &ImportOptions::apkg()
        )
        .expect_err("add-on filter refused")
        .to_string()
        .contains("add-on template filter"));
        let _ignored = fs::remove_dir_all(root);
    }

    #[test]
    fn inactive_templates_do_not_block_cards_that_never_use_them() {
        let root = unique("inactive-template");
        fs::create_dir_all(&root).expect("inactive template root");
        let collection_path = root.join("collection.anki2");
        let mut collection = CollectionBuilder::new(&collection_path)
            .build()
            .expect("rslib collection");
        let basic = collection
            .get_notetype_by_name("Basic (and reversed card)")
            .expect("basic lookup")
            .expect("basic and reversed notetype");
        let mut changed = (*basic).clone();
        changed.templates[1].config.q_format = "<script>alert(1)</script>{{Back}}".to_owned();
        collection
            .update_notetype(&mut changed, false)
            .expect("inactive template");
        let mut note = changed.new_note();
        note.set_field(0, "front").expect("front");
        note.set_field(1, "").expect("empty reverse field");
        collection
            .add_note(&mut note, DeckId(1))
            .expect("forward-only note");
        let card_count: i64 = collection
            .storage
            .db()
            .query_row("SELECT count(*) FROM cards", [], |row| row.get(0))
            .expect("card count");
        assert_eq!(card_count, 1);
        collection
            .close(Some(anki::storage::SchemaVersion::V11))
            .expect("schema-11 package");
        let package = root.join("inactive-template.apkg");
        write_package(&package, &collection_path, "collection.anki2", &[]);
        import(
            &package,
            &root.join("inactive-template.cobfc"),
            &ImportOptions::apkg(),
        )
        .expect("unused active template remains inert");
        let _ignored = fs::remove_dir_all(root);
    }

    #[test]
    fn svg_resolution_fonts_and_dimensions_are_strictly_bounded() {
        let safe = br#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><text x="1" y="12">7</text></svg>"#;
        let png = rasterize_svg(safe).expect("controlled SVG text");
        assert!(kobo_image::decode(&png).is_ok());
        assert!(rasterize_svg(
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><image href="../outside.png" width="20" height="20"/></svg>"#
        )
        .is_err());
        assert!(rasterize_svg(
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="100000" height="100000"><path d="M0 0L1 1"/></svg>"#
        )
        .is_err());
        assert!(rasterize_svg(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><text x="1" y="12">😀</text></svg>"#
                .as_bytes()
        )
        .is_err());
    }

    #[test]
    fn bundle_verification_binds_svg_raster_to_retained_source() {
        let root = unique("svg-binding");
        fs::create_dir_all(&root).expect("SVG binding root");
        let collection_path = root.join("collection.anki2");
        let mut collection = CollectionBuilder::new(&collection_path)
            .build()
            .expect("rslib collection");
        let basic = collection
            .get_notetype_by_name("Basic")
            .expect("basic lookup")
            .expect("basic notetype");
        let mut note = basic.new_note();
        note.set_field(0, r#"<img src="source.svg">"#)
            .expect("front");
        note.set_field(1, "back").expect("back");
        collection.add_note(&mut note, DeckId(1)).expect("SVG note");
        collection
            .close(Some(anki::storage::SchemaVersion::V11))
            .expect("schema-11 package");
        let svg =
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><rect width="20" height="20"/></svg>"#;
        let package = root.join("svg.apkg");
        write_package(
            &package,
            &collection_path,
            "collection.anki2",
            &[("source.svg", svg)],
        );
        let imported = root.join("svg.cobfc");
        import(&package, &imported, &ImportOptions::apkg()).expect("SVG import");
        verify_bundle(&imported).expect("valid SVG binding");

        let imported_bytes = fs::read(&imported).expect("SVG bundle");
        let parsed = decode(&imported_bytes).expect("SVG decode");
        let rendered_name = parsed.manifest().cards[0].attachments[0]
            .rendered_name
            .clone()
            .expect("rendered SVG name");
        let mut media = parsed
            .manifest()
            .media
            .iter()
            .map(|record| {
                (
                    record.name.clone(),
                    parsed.media(&record.name).expect("media bytes").to_vec(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        media.insert(
            rendered_name,
            kobo_image::encode_png_grey(1, 1, &[255]).expect("unrelated PNG"),
        );
        let tampered = root.join("svg-tampered.cobfc");
        fs::write(
            &tampered,
            encode(parsed.manifest().clone(), media).expect("tampered bundle"),
        )
        .expect("tampered file");
        assert!(verify_bundle(&tampered)
            .expect_err("mismatched SVG raster refused")
            .to_string()
            .contains("does not match"));
        let mut merge = ImportOptions::apkg();
        merge.merge_into = Some(tampered);
        assert!(import(&package, &root.join("merged.cobfc"), &merge)
            .expect_err("unverified merge base refused")
            .to_string()
            .contains("does not match"));
        let _ignored = fs::remove_dir_all(root);
    }

    #[test]
    fn cards_with_multiple_images_on_one_side_fail_closed() {
        let root = unique("multiple-images");
        fs::create_dir_all(&root).expect("multiple image root");
        let collection_path = root.join("collection.anki2");
        let mut collection = CollectionBuilder::new(&collection_path)
            .build()
            .expect("rslib collection");
        let basic = collection
            .get_notetype_by_name("Basic")
            .expect("basic lookup")
            .expect("basic notetype");
        let mut note = basic.new_note();
        note.set_field(0, r#"<img src="one.png"><img src="two.png">"#)
            .expect("front");
        note.set_field(1, "back").expect("back");
        collection
            .add_note(&mut note, DeckId(1))
            .expect("multiple image note");
        collection
            .close(Some(anki::storage::SchemaVersion::V11))
            .expect("schema-11 package");
        let image = kobo_image::encode_png_grey(1, 1, &[0]).expect("tiny PNG");
        let package = root.join("multiple-images.apkg");
        write_package(
            &package,
            &collection_path,
            "collection.anki2",
            &[("one.png", &image), ("two.png", &image)],
        );
        assert!(import(
            &package,
            &root.join("multiple-images.cobfc"),
            &ImportOptions::apkg()
        )
        .expect_err("multiple images refused")
        .to_string()
        .contains("more than one image"));
        let _ignored = fs::remove_dir_all(root);
    }

    #[test]
    fn referenced_unknown_media_types_fail_closed() {
        let root = unique("unknown-side-media");
        fs::create_dir_all(&root).expect("unknown media root");
        let collection_path = root.join("collection.anki2");
        let mut collection = CollectionBuilder::new(&collection_path)
            .build()
            .expect("rslib collection");
        let basic = collection
            .get_notetype_by_name("Basic")
            .expect("basic lookup")
            .expect("basic notetype");
        let mut note = basic.new_note();
        note.set_field(0, r#"<img src="picture.bmp">"#)
            .expect("front");
        note.set_field(1, "back").expect("back");
        collection
            .add_note(&mut note, DeckId(1))
            .expect("unknown media note");
        collection
            .close(Some(anki::storage::SchemaVersion::V11))
            .expect("schema-11 package");
        let package = root.join("unknown-media.apkg");
        write_package(
            &package,
            &collection_path,
            "collection.anki2",
            &[("picture.bmp", b"BMunsupported")],
        );
        assert!(import(
            &package,
            &root.join("unknown-media.cobfc"),
            &ImportOptions::apkg()
        )
        .expect_err("unknown referenced media refused")
        .to_string()
        .contains("cannot display"));
        let _ignored = fs::remove_dir_all(root);
    }

    #[test]
    fn apkg_merge_preserves_all_metadata_and_rejects_content_conflicts() {
        let root = unique("metadata-merge");
        fs::create_dir_all(&root).expect("merge root");
        let first_collection = root.join("first.anki2");
        create_fixture_collection(&first_collection);
        let second_collection = root.join("second.anki2");
        fs::copy(&first_collection, &second_collection).expect("copy collection");
        let database = Connection::open(&second_collection).expect("second collection");
        database
            .execute("UPDATE notes SET id = id + 100000", [])
            .expect("remap notes");
        database
            .execute("UPDATE cards SET id = id + 100000, nid = nid + 100000", [])
            .expect("remap cards");
        database
            .execute("UPDATE revlog SET id = id + 100000, cid = cid + 100000", [])
            .expect("remap revlog");
        drop(database);
        let first_package = root.join("first.apkg");
        let second_package = root.join("second.apkg");
        write_fixture_media_package(&first_package, &first_collection, "collection.anki2");
        write_fixture_media_package(&second_package, &second_collection, "collection.anki2");
        let first_bundle = root.join("first.cobfc");
        import(&first_package, &first_bundle, &ImportOptions::apkg()).expect("first import");
        let mut options = ImportOptions::apkg();
        options.merge_into = Some(first_bundle.clone());
        let merged_bundle = root.join("merged.cobfc");
        import(&second_package, &merged_bundle, &options).expect("metadata merge");
        let merged_bytes = fs::read(&merged_bundle).expect("merged bundle");
        let merged = decode(&merged_bytes).expect("merged decode");
        assert_eq!(merged.manifest().sources.len(), 2);
        assert_eq!(merged.manifest().notes.len(), 4);
        assert_eq!(merged.manifest().cards.len(), 6);
        assert_eq!(merged.manifest().revlog.len(), 2);
        assert_eq!(merged.manifest().review_queue.card_ids.len(), 6);
        assert_eq!(
            merged.manifest().notetypes.len(),
            decode(&fs::read(&first_bundle).expect("first bytes"))
                .expect("first decode")
                .manifest()
                .notetypes
                .len()
        );

        let conflicting_collection = root.join("conflict.anki2");
        fs::copy(&second_collection, &conflicting_collection).expect("conflict collection");
        let conflict_db = Connection::open(&conflicting_collection).expect("conflict db");
        let decks_json: String = conflict_db
            .query_row("SELECT decks FROM col", [], |row| row.get(0))
            .expect("decks json");
        let mut decks: Value = serde_json::from_str(&decks_json).expect("decks");
        decks["1"]["name"] = Value::String("Conflicting deck".to_owned());
        conflict_db
            .execute(
                "UPDATE col SET decks = ?",
                [serde_json::to_string(&decks).expect("decks json")],
            )
            .expect("conflicting deck");
        drop(conflict_db);
        let conflict_package = root.join("conflict.apkg");
        write_fixture_media_package(
            &conflict_package,
            &conflicting_collection,
            "collection.anki2",
        );
        assert!(
            import(&conflict_package, &root.join("conflict.cobfc"), &options)
                .expect_err("deck conflict refused")
                .to_string()
                .contains("deck identifier/content conflict")
        );
        let _ignored = fs::remove_dir_all(root);
    }

    #[test]
    fn malformed_database_duplicate_media_and_bomb_are_refused() {
        let root = unique("bad-fixtures");
        fs::create_dir_all(&root).expect("fixture root");
        let malformed = root.join("malformed.apkg");
        fs::write(
            &malformed,
            fixture(&[("collection.anki2", b"not sqlite"), ("media", b"{}")]),
        )
        .expect("package");
        assert!(matches!(
            import(&malformed, &root.join("bad.cobfc"), &ImportOptions::apkg()),
            Err(ImportError::Sql(_))
        ));

        let missing_collection = root.join("missing-media.anki2");
        create_fixture_collection(&missing_collection);
        let missing = root.join("missing-media.apkg");
        let image = kobo_image::encode_png_grey(1, 1, &[0]).expect("tiny PNG");
        write_package(
            &missing,
            &missing_collection,
            "collection.anki2",
            &[("picture.png", &image)],
        );
        assert!(import(
            &missing,
            &root.join("missing-media.cobfc"),
            &ImportOptions::apkg()
        )
        .expect_err("missing reference refused")
        .to_string()
        .contains("media absent"));

        let duplicate = root.join("duplicate.apkg");
        fs::write(
            &duplicate,
            fixture(&[
                ("collection.anki2", b"not inspected"),
                ("media", br#"{"0":"same.mp3","1":"same.mp3"}"#),
                ("0", b"ID3first"),
                ("1", b"ID3second"),
            ]),
        )
        .expect("package");
        assert!(read_archive(&duplicate, PackageKind::Apkg)
            .expect_err("duplicate media")
            .to_string()
            .contains("collide"));

        let duplicate_key = root.join("duplicate-key.apkg");
        fs::write(
            &duplicate_key,
            fixture(&[
                ("collection.anki2", b"not inspected"),
                ("media", br#"{"0":"first.mp3","0":"second.mp3"}"#),
                ("0", b"ID3audio"),
            ]),
        )
        .expect("duplicate key package");
        assert!(read_archive(&duplicate_key, PackageKind::Apkg)
            .expect_err("duplicate media key")
            .to_string()
            .contains("duplicate media map key"));

        let bomb = root.join("bomb.apkg");
        let large =
            vec![0_u8; usize::try_from(MAX_ARCHIVE_BYTES).expect("archive limit fits usize") + 1];
        fs::write(
            &bomb,
            fixture(&[("collection.anki2", &large), ("media", b"{}")]),
        )
        .expect("package");
        assert!(read_archive(&bomb, PackageKind::Apkg)
            .expect_err("bomb")
            .to_string()
            .contains("decompression limit"));
        let _ignored = fs::remove_dir_all(root);
    }

    fn create_fixture_collection(path: &Path) {
        let mut collection = CollectionBuilder::new(path)
            .build()
            .expect("rslib collection");
        let basic = collection
            .get_notetype_by_name("Basic (and reversed card)")
            .expect("basic lookup")
            .expect("basic and reversed stock notetype");
        let mut basic_note = basic.new_note();
        basic_note
            .set_field(0, "<b>こんにちは</b><img src=\"picture.png\">")
            .expect("front");
        basic_note
            .set_field(1, "Answer [sound:clip.mp3]")
            .expect("back");
        basic_note.tags.push("tag".to_owned());
        collection
            .add_note(&mut basic_note, DeckId(1))
            .expect("basic note");

        let cloze = collection
            .get_notetype_by_name("Cloze")
            .expect("cloze lookup")
            .expect("cloze stock notetype");
        let mut cloze_note = cloze.new_note();
        cloze_note
            .set_field(0, "{{c1::答え::答}}")
            .expect("cloze text");
        collection
            .add_note(&mut cloze_note, DeckId(1))
            .expect("cloze note");
        collection
            .close(Some(anki::storage::SchemaVersion::V11))
            .expect("schema-11 package");

        let database = Connection::open(path).expect("open downgraded fixture");
        let first_card: i64 = database
            .query_row("SELECT id FROM cards ORDER BY id LIMIT 1", [], |row| {
                row.get(0)
            })
            .expect("first card");
        database
            .execute(
                "INSERT INTO revlog VALUES (99, ?1, 0, 3, 4, 1, 2500, 500, 1)",
                params![first_card],
            )
            .expect("review log");
    }

    fn write_fixture_media_package(package: &Path, collection: &Path, member: &str) {
        let image = kobo_image::encode_png_grey(1, 1, &[0]).expect("tiny PNG");
        write_package(
            package,
            collection,
            member,
            &[("picture.png", image.as_slice()), ("clip.mp3", b"ID3audio")],
        );
    }

    fn write_package(
        package: &Path,
        collection: &Path,
        collection_member: &str,
        media: &[(&str, &[u8])],
    ) {
        let database = fs::read(collection).expect("collection bytes");
        let media_map = media
            .iter()
            .enumerate()
            .map(|(index, (name, _))| (index.to_string(), (*name).to_owned()))
            .collect::<BTreeMap<_, _>>();
        let media_json = serde_json::to_vec(&media_map).expect("media map");
        let mut entries = vec![
            (collection_member.to_owned(), database),
            ("media".to_owned(), media_json),
        ];
        entries.extend(
            media
                .iter()
                .enumerate()
                .map(|(index, (_, bytes))| (index.to_string(), bytes.to_vec())),
        );
        let borrowed = entries
            .iter()
            .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
            .collect::<Vec<_>>();
        fs::write(package, fixture(&borrowed)).expect("package");
    }
}

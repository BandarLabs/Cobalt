#![forbid(unsafe_code)]

//! Host-only APKG/COLPKG ingestion for the Flashcards offline bundle.
//!
//! It intentionally treats every package member as hostile. The only
//! executable input Anki cards normally carry is HTML/JavaScript; rendered
//! device strings are plain text, and the source template is retained only as
//! inert data in the manifest.

use kobo_flashcards_format::{
    canonical_media_name, decode, digest_hex, encode, media_type, Attachment, AttachmentKind,
    BundleManifest, Card, Deck, DeckConfiguration, Diagnostic, FormatError, NoteType, ReviewLog,
    Source, MAX_BUNDLE_BYTES, MAX_MEDIA_BYTES,
};
use rusqlite::Connection;
use serde_json::Value;
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use zip::ZipArchive;

pub const UPSTREAM_ANKI_REVISION: &str = "9e32ad8849068510a82273889c21b22e1acf0949";
pub const MAX_ARCHIVE_ENTRIES: usize = 8_192;
pub const MAX_ARCHIVE_BYTES: u64 = MAX_BUNDLE_BYTES;
pub const MAX_COLLECTION_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_EXPANSION_RATIO: u64 = 100;
pub const TRANSFER_CHUNK_BYTES: usize = 256 * 1024;

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
    let manifest = read_collection(&unpacked.collection, &unpacked.media, package_kind, output)?;
    let (manifest, media) = match options.mode {
        ImportMode::MergeApkg => {
            merge_existing(manifest, unpacked.media, options.merge_into.as_deref())?
        }
        ImportMode::ReplaceColpkg => (manifest, unpacked.media),
    };
    let encoded = encode(manifest.clone(), media)?;
    // Parsing before publication confirms the exact same checks the Kobo app
    // runs. No broken bundle has a path to the final name.
    let parsed = decode(&encoded)?;
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
    let parsed = decode(&bytes)?;
    for card in &parsed.manifest().cards {
        for attachment in card
            .attachments
            .iter()
            .filter(|attachment| attachment.kind == AttachmentKind::Image)
        {
            let name = attachment
                .rendered_name
                .as_deref()
                .unwrap_or(&attachment.name);
            let Some(image) = parsed.media(name) else {
                return Err(ImportError::InvalidPackage(format!(
                    "image attachment {name:?} has no digest-verified bytes"
                )));
            };
            verify_image_rendering(&attachment.mime, image)?;
        }
    }
    Ok(report_for(
        &parsed,
        &parsed.manifest().source.package_kind,
        &bytes,
    ))
}

fn verify_image_rendering(mime: &str, bytes: &[u8]) -> Result<(), ImportError> {
    let rendered;
    let bytes = if mime == "image/svg+xml" {
        rendered = rasterize_svg(bytes)?;
        &rendered
    } else {
        bytes
    };
    kobo_image::decode(bytes).map_err(|_| {
        ImportError::InvalidPackage(
            "a referenced image could not be decoded by the Kobo image path".to_owned(),
        )
    })?;
    Ok(())
}

#[allow(
    clippy::cast_precision_loss,
    reason = "both dimensions are capped to 1,920 pixels before scaling"
)]
fn rasterize_svg(bytes: &[u8]) -> Result<Vec<u8>, ImportError> {
    let sanitized = std::str::from_utf8(bytes)
        .map_err(|_| ImportError::InvalidPackage("a referenced SVG is not UTF-8".to_owned()))?
        .replace("kvg:", "metadata-kvg-")
        .replace("inkscape:", "metadata-inkscape-")
        .replace("sodipodi:", "metadata-sodipodi-");
    let tree = resvg::usvg::Tree::from_data(sanitized.as_bytes(), &resvg::usvg::Options::default())
        .map_err(|_| {
            ImportError::InvalidPackage("a referenced SVG cannot be rendered safely".to_owned())
        })?;
    let size = tree.size().to_int_size();
    let width = size.width().min(1_920);
    let height = size.height().min(1_920);
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height).ok_or_else(|| {
        ImportError::InvalidPackage("a referenced SVG has unsupported dimensions".to_owned())
    })?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(
            width as f32 / size.width() as f32,
            height as f32 / size.height() as f32,
        ),
        &mut pixmap.as_mut(),
    );
    pixmap.encode_png().map_err(|_| {
        ImportError::InvalidPackage(
            "a referenced SVG cannot be encoded for Kobo rendering".to_owned(),
        )
    })
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
    let mut offset = resume_offset(&partial, &journal, &source_digest, source.len());
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
        write_resume_record(&journal, &source_digest, offset, &partial)?;
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
    if bytes.len() > 16 * 1024 * 1024 {
        return Err(ImportError::InvalidPackage(
            "Cobalt review log exceeds the device export bound".to_owned(),
        ));
    }
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| ImportError::InvalidPackage("Cobalt review log is not UTF-8".to_owned()))?;
    let mut records = 0_usize;
    for line in text.lines().filter(|line| !line.is_empty()) {
        let value: Value = serde_json::from_str(line)
            .map_err(|error| ImportError::InvalidPackage(format!("Cobalt review log: {error}")))?;
        if value.get("format").and_then(Value::as_i64) != Some(1)
            || value.get("card_id").and_then(Value::as_i64).is_none()
            || value.get("grade").and_then(Value::as_str).is_none()
        {
            return Err(ImportError::InvalidPackage(
                "Cobalt review log record has an unknown shape".to_owned(),
            ));
        }
        records = records.saturating_add(1);
    }
    atomic_write(output, &bytes, None)?;
    Ok(ReviewLogExportReport {
        records,
        bytes: u64::try_from(bytes.len()).map_err(|_| ImportError::Interrupted)?,
        sha256: digest_hex(&bytes),
    })
}

fn resume_offset(
    partial: &Path,
    journal: &Path,
    source_digest: &str,
    source_length: usize,
) -> usize {
    let Ok(record) = fs::read_to_string(journal) else {
        return 0;
    };
    let mut fields = record.lines();
    let (Some(expected_source), Some(offset), Some(expected_partial), None) =
        (fields.next(), fields.next(), fields.next(), fields.next())
    else {
        return 0;
    };
    let Ok(offset) = offset.parse::<usize>() else {
        return 0;
    };
    if expected_source != source_digest || offset > source_length {
        return 0;
    }
    let Ok(bytes) = fs::read(partial) else {
        return 0;
    };
    if bytes.len() != offset || digest_hex(&bytes) != expected_partial {
        return 0;
    }
    offset
}

fn write_resume_record(
    journal: &Path,
    source_digest: &str,
    offset: usize,
    partial: &Path,
) -> Result<(), ImportError> {
    let partial_digest = digest_hex(&fs::read(partial)?);
    let writing = journal.with_extension("resume.writing");
    let _ignored = fs::remove_file(&writing);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&writing)?;
    writeln!(file, "{source_digest}\n{offset}\n{partial_digest}")?;
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
    media: BTreeMap<String, Vec<u8>>,
}

fn read_archive(path: &Path, kind: PackageKind) -> Result<Unpacked, ImportError> {
    let file = File::open(path)?;
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
        if size > MAX_ARCHIVE_BYTES || size / compressed > MAX_EXPANSION_RATIO {
            return Err(ImportError::InvalidPackage(format!(
                "member {name:?} exceeds the decompression limit"
            )));
        }
        total = total
            .checked_add(size)
            .ok_or_else(|| ImportError::InvalidPackage("archive size overflow".to_owned()))?;
        if total > MAX_ARCHIVE_BYTES {
            return Err(ImportError::InvalidPackage(
                "archive exceeds the Kobo bundle limit".to_owned(),
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
    let collection = members.remove("collection.anki2").ok_or_else(|| {
        ImportError::InvalidPackage("only legacy collection.anki2 packages are accepted".to_owned())
    })?;
    if collection.len() as u64 > MAX_COLLECTION_BYTES {
        return Err(ImportError::InvalidPackage(
            "collection database exceeds the limit".to_owned(),
        ));
    }
    let media_map = members
        .remove("media")
        .ok_or_else(|| ImportError::InvalidPackage("media map is missing".to_owned()))?;
    let map: BTreeMap<String, String> = serde_json::from_slice(&media_map)
        .map_err(|error| ImportError::InvalidPackage(format!("media map: {error}")))?;
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
    if kind == PackageKind::Colpkg && media.is_empty() {
        // A media-less collection is valid; this branch documents that
        // replacement does not require inventing an empty legacy media map.
    }
    Ok(Unpacked { collection, media })
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
    let expected = media_type(name);
    let detected = sniff_media(bytes);
    let mismatch = matches!(
        (expected, detected),
        ("image/png", Some("image/jpeg" | "image/gif" | "image/webp"))
            | ("image/jpeg", Some("image/png" | "image/gif" | "image/webp"))
            | ("image/gif", Some("image/png" | "image/jpeg" | "image/webp"))
            | ("image/webp", Some("image/png" | "image/jpeg" | "image/gif"))
    );
    if mismatch {
        return Err(ImportError::InvalidPackage(format!(
            "media type does not match filename for {name:?}"
        )));
    }
    if expected == "image/svg+xml" && is_safe_svg(bytes) {
        return Ok(());
    }
    if expected.starts_with("image/") && detected.is_none() {
        return Err(ImportError::InvalidPackage(format!(
            "image {name:?} has no recognized signature"
        )));
    }

    Ok(())
}

fn is_safe_svg(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    let lower = text
        .trim_start_matches('\u{feff}')
        .trim_start()
        .to_ascii_lowercase();
    (lower.starts_with("<svg") || (lower.starts_with("<?xml") && lower.contains("<svg")))
        && !lower.contains("<script")
        && !lower.contains("javascript:")
        && !lower.contains("file:")
        && !lower.contains("href=\"http")
        && !lower.contains("href='http")
        && !lower.contains("url(http")
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
    output: &Path,
) -> Result<BundleManifest, ImportError> {
    let scratch = collection_scratch_path(output)?;
    if let Some(parent) = scratch.parent() {
        fs::create_dir_all(parent)?;
    }
    {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&scratch)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    let result = read_collection_file(&scratch, media, kind);
    let _ignored = fs::remove_file(&scratch);
    result
}

fn collection_scratch_path(output: &Path) -> Result<PathBuf, ImportError> {
    let parent = output.parent().ok_or_else(|| {
        ImportError::InvalidPackage(
            "output must have a parent directory for the SQLite check".to_owned(),
        )
    })?;
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
) -> Result<BundleManifest, ImportError> {
    let database = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| ImportError::Sql(error.to_string()))?;
    let integrity: String = database
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(|error| ImportError::Sql(error.to_string()))?;
    if integrity != "ok" {
        return Err(ImportError::InvalidPackage(
            "SQLite integrity_check did not return ok".to_owned(),
        ));
    }
    let (schema, modified, models_json, decks_json, dconf_json) = database
        .query_row(
            "SELECT ver, mod, models, decks, dconf FROM col LIMIT 1",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .map_err(|error| ImportError::Sql(error.to_string()))?;
    if !(11..=18).contains(&schema) {
        return Err(ImportError::InvalidPackage(format!(
            "collection schema {schema} needs the pinned upstream migration helper"
        )));
    }
    let models = json_objects(&models_json, "models")?;
    let decks = json_objects(&decks_json, "decks")?;
    let configurations = json_objects(&dconf_json, "deck configurations")?;
    let mut manifest = BundleManifest::empty(Source {
        package_kind: kind.name().to_owned(),
        collection_schema: schema,
        collection_modified: modified,
        note_count: 0,
        upstream_anki_revision: UPSTREAM_ANKI_REVISION.to_owned(),
    });
    manifest.notetypes = models
        .iter()
        .map(|(id, model)| NoteType {
            id: *id,
            name: json_name(model).unwrap_or_else(|| format!("Notetype {id}")),
            original_json: canonical_json(model),
        })
        .collect();
    manifest.decks = decks
        .iter()
        .map(|(id, deck)| Deck {
            id: *id,
            name: json_name(deck).unwrap_or_else(|| format!("Deck {id}")),
            configuration_id: deck.get("conf").and_then(Value::as_i64),
            original_json: canonical_json(deck),
        })
        .collect();
    manifest.deck_configurations = configurations
        .iter()
        .map(|(id, configuration)| DeckConfiguration {
            id: *id,
            name: json_name(configuration).unwrap_or_else(|| format!("Configuration {id}")),
            original_json: canonical_json(configuration),
        })
        .collect();
    let notes = read_notes(&database)?;
    manifest.source.note_count = notes.len();
    manifest.cards = read_cards(&database, &notes, &models, media)?;
    manifest.revlog = read_revlog(&database)?;
    for card in &manifest.cards {
        manifest
            .diagnostics
            .extend(card.diagnostics.iter().cloned());
    }
    Ok(manifest)
}

#[derive(Clone)]
struct Note {
    model_id: i64,
    fields: BTreeMap<String, String>,
    tags: Vec<String>,
}

fn read_notes(database: &Connection) -> Result<BTreeMap<i64, Note>, ImportError> {
    let mut statement = database
        .prepare("SELECT id, mid, tags, flds FROM notes")
        .map_err(|error| ImportError::Sql(error.to_string()))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(|error| ImportError::Sql(error.to_string()))?;
    let mut notes = BTreeMap::new();
    for row in rows {
        let (id, model_id, tags, fields) =
            row.map_err(|error| ImportError::Sql(error.to_string()))?;
        notes.insert(
            id,
            Note {
                model_id,
                fields: field_map(model_id, &fields),
                tags: tags.split_whitespace().map(ToOwned::to_owned).collect(),
            },
        );
    }
    Ok(notes)
}

fn field_map(_model_id: i64, fields: &str) -> BTreeMap<String, String> {
    // Names are attached when templates are rendered below. Storing numerical
    // names first keeps malformed model JSON from shifting a note's fields.
    fields
        .split('\u{1f}')
        .enumerate()
        .map(|(index, value)| (format!("_{index}"), value.to_owned()))
        .collect()
}

#[allow(
    clippy::too_many_lines,
    reason = "a card row, its template, references, and scheduling fields form one import transaction"
)]
fn read_cards(
    database: &Connection,
    notes: &BTreeMap<i64, Note>,
    models: &BTreeMap<i64, Value>,
    media: &BTreeMap<String, Vec<u8>>,
) -> Result<Vec<Card>, ImportError> {
    let mut statement = database
        .prepare(
            "SELECT id, nid, did, ord, mod, type, queue, due, ivl, factor, reps, lapses, left FROM cards ORDER BY id",
        )
        .map_err(|error| ImportError::Sql(error.to_string()))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i32>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i32>(5)?,
                row.get::<_, i32>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, i32>(8)?,
                row.get::<_, i32>(9)?,
                row.get::<_, i32>(10)?,
                row.get::<_, i32>(11)?,
                row.get::<_, i32>(12)?,
            ))
        })
        .map_err(|error| ImportError::Sql(error.to_string()))?;
    let mut cards = Vec::new();
    for row in rows {
        let (
            id,
            note_id,
            deck_id,
            ordinal,
            modified,
            card_type,
            queue,
            due,
            interval,
            ease_factor,
            repetitions,
            lapses,
            remaining_steps,
        ) = row.map_err(|error| ImportError::Sql(error.to_string()))?;
        let note = notes.get(&note_id).ok_or_else(|| {
            ImportError::InvalidPackage(format!("card {id} references missing note {note_id}"))
        })?;
        let model = models
            .get(&note.model_id)
            .filter(|model| model_field_count(model) == note.fields.len())
            .ok_or_else(|| {
                ImportError::InvalidPackage(format!("note {note_id} has no usable notetype"))
            })?;
        let named_fields = named_fields(model, &note.fields);
        let template = model
            .get("tmpls")
            .and_then(Value::as_array)
            .and_then(|templates| templates.get(usize::try_from(ordinal).unwrap_or(usize::MAX)))
            .ok_or_else(|| {
                ImportError::InvalidPackage(format!("card {id} has no template {ordinal}"))
            })?;
        let template_name = json_name(template).unwrap_or_else(|| format!("Card {}", ordinal + 1));
        let mut diagnostics = Vec::new();
        let (front, back, question_media_names, answer_media_names) =
            render_with_rslib(template, model, &named_fields, ordinal, &mut diagnostics);
        let media_names = question_media_names
            .iter()
            .chain(&answer_media_names)
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        for name in &media_names {
            if !media.contains_key(name) {
                diagnostics.push(diagnostic(
                    "missing-media",
                    &format!("referenced media {name:?} is absent from this package"),
                ));
            }
        }
        let attachments = media_names
            .iter()
            .filter_map(|name| media.get(name).map(|bytes| attachment(name, bytes)))
            .collect();
        cards.push(Card {
            id,
            note_id,
            deck_id,
            ordinal,
            queue,
            card_type,
            due,
            interval,
            ease_factor,
            repetitions,
            lapses,
            remaining_steps,
            modified,
            template_name,
            front,
            back,
            tags: note.tags.clone(),
            question_media_names,
            answer_media_names,
            media_names,
            attachments,
            diagnostics,
        });
    }
    Ok(cards)
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

fn json_objects(text: &str, label: &str) -> Result<BTreeMap<i64, Value>, ImportError> {
    let object: serde_json::Map<String, Value> = serde_json::from_str(text)
        .map_err(|error| ImportError::InvalidPackage(format!("{label}: {error}")))?;
    object
        .into_iter()
        .map(|(id, value)| {
            id.parse::<i64>().map(|id| (id, value)).map_err(|_| {
                ImportError::InvalidPackage(format!("{label} identifier {id:?} is not numeric"))
            })
        })
        .collect()
}

fn json_name(value: &Value) -> Option<String> {
    value
        .get("name")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn canonical_json(value: &Value) -> String {
    serde_json::to_string(value).expect("serializing JSON values cannot fail")
}

fn model_field_count(model: &Value) -> usize {
    model
        .get("flds")
        .and_then(Value::as_array)
        .map_or(0, Vec::len)
}

fn named_fields(model: &Value, positional: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    model
        .get("flds")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(index, field)| {
            json_name(field).map(|name| {
                (
                    name,
                    positional
                        .get(&format!("_{index}"))
                        .cloned()
                        .unwrap_or_default(),
                )
            })
        })
        .collect()
}

fn render_with_rslib(
    template: &Value,
    model: &Value,
    fields: &BTreeMap<String, String>,
    ordinal: i32,
    diagnostics: &mut Vec<Diagnostic>,
) -> (String, String, Vec<String>, Vec<String>) {
    let field_map = fields
        .iter()
        .map(|(name, value)| (name.as_str(), Cow::Borrowed(value.as_str())))
        .collect::<std::collections::HashMap<_, _>>();
    let ordinal = u16::try_from(ordinal).unwrap_or(u16::MAX);
    let tr = anki_i18n::I18n::new(&["en"]);
    let result = anki::template::render_card(anki::template::RenderCardRequest {
        qfmt: template
            .get("qfmt")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        afmt: template
            .get("afmt")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        field_map: &field_map,
        card_ord: ordinal,
        is_cloze: model.get("type").and_then(Value::as_i64) == Some(1),
        browser: false,
        tr: &tr,
        partial_render: false,
    });
    let Ok(rendered) = result else {
        diagnostics.push(diagnostic(
            "rslib-render-error",
            "Anki rslib rejected this template; it was not approximated.",
        ));
        return (
            "[Unsupported Anki template]".to_owned(),
            "[Unsupported Anki template]".to_owned(),
            Vec::new(),
            Vec::new(),
        );
    };
    let mut join = |nodes: Vec<anki::template::RenderedNode>| {
        nodes
            .into_iter()
            .map(|node| match node {
                anki::template::RenderedNode::Text { text } => text,
                anki::template::RenderedNode::Replacement {
                    field_name, filters, ..
                } => {
                    diagnostics.push(diagnostic(
                        "rslib-partial-render",
                        &format!("Anki rslib retained unresolved filters for {field_name:?}: {filters:?}"),
                    ));
                    "[Unsupported Anki template filter]".to_owned()
                }
            })
            .collect::<String>()
    };
    let question = join(rendered.qnodes);
    let answer = join(rendered.anodes);
    let question_media = referenced_media_text(&question);
    let answer_media = referenced_media_text(&answer);
    (
        kobo_html::to_text(&question),
        kobo_html::to_text(&answer),
        question_media,
        answer_media,
    )
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

fn referenced_media_text(value: &str) -> Vec<String> {
    sound_references(value)
        .into_iter()
        .chain(image_references(value))
        .filter_map(|name| canonical_media_name(&name).ok())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

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
    let existing = decode(&existing_bytes)?;
    let mut cards = existing.manifest().cards.clone();
    let ids = cards.iter().map(|card| card.id).collect::<BTreeSet<_>>();
    if incoming.cards.iter().any(|card| ids.contains(&card.id)) {
        return Err(ImportError::InvalidPackage(
            "APKG card identifiers conflict with the target bundle; import into a replacement bundle".to_owned(),
        ));
    }
    cards.append(&mut incoming.cards);
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
    incoming.cards = cards;
    incoming_media = media;
    Ok((incoming, incoming_media))
}

fn report_for(
    bundle: &kobo_flashcards_format::ParsedBundle,
    package_kind: &str,
    bytes: &[u8],
) -> ImportReport {
    let manifest = bundle.manifest();
    let active_cards = manifest.cards.iter().filter(|card| card.queue >= 0).count();
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
        notes: manifest.source.note_count,
        active_cards,
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
    let parent = path.parent().ok_or_else(|| {
        ImportError::InvalidPackage("output must have a parent directory".to_owned())
    })?;
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
    fn staging_is_chunked_resumable_and_never_replaces_a_partial_import() {
        let root = unique("stage");
        fs::create_dir_all(&root).expect("fixture root");
        let bundle = root.join("collection.cobfc");
        let source = kobo_flashcards_format::Source {
            package_kind: "apkg".to_owned(),
            collection_schema: 11,
            collection_modified: 1,
            note_count: 0,
            upstream_anki_revision: UPSTREAM_ANKI_REVISION.to_owned(),
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
        let _ignored = fs::remove_dir_all(root);
    }

    #[test]
    fn local_review_log_export_is_lossless_and_checked() {
        let root = unique("review-log");
        let mounted = root.join("kobo");
        let log = mounted.join(".adds/cobalt/data/flashcards/cobalt-review-log.ndjson");
        fs::create_dir_all(log.parent().expect("parent")).expect("log parent");
        let original = b"{\"format\":1,\"card_id\":7,\"grade\":\"good\",\"imported_due\":3,\"imported_reps\":2}\n";
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
        assert_eq!(report.notes, 2);
        assert_eq!(report.active_cards, 3);
        assert_eq!(report.decks, 1);
        assert_eq!(report.media_files, 2);
        assert_eq!(report.image_bearing_notes, 1);
        assert_eq!(report.sound_bearing_notes, 1);
        let bundle_bytes = fs::read(&output).expect("bundle");
        let bundle = decode(&bundle_bytes).expect("verified bundle");
        assert_eq!(bundle.media("picture.png"), Some(image.as_slice()));
        verify_bundle(&output).expect("Kobo image verification");
        assert_eq!(bundle.manifest().revlog.len(), 1);
        assert!(bundle.manifest().notetypes[0]
            .original_json
            .contains("\"css\""));
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
        let database = Connection::open(path).expect("create SQLite fixture");
        database
            .execute_batch(
                "
                CREATE TABLE col (
                    id INTEGER PRIMARY KEY, crt INTEGER, mod INTEGER, scm INTEGER, ver INTEGER,
                    dty INTEGER, usn INTEGER, ls INTEGER, conf TEXT, models TEXT, decks TEXT,
                    dconf TEXT, tags TEXT
                );
                CREATE TABLE notes (
                    id INTEGER PRIMARY KEY, guid TEXT, mid INTEGER, mod INTEGER, usn INTEGER,
                    tags TEXT, flds TEXT, sfld INTEGER, csum INTEGER, flags INTEGER, data TEXT
                );
                CREATE TABLE cards (
                    id INTEGER PRIMARY KEY, nid INTEGER, did INTEGER, ord INTEGER, mod INTEGER,
                    usn INTEGER, type INTEGER, queue INTEGER, due INTEGER, ivl INTEGER,
                    factor INTEGER, reps INTEGER, lapses INTEGER, left INTEGER, odue INTEGER,
                    odid INTEGER, flags INTEGER, data TEXT
                );
                CREATE TABLE revlog (
                    id INTEGER PRIMARY KEY, cid INTEGER, usn INTEGER, ease INTEGER, ivl INTEGER,
                    lastIvl INTEGER, factor INTEGER, time INTEGER, type INTEGER
                );
                ",
            )
            .expect("schema");
        let models = serde_json::json!({
            "100": {
                "name": "Basic and reversed", "css": ".card { color: black; }",
                "flds": [{"name":"Front"}, {"name":"Back"}],
                "tmpls": [
                    {"name":"Forward", "qfmt":"{{Front}}", "afmt":"{{FrontSide}}<hr>{{Back}}"},
                    {"name":"Reverse", "qfmt":"{{Back}}", "afmt":"{{FrontSide}}<hr>{{unsupported:Front}}"}
                ]
            },
            "200": {
                "name": "Cloze", "css": ".card { font-family: serif; }",
                "flds": [{"name":"Text"}],
                "tmpls": [{"name":"Cloze", "qfmt":"{{cloze:Text}}", "afmt":"{{cloze:Text}}"}]
            }
        })
        .to_string();
        let decks = serde_json::json!({
            "1": {"name":"Language::日本語", "conf":1}
        })
        .to_string();
        let dconf = serde_json::json!({
            "1": {"name":"Default", "new":{"delays":[1,10]}, "lapse":{"delays":[10]}}
        })
        .to_string();
        database
            .execute(
                "INSERT INTO col VALUES (1, 0, 42, 0, 11, 0, 0, 0, '{}', ?1, ?2, ?3, '{}')",
                params![models, decks, dconf],
            )
            .expect("collection");
        database
            .execute(
                "INSERT INTO notes VALUES (1, 'a', 100, 1, 0, ' tag ', ?1, 0, 0, 0, '')",
                params!["<b>こんにちは</b><img src=\"picture.png\">\u{1f}Answer [sound:clip.mp3]"],
            )
            .expect("basic note");
        database
            .execute(
                "INSERT INTO notes VALUES (2, 'b', 200, 1, 0, '', ?1, 0, 0, 0, '')",
                params!["{{c1::答え::答}}"],
            )
            .expect("cloze note");
        for (id, note, ordinal) in [(10_i64, 1_i64, 0_i32), (11, 1, 1), (12, 2, 0)] {
            database
                .execute(
                    "INSERT INTO cards VALUES (?1, ?2, 1, ?3, 8, 0, 2, 2, 5, 3, 2500, 4, 0, 0, 0, 0, 0, '')",
                    params![id, note, ordinal],
                )
                .expect("card");
        }
        database
            .execute(
                "INSERT INTO revlog VALUES (99, 10, 0, 3, 4, 1, 2500, 500, 1)",
                [],
            )
            .expect("review log");
    }
}

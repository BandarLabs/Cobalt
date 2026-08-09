use crate::{
    FormatError, FORMAT_VERSION, MAX_APP_ID_BYTES, MAX_BINARY_BYTES, MAX_CAPABILITIES,
    MAX_CATALOG_BYTES, MAX_CATALOG_ENTRIES, MAX_DISPLAY_NAME_BYTES, MAX_GLYPH_BYTES,
    MAX_MANIFEST_BYTES, MAX_PACKAGE_BYTES, MAX_PACKAGE_URL_BYTES, MAX_SHORT_LABEL_BYTES,
    MAX_SUMMARY_BYTES, MAX_VERSION_BYTES,
};
use kobo_json::Value;
use kobo_policy::{Capability, Declared};
use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

const MANIFEST_FIELDS: [&str; 11] = [
    "format_version",
    "id",
    "display_name",
    "short_label",
    "summary",
    "version",
    "minimum_cobalt_version",
    "glyph",
    "capabilities",
    "binary_sha256",
    "binary_bytes",
];
const CATALOG_FIELDS: [&str; 2] = ["format_version", "entries"];
const ENTRY_FIELDS: [&str; 4] = ["manifest", "package_url", "package_sha256", "package_bytes"];
const MAX_CAPABILITY_NAME_BYTES: usize = 32;

static PUBLIC_RESERVED_APP_IDS: &[&str] = &[
    "audiobook",
    "brief",
    "chat",
    "cobalt",
    "gallery",
    "gutenbird",
    "hn",
    "kobod",
    "launcher",
    "magnet",
    "rss",
    "settings",
    "sidekick",
    "store",
    "terminal",
    "tictactoe",
    "todo",
];

/// Platform-owned application identifiers that public packages may not use.
#[must_use]
pub fn public_reserved_app_ids() -> &'static [&'static str] {
    PUBLIC_RESERVED_APP_IDS
}

/// Returns whether `id` is reserved for Cobalt itself.
#[must_use]
pub fn is_public_reserved_app_id(id: &str) -> bool {
    PUBLIC_RESERVED_APP_IDS.contains(&id)
}

/// Returns whether the released runtime can render this manifest glyph.
#[must_use]
pub fn is_public_glyph(name: &str) -> bool {
    matches!(
        name,
        "app"
            | "book"
            | "note"
            | "clock"
            | "settings"
            | "folder"
            | "chart"
            | "search"
            | "wifi"
            | "battery"
            | "reader"
            | "power"
            | "grid"
            | "circle"
            | "check"
            | "terminal"
            | "chat"
            | "news"
            | "rss"
            | "light"
            | "close"
            | "download"
            | "bookmark"
            | "filter"
            | "person"
            | "tag"
            | "globe"
            | "refresh"
            | "more"
            | "bluetooth"
            | "key"
            | "magnet"
            | "play"
            | "pause"
            | "rewind30"
            | "forward30"
            | "volume-down"
            | "volume-up"
            | "more-vertical"
            | "trash"
            | "previous"
            | "next"
            | "plus"
            | "headphones"
    )
}

/// A validated lowercase SHA-256 digest.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Sha256Digest(String);

impl Sha256Digest {
    /// Parses exactly 64 lowercase hexadecimal characters.
    ///
    /// # Errors
    ///
    /// Returns [`FormatError::InvalidValue`] for any other spelling.
    pub fn parse(value: &str, field: &'static str) -> Result<Self, FormatError> {
        if is_lower_hex(value, 64) {
            Ok(Self(value.to_owned()))
        } else {
            Err(FormatError::InvalidValue {
                field,
                reason: "must be 64 lowercase hexadecimal characters",
            })
        }
    }

    /// The canonical lowercase hexadecimal spelling.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Sha256Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for Sha256Digest {
    type Err = FormatError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value, "sha256")
    }
}

/// Unvalidated owned fields used to construct a [`Manifest`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManifestInput {
    pub id: String,
    pub display_name: String,
    pub short_label: String,
    pub summary: String,
    pub version: String,
    pub minimum_cobalt_version: String,
    pub glyph: String,
    pub capabilities: Vec<String>,
    pub binary_sha256: String,
    pub binary_bytes: u64,
}

/// A fully validated application manifest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Manifest {
    id: String,
    display_name: String,
    short_label: String,
    summary: String,
    version: String,
    minimum_cobalt_version: String,
    glyph: String,
    capabilities: Declared,
    binary_sha256: Sha256Digest,
    binary_bytes: u64,
}

impl Manifest {
    /// Validates fields, rejecting identifiers present in `reserved_ids`.
    ///
    /// # Errors
    ///
    /// Returns an error when any field is malformed, unbounded, reserved, or
    /// contains an invalid capability declaration.
    pub fn new(input: ManifestInput, reserved_ids: &[&str]) -> Result<Self, FormatError> {
        validate_identifier(&input.id, "id", MAX_APP_ID_BYTES)?;
        if reserved_ids.contains(&input.id.as_str()) {
            return Err(FormatError::ReservedAppId(input.id));
        }
        validate_text(&input.display_name, "display_name", MAX_DISPLAY_NAME_BYTES)?;
        validate_text(&input.short_label, "short_label", MAX_SHORT_LABEL_BYTES)?;
        validate_text(&input.summary, "summary", MAX_SUMMARY_BYTES)?;
        validate_version(&input.version, "version")?;
        validate_version(&input.minimum_cobalt_version, "minimum_cobalt_version")?;
        validate_identifier(&input.glyph, "glyph", MAX_GLYPH_BYTES)?;
        let capabilities = validate_capabilities(&input.capabilities)?;
        let binary_sha256 = Sha256Digest::parse(&input.binary_sha256, "binary_sha256")?;
        validate_count(input.binary_bytes, "binary_bytes", MAX_BINARY_BYTES)?;

        Ok(Self {
            id: input.id,
            display_name: input.display_name,
            short_label: input.short_label,
            summary: input.summary,
            version: input.version,
            minimum_cobalt_version: input.minimum_cobalt_version,
            glyph: input.glyph,
            capabilities,
            binary_sha256,
            binary_bytes: input.binary_bytes,
        })
    }

    /// Validates fields against Cobalt's public reserved identifiers.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::new`].
    pub fn new_public(input: ManifestInput) -> Result<Self, FormatError> {
        Self::validate_public_manifest(Self::new(input, public_reserved_app_ids())?)
    }

    /// Parses strict UTF-8 manifest JSON with caller-supplied reserved IDs.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid UTF-8/JSON, unknown or duplicate fields,
    /// unsupported format versions, or invalid manifest values.
    pub fn parse(json: &[u8], reserved_ids: &[&str]) -> Result<Self, FormatError> {
        let value = parse_document(json, MAX_MANIFEST_BYTES)?;
        parse_manifest_value(&value, reserved_ids)
    }

    /// Parses strict UTF-8 manifest JSON using Cobalt's reserved IDs.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::parse`].
    pub fn parse_public(json: &[u8]) -> Result<Self, FormatError> {
        Self::validate_public_manifest(Self::parse(json, public_reserved_app_ids())?)
    }

    fn validate_public_manifest(manifest: Manifest) -> Result<Manifest, FormatError> {
        manifest.ensure_public()?;
        Ok(manifest)
    }

    pub(crate) fn ensure_public(&self) -> Result<(), FormatError> {
        if !is_public_glyph(&self.glyph) {
            return Err(FormatError::InvalidValue {
                field: "glyph",
                reason: "is not supported by released Cobalt runtimes",
            });
        }
        if self.capabilities.holds(Capability::Shell) {
            return Err(FormatError::InvalidValue {
                field: "capabilities",
                reason: "public applications cannot request shell access",
            });
        }
        Ok(())
    }

    /// Deterministic compact JSON used for signatures.
    #[must_use]
    pub fn to_canonical_json(&self) -> String {
        let mut out = String::with_capacity(512);
        write_manifest(self, &mut out);
        out
    }

    /// Deterministic UTF-8 JSON bytes used for signatures.
    #[must_use]
    pub fn to_canonical_bytes(&self) -> Vec<u8> {
        self.to_canonical_json().into_bytes()
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    #[must_use]
    pub fn short_label(&self) -> &str {
        &self.short_label
    }

    #[must_use]
    pub fn summary(&self) -> &str {
        &self.summary
    }

    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    #[must_use]
    pub fn minimum_cobalt_version(&self) -> &str {
        &self.minimum_cobalt_version
    }

    #[must_use]
    pub fn glyph(&self) -> &str {
        &self.glyph
    }

    pub fn capabilities(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.capabilities
            .iter()
            .map(kobo_policy::Capability::manifest_name)
    }

    #[must_use]
    pub fn declared_capabilities(&self) -> &Declared {
        &self.capabilities
    }

    #[must_use]
    pub fn binary_sha256(&self) -> &Sha256Digest {
        &self.binary_sha256
    }

    #[must_use]
    pub fn binary_bytes(&self) -> u64 {
        self.binary_bytes
    }
}

/// Unvalidated package fields used to construct a [`CatalogEntry`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogEntryInput {
    pub manifest: Manifest,
    pub package_url: String,
    pub package_sha256: String,
    pub package_bytes: u64,
}

/// A validated catalog entry and the package carrying its bundle.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogEntry {
    manifest: Manifest,
    package_url: String,
    package_sha256: Sha256Digest,
    package_bytes: u64,
}

impl CatalogEntry {
    /// Validates package metadata around an already validated manifest.
    ///
    /// # Errors
    ///
    /// Returns an error for a non-HTTPS URL, malformed digest, or invalid
    /// package byte count.
    pub fn new(input: CatalogEntryInput) -> Result<Self, FormatError> {
        validate_package_url(&input.package_url)?;
        let package_sha256 = Sha256Digest::parse(&input.package_sha256, "package_sha256")?;
        validate_count(input.package_bytes, "package_bytes", MAX_PACKAGE_BYTES)?;
        Ok(Self {
            manifest: input.manifest,
            package_url: input.package_url,
            package_sha256,
            package_bytes: input.package_bytes,
        })
    }

    #[must_use]
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    #[must_use]
    pub fn package_url(&self) -> &str {
        &self.package_url
    }

    #[must_use]
    pub fn package_sha256(&self) -> &Sha256Digest {
        &self.package_sha256
    }

    #[must_use]
    pub fn package_bytes(&self) -> u64 {
        self.package_bytes
    }
}

/// A bounded catalog with unique application identifiers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Catalog {
    entries: Vec<CatalogEntry>,
}

impl Catalog {
    /// Builds a catalog, rejecting too many entries and duplicate app IDs.
    ///
    /// # Errors
    ///
    /// Returns an error when the entry count exceeds [`MAX_CATALOG_ENTRIES`]
    /// or two manifests carry the same app ID.
    pub fn new(entries: Vec<CatalogEntry>) -> Result<Self, FormatError> {
        if entries.len() > MAX_CATALOG_ENTRIES {
            return Err(FormatError::InvalidValue {
                field: "entries",
                reason: "too many catalog entries",
            });
        }
        let mut ids = BTreeSet::new();
        for entry in &entries {
            if !ids.insert(entry.manifest.id()) {
                return Err(FormatError::DuplicateAppId(entry.manifest.id().to_owned()));
            }
        }
        Ok(Self { entries })
    }

    /// Parses strict UTF-8 catalog JSON with caller-supplied reserved IDs.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed structure, entries, package metadata,
    /// duplicate app IDs, or an unsupported format version.
    pub fn parse(json: &[u8], reserved_ids: &[&str]) -> Result<Self, FormatError> {
        let value = parse_document(json, MAX_CATALOG_BYTES)?;
        let object = StrictObject::new(&value, "catalog", &CATALOG_FIELDS)?;
        validate_format(object.value("format_version")?)?;
        let entries = object
            .value("entries")?
            .as_array()
            .ok_or(FormatError::InvalidType("entries"))?;
        if entries.len() > MAX_CATALOG_ENTRIES {
            return Err(FormatError::InvalidValue {
                field: "entries",
                reason: "too many catalog entries",
            });
        }
        let parsed = entries
            .iter()
            .map(|entry| parse_catalog_entry(entry, reserved_ids))
            .collect::<Result<Vec<_>, _>>()?;
        Self::new(parsed)
    }

    /// Parses strict UTF-8 catalog JSON using Cobalt's reserved IDs.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::parse`].
    pub fn parse_public(json: &[u8]) -> Result<Self, FormatError> {
        let catalog = Self::parse(json, public_reserved_app_ids())?;
        for entry in &catalog.entries {
            entry.manifest.ensure_public()?;
        }
        Ok(catalog)
    }

    /// Deterministic compact JSON used for signatures.
    #[must_use]
    pub fn to_canonical_json(&self) -> String {
        let mut out = String::with_capacity(self.entries.len().saturating_mul(640));
        out.push_str("{\"format_version\":1,\"entries\":[");
        for (index, entry) in self.entries.iter().enumerate() {
            if index != 0 {
                out.push(',');
            }
            out.push_str("{\"manifest\":");
            write_manifest(&entry.manifest, &mut out);
            out.push_str(",\"package_url\":");
            kobo_json::escape_into(&entry.package_url, &mut out);
            out.push_str(",\"package_sha256\":");
            kobo_json::escape_into(entry.package_sha256.as_str(), &mut out);
            out.push_str(",\"package_bytes\":");
            out.push_str(&entry.package_bytes.to_string());
            out.push('}');
        }
        out.push_str("]}");
        out
    }

    /// Deterministic UTF-8 JSON bytes used for signatures.
    #[must_use]
    pub fn to_canonical_bytes(&self) -> Vec<u8> {
        self.to_canonical_json().into_bytes()
    }

    #[must_use]
    pub fn entries(&self) -> &[CatalogEntry] {
        &self.entries
    }
}

fn parse_catalog_entry(value: &Value, reserved_ids: &[&str]) -> Result<CatalogEntry, FormatError> {
    let object = StrictObject::new(value, "catalog entry", &ENTRY_FIELDS)?;
    CatalogEntry::new(CatalogEntryInput {
        manifest: parse_manifest_value(object.value("manifest")?, reserved_ids)?,
        package_url: string_field(&object, "package_url")?,
        package_sha256: string_field(&object, "package_sha256")?,
        package_bytes: count_field(&object, "package_bytes")?,
    })
}

fn parse_manifest_value(value: &Value, reserved_ids: &[&str]) -> Result<Manifest, FormatError> {
    let object = StrictObject::new(value, "manifest", &MANIFEST_FIELDS)?;
    validate_format(object.value("format_version")?)?;
    Manifest::new(
        ManifestInput {
            id: string_field(&object, "id")?,
            display_name: string_field(&object, "display_name")?,
            short_label: string_field(&object, "short_label")?,
            summary: string_field(&object, "summary")?,
            version: string_field(&object, "version")?,
            minimum_cobalt_version: string_field(&object, "minimum_cobalt_version")?,
            glyph: string_field(&object, "glyph")?,
            capabilities: capability_fields(&object)?,
            binary_sha256: string_field(&object, "binary_sha256")?,
            binary_bytes: count_field(&object, "binary_bytes")?,
        },
        reserved_ids,
    )
}

fn parse_document(json: &[u8], maximum: usize) -> Result<Value, FormatError> {
    if json.len() > maximum {
        return Err(FormatError::DocumentTooLarge { maximum });
    }
    let text = std::str::from_utf8(json).map_err(|_| FormatError::InvalidUtf8)?;
    kobo_json::parse(text).map_err(FormatError::from)
}

struct StrictObject<'a> {
    fields: &'a [(String, Value)],
    name: &'static str,
}

impl<'a> StrictObject<'a> {
    fn new(
        value: &'a Value,
        name: &'static str,
        expected: &[&'static str],
    ) -> Result<Self, FormatError> {
        let Value::Object(fields) = value else {
            return Err(FormatError::ExpectedObject(name));
        };
        let mut seen = BTreeSet::new();
        for (field, _) in fields {
            if !expected.contains(&field.as_str()) {
                return Err(FormatError::UnknownField {
                    object: name,
                    field: field.clone(),
                });
            }
            if !seen.insert(field.as_str()) {
                return Err(FormatError::DuplicateField {
                    object: name,
                    field: field.clone(),
                });
            }
        }
        for field in expected {
            if !seen.contains(field) {
                return Err(FormatError::MissingField {
                    object: name,
                    field,
                });
            }
        }
        Ok(Self { fields, name })
    }

    fn value(&self, key: &'static str) -> Result<&'a Value, FormatError> {
        self.fields
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value)
            .ok_or(FormatError::MissingField {
                object: self.name,
                field: key,
            })
    }
}

fn string_field(object: &StrictObject<'_>, field: &'static str) -> Result<String, FormatError> {
    object
        .value(field)?
        .as_str()
        .map(str::to_owned)
        .ok_or(FormatError::InvalidType(field))
}

fn count_field(object: &StrictObject<'_>, field: &'static str) -> Result<u64, FormatError> {
    let value = object
        .value(field)?
        .as_i64()
        .ok_or(FormatError::InvalidType(field))?;
    u64::try_from(value).map_err(|_| FormatError::InvalidValue {
        field,
        reason: "must be a positive integer",
    })
}

fn capability_fields(object: &StrictObject<'_>) -> Result<Vec<String>, FormatError> {
    object
        .value("capabilities")?
        .as_array()
        .ok_or(FormatError::InvalidType("capabilities"))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or(FormatError::InvalidType("capabilities"))
        })
        .collect()
}

fn validate_format(value: &Value) -> Result<(), FormatError> {
    let version = value
        .as_i64()
        .ok_or(FormatError::InvalidType("format_version"))?;
    if u64::try_from(version) == Ok(FORMAT_VERSION) {
        Ok(())
    } else {
        Err(FormatError::UnsupportedFormat(version))
    }
}

fn validate_text(value: &str, field: &'static str, maximum: usize) -> Result<(), FormatError> {
    if value.is_empty() {
        return Err(FormatError::InvalidValue {
            field,
            reason: "must not be empty",
        });
    }
    if value.len() > maximum {
        return Err(FormatError::InvalidValue {
            field,
            reason: "text is too long",
        });
    }
    if value.trim() != value || value.chars().any(char::is_control) {
        return Err(FormatError::InvalidValue {
            field,
            reason: "must be trimmed single-line text",
        });
    }
    Ok(())
}

fn validate_version(value: &str, field: &'static str) -> Result<(), FormatError> {
    validate_text(value, field, MAX_VERSION_BYTES)?;
    if value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+'))
    {
        Ok(())
    } else {
        Err(FormatError::InvalidValue {
            field,
            reason: "must be an ASCII version token",
        })
    }
}

fn validate_identifier(
    value: &str,
    field: &'static str,
    maximum: usize,
) -> Result<(), FormatError> {
    if value.is_empty() || value.len() > maximum {
        return Err(FormatError::InvalidValue {
            field,
            reason: "invalid length",
        });
    }
    let bytes = value.as_bytes();
    if !bytes[0].is_ascii_lowercase()
        || bytes.last() == Some(&b'-')
        || bytes.windows(2).any(|pair| pair == b"--")
        || !bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
    {
        return Err(FormatError::InvalidValue {
            field,
            reason: "must be a lowercase ASCII identifier",
        });
    }
    Ok(())
}

fn validate_capabilities(names: &[String]) -> Result<Declared, FormatError> {
    if names.len() > MAX_CAPABILITIES {
        return Err(FormatError::InvalidValue {
            field: "capabilities",
            reason: "too many capabilities",
        });
    }
    let mut seen = BTreeSet::new();
    for name in names {
        if name.is_empty() || name.len() > MAX_CAPABILITY_NAME_BYTES {
            return Err(FormatError::InvalidValue {
                field: "capabilities",
                reason: "invalid capability name length",
            });
        }
        if !seen.insert(name.as_str()) {
            return Err(FormatError::InvalidValue {
                field: "capabilities",
                reason: "duplicate capability",
            });
        }
    }
    Declared::parse(names.iter().map(String::as_str)).map_err(FormatError::from)
}

fn validate_count(value: u64, field: &'static str, maximum: u64) -> Result<(), FormatError> {
    if value == 0 || value > maximum {
        Err(FormatError::InvalidValue {
            field,
            reason: "byte count is outside its limit",
        })
    } else {
        Ok(())
    }
}

fn validate_package_url(value: &str) -> Result<(), FormatError> {
    if value.is_empty() || value.len() > MAX_PACKAGE_URL_BYTES || kobo_net::parse(value).is_err() {
        Err(FormatError::InvalidValue {
            field: "package_url",
            reason: "must be a valid bounded HTTPS URL without credentials",
        })
    } else {
        Ok(())
    }
}

fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn write_manifest(manifest: &Manifest, out: &mut String) {
    out.push_str("{\"format_version\":1,\"id\":");
    kobo_json::escape_into(&manifest.id, out);
    out.push_str(",\"display_name\":");
    kobo_json::escape_into(&manifest.display_name, out);
    out.push_str(",\"short_label\":");
    kobo_json::escape_into(&manifest.short_label, out);
    out.push_str(",\"summary\":");
    kobo_json::escape_into(&manifest.summary, out);
    out.push_str(",\"version\":");
    kobo_json::escape_into(&manifest.version, out);
    out.push_str(",\"minimum_cobalt_version\":");
    kobo_json::escape_into(&manifest.minimum_cobalt_version, out);
    out.push_str(",\"glyph\":");
    kobo_json::escape_into(&manifest.glyph, out);
    out.push_str(",\"capabilities\":[");
    for (index, capability) in manifest.capabilities().enumerate() {
        if index != 0 {
            out.push(',');
        }
        kobo_json::escape_into(capability, out);
    }
    out.push_str("],\"binary_sha256\":");
    kobo_json::escape_into(manifest.binary_sha256.as_str(), out);
    out.push_str(",\"binary_bytes\":");
    out.push_str(&manifest.binary_bytes.to_string());
    out.push('}');
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobo_net::sha256::hex_digest;

    fn input(id: &str, binary: &[u8]) -> ManifestInput {
        ManifestInput {
            id: id.to_owned(),
            display_name: "Daily Brief".to_owned(),
            short_label: "Brief".to_owned(),
            summary: "A concise daily briefing.".to_owned(),
            version: "1.2.3".to_owned(),
            minimum_cobalt_version: "0.1.9".to_owned(),
            glyph: "clock".to_owned(),
            capabilities: vec![
                "scheduled-wake".to_owned(),
                "network".to_owned(),
                "background-network".to_owned(),
            ],
            binary_sha256: hex_digest(binary),
            binary_bytes: binary.len() as u64,
        }
    }

    fn manifest(id: &str) -> Manifest {
        Manifest::new(input(id, b"binary"), &[]).expect("valid manifest")
    }

    fn entry(id: &str) -> CatalogEntry {
        CatalogEntry::new(CatalogEntryInput {
            manifest: manifest(id),
            package_url: format!("https://apps.example/{id}.cobalt"),
            package_sha256: hex_digest(id.as_bytes()),
            package_bytes: 100,
        })
        .expect("valid entry")
    }

    #[test]
    fn canonical_manifest_round_trip_normalizes_capability_order() {
        let manifest = manifest("daily-brief");
        let json = manifest.to_canonical_json();
        assert_eq!(Manifest::parse(json.as_bytes(), &[]), Ok(manifest));
        assert_eq!(
            json,
            format!(
                concat!(
                    r#"{{"format_version":1,"id":"daily-brief","display_name":"Daily Brief","#,
                    r#""short_label":"Brief","summary":"A concise daily briefing.","version":"1.2.3","#,
                    r#""minimum_cobalt_version":"0.1.9","glyph":"clock","capabilities":["network","#,
                    r#""background-network","scheduled-wake"],"binary_sha256":"{}","binary_bytes":6}}"#
                ),
                hex_digest(b"binary")
            )
        );
        assert_eq!(
            json,
            Manifest::parse(json.as_bytes(), &[])
                .unwrap()
                .to_canonical_json()
        );
    }

    #[test]
    fn public_applications_cannot_request_a_root_shell() {
        let mut requested = input("shell-tool", b"binary");
        requested.capabilities = vec!["shell".to_owned()];
        assert!(matches!(
            Manifest::new_public(requested.clone()),
            Err(FormatError::InvalidValue {
                field: "capabilities",
                ..
            })
        ));

        let manifest = Manifest::new(requested, &[]).expect("generic shell manifest");
        let catalog = Catalog::new(vec![CatalogEntry::new(CatalogEntryInput {
            manifest,
            package_url: "https://apps.example/shell-tool.cobalt-app".to_owned(),
            package_sha256: hex_digest(b"package"),
            package_bytes: 100,
        })
        .expect("entry")])
        .expect("catalog");
        assert!(matches!(
            Catalog::parse_public(&catalog.to_canonical_bytes()),
            Err(FormatError::InvalidValue {
                field: "capabilities",
                ..
            })
        ));
    }

    #[test]
    fn canonical_catalog_round_trip_is_deterministic() {
        let catalog = Catalog::new(vec![entry("brief"), entry("news")]).expect("catalog");
        let first = catalog.to_canonical_json();
        let second = catalog.to_canonical_json();
        assert_eq!(first, second);
        assert_eq!(Catalog::parse(first.as_bytes(), &[]), Ok(catalog));
    }

    #[test]
    fn parser_refuses_unknown_missing_and_duplicate_fields() {
        let json = manifest("brief").to_canonical_json();
        assert!(Manifest::parse(
            json.replace("\"id\":", "\"extra\":0,\"id\":").as_bytes(),
            &[]
        )
        .is_err());
        assert!(Manifest::parse(
            json.replace("\"summary\":\"A concise daily briefing.\",", "")
                .as_bytes(),
            &[]
        )
        .is_err());
        assert!(Manifest::parse(
            json.replace("\"id\":\"brief\",", "\"id\":\"brief\",\"id\":\"other\",")
                .as_bytes(),
            &[]
        )
        .is_err());
    }

    #[test]
    fn parser_refuses_unknown_formats_and_non_utf8() {
        let json = manifest("brief").to_canonical_json();
        assert_eq!(
            Manifest::parse(json.replacen(":1", ":2", 1).as_bytes(), &[]),
            Err(FormatError::UnsupportedFormat(2))
        );
        assert_eq!(Manifest::parse(&[0xff], &[]), Err(FormatError::InvalidUtf8));
    }

    #[test]
    fn ids_are_lowercase_pathless_and_reservable() {
        for id in ["Bad", "two--dashes", "trailing-", "../escape", "a_b"] {
            assert!(Manifest::new(input(id, b"x"), &[]).is_err(), "{id}");
        }
        assert_eq!(
            Manifest::new(input("private", b"x"), &["private"]),
            Err(FormatError::ReservedAppId("private".to_owned()))
        );
        assert!(is_public_reserved_app_id("launcher"));
        assert!(Manifest::new_public(input("launcher", b"x")).is_err());
    }

    #[test]
    fn malformed_digests_and_counts_are_refused() {
        let mut bad = input("brief", b"x");
        bad.binary_sha256 = "A".repeat(64);
        assert!(Manifest::new(bad, &[]).is_err());
        let mut bad = input("brief", b"x");
        bad.binary_bytes = 0;
        assert!(Manifest::new(bad, &[]).is_err());
        let mut bad = input("brief", b"x");
        bad.binary_bytes = MAX_BINARY_BYTES + 1;
        assert!(Manifest::new(bad, &[]).is_err());

        let result = CatalogEntry::new(CatalogEntryInput {
            manifest: manifest("brief"),
            package_url: "https://apps.example/brief".to_owned(),
            package_sha256: "0".repeat(63),
            package_bytes: 1,
        });
        assert!(result.is_err());
    }

    #[test]
    fn malformed_urls_are_refused() {
        for url in [
            "http://apps.example/a",
            "https://",
            "https://user:password@apps.example/a",
            "not a url",
        ] {
            let result = CatalogEntry::new(CatalogEntryInput {
                manifest: manifest("brief"),
                package_url: url.to_owned(),
                package_sha256: "a".repeat(64),
                package_bytes: 1,
            });
            assert!(result.is_err(), "{url}");
        }
    }

    #[test]
    fn unknown_duplicate_and_incomplete_capabilities_are_refused() {
        let mut bad = input("brief", b"x");
        bad.capabilities = vec!["sudo".to_owned()];
        assert!(Manifest::new(bad, &[]).is_err());
        let mut bad = input("brief", b"x");
        bad.capabilities = vec!["network".to_owned(), "network".to_owned()];
        assert!(Manifest::new(bad, &[]).is_err());
        let mut bad = input("brief", b"x");
        bad.capabilities = vec!["background-network".to_owned()];
        assert!(Manifest::new(bad, &[]).is_err());
    }

    #[test]
    fn duplicate_catalog_ids_are_refused() {
        assert_eq!(
            Catalog::new(vec![entry("brief"), entry("brief")]),
            Err(FormatError::DuplicateAppId("brief".to_owned()))
        );
    }

    #[test]
    fn oversized_fields_lists_documents_and_counts_are_refused() {
        let mut bad = input("brief", b"x");
        bad.summary = "x".repeat(MAX_SUMMARY_BYTES + 1);
        assert!(Manifest::new(bad, &[]).is_err());
        let mut bad = input("brief", b"x");
        bad.capabilities = vec!["network".to_owned(); MAX_CAPABILITIES + 1];
        assert!(Manifest::new(bad, &[]).is_err());
        assert_eq!(
            Manifest::parse(&vec![b' '; MAX_MANIFEST_BYTES + 1], &[]),
            Err(FormatError::DocumentTooLarge {
                maximum: MAX_MANIFEST_BYTES
            })
        );
        assert!(Catalog::new(vec![entry("brief"); MAX_CATALOG_ENTRIES + 1]).is_err());
        let result = CatalogEntry::new(CatalogEntryInput {
            manifest: manifest("brief"),
            package_url: "https://apps.example/a".to_owned(),
            package_sha256: "a".repeat(64),
            package_bytes: MAX_PACKAGE_BYTES + 1,
        });
        assert!(result.is_err());
    }
}

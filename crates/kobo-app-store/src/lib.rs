#![forbid(unsafe_code)]

//! Signed, pathless application distribution for Cobalt.
//!
//! Manifests and catalogs have a single compact canonical JSON spelling.
//! Signatures cover those exact UTF-8 bytes. A bundle contains only a signed
//! manifest followed by one executable byte string; it has no path or archive
//! member names to interpret.

mod bundle;
mod error;
mod model;
mod release;
mod signing;

pub use bundle::{build_bundle, parse_bundle, parse_public_bundle, ParsedBundle};
pub use error::{BundleError, FormatError, SignatureError};
pub use model::{
    cobalt_version_at_least, is_public_glyph, is_public_reserved_app_id, public_reserved_app_ids,
    Catalog, CatalogEntry, CatalogEntryInput, Manifest, ManifestInput, Sha256Digest,
};
pub use release::{verify_release_manifest, ReleaseAsset, ReleaseManifest};
pub use signing::{derive_public_key, sign, verify, DetachedSignature, Ed25519PublicKey};

/// JSON schema version understood by this crate.
pub const FORMAT_VERSION: u64 = 1;

/// Maximum UTF-8 byte length of an application identifier.
pub const MAX_APP_ID_BYTES: usize = 32;
/// Maximum UTF-8 byte length of a display name.
pub const MAX_DISPLAY_NAME_BYTES: usize = 96;
/// Maximum UTF-8 byte length of the compact launcher label.
pub const MAX_SHORT_LABEL_BYTES: usize = 32;
/// Maximum UTF-8 byte length of an application summary.
pub const MAX_SUMMARY_BYTES: usize = 512;
/// Maximum UTF-8 byte length of a version string.
pub const MAX_VERSION_BYTES: usize = 64;
/// Maximum UTF-8 byte length of a glyph name.
pub const MAX_GLYPH_BYTES: usize = 64;
/// Maximum number of declared capabilities.
pub const MAX_CAPABILITIES: usize = 16;
/// Maximum UTF-8 byte length of a package URL.
pub const MAX_PACKAGE_URL_BYTES: usize = 2_048;
/// Maximum number of applications in one catalog.
pub const MAX_CATALOG_ENTRIES: usize = 128;
/// Maximum accepted manifest JSON length.
pub const MAX_MANIFEST_BYTES: usize = 64 * 1024;
/// Maximum accepted catalog JSON length.
pub const MAX_CATALOG_BYTES: usize = 512 * 1024;
/// Maximum application executable size.
pub const MAX_BINARY_BYTES: u64 = 64 * 1024 * 1024;
/// Maximum downloaded package size.
pub const MAX_PACKAGE_BYTES: u64 = 128 * 1024 * 1024;

/// Eight-byte prefix of every pathless application bundle.
pub const BUNDLE_MAGIC: [u8; 8] = *b"COBALTAP";
/// Binary bundle format version.
pub const BUNDLE_VERSION: u16 = 1;

/// Ed25519 public key trusted by released Cobalt runtimes and publishing CI.
pub const PUBLIC_RELEASE_KEY_HEX: &str =
    "bed7511de9fadbcf81fb4efe445b8a073c81a8333f64410c6ded588bbfd4a5de";

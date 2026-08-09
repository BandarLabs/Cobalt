use kobo_json::ParseError;
use kobo_policy::DeclarationError;
use std::fmt;

/// Why manifest or catalog JSON was refused.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FormatError {
    DocumentTooLarge {
        maximum: usize,
    },
    InvalidUtf8,
    InvalidJson(ParseError),
    ExpectedObject(&'static str),
    UnknownField {
        object: &'static str,
        field: String,
    },
    DuplicateField {
        object: &'static str,
        field: String,
    },
    MissingField {
        object: &'static str,
        field: &'static str,
    },
    InvalidType(&'static str),
    UnsupportedFormat(i64),
    InvalidValue {
        field: &'static str,
        reason: &'static str,
    },
    ReservedAppId(String),
    DuplicateAppId(String),
    Capability(DeclarationError),
}

impl fmt::Display for FormatError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DocumentTooLarge { maximum } => {
                write!(formatter, "JSON exceeds the {maximum}-byte limit")
            }
            Self::InvalidUtf8 => formatter.write_str("JSON is not UTF-8"),
            Self::InvalidJson(error) => write!(formatter, "invalid JSON: {error}"),
            Self::ExpectedObject(object) => write!(formatter, "{object} must be an object"),
            Self::UnknownField { object, field } => {
                write!(formatter, "unknown field '{field}' in {object}")
            }
            Self::DuplicateField { object, field } => {
                write!(formatter, "duplicate field '{field}' in {object}")
            }
            Self::MissingField { object, field } => {
                write!(formatter, "missing field '{field}' in {object}")
            }
            Self::InvalidType(field) => write!(formatter, "field '{field}' has the wrong type"),
            Self::UnsupportedFormat(version) => {
                write!(formatter, "unsupported format version {version}")
            }
            Self::InvalidValue { field, reason } => {
                write!(formatter, "invalid field '{field}': {reason}")
            }
            Self::ReservedAppId(id) => write!(formatter, "application id '{id}' is reserved"),
            Self::DuplicateAppId(id) => write!(formatter, "duplicate application id '{id}'"),
            Self::Capability(error) => write!(formatter, "invalid capabilities: {error}"),
        }
    }
}

impl std::error::Error for FormatError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidJson(error) => Some(error),
            Self::Capability(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ParseError> for FormatError {
    fn from(value: ParseError) -> Self {
        Self::InvalidJson(value)
    }
}

impl From<DeclarationError> for FormatError {
    fn from(value: DeclarationError) -> Self {
        Self::Capability(value)
    }
}

/// Why an Ed25519 key, signature, or verification operation failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SignatureError {
    InvalidSignatureHex,
    InvalidPublicKeyHex,
    KeyRejected,
    VerificationFailed,
}

impl fmt::Display for SignatureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidSignatureHex => {
                "signature must be exactly 128 lowercase hexadecimal characters"
            }
            Self::InvalidPublicKeyHex => {
                "public key must be exactly 64 lowercase hexadecimal characters"
            }
            Self::KeyRejected => "Ed25519 seed was rejected",
            Self::VerificationFailed => "Ed25519 signature verification failed",
        })
    }
}

impl std::error::Error for SignatureError {}

/// Why a pathless bundle was refused or could not be built.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BundleError {
    TooShort,
    InvalidMagic,
    UnsupportedVersion(u16),
    InvalidManifestLength(usize),
    Signature(SignatureError),
    Manifest(FormatError),
    NonCanonicalManifest,
    BinaryTooLarge,
    BinaryTooShort { expected: u64, actual: u64 },
    TrailingData { expected: u64, actual: u64 },
    BinaryLengthMismatch { expected: u64, actual: u64 },
    BinaryDigestMismatch,
}

impl fmt::Display for BundleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooShort => formatter.write_str("bundle is shorter than its fixed header"),
            Self::InvalidMagic => formatter.write_str("bundle magic is invalid"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported bundle version {version}")
            }
            Self::InvalidManifestLength(length) => {
                write!(formatter, "invalid bundle manifest length {length}")
            }
            Self::Signature(error) => write!(formatter, "manifest signature: {error}"),
            Self::Manifest(error) => write!(formatter, "manifest: {error}"),
            Self::NonCanonicalManifest => {
                formatter.write_str("signed manifest bytes are not canonical JSON")
            }
            Self::BinaryTooLarge => formatter.write_str("application binary exceeds its limit"),
            Self::BinaryTooShort { expected, actual } => {
                write!(
                    formatter,
                    "binary is short: expected {expected}, found {actual}"
                )
            }
            Self::TrailingData { expected, actual } => {
                write!(
                    formatter,
                    "bundle has trailing data: expected {expected}, found {actual}"
                )
            }
            Self::BinaryLengthMismatch { expected, actual } => {
                write!(
                    formatter,
                    "binary length mismatch: manifest says {expected}, found {actual}"
                )
            }
            Self::BinaryDigestMismatch => formatter.write_str("binary SHA-256 does not match"),
        }
    }
}

impl std::error::Error for BundleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Signature(error) => Some(error),
            Self::Manifest(error) => Some(error),
            _ => None,
        }
    }
}

impl From<SignatureError> for BundleError {
    fn from(value: SignatureError) -> Self {
        Self::Signature(value)
    }
}

impl From<FormatError> for BundleError {
    fn from(value: FormatError) -> Self {
        Self::Manifest(value)
    }
}

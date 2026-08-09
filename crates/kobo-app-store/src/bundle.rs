use crate::{
    sign, verify, BundleError, DetachedSignature, Ed25519PublicKey, Manifest, BUNDLE_MAGIC,
    BUNDLE_VERSION, MAX_BINARY_BYTES, MAX_MANIFEST_BYTES,
};
use kobo_net::sha256::hex_digest;

const MAGIC_END: usize = BUNDLE_MAGIC.len();
const VERSION_END: usize = MAGIC_END + 2;
const LENGTH_END: usize = VERSION_END + 4;
const SIGNATURE_END: usize = LENGTH_END + 64;

/// A verified pathless bundle.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsedBundle {
    manifest: Manifest,
    signature: DetachedSignature,
    binary: Vec<u8>,
}

impl ParsedBundle {
    #[must_use]
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    #[must_use]
    pub const fn signature(&self) -> DetachedSignature {
        self.signature
    }

    #[must_use]
    pub fn binary(&self) -> &[u8] {
        &self.binary
    }

    #[must_use]
    pub fn into_parts(self) -> (Manifest, DetachedSignature, Vec<u8>) {
        (self.manifest, self.signature, self.binary)
    }
}

/// Builds a deterministic pathless bundle from one manifest and one binary.
///
/// The manifest must already state the exact binary length and SHA-256.
///
/// # Errors
///
/// Returns an error for a binary outside its limit, metadata that does not
/// match the supplied bytes, or a signing failure.
pub fn build_bundle(
    manifest: &Manifest,
    binary: &[u8],
    seed: &[u8; 32],
) -> Result<Vec<u8>, BundleError> {
    let actual = u64::try_from(binary.len()).map_err(|_| BundleError::BinaryTooLarge)?;
    if actual > MAX_BINARY_BYTES {
        return Err(BundleError::BinaryTooLarge);
    }
    if actual != manifest.binary_bytes() {
        return Err(BundleError::BinaryLengthMismatch {
            expected: manifest.binary_bytes(),
            actual,
        });
    }
    if hex_digest(binary) != manifest.binary_sha256().as_str() {
        return Err(BundleError::BinaryDigestMismatch);
    }

    let manifest_bytes = manifest.to_canonical_bytes();
    if manifest_bytes.is_empty() || manifest_bytes.len() > MAX_MANIFEST_BYTES {
        return Err(BundleError::InvalidManifestLength(manifest_bytes.len()));
    }
    let manifest_length = u32::try_from(manifest_bytes.len())
        .map_err(|_| BundleError::InvalidManifestLength(manifest_bytes.len()))?;
    let signature = sign(&manifest_bytes, seed)?;
    let capacity = SIGNATURE_END
        .checked_add(manifest_bytes.len())
        .and_then(|length| length.checked_add(binary.len()))
        .ok_or(BundleError::BinaryTooLarge)?;
    let mut bundle = Vec::with_capacity(capacity);
    bundle.extend_from_slice(&BUNDLE_MAGIC);
    bundle.extend_from_slice(&BUNDLE_VERSION.to_be_bytes());
    bundle.extend_from_slice(&manifest_length.to_be_bytes());
    bundle.extend_from_slice(signature.as_bytes());
    bundle.extend_from_slice(&manifest_bytes);
    bundle.extend_from_slice(binary);
    Ok(bundle)
}

/// Verifies and parses a bundle with caller-supplied reserved app IDs.
///
/// Signature verification covers the exact embedded manifest bytes and occurs
/// before JSON parsing. The bytes must also equal the manifest's canonical
/// serialization.
///
/// # Errors
///
/// Returns an error for malformed headers, signatures, manifests, lengths,
/// hashes, trailing data, or binaries outside the format limits.
pub fn parse_bundle(
    bytes: &[u8],
    public_key: &Ed25519PublicKey,
    reserved_ids: &[&str],
) -> Result<ParsedBundle, BundleError> {
    if bytes.len() < SIGNATURE_END {
        return Err(BundleError::TooShort);
    }
    if bytes[..MAGIC_END] != BUNDLE_MAGIC {
        return Err(BundleError::InvalidMagic);
    }
    let version = u16::from_be_bytes([bytes[MAGIC_END], bytes[MAGIC_END + 1]]);
    if version != BUNDLE_VERSION {
        return Err(BundleError::UnsupportedVersion(version));
    }
    let manifest_length = u32::from_be_bytes([
        bytes[VERSION_END],
        bytes[VERSION_END + 1],
        bytes[VERSION_END + 2],
        bytes[VERSION_END + 3],
    ]) as usize;
    if manifest_length == 0 || manifest_length > MAX_MANIFEST_BYTES {
        return Err(BundleError::InvalidManifestLength(manifest_length));
    }
    let manifest_end = SIGNATURE_END
        .checked_add(manifest_length)
        .ok_or(BundleError::InvalidManifestLength(manifest_length))?;
    let manifest_bytes = bytes
        .get(SIGNATURE_END..manifest_end)
        .ok_or(BundleError::TooShort)?;
    let signature_bytes: [u8; 64] = bytes[LENGTH_END..SIGNATURE_END]
        .try_into()
        .map_err(|_| BundleError::TooShort)?;
    let signature = DetachedSignature::from_bytes(signature_bytes);
    verify(manifest_bytes, &signature, public_key)?;

    let manifest = Manifest::parse(manifest_bytes, reserved_ids)?;
    if manifest.to_canonical_bytes() != manifest_bytes {
        return Err(BundleError::NonCanonicalManifest);
    }
    let expected = manifest.binary_bytes();
    if expected > MAX_BINARY_BYTES {
        return Err(BundleError::BinaryTooLarge);
    }
    let binary = bytes.get(manifest_end..).ok_or(BundleError::TooShort)?;
    let actual = u64::try_from(binary.len()).map_err(|_| BundleError::BinaryTooLarge)?;
    if actual < expected {
        return Err(BundleError::BinaryTooShort { expected, actual });
    }
    if actual > expected {
        return Err(BundleError::TrailingData { expected, actual });
    }
    if hex_digest(binary) != manifest.binary_sha256().as_str() {
        return Err(BundleError::BinaryDigestMismatch);
    }
    Ok(ParsedBundle {
        manifest,
        signature,
        binary: binary.to_vec(),
    })
}

/// Verifies and parses a bundle using Cobalt's public reserved app IDs.
///
/// # Errors
///
/// Returns the same errors as [`parse_bundle`].
pub fn parse_public_bundle(
    bytes: &[u8],
    public_key: &Ed25519PublicKey,
) -> Result<ParsedBundle, BundleError> {
    let bundle = parse_bundle(bytes, public_key, crate::public_reserved_app_ids())?;
    bundle.manifest.ensure_public()?;
    Ok(bundle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{derive_public_key, FormatError, ManifestInput};

    fn manifest(binary: &[u8]) -> Manifest {
        Manifest::new(
            ManifestInput {
                id: "reader".to_owned(),
                display_name: "Reader".to_owned(),
                short_label: "Reader".to_owned(),
                summary: "Reads one thing well.".to_owned(),
                version: "1.0.0".to_owned(),
                minimum_cobalt_version: "0.1.9".to_owned(),
                glyph: "book".to_owned(),
                capabilities: Vec::new(),
                binary_sha256: hex_digest(binary),
                binary_bytes: binary.len() as u64,
            },
            &[],
        )
        .expect("manifest")
    }

    #[test]
    fn public_bundle_parser_rejects_shell_capability() {
        let seed = [7_u8; 32];
        let key = derive_public_key(&seed).expect("key");
        let binary = b"binary";
        let mut input = ManifestInput {
            id: "shell-tool".to_owned(),
            display_name: "Shell Tool".to_owned(),
            short_label: "Shell".to_owned(),
            summary: "Requests a capability public apps may not hold.".to_owned(),
            version: "1.0.0".to_owned(),
            minimum_cobalt_version: "0.2.0".to_owned(),
            glyph: "terminal".to_owned(),
            capabilities: vec!["shell".to_owned()],
            binary_sha256: hex_digest(binary),
            binary_bytes: binary.len() as u64,
        };
        let manifest = Manifest::new(input.clone(), &[]).expect("generic manifest");
        let bundle = build_bundle(&manifest, binary, &seed).expect("bundle");
        assert!(matches!(
            parse_public_bundle(&bundle, &key),
            Err(BundleError::Manifest(FormatError::InvalidValue {
                field: "capabilities",
                ..
            }))
        ));

        input.capabilities.clear();
        assert!(Manifest::new_public(input).is_ok());
    }

    fn raw_bundle(manifest: &[u8], binary: &[u8], seed: &[u8; 32]) -> Vec<u8> {
        let signature = sign(manifest, seed).expect("signature");
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&BUNDLE_MAGIC);
        bytes.extend_from_slice(&BUNDLE_VERSION.to_be_bytes());
        let manifest_length = u32::try_from(manifest.len()).expect("test manifest fits");
        bytes.extend_from_slice(&manifest_length.to_be_bytes());
        bytes.extend_from_slice(signature.as_bytes());
        bytes.extend_from_slice(manifest);
        bytes.extend_from_slice(binary);
        bytes
    }

    #[test]
    fn bundles_round_trip_and_are_deterministic() {
        let seed = [4_u8; 32];
        let key = derive_public_key(&seed).expect("key");
        let manifest = manifest(b"application");
        let first = build_bundle(&manifest, b"application", &seed).expect("bundle");
        let second = build_bundle(&manifest, b"application", &seed).expect("bundle");
        assert_eq!(first, second);
        let parsed = parse_bundle(&first, &key, &[]).expect("verified bundle");
        assert_eq!(parsed.manifest(), &manifest);
        assert_eq!(parsed.binary(), b"application");
    }

    #[test]
    fn tampered_manifests_and_wrong_keys_are_refused() {
        let seed = [4_u8; 32];
        let manifest = manifest(b"application");
        let mut bundle = build_bundle(&manifest, b"application", &seed).expect("bundle");
        bundle[SIGNATURE_END + 10] ^= 1;
        let key = derive_public_key(&seed).expect("key");
        assert!(matches!(
            parse_bundle(&bundle, &key, &[]),
            Err(BundleError::Signature(_))
        ));

        let bundle = build_bundle(&manifest, b"application", &seed).expect("bundle");
        let wrong = derive_public_key(&[5_u8; 32]).expect("key");
        assert!(matches!(
            parse_bundle(&bundle, &wrong, &[]),
            Err(BundleError::Signature(_))
        ));
    }

    #[test]
    fn truncation_and_trailing_bytes_are_refused() {
        let seed = [4_u8; 32];
        let key = derive_public_key(&seed).expect("key");
        let manifest = manifest(b"application");
        let bundle = build_bundle(&manifest, b"application", &seed).expect("bundle");
        for length in [0, MAGIC_END, VERSION_END, LENGTH_END, SIGNATURE_END - 1] {
            assert_eq!(
                parse_bundle(&bundle[..length], &key, &[]),
                Err(BundleError::TooShort)
            );
        }
        let mut short = bundle.clone();
        short.pop();
        assert!(matches!(
            parse_bundle(&short, &key, &[]),
            Err(BundleError::BinaryTooShort { .. })
        ));
        let mut trailing = bundle;
        trailing.push(0);
        assert!(matches!(
            parse_bundle(&trailing, &key, &[]),
            Err(BundleError::TrailingData { .. })
        ));
    }

    #[test]
    fn malformed_headers_and_oversized_manifest_lengths_are_refused() {
        let seed = [4_u8; 32];
        let key = derive_public_key(&seed).expect("key");
        let manifest = manifest(b"x");
        let mut bundle = build_bundle(&manifest, b"x", &seed).expect("bundle");
        bundle[0] ^= 1;
        assert_eq!(
            parse_bundle(&bundle, &key, &[]),
            Err(BundleError::InvalidMagic)
        );

        let mut bundle = build_bundle(&manifest, b"x", &seed).expect("bundle");
        bundle[MAGIC_END..VERSION_END].copy_from_slice(&2_u16.to_be_bytes());
        assert_eq!(
            parse_bundle(&bundle, &key, &[]),
            Err(BundleError::UnsupportedVersion(2))
        );

        let mut bundle = build_bundle(&manifest, b"x", &seed).expect("bundle");
        let oversized = u32::try_from(MAX_MANIFEST_BYTES + 1).expect("limit fits");
        bundle[VERSION_END..LENGTH_END].copy_from_slice(&oversized.to_be_bytes());
        assert_eq!(
            parse_bundle(&bundle, &key, &[]),
            Err(BundleError::InvalidManifestLength(MAX_MANIFEST_BYTES + 1))
        );
    }

    #[test]
    fn signed_noncanonical_and_malformed_manifests_are_refused() {
        let seed = [4_u8; 32];
        let key = derive_public_key(&seed).expect("key");
        let canonical = manifest(b"x").to_canonical_json();
        let noncanonical = canonical.replacen('{', "{ ", 1);
        let bundle = raw_bundle(noncanonical.as_bytes(), b"x", &seed);
        assert_eq!(
            parse_bundle(&bundle, &key, &[]),
            Err(BundleError::NonCanonicalManifest)
        );

        let bundle = raw_bundle(b"{}", b"x", &seed);
        assert!(matches!(
            parse_bundle(&bundle, &key, &[]),
            Err(BundleError::Manifest(FormatError::MissingField { .. }))
        ));
    }

    #[test]
    fn binary_digest_and_build_metadata_mismatches_are_refused() {
        let seed = [4_u8; 32];
        let key = derive_public_key(&seed).expect("key");
        let manifest = manifest(b"application");
        assert!(matches!(
            build_bundle(&manifest, b"short", &seed),
            Err(BundleError::BinaryLengthMismatch { .. })
        ));

        let same_length_wrong = b"applicatioN";
        assert_eq!(
            build_bundle(&manifest, same_length_wrong, &seed),
            Err(BundleError::BinaryDigestMismatch)
        );

        let mut bundle = build_bundle(&manifest, b"application", &seed).expect("bundle");
        let last = bundle.len() - 1;
        bundle[last] ^= 1;
        assert_eq!(
            parse_bundle(&bundle, &key, &[]),
            Err(BundleError::BinaryDigestMismatch)
        );
    }
}

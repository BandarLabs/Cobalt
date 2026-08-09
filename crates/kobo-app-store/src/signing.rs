use crate::SignatureError;
use ring::signature::{self, Ed25519KeyPair, KeyPair};
use std::fmt;
use std::str::FromStr;

/// A detached 64-byte Ed25519 signature.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct DetachedSignature([u8; 64]);

impl DetachedSignature {
    /// Parses exactly 128 lowercase hexadecimal characters.
    ///
    /// # Errors
    ///
    /// Returns [`SignatureError::InvalidSignatureHex`] for any other spelling.
    pub fn from_hex(value: &str) -> Result<Self, SignatureError> {
        decode_hex(value)
            .map(Self)
            .ok_or(SignatureError::InvalidSignatureHex)
    }

    /// Constructs a signature from its wire bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 64]) -> Self {
        Self(bytes)
    }

    /// The detached 64-byte wire value.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 64] {
        &self.0
    }

    /// The canonical 128-character lowercase hexadecimal spelling.
    #[must_use]
    pub fn to_hex(self) -> String {
        encode_hex(&self.0)
    }
}

impl fmt::Debug for DetachedSignature {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("DetachedSignature")
            .field(&self.to_hex())
            .finish()
    }
}

impl fmt::Display for DetachedSignature {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_hex())
    }
}

impl FromStr for DetachedSignature {
    type Err = SignatureError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::from_hex(value)
    }
}

/// A 32-byte Ed25519 public key.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct Ed25519PublicKey([u8; 32]);

impl Ed25519PublicKey {
    /// Parses exactly 64 lowercase hexadecimal characters.
    ///
    /// # Errors
    ///
    /// Returns [`SignatureError::InvalidPublicKeyHex`] for any other spelling.
    pub fn from_hex(value: &str) -> Result<Self, SignatureError> {
        decode_hex(value)
            .map(Self)
            .ok_or(SignatureError::InvalidPublicKeyHex)
    }

    /// Constructs a public key from its wire bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The 32-byte wire value.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// The canonical 64-character lowercase hexadecimal spelling.
    #[must_use]
    pub fn to_hex(self) -> String {
        encode_hex(&self.0)
    }
}

impl fmt::Debug for Ed25519PublicKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("Ed25519PublicKey")
            .field(&self.to_hex())
            .finish()
    }
}

impl fmt::Display for Ed25519PublicKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_hex())
    }
}

impl FromStr for Ed25519PublicKey {
    type Err = SignatureError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::from_hex(value)
    }
}

/// Signs exact bytes with an Ed25519 key derived from a 32-byte seed.
///
/// # Errors
///
/// Returns [`SignatureError::KeyRejected`] if ring rejects the seed.
pub fn sign(message: &[u8], seed: &[u8; 32]) -> Result<DetachedSignature, SignatureError> {
    let key = Ed25519KeyPair::from_seed_unchecked(seed).map_err(|_| SignatureError::KeyRejected)?;
    let bytes: [u8; 64] = key
        .sign(message)
        .as_ref()
        .try_into()
        .map_err(|_| SignatureError::KeyRejected)?;
    Ok(DetachedSignature::from_bytes(bytes))
}

/// Derives the Ed25519 public key belonging to a 32-byte seed.
///
/// # Errors
///
/// Returns [`SignatureError::KeyRejected`] if ring rejects the seed.
pub fn derive_public_key(seed: &[u8; 32]) -> Result<Ed25519PublicKey, SignatureError> {
    let key = Ed25519KeyPair::from_seed_unchecked(seed).map_err(|_| SignatureError::KeyRejected)?;
    let bytes: [u8; 32] = key
        .public_key()
        .as_ref()
        .try_into()
        .map_err(|_| SignatureError::KeyRejected)?;
    Ok(Ed25519PublicKey::from_bytes(bytes))
}

/// Verifies a detached signature over the exact supplied bytes.
///
/// # Errors
///
/// Returns [`SignatureError::VerificationFailed`] when the key, message, and
/// signature do not match.
pub fn verify(
    message: &[u8],
    signature: &DetachedSignature,
    public_key: &Ed25519PublicKey,
) -> Result<(), SignatureError> {
    signature::UnparsedPublicKey::new(&signature::ED25519, public_key.as_bytes())
        .verify(message, signature.as_bytes())
        .map_err(|_| SignatureError::VerificationFailed)
}

fn decode_hex<const N: usize>(value: &str) -> Option<[u8; N]> {
    if value.len() != N * 2
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }
    let mut bytes = [0_u8; N];
    for (slot, pair) in bytes.iter_mut().zip(value.as_bytes().chunks_exact(2)) {
        *slot = (hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?;
    }
    Some(bytes)
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures_are_deterministic_hex_and_verify_exact_bytes() {
        let seed = [7_u8; 32];
        let key = derive_public_key(&seed).expect("key");
        let first = sign(b"catalog", &seed).expect("signature");
        let second = sign(b"catalog", &seed).expect("signature");
        assert_eq!(first, second);
        assert_eq!(first.to_hex().len(), 128);
        assert_eq!(DetachedSignature::from_hex(&first.to_hex()), Ok(first));
        assert_eq!(Ed25519PublicKey::from_hex(&key.to_hex()), Ok(key));
        assert_eq!(verify(b"catalog", &first, &key), Ok(()));
        assert_eq!(
            verify(b"catalog!", &first, &key),
            Err(SignatureError::VerificationFailed)
        );
    }

    #[test]
    fn wrong_keys_and_malformed_hex_are_refused() {
        let signature = sign(b"manifest", &[1_u8; 32]).expect("signature");
        let wrong = derive_public_key(&[2_u8; 32]).expect("key");
        assert_eq!(
            verify(b"manifest", &signature, &wrong),
            Err(SignatureError::VerificationFailed)
        );
        for value in ["", "aa", &"A".repeat(128), &"g".repeat(128)] {
            assert_eq!(
                DetachedSignature::from_hex(value),
                Err(SignatureError::InvalidSignatureHex)
            );
        }
        assert_eq!(
            Ed25519PublicKey::from_hex(&"a".repeat(63)),
            Err(SignatureError::InvalidPublicKeyHex)
        );
    }
}

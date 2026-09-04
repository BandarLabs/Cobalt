use crate::{verify, DetachedSignature, Ed25519PublicKey, PUBLIC_RELEASE_KEY_HEX};

const MAX_RELEASE_MANIFEST_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleaseAsset {
    pub kind: String,
    pub platform: Option<String>,
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleaseManifest {
    pub version: String,
    pub channels: Vec<String>,
    pub source: String,
    pub assets: Vec<ReleaseAsset>,
}

impl ReleaseManifest {
    /// Parses the canonical host/platform release manifest.
    ///
    /// # Errors
    ///
    /// Returns a description of the first malformed or missing field.
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_RELEASE_MANIFEST_BYTES {
            return Err("release manifest is too large".to_owned());
        }
        let text =
            std::str::from_utf8(bytes).map_err(|_| "release manifest is not UTF-8".to_owned())?;
        if !text.ends_with('\n') {
            return Err("release manifest must end with a newline".to_owned());
        }
        let mut lines = text.lines();
        if lines.next() != Some("cobalt-host-release 1") {
            return Err("unsupported release manifest format".to_owned());
        }
        let version = field(lines.next(), "version")?;
        if !valid_version(&version) {
            return Err("release manifest has an invalid version".to_owned());
        }
        let channel_text = lines
            .next()
            .and_then(|line| line.strip_prefix("channels "))
            .ok_or_else(|| "release manifest has no channels".to_owned())?
            .to_owned();
        let channels = channel_text
            .split(',')
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        if channels != ["stable", "beta"] {
            return Err(
                "release manifest must be promotable unchanged from beta to stable".to_owned(),
            );
        }
        let source = field(lines.next(), "source")?;
        if source.len() != 40 || !source.bytes().all(is_lower_hex) {
            return Err("release manifest has an invalid source commit".to_owned());
        }

        let mut assets = Vec::new();
        for line in lines {
            let fields = line.split(' ').collect::<Vec<_>>();
            let (kind, platform, name, byte_text, sha256) = match fields.as_slice() {
                ["device", name, bytes, sha256] => ("device", None, *name, *bytes, *sha256),
                ["bootstrap", name, bytes, sha256] => ("bootstrap", None, *name, *bytes, *sha256),
                ["host", platform, name, bytes, sha256] => {
                    ("host", Some((*platform).to_owned()), *name, *bytes, *sha256)
                }
                _ => return Err(format!("invalid release manifest line {line:?}")),
            };
            if !safe_token(name) || sha256.len() != 64 || !sha256.bytes().all(is_lower_hex) {
                return Err(format!("invalid release asset line {line:?}"));
            }
            let bytes = byte_text
                .parse::<u64>()
                .map_err(|_| format!("invalid release asset size in {line:?}"))?;
            if bytes == 0 {
                return Err(format!("empty release asset in {line:?}"));
            }
            assets.push(ReleaseAsset {
                kind: kind.to_owned(),
                platform,
                name: name.to_owned(),
                bytes,
                sha256: sha256.to_owned(),
            });
        }
        if assets.iter().filter(|asset| asset.kind == "device").count() != 1 {
            return Err("release manifest must name exactly one device package".to_owned());
        }
        if assets
            .iter()
            .filter(|asset| asset.kind == "bootstrap" && asset.name == "install.sh")
            .count()
            != 1
        {
            return Err("release manifest must name exactly one install.sh bootstrap".to_owned());
        }
        for platform in ["macos-x86_64", "macos-arm64", "linux-x86_64", "linux-arm64"] {
            if assets
                .iter()
                .filter(|asset| asset.platform.as_deref() == Some(platform))
                .count()
                != 1
            {
                return Err(format!(
                    "release manifest must name exactly one {platform} host package"
                ));
            }
        }
        Ok(Self {
            version,
            channels,
            source,
            assets,
        })
    }

    #[must_use]
    pub fn device(&self) -> Option<&ReleaseAsset> {
        self.assets.iter().find(|asset| asset.kind == "device")
    }

    #[must_use]
    pub fn allows_channel(&self, channel: &str) -> bool {
        self.channels.iter().any(|allowed| allowed == channel)
    }
}

/// Verifies and parses a raw detached Ed25519 release-manifest signature.
///
/// # Errors
///
/// Returns an error for malformed signatures, verification failure, or an
/// invalid manifest.
pub fn verify_release_manifest(
    bytes: &[u8],
    signature_hex: &str,
) -> Result<ReleaseManifest, String> {
    let signature = DetachedSignature::from_hex(signature_hex.trim())
        .map_err(|error| format!("invalid release signature: {error}"))?;
    let public = Ed25519PublicKey::from_hex(PUBLIC_RELEASE_KEY_HEX)
        .map_err(|error| format!("invalid built-in release key: {error}"))?;
    verify(bytes, &signature, &public)
        .map_err(|_| "release manifest signature verification failed".to_owned())?;
    ReleaseManifest::parse(bytes)
}

fn field(line: Option<&str>, name: &str) -> Result<String, String> {
    let line = line.ok_or_else(|| format!("release manifest has no {name}"))?;
    let prefix = format!("{name} ");
    let value = line
        .strip_prefix(&prefix)
        .ok_or_else(|| format!("release manifest has no {name}"))?;
    if !safe_token(value) {
        return Err(format!("release manifest has an invalid {name}"));
    }
    Ok(value.to_owned())
}

fn valid_version(value: &str) -> bool {
    let parts = value.split('.').collect::<Vec<_>>();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

fn safe_token(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn is_lower_hex(byte: u8) -> bool {
    byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> Vec<u8> {
        b"cobalt-host-release 1\nversion 0.3.4\nchannels stable,beta\nsource 0123456789abcdef0123456789abcdef01234567\ndevice cobalt-0.3.4-KoboRoot.tgz 12 aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\nbootstrap install.sh 13 bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\nhost macos-x86_64 kobo-0.3.4-macos-x86_64.tar.gz 14 cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc\nhost macos-arm64 kobo-0.3.4-macos-arm64.tar.gz 15 dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd\nhost linux-x86_64 kobo-0.3.4-linux-x86_64.tar.gz 16 eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee\nhost linux-arm64 kobo-0.3.4-linux-arm64.tar.gz 17 ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff\n".to_vec()
    }

    #[test]
    fn release_manifest_binds_both_channels_and_the_device_digest() {
        let parsed = ReleaseManifest::parse(&manifest()).expect("manifest");
        assert!(parsed.allows_channel("stable"));
        assert!(parsed.allows_channel("beta"));
        let device = parsed.device().expect("device");
        assert_eq!(device.name, "cobalt-0.3.4-KoboRoot.tgz");
        assert_eq!(device.sha256, "a".repeat(64));
    }

    #[test]
    fn unsigned_or_malformed_release_metadata_is_refused() {
        assert!(verify_release_manifest(&manifest(), &"0".repeat(128)).is_err());
        let mut malformed = manifest();
        malformed.extend_from_slice(b"version 9.9.9\n");
        assert!(ReleaseManifest::parse(&malformed).is_err());
    }
}

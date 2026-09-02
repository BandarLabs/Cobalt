//! Host release manifests and OpenSSH-compatible release signatures.
//!
//! Cobalt already uses one protected Ed25519 release key for the public app
//! catalog. Host releases use the same established trust root. The raw
//! detached signature is consumed by `kobo setup`; the SSHSIG spelling lets a
//! bootstrap shell verify the same manifest with the `ssh-keygen` shipped by
//! macOS and mainstream Linux distributions before it executes the download.

use std::fmt::Write as _;

use ring::digest::{digest, SHA512};

pub const SIGNING_NAMESPACE: &str = "cobalt-host-release";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Asset {
    pub kind: String,
    pub platform: Option<String>,
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Manifest {
    pub version: String,
    pub channels: Vec<String>,
    pub source: String,
    pub assets: Vec<Asset>,
}

impl Manifest {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
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
            assets.push(Asset {
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

    pub fn device(&self) -> &Asset {
        self.assets
            .iter()
            .find(|asset| asset.kind == "device")
            .expect("parse requires one device asset")
    }

    pub fn allows_channel(&self, channel: &str) -> bool {
        self.channels.iter().any(|allowed| allowed == channel)
    }
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

pub fn verify_manifest(bytes: &[u8], signature_hex: &str) -> Result<Manifest, String> {
    let signature = kobo_app_store::DetachedSignature::from_hex(signature_hex.trim())
        .map_err(|error| format!("invalid release signature: {error}"))?;
    let public = kobo_app_store::Ed25519PublicKey::from_hex(kobo_app_store::PUBLIC_RELEASE_KEY_HEX)
        .map_err(|error| format!("invalid built-in release key: {error}"))?;
    kobo_app_store::verify(bytes, &signature, &public)
        .map_err(|_| "release manifest signature verification failed".to_owned())?;
    Manifest::parse(bytes)
}

pub fn sign(
    bytes: &[u8],
    seed: &[u8; 32],
) -> Result<(kobo_app_store::DetachedSignature, String), String> {
    let signature =
        kobo_app_store::sign(bytes, seed).map_err(|error| format!("sign manifest: {error}"))?;
    Ok((signature, ssh_signature(bytes, seed)?))
}

fn ssh_signature(bytes: &[u8], seed: &[u8; 32]) -> Result<String, String> {
    let public = kobo_app_store::derive_public_key(seed)
        .map_err(|error| format!("derive release key: {error}"))?;
    let mut public_blob = Vec::new();
    push_string(&mut public_blob, b"ssh-ed25519");
    push_string(&mut public_blob, public.as_bytes());

    let message_hash = digest(&SHA512, bytes);
    let mut signed = Vec::new();
    signed.extend_from_slice(b"SSHSIG");
    push_string(&mut signed, SIGNING_NAMESPACE.as_bytes());
    push_string(&mut signed, b"");
    push_string(&mut signed, b"sha512");
    push_string(&mut signed, message_hash.as_ref());
    let signature = kobo_app_store::sign(&signed, seed)
        .map_err(|error| format!("sign SSH manifest: {error}"))?;

    let mut signature_blob = Vec::new();
    push_string(&mut signature_blob, b"ssh-ed25519");
    push_string(&mut signature_blob, signature.as_bytes());

    let mut sshsig = Vec::new();
    sshsig.extend_from_slice(b"SSHSIG");
    sshsig.extend_from_slice(&1_u32.to_be_bytes());
    push_string(&mut sshsig, &public_blob);
    push_string(&mut sshsig, SIGNING_NAMESPACE.as_bytes());
    push_string(&mut sshsig, b"");
    push_string(&mut sshsig, b"sha512");
    push_string(&mut sshsig, &signature_blob);

    let encoded = base64(&sshsig);
    let mut armored = String::from("-----BEGIN SSH SIGNATURE-----\n");
    for line in encoded.as_bytes().chunks(70) {
        let _ = writeln!(armored, "{}", String::from_utf8_lossy(line));
    }
    armored.push_str("-----END SSH SIGNATURE-----\n");
    Ok(armored)
}

#[cfg(test)]
fn allowed_signer(public: &kobo_app_store::Ed25519PublicKey, identity: &str) -> String {
    let mut public_blob = Vec::new();
    push_string(&mut public_blob, b"ssh-ed25519");
    push_string(&mut public_blob, public.as_bytes());
    format!("{identity} ssh-ed25519 {}", base64(&public_blob))
}

fn push_string(out: &mut Vec<u8>, value: &[u8]) {
    let length = u32::try_from(value.len()).expect("SSHSIG field fits u32");
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(value);
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = u32::from(chunk[0]);
        let second = u32::from(*chunk.get(1).unwrap_or(&0));
        let third = u32::from(*chunk.get(2).unwrap_or(&0));
        let value = (first << 16) | (second << 8) | third;
        out.push(char::from(TABLE[((value >> 18) & 0x3f) as usize]));
        out.push(char::from(TABLE[((value >> 12) & 0x3f) as usize]));
        out.push(if chunk.len() > 1 {
            char::from(TABLE[((value >> 6) & 0x3f) as usize])
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            char::from(TABLE[(value & 0x3f) as usize])
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> Vec<u8> {
        b"cobalt-host-release 1\nversion 0.3.3\nchannels stable,beta\nsource 0123456789abcdef0123456789abcdef01234567\ndevice cobalt-0.3.3-KoboRoot.tgz 12 aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\nbootstrap install.sh 11 ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff\nhost macos-x86_64 kobo-0.3.3-macos-x86_64.tar.gz 13 bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\nhost macos-arm64 kobo-0.3.3-macos-arm64.tar.gz 14 cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc\nhost linux-x86_64 kobo-0.3.3-linux-x86_64.tar.gz 15 dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd\nhost linux-arm64 kobo-0.3.3-linux-arm64.tar.gz 16 eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee\n".to_vec()
    }

    #[test]
    fn strict_manifest_names_every_supported_host_and_one_device() {
        let parsed = Manifest::parse(&manifest()).expect("manifest");
        assert_eq!(parsed.version, "0.3.3");
        assert!(parsed.allows_channel("stable"));
        assert!(parsed.allows_channel("beta"));
        assert_eq!(parsed.device().bytes, 12);
        let mut missing = manifest();
        let start = String::from_utf8(missing.clone())
            .expect("text")
            .find("host linux-arm64")
            .expect("line");
        missing.truncate(start);
        assert!(Manifest::parse(&missing)
            .expect_err("missing platform")
            .contains("linux-arm64"));
    }

    #[test]
    fn raw_signature_verifies_and_detects_changes() {
        let bytes = manifest();
        let seed = [7_u8; 32];
        let public = kobo_app_store::derive_public_key(&seed).expect("key");
        let signature = kobo_app_store::sign(&bytes, &seed).expect("signature");
        kobo_app_store::verify(&bytes, &signature, &public).expect("verify");
        let mut changed = bytes;
        changed[30] ^= 1;
        assert!(kobo_app_store::verify(&changed, &signature, &public).is_err());
        assert!(verify_manifest(&manifest(), &"00".repeat(64))
            .expect_err("wrong release signature")
            .contains("verification failed"));
    }

    #[test]
    fn ssh_signature_is_armored_and_deterministic() {
        let first = ssh_signature(&manifest(), &[7_u8; 32]).expect("signature");
        let second = ssh_signature(&manifest(), &[7_u8; 32]).expect("signature");
        assert_eq!(first, second);
        assert!(first.starts_with("-----BEGIN SSH SIGNATURE-----\n"));
        assert!(first.ends_with("-----END SSH SIGNATURE-----\n"));
    }

    #[test]
    fn ssh_keygen_accepts_the_generated_signature() {
        use std::fs;
        use std::process::{Command, Stdio};

        let seed = [7_u8; 32];
        let public = kobo_app_store::derive_public_key(&seed).expect("key");
        let root = std::env::temp_dir().join(format!(
            "cobalt-sshsig-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("directory");
        let manifest_path = root.join("manifest");
        let signature_path = root.join("manifest.sshsig");
        let signers_path = root.join("allowed_signers");
        fs::write(&manifest_path, manifest()).expect("manifest");
        fs::write(
            &signature_path,
            ssh_signature(&manifest(), &seed).expect("signature"),
        )
        .expect("signature file");
        fs::write(
            &signers_path,
            format!("{}\n", allowed_signer(&public, SIGNING_IDENTITY_FOR_TEST)),
        )
        .expect("allowed signers");
        let input = fs::File::open(&manifest_path).expect("manifest input");
        let status = Command::new("ssh-keygen")
            .args([
                "-Y",
                "verify",
                "-q",
                "-f",
                signers_path.to_str().expect("path"),
                "-I",
                SIGNING_IDENTITY_FOR_TEST,
                "-n",
                SIGNING_NAMESPACE,
                "-s",
                signature_path.to_str().expect("path"),
            ])
            .stdin(Stdio::from(input))
            .status()
            .expect("ssh-keygen");
        let _ = fs::remove_dir_all(&root);
        assert!(status.success());
    }

    const SIGNING_IDENTITY_FOR_TEST: &str = "cobalt-release";
}

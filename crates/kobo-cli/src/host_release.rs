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
#[cfg(test)]
pub use kobo_app_store::ReleaseAsset as Asset;
pub use kobo_app_store::ReleaseManifest as Manifest;

pub fn verify_manifest(bytes: &[u8], signature_hex: &str) -> Result<Manifest, String> {
    kobo_app_store::verify_release_manifest(bytes, signature_hex)
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
        assert_eq!(parsed.device().expect("device").bytes, 12);
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

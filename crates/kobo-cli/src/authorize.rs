//! Giving this machine a way in, without anybody editing a file by hand.
//!
//! Every `kobo` command that names a device runs over SSH as root, using
//! `~/.ssh/kobo_cobalt`. Until now nothing created that key and nothing
//! installed it: `setup --enable-ssh` renamed the firmware's marker so the
//! server starts, and then a developer was on their own. The instruction that
//! filled the gap was "get your public key into the reader's
//! `authorized_keys`", which is the exact mess this SDK exists to avoid, and
//! it is not something a second developer can reproduce from the CLI.
//!
//! The firmware already has the mechanism. A `KoboRoot.tgz` dropped into
//! `.kobo/` is extracted as root at the next start, which is how NickelMenu
//! installs itself and how this reaches the reader's `authorized_keys`.
//!
//! This is the one thing in the CLI that writes to the root filesystem. The
//! Cobalt payload deliberately cannot: [`crate::package::check`] refuses any
//! path outside the install folder. So this module is separate, is named for
//! what it does, ships nothing but this machine's public key, and says so in
//! the report.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Where the key lives on this machine, matching `DEVICE_KEY_NAME`.
pub const KEY_NAME: &str = "kobo_cobalt";

/// Where the reader keeps the keys it could accept.
/// Both /root/ and / can be root's home directory.
const AUTHORIZED_KEYS: &[&str] = &["root/.ssh/authorized_keys", ".ssh/authorized_keys"];

/// The directories those files live in, with the mode sshd requires.
const KEY_DIRS: &[(&str, u32)] = &[("root/.ssh", 0o700), (".ssh", 0o700)];

/// Every path this module ever stages, for the undo to recognise its own work.
///
/// The directories are listed too, because an archive lists the folders it
/// creates and an undo that did not know about them would decide the archive
/// belonged to somebody else.
pub const STAGED_MEMBERS: &[&str] = &[
    "root/.ssh",
    "root/.ssh/authorized_keys",
    ".ssh",
    ".ssh/authorized_keys",
];

/// What the archive is staged as, the single slot the firmware looks at.
pub const KOBOROOT: &str = ".kobo/KoboRoot.tgz";

/// What became of the key.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Key {
    /// Generated, because this machine had none.
    Created,
    /// Already present and reused.
    Existing,
}

/// What became of the archive.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Staged {
    /// Written, and the firmware will extract it at the next start.
    Written,
    /// Added to the archive this same run had already staged, because the
    /// firmware extracts one archive and both things have to travel in it.
    Merged,
    /// Something else is already waiting in the single slot.
    SlotTaken,
}

/// The private key path on this machine.
#[must_use]
pub fn key_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".ssh").join(KEY_NAME))
}

/// Returns this machine's public key, creating the pair if it has none.
///
/// Ed25519 with no passphrase. No passphrase because every device command runs
/// unattended under `BatchMode=yes`, and a key this one cannot use without a
/// prompt is a key that turns every command into a password prompt. The blast
/// radius is a reader on a desk, which is the same thing an unlocked USB cable
/// already gives anybody in the room.
///
/// # Errors
///
/// When `HOME` is unset, `ssh-keygen` is missing or fails, or the key cannot
/// be read back.
pub fn public_key() -> Result<(String, Key), String> {
    let private = key_path().ok_or("HOME is not set, so there is nowhere to keep a key")?;
    let public = with_pub_suffix(&private);
    if public.is_file() {
        let text = fs::read_to_string(&public)
            .map_err(|error| format!("read {}: {error}", public.display()))?;
        return Ok((text.trim().to_owned(), Key::Existing));
    }
    if private.exists() {
        return Err(format!(
            "{} exists but {} does not; delete the first or restore the second",
            private.display(),
            public.display()
        ));
    }
    if let Some(folder) = private.parent() {
        fs::create_dir_all(folder)
            .map_err(|error| format!("create {}: {error}", folder.display()))?;
    }
    let status = Command::new("ssh-keygen")
        .args(["-t", "ed25519", "-N", "", "-C", "kobo-cobalt", "-f"])
        .arg(&private)
        .stdout(std::process::Stdio::null())
        .status()
        .map_err(|error| format!("run ssh-keygen: {error}"))?;
    if !status.success() {
        return Err(format!(
            "ssh-keygen refused to create {}",
            private.display()
        ));
    }
    let text = fs::read_to_string(&public)
        .map_err(|error| format!("read {}: {error}", public.display()))?;
    Ok((text.trim().to_owned(), Key::Created))
}

fn with_pub_suffix(private: &Path) -> PathBuf {
    let mut name = private.as_os_str().to_os_string();
    name.push(".pub");
    PathBuf::from(name)
}

/// The archive the firmware will extract, holding one key and nothing else.
///
/// # Errors
///
/// When the key is not a single line of OpenSSH public key.
pub fn archive(public_key: &str) -> Result<Vec<u8>, String> {
    let (folders, files) = entries(public_key)?;
    Ok(crate::package::archive(&folders, &files))
}

/// The same key, added to an archive somebody else has already built.
///
/// `base` is an unpacked tar, not a `.tgz`. The caller decompresses, because
/// the caller is the one that knows how to run `gzip`.
///
/// # Errors
///
/// When the key is not a single line of OpenSSH public key, or `base` is not
/// an archive that ends.
pub fn merge(base: &[u8], public_key: &str) -> Result<Vec<u8>, String> {
    let (folders, files) = entries(public_key)?;
    crate::package::extend(base, &folders, &files)
}

/// A directory the archive creates, and the mode it gets.
type Folder<'a> = (&'a str, u32);

/// A file the archive carries: where it goes, what is in it, and its mode.
type File = (String, Vec<u8>, u32);

/// The entries, in the one place that decides what they are: the key at each
/// place a reader's sshd might read it from, and the folders that hold them.
fn entries(public_key: &str) -> Result<(Vec<Folder<'_>>, Vec<File>), String> {
    let line = check_public_key(public_key)?;
    // A trailing newline because this file is a list, and a later key appended
    // to a file whose last line has no terminator joins onto it and breaks
    // both.
    let contents = format!("{line}\n");
    let folders = KEY_DIRS.to_vec();
    let files = AUTHORIZED_KEYS
        .iter()
        .map(|path| ((*path).to_owned(), contents.clone().into_bytes(), 0o600))
        .collect();
    Ok((folders, files))
}

/// Refuses anything that is not one OpenSSH public key.
///
/// This ends up in a file that decides who may log in as root, so a private
/// key pasted by mistake, or two keys, or a line with a newline hidden in it,
/// are all refused rather than written.
fn check_public_key(text: &str) -> Result<&str, String> {
    let line = text.trim();
    if line.is_empty() {
        return Err("the public key is empty".to_owned());
    }
    // Before the line count, because a private key is many lines and the
    // reason to refuse it is not that there are too many of them.
    if line.contains("PRIVATE KEY") {
        return Err("that is a private key, and it must never leave this machine".to_owned());
    }
    if line.lines().count() != 1 {
        return Err("a public key is one line; this is several".to_owned());
    }
    let mut fields = line.split_whitespace();
    let kind = fields.next().unwrap_or_default();
    let body = fields.next().unwrap_or_default();
    if !kind.starts_with("ssh-") && !kind.starts_with("ecdsa-") {
        return Err(format!("{kind:?} is not an OpenSSH public key type"));
    }
    if body.len() < 16 || !body.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err("the public key has no usable body".to_owned());
    }
    Ok(line)
}

/// Stages the archive on the reader's book partition.
///
/// Refuses an occupied slot. The archive already waiting there belongs to
/// something else, and overwriting it means quietly cancelling an install
/// somebody is expecting.
///
/// # Errors
///
/// When the volume cannot be written to.
pub fn stage(volume: &Path, archive: &[u8]) -> Result<Staged, String> {
    let slot = volume.join(KOBOROOT);
    if slot.exists() {
        return Ok(Staged::SlotTaken);
    }
    write_slot(volume, archive)?;
    Ok(Staged::Written)
}

/// Puts back an archive this same run staged, now carrying the key as well.
///
/// Separate from [`stage`] so that overwriting the slot is something a caller
/// has to ask for by name, and can only ask for when it knows what is in it.
///
/// # Errors
///
/// When the volume cannot be written to.
pub fn restage(volume: &Path, archive: &[u8]) -> Result<Staged, String> {
    write_slot(volume, archive)?;
    Ok(Staged::Merged)
}

fn write_slot(volume: &Path, archive: &[u8]) -> Result<(), String> {
    let slot = volume.join(KOBOROOT);
    if let Some(folder) = slot.parent() {
        fs::create_dir_all(folder)
            .map_err(|error| format!("create {}: {error}", folder.display()))?;
    }
    fs::write(&slot, archive).map_err(|error| format!("write {}: {error}", slot.display()))
}

#[cfg(test)]
mod tests {
    use super::{
        archive, check_public_key, merge, restage, stage, Staged, AUTHORIZED_KEYS, KOBOROOT,
    };
    use crate::package::list;

    const SAMPLE: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIJq0ZQ8vN0example0key0bytes kobo-cobalt";

    #[test]
    fn a_real_public_key_is_accepted() {
        assert_eq!(check_public_key(SAMPLE).unwrap(), SAMPLE);
        assert_eq!(check_public_key(&format!("  {SAMPLE}\n")).unwrap(), SAMPLE);
    }

    #[test]
    fn a_private_key_is_refused_before_it_can_be_staged() {
        // Assembled rather than written out. The pre-commit hook searches the
        // tree for the shape of a private key header and does not care that
        // this one is a test fixture, which is the right trade: a hook that
        // makes exceptions is a hook somebody talks into an exception.
        let header = format!("-----BEGIN OPENSSH {} KEY-----", "PRIVATE");
        let error = check_public_key(&format!("{header}\nabc\n")).unwrap_err();
        assert!(error.contains("never leave this machine"), "{error}");
    }

    #[test]
    fn several_keys_at_once_are_refused() {
        let error = check_public_key(&format!("{SAMPLE}\n{SAMPLE}")).unwrap_err();
        assert!(error.contains("several"), "{error}");
    }

    #[test]
    fn something_that_is_not_a_key_at_all_is_refused() {
        assert!(check_public_key("").is_err());
        assert!(check_public_key("hello there").is_err());
        assert!(check_public_key("ssh-ed25519 short").is_err());
    }

    #[test]
    fn the_archive_holds_the_key_at_each_location() {
        let built = archive(SAMPLE).expect("an archive");
        let listed = list(&built).expect("a readable archive");
        let files: Vec<&str> = listed
            .iter()
            .filter(|entry| entry.kind == b'0')
            .map(|entry| entry.path.as_str())
            .collect();
        assert_eq!(files.len(), AUTHORIZED_KEYS.len(), "{listed:?}");
        for path in AUTHORIZED_KEYS {
            assert!(files.contains(path), "{path} missing from {files:?}");
        }
    }

    #[test]
    fn every_key_file_is_readable_only_by_root() {
        let built = archive(SAMPLE).expect("an archive");
        let listed = list(&built).expect("a readable archive");
        // sshd refuses a key file that anyone else can read, so a wrong mode
        // here is a reader that silently keeps asking for a password.
        for path in AUTHORIZED_KEYS {
            let key = listed
                .iter()
                .find(|entry| entry.path == *path)
                .unwrap_or_else(|| panic!("the key at {path}"));
            assert_eq!(key.mode, 0o600, "{path} was mode {:o}", key.mode);
        }
    }

    #[test]
    fn every_directory_is_private_too() {
        let built = archive(SAMPLE).expect("an archive");
        let listed = list(&built).expect("a readable archive");
        for (dir, _) in super::KEY_DIRS {
            let folder = listed
                .iter()
                .find(|entry| entry.path.trim_end_matches('/') == *dir)
                .unwrap_or_else(|| panic!("the folder {dir}"));
            assert_eq!(folder.mode, 0o700, "{dir} was mode {:o}", folder.mode);
        }
    }

    #[test]
    fn the_archive_writes_nothing_but_the_key() {
        let built = archive(SAMPLE).expect("an archive");
        for entry in list(&built).expect("a readable archive") {
            assert!(
                entry.path.starts_with("root/.ssh") || entry.path.starts_with(".ssh"),
                "{} is outside the ssh folders",
                entry.path
            );
        }
    }

    #[test]
    fn a_merged_archive_keeps_what_was_already_in_it() {
        // The single KoboRoot slot means NickelMenu and the key travel
        // together or one of them does not travel at all.
        let base = crate::package::archive(
            &[("usr/local/nm", 0o755)],
            &[("usr/local/nm/doc".to_owned(), b"nickelmenu".to_vec(), 0o644)],
        );
        let merged = merge(&base, SAMPLE).expect("a merged archive");
        let paths: Vec<String> = list(&merged)
            .expect("a readable archive")
            .into_iter()
            .map(|entry| entry.path.trim_end_matches('/').to_owned())
            .collect();
        assert!(paths.contains(&"usr/local/nm/doc".to_owned()), "{paths:?}");
        for key in AUTHORIZED_KEYS {
            assert!(paths.contains(&(*key).to_owned()), "{paths:?}");
        }
    }

    #[test]
    fn a_merged_key_is_still_readable_only_by_root() {
        let base = crate::package::archive(&[], &[("a".to_owned(), b"b".to_vec(), 0o644)]);
        let merged = merge(&base, SAMPLE).expect("a merged archive");
        let listed = list(&merged).expect("a readable archive");
        for path in AUTHORIZED_KEYS {
            let key = listed
                .iter()
                .find(|entry| entry.path == *path)
                .unwrap_or_else(|| panic!("the key at {path}"));
            assert_eq!(key.mode, 0o600, "{path} was mode {:o}", key.mode);
        }
    }

    #[test]
    fn something_that_is_not_an_archive_is_not_merged_into() {
        // Better to report that the slot could not be reopened than to hand
        // the firmware a file it will extract as root and we cannot read.
        assert!(merge(b"not a tar at all", SAMPLE).is_err());
        assert!(merge(&[0u8; 512], SAMPLE).is_err());
    }

    #[test]
    fn a_slot_this_run_staged_is_put_back_with_the_key_in_it() {
        let volume =
            std::env::temp_dir().join(format!("kobo-authorize-merge-{}", std::process::id()));
        std::fs::create_dir_all(&volume).expect("the volume");
        std::fs::create_dir_all(volume.join(".kobo")).expect("the folder");
        std::fs::write(volume.join(KOBOROOT), b"nickelmenu").expect("the staged archive");
        assert_eq!(restage(&volume, b"both").unwrap(), Staged::Merged);
        assert_eq!(
            std::fs::read(volume.join(KOBOROOT)).expect("written"),
            b"both"
        );
        std::fs::remove_dir_all(&volume).ok();
    }

    /// Proves the merge against the real NickelMenu release.
    ///
    /// Skipped unless `KOBO_RELEASE_ARCHIVE` names a downloaded `KoboRoot.tgz`,
    /// because a test that reaches the network fails on an aeroplane for a
    /// reason that has nothing to do with the code. It exists because the
    /// fresh-reader path cannot be tried on a reader that already has the
    /// plugin, and that path is the one a non-developer takes: if the merge
    /// damaged the release, their reader would restart with no menu entry and
    /// nothing to tell them why.
    ///
    /// Run it with:
    ///
    /// ```text
    /// KOBO_RELEASE_ARCHIVE=/tmp/KoboRoot.tgz cargo test -p kobo-cli
    /// ```
    #[test]
    fn the_real_release_survives_having_the_key_merged_into_it() {
        let Some(path) = std::env::var_os("KOBO_RELEASE_ARCHIVE") else {
            return;
        };
        let compressed = std::fs::read(&path).expect("the release archive");
        let before = decompress(&compressed);
        let after = merge(&before, SAMPLE).expect("a merged archive");

        // Everything the release itself wrote must be byte for byte where it
        // was. Only the terminator was replaced, so the archive up to it is
        // the whole of the release's own content.
        let kept = content_length(&before);
        assert!(kept > 0 && kept < before.len());
        assert_eq!(
            &after[..kept],
            &before[..kept],
            "the release's own bytes were disturbed"
        );

        // Then read it the way the reader will, with tar rather than our own
        // parser, because our parser refuses the './' prefix this release uses
        // and the firmware does not.
        let merged = std::env::temp_dir().join(format!("kobo-merged-{}.tgz", std::process::id()));
        std::fs::write(&merged, recompress(&after)).expect("write the merged archive");
        let listing = std::process::Command::new("tar")
            .arg("tzvf")
            .arg(&merged)
            .output()
            .expect("tar");
        let listing = String::from_utf8_lossy(&listing.stdout).into_owned();
        let _ = std::fs::remove_file(&merged);
        for member in crate::menu::ARCHIVE_MEMBERS {
            assert!(listing.contains(member), "{member} was lost\n{listing}");
        }
        for key in AUTHORIZED_KEYS {
            assert!(
                listing.contains(key),
                "the key did not arrive at {key}\n{listing}"
            );
        }
        assert!(
            listing.contains("136056"),
            "libnm.so changed size\n{listing}"
        );
        assert!(
            listing.contains("-rwxr-xr-x"),
            "libnm.so stopped being a program\n{listing}"
        );
    }

    /// Where an archive's content ends and its terminator begins.
    fn content_length(archive: &[u8]) -> usize {
        let mut end = archive.len();
        while end >= 512 && archive[end - 512..end].iter().all(|&byte| byte == 0) {
            end -= 512;
        }
        end
    }

    fn recompress(bytes: &[u8]) -> Vec<u8> {
        pipe(bytes, &["-n", "-c"])
    }

    fn decompress(bytes: &[u8]) -> Vec<u8> {
        pipe(bytes, &["-d", "-c"])
    }

    fn pipe(bytes: &[u8], arguments: &[&str]) -> Vec<u8> {
        use std::io::Write;
        let mut child = std::process::Command::new("gzip")
            .args(arguments)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("gzip");
        let owned = bytes.to_vec();
        let mut stdin = child.stdin.take().expect("stdin");
        let writer = std::thread::spawn(move || stdin.write_all(&owned));
        let output = child.wait_with_output().expect("gzip output");
        let _ = writer.join().expect("the writer");
        output.stdout
    }

    #[test]
    fn a_slot_somebody_else_is_using_is_left_alone() {
        let volume = std::env::temp_dir().join(format!("kobo-authorize-{}", std::process::id()));
        let slot = volume.join(KOBOROOT);
        std::fs::create_dir_all(slot.parent().expect("a folder")).expect("the folder");
        std::fs::write(&slot, b"somebody else's archive").expect("the other archive");
        assert_eq!(stage(&volume, b"ours").unwrap(), Staged::SlotTaken);
        assert_eq!(
            std::fs::read(&slot).expect("still there"),
            b"somebody else's archive"
        );
        std::fs::remove_dir_all(&volume).ok();
    }

    #[test]
    fn an_empty_slot_is_written() {
        let volume =
            std::env::temp_dir().join(format!("kobo-authorize-free-{}", std::process::id()));
        std::fs::create_dir_all(&volume).expect("the volume");
        assert_eq!(stage(&volume, b"ours").unwrap(), Staged::Written);
        assert_eq!(
            std::fs::read(volume.join(KOBOROOT)).expect("written"),
            b"ours"
        );
        std::fs::remove_dir_all(&volume).ok();
    }
}

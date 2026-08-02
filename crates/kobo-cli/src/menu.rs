//! A way into Cobalt from the reader's own menus, by way of NickelMenu.
//!
//! Everything else this CLI does to a reader is undone by renaming a file, and
//! this is the one thing that is not: it hands a tarball to the firmware, and
//! the firmware extracts it as root. So the whole of this module is about
//! being able to say exactly what will be extracted before it is.
//!
//! # Why NickelMenu and not our own
//!
//! The reader's home screen is Qt, drawn by `libnickel.so.1.0.0`, a stripped
//! 24 MB C++ binary. A menu entry means running code inside that process and
//! calling its own methods to build the item, which means: a shared library
//! that Qt will load, resolution of mangled C++ symbols out of a proprietary
//! binary, and rewriting the GOT entry behind one of them under `mprotect`.
//! None of that can be Rust in any useful sense, and all of it is `unsafe`,
//! which this workspace confines to `kobo-abi`. Writing it again would be
//! writing NickelMenu again, without NickelMenu's failsafe.
//!
//! That failsafe is the reason this is safe to do at all. NickelMenu moves its
//! own library aside *before* it hooks anything and only puts it back some
//! seconds after a successful start, so a reader that crashes while hooking
//! comes up at the next boot with nothing to load. It cannot boot-loop.
//!
//! # What this module adds on top
//!
//! The firmware extracts `KoboRoot.tgz` as root without looking inside it. A
//! tarball naming `./etc/init.d/rcS` would be extracted just as willingly as
//! one naming a plugin. So this module refuses to write any archive whose
//! members are not exactly the two paths NickelMenu is supposed to ship, and
//! refuses to overwrite an archive somebody else has already staged.

use std::fs;
use std::path::Path;
use std::process::Command;

/// The release this CLI installs.
///
/// Pinned rather than tracked to `latest` because the digest below is what
/// makes the download unnecessary to trust, and a digest cannot be pinned to a
/// moving target.
pub const VERSION: &str = "v0.6.0";

/// Where that release is downloaded from.
pub const RELEASE_URL: &str =
    "https://github.com/pgaskin/NickelMenu/releases/download/v0.6.0/KoboRoot.tgz";

/// The SHA-256 of the release archive, measured from the published artifact.
pub const RELEASE_DIGEST: &str = "322ff9aa863860e8f5f7e0b55cae561c54bf95983b9bce1d19819d1225d064af";

/// Every path the archive is permitted to contain.
///
/// Checked rather than assumed. The firmware extracts as root from `/`, so a
/// member named `./etc/init.d/rcS` would replace the reader's init script and
/// the next boot would be the last one. Two files is the whole of NickelMenu:
/// the Qt plugin, and the documentation it drops beside its own config folder.
pub const ARCHIVE_MEMBERS: &[&str] = &[
    "./mnt/onboard/.adds/nm/doc",
    "./usr/local/Kobo/imageformats/libnm.so",
];

/// The firmware's install slot, relative to the mounted volume.
///
/// The reader looks for this at boot, extracts it, and deletes it. It is a
/// single slot shared by every mod, which is why staging one on top of
/// somebody else's is refused. The one exception is the key
/// [`crate::authorize`] installs in the same run: that is merged into the
/// archive rather than made to wait for a second run.
pub const KOBOROOT: &str = ".kobo/KoboRoot.tgz";

/// NickelMenu's configuration folder, relative to the mounted volume.
pub const CONFIG_FOLDER: &str = ".adds/nm";

/// The file this CLI owns inside it.
///
/// Named for Cobalt so that a config somebody wrote by hand, and any other
/// mod's, is left alone by both install and undo.
pub const CONFIG: &str = ".adds/nm/cobalt";

/// The documentation NickelMenu drops once the firmware has extracted it.
///
/// This is the only evidence available over USB that the plugin is installed:
/// the plugin itself lands on the root filesystem, which a mounted reader does
/// not expose.
pub const INSTALLED_MARKER: &str = ".adds/nm/doc";

/// NickelMenu's own uninstall flag.
///
/// Creating it asks NickelMenu to remove itself at the next boot and delete
/// the flag afterwards. Using its mechanism rather than deleting its plugin
/// means the undo works over USB, where the plugin is not reachable.
pub const UNINSTALL_FLAG: &str = ".adds/nm/uninstall";

/// The firmware's own version file, relative to the mounted volume.
///
/// Rewritten by a firmware update, which also wipes the root filesystem the
/// plugin lives on. A [`INSTALLED_MARKER`] older than this file is therefore
/// evidence of a NickelMenu the firmware has since removed.
pub const VERSION_FILE: &str = ".kobo/version";

/// The menu entry itself.
///
/// `cmd_spawn` starts `start.sh` in the background and returns, which is what
/// is wanted: `kobod` stops the reader and takes the panel, so a menu item
/// that waited for it would wait for the whole session.
///
/// Started on demand, deliberately. `kobod` has one mode and it is to replace
/// the reader, so starting it at boot would leave a device with no stock
/// reader on it, and would spend the safety net every other risky thing in
/// this project has been leaning on, which is that restarting always comes
/// back to stock.
#[must_use]
pub fn config(install_folder: &str) -> String {
    format!(
        "# Cobalt. Written by 'kobo setup'; removed by 'kobo setup --undo'.\n\
         #\n\
         # Starting Cobalt stops the reader and takes over the screen. Restart\n\
         # the device to get the reader back.\n\
         menu_item :main :Cobalt :cmd_spawn :quiet:/mnt/onboard/{install_folder}/start.sh\n"
    )
}

/// What became of the menu entry, or why it could not be made.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Menu {
    /// The plugin was staged for the next boot, and the entry written.
    Staged,
    /// The plugin was already installed, so only the entry was written.
    Added,
    /// Everything was already as wanted.
    Unchanged,
    /// The entry was written, but the plugin's marker predates the last
    /// firmware update, so the plugin itself is probably gone.
    MarkerStale,
    /// Somebody else's archive is already waiting in the slot.
    SlotTaken,
}

impl Menu {
    /// One line for the report.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Staged => format!(
                "NickelMenu {VERSION} staged in {KOBOROOT}, and a Cobalt entry written to \
                 {CONFIG}. The reader installs it at the next restart."
            ),
            Self::Added => format!("Cobalt entry written to {CONFIG}"),
            Self::Unchanged => format!("menu entry already as wanted ({CONFIG})"),
            Self::MarkerStale => format!(
                "menu entry written to {CONFIG}, but NickelMenu's own files predate the \
                 last firmware update, and an update removes the plugin. If no Cobalt \
                 entry appears after a restart, run 'kobo setup --menu' to install \
                 NickelMenu again; that keeps every menu entry already on the reader."
            ),
            Self::SlotTaken => format!(
                "menu entry written, but {KOBOROOT} already holds another mod's archive, so \
                 NickelMenu was not staged. Restart the reader to let that one install, then \
                 run this again."
            ),
        }
    }
}

/// Refuses an archive that would extract anything unexpected.
///
/// # Errors
///
/// When the archive cannot be listed, or names a member outside
/// [`ARCHIVE_MEMBERS`].
pub fn check_archive(archive: &Path) -> Result<(), String> {
    check_members(&listing(archive)?)
}

/// What `tar` says is in an archive, or why it could not say.
fn listing(archive: &Path) -> Result<String, String> {
    let output = Command::new("tar")
        .arg("tzf")
        .arg(archive)
        .output()
        .map_err(|error| format!("tar: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "{} is not a readable gzipped tar: {}",
            archive.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// True when everything in the slot is something this tool put there.
///
/// Wider than [`check_members`] on purpose. That one guards what may be
/// installed, so it stays exact. This one decides what an undo may take back,
/// and by then the archive may also carry the key `kobo setup` merged into it,
/// which is still ours to remove.
fn ours_to_unstage(archive: &Path) -> bool {
    let Ok(listing) = listing(archive) else {
        return false;
    };
    let members: Vec<&str> = listing
        .lines()
        .map(|line| line.trim().trim_end_matches('/'))
        .filter(|line| !line.is_empty())
        .collect();
    !members.is_empty()
        && members.iter().all(|member| {
            ARCHIVE_MEMBERS.contains(member) || crate::authorize::STAGED_MEMBERS.contains(member)
        })
}

/// The judgement itself, separated from running `tar` so it can be tested.
///
/// # Errors
///
/// When any member is not one of [`ARCHIVE_MEMBERS`], or one is missing.
pub fn check_members(listing: &str) -> Result<(), String> {
    let members: Vec<&str> = listing
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    for member in &members {
        if !ARCHIVE_MEMBERS.contains(member) {
            return Err(format!(
                "refusing to install this archive: it would extract {member} as root, and the \
                 only paths expected of NickelMenu are {}",
                ARCHIVE_MEMBERS.join(" and ")
            ));
        }
    }
    for expected in ARCHIVE_MEMBERS {
        if !members.contains(expected) {
            return Err(format!(
                "refusing to install this archive: it does not contain {expected}"
            ));
        }
    }
    Ok(())
}

/// True once the firmware has extracted NickelMenu.
#[must_use]
pub fn installed(volume: &Path) -> bool {
    volume.join(INSTALLED_MARKER).exists()
}

/// True when the marker predates the last firmware update.
///
/// A firmware update rewrites [`VERSION_FILE`] and replaces the root
/// filesystem, taking the plugin with it, but leaves the book partition and
/// so the marker alone. The marker is written again the next time NickelMenu
/// is extracted, so one older than the version file means the plugin was
/// probably removed. Probably: this is a heuristic over file times, which is
/// why it changes what is *said* and not what is staged.
#[must_use]
pub fn marker_stale(volume: &Path) -> bool {
    let modified = |path: &str| {
        volume
            .join(path)
            .metadata()
            .and_then(|meta| meta.modified())
    };
    match (modified(INSTALLED_MARKER), modified(VERSION_FILE)) {
        (Ok(marker), Ok(version)) => marker < version,
        _ => false,
    }
}

/// Writes the menu entry, staging the plugin too when it is not yet installed.
///
/// `archive` is the verified release, and is only consulted when the plugin is
/// missing or `force` is set. Callers that cannot obtain it may pass `None`,
/// which writes the entry alone, correct when NickelMenu is already there, and
/// reported as unchanged when it is not.
///
/// `force` stages the archive even when the marker says the plugin is
/// installed, for a reader whose plugin a firmware update has removed.
/// Re-extracting the plugin leaves every entry in the config folder alone, and
/// a slot already holding somebody else's archive is still refused.
///
/// # Errors
///
/// When the archive fails [`check_archive`], or a write to the volume fails.
pub fn install(
    volume: &Path,
    archive: Option<&Path>,
    install_folder: &str,
    force: bool,
) -> Result<Menu, String> {
    let folder = volume.join(CONFIG_FOLDER);
    fs::create_dir_all(&folder).map_err(|error| format!("create {}: {error}", folder.display()))?;

    // An undo may have left the flag behind, and NickelMenu deletes it only
    // once it has acted on it. Installing over a pending uninstall otherwise
    // means installing something that removes itself at the next boot.
    let flag = volume.join(UNINSTALL_FLAG);
    if flag.exists() {
        fs::remove_file(&flag).map_err(|error| format!("remove {}: {error}", flag.display()))?;
    }

    let wanted = config(install_folder);
    let entry = volume.join(CONFIG);
    let already = fs::read_to_string(&entry).is_ok_and(|found| found == wanted);
    if !already {
        fs::write(&entry, &wanted)
            .map_err(|error| format!("write {}: {error}", entry.display()))?;
    }

    if installed(volume) && !force {
        if marker_stale(volume) {
            return Ok(Menu::MarkerStale);
        }
        return Ok(if already {
            Menu::Unchanged
        } else {
            Menu::Added
        });
    }

    let Some(archive) = archive else {
        return Ok(Menu::Unchanged);
    };
    check_archive(archive)?;

    let slot = volume.join(KOBOROOT);
    if slot.exists() {
        return Ok(Menu::SlotTaken);
    }
    fs::copy(archive, &slot).map_err(|error| format!("write {}: {error}", slot.display()))?;
    Ok(Menu::Staged)
}

/// What became of the plugin when the Cobalt entry was taken away.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Plugin {
    /// It was never extracted, so there is nothing to uninstall.
    #[default]
    Absent,
    /// It was asked to uninstall itself at the next boot.
    Flagged,
    /// It was left alone, because somebody else's menu items need it.
    Shared,
}

/// What an undo did.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Removed {
    /// The Cobalt entry was deleted.
    pub entry: bool,
    /// An archive that the reader had not extracted yet was taken back.
    pub unstaged: bool,
    /// What became of the plugin itself.
    pub plugin: Plugin,
}

/// Takes the entry away, and NickelMenu with it when nothing else needs it.
///
/// The plugin is left in place when any other configuration file remains,
/// because it is shared: removing it would take away somebody else's menu
/// items too. When the folder holds nothing but NickelMenu's own documentation
/// the plugin has no reason to be there, and its uninstall flag is set.
///
/// # Errors
///
/// When a write to the volume fails.
pub fn remove(volume: &Path) -> Result<Removed, String> {
    let mut removed = Removed::default();
    let entry = volume.join(CONFIG);
    if entry.exists() {
        fs::remove_file(&entry).map_err(|error| format!("remove {}: {error}", entry.display()))?;
        removed.entry = true;
    }

    // Only ours. An archive that holds anything this tool does not stage is
    // somebody else's, and taking it away would be undoing something this
    // command never did.
    let slot = volume.join(KOBOROOT);
    if slot.exists() && ours_to_unstage(&slot) {
        fs::remove_file(&slot).map_err(|error| format!("remove {}: {error}", slot.display()))?;
        removed.unstaged = true;
    }

    if !installed(volume) {
        return Ok(removed);
    }
    if !only_ours(volume) {
        removed.plugin = Plugin::Shared;
        return Ok(removed);
    }
    let flag = volume.join(UNINSTALL_FLAG);
    fs::write(&flag, "").map_err(|error| format!("write {}: {error}", flag.display()))?;
    removed.plugin = Plugin::Flagged;
    Ok(removed)
}

/// True when nothing in NickelMenu's folder is anybody else's configuration.
///
/// `doc` is NickelMenu's own, and `uninstall` is its flag; neither counts. Any
/// other file is somebody's menu items, and they get to keep the plugin.
fn only_ours(volume: &Path) -> bool {
    let Ok(entries) = fs::read_dir(volume.join(CONFIG_FOLDER)) else {
        return true;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == "doc" || name == "uninstall" || name.starts_with('.') {
            continue;
        }
        return false;
    }
    true
}

/// Downloads the pinned release and checks its digest.
///
/// The transport does not have to be trusted, because the digest is what is
/// checked; a proxy or a mirror that returns something else fails here rather
/// than on the reader.
///
/// # Errors
///
/// When the download fails, or the digest is not [`RELEASE_DIGEST`].
pub fn download(into: &Path) -> Result<(), String> {
    let output = Command::new("curl")
        .args([
            "--silent",
            "--show-error",
            "--location",
            "--fail",
            "--output",
        ])
        .arg(into)
        .arg(RELEASE_URL)
        .output()
        .map_err(|error| format!("curl: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "download NickelMenu {VERSION}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let bytes = fs::read(into).map_err(|error| format!("read {}: {error}", into.display()))?;
    let digest = crate::sha256::hex_digest(&bytes);
    if digest != RELEASE_DIGEST {
        let _ = fs::remove_file(into);
        return Err(format!(
            "NickelMenu {VERSION} downloaded as {digest}, expected {RELEASE_DIGEST}"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing(members: &[&str]) -> String {
        let mut text = members.join("\n");
        text.push('\n');
        text
    }

    #[test]
    fn the_published_archive_is_accepted() {
        check_members(&listing(ARCHIVE_MEMBERS)).expect("the real listing passes");
    }

    #[test]
    fn an_archive_that_would_replace_the_init_script_is_refused() {
        // This is the failure the whole check exists for. The firmware
        // extracts as root from /, so this member is the reader's init script
        // and a bad one is a device that does not boot.
        let mut members = ARCHIVE_MEMBERS.to_vec();
        members.push("./etc/init.d/rcS");
        let error = check_members(&listing(&members)).expect_err("refused");
        assert!(error.contains("./etc/init.d/rcS"), "{error}");
        assert!(error.contains("as root"), "{error}");
    }

    #[test]
    fn an_archive_missing_the_plugin_is_refused() {
        let error = check_members(&listing(&["./mnt/onboard/.adds/nm/doc"]))
            .expect_err("an archive without the plugin is refused");
        assert!(error.contains("libnm.so"), "{error}");
    }

    #[test]
    fn blank_lines_in_the_listing_are_ignored() {
        check_members(&format!("\n{}\n\n", listing(ARCHIVE_MEMBERS))).expect("blank lines pass");
    }

    #[test]
    fn the_entry_starts_cobalt_in_the_background_and_says_so() {
        let text = config(".adds/cobalt");
        assert!(
            text.contains("menu_item :main :Cobalt :cmd_spawn :quiet:"),
            "{text}"
        );
        assert!(
            text.contains("/mnt/onboard/.adds/cobalt/start.sh"),
            "{text}"
        );
        // cmd_output would block the reader's UI thread for as long as Cobalt
        // ran, which is the whole session.
        assert!(!text.contains("cmd_output"), "{text}");
    }

    #[test]
    fn an_undo_takes_back_the_archive_the_key_was_merged_into() {
        // A merged archive fails check_members on purpose, because it holds
        // one path NickelMenu's release never does. If the undo used that
        // check it would decide its own archive was somebody else's and leave
        // it in the slot for the reader to install after the undo.
        let volume = volume();
        let merged = crate::package::archive(
            &[("root/.ssh", 0o700)],
            &[
                (ARCHIVE_MEMBERS[0].to_owned(), b"doc".to_vec(), 0o644),
                (ARCHIVE_MEMBERS[1].to_owned(), b"plugin".to_vec(), 0o755),
                (
                    "root/.ssh/authorized_keys".to_owned(),
                    b"ssh-ed25519 AAAA\n".to_vec(),
                    0o600,
                ),
            ],
        );
        let slot = volume.join(KOBOROOT);
        fs::write(&slot, gzipped(&merged)).expect("stage the merged archive");
        assert!(check_archive(&slot).is_err(), "it is not a plain release");
        let removed = remove(&volume).expect("undo");
        assert!(removed.unstaged, "the merged archive was left behind");
        assert!(!slot.exists());
        let _ = fs::remove_dir_all(&volume);
    }

    #[test]
    fn an_undo_leaves_somebody_elses_archive_where_it_is() {
        let volume = volume();
        let theirs = crate::package::archive(
            &[],
            &[("usr/local/other/thing".to_owned(), b"x".to_vec(), 0o644)],
        );
        let slot = volume.join(KOBOROOT);
        fs::write(&slot, gzipped(&theirs)).expect("stage");
        let removed = remove(&volume).expect("undo");
        assert!(!removed.unstaged, "somebody else's archive was taken");
        assert!(slot.exists());
        let _ = fs::remove_dir_all(&volume);
    }

    fn gzipped(bytes: &[u8]) -> Vec<u8> {
        use std::io::Write;
        let mut child = Command::new("gzip")
            .args(["-n", "-c"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("gzip");
        let owned = bytes.to_vec();
        let mut stdin = child.stdin.take().expect("stdin");
        let writer = std::thread::spawn(move || stdin.write_all(&owned));
        let output = child.wait_with_output().expect("gzip output");
        writer.join().expect("writer").expect("write");
        output.stdout
    }

    fn volume() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "kobo-menu-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(path.join(".kobo")).expect("make a volume");
        path
    }

    #[test]
    fn installing_without_the_plugin_present_stages_the_archive() {
        let volume = volume();
        let archive = volume.join("release.tgz");
        stub_archive(&archive);

        let menu = install(&volume, Some(&archive), ".adds/cobalt", false).expect("install");
        assert_eq!(menu, Menu::Staged);
        assert!(volume.join(KOBOROOT).exists());
        assert!(volume.join(CONFIG).exists());
        let _ = fs::remove_dir_all(&volume);
    }

    #[test]
    fn installing_twice_changes_nothing_the_second_time() {
        let volume = volume();
        fs::create_dir_all(volume.join(CONFIG_FOLDER)).expect("make the folder");
        fs::write(volume.join(INSTALLED_MARKER), "docs").expect("pretend it is installed");

        assert_eq!(
            install(&volume, None, ".adds/cobalt", false).expect("first"),
            Menu::Added
        );
        assert_eq!(
            install(&volume, None, ".adds/cobalt", false).expect("second"),
            Menu::Unchanged
        );
        let _ = fs::remove_dir_all(&volume);
    }

    #[test]
    fn a_marker_older_than_the_firmware_update_is_reported_not_trusted() {
        let volume = volume();
        fs::create_dir_all(volume.join(CONFIG_FOLDER)).expect("make the folder");
        fs::write(volume.join(INSTALLED_MARKER), "docs").expect("old marker");
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(volume.join(VERSION_FILE), "N365,4.9,4.45").expect("newer firmware");

        assert!(marker_stale(&volume));
        assert_eq!(
            install(&volume, None, ".adds/cobalt", false).expect("install"),
            Menu::MarkerStale,
            "a probably-wiped plugin is said out loud, and nothing is staged"
        );
        assert!(
            !volume.join(KOBOROOT).exists(),
            "saying it must not become staging it"
        );
        let _ = fs::remove_dir_all(&volume);
    }

    #[test]
    fn forcing_stages_the_plugin_over_an_installed_marker() {
        let volume = volume();
        fs::create_dir_all(volume.join(CONFIG_FOLDER)).expect("make the folder");
        fs::write(volume.join(INSTALLED_MARKER), "docs").expect("marker");
        let archive = volume.join("release.tgz");
        stub_archive(&archive);

        assert_eq!(
            install(&volume, Some(&archive), ".adds/cobalt", true).expect("install"),
            Menu::Staged
        );
        assert!(volume.join(KOBOROOT).exists());
        let _ = fs::remove_dir_all(&volume);
    }

    #[test]
    fn a_marker_newer_than_the_firmware_update_is_trusted() {
        let volume = volume();
        fs::create_dir_all(volume.join(CONFIG_FOLDER)).expect("make the folder");
        fs::write(volume.join(VERSION_FILE), "N365,4.9,4.45").expect("firmware");
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(volume.join(INSTALLED_MARKER), "docs").expect("fresh marker");

        assert!(!marker_stale(&volume));
        assert_eq!(
            install(&volume, None, ".adds/cobalt", false).expect("install"),
            Menu::Added
        );
        let _ = fs::remove_dir_all(&volume);
    }

    #[test]
    fn a_pending_uninstall_is_cleared_by_installing_again() {
        // Undo then setup, in that order, on the same volume. Without this the
        // reader would extract the plugin at one boot and uninstall it at the
        // same one.
        let volume = volume();
        fs::create_dir_all(volume.join(CONFIG_FOLDER)).expect("make the folder");
        fs::write(volume.join(INSTALLED_MARKER), "docs").expect("installed");
        fs::write(volume.join(UNINSTALL_FLAG), "").expect("pending uninstall");

        install(&volume, None, ".adds/cobalt", false).expect("install");
        assert!(!volume.join(UNINSTALL_FLAG).exists());
        let _ = fs::remove_dir_all(&volume);
    }

    #[test]
    fn somebody_elses_pending_archive_is_not_overwritten() {
        let volume = volume();
        fs::write(volume.join(KOBOROOT), b"somebody else's").expect("their archive");
        let archive = volume.join("release.tgz");
        stub_archive(&archive);

        assert_eq!(
            install(&volume, Some(&archive), ".adds/cobalt", false).expect("install"),
            Menu::SlotTaken
        );
        assert_eq!(
            fs::read(volume.join(KOBOROOT)).expect("still theirs"),
            b"somebody else's"
        );
        let _ = fs::remove_dir_all(&volume);
    }

    #[test]
    fn undoing_asks_nickelmenu_to_uninstall_itself_when_nothing_else_wants_it() {
        let volume = volume();
        fs::create_dir_all(volume.join(CONFIG_FOLDER)).expect("make the folder");
        fs::write(volume.join(INSTALLED_MARKER), "docs").expect("installed");
        install(&volume, None, ".adds/cobalt", false).expect("install");

        let removed = remove(&volume).expect("undo");
        assert!(removed.entry);
        assert_eq!(removed.plugin, Plugin::Flagged);
        assert!(!volume.join(CONFIG).exists());
        assert!(volume.join(UNINSTALL_FLAG).exists());
        let _ = fs::remove_dir_all(&volume);
    }

    #[test]
    fn undoing_leaves_the_plugin_alone_when_another_mod_uses_it() {
        let volume = volume();
        fs::create_dir_all(volume.join(CONFIG_FOLDER)).expect("make the folder");
        fs::write(volume.join(INSTALLED_MARKER), "docs").expect("installed");
        fs::write(
            volume.join(".adds/nm/theirs"),
            "menu_item :main :Theirs :dbg_toast :hi",
        )
        .expect("their config");
        install(&volume, None, ".adds/cobalt", false).expect("install");

        let removed = remove(&volume).expect("undo");
        assert!(removed.entry);
        assert_eq!(
            removed.plugin,
            Plugin::Shared,
            "their menu items must keep working"
        );
        assert!(!volume.join(UNINSTALL_FLAG).exists());
        let _ = fs::remove_dir_all(&volume);
    }

    #[test]
    fn undoing_takes_back_an_archive_that_has_not_been_extracted_yet() {
        let volume = volume();
        let archive = volume.join("release.tgz");
        stub_archive(&archive);
        install(&volume, Some(&archive), ".adds/cobalt", false).expect("install");
        assert!(volume.join(KOBOROOT).exists());

        let removed = remove(&volume).expect("undo");
        assert!(removed.unstaged);
        assert_eq!(removed.plugin, Plugin::Absent);
        assert!(
            !volume.join(KOBOROOT).exists(),
            "a setup that is undone before the reader restarts installs nothing"
        );
        let _ = fs::remove_dir_all(&volume);
    }

    #[test]
    fn undoing_leaves_somebody_elses_staged_archive_alone() {
        let volume = volume();
        fs::write(volume.join(KOBOROOT), b"somebody else's").expect("their archive");

        let removed = remove(&volume).expect("undo");
        assert!(!removed.unstaged);
        assert!(volume.join(KOBOROOT).exists(), "not ours to delete");
        let _ = fs::remove_dir_all(&volume);
    }

    #[test]
    fn an_archive_that_would_replace_the_init_script_is_refused_as_a_real_file() {
        // The string-level test above proves the judgement; this proves it is
        // actually reached, through tar, on a file the firmware would have
        // extracted as root.
        let volume = volume();
        let archive = volume.join("tampered.tgz");
        let staging = volume.join("staging");
        for member in ["./etc/init.d/rcS", "./usr/local/Kobo/imageformats/libnm.so"] {
            let path = staging.join(member.trim_start_matches("./"));
            fs::create_dir_all(path.parent().expect("a parent")).expect("make the tree");
            fs::write(&path, b"").expect("write a member");
        }
        assert!(Command::new("tar")
            .arg("czf")
            .arg(&archive)
            .arg("-C")
            .arg(&staging)
            .args(["./etc/init.d/rcS", "./usr/local/Kobo/imageformats/libnm.so"])
            .status()
            .expect("run tar")
            .success());

        let error = check_archive(&archive).expect_err("refused");
        assert!(error.contains("./etc/init.d/rcS"), "{error}");

        // And the refusal must stop the install, not merely be noticed.
        let refused = install(&volume, Some(&archive), ".adds/cobalt", false).expect_err("refused");
        assert!(refused.contains("as root"), "{refused}");
        assert!(!volume.join(KOBOROOT).exists(), "nothing was staged");
        let _ = fs::remove_dir_all(&volume);
    }

    /// A real gzipped tar with the two expected members and nothing in them.
    fn stub_archive(at: &Path) {
        let staging = at.with_extension("staging");
        let _ = fs::remove_dir_all(&staging);
        for member in ARCHIVE_MEMBERS {
            let path = staging.join(member.trim_start_matches("./"));
            fs::create_dir_all(path.parent().expect("a parent")).expect("make the tree");
            fs::write(&path, b"").expect("write a member");
        }
        let status = Command::new("tar")
            .arg("czf")
            .arg(at)
            .arg("-C")
            .arg(&staging)
            .args(ARCHIVE_MEMBERS)
            .status()
            .expect("run tar");
        assert!(status.success(), "tar failed");
        let _ = fs::remove_dir_all(&staging);
    }
}

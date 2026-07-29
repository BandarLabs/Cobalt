//! Preparing a stock reader over USB, before there is any way in.
//!
//! Every other command in this tool needs a network address and an SSH server.
//! A reader out of its box has neither, and no way to get either: `start.sh`
//! needs a shell to run it, and a `NickelMenu` entry needs `NickelMenu`. So the
//! first install has to happen over the USB cable, against a filesystem that
//! the reader itself is not running from.
//!
//! # What this is allowed to touch
//!
//! Only the book partition, the FAT volume that appears when the cable is
//! plugged in. Nothing here writes to the system partition, and nothing here
//! is extracted as root.
//!
//! That rule is worth stating plainly because the obvious way to do this
//! violates it. Dropping a `KoboRoot.tgz` into `.kobo/` makes the firmware
//! unpack it **as root, at `/`, at the next boot**, which is how every other
//! Kobo modification is distributed. It is also the one mechanism on the
//! device that can leave it unbootable: a bad path in that archive overwrites
//! part of the running system, and there is no recovery short of a firmware
//! reflash. Cobalt's archive is confined to `.adds/cobalt` and would be
//! harmless, but the mechanism does not check that, the archive does.
//!
//! So this does not use it. [`write_payload`] copies the same files straight
//! into `.adds/cobalt` on the mounted volume, which is a plain folder copy the
//! reader never elevates. The worst outcome of a setup that goes wrong is a
//! folder to delete.
//!
//! # What that costs
//!
//! A folder copy does not trigger the firmware's update-and-restart, so the
//! reader has to be restarted by hand for [`enable_ssh`] to take effect. That
//! is one button held down, in exchange for never handing the boot script an
//! archive. It is the right trade.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// The folder a reader's own files live in, relative to the mounted volume.
pub const SYSTEM_FOLDER: &str = ".kobo";

/// Where Cobalt is installed, relative to the mounted volume.
pub const INSTALL_FOLDER: &str = ".adds/cobalt";

/// The firmware's own marker for a disabled SSH server.
///
/// Firmware 4.42 and later ship a server and gate it on the name of this file,
/// which is the whole reason this command can exist without installing one.
pub const SSH_DISABLED: &str = ".kobo/ssh-disabled";

/// The same marker, renamed to let the server start.
pub const SSH_ENABLED: &str = ".kobo/ssh-enabled";

/// The reader's own settings file, in Qt's INI dialect.
pub const SETTINGS: &str = ".kobo/Kobo/Kobo eReader.conf";

/// The settings this command writes, as section, key and value.
///
/// Both are the reader's own settings, applied by the reader's own code. That
/// is the whole rule for this list: nothing here may make Cobalt a second
/// owner of the radio or of power, which is the mistake that cost this project
/// a device once already.
///
/// `ForceWifiOn` keeps the radio up once the reader is awake. On its own that
/// is not enough, because the reader does not merely let the radio idle, it
/// suspends the whole device, and a suspended device answers nothing. The
/// suspend is requested by nickel itself, so no wake lock can prevent it; the
/// only lever is nickel's own timer. `AutoSleepMinutes` is that timer, and
/// ninety minutes is long enough to install, deploy and test without the
/// device going out from under you mid-session.
///
/// Neither key is guessed. `AutoSleepMinutes` was found by enumerating the key
/// strings beside the `PowerSettings` type information in `libnickel`, then
/// confirmed on hardware: the reader reported it as supported, and a device
/// that had been suspending for ninety-three per cent of its life stayed awake
/// for thirty-eight unattended minutes afterwards. `kobo setup --undo` removes
/// both, and the reader's own Energy saving screen overrides them at any time.
pub const SETTINGS_APPLIED: &[(&str, &str, &str)] = &[
    ("DeveloperSettings", "ForceWifiOn", "true"),
    ("PowerOptions", "AutoSleepMinutes", "90"),
];

/// A mounted reader, and what it says it is.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Mounted {
    /// The mount point of the book partition.
    pub volume: PathBuf,
    /// Full serial, whose first four characters are the model code.
    pub serial: String,
    /// Firmware version string.
    pub firmware: String,
}

impl Mounted {
    /// The four-character model code, which is what a device profile matches.
    #[must_use]
    pub fn model_code(&self) -> &str {
        self.serial.get(..4).unwrap_or_default()
    }

    /// A one-line description of what was found.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "{} at {} · serial {} · firmware {}",
            self.model_code(),
            self.volume.display(),
            self.serial,
            self.firmware
        )
    }
}

/// Reads the four comma-separated facts the firmware records about itself.
///
/// The file is a single line of the form `serial,…,firmware,…`. Only the first
/// and third fields mean anything here, and a short line yields empty strings
/// rather than an error, because a reader that has been reset mid-write is
/// still a reader worth naming.
#[must_use]
pub fn parse_version(line: &str) -> (String, String) {
    let mut fields = line.trim().split(',');
    let serial = fields.next().unwrap_or_default().trim().to_owned();
    let firmware = fields.nth(1).unwrap_or_default().trim().to_owned();
    (serial, firmware)
}

/// True when a serial is recognisably a Kobo's.
///
/// Every Kobo serial begins with `N` and three digits. This is the same test
/// [`crate::connect::Identity::is_kobo`] applies over the network, kept
/// separate because the evidence arrives by a different route.
#[must_use]
pub fn is_kobo_serial(serial: &str) -> bool {
    let bytes = serial.as_bytes();
    bytes.len() >= 4 && bytes[0] == b'N' && bytes[1..4].iter().all(u8::is_ascii_digit)
}

/// Every place a removable volume is mounted on this operating system.
///
/// Not every entry is a reader, and most of the time none of them are. The
/// filtering is [`mounted_readers`]'s job.
#[must_use]
pub fn mount_roots() -> Vec<PathBuf> {
    if cfg!(target_os = "macos") {
        vec![PathBuf::from("/Volumes")]
    } else {
        let mut roots = vec![PathBuf::from("/media"), PathBuf::from("/run/media")];
        if let Ok(user) = std::env::var("USER") {
            roots.push(Path::new("/media").join(&user));
            roots.push(Path::new("/run/media").join(&user));
        }
        roots.push(PathBuf::from("/mnt"));
        roots
    }
}

/// Every mounted reader this machine can see.
///
/// A volume qualifies when it has a readable `.kobo/version` naming a Kobo
/// serial. That file is the firmware's, not ours, so this recognises a reader
/// that has never had Cobalt on it, which is the only kind this command is
/// for.
#[must_use]
pub fn mounted_readers() -> Vec<Mounted> {
    let mut found = Vec::new();
    for root in mount_roots() {
        let Ok(entries) = fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            if let Some(reader) = read_reader(&entry.path()) {
                found.push(reader);
            }
        }
    }
    found.sort_by(|left, right| left.volume.cmp(&right.volume));
    found.dedup_by(|left, right| left.volume == right.volume);
    found
}

/// Identifies one volume, if it is a reader at all.
#[must_use]
pub fn read_reader(volume: &Path) -> Option<Mounted> {
    let line = fs::read_to_string(volume.join(SYSTEM_FOLDER).join("version")).ok()?;
    let (serial, firmware) = parse_version(&line);
    is_kobo_serial(&serial).then(|| Mounted {
        volume: volume.to_owned(),
        serial,
        firmware,
    })
}

/// What enabling the firmware's SSH server did, or why it could not be done.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ssh {
    /// The marker was renamed. The server starts at the next boot.
    Enabled,
    /// It was already enabled, by this command or by hand.
    AlreadyEnabled,
    /// Neither marker exists, so this firmware has no server to enable.
    Unsupported,
}

impl Ssh {
    /// What to tell the owner about this outcome.
    #[must_use]
    pub fn describe(self) -> &'static str {
        match self {
            Self::Enabled => "SSH enabled (starts at the next restart)",
            Self::AlreadyEnabled => "SSH was already enabled",
            Self::Unsupported => {
                "SSH not available: this firmware has no ssh-disabled marker, so it \
                 predates the built-in server. Update the reader from its own \
                 settings and run this again."
            }
        }
    }
}

/// Renames the firmware's marker so its SSH server starts at the next boot.
///
/// This is the firmware's documented mechanism, described by the marker file
/// itself, and it is undone by renaming the file back. Nothing is installed.
///
/// # Errors
///
/// When the rename fails, which on a FAT volume means the cable was pulled or
/// the volume is mounted read-only.
pub fn enable_ssh(volume: &Path) -> Result<Ssh, String> {
    let disabled = volume.join(SSH_DISABLED);
    let enabled = volume.join(SSH_ENABLED);
    if enabled.exists() {
        return Ok(Ssh::AlreadyEnabled);
    }
    if !disabled.exists() {
        return Ok(Ssh::Unsupported);
    }
    fs::rename(&disabled, &enabled)
        .map_err(|error| format!("rename {}: {error}", disabled.display()))?;
    Ok(Ssh::Enabled)
}

/// Puts the SSH marker back, leaving the reader as it shipped.
///
/// # Errors
///
/// When the rename fails.
pub fn disable_ssh(volume: &Path) -> Result<bool, String> {
    let disabled = volume.join(SSH_DISABLED);
    let enabled = volume.join(SSH_ENABLED);
    if !enabled.exists() {
        return Ok(false);
    }
    fs::rename(&enabled, &disabled)
        .map_err(|error| format!("rename {}: {error}", enabled.display()))?;
    Ok(true)
}

/// Sets one key in one section of a Qt INI file, preserving everything else.
///
/// The reader's settings file holds several hundred keys it wrote itself, and
/// a setup command that reformats them is a setup command that loses one. So
/// this is a line editor, not a parser: every line it does not recognise comes
/// out exactly as it went in, in the same order.
#[must_use]
pub fn set_setting(text: &str, section: &str, key: &str, value: &str) -> String {
    let header = format!("[{section}]");
    let assignment = format!("{key}={value}");
    let mut out: Vec<String> = Vec::new();
    let mut inside = false;
    let mut written = false;

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if inside && !written {
                push_into_section(&mut out, assignment.clone());
                written = true;
            }
            inside = trimmed == header;
            out.push(line.to_owned());
            continue;
        }
        if inside && !written && names_key(line, key) {
            out.push(assignment.clone());
            written = true;
            continue;
        }
        out.push(line.to_owned());
    }

    if !written {
        if inside {
            push_into_section(&mut out, assignment);
        } else {
            if !out.last().is_none_or(|last| last.trim().is_empty()) {
                out.push(String::new());
            }
            out.push(header);
            out.push(assignment);
        }
    }

    let mut joined = out.join("\n");
    joined.push('\n');
    joined
}

/// Removes one key from one section, leaving everything else alone.
#[must_use]
pub fn clear_setting(text: &str, section: &str, key: &str) -> String {
    let header = format!("[{section}]");
    let mut out: Vec<String> = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            inside = trimmed == header;
        } else if inside && names_key(line, key) {
            continue;
        }
        out.push(line.to_owned());
    }
    let mut joined = out.join("\n");
    joined.push('\n');
    joined
}

/// True when a line assigns the named key.
fn names_key(line: &str, key: &str) -> bool {
    line.split_once('=')
        .is_some_and(|(name, _)| name.trim() == key)
}

/// Appends to a section that has just ended, above any blank lines closing it.
fn push_into_section(out: &mut Vec<String>, assignment: String) {
    let mut blanks = 0;
    while out
        .last()
        .is_some_and(|last| last.trim().is_empty() && blanks < out.len())
    {
        out.pop();
        blanks += 1;
    }
    out.push(assignment);
    for _ in 0..blanks {
        out.push(String::new());
    }
}

/// Applies [`SETTINGS_APPLIED`] to the reader's settings file.
///
/// Returns the keys that were changed. A file that already holds every value
/// is left untouched, so a second run reports nothing rather than rewriting.
///
/// # Errors
///
/// When the settings file cannot be read or written.
pub fn apply_settings(volume: &Path) -> Result<Vec<String>, String> {
    edit_settings(volume, SETTINGS_APPLIED.iter().copied(), true)
}

/// Removes [`SETTINGS_APPLIED`] again.
///
/// # Errors
///
/// When the settings file cannot be read or written.
pub fn revert_settings(volume: &Path) -> Result<Vec<String>, String> {
    edit_settings(volume, SETTINGS_APPLIED.iter().copied(), false)
}

fn edit_settings<'a>(
    volume: &Path,
    settings: impl Iterator<Item = (&'a str, &'a str, &'a str)>,
    set: bool,
) -> Result<Vec<String>, String> {
    let path = volume.join(SETTINGS);
    let original = match fs::read_to_string(&path) {
        Ok(text) => text,
        // A reader that has never finished its own setup has no settings file.
        // Creating one is fine; nickel merges what it finds.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(format!("read {}: {error}", path.display())),
    };

    let mut text = original.clone();
    let mut changed = Vec::new();
    for (section, key, value) in settings {
        let edited = if set {
            set_setting(&text, section, key, value)
        } else {
            clear_setting(&text, section, key)
        };
        if edited != text {
            changed.push(format!("{section}/{key}"));
            text = edited;
        }
    }

    if text != original {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
        }
        fs::write(&path, &text).map_err(|error| format!("write {}: {error}", path.display()))?;
    }
    Ok(changed)
}

/// Copies Cobalt into `.adds/cobalt` on a mounted reader.
///
/// This is a plain folder copy onto the book partition. The member list is
/// checked before anything is written (the same check the archive builder
/// applies) so a member naming a path outside the install root writes nothing
/// at all rather than writing what it can and then failing.
///
/// # Errors
///
/// When a member lies outside the install root, or the volume cannot be
/// written to.
pub fn write_payload(members: &[crate::package::Member], volume: &Path) -> Result<usize, String> {
    let destination = volume.join(INSTALL_FOLDER);
    crate::package::write_folder(members, &destination)?;
    Ok(members.len())
}

/// Reads back everything that was written and compares it byte for byte.
///
/// Writes to a FAT volume over USB are buffered by the host, and a cable
/// pulled at the wrong moment leaves a file that exists, has a plausible size,
/// and is not what was sent. On a reader that surfaces as a program which
/// starts and immediately dies, with nothing to read afterwards because the
/// volume is gone. So the bytes are read back while the volume is still
/// mounted and still ours, which is the only moment this can be checked at.
///
/// # Errors
///
/// When a file is missing, short, or different, naming which, because the
/// answer determines whether to run setup again or replace the cable.
pub fn verify_payload(members: &[crate::package::Member], volume: &Path) -> Result<(), String> {
    let prefix = format!("{}/", crate::package::INSTALL_ROOT);
    let destination = volume.join(INSTALL_FOLDER);
    for member in members {
        let relative = member
            .path
            .strip_prefix(&prefix)
            .ok_or_else(|| format!("{:?} is outside the install root", member.path))?;
        let path = destination.join(relative);
        let written =
            fs::read(&path).map_err(|error| format!("read back {}: {error}", path.display()))?;
        if written.len() != member.bytes.len() {
            return Err(format!(
                "{} was written short: {} bytes on the reader, {} sent",
                path.display(),
                written.len(),
                member.bytes.len()
            ));
        }
        if written != member.bytes {
            return Err(format!(
                "{} differs from what was sent; the volume may be failing",
                path.display()
            ));
        }
    }
    Ok(())
}

/// Removes an installed Cobalt from a mounted reader.
///
/// # Errors
///
/// When the folder exists but cannot be removed.
pub fn remove_payload(volume: &Path) -> Result<bool, String> {
    let installed = volume.join(INSTALL_FOLDER);
    if !installed.exists() {
        return Ok(false);
    }
    fs::remove_dir_all(&installed)
        .map_err(|error| format!("remove {}: {error}", installed.display()))?;
    Ok(true)
}

/// Flushes the volume and ejects it, so the reader remounts its own storage.
///
/// A reader will not look at the book partition again until the cable is
/// logically disconnected, so an install that is never ejected is an install
/// the reader has not seen.
///
/// # Errors
///
/// When the eject tool is missing or refuses, usually because a terminal is
/// still sitting in a directory on the volume.
pub fn eject(volume: &Path) -> Result<(), String> {
    let _ = Command::new("sync").status();
    if !cfg!(target_os = "macos") {
        return Err(format!(
            "eject {} yourself, then restart the reader",
            volume.display()
        ));
    }
    let output = Command::new("diskutil")
        .arg("eject")
        .arg(volume)
        .output()
        .map_err(|error| format!("diskutil: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "diskutil eject {} failed: {}",
        volume.display(),
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

/// What a completed setup did, in the order it did it.
#[derive(Clone, Debug, Default)]
pub struct Report {
    /// Files written into `.adds/cobalt`.
    pub installed: usize,
    /// What became of the SSH server.
    pub ssh: Option<Ssh>,
    /// What became of this machine's key, when one was asked for.
    pub key: Option<Result<(crate::authorize::Key, crate::authorize::Staged), String>>,
    /// Settings keys that changed.
    pub settings: Vec<String>,
    /// What became of the reader's own menu entry, when one was asked for.
    pub menu: Option<Result<crate::menu::Menu, String>>,
    /// Whether the volume was ejected.
    pub ejected: bool,
    /// Whether the command will wait for the restarted reader itself.
    pub waiting: bool,
}

impl Report {
    /// The whole of what happened, and how to undo each part of it.
    #[must_use]
    pub fn describe(&self, volume: &Path) -> String {
        let mut text = String::new();
        let _ = writeln!(text, "\nSet up {}:", volume.display());
        let _ = writeln!(
            text,
            "  · {} files installed into {INSTALL_FOLDER}",
            self.installed
        );
        if let Some(ssh) = self.ssh {
            let _ = writeln!(text, "  · {}", ssh.describe());
        }
        match &self.key {
            Some(Ok((key, staged))) => {
                let _ = writeln!(text, "  · {}", describe_key(*key, *staged));
            }
            // Reported, not raised. Everything else worked, and a key can be
            // installed on the next run once whatever holds the slot is gone.
            Some(Err(error)) => {
                let _ = writeln!(text, "  · this machine's key was not installed: {error}");
            }
            None => {}
        }
        if self.settings.is_empty() {
            let _ = writeln!(text, "  · settings already as wanted");
        } else {
            let _ = writeln!(text, "  · settings set: {}", self.settings.join(", "));
        }
        match &self.menu {
            Some(Ok(menu)) => {
                let _ = writeln!(text, "  · {}", menu.describe());
            }
            // Reported, not raised. The install itself succeeded, and Cobalt
            // still starts from start.sh over SSH without any menu entry.
            Some(Err(error)) => {
                let _ = writeln!(text, "  · no menu entry: {error}");
            }
            None => {}
        }
        let _ = writeln!(
            text,
            "  · {}",
            if self.ejected {
                "volume ejected"
            } else {
                "volume left mounted"
            }
        );
        text.push_str(&next_steps(self.waiting, self.staged_an_archive()));
        text
    }

    /// Whether anything was left for the firmware to extract as root.
    ///
    /// Both the menu plugin and this machine's key are staged the same way, so
    /// either one makes the "nothing was extracted as root" wording false.
    fn staged_an_archive(&self) -> Staged {
        let plugin = matches!(self.menu, Some(Ok(crate::menu::Menu::Staged)));
        let key = matches!(
            self.key,
            Some(Ok((
                _,
                crate::authorize::Staged::Written | crate::authorize::Staged::Merged
            )))
        );
        match (plugin, key) {
            (true, true) => Staged::PluginAndKey,
            (true, false) => Staged::Plugin,
            (false, true) => Staged::Key,
            (false, false) => Staged::Nothing,
        }
    }
}

/// What was done with this machine's key, in one line.
fn describe_key(key: crate::authorize::Key, staged: crate::authorize::Staged) -> String {
    let origin = match key {
        crate::authorize::Key::Created => "a key was created for this machine and",
        crate::authorize::Key::Existing => "this machine's key",
    };
    match staged {
        crate::authorize::Staged::Written | crate::authorize::Staged::Merged => format!(
            "{origin} will be accepted by the reader after it restarts. This replaces \
             /root/.ssh/authorized_keys rather than adding to it, because that file is on \
             the root filesystem and USB cannot read it back. Another machine that had \
             access will need to run this again from its own desk."
        ),
        crate::authorize::Staged::SlotTaken => format!(
            "{origin} was not installed: another archive is already waiting in \
             .kobo/KoboRoot.tgz. Restart the reader to let it be taken, then run \
             'kobo setup --enable-ssh --no-menu' again."
        ),
    }
}

/// What the owner has to do, and what to do if they want none of this.
///
/// The third step differs by whether this command is about to do it for them.
/// Telling somebody to run `kobo devices` and then running it for them reads
/// as though one of the two did not happen.
#[must_use]
pub fn next_steps(waiting: bool, staged: Staged) -> String {
    let finding = if waiting {
        "  3. This command is waiting for it, and will print its address when it\n\
         \x20    appears. Ctrl-C stops the wait; nothing on the reader depends on it."
    } else {
        "  3. Find it with 'kobo devices', then 'kobo deploy' works from here on."
    };
    format!(
        "{}{}{finding}{NEXT_STEPS_TAIL}",
        staged.scope(),
        staged.restart()
    )
}

/// What ended up in the single archive the firmware extracts as root.
///
/// Named rather than counted, because this paragraph is the one part of the
/// report an owner cannot check with a file manager, and a paragraph that
/// describes NickelMenu on a reader that only received a key is worse than no
/// paragraph at all.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Staged {
    /// Nothing. Everything went to the book partition.
    Nothing,
    /// The plugin alone, on a reader that asked for no key.
    Plugin,
    /// This machine's key alone, on a reader that already had the plugin.
    Key,
    /// Both, because there is only one slot and they had to travel together.
    PluginAndKey,
}

impl Staged {
    fn scope(self) -> &'static str {
        match self {
            Self::Nothing => UNTOUCHED_SCOPE,
            Self::Plugin => PLUGIN_SCOPE,
            Self::Key => KEY_SCOPE,
            Self::PluginAndKey => PLUGIN_AND_KEY_SCOPE,
        }
    }

    /// Whether the owner has to restart the reader, or the firmware will.
    fn restart(self) -> &'static str {
        if self == Self::Nothing {
            RESTART_BY_HAND
        } else {
            RESTARTS_ITSELF
        }
    }
}

/// What was written, when the only thing written was the book partition.
const UNTOUCHED_SCOPE: &str = "
Nothing was written outside the book partition, and nothing was extracted as
root. To undo all of it: 'kobo setup --undo', or delete .adds/cobalt and rename
.kobo/ssh-enabled back to ssh-disabled.
";

/// What was written, when the plugin was staged and no key was asked for.
///
/// Said plainly because it is the one thing this command does that the owner
/// cannot undo with a file manager. The archive was listed before it was
/// written and contains only NickelMenu's plugin and its documentation, but it
/// is still the firmware that extracts it, and it still extracts it as root.
const PLUGIN_SCOPE: &str = "
One archive was staged for the firmware to extract as root at the next restart:
NickelMenu, checked first to contain nothing but its own plugin and its own
documentation. Everything else was written to the book partition. NickelMenu
removes itself if it fails to start, so it cannot leave the reader unable to
boot. To undo all of it: 'kobo setup --undo'.
";

/// What was written, when the reader already had the plugin.
const KEY_SCOPE: &str = "
One archive was staged for the firmware to extract as root at the next restart:
this machine's public key, and nothing else. It becomes
/root/.ssh/authorized_keys, which is the file the reader's SSH server reads to
decide who may log in. Everything else was written to the book partition. To
undo all of it: 'kobo setup --undo'.
";

/// What was written, on a reader receiving both. The usual first-time case.
const PLUGIN_AND_KEY_SCOPE: &str = "
One archive was staged for the firmware to extract as root at the next restart,
holding two things, because the firmware extracts one archive and both had to
travel in it: NickelMenu, checked first to contain nothing but its own plugin
and its own documentation, and this machine's public key, which becomes
/root/.ssh/authorized_keys and is the file the reader's SSH server reads to
decide who may log in. Everything else was written to the book partition.
NickelMenu removes itself if it fails to start, so it cannot leave the reader
unable to boot. To undo all of it: 'kobo setup --undo'.
";

/// Everything above the step that differs.
const RESTART_BY_HAND: &str = "
Next, on the reader:

  1. Restart it. Hold the power button until it powers off, then press it
     again. The SSH server only starts at boot.
  2. Join it to Wi-Fi if it is not already.
";

/// The same, for a reader that was left an archive.
///
/// Measured rather than assumed: ejecting a reader with an archive waiting
/// makes the firmware show its Updating screen, take the archive, and reboot,
/// with nobody touching the power button. Telling somebody to restart a reader
/// that has already restarted reads as though the command did not work.
const RESTARTS_ITSELF: &str = "
Next, on the reader:

  1. Nothing. It restarts by itself: ejecting leaves the archive where the
     firmware looks, so it shows its Updating screen, takes it, and reboots.
     The SSH server starts with that boot.
  2. Join it to Wi-Fi if it is not already.
";

/// Everything below it.
const NEXT_STEPS_TAIL: &str = "

Cobalt itself is started from the Cobalt entry in the reader's own menu, or
from .adds/cobalt/start.sh. Starting it stops the reader and takes the screen;
a restart always returns to the stock reader.

The reader is also set to stay awake for ninety minutes rather than a few, so
that it is still reachable when you come back to it. That costs battery. The
reader's own Energy saving screen changes it back at any time, as does
'kobo setup --undo'.
";

/// How long a restarted reader is given to come back on the network.
///
/// A Kobo takes about a minute to boot and another to join Wi-Fi, and somebody
/// who walked away to fetch the cable takes longer than both. Five minutes is
/// long enough not to give up on a working reader and short enough that a
/// forgotten terminal is not still sweeping an hour later.
pub const WAIT_LIMIT: Duration = Duration::from_secs(300);

/// How long between sweeps while waiting.
pub const WAIT_INTERVAL: Duration = Duration::from_secs(10);

/// What a look at one newly-arrived address concluded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// It answered as a reader.
    Reader,
    /// It answered, and is some other machine.
    Other,
    /// It could not be asked yet. A reader accepts connections while it is
    /// still booting, well before it will hold a conversation.
    Unknown,
}

/// What came back from waiting for a restarted reader.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Arrival {
    /// One address that was not answering before is answering now, and said it
    /// was a reader.
    Found(Ipv4Addr),
    /// Several did, so this will not guess which is the wanted one.
    Several(Vec<Ipv4Addr>),
    /// No reader answered before the limit ran out. Carries any addresses that
    /// did appear and turned out to be something else, so that nothing
    /// happening and the wrong machine arriving can be told apart.
    TimedOut(Vec<Ipv4Addr>),
}

/// Waits for a reader to start answering on the SSH port that was not
/// answering when the wait began.
///
/// Two things narrow it down. The first is *change* (it was off the network a
/// moment ago and is on it now) which alone rules out the machine this is
/// running on, the router and a NAS, since those were all there at the start.
/// Change is necessary and is not sufficient: a laptop waking from sleep
/// mid-wait is also a newcomer, and naming it as the reader sends somebody to
/// install on a stranger's machine.
///
/// So each newcomer is asked what it is. An earlier attempt guessed from the
/// SSH banner instead, on the assumption that a Kobo runs Dropbear. The reader
/// this was written for runs OpenSSH 8.9 (the same server a laptop runs) so
/// the banner cannot tell them apart, and that check rejected the very device
/// it was meant to find. Asking costs a login, which a just-booted reader
/// gives away: its firmware clears root's password at every boot and permits
/// empty ones.
///
/// `sweep` returns everything answering right now, `probe` asks one address
/// what it is, and `pause` waits out one interval and returns false when there
/// is no time left. All three are passed in so the whole of the decision can be
/// tested without a network.
pub fn wait_for_reader(
    mut sweep: impl FnMut() -> Vec<Ipv4Addr>,
    mut probe: impl FnMut(Ipv4Addr) -> Verdict,
    mut pause: impl FnMut() -> bool,
) -> Arrival {
    let baseline: BTreeSet<Ipv4Addr> = sweep().into_iter().collect();
    let mut settled: BTreeSet<Ipv4Addr> = BTreeSet::new();
    let mut passed_over: Vec<Ipv4Addr> = Vec::new();
    while pause() {
        let mut arrived: Vec<Ipv4Addr> = sweep()
            .into_iter()
            .filter(|address| !baseline.contains(address) && !settled.contains(address))
            .collect();
        arrived.sort_unstable();
        arrived.dedup();
        let mut readers = Vec::new();
        for address in arrived {
            match probe(address) {
                // Not a verdict, and deliberately not settled. Asking again
                // next round is the whole point of there being a third answer:
                // a booting reader accepts a connection minutes before it will
                // answer a question, and writing it off here would lose the
                // device for the rest of the wait.
                Verdict::Unknown => {}
                Verdict::Reader => {
                    settled.insert(address);
                    readers.push(address);
                }
                Verdict::Other => {
                    settled.insert(address);
                    passed_over.push(address);
                }
            }
        }
        match readers.len() {
            0 => {}
            1 => return Arrival::Found(readers[0]),
            _ => return Arrival::Several(readers),
        }
    }
    Arrival::TimedOut(passed_over)
}

#[cfg(test)]
mod tests {
    use super::{
        clear_setting, is_kobo_serial, next_steps, parse_version, set_setting, wait_for_reader,
        Arrival, Mounted, Report, Ssh, Staged, Verdict, INSTALL_FOLDER, SETTINGS_APPLIED,
        SSH_DISABLED, SSH_ENABLED,
    };
    use std::net::Ipv4Addr;
    use std::path::PathBuf;

    fn address(last: u8) -> Ipv4Addr {
        Ipv4Addr::new(192, 168, 1, last)
    }

    /// Sweeps that return each of `rounds` in turn, then the last one forever.
    fn sweeps(rounds: Vec<Vec<u8>>) -> impl FnMut() -> Vec<Ipv4Addr> {
        let mut index = 0;
        move || {
            let round = rounds[index.min(rounds.len() - 1)].clone();
            index += 1;
            round.into_iter().map(address).collect()
        }
    }

    /// A clock that allows exactly `limit` more rounds.
    fn rounds(limit: usize) -> impl FnMut() -> bool {
        let mut left = limit;
        move || {
            let more = left > 0;
            left = left.saturating_sub(1);
            more
        }
    }

    /// A probe for which every address is a reader.
    fn all_readers() -> impl FnMut(Ipv4Addr) -> Verdict {
        |_| Verdict::Reader
    }

    /// A probe for which only `readers` are readers and the rest are ordinary
    /// machines.
    fn only(readers: Vec<u8>) -> impl FnMut(Ipv4Addr) -> Verdict {
        move |seen| {
            if readers.iter().any(|last| address(*last) == seen) {
                Verdict::Reader
            } else {
                Verdict::Other
            }
        }
    }

    #[test]
    fn a_machine_already_on_the_network_is_not_mistaken_for_the_reader() {
        // The router, this laptop and a NAS all answer on 22 the whole time.
        // None of them restarted, so none of them is what was waited for, even
        // though a banner check would have accepted all three.
        let arrival = wait_for_reader(sweeps(vec![vec![1, 5, 40]]), all_readers(), rounds(4));
        assert_eq!(arrival, Arrival::TimedOut(Vec::new()));
    }

    #[test]
    fn the_one_address_that_joined_is_the_answer() {
        let arrival = wait_for_reader(
            sweeps(vec![vec![1, 5], vec![1, 5], vec![1, 5, 22]]),
            all_readers(),
            rounds(4),
        );
        assert_eq!(arrival, Arrival::Found(address(22)));
    }

    #[test]
    fn two_arrivals_at_once_are_reported_rather_than_guessed_between() {
        let arrival = wait_for_reader(
            sweeps(vec![vec![1], vec![1, 22, 23]]),
            all_readers(),
            rounds(4),
        );
        assert_eq!(arrival, Arrival::Several(vec![address(22), address(23)]));
    }

    #[test]
    fn a_machine_that_drops_off_while_waiting_is_not_an_arrival() {
        // Fewer answering than before is still nothing new answering.
        let arrival = wait_for_reader(
            sweeps(vec![vec![1, 5, 40], vec![1]]),
            all_readers(),
            rounds(3),
        );
        assert_eq!(arrival, Arrival::TimedOut(Vec::new()));
    }

    #[test]
    fn the_wait_gives_up_rather_than_sweeping_for_ever() {
        let mut taken = 0;
        let arrival = wait_for_reader(
            || {
                taken += 1;
                Vec::new()
            },
            all_readers(),
            rounds(3),
        );
        assert_eq!(arrival, Arrival::TimedOut(Vec::new()));
        assert_eq!(taken, 4, "one baseline sweep and one per allowed round");
    }

    #[test]
    fn a_laptop_waking_from_sleep_mid_wait_is_not_named_as_the_reader() {
        // Change alone named .10, which was a machine that had woken up.
        let arrival = wait_for_reader(sweeps(vec![vec![1], vec![1, 10]]), only(vec![]), rounds(4));
        assert_eq!(arrival, Arrival::TimedOut(vec![address(10)]));
    }

    #[test]
    fn a_reader_arriving_after_some_other_machine_is_still_found() {
        // Passing one over must not end the wait, or the reader that comes up
        // half a minute later is never seen.
        let arrival = wait_for_reader(
            sweeps(vec![vec![1], vec![1, 10], vec![1, 10, 22]]),
            only(vec![22]),
            rounds(5),
        );
        assert_eq!(arrival, Arrival::Found(address(22)));
    }

    #[test]
    fn an_address_that_answered_is_only_probed_once() {
        let mut asked = Vec::new();
        let arrival = wait_for_reader(
            sweeps(vec![vec![1], vec![1, 10]]),
            |seen| {
                asked.push(seen);
                Verdict::Other
            },
            rounds(4),
        );
        assert_eq!(arrival, Arrival::TimedOut(vec![address(10)]));
        assert_eq!(
            asked,
            vec![address(10)],
            "it stays in view but is not re-probed"
        );
    }

    #[test]
    fn an_address_that_said_nothing_is_asked_again() {
        // A Kobo accepts a connection while booting before its SSH server will
        // talk. Reading one silence as a verdict would lose the reader for the
        // rest of the wait.
        let mut asked = 0;
        let arrival = wait_for_reader(
            sweeps(vec![vec![1], vec![1, 22]]),
            |_| {
                asked += 1;
                if asked < 3 {
                    Verdict::Unknown
                } else {
                    Verdict::Reader
                }
            },
            rounds(5),
        );
        assert_eq!(arrival, Arrival::Found(address(22)));
        assert_eq!(asked, 3, "asked again each round until it answered");
    }

    #[test]
    fn only_the_reader_among_several_arrivals_is_named() {
        let arrival = wait_for_reader(
            sweeps(vec![vec![1], vec![1, 10, 22]]),
            only(vec![22]),
            rounds(4),
        );
        assert_eq!(arrival, Arrival::Found(address(22)));
    }

    #[test]
    fn a_reader_that_runs_the_same_ssh_server_as_a_laptop_is_still_found() {
        // The reader this was written for runs OpenSSH 8.9, exactly as an
        // ordinary Linux machine does. Nothing about the wait may depend on
        // the two being distinguishable without asking.
        let arrival = wait_for_reader(
            sweeps(vec![vec![1], vec![1, 10, 22]]),
            only(vec![22]),
            rounds(4),
        );
        assert_eq!(arrival, Arrival::Found(address(22)));
    }

    #[test]
    fn a_reader_that_restarts_itself_is_not_told_to_restart() {
        // Watched on hardware: the reader was ejected, showed its Updating
        // screen, took the archive and rebooted, with nobody touching the
        // power button. The instruction to hold it down described work that
        // was already done.
        for staged in [Staged::Plugin, Staged::Key, Staged::PluginAndKey] {
            let text = next_steps(false, staged);
            assert!(text.contains("restarts by itself"), "{text}");
            assert!(!text.contains("Hold the power button"), "{text}");
        }
        let untouched = next_steps(false, Staged::Nothing);
        assert!(untouched.contains("Hold the power button"), "{untouched}");
    }

    #[test]
    fn the_staged_paragraph_names_what_was_actually_staged() {
        // Found on a real reader: it had NickelMenu already, so only the key
        // was staged, and the report still described the archive as
        // NickelMenu's plugin and documentation. This paragraph is the one
        // part an owner cannot check with a file manager, so it has to be
        // about the archive that exists.
        let key_only = next_steps(false, Staged::Key);
        assert!(key_only.contains("authorized_keys"), "{key_only}");
        assert!(!key_only.contains("NickelMenu"), "{key_only}");

        let plugin_only = next_steps(false, Staged::Plugin);
        assert!(plugin_only.contains("NickelMenu"), "{plugin_only}");
        assert!(!plugin_only.contains("authorized_keys"), "{plugin_only}");

        let both = next_steps(false, Staged::PluginAndKey);
        assert!(both.contains("NickelMenu"), "{both}");
        assert!(both.contains("authorized_keys"), "{both}");
        assert!(both.contains("one archive"), "{both}");

        let neither = next_steps(false, Staged::Nothing);
        assert!(neither.contains("nothing was extracted as"), "{neither}");
    }

    #[test]
    fn the_report_picks_the_paragraph_from_what_it_did() {
        let report = |menu, key| Report {
            installed: 19,
            ssh: Some(Ssh::Enabled),
            key,
            settings: Vec::new(),
            menu,
            ejected: true,
            waiting: false,
        };
        let staged_key = Some(Ok((
            crate::authorize::Key::Existing,
            crate::authorize::Staged::Written,
        )));
        assert_eq!(
            report(Some(Ok(crate::menu::Menu::Added)), staged_key.clone()).staged_an_archive(),
            Staged::Key
        );
        assert_eq!(
            report(Some(Ok(crate::menu::Menu::Staged)), None).staged_an_archive(),
            Staged::Plugin
        );
        assert_eq!(
            report(Some(Ok(crate::menu::Menu::Staged)), staged_key).staged_an_archive(),
            Staged::PluginAndKey
        );
        assert_eq!(
            report(Some(Ok(crate::menu::Menu::Unchanged)), None).staged_an_archive(),
            Staged::Nothing
        );
    }

    #[test]
    fn the_reader_is_not_told_to_go_looking_for_it_while_this_is_looking_for_it() {
        assert!(next_steps(true, Staged::Nothing).contains("waiting for it"));
        assert!(!next_steps(true, Staged::Nothing).contains("Find it with"));
        assert!(next_steps(false, Staged::Nothing).contains("Find it with 'kobo devices'"));
        for waiting in [true, false] {
            let text = next_steps(waiting, Staged::Nothing);
            assert!(text.contains("kobo setup --undo"), "undo is always offered");
            assert!(
                text.contains("Restart it"),
                "the restart is always asked for"
            );
            assert!(
                text.contains("ninety minutes"),
                "the sleep change is declared"
            );
        }
    }

    #[test]
    fn a_version_line_yields_the_serial_and_the_firmware() {
        let (serial, firmware) =
            parse_version("N365410043013,4.9.77,4.45.23697,4.9.77,4.9.77,00000000-0000\n");
        assert_eq!(serial, "N365410043013");
        assert_eq!(firmware, "4.45.23697");
    }

    #[test]
    fn a_truncated_version_line_is_not_an_error() {
        let (serial, firmware) = parse_version("N365410043013");
        assert_eq!(serial, "N365410043013");
        assert!(firmware.is_empty());
    }

    #[test]
    fn only_an_n_and_three_digits_is_a_reader() {
        assert!(is_kobo_serial("N365410043013"));
        assert!(!is_kobo_serial("Macintosh HD"));
        assert!(!is_kobo_serial("N36"));
        assert!(!is_kobo_serial("NABC410043013"));
    }

    #[test]
    fn an_existing_key_is_replaced_in_place_and_nothing_else_moves() {
        let before = "[ApplicationPreferences]\nCurrentLocale=en_US\n\n[DeveloperSettings]\nForceWifiOn=false\n\n[PowerOptions]\nAutoColorEnabled=true\n";
        let after = set_setting(before, "DeveloperSettings", "ForceWifiOn", "true");
        assert!(after.contains("ForceWifiOn=true"));
        assert!(!after.contains("ForceWifiOn=false"));
        assert!(after.contains("CurrentLocale=en_US"));
        assert!(after.contains("AutoColorEnabled=true"));
        assert_eq!(after.lines().count(), before.lines().count());
    }

    #[test]
    fn a_missing_key_joins_its_section_rather_than_starting_a_new_one() {
        let before = "[DeveloperSettings]\nSomething=1\n\n[PowerOptions]\nAutoColorEnabled=true\n";
        let after = set_setting(before, "DeveloperSettings", "ForceWifiOn", "true");
        assert_eq!(after.matches("[DeveloperSettings]").count(), 1);
        let developer = after.find("[DeveloperSettings]").expect("section");
        let power = after.find("[PowerOptions]").expect("section");
        let key = after.find("ForceWifiOn=true").expect("key");
        assert!(developer < key && key < power, "{after}");
    }

    #[test]
    fn a_missing_section_is_appended_whole() {
        let before = "[PowerOptions]\nAutoColorEnabled=true\n";
        let after = set_setting(before, "DeveloperSettings", "ForceWifiOn", "true");
        assert!(
            after.contains("[DeveloperSettings]\nForceWifiOn=true"),
            "{after}"
        );
        assert!(after.contains("AutoColorEnabled=true"));
    }

    #[test]
    fn an_empty_settings_file_gains_exactly_one_section() {
        let after = set_setting("", "DeveloperSettings", "ForceWifiOn", "true");
        assert_eq!(after, "[DeveloperSettings]\nForceWifiOn=true\n");
    }

    #[test]
    fn setting_a_value_that_is_already_set_changes_nothing() {
        let before = "[DeveloperSettings]\nForceWifiOn=true\n";
        assert_eq!(
            set_setting(before, "DeveloperSettings", "ForceWifiOn", "true"),
            before
        );
    }

    #[test]
    fn a_key_of_the_same_name_in_another_section_is_left_alone() {
        let before = "[Other]\nForceWifiOn=false\n\n[DeveloperSettings]\nX=1\n";
        let after = set_setting(before, "DeveloperSettings", "ForceWifiOn", "true");
        assert!(after.contains("[Other]\nForceWifiOn=false"), "{after}");
        assert_eq!(after.matches("ForceWifiOn").count(), 2);
    }

    #[test]
    fn clearing_removes_only_the_named_key() {
        let before = "[DeveloperSettings]\nForceWifiOn=true\nSomething=1\n";
        let after = clear_setting(before, "DeveloperSettings", "ForceWifiOn");
        assert!(!after.contains("ForceWifiOn"));
        assert!(after.contains("Something=1"));
        assert!(after.contains("[DeveloperSettings]"));
    }

    #[test]
    fn setting_then_clearing_returns_the_original() {
        let before = "[ApplicationPreferences]\nCurrentLocale=en_US\n\n[PowerOptions]\nAutoColorEnabled=true\n";
        let set = set_setting(before, "DeveloperSettings", "ForceWifiOn", "true");
        let cleared = clear_setting(&set, "DeveloperSettings", "ForceWifiOn");
        assert!(cleared.contains("CurrentLocale=en_US"));
        assert!(!cleared.contains("ForceWifiOn"));
    }

    #[test]
    fn the_ssh_markers_differ_only_in_the_last_word() {
        assert_eq!(SSH_DISABLED.replace("disabled", "enabled"), SSH_ENABLED);
    }

    #[test]
    fn nothing_this_command_writes_leaves_the_book_partition() {
        assert!(!INSTALL_FOLDER.starts_with('/'));
        assert!(!SSH_ENABLED.starts_with('/'));
        for (section, key, _) in SETTINGS_APPLIED {
            assert!(!section.is_empty() && !key.is_empty());
        }
    }

    #[test]
    fn every_setting_this_command_writes_can_be_taken_back_off_again() {
        // A stock file, in the shape the reader writes: sections it already
        // has, sections it does not, and keys around the ones being changed.
        let before = "[ApplicationPreferences]\nCurrentLocale=en_US\n\n\
                      [PowerOptions]\nAutoColorEnabled=true\nFrontLightLevel=7\n";
        let mut after = before.to_owned();
        for (section, key, value) in SETTINGS_APPLIED {
            after = set_setting(&after, section, key, value);
        }
        for (section, key, value) in SETTINGS_APPLIED {
            assert!(after.contains(&format!("{key}={value}")), "{after}");
            // In its own section, not appended to whichever came last.
            let header = after.find(&format!("[{section}]")).expect("section");
            let assignment = after.find(&format!("{key}={value}")).expect("key");
            assert!(header < assignment, "{key} landed outside {section}");
        }
        // Keys the reader owns are untouched throughout.
        assert!(after.contains("FrontLightLevel=7"));
        assert!(after.contains("CurrentLocale=en_US"));

        let mut undone = after;
        for (section, key, _) in SETTINGS_APPLIED {
            undone = clear_setting(&undone, section, key);
        }
        for (_, key, _) in SETTINGS_APPLIED {
            assert!(!undone.contains(key), "{key} survived the undo: {undone}");
        }
        assert!(undone.contains("FrontLightLevel=7"));
    }

    #[test]
    fn the_reader_is_kept_awake_long_enough_to_work_on_it() {
        // The whole point of the setup is that the device is reachable after
        // it is put down. A value small enough to suspend mid-deploy would
        // leave that broken in a way only hardware would show.
        let minutes: u32 = SETTINGS_APPLIED
            .iter()
            .find(|(section, key, _)| *section == "PowerOptions" && *key == "AutoSleepMinutes")
            .expect("the sleep timer is set")
            .2
            .parse()
            .expect("a number of minutes");
        assert!(
            minutes >= 60,
            "the reader would sleep after {minutes} minutes"
        );
    }

    #[test]
    fn an_unsupported_firmware_says_what_to_do_about_it() {
        assert!(Ssh::Unsupported.describe().contains("Update the reader"));
        assert!(Ssh::Enabled.describe().contains("next restart"));
    }

    #[test]
    fn a_report_names_every_change_and_how_to_undo_them() {
        let report = Report {
            installed: 13,
            ssh: Some(Ssh::Enabled),
            key: None,
            settings: vec!["DeveloperSettings/ForceWifiOn".to_owned()],
            menu: None,
            ejected: true,
            waiting: false,
        };
        let text = report.describe(&PathBuf::from("/Volumes/KOBOeReader"));
        assert!(text.contains("13 files"));
        assert!(text.contains("SSH enabled"));
        assert!(text.contains("ForceWifiOn"));
        assert!(text.contains("--undo"));
        assert!(text.contains("nothing was extracted as\nroot"), "{text}");
    }

    #[test]
    fn a_report_that_staged_an_archive_stops_claiming_nothing_was_extracted() {
        // The claim is the point. It is true of every other thing this command
        // does, and staging KoboRoot.tgz is the one case where it is not.
        let report = Report {
            installed: 13,
            ssh: Some(Ssh::Enabled),
            key: None,
            settings: Vec::new(),
            menu: Some(Ok(crate::menu::Menu::Staged)),
            ejected: true,
            waiting: false,
        };
        let text = report.describe(&PathBuf::from("/Volumes/KOBOeReader"));
        assert!(!text.contains("nothing was extracted as"), "{text}");
        assert!(text.contains("extract as root"), "{text}");
        assert!(
            text.contains("cannot leave the reader unable to\nboot"),
            "{text}"
        );
        assert!(text.contains("--undo"), "{text}");
    }

    #[test]
    fn a_report_that_installed_a_key_stops_claiming_nothing_was_extracted() {
        // The key is staged the same way the menu plugin is, so it makes the
        // same claim false. Reporting it as though nothing left the book
        // partition would be the one untruth in this whole report.
        let report = Report {
            installed: 13,
            ssh: Some(Ssh::Enabled),
            key: Some(Ok((
                crate::authorize::Key::Created,
                crate::authorize::Staged::Written,
            ))),
            settings: Vec::new(),
            menu: None,
            ejected: true,
            waiting: false,
        };
        let text = report.describe(&PathBuf::from("/Volumes/KOBOeReader"));
        assert!(!text.contains("nothing was extracted as"), "{text}");
        assert!(
            text.contains("a key was created for this machine"),
            "{text}"
        );
        assert!(text.contains("after it restarts"), "{text}");
    }

    #[test]
    fn a_key_that_could_not_be_staged_says_what_to_do_about_it() {
        let report = Report {
            installed: 13,
            ssh: Some(Ssh::Enabled),
            key: Some(Ok((
                crate::authorize::Key::Existing,
                crate::authorize::Staged::SlotTaken,
            ))),
            settings: Vec::new(),
            menu: None,
            ejected: true,
            waiting: false,
        };
        let text = report.describe(&PathBuf::from("/Volumes/KOBOeReader"));
        assert!(
            text.contains("another archive is already waiting"),
            "{text}"
        );
        assert!(text.contains("--enable-ssh --no-menu"), "{text}");
        // Nothing of ours was staged, so the claim is still true.
        assert!(text.contains("nothing was extracted as\nroot"), "{text}");
    }

    #[test]
    fn a_key_that_failed_is_reported_without_failing_the_install() {
        let report = Report {
            installed: 13,
            ssh: Some(Ssh::Enabled),
            key: Some(Err("ssh-keygen refused".to_owned())),
            settings: Vec::new(),
            menu: None,
            ejected: true,
            waiting: false,
        };
        let text = report.describe(&PathBuf::from("/Volumes/KOBOeReader"));
        assert!(text.contains("13 files"), "{text}");
        assert!(
            text.contains("was not installed: ssh-keygen refused"),
            "{text}"
        );
    }

    #[test]
    fn a_report_that_only_wrote_the_entry_keeps_the_claim() {
        let report = Report {
            installed: 13,
            ssh: Some(Ssh::Enabled),
            key: None,
            settings: Vec::new(),
            menu: Some(Ok(crate::menu::Menu::Added)),
            ejected: true,
            waiting: false,
        };
        let text = report.describe(&PathBuf::from("/Volumes/KOBOeReader"));
        assert!(text.contains("nothing was extracted as\nroot"), "{text}");
    }

    #[test]
    fn what_was_written_is_read_back_and_compared() {
        use super::{verify_payload, write_payload};
        use crate::package::{Member, INSTALL_ROOT};

        let root = std::env::temp_dir().join(format!("kobo-setup-readback-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temporary volume");
        let members = vec![Member {
            path: format!("{INSTALL_ROOT}/bin/kobod"),
            bytes: b"a whole binary".to_vec(),
            program: true,
        }];

        assert_eq!(write_payload(&members, &root).expect("write"), 1);
        verify_payload(&members, &root).expect("what was written reads back");

        let installed = root.join(INSTALL_FOLDER).join("bin/kobod");
        std::fs::write(&installed, b"a whole").expect("truncate");
        let error = verify_payload(&members, &root).expect_err("a short file is caught");
        assert!(error.contains("written short"), "{error}");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_found_reader_names_its_model_and_where_it_is() {
        let reader = Mounted {
            volume: PathBuf::from("/Volumes/KOBOeReader"),
            serial: "N365410043013".to_owned(),
            firmware: "4.45.23697".to_owned(),
        };
        assert_eq!(reader.model_code(), "N365");
        assert!(reader.summary().contains("/Volumes/KOBOeReader"));
        assert!(reader.summary().contains("4.45.23697"));
    }
}

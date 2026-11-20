//! Apps on a reader that is plugged in rather than running.
//!
//! The Store on the reader installs applications over Wi-Fi, and it was the
//! only thing that could. An owner at a computer with the reader on a cable
//! could not see what was installed, add an application or take one away
//! without opening the browser installer in one of the three browsers it
//! runs in.
//!
//! Nothing here is a second installer. The transaction is the runtime's own,
//! [`kobod::app_store`], pointed at the installation on the mounted book
//! partition instead of the one it is running from: the same signed catalog
//! checked against the same built-in key, the same package length, digest
//! and bundle signature, the same staged directory swap, and the same
//! recovery of a swap that was interrupted. A package this refuses is one the
//! reader would have refused, for the same reason.
//!
//! # What is different on a cable
//!
//! The Cobalt that will run a package is the one on the volume, not this
//! build, so [`Installation`] reads the release from the volume and every
//! compatibility answer is given for that release. The catalog is the one for
//! the channel the reader's own Settings chose, read from the same file the
//! runtime keeps it in.
//!
//! The volume is the same FAT partition the reader installs onto, but it is
//! mounted by this computer's driver and its writes sit in this computer's
//! cache until the volume is ejected. The transaction flushes every file and
//! directory it touches, which is what makes it survive a power cut on the
//! reader; it is not a promise about a cable pulled out of a computer. Every
//! change therefore ends by saying to eject, and `--eject` does it.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use kobo_app_store::Ed25519PublicKey;
use kobo_protocol::{AppInfo, DeviceError, UpdateChannel};
use serde_json::{json, Value};

use crate::console::{self, Console};
use crate::setup;

/// What `kobo apps` says about the verbs that act on a plugged-in reader.
pub const HELP: &str = "Apps on a reader connected by USB

  kobo apps installed          List the apps on the reader
  kobo apps available          List the published apps the reader can install
  kobo apps install APP...     Install apps, or update ones already there
  kobo apps remove APP         Remove an app and keep its data

Plug the reader in and tap Connect on its screen first. Cobalt must already
be on it; kobo setup installs it.

  --volume PATH   The mounted reader, when more than one is connected
  --eject         Eject the reader when the command has finished
  --json          Print one JSON object instead of text

APP is the app's id or its name as the Store shows it. Quote a name with a
space in it: kobo apps install \"music stand\".

Apps come from the signed catalog for the channel the reader is on, and each
download is checked against that catalog and the app's own signature before
it is written, exactly as the reader's Store checks it.";

/// Whether `verb` is one of the verbs this module answers.
#[must_use]
pub fn handles(verb: &str) -> bool {
    matches!(verb, "installed" | "available" | "install" | "remove")
}

/// What kind of thing went wrong, for a caller that does more with a
/// failure than print it.
///
/// The sentence is for the owner and is all the command line shows. The
/// companion window reads the kind as well, because what to offer next
/// differs: a catalog that could not be reached is tried again, a download
/// that failed its signature is reported, a reader that could not be written
/// to is looked at.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Trouble {
    /// The catalog or a package could not be reached.
    Unreachable,
    /// Something did not match its signature or digest.
    Integrity,
    /// What was asked for is not in the catalog, or not on the reader.
    Missing,
    /// The reader's Cobalt is too old for the package.
    TooOld,
    /// The volume could not be read or written.
    Write,
    /// Anything else.
    Other,
}

/// A failure: what kind, and the sentence that says it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Failure {
    pub trouble: Trouble,
    pub said: String,
}

impl Failure {
    fn other(said: impl Into<String>) -> Self {
        Self {
            trouble: Trouble::Other,
            said: said.into(),
        }
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.said)
    }
}

impl From<Failure> for String {
    fn from(failure: Failure) -> Self {
        failure.said
    }
}

/// A Cobalt installation on a mounted reader.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Installation {
    /// The mount point of the reader's book partition.
    pub volume: PathBuf,
    /// The installation's own folder, which is the root the runtime's
    /// application transactions take.
    pub root: PathBuf,
    /// The Cobalt release on the volume, which is the one that will run
    /// whatever is installed here.
    pub version: String,
    /// The catalog the reader's Settings chose.
    pub channel: UpdateChannel,
}

impl Installation {
    /// Reads the installation on a mounted reader.
    ///
    /// # Errors
    ///
    /// Refuses a volume with no Cobalt on it, and one whose Cobalt does not
    /// name a release. Both are refusals rather than guesses because the
    /// release decides which packages may be written: an installation that
    /// cannot say what it is would have every compatibility question answered
    /// for some other Cobalt.
    pub fn on(volume: &Path) -> Result<Self, String> {
        let root = volume.join(setup::INSTALL_FOLDER);
        let is_folder = fs::symlink_metadata(&root).is_ok_and(|metadata| metadata.is_dir());
        let version = is_folder
            .then(|| fs::read_to_string(root.join("VERSION")).ok())
            .flatten()
            .map(|text| text.trim().to_owned());
        let Some(version) = version else {
            return Err(console::target(format!(
                "Cobalt is not installed on the reader at {}. Install it first: kobo setup",
                volume.display()
            )));
        };
        if !kobo_app_store::cobalt_version_at_least(&version, "0.0.0") {
            return Err(format!(
                "The Cobalt on the reader at {} does not say which release it is, so nothing \
                 can be checked against it. Run kobo setup to install it again; apps and their \
                 data are kept.",
                volume.display()
            ));
        }
        let channel = fs::read_to_string(root.join("state").join("auto-update"))
            .map_or(UpdateChannel::Stable, |text| channel_in(&text));
        Ok(Self {
            volume: volume.to_owned(),
            root,
            version,
            channel,
        })
    }

    /// The channel as the catalog's release tags and this tool's output
    /// spell it.
    #[must_use]
    pub const fn channel_name(&self) -> &'static str {
        match self.channel {
            UpdateChannel::Stable => "stable",
            UpdateChannel::Beta => "beta",
        }
    }

    /// Every app on the reader whose signed manifest verifies.
    ///
    /// Needs no network: it reads what is on the volume and nothing else.
    ///
    /// # Errors
    ///
    /// When an installed package cannot be read or does not verify.
    pub fn installed(&self, key: &Ed25519PublicKey) -> Result<Vec<AppInfo>, Failure> {
        kobod::app_store::installed_using(&self.root, key)
            .map_err(|error| self.explain(error, "read the apps on the reader"))
    }

    /// Fetches and verifies the published catalog for the reader's channel
    /// and lists what it offers, each entry marked with the version on the
    /// reader when there is one.
    ///
    /// The verified catalog is also left on the reader as its own Store's
    /// cache, by the same swap the Store uses, because the install that
    /// follows reads it from there.
    ///
    /// Applications that are part of Cobalt itself are left out. They are
    /// installed and updated with the platform, and offering them here would
    /// be offering something no verb in this module can act on.
    ///
    /// # Errors
    ///
    /// When the catalog cannot be fetched, fails verification, or cannot be
    /// written to the reader.
    pub fn available(
        &self,
        key: &Ed25519PublicKey,
        fetch: impl FnMut(&str, u32) -> Result<Vec<u8>, DeviceError>,
    ) -> Result<Vec<AppInfo>, Failure> {
        let mut apps = kobod::app_store::refresh_using(&self.root, self.channel, key, fetch)
            .map_err(|error| self.explain(error, "fetch the app catalog"))?;
        apps.retain(|app| !kobo_app_store::is_public_reserved_app_id(&app.id));
        self.tidy();
        Ok(apps)
    }

    /// Whether this reader's Cobalt is new enough to run `app`.
    #[must_use]
    pub fn runs(&self, app: &AppInfo) -> bool {
        kobo_app_store::cobalt_version_at_least(&self.version, &app.minimum_cobalt_version)
    }

    /// Installs or updates one app from the catalog [`Self::available`] just
    /// verified. `app` is that listing's entry for it.
    ///
    /// An app already on the reader is verified in full first, bytes
    /// included, so "already there" is never said of a package whose binary
    /// has rotted. One that fails is removed and installed afresh. The
    /// runtime's install refuses to build on a copy it cannot verify, and it
    /// is right to; taking the damaged copy away first is what its own Store
    /// has the owner do, and nothing is lost by it, because a copy that does
    /// not verify could not be opened anyway.
    ///
    /// # Errors
    ///
    /// When the reader's Cobalt is too old for the package, or the download,
    /// its verification or the write to the volume fails. Nothing is
    /// activated on the reader in any of those cases.
    pub fn install(
        &self,
        app: &AppInfo,
        key: &Ed25519PublicKey,
        fetch: impl FnMut(&str, u32) -> Result<Vec<u8>, DeviceError>,
    ) -> Result<Outcome, Failure> {
        if !self.runs(app) {
            return Err(Failure {
                trouble: Trouble::TooOld,
                said: format!(
                    "{} {} needs Cobalt {} or newer, and this reader has {}. Update the \
                     reader first: kobo setup",
                    app.title, app.version, app.minimum_cobalt_version, self.version
                ),
            });
        }
        let before = app.installed_version.clone();
        let mut damaged = false;
        if let Some(installed) = &before {
            if kobod::app_store::resolve_using(&self.root, &app.id, key).is_ok() {
                if *installed == app.version {
                    return Ok(Outcome::Current {
                        version: installed.clone(),
                    });
                }
                if !app.has_update() {
                    return Ok(Outcome::Newer {
                        installed: installed.clone(),
                        offered: app.version.clone(),
                    });
                }
            } else {
                damaged = true;
                kobod::app_store::uninstall_using(&self.root, &app.id, key).map_err(|error| {
                    self.explain(error, &format!("remove the damaged copy of {}", app.title))
                })?;
            }
        }
        kobod::app_store::install_onto_using(
            &self.root,
            &app.id,
            self.channel,
            key,
            &self.version,
            fetch,
        )
        .map_err(|error| self.explain(error, &format!("install {}", app.title)))?;
        // Read back rather than assumed: the answer the owner gets is what
        // the volume now verifies as, not what was meant to be written.
        let after = self
            .installed(key)?
            .into_iter()
            .find(|installed| installed.id == app.id)
            .and_then(|installed| installed.installed_version);
        if after.as_deref() != Some(app.version.as_str()) {
            return Err(Failure {
                trouble: Trouble::Write,
                said: format!(
                    "{} was written to the reader but does not read back as version {}. Check \
                     that the reader is still connected and try again.",
                    app.title, app.version
                ),
            });
        }
        self.tidy();
        Ok(match before {
            Some(_) if damaged => Outcome::Repaired {
                version: app.version.clone(),
            },
            Some(from) => Outcome::Updated {
                from,
                to: app.version.clone(),
            },
            None => Outcome::Installed {
                version: app.version.clone(),
            },
        })
    }

    /// Removes one app. Its data lives outside the package and is kept.
    ///
    /// # Errors
    ///
    /// When the app is part of Cobalt, is not installed, or the volume
    /// cannot be written.
    pub fn remove(&self, id: &str, key: &Ed25519PublicKey) -> Result<(), Failure> {
        kobod::app_store::uninstall_using(&self.root, id, key).map_err(|error| match error {
            DeviceError::NotFound => Failure {
                trouble: Trouble::Missing,
                said: format!("{id} is not installed on this reader."),
            },
            DeviceError::InvalidInput => Failure::other(format!(
                "{id} is part of Cobalt and cannot be removed on its own."
            )),
            other => self.explain(other, &format!("remove {id}")),
        })?;
        self.tidy();
        Ok(())
    }

    /// Clears the sidecar files a Mac leaves in the two folders these verbs
    /// write to. Never an error: the change it follows is already complete
    /// and verified, and a sidecar left behind costs the reader a few
    /// kilobytes.
    fn tidy(&self) {
        for folder in ["apps", "store"] {
            sweep_sidecars(&self.root.join(folder), SIDECAR_DEPTH);
            // The folder's own sidecar sits beside it, one level up, among
            // files this module has no business sweeping. It is taken by
            // name.
            let beside = self.root.join(format!("._{folder}"));
            if begins_with(&beside, APPLE_DOUBLE) {
                let _ = fs::remove_file(beside);
            }
        }
    }

    /// Says what went wrong in terms of a cable and a catalog.
    ///
    /// The runtime's errors are worded for the reader's own panel, where
    /// "the reader could not complete the request" is accurate. Here the
    /// same failure is this computer unable to write to a drive, and telling
    /// an owner their reader failed sends them to the wrong device.
    fn explain(&self, error: DeviceError, doing: &str) -> Failure {
        let (trouble, why) = match error {
            DeviceError::Unreachable | DeviceError::TimedOut => (
                Trouble::Unreachable,
                "the app catalog could not be reached. Check this computer's internet \
                 connection and try again."
                    .to_owned(),
            ),
            DeviceError::Integrity => (
                Trouble::Integrity,
                "what was downloaded, or what is on the reader, does not match its published \
                 signature and digest. Nothing was changed. Try again; if it keeps happening, \
                 report it."
                    .to_owned(),
            ),
            DeviceError::NotFound => (
                Trouble::Missing,
                format!(
                    "it is not where the {} catalog is published. Try again later; if it keeps \
                     happening, report it.",
                    self.channel_name()
                ),
            ),
            DeviceError::InvalidInput => (
                Trouble::Other,
                "the catalog or package was not something this reader can use.".to_owned(),
            ),
            DeviceError::Backend | DeviceError::Authentication => (
                Trouble::Write,
                format!(
                    "the reader at {} could not be written to. Check that it is still \
                     connected and showing Connected, and that it has free space.",
                    self.volume.display()
                ),
            ),
        };
        Failure {
            trouble,
            said: format!("Could not {doing}: {why}"),
        }
    }

    /// How many bytes each app the reader's cached catalog offers takes up
    /// once installed, by id. Empty when no catalog has been fetched for
    /// this reader yet, which only means nothing can be said about sizes.
    #[must_use]
    pub fn sizes(&self, key: &Ed25519PublicKey) -> std::collections::BTreeMap<String, u64> {
        kobod::app_store::catalog_sizes_using(&self.root, self.channel, key)
            .map(|sizes| sizes.into_iter().collect())
            .unwrap_or_default()
    }
}

/// How an `AppleDouble` file begins.
const APPLE_DOUBLE: [u8; 4] = [0x00, 0x05, 0x16, 0x07];

/// How far below `apps` and `store` the sweep looks. The deepest thing
/// either holds is `apps/<id>/bin/kobo-<id>`, three folders down, so a
/// fourth level would only ever be somebody else's files.
const SIDECAR_DEPTH: u8 = 3;

/// Removes the `._name` files macOS writes beside `name` on a FAT volume.
///
/// FAT has nowhere to keep an extended attribute, so macOS keeps a file's
/// attributes in a second file next to it. On a Mac that tags what a program
/// writes with where the program came from, that is one four-kilobyte file
/// for every file and folder an install creates, measured on a FAT volume
/// under macOS 27. The reader ignores them, since none is a valid app id or
/// a name the runtime opens, but they are litter on somebody's reader and
/// they outlive the app they were written beside.
///
/// Only a file that begins with the `AppleDouble` signature is removed, so a
/// file that merely has the name is left alone. It runs on every operating
/// system because a reader set up on a Mac can be plugged into anything
/// afterwards, and because checking four bytes of a few files is cheaper
/// than explaining why the sweep only happens on one of them.
fn sweep_sidecars(folder: &Path, depth: u8) {
    let Ok(entries) = fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if depth > 0 {
                sweep_sidecars(&path, depth - 1);
            }
        } else if kind.is_file()
            && entry.file_name().to_string_lossy().starts_with("._")
            && begins_with(&path, APPLE_DOUBLE)
        {
            let _ = fs::remove_file(&path);
        }
    }
}

/// Whether the file at `path` starts with `signature`.
fn begins_with(path: &Path, signature: [u8; 4]) -> bool {
    use std::io::Read as _;
    let mut start = [0_u8; 4];
    fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut start))
        .is_ok()
        && start == signature
}

/// The channel named in the runtime's update preferences.
///
/// The format is the runtime's, one `name=value` line per choice, kept in
/// `state/auto-update` and read here rather than through the runtime because
/// that module belongs to the device binary. Like the runtime, this ignores
/// what it does not recognise and treats a missing choice as the default, so
/// a file written by a newer Cobalt still reads.
fn channel_in(preferences: &str) -> UpdateChannel {
    let mut channel = UpdateChannel::Stable;
    for line in preferences.lines() {
        match line.trim() {
            "channel=stable" => channel = UpdateChannel::Stable,
            "channel=beta" => channel = UpdateChannel::Beta,
            _ => {}
        }
    }
    channel
}

/// What an install did.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Outcome {
    /// The app was not on the reader and now is.
    Installed { version: String },
    /// An older version was replaced.
    Updated { from: String, to: String },
    /// The copy on the reader did not verify, and was replaced.
    Repaired { version: String },
    /// The catalog's version was already there and verifies. Nothing was
    /// downloaded.
    Current { version: String },
    /// The reader has a later version than the catalog offers, which
    /// happens when a reader moves from the Beta channel back to Stable.
    /// Left alone, as the reader's own Store leaves it.
    Newer { installed: String, offered: String },
}

impl Outcome {
    /// The outcome as one word a script can match.
    #[must_use]
    pub const fn word(&self) -> &'static str {
        match self {
            Self::Installed { .. } => "installed",
            Self::Updated { .. } => "updated",
            Self::Repaired { .. } => "repaired",
            Self::Current { .. } => "current",
            Self::Newer { .. } => "newer",
        }
    }

    /// Whether anything was written to the reader.
    #[must_use]
    pub const fn changed(&self) -> bool {
        matches!(
            self,
            Self::Installed { .. } | Self::Updated { .. } | Self::Repaired { .. }
        )
    }

    /// The outcome as a sentence for the owner.
    #[must_use]
    pub fn sentence(&self, title: &str) -> String {
        match self {
            Self::Installed { version } => format!("Installed {title} {version}."),
            Self::Updated { from, to } => format!("Updated {title} from {from} to {to}."),
            Self::Repaired { version } => format!(
                "{title} on the reader did not verify, so it was removed and {version} was \
                 installed in its place."
            ),
            Self::Current { version } => format!("{title} {version} is already on the reader."),
            Self::Newer { installed, offered } => format!(
                "{title} {installed} on the reader is newer than the {offered} the catalog \
                 offers, so it was left as it is."
            ),
        }
    }
}

/// The key every published catalog and package is signed with.
///
/// # Errors
///
/// Only if the key compiled into this build is malformed.
pub fn release_key() -> Result<Ed25519PublicKey, String> {
    Ed25519PublicKey::from_hex(kobo_app_store::PUBLIC_RELEASE_KEY_HEX)
        .map_err(|error| format!("read the built-in app signing key: {error}"))
}

/// Fetches from where the catalog and its packages are published.
///
/// The failures are folded into the runtime's own error type because the
/// transaction decides on them: a catalog that is not found sends it to the
/// earlier catalog file, and nothing else does.
///
/// # Errors
///
/// When the address cannot be reached, answers with a refusal, or sends more
/// than `maximum` bytes.
pub fn published(url: &str, maximum: u32) -> Result<Vec<u8>, DeviceError> {
    use kobo_protocol::TaskError;
    kobo_net::fetch(url, maximum).map_err(|error| match error {
        TaskError::Offline | TaskError::Unreachable | TaskError::RateLimited(_) => {
            DeviceError::Unreachable
        }
        TaskError::TimedOut => DeviceError::TimedOut,
        TaskError::NotFound => DeviceError::NotFound,
        TaskError::TooLarge | TaskError::Denied => DeviceError::InvalidInput,
        TaskError::NoCredential | TaskError::Unauthorized => DeviceError::Authentication,
    })
}

/// The mounted reader a command should act on: the one named, or the only
/// one there is.
///
/// # Errors
///
/// When no reader is mounted, more than one is and none was named, or the
/// named path is not a reader.
pub fn reader(volume: Option<&Path>) -> Result<setup::Mounted, String> {
    // The chooser's advice ends by naming `kobo setup --volume`, which is the
    // right command there and the wrong one to hand somebody who asked about
    // apps.
    crate::chosen_reader(volume).map_err(|error| {
        console::target(error.replace("kobo setup --volume", "kobo apps --volume"))
    })
}

/// The app an owner means by `name`, among the ones a listing holds.
///
/// The id first, because it is exact, then the name and the short label as
/// the Store shows them with case, spaces and punctuation ignored.
///
/// # Errors
///
/// When nothing matches, with the closest names there are, or when a name
/// belongs to more than one app.
pub fn find<'a>(apps: &'a [AppInfo], name: &str) -> Result<&'a AppInfo, String> {
    let wanted = super::squeeze(name);
    let tiers: [&dyn Fn(&AppInfo) -> bool; 3] = [
        &|app| app.id == name || super::squeeze(&app.id) == wanted,
        &|app| super::squeeze(&app.title) == wanted,
        &|app| super::squeeze(&app.label) == wanted,
    ];
    for matches in tiers {
        let found: Vec<&AppInfo> = apps.iter().filter(|app| matches(app)).collect();
        match found.as_slice() {
            [] => {}
            [only] => return Ok(only),
            several => {
                return Err(format!(
                    "{name:?} is the name of more than one app: {}. Name one by its id.",
                    several
                        .iter()
                        .map(|app| format!("{} ({})", super::plain(&app.title), app.id))
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            }
        }
    }
    let near: Vec<String> = apps
        .iter()
        .filter(|app| {
            !wanted.is_empty()
                && [&app.id, &app.title, &app.label].into_iter().any(|known| {
                    let known = super::squeeze(known);
                    (wanted.len() >= 3 && (known.contains(&wanted) || wanted.contains(&known)))
                        || super::distance(&known, &wanted) <= 2
                })
        })
        .take(3)
        .map(|app| format!("{} ({})", super::plain(&app.title), app.id))
        .collect();
    if near.is_empty() {
        Err(format!("No app is called {name:?}."))
    } else {
        Err(format!(
            "No app is called {name:?}. Did you mean {}?",
            near.join(" or ")
        ))
    }
}

/// One app as the JSON output and the companion window describe it.
#[must_use]
pub fn app_json(installation: &Installation, app: &AppInfo) -> Value {
    json!({
        "id": app.id,
        "title": super::plain(&app.title),
        "label": super::plain(&app.label),
        "summary": super::plain(&app.summary),
        "version": app.version,
        "installed_version": app.installed_version,
        "update_available": app.has_update(),
        "minimum_cobalt_version": app.minimum_cobalt_version,
        "runs_on_this_reader": installation.runs(app),
        "capabilities": app.capabilities,
    })
}

/// The reader an answer is about, for the same two consumers.
#[must_use]
pub fn installation_json(installation: &Installation) -> Value {
    json!({
        "volume": installation.volume.display().to_string(),
        "cobalt_version": installation.version,
        "channel": installation.channel_name(),
    })
}

/// How a command line asked for one of these verbs.
#[derive(Debug, Default, Eq, PartialEq)]
struct Options {
    volume: Option<PathBuf>,
    json: bool,
    eject: bool,
    names: Vec<String>,
}

fn parse(verb: &str, arguments: &[String]) -> Result<Options, String> {
    let mut options = Options::default();
    let mut words = arguments.iter();
    while let Some(word) = words.next() {
        match word.as_str() {
            "--volume" | "-v" => {
                let path = words
                    .next()
                    .ok_or_else(|| console::usage("--volume needs the path to a mounted reader"))?;
                options.volume = Some(PathBuf::from(path));
            }
            "--json" => options.json = true,
            "--eject" => options.eject = true,
            flag if flag.starts_with("--") => {
                return Err(console::usage(format!(
                    "kobo apps {verb} [--volume PATH] [--eject] [--json]\nunknown option '{flag}'"
                )));
            }
            name => options.names.push(name.to_owned()),
        }
    }
    let wanted = match verb {
        "install" if options.names.is_empty() => Some("kobo apps install APP..."),
        "remove" if options.names.is_empty() => Some("kobo apps remove APP"),
        "installed" | "available" if !options.names.is_empty() => {
            Some("kobo apps installed | available")
        }
        _ => None,
    };
    if let Some(usage) = wanted {
        return Err(console::usage(format!(
            "{usage} [--volume PATH] [--eject] [--json]"
        )));
    }
    Ok(options)
}

/// `kobo apps installed`, `available`, `install` and `remove`.
///
/// # Errors
///
/// When no reader with Cobalt on it is mounted, or the verb itself fails.
pub fn command(verb: &str, arguments: &[String]) -> Result<(), String> {
    let options = parse(verb, arguments)?;
    let reader = reader(options.volume.as_deref())?;
    let installation = Installation::on(&reader.volume)?;
    let key = release_key()?;
    let result = run(verb, &options, &installation, &key, published, setup::eject)?;
    if options.json {
        Console::print_json(&format!("apps {verb}"), &result.data);
    } else {
        print!("{}", result.text);
    }
    // Each failure has already been said in full beside the app it belongs
    // to. This is only the status: a script installing five apps must not
    // read success because the last one worked.
    match (result.failed, options.names.len()) {
        (0, _) => Ok(()),
        (_, 1) => Err("the app was not installed".to_owned()),
        (failed, asked) => Err(format!("{failed} of {asked} apps were not installed")),
    }
}

/// What a verb produced: the text a person reads, the object a script
/// reads, how many of the apps asked for failed, and whether anything on
/// the reader was changed.
#[derive(Debug)]
struct Answer {
    text: String,
    data: Value,
    failed: usize,
    changed: bool,
}

/// One verb against one installation, with the network and the eject
/// supplied, so the whole of it runs in a test against a folder and a
/// fixture catalog.
fn run(
    verb: &str,
    options: &Options,
    installation: &Installation,
    key: &Ed25519PublicKey,
    fetch: impl FnMut(&str, u32) -> Result<Vec<u8>, DeviceError>,
    eject: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<Answer, String> {
    let mut answer = Answer {
        text: String::new(),
        data: json!({ "reader": installation_json(installation) }),
        failed: 0,
        changed: false,
    };
    match verb {
        "installed" => list_installed(&mut answer, installation, key)?,
        "available" => list_available(&mut answer, installation, key, fetch)?,
        "install" => install_named(&mut answer, &options.names, installation, key, fetch)?,
        // Joined, so `kobo apps remove music stand` works unquoted, as
        // `kobo apps setup` does. Remove takes one app, so there is no
        // second name for the words to be confused with.
        "remove" => remove_named(&mut answer, &options.names.join(" "), installation, key)?,
        other => return Err(console::usage(format!("kobo apps has no verb '{other}'"))),
    }
    let ejected = if options.eject {
        match eject(&installation.volume) {
            Ok(()) => {
                answer
                    .text
                    .push_str("The reader is ejected and can be unplugged.\n");
                true
            }
            Err(error) => {
                let _ = writeln!(answer.text, "The reader was not ejected: {error}");
                false
            }
        }
    } else {
        if answer.changed {
            answer.text.push_str(
                "Eject the reader before you unplug it, so the change is fully written.\n",
            );
        }
        false
    };
    answer.data["ejected"] = Value::Bool(ejected);
    Ok(answer)
}

fn list_installed(
    answer: &mut Answer,
    installation: &Installation,
    key: &Ed25519PublicKey,
) -> Result<(), String> {
    let apps = installation.installed(key)?;
    let _ = writeln!(
        answer.text,
        "On the reader at {} (Cobalt {}):\n",
        installation.volume.display(),
        installation.version
    );
    if apps.is_empty() {
        answer.text.push_str("No apps are installed.\n");
    }
    for app in &apps {
        let _ = writeln!(
            answer.text,
            "  {} ({}) {}",
            super::plain(&app.title),
            app.id,
            app.installed_version.as_deref().unwrap_or(&app.version)
        );
    }
    answer
        .text
        .push_str("\nUpdates and more apps: kobo apps available\n");
    answer.data["apps"] = apps.iter().map(|app| app_json(installation, app)).collect();
    Ok(())
}

fn list_available(
    answer: &mut Answer,
    installation: &Installation,
    key: &Ed25519PublicKey,
    fetch: impl FnMut(&str, u32) -> Result<Vec<u8>, DeviceError>,
) -> Result<(), String> {
    let apps = installation.available(key, fetch)?;
    let _ = writeln!(
        answer.text,
        "Published for the reader at {} (Cobalt {}, {} catalog):\n",
        installation.volume.display(),
        installation.version,
        installation.channel_name()
    );
    for app in &apps {
        let _ = writeln!(answer.text, "  {}", available_line(installation, app));
        let _ = writeln!(answer.text, "    {}", super::plain(&app.summary));
    }
    answer
        .text
        .push_str("\nInstall or update one: kobo apps install APP\n");
    answer.data["apps"] = apps.iter().map(|app| app_json(installation, app)).collect();
    Ok(())
}

/// Installs each named app in turn. One that fails is said and counted and
/// the rest still run, because the apps are independent and an owner who
/// asked for five should not get none over a typing mistake in the third.
fn install_named(
    answer: &mut Answer,
    names: &[String],
    installation: &Installation,
    key: &Ed25519PublicKey,
    mut fetch: impl FnMut(&str, u32) -> Result<Vec<u8>, DeviceError>,
) -> Result<(), String> {
    let apps = installation.available(key, &mut fetch)?;
    let mut results = Vec::new();
    for name in names {
        let outcome = find(&apps, name).and_then(|app| {
            Console::progress(&format!(
                "Checking {} {}...",
                super::plain(&app.title),
                app.version
            ));
            installation
                .install(app, key, &mut fetch)
                .map(|outcome| (app, outcome))
                .map_err(String::from)
        });
        match outcome {
            Ok((app, outcome)) => {
                answer.changed |= outcome.changed();
                let title = super::plain(&app.title);
                let _ = writeln!(answer.text, "{}", outcome.sentence(&title));
                results.push(json!({
                    "id": app.id,
                    "title": title,
                    "outcome": outcome.word(),
                    "version": app.version,
                    "previous_version": app.installed_version,
                }));
            }
            Err(error) => {
                answer.failed += 1;
                let _ = writeln!(answer.text, "{}", console::display(&error));
                results.push(json!({
                    "name": name,
                    "outcome": "failed",
                    "error": console::display(&error),
                }));
            }
        }
    }
    answer.data["results"] = Value::Array(results);
    Ok(())
}

fn remove_named(
    answer: &mut Answer,
    name: &str,
    installation: &Installation,
    key: &Ed25519PublicKey,
) -> Result<(), String> {
    let apps = installation.installed(key)?;
    let app = find(&apps, name)
        .map_err(|error| format!("{error} kobo apps installed lists what is on the reader."))?;
    installation.remove(&app.id, key)?;
    answer.changed = true;
    let title = super::plain(&app.title);
    let _ = writeln!(
        answer.text,
        "Removed {title} from the reader. Its data is kept, so installing it again picks up \
         where it left off."
    );
    answer.data["removed"] = json!({ "id": app.id, "title": title });
    Ok(())
}

/// One catalog entry as `kobo apps available` lists it.
fn available_line(installation: &Installation, app: &AppInfo) -> String {
    let mut line = format!("{} ({}) {}", super::plain(&app.title), app.id, app.version);
    match &app.installed_version {
        Some(installed) if app.has_update() => {
            let _ = write!(line, ", update from {installed}");
        }
        Some(installed) if *installed == app.version => line.push_str(", installed"),
        Some(installed) => {
            let _ = write!(line, ", reader has {installed}");
        }
        None => {}
    }
    if !installation.runs(app) {
        let _ = write!(
            line,
            ", needs Cobalt {} or newer",
            app.minimum_cobalt_version
        );
    }
    line
}

#[cfg(test)]
#[path = "apps_usb_tests.rs"]
mod tests;

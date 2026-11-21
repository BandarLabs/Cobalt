//! The apps as a shop shows them: what each one is, which group it is listed
//! under, and its pictures.
//!
//! A list of names with an Install button asks an owner to choose between
//! apps they have never seen. The website shows each one, and this is the
//! same showing for the window. Almost all of it is built into this command:
//! every app's own manifest is, and so is the short listing of groups and
//! picture captions the website is generated from, which is all a store
//! needs on a computer with no network. Only the pictures themselves are
//! not, because together they are several times the size of this command.
//! They are fetched from the published site when somebody looks at them and
//! kept on disk afterwards.
//!
//! Fetching is this process's job and never the front end's. The page is
//! forbidden by its own headers from loading anything but this server, and
//! that rule is worth more than a picture: it is what makes "this page asks
//! nobody else anything" checkable. So a front end asks here for a picture
//! by the name the listing gave it, and this process is the only thing that
//! ever names the published site.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use kobo_protocol::DeviceError;
use serde_json::{json, Value};

use crate::apps::{capability, plain};

/// Groups, pictures and captions, as `tools/store-listing.mjs` wrote them
/// from the tables the website is drawn from.
const LISTING: &str = include_str!("store-listing.json");

/// Where the pictures are published. A picture's address is this followed
/// by its listed name and nothing else, so no request can be pointed
/// anywhere but under here.
const PUBLISHED: &str = "https://bandarlabs.github.io/Cobalt/media/site/apps/";

/// The most a picture may weigh.
///
/// The heaviest published today is under a megabyte. The bound is what a
/// kept picture can cost the disk and what a wrong answer from the network
/// can cost memory, and a picture over it is treated as not a picture.
pub const HEAVIEST: u32 = 2 * 1024 * 1024;

/// The longest side a picture may claim to have, in pixels.
///
/// A reader's screen is 1072 by 1448 and the largest is under 2000 on its
/// long side. A file that claims far more is refused before a front end is
/// handed it, because a small file can declare an image that takes
/// gigabytes to decode and it is the front end that would pay for that.
const LONGEST_SIDE: u32 = 4096;

/// How long a kept picture is shown before the site is asked for it again.
///
/// An app retakes its pictures a few times a year, so a week-old copy is
/// almost always the current one, and asking again is only a way to notice
/// that it is not. When the site cannot be reached the old copy is still
/// shown, however old it is.
const FRESH: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// How many pictures are fetched at once.
///
/// A front end asks for a screenful of them together. Each fetch is a
/// thread of this server held for as long as the network takes, and the
/// server only has so many, so the rest wait their turn here instead of
/// crowding out the requests that say whether a reader is plugged in.
const AT_ONCE: usize = 3;

/// How long nothing is fetched after the site could not be reached, and how
/// long a picture the site did not have is not asked for again.
///
/// Without it a store opened on a train asks for forty pictures and waits
/// for forty failures, one after another. With it the first failure answers
/// for the rest at once, and the site is tried again the next time a
/// picture is looked at after this has passed.
const REST: Duration = Duration::from_secs(30);

/// How long one picture is waited for before it is given up on.
///
/// The transport underneath waits minutes for a download that has gone
/// quiet, which suits an app package and not a thumbnail somebody is
/// looking at an empty frame for. A picture that is this slow is shown as
/// missing and fetched again the next time it is wanted.
const PATIENCE: Duration = Duration::from_secs(15);

/// What `GET /api/store` answers with, and the pictures it names.
pub struct Shopfront {
    answer: Value,
    pictures: BTreeSet<String>,
}

impl Shopfront {
    /// The store this command was built with.
    ///
    /// # Errors
    ///
    /// When a bundled manifest or the listing does not read, which the
    /// tests of both rule out for anything that is released.
    pub fn bundled() -> Result<Self, String> {
        Self::from(kobo_catalog::sources(), LISTING)
    }

    /// A store of `manifests`, grouped and pictured as `listing` says.
    ///
    /// An app the listing has no group for still appears, under a last
    /// group of its own, and one with no pictures has none. Leaving such an
    /// app out would hide something that can be installed because a table
    /// elsewhere had not caught up with it.
    ///
    /// # Errors
    ///
    /// When either does not read, or the listing names a picture in a way
    /// that could be an address of its own.
    pub fn from(manifests: &[&str], listing: &str) -> Result<Self, String> {
        let listing: Value = serde_json::from_str(listing)
            .map_err(|error| format!("the store listing does not read: {error}"))?;
        let mut pictures = BTreeSet::new();
        let mut apps = BTreeMap::new();
        for source in manifests {
            let manifest: Value = serde_json::from_str(source)
                .map_err(|error| format!("an app manifest does not read: {error}"))?;
            let app = described(&manifest, &listing, &mut pictures)?;
            let id = app["id"].as_str().unwrap_or_default().to_owned();
            apps.insert(id, app);
        }

        let title_of = |id: &str| {
            apps.get(id)
                .and_then(|app| app["title"].as_str())
                .unwrap_or(id)
                .to_lowercase()
        };
        let mut grouped = BTreeSet::new();
        let mut groups = Vec::new();
        for group in listing["groups"].as_array().into_iter().flatten() {
            let mut ids: Vec<&str> = group["apps"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter(|id| apps.contains_key(*id) && grouped.insert((*id).to_owned()))
                .collect();
            ids.sort_by_key(|id| (title_of(id), (*id).to_owned()));
            groups.push(json!({ "id": group["id"], "title": group["title"], "apps": ids }));
        }
        let mut others: Vec<&str> = apps
            .keys()
            .map(String::as_str)
            .filter(|id| !grouped.contains(*id))
            .collect();
        if !others.is_empty() {
            others.sort_by_key(|id| (title_of(id), (*id).to_owned()));
            groups.push(json!({ "id": "other", "title": "Other apps", "apps": others }));
        }
        Ok(Self {
            answer: json!({ "groups": groups, "apps": apps }),
            pictures,
        })
    }

    /// The whole store, as a front end is sent it.
    #[must_use]
    pub const fn answer(&self) -> &Value {
        &self.answer
    }

    /// Whether the store lists a picture by exactly this name.
    #[must_use]
    pub fn shows(&self, name: &str) -> bool {
        self.pictures.contains(name)
    }

    /// Every picture the store lists.
    #[must_use]
    pub const fn pictures(&self) -> &BTreeSet<String> {
        &self.pictures
    }
}

/// One app as the store describes it, from its manifest and the listing.
fn described(
    manifest: &Value,
    listing: &Value,
    pictures: &mut BTreeSet<String>,
) -> Result<Value, String> {
    let said = |field: &str| manifest[field].as_str().map(plain);
    let (Some(id), Some(title), Some(summary), Some(version)) = (
        said("id"),
        said("display_name"),
        said("summary"),
        said("version"),
    ) else {
        return Err("an app manifest is missing its id, name, summary or version".to_owned());
    };
    // The website hides or rewords the note of a version whose note was
    // about the repository, not the app, and the same table is honoured
    // here so the two never disagree about what changed.
    let notes = match listing["release_note_overrides"].get(format!("{id}@{version}")) {
        Some(instead) => instead.as_str().map(plain),
        None => said("release_notes"),
    };
    let access: Vec<String> = manifest["capabilities"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(capability)
        .collect();
    let setup: Vec<Value> = manifest["setup"]["steps"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|step| {
            let link = step["link"]["url"]
                .as_str()
                .filter(|url| url.starts_with("https://") || url.starts_with("http://"))
                .map(|url| {
                    json!({
                        "label": step["link"]["label"].as_str().map_or_else(|| plain(url), plain),
                        "url": plain(url),
                    })
                });
            json!({
                "text": step["text"].as_str().map(plain).unwrap_or_default(),
                "command": step["command"].as_str().map(plain),
                "link": link,
            })
        })
        .collect();
    let mut shots = Vec::new();
    for shot in listing["shots"][&id].as_array().into_iter().flatten() {
        let name = shot["name"].as_str().unwrap_or_default();
        if !is_a_picture_name(name) {
            return Err(format!(
                "the store listing names a picture this would not fetch: {name}"
            ));
        }
        pictures.insert(name.to_owned());
        shots.push(json!({
            "name": name,
            "caption": shot["caption"].as_str().map(plain).unwrap_or_default(),
            "width": shot["width"].as_u64().unwrap_or_default(),
            "height": shot["height"].as_u64().unwrap_or_default(),
        }));
    }
    Ok(json!({
        "id": id,
        "title": title,
        "summary": summary,
        "description": said("page_description").unwrap_or_else(|| summary.clone()),
        "version": version,
        "release_notes": notes,
        "access": access,
        "setup": setup,
        "shots": shots,
    }))
}

/// Whether `name` is shaped like a picture the site publishes: a file, or a
/// file in one folder, in lowercase letters, digits and hyphens, ending
/// `.png`.
///
/// The name becomes the end of an address and the name of a file on disk.
/// Every name comes from a listing built into this command, so this is not
/// what keeps a stranger out; it is what keeps a mistake in that listing
/// from being a path with `..` in it.
fn is_a_picture_name(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".png") else {
        return false;
    };
    let mut parts = stem.split('/');
    let word = |part: Option<&str>| {
        part.is_some_and(|part| {
            part.starts_with(|first: char| first.is_ascii_lowercase() || first.is_ascii_digit())
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
    };
    let first = word(parts.next());
    let second = parts.next();
    first && (second.is_none() || word(second)) && parts.next().is_none()
}

/// Whether `bytes` open as a PNG of a size a screen could be.
///
/// Only the first few bytes are read: the signature every PNG starts with
/// and the header that follows it, which states the size. That is enough to
/// turn away an error page, a file cut short at its start and an image that
/// claims to be enormous. Whether the rest decodes is the front end's to
/// find out, and a picture that does not is drawn as a missing one.
fn is_a_picture(bytes: &[u8]) -> bool {
    const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
    let side = |at: usize| {
        bytes
            .get(at..at + 4)
            .and_then(|side| side.try_into().ok())
            .map(u32::from_be_bytes)
            .is_some_and(|side| (1..=LONGEST_SIDE).contains(&side))
    };
    bytes.starts_with(SIGNATURE) && bytes.get(12..16) == Some(b"IHDR") && side(16) && side(20)
}

/// How one picture's bytes are fetched from an address, up to a size.
pub type Fetch = dyn Fn(&str, u32) -> Result<Vec<u8>, DeviceError> + Send + Sync;

/// The pictures kept on this computer, and the fetching of the rest.
pub struct Pictures {
    folder: PathBuf,
    outward: Mutex<Outward>,
    turn: Condvar,
    /// [`FRESH`], [`REST`] and [`PATIENCE`], held here so that a test of
    /// what happens when one runs out does not have to wait for it.
    fresh: Duration,
    rest: Duration,
    patience: Duration,
}

/// What is known about the site from the fetches so far.
#[derive(Default)]
struct Outward {
    /// Fetches under way.
    going: usize,
    /// When the site last could not be reached.
    unreachable: Option<Instant>,
    /// Pictures the site answered for and did not have, and when.
    refused: BTreeMap<String, Instant>,
}

/// Why a fetch gave nothing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Lacking {
    /// The site was not reached.
    Unreachable,
    /// The site answered, and not with this picture.
    Refused,
}

impl Pictures {
    /// The pictures kept in `folder`, which will hold only ones in
    /// `listed`.
    ///
    /// Anything else found there is deleted: a picture an earlier release
    /// listed and this one does not, or a download that was cut off. That
    /// is what bounds the folder, at one file of at most [`HEAVIEST`] for
    /// each listed picture, however many releases have filled it. A folder
    /// that cannot be made is not an error, because the store is the same
    /// without it, only slower.
    #[must_use]
    pub fn open(folder: PathBuf, listed: &BTreeSet<String>) -> Self {
        let _ = fs::create_dir_all(&folder);
        let wanted: BTreeSet<String> = listed.iter().map(|name| kept_name(name)).collect();
        for entry in fs::read_dir(&folder).into_iter().flatten().flatten() {
            let named = entry.file_name();
            let known = named.to_str().is_some_and(|name| wanted.contains(name));
            if !known && entry.file_type().is_ok_and(|kind| kind.is_file()) {
                let _ = fs::remove_file(entry.path());
            }
        }
        Self {
            folder,
            outward: Mutex::default(),
            turn: Condvar::new(),
            fresh: FRESH,
            rest: REST,
            patience: PATIENCE,
        }
    }

    /// The folder pictures are kept in on this computer.
    ///
    /// Under the cache folder the system names, or the one in the home
    /// folder where it names none: these are copies of something published
    /// and can be deleted at any time at the cost of fetching them again.
    /// A name that is empty or not a whole path is the same as none, or
    /// the pictures would be kept in whatever folder the command was run
    /// from.
    #[must_use]
    pub fn folder_here() -> PathBuf {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .filter(|named| named.is_absolute())
            .or_else(|| kobo_abi::home_directory().map(|home| home.join(".cache")))
            .unwrap_or_else(std::env::temp_dir)
            .join("cobalt")
            .join("companion")
            .join("pictures")
    }

    /// One listed picture: the kept copy when it is recent, the published
    /// one when it can be fetched, an old kept copy when it cannot, and
    /// nothing only when there is neither.
    ///
    /// `name` must be one the store lists. It is joined to a folder and to
    /// an address as it is.
    pub fn get(&self, name: &str, fetch: &Fetch) -> Option<Vec<u8>> {
        let path = self.folder.join(kept_name(name));
        if let Some(picture) = kept(&path, Some(self.fresh)) {
            return Some(picture);
        }
        if let Some(turn) = self.take_turn(name) {
            // Two frames showing the same picture ask for it together, and
            // the one that waited finds it already here.
            let fetched = kept(&path, Some(self.fresh)).map_or_else(
                || {
                    let picture = fetch(&format!("{PUBLISHED}{name}"), HEAVIEST).map_err(
                        |error| match error {
                            DeviceError::NotFound | DeviceError::InvalidInput => Lacking::Refused,
                            _ => Lacking::Unreachable,
                        },
                    )?;
                    if !is_a_picture(&picture) {
                        return Err(Lacking::Refused);
                    }
                    keep(&path, &picture);
                    Ok(picture)
                },
                Ok,
            );
            turn.finish(name, fetched.as_ref().err().copied());
            if let Ok(picture) = fetched {
                return Some(picture);
            }
        }
        kept(&path, None)
    }

    /// Waits for one of the [`AT_ONCE`] turns at fetching. `None` when the
    /// site is being left alone for now, or no turn came in the time one
    /// fetch is allowed.
    fn take_turn(&self, name: &str) -> Option<Turn<'_>> {
        let resting = |outward: &Outward| {
            let recent = |at: &Instant| at.elapsed() < self.rest;
            outward.unreachable.as_ref().is_some_and(recent)
                || outward.refused.get(name).is_some_and(recent)
        };
        let outward = self.outward();
        if resting(&outward) {
            return None;
        }
        let (mut outward, waited) = self
            .turn
            .wait_timeout_while(outward, self.patience, |outward| {
                outward.going >= AT_ONCE && !resting(outward)
            })
            .unwrap_or_else(PoisonError::into_inner);
        if waited.timed_out() || resting(&outward) {
            return None;
        }
        outward.going += 1;
        Some(Turn {
            pictures: self,
            finished: false,
        })
    }

    fn outward(&self) -> MutexGuard<'_, Outward> {
        self.outward.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// One of the turns at fetching, given back when it is dropped.
///
/// Given back in `drop` and not only by [`Turn::finish`], so that a fetch
/// that panics does not take its turn with it and leave the store one
/// short for the rest of the session.
struct Turn<'a> {
    pictures: &'a Pictures,
    finished: bool,
}

impl Turn<'_> {
    /// Gives the turn back and records how the fetch for `name` went.
    fn finish(mut self, name: &str, lacking: Option<Lacking>) {
        let mut outward = self.pictures.outward();
        outward.going = outward.going.saturating_sub(1);
        match lacking {
            None => {
                outward.unreachable = None;
                outward.refused.remove(name);
            }
            Some(Lacking::Unreachable) => outward.unreachable = Some(Instant::now()),
            Some(Lacking::Refused) => {
                outward.unreachable = None;
                outward.refused.insert(name.to_owned(), Instant::now());
            }
        }
        drop(outward);
        self.finished = true;
        // All of them, because the ones waiting may now have nothing to
        // wait for.
        self.pictures.turn.notify_all();
    }
}

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        if !self.finished {
            let mut outward = self.pictures.outward();
            outward.going = outward.going.saturating_sub(1);
            drop(outward);
            self.pictures.turn.notify_all();
        }
    }
}

/// The name a picture is kept under. A listed name has no underscore in
/// it, so no two of them are kept under the same one.
fn kept_name(name: &str) -> String {
    name.replace('/', "_")
}

/// The kept copy at `path`, when there is one that is still a picture and,
/// if `within` is given, was fetched no longer ago than that.
fn kept(path: &Path, within: Option<Duration>) -> Option<Vec<u8>> {
    let about = fs::metadata(path).ok()?;
    if !about.is_file() || about.len() > u64::from(HEAVIEST) {
        return None;
    }
    if let Some(within) = within {
        // A copy dated in the future, by a clock that has since been set
        // back, counts as new: the alternative is fetching it every time.
        let age = about.modified().ok()?.elapsed().unwrap_or_default();
        if age > within {
            return None;
        }
    }
    fs::read(path).ok().filter(|bytes| is_a_picture(bytes))
}

/// Keeps a fetched picture, whole or not at all.
///
/// Written beside its place and moved into it, so a copy that is being
/// written is never read as a picture and a full disk leaves the old copy
/// where it was. Failing is not reported: the picture was fetched and is
/// shown either way, and is fetched again next time.
fn keep(path: &Path, picture: &[u8]) {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let part = path.with_extension(format!(
        "part-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    if fs::write(&part, picture).is_err() || fs::rename(&part, path).is_err() {
        let _ = fs::remove_file(&part);
    }
}

/// Fetches a published picture, giving up after [`PATIENCE`].
///
/// # Errors
///
/// When the address cannot be reached in that time, answers with a refusal,
/// or sends more than `most` bytes.
pub fn published(url: &str, most: u32) -> Result<Vec<u8>, DeviceError> {
    use kobo_protocol::TaskError;
    let cancel = AtomicBool::new(false);
    let done = (Mutex::new(false), Condvar::new());
    let fetched = std::thread::scope(|scope| {
        // Watches the clock for the fetch, which has no deadline of its own
        // to give, and is woken early when the fetch ends first.
        scope.spawn(|| {
            let finished = done.0.lock().unwrap_or_else(PoisonError::into_inner);
            let (_finished, waited) = done
                .1
                .wait_timeout_while(finished, PATIENCE, |finished| !*finished)
                .unwrap_or_else(PoisonError::into_inner);
            if waited.timed_out() {
                cancel.store(true, Ordering::SeqCst);
            }
        });
        let fetched = kobo_net::fetch_from_controlled(
            url,
            0,
            most,
            None,
            &[],
            kobo_net::RequestOptions::default(),
            &cancel,
        );
        *done.0.lock().unwrap_or_else(PoisonError::into_inner) = true;
        done.1.notify_all();
        fetched
    });
    fetched.map_err(|error| match error {
        TaskError::NotFound => DeviceError::NotFound,
        TaskError::TooLarge | TaskError::Denied => DeviceError::InvalidInput,
        TaskError::TimedOut => DeviceError::TimedOut,
        _ => DeviceError::Unreachable,
    })
}

#[cfg(test)]
#[path = "shopfront_tests.rs"]
mod tests;

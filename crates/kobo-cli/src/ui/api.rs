//! What the page can ask for, each over something that already existed.
//!
//! Nothing here decides anything about a reader that the command line does
//! not already decide. Questions are answered by calling the functions the
//! commands call: which readers are mounted and what they are, what a setup
//! would change, what the signed catalog offers, which browsers are here.
//! Anything long or that talks as it goes is the command itself, run as a
//! job. The routes exist to put those in front of somebody who will never
//! open a terminal twice, not to be a second implementation of them.
//!
//! Two rules hold for every handler. A path or a name that arrives from the
//! page is never used as it is: a reader is looked up among the ones this
//! process finds for itself, a browser and an app among the ones it knows,
//! a saved reader among the ones in the store. And no handler builds a
//! command line from the page's words, only from its own fixed ones plus
//! values that have been through that lookup.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use kobo_app_store::Ed25519PublicKey;
use kobo_protocol::DeviceError;
use serde_json::{json, Value};

use super::arrival;
use super::http::{Malformed, Response};
use super::jobs::{self, Finish, Kind};
use super::page;
use super::server::{Request, Route, Shared};
use super::shopfront::{self, Pictures, Shopfront};
use super::surroundings::{self, Room};
use crate::apps::usb;
use crate::clip_delivery as delivery;
use crate::clip_install::{self, Browser, Os, Registration};
use crate::detect::{self, Companion};
use crate::{console, readers, setup};

/// Which shape of answers this is.
///
/// Raised when a field a front end relies on changes meaning or goes away,
/// never for an addition. The Mac app is installed beside this command but
/// is a separate program, so the two can be different releases for as long
/// as an update takes; a front end that sees a number it was not written
/// for says so instead of drawing answers it may be misreading.
const API: u32 = 1;

/// The route the page calls as it closes.
pub const GOODBYE: &str = "/api/goodbye";

/// Every route there is.
///
/// A table rather than a `match`, so that a test can walk it and hold each
/// route to the rules for its method without anybody remembering to add the
/// new one to the test.
pub const ROUTES: &[Route] = &[
    Route {
        method: "GET",
        path: "/",
        handle: page::index,
    },
    Route {
        method: "GET",
        path: "/app.css",
        handle: page::style,
    },
    Route {
        method: "GET",
        path: "/app.js",
        handle: page::script,
    },
    Route {
        method: "GET",
        path: "/logic.mjs",
        handle: page::logic,
    },
    Route {
        method: "GET",
        path: "/api/state",
        handle: state,
    },
    Route {
        method: "GET",
        path: "/api/plan",
        handle: plan,
    },
    Route {
        method: "POST",
        path: "/api/reader/install",
        handle: install_cobalt,
    },
    Route {
        method: "POST",
        path: "/api/reader/remove",
        handle: remove_cobalt,
    },
    Route {
        method: "POST",
        path: "/api/reader/eject",
        handle: eject,
    },
    Route {
        method: "POST",
        path: "/api/reader/find",
        handle: find_reader,
    },
    Route {
        method: "POST",
        path: "/api/readers/rename",
        handle: rename_reader,
    },
    Route {
        method: "POST",
        path: "/api/readers/forget",
        handle: forget_reader,
    },
    Route {
        method: "GET",
        path: "/api/apps",
        handle: installed_apps,
    },
    Route {
        method: "POST",
        path: "/api/apps/catalog",
        handle: catalog,
    },
    Route {
        method: "POST",
        path: "/api/apps/install",
        handle: install_apps,
    },
    Route {
        method: "POST",
        path: "/api/apps/remove",
        handle: remove_app,
    },
    Route {
        method: "GET",
        path: "/api/store",
        handle: store,
    },
    Route {
        method: "GET",
        path: "/api/shot",
        handle: shot,
    },
    Route {
        method: "GET",
        path: "/api/browsers",
        handle: browsers,
    },
    Route {
        method: "POST",
        path: "/api/browsers/setup",
        handle: set_up_browser,
    },
    Route {
        method: "POST",
        path: "/api/browsers/remove",
        handle: forget_browser,
    },
    Route {
        method: "GET",
        path: "/api/send",
        handle: sending,
    },
    Route {
        method: "POST",
        path: "/api/send/file",
        handle: receive_file,
    },
    Route {
        method: "POST",
        path: "/api/send/start",
        handle: send_file,
    },
    Route {
        method: "POST",
        path: "/api/send/address",
        handle: send_address,
    },
    Route {
        method: "POST",
        path: "/api/send/queue",
        handle: send_waiting,
    },
    Route {
        method: "GET",
        path: "/api/job",
        handle: job,
    },
    Route {
        method: "POST",
        path: GOODBYE,
        handle: goodbye,
    },
];

/// How catalog and package bytes are fetched.
pub type Transport = Box<dyn Fn(&str, u32) -> Result<Vec<u8>, DeviceError> + Send + Sync>;

/// How a mounted reader is ejected.
pub type Eject = Box<dyn Fn(&Path) -> Result<(), String> + Send + Sync>;

/// How the mounted readers are found.
pub type Readers = Box<dyn Fn() -> Vec<setup::Mounted> + Send + Sync>;

/// How a volume's free space and writability are read.
pub type RoomOf = Box<dyn Fn(&Path) -> Room + Send + Sync>;

/// How the drives the system would not let this process read are found.
pub type Blocked = Box<dyn Fn() -> Vec<PathBuf> + Send + Sync>;

/// How the Kobos on the USB bus are counted, when this system can count.
pub type Attached = Box<dyn Fn() -> Option<usize> + Send + Sync>;

/// How it is told whether this computer is on a network a reader could be
/// looked for on.
pub type Network = Box<dyn Fn() -> bool + Send + Sync>;

/// What the handlers know about the computer they are running on.
///
/// Everything that reaches outside this process is a field here: where
/// readers are looked for, what is run as a child, where the catalog comes
/// from, where this person's files are. The real ones are filled in by
/// [`Machine::here`]; a test fills in folders and a fixture catalog, and the
/// handlers cannot tell the difference.
pub struct Machine {
    /// The program run for the jobs that are commands: this one.
    pub program: PathBuf,
    /// Every reader mounted now.
    pub readers: Readers,
    /// The Cobalt release this computer would install, or why it has none.
    pub release: Result<String, String>,
    /// A release folder named on the command line, passed on to setup.
    pub release_dir: Option<PathBuf>,
    /// A private folder for files on their way to a reader.
    pub scratch: PathBuf,
    pub fetch: Transport,
    /// Every app as the store shows it, with or without a reader.
    pub shopfront: Shopfront,
    /// The pictures of them kept on this computer.
    pub pictures: Pictures,
    /// How a picture that is not kept is fetched.
    pub picture: Box<shopfront::Fetch>,
    pub key: Ed25519PublicKey,
    pub eject: Eject,
    pub room: RoomOf,
    pub blocked: Blocked,
    pub attached: Attached,
    pub network: Network,
    pub home: PathBuf,
    /// The companion's own folder: saved readers, the clip queue, the
    /// extension.
    pub config: PathBuf,
    /// Files received and not yet sent.
    pub uploads: Mutex<Uploads>,
    /// Held while a file is arriving, so that one arrives at a time. Without
    /// it the bound on what received files can cost the disk would be the
    /// number of connections, not [`MOST_UPLOADS`].
    pub receiving: Mutex<()>,
    /// Held across every call into the reader's app store.
    ///
    /// The store finishes an interrupted transaction before it answers
    /// anything, including a listing, so two requests arriving together
    /// could each start repairing the same half-finished swap. One at a
    /// time, they cannot.
    pub store: Mutex<()>,
}

/// A file the page has handed over, waiting to be sent.
#[derive(Debug)]
pub struct Upload {
    id: u64,
    /// The folder made for it, which holds only it.
    folder: PathBuf,
    /// Its name inside that folder, already made safe.
    name: String,
}

/// The files waiting, and the number the next one gets.
///
/// The number only ever goes up. A file's folder is named for it and is
/// still in use while the file is being sent, so a number given out twice
/// would hand a second file the folder a running send is reading from.
#[derive(Debug, Default)]
pub struct Uploads {
    given: u64,
    waiting: Vec<Upload>,
}

/// How many received files may wait at once.
///
/// One is the use: a file is handed over, a question about it is answered,
/// and it is sent. The few extra are for a second window or a changed mind,
/// and past them the oldest is deleted, so a page that hands over files and
/// never sends them costs the disk at most this many times the upload
/// limit, plus the one file that is arriving.
const MOST_UPLOADS: usize = 3;

/// The most apps one install may name. Above the size of the whole
/// catalog, and well below what a body at the JSON limit could carry.
const MOST_APPS: usize = 128;

impl Machine {
    /// This computer as it is.
    ///
    /// `volume` narrows the search for readers to one mounted path, as
    /// `kobo setup --volume` does. `release_dir` names the release to
    /// install instead of the one this command was installed with.
    ///
    /// # Errors
    ///
    /// When this program cannot find itself to run again, or cannot make its
    /// private folder.
    pub fn here(volume: Option<PathBuf>, release_dir: Option<PathBuf>) -> Result<Self, String> {
        let program = std::env::current_exe()
            .and_then(fs::canonicalize)
            .map_err(|error| format!("could not find this kobo command: {error}"))?;
        let (readers, blocked): (Readers, Blocked) = match volume {
            Some(volume) => {
                let named = volume.clone();
                (
                    Box::new(move || setup::read_reader(&volume).into_iter().collect()),
                    Box::new(move || surroundings::refused(std::slice::from_ref(&named))),
                )
            }
            None => (
                Box::new(setup::mounted_readers),
                Box::new(|| surroundings::refused(&surroundings::removable_volumes())),
            ),
        };
        let shopfront = Shopfront::bundled()?;
        let pictures = Pictures::open(Pictures::folder_here(), shopfront.pictures());
        Ok(Self {
            program,
            readers,
            release: release_here(release_dir.as_deref()),
            release_dir,
            scratch: private_folder()?,
            fetch: Box::new(usb::published),
            shopfront,
            pictures,
            picture: Box::new(shopfront::published),
            key: usb::release_key()?,
            eject: Box::new(setup::eject),
            room: Box::new(surroundings::room),
            blocked,
            attached: Box::new(surroundings::kobos_attached),
            network: Box::new(|| crate::connect::local_subnet().is_some()),
            home: kobo_abi::home_directory().unwrap_or_default(),
            config: delivery::config_root(),
            uploads: Mutex::default(),
            receiving: Mutex::default(),
            store: Mutex::default(),
        })
    }
}

/// Makes the folder received files are kept in until they are sent.
///
/// Under the system's temporary folder, named for this process, and on Unix
/// readable by this user only: what an owner sends to their reader can be a
/// private document, and the temporary folder is one other accounts on the
/// computer can list.
fn private_folder() -> Result<PathBuf, String> {
    let folder = std::env::temp_dir().join(format!("kobo-ui-{}", std::process::id()));
    let _ = fs::remove_dir_all(&folder);
    fs::create_dir_all(&folder)
        .map_err(|error| format!("could not make {}: {error}", folder.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&folder, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("could not make {} private: {error}", folder.display()))?;
    }
    Ok(folder)
}

/// The Cobalt release this computer would put on a reader.
///
/// The same folder `kobo setup` installs from, and the same refusals it
/// makes, read from the signed manifest alone so that the window can say
/// which release it has without unpacking it. Setup checks the package
/// itself, in full, when it runs.
fn release_here(named: Option<&Path>) -> Result<String, String> {
    let Some(directory) = named
        .map(Path::to_owned)
        .or_else(crate::managed_release_directory)
    else {
        return Err(if cfg!(windows) {
            "There is no installer for the kobo command on Windows yet, so this copy has no \
             Cobalt release to put on a reader. The browser installer does the same job: \
             https://bandarlabs.github.io/Cobalt/install/"
                .to_owned()
        } else {
            "This copy of the kobo command was not installed with a Cobalt release, so it has \
             nothing to put on a reader. Install it from https://bandarlabs.github.io/Cobalt/ \
             and open this window again."
                .to_owned()
        });
    };
    let read = |name: &str| {
        fs::read(directory.join(name)).map_err(|error| {
            format!(
                "The Cobalt release in {} could not be read ({name}: {error}).",
                directory.display()
            )
        })
    };
    let manifest = read("cobalt-host-manifest.txt")?;
    let signature = String::from_utf8_lossy(&read("cobalt-host-manifest.txt.sig")?).into_owned();
    let manifest = crate::host_release::verify_manifest(&manifest, &signature)
        .map_err(|error| format!("The Cobalt release on this computer was refused: {error}."))?;
    let channel = String::from_utf8_lossy(&read("channel")?).trim().to_owned();
    if channel != "stable" || !manifest.allows_channel("stable") {
        return Err(
            "The release on this computer is not a stable one, and only stable releases are \
             installed over USB. Beta updates are turned on later, in Cobalt Settings on the \
             reader."
                .to_owned(),
        );
    }
    if !kobo_app_store::cobalt_version_at_least(env!("CARGO_PKG_VERSION"), &manifest.version) {
        return Err(format!(
            "The release on this computer, {}, is newer than this kobo command, {}. Run kobo \
             update, then open this window again.",
            manifest.version,
            env!("CARGO_PKG_VERSION")
        ));
    }
    Ok(manifest.version)
}

/// A string field of a JSON body.
fn text<'a>(body: &'a Value, field: &str) -> Option<&'a str> {
    body.get(field).and_then(Value::as_str)
}

fn bad(message: &str) -> Response {
    Response::refusal(400, "Bad Request", "bad_request", message)
}

fn conflict(kind: &str, message: &str) -> Response {
    Response::refusal(409, "Conflict", kind, console::display(message))
}

fn failed(message: &str) -> Response {
    Response::refusal(
        500,
        "Internal Server Error",
        "failed",
        console::display(message),
    )
}

/// The answer when a job of the same kind is already running.
fn busy(title: &str) -> Response {
    conflict(
        "busy",
        &format!("{title} has not finished yet. Wait for it, then try again."),
    )
}

/// A failure from the reader's app store, as the status and kind that tell
/// a front end what it was.
fn store_failed(failure: &usb::Failure) -> Response {
    let (status, reason, kind) = match failure.trouble {
        usb::Trouble::Unreachable => (502, "Bad Gateway", "unreachable"),
        usb::Trouble::Integrity => (502, "Bad Gateway", "integrity"),
        usb::Trouble::Missing => (404, "Not Found", "not_found"),
        usb::Trouble::TooOld => (409, "Conflict", "refused"),
        usb::Trouble::Write => (500, "Internal Server Error", "write_failed"),
        usb::Trouble::Other => (500, "Internal Server Error", "failed"),
    };
    Response::refusal(status, reason, kind, &failure.said)
}

/// Refuses to start writing to a card that is mounted read-only.
///
/// A reader's card comes up read-only when its filesystem was left dirty,
/// usually by a cable pulled without ejecting. Every write would fail one
/// file in, with whatever the system happens to say; this says what is
/// wrong and what clears it before anything is attempted.
fn card_takes_writes(shared: &Shared, reader: &setup::Mounted) -> Result<(), Response> {
    if (shared.machine.room)(&reader.volume).writable == Some(false) {
        return Err(conflict(
            "read_only",
            "The reader's storage is mounted read-only, so nothing can be written to it. \
             Eject it, unplug it, restart the reader and connect it again. If it comes back \
             read-only, its storage needs repairing from the reader's own settings.",
        ));
    }
    Ok(())
}

/// The mounted reader a request names.
///
/// The name is only ever compared with what this process finds mounted. A
/// path is not taken from the page and opened, because the page is the one
/// part of this program an attacker who got past every other rule would be
/// speaking through, and "write Cobalt to this folder" is not something to
/// do on its say-so.
fn named_reader(shared: &Shared, volume: Option<&str>) -> Result<setup::Mounted, Response> {
    let Some(volume) = volume else {
        return Err(bad("Name the reader to act on."));
    };
    (shared.machine.readers)()
        .into_iter()
        .find(|reader| reader.volume.to_string_lossy() == volume)
        .ok_or_else(|| {
            Response::refusal(
                404,
                "Not Found",
                "gone",
                "That reader is no longer connected. Plug it in and tap Connect on its screen.",
            )
        })
}

/// Refuses while a job is writing to a reader.
fn reader_is_free(shared: &Shared) -> Result<(), Response> {
    match shared.jobs.busy(Kind::Reader) {
        Some(title) => Err(busy(&title)),
        None => Ok(()),
    }
}

/// Starts this program again as a job.
fn start_command(
    shared: &Arc<Shared>,
    kind: Kind,
    title: &str,
    volume: Option<&Path>,
    arguments: Vec<String>,
    directory: Option<PathBuf>,
    after: impl FnOnce() + Send + 'static,
) -> Response {
    let program = shared.machine.program.clone();
    let started = shared.jobs.start(
        kind,
        title,
        volume,
        move |job| jobs::run_child(&program, &arguments, directory.as_deref(), job),
        after,
    );
    match started {
        Ok(id) => Response::ok(&json!({ "job": id })),
        Err(title) => busy(&title),
    }
}

/// How the Cobalt on a reader compares with the release on this computer.
fn standing(on_reader: Option<&str>, release: Result<&String, &String>) -> &'static str {
    match (on_reader, release) {
        (None, _) => "absent",
        (Some(_), Err(_)) => "unknown",
        (Some(reader), Ok(release)) if reader == release => "current",
        (Some(reader), Ok(release)) if kobo_app_store::cobalt_version_at_least(release, reader) => {
            "behind"
        }
        (Some(_), Ok(_)) => "ahead",
    }
}

/// One mounted reader as the page shows it.
///
/// The serial is left out. The page has no use for it, and the command line
/// has never shown more than its first four characters, which are the model.
fn reader_json(machine: &Machine, saved: &readers::Store, reader: &setup::Mounted) -> Value {
    let identified = setup::install_profile(reader);
    let profile = identified.as_ref().map_or_else(
        |_| setup::undo_profile(reader).profile,
        |found| found.profile,
    );
    let model = profile.map_or_else(
        || format!("Kobo, model code {}", reader.model_code()),
        |profile| profile.model.to_owned(),
    );
    let installed = reader
        .volume
        .join(setup::INSTALL_FOLDER)
        .join("VERSION")
        .is_file();
    let (cobalt, cobalt_problem) = match usb::Installation::on(&reader.volume) {
        Ok(installation) => (Some(usb::installation_json(&installation)), None),
        Err(_) if !installed => (None, None),
        Err(error) => (None, Some(console::display(&error).to_owned())),
    };
    let version = cobalt
        .as_ref()
        .and_then(|cobalt| cobalt.get("cobalt_version"))
        .and_then(Value::as_str);
    let room = (machine.room)(&reader.volume);
    json!({
        "volume": reader.volume.to_string_lossy(),
        "model": model,
        "model_code": reader.model_code(),
        "firmware": reader.firmware,
        "supported": identified.is_ok(),
        "refusal": identified.as_ref().err(),
        "untested": identified.as_ref().ok().and_then(|found| found.untested(reader)),
        "standing": standing(version, machine.release.as_ref()),
        "cobalt": cobalt,
        "cobalt_problem": cobalt_problem,
        "free_bytes": room.free,
        "writable": room.writable,
        "saved_as": arrival::saved_as(saved, &reader.serial),
        "menu_corner": arrival::menu_corner(&reader.firmware),
    })
}

/// The readers saved on this computer. A store that cannot be read is an
/// empty one here: every route that would write to it reads it again and
/// says so.
fn saved_readers(machine: &Machine) -> readers::Store {
    readers::Store::load(&machine.config.join("readers")).unwrap_or_default()
}

/// Every job, each install and search with where it has got to.
///
/// That is in the state, and not only in the job's own report, so that a
/// window opened in the middle of an install knows which step to show from
/// the first thing it reads.
fn jobs_json(shared: &Shared) -> Vec<Value> {
    shared
        .jobs
        .summaries()
        .into_iter()
        .map(|summary| {
            let job = summary["id"].as_u64().and_then(|id| shared.jobs.find(id));
            job.map_or(summary.clone(), |job| with_arrival(&job, summary))
        })
        .collect()
}

/// Adds where an install or a search has got to, for a job that is one.
fn with_arrival(job: &jobs::Job, mut answer: Value) -> Value {
    if arrival::follows(&job.title) {
        let whole = job.report(0);
        answer["setup"] = arrival::progress(
            whole["text"].as_str().unwrap_or_default(),
            whole["running"] == true,
            whole["ok"].as_bool(),
        );
    }
    answer
}

/// `GET /api/state`: everything the page shows without being asked, which
/// it reads every couple of seconds to notice a reader arriving or leaving.
fn state(shared: &Arc<Shared>, _: &mut Request<'_>) -> Response {
    let machine = &shared.machine;
    let mounted = (machine.readers)();
    let saved = saved_readers(machine);
    let readers: Vec<Value> = mounted
        .iter()
        .map(|reader| reader_json(machine, &saved, reader))
        .collect();
    Response::ok(&json!({
        "api": API,
        "computer": {
            "kobo": env!("CARGO_PKG_VERSION"),
            "os": if cfg!(target_os = "macos") {
                "macos"
            } else if cfg!(windows) {
                "windows"
            } else {
                "linux"
            },
            "release": machine.release.as_ref().ok(),
            "release_problem": machine.release.as_ref().err(),
            "ejects": cfg!(target_os = "macos"),
            "upload_limit": shared.limits.upload,
            "network": (machine.network)(),
        },
        "saved": arrival::saved(&saved, &mounted),
        "awaiting": arrival::awaiting(&machine.config).is_some(),
        "attached": (machine.attached)(),
        "blocked": (machine.blocked)()
            .iter()
            .map(|volume| volume.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        "readers": readers,
        "jobs": jobs_json(shared),
    }))
}

/// `GET /api/plan`: what installing or removing Cobalt would change on a
/// reader, in the words `kobo setup --dry-run` uses, before anything is
/// asked to happen.
fn plan(shared: &Arc<Shared>, request: &mut Request<'_>) -> Response {
    let volume = request.head.query("volume");
    let reader = match named_reader(shared, volume.as_deref()) {
        Ok(reader) => reader,
        Err(refusal) => return refusal,
    };
    let undo = request.head.query("undo").is_some_and(|value| value == "1");
    let mut words = vec![
        "--dry-run".to_owned(),
        "--volume".to_owned(),
        reader.volume.to_string_lossy().into_owned(),
    ];
    if undo {
        words.push("--undo".to_owned());
    }
    let options = match crate::parse_setup(&words) {
        Ok(options) => options,
        Err(error) => return failed(&error),
    };
    let identified = if undo {
        Ok(setup::undo_profile(&reader))
    } else {
        setup::install_profile(&reader)
    };
    let identified = match identified {
        Ok(identified) => identified,
        Err(reason) => return conflict("refused", &reason),
    };
    Response::ok(&json!({
        "undo": undo,
        "lines": joined(&crate::dry_run_plan(&options, &reader)),
        "release": shared.machine.release.as_ref().ok(),
        "untested": identified.untested(&reader).filter(|_| !undo),
    }))
}

/// The plan as one entry per change.
///
/// The plan is wrapped for a terminal, with a continued line indented under
/// the one it continues. The page wraps for itself, so the pieces are put
/// back together here.
fn joined(plan: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for line in plan.lines() {
        match lines.last_mut() {
            Some(previous) if line.starts_with(' ') => {
                previous.push(' ');
                previous.push_str(line.trim());
            }
            _ => lines.push(line.trim().to_owned()),
        }
    }
    lines
}

/// `POST /api/reader/install`: installs Cobalt, or updates it, by running
/// `kobo setup` with its confirmation already given. The confirmation was
/// the page showing the plan and the owner pressing the button under it.
fn install_cobalt(shared: &Arc<Shared>, request: &mut Request<'_>) -> Response {
    let body = match request.json() {
        Ok(body) => body,
        Err(refusal) => return refusal,
    };
    let reader = match named_reader(shared, text(&body, "volume")) {
        Ok(reader) => reader,
        Err(refusal) => return refusal,
    };
    if let Err(reason) = setup::install_profile(&reader) {
        return conflict("refused", &reason);
    }
    if let Err(problem) = &shared.machine.release {
        return conflict("refused", problem);
    }
    if let Err(refusal) = card_takes_writes(shared, &reader) {
        return refusal;
    }
    let mut arguments = vec![
        "setup".to_owned(),
        "--yes".to_owned(),
        "--non-interactive".to_owned(),
        "--volume".to_owned(),
        reader.volume.to_string_lossy().into_owned(),
    ];
    if let Some(directory) = &shared.machine.release_dir {
        arguments.push("--release-dir".to_owned());
        arguments.push(directory.to_string_lossy().into_owned());
    }
    // Named for what the owner will see happen. Setup does the same thing in
    // all three cases; it is the reader that differs.
    let title = match usb::Installation::on(&reader.volume) {
        Ok(installation) if Ok(&installation.version) == shared.machine.release.as_ref() => {
            "Installing Cobalt again"
        }
        Ok(_) => "Updating Cobalt",
        Err(_) => "Installing Cobalt",
    };
    start_command(
        shared,
        Kind::Reader,
        title,
        Some(&reader.volume),
        arguments,
        None,
        || {},
    )
}

/// `POST /api/reader/find`: looks again, over Wi-Fi, for the reader that
/// was set up from this computer and has not been saved.
///
/// Setup looks for five minutes after an install and then gives up, and
/// until now its last word was a command to type. This is that wait again,
/// with the saving and the locking that follow it, as a job the window can
/// start from a button. It takes no reader's name from the request: which
/// reader is waiting is read from what setup itself recorded.
fn find_reader(shared: &Arc<Shared>, request: &mut Request<'_>) -> Response {
    if let Err(refusal) = request.json() {
        return refusal;
    }
    if arrival::awaiting(&shared.machine.config).is_none() {
        return conflict(
            "refused",
            "No reader set up from this computer is waiting to be found. Plug the reader in \
             and install Cobalt on it from here first.",
        );
    }
    start_command(
        shared,
        Kind::Reader,
        arrival::FINDING,
        None,
        vec!["ui".to_owned(), "--find-reader".to_owned()],
        None,
        || {},
    )
}

/// `POST /api/readers/rename`: gives a saved reader another name.
fn rename_reader(shared: &Arc<Shared>, request: &mut Request<'_>) -> Response {
    let body = match request.json() {
        Ok(body) => body,
        Err(refusal) => return refusal,
    };
    let (Some(name), Some(to)) = (text(&body, "name"), text(&body, "to")) else {
        return bad("Name the reader and the name to give it.");
    };
    if let Some(problem) = arrival::name_problem(to) {
        return bad(problem);
    }
    saved_answer(shared, arrival::rename(&shared.machine.config, name, to))
}

/// `POST /api/readers/forget`: takes a reader out of the ones saved on this
/// computer, and changes nothing on the reader.
fn forget_reader(shared: &Arc<Shared>, request: &mut Request<'_>) -> Response {
    let body = match request.json() {
        Ok(body) => body,
        Err(refusal) => return refusal,
    };
    let Some(name) = text(&body, "name") else {
        return bad("Name the reader to forget.");
    };
    saved_answer(shared, arrival::forget(&shared.machine.config, name))
}

/// The saved readers as they now stand, or why they were not changed.
fn saved_answer(shared: &Shared, changed: Result<(), Option<String>>) -> Response {
    match changed {
        Ok(()) => {
            let machine = &shared.machine;
            let saved = arrival::saved(&saved_readers(machine), &(machine.readers)());
            Response::ok(&json!({ "saved": saved }))
        }
        Err(None) => Response::refusal(
            404,
            "Not Found",
            "not_found",
            "No reader is saved under that name.",
        ),
        Err(Some(reason)) => conflict("refused", &reason),
    }
}

/// `POST /api/reader/remove`: takes Cobalt off the reader with
/// `kobo setup --undo`.
fn remove_cobalt(shared: &Arc<Shared>, request: &mut Request<'_>) -> Response {
    let body = match request.json() {
        Ok(body) => body,
        Err(refusal) => return refusal,
    };
    let reader = match named_reader(shared, text(&body, "volume")) {
        Ok(reader) => reader,
        Err(refusal) => return refusal,
    };
    if let Err(refusal) = card_takes_writes(shared, &reader) {
        return refusal;
    }
    let arguments = vec![
        "setup".to_owned(),
        "--undo".to_owned(),
        "--yes".to_owned(),
        "--non-interactive".to_owned(),
        "--volume".to_owned(),
        reader.volume.to_string_lossy().into_owned(),
    ];
    start_command(
        shared,
        Kind::Reader,
        "Removing Cobalt",
        Some(&reader.volume),
        arguments,
        None,
        || {},
    )
}

/// `POST /api/reader/eject`.
fn eject(shared: &Arc<Shared>, request: &mut Request<'_>) -> Response {
    let body = match request.json() {
        Ok(body) => body,
        Err(refusal) => return refusal,
    };
    let reader = match named_reader(shared, text(&body, "volume")) {
        Ok(reader) => reader,
        Err(refusal) => return refusal,
    };
    if let Err(refusal) = reader_is_free(shared) {
        return refusal;
    }
    match (shared.machine.eject)(&reader.volume) {
        Ok(()) => Response::ok(&json!({ "ejected": true })),
        Err(error) => failed(&error),
    }
}

/// The installation on the reader a request names, with the store held.
///
/// `writes` says whether the answer will write to the card, so that a card
/// mounted read-only is refused with its own sentence instead of with
/// whatever the first failed write happens to say.
fn with_installation(
    shared: &Arc<Shared>,
    volume: Option<&str>,
    writes: bool,
    answer: impl FnOnce(&usb::Installation) -> Result<Value, usb::Failure>,
) -> Response {
    let reader = match named_reader(shared, volume) {
        Ok(reader) => reader,
        Err(refusal) => return refusal,
    };
    if let Err(refusal) = reader_is_free(shared) {
        return refusal;
    }
    if writes {
        if let Err(refusal) = card_takes_writes(shared, &reader) {
            return refusal;
        }
    }
    let installation = match usb::Installation::on(&reader.volume) {
        Ok(installation) => installation,
        Err(error) => return conflict("no_cobalt", &error),
    };
    let Ok(_held) = shared.machine.store.lock() else {
        return failed("the reader's app store is in use");
    };
    match answer(&installation) {
        Ok(mut value) => {
            value["reader"] = usb::installation_json(&installation);
            Response::ok(&value)
        }
        Err(failure) => store_failed(&failure),
    }
}

/// What installing an app needs beyond its own bytes: the staged manifest
/// and signature beside the binary, and the directory entries for all
/// three. Generous, because being told something fits and then watching it
/// fail is worse than being told it does not.
const ROOM_TO_SPARE: u64 = 256 * 1024;

/// The apps as a front end lists them, each with how large it is once
/// installed and whether the reader has room for it, where either is known.
fn apps_json(
    machine: &Machine,
    installation: &usb::Installation,
    apps: &[kobo_protocol::AppInfo],
) -> Value {
    let sizes = installation.sizes(&machine.key);
    let free = (machine.room)(&installation.volume).free;
    json!({
        "apps": apps
            .iter()
            .map(|app| {
                let mut entry = usb::app_json(installation, app);
                let bytes = sizes.get(&app.id).copied();
                entry["bytes"] = json!(bytes);
                entry["fits"] = json!(bytes
                    .zip(free)
                    .map(|(bytes, free)| bytes.saturating_add(ROOM_TO_SPARE) <= free));
                entry
            })
            .collect::<Vec<_>>(),
    })
}

/// A size as an owner reads it, in the decimal units a reader's own
/// storage screen uses.
fn megabytes(bytes: u64) -> String {
    #[allow(
        clippy::cast_precision_loss,
        reason = "a size shown to one decimal place of a megabyte"
    )]
    let megabytes = bytes as f64 / 1_000_000.0;
    format!("{megabytes:.1} MB")
}

/// `GET /api/apps`: the apps on the reader, from the volume alone.
///
/// This is the one question whose answer can involve a write. The store
/// finishes a swap that was interrupted before it lists anything, exactly
/// as the reader does at its next start, and there is no way to ask it not
/// to. It changes nothing an owner chose and is the same whoever asks.
fn installed_apps(shared: &Arc<Shared>, request: &mut Request<'_>) -> Response {
    let volume = request.head.query("volume");
    with_installation(shared, volume.as_deref(), false, |installation| {
        let apps = installation.installed(&shared.machine.key)?;
        Ok(apps_json(&shared.machine, installation, &apps))
    })
}

/// `POST /api/apps/catalog`: fetches and verifies the published catalog for
/// the reader's channel. A `POST` because the verified catalog is left on
/// the reader as its Store's own cache.
fn catalog(shared: &Arc<Shared>, request: &mut Request<'_>) -> Response {
    let body = match request.json() {
        Ok(body) => body,
        Err(refusal) => return refusal,
    };
    with_installation(shared, text(&body, "volume"), true, |installation| {
        let machine = &shared.machine;
        let apps = installation.available(&machine.key, |url, most| (machine.fetch)(url, most))?;
        Ok(apps_json(machine, installation, &apps))
    })
}

/// `POST /api/apps/install`: installs or updates the apps named, as a job,
/// one after another, each through the store's verified transaction.
fn install_apps(shared: &Arc<Shared>, request: &mut Request<'_>) -> Response {
    let body = match request.json() {
        Ok(body) => body,
        Err(refusal) => return refusal,
    };
    let reader = match named_reader(shared, text(&body, "volume")) {
        Ok(reader) => reader,
        Err(refusal) => return refusal,
    };
    let ids: Option<Vec<String>> = body.get("ids").and_then(Value::as_array).and_then(|ids| {
        ids.iter()
            .map(|id| {
                id.as_str()
                    .filter(|id| kobo_protocol::valid_app_id(id))
                    .map(str::to_owned)
            })
            .collect()
    });
    let Some(ids) = ids.filter(|ids| !ids.is_empty() && ids.len() <= MOST_APPS) else {
        return bad("Name the apps to install by their ids.");
    };
    if let Err(refusal) = card_takes_writes(shared, &reader) {
        return refusal;
    }
    let installation = match usb::Installation::on(&reader.volume) {
        Ok(installation) => installation,
        Err(error) => return conflict("no_cobalt", &error),
    };
    let title = if ids.len() == 1 {
        "Installing an app"
    } else {
        "Installing apps"
    };
    let working = Arc::clone(shared);
    let started = shared.jobs.start(
        Kind::Reader,
        title,
        Some(&reader.volume),
        move |job| install_each(&working.machine, &installation, &ids, job),
        || {},
    );
    match started {
        Ok(id) => Response::ok(&json!({ "job": id })),
        Err(title) => busy(&title),
    }
}

/// The work of an app install: the catalog once, then each app in turn. One
/// that fails is said and counted and the rest still run.
fn install_each(
    machine: &Machine,
    installation: &usb::Installation,
    ids: &[String],
    job: &jobs::Job,
) -> Finish {
    let failed = Finish {
        ok: false,
        code: None,
    };
    let Ok(_held) = machine.store.lock() else {
        job.say("The reader's app store is in use.\n");
        return failed;
    };
    let fetch = |url: &str, most: u32| (machine.fetch)(url, most);
    job.say("Fetching the signed app catalog.\n");
    let apps = match installation.available(&machine.key, fetch) {
        Ok(apps) => apps,
        Err(error) => {
            job.say(&format!("{error}\n"));
            return failed;
        }
    };
    let sizes = installation.sizes(&machine.key);
    let mut failures = 0;
    for id in ids {
        let Some(app) = apps.iter().find(|app| app.id == *id) else {
            failures += 1;
            job.say(&format!("The catalog has no app with the id {id}.\n"));
            continue;
        };
        // Asked afresh for each app, since the one before it took room.
        let free = (machine.room)(&installation.volume).free;
        if let Some((bytes, free)) = sizes.get(id).copied().zip(free) {
            if bytes.saturating_add(ROOM_TO_SPARE) > free {
                failures += 1;
                job.say(&format!(
                    "{} needs {} and the reader has {} free. Remove something from the \
                     reader and try again.\n",
                    app.title,
                    megabytes(bytes),
                    megabytes(free)
                ));
                continue;
            }
        }
        job.say(&format!(
            "Fetching and checking {} {}.\n",
            app.title, app.version
        ));
        match installation.install(app, &machine.key, fetch) {
            Ok(outcome) => job.say(&format!("{}\n", outcome.sentence(&app.title))),
            Err(error) => {
                failures += 1;
                job.say(&format!("{error}\n"));
            }
        }
    }
    Finish {
        ok: failures == 0,
        code: None,
    }
}

/// `GET /api/store`: every app as a store shows it, in the website's
/// groups, from what this command was built with.
///
/// It asks nothing of a reader or the network, so the store can be read on
/// a computer that has neither. What a particular reader has, and what the
/// signed catalog offers it today, are the two routes above; a front end
/// lays those over this by app id.
fn store(shared: &Arc<Shared>, _: &mut Request<'_>) -> Response {
    Response::ok(shared.machine.shopfront.answer())
}

/// `GET /api/shot`: one of the pictures the store lists, by the name it is
/// listed under.
///
/// A name is looked up among the listed ones before it is used for
/// anything, so what this fetches and what it reads from disk are chosen
/// from a list built into this command and never composed from the
/// request. It is a question that can write: a picture fetched for the
/// first time is kept in this computer's cache folder. Nothing an owner
/// chose changes by it and nothing on a reader does.
fn shot(shared: &Arc<Shared>, request: &mut Request<'_>) -> Response {
    let machine = &shared.machine;
    let Some(name) = request
        .head
        .query("name")
        .filter(|name| machine.shopfront.shows(name))
    else {
        return Response::refusal(
            404,
            "Not Found",
            "not_found",
            "The store lists no picture by that name.",
        );
    };
    match machine.pictures.get(&name, &*machine.picture) {
        Some(picture) => Response::picture(&picture),
        None => Response::refusal(
            502,
            "Bad Gateway",
            "unreachable",
            "The picture could not be fetched. Check this computer's internet connection.",
        ),
    }
}

/// `POST /api/apps/remove`: removes one app and answers with what is left.
fn remove_app(shared: &Arc<Shared>, request: &mut Request<'_>) -> Response {
    let body = match request.json() {
        Ok(body) => body,
        Err(refusal) => return refusal,
    };
    let Some(id) = text(&body, "id").filter(|id| kobo_protocol::valid_app_id(id)) else {
        return bad("Name the app to remove by its id.");
    };
    with_installation(shared, text(&body, "volume"), true, |installation| {
        let key = &shared.machine.key;
        let before = installation.installed(key)?;
        let app = before
            .iter()
            .find(|app| app.id == id)
            .ok_or_else(|| usb::Failure {
                trouble: usb::Trouble::Missing,
                said: format!("{id} is not installed on this reader."),
            })?;
        installation.remove(&app.id, key)?;
        let mut answer = apps_json(&shared.machine, installation, &installation.installed(key)?);
        answer["removed"] = json!({ "id": app.id, "title": app.title });
        Ok(answer)
    })
}

/// The browsers on this computer that Send to Kobo can be set up in.
///
/// A browser counts as here when the folder it keeps its profiles in
/// exists, which is the test `kobo clip install --all` uses.
fn browsers_here(machine: &Machine) -> Vec<Browser> {
    let os = Os::current();
    let local = std::env::var_os("LOCALAPPDATA").map_or_else(PathBuf::new, PathBuf::from);
    let roaming = std::env::var_os("APPDATA").map_or_else(PathBuf::new, PathBuf::from);
    Browser::ALL
        .into_iter()
        .filter(|browser| os == Os::Mac || *browser != Browser::Safari)
        .filter(|browser| {
            let folder =
                clip_install::profile_folder(*browser, os, &machine.home, &local, &roaming);
            !folder.as_os_str().is_empty() && folder.exists()
        })
        .collect()
}

/// Whether Send to Kobo has been set up for `browser`: whether the file
/// that browser reads to find `kobo` is where setting it up puts it.
fn is_set_up(machine: &Machine, browser: Browser) -> bool {
    match clip_install::registration(browser, Os::current(), &machine.home, &machine.config) {
        Some(Registration::File(path)) => path.is_file(),
        Some(Registration::Registry { manifest, .. }) => manifest.is_file(),
        None => clip_install::safari_service(&machine.home).exists(),
    }
}

fn browsers_json(machine: &Machine) -> Value {
    let extension = machine.config.join("clip").join("extension");
    let found: Vec<Value> = browsers_here(machine)
        .into_iter()
        .map(|browser| {
            json!({
                "id": browser.id(),
                "title": browser.title(),
                "set_up": is_set_up(machine, browser),
                "step": clip_install::steps(browser, &extension),
            })
        })
        .collect();
    json!({ "browsers": found })
}

/// `GET /api/browsers`.
fn browsers(shared: &Arc<Shared>, _: &mut Request<'_>) -> Response {
    Response::ok(&browsers_json(&shared.machine))
}

/// Runs `kobo clip install` or `uninstall` for one browser and answers with
/// what it said and how the browsers stand afterwards.
///
/// Run and waited for rather than made a job: it writes a few small files
/// and is done in well under a second.
fn clip_command(shared: &Arc<Shared>, request: &mut Request<'_>, verb: &str) -> Response {
    let body = match request.json() {
        Ok(body) => body,
        Err(refusal) => return refusal,
    };
    let machine = &shared.machine;
    let Some(browser) = text(&body, "browser")
        .and_then(Browser::named)
        .filter(|browser| browsers_here(machine).contains(browser))
    else {
        return bad("That browser was not found on this computer.");
    };
    let output = std::process::Command::new(&machine.program)
        .args(["clip", verb, "--browser", browser.id()])
        .stdin(std::process::Stdio::null())
        .output();
    let output = match output {
        Ok(output) => output,
        Err(error) => return failed(&format!("Could not run the kobo command: {error}")),
    };
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if !output.status.success() {
        return failed(said.trim());
    }
    let mut answer = browsers_json(machine);
    answer["said"] = Value::String(said);
    Response::ok(&answer)
}

/// `POST /api/browsers/setup`.
fn set_up_browser(shared: &Arc<Shared>, request: &mut Request<'_>) -> Response {
    clip_command(shared, request, "install")
}

/// `POST /api/browsers/remove`.
fn forget_browser(shared: &Arc<Shared>, request: &mut Request<'_>) -> Response {
    clip_command(shared, request, "uninstall")
}

/// `GET /api/send`: where a send can go and what is waiting to.
fn sending(shared: &Arc<Shared>, _: &mut Request<'_>) -> Response {
    let machine = &shared.machine;
    let saved = readers::Store::load(&machine.config.join("readers")).unwrap_or_default();
    let waiting = delivery::waiting(&machine.config.join("clip").join("queue"));
    Response::ok(&json!({
        "readers": saved
            .readers
            .iter()
            .map(|reader| json!({ "name": reader.nickname, "paired": reader.paired }))
            .collect::<Vec<_>>(),
        "waiting": waiting
            .iter()
            .map(|article| json!({ "title": article.title, "site": article.site }))
            .collect::<Vec<_>>(),
    }))
}

/// A file name that is safe to create in a folder and to pass to a command.
///
/// What the page sends is what the owner's file is called, which can be
/// anything. Only the last component is kept, so the name cannot climb out
/// of its folder. Characters a file name cannot hold on some system become
/// underscores. A leading dash would be read by `kobo send` as an option and
/// a leading dot would hide the file, so neither survives. The extension is
/// kept whole when the rest is shortened, because the extension is how the
/// file is matched to the app that opens it.
fn safe_name(offered: &str) -> String {
    let last = offered.rsplit(['/', '\\']).next().unwrap_or_default();
    let cleaned: String = last
        .chars()
        .map(|character| {
            if character.is_control() || "<>:\"|?*".contains(character) {
                '_'
            } else {
                character
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_start_matches(['-', '.']).trim();
    if cleaned.is_empty() {
        return "file".to_owned();
    }
    let (stem, extension) = match cleaned.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() && extension.chars().count() <= 12 => {
            (stem, Some(extension))
        }
        _ => (cleaned, None),
    };
    let stem: String = stem.chars().take(96).collect();
    match extension {
        Some(extension) => format!("{stem}.{extension}"),
        None => stem,
    }
}

fn companion_json(companion: Companion) -> Value {
    json!({
        "name": companion.name(),
        "title": companion.title(),
        "purpose": companion.purpose(),
        "replaces": companion.replaces(),
    })
}

/// `POST /api/send/file`: receives a file and says which app opens it.
///
/// The bytes go straight to a file in a folder of their own. Which app
/// takes the file is then decided the way `kobo send` decides it, and the
/// two questions a terminal would be asked are sent back as data instead:
/// which app, when more than one opens this kind of file, and whether to
/// replace, when the app keeps one thing at a time.
fn receive_file(shared: &Arc<Shared>, request: &mut Request<'_>) -> Response {
    let machine = &shared.machine;
    let name = safe_name(&request.head.query("name").unwrap_or_default());
    let Ok(_receiving) = machine.receiving.lock() else {
        return failed("another file is still arriving");
    };
    let id = {
        let Ok(mut uploads) = machine.uploads.lock() else {
            return failed("the received files could not be listed");
        };
        uploads.given += 1;
        uploads.given
    };
    let folder = machine.scratch.join(format!("file-{id}"));
    let path = folder.join(&name);
    let received = fs::create_dir_all(&folder)
        .and_then(|()| fs::File::create(&path))
        .map_err(|_| Malformed::Incomplete)
        .and_then(|mut file| request.save(&mut file));
    let bytes = match received {
        Ok(bytes) => bytes,
        Err(why) => {
            let _ = fs::remove_dir_all(&folder);
            return Response::malformed(why);
        }
    };
    let mut answer = json!({ "upload": id, "name": name, "bytes": bytes });
    match detect::choose(&detect::candidates(&path), None) {
        Ok(companion) => answer["app"] = companion_json(companion),
        Err(detect::ChooseError::Ambiguous(offered)) => {
            answer["choices"] = offered.into_iter().map(companion_json).collect();
        }
        Err(detect::ChooseError::Message(why)) => {
            let _ = fs::remove_dir_all(&folder);
            return Response::refusal(
                422,
                "Unprocessable Content",
                "unsupported_file",
                console::display(&why),
            );
        }
    }
    if let Ok(mut uploads) = machine.uploads.lock() {
        uploads.waiting.push(Upload { id, folder, name });
        // Only now, with the new file whole and wanted, does an old one make
        // way for it. A file that was refused must not cost the owner one
        // they were about to send.
        while uploads.waiting.len() > MOST_UPLOADS {
            let oldest = uploads.waiting.remove(0);
            let _ = fs::remove_dir_all(oldest.folder);
        }
    }
    Response::ok(&answer)
}

/// The saved reader a request names, as the flags that send to it.
///
/// No name is the reader `kobo setup` saved, which needs no flag. A name
/// has to be one in the store, so that what follows `--reader` on the
/// command line is always something this computer already wrote down.
fn reader_flags(machine: &Machine, body: &Value) -> Result<Vec<String>, Response> {
    let Some(name) = text(body, "reader").filter(|name| !name.is_empty()) else {
        return Ok(Vec::new());
    };
    let saved = readers::Store::load(&machine.config.join("readers")).unwrap_or_default();
    if saved.find(name).is_none() {
        return Err(bad("No reader is saved under that name."));
    }
    Ok(vec!["--reader".to_owned(), name.to_owned()])
}

/// `POST /api/send/start`: sends a received file with `kobo send`.
///
/// The command runs in the file's own folder and is given the bare name, so
/// that what it says is "Sent Winter.cbz to Panels" and not the path of a
/// temporary folder. The file is deleted when the command ends, whichever
/// way it ends.
fn send_file(shared: &Arc<Shared>, request: &mut Request<'_>) -> Response {
    let body = match request.json() {
        Ok(body) => body,
        Err(refusal) => return refusal,
    };
    let machine = &shared.machine;
    let mut arguments = vec!["send".to_owned()];
    if let Some(app) = text(&body, "to") {
        let Some(companion) = Companion::ALL
            .into_iter()
            .find(|companion| companion.name() == app)
        else {
            return bad("No app by that name takes files.");
        };
        arguments.push("--to".to_owned());
        arguments.push(companion.name().to_owned());
    }
    if body.get("replace").and_then(Value::as_bool) == Some(true) {
        arguments.push("--replace".to_owned());
    }
    match reader_flags(machine, &body) {
        Ok(flags) => arguments.extend(flags),
        Err(refusal) => return refusal,
    }
    if let Some(title) = shared.jobs.busy(Kind::Send) {
        return busy(&title);
    }
    let wanted = body.get("upload").and_then(Value::as_u64);
    let upload = machine.uploads.lock().ok().and_then(|mut uploads| {
        let position = uploads
            .waiting
            .iter()
            .position(|upload| Some(upload.id) == wanted)?;
        Some(uploads.waiting.remove(position))
    });
    let Some(upload) = upload else {
        return Response::refusal(
            404,
            "Not Found",
            "not_found",
            "That file is no longer here. Choose it again.",
        );
    };
    // The name lost any leading dash when the file was received, so it
    // cannot be read as an option wherever on the line it stands.
    arguments.insert(1, upload.name.clone());
    let folder = upload.folder.clone();
    start_command(
        shared,
        Kind::Send,
        &format!("Sending {}", upload.name),
        None,
        arguments,
        Some(upload.folder),
        move || {
            let _ = fs::remove_dir_all(folder);
        },
    )
}

/// An address `kobo clip` can be handed.
///
/// Only the two schemes a web page has. The scheme check is also what makes
/// the address safe as an argument: nothing that begins `http` can be read
/// as an option. Spaces and control characters are refused rather than
/// escaped, because a pasted address that contains them is two things or a
/// mistake.
fn web_address(offered: &str) -> Option<&str> {
    let address = offered.trim();
    let has_scheme = ["http://", "https://"].iter().any(|scheme| {
        address
            .get(..scheme.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(scheme))
            && address.len() > scheme.len()
    });
    (has_scheme
        && address.len() <= 2048
        && !address
            .chars()
            .any(|character| character.is_whitespace() || character.is_control()))
    .then_some(address)
}

/// `POST /api/send/address`: sends a web article with `kobo clip`.
fn send_address(shared: &Arc<Shared>, request: &mut Request<'_>) -> Response {
    let body = match request.json() {
        Ok(body) => body,
        Err(refusal) => return refusal,
    };
    let Some(address) = text(&body, "url").and_then(web_address) else {
        return bad("Enter a web address that starts with http:// or https://.");
    };
    let mut arguments = vec!["clip".to_owned(), address.to_owned()];
    match reader_flags(&shared.machine, &body) {
        Ok(flags) => arguments.extend(flags),
        Err(refusal) => return refusal,
    }
    start_command(
        shared,
        Kind::Send,
        "Sending an article",
        None,
        arguments,
        None,
        || {},
    )
}

/// `POST /api/send/queue`: sends what is waiting with `kobo clip sync`.
fn send_waiting(shared: &Arc<Shared>, request: &mut Request<'_>) -> Response {
    let body = match request.json() {
        Ok(body) => body,
        Err(refusal) => return refusal,
    };
    let mut arguments = vec!["clip".to_owned(), "sync".to_owned()];
    match reader_flags(&shared.machine, &body) {
        Ok(flags) => arguments.extend(flags),
        Err(refusal) => return refusal,
    }
    start_command(
        shared,
        Kind::Send,
        "Sending waiting articles",
        None,
        arguments,
        None,
        || {},
    )
}

/// `GET /api/job`: how a job is going and what it has said since `from`.
fn job(shared: &Arc<Shared>, request: &mut Request<'_>) -> Response {
    let id = request
        .head
        .query("id")
        .and_then(|id| id.parse::<u64>().ok());
    let from = request
        .head
        .query("from")
        .and_then(|from| from.parse::<usize>().ok())
        .unwrap_or(0);
    match id.and_then(|id| shared.jobs.find(id)) {
        Some(job) => Response::ok(&with_arrival(&job, job.report(from))),
        None => Response::refusal(
            404,
            "Not Found",
            "not_found",
            "That job is not remembered any more.",
        ),
    }
}

/// `POST /api/goodbye`: the page is closing. The server notes it and waits
/// a moment to see whether it comes back, which a reload does.
fn goodbye(_: &Arc<Shared>, _: &mut Request<'_>) -> Response {
    Response::ok(&json!({}))
}

#[cfg(test)]
#[path = "api_tests.rs"]
mod tests;

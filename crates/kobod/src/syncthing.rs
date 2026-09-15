//! The small, bounded part of Syncthing Cobalt owns.
//!
//! Syncthing itself remains unmodified.  This module owns its private home,
//! loopback REST key, process lifetime, and the fixed folder policy.  It never
//! accepts a path, command, peer id, or API key from an application.

use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const APP_STATE: &str = "/mnt/onboard/.adds/cobalt/state/syncthing";
const HOME: &str = "/var/lib/cobalt/syncthing";
const ENGINE: &str = "/mnt/onboard/.adds/cobalt/bin/syncthing";
const ENGINE_SHA256: &str = "e7e0523d8db0328b22ebff5c98bd721c94e295122771c0538414898a06ef8ebf";

/// Where the engine is fetched from when it is not installed yet.
///
/// A tag of its own rather than the current release: the engine changes when
/// Syncthing is rebuilt, which is rarely, and tying it to the platform release
/// would put a 27.9 MB download back into the path of every reader on every
/// update, which is the whole thing this avoids.
const ENGINE_URL: &str =
    "https://github.com/BandarLabs/Cobalt/releases/download/syncthing-v2.0.9/syncthing-armv7";

/// The most the engine download may be. The reviewed artifact is 27.9 MB.
const ENGINE_LIMIT: u32 = 48 * 1024 * 1024;
const SYNC_ROOT: &str = "/mnt/onboard/.adds/cobalt/sync";
const VAULT_DATA: &str = "/mnt/onboard/.adds/cobalt/data/vault";
const FRAME_DATA: &str = "/mnt/onboard/.adds/cobalt/data/frame";
const MAX_WINDOW: Duration = Duration::from_secs(5 * 60);
const TAIL_WINDOW: Duration = Duration::from_secs(90);
const POLL: Duration = Duration::from_secs(2);

const FOLDERS: [(&str, &str); 4] = [
    ("vault", "receiveonly"),
    ("frame", "receiveonly"),
    ("books", "receiveonly"),
    ("out", "sendonly"),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Cadence {
    Manual,
    Hourly,
    FourHourly,
    Daily,
}

impl Cadence {
    const fn seconds(self) -> Option<u32> {
        match self {
            Self::Manual => None,
            Self::Hourly => Some(60 * 60),
            Self::FourHourly => Some(4 * 60 * 60),
            Self::Daily => Some(24 * 60 * 60),
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "1" => Self::Hourly,
            "2" => Self::FourHourly,
            "3" => Self::Daily,
            _ => Self::Manual,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Settings {
    enabled: bool,
    cadence: Cadence,
}

impl Settings {
    fn load(root: &Path) -> Result<Self, String> {
        let text = fs::read_to_string(root.join("sync-config"))
            .map_err(|error| format!("read sync configuration: {error}"))?;
        Self::parse(&text)
    }

    fn parse(text: &str) -> Result<Self, String> {
        let mut lines = text.lines();
        let enabled = match lines.next() {
            Some("true") => true,
            Some("false") => false,
            _ => return Err("sync configuration has an invalid enabled value".to_owned()),
        };
        let cadence = Cadence::parse(lines.next().unwrap_or_default());
        Ok(Self { enabled, cadence })
    }
}

/// `kobod --syncthing id|pair DEVICE_ID FOLDER|status|window [seconds]|tail|scheduled|stop`.
pub fn command(arguments: &[String]) -> Result<(), String> {
    let state = Path::new(APP_STATE);
    match arguments {
        [action] if action == "id" => {
            let home = Path::new(HOME);
            initialize(home)?;
            println!("{}", local_device_id(home)?);
            Ok(())
        }
        [action, peer_id, folder] if action == "pair" => {
            pair(Path::new(HOME), peer_id, folder)?;
            println!(
                "Paired host {peer_id} with kobo-{folder}; Kobo device ID is {}.",
                local_device_id(Path::new(HOME))?
            );
            Ok(())
        }
        [action] if action == "status" => {
            println!("{}", read_status(state));
            Ok(())
        }
        [action] if action == "stop" => {
            if let Ok(key) = fs::read_to_string(Path::new(HOME).join("api-key")) {
                let _ignored = rest_shutdown(key.trim());
            }
            write_status(state, &Status::quiet("stopped", "Stopped by the owner."))?;
            Ok(())
        }
        [action] if action == "tail" => run_window(state, TAIL_WINDOW, "tail"),
        [action] if action == "scheduled" => run_window(state, MAX_WINDOW, "scheduled"),
        [action, seconds] if action == "window" => {
            let seconds = seconds
                .parse::<u64>()
                .map_err(|_| "Sync window seconds must be a whole number".to_owned())?;
            run_window(
                state,
                Duration::from_secs(seconds).min(MAX_WINDOW),
                "settings",
            )
        }
        _ => Err(
            "usage: kobod --syncthing id|pair DEVICE_ID vault|frame|books|out|status|stop|tail|scheduled|window SECONDS"
                .to_owned(),
        ),
    }
}

fn run_window(app_state: &Path, requested: Duration, reason: &str) -> Result<(), String> {
    let settings = Settings::load(app_state)?;
    if !settings.enabled {
        write_status(app_state, &Status::quiet("disabled", "Sync is off."))?;
        return Ok(());
    }
    if !window_is_due(&settings, reason) {
        write_status(app_state, &Status::quiet("waiting", "Manual sync only."))?;
        return Ok(());
    }
    let home = Path::new(HOME);
    prepare(home)?;
    let mut child = match start(home) {
        Ok(child) => child,
        Err(detail) => {
            write_status(app_state, &Status::quiet("failed", detail))?;
            return Ok(());
        }
    };
    write_status(app_state, &Status::quiet("running", "Syncing folders."))?;
    let deadline = Instant::now() + requested;
    let key = fs::read_to_string(home.join("api-key"))
        .map_err(|error| format!("read Sync REST key: {error}"))?;
    let peers = read_peers(home)?;
    loop {
        if !Settings::load(app_state)?.enabled {
            stop(&mut child, &key);
            write_status(app_state, &Status::quiet("paused", "Paused by the owner."))?;
            return Ok(());
        }
        if child
            .try_wait()
            .map_err(|error| format!("wait for Sync engine: {error}"))?
            .is_some()
        {
            write_status(
                app_state,
                &Status::quiet(
                    "failed",
                    "Sync engine stopped. Install the platform update again.",
                ),
            )?;
            return Ok(());
        }
        match folder_state(&key, &peers) {
            Ok(progress) if progress.idle && progress.conflicts > 0 => {
                stop(&mut child, &key);
                conflict_window(app_state, &progress)?;
                return Ok(());
            }
            Ok(progress) if progress.idle => {
                stop(&mut child, &key);
                complete_window(app_state, &progress)?;
                return Ok(());
            }
            Ok(progress) => {
                write_status(
                    app_state,
                    &Status {
                        state: "running",
                        bytes: progress.bytes,
                        message: "Syncing folders.".to_owned(),
                        peers: progress.online,
                        conflicts: progress.conflicts,
                    },
                )?;
            }
            Err(detail) => write_status(app_state, &Status::quiet("running", detail))?,
        }
        if Instant::now() >= deadline {
            stop(&mut child, &key);
            write_status(
                app_state,
                &Status::quiet("timed-out", "Sync window ended after five minutes."),
            )?;
            return Ok(());
        }
        std::thread::sleep(POLL);
    }
}

/// Whether this trigger may open a radio window. A stored `false` is checked
/// immediately before spawning, not only when a wake was scheduled.
fn window_is_due(settings: &Settings, reason: &str) -> bool {
    settings.enabled && (reason != "scheduled" || settings.cadence.seconds().is_some())
}

fn initialize(home: &Path) -> Result<(), String> {
    verify_engine(Path::new(ENGINE))?;
    fs::create_dir_all(home).map_err(|error| format!("create Sync state: {error}"))?;
    // Windows has no mode bits; the user profile ACL scopes the directory.
    #[cfg(unix)]
    fs::set_permissions(home, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("protect Sync state: {error}"))?;
    if !home.join("cert.pem").is_file() || !home.join("key.pem").is_file() {
        let output = Command::new(ENGINE)
            .arg("generate")
            .arg("--home")
            .arg(home)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|error| format!("generate Sync identity: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "generate Sync identity: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
    }
    Ok(())
}

fn prepare(home: &Path) -> Result<(), String> {
    initialize(home)?;
    fs::create_dir_all(SYNC_ROOT).map_err(|error| format!("create Sync folders: {error}"))?;
    for (name, _) in FOLDERS {
        fs::create_dir_all(Path::new(SYNC_ROOT).join(name))
            .map_err(|error| format!("create sync/{name}: {error}"))?;
    }
    let key_path = home.join("api-key");
    if !key_path.exists() {
        let mut entropy =
            File::open("/dev/urandom").map_err(|error| format!("open entropy: {error}"))?;
        let mut bytes = [0_u8; 32];
        entropy
            .read_exact(&mut bytes)
            .map_err(|error| format!("read entropy: {error}"))?;
        let mut key = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            let _ignored = write!(key, "{byte:02x}");
        }
        atomic_write(&key_path, &key, 0o600)?;
    }
    let key =
        fs::read_to_string(&key_path).map_err(|error| format!("read Sync REST key: {error}"))?;
    let local_id = local_device_id(home)?;
    let peers = read_peers(home)?;
    atomic_write(
        &home.join("config.xml"),
        &xml(&local_id, &peers, key.trim()),
        0o600,
    )
}

fn local_device_id(home: &Path) -> Result<String, String> {
    let output = Command::new(ENGINE)
        .arg("device-id")
        .arg("--home")
        .arg(home)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("read Sync device ID: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "read Sync device ID: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let id = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !valid_device_id(&id) {
        return Err("Sync engine returned an invalid device ID.".to_owned());
    }
    Ok(id)
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Peer {
    folder: String,
    device_id: String,
}

fn pair(home: &Path, peer_id: &str, folder: &str) -> Result<(), String> {
    if !valid_device_id(peer_id) {
        return Err("host Syncthing device ID is not canonical".to_owned());
    }
    if !FOLDERS.iter().any(|(name, _)| *name == folder) {
        return Err("Sync folder must be vault, frame, books, or out".to_owned());
    }
    initialize(home)?;
    let address: SocketAddr = "127.0.0.1:8384".parse().expect("fixed loopback socket");
    if TcpStream::connect_timeout(&address, Duration::from_millis(200)).is_ok() {
        return Err("Sync is running; stop the current window before pairing.".to_owned());
    }
    let mut peers = read_peers(home)?;
    peers.retain(|peer| peer.folder != folder || peer.device_id == peer_id);
    if !peers
        .iter()
        .any(|peer| peer.folder == folder && peer.device_id == peer_id)
    {
        peers.push(Peer {
            folder: folder.to_owned(),
            device_id: peer_id.to_owned(),
        });
    }
    peers.sort_by(|left, right| {
        (&left.folder, &left.device_id).cmp(&(&right.folder, &right.device_id))
    });
    let contents = peers.iter().fold(String::new(), |mut contents, peer| {
        let _ignored = writeln!(contents, "{}\t{}", peer.folder, peer.device_id);
        contents
    });
    atomic_write(&home.join("peers"), &contents, 0o600)?;
    prepare(home)
}

fn read_peers(home: &Path) -> Result<Vec<Peer>, String> {
    let Ok(text) = fs::read_to_string(home.join("peers")) else {
        return Ok(Vec::new());
    };
    text.lines()
        .map(|line| {
            let (folder, device_id) = line
                .split_once('\t')
                .ok_or_else(|| "Sync peer configuration is malformed.".to_owned())?;
            if !FOLDERS.iter().any(|(name, _)| *name == folder) || !valid_device_id(device_id) {
                return Err("Sync peer configuration is malformed.".to_owned());
            }
            Ok(Peer {
                folder: folder.to_owned(),
                device_id: device_id.to_owned(),
            })
        })
        .collect()
}

fn valid_device_id(value: &str) -> bool {
    let groups = value.split('-').collect::<Vec<_>>();
    groups.len() == 8
        && groups.iter().all(|group| {
            group.len() == 7
                && group
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || (b'2'..=b'7').contains(&byte))
        })
}

fn xml(local_id: &str, peers: &[Peer], key: &str) -> String {
    let folders = FOLDERS
        .iter()
        .fold(String::new(), |mut folders, (name, kind)| {
            let peer_devices = peers.iter().filter(|peer| peer.folder == *name).fold(
                String::new(),
                |mut devices, peer| {
                    let _ignored = write!(devices, "<device id=\"{}\"/>", peer.device_id);
                    devices
                },
            );
            let _ignored = write!(
                folders,
                "<folder id=\"kobo-{name}\" path=\"{SYNC_ROOT}/{name}\" type=\"{kind}\">\
                 <device id=\"{local_id}\"/>{peer_devices}</folder>"
            );
            folders
        });
    let devices = peers.iter().fold(String::new(), |mut devices, peer| {
        if !devices.contains(&format!("id=\"{}\"", peer.device_id)) {
            let _ignored = write!(
                devices,
                "<device id=\"{}\" name=\"Kobo host\"><address>dynamic</address></device>",
                peer.device_id
            );
        }
        devices
    });
    format!("<configuration version=\"37\"><options><globalAnnounceEnabled>true</globalAnnounceEnabled><relaysEnabled>true</relaysEnabled><natEnabled>false</natEnabled><localAnnounceEnabled>true</localAnnounceEnabled><listenAddresses><address>default</address></listenAddresses></options><gui enabled=\"true\" tls=\"false\" address=\"127.0.0.1:8384\" apikey=\"{key}\"/>{folders}{devices}</configuration>")
}

fn atomic_write(path: &Path, value: &str, mode: u32) -> Result<(), String> {
    let temporary = path.with_extension("new");
    match fs::remove_file(&temporary) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("remove stale {}: {error}", temporary.display())),
    }
    #[allow(unused_mut)] // Windows has no mode bits to set.
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(mode);
    let _ = mode;
    let mut file = options
        .open(&temporary)
        .map_err(|error| format!("write {}: {error}", temporary.display()))?;
    file.write_all(value.as_bytes())
        .map_err(|error| format!("write {}: {error}", temporary.display()))?;
    file.sync_all()
        .map_err(|error| format!("sync {}: {error}", temporary.display()))?;
    fs::rename(&temporary, path).map_err(|error| format!("replace {}: {error}", path.display()))
}

fn start(home: &Path) -> Result<Child, String> {
    ensure_engine(Path::new(ENGINE))?;
    verify_engine(Path::new(ENGINE))?;
    Command::new(ENGINE)
        .arg("--home")
        .arg(home)
        .arg("--no-browser")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("start Sync engine: {error}"))
}

/// Fetches the engine the first time somebody actually turns Sync on.
///
/// The engine used to travel inside the platform package, where it was 27.9 MB
/// of a 31.0 MB release: every reader downloaded it on every update, over
/// Wi-Fi, and the updater holds the whole archive and the whole expanded tree
/// in memory at once on a device with half a gigabyte of it. That is a large
/// bill for a feature most readers never enable, presented at the worst
/// possible moment.
///
/// So it is fetched here instead, once, by the reader that asked for it. The
/// address is a tag that does not move, because the digest below is what makes
/// the download safe to trust and a digest cannot be pinned to a moving target.
/// Nothing is trusted on arrival: [`verify_engine`] runs afterwards exactly as
/// it did when the firmware unpacked these bytes, so a truncated or substituted
/// download is refused rather than executed.
fn ensure_engine(engine: &Path) -> Result<(), String> {
    if engine.exists() {
        return Ok(());
    }
    let bytes = kobo_net::fetch(ENGINE_URL, ENGINE_LIMIT).map_err(|_| {
        "Sync engine could not be downloaded. Check Wi-Fi and try again.".to_owned()
    })?;
    if kobo_net::sha256::hex_digest(&bytes) != ENGINE_SHA256 {
        return Err("Sync engine download did not match its checksum.".to_owned());
    }
    // Written beside the destination and renamed, so an interrupted download
    // never leaves a partial file at a path the next run would take for an
    // installed engine and hand to `verify_engine` as a checksum failure.
    let partial = engine.with_extension("part");
    #[allow(unused_mut)] // Windows has no mode bits to set.
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o755);
    let mut file = options
        .open(&partial)
        .map_err(|error| format!("create Sync engine: {error}"))?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("write Sync engine: {error}"))?;
    drop(file);
    fs::rename(&partial, engine).map_err(|error| format!("install Sync engine: {error}"))
}

/// Refuses an engine the signed platform package did not pin by digest.
///
/// The updater verifies its archive before extraction; this second check
/// catches a corrupt or locally replaced executable before it is spawned.
/// Neither the app nor a network response gets to choose either path.
fn verify_engine(engine: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(engine)
        .map_err(|_| "Sync engine is not installed. Check Wi-Fi and try again.".to_owned())?;
    // Ownership and permission hardening are Unix checks (root-owned, not
    // group/world-writable, executable). std exposes neither owner nor mode
    // on Windows, where the daemon does not run outside development; the
    // digest pin below is the integrity check that still applies everywhere.
    #[cfg(unix)]
    let unsafe_metadata =
        metadata.mode() & 0o111 == 0 || metadata.mode() & 0o022 != 0 || metadata.uid() != 0;
    #[cfg(windows)]
    let unsafe_metadata = false;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() || unsafe_metadata {
        return Err(
            "Sync engine is unsafe or corrupt. Install the platform update again.".to_owned(),
        );
    }
    let actual = sha256(engine)?;
    if actual != ENGINE_SHA256 {
        return Err(
            "Sync engine checksum did not match. Install the platform update again.".to_owned(),
        );
    }
    Ok(())
}

#[cfg(test)]
fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn sha256(path: &Path) -> Result<String, String> {
    let file = File::open(path).map_err(|error| format!("read Sync engine: {error}"))?;
    let mut reader = BufReader::new(file);
    let mut hash = ring::digest::Context::new(&ring::digest::SHA256);
    let mut bytes = vec![0_u8; 64 * 1024];
    loop {
        let count = reader
            .read(&mut bytes)
            .map_err(|error| format!("read Sync engine: {error}"))?;
        if count == 0 {
            break;
        }
        hash.update(&bytes[..count]);
    }
    Ok(hash
        .finish()
        .as_ref()
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            let _ignored = write!(hex, "{byte:02x}");
            hex
        }))
}

fn conflict_window(app_state: &Path, progress: &WindowProgress) -> Result<(), String> {
    write_status(
        app_state,
        &Status {
            state: "conflict",
            bytes: 0,
            message: format!(
                "{} file(s) could not sync. They retry on the next window.",
                progress.conflicts
            ),
            peers: progress.online,
            conflicts: progress.conflicts,
        },
    )
}

fn complete_window(app_state: &Path, progress: &WindowProgress) -> Result<(), String> {
    let now = epoch_now();
    record_success(app_state, now)?;
    let ingest_note = match ingest(app_state, now) {
        Ok(note) => note,
        Err(detail) => format!(" Import needs attention: {detail}"),
    };
    write_status(
        app_state,
        &Status {
            state: "idle",
            bytes: 0,
            message: format!("Last sync: complete.{ingest_note}"),
            peers: progress.online,
            conflicts: 0,
        },
    )
}

fn stop(child: &mut Child, key: &str) {
    let _ignored = rest_shutdown(key);
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if child.try_wait().ok().flatten().is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    // A wedged engine must not keep the Wi-Fi window and reader awake forever.
    // The next run verifies Syncthing's own database recovery before syncing.
    let _ignored = child.kill();
    let _ignored = child.wait();
}

/// One probe across every folder and peer: how much is left, who is online,
/// and how many files the engine has given up on for now.
struct WindowProgress {
    idle: bool,
    bytes: u64,
    online: usize,
    conflicts: u64,
}

struct FolderProbe {
    idle: bool,
    need_bytes: u64,
    pull_errors: u64,
}

fn folder_state(key: &str, peers: &[Peer]) -> Result<WindowProgress, String> {
    let mut idle = true;
    let mut bytes = 0_u64;
    let mut conflicts = 0_u64;
    for (name, _) in FOLDERS {
        let probe = one_folder_state(key, name)?;
        idle = idle && probe.idle;
        bytes = bytes.saturating_add(probe.need_bytes);
        conflicts = conflicts.saturating_add(probe.pull_errors);
    }
    let mut online = 0_usize;
    for peer in peers {
        let connected = peer_connected(key, &peer.device_id)?;
        if connected {
            online += 1;
        }
        if !connected || !peer_complete(key, &peer.device_id, &peer.folder)? {
            idle = false;
            bytes = bytes.max(1);
        }
    }
    if peers.is_empty() {
        idle = false;
        bytes = bytes.max(1);
    }
    Ok(WindowProgress {
        idle,
        bytes,
        online,
        conflicts,
    })
}

fn one_folder_state(key: &str, folder: &str) -> Result<FolderProbe, String> {
    parse_folder_state(&rest_get(
        key,
        &format!("/rest/db/status?folder=kobo-{folder}"),
    )?)
}

fn peer_connected(key: &str, peer: &str) -> Result<bool, String> {
    let answer = rest_get(key, "/rest/system/connections")?;
    Ok(parse_peer_connected(&answer, peer))
}

fn parse_peer_connected(answer: &str, peer: &str) -> bool {
    let Some(after_id) = answer.split(&format!("\"{peer}\":")).nth(1) else {
        return false;
    };
    let entry = after_id.split('}').next().unwrap_or(after_id);
    entry.contains("\"connected\":true")
}

fn peer_complete(key: &str, peer: &str, folder: &str) -> Result<bool, String> {
    let answer = rest_get(
        key,
        &format!("/rest/db/completion?device={peer}&folder=kobo-{folder}"),
    )?;
    Ok(parse_peer_completion(&answer) == Some(100))
}

fn parse_peer_completion(answer: &str) -> Option<u32> {
    answer
        .split("\"completion\":")
        .nth(1)
        .and_then(|value| {
            value
                .split(|character: char| !character.is_ascii_digit())
                .next()
        })
        .and_then(|value| value.parse::<u32>().ok())
}

fn rest_get(key: &str, target: &str) -> Result<String, String> {
    let address: SocketAddr = "127.0.0.1:8384".parse().expect("fixed loopback socket");
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(2))
        .map_err(|_| "Sync engine is starting.".to_owned())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|error| format!("set Sync REST timeout: {error}"))?;
    write!(
        stream,
        "GET {target} HTTP/1.1\r\nHost: localhost\r\nX-API-Key: {key}\r\nConnection: close\r\n\r\n"
    )
    .map_err(|error| format!("query Sync REST API: {error}"))?;
    let mut answer = String::new();
    stream
        .read_to_string(&mut answer)
        .map_err(|error| format!("read Sync REST API: {error}"))?;
    if !answer.starts_with("HTTP/1.1 200") {
        return Err("Sync engine did not accept its private status request.".to_owned());
    }
    Ok(answer)
}

fn parse_folder_state(answer: &str) -> Result<FolderProbe, String> {
    let state = answer
        .split("\"state\":\"")
        .nth(1)
        .and_then(|value| value.split('"').next())
        .ok_or_else(|| "Sync engine returned an unreadable folder state.".to_owned())?;
    let number = |field: &str| -> u64 {
        answer
            .split(field)
            .nth(1)
            .and_then(|value| value.split(|c: char| !c.is_ascii_digit()).next())
            .and_then(|value| value.parse().ok())
            .unwrap_or(0)
    };
    let pull_errors = number("\"pullErrors\":");
    if state == "idle" {
        Ok(FolderProbe {
            idle: true,
            need_bytes: 0,
            pull_errors,
        })
    } else {
        Ok(FolderProbe {
            idle: false,
            need_bytes: number("\"needBytes\":").max(1),
            pull_errors,
        })
    }
}

fn rest_shutdown(key: &str) -> Result<(), String> {
    let address: SocketAddr = "127.0.0.1:8384".parse().expect("fixed loopback socket");
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(2))
        .map_err(|error| format!("connect to Sync engine: {error}"))?;
    write!(stream, "POST /rest/system/shutdown HTTP/1.1\r\nHost: localhost\r\nX-API-Key: {key}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        .map_err(|error| format!("stop Sync engine: {error}"))?;
    Ok(())
}

/// One line in the app-facing status, in the order the Sync app reads them.
/// The first three lines are the original contract; an older app ignores the
/// rest, and a newer app defaults what an older supervisor never wrote.
struct Status {
    state: &'static str,
    bytes: u64,
    message: String,
    peers: usize,
    conflicts: u64,
}

impl Status {
    fn quiet(state: &'static str, message: impl Into<String>) -> Self {
        Self {
            state,
            bytes: 0,
            message: message.into(),
            peers: 0,
            conflicts: 0,
        }
    }

    fn render(&self, last_success: u64) -> String {
        format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            self.state, self.bytes, self.message, self.peers, last_success, self.conflicts
        )
    }
}

fn write_status(root: &Path, status: &Status) -> Result<(), String> {
    fs::create_dir_all(root).map_err(|error| format!("create Sync app state: {error}"))?;
    let last_success = last_success(root);
    atomic_write(
        &root.join("sync-status"),
        &status.render(last_success),
        0o600,
    )
}

fn last_success(root: &Path) -> u64 {
    fs::read_to_string(root.join("sync-last-success"))
        .ok()
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or(0)
}

fn record_success(root: &Path, epoch: u64) -> Result<(), String> {
    fs::create_dir_all(root).map_err(|error| format!("create Sync app state: {error}"))?;
    atomic_write(&root.join("sync-last-success"), &format!("{epoch}"), 0o600)
}

fn epoch_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// Folders whose staged content becomes an application shelf once a window
/// completes: (sync folder, application data root, package marker). Vault
/// takes a `synced.v1` sidecar shelf (the app merges it next to its own),
/// Frame a whole `manifest.v1` album (the last publisher wins, exactly as
/// with the desktop publisher).
const INGEST: [(&str, &str, &str); 2] = [
    ("vault", VAULT_DATA, "synced.v1"),
    ("frame", FRAME_DATA, "manifest.v1"),
];
const INGEST_FILES: usize = 512;
const INGEST_BYTES: u64 = 1024 * 1024 * 1024;

fn ingest(app_state: &Path, epoch: u64) -> Result<String, String> {
    let mut note = String::new();
    for (folder, target, marker) in INGEST {
        let staging = Path::new(SYNC_ROOT).join(folder);
        let digest_path = app_state.join(format!("sync-ingest-{folder}.digest"));
        if let Some((files, bytes, digest)) =
            ingest_folder(&staging, Path::new(target), marker, &digest_path)?
        {
            record_ingest(app_state, folder, files, bytes, epoch)?;
            atomic_write(&digest_path, &digest, 0o600)?;
            let _ignored = write!(note, " {folder}: {files} files imported.");
        }
    }
    Ok(note)
}

/// Mirrors one staged package into an application shelf. Returns the file
/// count, byte total and content digest when anything changed, `None` when
/// the folder holds no package or the package is the one already imported.
/// Deletions never propagate here: removing from a shelf is the application's
/// own act, not the transport's.
fn ingest_folder(
    staging: &Path,
    target: &Path,
    marker: &str,
    digest_path: &Path,
) -> Result<Option<(u64, u64, String)>, String> {
    if !staging.join(marker).is_file() {
        return Ok(None);
    }
    let mut files: Vec<(String, u64, u64)> = Vec::new();
    collect(staging, staging, 0, &mut files)?;
    let mut digest: u64 = 0xcbf2_9ce4_8422_2325;
    let mut total = 0_u64;
    for (path, length, modified) in &files {
        for byte in path.bytes() {
            digest = (digest ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
        }
        digest = (digest ^ length).wrapping_mul(0x0000_0100_0000_01b3);
        digest = (digest ^ modified).wrapping_mul(0x0000_0100_0000_01b3);
        total = total.saturating_add(*length);
    }
    if total > INGEST_BYTES {
        return Err("a synced package is over the 1 GB shelf limit".to_owned());
    }
    let digest = format!("{digest:016x}");
    if fs::read_to_string(digest_path).ok().as_deref() == Some(digest.as_str()) {
        return Ok(None);
    }
    let mut copied = 0_u64;
    for (path, _, _) in &files {
        let source = staging.join(path);
        let destination = target.join(path);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|error| format!("create {parent:?}: {error}"))?;
        }
        let bytes = fs::read(&source).map_err(|error| format!("read {source:?}: {error}"))?;
        atomic_bytes(&destination, &bytes, 0o600)?;
        copied += 1;
    }
    Ok(Some((copied, total, digest)))
}

fn collect(
    root: &Path,
    directory: &Path,
    depth: u32,
    files: &mut Vec<(String, u64, u64)>,
) -> Result<(), String> {
    if depth > 8 {
        return Err("a synced package nests deeper than the shelf allows".to_owned());
    }
    let entries =
        fs::read_dir(directory).map_err(|error| format!("read {directory:?}: {error}"))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("read {directory:?}: {error}"))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "a synced file name is not UTF-8".to_owned())?;
        // Syncthing's own markers (.stfolder, .stignore) and editor debris
        // are transport details, never shelf content.
        if name.starts_with('.') {
            continue;
        }
        if name.bytes().any(|byte| byte < 0x20) {
            return Err("a synced file name holds a control character".to_owned());
        }
        let path = entry.path();
        let kind = entry
            .file_type()
            .map_err(|error| format!("stat {path:?}: {error}"))?;
        if kind.is_dir() {
            collect(root, &path, depth + 1, files)?;
        } else if kind.is_file() {
            if files.len() >= INGEST_FILES {
                return Err(format!("a synced package is over {INGEST_FILES} files"));
            }
            let metadata = entry
                .metadata()
                .map_err(|error| format!("stat {path:?}: {error}"))?;
            let relative = path
                .strip_prefix(root)
                .map_err(|_| "a synced file escaped its folder".to_owned())?
                .to_str()
                .ok_or_else(|| "a synced file name is not UTF-8".to_owned())?
                .to_owned();
            let modified = metadata
                .modified()
                .ok()
                .and_then(|when| when.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |since| since.as_secs());
            files.push((relative, metadata.len(), modified));
        }
    }
    Ok(())
}

fn record_ingest(
    app_state: &Path,
    folder: &str,
    files: u64,
    bytes: u64,
    epoch: u64,
) -> Result<(), String> {
    fs::create_dir_all(app_state).map_err(|error| format!("create Sync app state: {error}"))?;
    let path = app_state.join("sync-ingest");
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let mut lines: Vec<String> = existing
        .lines()
        .filter(|line| !line.starts_with(&format!("{folder}\t")))
        .map(str::to_owned)
        .collect();
    lines.push(format!("{folder}\t{files}\t{bytes}\t{epoch}"));
    lines.sort();
    atomic_write(&path, &lines.join("\n"), 0o600)
}

fn atomic_bytes(path: &Path, value: &[u8], mode: u32) -> Result<(), String> {
    let temporary = path.with_extension("new");
    match fs::remove_file(&temporary) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("remove stale {}: {error}", temporary.display())),
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&temporary)
        .map_err(|error| format!("write {}: {error}", temporary.display()))?;
    file.write_all(value)
        .map_err(|error| format!("write {}: {error}", temporary.display()))?;
    file.sync_all()
        .map_err(|error| format!("sync {}: {error}", temporary.display()))?;
    drop(file);
    fs::rename(&temporary, path).map_err(|error| format!("replace {}: {error}", path.display()))
}

fn read_status(root: &Path) -> String {
    fs::read_to_string(root.join("sync-status"))
        .unwrap_or_else(|_| "disabled\n0\nSync has not run.".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_ignore_the_removed_legacy_third_line() {
        assert_eq!(
            Settings::parse("true\n1\nlegacy-third-line"),
            Ok(Settings {
                enabled: true,
                cadence: Cadence::Hourly,
            })
        );
    }
    #[test]
    fn generated_configuration_distinguishes_local_and_peer_identities() {
        let local = "AAAAAAA-AAAAAAA-AAAAAAA-AAAAAAA-AAAAAAA-AAAAAAA-AAAAAAA-AAAAAAA";
        let peer = "BBBBBBB-BBBBBBB-BBBBBBB-BBBBBBB-BBBBBBB-BBBBBBB-BBBBBBB-BBBBBBB";
        let config = xml(
            local,
            &[Peer {
                folder: "vault".to_owned(),
                device_id: peer.to_owned(),
            }],
            "private",
        );
        assert!(config.contains("127.0.0.1:8384"));
        assert!(config.contains("receiveonly"));
        assert!(config.contains("sendonly"));
        assert!(config.contains("<natEnabled>false</natEnabled>"));
        assert_eq!(
            config.matches(&format!("<device id=\"{local}\"/>")).count(),
            4
        );
        assert_eq!(
            config.matches(&format!("<device id=\"{peer}\"/>")).count(),
            1
        );
        assert_eq!(
            config
                .matches(&format!("<device id=\"{peer}\" name=\"Kobo host\">"))
                .count(),
            1
        );
        assert!(!config.contains(&format!("<device id=\"{local}\" name=")));
        assert!(!config.contains("../"));
    }
    #[test]
    fn device_ids_must_be_exact_syncthing_identities() {
        assert!(valid_device_id(
            "AAAAAAA-AAAAAAA-AAAAAAA-AAAAAAA-AAAAAAA-AAAAAAA-AAAAAAA-AAAAAA2"
        ));
        assert!(!valid_device_id("legacy-third-line"));
        assert!(!valid_device_id(
            "aaaaaaa-AAAAAAA-AAAAAAA-AAAAAAA-AAAAAAA-AAAAAAA-AAAAAAA-AAAAAAA"
        ));
        assert!(!valid_device_id(
            "AAAAAAA-AAAAAAA-AAAAAAA-AAAAAAA-AAAAAAA-AAAAAAA-AAAAAAA-AAAAAA8"
        ));
    }
    #[test]
    fn scheduled_windows_do_not_run_for_manual_mode() {
        assert_eq!(Cadence::Manual.seconds(), None);
        assert_eq!(Cadence::FourHourly.seconds(), Some(14_400));
        let manual = Settings {
            enabled: true,
            cadence: Cadence::Manual,
        };
        assert!(!window_is_due(&manual, "scheduled"));
        assert!(window_is_due(&manual, "settings"));
        let paused = Settings {
            enabled: false,
            ..manual
        };
        assert!(!window_is_due(&paused, "tail"));
    }

    #[test]
    fn only_an_explicit_idle_folder_is_complete() {
        let idle = parse_folder_state(
            "HTTP/1.1 200 OK\r\n\r\n{\"state\":\"idle\",\"needBytes\":0,\"pullErrors\":2}",
        )
        .expect("idle probe");
        assert!(idle.idle);
        assert_eq!(idle.need_bytes, 0);
        assert_eq!(idle.pull_errors, 2);
        for state in [
            "starting",
            "scanning",
            "scan-waiting",
            "sync-waiting",
            "sync-preparing",
            "syncing",
            "cleaning",
            "clean-waiting",
        ] {
            let answer =
                format!("HTTP/1.1 200 OK\r\n\r\n{{\"state\":\"{state}\",\"needBytes\":0}}");
            let probe = parse_folder_state(&answer).expect("working probe");
            assert!(!probe.idle);
            assert_eq!(probe.need_bytes, 1);
        }
    }

    #[test]
    fn an_older_supervisor_status_still_reads_and_a_new_one_renders() {
        let status = Status {
            state: "running",
            bytes: 42,
            message: "Syncing folders.".to_owned(),
            peers: 1,
            conflicts: 0,
        };
        assert_eq!(status.render(99), "running\n42\nSyncing folders.\n1\n99\n0");
        let rendered = status.render(99);
        let mut lines = rendered.lines();
        assert_eq!(lines.next(), Some("running"));
        assert_eq!(lines.next(), Some("42"));
        assert_eq!(lines.next(), Some("Syncing folders."));
    }

    #[test]
    fn ingest_mirrors_a_package_once_and_never_follows_debris() {
        let temporary =
            std::env::temp_dir().join(format!("cobalt-sync-ingest-test-{}", std::process::id()));
        let _ignored = fs::remove_dir_all(&temporary);
        let staging = temporary.join("staging");
        let target = temporary.join("target");
        fs::create_dir_all(staging.join("nested")).expect("staging");
        fs::create_dir_all(&target).expect("target");
        fs::write(staging.join("synced.v1"), "{}").expect("marker");
        fs::write(staging.join("nested/note.md"), "hello").expect("note");
        fs::write(staging.join(".stfolder"), "").expect("engine marker");
        fs::write(staging.join(".trashed"), "x").expect("debris");
        let digest_path = temporary.join("digest");

        let first = ingest_folder(&staging, &target, "synced.v1", &digest_path)
            .expect("first ingest")
            .expect("package present");
        assert_eq!(first.0, 2, "marker plus note, hidden files skipped");
        assert_eq!(
            fs::read_to_string(target.join("nested/note.md")).expect("copied note"),
            "hello"
        );
        assert!(!target.join(".stfolder").exists());
        fs::write(&digest_path, &first.2).expect("record digest");

        assert!(
            ingest_folder(&staging, &target, "synced.v1", &digest_path)
                .expect("second ingest")
                .is_none(),
            "an unchanged package is not imported twice"
        );

        fs::write(staging.join("nested/second.md"), "more").expect("new note");
        let second = ingest_folder(&staging, &target, "synced.v1", &digest_path)
            .expect("changed ingest")
            .expect("changed package");
        assert_eq!(second.0, 3);

        // A staged deletion never removes what the shelf already holds.
        fs::remove_file(staging.join("nested/note.md")).expect("stage removal");
        let third =
            ingest_folder(&staging, &target, "synced.v1", &digest_path).expect("deletion ingest");
        assert!(third.is_some());
        assert!(
            target.join("nested/note.md").is_file(),
            "the shelf keeps its copy when the transport deletes"
        );
        let _ignored = fs::remove_dir_all(&temporary);
    }

    #[test]
    fn ingest_report_replaces_only_its_own_folder_line() {
        let temporary =
            std::env::temp_dir().join(format!("cobalt-sync-ingest-report-{}", std::process::id()));
        let _ignored = fs::remove_dir_all(&temporary);
        record_ingest(&temporary, "vault", 3, 30, 100).expect("vault line");
        record_ingest(&temporary, "frame", 5, 50, 200).expect("frame line");
        record_ingest(&temporary, "vault", 4, 40, 300).expect("vault refresh");
        let report = fs::read_to_string(temporary.join("sync-ingest")).expect("report");
        assert_eq!(report, "frame\t5\t50\t200\nvault\t4\t40\t300");
        let _ignored = fs::remove_dir_all(&temporary);
    }

    #[test]
    fn a_window_waits_for_the_configured_peer_to_finish() {
        let peer = "BBBBBBB-BBBBBBB-BBBBBBB-BBBBBBB-BBBBBBB-BBBBBBB-BBBBBBB-BBBBBBB";
        assert!(parse_peer_connected(
            &format!(
                "HTTP/1.1 200 OK\r\n\r\n{{\"connections\":{{\"{peer}\":{{\"connected\":true,\"type\":\"tcp-client\"}}}}}}"
            ),
            peer
        ));
        assert!(!parse_peer_connected(
            &format!(
                "HTTP/1.1 200 OK\r\n\r\n{{\"connections\":{{\"{peer}\":{{\"connected\":false}}}}}}"
            ),
            peer
        ));
        assert_eq!(
            parse_peer_completion("HTTP/1.1 200 OK\r\n\r\n{\"completion\":100,\"needBytes\":0}"),
            Some(100)
        );
        assert_eq!(
            parse_peer_completion("HTTP/1.1 200 OK\r\n\r\n{\"completion\":99.8,\"needBytes\":1}"),
            Some(99)
        );
    }

    #[test]
    fn malformed_checksum_cannot_be_accepted() {
        // The full filesystem check is device-owned; this pins the accepted
        // checksum grammar so a release cannot accidentally publish prose.
        assert!(valid_digest(&"a".repeat(64)));
        assert!(!valid_digest(&"A".repeat(64)));
        assert!(!valid_digest("not-a-checksum"));
        assert!(valid_digest(ENGINE_SHA256));
    }
}

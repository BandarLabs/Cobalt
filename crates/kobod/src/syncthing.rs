//! The small, bounded part of Syncthing Cobalt owns.
//!
//! Syncthing itself remains unmodified.  This module owns its private home,
//! loopback REST key, process lifetime, and the fixed folder policy.  It never
//! accepts a path, command, peer id, or API key from an application.

use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const APP_STATE: &str = "/mnt/onboard/.adds/cobalt/state/syncthing";
const HOME: &str = "/var/lib/cobalt/syncthing";
const ENGINE: &str = "/mnt/onboard/.adds/cobalt/bin/syncthing";
const ENGINE_SHA256: &str = "845336fa67494f38ecb69dfaa0a81de6e33e9b5427bd707385d85051596641a1";
const SYNC_ROOT: &str = "/mnt/onboard/.adds/cobalt/sync";
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
            write_status(state, "stopped\n0\nStopped by the owner.")?;
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
        write_status(app_state, "disabled\n0\nSync is off.")?;
        return Ok(());
    }
    if !window_is_due(&settings, reason) {
        write_status(app_state, "waiting\n0\nManual sync only.")?;
        return Ok(());
    }
    let home = Path::new(HOME);
    prepare(home)?;
    let mut child = match start(home) {
        Ok(child) => child,
        Err(detail) => {
            write_status(app_state, &format!("failed\n0\n{detail}"))?;
            return Ok(());
        }
    };
    write_status(app_state, "running\n0\nSyncing folders.")?;
    let deadline = Instant::now() + requested;
    let key = fs::read_to_string(home.join("api-key"))
        .map_err(|error| format!("read Sync REST key: {error}"))?;
    let peers = read_peers(home)?;
    loop {
        if !Settings::load(app_state)?.enabled {
            stop(&mut child, &key);
            write_status(app_state, "paused\n0\nPaused by the owner.")?;
            return Ok(());
        }
        if child
            .try_wait()
            .map_err(|error| format!("wait for Sync engine: {error}"))?
            .is_some()
        {
            write_status(
                app_state,
                "failed\n0\nSync engine stopped. Install the platform update again.",
            )?;
            return Ok(());
        }
        match folder_state(&key, &peers) {
            Ok(FolderState::Idle) => {
                stop(&mut child, &key);
                write_status(app_state, "idle\n0\nLast sync: complete.")?;
                return Ok(());
            }
            Ok(FolderState::Working(bytes)) => {
                write_status(app_state, &format!("running\n{bytes}\nSyncing folders."))?;
            }
            Err(detail) => write_status(app_state, &format!("running\n0\n{detail}"))?,
        }
        if Instant::now() >= deadline {
            stop(&mut child, &key);
            write_status(
                app_state,
                "timed-out\n0\nSync window ended after five minutes.",
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
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&temporary)
        .map_err(|error| format!("write {}: {error}", temporary.display()))?;
    file.write_all(value.as_bytes())
        .map_err(|error| format!("write {}: {error}", temporary.display()))?;
    file.sync_all()
        .map_err(|error| format!("sync {}: {error}", temporary.display()))?;
    fs::rename(&temporary, path).map_err(|error| format!("replace {}: {error}", path.display()))
}

fn start(home: &Path) -> Result<Child, String> {
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

/// Refuses an engine the signed platform package did not pin by digest.
///
/// The updater verifies its archive before extraction; this second check
/// catches a corrupt or locally replaced executable before it is spawned.
/// Neither the app nor a network response gets to choose either path.
fn verify_engine(engine: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(engine).map_err(|_| {
        "Sync engine not installed. Install the platform update that includes Syncthing.".to_owned()
    })?;
    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || metadata.mode() & 0o111 == 0
        || metadata.mode() & 0o022 != 0
        || metadata.uid() != 0
    {
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

enum FolderState {
    Idle,
    Working(u64),
}

fn folder_state(key: &str, peers: &[Peer]) -> Result<FolderState, String> {
    let mut transferred = 0_u64;
    for (name, _) in FOLDERS {
        match one_folder_state(key, name)? {
            FolderState::Idle => {}
            FolderState::Working(bytes) => transferred = transferred.saturating_add(bytes),
        }
    }
    for peer in peers {
        if !peer_connected(key, &peer.device_id)?
            || !peer_complete(key, &peer.device_id, &peer.folder)?
        {
            transferred = transferred.max(1);
        }
    }
    if peers.is_empty() {
        transferred = 1;
    }
    if transferred == 0 {
        Ok(FolderState::Idle)
    } else {
        Ok(FolderState::Working(transferred))
    }
}

fn one_folder_state(key: &str, folder: &str) -> Result<FolderState, String> {
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

fn parse_folder_state(answer: &str) -> Result<FolderState, String> {
    let state = answer
        .split("\"state\":\"")
        .nth(1)
        .and_then(|value| value.split('"').next())
        .ok_or_else(|| "Sync engine returned an unreadable folder state.".to_owned())?;
    if state == "idle" {
        Ok(FolderState::Idle)
    } else {
        let bytes = answer
            .split("\"needBytes\":")
            .nth(1)
            .and_then(|value| value.split(|c: char| !c.is_ascii_digit()).next())
            .and_then(|value| value.parse().ok())
            .unwrap_or(0)
            .max(1);
        Ok(FolderState::Working(bytes))
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

fn write_status(root: &Path, status: &str) -> Result<(), String> {
    fs::create_dir_all(root).map_err(|error| format!("create Sync app state: {error}"))?;
    atomic_write(&root.join("sync-status"), status, 0o600)
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
        assert!(matches!(
            parse_folder_state("HTTP/1.1 200 OK\r\n\r\n{\"state\":\"idle\",\"needBytes\":0}"),
            Ok(FolderState::Idle)
        ));
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
            assert!(matches!(
                parse_folder_state(&answer),
                Ok(FolderState::Working(1))
            ));
        }
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

//! A private host Syncthing peer for the Kobo's four fixed sync folders.

use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::env;
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const USAGE: &str = "usage: kobo sync setup LOCAL_DIR --folder vault|frame|books|out --device IP\n\
                     \x20      kobo sync run [--foreground] [--seconds 1-86400]\n\
                     \x20      kobo sync status\n\
                     \x20      kobo sync stop";
const GUI_ADDRESS: &str = "127.0.0.1:8385";
const KOBO_KOBOD: &str = "/mnt/onboard/.adds/cobalt/bin/kobod";
const REMOTE_TIMEOUT: Duration = Duration::from_secs(60);
const START_TIMEOUT: Duration = Duration::from_secs(15);
const STOP_TIMEOUT: Duration = Duration::from_secs(15);
const FOREGROUND_DEFAULT_SECONDS: u64 = 300;
const FOREGROUND_MAX_SECONDS: u64 = 86_400;

#[derive(Clone, Debug, Eq, PartialEq)]
struct Mapping {
    path: PathBuf,
    device: u64,
    inode: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct State {
    binary: PathBuf,
    binary_sha256: String,
    version: String,
    api_key: String,
    host_id: String,
    kobo_id: String,
    mappings: BTreeMap<String, Mapping>,
}

pub fn command(arguments: &[String]) -> Result<(), String> {
    match arguments.first().map(String::as_str) {
        Some("setup") => setup(&arguments[1..]),
        Some("run") => run(&arguments[1..]),
        Some("status") if arguments.len() == 1 => status(),
        Some("stop") if arguments.len() == 1 => stop(),
        _ => Err(USAGE.to_owned()),
    }
}

fn setup(arguments: &[String]) -> Result<(), String> {
    let (local, folder, device) = parse_setup(arguments)?;
    let home = host_home()?;
    protect_home(&home)?;
    let _lock = OperationLock::acquire(&home)?;
    let previous = optional_state(&home)?;
    let new_home = previous.is_none();
    if previous.is_none()
        && fs::read_dir(&home)
            .map_err(|error| format!("read dedicated Sync home: {error}"))?
            .filter_map(Result::ok)
            .any(|entry| entry.file_name() != "operation.lock")
    {
        return Err(format!(
            "{} already contains files not created by this workflow; refusing to take it over",
            home.display()
        ));
    }
    if let Some(existing) = &previous {
        if running(existing, &home)? {
            return Err(
                "the dedicated Sync peer is running; run 'kobo sync stop' first".to_owned(),
            );
        }
    }
    let local = secure_directory(Path::new(local))?;
    let binary = locate_syncthing()?;
    let version = syncthing_version(&binary)?;
    let binary_sha256 = digest_file(&binary)?;
    generate_identity(&binary, &home)?;
    let host_id = device_id(&binary, &home)?;
    let kobo_id = match remote_kobo_id(device) {
        Ok(id) => id,
        Err(error) => {
            if new_home {
                cleanup_new_home(&home);
            }
            return Err(error);
        }
    };
    let api_key = match &previous {
        Some(state) => state.api_key.clone(),
        None => generate_key()?,
    };
    let mut state = previous.unwrap_or(State {
        binary: binary.clone(),
        binary_sha256: binary_sha256.clone(),
        version: version.clone(),
        api_key,
        host_id: host_id.clone(),
        kobo_id: kobo_id.clone(),
        mappings: BTreeMap::new(),
    });
    if state.host_id != host_id || state.kobo_id != kobo_id {
        return Err(
            "the dedicated Sync identity or paired Kobo changed; remove its private home only after reviewing the existing mapping"
                .to_owned(),
        );
    }
    if state.binary != binary || state.binary_sha256 != binary_sha256 || state.version != version {
        return Err(
            "the Syncthing binary changed since this dedicated peer was created; review it, then remove the private Sync state to re-enrol it"
                .to_owned(),
        );
    }
    add_mapping(&mut state, folder, &local, &home)?;
    if let Err(error) = configure_host(&state, &home) {
        if new_home {
            cleanup_new_home(&home);
        }
        return Err(error);
    }
    if let Err(error) = write_state(&home, &state) {
        if new_home {
            cleanup_new_home(&home);
        }
        return Err(error);
    }
    pair_kobo(device, &host_id, folder)?;
    println!(
        "Sync mapping ready.\n\n  local folder  {}\n  Kobo folder   sync/{folder}\n  host mode     {}\n  Kobo device   {kobo_id}\n  host device   {host_id}\n  private home  {}\n  Syncthing     {}\n\nThe Kobo service remains owner-controlled. Open Sync on the Kobo, tap Resume Sync,\nthen run 'kobo sync run'. For an attended first test while the reader is awake:\n  kobo shell --device {device} '{KOBO_KOBOD} --syncthing window 300'",
        local.display(),
        host_folder_type(folder),
        home.display(),
        state.version
    );
    Ok(())
}

fn add_mapping(state: &mut State, folder: &str, local: &Path, home: &Path) -> Result<(), String> {
    let metadata =
        fs::metadata(local).map_err(|error| format!("inspect {}: {error}", local.display()))?;
    let mapping = Mapping {
        path: local.to_owned(),
        device: metadata.dev(),
        inode: metadata.ino(),
    };
    let already_mapped = state.mappings.contains_key(folder);
    if let Some(existing) = state.mappings.get(folder) {
        if existing != &mapping {
            return Err(format!(
                "kobo-{folder} is already mapped to {}; refusing to replace a synchronization root",
                existing.path.display()
            ));
        }
    }
    if folder == "out"
        && !already_mapped
        && fs::read_dir(local)
            .map_err(|error| format!("read receive-only directory {}: {error}", local.display()))?
            .next()
            .is_some()
    {
        return Err(
            "the host receive-only 'out' directory must be empty before its first sync; refusing to expose existing files to remote reconciliation"
                .to_owned(),
        );
    }
    if home.starts_with(local) || local.starts_with(home) {
        return Err("LOCAL_DIR must not contain or live inside the private Sync home".to_owned());
    }
    if let Some((other, existing)) = state.mappings.iter().find(|(other, existing)| {
        *other != folder && (existing.path.starts_with(local) || local.starts_with(&existing.path))
    }) {
        return Err(format!(
            "LOCAL_DIR overlaps the existing kobo-{other} root {}; Syncthing roots must be distinct",
            existing.path.display()
        ));
    }
    state.mappings.insert(folder.to_owned(), mapping);
    Ok(())
}

fn parse_setup(arguments: &[String]) -> Result<(&str, &str, &str), String> {
    let Some(local) = arguments.first() else {
        return Err(USAGE.to_owned());
    };
    let mut folder = None;
    let mut device = None;
    let mut index = 1;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--folder" => {
                folder = arguments.get(index + 1).map(String::as_str);
                index += 2;
            }
            flag if super::is_device_flag(flag) => {
                device = arguments.get(index + 1).map(String::as_str);
                index += 2;
            }
            _ => return Err(USAGE.to_owned()),
        }
    }
    let folder = folder.ok_or_else(|| USAGE.to_owned())?;
    if !matches!(folder, "vault" | "frame" | "books" | "out") {
        return Err("--folder must be vault, frame, books, or out".to_owned());
    }
    let device = device.ok_or_else(|| USAGE.to_owned())?;
    if !super::valid_device_host(device) {
        return Err("device host contains unsupported characters".to_owned());
    }
    Ok((local, folder, device))
}

fn run(arguments: &[String]) -> Result<(), String> {
    let (foreground, seconds) = parse_run(arguments)?;
    let home = host_home()?;
    protect_home(&home)?;
    let _lock = OperationLock::acquire(&home)?;
    let state = optional_state(&home)?.ok_or_else(|| {
        "Sync is not configured; run 'kobo sync setup LOCAL_DIR --folder ... --device IP' first"
            .to_owned()
    })?;
    verify_state(&state)?;
    if running(&state, &home)? {
        return Err("the dedicated Sync peer is already running".to_owned());
    }
    let mut child = start(&state, &home, false)?;
    write_pid(&home, child.id())?;
    if let Err(error) = wait_ready(&state, &mut child) {
        let _ignored = child.kill();
        let _ignored = child.wait();
        remove_pid(&home);
        return Err(error);
    }
    if !foreground {
        println!(
            "Sync peer started in the background (PID {}).\nRun 'kobo sync status' for folder state and 'kobo sync stop' to stop it.",
            child.id()
        );
        return Ok(());
    }
    println!(
        "Sync peer is running for at most {seconds} seconds; 'kobo sync stop' can end it sooner."
    );
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < deadline {
        if let Some(exit) = child
            .try_wait()
            .map_err(|error| format!("wait for Syncthing: {error}"))?
        {
            remove_pid(&home);
            return Err(format!("Syncthing exited unexpectedly with {exit}"));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    shutdown(&state)?;
    wait_child(&mut child, STOP_TIMEOUT)?;
    remove_pid(&home);
    println!("Sync peer stopped after the bounded run.");
    Ok(())
}

fn parse_run(arguments: &[String]) -> Result<(bool, u64), String> {
    let mut foreground = false;
    let mut seconds = FOREGROUND_DEFAULT_SECONDS;
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--foreground" => {
                foreground = true;
                index += 1;
            }
            "--seconds" => {
                seconds = arguments
                    .get(index + 1)
                    .ok_or_else(|| USAGE.to_owned())?
                    .parse()
                    .map_err(|_| "--seconds must be a whole number".to_owned())?;
                index += 2;
            }
            _ => return Err(USAGE.to_owned()),
        }
    }
    if !(1..=FOREGROUND_MAX_SECONDS).contains(&seconds) {
        return Err(format!(
            "--seconds must be between 1 and {FOREGROUND_MAX_SECONDS}"
        ));
    }
    if !foreground && arguments.iter().any(|argument| argument == "--seconds") {
        return Err("--seconds requires --foreground".to_owned());
    }
    Ok((foreground, seconds))
}

fn status() -> Result<(), String> {
    let home = host_home()?;
    let state = optional_state(&home)?.ok_or_else(|| {
        "Sync is not configured; run 'kobo sync setup LOCAL_DIR --folder ... --device IP' first"
            .to_owned()
    })?;
    verify_state(&state)?;
    let active = running(&state, &home)?;
    println!(
        "Dedicated Sync peer: {}\n  home         {}\n  host device  {}\n  Kobo device  {}\n  Syncthing    {}",
        if active { "running" } else { "stopped" },
        home.display(),
        state.host_id,
        state.kobo_id,
        state.version
    );
    for (folder, mapping) in &state.mappings {
        let direction = host_folder_type(folder);
        let folder_state = if active {
            rest(
                &state.api_key,
                "GET",
                &format!("/rest/db/status?folder=kobo-{folder}"),
                None,
            )
            .ok()
            .and_then(|value| {
                value
                    .get("state")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| "starting".to_owned())
        } else {
            "not running".to_owned()
        };
        println!(
            "  kobo-{folder:<6}  {direction:<11}  {folder_state:<12}  {}",
            mapping.path.display()
        );
    }
    Ok(())
}

fn stop() -> Result<(), String> {
    let home = host_home()?;
    let state = optional_state(&home)?.ok_or_else(|| "Sync is not configured".to_owned())?;
    if !running(&state, &home)? {
        remove_pid(&home);
        println!("Dedicated Sync peer is already stopped.");
        return Ok(());
    }
    shutdown(&state)?;
    let deadline = Instant::now() + STOP_TIMEOUT;
    while Instant::now() < deadline {
        if !running(&state, &home)? {
            remove_pid(&home);
            println!("Dedicated Sync peer stopped cleanly.");
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Err("Syncthing accepted shutdown but did not stop within 15 seconds".to_owned())
}

fn host_home() -> Result<PathBuf, String> {
    let home = env::var_os("HOME").ok_or("HOME is not set")?;
    Ok(PathBuf::from(home)
        .join(".config")
        .join("kobo")
        .join("syncthing"))
}

fn protect_home(home: &Path) -> Result<(), String> {
    if let Ok(metadata) = fs::symlink_metadata(home) {
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(format!(
                "refusing unsafe dedicated Sync home {}",
                home.display()
            ));
        }
    } else {
        fs::create_dir_all(home)
            .map_err(|error| format!("create dedicated Sync home {}: {error}", home.display()))?;
    }
    reject_symlink_components(home, "dedicated Sync home")?;
    fs::set_permissions(home, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("protect dedicated Sync home {}: {error}", home.display()))
}

fn secure_directory(input: &Path) -> Result<PathBuf, String> {
    if input
        .components()
        .any(|component| component == Component::ParentDir)
    {
        return Err("LOCAL_DIR must not contain '..' components".to_owned());
    }
    let absolute = if input.is_absolute() {
        input.to_owned()
    } else {
        env::current_dir()
            .map_err(|error| format!("read current directory: {error}"))?
            .join(input)
    };
    reject_symlink_components(&absolute, "LOCAL_DIR")?;
    let canonical = fs::canonicalize(&absolute)
        .map_err(|error| format!("resolve LOCAL_DIR {}: {error}", input.display()))?;
    let metadata = fs::symlink_metadata(&canonical)
        .map_err(|error| format!("inspect LOCAL_DIR {}: {error}", canonical.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("LOCAL_DIR must be a real directory, not a file or symlink".to_owned());
    }
    canonical
        .to_str()
        .ok_or_else(|| "LOCAL_DIR must be valid UTF-8 for Syncthing".to_owned())?;
    Ok(canonical)
}

fn reject_symlink_components(path: &Path, purpose: &str) -> Result<(), String> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        let metadata = fs::symlink_metadata(&current).map_err(|error| {
            format!("inspect {purpose} component {}: {error}", current.display())
        })?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "{purpose} must not pass through symlink {}",
                current.display()
            ));
        }
    }
    Ok(())
}

fn locate_syncthing() -> Result<PathBuf, String> {
    let path = env::var_os("PATH").unwrap_or_default();
    for directory in env::split_paths(&path) {
        let candidate = directory.join("syncthing");
        let Ok(canonical) = fs::canonicalize(candidate) else {
            continue;
        };
        let Ok(metadata) = fs::metadata(&canonical) else {
            continue;
        };
        if metadata.is_file()
            && metadata.permissions().mode() & 0o111 != 0
            && metadata.permissions().mode() & 0o022 == 0
        {
            return Ok(canonical);
        }
    }
    Err(
        "no safe host 'syncthing' executable was found on PATH. Install it with your OS package manager (macOS: 'brew install syncthing'; Debian/Ubuntu: 'sudo apt install syncthing'; Fedora: 'sudo dnf install syncthing'), then rerun setup. Cobalt never downloads or executes a release binary for you."
            .to_owned(),
    )
}

fn syncthing_version(binary: &Path) -> Result<String, String> {
    let output = Command::new(binary)
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("run {} --version: {error}", binary.display()))?;
    if !output.status.success() {
        return Err(format!(
            "{} --version exited with {}",
            binary.display(),
            output.status
        ));
    }
    let version = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !version.starts_with("syncthing v") || version.contains('\n') {
        return Err(format!(
            "{} did not identify itself as Syncthing",
            binary.display()
        ));
    }
    Ok(version)
}

fn digest_file(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    Ok(super::sha256::hex_digest(&bytes))
}

fn generate_identity(binary: &Path, home: &Path) -> Result<(), String> {
    if !home.join("cert.pem").is_file() || !home.join("key.pem").is_file() {
        let output = Command::new(binary)
            .arg("generate")
            .arg("--home")
            .arg(home)
            .stdin(Stdio::null())
            .output()
            .map_err(|error| format!("generate dedicated Syncthing identity: {error}"))?;
        if !output.status.success() {
            return Err(command_error(
                "generate dedicated Syncthing identity",
                &output,
            ));
        }
    }
    for name in ["cert.pem", "key.pem", "config.xml"] {
        let path = home.join(name);
        if path.exists() {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
                .map_err(|error| format!("protect {}: {error}", path.display()))?;
        }
    }
    Ok(())
}

fn device_id(binary: &Path, home: &Path) -> Result<String, String> {
    let output = Command::new(binary)
        .arg("device-id")
        .arg("--home")
        .arg(home)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("read dedicated Syncthing device ID: {error}"))?;
    if !output.status.success() {
        return Err(command_error("read dedicated Syncthing device ID", &output));
    }
    let id = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !valid_device_id(&id) {
        return Err("Syncthing returned a non-canonical device ID".to_owned());
    }
    Ok(id)
}

fn command_error(context: &str, output: &std::process::Output) -> String {
    let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if detail.is_empty() {
        format!("{context}: process exited with {}", output.status)
    } else {
        format!("{context}: {detail}")
    }
}

fn remote_kobo_id(host: &str) -> Result<String, String> {
    let output = remote(host, &format!("set -eu\n'{KOBO_KOBOD}' --syncthing id\n"))?;
    let id = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !valid_device_id(&id) {
        return Err("the Kobo returned a non-canonical Syncthing device ID".to_owned());
    }
    Ok(id)
}

fn pair_kobo(host: &str, peer_id: &str, folder: &str) -> Result<(), String> {
    let script = format!("set -eu\n'{KOBO_KOBOD}' --syncthing pair '{peer_id}' '{folder}'\n");
    let output = remote(host, &script)?;
    print!("{}", String::from_utf8_lossy(&output.stdout));
    Ok(())
}

fn remote(host: &str, script: &str) -> Result<super::RemoteShellOutput, String> {
    let output = super::run_remote_shell(&format!("root@{host}"), script, REMOTE_TIMEOUT)
        .map_err(super::unreachable_device)?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(super::unreachable_if_ssh_gave_up(
            super::remote_shell_error(
                format!("Sync setup on {host} exited with {}", output.status),
                &output.stdout,
                &output.stderr,
            ),
            &output,
        ))
    }
}

fn generate_key() -> Result<String, String> {
    let mut bytes = [0_u8; 32];
    File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .map_err(|error| format!("generate private Syncthing API key: {error}"))?;
    Ok(bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut key, byte| {
            let _ignored = write!(key, "{byte:02x}");
            key
        }))
}

fn configure_host(state: &State, home: &Path) -> Result<(), String> {
    verify_mappings(state)?;
    let mut child = start(state, home, true)?;
    if let Err(error) = wait_ready(state, &mut child) {
        let _ignored = child.kill();
        let _ignored = child.wait();
        return Err(error);
    }
    let result = configure_live(state);
    let _ignored = shutdown(state);
    let stop_result = wait_child(&mut child, STOP_TIMEOUT);
    result.and(stop_result)?;
    fs::set_permissions(home.join("config.xml"), fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("protect dedicated Syncthing config: {error}"))
}

fn configure_live(state: &State) -> Result<(), String> {
    let config = rest(&state.api_key, "GET", "/rest/config", None)?;
    let folder_template = rest(&state.api_key, "GET", "/rest/config/defaults/folder", None)?;
    let device_template = rest(&state.api_key, "GET", "/rest/config/defaults/device", None)?;
    let config = configured_json(config, &folder_template, device_template, state);
    rest(&state.api_key, "PUT", "/rest/config", Some(&config))?;
    Ok(())
}

fn configured_json(
    mut config: Value,
    folder_template: &Value,
    device_template: Value,
    state: &State,
) -> Value {
    let folders = state
        .mappings
        .iter()
        .map(|(folder, mapping)| {
            let mut entry = folder_template.clone();
            entry["id"] = json!(format!("kobo-{folder}"));
            entry["label"] = json!(format!("Kobo {folder}"));
            entry["path"] = json!(mapping.path);
            entry["type"] = json!(host_folder_type(folder));
            entry["paused"] = json!(false);
            entry["devices"] = json!([
                {"deviceID": state.host_id},
                {"deviceID": state.kobo_id}
            ]);
            entry
        })
        .collect::<Vec<_>>();
    let mut device = device_template;
    device["deviceID"] = json!(state.kobo_id);
    device["name"] = json!("Kobo");
    device["addresses"] = json!(["dynamic"]);
    device["introducer"] = json!(false);
    device["autoAcceptFolders"] = json!(false);
    config["folders"] = Value::Array(folders);
    config["devices"] = Value::Array(vec![device]);
    config["gui"]["enabled"] = json!(true);
    config["gui"]["tls"] = json!(false);
    config["gui"]["address"] = json!(GUI_ADDRESS);
    config["gui"]["apiKey"] = json!(state.api_key);
    config["options"]["globalAnnounceEnabled"] = json!(true);
    config["options"]["relaysEnabled"] = json!(true);
    config["options"]["localAnnounceEnabled"] = json!(true);
    config["options"]["natEnabled"] = json!(false);
    config["options"]["listenAddresses"] = json!(["default"]);
    config
}

fn host_folder_type(folder: &str) -> &'static str {
    if folder == "out" {
        "receiveonly"
    } else {
        "sendonly"
    }
}

fn verify_state(state: &State) -> Result<(), String> {
    let metadata = fs::symlink_metadata(&state.binary).map_err(|error| {
        format!(
            "inspect Syncthing binary {}: {error}",
            state.binary.display()
        )
    })?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o111 == 0
        || metadata.permissions().mode() & 0o022 != 0
        || digest_file(&state.binary)? != state.binary_sha256
    {
        return Err(
            "the configured Syncthing binary is missing, writable by other users, or changed; refusing to run it"
                .to_owned(),
        );
    }
    if syncthing_version(&state.binary)? != state.version {
        return Err("the configured Syncthing version changed; refusing to run it".to_owned());
    }
    verify_mappings(state)
}

fn verify_mappings(state: &State) -> Result<(), String> {
    for (folder, mapping) in &state.mappings {
        let canonical = secure_directory(&mapping.path)?;
        let metadata = fs::metadata(&canonical)
            .map_err(|error| format!("inspect {}: {error}", canonical.display()))?;
        if canonical != mapping.path
            || metadata.dev() != mapping.device
            || metadata.ino() != mapping.inode
        {
            return Err(format!(
                "the local directory for kobo-{folder} was replaced or redirected; refusing to synchronize it"
            ));
        }
    }
    Ok(())
}

fn start(state: &State, home: &Path, paused: bool) -> Result<Child, String> {
    let log_path = home.join("syncthing.log");
    if let Ok(metadata) = fs::symlink_metadata(&log_path) {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(format!(
                "refusing unsafe Syncthing log {}",
                log_path.display()
            ));
        }
    }
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(&log_path)
        .map_err(|error| format!("open private Syncthing log: {error}"))?;
    fs::set_permissions(&log_path, fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("protect private Syncthing log: {error}"))?;
    let error_log = log
        .try_clone()
        .map_err(|error| format!("open private Syncthing error log: {error}"))?;
    let mut command = Command::new(&state.binary);
    command
        .arg("serve")
        .arg("--home")
        .arg(home)
        .arg("--no-browser")
        .arg("--no-restart")
        .arg("--no-upgrade")
        .arg("--no-port-probing")
        .env("STGUIADDRESS", GUI_ADDRESS)
        .env("STGUIAPIKEY", &state.api_key)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(error_log));
    if paused {
        command.arg("--paused");
    }
    command
        .spawn()
        .map_err(|error| format!("start dedicated Syncthing peer: {error}"))
}

fn wait_ready(state: &State, child: &mut Child) -> Result<(), String> {
    let deadline = Instant::now() + START_TIMEOUT;
    while Instant::now() < deadline {
        if let Some(exit) = child
            .try_wait()
            .map_err(|error| format!("wait for Syncthing startup: {error}"))?
        {
            return Err(format!(
                "dedicated Syncthing peer exited during startup with {exit}; inspect {}",
                host_home()?.join("syncthing.log").display()
            ));
        }
        if system_id(state).as_deref() == Ok(state.host_id.as_str()) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(format!(
        "dedicated Syncthing API did not become ready at {GUI_ADDRESS}; another process may own that loopback port"
    ))
}

fn running(state: &State, home: &Path) -> Result<bool, String> {
    match system_id(state) {
        Ok(id) if id == state.host_id => Ok(true),
        Ok(_) => Err(format!(
            "another Syncthing identity is answering at {GUI_ADDRESS}; refusing to control it"
        )),
        Err(error) => match read_pid(home)? {
            Some(pid) if process_exists(pid) => Err(format!(
                "the dedicated Syncthing process {pid} is still running but its private API is unavailable: {error}"
            )),
            _ => Ok(false),
        },
    }
}

fn system_id(state: &State) -> Result<String, String> {
    let value = rest(&state.api_key, "GET", "/rest/system/status", None)?;
    value
        .get("myID")
        .and_then(Value::as_str)
        .filter(|id| valid_device_id(id))
        .map(str::to_owned)
        .ok_or_else(|| "Syncthing status omitted its device ID".to_owned())
}

fn shutdown(state: &State) -> Result<(), String> {
    rest(
        &state.api_key,
        "POST",
        "/rest/system/shutdown",
        Some(&json!({})),
    )?;
    Ok(())
}

fn wait_child(child: &mut Child, timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if child
            .try_wait()
            .map_err(|error| format!("wait for Syncthing shutdown: {error}"))?
            .is_some()
        {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ignored = child.kill();
    let _ignored = child.wait();
    Err("Syncthing did not stop cleanly within 15 seconds".to_owned())
}

fn rest(key: &str, method: &str, target: &str, body: Option<&Value>) -> Result<Value, String> {
    let address: SocketAddr = GUI_ADDRESS
        .parse()
        .expect("dedicated Syncthing address is fixed");
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(2))
        .map_err(|error| format!("connect to dedicated Syncthing API: {error}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|error| format!("set Syncthing API timeout: {error}"))?;
    let body = body.map_or_else(Vec::new, |value| value.to_string().into_bytes());
    write!(
        stream,
        "{method} {target} HTTP/1.1\r\nHost: localhost\r\nX-API-Key: {key}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .and_then(|()| stream.write_all(&body))
    .map_err(|error| format!("write Syncthing API request: {error}"))?;
    let mut answer = Vec::new();
    stream
        .read_to_end(&mut answer)
        .map_err(|error| format!("read Syncthing API response: {error}"))?;
    parse_http_json(&answer)
}

fn parse_http_json(answer: &[u8]) -> Result<Value, String> {
    let split = answer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| "Syncthing API returned an invalid HTTP response".to_owned())?;
    let headers = std::str::from_utf8(&answer[..split])
        .map_err(|_| "Syncthing API returned invalid HTTP headers".to_owned())?;
    let status = headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or_else(|| "Syncthing API returned an invalid status".to_owned())?;
    let mut body = answer[split + 4..].to_vec();
    if headers
        .lines()
        .any(|line| line.eq_ignore_ascii_case("transfer-encoding: chunked"))
    {
        body = decode_chunked(&body)?;
    }
    if !(200..300).contains(&status) {
        return Err(format!(
            "Syncthing API returned HTTP {status}: {}",
            String::from_utf8_lossy(&body).trim()
        ));
    }
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(json!({}));
    }
    serde_json::from_slice(&body)
        .map_err(|error| format!("Syncthing API returned invalid JSON: {error}"))
}

fn decode_chunked(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut input = bytes;
    let mut output = Vec::new();
    loop {
        let line_end = input
            .windows(2)
            .position(|window| window == b"\r\n")
            .ok_or_else(|| "Syncthing API returned invalid chunked data".to_owned())?;
        let size_text = std::str::from_utf8(&input[..line_end])
            .map_err(|_| "Syncthing API returned invalid chunk size".to_owned())?;
        let size = usize::from_str_radix(size_text.split(';').next().unwrap_or_default(), 16)
            .map_err(|_| "Syncthing API returned invalid chunk size".to_owned())?;
        input = &input[line_end + 2..];
        if size == 0 {
            return Ok(output);
        }
        if input.len() < size + 2 || &input[size..size + 2] != b"\r\n" {
            return Err("Syncthing API returned a truncated chunk".to_owned());
        }
        output.extend_from_slice(&input[..size]);
        input = &input[size + 2..];
    }
}

fn write_state(home: &Path, state: &State) -> Result<(), String> {
    protect_home(home)?;
    let mappings = state
        .mappings
        .iter()
        .map(|(folder, mapping)| {
            (
                folder.clone(),
                json!({
                    "path": mapping.path,
                    "device": mapping.device,
                    "inode": mapping.inode
                }),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    let value = json!({
        "version": 1,
        "binary": state.binary,
        "binarySha256": state.binary_sha256,
        "syncthingVersion": state.version,
        "apiKey": state.api_key,
        "hostDeviceId": state.host_id,
        "koboDeviceId": state.kobo_id,
        "mappings": mappings
    });
    atomic_write(&home.join("kobo-host.json"), &value.to_string(), 0o600)
}

fn read_state(home: &Path) -> Result<State, String> {
    let path = home.join("kobo-host.json");
    let metadata =
        fs::symlink_metadata(&path).map_err(|error| format!("read {}: {error}", path.display()))?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(format!("refusing unsafe Sync state {}", path.display()));
    }
    let value: Value = serde_json::from_slice(
        &fs::read(&path).map_err(|error| format!("read {}: {error}", path.display()))?,
    )
    .map_err(|error| format!("parse {}: {error}", path.display()))?;
    if value.get("version").and_then(Value::as_u64) != Some(1) {
        return Err("unsupported dedicated Sync state version".to_owned());
    }
    let text = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| format!("dedicated Sync state is missing {key}"))
    };
    let binary = PathBuf::from(text("binary")?);
    let host_id = text("hostDeviceId")?;
    let kobo_id = text("koboDeviceId")?;
    if !binary.is_absolute() || !valid_device_id(&host_id) || !valid_device_id(&kobo_id) {
        return Err("dedicated Sync state contains an invalid identity or binary".to_owned());
    }
    let mappings = value
        .get("mappings")
        .and_then(Value::as_object)
        .ok_or_else(|| "dedicated Sync state is missing mappings".to_owned())?
        .iter()
        .map(|(folder, entry)| {
            if !matches!(folder.as_str(), "vault" | "frame" | "books" | "out") {
                return Err("dedicated Sync state contains an invalid folder".to_owned());
            }
            let path = entry
                .get("path")
                .and_then(Value::as_str)
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .ok_or_else(|| "dedicated Sync state contains an invalid path".to_owned())?;
            let device = entry
                .get("device")
                .and_then(Value::as_u64)
                .ok_or_else(|| "dedicated Sync state is missing a device number".to_owned())?;
            let inode = entry
                .get("inode")
                .and_then(Value::as_u64)
                .ok_or_else(|| "dedicated Sync state is missing an inode".to_owned())?;
            Ok((
                folder.clone(),
                Mapping {
                    path,
                    device,
                    inode,
                },
            ))
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    let binary_sha256 = text("binarySha256")?;
    let version = text("syncthingVersion")?;
    let api_key = text("apiKey")?;
    if !valid_hex(&binary_sha256)
        || !valid_hex(&api_key)
        || !version.starts_with("syncthing v")
        || version.len() > 256
        || version.chars().any(char::is_control)
        || host_id == kobo_id
        || mappings.is_empty()
    {
        return Err("dedicated Sync state contains unsafe values".to_owned());
    }
    Ok(State {
        binary,
        binary_sha256,
        version,
        api_key,
        host_id,
        kobo_id,
        mappings,
    })
}

fn optional_state(home: &Path) -> Result<Option<State>, String> {
    match fs::symlink_metadata(home.join("kobo-host.json")) {
        Ok(_) => read_state(home).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("inspect dedicated Sync state: {error}")),
    }
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
    fs::set_permissions(&temporary, fs::Permissions::from_mode(mode))
        .map_err(|error| format!("protect {}: {error}", temporary.display()))?;
    fs::rename(&temporary, path).map_err(|error| format!("replace {}: {error}", path.display()))
}

fn write_pid(home: &Path, pid: u32) -> Result<(), String> {
    atomic_write(&home.join("syncthing.pid"), &format!("{pid}\n"), 0o600)
}

fn read_pid(home: &Path) -> Result<Option<u32>, String> {
    let path = home.join("syncthing.pid");
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("inspect {}: {error}", path.display())),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!("refusing unsafe Sync PID file {}", path.display()));
    }
    let value =
        fs::read_to_string(&path).map_err(|error| format!("read {}: {error}", path.display()))?;
    let pid = value
        .trim()
        .parse::<u32>()
        .map_err(|_| format!("{} contains an invalid process ID", path.display()))?;
    Ok(Some(pid))
}

fn remove_pid(home: &Path) {
    let _ignored = fs::remove_file(home.join("syncthing.pid"));
}

fn cleanup_new_home(home: &Path) {
    let Ok(entries) = fs::read_dir(home) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name() == "operation.lock" {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            let _ignored = fs::remove_dir_all(path);
        } else {
            let _ignored = fs::remove_file(path);
        }
    }
}

struct OperationLock {
    path: PathBuf,
}

impl OperationLock {
    fn acquire(home: &Path) -> Result<Self, String> {
        let path = home.join("operation.lock");
        for _attempt in 0..2 {
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
            {
                Ok(mut file) => {
                    writeln!(file, "{}", std::process::id())
                        .map_err(|error| format!("write Sync operation lock: {error}"))?;
                    return Ok(Self { path });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let pid = fs::read_to_string(&path)
                        .ok()
                        .and_then(|value| value.trim().parse::<u32>().ok());
                    if pid.is_some_and(process_exists) {
                        return Err(
                            "another 'kobo sync setup' or 'kobo sync run' is in progress"
                                .to_owned(),
                        );
                    }
                    fs::remove_file(&path)
                        .map_err(|remove| format!("remove stale Sync lock: {remove}"))?;
                }
                Err(error) => return Err(format!("create Sync operation lock: {error}")),
            }
        }
        Err("could not acquire the dedicated Sync operation lock".to_owned())
    }
}

impl Drop for OperationLock {
    fn drop(&mut self) {
        let _ignored = fs::remove_file(&self.path);
    }
}

fn process_exists(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
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

fn valid_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_root(name: &str) -> PathBuf {
        let base = env::var_os("CARGO_TARGET_DIR").map_or_else(
            || env::current_dir().expect("cwd").join("target"),
            PathBuf::from,
        );
        let path = base
            .join("sync-host-tests")
            .join(format!("{name}-{}", std::process::id()));
        let _ignored = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("test root");
        path
    }

    #[test]
    fn setup_requires_one_fixed_folder_and_safe_device() {
        let arguments = vec![
            "notes".to_owned(),
            "--folder".to_owned(),
            "vault".to_owned(),
            "--device".to_owned(),
            "192.0.2.1".to_owned(),
        ];
        assert_eq!(
            parse_setup(&arguments).expect("setup"),
            ("notes", "vault", "192.0.2.1")
        );
        let mut bad = arguments;
        bad[2] = "../vault".to_owned();
        assert!(parse_setup(&bad).is_err());
    }

    #[test]
    fn host_direction_is_the_safe_inverse_of_kobo_direction() {
        for folder in ["vault", "frame", "books"] {
            assert_eq!(host_folder_type(folder), "sendonly");
        }
        assert_eq!(host_folder_type("out"), "receiveonly");
    }

    #[test]
    fn local_roots_reject_parent_traversal_and_symlinks() {
        let root = test_root("paths");
        let real = root.join("real");
        fs::create_dir(&real).expect("real");
        assert_eq!(secure_directory(&real).expect("secure"), real);
        assert!(secure_directory(Path::new("../elsewhere")).is_err());
        let link = root.join("link");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");
        assert!(secure_directory(&link).is_err());
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn state_is_private_and_detects_replaced_roots() {
        let root = test_root("state");
        protect_home(&root).expect("home");
        let local = root.join("local");
        fs::create_dir(&local).expect("local");
        let metadata = fs::metadata(&local).expect("metadata");
        let id = "AAAAAAA-AAAAAAA-AAAAAAA-AAAAAAA-AAAAAAA-AAAAAAA-AAAAAAA-AAAAAA2";
        let state = State {
            binary: PathBuf::from("/usr/bin/syncthing"),
            binary_sha256: "a".repeat(64),
            version: "syncthing v2.0.9".to_owned(),
            api_key: "b".repeat(64),
            host_id: id.to_owned(),
            kobo_id: id.replace('A', "B"),
            mappings: BTreeMap::from([(
                "vault".to_owned(),
                Mapping {
                    path: local.clone(),
                    device: metadata.dev(),
                    inode: metadata.ino(),
                },
            )]),
        };
        write_state(&root, &state).expect("write");
        let mode = fs::metadata(root.join("kobo-host.json"))
            .expect("state metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0);
        assert_eq!(read_state(&root).expect("read"), state);
        fs::remove_dir(&local).expect("remove root");
        fs::create_dir(&local).expect("replace root");
        assert!(verify_mappings(&state).is_err());
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn parses_content_length_and_chunked_rest_answers() {
        let plain = b"HTTP/1.1 200 OK\r\nContent-Length: 13\r\n\r\n{\"state\":\"idle\"}";
        assert_eq!(parse_http_json(plain).expect("plain")["state"], "idle");
        let chunked =
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n8\r\n{\"state\"\r\n7\r\n:\"idle\"\r\n1\r\n}\r\n0\r\n\r\n";
        assert_eq!(parse_http_json(chunked).expect("chunked")["state"], "idle");
    }

    #[test]
    fn foreground_runs_are_explicitly_bounded() {
        assert_eq!(
            parse_run(&[
                "--foreground".to_owned(),
                "--seconds".to_owned(),
                "30".to_owned()
            ])
            .expect("run"),
            (true, 30)
        );
        assert!(parse_run(&["--seconds".to_owned(), "30".to_owned()]).is_err());
        assert!(parse_run(&[
            "--foreground".to_owned(),
            "--seconds".to_owned(),
            "86401".to_owned()
        ])
        .is_err());
    }

    #[test]
    fn generated_host_config_uses_exact_peers_and_safe_directions() {
        let root = test_root("config");
        let vault = root.join("vault");
        let out = root.join("out");
        fs::create_dir_all(&vault).expect("vault");
        fs::create_dir_all(&out).expect("out");
        let mapping = |path: PathBuf| {
            let metadata = fs::metadata(&path).expect("metadata");
            Mapping {
                path,
                device: metadata.dev(),
                inode: metadata.ino(),
            }
        };
        let host_id = "AAAAAAA-AAAAAAA-AAAAAAA-AAAAAAA-AAAAAAA-AAAAAAA-AAAAAAA-AAAAAA2";
        let kobo_id = "BBBBBBB-BBBBBBB-BBBBBBB-BBBBBBB-BBBBBBB-BBBBBBB-BBBBBBB-BBBBBB2";
        let state = State {
            binary: PathBuf::from("/usr/bin/syncthing"),
            binary_sha256: "a".repeat(64),
            version: "syncthing v2.0.9".to_owned(),
            api_key: "b".repeat(64),
            host_id: host_id.to_owned(),
            kobo_id: kobo_id.to_owned(),
            mappings: BTreeMap::from([
                ("out".to_owned(), mapping(out)),
                ("vault".to_owned(), mapping(vault)),
            ]),
        };
        let config = configured_json(
            json!({"gui": {}, "options": {}}),
            &json!({"rescanIntervalS": 3600}),
            json!({"compression": "metadata"}),
            &state,
        );
        assert_eq!(config["gui"]["address"], GUI_ADDRESS);
        assert_eq!(config["gui"]["apiKey"], "b".repeat(64));
        assert_eq!(config["options"]["globalAnnounceEnabled"], true);
        assert_eq!(config["options"]["relaysEnabled"], true);
        assert_eq!(config["options"]["localAnnounceEnabled"], true);
        assert_eq!(config["options"]["natEnabled"], false);
        assert_eq!(config["devices"].as_array().expect("devices").len(), 1);
        assert_eq!(config["devices"][0]["deviceID"], kobo_id);
        let folders = config["folders"].as_array().expect("folders");
        assert_eq!(folders.len(), 2);
        assert_eq!(folders[0]["id"], "kobo-out");
        assert_eq!(folders[0]["type"], "receiveonly");
        assert_eq!(folders[1]["id"], "kobo-vault");
        assert_eq!(folders[1]["type"], "sendonly");
        for folder in folders {
            assert_eq!(folder["devices"][0]["deviceID"], host_id);
            assert_eq!(folder["devices"][1]["deviceID"], kobo_id);
        }
        fs::remove_dir_all(root).expect("cleanup");
    }
}

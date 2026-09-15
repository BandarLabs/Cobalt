//! A private host Syncthing peer for the Kobo's four fixed sync folders.

use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::env;
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const USAGE: &str = "usage: kobo sync setup LOCAL_DIR --folder vault|frame|books|out --device IP\n\
                     \x20      kobo sync run [--foreground] [--seconds 1-86400]\n\
                     \x20      kobo sync plan [--json]\n\
                     \x20      kobo sync status [--json]\n\
                     \x20      kobo sync publish --folder vault|frame\n\
                     \x20      kobo sync pause\n\
                     \x20      kobo sync resume\n\
                     \x20      kobo sync stop\n\
                     \x20      \n\
                     \x20      While the peer runs it uses this computer's network. A sleeping\n\
                     \x20      reader is not kept awake; it syncs during the windows its owner\n\
                     \x20      opens on the Kobo.";
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
    if super::wants_help(arguments) {
        return super::print_command_help(USAGE);
    }
    match arguments.first().map(String::as_str) {
        Some("setup") => setup(&arguments[1..]),
        Some("run") => run(&arguments[1..]),
        Some("plan") => plan(&arguments[1..]),
        Some("status") => status(&arguments[1..]),
        Some("publish") => publish(&arguments[1..]),
        Some("pause") if arguments.len() == 1 => set_paused(true),
        Some("resume") if arguments.len() == 1 => set_paused(false),
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
        "Sync mapping ready.\n\n  local folder  {}\n  Kobo folder   sync/{folder}\n  host mode     {}\n  Kobo device   {kobo_id}\n  host device   {host_id}\n  private home  {}\n  Syncthing     {}\n\nThe Kobo service remains owner-controlled and wakes only for the windows its owner\nopens; continuous sync does not keep a sleeping reader awake. While the peer runs it\nuses this computer's network. Open Sync on the Kobo, tap Resume Sync,\nthen run 'kobo sync run'. For an attended first test while the reader is awake:\n  kobo shell --device {device} '{KOBO_KOBOD} --syncthing window 300'",
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
    let (device, inode) = dir_identity(&metadata);
    let mapping = Mapping {
        path: local.to_owned(),
        device,
        inode,
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
            "Sync peer started in the background (PID {}).\nIt uses this computer's network until stopped; it does not keep a sleeping reader awake.\nRun 'kobo sync status' for folder state, 'kobo sync pause' to pause transfers, or 'kobo sync stop' to quit.",
            child.id()
        );
        return Ok(());
    }
    println!(
        "Sync peer is running for at most {seconds} seconds; it uses this computer's network during that window and does not keep a sleeping reader awake. 'kobo sync stop' can end it sooner."
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

/// One folder as the companion sees it: direction is fixed by the folder,
/// state and diagnostics come from the daemon when it is running.
#[derive(Clone, Debug)]
struct FolderReport {
    folder: String,
    direction: &'static str,
    path: PathBuf,
    state: String,
    last_change: Option<u64>,
    errors: u64,
}

fn folder_report(state: &State, active: bool, folder: &str, mapping: &Mapping) -> FolderReport {
    let mut report = FolderReport {
        folder: (*folder).to_owned(),
        direction: host_folder_type(folder),
        path: mapping.path.clone(),
        state: "not running".to_owned(),
        last_change: None,
        errors: 0,
    };
    if !active {
        return report;
    }
    let Ok(value) = rest(
        &state.api_key,
        "GET",
        &format!("/rest/db/status?folder=kobo-{folder}"),
        None,
    ) else {
        "starting".clone_into(&mut report.state);
        return report;
    };
    value
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or("starting")
        .clone_into(&mut report.state);
    // stateChanged is when the folder last entered its current state; for an
    // idle folder that is the last completed sync pass.
    report.last_change = value
        .get("stateChanged")
        .and_then(Value::as_str)
        .and_then(rfc3339_epoch);
    report.errors = value.get("errors").and_then(Value::as_u64).unwrap_or(0)
        + value.get("pullErrors").and_then(Value::as_u64).unwrap_or(0);
    report
}

fn load_state() -> Result<(PathBuf, State), String> {
    let home = host_home()?;
    let state = optional_state(&home)?.ok_or_else(|| {
        "Sync is not configured; run 'kobo sync setup LOCAL_DIR --folder ... --device IP' first"
            .to_owned()
    })?;
    verify_state(&state)?;
    Ok((home, state))
}

fn parse_json_flag(arguments: &[String]) -> Result<bool, String> {
    match arguments {
        [] => Ok(false),
        [flag] if flag == "--json" => Ok(true),
        _ => Err(USAGE.to_owned()),
    }
}

/// The fixed ingest contract, named here so plan and status can state which
/// folders an application consumes after a window completes.
fn ingest_note(folder: &str) -> &'static str {
    match folder {
        "vault" => "imports into Vault when a synced.v1 shelf package is present",
        "frame" => "imports into Frame when a manifest.v1 album package is present",
        "books" => "stays plain files for the reader library",
        _ => "collects exports from the reader",
    }
}

fn plan_json(home: &Path, state: &State, reports: &[FolderReport]) -> Value {
    json!({
        "format": "kobo-sync-plan",
        "version": 1,
        "home": home,
        "host_id": state.host_id,
        "kobo_id": state.kobo_id,
        "syncthing": {
            "binary": state.binary,
            "sha256": state.binary_sha256,
            "version": state.version,
        },
        "gui": GUI_ADDRESS,
        "folders": reports.iter().map(|report| json!({
            "id": format!("kobo-{}", report.folder),
            "direction": report.direction,
            "path": report.path,
            "ingest": ingest_note(&report.folder),
        })).collect::<Vec<_>>(),
    })
}

fn plan(arguments: &[String]) -> Result<(), String> {
    let as_json = parse_json_flag(arguments)?;
    let (home, state) = load_state()?;
    let reports = state
        .mappings
        .iter()
        .map(|(folder, mapping)| folder_report(&state, false, folder, mapping))
        .collect::<Vec<_>>();
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&plan_json(&home, &state, &reports))
                .map_err(|error| format!("render plan: {error}"))?
        );
        return Ok(());
    }
    println!(
        "Sync plan\n  home         {}\n  host device  {}\n  Kobo device  {}\n  Syncthing    {} ({}, sha256 {})\n  API          http://{}",
        home.display(),
        state.host_id,
        state.kobo_id,
        state.version,
        state.binary.display(),
        &state.binary_sha256[..12],
        GUI_ADDRESS
    );
    for report in &reports {
        println!(
            "  kobo-{:<6}  {:<12}  {} - {}",
            report.folder,
            report.direction,
            report.path.display(),
            ingest_note(&report.folder)
        );
    }
    Ok(())
}

fn status_json(home: &Path, state: &State, active: bool, reports: &[FolderReport]) -> Value {
    json!({
        "format": "kobo-sync-status",
        "version": 1,
        "running": active,
        "home": home,
        "host_id": state.host_id,
        "kobo_id": state.kobo_id,
        "syncthing": state.version,
        "folders": reports.iter().map(|report| json!({
            "id": format!("kobo-{}", report.folder),
            "direction": report.direction,
            "path": report.path,
            "state": report.state,
            "last_change": report.last_change,
            "errors": report.errors,
        })).collect::<Vec<_>>(),
    })
}

fn status(arguments: &[String]) -> Result<(), String> {
    let as_json = parse_json_flag(arguments)?;
    let (home, state) = load_state()?;
    let active = running(&state, &home)?;
    let reports = state
        .mappings
        .iter()
        .map(|(folder, mapping)| folder_report(&state, active, folder, mapping))
        .collect::<Vec<_>>();
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&status_json(&home, &state, active, &reports))
                .map_err(|error| format!("render status: {error}"))?
        );
        return Ok(());
    }
    println!(
        "Dedicated Sync peer: {}\n  home         {}\n  host device  {}\n  Kobo device  {}\n  Syncthing    {}",
        if active { "running" } else { "stopped" },
        home.display(),
        state.host_id,
        state.kobo_id,
        state.version
    );
    for report in &reports {
        let last = report
            .last_change
            .map_or_else(|| "-".to_owned(), |epoch| epoch.to_string());
        println!(
            "  kobo-{:<6}  {:<11}  {:<12}  last change {:<12}  errors {:<3}  {}",
            report.folder,
            report.direction,
            report.state,
            last,
            report.errors,
            report.path.display()
        );
    }
    Ok(())
}

/// Pausing suspends transfers with the paired Kobo without stopping the
/// dedicated peer; the reader's own window keeps its independent cadence.
fn set_paused(paused: bool) -> Result<(), String> {
    let (home, state) = load_state()?;
    if !running(&state, &home)? {
        return Err("the dedicated Sync peer is not running".to_owned());
    }
    let action = if paused { "pause" } else { "resume" };
    rest(
        &state.api_key,
        "POST",
        &format!("/rest/system/{action}?device={}", state.kobo_id),
        None,
    )?;
    println!(
        "Sync with the Kobo {}. Resume with 'kobo sync {}'.",
        if paused { "paused" } else { "resumed" },
        if paused { "resume" } else { "pause" }
    );
    Ok(())
}

/// Parses the RFC 3339 timestamps Syncthing reports into epoch seconds.
/// Only the calendar prefix is read; fractional seconds and the zone are
/// accepted in the shapes Syncthing emits (Z or a numeric offset).
fn rfc3339_epoch(value: &str) -> Option<u64> {
    let bytes = value.as_bytes();
    if bytes.len() < 20 || bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' {
        return None;
    }
    let number = |from: usize, to: usize| -> Option<i64> { value.get(from..to)?.parse().ok() };
    let year = number(0, 4)?;
    let month = number(5, 7)?;
    let day = number(8, 10)?;
    let hour = number(11, 13)?;
    let minute = number(14, 16)?;
    let second = number(17, 19)?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 {
        return None;
    }
    // Days-from-civil (Howard Hinnant's algorithm).
    let shifted = if month <= 2 { year - 1 } else { year };
    let era = shifted.div_euclid(400);
    let year_of_era = shifted.rem_euclid(400);
    let month_prime = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * month_prime + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    let mut epoch = days * 86_400 + hour * 3_600 + minute * 60 + second;
    match bytes.get(19) {
        Some(b'Z' | b'.') => {}
        Some(b'+' | b'-') if bytes.len() >= 25 => {
            let offset = number(20, 22)? * 3_600 + number(23, 25)? * 60;
            epoch -= if bytes[19] == b'+' { offset } else { -offset };
        }
        _ => return None,
    }
    u64::try_from(epoch).ok()
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
    // An explicit root keeps tests and parallel configurations away from the
    // owner's real Sync home. It must be absolute so every later relative
    // write stays inside it.
    if let Some(root) = env::var_os("KOBO_SYNC_HOME") {
        let root = PathBuf::from(root);
        if !root.is_absolute() {
            return Err("KOBO_SYNC_HOME must be an absolute path".to_owned());
        }
        return Ok(root);
    }
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
    #[cfg(unix)]
    fs::set_permissions(home, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("protect dedicated Sync home {}: {error}", home.display()))?;
    Ok(())
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
    Ok(simplify_verbatim(canonical))
}

/// Windows `canonicalize` returns verbatim `\\?\` paths, which compare
/// unequal to the same path written the ordinary way and print awkwardly in
/// errors. Strip the prefix for drive-letter paths; genuine UNC paths keep
/// it, since a bare `\\server\share` loses the distinction.
#[cfg(windows)]
fn simplify_verbatim(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy().into_owned();
    if let Some(rest) = text.strip_prefix("\\?\\") {
        let mut chars = rest.chars();
        if chars
            .next()
            .is_some_and(|drive| drive.is_ascii_alphabetic())
            && chars.next() == Some(':')
        {
            return PathBuf::from(rest);
        }
    }
    path
}

/// No verbatim prefix exists off Windows.
#[cfg(not(windows))]
fn simplify_verbatim(path: PathBuf) -> PathBuf {
    path
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
        // Windows has no executable bit; a regular file found on the
        // search path is the engine candidate there.
        if metadata.is_file() && executable_engine(&metadata) {
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
            #[cfg(unix)]
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
    #[cfg(unix)]
    fs::set_permissions(home.join("config.xml"), fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("protect dedicated Syncthing config: {error}"))?;
    Ok(())
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
    // Windows has no mode bits; the symlink check and the pinned digest
    // still guard the engine there, so the boundary narrows but holds.
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || !executable_engine(&metadata)
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
        let (device, inode) = dir_identity(&metadata);
        if canonical != mapping.path || device != mapping.device || inode != mapping.inode {
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
    #[allow(unused_mut)] // Windows has no mode bits to set.
    let mut log_options = OpenOptions::new();
    log_options.create(true).append(true);
    #[cfg(unix)]
    log_options.mode(0o600);
    let log = log_options
        .open(&log_path)
        .map_err(|error| format!("open private Syncthing log: {error}"))?;
    #[cfg(unix)]
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
    if !metadata.is_file() || metadata.file_type().is_symlink() || !private_mode(&metadata) {
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

fn atomic_write(
    path: &Path,
    value: &str,
    #[cfg_attr(not(unix), allow(unused))] mode: u32,
) -> Result<(), String> {
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
    let mut file = options
        .open(&temporary)
        .map_err(|error| format!("write {}: {error}", temporary.display()))?;
    file.write_all(value.as_bytes())
        .map_err(|error| format!("write {}: {error}", temporary.display()))?;
    file.sync_all()
        .map_err(|error| format!("sync {}: {error}", temporary.display()))?;
    #[cfg(unix)]
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
            #[allow(unused_mut)] // Windows has no mode bits to set.
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            options.mode(0o600);
            match options.open(&path) {
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

/// Packs the raw files in a mapped folder into the exact package the
/// reader's ingest mirrors onto an application shelf: Vault takes a
/// `synced.v1` manifest beside `synced-note-*.md` files, Frame a
/// `manifest.v1` album. Publishing is the host half of the ingest contract;
/// the daemon then carries the package like any other file.
fn publish(arguments: &[String]) -> Result<(), String> {
    let folder = match arguments {
        [flag, folder] if flag == "--folder" => folder.as_str(),
        _ => return Err(USAGE.to_owned()),
    };
    let (_home, state) = load_state()?;
    let mapping = state.mappings.get(folder).ok_or_else(|| {
        format!("folder {folder} is not mapped; run 'kobo sync setup LOCAL_DIR --folder {folder} --device IP' first")
    })?;
    match folder {
        "vault" => publish_vault(&mapping.path),
        "frame" => publish_frame(&mapping.path),
        _ => Err(format!(
            "folder {folder} syncs as plain files; no application package is needed"
        )),
    }
}

/// Repacking is idempotent: the previous package files are excluded from the
/// walk, planned against, then rewritten only where content moved.
fn publish_vault(folder: &Path) -> Result<(), String> {
    use kobo_vault_host::{
        plan_prefixed, Manifest, Push, MAX_NOTES, SYNCED_MANIFEST, SYNCED_PREFIX,
    };
    let manifest_path = folder.join(SYNCED_MANIFEST);
    let existing = match fs::read(&manifest_path) {
        Ok(bytes) => Manifest::decode(&bytes)
            .map_err(|error| format!("existing {SYNCED_MANIFEST} in the sync folder: {error}"))?,
        Err(_) => Manifest {
            notes: Vec::new(),
            failures: Vec::new(),
        },
    };
    let excludes = [SYNCED_MANIFEST.to_owned(), SYNCED_PREFIX.to_owned()];
    let walk = super::vault::walk(folder, &excludes)?;
    let offered = walk.offered.len();
    let push = plan_prefixed(
        &existing,
        walk.offered.clone(),
        walk.failures.clone(),
        SYNCED_PREFIX,
    )?;
    super::vault::print_plan(&push, &walk);
    if push.manifest.notes.len() > MAX_NOTES {
        return Err(format!("Vault holds at most {MAX_NOTES} synced notes"));
    }
    for prepared in &push.notes {
        let name = Push::note_name(&prepared.note_id);
        atomic_write(
            &folder.join(&name),
            &String::from_utf8_lossy(&prepared.markdown),
            0o600,
        )?;
    }
    for removed in &push.removed {
        let _ignored = fs::remove_file(folder.join(Push::note_name(&removed.id)));
    }
    let encoded = push.manifest.encode();
    atomic_write(
        &manifest_path,
        std::str::from_utf8(&encoded).map_err(|_| "the packed shelf is not UTF-8")?,
        0o600,
    )?;
    println!(
        "Packed {offered} note(s) into {SYNCED_MANIFEST}; the next sync window carries the package and Vault imports it."
    );
    Ok(())
}

/// Packs the images in the mapped frame folder into a whole `manifest.v1`
/// album, prepared for the reader's default panel. The shelf lives inside
/// the source folder, so the walk skips its own output: the manifest, the
/// sidecars and every photo the manifest already lists. Mirror semantics
/// hold for the owner's sources - a removed photo leaves the album - and
/// owner files are never modified.
fn publish_frame(folder: &Path) -> Result<(), String> {
    use kobo_frame_host::{
        prepare_for_panel_excluding, Fit, Manifest, DEFAULT_PANEL, DIGEST_MANIFEST, FIT_MANIFEST,
        MANIFEST, MAX_FRAME_CAPACITY,
    };
    let existing = match fs::read(folder.join(MANIFEST)) {
        Ok(bytes) => Manifest::decode(&bytes)
            .map_err(|error| format!("existing {MANIFEST} in the sync folder: {error}"))?,
        Err(_) => Manifest { photos: Vec::new() },
    };
    let mut exclude = existing
        .photos
        .iter()
        .map(kobo_frame_host::Photo::shelf_name)
        .collect::<std::collections::BTreeSet<_>>();
    for sidecar in [MANIFEST, DIGEST_MANIFEST, FIT_MANIFEST] {
        exclude.insert(sidecar.to_owned());
    }
    let push =
        prepare_for_panel_excluding(folder, Fit::Crop, &existing, true, DEFAULT_PANEL, &exclude)?;
    let mut total = 0_usize;
    for photo in &push.manifest.photos {
        total = total.saturating_add(match fs::metadata(folder.join(photo.shelf_name())) {
            Ok(metadata) => usize::try_from(metadata.len()).unwrap_or(usize::MAX),
            Err(_) => push
                .photos
                .iter()
                .find(|prepared| prepared.photo.id == photo.id)
                .and_then(|prepared| prepared.png.as_ref())
                .map_or(0, Vec::len),
        });
    }
    if total > MAX_FRAME_CAPACITY {
        return Err(format!(
            "this would use {} MB for Frame photos; its capacity is {} MB",
            total / (1024 * 1024),
            MAX_FRAME_CAPACITY / (1024 * 1024)
        ));
    }
    for prepared in push.photos.iter().filter(|prepared| prepared.png.is_some()) {
        let name = prepared.photo.shelf_name();
        let partial = folder.join(format!(".{name}.writing"));
        fs::write(&partial, prepared.png.as_ref().expect("filtered"))
            .map_err(|error| format!("write Frame photo {name}: {error}"))?;
        fs::rename(&partial, folder.join(&name))
            .map_err(|error| format!("publish Frame photo {name}: {error}"))?;
    }
    let partial = folder.join(format!(".{MANIFEST}.writing"));
    fs::write(&partial, push.manifest.encode())
        .map_err(|error| format!("write Frame manifest: {error}"))?;
    fs::rename(&partial, folder.join(MANIFEST))
        .map_err(|error| format!("publish Frame manifest: {error}"))?;
    for photo in &push.removed {
        let _ignored = fs::remove_file(folder.join(photo.shelf_name()));
    }
    println!(
        "Packed {} photo(s) into {MANIFEST} for the {}x{} panel; the next sync window carries the album and Frame adopts it.",
        push.manifest.photos.len(),
        DEFAULT_PANEL.width,
        DEFAULT_PANEL.height
    );
    Ok(())
}

#[cfg(unix)]
fn dir_identity(metadata: &fs::Metadata) -> (u64, u64) {
    (metadata.dev(), metadata.ino())
}

/// Windows std has no stable device/inode identity, so the sync-root guard
/// relies on the canonical path there: redirection is still refused, but a
/// delete-and-recreate at the same path is not detected. Documented weaker
/// boundary.
#[cfg(not(unix))]
fn dir_identity(_metadata: &fs::Metadata) -> (u64, u64) {
    (0, 0)
}

#[cfg(unix)]
fn executable_engine(metadata: &fs::Metadata) -> bool {
    metadata.permissions().mode() & 0o111 != 0 && metadata.permissions().mode() & 0o022 == 0
}

#[cfg(not(unix))]
fn executable_engine(_metadata: &fs::Metadata) -> bool {
    true
}

#[cfg(unix)]
fn private_mode(metadata: &fs::Metadata) -> bool {
    metadata.permissions().mode().trailing_zeros() >= 6
}

/// The dedicated Sync home lives under the account profile on Windows, whose
/// ACL already scopes it to the account.
#[cfg(not(unix))]
fn private_mode(_metadata: &fs::Metadata) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_epoch_reads_syncthing_timestamps() {
        assert_eq!(rfc3339_epoch("1970-01-01T00:00:00Z"), Some(0));
        // Numeric zones shift the instant; a +05:30 stamp is 5.5 hours earlier.
        assert_eq!(
            rfc3339_epoch("2026-09-17T07:39:00+05:30"),
            rfc3339_epoch("2026-09-17T02:09:00Z")
        );
        assert_eq!(
            rfc3339_epoch("2026-09-17T02:09:00.123456789Z"),
            rfc3339_epoch("2026-09-17T02:09:00Z")
        );
        assert_eq!(rfc3339_epoch("not a time"), None);
        assert_eq!(rfc3339_epoch("2026-13-17T02:09:00Z"), None);
    }

    #[test]
    fn plan_marks_the_ingest_contract_per_folder() {
        assert!(ingest_note("vault").contains("synced.v1"));
        assert!(ingest_note("frame").contains("manifest.v1"));
        assert!(!ingest_note("books").contains("import"));
    }

    #[test]
    fn folder_report_is_honest_when_the_daemon_is_down() {
        let state = State {
            binary: PathBuf::from("/bin/false"),
            binary_sha256: "0".repeat(64),
            version: "v2.0.9".to_owned(),
            api_key: "key".to_owned(),
            host_id: "HOST".to_owned(),
            kobo_id: "KOBO".to_owned(),
            mappings: BTreeMap::new(),
        };
        let mapping = Mapping {
            path: PathBuf::from("/tmp/notes"),
            device: 1,
            inode: 1,
        };
        let report = folder_report(&state, false, "vault", &mapping);
        assert_eq!(report.state, "not running");
        assert_eq!(report.direction, "sendonly");
        assert_eq!(report.last_change, None);
        assert_eq!(report.errors, 0);
    }

    #[test]
    fn explicit_sync_home_root_wins_and_must_be_absolute() {
        // No other test in this binary reads KOBO_SYNC_HOME.
        env::set_var("KOBO_SYNC_HOME", "/tmp/cobalt-sync-home-test");
        assert_eq!(
            host_home().expect("absolute root"),
            PathBuf::from("/tmp/cobalt-sync-home-test")
        );
        env::set_var("KOBO_SYNC_HOME", "relative");
        assert!(host_home().is_err());
        env::remove_var("KOBO_SYNC_HOME");
    }

    #[test]
    fn host_folder_directions_protect_owner_originals() {
        assert_eq!(host_folder_type("vault"), "sendonly");
        assert_eq!(host_folder_type("frame"), "sendonly");
        assert_eq!(host_folder_type("books"), "sendonly");
        assert_eq!(host_folder_type("out"), "receiveonly");
    }

    #[test]
    fn publish_vault_packs_raw_notes_and_unpublishes_removed_ones() {
        let root =
            std::env::temp_dir().join(format!("cobalt-sync-publish-test-{}", std::process::id()));
        fs::create_dir_all(&root).expect("root");
        fs::write(root.join("Alpha.md"), "# Alpha\nsee [[Beta]]").expect("note a");
        fs::write(root.join("Beta.md"), "Beta body").expect("note b");
        publish_vault(&root).expect("first publish");
        let manifest =
            kobo_vault_host::Manifest::decode(&fs::read(root.join("synced.v1")).expect("manifest"))
                .expect("decode");
        assert_eq!(manifest.notes.len(), 2);
        assert!(manifest
            .notes
            .iter()
            .all(|note| note.id.starts_with("synced-note-")));
        assert_eq!(
            manifest
                .notes
                .iter()
                .find(|note| note.path == "Alpha.md")
                .expect("alpha")
                .links,
            vec!["Beta"]
        );
        for note in &manifest.notes {
            assert!(root.join(format!("{}.md", note.id)).is_file());
        }
        // A second publish with unchanged inputs changes nothing.
        publish_vault(&root).expect("idempotent publish");
        let again =
            kobo_vault_host::Manifest::decode(&fs::read(root.join("synced.v1")).expect("manifest"))
                .expect("decode");
        assert_eq!(again.notes, manifest.notes);
        // Removing a raw note drops its package file; every other raw note
        // stays exactly where the owner put it.
        let dropped = manifest
            .notes
            .iter()
            .find(|note| note.path == "Alpha.md")
            .expect("alpha")
            .id
            .clone();
        fs::remove_file(root.join("Alpha.md")).expect("remove raw");
        publish_vault(&root).expect("third publish");
        let final_manifest =
            kobo_vault_host::Manifest::decode(&fs::read(root.join("synced.v1")).expect("manifest"))
                .expect("decode");
        assert_eq!(final_manifest.notes.len(), 1);
        assert!(!root.join(format!("{dropped}.md")).exists());
        assert!(root.join("Beta.md").is_file());
        fs::remove_dir_all(root).expect("cleanup");
    }

    // A valid 1x1 PNG.
    const TINY_PNG: [u8; 67] = [
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f,
        0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn publish_frame_packs_an_album_and_never_reingests_its_own_output() {
        let root =
            std::env::temp_dir().join(format!("cobalt-sync-frame-test-{}", std::process::id()));
        fs::create_dir_all(&root).expect("root");
        fs::write(root.join("photo-one.png"), TINY_PNG).expect("photo");
        publish_frame(&root).expect("first publish");
        let manifest = kobo_frame_host::Manifest::decode(
            &fs::read(root.join("manifest.v1")).expect("manifest"),
        )
        .expect("decode");
        assert_eq!(manifest.photos.len(), 1);
        assert!(root.join(manifest.photos[0].shelf_name()).is_file());
        // A second publish walks the same folder, skips its own shelf and
        // changes nothing.
        publish_frame(&root).expect("idempotent publish");
        let again = kobo_frame_host::Manifest::decode(
            &fs::read(root.join("manifest.v1")).expect("manifest"),
        )
        .expect("decode");
        assert_eq!(again.photos.len(), 1);
        assert_eq!(again.photos[0].id, manifest.photos[0].id);
        // Removing the source drops the photo and its shelf file; the raw
        // file was never modified.
        assert_eq!(fs::read(root.join("photo-one.png")).expect("raw"), TINY_PNG);
        fs::remove_file(root.join("photo-one.png")).expect("remove source");
        publish_frame(&root).expect("mirror publish");
        let final_manifest = kobo_frame_host::Manifest::decode(
            &fs::read(root.join("manifest.v1")).expect("manifest"),
        )
        .expect("decode");
        assert!(final_manifest.photos.is_empty());
        assert!(!root.join(manifest.photos[0].shelf_name()).exists());
        fs::remove_dir_all(root).expect("cleanup");
    }

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
    fn help_succeeds() {
        super::command(&["--help".into()]).expect("help");
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
        #[cfg(unix)]
        {
            let link = root.join("link");
            std::os::unix::fs::symlink(&real, &link).expect("symlink");
            assert!(secure_directory(&link).is_err());
        }
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
            // "/usr/bin/syncthing" is not absolute on Windows; the test
            // binary itself is an absolute path on both.
            #[cfg(unix)]
            binary: PathBuf::from("/usr/bin/syncthing"),
            #[cfg(windows)]
            binary: std::env::current_exe().expect("test binary"),
            binary_sha256: "a".repeat(64),
            version: "syncthing v2.0.9".to_owned(),
            api_key: "b".repeat(64),
            host_id: id.to_owned(),
            kobo_id: id.replace('A', "B"),
            mappings: BTreeMap::from([{
                let (device, inode) = dir_identity(&metadata);
                (
                    "vault".to_owned(),
                    Mapping {
                        path: local.clone(),
                        device,
                        inode,
                    },
                )
            }]),
        };
        write_state(&root, &state).expect("write");
        #[cfg(unix)]
        {
            let mode = fs::metadata(root.join("kobo-host.json"))
                .expect("state metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0);
        }
        assert_eq!(read_state(&root).expect("read"), state);
        #[cfg(unix)]
        {
            fs::remove_dir(&local).expect("remove root");
            let decoy = root.join("decoy");
            fs::create_dir(&decoy).expect("decoy");
            std::os::unix::fs::symlink(&decoy, &local).expect("replace root with a symlink");
            assert!(verify_mappings(&state).is_err());
        }
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
            let (device, inode) = dir_identity(&metadata);
            Mapping {
                path,
                device,
                inode,
            }
        };
        let host_id = "AAAAAAA-AAAAAAA-AAAAAAA-AAAAAAA-AAAAAAA-AAAAAAA-AAAAAAA-AAAAAA2";
        let kobo_id = "BBBBBBB-BBBBBBB-BBBBBBB-BBBBBBB-BBBBBBB-BBBBBBB-BBBBBBB-BBBBBB2";
        let state = State {
            #[cfg(unix)]
            binary: PathBuf::from("/usr/bin/syncthing"),
            #[cfg(windows)]
            binary: std::env::current_exe().expect("test binary"),
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

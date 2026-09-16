//! Host-side bridge from a local Fugleramme/BirdNET-Go station to the Birds app.

use serde_json::{json, Value};
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const ROOT: &str = "/mnt/onboard/.adds/cobalt/data/birds";
const SNAPSHOT: &str = "current.json";
const IMAGE: &str = "current.png";
const MAX_JSON: usize = 64 * 1024;
const MAX_IMAGE: usize = 4 * 1024 * 1024;
const USAGE: &str = "usage: kobo birds listen --source http://HOST:PORT (--device IP | --sim) [--interval SECONDS]\n\
                     \x20      kobo birds status\n\
                     \x20      kobo birds stop\n\
                     \x20      kobo birds push SNAPSHOT.json IMAGE.png (--device IP | --sim)\n\
                     \x20      kobo birds _worker --source URL --device IP --interval SECONDS\n\
                     macOS and Linux only. --source is Fugleramme's local web endpoint; BirdNET-Go owns the microphone and model.";

pub fn command(arguments: &[String]) -> Result<(), String> {
    if super::wants_help(arguments) {
        return super::print_command_help(USAGE);
    }
    match arguments.first().map(String::as_str) {
        Some("listen") => listen(&arguments[1..]),
        Some("status") if arguments.len() == 1 => status(),
        Some("stop") if arguments.len() == 1 => stop(),
        Some("push") => push_command(&arguments[1..]),
        Some("_worker") => worker(&arguments[1..]),
        _ => Err(USAGE.to_owned()),
    }
}

fn home() -> Result<PathBuf, String> {
    let base = env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|v| PathBuf::from(v).join(".local/state")))
        .ok_or("HOME is not set")?;
    Ok(base.join("cobalt/birds"))
}
fn pid_path() -> Result<PathBuf, String> {
    Ok(home()?.join("listener.pid"))
}
fn log_path() -> Result<PathBuf, String> {
    Ok(home()?.join("listener.log"))
}

#[derive(Debug, Eq, PartialEq)]
struct Listen {
    source: String,
    target: Target,
    interval: u64,
}
fn parse_listen(args: &[String]) -> Result<Listen, String> {
    let mut source = None;
    let mut target = None;
    let mut interval = 5;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--source" => {
                source = args.get(i + 1).cloned();
                i += 2;
            }
            f if super::is_device_flag(f) => {
                let host = args.get(i + 1).cloned().ok_or(USAGE)?;
                if !super::valid_device_host(&host) {
                    return Err("device host contains unsupported characters".into());
                }
                if target.is_some() {
                    return Err(USAGE.into());
                }
                target = Some(Target::Device(host));
                i += 2;
            }
            "--sim" => {
                if target.is_some() {
                    return Err(USAGE.into());
                }
                target = Some(Target::Sim);
                i += 1;
            }
            "--interval" => {
                interval = args
                    .get(i + 1)
                    .ok_or(USAGE)?
                    .parse()
                    .map_err(|_| "--interval must be 2 to 3600 seconds")?;
                i += 2;
            }
            _ => return Err(USAGE.into()),
        }
    }
    if !(2..=3600).contains(&interval) {
        return Err("--interval must be 2 to 3600 seconds".into());
    }
    let source = source.ok_or(USAGE)?.trim_end_matches('/').to_owned();
    parse_http(&source)?;
    Ok(Listen {
        source,
        target: target.ok_or(USAGE)?,
        interval,
    })
}

fn listen(args: &[String]) -> Result<(), String> {
    let options = parse_listen(args)?;
    fs::create_dir_all(home()?).map_err(|e| format!("create Birds state: {e}"))?;
    // The running check, spawn and PID write must be one critical section:
    // two concurrent listens may otherwise both spawn workers and overwrite
    // the single PID file, leaving one worker untracked.
    let _lock = OperationLock::acquire(&home()?)?;
    if running()? {
        return Err("Birds listener is already running; use 'kobo birds status'".into());
    }
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path()?)
        .map_err(|e| format!("open Birds log: {e}"))?;
    let interval = options.interval.to_string();
    let mut command = Command::new(env::current_exe().map_err(|e| format!("locate kobo: {e}"))?);
    command.args(["birds", "_worker", "--source", &options.source]);
    match &options.target {
        Target::Device(host) => {
            command.args(["--device", host]);
        }
        Target::Sim => {
            command.arg("--sim");
        }
    }
    let child = command
        .args(["--interval", &interval])
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone().map_err(|e| e.to_string())?))
        .stderr(Stdio::from(log))
        .spawn()
        .map_err(|e| format!("start Birds listener: {e}"))?;
    atomic_write(&pid_path()?, format!("{}\n", child.id()).as_bytes())?;
    println!("Birds listener started (PID {}). BirdNET-Go inference remains on this computer through Fugleramme.",child.id());
    Ok(())
}
fn status() -> Result<(), String> {
    match read_pid()? {
        Some(pid) if process_alive(pid) => {
            println!(
                "Birds listener is running (PID {pid}).\nLog: {}",
                log_path()?.display()
            );
            Ok(())
        }
        _ => Err("Birds listener is not running".into()),
    }
}
fn stop() -> Result<(), String> {
    let Some(pid) = read_pid()? else {
        return Err("Birds listener is not running".into());
    };
    if process_alive(pid) {
        // A PID alone is not identity: the worker may have exited and the OS
        // may have reused the number, so SIGTERM could hit a stranger.
        if !worker_process(pid) {
            return Err(format!(
                "PID {pid} is not the Birds listener; the PID file is stale. Remove {} by hand if no listener is running.",
                pid_path()?.display()
            ));
        }
        let s = Command::new("kill")
            .arg(pid.to_string())
            .status()
            .map_err(|e| format!("stop Birds listener: {e}"))?;
        if !s.success() {
            return Err("the Birds listener did not stop".into());
        }
        // Wait for this worker to exit before clearing the PID file, so a
        // following listen cannot run alongside the old worker.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while process_alive(pid) {
            if std::time::Instant::now() >= deadline {
                return Err(
                    "the Birds listener accepted SIGTERM but did not exit within 5 seconds".into(),
                );
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    let _ = fs::remove_file(pid_path()?);
    println!("Birds listener stopped.");
    Ok(())
}

/// The PID must name this binary running the Birds worker, checked through
/// ps so it holds on both macOS and Linux.
fn worker_process(pid: u32) -> bool {
    Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "args="])
        .output()
        .map(|o| {
            o.status.success() && {
                let args = String::from_utf8_lossy(&o.stdout);
                args.contains("birds") && args.contains("_worker")
            }
        })
        .unwrap_or(false)
}

/// Serializes listener start/stop across concurrent invocations. Mirrors the
/// dedicated Sync peer's operation lock: an exclusively created file that a
/// stale-holder check may clear.
struct OperationLock {
    path: PathBuf,
}
impl OperationLock {
    fn acquire(home: &Path) -> Result<Self, String> {
        let path = home.join("listener.lock");
        for _attempt in 0..2 {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    writeln!(file, "{}", std::process::id())
                        .map_err(|e| format!("write Birds listener lock: {e}"))?;
                    return Ok(Self { path });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let pid = fs::read_to_string(&path)
                        .ok()
                        .and_then(|v| v.trim().parse::<u32>().ok());
                    if pid.is_some_and(process_alive) {
                        return Err("another 'kobo birds listen' is starting".into());
                    }
                    fs::remove_file(&path)
                        .map_err(|e| format!("remove stale Birds listener lock: {e}"))?;
                }
                Err(e) => return Err(format!("create Birds listener lock: {e}")),
            }
        }
        Err("another 'kobo birds listen' is starting".into())
    }
}
impl Drop for OperationLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
fn read_pid() -> Result<Option<u32>, String> {
    match fs::read_to_string(pid_path()?) {
        Ok(s) => Ok(s.trim().parse().ok()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("read Birds PID: {e}")),
    }
}
fn process_alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .is_ok_and(|s| s.success())
}
fn running() -> Result<bool, String> {
    Ok(read_pid()?.is_some_and(process_alive))
}

fn worker(args: &[String]) -> Result<(), String> {
    let o = parse_listen(args)?;
    let mut token = String::new();
    loop {
        // One bad poll (a transient timeout, a 503 while Fugleramme
        // re-renders) must never kill the long-lived worker: log it and
        // retry on the next interval.
        if let Err(e) = poll_once(&o, &mut token) {
            eprintln!("Birds update failed; retrying in {}s: {e}", o.interval);
        }
        std::thread::sleep(Duration::from_secs(o.interval));
    }
}

fn poll_once(o: &Listen, token: &mut String) -> Result<(), String> {
    match http_get(&format!("{}/state", o.source), MAX_JSON).and_then(|b| state_token(&b)) {
        Ok(next) if next != *token => {
            let image = http_get(&format!("{}/collage.png", o.source), MAX_IMAGE)?;
            if !image.starts_with(b"\x89PNG\r\n\x1a\n") {
                return Err("Fugleramme collage is not PNG".into());
            }
            let generated = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|_| "clock is before 1970")?
                .as_secs();
            let snapshot = json!({"format":"cobalt-birds-v1","generated_at":generated,"source":o.source,"token":next,"recent":[]});
            push_bytes(snapshot.to_string().as_bytes(), &image, o.target.clone())?;
            *token = next;
        }
        Ok(_) => {}
        Err(e) => eprintln!("Birds source unavailable; keeping the last snapshot: {e}"),
    }
    Ok(())
}
fn state_token(bytes: &[u8]) -> Result<String, String> {
    let v: Value =
        serde_json::from_slice(bytes).map_err(|_| "Fugleramme /state returned invalid JSON")?;
    v.get("token")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| "Fugleramme /state omitted token".into())
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Target {
    Device(String),
    Sim,
}
fn push_command(args: &[String]) -> Result<(), String> {
    if args.len() < 3 {
        return Err(USAGE.into());
    }
    let json = fs::read(&args[0]).map_err(|e| format!("read {}: {e}", args[0]))?;
    let image = fs::read(&args[1]).map_err(|e| format!("read {}: {e}", args[1]))?;
    let target = match &args[2..] {
        [f] if f == "--sim" => Target::Sim,
        [f, h] if super::is_device_flag(f) => {
            if !super::valid_device_host(h) {
                return Err("device host contains unsupported characters".into());
            }
            Target::Device(h.clone())
        }
        _ => return Err(USAGE.into()),
    };
    push_bytes(&json, &image, target)
}
fn validate(json: &[u8], image: &[u8]) -> Result<(), String> {
    if json.len() > MAX_JSON {
        return Err("Birds snapshot exceeds 64 KiB".into());
    }
    if image.len() > MAX_IMAGE || !image.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err("Birds image must be a PNG no larger than 4 MiB".into());
    }
    let v: Value = serde_json::from_slice(json).map_err(|_| "Birds snapshot is invalid JSON")?;
    if v.get("format").and_then(Value::as_str) != Some("cobalt-birds-v1")
        || v.get("generated_at").and_then(Value::as_u64).is_none()
    {
        return Err("Birds snapshot has an unsupported format".into());
    }
    Ok(())
}
fn push_bytes(json: &[u8], image: &[u8], target: Target) -> Result<(), String> {
    let paired = pair_checksum(json, image)?;
    validate(&paired, image)?;
    match target {
        Target::Sim => {
            let root = kobo_sim::simulated_data_root("birds");
            fs::create_dir_all(&root).map_err(|e| e.to_string())?;
            // The image lands first and the snapshot last: the snapshot is the
            // commit point, and the app only trusts a snapshot whose recorded
            // checksum matches the image on the shelf.
            atomic_write(&root.join(IMAGE), image)?;
            atomic_write(&root.join(SNAPSHOT), &paired)?;
        }
        Target::Device(host) => {
            let script = format!(
                "set -eu\nroot='{ROOT}'\nmkdir -p \"$root\"\nchmod 700 \"$root\"\nbase64 -d > \"$root/.{IMAGE}.writing\" <<'BIRDS_IMAGE'\n{}\nBIRDS_IMAGE\nbase64 -d > \"$root/.{SNAPSHOT}.writing\" <<'BIRDS_JSON'\n{}\nBIRDS_JSON\nchmod 600 \"$root/.{IMAGE}.writing\" \"$root/.{SNAPSHOT}.writing\"\nmv -f \"$root/.{IMAGE}.writing\" \"$root/{IMAGE}\"\nmv -f \"$root/.{SNAPSHOT}.writing\" \"$root/{SNAPSHOT}\"\nsync\n",
                base64(image),
                base64(&paired)
            );
            let out =
                super::run_remote_shell(&format!("root@{host}"), &script, Duration::from_secs(60))?;
            if !out.status.success() {
                return Err(format!(
                    "Birds transfer failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                ));
            }
        }
    }
    println!("Birds snapshot published atomically.");
    Ok(())
}
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    // Stage beside the destination under its own name: with_extension would
    // map both current.png and current.json to the same current.writing, and
    // an occupied check without exclusive creation races a concurrent push.
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("{} has no file name", path.display()))?;
    let tmp = path.with_file_name(format!(".{name}.writing"));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                format!("{} is occupied by another operation", tmp.display())
            } else {
                format!("write {}: {e}", tmp.display())
            }
        })?;
    file.write_all(bytes)
        .map_err(|e| format!("write {}: {e}", tmp.display()))?;
    drop(file);
    fs::rename(&tmp, path).map_err(|e| format!("publish {}: {e}", path.display()))
}

/// Records the image's checksum inside the snapshot so the reader can prove
/// the two files it holds came from the same publish, never a mixed pair.
fn pair_checksum(json: &[u8], image: &[u8]) -> Result<Vec<u8>, String> {
    let mut v: Value =
        serde_json::from_slice(json).map_err(|_| "Birds snapshot is invalid JSON")?;
    v["image_checksum"] = json!(fnv64(image));
    serde_json::to_string(&v)
        .map(String::into_bytes)
        .map_err(|e| format!("encode Birds snapshot: {e}"))
}

/// FNV-1a 64, hex: cheap on the reader and sufficient to detect a generation
/// mismatch between current.png and current.json.
fn fnv64(bytes: &[u8]) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}
fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut o = String::new();
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        for s in [18, 12, 6, 0] {
            o.push(T[((n >> s) & 63) as usize] as char);
        }
        if c.len() < 3 {
            o.pop();
            o.push('=');
        }
        if c.len() < 2 {
            o.pop();
            o.push('=');
        }
    }
    o
}
fn parse_http(url: &str) -> Result<(String, u16, String), String> {
    let r = url
        .strip_prefix("http://")
        .ok_or("--source must be an http:// Fugleramme URL on the local network")?;
    let (authority, path) = r.split_once('/').map_or((r, "/"), |(a, p)| (a, p));
    let (host, port) = authority
        .rsplit_once(':')
        .map_or((authority, 8080), |(h, p)| (h, p.parse().unwrap_or(0)));
    if host.is_empty()
        || port == 0
        || host
            .chars()
            .any(|c| !(c.is_ascii_alphanumeric() || ".-".contains(c)))
    {
        return Err("--source has an unsupported host or port".into());
    }
    Ok((host.to_owned(), port, format!("/{path}")))
}
fn http_get(url: &str, max: usize) -> Result<Vec<u8>, String> {
    let (host, port, path) = parse_http(url)?;
    let mut s = TcpStream::connect((host.as_str(), port))
        .map_err(|e| format!("connect to Fugleramme: {e}"))?;
    s.set_read_timeout(Some(Duration::from_secs(15)))
        .map_err(|e| e.to_string())?;
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n\r\n"
    )
    .map_err(|e| e.to_string())?;
    let mut all = Vec::new();
    s.take((max + 16384) as u64)
        .read_to_end(&mut all)
        .map_err(|e| e.to_string())?;
    let split = all
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("Fugleramme returned malformed HTTP")?;
    if !all.starts_with(b"HTTP/1.1 200") && !all.starts_with(b"HTTP/1.0 200") {
        return Err("Fugleramme did not return 200".into());
    }
    let body = all.split_off(split + 4);
    if body.len() > max {
        return Err("Fugleramme response exceeds the Birds limit".into());
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn options_are_bounded() {
        assert_eq!(
            parse_listen(&[
                "--source".into(),
                "http://127.0.0.1:8080".into(),
                "--device".into(),
                "kobo.local".into()
            ])
            .unwrap()
            .interval,
            5
        );
        assert!(parse_listen(&["--source".into(), "https://x".into(), "--sim".into()]).is_err());
    }
    #[test]
    fn snapshot_validation() {
        let j = br#"{"format":"cobalt-birds-v1","generated_at":1,"recent":[]}"#;
        assert!(validate(j, b"\x89PNG\r\n\x1a\nrest").is_ok());
        assert!(validate(b"{}", b"\x89PNG\r\n\x1a\nrest").is_err());
    }
    #[test]
    fn state_requires_token() {
        assert_eq!(state_token(br#"{"token":"abc"}"#).unwrap(), "abc");
        assert!(state_token(b"{}").is_err());
    }
    #[test]
    fn pairing_records_the_image_checksum() {
        let json = br#"{"format":"cobalt-birds-v1","generated_at":1,"recent":[]}"#;
        let image = b"\x89PNG\r\n\x1a\nrest";
        let paired = pair_checksum(json, image).unwrap();
        let v: Value = serde_json::from_slice(&paired).unwrap();
        assert_eq!(
            v["image_checksum"].as_str().unwrap(),
            fnv64(image),
            "the snapshot must name the exact image bytes it ships with"
        );
        // A caller-supplied checksum never survives: the pair is recomputed.
        let lying =
            br#"{"format":"cobalt-birds-v1","generated_at":1,"image_checksum":"0000000000000000"}"#;
        let paired = pair_checksum(lying, image).unwrap();
        let v: Value = serde_json::from_slice(&paired).unwrap();
        assert_eq!(v["image_checksum"].as_str().unwrap(), fnv64(image));
    }
    #[test]
    fn staging_names_are_destination_specific_and_exclusive() {
        let root = std::env::temp_dir().join(format!("birds-test-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let png = root.join("current.png");
        let json = root.join("current.json");
        atomic_write(&png, b"png").unwrap();
        atomic_write(&json, b"json").unwrap();
        // Distinct staging files: neither destination reused the other's name.
        assert!(!root.join("current.writing").exists());
        // An occupied staging path fails without touching the destination.
        fs::write(root.join(".current.json.writing"), b"other").unwrap();
        assert!(atomic_write(&json, b"new").is_err());
        assert_eq!(fs::read(&json).unwrap(), b"json");
        let _ = fs::remove_dir_all(&root);
    }
}

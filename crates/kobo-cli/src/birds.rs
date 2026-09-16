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
        let s = Command::new("kill")
            .arg(pid.to_string())
            .status()
            .map_err(|e| format!("stop Birds listener: {e}"))?;
        if !s.success() {
            return Err("the Birds listener did not stop".into());
        }
    }
    let _ = fs::remove_file(pid_path()?);
    println!("Birds listener stopped.");
    Ok(())
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
        match http_get(&format!("{}/state", o.source), MAX_JSON).and_then(|b| state_token(&b)) {
            Ok(next) if next != token => {
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
                token = next;
            }
            Ok(_) => {}
            Err(e) => eprintln!("Birds source unavailable; keeping the last snapshot: {e}"),
        }
        std::thread::sleep(Duration::from_secs(o.interval));
    }
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
    validate(json, image)?;
    match target {
        Target::Sim => {
            let root = kobo_sim::simulated_data_root("birds");
            fs::create_dir_all(&root).map_err(|e| e.to_string())?;
            atomic_write(&root.join(IMAGE), image)?;
            atomic_write(&root.join(SNAPSHOT), json)?;
        }
        Target::Device(host) => {
            let script=format!("set -eu\nroot='{ROOT}'\nmkdir -p \"$root\"\nchmod 700 \"$root\"\nbase64 -d > \"$root/.{IMAGE}.writing\" <<'BIRDS_IMAGE'\n{}\nBIRDS_IMAGE\nbase64 -d > \"$root/.{SNAPSHOT}.writing\" <<'BIRDS_JSON'\n{}\nBIRDS_JSON\nchmod 600 \"$root/.{IMAGE}.writing\" \"$root/.{SNAPSHOT}.writing\"\nmv -f \"$root/.{IMAGE}.writing\" \"$root/{IMAGE}\"\nmv -f \"$root/.{SNAPSHOT}.writing\" \"$root/{SNAPSHOT}\"\nsync\n",base64(image),base64(json));
            let out = super::run_remote_shell(&host, &script, Duration::from_secs(60))?;
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
    let tmp = path.with_extension("writing");
    if tmp.exists() {
        return Err(format!(
            "{} is occupied by another operation",
            tmp.display()
        ));
    }
    fs::write(&tmp, bytes).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|e| format!("publish {}: {e}", path.display()))
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
}

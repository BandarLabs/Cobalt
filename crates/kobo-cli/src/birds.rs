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
const MAX_JSON: usize = 64 * 1024;
const MAX_IMAGE: usize = 4 * 1024 * 1024;
const USAGE: &str = concat!(
    "usage: kobo birds listen --source http://HOST:PORT ",
    crate::targets::target_choice!(),
    " [--interval SECONDS]\n\
                     \x20      kobo birds status\n\
                     \x20      kobo birds stop\n\
                     \x20      kobo birds push SNAPSHOT.json IMAGE.png ",
    crate::targets::target_choice!(),
    "\n\
                     \x20      kobo birds _worker --source URL --device IP --interval SECONDS\n\
                     macOS and Linux only. --source is Fugleramme's local web endpoint; BirdNET-Go owns the microphone and model.\n",
    crate::targets::saved_reader_note!(),
);

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
fn health_path() -> Result<PathBuf, String> {
    Ok(home()?.join("listener.health"))
}

/// How the last look at the station went, as the worker writes it after
/// every poll and `status` reads it back.
///
/// A process that is alive says nothing about whether birds are reaching
/// the reader: a listener pointed at a station that is not running polled
/// and failed every few seconds while `status` reported it running. This is
/// what lets `status` say which half of the chain is broken.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Health {
    /// The last poll worked. The time is when a collage last reached the
    /// reader, if one has since the listener started.
    Working { delivered: Option<u64> },
    /// Every poll since `since` has failed, the latest for this reason.
    Failing { since: u64, cause: String },
}

impl Health {
    fn encode(&self) -> String {
        match self {
            Self::Working { delivered } => format!("ok\t{}\n", delivered.unwrap_or(0)),
            Self::Failing { since, cause } => {
                format!("fail\t{since}\t{}\n", cause.replace(['\n', '\t'], " "))
            }
        }
    }

    fn decode(text: &str) -> Option<Self> {
        let mut fields = text.trim_end().splitn(3, '\t');
        match fields.next()? {
            "ok" => {
                let delivered = fields.next()?.parse().ok()?;
                Some(Self::Working {
                    delivered: (delivered > 0).then_some(delivered),
                })
            }
            "fail" => Some(Self::Failing {
                since: fields.next()?.parse().ok()?,
                cause: fields.next()?.to_owned(),
            }),
            _ => None,
        }
    }

    /// The health after one more poll.
    fn after(self, poll: Result<bool, String>, now: u64) -> Self {
        match (poll, self) {
            (Ok(true), _) => Self::Working {
                delivered: Some(now),
            },
            (Ok(false), Self::Working { delivered }) => Self::Working { delivered },
            (Ok(false), Self::Failing { .. }) => Self::Working { delivered: None },
            (Err(cause), Self::Failing { since, .. }) => Self::Failing { since, cause },
            (Err(cause), Self::Working { .. }) => Self::Failing { since: now, cause },
        }
    }
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn ago(seconds: u64) -> String {
    match seconds {
        0..=59 => "less than a minute ago".to_owned(),
        60..=3599 => format!("{} min ago", seconds / 60),
        _ => format!("{} hr ago", seconds / 3600),
    }
}

/// What `status` says about a running listener, from its last poll.
fn describe(pid: u32, health: Option<&Health>, now: u64) -> Result<String, String> {
    match health {
        None => Ok(format!(
            "Birds listener is running (PID {pid}). It has not finished its first look at the station yet."
        )),
        Some(Health::Working {
            delivered: Some(at),
        }) => Ok(format!(
            "Birds listener is running (PID {pid}). The last collage reached the reader {}.",
            ago(now.saturating_sub(*at))
        )),
        Some(Health::Working { delivered: None }) => Ok(format!(
            "Birds listener is running (PID {pid}). The station is answering, and the reader already has its latest collage."
        )),
        Some(Health::Failing { since, cause }) => Err(format!(
            "Birds listener is running (PID {pid}), but nothing has reached the reader since {}: {cause}",
            ago(now.saturating_sub(*since))
        )),
    }
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
    // One look in the foreground first. A listener started against a station
    // that is not running, or a reader that cannot be reached, used to report
    // itself started and then fail in a log nobody reads.
    let mut token = String::new();
    poll_once(&options, &mut token)
        .map_err(|cause| format!("Birds did not start: {cause} Nothing was left running."))?;
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
    let _ = fs::remove_file(health_path()?);
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
    println!(
        "Birds listener started (PID {}). The first collage reached the reader. Check on it with kobo birds status.",
        child.id()
    );
    Ok(())
}
fn status() -> Result<(), String> {
    match read_pid()? {
        Some(pid) if process_alive(pid) => {
            let health = fs::read_to_string(health_path()?)
                .ok()
                .and_then(|text| Health::decode(&text));
            let log = log_path()?;
            match describe(pid, health.as_ref(), now_seconds()) {
                Ok(line) => {
                    println!("{line}\nLog: {}", log.display());
                    Ok(())
                }
                Err(line) => Err(format!("{line}\nLog: {}", log.display())),
            }
        }
        _ => Err("Birds listener is not running. Start it with kobo birds listen.".into()),
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
    let mut health = Health::Working { delivered: None };
    loop {
        // One bad poll (a transient timeout, a 503 while Fugleramme
        // re-renders) must never kill the long-lived worker: record it for
        // `status`, log it, and retry on the next interval.
        let poll = poll_once(&o, &mut token);
        if let Err(e) = &poll {
            eprintln!("Birds update failed; retrying in {}s: {e}", o.interval);
        }
        health = health.after(poll, now_seconds());
        if let Ok(path) = health_path() {
            let _ = fs::write(path, health.encode());
        }
        std::thread::sleep(Duration::from_secs(o.interval));
    }
}

/// One look at the station. `Ok(true)` when a new collage went to the
/// reader, `Ok(false)` when there was nothing new to send, and an error
/// naming which half of the chain failed and what to do about it.
fn poll_once(o: &Listen, token: &mut String) -> Result<bool, String> {
    let station = |error: String| {
        format!(
            "Fugleramme did not answer at {} ({error}). Start Fugleramme and BirdNET-Go on this computer, as the Birds README describes.",
            o.source
        )
    };
    let next = http_get(&format!("{}/state", o.source), MAX_JSON)
        .and_then(|b| state_token(&b))
        .map_err(station)?;
    if next == *token {
        return Ok(false);
    }
    let image = http_get(&format!("{}/collage.png", o.source), MAX_IMAGE).map_err(station)?;
    // The token and the collage arrive in separate requests; if
    // Fugleramme re-rendered meanwhile, this pair mixes generations.
    // Re-read the state and skip the cycle rather than consume it.
    let still = http_get(&format!("{}/state", o.source), MAX_JSON)
        .and_then(|b| state_token(&b))
        .map_err(station)?;
    if still != next {
        return Ok(false);
    }
    let generated = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "clock is before 1970")?
        .as_secs();
    let snapshot = json!({"format":"cobalt-birds-v1","generated_at":generated,"source":o.source,"token":next,"recent":[]});
    push_bytes(snapshot.to_string().as_bytes(), &image, o.target.clone()).map_err(|error| {
        format!("The collage could not be sent to the reader ({error}). Wake the reader and check its Wi-Fi.")
    })?;
    *token = next;
    Ok(true)
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
    if image.len() > MAX_IMAGE || !png_complete(image) {
        return Err("Birds image must be a complete PNG no larger than 4 MiB".into());
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
    let (image_name, paired) = pair_checksum(json, image)?;
    validate(&paired, image)?;
    match target {
        Target::Sim => {
            let root = kobo_sim::simulated_data_root("birds");
            fs::create_dir_all(&root).map_err(|e| e.to_string())?;
            // The image is content-addressed and lands first under its own
            // name, so the previous generation's file is never overwritten;
            // the snapshot commits last as the pointer. If the pointer commit
            // fails, the old snapshot still names the old, intact image, and
            // an orphaned new image is pruned by the next successful publish.
            let image_path = root.join(&image_name);
            if !image_path.exists() {
                atomic_write(&image_path, image)?;
            }
            atomic_write(&root.join(SNAPSHOT), &paired)?;
            prune_images(&root, &image_name);
        }
        Target::Device(host) => {
            let script = format!(
                "set -eu\nroot='{ROOT}'\nmkdir -p \"$root\"\nchmod 700 \"$root\"\nbase64 -d > \"$root/.{image_name}.writing\" <<'BIRDS_IMAGE'\n{}\nBIRDS_IMAGE\nbase64 -d > \"$root/.{SNAPSHOT}.writing\" <<'BIRDS_JSON'\n{}\nBIRDS_JSON\nchmod 600 \"$root/.{image_name}.writing\" \"$root/.{SNAPSHOT}.writing\"\nmv -f \"$root/.{image_name}.writing\" \"$root/{image_name}\"\nmv -f \"$root/.{SNAPSHOT}.writing\" \"$root/{SNAPSHOT}\"\nfor old in \"$root\"/img-*.png; do [ \"$old\" = \"$root/{image_name}\" ] || rm -f \"$old\"; done\nsync\n",
                super::base64_encode(image),
                super::base64_encode(&paired)
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

/// Drops unreferenced content-addressed images after a successful commit;
/// the referenced one stays.
fn prune_images(root: &Path, keep: &str) {
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if name.starts_with("img-")
                && name.to_ascii_lowercase().ends_with(".png")
                && name != keep
            {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
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

/// Points the snapshot at a content-addressed image name and records the
/// image's checksum, so the reader can prove the two files it holds came from
/// the same publish, never a mixed pair. A caller-supplied pointer never
/// survives: both fields are recomputed from the bytes being shipped.
fn pair_checksum(json: &[u8], image: &[u8]) -> Result<(String, Vec<u8>), String> {
    let mut v: Value =
        serde_json::from_slice(json).map_err(|_| "Birds snapshot is invalid JSON")?;
    let checksum = fnv64(image);
    let image_name = format!("img-{checksum}.png");
    v["image"] = json!(image_name);
    v["image_checksum"] = json!(checksum);
    let paired = serde_json::to_string(&v)
        .map(String::into_bytes)
        .map_err(|e| format!("encode Birds snapshot: {e}"))?;
    Ok((image_name, paired))
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
    // A connection closed midway must not pass as a complete response: when
    // the server declared a length, receive exactly that many bytes.
    let headers = String::from_utf8_lossy(&all);
    if let Some(line) = headers
        .lines()
        .find(|l| l.to_ascii_lowercase().starts_with("content-length:"))
    {
        let declared: usize = line[15..]
            .trim()
            .parse()
            .map_err(|_| "Fugleramme sent a malformed Content-Length".to_owned())?;
        if body.len() != declared {
            return Err(format!(
                "Fugleramme response truncated: declared {declared} bytes, received {}",
                body.len()
            ));
        }
    }
    if body.len() > max {
        return Err("Fugleramme response exceeds the Birds limit".into());
    }
    Ok(body)
}

/// The whole PNG must parse: 8-byte signature, IHDR first, well-formed
/// chunks, and an IEND exactly at the end. A file that stops early keeps its
/// prefix, so the signature alone proves nothing about completeness.
fn png_complete(bytes: &[u8]) -> bool {
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return false;
    }
    let mut offset = 8;
    let mut first = true;
    loop {
        if offset + 12 > bytes.len() {
            return false;
        }
        let length = u32::from_be_bytes(bytes[offset..offset + 4].try_into().expect("4")) as usize;
        let kind = &bytes[offset + 4..offset + 8];
        let Some(end) = offset.checked_add(12).and_then(|v| v.checked_add(length)) else {
            return false;
        };
        if end > bytes.len() {
            return false;
        }
        if first && kind != b"IHDR" {
            return false;
        }
        first = false;
        if kind == b"IEND" {
            return length == 0 && end == bytes.len();
        }
        offset = end;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The smallest structurally complete PNG: signature, IHDR, IEND.
    fn minimal_png() -> Vec<u8> {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&13_u32.to_be_bytes());
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&[0; 13]);
        png.extend_from_slice(&[0; 4]);
        png.extend_from_slice(&0_u32.to_be_bytes());
        png.extend_from_slice(b"IEND");
        png.extend_from_slice(&[0; 4]);
        png
    }

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
        assert!(validate(j, &minimal_png()).is_ok());
        assert!(validate(b"{}", &minimal_png()).is_err());
        assert!(validate(j, b"\x89PNG\r\n\x1a\nrest").is_err());
    }
    #[test]
    fn a_listener_whose_every_poll_fails_says_so_and_why() {
        let health = Health::Working { delivered: None }
            .after(Err("Fugleramme did not answer.".into()), 100)
            .after(Err("Fugleramme still did not answer.".into()), 160);
        assert_eq!(
            health,
            Health::Failing {
                since: 100,
                cause: "Fugleramme still did not answer.".into()
            }
        );
        assert_eq!(Health::decode(&health.encode()), Some(health.clone()));
        let said = describe(42, Some(&health), 400).expect_err("a failing listener is an error");
        assert!(
            said.contains("running (PID 42), but nothing has reached the reader since 5 min ago")
        );
        assert!(said.contains("Fugleramme still did not answer."));
    }

    #[test]
    fn a_listener_that_delivered_says_when() {
        let health = Health::Failing {
            since: 1,
            cause: "x".into(),
        }
        .after(Ok(true), 1_000);
        assert_eq!(
            health,
            Health::Working {
                delivered: Some(1_000)
            }
        );
        assert_eq!(Health::decode(&health.encode()), Some(health.clone()));
        let said = describe(7, Some(&health), 1_030).expect("working");
        assert!(said.contains("reached the reader less than a minute ago"));
        assert!(describe(7, None, 0).is_ok());
    }

    #[test]
    fn listen_against_a_station_that_is_not_running_names_the_station_and_the_fix() {
        let options = Listen {
            source: "http://127.0.0.1:9".into(),
            target: Target::Sim,
            interval: 5,
        };
        let error = poll_once(&options, &mut String::new()).expect_err("nothing listens there");
        assert!(error.contains("Fugleramme did not answer at http://127.0.0.1:9"));
        assert!(error.contains("Start Fugleramme and BirdNET-Go"));
    }

    #[test]
    fn state_requires_token() {
        assert_eq!(state_token(br#"{"token":"abc"}"#).unwrap(), "abc");
        assert!(state_token(b"{}").is_err());
    }
    #[test]
    fn the_transfer_encoding_never_lengthens_a_picture_by_a_trailing_zero() {
        // A hand-rolled encoder here padded a one-byte tail as though it were
        // two, so the reader's `base64 -d` wrote one extra zero byte onto the
        // end of the file. The picture still opened, but its checksum no
        // longer matched the pointer the snapshot shipped with, and the app
        // correctly refused every such update and kept the previous view
        // forever. One picture in three has a length that hits it.
        assert_eq!(crate::base64_encode(b"\x01\x02\x03"), "AQID");
        assert_eq!(crate::base64_encode(b"\x01\x02\x03\x04"), "AQIDBA==");
        assert_eq!(crate::base64_encode(b"\x01\x02\x03\x04\x05"), "AQIDBAU=");
        // Every length, because only those one past a group boundary were wrong.
        for length in 1..200_usize {
            let bytes = vec![0x5a_u8; length];
            let encoded = crate::base64_encode(&bytes);
            let characters = encoded.chars().filter(|c| !c.is_whitespace()).count();
            let padding = encoded.chars().filter(|c| *c == '=').count();
            assert_eq!(
                (characters - padding) * 3 / 4,
                length,
                "{length} bytes must decode back to {length} bytes, not more"
            );
        }
    }
    #[test]
    fn pairing_records_the_content_addressed_image() {
        let json = br#"{"format":"cobalt-birds-v1","generated_at":1,"recent":[]}"#;
        let image = minimal_png();
        let (name, paired) = pair_checksum(json, &image).unwrap();
        assert_eq!(name, format!("img-{}.png", fnv64(&image)));
        let v: Value = serde_json::from_slice(&paired).unwrap();
        assert_eq!(v["image"].as_str().unwrap(), name);
        assert_eq!(
            v["image_checksum"].as_str().unwrap(),
            fnv64(&image),
            "the snapshot must name the exact image bytes it ships with"
        );
        // A caller-supplied pointer never survives: the pair is recomputed.
        let lying =
            br#"{"format":"cobalt-birds-v1","generated_at":1,"image":"old.png","image_checksum":"0000000000000000"}"#;
        let (name, paired) = pair_checksum(lying, &image).unwrap();
        let v: Value = serde_json::from_slice(&paired).unwrap();
        assert_eq!(v["image"].as_str().unwrap(), name);
        assert_eq!(v["image_checksum"].as_str().unwrap(), fnv64(&image));
    }
    #[test]
    fn png_must_be_complete() {
        let good = minimal_png();
        assert!(png_complete(&good));
        // Truncated mid-chunk, trailing garbage after IEND, missing IHDR.
        assert!(!png_complete(&good[..good.len() - 3]));
        let mut trailing = good.clone();
        trailing.push(0);
        assert!(!png_complete(&trailing));
        assert!(!png_complete(b"\x89PNG\r\n\x1a\nIEND-only-prefix"));
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

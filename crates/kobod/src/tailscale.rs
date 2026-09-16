//! The small, bounded part of Tailscale Cobalt owns.
//!
//! EXPERIMENTAL (P0/P1): probe support and a runtime-owned supervisor. No
//! hardware run has validated this module yet; the attended evidence bar in
//! docs/DEVICES.md applies before any of it can be called tested.
//!
//! Tailscale itself remains unmodified. This module owns the daemon's home,
//! process lifetime, the `/etc/hosts` tailnet block, and the fixed flag
//! policy. It never accepts a flag, route, or exit-node choice from an
//! application.
//!
//! Design constraints, from on-device community evidence:
//!
//! * `/mnt/onboard` is vfat: it cannot hold the 0600 the state file needs and
//!   cannot host a unix socket at all, so state and the control socket live on
//!   the root filesystem under [`HOME`]. Only the (large) binaries live under
//!   [`BIN_DIR`].
//! * `/dev` is devtmpfs and is rebuilt at every boot, so the tun node is
//!   created on every start, never at install time.
//! * The Kobo ships no `iptables` binary at all, so `--netfilter-mode=off` is
//!   not optional. It is safe here because the reader is a leaf node, never an
//!   exit node or subnet router. The flag belongs to `tailscale up`; the
//!   daemon no longer accepts it.
//! * The Kobo has no DNS manager, so `--accept-dns=false` keeps
//!   `/etc/resolv.conf` stock; name resolution is the marker-delimited
//!   `/etc/hosts` block this module maintains instead of `MagicDNS`.
//! * Tailscaled idles at tens of megabytes of RSS unclamped; it is started
//!   with `GOMEMLIMIT=48MiB` and `GOGC=50`, the documented e-reader clamps.

use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Read};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Where the Tailscale app keeps what the owner chose.
const APP_STATE: &str = "/mnt/onboard/.adds/cobalt/state/tailscale";
/// The daemon's home on the root filesystem: state file and control socket.
const HOME: &str = "/var/lib/cobalt/tailscale";
/// The large static binaries, on the user partition beside the Sync engine.
const BIN_DIR: &str = "/mnt/onboard/.adds/cobalt/bin/tailscale";

/// The pinned upstream build. A versioned address, not a moving target: the
/// digest below is what makes the download safe to trust, and a digest cannot
/// be pinned to `stable/latest`.
const TARBALL_URL: &str = "https://pkgs.tailscale.com/stable/tailscale_1.102.2_arm.tgz";
const TARBALL_SHA256: &str = "4d514c8659f21aa21b11c10200e25c38e1f5400d3b0c5bc17560b2f7d9c5c676";
/// The reviewed artifact is 34.7 MB; allow headroom, refuse absurdity.
const TARBALL_LIMIT: u32 = 64 * 1024 * 1024;

const HOSTS_FILE: &str = "/etc/hosts";
const HOSTS_BEGIN: &str = "# >>> cobalt-tailscale >>>";
const HOSTS_END: &str = "# <<< cobalt-tailscale <<<";

/// How long `login` waits for the owner to approve the device in a browser.
const LOGIN_WAIT: Duration = Duration::from_secs(10 * 60);
const POLL: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, Eq, PartialEq)]
struct Settings {
    enabled: bool,
}

impl Settings {
    fn load(root: &Path) -> Result<Self, String> {
        let text = fs::read_to_string(root.join("tailscale-config"))
            .map_err(|error| format!("read Tailscale configuration: {error}"))?;
        Self::parse(&text)
    }

    fn parse(text: &str) -> Result<Self, String> {
        match text.lines().next() {
            Some("true") => Ok(Self { enabled: true }),
            Some("false") => Ok(Self { enabled: false }),
            _ => Err("Tailscale configuration has an invalid enabled value".to_owned()),
        }
    }
}

fn tailscaled() -> PathBuf {
    Path::new(BIN_DIR).join("tailscaled")
}

fn tailscale_cli() -> PathBuf {
    Path::new(BIN_DIR).join("tailscale")
}

fn socket() -> PathBuf {
    Path::new(HOME).join("tailscaled.sock")
}

/// A `tailscale` CLI invocation against this daemon's socket.
fn cli() -> Command {
    let mut command = Command::new(tailscale_cli());
    command.arg("--socket").arg(socket());
    command
}

/// `kobod --tailscale status|ensure|start|stop|login|hosts-sync`.
pub fn command(arguments: &[String]) -> Result<(), String> {
    let state = Path::new(APP_STATE);
    match arguments {
        [action] if action == "status" => {
            println!("{}", read_status(state));
            Ok(())
        }
        [action] if action == "ensure" => {
            // Idempotent start-if-enabled, for the session/wake launcher.
            let settings = Settings::load(state)?;
            if !settings.enabled {
                // Off means off: a running daemon is stopped, not left behind.
                if backend_state() == Ok(BackendState::Running) {
                    return stop(state);
                }
                write_status(state, "disabled\n\nTailscale is off.\n")?;
                return Ok(());
            }
            if backend_state() == Ok(BackendState::Running) {
                sync_hosts()?;
                return Ok(());
            }
            start(state)
        }
        [action] if action == "start" => start(state),
        [action] if action == "stop" => stop(state),
        [action] if action == "login" => login(state),
        [action] if action == "hosts-sync" => sync_hosts(),
        _ => Err("usage: kobod --tailscale status|ensure|start|stop|login|hosts-sync".to_owned()),
    }
}

fn start(state: &Path) -> Result<(), String> {
    ensure_engine()?;
    verify_engine()?;
    prepare_home()?;
    ensure_tun_node()?;
    ensure_loopback()?;
    if backend_state() != Ok(BackendState::Running) {
        spawn_daemon()?;
        wait_for_daemon()?;
    }
    up(state)
}

/// Brings the node up with the fixed leaf-node flag policy.
fn up(state: &Path) -> Result<(), String> {
    let output = cli()
        .arg("up")
        .arg("--accept-dns=false")
        .arg("--netfilter-mode=off")
        .arg("--hostname=kobo-cobalt")
        // Never let a wake-driven ensure hang forever when control-plane
        // access is unavailable.
        .arg("--timeout=30s")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("bring Tailscale up: {error}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if output.status.success() {
        sync_hosts()?;
        return report_running(state);
    }
    let status_url = status_json().ok().and_then(|json| auth_url(&json));
    if let Some(url) = login_url(&stdout)
        .or_else(|| login_url(&stderr))
        .map(str::to_owned)
        .or(status_url)
    {
        write_status(
            state,
            &format!("needs-login\n\nApprove this Kobo from any browser.\n{url}"),
        )?;
        return Err(
            "Tailscale needs sign-in; the approval link is on the Tailscale screen.".into(),
        );
    }
    write_status(state, "failed\n\nTailscale could not come up.\n")?;
    Err(format!("tailscale up failed: {}", stderr.trim()))
}

/// Runs the interactive sign-in, surfacing the approval URL to the app the
/// moment the daemon prints it, then finishing the up sequence.
fn login(state: &Path) -> Result<(), String> {
    ensure_engine()?;
    verify_engine()?;
    prepare_home()?;
    ensure_tun_node()?;
    ensure_loopback()?;
    if backend_state() != Ok(BackendState::Running) {
        spawn_daemon()?;
        wait_for_daemon()?;
    }
    let mut child = cli()
        .arg("login")
        .arg("--timeout")
        .arg("10m")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("start Tailscale sign-in: {error}"))?;
    let (sender, receiver) = std::sync::mpsc::channel();
    if let Some(stdout) = child.stdout.take() {
        forward_lines(stdout, sender.clone());
    }
    if let Some(stderr) = child.stderr.take() {
        forward_lines(stderr, sender);
    }
    let deadline = Instant::now() + LOGIN_WAIT;
    let mut announced = false;
    loop {
        match receiver.recv_timeout(POLL) {
            Ok(line) => {
                if let Some(url) = login_url(&line) {
                    write_status(
                        state,
                        &format!("needs-login\n\nApprove this Kobo from any browser.\n{url}"),
                    )?;
                    announced = true;
                }
            }
            Err(
                std::sync::mpsc::RecvTimeoutError::Timeout
                | std::sync::mpsc::RecvTimeoutError::Disconnected,
            ) => {}
        }
        if child
            .try_wait()
            .map_err(|error| format!("wait for sign-in: {error}"))?
            .is_some()
        {
            while let Ok(line) = receiver.try_recv() {
                if let Some(url) = login_url(&line) {
                    write_status(
                        state,
                        &format!("needs-login\n\nApprove this Kobo from any browser.\n{url}"),
                    )?;
                    announced = true;
                }
            }
            break;
        }
        if Instant::now() >= deadline {
            let _ignored = child.kill();
            let _ignored = child.wait();
            write_status(state, "needs-login\n\nSign-in timed out. Try again.\n")?;
            return Err("Tailscale sign-in timed out".to_owned());
        }
    }
    if !announced {
        if let Ok(json) = status_json() {
            if let Some(url) = auth_url(&json) {
                write_status(
                    state,
                    &format!("needs-login\n\nApprove this Kobo from any browser.\n{url}"),
                )?;
                announced = true;
            }
        }
    }
    if !announced && backend_state() == Ok(BackendState::NeedsLogin) {
        write_status(state, "needs-login\n\nSign-in is required.\n")?;
        return Err("Tailscale sign-in produced no approval link".to_owned());
    }
    up(state)
}

fn forward_lines<R: Read + Send + 'static>(reader: R, sender: std::sync::mpsc::Sender<String>) {
    std::thread::spawn(move || {
        for line in BufReader::new(reader).lines().map_while(Result::ok) {
            if sender.send(line).is_err() {
                break;
            }
        }
    });
}

fn stop(state: &Path) -> Result<(), String> {
    if tailscale_cli().exists() {
        let _ignored = cli().arg("down").stdin(Stdio::null()).output();
    }
    // A supervised stop: TERM the daemon by pidfile-free lookup through
    // pidof, the same busybox tool the session scripts already rely on.
    let _ignored = Command::new("pidof")
        .arg("tailscaled")
        .stdout(Stdio::piped())
        .output()
        .map(|output| {
            for pid in String::from_utf8_lossy(&output.stdout).split_whitespace() {
                let _ignored = Command::new("kill").arg(pid).output();
            }
        });
    clear_hosts()?;
    write_status(state, "stopped\n\nTailscale is off.\n")?;
    Ok(())
}

fn prepare_home() -> Result<(), String> {
    fs::create_dir_all(HOME).map_err(|error| format!("create Tailscale home: {error}"))?;
    fs::set_permissions(HOME, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("protect Tailscale home: {error}"))
}

/// `/dev` is devtmpfs: the node is recreated on every boot, so this runs on
/// every start rather than once at install.
fn ensure_tun_node() -> Result<(), String> {
    let node = Path::new("/dev/net/tun");
    if node.exists() {
        return Ok(());
    }
    fs::create_dir_all("/dev/net").map_err(|error| format!("create /dev/net: {error}"))?;
    let status = Command::new("mknod")
        .arg(node)
        .arg("c")
        .arg("10")
        .arg("200")
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("create the tun device node: {error}"))?;
    if !status.status.success() {
        return Err(format!(
            "create the tun device node: {}",
            String::from_utf8_lossy(&status.stderr).trim()
        ));
    }
    Ok(())
}

/// Some Kobo firmware boots with loopback down; the status socket and any
/// userspace-mode listeners need it. A no-op when it is already up.
fn ensure_loopback() -> Result<(), String> {
    let state = fs::read_to_string("/sys/class/net/lo/operstate").unwrap_or_default();
    if state.trim() == "up" {
        return Ok(());
    }
    for program in ["ifconfig", "ip"] {
        let attempt = if program == "ifconfig" {
            Command::new(program).arg("lo").arg("up").output()
        } else {
            Command::new(program)
                .arg("link")
                .arg("set")
                .arg("lo")
                .arg("up")
                .output()
        };
        if attempt
            .map(|output| output.status.success())
            .unwrap_or(false)
        {
            return Ok(());
        }
    }
    Err("loopback is down and neither ifconfig nor ip could bring it up".to_owned())
}

fn spawn_daemon() -> Result<Child, String> {
    Command::new(tailscaled())
        .arg("--statedir")
        .arg(HOME)
        .arg("--socket")
        .arg(socket())
        .arg("--tun=tailscale0")
        .env("GOMEMLIMIT", "48MiB")
        .env("GOGC", "50")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("start the Tailscale daemon: {error}"))
}

fn wait_for_daemon() -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if socket().exists() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Err("the Tailscale daemon did not come up".to_owned())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BackendState {
    Running,
    NeedsLogin,
    Stopped,
    Other,
}

fn status_json() -> Result<String, String> {
    let output = cli()
        .arg("status")
        .arg("--json")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map_err(|error| format!("read Tailscale status: {error}"))?;
    if !output.status.success() {
        return Err("Tailscale status command failed".to_owned());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn backend_state() -> Result<BackendState, String> {
    let output = cli()
        .arg("status")
        .arg("--json")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map_err(|error| format!("read Tailscale status: {error}"))?;
    if !output.status.success() {
        return Ok(BackendState::Stopped);
    }
    let text = String::from_utf8_lossy(&output.stdout);
    parse_backend_state(&text)
}

fn parse_backend_state(json: &str) -> Result<BackendState, String> {
    let value = kobo_json::parse(json).map_err(|_| "Tailscale status was not JSON".to_owned())?;
    Ok(match value.get("BackendState").and_then(|v| v.as_str()) {
        Some("Running") => BackendState::Running,
        Some("NeedsLogin" | "NeedsMachineAuth") => BackendState::NeedsLogin,
        Some("Stopped") => BackendState::Stopped,
        _ => BackendState::Other,
    })
}

/// Reads the node's own first tailnet address and the peer count from status
/// JSON. Absent fields are an empty answer, never an error: a node that has
/// not logged in yet has no address and no peers.
fn parse_self_and_peers(json: &str) -> (String, usize) {
    let Ok(value) = kobo_json::parse(json) else {
        return (String::new(), 0);
    };
    let ip = value
        .get("Self")
        .and_then(|s| s.get("TailscaleIPs"))
        .and_then(|ips| ips.index(0))
        .and_then(|ip| ip.as_str())
        .unwrap_or_default()
        .to_owned();
    let peers = match value.get("Peer") {
        Some(kobo_json::Value::Object(entries)) => entries.len(),
        _ => 0,
    };
    (ip, peers)
}

fn report_running(state: &Path) -> Result<(), String> {
    let output = cli()
        .arg("status")
        .arg("--json")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map_err(|error| format!("read Tailscale status: {error}"))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let (ip, peers) = parse_self_and_peers(&text);
    write_status(
        state,
        &format!("running\n{ip}\nConnected. {peers} peer(s) on the tailnet.\n"),
    )
}

/// The sign-in URL a `tailscale up` or `tailscale login` prints when the node
/// needs approval.
fn auth_url(json: &str) -> Option<String> {
    kobo_json::parse(json)
        .ok()?
        .get("AuthURL")?
        .as_str()
        .filter(|url| url.starts_with("https://login.tailscale.com/"))
        .map(str::to_owned)
}

fn login_url(output: &str) -> Option<&str> {
    output
        .split_whitespace()
        .find(|word| word.starts_with("https://login.tailscale.com/"))
        .map(|word| word.trim_end_matches(['.', ',']))
}

/// Replaces the marker-delimited tailnet block in `/etc/hosts` from live
/// status. This stands in for `MagicDNS`, which `--accept-dns=false` declines:
/// apps dial short names, and the block disappears when the service stops.
fn sync_hosts() -> Result<(), String> {
    let output = cli()
        .arg("status")
        .arg("--json")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map_err(|error| format!("read Tailscale status: {error}"))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let block = hosts_block(&text);
    rewrite_hosts(&block)
}

/// Builds the replacement block: one line per peer and for this node, full
/// `MagicDNS` name and short name, peers with no address skipped. Deterministic
/// ordering keeps the file diff-stable.
fn hosts_block(json: &str) -> Vec<String> {
    let Ok(value) = kobo_json::parse(json) else {
        return Vec::new();
    };
    let mut lines = Vec::new();
    let mut push = |node: &kobo_json::Value| {
        let Some(ip) = node
            .get("TailscaleIPs")
            .and_then(|ips| ips.index(0))
            .and_then(|ip| ip.as_str())
        else {
            return;
        };
        let name = node
            .get("DNSName")
            .and_then(|n| n.as_str())
            .unwrap_or_default();
        let name = name.trim_end_matches('.');
        if name.is_empty() {
            return;
        }
        let short = name.split('.').next().unwrap_or_default();
        if short.is_empty() || short == name {
            lines.push(format!("{ip}\t{name}"));
        } else {
            lines.push(format!("{ip}\t{name} {short}"));
        }
    };
    if let Some(self_node) = value.get("Self") {
        push(self_node);
    }
    if let Some(kobo_json::Value::Object(peers)) = value.get("Peer") {
        for (_, peer) in peers {
            push(peer);
        }
    }
    lines.sort();
    lines.dedup();
    lines
}

/// Swaps the marked block in `/etc/hosts` atomically. Everything outside the
/// markers is the stock file's own business and is carried through verbatim.
fn rewrite_hosts(block: &[String]) -> Result<(), String> {
    let existing = fs::read_to_string(HOSTS_FILE).unwrap_or_default();
    let mut next = String::with_capacity(existing.len() + 256);
    let mut inside = false;
    for line in existing.lines() {
        if line == HOSTS_BEGIN {
            inside = true;
            continue;
        }
        if line == HOSTS_END {
            inside = false;
            continue;
        }
        if !inside {
            let _ignored = writeln!(next, "{line}");
        }
    }
    if !block.is_empty() {
        let _ignored = writeln!(next, "{HOSTS_BEGIN}");
        for line in block {
            let _ignored = writeln!(next, "{line}");
        }
        let _ignored = writeln!(next, "{HOSTS_END}");
    }
    atomic_write(Path::new(HOSTS_FILE), &next, 0o644)
}

fn clear_hosts() -> Result<(), String> {
    rewrite_hosts(&[])
}

/// Fetches the pinned upstream build the first time somebody turns Tailscale
/// on, exactly the way the Sync engine arrives: the digest is pinned to a
/// versioned address, nothing is trusted on arrival, and an interrupted
/// download never leaves a partial file at a path the next run would take
/// for an installed binary.
fn ensure_engine() -> Result<(), String> {
    if tailscaled().exists() && tailscale_cli().exists() {
        return Ok(());
    }
    let bytes = kobo_net::fetch(TARBALL_URL, TARBALL_LIMIT)
        .map_err(|_| "Tailscale could not be downloaded. Check Wi-Fi and try again.".to_owned())?;
    if kobo_net::sha256::hex_digest(&bytes) != TARBALL_SHA256 {
        return Err("Tailscale download did not match its checksum.".to_owned());
    }
    fs::create_dir_all(BIN_DIR).map_err(|error| format!("create {BIN_DIR}: {error}"))?;
    let partial = Path::new(BIN_DIR).join("tailscale.tgz.part");
    fs::write(&partial, &bytes).map_err(|error| format!("write Tailscale download: {error}"))?;
    // busybox tar is certain to be present; the archive holds exactly the two
    // static binaries under a single versioned directory.
    let output = Command::new("tar")
        .arg("-xzf")
        .arg(&partial)
        .arg("-C")
        .arg(BIN_DIR)
        .arg("--strip-components=1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("unpack Tailscale: {error}"))?;
    let _ignored = fs::remove_file(&partial);
    if !output.status.success() {
        return Err(format!(
            "unpack Tailscale: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

/// Refuses binaries this build did not pin by digest. The tarball digest pins
/// the pair together; each executable is then checked the way the Sync engine
/// is: a real root-owned file, not world-writable, not a symlink.
fn verify_engine() -> Result<(), String> {
    for binary in [tailscaled(), tailscale_cli()] {
        let metadata = fs::symlink_metadata(&binary)
            .map_err(|_| "Tailscale is not installed. Check Wi-Fi and try again.".to_owned())?;
        if !metadata.file_type().is_file()
            || metadata.file_type().is_symlink()
            || metadata.mode() & 0o111 == 0
            || metadata.mode() & 0o022 != 0
            || metadata.uid() != 0
        {
            return Err(
                "Tailscale is unsafe or corrupt. Install the platform update again.".to_owned(),
            );
        }
    }
    Ok(())
}

fn read_status(state: &Path) -> String {
    fs::read_to_string(state.join("tailscale-status"))
        .unwrap_or_else(|_| "disabled\n\nTailscale has not run.\n".to_owned())
}

fn write_status(state: &Path, status: &str) -> Result<(), String> {
    fs::create_dir_all(state).map_err(|error| format!("create Tailscale state: {error}"))?;
    atomic_write(&state.join("tailscale-status"), status, 0o644)
}

fn atomic_write(path: &Path, value: &str, mode: u32) -> Result<(), String> {
    use std::io::Write as _;

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

#[cfg(test)]
mod tests {
    use super::*;

    const STATUS_JSON: &str = r#"{
        "BackendState": "Running",
        "Self": {"DNSName": "kobo-cobalt.tail1234.ts.net.", "TailscaleIPs": ["100.64.0.7", "fd7a::7"]},
        "Peer": {
            "node1": {"DNSName": "myserver.tail1234.ts.net.", "TailscaleIPs": ["100.64.0.10"]},
            "node2": {"DNSName": "", "TailscaleIPs": []}
        }
    }"#;

    #[test]
    fn backend_state_is_read_from_status_json() {
        assert_eq!(parse_backend_state(STATUS_JSON), Ok(BackendState::Running));
        assert_eq!(
            parse_backend_state(r#"{"BackendState":"NeedsLogin"}"#),
            Ok(BackendState::NeedsLogin)
        );
        assert_eq!(
            parse_backend_state(r#"{"BackendState":"Stopped"}"#),
            Ok(BackendState::Stopped)
        );
        assert!(parse_backend_state("not json").is_err());
    }

    #[test]
    fn self_address_and_peer_count_come_from_status() {
        let (ip, peers) = parse_self_and_peers(STATUS_JSON);
        assert_eq!(ip, "100.64.0.7");
        assert_eq!(peers, 2);
        let (ip, peers) = parse_self_and_peers(r#"{"BackendState":"NeedsLogin"}"#);
        assert!(ip.is_empty());
        assert_eq!(peers, 0);
    }

    #[test]
    fn login_url_is_found_in_daemon_output() {
        let output = "\nTo authenticate, visit:\n\n\thttps://login.tailscale.com/a/1a2b3c4d\n\n";
        assert_eq!(
            login_url(output),
            Some("https://login.tailscale.com/a/1a2b3c4d")
        );
        assert_eq!(login_url("Success."), None);
        assert_eq!(
            auth_url(r#"{"AuthURL":"https://login.tailscale.com/a/code"}"#),
            Some("https://login.tailscale.com/a/code".to_owned())
        );
        assert_eq!(auth_url(r#"{"AuthURL":"https://example.com/no"}"#), None);
    }

    #[test]
    fn hosts_block_carries_full_and_short_names() {
        let block = hosts_block(STATUS_JSON);
        assert_eq!(
            block,
            vec![
                "100.64.0.10\tmyserver.tail1234.ts.net myserver".to_owned(),
                "100.64.0.7\tkobo-cobalt.tail1234.ts.net kobo-cobalt".to_owned(),
            ]
        );
        assert!(hosts_block("not json").is_empty());
    }

    #[test]
    fn settings_reject_anything_but_a_boolean_word() {
        assert!(Settings::parse("true\n").unwrap().enabled);
        assert!(!Settings::parse("false\n").unwrap().enabled);
        assert!(Settings::parse("yes\n").is_err());
    }
}

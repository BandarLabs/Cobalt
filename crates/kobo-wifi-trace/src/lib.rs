#![forbid(unsafe_code)]

use kobo_json::{ObjectBuilder, Value};
use ring::digest::{digest, SHA256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::env;
use std::ffi::OsStr;
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, UNIX_EPOCH};

pub const TRACE_VERSION: u32 = 1;
pub const ENABLE_ENV: &str = "KOBO_WIFI_HANDOFF_TRACE";
pub const ENABLE_PHRASE: &str = "OWNER_ATTENDED_N365_WIFI_HANDOFF_TRACE";
pub const ACTIVE_PROBES_ENV: &str = "KOBO_WIFI_HANDOFF_ACTIVE_PROBES";
pub const ACTIVE_PROBES_PHRASE: &str = "OWNER_ATTENDED_BOUNDED_WIFI_PROBES";
pub const DIAGNOSTICS_DIR: &str = "/mnt/onboard/.adds/cobalt/diagnostics";
// Leaves room for the final bounded tool batch so wall-clock lifetime still
// remains below the documented 15-minute ceiling.
pub const MAXIMUM_RUNTIME: Duration = Duration::from_secs(14 * 60 + 30);
pub const SOAK_AFTER_NICKEL: Duration = Duration::from_secs(10 * 60);
pub const RAPID_INTERVAL: Duration = Duration::from_millis(200);
pub const SOAK_INTERVAL: Duration = Duration::from_secs(5);
const INITIAL_RAPID_WINDOW: Duration = Duration::from_secs(90);
const CHECKPOINT_RAPID_WINDOW: Duration = Duration::from_secs(30);
const EXTERNAL_RAPID_INTERVAL: Duration = Duration::from_secs(1);
const KERNEL_INTERVAL: Duration = Duration::from_secs(5);
const ACTIVE_PROBE_INTERVAL: Duration = Duration::from_secs(30);
const SYNC_INTERVAL: Duration = Duration::from_secs(2);
const MAX_TRACE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_RETAINED_TRACES: usize = 4;
const HTTP_PROBE_URL: &str =
    "https://github.com/BandarLabs/Cobalt/releases/download/app-catalog/cobalt-app-catalog.json.sig";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Lifecycle {
    PreStop,
    NickelStopped,
    RecoveryBegin,
    RecoveryFirstSuccess,
    NickelStartRequested,
    NickelPidObserved,
    NickelRecoveryBegin,
    NickelRecoveryFirstSuccess,
    Current94GateAccepted,
    KobodExit,
}

impl Lifecycle {
    const fn name(self) -> &'static str {
        match self {
            Self::PreStop => "pre_stop",
            Self::NickelStopped => "nickel_stopped",
            Self::RecoveryBegin => "recovery_begin",
            Self::RecoveryFirstSuccess => "recovery_first_success",
            Self::NickelStartRequested => "nickel_start_requested",
            Self::NickelPidObserved => "nickel_pid_observed",
            Self::NickelRecoveryBegin => "nickel_recovery_begin",
            Self::NickelRecoveryFirstSuccess => "nickel_recovery_first_success",
            Self::Current94GateAccepted => "current_94_gate_accepted",
            Self::KobodExit => "kobod_exit",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Some(match value.trim() {
            "pre_stop" => Self::PreStop,
            "nickel_stopped" => Self::NickelStopped,
            "recovery_begin" => Self::RecoveryBegin,
            "recovery_first_success" => Self::RecoveryFirstSuccess,
            "nickel_start_requested" => Self::NickelStartRequested,
            "nickel_pid_observed" => Self::NickelPidObserved,
            "nickel_recovery_begin" => Self::NickelRecoveryBegin,
            "nickel_recovery_first_success" => Self::NickelRecoveryFirstSuccess,
            "current_94_gate_accepted" => Self::Current94GateAccepted,
            "kobod_exit" => Self::KobodExit,
            _ => return None,
        })
    }
}

pub struct TraceClient {
    events: Option<File>,
    trace_path: Option<PathBuf>,
}

impl TraceClient {
    /// Starts the detached helper only under the exact owner-attended unlock.
    ///
    /// With no unlock this performs no filesystem access, starts no process,
    /// and returns a client whose checkpoints are no-ops.
    ///
    /// # Errors
    ///
    /// Returns an error in attended mode when the diagnostics directory,
    /// event journal, helper process, or durable baseline cannot be created.
    pub fn start_if_enabled() -> io::Result<Self> {
        if !trace_unlocked(env::var(ENABLE_ENV).ok().as_deref()) {
            return Ok(Self {
                events: None,
                trace_path: None,
            });
        }

        let active = env::var(ACTIVE_PROBES_ENV).ok().as_deref() == Some(ACTIVE_PROBES_PHRASE);
        let session = format!("{}-{}", monotonic_millis(), std::process::id());
        let event_path = PathBuf::from(format!("/tmp/cobalt-wifi-handoff-{session}.events"));
        let trace_path =
            PathBuf::from(DIAGNOSTICS_DIR).join(format!("wifi-handoff-v1-{session}.jsonl"));
        fs::create_dir_all(DIAGNOSTICS_DIR)?;
        let events = OpenOptions::new()
            .create_new(true)
            .append(true)
            .open(&event_path)?;
        let helper = env::current_exe()?
            .parent()
            .unwrap_or_else(|| Path::new(DIAGNOSTICS_DIR))
            .join("kobo-wifi-trace");
        if !helper.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("diagnostic helper is missing at {}", helper.display()),
            ));
        }
        let probes = if active { "1" } else { "0" };
        let nohup = ["/usr/bin/nohup", "/bin/nohup"]
            .into_iter()
            .find(|path| Path::new(path).is_file());
        let (launcher, applet) = match nohup {
            Some(path) => (path, None),
            None if Path::new("/bin/busybox").is_file() => ("/bin/busybox", Some("nohup")),
            None => {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "firmware nohup is missing",
                ));
            }
        };
        let mut command = Command::new(launcher);
        if let Some(applet) = applet {
            command.arg(applet);
        }
        let child = command
            .arg(&helper)
            .arg("--run")
            .arg(&event_path)
            .arg(&trace_path)
            .arg(probes)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        drop(child);
        let ready_by = Instant::now() + Duration::from_secs(20);
        while Instant::now() < ready_by {
            if fs::read_to_string(&trace_path)
                .is_ok_and(|trace| trace.contains("\"reason\":\"baseline\""))
            {
                return Ok(Self {
                    events: Some(events),
                    trace_path: Some(trace_path),
                });
            }
            thread::sleep(Duration::from_millis(50));
        }
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "Wi-Fi trace helper did not write its baseline",
        ))
    }

    #[must_use]
    pub fn enabled(&self) -> bool {
        self.events.is_some()
    }

    #[must_use]
    pub fn trace_path(&self) -> Option<&Path> {
        self.trace_path.as_deref()
    }

    pub fn checkpoint(&mut self, event: Lifecycle) {
        let Some(events) = self.events.as_mut() else {
            return;
        };
        let line = format!("{}\n", event.name());
        let _ignored = events.write_all(line.as_bytes());
        let _ignored = events.flush();
    }

    #[cfg(test)]
    fn disabled() -> Self {
        Self {
            events: None,
            trace_path: None,
        }
    }
}

fn trace_unlocked(value: Option<&str>) -> bool {
    value == Some(ENABLE_PHRASE)
}

impl Drop for TraceClient {
    fn drop(&mut self) {
        self.checkpoint(Lifecycle::KobodExit);
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
struct ProcessIdentity {
    pid: i32,
    starttime: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ProcessRecord {
    role: &'static str,
    identity: ProcessIdentity,
    ppid: i32,
    executable: String,
    cmdline_sha256: String,
    generation: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileRecord {
    path_sha256: String,
    kind: &'static str,
    inode: u64,
    size: u64,
    mtime: i64,
    content_sha256: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RouteState {
    default_present: bool,
    default_count: usize,
    up: bool,
    up_count: usize,
    gateway: bool,
    gateway_count: usize,
    metric: String,
    prefix: String,
}

impl Default for RouteState {
    fn default() -> Self {
        Self {
            default_present: false,
            default_count: 0,
            up: false,
            up_count: 0,
            gateway: false,
            gateway_count: 0,
            metric: "none".to_owned(),
            prefix: "none".to_owned(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ProbeState {
    gateway: String,
    dns: String,
    http: String,
}

impl ProbeState {
    fn disabled() -> Self {
        Self {
            gateway: "disabled".to_owned(),
            dns: "disabled".to_owned(),
            http: "disabled".to_owned(),
        }
    }
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, Eq, PartialEq)]
struct Snapshot {
    processes: Vec<ProcessRecord>,
    nickel_count: usize,
    supplicant_count: usize,
    dhcp_count: usize,
    dbus_process_count: usize,
    tuple: String,
    wpa_socket: String,
    dbus_owner: String,
    wlan_present: bool,
    operstate: String,
    carrier: String,
    flag_up: bool,
    flag_running: bool,
    driver: String,
    module: String,
    wpa_state: String,
    associated: bool,
    ipv4: String,
    route: RouteState,
    resolver: String,
    resolver_count: usize,
    resolver_search: bool,
    dhcp_files: Vec<FileRecord>,
    probes: ProbeState,
    watchdog: String,
    power_state: String,
    wakeup_count: String,
    suspend_success: String,
    suspend_fail: String,
    reboot_reason: String,
    pstore: String,
    last_kmsg: String,
}

#[derive(Default)]
struct Generations {
    next: BTreeMap<&'static str, u32>,
    known: HashMap<(&'static str, ProcessIdentity), u32>,
}

impl Generations {
    fn number(&mut self, role: &'static str, identity: &ProcessIdentity) -> u32 {
        let key = (role, identity.clone());
        if let Some(generation) = self.known.get(&key) {
            return *generation;
        }
        let next = self.next.entry(role).or_insert(0);
        *next += 1;
        self.known.insert(key, *next);
        *next
    }
}

struct ExternalCache {
    refreshed: Option<Instant>,
    kernel_refreshed: Option<Instant>,
    probes_refreshed: Option<Instant>,
    wpa_state: String,
    ipv4: String,
    dbus_owner: String,
    probes: ProbeState,
    seen_kernel: BTreeSet<String>,
}

impl Default for ExternalCache {
    fn default() -> Self {
        Self {
            refreshed: None,
            kernel_refreshed: None,
            probes_refreshed: None,
            wpa_state: "tool_missing".to_owned(),
            ipv4: "tool_missing".to_owned(),
            dbus_owner: "tool_missing".to_owned(),
            probes: ProbeState::disabled(),
            seen_kernel: BTreeSet::new(),
        }
    }
}

struct Sampler {
    root: PathBuf,
    generations: Generations,
    external: ExternalCache,
    active_probes: bool,
}

impl Sampler {
    fn device(active_probes: bool) -> Self {
        // The baseline is passive and must exist before Nickel is stopped.
        // Active reachability begins on the first 30-second probe interval,
        // after the process/sysfs/route baseline is already durable.
        let external = ExternalCache {
            probes_refreshed: active_probes.then(Instant::now),
            ..ExternalCache::default()
        };
        Self {
            root: PathBuf::from("/"),
            generations: Generations::default(),
            external,
            active_probes,
        }
    }

    #[cfg(test)]
    fn at(root: PathBuf) -> Self {
        Self {
            root,
            generations: Generations::default(),
            external: ExternalCache::default(),
            active_probes: false,
        }
    }

    fn path(&self, absolute: &str) -> PathBuf {
        if self.root == Path::new("/") {
            PathBuf::from(absolute)
        } else {
            self.root.join(absolute.trim_start_matches('/'))
        }
    }

    #[allow(clippy::too_many_lines)]
    fn sample(&mut self, now: Instant, rapid: bool) -> (Snapshot, Vec<KernelEvent>) {
        let external_interval = if rapid {
            EXTERNAL_RAPID_INTERVAL
        } else {
            SOAK_INTERVAL
        };
        if self
            .external
            .refreshed
            .is_none_or(|taken| now.duration_since(taken) >= external_interval)
        {
            self.external.refreshed = Some(now);
            if self.root == Path::new("/") {
                self.external.wpa_state = read_wpa_state();
                self.external.ipv4 = read_ipv4_category();
                self.external.dbus_owner = read_dbus_owner();
            }
        }
        let (route, gateway_probe_target) =
            parse_routes(&fs::read_to_string(self.path("/proc/net/route")).unwrap_or_default());
        if self.active_probes
            && self.root == Path::new("/")
            && self
                .external
                .probes_refreshed
                .is_none_or(|taken| now.duration_since(taken) >= ACTIVE_PROBE_INTERVAL)
        {
            self.external.probes_refreshed = Some(now);
            self.external.probes = run_active_probes(gateway_probe_target.as_deref());
        }
        let kernel = if self.root == Path::new("/")
            && self
                .external
                .kernel_refreshed
                .is_none_or(|taken| now.duration_since(taken) >= KERNEL_INTERVAL)
        {
            self.external.kernel_refreshed = Some(now);
            collect_kernel_events(&mut self.external.seen_kernel)
        } else {
            Vec::new()
        };

        let processes = self.processes();
        let nickel_count = count_role(&processes, "nickel");
        let supplicant_count = count_role(&processes, "supplicant");
        let dhcp_count = count_role(&processes, "dhcp");
        let dbus_process_count = count_role(&processes, "dhcpcd_dbus");
        let wpa_socket = socket_identity(&self.path("/var/run/wpa_supplicant/wlan0"));
        let tuple = generation_tuple(&processes, &self.external.dbus_owner, &wpa_socket);
        let wlan = self.path("/sys/class/net/wlan0");
        let flags = read_hex(&wlan.join("flags")).unwrap_or(0);
        let resolver_text = fs::read_to_string(self.path("/etc/resolv.conf")).unwrap_or_default();
        let (resolver, resolver_count, resolver_search) = resolver_category(&resolver_text);
        let driver = link_name(&wlan.join("device/driver"));
        let module = link_name(&wlan.join("device/driver/module"));
        let watchdog = category_text(&self.path("/proc/wdk"), &["0", "1"], &["slack", "armed"]);
        let power_text = fs::read_to_string(self.path("/sys/power/state")).unwrap_or_default();
        let power_state = if power_text.is_empty() {
            "missing".to_owned()
        } else {
            format!(
                "freeze:{};standby:{};mem:{}",
                power_text.split_whitespace().any(|value| value == "freeze"),
                power_text
                    .split_whitespace()
                    .any(|value| value == "standby"),
                power_text.split_whitespace().any(|value| value == "mem")
            )
        };
        let wakeup_count = bounded_number(&self.path("/sys/power/wakeup_count"));
        let suspend_success = bounded_number(&self.path("/sys/kernel/debug/suspend_stats/success"));
        let suspend_fail = bounded_number(&self.path("/sys/kernel/debug/suspend_stats/fail"));
        let reboot_reason = first_digest(&[
            self.path("/proc/bootreason"),
            self.path("/proc/sys/kernel/boot_reason"),
            self.path("/sys/devices/platform/mtk-kpd/reboot_reason"),
        ]);
        let pstore = evidence_state(&self.path("/sys/fs/pstore"));
        let last_kmsg = if self.path("/proc/last_kmsg").is_file() {
            "present"
        } else {
            "missing"
        }
        .to_owned();
        let operstate = safe_state(
            &fs::read_to_string(wlan.join("operstate")).unwrap_or_default(),
            &["up", "down", "dormant", "unknown", "lowerlayerdown"],
        );
        let carrier = safe_state(
            &fs::read_to_string(wlan.join("carrier")).unwrap_or_default(),
            &["0", "1"],
        );
        let wpa_state = self.external.wpa_state.clone();
        let associated = wpa_state == "completed";
        (
            Snapshot {
                processes,
                nickel_count,
                supplicant_count,
                dhcp_count,
                dbus_process_count,
                tuple,
                wpa_socket,
                dbus_owner: self.external.dbus_owner.clone(),
                wlan_present: wlan.exists(),
                operstate,
                carrier,
                flag_up: flags & 0x1 != 0,
                flag_running: flags & 0x40 != 0,
                driver,
                module,
                wpa_state,
                associated,
                ipv4: self.external.ipv4.clone(),
                route,
                resolver,
                resolver_count,
                resolver_search,
                dhcp_files: self.dhcp_files(),
                probes: self.external.probes.clone(),
                watchdog,
                power_state,
                wakeup_count,
                suspend_success,
                suspend_fail,
                reboot_reason,
                pstore,
                last_kmsg,
            },
            kernel,
        )
    }

    fn processes(&mut self) -> Vec<ProcessRecord> {
        let proc_root = self.path("/proc");
        let Ok(entries) = fs::read_dir(&proc_root) else {
            return Vec::new();
        };
        let mut records = Vec::new();
        for entry in entries.flatten().take(512) {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<i32>().ok())
            else {
                continue;
            };
            let directory = entry.path();
            let cmdline = read_bounded(&directory.join("cmdline"), 64 * 1024).unwrap_or_default();
            let executable_path = fs::read_link(directory.join("exe")).ok();
            let Some(role) = process_role(executable_path.as_deref(), &cmdline) else {
                continue;
            };
            let stat = fs::read_to_string(directory.join("stat")).unwrap_or_default();
            let Some((parent_pid, starttime)) = parse_process_stat(&stat) else {
                continue;
            };
            let identity = ProcessIdentity { pid, starttime };
            let generation = self.generations.number(role, &identity);
            records.push(ProcessRecord {
                role,
                identity,
                ppid: parent_pid,
                executable: safe_executable(executable_path.as_deref()),
                cmdline_sha256: sha256_hex(&cmdline),
                generation,
            });
        }
        records.sort_by_key(|record| (record.role, record.identity.pid, record.identity.starttime));
        records
    }

    fn dhcp_files(&self) -> Vec<FileRecord> {
        let mut paths = Vec::new();
        for exact in [
            "/var/run/dhcpcd.pid",
            "/var/run/dhcpcd.sock",
            "/var/run/dhcpcd.unpriv.sock",
            "/var/run/dhcpcd-wlan0.pid",
            "/var/run/dhcpcd-wlan0.sock",
            "/var/lib/dhcpcd/dhcpcd-wlan0.lease",
            "/var/db/dhcpcd-wlan0.lease",
        ] {
            paths.push(self.path(exact));
        }
        for directory in ["/var/run", "/var/run/dhcpcd", "/var/lib/dhcpcd", "/var/db"] {
            let directory = self.path(directory);
            let Ok(entries) = fs::read_dir(directory) else {
                continue;
            };
            for entry in entries
                .flatten()
                .filter(|entry| entry.file_name().to_string_lossy().starts_with("dhcpcd"))
                .take(64)
            {
                if matches!(
                    entry.path().extension().and_then(OsStr::to_str),
                    Some("pid" | "sock" | "lease")
                ) {
                    paths.push(entry.path());
                }
            }
        }
        paths.sort();
        paths.dedup();
        paths
            .into_iter()
            .filter_map(|path| file_record(&path))
            .collect()
    }
}

#[derive(Clone, Debug)]
struct KernelEvent {
    category: &'static str,
    tags: String,
    digest: String,
}

struct TraceWriter {
    file: File,
    bytes: u64,
    last_sync: Instant,
}

impl TraceWriter {
    fn open(path: &Path, active_probes: bool) -> io::Result<Self> {
        let existed = path.exists();
        if existed
            && !fs::symlink_metadata(path)
                .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "existing trace path is not a regular file",
            ));
        }
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        let mut writer = Self {
            bytes: file.metadata()?.len(),
            file,
            last_sync: Instant::now(),
        };
        writer.line(
            &ObjectBuilder::new()
                .set("version", TRACE_VERSION)
                .set("mono_ms", 0_u32)
                .set(
                    "kind",
                    if existed {
                        "tracer_restart"
                    } else {
                        "trace_start"
                    },
                )
                .set("active_probes", active_probes)
                .set("maximum_runtime_ms", json_duration(MAXIMUM_RUNTIME))
                .set("soak_after_nickel_ms", json_duration(SOAK_AFTER_NICKEL))
                .build(),
            true,
        )?;
        Ok(writer)
    }

    fn line(&mut self, value: &Value, critical: bool) -> io::Result<()> {
        if self.bytes >= MAX_TRACE_BYTES {
            return Ok(());
        }
        let mut line = value.to_json();
        line.push('\n');
        self.file.write_all(line.as_bytes())?;
        self.file.flush()?;
        self.bytes = self
            .bytes
            .saturating_add(u64::try_from(line.len()).unwrap_or(u64::MAX));
        if critical || self.last_sync.elapsed() >= SYNC_INTERVAL {
            self.file.sync_data()?;
            self.last_sync = Instant::now();
        }
        Ok(())
    }

    fn snapshot(&mut self, mono_ms: u64, reason: &str, snapshot: &Snapshot) -> io::Result<()> {
        self.line(&snapshot_value(mono_ms, reason, snapshot), false)
    }

    fn lifecycle(&mut self, mono_ms: u64, event: Lifecycle) -> io::Result<()> {
        self.line(
            &ObjectBuilder::new()
                .set("version", TRACE_VERSION)
                .set("mono_ms", json_u64(mono_ms))
                .set("kind", "lifecycle")
                .set("event", event.name())
                .build(),
            true,
        )
    }

    fn kernel(&mut self, mono_ms: u64, event: &KernelEvent) -> io::Result<()> {
        self.line(
            &ObjectBuilder::new()
                .set("version", TRACE_VERSION)
                .set("mono_ms", json_u64(mono_ms))
                .set("kind", "kernel")
                .set("category", event.category)
                .set("tags", event.tags.clone())
                .set("line_sha256", event.digest.clone())
                .build(),
            false,
        )
    }
}

/// Runs the bounded device-side sampler until its soak or hard deadline.
///
/// # Errors
///
/// Returns an error when fixed trace paths are invalid or when the event
/// journal or append-only trace cannot be read, written, flushed, or synced.
pub fn run_device_trace(event_path: &Path, trace_path: &Path, active: bool) -> io::Result<()> {
    if trace_path
        .parent()
        .is_none_or(|parent| parent != Path::new(DIAGNOSTICS_DIR))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "trace must stay in the diagnostics directory",
        ));
    }
    cleanup_stale_traces(Path::new(DIAGNOSTICS_DIR), Some(trace_path))?;
    let mut writer = TraceWriter::open(trace_path, active)?;
    let started = Instant::now();
    let hard_deadline = started + MAXIMUM_RUNTIME;
    let mut rapid_until = started + INITIAL_RAPID_WINDOW;
    let mut finish_at = hard_deadline;
    let mut events = OpenOptions::new().read(true).open(event_path)?;
    let mut event_offset = 0_u64;
    let mut sampler = Sampler::device(active);
    let (baseline, kernel) = sampler.sample(Instant::now(), true);
    writer.line(&snapshot_value(0, "baseline", &baseline), true)?;
    for event in kernel {
        writer.kernel(0, &event)?;
    }
    let mut previous = Some(baseline);
    let mut last_snapshot = Instant::now();
    loop {
        let now = Instant::now();
        if now >= finish_at || now >= hard_deadline || writer.bytes >= MAX_TRACE_BYTES {
            break;
        }
        for event in read_lifecycle_events(&mut events, &mut event_offset)? {
            let mono_ms = elapsed_millis(started, now);
            writer.lifecycle(mono_ms, event)?;
            if matches!(
                event,
                Lifecycle::PreStop
                    | Lifecycle::NickelStopped
                    | Lifecycle::RecoveryBegin
                    | Lifecycle::NickelStartRequested
                    | Lifecycle::NickelPidObserved
                    | Lifecycle::NickelRecoveryBegin
            ) {
                rapid_until = (now + CHECKPOINT_RAPID_WINDOW).min(hard_deadline);
            }
            if matches!(event, Lifecycle::NickelPidObserved | Lifecycle::KobodExit) {
                finish_at = (now + SOAK_AFTER_NICKEL).min(hard_deadline);
            }
        }
        let rapid = now < rapid_until;
        let (snapshot, kernel) = sampler.sample(now, rapid);
        let changed = previous.as_ref() != Some(&snapshot);
        if changed || !rapid || last_snapshot.elapsed() >= Duration::from_secs(2) {
            if changed {
                writer.line(
                    &snapshot_value(elapsed_millis(started, now), "change", &snapshot),
                    true,
                )?;
            } else {
                writer.snapshot(elapsed_millis(started, now), "sample", &snapshot)?;
            }
            previous = Some(snapshot);
            last_snapshot = now;
        }
        for event in kernel {
            writer.kernel(elapsed_millis(started, now), &event)?;
        }
        let interval = if rapid { RAPID_INTERVAL } else { SOAK_INTERVAL };
        thread::sleep(interval.min(finish_at.saturating_duration_since(Instant::now())));
    }
    writer.line(
        &ObjectBuilder::new()
            .set("version", TRACE_VERSION)
            .set("mono_ms", json_u64(elapsed_millis(started, Instant::now())))
            .set("kind", "trace_end")
            .set(
                "reason",
                if writer.bytes >= MAX_TRACE_BYTES {
                    "size_limit"
                } else {
                    "deadline"
                },
            )
            .build(),
        true,
    )?;
    let _ignored = fs::remove_file(event_path);
    Ok(())
}

fn read_lifecycle_events(file: &mut File, offset: &mut u64) -> io::Result<Vec<Lifecycle>> {
    file.seek(SeekFrom::Start(*offset))?;
    let mut reader = BufReader::new(file);
    let mut events = Vec::new();
    let mut consumed = 0_u64;
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            break;
        }
        if line.ends_with('\n') {
            consumed = consumed.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
            if let Some(event) = Lifecycle::parse(&line) {
                events.push(event);
            }
        } else {
            break;
        }
    }
    *offset = offset.saturating_add(consumed);
    Ok(events)
}

fn snapshot_value(mono_ms: u64, reason: &str, snapshot: &Snapshot) -> Value {
    let processes = snapshot
        .processes
        .iter()
        .map(|process| {
            ObjectBuilder::new()
                .set("role", process.role)
                .set("pid", process.identity.pid)
                .set("ppid", process.ppid)
                .set("starttime", json_u64(process.identity.starttime))
                .set("generation", process.generation)
                .set("exe", process.executable.clone())
                .set("cmdline_sha256", process.cmdline_sha256.clone())
                .build()
        })
        .collect::<Vec<_>>();
    let files = snapshot
        .dhcp_files
        .iter()
        .map(|file| {
            ObjectBuilder::new()
                .set("path_sha256", file.path_sha256.clone())
                .set("kind", file.kind)
                .set("inode", json_u64(file.inode))
                .set("size", json_u64(file.size))
                .set("mtime", json_i64(file.mtime))
                .set(
                    "content_sha256",
                    file.content_sha256.clone().map_or(Value::Null, Value::from),
                )
                .build()
        })
        .collect::<Vec<_>>();
    ObjectBuilder::new()
        .set("version", TRACE_VERSION)
        .set("mono_ms", json_u64(mono_ms))
        .set("kind", "snapshot")
        .set("reason", reason)
        .set("processes", processes)
        .set("tuple", snapshot.tuple.clone())
        .set("nickel_count", json_usize(snapshot.nickel_count))
        .set("supplicant_count", json_usize(snapshot.supplicant_count))
        .set("dhcp_count", json_usize(snapshot.dhcp_count))
        .set(
            "dbus_process_count",
            json_usize(snapshot.dbus_process_count),
        )
        .set("wpa_socket", snapshot.wpa_socket.clone())
        .set("dbus_owner", snapshot.dbus_owner.clone())
        .set("wlan_present", snapshot.wlan_present)
        .set("operstate", snapshot.operstate.clone())
        .set("carrier", snapshot.carrier.clone())
        .set("flag_up", snapshot.flag_up)
        .set("flag_running", snapshot.flag_running)
        .set("driver", snapshot.driver.clone())
        .set("module", snapshot.module.clone())
        .set("wpa_state", snapshot.wpa_state.clone())
        .set("associated", snapshot.associated)
        .set("ipv4", snapshot.ipv4.clone())
        .set("default_route", snapshot.route.default_present)
        .set(
            "default_route_count",
            json_usize(snapshot.route.default_count),
        )
        .set("route_up", snapshot.route.up)
        .set("route_up_count", json_usize(snapshot.route.up_count))
        .set("route_gateway", snapshot.route.gateway)
        .set(
            "route_gateway_count",
            json_usize(snapshot.route.gateway_count),
        )
        .set("route_metric", snapshot.route.metric.clone())
        .set("route_prefix", snapshot.route.prefix.clone())
        .set("resolver", snapshot.resolver.clone())
        .set("resolver_count", json_usize(snapshot.resolver_count))
        .set("resolver_search", snapshot.resolver_search)
        .set("dhcp_files", files)
        .set("gateway_probe", snapshot.probes.gateway.clone())
        .set("dns_probe", snapshot.probes.dns.clone())
        .set("http_probe", snapshot.probes.http.clone())
        .set("watchdog", snapshot.watchdog.clone())
        .set("power_state", snapshot.power_state.clone())
        .set("wakeup_count", snapshot.wakeup_count.clone())
        .set("suspend_success", snapshot.suspend_success.clone())
        .set("suspend_fail", snapshot.suspend_fail.clone())
        .set("reboot_reason", snapshot.reboot_reason.clone())
        .set("pstore", snapshot.pstore.clone())
        .set("last_kmsg", snapshot.last_kmsg.clone())
        .build()
}

fn process_role(executable: Option<&Path>, cmdline: &[u8]) -> Option<&'static str> {
    let exe = executable.and_then(Path::file_name).and_then(OsStr::to_str);
    let argv0 = cmdline
        .split(|byte| *byte == 0)
        .next()
        .and_then(|value| std::str::from_utf8(value).ok())
        .and_then(|value| Path::new(value).file_name())
        .and_then(OsStr::to_str);
    for name in [exe, argv0].into_iter().flatten() {
        match name {
            "nickel" => return Some("nickel"),
            "wpa_supplicant" => return Some("supplicant"),
            "dhcpcd" => return Some("dhcp"),
            "dhcpcd-dbus" | "dhcpcd_dbus" => return Some("dhcpcd_dbus"),
            _ => {}
        }
    }
    None
}

fn parse_process_stat(stat: &str) -> Option<(i32, u64)> {
    let close = stat.rfind(')')?;
    let fields = stat
        .get(close + 1..)?
        .split_whitespace()
        .collect::<Vec<_>>();
    let ppid = fields.get(1)?.parse().ok()?;
    let starttime = fields.get(19)?.parse().ok()?;
    Some((ppid, starttime))
}

fn safe_executable(path: Option<&Path>) -> String {
    let Some(path) = path else {
        return "unreadable".to_owned();
    };
    let text = path.to_string_lossy();
    if ["/bin/", "/sbin/", "/usr/", "/lib/", "/opt/"]
        .iter()
        .any(|prefix| text.starts_with(prefix))
    {
        text.into_owned()
    } else {
        format!("other:{}", sha256_hex(text.as_bytes()))
    }
}

fn generation_tuple(processes: &[ProcessRecord], dbus_owner: &str, socket: &str) -> String {
    let part = |role: &str, prefix: char| {
        let generations = processes
            .iter()
            .filter(|process| process.role == role)
            .map(|process| process.generation.to_string())
            .collect::<Vec<_>>();
        if generations.is_empty() {
            format!("{prefix}0")
        } else {
            format!("{prefix}{}", generations.join("+"))
        }
    };
    format!(
        "{};{};{};{};B={};C={}",
        part("nickel", 'N'),
        part("supplicant", 'S'),
        part("dhcp", 'D'),
        part("dhcpcd_dbus", 'B'),
        dbus_owner,
        socket
    )
}

fn count_role(processes: &[ProcessRecord], role: &str) -> usize {
    processes
        .iter()
        .filter(|process| process.role == role)
        .count()
}

fn socket_identity(path: &Path) -> String {
    fs::symlink_metadata(path).map_or_else(
        |_| "absent".to_owned(),
        |metadata| format!("inode:{}", metadata.ino()),
    )
}

fn parse_routes(table: &str) -> (RouteState, Option<String>) {
    let mut route = RouteState::default();
    let mut gateway_probe_target = None;
    let mut metrics = BTreeSet::new();
    let mut prefixes = BTreeSet::new();
    for line in table.lines().skip(1) {
        let columns = line.split_whitespace().collect::<Vec<_>>();
        if columns.len() < 8 || columns[0] != "wlan0" || columns[1] != "00000000" {
            continue;
        }
        let flags = u32::from_str_radix(columns[3], 16).unwrap_or(0);
        let metric = columns[6].parse::<u32>().unwrap_or(u32::MAX);
        let gateway = parse_route_ipv4(columns[2]);
        route.default_count += 1;
        route.up_count += usize::from(flags & 0x1 != 0);
        let has_gateway = flags & 0x2 != 0 && gateway.is_some_and(|value| value != [0; 4]);
        route.gateway_count += usize::from(has_gateway);
        metrics.insert(metric_category(metric));
        prefixes.insert(mask_category(columns[7]));
        if gateway_probe_target.is_none() && has_gateway {
            gateway_probe_target = gateway
                .map(|octets| format!("{}.{}.{}.{}", octets[0], octets[1], octets[2], octets[3]));
        }
    }
    route.default_present = route.default_count > 0;
    route.up = route.default_present && route.up_count == route.default_count;
    route.gateway = route.default_present && route.gateway_count == route.default_count;
    if route.default_present {
        route.metric = metrics.into_iter().collect::<Vec<_>>().join("+");
        route.prefix = prefixes.into_iter().collect::<Vec<_>>().join("+");
    }
    (route, gateway_probe_target)
}

fn parse_route_ipv4(value: &str) -> Option<[u8; 4]> {
    let raw = u32::from_str_radix(value, 16).ok()?;
    Some(raw.to_le_bytes())
}

const fn metric_category(metric: u32) -> &'static str {
    match metric {
        0 => "zero",
        1..=100 => "low",
        101..=500 => "medium",
        _ => "high",
    }
}

fn mask_category(mask: &str) -> &'static str {
    let Ok(raw) = u32::from_str_radix(mask, 16) else {
        return "unknown";
    };
    match raw
        .to_le_bytes()
        .iter()
        .map(|byte| byte.count_ones())
        .sum::<u32>()
    {
        0 => "default",
        24 => "24",
        16..=23 => "16_23",
        25..=32 => "25_32",
        _ => "other",
    }
}

fn resolver_category(text: &str) -> (String, usize, bool) {
    let count = text
        .lines()
        .filter(|line| line.split_whitespace().next() == Some("nameserver"))
        .count();
    let category = match count {
        0 => "none".to_owned(),
        1 => "one".to_owned(),
        _ => "multiple".to_owned(),
    };
    let search = text
        .lines()
        .any(|line| matches!(line.split_whitespace().next(), Some("search" | "domain")));
    (category, count, search)
}

fn file_record(path: &Path) -> Option<FileRecord> {
    let metadata = fs::symlink_metadata(path).ok()?;
    let kind = if metadata.file_type().is_socket() {
        "socket"
    } else if metadata.is_file() {
        "file"
    } else {
        return None;
    };
    let content_sha256 = if metadata.is_file() && metadata.len() <= 1024 * 1024 {
        read_bounded(path, 1024 * 1024).map(|content| sha256_hex(&content))
    } else {
        None
    };
    Some(FileRecord {
        path_sha256: sha256_hex(path.as_os_str().as_encoded_bytes()),
        kind,
        inode: metadata.ino(),
        size: metadata.len(),
        mtime: metadata.mtime(),
        content_sha256,
    })
}

fn link_name(path: &Path) -> String {
    let Ok(target) = fs::read_link(path) else {
        return "missing".to_owned();
    };
    let Some(name) = target.file_name().and_then(OsStr::to_str) else {
        return "unreadable".to_owned();
    };
    if name
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
    {
        name.to_owned()
    } else {
        format!("other:{}", sha256_hex(name.as_bytes()))
    }
}

fn safe_state(text: &str, allowed: &[&str]) -> String {
    let value = text.trim();
    if allowed.contains(&value) {
        value.to_owned()
    } else if value.is_empty() {
        "missing".to_owned()
    } else {
        "other".to_owned()
    }
}

fn category_text(path: &Path, values: &[&str], categories: &[&str]) -> String {
    let text = fs::read_to_string(path).unwrap_or_default();
    values
        .iter()
        .position(|value| text.trim() == *value)
        .map_or_else(
            || {
                if text.is_empty() {
                    "missing".to_owned()
                } else {
                    "other".to_owned()
                }
            },
            |index| categories[index].to_owned(),
        )
}

fn bounded_number(path: &Path) -> String {
    let text = fs::read_to_string(path).unwrap_or_default();
    text.trim()
        .parse::<u64>()
        .map_or_else(|_| "missing".to_owned(), |number| number.to_string())
}

fn evidence_state(path: &Path) -> String {
    let Ok(entries) = fs::read_dir(path) else {
        return "missing".to_owned();
    };
    if entries.flatten().next().is_some() {
        "present".to_owned()
    } else {
        "empty".to_owned()
    }
}

fn first_digest(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .find_map(|path| read_bounded(path, 4096))
        .map_or_else(
            || "missing".to_owned(),
            |content| format!("present:{}", sha256_hex(&content)),
        )
}

fn read_hex(path: &Path) -> Option<u32> {
    let text = fs::read_to_string(path).ok()?;
    u32::from_str_radix(text.trim().trim_start_matches("0x"), 16).ok()
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CommandSpec {
    executable: &'static str,
    arguments: Vec<String>,
    purpose: &'static str,
}

fn readonly_command_specs(gateway: Option<&str>) -> Vec<CommandSpec> {
    let mut commands = vec![
        CommandSpec {
            executable: "wpa_cli",
            arguments: vec!["-i", "wlan0", "status"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            purpose: "wpa_status",
        },
        CommandSpec {
            executable: "ip",
            arguments: vec!["-o", "-4", "address", "show", "dev", "wlan0"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            purpose: "ipv4",
        },
        CommandSpec {
            executable: "dbus-send",
            arguments: vec![
                "--system",
                "--print-reply=literal",
                "--dest=org.freedesktop.DBus",
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus.GetNameOwner",
                "string:name.marples.roy.dhcpcd",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            purpose: "dbus_owner",
        },
        CommandSpec {
            executable: "dmesg",
            arguments: vec!["-s".to_owned(), "262144".to_owned()],
            purpose: "kernel",
        },
        CommandSpec {
            executable: "nslookup",
            arguments: vec!["github.com".to_owned()],
            purpose: "dns_probe",
        },
        CommandSpec {
            executable: "kobod",
            arguments: vec![
                "--fetch".to_owned(),
                HTTP_PROBE_URL.to_owned(),
                "65536".to_owned(),
            ],
            purpose: "http_probe",
        },
    ];
    if let Some(gateway) = gateway.filter(|value| valid_ipv4(value)) {
        commands.push(CommandSpec {
            executable: "ping",
            arguments: vec![
                "-c".to_owned(),
                "1".to_owned(),
                "-W".to_owned(),
                "2".to_owned(),
                gateway.to_owned(),
            ],
            purpose: "gateway_probe",
        });
    }
    commands
}

fn run_spec(spec: &CommandSpec) -> Option<std::process::Output> {
    let timeout = ["/usr/bin/timeout", "/bin/timeout"]
        .into_iter()
        .find(|path| Path::new(path).is_file())?;
    let executable = match spec.executable {
        "wpa_cli" => [
            "/bin/wpa_cli",
            "/sbin/wpa_cli",
            "/usr/sbin/wpa_cli",
            "/usr/bin/wpa_cli",
        ]
        .into_iter()
        .find(|path| Path::new(path).is_file())?,
        "ip" => ["/sbin/ip", "/bin/ip", "/usr/sbin/ip", "/usr/bin/ip"]
            .into_iter()
            .find(|path| Path::new(path).is_file())?,
        "dbus-send" => ["/usr/bin/dbus-send", "/bin/dbus-send"]
            .into_iter()
            .find(|path| Path::new(path).is_file())?,
        "dmesg" => ["/bin/dmesg", "/usr/bin/dmesg"]
            .into_iter()
            .find(|path| Path::new(path).is_file())?,
        "nslookup" => ["/usr/bin/nslookup", "/bin/nslookup"]
            .into_iter()
            .find(|path| Path::new(path).is_file())?,
        "ping" => ["/bin/ping", "/usr/bin/ping"]
            .into_iter()
            .find(|path| Path::new(path).is_file())?,
        "kobod" => {
            let path = "/mnt/onboard/.adds/cobalt/bin/kobod";
            Path::new(path).is_file().then_some(path)?
        }
        _ => return None,
    };
    Command::new(timeout)
        .arg("3")
        .arg(executable)
        .args(&spec.arguments)
        .stdin(Stdio::null())
        .output()
        .ok()
}

fn spec_named(name: &str, gateway: Option<&str>) -> Option<CommandSpec> {
    readonly_command_specs(gateway)
        .into_iter()
        .find(|spec| spec.purpose == name)
}

fn read_wpa_state() -> String {
    let Some(output) = spec_named("wpa_status", None).and_then(|spec| run_spec(&spec)) else {
        return "tool_missing".to_owned();
    };
    if !output.status.success() {
        return "unavailable".to_owned();
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let state = text
        .lines()
        .find_map(|line| line.strip_prefix("wpa_state="));
    match state {
        Some("COMPLETED") => "completed",
        Some("ASSOCIATING" | "ASSOCIATED" | "4WAY_HANDSHAKE" | "GROUP_HANDSHAKE") => "associating",
        Some("SCANNING") => "scanning",
        Some("DISCONNECTED" | "INACTIVE" | "INTERFACE_DISABLED") => "disconnected",
        Some(_) => "other",
        None => "unavailable",
    }
    .to_owned()
}

fn read_ipv4_category() -> String {
    let Some(output) = spec_named("ipv4", None).and_then(|spec| run_spec(&spec)) else {
        return "tool_missing".to_owned();
    };
    if !output.status.success() {
        return "none".to_owned();
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let cidr = text
        .split_whitespace()
        .find(|word| word.contains('.') && word.contains('/'));
    cidr.map_or_else(|| "none".to_owned(), classify_cidr)
}

fn classify_cidr(cidr: &str) -> String {
    let Some((address, prefix)) = cidr.split_once('/') else {
        return "other".to_owned();
    };
    let octets = address
        .split('.')
        .filter_map(|part| part.parse::<u8>().ok())
        .collect::<Vec<_>>();
    if octets.len() != 4 || prefix.parse::<u8>().is_err() {
        return "other".to_owned();
    }
    let prefix = prefix.parse::<u8>().unwrap_or(0);
    let scope = if octets[0] == 10
        || (octets[0] == 172 && (16..=31).contains(&octets[1]))
        || (octets[0] == 192 && octets[1] == 168)
    {
        "private"
    } else if octets[0] == 169 && octets[1] == 254 {
        "link_local"
    } else {
        "public"
    };
    let prefix = match prefix {
        24 => "24",
        16..=23 => "16_23",
        25..=32 => "25_32",
        _ => "other",
    };
    format!("{scope}_{prefix}")
}

fn read_dbus_owner() -> String {
    let Some(output) = spec_named("dbus_owner", None).and_then(|spec| run_spec(&spec)) else {
        return "tool_missing".to_owned();
    };
    if !output.status.success() {
        return "absent".to_owned();
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let owner = text.split_whitespace().find(|token| token.starts_with(':'));
    owner.map_or_else(
        || "absent".to_owned(),
        |value| format!("owner:{}", sha256_hex(value.as_bytes())),
    )
}

fn run_active_probes(gateway: Option<&str>) -> ProbeState {
    let run = |purpose: &str, gateway: Option<&str>| {
        let started = Instant::now();
        let Some(output) = spec_named(purpose, gateway).and_then(|spec| run_spec(&spec)) else {
            return "unavailable".to_owned();
        };
        format!(
            "{}:{}ms",
            if output.status.success() { "yes" } else { "no" },
            started.elapsed().as_millis()
        )
    };
    ProbeState {
        gateway: gateway.map_or_else(
            || "no_gateway".to_owned(),
            |value| run("gateway_probe", Some(value)),
        ),
        dns: run("dns_probe", None),
        http: run("http_probe", None),
    }
}

fn valid_ipv4(value: &str) -> bool {
    let parts = value.split('.').collect::<Vec<_>>();
    parts.len() == 4 && parts.iter().all(|part| part.parse::<u8>().is_ok())
}

fn collect_kernel_events(seen: &mut BTreeSet<String>) -> Vec<KernelEvent> {
    let mut events = Vec::new();
    if let Some(output) = spec_named("kernel", None).and_then(|spec| run_spec(&spec)) {
        collect_kernel_text(&output.stdout, seen, &mut events);
    }
    for path in [Path::new("/proc/last_kmsg"), Path::new("/sys/fs/pstore")] {
        if path.is_file() {
            if let Some(content) = read_bounded(path, 256 * 1024) {
                collect_kernel_text(&content, seen, &mut events);
            }
            continue;
        }
        let Ok(entries) = fs::read_dir(path) else {
            continue;
        };
        for entry in entries.flatten().take(8) {
            if let Some(content) = read_bounded(&entry.path(), 256 * 1024) {
                collect_kernel_text(&content, seen, &mut events);
            }
        }
    }
    events
}

fn collect_kernel_text(content: &[u8], seen: &mut BTreeSet<String>, events: &mut Vec<KernelEvent>) {
    let text = String::from_utf8_lossy(content);
    events.extend(
        text.lines()
            .filter_map(classify_kernel_line)
            .filter(|event| seen.insert(event.digest.clone())),
    );
}

fn classify_kernel_line(line: &str) -> Option<KernelEvent> {
    let lower = line.to_ascii_lowercase();
    let category = if lower.contains("wlan_drv_gen4m") {
        "wlan_drv_gen4m"
    } else if lower.contains("wmt") {
        "wmt"
    } else if lower.contains("sdio") {
        "sdio"
    } else if lower.contains("watchdog") || lower.contains("wdt") {
        "watchdog"
    } else if lower.contains("panic") {
        "panic"
    } else if lower.contains("reset") || lower.contains("reboot") {
        "reset_reboot"
    } else if lower.contains("suspend") || lower.contains("resume") {
        "power"
    } else {
        return None;
    };
    let tags = [
        "error",
        "fail",
        "timeout",
        "hang",
        "panic",
        "reset",
        "reboot",
        "suspend",
        "resume",
        "remove",
        "probe",
        "disconnect",
    ]
    .into_iter()
    .filter(|tag| lower.contains(tag))
    .collect::<Vec<_>>()
    .join(",");
    Some(KernelEvent {
        category,
        tags,
        digest: sha256_hex(line.as_bytes()),
    })
}

fn cleanup_stale_traces(directory: &Path, current: Option<&Path>) -> io::Result<()> {
    fs::create_dir_all(directory)?;
    let mut traces = fs::read_dir(directory)?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(OsStr::to_str)
                .is_some_and(|name| name.starts_with("wifi-handoff-v1-"))
                && path
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("jsonl"))
                && current != Some(path.as_path())
        })
        .collect::<Vec<_>>();
    traces.sort_by_key(|path| {
        fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .unwrap_or(UNIX_EPOCH)
    });
    let remove = traces.len().saturating_sub(MAX_RETAINED_TRACES - 1);
    for path in traces.into_iter().take(remove) {
        fs::remove_file(path)?;
    }
    Ok(())
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, Eq, PartialEq)]
struct SummarySnapshot {
    mono_ms: u64,
    tuple: String,
    nickel_count: u64,
    supplicant_count: u64,
    dhcp_count: u64,
    dbus_process_count: u64,
    wpa_socket: String,
    dbus_owner: String,
    wlan_present: bool,
    operstate: String,
    carrier: String,
    flag_up: bool,
    flag_running: bool,
    driver: String,
    module: String,
    associated: bool,
    ipv4: String,
    default_route: bool,
    default_route_count: u64,
    route_up: bool,
    route_gateway: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TraceSummary {
    pub valid_lines: usize,
    pub ignored_lines: usize,
    pub transitions: Vec<String>,
    pub first_divergence: Option<String>,
    pub gate_mono_ms: Option<u64>,
    pub ended: bool,
}

impl TraceSummary {
    #[must_use]
    pub fn render(&self) -> String {
        let mut lines = vec![format!(
            "trace v{TRACE_VERSION}: {} valid lines, {} interrupted/invalid lines",
            self.valid_lines, self.ignored_lines
        )];
        if self.transitions.is_empty() {
            lines.push("generation transitions: none recorded".to_owned());
        } else {
            lines.push("generation transitions:".to_owned());
            lines.extend(self.transitions.iter().map(|line| format!("  {line}")));
        }
        lines.push(match (&self.first_divergence, self.gate_mono_ms) {
            (Some(divergence), Some(gate)) => {
                format!("first divergence after #94 gate at {gate}ms: {divergence}")
            }
            (None, Some(gate)) => {
                format!("no later divergence recorded after #94 gate at {gate}ms")
            }
            _ => "the #94 gate-accepted checkpoint was not recorded".to_owned(),
        });
        lines.push(if self.ended {
            "trace ended at its bounded deadline".to_owned()
        } else {
            "trace has no clean end marker (running, interrupted, or rebooted)".to_owned()
        });
        lines.join("\n")
    }
}

pub fn summarize(bytes: &[u8]) -> TraceSummary {
    let text = String::from_utf8_lossy(bytes);
    let complete = text.ends_with('\n');
    let line_count = text.lines().count();
    let mut summary = TraceSummary::default();
    let mut previous: Option<SummarySnapshot> = None;
    let mut gate_pending = false;
    let mut gate: Option<SummarySnapshot> = None;
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        if !complete && index + 1 == line_count {
            summary.ignored_lines += 1;
            continue;
        }
        let Ok(value) = kobo_json::parse(line) else {
            summary.ignored_lines += 1;
            continue;
        };
        if value.get("version").and_then(Value::as_i64) != Some(i64::from(TRACE_VERSION)) {
            summary.ignored_lines += 1;
            continue;
        }
        summary.valid_lines += 1;
        match value.get("kind").and_then(Value::as_str) {
            Some("lifecycle")
                if value.get("event").and_then(Value::as_str)
                    == Some(Lifecycle::Current94GateAccepted.name()) =>
            {
                summary.gate_mono_ms = value
                    .get("mono_ms")
                    .and_then(Value::as_i64)
                    .and_then(|value| u64::try_from(value).ok());
                gate_pending = true;
            }
            Some("trace_end") => summary.ended = true,
            Some("snapshot") => {
                let Some(snapshot) = summary_snapshot(&value) else {
                    summary.ignored_lines += 1;
                    summary.valid_lines = summary.valid_lines.saturating_sub(1);
                    continue;
                };
                if let Some(before) = previous
                    .as_ref()
                    .filter(|before| before.tuple != snapshot.tuple)
                {
                    summary.transitions.push(format!(
                        "{}ms {} -> {}",
                        snapshot.mono_ms, before.tuple, snapshot.tuple
                    ));
                }
                if gate_pending {
                    gate = Some(snapshot.clone());
                    gate_pending = false;
                } else if summary.first_divergence.is_none() {
                    if let Some(reference) = gate.as_ref() {
                        if let Some(reason) = divergence(reference, &snapshot) {
                            summary.first_divergence =
                                Some(format!("{}ms {reason}", snapshot.mono_ms));
                        }
                    }
                }
                previous = Some(snapshot);
            }
            _ => {}
        }
    }
    summary
}

fn summary_snapshot(value: &Value) -> Option<SummarySnapshot> {
    let number = |name: &str| {
        value
            .get(name)
            .and_then(Value::as_i64)
            .and_then(|number| u64::try_from(number).ok())
    };
    let text = |name: &str| value.get(name).and_then(Value::as_str).map(str::to_owned);
    let boolean = |name: &str| value.get(name).and_then(Value::as_bool);
    Some(SummarySnapshot {
        mono_ms: number("mono_ms")?,
        tuple: text("tuple")?,
        nickel_count: number("nickel_count")?,
        supplicant_count: number("supplicant_count")?,
        dhcp_count: number("dhcp_count")?,
        dbus_process_count: number("dbus_process_count")?,
        wpa_socket: text("wpa_socket")?,
        dbus_owner: text("dbus_owner")?,
        wlan_present: boolean("wlan_present")?,
        operstate: text("operstate")?,
        carrier: text("carrier")?,
        flag_up: boolean("flag_up")?,
        flag_running: boolean("flag_running")?,
        driver: text("driver")?,
        module: text("module")?,
        associated: boolean("associated")?,
        ipv4: text("ipv4")?,
        default_route: boolean("default_route")?,
        default_route_count: number("default_route_count")?,
        route_up: boolean("route_up")?,
        route_gateway: boolean("route_gateway")?,
    })
}

fn divergence(reference: &SummarySnapshot, current: &SummarySnapshot) -> Option<&'static str> {
    if !current.wlan_present {
        return Some("wlan0 disappeared");
    }
    if current.driver != reference.driver || current.module != reference.module {
        return Some("wlan0 driver/module ownership changed");
    }
    if reference.flag_up && !current.flag_up {
        return Some("wlan0 lost IFF_UP");
    }
    if reference.flag_running && !current.flag_running {
        return Some("wlan0 lost IFF_RUNNING");
    }
    if reference.carrier == "1" && current.carrier == "0" {
        return Some("wlan0 carrier dropped");
    }
    if reference.operstate == "up" && current.operstate != "up" {
        return Some("wlan0 operstate left up");
    }
    if current.supplicant_count > 1 {
        return Some("duplicate supplicant processes");
    }
    if current.dhcp_count > 1 {
        return Some("duplicate DHCP processes");
    }
    if current.nickel_count != 1 {
        return Some("Nickel process ownership changed unexpectedly");
    }
    if current.default_route_count > 1 {
        return Some("multiple wlan0 default routes appeared");
    }
    if current.supplicant_count == 0 {
        return Some("supplicant process disappeared");
    }
    if current.dhcp_count == 0 && current.default_route {
        return Some("stale default route remained after DHCP died");
    }
    if current.dhcp_count == 0 {
        return Some("DHCP process disappeared");
    }
    if reference.dbus_process_count > 0 && current.dbus_process_count == 0 {
        return Some("dhcpcd D-Bus adapter process disappeared");
    }
    if current.default_route && (!current.route_up || !current.route_gateway) {
        return Some("default route lost RTF_UP or RTF_GATEWAY");
    }
    if current.associated && current.ipv4 == "none" {
        return Some("association remained completed without an IPv4 address");
    }
    if reference.associated && !current.associated {
        return Some("wpa association stopped being completed");
    }
    if reference.ipv4 != "none" && current.ipv4 == "none" {
        return Some("IPv4 address disappeared");
    }
    if reference.supplicant_count == 1
        && current.supplicant_count == 1
        && generation_part(&reference.tuple, 'S') != generation_part(&current.tuple, 'S')
        && generation_part(&reference.tuple, 'D') == generation_part(&current.tuple, 'D')
        && generation_part(&reference.tuple, 'B') == generation_part(&current.tuple, 'B')
    {
        return Some("new supplicant appeared with old DHCP/D-Bus generations");
    }
    if current.wpa_socket != reference.wpa_socket {
        return Some("wpa control-socket inode changed");
    }
    if current.dbus_owner != reference.dbus_owner {
        return Some("dhcpcd D-Bus owner changed");
    }
    if reference.default_route && !current.default_route {
        return Some("default route was deleted after the initially healthy gate");
    }
    None
}

fn generation_part(tuple: &str, prefix: char) -> Option<&str> {
    tuple.split(';').find_map(|part| {
        part.strip_prefix(prefix)
            .filter(|rest| rest.starts_with(|character: char| character.is_ascii_digit()))
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    digest(&SHA256, bytes)
        .as_ref()
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            let _ignored = write!(output, "{byte:02x}");
            output
        })
}

fn read_bounded(path: &Path, maximum: u64) -> Option<Vec<u8>> {
    let mut file = File::open(path).ok()?;
    let mut content = Vec::new();
    Read::by_ref(&mut file)
        .take(maximum.saturating_add(1))
        .read_to_end(&mut content)
        .ok()?;
    (u64::try_from(content.len()).ok()? <= maximum).then_some(content)
}

fn monotonic_millis() -> u64 {
    let text = fs::read_to_string("/proc/uptime").unwrap_or_default();
    text.split_whitespace()
        .next()
        .and_then(|value| value.parse::<f64>().ok())
        .map_or(0, |seconds| {
            if seconds.is_sign_negative() || !seconds.is_finite() {
                0
            } else {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                {
                    (seconds * 1000.0) as u64
                }
            }
        })
}

fn elapsed_millis(started: Instant, now: Instant) -> u64 {
    u64::try_from(now.duration_since(started).as_millis()).unwrap_or(u64::MAX)
}

#[allow(clippy::cast_precision_loss)]
fn json_u64(value: u64) -> Value {
    Value::Number(value as f64)
}

#[allow(clippy::cast_precision_loss)]
fn json_i64(value: i64) -> Value {
    Value::Number(value as f64)
}

fn json_duration(value: Duration) -> Value {
    json_u64(u64::try_from(value.as_millis()).unwrap_or(u64::MAX))
}

fn json_usize(value: usize) -> Value {
    json_u64(u64::try_from(value).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn test_root(label: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/kobo-wifi-trace-tests")
            .join(format!(
                "{label}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(&root).expect("create fixture root");
        root
    }

    fn process(role: &'static str, pid: i32, starttime: u64, generation: u32) -> ProcessRecord {
        ProcessRecord {
            role,
            identity: ProcessIdentity { pid, starttime },
            ppid: 1,
            executable: format!("/sbin/{role}"),
            cmdline_sha256: sha256_hex(role.as_bytes()),
            generation,
        }
    }

    fn healthy(tuple: &str) -> Snapshot {
        Snapshot {
            processes: vec![
                process("nickel", 10, 100, 1),
                process("supplicant", 20, 200, 1),
                process("dhcp", 30, 300, 1),
                process("dhcpcd_dbus", 40, 400, 1),
            ],
            nickel_count: 1,
            supplicant_count: 1,
            dhcp_count: 1,
            dbus_process_count: 1,
            tuple: tuple.to_owned(),
            wpa_socket: "inode:1".to_owned(),
            dbus_owner: "owner:one".to_owned(),
            wlan_present: true,
            operstate: "up".to_owned(),
            carrier: "1".to_owned(),
            flag_up: true,
            flag_running: true,
            driver: "wlan_drv_gen4m".to_owned(),
            module: "wlan_drv_gen4m".to_owned(),
            wpa_state: "completed".to_owned(),
            associated: true,
            ipv4: "private_24".to_owned(),
            route: RouteState {
                default_present: true,
                default_count: 1,
                up: true,
                up_count: 1,
                gateway: true,
                gateway_count: 1,
                metric: "medium".to_owned(),
                prefix: "default".to_owned(),
            },
            resolver: "one".to_owned(),
            resolver_count: 1,
            resolver_search: false,
            dhcp_files: Vec::new(),
            probes: ProbeState::disabled(),
            watchdog: "armed".to_owned(),
            power_state: "freeze:false;standby:true;mem:true".to_owned(),
            wakeup_count: "10".to_owned(),
            suspend_success: "1".to_owned(),
            suspend_fail: "0".to_owned(),
            reboot_reason: "missing".to_owned(),
            pstore: "missing".to_owned(),
            last_kmsg: "missing".to_owned(),
        }
    }

    fn trace(snapshots: &[Snapshot]) -> Vec<u8> {
        let mut text = ObjectBuilder::new()
            .set("version", TRACE_VERSION)
            .set("mono_ms", 10_u32)
            .set("kind", "lifecycle")
            .set("event", "current_94_gate_accepted")
            .build()
            .to_json();
        text.push('\n');
        for (index, snapshot) in snapshots.iter().enumerate() {
            text.push_str(
                &snapshot_value(
                    20 + u64::try_from(index).unwrap_or(0) * 10,
                    "sample",
                    snapshot,
                )
                .to_json(),
            );
            text.push('\n');
        }
        text.into_bytes()
    }

    #[test]
    fn pid_reuse_is_a_new_generation_and_duplicates_are_visible() {
        let mut generations = Generations::default();
        assert_eq!(
            generations.number(
                "supplicant",
                &ProcessIdentity {
                    pid: 20,
                    starttime: 1
                }
            ),
            1
        );
        assert_eq!(
            generations.number(
                "supplicant",
                &ProcessIdentity {
                    pid: 20,
                    starttime: 1
                }
            ),
            1
        );
        assert_eq!(
            generations.number(
                "supplicant",
                &ProcessIdentity {
                    pid: 20,
                    starttime: 2
                }
            ),
            2
        );
        let mut duplicate = healthy("N1;S1+2;D1;B1;B=owner:one;C=inode:1");
        duplicate.supplicant_count = 2;
        assert!(summarize(&trace(&[
            healthy("N1;S1;D1;B1;B=owner:one;C=inode:1"),
            duplicate
        ]))
        .first_divergence
        .is_some_and(|value| value.contains("duplicate supplicant")));
    }

    #[test]
    fn ownership_divergences_are_classified() {
        let baseline = healthy("N1;S1;D1;B1;B=owner:one;C=inode:1");
        let cases = [
            ("stale default route", {
                let mut state = baseline.clone();
                state.dhcp_count = 0;
                state
            }),
            ("RTF_UP or RTF_GATEWAY", {
                let mut state = baseline.clone();
                state.route.up = false;
                state
            }),
            ("without an IPv4", {
                let mut state = baseline.clone();
                state.ipv4 = "none".to_owned();
                state
            }),
            ("new supplicant", {
                let mut state = baseline.clone();
                state.tuple = "N1;S2;D1;B1;B=owner:one;C=inode:1".to_owned();
                state
            }),
            ("control-socket inode", {
                let mut state = baseline.clone();
                state.wpa_socket = "inode:2".to_owned();
                state
            }),
            ("D-Bus owner", {
                let mut state = baseline.clone();
                state.dbus_owner = "owner:two".to_owned();
                state
            }),
            ("deleted after", {
                let mut state = baseline.clone();
                state.route.default_present = false;
                state.route.default_count = 0;
                state.route.up = false;
                state.route.up_count = 0;
                state.route.gateway = false;
                state.route.gateway_count = 0;
                state
            }),
            ("wlan0 disappeared", {
                let mut state = baseline.clone();
                state.wlan_present = false;
                state
            }),
        ];
        for (expected, changed) in cases {
            let summary = summarize(&trace(&[baseline.clone(), changed]));
            assert!(
                summary
                    .first_divergence
                    .as_deref()
                    .is_some_and(|value| value.contains(expected)),
                "{expected}: {summary:?}"
            );
        }
    }

    #[test]
    fn duplicate_dhcp_is_detected() {
        let baseline = healthy("N1;S1;D1;B1;B=owner:one;C=inode:1");
        let mut changed = baseline.clone();
        changed.dhcp_count = 2;
        let summary = summarize(&trace(&[baseline, changed]));
        assert!(summary
            .first_divergence
            .is_some_and(|value| value.contains("duplicate DHCP")));
    }

    #[test]
    fn process_stat_distinguishes_starttime() {
        let stat = "42 (wpa_supplicant) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 98765 20";
        assert_eq!(parse_process_stat(stat), Some((1, 98765)));
    }

    #[test]
    fn script_daemons_are_identified_by_argv_when_exe_is_an_interpreter() {
        assert_eq!(
            process_role(
                Some(Path::new("/bin/sh")),
                b"/usr/bin/dhcpcd-dbus\0--system\0"
            ),
            Some("dhcpcd_dbus")
        );
    }

    #[test]
    fn missing_optional_device_sources_are_represented() {
        let root = test_root("missing");
        fs::create_dir_all(root.join("proc/net")).expect("proc");
        fs::write(
            root.join("proc/net/route"),
            "Iface Destination Gateway Flags RefCnt Use Metric Mask\n",
        )
        .expect("route");
        let (snapshot, _) = Sampler::at(root.clone()).sample(Instant::now(), true);
        assert!(!snapshot.wlan_present);
        assert_eq!(snapshot.dbus_owner, "tool_missing");
        assert_eq!(snapshot.pstore, "missing");
        assert_eq!(snapshot.last_kmsg, "missing");
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn schedule_and_runtime_are_hard_bounded() {
        assert!((Duration::from_millis(100)..=Duration::from_millis(250)).contains(&RAPID_INTERVAL));
        assert!((Duration::from_secs(5)..=Duration::from_secs(10)).contains(&SOAK_INTERVAL));
        assert_eq!(SOAK_AFTER_NICKEL, Duration::from_secs(600));
        assert!(MAXIMUM_RUNTIME <= Duration::from_secs(15 * 60));
        assert!(INITIAL_RAPID_WINDOW < MAXIMUM_RUNTIME);
    }

    #[test]
    fn interrupted_last_write_is_retained_and_ignored() {
        let mut bytes = trace(&[healthy("N1;S1;D1;B1;B=owner:one;C=inode:1")]);
        bytes.extend_from_slice(br#"{"version":1,"kind":"snapshot""#);
        let summary = summarize(&bytes);
        assert_eq!(summary.ignored_lines, 1);
        assert!(summary.valid_lines >= 2);
    }

    #[test]
    fn trace_restart_appends_and_stale_cleanup_is_bounded() {
        let root = test_root("cleanup");
        for index in 0..7 {
            fs::write(
                root.join(format!("wifi-handoff-v1-{index}.jsonl")),
                b"old\n",
            )
            .expect("trace");
            thread::sleep(Duration::from_millis(2));
        }
        cleanup_stale_traces(&root, None).expect("cleanup traces");
        let count = fs::read_dir(&root).expect("read").count();
        assert_eq!(count, MAX_RETAINED_TRACES - 1);
        let path = root.join("wifi-handoff-v1-current.jsonl");
        TraceWriter::open(&path, false).expect("first writer");
        TraceWriter::open(&path, false).expect("restart writer");
        let text = fs::read_to_string(path).expect("read trace");
        assert!(text.contains("\"kind\":\"trace_start\""));
        assert!(text.contains("\"kind\":\"tracer_restart\""));
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn collection_omits_representative_owner_data() {
        let sensitive = [
            "Home WiFi",
            "aa:bb:cc:dd:ee:ff",
            "192.168.1.23",
            "192.168.1.1",
            "reader-owner.example",
            "secret-password",
            "123456789012345",
            "My Bluetooth Headphones",
        ];
        let snapshot = healthy("N1;S1;D1;B1;B=owner:one;C=inode:1");
        let exported = snapshot_value(1, "sample", &snapshot).to_json();
        for value in sensitive {
            assert!(!exported.contains(value), "{value:?} leaked");
        }
        assert_eq!(classify_cidr("192.168.1.23/24"), "private_24");
        assert!(
            !readable_kernel_event("wlan_drv_gen4m aa:bb:cc:dd:ee:ff 192.168.1.23")
                .contains("aa:bb")
        );
    }

    fn readable_kernel_event(line: &str) -> String {
        let event = classify_kernel_line(line).expect("kernel event");
        format!("{} {} {}", event.category, event.tags, event.digest)
    }

    #[test]
    fn command_surface_is_fixed_and_read_only() {
        let commands = readonly_command_specs(Some("192.168.1.1"));
        for command in &commands {
            let joined = command.arguments.join(" ").to_ascii_lowercase();
            for forbidden in [
                "reconnect",
                "reassociate",
                "disconnect",
                "scan",
                "set_network",
                "add_network",
                "remove_network",
                "link set",
                "ifconfig",
                "kill",
                "signal",
                "/proc/wmt",
                "dhcpcd -",
            ] {
                assert!(
                    !joined.contains(forbidden),
                    "{} exposes forbidden operation {forbidden}: {joined}",
                    command.purpose
                );
            }
            if command.executable == "dbus-send" {
                assert!(joined.contains("org.freedesktop.dbus.getnameowner"));
                assert!(!joined.contains("name.marples.roy.dhcpcd."));
            }
        }
        let wpa = commands
            .iter()
            .find(|command| command.purpose == "wpa_status")
            .expect("wpa");
        assert_eq!(wpa.arguments, ["-i", "wlan0", "status"]);
    }

    #[test]
    fn disabled_client_has_zero_trace_io_surface() {
        let mut client = TraceClient::disabled();
        assert!(!client.enabled());
        assert!(client.trace_path().is_none());
        client.checkpoint(Lifecycle::PreStop);
        assert!(!trace_unlocked(None));
        assert!(!trace_unlocked(Some("1")));
        assert!(trace_unlocked(Some(ENABLE_PHRASE)));
    }

    #[test]
    fn route_semantics_do_not_export_addresses() {
        let (route, gateway) = parse_routes(
            "Iface Destination Gateway Flags RefCnt Use Metric Mask\n\
             wlan0 00000000 0101A8C0 0003 0 0 312 00000000\n",
        );
        assert!(route.default_present);
        assert!(route.up);
        assert!(route.gateway);
        assert_eq!(route.metric, "medium");
        assert_eq!(route.default_count, 1);
        let shown = format!("{route:?}");
        assert!(!shown.contains("192.168"));
        assert_eq!(gateway.as_deref(), Some("192.168.1.1"));
    }

    #[test]
    fn every_default_route_contributes_semantics() {
        let (route, _) = parse_routes(
            "Iface Destination Gateway Flags RefCnt Use Metric Mask\n\
             wlan0 00000000 0101A8C0 0003 0 0 100 00000000\n\
             wlan0 00000000 00000000 0001 0 0 600 00000000\n",
        );
        assert_eq!(route.default_count, 2);
        assert_eq!(route.up_count, 2);
        assert_eq!(route.gateway_count, 1);
        assert!(!route.gateway);
        assert_eq!(route.metric, "high+low");
    }
}

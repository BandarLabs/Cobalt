use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

mod connect;
mod devsession;
mod package;
mod sha256;

const DEVICE_PACKAGES: &[&str] = &["kobo-doctor", "kobod", "kobo-todo", "kobo-terminal"];
/// Everything an owner's device needs, in the order it is packaged, with the
/// features each one has to be built with.
///
/// The launcher is first because it is what `kobod` is pointed at, and the
/// rest are what it can start. `kobo-doctor`, `kobo-smoke`, `kobo-handoff` and
/// `kobo-guard` are deliberately absent: they are development tools, and two
/// of them write to hardware.
///
/// `kobod` needs `device-write` or `--present` is not compiled in at all, and
/// `start.sh` — the only thing in the package an owner runs — fails with a
/// usage message. That is exactly what shipped until an installed package was
/// run on a real device, so `every_packaged_binary_is_built_with_what_it_needs`
/// and the artifact check in `build_package` both exist to keep it shipped.
const INSTALLED_PACKAGES: &[(&str, Option<&str>)] = &[
    ("kobod", Some("device-write")),
    ("kobo-launcher", None),
    ("kobo-terminal", None),
    ("kobo-todo", None),
    ("kobo-brief", None),
    ("kobo-chat", None),
    ("kobo-gutenshelf", None),
    ("kobo-gallery", None),
    ("kobo-tictactoe", None),
    ("kobo-hn", None),
];
/// Proof that the daemon in the package can actually take the panel. The
/// phrase only exists inside `present_on_panel`, which is behind
/// `device-write`, so finding it in the finished binary is the artifact-level
/// version of running `start.sh`.
const PRESENT_UNLOCK_PHRASE: &[u8] = b"OWNER_ATTENDED_PANEL_SESSION";
/// What the owner runs, and the only thing that starts a panel session.
///
/// It sets the unlock the daemon requires, because on an installed device the
/// owner tapping a menu entry *is* the attendance that gate was asking for.
/// The session hands the panel back on every exit path, and a reboot always
/// lands in the stock reader, so the worst case remains a power cycle.
const START_SCRIPT: &str = "\
#!/bin/sh
# Starts Cobalt. The stock reader is stopped, the panel is handed over, and the
# reader is started again when the session ends. A reboot always returns to the
# stock reader, so nothing here needs undoing by hand.
set -e
root=/mnt/onboard/.adds/cobalt
KOBO_PRESENT_UNLOCK=OWNER_ATTENDED_PANEL_SESSION \\
  exec \"$root/bin/kobod\" --present \"$root/bin/kobo-launcher\"
";

/// Shipped inside the package, because the thing an owner most needs to find
/// is how to get rid of it.
const INSTALL_README: &str = "\
Cobalt
======

Everything is in this folder, on the same partition your books are on. It is
visible from any computer over USB.

To remove it completely: delete this folder. Nothing was written to the
system partition, no start-up script was added, and no part of the reader was
replaced, so there is nothing else to undo.

To start it: run start.sh. If you have NickelMenu installed, add this one line
to .adds/nm/menu to get an entry in the reader's own menu:

  menu_item :main    :Cobalt    :cmd_spawn    :quiet:/mnt/onboard/.adds/cobalt/start.sh

Starting Cobalt stops the stock reader for the length of the session and
starts it again afterwards. That takes twenty to thirty seconds each way. A
reboot always returns you to the stock reader.
";

/// Printed after a package is built, and the same words the project's own
/// instructions use.
const INSTALL_INSTRUCTIONS: &str = "\
To install on a device:
  1. Charge it. The reader refuses to install anything on a low battery, and
     it does so silently.
  2. Connect it by USB and copy this file to .kobo/KoboRoot.tgz on the drive
     that appears.
  3. Eject the drive. The device installs it at the next boot and restarts.

Everything lands in .adds/cobalt on the same drive. Deleting that folder is a
complete uninstall; nothing is written to the system partition.";

const REMOTE_CONNECT_TIMEOUT_SECONDS: u64 = 10;
const REMOTE_COMMAND_TIMEOUT: Duration = Duration::from_secs(60);
const REMOTE_CLEANUP_TIMEOUT: Duration = Duration::from_secs(5);
/// Session commands search the reader libraries, which are large, so they need
/// more room than a cleanup command.
const REMOTE_SESSION_TIMEOUT: Duration = Duration::from_secs(45);
/// How long each answering address in a sweep is given to identify itself.
///
/// Reading four small files takes no time at all; the whole budget is the SSH
/// handshake, and something on the network that is not a device has to be
/// given up on quickly or one stranger's host holds up the whole listing.
const DEVICE_IDENTITY_TIMEOUT: Duration = Duration::from_secs(15);
/// How long an install over Wi-Fi is given.
///
/// The package is around six and a half megabytes of base64 through a single
/// stdin pipe, which measured about ten seconds on this device, and the
/// extraction and `sync` afterwards are unhurried on vfat. This is generous by
/// an order of magnitude on purpose: a deploy killed halfway is the one thing
/// here that could leave a half-written install directory.
const DEPLOY_TIMEOUT: Duration = Duration::from_secs(300);
/// How long a single reachability probe is given before it counts as a miss.
const DEVICE_PROBE_TIMEOUT: Duration = Duration::from_secs(6);
/// Gap between reachability probes while waiting for a device.
const DEVICE_PROBE_INTERVAL: Duration = Duration::from_secs(5);
/// Longest wait `kobo wait` will accept, so it can never block forever.
const DEVICE_WAIT_MAXIMUM_SECONDS: u64 = 6 * 60 * 60;
/// How often a held wake lock is re-applied.
///
/// Measured on the device, the lock is cleared somewhere between two and three
/// minutes after it is taken, so renewal has to be well inside that.
const WAKE_LOCK_RENEW_INTERVAL: Duration = Duration::from_secs(30);
/// Longest a hold may last, so a forgotten session always ends by itself.
const HOLD_MAXIMUM_MINUTES: u64 = 8 * 60;
/// Longest sleep delay this tool will write.
///
/// A device that never sleeps flattens its battery, so the delay is bounded and
/// `--sleep-after default` always puts the reader back on its own default.
const SLEEP_AFTER_MAXIMUM_MINUTES: u64 = 4 * 60;
#[cfg(feature = "device-write")]
const REMOTE_SMOKE_TIMEOUT_SECONDS: u64 = 25;
/// Default and maximum touch observation windows, in seconds.
const TOUCH_PROBE_DEFAULT_SECONDS: u64 = 20;
const TOUCH_PROBE_MAXIMUM_SECONDS: u64 = 120;
/// Slack added to the observation window for build, upload, probe and cleanup.
const TOUCH_PROBE_OVERHEAD: Duration = Duration::from_secs(60);
/// The guard test damages a region, supervises a child that fails immediately,
/// and restores. The child is a stock `BusyBox` applet at an exact absolute path.
#[cfg(feature = "device-write")]
const GUARD_TEST_CHILD: &str = "/bin/false";
#[cfg(feature = "device-write")]
const GUARD_TEST_TIMEOUT_SECONDS: u64 = 10;
#[cfg(feature = "device-write")]
const GUARD_TEST_CONFIRMATION: &str = "GUARD_RESTORE_AFTER_FAILURE";

/// The owner-attended smoke stages, selected by an exact confirmation phrase.
///
/// Each stage maps to exactly one `KOBO_SMOKE_UNLOCK` value on the device, so
/// no free-form value ever reaches the device binary.
#[cfg(feature = "device-write")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SmokeStage {
    DisplayOnly,
    ReversiblePixels,
    ScreenSnapshot,
    FastFeedback,
}

#[cfg(feature = "device-write")]
impl SmokeStage {
    const CONFIRM_DISPLAY_ONLY: &'static str = "DISPLAY_ONLY_GC16";
    const CONFIRM_REVERSIBLE_PIXELS: &'static str = "REVERSIBLE_PIXELS_GC16";
    const CONFIRM_SCREEN_SNAPSHOT: &'static str = "SCREEN_SNAPSHOT_RESTORE";
    const CONFIRM_FAST_FEEDBACK: &'static str = "REVERSIBLE_PIXELS_DU";

    /// Every stage, so the usage text can never drift from what is accepted.
    const ALL: [Self; 4] = [
        Self::DisplayOnly,
        Self::ReversiblePixels,
        Self::ScreenSnapshot,
        Self::FastFeedback,
    ];

    const fn confirmation(self) -> &'static str {
        match self {
            Self::DisplayOnly => Self::CONFIRM_DISPLAY_ONLY,
            Self::ReversiblePixels => Self::CONFIRM_REVERSIBLE_PIXELS,
            Self::ScreenSnapshot => Self::CONFIRM_SCREEN_SNAPSHOT,
            Self::FastFeedback => Self::CONFIRM_FAST_FEEDBACK,
        }
    }

    fn confirmation_list() -> String {
        Self::ALL
            .iter()
            .map(|stage| stage.confirmation())
            .collect::<Vec<_>>()
            .join("|")
    }

    fn from_confirmation(value: &str) -> Option<Self> {
        match value {
            Self::CONFIRM_DISPLAY_ONLY => Some(Self::DisplayOnly),
            Self::CONFIRM_REVERSIBLE_PIXELS => Some(Self::ReversiblePixels),
            Self::CONFIRM_SCREEN_SNAPSHOT => Some(Self::ScreenSnapshot),
            Self::CONFIRM_FAST_FEEDBACK => Some(Self::FastFeedback),
            _ => None,
        }
    }

    fn device_unlock(self) -> &'static str {
        match self {
            Self::DisplayOnly => "OWNER_ATTENDED_DISPLAY_ONLY_GC16",
            Self::ReversiblePixels => "OWNER_ATTENDED_REVERSIBLE_PIXELS_GC16",
            Self::ScreenSnapshot => "OWNER_ATTENDED_SCREEN_SNAPSHOT_RESTORE",
            Self::FastFeedback => "OWNER_ATTENDED_REVERSIBLE_PIXELS_DU",
        }
    }
}

fn main() -> ExitCode {
    let arguments = env::args().skip(1).collect::<Vec<_>>();
    match run(&arguments) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("kobo: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(arguments: &[String]) -> Result<(), String> {
    let Some(command) = arguments.first().map(String::as_str) else {
        print_help();
        return Ok(());
    };
    match command {
        "new" => create_app(arguments.get(1).ok_or("usage: kobo new <name>")?),
        "dev" => dev(&arguments[1..]),
        "build" => build_device(arguments.iter().any(|argument| argument == "--device")),
        "doctor" => doctor(&arguments[1..]),
        "devices" => list_devices(&arguments[1..]),
        "session" => dev_session(&arguments[1..]),
        "wait" => wait_for_device(&arguments[1..]),
        "touch-probe" => touch_probe(&arguments[1..]),
        #[cfg(feature = "device-write")]
        "smoke-display" => smoke_display(&arguments[1..]),
        #[cfg(feature = "device-write")]
        "guard-test" => guard_test(&arguments[1..]),
        #[cfg(not(feature = "device-write"))]
        "guard-test" => Err(
            "guard-test is not compiled in; rebuild the CLI with --features device-write"
                .to_owned(),
        ),
        #[cfg(not(feature = "device-write"))]
        "smoke-display" => Err(
            "smoke-display is not compiled in; rebuild the CLI with --features device-write"
                .to_owned(),
        ),
        "package" => build_package(&arguments[1..]),
        "deploy" => deploy_package(&arguments[1..]),
        "inspect" => inspect_package(&arguments[1..]),
        "verify" => verify_command(&arguments[1..]),
        "run" if arguments.get(1).is_some_and(|value| value == "--sim") => run_simulation(),
        "run" => {
            Err("device execution is safety-gated; use 'kobo run --sim' on the host".to_owned())
        }
        "help" | "--help" | "-h" => {
            print_help();
            Ok(())
        }
        "version" | "--version" | "-V" => {
            println!("kobo 0.1.0");
            Ok(())
        }
        unknown => Err(format!("unknown command '{unknown}'")),
    }
}

fn create_app(name: &str) -> Result<(), String> {
    if !valid_slug(name) {
        return Err("app name must contain only lowercase letters, digits, and hyphens".to_owned());
    }
    let root = PathBuf::from(name);
    if root.exists() {
        return Err(format!("{} already exists", root.display()));
    }
    let sdk = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../kobo-sdk")
        .canonicalize()
        .map_err(|error| format!("locate local SDK: {error}"))?;
    let sdk = sdk
        .to_str()
        .ok_or("local SDK path is not valid UTF-8")?
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    fs::create_dir_all(root.join("src")).map_err(|error| error.to_string())?;
    fs::write(
        root.join("Cargo.toml"),
        format!(
            "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
             [dependencies]\nkobo-sdk = {{ path = \"{sdk}\" }}\n\n[workspace]\n"
        ),
    )
    .map_err(|error| error.to_string())?;
    fs::write(root.join("src/main.rs"), GENERATED_APP_SOURCE).map_err(|error| error.to_string())?;
    println!("created {}", root.display());
    println!("next: cd {name} && kobo dev");
    Ok(())
}

const GENERATED_APP_SOURCE: &str = r#"use kobo_sdk::prelude::*;
use std::env;
use std::process::ExitCode;

#[derive(Default)]
struct Hello {
    battery: Option<String>,
}

impl Hello {
    fn show(&self, context: &mut Context) {
        context.set_screen(
            ScreenBuilder::new("hello")
                .heading("Hello, Kobo")
                .text("Built with the Kobo SDK.")
                .text(
                    self.battery
                        .clone()
                        .unwrap_or_else(|| "Battery: asking...".into()),
                )
                .button("refresh", "Refresh")
                .button("close", "Close")
                .build(),
        );
    }
}

impl KoboApp for Hello {
    fn on_start(&mut self, context: &mut Context) {
        // Hardware is asked for, never touched directly. Every request gets
        // exactly one answer in on_device_result, including a refusal.
        context.device().read_battery();
        self.show(context);
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if action == action_id("close") {
            context.exit();
        } else if action == action_id("refresh") {
            context.device().read_battery();
        }
    }

    fn on_device_result(
        &mut self,
        context: &mut Context,
        request: DeviceRequest,
        result: DeviceResult,
    ) {
        if request != DeviceRequest::ReadBattery {
            return;
        }
        self.battery = Some(match result {
            DeviceResult::Battery { percent, charging } => {
                let state = if charging { ", charging" } else { "" };
                format!("Battery: {percent}%{state}")
            }
            DeviceResult::Denied(reason) => format!("Battery unavailable: {reason}"),
            _ => "Battery: unexpected answer".into(),
        });
        self.show(context);
    }
}

fn main() -> ExitCode {
    let mut runner = AppRunner::new(Hello::default());
    let initial_commands = runner.start();
    let Some(socket) = env::var_os("KOBO_SOCKET") else {
        println!("Kobo screen initialized; set KOBO_SOCKET to run in the simulator.");
        return ExitCode::SUCCESS;
    };
    let mut client = match Client::connect(socket, env!("CARGO_PKG_NAME")) {
        Ok(client) => client,
        Err(error) => {
            eprintln!("app: {error}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(error) = client.send_commands(initial_commands) {
        eprintln!("app: {error}");
        return ExitCode::FAILURE;
    }
    loop {
        match client.next_event() {
            Ok(ClientEvent::Action(action)) => {
                let commands = runner.action(action);
                let exiting = commands.iter().any(|command| matches!(command, Command::Exit));
                if let Err(error) = client.send_commands(commands) {
                    eprintln!("app: {error}");
                    return ExitCode::FAILURE;
                }
                if exiting {
                    return ExitCode::SUCCESS;
                }
            }
            Ok(ClientEvent::Device(result)) => {
                if let Err(error) = client.send_commands(runner.device_result(result)) {
                    eprintln!("app: {error}");
                    return ExitCode::FAILURE;
                }
            }
            Ok(ClientEvent::Exit) => {
                let _ = client.send_commands(runner.exit());
                return ExitCode::SUCCESS;
            }
            Err(error) => {
                eprintln!("app: {error}");
                return ExitCode::FAILURE;
            }
        }
    }
}
"#;

fn dev(arguments: &[String]) -> Result<(), String> {
    let (built_in, address) = match arguments {
        [] => (false, "127.0.0.1:8787"),
        [address] if address == "--builtin" => (true, "127.0.0.1:8787"),
        [address] => (false, address.as_str()),
        [flag, address] if flag == "--builtin" => (true, address.as_str()),
        _ => return Err("usage: kobo dev [--builtin] [address]".to_owned()),
    };
    if built_in || !current_manifest_uses_sdk()? {
        return kobo_sim::run_server(address).map_err(|error| error.to_string());
    }
    dev_sdk_app(address)
}

fn current_manifest_uses_sdk() -> Result<bool, String> {
    let manifest = Path::new("Cargo.toml");
    match fs::read_to_string(manifest) {
        Ok(contents) => Ok(manifest_uses_sdk(&contents)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("read {}: {error}", manifest.display())),
    }
}

fn manifest_uses_sdk(manifest: &str) -> bool {
    manifest.lines().any(|line| {
        let line = line.trim_start();
        !line.starts_with('#') && line.starts_with("kobo-sdk")
    })
}

fn dev_sdk_app(address: &str) -> Result<(), String> {
    let dev_session = DevSessionGuard::new()?;
    let server = kobo_sim::AppServer::bind(address, &dev_session.socket)
        .map_err(|error| format!("start app simulator: {error}"))?;
    server
        .set_nonblocking(true)
        .map_err(|error| format!("configure app simulator: {error}"))?;
    let executable = build_dev_app()?;
    let mut app = AppChild::spawn(&executable, &dev_session.socket)?;
    let session = wait_for_app(&server, &mut app)?;
    println!(
        "Kobo app simulator: http://{}",
        server
            .local_addr()
            .map_err(|error| format!("read simulator address: {error}"))?
    );
    serve_app(&server, &session, &mut app)
}

struct DevSessionGuard {
    root: PathBuf,
    socket: PathBuf,
}

impl DevSessionGuard {
    fn new() -> Result<Self, String> {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        Self::new_at(env::temp_dir().join(format!("kobo-dev-{}-{unique}", std::process::id())))
    }

    fn new_at(root: PathBuf) -> Result<Self, String> {
        fs::create_dir(&root).map_err(|error| format!("create {}: {error}", root.display()))?;
        let session = Self {
            socket: root.join("app.sock"),
            root,
        };
        if let Err(error) = fs::set_permissions(&session.root, fs::Permissions::from_mode(0o700)) {
            let message = format!("protect {}: {error}", session.root.display());
            drop(session);
            return Err(message);
        }
        Ok(session)
    }
}

impl Drop for DevSessionGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.socket);
        let _ = fs::remove_dir(&self.root);
    }
}

fn build_dev_app() -> Result<PathBuf, String> {
    let output = Command::new("cargo")
        .args(["build", "--message-format=json"])
        .output()
        .map_err(|error| format!("build application: {error}"))?;
    if !output.status.success() {
        eprint!("{}", String::from_utf8_lossy(&output.stdout));
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
        return Err(format!("cargo build exited with {}", output.status));
    }
    let executables = build_executables(&String::from_utf8_lossy(&output.stdout));
    match executables.as_slice() {
        [executable] => Ok(executable.clone()),
        [] => Err("cargo build did not produce an application binary".to_owned()),
        _ => Err(
            "cargo build produced multiple application binaries; run `kobo dev` from a package with one binary"
                .to_owned(),
        ),
    }
}

fn build_executables(output: &str) -> Vec<PathBuf> {
    output
        .lines()
        .filter(|line| {
            line.contains(r#""reason":"compiler-artifact""#) && line.contains(r#""kind":["bin"]"#)
        })
        .filter_map(|line| json_string_field(line, "executable"))
        .map(PathBuf::from)
        .collect()
}

fn json_string_field(line: &str, field: &str) -> Option<String> {
    let field = format!("\"{field}\"");
    let value = &line[line.find(&field)? + field.len()..];
    let value = value.strip_prefix(':')?.trim_start();
    let value = value.strip_prefix('"')?;
    let mut result = String::new();
    let mut characters = value.chars();
    while let Some(character) = characters.next() {
        match character {
            '"' => return Some(result),
            '\\' => match characters.next()? {
                '"' => result.push('"'),
                '\\' => result.push('\\'),
                '/' => result.push('/'),
                'b' => result.push('\u{0008}'),
                'f' => result.push('\u{000c}'),
                'n' => result.push('\n'),
                'r' => result.push('\r'),
                't' => result.push('\t'),
                'u' => {
                    let code = characters.by_ref().take(4).collect::<String>();
                    result.push(char::from_u32(u32::from_str_radix(&code, 16).ok()?)?);
                }
                _ => return None,
            },
            character => result.push(character),
        }
    }
    None
}

struct AppChild {
    child: Option<Child>,
}

impl AppChild {
    fn spawn(executable: &Path, socket: &Path) -> Result<Self, String> {
        let child = Command::new(executable)
            .env("KOBO_SOCKET", socket)
            .spawn()
            .map_err(|error| format!("launch {}: {error}", executable.display()))?;
        Ok(Self { child: Some(child) })
    }

    fn try_wait(&mut self) -> Result<Option<ExitStatus>, String> {
        self.child.as_mut().map_or(Ok(None), |child| {
            child
                .try_wait()
                .map_err(|error| format!("inspect application: {error}"))
        })
    }
}

impl Drop for AppChild {
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        if child.try_wait().ok().flatten().is_none() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn wait_for_app(
    server: &kobo_sim::AppServer,
    app: &mut AppChild,
) -> Result<kobo_sim::AppSession, String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(session) = server
            .try_accept_app()
            .map_err(|error| format!("accept application: {error}"))?
        {
            return Ok(session);
        }
        if let Some(status) = app.try_wait()? {
            return Err(format!("application exited before connecting: {status}"));
        }
        if Instant::now() >= deadline {
            return Err(
                "application did not connect to the simulator within 10 seconds".to_owned(),
            );
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn serve_app(
    server: &kobo_sim::AppServer,
    session: &kobo_sim::AppSession,
    app: &mut AppChild,
) -> Result<(), String> {
    loop {
        server
            .try_serve_one(session)
            .map_err(|error| format!("serve browser request: {error}"))?;
        if let Some(status) = app.try_wait()? {
            return if status.success() {
                Ok(())
            } else {
                Err(format!("application exited with {status}"))
            };
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn valid_slug(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && !value.ends_with('-')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn build_device(device: bool) -> Result<(), String> {
    let mut command = Command::new("cargo");
    command.args(["build", "--release"]);
    if device {
        let linker = find_rust_lld()?;
        command.env("CARGO_TARGET_ARMV7_UNKNOWN_LINUX_MUSLEABIHF_LINKER", linker);
        command.args(["--target", "armv7-unknown-linux-musleabihf"]);
        for package in DEVICE_PACKAGES {
            command.args(["-p", package]);
        }
    }
    run_status(&mut command, "cargo build")?;
    if device {
        for name in DEVICE_PACKAGES {
            let binary = Path::new("target/armv7-unknown-linux-musleabihf/release").join(name);
            verify_arm_elf(&binary)?;
            println!(
                "verified static ARMv7 hard-float binary: {}",
                binary.display()
            );
        }
    }
    Ok(())
}

fn doctor(arguments: &[String]) -> Result<(), String> {
    if let Some(position) = arguments.iter().position(|argument| argument == "--device") {
        let host = arguments
            .get(position + 1)
            .ok_or("usage: kobo doctor --device <host>")?;
        return remote_doctor(host);
    }
    let binary = sibling_binary("kobo-doctor");
    let mut command = Command::new(&binary);
    run_status(&mut command, format!("{}", binary.display()))
}

/// Watches the touch panel read-only so the profile's touch transform can be
/// checked against a physical touch at a known place on the screen.
///
/// Nothing is written and the panel is never grabbed, so the stock reader keeps
/// receiving every touch and the screen is untouched.
fn touch_probe(arguments: &[String]) -> Result<(), String> {
    let (host, seconds) = parse_touch_probe(arguments)?;
    println!("touch probe: watching {host} read-only for {seconds}s");
    println!("touch the screen at a corner you can describe, then wait");
    run_remote_fixed_artifact(host, &RemoteArtifact::touch_probe(seconds))
}

fn parse_touch_probe(arguments: &[String]) -> Result<(&str, u64), String> {
    let (host, seconds) = match arguments {
        [device, host] if device == "--device" => (host, TOUCH_PROBE_DEFAULT_SECONDS),
        [device, host, flag, value] if device == "--device" && flag == "--seconds" => {
            let seconds = value
                .parse::<u64>()
                .map_err(|_| "--seconds must be a whole number".to_owned())?;
            (host, seconds)
        }
        _ => return Err("usage: kobo touch-probe --device <host> [--seconds <1-120>]".to_owned()),
    };
    if seconds == 0 || seconds > TOUCH_PROBE_MAXIMUM_SECONDS {
        return Err(format!(
            "--seconds must be between 1 and {TOUCH_PROBE_MAXIMUM_SECONDS}"
        ));
    }
    if !valid_device_host(host) {
        return Err("device host contains unsupported characters".to_owned());
    }
    Ok((host, seconds))
}

fn remote_doctor(host: &str) -> Result<(), String> {
    if !valid_device_host(host) {
        return Err("device host contains unsupported characters".to_owned());
    }
    run_remote_fixed_artifact(host, &RemoteArtifact::doctor())
}

/// Names every Kobo on the local network, because its address changed again.
///
/// A reader takes a new address from DHCP every time its radio comes back, so
/// the address that worked an hour ago is a guess. This is the answer to "what
/// is it now", and it is deliberately the one command here that needs no
/// argument at all.
///
/// It knocks on port 22, opens a shell on whatever answered, and reads four
/// files. Everything it does is read-only, and hosts that are not readers are
/// counted rather than listed: a tool that prints an inventory of somebody's
/// home network when they asked where their e-reader went has answered a
/// question nobody asked.
fn list_devices(arguments: &[String]) -> Result<(), String> {
    let subnet = parse_devices(arguments)?;
    println!(
        "scanning {subnet}.1-254 on port {} for readers",
        connect::SSH_PORT
    );
    let answered = connect::sweep(&subnet, connect::PROBE_TIMEOUT);
    let mut readers = Vec::new();
    let mut others = 0_usize;
    for address in &answered {
        match identify_device(&address.to_string()) {
            Some(identity) if identity.is_kobo() => {
                println!("{address}  {}", identity.summary());
                readers.push(*address);
            }
            _ => others += 1,
        }
    }
    if others > 0 {
        println!(
            "{others} other host(s) answered on port {}",
            connect::SSH_PORT
        );
    }
    let Some(first) = readers.first() else {
        return Err(unreachable_device(format!(
            "no reader answered on {subnet}.0/24"
        )));
    };
    println!("use it with --device, for example: kobo doctor --device {first}");
    Ok(())
}

/// Reads a host's identity, or `None` when it is not something we can talk to.
///
/// An address that completes a TCP handshake proves only that something is
/// listening. No key, a different SSH server, or a machine that is simply not
/// ours all fail here, and every one of them is an ordinary result on a home
/// network rather than a reason to abandon the sweep.
fn identify_device(host: &str) -> Option<connect::Identity> {
    if !valid_device_host(host) {
        return None;
    }
    let output = run_remote_shell(
        &format!("root@{host}"),
        &connect::identity_script(),
        DEVICE_IDENTITY_TIMEOUT,
    )
    .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(connect::Identity::parse(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

fn parse_devices(arguments: &[String]) -> Result<String, String> {
    const USAGE: &str = "usage: kobo devices [--subnet A.B.C]";
    let subnet = match arguments {
        [] => connect::local_subnet().ok_or(
            "this machine has no route to a network, so there is nothing to scan; \
             connect to the same Wi-Fi as the reader, or pass --subnet A.B.C",
        )?,
        [flag, value] if flag == "--subnet" => (*value).clone(),
        _ => return Err(USAGE.to_owned()),
    };
    if !connect::valid_subnet(&subnet) {
        return Err(format!(
            "--subnet takes the first three octets and nothing else, such as 192.168.1, \
             not {subnet:?}"
        ));
    }
    Ok(subnet)
}

/// Controls how long a connected device stays reachable while developing.
///
/// Every action is reversible and none of them touch a partition, the
/// bootloader, the kernel, firmware, or any book.
fn dev_session(arguments: &[String]) -> Result<(), String> {
    let (host, action) = parse_dev_session(arguments)?;
    if let DevSessionAction::Hold(minutes) = action {
        hold_device_awake(host, minutes);
        return Ok(());
    }
    let script = match action {
        DevSessionAction::Status => devsession::status_script(),
        DevSessionAction::KeepAwake(switch) => devsession::wake_lock_script(switch),
        DevSessionAction::WifiAlwaysOn(switch) => {
            devsession::setting_script(&devsession::Setting::force_wifi_on(), switch)
        }
        DevSessionAction::SleepAfter(minutes) => devsession::setting_script(
            &devsession::Setting::auto_sleep_minutes(minutes),
            devsession::Switch::On,
        ),
        DevSessionAction::RestoreSleepDefault => devsession::setting_script(
            &devsession::Setting::auto_sleep_minutes(0),
            devsession::Switch::Off,
        ),
        DevSessionAction::RestoreConfig => devsession::restore_config_script(),
        DevSessionAction::Hold(_) => unreachable!("hold is handled above"),
    };
    let output = run_remote_shell(&format!("root@{host}"), &script, REMOTE_SESSION_TIMEOUT)
        .map_err(unreachable_device)?;
    print!("{}", String::from_utf8_lossy(&output.stdout));
    if output.status.success() {
        // Advising a restart is only true when something actually changed; the
        // reader already holds the intended value otherwise.
        let changes_a_setting = matches!(
            action,
            DevSessionAction::WifiAlwaysOn(_)
                | DevSessionAction::SleepAfter(_)
                | DevSessionAction::RestoreSleepDefault
        );
        if changes_a_setting && changed_lines(&output.stdout) > 0 {
            println!(
                "the reader reads this file only at startup, so restart the reader or \
                 reboot the device for this setting to take effect"
            );
        }
        Ok(())
    } else {
        Err(unreachable_if_ssh_gave_up(
            remote_session_failure(
                format!("device session command exited with {}", output.status),
                &output,
                None,
            ),
            &output,
        ))
    }
}

/// Keeps a device awake and reachable for a bounded time by renewing the
/// developer wake lock, so testing does not need someone tapping the screen.
///
/// The lock is RAM-only kernel state. It is released when the hold ends, and a
/// reboot clears it regardless, so this can never leave a device unable to
/// sleep. A device that disappears mid-hold is waited for rather than treated
/// as a failure.
fn hold_device_awake(host: &str, minutes: u64) {
    let remote = format!("root@{host}");
    let budget = Duration::from_secs(minutes * 60);
    let started = Instant::now();
    println!("holding {host} awake for {minutes} minute(s); press Ctrl-C to stop early");
    let mut renewals: u64 = 0;
    let mut reacquired: u64 = 0;
    let mut lost_contact: u64 = 0;
    while started.elapsed() < budget {
        match run_remote_shell(
            &remote,
            &devsession::wake_lock_renew_script(),
            DEVICE_PROBE_TIMEOUT,
        ) {
            Ok(output) if output.status.success() => {
                renewals += 1;
                if String::from_utf8_lossy(&output.stdout).contains("reacquired") {
                    reacquired += 1;
                    println!(
                        "{}s: wake lock had been cleared and was reacquired",
                        started.elapsed().as_secs()
                    );
                }
            }
            _ => {
                lost_contact += 1;
                println!(
                    "{}s: device not answering; waiting for it to come back",
                    started.elapsed().as_secs()
                );
            }
        }
        thread::sleep(WAKE_LOCK_RENEW_INTERVAL);
    }
    // Releasing is best effort: an unreachable device clears the lock on its
    // next reboot anyway, so a failure here cannot leave lasting state.
    let released = run_remote_shell(
        &remote,
        &devsession::wake_lock_script(devsession::Switch::Off),
        DEVICE_PROBE_TIMEOUT,
    )
    .is_ok_and(|output| output.status.success());
    println!(
        "hold finished: {renewals} renewal(s), {reacquired} reacquisition(s), \
         {lost_contact} missed probe(s), wake lock released: {released}"
    );
    if !released {
        println!("the wake lock is RAM only and clears on the next reboot");
    }
}

/// Returns the number of settings lines the device reported changing.
///
/// An unreadable or absent count is treated as no change, so this can only ever
/// suppress advice, never invent it.
fn changed_lines(stdout: &[u8]) -> u32 {
    String::from_utf8_lossy(stdout)
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("applied; changed_lines=")?
                .parse()
                .ok()
        })
        .unwrap_or(0)
}

/// Blocks until a device answers, so a workflow survives the reader dropping
/// Wi-Fi on its own inactivity timer.
///
/// This only opens and closes a shell session. It reads nothing, writes
/// nothing, and leaves no file behind, so waiting is always safe to run.
fn wait_for_device(arguments: &[String]) -> Result<(), String> {
    let (host, budget) = parse_wait(arguments)?;
    let remote = format!("root@{host}");
    let started = Instant::now();
    let mut attempts: u64 = 0;
    loop {
        attempts += 1;
        if device_answers(&remote) {
            println!(
                "device {host} reachable after {}s and {attempts} probe(s)",
                started.elapsed().as_secs()
            );
            return Ok(());
        }
        let waited = started.elapsed();
        if waited + DEVICE_PROBE_INTERVAL >= budget {
            return Err(unreachable_device(format!(
                "device {host} did not answer within {}s; wake it and try again",
                budget.as_secs()
            )));
        }
        if attempts == 1 {
            println!(
                "waiting up to {}s for {host}; probing every {}s",
                budget.as_secs(),
                DEVICE_PROBE_INTERVAL.as_secs()
            );
        }
        thread::sleep(DEVICE_PROBE_INTERVAL);
    }
}

/// Returns true when a bounded shell session opens and exits cleanly.
fn device_answers(remote: &str) -> bool {
    run_remote_shell(remote, "exit\n", DEVICE_PROBE_TIMEOUT)
        .is_ok_and(|output| output.status.success())
}

fn parse_wait(arguments: &[String]) -> Result<(&str, Duration), String> {
    const USAGE: &str = "usage: kobo wait --device <host> [--timeout <seconds>]";
    let (host, rest) = match arguments {
        [device, host, rest @ ..] if device == "--device" => (host, rest),
        _ => return Err(USAGE.to_owned()),
    };
    if !valid_device_host(host) {
        return Err("device host contains unsupported characters".to_owned());
    }
    let seconds = match rest {
        [] => 300,
        [flag, value] if flag == "--timeout" => value
            .parse::<u64>()
            .map_err(|_| "--timeout takes a whole number of seconds".to_owned())?,
        _ => return Err(USAGE.to_owned()),
    };
    if seconds == 0 || seconds > DEVICE_WAIT_MAXIMUM_SECONDS {
        return Err(format!(
            "--timeout must be between 1 and {DEVICE_WAIT_MAXIMUM_SECONDS} seconds"
        ));
    }
    Ok((host, Duration::from_secs(seconds)))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DevSessionAction {
    Status,
    KeepAwake(devsession::Switch),
    WifiAlwaysOn(devsession::Switch),
    RestoreConfig,
    Hold(u64),
    SleepAfter(u32),
    RestoreSleepDefault,
}

fn parse_dev_session(arguments: &[String]) -> Result<(&str, DevSessionAction), String> {
    const USAGE: &str = "usage: kobo session --device <host> \
                         [--status | --keep-awake on|off | --wifi-always-on on|off \
                         | --sleep-after <minutes> | --sleep-after default \
                         | --hold [minutes] | --restore-reader-config]";
    let (host, rest) = match arguments {
        [device, host, rest @ ..] if device == "--device" => (host, rest),
        _ => return Err(USAGE.to_owned()),
    };
    if !valid_device_host(host) {
        return Err("device host contains unsupported characters".to_owned());
    }
    let action = match rest {
        [] | [_] if rest.first().is_none_or(|flag| flag == "--status") => DevSessionAction::Status,
        [flag] if flag == "--restore-reader-config" => DevSessionAction::RestoreConfig,
        [flag, value] if flag == "--keep-awake" => DevSessionAction::KeepAwake(
            devsession::Switch::parse(value).ok_or("--keep-awake takes exactly on or off")?,
        ),
        [flag, value] if flag == "--wifi-always-on" => DevSessionAction::WifiAlwaysOn(
            devsession::Switch::parse(value).ok_or("--wifi-always-on takes exactly on or off")?,
        ),
        [flag, value] if flag == "--sleep-after" && value == "default" => {
            DevSessionAction::RestoreSleepDefault
        }
        [flag, value] if flag == "--sleep-after" => {
            let minutes = value
                .parse::<u32>()
                .map_err(|_| "--sleep-after takes whole minutes or the word default".to_owned())?;
            if minutes == 0 || u64::from(minutes) > SLEEP_AFTER_MAXIMUM_MINUTES {
                return Err(format!(
                    "--sleep-after must be between 1 and {SLEEP_AFTER_MAXIMUM_MINUTES} minutes, \
                     or the word default"
                ));
            }
            DevSessionAction::SleepAfter(minutes)
        }
        [flag] if flag == "--hold" => DevSessionAction::Hold(30),
        [flag, value] if flag == "--hold" => {
            let minutes = value
                .parse::<u64>()
                .map_err(|_| "--hold takes a whole number of minutes".to_owned())?;
            if minutes == 0 || minutes > HOLD_MAXIMUM_MINUTES {
                return Err(format!(
                    "--hold must be between 1 and {HOLD_MAXIMUM_MINUTES} minutes"
                ));
            }
            DevSessionAction::Hold(minutes)
        }
        _ => return Err(USAGE.to_owned()),
    };
    Ok((host, action))
}

#[cfg(feature = "device-write")]
fn smoke_display(arguments: &[String]) -> Result<(), String> {
    let (host, stage) = parse_smoke_display(arguments)?;
    run_remote_fixed_artifact(host, &RemoteArtifact::smoke(stage))
}

/// Proves the guardian restores the screen after a supervised child fails.
///
/// The guard damages a region on purpose, runs a child that exits non-zero,
/// then restores the captured screen and verifies it byte for byte. Without the
/// deliberate damage a passing run would prove nothing.
#[cfg(feature = "device-write")]
fn guard_test(arguments: &[String]) -> Result<(), String> {
    let host = parse_guard_test(arguments)?;
    run_remote_fixed_artifact(host, &RemoteArtifact::guard())
}

#[cfg(feature = "device-write")]
fn parse_guard_test(arguments: &[String]) -> Result<&str, String> {
    match arguments {
        [device, host, confirm, value] if device == "--device" && confirm == "--confirm" => {
            if value != GUARD_TEST_CONFIRMATION {
                return Err(format!(
                    "confirmation must be exactly {GUARD_TEST_CONFIRMATION}"
                ));
            }
            if valid_device_host(host) {
                Ok(host)
            } else {
                Err("device host contains unsupported characters".to_owned())
            }
        }
        _ => Err(format!(
            "usage: kobo guard-test --device <host> --confirm {GUARD_TEST_CONFIRMATION}"
        )),
    }
}

#[cfg(feature = "device-write")]
fn parse_smoke_display(arguments: &[String]) -> Result<(&str, SmokeStage), String> {
    match arguments {
        [device, host, confirm, value] if device == "--device" && confirm == "--confirm" => {
            let stage = SmokeStage::from_confirmation(value).ok_or_else(|| {
                format!(
                    "confirmation must be exactly one of {}",
                    SmokeStage::confirmation_list()
                )
            })?;
            if valid_device_host(host) {
                Ok((host, stage))
            } else {
                Err("device host contains unsupported characters".to_owned())
            }
        }
        _ => Err(format!(
            "usage: kobo smoke-display --device <host> --confirm <{}>",
            SmokeStage::confirmation_list()
        )),
    }
}

/// Builds a device binary from this CLI's own workspace manifest.
///
/// Pinning the manifest path means the uploaded artifact is always built from
/// the reviewed source tree, never from whatever workspace the caller happens
/// to be standing in, and never a stale binary left in `target` by an earlier
/// revision.
fn device_build_command(package: &str, features: Option<&str>) -> Result<Command, String> {
    let linker = find_rust_lld()?;
    let mut command = Command::new("cargo");
    command
        .args([
            "build",
            "--release",
            "--locked",
            "--manifest-path",
            &workspace_manifest().display().to_string(),
            "--target",
            "armv7-unknown-linux-musleabihf",
            "-p",
            package,
            "--bin",
            package,
        ])
        .env("CARGO_TARGET_ARMV7_UNKNOWN_LINUX_MUSLEABIHF_LINKER", linker);
    if let Some(features) = features {
        command.args(["--features", features]);
    }
    Ok(command)
}

#[derive(Clone, Copy)]
enum RemoteProgram {
    Doctor,
    /// The same read-only doctor binary, additionally watching touch for the
    /// given number of seconds.
    TouchProbe(u64),
    #[cfg(feature = "device-write")]
    Smoke(SmokeStage),
    #[cfg(feature = "device-write")]
    Guard,
}

struct RemoteArtifact {
    label: &'static str,
    directory_label: &'static str,
    binary_name: &'static str,
    local_binary: PathBuf,
    package: &'static str,
    features: Option<&'static str>,
    program: RemoteProgram,
}

impl RemoteArtifact {
    /// The host-side ceiling for this artifact, which must always exceed the
    /// device-side one so the device's own bound is what actually fires.
    fn timeout(&self) -> Duration {
        match self.program {
            RemoteProgram::TouchProbe(seconds) => {
                Duration::from_secs(seconds) + TOUCH_PROBE_OVERHEAD
            }
            RemoteProgram::Doctor => REMOTE_COMMAND_TIMEOUT,
            #[cfg(feature = "device-write")]
            RemoteProgram::Smoke(_) | RemoteProgram::Guard => REMOTE_COMMAND_TIMEOUT,
        }
    }
}

impl RemoteArtifact {
    fn doctor() -> Self {
        Self {
            label: "read-only doctor",
            directory_label: "kobo-doctor",
            binary_name: "kobo-doctor",
            local_binary: workspace_doctor_binary(),
            package: "kobo-doctor",
            features: None,
            program: RemoteProgram::Doctor,
        }
    }

    fn touch_probe(seconds: u64) -> Self {
        Self {
            program: RemoteProgram::TouchProbe(seconds),
            label: "read-only touch probe",
            ..Self::doctor()
        }
    }

    #[cfg(feature = "device-write")]
    fn guard() -> Self {
        Self {
            label: "guard restore test",
            directory_label: "kobo-guard",
            binary_name: "kobo-guard",
            local_binary: workspace_device_binary("kobo-guard"),
            package: "kobo-guard",
            features: Some("device-write"),
            program: RemoteProgram::Guard,
        }
    }

    #[cfg(feature = "device-write")]
    fn smoke(stage: SmokeStage) -> Self {
        Self {
            label: "display smoke",
            directory_label: "kobo-smoke",
            binary_name: "kobo-smoke",
            local_binary: workspace_smoke_binary(),
            package: "kobo-smoke",
            features: Some("device-write"),
            program: RemoteProgram::Smoke(stage),
        }
    }
}

struct RemoteArtifactSession {
    directory: String,
    binary: String,
    owner_file: String,
    owner_token: String,
}

fn run_remote_fixed_artifact(host: &str, artifact: &RemoteArtifact) -> Result<(), String> {
    // Always rebuild from the pinned workspace. Uploading a binary that does not
    // match the source in front of the reviewer is exactly how a device ends up
    // running something nobody checked.
    let mut build = device_build_command(artifact.package, artifact.features)?;
    run_status(
        &mut build,
        format!("build fixed {} artifact", artifact.label),
    )?;
    if !artifact.local_binary.is_file() {
        return Err(format!(
            "{} not found after building the fixed {} artifact",
            artifact.local_binary.display(),
            artifact.label
        ));
    }
    verify_arm_elf(&artifact.local_binary)?;
    let bytes = fs::read(&artifact.local_binary).map_err(|error| {
        format!(
            "read {} for upload: {error}",
            artifact.local_binary.display()
        )
    })?;
    // Hash exactly the bytes that are uploaded, so the device verifies the same
    // artifact this process read rather than whatever is on disk afterwards.
    let checksum = sha256::hex_digest(&bytes);
    let session = remote_artifact_session(artifact)?;
    let remote = format!("root@{host}");
    let script = remote_fixed_artifact_script(
        &session,
        artifact.program,
        &checksum,
        &base64_encode(&bytes),
    );
    match run_remote_shell(&remote, &script, artifact.timeout()) {
        Ok(output) if output.status.success() => {
            print!("{}", String::from_utf8_lossy(&output.stdout));
            Ok(())
        }
        Ok(output) => {
            let cleanup = cleanup_remote_fixed_artifact(&remote, &session);
            Err(unreachable_if_ssh_gave_up(
                remote_session_failure(
                    format!("{} exited with {}", artifact.label, output.status),
                    &output,
                    cleanup.err(),
                ),
                &output,
            ))
        }
        Err(error) => {
            let cleanup = cleanup_remote_fixed_artifact(&remote, &session);
            Err(unreachable_device(match cleanup {
                Ok(()) => error,
                Err(cleanup_error) => format!("{error}; cleanup failed: {cleanup_error}"),
            }))
        }
    }
}

fn valid_device_host(host: &str) -> bool {
    !host.is_empty()
        && host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b':' | b'-' | b'_'))
}

fn workspace_manifest() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("Cargo.toml")
}

/// Resolves a device binary inside this workspace's own target directory.
///
/// Pinning it to this manifest means an uploaded artifact always comes from the
/// reviewed source tree rather than whatever workspace the caller stood in.
fn workspace_device_binary(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("target/armv7-unknown-linux-musleabihf/release")
        .join(name)
}

fn workspace_doctor_binary() -> PathBuf {
    workspace_device_binary("kobo-doctor")
}

#[cfg(feature = "device-write")]
fn workspace_smoke_binary() -> PathBuf {
    workspace_device_binary("kobo-smoke")
}

fn remote_artifact_session(artifact: &RemoteArtifact) -> Result<RemoteArtifactSession, String> {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let directory = format!(
        "/tmp/{}-{}-{unique}",
        artifact.directory_label,
        std::process::id()
    );
    Ok(RemoteArtifactSession {
        binary: format!("{directory}/{}", artifact.binary_name),
        owner_file: format!("{directory}/.{}-owner", artifact.directory_label),
        directory,
        owner_token: remote_owner_token()?,
    })
}

fn remote_owner_token() -> Result<String, String> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut bytes = [0_u8; 16];
    fs::File::open("/dev/urandom")
        .and_then(|mut random| random.read_exact(&mut bytes))
        .map_err(|error| format!("create remote cleanup ownership token: {error}"))?;
    let mut token = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        token.push(char::from(HEX[usize::from(byte >> 4)]));
        token.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    Ok(token)
}

fn remote_fixed_artifact_script(
    session: &RemoteArtifactSession,
    program: RemoteProgram,
    checksum: &str,
    encoded_artifact: &str,
) -> String {
    let execution = match program {
        RemoteProgram::Doctor => "\"$bin\"".to_owned(),
        // Bounded twice: the observation window is enforced in the binary and
        // again by timeout, so a stuck read cannot hold the device.
        RemoteProgram::TouchProbe(seconds) => format!(
            "if [ -x /usr/bin/timeout ]; then\n\
             \x20 KOBO_DOCTOR_OBSERVE_TOUCH={seconds} /usr/bin/timeout {} \"$bin\"\n\
             else\n\
             \x20 echo 'BusyBox timeout is unavailable; refusing touch probe' >&2\n\
             \x20 exit 1\n\
             fi",
            seconds + 15
        ),
        #[cfg(feature = "device-write")]
        RemoteProgram::Smoke(stage) => format!(
            "if [ -x /usr/bin/timeout ]; then\n\
             \x20 KOBO_SMOKE_UNLOCK='{}' /usr/bin/timeout {REMOTE_SMOKE_TIMEOUT_SECONDS} \"$bin\"\n\
             else\n\
             \x20 echo 'BusyBox timeout is unavailable; refusing display smoke' >&2\n\
             \x20 exit 1\n\
             fi",
            stage.device_unlock()
        ),
        #[cfg(feature = "device-write")]
        RemoteProgram::Guard => format!(
            "if [ -x /usr/bin/timeout ]; then\n\
             \x20 KOBO_GUARD_UNLOCK='OWNER_ATTENDED_GUARDED_SESSION' \
             /usr/bin/timeout {} \"$bin\" --run {GUARD_TEST_CHILD} --prove-restore \
             --timeout-seconds {GUARD_TEST_TIMEOUT_SECONDS}\n\
             else\n\
             \x20 echo 'BusyBox timeout is unavailable; refusing guard test' >&2\n\
             \x20 exit 1\n\
             fi",
            GUARD_TEST_TIMEOUT_SECONDS + 20
        ),
    };
    let checksum_error = match program {
        RemoteProgram::Doctor | RemoteProgram::TouchProbe(_) => {
            "uploaded doctor checksum does not match"
        }
        #[cfg(feature = "device-write")]
        RemoteProgram::Smoke(_) => "uploaded smoke checksum does not match",
        #[cfg(feature = "device-write")]
        RemoteProgram::Guard => "uploaded guard checksum does not match",
    };
    format!(
        "set -eu\n\
         umask 077\n\
         dir='{}'\n\
         bin='{}'\n\
         owner='{}'\n\
         token='{}'\n\
         mkdir -m 700 \"$dir\"\n\
         printf '%s\\n' \"$token\" > \"$owner\"\n\
         owned() {{\n\
           [ -f \"$owner\" ] || return 1\n\
           IFS= read -r actual < \"$owner\" || return 1\n\
           [ \"$actual\" = \"$token\" ]\n\
         }}\n\
         cleanup() {{\n\
           if owned; then\n\
             rm -f \"$bin\" \"$owner\"\n\
             rmdir \"$dir\"\n\
           fi\n\
         }}\n\
         trap cleanup EXIT HUP INT TERM\n\
         base64 -d > \"$bin\" <<'KOBO_ARTIFACT_BASE64'\n\
         {}\n\
         KOBO_ARTIFACT_BASE64\n\
         chmod 500 \"$bin\"\n\
         set -- $(sha256sum \"$bin\")\n\
         if [ \"$1\" != '{}' ]; then\n\
           echo '{}' >&2\n\
           exit 1\n\
         fi\n\
         {}\n\
         exit\n",
        session.directory,
        session.binary,
        session.owner_file,
        session.owner_token,
        encoded_artifact,
        checksum,
        checksum_error,
        execution,
    )
}

fn remote_cleanup_script(session: &RemoteArtifactSession) -> String {
    format!(
        "set -eu\n\
         dir='{}'\n\
         bin='{}'\n\
         owner='{}'\n\
         token='{}'\n\
         if [ -f \"$owner\" ]; then\n\
          actual=''\n\
          IFS= read -r actual < \"$owner\" || exit 0\n\
          if [ \"$actual\" = \"$token\" ]; then\n\
            rm -f \"$bin\" \"$owner\"\n\
            rmdir \"$dir\" 2>/dev/null || true\n\
          fi\n\
         fi\n\
         exit\n",
        session.directory, session.binary, session.owner_file, session.owner_token
    )
}

fn cleanup_remote_fixed_artifact(
    remote: &str,
    session: &RemoteArtifactSession,
) -> Result<(), String> {
    let output = run_remote_shell(
        remote,
        &remote_cleanup_script(session),
        REMOTE_CLEANUP_TIMEOUT,
    )?;
    if output.status.success() {
        Ok(())
    } else {
        Err(remote_session_failure(
            format!("remote cleanup exited with {}", output.status),
            &output,
            None,
        ))
    }
}

/// Adds the four-cause checklist to an error that means the device was never
/// reached.
///
/// Every one of those causes produces the same connection timeout, so the
/// error on its own tells the reader nothing they can act on. It is added at
/// the points where contact was never made rather than to every failure,
/// because a device that answered and then refused something has already told
/// them what was wrong.
#[must_use]
fn unreachable_device(mut error: String) -> String {
    error.push_str("\n\n");
    error.push_str(connect::OFFLINE_HELP);
    error
}

/// The same, for a session that ssh itself gave up on.
///
/// ssh reserves exit status 255 for its own failures — refused, timed out, key
/// rejected — so anything else came back from a shell that really did run on
/// the device, and the checklist would be misleading there.
#[must_use]
fn unreachable_if_ssh_gave_up(error: String, output: &RemoteShellOutput) -> String {
    if output.status.code() == Some(255) {
        unreachable_device(error)
    } else {
        error
    }
}

fn remote_session_failure(
    message: String,
    output: &RemoteShellOutput,
    cleanup_error: Option<String>,
) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    let mut result = message;
    if !stdout.is_empty() {
        result.push_str("; stdout: ");
        result.push_str(&stdout);
    }
    if !stderr.is_empty() {
        result.push_str("; stderr: ");
        result.push_str(&stderr);
    }
    if let Some(cleanup_error) = cleanup_error {
        result.push_str("; cleanup failed: ");
        result.push_str(&cleanup_error);
    }
    result
}

fn remote_shell_command(remote: &str) -> Command {
    let mut command = Command::new("ssh");
    command
        .args(["-T", "-o", "BatchMode=yes", "-o"])
        .arg(format!("ConnectTimeout={REMOTE_CONNECT_TIMEOUT_SECONDS}"))
        .arg(remote);
    command
}

#[derive(Debug)]
struct RemoteShellOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn run_remote_shell(
    remote: &str,
    script: &str,
    timeout: Duration,
) -> Result<RemoteShellOutput, String> {
    let mut command = remote_shell_command(remote);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("start remote shell: {error}"))?;
    let stdin_handle = child.stdin.take();
    let stdin = take_remote_pipe(&mut child, stdin_handle, "stdin")?;
    let stdout_handle = child.stdout.take();
    let stdout = take_remote_pipe(&mut child, stdout_handle, "stdout")?;
    let stderr_handle = child.stderr.take();
    let stderr = take_remote_pipe(&mut child, stderr_handle, "stderr")?;
    let script = script.as_bytes().to_vec();
    let writer = thread::spawn(move || -> std::io::Result<()> {
        let mut stdin = stdin;
        stdin.write_all(&script)?;
        stdin.flush()
    });
    let stdout_reader = thread::spawn(move || read_remote_pipe(stdout));
    let stderr_reader = thread::spawn(move || read_remote_pipe(stderr));
    let status = wait_for_remote_child(&mut child, "remote shell session", timeout);
    let writer_result = writer
        .join()
        .map_err(|_| "remote shell stdin writer panicked".to_owned())?;
    let stdout = stdout_reader
        .join()
        .map_err(|_| "remote shell stdout reader panicked".to_owned())?
        .map_err(|error| format!("read remote stdout: {error}"))?;
    let stderr = stderr_reader
        .join()
        .map_err(|_| "remote shell stderr reader panicked".to_owned())?
        .map_err(|error| format!("read remote stderr: {error}"))?;
    let status = status.map_err(|error| remote_shell_error(error, &stdout, &stderr))?;
    if let Err(error) = writer_result {
        return Err(remote_shell_error(
            format!("write remote script: {error}"),
            &stdout,
            &stderr,
        ));
    }
    Ok(RemoteShellOutput {
        status,
        stdout,
        stderr,
    })
}

fn remote_shell_error(message: String, stdout: &[u8], stderr: &[u8]) -> String {
    let stdout = String::from_utf8_lossy(stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(stderr).trim().to_owned();
    let mut result = message;
    if !stdout.is_empty() {
        result.push_str("; stdout: ");
        result.push_str(&stdout);
    }
    if !stderr.is_empty() {
        result.push_str("; stderr: ");
        result.push_str(&stderr);
    }
    result
}

fn read_remote_pipe(mut pipe: impl Read) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    pipe.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn take_remote_pipe<T>(child: &mut Child, pipe: Option<T>, name: &str) -> Result<T, String> {
    pipe.ok_or_else(|| {
        terminate_remote_child(child);
        format!("start remote shell: {name} was not captured")
    })
}

fn terminate_remote_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn wait_for_remote_child(
    child: &mut Child,
    description: &str,
    timeout: Duration,
) -> Result<ExitStatus, String> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {}
            Err(error) => {
                terminate_remote_child(child);
                return Err(format!("{description}: inspect child: {error}"));
            }
        }
        if Instant::now() >= deadline {
            terminate_remote_child(child);
            return Err(format!(
                "{description} timed out after {} seconds",
                timeout.as_secs()
            ));
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4 + bytes.len() / 57);
    let mut column = 0;
    for chunk in bytes.chunks(3) {
        let first = u32::from(chunk[0]);
        let second = u32::from(*chunk.get(1).unwrap_or(&0));
        let third = u32::from(*chunk.get(2).unwrap_or(&0));
        let value = (first << 16) | (second << 8) | third;
        output.push(TABLE[((value >> 18) & 0x3f) as usize] as char);
        output.push(TABLE[((value >> 12) & 0x3f) as usize] as char);
        output.push(if chunk.len() > 1 {
            TABLE[((value >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            TABLE[(value & 0x3f) as usize] as char
        } else {
            '='
        });
        column += 4;
        if column == 76 {
            output.push('\n');
            column = 0;
        }
    }
    output
}

fn verify_command(arguments: &[String]) -> Result<(), String> {
    let path = arguments.first().ok_or("usage: kobo verify <arm-binary>")?;
    verify_arm_elf(Path::new(path))?;
    println!("{path}: static ARM EABI5 hard-float");
    Ok(())
}

/// Builds the single file a Kobo owner copies onto their device.
///
/// The whole point is that the owner never sees SSH, an IP address, or this
/// device's habit of ignoring remote arguments. They copy one file into
/// `.kobo/`, eject, and the reader installs it at the next boot using its own
/// battery-checked, recovery-bracketed installer.
/// Refuses a daemon that cannot start a panel session.
///
/// A `kobod` built without `device-write` is a perfectly valid ARM binary that
/// passes every other check in this file and then answers `start.sh` with a
/// usage message. The phrase searched for here is the unlock `present_on_panel`
/// compares against, and that function is the whole of what the feature adds.
fn verify_present_is_compiled_in(bytes: &[u8], binary: &Path) -> Result<(), String> {
    if bytes
        .windows(PRESENT_UNLOCK_PHRASE.len())
        .any(|window| window == PRESENT_UNLOCK_PHRASE)
    {
        return Ok(());
    }
    Err(format!(
        "{}: built without the device-write feature, so --present is not compiled in \
         and start.sh would fail with a usage message",
        binary.display()
    ))
}

/// A package, and what reading its finished bytes back said was in it.
///
/// Built once and then used whichever way it is going to reach a device, so
/// `kobo package` and `kobo deploy` can never disagree about what Cobalt is.
struct BuiltPackage {
    members: Vec<package::Member>,
    listed: Vec<package::Listed>,
    compressed: Vec<u8>,
}

impl BuiltPackage {
    /// How many regular files an owner is about to gain.
    fn file_count(&self) -> usize {
        self.listed
            .iter()
            .filter(|entry| entry.kind == b'0')
            .count()
    }
}

/// Builds every device binary and packs them into the archive an owner
/// installs.
///
/// This is the whole of the build, deliberately separated from writing it to a
/// file: the archive that goes over USB and the archive that goes over Wi-Fi
/// have to be the same bytes, produced by the same checks, or one of the two
/// paths is unreviewed.
fn build_package_bytes() -> Result<BuiltPackage, String> {
    let mut members = Vec::new();
    for (name, features) in INSTALLED_PACKAGES {
        run_status(
            &mut device_build_command(name, *features)?,
            format!("cargo build {name}"),
        )?;
        let binary = Path::new("target/armv7-unknown-linux-musleabihf/release").join(name);
        // The same check the device build already applies, repeated here
        // because this is the artifact somebody else's device will run.
        verify_arm_elf(&binary)?;
        let bytes =
            fs::read(&binary).map_err(|error| format!("read {}: {error}", binary.display()))?;
        if *name == "kobod" {
            verify_present_is_compiled_in(&bytes, &binary)?;
        }
        members.push(package::Member {
            path: format!("{}/bin/{name}", package::INSTALL_ROOT),
            bytes,
            program: true,
        });
    }
    members.push(text_member("start.sh", START_SCRIPT, true));
    members.push(text_member("README.txt", INSTALL_README, false));
    members.push(text_member(
        "VERSION",
        &format!("{}\n", env!("CARGO_PKG_VERSION")),
        false,
    ));

    let archive = package::tar(&members)?;
    // Read back rather than trusted. This archive is extracted as root by the
    // device's boot script, so the list of what it will write is checked from
    // the bytes that were produced, not from the list they were produced from.
    let listed = package::list(&archive)?;
    let outside = members_outside_install_root(&listed);
    if !outside.is_empty() {
        return Err(format!(
            "refusing to build: {} would be written outside {}",
            outside.join(", "),
            package::INSTALL_ROOT
        ));
    }
    let compressed = gzip(&archive)?;
    // Exactly what `rcS` does before it extracts anything. A tarball that
    // fails this is silently ignored on the device, which looks like an
    // install that did nothing.
    gzip_test(&compressed)?;
    Ok(BuiltPackage {
        members,
        listed,
        compressed,
    })
}

fn build_package(arguments: &[String]) -> Result<(), String> {
    let (tarball, folder) = parse_package(arguments)?;
    let built = build_package_bytes()?;

    if let Some(parent) = tarball.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    fs::write(&tarball, &built.compressed)
        .map_err(|error| format!("write {}: {error}", tarball.display()))?;
    if let Some(folder) = folder {
        package::write_folder(&built.members, &folder)?;
        println!(
            "also written as a plain folder: {}\n\
             copy it into .adds/ on the device and name it cobalt, or copy it over\n\
             an existing .adds/cobalt to update in place",
            folder.display()
        );
    }

    let files = built.file_count();
    println!(
        "{}: {files} files, {} bytes, sha256 {}",
        tarball.display(),
        built.compressed.len(),
        sha256::hex_digest(&built.compressed)
    );
    println!("{INSTALL_INSTRUCTIONS}");
    Ok(())
}

/// Installs Cobalt onto a device over Wi-Fi, with no reboot and no USB cable.
///
/// This exists because `/mnt/onboard` is mounted without `noexec`, so an
/// install is nothing more than putting a folder of files on the book
/// partition. The vendor installer is not involved, which is why this needs no
/// reboot and is not the path an ordinary owner uses: it needs SSH already set
/// up, and `kobo package` remains the answer for somebody who has no terminal.
///
/// Nothing here can write outside `.adds/cobalt`. The archive is checked on
/// this machine before it is sent, and the script checks the same thing again
/// on the device from the bytes that actually arrived, because that half runs
/// as root. A running panel session is refused rather than overwritten, since
/// the files being replaced are the ones it is executing.
fn deploy_package(arguments: &[String]) -> Result<(), String> {
    let (host, supplied) = parse_deploy(arguments)?;
    let (compressed, files) = if let Some(path) = supplied {
        validated_package(&path)?
    } else {
        let built = build_package_bytes()?;
        let files = built.file_count();
        (built.compressed, files)
    };
    // Hash exactly the bytes that go up the pipe, so what the device verifies
    // is what this process sent rather than whatever is on disk afterwards.
    let checksum = sha256::hex_digest(&compressed);
    println!(
        "installing {files} files, {} bytes, sha256 {checksum} into {} on {host}",
        compressed.len(),
        connect::INSTALL_DIRECTORY
    );
    let script = connect::install_script(&base64_encode(&compressed), &checksum);
    let output = run_remote_shell(&format!("root@{host}"), &script, DEPLOY_TIMEOUT)
        .map_err(unreachable_device)?;
    if !output.status.success() {
        return Err(unreachable_if_ssh_gave_up(
            remote_session_failure(
                format!("install on {host} exited with {}", output.status),
                &output,
                None,
            ),
            &output,
        ));
    }
    let reported = String::from_utf8_lossy(&output.stdout);
    let version = reported_value(&reported, "installed").unwrap_or("unknown");
    let binaries = reported_value(&reported, "binaries").unwrap_or("no");
    println!(
        "installed Cobalt {version} on {host}: {binaries} binaries in {}",
        connect::INSTALL_DIRECTORY
    );
    println!(
        "nothing is running yet. Start it on the reader with {}/start.sh, or from a\n\
         NickelMenu entry if you have one. A reboot always returns to the stock reader.",
        connect::INSTALL_DIRECTORY
    );
    Ok(())
}

/// The value of one `key=value` line a device script reported.
///
/// Absent rather than wrong when the line is missing, so a device that printed
/// less than expected produces a vaguer message rather than a false one.
fn reported_value<'a>(output: &'a str, key: &str) -> Option<&'a str> {
    output.lines().find_map(|line| {
        let (name, value) = line.trim().split_once('=')?;
        (name == key).then_some(value.trim())
    })
}

/// Reads a package from disk and refuses one that could write anywhere but the
/// install root.
///
/// Exactly the reading `kobo inspect` performs, applied before anything is
/// uploaded: an archive nobody has read back is an archive nobody knows the
/// contents of, and this one is extracted as root.
fn validated_package(path: &Path) -> Result<(Vec<u8>, usize), String> {
    let compressed = fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    gzip_test(&compressed)?;
    let archive = gunzip(&compressed)?;
    let listed = package::list(&archive)?;
    let outside = members_outside_install_root(&listed);
    if !outside.is_empty() {
        return Err(format!(
            "refusing to upload {}: {} would be written outside {}",
            path.display(),
            outside.join(", "),
            package::INSTALL_ROOT
        ));
    }
    Ok((
        compressed,
        listed.iter().filter(|entry| entry.kind == b'0').count(),
    ))
}

fn parse_deploy(arguments: &[String]) -> Result<(&str, Option<PathBuf>), String> {
    const USAGE: &str = "usage: kobo deploy --device <host> [--package <path>]";
    let (host, rest) = match arguments {
        [device, host, rest @ ..] if device == "--device" => (host.as_str(), rest),
        _ => return Err(USAGE.to_owned()),
    };
    if !valid_device_host(host) {
        return Err("device host contains unsupported characters".to_owned());
    }
    let package = match rest {
        [] => None,
        [flag, value] if flag == "--package" => Some(PathBuf::from(value)),
        _ => return Err(USAGE.to_owned()),
    };
    Ok((host, package))
}

/// Every listed path that would land somewhere other than the install root.
///
/// Taken from entries read back out of finished archive bytes rather than from
/// the member list they were built from, because the archive is what a device
/// extracts as root. The directories leading down to the root are allowed,
/// since an archive has to create them to create anything inside them.
fn members_outside_install_root(listed: &[package::Listed]) -> Vec<String> {
    let root = format!("{}/", package::INSTALL_ROOT);
    listed
        .iter()
        .filter(|entry| {
            !(entry.path.starts_with(&root) || root.starts_with(entry.path.trim_end_matches('/')))
        })
        .map(|entry| entry.path.clone())
        .collect()
}

/// Lists a package and proves it cannot write outside the install root.
fn inspect_package(arguments: &[String]) -> Result<(), String> {
    let path = arguments.first().ok_or("usage: kobo inspect <package>")?;
    let compressed = fs::read(path).map_err(|error| format!("read {path}: {error}"))?;
    gzip_test(&compressed)?;
    let archive = gunzip(&compressed)?;
    let listed = package::list(&archive)?;
    for entry in &listed {
        let kind = if entry.kind == b'5' { "dir " } else { "file" };
        println!("{kind} {:o} {:>9} {}", entry.mode, entry.size, entry.path);
    }
    let outside = members_outside_install_root(&listed);
    if outside.is_empty() {
        println!(
            "nothing outside {}; this package writes no root filesystem file",
            package::INSTALL_ROOT
        );
        Ok(())
    } else {
        Err(format!(
            "refusing: {} would be written outside {}",
            outside.join(", "),
            package::INSTALL_ROOT
        ))
    }
}

fn text_member(name: &str, contents: &str, program: bool) -> package::Member {
    package::Member {
        path: format!("{}/{name}", package::INSTALL_ROOT),
        bytes: contents.as_bytes().to_vec(),
        program,
    }
}

fn parse_package(arguments: &[String]) -> Result<(PathBuf, Option<PathBuf>), String> {
    let mut tarball = PathBuf::from("target/KoboRoot.tgz");
    let mut folder = None;
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--out" => {
                let value = arguments
                    .get(index + 1)
                    .ok_or("usage: kobo package [--out PATH] [--folder PATH]")?;
                tarball = PathBuf::from(value);
                index += 2;
            }
            "--folder" => {
                let value = arguments
                    .get(index + 1)
                    .ok_or("usage: kobo package [--out PATH] [--folder PATH]")?;
                folder = Some(PathBuf::from(value));
                index += 2;
            }
            other => return Err(format!("unknown option {other:?}")),
        }
    }
    Ok((tarball, folder))
}

/// Compresses with the system `gzip`.
///
/// `-n` keeps the name and timestamp out of the header, so the same input
/// produces the same file and the checksum an owner compares is stable.
fn gzip(bytes: &[u8]) -> Result<Vec<u8>, String> {
    pipe_through(Command::new("gzip").args(["-n", "-9", "-c"]), bytes)
}

fn gunzip(bytes: &[u8]) -> Result<Vec<u8>, String> {
    pipe_through(Command::new("gzip").args(["-d", "-c"]), bytes)
}

/// The integrity check `rcS` runs before it extracts anything.
fn gzip_test(bytes: &[u8]) -> Result<(), String> {
    pipe_through(Command::new("gzip").arg("-t"), bytes)
        .map(|_| ())
        .map_err(|error| format!("the package fails the check the device runs first: {error}"))
}

fn pipe_through(command: &mut Command, input: &[u8]) -> Result<Vec<u8>, String> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("run gzip: {error}"))?;
    let mut stdin = child.stdin.take().ok_or("gzip has no standard input")?;
    let bytes = input.to_vec();
    // Written on a thread because a large archive fills the pipe buffer, and
    // writing it all before reading anything would deadlock against a gzip
    // that is waiting for somebody to read its output.
    let writer = thread::spawn(move || stdin.write_all(&bytes));
    let output = child
        .wait_with_output()
        .map_err(|error| format!("gzip: {error}"))?;
    writer
        .join()
        .map_err(|_| "the gzip writer panicked".to_owned())?
        .map_err(|error| format!("write to gzip: {error}"))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

fn run_simulation() -> Result<(), String> {
    let mut build = Command::new("cargo");
    build.args(["build", "-p", "kobod", "-p", "kobo-todo"]);
    run_status(&mut build, "build host simulation")?;

    let mut simulation = SimulationGuard::new()?;
    simulation.spawn_daemon()?;
    let ready_deadline = Instant::now() + Duration::from_secs(5);
    while !simulation.socket.exists() {
        if Instant::now() >= ready_deadline {
            return Err("simulated kobod did not become ready".to_owned());
        }
        if let Some(status) = simulation.daemon_try_wait()? {
            return Err(format!(
                "simulated kobod exited before accepting an app: {status}"
            ));
        }
        thread::sleep(Duration::from_millis(25));
    }
    if let Some(status) = simulation.daemon_try_wait()? {
        return Err(format!(
            "simulated kobod exited before accepting an app: {status}"
        ));
    }

    let app_status = Command::new("target/debug/kobo-todo")
        .env("KOBO_SOCKET", &simulation.socket)
        .env("KOBO_SIM_ONESHOT", "1")
        .status()
        .map_err(|error| format!("run todo app: {error}"))?;
    let daemon_status = simulation.daemon_wait()?;
    if !app_status.success() || !daemon_status.success() {
        return Err(format!(
            "simulation failed: app={app_status}, daemon={daemon_status}"
        ));
    }
    let expected = 1072_usize * 1448;
    let actual = fs::metadata(&simulation.frame)
        .map_err(|error| format!("inspect rendered frame: {error}"))?
        .len();
    if actual != expected as u64 {
        return Err(format!(
            "rendered frame is {actual} bytes; expected {expected}"
        ));
    }
    let output = Path::new("target/kobo-sim-last.raw");
    fs::copy(&simulation.frame, output).map_err(|error| format!("save rendered frame: {error}"))?;
    println!("host runtime completed; frame: {}", output.display());
    Ok(())
}

struct SimulationGuard {
    root: PathBuf,
    socket: PathBuf,
    frame: PathBuf,
    daemon: Option<Child>,
    daemon_frame_temporary: Option<PathBuf>,
}

impl SimulationGuard {
    fn new() -> Result<Self, String> {
        Self::new_at(env::temp_dir().join(format!("kobo-sim-{}", std::process::id())))
    }

    fn new_at(root: PathBuf) -> Result<Self, String> {
        fs::create_dir(&root).map_err(|error| format!("create {}: {error}", root.display()))?;
        let guard = Self {
            socket: root.join("kobod.sock"),
            frame: root.join("frame.raw"),
            root,
            daemon: None,
            daemon_frame_temporary: None,
        };
        if let Err(error) = fs::set_permissions(&guard.root, fs::Permissions::from_mode(0o700)) {
            let message = format!("protect {}: {error}", guard.root.display());
            drop(guard);
            return Err(message);
        }
        Ok(guard)
    }

    fn spawn_daemon(&mut self) -> Result<(), String> {
        let daemon = Command::new("target/debug/kobod")
            .args(["--sim-socket"])
            .arg(&self.socket)
            .arg("--frame")
            .arg(&self.frame)
            .spawn()
            .map_err(|error| format!("start simulated kobod: {error}"))?;
        self.daemon_frame_temporary = Some(
            self.frame
                .with_extension(format!("raw.tmp-{}", daemon.id())),
        );
        self.daemon = Some(daemon);
        Ok(())
    }

    fn daemon_try_wait(&mut self) -> Result<Option<ExitStatus>, String> {
        self.daemon.as_mut().map_or(Ok(None), |daemon| {
            daemon
                .try_wait()
                .map_err(|error| format!("inspect simulated kobod: {error}"))
        })
    }

    fn daemon_wait(&mut self) -> Result<ExitStatus, String> {
        self.daemon.as_mut().map_or_else(
            || Err("simulated kobod was not started".to_owned()),
            |daemon| {
                daemon
                    .wait()
                    .map_err(|error| format!("wait for simulated kobod: {error}"))
            },
        )
    }
}

impl Drop for SimulationGuard {
    fn drop(&mut self) {
        if let Some(daemon) = &mut self.daemon {
            if daemon.try_wait().ok().flatten().is_none() {
                let _ = daemon.kill();
                let _ = daemon.wait();
            }
        }
        let _ = fs::remove_file(&self.socket);
        let _ = fs::remove_file(&self.frame);
        if let Some(temporary) = &self.daemon_frame_temporary {
            let _ = fs::remove_file(temporary);
        }
        let _ = fs::remove_dir(&self.root);
    }
}

fn find_rust_lld() -> Result<PathBuf, String> {
    let output = Command::new("rustc")
        .arg("--print")
        .arg("sysroot")
        .output()
        .map_err(|error| format!("locate Rust sysroot: {error}"))?;
    if !output.status.success() {
        return Err("rustc --print sysroot failed".to_owned());
    }
    let root = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    let rustlib = root.join("lib/rustlib");
    for entry in
        fs::read_dir(&rustlib).map_err(|error| format!("read {}: {error}", rustlib.display()))?
    {
        let candidate = entry
            .map_err(|error| error.to_string())?
            .path()
            .join("bin/rust-lld");
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err("rust-lld was not found in the active Rust toolchain".to_owned())
}

fn verify_arm_elf(path: &Path) -> Result<(), String> {
    let bytes = fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    if bytes.len() < 52 || &bytes[..4] != b"\x7fELF" {
        return Err(format!("{} is not an ELF binary", path.display()));
    }
    if bytes[4] != 1 || bytes[5] != 1 {
        return Err("expected a little-endian ELF32 binary".to_owned());
    }
    if read_u16(&bytes, 18)? != 40 {
        return Err("expected an ARM ELF binary".to_owned());
    }
    let flags = read_u32(&bytes, 36)?;
    if flags & 0x400 == 0 || flags & 0x200 != 0 {
        return Err(format!(
            "expected ARM hard-float ABI flags, found 0x{flags:08x}"
        ));
    }
    let program_offset =
        usize::try_from(read_u32(&bytes, 28)?).map_err(|_| "program offset overflow")?;
    let entry_size = usize::from(read_u16(&bytes, 42)?);
    let entry_count = usize::from(read_u16(&bytes, 44)?);
    if entry_size < 32 {
        return Err("invalid ELF program header size".to_owned());
    }
    for index in 0..entry_count {
        let offset = program_offset
            .checked_add(
                index
                    .checked_mul(entry_size)
                    .ok_or("program header overflow")?,
            )
            .ok_or("program header overflow")?;
        let kind = read_u32(&bytes, offset)?;
        if kind == 2 || kind == 3 {
            return Err("binary contains a dynamic or interpreter program header".to_owned());
        }
    }
    Ok(())
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, String> {
    let value = bytes
        .get(offset..offset + 2)
        .ok_or("truncated ELF header")?;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, String> {
    let value = bytes
        .get(offset..offset + 4)
        .ok_or("truncated ELF header")?;
    Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

fn sibling_binary(name: &str) -> PathBuf {
    env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join(name)))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from(name))
}

fn run_status<S>(command: &mut Command, description: S) -> Result<(), String>
where
    S: AsRef<OsStr>,
{
    let status = command
        .status()
        .map_err(|error| format!("{}: {error}", Path::new(description.as_ref()).display()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "{} exited with {status}",
            Path::new(description.as_ref()).display()
        ))
    }
}

fn print_help() {
    println!(
        "Kobo application SDK\n\n\
         Usage: kobo <command>\n\n\
         Commands:\n\
           new <name>             Create a Rust application\n\
           dev [--builtin] [address]  Run this SDK app in the browser simulator\n\
           build [--device]       Build host workspace or ARM safe doctor, disabled kobod, and sample app\n\
           doctor [--device IP]   Run read-only device diagnostics\n\
           devices [--subnet A.B.C]  Find every reader on the local network\n\
           session --device IP    Keep a device awake and on Wi-Fi while developing\n\
           session --device IP --hold [minutes]  Keep it reachable for unattended testing\n\
           wait --device IP       Block until a device answers again\n\
           touch-probe --device IP [--seconds N]  Watch touch read-only to check the transform\n\
           guard-test --device IP --confirm ...   Prove the guardian restores the screen\n\
           package [--out PATH] [--folder PATH]  Build the KoboRoot.tgz an owner copies\n\
           deploy --device IP [--package PATH]   Install over Wi-Fi, no reboot\n\
           inspect <package>       List a package and prove it writes nothing to the rootfs\n\
           verify <arm-binary>     Verify static ARM hard-float format\n\
           run --sim              Run SDK, IPC, daemon and app on host\n\
           run                    Device execution remains safety-gated\n\
           version                Print version"
    );
}

#[cfg(test)]
mod tests {
    use super::package;
    use super::{
        build_executables, manifest_uses_sdk, parse_deploy, parse_devices, parse_touch_probe,
        unreachable_device, valid_device_host, valid_slug, verify_arm_elf, wait_for_remote_child,
        workspace_doctor_binary, DevSessionGuard, RemoteArtifact, SimulationGuard, DEPLOY_TIMEOUT,
        DEVICE_PACKAGES, GENERATED_APP_SOURCE, TOUCH_PROBE_DEFAULT_SECONDS,
        TOUCH_PROBE_MAXIMUM_SECONDS,
    };
    #[cfg(feature = "device-write")]
    use super::{
        parse_guard_test, parse_smoke_display, run, workspace_smoke_binary, RemoteArtifactSession,
        RemoteProgram, SmokeStage, GUARD_TEST_CHILD, GUARD_TEST_CONFIRMATION,
        REMOTE_CLEANUP_TIMEOUT, REMOTE_COMMAND_TIMEOUT, REMOTE_CONNECT_TIMEOUT_SECONDS,
        REMOTE_SMOKE_TIMEOUT_SECONDS,
    };
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;
    use std::time::Duration;

    #[test]
    fn a_touch_probe_window_is_bounded_and_the_host_waits_longer_than_the_device() {
        let arguments = |parts: &[&str]| {
            parts
                .iter()
                .map(|part| (*part).to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            parse_touch_probe(&arguments(&["--device", "192.168.1.15"])),
            Ok(("192.168.1.15", TOUCH_PROBE_DEFAULT_SECONDS))
        );
        for rejected in [
            vec!["--device", "192.168.1.15", "--seconds", "0"],
            vec!["--device", "192.168.1.15", "--seconds", "121"],
            vec!["--device", "192.168.1.15", "--seconds", "ten"],
            vec!["--device", "192.168.1.15; reboot"],
            vec!["--device"],
        ] {
            assert!(
                parse_touch_probe(&arguments(&rejected)).is_err(),
                "{rejected:?} must be refused"
            );
        }
        // The device enforces its own bound, so the host must outlast it.
        let artifact = RemoteArtifact::touch_probe(TOUCH_PROBE_MAXIMUM_SECONDS);
        assert!(artifact.timeout().as_secs() > TOUCH_PROBE_MAXIMUM_SECONDS + 15);
    }

    /// A sweep builds addresses by appending a host part, so anything that is
    /// not exactly three octets would produce addresses nobody asked for.
    #[test]
    fn a_sweep_is_confined_to_one_named_subnet() {
        let arguments = |parts: &[&str]| {
            parts
                .iter()
                .map(|part| (*part).to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            parse_devices(&arguments(&["--subnet", "192.168.1"])),
            Ok("192.168.1".to_owned())
        );
        for rejected in [
            vec!["--subnet", "192.168.1.10"],
            vec!["--subnet", "192.168"],
            vec!["--subnet", "192.168.1; reboot"],
            vec!["--subnet", "$(hostname)"],
            vec!["--subnet"],
            vec!["--subnet", "192.168.1", "--extra"],
            vec!["192.168.1"],
        ] {
            assert!(
                parse_devices(&arguments(&rejected)).is_err(),
                "{rejected:?} must be refused"
            );
        }
        // With no argument the subnet comes from this machine's own route, and
        // a machine with no route has nothing to scan rather than a default.
        assert_eq!(
            parse_devices(&[]).is_ok(),
            super::connect::local_subnet().is_some()
        );
    }

    #[test]
    fn a_deploy_names_one_host_and_at_most_one_package() {
        let arguments = |parts: &[&str]| {
            parts
                .iter()
                .map(|part| (*part).to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            parse_deploy(&arguments(&["--device", "192.168.1.15"])),
            Ok(("192.168.1.15", None))
        );
        assert_eq!(
            parse_deploy(&arguments(&[
                "--device",
                "192.168.1.15",
                "--package",
                "target/KoboRoot.tgz"
            ])),
            Ok(("192.168.1.15", Some(PathBuf::from("target/KoboRoot.tgz"))))
        );
        for rejected in [
            vec!["--device", "192.168.1.15; reboot"],
            vec!["--device", ""],
            vec!["--device"],
            vec!["--device", "192.168.1.15", "--package"],
            vec!["--device", "192.168.1.15", "--out", "somewhere"],
            vec!["--package", "target/KoboRoot.tgz"],
            vec![],
        ] {
            assert!(
                parse_deploy(&arguments(&rejected)).is_err(),
                "{rejected:?} must be refused"
            );
        }
        // Six and a half megabytes of base64 through one stdin pipe took about
        // ten seconds on the device, so the budget has to be far larger.
        assert!(DEPLOY_TIMEOUT.as_secs() >= 180);
    }

    /// The checklist is the whole value of these messages, so it has to
    /// survive being attached to an error rather than replacing one.
    #[test]
    fn an_unreachable_device_keeps_its_error_and_gains_the_checklist() {
        let reported = unreachable_device("device 192.168.1.15 did not answer".to_owned());
        assert!(reported.starts_with("device 192.168.1.15 did not answer"));
        assert!(reported.contains("kobo devices"));
        assert!(reported.contains("asleep"));
    }

    #[test]
    fn app_names_are_shell_safe() {
        assert!(valid_slug("weather"));
        assert!(valid_slug("home-panel-2"));
        assert!(!valid_slug("../bad"));
        assert!(!valid_slug("Bad"));
        assert!(!valid_slug("bad;rm"));
    }

    #[test]
    fn rejects_non_elf_binary() {
        let path = std::env::temp_dir().join(format!("kobo-cli-not-elf-{}", std::process::id()));
        fs::write(&path, b"not an elf").expect("write fixture");
        assert!(verify_arm_elf(&path).is_err());
        fs::remove_file(path).expect("remove fixture");
    }

    #[test]
    fn every_uploaded_artifact_is_built_from_this_workspace() {
        let command =
            super::device_build_command("kobo-doctor", None).expect("create doctor build command");
        let arguments = command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            arguments,
            [
                "build",
                "--release",
                "--locked",
                "--manifest-path",
                &super::workspace_manifest().display().to_string(),
                "--target",
                "armv7-unknown-linux-musleabihf",
                "-p",
                "kobo-doctor",
                "--bin",
                "kobo-doctor",
            ]
        );
    }

    #[test]
    fn every_installed_package_is_a_member_of_this_workspace() {
        let manifest = fs::read_to_string(super::workspace_manifest()).expect("read the workspace");
        for (name, _) in super::INSTALLED_PACKAGES {
            let directory = if *name == "kobod" {
                "crates/kobod".to_owned()
            } else {
                format!("examples/{}", name.trim_start_matches("kobo-"))
            };
            assert!(
                manifest.contains(&format!("\"{directory}\"")),
                "{name} is packaged but {directory} is not a workspace member"
            );
        }
    }

    /// The daemon shipped in the package was built without `device-write` for
    /// as long as the packager existed, so `--present` was not compiled in and
    /// `start.sh` answered the owner with a usage message. Everything else
    /// about that binary was correct, which is why nothing else caught it.
    #[test]
    fn every_packaged_binary_is_built_with_what_it_needs() {
        let features = super::INSTALLED_PACKAGES
            .iter()
            .find(|(name, _)| *name == "kobod")
            .map(|(_, features)| *features)
            .expect("kobod is packaged");
        assert_eq!(
            features,
            Some("device-write"),
            "a kobod without device-write cannot take the panel, and start.sh is \
             the only thing in the package an owner runs"
        );
        let command = super::device_build_command("kobod", features).expect("build command");
        let arguments = command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(
            arguments.windows(2).any(|pair| pair[0] == "--features"
                && pair[1].split(',').any(|one| one == "device-write")),
            "the build command dropped the feature: {arguments:?}"
        );
    }

    #[test]
    fn a_daemon_without_the_panel_session_is_refused() {
        let path = std::path::Path::new("target/kobod");
        super::verify_present_is_compiled_in(b"nothing useful in here", path)
            .expect_err("a binary without the unlock phrase is not shippable");
        let mut bytes = b"padding".to_vec();
        bytes.extend_from_slice(super::PRESENT_UNLOCK_PHRASE);
        bytes.extend_from_slice(b"more padding");
        super::verify_present_is_compiled_in(&bytes, path)
            .expect("a binary carrying the unlock phrase is shippable");
    }

    #[test]
    fn a_package_writes_nothing_outside_the_install_root() {
        // The archive is extracted as root by the device's own boot script, so
        // the check that matters is what it is *able* to write.
        let members = vec![
            super::text_member("start.sh", super::START_SCRIPT, true),
            super::text_member("README.txt", super::INSTALL_README, false),
        ];
        let archive = package::tar(&members).expect("build the archive");
        let root = format!("{}/", package::INSTALL_ROOT);
        for entry in package::list(&archive).expect("read the archive back") {
            assert!(
                entry.path.starts_with(&root) || root.starts_with(entry.path.trim_end_matches('/')),
                "{} is outside the install root",
                entry.path
            );
        }
    }

    #[test]
    fn a_package_survives_the_check_the_device_runs_first() {
        let members = vec![super::text_member("VERSION", "0.1.0\n", false)];
        let archive = package::tar(&members).expect("build the archive");
        let compressed = super::gzip(&archive).expect("compress");
        super::gzip_test(&compressed).expect("the device would accept this");
        assert_eq!(
            super::gunzip(&compressed).expect("decompress"),
            archive,
            "compression must round-trip exactly"
        );
        assert_eq!(
            super::gzip(&archive).expect("compress again"),
            compressed,
            "the same input must produce the same file, or a checksum means nothing"
        );
    }

    #[test]
    fn the_start_script_points_at_the_folder_the_package_writes() {
        assert!(super::START_SCRIPT.contains(&format!("/{}", package::INSTALL_ROOT)));
        assert!(super::INSTALL_README.contains(&format!("/{}", package::INSTALL_ROOT)));
    }

    #[test]
    fn package_options_are_parsed_and_unknown_ones_refused() {
        let (tarball, folder) = super::parse_package(&[]).expect("defaults");
        assert_eq!(tarball, PathBuf::from("target/KoboRoot.tgz"));
        assert!(folder.is_none());
        let (tarball, folder) = super::parse_package(&[
            "--out".to_owned(),
            "/tmp/a.tgz".to_owned(),
            "--folder".to_owned(),
            "/tmp/b".to_owned(),
        ])
        .expect("explicit paths");
        assert_eq!(tarball, PathBuf::from("/tmp/a.tgz"));
        assert_eq!(folder, Some(PathBuf::from("/tmp/b")));
        assert!(super::parse_package(&["--onto".to_owned()]).is_err());
    }

    #[test]
    fn generated_app_uses_the_sdk_event_loop() {
        assert!(GENERATED_APP_SOURCE.contains("AppRunner::new(Hello::default())"));
        assert!(GENERATED_APP_SOURCE.contains("Client::connect(socket"));
        assert!(GENERATED_APP_SOURCE.contains("client.next_event()"));
        assert!(GENERATED_APP_SOURCE.contains("runner.action(action)"));
        // A generated app must show how hardware is asked for and how every
        // answer, including a refusal, reaches the application.
        assert!(GENERATED_APP_SOURCE.contains("context.device().read_battery()"));
        assert!(GENERATED_APP_SOURCE.contains("fn on_device_result"));
        assert!(GENERATED_APP_SOURCE.contains("runner.device_result(result)"));
        assert!(GENERATED_APP_SOURCE.contains("DeviceResult::Denied(reason)"));
        // It must never reach hardware itself.
        assert!(!GENERATED_APP_SOURCE.contains("/dev/"));
        assert!(!GENERATED_APP_SOURCE.contains("/sys/"));
    }

    #[test]
    fn detects_sdk_application_manifests() {
        assert!(manifest_uses_sdk(
            "[dependencies]\nkobo-sdk = { path = \"../kobo-sdk\" }"
        ));
        assert!(manifest_uses_sdk(
            "[dependencies]\nkobo-sdk.workspace = true"
        ));
        assert!(!manifest_uses_sdk("[dependencies]\nkobo-ui = \"0.1\""));
    }

    #[test]
    fn finds_executable_from_cargo_build_output() {
        let output = concat!(
            r#"{"reason":"compiler-artifact","target":{"kind":["lib"]},"executable":null}"#,
            "\n",
            r#"{"reason":"compiler-artifact","target":{"kind":["bin"]},"executable":"/apps/hello/target/debug/hello"}"#
        );
        assert_eq!(
            build_executables(output),
            vec![std::path::PathBuf::from("/apps/hello/target/debug/hello")]
        );
    }

    #[test]
    fn simulation_guard_removes_private_artifacts() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(format!(".simulation-cleanup-{}", std::process::id()));
        let guard = SimulationGuard::new_at(root.clone()).expect("create simulation guard");
        fs::write(&guard.socket, b"socket").expect("write socket fixture");
        fs::write(&guard.frame, b"frame").expect("write frame fixture");
        drop(guard);
        assert!(!root.exists());
    }

    #[test]
    fn dev_session_guard_removes_private_artifacts() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(format!(".dev-cleanup-{}", std::process::id()));
        let guard = DevSessionGuard::new_at(root.clone()).expect("create development session");
        fs::write(&guard.socket, b"socket").expect("write socket fixture");
        drop(guard);
        assert!(!root.exists());
    }

    #[test]
    fn default_device_build_excludes_guard_and_smoke() {
        assert_eq!(
            DEVICE_PACKAGES,
            ["kobo-doctor", "kobod", "kobo-todo", "kobo-terminal"]
        );
        assert!(!DEVICE_PACKAGES.contains(&"kobo-guard"));
        assert!(!DEVICE_PACKAGES.contains(&"kobo-smoke"));
    }

    #[cfg(feature = "device-write")]
    #[test]
    fn the_guard_test_needs_the_exact_confirmation_and_a_clean_host() {
        let arguments = |parts: &[&str]| {
            parts
                .iter()
                .map(|part| (*part).to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            parse_guard_test(&arguments(&[
                "--device",
                "192.168.1.15",
                "--confirm",
                GUARD_TEST_CONFIRMATION
            ])),
            Ok("192.168.1.15")
        );
        for rejected in [
            vec!["--device", "192.168.1.15", "--confirm", "GUARD_RESTORE"],
            vec!["--device", "192.168.1.15", "--confirm", ""],
            vec!["--device", "192.168.1.15"],
            vec![
                "--device",
                "192.168.1.15; reboot",
                "--confirm",
                GUARD_TEST_CONFIRMATION,
            ],
        ] {
            assert!(
                parse_guard_test(&arguments(&rejected)).is_err(),
                "{rejected:?} must be refused"
            );
        }
    }

    #[cfg(feature = "device-write")]
    #[test]
    fn the_guard_artifact_is_built_from_this_workspace_and_never_the_default_device_set() {
        let artifact = RemoteArtifact::guard();
        assert_eq!(artifact.package, "kobo-guard");
        assert_eq!(artifact.features, Some("device-write"));
        assert!(artifact
            .local_binary
            .ends_with("armv7-unknown-linux-musleabihf/release/kobo-guard"));
        // The child is an exact absolute path, never resolved through PATH.
        assert!(GUARD_TEST_CHILD.starts_with('/'));
    }

    #[cfg(feature = "device-write")]
    #[test]
    fn existing_run_command_cannot_invoke_the_smoke_binary() {
        let error = run(&["run".to_owned(), "kobo-smoke".to_owned()]).expect_err("run is gated");
        assert!(error.contains("device execution is safety-gated"));
    }

    #[test]
    fn remote_doctor_uses_strict_hosts_and_workspace_artifact() {
        assert!(valid_device_host("192.0.2.1"));
        assert!(valid_device_host("kobo-reader_1"));
        assert!(!valid_device_host(""));
        assert!(!valid_device_host("reader;reboot"));
        assert!(!valid_device_host("reader name"));
        assert_eq!(
            workspace_doctor_binary(),
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join("target/armv7-unknown-linux-musleabihf/release/kobo-doctor")
        );
        #[cfg(feature = "device-write")]
        assert_eq!(
            workspace_smoke_binary(),
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join("target/armv7-unknown-linux-musleabihf/release/kobo-smoke")
        );
    }

    #[cfg(feature = "device-write")]
    #[test]
    fn smoke_confirmation_is_exact_and_has_no_arbitrary_arguments() {
        let exact = [
            "--device".to_owned(),
            "192.0.2.1".to_owned(),
            "--confirm".to_owned(),
            "DISPLAY_ONLY_GC16".to_owned(),
        ];
        assert_eq!(
            parse_smoke_display(&exact),
            Ok(("192.0.2.1", SmokeStage::DisplayOnly))
        );
        let reversible = [
            "--device".to_owned(),
            "192.0.2.1".to_owned(),
            "--confirm".to_owned(),
            "REVERSIBLE_PIXELS_GC16".to_owned(),
        ];
        assert_eq!(
            parse_smoke_display(&reversible),
            Ok(("192.0.2.1", SmokeStage::ReversiblePixels))
        );
        for invalid in [
            vec![],
            vec!["--device", "192.0.2.1", "--confirm", "display_only_gc16"],
            vec!["--device", "192.0.2.1", "--confirm", "FULL_SCREEN_GC16"],
            vec![
                "--device",
                "192.0.2.1",
                "--confirm",
                "DISPLAY_ONLY_GC16",
                "--extra",
            ],
            vec![
                "--device",
                "reader;reboot",
                "--confirm",
                "DISPLAY_ONLY_GC16",
            ],
        ] {
            let invalid = invalid.into_iter().map(str::to_owned).collect::<Vec<_>>();
            assert!(parse_smoke_display(&invalid).is_err());
        }
    }

    #[test]
    fn dev_session_parsing_is_exact_and_host_checked() {
        use super::{devsession::Switch, DevSessionAction};
        let base = ["--device".to_owned(), "192.0.2.1".to_owned()];
        let parse = |extra: &[&str]| {
            let mut arguments = base.to_vec();
            arguments.extend(extra.iter().map(|value| (*value).to_owned()));
            super::parse_dev_session(&arguments).map(|(host, action)| (host.to_owned(), action))
        };
        assert_eq!(
            parse(&[]),
            Ok(("192.0.2.1".to_owned(), DevSessionAction::Status))
        );
        assert_eq!(
            parse(&["--status"]),
            Ok(("192.0.2.1".to_owned(), DevSessionAction::Status))
        );
        assert_eq!(
            parse(&["--keep-awake", "on"]),
            Ok((
                "192.0.2.1".to_owned(),
                DevSessionAction::KeepAwake(Switch::On)
            ))
        );
        assert_eq!(
            parse(&["--wifi-always-on", "off"]),
            Ok((
                "192.0.2.1".to_owned(),
                DevSessionAction::WifiAlwaysOn(Switch::Off)
            ))
        );
        assert_eq!(
            parse(&["--restore-reader-config"]),
            Ok(("192.0.2.1".to_owned(), DevSessionAction::RestoreConfig))
        );
        for invalid in [
            vec!["--keep-awake"],
            vec!["--keep-awake", "yes"],
            vec!["--wifi-always-on", "1"],
            vec!["--unknown"],
            vec!["--status", "--keep-awake", "on"],
        ] {
            assert!(parse(&invalid).is_err(), "{invalid:?} must be rejected");
        }
        let hostile = [
            "--device".to_owned(),
            "reader;reboot".to_owned(),
            "--status".to_owned(),
        ];
        assert!(super::parse_dev_session(&hostile).is_err());
        assert!(super::parse_dev_session(&[]).is_err());
    }

    #[cfg(feature = "device-write")]
    #[test]
    fn each_stage_maps_to_exactly_one_device_unlock() {
        assert_eq!(
            SmokeStage::DisplayOnly.device_unlock(),
            "OWNER_ATTENDED_DISPLAY_ONLY_GC16"
        );
        assert_eq!(
            SmokeStage::ReversiblePixels.device_unlock(),
            "OWNER_ATTENDED_REVERSIBLE_PIXELS_GC16"
        );
    }

    #[cfg(feature = "device-write")]
    #[test]
    fn smoke_build_is_pinned_to_this_workspace_and_feature_targeted() {
        let command = super::device_build_command("kobo-smoke", Some("device-write"))
            .expect("create smoke build command");
        let arguments = command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            arguments,
            [
                "build",
                "--release",
                "--locked",
                "--manifest-path",
                &super::workspace_manifest().display().to_string(),
                "--target",
                "armv7-unknown-linux-musleabihf",
                "-p",
                "kobo-smoke",
                "--bin",
                "kobo-smoke",
                "--features",
                "device-write",
            ]
        );
        assert!(super::workspace_manifest().is_file());
    }

    #[cfg(feature = "device-write")]
    #[test]
    fn remote_session_uses_stdin_only_and_fixed_safe_artifacts() {
        let ssh = super::remote_shell_command("root@192.0.2.1");
        let ssh_args = ssh
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            ssh_args,
            [
                "-T",
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=10",
                "root@192.0.2.1"
            ]
        );
        assert_eq!(ssh_args.last().expect("remote host"), "root@192.0.2.1");
        let checksum = "a".repeat(64);
        let encoded = super::base64_encode(b"fixed artifact");
        let session = RemoteArtifactSession {
            directory: "/tmp/kobo-smoke-123-456".to_owned(),
            binary: "/tmp/kobo-smoke-123-456/kobo-smoke".to_owned(),
            owner_file: "/tmp/kobo-smoke-123-456/.kobo-smoke-owner".to_owned(),
            owner_token: "0123456789abcdef0123456789abcdef".to_owned(),
        };
        let script = super::remote_fixed_artifact_script(
            &session,
            RemoteProgram::Smoke(SmokeStage::DisplayOnly),
            &checksum,
            &encoded,
        );
        assert!(script.starts_with("set -eu\numask 077\n"));
        assert!(script.contains("mkdir -m 700 \"$dir\""));
        assert!(script.contains("trap cleanup EXIT HUP INT TERM"));
        assert!(script.contains("IFS= read -r actual < \"$owner\""));
        assert!(script.contains("[ \"$actual\" = \"$token\" ]"));
        assert!(
            script.find("mkdir -m 700").expect("mkdir")
                < script.find("trap cleanup").expect("trap")
        );
        assert!(script.contains("rm -f \"$bin\""));
        assert!(script.contains("rmdir \"$dir\""));
        assert!(script.contains("base64 -d > \"$bin\" <<'KOBO_ARTIFACT_BASE64'"));
        assert!(script.contains(&encoded));
        assert!(script.contains(&checksum));
        assert!(script.contains("KOBO_SMOKE_UNLOCK='OWNER_ATTENDED_DISPLAY_ONLY_GC16'"));
        assert!(script.contains("[ -x /usr/bin/timeout ]"));
        assert!(script.contains("/usr/bin/timeout 25 \"$bin\""));
        assert!(script.contains("refusing display smoke"));
        assert!(!script.contains("192.0.2.1"));
        assert!(!script.contains("scp"));
        assert!(!script.contains("reader;reboot"));
        assert!(script.ends_with("exit\n"));
        let cleanup = super::remote_cleanup_script(&session);
        assert!(cleanup.contains("IFS= read -r actual < \"$owner\""));
        assert!(cleanup.contains("[ \"$actual\" = \"$token\" ]"));
        assert!(cleanup.contains("rm -f \"$bin\" \"$owner\""));
        assert!(cleanup.contains("rmdir \"$dir\""));
        assert_eq!(REMOTE_CONNECT_TIMEOUT_SECONDS, 10);
        assert_eq!(REMOTE_COMMAND_TIMEOUT.as_secs(), 60);
        assert_eq!(REMOTE_CLEANUP_TIMEOUT.as_secs(), 5);
        #[cfg(feature = "device-write")]
        {
            assert_eq!(REMOTE_SMOKE_TIMEOUT_SECONDS, 25);
            assert!(REMOTE_COMMAND_TIMEOUT.as_secs() > REMOTE_SMOKE_TIMEOUT_SECONDS);
        }
        let token = super::remote_owner_token().expect("ownership token");
        assert_eq!(token.len(), 32);
        assert!(token.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }

    #[test]
    fn base64_round_trips_artifact_bytes() {
        let bytes = (0_u8..58).collect::<Vec<_>>();
        let encoded = super::base64_encode(&bytes);
        assert!(encoded.contains('\n'));
        assert_eq!(decode_base64(&encoded), bytes);
    }

    #[test]
    fn remote_session_error_includes_captured_output() {
        let message = super::remote_shell_error(
            "remote doctor failed".to_owned(),
            b"doctor stdout",
            b"doctor stderr",
        );
        assert!(message.contains("stdout: doctor stdout"));
        assert!(message.contains("stderr: doctor stderr"));
    }

    #[test]
    fn remote_child_timeout_kills_the_local_process() {
        let mut child = Command::new("/bin/sleep")
            .arg("5")
            .spawn()
            .expect("start local sleep");
        let error =
            wait_for_remote_child(&mut child, "test remote command", Duration::from_millis(1))
                .expect_err("timeout");
        assert!(error.contains("timed out"));
        assert!(child.try_wait().expect("inspect child").is_some());
    }

    fn decode_base64(value: &str) -> Vec<u8> {
        fn digit(value: u8) -> u8 {
            match value {
                b'A'..=b'Z' => value - b'A',
                b'a'..=b'z' => value - b'a' + 26,
                b'0'..=b'9' => value - b'0' + 52,
                b'+' => 62,
                b'/' => 63,
                _ => panic!("invalid base64 byte"),
            }
        }

        let compact = value
            .bytes()
            .filter(|byte| !byte.is_ascii_whitespace())
            .collect::<Vec<_>>();
        let mut decoded = Vec::new();
        for chunk in compact.chunks_exact(4) {
            let padding = usize::from(chunk[2] == b'=') + usize::from(chunk[3] == b'=');
            let value = (u32::from(digit(chunk[0])) << 18)
                | (u32::from(digit(chunk[1])) << 12)
                | (u32::from(if chunk[2] == b'=' { 0 } else { digit(chunk[2]) }) << 6)
                | u32::from(if chunk[3] == b'=' { 0 } else { digit(chunk[3]) });
            let bytes = value.to_be_bytes();
            decoded.push(bytes[1]);
            if padding < 2 {
                decoded.push(bytes[2]);
            }
            if padding == 0 {
                decoded.push(bytes[3]);
            }
        }
        decoded
    }

    mod holding {
        use super::super::{parse_dev_session, DevSessionAction, HOLD_MAXIMUM_MINUTES};

        fn arguments(values: &[&str]) -> Vec<String> {
            values.iter().map(|value| (*value).to_owned()).collect()
        }

        #[test]
        fn defaults_to_thirty_minutes() {
            let given = arguments(&["--device", "192.0.2.1", "--hold"]);
            assert_eq!(
                parse_dev_session(&given).expect("parse").1,
                DevSessionAction::Hold(30)
            );
        }

        #[test]
        fn accepts_an_explicit_duration() {
            let given = arguments(&["--device", "192.0.2.1", "--hold", "90"]);
            assert_eq!(
                parse_dev_session(&given).expect("parse").1,
                DevSessionAction::Hold(90)
            );
        }

        #[test]
        fn refuses_a_zero_or_unbounded_hold() {
            // A hold must always end by itself, so it can never be forgotten.
            let zero = arguments(&["--device", "192.0.2.1", "--hold", "0"]);
            assert!(parse_dev_session(&zero).is_err());
            let too_long = (HOLD_MAXIMUM_MINUTES + 1).to_string();
            let over = arguments(&["--device", "192.0.2.1", "--hold", &too_long]);
            assert!(parse_dev_session(&over).is_err());
            let words = arguments(&["--device", "192.0.2.1", "--hold", "forever"]);
            assert!(parse_dev_session(&words).is_err());
        }
    }

    mod change_counting {
        use super::super::changed_lines;

        #[test]
        fn reads_the_reported_count() {
            assert_eq!(
                changed_lines(b"applied; changed_lines=3\nforce_wifi_on: true\n"),
                3
            );
            assert_eq!(changed_lines(b"applied; changed_lines=0\n"), 0);
        }

        #[test]
        fn treats_anything_unreadable_as_no_change() {
            // Advice may only be suppressed by this, never invented.
            assert_eq!(changed_lines(b""), 0);
            assert_eq!(changed_lines(b"applied; changed_lines=lots\n"), 0);
            assert_eq!(changed_lines(b"something else entirely\n"), 0);
            assert_eq!(changed_lines(&[0xff, 0xfe, 0x00]), 0);
        }
    }

    mod waiting {
        use super::super::{parse_wait, DEVICE_WAIT_MAXIMUM_SECONDS};

        fn arguments(values: &[&str]) -> Vec<String> {
            values.iter().map(|value| (*value).to_owned()).collect()
        }

        #[test]
        fn defaults_to_five_minutes() {
            let given = arguments(&["--device", "192.0.2.1"]);
            let parsed = parse_wait(&given).expect("parse");
            assert_eq!(parsed.0, "192.0.2.1");
            assert_eq!(parsed.1.as_secs(), 300);
        }

        #[test]
        fn accepts_an_explicit_timeout() {
            let given = arguments(&["--device", "192.0.2.1", "--timeout", "90"]);
            let parsed = parse_wait(&given).expect("parse");
            assert_eq!(parsed.1.as_secs(), 90);
        }

        #[test]
        fn refuses_a_zero_or_unbounded_wait() {
            let zero = arguments(&["--device", "192.0.2.1", "--timeout", "0"]);
            assert!(parse_wait(&zero).is_err());
            let too_long = (DEVICE_WAIT_MAXIMUM_SECONDS + 1).to_string();
            let over = arguments(&["--device", "192.0.2.1", "--timeout", &too_long]);
            assert!(parse_wait(&over).is_err());
        }

        #[test]
        fn refuses_an_unsafe_host() {
            let given = arguments(&["--device", "192.0.2.1; rm -rf /"]);
            assert!(parse_wait(&given).is_err());
        }

        #[test]
        fn refuses_unknown_flags() {
            let given = arguments(&["--device", "192.0.2.1", "--forever"]);
            assert!(parse_wait(&given).is_err());
        }
    }
}

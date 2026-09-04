use std::collections::BTreeSet;
use std::env;
use std::ffi::OsStr;
use std::fmt::Write as _;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

mod authorize;
mod connect;
mod devsession;
mod drive;
mod host_release;
mod menu;
mod package;
// Only the `device-write` build dispatches to this, but its tests decide what
// gets sent to a reader and are worth running on every build. So it compiles
// either way, and the unused warning is silenced rather than the module gated
// out and its tests with it.
#[cfg_attr(not(feature = "device-write"), allow(dead_code))]
mod panel;
mod setup;
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
/// `start.sh` (the only thing in the package an owner runs) fails with a usage
/// message. That is exactly what shipped until an installed package was run on
/// a real device, so `every_packaged_binary_is_built_with_what_it_needs` and
/// the artifact check in `build_package` both exist to keep it shipped.
const INSTALLED_PACKAGES: &[(&str, Option<&str>)] = &[
    ("kobod", Some("device-write")),
    ("kobo-launcher", None),
    ("kobo-audiobook", None),
    ("kobo-terminal", None),
    ("kobo-todo", None),
    ("kobo-brief", None),
    ("kobo-chat", None),
    ("kobo-gutenbird", None),
    ("kobo-gallery", None),
    ("kobo-tictactoe", None),
    ("kobo-magnet", None),
    ("kobo-hn", None),
    ("kobo-rss", None),
    ("kobo-settings", None),
    ("kobo-sidekick", None),
    ("kobo-store", None),
];
/// Applications released through Store, including the initial built-in copies
/// that users can update, remove and reinstall independently of Cobalt.
const STORE_PACKAGES: &[&str] = &[
    "kobo-arxiv",
    "kobo-audiobook",
    "kobo-backgammon",
    "kobo-brief",
    "kobo-chat",
    "kobo-gallery",
    "kobo-gutenbird",
    "kobo-hn",
    "kobo-magnet",
    "kobo-morse",
    "kobo-rss",
    "kobo-sidekick",
    "kobo-sudoku",
    "kobo-tictactoe",
    "kobo-todo",
    "kobo-paperterm",
    "kobo-zotero-reader",
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
# `kobo setup --enable-ssh` leaves only a public key here. The reader's
# root-owned menu action can finish the step a USB volume cannot.
staged_key=\"$root/bootstrap/authorized_key\"
if [ -s \"$staged_key\" ]; then
  umask 077
  # Root's home is not /root on every Kobo: the i.MX6 firmware ships
  # root:...:0:0:root:/:/bin/sh, and sshd resolves AuthorizedKeysFile
  # relative to the home directory, so a key under /root never authenticates
  # there. Ask /etc/passwd instead of assuming.
  home=$(awk -F: '$1 == \"root\" { print $6 }' /etc/passwd)
  home=\"${home%/}\"
  keys=\"$home/.ssh/authorized_keys\"
  mkdir -p \"$home/.ssh\"
  touch \"$keys\"
  chmod 700 \"$home/.ssh\"
  chmod 600 \"$keys\"
  key=$(head -n 1 \"$staged_key\")
  found=false
  while IFS= read -r known; do
    if [ \"$known\" = \"$key\" ]; then
      found=true
      break
    fi
  done < \"$keys\"
  if [ \"$found\" = false ]; then
    printf '%s\\n' \"$key\" >> \"$keys\"
  fi
  rm -f \"$staged_key\"
  sync
fi
# The terminal opens a pty: ptsname names /dev/pts/N and the child opens it.
# Some Kobo firmware does not mount devpts, so without this every terminal is
# refused with Failed. A kernel mount, not a write to the root filesystem, and
# gone again at the next reboot.
if ! grep -q ' /dev/pts ' /proc/mounts 2>/dev/null; then
  mkdir -p /dev/pts 2>/dev/null &&
    mount -t devpts devpts /dev/pts -o mode=0620,ptmxmode=0666 || true
fi
KOBO_PRESENT_UNLOCK=OWNER_ATTENDED_PANEL_SESSION \\
  exec \"$root/bin/kobod\" --present \"$root/bin/kobo-launcher\" > /mnt/onboard/kobod.txt 2>&1
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
    WaitTiming,
}

fn confirmation_answer(answer: &str) -> bool {
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

#[cfg(feature = "device-write")]
impl SmokeStage {
    const CONFIRM_DISPLAY_ONLY: &'static str = "DISPLAY_ONLY_GC16";
    const CONFIRM_REVERSIBLE_PIXELS: &'static str = "REVERSIBLE_PIXELS_GC16";
    const CONFIRM_SCREEN_SNAPSHOT: &'static str = "SCREEN_SNAPSHOT_RESTORE";
    const CONFIRM_FAST_FEEDBACK: &'static str = "REVERSIBLE_PIXELS_DU";
    const CONFIRM_WAIT_TIMING: &'static str = "WAIT_TIMING_GC16_DU";

    /// Every stage, so the usage text can never drift from what is accepted.
    const ALL: [Self; 5] = [
        Self::DisplayOnly,
        Self::ReversiblePixels,
        Self::ScreenSnapshot,
        Self::FastFeedback,
        Self::WaitTiming,
    ];

    const fn confirmation(self) -> &'static str {
        match self {
            Self::DisplayOnly => Self::CONFIRM_DISPLAY_ONLY,
            Self::ReversiblePixels => Self::CONFIRM_REVERSIBLE_PIXELS,
            Self::ScreenSnapshot => Self::CONFIRM_SCREEN_SNAPSHOT,
            Self::FastFeedback => Self::CONFIRM_FAST_FEEDBACK,
            Self::WaitTiming => Self::CONFIRM_WAIT_TIMING,
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
            Self::CONFIRM_WAIT_TIMING => Some(Self::WaitTiming),
            _ => None,
        }
    }

    fn device_unlock(self) -> &'static str {
        match self {
            Self::DisplayOnly => "OWNER_ATTENDED_DISPLAY_ONLY_GC16",
            Self::ReversiblePixels => "OWNER_ATTENDED_REVERSIBLE_PIXELS_GC16",
            Self::ScreenSnapshot => "OWNER_ATTENDED_SCREEN_SNAPSHOT_RESTORE",
            Self::FastFeedback => "OWNER_ATTENDED_REVERSIBLE_PIXELS_DU",
            Self::WaitTiming => "OWNER_ATTENDED_WAIT_TIMING_GC16_DU",
        }
    }
}

fn main() -> ExitCode {
    let arguments = env::args().skip(1).collect::<Vec<_>>();
    // One verb decides its own exit code. Every other command either worked or
    // did not, and flattening the reader's status to "something failed" would
    // make `kobo shell` the one thing here that cannot be tested for in a
    // script.
    if arguments.first().map(|command| canonical(command)) == Some("shell") {
        return match shell_command(&arguments[1..]) {
            Ok(code) => code,
            Err(error) => {
                eprintln!("kobo: {error}");
                ExitCode::FAILURE
            }
        };
    }
    match run(&arguments) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("kobo: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Other names for commands that already exist.
///
/// Nobody arrives here without habits. Somebody who has shipped an Android or
/// iOS application has `adb logcat`, `adb install` and `adb wait-for-device`
/// in their fingers, and a tool that answers "unknown command" to those is
/// spending the reader's goodwill on nothing: the concepts are the same and
/// only the spelling differs. These map onto the canonical name rather than
/// duplicating it, so there is still one implementation and one help entry.
const ALIASES: &[(&str, &str)] = &[
    ("logcat", "logs"),
    ("sh", "shell"),
    ("install", "deploy"),
    ("wait-for-device", "wait"),
    ("simulator", "dev"),
    ("sim", "dev"),
    ("init", "new"),
    ("create", "new"),
];

/// Whether an argument names the device to act on.
///
/// `-s` because that is what `adb` calls it and the muscle memory is real;
/// `--device` because that is what this tool called it first and what every
/// example still says.
fn is_device_flag(argument: &str) -> bool {
    argument == "--device" || argument == "-s"
}

/// Resolves an alias to the command it stands for.
fn canonical(command: &str) -> &str {
    ALIASES
        .iter()
        .find_map(|(alias, name)| (*alias == command).then_some(*name))
        .unwrap_or(command)
}

fn run(arguments: &[String]) -> Result<(), String> {
    let Some(command) = arguments.first().map(String::as_str) else {
        print_help();
        return Ok(());
    };
    match canonical(command) {
        "new" => create_app(arguments.get(1).ok_or("usage: kobo new <name>")?),
        "dev" => dev(&arguments[1..]),
        "drive" => drive_command(&arguments[1..]),
        "shot" => shot_command(&arguments[1..]),
        #[cfg(feature = "device-write")]
        "tap" => tap_command(&arguments[1..]),
        #[cfg(feature = "device-write")]
        "present" => panel::present(&arguments[1..]),
        #[cfg(feature = "device-write")]
        "stop" => panel::stop(&arguments[1..]),
        #[cfg(not(feature = "device-write"))]
        "present" | "stop" => Err(format!(
            "{command} takes the panel, so it is not compiled in; rebuild the CLI with \
             --features device-write"
        )),
        "build" => build_device(arguments.iter().any(|argument| is_device_flag(argument))),
        "doctor" => doctor(&arguments[1..]),
        "devices" => list_devices(&arguments[1..]),
        "app-link" => app_link_command(&arguments[1..]),
        "session" => dev_session(&arguments[1..]),
        "wait" => wait_for_device(&arguments[1..]),
        "logs" => device_logs(&arguments[1..]),
        // Reached only when something other than main dispatches, which today
        // is the tests. main takes this verb first so that the reader's own
        // exit code survives.
        "shell" => shell_command(&arguments[1..]).map(|_| ()),
        "touch-probe" => touch_probe(&arguments[1..]),
        "record" => record_command(&arguments[1..]),
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
        "app-key" => app_key(&arguments[1..]),
        "app-bundle" => app_bundle(&arguments[1..]),
        "app-catalog" => app_catalog(&arguments[1..]),
        "app-list" => app_list(&arguments[1..]),
        "stream" => stream_command(&arguments[1..]),
        "app-check" => app_check(&arguments[1..]),
        "app-release" => app_release(&arguments[1..]),
        "host-release-sign" => host_release_sign(&arguments[1..]),
        "host-release-verify" => host_release_verify(&arguments[1..]),
        "update" => update_host(&arguments[1..]),
        "setup" => setup_device(&arguments[1..]),
        "deploy" => deploy_package(&arguments[1..]),
        "secret" => secret_command(&arguments[1..]),
        "trust" => trust_command(&arguments[1..]),
        "inspect" => inspect_package(&arguments[1..]),
        "verify" => verify_command(&arguments[1..]),
        "run" if arguments.get(1).is_some_and(|value| value == "--sim") => {
            run_simulation(&arguments[2..])
        }
        "run" => {
            Err("device execution is safety-gated; use 'kobo run --sim' on the host".to_owned())
        }
        "help" | "--help" | "-h" => {
            print_help();
            Ok(())
        }
        "version" | "--version" | "-V" => {
            println!("kobo {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        unknown => Err(format!("unknown command '{unknown}'")),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HostUpdateChannel {
    Stable,
    Beta,
}

impl HostUpdateChannel {
    const fn name(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Beta => "beta",
        }
    }
}

fn parse_host_update(arguments: &[String]) -> Result<HostUpdateChannel, String> {
    match arguments {
        [] => Ok(HostUpdateChannel::Stable),
        [flag, channel] if flag == "--channel" => match channel.as_str() {
            "stable" => Ok(HostUpdateChannel::Stable),
            "beta" => Ok(HostUpdateChannel::Beta),
            _ => Err("usage: kobo update [--channel stable|beta]".to_owned()),
        },
        _ => Err("usage: kobo update [--channel stable|beta]".to_owned()),
    }
}

fn update_host(arguments: &[String]) -> Result<(), String> {
    let channel = parse_host_update(arguments)?;
    let home = env::var_os("HOME").ok_or("HOME is not set")?;
    let data = env::var_os("XDG_DATA_HOME")
        .map_or_else(|| Path::new(&home).join(".local/share"), PathBuf::from);
    let root = data.join("kobo");
    let state_path = root.join("install-state");
    let state = fs::read_to_string(&state_path).map_err(|error| {
        format!(
            "kobo update requires a managed host installation (read {}: {error})",
            state_path.display()
        )
    })?;
    let binary = state
        .lines()
        .find_map(|line| line.strip_prefix("binary "))
        .map(PathBuf::from)
        .ok_or("managed installation state has no binary path")?;
    if !binary.exists() {
        return Err(format!(
            "managed kobo command is missing at {}",
            binary.display()
        ));
    }
    let host = managed_host_directory(&root)?;
    let current = env::current_exe()
        .and_then(|path| path.canonicalize())
        .map_err(|error| format!("locate running kobo: {error}"))?;
    let installed = host
        .join("kobo")
        .canonicalize()
        .map_err(|error| format!("locate selected host kobo: {error}"))?;
    if current != installed {
        return Err(
            "kobo update is available only from the host command installed by Cobalt; \
             source checkouts use git and cargo"
                .to_owned(),
        );
    }
    let updater = host.join("updater.sh");
    if !updater.is_file() {
        return Err(format!(
            "verified host updater is missing at {}; rerun the stable installer",
            updater.display()
        ));
    }
    let status = Command::new("sh")
        .arg(&updater)
        .args(["--host-update", "--channel", channel.name()])
        .status()
        .map_err(|error| format!("run verified host updater: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("host update exited with {status}"))
    }
}

fn managed_host_directory(root: &Path) -> Result<PathBuf, String> {
    let selector = root.join("current");
    let selected = fs::read_to_string(&selector)
        .map_err(|error| format!("read host selector {}: {error}", selector.display()))?;
    let selected = selected.trim();
    if selected.is_empty()
        || !selected
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err("managed host selector is invalid".to_owned());
    }
    Ok(root.join("hosts").join(selected))
}

fn host_release_sign(arguments: &[String]) -> Result<(), String> {
    const USAGE: &str = "usage: kobo host-release-sign --manifest PATH --seed PATH \
                         --signature PATH --ssh-signature PATH";
    let manifest_path = single_path_flag(arguments, "--manifest", USAGE)?;
    let seed_path = single_path_flag(arguments, "--seed", USAGE)?;
    let signature_path = single_path_flag(arguments, "--signature", USAGE)?;
    let ssh_signature_path = single_path_flag(arguments, "--ssh-signature", USAGE)?;
    ensure_only_flags(
        arguments,
        &["--manifest", "--seed", "--signature", "--ssh-signature"],
        USAGE,
    )?;
    let manifest = fs::read(&manifest_path)
        .map_err(|error| format!("read {}: {error}", manifest_path.display()))?;
    host_release::Manifest::parse(&manifest)?;
    let seed = read_signing_seed(&seed_path)?;
    let public = kobo_app_store::derive_public_key(&seed).map_err(|error| error.to_string())?;
    if public.to_hex() != kobo_app_store::PUBLIC_RELEASE_KEY_HEX {
        return Err("host release seed does not match the public release key".to_owned());
    }
    let (signature, ssh_signature) = host_release::sign(&manifest, &seed)?;
    fs::write(&signature_path, format!("{signature}\n"))
        .map_err(|error| format!("write {}: {error}", signature_path.display()))?;
    fs::write(&ssh_signature_path, ssh_signature)
        .map_err(|error| format!("write {}: {error}", ssh_signature_path.display()))?;
    println!("created {}", signature_path.display());
    println!("created {}", ssh_signature_path.display());
    Ok(())
}

fn host_release_verify(arguments: &[String]) -> Result<(), String> {
    const USAGE: &str = "usage: kobo host-release-verify --manifest PATH --signature PATH";
    let manifest_path = single_path_flag(arguments, "--manifest", USAGE)?;
    let signature_path = single_path_flag(arguments, "--signature", USAGE)?;
    ensure_only_flags(arguments, &["--manifest", "--signature"], USAGE)?;
    let manifest = fs::read(&manifest_path)
        .map_err(|error| format!("read {}: {error}", manifest_path.display()))?;
    let signature = fs::read_to_string(&signature_path)
        .map_err(|error| format!("read {}: {error}", signature_path.display()))?;
    let parsed = host_release::verify_manifest(&manifest, &signature)?;
    println!(
        "verified Cobalt {} host release from {}",
        parsed.version, parsed.source
    );
    Ok(())
}

/// Runs the host half of a Paperterm session.
fn stream_command(arguments: &[String]) -> Result<(), String> {
    const USAGE: &str = "usage: kobo stream init [--host ADDRESS ...]\n\
                         \x20      kobo stream [--grid COLSxROWS] [--controls | --interactive] \
                         [--read-only] [--port PORT] -- COMMAND [ARG ...]";
    if arguments.first().is_some_and(|argument| argument == "init") {
        return kobo_stream::init(&arguments[1..]);
    }
    let separator = arguments
        .iter()
        .position(|argument| argument == "--")
        .ok_or(USAGE)?;
    let mut grid = kobo_stream::Grid::fallback();
    let mut controls = false;
    let mut interactive = false;
    let mut port = kobo_stream::DEFAULT_PORT;
    let mut index = 0;
    while index < separator {
        match arguments[index].as_str() {
            "--grid" => {
                grid = kobo_stream::Grid::parse(arguments.get(index + 1).ok_or(USAGE)?)?;
                index += 2;
            }
            "--controls" => {
                controls = true;
                index += 1;
            }
            "--interactive" => {
                interactive = true;
                controls = true;
                index += 1;
            }
            "--read-only" => {
                controls = false;
                interactive = false;
                index += 1;
            }
            "--port" => {
                port = arguments
                    .get(index + 1)
                    .ok_or(USAGE)?
                    .parse::<u16>()
                    .map_err(|_| "--port must be 1 through 65535")?;
                if port == 0 {
                    return Err("--port must be 1 through 65535".to_owned());
                }
                index += 2;
            }
            _ => return Err(USAGE.to_owned()),
        }
    }
    kobo_stream::run(kobo_stream::Options {
        grid,
        controls,
        interactive,
        port,
        command: arguments[separator + 1..].to_vec(),
    })
    .map(|_| ())
}

fn app_key(arguments: &[String]) -> Result<(), String> {
    let seed_path = single_path_flag(arguments, "--seed", "usage: kobo app-key --seed PATH")?;
    let seed = read_signing_seed(&seed_path)?;
    let public = kobo_app_store::derive_public_key(&seed).map_err(|error| error.to_string())?;
    println!("{public}");
    Ok(())
}

fn app_bundle(arguments: &[String]) -> Result<(), String> {
    const USAGE: &str =
        "usage: kobo app-bundle --manifest PATH --binary PATH --seed PATH --out PATH";
    let manifest_path = single_path_flag(arguments, "--manifest", USAGE)?;
    let binary_path = single_path_flag(arguments, "--binary", USAGE)?;
    let seed_path = single_path_flag(arguments, "--seed", USAGE)?;
    let output = single_path_flag(arguments, "--out", USAGE)?;
    ensure_only_flags(
        arguments,
        &["--manifest", "--binary", "--seed", "--out"],
        USAGE,
    )?;

    let manifest_bytes = fs::read(&manifest_path)
        .map_err(|error| format!("read {}: {error}", manifest_path.display()))?;
    let manifest = kobo_app_store::Manifest::parse_public(&manifest_bytes)
        .map_err(|error| format!("invalid app manifest: {error}"))?;
    verify_arm_elf(&binary_path)?;
    let binary = fs::read(&binary_path)
        .map_err(|error| format!("read {}: {error}", binary_path.display()))?;
    let seed = read_signing_seed(&seed_path)?;
    let bundle = kobo_app_store::build_bundle(&manifest, &binary, &seed)
        .map_err(|error| format!("build app bundle: {error}"))?;
    fs::write(&output, bundle).map_err(|error| format!("write {}: {error}", output.display()))?;
    println!("created {}", output.display());
    Ok(())
}

fn app_catalog(arguments: &[String]) -> Result<(), String> {
    const USAGE: &str = "usage: kobo app-catalog --seed PATH --out PATH --signature PATH \
                         --entry PACKAGE HTTPS_URL [--entry PACKAGE HTTPS_URL ...]";
    let seed_path = single_path_flag(arguments, "--seed", USAGE)?;
    let output = single_path_flag(arguments, "--out", USAGE)?;
    let signature_output = single_path_flag(arguments, "--signature", USAGE)?;
    let entries = paired_flag(arguments, "--entry", USAGE)?;
    ensure_only_flags(
        arguments,
        &["--seed", "--out", "--signature", "--entry"],
        USAGE,
    )?;
    if entries.is_empty() {
        return Err(USAGE.to_owned());
    }

    let seed = read_signing_seed(&seed_path)?;
    let public = kobo_app_store::derive_public_key(&seed).map_err(|error| error.to_string())?;
    let mut catalog_entries = Vec::with_capacity(entries.len());
    for (package_path, url) in entries {
        let package_path = PathBuf::from(package_path);
        let package = fs::read(&package_path)
            .map_err(|error| format!("read {}: {error}", package_path.display()))?;
        let parsed = kobo_app_store::parse_public_bundle(&package, &public)
            .map_err(|error| format!("verify {}: {error}", package_path.display()))?;
        let package_bytes =
            u64::try_from(package.len()).map_err(|_| "app package is too large".to_owned())?;
        catalog_entries.push(
            kobo_app_store::CatalogEntry::new(kobo_app_store::CatalogEntryInput {
                manifest: parsed.manifest().clone(),
                package_url: url,
                package_sha256: kobo_net::sha256::hex_digest(&package),
                package_bytes,
            })
            .map_err(|error| format!("invalid catalog entry: {error}"))?,
        );
    }
    let catalog = kobo_app_store::Catalog::new(catalog_entries)
        .map_err(|error| format!("invalid app catalog: {error}"))?;
    let bytes = catalog.to_canonical_bytes();
    let signature =
        kobo_app_store::sign(&bytes, &seed).map_err(|error| format!("sign catalog: {error}"))?;
    fs::write(&output, &bytes).map_err(|error| format!("write {}: {error}", output.display()))?;
    fs::write(&signature_output, format!("{signature}\n"))
        .map_err(|error| format!("write {}: {error}", signature_output.display()))?;
    println!("created {}", output.display());
    println!("created {}", signature_output.display());
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ReleaseApp {
    package: String,
    id: String,
    display_name: String,
    short_label: String,
    summary: String,
    version: String,
    minimum_cobalt_version: String,
    glyph: String,
    capabilities: Vec<String>,
}

fn app_list(arguments: &[String]) -> Result<(), String> {
    const USAGE: &str = "usage: kobo app-list --registry PATH";
    let registry_path = single_path_flag(arguments, "--registry", USAGE)?;
    ensure_only_flags(arguments, &["--registry"], USAGE)?;
    let apps = read_release_registry(&registry_path)?;
    if apps.is_empty() {
        return Err("the app registry is empty".to_owned());
    }
    let packages = apps
        .iter()
        .map(|app| format!("\"{}\"", app.package))
        .collect::<Vec<_>>()
        .join(",");
    println!("[{packages}]");
    Ok(())
}

fn app_check(arguments: &[String]) -> Result<(), String> {
    const USAGE: &str = "usage: kobo app-check --registry PATH [--package PACKAGE] [--out PATH]";
    let registry_path = single_path_flag(arguments, "--registry", USAGE)?;
    let package = optional_value_flag(arguments, "--package", USAGE)?;
    let output = optional_path_flag(arguments, "--out", USAGE)?;
    ensure_only_flags(arguments, &["--registry", "--package", "--out"], USAGE)?;
    let mut apps = read_release_registry(&registry_path)?;
    if apps.is_empty() {
        return Err("the app registry is empty".to_owned());
    }
    if let Some(package) = package {
        apps.retain(|app| app.package == package);
        if apps.is_empty() {
            return Err(format!("package '{package}' is not registered"));
        }
    }
    if let Some(output) = &output {
        fs::create_dir_all(output)
            .map_err(|error| format!("create {}: {error}", output.display()))?;
    }
    for app in apps {
        let binary = build_release_binary(&app)?;
        if let Some(output) = &output {
            let path = output.join(&app.package);
            fs::write(&path, binary)
                .map_err(|error| format!("write {}: {error}", path.display()))?;
        }
        println!("verified {} ({})", app.id, app.package);
    }
    Ok(())
}

fn app_release(arguments: &[String]) -> Result<(), String> {
    const USAGE: &str = "usage: kobo app-release --registry PATH --seed PATH --out PATH \
                         --base-url HTTPS_URL [--prebuilt-dir PATH | --artifact-dir PATH]";
    let registry_path = single_path_flag(arguments, "--registry", USAGE)?;
    let seed_path = single_path_flag(arguments, "--seed", USAGE)?;
    let output = single_path_flag(arguments, "--out", USAGE)?;
    let base_url = single_value_flag(arguments, "--base-url", USAGE)?;
    let prebuilt = optional_path_flag(arguments, "--prebuilt-dir", USAGE)?;
    let artifacts = optional_path_flag(arguments, "--artifact-dir", USAGE)?;
    if prebuilt.is_some() && artifacts.is_some() {
        return Err(USAGE.to_owned());
    }
    ensure_only_flags(
        arguments,
        &[
            "--registry",
            "--seed",
            "--out",
            "--base-url",
            "--prebuilt-dir",
            "--artifact-dir",
        ],
        USAGE,
    )?;
    if !base_url.starts_with("https://") {
        return Err("--base-url must use HTTPS".to_owned());
    }
    let base_url = base_url.trim_end_matches('/');
    let apps = read_release_registry(&registry_path)?;
    if apps.is_empty() {
        return Err("the app registry is empty".to_owned());
    }
    if let Some(directory) = &prebuilt {
        validate_prebuilt_directory(&apps, directory)?;
    }
    if let Some(directory) = &artifacts {
        validate_artifact_directory(&apps, directory)?;
    }
    let seed = read_signing_seed(&seed_path)?;
    let public = kobo_app_store::derive_public_key(&seed).map_err(|error| error.to_string())?;
    if public.to_string() != kobo_app_store::PUBLIC_RELEASE_KEY_HEX {
        return Err(
            "the signing seed does not match the public key trusted by Cobalt runtimes".to_owned(),
        );
    }
    fs::create_dir_all(&output).map_err(|error| format!("create {}: {error}", output.display()))?;

    let mut entries = Vec::with_capacity(apps.len());
    for app in apps {
        let binary = match &prebuilt {
            Some(directory) => read_release_binary_from(&app, directory)?,
            None => match &artifacts {
                Some(directory) => read_release_artifact(&app, directory)?,
                None => build_release_binary(&app)?,
            },
        };
        let manifest = kobo_app_store::Manifest::new_public(kobo_app_store::ManifestInput {
            id: app.id.clone(),
            display_name: app.display_name,
            short_label: app.short_label,
            summary: app.summary,
            version: app.version,
            minimum_cobalt_version: app.minimum_cobalt_version,
            glyph: app.glyph,
            capabilities: app.capabilities,
            binary_sha256: kobo_net::sha256::hex_digest(&binary),
            binary_bytes: u64::try_from(binary.len())
                .map_err(|_| format!("{} binary is too large", app.package))?,
        })
        .map_err(|error| format!("invalid {} metadata: {error}", app.id))?;
        let bundle = kobo_app_store::build_bundle(&manifest, &binary, &seed)
            .map_err(|error| format!("bundle {}: {error}", app.id))?;
        let (package_name, package_sha256) = release_package_name(&app.id, &bundle);
        let package_path = output.join(&package_name);
        fs::write(&package_path, &bundle)
            .map_err(|error| format!("write {}: {error}", package_path.display()))?;
        entries.push(
            kobo_app_store::CatalogEntry::new(kobo_app_store::CatalogEntryInput {
                manifest,
                package_url: format!("{base_url}/{package_name}"),
                package_sha256,
                package_bytes: u64::try_from(bundle.len())
                    .map_err(|_| format!("{} package is too large", app.id))?,
            })
            .map_err(|error| format!("catalog {}: {error}", app.id))?,
        );
        println!("created {}", package_path.display());
    }

    let catalog = kobo_app_store::Catalog::new(entries)
        .map_err(|error| format!("build app catalog: {error}"))?;
    let catalog_bytes = catalog.to_canonical_bytes();
    let signature =
        kobo_app_store::sign(&catalog_bytes, &seed).map_err(|error| error.to_string())?;
    let catalog_path = output.join("cobalt-app-catalog.json");
    let signature_path = output.join("cobalt-app-catalog.json.sig");
    fs::write(&catalog_path, catalog_bytes)
        .map_err(|error| format!("write {}: {error}", catalog_path.display()))?;
    fs::write(&signature_path, format!("{signature}\n"))
        .map_err(|error| format!("write {}: {error}", signature_path.display()))?;
    println!("created {}", catalog_path.display());
    println!("created {}", signature_path.display());
    Ok(())
}

fn build_release_binary(app: &ReleaseApp) -> Result<Vec<u8>, String> {
    let mut build = device_build_command(&app.package, None)?;
    run_status(&mut build, format!("build {}", app.package))?;
    read_release_binary(app)
}

fn read_release_binary(app: &ReleaseApp) -> Result<Vec<u8>, String> {
    read_verified_arm_binary(&workspace_device_binary(&app.package))
}

fn read_release_binary_from(app: &ReleaseApp, directory: &Path) -> Result<Vec<u8>, String> {
    read_verified_arm_binary(&directory.join(&app.package))
}

fn validate_prebuilt_directory(apps: &[ReleaseApp], directory: &Path) -> Result<(), String> {
    let expected = apps
        .iter()
        .map(|app| app.package.as_str())
        .collect::<BTreeSet<_>>();
    let mut found = BTreeSet::new();
    let entries = fs::read_dir(directory)
        .map_err(|error| format!("read prebuilt directory {}: {error}", directory.display()))?;
    for entry in entries {
        let entry = entry
            .map_err(|error| format!("read prebuilt directory {}: {error}", directory.display()))?;
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|error| format!("inspect {}: {error}", entry.path().display()))?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            return Err("prebuilt binary names must be UTF-8".to_owned());
        };
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            return Err(format!("prebuilt entry '{name}' must be a regular file"));
        }
        found.insert(name);
    }
    let found = found.iter().map(String::as_str).collect::<BTreeSet<_>>();
    if found != expected {
        return Err(format!(
            "prebuilt directory contents do not match the registry: expected {expected:?}, found {found:?}"
        ));
    }
    Ok(())
}

fn validate_artifact_directory(apps: &[ReleaseApp], directory: &Path) -> Result<(), String> {
    let expected = apps
        .iter()
        .map(|app| format!("verified-app-{}", app.package))
        .collect::<BTreeSet<_>>();
    let found = directory_entries(directory)?;
    if found != expected {
        return Err(format!(
            "artifact directory contents do not match the registry: expected {expected:?}, found {found:?}"
        ));
    }
    for app in apps {
        let artifact = directory.join(format!("verified-app-{}", app.package));
        let metadata = fs::symlink_metadata(&artifact)
            .map_err(|error| format!("inspect {}: {error}", artifact.display()))?;
        if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
            return Err(format!(
                "artifact '{}' must be a directory",
                artifact.display()
            ));
        }
        let contents = directory_entries(&artifact)?;
        let expected_file = BTreeSet::from([app.package.clone()]);
        if contents != expected_file {
            return Err(format!(
                "artifact '{}' must contain only '{}'",
                artifact.display(),
                app.package
            ));
        }
        let binary = artifact.join(&app.package);
        let metadata = fs::symlink_metadata(&binary)
            .map_err(|error| format!("inspect {}: {error}", binary.display()))?;
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            return Err(format!(
                "artifact binary '{}' is not a regular file",
                binary.display()
            ));
        }
    }
    Ok(())
}

fn directory_entries(directory: &Path) -> Result<BTreeSet<String>, String> {
    fs::read_dir(directory)
        .map_err(|error| format!("read directory {}: {error}", directory.display()))?
        .map(|entry| {
            let entry = entry
                .map_err(|error| format!("read directory {}: {error}", directory.display()))?;
            entry
                .file_name()
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| {
                    format!(
                        "directory {} contains a non-UTF-8 name",
                        directory.display()
                    )
                })
        })
        .collect()
}

fn read_release_artifact(app: &ReleaseApp, directory: &Path) -> Result<Vec<u8>, String> {
    read_verified_arm_binary(
        &directory
            .join(format!("verified-app-{}", app.package))
            .join(&app.package),
    )
}

fn read_verified_arm_binary(path: &Path) -> Result<Vec<u8>, String> {
    verify_arm_elf(path)?;
    fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))
}

fn read_release_registry(path: &Path) -> Result<Vec<ReleaseApp>, String> {
    let text =
        fs::read_to_string(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    let document =
        kobo_json::parse(&text).map_err(|error| format!("parse {}: {error}", path.display()))?;
    let fields = strict_registry_object(&document, "registry", &["format_version", "apps"], &[])?;
    if registry_field(fields, "format_version")?.as_i64() != Some(1) {
        return Err("app registry format_version must be 1".to_owned());
    }
    let values = registry_field(fields, "apps")?
        .as_array()
        .ok_or_else(|| "app registry field 'apps' must be an array".to_owned())?;
    let mut apps = values
        .iter()
        .map(parse_release_app)
        .collect::<Result<Vec<_>, _>>()?;
    let mut packages = BTreeSet::new();
    let mut ids = BTreeSet::new();
    for app in &apps {
        if !valid_slug(&app.package) || !app.package.starts_with("kobo-") {
            return Err(format!(
                "app package '{}' must be a lowercase kobo-* Cargo package",
                app.package
            ));
        }
        if !packages.insert(&app.package) {
            return Err(format!("duplicate app package '{}'", app.package));
        }
        if !ids.insert(&app.id) {
            return Err(format!("duplicate app id '{}'", app.id));
        }
        kobo_app_store::Manifest::new_public(kobo_app_store::ManifestInput {
            id: app.id.clone(),
            display_name: app.display_name.clone(),
            short_label: app.short_label.clone(),
            summary: app.summary.clone(),
            version: app.version.clone(),
            minimum_cobalt_version: app.minimum_cobalt_version.clone(),
            glyph: app.glyph.clone(),
            capabilities: app.capabilities.clone(),
            binary_sha256: "0".repeat(64),
            binary_bytes: 1,
        })
        .map_err(|error| format!("invalid {} registry entry: {error}", app.id))?;
    }
    apps.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(apps)
}

fn parse_release_app(value: &kobo_json::Value) -> Result<ReleaseApp, String> {
    const FIELDS: [&str; 9] = [
        "package",
        "id",
        "display_name",
        "short_label",
        "summary",
        "version",
        "minimum_cobalt_version",
        "glyph",
        "capabilities",
    ];
    // `setup` and `release_notes` are website-only registry fields. The page
    // generator validates their metadata; the CLI ignores them when building
    // signed release manifests.
    let fields = strict_registry_object(value, "app", &FIELDS, &["setup", "release_notes"])?;
    let string = |name| {
        registry_field(fields, name)?
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| format!("app field '{name}' must be a string"))
    };
    let capabilities = registry_field(fields, "capabilities")?
        .as_array()
        .ok_or_else(|| "app field 'capabilities' must be an array".to_owned())?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| "app capabilities must be strings".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ReleaseApp {
        package: string("package")?,
        id: string("id")?,
        display_name: string("display_name")?,
        short_label: string("short_label")?,
        summary: string("summary")?,
        version: string("version")?,
        minimum_cobalt_version: string("minimum_cobalt_version")?,
        glyph: string("glyph")?,
        capabilities,
    })
}

fn strict_registry_object<'a>(
    value: &'a kobo_json::Value,
    object: &str,
    required: &[&str],
    optional: &[&str],
) -> Result<&'a [(String, kobo_json::Value)], String> {
    let kobo_json::Value::Object(fields) = value else {
        return Err(format!("{object} must be an object"));
    };
    let mut seen = BTreeSet::new();
    for (name, _) in fields {
        if !required.contains(&name.as_str()) && !optional.contains(&name.as_str()) {
            return Err(format!("unknown field '{name}' in {object}"));
        }
        if !seen.insert(name.as_str()) {
            return Err(format!("duplicate field '{name}' in {object}"));
        }
    }
    for name in required {
        if !seen.contains(name) {
            return Err(format!("missing field '{name}' in {object}"));
        }
    }
    Ok(fields)
}

fn registry_field<'a>(
    fields: &'a [(String, kobo_json::Value)],
    name: &str,
) -> Result<&'a kobo_json::Value, String> {
    fields
        .iter()
        .find(|(field, _)| field == name)
        .map(|(_, value)| value)
        .ok_or_else(|| format!("missing registry field '{name}'"))
}

fn single_value_flag(arguments: &[String], flag: &str, usage: &str) -> Result<String, String> {
    let mut values = arguments
        .windows(2)
        .filter(|pair| pair[0] == flag)
        .map(|pair| pair[1].as_str());
    let value = values.next().ok_or_else(|| usage.to_owned())?;
    if values.next().is_some() || value.starts_with("--") {
        return Err(usage.to_owned());
    }
    Ok(value.to_owned())
}

fn single_path_flag(arguments: &[String], flag: &str, usage: &str) -> Result<PathBuf, String> {
    single_value_flag(arguments, flag, usage).map(PathBuf::from)
}

fn optional_path_flag(
    arguments: &[String],
    flag: &str,
    usage: &str,
) -> Result<Option<PathBuf>, String> {
    optional_value_flag(arguments, flag, usage).map(|value| value.map(PathBuf::from))
}

fn optional_value_flag(
    arguments: &[String],
    flag: &str,
    usage: &str,
) -> Result<Option<String>, String> {
    let count = arguments
        .iter()
        .filter(|argument| *argument == flag)
        .count();
    match count {
        0 => Ok(None),
        1 => single_value_flag(arguments, flag, usage).map(Some),
        _ => Err(usage.to_owned()),
    }
}

fn paired_flag(
    arguments: &[String],
    flag: &str,
    usage: &str,
) -> Result<Vec<(String, String)>, String> {
    let mut entries = Vec::new();
    let mut index = 0;
    while index < arguments.len() {
        if arguments[index] == flag {
            let package = arguments.get(index + 1).ok_or_else(|| usage.to_owned())?;
            let url = arguments.get(index + 2).ok_or_else(|| usage.to_owned())?;
            if package.starts_with("--") || url.starts_with("--") {
                return Err(usage.to_owned());
            }
            entries.push((package.clone(), url.clone()));
            index += 3;
        } else {
            index += 2;
        }
    }
    Ok(entries)
}

fn ensure_only_flags(arguments: &[String], flags: &[&str], usage: &str) -> Result<(), String> {
    let mut index = 0;
    while index < arguments.len() {
        let flag = arguments[index].as_str();
        let width = if flag == "--entry" { 3 } else { 2 };
        if !flags.contains(&flag)
            || arguments.get(index + width - 1).is_none()
            || arguments[index + 1..index + width]
                .iter()
                .any(|value| value.starts_with("--"))
        {
            return Err(usage.to_owned());
        }
        index += width;
    }
    Ok(())
}

fn release_package_name(id: &str, bundle: &[u8]) -> (String, String) {
    let digest = kobo_net::sha256::hex_digest(bundle);
    (format!("{id}-{digest}.cobalt-app"), digest)
}

fn read_signing_seed(path: &Path) -> Result<[u8; 32], String> {
    let bytes = fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    if let Ok(seed) = <[u8; 32]>::try_from(bytes.as_slice()) {
        return Ok(seed);
    }
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| "signing seed must be 32 raw bytes or 64 lowercase hex characters".to_owned())?
        .trim();
    if text.len() != 64
        || !text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("signing seed must be 32 raw bytes or 64 lowercase hex characters".to_owned());
    }
    let mut seed = [0_u8; 32];
    for (slot, pair) in seed.iter_mut().zip(text.as_bytes().chunks_exact(2)) {
        let high = hex_digit(pair[0]).ok_or("invalid signing seed")?;
        let low = hex_digit(pair[1]).ok_or("invalid signing seed")?;
        *slot = (high << 4) | low;
    }
    Ok(seed)
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
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
    fs::write(root.join("src/main.rs"), generated_app_source())
        .map_err(|error| error.to_string())?;
    println!("created {}", root.display());
    println!("next: cd {name} && kobo dev");
    Ok(())
}

/// The application `kobo new` writes, which is `examples/hello` verbatim.
///
/// Included from a real workspace member rather than held here as a string,
/// so that `cargo build` compiles it and `cargo test` runs its tests. The
/// template was a string constant once, guarded by a test that searched it for
/// words it should contain. Every word was still there on the day the SDK's
/// event enum grew two variants and every application `kobo new` produced
/// stopped compiling. A template nothing builds is a template that rots.
const TEMPLATE: &str = include_str!("../../../examples/hello/src/main.rs");

/// The template with its own front matter removed.
///
/// The `//!` block at the top of `examples/hello` explains that the file is a
/// template, which is true where it lives and meaningless once it has been
/// copied into somebody's new application.
fn generated_app_source() -> String {
    let body: String = TEMPLATE
        .lines()
        .skip_while(|line| line.starts_with("//!") || line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    format!("{body}\n")
}

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
        // Rust code needs no C compiler, but `ring` carries assembly and C
        // that cc-rs builds itself, and cc-rs looks for a tool named after the
        // target and then gives up with a message about a name nobody has
        // heard of. Resolved here so the failure names the missing package.
        if std::env::var_os("CC_armv7_unknown_linux_musleabihf").is_none() {
            command.env("CC_armv7_unknown_linux_musleabihf", find_device_cc()?);
        }
        if std::env::var_os("AR_armv7_unknown_linux_musleabihf").is_none() {
            command.env("AR_armv7_unknown_linux_musleabihf", find_device_ar()?);
        }
        command.args(["--target", "armv7-unknown-linux-musleabihf"]);
        for package in DEVICE_PACKAGES {
            command.args(["-p", package]);
        }
    }
    run_status(&mut command, "cargo build")?;
    if device {
        for name in DEVICE_PACKAGES {
            let binary = workspace_device_binary(name);
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
    if let Some(position) = arguments
        .iter()
        .position(|argument| is_device_flag(argument))
    {
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
        [device, host] if is_device_flag(device) => (host, TOUCH_PROBE_DEFAULT_SECONDS),
        [device, host, flag, value] if is_device_flag(device) && flag == "--seconds" => {
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

fn app_link_command(arguments: &[String]) -> Result<(), String> {
    let (action, host) = parse_app_link(arguments)?;
    let script = format!(
        "set -eu\nexec '{}/bin/kobod' --app-link '{action}'\n",
        connect::INSTALL_DIRECTORY
    );
    let output = run_remote_shell(&format!("root@{host}"), &script, REMOTE_COMMAND_TIMEOUT)
        .map_err(unreachable_device)?;
    if !output.status.success() {
        return Err(unreachable_if_ssh_gave_up(
            remote_shell_error(
                format!("app-link {action} on {host} exited with {}", output.status),
                &output.stdout,
                &output.stderr,
            ),
            &output,
        ));
    }
    print!("{}", String::from_utf8_lossy(&output.stdout));
    Ok(())
}

fn parse_app_link(arguments: &[String]) -> Result<(&str, &str), String> {
    const USAGE: &str = "usage: kobo app-link status|unpair --device HOST";
    let [action, flag, host] = arguments else {
        return Err(USAGE.to_owned());
    };
    if !matches!(action.as_str(), "status" | "unpair")
        || !is_device_flag(flag)
        || !valid_device_host(host)
    {
        return Err(USAGE.to_owned());
    }
    Ok((action, host))
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

/// Where the runtime's trace lands on the device.
///
/// Named here rather than shared with `kobod`, because the CLI is built for
/// the host and the runtime for the device: a common constant would mean one
/// crate depending on the other for a string neither owns.
const DEVICE_TRACE_LOG: &str = "/mnt/onboard/.kobo-blackbox.log";
/// The most trace lines a single `kobo logs` may print without `--lines`.
const DEFAULT_TRACE_LINES: u32 = 200;
const MAXIMUM_TRACE_LINES: u32 = 10_000;

/// What a `kobo logs` invocation asked for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LogRequest<'a> {
    host: &'a str,
    follow: bool,
    lines: u32,
    clear: bool,
}

/// Prints the runtime's trace from the device, optionally as it is written.
///
/// The trace is the only view into what a session is actually doing (which
/// taps landed, which screens were drawn, which tasks came back) and without
/// this it can only be read by opening a shell by hand.
///
/// Spelled for hands that already know `adb logcat`: `-f` follows, `-d` dumps
/// what is there and exits, `-t` takes a line count and `-c` clears. Following
/// is a plain `tail -f` over the same bounded SSH session everything else
/// uses, with output inherited rather than captured so lines appear as they
/// happen rather than in one lump at the end.
fn device_logs(arguments: &[String]) -> Result<(), String> {
    let request = parse_logs(arguments)?;
    let remote = format!("root@{}", request.host);
    if request.clear {
        // Truncated rather than removed, so a session already holding the file
        // open keeps writing to the same one instead of into a deleted inode.
        let script = format!(": > {DEVICE_TRACE_LOG}\n");
        let output =
            run_remote_shell(&remote, &script, DEVICE_PROBE_TIMEOUT).map_err(unreachable_device)?;
        if !output.status.success() {
            return Err(unreachable_device(format!(
                "clearing the trace on {} failed",
                request.host
            )));
        }
        println!("cleared {DEVICE_TRACE_LOG} on {}", request.host);
        if !request.follow {
            return Ok(());
        }
    }
    // Reported before the shell opens, because a reader looking at an empty
    // trace should learn why here rather than conclude the device is broken.
    if request.follow {
        eprintln!(
            "following {DEVICE_TRACE_LOG} on {}; press Ctrl-C to stop",
            request.host
        );
    }
    let script = format!(
        "if [ ! -f {log} ]; then\n\
         echo 'no trace on this device yet' >&2\n\
         echo 'the runtime only writes one when started with KOBO_BLACKBOX=1' >&2\n\
         exit 3\n\
         fi\n\
         exec tail -n {lines}{follow} {log}\n",
        log = DEVICE_TRACE_LOG,
        lines = request.lines,
        follow = if request.follow { " -f" } else { "" },
    );
    let mut command = remote_shell_command(&remote);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    let mut child = command
        .spawn()
        .map_err(|error| format!("start remote shell: {error}"))?;
    let stdin_handle = child.stdin.take();
    let mut stdin = take_remote_pipe(&mut child, stdin_handle, "stdin")?;
    stdin
        .write_all(script.as_bytes())
        .and_then(|()| stdin.flush())
        .map_err(|error| format!("send the log request: {error}"))?;
    // Closed so the remote shell sees end of input and the session ends when
    // the reader interrupts rather than waiting for a line that never comes.
    drop(stdin);
    let status = child
        .wait()
        .map_err(|error| format!("wait for the log session: {error}"))?;
    match status.code() {
        Some(0) | None => Ok(()),
        // The script's own refusal, already explained on stderr.
        Some(3) => Err("no trace to read".to_owned()),
        Some(code) => Err(unreachable_device(format!(
            "reading the trace from {} failed with status {code}",
            request.host
        ))),
    }
}

/// How long a one-off command is given before the connection is abandoned.
///
/// Far longer than a probe, because the point of the verb is the command
/// nothing else runs: `dmesg`, a `find` across the card, reading a sysfs tree.
/// Six seconds is right for asking a device what it is and wrong for asking it
/// to do something.
const SHELL_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug)]
struct ShellRequest<'a> {
    host: &'a str,
    /// The words after the host, joined back into one line of shell. `None`
    /// means nobody gave a command, which is the request for a session.
    command: Option<String>,
}

fn parse_shell(arguments: &[String]) -> Result<ShellRequest<'_>, String> {
    const USAGE: &str = "usage: kobo shell --device <host> [command ...]";
    let (host, rest) = match arguments {
        [device, host, rest @ ..] if is_device_flag(device) => (host.as_str(), rest),
        _ => return Err(USAGE.to_owned()),
    };
    if !valid_device_host(host) {
        return Err("device host contains unsupported characters".to_owned());
    }
    // Joined with spaces and sent as one line, which is what ssh and adb both
    // do and therefore what anybody typing this expects. It also means the
    // quoting the device sees is the quoting that survived the local shell,
    // so a command with spaces in an argument wants quoting for both.
    let command = if rest.is_empty() {
        None
    } else {
        Some(rest.join(" "))
    };
    Ok(ShellRequest { host, command })
}

/// Runs one command on the reader, or opens a session on it.
///
/// This exists because the obvious thing does not work. `ssh root@reader 'cmd'`
/// returns nothing at all on this firmware: the login shell ignores the command
/// it was handed, so the command has to arrive on standard input with the
/// terminal turned off instead. Everything in this CLI that touches a device
/// has always done that internally; there was simply no way to ask for it, and
/// every developer who tried the obvious spelling concluded the reader was
/// broken.
///
/// A command is buffered rather than streamed, because classifying "the radio
/// was dozing" apart from "the command failed" means reading what the device
/// said before deciding whether to ask again, and that retry is worth more
/// than live output for the things this verb is for. Something that prints as
/// it goes wants `kobo logs --follow`, which streams for exactly that reason.
///
/// The remote's own exit status is returned rather than flattened, because a
/// shell that always exits 0 or 1 cannot be put in a script, and putting it in
/// a script is most of the point.
fn shell_command(arguments: &[String]) -> Result<ExitCode, String> {
    let request = parse_shell(arguments)?;
    let remote = format!("root@{}", request.host);
    let Some(command) = request.command else {
        return interactive_shell(&remote);
    };
    // A trailing newline, because the device reads this as a script and a last
    // line with no newline on it is a last line some shells decline to run.
    let script = format!(
        "{command}
"
    );
    let output = panel::run_remote_shell_waking(&remote, &script, SHELL_TIMEOUT)?;
    std::io::stdout()
        .write_all(&output.stdout)
        .map_err(|error| format!("write command output: {error}"))?;
    std::io::stderr()
        .write_all(&output.stderr)
        .map_err(|error| format!("write command errors: {error}"))?;
    Ok(exit_code_of(output.status))
}

/// Hands the session to the person at the keyboard.
///
/// Inherited streams and a terminal, so this is an ordinary login: line
/// editing, job control and a prompt all work, and nothing here reads or
/// rewrites what passes through. No waking retry either, because a retry that
/// silently reopens a session somebody was typing into is worse than being
/// told to try again.
fn interactive_shell(remote: &str) -> Result<ExitCode, String> {
    eprintln!("opening a session on {remote}; exit or Ctrl-D to leave");
    let status = remote_ssh_command(remote, Tty::Yes)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|error| format!("start remote session: {error}"))?;
    Ok(exit_code_of(status))
}

/// The process's status as an exit code this process can return.
///
/// A command killed by a signal has no exit code of its own. It reports as 128
/// plus the signal, which is what every shell reports for the same thing, so a
/// script reading this sees what it would have seen running the command
/// locally.
fn exit_code_of(status: ExitStatus) -> ExitCode {
    match status.code() {
        Some(code) => ExitCode::from(u8::try_from(code & 0xff).unwrap_or(1)),
        None => ExitCode::from(128_u8.saturating_add(signal_of(status))),
    }
}

#[cfg(unix)]
fn signal_of(status: ExitStatus) -> u8 {
    use std::os::unix::process::ExitStatusExt;
    u8::try_from(status.signal().unwrap_or(0)).unwrap_or(0)
}

#[cfg(not(unix))]
fn signal_of(_status: ExitStatus) -> u8 {
    0
}

fn parse_logs(arguments: &[String]) -> Result<LogRequest<'_>, String> {
    const USAGE: &str =
        "usage: kobo logs --device <host> [--follow|-f] [--dump|-d] [--lines|-t <count>] \
         [--clear|-c]";
    let (host, mut rest) = match arguments {
        [device, host, rest @ ..] if is_device_flag(device) => (host.as_str(), rest),
        _ => return Err(USAGE.to_owned()),
    };
    if !valid_device_host(host) {
        return Err("device host contains unsupported characters".to_owned());
    }
    // Left unset until asked, so the default can depend on `--clear` without
    // depending on the order the two were written in.
    let mut follow: Option<bool> = None;
    let mut lines = DEFAULT_TRACE_LINES;
    let mut clear = false;
    while let Some(argument) = rest.first() {
        match argument.as_str() {
            "--follow" | "-f" => {
                follow = Some(true);
                rest = &rest[1..];
            }
            "--dump" | "-d" => {
                follow = Some(false);
                rest = &rest[1..];
            }
            "--clear" | "-c" => {
                clear = true;
                rest = &rest[1..];
            }
            "--lines" | "-t" | "-n" => {
                let value = rest.get(1).ok_or_else(|| USAGE.to_owned())?;
                lines = value
                    .parse::<u32>()
                    .map_err(|_| "--lines takes a whole number".to_owned())?;
                if lines == 0 || lines > MAXIMUM_TRACE_LINES {
                    return Err(format!(
                        "--lines must be between 1 and {MAXIMUM_TRACE_LINES}"
                    ));
                }
                rest = &rest[2..];
            }
            _ => return Err(USAGE.to_owned()),
        }
    }
    Ok(LogRequest {
        host,
        // Following is the default, because the reason to ask for a device's
        // log is almost always to watch what happens next. Clearing on its own
        // clears and exits, which is what `adb logcat -c` does; asked for
        // together they clear first and then watch, which is the useful shape
        // before a test run.
        follow: follow.unwrap_or(!clear),
        lines,
        clear,
    })
}

/// Returns true when a bounded shell session opens and exits cleanly.
fn device_answers(remote: &str) -> bool {
    run_remote_shell(remote, "exit\n", DEVICE_PROBE_TIMEOUT)
        .is_ok_and(|output| output.status.success())
}

fn parse_wait(arguments: &[String]) -> Result<(&str, Duration), String> {
    const USAGE: &str = "usage: kobo wait --device <host> [--timeout <seconds>]";
    let (host, rest) = match arguments {
        [device, host, rest @ ..] if is_device_flag(device) => (host, rest),
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
        [device, host, rest @ ..] if is_device_flag(device) => (host, rest),
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
        [device, host, confirm, value] if is_device_flag(device) && confirm == "--confirm" => {
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
        [device, host, confirm, value] if is_device_flag(device) && confirm == "--confirm" => {
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
    if std::env::var_os("CC_armv7_unknown_linux_musleabihf").is_none() {
        command.env("CC_armv7_unknown_linux_musleabihf", find_device_cc()?);
    }
    if std::env::var_os("AR_armv7_unknown_linux_musleabihf").is_none() {
        command.env("AR_armv7_unknown_linux_musleabihf", find_device_ar()?);
    }
    if let Some(features) = features {
        command.args(["--features", features]);
    }
    Ok(command)
}

#[derive(Clone)]
enum RemoteProgram {
    Doctor,
    /// The same read-only doctor binary, additionally watching touch for the
    /// given number of seconds.
    TouchProbe(u64),
    /// The same read-only doctor binary, additionally copying the panel out.
    Capture,
    /// The same read-only doctor binary, copying the panel out repeatedly.
    Record {
        seconds: u64,
        fps: u32,
    },
    /// A run of synthetic taps, with the waits between them.
    ///
    /// One run rather than one per tap because each of these uploads the tap
    /// binary, checksums it on the reader's own processor and removes it
    /// again. Paying that per tap made driving an application slower than the
    /// application, and put an SSH round trip inside every wait.
    #[cfg(feature = "device-write")]
    Tap {
        sequence: String,
        millis: u64,
    },
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
        match &self.program {
            RemoteProgram::TouchProbe(seconds) => {
                Duration::from_secs(*seconds) + TOUCH_PROBE_OVERHEAD
            }
            // The reading itself is bounded in the binary and again by
            // timeout, and the wait has to outlast the recording rather than
            // the usual single round trip.
            RemoteProgram::Record { seconds, .. } => {
                Duration::from_secs(*seconds) + TOUCH_PROBE_OVERHEAD
            }
            RemoteProgram::Doctor | RemoteProgram::Capture => REMOTE_COMMAND_TIMEOUT,
            // A sequence sleeps on the device for as long as it was asked to,
            // so the host has to outlast the sleeping as well as the transfer.
            #[cfg(feature = "device-write")]
            RemoteProgram::Tap { millis, .. } => {
                Duration::from_millis(*millis) + REMOTE_COMMAND_TIMEOUT
            }
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

    fn capture() -> Self {
        Self {
            program: RemoteProgram::Capture,
            label: "read-only screen capture",
            ..Self::doctor()
        }
    }

    fn record(seconds: u64, fps: u32) -> Self {
        Self {
            program: RemoteProgram::Record { seconds, fps },
            label: "read-only screen recording",
            ..Self::doctor()
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
    fn tap(sequence: String, millis: u64) -> Self {
        Self {
            label: "synthetic tap",
            directory_label: "kobo-tap",
            binary_name: "kobo-tap",
            local_binary: workspace_device_binary("kobo-tap"),
            package: "kobo-tap",
            features: Some("device-write"),
            program: RemoteProgram::Tap { sequence, millis },
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

/// Runs a fixed artifact on the device and prints what it said.
fn run_remote_fixed_artifact(host: &str, artifact: &RemoteArtifact) -> Result<(), String> {
    let transcript = capture_remote_fixed_artifact(host, artifact)?;
    print!("{transcript}");
    Ok(())
}

/// The same, but handing the transcript back instead of printing it.
///
/// Split out for the screenshot, which arrives as two megabytes of base64 in
/// the middle of the doctor's ordinary report. Printing that to a terminal
/// would be a practical joke.
fn capture_remote_fixed_artifact(host: &str, artifact: &RemoteArtifact) -> Result<String, String> {
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
        &artifact.program,
        &checksum,
        &base64_encode(&bytes),
    );
    match run_remote_shell(&remote, &script, artifact.timeout()) {
        Ok(output) if output.status.success() => {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
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
/// Cargo resolves a relative `CARGO_TARGET_DIR` from its invoking current
/// directory, not from the manifest's workspace.
fn workspace_device_binary(name: &str) -> PathBuf {
    workspace_target_directory()
        .join("armv7-unknown-linux-musleabihf/release")
        .join(name)
}

fn workspace_host_binary(name: &str) -> PathBuf {
    workspace_target_directory().join("debug").join(name)
}

fn workspace_target_directory() -> PathBuf {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let invocation = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    configured_target_directory(
        &workspace,
        &invocation,
        env::var_os("CARGO_TARGET_DIR").as_deref(),
    )
}

fn configured_target_directory(
    workspace: &Path,
    invocation: &Path,
    configured: Option<&OsStr>,
) -> PathBuf {
    configured.map_or_else(
        || workspace.join("target"),
        |target| {
            let target = Path::new(target);
            if target.is_absolute() {
                target.to_path_buf()
            } else {
                invocation.join(target)
            }
        },
    )
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

#[allow(clippy::too_many_lines)]
fn remote_fixed_artifact_script(
    session: &RemoteArtifactSession,
    program: &RemoteProgram,
    checksum: &str,
    encoded_artifact: &str,
) -> String {
    let execution = match program {
        RemoteProgram::Doctor => "\"$bin\"".to_owned(),
        // Read-only, like the doctor it is: it opens the framebuffer for
        // reading and never grabs, refreshes or writes, so it is safe to point
        // at a device with the stock reader in the foreground.
        RemoteProgram::Capture => "KOBO_DOCTOR_CAPTURE=1 \"$bin\"".to_owned(),
        // Bounded twice, like the touch probe: a tool that watches the panel
        // for a while must stop on its own even if the host walks away.
        RemoteProgram::Record { seconds, fps } => format!(
            "if [ -x /usr/bin/timeout ]; then\n\
             \x20 KOBO_DOCTOR_RECORD={seconds}:{fps} KOBO_DOCTOR_RECORD_PATH='{RECORDING_ON_DEVICE}' \
             /usr/bin/timeout {} \"$bin\"\n\
             else\n\
             \x20 echo 'BusyBox timeout is unavailable; refusing recording' >&2\n\
             \x20 exit 1\n\
             fi",
            seconds + 20
        ),
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
        RemoteProgram::Tap { sequence, millis } => format!(
            "if [ -x /usr/bin/timeout ]; then\n\
             \x20 KOBO_TAP_UNLOCK='OWNER_ATTENDED_SYNTHETIC_TOUCH' KOBO_TAP_POINT='{sequence}' \
             /usr/bin/timeout {} \"$bin\"\n\
             else\n\
             \x20 echo 'BusyBox timeout is unavailable; refusing synthetic tap' >&2\n\
             \x20 exit 1\n\
             fi",
            millis / 1000 + 30
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
        RemoteProgram::Doctor
        | RemoteProgram::TouchProbe(_)
        | RemoteProgram::Capture
        | RemoteProgram::Record { .. } => "uploaded doctor checksum does not match",
        #[cfg(feature = "device-write")]
        RemoteProgram::Smoke(_) => "uploaded smoke checksum does not match",
        #[cfg(feature = "device-write")]
        RemoteProgram::Guard => "uploaded guard checksum does not match",
        #[cfg(feature = "device-write")]
        RemoteProgram::Tap { .. } => "uploaded tap checksum does not match",
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
/// ssh reserves exit status 255 for its own failures (refused, timed out, key
/// rejected) so anything else came back from a shell that really did run on
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

/// Where an owner may keep a dedicated key for a reader they secured.
///
/// A name of its own rather than `id_ed25519`, so that setting up a reader
/// never touches whatever key somebody already uses for everything else.
pub const DEVICE_KEY_NAME: &str = "kobo_cobalt";

fn default_device_key_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".ssh").join(DEVICE_KEY_NAME))
}

/// The dedicated key used for reader connections, when it exists.
#[must_use]
pub fn device_key_path() -> Option<PathBuf> {
    let key = default_device_key_path()?;
    key.is_file().then_some(key)
}

/// An `ssh` invocation that will offer the reader's key.
///
/// Without `-i` this offered only the default identities, and the reader's key
/// is deliberately not one of those, so every connection failed on a reader
/// that was set up correctly. `IdentitiesOnly` prevents a busy SSH agent from
/// exhausting the reader's authentication attempts before this key is tried.
fn remote_shell_command(remote: &str) -> Command {
    remote_ssh_command(remote, Tty::No)
}

/// Whether the reader should be given a terminal.
///
/// Every other caller wants `No`: a script is piped in and read back, and a
/// terminal would echo the script into the output. `Yes` exists for the one
/// verb that hands the session to a person, where line editing, job control
/// and a prompt are the whole point.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tty {
    No,
    Yes,
}

fn remote_ssh_command(remote: &str, tty: Tty) -> Command {
    let mut command = Command::new("ssh");
    command
        .args([
            if tty == Tty::Yes { "-t" } else { "-T" },
            "-o",
            "BatchMode=yes",
            "-o",
        ])
        .arg(format!("ConnectTimeout={REMOTE_CONNECT_TIMEOUT_SECONDS}"));
    if let Some(key) = device_key_path() {
        command.args(["-o", "IdentitiesOnly=yes", "-i"]).arg(key);
    }
    command.arg(remote);
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
        // A script that decides not to read the rest of its input (because it
        // refused, or because it ended in `exec`) closes the pipe under this
        // writer, and that is not a transport failure. The device answered;
        // its status and its stderr are what the caller has to be told, and
        // reporting a broken pipe instead buries a plain refusal under advice
        // about Wi-Fi and sleeping readers.
        if error.kind() != std::io::ErrorKind::BrokenPipe {
            return Err(remote_shell_error(
                format!("write remote script: {error}"),
                &stdout,
                &stderr,
            ));
        }
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
        let binary = workspace_device_binary(name);
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
        "LICENSE",
        include_str!("../../../LICENSE"),
        false,
    ));
    members.push(text_member(
        "THIRD-PARTY.md",
        include_str!("../../../THIRD-PARTY.md"),
        false,
    ));
    members.push(text_member(
        "licenses/LICENSE-Rust-dependencies.txt",
        include_str!("../../../licenses/LICENSE-Rust-dependencies.txt"),
        false,
    ));
    members.push(text_member(
        "licenses/LICENSE-AtkinsonHyperlegible.txt",
        include_str!("../../kobo-text/fonts/LICENSE-AtkinsonHyperlegible.txt"),
        false,
    ));
    members.push(text_member(
        "licenses/LICENSE-DejaVu.txt",
        include_str!("../../kobo-text/fonts/LICENSE-DejaVu.txt"),
        false,
    ));
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

/// What to say when the cable is in but nothing usable is behind it.
const NO_READER_FOUND: &str = "\
No mounted reader found. A Kobo appears as a removable drive holding a
.kobo/version file, and only while it is showing 'Connected' on its own
screen.

  1. Plug the cable into the reader and this machine directly, not through a
     hub or a charger-only cable.
  2. The reader asks whether to connect. Tap 'Connect'. It will not mount
     until you do.
  3. If it is already mounted somewhere unusual, name it: kobo setup --volume /path";

/// Which direction `kobo setup` was pointed in.
///
/// A mode rather than a flag because the two are exclusive and reading them as
/// two booleans made `--undo --dry-run` perform the undo: the undo branch was
/// taken before the dry run was ever consulted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SetupMode {
    /// Install Cobalt and prepare the reader.
    Install,
    /// Put the reader back to how it shipped.
    Undo,
}

/// Whether to give the reader its own way into Cobalt.
///
/// An enum rather than a fourth boolean because it is the one option that
/// decides whether this command hands anything to a root extractor, and a
/// named either/or is harder to pass in the wrong position than a bare `true`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MenuEntry {
    /// Write the entry, staging NickelMenu if it is not already installed.
    Add,
    /// Stage NickelMenu even when its marker says it is installed, for a
    /// reader whose plugin a firmware update removed.
    Force,
    /// Leave the reader's menus alone.
    Skip,
}

/// How `kobo setup` was asked to run.
#[derive(Debug)]
#[allow(clippy::struct_excessive_bools)]
struct SetupOptions {
    volume: Option<PathBuf>,
    release_dir: Option<PathBuf>,
    source: bool,
    mode: SetupMode,
    menu: MenuEntry,
    eject: bool,
    dry_run: bool,
    yes: bool,
    non_interactive: bool,
    wait_for_reader: bool,
    wait: bool,
    enable_ssh: bool,
    /// Whether this machine's key is installed alongside the SSH server.
    ///
    /// Default on, because a server that starts and accepts nobody is not a
    /// thing anybody asked for. `--no-key` is for a reader that already has
    /// the key, or one being prepared for somebody else.
    authorize_key: bool,
}

fn parse_setup(arguments: &[String]) -> Result<SetupOptions, String> {
    let mut options = SetupOptions {
        volume: None,
        release_dir: None,
        source: false,
        mode: SetupMode::Install,
        menu: MenuEntry::Add,
        eject: true,
        dry_run: false,
        yes: false,
        non_interactive: false,
        wait_for_reader: false,
        wait: true,
        enable_ssh: false,
        authorize_key: true,
    };
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--volume" | "-v" => {
                let value = arguments
                    .get(index + 1)
                    .ok_or("--volume needs a path to a mounted reader")?;
                options.volume = Some(PathBuf::from(value));
                index += 1;
            }
            "--undo" => options.mode = SetupMode::Undo,
            "--release-dir" => {
                let value = arguments
                    .get(index + 1)
                    .ok_or("--release-dir needs a verified release directory")?;
                options.release_dir = Some(PathBuf::from(value));
                index += 1;
            }
            "--source" => options.source = true,
            "--yes" => options.yes = true,
            "--non-interactive" => options.non_interactive = true,
            "--wait-for-reader" => options.wait_for_reader = true,
            "--no-eject" => options.eject = false,
            "--no-wait" => options.wait = false,
            "--no-menu" => options.menu = MenuEntry::Skip,
            "--menu" => options.menu = MenuEntry::Force,
            "--enable-ssh" => options.enable_ssh = true,
            "--no-key" => options.authorize_key = false,
            "--dry-run" => options.dry_run = true,
            other => {
                return Err(format!(
                    "unknown option '{other}'\n\
                     usage: kobo setup [--volume PATH] [--undo] [--enable-ssh] [--no-key] \
                     [--no-eject] [--no-wait] [--menu] [--no-menu] [--dry-run] [--yes] \
                     [--non-interactive] [--wait-for-reader] [--release-dir PATH] [--source]"
                ))
            }
        }
        index += 1;
    }
    if options.source && options.release_dir.is_some() {
        return Err("--source and --release-dir are mutually exclusive".to_owned());
    }
    if options.non_interactive && !options.yes && !options.dry_run {
        return Err("--non-interactive requires --yes for any change".to_owned());
    }
    Ok(options)
}

/// Picks the reader to work on, and refuses to guess between two.
fn chosen_reader(volume: Option<&Path>) -> Result<setup::Mounted, String> {
    if let Some(path) = volume {
        return setup::read_reader(path).ok_or_else(|| {
            format!(
                "{} is not a mounted reader: no {}/version naming a Kobo serial",
                path.display(),
                setup::SYSTEM_FOLDER
            )
        });
    }
    choose_discovered_reader()
}

fn wait_for_mounted_reader(volume: Option<&Path>, wait: bool) -> Result<setup::Mounted, String> {
    if !wait {
        return chosen_reader(volume);
    }
    println!(
        "Connect a charged Kobo with a data-capable USB cable, tap Connect on the reader,\n\
             and leave the mounted volume connected. Waiting up to 10 minutes..."
    );
    let deadline = Instant::now() + Duration::from_secs(600);
    loop {
        match chosen_reader(volume) {
            Ok(reader) => return Ok(reader),
            Err(error) if error.starts_with("No mounted reader found.") => {
                if Instant::now() >= deadline {
                    return Err(error);
                }
                thread::sleep(Duration::from_secs(2));
            }
            Err(error) => return Err(error),
        }
    }
}

enum SetupPayload {
    Source,
    Prebuilt {
        built: BuiltPackage,
        version: String,
        channel: String,
    },
}

impl SetupPayload {
    fn description(&self) -> String {
        match self {
            Self::Source => format!(
                "Cobalt {} from this source checkout",
                env!("CARGO_PKG_VERSION")
            ),
            Self::Prebuilt {
                version, channel, ..
            } => format!("Cobalt {version} ({channel})"),
        }
    }

    fn into_built(self) -> Result<BuiltPackage, String> {
        match self {
            Self::Source => build_package_bytes(),
            Self::Prebuilt { built, .. } => Ok(built),
        }
    }
}

fn setup_payload(options: &SetupOptions) -> Result<SetupPayload, String> {
    if options.source {
        return Ok(SetupPayload::Source);
    }
    if let Some(directory) = options
        .release_dir
        .clone()
        .or_else(managed_release_directory)
    {
        return load_release_package(&directory);
    }
    Ok(SetupPayload::Source)
}

fn load_release_package(directory: &Path) -> Result<SetupPayload, String> {
    let manifest_path = directory.join("cobalt-host-manifest.txt");
    let signature_path = directory.join("cobalt-host-manifest.txt.sig");
    let manifest_bytes = fs::read(&manifest_path)
        .map_err(|error| format!("read {}: {error}", manifest_path.display()))?;
    let signature = fs::read_to_string(&signature_path)
        .map_err(|error| format!("read {}: {error}", signature_path.display()))?;
    let manifest = host_release::verify_manifest(&manifest_bytes, &signature)?;
    load_release_package_from_manifest(directory, manifest)
}

fn load_release_package_from_manifest(
    directory: &Path,
    manifest: host_release::Manifest,
) -> Result<SetupPayload, String> {
    if !kobo_app_store::cobalt_version_at_least(env!("CARGO_PKG_VERSION"), &manifest.version) {
        return Err(format!(
            "stable setup package {} is newer than this kobo {}; update the host command first",
            manifest.version,
            env!("CARGO_PKG_VERSION")
        ));
    }
    let asset = manifest
        .device()
        .ok_or("signed release manifest has no device package")?;
    let channel = fs::read_to_string(directory.join("channel"))
        .map_err(|error| format!("read release channel: {error}"))?
        .trim()
        .to_owned();
    if channel != "stable" {
        return Err(
            "kobo setup installs stable releases only; enable Beta updates later in Cobalt Settings"
                .to_owned(),
        );
    }
    if !manifest.allows_channel("stable") {
        return Err("signed release manifest does not allow stable installation".to_owned());
    }
    let package_path = directory.join(&asset.name);
    let compressed = fs::read(&package_path)
        .map_err(|error| format!("read {}: {error}", package_path.display()))?;
    let actual_bytes =
        u64::try_from(compressed.len()).map_err(|_| "device package is too large".to_owned())?;
    if actual_bytes != asset.bytes {
        return Err(format!(
            "{} is truncated: manifest says {} bytes, found {actual_bytes}",
            package_path.display(),
            asset.bytes
        ));
    }
    let actual_digest = sha256::hex_digest(&compressed);
    if actual_digest != asset.sha256 {
        return Err(format!(
            "{} checksum failed: expected {}, found {actual_digest}",
            package_path.display(),
            asset.sha256
        ));
    }
    gzip_test(&compressed)?;
    let archive = gunzip(&compressed)?;
    let members = package::members(&archive)?;
    let listed = package::list(&archive)?;
    let outside = members_outside_install_root(&listed);
    if !outside.is_empty() {
        return Err(format!(
            "refusing prebuilt package: {} would be written outside {}",
            outside.join(", "),
            package::INSTALL_ROOT
        ));
    }
    let expected_version = format!("{}\n", manifest.version);
    let packaged_version = members
        .iter()
        .find(|member| member.path == format!("{}/VERSION", package::INSTALL_ROOT))
        .ok_or("prebuilt package has no VERSION file")?;
    if packaged_version.bytes != expected_version.as_bytes() {
        return Err("prebuilt package VERSION does not match its signed manifest".to_owned());
    }
    Ok(SetupPayload::Prebuilt {
        built: BuiltPackage {
            members,
            listed,
            compressed,
        },
        version: manifest.version,
        channel,
    })
}

fn managed_release_directory() -> Option<PathBuf> {
    let home = env::var_os("HOME")?;
    let data = env::var_os("XDG_DATA_HOME")
        .map_or_else(|| Path::new(&home).join(".local/share"), PathBuf::from);
    let state = fs::read_to_string(data.join("kobo/install-state")).ok()?;
    let mut binary = None;
    let mut release = None;
    let mut channel = None;
    for line in state.lines() {
        if line == "cobalt-kobo-install 1" {
            continue;
        }
        if let Some(value) = line.strip_prefix("binary ") {
            binary = Some(PathBuf::from(value));
        } else if let Some(value) = line.strip_prefix("release ") {
            release = Some(PathBuf::from(value));
        } else if let Some(value) = line.strip_prefix("channel ") {
            channel = Some(value.to_owned());
        }
    }
    let current = env::current_exe().ok()?.canonicalize().ok()?;
    let installed = binary?.canonicalize().ok()?;
    let root = data.join("kobo");
    let selected = managed_host_directory(&root)
        .ok()
        .and_then(|directory| directory.join("kobo").canonicalize().ok());
    if current != installed && selected.as_ref() != Some(&current) {
        return None;
    }
    let derived =
        data.join("kobo/releases")
            .join(format!("{}-{}", env!("CARGO_PKG_VERSION"), channel?));
    Some(if derived.is_dir() { derived } else { release? })
}

fn reader_still_connected(expected: &setup::Mounted) -> Result<(), String> {
    let current = setup::read_reader(&expected.volume).ok_or_else(|| {
        format!(
            "{} was unmounted or is no longer a Kobo; nothing was written",
            expected.volume.display()
        )
    })?;
    if current.serial != expected.serial || current.firmware != expected.firmware {
        return Err(format!(
            "the reader at {} changed after confirmation; nothing was written",
            expected.volume.display()
        ));
    }
    Ok(())
}

fn confirmed_setup(
    options: &SetupOptions,
    reader: &setup::Mounted,
    profile: &kobo_profile::DeviceProfile,
    payload: &SetupPayload,
) -> Result<bool, String> {
    println!(
            "\nReady to {}:\n  Model: {} (device code {}, profile {})\n  Firmware: {}\n  Mount: {}\n  Release: {}\n  Changes: {}\n",
            if options.mode == SetupMode::Undo {
                "remove Cobalt"
            } else {
                "install Cobalt"
            },
            profile.model,
            profile.device_code,
            profile.id,
            reader.firmware,
            reader.volume.display(),
            payload.description(),
            if options.mode == SetupMode::Undo {
                "remove .adds/cobalt and revert only Cobalt-managed settings/menu/SSH markers"
            } else {
                "update Cobalt program files in .adds/cobalt, preserve app data, secrets, owner files and unrelated NickelMenu entries"
            }
        );
    if options.yes {
        return Ok(true);
    }
    if options.non_interactive {
        return Err("noninteractive setup was not explicitly confirmed with --yes".to_owned());
    }
    let tty = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .map_err(|error| format!("open /dev/tty for confirmation: {error}; pass --yes only after reviewing --dry-run"))?;
    let mut writer = &tty;
    writer
        .write_all(b"Continue? [y/N] ")
        .map_err(|error| format!("write confirmation prompt: {error}"))?;
    writer
        .flush()
        .map_err(|error| format!("flush confirmation prompt: {error}"))?;
    let mut reader_tty = std::io::BufReader::new(tty);
    let mut answer = String::new();
    std::io::BufRead::read_line(&mut reader_tty, &mut answer)
        .map_err(|error| format!("read confirmation: {error}"))?;
    Ok(confirmation_answer(&answer))
}

fn choose_discovered_reader() -> Result<setup::Mounted, String> {
    choose_reader_list(setup::mounted_readers())
}

fn choose_reader_list(mut found: Vec<setup::Mounted>) -> Result<setup::Mounted, String> {
    match found.len() {
        0 => Err(NO_READER_FOUND.to_owned()),
        1 => Ok(found.remove(0)),
        _ => {
            let listed = found
                .iter()
                .map(|reader| format!("  {}", reader.summary()))
                .collect::<Vec<_>>()
                .join("\n");
            Err(format!(
                "{} readers are mounted, so this will not guess. Name one:\n{listed}\n\n\
                 kobo setup --volume <path>",
                found.len()
            ))
        }
    }
}

/// Prepares a reader over USB, which is the only way in to a stock one.
///
/// Deliberately the whole of the first install: the files, the firmware's own
/// SSH server, and the setting that keeps the radio up. Everything after this
/// happens over Wi-Fi, so everything before it has to happen here.
fn setup_device(arguments: &[String]) -> Result<(), String> {
    setup_device_with_confirmation(arguments, confirmed_setup)
}

fn setup_device_with_confirmation(
    arguments: &[String],
    confirm: impl FnOnce(
        &SetupOptions,
        &setup::Mounted,
        &kobo_profile::DeviceProfile,
        &SetupPayload,
    ) -> Result<bool, String>,
) -> Result<(), String> {
    let options = parse_setup(arguments)?;
    let reader = wait_for_mounted_reader(options.volume.as_deref(), options.wait_for_reader)?;
    let profile = setup::install_profile(&reader)?;
    let payload = if options.mode == SetupMode::Undo {
        SetupPayload::Source
    } else {
        setup_payload(&options)?
    };
    println!(
        "found {} ({}, profile {}, device code {}) at {} · firmware {}",
        profile.model,
        reader.model_code(),
        profile.id,
        profile.device_code,
        reader.volume.display(),
        reader.firmware
    );

    if options.dry_run {
        println!(
            "{}\nrelease: {}",
            dry_run_plan(&options, &reader),
            payload.description()
        );
        return Ok(());
    }
    if !confirm(&options, &reader, profile, &payload)? {
        println!("Declined; nothing was written.");
        return Ok(());
    }
    reader_still_connected(&reader)?;
    if options.mode == SetupMode::Undo {
        return undo_setup(&reader, options.eject);
    }

    // Built or fully verified before anything is written, so a failure leaves
    // the reader exactly as it was rather than half set up.
    let built = payload.into_built()?;
    reader_still_connected(&reader)?;
    let installed = setup::write_payload(&built.members, &reader.volume)?;
    setup::verify_payload(&built.members, &reader.volume)?;
    reader_still_connected(&reader)?;
    let ssh = options
        .enable_ssh
        .then(|| setup::enable_ssh(&reader.volume))
        .transpose()?;
    let settings = setup::apply_settings(&reader.volume)?;
    let trust = setup::carry_trust_roots(&reader.volume);
    let menu = (options.menu != MenuEntry::Skip)
        .then(|| add_menu_entry(&reader.volume, options.menu == MenuEntry::Force));
    // After the menu, because the firmware extracts exactly one archive and
    // the first draft of this raced the menu for it: staging the key first
    // meant NickelMenu reported the slot taken on every first-time setup, and
    // the owner had to run the command twice to get the entry they asked for.
    // When this run is the one that staged that archive, the key goes into it.
    let staged_here = matches!(menu, Some(Ok(menu::Menu::Staged)));
    let key = (options.enable_ssh && options.authorize_key)
        .then(|| authorize_this_machine(&reader.volume, staged_here));
    let ejected = ejected_or_explained(&reader.volume, options.eject);

    // A reader that was never ejected has not seen the install and will not be
    // restarted into it, so there is nothing to wait for.
    let subnet = connect::local_subnet();
    let waiting = options.enable_ssh && options.wait && ejected && subnet.is_some();

    print!(
        "{}",
        setup::Report {
            installed,
            ssh,
            key,
            settings,
            trust,
            menu,
            ejected,
            waiting,
        }
        .describe_for(&reader)
    );
    if waiting {
        let subnet = subnet.unwrap_or_default();
        await_reader(&subnet);
    }
    Ok(())
}

/// Puts this machine's public key where the reader will accept it.
///
/// Never fails the setup, for the same reason the menu entry does not: the
/// install itself succeeded, and a reader that has to be reached some other
/// way is still a reader with Cobalt on it.
fn authorize_this_machine(
    volume: &Path,
    staged_here: bool,
) -> Result<(authorize::Key, authorize::Staged), String> {
    let (public_key, key) = authorize::public_key()?;
    let slot = volume.join(authorize::KOBOROOT);
    if slot.exists() {
        // Anything already in the slot that this run did not put there
        // belongs to somebody else, and replacing it would quietly cancel an
        // install the owner is expecting.
        if !staged_here {
            return Ok((key, authorize::Staged::SlotTaken));
        }
        let existing =
            fs::read(&slot).map_err(|error| format!("read {}: {error}", slot.display()))?;
        let merged = authorize::merge(&gunzip(&existing)?, &public_key)?;
        return Ok((key, authorize::restage(volume, &compressed(&merged)?)?));
    }
    let alone = authorize::archive(&public_key)?;
    Ok((key, authorize::stage(volume, &compressed(&alone)?)?))
}

/// Gzips an archive and checks it the way the reader's own `rcS` will.
fn compressed(archive: &[u8]) -> Result<Vec<u8>, String> {
    let bytes = gzip(archive)?;
    gzip_test(&bytes)?;
    Ok(bytes)
}

/// Adds the reader's own way into Cobalt, or explains why it could not.
/// Never fails the setup. Everything else this command does works without a
/// menu entry (`start.sh` over SSH is how the whole project has been run so
/// far) so a download that cannot happen on an aeroplane should not cost
/// somebody the install they came for.
fn add_menu_entry(volume: &Path, force: bool) -> Result<menu::Menu, String> {
    if menu::installed(volume) && !force {
        return menu::install(volume, None, setup::INSTALL_FOLDER, false);
    }
    let archive = env::temp_dir().join(format!("kobo-nickelmenu-{}.tgz", std::process::id()));
    menu::download(&archive)?;
    let outcome = menu::install(volume, Some(&archive), setup::INSTALL_FOLDER, force);
    let _ = fs::remove_file(&archive);
    outcome
}

/// Watches `subnet` until an address that was not answering starts to.
///
/// Prints rather than returns, and never fails: everything this command was
/// asked to do is already on the reader by the time it is called, so a wait
/// that finds nothing is a wait that found nothing, not a setup that failed.
fn await_reader(subnet: &str) {
    println!(
        "\nWaiting up to {} minutes for the reader to come back on {subnet}.0/24. Ctrl-C to stop.",
        setup::WAIT_LIMIT.as_secs() / 60
    );
    let deadline = Instant::now() + setup::WAIT_LIMIT;
    let arrival = setup::wait_for_reader(
        || connect::sweep(subnet, connect::PROBE_TIMEOUT),
        |address| match identify_device(&address.to_string()) {
            Some(identity) if identity.is_kobo() => setup::Verdict::Reader,
            // Told apart deliberately. A machine that answered and is not a
            // reader is settled; one that could not be reached at all may
            // simply still be booting, and is asked again next round.
            Some(_) => setup::Verdict::Other,
            None => setup::Verdict::Unknown,
        },
        || {
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(setup::WAIT_INTERVAL);
            print!(".");
            let _ = std::io::stdout().flush();
            true
        },
    );
    println!();
    match arrival {
        setup::Arrival::Found(address) => println!(
            "The reader is at {address}.\n\n  kobo deploy --device {address}\n\n\
             That installs over Wi-Fi from here on, with no cable and no restart."
        ),
        setup::Arrival::Several(addresses) => {
            let listed = addresses
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            println!(
                "More than one reader joined while waiting ({listed}), so this will not\n\
                 guess which is yours. 'kobo devices' asks each one what it is."
            );
        }
        setup::Arrival::TimedOut(passed_over) => {
            println!(
                "The reader did not appear. It is set up either way, the files are on it\n\
                 and its SSH server starts at the next boot. 'kobo devices' finds it once\n\
                 it is awake and on Wi-Fi."
            );
            if !passed_over.is_empty() {
                let listed = passed_over
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ");
                println!(
                    "\nSomething else joined the network while waiting ({listed}) and was\n\
                     passed over: each was asked what it was, and none of them is a reader."
                );
            }
        }
    }
}

/// What a run would do, without doing any of it.
///
/// A pure function of the options and the reader, so that what `--dry-run`
/// promises can be tested rather than read.
fn dry_run_plan(options: &SetupOptions, reader: &setup::Mounted) -> String {
    // Asked of the reader in front of us rather than assumed. A plan that
    // promised to stage NickelMenu on a device that already has it described
    // an archive that was never going to be written, and then described the
    // key as sharing it.
    let plugin_installed = menu::installed(&reader.volume);
    let would_stage = match options.menu {
        MenuEntry::Force => true,
        MenuEntry::Add => !plugin_installed,
        MenuEntry::Skip => false,
    };
    let trust_plan = describe_trust_plan();
    let keys = setup::SETTINGS_APPLIED
        .iter()
        .map(|(section, key, value)| format!("{section}/{key}={value}"))
        .collect::<Vec<_>>()
        .join(", ");
    if options.mode == SetupMode::Undo {
        return format!(
            "would remove {}/{}\n\
             would disable the firmware's SSH server by renaming {} back\n\
             would clear {}\n\
             would remove {} and ask NickelMenu to uninstall itself, unless another\n\
             \x20 mod still has a configuration file beside it\n\
             nothing else on the reader is touched",
            reader.volume.display(),
            setup::INSTALL_FOLDER,
            setup::SSH_ENABLED,
            keys,
            menu::CONFIG,
        );
    }
    format!(
        "would install Cobalt into {}/{}\n\
         {}\n\
         would set {keys}\n\
         {trust_plan}\n\
         {}\n\
         {}\n\
         would eject, then {}\n\
         nothing outside the book partition{}",
        reader.volume.display(),
        setup::INSTALL_FOLDER,
        if options.enable_ssh {
            format!(
                "would create or reuse ~/.ssh/{DEVICE_KEY_NAME}, stage only its public half for Cobalt to install on first launch, and enable the firmware's root SSH server by renaming {}",
                setup::SSH_DISABLED
            )
        } else {
            "would leave the firmware's SSH server disabled (pass --enable-ssh to opt in)"
                .to_owned()
        },
        if options.menu == MenuEntry::Skip {
            "would add no menu entry, because --no-menu was given".to_owned()
        } else if !would_stage {
            if menu::marker_stale(&reader.volume) {
                format!(
                    "would write only a Cobalt entry to {}. It would not write or reinstall\n\
                     \x20 any NickelMenu files or stage a KoboRoot.tgz. NickelMenu's own files\n\
                     \x20 predate the last firmware update, though, and an update removes the\n\
                     \x20 plugin: pass --menu to stage it again",
                    menu::CONFIG,
                )
            } else {
                format!(
                    "would write only a Cobalt entry to {}. It would not write or reinstall\n\
                     \x20 any NickelMenu files or stage a KoboRoot.tgz, because NickelMenu is\n\
                     \x20 already installed on this reader",
                    menu::CONFIG,
                )
            }
        } else {
            format!(
                "would write a Cobalt entry to {}, and stage NickelMenu {} in {}\n\
                 \x20 for the firmware to extract, after checking that the archive contains\n\
                 \x20 nothing but {}",
                menu::CONFIG,
                menu::VERSION,
                menu::KOBOROOT,
                menu::ARCHIVE_MEMBERS.join(" and "),
            )
        },
        describe_key_plan(options, would_stage),
        if options.enable_ssh && options.wait {
            "wait for the restarted reader to appear on the network"
        } else if !options.enable_ssh {
            "stop after ejecting, because SSH was not enabled"
        } else {
            "stop, because --no-wait was given"
        },
        match (
            would_stage,
            options.enable_ssh && options.authorize_key,
        ) {
            (true, true) => ", and nothing extracted as root but NickelMenu's own two files and one authorized_keys",
            (true, false) => ", and nothing extracted as root but NickelMenu's own two files",
            (false, true) => ", and nothing extracted as root but one authorized_keys",
            (false, false) => ", nothing extracted as root",
        }
    )
}

/// The one line of the dry run that covers this machine's trust roots.
fn describe_trust_plan() -> String {
    let names = setup::host_trust_names();
    if names.is_empty() {
        "would carry no trust roots, because ~/.config/kobo/trust holds none".to_owned()
    } else {
        format!(
            "would copy this machine's trust roots ({}) into {}/trust",
            names.join(", "),
            setup::INSTALL_FOLDER,
        )
    }
}

/// The one line of the dry run that covers this machine's key.
fn describe_key_plan(options: &SetupOptions, would_stage: bool) -> String {
    if !options.enable_ssh {
        return "would install no key, because there is no SSH server to use it".to_owned();
    }
    if !options.authorize_key {
        return "would install no key, because --no-key was given".to_owned();
    }
    let slot = if would_stage {
        format!(
            "into the same {} the menu plugin is staged in",
            menu::KOBOROOT
        )
    } else {
        format!("into {}", menu::KOBOROOT)
    };
    format!(
        "would put this machine's public key {slot}, creating\n\
         \x20 ~/.ssh/{} first if it does not exist, so that 'kobo devices' and every\n\
         \x20 other device command can reach the reader without a password. This replaces\n\
         \x20 any keys the reader already accepts, because USB cannot read that file back",
        authorize::KEY_NAME,
    )
}

/// One line for what the undo did to the reader's own menus.
fn describe_unmenu(removed: menu::Removed) -> String {
    let entry = match (removed.entry, removed.unstaged) {
        (true, true) => "menu entry removed, and the staged NickelMenu archive taken back before \
                         the reader could extract it"
            .to_owned(),
        (true, false) => "menu entry removed".to_owned(),
        (false, true) => "the staged NickelMenu archive was taken back".to_owned(),
        (false, false) => return "there was no menu entry".to_owned(),
    };
    match removed.plugin {
        menu::Plugin::Absent => entry,
        menu::Plugin::Flagged => format!(
            "{entry}; NickelMenu will uninstall itself at the next restart ({})",
            menu::UNINSTALL_FLAG
        ),
        menu::Plugin::Shared => {
            format!("{entry}; NickelMenu kept, because another mod is still configured to use it")
        }
    }
}

/// Puts a reader back to how it shipped.
fn undo_setup(reader: &setup::Mounted, eject: bool) -> Result<(), String> {
    let removed = setup::remove_payload(&reader.volume)?;
    let ssh = setup::disable_ssh(&reader.volume)?;
    let settings = setup::revert_settings(&reader.volume)?;
    let unmenued = menu::remove(&reader.volume)?;
    let ejected = ejected_or_explained(&reader.volume, eject);

    println!(
        "\nUndone on {}:\n  · {}\n  · {}\n  · {}\n  · {}\n  · {}",
        reader.volume.display(),
        if removed {
            "Cobalt removed"
        } else {
            "Cobalt was not installed"
        },
        if ssh {
            "SSH disabled again (takes effect at the next restart)"
        } else {
            "SSH was not enabled by this tool"
        },
        if settings.is_empty() {
            "no settings to restore".to_owned()
        } else {
            format!("settings removed: {}", settings.join(", "))
        },
        describe_unmenu(unmenued),
        if ejected {
            "volume ejected"
        } else {
            "volume left mounted"
        }
    );
    // Said plainly rather than left for somebody to discover. The book
    // partition is all this command can reach over USB, and a key the reader
    // has already extracted lives on the root filesystem, so an undo cannot
    // reach it.
    println!(
        "\nA key this tool staged is taken back with the archive it was in. One the\n\
         reader has already extracted stays in authorized_keys under root's home\n\
         directory (/.ssh on the i.MX6 readers, /root/.ssh elsewhere), which is\n\
         on the root filesystem, and USB does not reach it. To remove that one, edit\n\
         the file over SSH and delete the line ending in 'kobo-cobalt'."
    );
    Ok(())
}

/// Ejects, or says why it could not, without failing the whole command.
///
/// Everything is already written by this point. An eject that fails because a
/// shell is sitting in a directory on the volume is worth reporting and not
/// worth undoing an install over.
fn ejected_or_explained(volume: &Path, wanted: bool) -> bool {
    if !wanted {
        return false;
    }
    match setup::eject(volume) {
        Ok(()) => true,
        Err(error) => {
            eprintln!("everything was written, but the volume did not eject: {error}");
            false
        }
    }
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
        [device, host, rest @ ..] if is_device_flag(device) => (host.as_str(), rest),
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
    let root = Path::new(package::INSTALL_ROOT);
    listed
        .iter()
        .filter(|entry| {
            let path = Path::new(entry.path.trim_end_matches('/'));
            !(path.starts_with(root) || root.starts_with(path))
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

fn run_simulation(arguments: &[String]) -> Result<(), String> {
    let package = simulated_package(arguments)?;
    let mut build = Command::new("cargo");
    build.args(["build", "-p", "kobod", "-p", package]);
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

    let app_status = Command::new(workspace_host_binary(package))
        .env("KOBO_SOCKET", &simulation.socket)
        .env("KOBO_SIM_ONESHOT", "1")
        .status()
        .map_err(|error| format!("run {package}: {error}"))?;
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
    println!(
        "host runtime completed for {package}; frame: {}",
        output.display()
    );
    Ok(())
}

/// The package `--app` named, checked against built-in and Store applications.
///
/// Restricted to that list rather than taking any string, because the name
/// becomes both a cargo argument and a path under `target/debug`, and because
/// a typo is worth a list of what exists rather than a build failure four
/// minutes later. `kobod` is on that list and is not an application: it is the
/// runtime the simulation is already starting.
fn simulated_package(arguments: &[String]) -> Result<&'static str, String> {
    let mut wanted = "todo";
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--app" | "-a" => {
                wanted = arguments
                    .get(index + 1)
                    .ok_or_else(|| format!("--app needs a name; one of {}", simulatable()))?;
                index += 1;
            }
            other => {
                return Err(format!(
                    "unknown option '{other}'\nusage: kobo run --sim [--app NAME]"
                ))
            }
        }
        index += 1;
    }
    // Both spellings work, because the launcher calls it 'rss' and cargo calls
    // it 'kobo-rss', and somebody reading either should not have to know.
    INSTALLED_PACKAGES
        .iter()
        .map(|(package, _)| *package)
        .chain(STORE_PACKAGES.iter().copied())
        .find(|package| {
            *package != "kobod"
                && (*package == wanted || package.strip_prefix("kobo-") == Some(wanted))
        })
        .ok_or_else(|| format!("no application called '{wanted}'; one of {}", simulatable()))
}

/// The application names `--app` accepts, for an error message to list.
fn simulatable() -> String {
    let names = INSTALLED_PACKAGES
        .iter()
        .filter_map(|(package, _)| package.strip_prefix("kobo-"))
        .chain(
            STORE_PACKAGES
                .iter()
                .filter_map(|package| package.strip_prefix("kobo-")),
        )
        .collect::<BTreeSet<_>>();
    names.into_iter().collect::<Vec<_>>().join(", ")
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
        let daemon = Command::new(workspace_host_binary("kobod"))
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

/// The C cross-compiler `ring` needs to build its own sources for the reader.
///
/// Several distributions and taps spell the same toolchain differently, so
/// every name in use is tried before the build is refused.
fn find_device_cc() -> Result<String, String> {
    const NAMES: [&str; 4] = [
        "armv7-unknown-linux-musleabihf-gcc",
        "armv7-linux-musleabihf-gcc",
        "arm-linux-musleabihf-gcc",
        "arm-linux-gnueabihf-gcc",
    ];
    for name in NAMES {
        let found = Command::new(name)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        if found {
            return Ok(name.to_owned());
        }
    }
    Err(format!(
        "no ARM C cross-compiler was found, and one is needed because the TLS \
         stack builds C for the reader. Tried: {}.\n  macOS:  brew install \
         messense/macos-cross-toolchains/armv7-unknown-linux-musleabihf\n  \
         Debian: sudo apt-get install gcc-arm-linux-gnueabihf\nSet \
         CC_armv7_unknown_linux_musleabihf to override.",
        NAMES.join(", ")
    ))
}

fn find_device_ar() -> Result<String, String> {
    const NAMES: [&str; 4] = [
        "armv7-unknown-linux-musleabihf-ar",
        "armv7-linux-musleabihf-ar",
        "arm-linux-musleabihf-ar",
        "arm-linux-gnueabihf-ar",
    ];
    for name in NAMES {
        let found = Command::new(name)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        if found {
            return Ok(name.to_owned());
        }
    }
    Err(format!(
        "no ARM cross-archiver was found, and one is needed for C dependencies. \
         Tried: {}.\nSet AR_armv7_unknown_linux_musleabihf to override.",
        NAMES.join(", ")
    ))
}

fn verify_arm_elf(path: &Path) -> Result<(), String> {
    let bytes = fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    if bytes.len() < 52 || &bytes[..4] != b"\x7fELF" {
        return Err(format!("{} is not an ELF binary", path.display()));
    }
    if bytes[4] != 1 || bytes[5] != 1 {
        return Err("expected a little-endian ELF32 binary".to_owned());
    }
    if read_u16(&bytes, 16)? != 2 {
        return Err("expected an executable ELF file".to_owned());
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
    let entry = read_u32(&bytes, 24)?;
    let mut executable_entry = false;
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
        if kind == 1 {
            let file_offset = usize::try_from(read_u32(&bytes, offset + 4)?)
                .map_err(|_| "load segment offset overflow")?;
            let virtual_address = read_u32(&bytes, offset + 8)?;
            let file_size = usize::try_from(read_u32(&bytes, offset + 16)?)
                .map_err(|_| "load segment size overflow")?;
            let memory_size = read_u32(&bytes, offset + 20)?;
            let segment_flags = read_u32(&bytes, offset + 24)?;
            if file_size > usize::try_from(memory_size).unwrap_or(usize::MAX)
                || file_offset
                    .checked_add(file_size)
                    .is_none_or(|end| end > bytes.len())
            {
                return Err("invalid ELF load segment".to_owned());
            }
            if segment_flags & 1 != 0
                && entry >= virtual_address
                && entry < virtual_address.saturating_add(memory_size)
            {
                executable_entry = true;
            }
        }
    }
    if !executable_entry {
        return Err("ELF entry point is not inside an executable load segment".to_owned());
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

/// Drives a running simulator from a script, and brings the panel back as PNG.
///
/// With `--record` it also brings it back as a moving picture. That lives here
/// rather than in `kobo record` because the division between the two is
/// deliberate and documented: `drive` is the simulator and `record` is the
/// device. `record` is a framebuffer reader -- it uploads the doctor, has the
/// device write `/dev/fb0` into a file, and pulls that file home over SSH --
/// and there is no framebuffer on this side to point it at. What there is, is
/// a driver already holding a connection to the process doing the rendering,
/// which is the only thing that knows when a screen has changed.
fn drive_command(arguments: &[String]) -> Result<(), String> {
    let mut address = "127.0.0.1:8787".to_owned();
    let mut shots = PathBuf::from("target/kobo-shots");
    let mut script: Option<String> = None;
    let mut steps: Vec<String> = Vec::new();
    let mut ideal = false;
    let mut record: Option<PathBuf> = None;
    let mut fps = drive::DEFAULT_RECORD_FPS;
    let mut ghosting = false;
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--address" => {
                address.clone_from(
                    arguments
                        .get(index + 1)
                        .ok_or("--address needs host:port")?,
                );
                index += 1;
            }
            "--shots" => {
                shots = PathBuf::from(arguments.get(index + 1).ok_or("--shots needs a folder")?);
                index += 1;
            }
            "--script" => {
                let path = arguments.get(index + 1).ok_or("--script needs a path")?;
                script = Some(
                    fs::read_to_string(path).map_err(|error| format!("read {path}: {error}"))?,
                );
                index += 1;
            }
            "--step" => {
                steps.push(
                    arguments
                        .get(index + 1)
                        .ok_or("--step needs a step")?
                        .clone(),
                );
                index += 1;
            }
            "--ideal" => ideal = true,
            "--record" => {
                record = Some(PathBuf::from(
                    arguments.get(index + 1).ok_or("--record needs a folder")?,
                ));
                index += 1;
            }
            "--fps" => {
                fps = arguments
                    .get(index + 1)
                    .ok_or("--fps needs a rate")?
                    .parse()
                    .map_err(|_| "--fps takes a whole number".to_owned())?;
                index += 1;
            }
            "--ghosting" => ghosting = true,
            other => return Err(format!("unknown option '{other}'\n{DRIVE_USAGE}")),
        }
        index += 1;
    }
    if script.is_none() && steps.is_empty() {
        return Err(DRIVE_USAGE.to_owned());
    }
    // Before the simulator is touched, because the frames are only half of
    // what `--record` was asked for and finding out at the end costs the whole
    // run. `kobo record --device` is deliberately softer about this: there the
    // numbered PNGs are the product and the video is a bonus.
    if record.is_some() && !ffmpeg_is_available() {
        return Err(FFMPEG_MISSING.to_owned());
    }
    let recorder = record
        .as_ref()
        .map(|_| drive::Recorder::start(&address, fps, ghosting))
        .transpose()?;

    let mut driver = drive::Driver::new(&address, &shots).ideal(ideal);
    let outcome = script
        .map_or(Ok(()), |script| driver.run_script(&script))
        .and_then(|()| driver.run_script(&steps.join("\n")));

    // Written even when a step failed. A recording of the run that went wrong
    // is the recording worth having, and throwing it away to report the
    // failure a second later would be the wrong way round.
    if let (Some(recorder), Some(directory)) = (recorder, record.as_ref()) {
        match recorder.finish() {
            Ok(recording) => write_recording(directory, &recording)?,
            Err(error) if outcome.is_ok() => return Err(error),
            Err(error) => eprintln!("warning: nothing was recorded: {error}"),
        }
    }
    outcome?;

    println!(
        "drive: every step passed; screenshots in {}",
        shots.display()
    );
    Ok(())
}

/// Taps the real glass at a point, so the whole input path is exercised.
#[cfg(feature = "device-write")]
fn tap_command(arguments: &[String]) -> Result<(), String> {
    let mut host: Option<String> = None;
    let mut steps: Vec<String> = Vec::new();
    let mut millis = 0_u64;
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--device" => {
                let value = arguments.get(index + 1).ok_or("--device needs a host")?;
                if !valid_device_host(value) {
                    return Err(format!("'{value}' is not a usable device host"));
                }
                host = Some(value.clone());
                index += 1;
            }
            other => {
                millis = millis
                    .checked_add(parse_tap_step(other)?)
                    .ok_or("that sequence waits longer than any run will")?;
                steps.push(other.to_owned());
            }
        }
        index += 1;
    }
    let host = host.ok_or_else(|| TAP_USAGE.to_owned())?;
    if steps.is_empty() {
        return Err(TAP_USAGE.to_owned());
    }
    run_remote_fixed_artifact(&host, &RemoteArtifact::tap(steps.join(" "), millis))
}

/// Checks one step of a sequence and returns the wait it asks for.
///
/// The points are checked again on the device, against the profile that
/// actually matched, which is the check that counts. This one exists so that a
/// typo in a long sequence is a message here rather than a build, an upload and
/// a checksum away.
#[cfg(feature = "device-write")]
fn parse_tap_step(step: &str) -> Result<u64, String> {
    let (wait, point) = match step.split_once(':') {
        Some((wait, point)) => (
            wait.trim()
                .parse::<u64>()
                .map_err(|_| format!("'{step}' does not start with a wait in milliseconds"))?,
            point,
        ),
        None => (0, step),
    };
    let (x, y) = point
        .split_once(',')
        .ok_or_else(|| format!("expected 'x,y' or 'wait:x,y', got '{step}'\n{TAP_USAGE}"))?;
    x.trim()
        .parse::<u32>()
        .map_err(|_| TAP_USAGE.to_owned())
        .and_then(|_| y.trim().parse::<u32>().map_err(|_| TAP_USAGE.to_owned()))?;
    Ok(wait)
}

#[cfg(feature = "device-write")]
const TAP_USAGE: &str = "usage: kobo tap --device HOST X,Y [MILLIS:X,Y ...]\n\
                         a step is a point, or a wait in milliseconds and then a point.\n\
                         several steps run in one upload, timed on the device.";

/// Brings back a picture of whatever is on the panel right now.
///
/// Two sources, one command, because the question is the same either way:
/// what does this actually look like. `--device` photographs the real e-ink
/// panel over SSH; with no `--device` it takes the frame from a running
/// simulator, which paints with the same renderer and the same refresh
/// planner.
fn shot_command(arguments: &[String]) -> Result<(), String> {
    let mut host: Option<String> = None;
    let mut address = "127.0.0.1:8787".to_owned();
    let mut output = PathBuf::from("kobo-shot.png");
    let mut ideal = false;
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--device" => {
                let value = arguments.get(index + 1).ok_or("--device needs a host")?;
                if !valid_device_host(value) {
                    return Err(format!("'{value}' is not a usable device host"));
                }
                host = Some(value.clone());
                index += 1;
            }
            "--address" => {
                address.clone_from(
                    arguments
                        .get(index + 1)
                        .ok_or("--address needs host:port")?,
                );
                index += 1;
            }
            "--out" => {
                output = PathBuf::from(arguments.get(index + 1).ok_or("--out needs a path")?);
                index += 1;
            }
            "--ideal" => ideal = true,
            other => return Err(format!("unknown option '{other}'\n{SHOT_USAGE}")),
        }
        index += 1;
    }
    let (width, height, grey) = if let Some(host) = host {
        let transcript = capture_remote_fixed_artifact(&host, &RemoteArtifact::capture())?;
        drive::decode_capture(&transcript)?
    } else {
        let driver = drive::Driver::new(&address, Path::new(".")).ideal(ideal);
        let (width, height) = drive::SIMULATED_PANEL;
        (width, height, driver.frame()?)
    };
    let png = kobo_image::encode_png_grey(width, height, &grey)
        .map_err(|error| format!("encode the panel: {error}"))?;
    fs::write(&output, png).map_err(|error| format!("write {}: {error}", output.display()))?;
    println!("shot {} ({width}x{height})", output.display());
    Ok(())
}

/// Where the doctor leaves a recording, and where the host looks for it.
const RECORDING_ON_DEVICE: &str = "/mnt/onboard/.kobo-record.bin";

/// Long enough to carry a recording home over the reader's radio, which is the
/// slowest thing in this loop by a wide margin.
const RECORDING_TRANSFER_TIMEOUT: Duration = Duration::from_secs(300);

const RECORD_USAGE: &str = "usage: kobo record --device HOST [--seconds N] [--fps F] \
                            [--out DIR] [--keep-on-device]";

/// Records the panel while somebody, or something, drives the reader.
///
/// The still picture's sibling. `kobo shot` answers what the screen looks
/// like; this answers what it did, which is the question whenever a tap lands
/// somewhere unexpected, a screen flashes through a wrong state before
/// settling, or a refresh leaves ink behind.
///
/// Read-only on the device, exactly like `kobo shot`: it opens the framebuffer
/// for reading and never grabs, refreshes or writes, so it can watch our own
/// application or the stock reader without changing either.
fn record_command(arguments: &[String]) -> Result<(), String> {
    let mut host: Option<String> = None;
    let mut seconds = 20_u64;
    let mut fps = 2_u32;
    let mut output = PathBuf::from("kobo-recording");
    let mut keep = false;
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            flag if is_device_flag(flag) => {
                let value = arguments.get(index + 1).ok_or("--device needs a host")?;
                if !valid_device_host(value) {
                    return Err(format!("'{value}' is not a usable device host"));
                }
                host = Some(value.clone());
                index += 1;
            }
            "--seconds" => {
                seconds = arguments
                    .get(index + 1)
                    .ok_or("--seconds needs a count")?
                    .parse()
                    .map_err(|_| "--seconds takes a whole number".to_owned())?;
                index += 1;
            }
            "--fps" => {
                fps = arguments
                    .get(index + 1)
                    .ok_or("--fps needs a rate")?
                    .parse()
                    .map_err(|_| "--fps takes a whole number".to_owned())?;
                index += 1;
            }
            "--out" => {
                output = PathBuf::from(arguments.get(index + 1).ok_or("--out needs a path")?);
                index += 1;
            }
            "--keep-on-device" => keep = true,
            other => return Err(format!("unknown option '{other}'\n{RECORD_USAGE}")),
        }
        index += 1;
    }
    let host = host.ok_or(RECORD_USAGE)?;
    println!("recording {seconds}s at {fps} fps from {host}; drive the reader now");
    let transcript = capture_remote_fixed_artifact(&host, &RemoteArtifact::record(seconds, fps))?;
    let summary = transcript
        .lines()
        .find_map(|line| line.strip_prefix("record-written "))
        .ok_or("the device did not report a recording")?;
    println!(
        "device kept {} frames",
        summary.split(' ').nth(1).unwrap_or("?")
    );

    let raw = pull_recording(&host)?;
    let frames = decode_recording(&raw)?;
    write_recording(&output, &frames)?;
    if !keep {
        // A megabyte-a-frame file left in the library shows up as a broken
        // book on the reader's home screen, so it goes as soon as it is home.
        let _ = run_remote_shell(
            &format!("root@{host}"),
            &format!("rm -f '{RECORDING_ON_DEVICE}'\n"),
            REMOTE_COMMAND_TIMEOUT,
        );
    }
    Ok(())
}

/// Brings the recording home, compressed on the way.
///
/// Gzipped by the device rather than sent raw: a frame is flat white over most
/// of its area and compresses to a fraction of its size, and this crosses
/// Wi-Fi from a reader whose radio is the slowest thing in the loop.
fn pull_recording(host: &str) -> Result<Vec<u8>, String> {
    let output = run_remote_shell(
        &format!("root@{host}"),
        &format!("gzip -c < '{RECORDING_ON_DEVICE}'\n"),
        RECORDING_TRANSFER_TIMEOUT,
    )
    .map_err(unreachable_device)?;
    if !output.status.success() {
        return Err(format!(
            "fetch the recording: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    gunzip(&output.stdout)
}

/// One recorded frame: when it appeared, and what was on the panel.
struct RecordedFrame {
    millis: u32,
    grey: Vec<u8>,
}

/// Reads the device's recording format.
///
/// Deliberately strict. A truncated recording is a real possibility, because
/// the device can be unplugged or run out of room mid-write, and half a frame
/// decoded as a whole one would be a picture of nothing that looks like a
/// rendering bug.
fn decode_recording(raw: &[u8]) -> Result<(u32, u32, Vec<RecordedFrame>), String> {
    const MAGIC: &[u8; 8] = b"KOBOCST1";
    if raw.len() < 16 || &raw[..8] != MAGIC {
        return Err("this is not a recording written by this version".to_owned());
    }
    let width = u32::from_le_bytes([raw[8], raw[9], raw[10], raw[11]]);
    let height = u32::from_le_bytes([raw[12], raw[13], raw[14], raw[15]]);
    let pixels = usize::try_from(width)
        .ok()
        .and_then(|width| width.checked_mul(usize::try_from(height).ok()?))
        .filter(|pixels| *pixels > 0)
        .ok_or("the recording claims a panel of no size")?;
    let mut frames = Vec::new();
    let mut at = 16;
    while at + 4 + pixels <= raw.len() {
        let millis = u32::from_le_bytes([raw[at], raw[at + 1], raw[at + 2], raw[at + 3]]);
        frames.push(RecordedFrame {
            millis,
            grey: raw[at + 4..at + 4 + pixels].to_vec(),
        });
        at += 4 + pixels;
    }
    if at != raw.len() {
        eprintln!(
            "warning: {} trailing bytes; the recording was cut short",
            raw.len() - at
        );
    }
    if frames.is_empty() {
        return Err("the recording holds no frames".to_owned());
    }
    Ok((width, height, frames))
}

/// Writes the recording out as numbered pictures, and a video if one can be
/// made.
///
/// Numbered PNGs are the product, not a fallback. They are what a reviewer
/// actually opens, they diff, and they need nothing installed. A video is
/// offered on top when ffmpeg happens to be on the path, because scrubbing is
/// the better way to watch a transition.
fn write_recording(
    directory: &Path,
    (width, height, frames): &(u32, u32, Vec<RecordedFrame>),
) -> Result<(), String> {
    fs::create_dir_all(directory)
        .map_err(|error| format!("create {}: {error}", directory.display()))?;
    for (index, frame) in frames.iter().enumerate() {
        let png = kobo_image::encode_png_grey(*width, *height, &frame.grey)
            .map_err(|error| format!("encode frame {index}: {error}"))?;
        let path = directory.join(format!("frame-{index:04}.png"));
        fs::write(&path, png).map_err(|error| format!("write {}: {error}", path.display()))?;
    }
    let mut timings = String::new();
    for (index, frame) in frames.iter().enumerate() {
        let _ = writeln!(timings, "frame-{index:04}.png {}", frame.millis);
    }
    let index_path = directory.join("timings.txt");
    fs::write(&index_path, timings)
        .map_err(|error| format!("write {}: {error}", index_path.display()))?;
    println!(
        "recorded {} frames ({width}x{height}) into {}",
        frames.len(),
        directory.display()
    );
    match write_recording_video(directory, frames) {
        Ok(Some(path)) => println!("video {}", path.display()),
        Ok(None) => println!("ffmpeg is not on the path, so no video was made"),
        Err(error) => eprintln!("warning: the pictures are fine but the video failed: {error}"),
    }
    Ok(())
}

/// Whether ffmpeg can be run at all.
///
/// Asked before a run rather than after it wherever the moving picture is the
/// point of the run. Discovering that ffmpeg is missing at the end of a script
/// that took a minute to drive is a minute nobody gets back.
fn ffmpeg_is_available() -> bool {
    std::process::Command::new("ffmpeg")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// What to say when it is not there.
const FFMPEG_MISSING: &str = "ffmpeg is not on the path, and a recording is assembled with it.\n\
                              install it with: brew install ffmpeg (macOS) \
                              or apt install ffmpeg (Debian)";

/// The concat list ffmpeg is fed, holding each frame and how long it was up.
///
/// The frames are not evenly spaced, because only the ones that changed were
/// kept, so a list carrying each frame's real duration is used rather than a
/// fixed rate. Otherwise a screen held for ten seconds would flash past in the
/// same time as one held for a tenth of a second.
fn concat_list(frames: &[RecordedFrame]) -> String {
    let mut list = String::new();
    for (index, frame) in frames.iter().enumerate() {
        let next = frames
            .get(index + 1)
            .map_or(frame.millis + 1000, |frame| frame.millis);
        let seconds = f64::from(next.saturating_sub(frame.millis)).max(100.0) / 1000.0;
        let _ = writeln!(list, "file 'frame-{index:04}.png'\nduration {seconds:.3}");
    }
    // ffmpeg's concat demuxer ignores the last duration, so the final frame is
    // named twice to give it one.
    if let Some(index) = frames.len().checked_sub(1) {
        let _ = writeln!(list, "file 'frame-{index:04}.png'");
    }
    list
}

/// Turns the frames into an mp4 and a looping GIF, if ffmpeg is available.
///
/// Two files because they are read in two places. The mp4 is what you scrub
/// through when you are looking for the frame where it went wrong. The GIF is
/// what goes in a README: a repository path in an `<img>` renders everywhere
/// and a `<video>` element does not, and a demo that waits to be clicked is a
/// demo nobody sees.
fn write_recording_video(
    directory: &Path,
    frames: &[RecordedFrame],
) -> Result<Option<PathBuf>, String> {
    if !ffmpeg_is_available() {
        return Ok(None);
    }
    let list_path = directory.join("frames.txt");
    fs::write(&list_path, concat_list(frames))
        .map_err(|error| format!("write {}: {error}", list_path.display()))?;
    let video = directory.join("recording.mp4");
    let mut encode = std::process::Command::new("ffmpeg");
    encode
        .args(["-nostdin", "-y", "-loglevel", "error"])
        .args(["-f", "concat", "-safe", "0", "-i"])
        .arg(&list_path)
        // Even dimensions, because h264 refuses odd ones and 1072x1448 is
        // only even by luck.
        .args([
            "-vf",
            "pad=ceil(iw/2)*2:ceil(ih/2)*2",
            "-pix_fmt",
            "yuv420p",
            "-r",
            "10",
        ])
        .arg(&video);
    run_ffmpeg(encode)?;
    // Built from the mp4 rather than from the frames again, the same way
    // `cut-tour.py` builds the one on the front page, so the two are the same
    // picture at the same width with the same palette.
    let loop_path = directory.join("recording.gif");
    let mut looping = std::process::Command::new("ffmpeg");
    looping
        .args(["-nostdin", "-y", "-loglevel", "error", "-i"])
        .arg(&video)
        .args([
            "-vf",
            "scale=600:-1:flags=lanczos,fps=12,split[a][b];\
             [a]palettegen=max_colors=64[p];\
             [b][p]paletteuse=dither=bayer:bayer_scale=3",
        ])
        .arg(&loop_path);
    run_ffmpeg(looping)?;
    println!("loop {}", loop_path.display());
    Ok(Some(video))
}

fn run_ffmpeg(command: std::process::Command) -> Result<(), String> {
    let mut command = command;
    let output = command
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .map_err(|error| format!("run ffmpeg: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if stderr.is_empty() {
        Err("ffmpeg refused the frames".to_owned())
    } else {
        Err(format!("ffmpeg refused the frames: {stderr}"))
    }
}

const SHOT_USAGE: &str =
    "usage: kobo shot [--device HOST | --address host:port] [--out PATH] [--ideal]";

const DRIVE_USAGE: &str = "usage: kobo drive [--address host:port] [--shots DIR] [--ideal]\n\
                           \u{20}                 [--record DIR [--fps N] [--ghosting]]\n\
                           \u{20}                 (--script PATH | --step 'tap Search' ...)\n\
                           steps: tap LABEL | tap-at X,Y | type TEXT | shot NAME | expect TEXT\n\
                           \u{20}       expect-missing TEXT | wait-for TEXT | clean | dump\n\
                           \u{20}       lifecycle foreground|background | scenario NAME | wait MS\n\
                           --record films the panel while the script runs and writes numbered\n\
                           \u{20} PNGs, timings.txt, recording.mp4 and recording.gif into DIR.\n\
                           \u{20} Frames are residue-free unless --ghosting asks for the real\n\
                           \u{20} e-ink refresh; --ideal governs the `shot` steps as before.";

/// Where the runtime reads named secrets from, mirrored from `kobod`.
const DEVICE_SECRETS_DIRECTORY: &str = "/mnt/onboard/.adds/cobalt/secrets";

/// The largest credential the runtime will read back, from `kobo-policy`.
const SECRET_MAXIMUM_BYTES: usize = 4096;

/// The heredoc delimiter the install script uses.
///
/// A credential is written through the shell, so it must not be possible for
/// the credential itself to end the document early. Any value containing this
/// line is refused rather than truncated.
const SECRET_DELIMITER: &str = "COBALT_SECRET_VALUE_ENDS_HERE";

#[derive(Debug, Eq, PartialEq)]
enum SecretAction {
    Set { name: String, source: PathBuf },
    List,
    Remove { name: String },
}

#[derive(Debug, Eq, PartialEq)]
enum SecretTarget {
    Device(String),
    Volume(PathBuf),
}

const SECRET_USAGE: &str =
    "usage: kobo secret set <name> [--from PATH] (--device IP | --volume PATH)\n\
                            \x20      kobo secret list (--device IP | --volume PATH)\n\
                            \x20      kobo secret remove <name> (--device IP | --volume PATH)";

/// Accepts the names the runtime will actually resolve.
///
/// The same rule as `kobo_policy::tasks::secret`, applied here so a name that
/// could never be read back is refused at the point it is typed rather than
/// installed and silently ignored.
fn valid_secret_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// Where a credential is looked for when `--from` is not given.
///
/// A key already on this machine is the common case, and asking for its path
/// every time is the kind of friction this SDK exists to remove. The order is
/// most specific first: an explicit secrets directory, then the SDK's own
/// configuration, then the dotfile people actually keep.
fn secret_source_candidates(name: &str) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(directory) = std::env::var_os("KOBO_SECRETS_DIR") {
        candidates.push(PathBuf::from(directory).join(name));
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        candidates.push(
            home.join(".config")
                .join("cobalt")
                .join("secrets")
                .join(name),
        );
        candidates.push(home.join(".config").join("kobo").join("secrets").join(name));
        candidates.push(home.join(format!(".{name}")));
    }
    candidates
}

fn find_secret_source(name: &str) -> Result<PathBuf, String> {
    let candidates = secret_source_candidates(name);
    for candidate in &candidates {
        if candidate.is_file() {
            return Ok(candidate.clone());
        }
    }
    let looked = candidates
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Err(format!(
        "no file holds the '{name}' credential; pass --from PATH, or put it in one of: {looked}"
    ))
}

/// Reads a credential off this machine and checks the runtime could read it back.
fn read_secret_file(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    if bytes.len() > SECRET_MAXIMUM_BYTES {
        return Err(format!(
            "{} holds {} bytes; the runtime refuses anything over {SECRET_MAXIMUM_BYTES}",
            path.display(),
            bytes.len()
        ));
    }
    let value = String::from_utf8(bytes).map_err(|_| format!("{} is not text", path.display()))?;
    let value = normalise_secret_value(value.trim());
    if value.is_empty() {
        return Err(format!("{} is empty", path.display()));
    }
    if value.lines().any(|line| line.trim() == SECRET_DELIMITER) {
        return Err("the credential contains the delimiter this command writes with".to_owned());
    }
    // A key pasted with its shell assignment still reads as a key to a person
    // and as forty wrong characters to the server, so it is caught here.
    if let Some((before, _)) = value.split_once('=') {
        if !before.contains(char::is_whitespace)
            && before
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte == b'_')
            && !before.is_empty()
        {
            return Err(format!(
                "{} looks like a shell assignment ({before}=...); store the value on its own",
                path.display()
            ));
        }
    }
    Ok(value)
}

/// Accepts both a raw key and the common one-line `NAME=value` dotfile shape.
/// The name is discarded locally; only the value is ever installed.
fn normalise_secret_value(value: &str) -> String {
    let Some((name, assigned)) = value.split_once('=') else {
        return value.to_owned();
    };
    let assignment = !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
        && !assigned.contains('\n');
    if !assignment {
        return value.to_owned();
    }
    let assigned = assigned.trim();
    if assigned.len() >= 2
        && ((assigned.starts_with('"') && assigned.ends_with('"'))
            || (assigned.starts_with('\'') && assigned.ends_with('\'')))
    {
        assigned[1..assigned.len() - 1].to_owned()
    } else {
        assigned.to_owned()
    }
}

fn parse_secret(arguments: &[String]) -> Result<(SecretAction, SecretTarget), String> {
    let (verb, rest) = arguments
        .split_first()
        .ok_or_else(|| SECRET_USAGE.to_owned())?;
    let (name, rest) = match verb.as_str() {
        "set" | "remove" => {
            let (name, rest) = rest.split_first().ok_or_else(|| SECRET_USAGE.to_owned())?;
            if !valid_secret_name(name) {
                return Err(
                    "a secret name is letters, digits, '-' and '_', up to 64 characters".to_owned(),
                );
            }
            (Some(name.clone()), rest)
        }
        "list" => (None, rest),
        _ => return Err(SECRET_USAGE.to_owned()),
    };
    let mut target = None;
    let mut source = None;
    let mut index = 0;
    while index < rest.len() {
        let flag = rest[index].as_str();
        let value = || {
            rest.get(index + 1)
                .cloned()
                .ok_or_else(|| SECRET_USAGE.to_owned())
        };
        match flag {
            flag if is_device_flag(flag) => {
                let host = value()?;
                if !valid_device_host(&host) {
                    return Err("device host contains unsupported characters".to_owned());
                }
                target = Some(SecretTarget::Device(host));
                index += 2;
            }
            "--volume" => {
                target = Some(SecretTarget::Volume(PathBuf::from(value()?)));
                index += 2;
            }
            "--from" => {
                source = Some(PathBuf::from(value()?));
                index += 2;
            }
            _ => return Err(SECRET_USAGE.to_owned()),
        }
    }
    let target = target.ok_or_else(|| SECRET_USAGE.to_owned())?;
    let action = match verb.as_str() {
        "set" => {
            let name = name.expect("set parsed a name");
            let source = match source {
                Some(path) => path,
                None => find_secret_source(&name)?,
            };
            SecretAction::Set { name, source }
        }
        "remove" => SecretAction::Remove {
            name: name.expect("remove parsed a name"),
        },
        _ => SecretAction::List,
    };
    Ok((action, target))
}

fn secret_command(arguments: &[String]) -> Result<(), String> {
    let (action, target) = parse_secret(arguments)?;
    match (&action, &target) {
        (SecretAction::Set { name, source }, _) => {
            let value = read_secret_file(source)?;
            // The value is never printed and never passed as an argument, so
            // it does not reach a terminal, a shell history or the remote
            // process table. Only its length is reported.
            let bytes = value.len();
            match &target {
                SecretTarget::Device(host) => {
                    let script = format!(
                        "set -e\n\
                         mkdir -p {DEVICE_SECRETS_DIRECTORY}\n\
                         chmod 700 {DEVICE_SECRETS_DIRECTORY}\n\
                         cat > {DEVICE_SECRETS_DIRECTORY}/{name} <<'{SECRET_DELIMITER}'\n\
                         {value}\n\
                         {SECRET_DELIMITER}\n\
                         chmod 600 {DEVICE_SECRETS_DIRECTORY}/{name}\n"
                    );
                    let output =
                        run_remote_shell(&format!("root@{host}"), &script, DEVICE_PROBE_TIMEOUT)?;
                    if !output.status.success() {
                        return Err(remote_shell_error(
                            format!("install the '{name}' credential"),
                            &output.stdout,
                            &output.stderr,
                        ));
                    }
                    println!("Installed '{name}' ({bytes} bytes) on {host}.");
                }
                SecretTarget::Volume(volume) => {
                    let directory = volume.join(".adds").join("cobalt").join("secrets");
                    std::fs::create_dir_all(&directory)
                        .map_err(|error| format!("create {}: {error}", directory.display()))?;
                    let path = directory.join(name);
                    std::fs::write(&path, format!("{value}\n"))
                        .map_err(|error| format!("write {}: {error}", path.display()))?;
                    println!("Installed '{name}' ({bytes} bytes) at {}.", path.display());
                }
            }
            println!("An application reaches it by naming the secret '{name}'; the value is never sent to the application itself.");
            Ok(())
        }
        (SecretAction::Remove { name }, SecretTarget::Device(host)) => {
            let script = format!("rm -f {DEVICE_SECRETS_DIRECTORY}/{name}\n");
            let output = run_remote_shell(&format!("root@{host}"), &script, DEVICE_PROBE_TIMEOUT)?;
            if !output.status.success() {
                return Err(remote_shell_error(
                    format!("remove the '{name}' credential"),
                    &output.stdout,
                    &output.stderr,
                ));
            }
            println!("Removed '{name}' from {host}.");
            Ok(())
        }
        (SecretAction::Remove { name }, SecretTarget::Volume(volume)) => {
            let path = volume
                .join(".adds")
                .join("cobalt")
                .join("secrets")
                .join(name);
            match std::fs::remove_file(&path) {
                Ok(()) => println!("Removed {}.", path.display()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    println!("No '{name}' credential was installed.");
                }
                Err(error) => return Err(format!("remove {}: {error}", path.display())),
            }
            Ok(())
        }
        (SecretAction::List, SecretTarget::Device(host)) => {
            // Names only. A command that can print a key is a command someone
            // will run over a shoulder or paste into a bug report.
            let script = format!(
                "for secret in {DEVICE_SECRETS_DIRECTORY}/*; do\n\
                 \x20 [ -f \"$secret\" ] && [ ! -L \"$secret\" ] && basename \"$secret\"\n\
                 done\n"
            );
            let output = run_remote_shell(&format!("root@{host}"), &script, DEVICE_PROBE_TIMEOUT)?;
            if !output.status.success() {
                return Err(remote_shell_error(
                    "list credentials".to_owned(),
                    &output.stdout,
                    &output.stderr,
                ));
            }
            report_secret_names(String::from_utf8_lossy(&output.stdout).lines());
            Ok(())
        }
        (SecretAction::List, SecretTarget::Volume(volume)) => {
            let directory = volume.join(".adds").join("cobalt").join("secrets");
            let mut names = Vec::new();
            if let Ok(entries) = std::fs::read_dir(&directory) {
                for entry in entries.flatten() {
                    if entry
                        .file_type()
                        .is_ok_and(|kind| kind.is_file() && !kind.is_symlink())
                    {
                        names.push(entry.file_name().to_string_lossy().into_owned());
                    }
                }
            }
            names.sort();
            report_secret_names(names.iter().map(String::as_str));
            Ok(())
        }
    }
}

fn report_secret_names<'a>(names: impl Iterator<Item = &'a str>) {
    let names: Vec<&str> = names
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .collect();
    if names.is_empty() {
        println!("No credentials are installed.");
        return;
    }
    println!("Installed credentials:");
    for name in names {
        println!("  {name}");
    }
}

/// Where the runtime reads owner-installed TLS trust roots, from `kobod`.
const DEVICE_TRUST_DIRECTORY: &str = "/mnt/onboard/.adds/cobalt/trust";

const TRUST_USAGE: &str =
    "usage: kobo trust set <name> --from PATH (--device IP | --volume PATH)\n\
                           \x20      kobo trust list (--device IP | --volume PATH)\n\
                           \x20      kobo trust remove <name> (--device IP | --volume PATH)";

/// Installs, lists or removes owner TLS trust roots on a reader.
///
/// The same shape as `kobo secret`, because it is the same act: an owner,
/// attended, putting a file where only the runtime reads it. The value is a
/// PEM certificate rather than a credential, so unlike a secret it is checked
/// for being one before it travels, and listing it is harmless.
fn trust_command(arguments: &[String]) -> Result<(), String> {
    let (action, target) = parse_trust(arguments)?;
    match (action, target) {
        (SecretAction::Set { name, source }, target) => trust_set(&name, &source, &target),
        (SecretAction::Remove { name }, target) => trust_remove(&name, &target),
        (SecretAction::List, target) => trust_list(&target),
    }
}

/// Reads, checks and installs one PEM certificate as an owner trust root.
fn trust_set(name: &str, source: &Path, target: &SecretTarget) -> Result<(), String> {
    let text = std::fs::read_to_string(source)
        .map_err(|error| format!("read {}: {error}", source.display()))?;
    let found = kobo_net::pem::certificates(&text).len();
    if found == 0 {
        return Err(format!(
            "{} holds no CERTIFICATE block; expected a PEM certificate",
            source.display()
        ));
    }
    if text.lines().any(|line| line.trim() == SECRET_DELIMITER) {
        return Err("that file cannot travel over the install script".to_owned());
    }
    match target {
        SecretTarget::Device(host) => {
            let script = format!(
                "set -e\n\
                 mkdir -p {DEVICE_TRUST_DIRECTORY}\n\
                 cat > {DEVICE_TRUST_DIRECTORY}/{name}.pem <<'{SECRET_DELIMITER}'\n\
                 {text}\n\
                 {SECRET_DELIMITER}\n"
            );
            let output = run_remote_shell(&format!("root@{host}"), &script, DEVICE_PROBE_TIMEOUT)?;
            if !output.status.success() {
                return Err(remote_shell_error(
                    format!("install the '{name}' trust root"),
                    &output.stdout,
                    &output.stderr,
                ));
            }
            println!("Installed trust root '{name}' ({found} certificate(s)) on {host}.");
        }
        SecretTarget::Volume(volume) => {
            let directory = volume.join(".adds").join("cobalt").join("trust");
            std::fs::create_dir_all(&directory)
                .map_err(|error| format!("create {}: {error}", directory.display()))?;
            let path = directory.join(format!("{name}.pem"));
            std::fs::write(&path, &text)
                .map_err(|error| format!("write {}: {error}", path.display()))?;
            println!("Installed trust root '{name}' at {}.", path.display());
        }
    }
    println!(
        "The runtime now verifies TLS hosts against it, beside the public roots. \
         It takes effect at the next session."
    );
    Ok(())
}

fn trust_remove(name: &str, target: &SecretTarget) -> Result<(), String> {
    match target {
        SecretTarget::Device(host) => {
            let script = format!("rm -f {DEVICE_TRUST_DIRECTORY}/{name}.pem\n");
            let output = run_remote_shell(&format!("root@{host}"), &script, DEVICE_PROBE_TIMEOUT)?;
            if !output.status.success() {
                return Err(remote_shell_error(
                    format!("remove the '{name}' trust root"),
                    &output.stdout,
                    &output.stderr,
                ));
            }
            println!("Removed trust root '{name}' from {host}.");
        }
        SecretTarget::Volume(volume) => {
            let path = volume
                .join(".adds")
                .join("cobalt")
                .join("trust")
                .join(format!("{name}.pem"));
            match std::fs::remove_file(&path) {
                Ok(()) => println!("Removed {}.", path.display()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    println!("No '{name}' trust root was installed.");
                }
                Err(error) => return Err(format!("remove {}: {error}", path.display())),
            }
        }
    }
    Ok(())
}

fn trust_list(target: &SecretTarget) -> Result<(), String> {
    match target {
        SecretTarget::Device(host) => {
            let script = format!("ls -1 {DEVICE_TRUST_DIRECTORY} 2>/dev/null || true\n");
            let output = run_remote_shell(&format!("root@{host}"), &script, DEVICE_PROBE_TIMEOUT)?;
            if !output.status.success() {
                return Err(remote_shell_error(
                    "list trust roots".to_owned(),
                    &output.stdout,
                    &output.stderr,
                ));
            }
            report_trust_names(String::from_utf8_lossy(&output.stdout).lines());
        }
        SecretTarget::Volume(volume) => {
            let directory = volume.join(".adds").join("cobalt").join("trust");
            let mut names = Vec::new();
            if let Ok(entries) = std::fs::read_dir(&directory) {
                for entry in entries.flatten() {
                    names.push(entry.file_name().to_string_lossy().into_owned());
                }
            }
            names.sort();
            report_trust_names(names.iter().map(String::as_str));
        }
    }
    Ok(())
}

/// The trust grammar, reusing the secret shapes: the verbs, targets and name
/// rule are identical, and a second copy of the parser would only drift.
fn parse_trust(arguments: &[String]) -> Result<(SecretAction, SecretTarget), String> {
    let (verb, rest) = arguments
        .split_first()
        .ok_or_else(|| TRUST_USAGE.to_owned())?;
    let (name, rest) = match verb.as_str() {
        "set" | "remove" => {
            let (name, rest) = rest.split_first().ok_or_else(|| TRUST_USAGE.to_owned())?;
            if !valid_secret_name(name) {
                return Err(
                    "a trust root name is letters, digits, '-' and '_', up to 64 characters"
                        .to_owned(),
                );
            }
            (Some(name.clone()), rest)
        }
        "list" => (None, rest),
        _ => return Err(TRUST_USAGE.to_owned()),
    };
    let mut target = None;
    let mut source = None;
    let mut index = 0;
    while index < rest.len() {
        let flag = rest[index].as_str();
        let value = || {
            rest.get(index + 1)
                .cloned()
                .ok_or_else(|| TRUST_USAGE.to_owned())
        };
        match flag {
            flag if is_device_flag(flag) => {
                let host = value()?;
                if !valid_device_host(&host) {
                    return Err("device host contains unsupported characters".to_owned());
                }
                target = Some(SecretTarget::Device(host));
                index += 2;
            }
            "--volume" => {
                target = Some(SecretTarget::Volume(PathBuf::from(value()?)));
                index += 2;
            }
            "--from" => {
                source = Some(PathBuf::from(value()?));
                index += 2;
            }
            _ => return Err(TRUST_USAGE.to_owned()),
        }
    }
    let target = target.ok_or_else(|| TRUST_USAGE.to_owned())?;
    let action = match verb.as_str() {
        "set" => {
            let name = name.expect("set parsed a name");
            let source = match source {
                Some(path) => path,
                None => trust_source(&name)?,
            };
            SecretAction::Set { name, source }
        }
        "remove" => SecretAction::Remove {
            name: name.expect("remove parsed a name"),
        },
        _ => SecretAction::List,
    };
    Ok((action, target))
}

/// Where a trust root is looked for when `--from` is not given: the host
/// trust directory every host runtime already reads, which is where
/// `kobo-sidekick init` writes its certificate.
fn trust_source(name: &str) -> Result<PathBuf, String> {
    let Some(home) = std::env::var_os("HOME") else {
        return Err(format!(
            "no HOME to look in; pass --from PATH\n{TRUST_USAGE}"
        ));
    };
    let candidate = PathBuf::from(home)
        .join(".config")
        .join("kobo")
        .join("trust")
        .join(format!("{name}.pem"));
    if candidate.is_file() {
        return Ok(candidate);
    }
    Err(format!(
        "no certificate at {}; pass --from PATH",
        candidate.display()
    ))
}

fn report_trust_names<'a>(names: impl Iterator<Item = &'a str>) {
    let names: Vec<&str> = names
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .collect();
    if names.is_empty() {
        println!("No trust roots are installed.");
        return;
    }
    println!("Installed trust roots:");
    for name in names {
        println!("  {name}");
    }
}

fn print_help() {
    // Two commands write to the panel and are compiled out without the
    // feature, so they are named here only when they are really present.
    // Advertising a command this binary would reject is worse than saying
    // nothing, and it is the sort of drift a help string invites.
    #[cfg(feature = "device-write")]
    const WRITING: &str = "\n\nBuilt with --features device-write, so also:\n  \
         tap --device IP X,Y [MS:X,Y ...]  Tap the real panel through the real touch node.\n  \
         \x20                              Several steps run in one upload, timed on the\n  \
         \x20                              device, which is how an application is driven.\n  \
         smoke-display --device IP --confirm ...  Attended display checks, one at a time";
    #[cfg(not(feature = "device-write"))]
    const WRITING: &str = "\n\nBuilt without --features device-write, so the commands that write \
         to a panel\n(tap, smoke-display) are not in this binary.";
    println!(
        "Kobo application SDK\n\n\
         Usage: kobo <command>\n\n\
         Commands:\n\
           new <name>             Create a Rust application\n\
           dev [--builtin] [address]  Run this SDK app in the browser simulator\n\
           drive --script PATH    Drive a running simulator and save PNG screenshots\n\
           drive --script PATH --record DIR  ... and film it, no hardware needed\n\
           shot [--device HOST]   Save a PNG of the panel (device or simulator)\n\
           record --device IP [--seconds N] [--fps F] [--out DIR]  Film the panel, read-only\n\
           present <app> --device IP [--seconds N]  Run one app on the panel\n\
           stop --device IP       Hand the panel back to the reader now\n\
           build [--device]       Build host workspace or ARM safe doctor, disabled kobod, and sample app\n\
           doctor [--device IP]   Run read-only device diagnostics\n\
           devices [--subnet A.B.C]  Find every reader on the local network\n\
           app-link status|unpair --device IP  Inspect or revoke browser pairing\n\
           session --device IP    Keep a device awake and on Wi-Fi while developing\n\
           session --device IP --hold [minutes]  Keep it reachable for unattended testing\n\
           wait --device IP       Block until a device answers again\n\
           logs --device IP [--follow] [--lines N]  Read the runtime trace from the device\n\
           shell --device IP [command ...]  Run one command on the reader, or open a\n\
           \x20                             session when no command is given. Exits with\n\
           \x20                             whatever the reader exited with\n\
           touch-probe --device IP [--seconds N]  Watch touch read-only to check the transform\n\
           guard-test --device IP --confirm ...   Prove the guardian restores the screen\n\
           package [--out PATH] [--folder PATH]  Build the KoboRoot.tgz an owner copies\n\
           app-key --seed PATH     Print the Ed25519 public key for a release seed\n\
           app-bundle --manifest PATH --binary PATH --seed PATH --out PATH\n\
                                   Build one signed, pathless .cobalt-app package\n\
           app-catalog --seed PATH --out PATH --signature PATH --entry PACKAGE HTTPS_URL ...\n\
                                   Build and sign the public app catalog\n\
           app-list --registry PATH\n\
                                   List validated Store app packages as JSON\n\
           stream init [--host ADDRESS ...]  Create or rotate Paperterm TLS identity\n\
           stream [--grid COLSxROWS] [--controls | --interactive | --read-only] [--port PORT] -- COMMAND [ARG ...]\n\
                                   Mirror a host terminal to paired Paperterm readers\n\
           app-check --registry PATH [--package PACKAGE] [--out PATH]\n\
                                   Build and verify every registered Store app\n\
           app-release --registry PATH --seed PATH --out PATH --base-url HTTPS_URL [--prebuilt-dir PATH | --artifact-dir PATH]\n\
                                   Build and sign every registered Store app\n\
           host-release-sign --manifest PATH --seed PATH --signature PATH --ssh-signature PATH\n\
                                   Sign host release metadata for publishing\n\
           host-release-verify --manifest PATH --signature PATH\n\
                                   Verify signed host release metadata\n\
           update [--channel stable|beta]\n\
                                   Update only the installed host kobo command; Stable is default\n\
           setup [--volume PATH] [--undo] [--enable-ssh] [--no-key] [--dry-run]\n\
                 [--yes] [--non-interactive] [--release-dir PATH | --source]\n\
                                   Prepare a reader over USB after default-no confirmation;\n\
                                   installed builds use their verified prebuilt device package\n\
           deploy --device IP [--package PATH]   Install over Wi-Fi, no reboot\n\
           secret set <name> [--from PATH] --device IP   Install a credential an app can name\n\
           secret list --device IP   Name the installed credentials, never their values\n\
           secret remove <name> --device IP   Take one credential off the reader\n\
           trust set <name> [--from PATH] --device IP   Install an owner TLS root the runtime verifies against\n\
           trust list --device IP   Name the installed trust roots\n\
           trust remove <name> --device IP   Take one trust root off the reader\n\
           inspect <package>       List a package and prove it writes nothing to the rootfs\n\
           verify <arm-binary>     Verify static ARM hard-float format\n\
           run --sim [--app NAME]  Run SDK, IPC, daemon and one app on host\n\
           run                    Device execution remains safety-gated\n\
           version                Print version\n\n\
         Every command that takes --device also takes -s, and these names\n\
         work if they are the ones you already know:\n\
           logcat -> logs   install -> deploy   wait-for-device -> wait\n\
           sim, simulator -> dev   init, create -> new{WRITING}"
    );
}

#[cfg(test)]
mod tests {
    /// Builds a recording the way the device writes one.
    fn recording(width: u32, height: u32, frames: &[(u32, u8)]) -> Vec<u8> {
        let mut raw = b"KOBOCST1".to_vec();
        raw.extend_from_slice(&width.to_le_bytes());
        raw.extend_from_slice(&height.to_le_bytes());
        for (millis, fill) in frames {
            raw.extend_from_slice(&millis.to_le_bytes());
            raw.extend(std::iter::repeat_n(*fill, (width * height) as usize));
        }

        raw
    }

    #[test]
    fn host_update_defaults_stable_and_requires_explicit_beta() {
        let arguments = |values: &[&str]| {
            values
                .iter()
                .map(|value| (*value).to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            super::parse_host_update(&arguments(&[])),
            Ok(super::HostUpdateChannel::Stable)
        );
        assert_eq!(
            super::parse_host_update(&arguments(&["--channel", "beta"])),
            Ok(super::HostUpdateChannel::Beta)
        );
        assert!(super::parse_host_update(&arguments(&["--beta"])).is_err());
        assert!(super::parse_host_update(&arguments(&["--channel", "nightly"])).is_err());
    }

    #[test]
    fn release_seed_accepts_raw_or_lowercase_hex() {
        let root = std::env::temp_dir().join(format!("kobo-seed-test-{}", std::process::id()));
        fs::create_dir_all(&root).expect("create fixture");
        let raw = root.join("raw");
        let hex = root.join("hex");
        fs::write(&raw, [7_u8; 32]).expect("write raw seed");
        fs::write(&hex, format!("{}\n", "07".repeat(32))).expect("write hex seed");
        assert_eq!(
            super::read_signing_seed(&raw).expect("raw seed"),
            [7_u8; 32]
        );
        assert_eq!(
            super::read_signing_seed(&hex).expect("hex seed"),
            [7_u8; 32]
        );
        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn release_registry_accepts_website_setup_metadata() {
        let value = kobo_json::parse(
            r#"{
                "package":"kobo-library",
                "id":"library",
                "display_name":"Library",
                "short_label":"Library",
                "summary":"Read a personal library.",
                "version":"1.0.0",
                "minimum_cobalt_version":"0.2.4",
                "glyph":"book",
                "capabilities":["network"],
                "release_notes":"Adds a faster way to read a personal library.",
                "setup":{"steps":[{"text":"Create a read-only key."}]}
            }"#,
        )
        .expect("registry fixture");

        let app = super::parse_release_app(&value).expect("setup metadata is registry-only");
        assert_eq!(app.id, "library");
    }

    #[test]
    fn app_bundle_and_catalog_commands_produce_verified_assets() {
        let root = std::env::temp_dir().join(format!("kobo-app-assets-{}", std::process::id()));
        fs::create_dir_all(&root).expect("create fixture");
        let seed_path = root.join("seed");
        let manifest_path = root.join("manifest.json");
        let binary_path = root.join("kobo-word-count");
        let bundle_path = root.join("word-count.cobalt-app");
        let catalog_path = root.join("cobalt-app-catalog.json");
        let signature_path = root.join("cobalt-app-catalog.json.sig");
        let seed = [9_u8; 32];
        let mut binary = vec![0_u8; 84];
        binary[..6].copy_from_slice(b"\x7fELF\x01\x01");
        binary[16..18].copy_from_slice(&2_u16.to_le_bytes());
        binary[18..20].copy_from_slice(&40_u16.to_le_bytes());
        binary[24..28].copy_from_slice(&0x10_0040_u32.to_le_bytes());
        binary[28..32].copy_from_slice(&52_u32.to_le_bytes());
        binary[36..40].copy_from_slice(&0x400_u32.to_le_bytes());
        binary[42..44].copy_from_slice(&32_u16.to_le_bytes());
        binary[44..46].copy_from_slice(&1_u16.to_le_bytes());
        binary[52..56].copy_from_slice(&1_u32.to_le_bytes());
        binary[56..60].copy_from_slice(&0_u32.to_le_bytes());
        binary[60..64].copy_from_slice(&0x10_0000_u32.to_le_bytes());
        let binary_size = u32::try_from(binary.len()).expect("fixture fits in u32");
        binary[68..72].copy_from_slice(&binary_size.to_le_bytes());
        binary[72..76].copy_from_slice(&binary_size.to_le_bytes());
        binary[76..80].copy_from_slice(&5_u32.to_le_bytes());
        fs::write(&seed_path, seed).expect("write seed");
        fs::write(&binary_path, &binary).expect("write binary");
        let manifest = kobo_app_store::Manifest::new_public(kobo_app_store::ManifestInput {
            id: "word-count".to_owned(),
            display_name: "Word Count".to_owned(),
            short_label: "Words".to_owned(),
            summary: "Counts words in a note.".to_owned(),
            version: "1.0.0".to_owned(),
            minimum_cobalt_version: env!("CARGO_PKG_VERSION").to_owned(),
            glyph: "note".to_owned(),
            capabilities: Vec::new(),
            binary_sha256: kobo_net::sha256::hex_digest(&binary),
            binary_bytes: binary.len() as u64,
        })
        .expect("manifest");
        fs::write(&manifest_path, manifest.to_canonical_bytes()).expect("write manifest");

        let header_only = root.join("header-only");
        let mut header = vec![0_u8; 52];
        header[..6].copy_from_slice(b"\x7fELF\x01\x01");
        header[16..18].copy_from_slice(&2_u16.to_le_bytes());
        header[18..20].copy_from_slice(&40_u16.to_le_bytes());
        header[24..28].copy_from_slice(&0x10_0040_u32.to_le_bytes());
        header[36..40].copy_from_slice(&0x400_u32.to_le_bytes());
        header[42..44].copy_from_slice(&32_u16.to_le_bytes());
        fs::write(&header_only, header).expect("write header-only binary");
        assert!(super::verify_arm_elf(&header_only)
            .expect_err("an ELF header without a load segment was accepted")
            .contains("executable load segment"));

        super::app_bundle(&[
            "--manifest".to_owned(),
            manifest_path.display().to_string(),
            "--binary".to_owned(),
            binary_path.display().to_string(),
            "--seed".to_owned(),
            seed_path.display().to_string(),
            "--out".to_owned(),
            bundle_path.display().to_string(),
        ])
        .expect("build bundle");
        super::app_catalog(&[
            "--seed".to_owned(),
            seed_path.display().to_string(),
            "--out".to_owned(),
            catalog_path.display().to_string(),
            "--signature".to_owned(),
            signature_path.display().to_string(),
            "--entry".to_owned(),
            bundle_path.display().to_string(),
            "https://example.test/word-count.cobalt-app".to_owned(),
        ])
        .expect("build catalog");

        let public = kobo_app_store::derive_public_key(&seed).expect("public key");
        let bundle = fs::read(&bundle_path).expect("read bundle");
        assert_eq!(
            kobo_app_store::parse_public_bundle(&bundle, &public)
                .expect("verify bundle")
                .manifest()
                .id(),
            "word-count"
        );
        let catalog_bytes = fs::read(&catalog_path).expect("read catalog");
        let signature = fs::read_to_string(&signature_path).expect("read signature");
        let signature =
            kobo_app_store::DetachedSignature::from_hex(signature.trim()).expect("signature");
        kobo_app_store::verify(&catalog_bytes, &signature, &public).expect("verify catalog");
        let catalog = kobo_app_store::Catalog::parse_public(&catalog_bytes).expect("parse catalog");
        assert_eq!(catalog.entries().len(), 1);
        fs::remove_dir_all(root).expect("remove fixture");
    }

    /// A session is what somebody asked for by not asking for anything else.
    #[test]
    fn a_shell_with_no_command_is_a_request_for_a_session() {
        let arguments = ["--device".to_owned(), "192.168.1.2".to_owned()];
        let request =
            super::parse_shell(&arguments).expect("a host and nothing else is a valid request");
        assert_eq!(request.host, "192.168.1.2");
        assert!(request.command.is_none());
    }

    /// The words after the host are one line of shell, not a list of
    /// arguments. Somebody typing a pipeline or a redirection means it.
    #[test]
    fn the_words_after_the_host_become_one_line_of_shell() {
        let arguments = [
            "-s".to_owned(),
            "192.168.1.2".to_owned(),
            "dmesg".to_owned(),
            "|".to_owned(),
            "tail".to_owned(),
            "-n".to_owned(),
            "5".to_owned(),
        ];
        let request = super::parse_shell(&arguments).expect("a command is a valid request");
        assert_eq!(request.command.as_deref(), Some("dmesg | tail -n 5"));
    }

    /// The host goes into an ssh argument, so anything that could be read as
    /// something else has to be refused before it gets there.
    #[test]
    fn a_shell_refuses_a_host_that_is_not_one() {
        for host in ["a;rm -rf /", "1.2.3.4 -oProxyCommand=x", "$(whoami)"] {
            let arguments = ["--device".to_owned(), host.to_owned(), "uname".to_owned()];
            assert!(
                super::parse_shell(&arguments).is_err(),
                "{host} was accepted as a device"
            );
        }
    }

    /// Missing the device entirely is the usage message, not a panic on an
    /// empty slice.
    #[test]
    fn a_shell_without_a_device_says_how_to_spell_it() {
        for arguments in [
            Vec::new(),
            vec!["uname".to_owned()],
            vec!["192.168.1.2".to_owned()],
        ] {
            let error = super::parse_shell(&arguments).expect_err("no device was named");
            assert!(error.starts_with("usage: kobo shell"), "{error}");
        }
    }

    /// `sh` is what a shell is called by the people most likely to want one.
    #[test]
    fn sh_is_another_name_for_shell() {
        assert_eq!(super::canonical("sh"), "shell");
    }

    #[test]
    fn a_recording_decodes_to_the_frames_that_were_kept() {
        let raw = recording(2, 3, &[(0, 0xff), (500, 0x40)]);
        let (width, height, frames) = super::decode_recording(&raw).expect("decode");
        assert_eq!((width, height), (2, 3));
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].millis, 0);
        assert_eq!(frames[1].millis, 500);
        assert_eq!(frames[1].grey, vec![0x40; 6]);
    }

    #[test]
    fn every_grey_level_survives_the_round_trip() {
        // The panel is greyscale and the text on it is anti-aliased. A
        // recording that flattened the greys would look harsher than the
        // device and would be read as a rendering bug that is not there.
        let mut raw = b"KOBOCST1".to_vec();
        raw.extend_from_slice(&16_u32.to_le_bytes());
        raw.extend_from_slice(&16_u32.to_le_bytes());
        raw.extend_from_slice(&0_u32.to_le_bytes());
        let ramp: Vec<u8> = (0..=255_u8).step_by(1).take(256).collect();
        raw.extend_from_slice(&ramp);
        let (_, _, frames) = super::decode_recording(&raw).expect("decode");
        assert_eq!(frames[0].grey, ramp);
    }

    #[test]
    fn half_a_frame_is_dropped_rather_than_shown_as_a_whole_one() {
        // The device can be unplugged or fill up mid-write. Half a frame
        // decoded as a whole one is a picture of nothing that looks exactly
        // like a rendering failure.
        let mut raw = recording(2, 3, &[(0, 0xff)]);
        raw.extend_from_slice(&99_u32.to_le_bytes());
        raw.extend_from_slice(&[0x10, 0x10]);
        let (_, _, frames) = super::decode_recording(&raw).expect("decode");
        assert_eq!(frames.len(), 1, "a torn frame was kept");
    }

    #[test]
    fn something_that_is_not_a_recording_is_refused() {
        assert!(super::decode_recording(b"not a recording at all").is_err());
        assert!(super::decode_recording(&recording(2, 3, &[])).is_err());
    }

    /// The frames are not evenly spaced, because only the ones that moved were
    /// kept. Handing them to ffmpeg at a fixed rate would play a screen that
    /// was up for ten seconds in the same time as one that was up for a tenth
    /// of one, which is a recording of a run that never happened.
    #[test]
    fn each_frame_is_played_for_as_long_as_it_was_really_on_the_panel() {
        let raw = recording(2, 3, &[(0, 0xff), (2_000, 0x40), (2_150, 0x10)]);
        let (_, _, frames) = super::decode_recording(&raw).expect("decode");
        assert_eq!(
            super::concat_list(&frames),
            "file 'frame-0000.png'\nduration 2.000\n\
             file 'frame-0001.png'\nduration 0.150\n\
             file 'frame-0002.png'\nduration 1.000\n\
             file 'frame-0002.png'\n",
            "the last frame is named twice because concat ignores the last duration"
        );
    }

    /// A frame that was replaced within a tenth of a second is still a frame
    /// somebody has to be able to see. Played for its real duration it is one
    /// or two hundredths of a second, which at any sane frame rate is a frame
    /// the encoder drops entirely.
    #[test]
    fn a_screen_that_barely_appeared_is_still_held_long_enough_to_see() {
        let raw = recording(2, 3, &[(0, 0xff), (20, 0x40)]);
        let (_, _, frames) = super::decode_recording(&raw).expect("decode");
        assert!(
            super::concat_list(&frames).contains("duration 0.100"),
            "a frame was given less time than the encoder will keep"
        );
    }

    use super::package;
    use super::{
        build_executables, canonical, configured_target_directory, is_device_flag,
        manifest_uses_sdk, normalise_secret_value, parse_deploy, parse_devices, parse_logs,
        parse_touch_probe, unreachable_device, valid_device_host, valid_slug, verify_arm_elf,
        wait_for_remote_child, DevSessionGuard, RemoteArtifact, SimulationGuard, ALIASES,
        DEFAULT_TRACE_LINES, DEPLOY_TIMEOUT, DEVICE_PACKAGES, TOUCH_PROBE_DEFAULT_SECONDS,
        TOUCH_PROBE_MAXIMUM_SECONDS,
    };
    #[cfg(feature = "device-write")]
    use super::{
        parse_guard_test, parse_smoke_display, run, RemoteArtifactSession, RemoteProgram,
        SmokeStage, GUARD_TEST_CHILD, GUARD_TEST_CONFIRMATION, REMOTE_CLEANUP_TIMEOUT,
        REMOTE_COMMAND_TIMEOUT, REMOTE_CONNECT_TIMEOUT_SECONDS, REMOTE_SMOKE_TIMEOUT_SECONDS,
    };
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;
    use std::time::Duration;

    #[test]
    fn secret_files_accept_raw_and_assignment_forms() {
        assert_eq!(normalise_secret_value("sk-secret"), "sk-secret");
        assert_eq!(
            normalise_secret_value("EXA_API_KEY='exa-secret'"),
            "exa-secret"
        );
        assert_eq!(normalise_secret_value("token=still=raw"), "token=still=raw");
    }

    #[test]
    fn a_log_request_reads_the_way_adb_logcat_does() {
        let arguments = |parts: &[&str]| {
            parts
                .iter()
                .map(|part| (*part).to_owned())
                .collect::<Vec<_>>()
        };
        let parse = |parts: &[&str]| {
            parse_logs(&arguments(parts))
                .map(|request| (request.follow, request.lines, request.clear))
        };
        // Watching is the point, so it is what asking for nothing gets.
        assert_eq!(
            parse(&["--device", "192.168.1.15"]),
            Ok((true, DEFAULT_TRACE_LINES, false))
        );
        // -s is adb's spelling of the same flag and has to reach the same code.
        assert_eq!(
            parse(&["-s", "192.168.1.15", "-d"]),
            Ok((false, DEFAULT_TRACE_LINES, false))
        );
        assert_eq!(
            parse(&["--device", "host", "-t", "40"]),
            Ok((true, 40, false))
        );
        assert_eq!(
            parse(&["--device", "host", "--lines", "40", "--dump"]),
            Ok((false, 40, false))
        );
        // Clearing alone clears and stops; asked for with --follow it clears
        // and then watches, whichever order the two were written in.
        assert_eq!(
            parse(&["--device", "host", "-c"]),
            Ok((false, DEFAULT_TRACE_LINES, true))
        );
        assert_eq!(
            parse(&["--device", "host", "-c", "-f"]),
            Ok((true, DEFAULT_TRACE_LINES, true))
        );
        assert_eq!(
            parse(&["--device", "host", "-f", "-c"]),
            Ok((true, DEFAULT_TRACE_LINES, true))
        );
        for rejected in [
            // A host that could carry a second command into the remote shell.
            vec!["--device", "192.168.1.15; reboot"],
            vec!["--device"],
            vec!["--device", "host", "--lines"],
            vec!["--device", "host", "--lines", "0"],
            vec!["--device", "host", "--lines", "10001"],
            vec!["--device", "host", "--nonsense"],
            vec!["host"],
        ] {
            assert!(
                parse(&rejected).is_err(),
                "{rejected:?} should not be accepted"
            );
        }
    }

    #[test]
    fn names_from_other_mobile_toolchains_reach_the_command_they_mean() {
        // Somebody who has shipped for Android should not have to learn a new
        // word for the same idea before they can read a log.
        assert_eq!(canonical("logcat"), "logs");
        assert_eq!(canonical("install"), "deploy");
        assert_eq!(canonical("wait-for-device"), "wait");
        assert_eq!(canonical("sim"), "dev");
        assert_eq!(canonical("init"), "new");
        // A canonical name is left exactly as it is, and so is a name nobody
        // knows, so an unknown command still reports itself rather than an
        // alias it was silently turned into.
        assert_eq!(canonical("logs"), "logs");
        assert_eq!(canonical("nonsense"), "nonsense");
        for (alias, name) in ALIASES {
            assert_ne!(alias, name, "an alias for itself is dead weight");
            assert_eq!(canonical(name), *name, "aliases must not chain");
        }
        assert!(is_device_flag("--device"));
        assert!(is_device_flag("-s"));
        assert!(!is_device_flag("--devices"));
    }

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
    fn the_start_script_installs_the_staged_public_key_once() {
        let script = super::START_SCRIPT;
        // The home directory is read from /etc/passwd rather than assumed to
        // be /root: the i.MX6 firmware gives root a home of `/`, and a key
        // installed under /root never authenticates there.
        assert!(script.contains("awk -F: '$1 == \"root\" { print $6 }' /etc/passwd"));
        assert!(script.contains("keys=\"$home/.ssh/authorized_keys\""));
        assert!(script.contains("while IFS= read -r known"));
        assert!(script.contains("if [ \"$known\" = \"$key\" ]"));
        assert!(script.contains("printf '%s\\n' \"$key\" >> \"$keys\""));
        assert!(script.contains("rm -f \"$staged_key\""));
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

    /// That the template compiles is settled by `examples/hello` being a
    /// workspace member, which is the whole reason it is one. What is left to
    /// check here is that it still teaches the right things and that the
    /// front matter naming it a template does not follow it out the door.
    #[test]
    fn the_generated_app_teaches_the_contract_it_should() {
        let source = super::generated_app_source();

        // The loop belongs to the SDK. An application that hand-rolls one
        // breaks the next time the event enum grows, which is how the
        // previous template died.
        assert!(source.contains("kobo_sdk::run("));
        assert!(!source.contains("next_event"));

        // Hardware is asked for, and every answer including a refusal is
        // shown. Both halves are the point of the example.
        assert!(source.contains("context.device().read_battery()"));
        assert!(source.contains("fn on_device_result"));
        assert!(source.contains("DeviceResult::Denied(reason)"));

        // It must never reach hardware itself.
        assert!(!source.contains("/dev/"));
        assert!(!source.contains("/sys/"));

        // It arrives as somebody's own application, not as a copy of a file
        // that describes itself as a template.
        assert!(
            source.starts_with("use kobo_sdk::prelude::*;"),
            "{}",
            &source[..80]
        );
        assert!(!source.contains("//!"));
        assert!(source.ends_with('\n'));
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
    }

    #[test]
    fn cargo_target_dir_resolves_exactly_like_the_build_invocation() {
        let workspace = PathBuf::from("/source/cobalt");
        let invocation = PathBuf::from("/runner/jobs/package");
        let relative = std::ffi::OsStr::new("../../cobalt-targets");
        let resolved = configured_target_directory(&workspace, &invocation, Some(relative));
        assert_eq!(resolved, invocation.join(relative));
        assert_ne!(resolved, workspace.join(relative));
        assert_eq!(
            resolved.join("armv7-unknown-linux-musleabihf/release/kobod"),
            invocation
                .join(relative)
                .join("armv7-unknown-linux-musleabihf/release/kobod")
        );
        assert_eq!(
            configured_target_directory(&workspace, &invocation, None),
            workspace.join("target")
        );
        assert_eq!(
            configured_target_directory(
                &workspace,
                &invocation,
                Some(std::ffi::OsStr::new("/external/cobalt-targets"))
            ),
            PathBuf::from("/external/cobalt-targets")
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
            &ssh_args[..5],
            ["-T", "-o", "BatchMode=yes", "-o", "ConnectTimeout=10"]
        );
        assert!(matches!(ssh_args.len(), 6 | 10), "{ssh_args:?}");
        if ssh_args.len() == 10 {
            assert_eq!(&ssh_args[5..8], ["-o", "IdentitiesOnly=yes", "-i"]);
            assert_eq!(
                PathBuf::from(&ssh_args[8])
                    .file_name()
                    .and_then(|name| name.to_str()),
                Some(super::DEVICE_KEY_NAME)
            );
        }
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
            &RemoteProgram::Smoke(SmokeStage::DisplayOnly),
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

    #[cfg(feature = "device-write")]
    #[test]
    fn a_whole_tap_sequence_travels_in_one_upload_and_outlasts_its_own_waits() {
        // The point of the sequence. Every step must reach the device in one
        // script, and both timeouts must outlast the sleeping the device is
        // being asked to do, or the run is killed partway through a tour.
        let session = RemoteArtifactSession {
            directory: "/tmp/kobo-tap-123-456".to_owned(),
            binary: "/tmp/kobo-tap-123-456/kobo-tap".to_owned(),
            owner_file: "/tmp/kobo-tap-123-456/.kobo-tap-owner".to_owned(),
            owner_token: "0123456789abcdef0123456789abcdef".to_owned(),
        };
        let sequence = "1500:536,400 2000:80,80 2500:400,1380";
        let artifact = super::RemoteArtifact::tap(sequence.to_owned(), 6_000);
        let script = super::remote_fixed_artifact_script(
            &session,
            &artifact.program,
            &"a".repeat(64),
            &super::base64_encode(b"tap"),
        );
        assert!(script.contains(&format!("KOBO_TAP_POINT='{sequence}'")));
        assert!(script.contains("KOBO_TAP_UNLOCK='OWNER_ATTENDED_SYNTHETIC_TOUCH'"));
        // 6s of waiting, so the device-side bound is 36 and the host's is 66.
        assert!(script.contains("/usr/bin/timeout 36 \"$bin\""));
        assert!(artifact.timeout() > Duration::from_secs(6));
        assert!(artifact.timeout() > Duration::from_secs(36));
    }

    #[cfg(feature = "device-write")]
    #[test]
    fn a_step_is_checked_here_before_anything_is_built_or_uploaded() {
        assert_eq!(super::parse_tap_step("536,400"), Ok(0));
        assert_eq!(super::parse_tap_step("1500:536,400"), Ok(1500));
        assert!(super::parse_tap_step("536").is_err());
        assert!(super::parse_tap_step("soon:536,400").is_err());
        assert!(super::parse_tap_step("536,-4").is_err());
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

    mod simulating {
        use super::super::simulated_package;

        fn arguments(values: &[&str]) -> Vec<String> {
            values.iter().map(|value| (*value).to_owned()).collect()
        }

        #[test]
        fn without_an_app_it_runs_the_one_it_always_ran() {
            assert_eq!(simulated_package(&arguments(&[])), Ok("kobo-todo"));
        }

        #[test]
        fn an_app_can_be_named_the_way_the_launcher_names_it() {
            assert_eq!(
                simulated_package(&arguments(&["--app", "rss"])),
                Ok("kobo-rss")
            );
            assert_eq!(
                simulated_package(&arguments(&["-a", "gutenbird"])),
                Ok("kobo-gutenbird")
            );
            assert_eq!(
                simulated_package(&arguments(&["--app", "sudoku"])),
                Ok("kobo-sudoku")
            );
        }

        #[test]
        fn an_app_can_also_be_named_the_way_cargo_names_it() {
            assert_eq!(
                simulated_package(&arguments(&["--app", "kobo-hn"])),
                Ok("kobo-hn")
            );
        }

        #[test]
        fn the_runtime_is_not_an_application_to_run_against_itself() {
            // kobod is on the packages list and is the thing already being
            // started; asking for it would start two of them.
            assert!(simulated_package(&arguments(&["--app", "kobod"])).is_err());
        }

        #[test]
        fn a_name_that_is_not_an_app_is_refused_with_the_ones_that_are() {
            let error =
                simulated_package(&arguments(&["--app", "../../etc/passwd"])).expect_err("refused");
            assert!(error.contains("rss"), "{error}");
            assert!(error.contains("todo"), "{error}");
            let missing = simulated_package(&arguments(&["--app"])).expect_err("refused");
            assert!(missing.contains("needs a name"), "{missing}");
        }

        mod app_registry {
            use super::super::super::{read_release_registry, workspace_manifest, STORE_PACKAGES};
            use std::collections::BTreeSet;

            #[test]
            fn checked_in_registry_contains_every_store_application() {
                let registry = workspace_manifest()
                    .parent()
                    .expect("workspace root")
                    .join("apps/catalog.json");
                let apps = read_release_registry(&registry).expect("registry");
                let registered = apps
                    .iter()
                    .map(|app| app.package.as_str())
                    .collect::<BTreeSet<_>>();
                assert_eq!(
                    registered,
                    STORE_PACKAGES.iter().copied().collect::<BTreeSet<_>>()
                );
                // Versions move with every release, so the check is that
                // each entry carries a version rather than which version it
                // carries. A pinned number here broke every routine catalog
                // bump while catching nothing the shape check misses.
                for app in &apps {
                    let parts: Vec<&str> = app.version.split('.').collect();
                    assert!(
                        parts.len() == 3
                            && parts.iter().all(|part| {
                                !part.is_empty() && part.chars().all(|digit| digit.is_ascii_digit())
                            }),
                        "{} version {:?} is not three dot-separated numbers",
                        app.id,
                        app.version
                    );
                }
            }
        }
    }

    mod preparing {
        use super::super::{
            choose_reader_list, confirmation_answer, dry_run_plan, gzip,
            load_release_package_from_manifest, parse_setup, setup, setup_device_with_confirmation,
            SetupMode, SetupPayload,
        };
        use std::path::PathBuf;

        fn arguments(values: &[&str]) -> Vec<String> {
            values.iter().map(|value| (*value).to_owned()).collect()
        }

        /// A reader that has never had NickelMenu on it.
        ///
        /// A real path rather than a made-up one, because the plan now asks
        /// the volume what is already installed. The first version of this
        /// pointed at /Volumes/KOBOeReader and quietly read whichever device
        /// happened to be plugged in, so the test passed or failed by what was
        /// on somebody's desk.
        fn fresh_reader() -> (setup::Mounted, TempVolume) {
            let volume = TempVolume::new("fresh");
            (mounted(volume.path.clone()), volume)
        }

        /// A reader that already has the plugin, which most do by the second
        /// run of this command.
        fn prepared_reader() -> (setup::Mounted, TempVolume) {
            let volume = TempVolume::new("prepared");
            let folder = volume.path.join(menu_config_folder());
            std::fs::create_dir_all(&folder).expect("the plugin folder");
            std::fs::write(folder.join("doc"), "nickelmenu").expect("the marker");
            (mounted(volume.path.clone()), volume)
        }

        fn menu_config_folder() -> &'static str {
            crate::menu::CONFIG_FOLDER
        }

        struct TempVolume {
            path: PathBuf,
        }

        impl TempVolume {
            fn new(name: &str) -> Self {
                let path = std::env::temp_dir().join(format!(
                    "kobo-plan-{name}-{}-{:?}",
                    std::process::id(),
                    std::thread::current().id()
                ));
                let _ = std::fs::remove_dir_all(&path);
                std::fs::create_dir_all(&path).expect("a volume");
                Self { path }
            }
        }

        impl Drop for TempVolume {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.path);
            }
        }

        fn mounted(volume: PathBuf) -> setup::Mounted {
            setup::Mounted {
                volume,
                serial: "N365410043013".to_owned(),
                firmware: "4.45.23697".to_owned(),
            }
        }

        #[test]
        fn a_bare_run_installs_ejects_and_waits() {
            let parsed = parse_setup(&arguments(&[])).expect("parse");
            assert_eq!(parsed.mode, SetupMode::Install);
            assert!(parsed.eject);
            assert!(parsed.wait);
            assert!(!parsed.dry_run);
            assert!(!parsed.enable_ssh);
            assert!(!parsed.yes);
            assert!(!parsed.non_interactive);
        }

        #[test]
        fn automation_requires_an_explicit_confirmation() {
            let error =
                parse_setup(&arguments(&["--non-interactive"])).expect_err("confirmation required");
            assert!(error.contains("--yes"), "{error}");
            let parsed =
                parse_setup(&arguments(&["--non-interactive", "--yes"])).expect("confirmed");
            assert!(parsed.non_interactive);
            assert!(parsed.yes);
            for declined in ["", "n", "no", "later"] {
                assert!(!confirmation_answer(declined));
            }
            assert!(confirmation_answer("yes\n"));
        }

        #[test]
        fn declined_confirmation_writes_nothing() {
            let volume = TempVolume::new("declined");
            let system = volume.path.join(".kobo");
            std::fs::create_dir_all(&system).expect("system folder");
            std::fs::write(
                system.join("version"),
                "N365410043013,4.9.77,4.45.23697,4.9.77\n",
            )
            .expect("version");
            setup_device_with_confirmation(
                &arguments(&[
                    "--volume",
                    volume.path.to_str().expect("path"),
                    "--source",
                    "--no-eject",
                ]),
                |_, _, _, _| Ok(false),
            )
            .expect("declined cleanly");
            assert!(!volume.path.join(setup::INSTALL_FOLDER).exists());
            assert!(!volume.path.join(setup::SETTINGS).exists());
        }

        #[test]
        fn several_readers_are_refused_without_printing_full_serials() {
            let first = mounted(PathBuf::from("/Volumes/ONE"));
            let mut second = mounted(PathBuf::from("/Volumes/TWO"));
            second.serial = "N418999999999".to_owned();
            let error = choose_reader_list(vec![first, second]).expect_err("ambiguous");
            assert!(error.contains("2 readers"), "{error}");
            assert!(error.contains("/Volumes/ONE"), "{error}");
            assert!(!error.contains("N365410043013"), "{error}");
            assert!(!error.contains("N418999999999"), "{error}");
        }

        #[test]
        fn source_build_setup_remains_an_explicit_supported_path() {
            let parsed = parse_setup(&arguments(&["--source", "--dry-run"])).expect("source");
            assert!(parsed.source);
            assert!(parse_setup(&arguments(&["--source", "--release-dir", "release"])).is_err());
        }

        #[test]
        fn a_verified_prebuilt_device_package_becomes_direct_writer_members() {
            let release = TempVolume::new("prebuilt");
            std::fs::write(release.path.join("channel"), "stable\n").expect("channel");
            let members = vec![
                crate::package::Member {
                    path: format!("{}/bin/kobod", crate::package::INSTALL_ROOT),
                    bytes: b"prebuilt binary".to_vec(),
                    program: true,
                },
                crate::package::Member {
                    path: format!("{}/VERSION", crate::package::INSTALL_ROOT),
                    bytes: format!("{}\n", env!("CARGO_PKG_VERSION")).into_bytes(),
                    program: false,
                },
            ];
            let archive = crate::package::tar(&members).expect("archive");
            let compressed = gzip(&archive).expect("gzip");
            let asset_name = format!("cobalt-{}-KoboRoot.tgz", env!("CARGO_PKG_VERSION"));
            std::fs::write(release.path.join(&asset_name), &compressed).expect("package");
            let manifest = crate::host_release::Manifest {
                version: env!("CARGO_PKG_VERSION").to_owned(),
                channels: vec!["stable".to_owned(), "beta".to_owned()],
                source: "0123456789abcdef0123456789abcdef01234567".to_owned(),
                assets: vec![crate::host_release::Asset {
                    kind: "device".to_owned(),
                    platform: None,
                    name: asset_name,
                    bytes: compressed.len() as u64,
                    sha256: crate::sha256::hex_digest(&compressed),
                }],
            };
            let mut damaged = manifest.clone();
            damaged.assets[0].sha256 = "0".repeat(64);
            let Err(error) = load_release_package_from_manifest(&release.path, damaged) else {
                panic!("bad checksum was accepted");
            };
            assert!(error.contains("checksum failed"), "{error}");
            let payload =
                load_release_package_from_manifest(&release.path, manifest).expect("prebuilt");
            match payload {
                SetupPayload::Prebuilt { built, channel, .. } => {
                    assert_eq!(channel, "stable");
                    assert_eq!(built.members, members);
                }
                SetupPayload::Source => panic!("prebuilt became source build"),
            }

            std::fs::write(release.path.join("channel"), "beta\n").expect("channel");
            let manifest = crate::host_release::Manifest {
                version: env!("CARGO_PKG_VERSION").to_owned(),
                channels: vec!["stable".to_owned(), "beta".to_owned()],
                source: "0123456789abcdef0123456789abcdef01234567".to_owned(),
                assets: vec![crate::host_release::Asset {
                    kind: "device".to_owned(),
                    platform: None,
                    name: format!("cobalt-{}-KoboRoot.tgz", env!("CARGO_PKG_VERSION")),
                    bytes: compressed.len() as u64,
                    sha256: crate::sha256::hex_digest(&compressed),
                }],
            };
            let Err(error) = load_release_package_from_manifest(&release.path, manifest) else {
                panic!("beta USB package was accepted");
            };
            assert!(error.contains("stable releases only"), "{error}");
        }

        #[test]
        fn the_wait_can_be_declined() {
            let parsed = parse_setup(&arguments(&["--no-wait"])).expect("parse");
            assert!(!parsed.wait);
            assert!(
                parsed.eject,
                "declining the wait does not decline the eject"
            );
        }

        #[test]
        fn a_dry_run_of_an_undo_describes_the_undo_and_performs_nothing() {
            // This is the whole reason the two are one mode and not two flags.
            // Read as two booleans, '--undo --dry-run' took the undo branch
            // first and removed Cobalt from a reader nobody had agreed to.
            let parsed = parse_setup(&arguments(&["--undo", "--dry-run"])).expect("parse");
            assert_eq!(parsed.mode, SetupMode::Undo);
            assert!(parsed.dry_run);
            let plan = dry_run_plan(&parsed, &fresh_reader().0);
            assert!(plan.starts_with("would "), "{plan}");
            assert!(plan.contains("would remove"), "{plan}");
            assert!(plan.contains(setup::SSH_ENABLED), "{plan}");
            assert!(!plan.contains("would install"), "{plan}");
        }

        #[test]
        fn a_dry_run_names_every_change_it_would_make() {
            let parsed = parse_setup(&arguments(&["--dry-run"])).expect("parse");
            let plan = dry_run_plan(&parsed, &fresh_reader().0);
            assert!(plan.contains("would install"));
            assert!(plan.contains("leave the firmware's SSH server disabled"));
            assert!(plan.contains("stop after ejecting"));
            for (section, key, value) in setup::SETTINGS_APPLIED {
                assert!(plan.contains(&format!("{section}/{key}={value}")), "{plan}");
            }
        }

        #[test]
        fn root_ssh_requires_an_explicit_opt_in() {
            let parsed = parse_setup(&arguments(&["--enable-ssh", "--dry-run"])).expect("parse");
            assert!(parsed.enable_ssh);
            let plan = dry_run_plan(&parsed, &fresh_reader().0);
            assert!(plan.contains(setup::SSH_DISABLED), "{plan}");
            assert!(plan.contains("root SSH"), "{plan}");
            assert!(plan.contains("wait for the restarted reader"), "{plan}");
        }

        #[test]
        fn a_dry_run_that_will_not_wait_says_so() {
            let parsed = parse_setup(&arguments(&["--dry-run", "--enable-ssh", "--no-wait"]))
                .expect("parse");
            assert!(dry_run_plan(&parsed, &fresh_reader().0).contains("--no-wait was given"));
        }

        #[test]
        fn enabling_ssh_installs_this_machines_key_by_default() {
            let parsed = parse_setup(&arguments(&["--enable-ssh", "--dry-run"])).expect("parse");
            assert!(parsed.authorize_key);
            let plan = dry_run_plan(&parsed, &fresh_reader().0);
            assert!(plan.contains("this machine's public key"), "{plan}");
            assert!(plan.contains("kobo_cobalt"), "{plan}");
            // Both go into the one slot the firmware reads, so the plan has to
            // say so rather than describe two archives that cannot both exist.
            assert!(plan.contains("same .kobo/KoboRoot.tgz"), "{plan}");
            assert!(plan.contains("one authorized_keys"), "{plan}");
        }

        #[test]
        fn no_key_says_why_no_key() {
            let parsed =
                parse_setup(&arguments(&["--enable-ssh", "--no-key", "--dry-run"])).expect("parse");
            assert!(!parsed.authorize_key);
            let plan = dry_run_plan(&parsed, &fresh_reader().0);
            assert!(plan.contains("--no-key was given"), "{plan}");
            assert!(!plan.contains("one authorized_keys"), "{plan}");
        }

        #[test]
        fn without_ssh_there_is_no_key_to_install() {
            let parsed = parse_setup(&arguments(&["--dry-run"])).expect("parse");
            let plan = dry_run_plan(&parsed, &fresh_reader().0);
            assert!(plan.contains("no SSH server to use it"), "{plan}");
        }

        #[test]
        fn a_fresh_reader_is_told_both_things_go_into_the_one_slot() {
            // The firmware extracts exactly one archive, so a plan that
            // described two would be describing something impossible.
            let (reader, _volume) = fresh_reader();
            let parsed = parse_setup(&arguments(&["--enable-ssh", "--dry-run"])).expect("parse");
            let plan = dry_run_plan(&parsed, &reader);
            assert!(plan.contains("stage NickelMenu"), "{plan}");
            assert!(plan.contains("same .kobo/KoboRoot.tgz"), "{plan}");
            assert!(
                plan.contains("NickelMenu's own two files and one authorized_keys"),
                "{plan}"
            );
        }

        #[test]
        fn a_reader_that_already_has_the_plugin_is_not_promised_it_again() {
            // Found on a real reader: the plan said it would stage NickelMenu
            // and put the key in beside it, on a device that already had the
            // plugin and where the key would go in alone.
            let (reader, _volume) = prepared_reader();
            let parsed = parse_setup(&arguments(&["--enable-ssh", "--dry-run"])).expect("parse");
            let plan = dry_run_plan(&parsed, &reader);
            assert!(plan.contains("already installed on this reader"), "{plan}");
            assert!(plan.contains("would not write or reinstall"), "{plan}");
            assert!(plan.contains("any NickelMenu files"), "{plan}");
            assert!(plan.contains("or stage a KoboRoot.tgz"), "{plan}");
            assert!(!plan.contains("stage NickelMenu"), "{plan}");
            assert!(!plan.contains("same .kobo/KoboRoot.tgz"), "{plan}");
            assert!(
                plan.contains("nothing extracted as root but one authorized_keys"),
                "{plan}"
            );
        }

        #[test]
        fn an_unknown_option_is_refused_with_the_whole_usage() {
            let error = parse_setup(&arguments(&["--force"])).expect_err("refused");
            assert!(error.contains("--no-wait"), "{error}");
            assert!(error.contains("--undo"), "{error}");
            assert!(error.contains("--no-key"), "{error}");
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

    #[test]
    fn app_link_maintenance_accepts_only_fixed_actions_and_safe_hosts() {
        let status = vec![
            "status".to_owned(),
            "--device".to_owned(),
            "192.0.2.1".to_owned(),
        ];
        assert_eq!(super::parse_app_link(&status), Ok(("status", "192.0.2.1")));
        let unpair = vec![
            "unpair".to_owned(),
            "-s".to_owned(),
            "reader.local".to_owned(),
        ];
        assert_eq!(
            super::parse_app_link(&unpair),
            Ok(("unpair", "reader.local"))
        );
        for invalid in [
            vec![
                "delete".to_owned(),
                "--device".to_owned(),
                "reader".to_owned(),
            ],
            vec![
                "status".to_owned(),
                "--device".to_owned(),
                "reader;reboot".to_owned(),
            ],
            vec!["status".to_owned()],
        ] {
            assert!(super::parse_app_link(&invalid).is_err());
        }
    }
}

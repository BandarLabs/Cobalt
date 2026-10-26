use std::collections::BTreeSet;
use std::env;
use std::ffi::OsStr;
use std::fmt::Write as _;
use std::fs;
use std::io::{IsTerminal, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

mod apps;
mod authorize;
mod beta_store_smoke;
mod birds;
mod bootstrap;
mod connect;
mod console;
mod deck;
mod detect;
mod devsession;
mod drive;
mod exports;
mod feeds;
mod fieldbook;
mod flashcards;
mod frame;
mod frame_preview;
mod frame_recovery;
mod host_release;
mod menu;
mod musicstand;
mod needles;
mod nonograms;
mod owner_start;
mod package;
mod panels;
mod post;
mod publish;
mod readers;
mod readlater;
mod receipts;
mod report;
mod runtime_dev;
mod server_secret;
mod stream_demo;
mod vault;
// Only the `device-write` build dispatches to this, but its tests decide what
// gets sent to a reader and are worth running on every build. So it compiles
// either way, and the unused warning is silenced rather than the module gated
// out and its tests with it.
#[cfg_attr(not(feature = "device-write"), allow(dead_code))]
mod panel;
mod setup;
mod sha256;
mod sidekick;
mod steps;
mod sync;
mod targets;

const DEVICE_PACKAGES: &[&str] = &["kobo-doctor", "kobod", "kobo-todo", "kobo-terminal"];
const SYNCTHING_SOURCE_RECORD: &str = "\
Syncthing source: https://github.com/syncthing/syncthing.git
Tag: v2.0.9
Commit: 3382ccc3f16536b5a7b6df7c8212951f7d4d3a9f
License: MPL-2.0
Toolchain: go1.24.13
Build: GOOS=linux GOARCH=arm GOARM=7 CGO_ENABLED=0 BUILD_USER=cobalt BUILD_HOST=cobalt \
go run build.go -goos linux -goarch arm build
SHA-256: e7e0523d8db0328b22ebff5c98bd721c94e295122771c0538414898a06ef8ebf
";
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
    ("kobo-books", None),
    ("kobo-sidekick", None),
    ("kobo-store", None),
];
/// Applications released through Store, including the initial built-in copies
/// that users can update, remove and reinstall independently of Cobalt.
const STORE_PACKAGES: &[&str] = &[
    "kobo-arxiv",
    "kobo-audiobook",
    "kobo-backgammon",
    "kobo-birds",
    "kobo-brief",
    "kobo-calibre-web",
    "kobo-chat",
    "kobo-crossword",
    "kobo-deck",
    "kobo-fanshelf",
    "kobo-fieldbook",
    "kobo-flashcards",
    "kobo-frame",
    "kobo-gallery",
    "kobo-grimoire",
    "kobo-gutenbird",
    "kobo-habits",
    "kobo-hn",
    "kobo-homepanel",
    "kobo-inkling",
    "kobo-kitchencard",
    "kobo-lichess",
    "kobo-logicpack",
    "kobo-magnet",
    "kobo-morse",
    "kobo-musicstand",
    "kobo-needles",
    "kobo-nonograms",
    "kobo-panels",
    "kobo-paperterm",
    "kobo-parlor",
    "kobo-parser",
    "kobo-post",
    "kobo-pubquiz",
    "kobo-readlater",
    "kobo-rss",
    "kobo-rss-miniflux",
    "kobo-sidekick",
    "kobo-sudoku",
    "kobo-syncthing",
    "kobo-tictactoe",
    "kobo-todo",
    "kobo-vault",
    "kobo-verses",
    "kobo-zotero-reader",
];
/// Store contributions discovered from their one checked-in manifest.
///
/// Released host commands may not have a source tree beside them, so the
/// built-in list remains the fallback. Source builds add every valid
/// `apps/*/cobalt-app.json` automatically.
pub(crate) fn contributed_store_packages() -> &'static [String] {
    static PACKAGES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    PACKAGES.get_or_init(|| {
        let apps = workspace_manifest()
            .parent()
            .expect("workspace manifest has a parent")
            .join("apps");
        let Ok(entries) = fs::read_dir(apps) else {
            return Vec::new();
        };
        let mut packages = entries
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .filter_map(|entry| {
                let text = fs::read_to_string(entry.path().join("cobalt-app.json")).ok()?;
                let value = kobo_json::parse(&text).ok()?;
                let kobo_json::Value::Object(fields) = value else {
                    return None;
                };
                let ids = fields
                    .iter()
                    .filter(|(name, _)| name == "id")
                    .filter_map(|(_, value)| value.as_str())
                    .collect::<Vec<_>>();
                let [id] = ids.as_slice() else {
                    return None;
                };
                (kobo_protocol::valid_app_id(id) && entry.file_name().to_str() == Some(*id))
                    .then(|| format!("kobo-{id}"))
            })
            .collect::<Vec<_>>();
        packages.sort();
        packages.dedup();
        packages
    })
}
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

Everything is on the same partition your books are on and is visible from any
computer over USB. The managed payload is in this folder; its stable launch
entrypoint is the sibling .adds/cobalt-launch.sh.

For a safe complete removal, run `kobo setup --undo`. It removes the managed
cobalt/current, next, and previous trees, .adds/cobalt-launch.sh, and only the
exact Cobalt entry from .adds/nm/cobalt or .adds/nm/menu. Owner folders are
moved to .adds/cobalt.recovery.N first. Inspect those directories and any
.adds/cobalt.unusable[.N] quarantine before deleting recoverable data.
Nothing was written to the system partition and no startup script was added.

To start it: run .adds/cobalt-launch.sh. If you have NickelMenu installed, add
this one line to .adds/nm/menu to get an entry in the reader's own menu:

  menu_item :main    :Cobalt    :cmd_spawn    :quiet:/mnt/onboard/.adds/cobalt-launch.sh

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

Everything lands in .adds/cobalt plus the stable .adds/cobalt-launch.sh on the
same drive. Use `kobo setup --undo` for a complete safe uninstall: it also
removes the exact Cobalt NickelMenu entry and preserves owner folders under
.adds/cobalt.recovery.N. Inspect recovery and .adds/cobalt.unusable[.N]
directories before deleting them. Nothing is written to the system partition.";

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
/// Renew well before the two-minute kernel lease expires, allowing several
/// missed probes without leaving an indefinite hold after a disconnect.
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
        let arguments = match with_default_reader(&arguments) {
            Ok(filled) => filled,
            Err(error) => {
                eprintln!("kobo: {}", console::display(&error));
                return ExitCode::from(console::category_of(&error));
            }
        };
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
            eprintln!("kobo: {}", console::display(&error));
            ExitCode::from(console::category_of(&error))
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

/// Whether a companion command should print its usage and exit successfully.
fn wants_help(arguments: &[String]) -> bool {
    arguments.is_empty()
        || arguments
            .iter()
            .any(|argument| argument == "--help" || argument == "-h")
}

// Returns a Result it can never fail to produce so every `command` can end
// with `return print_command_help(USAGE);` rather than a print and a separate
// Ok, which is the shape all nine callers use.
#[allow(
    clippy::unnecessary_wraps,
    reason = "the Result is the caller's tail expression, not a failure channel"
)]
fn print_command_help(usage: &str) -> Result<(), String> {
    println!("{usage}");
    Ok(())
}

/// Resolves an alias to the command it stands for.
fn canonical(command: &str) -> &str {
    ALIASES
        .iter()
        .find_map(|(alias, name)| (*alias == command).then_some(*name))
        .unwrap_or(command)
}

fn run_owner_menu() -> Result<(), String> {
    let console = console::Console::detect();
    if console.interactive() {
        let selected =
            owner_start::choose(&mut std::io::stdin().lock(), &mut std::io::stdout().lock())?;
        if let Some(selected) = selected {
            return run(&selected);
        }
    } else {
        // A bare `kobo` in a pipe prints what there is to know and exits,
        // rather than blocking on answers nobody can give.
        println!("{}", owner_start::COMPACT_HELP);
    }
    Ok(())
}

/// Sends a file to the companion that reads it.
///
/// The owner names the file; [`detect`] names the companion. An explicit
/// path is the command line's half of the bargain (the guided surface asks
/// for one when it is driving), an ambiguous container is settled by `--app`
/// or by asking, and the target flags are the shared ones, so a saved reader
/// name works here exactly as it does everywhere else. `--preview` looks
/// before anything is sent, and every acknowledged send leaves a receipt,
/// so re-sending an unchanged file is reported as already done rather than
/// pushed twice.
fn send_file(arguments: &[String]) -> Result<(), String> {
    const USAGE: &str = "usage: kobo send FILE [--app APP] (--sim | --device IP | --reader NAME)\n\
                         \x20      kobo send --preview FILE --out DIRECTORY [--app APP] [--profile PROFILE]\n\
                         \x20      send routes a file to the companion that reads it:\n\
                         \x20      photos to Frame, OPML lists to Feeds, CBZ comics to Panels,\n\
                         \x20      story files to Parser, APKG/COLPKG decks to Flashcards.\n\
                         \x20      --app settles a container more than one companion reads.\n\
                         \x20      --preview renders or checks the content locally; nothing is sent.\n\
                         \x20      --retry repeats the kept selection after a failed send.";
    if wants_help(arguments) {
        return print_command_help(USAGE);
    }
    let (arguments, target_words) = send_arguments(arguments)?;
    let (target, rest) = targets::TargetArgs::parse(&arguments)?;
    let options = SendOptions::parse(&rest, USAGE)?;
    let file = options.file.ok_or_else(|| console::usage(USAGE))?;
    let path = Path::new(&file);
    if !path.is_file() {
        return Err(console::usage(format!(
            "{file}: no such file on this computer"
        )));
    }
    let candidates = detect::candidates(path);
    let chosen = match detect::choose(&candidates, options.app.as_deref()) {
        Ok(companion) => companion,
        Err(detect::ChooseError::Ambiguous(offered)) => match pick_companion(&offered)? {
            Some(companion) => companion,
            None => return Ok(()),
        },
        Err(detect::ChooseError::Message(error)) => return Err(error),
    };

    // Preview is local by definition: it renders or checks for the owner's
    // eyes on this computer, so target flags are a misunderstanding.
    if options.preview {
        if target != targets::TargetArgs::default() {
            return Err(console::usage(
                "a preview is local; it takes no target flags",
            ));
        }
        return send_preview(
            chosen,
            &file,
            options.out.as_deref(),
            options.profile.as_deref(),
        );
    }
    if options.out.is_some() || options.profile.is_some() {
        return Err(console::usage("--out and --profile belong to --preview"));
    }

    let resolved = target.resolve()?;
    // A receipt belongs to a reader, not to one spelling of it: a nickname
    // and its address are the same device, and the serial outlives a new
    // DHCP lease. Fall back to the address only when no serial answers.
    let target_label = match &resolved {
        targets::Target::Simulator => "sim".to_owned(),
        targets::Target::Address(host) => {
            targets::probe_serial(host).unwrap_or_else(|| host.clone())
        }
        targets::Target::Nickname(name) => {
            let host = targets::resolve_nickname(name)?;
            targets::probe_serial(&host).unwrap_or(host)
        }
    };
    let device_flags = match resolved {
        targets::Target::Simulator => vec!["--sim".to_owned()],
        targets::Target::Address(host) => vec!["--device".to_owned(), host],
        targets::Target::Nickname(name) => {
            vec!["--device".to_owned(), targets::resolve_nickname(&name)?]
        }
    };

    // Preparing is real work: hashing the content is what makes "already
    // sent" mean the same bytes, and it is what a resumed send compares.
    console::Console::progress(&format!("preparing {file} for {}", chosen.name()));
    let sha = receipts::hash_file(path)?;
    let receipts_path = receipts::receipts_path();
    let mut ledger = receipts::Receipts::load(&receipts_path)?;
    if chosen != detect::Companion::Flashcards {
        if let Some(receipt) = ledger.find(chosen.name(), &target_label, &sha) {
            println!(
                "{file} is unchanged since it was sent to {target_label} (receipt at {}); nothing to do",
                receipt.at
            );
            return Ok(());
        }
    }

    send_dispatch(
        chosen,
        &file,
        &device_flags,
        &target_label,
        &target_words,
        &mut ledger,
        &receipts_path,
        sha,
    )
}

/// The transfer itself: keep the selection, dispatch to the companion, and
/// only past its acknowledgement clear the pending send and write the
/// receipt - an interrupted transfer keeps the one and never gains the
/// other, which is what makes the next send a resume rather than a
/// duplicate.
#[allow(clippy::too_many_arguments)]
fn send_dispatch(
    chosen: detect::Companion,
    file: &str,
    device_flags: &[String],
    target_label: &str,
    target_words: &str,
    ledger: &mut receipts::Receipts,
    receipts_path: &std::path::Path,
    sha: String,
) -> Result<(), String> {
    let pending_path = receipts::pending_path();
    let pending = receipts::Pending {
        file: file.to_owned(),
        app: Some(chosen.name().to_owned()),
        target: target_words.to_owned(),
    };
    if let Err(error) = pending.save(&pending_path) {
        println!("note: this send could not be kept for --retry ({error}); the send itself is unaffected");
    }
    let mut forwarded = vec!["push".to_owned(), file.to_owned()];
    forwarded.extend(device_flags.iter().cloned());
    let sent = match chosen {
        detect::Companion::Frame => frame::command(&forwarded),
        detect::Companion::Feeds => feeds::command(&forwarded),
        detect::Companion::Panels => panels::command(&forwarded),
        detect::Companion::Parser => parser_command(&forwarded),
        detect::Companion::Needles => needles::command(&forwarded),
        detect::Companion::Flashcards => {
            // Decks are a two-step import, not a push: the helper merges into
            // a collection, and staging needs a mounted reader. Say so with
            // the real commands rather than pretending a push happened.
            println!(
                "{file} is a study deck. Decks import into a collection first, then stage:\n  kobo flashcards import {file} --merge COLLECTION.cobfc\n  kobo flashcards stage COLLECTION.cobfc --kobo-root MOUNT"
            );
            receipts::Pending::clear(&pending_path);
            return Ok(());
        }
        detect::Companion::Fanshelf => {
            // Fanshelf downloads the works on its shelf itself, from the
            // reader; there is no host-side shelf push to forward to. Say so
            // rather than pretending a push happened.
            println!(
                "{file} is an EPUB book. Fanshelf shelves works it downloads itself: add the work from the app on the reader."
            );
            receipts::Pending::clear(&pending_path);
            return Ok(());
        }
    };
    if let Err(error) = sent {
        return Err(console::Console::with_details(
            format!("{error}\nThe selection is kept; retry with: kobo send --retry"),
            &format!("forwarded: kobo {} {}", chosen.name(), forwarded.join(" ")),
        ));
    }
    receipts::Pending::clear(&pending_path);
    ledger.record(receipts::Receipt {
        file: file.to_owned(),
        app: chosen.name().to_owned(),
        target: target_label.to_owned(),
        sha256: sha,
        at: steps::now(),
    });
    if let Err(error) = ledger.save(receipts_path) {
        println!(
            "note: the send's receipt could not be written ({error}); the send itself finished"
        );
    }
    console::Console::progress(&format!("sent to {target_label}"));
    Ok(())
}

/// The arguments a send runs with, and its target flags as words.
///
/// Ordinarily the arguments as given. With `--retry` they are rebuilt from
/// the kept pending send: same file, same companion, same target, none of
/// it retyped.
fn send_arguments(arguments: &[String]) -> Result<(Vec<String>, String), String> {
    if !arguments.iter().any(|argument| argument == "--retry") {
        return Ok((arguments.to_vec(), target_words(arguments)));
    }
    let pending_path = receipts::pending_path();
    let pending = receipts::Pending::load(&pending_path)?
        .ok_or_else(|| console::target("nothing is waiting to be retried"))?;
    let mut rebuilt = vec![pending.file.clone()];
    if let Some(app) = pending.app.clone() {
        rebuilt.push("--app".to_owned());
        rebuilt.push(app);
    }
    rebuilt.extend(pending.target.split(' ').map(str::to_owned));
    console::Console::progress(&format!("retrying the kept send of {}", pending.file));
    let words = pending.target.clone();
    Ok((rebuilt, words))
}

/// Writes a diagnostic report: redacted by default, `--include-paths` when
/// the file names are the question, `--out` to a file or stdout without.
fn report_command(arguments: &[String]) -> Result<(), String> {
    const USAGE: &str = "usage: kobo report [--out FILE] [--include-paths]\n\
                         \x20      A diagnostic snapshot for a helper: versions, counts and kinds.\n\
                         \x20      Never file contents, trust material, keys or full serials;\n\
                         \x20      paths only when --include-paths is given.";
    if wants_help(arguments) {
        return print_command_help(USAGE);
    }
    let mut out = None;
    let mut include_paths = false;
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--out" if out.is_none() => {
                out = Some(
                    arguments
                        .next()
                        .ok_or_else(|| console::usage("--out takes a file"))?
                        .clone(),
                );
            }
            "--include-paths" if !include_paths => include_paths = true,
            _ => return Err(console::usage(USAGE)),
        }
    }
    let text = report::build(include_paths);
    match out {
        Some(path) => {
            std::fs::write(&path, &text)
                .map_err(|error| format!("{path} cannot be written: {error}"))?;
            println!("report written to {path}");
        }
        None => print!("{text}"),
    }
    Ok(())
}

/// The target flags exactly as given, so a kept send retries the same way.
fn target_words(arguments: &[String]) -> String {
    let mut words = Vec::new();
    let mut arguments = arguments.iter().peekable();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--sim" => words.push("--sim".to_owned()),
            "--device" | "-s" | "--reader" => {
                words.push(argument.clone());
                if let Some(value) = arguments.next() {
                    words.push(value.clone());
                }
            }
            _ => {}
        }
    }
    words.join(" ")
}

/// What a send run was asked for, beyond the shared target flags.
#[derive(Default)]
struct SendOptions {
    file: Option<String>,
    app: Option<String>,
    preview: bool,
    out: Option<String>,
    profile: Option<String>,
}

impl SendOptions {
    fn parse(arguments: &[String], usage: &str) -> Result<Self, String> {
        let mut options = Self::default();
        let mut arguments = arguments.iter();
        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--app" if options.app.is_none() => {
                    options.app = Some(
                        arguments
                            .next()
                            .ok_or_else(|| console::usage("--app takes a companion name"))?
                            .clone(),
                    );
                }
                "--preview" if !options.preview => options.preview = true,
                "--out" if options.out.is_none() => {
                    options.out = Some(
                        arguments
                            .next()
                            .ok_or_else(|| console::usage("--out takes a directory"))?
                            .clone(),
                    );
                }
                "--profile" if options.profile.is_none() => {
                    options.profile = Some(
                        arguments
                            .next()
                            .ok_or_else(|| console::usage("--profile takes a profile id"))?
                            .clone(),
                    );
                }
                _ if options.file.is_none() && !argument.starts_with('-') => {
                    options.file = Some(argument.clone());
                }
                _ => return Err(console::usage(usage)),
            }
        }
        Ok(options)
    }
}

/// Asks which companion an ambiguous container goes to. A pipe cannot ask,
/// so it gets a usage error naming `--app`; a blank answer cancels.
fn pick_companion(candidates: &[detect::Companion]) -> Result<Option<detect::Companion>, String> {
    let names: Vec<String> = candidates
        .iter()
        .map(|companion| companion.name().to_owned())
        .collect();
    if !std::io::stdin().is_terminal() {
        return Err(console::usage(format!(
            "several companions could take it: {} - name one with --app ({})",
            names.join(", "),
            names.join("|")
        )));
    }
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let mut output = std::io::stdout();
    Ok(console::choose_numbered(
        &mut input,
        &mut output,
        &names,
        "Send it to which companion (blank cancels): ",
    )?
    .map(|index| candidates[index]))
}

/// The local half of send: render or check the content, transfer nothing.
fn send_preview(
    app: detect::Companion,
    file: &str,
    out: Option<&str>,
    profile: Option<&str>,
) -> Result<(), String> {
    let app_name = app.name();
    match app {
        detect::Companion::Frame | detect::Companion::Panels => {
            let out = out.ok_or_else(|| {
                console::usage(format!("a {app_name} preview needs --out DIRECTORY"))
            })?;
            // frame_preview owns its verb already (frame strips it); panels
            // dispatches its own, so each gets the argv shape it expects.
            let mut forwarded = vec![file.to_owned(), "--out".to_owned(), out.to_owned()];
            if let Some(profile) = profile {
                forwarded.push("--profile".to_owned());
                forwarded.push(profile.to_owned());
            }
            if app == detect::Companion::Frame {
                frame_preview::command(&forwarded)
            } else {
                forwarded.insert(0, "preview".to_owned());
                panels::command(&forwarded)
            }
        }
        detect::Companion::Feeds => feeds::command(&["check".to_owned(), file.to_owned()]),
        detect::Companion::Parser => parser_command(&["inspect".to_owned(), file.to_owned()]),
        detect::Companion::Needles => {
            let out = out.ok_or_else(|| {
                console::usage(format!("a {app_name} preview needs --out DIRECTORY"))
            })?;
            needles::command(&[
                "preview".to_owned(),
                file.to_owned(),
                "--out".to_owned(),
                out.to_owned(),
            ])
        }
        detect::Companion::Flashcards => {
            println!(
                "{file} is a study deck. Preview the collection it merges into:\n  kobo flashcards preview COLLECTION.cobfc --out PREVIEW.html"
            );
            Ok(())
        }
        detect::Companion::Fanshelf => {
            println!(
                "{file} is an EPUB book. Fanshelf shelves works it downloads itself: add the work from the app on the reader."
            );
            Ok(())
        }
    }
}

/// The command line as it will run: an alias spelled out, and the reader it
/// acts on filled in from `kobo pair` or `KOBO_READER` when none is named.
///
/// The alias goes first so `kobo logcat` finds the same reader `kobo logs`
/// does, rather than being the one spelling that still asks for an address.
fn with_default_reader(arguments: &[String]) -> Result<Vec<String>, String> {
    let mut line = arguments.to_vec();
    if let Some(first) = line.first_mut() {
        *first = canonical(first).to_owned();
    }
    targets::with_reader(
        &line,
        targets::configured_default(),
        targets::resolve_nickname,
        |said| eprintln!("{said}"),
    )
}

fn run(arguments: &[String]) -> Result<(), String> {
    let filled = with_default_reader(arguments)?;
    let arguments = filled.as_slice();
    let Some(command) = arguments.first().map(String::as_str) else {
        return run_owner_menu();
    };
    match canonical(command) {
        "pair" => pair_command(&arguments[1..]),
        "new" => create_app(arguments.get(1).ok_or("usage: kobo new <name>")?),
        "dev" => dev(&arguments[1..]),
        "drive" => drive_command(&arguments[1..]),
        "deck" => deck::command(&arguments[1..]),
        "flashcards" => flashcards::command(&arguments[1..]),
        "frame" => frame::command(&arguments[1..]),
        "musicstand" => musicstand::command(&arguments[1..]),
        "post" => post::command(&arguments[1..]),
        "readlater" => readlater::command(&arguments[1..]),
        "birds" => birds::command(&arguments[1..]),
        "vault" => vault::command(&arguments[1..]),
        "sync" => sync::command(&arguments[1..]),
        "sidekick" => sidekick::command(&arguments[1..]),
        "export" => exports::command(&arguments[1..]),
        "feeds" => feeds::command(&arguments[1..]),
        "send" => send_file(&arguments[1..]),
        "report" => report_command(&arguments[1..]),
        "fieldbook" => fieldbook::command(&arguments[1..]),
        "needles" => needles::command(&arguments[1..]),
        "nonograms" => nonograms::command(&arguments[1..]),
        "parser" => parser_command(&arguments[1..]),
        "panels" => panels::command(&arguments[1..]),
        "shot" => shot_command(&arguments[1..]),
        #[cfg(feature = "device-write")]
        "tap" => tap_command(&arguments[1..]),
        #[cfg(feature = "device-write")]
        "present" => panel::present(&arguments[1..]),
        #[cfg(feature = "device-write")]
        "stop" => panel::stop(&arguments[1..]),
        #[cfg(not(feature = "device-write"))]
        "present" | "stop" => Err(console::unsupported(format!(
            "{command} takes the panel, so it is not compiled in; rebuild the CLI with \
             --features device-write"
        ))),
        "build" => build_device(arguments.iter().any(|argument| is_device_flag(argument))),
        "doctor" => doctor(&arguments[1..]),
        "devices" => list_devices(&arguments[1..]),
        "app-link" => app_link_command(&arguments[1..]),
        "session" => dev_session(&arguments[1..]),
        "wait" => wait_for_device(&arguments[1..]),
        "logs" => device_logs(&arguments[1..]),
        "wifi-trace" => wifi_trace_command(&arguments[1..]),
        "stream" => stream_command(&arguments[1..]),
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
        "guard-test" => Err(console::unsupported(
            "guard-test is not compiled in; rebuild the CLI with --features device-write",
        )),
        #[cfg(not(feature = "device-write"))]
        "smoke-display" => Err(console::unsupported(
            "smoke-display is not compiled in; rebuild the CLI with --features device-write",
        )),
        "package" => build_package(&arguments[1..]),
        "app-key" => app_key(&arguments[1..]),
        "app-bundle" => app_bundle(&arguments[1..]),
        "app-verify" => app_verify(&arguments[1..]),
        "app-catalog-verify" => app_catalog_verify(&arguments[1..]),
        "app-catalog" => app_catalog(&arguments[1..]),
        "app-list" => app_list(&arguments[1..]),
        "app-check" => app_check(&arguments[1..]),
        "app-release" => app_release(&arguments[1..]),
        "beta-store-smoke" => beta_store_smoke::command(&arguments[1..]),
        "host-release-sign" => host_release_sign(&arguments[1..]),
        "host-release-verify" => host_release_verify(&arguments[1..]),
        "update" => update_host(&arguments[1..]),
        "setup" => setup_device(&arguments[1..]),
        "apps" => apps::command(&arguments[1..]),
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
        "version" | "--version" | "-V" => version_command(&arguments[1..]),
        unknown => Err(format!("unknown command '{unknown}'")),
    }
}

/// `kobo version` is one line a script can read; `--full` is the
/// compatibility report a person reads before an update: what this host is,
/// which helpers it can see, and how signed updates are checked.
fn version_command(arguments: &[String]) -> Result<(), String> {
    match arguments {
        [] => {
            println!("kobo {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        [flag] if flag == "--full" => {
            println!("kobo {}", env!("CARGO_PKG_VERSION"));
            println!("host: {} {}", std::env::consts::OS, std::env::consts::ARCH);
            for helper in ["kobo-doctor", "flashcards-import"] {
                println!("helper: {}", helper_status(helper));
            }
            println!(
                "updates: host release packages are signed; setup verifies the manifest signature before anything is installed"
            );
            println!(
                "compatibility: a package built for a newer kobo than {} is refused with an update prompt, and each companion declares the minimum kobo it needs in the signed manifest",
                env!("CARGO_PKG_VERSION")
            );
            println!(
                "reader: a reader's own model and firmware are read from the device itself - 'kobo doctor --device HOST' or a setup dry run"
            );
            Ok(())
        }
        _ => Err(console::usage("usage: kobo version [--full]")),
    }
}

/// One line about a helper: its version when it answers, its path problem
/// when it cannot run, "not installed" when it is absent.
fn helper_status(name: &str) -> String {
    // The probe is the helper answering --version: present helpers name
    // themselves, absent ones are reported absent, and a lookup that itself
    // fails (an unsearchable PATH entry, a dangling sibling) says the lookup
    // failed rather than guessing either way.
    match Command::new(sibling_binary(name)).arg("--version").output() {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout);
            format!("{name} {}", version.trim())
        }
        Ok(output) => format!("{name} present but --version exited {}", output.status),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            format!("{name} not installed (looked beside kobo and on PATH)")
        }
        Err(error) => format!("{name} could not be looked up: {error}"),
    }
}

const PARSER_USAGE: &str = "usage: kobo parser inspect FILE\n\
                         \x20      kobo parser push FILE (--sim | --device IP) [--replace]\n\
                         inspect reports format, title and Parser compatibility.\n\
                         push validates with the interpreter's shared inspector first.";

fn parser_command(arguments: &[String]) -> Result<(), String> {
    if wants_help(arguments) {
        return print_command_help(PARSER_USAGE);
    }
    if let [verb, file] = arguments {
        if matches!(verb.as_str(), "inspect" | "check") {
            let path = Path::new(file);
            let bytes = fs::read(path)
                .map_err(|error| format!("could not read {}: {error}", path.display()))?;
            let info =
                kobo_zstory::StoryInfo::inspect(&bytes, file).map_err(|error| error.to_string())?;
            println!(
                "Title: {}\nFormat: {}\nCompatibility: {}\nRelease: {}\nSerial: {}\nChecksum: {:04x}\nSize: {} bytes",
                info.title,
                info.format(),
                info.compatibility(),
                info.release,
                String::from_utf8_lossy(&info.serial),
                info.checksum,
                info.bytes
            );
            return Ok(());
        }
    }
    if arguments.first().map(String::as_str) != Some("push") {
        return Err(PARSER_USAGE.to_owned());
    }
    let file = arguments.get(1).ok_or_else(|| PARSER_USAGE.to_owned())?;
    let mut target: Option<String> = None;
    let mut replace = false;
    let mut index = 2;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--sim" => {
                if target.is_some() {
                    return Err(PARSER_USAGE.to_owned());
                }
                target = Some(String::new());
                index += 1;
            }
            flag if is_device_flag(flag) => {
                let host = arguments
                    .get(index + 1)
                    .ok_or_else(|| PARSER_USAGE.to_owned())?;
                if !valid_device_host(host) {
                    return Err("device host contains unsupported characters".to_owned());
                }
                if target.replace(host.clone()).is_some() {
                    return Err(PARSER_USAGE.to_owned());
                }
                index += 2;
            }
            "--replace" => {
                replace = true;
                index += 1;
            }
            _ => return Err(PARSER_USAGE.to_owned()),
        }
    }
    let path = Path::new(file);
    let bytes =
        fs::read(path).map_err(|error| format!("could not read {}: {error}", path.display()))?;
    let info = kobo_zstory::StoryInfo::inspect(&bytes, file).map_err(|error| error.to_string())?;
    let name = info.shelf_name(file);
    match target.ok_or_else(|| PARSER_USAGE.to_owned())? {
        host if host.is_empty() => parser_publish_local(&name, &bytes, replace)?,
        host => parser_publish_remote(&host, &name, &bytes, replace)?,
    }
    println!(
        "Parser story ready: {} ({}, {})",
        info.title,
        info.format(),
        info.compatibility()
    );
    Ok(())
}

fn parser_publish_local(name: &str, bytes: &[u8], replace: bool) -> Result<(), String> {
    let root = kobo_sim::simulated_data_root("parser");
    fs::create_dir_all(&root).map_err(|e| format!("create Parser shelf: {e}"))?;
    let final_path = root.join(name);
    if final_path.exists() && !replace {
        return Err(format!(
            "{name} is already on the Parser shelf; use --replace to overwrite it"
        ));
    }
    let partial = root.join(format!(".{name}.{}.writing", std::process::id()));
    fs::write(&partial, bytes).map_err(|e| format!("write Parser staging file: {e}"))?;
    fs::rename(&partial, &final_path).map_err(|e| format!("publish Parser story: {e}"))
}
fn parser_publish_remote(
    host: &str,
    name: &str,
    bytes: &[u8],
    replace: bool,
) -> Result<(), String> {
    let encoded = base64_encode(bytes);
    let count = bytes.len();
    let digest = kobo_net::sha256::hex_digest(bytes);
    let overwrite = if replace {
        "true"
    } else {
        "test ! -e \"$final\""
    };
    let script = format!(
        "set -eu\nroot=/mnt/onboard/.adds/cobalt/data/parser\nmkdir -p \"$root\"\npartial=\"$root/.{name}.$$.writing\"\nfinal=\"$root/{name}\"\n{overwrite}\ntrap 'rm -f \"$partial\"' EXIT HUP INT TERM\nbase64 -d > \"$partial\" <<'KOBO_PARSER_STORY'\n{encoded}\nKOBO_PARSER_STORY\ntest \"$(wc -c < \"$partial\")\" = '{count}'\nset -- $(sha256sum \"$partial\"); test \"$1\" = '{digest}'\nchmod 600 \"$partial\"\nmv -f \"$partial\" \"$final\"\nsync\n"
    );
    let output = run_remote_shell(&format!("root@{host}"), &script, REMOTE_COMMAND_TIMEOUT)
        .map_err(unreachable_device)?;
    if !output.status.success() {
        return Err(format!(
            "the reader refused the story transfer: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

#[cfg(test)]
fn validate_parser_story(bytes: &[u8]) -> Result<(), String> {
    kobo_zstory::StoryInfo::inspect(bytes, "story.z5")
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// Runs the host half of a Paperterm session.
fn wifi_trace_command(arguments: &[String]) -> Result<(), String> {
    const USAGE: &str = "usage: kobo wifi-trace summarize PATH | \
                         kobo wifi-trace retrieve --device HOST --out PATH";
    match arguments {
        [action, path] if action == "summarize" => {
            let bytes = fs::read(path).map_err(|error| format!("read {path}: {error}"))?;
            println!("{}", kobo_wifi_trace::summarize(&bytes).render());
            Ok(())
        }
        [action, device, host, out, path]
            if action == "retrieve" && is_device_flag(device) && out == "--out" =>
        {
            if !valid_device_host(host) {
                return Err("device host contains unsupported characters".to_owned());
            }
            let script = format!(
                "set -e\n\
                 dir={directory}\n\
                 latest=$(ls -1t \"$dir\"/wifi-handoff-v1-*.jsonl 2>/dev/null | head -n 1)\n\
                 if [ -z \"$latest\" ]; then\n\
                   echo 'no Wi-Fi handoff trace on this device' >&2\n\
                   exit 3\n\
                 fi\n\
                 cat \"$latest\"\n",
                directory = kobo_wifi_trace::DIAGNOSTICS_DIR,
            );
            let remote = format!("root@{host}");
            let output = run_remote_shell(&remote, &script, REMOTE_SESSION_TIMEOUT)
                .map_err(unreachable_device)?;
            if !output.status.success() {
                return Err(match output.status.code() {
                    Some(3) => "no Wi-Fi handoff trace to retrieve".to_owned(),
                    _ => unreachable_device(format!(
                        "retrieving the Wi-Fi handoff trace from {host} failed"
                    )),
                });
            }
            fs::write(path, &output.stdout)
                .map_err(|error| format!("write retrieved trace to {path}: {error}"))?;
            println!(
                "saved {} bytes to {path}\n{}",
                output.stdout.len(),
                kobo_wifi_trace::summarize(&output.stdout).render()
            );
            Ok(())
        }
        _ => Err(USAGE.to_owned()),
    }
}

/// Prepares Paperterm pairing and installs the trust root on a named reader.
///
/// What this replaces: `kobo stream init` minted a certificate, printed
/// "address your-computer:9332" when nobody had passed --host, and told the
/// owner to go and run `kobo trust set stream --device READER_IP`. Both
/// addresses were things the companion could find out for itself, and the
/// second command was one more thing to get wrong before anything worked.
///
/// So it finds this computer's address, finds the readers on the same network
/// and names them, and installs the trust root on the one that was chosen. A
/// reader can still be named outright with --device, and --host still overrides
/// the address the certificate is minted for. Once a reader is saved with
/// `kobo pair` it is that reader, found by its serial, and never whichever
/// reader happens to answer.
fn stream_init(arguments: &[String]) -> Result<(), String> {
    const USAGE: &str =
        "usage: kobo stream init [--device IP] [--reader NAME] [--host ADDRESS ...]";
    let mut device = None;
    let mut nickname = None;
    let mut hosts = Vec::new();
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--device" | "-s" => {
                device = Some(arguments.get(index + 1).ok_or(USAGE)?.clone());
                index += 2;
            }
            "--reader" => {
                nickname = Some(arguments.get(index + 1).ok_or(USAGE)?.clone());
                index += 2;
            }
            "--host" => {
                hosts.push("--host".to_owned());
                hosts.push(arguments.get(index + 1).ok_or(USAGE)?.clone());
                index += 2;
            }
            _ => return Err(USAGE.to_owned()),
        }
    }
    // A reader named on the command line is checked before anything is minted
    // or printed, so a typo does not leave a half-done pairing behind.
    let search = match device {
        Some(host) => {
            if !valid_device_host(&host) {
                return Err(format!("{host:?} is not an address this can reach"));
            }
            ReaderSearch::Found(host)
        }
        None => saved_reader_search(nickname.as_deref())?,
    };
    // The certificate has to name an address the reader can reach. Without
    // --host that is this computer's own address, which it can find.
    if hosts.is_empty() {
        if let Some(address) = connect::local_address() {
            println!("Minting the certificate for this computer at {address}.");
            hosts.push("--host".to_owned());
            hosts.push(address);
        }
    }
    kobo_stream::init(&hosts)?;
    // Minting is not pairing, and this used to read as though it were: the
    // identity was minted, "Paperterm is ready to pair" was printed with a
    // code, and only then did the last line admit the trust root was never
    // installed. It then exited 0. An owner following it in order opened
    // Paperterm, typed a code that could not work, and was told the pairing
    // was refused. The certificate is still kept, because it is what the
    // instruction below needs, but nothing here claims to be ready.
    let ReaderSearch::Found(reader) = search else {
        return Err(format!(
            "{}\n\nThe computer's identity and pairing code are saved and stay valid:\n{}",
            search.explain(),
            kobo_stream::pairing_details(kobo_stream::DEFAULT_PORT)?
        ));
    };
    let authority = stream_authority()?;
    println!("Installing the trust root on {reader}.");
    trust_set("stream", &authority, &SecretTarget::Device(reader.clone()))?;
    // Remember the pairing by serial, so the next command can name this
    // reader and find it again after its address changes.
    if let Some(identity) = identify_device(&reader).filter(connect::Identity::is_kobo) {
        let path = readers::store_path();
        let mut store = readers::Store::load(&path)?;
        store.record_pairing(&identity.serial, &reader, nickname.as_deref());
        store.save(&path)?;
    }
    // Said once the trust root is actually on the reader, and carrying the
    // address and code the owner now needs, which the mint used to print
    // before anyone knew whether this point would be reached.
    println!(
        "Paperterm is paired with {reader}. Open it on the reader and enter:\n{}",
        kobo_stream::pairing_details(kobo_stream::DEFAULT_PORT)?
    );
    Ok(())
}

/// The reader `stream init` pairs when no address is given.
///
/// A saved reader is found by its serial and nowhere else. This used to sweep
/// every time, so with the paired reader asleep and a neighbour's Kobo awake,
/// the neighbour's was the one that answered and the trust root went onto it.
/// A sweep is left for the first pairing, when nothing is saved to look for,
/// and for a name not saved yet, which is the owner asking for a new one.
fn saved_reader_search(nickname: Option<&str>) -> Result<ReaderSearch, String> {
    let store = readers::Store::load(&readers::store_path())?;
    let default = match nickname {
        Some(name) if store.find(name).is_some() => {
            Some(targets::DefaultReader::Nickname(name.to_owned()))
        }
        Some(_) => None,
        None => targets::configured_default(),
    };
    match default {
        Some(targets::DefaultReader::Address(address)) => Ok(ReaderSearch::Found(address)),
        Some(targets::DefaultReader::Nickname(name)) => {
            let address = targets::resolve_nickname(&name)?;
            println!("Using your saved reader \"{name}\" at {address}.");
            Ok(ReaderSearch::Found(address))
        }
        None if nickname.is_none() && !store.readers.is_empty() => Err(console::target(format!(
            "{} readers are saved on this computer, so this will not guess which one to \
                 pair. Name it: kobo stream init --reader NAME",
            store.readers.len()
        ))),
        None => Ok(choose_reader()),
    }
}

/// What a sweep for readers concluded.
///
/// Three different things used to arrive at the caller as one `None`, and a
/// caller that cannot tell them apart can only guess which to advise about.
/// It guessed "no reader answered", which is a falsehood when several did.
enum ReaderSearch {
    /// Exactly one answered, so there is nothing to choose.
    Found(String),
    /// No route to any network, so no sweep was run at all.
    NoRoute,
    /// The sweep ran and nothing answered.
    NoneAnswered,
    /// Several answered. Their addresses have already been listed.
    Several(usize),
}

impl ReaderSearch {
    /// Why no single reader came out of this, and what to do about it.
    ///
    /// Each case earns its own next step: a reader that is not on the network
    /// has to be put there, and a reader that is one of several only has to be
    /// named, which is a different sentence and a different fix.
    fn explain(&self) -> String {
        match self {
            Self::Found(_) => String::new(),
            Self::NoRoute => "This computer has no route to a network, so no reader can be \
                 reached from here. Join the Wi-Fi the reader is on and run this again."
                .to_owned(),
            Self::NoneAnswered => "No reader was reached, so the trust root is not installed \
                 and Paperterm cannot pair yet.\n\nPut the reader on this Wi-Fi and run this \
                 again, or name it directly:\n  kobo stream init --device READER_IP\n\nThe \
                 reader does not show its own address anywhere on the device yet, so take \
                 READER_IP from the list of clients on your router."
                .to_owned(),
            Self::Several(count) => format!(
                "{count} readers answered, so this will not guess which one to pair. Name the \
                 one you want from the addresses listed above:\n  kobo stream init --device \
                 READER_IP"
            ),
        }
    }
}

/// The readers on this network, named, and the one to use.
///
/// One reader is chosen without asking, because there is nothing to choose.
/// Several are listed by what they are rather than by address alone, because
/// "192.168.1.23" is not how anybody knows their own reader.
/// Saves the reader on this network under a name, so companion commands can
/// find it without being told its address.
///
/// Every app that is set up from a computer used to end its instructions with
/// `--device IP`, and nothing on the reader says what that is. After this,
/// `kobo frame push PHOTOS` goes to the reader saved here, and the reader is
/// found again by its serial when its address changes.
fn pair_command(arguments: &[String]) -> Result<(), String> {
    let (target, rest) = targets::TargetArgs::parse(arguments)?;
    let name = match rest.as_slice() {
        [] => "kobo".to_owned(),
        [name] if !name.starts_with('-') => name.clone(),
        _ => return Err(console::usage("usage: kobo pair [NAME] [--device IP]")),
    };
    let address = if target.is_empty() {
        match choose_reader() {
            ReaderSearch::Found(address) => address,
            other => {
                return Err(format!(
                    "{}\nName the one you mean: kobo pair {name} --device IP",
                    other.explain()
                ))
            }
        }
    } else {
        match target.resolve()? {
            targets::Target::Address(address) => address,
            targets::Target::Nickname(nickname) => targets::resolve_nickname(&nickname)?,
            targets::Target::Simulator => {
                return Err(console::usage("the simulator needs no pairing; use --sim"))
            }
        }
    };
    let identity = identify_device(&address)
        .filter(connect::Identity::is_kobo)
        .ok_or_else(|| format!("{address} did not answer as a Kobo"))?;
    let path = readers::store_path();
    let mut store = readers::Store::load(&path)?;
    store.name(&identity.serial, &address, &name)?;
    store.save(&path)?;
    println!(
        "Saved {} as \"{name}\" at {address}.\nCommands that act on a reader now use it, for example:\n  kobo frame push ~/Pictures/family",
        identity.summary()
    );
    Ok(())
}

fn choose_reader() -> ReaderSearch {
    let Some(subnet) = connect::local_subnet() else {
        return ReaderSearch::NoRoute;
    };
    println!("Looking for readers on {subnet}.1-254.");
    let mut readers = Vec::new();
    for address in connect::sweep(&subnet, connect::PROBE_TIMEOUT) {
        let host = address.to_string();
        if let Some(identity) = identify_device(&host) {
            if identity.is_kobo() {
                println!("  {host}  {}", identity.summary());
                readers.push(host);
            }
        }
    }
    match readers.as_slice() {
        [] => ReaderSearch::NoneAnswered,
        [only] => ReaderSearch::Found(only.clone()),
        several => ReaderSearch::Several(several.len()),
    }
}

/// The stream authority this computer minted, wherever it keeps it.
fn stream_authority() -> Result<PathBuf, String> {
    let root = if let Some(value) = std::env::var_os("KOBO_STREAM_CONFIG_DIR") {
        PathBuf::from(value)
    } else {
        let home = std::env::var_os("HOME").ok_or("no HOME in the environment")?;
        PathBuf::from(home).join(".config").join("kobo")
    };
    stream_authority_in(&root)
}

/// The same, under a named root, which is the half a test can ask about.
fn stream_authority_in(root: &Path) -> Result<PathBuf, String> {
    let authority = root.join("stream").join("ca-cert.pem");
    if !authority.is_file() {
        return Err(format!(
            "{} is not there, so there is no trust root to install",
            authority.display()
        ));
    }
    Ok(authority)
}

fn stream_companion(arguments: &[String]) -> Result<(), String> {
    let port = match &arguments[1..] {
        [] => kobo_stream::DEFAULT_PORT,
        [flag, value] if flag == "--port" => value
            .parse::<u16>()
            .ok()
            .filter(|port| *port > 0)
            .ok_or("--port must be 1 through 65535")?,
        _ => {
            return Err(
                "usage: kobo stream demo|terminal|monitor|pairing [--port PORT]".to_owned(),
            );
        }
    };
    let pairing = kobo_stream::pairing_instructions(port)?;
    if arguments[0] == "pairing" {
        println!("{pairing}");
        return Ok(());
    }
    eprintln!("{pairing}\n");
    let (command, title) = stream_preset(&arguments[0], std::env::var("SHELL").ok().as_deref())?;
    eprintln!(
        "Open Paperterm on your reader and connect to this computer. Keep the computer awake."
    );
    if arguments[0] == "demo" {
        eprintln!("This check echoes text; it does not run commands. Type exit to finish.");
    }
    kobo_stream::run_with_title(
        kobo_stream::Options {
            grid: kobo_stream::Grid::fallback(),
            controls: true,
            interactive: true,
            port,
            command,
        },
        title,
    )
    .map(|_| ())
}

fn stream_preset(name: &str, shell: Option<&str>) -> Result<(Vec<String>, &'static str), String> {
    match name {
        "demo" => {
            let executable = std::env::current_exe().map_err(|e| format!("find this CLI: {e}"))?;
            let executable = executable.to_str().ok_or("CLI path must be UTF-8")?;
            Ok((
                vec![
                    executable.into(),
                    "stream".into(),
                    "__connection-check".into(),
                ],
                "Connection check",
            ))
        }
        "terminal" => {
            let shell = shell.unwrap_or("/bin/sh");
            if !Path::new(shell).is_absolute() || !Path::new(shell).is_file() {
                return Err("Your default shell is unavailable. Set SHELL to an installed shell's absolute path, or use the connection check: kobo stream demo".into());
            }
            Ok((vec![shell.into(), "-l".into()], "Terminal"))
        }
        "monitor" => Ok((vec!["top".into()], "System monitor")),
        _ => Err(
            "Choose demo, terminal or monitor. Use kobo stream --help for custom commands.".into(),
        ),
    }
}

const STREAM_START: &str = "Paperterm shares a computer terminal with your reader.

Start with a connection check:
  kobo stream demo

Then choose a session:
  kobo stream terminal    Open your usual shell
  kobo stream monitor     Watch this computer's processes with top

First use: run kobo stream init. It finds this computer's address, finds the
readers on this network, and installs the trust root on the one you choose;
name a reader outright with --device IP. Open Paperterm on the reader and
enter the computer address and pairing code it prints.

Keep the computer awake. Press Ctrl+] on the computer to stop sharing.
For custom commands and other advanced options: kobo stream --help";

fn stream_command(arguments: &[String]) -> Result<(), String> {
    const USAGE: &str = "usage: kobo stream init [--device IP] [--reader NAME] [--host ADDRESS ...]\n\
                         \x20      kobo stream demo [--port PORT]\n\
                         \x20      kobo stream terminal|monitor [--port PORT]\n\
                         \x20      kobo stream pairing [--port PORT]\n\
                         \x20      kobo stream [--grid COLSxROWS] [--controls | --interactive] \
                         [--read-only] [--port PORT] -- COMMAND [ARG ...]\n\
                         Host-only. The reader never opens a shell; it paints rows this command serves.";
    if arguments.is_empty() {
        println!("{STREAM_START}");
        return Ok(());
    }
    if wants_help(arguments) {
        return print_command_help(USAGE);
    }
    if arguments == ["__connection-check"] {
        return stream_demo::run();
    }
    if arguments.first().is_some_and(|argument| argument == "init") {
        return stream_init(&arguments[1..]);
    }
    if arguments.first().is_some_and(|argument| {
        matches!(
            argument.as_str(),
            "demo" | "terminal" | "monitor" | "pairing"
        )
    }) {
        return stream_companion(arguments);
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

fn verification_bytes(path: &Path) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))
}

fn verification_key(path: &Path) -> Result<kobo_app_store::Ed25519PublicKey, String> {
    let text = String::from_utf8(verification_bytes(path)?)
        .map_err(|_| "Public key must be UTF-8 hexadecimal")?;
    kobo_app_store::Ed25519PublicKey::from_hex(text.trim()).map_err(|error| error.to_string())
}

fn app_verify(arguments: &[String]) -> Result<(), String> {
    const USAGE: &str =
        "usage: kobo app-verify --package PATH --public-key PATH --manifest PATH --binary PATH";
    ensure_only_flags(
        arguments,
        &["--package", "--public-key", "--manifest", "--binary"],
        USAGE,
    )?;
    let package = verification_bytes(&single_path_flag(arguments, "--package", USAGE)?)?;
    let key = verification_key(&single_path_flag(arguments, "--public-key", USAGE)?)?;
    let manifest = verification_bytes(&single_path_flag(arguments, "--manifest", USAGE)?)?;
    let binary = verification_bytes(&single_path_flag(arguments, "--binary", USAGE)?)?;
    let parsed = kobo_app_store::parse_public_bundle(&package, &key)
        .map_err(|e| format!("Package verification failed: {e}"))?;
    let expected = kobo_app_store::Manifest::parse_public(&manifest)
        .map_err(|e| format!("Invalid expected manifest: {e}"))?;
    if parsed.manifest().to_canonical_bytes() != expected.to_canonical_bytes()
        || parsed.binary() != binary
    {
        return Err("Verified package differs from the supplied manifest or binary".into());
    }
    verify_arm_elf_bytes(&binary, true)?;
    println!("Verified package signature, manifest and binary.");
    Ok(())
}

fn app_catalog_verify(arguments: &[String]) -> Result<(), String> {
    const USAGE: &str = "usage: kobo app-catalog-verify --catalog PATH --signature PATH --public-key PATH --package PATH";
    ensure_only_flags(
        arguments,
        &["--catalog", "--signature", "--public-key", "--package"],
        USAGE,
    )?;
    let catalog = verification_bytes(&single_path_flag(arguments, "--catalog", USAGE)?)?;
    let signature = verification_bytes(&single_path_flag(arguments, "--signature", USAGE)?)?;
    let signature =
        std::str::from_utf8(&signature).map_err(|_| "Signature must be UTF-8 hexadecimal")?;
    let signature =
        kobo_app_store::DetachedSignature::from_hex(signature.trim()).map_err(|e| e.to_string())?;
    let key = verification_key(&single_path_flag(arguments, "--public-key", USAGE)?)?;
    kobo_app_store::verify(&catalog, &signature, &key)
        .map_err(|e| format!("Catalog signature verification failed: {e}"))?;
    let catalog = kobo_app_store::Catalog::parse_public(&catalog).map_err(|e| e.to_string())?;
    let package = verification_bytes(&single_path_flag(arguments, "--package", USAGE)?)?;
    let parsed = kobo_app_store::parse_public_bundle(&package, &key).map_err(|e| e.to_string())?;
    let entry = catalog
        .entries()
        .iter()
        .find(|entry| entry.manifest().id() == parsed.manifest().id())
        .ok_or("Verified catalog does not contain the supplied package")?;
    if entry.manifest().to_canonical_bytes() != parsed.manifest().to_canonical_bytes()
        || entry.package_bytes() != package.len() as u64
        || entry.package_sha256().as_str() != kobo_net::sha256::hex_digest(&package)
    {
        return Err("Catalog entry differs from the supplied package".into());
    }
    println!("Verified catalog signature and package entry.");
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
    // These are website-only registry fields. The page generator validates
    // them; the CLI ignores them and release manifests contain neither.
    let fields = strict_registry_object(
        value,
        "app",
        &FIELDS,
        &["page_description", "setup", "release_notes"],
    )?;
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
    if arguments.first().is_some_and(|arg| arg == "--runtime") {
        return runtime_dev::run(&arguments[1..]);
    }
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
    let server = match fs::File::open("cobalt-app.json") {
        Ok(file) => {
            let mut source = String::new();
            file.take(64 * 1024 + 1)
                .read_to_string(&mut source)
                .map_err(|error| format!("read app manifest: {error}"))?;
            server
                .with_manifest(&source)
                .map_err(|error| error.to_string())?
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => server,
        Err(error) => return Err(format!("read app manifest: {error}")),
    };
    server
        .set_nonblocking(true)
        .map_err(|error| format!("configure app simulator: {error}"))?;
    let executable = build_dev_app()?;
    let server = server
        .with_capture_source(dev_capture_source(&executable)?)
        .map_err(|error| error.to_string())?;
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

fn dev_capture_source(executable: &Path) -> Result<kobo_sim::CaptureSource, String> {
    let git = |arguments: &[&str]| {
        Command::new("git")
            .args(arguments)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok())
    };
    let revision = git(&["rev-parse", "HEAD"]).map(|value| value.trim().to_owned());
    let dirty = git(&["status", "--porcelain"]).map(|value| !value.is_empty());
    let mut bytes = Vec::new();
    fs::File::open(executable)
        .and_then(|file| file.take(256 * 1024 * 1024 + 1).read_to_end(&mut bytes))
        .map_err(|error| format!("read app build for capture provenance: {error}"))?;
    let binary_sha256 =
        (bytes.len() <= 256 * 1024 * 1024).then(|| kobo_net::sha256::hex_digest(&bytes));
    let fixture = std::env::var("KOBO_SIM_FIXTURE").ok();
    let seed = std::env::var("KOBO_SIM_SEED")
        .ok()
        .map(|value| {
            value
                .parse()
                .map_err(|_| "KOBO_SIM_SEED must be an unsigned integer")
        })
        .transpose()?;
    Ok(kobo_sim::CaptureSource {
        revision,
        dirty,
        binary_sha256,
        fixture,
        seed,
    })
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
            .env("KOBO_SIM_CALLBACKS", "1")
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
            let binary = workspace_target_directory()
                .join("armv7-unknown-linux-musleabihf/release")
                .join(name);
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
    let (host, json) = parse_doctor(arguments)?;
    if let Some(host) = host {
        if json {
            let artifact = RemoteArtifact {
                program: RemoteProgram::DoctorJson,
                ..RemoteArtifact::doctor()
            };
            return run_remote_fixed_artifact(&host, &artifact);
        }
        return remote_doctor(&host);
    }
    let binary = sibling_binary("kobo-doctor");
    let mut command = Command::new(&binary);
    if json {
        command.env("KOBO_DOCTOR_JSON", "1");
    }
    run_status(&mut command, format!("{}", binary.display()))
}

fn parse_doctor(arguments: &[String]) -> Result<(Option<String>, bool), String> {
    let usage = "usage: kobo doctor [--device HOST | --reader NAME] [--json]";
    let (flags, rest) = targets::TargetArgs::parse(arguments)?;
    let mut host = None;
    let mut json = false;
    if !flags.is_empty() {
        host = Some(match flags.resolve() {
            Ok(targets::Target::Address(address)) => address,
            Ok(targets::Target::Nickname(name)) => targets::resolve_nickname(&name)?,
            Ok(targets::Target::Simulator) => {
                return Err(
                    "usage: the simulator has no hardware to diagnose; doctor is for a reader"
                        .to_owned(),
                );
            }
            Err(_) => return Err(usage.into()),
        });
    }
    for arg in &rest {
        if arg == "--json" && !json {
            json = true;
        } else {
            return Err(usage.into());
        }
    }
    if let Some(host) = &host {
        if !valid_device_host(host) {
            return Err("device host contains unsupported characters".into());
        }
    }
    Ok((host, json))
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
    let (subnet, json) = parse_devices(arguments)?;
    console::Console::progress(&format!(
        "scanning {subnet}.1-254 on port {} for readers",
        connect::SSH_PORT
    ));
    let answered = connect::sweep(&subnet, connect::PROBE_TIMEOUT);
    let mut readers = Vec::new();
    let mut others = 0_usize;
    for address in &answered {
        match identify_device(&address.to_string()) {
            Some(identity) if identity.is_kobo() => {
                readers.push((*address, identity.summary()));
            }

            _ => others += 1,
        }
    }
    if json {
        console::Console::print_json(
            "devices",
            &serde_json::json!({
                "subnet": format!("{subnet}.0/24"),
                "readers": readers
                    .iter()
                    .map(|(address, summary)| serde_json::json!({
                        "address": address.to_string(),
                        "summary": summary,
                    }))
                    .collect::<Vec<_>>(),
                "other_hosts": others,
            }),
        );
        if readers.is_empty() {
            return Err(unreachable_device(format!(
                "no reader answered on {subnet}.0/24"
            )));
        }
        return Ok(());
    }
    for (address, summary) in &readers {
        println!("{address}  {summary}");
    }
    if others > 0 {
        println!(
            "{others} other host(s) answered on port {}",
            connect::SSH_PORT
        );
    }
    let Some((first, _)) = readers.first() else {
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

fn parse_devices(arguments: &[String]) -> Result<(String, bool), String> {
    const USAGE: &str = "usage: kobo devices [--subnet A.B.C] [--json]";
    let mut subnet = None;
    let mut json = false;
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        if argument == "--json" && !json {
            json = true;
        } else if argument == "--subnet" && subnet.is_none() {
            subnet = Some(arguments.next().ok_or(USAGE)?.clone());
        } else {
            return Err(USAGE.to_owned());
        }
    }
    let subnet = match subnet {
        Some(value) => value,
        None => connect::local_subnet().ok_or(
            "this machine has no route to a network, so there is nothing to scan; \
             connect to the same Wi-Fi as the reader, or pass --subnet A.B.C",
        )?,
    };
    if !connect::valid_subnet(&subnet) {
        return Err(format!(
            "--subnet takes the first three octets and nothing else, such as 192.168.1, \
             not {subnet:?}"
        ));
    }
    Ok((subnet, json))
}

/// Controls how long a connected device stays reachable while developing.
///
/// These controls change bounded kernel wake leases or the reader's settings;
/// they do not rewrite partitions, firmware or books.
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
        if matches!(action, DevSessionAction::KeepAwake(devsession::Switch::On)) {
            println!(
                "The reader can stay awake for two minutes. Use kobo session --device ADDRESS --hold MINUTES for a longer timed session."
            );
        }
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
/// two-minute kernel lease also expires if the computer disconnects or exits.
/// A device that disappears mid-hold is waited for rather than treated
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
    // The last lease expires even if this best-effort release cannot connect.
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
        println!(
            "The last wake lease expires within {} seconds. No reboot is needed.",
            devsession::WAKE_LEASE_SECONDS
        );
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
    const USAGE: &str = "usage: kobo shell --device <host> [command ...]\n\
                         \x20      An advanced control: the command runs on the reader exactly as\n\
                         \x20      typed, as root, with no validation. Prefer a named verb when one\n\
                         \x20      exists - logs, shot, record and doctor cover the common reads.";
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

fn parse_wait(arguments: &[String]) -> Result<(String, Duration), String> {
    const USAGE: &str =
        "usage: kobo wait (--device <host> | --reader <name>) [--timeout <seconds>]";
    let (flags, rest) = targets::TargetArgs::parse(arguments)?;
    let host = match flags.resolve() {
        Ok(targets::Target::Address(host)) => host,
        Ok(targets::Target::Nickname(name)) => targets::resolve_nickname(&name)?,
        Ok(targets::Target::Simulator) => {
            return Err(
                "usage: the simulator is already here; wait is for a reader on the network"
                    .to_owned(),
            );
        }
        Err(_) => return Err(USAGE.to_owned()),
    };
    if !valid_device_host(&host) {
        return Err("device host contains unsupported characters".to_owned());
    }
    let seconds = match rest.as_slice() {
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
    DoctorJson,
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
            RemoteProgram::Doctor | RemoteProgram::DoctorJson | RemoteProgram::Capture => {
                REMOTE_COMMAND_TIMEOUT
            }
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
        && !host.starts_with('-')
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
        RemoteProgram::DoctorJson => "KOBO_DOCTOR_JSON=1 \"$bin\"".to_owned(),
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
        | RemoteProgram::DoctorJson
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
    console::target(error)
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
    // First on purpose. A pre-bootstrap updater such as f49b32c refuses this
    // exact outside-tree member while unpacking and therefore fails before it
    // can retire `cobalt`. A bootstrap-aware updater validates and skips the
    // independently installed copy before its rename transaction.
    let mut members = vec![package::Member {
        path: package::LAUNCH_BOOTSTRAP.to_owned(),
        bytes: bootstrap::CONTENT.as_bytes().to_vec(),
        program: true,
    }];
    for (name, features) in INSTALLED_PACKAGES {
        run_status(
            &mut device_build_command(name, *features)?,
            format!("cargo build {name}"),
        )?;
        let binary = workspace_target_directory()
            .join("armv7-unknown-linux-musleabihf/release")
            .join(name);
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
    // The Syncthing engine is deliberately not a member of this package.
    //
    // It is 27.9 MB, which was 44% of the release and took the archive from
    // 15.0 MB to 31.0 MB compressed and 26.9 MB to 63.0 MB expanded. Every
    // reader pays that on every update, over Wi-Fi, and `update::install`
    // holds both the archive and the expanded tree in memory at once on a
    // device with half a gigabyte of it -- so the cost of one optional
    // feature is charged to everybody who never enables it, at the moment
    // they are least able to afford it.
    //
    // The engine is fetched on first use instead, from a tag that does not
    // move, and checked against the same digest the runtime already enforces
    // before it will execute it. The source record stays here: a reader that
    // runs Syncthing is owed the notice whether the bytes arrived in this
    // archive or afterwards.
    members.push(text_member(
        "licenses/SYNCTHING.md",
        SYNCTHING_SOURCE_RECORD,
        false,
    ));
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
    // The serif every application falls back to when the reader has none of
    // its own. Like the two faces above it is compiled into binaries this
    // package installs, and the Open Font License travels with every copy.
    members.push(text_member(
        "licenses/LICENSE-Literata.txt",
        include_str!("../../kobo-text/fonts/LICENSE-Literata.txt"),
        false,
    ));
    members.push(text_member(
        "licenses/SOURCE-Literata.md",
        include_str!("../../../licenses/SOURCE-Literata.md"),
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
    let (readback_members, listed) = validated_release_archive(&archive)?;
    if readback_members != members {
        return Err("refusing to build: archive readback differs from its inputs".to_owned());
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
    /// Whether a short original welcome note joins the install, so the first
    /// reconnect shows something new to open. `--no-sample` skips it.
    sample: bool,
}

/// What `kobo setup --help` says. It used to answer "unknown option" to the
/// one flag everybody tries first on a command they have not run before.
const SETUP_HELP: &str = "usage: kobo setup [options]

Prepares a reader connected over USB. Run it with no options and it finds the
reader, says what it will change, and asks before changing anything.

  --enable-ssh       Also turn on SSH, which the companion commands use
  --no-key           Do not install this computer's SSH key
  --volume PATH      The mounted reader, when more than one is connected
  --undo             Remove what setup installed
  --dry-run          Say what would change and change nothing
  --yes              Answer yes to the confirmation
  --non-interactive  Never ask; fail where a question would be needed
  --wait-for-reader  Wait for a reader to be plugged in
  --no-eject         Leave the reader mounted afterwards
  --no-wait          Do not wait for the reader to come back
  --menu, --no-menu  Add or skip the launcher's menu entry
  --no-sample        Skip the welcome note
  --release-dir PATH | --source  Install a verified release, or a source build

Next: kobo pair, so the companion commands find this reader on Wi-Fi.";

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
        sample: true,
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
            "--no-sample" => options.sample = false,
            "--dry-run" => options.dry_run = true,
            other => {
                return Err(format!(
                    "unknown option '{other}'\n\
                     usage: kobo setup [--volume PATH] [--undo] [--enable-ssh] [--no-key] \
                     [--no-eject] [--no-wait] [--menu] [--no-menu] [--dry-run] [--yes] \
                     [--non-interactive] [--wait-for-reader] [--release-dir PATH] [--source] \
                     [--no-sample]"
                ));
            }
        }
        index += 1;
    }
    if options.source && options.release_dir.is_some() {
        return Err("--source and --release-dir are mutually exclusive".to_owned());
    }
    if options.non_interactive && !options.yes && !options.dry_run {
        return Err(crate::console::usage(
            "--non-interactive requires --yes for any change",
        ));
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
    let (members, listed) = validated_release_archive(&archive)
        .map_err(|error| format!("refusing prebuilt package: {error}"))?;
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
        return Err(crate::console::usage(
            "noninteractive setup was not explicitly confirmed with --yes",
        ));
    }
    let tty = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .map_err(|error| {
            format!(
                "open /dev/tty for confirmation: {error}; pass --yes only after reviewing --dry-run"
            )
        })?;
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
    // Only a first argument of --help, because a bare setup is the guided
    // run and must not print usage instead of starting.
    if matches!(arguments.first().map(String::as_str), Some("--help" | "-h")) {
        return print_command_help(SETUP_HELP);
    }
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
    let profile = if options.mode == SetupMode::Undo {
        setup::undo_profile(&reader)?
    } else {
        setup::install_profile(&reader)?
    };
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
    // Before the eject, because the note is a write to the volume. It never
    // fails the install: a set-up reader without the note is still set up.
    let sample = options.sample.then(|| setup::write_sample(&reader.volume));
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
    // The step is recorded only here, past the eject: a dry run, a decline
    // or a failed verify completes nothing.
    if let Err(error) = (|| {
        let path = steps::steps_path();
        let mut done = steps::Steps::load(&path)?;
        done.complete(steps::SETUP, &reader.serial, steps::now());
        done.save(&path)
    })() {
        println!("note: the completed setup could not be remembered ({error}); the install itself is fine");
    }
    match &sample {
        Some(Ok(path)) => println!(
            "sample: {} is in the library; open it on the reader after the restart, and delete it whenever you like",
            path.display()
        ),
        Some(Err(error)) => println!(
            "sample: the welcome note was not written ({error}); the install itself is fine"
        ),
        None => {}
    }
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
            "would remove managed Cobalt trees under {}/{} after moving any owner folders\n\
             \x20 into a free .adds/cobalt.recovery.N directory\n\
             would remove {}/{} and the exact Cobalt line from .adds/nm/cobalt and .adds/nm/menu\n\
             would leave any .adds/cobalt.unusable[.N] quarantine for explicit inspection\n\
             would disable the firmware's SSH server by renaming {} back\n\
             would clear {}\n\
             would remove {} and ask NickelMenu to uninstall itself, unless another\n\
             \x20 mod still has a configuration file beside it\n\
             nothing else on the reader is touched",
            reader.volume.display(),
            setup::INSTALL_FOLDER,
            reader.volume.display(),
            bootstrap::RELATIVE_PATH,
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
        describe_sample_plan(options),
        if options.enable_ssh && options.wait {
            "wait for the restarted reader to appear on the network"
        } else if !options.enable_ssh {
            "stop after ejecting, because SSH was not enabled"
        } else {
            "stop, because --no-wait was given"
        },
        match (would_stage, options.enable_ssh && options.authorize_key,) {
            (true, true) =>
                ", and nothing extracted as root but NickelMenu's own two files and one authorized_keys",
            (true, false) => ", and nothing extracted as root but NickelMenu's own two files",
            (false, true) => ", and nothing extracted as root but one authorized_keys",
            (false, false) => ", nothing extracted as root",
        }
    )
}

/// The one line of the dry run that covers the welcome note.
fn describe_sample_plan(options: &SetupOptions) -> String {
    if options.sample {
        format!(
            "would add {} to the library, a short welcome note to open after the restart",
            setup::SAMPLE_NAME
        )
    } else {
        "would add no welcome note, because --no-sample was given".to_owned()
    }
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
    let settings = setup::revert_settings(&reader.volume)?;
    let removal = setup::remove_payload(&reader.volume)?;
    let ssh = setup::disable_ssh(&reader.volume)?;
    let unmenued = menu::remove(&reader.volume)?;
    let ejected = ejected_or_explained(&reader.volume, eject);

    println!(
        "\nUndone on {}:\n  · {}\n  · {}\n  · {}\n  · {}\n  · {}",
        reader.volume.display(),
        if removal.removed {
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
            format!("settings restored: {}", settings.join(", "))
        },
        describe_unmenu(unmenued),
        if ejected {
            "volume ejected"
        } else {
            "volume left mounted"
        }
    );
    if !removal.recoveries.is_empty() {
        println!(
            "\nOwner data was preserved for manual backup or deletion in:\n  {}",
            removal.recoveries.join("\n  ")
        );
    }
    if !removal.quarantines.is_empty() {
        println!(
            "\nUnusable managed trees were not deleted because they may contain recoverable\n\
             owner data. Inspect them before removing them:\n  {}",
            removal.quarantines.join("\n  ")
        );
    }
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
        package::write_volume_layout(&built.members, &folder)?;
        println!(
            "also written as a volume-relative folder: {}\n\
             copy the .adds directory inside it to the root of the mounted Kobo volume;\n\
             it contains both .adds/cobalt and .adds/cobalt-launch.sh",
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
        deployment_package(&built.members)?
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
    let (members, _) = validated_release_archive(&archive)
        .map_err(|error| format!("refusing to upload {}: {error}", path.display()))?;
    deployment_package(&members)
}

fn deployment_package(members: &[package::Member]) -> Result<(Vec<u8>, usize), String> {
    let deploy_members: Vec<_> = members
        .iter()
        .filter(|member| !package::is_launch_bootstrap(member))
        .cloned()
        .collect();
    let deploy_archive = package::tar(&deploy_members)?;
    let compressed = gzip(&deploy_archive)?;
    Ok((compressed, deploy_members.len()))
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
struct ReviewedLaunchBootstrap;

fn members_outside_install_root(
    listed: &[package::Listed],
    _reviewed: &ReviewedLaunchBootstrap,
) -> Vec<String> {
    let root = Path::new(package::INSTALL_ROOT);
    listed
        .iter()
        .enumerate()
        .filter(|(index, entry)| {
            let path = Path::new(entry.path.trim_end_matches('/'));
            let reviewed_bootstrap = *index == 0
                && entry.path == package::LAUNCH_BOOTSTRAP
                && entry.kind == b'0'
                && entry.mode == 0o755
                && entry.size == bootstrap::CONTENT.len();
            !(path.starts_with(root) || root.starts_with(path) || reviewed_bootstrap)
        })
        .map(|(_, entry)| entry.path.clone())
        .collect()
}

fn validated_release_archive(
    archive: &[u8],
) -> Result<(Vec<package::Member>, Vec<package::Listed>), String> {
    let listed = package::list(archive)?;
    let first = listed
        .first()
        .ok_or("release archive has no standalone launch bootstrap")?;
    if first.path != package::LAUNCH_BOOTSTRAP
        || first.kind != b'0'
        || first.mode != 0o755
        || first.size != bootstrap::CONTENT.len()
    {
        return Err(
            "standalone launch bootstrap must be the first regular 0755 archive member".to_owned(),
        );
    }
    if listed
        .iter()
        .filter(|entry| entry.path == package::LAUNCH_BOOTSTRAP)
        .count()
        != 1
    {
        return Err("standalone launch bootstrap must appear exactly once".to_owned());
    }
    let members = package::members(archive)?;
    let bootstrap_member = members
        .first()
        .ok_or("release archive has no standalone launch bootstrap")?;
    if !package::is_launch_bootstrap(bootstrap_member)
        || !bootstrap_member.program
        || bootstrap_member.bytes != bootstrap::CONTENT.as_bytes()
    {
        return Err("standalone launch bootstrap differs from the reviewed executable".to_owned());
    }
    let reviewed = ReviewedLaunchBootstrap;
    let outside = members_outside_install_root(&listed, &reviewed);
    if !outside.is_empty() {
        return Err(format!(
            "{} would be written outside {}",
            outside.join(", "),
            package::INSTALL_ROOT
        ));
    }
    Ok((members, listed))
}

/// Lists a package and proves it cannot write outside the install root.
fn inspect_package(arguments: &[String]) -> Result<(), String> {
    let path = arguments.first().ok_or("usage: kobo inspect <package>")?;
    let compressed = fs::read(path).map_err(|error| format!("read {path}: {error}"))?;
    gzip_test(&compressed)?;
    let archive = gunzip(&compressed)?;
    let (_, listed) = validated_release_archive(&archive)?;
    for entry in &listed {
        let kind = if entry.kind == b'5' { "dir " } else { "file" };
        println!("{kind} {:o} {:>9} {}", entry.mode, entry.size, entry.path);
    }
    println!(
        "only the reviewed {} bootstrap is outside {}; this package writes no root filesystem file",
        package::LAUNCH_BOOTSTRAP,
        package::INSTALL_ROOT
    );
    Ok(())
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
    let target = workspace_target_directory();
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
        .env("KOBO_SIM_CALLBACKS", "1")
        .status()
        .map_err(|error| format!("run {package}: {error}"))?;
    let daemon_status = simulation.daemon_wait()?;
    if !app_status.success() || !daemon_status.success() {
        return Err(format!(
            "simulation failed: app={app_status}, daemon={daemon_status}"
        ));
    }
    let profile = kobo_sim::selected_profile();
    let expected = profile.width as usize * profile.height as usize;
    let actual = fs::metadata(&simulation.frame)
        .map_err(|error| format!("inspect rendered frame: {error}"))?
        .len();
    if actual != expected as u64 {
        return Err(format!(
            "rendered frame is {actual} bytes; expected {expected}"
        ));
    }
    let output = target.join("kobo-sim-last.raw");
    fs::copy(&simulation.frame, &output)
        .map_err(|error| format!("save rendered frame: {error}"))?;
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
                ));
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
    verify_arm_elf_bytes(&bytes, true).map_err(|error| format!("{}: {error}", path.display()))
}

fn verify_arm_elf_bytes(bytes: &[u8], require_hard_float: bool) -> Result<(), String> {
    if bytes.len() < 52 || &bytes[..4] != b"\x7fELF" {
        return Err("not an ELF binary".to_owned());
    }
    if bytes[4] != 1 || bytes[5] != 1 {
        return Err("expected a little-endian ELF32 binary".to_owned());
    }
    if read_u16(bytes, 16)? != 2 {
        return Err("expected an executable ELF file".to_owned());
    }
    if read_u16(bytes, 18)? != 40 {
        return Err("expected an ARM ELF binary".to_owned());
    }
    let flags = read_u32(bytes, 36)?;
    if require_hard_float && (flags & 0x400 == 0 || flags & 0x200 != 0) {
        return Err(format!(
            "expected ARM hard-float ABI flags, found 0x{flags:08x}"
        ));
    }
    let program_offset =
        usize::try_from(read_u32(bytes, 28)?).map_err(|_| "program offset overflow")?;
    let entry_size = usize::from(read_u16(bytes, 42)?);
    let entry_count = usize::from(read_u16(bytes, 44)?);
    if entry_size < 32 {
        return Err("invalid ELF program header size".to_owned());
    }
    let entry = read_u32(bytes, 24)?;
    let mut executable_entry = false;
    for index in 0..entry_count {
        let offset = program_offset
            .checked_add(
                index
                    .checked_mul(entry_size)
                    .ok_or("program header overflow")?,
            )
            .ok_or("program header overflow")?;
        let kind = read_u32(bytes, offset)?;
        if kind == 2 || kind == 3 {
            return Err("binary contains a dynamic or interpreter program header".to_owned());
        }
        if kind == 1 {
            let file_offset = usize::try_from(read_u32(bytes, offset + 4)?)
                .map_err(|_| "load segment offset overflow")?;
            let virtual_address = read_u32(bytes, offset + 8)?;
            let file_size = usize::try_from(read_u32(bytes, offset + 16)?)
                .map_err(|_| "load segment size overflow")?;
            let memory_size = read_u32(bytes, offset + 20)?;
            let segment_flags = read_u32(bytes, offset + 24)?;
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
        .map(|directory| {
            drive::Recorder::start(&address, fps, ghosting)
                .map(|recorder| recorder.with_metadata(directory))
        })
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
    let mut capture_metadata = None;
    let (width, height, grey) = if let Some(host) = host {
        let transcript = capture_remote_fixed_artifact(&host, &RemoteArtifact::capture())?;
        drive::decode_capture(&transcript)?
    } else {
        let driver = drive::Driver::new(&address, Path::new(".")).ideal(ideal);
        let capture = driver.capture()?;
        capture_metadata = Some(capture.metadata);
        (capture.width, capture.height, capture.grey)
    };
    let png = kobo_image::encode_png_grey(width, height, &grey)
        .map_err(|error| format!("encode the panel: {error}"))?;
    fs::write(&output, png).map_err(|error| format!("write {}: {error}", output.display()))?;
    if let Some(metadata) = capture_metadata {
        let sidecar = output.with_extension("json");
        fs::write(
            &sidecar,
            serde_json::to_vec_pretty(&metadata).map_err(|error| error.to_string())?,
        )
        .map_err(|error| format!("write {}: {error}", sidecar.display()))?;
    }
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
    record_command_notifying(arguments, None)
}

/// Runs `kobo record`, optionally notifying a coordinator the instant the
/// device has finished writing frames and before the slower pull/encode work.
fn record_command_notifying(
    arguments: &[String],
    capture_barrier: Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>,
) -> Result<(), String> {
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
    if let Some((complete, resume)) = capture_barrier {
        let _ignored = complete.send(());
        resume
            .recv_timeout(Duration::from_secs(120))
            .map_err(|_| "recording coordinator did not release the transfer".to_owned())?;
    }

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
                           steps: tap LABEL | tap-id ACTION | tap-at X,Y | type TEXT | shot NAME | expect TEXT\n\
                           \u{20}       expect-missing TEXT | wait-for TEXT | wait-for-id ACTION | clean | dump\n\
                           \u{20}       lifecycle foreground|background | scenario NAME | wait MS | wait-idle [MS]\n\
                           expect-state ENDPOINT#JSON_POINTER JSON_VALUE checks typed state.\n\
                           tap-id and wait-for-id accept numeric IDs or stable action names.\n\
                           Transition steps also check for serious layout diagnostics.\n\
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
                            \x20      kobo secret set <name> --app APP --server HTTPS_URL (--from PATH | --stdin) --device IP\n\
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
    let metadata = fs::metadata(path)
        .map_err(|error| format!("Read credential file {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err("Choose a regular file containing the credential.".to_owned());
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .and_then(|file| {
            file.take((SECRET_MAXIMUM_BYTES + 1) as u64)
                .read_to_end(&mut bytes)
        })
        .map_err(|error| format!("Read credential file {}: {error}", path.display()))?;
    if bytes.len() > SECRET_MAXIMUM_BYTES {
        return Err(format!(
            "{} is too large; choose a credential file of at most {SECRET_MAXIMUM_BYTES} bytes",
            path.display()
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
                select_secret_target(&mut target, SecretTarget::Device(host))?;
                index += 2;
            }
            "--volume" => {
                select_secret_target(&mut target, SecretTarget::Volume(PathBuf::from(value()?)))?;
                index += 2;
            }
            "--from" => {
                if verb != "set" || source.is_some() {
                    return Err("Use --from once, with set only.".to_owned());
                }
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

fn provider_help_requested(arguments: &[String]) -> bool {
    matches!(arguments, [help] if matches!(help.as_str(), "--help" | "-h" | "help"))
        || matches!(arguments, [verb, help]
            if matches!(verb.as_str(), "set" | "list" | "remove")
                && matches!(help.as_str(), "--help" | "-h"))
}

fn select_secret_target(
    target: &mut Option<SecretTarget>,
    next: SecretTarget,
) -> Result<(), String> {
    if target.is_some() {
        return Err("Choose one reader: use --device ADDRESS or --volume PATH once.".to_owned());
    }
    *target = Some(next);
    Ok(())
}

fn secret_install_script(name: &str, value: &str) -> String {
    format!(
        "set -e\numask 077\n\
         mkdir -p {DEVICE_SECRETS_DIRECTORY}\n\
         chmod 700 {DEVICE_SECRETS_DIRECTORY}\n\
         cd {DEVICE_SECRETS_DIRECTORY}\n\
         (set -C; : > .{name}.writing) || exit 1\n\
         trap 'rm -f .{name}.writing' EXIT\n\
         trap 'exit 1' HUP INT TERM\n\
         cat > .{name}.writing <<'{SECRET_DELIMITER}'\n\
         {value}\n\
         {SECRET_DELIMITER}\n\
         chmod 600 .{name}.writing\n\
         mv -f .{name}.writing {name}\n\
         trap - EXIT HUP INT TERM\n{}",
        forget_typed_copies_script(name)
    )
}

fn secret_remove_script(name: &str) -> String {
    format!(
        "set -e\nrm -f {DEVICE_SECRETS_DIRECTORY}/{name}\n{}",
        forget_typed_copies_script(name)
    )
}

/// What the reader prints for each copy typed on it that a command removed.
const TYPED_COPY_REMOVED: &str = "typed-copy-removed";

/// Deletes the copies of a secret that apps saved from the reader's keyboard.
///
/// The runtime prefers an app's own copy to the owner's file, so an app can be
/// connected without a computer. Left alone, that preference would make
/// setting or removing a secret from here change nothing for an app connected
/// that way: the owner replaces a rejected token and the app keeps sending the
/// old one. Server-bound accounts live a directory deeper and are never
/// matched. Each removal is announced so the owner is told what went.
fn forget_typed_copies_script(name: &str) -> String {
    format!(
        "for typed in {DEVICE_SECRETS_DIRECTORY}/apps/*/{name}; do\n\
         if [ -f \"$typed\" ]; then\n\
         rm -f \"$typed\"\n\
         echo {TYPED_COPY_REMOVED}\n\
         fi\n\
         done\n"
    )
}

/// The same as [`forget_typed_copies_script`], for a reader mounted over USB.
fn forget_typed_copies(secrets: &Path, name: &str) -> Result<usize, String> {
    let apps = secrets.join("apps");
    let entries = match fs::read_dir(&apps) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(format!("read {}: {error}", apps.display())),
    };
    let mut removed = 0;
    for entry in entries {
        let typed = entry
            .map_err(|error| format!("read {}: {error}", apps.display()))?
            .path()
            .join(name);
        if fs::symlink_metadata(&typed).is_ok_and(|metadata| metadata.is_file()) {
            fs::remove_file(&typed)
                .map_err(|error| format!("remove {}: {error}", typed.display()))?;
            removed += 1;
        }
    }
    Ok(removed)
}

fn typed_copies_removed(stdout: &[u8]) -> usize {
    String::from_utf8_lossy(stdout)
        .lines()
        .filter(|line| line.trim() == TYPED_COPY_REMOVED)
        .count()
}

fn publish_secret(path: &Path, value: &str) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("Invalid credential name")?;
    let partial = path.with_file_name(format!(".{name}.writing"));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&partial)
        .map_err(|error| format!("Prepare credential (previous value unchanged): {error}"))?;
    let result = writeln!(file, "{value}")
        .and_then(|()| file.sync_all())
        .and_then(|()| {
            drop(file);
            fs::rename(&partial, path)
        });
    if let Err(error) = result {
        let _ = fs::remove_file(&partial);
        return Err(format!(
            "Could not publish credential; previous value unchanged: {error}"
        ));
    }
    Ok(())
}

fn remove_secret(name: &str, target: &SecretTarget) -> Result<(), String> {
    match target {
        SecretTarget::Device(host) => {
            let script = secret_remove_script(name);
            let output = run_remote_shell(&format!("root@{host}"), &script, DEVICE_PROBE_TIMEOUT)?;
            if !output.status.success() {
                return Err(remote_shell_error(
                    format!("remove the '{name}' credential"),
                    &output.stdout,
                    &output.stderr,
                ));
            }
            println!("Removed '{name}' from {host}.");
            if typed_copies_removed(&output.stdout) > 0 {
                println!("Removed the value typed on the reader.");
            }
        }
        SecretTarget::Volume(volume) => {
            let directory = volume.join(".adds").join("cobalt").join("secrets");
            let path = directory.join(name);
            let typed = forget_typed_copies(&directory, name)?;
            match std::fs::remove_file(&path) {
                Ok(()) => println!("Removed {}.", path.display()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound && typed == 0 => {
                    println!("No '{name}' credential was installed.");
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("remove {}: {error}", path.display())),
            }
            if typed > 0 {
                println!("Removed the value typed on the reader.");
            }
        }
    }
    Ok(())
}

fn secret_command(arguments: &[String]) -> Result<(), String> {
    if server_secret::requested(arguments) {
        return server_secret::command(arguments);
    }
    if provider_help_requested(arguments) {
        println!("{SECRET_USAGE}");
        return Ok(());
    }
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
                    let script = secret_install_script(name, &value);
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
                    if typed_copies_removed(&output.stdout) > 0 {
                        println!("It replaces the value typed on the reader.");
                    }
                }
                SecretTarget::Volume(volume) => {
                    let directory = volume.join(".adds").join("cobalt").join("secrets");
                    std::fs::create_dir_all(&directory)
                        .map_err(|error| format!("create {}: {error}", directory.display()))?;
                    let path = directory.join(name);
                    publish_secret(&path, &value)?;
                    println!("Installed '{name}' ({bytes} bytes) at {}.", path.display());
                    if forget_typed_copies(&directory, name)? > 0 {
                        println!("It replaces the value typed on the reader.");
                    }
                }
            }
            println!(
                "An application reaches it by naming the secret '{name}'; the value is never sent to the application itself."
            );
            Ok(())
        }
        (SecretAction::Remove { name }, target) => remove_secret(name, target),
        (SecretAction::List, SecretTarget::Device(host)) => {
            // Names only. A command that can print a key is a command someone
            // will run over a shoulder or paste into a bug report.
            let script = format!("ls -1 {DEVICE_SECRETS_DIRECTORY} 2>/dev/null || true\n");
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
                    names.push(entry.file_name().to_string_lossy().into_owned());
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
        .filter(|name| valid_secret_name(name))
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
    if provider_help_requested(arguments) {
        println!("{TRUST_USAGE}");
        return Ok(());
    }
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
                select_secret_target(&mut target, SecretTarget::Device(host))?;
                index += 2;
            }
            "--volume" => {
                select_secret_target(&mut target, SecretTarget::Volume(PathBuf::from(value()?)))?;
                index += 2;
            }
            "--from" => {
                if verb != "set" || source.is_some() {
                    return Err("Use --from once, with set only.".to_owned());
                }
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

#[allow(
    clippy::too_many_lines,
    reason = "the flat command reference stays grep-friendly and one line per command"
)]
fn print_help() {
    let console = console::Console::detect();
    println!(
        "{}",
        console.wrap(
            "Kobo: set up your reader, send things to its apps, and build new ones\n\n\
         Usage: kobo <command>\n\n\
         Getting started:\n\
           setup [--enable-ssh]   Prepare a reader over USB (run with no options to be asked)\n\
           pair [NAME]            Save the reader on this network so the commands below find it\n\
           apps [search WORD | setup APP]  Find apps and read their setup steps\n\
           update [--channel stable|beta]  Update this kobo command\n\n\
         Send things to your apps (these use your paired reader; add --reader NAME,\n\
         --device IP or --sim to send somewhere else):\n\
           send FILE [--app APP]  Send a file to whichever app opens it\n\
           frame init | push PHOTOS [--fit crop|pad] | ls | rm ID   Photos for Frame\n\
           musicstand init | push SCORE.pdf | ls | rm ID          Scores for Music Stand\n\
           vault init | push NOTES_DIR | ingest DIR | ls | rm ID  Markdown notes for Vault\n\
           flashcards --help      Import Anki decks and stage them for Flashcards\n\
           fieldbook --help       Field packs in, eBird checklists out\n\
           panels push COMIC.cbz  A comic for Panels\n\
           needles push PATTERN.pdf  A knitting pattern you own, for Needles\n\
           nonograms push IMAGE --name NAME --size 5|7|9  A photo puzzle for Nonograms\n\
           parser push STORY [--replace]  An interactive story for Parser\n\
           feeds check|push FILE.opml  Subscriptions for Feeds\n\
           deck set PAD ... | ls | show | push  Lay out Deck's pads, then send them\n\
           birds listen --source URL | status | stop  Mirror BirdNET-Go to Birds\n\
           post login --gateway URL --token-file PATH  Sign Post in to a Hermes gateway\n\
           readlater login --server URL ...  Sign Read Later in to Wallabag\n\
           sync setup DIR --folder NAME | run | status | stop  Keep a folder in sync\n\
           export --app APP --out DIR  Receive a copy an app prepared for you\n\
           sidekick init | setup | run | status | stop | sample | test  The Sidekick helper\n\
           stream init | stream -- COMMAND  Pair Paperterm, then serve it a terminal\n\n\
         Credentials an app asks for by name:\n\
           secret set NAME [--from PATH] | list | remove NAME   Tokens and passwords\n\
           trust set NAME --from PATH | list | remove NAME      A private HTTPS root\n\n\
         Build apps:\n\
           new <name>             Create a Rust application\n\
           dev [address]          Run this app in the browser simulator\n\
           dev --runtime [address] [--apps IDs]  Run the launcher and selected local apps\n\
           drive --script PATH [--record DIR]  Tap through the simulator and save screenshots\n\
           build [--device]       Build the host workspace or the device binaries\n\
           deploy --device IP [--package PATH]  Install over Wi-Fi, no reboot\n\
           package [--out PATH] [--folder PATH]  Build the KoboRoot.tgz an owner copies\n\
           run --sim [--app NAME] Run SDK, IPC, daemon and one app on this computer\n\
           inspect <package>      List a package and prove it writes nothing to the rootfs\n\
           verify <arm-binary>    Verify static ARM hard-float format\n\n\
         Look at a reader (your paired reader too, unless shown with a target):\n\
           devices [--subnet A.B.C] [--json]  Find every reader on the local network\n\
           doctor --reader NAME [--json]  Read-only diagnostics\n\
           shot [--reader NAME]   Save a PNG of a reader's panel, or the simulator's\n\
           record [--seconds N] [--fps F] [--out DIR]  Film the panel, read-only\n\
           logs [--follow] [--lines N]  Read the runtime trace\n\
           session [--hold MINUTES]  Keep a reader awake and on Wi-Fi while developing\n\
           wait                   Block until a reader answers again\n\
           app-link status|unpair Inspect or revoke browser pairing\n\
           touch-probe [--seconds N]  Watch touch, read-only, to check the transform\n\
           guard-test --device IP --confirm ...  Prove the guardian restores the screen\n\
           shell [command ...]    Advanced: run a command on the reader, unchecked. Exits\n\
           \x20                      with whatever the reader exited with\n\n\
         Publish to the Store:\n\
           app-key --seed PATH    Print the Ed25519 public key for a release seed\n\
           app-bundle --manifest PATH --binary PATH --seed PATH --out PATH\n\
           app-catalog --seed PATH --out PATH --signature PATH --entry PACKAGE HTTPS_URL ...\n\
           app-list | app-check | app-release --registry PATH ...  Store packages\n\
           host-release-sign | host-release-verify --manifest PATH ...\n\n\
           version                Print version"
        )
    );
    print_output_contract();
    print_other_names();
}

/// Which stream carries what, what `--json` prints, and how an exit status
/// is meant to be read. Split from the command list for the same reason the
/// aliases are: the list is at the length the lints allow.
fn print_output_contract() {
    let console = console::Console::detect();
    println!(
        "{}",
        console.wrap(
            "\nReading the output:\n\
             Progress and explanations print on stderr; results print on stdout,\n\
             so kobo devices > readers.txt holds only readers. --json prints one\n\
             JSON object on stdout, with a version field, for programs to read.\n\
             Exit status is a category, stable enough to test in a script:\n\
               0 done   2 the command was misspelled   3 the reader or simulator\n\
               could not be reached   4 this build or host cannot do it   1 anything else\n\
             Verbs stay the same across commands: check reads, preview shows,\n\
             prepare writes on this computer, push sends, status reports."
        )
    );
}

/// The aliases, and the note about what this build can and cannot write.
///
/// Split from the list itself because the list is at the length the lints
/// allow and every new command pushes it over.
fn print_other_names() {
    // Two commands write to the panel and are compiled out without the
    // feature, so they are named here only when they are really present.
    // Advertising a command this binary would reject is worse than saying
    // nothing, and it is the sort of drift a help string invites.
    #[cfg(feature = "device-write")]
    const WRITING: &str = "\n\nBuilt with --features device-write, so also:\n  \
         tap --device IP X,Y [MS:X,Y ...]  Tap the real panel through the real touch node.\n  \
         \x20                              Several steps run in one upload, timed on the\n  \
         \x20                              device, which is how an application is driven.\n  \
         smoke-display --device IP --confirm ...  Attended display checks, one at a time
  \
         present <app> --device IP [--seconds N]  Run one app on the panel
  \
         stop --device IP       Hand the panel back to the reader now";
    #[cfg(not(feature = "device-write"))]
    const WRITING: &str = "\n\nBuilt without --features device-write, so the commands that write \
         to a panel\n(tap, smoke-display) are not in this binary.";
    println!(
        "\nEvery command that takes --device also takes -s, and these names\n\
         work if they are the ones you already know:\n\
           logcat -> logs   install -> deploy   wait-for-device -> wait\n\
           sim, simulator -> dev   init, create -> new{WRITING}"
    );
}

#[cfg(test)]
mod tests;

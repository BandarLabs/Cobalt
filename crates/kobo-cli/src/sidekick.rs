//! Manage the Sidekick helper from the companion, and prove the path works.
//!
//! The helper is its own binary, and until now an owner had to know that: find
//! `kobo-sidekickd`, run `setup` for their agent, remember to start it, and
//! have no way to ask whether it was running or to stop it. Everything else on
//! this device that keeps a helper alive is `kobo <thing> setup|run|status|
//! stop`, which is what Sync established, so Sidekick answers to the same four
//! words.
//!
//! `test` is the fifth, and it is the one that matters on a first evening: it
//! sends a harmless question of its own, waits for the reader to answer it, and
//! prints what came back. A permission prompt from a real agent is a poor first
//! test, because the thing it is asking to do is usually something nobody wants
//! to say yes to twice.
//!
//! `init` pairs more than Sidekick. Sidekick and Deck talk to the same helper
//! and Paperterm to its own host on this computer, and each used to want an
//! address, a pairing code and a `kobo trust set` of its own: the owner paired
//! three times and typed six things on e-ink. The companion already writes
//! app state over SSH for `kobo deck push`, so `init` now installs both trust
//! roots and writes all three pairings onto the saved reader in one visit.
use std::fs;
use std::io::Read as _;
use std::io::Write as _;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The port the helper listens on for questions from an agent's hook.
const HOOK_PORT: u16 = 9330;

/// How long `test` waits for somebody to pick the reader up and answer.
///
/// Ten minutes rather than two: the first time anybody runs this they are also
/// pairing the reader, and two minutes ran out while the pairing code was
/// still being typed.
const TEST_PATIENCE: Duration = Duration::from_secs(600);

/// How long a background start waits to see the helper actually listening.
const START_PATIENCE: Duration = Duration::from_secs(10);

/// The port the reader reaches the helper on, for Sidekick and Deck.
const READER_PORT: u16 = 9331;

/// How long the one SSH visit that pairs the reader may take.
const DELIVERY_PATIENCE: Duration = Duration::from_secs(60);

/// Where the reader keeps owner trust roots and each app's saved state.
const DEVICE_TRUST: &str = "/mnt/onboard/.adds/cobalt/trust";
const DEVICE_STATE: &str = "/mnt/onboard/.adds/cobalt/state";

const USAGE: &str = "usage: kobo sidekick init [--sim | --device HOST | --reader NAME] [--host ADDRESS ...] [--tailnet]\n\
                     \x20      kobo sidekick setup [AGENT] | run [--foreground] | status | stop | sample | test";

/// What `kobo sidekick init --help` prints: the usage, and what each way of
/// choosing this computer's address means for where the pairing works.
const INIT_HELP: &str = "usage: kobo sidekick init [--sim | --device HOST | --reader NAME] [--host ADDRESS ...] [--tailnet]

Pairs Sidekick, Deck, Books and Paperterm on the reader with this computer.
Each is given where to reach this computer:
  (neither)       This computer's address on the network it is on, which works
                  while the reader is on the same network. When this computer is
                  on a tailnet, its Tailscale name as well, which works from any
                  network while Tailscale is on here and on the reader. Each app
                  tries the one that last answered first, then the other.
  --tailnet       This computer's Tailscale name only.
  --host ADDRESS  The address or name you give, only. Repeat it to name more in
                  the certificates; the reader is given the first, or the
                  Tailscale name when --tailnet is given too.
When this computer is on a tailnet, the certificates name its tailnet name and
address either way, unless you name the hosts yourself with --host.";

pub fn command(arguments: &[String]) -> Result<(), String> {
    // Asked here, before any verb runs, so that `run --help` describes the
    // helper rather than starting it. `init` has a longer answer of its own.
    if arguments
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
        || matches!(arguments, [help] if help == "help")
    {
        let help = if arguments.first().is_some_and(|verb| verb == "init") {
            INIT_HELP
        } else {
            USAGE
        };
        println!("{help}");
        return Ok(());
    }
    match arguments.split_first() {
        Some((verb, rest)) => match (verb.as_str(), rest) {
            ("init", extra) => init(extra),
            ("setup", extra) => setup(extra),
            ("run", extra) => run(extra),
            ("status", []) => {
                status();
                Ok(())
            }
            ("stop", []) => stop(),
            ("sample", []) => sample(),
            ("test", extra) => test(extra),
            _ => Err(USAGE.to_owned()),
        },
        None => Err(USAGE.to_owned()),
    }
}

/// Mints the helper's identity and Paperterm's, then pairs the reader.
///
/// Deck and Sidekick both ask for an address and a code, and Paperterm for a
/// third pair of its own. With a saved reader, nothing has to be typed: both
/// trust roots and all three pairings are written onto it over SSH. Without
/// one, the address and codes are printed for the reader's screens instead.
fn init(arguments: &[String]) -> Result<(), String> {
    let (target, rest) = super::targets::TargetArgs::parse(arguments)?;
    let (hosts, prefer_tailnet) = parse_hosts(&rest)?;
    let helper = locate()?;
    // The reader is found before anything is minted, so a name that does not
    // resolve leaves nothing half done behind it.
    let destination = destination(&target)?;
    let simulator = matches!(destination, Some(Destination::Simulator));
    if simulator && prefer_tailnet {
        return Err(USAGE.to_owned());
    }
    let Minted {
        plan,
        computers,
        helper_is_stale,
    } = mint(&helper, &hosts, prefer_tailnet, simulator)?;
    let pairing = Pairing::gather(&computers)?;
    let Some(destination) = destination else {
        println!(
            "No reader is saved on this computer, so nothing was written to one.\n\
             Set the reader up once with kobo setup, then run kobo sidekick init again,\n\
             or enter these on the reader:\n\n{}",
            pairing.by_hand()
        );
        if helper_is_stale {
            println!("\n{RESTART_HELPER}");
        }
        return Ok(());
    };
    let restart = match &destination {
        Destination::Simulator => {
            pairing.write_simulator()?;
            println!(
                "Paired Sidekick, Deck, Books and Paperterm in the simulator with this computer."
            );
            false
        }
        Destination::Reader(host) => {
            let changed = deliver(host, &pairing.roots, &pairing.apps).map_err(|error| {
                format!(
                    "{error}\n\nThis computer is ready. Run kobo sidekick init again once the \
                     reader is awake, or enter these on the reader:\n\n{}",
                    pairing.by_hand()
                )
            })?;
            println!(
                "Paired Sidekick, Deck, Books and Paperterm on {host} with this computer at {}.\n\
                 There is nothing to type on the reader.",
                computers.join(" and ")
            );
            if let Some(reach) = &plan.reach {
                println!("{reach}");
            }
            // Only sharpens what a later Paperterm session says if this
            // computer moves, so failing to write it undoes nothing.
            let _ignored = kobo_stream::record_paired_hosts(&computers);
            changed
        }
    };
    if restart {
        println!("{RESTART_ONCE}");
    }
    if helper_is_stale {
        println!("{RESTART_HELPER}");
    }
    println!(
        "\nNext:\n  \
         kobo sidekick setup claude     let Claude Code ask through Sidekick (or codex)\n  \
         kobo deck init --preset build  give Deck its first pads, from your project folder\n  \
         kobo sidekick run              start the helper Sidekick and Deck talk to\n  \
         kobo stream terminal           share a terminal with Paperterm"
    );
    Ok(())
}

/// What both mints left behind for the pairing to be made from.
struct Minted {
    /// The hosts the certificates name, and what there is to say about them.
    plan: super::tailnet::Plan,
    /// The hosts the reader is given for this computer, the one to try first
    /// first.
    computers: Vec<String>,
    /// Whether a helper is running with a certificate older than the one
    /// just minted.
    helper_is_stale: bool,
}

/// Decides which hosts this computer answers to and mints both identities
/// for them: the helper's, and Paperterm's.
fn mint(
    helper: &Path,
    hosts: &[String],
    prefer_tailnet: bool,
    simulator: bool,
) -> Result<Minted, String> {
    // This computer's own address, and its tailnet name and address when it
    // has them. The simulator runs on this computer and reaches it over
    // loopback, so a tailnet is not asked about on its account.
    let local = if hosts.is_empty() {
        super::connect::local_address()
    } else {
        None
    };
    let source: &dyn super::tailnet::StatusSource = if simulator {
        &super::tailnet::NotAsked
    } else {
        &super::tailnet::Installed
    };
    let plan = super::tailnet::plan(hosts, local.as_deref(), prefer_tailnet, source)?;
    if let Some(named) = &plan.named {
        println!("{named}");
    }
    // The helper finds the local address itself each time it starts and
    // keeps what it is given here for good. So it is given every host but
    // that one: an address the router handed out would otherwise stay in its
    // certificate after the computer had moved on from it.
    let extra: Vec<String> = plan
        .certificate
        .iter()
        .filter(|host| Some(host.as_str()) != local.as_deref())
        .cloned()
        .collect();
    let leaf = config()?.join("cert.pem");
    let leaf_before = fs::read(&leaf).ok();
    let output = Command::new(helper)
        .arg("init")
        .args(super::host_flags(&extra))
        .output()
        .map_err(|error| format!("run {}: {error}", helper.display()))?;
    if !output.status.success() {
        return Err(format!(
            "the Sidekick helper could not prepare this computer: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    // The helper reads its certificate when it starts. One that is already
    // running goes on presenting the certificate it started with, which
    // does not name a host added just now, and a reader paired with that
    // host would be refused by a helper that looks perfectly healthy.
    let helper_is_stale = fs::read(&leaf).ok() != leaf_before && listening();
    let computers = if !plan.written.is_empty() {
        plan.written.clone()
    } else if simulator {
        vec!["127.0.0.1".to_owned()]
    } else {
        return Err(
            "This computer has no network address a reader could reach. Join the \
                    reader's Wi-Fi and run kobo sidekick init again."
                .to_owned(),
        );
    };
    let stream_hosts = if plan.certificate.is_empty() {
        computers.clone()
    } else {
        plan.certificate.clone()
    };
    kobo_stream::init(&super::host_flags(&stream_hosts))?;
    Ok(Minted {
        plan,
        computers,
        helper_is_stale,
    })
}

/// Said only when a trust root was new or different, because only then does
/// the reader need it: the runtime reads its roots when Cobalt opens.
const RESTART_ONCE: &str = "Close Cobalt on the reader and open it again, once, so it trusts \
                            this computer.";

/// Said when the certificate changed under a helper that is running.
const RESTART_HELPER: &str = "The Sidekick helper is still running with its earlier certificate. \
                              Run kobo sidekick stop, then kobo sidekick run, so it answers at \
                              the new address.";

/// The `--host` addresses and whether `--tailnet` was given, the only words
/// `init` takes besides a target.
fn parse_hosts(rest: &[String]) -> Result<(Vec<String>, bool), String> {
    let mut hosts = Vec::new();
    let mut prefer_tailnet = false;
    let mut words = rest.iter();
    while let Some(word) = words.next() {
        if word == "--tailnet" {
            prefer_tailnet = true;
            continue;
        }
        if word != "--host" {
            return Err(USAGE.to_owned());
        }
        let host = words.next().ok_or("--host needs an address")?;
        if host.is_empty()
            || !host
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b':'))
        {
            return Err(format!("{host:?} is not an address the reader could reach"));
        }
        hosts.push(host.clone());
    }
    Ok((hosts, prefer_tailnet))
}

/// Where the pairing goes.
enum Destination {
    Simulator,
    Reader(String),
}

/// The target named on the command line, or the reader saved on this
/// computer. `None` when there is neither, which is not an error: the owner
/// can still type the pairing on the reader.
fn destination(target: &super::targets::TargetArgs) -> Result<Option<Destination>, String> {
    use super::targets::{DefaultReader, Target};
    if !target.is_empty() {
        return Ok(Some(match target.resolve()? {
            Target::Simulator => Destination::Simulator,
            Target::Address(host) => {
                if !super::valid_device_host(&host) {
                    return Err(format!("{host:?} is not an address this can reach"));
                }
                Destination::Reader(host)
            }
            Target::Nickname(name) => Destination::Reader(super::targets::resolve_nickname(&name)?),
        }));
    }
    Ok(match super::targets::configured_default() {
        Some(DefaultReader::Address(host)) => Some(Destination::Reader(host)),
        Some(DefaultReader::Nickname(name)) => {
            let host = super::targets::resolve_nickname(&name)?;
            println!("Using your saved reader \"{name}\" at {host}.");
            Some(Destination::Reader(host))
        }
        None => None,
    })
}

/// One app's pairing, as the app itself saves it.
///
/// The apps grew three formats, and each is kept as it is so that a reader
/// already paired by hand reads the same file it always wrote. Every one of
/// them reads the code as everything after the address, so a second host
/// cannot go in the same file without becoming part of the code on a reader
/// that has not been updated. It goes under a key of its own beside it,
/// which only an app that knows about it asks for.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AppPairing {
    app: &'static str,
    key: &'static str,
    /// What is written under the key, or `None` to remove the key.
    value: Option<String>,
}

impl AppPairing {
    fn sidekick(address: &str, code: &str) -> Self {
        Self {
            app: "sidekick",
            key: "paired",
            value: Some(format!("{address}\n{code}")),
        }
    }

    fn deck(address: &str, code: &str) -> Self {
        Self {
            app: "deck",
            key: "paired",
            value: Some(format!("{address}|{code}")),
        }
    }

    /// Books asks the same helper for articles waiting on this computer
    /// when Sync is tapped on its Articles shelf.
    fn books(address: &str, code: &str) -> Self {
        Self {
            app: "books",
            key: "computer",
            value: Some(format!("{address}|{code}")),
        }
    }

    pub(crate) fn paperterm(address: &str, code: &str) -> Self {
        Self {
            app: "paperterm",
            key: "pairing",
            value: Some(format!("{address}\n{code}")),
        }
    }

    /// Every host `app` reaches this computer at, beside its pairing.
    ///
    /// With one host the record is removed rather than written: one left by
    /// an earlier pairing would otherwise send the app to a host the owner
    /// has just chosen not to use.
    pub(crate) fn hosts(app: &'static str, addresses: &[String]) -> Self {
        Self {
            app,
            key: kobo_dial::HOSTS_KEY,
            value: (addresses.len() > 1).then(|| kobo_dial::record(addresses)),
        }
    }
}

/// Everything one visit to the reader carries.
struct Pairing {
    sidekick_address: String,
    sidekick_code: String,
    paperterm_address: String,
    paperterm_code: String,
    roots: Vec<(&'static str, String)>,
    apps: Vec<AppPairing>,
}

impl Pairing {
    /// Reads back what the two `init`s just made, for a computer the reader
    /// reaches at `computers`, the one to try first first.
    fn gather(computers: &[String]) -> Result<Self, String> {
        let directory = config()?;
        let read = |name: &str| {
            fs::read_to_string(directory.join(name))
                .map(|text| text.trim().to_owned())
                .map_err(|error| format!("read {}: {error}", directory.join(name).display()))
        };
        let sidekick_code = read("pairing")?;
        let sidekick_root = read("ca-cert.pem")?;
        let paperterm_code = kobo_stream::saved_code()?;
        let stream_path = kobo_stream::authority_path()?;
        let stream_root = fs::read_to_string(&stream_path)
            .map_err(|error| format!("read {}: {error}", stream_path.display()))?;
        let at = |port| {
            computers
                .iter()
                .map(|computer| host_port(computer, port))
                .collect::<Vec<_>>()
        };
        let sidekick_addresses = at(READER_PORT);
        let paperterm_addresses = at(kobo_stream::DEFAULT_PORT);
        let sidekick_address = sidekick_addresses.first().cloned().unwrap_or_default();
        let paperterm_address = paperterm_addresses.first().cloned().unwrap_or_default();
        Ok(Self {
            apps: vec![
                AppPairing::sidekick(&sidekick_address, &sidekick_code),
                AppPairing::hosts("sidekick", &sidekick_addresses),
                AppPairing::deck(&sidekick_address, &sidekick_code),
                AppPairing::hosts("deck", &sidekick_addresses),
                AppPairing::books(&sidekick_address, &sidekick_code),
                AppPairing::hosts("books", &sidekick_addresses),
                AppPairing::paperterm(&paperterm_address, &paperterm_code),
                AppPairing::hosts("paperterm", &paperterm_addresses),
            ],
            roots: vec![("sidekick", sidekick_root), ("stream", stream_root)],
            sidekick_address,
            sidekick_code,
            paperterm_address,
            paperterm_code,
        })
    }

    /// The same pairing, for typing on the reader's own screens.
    fn by_hand(&self) -> String {
        format!(
            "  Sidekick and Deck  address {}  code {}\n  Paperterm          address {}  code {}",
            self.sidekick_address, self.sidekick_code, self.paperterm_address, self.paperterm_code
        )
    }

    /// The simulator reads its trust roots from this computer's own
    /// directory, which both `init`s already filled, and its app state from
    /// a temporary folder, where the pairings go under loopback.
    fn write_simulator(&self) -> Result<(), String> {
        let local = [
            AppPairing::sidekick(&host_port("127.0.0.1", READER_PORT), &self.sidekick_code),
            AppPairing::deck(&host_port("127.0.0.1", READER_PORT), &self.sidekick_code),
            AppPairing::books(&host_port("127.0.0.1", READER_PORT), &self.sidekick_code),
            AppPairing::paperterm(
                &host_port("127.0.0.1", kobo_stream::DEFAULT_PORT),
                &self.paperterm_code,
            ),
        ];
        for pairing in local {
            let directory = kobo_sim::simulated_state_root(pairing.app);
            fs::create_dir_all(&directory)
                .map_err(|error| format!("create {}: {error}", directory.display()))?;
            let path = directory.join(pairing.key);
            if let Some(value) = &pairing.value {
                fs::write(&path, value.as_bytes())
                    .map_err(|error| format!("write {}: {error}", path.display()))?;
            }
            // The simulator reaches this computer at loopback alone, so a
            // hosts record from a reader's pairing has no place here.
            forget_simulated(pairing.app, kobo_dial::HOSTS_KEY)?;
        }
        Ok(())
    }
}

/// Removes one key from an app's simulated state, if it is there.
pub(crate) fn forget_simulated(app: &str, key: &str) -> Result<(), String> {
    let path = kobo_sim::simulated_state_root(app).join(key);
    match fs::remove_file(&path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            Err(format!("remove {}: {error}", path.display()))
        }
        _ => Ok(()),
    }
}

/// `host:port`, bracketing an IPv6 host so the port is not read as part of it.
fn host_port(host: &str, port: u16) -> String {
    if host.parse::<std::net::Ipv6Addr>().is_ok() {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// Installs trust roots and app pairings on a reader in one SSH visit.
///
/// Returns whether any trust root was new or different. The runtime reads its
/// roots only when Cobalt opens, so that is the one case where the owner has
/// to reopen it; a reader re-paired after a code change needs nothing.
pub(crate) fn deliver(
    host: &str,
    roots: &[(&str, String)],
    apps: &[AppPairing],
) -> Result<bool, String> {
    let output = super::run_remote_shell(
        &format!("root@{host}"),
        &delivery_script(roots, apps),
        DELIVERY_PATIENCE,
    )
    .map_err(super::unreachable_device)?;
    if !output.status.success() {
        return Err(super::unreachable_if_ssh_gave_up(
            super::remote_shell_error(
                format!("pairing the reader at {host} failed"),
                &output.stdout,
                &output.stderr,
            ),
            &output,
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).contains("trust-changed=1"))
}

/// The script `deliver` runs on the reader.
///
/// Everything travels base64 encoded inside quoted here-documents, so no
/// byte of a certificate or code is ever read by the reader's shell. Each
/// file is written beside its final name and moved into place, so an
/// interrupted visit leaves the previous pairing whole.
fn delivery_script(roots: &[(&str, String)], apps: &[AppPairing]) -> String {
    let mut script = format!(
        "set -eu\n\
         umask 077\n\
         trust='{DEVICE_TRUST}'\n\
         state='{DEVICE_STATE}'\n\
         changed=0\n\
         mkdir -p \"$trust\"\n\
         put_root() {{\n\
           partial=\"$trust/.$1.pem.writing\"\n\
           base64 -d > \"$partial\"\n\
           if [ -f \"$trust/$1.pem\" ] && cmp -s \"$partial\" \"$trust/$1.pem\"; then\n\
             rm -f \"$partial\"\n\
           else\n\
             mv -f \"$partial\" \"$trust/$1.pem\"\n\
             changed=1\n\
           fi\n\
         }}\n\
         put_state() {{\n\
           mkdir -p \"$state/$1\"\n\
           chmod 700 \"$state/$1\"\n\
           partial=\"$state/$1/.$2.writing\"\n\
           base64 -d > \"$partial\"\n\
           chmod 600 \"$partial\"\n\
           mv -f \"$partial\" \"$state/$1/$2\"\n\
         }}\n\
         drop_state() {{\n\
           rm -f \"$state/$1/$2\"\n\
         }}\n"
    );
    for (name, pem) in roots {
        script.push_str(&format!(
            "put_root '{name}' <<'KOBO_PAIRING'\n{}\nKOBO_PAIRING\n",
            super::base64_encode(pem.as_bytes())
        ));
    }
    for pairing in apps {
        match &pairing.value {
            Some(value) => script.push_str(&format!(
                "put_state '{}' '{}' <<'KOBO_PAIRING'\n{}\nKOBO_PAIRING\n",
                pairing.app,
                pairing.key,
                super::base64_encode(value.as_bytes())
            )),
            None => script.push_str(&format!("drop_state '{}' '{}'\n", pairing.app, pairing.key)),
        }
    }
    script.push_str("sync\nprintf 'trust-changed=%s\\n' \"$changed\"\n");
    script
}

/// Installs the hook for one agent, through the helper's own setup.
///
/// The helper owns the knowledge of where each agent keeps its hooks, so this
/// does not duplicate it: it finds the binary, runs its setup, and says what
/// happened in the companion's voice.
fn setup(arguments: &[String]) -> Result<(), String> {
    let helper = locate()?;
    let agent = match arguments {
        [] => "claude".to_owned(),
        [name] => name.clone(),
        _ => return Err(USAGE.to_owned()),
    };
    let output = Command::new(&helper)
        .arg("setup")
        .arg(&agent)
        .output()
        .map_err(|error| format!("run {}: {error}", helper.display()))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let problem = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        return Err(format!(
            "the helper refused to set up {agent}: {}",
            problem.trim()
        ));
    }
    if !text.trim().is_empty() {
        println!("{}", text.trim_end());
    }
    println!("Sidekick is set up for {agent}. Start it with 'kobo sidekick run'.");
    Ok(())
}

fn run(arguments: &[String]) -> Result<(), String> {
    let foreground = match arguments {
        [] => false,
        [flag] if flag == "--foreground" => true,
        _ => return Err(USAGE.to_owned()),
    };
    let helper = locate()?;
    if listening() {
        return Err("the Sidekick helper is already running".to_owned());
    }
    if foreground {
        let status = Command::new(&helper)
            .arg("run")
            .env(COMPANION_VARIABLE, companion())
            .status()
            .map_err(|error| format!("run {}: {error}", helper.display()))?;
        return if status.success() {
            Ok(())
        } else {
            Err(format!("the helper exited with {status}"))
        };
    }
    let mut command = Command::new(&helper);
    command
        .arg("run")
        .env(COMPANION_VARIABLE, companion())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // A Windows console program dies with the console it was started from,
    // so closing the window that ran `kobo sidekick run` would take the
    // helper with it. Detached, it outlives that window as it does on Unix.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    let child = command
        .spawn()
        .map_err(|error| format!("start {}: {error}", helper.display()))?;
    let pid = child.id();
    // Started is not the same as listening, and reporting the first as the
    // second is how a companion tells somebody everything is fine while the
    // reader sees nothing.
    let deadline = Instant::now() + START_PATIENCE;
    while Instant::now() < deadline {
        if listening() {
            write_pid(pid)?;
            println!(
                "Sidekick helper started in the background (PID {pid}).\nRun 'kobo sidekick test' to send the reader a harmless question, or 'kobo sidekick stop' to stop it."
            );
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Err(format!(
        "the helper started (PID {pid}) but is not answering on port {HOOK_PORT}; run 'kobo sidekick run --foreground' to see why"
    ))
}

fn status() {
    let helper = locate().ok();
    let running = listening();
    println!(
        "Sidekick helper: {}\n  binary   {}\n  hook     127.0.0.1:{HOOK_PORT}\n  config   {}",
        if running { "running" } else { "stopped" },
        helper.map_or_else(
            || "not found on PATH".to_owned(),
            |path| path.display().to_string()
        ),
        config().map_or_else(|_| "unknown".to_owned(), |path| path.display().to_string()),
    );
    if let Some(pid) = read_pid() {
        println!("  started  PID {pid} by this companion");
    }
    if !running {
        println!("Run 'kobo sidekick run' to start it.");
        return;
    }
    println!("{}", reader_line(reader_watching()));
}

/// Whether Sidekick on a reader is in touch with the running helper, as the
/// helper sees it. `None` when the helper is too old to say.
fn reader_watching() -> Option<bool> {
    let request =
        format!("GET /status HTTP/1.1\r\nHost: 127.0.0.1:{HOOK_PORT}\r\nConnection: close\r\n\r\n");
    let mut stream = TcpStream::connect(("127.0.0.1", HOOK_PORT)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    stream.write_all(request.as_bytes()).ok()?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response).ok()?;
    let head = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")?;
    let body = String::from_utf8_lossy(&response[head + 4..]).into_owned();
    kobo_json::parse(&body)
        .ok()?
        .get("reader_watching")
        .and_then(kobo_json::Value::as_bool)
}

/// The reader's half of `status`, in words.
fn reader_line(watching: Option<bool>) -> &'static str {
    match watching {
        Some(true) => "  reader   Sidekick is open on a reader and watching",
        Some(false) => {
            "  reader   no reader in touch, so agents' questions go straight to their terminals"
        }
        None => "  reader   unknown; this helper is older than the companion",
    }
}

fn stop() -> Result<(), String> {
    let Some(pid) = read_pid() else {
        return if listening() {
            Err(
                "the helper is running but was not started by this companion; stop it where it was started"
                    .to_owned(),
            )
        } else {
            println!("The Sidekick helper is not running.");
            Ok(())
        };
    };
    // A helper that is not answering has gone, and the number it left behind
    // may by now belong to something else entirely. Signalling it would end
    // that instead, so a silent helper's record is only cleared.
    if !listening() {
        clear_pid();
        println!("The Sidekick helper is not running.");
        return Ok(());
    }
    let signalled = stop_command(pid)
        .status()
        .map_err(|error| format!("stop PID {pid}: {error}"))?;
    if !signalled.success() {
        clear_pid();
        return Err(format!("PID {pid} could not be stopped; it may be gone"));
    }
    let deadline = Instant::now() + START_PATIENCE;
    while Instant::now() < deadline {
        if !listening() {
            clear_pid();
            println!("Sidekick helper stopped.");
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Err(format!(
        "PID {pid} was asked to stop and is still answering"
    ))
}

/// The command that asks process `pid` to end. Windows has no `kill`;
/// `taskkill` ships with every edition, and needs `/F` because a helper with
/// no window never receives the close message it sends without it.
fn stop_command(pid: u32) -> Command {
    #[cfg(windows)]
    {
        let mut command = Command::new("taskkill");
        command
            .args(["/F", "/PID", &pid.to_string()])
            .stdout(Stdio::null());
        command
    }
    #[cfg(not(windows))]
    {
        let mut command = Command::new("kill");
        command.arg(pid.to_string());
        command
    }
}

/// Runs the helper's own sample, for a reader that has nothing set up yet.
///
/// Two ways to see this work, and they answer different questions. `sample`
/// starts a helper with no agent hooks and no deck, asks its own question and
/// waits: it is for somebody who has installed nothing and wants to see the
/// path light up. `test` sends a question through a helper that is already
/// running for real, which is the one that proves an actual installation.
///
/// Both live here so that neither needs the owner to know the helper binary
/// exists, which is the whole point of this module.
fn sample() -> Result<(), String> {
    let helper = locate()?;
    if listening() {
        return Err(
            "the Sidekick helper is already running; use 'kobo sidekick test' to send that one a question"
                .to_owned(),
        );
    }
    let status = Command::new(&helper)
        .arg("sample")
        .status()
        .map_err(|error| format!("run {}: {error}", helper.display()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("the sample ended with {status}"))
    }
}

/// Sends the reader a question of the companion's own and prints the answer.
fn test(arguments: &[String]) -> Result<(), String> {
    if !arguments.is_empty() {
        return Err(USAGE.to_owned());
    }
    if !listening() {
        return Err(
            "the Sidekick helper is not running; start it with 'kobo sidekick run'".to_owned(),
        );
    }
    if reader_watching() == Some(false) {
        println!("Sidekick is not open on a reader yet. Open it there; this test waits for it.");
    }
    // Nothing here runs anything. The detail says so, because the whole point
    // of the test is that answering it is safe either way. An agent's own
    // question goes back to its terminal when no reader is watching; this
    // one waits, because opening the reader is what is being tested.
    let body = kobo_json::ObjectBuilder::new()
        .set("source", "kobo sidekick test")
        .set("session", "companion check")
        .set("tool", "Sample")
        .set(
            "detail",
            "This is a test from your computer. Nothing runs whichever you choose.",
        )
        .set("choices", kobo_json::Value::Array(Vec::new()))
        .set("permission", true)
        .set("multi", false)
        .set("wait_for_reader", true)
        .build()
        .to_json();
    println!(
        "Sent a test question to the reader. Pick it up and answer; waiting up to {} seconds.",
        TEST_PATIENCE.as_secs()
    );
    let reply = post_ask(&body, TEST_PATIENCE)?;
    let decision = kobo_json::parse(&reply)
        .ok()
        .and_then(|value| {
            value
                .get("decision")
                .and_then(kobo_json::Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_default();
    match decision.as_str() {
        "allow" => println!("The reader answered: allow. The whole path works."),
        "deny" => println!("The reader answered: deny. The whole path works."),
        "" | "pass" => println!(
            "The reader left it for the terminal, which is also an answer: the question reached it and came back."
        ),
        other => println!("The reader answered: {other}. The whole path works."),
    }
    Ok(())
}

fn post_ask(body: &str, patience: Duration) -> Result<String, String> {
    let mut stream = TcpStream::connect(("127.0.0.1", HOOK_PORT))
        .map_err(|error| format!("reach the helper on port {HOOK_PORT}: {error}"))?;
    stream
        .set_read_timeout(Some(patience))
        .map_err(|error| format!("set a timeout: {error}"))?;
    let request = format!(
        "POST /ask HTTP/1.1\r\nHost: 127.0.0.1:{HOOK_PORT}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|error| format!("send the question: {error}"))?;
    let mut response = Vec::new();
    // A timeout here is nobody picking the reader up, which is a sentence
    // rather than an error number: this reported "Resource temporarily
    // unavailable (os error 35)" the first time it happened.
    stream.read_to_end(&mut response).map_err(|error| {
        if matches!(
            error.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        ) {
            format!(
                "nobody answered on the reader within {} minutes; the question is still waiting there",
                patience.as_secs() / 60
            )
        } else {
            format!("wait for the answer: {error}")
        }
    })?;
    let head = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or("the helper sent something this companion could not read")?;
    Ok(String::from_utf8_lossy(&response[head + 4..]).into_owned())
}

/// Whether anything is answering where the helper listens.
fn listening() -> bool {
    TcpStream::connect_timeout(
        &(std::net::Ipv4Addr::LOCALHOST, HOOK_PORT).into(),
        Duration::from_millis(400),
    )
    .is_ok()
}

/// Where the helper finds this companion, to run `kobo clip sync` when Sync
/// is tapped on the reader. The helper looks beside itself and on PATH
/// without it.
const COMPANION_VARIABLE: &str = "KOBO_COMPANION";

fn companion() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("kobo"))
}

/// The helper beside this binary first, then whatever is on PATH.
///
/// Beside first because that is where a release puts them together, and an
/// owner who has just unpacked one should not need a PATH entry to use it.
/// PATH is searched here rather than through `command -v`, which is a shell
/// builtin and not a program: starting it directly found nothing on Linux or
/// Windows, and on Windows the helper is also named with `.exe`.
fn locate() -> Result<PathBuf, String> {
    let name = format!("kobo-sidekickd{}", std::env::consts::EXE_SUFFIX);
    let beside = std::env::current_exe()
        .ok()
        .and_then(|here| here.parent().map(|directory| directory.join(&name)));
    let on_path = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .unwrap_or_default();
    beside
        .into_iter()
        .chain(on_path.into_iter().map(|directory| directory.join(&name)))
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| {
            format!(
                "{name} is not beside this binary or on PATH; install it from the release you are running"
            )
        })
}

/// The helper's own folder, found the way the helper finds it, so the two
/// never disagree about where the pairing code lives.
fn config() -> Result<PathBuf, String> {
    if let Some(root) = std::env::var_os("KOBO_SIDEKICK_CONFIG_DIR") {
        return Ok(PathBuf::from(root).join("sidekick"));
    }
    let home = kobo_abi::home_directory().ok_or_else(|| "HOME is not set".to_owned())?;
    Ok(Path::new(&home).join(".config/kobo/sidekick"))
}

fn pid_file() -> Result<PathBuf, String> {
    Ok(config()?.join("companion.pid"))
}

fn write_pid(pid: u32) -> Result<(), String> {
    let path = pid_file()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    fs::write(&path, format!("{pid}\n")).map_err(|error| format!("{}: {error}", path.display()))
}

fn read_pid() -> Option<u32> {
    fs::read_to_string(pid_file().ok()?)
        .ok()?
        .trim()
        .parse()
        .ok()
}

fn clear_pid() {
    if let Ok(path) = pid_file() {
        let _ignored = fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::{command, USAGE};

    fn words(input: &[&str]) -> Vec<String> {
        input.iter().map(|word| (*word).to_owned()).collect()
    }

    #[test]
    fn every_verb_is_one_the_other_helpers_already_use() {
        // Sync established setup, run, status and stop, and an owner who has
        // used one helper should not have to learn another vocabulary.
        for verb in ["setup", "run", "status", "stop", "sample", "test"] {
            assert!(USAGE.contains(verb), "{verb} is not in the usage line");
        }
    }

    #[test]
    fn an_unknown_verb_prints_the_usage_rather_than_guessing() {
        for arguments in [vec![], words(&["restart"]), words(&["status", "--json"])] {
            let error = command(&arguments).expect_err("refused");
            assert_eq!(error, USAGE, "{arguments:?}");
        }
    }

    #[test]
    fn asking_any_verb_for_help_answers_without_running_it() {
        // `run --help` used to reach the reference line printed for commands
        // that do not answer their own help, and once this command answered
        // its own, `run` would have taken the flag as a reason to start the
        // helper. Every form is answered here before a verb is looked at.
        for arguments in [
            words(&["--help"]),
            words(&["-h"]),
            words(&["help"]),
            words(&["run", "--help"]),
            words(&["stop", "-h"]),
            words(&["init", "--help"]),
            words(&["init", "--device", "192.0.2.1", "--help"]),
        ] {
            assert_eq!(command(&arguments), Ok(()), "{arguments:?}");
        }
        assert_eq!(
            super::super::unanswered_help(&words(&["sidekick", "init", "--help"])),
            None,
            "the reference line would be printed instead of the init help"
        );
    }

    #[test]
    fn init_accepts_a_target_and_host_addresses_and_nothing_else() {
        assert_eq!(
            super::parse_hosts(&words(&["--host", "studio.local", "--host", "10.0.0.4"])),
            Ok((words(&["studio.local", "10.0.0.4"]), false))
        );
        assert_eq!(
            super::parse_hosts(&words(&["--tailnet"])),
            Ok((Vec::new(), true))
        );
        assert_eq!(
            super::parse_hosts(&words(&["--host", "studio.local", "--tailnet"])),
            Ok((words(&["studio.local"]), true))
        );
        assert_eq!(
            super::parse_hosts(&words(&["--force"])),
            Err(USAGE.to_owned())
        );
        assert!(super::parse_hosts(&words(&["--host"])).is_err());
        assert!(super::parse_hosts(&words(&["--host", "a;b"])).is_err());
    }

    #[test]
    fn each_app_gets_its_pairing_in_the_format_it_already_reads() {
        let sidekick = super::AppPairing::sidekick("192.168.1.10:9331", "abc234");
        assert_eq!((sidekick.app, sidekick.key), ("sidekick", "paired"));
        assert_eq!(sidekick.value.as_deref(), Some("192.168.1.10:9331\nabc234"));
        let deck = super::AppPairing::deck("192.168.1.10:9331", "abc234");
        assert_eq!((deck.app, deck.key), ("deck", "paired"));
        assert_eq!(deck.value.as_deref(), Some("192.168.1.10:9331|abc234"));
        let paperterm = super::AppPairing::paperterm("192.168.1.10:9332", "xyz789");
        assert_eq!((paperterm.app, paperterm.key), ("paperterm", "pairing"));
        assert_eq!(
            paperterm.value.as_deref(),
            Some("192.168.1.10:9332\nxyz789")
        );
        let books = super::AppPairing::books("192.168.1.10:9331", "abc234");
        assert_eq!((books.app, books.key), ("books", "computer"));
        assert_eq!(books.value.as_deref(), Some("192.168.1.10:9331|abc234"));
        assert_eq!(super::host_port("fe80::1", 9331), "[fe80::1]:9331");
    }

    #[test]
    fn a_second_host_goes_beside_each_pairing_where_older_apps_never_look() {
        let both = words(&["192.168.1.10:9331", "studio.example.ts.net:9331"]);
        for app in ["sidekick", "deck", "books", "paperterm"] {
            let hosts = super::AppPairing::hosts(app, &both);
            assert_eq!((hosts.app, hosts.key), (app, "hosts"));
            assert_eq!(
                hosts.value.as_deref(),
                Some("192.168.1.10:9331\nstudio.example.ts.net:9331")
            );
            // One host removes a record an earlier pairing left.
            let one = super::AppPairing::hosts(app, &both[..1]);
            assert_eq!(one.value, None);
        }
        // Each older app's own reading of its pairing, which reads the code as
        // everything after the address. The pairing is the same with one host
        // or two, so each still reads the first host and the right code.
        let sidekick = super::AppPairing::sidekick(&both[0], "abc234")
            .value
            .expect("set");
        let (address, code) = sidekick.split_once('\n').expect("two lines");
        assert_eq!(
            (address.trim(), code.trim()),
            ("192.168.1.10:9331", "abc234")
        );
        let deck = super::AppPairing::deck(&both[0], "abc234")
            .value
            .expect("set");
        let (address, code) = deck.split_once('|').expect("one bar");
        assert_eq!(
            (address.trim(), code.trim()),
            ("192.168.1.10:9331", "abc234")
        );
        let books = super::AppPairing::books(&both[0], "abc234")
            .value
            .expect("set");
        let (address, code) = books.trim().split_once('|').expect("one bar");
        assert_eq!((address, code), ("192.168.1.10:9331", "abc234"));
        let paperterm = super::AppPairing::paperterm("192.168.1.10:9332", "xyz789")
            .value
            .expect("set");
        let (address, code) = paperterm.split_once('\n').expect("two lines");
        assert_eq!((address, code.trim()), ("192.168.1.10:9332", "xyz789"));
    }

    #[test]
    fn a_pairing_with_one_host_removes_the_hosts_record_on_the_reader() {
        let script = super::delivery_script(
            &[],
            &[
                super::AppPairing::deck("192.168.1.10:9331", "abc234"),
                super::AppPairing::hosts("deck", &words(&["192.168.1.10:9331"])),
            ],
        );
        assert!(script.contains("put_state 'deck' 'paired'"), "{script}");
        assert!(script.contains("\ndrop_state 'deck' 'hosts'\n"), "{script}");
        let both = super::delivery_script(
            &[],
            &[super::AppPairing::hosts(
                "deck",
                &words(&["192.168.1.10:9331", "studio.example.ts.net:9331"]),
            )],
        );
        assert!(
            both.contains("put_state 'deck' 'hosts' <<'KOBO_PAIRING'"),
            "{both}"
        );
        assert!(!both.contains("ts.net"), "the hosts travel encoded");
    }

    #[test]
    fn the_reader_visit_carries_every_root_and_pairing_without_the_shell_reading_them() {
        let script = super::delivery_script(
            &[(
                "sidekick",
                "-----BEGIN CERTIFICATE-----\n$(reboot)\n".to_owned(),
            )],
            &[super::AppPairing::deck("192.168.1.10:9331", "abc234")],
        );
        assert!(
            script.contains("put_root 'sidekick' <<'KOBO_PAIRING'"),
            "{script}"
        );
        assert!(
            script.contains("put_state 'deck' 'paired' <<'KOBO_PAIRING'"),
            "{script}"
        );
        assert!(
            !script.contains("$(reboot)"),
            "a certificate byte reached the shell"
        );
        assert!(!script.contains("abc234"), "the code travelled unencoded");
        assert!(script.contains("/mnt/onboard/.adds/cobalt/state"));
        assert!(script.trim_end().ends_with("\"$changed\""), "{script}");
    }

    #[cfg(unix)]
    #[test]
    fn the_reader_visit_writes_both_hosts_and_a_later_one_host_pairing_takes_them_away() {
        let root = std::env::temp_dir().join(format!(
            "kobo-pairing-hosts-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_nanos())
        ));
        let run = |apps: &[super::AppPairing]| {
            let script = super::delivery_script(&[], apps)
                .replace(
                    super::DEVICE_STATE,
                    &root.join("state").display().to_string(),
                )
                .replace(
                    super::DEVICE_TRUST,
                    &root.join("trust").display().to_string(),
                );
            let output = std::process::Command::new("sh")
                .arg("-c")
                .arg(&script)
                .output()
                .expect("sh runs");
            assert!(output.status.success(), "{output:?}");
        };
        let both = words(&["192.168.1.10:9332", "studio.example.ts.net:9332"]);
        run(&[
            super::AppPairing::paperterm(&both[0], "xyz789"),
            super::AppPairing::hosts("paperterm", &both),
        ]);
        let state = root.join("state/paperterm");
        assert_eq!(
            std::fs::read_to_string(state.join("pairing")).expect("pairing"),
            "192.168.1.10:9332\nxyz789"
        );
        assert_eq!(
            std::fs::read_to_string(state.join("hosts")).expect("hosts"),
            "192.168.1.10:9332\nstudio.example.ts.net:9332"
        );
        run(&[
            super::AppPairing::paperterm(&both[1], "xyz789"),
            super::AppPairing::hosts("paperterm", &both[1..]),
        ]);
        assert!(
            !state.join("hosts").exists(),
            "an earlier record was left behind"
        );
        std::fs::remove_dir_all(&root).expect("clean up");
    }

    #[test]
    fn status_says_plainly_whether_a_reader_is_watching() {
        assert!(super::reader_line(Some(true)).contains("watching"));
        assert!(super::reader_line(Some(false)).contains("straight to their terminals"));
    }

    #[test]
    fn test_refuses_extra_arguments_and_says_how_to_start_the_helper() {
        // With nothing listening, `test` has one useful thing to say, and it
        // is not a connection error.
        let error = command(&words(&["test"])).expect_err("nothing is listening");
        assert!(
            error.contains("kobo sidekick run"),
            "the way out is missing: {error}"
        );
    }
}

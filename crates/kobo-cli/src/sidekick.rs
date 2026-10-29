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

const USAGE: &str = "usage: kobo sidekick init [--sim | --device HOST | --reader NAME] [--host ADDRESS ...]\n\
                     \x20      kobo sidekick setup [AGENT] | run [--foreground] | status | stop | sample | test";

pub fn command(arguments: &[String]) -> Result<(), String> {
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
    let hosts = parse_hosts(&rest)?;
    let helper = locate()?;
    // The reader is found before anything is minted, so a name that does not
    // resolve leaves nothing half done behind it.
    let destination = destination(&target)?;
    let mut flags = Vec::new();
    for host in &hosts {
        flags.push("--host".to_owned());
        flags.push(host.clone());
    }
    let output = Command::new(&helper)
        .arg("init")
        .args(&flags)
        .output()
        .map_err(|error| format!("run {}: {error}", helper.display()))?;
    if !output.status.success() {
        return Err(format!(
            "the Sidekick helper could not prepare this computer: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    // The simulator runs on this computer and reaches it over loopback.
    let computer = hosts
        .first()
        .cloned()
        .or_else(super::connect::local_address)
        .or_else(|| {
            matches!(destination, Some(Destination::Simulator)).then(|| "127.0.0.1".to_owned())
        })
        .ok_or(
            "This computer has no network address a reader could reach. Join the reader's \
             Wi-Fi and run kobo sidekick init again.",
        )?;
    let mut stream_flags = vec!["--host".to_owned(), computer.clone()];
    for host in hosts.iter().skip(1) {
        stream_flags.push("--host".to_owned());
        stream_flags.push(host.clone());
    }
    kobo_stream::init(&stream_flags)?;
    let pairing = Pairing::gather(&computer)?;
    let Some(destination) = destination else {
        println!(
            "No reader is saved on this computer, so nothing was written to one.\n\
             Set the reader up once with kobo setup, then run kobo sidekick init again,\n\
             or enter these on the reader:\n\n{}",
            pairing.by_hand()
        );
        return Ok(());
    };
    let restart = match &destination {
        Destination::Simulator => {
            pairing.write_simulator()?;
            println!("Paired Sidekick, Deck and Paperterm in the simulator with this computer.");
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
                "Paired Sidekick, Deck and Paperterm on {host} with this computer at {computer}.\n\
                 There is nothing to type on the reader."
            );
            changed
        }
    };
    if restart {
        println!("{RESTART_ONCE}");
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

/// Said only when a trust root was new or different, because only then does
/// the reader need it: the runtime reads its roots when Cobalt opens.
const RESTART_ONCE: &str = "Close Cobalt on the reader and open it again, once, so it trusts \
                            this computer.";

/// The `--host` addresses, the only words `init` takes besides a target.
fn parse_hosts(rest: &[String]) -> Result<Vec<String>, String> {
    let mut hosts = Vec::new();
    let mut words = rest.iter();
    while let Some(word) = words.next() {
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
    Ok(hosts)
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
/// The three apps grew three formats, and each is kept as it is so that a
/// reader already paired by hand reads the same file it always wrote.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AppPairing {
    app: &'static str,
    key: &'static str,
    value: String,
}

impl AppPairing {
    fn sidekick(address: &str, code: &str) -> Self {
        Self {
            app: "sidekick",
            key: "paired",
            value: format!("{address}\n{code}"),
        }
    }

    fn deck(address: &str, code: &str) -> Self {
        Self {
            app: "deck",
            key: "paired",
            value: format!("{address}|{code}"),
        }
    }

    pub(crate) fn paperterm(address: &str, code: &str) -> Self {
        Self {
            app: "paperterm",
            key: "pairing",
            value: format!("{address}\n{code}"),
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
    /// Reads back what the two `init`s just made.
    fn gather(computer: &str) -> Result<Self, String> {
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
        let sidekick_address = host_port(computer, READER_PORT);
        let paperterm_address = host_port(computer, kobo_stream::DEFAULT_PORT);
        Ok(Self {
            apps: vec![
                AppPairing::sidekick(&sidekick_address, &sidekick_code),
                AppPairing::deck(&sidekick_address, &sidekick_code),
                AppPairing::paperterm(&paperterm_address, &paperterm_code),
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
            fs::write(&path, pairing.value.as_bytes())
                .map_err(|error| format!("write {}: {error}", path.display()))?;
        }
        Ok(())
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
         }}\n"
    );
    for (name, pem) in roots {
        script.push_str(&format!(
            "put_root '{name}' <<'KOBO_PAIRING'\n{}\nKOBO_PAIRING\n",
            super::base64_encode(pem.as_bytes())
        ));
    }
    for pairing in apps {
        script.push_str(&format!(
            "put_state '{}' '{}' <<'KOBO_PAIRING'\n{}\nKOBO_PAIRING\n",
            pairing.app,
            pairing.key,
            super::base64_encode(pairing.value.as_bytes())
        ));
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
            .status()
            .map_err(|error| format!("run {}: {error}", helper.display()))?;
        return if status.success() {
            Ok(())
        } else {
            Err(format!("the helper exited with {status}"))
        };
    }
    let child = Command::new(&helper)
        .arg("run")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
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
    let signalled = Command::new("kill")
        .arg(pid.to_string())
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

/// The helper beside this binary first, then whatever is on PATH.
///
/// Beside first because that is where a release puts them together, and an
/// owner who has just unpacked one should not need a PATH entry to use it.
fn locate() -> Result<PathBuf, String> {
    if let Ok(here) = std::env::current_exe() {
        if let Some(directory) = here.parent() {
            let beside = directory.join("kobo-sidekickd");
            if beside.is_file() {
                return Ok(beside);
            }
        }
    }
    let found = Command::new("command")
        .args(["-v", "kobo-sidekickd"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|path| !path.is_empty());
    match found {
        Some(path) => Ok(PathBuf::from(path)),
        None => Err(
            "kobo-sidekickd is not beside this binary or on PATH; install it from the release you are running"
                .to_owned(),
        ),
    }
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
    fn init_accepts_a_target_and_host_addresses_and_nothing_else() {
        assert_eq!(
            super::parse_hosts(&words(&["--host", "studio.local", "--host", "10.0.0.4"])),
            Ok(words(&["studio.local", "10.0.0.4"]))
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
        assert_eq!(sidekick.value, "192.168.1.10:9331\nabc234");
        let deck = super::AppPairing::deck("192.168.1.10:9331", "abc234");
        assert_eq!((deck.app, deck.key), ("deck", "paired"));
        assert_eq!(deck.value, "192.168.1.10:9331|abc234");
        let paperterm = super::AppPairing::paperterm("192.168.1.10:9332", "xyz789");
        assert_eq!((paperterm.app, paperterm.key), ("paperterm", "pairing"));
        assert_eq!(paperterm.value, "192.168.1.10:9332\nxyz789");
        assert_eq!(super::host_port("fe80::1", 9331), "[fe80::1]:9331");
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

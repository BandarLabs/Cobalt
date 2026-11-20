//! The small, bounded part of Tailscale Cobalt owns.
//!
//! Tailscale itself remains unmodified. This module owns where its two
//! programs come from, the daemon's home, its process lifetime, the tailnet
//! block in the hosts file, and one fixed set of flags. It never accepts a
//! flag, a route or an exit node from an application: the Tailscale app saves
//! a word that says on or off, and everything else is decided here.
//!
//! What the reader becomes is a leaf. It takes an address on its owner's
//! tailnet so the owner's computers can reach its SSH server from anywhere,
//! which is what lets `kobo send` and `kobo clip` work away from home, and so
//! applications on the reader can reach the owner's own servers. It forwards
//! nothing, advertises nothing and offers no Tailscale SSH.
//!
//! The service runs while Cobalt is open and stops when the reader is handed
//! back, for the reasons a Sync window does. Nothing would be watching the
//! daemon under the stock reader, and its programs live on the partition the
//! reader exports over USB, so a daemon still running from it when the owner
//! plugs in a cable is running from a filesystem a computer is rewriting.
//!
//! Constraints this is built around:
//!
//! * The partition that holds Cobalt is FAT. It cannot keep a private file
//!   private and cannot hold a socket, so the node key and the control socket
//!   live under [`HOME`] on the root filesystem and only the two large
//!   programs live beside the Sync engine.
//! * Being FAT also means anything can be swapped over USB, and every file
//!   there is owned by root whatever wrote it. So each program is checked
//!   against a pinned digest every time the daemon is started, and ownership
//!   is not treated as evidence of anything.
//! * `/dev` is rebuilt at every boot, so the tunnel's device node is made on
//!   every start rather than once at install.
//! * The reader ships no packet filter tools, so the daemon is told to leave
//!   the firewall alone. Every port the reader listens on is therefore
//!   reachable from the tailnet, which is why [`exposure`] is asked before
//!   the reader joins one.
//!
//! No reader has run this yet. The evidence bar in docs/DEVICES.md applies
//! before any of it is called tested.

mod hosts;
#[cfg(test)]
mod tests;

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Where the Tailscale app keeps what the owner chose, and reads what
/// happened. On the book partition like every app's state, so nothing secret
/// is ever written here.
const APP_STATE: &str = "/mnt/onboard/.adds/cobalt/state/tailscale";
/// The daemon's home on the root filesystem: the node key and the socket.
const HOME: &str = "/var/lib/cobalt/tailscale";
/// The two programs, on the book partition beside the Sync engine.
const BIN_DIR: &str = "/mnt/onboard/.adds/cobalt/bin/tailscale";

/// The name this reader asks for on the tailnet. A second reader on the same
/// tailnet is given the same name with a number after it.
const HOSTNAME: &str = "kobo-cobalt";
/// The interface the daemon makes, which is also how the runtime tells a
/// tunnel that is up from a status file that only says so.
const INTERFACE: &str = "tailscale0";

const CONFIG: &str = "tailscale-config";
const STATUS: &str = "tailscale-status";
/// Saved by the app to ask for something a switch cannot say. Its presence
/// with the word below is the whole request, and whatever answers removes it.
const REQUEST: &str = "tailscale-request";
const SIGN_OUT: &str = "sign-out";

/// Left in the daemon's home by a session that is ending, so the supervisor
/// it started, running in a process of its own, ends with it.
const STOP: &str = "stop";
const DAEMON_PID: &str = "tailscaled.pid";
const SUPERVISOR_PID: &str = "supervisor.pid";
/// What the daemon last said, kept for the report somebody will file the
/// first time this fails on a reader nobody has tried.
const DAEMON_LOG: &str = "tailscaled.log";
const LOG_LINES: usize = 200;

/// The most a status answer may be. A tailnet large enough to pass this is
/// not one whose names this reader was going to list.
const LARGEST_ANSWER: u64 = 2 * 1024 * 1024;

/// Room the book partition must have before the download starts: the archive
/// and both programs are on it at once for a moment, with a margin, because
/// the stock reader's library database lives on the same partition and a card
/// filled by a feature most owners never open is a library that will not save.
const ROOM_NEEDED: u64 = 160 * 1024 * 1024;

/// The upstream build this runtime will run, and no other.
///
/// A versioned address rather than the one that always means newest: the
/// digests are what make a download safe to run, and a digest cannot be
/// pinned to a moving target. The archive digest covers the download, and the
/// two program digests cover every later start, when the archive is long
/// gone and the card has been in other hands.
///
/// 1.104.7 rather than the 1.102.2 this began with, for reasons that matter
/// on this device and not because it is newer: 1.104 sizes its packet queues
/// to the number of cores, of which a reader has one, keeps the last network
/// map so a start without the control plane still connects, and fixes a stall
/// on connections that have been quiet, which describes a reader exactly.
#[derive(Clone, Debug)]
struct Engine {
    url: &'static str,
    /// The folder inside the archive that holds the two programs.
    folder: &'static str,
    archive_sha256: &'static str,
    daemon_sha256: &'static str,
    cli_sha256: &'static str,
    /// The reviewed archive is 37.2 MB. Allow headroom, refuse absurdity.
    limit: u32,
}

const PINNED: Engine = Engine {
    url: "https://pkgs.tailscale.com/stable/tailscale_1.104.7_arm.tgz",
    folder: "tailscale_1.104.7_arm",
    archive_sha256: "1c753fef45d497491081db37552b7a2a42c7932292743bf6936cb1a42b6bceb0",
    daemon_sha256: "7b6e407353575cb4dcd1d95da7a1b197c9e6c0597b9570007d7564d2929a7ae4",
    cli_sha256: "1c14fb51b91c9d70e24643a905f6046675a3f76d8bea89c9eb11e8e601c2adbb",
    limit: 64 * 1024 * 1024,
};

/// How long each wait is. Held as values so the tests can run the same
/// supervisor in milliseconds.
#[derive(Clone, Copy, Debug)]
struct Timing {
    /// How often the supervisor looks for a reason to stop.
    poll: Duration,
    /// How often a running tunnel is asked how it is.
    refresh: Duration,
    /// How long the daemon is given to open its socket.
    socket: Duration,
    /// How long the tailnet is given to answer at all, before any sign-in
    /// link has appeared. Past this the reader has Wi-Fi and no way out of
    /// it, and saying so beats ten minutes of "Starting".
    connect: Duration,
    /// How long the owner has to approve the reader from a browser.
    sign_in: Duration,
    /// How long the daemon is given to leave after being asked.
    stop: Duration,
    /// The longest one question to the daemon may take.
    answer: Duration,
}

const DEVICE_TIMING: Timing = Timing {
    poll: Duration::from_secs(2),
    refresh: Duration::from_secs(60),
    socket: Duration::from_secs(20),
    connect: Duration::from_secs(45),
    sign_in: Duration::from_secs(10 * 60),
    stop: Duration::from_secs(10),
    answer: Duration::from_secs(15),
};

/// Everything outside this process the supervisor reads or changes, as paths.
///
/// One value rather than constants at each use, so the tests can stand the
/// whole thing up in a scratch folder with two shell scripts in place of the
/// daemon and its command, and run every failure the real ones can produce
/// without a reader, a tailnet or root.
#[derive(Clone, Debug)]
struct Layout {
    app_state: PathBuf,
    home: PathBuf,
    bin: PathBuf,
    hosts: PathBuf,
    tun_node: PathBuf,
    /// `/proc`, for who is listening and for whether a process is ours.
    proc: PathBuf,
    /// `/sys/class/net`, for loopback and for the tunnel's interface.
    interfaces: PathBuf,
    sshd: PathBuf,
    sshd_configs: [PathBuf; 2],
    engine: Engine,
    /// Free room the book partition must have before a download.
    room: u64,
    timing: Timing,
}

impl Layout {
    fn device() -> Self {
        Self {
            app_state: PathBuf::from(APP_STATE),
            home: PathBuf::from(HOME),
            bin: PathBuf::from(BIN_DIR),
            hosts: PathBuf::from("/etc/hosts"),
            tun_node: PathBuf::from("/dev/net/tun"),
            proc: PathBuf::from("/proc"),
            interfaces: PathBuf::from("/sys/class/net"),
            sshd: PathBuf::from("/usr/sbin/sshd"),
            sshd_configs: [
                PathBuf::from("/etc/ssh/sshd_config"),
                PathBuf::from("/etc/sshd_config"),
            ],
            engine: PINNED,
            room: ROOM_NEEDED,
            timing: DEVICE_TIMING,
        }
    }

    fn daemon(&self) -> PathBuf {
        self.bin.join("tailscaled")
    }

    fn cli(&self) -> PathBuf {
        self.bin.join("tailscale")
    }

    fn socket(&self) -> PathBuf {
        self.home.join("tailscaled.sock")
    }

    /// A question or instruction for this daemon and no other on the reader.
    fn ask(&self) -> Command {
        let mut command = Command::new(self.cli());
        command.arg("--socket").arg(self.socket());
        command
    }
}

/// What the owner chose. A missing or unreadable choice is off, because the
/// only safe reading of "nobody said" for a service that opens the reader to
/// a network is that nobody asked for it.
fn enabled(layout: &Layout) -> bool {
    fs::read_to_string(layout.app_state.join(CONFIG))
        .is_ok_and(|text| text.lines().next() == Some("true"))
}

fn sign_out_requested(layout: &Layout) -> bool {
    fs::read_to_string(layout.app_state.join(REQUEST)).is_ok_and(|text| text.trim() == SIGN_OUT)
}

/// The app-facing status, one field a line in the order the app reads them.
/// The first four lines are the original contract; the name was added after,
/// and an app that predates it ignores the line.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Status {
    state: &'static str,
    address: String,
    message: String,
    sign_in: String,
    name: String,
}

impl Status {
    fn quiet(state: &'static str, message: impl Into<String>) -> Self {
        Self {
            state,
            message: message.into(),
            ..Self::default()
        }
    }

    fn render(&self) -> String {
        format!(
            "{}\n{}\n{}\n{}\n{}\n",
            self.state, self.address, self.message, self.sign_in, self.name
        )
    }
}

const OFF: &str = "Tailscale is off.";
const WAITING_FOR_WIFI: &str =
    "Waiting for Wi-Fi. Tailscale starts by itself when the reader is online.";
const LEFT_COBALT: &str =
    "Tailscale stopped when you left Cobalt. It starts again the next time you open Cobalt.";
const SIGNED_OUT: &str =
    "Signed out. If this reader is still listed in your Tailscale admin console, remove it there.";

fn read_status(layout: &Layout) -> String {
    fs::read_to_string(layout.app_state.join(STATUS))
        .unwrap_or_else(|_| Status::quiet("off", "Tailscale has not run.").render())
}

/// Says what happened, once.
///
/// The same words are not written twice, because the file is on flash and the
/// supervisor reaches the same conclusion every minute of a quiet evening.
fn write_status(layout: &Layout, status: &Status) -> Result<(), String> {
    let text = status.render();
    let path = layout.app_state.join(STATUS);
    if fs::read_to_string(&path).is_ok_and(|current| current == text) {
        return Ok(());
    }
    fs::create_dir_all(&layout.app_state)
        .map_err(|error| format!("create Tailscale app state: {error}"))?;
    atomic_write(&path, text.as_bytes(), 0o644)
}

fn atomic_write(path: &Path, value: &[u8], mode: u32) -> Result<(), String> {
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
    file.write_all(value)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("write {}: {error}", temporary.display()))?;
    fs::rename(&temporary, path).map_err(|error| format!("replace {}: {error}", path.display()))
}

/// `kobod --tailscale run|status|stop|sign-out`.
///
/// # Errors
///
/// `run` fails when the service could not be brought up for a reason that
/// may pass by itself, which is how the runtime knows to try again later. A
/// run that ended because the owner has something to do first succeeds: the
/// status says what, and trying again without them would change nothing.
pub fn command(arguments: &[String]) -> Result<(), String> {
    let layout = Layout::device();
    match arguments {
        [action] if action == "run" => match run(&layout) {
            End::Settled => Ok(()),
            End::Later(why) => Err(why),
        },
        [action] if action == "status" => {
            print!("{}", read_status(&layout));
            Ok(())
        }
        [action] if action == "stop" => {
            ask_to_stop_in(&layout).map_err(|error| format!("ask Tailscale to stop: {error}"))?;
            wait_for_supervisor_to_leave(&layout);
            sweep_in(&layout);
            forget_stop_request_in(&layout);
            Ok(())
        }
        [action] if action == "sign-out" => sign_out_while_off(&layout),
        _ => Err("usage: kobod --tailscale run|status|stop|sign-out".to_owned()),
    }
}

/// What the runtime should start now, if anything.
///
/// Only the device runtime starts the service; a host build of `kobod`
/// carries this module for its `--tailscale` commands and never asks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(not(feature = "device-write"), allow(dead_code))]
pub enum Want {
    /// The owner has Tailscale switched on.
    Run,
    /// The owner asked for this reader's key to be forgotten.
    SignOut,
}

#[cfg_attr(not(feature = "device-write"), allow(dead_code))]
impl Want {
    /// The `kobod` arguments that do this in a process of its own.
    ///
    /// A process rather than a thread for the reason a Sync window is one:
    /// the supervisor has to be something the session can end as a group,
    /// daemon included, when the reader is handed back.
    pub const fn arguments(self) -> [&'static str; 2] {
        match self {
            Self::Run => ["--tailscale", "run"],
            Self::SignOut => ["--tailscale", "sign-out"],
        }
    }
}

/// Whether the service should be started, read from what the app saved.
#[cfg_attr(not(feature = "device-write"), allow(dead_code))]
pub fn wanted() -> Option<Want> {
    wanted_in(&Layout::device())
}

fn wanted_in(layout: &Layout) -> Option<Want> {
    if sign_out_requested(layout) {
        return Some(Want::SignOut);
    }
    enabled(layout).then_some(Want::Run)
}

/// Whether a store key the Tailscale app just saved may change what should
/// be running, so a tap is answered at once rather than at the next look.
#[cfg_attr(not(feature = "device-write"), allow(dead_code))]
pub fn asks_for_the_service(key: &str) -> bool {
    key == CONFIG || key == REQUEST
}

/// Tells the app why the service it switched on has not started.
#[cfg_attr(not(feature = "device-write"), allow(dead_code))]
pub fn waiting_for_wifi() {
    let _ignored = write_status(
        &Layout::device(),
        &Status::quiet("offline", WAITING_FOR_WIFI),
    );
}

/// Asks the supervisor this session started to stop, because the session is
/// ending.
///
/// # Errors
///
/// When the request cannot be written; the session then ends the supervisor
/// itself once it has waited.
#[cfg_attr(not(feature = "device-write"), allow(dead_code))]
pub fn ask_to_stop() -> std::io::Result<()> {
    ask_to_stop_in(&Layout::device())
}

fn ask_to_stop_in(layout: &Layout) -> std::io::Result<()> {
    fs::create_dir_all(&layout.home)?;
    fs::write(layout.home.join(STOP), b"")
}

/// Clears a request left by a session that ended before its supervisor saw
/// it, so the next one is not stopped by it as it starts.
#[cfg_attr(not(feature = "device-write"), allow(dead_code))]
pub fn forget_stop_request() {
    forget_stop_request_in(&Layout::device());
}

fn forget_stop_request_in(layout: &Layout) {
    let _ignored = fs::remove_file(layout.home.join(STOP));
}

/// Ends whatever an earlier supervisor left behind, and takes the tailnet
/// names out of the hosts file.
///
/// A supervisor stops its own daemon on every path it controls. The ones it
/// does not are a supervisor that was killed outright and a reader that lost
/// power, and after either there may be a daemon with nobody watching it and
/// a block of names for a tunnel that is gone. The session calls this when it
/// starts and again when it ends, so neither outlives the next look.
#[cfg_attr(not(feature = "device-write"), allow(dead_code))]
pub fn sweep() {
    sweep_in(&Layout::device());
}

fn sweep_in(layout: &Layout) {
    stop_recorded_daemon(layout);
    let _ignored = hosts::write(&layout.hosts, &[]);
    let _ignored = fs::remove_file(layout.socket());
    // A status that still says connected after that would be a lie the band
    // repeats on every screen.
    if matches!(
        read_status(layout).lines().next(),
        Some("running" | "starting" | "needs-login" | "downloading")
    ) {
        let _ignored = write_status(layout, &Status::quiet("stopped", LEFT_COBALT));
    }
}

/// Whether the tunnel is up right now, for the mark in the status band.
///
/// Both witnesses are needed. The status file is written by a process that
/// can die without a last word, and the interface exists for a moment before
/// the reader has an address on it. The interface is the kernel's own word
/// and disappears with the daemon, so a daemon that is gone takes the mark
/// with it within one look.
#[cfg_attr(not(feature = "device-write"), allow(dead_code))]
pub fn connected() -> bool {
    connected_in(&Layout::device())
}

fn connected_in(layout: &Layout) -> bool {
    layout.interfaces.join(INTERFACE).exists()
        && read_status(layout).lines().next() == Some("running")
}

/// How a run of the supervisor ended, which is all the runtime needs to know
/// to decide whether to start another.
#[derive(Debug, Eq, PartialEq)]
enum End {
    /// Nothing more to do until the owner does something: switched off,
    /// stopped with the session, or waiting on a step only they can take.
    Settled,
    /// It failed for a reason that may pass, such as a download that did not
    /// finish. Worth another try later.
    Later(String),
}

/// Why a supervisor that was doing fine should stop.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Interrupt {
    /// The session is ending and asked.
    SessionEnding,
    /// The runtime that started this is gone without asking, so nothing is
    /// left to ask later either.
    Orphaned,
    SignOut,
    SwitchedOff,
}

/// One run of the service, from nothing to the tunnel being taken down.
struct Supervisor<'a> {
    layout: &'a Layout,
    /// The process that started this one. A different answer later means it
    /// died and this was adopted, which is the only notice an orphan gets.
    parent: u32,
}

impl Supervisor<'_> {
    fn interrupted(&self) -> Option<Interrupt> {
        if self.layout.home.join(STOP).exists() {
            return Some(Interrupt::SessionEnding);
        }
        if std::os::unix::process::parent_id() != self.parent {
            return Some(Interrupt::Orphaned);
        }
        // Asked before the switch is read: the app saves the request and
        // then switches off, and a sign-out mistaken for a plain off would
        // stop the daemon before it could tell the tailnet the key is dead.
        if sign_out_requested(self.layout) {
            return Some(Interrupt::SignOut);
        }
        (!enabled(self.layout)).then_some(Interrupt::SwitchedOff)
    }

    fn say(&self, status: &Status) {
        if let Err(error) = write_status(self.layout, status) {
            eprintln!("kobod: tailscale status not written: {error}");
        }
    }

    /// Takes the tunnel down and says why it went.
    fn leave(&self, daemon: &mut Daemon, interrupt: Interrupt) -> End {
        if interrupt == Interrupt::SignOut {
            // While the daemon is still up, so the tailnet hears that the key
            // is finished rather than merely seeing the reader go quiet.
            let mut command = self.layout.ask();
            command.arg("logout");
            let _ignored = answer(command, self.layout.timing.answer);
        }
        daemon.stop(self.layout);
        let _ignored = hosts::write(&self.layout.hosts, &[]);
        match interrupt {
            Interrupt::SignOut => {
                forget_node(self.layout);
                self.say(&Status::quiet("signed-out", SIGNED_OUT));
            }
            Interrupt::SwitchedOff => self.say(&Status::quiet("off", OFF)),
            Interrupt::SessionEnding | Interrupt::Orphaned => {
                self.say(&Status::quiet("stopped", LEFT_COBALT));
            }
        }
        forget_stop_request_in(self.layout);
        End::Settled
    }

    /// Takes the tunnel down after something went wrong, and says what.
    fn fail(&self, daemon: &mut Daemon, status: &Status, end: End) -> End {
        daemon.stop(self.layout);
        let _ignored = hosts::write(&self.layout.hosts, &[]);
        self.say(status);
        end
    }

    /// The same, for the failures that may pass by themselves.
    fn fail_for_now(&self, daemon: &mut Daemon, why: &str) -> End {
        self.fail(
            daemon,
            &Status::quiet("failed", why),
            End::Later(why.to_owned()),
        )
    }
}

fn run(layout: &Layout) -> End {
    let supervisor = Supervisor {
        layout,
        parent: std::os::unix::process::parent_id(),
    };
    if !enabled(layout) {
        sweep_in(layout);
        supervisor.say(&Status::quiet("off", OFF));
        return End::Settled;
    }
    if let Err(end) = claim(layout) {
        return end;
    }
    let end = supervise(&supervisor);
    let _ignored = fs::remove_file(layout.home.join(SUPERVISOR_PID));
    end
}

/// Makes this the one supervisor, and clears what the last one left.
fn claim(layout: &Layout) -> Result<(), End> {
    prepare_home(layout).map_err(End::Later)?;
    let pid_file = layout.home.join(SUPERVISOR_PID);
    if let Some(other) = recorded_pid(&pid_file) {
        if other != std::process::id() && process_names(layout, other, "--tailscale") {
            // Two would each take the other's daemon for a leftover.
            return Err(End::Later(
                "another Tailscale supervisor is already running".to_owned(),
            ));
        }
    }
    atomic_write(&pid_file, std::process::id().to_string().as_bytes(), 0o600)
        .map_err(End::Later)?;
    sweep_in(layout);
    forget_stop_request_in(layout);
    Ok(())
}

fn supervise(supervisor: &Supervisor) -> End {
    let layout = supervisor.layout;
    if let Some(refusal) = preflight(layout) {
        supervisor.say(&refusal);
        return End::Settled;
    }
    if !engine_is_installed(layout) {
        supervisor.say(&Status::quiet(
            "downloading",
            "Downloading Tailscale. This takes a few minutes the first time.",
        ));
    }
    if let Err(why) = ensure_engine(layout, &|| {
        kobo_net::fetch(layout.engine.url, layout.engine.limit).ok()
    }) {
        supervisor.say(&Status::quiet("failed", why.clone()));
        return End::Later(why);
    }
    // The download can take most of a short session.
    if let Some(interrupt) = supervisor.interrupted() {
        return supervisor.leave(&mut Daemon::absent(), interrupt);
    }
    supervisor.say(&Status::quiet("starting", "Starting Tailscale."));
    let mut daemon = match Daemon::start(layout) {
        Ok(daemon) => daemon,
        Err(why) => {
            supervisor.say(&Status::quiet("failed", why.clone()));
            return End::Later(why);
        }
    };
    loop {
        if let Err(end) = bring_up(supervisor, &mut daemon) {
            return end;
        }
        // Returning without an end means the tailnet wants the reader
        // approved again, which is the same road as the first time.
        if let Err(end) = keep_up(supervisor, &mut daemon) {
            return end;
        }
    }
}

/// The reasons not to join a tailnet at all, checked before anything is
/// downloaded or started, in words the owner can act on.
fn preflight(layout: &Layout) -> Option<Status> {
    if let Err(why) = ensure_tun(layout) {
        eprintln!("kobod: tailscale tunnel device: {why}");
        return Some(Status::quiet(
            "unsupported",
            "This reader's software has no tunnel support, so Tailscale cannot run on it.",
        ));
    }
    match exposure(layout) {
        Exposure::Acceptable => None,
        Exposure::PasswordLogin => Some(Status::quiet(
            "refused",
            "Root can still log in to this reader with a password. Run kobo setup on your \
             computer to close that, then turn Tailscale on again.",
        )),
        Exposure::DebugServices => Some(Status::quiet(
            "refused",
            "Telnet or FTP is running on this reader and takes no password. Turn off the \
             reader's debug services, restart it, then turn Tailscale on again.",
        )),
    }
}

/// What the reader would be offering a tailnet that it should not.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Exposure {
    Acceptable,
    /// The SSH server is listening and still takes root by password.
    PasswordLogin,
    /// The firmware's debug services are listening.
    DebugServices,
}

/// Looks at what is listening before the reader is put on a tailnet.
///
/// With no packet filter on the reader, joining a tailnet offers every
/// listening port to every computer on it, and a tailnet is not only the
/// owner's laptop: it is every device signed in to it and anything shared
/// into it. The SSH server is the point of the exercise and is fine once it
/// takes keys only, which `kobo setup` arranges. The stock firmware's
/// defaults are not fine. Its SSH server accepts root with a password that
/// is published or empty, and its debug services are telnet and FTP as root
/// with no password at all. On the owner's own Wi-Fi those were a risk they
/// had already taken. Carried onto a tailnet by a switch in an app, they are
/// one nobody chose, so the switch does nothing until they are closed.
fn exposure(layout: &Layout) -> Exposure {
    let ports = listening_ports(layout);
    if ports.contains(&21) || ports.contains(&23) {
        return Exposure::DebugServices;
    }
    if ports.contains(&22) && !password_login_closed(layout) {
        return Exposure::PasswordLogin;
    }
    Exposure::Acceptable
}

/// The TCP ports something is listening on, on an address other machines
/// can reach.
fn listening_ports(layout: &Layout) -> Vec<u16> {
    ["net/tcp", "net/tcp6"]
        .iter()
        .filter_map(|table| fs::read_to_string(layout.proc.join(table)).ok())
        .flat_map(|table| parse_listeners(&table))
        .collect()
}

/// Reads the kernel's table of TCP sockets for the listening ones.
///
/// The local address is the second column, hexadecimal address and port, and
/// the state is the fourth, where `0A` is listening. A listener on loopback
/// is left out, because a tunnel made by the kernel does not carry it.
fn parse_listeners(table: &str) -> Vec<u16> {
    const LISTENING: &str = "0A";
    table
        .lines()
        .skip(1)
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let local = fields.nth(1)?;
            let state = fields.nth(1)?;
            let (address, port) = local.rsplit_once(':')?;
            let loopback = matches!(address, "0100007F" | "00000000000000000000000001000000");
            (state == LISTENING && !loopback)
                .then(|| u16::from_str_radix(port, 16).ok())
                .flatten()
        })
        .collect()
}

/// Whether the SSH server refuses root a password.
///
/// The server is asked for the configuration it would apply to root, which
/// is the only reading that accounts for every file and every `Match` block
/// it honours. Where it cannot be asked, the block `kobo setup` writes is
/// looked for instead, and one still waiting for its confirming login does
/// not count: until that login it is a change that will undo itself.
fn password_login_closed(layout: &Layout) -> bool {
    let mut command = Command::new(&layout.sshd);
    command.args(["-T", "-C", "user=root,host=reader,addr=127.0.0.1"]);
    if let Ok(said) = answer(command, layout.timing.answer) {
        if let Some(setting) = said
            .lines()
            .find_map(|line| line.strip_prefix("passwordauthentication "))
        {
            return setting.trim() == "no";
        }
    }
    layout.sshd_configs.iter().any(|config| {
        let mut pending = config.as_os_str().to_owned();
        pending.push(".cobalt-revert");
        fs::read_to_string(config).is_ok_and(|text| {
            text.lines()
                .any(|line| line.starts_with("# cobalt-hardening:"))
        }) && !Path::new(&pending).exists()
    })
}

/// Makes the tunnel's device node and proves a driver answers behind it.
///
/// The node alone proves nothing, since anybody can make one. Opening it is
/// what asks the kernel, and a kernel built without the driver refuses. No
/// tunnel is created by the open: that takes a second request this never
/// sends, so the question costs nothing and leaves nothing behind.
fn ensure_tun(layout: &Layout) -> Result<(), String> {
    let node = &layout.tun_node;
    if !node.exists() {
        if let Some(folder) = node.parent() {
            fs::create_dir_all(folder).map_err(|error| format!("create /dev/net: {error}"))?;
        }
        let made = Command::new("mknod")
            .arg(node)
            .args(["c", "10", "200"])
            .stdin(Stdio::null())
            .output()
            .map_err(|error| format!("make the tunnel device node: {error}"))?;
        if !made.status.success() {
            return Err(format!(
                "make the tunnel device node: {}",
                String::from_utf8_lossy(&made.stderr).trim()
            ));
        }
    }
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(node)
        .map(drop)
        .map_err(|error| format!("the kernel has no tunnel driver: {error}"))
}

/// Brings loopback up where the firmware left it down.
///
/// Asked through the interface's flags. Its `operstate` reads `unknown` on a
/// loopback that is working perfectly, so a check for `up` there never
/// passes and the answer was being ignored on every start.
fn ensure_loopback(layout: &Layout) {
    let flags = fs::read_to_string(layout.interfaces.join("lo/flags")).unwrap_or_default();
    let up = u32::from_str_radix(flags.trim().trim_start_matches("0x"), 16)
        .is_ok_and(|flags| flags & 1 == 1);
    if up {
        return;
    }
    for arguments in [
        &["ifconfig", "lo", "up"][..],
        &["ip", "link", "set", "lo", "up"],
    ] {
        let raised = Command::new(arguments[0])
            .args(&arguments[1..])
            .stdin(Stdio::null())
            .output()
            .is_ok_and(|output| output.status.success());
        if raised {
            return;
        }
    }
}

fn prepare_home(layout: &Layout) -> Result<(), String> {
    fs::create_dir_all(&layout.home).map_err(|error| format!("create Tailscale home: {error}"))?;
    fs::set_permissions(&layout.home, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("protect Tailscale home: {error}"))
}

/// Deletes the node key and everything else the daemon kept.
///
/// The only way this reader stops being a member of the tailnet from its own
/// side. Uninstalling Cobalt cannot do it, because that happens over USB and
/// the key is on the root filesystem, which USB does not reach.
fn forget_node(layout: &Layout) {
    let _ignored = fs::remove_dir_all(&layout.home);
    let _ignored = fs::remove_file(layout.app_state.join(REQUEST));
}

/// Sign-out asked for while the service is off: there is no daemon to tell
/// the tailnet, so the key is destroyed here and the owner is told where the
/// reader's entry may still be listed.
fn sign_out_while_off(layout: &Layout) -> Result<(), String> {
    sweep_in(layout);
    forget_node(layout);
    write_status(layout, &Status::quiet("signed-out", SIGNED_OUT))
}

fn wait_for_supervisor_to_leave(layout: &Layout) {
    let pid_file = layout.home.join(SUPERVISOR_PID);
    let deadline = Instant::now() + layout.timing.stop + layout.timing.poll * 2;
    while pid_file.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn recorded_pid(path: &Path) -> Option<u32> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// Whether process `pid` was started with `word` among its arguments.
///
/// A recorded number is only a number: after a restart it may belong to the
/// stock reader. So before anything is signalled, the kernel is asked what
/// that process was started as, and a process that is not ours is left alone.
fn process_names(layout: &Layout, pid: u32, word: &str) -> bool {
    fs::read(layout.proc.join(pid.to_string()).join("cmdline")).is_ok_and(|line| {
        line.split(|byte| *byte == 0)
            .any(|argument| argument == word.as_bytes())
    })
}

/// Stops the daemon an earlier supervisor recorded, if it is still there and
/// still the daemon.
fn stop_recorded_daemon(layout: &Layout) {
    let pid_file = layout.home.join(DAEMON_PID);
    let Some(pid) = recorded_pid(&pid_file) else {
        return;
    };
    let daemon = layout.daemon();
    let ours = |pid| process_names(layout, pid, &daemon.to_string_lossy());
    if ours(pid) {
        let _ignored = kobo_abi::process_group::signal(pid, kobo_abi::process_group::SIGTERM);
        let deadline = Instant::now() + layout.timing.stop;
        while ours(pid) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
        if ours(pid) {
            let _ignored = kobo_abi::process_group::signal(pid, kobo_abi::process_group::SIGKILL);
        }
    }
    let _ignored = fs::remove_file(pid_file);
}

/// The running daemon and the last things it said.
struct Daemon {
    child: Option<Child>,
    log: Arc<Mutex<VecDeque<String>>>,
    listener: Option<std::thread::JoinHandle<()>>,
}

impl Daemon {
    /// No daemon, for the paths that leave before one was started.
    fn absent() -> Self {
        Self {
            child: None,
            log: Arc::default(),
            listener: None,
        }
    }

    /// Starts the daemon and waits for it to open its socket.
    ///
    /// Three settings are for this device in particular. The memory limit and
    /// the collector's pace keep the daemon's heap small on a reader with half
    /// a gigabyte shared with the stock reader. Log upload is switched off,
    /// because it buffers to files beside the node key and sends them over the
    /// radio, and a reader has neither the flash nor the battery to spend on a
    /// support channel its owner never opened. File transfer is switched off
    /// because a file sent to the reader lands on the root filesystem, where
    /// nothing would ever collect it and a large one would fill the partition
    /// the firmware updates itself onto.
    fn start(layout: &Layout) -> Result<Self, String> {
        verify_engine(layout)?;
        prepare_home(layout)?;
        ensure_loopback(layout);
        let _ignored = fs::remove_dir_all(layout.home.join("files"));
        let _ignored = fs::remove_file(layout.socket());
        let mut command = Command::new(layout.daemon());
        command
            .arg("--statedir")
            .arg(&layout.home)
            .arg("--socket")
            .arg(layout.socket())
            .arg(format!("--tun={INTERFACE}"))
            .arg("--no-logs-no-support")
            .env("GOMEMLIMIT", "48MiB")
            .env("GOGC", "50")
            .env("TS_DISABLE_TAILDROP", "true")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        // A group of its own, so it can be asked to leave by signal without
        // the supervisor asking itself to leave as well.
        kobo_abi::process_group::configure(&mut command);
        let mut child = command
            .spawn()
            .map_err(|error| format!("Tailscale could not be started: {error}"))?;
        let log = Arc::default();
        let listener = child
            .stderr
            .take()
            .map(|stderr| keep_last_lines(stderr, Arc::clone(&log)));
        let pid = child.id();
        let mut daemon = Self {
            child: Some(child),
            log,
            listener,
        };
        if let Err(why) = atomic_write(
            &layout.home.join(DAEMON_PID),
            pid.to_string().as_bytes(),
            0o600,
        ) {
            daemon.stop(layout);
            return Err(why);
        }
        // If memory runs out, the kernel should take this before it takes the
        // stock reader or the runtime drawing the screen. Losing the tunnel is
        // a reconnect; losing either of those is the owner's evening.
        let _ignored = fs::write(
            layout.proc.join(pid.to_string()).join("oom_score_adj"),
            b"800",
        );
        let deadline = Instant::now() + layout.timing.socket;
        while !layout.socket().exists() {
            if daemon.exited() || Instant::now() >= deadline {
                daemon.stop(layout);
                return Err(
                    "Tailscale did not start. The reason is in the Tailscale log on the reader."
                        .to_owned(),
                );
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Ok(daemon)
    }

    fn exited(&mut self) -> bool {
        self.child
            .as_mut()
            .is_none_or(|child| !matches!(child.try_wait(), Ok(None)))
    }

    /// Asks the daemon to leave, makes it leave if it will not, and keeps
    /// what it last said.
    ///
    /// Asked first, because a daemon that is asked takes its interface and
    /// its routes with it and tells its peers it is going. One that has not
    /// gone in time is ended anyway: a wedged daemon must not keep the radio
    /// and the reader's hand-back waiting.
    fn stop(&mut self, layout: &Layout) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        if matches!(child.try_wait(), Ok(None)) {
            let _ignored =
                kobo_abi::process_group::signal(child.id(), kobo_abi::process_group::SIGTERM);
            let deadline = Instant::now() + layout.timing.stop;
            while matches!(child.try_wait(), Ok(None)) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(50));
            }
            if matches!(child.try_wait(), Ok(None)) {
                let _ignored = child.kill();
            }
        }
        let _ignored = child.wait();
        let _ignored = fs::remove_file(layout.home.join(DAEMON_PID));
        let _ignored = fs::remove_file(layout.socket());
        // The last thing a dying daemon says is the line worth having, and
        // it is still in the pipe for a moment after the daemon is gone.
        // Waited for briefly rather than joined, so nothing that inherited
        // the pipe can hold the hand-back up.
        if let Some(listener) = self.listener.take() {
            let deadline = Instant::now() + Duration::from_millis(500);
            while !listener.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        let lines = self.log.lock().map_or_else(
            |_| String::new(),
            |log| {
                log.iter().fold(String::new(), |mut text, line| {
                    let _ignored = writeln!(text, "{line}");
                    text
                })
            },
        );
        if layout.home.is_dir() {
            let _ignored = atomic_write(&layout.home.join(DAEMON_LOG), lines.as_bytes(), 0o600);
        }
    }
}

/// Keeps the last lines a child wrote, and no more than that.
///
/// The daemon is talkative and a session can last hours, so its output goes
/// neither to a file that grows nor to nowhere: the end of it is what a
/// report needs, and the end is all that is held.
fn keep_last_lines<R: Read + Send + 'static>(
    reader: R,
    log: Arc<Mutex<VecDeque<String>>>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        for line in BufReader::new(reader).lines().map_while(Result::ok) {
            let Ok(mut log) = log.lock() else {
                break;
            };
            if log.len() == LOG_LINES {
                log.pop_front();
            }
            log.push_back(line);
        }
    })
}

/// What the daemon says about itself.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Report {
    backend: Backend,
    sign_in: Option<String>,
    address: String,
    name: String,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Backend {
    Running,
    NeedsLogin,
    /// Signed in, and waiting for somebody who runs the tailnet to let the
    /// device on. There is no link for the owner to follow for this one.
    NeedsApproval,
    #[default]
    Other,
}

fn parse_report(json: &str) -> Option<Report> {
    let value = kobo_json::parse(json).ok()?;
    let text = |value: Option<&kobo_json::Value>| {
        value
            .and_then(kobo_json::Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let own = value.get("Self");
    Some(Report {
        backend: match value.get("BackendState").and_then(kobo_json::Value::as_str) {
            Some("Running") => Backend::Running,
            Some("NeedsLogin") => Backend::NeedsLogin,
            Some("NeedsMachineAuth") => Backend::NeedsApproval,
            _ => Backend::Other,
        },
        sign_in: sign_in_link(&text(value.get("AuthURL"))).map(str::to_owned),
        address: text(own.and_then(|own| own.get("TailscaleIPs")?.index(0))),
        name: text(own.and_then(|own| own.get("DNSName")))
            .trim_end_matches('.')
            .to_owned(),
    })
}

/// The sign-in link in a line of output, when there is one.
///
/// Only an address on Tailscale's own sign-in host is accepted. Whatever is
/// found is drawn on the reader as a code for the owner's phone to open, and
/// that must never be an address of anybody else's choosing.
fn sign_in_link(output: &str) -> Option<&str> {
    output
        .split_whitespace()
        .find(|word| word.starts_with("https://login.tailscale.com/"))
        .map(|word| word.trim_end_matches(['.', ',']))
        .filter(|link| {
            link.bytes()
                .all(|byte| byte.is_ascii_graphic() && byte != b'"')
        })
}

/// Every computer on the tailnet with a name, this reader included.
fn parse_nodes(json: &str) -> Vec<hosts::Node> {
    let Ok(value) = kobo_json::parse(json) else {
        return Vec::new();
    };
    let node = |node: &kobo_json::Value| {
        Some(hosts::Node {
            address: node.get("TailscaleIPs")?.index(0)?.as_str()?.to_owned(),
            name: node.get("DNSName")?.as_str()?.to_owned(),
        })
    };
    let mut nodes = value
        .get("Self")
        .and_then(node)
        .into_iter()
        .collect::<Vec<_>>();
    if let Some(kobo_json::Value::Object(peers)) = value.get("Peer") {
        nodes.extend(peers.iter().filter_map(|(_, peer)| node(peer)));
    }
    nodes
}

/// Runs one command to the end and returns what it printed, or gives up.
///
/// Every question to the daemon goes through here, because a daemon that has
/// stopped answering is exactly when the supervisor most needs to keep its
/// own appointments: the session asking it to stop, the owner switching off.
fn answer(mut command: Command, limit: Duration) -> Result<String, String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("could not be run: {error}"))?;
    let (sender, receiver) = mpsc::channel();
    if let Some(stdout) = child.stdout.take() {
        std::thread::spawn(move || {
            let mut text = Vec::new();
            let read = stdout.take(LARGEST_ANSWER).read_to_end(&mut text);
            let _ignored = sender.send(read.map(|_| text));
        });
    }
    let deadline = Instant::now() + limit;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                let _ignored = child.kill();
                let _ignored = child.wait();
                return Err("did not answer in time".to_owned());
            }
        }
    };
    let text = receiver
        .recv_timeout(limit)
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();
    if !status.success() {
        return Err("answered with a failure".to_owned());
    }
    Ok(String::from_utf8_lossy(&text).into_owned())
}

fn report(layout: &Layout) -> Option<Report> {
    let mut command = layout.ask();
    command.args(["status", "--json", "--peers=false"]);
    parse_report(&answer(command, layout.timing.answer).ok()?)
}

fn sync_hosts(layout: &Layout) {
    let mut command = layout.ask();
    command.args(["status", "--json"]);
    let Ok(json) = answer(command, layout.timing.answer) else {
        return;
    };
    if let Err(error) = hosts::write(&layout.hosts, &hosts::block(&parse_nodes(&json))) {
        eprintln!("kobod: tailnet names not written: {error}");
    }
}

/// The one way this reader is ever brought up.
///
/// `--reset` first, because settings persist in the daemon's state and this
/// command otherwise refuses to run when one it was not told about is set.
/// Somebody with a shell could have enabled Tailscale SSH or advertised a
/// route by hand, and without the reset that choice would outlive them and
/// take the service down with an error nobody on the reader could read. Each
/// thing a leaf must not do is then named as well, so that what this reader
/// is does not depend on a default staying what it is today.
fn up_command(layout: &Layout) -> Command {
    let mut command = layout.ask();
    command
        .arg("up")
        .arg("--reset")
        .arg("--accept-dns=false")
        .arg("--accept-routes=false")
        .arg("--ssh=false")
        .arg("--advertise-exit-node=false")
        .arg("--advertise-routes=")
        .arg("--exit-node=")
        .arg("--shields-up=false")
        .arg("--netfilter-mode=off")
        .arg(format!("--hostname={HOSTNAME}"))
        .arg(format!(
            "--timeout={}s",
            layout.timing.sign_in.as_secs().max(1)
        ));
    command
}

fn forward_lines<R: Read + Send + 'static>(reader: R, sender: mpsc::Sender<String>) {
    std::thread::spawn(move || {
        for line in BufReader::new(reader).lines().map_while(Result::ok) {
            if sender.send(line).is_err() {
                break;
            }
        }
    });
}

/// Joins the tailnet, showing the owner the sign-in link if one is needed,
/// and returns once the tunnel is up.
///
/// The command that does this waits for the owner, so it is run beside the
/// supervisor rather than by it: the link has to reach the screen the moment
/// it is printed, and a session that ends while somebody is finding their
/// phone still has to be answered within a couple of seconds.
fn bring_up(supervisor: &Supervisor, daemon: &mut Daemon) -> Result<(), End> {
    let layout = supervisor.layout;
    let mut child = match up_command(layout)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            return Err(supervisor.fail_for_now(
                daemon,
                &format!("Tailscale could not be brought up: {error}"),
            ));
        }
    };
    let (sender, lines) = mpsc::channel();
    if let Some(stdout) = child.stdout.take() {
        forward_lines(stdout, sender.clone());
    }
    if let Some(stderr) = child.stderr.take() {
        forward_lines(stderr, sender);
    }
    let started = Instant::now();
    let mut announced = false;
    let finished = loop {
        announced |= announce_sign_in(supervisor, &lines, layout.timing.poll);
        let interrupt = supervisor.interrupted();
        let gone = daemon.exited();
        let patience = if announced {
            layout.timing.sign_in + layout.timing.answer
        } else {
            layout.timing.connect
        };
        let ended = match child.try_wait() {
            Ok(Some(status)) => Some(status.success()),
            Ok(None) if started.elapsed() < patience && interrupt.is_none() && !gone => None,
            _ => {
                let _ignored = child.kill();
                let _ignored = child.wait();
                Some(false)
            }
        };
        if let Some(interrupt) = interrupt {
            return Err(supervisor.leave(daemon, interrupt));
        }
        if let Some(succeeded) = ended {
            break succeeded;
        }
    };
    if finished {
        return Ok(());
    }
    Err(explain_failed_start(supervisor, daemon, announced))
}

/// Puts a sign-in link on the screen if the command has printed one, waiting
/// up to `patience` for the next line. Returns whether it did.
fn announce_sign_in(supervisor: &Supervisor, lines: &Receiver<String>, patience: Duration) -> bool {
    let mut announced = false;
    let mut wait = patience;
    loop {
        match lines.recv_timeout(wait) {
            Ok(line) => {
                if let Some(link) = sign_in_link(&line) {
                    supervisor.say(&Status {
                        sign_in: link.to_owned(),
                        ..Status::quiet("needs-login", "Approve this reader to finish.")
                    });
                    announced = true;
                }
                // Whatever else is already waiting is read without a pause.
                wait = Duration::ZERO;
            }
            Err(RecvTimeoutError::Timeout) => return announced,
            Err(RecvTimeoutError::Disconnected) => {
                // Both pipes are closed, so the command is finishing. The
                // caller's loop still gets a pause, or it would spin until
                // the command has been collected.
                std::thread::sleep(patience.min(Duration::from_millis(50)));
                return announced;
            }
        }
    }
}

/// Works out why the tunnel did not come up, which is one of several very
/// different things with different fixes, and ends the run accordingly.
fn explain_failed_start(supervisor: &Supervisor, daemon: &mut Daemon, announced: bool) -> End {
    if daemon.exited() {
        return supervisor
            .fail_for_now(daemon, "Tailscale stopped unexpectedly. It will try again.");
    }
    match report(supervisor.layout).map(|report| report.backend) {
        Some(Backend::NeedsApproval) => supervisor.fail(
            daemon,
            &Status::quiet(
                "needs-approval",
                "Signed in. Somebody who manages your tailnet has to approve this reader. Tap \
                 Try again once they have.",
            ),
            End::Settled,
        ),
        Some(Backend::NeedsLogin) | None if announced => supervisor.fail(
            daemon,
            &Status::quiet(
                "timed-out",
                "Sign-in was not finished in time. Tap Try again for a new code.",
            ),
            End::Settled,
        ),
        _ => supervisor.fail_for_now(
            daemon,
            "Tailscale could not reach its servers. It will try again.",
        ),
    }
}

/// Watches a tunnel that is up until there is a reason to stop watching.
///
/// Returns `Ok` when the tailnet wants the reader signed in again, which the
/// caller answers by bringing it up the way it did the first time.
fn keep_up(supervisor: &Supervisor, daemon: &mut Daemon) -> Result<(), End> {
    let layout = supervisor.layout;
    let mut asked: Option<Instant> = None;
    let mut unanswered = 0_u8;
    loop {
        if let Some(interrupt) = supervisor.interrupted() {
            return Err(supervisor.leave(daemon, interrupt));
        }
        if daemon.exited() {
            return Err(supervisor
                .fail_for_now(daemon, "Tailscale stopped unexpectedly. It will try again."));
        }
        if asked.is_none_or(|at| at.elapsed() >= layout.timing.refresh) {
            asked = Some(Instant::now());
            match report(layout) {
                Some(report) if report.backend == Backend::Running => {
                    unanswered = 0;
                    supervisor.say(&Status {
                        address: report.address,
                        name: report.name,
                        ..Status::quiet("running", "Connected.")
                    });
                    sync_hosts(layout);
                }
                // Between states, as it is for a moment after the radio
                // comes back. Asked again at the next look.
                Some(report) if report.backend == Backend::Other => unanswered = 0,
                Some(_) => return Ok(()),
                None => {
                    unanswered += 1;
                    if unanswered >= 3 {
                        return Err(supervisor.fail_for_now(
                            daemon,
                            "Tailscale stopped answering. It will try again.",
                        ));
                    }
                }
            }
        }
        std::thread::sleep(layout.timing.poll);
    }
}

fn engine_is_installed(layout: &Layout) -> bool {
    verify_engine(layout).is_ok()
}

/// Fetches the two programs the first time somebody turns Tailscale on.
///
/// Arriving the way the Sync engine does, for the same reason: 70 MB that
/// most readers will never run does not belong in every platform update. The
/// archive is checked against its digest before a byte of it is unpacked, and
/// each program against its own before it is given its name, so a download
/// cut short or tampered with never leaves anything at a path the next start
/// would run. Only the two programs are taken out of the archive; the service
/// files that travel with them are for a different kind of computer.
///
/// `fetch` is the download, passed in so the tests can hand over an archive
/// without a network.
fn ensure_engine(layout: &Layout, fetch: &dyn Fn() -> Option<Vec<u8>>) -> Result<(), String> {
    if engine_is_installed(layout) {
        return Ok(());
    }
    // Whatever is there failed its check, so none of it is kept.
    let staging = layout.bin.join("unpack.part");
    let archive = layout.bin.join("tailscale.tgz.part");
    let clear = || {
        let _ignored = fs::remove_dir_all(&staging);
        let _ignored = fs::remove_file(&archive);
    };
    let _ignored = fs::remove_file(layout.daemon());
    let _ignored = fs::remove_file(layout.cli());
    clear();
    fs::create_dir_all(&staging)
        .map_err(|error| format!("create the Tailscale folder: {error}"))?;
    let installed =
        download(layout, fetch, &archive).and_then(|()| unpack(layout, &archive, &staging));
    clear();
    installed.inspect_err(|_| {
        let _ignored = fs::remove_file(layout.daemon());
        let _ignored = fs::remove_file(layout.cli());
    })
}

const NO_ROOM: &str =
    "The reader does not have room for Tailscale. Free 160 MB of storage and try again.";

/// Fetches the archive, checks it, and puts it beside where it will unpack.
fn download(
    layout: &Layout,
    fetch: &dyn Fn() -> Option<Vec<u8>>,
    archive: &Path,
) -> Result<(), String> {
    if kobo_abi::free_space(&layout.bin).is_some_and(|free| free < layout.room) {
        return Err(NO_ROOM.to_owned());
    }
    let bytes = fetch().ok_or_else(|| {
        "Tailscale could not be downloaded. Check Wi-Fi and try again.".to_owned()
    })?;
    if kobo_net::sha256::hex_digest(&bytes) != layout.engine.archive_sha256 {
        return Err("The Tailscale download did not match its checksum.".to_owned());
    }
    fs::write(archive, &bytes).map_err(|error| {
        if error.kind() == std::io::ErrorKind::StorageFull {
            NO_ROOM.to_owned()
        } else {
            format!("The Tailscale download could not be saved: {error}")
        }
    })
}

/// Takes the two programs out of the archive and gives them their names.
///
/// Named members and nothing else, in the plainest spelling the command has,
/// because the reader's own is a small one and the firmware's updater uses
/// exactly this form.
fn unpack(layout: &Layout, archive: &Path, staging: &Path) -> Result<(), String> {
    let folder = layout.engine.folder;
    let unpacked = Command::new("tar")
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(staging)
        .arg(format!("{folder}/tailscaled"))
        .arg(format!("{folder}/tailscale"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("Tailscale could not be unpacked: {error}"))?;
    if !unpacked.status.success() {
        let said = String::from_utf8_lossy(&unpacked.stderr);
        return Err(if said.contains("No space left") {
            NO_ROOM.to_owned()
        } else {
            format!("Tailscale could not be unpacked: {}", said.trim())
        });
    }
    let members = [
        ("tailscaled", layout.daemon(), layout.engine.daemon_sha256),
        ("tailscale", layout.cli(), layout.engine.cli_sha256),
    ];
    for (name, _, digest) in &members {
        if sha256(&staging.join(folder).join(name))? != *digest {
            return Err("A Tailscale program did not match its checksum.".to_owned());
        }
    }
    for (name, installed, _) in &members {
        let unpacked = staging.join(folder).join(name);
        fs::set_permissions(&unpacked, fs::Permissions::from_mode(0o755))
            .and_then(|()| fs::rename(&unpacked, installed))
            .map_err(|error| format!("Tailscale could not be installed: {error}"))?;
    }
    Ok(())
}

/// Refuses programs this build did not pin by digest.
///
/// Run before every start of the daemon, not only after the download. The
/// programs sit on the partition any computer can write over USB, and the
/// daemon runs as root, so the bytes about to be run are compared with the
/// bytes that were reviewed each time, not trusted because they were once.
fn verify_engine(layout: &Layout) -> Result<(), String> {
    for (program, digest) in [
        (layout.daemon(), layout.engine.daemon_sha256),
        (layout.cli(), layout.engine.cli_sha256),
    ] {
        let kind = fs::symlink_metadata(&program)
            .map_err(|_| "Tailscale is not installed yet.".to_owned())?
            .file_type();
        if !kind.is_file() || sha256(&program)? != digest {
            return Err(
                "A Tailscale program on the reader is not the one that was checked. It will be \
                 downloaded again."
                    .to_owned(),
            );
        }
    }
    Ok(())
}

fn sha256(path: &Path) -> Result<String, String> {
    let file = File::open(path).map_err(|error| format!("read a Tailscale program: {error}"))?;
    let mut reader = BufReader::new(file);
    let mut hash = ring::digest::Context::new(&ring::digest::SHA256);
    let mut bytes = vec![0_u8; 64 * 1024];
    loop {
        let count = reader
            .read(&mut bytes)
            .map_err(|error| format!("read a Tailscale program: {error}"))?;
        if count == 0 {
            break;
        }
        hash.update(&bytes[..count]);
    }
    Ok(hash
        .finish()
        .as_ref()
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            let _ignored = write!(hex, "{byte:02x}");
            hex
        }))
}

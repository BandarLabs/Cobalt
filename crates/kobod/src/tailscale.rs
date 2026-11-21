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
//! Two things beyond that are the owner's to switch on, each off until they
//! do: offering the reader as an exit node, and sharing the network its Wi-Fi
//! is on. Neither is done the way a Linux router does it. That way needs the
//! kernel to forward and translate packets, which needs a packet filter the
//! reader's firmware gives every sign of not having, and leaves rules and
//! settings in the firmware's kernel behind a daemon that is killed. Instead
//! the daemon is asked to carry that traffic itself, in its own process,
//! while the reader's own address stays on the kernel's tunnel. That last
//! part is the point of doing it this way and not with the daemon's fully
//! userspace mode, which hands every connection for the reader to loopback
//! and so offers the tailnet the ports this runtime keeps there. See
//! [`Routing`].
//!
//! Unless the owner asks otherwise, the service runs while Cobalt is open and
//! stops when the reader is handed back, for the reasons a Sync window does.
//! Nothing would be watching the daemon under the stock reader, and its
//! programs live on the partition the reader exports over USB, so a daemon
//! still running from it when the owner plugs in a cable is running from a
//! filesystem a computer is rewriting.
//!
//! An owner who wants the reader reachable after leaving Cobalt can switch
//! that on, and then both objections are answered rather than ignored. The
//! programs, and a copy of this runtime to supervise them, are put on the
//! root filesystem, which is never exported. That supervisor is started
//! detached the way the recovery watchdog is, so it outlives the session,
//! and it is the same supervisor: it keeps checking what the reader offers
//! the tailnet, takes the names out of the hosts file when it stops, and
//! treats the card being away, while a computer has it, as nothing having
//! been said. Nothing starts it at boot. After a restart it comes back the
//! next time Cobalt is opened.
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
//! * A check is only as good as the place checked. A program verified on
//!   the card and then run from the card can be a different program by the
//!   time it runs, to anything able to write the card in between. So where
//!   the root filesystem has the room, the programs are copied there,
//!   verified there, and run from there, and the card's copy is only where
//!   they are copied from. See [`place`] for the reader without the room.
//! * `/dev` is rebuilt at every boot, so the tunnel's device node is made on
//!   every start rather than once at install.
//! * The reader ships no packet filter tools, so the daemon is told to leave
//!   the firewall alone. Every port the reader listens on is therefore
//!   reachable from the tailnet, which is why [`exposure`] is asked before
//!   the reader joins one.
//!
//! No reader has run this yet. The evidence bar in docs/DEVICES.md applies
//! before any of it is called tested.

mod door;
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
///
/// Under `/usr/local`, beside the stock reader's own folder, and not under
/// `/var/lib`, where this began. The firmware mounts a RAM disk of sixteen
/// kilobytes over `/var/lib` at every start (seen on a Clara BW on
/// 4.46.23852), so a key kept there was gone at the next restart, leaving the
/// reader to be approved again and a dead entry behind in the tailnet, and
/// the daemon's state did not fit in it to begin with. A folder here is on
/// the flash, is not exported over USB, and is left alone by a firmware
/// update, which only writes the files it carries.
const HOME: &str = "/usr/local/cobalt/tailscale";
/// The two programs, on the book partition beside the Sync engine.
const BIN_DIR: &str = "/mnt/onboard/.adds/cobalt/bin/tailscale";
/// The two programs again, with a copy of this runtime, on the root
/// filesystem, for a reader that stays connected after Cobalt hands the
/// screen back. Beside the daemon's home rather than inside it, so signing
/// out, which empties the home, does not also mean downloading 70 MB again.
const KEPT_BIN_DIR: &str = "/usr/local/cobalt/tailscale-bin";
/// The partition the stock reader hands to a computer over USB.
const CARD: &str = "/mnt/onboard";

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
/// Left in the daemon's home when staying connected could not be arranged,
/// holding the sentence that says why, so the service runs with the session
/// instead of failing to start in a loop.
const CANNOT_STAY: &str = "cannot-stay";
/// The copy of this runtime that supervises a reader kept connected, and
/// the note beside it of which build it was copied from.
const KEPT_RUNTIME: &str = "kobod";
const KEPT_RUNTIME_FROM: &str = "kobod.from";

/// The most a status answer may be. A tailnet large enough to pass this is
/// not one whose names this reader was going to list.
const LARGEST_ANSWER: u64 = 2 * 1024 * 1024;

/// Room the book partition must have before the download starts: the archive
/// and both programs are on it at once for a moment, with a margin, because
/// the stock reader's library database lives on the same partition and a card
/// filled by a feature most owners never open is a library that will not save.
const ROOM_NEEDED: u64 = 160 * 1024 * 1024;

/// How much of the archive is fetched, and so held in memory, at once.
///
/// The archive is 37 MB and the reader has half a gigabyte for everything.
/// Fetched whole, it sat in memory for the length of the download and again
/// while it was written out. Each piece costs a connection of its own, so
/// the size is a trade: at four megabytes the download is ten requests and
/// never more than one of them is in memory.
const PIECE: u32 = 4 * 1024 * 1024;

/// Room the root filesystem must have before the programs are put on it.
///
/// They are 70 MB, and the margin on top is not politeness: the firmware
/// unpacks its own updates onto this partition, and a reader that cannot
/// update because a feature filled its system storage has been broken by it.
const KEPT_ROOM: u64 = 110 * 1024 * 1024;

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
    /// Where this supervisor's programs are: the card's copy for one that
    /// ends with its session, the root filesystem's for one that is kept.
    bin: PathBuf,
    card_bin: PathBuf,
    kept_bin: PathBuf,
    /// Where the book partition is mounted, when it is.
    card: PathBuf,
    /// Whether this supervisor is meant to outlive the session.
    kept: bool,
    hosts: PathBuf,
    tun_node: PathBuf,
    /// `/proc`, for who is listening and for whether a process is ours.
    proc: PathBuf,
    /// `/sys/class/net`, for loopback and for the tunnel's interface.
    interfaces: PathBuf,
    sshd: PathBuf,
    /// The SSH server's settings file, which [`door::close`] adds a block to.
    sshd_config: PathBuf,
    /// The reader's list of accounts, for where root's home is.
    passwd: PathBuf,
    /// What root's home and the server's key files are found under.
    system: PathBuf,
    /// The Sync engine, whose listening port is its own business and is
    /// allowed for as long as it is that program holding it.
    sync_engine: PathBuf,
    engine: Engine,
    /// Free room the book partition must have before a download, and the
    /// root filesystem before the programs are put on it.
    card_room: u64,
    system_room: u64,
    /// How much of the download is asked for, and held, at a time.
    piece: u32,
    timing: Timing,
}

impl Layout {
    fn device() -> Self {
        Self {
            app_state: PathBuf::from(APP_STATE),
            home: PathBuf::from(HOME),
            bin: PathBuf::from(BIN_DIR),
            card_bin: PathBuf::from(BIN_DIR),
            kept_bin: PathBuf::from(KEPT_BIN_DIR),
            card: PathBuf::from(CARD),
            kept: false,
            hosts: PathBuf::from("/etc/hosts"),
            tun_node: PathBuf::from("/dev/net/tun"),
            proc: PathBuf::from("/proc"),
            interfaces: PathBuf::from("/sys/class/net"),
            sshd: ["/usr/sbin/sshd", "/usr/bin/sshd", "/sbin/sshd"]
                .iter()
                .map(PathBuf::from)
                .find(|path| path.is_file())
                .unwrap_or_else(|| PathBuf::from("/usr/sbin/sshd")),
            // The firmware keeps its server's files in `/etc/ssh`; the bare
            // `/etc` spelling is the other place the server is built to look.
            sshd_config: ["/etc/ssh/sshd_config", "/etc/sshd_config"]
                .iter()
                .map(PathBuf::from)
                .find(|path| path.is_file())
                .unwrap_or_else(|| PathBuf::from("/etc/ssh/sshd_config")),
            passwd: PathBuf::from("/etc/passwd"),
            system: PathBuf::from("/"),
            sync_engine: PathBuf::from(crate::syncthing::ENGINE),
            engine: PINNED,
            card_room: ROOM_NEEDED,
            system_room: KEPT_ROOM,
            piece: PIECE,
            timing: DEVICE_TIMING,
        }
    }

    /// The same reader with its programs on the root filesystem.
    fn on_the_root_filesystem(self) -> Self {
        Self {
            bin: self.kept_bin.clone(),
            ..self
        }
    }

    /// The same reader, for the supervisor that stays after the session.
    fn kept(self) -> Self {
        Self {
            kept: true,
            ..self.on_the_root_filesystem()
        }
    }

    /// Whether this supervisor's programs are somewhere a computer with the
    /// card in its hands cannot write.
    fn off_the_card(&self) -> bool {
        self.bin != self.card_bin
    }

    /// The room that has to be free where the programs are going.
    fn room(&self) -> u64 {
        if self.off_the_card() {
            self.system_room
        } else {
            self.card_room
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

/// The daemon setting that makes it carry routed traffic itself.
///
/// With it, connections a tailnet device sends through this reader, to the
/// internet or to the network the reader is on, are ended inside the daemon
/// and made again from the reader as ordinary connections of its own. That
/// needs nothing from the kernel but the tunnel: no forwarding switch, no
/// packet filter, no address translation. It is how the daemon routes by
/// default on network storage boxes, which are short of the same things.
///
/// It is a setting the daemon documents nowhere and reads once, at start.
/// So this module does not take its word for it: after bringing the reader
/// up it looks for the complaint a daemon that is still relying on the
/// kernel always makes on a reader, and backs out if it finds one. A new pin
/// has to be checked for this name like it is checked for its digests.
const FORWARD_IN_THE_DAEMON: &str = "TS_DEBUG_NETSTACK_SUBNETS";

/// What the reader offers to carry for other devices on the tailnet.
///
/// Both are off unless the owner turned them on, and both wait for the
/// tailnet's admin console to approve them before any device can use them.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Routing {
    /// Other devices may send their internet traffic through this reader.
    exit_node: bool,
    /// The network the reader's Wi-Fi is on, offered to the tailnet.
    network: Option<String>,
}

impl Routing {
    const fn any(&self) -> bool {
        self.exit_node || self.network.is_some()
    }
}

/// Why the reader's network is not being shared.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NotShared {
    /// No Wi-Fi network right now. Whatever was shared before still is,
    /// since nothing can reach it either way until Wi-Fi is back.
    NoNetwork,
    NotPrivate,
    TooLarge,
}

impl NotShared {
    const fn why(self) -> &'static str {
        match self {
            Self::NoNetwork => {
                "The reader is not on a Wi-Fi network yet, so there is nothing to share."
            }
            Self::NotPrivate => {
                "The Wi-Fi this reader is on is not a private home network, so it is not shared."
            }
            Self::TooLarge => "The Wi-Fi network this reader is on is too large to share.",
        }
    }
}

/// The network the reader's Wi-Fi is on, as the tailnet would be offered it.
///
/// Worked out from the kernel's routing table and never from a setting: the
/// network to share is the one the reader is actually on, and a reader
/// carried to another house is on another one. Only a private range is ever
/// offered, because offering anything else would be claiming somebody else's
/// addresses for every device that trusts this tailnet's routes. And only
/// one of an ordinary size. A network of sixteen million addresses is not
/// somebody's home, and advertising it would take that whole range away
/// from every other device on the tailnet.
fn shared_network(layout: &Layout) -> Result<String, NotShared> {
    let table = fs::read_to_string(layout.proc.join("net/route")).unwrap_or_default();
    parse_shared_network(&table)
}

fn parse_shared_network(table: &str) -> Result<String, NotShared> {
    // Interface, destination, gateway, flags, three counters, mask. The
    // addresses are printed as the reader's processor holds them, which on
    // every Kobo is least significant byte first.
    let rows = table
        .lines()
        .skip(1)
        .filter_map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            let word = |index: usize| u32::from_str_radix(fields.get(index)?, 16).ok();
            Some((*fields.first()?, word(1)?, word(2)?, word(7)?))
        })
        .filter(|(interface, ..)| *interface != INTERFACE)
        .collect::<Vec<_>>();
    let (wifi, ..) = rows
        .iter()
        .find(|(_, destination, _, mask)| *destination == 0 && *mask == 0)
        .ok_or(NotShared::NoNetwork)?;
    let (_, destination, _, mask) = rows
        .iter()
        .find(|(interface, _, gateway, mask)| interface == wifi && *gateway == 0 && *mask != 0)
        .ok_or(NotShared::NoNetwork)?;
    let network = std::net::Ipv4Addr::from(destination.to_le_bytes());
    let mask = u32::from_be_bytes(mask.to_le_bytes());
    let prefix = mask.leading_ones();
    if mask.count_ones() != prefix || !network.is_private() {
        return Err(NotShared::NotPrivate);
    }
    if !(16..=30).contains(&prefix) {
        return Err(NotShared::TooLarge);
    }
    Ok(format!("{network}/{prefix}"))
}

/// What the owner chose, one word a line as the app saves it. Lines an older
/// app never wrote are off.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "four switches the owner sets one at a time; any other shape is sixteen variants"
)]
struct Choices {
    on: bool,
    /// Stay on the tailnet after Cobalt hands the screen back.
    outside: bool,
    /// Offer this reader as an exit node.
    exit_node: bool,
    /// Share the network the reader's Wi-Fi is on.
    share_network: bool,
}

impl Choices {
    /// Whether the reader is asked to carry traffic for other devices.
    const fn routes(self) -> bool {
        self.exit_node || self.share_network
    }
}

/// Whether the book partition is mounted where the app's files are.
///
/// It is not while a computer has it over USB. The folder it mounts on is
/// still there then, empty, on the root filesystem, so asking whether a file
/// exists gets a confident wrong answer: every switch reads as off, and a
/// status written there lands on the wrong partition and is hidden the moment
/// the card comes back. The kernel's list of mounts is the only honest
/// witness.
fn card_present(layout: &Layout) -> bool {
    fs::read_to_string(layout.proc.join("mounts")).is_ok_and(|mounts| {
        mounts.lines().any(|line| {
            line.split_whitespace()
                .nth(1)
                .is_some_and(|at| Path::new(at) == layout.card)
        })
    })
}

/// What the owner chose, or `None` when that cannot be known right now
/// because the card is away.
///
/// With the card present, a choice that is missing or unreadable is off: the
/// only safe reading of "nobody said" for a service that opens the reader to
/// a network is that nobody asked for it.
fn choices(layout: &Layout) -> Option<Choices> {
    if !card_present(layout) {
        return None;
    }
    let text = fs::read_to_string(layout.app_state.join(CONFIG)).unwrap_or_default();
    let mut lines = text.lines();
    let mut next = || lines.next() == Some("true");
    Some(Choices {
        on: next(),
        outside: next(),
        exit_node: next(),
        share_network: next(),
    })
}

/// Whether this reader is to stay connected after the session: the owner
/// asked for it, and nothing has said it cannot be arranged.
fn staying(layout: &Layout, choices: Choices) -> bool {
    choices.outside && !layout.home.join(CANNOT_STAY).exists()
}

fn sign_out_requested(layout: &Layout) -> bool {
    card_present(layout)
        && fs::read_to_string(layout.app_state.join(REQUEST))
            .is_ok_and(|text| text.trim() == SIGN_OUT)
}

/// The app-facing status, one field a line in the order the app reads them.
/// The first four lines are the original contract; the rest were added after,
/// and an app that predates a line ignores it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Status {
    state: &'static str,
    address: String,
    message: String,
    sign_in: String,
    name: String,
    /// Why the reader cannot stay connected outside Cobalt, when the owner
    /// asked for that and it could not be arranged. Empty otherwise.
    cannot_stay: String,
    /// Where the two routing choices stand, and the network being shared.
    routing: RoutingNotes,
}

/// What the app is told about the exit node and the shared network: for
/// each, nothing when it is off, `waiting` until the tailnet's admin console
/// has approved it, `approved`, or `unavailable` with the reason beside it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct RoutingNotes {
    exit_node: &'static str,
    network: &'static str,
    /// The network being offered, as written in the admin console.
    shared: String,
    exit_node_why: String,
    network_why: String,
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
        let routing = &self.routing;
        format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n",
            self.state,
            self.address,
            self.message,
            self.sign_in,
            self.name,
            self.cannot_stay,
            routing.exit_node,
            routing.network,
            routing.shared,
            routing.exit_node_why,
            routing.network_why
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
const STOPPED: &str = "Tailscale stopped. It starts again the next time you open Cobalt.";
const CLOSING_PASSWORD_LOGIN: &str = "Closing password login to this reader over SSH.";
const CANNOT_ROUTE: &str =
    "Tailscale on this reader could not carry traffic for other devices, so \
     it is not an exit node and does not share its network.";
const NO_SYSTEM_ROOM: &str =
    "The reader's system storage ran out of room for Tailscale. It will try again from the card.";
const NO_ROOM_TO_STAY: &str = "The reader's system storage does not have the 110 MB free that \
     staying connected outside Cobalt needs. Tailscale runs while Cobalt is open instead.";

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

/// `kobod --tailscale run|stay|keep|status|stop|sign-out`.
///
/// `run` is the supervisor that ends with its session. `stay` prepares the
/// root filesystem and starts `keep`, the same supervisor detached, from the
/// copy of this runtime it put there.
///
/// # Errors
///
/// `run` and `keep` fail when the service could not be brought up for a
/// reason that may pass by itself, which is how the runtime knows to try
/// again later. A run that ended because the owner has something to do first
/// succeeds: the status says what, and trying again without them would change
/// nothing.
pub fn command(arguments: &[String]) -> Result<(), String> {
    let layout = Layout::device();
    match arguments {
        [action] if action == "run" => match run(&layout) {
            End::Settled => Ok(()),
            End::Later(why) => Err(why),
        },
        [action] if action == "keep" => match run(&layout.kept()) {
            End::Settled => Ok(()),
            End::Later(why) => Err(why),
        },
        [action] if action == "stay" => {
            let runtime = std::env::current_exe()
                .map_err(|error| format!("find this runtime to copy: {error}"))?;
            stay(&layout.kept(), &runtime)
        }
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
        _ => Err("usage: kobod --tailscale run|stay|keep|status|stop|sign-out".to_owned()),
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
    /// Switched on, and to stay connected after Cobalt hands the screen back.
    Stay,
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
            Self::Stay => ["--tailscale", "stay"],
            Self::SignOut => ["--tailscale", "sign-out"],
        }
    }
}

/// What a session remembers of a start that ended with nothing wrong, so it
/// is not made again until the owner has done something.
///
/// A supervisor ends without failing when it is waiting on the owner: the
/// reader was refused, nobody finished signing in, an admin has to approve
/// it. The status says which, and starting again without them changes
/// nothing. Yet the session's only question used to be whether the switch
/// was on, so on a reader it started the next supervisor the moment one
/// ended, around and around for as long as Cobalt was open: a refusal was
/// checked for again every second or two, and a sign-in nobody finished
/// never stopped its daemon, it only got a new link every ten minutes.
///
/// The one settled end that must be followed by a start is the supervisor
/// leaving because the owner changed a choice it cannot take up while
/// running. That is told apart by the owner having asked while it ran.
#[derive(Clone, Copy, Debug, Default)]
#[cfg_attr(not(feature = "device-write"), allow(dead_code))]
pub struct Settled {
    ended: Option<Want>,
    asked_meanwhile: bool,
}

#[cfg_attr(not(feature = "device-write"), allow(dead_code))]
impl Settled {
    /// The owner saved something in the app. `running` says whether a
    /// supervisor was there to see it.
    pub fn asked(&mut self, running: bool) {
        self.ended = None;
        self.asked_meanwhile = running;
    }

    /// What was started for `want` ended without failing.
    pub fn ended(&mut self, want: Want) {
        if !std::mem::take(&mut self.asked_meanwhile) {
            self.ended = Some(want);
        }
    }

    /// What was started failed, which is tried again by the clock.
    pub fn failed(&mut self) {
        self.asked_meanwhile = false;
    }

    /// Whether `want` has been done as far as it can be without the owner.
    ///
    /// Asked of what is wanted now, not of what ended: a step that could not
    /// arrange for the reader to stay connected ends well too, and what is
    /// wanted after it is the supervisor that runs with the session.
    #[must_use]
    pub fn holds(self, want: Want) -> bool {
        self.ended == Some(want)
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
    let choices = choices(layout).filter(|choices| choices.on)?;
    Some(if staying(layout, choices) {
        Want::Stay
    } else {
        Want::Run
    })
}

/// What is already supervising Tailscale when a session looks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(not(feature = "device-write"), allow(dead_code))]
pub enum Standing {
    Nothing,
    /// A supervisor kept from an earlier session, made from this build. It
    /// is adopted as it stands: it reads the owner's choices itself.
    Kept,
    /// A supervisor from another build of the runtime, whose pins and rules
    /// may not be this build's. It is asked to leave so one can be started.
    Stale,
}

/// Looks for a supervisor that outlived the session that started it.
#[cfg_attr(not(feature = "device-write"), allow(dead_code))]
pub fn standing() -> Standing {
    std::env::current_exe().map_or(Standing::Nothing, |runtime| {
        standing_in(&Layout::device().kept(), &runtime)
    })
}

fn standing_in(layout: &Layout, runtime: &Path) -> Standing {
    let Some(supervisor) = live_supervisor(layout) else {
        return Standing::Nothing;
    };
    let copied_from = fs::read_to_string(layout.bin.join(KEPT_RUNTIME_FROM)).ok();
    if process_names(layout, supervisor, "keep")
        && copied_from.is_some()
        && copied_from == build_stamp(runtime)
    {
        Standing::Kept
    } else {
        Standing::Stale
    }
}

/// The supervisor recorded in the daemon's home, if it is still running and
/// still a supervisor.
fn live_supervisor(layout: &Layout) -> Option<u32> {
    recorded_pid(&layout.home.join(SUPERVISOR_PID))
        .filter(|pid| *pid != std::process::id() && process_names(layout, *pid, "--tailscale"))
}

/// What identifies a build of the runtime cheaply enough to ask on every
/// look: its version, its size and when it was written. An update changes at
/// least the last, and nothing here has to read ten megabytes to find out.
fn build_stamp(runtime: &Path) -> Option<String> {
    let metadata = fs::metadata(runtime).ok()?;
    let written = metadata
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some(format!(
        "{} {} {written}",
        env!("CARGO_PKG_VERSION"),
        metadata.len()
    ))
}

/// Lets staying connected be tried again, because the owner just changed
/// something or a new session has begun.
///
/// The reason it could not be arranged last time, usually room, is the kind
/// of thing an owner fixes and then expects to be noticed.
#[cfg_attr(not(feature = "device-write"), allow(dead_code))]
pub fn try_staying_again() {
    try_staying_again_in(&Layout::device());
}

fn try_staying_again_in(layout: &Layout) {
    let _ignored = fs::remove_file(layout.home.join(CANNOT_STAY));
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
    // A supervisor that is running owns everything this would clear. The one
    // that can be found here is the kind kept past its session, and its
    // daemon is not a leftover.
    if live_supervisor(layout).is_some() {
        return;
    }
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

/// Whether another device may be sending its traffic through this reader
/// right now: the tunnel is up, and the tailnet has approved the exit node
/// or the shared network.
///
/// An offer still waiting for approval is not this. No device can use it
/// yet, and a reader kept awake for it would spend its battery on nothing.
/// The witnesses are the two [`connected`] takes, for the same reasons.
#[cfg_attr(not(feature = "device-write"), allow(dead_code))]
fn carrying_in(layout: &Layout) -> bool {
    if !layout.interfaces.join(INTERFACE).exists() {
        return false;
    }
    let status = read_status(layout);
    let mut lines = status.lines();
    // The exit node and the shared network are the seventh and eighth lines.
    lines.next() == Some("running") && lines.skip(5).take(2).any(|offer| offer == "approved")
}

/// How often a session asks whether the reader is carrying traffic for other
/// devices. The supervisor learns what the tailnet has approved once a
/// minute, so asking more often would hear the same answer.
#[cfg_attr(not(feature = "device-write"), allow(dead_code))]
const HOLD_CHECK: Duration = Duration::from_secs(60);

/// The charge below which a reader stops staying awake for other devices.
///
/// Carrying traffic is something the owner switches on and walks away from,
/// and what they come back to has to be a reader that still turns on. On a
/// charger it is held at any charge. The Tailscale app says this number.
#[cfg_attr(not(feature = "device-write"), allow(dead_code))]
const HOLD_MIN_BATTERY: u8 = 20;

/// Keeps a session from ending under a device that depends on this reader.
///
/// A session nobody has touched for an hour ends itself. That hands the
/// reader to the Kobo software, which puts it to sleep and switches Wi-Fi
/// off, and a phone using this reader as its exit node loses the internet at
/// that moment with nothing on the phone to say why. So a reader whose exit
/// node or shared network the tailnet has approved counts as in use, for as
/// long as that is so and the battery can afford it.
///
/// A gauge that cannot be read does not hold the reader. Trusting it would
/// mean running a battery flat on the strength of not knowing how full it is.
///
/// The power button and the cover are not held back by this. They are the
/// owner saying the reader is done, whatever it is carrying.
#[cfg_attr(not(feature = "device-write"), allow(dead_code))]
#[derive(Debug, Default)]
pub struct AwakeForOthers {
    looked: Option<Instant>,
    held: bool,
}

#[cfg_attr(not(feature = "device-write"), allow(dead_code))]
impl AwakeForOthers {
    /// Whether the session's limits should move on at `now`.
    pub fn holds(&mut self, now: Instant) -> bool {
        self.holds_in(&Layout::device(), now, kobo_hal::battery::read)
    }

    fn holds_in(
        &mut self,
        layout: &Layout,
        now: Instant,
        battery: impl FnOnce() -> Option<kobo_hal::battery::Battery>,
    ) -> bool {
        if self
            .looked
            .is_some_and(|at| now.saturating_duration_since(at) < HOLD_CHECK)
        {
            return self.held;
        }
        self.looked = Some(now);
        self.held = carrying_in(layout)
            && battery()
                .is_some_and(|battery| battery.charging || battery.percent >= HOLD_MIN_BATTERY);
        self.held
    }
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
    /// The claim to be the one supervisor no longer names this one, so
    /// another is running and this one has to get out of its way.
    Displaced,
    /// The owner changed something only a fresh start can honour: whether
    /// the reader stays connected outside Cobalt, which is a different
    /// supervisor's job, or whether it carries traffic for other devices,
    /// which the daemon is told once, as it starts.
    OtherKind,
}

/// One run of the service, from nothing to the tunnel being taken down.
struct Supervisor<'a> {
    layout: &'a Layout,
    /// The process that started this one. A different answer later means it
    /// died and this was adopted, which is the only notice an orphan gets.
    parent: u32,
    /// What the owner last chose, for while the card is away and cannot say.
    known: std::cell::Cell<Choices>,
    /// A status that could not be written because the card was away.
    unsaid: std::cell::RefCell<Option<Status>>,
    /// Why this reader is not staying connected although its owner asked.
    cannot_stay: String,
    /// The routing choices this supervisor started with. The daemon learns
    /// them from its environment, so a change is a new start.
    asked: Choices,
    /// Set when the daemon turned out not to be carrying traffic itself, so
    /// nothing is advertised that would then go nowhere.
    cannot_route: std::cell::Cell<bool>,
    /// What is being advertised now, and where it stands.
    advertised: std::cell::RefCell<Routing>,
    notes: std::cell::RefCell<RoutingNotes>,
    /// What this supervisor wrote to claim being the only one.
    claim: String,
}

impl<'a> Supervisor<'a> {
    fn new(layout: &'a Layout) -> Self {
        let known = choices(layout).unwrap_or_default();
        // Read once: it is written before this supervisor is started and
        // cleared only by the owner asking again, which starts another.
        let cannot_stay = if known.outside && !layout.kept {
            fs::read_to_string(layout.home.join(CANNOT_STAY)).unwrap_or_default()
        } else {
            String::new()
        };
        Self {
            layout,
            parent: std::os::unix::process::parent_id(),
            known: std::cell::Cell::new(known),
            unsaid: std::cell::RefCell::new(None),
            cannot_stay: cannot_stay.trim().to_owned(),
            asked: known,
            cannot_route: std::cell::Cell::new(false),
            advertised: std::cell::RefCell::default(),
            notes: std::cell::RefCell::default(),
            claim: new_claim(),
        }
    }

    /// What the owner chose, as far as it can be known. While the card is
    /// away the last answer stands: a computer holding the card is not the
    /// owner switching anything off.
    fn choices(&self) -> Choices {
        if let Some(fresh) = choices(self.layout) {
            self.known.set(fresh);
        }
        self.known.get()
    }

    fn interrupted(&self) -> Option<Interrupt> {
        self.catch_up();
        if fs::read_to_string(self.layout.home.join(SUPERVISOR_PID))
            .is_ok_and(|held| held != self.claim)
        {
            return Some(Interrupt::Displaced);
        }
        if self.layout.home.join(STOP).exists() {
            return Some(Interrupt::SessionEnding);
        }
        // A supervisor that is kept has no session to be orphaned by: being
        // adopted by init is how it was started.
        if !self.layout.kept && std::os::unix::process::parent_id() != self.parent {
            return Some(Interrupt::Orphaned);
        }
        // Asked before the switch is read: the app saves the request and
        // then switches off, and a sign-out mistaken for a plain off would
        // stop the daemon before it could tell the tailnet the key is dead.
        if sign_out_requested(self.layout) {
            return Some(Interrupt::SignOut);
        }
        let choices = self.choices();
        if !choices.on {
            return Some(Interrupt::SwitchedOff);
        }
        (staying(self.layout, choices) != self.layout.kept
            || choices.exit_node != self.asked.exit_node
            || choices.share_network != self.asked.share_network)
            .then_some(Interrupt::OtherKind)
    }

    /// Works out what to advertise now: what the owner asked for, less
    /// whatever this reader has shown it cannot do, with the network read
    /// fresh, because a reader carried to another Wi-Fi is on another one.
    fn plan(&self) -> Routing {
        let mut notes = RoutingNotes::default();
        let mut plan = Routing::default();
        if self.cannot_route.get() {
            if self.asked.exit_node {
                notes.exit_node = "unavailable";
                CANNOT_ROUTE.clone_into(&mut notes.exit_node_why);
            }
            if self.asked.share_network {
                notes.network = "unavailable";
                CANNOT_ROUTE.clone_into(&mut notes.network_why);
            }
        } else {
            if self.asked.exit_node {
                plan.exit_node = true;
                notes.exit_node = "waiting";
            }
            if self.asked.share_network {
                match shared_network(self.layout) {
                    Ok(network) => {
                        notes.network = "waiting";
                        notes.shared.clone_from(&network);
                        plan.network = Some(network);
                    }
                    Err(why) => {
                        notes.network = "unavailable";
                        why.why().clone_into(&mut notes.network_why);
                    }
                }
            }
        }
        *self.notes.borrow_mut() = notes;
        *self.advertised.borrow_mut() = plan.clone();
        plan
    }

    /// Whether what is advertised should be advertised again from scratch:
    /// the daemon has said it is not forwarding after all, or the reader is
    /// now on a different network from the one it is offering.
    fn routing_changed(&self, report: &Report) -> bool {
        let advertised = self.advertised.borrow().clone();
        if advertised.any() && report.not_forwarding {
            self.cannot_route.set(true);
            return true;
        }
        if !self.asked.share_network || self.cannot_route.get() {
            return false;
        }
        match shared_network(self.layout) {
            Ok(network) => advertised.network.as_ref() != Some(&network),
            // Offline, the old offer stands: nothing reaches it either way.
            Err(NotShared::NoNetwork) => false,
            Err(_) => advertised.network.is_some(),
        }
    }

    /// Records what the tailnet has approved of what is being advertised.
    fn note_approvals(&self, report: &Report) {
        let advertised = self.advertised.borrow();
        let mut notes = self.notes.borrow_mut();
        if advertised.exit_node {
            notes.exit_node = if report.exit_node_approved {
                "approved"
            } else {
                "waiting"
            };
        }
        if let Some(network) = &advertised.network {
            notes.network = if report.allowed.iter().any(|allowed| allowed == network) {
                "approved"
            } else {
                "waiting"
            };
        }
    }

    /// Says what happened, or remembers it for when the card is back.
    fn say(&self, status: &Status) {
        let status = Status {
            cannot_stay: self.cannot_stay.clone(),
            routing: self.notes.borrow().clone(),
            ..status.clone()
        };
        if !card_present(self.layout) {
            *self.unsaid.borrow_mut() = Some(status);
            return;
        }
        self.unsaid.borrow_mut().take();
        if let Err(error) = write_status(self.layout, &status) {
            eprintln!("kobod: tailscale status not written: {error}");
        }
    }

    /// Writes the status the card missed while a computer had it.
    fn catch_up(&self) {
        if !card_present(self.layout) {
            return;
        }
        if let Some(status) = self.unsaid.borrow_mut().take() {
            let _ignored = write_status(self.layout, &status);
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
            Interrupt::SwitchedOff => {
                // Switched off, nothing of this stays on the firmware's
                // partition but the key that makes switching on again a
                // matter of seconds. The card keeps its copy to copy from.
                let _ignored = fs::remove_dir_all(&self.layout.kept_bin);
                self.say(&Status::quiet("off", OFF));
            }
            // The supervisor that holds the claim is the one whose word
            // about the service is true. This one says nothing.
            Interrupt::Displaced => return End::Settled,
            // The other kind of supervisor is about to be started in this
            // one's place, so what the owner sees is a start.
            Interrupt::OtherKind => self.say(&Status::quiet("starting", "Starting Tailscale.")),
            Interrupt::SessionEnding | Interrupt::Orphaned => {
                self.say(&Status::quiet(
                    "stopped",
                    if self.layout.kept {
                        STOPPED
                    } else {
                        LEFT_COBALT
                    },
                ));
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

/// Decides where a supervisor that ends with its session runs its programs
/// from: the root filesystem where it has the room, the card where it does
/// not.
///
/// From the root filesystem, what is checked is what is run. Nothing but
/// root on the reader can write there, and a computer holding the card
/// cannot reach it, so the digest taken before each start is of the bytes
/// that start.
///
/// From the card that is not quite true, and a reader without the room is
/// left with the difference rather than without the service. The programs
/// are verified where they lie and run from where they lie, so anything able
/// to write the card between the check and the start, or to replace the
/// command the supervisor goes on running every minute, decides what runs
/// as root. During a session that is nothing a computer can do, because the
/// card is not exported while Cobalt has the screen, and nothing an app can
/// do, because apps are confined away from this folder. It is something
/// only a process that is already root on the reader can do, which gains
/// nothing by it. The other copy closes it anyway, where it fits, because
/// that sentence stops being true the day something less than root can
/// write the card.
fn place(layout: &Layout) -> Layout {
    let system = layout.clone().on_the_root_filesystem();
    let installed = system.daemon().is_file() && system.cli().is_file();
    let room = kobo_abi::free_space(&system.bin)
        .or_else(|| system.bin.parent().and_then(kobo_abi::free_space))
        // Not knowing is not room: this is the firmware's own partition.
        .is_some_and(|free| free >= system.room());
    if installed || room {
        system
    } else {
        layout.clone()
    }
}

fn run(layout: &Layout) -> End {
    let placed = if layout.kept {
        layout.clone()
    } else {
        place(layout)
    };
    let layout = &placed;
    let supervisor = Supervisor::new(layout);
    let choices = supervisor.choices();
    if !choices.on {
        sweep_in(layout);
        supervisor.say(&Status::quiet("off", OFF));
        return End::Settled;
    }
    if staying(layout, choices) != layout.kept {
        // Started as one kind of supervisor when the owner wants the other.
        // Nothing to take down; the session starts the right one next.
        return End::Settled;
    }
    if let Err(end) = claim(layout, &supervisor.claim) {
        return end;
    }
    let end = supervise(&supervisor);
    // Only its own claim: one that names another supervisor is not this
    // one's to remove.
    let pid_file = layout.home.join(SUPERVISOR_PID);
    if fs::read_to_string(&pid_file).is_ok_and(|held| held == supervisor.claim) {
        let _ignored = fs::remove_file(pid_file);
    }
    end
}

/// Arranges for the reader to stay connected, then starts the supervisor
/// that does it and returns.
///
/// Run by the session in a process of its own, because copying a runtime is
/// not work for the loop that draws the screen. When it cannot be arranged
/// the reason is left where the session and the app will find it, and this
/// still succeeds: the service then runs with the session, which is better
/// than not running, and trying again would fail the same way.
fn stay(layout: &Layout, runtime: &Path) -> Result<(), String> {
    if live_supervisor(layout).is_some() {
        return Ok(());
    }
    if let Err(why) = prepare_to_stay(layout, runtime) {
        prepare_home(layout)?;
        return atomic_write(&layout.home.join(CANNOT_STAY), why.as_bytes(), 0o600);
    }
    // Detached the way the recovery watchdog is: started by a shell that has
    // already gone, so it belongs to no session that ends. Its own session
    // where the reader can give it one, and never a directory on the card to
    // stand in, or the stock reader could not hand the card to a computer.
    Command::new("/bin/sh")
        .arg("-c")
        .arg(
            r#"cd / || exit 1
if command -v setsid >/dev/null 2>&1; then
  nohup setsid "$0" --tailscale keep >/dev/null 2>&1 &
else
  nohup "$0" --tailscale keep >/dev/null 2>&1 &
fi"#,
        )
        .arg(layout.bin.join(KEPT_RUNTIME))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(drop)
        .map_err(|error| format!("start the kept Tailscale supervisor: {error}"))
}

/// Puts a copy of this runtime on the root filesystem, after checking that
/// it and the programs will fit.
///
/// The copy is what makes the supervisor independent of the card. A
/// supervisor run from the card would be a program whose own file a computer
/// can replace, or the stock reader unmount, while it is running.
fn prepare_to_stay(layout: &Layout, runtime: &Path) -> Result<(), String> {
    let copy = layout.bin.join(KEPT_RUNTIME);
    let stamp = build_stamp(runtime).ok_or("This runtime could not be read to copy it.")?;
    let current = copy.is_file()
        && fs::read_to_string(layout.bin.join(KEPT_RUNTIME_FROM)).is_ok_and(|from| from == stamp);
    let programs = layout.daemon().is_file() && layout.cli().is_file();
    if current && programs {
        return Ok(());
    }
    let own = |error: std::io::Error| format!("Tailscale could not be set up to stay: {error}");
    fs::create_dir_all(&layout.bin).map_err(own)?;
    fs::set_permissions(&layout.bin, fs::Permissions::from_mode(0o700)).map_err(own)?;
    let runtime_bytes = if current {
        0
    } else {
        fs::metadata(runtime).map_err(own)?.len()
    };
    let needed = if programs { 0 } else { layout.room() }.saturating_add(runtime_bytes);
    if kobo_abi::free_space(&layout.bin).is_some_and(|free| free < needed) {
        return Err(NO_ROOM_TO_STAY.to_owned());
    }
    if !current {
        let partial = layout.bin.join("kobod.part");
        fs::copy(runtime, &partial)
            .and_then(|_| fs::set_permissions(&partial, fs::Permissions::from_mode(0o700)))
            .and_then(|()| kobo_protocol::durability::sync_file(&partial))
            .and_then(|()| fs::rename(&partial, &copy))
            .map_err(|error| {
                let _ignored = fs::remove_file(&partial);
                own(error)
            })?;
        atomic_write(&layout.bin.join(KEPT_RUNTIME_FROM), stamp.as_bytes(), 0o600)?;
    }
    Ok(())
}

/// Makes this the one supervisor, and clears what the last one left.
///
/// Two supervisors would each take the other's daemon for a leftover, so
/// being the only one cannot rest on looking and then writing: two that look
/// in the same moment both find nobody. The claim is a file whose name can
/// be given only once. This one's claim is written out whole under a name of
/// its own and then linked to the shared name, which the filesystem refuses
/// if the name is taken, and which never shows anybody a half-written claim
/// to mistake for a dead one.
///
/// A claim whose owner is gone is moved aside before another is made, and
/// what was moved is looked at, because a rival may have replaced the dead
/// claim with a live one in the moment between reading it and moving it.
/// That one is put back. Should a third have claimed in that same moment,
/// the supervisor that was displaced finds out at its next look, since every
/// supervisor keeps checking that the claim still names it, and leaves.
fn claim(layout: &Layout, token: &str) -> Result<(), End> {
    prepare_home(layout).map_err(End::Later)?;
    let pid_file = layout.home.join(SUPERVISOR_PID);
    // Names nobody else can be using, this claim's own word in each, so two
    // claimants never write through the same temporary name.
    let word = token.replace(' ', "-");
    let own = layout.home.join(format!("claim-{word}"));
    let aside = layout.home.join(format!("claim-{word}.stale"));
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&own)
        .and_then(|mut file| file.write_all(token.as_bytes()))
        .map_err(|error| End::Later(format!("write the Tailscale supervisor's claim: {error}")))?;
    let settled = settle_claim(layout, &pid_file, &own, &aside);
    let _ignored = fs::remove_file(&own);
    settled?;
    sweep_in(layout);
    forget_stop_request_in(layout);
    Ok(())
}

fn settle_claim(layout: &Layout, pid_file: &Path, own: &Path, aside: &Path) -> Result<(), End> {
    for _ in 0..8 {
        match fs::hard_link(own, pid_file) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(End::Later(format!(
                    "claim the Tailscale supervisor: {error}"
                )))
            }
        }
        let held = fs::read_to_string(pid_file).unwrap_or_default();
        if claimant_is_alive(layout, &held) {
            return Err(End::Later(
                "another Tailscale supervisor is already running".to_owned(),
            ));
        }
        if fs::rename(pid_file, aside).is_ok() {
            let taken = fs::read_to_string(aside).unwrap_or_default();
            if taken != held && claimant_is_alive(layout, &taken) {
                let _ignored = fs::hard_link(aside, pid_file);
            }
            let _ignored = fs::remove_file(aside);
        }
    }
    Err(End::Later(
        "the Tailscale supervisor's claim could not be settled".to_owned(),
    ))
}

/// Whether the supervisor a claim names is still running. A claim is the
/// claimant's process number and a word that tells its claims apart.
fn claimant_is_alive(layout: &Layout, claim: &str) -> bool {
    claim
        .split_whitespace()
        .next()
        .and_then(|pid| pid.parse().ok())
        .is_some_and(|pid| process_names(layout, pid, "--tailscale"))
}

/// A claim no other supervisor's can equal: this process, and a word made
/// from the clock and a count, so two started in one process differ too.
fn new_claim() -> String {
    static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    format!(
        "{} {nanos:x}-{}",
        std::process::id(),
        COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

fn supervise(supervisor: &Supervisor) -> End {
    let layout = supervisor.layout;
    if let Some(refusal) = preflight(supervisor) {
        supervisor.say(&refusal);
        return End::Settled;
    }
    if !engine_is_installed(layout) {
        supervisor.say(&Status::quiet(
            "downloading",
            "Downloading Tailscale. This takes a few minutes the first time.",
        ));
    }
    if let Err(why) = ensure_engine(layout, &|offset, length| {
        kobo_net::fetch_from(layout.engine.url, offset, length, None, &[]).ok()
    }) {
        if layout.kept && why == NO_ROOM_TO_STAY {
            // Not a failure to retry: the service runs with the session
            // instead, and the app says why it is not staying.
            let _ignored = atomic_write(&layout.home.join(CANNOT_STAY), why.as_bytes(), 0o600);
            supervisor.say(&Status::quiet("starting", "Starting Tailscale."));
            return End::Settled;
        }
        supervisor.say(&Status::quiet("failed", why.clone()));
        return End::Later(why);
    }
    // The download can take most of a short session.
    if let Some(interrupt) = supervisor.interrupted() {
        return supervisor.leave(&mut Daemon::absent(), interrupt);
    }
    supervisor.say(&Status::quiet("starting", "Starting Tailscale."));
    let mut daemon = match Daemon::start(layout, supervisor.asked.routes()) {
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
///
/// One of them the reader can put right by itself. Root getting in with a
/// password is closed here, where a key is already on the reader to keep its
/// owner in, so switching Tailscale on does not send them to a computer for
/// something the reader can do. See [`door`].
fn preflight(supervisor: &Supervisor) -> Option<Status> {
    let layout = supervisor.layout;
    if let Err(why) = ensure_tun(layout) {
        eprintln!("kobod: tailscale tunnel device: {why}");
        return Some(Status::quiet(
            "unsupported",
            "This reader's software has no tunnel support, so Tailscale cannot run on it.",
        ));
    }
    let mut found = exposure(layout, None);
    if found == Exposure::PasswordLogin {
        supervisor.say(&Status::quiet("starting", CLOSING_PASSWORD_LOGIN));
        match door::close(layout) {
            Ok(()) => {
                eprintln!("kobod: tailscale: closed password login for root over SSH");
                found = exposure(layout, None);
            }
            Err(why) => eprintln!("kobod: tailscale: password login left as it is: {why}"),
        }
    }
    refusal(layout, found)
}

/// The words for a reader that should not be on a tailnet as it stands.
fn refusal(layout: &Layout, exposure: Exposure) -> Option<Status> {
    let why = match exposure {
        Exposure::Acceptable => return None,
        Exposure::PasswordLogin => {
            "Root can still log in to this reader with a password. Run kobo setup on your \
             computer to close that, then turn Tailscale on again."
                .to_owned()
        }
        Exposure::SshUnreadable => {
            "The reader's SSH server could not be asked how it lets root log in, so Tailscale \
             will not put the reader on a tailnet."
                .to_owned()
        }
        Exposure::DebugServices => {
            "Telnet or FTP is running on this reader and takes no password. Turn off the \
             reader's debug services, restart it, then turn Tailscale on again."
                .to_owned()
        }
        // Named where it can be, because "something" is no help to the owner
        // who has to stop it, and none to whoever is told that a reader was
        // refused and has to decide whether the list above is one short.
        Exposure::Listening(port) => format!(
            "{} is listening on port {port} on this reader, where your whole tailnet could \
             reach it. Stop it, then turn Tailscale on again.",
            who_listens(layout, port).map_or_else(
                || "Something".to_owned(),
                |program| format!("A program called {program}")
            )
        ),
        Exposure::Unreadable => {
            "The reader does not say what is listening on it, so Tailscale will not put it on \
             a tailnet."
                .to_owned()
        }
    };
    Some(Status::quiet("refused", why))
}

/// What the reader would be offering a tailnet that it should not.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Exposure {
    Acceptable,
    /// The SSH server is listening and root can still get in with something
    /// typed rather than a key.
    PasswordLogin,
    /// The SSH server is listening and would not say what it enforces.
    SshUnreadable,
    /// The firmware's debug services are listening.
    DebugServices,
    /// Something this runtime does not know is listening on this port.
    Listening(u16),
    /// The kernel's list of what is listening could not be read.
    Unreadable,
}

/// Looks at what is listening before the reader is put on a tailnet, and
/// again every minute that it is on one.
///
/// With no packet filter on the reader, joining a tailnet offers every
/// listening port to every computer on it, and a tailnet is not only the
/// owner's laptop: it is every device signed in to it and anything shared
/// into it. So the question asked is not whether anything known to be
/// dangerous is listening. That was the first version of this, and it
/// waved through a second SSH server on another port, telnet moved off its
/// usual one, and anything a shell had started. The question is whether
/// everything listening is something known and judged safe, and the list of
/// those is short:
///
/// * The SSH server, on the ports it says are its own, once root cannot get
///   in with anything but a key. It is the point of the exercise.
/// * The Sync engine's transfer port while a sync is running, recognised by
///   the engine being the process that holds the socket, not by its number.
///   It accepts only the computers the owner paired.
/// * The daemon's own listener on the tailnet address, recognised the same
///   way, which exists only once the tunnel does.
///
/// Anything else refuses the join and names the port, and so does not being
/// able to read the list. A reader this cannot see into is not one it puts
/// on a network.
///
/// This is TCP. Datagram sockets cannot be held to a list this way, because
/// every name lookup any program makes is, for a moment, a bound socket on a
/// port nobody chose.
fn exposure(layout: &Layout, daemon: Option<u32>) -> Exposure {
    let Some(listeners) = listeners(layout) else {
        return Exposure::Unreadable;
    };
    if listeners.is_empty() {
        return Exposure::Acceptable;
    }
    // Asked only when something is listening, and only once however many
    // ports it has.
    let mut ssh: Option<SshDoor> = None;
    let sync = processes_started_with(layout, &layout.sync_engine);
    for listener in listeners {
        let held_by_ours = listener.inode.is_some_and(|inode| {
            daemon
                .iter()
                .chain(&sync)
                .any(|pid| holds_socket(layout, *pid, inode))
        });
        if held_by_ours {
            continue;
        }
        if matches!(listener.port, 21 | 23) {
            return Exposure::DebugServices;
        }
        match ssh.get_or_insert_with(|| ssh_door(layout)) {
            SshDoor::Closed(ports) if ports.contains(&listener.port) => {}
            SshDoor::Open(ports) if ports.contains(&listener.port) => {
                return Exposure::PasswordLogin
            }
            // The usual SSH port with a server that will not answer is far
            // more likely that server than a stranger, and the owner is
            // better told about the server.
            SshDoor::Unreadable if listener.port == 22 => return Exposure::SshUnreadable,
            _ => return Exposure::Listening(listener.port),
        }
    }
    Exposure::Acceptable
}

/// A TCP socket something is listening on, at an address other machines can
/// reach.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Listener {
    port: u16,
    /// The socket's number in the kernel, which is how the process holding
    /// it is found. Absent on a kernel whose table does not carry it.
    inode: Option<u64>,
}

/// Every such socket, or `None` when the kernel's table cannot be read.
///
/// The table for the older address family is the one that must be readable.
/// The other is missing on a kernel built without that family, and a kernel
/// without the family has no listeners in it.
fn listeners(layout: &Layout) -> Option<Vec<Listener>> {
    let mut found = parse_listeners(&fs::read_to_string(layout.proc.join("net/tcp")).ok()?);
    if let Ok(table) = fs::read_to_string(layout.proc.join("net/tcp6")) {
        found.extend(parse_listeners(&table));
    }
    Some(found)
}

/// Reads the kernel's table of TCP sockets for the listening ones.
///
/// The local address is the second column, hexadecimal address and port, the
/// state is the fourth, where `0A` is listening, and the socket's number is
/// the tenth. A listener on loopback is left out, because a tunnel made by
/// the kernel does not carry it.
fn parse_listeners(table: &str) -> Vec<Listener> {
    const LISTENING: &str = "0A";
    table
        .lines()
        .skip(1)
        .filter_map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            let (address, port) = fields.get(1)?.rsplit_once(':')?;
            let loopback = matches!(address, "0100007F" | "00000000000000000000000001000000");
            if *fields.get(3)? != LISTENING || loopback {
                return None;
            }
            Some(Listener {
                port: u16::from_str_radix(port, 16).ok()?,
                inode: fields.get(9).and_then(|inode| inode.parse().ok()),
            })
        })
        .collect()
}

/// Whether process `pid` holds the socket numbered `inode`, read from the
/// kernel's list of that process's open files.
fn holds_socket(layout: &Layout, pid: u32, inode: u64) -> bool {
    let socket = format!("socket:[{inode}]");
    fs::read_dir(layout.proc.join(pid.to_string()).join("fd")).is_ok_and(|open| {
        open.filter_map(Result::ok).any(|file| {
            fs::read_link(file.path()).is_ok_and(|target| target.as_os_str() == socket.as_str())
        })
    })
}

/// The program holding the listener on `port`, by the name it was started
/// under.
///
/// The name is the process's own word about itself and ends up drawn on the
/// reader's screen, so only a short, plain one is passed on. A program that
/// calls itself anything else is not named at all.
fn who_listens(layout: &Layout, port: u16) -> Option<String> {
    let inode = listeners(layout)?
        .into_iter()
        .find(|listener| listener.port == port)?
        .inode?;
    let holder = fs::read_dir(&layout.proc)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
        .find(|pid| holds_socket(layout, *pid, inode))?;
    let line = fs::read(layout.proc.join(holder.to_string()).join("cmdline")).ok()?;
    let started_as = std::str::from_utf8(line.split(|byte| *byte == 0).next()?).ok()?;
    let name = Path::new(started_as).file_name()?.to_str()?;
    let plain = !name.is_empty()
        && name.len() <= 32
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
    plain.then(|| name.to_owned())
}

/// The processes that were started as `program`.
fn processes_started_with(layout: &Layout, program: &Path) -> Vec<u32> {
    let program = program.to_string_lossy();
    fs::read_dir(&layout.proc)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter_map(|entry| entry.file_name().to_str()?.parse().ok())
                .filter(|pid| process_names(layout, *pid, &program))
                .collect()
        })
        .unwrap_or_default()
}

/// How the SSH server lets root in, and the ports it says are its own.
#[derive(Clone, Debug, Eq, PartialEq)]
enum SshDoor {
    /// Root gets in with a key or not at all.
    Closed(Vec<u16>),
    /// Something typed still gets root in.
    Open(Vec<u16>),
    /// The server would not say.
    Unreadable,
}

/// Asks the SSH server what it would enforce for root arriving from the
/// tailnet.
///
/// The server is the only witness worth asking. A comment in its file that
/// says it was hardened says what somebody once wrote there, not what the
/// server does with the file as it is now, so nothing is inferred from the
/// file and a server that cannot be asked is treated as an open door.
///
/// It is asked as a connection from a tailnet address, in both address
/// families, because that is who this is about. A server's rules can depend
/// on where a connection comes from, and the answer for a connection from
/// the reader itself, which is what was asked before, is the answer to a
/// different question.
fn ssh_door(layout: &Layout) -> SshDoor {
    let mut ports = Vec::new();
    for address in ["100.64.0.1", "fd7a:115c:a1e0::1"] {
        let mut command = Command::new(&layout.sshd);
        command.args([
            "-T",
            "-C",
            &format!("user=root,host=tailnet,addr={address}"),
        ]);
        let Ok(said) = answer(command, layout.timing.answer) else {
            return SshDoor::Unreadable;
        };
        match parse_ssh_door(&said) {
            SshDoor::Closed(said_ports) => ports = said_ports,
            other => return other,
        }
    }
    SshDoor::Closed(ports)
}

/// Every value the server gave for the setting called `name`.
fn said<'a>(settings: &'a str, name: &'a str) -> impl Iterator<Item = &'a str> + 'a {
    settings.lines().filter_map(move |line| {
        line.strip_prefix(name)
            .and_then(|rest| rest.strip_prefix(' '))
            .map(str::trim)
    })
}

/// Reads the server's effective settings for whether root can get in with
/// anything but a key.
///
/// Two settings carry a typed secret: password login, and the question and
/// answer login that the server calls keyboard-interactive, which is a
/// password by another name wherever there is something behind it to check
/// one. On Linux that something is PAM and nothing else, and a server built
/// without PAM does not print the setting at all, so keyboard-interactive
/// counts as open only where the server says PAM is in use. A server that
/// refuses root outright, or lets root in by key only, is closed whatever
/// the rest says.
fn parse_ssh_door(settings: &str) -> SshDoor {
    let is = |name: &str, value: &str| said(settings, name).any(|said| said == value);
    let ports = said(settings, "port")
        .filter_map(|port| port.parse().ok())
        .collect::<Vec<u16>>();
    let Some(password) = said(settings, "passwordauthentication").next() else {
        // Not the server's answer, whatever it is.
        return SshDoor::Unreadable;
    };
    let root_by_key_only = [
        "no",
        "prohibit-password",
        "without-password",
        "forced-commands-only",
    ]
    .iter()
    .any(|value| is("permitrootlogin", value));
    let asks_questions =
        is("kbdinteractiveauthentication", "yes") || is("challengeresponseauthentication", "yes");
    let typed = password != "no" || (asks_questions && is("usepam", "yes"));
    if root_by_key_only || !typed {
        SshDoor::Closed(ports)
    } else {
        SshDoor::Open(ports)
    }
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
    // And the programs kept beside it, so a reader being given away carries
    // nothing of this on the firmware's partition.
    let _ignored = fs::remove_dir_all(&layout.kept_bin);
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

/// Waits for the supervisor that was asked to stop, for as long as there is
/// one.
///
/// Watching for its claim to disappear is not the same thing. A supervisor
/// that crashed leaves its claim behind, and whoever waited on the file then
/// waited out the whole patience for a process that was already gone.
fn wait_for_supervisor_to_leave(layout: &Layout) {
    let deadline = Instant::now() + layout.timing.stop + layout.timing.poll * 2;
    while live_supervisor(layout).is_some() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The process number a file records: its first word, so a supervisor's
/// claim and a daemon's bare number are read the same way.
fn recorded_pid(path: &Path) -> Option<u32> {
    fs::read_to_string(path)
        .ok()?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
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
    ///
    /// A fourth is set only when the owner asked the reader to carry traffic
    /// for other devices: the daemon is told to do that forwarding itself,
    /// and is given more room to do it in. With neither switch on, the
    /// daemon is started exactly as a leaf always was.
    fn start(layout: &Layout, routes: bool) -> Result<Self, String> {
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
            .env("GOMEMLIMIT", if routes { "96MiB" } else { "48MiB" })
            .env("GOGC", "50")
            .env("TS_DISABLE_TAILDROP", "true")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        if routes {
            command.env(FORWARD_IN_THE_DAEMON, "true");
        }
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

    fn pid(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
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
    /// Whether the kernel's tunnel is in use. If it ever is not, the daemon
    /// is in the mode that hands the reader's connections to loopback.
    tunnel: bool,
    /// Whether the daemon says packets it was asked to route will not be.
    not_forwarding: bool,
    /// Offered as an exit node and approved in the admin console.
    exit_node_approved: bool,
    /// The addresses the tailnet will route to this reader.
    allowed: Vec<String>,
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
        tunnel: value.get("TUN").and_then(kobo_json::Value::as_bool) != Some(false),
        not_forwarding: value
            .get("Health")
            .and_then(kobo_json::Value::as_array)
            .is_some_and(|health| {
                health
                    .iter()
                    .filter_map(kobo_json::Value::as_str)
                    .any(says_not_forwarding)
            }),
        exit_node_approved: own
            .and_then(|own| own.get("ExitNodeOption"))
            .and_then(kobo_json::Value::as_bool)
            == Some(true),
        allowed: own
            .and_then(|own| own.get("AllowedIPs"))
            .and_then(kobo_json::Value::as_array)
            .map(|allowed| {
                allowed
                    .iter()
                    .filter_map(kobo_json::Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
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
///
/// The two things an owner may switch on each change exactly one of these
/// words and nothing else: the exit node, and the network that is shared.
/// The firewall is left alone in every case, because the forwarding those
/// need is done inside the daemon and not by the kernel.
fn up_command(layout: &Layout, routing: &Routing) -> Command {
    let mut command = layout.ask();
    command
        .arg("up")
        .arg("--reset")
        .arg("--accept-dns=false")
        .arg("--accept-routes=false")
        .arg("--ssh=false")
        .arg(format!("--advertise-exit-node={}", routing.exit_node))
        .arg(format!(
            "--advertise-routes={}",
            routing.network.as_deref().unwrap_or_default()
        ))
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
    let routing = supervisor.plan();
    let mut child = match up_command(layout, &routing)
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
    let mut not_forwarding = false;
    let finished = loop {
        let said = read_what_up_says(supervisor, &lines, layout.timing.poll);
        announced |= said.sign_in;
        not_forwarding |= said.not_forwarding;
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
        if routing.any() && not_forwarding {
            // The daemon expects the kernel to forward, which this reader's
            // does not do, so what was just advertised would go nowhere.
            // Taken back at once, and the owner is told why.
            supervisor.cannot_route.set(true);
            return bring_up(supervisor, daemon);
        }
        return Ok(());
    }
    Err(explain_failed_start(supervisor, daemon, announced))
}

/// What the command that brings the reader up printed while it ran.
#[derive(Clone, Copy, Debug, Default)]
struct Said {
    sign_in: bool,
    not_forwarding: bool,
}

/// Whether a line from the daemon or its command says that routed packets
/// will not be forwarded.
///
/// This is how a daemon that was asked to carry traffic itself and is not
/// doing so gives itself away. One that is doing so never checks the
/// kernel's forwarding switch; one that expects the kernel to forward finds
/// the switch off, as it is on a reader, and says so.
fn says_not_forwarding(line: &str) -> bool {
    line.contains("IP forwarding is disabled") || line.contains("IP forwarding is off")
}

/// Reads what the command has printed, waiting up to `patience` for the next
/// line: a sign-in link goes straight to the screen, and a complaint that
/// nothing will be forwarded is remembered.
fn read_what_up_says(
    supervisor: &Supervisor,
    lines: &Receiver<String>,
    patience: Duration,
) -> Said {
    let mut said = Said::default();
    let mut wait = patience;
    loop {
        match lines.recv_timeout(wait) {
            Ok(line) => {
                if let Some(link) = sign_in_link(&line) {
                    supervisor.say(&Status {
                        sign_in: link.to_owned(),
                        ..Status::quiet("needs-login", "Approve this reader to finish.")
                    });
                    said.sign_in = true;
                }
                said.not_forwarding |= says_not_forwarding(&line);
                // Whatever else is already waiting is read without a pause.
                wait = Duration::ZERO;
            }
            Err(RecvTimeoutError::Timeout) => return said,
            Err(RecvTimeoutError::Disconnected) => {
                // Both pipes are closed, so the command is finishing. The
                // caller's loop still gets a pause, or it would spin until
                // the command has been collected.
                std::thread::sleep(patience.min(Duration::from_millis(50)));
                return said;
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
            // Asked again every time, not only before joining. What the
            // reader listens on can change under a tunnel that is up: a
            // hardening nobody confirmed undoes itself after five minutes,
            // and a reader kept connected for days has days for that.
            if let Some(refusal) = refusal(layout, exposure(layout, daemon.pid())) {
                return Err(supervisor.fail(daemon, &refusal, End::Settled));
            }
            match report(layout) {
                Some(report) if !report.tunnel => {
                    // The mode this module refuses on purpose. Every port on
                    // loopback is on the tailnet for as long as it lasts.
                    return Err(supervisor.fail(
                        daemon,
                        &Status::quiet(
                            "refused",
                            "Tailscale stopped using the reader's tunnel, which would have \
                             opened the reader's private ports to your tailnet, so it was \
                             stopped.",
                        ),
                        End::Settled,
                    ));
                }
                Some(report) if report.backend == Backend::Running => {
                    unanswered = 0;
                    if supervisor.routing_changed(&report) {
                        // Advertised again from the start, with what is now
                        // known: the same road as a first start.
                        return Ok(());
                    }
                    supervisor.note_approvals(&report);
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
/// `fetch` is the download, one piece at a time, passed in so the tests can
/// hand over an archive without a network.
fn ensure_engine(layout: &Layout, fetch: Fetch) -> Result<(), String> {
    if engine_is_installed(layout) {
        return Ok(());
    }
    // Whatever is there failed its check, so none of it is kept.
    let staging = layout.bin.join("unpack.part");
    // When the programs are going onto the root filesystem the archive waits
    // on the card, which has the room; the root filesystem is asked to hold
    // only the two programs it will keep.
    let archive = if layout.off_the_card() && card_present(layout) {
        layout.card_bin.join("tailscale.tgz.part")
    } else {
        layout.bin.join("tailscale.tgz.part")
    };
    let clear = || {
        let _ignored = fs::remove_dir_all(&staging);
        let _ignored = fs::remove_file(&archive);
    };
    let _ignored = fs::remove_file(layout.daemon());
    let _ignored = fs::remove_file(layout.cli());
    clear();
    if kobo_abi::free_space(&layout.bin)
        .or_else(|| layout.bin.parent().and_then(kobo_abi::free_space))
        .is_some_and(|free| free < layout.room())
    {
        return Err(if layout.kept {
            NO_ROOM_TO_STAY
        } else if layout.off_the_card() {
            NO_SYSTEM_ROOM
        } else {
            NO_ROOM
        }
        .to_owned());
    }
    fs::create_dir_all(&staging)
        .map_err(|error| format!("create the Tailscale folder: {error}"))?;
    let installed = copy_from_the_card(layout, &staging).or_else(|()| {
        if let Some(folder) = archive.parent() {
            fs::create_dir_all(folder)
                .map_err(|error| format!("create the Tailscale folder: {error}"))?;
        }
        download(layout, fetch, &archive).and_then(|()| unpack(layout, &archive, &staging))
    });
    clear();
    installed.inspect_err(|_| {
        let _ignored = fs::remove_file(layout.daemon());
        let _ignored = fs::remove_file(layout.cli());
    })
}

const NO_ROOM: &str =
    "The reader does not have room for Tailscale. Free 160 MB of storage and try again.";

/// One piece of the download: at most the second number of bytes, starting
/// the first number of bytes in. Fewer, or none, means the archive ended
/// there. `None` means the piece could not be had.
type Fetch<'a> = &'a dyn Fn(u32, u32) -> Option<Vec<u8>>;

/// Fetches the archive a piece at a time into the file it will be unpacked
/// from, and checks the whole of it against its digest.
///
/// The digest is taken as the pieces go by, so nothing has to be read back
/// and nothing larger than a piece is ever in memory. What is written is
/// not trusted until the last piece has arrived and the digest is the pinned
/// one; until then it is a file with a name nothing runs.
fn download(layout: &Layout, fetch: Fetch, archive: &Path) -> Result<(), String> {
    let saved = |error: std::io::Error| {
        if error.kind() == std::io::ErrorKind::StorageFull {
            NO_ROOM.to_owned()
        } else {
            format!("The Tailscale download could not be saved: {error}")
        }
    };
    let mut file = File::create(archive).map_err(saved)?;
    let mut hash = ring::digest::Context::new(&ring::digest::SHA256);
    let mut offset = 0_u32;
    loop {
        let piece = fetch(offset, layout.piece).ok_or_else(|| {
            "Tailscale could not be downloaded. Check Wi-Fi and try again.".to_owned()
        })?;
        let length = u32::try_from(piece.len()).unwrap_or(u32::MAX);
        offset = offset.saturating_add(length);
        // More than was asked for, or more than the archive could ever be,
        // is not the archive.
        if length > layout.piece || offset > layout.engine.limit {
            return Err("The Tailscale download did not match its checksum.".to_owned());
        }
        hash.update(&piece);
        file.write_all(&piece).map_err(saved)?;
        if length < layout.piece {
            break;
        }
    }
    file.sync_all().map_err(saved)?;
    if hex(hash.finish().as_ref()) != layout.engine.archive_sha256 {
        return Err("The Tailscale download did not match its checksum.".to_owned());
    }
    Ok(())
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
    install_checked(layout, &staging.join(folder))
}

/// Gives the two programs in `staged` their names, once each is the one
/// that was pinned.
fn install_checked(layout: &Layout, staged: &Path) -> Result<(), String> {
    let members = [
        ("tailscaled", layout.daemon(), layout.engine.daemon_sha256),
        ("tailscale", layout.cli(), layout.engine.cli_sha256),
    ];
    for (name, _, digest) in &members {
        if sha256(&staged.join(name))? != *digest {
            return Err("A Tailscale program did not match its checksum.".to_owned());
        }
    }
    for (name, installed, _) in &members {
        let staged = staged.join(name);
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o755))
            .and_then(|()| fs::rename(&staged, installed))
            .map_err(|error| format!("Tailscale could not be installed: {error}"))?;
    }
    Ok(())
}

/// Copies the programs a reader already downloaded to the card onto the
/// root filesystem, instead of downloading them a second time.
///
/// The card's copy is not trusted for having been checked once. Each program
/// is compared with its pin where it lies and again where it lands, because
/// the card is the partition a computer can rewrite and the root filesystem
/// is where the result will be run from as root for days.
fn copy_from_the_card(layout: &Layout, staging: &Path) -> Result<(), ()> {
    if !layout.off_the_card() || !card_present(layout) {
        return Err(());
    }
    for (name, digest) in [
        ("tailscaled", layout.engine.daemon_sha256),
        ("tailscale", layout.engine.cli_sha256),
    ] {
        let source = layout.card_bin.join(name);
        if sha256(&source).ok().as_deref() != Some(digest) {
            return Err(());
        }
        fs::copy(&source, staging.join(name)).map_err(drop)?;
    }
    install_checked(layout, staging).map_err(drop)
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
    Ok(hex(hash.finish().as_ref()))
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut hex, byte| {
            let _ignored = write!(hex, "{byte:02x}");
            hex
        })
}

//! Wi-Fi control through the firmware's running `wpa_supplicant`.
//!
//! This module never starts a second supplicant. Nickel and Cobalt would then
//! be two owners of one interface, an arrangement already proven unsafe on the
//! Clara BW. The backend is available only when the firmware's `wpa_cli` and
//! `wlan0` are both present; all operations go through that existing owner.

use crate::network::{is_online, renew_lease, signal_dbm, WIRELESS_LINK};
use kobo_protocol::{DeviceError, DeviceResult, WifiNetwork, MAX_RADIO_DEVICES};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

/// How long association and a DHCP lease are given after `reconnect`.
///
/// Measured rather than guessed: a Clara BW that already knows the network
/// is typically on a default route in two or three seconds; ten covers a
/// slow AP without turning a missing network into a long hang. The wait
/// returns as soon as the route appears.
const ASSOCIATE: Duration = Duration::from_secs(10);

/// Pause between taking the radio down and bringing it back up.
///
/// Long enough for the driver to drop the old association, short enough
/// that a reader watching a spinner does not notice a second attempt.
const BOUNCE: Duration = Duration::from_millis(400);

/// How long to wait for `wpa_cli` after the interface has just come up.
const SUPPLICANT_SETTLE: Duration = Duration::from_millis(200);

/// How long the wifi node may take to reappear after a `mem` wake.
///
/// `state-extended` powers the radio down with the rest of the subsystems.
/// Clearing it does not make `wlan0` instant, and a fetch that asks `Wifi::open`
/// in that gap used to proceed offline without ever talking to the supplicant.
const INTERFACE_WAIT: Duration = Duration::from_secs(8);

/// After sleep the node and the firmware DHCP client are both slower than a
/// fetch that happens while the reader is already awake. Eight seconds was
/// leaving the first request after a wake to open a socket against a ghost
/// route, which then sat until the TLS read timed out.
const AFTER_SLEEP_LINK: Duration = Duration::from_secs(15);

/// How long association and a DHCP lease are given after sleep.
const AFTER_SLEEP_ROUTE: Duration = Duration::from_secs(20);

/// How long the supplicant's control socket may take after the link is up.
const SUPPLICANT_WAIT: Duration = Duration::from_secs(5);

/// Poll while waiting for a default route, so sleep can abort a bring-up
/// without waiting out the whole association allowance.
const ROUTE_POLL: Duration = Duration::from_millis(250);

/// Whether this session still wants a remembered network on the air.
///
/// Nickel holds the association while it is awake. Cobalt has to do the same:
/// a reconnect that works after sleep and then dies is the usual `MediaTek`
/// outcome when nothing keeps the radio from power-saving and the DHCP client
/// from going away with the interface.
static WANTED: AtomicBool = AtomicBool::new(false);

/// False while Cobalt has put the device to sleep, so a fetch in flight cannot
/// turn the radio back on under a sleep screen.
static AWAKE: AtomicBool = AtomicBool::new(true);

/// Set when the radio is taken down for sleep. A default route can survive in
/// `/proc/net/route` across a `mem` wake while the association is gone; a
/// fetch that trusts that route opens a socket that never answers.
static STALE: AtomicBool = AtomicBool::new(false);

/// One attempt at a time, whether it is a wake restore or a fetch.
static BRING_UP: Mutex<()> = Mutex::new(());

/// Outcome of asking the firmware supplicant to put a remembered network
/// back on the air.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BringUp {
    /// A default route was already there; nothing was changed.
    AlreadyOnline,
    /// The supplicant has no remembered network, so there is nothing to join.
    NoSavedNetwork,
    /// The radio was down and a saved network now has a default route.
    Associated,
    /// A saved network was tried, including one radio bounce, and the
    /// reader is still offline.
    StillDown,
}

/// Records whether the session wants Wi-Fi while it is awake.
pub fn set_wanted(wanted: bool) {
    WANTED.store(wanted, Ordering::Relaxed);
}

/// Whether the session currently wants a remembered network held up.
#[must_use]
pub fn wanted() -> bool {
    WANTED.load(Ordering::Relaxed)
}

/// Puts `wlan0` up for the stock reader without starting a supplicant or a
/// DHCP client. Sleep and a session that turned the radio off can leave the
/// node administratively down; a restarted Nickel then has nothing to
/// associate on. This is not owning the radio: Nickel still starts the only
/// supplicant and still chooses whether to reconnect.
pub fn leave_link_up() {
    let _ignored = set_interface(true);
}

/// Records whether the session is awake. Sleep clears this before the radio
/// is taken down, so an in-flight bring-up stops instead of racing sleep.
pub fn set_awake(awake: bool) {
    if !awake {
        STALE.store(true, Ordering::Relaxed);
    }
    AWAKE.store(awake, Ordering::Relaxed);
}

fn stale() -> bool {
    STALE.load(Ordering::Relaxed)
}

fn mark_fresh() {
    STALE.store(false, Ordering::Relaxed);
}

fn joined(outcome: BringUp) -> BringUp {
    if matches!(outcome, BringUp::Associated | BringUp::AlreadyOnline) {
        mark_fresh();
    }
    outcome
}

/// True when a background keeper should put a saved network back on the air.
#[must_use]
pub fn should_keep() -> bool {
    AWAKE.load(Ordering::Relaxed) && WANTED.load(Ordering::Relaxed)
}

fn awake() -> bool {
    AWAKE.load(Ordering::Relaxed)
}

/// Where the firmware might keep `wpa_cli`. The Clara BW puts it in `/bin`;
/// the conventional places are checked too, because this list costs one
/// `stat` each and being wrong about it makes Wi-Fi report itself missing on
/// a reader that has it.
const WPA_TOOLS: [&str; 4] = [
    "/bin/wpa_cli",
    "/sbin/wpa_cli",
    "/usr/sbin/wpa_cli",
    "/usr/bin/wpa_cli",
];

#[derive(Clone, Debug)]
pub struct Wifi {
    wpa_cli: PathBuf,
}

impl Wifi {
    /// The firmware's `wpa_cli`, when present.
    ///
    /// `wlan0` is not required here: after a `mem` wake the node can take a
    /// few seconds to reappear, and a fetch in that gap still needs the same
    /// backend. [`Self::open`] is the stricter check used at session start.
    #[must_use]
    pub fn from_cli() -> Option<Self> {
        WPA_TOOLS
            .into_iter()
            .map(Path::new)
            .find(|path| path.is_file())
            .map(|path| Self {
                wpa_cli: path.to_path_buf(),
            })
    }

    #[must_use]
    pub fn open() -> Option<Self> {
        if !Path::new("/sys/class/net/wlan0").exists() {
            return None;
        }
        Self::from_cli()
    }

    #[must_use]
    pub fn state(&self) -> DeviceResult {
        // A status that cannot be read while the link is down is "off", not a
        // failure. Settings asks this on a clock, and treating a down
        // interface as an error painted a banner over a row that was simply
        // telling the truth.
        let status = self.command(["status"]).unwrap_or_default();
        let completed = value(&status, "wpa_state").is_some_and(|state| state == "COMPLETED");
        DeviceResult::Wifi {
            available: true,
            enabled: interface_enabled(),
            connected_ssid: completed
                .then(|| value(&status, "ssid").unwrap_or_default().to_owned()),
            networks: Vec::new(),
        }
    }

    /// Returns whether the firmware supplicant has completed association.
    ///
    /// # Errors
    ///
    /// Returns an error when the supplicant control socket cannot be queried.
    pub fn associated(&self) -> Result<bool, DeviceError> {
        self.command(["status"])
            .map(|status| value(&status, "wpa_state").is_some_and(|state| state == "COMPLETED"))
    }

    /// Asks the existing firmware supplicant to reconnect.
    ///
    /// # Errors
    ///
    /// Returns an error when the control socket rejects or cannot receive the
    /// request.
    pub fn reconnect(&self) -> Result<(), DeviceError> {
        self.command(["reconnect"]).map(|_| ())
    }

    /// Reproduces the stock reader's network-screen recovery without changing
    /// saved networks or starting another supplicant.
    ///
    /// # Errors
    ///
    /// Returns an error when the interface cannot be raised or the existing
    /// firmware supplicant rejects one of the recovery commands.
    pub fn recover_association(&self) -> Result<(), DeviceError> {
        if !set_interface(true) {
            return Err(DeviceError::Backend);
        }
        if self.associated()? {
            return Ok(());
        }
        for command in association_recovery_commands() {
            self.command(command)?;
            if self.associated()? {
                return Ok(());
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn set_enabled(&self, enabled: bool) -> DeviceResult {
        if enabled {
            if !self.bring_radio_up() {
                return DeviceResult::Failed(DeviceError::Backend);
            }
            if let Err(error) = self.command(["reconnect"]) {
                return DeviceResult::Failed(error);
            }
            return self.state();
        }
        // Turning the radio off is the request. `wpa_cli disconnect` often
        // returns FAIL once the firmware supplicant has already dropped the
        // link, and `status` cannot be read after the interface is down.
        // Reporting either as "the system radio service failed" was a lie:
        // the radio was off.
        let _ignored = self.command(["disconnect"]);
        if !set_interface(false) && interface_enabled() {
            return DeviceResult::Failed(DeviceError::Backend);
        }
        DeviceResult::Wifi {
            available: true,
            enabled: false,
            connected_ssid: None,
            networks: Vec::new(),
        }
    }

    #[must_use]
    pub fn scan(&self) -> DeviceResult {
        if let Err(error) = self.command(["scan"]) {
            return DeviceResult::Failed(error);
        }
        let results = match self.command(["scan_results"]) {
            Ok(results) => results,
            Err(error) => return DeviceResult::Failed(error),
        };
        let status = self.command(["status"]).unwrap_or_default();
        let connected = value(&status, "ssid");
        DeviceResult::Wifi {
            available: true,
            enabled: interface_enabled(),
            connected_ssid: connected.map(str::to_owned),
            networks: parse_scan_results(&results, connected),
        }
    }

    #[must_use]
    pub fn join(&self, ssid: &str, password: &str) -> DeviceResult {
        if !valid_credentials(ssid, password) {
            return DeviceResult::Failed(DeviceError::InvalidInput);
        }
        if !set_interface(true) {
            return DeviceResult::Failed(DeviceError::Backend);
        }
        let network = match self.command(["add_network"]).and_then(|output| {
            output
                .lines()
                .rev()
                .find_map(|line| line.trim().parse::<u32>().ok())
                .ok_or(DeviceError::Backend)
        }) {
            Ok(network) => network,
            Err(error) => return DeviceResult::Failed(error),
        };
        let ssid = quote(ssid);
        let commands = if password.is_empty() {
            format!(
                "set_network {network} ssid {ssid}\nset_network {network} key_mgmt NONE\n\
                 enable_network {network}\nselect_network {network}\nsave_config\nquit\n"
            )
        } else {
            let password = quote(password);
            format!(
                "set_network {network} ssid {ssid}\nset_network {network} psk {password}\n\
                 enable_network {network}\nselect_network {network}\nsave_config\nquit\n"
            )
        };
        match self.script(&commands) {
            Ok(_) => self.state(),
            Err(error) => {
                let network = network.to_string();
                let _ = self.command(["remove_network", network.as_str()]);
                DeviceResult::Failed(error)
            }
        }
    }

    #[must_use]
    pub fn disconnect(&self) -> DeviceResult {
        match self.command(["disconnect"]) {
            Ok(_) => self.state(),
            Err(error) => DeviceResult::Failed(error),
        }
    }

    /// True when `wlan0` is administratively up.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        interface_enabled()
    }

    /// After sleep: if the radio is down, turn it on and rejoin.
    ///
    /// Sleep already took the link down when it was on. Wake puts it back
    /// here, rather than waiting for an application to need the network. A
    /// working association is left alone. A leftover route with no
    /// association is dropped first so `wait_for_route` cannot succeed
    /// against a hole. Failure leaves the radio up: the session keeper
    /// retries, and a fetch must not find the interface down again.
    #[must_use]
    pub fn restore_after_sleep(&self) -> BringUp {
        let _guard = BRING_UP
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.restore_after_sleep_locked()
    }

    /// Puts a remembered network on the air if the reader has no default route.
    ///
    /// Nickel does this when something needs the network: the radio comes up,
    /// the firmware supplicant joins what it already knows, and the request
    /// proceeds. Cobalt owns the panel during a session, so the same work
    /// happens here, through the same `wpa_cli`, without starting a second
    /// supplicant or a long-lived DHCP client of our own.
    ///
    /// A completed association with no route is a missing lease, not a dead
    /// radio: the lease is renewed instead of bouncing the interface. A bounce
    /// is saved for a supplicant that is not associating at all. An empty
    /// `list_networks` right after the link comes up is not treated as "no
    /// saved network": that was taking the radio back down while the
    /// firmware's remembered networks were still loading. Sleep clears the
    /// awake flag so this returns without turning the radio back on.
    ///
    /// Two concurrent fetches share one attempt, so a pair of applications
    /// that wake together do not bounce the interface twice.
    #[must_use]
    pub fn ensure_online(&self) -> BringUp {
        self.bring_up()
    }

    fn restore_after_sleep_locked(&self) -> BringUp {
        if !awake() {
            return BringUp::StillDown;
        }
        WANTED.store(true, Ordering::Relaxed);
        if interface_enabled() && self.completed() && is_online(WIRELESS_LINK) {
            return joined(BringUp::AlreadyOnline);
        }
        if stale() && is_online(WIRELESS_LINK) && !self.completed() {
            let _ignored = set_interface(false);
            thread::sleep(BOUNCE);
            if !awake() {
                return BringUp::StillDown;
            }
        }
        if !self.bring_radio_up() {
            return BringUp::StillDown;
        }
        let _ignored = self.command(["reconfigure"]);
        thread::sleep(SUPPLICANT_SETTLE);
        let _ignored = self.reconnect_saved();
        // A lease asked for before association is the `udhcpc: no lease`
        // loop after sleep: five discovers into a radio that is not yet
        // joined, then "no route yet" with the icon already on.
        if !self.wait_until_completed(AFTER_SLEEP_ROUTE) {
            return BringUp::StillDown;
        }
        renew_lease(WIRELESS_LINK);
        if wait_for_route(AFTER_SLEEP_ROUTE) {
            return joined(BringUp::Associated);
        }
        renew_lease(WIRELESS_LINK);
        if wait_for_route(ASSOCIATE) {
            return joined(BringUp::Associated);
        }
        BringUp::StillDown
    }

    fn bring_up(&self) -> BringUp {
        let _guard = BRING_UP
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let rejoin = stale();
        if is_online(WIRELESS_LINK) && self.completed() && interface_enabled() {
            WANTED.store(true, Ordering::Relaxed);
            return joined(BringUp::AlreadyOnline);
        }
        if is_online(WIRELESS_LINK) && !rejoin {
            WANTED.store(true, Ordering::Relaxed);
            return BringUp::AlreadyOnline;
        }
        if !awake() {
            return BringUp::StillDown;
        }
        if rejoin && is_online(WIRELESS_LINK) && !self.completed() {
            // A route that outlived the association. Taking the interface
            // down drops it. A radio that is already off, or that wake just
            // turned on, is left alone: bouncing it is what made a fetch
            // after sleep find the link down.
            let _ignored = set_interface(false);
            thread::sleep(BOUNCE);
            if !awake() {
                return BringUp::StillDown;
            }
        }
        if !self.bring_radio_up() {
            return BringUp::StillDown;
        }

        let _ignored = self.command(["reconfigure"]);
        thread::sleep(SUPPLICANT_SETTLE);

        WANTED.store(true, Ordering::Relaxed);
        let within = if rejoin { AFTER_SLEEP_ROUTE } else { ASSOCIATE };

        if !rejoin && self.completed() {
            renew_lease(WIRELESS_LINK);
            if wait_for_route(within) {
                return joined(BringUp::Associated);
            }
            let _ignored = self.reconnect_saved();
            renew_lease(WIRELESS_LINK);
            return joined(if wait_for_route(within) {
                BringUp::Associated
            } else {
                BringUp::StillDown
            });
        }

        if self.reconnect_saved() && wait_for_route(within) {
            return joined(BringUp::Associated);
        }
        if !awake() {
            return BringUp::StillDown;
        }
        if handshake_in_progress(&self.wpa_state()) {
            renew_lease(WIRELESS_LINK);
            if wait_for_route(within) {
                return joined(BringUp::Associated);
            }
            return BringUp::StillDown;
        }

        if wanted() && interface_enabled() {
            // The session still wants the radio. Taking it down here is what
            // left a fetch, and then the stock reader, offline after wake.
            return BringUp::StillDown;
        }

        let _ignored = self.set_enabled(false);
        thread::sleep(BOUNCE);
        if !awake() {
            return BringUp::StillDown;
        }
        if !self.bring_radio_up() {
            return BringUp::StillDown;
        }
        if self.reconnect_saved() && wait_for_route(within) {
            joined(BringUp::Associated)
        } else if self.completed() {
            renew_lease(WIRELESS_LINK);
            joined(if wait_for_route(within) {
                BringUp::Associated
            } else {
                BringUp::StillDown
            })
        } else if self
            .list_networks()
            .is_ok_and(|list| !has_saved_network(&list))
        {
            BringUp::NoSavedNetwork
        } else {
            BringUp::StillDown
        }
    }

    fn bring_radio_up(&self) -> bool {
        let wait = if stale() {
            AFTER_SLEEP_LINK
        } else {
            INTERFACE_WAIT
        };
        let deadline = Instant::now() + wait;
        loop {
            if !awake() {
                return false;
            }
            if Path::new("/sys/class/net/wlan0").exists() && set_interface(true) {
                disable_power_save();
                return self.wait_for_supplicant(SUPPLICANT_WAIT);
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(ROUTE_POLL);
        }
    }

    fn wait_for_supplicant(&self, within: Duration) -> bool {
        let deadline = Instant::now() + within;
        loop {
            if !awake() {
                return false;
            }
            if self.command(["status"]).is_ok() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(ROUTE_POLL);
        }
    }

    fn wpa_state(&self) -> String {
        self.command(["status"])
            .ok()
            .as_deref()
            .and_then(|status| value(status, "wpa_state"))
            .unwrap_or_default()
            .to_owned()
    }

    fn completed(&self) -> bool {
        self.wpa_state() == "COMPLETED"
    }

    fn wait_until_completed(&self, within: Duration) -> bool {
        let deadline = Instant::now() + within;
        loop {
            if !awake() {
                return false;
            }
            if self.completed() {
                return true;
            }
            if Instant::now() >= deadline {
                return self.completed() && awake();
            }
            thread::sleep(ROUTE_POLL);
        }
    }

    fn list_networks(&self) -> Result<String, DeviceError> {
        match self.command(["list_networks"]) {
            Ok(list) => Ok(list),
            Err(error) => {
                thread::sleep(SUPPLICANT_SETTLE);
                self.command(["list_networks"]).or(Err(error))
            }
        }
    }

    fn reconnect_saved(&self) -> bool {
        reconnect_saved_with(|arguments| self.command_slice(arguments).is_ok())
    }

    fn command<const N: usize>(&self, arguments: [&str; N]) -> Result<String, DeviceError> {
        self.command_slice(&arguments)
    }

    fn command_slice(&self, arguments: &[&str]) -> Result<String, DeviceError> {
        let output = Command::new(&self.wpa_cli)
            .args(["-i", WIRELESS_LINK])
            .args(arguments)
            .output()
            .map_err(|_| DeviceError::Backend)?;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        if stdout.to_ascii_lowercase().contains("failed to connect") {
            return Err(DeviceError::Backend);
        }
        if output.status.success() && !stdout.lines().any(|line| line.trim() == "FAIL") {
            Ok(stdout)
        } else if stdout.to_ascii_lowercase().contains("password") {
            Err(DeviceError::Authentication)
        } else {
            Err(DeviceError::Backend)
        }
    }

    /// Sends credentials over stdin instead of process arguments, so another
    /// process inspecting `/proc/*/cmdline` cannot read the password.
    fn script(&self, commands: &str) -> Result<String, DeviceError> {
        let mut child = Command::new(&self.wpa_cli)
            .args(["-i", WIRELESS_LINK])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| DeviceError::Backend)?;
        child
            .stdin
            .take()
            .ok_or(DeviceError::Backend)?
            .write_all(commands.as_bytes())
            .map_err(|_| DeviceError::Backend)?;
        let output = child.wait_with_output().map_err(|_| DeviceError::Backend)?;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        if output.status.success() && !stdout.lines().any(|line| line.trim() == "FAIL") {
            Ok(stdout)
        } else if stdout.to_ascii_lowercase().contains("invalid") {
            Err(DeviceError::Authentication)
        } else {
            Err(DeviceError::Backend)
        }
    }
}

fn interface_enabled() -> bool {
    if let Ok(flags) = std::fs::read_to_string("/sys/class/net/wlan0/flags") {
        return u32::from_str_radix(flags.trim().trim_start_matches("0x"), 16)
            .is_ok_and(|flags| flags & 1 != 0);
    }
    std::fs::read_to_string("/sys/class/net/wlan0/operstate")
        .is_ok_and(|state| state.trim() != "down")
}

fn set_interface(enabled: bool) -> bool {
    let state = if enabled { "up" } else { "down" };
    for (tool, arguments) in [
        ("/sbin/ip", vec!["link", "set", WIRELESS_LINK, state]),
        ("/bin/ip", vec!["link", "set", WIRELESS_LINK, state]),
        ("/sbin/ifconfig", vec![WIRELESS_LINK, state]),
        ("/bin/ifconfig", vec![WIRELESS_LINK, state]),
    ] {
        if Path::new(tool).is_file()
            && Command::new(tool)
                .args(arguments)
                .status()
                .is_ok_and(|status| status.success())
        {
            return true;
        }
    }
    false
}

/// Stops the chip from dropping an association the session still wants.
///
/// Nickel keeps the radio from power-saving while it is awake. Without that,
/// `MediaTek`'s driver associates, looks idle, and takes the link down a short
/// time later — which is exactly "it worked after sleep, then it went".
fn disable_power_save() {
    for tool in ["/sbin/iwconfig", "/bin/iwconfig"] {
        if Path::new(tool).is_file() {
            let _ignored = Command::new(tool).args(["wlan0", "power", "off"]).status();
            return;
        }
    }
    for tool in ["/sbin/iw", "/bin/iw"] {
        if Path::new(tool).is_file() {
            let _ignored = Command::new(tool)
                .args(["dev", "wlan0", "set", "power_save", "off"])
                .status();
            return;
        }
    }
}

fn association_recovery_commands() -> [[&'static str; 1]; 3] {
    [["scan"], ["reassociate"], ["reconnect"]]
}

fn parse_scan_results(output: &str, connected: Option<&str>) -> Vec<WifiNetwork> {
    let mut networks = Vec::new();
    for line in output
        .lines()
        .skip_while(|line| !line.contains("bssid"))
        .skip(1)
    {
        let fields = line.split('\t').collect::<Vec<_>>();
        if fields.len() < 5 {
            continue;
        }
        let ssid = fields[4].trim();
        if ssid.is_empty() || ssid.len() > 32 {
            continue;
        }
        let signal_dbm = fields[2]
            .parse::<i16>()
            .ok()
            .or_else(|| signal_dbm(WIRELESS_LINK).and_then(|value| i16::try_from(value).ok()))
            .unwrap_or(-100);
        let flags = fields[3];
        if let Some(existing) = networks
            .iter_mut()
            .find(|network: &&mut WifiNetwork| network.ssid == ssid)
        {
            if signal_dbm > existing.signal_dbm {
                existing.signal_dbm = signal_dbm;
            }
            continue;
        }
        networks.push(WifiNetwork {
            ssid: ssid.to_owned(),
            signal_dbm,
            secured: !flags.contains("[ESS]") || flags.contains("WPA") || flags.contains("WEP"),
            connected: connected == Some(ssid),
        });
    }
    networks.sort_by_key(|network| std::cmp::Reverse(network.signal_dbm));
    networks.truncate(MAX_RADIO_DEVICES);
    networks
}

fn value<'a>(status: &'a str, wanted: &str) -> Option<&'a str> {
    status.lines().find_map(|line| {
        let (name, value) = line.split_once('=')?;
        (name == wanted).then_some(value)
    })
}

fn valid_credentials(ssid: &str, password: &str) -> bool {
    !ssid.is_empty()
        && ssid.len() <= 32
        && (password.is_empty() || (8..=63).contains(&password.len()))
        && ssid.chars().all(|character| !character.is_control())
        && password.chars().all(|character| !character.is_control())
}

/// True when `wpa_cli list_networks` names at least one remembered network.
///
/// Disabled networks count: they are still saved, and `enable_network all`
/// before `reconnect` puts them back in play. An empty list, or only the
/// header, is a reader that has never joined anything.
fn has_saved_network(list: &str) -> bool {
    list.lines().any(|line| {
        let line = line.trim();
        line.bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_digit())
    })
}

/// States in which bouncing the radio would throw away work already in flight.
fn handshake_in_progress(state: &str) -> bool {
    matches!(
        state,
        "ASSOCIATING" | "ASSOCIATED" | "4WAY_HANDSHAKE" | "GROUP_HANDSHAKE" | "COMPLETED"
    )
}

fn wait_for_route(within: Duration) -> bool {
    let deadline = Instant::now() + within;
    loop {
        if !awake() {
            return false;
        }
        if is_online(WIRELESS_LINK) {
            return true;
        }
        if Instant::now() >= deadline {
            return is_online(WIRELESS_LINK) && awake();
        }
        thread::sleep(ROUTE_POLL);
    }
}

fn reconnect_saved_with(mut command: impl FnMut(&[&str]) -> bool) -> bool {
    let _ignored = command(&["reconfigure"]);
    let _ignored = command(&["enable_network", "all"]);
    // Let wpa_supplicant choose among all saved networks by priority and
    // signal. `select_network` disables every other entry and pinning the
    // first id made wake reconnect to the wrong access point—or none.
    command(&["reassociate"]) || command(&["reconnect"])
}

fn quote(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    for character in value.chars() {
        if matches!(character, '"' | '\\') {
            quoted.push('\\');
        }
        quoted.push(character);
    }
    quoted.push('"');
    quoted
}

#[cfg(test)]
mod tests {
    use super::{
        association_recovery_commands, handshake_in_progress, has_saved_network,
        parse_scan_results, quote, reconnect_saved_with, valid_credentials, value,
    };

    #[test]
    fn sleep_makes_a_surviving_route_untrusted_until_a_join() {
        super::set_awake(false);
        assert!(super::stale());
        super::set_awake(true);
        assert!(super::stale(), "waking must still rejoin");
        super::joined(super::BringUp::Associated);
        assert!(!super::stale());
        super::set_awake(true);
        assert!(!super::stale(), "a second wake without sleep is not stale");
    }

    #[test]
    fn association_recovery_matches_the_stock_network_screen_sequence() {
        assert_eq!(
            association_recovery_commands(),
            [["scan"], ["reassociate"], ["reconnect"]]
        );
    }

    #[test]
    fn a_wpa_scan_is_sorted_and_deduplicated() {
        let scan = "bssid / frequency / signal level / flags / ssid\n\
                    aa\t2412\t-70\t[WPA2-PSK-CCMP][ESS]\tHome\n\
                    bb\t5180\t-42\t[WPA2-PSK-CCMP][ESS]\tHome\n\
                    cc\t2412\t-55\t[ESS]\tCafe\n";
        let networks = parse_scan_results(scan, Some("Home"));
        assert_eq!(networks.len(), 2);
        assert_eq!(networks[0].ssid, "Home");
        assert_eq!(networks[0].signal_dbm, -42);
        assert!(networks[0].connected);
        assert!(!networks[1].secured);
    }

    #[test]
    fn credentials_are_quoted_for_wpa_without_becoming_commands() {
        assert_eq!(quote("say \"hi\"\\now"), "\"say \\\"hi\\\"\\\\now\"");
    }

    #[test]
    fn status_values_are_exact_keys() {
        assert_eq!(value("ssid=Home\nbssid=x\n", "ssid"), Some("Home"));
    }

    #[test]
    fn wifi_passwords_are_open_or_wpa_length() {
        assert!(valid_credentials("Cafe", ""));
        assert!(!valid_credentials("Home", "short"));
        assert!(valid_credentials("Home", "password"));
    }

    #[test]
    fn a_supplicant_list_with_a_network_is_a_saved_network() {
        let list = "network id / ssid / bssid / flags\n\
                    0\tHome\tany\t[CURRENT]\n\
                    1\tCafe\tany\t[DISABLED]\n";
        assert!(has_saved_network(list));
    }

    #[test]
    fn an_empty_supplicant_list_is_not_a_saved_network() {
        assert!(!has_saved_network(
            "network id / ssid / bssid / flags\nSelected interface: 'wlan0'\n"
        ));
        assert!(!has_saved_network(""));
    }

    #[test]
    fn a_handshake_is_not_bounced() {
        assert!(handshake_in_progress("4WAY_HANDSHAKE"));
        assert!(handshake_in_progress("COMPLETED"));
        assert!(!handshake_in_progress("DISCONNECTED"));
        assert!(!handshake_in_progress("INACTIVE"));
    }

    #[test]
    fn reconnect_keeps_every_saved_network_eligible() {
        let mut calls = Vec::new();
        let connected = reconnect_saved_with(|arguments| {
            calls.push(arguments.join(" "));
            arguments != ["reassociate"]
        });
        assert!(connected, "reconnect fallback did not succeed");
        assert_eq!(
            calls,
            [
                "reconfigure",
                "enable_network all",
                "reassociate",
                "reconnect"
            ]
        );
        assert!(calls.iter().all(|call| !call.starts_with("select_network")));
    }
}

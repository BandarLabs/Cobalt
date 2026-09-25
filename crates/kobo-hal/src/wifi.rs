//! Wi-Fi control through the firmware's running `wpa_supplicant`.
//!
//! This module never starts a second supplicant. Nickel and Cobalt would then
//! be two owners of one interface, an arrangement already proven unsafe on the
//! Clara BW. The backend is available only when the firmware's `wpa_cli` and
//! the device's wireless interface (detected by [`crate::network::wireless_link`],
//! not assumed to be `wlan0`) are both present; all operations go through that
//! existing owner.

use crate::network::{signal_dbm, wireless_link};
use kobo_protocol::{DeviceError, DeviceResult, WifiNetwork, MAX_RADIO_DEVICES};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// How long a certificate probe waits for the server to present itself.
/// PEAP reaches the certificate within a couple of round trips once the
/// association completes. Device requests are answered on the runtime's main
/// loop, so this matches the longest wait Bluetooth commands already take
/// there rather than setting a new worst case.
const PROBE_TIMEOUT: Duration = Duration::from_secs(18);

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
    #[must_use]
    pub fn open() -> Option<Self> {
        if !Path::new("/sys/class/net").join(wireless_link()).exists() {
            return None;
        }
        WPA_TOOLS
            .into_iter()
            .map(Path::new)
            .find(|path| path.is_file())
            .map(|path| Self {
                wpa_cli: path.to_path_buf(),
            })
    }

    #[must_use]
    pub fn state(&self) -> DeviceResult {
        let status = match self.command(["status"]) {
            Ok(status) => status,
            Err(error) => return DeviceResult::Failed(error),
        };
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
            if !set_interface(true) {
                return DeviceResult::Failed(DeviceError::Backend);
            }
            if let Err(error) = self.command(["reconnect"]) {
                return DeviceResult::Failed(error);
            }
        } else {
            if let Err(error) = self.command(["disconnect"]) {
                return DeviceResult::Failed(error);
            }
            if !set_interface(false) {
                return DeviceResult::Failed(DeviceError::Backend);
            }
        }
        self.state()
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

    /// Asks an enterprise network for its RADIUS server certificate.
    ///
    /// Uses the supplicant's own `probe://` mode: the TLS handshake stops as
    /// soon as the server has presented its certificate, so no password is
    /// configured, let alone sent. Only the outer, anonymous identity leaves
    /// the reader. The temporary network is removed afterwards and the
    /// networks that were enabled before are enabled again.
    #[must_use]
    pub fn probe_enterprise(&self, ssid: &str, identity: &str) -> DeviceResult {
        if !valid_enterprise(ssid, identity, None) {
            return DeviceResult::Failed(DeviceError::InvalidInput);
        }
        if !set_interface(true) {
            return DeviceResult::Failed(DeviceError::Backend);
        }
        let enabled_before = self
            .command(["list_networks"])
            .map(|list| enabled_network_ids(&list))
            .unwrap_or_default();
        let network = match self.add_network() {
            Ok(network) => network,
            Err(error) => return DeviceResult::Failed(error),
        };
        let configure = enterprise_commands(network, ssid, identity, None, "probe://");
        let result = self
            .script(&configure)
            .and_then(|_| self.watch_for_certificate(network));
        let id = network.to_string();
        let _ = self.command(["remove_network", id.as_str()]);
        for id in &enabled_before {
            let _ = self.command(["enable_network", id.as_str()]);
        }
        let _ = self.command(["reconnect"]);
        match result {
            Ok((subject, sha256)) => DeviceResult::WifiCertificate { subject, sha256 },
            Err(error) => DeviceResult::Failed(error),
        }
    }

    /// Joins an enterprise network with PEAP/MSCHAPv2, trusting only the
    /// server certificate whose SHA-256 is `server_sha256`.
    ///
    /// The pin is the whole of the trust decision: Kobo firmware ships no CA
    /// bundle, so without it any access point calling itself eduroam could
    /// complete the handshake and collect the password.
    #[must_use]
    pub fn join_enterprise(
        &self,
        ssid: &str,
        identity: &str,
        password: &str,
        server_sha256: &[u8; 32],
    ) -> DeviceResult {
        if !valid_enterprise(ssid, identity, Some(password)) {
            return DeviceResult::Failed(DeviceError::InvalidInput);
        }
        if !set_interface(true) {
            return DeviceResult::Failed(DeviceError::Backend);
        }
        let network = match self.add_network() {
            Ok(network) => network,
            Err(error) => return DeviceResult::Failed(error),
        };
        let ca_cert = format!("hash://server/sha256/{}", hex(server_sha256));
        let mut commands =
            enterprise_commands(network, ssid, identity, Some(password), ca_cert.as_str());
        commands.push_str(&format!("select_network {network}\nsave_config\nquit\n"));
        match self.script(&commands) {
            Ok(_) => self.state(),
            Err(error) => {
                let network = network.to_string();
                let _ = self.command(["remove_network", network.as_str()]);
                DeviceResult::Failed(error)
            }
        }
    }

    fn add_network(&self) -> Result<u32, DeviceError> {
        self.command(["add_network"]).and_then(|output| {
            output
                .lines()
                .rev()
                .find_map(|line| line.trim().parse::<u32>().ok())
                .ok_or(DeviceError::Backend)
        })
    }

    /// Selects `network` while an attached `wpa_cli` listens for the
    /// supplicant's certificate event, and returns the leaf certificate's
    /// subject and digest.
    fn watch_for_certificate(&self, network: u32) -> Result<(String, [u8; 32]), DeviceError> {
        // With no command, wpa_cli runs interactively and attaches to the
        // control socket, so events are printed as they happen. Its stdin is
        // held open for the whole watch: EOF would end the session.
        let mut monitor = Command::new(&self.wpa_cli)
            .args(["-i", wireless_link()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| DeviceError::Backend)?;
        let stdout = monitor.stdout.take().ok_or(DeviceError::Backend)?;
        let (lines, received) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if lines.send(line).is_err() {
                    break;
                }
            }
        });
        let id = network.to_string();
        let selected = self.command(["select_network", id.as_str()]);
        let deadline = Instant::now() + PROBE_TIMEOUT;
        let mut found = None;
        let mut failure = DeviceError::TimedOut;
        if selected.is_ok() {
            while let Some(left) = deadline.checked_duration_since(Instant::now()) {
                let Ok(line) = received.recv_timeout(left) else {
                    break;
                };
                if let Some(certificate) = peer_certificate(&line) {
                    found = Some(certificate);
                    break;
                }
                if line.contains("CTRL-EVENT-SSID-TEMP-DISABLED")
                    || line.contains("CTRL-EVENT-NETWORK-NOT-FOUND")
                {
                    failure = DeviceError::NotFound;
                }
            }
        } else {
            failure = DeviceError::Backend;
        }
        let _ = monitor.kill();
        let _ = monitor.wait();
        found.ok_or(failure)
    }

    #[must_use]
    pub fn disconnect(&self) -> DeviceResult {
        match self.command(["disconnect"]) {
            Ok(_) => self.state(),
            Err(error) => DeviceResult::Failed(error),
        }
    }

    fn command<const N: usize>(&self, arguments: [&str; N]) -> Result<String, DeviceError> {
        let output = Command::new(&self.wpa_cli)
            .args(["-i", wireless_link()])
            .args(arguments)
            .output()
            .map_err(|_| DeviceError::Backend)?;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
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
            .args(["-i", wireless_link()])
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
    let link = wireless_link();
    if let Ok(flags) = std::fs::read_to_string(format!("/sys/class/net/{link}/flags")) {
        return u32::from_str_radix(flags.trim().trim_start_matches("0x"), 16)
            .is_ok_and(|flags| flags & 1 != 0);
    }
    std::fs::read_to_string(format!("/sys/class/net/{link}/operstate"))
        .is_ok_and(|state| state.trim() != "down")
}

fn set_interface(enabled: bool) -> bool {
    let state = if enabled { "up" } else { "down" };
    for (tool, arguments) in [
        ("/sbin/ip", vec!["link", "set", wireless_link(), state]),
        ("/bin/ip", vec!["link", "set", wireless_link(), state]),
        ("/sbin/ifconfig", vec![wireless_link(), state]),
        ("/bin/ifconfig", vec![wireless_link(), state]),
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
            .or_else(|| signal_dbm(wireless_link()).and_then(|value| i16::try_from(value).ok()))
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

/// The `wpa_cli` lines that describe one PEAP/MSCHAPv2 network.
///
/// Every string is sent hex-encoded, the one form `wpa_supplicant`'s config
/// parser accepts for arbitrary bytes: a quoted value is taken verbatim up to
/// its last quote with no escaping, so a password containing `"` would
/// otherwise be silently truncated. The outer identity is anonymous at the
/// same realm, so the username only travels inside the TLS tunnel.
fn enterprise_commands(
    network: u32,
    ssid: &str,
    identity: &str,
    password: Option<&str>,
    ca_cert: &str,
) -> String {
    let mut commands = format!(
        "set_network {network} ssid {}\n\
         set_network {network} scan_ssid 1\n\
         set_network {network} key_mgmt WPA-EAP\n\
         set_network {network} eap PEAP\n\
         set_network {network} phase2 \"auth=MSCHAPV2\"\n\
         set_network {network} identity {}\n\
         set_network {network} anonymous_identity {}\n\
         set_network {network} ca_cert \"{ca_cert}\"\n",
        hex(ssid.as_bytes()),
        hex(identity.as_bytes()),
        hex(anonymous_identity(identity).as_bytes()),
    );
    if let Some(password) = password {
        commands.push_str(&format!(
            "set_network {network} password {}\n",
            hex(password.as_bytes())
        ));
    }
    commands.push_str(&format!("enable_network {network}\n"));
    commands
}

/// `anonymous@realm`, keeping the realm so eduroam can route the request to
/// the home institution; plain `anonymous` when there is no realm.
fn anonymous_identity(identity: &str) -> String {
    identity
        .rsplit_once('@')
        .filter(|(_, realm)| !realm.is_empty())
        .map_or_else(
            || "anonymous".to_owned(),
            |(_, realm)| format!("anonymous@{realm}"),
        )
}

fn valid_enterprise(ssid: &str, identity: &str, password: Option<&str>) -> bool {
    !ssid.is_empty()
        && ssid.len() <= 32
        && ssid.chars().all(|character| !character.is_control())
        && kobo_protocol::valid_wifi_identity(identity)
        && password.is_none_or(kobo_protocol::valid_wifi_secret)
}

/// Reads the leaf certificate out of a `CTRL-EVENT-EAP-PEER-CERT` event.
///
/// The supplicant reports every certificate in the chain; only `depth=0`,
/// the server's own, is the one a `hash://server/sha256/` pin is checked
/// against. The digest comes from the event's `hash=` field, which is present
/// in `probe://` mode.
fn peer_certificate(line: &str) -> Option<(String, [u8; 32])> {
    let event = line
        .find("CTRL-EVENT-EAP-PEER-CERT ")
        .map(|start| &line[start + "CTRL-EVENT-EAP-PEER-CERT ".len()..])?;
    if !event.starts_with("depth=0 ") {
        return None;
    }
    let subject_start = event.find("subject='")? + "subject='".len();
    let subject_end = subject_start + event[subject_start..].rfind('\'')?;
    let subject = event[subject_start..subject_end].to_owned();
    let digest = event[subject_end..]
        .split_ascii_whitespace()
        .find_map(|field| field.strip_prefix("hash="))?;
    let sha256 = unhex(digest)?;
    (!subject.is_empty()
        && subject.len() <= kobo_protocol::MAX_CERT_SUBJECT
        && !subject.chars().any(char::is_control))
    .then_some((subject, sha256))
}

/// Network ids `list_networks` reports as not disabled.
fn enabled_network_ids(list: &str) -> Vec<String> {
    list.lines()
        .skip(1)
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let id = fields.next()?.trim();
            let flags = line.rsplit('\t').next().unwrap_or_default();
            (id.parse::<u32>().is_ok() && !flags.contains("[DISABLED]")).then(|| id.to_owned())
        })
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        })
}

fn unhex(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 {
        return None;
    }
    let mut digest = [0_u8; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        *byte = u8::from_str_radix(text.get(index * 2..index * 2 + 2)?, 16).ok()?;
    }
    Some(digest)
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
        anonymous_identity, association_recovery_commands, enabled_network_ids,
        enterprise_commands, hex, parse_scan_results, peer_certificate, quote, unhex,
        valid_credentials, valid_enterprise, value,
    };

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
    fn the_leaf_certificate_is_read_from_the_probe_event() {
        let digest = "5e".repeat(32);
        let line = format!(
            "<3>CTRL-EVENT-EAP-PEER-CERT depth=0 subject='/C=GB/O=The University of Edinburgh/CN=radius.example.ed.ac.uk' hash={digest}"
        );
        let (subject, sha256) = peer_certificate(&line).expect("a certificate");
        assert_eq!(
            subject,
            "/C=GB/O=The University of Edinburgh/CN=radius.example.ed.ac.uk"
        );
        assert_eq!(sha256, [0x5e; 32]);
    }

    #[test]
    fn intermediate_and_hashless_certificates_are_not_pinned() {
        let digest = "5e".repeat(32);
        assert!(peer_certificate(&format!(
            "<3>CTRL-EVENT-EAP-PEER-CERT depth=1 subject='/CN=Some CA' hash={digest}"
        ))
        .is_none());
        assert!(
            peer_certificate("<3>CTRL-EVENT-EAP-PEER-CERT depth=0 subject='/CN=x' cert=3082")
                .is_none()
        );
        assert!(peer_certificate(&format!(
            "<3>CTRL-EVENT-EAP-PEER-CERT depth=0 subject='/CN=x' hash={}",
            "5e".repeat(31)
        ))
        .is_none());
    }

    #[test]
    fn a_subject_containing_quotes_is_kept_whole() {
        let digest = "01".repeat(32);
        let line = format!(
            "CTRL-EVENT-EAP-PEER-CERT depth=0 subject='/O=King's College/CN=r' hash={digest}"
        );
        assert_eq!(
            peer_certificate(&line)
                .map(|(subject, _)| subject)
                .as_deref(),
            Some("/O=King's College/CN=r")
        );
    }

    #[test]
    fn the_outer_identity_keeps_only_the_realm() {
        assert_eq!(
            anonymous_identity("s1234567@ed.ac.uk"),
            "anonymous@ed.ac.uk"
        );
        assert_eq!(anonymous_identity("s1234567"), "anonymous");
        assert_eq!(anonymous_identity("s1234567@"), "anonymous");
    }

    #[test]
    fn enterprise_secrets_travel_hex_encoded_and_pinned() {
        let commands = enterprise_commands(
            3,
            "eduroam",
            "s1@ed.ac.uk",
            Some("pa\"ss"),
            "hash://server/sha256/00",
        );
        assert!(commands.contains(&format!("set_network 3 ssid {}", hex(b"eduroam"))));
        assert!(commands.contains(&format!("set_network 3 password {}", hex(b"pa\"ss"))));
        assert!(
            !commands.contains("pa\"ss"),
            "the password never appears raw"
        );
        assert!(commands.contains(&format!(
            "set_network 3 anonymous_identity {}",
            hex(b"anonymous@ed.ac.uk")
        )));
        assert!(commands.contains("set_network 3 eap PEAP"));
        assert!(commands.contains("set_network 3 phase2 \"auth=MSCHAPV2\""));
        assert!(commands.contains("set_network 3 ca_cert \"hash://server/sha256/00\""));
        // One command per line: nothing a credential contains can start a
        // second one, because none of it is sent as text.
        assert!(commands
            .lines()
            .all(|line| line.starts_with("set_network 3 ") || line == "enable_network 3"));
    }

    #[test]
    fn a_probe_configures_no_password() {
        let commands = enterprise_commands(0, "eduroam", "s1@ed.ac.uk", None, "probe://");
        assert!(!commands.contains("password"));
        assert!(commands.contains("ca_cert \"probe://\""));
    }

    #[test]
    fn enterprise_input_is_validated_before_any_command() {
        assert!(valid_enterprise("eduroam", "s1@ed.ac.uk", Some("x")));
        assert!(valid_enterprise("eduroam", "s1@ed.ac.uk", None));
        assert!(!valid_enterprise("", "s1@ed.ac.uk", None));
        assert!(!valid_enterprise("eduroam", "", None));
        assert!(!valid_enterprise("eduroam", "s1@ed.ac.uk", Some("")));
        assert!(!valid_enterprise("eduroam", "s1@ed.ac.uk", Some("a\nb")));
    }

    #[test]
    fn previously_enabled_networks_are_found_for_restoring() {
        let list = "network id / ssid / bssid / flags\n\
                    0\tHome\tany\t[CURRENT]\n\
                    1\tCafe\tany\t[DISABLED]\n\
                    2\tLibrary\tany\t\n";
        assert_eq!(enabled_network_ids(list), vec!["0", "2"]);
    }

    #[test]
    fn digests_round_trip_through_hex() {
        let digest = [0xa5; 32];
        assert_eq!(unhex(&hex(&digest)), Some(digest));
        assert_eq!(unhex("zz"), None);
    }
}

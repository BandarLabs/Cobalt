//! Wi-Fi control through the firmware's running `wpa_supplicant`.
//!
//! This module never starts a second supplicant. Nickel and Cobalt would then
//! be two owners of one interface, an arrangement already proven unsafe on the
//! Clara BW. The backend exists whenever the firmware's `wpa_cli` does, and
//! the interface is resolved on every exchange rather than at open. A radio
//! whose driver the stock reader had not loaded when the session began, or
//! that comes back under another name, is then usable as soon as it appears,
//! instead of the session reporting Wi-Fi unsupported until the next one.
//! Missing or ambiguous interfaces are refused per request, never guessed.
//! All operations go through the existing firmware owner.

use kobo_protocol::{DeviceError, DeviceResult, WifiNetwork, MAX_RADIO_DEVICES};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// How long one `wpa_cli` exchange may take before it counts as hung.
///
/// Requests are answered on the session loop, so this is also the longest a
/// stuck supplicant can freeze touch and drawing per command. A healthy reply
/// arrives in milliseconds.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(3);

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
        WPA_TOOLS
            .into_iter()
            .map(Path::new)
            .find(|path| path.is_file())
            .map(|path| Self {
                wpa_cli: path.to_path_buf(),
            })
    }

    /// The radio's interface as it is now.
    ///
    /// `NotFound` rather than `Backend` when there is none, because that is a
    /// radio the stock reader has not brought up, not a supplicant that failed.
    fn link() -> Result<String, DeviceError> {
        discover_link(Path::new("/sys/class/net")).ok_or(DeviceError::NotFound)
    }

    #[must_use]
    pub fn state(&self) -> DeviceResult {
        // No interface is a radio that is off as far as anyone here can tell,
        // not a missing feature: reporting it as unavailable would tell
        // Settings to stop asking, and the radio can appear a moment later.
        let Ok(link) = Self::link() else {
            return DeviceResult::Wifi {
                available: true,
                enabled: false,
                connected_ssid: None,
                networks: Vec::new(),
            };
        };
        let status = match self.command(["status"]) {
            Ok(status) => status,
            Err(error) => return DeviceResult::Failed(error),
        };
        let completed = value(&status, "wpa_state").is_some_and(|state| state == "COMPLETED");
        DeviceResult::Wifi {
            available: true,
            enabled: interface_enabled(&link),
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
        if !set_interface(&Self::link()?, true) {
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
        let link = match Self::link() {
            Ok(link) => link,
            Err(error) => return DeviceResult::Failed(error),
        };
        if enabled {
            if !set_interface(&link, true) {
                return DeviceResult::Failed(DeviceError::Backend);
            }
            if let Err(error) = self.command(["reconnect"]) {
                return DeviceResult::Failed(error);
            }
        } else {
            if let Err(error) = self.command(["disconnect"]) {
                return DeviceResult::Failed(error);
            }
            if !set_interface(&link, false) {
                return DeviceResult::Failed(DeviceError::Backend);
            }
        }
        self.state()
    }

    /// Starts a scan and answers with the results the supplicant already has.
    ///
    /// Does not wait for the scan it starts. Requests are answered on the
    /// session loop and a radio scan takes seconds, so waiting here would
    /// freeze touch and drawing for all of it, and Settings asks again every
    /// few seconds while the list is on screen. The scan one request starts is
    /// what the next request reports. `FAIL-BUSY` means a scan is already
    /// running, often the supplicant's own while it is not associated, and it
    /// fills the same results, so it is not a failure either.
    #[must_use]
    pub fn scan(&self) -> DeviceResult {
        let Ok(link) = Self::link() else {
            return self.state();
        };
        if !interface_enabled(&link) {
            return self.state();
        }
        if let Err(error) = self.request_scan() {
            return DeviceResult::Failed(error);
        }
        let results = match self.command(["scan_results"]) {
            Ok(results) => results,
            Err(error) => return DeviceResult::Failed(error),
        };
        let status = match self.command(["status"]) {
            Ok(status) => status,
            Err(error) => return DeviceResult::Failed(error),
        };
        let connected = connected_ssid(&status);
        DeviceResult::Wifi {
            available: true,
            enabled: interface_enabled(&link),
            connected_ssid: connected.map(str::to_owned),
            networks: parse_scan_results(&results, connected),
        }
    }

    #[must_use]
    pub fn join(&self, ssid: &str, password: &str) -> DeviceResult {
        if !valid_credentials(ssid, password) {
            return DeviceResult::Failed(DeviceError::InvalidInput);
        }
        let link = match Self::link() {
            Ok(link) => link,
            Err(error) => return DeviceResult::Failed(error),
        };
        if !set_interface(&link, true) {
            return DeviceResult::Failed(DeviceError::Backend);
        }
        let previous = match self
            .command(["list_networks"])
            .and_then(|list| saved_networks(&list))
        {
            Ok(previous) => previous,
            Err(error) => return DeviceResult::Failed(error),
        };
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
        let id = network.to_string();
        let (result, selected) =
            configure_join(network, ssid, password, |command| self.script(command));
        match result {
            Ok(()) => self.state(),
            Err(error) => {
                let _ = self.command(["remove_network", &id]);
                if selected {
                    for (old, current) in &previous {
                        if *current {
                            let _ = self.command(["select_network", old]);
                        }
                    }
                    for (old, _) in &previous {
                        let _ = self.command(["enable_network", old]);
                    }
                }
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

    fn command<const N: usize>(&self, arguments: [&str; N]) -> Result<String, DeviceError> {
        let link = Self::link()?;
        let mut command = Command::new(&self.wpa_cli);
        command.args(["-i", link.as_str()]).args(arguments);
        let reply = super::wifi_process::run(&mut command, b"", COMMAND_TIMEOUT)
            .map_err(|error| process_error(&error))?;
        let stdout = checked_reply(reply.text)?;
        if !reply.success {
            return Err(DeviceError::Backend);
        }
        if matches!(
            arguments.first(),
            Some(&"status" | &"scan_results" | &"list_networks" | &"add_network")
        ) || reply_ok(&stdout)
        {
            Ok(stdout)
        } else {
            Err(DeviceError::Backend)
        }
    }

    /// Credentials are delivered only through the bounded private input pipe.
    fn script(&self, commands: &str) -> Result<String, DeviceError> {
        let link = Self::link()?;
        let mut command = Command::new(&self.wpa_cli);
        command.args(["-i", link.as_str()]);
        let reply = super::wifi_process::run(&mut command, commands.as_bytes(), COMMAND_TIMEOUT)
            .map_err(|error| process_error(&error))?;
        let stdout = checked_reply(reply.text)?;
        if !reply.success {
            return Err(DeviceError::Backend);
        }
        if reply_ok(&stdout) {
            Ok(stdout)
        } else {
            Err(DeviceError::Backend)
        }
    }

    fn request_scan(&self) -> Result<(), DeviceError> {
        let link = Self::link()?;
        let mut command = Command::new(&self.wpa_cli);
        command.args(["-i", link.as_str(), "scan"]);
        let reply = super::wifi_process::run(&mut command, b"", COMMAND_TIMEOUT)
            .map_err(|error| process_error(&error))?;
        scan_accepted(&reply.text)
    }
}

/// Issue one mutation per acknowledged exchange; never batch past a failure.
fn configure_join(
    network: u32,
    ssid: &str,
    password: &str,
    mut execute: impl FnMut(&str) -> Result<String, DeviceError>,
) -> (Result<(), DeviceError>, bool) {
    let security = if password.is_empty() {
        "key_mgmt NONE".to_owned()
    } else {
        format!("psk {}", quote(password))
    };
    let commands = [
        format!("set_network {network} ssid {}", quote(ssid)),
        format!("set_network {network} {security}"),
        format!("select_network {network}"),
        "save_config".to_owned(),
    ];
    for (step, command) in commands.iter().enumerate() {
        if let Err(error) = execute(&format!("{command}\nquit\n")) {
            return (Err(error), step >= 2);
        }
    }
    (Ok(()), true)
}

fn process_error(error: &std::io::Error) -> DeviceError {
    if error.kind() == std::io::ErrorKind::TimedOut {
        DeviceError::TimedOut
    } else {
        DeviceError::Backend
    }
}

fn saved_networks(list: &str) -> Result<Vec<(String, bool)>, DeviceError> {
    let mut lines = list
        .lines()
        .skip_while(|line| !line.starts_with("network id"));
    if lines.next().is_none() {
        return Err(DeviceError::Backend);
    }
    let mut networks = Vec::new();
    for line in lines {
        let fields = line.split('\t').collect::<Vec<_>>();
        if fields.len() != 4 || fields[0].parse::<u32>().is_err() {
            return Err(DeviceError::Backend);
        }
        if !fields[3].contains("[DISABLED]") {
            networks.push((fields[0].to_owned(), fields[3].contains("[CURRENT]")));
        }
        if networks.len() > 32 {
            return Err(DeviceError::Backend);
        }
    }
    Ok(networks)
}

fn discover_link(root: &Path) -> Option<String> {
    super::network::wireless_link_in(root)
}

fn reply_ok(output: &str) -> bool {
    output
        .lines()
        .any(|line| line.trim().trim_start_matches("> ") == "OK")
}

fn checked_reply(output: String) -> Result<String, DeviceError> {
    if output.trim().is_empty()
        || output.lines().any(|line| {
            let line = line.trim().trim_start_matches("> ");
            line.starts_with("FAIL") || line.starts_with("UNKNOWN COMMAND")
        })
    {
        // Not `Authentication`, whatever the words say. A wrong password is
        // refused by the access point during the handshake, after every
        // command here has already answered `OK`, so no command reply can
        // report one. Guessing it from words like "invalid" showed people a
        // wrong-password error for malformed commands.
        Err(DeviceError::Backend)
    } else {
        Ok(output)
    }
}

/// Whether the supplicant took the scan request, or already has one running.
///
/// Read from the reply rather than the exit status, which differs between
/// `wpa_cli` builds for `FAIL-BUSY`.
fn scan_accepted(reply: &str) -> Result<(), DeviceError> {
    match reply.lines().map(str::trim).find(|line| !line.is_empty()) {
        Some("OK" | "FAIL-BUSY") => Ok(()),
        _ => Err(DeviceError::Backend),
    }
}

fn connected_ssid(status: &str) -> Option<&str> {
    (value(status, "wpa_state") == Some("COMPLETED"))
        .then(|| value(status, "ssid"))
        .flatten()
}

fn interface_enabled(link: &str) -> bool {
    if let Ok(flags) = std::fs::read_to_string(format!("/sys/class/net/{link}/flags")) {
        return u32::from_str_radix(flags.trim().trim_start_matches("0x"), 16)
            .is_ok_and(|flags| flags & 1 != 0);
    }
    std::fs::read_to_string(format!("/sys/class/net/{link}/operstate"))
        .is_ok_and(|state| state.trim() != "down")
}

fn set_interface(link: &str, enabled: bool) -> bool {
    if discover_link(Path::new("/sys/class/net")).as_deref() != Some(link) {
        return false;
    }
    let state = if enabled { "up" } else { "down" };
    for (tool, arguments) in [
        ("/sbin/ip", vec!["link", "set", link, state]),
        ("/bin/ip", vec!["link", "set", link, state]),
        ("/sbin/ifconfig", vec![link, state]),
        ("/bin/ifconfig", vec![link, state]),
    ] {
        if Path::new(tool).is_file()
            && super::wifi_process::run(Command::new(tool).args(arguments), b"", COMMAND_TIMEOUT)
                .is_ok_and(|reply| reply.success)
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
        let signal_dbm = fields[2].parse::<i16>().ok().unwrap_or(-100);
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
        association_recovery_commands, parse_scan_results, quote, valid_credentials, value,
    };

    #[test]
    fn join_stops_at_each_failed_step_before_later_mutations() {
        for failing_step in 0..4 {
            let mut seen = Vec::new();
            let (result, selected) = super::configure_join(7, "Home", "password", |command| {
                seen.push(command.to_owned());
                if seen.len() == failing_step + 1 {
                    Err(kobo_protocol::DeviceError::Backend)
                } else {
                    Ok("OK".into())
                }
            });
            assert!(result.is_err());
            assert_eq!(seen.len(), failing_step + 1);
            assert_eq!(selected, failing_step >= 2);
            if failing_step < 3 {
                assert!(!seen.iter().any(|s| s.contains("save_config")));
            }
        }
    }

    #[test]
    fn a_failed_command_is_never_reported_as_a_wrong_password() {
        // A wrong password is refused during the handshake, after every
        // command has already said OK, so these replies cannot mean one.
        for reply in [
            "FAIL\n",
            "FAIL invalid argument\n",
            "> FAIL password too short\n",
        ] {
            assert_eq!(
                super::checked_reply(reply.to_owned()),
                Err(kobo_protocol::DeviceError::Backend),
                "{reply:?}"
            );
        }
    }

    #[test]
    fn a_scan_already_running_counts_as_accepted_and_anything_else_fails() {
        assert_eq!(super::scan_accepted("OK\n"), Ok(()));
        assert_eq!(super::scan_accepted("FAIL-BUSY\n"), Ok(()));
        assert_eq!(super::scan_accepted("\nOK\n"), Ok(()));
        for reply in ["FAIL\n", "UNKNOWN COMMAND\n", "", "OKAY\n"] {
            assert_eq!(
                super::scan_accepted(reply),
                Err(kobo_protocol::DeviceError::Backend),
                "{reply:?}"
            );
        }
    }

    #[test]
    fn only_a_completed_association_names_a_connected_network() {
        assert_eq!(
            super::connected_ssid("wpa_state=SCANNING\nssid=old\n"),
            None
        );
        assert_eq!(
            super::connected_ssid("wpa_state=COMPLETED\nssid=Home\n"),
            Some("Home")
        );
    }

    #[test]
    fn remembers_only_previously_enabled_networks_for_rollback() {
        assert_eq!(super::saved_networks("network id / ssid / bssid / flags\n0\tHome\tany\t[CURRENT]\n1\tOff\tany\t[DISABLED]\n2\tCafe\tany\t\n").unwrap(), vec![("0".into(), true), ("2".into(), false)]);
        assert!(super::saved_networks("FAIL").is_err());
        assert!(super::saved_networks("network id\nbad row").is_err());
    }

    #[test]
    fn link_discovery_tracks_disappearance_and_refuses_ambiguity() {
        let root = std::env::temp_dir().join(format!("cobalt-wifi-links-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        assert_eq!(super::discover_link(&root), None);
        std::fs::create_dir_all(root.join("mlan0/wireless")).unwrap();
        assert_eq!(super::discover_link(&root), Some("mlan0".into()));
        std::fs::create_dir_all(root.join("wlan0/wireless")).unwrap();
        assert_eq!(super::discover_link(&root), None);
        std::fs::remove_dir_all(root.join("mlan0")).unwrap();
        assert_eq!(super::discover_link(&root), Some("wlan0".into()));
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(super::discover_link(&root), None);
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
}

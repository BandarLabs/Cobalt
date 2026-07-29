//! Wi-Fi control through the firmware's running `wpa_supplicant`.
//!
//! This module never starts a second supplicant. Nickel and Cobalt would then
//! be two owners of one interface, an arrangement already proven unsafe on the
//! Clara BW. The backend is available only when the firmware's `wpa_cli` and
//! `wlan0` are both present; all operations go through that existing owner.

use crate::network::{signal_dbm, WIRELESS_LINK};
use kobo_protocol::{DeviceError, DeviceResult, WifiNetwork, MAX_RADIO_DEVICES};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const WPA_TOOLS: [&str; 3] = ["/sbin/wpa_cli", "/usr/sbin/wpa_cli", "/usr/bin/wpa_cli"];

#[derive(Clone, Debug)]
pub struct Wifi {
    wpa_cli: PathBuf,
}

impl Wifi {
    #[must_use]
    pub fn open() -> Option<Self> {
        if !Path::new("/sys/class/net/wlan0").exists() {
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

    #[must_use]
    pub fn disconnect(&self) -> DeviceResult {
        match self.command(["disconnect"]) {
            Ok(_) => self.state(),
            Err(error) => DeviceResult::Failed(error),
        }
    }

    fn command<const N: usize>(&self, arguments: [&str; N]) -> Result<String, DeviceError> {
        let output = Command::new(&self.wpa_cli)
            .args(["-i", WIRELESS_LINK])
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
    use super::{parse_scan_results, quote, valid_credentials, value};

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

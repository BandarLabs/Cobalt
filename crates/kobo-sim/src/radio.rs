//! The simulated Wi-Fi radio, answering the way the device backend does.
//!
//! The device backend drives the firmware's supplicant through `wpa_cli`, and
//! two of its habits shape every app's Wi-Fi screen. A scan starts a new scan
//! and answers with the results the supplicant already had, so the first scan
//! after the radio comes up lists nothing and the list fills on the next one.
//! A join hands the network to the supplicant and answers at once, before the
//! radio has associated, so it never reports the connection itself; a later
//! read does, and a wrong password simply never connects. The old model listed
//! no networks and connected to any name instantly, which let an app look
//! finished without ever handling an empty first scan, a join that has not
//! landed yet, or one that never will.
//!
//! The faults are the failures the backend can actually report, so an app can
//! be shown each one at a desk.

use crate::facts::{Band, Bands};
use kobo_protocol::{DenyReason, DeviceError, DeviceRequest, DeviceResult, WifiNetwork};

/// A failure the operator imposes on the radio.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Fault {
    #[default]
    None,
    /// No wireless interface at session start, as on a reader whose radio the
    /// stock software never initialised. The backend is not offered at all.
    Absent,
    /// The supplicant does not answer and every command reaches its deadline.
    Hung,
    /// The supplicant answers, but refuses every command.
    Unresponsive,
    /// Joins are accepted and never associate, as with a wrong password.
    WrongPassword,
}

impl Fault {
    const ALL: [Self; 5] = [
        Self::None,
        Self::Absent,
        Self::Hung,
        Self::Unresponsive,
        Self::WrongPassword,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Absent => "absent",
            Self::Hung => "hung",
            Self::Unresponsive => "unresponsive",
            Self::WrongPassword => "wrong-password",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        Self::ALL.into_iter().find(|fault| fault.name() == text)
    }
}

struct Nearby {
    ssid: &'static str,
    signal_dbm: i16,
    secured: bool,
    band: Band,
}

/// The networks within range of every simulated reader, strongest first.
///
/// Both bands are represented, and both an open and a secured network, so the
/// band filter and the password prompt each have something to act on.
const NEARBY: [Nearby; 5] = [
    Nearby {
        ssid: "Cobalt Home",
        signal_dbm: -48,
        secured: true,
        band: Band::TwoPointFour,
    },
    Nearby {
        ssid: "Cobalt Home 5G",
        signal_dbm: -55,
        secured: true,
        band: Band::Five,
    },
    Nearby {
        ssid: "Library Guest",
        signal_dbm: -67,
        secured: false,
        band: Band::TwoPointFour,
    },
    Nearby {
        ssid: "Upstairs",
        signal_dbm: -79,
        secured: true,
        band: Band::Five,
    },
    Nearby {
        ssid: "Neighbour",
        signal_dbm: -86,
        secured: true,
        band: Band::TwoPointFour,
    },
];

#[derive(Debug)]
pub struct Radio {
    bands: Bands,
    fault: Fault,
    enabled: bool,
    connected: Option<String>,
    /// A join handed to the supplicant that has not been read back yet, and
    /// whether it will associate.
    joining: Option<(String, bool)>,
    /// What the last scan found, which is what the next scan reports.
    results: Vec<WifiNetwork>,
}

impl Radio {
    pub const fn new(bands: Bands) -> Self {
        Self {
            bands,
            fault: Fault::None,
            enabled: true,
            connected: None,
            joining: None,
            results: Vec::new(),
        }
    }

    pub fn set_fault(&mut self, fault: Fault) {
        self.fault = fault;
        if fault == Fault::WrongPassword {
            self.joining = None;
        }
    }

    /// Answers a Wi-Fi request, or `None` for any other request.
    pub fn handle(&mut self, request: &DeviceRequest) -> Option<DeviceResult> {
        if !is_wifi(request) {
            return None;
        }
        // The backend checks credentials before it runs any command, so a
        // malformed join is refused as invalid even while the supplicant is
        // failing. Only an absent backend is refused ahead of that, because
        // then there is no backend to do the checking.
        if let DeviceRequest::JoinWifi { ssid, password } = request {
            if self.fault != Fault::Absent && !valid_credentials(ssid, password) {
                return Some(DeviceResult::Failed(DeviceError::InvalidInput));
            }
        }
        Some(match self.fault {
            Fault::Absent => DeviceResult::Denied(DenyReason::Unsupported),
            Fault::Hung => DeviceResult::Failed(DeviceError::TimedOut),
            Fault::Unresponsive => DeviceResult::Failed(DeviceError::Backend),
            Fault::None | Fault::WrongPassword => self.answer(request),
        })
    }

    fn answer(&mut self, request: &DeviceRequest) -> DeviceResult {
        // The supplicant finishes associating between one request and the
        // next, so a pending join lands before anything else is read.
        if let Some((ssid, associates)) = self.joining.take() {
            if associates && self.enabled {
                self.connected = Some(ssid);
            }
        }
        match request {
            DeviceRequest::ScanWifi if self.enabled => {
                let found = self.in_range();
                let reported = std::mem::replace(&mut self.results, found);
                return self.state(self.mark_connected(reported));
            }
            DeviceRequest::SetWifi { enabled } => {
                self.enabled = *enabled;
                if !enabled {
                    self.connected = None;
                    self.results.clear();
                }
            }
            DeviceRequest::JoinWifi { ssid, password } => {
                if !valid_credentials(ssid, password) {
                    return DeviceResult::Failed(DeviceError::InvalidInput);
                }
                self.enabled = true;
                self.connected = None;
                let associates = self.fault != Fault::WrongPassword
                    && self.in_range().iter().any(|network| {
                        network.ssid == *ssid && network.secured != password.is_empty()
                    });
                self.joining = Some((ssid.clone(), associates));
            }
            DeviceRequest::DisconnectWifi => self.connected = None,
            _ => {}
        }
        self.state(Vec::new())
    }

    fn in_range(&self) -> Vec<WifiNetwork> {
        NEARBY
            .iter()
            .filter(|network| self.bands.hears(network.band))
            .map(|network| WifiNetwork {
                ssid: network.ssid.to_owned(),
                signal_dbm: network.signal_dbm,
                secured: network.secured,
                connected: false,
            })
            .collect()
    }

    fn mark_connected(&self, mut networks: Vec<WifiNetwork>) -> Vec<WifiNetwork> {
        for network in &mut networks {
            network.connected = self.connected.as_deref() == Some(network.ssid.as_str());
        }
        networks
    }

    fn state(&self, networks: Vec<WifiNetwork>) -> DeviceResult {
        DeviceResult::Wifi {
            available: true,
            enabled: self.enabled,
            connected_ssid: self.connected.clone(),
            networks,
        }
    }

    pub fn json(&self) -> kobo_json::Value {
        kobo_json::ObjectBuilder::new()
            .set("fault", self.fault.name())
            .set("enabled", self.enabled)
            .set(
                "connected",
                self.connected
                    .as_deref()
                    .map_or(kobo_json::Value::Null, kobo_json::Value::from),
            )
            .set("bands", self.bands.name())
            .build()
    }
}

/// Whether the radio, rather than the rest of the modelled services, answers.
pub const fn is_wifi(request: &DeviceRequest) -> bool {
    matches!(
        request,
        DeviceRequest::ReadWifi
            | DeviceRequest::ScanWifi
            | DeviceRequest::SetWifi { .. }
            | DeviceRequest::JoinWifi { .. }
            | DeviceRequest::DisconnectWifi
    )
}

/// The device backend's own rule, so the simulator refuses exactly what a
/// reader would rather than accepting a password the supplicant cannot take.
fn valid_credentials(ssid: &str, password: &str) -> bool {
    !ssid.is_empty()
        && ssid.len() <= 32
        && (password.is_empty() || (8..=63).contains(&password.len()))
        && ssid.chars().all(|character| !character.is_control())
        && password.chars().all(|character| !character.is_control())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn networks(result: Option<DeviceResult>) -> Vec<String> {
        match result {
            Some(DeviceResult::Wifi { networks, .. }) => {
                networks.into_iter().map(|network| network.ssid).collect()
            }
            other => panic!("expected a Wi-Fi state, got {other:?}"),
        }
    }

    fn connected(result: Option<DeviceResult>) -> Option<String> {
        match result {
            Some(DeviceResult::Wifi { connected_ssid, .. }) => connected_ssid,
            other => panic!("expected a Wi-Fi state, got {other:?}"),
        }
    }

    fn join(radio: &mut Radio, ssid: &str, password: &str) -> Option<DeviceResult> {
        radio.handle(&DeviceRequest::JoinWifi {
            ssid: ssid.into(),
            password: password.into(),
        })
    }

    #[test]
    fn the_first_scan_lists_nothing_and_the_next_reports_what_it_found() {
        let mut radio = Radio::new(Bands::Dual);
        assert!(networks(radio.handle(&DeviceRequest::ScanWifi)).is_empty());
        assert_eq!(
            networks(radio.handle(&DeviceRequest::ScanWifi)),
            [
                "Cobalt Home",
                "Cobalt Home 5G",
                "Library Guest",
                "Upstairs",
                "Neighbour"
            ]
        );
    }

    #[test]
    fn a_two_point_four_gigahertz_reader_never_lists_a_five_gigahertz_network() {
        let mut radio = Radio::new(Bands::TwoPointFour);
        radio.handle(&DeviceRequest::ScanWifi);
        assert_eq!(
            networks(radio.handle(&DeviceRequest::ScanWifi)),
            ["Cobalt Home", "Library Guest", "Neighbour"]
        );
        assert_eq!(
            connected({
                join(&mut radio, "Cobalt Home 5G", "password1");
                radio.handle(&DeviceRequest::ReadWifi)
            }),
            None
        );
    }

    #[test]
    fn a_join_answers_before_it_connects_and_a_later_read_shows_the_connection() {
        let mut radio = Radio::new(Bands::Dual);
        assert_eq!(
            connected(join(&mut radio, "Cobalt Home", "password1")),
            None
        );
        assert_eq!(
            connected(radio.handle(&DeviceRequest::ReadWifi)).as_deref(),
            Some("Cobalt Home")
        );
    }

    #[test]
    fn a_wrong_password_or_missing_password_never_connects() {
        let mut radio = Radio::new(Bands::Dual);
        join(&mut radio, "Cobalt Home", "");
        assert_eq!(connected(radio.handle(&DeviceRequest::ReadWifi)), None);
        radio.set_fault(Fault::WrongPassword);
        join(&mut radio, "Cobalt Home", "password1");
        assert_eq!(connected(radio.handle(&DeviceRequest::ReadWifi)), None);
        radio.set_fault(Fault::None);
        join(&mut radio, "Library Guest", "");
        assert_eq!(
            connected(radio.handle(&DeviceRequest::ReadWifi)).as_deref(),
            Some("Library Guest")
        );
    }

    #[test]
    fn credentials_the_backend_would_refuse_are_refused_here_too() {
        let mut radio = Radio::new(Bands::Dual);
        for (ssid, password) in [("", ""), ("Cobalt Home", "short"), ("a\nb", "")] {
            assert_eq!(
                join(&mut radio, ssid, password),
                Some(DeviceResult::Failed(DeviceError::InvalidInput)),
                "{ssid:?} {password:?}"
            );
        }
    }

    #[test]
    fn each_fault_answers_every_wifi_request_the_way_the_backend_fails() {
        let mut radio = Radio::new(Bands::Dual);
        for (fault, expected) in [
            (Fault::Absent, DeviceResult::Denied(DenyReason::Unsupported)),
            (Fault::Hung, DeviceResult::Failed(DeviceError::TimedOut)),
            (
                Fault::Unresponsive,
                DeviceResult::Failed(DeviceError::Backend),
            ),
        ] {
            radio.set_fault(fault);
            for request in [
                DeviceRequest::ReadWifi,
                DeviceRequest::ScanWifi,
                DeviceRequest::SetWifi { enabled: false },
                DeviceRequest::DisconnectWifi,
            ] {
                assert_eq!(radio.handle(&request), Some(expected.clone()), "{fault:?}");
            }
        }
    }

    #[test]
    fn a_malformed_join_is_refused_as_invalid_whatever_the_supplicant_is_doing() {
        let mut radio = Radio::new(Bands::Dual);
        for fault in [Fault::Hung, Fault::Unresponsive, Fault::WrongPassword] {
            radio.set_fault(fault);
            assert_eq!(
                join(&mut radio, "Cobalt Home", "short"),
                Some(DeviceResult::Failed(DeviceError::InvalidInput)),
                "{fault:?}"
            );
        }
        radio.set_fault(Fault::Absent);
        assert_eq!(
            join(&mut radio, "Cobalt Home", "short"),
            Some(DeviceResult::Denied(DenyReason::Unsupported))
        );
    }

    #[test]
    fn a_disabled_radio_scans_nothing_and_drops_its_connection() {
        let mut radio = Radio::new(Bands::Dual);
        join(&mut radio, "Library Guest", "");
        radio.handle(&DeviceRequest::ReadWifi);
        radio.handle(&DeviceRequest::SetWifi { enabled: false });
        let scanned = radio.handle(&DeviceRequest::ScanWifi);
        assert_eq!(connected(scanned.clone()), None);
        assert!(networks(scanned).is_empty());
    }

    #[test]
    fn requests_that_are_not_about_wifi_are_left_alone() {
        assert_eq!(
            Radio::new(Bands::Dual).handle(&DeviceRequest::ReadBattery),
            None
        );
    }

    #[test]
    fn faults_are_named_exactly_and_nothing_else_parses() {
        for fault in Fault::ALL {
            assert_eq!(Fault::parse(fault.name()), Some(fault));
        }
        assert_eq!(Fault::parse("wrong password"), None);
        assert_eq!(Fault::parse("busy"), None);
    }
}

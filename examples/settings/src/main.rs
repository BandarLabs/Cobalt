//! Radio settings implemented entirely through the public SDK.

use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{
    action_id, ActionId, BatteryDetail, BluetoothDevice, Context, DeviceRequest, DeviceResult,
    Glyph, KoboApp, RowLead, Screen, ScreenBuilder, Task, TaskId, TaskOutcome, WifiNetwork,
};
use std::process::ExitCode;

const BLUETOOTH: &str = "bluetooth";
const WIFI: &str = "wifi";
const BATTERY: &str = "battery";
const TOGGLE: &str = "toggle";
const RESCAN: &str = "rescan";
const MORE: &str = "more";
const DISCONNECT_WIFI: &str = "wifi-disconnect";
const PAGE_SIZE: usize = 4;
const DEVICE_ACTIONS: [&str; 10] = [
    "bt-0", "bt-1", "bt-2", "bt-3", "bt-4", "bt-5", "bt-6", "bt-7", "bt-8", "bt-9",
];
const NETWORK_ACTIONS: [&str; 10] = [
    "wifi-0", "wifi-1", "wifi-2", "wifi-3", "wifi-4", "wifi-5", "wifi-6", "wifi-7", "wifi-8",
    "wifi-9",
];

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum View {
    #[default]
    Home,
    Bluetooth,
    Wifi,
    WifiPassword,
    Battery,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum RadioState {
    #[default]
    Unavailable,
    Off,
    On,
}

impl RadioState {
    fn new(available: bool, enabled: bool) -> Self {
        match (available, enabled) {
            (false, _) => Self::Unavailable,
            (true, false) => Self::Off,
            (true, true) => Self::On,
        }
    }

    fn enabled(self) -> bool {
        self == Self::On
    }
}

#[derive(Clone, Debug)]
enum Pending {
    BluetoothRefresh,
    WifiRefresh,
    ConnectAfterPair(String),
}

/// Which row a failure belongs to.
///
/// One shared trouble string was the bug: a Wi-Fi read that failed raised a
/// banner on every screen, so "not supported by this runtime on this hardware"
/// appeared under the Battery row, and any later successful read cleared it.
/// A failure is now shown by the thing that failed and by nothing else.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Topic {
    Bluetooth,
    Wifi,
    Battery,
}

impl Topic {
    fn of(request: &DeviceRequest) -> Option<Self> {
        match request {
            DeviceRequest::ReadBluetooth
            | DeviceRequest::SetBluetooth { .. }
            | DeviceRequest::ScanBluetooth
            | DeviceRequest::PairBluetooth { .. }
            | DeviceRequest::ConnectBluetooth { .. }
            | DeviceRequest::DisconnectBluetooth { .. }
            | DeviceRequest::ForgetBluetooth { .. } => Some(Self::Bluetooth),
            DeviceRequest::ReadWifi
            | DeviceRequest::SetWifi { .. }
            | DeviceRequest::ScanWifi
            | DeviceRequest::JoinWifi { .. }
            | DeviceRequest::DisconnectWifi => Some(Self::Wifi),
            DeviceRequest::ReadBattery | DeviceRequest::ReadBatteryDetail => Some(Self::Battery),
            _ => None,
        }
    }
}

#[derive(Default)]
struct Settings {
    view: View,
    bluetooth_state: RadioState,
    devices: Vec<BluetoothDevice>,
    bluetooth_page: usize,
    restart_on_exit: bool,
    wifi_state: RadioState,
    connected_ssid: Option<String>,
    networks: Vec<WifiNetwork>,
    wifi_page: usize,
    selected_ssid: Option<String>,
    password: Keyboard,
    battery: Option<BatteryDetail>,
    pending: Option<Pending>,
    delayed: Option<TaskId>,
    trouble: Option<(Topic, String)>,
}

impl Settings {
    fn show(&self, context: &mut Context) {
        let screen = match self.view {
            View::Home => self.home(),
            View::Bluetooth => self.bluetooth(),
            View::Wifi => self.wifi(),
            View::WifiPassword => self.wifi_password(),
            View::Battery => self.battery(),
        };
        context.set_screen(screen);
    }

    fn home(&self) -> Screen {
        let bluetooth = match self.bluetooth_state {
            RadioState::Unavailable => "Unavailable on this firmware".to_owned(),
            RadioState::Off => "Off".to_owned(),
            RadioState::On => {
                let connected = self
                    .devices
                    .iter()
                    .filter(|device| device.connected)
                    .count();
                format!("On · {connected} connected")
            }
        };
        let wifi = match (self.wifi_state, &self.connected_ssid) {
            (RadioState::Unavailable, _) => "Unavailable on this firmware".to_owned(),
            (RadioState::On, Some(ssid)) => format!("Connected to {ssid}"),
            (RadioState::On, None) => "On · Not connected".to_owned(),
            (RadioState::Off, _) => "Off".to_owned(),
        };
        let screen = ScreenBuilder::new("settings")
            .top_bar("Settings")
            // A section, like the "Device" group under it. As a heading it was
            // set larger than the app's own name in the bar above, and one
            // screen was labelling two groups of the same kind in two
            // different ways.
            .section("Connections")
            .rows([
                (
                    BLUETOOTH,
                    "Bluetooth",
                    bluetooth,
                    RowLead::from(Glyph::Bluetooth),
                ),
                (WIFI, "Wi-Fi", wifi, RowLead::from(Glyph::Wifi)),
            ])
            .section("Device")
            .rows([(
                BATTERY,
                "Battery",
                self.battery_summary(),
                RowLead::from(Glyph::Battery),
            )]);
        screen.build()
    }

    fn bluetooth(&self) -> Screen {
        let mut screen = ScreenBuilder::new("settings-bluetooth")
            .top_bar("Bluetooth")
            .owns_back(true)
            .section_with_value(
                "Bluetooth",
                if self.bluetooth_state.enabled() {
                    "On"
                } else {
                    "Off"
                },
            )
            .button(
                TOGGLE,
                if self.bluetooth_state.enabled() {
                    "Turn Bluetooth off"
                } else {
                    "Turn Bluetooth on"
                },
            );
        if let Some(trouble) = self.banner_for(Topic::Bluetooth) {
            screen = screen.banner(kobo_sdk::BannerLevel::Attention, trouble);
        } else if self.restart_on_exit {
            screen = screen.banner(
                kobo_sdk::BannerLevel::Info,
                "Bluetooth shares one radio with Wi-Fi on this reader, and it can only start once per boot. Your reader will restart itself when you leave this app. Nothing you have saved is lost.",
            );
        }
        if self.bluetooth_state.enabled() {
            if self.devices.is_empty() {
                screen = screen
                    .text(
                        "No devices found. Put headphones or a keyboard in pairing mode, then rescan.",
                    )
                    .button(RESCAN, "Rescan for devices");
            } else {
                let pages = page_count(self.devices.len());
                screen = screen
                    .section_with_value("Devices", format!("{} / {pages}", self.bluetooth_page + 1))
                    .rows(
                        self.devices
                            .iter()
                            .skip(self.bluetooth_page * PAGE_SIZE)
                            .take(PAGE_SIZE)
                            .enumerate()
                            .map(|(index, device)| {
                                let state = if device.connected {
                                    "Connected"
                                } else if device.paired {
                                    "Paired · Tap to connect"
                                } else {
                                    "Available · Tap to pair"
                                };
                                (
                                    DEVICE_ACTIONS[index],
                                    device.name.as_str(),
                                    state,
                                    RowLead::from(if device.connected {
                                        Glyph::Check
                                    } else {
                                        Glyph::Circle
                                    }),
                                )
                            }),
                    );
                if pages > 1 {
                    screen =
                        screen.buttons([(MORE, "More devices"), (RESCAN, "Rescan for devices")]);
                } else {
                    screen = screen.button(RESCAN, "Rescan for devices");
                }
            }
        }
        screen.build()
    }

    fn wifi(&self) -> Screen {
        let mut screen = ScreenBuilder::new("settings-wifi")
            .top_bar("Wi-Fi")
            .owns_back(true)
            .section_with_value(
                "Wi-Fi",
                if self.wifi_state.enabled() {
                    "On"
                } else {
                    "Off"
                },
            )
            .button(
                TOGGLE,
                if self.wifi_state.enabled() {
                    "Turn Wi-Fi off"
                } else {
                    "Turn Wi-Fi on"
                },
            );
        if let Some(trouble) = self.banner_for(Topic::Wifi) {
            screen = screen.banner(kobo_sdk::BannerLevel::Attention, trouble);
        }
        if self.wifi_state.enabled() {
            if let Some(ssid) = &self.connected_ssid {
                screen = screen
                    .section_with_value("Connected", ssid.as_str())
                    .button(DISCONNECT_WIFI, "Disconnect");
            }
            if self.networks.is_empty() {
                screen = screen
                    .text("No networks found yet.")
                    .button(RESCAN, "Scan for networks");
            } else {
                let pages = page_count(self.networks.len());
                screen = screen
                    .section_with_value("Networks", format!("{} / {pages}", self.wifi_page + 1))
                    .rows(
                        self.networks
                            .iter()
                            .skip(self.wifi_page * PAGE_SIZE)
                            .take(PAGE_SIZE)
                            .enumerate()
                            .map(|(index, network)| {
                                let security = if network.secured { "Secured" } else { "Open" };
                                let summary = if network.connected {
                                    format!("Connected · {security} · {} dBm", network.signal_dbm)
                                } else {
                                    format!("{security} · {} dBm", network.signal_dbm)
                                };
                                (
                                    NETWORK_ACTIONS[index],
                                    network.ssid.as_str(),
                                    summary,
                                    RowLead::from(Glyph::Wifi),
                                )
                            }),
                    );
                if pages > 1 {
                    screen =
                        screen.buttons([(MORE, "More networks"), (RESCAN, "Scan for networks")]);
                } else {
                    screen = screen.button(RESCAN, "Scan for networks");
                }
            }
        }
        screen.build()
    }

    fn wifi_password(&self) -> Screen {
        let count = self.password.text().chars().count();
        let guidance = if let Some(trouble) = self.banner_for(Topic::Wifi) {
            trouble
        } else if count == 0 {
            "Type the network password.".to_owned()
        } else {
            let clipped = count.min(24);
            format!(
                "Password: {}{}",
                "•".repeat(clipped),
                if count > clipped { "…" } else { "" }
            )
        };
        ScreenBuilder::new("settings-wifi-password")
            .top_bar("Join Wi-Fi")
            .owns_back(true)
            .heading(self.selected_ssid.as_deref().unwrap_or("Network"))
            .text(guidance)
            .keyboard(&self.password, "Join")
            .build()
    }

    /// The banner for one topic, and nothing when the trouble belongs to
    /// another row.
    fn banner_for(&self, topic: Topic) -> Option<String> {
        self.trouble
            .as_ref()
            .filter(|(owner, _)| *owner == topic)
            .map(|(_, message)| message.clone())
    }

    fn battery_summary(&self) -> String {
        let Some(detail) = &self.battery else {
            return "Reading".to_owned();
        };
        let charge = detail
            .percent
            .map_or_else(|| "Unknown".to_owned(), |percent| format!("{percent}%"));
        detail.status.as_ref().map_or(charge.clone(), |status| {
            if detail.percent.is_some() {
                format!("{charge} · {status}")
            } else {
                status.clone()
            }
        })
    }

    /// Only the readings this gauge actually publishes reach the screen. A
    /// reader whose driver is thinner gets a shorter list, which is honest,
    /// rather than a column of dashes.
    fn battery(&self) -> Screen {
        let mut screen = ScreenBuilder::new("settings-battery")
            .top_bar("Battery")
            .owns_back(true);
        if let Some(trouble) = self.banner_for(Topic::Battery) {
            screen = screen.banner(kobo_sdk::BannerLevel::Attention, trouble);
        }
        let Some(detail) = &self.battery else {
            return screen.text("Reading the battery.").build();
        };
        if let Some(percent) = detail.percent {
            screen = screen
                .section_with_value("Charge", format!("{percent}%"))
                .progress(percent);
        }
        let mut facts: Vec<(String, String)> = Vec::new();
        let mut fact = |name: &str, value: Option<String>| {
            if let Some(value) = value {
                facts.push((name.to_owned(), value));
            }
        };
        fact("Status", detail.status.clone());
        fact(
            "Time remaining",
            detail.minutes_remaining().map(format_minutes),
        );
        fact("Health", detail.health.clone());
        fact(
            "Capacity",
            detail
                .health_percent()
                .map(|percent| format!("{percent}% of new")),
        );
        fact("Chemistry", detail.technology.clone());
        fact("Temperature", detail.decidegrees.map(format_temperature));
        fact("Voltage", detail.microvolts.map(format_volts));
        fact("Current", detail.microamps.map(format_amps));
        fact("Charge held", detail.charge_now.map(format_charge));
        fact("Charge when full", detail.charge_full.map(format_charge));
        fact(
            "Charge when new",
            detail.charge_full_design.map(format_charge),
        );
        if facts.is_empty() {
            screen = screen.text("This reader publishes nothing else about its battery.");
        } else {
            screen = screen.section("Details").facts(facts);
        }
        screen.button(RESCAN, "Read again").build()
    }

    fn refresh(context: &mut Context) {
        context.device().read_bluetooth();
        context.device().read_wifi();
        context.device().read_battery_detail();
    }

    fn delay_refresh(&mut self, context: &mut Context, pending: Pending) {
        self.pending = Some(pending);
        self.delayed = context.spawn(Task::Sleep { seconds: 3 });
    }

    /// Records a failure against the row that caused it, so it is reported
    /// once, where the reader was looking when they asked for it.
    fn fail(&mut self, topic: Topic, error: impl Into<String>) {
        self.pending = None;
        self.trouble = Some((topic, error.into()));
    }

    /// Clears a failure once the same row answers successfully. A Wi-Fi
    /// success must not silence a Bluetooth failure.
    fn settled(&mut self, topic: Topic) {
        if self
            .trouble
            .as_ref()
            .is_some_and(|(owner, _)| *owner == topic)
        {
            self.trouble = None;
        }
    }

    fn choose_bluetooth(&mut self, context: &mut Context, index: usize) {
        let Some(device) = self.devices.get(index).cloned() else {
            return;
        };
        self.settled(Topic::Bluetooth);
        if device.connected {
            context.device().disconnect_bluetooth(device.address);
        } else if device.paired {
            context.device().connect_bluetooth(device.address);
        } else if context.device().pair_bluetooth(device.address.clone()) {
            self.pending = Some(Pending::ConnectAfterPair(device.address));
        }
        self.show(context);
    }

    fn choose_network(&mut self, context: &mut Context, index: usize) {
        let Some(network) = self.networks.get(index).cloned() else {
            return;
        };
        self.settled(Topic::Wifi);
        if network.connected {
            return;
        }
        if network.secured {
            self.selected_ssid = Some(network.ssid);
            self.password.clear();
            self.view = View::WifiPassword;
            self.show(context);
        } else {
            context.device().join_wifi(network.ssid, "");
        }
    }
}

impl KoboApp for Settings {
    fn on_start(&mut self, context: &mut Context) {
        Self::refresh(context);
        self.show(context);
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if self.view == View::WifiPassword {
            if action == ActionId::BACK {
                self.view = View::Wifi;
                self.password.clear();
                self.show(context);
                return;
            }
            if let Some(pressed) = self.password.press(action) {
                if pressed == Pressed::Submitted {
                    if (8..=63).contains(&self.password.text().len()) {
                        let password = self.password.take();
                        if let Some(ssid) = self.selected_ssid.take() {
                            context.device().join_wifi(ssid, password);
                        }
                        self.settled(Topic::Wifi);
                        self.view = View::Wifi;
                    } else {
                        self.trouble = Some((
                            Topic::Wifi,
                            "A Wi-Fi password must be 8–63 bytes.".to_owned(),
                        ));
                    }
                } else {
                    self.settled(Topic::Wifi);
                }
                self.show(context);
            }
            return;
        }
        if action == ActionId::BACK {
            self.view = View::Home;
            self.show(context);
        } else if action == action_id(BLUETOOTH) {
            self.view = View::Bluetooth;
            context.device().read_bluetooth();
            self.show(context);
        } else if action == action_id(WIFI) {
            self.view = View::Wifi;
            context.device().read_wifi();
            self.show(context);
        } else if action == action_id(BATTERY) {
            self.view = View::Battery;
            context.device().read_battery_detail();
            self.show(context);
        } else if action == action_id(TOGGLE) {
            match self.view {
                View::Bluetooth => context
                    .device()
                    .set_bluetooth(!self.bluetooth_state.enabled()),
                View::Wifi => context.device().set_wifi(!self.wifi_state.enabled()),
                View::Home | View::WifiPassword | View::Battery => {}
            }
        } else if action == action_id(RESCAN) {
            match self.view {
                View::Bluetooth => {
                    self.bluetooth_page = 0;
                    context.device().scan_bluetooth();
                    self.delay_refresh(context, Pending::BluetoothRefresh);
                }
                View::Wifi => {
                    self.wifi_page = 0;
                    context.device().scan_wifi();
                    self.delay_refresh(context, Pending::WifiRefresh);
                }
                View::Battery => context.device().read_battery_detail(),
                View::Home | View::WifiPassword => {}
            }
            self.show(context);
        } else if action == action_id(MORE) {
            match self.view {
                View::Bluetooth => {
                    self.bluetooth_page =
                        (self.bluetooth_page + 1) % page_count(self.devices.len());
                }
                View::Wifi => {
                    self.wifi_page = (self.wifi_page + 1) % page_count(self.networks.len());
                }
                View::Home | View::WifiPassword | View::Battery => {}
            }
            self.show(context);
        } else if action == action_id(DISCONNECT_WIFI) {
            context.device().disconnect_wifi();
        } else if let Some(index) = DEVICE_ACTIONS
            .iter()
            .position(|name| action == action_id(name))
        {
            self.choose_bluetooth(context, self.bluetooth_page * PAGE_SIZE + index);
        } else if let Some(index) = NETWORK_ACTIONS
            .iter()
            .position(|name| action == action_id(name))
        {
            self.choose_network(context, self.wifi_page * PAGE_SIZE + index);
        }
    }

    fn on_device_result(
        &mut self,
        context: &mut Context,
        request: DeviceRequest,
        result: DeviceResult,
    ) {
        match result {
            DeviceResult::Bluetooth {
                available,
                enabled,
                devices,
                restart_on_exit,
            } => {
                self.bluetooth_state = RadioState::new(available, enabled);
                self.devices = devices;
                self.bluetooth_page %= page_count(self.devices.len());
                // Latched, so a later reading cannot withdraw a warning the
                // reader has already been shown.
                self.restart_on_exit |= restart_on_exit;
                self.settled(Topic::Bluetooth);
                if matches!(request, DeviceRequest::PairBluetooth { .. }) {
                    if let Some(Pending::ConnectAfterPair(address)) = self.pending.take() {
                        context.device().connect_bluetooth(address);
                    }
                }
            }
            DeviceResult::Wifi {
                available,
                enabled,
                connected_ssid,
                networks,
            } => {
                self.wifi_state = RadioState::new(available, enabled);
                self.connected_ssid = connected_ssid;
                if !networks.is_empty() || matches!(request, DeviceRequest::ScanWifi) {
                    self.networks = networks;
                    self.wifi_page %= page_count(self.networks.len());
                }
                self.settled(Topic::Wifi);
            }
            DeviceResult::BatteryDetail(detail) => {
                self.battery = Some(detail);
                self.settled(Topic::Battery);
            }
            // A failure belongs to whatever was asked for. When the request
            // is not one of the three rows, there is nowhere honest to show it,
            // so it is dropped rather than shown under an unrelated heading.
            DeviceResult::Failed(error) => {
                if let Some(topic) = Topic::of(&request) {
                    self.fail(topic, error.to_string());
                }
            }
            DeviceResult::Denied(reason) => {
                if let Some(topic) = Topic::of(&request) {
                    self.fail(topic, reason.to_string());
                }
            }
            DeviceResult::Done => match request {
                DeviceRequest::ReadBluetooth
                | DeviceRequest::SetBluetooth { .. }
                | DeviceRequest::ScanBluetooth
                | DeviceRequest::PairBluetooth { .. }
                | DeviceRequest::ConnectBluetooth { .. }
                | DeviceRequest::DisconnectBluetooth { .. }
                | DeviceRequest::ForgetBluetooth { .. } => context.device().read_bluetooth(),
                DeviceRequest::ReadWifi
                | DeviceRequest::SetWifi { .. }
                | DeviceRequest::ScanWifi
                | DeviceRequest::JoinWifi { .. }
                | DeviceRequest::DisconnectWifi => context.device().read_wifi(),
                _ => {}
            },
            DeviceResult::Granted { .. }
            | DeviceResult::Battery { .. }
            | DeviceResult::Frontlight { .. }
            | DeviceResult::Cover { .. }
            | DeviceResult::Audio { .. } => {}
        }
        self.show(context);
    }

    fn on_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
        if self.delayed != Some(task) {
            return;
        }
        self.delayed = None;
        match outcome {
            TaskOutcome::Completed(_) => match self.pending.take() {
                Some(Pending::BluetoothRefresh) => context.device().read_bluetooth(),
                Some(Pending::WifiRefresh) => context.device().scan_wifi(),
                Some(Pending::ConnectAfterPair(_)) | None => {}
            },
            TaskOutcome::Failed(error) => {
                let topic = match self.pending {
                    Some(Pending::WifiRefresh) => Topic::Wifi,
                    _ => Topic::Bluetooth,
                };
                self.fail(topic, error.to_string());
            }
            TaskOutcome::Cancelled => {}
        }
        self.show(context);
    }
}

fn page_count(items: usize) -> usize {
    items.div_ceil(PAGE_SIZE).max(1)
}

/// Hours and minutes, because "412 minutes" is arithmetic a reader should not
/// have to do standing up.
fn format_minutes(minutes: u32) -> String {
    let (hours, rest) = (minutes / 60, minutes % 60);
    match (hours, rest) {
        (0, rest) => format!("{rest} min"),
        (hours, 0) => format!("{hours} h"),
        (hours, rest) => format!("{hours} h {rest} min"),
    }
}

/// Tenths of a degree to one decimal place, without floating point: this
/// codebase has none and a division plus a remainder says the same thing.
fn format_temperature(decidegrees: i32) -> String {
    format!("{}.{} °C", decidegrees / 10, (decidegrees % 10).abs())
}

fn format_volts(microvolts: i32) -> String {
    let millivolts = microvolts / 1000;
    format!(
        "{}.{:02} V",
        millivolts / 1000,
        (millivolts % 1000).abs() / 10
    )
}

/// Signed, because the sign is the reading: a negative current is the pack
/// being drained and a positive one is it being filled.
fn format_amps(microamps: i32) -> String {
    format!("{} mA", microamps / 1000)
}

fn format_charge(microamp_hours: i32) -> String {
    format!("{} mAh", microamp_hours / 1000)
}

fn main() -> ExitCode {
    match kobo_sdk::run("settings", Settings::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("settings: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{RadioState, Settings, DEVICE_ACTIONS, MORE, NETWORK_ACTIONS, RESCAN};
    use kobo_sdk::{
        action_id, BatteryDetail, BluetoothDevice, BluetoothDeviceKind, Chrome, WifiNetwork,
        CLARA_BW_METRICS,
    };

    fn bluetooth_device(index: usize) -> BluetoothDevice {
        BluetoothDevice {
            address: format!("AA:BB:CC:DD:EE:{index:02X}"),
            name: format!("Device {index}"),
            kind: BluetoothDeviceKind::Audio,
            paired: index % 2 == 0,
            connected: index == 0,
        }
    }

    #[test]
    fn a_full_bluetooth_scan_keeps_every_drawn_action_on_the_panel() {
        let settings = Settings {
            bluetooth_state: RadioState::On,
            devices: (0..DEVICE_ACTIONS.len()).map(bluetooth_device).collect(),
            ..Settings::default()
        };
        let screen = settings.bluetooth();
        let issues = screen.validate(&CLARA_BW_METRICS);
        assert!(issues.is_empty(), "{issues:?}");
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true));
        assert!(layout.rect_of_action(action_id(RESCAN)).is_some());
        assert!(layout.rect_of_action(action_id(MORE)).is_some());
    }

    #[test]
    fn a_full_wifi_scan_keeps_every_drawn_action_on_the_panel() {
        let settings = Settings {
            wifi_state: RadioState::On,
            connected_ssid: Some("Network 0".to_owned()),
            networks: NETWORK_ACTIONS
                .iter()
                .enumerate()
                .map(|(index, _)| WifiNetwork {
                    ssid: format!("Network {index}"),
                    signal_dbm: -40 - i16::try_from(index).unwrap_or_default(),
                    secured: true,
                    connected: index == 0,
                })
                .collect(),
            ..Settings::default()
        };
        let screen = settings.wifi();
        let issues = screen.validate(&CLARA_BW_METRICS);
        assert!(issues.is_empty(), "{issues:?}");
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true));
        assert!(layout.rect_of_action(action_id(RESCAN)).is_some());
        assert!(layout.rect_of_action(action_id(MORE)).is_some());
    }

    #[test]
    fn the_password_keyboard_and_validation_message_fit_the_panel() {
        let settings = Settings {
            view: super::View::WifiPassword,
            selected_ssid: Some("A secured wireless network".to_owned()),
            trouble: Some((
                super::Topic::Wifi,
                "A Wi-Fi password must be 8–63 bytes.".to_owned(),
            )),
            ..Settings::default()
        };
        let issues = settings.wifi_password().validate(&CLARA_BW_METRICS);
        assert!(issues.is_empty(), "{issues:?}");
    }

    #[test]
    fn a_gauge_that_publishes_everything_still_fits_the_panel() {
        let settings = Settings {
            view: super::View::Battery,
            battery: Some(BatteryDetail {
                percent: Some(28),
                status: Some("Discharging".to_owned()),
                health: Some("Good".to_owned()),
                technology: Some("Li-ion".to_owned()),
                decidegrees: Some(290),
                microvolts: Some(3_720_000),
                microamps: Some(-180_000),
                charge_now: Some(420_000),
                charge_full: Some(1_480_000),
                charge_full_design: Some(1_500_000),
            }),
            ..Settings::default()
        };
        let screen = settings.battery();
        let issues = screen.validate(&CLARA_BW_METRICS);
        assert!(issues.is_empty(), "{issues:?}");
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true));
        assert!(layout.rect_of_action(action_id(RESCAN)).is_some());
    }

    /// A reader whose driver publishes almost nothing gets a short screen
    /// rather than a column of dashes, and the button stays reachable.
    #[test]
    fn a_gauge_that_publishes_almost_nothing_says_so_rather_than_showing_zeroes() {
        let settings = Settings {
            view: super::View::Battery,
            battery: Some(BatteryDetail {
                percent: Some(64),
                ..BatteryDetail::default()
            }),
            ..Settings::default()
        };
        let screen = settings.battery();
        let issues = screen.validate(&CLARA_BW_METRICS);
        assert!(issues.is_empty(), "{issues:?}");
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true));
        assert!(layout.rect_of_action(action_id(RESCAN)).is_some());
    }

    #[test]
    fn readings_are_formatted_the_way_somebody_standing_up_would_read_them() {
        assert_eq!(super::format_minutes(412), "6 h 52 min");
        assert_eq!(super::format_minutes(45), "45 min");
        assert_eq!(super::format_minutes(120), "2 h");
        assert_eq!(super::format_temperature(290), "29.0 °C");
        assert_eq!(super::format_volts(3_720_000), "3.72 V");
        assert_eq!(super::format_amps(-180_000), "-180 mA");
        assert_eq!(super::format_charge(1_480_000), "1480 mAh");
    }

    /// Wear is the whole point of the screen, so the arithmetic behind it gets
    /// its own test rather than being trusted because it looks right.
    #[test]
    fn wear_and_time_remaining_come_from_the_readings_and_not_from_guesses() {
        let detail = BatteryDetail {
            microamps: Some(-180_000),
            charge_now: Some(420_000),
            charge_full: Some(1_200_000),
            charge_full_design: Some(1_500_000),
            ..BatteryDetail::default()
        };
        assert_eq!(detail.health_percent(), Some(80));
        assert_eq!(detail.minutes_remaining(), Some(140));
        let idle = BatteryDetail {
            microamps: Some(0),
            charge_now: Some(420_000),
            ..BatteryDetail::default()
        };
        assert_eq!(idle.minutes_remaining(), None);
        assert_eq!(BatteryDetail::default().health_percent(), None);
    }

    /// The bug this pins: a Wi-Fi failure used to raise a banner on every
    /// screen, so "not supported by this runtime on this hardware" appeared
    /// under the Battery row, and any later successful read cleared it.
    #[test]
    fn a_failure_is_reported_by_the_row_that_caused_it_and_by_nothing_else() {
        let mut settings = Settings::default();
        settings.fail(super::Topic::Wifi, "not supported on this hardware");
        assert!(settings.banner_for(super::Topic::Wifi).is_some());
        assert!(settings.banner_for(super::Topic::Battery).is_none());
        assert!(settings.banner_for(super::Topic::Bluetooth).is_none());

        settings.settled(super::Topic::Battery);
        assert!(
            settings.banner_for(super::Topic::Wifi).is_some(),
            "a battery read succeeding must not silence a Wi-Fi failure"
        );
        settings.settled(super::Topic::Wifi);
        assert!(settings.banner_for(super::Topic::Wifi).is_none());
    }

    #[test]
    fn a_failure_is_filed_under_the_request_that_produced_it() {
        use kobo_sdk::DeviceRequest;
        assert_eq!(
            super::Topic::of(&DeviceRequest::ReadBatteryDetail),
            Some(super::Topic::Battery)
        );
        assert_eq!(
            super::Topic::of(&DeviceRequest::ScanWifi),
            Some(super::Topic::Wifi)
        );
        assert_eq!(
            super::Topic::of(&DeviceRequest::ScanBluetooth),
            Some(super::Topic::Bluetooth)
        );
        assert_eq!(super::Topic::of(&DeviceRequest::ReadFrontlight), None);
    }
}

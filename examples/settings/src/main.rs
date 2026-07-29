//! Radio settings implemented entirely through the public SDK.

use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{
    action_id, ActionId, BluetoothDevice, Context, DeviceRequest, DeviceResult, Glyph, KoboApp,
    RowLead, Screen, ScreenBuilder, Task, TaskId, TaskOutcome, WifiNetwork,
};
use std::process::ExitCode;

const BLUETOOTH: &str = "bluetooth";
const WIFI: &str = "wifi";
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

#[derive(Default)]
struct Settings {
    view: View,
    bluetooth_state: RadioState,
    devices: Vec<BluetoothDevice>,
    bluetooth_page: usize,
    wifi_state: RadioState,
    connected_ssid: Option<String>,
    networks: Vec<WifiNetwork>,
    wifi_page: usize,
    selected_ssid: Option<String>,
    password: Keyboard,
    pending: Option<Pending>,
    delayed: Option<TaskId>,
    trouble: Option<String>,
}

impl Settings {
    fn show(&self, context: &mut Context) {
        let screen = match self.view {
            View::Home => self.home(),
            View::Bluetooth => self.bluetooth(),
            View::Wifi => self.wifi(),
            View::WifiPassword => self.wifi_password(),
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
        let mut screen = ScreenBuilder::new("settings")
            .top_bar("Settings")
            .heading("Connections")
            .rows([
                (
                    BLUETOOTH,
                    "Bluetooth",
                    bluetooth,
                    RowLead::from(Glyph::Settings),
                ),
                (WIFI, "Wi-Fi", wifi, RowLead::from(Glyph::Wifi)),
            ]);
        if let Some(trouble) = &self.trouble {
            screen = screen.banner(kobo_sdk::BannerLevel::Attention, trouble.clone());
        }
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
        if let Some(trouble) = &self.trouble {
            screen = screen.banner(kobo_sdk::BannerLevel::Attention, trouble.clone());
        }
        if self.bluetooth_state.enabled() {
            if self.devices.is_empty() {
                screen = screen.text(
                    "No devices found. Put headphones or a keyboard in pairing mode, then rescan.",
                );
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
                    screen = screen.button(MORE, "More devices");
                }
            }
            screen = screen.button(RESCAN, "Rescan for devices");
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
        if let Some(trouble) = &self.trouble {
            screen = screen.banner(kobo_sdk::BannerLevel::Attention, trouble.clone());
        }
        if self.wifi_state.enabled() {
            if let Some(ssid) = &self.connected_ssid {
                screen = screen
                    .section_with_value("Connected", ssid.as_str())
                    .button(DISCONNECT_WIFI, "Disconnect");
            }
            if self.networks.is_empty() {
                screen = screen.text("No networks found yet.");
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
                    screen = screen.button(MORE, "More networks");
                }
            }
            screen = screen.button(RESCAN, "Scan for networks");
        }
        screen.build()
    }

    fn wifi_password(&self) -> Screen {
        let count = self.password.text().chars().count();
        let guidance = if let Some(trouble) = &self.trouble {
            trouble.clone()
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

    fn refresh(context: &mut Context) {
        context.device().read_bluetooth();
        context.device().read_wifi();
    }

    fn delay_refresh(&mut self, context: &mut Context, pending: Pending) {
        self.pending = Some(pending);
        self.delayed = context.spawn(Task::Sleep { seconds: 3 });
    }

    fn fail(&mut self, error: impl Into<String>) {
        self.pending = None;
        self.trouble = Some(error.into());
    }

    fn choose_bluetooth(&mut self, context: &mut Context, index: usize) {
        let Some(device) = self.devices.get(index).cloned() else {
            return;
        };
        self.trouble = None;
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
        self.trouble = None;
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
                        self.trouble = None;
                        self.view = View::Wifi;
                    } else {
                        self.trouble = Some("A Wi-Fi password must be 8–63 bytes.".to_owned());
                    }
                } else {
                    self.trouble = None;
                }
                self.show(context);
            }
            return;
        }
        if action == ActionId::BACK {
            self.view = View::Home;
            self.trouble = None;
            self.show(context);
        } else if action == action_id(BLUETOOTH) {
            self.view = View::Bluetooth;
            context.device().read_bluetooth();
            self.show(context);
        } else if action == action_id(WIFI) {
            self.view = View::Wifi;
            context.device().read_wifi();
            self.show(context);
        } else if action == action_id(TOGGLE) {
            match self.view {
                View::Bluetooth => context
                    .device()
                    .set_bluetooth(!self.bluetooth_state.enabled()),
                View::Wifi => context.device().set_wifi(!self.wifi_state.enabled()),
                View::Home | View::WifiPassword => {}
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
                View::Home | View::WifiPassword => {}
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
            } => {
                self.bluetooth_state = RadioState::new(available, enabled);
                self.devices = devices;
                self.bluetooth_page %= page_count(self.devices.len());
                self.trouble = None;
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
                self.trouble = None;
            }
            DeviceResult::Failed(error) => self.fail(error.to_string()),
            DeviceResult::Denied(reason) => self.fail(reason.to_string()),
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
            TaskOutcome::Failed(error) => self.fail(error.to_string()),
            TaskOutcome::Cancelled => {}
        }
        self.show(context);
    }
}

fn page_count(items: usize) -> usize {
    items.div_ceil(PAGE_SIZE).max(1)
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
        action_id, BluetoothDevice, BluetoothDeviceKind, Chrome, WifiNetwork, CLARA_BW_METRICS,
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
            trouble: Some("A Wi-Fi password must be 8–63 bytes.".to_owned()),
            ..Settings::default()
        };
        let issues = settings.wifi_password().validate(&CLARA_BW_METRICS);
        assert!(issues.is_empty(), "{issues:?}");
    }
}

//! Radio settings implemented entirely through the public SDK.

use kobo_json::Value;
use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{
    action_id, ActionId, BatteryDetail, BluetoothDevice, Context, DeviceRequest, DeviceResult,
    Glyph, Heartbeat, KoboApp, RowLead, Screen, ScreenBuilder, Task, TaskId, TaskOutcome,
    WifiNetwork,
};
use std::process::ExitCode;

const BLUETOOTH: &str = "bluetooth";
const WIFI: &str = "wifi";
const BATTERY: &str = "battery";
const UPDATE: &str = "update";
const CHECK: &str = "check";
const INSTALL: &str = "install";
const TOGGLE: &str = "toggle";
const RESCAN: &str = "rescan";
const MORE: &str = "more";
const PREVIOUS: &str = "previous";
const PAGE_SIZE: usize = 4;
const DEVICE_ACTIONS: [&str; 10] = [
    "bt-0", "bt-1", "bt-2", "bt-3", "bt-4", "bt-5", "bt-6", "bt-7", "bt-8", "bt-9",
];
const NETWORK_ACTIONS: [&str; 10] = [
    "wifi-0", "wifi-1", "wifi-2", "wifi-3", "wifi-4", "wifi-5", "wifi-6", "wifi-7", "wifi-8",
    "wifi-9",
];

/// The version this binary was compiled as, which is the version installed:
/// the binaries and the installer travel together.
const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Where releases are published.
const RELEASES: &str = "https://api.github.com/repos/BandarLabs/Cobalt/releases/latest";
/// The device profile this build is packaged for, as release assets name it.
const DEVICE: &str = "ClaraBW";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum View {
    #[default]
    Home,
    Bluetooth,
    Wifi,
    WifiPassword,
    Battery,
    Update,
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

/// Where the software update stands. One journey, told left to right:
/// nothing asked, asking GitHub, reading the digest file, ready to install,
/// installing, installed, or stopped with a reason.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
enum UpdateFlow {
    #[default]
    Idle,
    Checking,
    UpToDate {
        latest: String,
    },
    /// The release is newer; its digest file is being fetched so the
    /// download can be verified before it is installed.
    Digest {
        version: String,
        url: String,
    },
    Ready {
        version: String,
        url: String,
        sha256: String,
    },
    Installing {
        version: String,
    },
    Installed {
        version: String,
    },
    Failed(String),
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
    /// Ticks while the Wi-Fi screen is open, and each tick asks for another
    /// scan. A list of networks goes stale the moment the reader carries the
    /// device into another room, and asking them to press a button to find
    /// that out is asking them to do the radio's job.
    scan_clock: Heartbeat,
    scanning: bool,
    selected_ssid: Option<String>,
    password: Keyboard,
    battery: Option<BatteryDetail>,
    update: UpdateFlow,
    update_task: Option<TaskId>,
    pending: Option<Pending>,
    delayed: Option<TaskId>,
    trouble: Option<(Topic, String)>,
}

impl Settings {
    fn show(&mut self, context: &mut Context) {
        self.keep_scanning(context);
        let screen = match self.view {
            View::Home => self.home(),
            View::Bluetooth => self.bluetooth(),
            View::Wifi => self.wifi(),
            View::WifiPassword => self.wifi_password(),
            View::Battery => self.battery(),
            View::Update => self.update(),
        };
        context.set_screen(screen);
    }

    /// Keeps the radio looking for as long as the Wi-Fi list is the thing on
    /// the panel, and no longer.
    ///
    /// Every screen is drawn through `show`, so this is the one place that
    /// knows what the reader is looking at now rather than what they tapped a
    /// moment ago. A scan costs a repaint only when the answer differs: the
    /// frame planner declines an identical frame, so a still list is free.
    fn keep_scanning(&mut self, context: &mut Context) {
        if self.view == View::Wifi && self.wifi_state.enabled() {
            if !self.scan_clock.is_running() {
                self.scan_clock.start(context);
                self.scanning = true;
                context.device().scan_wifi();
            }
        } else {
            self.scan_clock.stop(context);
            self.scanning = false;
        }
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
            .rows([
                (
                    BATTERY,
                    "Battery",
                    self.battery_summary(),
                    RowLead::from(Glyph::Battery),
                ),
                (
                    UPDATE,
                    "Software update",
                    self.update_summary(),
                    RowLead::from(Glyph::Download),
                ),
            ])
            // The installed build's own version, baked in at compile time.
            // The binaries and the installer travel together, so what this
            // binary was compiled as is what is installed.
            .section_with_value("Cobalt", VERSION);
        screen.build()
    }

    /// One line for the home row: where the update journey stands, or an
    /// invitation to start it.
    fn update_summary(&self) -> String {
        match &self.update {
            UpdateFlow::Idle => format!("Cobalt {VERSION}"),
            UpdateFlow::Checking | UpdateFlow::Digest { .. } => "Checking".to_owned(),
            UpdateFlow::UpToDate { .. } => "Up to date".to_owned(),
            UpdateFlow::Ready { version, .. } => format!("{version} available"),
            UpdateFlow::Installing { .. } => "Installing".to_owned(),
            UpdateFlow::Installed { version } => format!("Updated to {version}"),
            UpdateFlow::Failed(_) => "Update failed".to_owned(),
        }
    }

    fn update(&self) -> Screen {
        let mut screen = ScreenBuilder::new("settings-update")
            .top_bar("Software update")
            .owns_back(true)
            .section_with_value("Installed", VERSION);
        match &self.update {
            UpdateFlow::Idle => {
                screen = screen
                    .text("Checking asks GitHub for the newest published release. Nothing is downloaded until you choose to install it.")
                    .button(CHECK, "Check for updates");
            }
            UpdateFlow::Checking | UpdateFlow::Digest { .. } => {
                screen = screen.text("Checking for the newest release.");
            }
            UpdateFlow::UpToDate { latest } => {
                screen = screen
                    .text(format!("{latest} is the newest published release, and it is what this reader is running."))
                    .button(CHECK, "Check again");
            }
            UpdateFlow::Ready { version, .. } => {
                screen = screen
                    .text(
                        "The download is checked against its published digest before anything is replaced, and the release you are running now is kept for one step back.",
                    )
                    .button(INSTALL, format!("Update to {version}"));
            }
            UpdateFlow::Installing { version } => {
                screen = screen.text(format!(
                    "Installing {version}. Keep the reader awake; this screen will change when it is done."
                ));
            }
            UpdateFlow::Installed { version } => {
                screen = screen.text(format!(
                    "Updated to {version}. Close Cobalt and open it again from the menu to run the new release."
                ));
            }
            UpdateFlow::Failed(reason) => {
                screen = screen
                    .banner(kobo_sdk::BannerLevel::Attention, reason.clone())
                    .button(CHECK, "Try again");
            }
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
                screen = screen.controls(
                    u8::try_from(paging(self.bluetooth_page, pages).len() + 1).unwrap_or(3),
                    paging(self.bluetooth_page, pages).into_iter().chain([(
                        RESCAN,
                        "Rescan",
                        Glyph::Refresh,
                    )]),
                );
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
            // Every verb on this screen is collected and drawn as one row.
            // Stacked full-width outlines read as a form rather than as a
            // choice, and this screen had three of them down the left margin.
            if let Some(ssid) = &self.connected_ssid {
                // A fact rather than a section. A section is a heading over
                // the rows that belong to it, and what is connected has no
                // rows: it is one label and one value, which is the shape the
                // battery screen uses for exactly this.
                screen = screen.facts([("Connected", ssid.as_str())]);
            }
            if self.networks.is_empty() {
                // This screen scans on its own, so "none found" is only true
                // once a scan has come back with nothing. Before that it is a
                // report on a question nobody has asked yet.
                screen = screen.text(if self.scanning {
                    "Looking for networks…"
                } else {
                    "No networks found."
                });
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
                                // Leaving a network is done where joining one
                                // is done, which is what the Bluetooth screen
                                // beside it already says on every row. A verb
                                // at the foot of the page was a second place
                                // to look for the same switch.
                                let summary = if network.connected {
                                    format!(
                                        "Connected · Tap to disconnect · {} dBm",
                                        network.signal_dbm
                                    )
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
                let turns = paging(self.wifi_page, pages);
                if !turns.is_empty() {
                    screen = screen.controls(u8::try_from(turns.len()).unwrap_or(2), turns);
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

    /// Asks GitHub what the newest published release is. Nothing is
    /// downloaded beyond the release description until the reader chooses to
    /// install.
    fn check_for_update(&mut self, context: &mut Context) {
        self.update = UpdateFlow::Checking;
        self.update_task = context.spawn(Task::Fetch {
            url: RELEASES.to_owned(),
            offset: 0,
            max_bytes: 256 * 1024,
            headers: Vec::new(),
        });
        if self.update_task.is_none() {
            self.update = UpdateFlow::Failed("This build was refused the network.".to_owned());
        }
    }

    fn install_update(&mut self, context: &mut Context) {
        let UpdateFlow::Ready {
            version,
            url,
            sha256,
        } = self.update.clone()
        else {
            return;
        };
        // The progress screen is queued ahead of the request, so the panel
        // shows it while the runtime blocks on the download and the swap.
        self.update = UpdateFlow::Installing { version };
        self.show(context);
        if !context.device().update(url, sha256) {
            // Nothing was queued, so no reply will ever arrive to move the
            // screen on; the refusal is reported here instead.
            self.update =
                UpdateFlow::Failed("The release names a download that cannot be used.".to_owned());
            self.show(context);
        }
    }

    /// Takes the reply to whichever update fetch was in flight: the release
    /// description first, then the digest file that lets the download be
    /// verified.
    fn took_update_reply(&mut self, context: &mut Context, bytes: &[u8]) {
        let body = String::from_utf8_lossy(bytes);
        match self.update.clone() {
            UpdateFlow::Checking => match latest_release(&body) {
                Err(reason) => self.update = UpdateFlow::Failed(reason),
                Ok(release) if release.newer_than(VERSION) => {
                    self.update_task = context.spawn(Task::Fetch {
                        url: release.digest,
                        offset: 0,
                        max_bytes: 16 * 1024,
                        headers: Vec::new(),
                    });
                    self.update = if self.update_task.is_none() {
                        UpdateFlow::Failed("This build was refused the network.".to_owned())
                    } else {
                        UpdateFlow::Digest {
                            version: release.version,
                            url: release.archive,
                        }
                    };
                }
                Ok(release) => {
                    self.update = UpdateFlow::UpToDate {
                        latest: release.version,
                    };
                }
            },
            UpdateFlow::Digest { version, url } => {
                self.update = match digest_for(&body, &archive_name(&version)) {
                    Some(sha256) => UpdateFlow::Ready {
                        version,
                        url,
                        sha256,
                    },
                    None => UpdateFlow::Failed(
                        "The release does not publish a digest for this reader's download."
                            .to_owned(),
                    ),
                };
            }
            _ => {}
        }
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

    /// Moves the list on the panel one page, clamped at both ends.
    fn turn_page(&mut self, forward: bool) {
        let (page, pages) = match self.view {
            View::Bluetooth => (&mut self.bluetooth_page, page_count(self.devices.len())),
            View::Wifi => (&mut self.wifi_page, page_count(self.networks.len())),
            View::Home | View::WifiPassword | View::Battery | View::Update => return,
        };
        *page = if forward {
            (*page + 1).min(pages - 1)
        } else {
            page.saturating_sub(1)
        };
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
            context.device().disconnect_wifi();
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
        } else if action == action_id(UPDATE) {
            self.view = View::Update;
            self.show(context);
        } else if action == action_id(CHECK) {
            self.check_for_update(context);
            self.show(context);
        } else if action == action_id(INSTALL) {
            self.install_update(context);
        } else if action == action_id(TOGGLE) {
            match self.view {
                View::Bluetooth => context
                    .device()
                    .set_bluetooth(!self.bluetooth_state.enabled()),
                View::Wifi => context.device().set_wifi(!self.wifi_state.enabled()),
                View::Home | View::WifiPassword | View::Battery | View::Update => {}
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
                View::Home | View::WifiPassword | View::Update => {}
            }
            self.show(context);
        } else if action == action_id(MORE) {
            self.turn_page(true);
            self.show(context);
        } else if action == action_id(PREVIOUS) {
            self.turn_page(false);
            self.show(context);
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
                if matches!(request, DeviceRequest::ScanWifi) {
                    self.scanning = false;
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
                if matches!(request, DeviceRequest::Update { .. }) {
                    self.update = UpdateFlow::Failed(error.to_string());
                } else if let Some(topic) = Topic::of(&request) {
                    self.fail(topic, error.to_string());
                }
            }
            DeviceResult::Denied(reason) => {
                if matches!(request, DeviceRequest::Update { .. }) {
                    self.update = UpdateFlow::Failed(reason.to_string());
                } else if let Some(topic) = Topic::of(&request) {
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
                DeviceRequest::Update { .. } => {
                    if let UpdateFlow::Installing { version } = self.update.clone() {
                        self.update = UpdateFlow::Installed { version };
                    }
                }
                _ => {}
            },
            DeviceResult::Granted { .. }
            | DeviceResult::Battery { .. }
            | DeviceResult::Frontlight { .. }
            | DeviceResult::Cover { .. }
            | DeviceResult::Audio { .. }
            | DeviceResult::Apps { .. } => {}
        }
        self.show(context);
    }

    fn on_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
        // A tick is not an answer to anything the application asked for, so it
        // is taken before the pending request is consulted.
        if self.scan_clock.on_task(context, task, &outcome) {
            if self.view == View::Wifi && self.wifi_state.enabled() {
                self.scanning = true;
                context.device().scan_wifi();
            }
            return;
        }
        if self.update_task == Some(task) {
            self.update_task = None;
            match outcome {
                TaskOutcome::Completed(bytes) => self.took_update_reply(context, &bytes),
                TaskOutcome::Failed(error) => self.update = UpdateFlow::Failed(error.to_string()),
                TaskOutcome::Cancelled => self.update = UpdateFlow::Idle,
            }
            self.show(context);
            return;
        }
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

/// The page turns a paginated list should offer from where it is standing.
///
/// A list that wraps is a list that lies: on the last page "More" promised
/// devices that were not there, and pressing it took the reader back to the
/// first page as if that were forward. Each direction is offered only where
/// there is a page on that side.
fn paging(page: usize, pages: usize) -> Vec<(&'static str, &'static str, Glyph)> {
    let mut turns = Vec::new();
    if page > 0 {
        turns.push((PREVIOUS, "Previous", Glyph::Previous));
    }
    if page + 1 < pages {
        turns.push((MORE, "Next", Glyph::Next));
    }
    turns
}

fn page_count(items: usize) -> usize {
    items.div_ceil(PAGE_SIZE).max(1)
}

/// The newest published release, as its assets name this device's download.
#[derive(Debug)]
struct Release {
    version: String,
    /// Where the installable archive is.
    archive: String,
    /// Where the digest file that vouches for it is.
    digest: String,
}

impl Release {
    /// Strictly newer, so the same version and anything unparseable both
    /// answer no and nothing is offered.
    fn newer_than(&self, installed: &str) -> bool {
        match (numbers(&self.version), numbers(installed)) {
            (Some(latest), Some(installed)) => latest > installed,
            _ => false,
        }
    }
}

fn numbers(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.split('.').map(str::parse::<u64>);
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(Ok(major)), Some(Ok(minor)), Some(Ok(patch)), None) => Some((major, minor, patch)),
        _ => None,
    }
}

fn archive_name(version: &str) -> String {
    format!("cobalt-{version}-{DEVICE}-KoboRoot.tgz")
}

/// Reads the GitHub "latest release" reply down to the two URLs this device
/// needs. The failure strings face the reader, so they say what is missing
/// rather than where in the JSON it was not.
fn latest_release(body: &str) -> Result<Release, String> {
    let value = kobo_json::parse(body)
        .map_err(|_| "GitHub sent something that is not a release.".to_owned())?;
    let tag = value
        .get("tag_name")
        .and_then(Value::as_str)
        .ok_or_else(|| "The newest release does not name a version.".to_owned())?;
    let version = tag.trim_start_matches('v').to_owned();
    let empty = [];
    let assets = value
        .get("assets")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    let url_of = |name: &str| {
        assets
            .iter()
            .find(|asset| asset.get("name").and_then(Value::as_str) == Some(name))
            .and_then(|asset| asset.get("browser_download_url"))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    let archive = url_of(&archive_name(&version))
        .ok_or_else(|| "The newest release has no download for this reader.".to_owned())?;
    let digest = url_of(&format!("cobalt-{version}-{DEVICE}.sha256"))
        .ok_or_else(|| "The newest release publishes no digest to verify against.".to_owned())?;
    Ok(Release {
        version,
        archive,
        digest,
    })
}

/// Finds the digest vouching for `asset` in a `sha256sum` style listing:
/// sixty-four hex characters, whitespace, a file name per line.
fn digest_for(listing: &str, asset: &str) -> Option<String> {
    listing.lines().find_map(|line| {
        let (digest, name) = line.split_once(char::is_whitespace)?;
        let named = name.trim_start().trim_start_matches('*') == asset;
        let plausible = digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        (named && plausible).then(|| digest.to_owned())
    })
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
    use super::{RadioState, Settings, DEVICE_ACTIONS, MORE, NETWORK_ACTIONS, PREVIOUS, RESCAN};
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
        assert!(layout.rect_of_action(action_id(MORE)).is_some());
        assert!(layout.rect_of_action(action_id(PREVIOUS)).is_none());
    }

    /// The bug this pins: the way on wrapped, so the last page of networks
    /// offered a page that was not there and called going back to the first
    /// one "More".
    #[test]
    fn the_last_page_of_networks_offers_only_the_way_back() {
        let mut settings = Settings {
            wifi_state: RadioState::On,
            networks: NETWORK_ACTIONS
                .iter()
                .enumerate()
                .map(|(index, _)| WifiNetwork {
                    ssid: format!("Network {index}"),
                    signal_dbm: -40,
                    secured: true,
                    connected: false,
                })
                .collect(),
            ..Settings::default()
        };
        settings.wifi_page = super::page_count(settings.networks.len()) - 1;
        let layout = settings
            .wifi()
            .layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true));
        assert!(layout.rect_of_action(action_id(PREVIOUS)).is_some());
        assert!(layout.rect_of_action(action_id(MORE)).is_none());
    }

    /// The bug this pins: an empty list said "No networks found yet" the
    /// instant the screen opened, before the radio had been asked anything.
    /// The screen scans on its own now, so it has to say which of the two
    /// situations it is in.
    #[test]
    fn an_empty_wifi_list_says_whether_it_is_still_looking() {
        let looking = Settings {
            wifi_state: RadioState::On,
            scanning: true,
            ..Settings::default()
        };
        let settled = Settings {
            wifi_state: RadioState::On,
            ..Settings::default()
        };
        assert!(text_of(&looking.wifi()).contains("Looking for networks"));
        assert!(text_of(&settled.wifi()).contains("No networks found."));
        // And neither offers a Scan button, because pressing one would ask
        // for what is already happening every five seconds.
        let layout = looking
            .wifi()
            .layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true));
        assert!(layout.rect_of_action(action_id(RESCAN)).is_none());
    }

    fn text_of(screen: &kobo_sdk::Screen) -> String {
        screen
            .nodes
            .iter()
            .filter_map(|node| match node {
                kobo_sdk::Node::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" ")
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

    fn release_json(version: &str, with_digest: bool) -> String {
        let archive = format!(
            r#"{{"name":"cobalt-{version}-ClaraBW-KoboRoot.tgz","browser_download_url":"https://example.test/{version}/KoboRoot.tgz"}}"#
        );
        let digest = if with_digest {
            format!(
                r#",{{"name":"cobalt-{version}-ClaraBW.sha256","browser_download_url":"https://example.test/{version}/checksums"}}"#
            )
        } else {
            String::new()
        };
        format!(r#"{{"tag_name":"v{version}","assets":[{archive}{digest}]}}"#)
    }

    #[test]
    fn a_release_is_read_down_to_the_two_urls_this_reader_needs() {
        let release = super::latest_release(&release_json("9.9.9", true)).expect("a full release");
        assert_eq!(release.version, "9.9.9");
        assert_eq!(release.archive, "https://example.test/9.9.9/KoboRoot.tgz");
        assert_eq!(release.digest, "https://example.test/9.9.9/checksums");
        assert!(release.newer_than(super::VERSION));
    }

    #[test]
    fn a_release_without_a_digest_to_verify_against_is_not_offered() {
        assert!(super::latest_release(&release_json("9.9.9", false))
            .expect_err("no digest, no offer")
            .contains("digest"));
    }

    #[test]
    fn the_installed_release_and_an_unreadable_tag_are_both_not_newer() {
        let same = super::latest_release(&release_json(super::VERSION, true)).expect("release");
        assert!(!same.newer_than(super::VERSION));
        let strange = super::Release {
            version: "nightly".to_owned(),
            archive: String::new(),
            digest: String::new(),
        };
        assert!(
            !strange.newer_than(super::VERSION),
            "a version that cannot be compared must not be offered as an upgrade"
        );
    }

    #[test]
    fn the_digest_is_found_beside_the_other_files_in_the_listing() {
        let digest = "a".repeat(64);
        let listing = format!(
            "{}  THIRD-PARTY.md\n{digest}  cobalt-9.9.9-ClaraBW-KoboRoot.tgz\n",
            "b".repeat(64)
        );
        assert_eq!(
            super::digest_for(&listing, "cobalt-9.9.9-ClaraBW-KoboRoot.tgz"),
            Some(digest)
        );
        assert_eq!(
            super::digest_for(&listing, "cobalt-9.9.9-ClaraBW.tgz"),
            None,
            "a digest for a different file vouches for nothing"
        );
        assert_eq!(
            super::digest_for("not a listing", "cobalt-9.9.9-ClaraBW-KoboRoot.tgz"),
            None
        );
    }

    #[test]
    fn every_stop_on_the_update_journey_fits_the_panel() {
        let flows = [
            super::UpdateFlow::Idle,
            super::UpdateFlow::Checking,
            super::UpdateFlow::UpToDate {
                latest: "0.1.0".to_owned(),
            },
            super::UpdateFlow::Ready {
                version: "9.9.9".to_owned(),
                url: "https://example.test/KoboRoot.tgz".to_owned(),
                sha256: "a".repeat(64),
            },
            super::UpdateFlow::Installing {
                version: "9.9.9".to_owned(),
            },
            super::UpdateFlow::Installed {
                version: "9.9.9".to_owned(),
            },
            super::UpdateFlow::Failed("The download did not match its digest.".to_owned()),
        ];
        for flow in flows {
            let settings = Settings {
                view: super::View::Update,
                update: flow.clone(),
                ..Settings::default()
            };
            let issues = settings.update().validate(&CLARA_BW_METRICS);
            assert!(issues.is_empty(), "{flow:?}: {issues:?}");
        }
    }

    #[test]
    fn only_a_release_that_is_ready_offers_the_install_button() {
        let ready = Settings {
            view: super::View::Update,
            update: super::UpdateFlow::Ready {
                version: "9.9.9".to_owned(),
                url: "https://example.test/KoboRoot.tgz".to_owned(),
                sha256: "a".repeat(64),
            },
            ..Settings::default()
        };
        let layout = ready
            .update()
            .layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true));
        assert!(layout.rect_of_action(action_id(super::INSTALL)).is_some());

        let checking = Settings {
            view: super::View::Update,
            update: super::UpdateFlow::Checking,
            ..Settings::default()
        };
        let layout = checking
            .update()
            .layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true));
        assert!(
            layout.rect_of_action(action_id(super::INSTALL)).is_none(),
            "nothing may be installed before it is verified"
        );
        assert!(
            layout.rect_of_action(action_id(super::CHECK)).is_none(),
            "a check that is already running must not be restartable"
        );
    }
}

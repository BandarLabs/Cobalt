//! Radio settings implemented entirely through the public SDK.

use kobo_json::Value;
use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{
    action_id, ActionId, BandAlign, BannerLevel, BatteryDetail, BluetoothDevice, Context,
    DenyReason, DeviceError, DeviceIdentity, DeviceRequest, DeviceResult, FieldKind, Glyph,
    Heartbeat, KoboApp, RowLead, Screen, ScreenBuilder, SlotWidth, Task, TaskError, TaskId,
    TaskOutcome, UpdateChannel, WifiNetwork,
};
use std::process::ExitCode;

const BLUETOOTH: &str = "bluetooth";
const WIFI: &str = "wifi";
const BATTERY: &str = "battery";
const ABOUT: &str = "about";
const UPDATE: &str = "update";
const CHECK: &str = "check";
const INSTALL: &str = "install";
const CLOSE: &str = "close";
const TOGGLE: &str = "toggle";
const AUTO_COBALT: &str = "auto-cobalt";
const AUTO_APPS: &str = "auto-apps";
/// The two answers of the update channel list. Choosing the one that is not
/// current asks for confirmation first; choosing the current one does nothing.
const CHANNEL_STABLE: &str = "channel-stable";
const CHANNEL_BETA: &str = "channel-beta";
/// The channel list's pager. Two answers always fit one page, so neither is
/// drawn, but an option list names them all the same.
const CHANNEL_PREVIOUS: &str = "channel-previous";
const CHANNEL_NEXT: &str = "channel-next";
/// The Wi-Fi password field. Tapping it does nothing: the keys are already
/// under it.
const PASSWORD_FIELD: &str = "wifi-password";
/// Opens the update channel and the automatic-update switches from the
/// update screen.
const UPDATE_OPTIONS: &str = "update-options";
const CONFIRM_CHANNEL: &str = "confirm-channel";
const CANCEL_CHANNEL: &str = "cancel-channel";
const RESCAN: &str = "rescan";
const MORE: &str = "more";
const PREVIOUS: &str = "previous";
/// Opens the Wi-Fi page from the update check that found no network.
///
/// Not the SDK's own join control: that one starts Settings, which is this
/// application, so here it would do nothing at all.
const JOIN_WIFI: &str = "join-wifi";

/// Where the runtime puts this reader's address on the local network.
///
/// The protocol has no field for it, and adding one is a platform change, so
/// the address travels the way the reader's time zone does: in the
/// environment the runtime builds for each application it starts. Settings
/// shows it when it is there and explains the computer side when it is not.
const READER_ADDRESS_VARIABLE: &str = "KOBO_READER_ADDRESS";

/// The version this binary was compiled as, which is the version installed:
/// the binaries and the installer travel together.
const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Where releases are published.
const RELEASES: &str = "https://api.github.com/repos/BandarLabs/Cobalt/releases/latest";
/// Only the newest few releases, because a hundred of them is a megabyte and
/// a half of assets a reader downloads over their own Wi-Fi to learn one
/// version number, and because that is how the feed outgrew its window.
/// Betas and stables alternate, so fifteen always holds several betas.
const BETA_RELEASES: &str = "https://api.github.com/repos/BandarLabs/Cobalt/releases?per_page=15";

/// How much of a release feed this reader will take.
///
/// A fetch carries a byte range, and GitHub honours it: a feed longer than
/// its window comes back as a 206 holding the first window of it, which is
/// valid JSON up to the point it stops being any JSON at all. Nothing in the
/// transport can tell that from a whole document, so the only defence is a
/// window the feed cannot outgrow, and a sentence for the day it does.
const fn release_window(channel: UpdateChannel) -> u32 {
    match channel {
        UpdateChannel::Stable => 256 * 1024,
        UpdateChannel::Beta => 4 * 1024 * 1024,
    }
}

/// The lines under the two automatic-update switches: what happens now with
/// each one as it is, rather than what tapping it would do.
const fn auto_update_summaries(cobalt: bool, apps: bool) -> (&'static str, &'static str) {
    (
        if cobalt {
            "Installs new versions itself"
        } else {
            "Installs only when you ask"
        },
        if apps {
            "Apps update themselves"
        } else {
            "Apps update when you ask"
        },
    )
}

const fn channel_name(channel: UpdateChannel) -> &'static str {
    match channel {
        UpdateChannel::Stable => "Stable",
        UpdateChannel::Beta => "Beta",
    }
}

/// The answer in the update channel list that stands for `channel`.
const fn channel_action(channel: UpdateChannel) -> &'static str {
    match channel {
        UpdateChannel::Stable => CHANNEL_STABLE,
        UpdateChannel::Beta => CHANNEL_BETA,
    }
}

const fn opposite_channel(channel: UpdateChannel) -> UpdateChannel {
    match channel {
        UpdateChannel::Stable => UpdateChannel::Beta,
        UpdateChannel::Beta => UpdateChannel::Stable,
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum View {
    #[default]
    Home,
    Bluetooth,
    Wifi,
    WifiPassword,
    Battery,
    About,
    Update,
    /// The standing update choices: the channel and the two switches.
    UpdateOptions,
    UpdateChannelConfirm,
}

/// `Unavailable` is a confirmed hardware fact: it is only ever produced by
/// `new`, from a successful device reply reporting `available: false`. It
/// must never be assumed from the absence of an answer -- `Unknown`, the
/// default, is what every radio starts as before its first read replies,
/// precisely so a pending or backend-failed read can never be mistaken for a
/// device that has no radio at all.
///
/// `Unsupported` is the second settled answer, and it is a fact about this
/// runtime rather than about the hardware: the reading was refused with
/// `DenyReason::Unsupported`, which the protocol defines as this runtime not
/// being able to do it on this hardware *yet*. That is not the same claim as
/// a missing radio, and the two must not be collapsed: a reader whose
/// Bluetooth the runtime has not brought up has the hardware, and telling
/// them it does not exist is a worse answer than the "Checking..." this
/// replaced. Anything that is neither a reading nor a refusal still leaves
/// the radio `Unknown`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum RadioState {
    #[default]
    Unknown,
    Unavailable,
    Unsupported,
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
/// nothing asked, asking GitHub, verifying signed metadata, ready to install,
/// installing, installed, or stopped with a reason.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
enum UpdateFlow {
    #[default]
    Idle,
    Checking {
        channel: UpdateChannel,
    },
    UpToDate {
        latest: String,
    },
    Ahead {
        latest: String,
    },
    /// The release is newer; its signed canonical manifest is being fetched.
    Manifest {
        release: Release,
    },
    /// The manifest bytes are held while their detached signature is fetched.
    ManifestSignature {
        release: Release,
        manifest: Vec<u8>,
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
    /// The check found no network at all. Separate from a failure because it
    /// is the one the owner can fix from here, so it gets the way to do it.
    Offline,
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
    About,
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
            DeviceRequest::ReadIdentity => Some(Self::About),
            _ => None,
        }
    }
}

/// Whether trouble with this request belongs on the Update screen.
///
/// The automatic-update switches are reached from the update screen, so a
/// runtime that cannot read or keep those choices reports it there, as well
/// as over the choices themselves.
fn update_screen_owns(request: &DeviceRequest) -> bool {
    matches!(
        request,
        DeviceRequest::Update { .. }
            | DeviceRequest::ReadAutoUpdate
            | DeviceRequest::SetAutoUpdate { .. }
            | DeviceRequest::ReadUpdateChannel
            | DeviceRequest::SetUpdateChannel { .. }
    )
}

/// Whether this request reads or keeps one of the standing update choices,
/// whose trouble is also shown over the choices themselves.
fn is_standing_choice(request: &DeviceRequest) -> bool {
    matches!(
        request,
        DeviceRequest::ReadAutoUpdate
            | DeviceRequest::SetAutoUpdate { .. }
            | DeviceRequest::ReadUpdateChannel
            | DeviceRequest::SetUpdateChannel { .. }
    )
}

/// One part of the About page, kept whole on whichever page it lands.
enum AboutPart {
    Facts(&'static str, Vec<(String, String)>),
    Text(&'static str, &'static str),
}

#[derive(Default)]
struct Settings {
    view: View,
    home_page: usize,
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
    identity: Option<DeviceIdentity>,
    about_page: usize,
    /// This reader's address on the local network, when the runtime said.
    address: Option<String>,
    update: UpdateFlow,
    update_task: Option<TaskId>,
    /// What the runtime updates on its own, once it has answered. `None`
    /// until the first reply, so the switches are drawn from a real answer
    /// rather than a guess that a tap would then contradict.
    auto_update: Option<(bool, bool)>,
    update_channel: Option<UpdateChannel>,
    /// Why the runtime would not read or keep a standing update choice,
    /// shown over the choices. The update screen hears it too, as it always
    /// has.
    choice_trouble: Option<String>,
    pending: Option<Pending>,
    delayed: Option<TaskId>,
    trouble: Option<(Topic, String)>,
}

impl Settings {
    fn show(&mut self, context: &mut Context) {
        self.keep_scanning(context);
        self.clamp_page(context);
        let screen = match self.view {
            View::Home => self.home_for(context),
            View::Bluetooth => self.bluetooth_for(context),
            View::Wifi => self.wifi_for(context),
            View::WifiPassword => self.wifi_password(),
            View::Battery => self.battery(),
            View::About => self.about_for(context),
            View::Update => self.update(),
            View::UpdateOptions => self.update_options_for(context),
            View::UpdateChannelConfirm => self.update_channel_confirmation_for(context),
        };
        context.set_screen(screen);
    }

    fn clamp_page(&mut self, context: &Context) {
        match self.view {
            View::Home => {
                self.home_page = self
                    .home_page
                    .min(self.home_pages(context).len().saturating_sub(1));
            }
            View::Bluetooth => {
                self.bluetooth_page = self
                    .bluetooth_page
                    .min(self.bluetooth_pages(context).len().saturating_sub(1));
            }
            View::About => {
                self.about_page = self
                    .about_page
                    .min(self.about_pages(context).len().saturating_sub(1));
            }
            View::Wifi => {
                self.wifi_page = self
                    .wifi_page
                    .min(self.wifi_pages(context).len().saturating_sub(1));
            }
            _ => {}
        }
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

    fn home_rows(&self) -> Vec<SettingsRow> {
        let bluetooth = match self.bluetooth_state {
            RadioState::Unknown => "Checking…".to_owned(),
            RadioState::Unavailable => "Not available on this device".to_owned(),
            RadioState::Unsupported => "Not available in this version of Cobalt".to_owned(),
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
            (RadioState::Unknown, _) => "Checking…".to_owned(),
            (RadioState::Unavailable, _) => "Not available on this device".to_owned(),
            (RadioState::Unsupported, _) => "Not available in this version of Cobalt".to_owned(),
            (RadioState::On, Some(ssid)) => format!("Connected to {ssid}"),
            (RadioState::On, None) => "On · Not connected".to_owned(),
            (RadioState::Off, _) => "Off".to_owned(),
        };
        vec![
            SettingsRow::new(
                Some("Connections"),
                BLUETOOTH,
                "Bluetooth",
                bluetooth,
                Glyph::Bluetooth,
            ),
            SettingsRow::new(None, WIFI, "Wi-Fi", wifi, Glyph::Wifi),
            SettingsRow::new(
                Some("Device"),
                BATTERY,
                "Battery",
                self.battery_summary(),
                Glyph::Battery,
            ),
            SettingsRow::new(
                None,
                UPDATE,
                "Software update",
                self.update_summary(),
                Glyph::Download,
            ),
            SettingsRow::new(
                None,
                ABOUT,
                "About",
                "Model, versions and Wi-Fi address",
                Glyph::Reader,
            ),
        ]
    }

    fn home_pages(&self, context: &Context) -> Vec<Vec<usize>> {
        let rows = self.home_rows();
        let measured: Vec<_> = rows
            .iter()
            .map(|row| (row.section, row.title.as_str(), row.summary.as_str()))
            .collect();
        let around = ScreenBuilder::new("settings")
            .top_bar("Settings")
            .section_with_value("Cobalt", VERSION)
            .build();
        context.paginate_rows_in_sections_under(
            &measured,
            false,
            kobo_sdk::Position::AtTheFoot,
            &around,
        )
    }

    fn home_for(&self, context: &Context) -> Screen {
        let rows = self.home_rows();
        let pages = self.home_pages(context);
        let page = self.home_page.min(pages.len().saturating_sub(1));
        let mut screen = ScreenBuilder::new("settings").top_bar("Settings");
        let mut remaining = pages[page].as_slice();
        while let Some((&first, rest)) = remaining.split_first() {
            if let Some(section) = rows[first].section {
                screen = screen.section(section);
            }
            let count = rest
                .iter()
                .position(|&index| rows[index].section.is_some())
                .map_or(remaining.len(), |position| position + 1);
            screen = screen.rows(remaining[..count].iter().map(|&index| rows[index].as_row()));
            remaining = &remaining[count..];
        }
        screen = screen.section_with_value("Cobalt", VERSION);
        page_controls(screen, page, pages.len()).build()
    }

    #[cfg(test)]
    fn home(&self) -> Screen {
        self.home_for(&Context::default())
    }

    /// One line for the home row: where the update journey stands, or an
    /// invitation to start it.
    fn update_summary(&self) -> String {
        match &self.update {
            UpdateFlow::Idle => format!("Cobalt {VERSION}"),
            UpdateFlow::Checking { .. }
            | UpdateFlow::Manifest { .. }
            | UpdateFlow::ManifestSignature { .. } => "Checking".to_owned(),
            UpdateFlow::UpToDate { .. } => "Up to date".to_owned(),
            UpdateFlow::Ahead { .. } => "Newer than this channel".to_owned(),
            UpdateFlow::Ready { version, .. } => format!("{version} available"),
            UpdateFlow::Installing { .. } => "Installing".to_owned(),
            UpdateFlow::Installed { version } => format!("Updated to {version}"),
            UpdateFlow::Offline => "Not on Wi-Fi".to_owned(),
            UpdateFlow::Failed(_) => "Update failed".to_owned(),
        }
    }

    fn update(&self) -> Screen {
        let mut screen = ScreenBuilder::new("settings-update")
            .top_bar("Software update")
            .owns_back(true);
        match &self.update {
            UpdateFlow::Idle => {
                screen = screen
                    .section_with_value("Installed", VERSION)
                    .text("Checking asks GitHub for the newest published release. Nothing is downloaded until you choose to install it.")
                    .button(CHECK, "Check for updates");
            }
            UpdateFlow::Checking { .. }
            | UpdateFlow::Manifest { .. }
            | UpdateFlow::ManifestSignature { .. } => {
                screen = screen
                    .section_with_value("Installed", VERSION)
                    .text("Checking for the newest release.");
            }
            UpdateFlow::UpToDate { latest } => {
                screen = screen
                    .section_with_value("Installed", VERSION)
                    .text(format!("{latest} is the newest published release, and it is what this reader is running."))
                    .button(CHECK, "Check again");
            }
            UpdateFlow::Ahead { latest } => {
                screen = screen
                    .section_with_value("Installed", VERSION)
                    .text(format!("{latest} is the newest release on this channel. This reader is already running a newer version, so nothing is downgraded."))
                    .button(CHECK, "Check again");
            }
            UpdateFlow::Ready { version, .. } => {
                screen = screen
                    .banner(BannerLevel::Info, format!("Cobalt {version} is available."))
                    .facts([("Installed", VERSION), ("Available", version.as_str())])
                    .text(
                        "The download is checked before it is installed. The version you have now is kept in case the update has to be undone.",
                    )
                    .primary_button(INSTALL, "Install update");
            }
            UpdateFlow::Installing { version } => {
                screen = screen.section_with_value("Installing", version).text(
                    "Keep the reader awake. This screen will change when installation is complete.",
                );
            }
            UpdateFlow::Installed { version } => {
                screen = screen
                    .facts([("Installed", version)])
                    .splash(
                        Some(Glyph::Check),
                        "Update complete",
                        "Close Cobalt, then reopen it from the menu.",
                    )
                    .primary_button(CLOSE, "Close Cobalt");
            }
            // The fix for being offline is joining a network, so that is
            // what the banner offers; checking again only helps afterwards.
            UpdateFlow::Offline => {
                screen = screen
                    .section_with_value("Installed", VERSION)
                    .banner(
                        BannerLevel::Attention,
                        "This reader is not on Wi-Fi. Join a network, then check again.",
                    )
                    .banner_action(JOIN_WIFI, "Join Wi-Fi")
                    .button(CHECK, "Check again");
            }
            UpdateFlow::Failed(reason) => {
                screen = screen
                    .section_with_value("Installed", VERSION)
                    .banner(BannerLevel::Error, reason.clone())
                    .banner_action(CHECK, "Try again");
            }
        }
        // The way to the standing choices, offered only around the quiet
        // states so an installation in progress keeps its screen to itself.
        // Absent until the runtime has answered, because a switch drawn from
        // a guess can be tapped into recording the opposite of what it
        // showed.
        if matches!(
            self.update,
            UpdateFlow::Idle
                | UpdateFlow::UpToDate { .. }
                | UpdateFlow::Ahead { .. }
                | UpdateFlow::Offline
                | UpdateFlow::Failed(_)
        ) {
            if let Some(summary) = self.update_options_summary() {
                screen = screen.rows([(
                    UPDATE_OPTIONS,
                    "Channel and automatic updates",
                    summary,
                    Glyph::Settings,
                )]);
            }
        }
        screen.build()
    }

    /// One line for the row that opens the standing choices: the channel,
    /// and what updates without being asked.
    fn update_options_summary(&self) -> Option<String> {
        let (Some((cobalt, apps)), Some(channel)) = (self.auto_update, self.update_channel) else {
            return None;
        };
        let automatic = match (cobalt, apps) {
            (true, true) => "Cobalt and apps update on their own",
            (true, false) => "Cobalt updates on its own",
            (false, true) => "Apps update on their own",
            (false, false) => "Updates only when you ask",
        };
        Some(format!("{} · {automatic}", channel_name(channel)))
    }

    /// The standing choices on a screen of their own.
    ///
    /// They shared the update screen until they became runtime controls,
    /// and with the release status, a banner and its action above them the
    /// screen ran off the foot of the panel at the default size held
    /// sideways, and at the largest size either way.
    ///
    /// The sentence under the channels and the line under each channel say
    /// nothing the confirmation does not say again, so where the panel
    /// cannot hold them at this size they go, the sentence first, rather
    /// than a switch going under the foot of the panel.
    fn update_options_for(&self, context: &Context) -> Screen {
        let metrics = context.metrics();
        // Each step gives up one more thing, least needed first: the
        // switches' own lines, which restate what the switch shows; the
        // sentence that warns about Beta; the channels' lines; and last the
        // list's prompt, which only repeats what the two choices under it
        // say, and only where a refusal over the choices would otherwise push
        // a switch off a panel held sideways.
        let mut screen = self.update_options_shaped(0);
        for given_up in 0..=4 {
            screen = self.update_options_shaped(given_up);
            if !screen
                .diagnostics(&metrics, &kobo_sdk::Chrome::measuring(true))
                .has_errors()
            {
                break;
            }
        }
        screen
    }

    #[cfg(test)]
    fn update_options(&self) -> Screen {
        self.update_options_for(&Context::default())
    }

    /// The update options with the first `given_up` of the things
    /// [`Self::update_options_for`] can do without left out.
    fn update_options_shaped(&self, given_up: usize) -> Screen {
        let switch_lines = given_up < 1;
        let explain = given_up < 2;
        let summaries = given_up < 3;
        let prompt = given_up < 4;
        let mut screen = ScreenBuilder::new("settings-update-options")
            .top_bar("Update options")
            .owns_back(true);
        if let Some(trouble) = &self.choice_trouble {
            screen = screen.banner(BannerLevel::Error, trouble.clone());
        }
        let (Some((cobalt, apps)), Some(channel)) = (self.auto_update, self.update_channel) else {
            return screen.text("Reading the update choices.").build();
        };
        let summary = |words: &'static str| if summaries { words } else { "" };
        screen = screen
            .option_list(
                if prompt { "Update channel" } else { "" },
                0,
                CHANNEL_PREVIOUS,
                CHANNEL_NEXT,
                [
                    (CHANNEL_STABLE, "Stable", summary("Tested versions")),
                    (CHANNEL_BETA, "Beta", summary("New versions sooner")),
                ],
            )
            .chosen(match channel {
                UpdateChannel::Stable => 0,
                UpdateChannel::Beta => 1,
            });
        if explain {
            screen = screen.text("Beta versions may have more problems. You can change back here at any time, and your apps, their data and your saved keys are kept.");
        }
        // Each switch's line says what happens now with it as it is, as the
        // Wi-Fi and Bluetooth switches do. They are the first thing to go
        // where the panel cannot hold everything.
        let (cobalt_now, apps_now) = if switch_lines {
            auto_update_summaries(cobalt, apps)
        } else {
            ("", "")
        };
        screen
            .toggle(
                AUTO_COBALT,
                "Update Cobalt automatically",
                cobalt_now,
                cobalt,
            )
            .toggle(AUTO_APPS, "Update apps automatically", apps_now, apps)
            .build()
    }

    #[cfg(test)]
    fn update_channel_confirmation(&self) -> Screen {
        self.update_channel_confirmation_for(&Context::default())
    }

    /// The confirmation, with its two answers stacked where the panel has
    /// the height for them and side by side where it does not, which is a
    /// panel held sideways at the largest sizes.
    fn update_channel_confirmation_for(&self, context: &Context) -> Screen {
        let stacked = self.update_channel_confirmation_shaped(false);
        if stacked
            .diagnostics(&context.metrics(), &kobo_sdk::Chrome::measuring(true))
            .has_errors()
        {
            self.update_channel_confirmation_shaped(true)
        } else {
            stacked
        }
    }

    fn update_channel_confirmation_shaped(&self, side_by_side: bool) -> Screen {
        let current = self.update_channel.unwrap_or_default();
        let chosen = opposite_channel(current);
        let explanation = match chosen {
            UpdateChannel::Beta => {
                "Cobalt and the App Store will offer Beta versions, which come sooner and may have more problems. Your apps, their data and your saved keys are kept."
            }
            UpdateChannel::Stable => {
                "Cobalt and the App Store will offer Stable versions from now on. Nothing is downgraded, and your apps, their data and your saved keys are kept."
            }
        };
        let confirm = format!("Use {} updates", channel_name(chosen));
        let screen = ScreenBuilder::new("settings-update-channel-confirm")
            .top_bar("Confirm update channel")
            .owns_back(true)
            .facts([
                ("Installed", format!("Cobalt {VERSION}")),
                ("Current channel", channel_name(current).to_owned()),
                ("New channel", channel_name(chosen).to_owned()),
            ])
            .text(explanation);
        if side_by_side {
            return screen
                .band(
                    BandAlign::Top,
                    [
                        (
                            SlotWidth::Fill,
                            Box::new(move |slot: ScreenBuilder| {
                                slot.primary_button(CONFIRM_CHANNEL, confirm)
                            })
                                as Box<dyn FnOnce(ScreenBuilder) -> ScreenBuilder>,
                        ),
                        (
                            SlotWidth::Fill,
                            Box::new(|slot: ScreenBuilder| slot.button(CANCEL_CHANNEL, "Cancel")),
                        ),
                    ],
                )
                .build();
        }
        screen
            .primary_button(CONFIRM_CHANNEL, confirm)
            .button(CANCEL_CHANNEL, "Cancel")
            .build()
    }

    fn bluetooth_prefix(&self) -> ScreenBuilder {
        let summary = match self.bluetooth_state {
            RadioState::Unknown => "Checking…".to_owned(),
            RadioState::On => match self
                .devices
                .iter()
                .filter(|device| device.connected)
                .count()
            {
                0 => "Nothing connected".to_owned(),
                connected => format!("{connected} connected"),
            },
            _ => "Turn on to find devices".to_owned(),
        };
        let mut screen = ScreenBuilder::new("settings-bluetooth")
            .top_bar("Bluetooth")
            .owns_back(true)
            .toggle(TOGGLE, "Bluetooth", summary, self.bluetooth_state.enabled());
        if let Some(trouble) = self.banner_for(Topic::Bluetooth) {
            screen = screen.banner(BannerLevel::Error, trouble);
        } else if self.restart_on_exit {
            screen = screen.banner(BannerLevel::Info,
                "Bluetooth shares Wi-Fi and starts once per boot. Leaving Settings restarts the reader. Saved content is kept.");
        }
        screen
    }

    fn bluetooth_rows(&self, context: &Context) -> Vec<SettingsRow> {
        self.devices
            .iter()
            .map(|device| {
                SettingsRow::new(
                    None,
                    bluetooth_action(&device.address),
                    context.clamped_row(&device.name, 2, true),
                    if device.connected {
                        "Connected"
                    } else if device.paired {
                        "Paired · Tap to connect"
                    } else {
                        "Available · Tap to pair"
                    },
                    if device.connected {
                        Glyph::Check
                    } else {
                        Glyph::Circle
                    },
                )
            })
            .collect()
    }

    fn bluetooth_pages(&self, context: &Context) -> Vec<Vec<usize>> {
        list_pages(
            context,
            &self.bluetooth_rows(context),
            true,
            self.bluetooth_prefix().section("Devices"),
        )
    }

    fn bluetooth_for(&self, context: &Context) -> Screen {
        if matches!(
            self.bluetooth_state,
            RadioState::Unavailable | RadioState::Unsupported
        ) {
            return ScreenBuilder::new("settings-bluetooth")
                .top_bar("Bluetooth")
                .owns_back(true)
                .text(if self.bluetooth_state == RadioState::Unavailable {
                    "This reader has no Bluetooth."
                } else {
                    "This version of Cobalt cannot use Bluetooth on this reader."
                })
                .build();
        }
        let mut screen = self.bluetooth_prefix();
        if self.bluetooth_state.enabled() {
            if self.devices.is_empty() {
                screen = screen.text("No devices found. Put headphones or a keyboard in pairing mode, then rescan.")
                    .bottom_action(RESCAN, "Rescan for devices");
            } else {
                let rows = self.bluetooth_rows(context);
                let pages = self.bluetooth_pages(context);
                let page = self.bluetooth_page.min(pages.len().saturating_sub(1));
                screen = screen
                    .section("Devices")
                    .rows(pages[page].iter().map(|&index| rows[index].as_row()));
                screen = page_controls(screen, page, pages.len()).bottom_action(RESCAN, "Rescan");
            }
        }
        screen.build()
    }

    #[cfg(test)]
    fn bluetooth(&self) -> Screen {
        self.bluetooth_for(&Context::default())
    }

    fn wifi_prefix(&self) -> ScreenBuilder {
        // The network joined is said under the switch, where the "Connected"
        // fact under the heading used to be. Its row in the list below has
        // the whole name if a long one is cut short here.
        let summary = match (self.wifi_state, &self.connected_ssid) {
            (RadioState::Unknown, _) => "Checking…".to_owned(),
            (RadioState::On, Some(ssid)) => format!("Connected to {ssid}"),
            (RadioState::On, None) => "Not connected".to_owned(),
            _ => "Turn on to see networks".to_owned(),
        };
        let mut screen = ScreenBuilder::new("settings-wifi")
            .top_bar("Wi-Fi")
            .owns_back(true)
            .toggle(TOGGLE, "Wi-Fi", summary, self.wifi_state.enabled());
        if let Some(trouble) = self.banner_for(Topic::Wifi) {
            screen = screen.banner(BannerLevel::Error, trouble);
        }
        screen
    }

    fn wifi_rows(&self, context: &Context) -> Vec<SettingsRow> {
        self.networks
            .iter()
            .map(|network| {
                let security = if network.secured {
                    "Password"
                } else {
                    "No password"
                };
                let signal = signal_words(network.signal_dbm);
                let summary = if network.connected {
                    format!("Connected · {signal} · Tap to disconnect")
                } else {
                    format!("{security} · {signal}")
                };
                SettingsRow::new(
                    None,
                    network_action(&network.ssid),
                    context.clamped_row(&network.ssid, 2, false),
                    summary,
                    Glyph::Wifi,
                )
            })
            .collect()
    }

    fn wifi_pages(&self, context: &Context) -> Vec<Vec<usize>> {
        list_pages(
            context,
            &self.wifi_rows(context),
            false,
            self.wifi_prefix().section("Networks"),
        )
    }

    fn wifi_for(&self, context: &Context) -> Screen {
        if matches!(
            self.wifi_state,
            RadioState::Unavailable | RadioState::Unsupported
        ) {
            return ScreenBuilder::new("settings-wifi")
                .top_bar("Wi-Fi")
                .owns_back(true)
                .text(if self.wifi_state == RadioState::Unavailable {
                    "This reader has no Wi-Fi."
                } else {
                    "This version of Cobalt cannot use Wi-Fi on this reader."
                })
                .build();
        }
        let mut screen = self.wifi_prefix();
        if self.wifi_state.enabled() {
            if self.networks.is_empty() {
                screen = screen.text(if self.scanning {
                    "Looking for networks…"
                } else {
                    "No networks found."
                });
            } else {
                let rows = self.wifi_rows(context);
                let pages = self.wifi_pages(context);
                let page = self.wifi_page.min(pages.len().saturating_sub(1));
                screen = screen
                    .section("Networks")
                    .rows(pages[page].iter().map(|&index| rows[index].as_row()));
                screen = page_controls(screen, page, pages.len());
            }
        }
        screen.build()
    }

    #[cfg(test)]
    fn wifi(&self) -> Screen {
        self.wifi_for(&Context::default())
    }

    /// The password for a secured network, typed on keys this application
    /// draws.
    ///
    /// Not the runtime's text field: a secret typed there is saved by the
    /// runtime and never handed back, and `JoinWifi` needs the password
    /// itself. The runtime field has no mode that masks a secret and still
    /// returns it, so this one stays app-drawn until it does.
    ///
    /// The field is a secret field, so the runtime draws a dot per
    /// character. It is sent a mask of the same length rather than the
    /// password, so the password is in no frame the runtime keeps.
    fn wifi_password(&self) -> Screen {
        let typed = "x".repeat(self.password.text().chars().count());
        let network = self.selected_ssid.as_deref().unwrap_or("this network");
        let mut screen = ScreenBuilder::new("settings-wifi-password")
            .top_bar("Join Wi-Fi")
            .owns_back(true)
            .field(PASSWORD_FIELD, typed, "Type the network password")
            .field_label(format!("Password for {network}"))
            .field_hint("8 to 63 characters")
            .field_kind(FieldKind::Secret);
        if let Some(trouble) = self.banner_for(Topic::Wifi) {
            screen = screen.field_error(trouble);
        }
        screen.keyboard(&self.password, "Join").build()
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
        // A failed reading carries the way to read again, in place of the
        // button at the foot.
        let trouble = self.banner_for(Topic::Battery);
        if let Some(trouble) = &trouble {
            screen = screen
                .banner(BannerLevel::Error, trouble.clone())
                .banner_action(RESCAN, "Read again");
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
        if trouble.is_none() {
            screen = screen.button(RESCAN, "Read again");
        }
        screen.build()
    }

    #[cfg(test)]
    fn about(&self) -> Screen {
        self.about_for(&Context::default())
    }

    /// The parts of the About page, in reading order, each kept whole on
    /// whichever page it lands: the reader itself, how a computer finds it,
    /// and the details support asks for. Empty until the runtime has said
    /// who this reader is.
    fn about_parts(&self) -> Vec<AboutPart> {
        let Some(identity) = &self.identity else {
            return Vec::new();
        };
        let mut facts: Vec<(String, String)> = vec![
            ("Model".to_owned(), identity.model.clone()),
            ("Cobalt".to_owned(), identity.runtime_version.clone()),
        ];
        if !identity.firmware.is_empty() {
            facts.push(("Kobo software".to_owned(), identity.firmware.clone()));
        }
        if let Some(address) = &self.address {
            facts.push(("Wi-Fi address".to_owned(), address.clone()));
        }
        let mut support: Vec<(String, String)> = vec![
            ("Profile".to_owned(), identity.profile_id.clone()),
            (
                "Screen".to_owned(),
                format!(
                    "{} × {} pixels",
                    identity.panel_width, identity.panel_height
                ),
            ),
        ];
        if identity.device_code != 0 {
            support.push(("Device code".to_owned(), identity.device_code.to_string()));
        }
        if !identity.kernel.is_empty() {
            support.push(("Kernel".to_owned(), identity.kernel.clone()));
        }
        let computer = if self.address.is_some() {
            "Once kobo setup has saved this reader, kobo commands find it on Wi-Fi. If one asks which reader, give it --device and the Wi-Fi address."
        } else {
            "Run kobo setup once with the reader plugged in over USB. Then kobo commands find it on Wi-Fi."
        };
        vec![
            AboutPart::Facts("This reader", facts),
            AboutPart::Text("From your computer", computer),
            AboutPart::Facts("For support", support),
        ]
    }

    /// Which parts go on which page. One page wherever the whole of About
    /// fits, which is every size but the largest few; there, each part moves
    /// to the next page rather than running off the foot of this one, so the
    /// owner who needs big type turns a page instead of losing a fact.
    fn about_pages(&self, context: &Context) -> Vec<Vec<usize>> {
        let count = self.about_parts().len();
        let all = (0..count).collect::<Vec<_>>();
        let metrics = context.metrics();
        // Measured as the last page, the one that also carries Read again,
        // so a page that fits here fits wherever it lands.
        let fits = |parts: &[usize], pages: usize| {
            !self
                .about_page_screen(parts, pages - 1, pages)
                .diagnostics(&metrics, &kobo_sdk::Chrome::measuring(true))
                .has_errors()
        };
        if count == 0 || fits(&all, 1) {
            return vec![all];
        }
        let mut pages: Vec<Vec<usize>> = Vec::new();
        for part in all {
            match pages.last_mut() {
                Some(page) if fits(&[page.as_slice(), &[part]].concat(), 2) => page.push(part),
                _ => pages.push(vec![part]),
            }
        }
        pages
    }

    /// Everything on this page names the reader it is drawn on: the matched
    /// profile, the firmware and kernel read from the hardware when the page
    /// asks, and the version this runtime was compiled as. A photograph of
    /// this page doubles as evidence that a build ran on real hardware, but
    /// the page itself stays a plain product page: the reader who opens it
    /// is a customer looking at their device, not a contributor.
    fn about_for(&self, context: &Context) -> Screen {
        let pages = self.about_pages(context);
        let page = self.about_page.min(pages.len().saturating_sub(1));
        self.about_page_screen(&pages[page], page, pages.len())
    }

    fn about_page_screen(&self, parts: &[usize], page: usize, pages: usize) -> Screen {
        let mut screen = ScreenBuilder::new("settings-about")
            .top_bar("About")
            .owns_back(true);
        let trouble = self.banner_for(Topic::About);
        if let Some(trouble) = &trouble {
            screen = screen
                .banner(BannerLevel::Error, trouble.clone())
                .banner_action(RESCAN, "Read again");
        }
        if self.identity.is_none() {
            return screen.text("Reading this reader's details.").build();
        }
        let all = self.about_parts();
        for part in parts.iter().filter_map(|&index| all.get(index)) {
            screen = match part {
                AboutPart::Facts(title, facts) => screen.section(*title).facts(facts.clone()),
                AboutPart::Text(title, text) => screen.section(*title).text(*text),
            };
        }
        // Read again sits at the foot of the last page, after everything it
        // reads, and only when a banner is not already offering it.
        if trouble.is_none() && page + 1 >= pages {
            screen = screen.button(RESCAN, "Read again");
        }
        page_controls(screen, page, pages).build()
    }

    fn refresh(context: &mut Context) {
        context.device().read_bluetooth();
        context.device().read_wifi();
        context.device().read_battery_detail();
        context.device().read_auto_update();
        context.device().read_update_channel();
    }

    /// Asks GitHub what the newest published release is. Nothing is
    /// downloaded beyond the release description until the reader chooses to
    /// install.
    fn check_for_update(&mut self, context: &mut Context) {
        let Some(channel) = self.update_channel else {
            self.update =
                UpdateFlow::Failed("Update preferences are still loading. Try again.".to_owned());
            return;
        };
        self.update = UpdateFlow::Checking { channel };
        self.update_task = context.spawn(Task::Fetch {
            url: match channel {
                UpdateChannel::Stable => RELEASES,
                UpdateChannel::Beta => BETA_RELEASES,
            }
            .to_owned(),
            offset: 0,
            max_bytes: release_window(channel),
            credential: None,
            headers: Vec::new(),
        });
        if self.update_task.is_none() {
            self.update = UpdateFlow::Failed(NO_NETWORK_ALLOWED.to_owned());
        }
    }

    /// Turns one of the two standing update switches while restating the
    /// other, so the runtime always hears a complete choice. Nothing happens
    /// until the runtime has answered the first read: a switch drawn from a
    /// guess must not be flippable into recording the opposite of what it
    /// showed.
    fn flip_auto_update(&self, context: &mut Context, choice: &str) {
        if let Some((cobalt, apps)) = self.auto_update {
            match choice {
                AUTO_COBALT => context.device().set_auto_update(!cobalt, apps),
                AUTO_APPS => context.device().set_auto_update(cobalt, !apps),
                _ => {}
            }
        }
    }

    fn update_channel_action(&mut self, context: &mut Context, action: ActionId) -> bool {
        if action == action_id(CHANNEL_STABLE) || action == action_id(CHANNEL_BETA) {
            // Only the answer that is not current asks anything; the current
            // one is already chosen.
            if let Some(channel) = self.update_channel {
                if action == action_id(channel_action(opposite_channel(channel))) {
                    self.view = View::UpdateChannelConfirm;
                    self.show(context);
                }
            }
            return true;
        }
        if action == action_id(CHANNEL_PREVIOUS) || action == action_id(CHANNEL_NEXT) {
            return true;
        }
        if action == action_id(CONFIRM_CHANNEL) && self.view == View::UpdateChannelConfirm {
            if let Some(channel) = self.update_channel {
                self.view = View::UpdateOptions;
                context
                    .device()
                    .set_update_channel(opposite_channel(channel));
                self.show(context);
            }
            return true;
        }
        if action == action_id(CANCEL_CHANNEL) && self.view == View::UpdateChannelConfirm {
            self.view = View::UpdateOptions;
            self.show(context);
            return true;
        }
        false
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
            self.update = UpdateFlow::Failed(
                "The update's download link could not be used. Try again later.".to_owned(),
            );
            self.show(context);
        }
    }

    /// Takes the reply to whichever update fetch was in flight: the release
    /// description, signed manifest, then detached signature.
    fn took_update_reply(&mut self, context: &mut Context, bytes: &[u8]) {
        let body = String::from_utf8_lossy(bytes);
        match self.update.clone() {
            UpdateFlow::Checking { channel } => match latest_release(&body, channel) {
                Err(reason) => self.update = UpdateFlow::Failed(reason),
                Ok(release) if release.newer_than(VERSION) => {
                    self.update_task = context.spawn(Task::Fetch {
                        url: release.manifest.clone(),
                        offset: 0,
                        max_bytes: 64 * 1024,
                        credential: None,
                        headers: Vec::new(),
                    });
                    self.update = if self.update_task.is_none() {
                        UpdateFlow::Failed(NO_NETWORK_ALLOWED.to_owned())
                    } else {
                        UpdateFlow::Manifest { release }
                    };
                }
                Ok(release) if release.older_than(VERSION) => {
                    self.update = UpdateFlow::Ahead {
                        latest: release.version,
                    };
                }
                Ok(release) => {
                    self.update = UpdateFlow::UpToDate {
                        latest: release.version,
                    };
                }
            },
            UpdateFlow::Manifest { release } => {
                self.update_task = context.spawn(Task::Fetch {
                    url: release.signature.clone(),
                    offset: 0,
                    max_bytes: 1024,
                    credential: None,
                    headers: Vec::new(),
                });
                self.update = if self.update_task.is_none() {
                    UpdateFlow::Failed(NO_NETWORK_ALLOWED.to_owned())
                } else {
                    UpdateFlow::ManifestSignature {
                        release,
                        manifest: bytes.to_vec(),
                    }
                };
            }
            UpdateFlow::ManifestSignature { release, manifest } => {
                self.update = match verified_device_digest(&release, &manifest, &body) {
                    Some(sha256) => UpdateFlow::Ready {
                        version: release.version,
                        url: release.archive,
                        sha256,
                    },
                    None => UpdateFlow::Failed(
                        "The update could not be verified, so it was not installed. Try again later."
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

    /// Answers a radio row that this runtime can never read.
    ///
    /// Only the success arm ever moved a radio off `Unknown`, and `Unknown`
    /// is drawn as "Checking…". A reader whose Bluetooth the runtime cannot
    /// drive therefore sat on "Checking…" for the rest of the session while
    /// the banner directly under it already said the reading would never
    /// come: one row promising a result, one banner saying there would not be
    /// one. `Unsupported` is a settled answer, so the row gives it.
    ///
    /// Only `Unsupported` settles anything. Every other failure is a reading
    /// that may yet succeed, and claiming the radio is absent because one
    /// read failed would be a worse lie than the one this fixes.
    fn settle_unreadable_radio(&mut self, topic: Topic, reason: DenyReason) {
        if reason != DenyReason::Unsupported {
            return;
        }
        match topic {
            Topic::Bluetooth => self.bluetooth_state = RadioState::Unsupported,
            Topic::Wifi => self.wifi_state = RadioState::Unsupported,
            Topic::Battery | Topic::About => {}
        }
    }

    /// The address the runtime gave this application, if it is one.
    ///
    /// Checked rather than shown as given, because whatever is on this page
    /// is what the owner will type into a command on their computer.
    fn read_address() -> Option<String> {
        std::env::var(READER_ADDRESS_VARIABLE)
            .ok()
            .and_then(|value| value.trim().parse::<std::net::IpAddr>().ok())
            .map(|address| address.to_string())
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
    fn turn_page(&mut self, context: &Context, forward: bool) {
        let pages = match self.view {
            View::Home => self.home_pages(context).len(),
            View::Bluetooth => self.bluetooth_pages(context).len(),
            View::Wifi => self.wifi_pages(context).len(),
            View::About => self.about_pages(context).len(),
            _ => return,
        };
        let page = match self.view {
            View::Home => &mut self.home_page,
            View::Bluetooth => &mut self.bluetooth_page,
            View::Wifi => &mut self.wifi_page,
            View::About => &mut self.about_page,
            _ => return,
        };
        let last = pages.saturating_sub(1);
        *page = if forward {
            (page.saturating_add(1)).min(last)
        } else {
            (*page).min(last).saturating_sub(1)
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
    /// Keystrokes for the Wi-Fi password screen, which owns every action
    /// while it is up: letters go to the keyboard, and the only ways off the
    /// screen are submitting a password or going back.
    fn password_action(&mut self, context: &mut Context, action: ActionId) {
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
                        "A Wi-Fi password has 8 to 63 characters.".to_owned(),
                    ));
                }
            } else {
                self.settled(Topic::Wifi);
            }
            self.show(context);
        }
    }

    /// Files a failure or a refusal under whatever was asked for.
    ///
    /// When the request is not one of the rows, the standing update choices
    /// or the update screen, there is nowhere honest to show it, so it is
    /// dropped rather than shown under an unrelated heading. A refusal of a
    /// radio reading may also settle that radio; see
    /// [`Self::settle_unreadable_radio`].
    fn took_trouble(&mut self, request: &DeviceRequest, words: &str, refused: Option<DenyReason>) {
        if is_standing_choice(request) {
            self.choice_trouble = Some(words.to_owned());
        }
        if update_screen_owns(request) {
            self.update = UpdateFlow::Failed(words.to_owned());
        } else if let Some(topic) = Topic::of(request) {
            if let Some(reason) = refused {
                self.settle_unreadable_radio(topic, reason);
            }
            self.fail(topic, words);
        }
    }

    fn took_update_channel(&mut self, request: &DeviceRequest, channel: UpdateChannel) {
        self.update_channel = Some(channel);
        if matches!(request, DeviceRequest::SetUpdateChannel { .. }) {
            self.update = UpdateFlow::Idle;
            self.update_task = None;
        }
    }
}

impl KoboApp for Settings {
    /// The platform's own blue: this screen is Cobalt itself rather than an
    /// application running on it, and it wears the colour of the logo.
    fn accent(&self) -> Option<kobo_sdk::Accent> {
        Some(kobo_sdk::Accent::Cobalt)
    }

    fn on_start(&mut self, context: &mut Context) {
        self.address = Self::read_address();
        Self::refresh(context);
        self.show(context);
    }

    fn on_page_turn(&mut self, context: &mut Context, forward: bool) {
        if matches!(self.view, View::Home | View::Bluetooth | View::Wifi) {
            self.turn_page(context, forward);
            self.show(context);
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one explicit dispatch table for every screen"
    )]
    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if self.view == View::WifiPassword {
            self.password_action(context, action);
            return;
        }
        if self.update_channel_action(context, action) {
            return;
        }
        if action == ActionId::BACK {
            self.view = match self.view {
                View::UpdateChannelConfirm => View::UpdateOptions,
                View::UpdateOptions => View::Update,
                _ => View::Home,
            };
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
        } else if action == action_id(ABOUT) {
            self.view = View::About;
            self.about_page = 0;
            context.device().read_identity();
            self.show(context);
        } else if action == action_id(UPDATE) {
            self.view = View::Update;
            self.show(context);
        } else if action == action_id(UPDATE_OPTIONS) {
            self.view = View::UpdateOptions;
            self.show(context);
        } else if action == action_id(JOIN_WIFI) {
            self.view = View::Wifi;
            context.device().read_wifi();
            self.show(context);
        } else if action == action_id(CHECK) {
            self.check_for_update(context);
            self.show(context);
        } else if action == action_id(INSTALL) {
            self.install_update(context);
        } else if action == action_id(AUTO_COBALT) {
            self.flip_auto_update(context, AUTO_COBALT);
        } else if action == action_id(AUTO_APPS) {
            self.flip_auto_update(context, AUTO_APPS);
        } else if action == action_id(CLOSE) && matches!(self.update, UpdateFlow::Installed { .. })
        {
            context.exit();
        } else if action == action_id(TOGGLE) {
            match self.view {
                View::Bluetooth => context
                    .device()
                    .set_bluetooth(!self.bluetooth_state.enabled()),
                View::Wifi => context.device().set_wifi(!self.wifi_state.enabled()),
                View::Home
                | View::WifiPassword
                | View::Battery
                | View::About
                | View::Update
                | View::UpdateOptions
                | View::UpdateChannelConfirm => {}
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
                View::About => context.device().read_identity(),
                View::Home
                | View::WifiPassword
                | View::Update
                | View::UpdateOptions
                | View::UpdateChannelConfirm => {}
            }
            self.show(context);
        } else if action == action_id(MORE) {
            self.turn_page(context, true);
            self.show(context);
        } else if action == action_id(PREVIOUS) {
            self.turn_page(context, false);
            self.show(context);
        } else if self.view == View::Bluetooth {
            if let Some(index) = self
                .devices
                .iter()
                .position(|device| action == action_id(&bluetooth_action(&device.address)))
            {
                self.choose_bluetooth(context, index);
            }
        } else if self.view == View::Wifi {
            if let Some(index) = self
                .networks
                .iter()
                .position(|network| action == action_id(&network_action(&network.ssid)))
            {
                self.choose_network(context, index);
            }
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
            DeviceResult::AutoUpdate { cobalt, apps } => {
                self.auto_update = Some((cobalt, apps));
                self.choice_trouble = None;
            }
            DeviceResult::UpdateChannel(channel) => {
                self.took_update_channel(&request, channel);
                self.choice_trouble = None;
            }
            DeviceResult::Identity(identity) => {
                self.identity = Some(identity);
                self.settled(Topic::About);
            }
            // A failure belongs to whatever was asked for.
            DeviceResult::Failed(error) => {
                self.took_trouble(&request, failure_words(error), None);
            }
            DeviceResult::Denied(reason) => {
                self.took_trouble(&request, refusal_words(reason), Some(reason));
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
            | DeviceResult::Dictionary { .. }
            | DeviceResult::Apps { .. }
            | DeviceResult::AppLink(_)
            | DeviceResult::RemoteInstall(_)
            | DeviceResult::Library { .. }
            | DeviceResult::LibraryDocument { .. }
            | DeviceResult::Secrets { .. }
            | DeviceResult::Text(_) => {}
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
                TaskOutcome::Failed(TaskError::Offline) => self.update = UpdateFlow::Offline,
                TaskOutcome::Failed(error) => {
                    self.update = UpdateFlow::Failed(check_failure_words(error).to_owned());
                }
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
            TaskOutcome::Failed(_) => {
                let topic = match self.pending {
                    Some(Pending::WifiRefresh) => Topic::Wifi,
                    _ => Topic::Bluetooth,
                };
                self.fail(topic, "The scan could not finish. Try again.");
            }
            TaskOutcome::Cancelled => {}
        }
        self.show(context);
    }
}

/// What Settings says when it was not allowed the network at all.
const NO_NETWORK_ALLOWED: &str =
    "Settings is not allowed to use the network in this version of Cobalt.";

/// A signal strength in the words a phone uses, not in decibel-milliwatts.
fn signal_words(dbm: i16) -> &'static str {
    match dbm {
        -60.. => "Strong signal",
        -70..=-61 => "Good signal",
        _ => "Weak signal",
    }
}

/// What a refused request means to the person holding the reader.
///
/// The protocol's own wording is written for whoever is debugging the
/// runtime ("not supported by this runtime on this hardware") and was shown
/// here as it came.
const fn refusal_words(reason: DenyReason) -> &'static str {
    match reason {
        DenyReason::NotDeclared | DenyReason::PolicyRejected => {
            "Cobalt did not allow Settings to do that."
        }
        DenyReason::WithheldForBattery => {
            "The battery is too low for that. Charge the reader, then try again."
        }
        DenyReason::Unsupported => "This version of Cobalt cannot do that on this reader.",
        DenyReason::Busy => "Something else is using that right now. Try again in a moment.",
    }
}

/// What a failed request means to the person holding the reader.
const fn failure_words(error: DeviceError) -> &'static str {
    match error {
        DeviceError::NotFound => {
            "That network or device was not found. Check that it is in range, then try again."
        }
        DeviceError::Authentication => {
            "The password or pairing was not accepted. Check it, then try again."
        }
        DeviceError::TimedOut => "That took too long. Try again.",
        DeviceError::Unreachable => "That could not be reached. Check Wi-Fi, then try again.",
        DeviceError::InvalidInput => "What came back could not be used. Try again later.",
        DeviceError::Backend => "The reader could not finish that. Try again.",
        DeviceError::Integrity => {
            "The download did not match what was published, so it was not used. Try again later."
        }
    }
}

/// Why asking GitHub for the newest release failed, with what to do.
const fn check_failure_words(error: TaskError) -> &'static str {
    match error {
        TaskError::Offline => "This reader is not on Wi-Fi. Join a network, then check again.",
        TaskError::Unreachable | TaskError::TimedOut => {
            "GitHub did not answer. Check that Wi-Fi reaches the internet, then try again."
        }
        TaskError::RateLimited(_) => {
            "GitHub asked this reader to wait before checking again. Try again in an hour."
        }
        TaskError::Denied | TaskError::NoCredential | TaskError::Unauthorized => NO_NETWORK_ALLOWED,
        TaskError::NotFound | TaskError::TooLarge => {
            "GitHub's list of releases could not be read. Try again later."
        }
    }
}

// A scanner can reorder rows between a drawn tap and its callback. Bind each
// action to the identity the user saw, rather than its old list position.
fn bluetooth_action(address: &str) -> String {
    format!("bt-device-{address}")
}
fn network_action(ssid: &str) -> String {
    format!("wifi-network-{ssid}")
}

struct SettingsRow {
    section: Option<&'static str>,
    action: String,
    title: String,
    summary: String,
    lead: RowLead,
}

impl SettingsRow {
    fn new(
        section: Option<&'static str>,
        action: impl Into<String>,
        title: impl Into<String>,
        summary: impl Into<String>,
        glyph: Glyph,
    ) -> Self {
        Self {
            section,
            action: action.into(),
            title: title.into(),
            summary: summary.into(),
            lead: glyph.into(),
        }
    }
    fn as_row(&self) -> (&str, &str, &str, RowLead) {
        (&self.action, &self.title, &self.summary, self.lead)
    }
}

fn list_pages(
    context: &Context,
    rows: &[SettingsRow],
    nav_bar: bool,
    prefix: ScreenBuilder,
) -> Vec<Vec<usize>> {
    let measured: Vec<_> = rows
        .iter()
        .map(|row| (row.title.as_str(), row.summary.as_str()))
        .collect();
    context.paginate_rows_under(
        &measured,
        nav_bar,
        kobo_sdk::Position::AtTheFoot,
        &prefix.build(),
    )
}

fn page_controls(screen: ScreenBuilder, page: usize, pages: usize) -> ScreenBuilder {
    if pages > 1 {
        screen.page_turns(PREVIOUS, MORE).page_position(
            u16::try_from(page + 1).unwrap_or(u16::MAX),
            u16::try_from(pages).unwrap_or(u16::MAX),
        )
    } else {
        screen
    }
}

/// The newest published release, as its assets name this device's download.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Release {
    version: String,
    /// Where the installable archive is.
    archive: String,
    /// Where the signed canonical release manifest is.
    manifest: String,
    /// Where the manifest's raw detached Ed25519 signature is.
    signature: String,
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

    fn older_than(&self, installed: &str) -> bool {
        match (numbers(&self.version), numbers(installed)) {
            (Some(latest), Some(installed)) => latest < installed,
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
    format!("cobalt-{version}-KoboRoot.tgz")
}

/// Reads the selected GitHub release feed down to the two URLs this device
/// needs. Beta selection is by numeric version rather than response order.
fn latest_release(body: &str, channel: UpdateChannel) -> Result<Release, String> {
    // A reply that filled its window and then failed to parse is a window
    // into a longer feed, not a broken one, and telling the reader their
    // release list is unreadable rubbish sends them looking for a fault at
    // GitHub that is not there.
    let value = kobo_json::parse(body).map_err(|_| {
        if body.len() >= release_window(channel) as usize {
            "GitHub's release list is longer than this reader can read.".to_owned()
        } else {
            "GitHub sent something that is not a release.".to_owned()
        }
    })?;
    match channel {
        UpdateChannel::Stable => stable_release(&value),
        UpdateChannel::Beta => value
            .as_array()
            .and_then(|releases| {
                releases
                    .iter()
                    .filter_map(|release| release_value(release, "beta-v", true))
                    .max_by_key(|release| numbers(&release.version))
            })
            .ok_or_else(|| "GitHub has no valid beta release for this reader.".to_owned()),
    }
}

fn stable_release(value: &Value) -> Result<Release, String> {
    let tag = value
        .get("tag_name")
        .and_then(Value::as_str)
        .ok_or_else(|| "The newest release does not name a version.".to_owned())?;
    let version = tag
        .strip_prefix('v')
        .filter(|version| numbers(version).is_some())
        .ok_or_else(|| "The newest release does not name a valid version.".to_owned())?
        .to_owned();
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
            .filter(|url| valid_release_url(url))
            .map(str::to_owned)
    };
    let archive = url_of(&archive_name(&version))
        .ok_or_else(|| "The newest release has no download for this reader.".to_owned())?;
    let manifest = url_of("cobalt-host-manifest.txt")
        .ok_or_else(|| "The newest release publishes no signed metadata.".to_owned())?;
    let signature = url_of("cobalt-host-manifest.txt.sig")
        .ok_or_else(|| "The newest release publishes no metadata signature.".to_owned())?;
    Ok(Release {
        version,
        archive,
        manifest,
        signature,
    })
}

fn release_value(value: &Value, tag_prefix: &str, prerelease: bool) -> Option<Release> {
    if value.get("draft").and_then(Value::as_bool).unwrap_or(false)
        || value
            .get("prerelease")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            != prerelease
    {
        return None;
    }
    let version = value
        .get("tag_name")
        .and_then(Value::as_str)?
        .strip_prefix(tag_prefix)?
        .to_owned();
    numbers(&version)?;
    let assets = value.get("assets").and_then(Value::as_array)?;
    let url_of = |name: &str| {
        assets
            .iter()
            .find(|asset| asset.get("name").and_then(Value::as_str) == Some(name))
            .and_then(|asset| asset.get("browser_download_url"))
            .and_then(Value::as_str)
            .filter(|url| valid_release_url(url))
            .map(str::to_owned)
    };
    Some(Release {
        archive: url_of(&archive_name(&version))?,
        manifest: url_of("cobalt-host-manifest.txt")?,
        signature: url_of("cobalt-host-manifest.txt.sig")?,
        version,
    })
}

fn valid_release_url(url: &str) -> bool {
    url.starts_with("https://") && url.len() <= kobo_sdk::MAX_URL_LEN
}

fn verified_device_digest(release: &Release, manifest: &[u8], signature: &str) -> Option<String> {
    let manifest = kobo_app_store::verify_release_manifest(manifest, signature).ok()?;
    device_digest_from_manifest(release, &manifest)
}

fn device_digest_from_manifest(
    release: &Release,
    manifest: &kobo_app_store::ReleaseManifest,
) -> Option<String> {
    if manifest.version != release.version {
        return None;
    }
    let device = manifest.device()?;
    (device.name == archive_name(&release.version)).then(|| device.sha256.clone())
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
    use super::{
        RadioState, Settings, Topic, View, AUTO_APPS, AUTO_COBALT, CANCEL_CHANNEL, CHANNEL_BETA,
        CHANNEL_STABLE, CONFIRM_CHANNEL, MORE, PREVIOUS, RESCAN, TOGGLE, VERSION,
    };
    use kobo_sdk::{
        action_id, BannerLevel, BatteryDetail, BluetoothDevice, BluetoothDeviceKind, Chrome,
        DenyReason, DeviceIdentity, DeviceRequest, DeviceResult, Emphasis, Glyph, Node,
        UpdateChannel, WifiNetwork, CLARA_BW_METRICS,
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
    fn the_update_screen_leads_to_the_channel_and_both_automatic_update_switches() {
        let settings = Settings {
            view: View::UpdateOptions,
            auto_update: Some((true, false)),
            update_channel: Some(UpdateChannel::Beta),
            ..Settings::default()
        };
        let row = settings
            .update()
            .layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true))
            .rect_of_action(action_id(super::UPDATE_OPTIONS));
        assert!(row.is_some());
        assert_eq!(
            settings.update_options_summary().as_deref(),
            Some("Beta · Cobalt updates on its own")
        );
        let screen = settings.update_options();
        let issues = screen.validate(&CLARA_BW_METRICS);
        assert!(issues.is_empty(), "{issues:?}");
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true));
        assert!(layout.rect_of_action(action_id(AUTO_COBALT)).is_some());
        assert!(layout.rect_of_action(action_id(AUTO_APPS)).is_some());
        assert!(layout.rect_of_action(action_id(CHANNEL_STABLE)).is_some());
        assert!(layout.rect_of_action(action_id(CHANNEL_BETA)).is_some());
        // Each switch is drawn by the runtime from the runtime's own answer.
        let switches: Vec<_> = screen
            .nodes
            .iter()
            .filter_map(|node| match node {
                Node::Toggle { label, on, .. } => Some((label.as_str(), *on)),
                _ => None,
            })
            .collect();
        assert_eq!(
            switches,
            [
                ("Update Cobalt automatically", true),
                ("Update apps automatically", false)
            ]
        );
        assert!(screen.nodes.iter().any(|node| matches!(
            node,
            Node::Options { prompt, selected: Some(1), .. } if prompt == "Update channel"
        )));
        let text = text_of(&screen);
        assert!(text.contains("change back here at any time"), "{text}");
        assert!(text.contains("saved keys are kept"), "{text}");
        assert!(!text.contains("state, and secrets"), "{text}");
    }

    #[test]
    fn changing_update_channels_has_an_explicit_confirmation_screen() {
        for (current, chosen) in [
            (UpdateChannel::Stable, "Beta"),
            (UpdateChannel::Beta, "Stable"),
        ] {
            let settings = Settings {
                view: View::UpdateChannelConfirm,
                update_channel: Some(current),
                ..Settings::default()
            };
            let screen = settings.update_channel_confirmation();
            let issues = screen.validate(&CLARA_BW_METRICS);
            assert!(issues.is_empty(), "{issues:?}");
            let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true));
            assert!(layout.rect_of_action(action_id(CONFIRM_CHANNEL)).is_some());
            assert!(layout.rect_of_action(action_id(CANCEL_CHANNEL)).is_some());
            let text = text_of(&screen);
            assert!(facts_of(&screen).contains(&format!("Cobalt {VERSION}")));
            assert!(text.contains(chosen), "{text}");
            assert!(text.contains("are kept"), "{text}");
            assert!(!text.contains("metadata"), "{text}");
        }
    }

    #[test]
    fn the_switches_wait_for_the_runtime_to_answer_before_being_drawn() {
        let settings = Settings {
            view: View::Update,
            ..Settings::default()
        };
        let update = settings
            .update()
            .layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true));
        assert!(update
            .rect_of_action(action_id(super::UPDATE_OPTIONS))
            .is_none());
        let layout = settings
            .update_options()
            .layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true));
        assert!(layout.rect_of_action(action_id(AUTO_COBALT)).is_none());
        assert!(layout.rect_of_action(action_id(AUTO_APPS)).is_none());
        assert!(layout.rect_of_action(action_id(CHANNEL_BETA)).is_none());
    }

    #[test]
    fn an_installed_beta_ahead_of_stable_is_not_reported_as_the_stable_version() {
        let settings = Settings {
            view: View::Update,
            update: super::UpdateFlow::Ahead {
                latest: "0.3.1".to_owned(),
            },
            ..Settings::default()
        };
        let text = text_of(&settings.update());
        assert!(text.contains("already running a newer version"));
        assert!(text.contains("nothing is downgraded"));
        assert!(!text.contains("it is what this reader is running"));
    }

    #[test]
    fn changing_channels_clears_only_status_from_the_previous_channel() {
        let mut settings = Settings {
            update: super::UpdateFlow::Ahead {
                latest: "0.3.1".to_owned(),
            },
            ..Settings::default()
        };
        settings.took_update_channel(&DeviceRequest::ReadUpdateChannel, UpdateChannel::Stable);
        assert!(matches!(settings.update, super::UpdateFlow::Ahead { .. }));

        settings.took_update_channel(
            &DeviceRequest::SetUpdateChannel {
                channel: UpdateChannel::Beta,
            },
            UpdateChannel::Beta,
        );
        assert_eq!(settings.update_channel, Some(UpdateChannel::Beta));
        assert_eq!(settings.update, super::UpdateFlow::Idle);
    }

    #[test]
    fn a_radio_the_runtime_cannot_read_stops_saying_it_is_being_checked() {
        // The bug this pins: only a successful reading moved a radio off
        // Unknown, and Unknown draws as "Checking...". On hardware whose
        // Bluetooth this runtime cannot drive, the home row promised a
        // reading for the rest of the session while the banner under it
        // already said the reading would never come.
        let mut settings = Settings::default();
        assert_eq!(settings.bluetooth_state, RadioState::Unknown);
        let before = format!("{:?}", settings.home());
        assert!(before.contains("Checking"), "{before}");

        settings.settle_unreadable_radio(Topic::Bluetooth, DenyReason::Unsupported);

        assert_eq!(settings.bluetooth_state, RadioState::Unsupported);
        let after = format!("{:?}", settings.home());
        assert!(
            after.contains("Not available in this version of Cobalt"),
            "{after}"
        );
    }

    #[test]
    fn a_refused_bluetooth_reading_settles_the_row_through_the_runtime() {
        // Driven through on_device_result rather than the helper, because the
        // helper passing proves only that the helper works: the defect was
        // that the refusal never reached a state change at all, and a test
        // that calls the transition directly would pass with the call site
        // deleted.
        use kobo_sdk::AppRunner;
        let mut runner = AppRunner::new(Settings::default());
        runner.start();
        assert_eq!(runner.app().bluetooth_state, RadioState::Unknown);

        // read_bluetooth is the first reading refresh asks for, so the first
        // answer delivered is the one it is matched to.
        runner.device_result(DeviceResult::Denied(DenyReason::Unsupported));

        assert_eq!(runner.app().bluetooth_state, RadioState::Unsupported);
        let home = format!("{:?}", runner.app().home());
        assert!(
            home.contains("Not available in this version of Cobalt"),
            "{home}"
        );
        // The refusal is about the runtime, so the screen must not go on to
        // claim the hardware is missing: this reader may well have a radio
        // the runtime has simply not brought up.
        let screen = text_of(&runner.app().bluetooth());
        assert!(
            screen.contains("This version of Cobalt cannot use Bluetooth on this reader."),
            "{screen}"
        );
        assert!(!screen.contains("has no Bluetooth"), "{screen}");
    }

    #[test]
    fn a_radio_that_merely_failed_once_is_still_worth_reading_again() {
        // The narrower half of the same fix: a refusal for any other reason
        // may yet be followed by a reading that succeeds, so the row must not
        // claim the hardware is absent on the strength of one refusal.
        let mut settings = Settings::default();

        settings.settle_unreadable_radio(Topic::Bluetooth, DenyReason::Busy);

        assert_eq!(settings.bluetooth_state, RadioState::Unknown);
    }

    #[test]
    fn a_reader_with_no_bluetooth_hardware_is_told_so_without_a_dead_end_toggle() {
        // The bug this pins: a toggle drawn regardless of hardware invited
        // the one action that could never succeed, and only failed once
        // tapped, on a Libra H2O with no Bluetooth radio at all.
        let settings = Settings {
            bluetooth_state: RadioState::Unavailable,
            ..Settings::default()
        };
        let screen = settings.bluetooth();
        let issues = screen.validate(&CLARA_BW_METRICS);
        assert!(issues.is_empty(), "{issues:?}");
        assert!(text_of(&screen).contains("This reader has no Bluetooth."));
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true));
        assert!(layout.rect_of_action(action_id(TOGGLE)).is_none());
    }

    #[test]
    fn a_reader_with_no_wifi_hardware_is_told_so_without_a_dead_end_toggle() {
        let settings = Settings {
            wifi_state: RadioState::Unavailable,
            ..Settings::default()
        };
        let screen = settings.wifi();
        let issues = screen.validate(&CLARA_BW_METRICS);
        assert!(issues.is_empty(), "{issues:?}");
        assert!(text_of(&screen).contains("This reader has no Wi-Fi."));
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true));
        assert!(layout.rect_of_action(action_id(TOGGLE)).is_none());
    }

    /// The bug this pins: `RadioState::Unavailable` is also the enum's
    /// default before the first `read_bluetooth`/`read_wifi` reply, and what
    /// a denied or failed read leaves it as. Before `Unknown` existed as a
    /// separate state, both screens above claimed the reader's hardware
    /// outright had no radio during that window -- true only once a
    /// successful reply has actually said so.
    #[test]
    fn a_radio_with_no_answer_yet_is_not_claimed_absent() {
        let settings = Settings::default();
        assert_eq!(settings.bluetooth_state, RadioState::Unknown);
        assert_eq!(settings.wifi_state, RadioState::Unknown);
        let bluetooth = settings.bluetooth();
        assert!(!text_of(&bluetooth).contains("no Bluetooth hardware"));
        let wifi = settings.wifi();
        assert!(!text_of(&wifi).contains("no Wi-Fi hardware"));
    }

    #[test]
    fn a_failed_bluetooth_read_leaves_hardware_state_unknown_not_absent() {
        let mut settings = Settings::default();
        settings.fail(super::Topic::Bluetooth, "backend unavailable");
        assert_eq!(settings.bluetooth_state, RadioState::Unknown);
        assert!(!text_of(&settings.bluetooth()).contains("no Bluetooth hardware"));
    }

    #[test]
    fn a_full_bluetooth_scan_keeps_every_drawn_action_on_the_panel() {
        let settings = Settings {
            bluetooth_state: RadioState::On,
            devices: (0..10).map(bluetooth_device).collect(),
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
            networks: (0..10)
                .map(|index| WifiNetwork {
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
            networks: (0..10)
                .map(|index| WifiNetwork {
                    ssid: format!("Network {index}"),
                    signal_dbm: -40,
                    secured: true,
                    connected: false,
                })
                .collect(),
            ..Settings::default()
        };
        settings.wifi_page = settings.wifi_pages(&kobo_sdk::Context::default()).len() - 1;
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

    /// Every word the panel draws for a screen, banners and rows included.
    fn drawn_words(screen: &kobo_sdk::Screen) -> String {
        screen
            .layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true))
            .nodes
            .iter()
            .flat_map(|node| node.text_lines.clone())
            .collect::<Vec<_>>()
            .join(" ")
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

    fn facts_of(screen: &kobo_sdk::Screen) -> String {
        screen
            .nodes
            .iter()
            .filter_map(|node| match node {
                kobo_sdk::Node::Facts { entries, .. } => Some(
                    entries
                        .iter()
                        .flat_map(|(label, value)| [label.as_str(), value.as_str()])
                        .collect::<Vec<_>>()
                        .join(" "),
                ),
                kobo_sdk::Node::Section { title, value, .. } => Some(format!(
                    "{} {}",
                    title,
                    value.as_deref().unwrap_or_default()
                )),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The About page names the reader it is drawn on, so everything that
    /// identifies the reader must actually be on it, nothing on it may name
    /// the reader's owner, and none of the contributor's reasons for the
    /// page may leak into what a customer sees.
    #[test]
    fn the_about_page_names_the_reader_it_is_drawn_on() {
        let mut settings = Settings::default();
        assert!(text_of(&settings.about()).contains("Reading this reader's details."));
        settings.identity = Some(DeviceIdentity {
            profile_id: "CLARA_BW_391".to_owned(),
            model: "Kobo Clara BW".to_owned(),
            device_code: 391,
            firmware: "4.41.23145".to_owned(),
            kernel: "5.10.117".to_owned(),
            runtime_version: "0.3.0".to_owned(),
            panel_width: 1072,
            panel_height: 1448,
        });
        let screen = settings.about();
        let issues = screen.validate(&CLARA_BW_METRICS);
        assert!(issues.is_empty(), "{issues:?}");
        let drawn = format!("{:?}", screen.nodes);
        for needle in [
            "Kobo Clara BW",
            "CLARA_BW_391",
            "391",
            "4.41.23145",
            "5.10.117",
            "1072 × 1448",
            "0.3.0",
            "kobo setup",
        ] {
            assert!(drawn.contains(needle), "the page never draws {needle}");
        }
        for stray in ["photograph", "pull request"] {
            assert!(!drawn.contains(stray), "the page still talks shop: {stray}");
        }
    }

    #[test]
    fn the_password_keyboard_and_validation_message_fit_the_panel() {
        let settings = Settings {
            view: super::View::WifiPassword,
            selected_ssid: Some("A secured wireless network".to_owned()),
            trouble: Some((
                super::Topic::Wifi,
                "A Wi-Fi password has 8 to 63 characters.".to_owned(),
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

    #[test]
    fn auto_update_trouble_belongs_to_the_update_screen() {
        use kobo_sdk::DeviceRequest;
        assert!(super::update_screen_owns(&DeviceRequest::ReadAutoUpdate));
        assert!(super::update_screen_owns(&DeviceRequest::SetAutoUpdate {
            cobalt: true,
            apps: false,
        }));
        assert!(super::update_screen_owns(&DeviceRequest::ReadUpdateChannel));
        assert!(super::update_screen_owns(
            &DeviceRequest::SetUpdateChannel {
                channel: UpdateChannel::Beta,
            }
        ));
        assert!(
            super::Topic::of(&DeviceRequest::ReadAutoUpdate).is_none(),
            "auto-update trouble may not appear under an unrelated row"
        );
        assert!(
            !super::update_screen_owns(&DeviceRequest::ReadBatteryDetail),
            "a battery failure is not the update screen's to report"
        );
    }

    fn release_json(version: &str, with_metadata: bool) -> String {
        let archive = format!(
            r#"{{"name":"cobalt-{version}-KoboRoot.tgz","browser_download_url":"https://example.test/{version}/KoboRoot.tgz"}}"#
        );
        let metadata = if with_metadata {
            format!(
                r#",{{"name":"cobalt-host-manifest.txt","browser_download_url":"https://example.test/{version}/manifest"}},{{"name":"cobalt-host-manifest.txt.sig","browser_download_url":"https://example.test/{version}/manifest.sig"}}"#
            )
        } else {
            String::new()
        };
        format!(r#"{{"tag_name":"v{version}","assets":[{archive}{metadata}]}}"#)
    }

    #[test]
    fn a_release_is_read_down_to_the_signed_metadata_urls_this_reader_needs() {
        let release = super::latest_release(&release_json("9.9.9", true), UpdateChannel::Stable)
            .expect("a full release");
        assert_eq!(release.version, "9.9.9");
        assert_eq!(release.archive, "https://example.test/9.9.9/KoboRoot.tgz");
        assert_eq!(release.manifest, "https://example.test/9.9.9/manifest");
        assert_eq!(release.signature, "https://example.test/9.9.9/manifest.sig");
        assert!(release.newer_than(super::VERSION));
    }

    #[test]
    fn the_release_archive_name_is_device_neutral() {
        assert_eq!(super::archive_name("9.9.9"), "cobalt-9.9.9-KoboRoot.tgz");
    }

    #[test]
    fn a_release_without_signed_metadata_is_not_offered() {
        assert!(
            super::latest_release(&release_json("9.9.9", false), UpdateChannel::Stable)
                .expect_err("no signed metadata, no offer")
                .contains("signed metadata")
        );
    }

    #[test]
    fn a_feed_cut_off_at_its_window_is_reported_as_too_long_not_as_nonsense() {
        let window = super::release_window(UpdateChannel::Beta) as usize;
        let cut_off = r#"[{"tag_name":"beta-v9.8.0","draft":false,"prerelease":true,"assets":[{"#
            .to_owned()
            + &" ".repeat(window);
        assert!(
            super::latest_release(&cut_off, UpdateChannel::Beta)
                .expect_err("a window into a feed is not a feed")
                .contains("longer than this reader can read"),
            "a reply that filled its window names the length as the fault"
        );
        assert!(
            super::latest_release("<html>", UpdateChannel::Beta)
                .expect_err("not a feed at all")
                .contains("not a release"),
            "a short reply that is not a feed keeps its own sentence"
        );
    }

    #[test]
    fn the_beta_feed_asks_for_few_enough_releases_to_stay_inside_its_window() {
        assert!(
            super::BETA_RELEASES.ends_with("per_page=15"),
            "the whole list outgrew the window once and must not be asked for again"
        );
        assert!(
            super::release_window(UpdateChannel::Beta)
                > 8 * super::release_window(UpdateChannel::Stable),
            "the beta feed is the long one and needs room to grow"
        );
    }

    #[test]
    fn beta_release_selection_orders_versions_and_excludes_other_releases() {
        let body = r#"[
          {"tag_name":"beta-v9.8.0","draft":false,"prerelease":true,"assets":[
            {"name":"cobalt-9.8.0-KoboRoot.tgz","browser_download_url":"https://example.test/9.8.0.tgz"},
            {"name":"cobalt-host-manifest.txt","browser_download_url":"https://example.test/9.8.0.manifest"},
            {"name":"cobalt-host-manifest.txt.sig","browser_download_url":"https://example.test/9.8.0.sig"}]},
          {"tag_name":"beta-v9.9.0","draft":true,"prerelease":true,"assets":[
            {"name":"cobalt-9.9.0-KoboRoot.tgz","browser_download_url":"https://example.test/draft.tgz"},
            {"name":"cobalt-host-manifest.txt","browser_download_url":"https://example.test/draft.manifest"},
            {"name":"cobalt-host-manifest.txt.sig","browser_download_url":"https://example.test/draft.sig"}]},
          {"tag_name":"v10.0.0","draft":false,"prerelease":false,"assets":[]},
          {"tag_name":"beta-v9.7.0","draft":false,"prerelease":true,"assets":[
            {"name":"wrong.tgz","browser_download_url":"https://example.test/wrong.tgz"},
            {"name":"cobalt-host-manifest.txt","browser_download_url":"https://example.test/9.7.0.manifest"},
            {"name":"cobalt-host-manifest.txt.sig","browser_download_url":"https://example.test/9.7.0.sig"}]}
        ]"#;
        let release = super::latest_release(body, UpdateChannel::Beta).expect("highest valid beta");
        assert_eq!(release.version, "9.8.0");
        assert_eq!(release.archive, "https://example.test/9.8.0.tgz");
        assert!(super::latest_release("[]", UpdateChannel::Beta).is_err());
    }

    #[test]
    fn the_installed_release_and_an_unreadable_tag_are_both_not_newer() {
        let same =
            super::latest_release(&release_json(super::VERSION, true), UpdateChannel::Stable)
                .expect("release");
        assert!(!same.newer_than(super::VERSION));
        let strange = super::Release {
            version: "nightly".to_owned(),
            archive: String::new(),
            manifest: String::new(),
            signature: String::new(),
        };
        assert!(
            !strange.newer_than(super::VERSION),
            "a version that cannot be compared must not be offered as an upgrade"
        );
        let older = super::Release {
            version: "0.1.0".to_owned(),
            archive: String::new(),
            manifest: String::new(),
            signature: String::new(),
        };
        assert!(older.older_than(super::VERSION));
    }

    #[test]
    fn verified_metadata_binds_the_version_and_device_archive_digest() {
        let release = super::latest_release(&release_json("9.9.9", true), UpdateChannel::Stable)
            .expect("release");
        let manifest = kobo_app_store::ReleaseManifest {
            version: "9.9.9".to_owned(),
            channels: vec!["stable".to_owned(), "beta".to_owned()],
            source: "0123456789abcdef0123456789abcdef01234567".to_owned(),
            assets: vec![kobo_app_store::ReleaseAsset {
                kind: "device".to_owned(),
                platform: None,
                name: "cobalt-9.9.9-KoboRoot.tgz".to_owned(),
                bytes: 123,
                sha256: "a".repeat(64),
            }],
        };
        assert_eq!(
            super::device_digest_from_manifest(&release, &manifest),
            Some("a".repeat(64))
        );
        let mut wrong = manifest;
        wrong.version = "9.9.8".to_owned();
        assert_eq!(super::device_digest_from_manifest(&release, &wrong), None);
        assert!(
            super::verified_device_digest(&release, b"not a manifest", &"0".repeat(128)).is_none()
        );
    }

    #[test]
    fn every_stop_on_the_update_journey_fits_the_panel() {
        let flows = [
            super::UpdateFlow::Idle,
            super::UpdateFlow::Checking {
                channel: UpdateChannel::Stable,
            },
            super::UpdateFlow::Manifest {
                release: super::latest_release(&release_json("9.9.9", true), UpdateChannel::Stable)
                    .expect("release"),
            },
            super::UpdateFlow::ManifestSignature {
                release: super::latest_release(&release_json("9.9.9", true), UpdateChannel::Stable)
                    .expect("release"),
                manifest: b"manifest".to_vec(),
            },
            super::UpdateFlow::UpToDate {
                latest: "0.1.0".to_owned(),
            },
            super::UpdateFlow::Ahead {
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
            super::UpdateFlow::Failed(
                "GitHub's release list is longer than this reader can read.".to_owned(),
            ),
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
            update: super::UpdateFlow::Checking {
                channel: UpdateChannel::Stable,
            },
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

    #[test]
    fn an_available_update_has_a_clear_status_versions_and_primary_action() {
        let screen = Settings {
            view: super::View::Update,
            update: super::UpdateFlow::Ready {
                version: "9.9.9".to_owned(),
                url: "https://example.test/KoboRoot.tgz".to_owned(),
                sha256: "a".repeat(64),
            },
            ..Settings::default()
        }
        .update();

        assert!(screen.nodes.iter().any(|node| matches!(
            node,
            Node::Banner {
                level: BannerLevel::Info,
                text,
                ..
            } if text == "Cobalt 9.9.9 is available."
        )));
        assert!(screen.nodes.iter().any(|node| matches!(
            node,
            Node::Facts { entries, .. }
                if entries == &[("Installed".to_owned(), super::VERSION.to_owned()),
                    ("Available".to_owned(), "9.9.9".to_owned())]
        )));
        assert!(screen.nodes.iter().any(|node| matches!(
            node,
            Node::Button {
                action,
                label,
                emphasis: Emphasis::Primary,
                ..
            } if *action == action_id(super::INSTALL) && label == "Install update"
        )));
    }

    #[test]
    fn a_completed_update_has_a_checkmark_and_a_primary_way_to_close() {
        let screen = Settings {
            view: super::View::Update,
            update: super::UpdateFlow::Installed {
                version: "9.9.9".to_owned(),
            },
            ..Settings::default()
        }
        .update();

        assert!(screen.nodes.iter().any(|node| matches!(
            node,
            Node::Splash {
                glyph: Some(Glyph::Check),
                title,
                summary,
                ..
            } if title == "Update complete"
                && summary == "Close Cobalt, then reopen it from the menu."
        )));
        assert!(screen.nodes.iter().any(|node| matches!(
            node,
            Node::Button {
                action,
                label,
                emphasis: Emphasis::Primary,
                ..
            } if *action == action_id(super::CLOSE) && label == "Close Cobalt"
        )));
    }

    #[test]
    fn an_update_check_with_no_network_offers_to_join_wifi() {
        use super::{CHECK, JOIN_WIFI, UPDATE};
        use kobo_sdk::{AppRunner, Command, TaskError, TaskOutcome};
        let mut runner = AppRunner::new(Settings::default());
        runner.start();
        runner.app_mut().update_channel = Some(UpdateChannel::Stable);
        runner.action(action_id(UPDATE));
        let commands = runner.action(action_id(CHECK));
        let task = commands
            .iter()
            .find_map(|command| match command {
                Command::Spawn { task, .. } => Some(*task),
                _ => None,
            })
            .expect("checking asks GitHub");
        runner.task_outcome(task, TaskOutcome::Failed(TaskError::Offline));
        let screen = runner.app().update();
        let text = drawn_words(&screen);
        assert!(text.contains("This reader is not on Wi-Fi."), "{text}");
        assert!(!text.contains("this reader is not on a network"), "{text}");
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true));
        assert!(layout.rect_of_action(action_id(JOIN_WIFI)).is_some());
        assert!(layout.rect_of_action(action_id(CHECK)).is_some());
        // Joining a network is what the banner about the network offers.
        assert!(screen.nodes.iter().any(|node| matches!(
            node,
            Node::Banner { action: Some(action), .. } if action.action == action_id(JOIN_WIFI)
        )));
        runner.action(action_id(JOIN_WIFI));
        assert_eq!(runner.app().view, View::Wifi);
    }

    #[test]
    fn a_failed_update_check_says_what_to_do_in_plain_words() {
        use super::{check_failure_words, refusal_words};
        use kobo_sdk::TaskError;
        for error in [
            TaskError::Unreachable,
            TaskError::TimedOut,
            TaskError::RateLimited(60),
            TaskError::Denied,
            TaskError::NotFound,
        ] {
            let words = check_failure_words(error);
            assert!(words.starts_with(char::is_uppercase), "{words}");
            assert!(words.ends_with('.'), "{words}");
            assert!(!words.contains("task"), "{words}");
        }
        for reason in [
            DenyReason::NotDeclared,
            DenyReason::WithheldForBattery,
            DenyReason::Unsupported,
            DenyReason::PolicyRejected,
            DenyReason::Busy,
        ] {
            assert!(!refusal_words(reason).contains("runtime"));
        }
    }

    #[test]
    fn the_about_page_shows_the_wifi_address_when_the_runtime_gives_one() {
        let mut settings = Settings {
            identity: Some(DeviceIdentity {
                profile_id: "CLARA_BW_391".to_owned(),
                model: "Kobo Clara BW".to_owned(),
                device_code: 391,
                firmware: "4.41.23145".to_owned(),
                kernel: "5.10.117".to_owned(),
                runtime_version: "0.3.0".to_owned(),
                panel_width: 1072,
                panel_height: 1448,
            }),
            ..Settings::default()
        };
        let without = facts_of(&settings.about());
        assert!(!without.contains("Wi-Fi address"), "{without}");
        settings.address = Some("192.168.1.23".to_owned());
        let screen = settings.about();
        assert!(screen.validate(&CLARA_BW_METRICS).is_empty());
        let with = facts_of(&screen);
        assert!(with.contains("192.168.1.23"), "{with}");
        assert!(text_of(&screen).contains("--device"));
    }

    #[test]
    fn the_about_page_turns_rather_than_running_off_the_panel_and_keeps_every_fact() {
        for metrics in every_size_and_pose() {
            let mut runner = kobo_sdk::AppRunner::with_metrics(Settings::default(), metrics);
            runner.start();
            runner.app_mut().identity = Some(DeviceIdentity {
                profile_id: "CLARA_BW_391".to_owned(),
                model: "Kobo Clara BW".to_owned(),
                device_code: 391,
                firmware: "4.41.23145".to_owned(),
                kernel: "5.10.117".to_owned(),
                runtime_version: "0.3.0".to_owned(),
                panel_width: 1072,
                panel_height: 1448,
            });
            runner.app_mut().address = Some("192.168.1.23".to_owned());
            runner.action(action_id(super::ABOUT));
            let pages = runner.app().about_pages(&runner.context()).len();
            if metrics.text_scale.larger().is_some() && metrics.height > metrics.width {
                assert_eq!(pages, 1, "About is one page where it fits");
            }
            let mut seen = String::new();
            let mut rescans = 0;
            for page in 0..pages {
                let screen = runner.app().about_for(&runner.context());
                fits(&screen, metrics, &format!("About page {page}"));
                let layout = screen.layout_with(&metrics, &Chrome::measuring(true));
                rescans += usize::from(layout.rect_of_action(action_id(RESCAN)).is_some());
                seen.push_str(&facts_of(&screen));
                seen.push_str(&text_of(&screen));
                if page + 1 < pages {
                    assert!(layout.rect_of_action(action_id(MORE)).is_some());
                    runner.action(action_id(MORE));
                }
            }
            assert_eq!(rescans, 1, "{metrics:?}: Read again once, at the end");
            for fact in [
                "Kobo Clara BW",
                "0.3.0",
                "4.41.23145",
                "192.168.1.23",
                "CLARA_BW_391",
                "1072 × 1448 pixels",
                "391",
                "5.10.117",
                "--device",
            ] {
                assert!(seen.contains(fact), "{metrics:?}: {fact} is on no page");
            }
            runner.action(kobo_sdk::ActionId::BACK);
            runner.action(action_id(super::ABOUT));
            assert_eq!(runner.app().about_page, 0, "About opens on its first page");
        }
    }

    /// Every screen this file changes, at the default and the largest text
    /// size, held either way, measured through a runner so the faces are the
    /// ones the runtime measures with.
    fn every_size_and_pose() -> Vec<kobo_sdk::DisplayMetrics> {
        let mut largest = CLARA_BW_METRICS;
        while let Some(larger) = largest.text_scale.larger() {
            largest.text_scale = larger;
        }
        [CLARA_BW_METRICS, largest]
            .into_iter()
            .flat_map(|metrics| {
                [
                    metrics,
                    kobo_sdk::DisplayMetrics {
                        width: metrics.height,
                        height: metrics.width,
                        ..metrics
                    },
                ]
            })
            .collect()
    }

    fn fits(screen: &kobo_sdk::Screen, metrics: kobo_sdk::DisplayMetrics, what: &str) {
        let report = screen.diagnostics(&metrics, &Chrome::measuring(true));
        assert!(
            !report.has_errors(),
            "{what} on {metrics:?}: {:?}",
            report.issues
        );
    }

    #[test]
    fn the_automatic_update_switches_say_what_is_true_now_and_read_whole_at_every_size() {
        for (cobalt, apps) in [(true, true), (true, false), (false, true), (false, false)] {
            let (cobalt_now, apps_now) = super::auto_update_summaries(cobalt, apps);
            // Every step, smallest to largest. Settings reaches the scale
            // through the SDK's metrics, which name no list of steps.
            let mut smallest = CLARA_BW_METRICS.text_scale;
            while let Some(smaller) = smallest.smaller() {
                smallest = smaller;
            }
            let steps = std::iter::successors(Some(smallest), |scale| scale.larger());
            for text_scale in steps {
                for (width, height) in [(1072, 1448), (1448, 1072)] {
                    let metrics = kobo_sdk::DisplayMetrics {
                        text_scale,
                        width,
                        height,
                        ..CLARA_BW_METRICS
                    };
                    let runner = kobo_sdk::AppRunner::with_metrics(Settings::default(), metrics);
                    let settings = Settings {
                        view: View::UpdateOptions,
                        auto_update: Some((cobalt, apps)),
                        update_channel: Some(UpdateChannel::Stable),
                        ..Settings::default()
                    };
                    let screen = settings.update_options_for(&runner.context());
                    let summaries: Vec<(String, bool)> = screen
                        .nodes
                        .iter()
                        .filter_map(|node| match node {
                            Node::Toggle { summary, on, .. } => Some((summary.clone(), *on)),
                            _ => None,
                        })
                        .collect();
                    let said = |now: &str, on: bool| {
                        // Where the panel holds it, the line is there and says
                        // what is true now; where it does not, it goes whole.
                        if text_scale == CLARA_BW_METRICS.text_scale && width < height {
                            (now.to_owned(), on)
                        } else if summaries.iter().all(|(summary, _)| summary.is_empty()) {
                            (String::new(), on)
                        } else {
                            (now.to_owned(), on)
                        }
                    };
                    assert_eq!(
                        summaries,
                        [said(cobalt_now, cobalt), said(apps_now, apps)],
                        "{text_scale:?} {width}x{height}"
                    );
                    // They restate the switch, so they go before the
                    // sentence that warns about Beta.
                    if summaries.iter().any(|(summary, _)| !summary.is_empty()) {
                        assert!(
                            format!("{screen:?}").contains("Beta versions may have more problems"),
                            "{text_scale:?} {width}x{height}"
                        );
                    }
                    let layout = screen.layout_with(&metrics, &Chrome::measuring(true));
                    for (summary, _) in summaries.iter().filter(|(summary, _)| !summary.is_empty())
                    {
                        assert!(
                            layout
                                .nodes
                                .iter()
                                .any(|node| node.text_lines.iter().any(|line| line == summary)),
                            "{summary:?} is cut at {text_scale:?} {width}x{height}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn every_update_screen_and_the_update_options_fit_at_every_size() {
        let flows = [
            super::UpdateFlow::Idle,
            super::UpdateFlow::UpToDate {
                latest: "0.1.0".to_owned(),
            },
            super::UpdateFlow::Ahead {
                latest: "0.1.0".to_owned(),
            },
            super::UpdateFlow::Offline,
            super::UpdateFlow::Failed(
                "GitHub did not answer. Check that Wi-Fi reaches the internet, then try again."
                    .to_owned(),
            ),
        ];
        for metrics in every_size_and_pose() {
            let runner = kobo_sdk::AppRunner::with_metrics(Settings::default(), metrics);
            let context = runner.context();
            for flow in &flows {
                let settings = Settings {
                    view: View::Update,
                    update: flow.clone(),
                    auto_update: Some((true, false)),
                    update_channel: Some(UpdateChannel::Stable),
                    ..Settings::default()
                };
                fits(&settings.update(), metrics, &format!("{flow:?}"));
            }
            for (cobalt, apps) in [(true, true), (false, false)] {
                let mut settings = Settings {
                    view: View::UpdateOptions,
                    auto_update: Some((cobalt, apps)),
                    update_channel: Some(UpdateChannel::Beta),
                    ..Settings::default()
                };
                fits(&settings.update_options_for(&context), metrics, "options");
                let confirm = settings.update_channel_confirmation_for(&context);
                fits(&confirm, metrics, "confirm");
                let layout = confirm.layout_with(&metrics, &Chrome::measuring(true));
                for answer in [CONFIRM_CHANNEL, CANCEL_CHANNEL] {
                    assert!(
                        layout.rect_of_action(action_id(answer)).is_some(),
                        "{answer}"
                    );
                }
                settings.choice_trouble =
                    Some("Something else is using that right now. Try again in a moment.".into());
                let options = settings.update_options_for(&context);
                fits(&options, metrics, "options with trouble");
                let layout = options.layout_with(&metrics, &Chrome::measuring(true));
                for choice in [CHANNEL_STABLE, CHANNEL_BETA, AUTO_COBALT, AUTO_APPS] {
                    assert!(
                        layout.rect_of_action(action_id(choice)).is_some(),
                        "{choice}"
                    );
                }
            }
        }
    }

    #[test]
    fn choosing_the_other_channel_asks_first_and_choosing_the_current_one_does_nothing() {
        use kobo_sdk::{AppRunner, Command};
        let mut runner = AppRunner::new(Settings::default());
        runner.start();
        runner.app_mut().update_channel = Some(UpdateChannel::Stable);
        runner.app_mut().auto_update = Some((false, false));
        runner.action(action_id(super::UPDATE));
        runner.action(action_id(super::UPDATE_OPTIONS));
        let commands = runner.action(action_id(CHANNEL_STABLE));
        assert_eq!(runner.app().view, View::UpdateOptions);
        assert!(!commands
            .iter()
            .any(|command| matches!(command, Command::Device(_))));
        runner.action(action_id(CHANNEL_BETA));
        assert_eq!(runner.app().view, View::UpdateChannelConfirm);
        let commands = runner.action(action_id(CONFIRM_CHANNEL));
        assert!(commands.iter().any(|command| matches!(
            command,
            Command::Device(DeviceRequest::SetUpdateChannel {
                channel: UpdateChannel::Beta
            })
        )));
        assert_eq!(runner.app().view, View::UpdateOptions);
        runner.action(kobo_sdk::ActionId::BACK);
        assert_eq!(runner.app().view, View::Update);
    }

    #[test]
    fn a_choice_the_runtime_would_not_keep_is_said_over_the_choices() {
        let mut settings = Settings {
            view: View::UpdateOptions,
            auto_update: Some((false, false)),
            update_channel: Some(UpdateChannel::Stable),
            ..Settings::default()
        };
        let mut context = kobo_sdk::Context::default();
        kobo_sdk::KoboApp::on_device_result(
            &mut settings,
            &mut context,
            DeviceRequest::SetAutoUpdate {
                cobalt: true,
                apps: false,
            },
            DeviceResult::Denied(DenyReason::Busy),
        );
        assert!(settings.update_options().nodes.iter().any(|node| matches!(
            node,
            Node::Banner { level: BannerLevel::Error, text, .. } if text.contains("Try again")
        )));
        kobo_sdk::KoboApp::on_device_result(
            &mut settings,
            &mut context,
            DeviceRequest::ReadAutoUpdate,
            DeviceResult::AutoUpdate {
                cobalt: false,
                apps: false,
            },
        );
        assert!(!settings
            .update_options()
            .nodes
            .iter()
            .any(|node| matches!(node, Node::Banner { .. })));
    }

    #[test]
    fn an_automatic_update_switch_applies_on_the_tap() {
        use kobo_sdk::{AppRunner, Command};
        let mut runner = AppRunner::new(Settings::default());
        runner.start();
        runner.app_mut().update_channel = Some(UpdateChannel::Stable);
        runner.app_mut().auto_update = Some((true, false));
        runner.action(action_id(super::UPDATE));
        let commands = runner.action(action_id(AUTO_APPS));
        assert!(commands.iter().any(|command| matches!(
            command,
            Command::Device(DeviceRequest::SetAutoUpdate {
                cobalt: true,
                apps: true
            })
        )));
    }

    #[test]
    fn a_failed_check_offers_to_try_again_from_its_error_banner_and_nowhere_else() {
        let screen = Settings {
            view: View::Update,
            update: super::UpdateFlow::Failed("That took too long. Try again.".to_owned()),
            ..Settings::default()
        }
        .update();
        assert!(screen.nodes.iter().any(|node| matches!(
            node,
            Node::Banner { level: BannerLevel::Error, action: Some(action), .. }
                if action.action == action_id(super::CHECK) && action.label == "Try again"
        )));
        assert!(!screen.nodes.iter().any(|node| matches!(
            node,
            Node::Button { action, .. } if *action == action_id(super::CHECK)
        )));
    }

    #[test]
    fn each_radio_is_a_switch_showing_its_state_and_a_tap_flips_it() {
        use kobo_sdk::{AppRunner, Command};
        for (view, open) in [
            (View::Wifi, super::WIFI),
            (View::Bluetooth, super::BLUETOOTH),
        ] {
            let mut runner = AppRunner::new(Settings::default());
            runner.start();
            runner.action(action_id(open));
            runner.app_mut().wifi_state = RadioState::On;
            runner.app_mut().bluetooth_state = RadioState::Off;
            let screen = match view {
                View::Wifi => runner.app().wifi(),
                _ => runner.app().bluetooth(),
            };
            let on = screen.nodes.iter().find_map(|node| match node {
                Node::Toggle { action, on, .. } if *action == action_id(TOGGLE) => Some(*on),
                _ => None,
            });
            assert_eq!(on, Some(view == View::Wifi), "{view:?}");
            let commands = runner.action(action_id(TOGGLE));
            assert!(
                commands.iter().any(|command| matches!(
                    command,
                    Command::Device(
                        DeviceRequest::SetWifi { enabled: false }
                            | DeviceRequest::SetBluetooth { enabled: true }
                    )
                )),
                "{view:?}: {commands:?}"
            );
        }
    }

    #[test]
    fn the_wifi_switch_says_which_network_is_joined() {
        let settings = Settings {
            wifi_state: RadioState::On,
            connected_ssid: Some("Library".to_owned()),
            ..Settings::default()
        };
        assert!(settings.wifi().nodes.iter().any(|node| matches!(
            node,
            Node::Toggle { summary, on: true, .. } if summary == "Connected to Library"
        )));
    }

    /// The password goes to the runtime as a mask of its length, because the
    /// runtime draws a secret field as dots anyway and keeps every frame it is
    /// sent until the next one arrives.
    #[test]
    fn the_wifi_password_field_is_a_secret_that_never_carries_the_password() {
        let mut settings = Settings {
            view: View::WifiPassword,
            selected_ssid: Some("Library".to_owned()),
            ..Settings::default()
        };
        for character in "hunter22".chars() {
            let column = "qwertyuiop".find(character);
            if let Some(column) = column {
                settings
                    .password
                    .press(action_id(&format!("kb.r0c{column}")));
            }
        }
        let typed = settings.password.text().to_owned();
        assert!(!typed.is_empty());
        let screen = settings.wifi_password();
        let field = screen
            .nodes
            .iter()
            .find_map(|node| match node {
                Node::Field {
                    value,
                    label,
                    hint,
                    kind,
                    ..
                } => Some((value.clone(), label.clone(), hint.clone(), *kind)),
                _ => None,
            })
            .expect("a password field");
        assert_eq!(field.0.chars().count(), typed.chars().count());
        assert!(!format!("{:?}", screen.nodes).contains(&typed));
        assert_eq!(field.1.as_deref(), Some("Password for Library"));
        assert_eq!(field.2.as_deref(), Some("8 to 63 characters"));
        assert_eq!(field.3, kobo_sdk::FieldKind::Secret);
    }

    #[test]
    fn a_refused_wifi_password_is_said_under_the_field_at_every_size() {
        for metrics in every_size_and_pose() {
            let _runner = kobo_sdk::AppRunner::with_metrics(Settings::default(), metrics);
            let settings = Settings {
                view: View::WifiPassword,
                selected_ssid: Some("A secured wireless network with a long name".to_owned()),
                trouble: Some((
                    Topic::Wifi,
                    "A Wi-Fi password has 8 to 63 characters.".to_owned(),
                )),
                ..Settings::default()
            };
            let screen = settings.wifi_password();
            assert!(screen.nodes.iter().any(|node| matches!(
                node,
                Node::Field { error: Some(error), .. } if error.contains("8 to 63")
            )));
            assert!(!screen
                .nodes
                .iter()
                .any(|node| matches!(node, Node::Banner { .. } | Node::Heading { .. })));
            fits(&screen, metrics, "Wi-Fi password");
        }
    }

    #[test]
    fn a_failed_reading_offers_to_read_again_from_its_banner_at_every_size() {
        for metrics in every_size_and_pose() {
            let context = kobo_sdk::AppRunner::with_metrics(Settings::default(), metrics).context();
            let mut settings = Settings {
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
                identity: Some(DeviceIdentity {
                    profile_id: "CLARA_BW_391".to_owned(),
                    model: "Kobo Clara BW".to_owned(),
                    device_code: 391,
                    firmware: "4.41.23145".to_owned(),
                    kernel: "5.10.117".to_owned(),
                    runtime_version: "0.3.0".to_owned(),
                    panel_width: 1072,
                    panel_height: 1448,
                }),
                ..Settings::default()
            };
            for topic in [Topic::Battery, Topic::About] {
                settings.trouble = Some((
                    topic,
                    "Something else is using that right now. Try again in a moment.".to_owned(),
                ));
                let screens = if topic == Topic::Battery {
                    vec![settings.battery()]
                } else {
                    (0..settings.about_pages(&context).len())
                        .map(|page| {
                            settings.about_page = page;
                            settings.about_for(&context)
                        })
                        .collect()
                };
                for screen in screens {
                    let rescans = screen
                        .nodes
                        .iter()
                        .filter(|node| match node {
                            Node::Banner {
                                action: Some(action),
                                ..
                            } => action.action == action_id(RESCAN),
                            Node::Button { action, .. } => *action == action_id(RESCAN),
                            _ => false,
                        })
                        .count();
                    assert_eq!(rescans, 1, "{topic:?}: one way to read again");
                    fits(&screen, metrics, &format!("{topic:?}"));
                }
            }
        }
    }

    #[test]
    fn wifi_signal_is_described_in_words() {
        use super::signal_words;
        assert_eq!(signal_words(-50), "Strong signal");
        assert_eq!(signal_words(-65), "Good signal");
        assert_eq!(signal_words(-80), "Weak signal");
        let settings = Settings {
            wifi_state: RadioState::On,
            networks: vec![WifiNetwork {
                ssid: "Home".to_owned(),
                signal_dbm: -55,
                secured: true,
                connected: false,
            }],
            ..Settings::default()
        };
        let text = drawn_words(&settings.wifi());
        assert!(!text.contains("dBm"), "{text}");
        assert!(text.contains("Strong signal"), "{text}");
    }
}

#[cfg(test)]
mod list_tests;

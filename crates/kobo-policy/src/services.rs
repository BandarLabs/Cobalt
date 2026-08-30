//! The runtime side of the application hardware API.
//!
//! Applications describe intent; this module decides what actually happens.
//! Two rules hold everywhere:
//!
//! * A capability that this build cannot perform safely is refused as
//!   `Unsupported`. It is never silently reported as done.
//! * A capability that is allowed is still clamped by [`PowerPolicy`], so an
//!   application cannot hold Wi-Fi or block suspend for longer than the system
//!   is willing to pay for.

use crate::{Capability, Declared, Grant, Grants, PowerPolicy};
use kobo_protocol::{
    AudioPlaybackState, DenyReason, DeviceError, DeviceRequest, DeviceResult, DictionaryEntry,
};
use std::collections::BTreeSet;
use std::time::Duration;

/// Which hardware this build is actually allowed to operate.
///
/// A capability that is not in this set is refused as unsupported, even when
/// the application declared it and policy would allow it. That is what keeps a
/// build honest: an unimplemented backend can never be reported as done.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Backends {
    available: BTreeSet<Capability>,
}

impl Backends {
    /// Nothing is owned. This is the only configuration currently proven safe,
    /// so it is what both the simulator and the device build use.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// Declares exactly which capabilities this build can really perform.
    #[must_use]
    pub fn with(capabilities: impl IntoIterator<Item = Capability>) -> Self {
        Self {
            available: capabilities.into_iter().collect(),
        }
    }

    #[must_use]
    pub fn supports(&self, capability: Capability) -> bool {
        self.available.contains(&capability)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.available.is_empty()
    }
}

/// The observable hardware state the runtime answers reads from.
///
/// On a device the runtime keeps this current from its own hardware sources;
/// in the simulator it is a believable model. Applications cannot tell the
/// difference, which is the point: the same application code is exercised.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeviceState {
    pub battery_percent: u8,
    pub charging: bool,
    pub frontlight_percent: u8,
    /// Whether a magnet is at the hall sensor.
    pub magnet_present: bool,
}

impl Default for DeviceState {
    fn default() -> Self {
        Self {
            battery_percent: 72,
            charging: false,
            frontlight_percent: 20,
            magnet_present: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct DeviceServices {
    grants: Grants,
    backends: Backends,
    state: DeviceState,
    wifi_held_for: Option<Duration>,
    awake_held_for: Option<Duration>,
    wake_scheduled_in: Option<Duration>,
    bluetooth_enabled: bool,
    wifi_enabled: bool,
    connected_ssid: Option<String>,
    audio_state: AudioPlaybackState,
    audio_position_ms: u32,
    audio_duration_ms: u32,
    audio_volume: u8,
    dictionaries: kobo_dict::Index,
    auto_update: AutoUpdateChoices,
}

/// The two standing update switches, held together because they are asked
/// and answered together.
#[derive(Clone, Copy, Debug)]
struct AutoUpdateChoices {
    cobalt: bool,
    apps: bool,
}

impl DeviceServices {
    /// Services for host development, where nothing real is touched.
    ///
    /// Every capability is available so an application's power, network and
    /// front-light paths can be exercised end to end, and every grant is still
    /// clamped by the real policy, so what an application learns here is what
    /// it will get on a device.
    #[must_use]
    pub fn simulated() -> Self {
        Self::new(
            Declared::all(),
            PowerPolicy::DEFAULT,
            Backends::with(Capability::ALL),
        )
    }

    /// Services for a real device.
    ///
    /// Pass only the capabilities this build can genuinely perform. Anything
    /// else is refused as unsupported rather than silently ignored.
    #[must_use]
    pub fn new(declared: Declared, policy: PowerPolicy, backends: Backends) -> Self {
        Self {
            grants: Grants::new(declared, policy),
            backends,
            state: DeviceState::default(),
            wifi_held_for: None,
            awake_held_for: None,
            wake_scheduled_in: None,
            bluetooth_enabled: false,
            wifi_enabled: true,
            connected_ssid: None,
            audio_state: AudioPlaybackState::Idle,
            audio_position_ms: 0,
            audio_duration_ms: 0,
            audio_volume: 70,
            dictionaries: kobo_dict::Index::default(),
            // Both on, matching a real reader that never opened settings.
            auto_update: AutoUpdateChoices {
                cobalt: true,
                apps: true,
            },
        }
    }

    /// Loads owner-installed offline dictionaries from bounded UTF-8 TSV
    /// files. Missing or malformed files yield fewer dictionaries rather than
    /// making the reading service unavailable.
    pub fn load_dictionaries(&mut self, directory: &std::path::Path) -> usize {
        self.dictionaries = kobo_dict::Index::load_directory(directory);
        self.dictionaries.len()
    }

    #[cfg(test)]
    pub fn install_dictionary(&mut self, dictionary: kobo_dict::Dictionary) -> bool {
        self.dictionaries.install(dictionary)
    }

    /// Updates the battery state used for both reads and policy decisions.
    pub fn observe_battery(&mut self, percent: u8, charging: bool) {
        self.grants.observe_battery(percent, charging);
        self.state.battery_percent = percent.min(100);
        self.state.charging = charging;
    }

    /// Whether a capability would be allowed right now.
    ///
    /// For callers that must drive hardware before the policy answers, so that
    /// an undeclared application or a low battery still stops the write rather
    /// than merely changing what it is told about it.
    #[must_use]
    pub fn may(&self, capability: Capability) -> bool {
        self.refusal(capability).is_none()
    }

    /// Records what the front light is actually set to.
    ///
    /// The stock reader is still running and may move the light underneath us,
    /// so this is a reading rather than a memory of what was last asked for.
    pub fn observe_frontlight(&mut self, percent: u8) {
        self.state.frontlight_percent = percent.min(100);
    }

    /// Moves the simulated magnet, and says whether that was a change.
    ///
    /// There is no bezel to hold a magnet against in the simulator, so this is
    /// how the two states are reached. It reports whether anything moved
    /// because a restated state must not be delivered as an edge: an
    /// application counting changes would count one that nobody made.
    pub fn set_magnet(&mut self, present: bool) -> bool {
        let changed = self.state.magnet_present != present;
        self.state.magnet_present = present;
        changed
    }

    /// The state applications currently observe.
    #[must_use]
    pub const fn state(&self) -> DeviceState {
        self.state
    }

    /// Currently held Wi-Fi duration, if any.
    #[must_use]
    pub const fn wifi_hold(&self) -> Option<Duration> {
        self.wifi_held_for
    }

    /// Currently held wake duration, if any.
    #[must_use]
    pub const fn wake_hold(&self) -> Option<Duration> {
        self.awake_held_for
    }

    /// Currently scheduled wake delay, if any.
    #[must_use]
    pub const fn scheduled_wake(&self) -> Option<Duration> {
        self.wake_scheduled_in
    }

    /// Answers exactly one request.
    pub fn handle(&mut self, request: DeviceRequest) -> DeviceResult {
        match request {
            DeviceRequest::ReadBattery => self.read_battery(),
            DeviceRequest::ReadBatteryDetail => self.read_battery_detail(),
            DeviceRequest::ReadCover => self.read_cover(),
            DeviceRequest::HoldWifi { seconds } => self.hold_wifi(seconds),
            DeviceRequest::ReleaseWifi => {
                self.wifi_held_for = None;
                DeviceResult::Done
            }
            DeviceRequest::KeepAwake { seconds } => self.keep_awake(seconds),
            DeviceRequest::AllowSleep => {
                self.awake_held_for = None;
                DeviceResult::Done
            }
            DeviceRequest::ScheduleWake { seconds } => self.schedule_wake(seconds),
            DeviceRequest::CancelWake => {
                self.wake_scheduled_in = None;
                DeviceResult::Done
            }
            DeviceRequest::SetFrontlight { percent } => self.set_frontlight(percent),
            DeviceRequest::ReadFrontlight => self.read_frontlight(),
            DeviceRequest::ReadBluetooth | DeviceRequest::ScanBluetooth => self.bluetooth_state(),
            DeviceRequest::SetBluetooth { enabled } => {
                if let Some(reason) = self.refusal(Capability::BluetoothControl) {
                    DeviceResult::Denied(reason)
                } else {
                    self.bluetooth_enabled = enabled;
                    self.bluetooth_state()
                }
            }
            DeviceRequest::PairBluetooth { .. }
            | DeviceRequest::ConnectBluetooth { .. }
            | DeviceRequest::DisconnectBluetooth { .. }
            | DeviceRequest::ForgetBluetooth { .. } => {
                if let Some(reason) = self.refusal(Capability::BluetoothControl) {
                    DeviceResult::Denied(reason)
                } else if self.bluetooth_enabled {
                    DeviceResult::Done
                } else {
                    DeviceResult::Failed(DeviceError::Unreachable)
                }
            }
            DeviceRequest::ReadWifi | DeviceRequest::ScanWifi => self.wifi_state(),
            DeviceRequest::SetWifi { enabled } => {
                if let Some(reason) = self.refusal(Capability::WifiControl) {
                    DeviceResult::Denied(reason)
                } else {
                    self.wifi_enabled = enabled;
                    if !enabled {
                        self.connected_ssid = None;
                    }
                    self.wifi_state()
                }
            }
            DeviceRequest::JoinWifi { ssid, .. } => {
                if let Some(reason) = self.refusal(Capability::WifiControl) {
                    DeviceResult::Denied(reason)
                } else {
                    self.wifi_enabled = true;
                    self.connected_ssid = Some(ssid);
                    self.wifi_state()
                }
            }
            DeviceRequest::DisconnectWifi => {
                if let Some(reason) = self.refusal(Capability::WifiControl) {
                    DeviceResult::Denied(reason)
                } else {
                    self.connected_ssid = None;
                    self.wifi_state()
                }
            }
            request @ (DeviceRequest::ReadAudio
            | DeviceRequest::LoadAudio { .. }
            | DeviceRequest::PlayAudio
            | DeviceRequest::PauseAudio
            | DeviceRequest::SeekAudio { .. }
            | DeviceRequest::StopAudio
            | DeviceRequest::SetAudioVolume { .. }) => self.handle_audio(&request),
            // Only a real reader has an installation to replace, so nothing
            // is downloaded or written here. The simulator answers the way a
            // reader that finished installing would, which is what lets an
            // application's whole update flow be exercised at a desk: the
            // scenario refusals still apply first, and a permitted request
            // simply reports done.
            DeviceRequest::Update { .. } => self
                .refusal(Capability::Network)
                .map_or(DeviceResult::Done, DeviceResult::Denied),
            DeviceRequest::ListInstalledApps
            | DeviceRequest::ReadAppCatalog
            | DeviceRequest::RefreshAppCatalog => DeviceResult::Apps {
                entries: Vec::new(),
            },
            DeviceRequest::InstallApp { .. } | DeviceRequest::UninstallApp { .. } => {
                DeviceResult::Done
            }
            DeviceRequest::LookupWord { word, language } => {
                self.lookup_word(word, language.as_deref())
            }
            DeviceRequest::ReadAppLink
            | DeviceRequest::BeginAppLink
            | DeviceRequest::PollAppLink
            | DeviceRequest::DisconnectAppLink => DeviceResult::Denied(DenyReason::Unsupported),
            DeviceRequest::ReadAutoUpdate => self.auto_update_state(),
            DeviceRequest::SetAutoUpdate { cobalt, apps } => self.choose_auto_update(cobalt, apps),
        }
    }

    fn choose_auto_update(&mut self, cobalt: bool, apps: bool) -> DeviceResult {
        self.auto_update = AutoUpdateChoices { cobalt, apps };
        self.auto_update_state()
    }

    /// The simulator keeps the two switches in memory so a settings screen's
    /// whole flow can be exercised at a desk.
    const fn auto_update_state(&self) -> DeviceResult {
        DeviceResult::AutoUpdate {
            cobalt: self.auto_update.cobalt,
            apps: self.auto_update.apps,
        }
    }

    fn lookup_word(&self, word: String, language: Option<&str>) -> DeviceResult {
        let entries = self
            .dictionaries
            .lookup(&word, language)
            .into_iter()
            .map(|entry| DictionaryEntry {
                dictionary: entry.dictionary,
                language: entry.language,
                headword: entry.headword,
                definition: entry.definition,
            })
            .collect();
        DeviceResult::Dictionary { word, entries }
    }

    /// Returns the policy refusal for a request that a real hardware backend
    /// wants to execute, or `None` when the backend may proceed.
    #[must_use]
    pub fn refusal_for(&self, request: &DeviceRequest) -> Option<DenyReason> {
        request_capability(request).and_then(|capability| self.refusal(capability))
    }

    /// Returns the refusal that applies to a capability, or `None` when the
    /// request may proceed.
    ///
    /// The order matters and is deliberate: an application first learns that it
    /// forgot to declare something, then that the battery is too low, and only
    /// then that this build cannot do it at all.
    fn refusal(&self, capability: Capability) -> Option<DenyReason> {
        match self.grants.check(capability) {
            Grant::NotDeclared => return Some(DenyReason::NotDeclared),
            Grant::WithheldForBattery => return Some(DenyReason::WithheldForBattery),
            Grant::Allowed => {}
        }
        if self.backends.supports(capability) {
            None
        } else {
            Some(DenyReason::Unsupported)
        }
    }

    fn read_battery(&self) -> DeviceResult {
        self.refusal(Capability::BatteryRead).map_or(
            DeviceResult::Battery {
                percent: self.state.battery_percent,
                charging: self.state.charging,
            },
            DeviceResult::Denied,
        )
    }

    /// The simulator has no gauge, so it publishes the fields it can derive
    /// from the state it does hold and leaves the rest absent. That is the
    /// same shape a reader with a thinner driver produces, which is the case
    /// worth having a simulator for.
    fn read_battery_detail(&self) -> DeviceResult {
        self.refusal(Capability::BatteryRead).map_or_else(
            || {
                DeviceResult::BatteryDetail(kobo_protocol::BatteryDetail {
                    percent: Some(self.state.battery_percent),
                    status: Some(
                        if self.state.charging {
                            "Charging"
                        } else {
                            "Discharging"
                        }
                        .to_owned(),
                    ),
                    ..kobo_protocol::BatteryDetail::default()
                })
            },
            DeviceResult::Denied,
        )
    }

    /// The simulator has no bezel to hold a magnet against, so it reports a
    /// sensor that is present and sees nothing. An application then exercises
    /// the same path it will on hardware rather than a "no sensor" branch it
    /// would never otherwise reach.
    fn read_cover(&self) -> DeviceResult {
        self.refusal(Capability::CoverSensor).map_or(
            DeviceResult::Cover {
                available: true,
                magnet_present: self.state.magnet_present,
            },
            DeviceResult::Denied,
        )
    }

    fn read_frontlight(&self) -> DeviceResult {
        self.refusal(Capability::FrontlightControl).map_or(
            DeviceResult::Frontlight {
                percent: self.state.frontlight_percent,
            },
            DeviceResult::Denied,
        )
    }

    fn bluetooth_state(&self) -> DeviceResult {
        self.refusal(Capability::BluetoothControl).map_or_else(
            || DeviceResult::Bluetooth {
                available: true,
                enabled: self.bluetooth_enabled,
                devices: Vec::new(),
                // The simulated host hands the panel back by returning, so
                // nothing here ever reboots.
                restart_on_exit: false,
            },
            DeviceResult::Denied,
        )
    }

    fn wifi_state(&self) -> DeviceResult {
        self.refusal(Capability::WifiControl).map_or_else(
            || DeviceResult::Wifi {
                available: true,
                enabled: self.wifi_enabled,
                connected_ssid: self.connected_ssid.clone(),
                networks: Vec::new(),
            },
            DeviceResult::Denied,
        )
    }

    fn audio_state(&self) -> DeviceResult {
        self.refusal(Capability::BluetoothAudio).map_or(
            DeviceResult::Audio {
                available: true,
                state: self.audio_state,
                position_ms: self.audio_position_ms,
                duration_ms: self.audio_duration_ms,
                volume: self.audio_volume,
            },
            DeviceResult::Denied,
        )
    }

    fn handle_audio(&mut self, request: &DeviceRequest) -> DeviceResult {
        if let Some(reason) = self.refusal(Capability::BluetoothAudio) {
            return DeviceResult::Denied(reason);
        }
        match request {
            DeviceRequest::ReadAudio => {}
            DeviceRequest::LoadAudio { .. } => {
                self.audio_state = AudioPlaybackState::Ready;
                self.audio_position_ms = 0;
                self.audio_duration_ms = 30 * 60 * 1_000;
            }
            DeviceRequest::PlayAudio | DeviceRequest::SeekAudio { .. }
                if self.audio_state == AudioPlaybackState::Idle =>
            {
                return DeviceResult::Failed(DeviceError::NotFound);
            }
            DeviceRequest::PlayAudio => self.audio_state = AudioPlaybackState::Playing,
            DeviceRequest::PauseAudio => {
                if self.audio_state == AudioPlaybackState::Playing {
                    self.audio_state = AudioPlaybackState::Paused;
                }
            }
            DeviceRequest::SeekAudio { position_ms } => {
                self.audio_position_ms = (*position_ms).min(self.audio_duration_ms);
            }
            DeviceRequest::StopAudio => {
                self.audio_state = if self.audio_duration_ms == 0 {
                    AudioPlaybackState::Idle
                } else {
                    AudioPlaybackState::Ready
                };
                self.audio_position_ms = 0;
            }
            DeviceRequest::SetAudioVolume { percent } => self.audio_volume = (*percent).min(100),
            _ => return DeviceResult::Failed(DeviceError::InvalidInput),
        }
        self.audio_state()
    }

    fn set_frontlight(&mut self, percent: u8) -> DeviceResult {
        if let Some(reason) = self.refusal(Capability::FrontlightControl) {
            return DeviceResult::Denied(reason);
        }
        self.state.frontlight_percent = percent.min(100);
        DeviceResult::Frontlight {
            percent: self.state.frontlight_percent,
        }
    }

    fn hold_wifi(&mut self, seconds: u32) -> DeviceResult {
        if seconds == 0 {
            return DeviceResult::Denied(DenyReason::PolicyRejected);
        }
        if let Some(reason) = self.refusal(Capability::HoldWifi) {
            return DeviceResult::Denied(reason);
        }
        let granted = self
            .grants
            .policy()
            .clamp_wifi_hold(Duration::from_secs(u64::from(seconds)));
        self.wifi_held_for = Some(granted);
        DeviceResult::Granted {
            seconds: clamp_seconds(granted),
        }
    }

    fn keep_awake(&mut self, seconds: u32) -> DeviceResult {
        if seconds == 0 {
            return DeviceResult::Denied(DenyReason::PolicyRejected);
        }
        if let Some(reason) = self.refusal(Capability::KeepAwake) {
            return DeviceResult::Denied(reason);
        }
        let policy = self.grants.policy();
        let granted = Duration::from_secs(u64::from(seconds)).min(policy.maximum_foreground_awake);
        self.awake_held_for = Some(granted);
        DeviceResult::Granted {
            seconds: clamp_seconds(granted),
        }
    }

    fn schedule_wake(&mut self, seconds: u32) -> DeviceResult {
        if let Some(reason) = self.refusal(Capability::ScheduledWake) {
            return DeviceResult::Denied(reason);
        }
        let granted = self
            .grants
            .policy()
            .clamp_wake_interval(Duration::from_secs(u64::from(seconds)));
        self.wake_scheduled_in = Some(granted);
        DeviceResult::Granted {
            seconds: clamp_seconds(granted),
        }
    }
}

/// Returns the application capability required by a hardware or platform request.
///
/// Runtime-owned app-store transactions return `None`; they are authorized by
/// the built-in caller identity instead of a third-party manifest capability.
#[must_use]
pub fn request_capability(request: &DeviceRequest) -> Option<Capability> {
    Some(match request {
        DeviceRequest::ReadBattery | DeviceRequest::ReadBatteryDetail => Capability::BatteryRead,
        DeviceRequest::ReadCover => Capability::CoverSensor,
        DeviceRequest::HoldWifi { .. } | DeviceRequest::ReleaseWifi => Capability::HoldWifi,
        DeviceRequest::KeepAwake { .. } | DeviceRequest::AllowSleep => Capability::KeepAwake,
        DeviceRequest::ScheduleWake { .. } | DeviceRequest::CancelWake => Capability::ScheduledWake,
        DeviceRequest::SetFrontlight { .. } | DeviceRequest::ReadFrontlight => {
            Capability::FrontlightControl
        }
        DeviceRequest::ReadBluetooth
        | DeviceRequest::SetBluetooth { .. }
        | DeviceRequest::ScanBluetooth
        | DeviceRequest::PairBluetooth { .. }
        | DeviceRequest::ConnectBluetooth { .. }
        | DeviceRequest::DisconnectBluetooth { .. }
        | DeviceRequest::ForgetBluetooth { .. } => Capability::BluetoothControl,
        DeviceRequest::ReadWifi
        | DeviceRequest::SetWifi { .. }
        | DeviceRequest::ScanWifi
        | DeviceRequest::JoinWifi { .. }
        | DeviceRequest::DisconnectWifi => Capability::WifiControl,
        DeviceRequest::ReadAudio
        | DeviceRequest::LoadAudio { .. }
        | DeviceRequest::PlayAudio
        | DeviceRequest::PauseAudio
        | DeviceRequest::SeekAudio { .. }
        | DeviceRequest::StopAudio
        | DeviceRequest::SetAudioVolume { .. } => Capability::BluetoothAudio,
        // An update is fetched from the network and applied to the
        // installation the requester is already running from, so the network
        // permission is the one that governs it.
        DeviceRequest::Update { .. } => Capability::Network,
        DeviceRequest::ListInstalledApps
        | DeviceRequest::ReadAppCatalog
        | DeviceRequest::RefreshAppCatalog
        | DeviceRequest::InstallApp { .. }
        | DeviceRequest::UninstallApp { .. }
        | DeviceRequest::LookupWord { .. }
        | DeviceRequest::ReadAppLink
        | DeviceRequest::BeginAppLink
        | DeviceRequest::PollAppLink
        | DeviceRequest::DisconnectAppLink
        // Reading and choosing the standing preference touch a settings file,
        // not a radio or the panel, so no capability governs them; who may
        // ask is decided by the runtime, exactly as for the store requests.
        | DeviceRequest::ReadAutoUpdate
        | DeviceRequest::SetAutoUpdate { .. } => return None,
    })
}

fn clamp_seconds(duration: Duration) -> u32 {
    u32::try_from(duration.as_secs()).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::{Backends, DeviceServices, DeviceState};
    use crate::{Capability, Declared, PowerPolicy};
    use kobo_protocol::{DenyReason, DeviceRequest, DeviceResult};

    fn seconds_of(duration: std::time::Duration) -> u32 {
        u32::try_from(duration.as_secs()).expect("policy fits in u32")
    }

    #[test]
    fn dictionary_lookup_is_local_normalized_and_explicit_when_missing() {
        let mut services = DeviceServices::simulated();
        assert!(services.install_dictionary(kobo_dict::Dictionary::from_tsv(
            "Pocket",
            "# language=en\nstory\tAn account of events.\n",
        )));
        assert!(matches!(
            services.handle(DeviceRequest::LookupWord {
                word: "stories".into(),
                language: Some("en".into()),
            }),
            DeviceResult::Dictionary { entries, .. }
                if entries.len() == 1 && entries[0].headword == "story"
        ));
        assert_eq!(
            services.handle(DeviceRequest::LookupWord {
                word: "absent".into(),
                language: Some("en".into()),
            }),
            DeviceResult::Dictionary {
                word: "absent".into(),
                entries: Vec::new(),
            }
        );
    }

    fn declared(names: &[&str]) -> Declared {
        Declared::parse(names.iter().copied()).expect("valid declaration")
    }

    #[test]
    fn a_capability_that_was_not_declared_is_refused_first() {
        let mut services = DeviceServices::new(
            declared(&[]),
            PowerPolicy::DEFAULT,
            Backends::with([Capability::BatteryRead]),
        );
        assert_eq!(
            services.handle(DeviceRequest::ReadBattery),
            DeviceResult::Denied(DenyReason::NotDeclared)
        );
    }

    #[test]
    fn a_declared_capability_without_a_backend_is_refused_as_unsupported() {
        let mut services = DeviceServices::new(
            declared(&["network", "hold-wifi"]),
            PowerPolicy::DEFAULT,
            Backends::none(),
        );
        assert_eq!(
            services.handle(DeviceRequest::HoldWifi { seconds: 60 }),
            DeviceResult::Denied(DenyReason::Unsupported)
        );
        assert_eq!(services.wifi_hold(), None);
    }

    #[test]
    fn a_simulated_update_reports_done_so_the_flow_can_be_tested() {
        let mut services = DeviceServices::new(
            declared(&["network"]),
            PowerPolicy::DEFAULT,
            Backends::with([Capability::Network]),
        );
        assert_eq!(
            services.handle(DeviceRequest::Update {
                url: "https://example.com/cobalt.tgz".to_owned(),
                sha256: "a".repeat(64),
            }),
            DeviceResult::Done
        );
    }

    #[test]
    fn simulated_automatic_updates_start_on_and_remember_a_choice() {
        let mut services = DeviceServices::simulated();
        assert_eq!(
            services.handle(DeviceRequest::ReadAutoUpdate),
            DeviceResult::AutoUpdate {
                cobalt: true,
                apps: true,
            }
        );
        assert_eq!(
            services.handle(DeviceRequest::SetAutoUpdate {
                cobalt: false,
                apps: true,
            }),
            DeviceResult::AutoUpdate {
                cobalt: false,
                apps: true,
            }
        );
        assert_eq!(
            services.handle(DeviceRequest::ReadAutoUpdate),
            DeviceResult::AutoUpdate {
                cobalt: false,
                apps: true,
            }
        );
    }

    #[test]
    fn an_update_without_the_network_declaration_is_refused() {
        let mut services = DeviceServices::new(
            declared(&[]),
            PowerPolicy::DEFAULT,
            Backends::with([Capability::Network]),
        );
        assert_eq!(
            services.handle(DeviceRequest::Update {
                url: "https://example.com/cobalt.tgz".to_owned(),
                sha256: "a".repeat(64),
            }),
            DeviceResult::Denied(DenyReason::NotDeclared)
        );
    }

    #[test]
    fn a_wifi_hold_is_clamped_to_the_policy_maximum() {
        let mut services = DeviceServices::new(
            declared(&["network", "hold-wifi"]),
            PowerPolicy::DEFAULT,
            Backends::with([Capability::HoldWifi]),
        );
        let result = services.handle(DeviceRequest::HoldWifi {
            seconds: 24 * 60 * 60,
        });
        let maximum = u32::try_from(PowerPolicy::DEFAULT.maximum_wifi_hold.as_secs())
            .expect("policy fits in u32");
        assert_eq!(result, DeviceResult::Granted { seconds: maximum });
        assert_eq!(
            services.wifi_hold(),
            Some(PowerPolicy::DEFAULT.maximum_wifi_hold)
        );
    }

    #[test]
    fn a_wake_request_is_raised_to_the_policy_minimum() {
        let mut services = DeviceServices::new(
            declared(&["scheduled-wake"]),
            PowerPolicy::DEFAULT,
            Backends::with([Capability::ScheduledWake]),
        );
        let result = services.handle(DeviceRequest::ScheduleWake { seconds: 1 });
        let minimum = u32::try_from(PowerPolicy::DEFAULT.minimum_wake_interval.as_secs())
            .expect("policy fits in u32");
        assert_eq!(result, DeviceResult::Granted { seconds: minimum });
    }

    #[test]
    fn expensive_capabilities_are_withheld_on_a_low_battery() {
        let mut services = DeviceServices::new(
            declared(&["network", "hold-wifi", "battery-read"]),
            PowerPolicy::DEFAULT,
            Backends::with([Capability::HoldWifi, Capability::BatteryRead]),
        );
        services.observe_battery(5, false);
        assert_eq!(
            services.handle(DeviceRequest::HoldWifi { seconds: 60 }),
            DeviceResult::Denied(DenyReason::WithheldForBattery)
        );
        // Reading the battery is cheap and must keep working, or an application
        // could not discover why it was refused.
        assert_eq!(
            services.handle(DeviceRequest::ReadBattery),
            DeviceResult::Battery {
                percent: 5,
                charging: false,
            }
        );
        // Charging restores the grant.
        services.observe_battery(5, true);
        assert!(matches!(
            services.handle(DeviceRequest::HoldWifi { seconds: 60 }),
            DeviceResult::Granted { .. }
        ));
    }

    #[test]
    fn a_zero_length_hold_is_rejected_rather_than_granted_forever() {
        let mut services = DeviceServices::new(
            declared(&["network", "hold-wifi", "keep-awake"]),
            PowerPolicy::DEFAULT,
            Backends::with([Capability::HoldWifi, Capability::KeepAwake]),
        );
        assert_eq!(
            services.handle(DeviceRequest::HoldWifi { seconds: 0 }),
            DeviceResult::Denied(DenyReason::PolicyRejected)
        );
        assert_eq!(
            services.handle(DeviceRequest::KeepAwake { seconds: 0 }),
            DeviceResult::Denied(DenyReason::PolicyRejected)
        );
    }

    #[test]
    fn releasing_a_hold_always_succeeds_and_clears_it() {
        let mut services = DeviceServices::new(
            declared(&["network", "hold-wifi", "keep-awake", "scheduled-wake"]),
            PowerPolicy::DEFAULT,
            Backends::with([
                Capability::HoldWifi,
                Capability::KeepAwake,
                Capability::ScheduledWake,
            ]),
        );
        services.handle(DeviceRequest::HoldWifi { seconds: 60 });
        services.handle(DeviceRequest::KeepAwake { seconds: 60 });
        services.handle(DeviceRequest::ScheduleWake { seconds: 3600 });
        assert!(services.wifi_hold().is_some());
        assert_eq!(
            services.handle(DeviceRequest::ReleaseWifi),
            DeviceResult::Done
        );
        assert_eq!(
            services.handle(DeviceRequest::AllowSleep),
            DeviceResult::Done
        );
        assert_eq!(
            services.handle(DeviceRequest::CancelWake),
            DeviceResult::Done
        );
        assert_eq!(services.wifi_hold(), None);
        assert_eq!(services.wake_hold(), None);
        assert_eq!(services.scheduled_wake(), None);
    }

    #[test]
    fn the_simulator_exercises_every_path_an_application_will_take() {
        let mut services = DeviceServices::simulated();
        let default = DeviceState::default();
        assert_eq!(
            services.handle(DeviceRequest::ReadBattery),
            DeviceResult::Battery {
                percent: default.battery_percent,
                charging: default.charging,
            }
        );
        assert_eq!(
            services.handle(DeviceRequest::SetFrontlight { percent: 80 }),
            DeviceResult::Frontlight { percent: 80 }
        );
        assert_eq!(
            services.handle(DeviceRequest::ReadFrontlight),
            DeviceResult::Frontlight { percent: 80 }
        );
        // Holds are granted, but with exactly the clamping a device applies, so
        // an application cannot be surprised later.
        assert_eq!(
            services.handle(DeviceRequest::HoldWifi {
                seconds: 24 * 60 * 60
            }),
            DeviceResult::Granted {
                seconds: seconds_of(PowerPolicy::DEFAULT.maximum_wifi_hold),
            }
        );
        assert_eq!(
            services.handle(DeviceRequest::ScheduleWake { seconds: 1 }),
            DeviceResult::Granted {
                seconds: seconds_of(PowerPolicy::DEFAULT.minimum_wake_interval),
            }
        );
    }

    #[test]
    fn a_device_build_that_owns_no_hardware_refuses_every_change() {
        let mut services =
            DeviceServices::new(Declared::all(), PowerPolicy::DEFAULT, Backends::none());
        for request in [
            DeviceRequest::ReadBattery,
            DeviceRequest::HoldWifi { seconds: 60 },
            DeviceRequest::KeepAwake { seconds: 60 },
            DeviceRequest::ScheduleWake { seconds: 3600 },
            DeviceRequest::SetFrontlight { percent: 50 },
            DeviceRequest::ReadFrontlight,
        ] {
            assert_eq!(
                services.handle(request.clone()),
                DeviceResult::Denied(DenyReason::Unsupported),
                "{request:?} must be refused without a backend"
            );
        }
    }

    #[test]
    fn no_capability_is_supported_by_the_empty_backend_set() {
        for capability in Capability::ALL {
            assert!(
                !Backends::none().supports(capability),
                "{capability} must not be supported without a backend"
            );
        }
    }

    #[test]
    fn a_simulated_low_battery_still_withholds_expensive_capabilities() {
        let mut services = DeviceServices::simulated();
        services.observe_battery(3, false);
        assert_eq!(
            services.handle(DeviceRequest::HoldWifi { seconds: 60 }),
            DeviceResult::Denied(DenyReason::WithheldForBattery)
        );
        assert_eq!(
            services.handle(DeviceRequest::ReadBattery),
            DeviceResult::Battery {
                percent: 3,
                charging: false,
            }
        );
    }
}

#![forbid(unsafe_code)]

//! What an application is allowed to ask the runtime to do.
//!
//! Applications never talk to hardware. They declare capabilities in their
//! manifest, the runtime grants a subset, and every grant is additionally
//! clamped by a system policy that the application cannot influence. An
//! application asking for something unreasonable gets a reduced grant rather
//! than the device it asked for.

pub mod services;
pub mod store;
pub mod tasks;

pub use services::{Backends, DeviceServices, DeviceState};
pub use tasks::{Finished, RejectReason, TaskRunner, MAX_TASKS_IN_FLIGHT};

use std::collections::BTreeSet;
use std::fmt;
use std::time::Duration;

/// A single thing an application may be permitted to do.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Capability {
    /// Reach the network while the application is in the foreground.
    Network,
    /// Reach the network from a scheduled background wake.
    BackgroundNetwork,
    /// Keep Wi-Fi associated instead of letting the system drop it.
    ///
    /// This is what an always-on dashboard needs. It is heavily clamped,
    /// because holding Wi-Fi is the single largest battery cost on this
    /// hardware.
    HoldWifi,
    /// Prevent the device suspending while the application is in the
    /// foreground.
    KeepAwake,
    /// Be woken on a schedule to refresh content.
    ScheduledWake,
    /// Read the battery percentage and charging state.
    BatteryRead,
    /// Change the front light brightness.
    FrontlightControl,
    /// Use the Bluetooth audio profile, for example for spoken content.
    BluetoothAudio,
    /// Play audio through the active output.
    Audio,
    /// Draw the sleep screen.
    SleepScreen,
    /// Post notifications the reader shows.
    Notifications,
    /// Read and write files in a shared, user-visible folder.
    SharedFiles,
    /// Run a program on a terminal the runtime owns.
    ///
    /// This is the most dangerous capability in the system and is deliberately
    /// last. Everything else this platform does is undone by a reboot: a
    /// framebuffer write is volatile, an input grab dies with its descriptor,
    /// a setting has a pristine backup beside it. A shell is the first thing
    /// that can write the root filesystem, so it is the first thing that can
    /// produce a device no reboot repairs.
    ///
    /// It is therefore never implied by anything, never granted by default,
    /// and refused outright by any build without a terminal backend.
    Shell,
}

impl Capability {
    /// The exact manifest spelling of this capability.
    #[must_use]
    pub fn manifest_name(self) -> &'static str {
        match self {
            Self::Network => "network",
            Self::BackgroundNetwork => "background-network",
            Self::HoldWifi => "hold-wifi",
            Self::KeepAwake => "keep-awake",
            Self::ScheduledWake => "scheduled-wake",
            Self::BatteryRead => "battery-read",
            Self::FrontlightControl => "frontlight-control",
            Self::BluetoothAudio => "bluetooth-audio",
            Self::Audio => "audio",
            Self::SleepScreen => "sleep-screen",
            Self::Notifications => "notifications",
            Self::SharedFiles => "shared-files",
            Self::Shell => "shell",
        }
    }

    /// Parses the exact manifest spelling. Unknown names are rejected rather
    /// than ignored, so a typo can never silently drop a permission.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|capability| capability.manifest_name() == name)
    }

    /// Every capability, in declaration order.
    pub const ALL: [Self; 13] = [
        Self::Network,
        Self::BackgroundNetwork,
        Self::HoldWifi,
        Self::KeepAwake,
        Self::ScheduledWake,
        Self::BatteryRead,
        Self::FrontlightControl,
        Self::BluetoothAudio,
        Self::Audio,
        Self::SleepScreen,
        Self::Notifications,
        Self::SharedFiles,
        Self::Shell,
    ];

    /// Capabilities that imply another capability must also be held.
    #[must_use]
    pub fn requires(self) -> Option<Self> {
        match self {
            Self::BackgroundNetwork => Some(Self::ScheduledWake),
            Self::HoldWifi => Some(Self::Network),
            Self::BluetoothAudio => Some(Self::Audio),
            _ => None,
        }
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.manifest_name())
    }
}

/// Hard system limits. An application cannot raise these.
///
/// The values are deliberately conservative for a single-core device with a
/// small battery whose primary job is still to be an e-reader.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PowerPolicy {
    pub minimum_wake_interval: Duration,
    pub maximum_foreground_awake: Duration,
    pub maximum_wifi_hold: Duration,
    pub low_battery_percent: u8,
}

impl PowerPolicy {
    /// The default policy applied to every application.
    pub const DEFAULT: Self = Self {
        minimum_wake_interval: Duration::from_secs(15 * 60),
        maximum_foreground_awake: Duration::from_secs(60 * 60),
        maximum_wifi_hold: Duration::from_secs(10 * 60),
        low_battery_percent: 15,
    };

    /// Returns the wake interval actually granted for a request.
    #[must_use]
    pub fn clamp_wake_interval(&self, requested: Duration) -> Duration {
        requested.max(self.minimum_wake_interval)
    }

    /// Returns how long Wi-Fi may actually be held for a request.
    #[must_use]
    pub fn clamp_wifi_hold(&self, requested: Duration) -> Duration {
        requested.min(self.maximum_wifi_hold)
    }

    /// Returns whether a capability is usable at the current battery level.
    ///
    /// Below the low battery threshold the expensive capabilities are withheld
    /// so that an application cannot flatten a device that the owner still
    /// wants to read on.
    #[must_use]
    pub fn allowed_at_battery(&self, capability: Capability, percent: u8, charging: bool) -> bool {
        if charging || percent >= self.low_battery_percent {
            return true;
        }
        !matches!(
            capability,
            Capability::BackgroundNetwork
                | Capability::HoldWifi
                | Capability::KeepAwake
                | Capability::ScheduledWake
                | Capability::BluetoothAudio
                | Capability::Audio
        )
    }
}

impl Default for PowerPolicy {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// The capabilities an application declared, after validation.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Declared {
    capabilities: BTreeSet<Capability>,
}

/// Why a declaration was rejected.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeclarationError {
    Unknown(String),
    Missing {
        requested: Capability,
        required: Capability,
    },
}

impl fmt::Display for DeclarationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown(name) => write!(formatter, "unknown capability '{name}'"),
            Self::Missing {
                requested,
                required,
            } => write!(
                formatter,
                "capability '{requested}' also requires '{required}'"
            ),
        }
    }
}

impl std::error::Error for DeclarationError {}

impl Declared {
    /// Validates a manifest capability list.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown capability name or an unmet dependency.
    pub fn parse<'a>(names: impl IntoIterator<Item = &'a str>) -> Result<Self, DeclarationError> {
        let mut capabilities = BTreeSet::new();
        for name in names {
            let capability = Capability::parse(name)
                .ok_or_else(|| DeclarationError::Unknown(name.to_owned()))?;
            capabilities.insert(capability);
        }
        for capability in &capabilities {
            if let Some(required) = capability.requires() {
                if !capabilities.contains(&required) {
                    return Err(DeclarationError::Missing {
                        requested: *capability,
                        required,
                    });
                }
            }
        }
        Ok(Self { capabilities })
    }

    /// Every capability, for a simulator or a trusted built-in application.
    #[must_use]
    pub fn all() -> Self {
        Self {
            capabilities: Capability::ALL.into_iter().collect(),
        }
    }

    #[must_use]
    pub fn holds(&self, capability: Capability) -> bool {
        self.capabilities.contains(&capability)
    }

    pub fn iter(&self) -> impl Iterator<Item = Capability> + '_ {
        self.capabilities.iter().copied()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.capabilities.is_empty()
    }
}

/// A capability decision made at the moment an application asks to use it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Grant {
    Allowed,
    NotDeclared,
    WithheldForBattery,
}

/// The runtime's view of what an application may currently do.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Grants {
    declared: Declared,
    policy: PowerPolicy,
    battery_percent: u8,
    charging: bool,
}

impl Grants {
    #[must_use]
    pub fn new(declared: Declared, policy: PowerPolicy) -> Self {
        Self {
            declared,
            policy,
            battery_percent: 100,
            charging: false,
        }
    }

    pub fn observe_battery(&mut self, percent: u8, charging: bool) {
        self.battery_percent = percent.min(100);
        self.charging = charging;
    }

    /// Returns whether the application may use a capability right now.
    #[must_use]
    pub fn check(&self, capability: Capability) -> Grant {
        if !self.declared.holds(capability) {
            return Grant::NotDeclared;
        }
        if self
            .policy
            .allowed_at_battery(capability, self.battery_percent, self.charging)
        {
            Grant::Allowed
        } else {
            Grant::WithheldForBattery
        }
    }

    #[must_use]
    pub fn policy(&self) -> PowerPolicy {
        self.policy
    }
}

#[cfg(test)]
mod tests {
    use super::{Capability, DeclarationError, Declared, Grant, Grants, PowerPolicy};
    use std::time::Duration;

    #[test]
    fn every_capability_round_trips_through_its_manifest_name() {
        for capability in Capability::ALL {
            assert_eq!(
                Capability::parse(capability.manifest_name()),
                Some(capability)
            );
        }
        assert_eq!(Capability::parse("root"), None);
        assert_eq!(Capability::parse("Network"), None);
        assert_eq!(Capability::parse(""), None);
    }

    #[test]
    fn unknown_capabilities_are_rejected_rather_than_ignored() {
        assert_eq!(
            Declared::parse(["network", "sudo"]),
            Err(DeclarationError::Unknown("sudo".to_owned()))
        );
    }

    #[test]
    fn dependent_capabilities_require_their_base() {
        assert_eq!(
            Declared::parse(["hold-wifi"]),
            Err(DeclarationError::Missing {
                requested: Capability::HoldWifi,
                required: Capability::Network,
            })
        );
        assert!(Declared::parse(["network", "hold-wifi"]).is_ok());
        assert_eq!(
            Declared::parse(["background-network", "scheduled-wake"]),
            Declared::parse(["scheduled-wake", "background-network"])
        );
        assert!(Declared::parse(["background-network"]).is_err());
        assert!(Declared::parse(["bluetooth-audio"]).is_err());
        assert!(Declared::parse(["audio", "bluetooth-audio"]).is_ok());
    }

    #[test]
    fn an_application_cannot_raise_the_system_power_limits() {
        let policy = PowerPolicy::DEFAULT;
        assert_eq!(
            policy.clamp_wake_interval(Duration::from_secs(5)),
            policy.minimum_wake_interval
        );
        assert_eq!(
            policy.clamp_wake_interval(Duration::from_secs(24 * 60 * 60)),
            Duration::from_secs(24 * 60 * 60)
        );
        assert_eq!(
            policy.clamp_wifi_hold(Duration::from_secs(24 * 60 * 60)),
            policy.maximum_wifi_hold
        );
        assert_eq!(
            policy.clamp_wifi_hold(Duration::from_secs(30)),
            Duration::from_secs(30)
        );
    }

    #[test]
    fn expensive_capabilities_are_withheld_on_a_low_battery() {
        let declared =
            Declared::parse(["network", "hold-wifi", "battery-read"]).expect("valid manifest");
        let mut grants = Grants::new(declared, PowerPolicy::DEFAULT);

        grants.observe_battery(80, false);
        assert_eq!(grants.check(Capability::HoldWifi), Grant::Allowed);

        grants.observe_battery(5, false);
        assert_eq!(
            grants.check(Capability::HoldWifi),
            Grant::WithheldForBattery
        );
        assert_eq!(grants.check(Capability::BatteryRead), Grant::Allowed);

        // Charging lifts the low battery restriction.
        grants.observe_battery(5, true);
        assert_eq!(grants.check(Capability::HoldWifi), Grant::Allowed);
    }

    #[test]
    fn undeclared_capabilities_are_never_granted() {
        let declared = Declared::parse(["network"]).expect("valid manifest");
        let grants = Grants::new(declared, PowerPolicy::DEFAULT);
        assert_eq!(grants.check(Capability::Network), Grant::Allowed);
        assert_eq!(grants.check(Capability::SleepScreen), Grant::NotDeclared);
        assert_eq!(grants.check(Capability::HoldWifi), Grant::NotDeclared);
    }
}

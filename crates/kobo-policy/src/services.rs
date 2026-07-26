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
use kobo_protocol::{DenyReason, DeviceRequest, DeviceResult};
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
}

impl Default for DeviceState {
    fn default() -> Self {
        Self {
            battery_percent: 72,
            charging: false,
            frontlight_percent: 20,
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
        }
    }

    /// Updates the battery state used for both reads and policy decisions.
    pub fn observe_battery(&mut self, percent: u8, charging: bool) {
        self.grants.observe_battery(percent, charging);
        self.state.battery_percent = percent.min(100);
        self.state.charging = charging;
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
        }
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

    fn read_frontlight(&self) -> DeviceResult {
        self.refusal(Capability::FrontlightControl).map_or(
            DeviceResult::Frontlight {
                percent: self.state.frontlight_percent,
            },
            DeviceResult::Denied,
        )
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
                services.handle(request),
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

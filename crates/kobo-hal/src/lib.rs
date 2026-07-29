//! Safe Kobo hardware abstractions.

/// Bounded MP3 playback through Kobo's firmware-owned A2DP audio HAL.
#[cfg(feature = "device-write")]
pub mod audio;
/// Read-only battery observation. Not gated: it reads two text files and
/// changes nothing.
pub mod battery;
/// Bluetooth control through the firmware-owned BlueZ service.
#[cfg(feature = "device-write")]
pub mod bluetooth;
#[cfg(feature = "device-write")]
pub mod display;
/// Exclusive touch ownership. Available only with `device-write`, because a
/// grab takes the panel away from the stock reader. Front light brightness.
/// Behind `device-write`, because it changes something the owner can see,
/// though only a register on the light driver, which a reboot restores.
#[cfg(feature = "device-write")]
pub mod frontlight;
#[cfg(feature = "device-write")]
pub mod input;
/// Putting the network back after a handoff. Available only with
/// `device-write`, because it starts processes this program did not create.
#[cfg(feature = "device-write")]
pub mod network;
pub mod observe;
pub mod probe;
/// Stopping and restarting the stock reader. Available only with
/// `device-write`, because it acts on a process this program did not create.
#[cfg(feature = "device-write")]
pub mod reader;
pub mod refresh;
/// Wi-Fi control through the firmware-owned supplicant.
#[cfg(feature = "device-write")]
pub mod wifi;
/// Noticing that this process has been asked to stop, so that everything it
/// took from the device is given back before it goes.
pub use kobo_abi::stop;
pub mod soc_watchdog;
pub mod supervisor;
pub mod surface;
pub mod touch;

pub use battery::Battery;
#[cfg(feature = "device-write")]
pub use display::{DisplayError, DisplaySession, OWNER_UNLOCK_PHRASE};
pub use observe::{observe_touch, ObserveError, TouchObservation};
pub use probe::{probe_device, ProbeError};
pub use refresh::{Rect, RefreshIntent, RefreshPlan, UpdateMarker};
pub use surface::{read_region, RegionPlacement, RegionSnapshot, SurfaceError, SurfaceGeometry};
pub use touch::{InputEvent32, TouchDecoder, TouchEvent};

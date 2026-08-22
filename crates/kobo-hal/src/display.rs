//! The single verified entry point for changing the display.
//!
//! Nothing else in this project may submit a hardware update. Opening a
//! [`DisplaySession`] proves, at runtime, that:
//!
//! 1. the probed hardware geometry matches the profile exactly,
//! 2. the device code, serial model prefix, firmware version, and kernel
//!    release match the profile exactly, and
//! 3. the caller supplied the exact owner-attended unlock phrase.
//!
//! The module is compiled only with the non-default `device-write` feature, so
//! a default build contains no callable display-write code at all.

use crate::probe::{probe_device, ProbeError};
use crate::refresh::{Rect, RefreshPlan};
use crate::surface::{self, RegionSnapshot, SurfaceError, SurfaceGeometry};
use kobo_abi::{hwtcon, mxc_epdc};
use kobo_profile::{DeviceProfile, DeviceSnapshot};
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::path::Path;

/// The exact phrase an owner must supply to open a write session.
pub const OWNER_UNLOCK_PHRASE: &str = "OWNER_ATTENDED_DISPLAY_WRITE";

#[derive(Debug)]
pub enum DisplayError {
    UnlockMissing,
    ProfileRejected(Vec<String>),
    IdentityRejected(Vec<String>),
    Probe(ProbeError),
    Surface(SurfaceError),
    Io(io::Error),
}

impl fmt::Display for DisplayError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnlockMissing => {
                formatter.write_str("owner-attended display unlock is missing or incorrect")
            }
            Self::ProfileRejected(reasons) => {
                write!(
                    formatter,
                    "hardware profile rejected: {}",
                    reasons.join("; ")
                )
            }
            Self::IdentityRejected(reasons) => {
                write!(
                    formatter,
                    "device identity rejected: {}",
                    reasons.join("; ")
                )
            }
            Self::Probe(error) => write!(formatter, "read-only probe: {error}"),
            Self::Surface(error) => write!(formatter, "{error}"),
            Self::Io(error) => write!(formatter, "display io: {error}"),
        }
    }
}

impl std::error::Error for DisplayError {}

impl From<SurfaceError> for DisplayError {
    fn from(error: SurfaceError) -> Self {
        Self::Surface(error)
    }
}

impl From<io::Error> for DisplayError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// An open, fully verified display write session.
pub struct DisplaySession {
    framebuffer: File,
    geometry: SurfaceGeometry,
    profile: &'static DeviceProfile,
    snapshot: DeviceSnapshot,
}

impl DisplaySession {
    /// Probes the device and opens the framebuffer read-write.
    ///
    /// # Errors
    ///
    /// Returns an error when the unlock phrase is wrong, the probe fails, the
    /// hardware profile does not match exactly, the device identity does not
    /// match exactly, or the framebuffer cannot be opened.
    pub fn open(unlock: Option<&str>) -> Result<Self, DisplayError> {
        if unlock != Some(OWNER_UNLOCK_PHRASE) {
            return Err(DisplayError::UnlockMissing);
        }
        let snapshot = probe_device().map_err(DisplayError::Probe)?;
        let profile = kobo_profile::identify_profile(&snapshot).ok_or_else(|| {
            DisplayError::ProfileRejected(vec![
                "no supported hardware profile matched this device".to_owned()
            ])
        })?;
        Self::open_verified(profile, snapshot, Path::new("/dev/fb0"))
    }

    fn open_verified(
        profile: &'static DeviceProfile,
        snapshot: DeviceSnapshot,
        framebuffer_path: &Path,
    ) -> Result<Self, DisplayError> {
        let report = profile.validate(&snapshot);
        if !report.mismatches.is_empty() {
            return Err(DisplayError::ProfileRejected(report.mismatches));
        }
        let identity = profile.write_identity_blockers(&snapshot);
        if !identity.is_empty() {
            return Err(DisplayError::IdentityRejected(identity));
        }
        let framebuffer = snapshot
            .framebuffer
            .as_ref()
            .ok_or_else(|| DisplayError::ProfileRejected(vec!["framebuffer missing".to_owned()]))?;
        let geometry = SurfaceGeometry {
            width: framebuffer.width,
            height: framebuffer.height,
            stride: framebuffer.stride,
            bits_per_pixel: framebuffer.bits_per_pixel,
            memory_length: u64::from(framebuffer.memory_length),
        };
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(framebuffer_path)?;
        Ok(Self {
            framebuffer: file,
            geometry,
            profile,
            snapshot,
        })
    }

    #[must_use]
    pub fn profile(&self) -> &'static DeviceProfile {
        self.profile
    }

    #[must_use]
    pub fn snapshot(&self) -> &DeviceSnapshot {
        &self.snapshot
    }

    #[must_use]
    pub fn geometry(&self) -> SurfaceGeometry {
        self.geometry
    }

    /// Captures the exact current bytes of `region` so they can be restored.
    ///
    /// # Errors
    ///
    /// Returns an error when the region is invalid or the read fails.
    pub fn capture(&self, region: Rect) -> Result<RegionSnapshot, DisplayError> {
        Ok(surface::read_region(
            &self.framebuffer,
            self.geometry,
            region,
        )?)
    }

    /// Writes a previously captured region back to the exact place it came
    /// from. The snapshot carries its own validated placement, so no other
    /// region can be addressed.
    ///
    /// # Errors
    ///
    /// Returns an error when the write fails.
    pub fn restore(&self, snapshot: &RegionSnapshot) -> Result<(), DisplayError> {
        surface::write_region(&self.framebuffer, self.geometry, snapshot)?;
        Ok(())
    }

    /// Submits one hardware update for `plan` and waits for it to complete.
    ///
    /// A fresh high-entropy marker is generated for every update. Markers are a
    /// global namespace shared with the stock reader, so a low fixed marker
    /// could be matched against another process's update.
    ///
    /// # Errors
    ///
    /// Returns an error when the region is invalid or either ioctl fails.
    pub fn refresh(&self, plan: RefreshPlan) -> Result<(), DisplayError> {
        // Validate the region against this exact surface before the kernel sees it.
        surface::RegionPlacement::new(self.geometry, plan.region)?;
        let marker = unique_marker()?;
        if self.profile.framebuffer_id == "mxc_epdc_fb" {
            let mut update = plan.epdc_update_data(marker);
            mxc_epdc::send_update(&self.framebuffer, &mut update)?;
            let mut wait = mxc_epdc::MxcfbUpdateMarkerData {
                update_marker: marker,
                collision_test: 0,
            };
            mxc_epdc::wait_for_update_complete(&self.framebuffer, &mut wait)?;
        } else {
            let mut update = plan.hwtcon_update_data(marker);
            hwtcon::send_update(&self.framebuffer, &mut update)?;
            let mut wait = hwtcon::HwtconUpdateMarkerData {
                update_marker: marker,
                collision_test: 0,
            };
            hwtcon::wait_for_update_complete(&self.framebuffer, &mut wait)?;
        }
        Ok(())
    }
}

/// Returns a random nonzero update marker.
///
/// # Errors
///
/// Returns an error when the system random source is unreadable.
fn unique_marker() -> Result<u32, DisplayError> {
    let mut bytes = [0_u8; 4];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    // Keep the value large so it cannot coincide with the small sequential
    // markers the stock reader is observed to use.
    Ok((u32::from_le_bytes(bytes) | 0x4000_0000).max(1))
}

#[cfg(test)]
mod tests {
    use super::{unique_marker, DisplayError, DisplaySession, OWNER_UNLOCK_PHRASE};
    use kobo_profile::{
        Bitfield, DeviceSnapshot, FramebufferSnapshot, IdentitySnapshot, TouchSnapshot,
        CLARA_BW_391,
    };
    use std::path::Path;

    fn matched_snapshot() -> DeviceSnapshot {
        let channel = Bitfield {
            offset: 0,
            length: 8,
            msb_right: 0,
        };
        DeviceSnapshot {
            compatible: vec!["mediatek,mt8110".into(), "mediatek,mt8512".into()],
            model: Some("MediaTek MT8110 board".into()),
            framebuffer: Some(FramebufferSnapshot {
                id: "hwtcon".into(),
                width: 1072,
                height: 1448,
                virtual_width: 1072,
                virtual_height: 1448,
                x_offset: 0,
                y_offset: 0,
                bits_per_pixel: 32,
                grayscale: 0,
                stride: 4288,
                memory_length: 6_243_328,
                kind: 0,
                visual: 2,
                rotation: 3,
                red: channel,
                green: Bitfield {
                    offset: 8,
                    ..channel
                },
                blue: Bitfield {
                    offset: 16,
                    ..channel
                },
                alpha: Bitfield {
                    offset: 24,
                    ..channel
                },
            }),
            touch: Some(TouchSnapshot {
                path: "/dev/input/event1".into(),
                name: "cyttsp5_mt".into(),
                x_min: 0,
                x_max: 1447,
                y_min: 0,
                y_max: 1071,
            }),
            identity: IdentitySnapshot {
                serial_prefix: Some("N365".into()),
                firmware_version: Some("4.45.23697".into()),
                kernel_release: Some("4.9.77".into()),
                device_code: Some(391),
            },
        }
    }

    #[test]
    fn refuses_a_wrong_unlock_phrase_before_probing() {
        assert!(matches!(
            DisplaySession::open(None),
            Err(DisplayError::UnlockMissing)
        ));
        assert!(matches!(
            DisplaySession::open(Some("please")),
            Err(DisplayError::UnlockMissing)
        ));
        assert_eq!(OWNER_UNLOCK_PHRASE, "OWNER_ATTENDED_DISPLAY_WRITE");
    }

    #[test]
    fn refuses_a_device_whose_identity_does_not_match() {
        for identity in [
            IdentitySnapshot::default(),
            IdentitySnapshot {
                device_code: Some(390),
                ..matched_snapshot().identity
            },
            IdentitySnapshot {
                firmware_version: Some("4.46.0".into()),
                ..matched_snapshot().identity
            },
            IdentitySnapshot {
                kernel_release: Some("5.10.0".into()),
                ..matched_snapshot().identity
            },
            IdentitySnapshot {
                serial_prefix: Some("N249".into()),
                ..matched_snapshot().identity
            },
        ] {
            let snapshot = DeviceSnapshot {
                identity,
                ..matched_snapshot()
            };
            assert!(matches!(
                DisplaySession::open_verified(&CLARA_BW_391, snapshot, Path::new("/dev/null")),
                Err(DisplayError::IdentityRejected(_))
            ));
        }
    }

    #[test]
    fn refuses_hardware_that_does_not_match_the_profile() {
        let mut snapshot = matched_snapshot();
        snapshot.framebuffer.as_mut().expect("framebuffer").stride = 4096;
        assert!(matches!(
            DisplaySession::open_verified(&CLARA_BW_391, snapshot, Path::new("/dev/null")),
            Err(DisplayError::ProfileRejected(_))
        ));
    }

    #[test]
    fn markers_are_high_entropy_and_never_collide_with_low_reader_markers() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..64 {
            let marker = unique_marker().expect("random marker");
            assert!(marker >= 0x4000_0000);
            assert_ne!(marker, 0);
            seen.insert(marker);
        }
        assert!(seen.len() > 32, "markers should not repeat");
    }
}

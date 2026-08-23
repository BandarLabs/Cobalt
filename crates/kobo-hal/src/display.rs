//! The single verified entry point for changing the display.
//!
//! Nothing else in this project may submit a hardware update. Opening a
//! [`DisplaySession`] proves, at runtime, that:
//!
//! 1. the probed hardware geometry matches the profile exactly,
//! 2. the device code, serial model prefix, firmware version, and kernel
//!    release match the profile exactly,
//! 3. the profile's owner-attended evidence is complete, and
//! 4. the caller supplied the exact owner-attended unlock phrase.
//!
//! The module is compiled only with the non-default `device-write` feature, so
//! a default build contains no callable display-write code at all.

use crate::probe::{probe_device, ProbeError};
use crate::refresh::{Backend, Rect, RefreshPlan};
use crate::surface::{self, RegionSnapshot, SurfaceError, SurfaceGeometry};
use kobo_abi::{hwtcon, mxcfb};
use kobo_profile::{DeviceProfile, DeviceSnapshot, WRITE_EVIDENCE_PENDING};
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::path::Path;
use std::thread::sleep;
use std::time::{Duration, Instant};

/// The exact phrase an owner must supply to open a write session.
pub const OWNER_UNLOCK_PHRASE: &str = "OWNER_ATTENDED_DISPLAY_WRITE";

const SMOKE_FIXED_REGION: Rect = Rect {
    x: 512,
    y: 704,
    width: 32,
    height: 32,
};
const SMOKE_PATCH_REGION: Rect = Rect {
    x: 408,
    y: 600,
    width: 256,
    height: 256,
};
const SMOKE_VISIBLE_HOLD: Duration = Duration::from_millis(1200);
const ATTENDED_SMOKE_UNLOCK_PHRASE: &str = "OWNER_ATTENDED_CANDIDATE_DISPLAY_VALIDATION";

/// One bounded operation used to gather owner-attended display evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttendedSmokeStage {
    DisplayOnly,
    ReversiblePixels,
    ScreenSnapshot,
    FastFeedback,
    /// Measures the submit and wait ioctls across the three offered
    /// waveforms, reversibly, on the fixed patch region.
    WaitTiming,
}

impl AttendedSmokeStage {
    const fn intent(self) -> crate::refresh::RefreshIntent {
        match self {
            Self::FastFeedback => crate::refresh::RefreshIntent::FastFeedback,
            _ => crate::refresh::RefreshIntent::QualityContent,
        }
    }
}

#[derive(Debug)]
pub enum DisplayError {
    UnlockMissing,
    ProfileRejected(Vec<String>),
    WriteRejected(Vec<String>),
    Smoke(String),
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
            Self::WriteRejected(reasons) => {
                write!(formatter, "device write rejected: {}", reasons.join("; "))
            }
            Self::Smoke(reason) => write!(formatter, "attended display smoke: {reason}"),
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
    backend: Backend,
    profile: &'static DeviceProfile,
    snapshot: DeviceSnapshot,
}

#[derive(Clone, Copy)]
enum WritePolicy {
    ReadyOnly,
    AttendedCandidateValidation,
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
        Self::open_verified(
            profile,
            snapshot,
            Path::new("/dev/fb0"),
            WritePolicy::ReadyOnly,
        )
    }

    fn open_for_attended_validation() -> Result<Self, DisplayError> {
        let snapshot = probe_device().map_err(DisplayError::Probe)?;
        let profile = kobo_profile::identify_profile(&snapshot).ok_or_else(|| {
            DisplayError::ProfileRejected(vec![
                "no supported hardware profile matched this device".to_owned()
            ])
        })?;
        Self::open_verified(
            profile,
            snapshot,
            Path::new("/dev/fb0"),
            WritePolicy::AttendedCandidateValidation,
        )
    }

    fn open_verified(
        profile: &'static DeviceProfile,
        snapshot: DeviceSnapshot,
        framebuffer_path: &Path,
        policy: WritePolicy,
    ) -> Result<Self, DisplayError> {
        let report = profile.validate(&snapshot);
        if !report.mismatches.is_empty() {
            return Err(DisplayError::ProfileRejected(report.mismatches));
        }
        let mut blockers = report.write_blockers;
        if matches!(policy, WritePolicy::AttendedCandidateValidation) {
            blockers.retain(|blocker| blocker != WRITE_EVIDENCE_PENDING);
        }
        if !blockers.is_empty() {
            return Err(DisplayError::WriteRejected(blockers));
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
            backend: Backend::from_controller(profile.framebuffer_controller),
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

    /// The panel-controller interface this device speaks.
    #[must_use]
    pub fn backend(&self) -> Backend {
        self.backend
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
        self.refresh_timed(plan).map(|_| ())
    }

    /// [`Self::refresh`], instrumented.
    ///
    /// Measures the submit and wait ioctls separately and reads back the
    /// waveform the driver actually selected: both vendors' `SEND_UPDATE`
    /// requests are in-out, and the driver copies the translated waveform mode
    /// back into the struct. The regular [`Self::refresh`] path discards it.
    ///
    /// # Errors
    ///
    /// Returns an error when the region is invalid or either ioctl fails.
    pub fn refresh_timed(&self, plan: RefreshPlan) -> Result<RefreshTiming, DisplayError> {
        // Validate the region against this exact surface before the kernel sees it.
        surface::RegionPlacement::new(self.geometry, plan.region)?;
        let marker = unique_marker()?;
        // The two backends' wait requests happen to be the same number over
        // the same struct, so one path would work for both. It is still
        // written out twice: the coincidence belongs to this kernel, not to
        // the interface, and a device whose wait struct grew a field would
        // otherwise be served a MediaTek ioctl through an i.MX session with
        // nothing in the code to make that visible.
        let submitted_waveform = plan.waveform(self.backend);
        let (translated_waveform, submit, wait) = match self.backend {
            Backend::Hwtcon => {
                let mut update = plan.hwtcon_update_data(marker);
                let submit_started = Instant::now();
                hwtcon::send_update(&self.framebuffer, &mut update)?;
                let submit = submit_started.elapsed();
                let mut wait = hwtcon::HwtconUpdateMarkerData {
                    update_marker: marker,
                    collision_test: 0,
                };
                let wait_started = Instant::now();
                hwtcon::wait_for_update_complete(&self.framebuffer, &mut wait)?;
                (update.waveform_mode, submit, wait_started.elapsed())
            }
            Backend::Mxcfb => {
                let mut update = plan.mxcfb_update_data(marker);
                let submit_started = Instant::now();
                mxcfb::send_update(&self.framebuffer, &mut update)?;
                let submit = submit_started.elapsed();
                let mut wait = mxcfb::MxcfbUpdateMarkerData {
                    update_marker: marker,
                    collision_test: 0,
                };
                let wait_started = Instant::now();
                mxcfb::wait_for_update_complete(&self.framebuffer, &mut wait)?;
                (update.waveform_mode, submit, wait_started.elapsed())
            }
        };
        Ok(RefreshTiming {
            submitted_waveform,
            translated_waveform,
            submit,
            wait,
        })
    }
}

/// What one instrumented refresh measured.
#[derive(Clone, Copy, Debug)]
pub struct RefreshTiming {
    /// The waveform constant submitted with the update.
    pub submitted_waveform: u32,
    /// The waveform the driver copied back after translating the request
    /// through the device's waveform table.
    pub translated_waveform: u32,
    /// How long the submit ioctl took.
    pub submit: Duration,
    /// How long the wait-for-complete ioctl blocked.
    pub wait: Duration,
}

/// Runs one fixed, bounded, restoration-verified candidate display check.
///
/// This is the only operation allowed to ignore the evidence-pending blocker.
/// It never exposes the underlying candidate-capable [`DisplaySession`]; the
/// regions, waveform intent, restoration, and verification all remain owned by
/// this hardware boundary. Geometry, framebuffer safety, and exact identity
/// blockers are still enforced before the framebuffer is opened.
///
/// # Errors
///
/// Returns an error when probing or exact validation fails, a fixed region is
/// invalid, a framebuffer operation fails, or restored bytes differ.
pub fn run_attended_smoke(
    stage: AttendedSmokeStage,
    unlock: Option<&str>,
) -> Result<String, DisplayError> {
    if unlock != Some(ATTENDED_SMOKE_UNLOCK_PHRASE) {
        return Err(DisplayError::UnlockMissing);
    }
    let session = DisplaySession::open_for_attended_validation()?;
    let plan = RefreshPlan::new(
        SMOKE_FIXED_REGION,
        stage.intent(),
        false,
        session.geometry().width,
        session.geometry().height,
    )
    .ok_or_else(|| DisplayError::Smoke("fixed region is not inside this screen".to_owned()))?;

    match stage {
        AttendedSmokeStage::DisplayOnly => {
            session.refresh(plan)?;
            Ok("display-only GC16 refresh completed; no pixel byte was written".to_owned())
        }
        AttendedSmokeStage::ReversiblePixels => {
            let original = session.capture(SMOKE_FIXED_REGION)?;
            smoke_show_and_restore(&session, plan, &original)?;
            Ok(format!(
                "reversible GC16 pixel test completed; {} bytes restored and verified",
                original.pixels().len()
            ))
        }
        AttendedSmokeStage::ScreenSnapshot => smoke_screen_snapshot_restore(&session),
        AttendedSmokeStage::WaitTiming => smoke_wait_timing(&session),
        AttendedSmokeStage::FastFeedback => {
            let original = session.capture(SMOKE_FIXED_REGION)?;
            smoke_show_and_restore(&session, plan, &original)?;
            Ok(format!(
                "reversible DU pixel test completed; {} bytes restored and verified",
                original.pixels().len()
            ))
        }
    }
}

const WAIT_TIMING_ROUNDS: usize = 4;

/// Measures the submit and wait ioctls, reversibly, on the patch region.
///
/// Each of the three offered waveforms is driven [`WAIT_TIMING_ROUNDS`] times
/// through an invert-and-restore pair, and every update reports the waveform
/// the driver actually translated the request to alongside both ioctl
/// durations. The screen is left exactly as found, and the restoration is
/// verified byte for byte even when a refresh fails mid-run.
fn smoke_wait_timing(session: &DisplaySession) -> Result<String, DisplayError> {
    use std::fmt::Write as _;

    let original = session.capture(SMOKE_PATCH_REGION)?;
    let inverted = original.inverted_rgb();

    let mut lines = String::from("update  intent   waveform  translated  submit_us  wait_us\n");
    let mut run = || -> Result<(), DisplayError> {
        let mut update = 0_usize;
        for intent in [
            crate::refresh::RefreshIntent::QualityContent,
            crate::refresh::RefreshIntent::TextContent,
            crate::refresh::RefreshIntent::FastFeedback,
        ] {
            let plan = smoke_plan_with_intent(session, SMOKE_PATCH_REGION, intent)?;
            for _ in 0..WAIT_TIMING_ROUNDS {
                for snapshot in [&inverted, &original] {
                    session.restore(snapshot)?;
                    let timing = session.refresh_timed(plan)?;
                    update += 1;
                    let _ = writeln!(
                        lines,
                        "{update:>6}  {:<8} {:>8}  {:>10}  {:>9}  {:>7}",
                        match intent {
                            crate::refresh::RefreshIntent::QualityContent => "GC16",
                            crate::refresh::RefreshIntent::TextContent => "GL16",
                            crate::refresh::RefreshIntent::FastFeedback => "DU",
                        },
                        timing.submitted_waveform,
                        timing.translated_waveform,
                        timing.submit.as_micros(),
                        timing.wait.as_micros(),
                    );
                }
            }
        }
        Ok(())
    };
    let outcome = run();

    // Always leave the screen as found, even when a refresh failed mid-run.
    let restored = session.restore(&original);
    outcome?;
    restored?;
    let verify = session.capture(SMOKE_PATCH_REGION)?;
    if !verify.matches(&original) {
        return Err(DisplayError::Smoke(
            "patch region does not match the original bytes".to_owned(),
        ));
    }
    Ok(format!(
        "{lines}wait timing completed; {} bytes restored and verified",
        original.pixels().len()
    ))
}

fn smoke_plan_with_intent(
    session: &DisplaySession,
    region: Rect,
    intent: crate::refresh::RefreshIntent,
) -> Result<RefreshPlan, DisplayError> {
    RefreshPlan::new(
        region,
        intent,
        false,
        session.geometry().width,
        session.geometry().height,
    )
    .ok_or_else(|| DisplayError::Smoke(format!("region {region:?} is not inside this screen")))
}

fn smoke_screen_snapshot_restore(session: &DisplaySession) -> Result<String, DisplayError> {
    let geometry = session.geometry();
    let whole_screen = Rect {
        x: 0,
        y: 0,
        width: geometry.width,
        height: geometry.height,
    };
    let screen = session.capture(whole_screen)?;
    let patch = session.capture(SMOKE_PATCH_REGION)?;
    let patch_plan = smoke_plan_for(session, SMOKE_PATCH_REGION)?;
    let screen_plan = smoke_plan_for(session, whole_screen)?;

    let shown = session
        .restore(&patch.inverted_rgb())
        .and_then(|()| session.refresh(patch_plan));
    if shown.is_ok() {
        sleep(SMOKE_VISIBLE_HOLD);
    }
    let restored = session
        .restore(&screen)
        .and_then(|()| session.refresh(screen_plan));
    shown?;
    restored?;

    let verify = session.capture(SMOKE_PATCH_REGION)?;
    if !verify.matches(&patch) {
        return Err(DisplayError::Smoke(
            "the changed region was not restored by the whole-screen write".to_owned(),
        ));
    }
    Ok(format!(
        "whole-screen snapshot and restore completed; {} screen bytes captured, \
         {} bytes changed and verified restored",
        screen.pixels().len(),
        patch.pixels().len()
    ))
}

fn smoke_plan_for(session: &DisplaySession, region: Rect) -> Result<RefreshPlan, DisplayError> {
    RefreshPlan::new(
        region,
        crate::refresh::RefreshIntent::QualityContent,
        false,
        session.geometry().width,
        session.geometry().height,
    )
    .ok_or_else(|| DisplayError::Smoke(format!("region {region:?} is not inside this screen")))
}

fn smoke_show_and_restore(
    session: &DisplaySession,
    plan: RefreshPlan,
    original: &RegionSnapshot,
) -> Result<(), DisplayError> {
    let shown = session
        .restore(&original.inverted_rgb())
        .and_then(|()| session.refresh(plan));
    if shown.is_ok() {
        sleep(SMOKE_VISIBLE_HOLD);
    }
    let restored = session
        .restore(original)
        .and_then(|()| session.refresh(plan));
    shown?;
    restored?;

    let verify = session.capture(original.placement().region())?;
    if verify.matches(original) {
        Ok(())
    } else {
        Err(DisplayError::Smoke(
            "restored region does not match the original bytes".to_owned(),
        ))
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
    use super::{
        unique_marker, AttendedSmokeStage, DisplayError, DisplaySession, Rect, RefreshPlan,
        WritePolicy, ATTENDED_SMOKE_UNLOCK_PHRASE, OWNER_UNLOCK_PHRASE, SMOKE_FIXED_REGION,
        SMOKE_PATCH_REGION, SMOKE_VISIBLE_HOLD,
    };
    use crate::surface::{RegionPlacement, SurfaceGeometry};
    use kobo_abi::hwtcon;
    use kobo_profile::{
        DeviceProfile, DeviceSnapshot, FramebufferSnapshot, IdentitySnapshot, TouchSnapshot,
        CLARA_BW_391, CLARA_HD_376, ELIPSA_2E_389, WRITE_EVIDENCE_PENDING,
    };
    use std::path::Path;

    fn matched_snapshot() -> DeviceSnapshot {
        snapshot_for(
            &CLARA_BW_391,
            IdentitySnapshot {
                serial_prefix: Some("N365".into()),
                firmware_version: Some("4.45.23697".into()),
                kernel_release: Some("4.9.77".into()),
                device_code: Some(391),
            },
        )
    }

    fn snapshot_for(profile: &DeviceProfile, identity: IdentitySnapshot) -> DeviceSnapshot {
        DeviceSnapshot {
            compatible: profile
                .compatible_fragments
                .iter()
                .map(|value| (*value).to_owned())
                .collect(),
            model: Some(profile.device_tree_model.to_owned()),
            framebuffer: Some(FramebufferSnapshot {
                id: profile.framebuffer_id.to_owned(),
                width: profile.width,
                height: profile.height,
                virtual_width: profile.virtual_width,
                virtual_height: profile.virtual_height,
                x_offset: profile.x_offset,
                y_offset: profile.y_offset,
                bits_per_pixel: profile.bits_per_pixel,
                grayscale: profile.grayscale,
                stride: profile.stride,
                memory_length: profile.memory_length,
                kind: profile.framebuffer_kind,
                visual: profile.framebuffer_visual,
                rotation: profile.rotation,
                red: profile.red,
                green: profile.green,
                blue: profile.blue,
                alpha: profile.alpha,
            }),
            touch: Some(TouchSnapshot {
                path: "/dev/input/event1".into(),
                name: profile.touch_name.into(),
                x_min: profile.touch_x_min,
                x_max: profile.touch_x_max,
                y_min: profile.touch_y_min,
                y_max: profile.touch_y_max,
            }),
            identity,
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
        assert!(matches!(
            super::run_attended_smoke(AttendedSmokeStage::DisplayOnly, None),
            Err(DisplayError::UnlockMissing)
        ));
        assert!(matches!(
            super::run_attended_smoke(AttendedSmokeStage::DisplayOnly, Some("please")),
            Err(DisplayError::UnlockMissing)
        ));
        assert_ne!(ATTENDED_SMOKE_UNLOCK_PHRASE, OWNER_UNLOCK_PHRASE);
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
                DisplaySession::open_verified(
                    &CLARA_BW_391,
                    snapshot,
                    Path::new("/dev/null"),
                    WritePolicy::ReadyOnly,
                ),
                Err(DisplayError::WriteRejected(_))
            ));
        }
    }

    #[test]
    fn ordinary_writes_refuse_a_candidate_but_attended_smoke_may_open_it() {
        const CANDIDATE: kobo_profile::DeviceProfile = kobo_profile::DeviceProfile {
            write_ready: false,
            ..ELIPSA_2E_389
        };
        let identity = IdentitySnapshot {
            serial_prefix: Some("N605".into()),
            firmware_version: Some("4.38.23697".into()),
            kernel_release: Some("4.9.77".into()),
            device_code: Some(389),
        };
        let snapshot = snapshot_for(&CANDIDATE, identity);
        let Err(error) = DisplaySession::open_verified(
            &CANDIDATE,
            snapshot.clone(),
            Path::new("/dev/null"),
            WritePolicy::ReadyOnly,
        ) else {
            panic!("ordinary display writes require completed evidence");
        };
        assert!(
            matches!(error, DisplayError::WriteRejected(ref blockers) if blockers.iter().any(|blocker| blocker == WRITE_EVIDENCE_PENDING))
        );

        DisplaySession::open_verified(
            &CANDIDATE,
            snapshot,
            Path::new("/dev/null"),
            WritePolicy::AttendedCandidateValidation,
        )
        .expect("the bounded attended smoke path may gather the missing evidence");
    }

    #[test]
    fn attended_smoke_never_bypasses_exact_identity() {
        let snapshot = snapshot_for(&ELIPSA_2E_389, IdentitySnapshot::default());
        assert!(matches!(
            DisplaySession::open_verified(
                &ELIPSA_2E_389,
                snapshot,
                Path::new("/dev/null"),
                WritePolicy::AttendedCandidateValidation,
            ),
            Err(DisplayError::WriteRejected(_))
        ));
    }

    #[test]
    fn hal_owned_smoke_regions_are_bounded_on_every_registered_panel() {
        for profile in [&CLARA_BW_391, &CLARA_HD_376, &ELIPSA_2E_389] {
            let geometry = SurfaceGeometry {
                width: profile.width,
                height: profile.height,
                stride: profile.stride,
                bits_per_pixel: profile.bits_per_pixel,
                memory_length: u64::from(profile.memory_length),
            };
            let fixed = RegionPlacement::new(geometry, SMOKE_FIXED_REGION)
                .expect("fixed smoke region fits the supported panel");
            assert_eq!(fixed.total_bytes(), 32 * 32 * 4);
            let patch = RegionPlacement::new(geometry, SMOKE_PATCH_REGION)
                .expect("patch smoke region fits the supported panel");
            assert_eq!(patch.total_bytes(), 256 * 256 * 4);

            let whole = Rect {
                x: 0,
                y: 0,
                width: profile.width,
                height: profile.height,
            };
            RegionPlacement::new(geometry, whole).expect("whole panel is valid");
        }
    }

    #[test]
    fn hal_owned_smoke_stages_use_only_partial_gc16_or_du_updates() {
        for (stage, waveform) in [
            (AttendedSmokeStage::DisplayOnly, hwtcon::WAVEFORM_GC16),
            (AttendedSmokeStage::ReversiblePixels, hwtcon::WAVEFORM_GC16),
            (AttendedSmokeStage::ScreenSnapshot, hwtcon::WAVEFORM_GC16),
            (AttendedSmokeStage::FastFeedback, hwtcon::WAVEFORM_DU),
        ] {
            let plan = RefreshPlan::new(
                SMOKE_FIXED_REGION,
                stage.intent(),
                false,
                CLARA_BW_391.width,
                CLARA_BW_391.height,
            )
            .expect("fixed plan");
            let update = plan.hwtcon_update_data(0x4000_0001);
            assert_eq!(update.waveform_mode, waveform);
            assert_eq!(update.update_mode, hwtcon::UPDATE_MODE_PARTIAL);
        }
        assert!(SMOKE_VISIBLE_HOLD.as_secs() < 5);
    }

    #[test]
    fn refuses_hardware_that_does_not_match_the_profile() {
        let mut snapshot = matched_snapshot();
        snapshot.framebuffer.as_mut().expect("framebuffer").stride = 4096;
        assert!(matches!(
            DisplaySession::open_verified(
                &CLARA_BW_391,
                snapshot,
                Path::new("/dev/null"),
                WritePolicy::ReadyOnly,
            ),
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

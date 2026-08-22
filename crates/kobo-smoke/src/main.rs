//! Owner-attended, fully reversible display smoke tests.
//!
//! Two stages are selected by the exact value of `KOBO_SMOKE_UNLOCK`:
//!
//! - `OWNER_ATTENDED_DISPLAY_ONLY_GC16` asks the controller to re-render one
//!   fixed region without changing a single pixel byte.
//! - `OWNER_ATTENDED_REVERSIBLE_PIXELS_GC16` additionally inverts that region,
//!   shows it, and then restores the exact original bytes and verifies the
//!   restoration byte for byte.
//! - `OWNER_ATTENDED_SCREEN_SNAPSHOT_RESTORE` snapshots the entire screen,
//!   changes a larger region, and then puts the whole screen back from that
//!   snapshot. This is the guarantee everything else rests on: whatever a
//!   future runtime draws, the reader's own screen can always be restored
//!   exactly.
//!
//! The first two stages are bounded to one fixed 32x32 region. The third reads
//! and rewrites only the visible framebuffer. No other waveform, device, or
//! file can be addressed, and nothing is written outside the framebuffer.

use kobo_hal::display::{DisplaySession, OWNER_UNLOCK_PHRASE};
use kobo_hal::{Rect, RefreshIntent, RefreshPlan, RegionSnapshot};
use std::env;
use std::process::ExitCode;
use std::thread::sleep;
use std::time::Duration;

const UNLOCK_ENV: &str = "KOBO_SMOKE_UNLOCK";
const UNLOCK_DISPLAY_ONLY: &str = "OWNER_ATTENDED_DISPLAY_ONLY_GC16";
const UNLOCK_REVERSIBLE_PIXELS: &str = "OWNER_ATTENDED_REVERSIBLE_PIXELS_GC16";
const UNLOCK_SCREEN_SNAPSHOT: &str = "OWNER_ATTENDED_SCREEN_SNAPSHOT_RESTORE";
const UNLOCK_FAST_FEEDBACK: &str = "OWNER_ATTENDED_REVERSIBLE_PIXELS_DU";

const FIXED_REGION: Rect = Rect {
    x: 512,
    y: 704,
    width: 32,
    height: 32,
};
/// The larger region the snapshot stage changes, so a full-screen restore is
/// visibly doing something rather than trivially succeeding.
const PATCH_REGION: Rect = Rect {
    x: 408,
    y: 600,
    width: 256,
    height: 256,
};
const VISIBLE_HOLD: Duration = Duration::from_millis(1200);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Stage {
    DisplayOnly,
    ReversiblePixels,
    ScreenSnapshot,
    FastFeedback,
}

impl Stage {
    /// The refresh intent this stage exercises.
    const fn intent(self) -> RefreshIntent {
        match self {
            Self::FastFeedback => RefreshIntent::FastFeedback,
            _ => RefreshIntent::QualityContent,
        }
    }
}

impl Stage {
    fn from_unlock(unlock: Option<&str>) -> Option<Self> {
        match unlock {
            Some(UNLOCK_DISPLAY_ONLY) => Some(Self::DisplayOnly),
            Some(UNLOCK_REVERSIBLE_PIXELS) => Some(Self::ReversiblePixels),
            Some(UNLOCK_SCREEN_SNAPSHOT) => Some(Self::ScreenSnapshot),
            Some(UNLOCK_FAST_FEEDBACK) => Some(Self::FastFeedback),
            _ => None,
        }
    }
}

fn main() -> ExitCode {
    match run(env::var(UNLOCK_ENV).ok().as_deref()) {
        Ok(report) => {
            println!("{report}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("kobo-smoke: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(unlock: Option<&str>) -> Result<String, String> {
    let stage = Stage::from_unlock(unlock)
        .ok_or_else(|| "owner-attended smoke unlock is missing or incorrect".to_owned())?;
    let session =
        DisplaySession::open(Some(OWNER_UNLOCK_PHRASE)).map_err(|error| error.to_string())?;
    let plan = RefreshPlan::new(
        FIXED_REGION,
        stage.intent(),
        false,
        session.geometry().width,
        session.geometry().height,
    )
    .ok_or_else(|| "fixed region is not inside this screen".to_owned())?;

    match stage {
        Stage::DisplayOnly => {
            session.refresh(plan).map_err(|error| error.to_string())?;
            Ok("display-only GC16 refresh completed; no pixel byte was written".to_owned())
        }
        Stage::ReversiblePixels => {
            let original = session
                .capture(FIXED_REGION)
                .map_err(|error| format!("capture region: {error}"))?;
            show_and_restore(&session, plan, &original)?;
            Ok(format!(
                "reversible GC16 pixel test completed; {} bytes restored and verified",
                original.pixels().len()
            ))
        }
        Stage::ScreenSnapshot => screen_snapshot_restore(&session),
        Stage::FastFeedback => {
            // DU is the two-level waveform interactive feedback uses. It is
            // driven through exactly the same capture, show, restore, verify
            // path as the GC16 stage, so only the waveform differs.
            let original = session
                .capture(FIXED_REGION)
                .map_err(|error| format!("capture region: {error}"))?;
            show_and_restore(&session, plan, &original)?;
            Ok(format!(
                "reversible DU pixel test completed; {} bytes restored and verified",
                original.pixels().len()
            ))
        }
    }
}

/// Snapshots the whole screen, changes a larger region, then puts the entire
/// screen back from that snapshot and proves the change is gone.
///
/// The full-screen snapshot is taken first and restored on every path,
/// including every error path, so the reader's screen is never left changed.
fn screen_snapshot_restore(session: &DisplaySession) -> Result<String, String> {
    let geometry = session.geometry();
    let whole_screen = Rect {
        x: 0,
        y: 0,
        width: geometry.width,
        height: geometry.height,
    };
    let screen = session
        .capture(whole_screen)
        .map_err(|error| format!("capture whole screen: {error}"))?;
    let patch = session
        .capture(PATCH_REGION)
        .map_err(|error| format!("capture patch region: {error}"))?;

    let patch_plan = plan_for(session, PATCH_REGION)?;
    let screen_plan = plan_for(session, whole_screen)?;

    // Change a large region, then put the whole screen back from the snapshot
    // rather than from the region snapshot, so it is the full-screen restore
    // that is being proven.
    let shown = session
        .restore(&patch.inverted_rgb())
        .map_err(|error| format!("write inverted patch: {error}"))
        .and_then(|()| {
            session
                .refresh(patch_plan)
                .map_err(|error| format!("refresh inverted patch: {error}"))
        });
    if shown.is_ok() {
        sleep(VISIBLE_HOLD);
    }

    let restored = session
        .restore(&screen)
        .map_err(|error| format!("restore whole screen: {error}"))
        .and_then(|()| {
            session
                .refresh(screen_plan)
                .map_err(|error| format!("refresh restored screen: {error}"))
        });

    shown?;
    restored?;

    let verify = session
        .capture(PATCH_REGION)
        .map_err(|error| format!("verify patch region: {error}"))?;
    if !verify.matches(&patch) {
        return Err("the changed region was not restored by the whole-screen write".to_owned());
    }
    Ok(format!(
        "whole-screen snapshot and restore completed; {} screen bytes captured, \
         {} bytes changed and verified restored",
        screen.pixels().len(),
        patch.pixels().len()
    ))
}

fn plan_for(session: &DisplaySession, region: Rect) -> Result<RefreshPlan, String> {
    RefreshPlan::new(
        region,
        RefreshIntent::QualityContent,
        false,
        session.geometry().width,
        session.geometry().height,
    )
    .ok_or_else(|| format!("region {region:?} is not inside this screen"))
}

/// Inverts the captured region, shows it, then always restores the original
/// bytes before returning, including on every error path.
fn show_and_restore(
    session: &DisplaySession,
    plan: RefreshPlan,
    original: &RegionSnapshot,
) -> Result<(), String> {
    let inverted = original.inverted_rgb();
    let shown = session
        .restore(&inverted)
        .map_err(|error| format!("write inverted region: {error}"))
        .and_then(|()| {
            session
                .refresh(plan)
                .map_err(|error| format!("refresh inverted region: {error}"))
        });
    if shown.is_ok() {
        sleep(VISIBLE_HOLD);
    }

    let restored = session
        .restore(original)
        .map_err(|error| format!("restore original region: {error}"))
        .and_then(|()| {
            session
                .refresh(plan)
                .map_err(|error| format!("refresh restored region: {error}"))
        });

    shown?;
    restored?;

    let verify = session
        .capture(original.placement().region())
        .map_err(|error| format!("verify restored region: {error}"))?;
    if verify.matches(original) {
        Ok(())
    } else {
        Err("restored region does not match the original bytes".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Stage, FIXED_REGION, PATCH_REGION, UNLOCK_DISPLAY_ONLY, UNLOCK_FAST_FEEDBACK,
        UNLOCK_REVERSIBLE_PIXELS, UNLOCK_SCREEN_SNAPSHOT, VISIBLE_HOLD,
    };
    use kobo_abi::hwtcon;
    use kobo_hal::surface::{RegionPlacement, SurfaceGeometry};
    use kobo_hal::{Rect, RefreshIntent, RefreshPlan, RefreshWaveform};

    const CLARA: SurfaceGeometry = SurfaceGeometry {
        width: 1072,
        height: 1448,
        stride: 4288,
        bits_per_pixel: 32,
        memory_length: 6_243_328,
    };

    #[test]
    fn the_patch_region_is_inside_the_screen_and_larger_than_the_fixed_region() {
        let placement = RegionPlacement::new(CLARA, PATCH_REGION).expect("region is valid");
        assert_eq!(placement.total_bytes(), 256 * 256 * 4);
        const {
            assert!(PATCH_REGION.x + PATCH_REGION.width <= CLARA.width);
            assert!(PATCH_REGION.y + PATCH_REGION.height <= CLARA.height);
        }
    }

    #[test]
    fn a_whole_screen_region_is_a_valid_placement() {
        let whole = Rect {
            x: 0,
            y: 0,
            width: CLARA.width,
            height: CLARA.height,
        };
        let placement = RegionPlacement::new(CLARA, whole).expect("whole screen is valid");
        assert_eq!(
            placement.total_bytes(),
            (CLARA.stride as usize) * (CLARA.height as usize)
        );
        // A whole-screen update must still be a partial-mode update, because
        // full mode is an untested code path on this controller.
        let plan = RefreshPlan::new(
            whole,
            RefreshIntent::QualityContent,
            false,
            CLARA.width,
            CLARA.height,
        )
        .expect("plan is valid");
        assert_eq!(
            plan.hwtcon_update_data(0x4000_0002).update_mode,
            hwtcon::UPDATE_MODE_PARTIAL
        );
    }

    #[test]
    fn only_the_exact_unlock_phrases_select_a_stage() {
        for (phrase, expected) in [
            (UNLOCK_DISPLAY_ONLY, Stage::DisplayOnly),
            (UNLOCK_REVERSIBLE_PIXELS, Stage::ReversiblePixels),
            (UNLOCK_SCREEN_SNAPSHOT, Stage::ScreenSnapshot),
            (UNLOCK_FAST_FEEDBACK, Stage::FastFeedback),
        ] {
            assert_eq!(Stage::from_unlock(Some(phrase)), Some(expected));
        }
        for wrong in [
            None,
            Some(""),
            Some("owner_attended_display_only_gc16"),
            Some("OWNER_ATTENDED_DISPLAY_ONLY_GC16 "),
            Some("OWNER_ATTENDED_DISPLAY_WRITE"),
            Some("OWNER_ATTENDED_REVERSIBLE_PIXELS_DU "),
            Some("REVERSIBLE_PIXELS_DU"),
        ] {
            assert_eq!(Stage::from_unlock(wrong), None, "{wrong:?} must not unlock");
        }
    }

    #[test]
    fn only_the_fast_feedback_stage_uses_the_du_waveform() {
        for (stage, waveform) in [
            (Stage::DisplayOnly, RefreshWaveform::Gc16),
            (Stage::ReversiblePixels, RefreshWaveform::Gc16),
            (Stage::ScreenSnapshot, RefreshWaveform::Gc16),
            (Stage::FastFeedback, RefreshWaveform::Du),
        ] {
            let plan = RefreshPlan::new(
                FIXED_REGION,
                stage.intent(),
                false,
                CLARA.width,
                CLARA.height,
            )
            .expect("plan");
            assert_eq!(plan.waveform, waveform, "wrong waveform for {stage:?}");
            // No stage may ever submit a full-mode update: full is untested on
            // this controller.
            assert!(!plan.full);
        }
    }

    #[test]
    fn the_fixed_region_is_small_and_inside_the_screen() {
        assert_eq!(FIXED_REGION.width, 32);
        assert_eq!(FIXED_REGION.height, 32);
        let placement = RegionPlacement::new(CLARA, FIXED_REGION).expect("region is valid");
        assert_eq!(placement.total_bytes(), 4096);
    }

    #[test]
    fn the_plan_is_always_a_partial_gc16_update() {
        let plan = RefreshPlan::new(
            FIXED_REGION,
            RefreshIntent::QualityContent,
            false,
            CLARA.width,
            CLARA.height,
        )
        .expect("plan is valid");
        let update = plan.hwtcon_update_data(0x4000_0001);
        assert_eq!(update.waveform_mode, hwtcon::WAVEFORM_GC16);
        assert_eq!(update.update_mode, hwtcon::UPDATE_MODE_PARTIAL);
        assert_eq!(update.update_region.left, FIXED_REGION.x);
        assert_eq!(update.update_region.top, FIXED_REGION.y);
        assert_eq!(update.update_region.width, FIXED_REGION.width);
        assert_eq!(update.update_region.height, FIXED_REGION.height);
    }

    #[test]
    fn the_visible_hold_fits_inside_the_remote_lease() {
        assert!(VISIBLE_HOLD.as_secs() < 5);
    }
}

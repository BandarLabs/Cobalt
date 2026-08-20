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

use kobo_hal::display::{run_attended_smoke, AttendedSmokeStage};
use std::env;
use std::process::ExitCode;

const UNLOCK_ENV: &str = "KOBO_SMOKE_UNLOCK";
const UNLOCK_DISPLAY_ONLY: &str = "OWNER_ATTENDED_DISPLAY_ONLY_GC16";
const UNLOCK_REVERSIBLE_PIXELS: &str = "OWNER_ATTENDED_REVERSIBLE_PIXELS_GC16";
const UNLOCK_SCREEN_SNAPSHOT: &str = "OWNER_ATTENDED_SCREEN_SNAPSHOT_RESTORE";
const UNLOCK_FAST_FEEDBACK: &str = "OWNER_ATTENDED_REVERSIBLE_PIXELS_DU";
const HAL_VALIDATION_UNLOCK: &str = "OWNER_ATTENDED_CANDIDATE_DISPLAY_VALIDATION";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Stage {
    DisplayOnly,
    ReversiblePixels,
    ScreenSnapshot,
    FastFeedback,
}

impl From<Stage> for AttendedSmokeStage {
    fn from(stage: Stage) -> Self {
        match stage {
            Stage::DisplayOnly => Self::DisplayOnly,
            Stage::ReversiblePixels => Self::ReversiblePixels,
            Stage::ScreenSnapshot => Self::ScreenSnapshot,
            Stage::FastFeedback => Self::FastFeedback,
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
    run_attended_smoke(stage.into(), Some(HAL_VALIDATION_UNLOCK)).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        Stage, UNLOCK_DISPLAY_ONLY, UNLOCK_FAST_FEEDBACK, UNLOCK_REVERSIBLE_PIXELS,
        UNLOCK_SCREEN_SNAPSHOT,
    };

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
}

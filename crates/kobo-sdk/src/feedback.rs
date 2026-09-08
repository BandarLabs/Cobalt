//! Inline feedback that keeps the current content and controls in place.
//!
//! The short status line occupies one caption line at supported profile sizes.
//! Keep document/provider details in the surrounding content. Explain a failure
//! and expose recovery beside the affected action; this line does not replace
//! an error explanation, acquire data or claim a save on an app's behalf.

use crate::ScreenBuilder;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Status {
    Ready,
    Loading,
    Refreshing,
    Saving,
    Unsaved,
    WaitingToSync,
    Offline,
    Stale,
    Saved,
    Updated,
    SaveFailed,
    UpdateFailed,
}
impl Status {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Ready => "Ready",
            Self::Loading => "Loading",
            Self::Refreshing => "Checking for updates",
            Self::Saving => "Saving",
            Self::Unsaved => "Not saved",
            Self::WaitingToSync => "Waiting to sync",
            Self::Offline => "Offline",
            Self::Stale => "Showing saved items",
            Self::Saved => "Saved",
            Self::Updated => "Up to date",
            Self::SaveFailed => "Could not save",
            Self::UpdateFailed => "Could not update",
        }
    }
}
impl From<kobo_state::draft::Status<'_>> for Status {
    fn from(status: kobo_state::draft::Status<'_>) -> Self {
        match status {
            kobo_state::draft::Status::Saved => Self::Saved,
            kobo_state::draft::Status::Unsaved => Self::Unsaved,
            kobo_state::draft::Status::Saving => Self::Saving,
            kobo_state::draft::Status::Failed(_) => Self::SaveFailed,
        }
    }
}
impl ScreenBuilder {
    /// Keep this slot present across loading, content and recovery transitions.
    /// Use `Saved` only after local acknowledgement, and `Updated` only after
    /// validating the response and acknowledging its local durable state.
    #[must_use]
    pub fn inline_status(self, status: Status) -> Self {
        self.secondary(status.label())
    }
}

#[cfg(all(test, feature = "text"))]
mod tests {
    use super::*;
    use crate::{action_id, Chrome, DisplayMetrics};
    use kobo_ui::{Orientation, TextScale};

    #[test]
    fn inline_feedback_does_not_move_content_or_controls_when_its_state_changes() {
        let statuses = [
            Status::Ready,
            Status::Loading,
            Status::Refreshing,
            Status::Saving,
            Status::Unsaved,
            Status::WaitingToSync,
            Status::Offline,
            Status::Stale,
            Status::Saved,
            Status::Updated,
            Status::SaveFailed,
            Status::UpdateFailed,
        ];
        for profile in kobo_profile::SUPPORTED_PROFILES {
            for scale in [TextScale::Default, TextScale::Large, TextScale::ExtraLarge] {
                for orientation in [Orientation::Portrait, Orientation::Landscape] {
                    let metrics = DisplayMetrics {
                        width: i32::try_from(profile.width).unwrap(),
                        height: i32::try_from(profile.height).unwrap(),
                        pixels_per_inch: i32::from(profile.pixels_per_inch),
                        text_scale: scale,
                    }
                    .oriented(orientation);
                    kobo_text::install(metrics).unwrap();
                    let mut previous = None;
                    for status in statuses {
                        let chrome = Chrome::with_back(true);
                        let screen = ScreenBuilder::new("feedback")
                            .top_bar("Notes")
                            .inline_status(status)
                            .text("The note stays available while its status changes.")
                            .button("open", "Open note")
                            .bottom_action("details", "Details")
                            .build_checked_with(&metrics, &chrome)
                            .unwrap();
                        let layout = screen.layout_with(&metrics, &chrome);
                        let targets = ["open", "details"].map(|name| {
                            layout
                                .nodes
                                .iter()
                                .find(|node| node.kind.acts_on() == Some(action_id(name)))
                                .unwrap()
                                .rect
                        });
                        if let Some(previous) = previous {
                            assert_eq!(targets, previous);
                        }
                        previous = Some(targets);
                    }
                }
            }
        }
    }

    #[test]
    fn a_write_in_flight_or_failed_is_never_presented_as_saved() {
        use kobo_state::draft::Draft;
        let mut draft = Draft::restored(b"old".to_vec(), 64).unwrap();
        draft.replace(b"new".to_vec()).unwrap();
        assert_eq!(Status::from(draft.status()), Status::Unsaved);
        let write = draft.begin().unwrap();
        assert_eq!(Status::from(draft.status()), Status::Saving);
        draft.finish(write.revision, Err("Storage full".into()));
        assert_eq!(Status::from(draft.status()), Status::SaveFailed);
        assert_eq!(draft.bytes(), b"new");
    }
}

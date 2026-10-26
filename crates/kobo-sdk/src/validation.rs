//! Validate at the metrics and chrome that will actually display the screen.
use crate::{Chrome, DiagnosticSeverity, DisplayMetrics, LayoutIssue, Screen, ScreenBuilder};

impl ScreenBuilder {
    /// Build with the runtime's layout diagnostics, including text overflow,
    /// hidden content and unreachable or undersized controls. Picture arrival
    /// is asynchronous, so missing cache handles are checked by the host later.
    ///
    /// Pass logical landscape metrics for a landscape screen, and use the same
    /// chrome as rendering. Collection truncation remains an error even when
    /// the truncated screen happens to fit. Nonblocking style warnings remain
    /// available through `Screen::diagnostics` on a successful screen.
    ///
    /// # Errors
    /// Returns collection warnings and layout diagnostics when content was
    /// omitted or any diagnostic has error severity.
    pub fn build_checked_with(
        mut self,
        metrics: &DisplayMetrics,
        chrome: &Chrome,
    ) -> Result<Screen, Vec<LayoutIssue>> {
        let mut issues = std::mem::take(&mut self.warnings);
        let omitted = !issues.is_empty();
        let screen = self.build();
        issues.extend(screen.diagnostics(metrics, chrome).issues);
        if omitted
            || issues
                .iter()
                .any(|issue| issue.severity == DiagnosticSeverity::Error)
        {
            Err(issues)
        } else {
            Ok(screen)
        }
    }
}

#[cfg(all(test, feature = "text"))]
mod tests {
    use super::*;
    use crate::Glyph;
    use kobo_ui::TextScale;

    #[test]
    fn small_valid_collection_can_still_overflow_the_actual_panel() {
        let metrics = crate::CLARA_BW_METRICS;
        kobo_text::install(metrics).unwrap();
        let make = || {
            ScreenBuilder::new("overflow")
                .top_bar("Notes")
                .text("A paragraph that needs several lines on the reader. ".repeat(100))
                .button("keep", "Keep note")
        };
        assert!(make().build_checked().is_ok());
        let issues = make()
            .build_checked_with(&metrics, &Chrome::with_back(true))
            .unwrap_err();
        assert!(issues
            .iter()
            .any(|issue| issue.severity == DiagnosticSeverity::Error));
        let rows = ScreenBuilder::new("rows")
            .rows((0..100).map(|n| (format!("row-{n}"), "Note", "", Glyph::Book)));
        assert!(rows
            .build_checked_with(&metrics, &Chrome::with_back(true))
            .is_err());
    }

    #[test]
    fn shared_metrics_accept_reachable_screens_in_both_orientations_and_all_sizes() {
        for profile in kobo_profile::SUPPORTED_PROFILES {
            for scale in [TextScale::Default, TextScale::Large, TextScale::ExtraLarge] {
                for orientation in [crate::Orientation::Portrait, crate::Orientation::Landscape] {
                    let metrics = DisplayMetrics {
                        width: i32::try_from(profile.width).unwrap(),
                        height: i32::try_from(profile.height).unwrap(),
                        pixels_per_inch: i32::from(profile.pixels_per_inch),
                        text_scale: scale,
                    }
                    .oriented(orientation);
                    kobo_text::install(metrics).unwrap();
                    let screen = ScreenBuilder::new("note")
                        .top_bar("Note")
                        .owns_back(true)
                        .secondary("Saved")
                        .text("Pick up bread on the way home.")
                        .bottom_action("edit", "Edit note")
                        .build_checked_with(&metrics, &Chrome::with_back(true))
                        .unwrap();
                    let diagnostics = screen.diagnostics(&metrics, &Chrome::with_back(true));
                    assert!(diagnostics
                        .layout
                        .nodes
                        .iter()
                        .any(|node| node.kind.acts_on() == Some(crate::action_id("edit"))));
                }
            }
        }
    }
}

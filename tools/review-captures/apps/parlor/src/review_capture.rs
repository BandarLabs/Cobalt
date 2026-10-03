//! Actual renderer snapshots, not interactive simulator captures.
use super::*;
#[path = "../screenshots/ui-review/render.rs"]
mod render_capture;

#[test]
#[ignore]
fn capture_review_screens() {
    let _runner = kobo_sdk::AppRunner::new(Parlor::default());
    let mut app = Parlor::default();
    app.start(Title::Kalah);
    app.handle_action(action_id("record"));
    render_capture::capture("parlor", "record", &app.screen());
    app.handle_action(ActionId::BACK);
    render_capture::capture("parlor", "returned", &app.screen());
    app.record = (0..40)
        .map(|index| {
            format!(
                "{} d3 flips 1",
                if index % 2 == 0 { "Black" } else { "White" }
            )
        })
        .collect();
    app.handle_action(action_id("record"));
    render_capture::capture("parlor", "long-record", &app.screen());
    app.record_page = app.record_pages(&Context::default()).len() - 1;
    render_capture::capture("parlor", "long-record-last", &app.screen());
}

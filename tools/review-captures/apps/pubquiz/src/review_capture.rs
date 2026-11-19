//! Opt-in real renderer snapshots, not interactive simulator captures.
use super::*;
use kobo_ui::{Chrome, TextScale, CLARA_BW_METRICS};
#[path = "../screenshots/ui-review/render.rs"]
mod render_capture;

#[test]
#[ignore]
fn capture_review_screens() {
    for (name, text_scale) in [("default", TextScale::Default), ("large", TextScale::Large)] {
        let metrics = kobo_ui::DisplayMetrics {
            text_scale,
            ..CLARA_BW_METRICS
        };
        let mut runner = kobo_sdk::AppRunner::with_metrics(Quiz::default(), metrics);
        runner.start();
        runner.action(action_id("party"));
        let context = runner.context();
        let category = setup_categories(runner.app())
            .iter()
            .position(|name| name == "Geography")
            .unwrap();
        let page = setup_pages(runner.app(), &context)
            .iter()
            .position(|rows| rows.contains(&(category + 1)))
            .unwrap();
        for _ in 0..page {
            runner.page_turn(true);
        }
        let screen = setup_screen(runner.app(), &context);
        render_capture::capture_at("pubquiz", &format!("{name}-setup"), &screen, metrics, &());
        let action = action_id(&format!("cat-{category}"));
        let layout = screen.layout_with(&metrics, &Chrome::measuring(true));
        let rect = layout.rect_of_action(action).unwrap();
        let hit = layout
            .hit_test(rect.x + rect.width / 2, rect.y + rect.height / 2)
            .unwrap();
        runner.action(hit);
        assert_eq!(runner.app().setup_category.as_deref(), Some("Geography"));
        render_capture::capture_at(
            "pubquiz",
            &format!("{name}-geography-selected"),
            &setup_screen(runner.app(), &context),
            metrics,
            &(),
        );
    }
    let mut runner = kobo_sdk::AppRunner::new(Quiz::default());
    runner.start();
    runner.action(action_id("party"));
    runner.action(action_id("continue-setup"));
    runner.action(action_id("rename-0"));
    runner.action(ActionId::BACK);
    render_capture::capture(
        "pubquiz",
        "rename-cancelled",
        &screen_with(runner.app(), &runner.context()),
    );
    runner.action(ActionId::BACK);
    render_capture::capture(
        "pubquiz",
        "players-back-to-setup",
        &screen_with(runner.app(), &runner.context()),
    );
}

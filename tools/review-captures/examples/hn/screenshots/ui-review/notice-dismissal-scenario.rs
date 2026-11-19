#[test]
fn native_review_snapshots() {
    for (name, text_scale) in [("default", kobo_ui::TextScale::Default), ("largest", kobo_ui::TextScale::Largest)] {
        let metrics = kobo_sdk::DisplayMetrics {text_scale, ..kobo_sdk::CLARA_BW_METRICS};
        let mut runner = AppRunner::with_metrics(Hn::default(), metrics);
        runner.start();
        answer_list(&mut runner);
        runner.app_mut().tab = Tab::Saved;
        runner.app_mut().problem = Some("Saved articles remain available offline.".into());
        let mut context = runner.context();
        runner.app_mut().repaginate_saved(&context);
        runner.action(action_id("saved-menu-0"));
        runner.app().show(&mut context);
        crate::audit_context(&context, &format!("{name}-notice-open-menu"));
        // Deliver the same native Back callback on both app versions. This
        // isolates app dismissal semantics from runtime routing, which PR237
        // handles separately for overlays on an unowned root screen.
        runner.action(ActionId::BACK);
        runner.app().show(&mut context);
        crate::audit_context(&context, &format!("{name}-notice-dismissed-menu"));
    }
}

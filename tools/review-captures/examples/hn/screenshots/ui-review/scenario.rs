#[test]
fn native_review_snapshots() {
    for (name,text_scale) in [("default",kobo_ui::TextScale::Default),("largest",kobo_ui::TextScale::Largest)] {
        let metrics=kobo_sdk::DisplayMetrics{text_scale,..kobo_sdk::CLARA_BW_METRICS};
        let mut runner=AppRunner::with_metrics(Hn::default(),metrics);runner.start();answer_list(&mut runner);
        let mut context=runner.context();runner.app().show(&mut context);crate::audit_context(&context,&format!("{name}-stories"));
        let index=runner.app().stories.iter().position(|s|s.id==THREAD_STORY.to_string()).unwrap();runner.action(action_id(&format!("story-{index}")));answer_lanes(&mut runner,&fixture(THREAD));
        runner.app().show(&mut context);crate::audit_context(&context,&format!("{name}-discussion"));
        runner.action(ActionId::BACK);runner.app_mut().tab=Tab::Saved;runner.app_mut().page=0;runner.app_mut().menu=Some(0);runner.app_mut().repaginate_saved(&context);runner.app().show(&mut context);crate::audit_context(&context,&format!("{name}-saved-menu"));
        runner.action(ActionId::BACK);runner.app().show(&mut context);crate::audit_context(&context,&format!("{name}-dismissed-menu"));
        runner.app_mut().problem=Some("The site did not answer. Saved stories are still available.".into());runner.app().show(&mut context);crate::audit_context(&context,&format!("{name}-saved-error"));
    }
}

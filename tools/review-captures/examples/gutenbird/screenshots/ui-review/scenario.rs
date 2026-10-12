#[test]
fn native_review_snapshots() {
    for (name,text_scale) in [("default",TextScale::Default),("largest",TextScale::Largest)] {
        let metrics=DisplayMetrics{text_scale,..CLARA_BW_METRICS};let mut context=AppRunner::with_metrics(Gutenbird::default(),metrics).context();
        let mut app=Gutenbird::default();app.view=View::Catalogs;app.show(&mut context);crate::audit_context(&context,&format!("{name}-catalogs"));
        app.catalogs=(0..20).map(|n|Catalog::new(format!("Library {}",n+1),format!("https://library-{n}.example/catalog"),true)).collect();app.show(&mut context);crate::audit_context(&context,&format!("{name}-twenty-catalogs"));
        let mut runner=AppRunner::with_metrics(Gutenbird {task:Some((TaskId(9),Awaiting::Feed(FeedPurpose::Root{catalog:0},BASE.into()))),..Gutenbird::default()},metrics);
        runner.task_outcome(TaskId(9),TaskOutcome::Completed(two_publication_feed_json().into_bytes()));runner.app().show(&mut context);crate::audit_context(&context,&format!("{name}-shelf"));
        runner.action(action_id("book-0"));runner.app().show(&mut context);crate::audit_context(&context,&format!("{name}-book-details"));
        runner.action(kobo_sdk::ActionId::BACK);runner.app_mut().problem=Some("The catalog could not be refreshed. Showing the saved page.".into());runner.app().show(&mut context);crate::audit_context(&context,&format!("{name}-saved-shelf"));
    }
}

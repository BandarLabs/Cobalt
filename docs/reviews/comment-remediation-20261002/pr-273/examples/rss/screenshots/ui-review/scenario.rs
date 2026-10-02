#[test]
fn native_review_snapshots() {
    for (name,text_scale) in [("default",kobo_ui::TextScale::Default),("largest",kobo_ui::TextScale::Largest)] {
        let metrics=kobo_sdk::DisplayMetrics{text_scale,..kobo_sdk::CLARA_BW_METRICS};let mut context=kobo_sdk::AppRunner::with_metrics(Feeds::default(),metrics).context();
        let mut app=Feeds{loaded:true,..Feeds::default()};app.show(&mut context);audit_context(&context,&format!("{name}-empty"));
        app.on_action(&mut context,action_id("add"));audit_context(&context,&format!("{name}-add"));app.view=View::Starters;app.show(&mut context);audit_context(&context,&format!("{name}-browse"));
        app.subscriptions=(0..12).map(|n|Subscription{url:format!("https://example.org/feed-{n}.xml"),title:format!("The Field Journal {n}"),site:"example.org".into()}).collect();app.view=View::Shelf;app.show(&mut context);audit_context(&context,&format!("{name}-subscriptions"));
        app.problem=Some("Subscriptions could not be saved. Retry saving before closing.".into());app.subscription_save.failed=true;app.show(&mut context);audit_context(&context,&format!("{name}-save-error"));
        app.problem=None;app.subscription_save.failed=false;app.open=Some(0);app.items=(0..20).map(|n|feed::Item{title:format!("Article {n}: A morning beside the river"),link:format!("https://example.org/{n}"),stamp:"2026-10-01".into(),author:"The Field Journal".into(),body:"The first light reaches the bridge before the town wakes.".into(),html:String::new()}).collect();app.view=View::Items;app.show(&mut context);audit_context(&context,&format!("{name}-articles"));
        app.problem=Some("The feed could not be refreshed. Saved articles are still available.".into());app.show(&mut context);audit_context(&context,&format!("{name}-refresh-error"));
    }
}

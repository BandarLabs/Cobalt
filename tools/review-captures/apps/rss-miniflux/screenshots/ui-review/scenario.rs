#[test]
fn native_review_snapshots() {
    for (name, text_scale) in [("default", kobo_ui::TextScale::Default), ("largest", kobo_ui::TextScale::Largest)] {
        let metrics = kobo_sdk::DisplayMetrics { text_scale, ..kobo_sdk::CLARA_BW_METRICS };
        let mut context = kobo_sdk::AppRunner::with_metrics(Reader::default(), metrics).context();
        let mut app = Reader::default();
        app.show(&mut context); audit_context(&context,&format!("{name}-setup"));
        app.on_action(&mut context, action_id("settings")); audit_context(&context,&format!("{name}-settings"));
        app.on_action(&mut context, action_id("directory")); audit_context(&context,&format!("{name}-directory"));
        app.on_action(&mut context, ActionId::BACK); audit_context(&context,&format!("{name}-back-from-directory"));
        app.server="https://miniflux.example".into();app.credential="miniflux".into();app.view=Some(View::Shelf);
        app.articles=(0..20).map(|index|Article {id:index+1,title:format!("A morning by the river, part {}",index+1),feed:"The Field Journal".into(),content:"<p>The river is quiet.</p>".into(),starred:false,url:format!("https://example.org/{index}"),status:Status::Unread}).collect();
        app.show(&mut context);audit_context(&context,&format!("{name}-unread"));
        app.on_action(&mut context,action_id("next"));audit_context(&context,&format!("{name}-next-page"));
        app.on_action(&mut context,action_id("settings"));app.on_action(&mut context,ActionId::BACK);audit_context(&context,&format!("{name}-back-from-settings"));
    }
}

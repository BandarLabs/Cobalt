#[test]
fn native_review_snapshots() {
    use kobo_sdk::KoboApp;
    for (name,text_scale) in [("default",kobo_ui::TextScale::Default),("largest",kobo_ui::TextScale::Largest)] {
        let metrics=kobo_sdk::DisplayMetrics{text_scale,..kobo_sdk::CLARA_BW_METRICS};
        let mut context=AppRunner::with_metrics(Arxiv::default(),metrics).context();
        let mut app=Arxiv::default();app.show(&mut context);crate::audit_context(&context,&format!("{name}-subjects"));
        app.trouble=Some("The saved searches could not be written. Try saving again.".into());app.show(&mut context);crate::audit_context(&context,&format!("{name}-subjects-error"));
        app.trouble=None;app.papers=(0..25).map(|n|{let mut p=paper();p.id=format!("2609.{n:05}");p.title=format!("Paper {}: Attention reconsidered in a study of language models",n+1);p}).collect();app.total=25;app.view=super::View::Listing;
        app.show(&mut context);crate::audit_context(&context,&format!("{name}-listing"));
        app.trouble=Some("The archive could not be reached. Showing the downloaded results.".into());app.show(&mut context);crate::audit_context(&context,&format!("{name}-listing-error"));
        app.trouble=None;app.on_action(&mut context,action_id("paper-0"));crate::audit_context(&context,&format!("{name}-abstract"));
        app.trouble=Some("This paper could not be saved. Try keeping it again.".into());app.show(&mut context);crate::audit_context(&context,&format!("{name}-abstract-error"));
        app.on_action(&mut context,action_id(super::READ_NEXT));crate::audit_context(&context,&format!("{name}-abstract-error-next"));app.on_action(&mut context,action_id(super::READ_BACK));
        app.view=super::View::Library;app.library=(0..20).map(|n|super::Kept{id:format!("2609.{n:05}"),title:format!("Paper {n}: Attention reconsidered"),authors:"Ada Lovelace, Alan Turing".into(),bytes:4096,..super::Kept::default()}).collect();app.show(&mut context);crate::audit_context(&context,&format!("{name}-library-error"));
    }
}

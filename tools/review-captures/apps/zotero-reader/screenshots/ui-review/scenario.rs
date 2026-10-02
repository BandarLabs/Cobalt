#[test]
fn native_review_snapshots() {
    for (name, text_scale) in [("default", kobo_ui::TextScale::Default), ("largest", kobo_ui::TextScale::Largest)] {
        let metrics = kobo_sdk::DisplayMetrics { text_scale, ..kobo_sdk::CLARA_BW_METRICS };
        let mut context = kobo_sdk::AppRunner::with_metrics(ReadingList::default(), metrics).context();
        let mut app = ReadingList::default();
        app.show(&mut context); audit_context(&context,&format!("{name}-setup"));
        app.trouble=Some("The Zotero user ID must contain only digits and is not your username.".into()); app.show(&mut context); audit_context(&context,&format!("{name}-setup-error")); app.trouble=None;
        app.view=View::Collections;app.collections=(0..20).map(|n|Collection{key:format!("COLL{n:04}"),name:format!("Research collection {}",n+1)}).collect();
        app.show(&mut context);audit_context(&context,&format!("{name}-collections"));
        app.trouble=Some("Zotero could not be reached. Cached papers are still available.".into());
        app.show(&mut context);audit_context(&context,&format!("{name}-collections-error"));
        app.view=View::Feed;app.trouble=None;app.selected=app.collections.first().cloned();
        app.snapshot.papers=(0..20).map(|n|model::Paper{key:format!("PAPER{n:04}"),title:format!("Research paper {}: A detailed study of the river and its surrounding landscapes, with measurements gathered over several seasons",n+1),creators:"Ada Lovelace, Alan Turing, Grace Hopper".into(),year:"2026".into(),..model::Paper::default()}).collect();
        app.filtered=(0..20).collect();app.show(&mut context);audit_context(&context,&format!("{name}-papers"));
        app.last_opened=Some("PAPER0000".into());app.show(&mut context);audit_context(&context,&format!("{name}-papers-last-opened"));
        app.trouble=Some("Zotero could not be reached. Cached papers are still available.".into());app.show(&mut context);audit_context(&context,&format!("{name}-papers-error"));
        app.library=app.snapshot.papers.iter().map(|paper| Kept {key:paper.key.clone(),title:paper.title.clone(),creators:paper.creators.clone(),bytes:4096}).collect();
        app.view=View::Library;app.pending_memory=Some(("memory.test".into(),vec![1]));app.show(&mut context);audit_context(&context,&format!("{name}-offline-error"));
        app.view=View::Feed;app.pending_memory=None;
        app.trouble=None;app.keyboard=Keyboard::with_text("missing query");app.apply_search();app.show(&mut context);audit_context(&context,&format!("{name}-no-results"));
        app.on_action(&mut context, action_id(SEARCH));audit_context(&context,&format!("{name}-edit-query"));
        app.view=View::Detail;app.opened_title="A study of river measurements".into();app.opened_key=Some("PAPER0000".into());
        app.detail=Some(model::Detail{paper:model::Paper{key:"PAPER0000".into(),title:app.opened_title.clone(),creators:"Ada Lovelace".into(),year:"2026".into(),has_pdf:true,..model::Paper::default()},abstract_text:"The researchers measured the river throughout the year, comparing rainfall and flow with the state of the surrounding fields. ".repeat(60),..model::Detail::default()});
        app.task=Some((TaskId(800),Awaiting::DetailChildren));
        app.on_task(&mut context,TaskId(800),TaskOutcome::Completed(include_bytes!("APP_FIXTURES/children.json").to_vec()));
        audit_context(&context,&format!("{name}-detail"));
        app.trouble=Some("Zotero could not be reached. Try opening the text again.".into());app.show(&mut context);audit_context(&context,&format!("{name}-detail-error"));

    }
}

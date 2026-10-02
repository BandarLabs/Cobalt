#[test]
fn capture_review_pages_when_requested() {
    let Ok(output) = std::env::var("COBALT_REVIEW_OUT") else {
        return;
    };
    let output = std::path::PathBuf::from(output);
    std::fs::create_dir_all(&output).unwrap();
    let runner = AppRunner::new(Post::default());
    let mut context = runner.context();
    let mut app = post();
    // Match the original before-render scenario, including the long subject.
    app.letters[0].title = "Letter 0: a rather long title about this morning's plans".into();
    app.letters[0].body = "Tea and toast are ready. ".repeat(120);
    let mut screens = vec![("long-letter-first", app.letter(&mut context))];
    let pages = app.letter_pages(&context, &app.letters[0]);
    let last: usize = pages
        .iter()
        .take(pages.len() - 1)
        .map(|page| page_words(page))
        .sum();
    app.set_place("long", last);
    screens.push(("long-letter-last", app.letter(&mut context)));
    app.set_place("long", 0);
    app.notice = Some("The Hermes gateway is offline. Your reply is still queued and will be sent when the connection returns.".into());
    app.outbox
        .push(Reply::new(&app.gateway, "long", "Thank you.").unwrap());
    screens.push(("letter-queued-offline", app.letter(&mut context)));
    for (name, screen) in screens {
        let chrome = Chrome::for_screen(&screen, false, Chrome::measuring(true).status);
        let screen = kobo_ui::ensure_way_back(screen, &chrome, "Post");
        let mut surface = kobo_ui::Surface::new(1072, 1448);
        kobo_ui::render_all(
            &screen,
            &CLARA_BW_METRICS,
            &chrome,
            &kobo_ui::PictureCache::default(),
            &mut surface,
            None,
        );
        let png = kobo_image::encode_png_grey(1072, 1448, &surface.pixels).unwrap();
        std::fs::write(output.join(format!("{name}.png")), png).unwrap();
        std::fs::write(
            output.join(format!("{name}.diagnostics.txt")),
            format!("{:#?}", screen.diagnostics(&CLARA_BW_METRICS, &chrome)),
        )
        .unwrap();
    }
}

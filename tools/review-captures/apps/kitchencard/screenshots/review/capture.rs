#[test]
fn capture_review_pages_when_requested() {
    let Ok(output) = std::env::var("COBALT_REVIEW_OUT") else {
        return;
    };
    let output = std::path::PathBuf::from(output);
    std::fs::create_dir_all(&output).unwrap();
    let runner = AppRunner::new(Kitchen::default());
    let context = runner.context();
    let mut app = kitchen();
    app.view = Some(View::Browse);
    let mut screens = vec![("recipes-first", app.screen(&context))];
    app.browse_page = app.browse_pages(&context).len() - 1;
    screens.push(("recipes-last", app.screen(&context)));
    app.view = Some(View::Ingredients);
    screens.push(("ingredients-first", app.screen(&context)));
    app.ingredients_page = app.ingredients_pages(&context).len() - 1;
    screens.push(("ingredients-last", app.screen(&context)));
    app.step = 1;
    app.on_action(&mut Context::default(), action_id("cook"));
    screens.push(("return-from-ingredients", app.screen(&context)));
    for (name, screen) in screens {
        let chrome = Chrome::for_screen(&screen, false, Chrome::measuring(true).status);
        let screen = kobo_ui::ensure_way_back(screen, &chrome, "Kitchen Card");
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

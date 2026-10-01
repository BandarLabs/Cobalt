#[test]
fn capture_review_pages_when_requested() {
    let Ok(output) = std::env::var("COBALT_REVIEW_OUT") else {
        return;
    };
    let output = std::path::PathBuf::from(output);
    std::fs::create_dir_all(&output).unwrap();
    let runner = AppRunner::new(HomePanel::default());
    let context = runner.context();
    let mut app = HomePanel {
        view: View::Add,
        entities: entities(20),
        ..HomePanel::default()
    };
    let mut screens = vec![("picker-first", app.add(&context))];
    app.picker_page = app.picker_pages(&context).len() - 1;
    screens.push(("picker-last", app.add(&context)));
    app.view = View::Grid;
    app.tiles = (0..12).map(|n| format!("light.room_{n:02}")).collect();
    app.wall = true;
    screens.push(("wall-first", app.grid(&context)));
    app.grid_page = app.grid_pages(&context).len() - 1;
    screens.push(("wall-last", app.grid(&context)));
    for (name, screen) in screens {
        let chrome = Chrome::for_screen(&screen, false, Chrome::measuring(true).status);
        let screen = kobo_ui::ensure_way_back(screen, &chrome, "Home Panel");
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

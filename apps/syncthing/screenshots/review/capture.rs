#[test]
fn capture_review_pages_when_requested() {
    let Ok(output) = std::env::var("COBALT_REVIEW_OUT") else {
        return;
    };
    let output = std::path::PathBuf::from(output);
    std::fs::create_dir_all(&output).unwrap();
    let mut runner = AppRunner::new(Sync::default());
    let opened = runner.action(action_id("setup"));
    let guide = screen(&opened).clone();
    let returned = runner.action(ActionId::BACK);
    let status = screen(&returned).clone();
    let opened = runner.action(action_id("folders"));
    let folders = screen(&opened).clone();
    let opened = runner.action(action_id("about"));
    let about = screen(&opened).clone();
    std::fs::write(
        output.join("back-ownership.json"),
        format!(
            "{{\"guide\":{},\"status_after_back\":{},\"sync_enabled\":{}}}\n",
            guide.owns_back,
            status.owns_back,
            runner.app().config.enabled
        ),
    )
    .unwrap();
    for (name, screen) in [
        ("guide", guide),
        ("folders", folders),
        ("about", about),
        ("status-after-back", status),
    ] {
        let chrome = Chrome::for_screen(&screen, false, Chrome::measuring(true).status);
        let screen = kobo_ui::ensure_way_back(screen, &chrome, "Sync");
        let issues = screen.diagnostics(&CLARA_BW_METRICS, &chrome).issues;
        assert!(issues.is_empty(), "{name}: {issues:#?}");
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

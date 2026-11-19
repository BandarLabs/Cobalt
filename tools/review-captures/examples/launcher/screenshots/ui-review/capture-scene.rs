#[test]
fn capture() {
    for scale in [kobo_ui::TextScale::Default, kobo_ui::TextScale::Largest] {
        let metrics = DisplayMetrics {
            text_scale: scale,
            ..CLARA_BW_METRICS
        };
        let app = Launcher::default();
        save_capture(
            &format!("{}-terminal-start", scale.percent()),
            &app.starting(3),
            metrics,
            true,
        );
    }
}

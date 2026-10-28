#[test]
fn capture() {
    for scale in [kobo_ui::TextScale::Default, kobo_ui::TextScale::Largest] {
        let metrics = DisplayMetrics {
            text_scale: scale,
            ..CLARA_BW_METRICS
        };
        let app = Chat {
            view: View::Choosing,
            ..Chat::default()
        };
        save_capture(
            &format!("{}-service", scale.percent()),
            &app.choosing(),
            metrics,
            false,
        );
    }
}

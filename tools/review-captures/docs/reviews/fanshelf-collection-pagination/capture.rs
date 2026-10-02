use super::*;
use crate::collection_tests::{collection_app, emitted, metrics, turn, COLLECTIONS};
use kobo_sdk::AppRunner;
use kobo_ui::{Chrome, DisplayMetrics};

fn capture(name: &str, original: Screen, metrics: DisplayMetrics) {
    let Ok(root) = std::env::var("COBALT_REVIEW_CAPTURE_DIR") else {
        return;
    };
    let font = kobo_text::install(metrics).expect("real font");
    let chrome = Chrome::for_screen(&original, false, Chrome::measuring(true).status);
    let screen = kobo_ui::ensure_way_back(
        original,
        &chrome,
        env!("CARGO_PKG_NAME").trim_start_matches("kobo-"),
    );
    let mut surface = kobo_ui::Surface::new(
        usize::try_from(metrics.width).unwrap(),
        usize::try_from(metrics.height).unwrap(),
    );
    kobo_ui::render_with(&screen, &metrics, &chrome, &mut surface, None);
    let png = kobo_image::encode_png_grey(
        u32::try_from(metrics.width).unwrap(),
        u32::try_from(metrics.height).unwrap(),
        &surface.pixels,
    )
    .unwrap();
    let directory =
        std::path::Path::new(&root).join(env!("CARGO_PKG_NAME").trim_start_matches("kobo-"));
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join(format!("{name}-{}.png", metrics.text_scale.percent())),
        png,
    )
    .unwrap();
    let evidence = format!("Capture: genuine app screen builder + Cobalt renderer; NOT a live simulator capture.\nScenario: {name}\nSource base: 97153048\nShell: synthetic measuring status strip, runtime ensure_way_back\nFont: {font:?}\nFont roles: {:?}\nMetrics: {metrics:?}\nScreen: {screen:#?}\nLayout: {:#?}\nDiagnostics: {:#?}\n", kobo_text::installed_sources(), screen.layout_with(&metrics, &chrome), screen.diagnostics(&metrics, &chrome));
    std::fs::write(
        directory.join(format!("{name}-{}.txt", metrics.text_scale.percent())),
        evidence,
    )
    .unwrap();
}
#[test]
fn capture_review_collections() {
    if std::env::var_os("COBALT_REVIEW_CAPTURE_DIR").is_none() {
        return;
    }
    for metrics in metrics() {
        for (name, view) in COLLECTIONS {
            let mut app = collection_app();
            app.view = view;
            let mut runner = AppRunner::with_metrics(app, metrics);
            let mut screen = emitted(runner.start()).unwrap();
            capture(name, screen.clone(), metrics);
            turn(&mut runner, &mut screen, action_id("page-next"));
            capture(&format!("{name}-next"), screen, metrics);
        }
    }
}

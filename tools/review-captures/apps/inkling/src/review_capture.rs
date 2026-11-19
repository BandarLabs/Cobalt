use super::*;
use kobo_sdk::AppRunner;
use kobo_ui::{Chrome, DisplayMetrics, TextScale, CLARA_BW_METRICS};

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
    let evidence = format!("Capture: genuine app screen builder + Cobalt renderer; NOT a live simulator capture.\nScenario: {name}\nApp source SHA-256: {}\nShell: synthetic measuring status strip, runtime ensure_way_back\nFont: {font:?}\nFont roles: {:?}\nMetrics: {metrics:?}\nScreen: {screen:#?}\nLayout: {:#?}\nDiagnostics: {:#?}\n", std::env::var("COBALT_REVIEW_SOURCE_SHA256").unwrap_or_default(), kobo_text::installed_sources(), screen.layout_with(&metrics, &chrome), screen.diagnostics(&metrics, &chrome));
    std::fs::write(
        directory.join(format!("{name}-{}.txt", metrics.text_scale.percent())),
        evidence,
    )
    .unwrap();
}
fn metrics() -> Vec<DisplayMetrics> {
    [
        TextScale::Default,
        TextScale::ExtraLarge,
        TextScale::Huge,
        TextScale::Largest,
    ]
    .into_iter()
    .map(|text_scale| DisplayMetrics {
        text_scale,
        ..std::env::var("COBALT_REVIEW_METRICS")
            .ok()
            .map(|value| {
                let parts: Vec<i32> = value.split(',').map(|n| n.parse().unwrap()).collect();
                DisplayMetrics {
                    width: parts[0],
                    height: parts[1],
                    pixels_per_inch: parts[2],
                    ..CLARA_BW_METRICS
                }
            })
            .unwrap_or(CLARA_BW_METRICS)
    })
    .collect()
}
#[test]
fn capture_review_baselines() {
    for metrics in metrics() {
        let mut runner = AppRunner::with_metrics(Game::for_day("2026-09-01"), metrics);
        runner.start();
        runner.store_result(StoreResult::Loaded {
            key: STATE.into(),
            value: None,
        });
        capture("board", runner.app().screen(), metrics);
        runner.action(action_id("how-to-play"));
        capture("help", runner.app().screen(), metrics);
        runner.action(action_id("close-help"));
        runner.action(action_id("enter"));
        runner.app_mut().keyboard = Keyboard::with_text("cra");
        capture("typing", runner.app().screen(), metrics);
        runner.action(ActionId::BACK);
        capture("typing-back", runner.app().screen(), metrics);
        runner.action(action_id("cancel"));
        runner.action(action_id("stats"));
        capture("stats", runner.app().screen(), metrics);
        runner.action(action_id("export"));
        capture("export-writing", runner.app().screen(), metrics);
        runner.store_result(StoreResult::Saved { key: EXPORT.into() });
        capture("export-saved", runner.app().screen(), metrics);
        runner.action(action_id("export"));
        runner.store_result(StoreResult::Denied(kobo_sdk::StoreError::TooFull));
        capture("export-failed", runner.app().screen(), metrics);
        runner.action(action_id("close-stats"));
        runner.action(action_id("archive"));
        capture("archive", runner.app().screen(), metrics);
        runner.action(action_id("earlier"));
        capture("archive-earlier", runner.app().screen(), metrics);
    }
}

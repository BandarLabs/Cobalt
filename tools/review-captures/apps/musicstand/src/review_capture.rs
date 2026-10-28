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
    let evidence = format!("Capture: genuine app screen builder + Cobalt renderer; NOT a live simulator capture.\nScenario: {name}\nSource base: 97153048\nShell: synthetic measuring status strip, runtime ensure_way_back\nFont: {font:?}\nFont roles: {:?}\nMetrics: {metrics:?}\nScreen: {screen:#?}\nLayout: {:#?}\nDiagnostics: {:#?}\n", kobo_text::installed_sources(), screen.layout_with(&metrics, &chrome), screen.diagnostics(&metrics, &chrome));
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
        TextScale::Largest,
    ]
    .into_iter()
    .map(|text_scale| DisplayMetrics {
        text_scale,
        ..CLARA_BW_METRICS
    })
    .collect()
}
#[test]
fn capture_review_baselines() {
    for metrics in metrics() {
        let mut runner = AppRunner::with_metrics(
            Stand {
                startup: Startup {
                    manifest_loaded: true,
                    state_loaded: true,
                    started: true,
                },
                ..Stand::default()
            },
            metrics,
        );
        capture("empty", runner.app().library(&runner.context()).0, metrics);
        runner.app_mut().scores = (0..12)
            .map(|i| Score {
                id: format!("score-{i}"),
                title: format!("Rehearsal score {}", i + 1),
                pages: 3,
                width: 1072,
                height: 1448,
            })
            .collect();
        capture(
            "library-twelve",
            runner.app().library(&runner.context()).0,
            metrics,
        );
        for _ in 0..12 {
            runner.action(action_id("list-next"));
        }
        capture(
            "library-last",
            runner.app().library(&runner.context()).0,
            metrics,
        );
        runner.action(action_id(SETLISTS));
        capture(
            "setlists-empty",
            runner.app().setlists(&runner.context()).0,
            metrics,
        );
        for _ in 0..12 {
            runner.action(action_id("new-list"));
        }
        capture(
            "setlists-twelve",
            runner.app().setlists(&runner.context()).0,
            metrics,
        );
        runner.action(action_id("list-0"));
        capture(
            "setlist-twelve",
            runner.app().setlist(0, &runner.context()).0,
            metrics,
        );
        for _ in 0..12 {
            runner.action(action_id("list-next"));
        }
        capture(
            "setlist-last",
            runner.app().setlist(0, &runner.context()).0,
            metrics,
        );
        runner.action(ActionId::BACK);
        capture(
            "setlist-back",
            runner.app().setlists(&runner.context()).0,
            metrics,
        );
        capture("transfer-help", Stand::about(), metrics);
    }
}

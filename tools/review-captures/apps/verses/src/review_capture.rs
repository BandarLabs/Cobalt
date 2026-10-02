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
        let mut runner = AppRunner::with_metrics(Verses::default(), metrics);
        capture("today", runner.app().screen(&runner.context()), metrics);
        runner.action(action_id("browse"));
        capture("browse", runner.app().screen(&runner.context()), metrics);
        runner.app_mut().saved.favorites = CORPUS.iter().map(|poem| poem.id.to_owned()).collect();
        capture(
            "browse-favorites",
            runner.app().screen(&runner.context()),
            metrics,
        );
        for _ in 0..20 {
            runner.action(action_id("list-next"));
        }
        capture(
            "browse-last",
            runner.app().screen(&runner.context()),
            metrics,
        );
        runner.action(action_id("search"));
        capture("search", runner.app().screen(&runner.context()), metrics);
        runner.app_mut().view = View::Results;
        runner.app_mut().results = (0..20)
            .map(|i| OnlinePoem {
                title: format!("Synthetic poem {}", i + 1),
                author: "Review Poet".into(),
                lines: vec!["A quiet day".into()],
                linecount: "1".into(),
            })
            .collect();
        capture(
            "results-twenty",
            runner.app().screen(&runner.context()),
            metrics,
        );
        for _ in 0..20 {
            runner.action(action_id("list-next"));
        }
        capture(
            "results-last",
            runner.app().screen(&runner.context()),
            metrics,
        );
        runner.app_mut().view = View::Online;
        runner.app_mut().online = Some(0);
        runner.app_mut().results[0].lines = (0..50)
            .map(|i| format!("This is line {} of the entire poem.", i + 1))
            .collect();
        capture(
            "online-long-poem",
            runner.app().screen(&runner.context()),
            metrics,
        );
        for _ in 0..50 {
            runner.action(action_id("online-next"));
        }
        capture(
            "online-poem-last",
            runner.app().screen(&runner.context()),
            metrics,
        );
        runner.action(action_id("more-by-author"));
        capture(
            "author-search",
            runner.app().screen(&runner.context()),
            metrics,
        );
        runner.action(ActionId::BACK);
        capture(
            "cancelled-search",
            runner.app().screen(&runner.context()),
            metrics,
        );
    }
}

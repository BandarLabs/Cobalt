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
            Audiobook {
                library: Some(Vec::new()),
                ..Audiobook::default()
            },
            metrics,
        );
        capture("empty", runner.app().screen(&runner.context()), metrics);
        runner.action(action_id(NEW));
        capture("compose", runner.app().screen(&runner.context()), metrics);
        runner.app_mut().stage = Stage::Library;
        runner.app_mut().shelf_unreadable = true;
        capture(
            "shelf-error",
            runner.app().screen(&runner.context()),
            metrics,
        );
        runner.app_mut().stage = Stage::Failed;
        runner
            .app_mut()
            .fail("The service could not be reached. Check Wi-Fi and try again.");
        capture(
            "creation-error",
            runner.app().screen(&runner.context()),
            metrics,
        );
    }
}

#[test]
fn capture_compose_recovery() {
    for metrics in metrics() {
        let mut runner = AppRunner::with_metrics(
            Audiobook {
                stage: Stage::Compose,
                library: Some(Vec::new()),
                checkpoint: Some(Checkpoint {
                    title: "A researched history of the Moon and the people who learned to understand it".into(),
                    parts: vec!["Narration part".into(); 8],
                    next_part: 2,
                    ..Checkpoint::default()
                }),
                ..Audiobook::default()
            },
            metrics,
        );
        capture(
            "compose-resume",
            runner.app().screen(&runner.context()),
            metrics,
        );
        runner.app_mut().hint = Some("That is too short. Add a few words.");
        capture(
            "compose-resume-hint",
            runner.app().screen(&runner.context()),
            metrics,
        );
        runner.app_mut().checkpoint = None;
        capture(
            "compose-hint",
            runner.app().screen(&runner.context()),
            metrics,
        );
    }
}

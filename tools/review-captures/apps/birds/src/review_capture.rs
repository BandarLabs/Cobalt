use super::*;
use kobo_sdk::AppRunner;
use kobo_ui::{Chrome, DisplayMetrics, TextScale, CLARA_BW_METRICS};

fn capture(name: &str, original: Screen, metrics: DisplayMetrics) {
    capture_with_pictures(name, original, metrics, &());
}
fn capture_with_pictures(
    name: &str,
    original: Screen,
    metrics: DisplayMetrics,
    pictures: &dyn kobo_ui::Pictures,
) {
    let Ok(root) = std::env::var("COBALT_REVIEW_CAPTURE_DIR") else {
        return;
    };
    let font = kobo_text::install(metrics).expect("real font");
    let chrome = Chrome::for_screen(&original, false, Chrome::measuring(true).status);
    let screen = kobo_ui::ensure_way_back(original, &chrome, "Birds");
    let mut surface = kobo_ui::Surface::new(
        usize::try_from(metrics.width).unwrap(),
        usize::try_from(metrics.height).unwrap(),
    );
    kobo_ui::render_all(&screen, &metrics, &chrome, pictures, &mut surface, None);
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
        let mut runner = AppRunner::with_metrics(Birds::default(), metrics);
        capture("empty", runner.app().screen(), metrics);
        runner.app_mut().notice=Some("The latest bird collage could not be read. Refresh after the companion sends a new snapshot.".into());
        capture("snapshot-error", runner.app().screen(), metrics);
        runner.app_mut().snapshot = Some(Snapshot {
            generated_at: unix_seconds(),
            source: "Synthetic Garden Station".into(),
            recent: vec!["American Robin".into()],
            image_checksum: None,
            image: None,
        });
        capture("missing-picture", runner.app().screen(), metrics);
        runner.action(action_id(MENU));
        capture("menu-error", runner.app().screen(), metrics);
    }
}

use super::refresh_tests::{accept_commands,loaded,settle_with_refresh};
use std::collections::VecDeque;

#[test]
fn capture_repeated_refresh_recovery() {
    for metrics in metrics() {
        let (mut runner, mut pictures) = loaded(metrics);
        let mut queue = VecDeque::new();
        accept_commands(runner.action(action_id(REFRESH)), &mut queue, &mut pictures);
        accept_commands(runner.action(action_id(REFRESH)), &mut queue, &mut pictures);
        settle_with_refresh(&mut runner, &mut queue, &mut pictures, true);
        runner.action(action_id(MENU));
        capture_with_pictures(
            "repeated-refresh",
            runner.app().screen(),
            metrics,
            &pictures,
        );
        eprintln!("Repeated Refresh result: {:?}", runner.app().notice);
    }
}

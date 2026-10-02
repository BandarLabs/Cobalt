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
    TextScale::STEPS.into_iter().map(|text_scale| DisplayMetrics {
        text_scale,
        ..CLARA_BW_METRICS
    }).collect()
}

fn collection_app() -> Fanshelf {
    let works = (0..18).map(|index| Work {
        id: (9000 + index).to_string(),
        title: format!("A Map Made of Starlight: The Journey Beyond the Northern Mountains {index:02}"),
        author: "Juniper Vale and Rowan Ink".into(),
        fandom: format!("The Long Road Home and Other Public Domain Adventures {index:02}"),
        chapters: 12,
        total_chapters: None,
        complete: false,
        download: DownloadState::Downloaded,
        last_checked: 1_789_617_600,
        ..Work::default()
    }).collect();
    let tags = (0..18).map(|index| FollowedTag {
        name: format!("The Long Road Home and Other Public Domain Adventures {index:02}"),
        slug: format!("PublicDomain{index}"),
    }).collect();
    let feed = (0..18).map(|index| FeedWork {
        id: (9100 + index).to_string(),
        title: format!("A Map Made of Starlight: The Journey Beyond the Northern Mountains {index:02}"),
        author: "Juniper Vale and Rowan Ink".into(),
        updated: "2026-09-02".into(),
    }).collect();
    Fanshelf {
        works, tags, feed,
        works_loaded: true,
        tags_loaded: true,
        demo: false,
        open_tag: Some(0),
        ..Fanshelf::default()
    }
}

#[test]
fn capture_review_collections() {
    for metrics in metrics() {
        for (name, view) in [
            ("shelf", View::Shelf), ("fandoms", View::Fandoms),
            ("follow", View::Follow), ("feed", View::Feed),
            ("updates", View::Updates),
        ] {
            let mut app = collection_app();
            app.view = view;
            let mut runner = AppRunner::with_metrics(app, metrics);
            runner.start();
            capture(name, runner.app().screen(), metrics);
            let screen = runner.app().screen();
            let chrome = Chrome::for_screen(&screen, false, Chrome::measuring(true).status);
            let issues = screen.diagnostics(&metrics, &chrome).issues;
            eprintln!("{name} {}%: {issues:?}", metrics.text_scale.percent());
            runner.action(action_id("page-next"));
            capture(&format!("{name}-next"), runner.app().screen(), metrics);
        }
    }
}

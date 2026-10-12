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
        let mut runner = AppRunner::with_metrics(Fieldbook::default(), metrics);
        capture(
            "empty-home",
            runner.app().screen(&runner.context()),
            metrics,
        );
        runner.app_mut().packs = shelf::Shelf::decode(include_bytes!(
            "../../../scripts/fixtures/fieldbook/packs.v1"
        ))
        .unwrap()
        .packs;
        capture("home", runner.app().screen(&runner.context()), metrics);
        for (name, view) in [
            ("packs", View::Packs),
            ("search", View::Search),
            ("life-empty", View::Life),
            ("export-empty", View::Export),
        ] {
            runner.app_mut().view = view;
            capture(name, runner.app().screen(&runner.context()), metrics);
        }
        runner.app_mut().outings = (0..6)
            .map(|i| Outing {
                id: i,
                location: format!("Morning walk {}", i + 1),
                date: "10/01/2026".into(),
                start: "07:00".into(),
            })
            .collect();
        runner.app_mut().view = View::Home;
        runner.app_mut().open_outing = Some(5);
        capture(
            "home-six-outings",
            runner.app().screen(&runner.context()),
            metrics,
        );
    }
}

#[test]
fn capture_populated_fieldbook_lists() {
    for metrics in metrics() {
        let mut runner = AppRunner::with_metrics(Fieldbook::default(), metrics);
        runner.app_mut().outings = vec![Outing {
            id: 1,
            location: "Review marsh".into(),
            date: "10/01/2026".into(),
            start: "07:00".into(),
        }];
        runner.app_mut().open_outing = Some(1);
        runner.app_mut().sightings = (0..18)
            .map(|i| Sighting {
                outing: 1,
                code: format!("bird-{i}"),
                common: format!("Review bird {}", i + 1),
                scientific: format!("Avis review {}", i + 1),
                count: 1,
            })
            .collect();
        runner.app_mut().view = View::Sightings;
        capture(
            "sightings-eighteen",
            runner.app().screen(&runner.context()),
            metrics,
        );
        for _ in 0..18 {
            runner.action(action_id("list-next"));
        }
        capture(
            "sightings-last",
            runner.app().screen(&runner.context()),
            metrics,
        );
        runner.action(action_id("sight-17"));
        capture(
            "delete-confirm",
            runner.app().screen(&runner.context()),
            metrics,
        );
        runner.action(action_id("delete-sighting"));
        capture(
            "delete-undo",
            runner.app().screen(&runner.context()),
            metrics,
        );
        runner.action(action_id("undo"));
        runner.app_mut().view = View::Life;
        capture(
            "life-eighteen",
            runner.app().screen(&runner.context()),
            metrics,
        );
        for _ in 0..18 {
            runner.action(action_id("list-next"));
        }
        capture("life-last", runner.app().screen(&runner.context()), metrics);
        let pack = shelf::Shelf::decode(include_bytes!(
            "../../../scripts/fixtures/fieldbook/packs.v1"
        ))
        .unwrap()
        .packs
        .remove(0);
        runner.app_mut().packs = (0..12)
            .map(|i| {
                let mut item = pack.clone();
                item.title = format!("Regional pack {}", i + 1);
                item
            })
            .collect();
        runner.app_mut().view = View::Packs;
        capture(
            "packs-twelve",
            runner.app().screen(&runner.context()),
            metrics,
        );
    }
}

#[test]
fn capture_manual_species() {
    for metrics in metrics() {
        let mut runner = AppRunner::with_metrics(
            Fieldbook {
                outings: vec![Outing {
                    id: 1,
                    location: "Review marsh".into(),
                    date: "10/01/2026".into(),
                    start: "07:00".into(),
                }],
                open_outing: Some(1),
                ..Fieldbook::default()
            },
            metrics,
        );
        for name in ["Robin", "Crow"] {
            runner.app_mut().query = name.into();
            runner.action(action_id("log-manual"));
        }
        runner.action(action_id("life"));
        capture(
            "manual-species",
            runner.app().screen(&runner.context()),
            metrics,
        );
    }
}

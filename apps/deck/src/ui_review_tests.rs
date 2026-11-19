//! In-process result-screen and callback coverage; no computer commands run here.
use super::*;
use kobo_sdk::{AppRunner, Command, DiagnosticSeverity};
use kobo_ui::{Chrome, DisplayMetrics, TextScale, CLARA_BW_METRICS};

fn result_app(tail: String) -> App {
    App {
        view: View::Result,
        address: "local".into(),
        result: Some((
            "Run tests".into(),
            RunResult {
                status: "failed".into(),
                exit: Some(1),
                tail,
            },
        )),
        last: Some("Run tests failed".into()),
        ..App::default()
    }
}

#[test]
fn every_output_word_and_page_fits_at_every_text_size() {
    let output = format!(
        "{}\nFinal output line.",
        "A test could not complete. Inspect the failing assertion and try again.\n".repeat(25)
    );
    for text_scale in TextScale::STEPS {
        let metrics = DisplayMetrics {
            text_scale,
            ..CLARA_BW_METRICS
        };
        let runner = AppRunner::with_metrics(App::default(), metrics);
        let context = runner.context();
        let mut app = result_app(output.clone());
        let pages = app.result_pages(&context);
        assert!(pages.len() > 1);
        let words = pages
            .iter()
            .flatten()
            .flat_map(|paragraph| paragraph.split_whitespace())
            .collect::<Vec<_>>();
        assert_eq!(words, output.split_whitespace().collect::<Vec<_>>());
        for page in 0..pages.len() {
            app.result_page = page;
            let screen = app.result_screen(&context);
            assert!(screen.owns_back);
            let errors = screen
                .diagnostics(&metrics, &Chrome::measuring(true))
                .issues
                .into_iter()
                .filter(|issue| issue.severity == DiagnosticSeverity::Error)
                .collect::<Vec<_>>();
            assert!(errors.is_empty(), "{text_scale:?} page {page}: {errors:#?}");
            let layout = screen.layout_with(&metrics, &Chrome::measuring(true));
            if page > 0 {
                assert!(layout
                    .rect_of_action(action_id("result-previous"))
                    .is_some());
            }
            if page + 1 < pages.len() {
                assert!(layout.rect_of_action(action_id("result-next")).is_some());
            }
        }
    }
}

#[test]
fn result_navigation_is_bounded_and_back_restores_the_deck() {
    let mut runner = AppRunner::new(result_app("A line of output.\n".repeat(100)));
    let last = runner.app().result_pages(&runner.context()).len() - 1;
    for _ in 0..50 {
        let commands = runner.action(action_id("result-next"));
        assert!(!commands
            .iter()
            .any(|command| matches!(command, Command::Spawn { .. })));
    }
    assert_eq!(runner.app().result_page, last);
    for _ in 0..50 {
        runner.action(action_id("result-previous"));
    }
    assert_eq!(runner.app().result_page, 0);
    runner.action(ActionId::BACK);
    assert_eq!(runner.app().view, View::Grid);
    assert!(runner.app().result.is_none());
    assert_eq!(runner.app().last.as_deref(), Some("Run tests failed"));
}

#[test]
fn empty_output_and_missing_results_keep_a_way_back() {
    let runner = AppRunner::new(App::default());
    let context = runner.context();
    let mut app = result_app(String::new());
    assert!(
        format!("{:?}", app.result_screen(&context)).contains("This command produced no output.")
    );
    app.result = None;
    let screen = app.result_screen(&context);
    assert!(screen.owns_back);
    assert!(screen
        .diagnostics(&CLARA_BW_METRICS, &Chrome::measuring(true))
        .issues
        .is_empty());
}

#[test]
fn capture_review_pages_when_requested() {
    let Ok(output) = std::env::var("COBALT_REVIEW_OUT") else {
        return;
    };
    let output = std::path::PathBuf::from(output);
    std::fs::create_dir_all(&output).unwrap();
    let runner = AppRunner::new(App::default());
    let context = runner.context();
    let mut app = result_app(
        "A test could not complete. Inspect the failing assertion and try again.\n".repeat(25),
    );
    let mut screens = vec![("result-first", app.result_screen(&context))];
    app.result_page = app.result_pages(&context).len() - 1;
    screens.push(("result-last", app.result_screen(&context)));
    for (name, screen) in screens {
        let chrome = Chrome::for_screen(&screen, false, Chrome::measuring(true).status);
        let screen = kobo_ui::ensure_way_back(screen, &chrome, "Deck");
        let mut surface = kobo_ui::Surface::new(1072, 1448);
        kobo_ui::render_all(
            &screen,
            &CLARA_BW_METRICS,
            &chrome,
            &kobo_ui::PictureCache::default(),
            &mut surface,
            None,
        );
        let png = kobo_image::encode_png_grey(1072, 1448, &surface.pixels).unwrap();
        std::fs::write(output.join(format!("{name}.png")), png).unwrap();
        std::fs::write(
            output.join(format!("{name}.diagnostics.txt")),
            format!("{:#?}", screen.diagnostics(&CLARA_BW_METRICS, &chrome)),
        )
        .unwrap();
    }
}

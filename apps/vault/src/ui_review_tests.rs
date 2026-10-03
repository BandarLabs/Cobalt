//! In-process app and renderer checks; no interactive simulator is required.
use super::*;
use kobo_sdk::{AppRunner, DiagnosticSeverity, Node};
use kobo_ui::{Chrome, DisplayMetrics, TextScale, CLARA_BW_METRICS};
use std::collections::BTreeSet;

fn vault() -> Vault {
    let notes: Vec<_> = (0..20)
        .map(|n| {
            (
                NoteEntry {
                    id: format!("id-{n}"),
                    path: format!("Note {n:02}.md"),
                    title: format!("Note {n:02} from a quiet morning"),
                    tags: vec!["reading".into(), format!("tag-{n:02}")],
                    links: vec!["Note 00".into()],
                    digest: String::new(),
                    bytes: 42,
                    added: 1_790_841_600,
                },
                Source::Pushed,
            )
        })
        .collect();
    let manifest = Manifest {
        notes: notes.iter().map(|(entry, _)| entry.clone()).collect(),
        failures: (0..12)
            .map(|n| ImportFailure {
                input: format!("Note {n:02}.pdf"),
                reason: "Unsupported file format. Convert this file to Markdown.".into(),
            })
            .collect(),
    };
    Vault {
        loaded: true,
        notes,
        pushed: Some(manifest),
        tag_filter: "reading".into(),
        results: (0..20)
            .map(|n| (n, "A matching sentence from the note.".into()))
            .collect(),
        ..Vault::default()
    }
}

fn assert_fits(screen: &Screen, metrics: DisplayMetrics) {
    let chrome = Chrome::for_screen(screen, false, Chrome::measuring(true).status);
    let errors: Vec<_> = screen
        .diagnostics(&metrics, &chrome)
        .issues
        .into_iter()
        .filter(|issue| issue.severity == DiagnosticSeverity::Error)
        .collect();
    assert!(errors.is_empty(), "{errors:#?}");
}

#[test]
fn all_list_rows_are_reachable_at_every_text_size() {
    for text_scale in TextScale::STEPS {
        let metrics = DisplayMetrics {
            text_scale,
            ..CLARA_BW_METRICS
        };
        let runner = AppRunner::with_metrics(Vault::default(), metrics);
        let mut context = runner.context();
        for (view, expected) in [
            (View::Browse, 20),
            (View::Tags, 21),
            (View::TagNotes, 20),
            (View::Recent, 20),
            (View::Search, 20),
            (View::About, 12),
            (View::Backlinks, 19),
        ] {
            let mut app = vault();
            app.view = view;
            let mut seen = BTreeSet::new();
            for _ in 0..100 {
                let screen = app.screen(&mut context);
                assert_fits(&screen, metrics);
                let layout = screen.layout_with(&metrics, &Chrome::measuring(true));
                for node in &screen.nodes {
                    if let Node::Rows { rows, .. } = node {
                        for row in rows {
                            assert!(layout.rect_of_action(row.action).is_some());
                            assert!(seen.insert(row.action.0), "duplicate row at {view:?}");
                        }
                    }
                }
                let before = app.page(view);
                app.on_action(&mut context, action_id("list-next"));
                if app.page(view) == before {
                    break;
                }
            }
            assert_eq!(seen.len(), expected, "{view:?} at {text_scale:?}");
        }
    }
}

#[test]
fn repeated_next_never_delays_previous() {
    let mut runner = AppRunner::new(vault());
    runner.action(action_id("browse"));
    for _ in 0..50 {
        runner.action(action_id("list-next"));
    }
    let last = runner.app().page(View::Browse);
    assert!(last > 0 && last < 20);
    runner.action(action_id("list-prev"));
    assert_eq!(runner.app().page(View::Browse), last - 1);
    for _ in 0..50 {
        runner.action(action_id("list-prev"));
    }
    assert_eq!(runner.app().page(View::Browse), 0);
}

#[test]
fn failure_details_open_and_back_preserves_the_report_page() {
    let mut runner = AppRunner::new(vault());
    runner.action(action_id("about"));
    for _ in 0..50 {
        runner.action(action_id("list-next"));
    }
    let last = runner.app().page(View::About);
    runner.action(action_id("failure-11"));
    assert_eq!(runner.app().view, View::Failure);
    let pages = runner.app().failure_pages(&runner.context());
    assert!(format!("{pages:?}").contains("Note 11.pdf"));
    assert!(format!("{pages:?}").contains("Convert this file to Markdown."));
    runner.action(ActionId::BACK);
    assert_eq!(runner.app().view, View::About);
    assert_eq!(runner.app().page(View::About), last);
    runner.action(ActionId::BACK);
    assert_eq!(runner.app().view, View::Home);
}

#[test]
fn long_failure_explanations_keep_every_word() {
    let reason =
        "The file could not be converted. Try exporting it as a Markdown note. ".repeat(50);
    for text_scale in TextScale::STEPS {
        let metrics = DisplayMetrics {
            text_scale,
            ..CLARA_BW_METRICS
        };
        let runner = AppRunner::with_metrics(Vault::default(), metrics);
        let context = runner.context();
        let mut app = vault();
        app.view = View::Failure;
        app.pushed.as_mut().unwrap().failures[0].reason = reason.clone();
        let expected = format!("Note 00.pdf {reason}");
        let pages = app.failure_pages(&context);
        assert_eq!(
            pages
                .iter()
                .flatten()
                .flat_map(|text| text.split_whitespace())
                .collect::<Vec<_>>(),
            expected.split_whitespace().collect::<Vec<_>>()
        );
        for page in 0..pages.len() {
            app.set_page(View::Failure, page);
            assert_fits(&app.failure_screen(&context), metrics);
        }
    }
}

#[test]
fn capture_review_pages_when_requested() {
    let Ok(output) = std::env::var("COBALT_REVIEW_OUT") else {
        return;
    };
    let output = std::path::PathBuf::from(output);
    std::fs::create_dir_all(&output).unwrap();
    let runner = AppRunner::new(Vault::default());
    let mut context = runner.context();
    let mut app = vault();
    app.view = View::About;
    let mut screens = vec![("failures-first", app.screen(&mut context))];
    for _ in 0..50 {
        app.on_action(&mut context, action_id("list-next"));
    }
    screens.push(("failures-last", app.screen(&mut context)));
    app.on_action(&mut context, action_id("failure-11"));
    screens.push(("failure-detail", app.screen(&mut context)));
    app.view = View::Browse;
    for _ in 0..50 {
        app.on_action(&mut context, action_id("list-next"));
    }
    app.on_action(&mut context, action_id("list-prev"));
    screens.push((
        "browse-after-overrun-and-previous",
        app.screen(&mut context),
    ));
    for (name, screen) in screens {
        let chrome = Chrome::for_screen(&screen, false, Chrome::measuring(true).status);
        let screen = kobo_ui::ensure_way_back(screen, &chrome, "Vault");
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

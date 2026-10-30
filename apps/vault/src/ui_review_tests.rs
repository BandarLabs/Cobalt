//! In-process app and renderer checks; no interactive simulator is required.
use super::*;
use kobo_sdk::{AppRunner, DiagnosticSeverity, Node};
use kobo_ui::{Chrome, DisplayMetrics, Orientation, TextScale, CLARA_BW_METRICS};
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

/// A note with headings at every level between long paragraphs, each
/// paragraph carrying a wiki link, to fill pages the way a real vault does.
fn long_note() -> String {
    let mut body = String::new();
    for section in 0..12 {
        let level = "#".repeat(section % 3 + 1);
        body.push_str(&format!("{level} Section {section} of the field notes\n\n"));
        for paragraph in 0..3 {
            body.push_str(&format!(
                "Paragraph {paragraph} of section {section} links to [[Note 01|the second note]] \
                 and goes on for a while about the morning, the light and the birds at the \
                 feeder, so that it wraps across several lines at every text size.\n\n"
            ));
        }
    }
    body
}

fn reading(app: &mut Vault, body: String) {
    app.bodies.insert("id-0".to_owned(), body);
    app.current = 0;
    app.view = View::Note;
}

#[test]
fn headings_are_set_as_headings_and_every_page_fits_at_every_text_size() {
    for text_scale in TextScale::STEPS {
        let metrics = DisplayMetrics {
            text_scale,
            ..CLARA_BW_METRICS
        };
        let runner = AppRunner::with_metrics(Vault::default(), metrics);
        let mut context = runner.context();
        let mut app = vault();
        reading(&mut app, long_note());
        let pages = app.note_pages(&context);
        assert!(pages.len() > 2, "{text_scale:?}");
        let mut headings = 0;
        for page in 0..pages.len() {
            app.note_page = page;
            let screen = app.screen(&mut context);
            assert_fits(&screen, metrics);
            headings += screen
                .nodes
                .iter()
                .filter(|node| matches!(node, Node::Heading { .. }))
                .count();
            // A heading never ends a page: the section it opens starts there too.
            if let Some(page::Piece::Heading { .. }) = pages[page].last() {
                panic!("a heading was left alone at the foot of page {page}");
            }
        }
        assert_eq!(headings, 12, "{text_scale:?}");
        let words = pages
            .iter()
            .flatten()
            .filter_map(|piece| match piece {
                page::Piece::Body { text, .. } => Some(text.split_whitespace().count()),
                page::Piece::Heading { .. } => None,
            })
            .sum::<usize>();
        let expected = md::blocks(&long_note())
            .iter()
            .filter_map(|block| match block {
                md::Block::Body { text, .. } => Some(text.split_whitespace().count()),
                md::Block::Heading { .. } => None,
            })
            .sum::<usize>();
        assert_eq!(words, expected, "no word is lost between pages");
    }
}

#[test]
fn a_wiki_link_opens_its_note_and_back_returns_to_the_page_it_was_on() {
    let mut runner = AppRunner::new(vault());
    runner.action(action_id("browse"));
    runner
        .app_mut()
        .bodies
        .insert("id-0".to_owned(), long_note());
    runner
        .app_mut()
        .bodies
        .insert("id-1".to_owned(), "# Second\n\nThe second note.".to_owned());
    runner.action(action_id("note-0"));
    assert_eq!(runner.app().view, View::Note);
    runner.action(action_id("next-page"));
    let page = runner.app().note_page;
    let mut context = runner.context();
    let screen = runner.app_mut().screen(&mut context);
    let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::measuring(true));
    assert!(
        layout.rect_of_action(action_id("wiki-1")).is_some(),
        "the link's words are a control"
    );
    runner.action(action_id("wiki-1"));
    assert_eq!(runner.app().current, 1);
    assert_eq!(runner.app().view, View::Note);
    runner.action(ActionId::BACK);
    assert_eq!(runner.app().current, 0);
    assert_eq!(runner.app().note_page, page);
    runner.action(ActionId::BACK);
    assert_eq!(runner.app().view, View::Browse);
}

#[test]
fn a_link_to_a_note_that_is_not_on_the_reader_stays_plain_words() {
    let runner = AppRunner::new(Vault::default());
    let mut context = runner.context();
    let mut app = vault();
    reading(&mut app, "Read [[Somewhere Else]] next.".to_owned());
    let screen = app.screen(&mut context);
    assert!(format!("{screen:?}").contains("Read Somewhere Else next."));
    assert!(!format!("{screen:?}").contains("wiki-"));
}

#[test]
fn the_page_buttons_turn_the_note() {
    let mut runner = AppRunner::new(vault());
    runner
        .app_mut()
        .bodies
        .insert("id-0".to_owned(), long_note());
    runner.action(action_id("recent"));
    runner.action(action_id("note-0"));
    runner.page_turn(true);
    assert_eq!(runner.app().note_page, 1);
    runner.page_turn(false);
    assert_eq!(runner.app().note_page, 0);
}

#[test]
fn notes_pushed_while_vault_was_away_appear_when_it_comes_back() {
    let manifest = |paths: &[&str]| {
        let notes = paths
            .iter()
            .map(|path| {
                format!(
                    r#"{{"id":"note-{path}","path":"{path}.md","title":"{path}","digest":"d","bytes":"1","added":"1"}}"#
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        let bytes =
            format!(r#"{{"format":"vault-shelf","version":"1","notes":[{notes}]}}"#).into_bytes();
        StoreResult::ShelfRead {
            name: shelf::MANIFEST.into(),
            offset: 0,
            size: u32::try_from(bytes.len()).unwrap(),
            bytes,
        }
    };
    let mut runner = AppRunner::new(Vault::default());
    runner.start();
    runner.store_result(StoreResult::Loaded {
        key: STATE.into(),
        value: None,
    });
    runner.store_result(manifest(&["Welcome"]));
    runner.store_result(StoreResult::Denied(kobo_sdk::StoreError::Missing));
    assert_eq!(runner.app().notes.len(), 1);
    runner
        .app_mut()
        .bodies
        .insert("note-Welcome".into(), "Hello.".into());
    runner.action(action_id("recent"));
    runner.action(action_id("note-0"));
    runner.resume();
    // The list stays on screen while the shelf is read again.
    let mut context = runner.context();
    let during = runner.app_mut().screen(&mut context);
    assert!(!format!("{during:?}").contains("Skeleton"));
    runner.store_result(manifest(&["Alpha", "Welcome"]));
    runner.store_result(StoreResult::Denied(kobo_sdk::StoreError::Missing));
    assert_eq!(runner.app().notes.len(), 2);
    assert_eq!(
        runner
            .app()
            .current_entry()
            .map(|(entry, _)| entry.id.as_str()),
        Some("note-Welcome"),
        "the note being read stays the note being read"
    );
    runner.lifecycle(kobo_sdk::Lifecycle::Foreground);
    assert!(runner.app().manifest_load.is_some());
}

fn banners(screen: &Screen) -> Vec<(BannerLevel, String, Option<ActionId>)> {
    screen
        .nodes
        .iter()
        .filter_map(|node| match node {
            Node::Banner {
                level,
                text,
                action,
                ..
            } => Some((*level, text.clone(), action.as_ref().map(|a| a.action))),
            _ => None,
        })
        .collect()
}

/// A note whose body could not be read used to drop the reader back on the
/// list with no word, so the tap looked as if it had done nothing.
#[test]
fn a_note_that_cannot_be_opened_says_so_on_the_list_it_returns_to() {
    let mut runner = AppRunner::new(vault());
    runner.action(action_id("recent"));
    runner.action(action_id("note-3"));
    assert!(runner.app().body_load.is_some(), "the body is being read");
    let mut context = runner.context();
    runner.app_mut().on_shelf(
        &mut context,
        &shelf::note_name("id-3"),
        StoreResult::Denied(kobo_sdk::StoreError::Missing),
    );
    assert_eq!(runner.app().view, View::Recent);
    let mut context = runner.context();
    let screen = runner.app_mut().screen(&mut context);
    assert_eq!(
        banners(&screen),
        [(BannerLevel::Error, UNREADABLE.to_owned(), None)]
    );
    // Turning the page of the list keeps it; the next tap moves on.
    runner.action(action_id("list-next"));
    assert!(runner.app().notice.is_some());
    runner.action(action_id("list-prev"));
    runner.action(action_id("note-4"));
    assert!(runner.app().notice.is_none());
}

/// The notice costs the lists their room, so every row of every list it can
/// sit over is still reached by turning pages, at every size held either way.
#[test]
fn every_list_row_is_reached_under_the_unreadable_note_notice_at_every_size_held_either_way() {
    for text_scale in TextScale::STEPS {
        for orientation in [Orientation::Portrait, Orientation::Landscape] {
            let metrics = DisplayMetrics {
                text_scale,
                ..CLARA_BW_METRICS
            }
            .oriented(orientation);
            let runner = AppRunner::with_metrics(Vault::default(), metrics);
            let mut context = runner.context();
            for (view, expected) in [
                (View::Browse, 20),
                (View::TagNotes, 20),
                (View::Recent, 20),
                (View::Search, 20),
                (View::Backlinks, 19),
            ] {
                let mut app = vault();
                app.view = view;
                app.notice = Some((BannerLevel::Error, UNREADABLE.to_owned()));
                let mut seen = BTreeSet::new();
                for _ in 0..100 {
                    let screen = app.screen(&mut context);
                    assert_fits(&screen, metrics);
                    assert_eq!(banners(&screen).len(), 1, "{view:?}");
                    let layout = screen.layout_with(&metrics, &Chrome::measuring(true));
                    for node in &screen.nodes {
                        if let Node::Rows { rows, .. } = node {
                            for row in rows {
                                assert!(layout.rect_of_action(row.action).is_some());
                                seen.insert(row.action.0);
                            }
                        }
                    }
                    let before = app.page(view);
                    app.on_action(&mut context, action_id("list-next"));
                    if app.page(view) == before {
                        break;
                    }
                }
                assert_eq!(
                    seen.len(),
                    expected,
                    "{view:?} at {text_scale:?} {orientation:?}"
                );
            }
        }
    }
}

/// Import failures are an error with See why on the banner, in place of the
/// About Vault button that went to the same screen, and the home screen keeps
/// every way in on the panel at every size held either way.
#[test]
fn import_failures_carry_see_why_in_place_of_about_vault_at_every_size_held_either_way() {
    for text_scale in TextScale::STEPS {
        for orientation in [Orientation::Portrait, Orientation::Landscape] {
            let metrics = DisplayMetrics {
                text_scale,
                ..CLARA_BW_METRICS
            }
            .oriented(orientation);
            let runner = AppRunner::with_metrics(Vault::default(), metrics);
            let mut context = runner.context();
            for failed in [true, false] {
                let mut app = vault();
                if !failed {
                    app.pushed.as_mut().unwrap().failures.clear();
                }
                let at = format!("{text_scale:?} {orientation:?} failed {failed}");
                let screen = app.screen(&mut context);
                assert_fits(&screen, metrics);
                let layout = screen.layout_with(&metrics, &Chrome::measuring(true));
                for action in ["browse", "tags", "recent", "search", "about"] {
                    assert!(
                        layout.rect_of_action(action_id(action)).is_some(),
                        "{at}: {action}"
                    );
                }
                let offered = layout
                    .nodes
                    .iter()
                    .filter(|node| node.kind.acts_on() == Some(action_id("about")))
                    .count();
                assert_eq!(offered, 1, "{at}");
                let drawn = banners(&screen);
                if failed {
                    assert_eq!(
                        drawn,
                        [(
                            BannerLevel::Error,
                            "12 notes did not import.".to_owned(),
                            Some(action_id("about"))
                        )],
                        "{at}"
                    );
                } else {
                    assert!(drawn.is_empty(), "{at}");
                    assert!(format!("{screen:?}").contains("About Vault"), "{at}");
                }
            }
        }
    }
}

#[test]
fn a_note_no_longer_on_the_shelf_is_drawn_as_an_error() {
    let runner = AppRunner::new(Vault::default());
    let mut context = runner.context();
    let mut app = vault();
    app.current = usize::MAX;
    app.view = View::Note;
    let screen = app.screen(&mut context);
    assert_eq!(
        banners(&screen),
        [(
            BannerLevel::Error,
            "This note is no longer on the shelf.".to_owned(),
            None
        )]
    );
}

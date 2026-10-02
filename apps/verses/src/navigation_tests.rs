use super::*;
use kobo_sdk::{AppRunner, Command};
use kobo_ui::{Chrome, DisplayMetrics, TextScale, CLARA_BW_METRICS};
use std::collections::BTreeSet;

fn poem(index: usize, lines: usize) -> OnlinePoem {
    OnlinePoem {
        title: format!("Review poem {}", index + 1),
        author: "Review Poet".into(),
        lines: (0..lines)
            .map(|line| format!("Line {} of this complete poem.", line + 1))
            .collect(),
        linecount: lines.to_string(),
    }
}
fn screen(runner: &AppRunner<Verses>) -> Screen {
    let mut context = runner.context();
    runner.app().show(&mut context);
    context
        .commands()
        .iter()
        .find_map(|command| match command {
            Command::SetScreen(screen) => Some(screen.clone()),
            _ => None,
        })
        .expect("shown screen")
}
fn tap(runner: &mut AppRunner<Verses>, name: &str) {
    let drawn = screen(runner);
    let metrics = runner.context().metrics();
    let chrome = Chrome::for_screen(&drawn, false, Chrome::measuring(true).status);
    let layout = drawn.layout_with(&metrics, &chrome);
    let action = action_id(name);
    let rect = layout
        .rect_of_action(action)
        .unwrap_or_else(|| panic!("missing {name}"));
    let (x, y) = (rect.x + rect.width / 2, rect.y + rect.height / 2);
    assert!(x >= 0 && x < metrics.width && y >= 0 && y < metrics.height);
    assert_eq!(layout.hit_test(x, y), Some(action));
    runner.action(action);
}
fn verify_collection(runner: &mut AppRunner<Verses>, prefix: &str, count: usize) {
    let mut found = BTreeSet::new();
    let mut steps = 0;
    loop {
        let drawn = screen(runner);
        let metrics = runner.context().metrics();
        let chrome = Chrome::for_screen(&drawn, false, Chrome::measuring(true).status);
        let diagnostics = drawn.diagnostics(&metrics, &chrome);
        assert!(
            diagnostics.issues.is_empty(),
            "{metrics:?}: {:?}",
            diagnostics.issues
        );
        let layout = drawn.layout_with(&metrics, &chrome);
        for index in 0..count {
            let action = action_id(&format!("{prefix}{index}"));
            if let Some(rect) = layout.rect_of_action(action) {
                assert_eq!(
                    layout.hit_test(rect.x + rect.width / 2, rect.y + rect.height / 2),
                    Some(action)
                );
                found.insert(index);
            }
        }
        let before = screen(runner);
        runner.page_turn(true);
        if screen(runner) == before {
            break;
        }
        steps += 1;
        assert!(steps <= count + 10);
    }
    assert_eq!(found, (0..count).collect());
    let last = screen(runner);
    runner.page_turn(true);
    runner.action(action_id("list-next"));
    assert_eq!(screen(runner), last);
}

#[test]
fn every_search_result_and_favorite_is_reachable_at_all_text_sizes() {
    for text_scale in TextScale::STEPS {
        let metrics = DisplayMetrics {
            text_scale,
            ..CLARA_BW_METRICS
        };
        let mut runner = AppRunner::with_metrics(
            Verses {
                view: View::Results,
                results: (0..48).map(|i| poem(i, 2)).collect(),
                ..Verses::default()
            },
            metrics,
        );
        verify_collection(&mut runner, "result-", 48);
        let last = runner.app().results_page;
        tap(&mut runner, "result-47");
        assert_eq!(runner.app().view, View::Online);
        runner.action(ActionId::BACK);
        assert_eq!(runner.app().view, View::Results);
        assert_eq!(runner.app().results_page, last);
        runner.app_mut().view = View::Browse;
        runner.app_mut().saved.favorites = CORPUS.iter().map(|poem| poem.id.into()).collect();
        runner.app_mut().saved.online_favorites = (0..12).map(|i| poem(i, 2)).collect();
        verify_collection(&mut runner, "saved-online-", 12);
        runner.app_mut().browse_page = 0;
        verify_collection(&mut runner, "poem-", CORPUS.len());
    }
}

#[test]
fn every_line_and_stanza_remains_readable_at_all_text_sizes() {
    for text_scale in TextScale::STEPS {
        let metrics = DisplayMetrics {
            text_scale,
            ..CLARA_BW_METRICS
        };
        let mut verse = poem(0, 50);
        verse.lines.insert(10, String::new());
        verse.lines.insert(22, String::new());
        let mut runner = AppRunner::with_metrics(
            Verses {
                view: View::Online,
                online: Some(0),
                results: vec![verse],
                ..Verses::default()
            },
            metrics,
        );
        let pages = runner
            .app()
            .online_pages(&runner.context(), &runner.app().results[0]);
        assert!(pages.len() > 1);
        let actual: Vec<_> = pages
            .iter()
            .flatten()
            .map(|line| line.text.as_str())
            .collect();
        let expected: Vec<_> = runner.app().results[0]
            .lines
            .iter()
            .filter(|line| !line.is_empty())
            .map(String::as_str)
            .collect();
        assert_eq!(actual, expected);
        assert!(pages.iter().flatten().any(|line| line.stanza_start));
        for page in 0..pages.len() {
            assert_eq!(runner.app().online_page, page);
            let drawn = screen(&runner);
            let chrome = Chrome::for_screen(&drawn, false, Chrome::measuring(true).status);
            assert!(drawn.diagnostics(&metrics, &chrome).issues.is_empty());
            runner.page_turn(true);
        }
        assert_eq!(runner.app().online_page, pages.len() - 1);
        let last = screen(&runner);
        runner.action(action_id("online-next"));
        assert_eq!(screen(&runner), last);
        for _ in 0..=pages.len() {
            runner.page_turn(false);
        }
        assert_eq!(runner.app().online_page, 0);
    }
}

#[test]
fn an_exceptionally_long_line_is_split_without_losing_words() {
    let text = "The complete line remains available ".repeat(150);
    let runner = AppRunner::new(Verses {
        view: View::Online,
        online: Some(0),
        results: vec![OnlinePoem {
            lines: vec![text.clone()],
            ..poem(0, 0)
        }],
        ..Verses::default()
    });
    let pages = runner
        .app()
        .online_pages(&runner.context(), &runner.app().results[0]);
    assert!(pages.len() > 1);
    let actual = pages
        .iter()
        .flatten()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    assert_eq!(
        actual.split_whitespace().collect::<Vec<_>>(),
        text.split_whitespace().collect::<Vec<_>>()
    );
}

#[test]
fn author_search_and_back_cancel_work_without_stale_navigation() {
    let mut runner = AppRunner::new(Verses {
        view: View::Online,
        online: Some(0),
        results: vec![poem(0, 2)],
        ..Verses::default()
    });
    tap(&mut runner, "more-by-author");
    assert_eq!(runner.app().view, View::Results);
    assert_eq!(runner.app().online, None);
    let old = runner.app().task.expect("search task");
    assert!(format!("{:?}", screen(&runner)).contains("Searching poetry"));
    runner.action(ActionId::BACK);
    assert_eq!(runner.app().view, View::Browse);
    assert_eq!(runner.app().task, None);
    let before = screen(&runner);
    runner.task_outcome(
        old,
        TaskOutcome::Completed(serde_json::to_vec(&vec![poem(1, 2)]).unwrap()),
    );
    assert_eq!(screen(&runner), before);
    tap(&mut runner, "search");
    runner.app_mut().keyboard = Keyboard::with_text("review");
    runner.action(ActionId::BACK);
    assert_eq!(runner.app().view, View::Browse);
    tap(&mut runner, "search");
    runner.app_mut().keyboard = Keyboard::with_text("review");
    tap(&mut runner, "kb.enter");
    let request = runner.app().task.expect("new search");
    tap(&mut runner, "search");
    assert_eq!(runner.app().view, View::Search);
    assert_eq!(runner.app().task, None);
    runner.task_outcome(request, TaskOutcome::Completed(b"[]".to_vec()));
    assert_eq!(runner.app().view, View::Search);
}

#[test]
fn quote_card_back_restores_the_screen_it_came_from() {
    for view in [View::Today, View::Reading] {
        let mut runner = AppRunner::new(Verses {
            view,
            ..Verses::default()
        });
        tap(&mut runner, "card");
        assert_eq!(runner.app().view, View::Card);
        runner.action(action_id("card"));
        assert_eq!(runner.app().card_from, view);
        runner.action(ActionId::BACK);
        assert_eq!(runner.app().view, view);
    }
}

#[test]
fn pagination_reuses_the_measurement_until_content_or_metrics_change() {
    let mut runner = AppRunner::new(Verses {
        view: View::Online,
        online: Some(0),
        results: vec![poem(0, 500)],
        ..Verses::default()
    });
    let start = std::time::Instant::now();
    let first = runner
        .app()
        .online_pages(&runner.context(), &runner.app().results[0]);
    let measured = start.elapsed();
    let start = std::time::Instant::now();
    let repeated = runner
        .app()
        .online_pages(&runner.context(), &runner.app().results[0]);
    eprintln!(
        "500-line pagination: first {measured:?}, cached {:?}; {} pages",
        start.elapsed(),
        first.len()
    );
    assert_eq!(first.len(), repeated.len());
    let key = runner.app().online_layout.borrow().as_ref().unwrap().1;
    runner.app_mut().results[0]
        .lines
        .push("A newly arrived final line.".into());
    let changed = runner
        .app()
        .online_pages(&runner.context(), &runner.app().results[0]);
    assert_ne!(runner.app().online_layout.borrow().as_ref().unwrap().1, key);
    assert_eq!(
        changed.last().unwrap().last().unwrap().text,
        "A newly arrived final line."
    );
}

#[test]
fn long_unbroken_lines_keep_combining_characters_together() {
    let text = "e\u{301}".repeat(1000);
    let runner = AppRunner::new(Verses::default());
    let (left, right) = split_online_line(&text, &runner.context()).expect("long line split");
    assert!(!right.starts_with('\u{301}'));
    assert!(left.ends_with('\u{301}'));
    assert_eq!(left + &right, text);
}

#[test]
fn every_bundled_poem_is_protocol_valid_at_every_text_size() {
    for text_scale in TextScale::STEPS {
        let metrics = DisplayMetrics {
            text_scale,
            ..CLARA_BW_METRICS
        };
        for poem in 0..CORPUS.len() {
            let runner = AppRunner::with_metrics(
                Verses {
                    view: View::Reading,
                    poem,
                    ..Verses::default()
                },
                metrics,
            );
            // show() invokes Context's actual frame validation, which catches
            // a one-entry action_bar even when a renderer-only layout fits.
            let shown = screen(&runner);
            assert!(shown
                .layout_with(&metrics, &Chrome::measuring(true))
                .rect_of_action(action_id("card"))
                .is_some());
        }
    }
}

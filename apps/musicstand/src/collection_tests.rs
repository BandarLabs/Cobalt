use super::*;
use kobo_sdk::{AppRunner, Command};
use kobo_ui::{Chrome, DisplayMetrics, TextScale, CLARA_BW_METRICS};
use std::collections::BTreeSet;

fn populated(metrics: DisplayMetrics) -> AppRunner<Stand> {
    AppRunner::with_metrics(
        Stand {
            scores: (0..24)
                .map(|index| Score {
                    id: format!("score-{index}"),
                    title: format!(
                        "Rehearsal score {} with a title long enough to need measured wrapping",
                        index + 1
                    ),
                    pages: 3,
                    width: 1072,
                    height: 1448,
                })
                .collect(),
            startup: Startup {
                state_loaded: true,
                manifest_loaded: true,
                started: true,
            },
            ..Stand::default()
        },
        metrics,
    )
}

fn screen(runner: &AppRunner<Stand>) -> Screen {
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

fn tap(runner: &mut AppRunner<Stand>, name: &str) {
    let drawn = screen(runner);
    let chrome = Chrome::for_screen(&drawn, false, Chrome::measuring(true).status);
    let metrics = runner.context().metrics();
    let layout = drawn.layout_with(&metrics, &chrome);
    let action = action_id(name);
    let rect = layout
        .rect_of_action(action)
        .unwrap_or_else(|| panic!("missing {name}"));
    let (x, y) = (rect.x + rect.width / 2, rect.y + rect.height / 2);
    assert!(x >= 0 && x < metrics.width && y >= 0 && y < metrics.height);
    assert_eq!(layout.hit_test(x, y), Some(action), "covered {name}");
    runner.action(action);
}

fn verify_collection(runner: &mut AppRunner<Stand>, prefix: &str, count: usize) {
    let mut found = BTreeSet::new();
    let mut steps = 0;
    loop {
        let drawn = screen(runner);
        let chrome = Chrome::for_screen(&drawn, false, Chrome::measuring(true).status);
        let metrics = runner.context().metrics();
        let diagnostics = drawn.diagnostics(&metrics, &chrome);
        assert!(
            diagnostics.issues.is_empty(),
            "{metrics:?}: {:?}",
            diagnostics.issues
        );
        let layout = drawn.layout_with(&metrics, &chrome);
        for index in 0..count {
            if let Some(rect) = layout.rect_of_action(action_id(&format!("{prefix}{index}"))) {
                assert_eq!(
                    layout.hit_test(rect.x + rect.width / 2, rect.y + rect.height / 2),
                    Some(action_id(&format!("{prefix}{index}")))
                );
                found.insert(index);
            }
        }
        let before = format!("{:?}", screen(runner));
        runner.page_turn(true);
        if format!("{:?}", screen(runner)) == before {
            break;
        }
        steps += 1;
        assert!(steps <= count, "paging did not stop");
    }
    assert_eq!(found, (0..count).collect(), "some rows never appeared");
    let last = screen(runner);
    runner.page_turn(true);
    runner.action(action_id("list-next"));
    assert_eq!(screen(runner), last, "past the final page");
}

#[test]
fn every_score_setlist_and_entry_is_reachable_at_every_text_size() {
    for text_scale in TextScale::STEPS {
        let metrics = DisplayMetrics {
            text_scale,
            ..CLARA_BW_METRICS
        };
        let mut runner = populated(metrics);
        verify_collection(&mut runner, "open-score-", 24);
        tap(&mut runner, ABOUT);
        assert!(screen(&runner).owns_back);
        runner.action(ActionId::BACK);
        let library_page = runner.app().library_page;
        tap(&mut runner, SETLISTS);
        for _ in 0..12 {
            tap(&mut runner, "new-list");
        }
        assert_eq!(runner.app().setlists.len(), 12);
        runner.app_mut().setlists_page = 0;
        verify_collection(&mut runner, "list-", 12);
        tap(&mut runner, "list-11");
        assert!(screen(&runner).owns_back);
        verify_collection(&mut runner, "entry-", 24);
        tap(&mut runner, "entry-23");
        assert_eq!(runner.app().current.as_deref(), Some("score-23"));
        assert_eq!(runner.app().setlist_position, Some(23));
        runner.action(ActionId::BACK);
        assert_eq!(runner.app().view, View::Library);
        assert_eq!(runner.app().library_page, library_page);
        tap(&mut runner, SETLISTS);
        tap(&mut runner, "list-11");
        runner.action(ActionId::BACK);
        assert_eq!(runner.app().view, View::Setlists);
        runner.action(ActionId::BACK);
        assert_eq!(runner.app().view, View::Library);
        assert!(!screen(&runner).owns_back);
    }
}

#[test]
fn shrinking_a_setlist_keeps_its_controls_and_returns_to_the_first_page() {
    let mut runner = populated(CLARA_BW_METRICS);
    tap(&mut runner, SETLISTS);
    tap(&mut runner, "new-list");
    tap(&mut runner, "list-0");
    verify_collection(&mut runner, "entry-", 24);
    for _ in 0..24 {
        tap(&mut runner, "remove-last");
    }
    let drawn = screen(&runner);
    assert!(format!("{drawn:?}").contains("No scores on this setlist"));
    assert!(drawn
        .diagnostics(&CLARA_BW_METRICS, &Chrome::measuring(true))
        .issues
        .is_empty());
    tap(&mut runner, SETLISTS);
    assert_eq!(runner.app().view, View::Setlists);
}

#[test]
fn a_notice_and_marked_score_still_fit_the_paginated_library() {
    let mut runner = populated(DisplayMetrics {
        text_scale: TextScale::Largest,
        ..CLARA_BW_METRICS
    });
    runner.app_mut().notice =
        Some("One score could not be opened. Re-push it from your computer.".into());
    runner.app_mut().state_mut("score-0").marked = true;
    verify_collection(&mut runner, "open-score-", 24);
}

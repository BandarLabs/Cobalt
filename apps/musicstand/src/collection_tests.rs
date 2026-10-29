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
        tap(&mut runner, NEW_LIST);
        assert_eq!(runner.app().view, View::Picker);
        verify_collection(&mut runner, "pick-", 24);
        for index in 0..24 {
            runner.action(action_id(&format!("pick-{index}")));
        }
        tap(&mut runner, DONE_ADDING);
        assert_eq!(runner.app().view, View::Setlist);
        runner.action(ActionId::BACK);
        let everything = runner.app().setlists[0].entries.clone();
        assert_eq!(everything.len(), 24);
        runner
            .app_mut()
            .setlists
            .extend((2..=12).map(|number| Setlist {
                name: format!("Setlist {number}"),
                entries: everything.clone(),
            }));
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

fn with_setlist(entries: &[&str]) -> AppRunner<Stand> {
    let mut runner = populated(CLARA_BW_METRICS);
    runner.app_mut().setlists.push(Setlist {
        name: "Gig".to_owned(),
        entries: entries.iter().map(|entry| (*entry).to_owned()).collect(),
    });
    tap(&mut runner, SETLISTS);
    tap(&mut runner, "list-0");
    runner
}

fn shown(runner: &AppRunner<Stand>) -> String {
    format!("{:?}", screen(runner))
}

#[test]
fn shrinking_a_setlist_keeps_its_controls_and_returns_to_the_first_page() {
    let ids = (0..24)
        .map(|index| format!("score-{index}"))
        .collect::<Vec<_>>();
    let mut runner = with_setlist(&ids.iter().map(String::as_str).collect::<Vec<_>>());
    verify_collection(&mut runner, "entry-", 24);
    runner.app_mut().setlist_page = 0;
    for _ in 0..24 {
        tap(&mut runner, "entry-0-menu");
        tap(&mut runner, REMOVE_ENTRY);
    }
    let drawn = screen(&runner);
    assert!(format!("{drawn:?}").contains("No scores on this setlist"));
    assert!(drawn
        .diagnostics(&CLARA_BW_METRICS, &Chrome::measuring(true))
        .issues
        .is_empty());
    tap(&mut runner, ADD_SCORES);
    assert_eq!(runner.app().view, View::Picker);
    runner.action(ActionId::BACK);
    runner.action(ActionId::BACK);
    assert_eq!(runner.app().view, View::Setlists);
}

#[test]
fn a_setlist_entry_moves_up_and_down_and_comes_off_without_leaving_the_library() {
    let mut runner = with_setlist(&["score-0", "score-1", "score-2"]);
    tap(&mut runner, "entry-2-menu");
    tap(&mut runner, MOVE_UP);
    assert_eq!(
        runner.app().setlists[0].entries,
        ["score-0", "score-2", "score-1"]
    );
    tap(&mut runner, "entry-0-menu");
    tap(&mut runner, MOVE_DOWN);
    assert_eq!(
        runner.app().setlists[0].entries,
        ["score-2", "score-0", "score-1"]
    );
    tap(&mut runner, "entry-1-menu");
    tap(&mut runner, REMOVE_ENTRY);
    assert_eq!(runner.app().setlists[0].entries, ["score-2", "score-1"]);
    assert_eq!(
        runner.app().scores.len(),
        24,
        "the score stays in the library"
    );
    // The first entry offers no way up, and the menu closes on Back.
    tap(&mut runner, "entry-0-menu");
    assert!(shown(&runner).contains("Move down"));
    assert!(!shown(&runner).contains("Move up"));
    runner.action(ActionId::BACK);
    assert!(runner.app().entry_menu.is_none());
    assert_eq!(runner.app().view, View::Setlist);
}

#[test]
fn a_new_setlist_starts_empty_and_is_filled_from_the_library() {
    let mut runner = populated(CLARA_BW_METRICS);
    tap(&mut runner, SETLISTS);
    tap(&mut runner, NEW_LIST);
    assert_eq!(runner.app().view, View::Picker);
    assert!(runner.app().setlists[0].entries.is_empty());
    tap(&mut runner, "pick-3");
    tap(&mut runner, "pick-1");
    assert_eq!(runner.app().setlists[0].entries, ["score-3", "score-1"]);
    assert!(shown(&runner).contains("On this setlist as number 2"));
    tap(&mut runner, DONE_ADDING);
    assert_eq!(runner.app().view, View::Setlist);
    assert!(shown(&runner).contains("2. Rehearsal score 2"));
}

#[test]
fn deleting_a_setlist_asks_first_and_keeps_its_scores() {
    let mut runner = with_setlist(&["score-0"]);
    tap(&mut runner, DELETE_LIST);
    assert!(runner.app().confirming_delete);
    tap(&mut runner, KEEP_LIST);
    assert_eq!(runner.app().setlists.len(), 1);
    tap(&mut runner, DELETE_LIST);
    tap(&mut runner, CONFIRM_DELETE);
    assert!(runner.app().setlists.is_empty());
    assert_eq!(runner.app().view, View::Setlists);
    assert_eq!(runner.app().scores.len(), 24);
}

#[test]
fn a_score_pushed_while_the_stand_was_away_is_in_the_library_when_it_comes_back() {
    let mut runner = AppRunner::new(Stand::default());
    runner.start();
    runner.store_result(StoreResult::Loaded {
        key: STATE.into(),
        value: None,
    });
    let manifest = |count: usize| {
        let scores = (0..count)
            .map(|index| {
                format!(
                    r#"{{"id":"score-{index}","title":"Piece {index}","digest":"d{index}","pages":"2","width":"1072","height":"1448","added":"1"}}"#
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        format!(r#"{{"format":"musicstand-shelf","version":"1","scores":[{scores}]}}"#).into_bytes()
    };
    let read = |bytes: Vec<u8>| StoreResult::ShelfRead {
        name: MANIFEST.into(),
        offset: 0,
        size: u32::try_from(bytes.len()).expect("fits"),
        bytes,
    };
    runner.store_result(read(manifest(1)));
    assert_eq!(runner.app().scores.len(), 1);
    runner.lifecycle(kobo_sdk::Lifecycle::Foreground);
    runner.store_result(read(manifest(3)));
    assert_eq!(runner.app().scores.len(), 3);
    runner.resume();
    assert!(runner.app().manifest_load.is_some());
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

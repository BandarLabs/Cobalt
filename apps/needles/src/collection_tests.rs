use super::*;
use kobo_sdk::{AppRunner, Command, Node, TaskError};
use kobo_ui::{Chrome, DisplayMetrics, Orientation, TextScale, CLARA_BW_METRICS};
use std::collections::BTreeSet;

fn populated(metrics: DisplayMetrics) -> AppRunner<Needles> {
    let app = Needles {
        projects: (0..12)
            .map(|index| Project::new(format!("Project {} for a long winter evening", index + 1)))
            .collect(),
        libraries: std::array::from_fn(|collection| {
            (0..MAX_PATTERNS)
                .map(|index| Pattern {
                    title: format!(
                        "Pattern {} in collection {} with a longer descriptive name",
                        index + 1,
                        collection + 1
                    ),
                    detail: "Synthetic knitting pattern".into(),
                })
                .collect()
        }),
        loaded: [true; 3],
        ..Needles::default()
    };
    AppRunner::with_metrics(app, metrics)
}

fn screen(runner: &AppRunner<Needles>) -> Screen {
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

fn tap(runner: &mut AppRunner<Needles>, name: &str) {
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
    assert_eq!(layout.hit_test(x, y), Some(action), "covered {name}");
    runner.action(action);
}

fn verify_collection(runner: &mut AppRunner<Needles>, prefix: &str, count: usize) {
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
        let previous = screen(runner);
        runner.page_turn(true);
        if screen(runner) == previous {
            break;
        }
        steps += 1;
        assert!(steps <= count, "paging did not stop");
    }
    assert_eq!(
        found,
        (0..count).collect(),
        "some rows were never reachable"
    );
    let last = screen(runner);
    runner.page_turn(true);
    runner.action(action_id("list-next"));
    assert_eq!(screen(runner), last);
}

#[test]
fn all_projects_and_collection_items_are_reachable_at_every_text_size() {
    for text_scale in TextScale::STEPS {
        let mut runner = populated(DisplayMetrics {
            text_scale,
            ..CLARA_BW_METRICS
        });
        tap(&mut runner, "plus");
        tap(&mut runner, "projects");
        verify_collection(&mut runner, "project-", 12);
        tap(&mut runner, "project-11");
        assert_eq!(runner.app().current, 11);
        tap(&mut runner, "plus");
        assert_eq!(runner.app().projects[0].counters[0].row, 1);
        tap(&mut runner, "more");
        tap(&mut runner, "library");
        verify_collection(&mut runner, "pattern-", MAX_PATTERNS);
        let last = runner.app().collection_pages[0];
        tap(&mut runner, "pattern-59");
        assert_eq!(runner.app().route, Route::Pattern);
        runner.action(ActionId::BACK);
        assert_eq!(runner.app().route, Route::Library);
        assert_eq!(runner.app().collection_pages[0], last);
        tap(&mut runner, "queue-tab");
        assert_eq!(runner.app().collection_pages[1], 0);
        runner.page_turn(true);
        let queue_page = runner.app().collection_pages[1];
        tap(&mut runner, "favorites-tab");
        runner.page_turn(true);
        tap(&mut runner, "queue-tab");
        assert_eq!(runner.app().collection_pages[1], queue_page);
        tap(&mut runner, "library-tab");
        assert_eq!(runner.app().collection_pages[0], last);
        runner.action(ActionId::BACK);
        assert_eq!(runner.app().route, Route::Project);
        assert_eq!(runner.app().current, 11);
        assert_eq!(runner.app().counter().row, 1);
    }
}

fn spawned(commands: &[Command]) -> usize {
    commands
        .iter()
        .filter(|command| matches!(command, Command::Spawn { .. }))
        .count()
}

#[test]
fn repeated_sync_keeps_one_request_and_refresh_resets_only_its_collection() {
    let mut runner = populated(CLARA_BW_METRICS);
    tap(&mut runner, "more");
    tap(&mut runner, "library");
    runner.page_turn(true);
    assert_eq!(spawned(&runner.action(action_id("sync"))), 1);
    let task = runner.app().task.expect("sync task").0;
    assert_eq!(spawned(&runner.action(action_id("sync"))), 0);
    tap(&mut runner, "queue-tab");
    runner.page_turn(true);
    let queue = runner.app().collection_pages[1];
    assert_eq!(spawned(&runner.action(action_id("sync"))), 0);
    runner.task_outcome(
        task,
        TaskOutcome::Completed(br#"{"patterns":[{"name":"One refreshed pattern"}]}"#.to_vec()),
    );
    assert_eq!(runner.app().libraries[0].len(), 1);
    assert_eq!(runner.app().collection_pages[0], 0);
    assert_eq!(runner.app().collection_pages[1], queue);
    assert!(runner.app().task.is_none());
    assert_eq!(runner.app().collection, Collection::Queue);
    tap(&mut runner, "library-tab");
    tap(&mut runner, "pattern-0");
    assert_eq!(
        runner.app().selected.as_ref().unwrap().title,
        "One refreshed pattern"
    );
}

/// A refused key used to read "Ravelry did not accept the named Basic
/// credential." at Attention. It is an error now, in Ravelry's terms, ending
/// on the command that fixes it. Asking again would fail the same way, so
/// the banner carries no Try again and Sync stays at the foot for after the
/// key is fixed.
#[test]
fn a_refused_ravelry_key_is_an_error_naming_the_computer_command_and_sync_stays_at_the_foot() {
    for text_scale in TextScale::STEPS {
        for orientation in [Orientation::Portrait, Orientation::Landscape] {
            let metrics = DisplayMetrics {
                text_scale,
                ..CLARA_BW_METRICS
            }
            .oriented(orientation);
            let at = format!("{text_scale:?} {orientation:?}");
            let mut runner = populated(metrics);
            tap(&mut runner, "more");
            tap(&mut runner, "library");
            runner.page_turn(true);
            tap(&mut runner, "sync");
            let task = runner.app().task.expect("sync task").0;
            runner.task_outcome(task, TaskOutcome::Failed(TaskError::Unauthorized));
            let drawn = screen(&runner);
            let diagnostics = drawn.diagnostics(&metrics, &Chrome::measuring(true));
            assert!(
                diagnostics.issues.is_empty(),
                "{at}: {:?}",
                diagnostics.issues
            );
            let banner = drawn.nodes.iter().find_map(|node| match node {
                Node::Banner {
                    level,
                    text,
                    action,
                    ..
                } => Some((*level, text.clone(), action.is_some())),
                _ => None,
            });
            let Some((level, text, offers)) = banner else {
                panic!("{at}: no banner");
            };
            assert_eq!(level, BannerLevel::Error, "{at}");
            assert!(
                text.ends_with("Run kobo secret set ravelry on your computer."),
                "{at}: {text}"
            );
            assert!(!offers, "{at}: a control that would fail the same way");
            assert!(runner.app().notice.is_none(), "{at}");
            tap(&mut runner, "sync");
            assert!(runner.app().task.is_some(), "{at}");
            assert!(runner.app().sync_problem.is_none(), "{at}");
        }
    }
}

/// A Ravelry that did not answer offers Try again on the banner that says
/// so, and Sync at the foot gives way to it, so the retry sits beside its
/// reason and is offered once, at every size held either way.
#[test]
fn a_ravelry_that_does_not_answer_offers_try_again_on_its_banner_and_only_there() {
    for text_scale in TextScale::STEPS {
        for orientation in [Orientation::Portrait, Orientation::Landscape] {
            let metrics = DisplayMetrics {
                text_scale,
                ..CLARA_BW_METRICS
            }
            .oriented(orientation);
            let at = format!("{text_scale:?} {orientation:?}");
            let mut runner = populated(metrics);
            tap(&mut runner, "more");
            tap(&mut runner, "library");
            runner.page_turn(true);
            tap(&mut runner, "sync");
            let task = runner.app().task.expect("sync task").0;
            // The first failure is tried once more by the SDK, unseen; the
            // second is the one the app is told about.
            for _ in 0..2 {
                runner.task_outcome(task, TaskOutcome::Failed(TaskError::TimedOut));
            }
            let drawn = screen(&runner);
            let diagnostics = drawn.diagnostics(&metrics, &Chrome::measuring(true));
            assert!(
                diagnostics.issues.is_empty(),
                "{at}: {:?}",
                diagnostics.issues
            );
            let on_banner = drawn.nodes.iter().any(|node| {
                matches!(
                    node,
                    Node::Banner { action: Some(action), .. }
                        if action.action == action_id("sync") && action.label == "Try again"
                )
            });
            assert!(on_banner, "{at}: Try again is not on the banner");
            let offered = diagnostics
                .layout
                .nodes
                .iter()
                .filter(|node| node.kind.acts_on() == Some(action_id("sync")))
                .count();
            assert_eq!(offered, 1, "{at}: sync offered {offered} times");
            // Every pattern is still reachable under the taller banner.
            while runner.app().collection_pages[0] > 0 {
                runner.page_turn(false);
            }
            verify_collection(&mut runner, "pattern-", MAX_PATTERNS);
            tap(&mut runner, "sync");
            assert!(runner.app().task.is_some(), "{at}");
        }
    }
}

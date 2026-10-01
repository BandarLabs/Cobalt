//! In-process app/renderer coverage. These are not interactive simulator tests.
use super::*;
use kobo_sdk::{AppRunner, Command, DiagnosticSeverity, Task};
use kobo_ui::{Chrome, DisplayMetrics, TextScale, CLARA_BW_METRICS};

fn entities(count: usize) -> Vec<ha::Entity> {
    (0..count)
        .map(|n| ha::Entity {
            id: format!("light.room_{n:02}"),
            name: format!("Room {n:02} ceiling light"),
            state: "off".into(),
        })
        .collect()
}

fn assert_fits(screen: &Screen, metrics: DisplayMetrics) {
    let errors: Vec<_> = screen
        .diagnostics(&metrics, &Chrome::measuring(true))
        .issues
        .into_iter()
        .filter(|issue| issue.severity == DiagnosticSeverity::Error)
        .collect();
    assert!(errors.is_empty(), "{:?}: {errors:#?}", screen.id);
}

#[test]
fn every_discovered_device_is_reachable_at_every_text_size() {
    for text_scale in TextScale::STEPS {
        let metrics = DisplayMetrics {
            text_scale,
            ..CLARA_BW_METRICS
        };
        let runner = AppRunner::with_metrics(HomePanel::default(), metrics);
        let context = runner.context();
        let mut app = HomePanel {
            view: View::Add,
            entities: entities(100),
            ..HomePanel::default()
        };
        let pages = app.picker_pages(&context);
        assert_eq!(
            pages.iter().flatten().copied().collect::<Vec<_>>(),
            (0..100).collect::<Vec<_>>()
        );
        for (page, indices) in pages.iter().enumerate() {
            app.picker_page = page;
            let screen = app.add(&context);
            assert_fits(&screen, metrics);
            let layout = screen.layout_with(&metrics, &Chrome::measuring(true));
            for &index in indices {
                let action = action_id(&format!("entity.light.room_{index:02}"));
                assert!(
                    layout.rect_of_action(action).is_some(),
                    "device {index} on page {page}"
                );
            }
        }
    }
}

#[test]
fn all_wall_tiles_and_error_notices_fit_at_every_text_size() {
    for text_scale in TextScale::STEPS {
        let metrics = DisplayMetrics {
            text_scale,
            ..CLARA_BW_METRICS
        };
        let runner = AppRunner::with_metrics(HomePanel::default(), metrics);
        let context = runner.context();
        for wall in [false, true] {
            let mut app = HomePanel {
                view: View::Grid,
                wall,
                tiles: (0..12).map(|n| format!("light.room_{n:02}")).collect(),
                banner: Some(
                    "Home Assistant did not answer. Check the address and that it is running."
                        .into(),
                ),
                ..HomePanel::default()
            };
            let pages = app.grid_pages(&context);
            assert_eq!(
                pages.iter().flatten().copied().collect::<Vec<_>>(),
                (0..12).collect::<Vec<_>>()
            );
            for (page, indices) in pages.iter().enumerate() {
                app.grid_page = page;
                let screen = app.grid(&context);
                assert_fits(&screen, metrics);
                let layout = screen.layout_with(&metrics, &Chrome::measuring(true));
                for &index in indices {
                    assert!(layout
                        .rect_of_action(action_id(&format!("tile.light.room_{index:02}")))
                        .is_some());
                }
            }
        }
    }
}

#[test]
fn picker_navigation_clamps_resets_search_and_keeps_back_behavior() {
    let mut runner = AppRunner::new(HomePanel {
        view: View::Add,
        entities: entities(20),
        ..HomePanel::default()
    });
    let last = runner.app().picker_pages(&runner.context()).len() - 1;
    for _ in 0..50 {
        runner.action(action_id("page-next"));
    }
    assert_eq!(runner.app().picker_page, last);
    runner.action(action_id(SEARCH));
    runner.app_mut().keyboard = Keyboard::with_text("room_19");
    runner.action(action_id("kb.enter"));
    assert_eq!(runner.app().picker_page, 0);
    assert_eq!(runner.app().picker_rows().len(), 1);
    runner.action(action_id("entity.light.room_19"));
    assert!(runner.app().view == View::Grid);
    assert_eq!(runner.app().tiles, ["light.room_19"]);
    runner.app_mut().view = View::Add;
    runner.action(ActionId::BACK);
    assert!(runner.app().view == View::Grid);
    for _ in 0..50 {
        runner.action(action_id("page-previous"));
    }
    assert_eq!(runner.app().grid_page, 0);
}

fn connected_panel() -> AppRunner<HomePanel> {
    AppRunner::new(HomePanel {
        view: View::Grid,
        base: "https://ha.example".into(),
        tiles: vec![
            "light.desk".into(),
            "switch.fan".into(),
            "climate.room".into(),
        ],
        states: vec![
            ("light.desk".into(), "off".into()),
            ("switch.fan".into(), "off".into()),
        ],
        climate: vec![(
            "climate.room".into(),
            ha::Climate {
                current: Some(19.5),
                target: Some(21.0),
            },
        )],
        ..HomePanel::default()
    })
}

fn spawned(commands: &[Command]) -> Vec<(TaskId, Task)> {
    commands
        .iter()
        .filter_map(|command| match command {
            Command::Spawn { task, work } => Some((*task, work.clone())),
            _ => None,
        })
        .collect()
}

fn begin_poll(runner: &mut AppRunner<HomePanel>) -> TaskId {
    let commands = runner.action(action_id(BACK));
    assert_eq!(spawned(&commands).len(), 1);
    let (task, kind) = runner.app().task.expect("active poll");
    assert_eq!(kind, "poll");
    task
}

#[test]
fn stale_poll_outcomes_preserve_the_current_service_and_its_result() {
    for succeeds in [false, true] {
        for stale in [
            TaskOutcome::Completed(br#"[{"id":"light.desk","s":"old"}]"#.to_vec()),
            TaskOutcome::Failed(TaskError::Unauthorized),
            TaskOutcome::Cancelled,
        ] {
            let mut runner = connected_panel();
            let poll = begin_poll(&mut runner);
            runner.action(action_id("tile.light.desk"));
            let current = runner.app().task.expect("active service");
            let pending = runner.app().pending.clone();
            let banner = runner.app().banner.clone();
            assert!(runner.task_outcome(poll, stale).is_empty());
            assert_eq!(runner.app().task, Some(current));
            assert_eq!(runner.app().pending, pending);
            assert_eq!(runner.app().banner, banner);
            assert_eq!(runner.app().state_of("light.desk"), Some("off"));
            if succeeds {
                let commands = runner.task_outcome(current.0, TaskOutcome::Completed(vec![]));
                assert_eq!(spawned(&commands).len(), 1);
                let (confirmation, kind) = runner.app().task.expect("confirmation poll");
                assert_eq!(kind, "poll");
                runner.task_outcome(
                    confirmation,
                    TaskOutcome::Completed(br#"[{"id":"light.desk","s":"on"}]"#.to_vec()),
                );
                assert_eq!(runner.app().banner.as_deref(), Some("desk is now on."));
            } else {
                runner.task_outcome(current.0, TaskOutcome::Failed(TaskError::Unauthorized));
                assert!(runner
                    .app()
                    .banner
                    .as_deref()
                    .unwrap()
                    .starts_with("Couldn't update desk."));
            }
            assert!(runner.app().task.is_none());
            assert!(runner.app().pending.is_none());
        }
    }
}

#[test]
fn repeated_taps_keep_each_service_request_and_only_the_latest_status() {
    for latest_first in [false, true] {
        let mut runner = connected_panel();
        let mut tasks = Vec::new();
        for entity in ["light.desk", "light.desk", "switch.fan"] {
            let commands = runner.action(action_id(&format!("tile.{entity}")));
            let requests = spawned(&commands);
            assert_eq!(requests.len(), 1);
            assert_eq!(
                requests[0].1,
                ha::service("https://ha.example", entity).unwrap()
            );
            assert!(!commands
                .iter()
                .any(|command| matches!(command, Command::Cancel(_))));
            tasks.push(requests[0].0);
        }
        let latest = tasks.pop().unwrap();
        assert_eq!(runner.app().task, Some((latest, "service")));
        if latest_first {
            runner.task_outcome(latest, TaskOutcome::Completed(vec![]));
        }
        for old in tasks {
            let tracked = runner.app().task;
            let pending = runner.app().pending.clone();
            let banner = runner.app().banner.clone();
            assert!(runner
                .task_outcome(old, TaskOutcome::Completed(vec![]))
                .is_empty());
            assert_eq!(runner.app().task, tracked);
            assert_eq!(runner.app().pending, pending);
            assert_eq!(runner.app().banner, banner);
        }
        if !latest_first {
            runner.task_outcome(latest, TaskOutcome::Completed(vec![]));
        }
        let (confirmation, kind) = runner
            .app()
            .task
            .expect("confirmation poll survives old replies");
        assert_eq!(kind, "poll");
        runner.task_outcome(
            confirmation,
            TaskOutcome::Completed(
                br#"[{"id":"light.desk","s":"off"},{"id":"switch.fan","s":"on"}]"#.to_vec(),
            ),
        );
        assert_eq!(runner.app().banner.as_deref(), Some("fan is now on."));
        assert!(runner.app().pending.is_none());
    }
}

#[test]
fn stale_poll_does_not_consume_device_discovery() {
    let mut runner = connected_panel();
    let poll = begin_poll(&mut runner);
    runner.action(action_id(ADD));
    let current = runner.app().task.expect("device discovery");
    assert_eq!(current.1, "entities");
    assert!(runner
        .task_outcome(poll, TaskOutcome::Failed(TaskError::Unauthorized))
        .is_empty());
    assert_eq!(runner.app().task, Some(current));
    assert!(runner.app().view == View::Add);
    assert!(runner.app().banner.is_none());
    runner.task_outcome(
        current.0,
        TaskOutcome::Completed(
            br#"[{"id":"light.kitchen","s":"on","n":"Kitchen light"}]"#.to_vec(),
        ),
    );
    assert_eq!(runner.app().entities[0].name, "Kitchen light");
    assert!(runner.app().task.is_none());
}

#[test]
fn stale_poll_does_not_consume_a_new_connection_test() {
    for succeeds in [false, true] {
        let mut runner = connected_panel();
        let poll = begin_poll(&mut runner);
        runner.action(action_id("change-url"));
        runner.action(action_id("kb.enter"));
        let current = runner.app().task.expect("connection test");
        assert_eq!(current.1, "test");
        assert!(runner
            .task_outcome(poll, TaskOutcome::Completed(vec![]))
            .is_empty());
        assert_eq!(runner.app().task, Some(current));
        assert_eq!(runner.app().banner.as_deref(), Some("Testing connection…"));
        if succeeds {
            runner.task_outcome(current.0, TaskOutcome::Completed(vec![]));
            assert!(runner.app().view == View::Grid);
            assert_eq!(runner.app().task.unwrap().1, "poll");
            assert!(runner.app().banner.is_none());
        } else {
            runner.task_outcome(current.0, TaskOutcome::Failed(TaskError::Unauthorized));
            assert!(runner.app().view == View::Setup);
            assert!(runner
                .app()
                .banner
                .as_deref()
                .unwrap()
                .contains("refused the token"));
        }
    }
}

#[test]
fn stale_poll_does_not_consume_the_latest_temperature_change() {
    let mut runner = connected_panel();
    let poll = begin_poll(&mut runner);
    runner.action(action_id("tile.climate.room"));
    let commands = runner.action(action_id("warm"));
    let current = runner.app().task.expect("temperature change");
    assert_eq!(current.1, "climate");
    assert_eq!(
        spawned(&commands)[0].1,
        ha::set_temperature("https://ha.example", "climate.room", 21.5)
    );
    assert!(runner.task_outcome(poll, TaskOutcome::Cancelled).is_empty());
    assert_eq!(runner.app().task, Some(current));
    assert_eq!(runner.app().banner.as_deref(), Some("Setting 21.5°…"));
    runner.task_outcome(current.0, TaskOutcome::Failed(TaskError::Unauthorized));
    assert!(runner
        .app()
        .banner
        .as_deref()
        .unwrap()
        .starts_with("Couldn't set the temperature."));
    assert_eq!(runner.app().climate_of("climate.room").target, Some(21.0));
    assert!(runner.app().task.is_none());
}

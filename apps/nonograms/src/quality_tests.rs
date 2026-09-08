use super::*;
use kobo_sdk::{AppRunner, Command, StoreError, StoreRequest};
use kobo_ui::{Chrome, DisplayMetrics, LayoutKind, TextScale};
fn ready(index: usize) -> AppRunner<Game> {
    let mut runner = AppRunner::new(Game::default());
    runner.start();
    runner.store_result(StoreResult::Loaded {
        key: SOLVED.into(),
        value: None,
    });
    runner.action(action_id(&format!("puzzle-{index}")));
    let key = runner.app().progress_key().unwrap();
    runner.store_result(StoreResult::Loaded { key, value: None });
    runner
}
fn written(commands: Vec<Command>) -> Vec<u8> {
    commands
        .into_iter()
        .find_map(|cmd| match cmd {
            Command::Store(StoreRequest::Save { key, value }) if key.starts_with("progress-") => {
                Some(value)
            }
            _ => None,
        })
        .expect("progress write")
}
fn ack(runner: &mut AppRunner<Game>) -> Vec<Command> {
    runner.store_result(StoreResult::Saved {
        key: runner.app().progress_key().unwrap(),
    })
}
#[test]
fn all_supported_boards_have_attached_clues_and_every_square_is_reachable_at_every_size() {
    for (width, height, ppi) in [
        (1072, 1448, 300),
        (1448, 1072, 300),
        (758, 1024, 212),
        (1024, 758, 212),
    ] {
        for text_scale in TextScale::STEPS {
            let metrics = DisplayMetrics {
                width,
                height,
                pixels_per_inch: ppi,
                text_scale,
            };
            let context = AppRunner::with_metrics(Game::default(), metrics).context();
            for index in [0, 12, 24, 36, 48] {
                let mut game = Game::default();
                game.select(&mut Context::default(), index);
                let (board, clues) = game.ink_board().unwrap();
                let mut view = game
                    .fitted_view(&context, &board, &clues)
                    .unwrap_or_else(|| panic!("usable board {metrics:?} index={index}"));
                assert!(view.contains(0));
                let mut reached = BTreeSet::new();
                loop {
                    loop {
                        reached.extend(view.cells());
                        game.viewport = Some(view.clone());
                        let screen = game.play(&context).with_own_back(true);
                        let chrome = Chrome::measuring(true);
                        let issues = screen.diagnostics(&metrics, &chrome).issues;
                        assert!(issues.is_empty(), "{metrics:?} index={index}: {issues:?}");
                        let layout = screen.layout_with(&metrics, &chrome);
                        for cell in view.cells() {
                            let rect = layout
                                .rect_of_action(action_id(&format!("board.cell.{cell}")))
                                .unwrap();
                            assert!(
                                rect.width >= metrics.touch_target_minimum()
                                    && rect.height >= metrics.touch_target_minimum()
                            );
                        }
                        for row in view.visible_rows() {
                            let rect = layout
                                .rect_of_action(action_id(&format!("board.row.{row}")))
                                .unwrap();
                            let cell = layout
                                .rect_of_action(action_id(&format!(
                                    "board.cell.{}",
                                    row * board.columns() + view.visible_columns().start
                                )))
                                .unwrap();
                            assert_eq!((rect.y, rect.height), (cell.y, cell.height));
                        }
                        for column in view.visible_columns() {
                            let rect = layout
                                .rect_of_action(action_id(&format!("board.column.{column}")))
                                .unwrap();
                            let cell = layout
                                .rect_of_action(action_id(&format!(
                                    "board.cell.{}",
                                    view.visible_rows().start * board.columns() + column
                                )))
                                .unwrap();
                            assert_eq!((rect.x, rect.width), (cell.x, cell.width));
                        }
                        if !view.pan(kobo_sdk::board::Direction::Right) {
                            break;
                        }
                    }
                    while view.pan(kobo_sdk::board::Direction::Left) {}
                    if !view.pan(kobo_sdk::board::Direction::Down) {
                        break;
                    }
                }
                assert_eq!(reached.len(), board.columns() * board.rows());
            }
        }
    }
}
#[test]
fn marks_runs_and_restart_have_atomic_persistent_undo() {
    let mut r = ready(0);
    written(r.action(action_id("board.cell.0")));
    ack(&mut r);
    r.action(action_id("more"));
    r.action(action_id("run-entry"));
    ack(&mut r);
    r.action(action_id("resume"));
    r.action(action_id("board.cell.5"));
    let saved = written(r.action(action_id("board.cell.9")));
    assert!(r.app().marks[5..10].iter().all(|m| *m == Mark::Fill));
    let mut restored = Game::default();
    restored.select(&mut Context::default(), 0);
    saved::restore(&mut restored, &saved).unwrap();
    restored.progress_loading = false;
    restored.on_action(&mut Context::default(), action_id("undo"));
    assert!(restored.marks[5..10].iter().all(|m| *m == Mark::Blank));
    assert_eq!(restored.marks[0], Mark::Fill);
    let before = restored.marks.clone();
    restored.route = Route::Menu;
    restored.on_action(&mut Context::default(), action_id("reset"));
    assert_eq!(restored.marks, before);
    restored.on_action(&mut Context::default(), action_id("confirm-reset"));
    assert!(restored.marks.iter().all(|m| *m == Mark::Blank));
    restored.on_action(&mut Context::default(), action_id("undo"));
    assert_eq!(restored.marks, before);
}
#[test]
fn failed_save_keeps_latest_marks_and_history_until_exact_retry_ack() {
    let mut r = ready(0);
    let old = written(r.action(action_id("board.cell.0")));
    r.action(action_id("board.cell.1"));
    assert!(!r.app().can_suspend());
    let latest = written(ack(&mut r));
    assert_ne!(old, latest);
    assert!(!r.app().can_suspend());
    r.store_result(StoreResult::Denied(StoreError::NoRoom));
    assert!(matches!(r.app().draft.status(), Status::Failed(_)));
    r.action(action_id("back-browser"));
    assert_eq!(r.app().route, Route::Menu);
    let retry = written(r.action(action_id("retry-save")));
    assert_eq!(retry, latest);
    ack(&mut r);
    assert!(r.app().can_suspend());
    r.action(action_id("back-browser"));
    assert_eq!(r.app().route, Route::Browser);
    let mut reopened = AppRunner::new(Game::default());
    reopened.start();
    reopened.action(action_id("puzzle-0"));
    reopened.store_result(StoreResult::Loaded {
        key: "progress-pack-00".into(),
        value: Some(retry),
    });
    assert_eq!(reopened.app().marks, r.app().marks);
    assert_eq!(reopened.app().undo, r.app().undo);
    reopened.action(action_id("undo"));
    assert_eq!(reopened.app().marks[0], Mark::Fill);
    assert_eq!(reopened.app().marks[1], Mark::Blank);
}
#[test]
fn corrupt_future_oversized_and_changed_identity_records_are_preserved() {
    let mut g = Game::default();
    g.select(&mut Context::default(), 0);
    let valid = saved::encode(&g).unwrap();
    let future = String::from_utf8(valid.clone())
        .unwrap()
        .replace("\"version\":1", "\"version\":99")
        .into_bytes();
    assert_ne!(valid, future);
    let changed = String::from_utf8(valid.clone())
        .unwrap()
        .replace(
            &kobo_net::sha256::hex_digest(
                &g.puzzle()
                    .unwrap()
                    .answer
                    .iter()
                    .map(|b| u8::from(*b))
                    .collect::<Vec<_>>(),
            ),
            &"0".repeat(64),
        )
        .into_bytes();
    for bytes in [
        b"broken".to_vec(),
        future,
        changed,
        vec![b'x'; saved::LIMIT + 1],
    ] {
        let mut r = ready(0);
        r.action(action_id("back-browser"));
        r.action(action_id("puzzle-0"));
        let commands = r.store_result(StoreResult::Loaded {
            key: "progress-pack-00".into(),
            value: Some(bytes),
        });
        assert!(r.app().progress_loading && r.app().load_error.is_some());
        assert!(!commands
            .iter()
            .any(|c| matches!(c, Command::Store(StoreRequest::Save { .. }))));
        r.action(action_id("board.cell.0"));
        assert!(r.app().marks.iter().all(|m| *m == Mark::Blank));
    }
    let mut r = ready(0);
    r.action(action_id("back-browser"));
    r.action(action_id("puzzle-0"));
    let commands = r.store_result(StoreResult::Loaded {
        key: "progress-pack-00".into(),
        value: Some([b"g\n".as_slice(), b"#........................"].concat()),
    });
    assert!(!r.app().progress_loading);
    assert!(r.app().guided);
    assert_eq!(r.app().marks[0], Mark::Fill);
    assert!(!commands
        .iter()
        .any(|c| matches!(c, Command::Store(StoreRequest::Save { .. }))));
}
#[test]
fn long_clues_options_recovery_and_every_help_page_fit() {
    for (width, height, ppi) in [
        (1072, 1448, 300),
        (1448, 1072, 300),
        (758, 1024, 212),
        (1024, 758, 212),
    ] {
        for text_scale in TextScale::STEPS {
            let metrics = DisplayMetrics {
                width,
                height,
                pixels_per_inch: ppi,
                text_scale,
            };
            let context = AppRunner::with_metrics(Game::default(), metrics).context();
            let mut g = Game::default();
            g.select(&mut Context::default(), 48);
            g.progress_loading = false;
            g.clue = Some(("Column 25".into(), vec!["1"; 13].join(" · ")));
            for route in [
                Route::Menu,
                Route::Clue,
                Route::Restart,
                Route::Photo,
                Route::Gate,
                Route::Reveal,
            ] {
                g.route = route;
                let screen = g.screen(&context).with_own_back(true);
                let issues = screen
                    .diagnostics(&metrics, &Chrome::measuring(true))
                    .issues;
                assert!(issues.is_empty(), "{metrics:?} {route:?}: {issues:?}");
            }
            for help_route in [Route::HowTo, Route::PhotoHelp] {
                g.route = help_route;
                for page in 0..g.help_pages(&context).len() {
                    g.help_page = page;
                    let issues = g
                        .screen(&context)
                        .diagnostics(&metrics, &Chrome::measuring(true))
                        .issues;
                    assert!(issues.is_empty(), "{metrics:?} help {page}: {issues:?}");
                }
            }
            g.route = Route::Menu;
            g.notice = Some("Save this game before opening another puzzle.".into());
            g.draft.replace(vec![1]).unwrap();
            let rev = g.draft.begin().unwrap().revision;
            g.draft.finish(rev, Err("Full".into()));
            let issues = g
                .screen(&context)
                .diagnostics(&metrics, &Chrome::measuring(true))
                .issues;
            assert!(issues.is_empty(), "{metrics:?} recovery: {issues:?}");
        }
    }
}
#[test]
fn history_stays_bounded_for_the_largest_board_and_rejects_offscreen_actions() {
    let mut g = Game::default();
    let mut context = Context::default();
    g.select(&mut context, 48);
    g.progress_loading = false;
    for _ in 0..80 {
        g.toggle(&mut context, 624);
    }
    assert_eq!(g.undo.len(), 64);
    assert!(saved::encode(&g).unwrap().len() < saved::LIMIT);
    g.focus = Some(0);
    let before = g.marks.clone();
    g.board_action(&mut context, action_id("board.cell.624"));
    assert_eq!(g.marks, before);
    let screen = g.play(&context);
    let layout = screen.layout_with(&context.metrics(), &Chrome::measuring(true));
    assert!(layout
        .nodes
        .iter()
        .any(|n| matches!(n.kind, LayoutKind::Cell(_, _, true))));
}
#[test]
fn completion_waits_for_the_solved_index_and_retries_its_failure() {
    let mut g = Game::default();
    g.select(&mut Context::default(), 0);
    g.progress_loading = false;
    g.marks = g
        .puzzle()
        .unwrap()
        .answer
        .iter()
        .map(|filled| if *filled { Mark::Fill } else { Mark::Cross })
        .collect();
    g.marks[24] = Mark::Fill;
    g.focus = Some(24);
    let mut r = AppRunner::new(g);
    r.start();
    r.store_result(StoreResult::Loaded {
        key: SOLVED.into(),
        value: None,
    });
    let commands = r.action(action_id("board.cell.24"));
    assert_eq!(r.app().route, Route::Reveal);
    assert_eq!(
        commands
            .iter()
            .filter(|c| matches!(c, Command::Store(StoreRequest::Save { .. })))
            .count(),
        2
    );
    ack(&mut r);
    assert!(!r.app().can_suspend());
    r.store_result(StoreResult::Denied(StoreError::NoRoom));
    assert!(matches!(r.app().solved_draft.status(), Status::Failed(_)));
    r.action(action_id("next-puzzle"));
    assert_eq!(r.app().route, Route::Menu);
    let retry = r.action(action_id("retry-save"));
    assert!(retry
        .iter()
        .any(|c| matches!(c, Command::Store(StoreRequest::Save { key, .. }) if key == SOLVED)));
    r.store_result(StoreResult::Saved { key: SOLVED.into() });
    assert!(r.app().can_suspend());
}

use super::*;
use kobo_sdk::{AppRunner, Command, StoreError, StoreRequest};
use kobo_ui::{Chrome, DisplayMetrics, LayoutKind, TextScale, CLARA_BW_METRICS};
fn load(value: Option<Vec<u8>>) -> StoreResult {
    StoreResult::Loaded {
        key: saved::KEY.into(),
        value,
    }
}
fn ack() -> StoreResult {
    StoreResult::Saved {
        key: saved::KEY.into(),
    }
}
fn ready() -> AppRunner<Sudoku> {
    let mut r = AppRunner::new(Sudoku::default());
    r.start();
    r.store_result(load(None));
    r.store_result(ack());
    r
}
fn tap(r: &mut AppRunner<Sudoku>, name: &str) -> Vec<Command> {
    r.action(action_id(name))
}
fn blank(g: &Game, puzzles: &[Puzzle]) -> usize {
    puzzles[g.puzzle]
        .clues
        .iter()
        .position(|&n| n == 0)
        .unwrap()
}
fn count(board: &mut [u8; CELLS], budget: &mut usize) -> usize {
    assert!(*budget > 0, "solver work limit");
    *budget -= 1;
    let mut choice = None;
    for cell in 0..CELLS {
        if board[cell] != 0 {
            continue;
        }
        let used = (0..CELLS)
            .filter(|&peer| {
                peer / 9 == cell / 9
                    || peer % 9 == cell % 9
                    || (peer / 27 == cell / 27 && peer % 9 / 3 == cell % 9 / 3)
            })
            .fold(0_u16, |bits, peer| bits | (1 << board[peer]));
        let mask = 0x3fe & !used;
        if mask == 0 {
            return 0;
        }
        if choice.is_none_or(|(_, old): (usize, u16)| mask.count_ones() < old.count_ones()) {
            choice = Some((cell, mask));
        }
    }
    let Some((cell, mask)) = choice else {
        return 1;
    };
    let mut total = 0;
    for n in 1..=9 {
        if mask & (1 << n) != 0 {
            board[cell] = n;
            total += count(board, budget);
            if total >= 2 {
                break;
            }
        }
    }
    board[cell] = 0;
    total.min(2)
}
#[test]
fn original_pack_has_36_distinct_unique_puzzles_and_valid_solutions() {
    let puzzles = game::pack();
    assert_eq!(puzzles.len(), 36);
    assert_eq!(
        puzzles
            .iter()
            .map(|p| p.clues)
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        36
    );
    for level in Level::ALL {
        assert_eq!(puzzles.iter().filter(|p| p.level == level).count(), 12);
    }
    for p in puzzles {
        for i in 0..CELLS {
            assert!((1..=9).contains(&p.solution[i]));
            assert!(p.clues[i] == 0 || p.clues[i] == p.solution[i]);
            for j in 0..i {
                if i / 9 == j / 9 || i % 9 == j % 9 || (i / 27 == j / 27 && i % 9 / 3 == j % 9 / 3)
                {
                    assert_ne!(p.solution[i], p.solution[j]);
                }
            }
        }
        assert_eq!(count(&mut p.clues.clone(), &mut 1_000_000), 1);
    }
}
#[test]
fn wrong_answers_are_kept_and_checking_is_optional() {
    let mut r = ready();
    let app = r.app();
    let c = blank(&app.game, &app.puzzles);
    let wrong = app.puzzles[0].solution[c] % 9 + 1;
    tap(&mut r, &cell_name(c));
    tap(&mut r, &digit_name(wrong));
    assert_eq!(r.app().game.position.board[c], wrong);
    assert!(!r.app().game.checking);
    assert!(!r.app().status().contains("Check this"));
    tap(&mut r, "more");
    tap(&mut r, "checking");
    tap(&mut r, "play");
    assert!(r.app().status().contains("Check this answer"));
}
#[test]
fn notes_erase_hint_and_reset_are_atomic_and_undoable() {
    let puzzles = game::pack();
    let mut g = Game::new(0, &puzzles);
    let c = blank(&g, &puzzles);
    g.position.selected = Some(c);
    g.pencil = true;
    g.enter(1, &puzzles);
    g.enter(9, &puzzles);
    assert_eq!(g.position.notes[c], 257);
    g.enter(1, &puzzles);
    assert_eq!(g.position.notes[c], 256);
    g.undo();
    assert_eq!(g.position.notes[c], 257);
    g.erase(&puzzles);
    assert_eq!(g.position.notes[c], 0);
    g.undo();
    let noted = g.position.clone();
    g.hint(&puzzles);
    assert_eq!(g.position.hints, 1);
    g.undo();
    assert_eq!(g.position, noted);
    g.pencil = false;
    g.enter(puzzles[0].solution[c], &puzzles);
    assert_eq!(g.position.notes[c], 0);
    let before = g.position.clone();
    g.reset(&puzzles);
    g.undo();
    assert_eq!(g.position, before);
    let given = puzzles[0].clues.iter().position(|&n| n != 0).unwrap();
    g.position.selected = Some(given);
    assert!(!g.enter(1, &puzzles) && !g.erase(&puzzles) && !g.hint(&puzzles));
}
#[test]
fn record_restores_notes_selection_settings_and_bounded_undo_history() {
    let puzzles = game::pack();
    let mut g = Game::new(12, &puzzles);
    let c = blank(&g, &puzzles);
    g.position.selected = Some(c);
    g.checking = true;
    g.pencil = true;
    for _ in 0..100 {
        g.enter(3, &puzzles);
    }
    assert_eq!(g.undo.len(), game::HISTORY);
    let bytes = saved::encode(&g, &puzzles).unwrap();
    assert!(bytes.len() < saved::LIMIT);
    let mut restored = saved::decode(&bytes, &puzzles).unwrap();
    assert_eq!(restored, g);
    restored.undo();
    assert_ne!(restored.position.notes, g.position.notes);
}
#[test]
fn invalid_future_and_changed_puzzle_records_preserve_source_without_writes() {
    let g = Sudoku::default();
    let bytes = saved::encode(&g.game, &g.puzzles).unwrap();
    let text = String::from_utf8(bytes).unwrap();
    for bytes in [
        b"".to_vec(),
        b"{}".to_vec(),
        vec![0; saved::LIMIT + 1],
        text.replace("\"version\":1", "\"version\":2").into_bytes(),
        text.replace("\"puzzle\":\"0\"", "\"puzzle\":\"1\"")
            .into_bytes(),
        text.replace("\"selected\":\"none\"", "\"selected\":\"81\"")
            .into_bytes(),
    ] {
        let mut r = AppRunner::new(Sudoku::default());
        r.start();
        let commands = r.store_result(load(Some(bytes.clone())));
        assert!(!r.app().loaded && r.app().load_error.is_some());
        assert!(!commands
            .iter()
            .any(|c| matches!(c, Command::Store(StoreRequest::Save { .. }))));
        tap(&mut r, "cell-0");
        assert!(!r.app().loaded);
    }
}
#[test]
fn save_acknowledges_exact_revision_and_failure_keeps_latest_moves_for_retry() {
    let mut r = ready();
    let c = blank(&r.app().game, &r.app().puzzles);
    tap(&mut r, &cell_name(c));
    tap(&mut r, "pencil");
    tap(&mut r, "digit-2");
    assert!(!r.app().can_suspend());
    r.store_result(ack());
    assert!(!r.app().can_suspend());
    r.store_result(StoreResult::Denied(StoreError::NoRoom));
    let latest = r.app().game.clone();
    assert!(matches!(r.app().draft.status(), Status::Failed(_)));
    tap(&mut r, "more");
    let commands = tap(&mut r, "retry-save");
    let written = commands
        .into_iter()
        .find_map(|c| {
            if let Command::Store(StoreRequest::Save { value, .. }) = c {
                Some(value)
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(saved::decode(&written, &r.app().puzzles).unwrap(), latest);
    r.store_result(ack());
    assert!(r.app().can_suspend());
    let mut reopened = AppRunner::new(Sudoku::default());
    reopened.start();
    reopened.store_result(load(Some(written)));
    assert_eq!(reopened.app().game, latest);
}
#[test]
fn completion_survives_reopen_and_undo_reopens_the_last_square() {
    let puzzles = game::pack();
    let mut g = Game::new(24, &puzzles);
    for cell in 0..CELLS {
        if g.editable(cell, &puzzles) {
            g.position.selected = Some(cell);
            g.enter(puzzles[g.puzzle].solution[cell], &puzzles);
        }
    }
    assert!(g.solved(&puzzles));
    let mut restored = saved::decode(&saved::encode(&g, &puzzles).unwrap(), &puzzles).unwrap();
    assert!(restored.solved(&puzzles));
    restored.undo();
    assert!(!restored.solved(&puzzles));
}
#[test]
fn confirmation_screens_do_not_discard_a_game_on_back() {
    let mut r = ready();
    let c = blank(&r.app().game, &r.app().puzzles);
    tap(&mut r, &cell_name(c));
    tap(&mut r, "digit-1");
    let before = r.app().game.clone();
    for destination in ["new-game", "restart", "hint"] {
        tap(&mut r, "more");
        tap(&mut r, destination);
        r.action(ActionId::BACK);
        assert_eq!(r.app().game, before);
    }
    tap(&mut r, "more");
    tap(&mut r, "new-game");
    tap(&mut r, "new-hard");
    assert_eq!(r.app().puzzles[r.app().game.puzzle].level, Level::Hard);
}
#[test]
fn all_routes_fit_at_supported_text_sizes_and_both_orientations() {
    for (width, height, ppi) in [
        (1072, 1448, 300),
        (758, 1024, 212),
        (1448, 1072, 300),
        (1024, 758, 212),
    ] {
        for scale in TextScale::STEPS {
            let metrics = DisplayMetrics {
                width,
                height,
                pixels_per_inch: ppi,
                text_scale: scale,
            };
            let mut r = AppRunner::with_metrics(
                Sudoku::default(),
                DisplayMetrics {
                    width: width.min(height),
                    height: width.max(height),
                    ..metrics
                },
            );
            r.start();
            r.store_result(load(None));
            r.store_result(ack());
            r.app_mut().game.landscape = width > height;
            for view in [
                View::Play,
                View::Menu,
                View::New,
                View::Restart,
                View::Help,
                View::Hint,
                View::Display,
            ] {
                r.app_mut().view = view;
                let screen = r.app().screen(&r.context());
                let chrome = Chrome::with_back(true);
                let issues = screen.diagnostics(&metrics, &chrome).issues;
                assert!(
                    issues.is_empty(),
                    "{width}x{height} {scale:?} {view:?}: {issues:?}"
                );
                if view == View::Play {
                    let layout = screen.layout_with(&metrics, &chrome);
                    for cell in 0..if width > height { 54 } else { CELLS } {
                        let rect = layout.rect_of_action(action_id(&cell_name(cell))).unwrap();
                        assert_eq!(rect.width, rect.height);
                        assert!(rect.width >= metrics.touch_target_minimum());
                    }
                    for name in ["digit-1", "digit-9", "pencil", "more"] {
                        assert!(layout.rect_of_action(action_id(name)).is_some());
                    }
                    assert_eq!(
                        layout
                            .nodes
                            .iter()
                            .filter(|n| matches!(
                                n.kind,
                                LayoutKind::Cell(_, kobo_ui::CellStyle::Board, _)
                            ))
                            .count(),
                        if width > height { 54 } else { CELLS }
                    );
                    if width > height {
                        tap(&mut r, "rows");
                        let screen = r.app().screen(&r.context());
                        assert!(screen.diagnostics(&metrics, &chrome).issues.is_empty());
                        let lower = screen.layout_with(&metrics, &chrome);
                        for cell in 27..CELLS {
                            assert!(lower.rect_of_action(action_id(&cell_name(cell))).is_some());
                        }
                        tap(&mut r, "rows");
                    }
                }
            }
        }
    }
    // Legacy portrait geometry still has additional separation between 3×3 boxes.
    let r = ready();
    let layout = r
        .app()
        .screen(&r.context())
        .layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true));
    let rect = |c| layout.rect_of_action(action_id(&cell_name(c))).unwrap();
    assert!(rect(3).x - rect(2).x > rect(1).x - rect(0).x);
}

#[test]
fn long_game_titles_notes_completion_failures_and_every_help_page_fit() {
    for (width, height, ppi) in [
        (1072, 1448, 300),
        (758, 1024, 212),
        (1448, 1072, 300),
        (1024, 758, 212),
    ] {
        for scale in TextScale::STEPS {
            let metrics = DisplayMetrics {
                width,
                height,
                pixels_per_inch: ppi,
                text_scale: scale,
            };
            let mut r = AppRunner::with_metrics(
                Sudoku::default(),
                DisplayMetrics {
                    width: width.min(height),
                    height: width.max(height),
                    ..metrics
                },
            );
            r.start();
            r.store_result(load(None));
            r.store_result(ack());
            r.app_mut().game.landscape = width > height;
            let assert_screen = |r: &AppRunner<Sudoku>| {
                let screen = r.app().screen(&r.context());
                let errors = screen
                    .diagnostics(&metrics, &Chrome::with_back(true))
                    .issues;
                assert!(
                    errors.is_empty(),
                    "{width}x{height} {scale:?} {:?}: {errors:?}",
                    r.app().view
                );
            };
            for puzzle in [11, 23, 35] {
                let app = r.app_mut();
                app.game = Game::new(puzzle, &app.puzzles);
                app.game.landscape = width > height;
                let cell = blank(&app.game, &app.puzzles);
                app.game.position.selected = Some(cell);
                app.game.position.notes[cell] = 511;
                app.game.pencil = true;
                app.view = View::Play;
                assert_screen(&r);
                {
                    let app = r.app_mut();
                    app.game.pencil = false;
                    app.game.checking = true;
                    app.game.position.notes[cell] = 0;
                    app.game.position.board[cell] = app.puzzles[puzzle].solution[cell] % 9 + 1;
                }
                assert_screen(&r);
                r.app_mut().game.position = game::Position {
                    board: r.app().puzzles[puzzle].solution,
                    notes: [0; CELLS],
                    hints: 7,
                    selected: None,
                };
                assert_screen(&r);
                r.app_mut().view = View::Menu;
                assert_screen(&r);
            }
            r.app_mut()
                .draft
                .replace(b"failure fixture".to_vec())
                .unwrap();
            let write = r.app_mut().draft.begin().unwrap();
            r.app_mut()
                .draft
                .finish(write.revision, Err("No room".into()));
            for view in [View::Play, View::Menu] {
                r.app_mut().view = view;
                assert_screen(&r);
            }
            r.app_mut().view = View::Help;
            for page in 0..r.app().help_pages(&r.context()).len() {
                r.app_mut().help_page = page;
                assert_screen(&r);
            }
            if width < height {
                r.app_mut().loaded = false;
                for error in [
                    kobo_state::record::Error::TooLarge,
                    kobo_state::record::Error::Corrupt,
                    kobo_state::record::Error::NewerVersion,
                ] {
                    r.app_mut().load_error = Some(error.to_string());
                    assert_screen(&r);
                }
            }
        }
    }
}

use super::*;
use kobo_sdk::{AppRunner, Command, StoreError, StoreRequest};
use kobo_ui::{Chrome, DisplayMetrics, TextScale};

#[test]
fn corpus_crossings_numbering_and_all_answers_are_consistent() {
    for p in PUZZLES {
        assert_eq!(p.answer.len(), p.side * p.side);
        assert_eq!(p.across.len(), p.side);
        assert_eq!(p.down.len(), p.side);
        if p.answer.contains(&b'#') {
            let mut words = std::collections::BTreeSet::new();
            for down in [false, true] {
                for n in 0..p.side {
                    let first = if down { n } else { n * p.side };
                    let word = p
                        .word(first, down)
                        .iter()
                        .map(|c| p.answer[*c])
                        .collect::<Vec<_>>();
                    assert!(
                        word.len() >= 3 && words.insert(word),
                        "no short or repeated entries"
                    );
                }
            }
            let down = (0..p.side)
                .map(|n| {
                    String::from_utf8(p.word(n, true).iter().map(|c| p.answer[*c]).collect())
                        .unwrap()
                })
                .collect::<Vec<_>>();
            assert_eq!(down, ["COW", "ARE", "TEDDY", "GOO", "EMU"]);
        } else {
            for row in 0..p.side {
                for column in 0..p.side {
                    assert_eq!(
                        p.answer[row * p.side + column],
                        p.answer[column * p.side + row]
                    );
                }
            }
        }
        let mut g = Progress::new(p);
        for row in 0..p.side {
            g.position.selected = (row * p.side..(row + 1) * p.side)
                .find(|c| p.answer[*c] != b'#')
                .unwrap();
            g.enter(
                p,
                std::str::from_utf8(&p.answer[row * p.side..(row + 1) * p.side])
                    .unwrap()
                    .trim_matches('#'),
            )
            .unwrap();
        }
        assert!(g.solved(p));
        assert!(g.solved_once);
        assert_eq!(
            p.clue(p.side, false).0,
            format!("{} Across", p.number(p.side).unwrap())
        );
    }
}
#[test]
fn edits_check_reveal_and_restart_history_round_trip() {
    let p = &PUZZLES[0];
    let mut games = PUZZLES.iter().map(Progress::new).collect::<Vec<_>>();
    let g = &mut games[0];
    assert!(g.enter(p, "1cat").is_err());
    assert!(g.undo.is_empty());
    g.enter(p, "dog").unwrap();
    assert!(g.check(p).starts_with("3 incorrect"));
    assert_eq!(g.position.letters[..3], *b"DOG");
    g.reveal(p);
    assert_eq!(g.position.letters[0], b'C');
    g.position = g.undo.pop_back().unwrap();
    assert_eq!(g.reveals, 1);
    assert_eq!(g.position.letters[0], b'D');
    g.remember();
    g.position.letters.fill(b'.');
    let bytes = saved::encode(&games, 0);
    let (mut restored, current) = saved::decode(&bytes).unwrap();
    assert_eq!(current, 0);
    assert_eq!(restored, games);
    restored[0].position = restored[0].undo.pop_back().unwrap();
    assert_eq!(restored[0].position.letters[0], b'D');
    assert!(saved::decode(&vec![b'x'; saved::LIMIT + 1]).is_err());
    assert!(saved::decode(&bytes[..bytes.len() - 1]).is_err());
}
#[test]
fn legacy_progress_preserved_and_corrupt_values_refused() {
    let bytes = b"H........................;0;1";
    let (games, current) = saved::decode(bytes).unwrap();
    assert_eq!(current, 2);
    assert_eq!(games[2].position.letters[0], b'H');
    assert!(games[2].position.down);
    for bad in [
        b"H........................;25;1".as_slice(),
        b"H........................;0;2",
        b"bad",
    ] {
        assert!(saved::decode(bad).is_err());
    }
    let future = String::from_utf8(saved::encode(&games, current))
        .unwrap()
        .replace("\"version\":1", "\"version\":99");
    assert!(saved::decode(future.as_bytes()).is_err());
}
fn ready() -> AppRunner<Crossword> {
    let mut r = AppRunner::new(Crossword::default());
    r.start();
    r.store_result(StoreResult::Loaded {
        key: saved::KEY.into(),
        value: None,
    });
    r
}
fn write(commands: Vec<Command>) -> Vec<u8> {
    commands
        .into_iter()
        .find_map(|c| match c {
            Command::Store(StoreRequest::Save { value, .. }) => Some(value),
            _ => None,
        })
        .expect("save")
}
#[test]
fn failed_write_keeps_latest_edit_and_retry_acknowledges_exact_bytes() {
    let mut r = ready();
    r.action(action_id("puzzle-0"));
    let first = write(r.action(action_id("cell-1")));
    r.action(action_id("direction"));
    assert!(!r.app().can_suspend());
    r.store_result(StoreResult::Denied(StoreError::NoRoom));
    let latest = write(r.action(action_id("retry-save")));
    assert_ne!(latest, first);
    assert!(saved::decode(&latest).unwrap().0[0].position.down);
    r.store_result(StoreResult::Saved {
        key: saved::KEY.into(),
    });
    assert!(r.app().can_suspend());
}
#[test]
fn unreadable_save_never_replaced_by_taps() {
    let mut r = AppRunner::new(Crossword::default());
    r.start();
    r.store_result(StoreResult::Loaded {
        key: saved::KEY.into(),
        value: Some(b"bad".to_vec()),
    });
    for name in ["puzzle-0", "cell-0", "confirm-restart"] {
        assert!(!r
            .action(action_id(name))
            .iter()
            .any(|c| matches!(c, Command::Store(StoreRequest::Save { .. }))));
    }
    assert!(!r.app().loaded);
}
#[test]
fn every_view_fits_all_text_sizes_on_both_portrait_profiles() {
    for (width, height, ppi) in [(1072, 1448, 300), (758, 1024, 212)] {
        for text_scale in TextScale::STEPS {
            let metrics = DisplayMetrics {
                width,
                height,
                pixels_per_inch: ppi,
                text_scale,
            };
            let context = AppRunner::with_metrics(Crossword::default(), metrics).context();
            let mut app = Crossword {
                loaded: true,
                ..Crossword::default()
            };
            for (index, puzzle) in PUZZLES.iter().enumerate() {
                app.current = index;
                for view in [
                    View::Puzzles,
                    View::Board,
                    View::Entry,
                    View::Clues,
                    View::More,
                    View::Reveal,
                    View::Restart,
                    View::Result,
                    View::Help,
                ] {
                    app.view = view;
                    let screen = app.screen(&context);
                    let chrome = Chrome::measuring(true);
                    let issues = screen.diagnostics(&metrics, &chrome).issues;
                    assert!(
                        issues.is_empty(),
                        "view={} puzzle={index} {metrics:?}: {issues:?}",
                        view as u8
                    );
                    if view == View::Board {
                        let layout = screen.layout_with(&metrics, &chrome);
                        for cell in (0..puzzle.answer.len()).filter(|c| puzzle.answer[*c] != b'#') {
                            assert!(layout
                                .rect_of_action(action_id(&format!("cell-{cell}")))
                                .is_some());
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn crossword_requests_portrait_and_help_returns_to_the_library() {
    let mut r = AppRunner::new(Crossword::default());
    let commands = r.start();
    assert_eq!(
        commands.first(),
        Some(&Command::SetOrientation(kobo_sdk::Orientation::Portrait))
    );
    r.store_result(StoreResult::Loaded {
        key: saved::KEY.into(),
        value: None,
    });
    r.action(action_id("help"));
    r.action(ActionId::BACK);
    assert!(r.app().view == View::Puzzles);
}
#[test]
fn every_clue_entry_and_validation_message_fits_at_largest_text() {
    for (width, height, pixels_per_inch) in [(1072, 1448, 300), (758, 1024, 212)] {
        let metrics = DisplayMetrics {
            width,
            height,
            pixels_per_inch,
            text_scale: TextScale::Largest,
        };
        let context = AppRunner::with_metrics(Crossword::default(), metrics).context();
        let mut app = Crossword {
            loaded: true,
            view: View::Entry,
            ..Crossword::default()
        };
        for (index, p) in PUZZLES.iter().enumerate() {
            app.current = index;
            for cell in (0..p.answer.len()).filter(|c| p.answer[*c] != b'#') {
                for down in [false, true] {
                    app.game_mut().position.selected = cell;
                    app.game_mut().position.down = down;
                    app.keyboard = Keyboard::with_text("AB");
                    app.notice = Some("Enter one letter or the whole word.".into());
                    let issues = app
                        .screen(&context)
                        .diagnostics(&metrics, &Chrome::measuring(true))
                        .issues;
                    assert!(
                        issues.is_empty(),
                        "puzzle={index} cell={cell} down={down} {metrics:?}: {issues:?}"
                    );
                }
            }
        }
    }
}
#[test]
fn black_squares_are_not_touch_targets_and_do_not_change_progress() {
    let mut app = Crossword {
        loaded: true,
        current: 3,
        view: View::Board,
        ..Crossword::default()
    };
    let mut context = Context::default();
    let before = app.games.clone();
    for cell in (0..25).filter(|c| PUZZLES[3].answer[*c] == b'#') {
        assert!(app
            .screen(&context)
            .layout_with(&kobo_ui::CLARA_BW_METRICS, &Chrome::measuring(true))
            .rect_of_action(action_id(&format!("cell-{cell}")))
            .is_none());
        app.on_action(&mut context, action_id(&format!("cell-{cell}")));
    }
    assert_eq!(app.games, before);
}
#[test]
fn all_histories_fit_the_record_bound_and_undo_stays_bounded() {
    let mut games = PUZZLES.iter().map(Progress::new).collect::<Vec<_>>();
    for (g, p) in games.iter_mut().zip(PUZZLES) {
        for _ in 0..100 {
            g.enter(p, "X").unwrap();
        }
        assert_eq!(g.undo.len(), game::HISTORY);
    }
    let encoded = saved::encode(&games, 3);
    assert!(encoded.len() < saved::LIMIT);
    assert_eq!(saved::decode(&encoded).unwrap().0, games);
}

#[test]
fn revealing_a_correct_guess_still_counts_as_assistance() {
    let p = &PUZZLES[3];
    let mut game = Progress::new(p);
    game.position.letters[0] = p.answer[0];
    game.reveal(p);
    assert_eq!(game.reveals, 1);
    game.position = game.undo.pop_back().unwrap();
    assert_eq!(game.reveals, 1);
    assert_eq!(game.position.letters[0], b'C');
}

#[test]
fn completed_board_clears_active_word_highlighting() {
    let mut app = Crossword {
        loaded: true,
        view: View::Board,
        ..Crossword::default()
    };
    let context = AppRunner::new(Crossword::default()).context();
    let selected = |screen: Screen| {
        screen
            .nodes
            .iter()
            .filter_map(|node| match node {
                kobo_sdk::Node::Grid { cells, .. } => {
                    Some(cells.iter().filter(|cell| cell.selected).count())
                }
                _ => None,
            })
            .sum::<usize>()
    };
    assert!(selected(app.screen(&context)) > 0);
    app.game_mut().position.letters = PUZZLES[app.current].answer.to_vec();
    assert!(app.game().solved(&PUZZLES[app.current]));
    assert_eq!(selected(app.screen(&context)), 0);
    app.game_mut().position.letters[0] = b'.';
    assert!(
        selected(app.screen(&context)) > 0,
        "reopening an answer restores its highlight"
    );
}

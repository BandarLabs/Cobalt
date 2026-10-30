use super::*;
use kobo_sdk::game as chrome;
use kobo_sdk::{AppRunner, Command, StoreError, StoreRequest};
use kobo_ui::{Chrome, DisplayMetrics, TextScale};

#[test]
fn the_clue_above_the_grid_names_the_word_the_grid_highlights() {
    // The line said "1 Down" while the top row was shaded, because the
    // direction was recomputed from itself and came out true whenever the
    // square had any entry at all.
    for puzzle in PUZZLES {
        for cell in 0..puzzle.answer.len() {
            if puzzle.answer[cell] == b'#' {
                continue;
            }
            for down in [false, true] {
                let word = puzzle.word(cell, down);
                let (label, clue) = puzzle.clue(cell, down);
                assert!(
                    !label.is_empty() && !clue.is_empty(),
                    "{}: {cell}",
                    puzzle.id
                );
                let reads_down = label.ends_with("Down");
                assert_eq!(
                    word,
                    puzzle.run(cell, reads_down),
                    "{}: square {cell} is highlighted as {label}",
                    puzzle.id
                );
                let number: usize = label
                    .split_whitespace()
                    .next()
                    .and_then(|first| first.parse().ok())
                    .expect("a numbered clue");
                assert_eq!(
                    puzzle.number(word[0]),
                    Some(number),
                    "{}: {label} does not start at its own number",
                    puzzle.id
                );
            }
        }
    }
}

#[test]
fn every_puzzle_is_ruled_the_way_a_printed_grid_is() {
    // What a newspaper will print: no entry shorter than three squares, no
    // answer twice in one puzzle, a clue for every entry, and black squares
    // placed in pairs that read the same upside down. The grid that broke
    // these rules had two by two slabs in its corners, which no paper sets.
    for puzzle in PUZZLES {
        assert_eq!(puzzle.answer.len(), puzzle.side * puzzle.side);
        let entries = puzzle.entries();
        assert!(!entries.is_empty(), "{}: no entries", puzzle.id);

        // A word square is every row and every column a word, so it repeats
        // its answers by construction and is clued twice over. It is a
        // legitimate puzzle and says so on its row in the library; the rule
        // against a repeated answer belongs to the grids that call themselves
        // crosswords.
        let square = !puzzle.answer.contains(&b'#');
        let mut answers = std::collections::BTreeSet::new();
        for entry in &entries {
            let cells = puzzle.run(entry.first, entry.down);
            assert!(
                cells.len() >= 3,
                "{}: {} {} is only {} squares",
                puzzle.id,
                entry.number,
                if entry.down { "down" } else { "across" },
                cells.len()
            );
            let word: Vec<u8> = cells.iter().map(|cell| puzzle.answer[*cell]).collect();
            assert!(
                answers.insert(word.clone()) || square,
                "{}: {} appears twice",
                puzzle.id,
                String::from_utf8_lossy(&word)
            );
            assert!(
                !puzzle.clue_of(entry).is_empty(),
                "{}: {} {} has no clue",
                puzzle.id,
                entry.number,
                if entry.down { "down" } else { "across" }
            );
        }

        // Every white square belongs to an across entry and a down entry, so
        // no letter is unchecked and no square is unreachable.
        for cell in 0..puzzle.answer.len() {
            if puzzle.answer[cell] == b'#' {
                continue;
            }
            assert!(
                puzzle.run(cell, false).len() >= 3 || puzzle.run(cell, true).len() >= 3,
                "{}: square {cell} is in no entry",
                puzzle.id
            );
        }

        // Black squares in pairs about the centre, and never four in a block.
        let side = puzzle.side;
        for cell in 0..puzzle.answer.len() {
            if puzzle.answer[cell] != b'#' {
                continue;
            }
            let opposite = puzzle.answer.len() - 1 - cell;
            assert_eq!(
                puzzle.answer[opposite], b'#',
                "{}: the black square at {cell} has no partner",
                puzzle.id
            );
            let (row, column) = (cell / side, cell % side);
            if row + 1 < side && column + 1 < side {
                let block = [cell, cell + 1, cell + side, cell + side + 1];
                assert!(
                    !block.iter().all(|square| puzzle.answer[*square] == b'#'),
                    "{}: four black squares meet at {cell}",
                    puzzle.id
                );
            }
        }
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
        .replace("\"version\":2", "\"version\":99");
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
    // The screens whose words come from the puzzle are drawn for every
    // puzzle; the rest look the same whichever puzzle is open.
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
                let views: &[View] = if index == 3 {
                    &[
                        View::Puzzles,
                        View::Board,
                        View::Entry,
                        View::Clues,
                        View::Reveal,
                        View::Restart,
                        View::Result,
                        View::Rules,
                    ]
                } else {
                    &[View::Board, View::Clues]
                };
                for view in views {
                    app.view = *view;
                    for open in [false, true] {
                        if open && *view != View::Board {
                            continue;
                        }
                        app.menu = Menu::default();
                        if open {
                            app.menu.read(action_id(chrome::MORE));
                        }
                        let screen = app.screen(&context);
                        let chrome = Chrome::measuring(true);
                        let issues = screen.diagnostics(&metrics, &chrome).issues;
                        assert!(
                            issues.is_empty(),
                            "view={} menu={open} puzzle={index} {metrics:?}: {issues:?}",
                            *view as u8
                        );
                        if *view == View::Board && !open {
                            let layout = screen.layout_with(&metrics, &chrome);
                            for cell in
                                (0..puzzle.answer.len()).filter(|c| puzzle.answer[*c] != b'#')
                            {
                                assert!(layout
                                    .rect_of_action(action_id(&format!("cell-{cell}")))
                                    .is_some());
                            }
                        }
                    }
                }
            }
            app.menu = Menu::default();
        }
    }
}

#[test]
fn crossword_requests_portrait_and_rules_from_the_shelf_return_to_the_shelf() {
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
    r.action(action_id(chrome::RULES));
    assert!(r.app().view == View::Rules);
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
        // Collected rather than stopping at the first, so that a clue too long
        // for the panel is reported together with every other one.
        let mut overflowing = std::collections::BTreeSet::new();
        for (index, p) in PUZZLES.iter().enumerate() {
            app.current = index;
            // Every square of an entry shows the same clue, so one square per
            // entry covers every clue the screen can show.
            for entry in p.entries() {
                app.game_mut().position.selected = entry.first;
                app.game_mut().position.down = entry.down;
                app.keyboard = Keyboard::with_text("AB");
                app.notice = Some("Enter one letter or the whole word.".into());
                let issues = app
                    .screen(&context)
                    .diagnostics(&metrics, &Chrome::measuring(true))
                    .issues;
                if !issues.is_empty() {
                    overflowing.insert(p.clue_of(&entry));
                }
            }
        }
        assert!(overflowing.is_empty(), "{metrics:?}: {overflowing:#?}");
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
fn every_puzzle_played_with_full_history_still_fits_and_keeps_the_current_undo() {
    let mut games = PUZZLES.iter().map(Progress::new).collect::<Vec<_>>();
    for (g, p) in games.iter_mut().zip(PUZZLES) {
        for _ in 0..100 {
            g.enter(p, "X").unwrap();
        }
        assert_eq!(g.undo.len(), game::HISTORY);
    }
    let current = PUZZLES.len() - 1;
    let encoded = saved::encode(&games, current);
    assert!(encoded.len() <= saved::LIMIT);
    let (restored, at) = saved::decode(&encoded).unwrap();
    assert_eq!(at, current);
    assert_eq!(restored[current], games[current]);
    for (restored, played) in restored.iter().zip(&games) {
        assert_eq!(restored.position, played.position, "letters always fit");
    }
}

#[test]
fn a_few_played_puzzles_keep_every_undo_step() {
    let mut games = PUZZLES.iter().map(Progress::new).collect::<Vec<_>>();
    for index in [0, 3, 10] {
        for _ in 0..40 {
            games[index].enter(&PUZZLES[index], "X").unwrap();
        }
    }
    let encoded = saved::encode(&games, 3);
    assert_eq!(saved::decode(&encoded).unwrap().0, games);
}

#[test]
fn a_save_names_only_the_puzzles_that_have_been_played() {
    let mut games = PUZZLES.iter().map(Progress::new).collect::<Vec<_>>();
    games[20].enter(&PUZZLES[20], "X").unwrap();
    let text = String::from_utf8(saved::encode(&games, 20)).unwrap();
    assert_eq!(text.matches("\"id\":").count(), 1);
    assert!(text.contains(PUZZLES[20].id));
    assert_eq!(saved::decode(text.as_bytes()).unwrap(), (games, 20));
}

#[test]
fn a_save_from_the_four_puzzle_version_opens_with_every_new_puzzle_fresh() {
    // Version 1 wrote all four puzzles of its day, in order, on every save.
    let mut games = PUZZLES.iter().map(Progress::new).collect::<Vec<_>>();
    for (index, game) in games.iter_mut().enumerate().take(4) {
        game.checks = u32::try_from(index).unwrap() + 1;
    }
    games[3].enter(&PUZZLES[3], "put").unwrap();
    let old = String::from_utf8(saved::encode(&games, 3))
        .unwrap()
        .replace("\"version\":2", "\"version\":1");
    assert_eq!(old.matches("\"id\":").count(), 4);
    let (restored, current) = saved::decode(old.as_bytes()).unwrap();
    assert_eq!(current, 3);
    assert_eq!(restored, games);
    assert!(restored[4..]
        .iter()
        .zip(&PUZZLES[4..])
        .all(|(game, puzzle)| *game == Progress::new(puzzle)));
}

#[test]
fn a_save_naming_a_puzzle_twice_or_one_this_version_lacks_is_refused() {
    let mut games = PUZZLES.iter().map(Progress::new).collect::<Vec<_>>();
    games[5].enter(&PUZZLES[5], "X").unwrap();
    let text = String::from_utf8(saved::encode(&games, 5)).unwrap();
    let unknown = text.replace(PUZZLES[5].id, "mini-999");
    assert!(saved::decode(unknown.as_bytes()).is_err());
    games[4].enter(&PUZZLES[4], "X").unwrap();
    let twice = String::from_utf8(saved::encode(&games, 5))
        .unwrap()
        .replace(PUZZLES[4].id, PUZZLES[5].id)
        .replace(
            std::str::from_utf8(PUZZLES[4].answer).unwrap(),
            std::str::from_utf8(PUZZLES[5].answer).unwrap(),
        );
    assert!(saved::decode(twice.as_bytes()).is_err());
}

#[test]
fn revealing_a_correct_guess_still_counts_as_assistance() {
    let p = &PUZZLES[3];
    let mut game = Progress::new(p);
    // The first white square of the grid, which is not square zero now that
    // the corners are black.
    let first = p
        .answer
        .iter()
        .position(|square| *square != b'#')
        .expect("a white square");
    game.position.selected = first;
    game.position.letters[first] = p.answer[first];
    game.reveal(p);
    assert_eq!(game.reveals, 1);
    game.position = game.undo.pop_back().unwrap();
    assert_eq!(game.reveals, 1);
    assert_eq!(game.position.letters[first], p.answer[first]);
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

#[test]
fn the_first_four_puzzles_keep_their_place_because_old_saves_name_them_by_position() {
    let ids: Vec<&str> = PUZZLES.iter().take(4).map(|p| p.id).collect();
    assert_eq!(ids, ["cat-v1", "ball-v1", "heart-v1", "mini-v1"]);
    assert_eq!(PUZZLES[3].answer, b"#PUT#MINORALIKENOTED#TEN#");
    assert!(PUZZLES.len() >= 60, "{} puzzles", PUZZLES.len());
    let mut ids = std::collections::BTreeSet::new();
    for puzzle in PUZZLES {
        assert!(ids.insert(puzzle.id), "{} appears twice", puzzle.id);
    }
}

#[test]
fn every_entry_has_exactly_one_clue_and_no_clue_gives_its_answer_away() {
    for puzzle in PUZZLES {
        let entries = puzzle.entries();
        let across = entries.iter().filter(|entry| !entry.down).count();
        assert_eq!(puzzle.across.len(), across, "{}: across clues", puzzle.id);
        assert_eq!(
            puzzle.down.len(),
            entries.len() - across,
            "{}: down clues",
            puzzle.id
        );
        if !puzzle.answer.contains(&b'#') {
            continue;
        }
        for entry in &entries {
            let word: String = puzzle
                .run(entry.first, entry.down)
                .iter()
                .map(|cell| char::from(puzzle.answer[*cell]))
                .collect();
            let clue = puzzle.clue_of(entry).to_ascii_uppercase();
            assert!(
                !clue
                    .split(|c: char| !c.is_ascii_alphabetic())
                    .any(|part| part == word),
                "{}: the clue for {word} contains it",
                puzzle.id
            );
            assert!(!clue.contains('\u{2014}'), "{}: dash in a clue", puzzle.id);
        }
    }
}

#[test]
fn no_answer_turns_up_more_often_than_the_generator_allows() {
    // The same limits tools/crossword/generate.py fills to, so that a hand
    // edit of the generated file cannot quietly make the puzzles samey.
    let mut uses = std::collections::BTreeMap::<String, usize>::new();
    for puzzle in PUZZLES.iter().filter(|p| p.answer.contains(&b'#')) {
        for entry in puzzle.entries() {
            let word: String = puzzle
                .run(entry.first, entry.down)
                .iter()
                .map(|cell| char::from(puzzle.answer[*cell]))
                .collect();
            *uses.entry(word).or_default() += 1;
        }
    }
    for (word, count) in uses {
        let cap = if word.len() == 3 { 3 } else { 2 };
        assert!(count <= cap, "{word} is used {count} times");
    }
}

fn on_board(index: usize) -> AppRunner<Crossword> {
    let mut r = ready();
    r.action(action_id(&format!("puzzle-{index}")));
    assert!(r.app().view == View::Board);
    r
}

fn notice_line(r: &AppRunner<Crossword>) -> String {
    let screen = r.app().screen(&r.context());
    screen
        .nodes
        .iter()
        .find_map(|node| match node {
            kobo_sdk::Node::Secondary { text, .. } => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

#[test]
fn undo_sits_first_at_the_foot_of_the_grid_and_answers_when_there_is_nothing_to_undo() {
    let mut r = on_board(3);
    let screen = r.app().screen(&r.context());
    let bar = screen.nav_bar.expect("the play screen has a foot bar");
    let labels: Vec<_> = bar.destinations.iter().map(|a| a.label.as_str()).collect();
    assert_eq!(labels, [chrome::UNDO_LABEL, "Clues", "Down"]);

    r.action(action_id(chrome::UNDO));
    assert_eq!(notice_line(&r), chrome::NOTHING_TO_UNDO);
    r.action(action_id("cell-1"));
    r.app_mut().keyboard = Keyboard::with_text("P");
    r.action(action_id("kb.enter"));
    r.action(action_id("board"));
    assert_eq!(r.app().game().position.letters[1], b'P');
    r.action(action_id(chrome::UNDO));
    assert_eq!(r.app().game().position.letters[1], b'.');
    assert!(r.app().view == View::Board);
}

#[test]
fn the_direction_switch_is_one_tap_away_at_the_foot_of_the_grid() {
    let mut r = on_board(3);
    assert!(!r.app().game().position.down);
    r.action(action_id("direction"));
    assert!(r.app().game().position.down);
    assert!(notice_line(&r).contains("Down"));
    let screen = r.app().screen(&r.context());
    let bar = screen.nav_bar.expect("foot bar");
    assert_eq!(bar.destinations[2].label, "Across");
}

#[test]
fn rules_open_in_one_tap_from_the_top_bar_and_back_returns_to_the_grid() {
    let mut r = on_board(3);
    let screen = r.app().screen(&r.context());
    let bar = screen.top_bar.expect("top bar");
    let labels: Vec<_> = bar.actions.iter().map(|a| a.label.as_str()).collect();
    assert_eq!(labels, [chrome::RULES_LABEL, "More"]);
    r.action(action_id(chrome::RULES));
    assert!(r.app().view == View::Rules);
    r.action(ActionId::BACK);
    assert!(r.app().view == View::Board);
}

#[test]
fn the_menu_starts_with_new_game_and_no_longer_holds_undo_or_the_rules() {
    let mut r = on_board(3);
    r.action(action_id(chrome::MORE));
    let screen = r.app().screen(&r.context());
    let labels: Vec<String> = screen
        .overlay
        .expect("the menu is open")
        .nodes
        .iter()
        .filter_map(|node| match node {
            kobo_sdk::Node::Button { label, .. } => Some(label.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(labels[0], chrome::NEW_GAME_LABEL);
    assert!(labels
        .iter()
        .all(|label| label != "Undo" && label != "How to play"));
    // A tap outside the menu closes it rather than leaving the puzzle.
    r.action(ActionId::BACK);
    assert!(r.app().view == View::Board);
    assert!(!r.app().menu.is_open());
}

#[test]
fn new_game_opens_the_next_puzzle_nobody_has_started() {
    let mut r = on_board(3);
    r.action(action_id("cell-1"));
    r.app_mut().keyboard = Keyboard::with_text("P");
    r.action(action_id("kb.enter"));
    r.action(action_id("board"));
    r.action(action_id(chrome::MORE));
    r.action(action_id(chrome::NEW_GAME));
    // The shelf puts the hand-written crossword first, then the word squares.
    assert_eq!(r.app().current, 0);
    assert!(r.app().view == View::Board);
    assert_eq!(r.app().games[3].position.letters[1], b'P', "kept");
}

#[test]
fn clearing_an_empty_square_says_so_instead_of_doing_nothing() {
    let mut r = on_board(3);
    r.action(action_id(chrome::MORE));
    r.action(action_id("clear"));
    assert_eq!(notice_line(&r), "This square is already empty.");
}

#[test]
fn a_square_in_one_word_only_carries_no_number() {
    // The generated grids have squares at the ends of short rows that belong
    // to one word; numbering them pushed every later clue number out by one.
    for puzzle in PUZZLES {
        let numbers: Vec<usize> = (0..puzzle.answer.len())
            .filter_map(|cell| puzzle.number(cell))
            .collect();
        let expected: Vec<usize> = (1..=numbers.len()).collect();
        assert_eq!(numbers, expected, "{}", puzzle.id);
        for entry in puzzle.entries() {
            assert_eq!(puzzle.number(entry.first), Some(entry.number));
        }
        let starts: std::collections::BTreeSet<usize> =
            puzzle.entries().iter().map(|entry| entry.first).collect();
        assert_eq!(starts.len(), numbers.len(), "{}", puzzle.id);
    }
}

/// The splash a whole-screen failure draws and the one control directly
/// under it, checked on the panel at this size.
fn assert_failure_splash(screen: &Screen, metrics: DisplayMetrics, retry: &str, label: &str) {
    assert!(screen.bottom_action.is_none(), "{metrics:?}");
    let splash = screen
        .nodes
        .iter()
        .position(|node| matches!(node, kobo_ui::Node::Splash { .. }))
        .expect("the failure is a splash");
    // The mark gives way only where a small panel at a large size needs the
    // room; on a Clara at the default size it is there.
    if metrics.text_scale == kobo_ui::TextScale::Default && metrics.pixels_per_inch == 300 {
        assert!(matches!(
            &screen.nodes[splash],
            kobo_ui::Node::Splash {
                glyph: Some(Glyph::Close),
                ..
            }
        ));
    }
    assert!(
        matches!(
            &screen.nodes[splash + 1],
            kobo_ui::Node::Button {
                label: drawn,
                emphasis: kobo_ui::Emphasis::Primary,
                ..
            } if drawn == label
        ),
        "{metrics:?}: {:?}",
        screen.nodes
    );
    let chrome = Chrome::measuring(true);
    let issues = screen.diagnostics(&metrics, &chrome).issues;
    assert!(issues.is_empty(), "{metrics:?}: {issues:?}");
    let layout = screen.layout_with(&metrics, &chrome);
    let reason = layout
        .nodes
        .iter()
        .find(|node| matches!(node.kind, kobo_ui::LayoutKind::SplashText))
        .expect("the reason is drawn")
        .rect;
    let button = layout
        .rect_of_action(action_id(retry))
        .expect("the fix is on the panel");
    assert!(button.y >= reason.y + reason.height, "{metrics:?}");
}

/// An unreadable record and a refused save each fill the screen, so each is
/// a splash with its one fix directly under the reason: Try again for the
/// read, Retry saving for the write. While the save is refused the board
/// still ignores every tap but the retry.
#[test]
fn load_and_save_failures_are_splashes_with_their_fix_directly_under_them() {
    for (width, height, ppi) in [(1072, 1448, 300), (758, 1024, 212)] {
        for text_scale in TextScale::STEPS {
            let metrics = DisplayMetrics {
                width,
                height,
                pixels_per_inch: ppi,
                text_scale,
            };
            let mut r = AppRunner::with_metrics(Crossword::default(), metrics);
            r.start();
            r.store_result(StoreResult::Loaded {
                key: saved::KEY.into(),
                value: Some(b"bad".to_vec()),
            });
            assert_failure_splash(
                &r.app().screen(&r.context()),
                metrics,
                "retry-load",
                "Try again",
            );

            let mut r = AppRunner::with_metrics(Crossword::default(), metrics);
            r.start();
            r.store_result(StoreResult::Loaded {
                key: saved::KEY.into(),
                value: None,
            });
            r.action(action_id("puzzle-0"));
            r.action(action_id("cell-1"));
            r.store_result(StoreResult::Denied(StoreError::NoRoom));
            assert_failure_splash(
                &r.app().screen(&r.context()),
                metrics,
                "retry-save",
                "Retry saving",
            );
            let before = r.app().games.clone();
            assert!(r.action(action_id("direction")).is_empty());
            assert_eq!(r.app().games, before);
        }
    }
}

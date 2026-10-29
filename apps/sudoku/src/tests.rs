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
fn menu_labels(screen: &Screen) -> Vec<String> {
    screen
        .overlay
        .as_ref()
        .map(|overlay| {
            overlay
                .nodes
                .iter()
                .filter_map(|node| match node {
                    kobo_ui::Node::Button { label, .. } => Some(label.clone()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}
fn shown(r: &AppRunner<Sudoku>) -> Screen {
    r.app().screen(&r.context())
}
const fn house(cell: usize) -> [usize; 3] {
    [cell / 9, cell % 9, cell / 27 * 3 + cell % 9 / 3]
}
/// Counts the solutions of a grid, stopping at two.
///
/// Each row, column and box keeps the digits it already holds as bits, and
/// the search always branches on the square with the fewest digits left, so
/// the whole pack is checked in well under a second even in a debug build.
struct Solutions {
    grid: [u8; CELLS],
    used: [[u16; 9]; 3],
    found: usize,
    first: Option<[u8; CELLS]>,
}
impl Solutions {
    fn of(clues: &[u8; CELLS]) -> Self {
        let mut counter = Self {
            grid: *clues,
            used: [[0; 9]; 3],
            found: 0,
            first: None,
        };
        let mut clash = false;
        for (cell, &digit) in clues.iter().enumerate() {
            if digit != 0 {
                let bit = 1 << digit;
                for (kind, index) in house(cell).into_iter().enumerate() {
                    clash |= counter.used[kind][index] & bit != 0;
                    counter.used[kind][index] |= bit;
                }
            }
        }
        if !clash {
            counter.search();
        }
        counter
    }
    fn free(&self, cell: usize) -> u16 {
        let [row, column, boxed] = house(cell);
        0x3fe & !(self.used[0][row] | self.used[1][column] | self.used[2][boxed])
    }
    fn search(&mut self) {
        let mut best: Option<(usize, u16)> = None;
        for cell in (0..CELLS).filter(|&cell| self.grid[cell] == 0) {
            let free = self.free(cell);
            if best.is_none_or(|(_, old)| free.count_ones() < old.count_ones()) {
                best = Some((cell, free));
                if free.count_ones() <= 1 {
                    break;
                }
            }
        }
        let Some((cell, free)) = best else {
            self.found += 1;
            self.first.get_or_insert(self.grid);
            return;
        };
        for digit in (1..=9).filter(|&digit| free & (1 << digit) != 0) {
            let bit = 1 << digit;
            self.grid[cell] = digit;
            for (kind, index) in house(cell).into_iter().enumerate() {
                self.used[kind][index] |= bit;
            }
            self.search();
            for (kind, index) in house(cell).into_iter().enumerate() {
                self.used[kind][index] &= !bit;
            }
            self.grid[cell] = 0;
            if self.found >= 2 {
                return;
            }
        }
    }
}
#[test]
fn every_shipped_puzzle_has_exactly_one_solution_and_it_agrees_with_its_givens() {
    let puzzles = game::pack();
    assert_eq!(
        puzzles
            .iter()
            .map(|p| p.clues)
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        puzzles.len(),
        "no puzzle appears twice"
    );
    for (line, p) in puzzles.iter().enumerate() {
        let line = line + 1;
        assert!(
            (0..CELLS).all(|i| p.clues[i] == 0 || p.clues[i] == p.solution[i]),
            "line {line}: a given disagrees with the solution"
        );
        let full = Solutions::of(&p.solution);
        assert_eq!(full.found, 1, "line {line}: the solution breaks a rule");
        let solved = Solutions::of(&p.clues);
        assert_eq!(solved.found, 1, "line {line}: needs exactly one solution");
        assert_eq!(
            solved.first,
            Some(p.solution),
            "line {line}: solves to something else"
        );
    }
}
#[test]
fn each_difficulty_ships_hundreds_of_puzzles_numbered_from_one() {
    let puzzles = game::pack();
    let each = puzzles.len() / Level::ALL.len();
    assert!(each >= 200, "{each} per difficulty");
    for level in Level::ALL {
        let numbers: Vec<_> = puzzles
            .iter()
            .filter(|p| p.level == level)
            .map(|p| p.number)
            .collect();
        assert_eq!(numbers, (1..=each).collect::<Vec<_>>(), "{level:?}");
    }
    assert_eq!(puzzles[35].number, 12, "the last original Hard");
    assert_eq!(puzzles[36].level, Level::Easy);
    assert_eq!(puzzles[36].number, 13, "the first new Easy follows them");
}
#[test]
fn the_original_36_puzzles_keep_their_lines_because_saved_games_name_them_by_line() {
    let original: String = include_str!("../assets/puzzles.txt")
        .lines()
        .take(36)
        .flat_map(|line| [line, "\n"])
        .collect();
    let fingerprint = original
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
        });
    assert_eq!(fingerprint, 0x09a3_48c1_533e_c8f3);
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
    assert!(!r.app().status().contains("Wrong"));
    tap(&mut r, chrome::MORE);
    assert!(menu_labels(&shown(&r)).contains(&"Show mistakes".to_owned()));
    tap(&mut r, "checking");
    assert!(!r.app().menu.is_open(), "choosing an entry closes More");
    assert!(r.app().status().starts_with("Wrong answer"));
    tap(&mut r, chrome::MORE);
    assert!(menu_labels(&shown(&r)).contains(&"Hide mistakes".to_owned()));
    tap(&mut r, "checking");
    assert!(!r.app().status().contains("Wrong"));
    assert_eq!(r.app().game.position.board[c], wrong);
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
    assert!(r.app().status().contains("Open More to retry"));
    tap(&mut r, chrome::MORE);
    assert_eq!(
        menu_labels(&shown(&r))[..2],
        [chrome::NEW_GAME_LABEL, "Retry save"]
    );
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
    for (destination, view) in [
        (chrome::NEW_GAME, View::New),
        ("restart", View::Restart),
        ("hint", View::Hint),
    ] {
        tap(&mut r, chrome::MORE);
        tap(&mut r, destination);
        if destination != "hint" || r.app().game.position.board[c] != r.app().puzzles[0].solution[c]
        {
            assert_eq!(r.app().view, view);
        }
        r.action(ActionId::BACK);
        assert_eq!(r.app().view, View::Play);
        assert_eq!(r.app().game, before);
    }
    tap(&mut r, chrome::MORE);
    tap(&mut r, chrome::NEW_GAME);
    tap(&mut r, "new-hard");
    assert_eq!(r.app().puzzles[r.app().game.puzzle].level, Level::Hard);
}
/// A loaded game on a panel of this size, turned the way the panel is.
///
/// The runner is always told the portrait size, as the runtime does, and the
/// game asks for landscape itself, so a landscape panel is a portrait one
/// that the game has turned.
fn on_panel(
    width: i32,
    height: i32,
    ppi: i32,
    scale: TextScale,
) -> (AppRunner<Sudoku>, DisplayMetrics) {
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
    (r, metrics)
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
            let (mut r, metrics) = on_panel(width, height, ppi, scale);
            for (view, menu) in [
                (View::Play, false),
                (View::Play, true),
                (View::New, false),
                (View::Restart, false),
                (View::Help, false),
                (View::Hint, false),
            ] {
                r.app_mut().view = view;
                r.app_mut().menu = Menu::default();
                if menu {
                    r.app_mut().menu.read(action_id(chrome::MORE));
                }
                let screen = r.app().screen(&r.context());
                let frame = Chrome::with_back(true);
                let issues = screen.diagnostics(&metrics, &frame).issues;
                assert!(
                    issues.is_empty(),
                    "{width}x{height} {scale:?} {view:?} menu {menu}: {issues:?}"
                );
                if view == View::Play && !menu {
                    let layout = screen.layout_with(&metrics, &frame);
                    for cell in 0..CELLS {
                        let rect = layout.rect_of_action(action_id(&cell_name(cell))).unwrap();
                        assert_eq!(rect.width, rect.height);
                        assert!(rect.width >= metrics.touch_target_minimum());
                    }
                    for name in [
                        "digit-1",
                        "digit-9",
                        "pencil",
                        "erase",
                        chrome::UNDO,
                        chrome::RULES,
                        chrome::MORE,
                    ] {
                        assert!(layout.rect_of_action(action_id(name)).is_some());
                    }
                    assert_eq!(
                        layout
                            .nodes
                            .iter()
                            .filter(|n| matches!(n.kind, LayoutKind::PencilMark(..)))
                            .count(),
                        CELLS
                    );
                }
            }
        }
    }
    // The 3x3 boxes are ruled heavier at the pencil renderer.
    let r = ready();
    let layout = r
        .app()
        .screen(&r.context())
        .layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true));
    let rect = |c| layout.rect_of_action(action_id(&cell_name(c))).unwrap();
    assert_eq!(rect(1).x - rect(0).x, rect(3).x - rect(2).x);
    assert!(layout
        .nodes
        .iter()
        .any(|n| matches!(n.kind, LayoutKind::PencilMark(_, _, _, mask) if mask != 0)));
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
            let (mut r, metrics) = on_panel(width, height, ppi, scale);
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
            // The last puzzle of each difficulty carries the longest title.
            let last = Level::ALL.map(|level| {
                r.app()
                    .puzzles
                    .iter()
                    .rposition(|p| p.level == level)
                    .unwrap()
            });
            for puzzle in last {
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
                r.app_mut().menu.read(action_id(chrome::MORE));
                assert_screen(&r);
                r.app_mut().menu.close();
            }
            r.app_mut()
                .draft
                .replace(b"failure fixture".to_vec())
                .unwrap();
            let write = r.app_mut().draft.begin().unwrap();
            r.app_mut()
                .draft
                .finish(write.revision, Err("No room".into()));
            r.app_mut().view = View::Play;
            assert_screen(&r);
            r.app_mut().menu.read(action_id(chrome::MORE));
            assert_screen(&r);
            r.app_mut().menu.close();
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

#[test]
fn rules_open_in_one_tap_from_the_puzzle_and_back_returns_to_it() {
    let mut r = ready();
    let bar = shown(&r).top_bar.expect("the puzzle has a top bar");
    assert_eq!(bar.title, "Sudoku · Easy 1");
    assert_eq!(bar.actions[0].label, chrome::RULES_LABEL);
    r.app_mut().help_page = 3;
    tap(&mut r, chrome::RULES);
    assert_eq!(r.app().view, View::Help);
    assert_eq!(r.app().help_page, 0, "the rules open at their first page");
    let rules = shown(&r);
    assert!(rules.owns_back);
    r.action(ActionId::BACK);
    assert_eq!(r.app().view, View::Play);
}
#[test]
fn more_starts_with_new_game_then_restart_mistakes_reveal_and_orientation() {
    let mut r = ready();
    assert!(shown(&r).overlay.is_none());
    tap(&mut r, chrome::MORE);
    assert_eq!(
        menu_labels(&shown(&r)),
        [
            chrome::NEW_GAME_LABEL,
            "Restart puzzle",
            "Show mistakes",
            "Reveal square",
            "Use landscape"
        ]
    );
    let before = r.app().game.clone();
    r.action(ActionId::BACK);
    assert!(!r.app().menu.is_open(), "a tap outside More closes it");
    assert_eq!(r.app().view, View::Play);
    assert_eq!(r.app().game, before);

    tap(&mut r, chrome::MORE);
    tap(&mut r, "rotate");
    assert!(r.app().game.landscape);
    tap(&mut r, chrome::MORE);
    assert_eq!(menu_labels(&shown(&r)).last().unwrap(), "Use portrait");
}
#[test]
fn undo_comes_first_at_the_foot_and_says_so_when_there_is_nothing_to_undo() {
    let mut r = ready();
    let foot = shown(&r).nav_bar.expect("undo and two verbs make a bar");
    let labels: Vec<_> = foot.destinations.iter().map(|a| a.label.as_str()).collect();
    assert_eq!(labels, [chrome::UNDO_LABEL, "Notes", "Erase"]);

    tap(&mut r, chrome::UNDO);
    assert_eq!(r.app().status(), chrome::NOTHING_TO_UNDO);
    let c = blank(&r.app().game, &r.app().puzzles);
    tap(&mut r, &cell_name(c));
    tap(&mut r, "digit-4");
    tap(&mut r, chrome::UNDO);
    assert_eq!(r.app().game.position.board[c], 0);

    tap(&mut r, "pencil");
    let foot = shown(&r).nav_bar.unwrap();
    assert_eq!(foot.destinations[1].label, "Digits");
}
#[test]
fn erase_and_reveal_say_why_when_they_have_nothing_to_do() {
    let mut r = ready();
    tap(&mut r, "erase");
    assert_eq!(r.app().status(), "Choose a square to erase.");
    tap(&mut r, chrome::MORE);
    tap(&mut r, "hint");
    assert_eq!(r.app().view, View::Play);
    assert_eq!(r.app().status(), "Choose a square to reveal.");

    let given = r.app().puzzles[0]
        .clues
        .iter()
        .position(|&n| n != 0)
        .unwrap();
    tap(&mut r, &cell_name(given));
    tap(&mut r, "erase");
    assert_eq!(r.app().status(), "Given numbers cannot change.");

    let c = blank(&r.app().game, &r.app().puzzles);
    tap(&mut r, &cell_name(c));
    tap(&mut r, "erase");
    assert_eq!(r.app().status(), "This square is already empty.");
    let answer = r.app().puzzles[0].solution[c];
    tap(&mut r, &digit_name(answer % 9 + 1));
    tap(&mut r, "pencil");
    tap(&mut r, "digit-1");
    assert_eq!(r.app().status(), "Erase this answer to add notes.");
    tap(&mut r, "erase");
    assert_eq!(r.app().game.position.board[c], 0);

    tap(&mut r, chrome::MORE);
    tap(&mut r, "hint");
    assert_eq!(r.app().view, View::Hint);
    tap(&mut r, "confirm-hint");
    assert_eq!(r.app().game.position.board[c], answer);
    tap(&mut r, chrome::MORE);
    tap(&mut r, "hint");
    assert_eq!(r.app().view, View::Play);
    assert_eq!(r.app().status(), "This square is already right.");
}
#[test]
fn new_game_moves_through_a_difficulty_in_order_before_any_puzzle_repeats() {
    let puzzles = game::pack();
    for level in Level::ALL {
        let each = puzzles.iter().filter(|p| p.level == level).count();
        let mut g = Game::new(0, &puzzles);
        let mut numbers = Vec::new();
        for _ in 0..=each {
            g = g.start(level, &puzzles);
            assert_eq!(puzzles[g.puzzle].level, level);
            numbers.push(puzzles[g.puzzle].number);
        }
        let first = usize::from(level != Level::Easy);
        let expected: Vec<_> = (1..=each).cycle().skip(1 - first).take(each + 1).collect();
        assert_eq!(numbers, expected, "{level:?}");
    }
}
#[test]
fn dropping_into_another_difficulty_does_not_skip_puzzles_in_the_first() {
    let puzzles = game::pack();
    let mut g = Game::new(0, &puzzles);
    g.checking = true;
    g.landscape = true;
    let mut seen = Vec::new();
    for level in [
        Level::Hard,
        Level::Easy,
        Level::Medium,
        Level::Hard,
        Level::Easy,
    ] {
        g = g.start(level, &puzzles);
        seen.push(format!("{:?} {}", level, puzzles[g.puzzle].number));
    }
    assert_eq!(seen, ["Hard 1", "Easy 2", "Medium 1", "Hard 2", "Easy 3"]);
    assert!(
        g.checking && g.landscape,
        "the player's settings carry over"
    );
    let restored = saved::decode(&saved::encode(&g, &puzzles).unwrap(), &puzzles).unwrap();
    assert_eq!(restored, g);
    assert_eq!(
        puzzles[restored.start(Level::Medium, &puzzles).puzzle].number,
        2
    );
}
#[test]
fn a_game_saved_before_the_larger_pack_carries_on_after_the_puzzle_it_was_on() {
    let puzzles = game::pack();
    // Medium 5, saved by a version that recorded no per-difficulty progress.
    let g = Game::new(16, &puzzles);
    let text = String::from_utf8(saved::encode(&g, &puzzles).unwrap()).unwrap();
    let field = text.find("\"recent\":").unwrap();
    let end = field + text[field..].find('}').unwrap() + 1;
    let legacy = format!("{}{}", &text[..field], &text[end..]).replace(",,", ",");
    assert!(!legacy.contains("recent"));
    let restored = saved::decode(legacy.as_bytes(), &puzzles).unwrap();
    assert_eq!(restored.puzzle, 16);
    let next = |level| puzzles[restored.start(level, &puzzles).puzzle].number;
    assert_eq!(next(Level::Medium), 6);
    assert_eq!(
        next(Level::Easy),
        13,
        "the first Easy the old pack never had"
    );
    assert_eq!(next(Level::Hard), 1);
}
#[test]
fn a_progress_entry_naming_a_puzzle_this_pack_lacks_is_dropped_rather_than_refusing_the_game() {
    let puzzles = game::pack();
    let g = Game::new(0, &puzzles).start(Level::Hard, &puzzles);
    let text = String::from_utf8(saved::encode(&g, &puzzles).unwrap()).unwrap();
    let damaged = text.replace("\"easy\":\"0\"", "\"easy\":\"99999\"");
    assert_ne!(damaged, text);
    let restored = saved::decode(damaged.as_bytes(), &puzzles).unwrap();
    assert_eq!(restored.recent[Level::Easy.index()], None);
    assert_eq!(restored.position, g.position);
    assert_eq!(
        puzzles[restored.start(Level::Easy, &puzzles).puzzle].number,
        1
    );
}
#[test]
fn new_game_only_warns_about_losing_progress_when_there_is_some() {
    let mut r = ready();
    tap(&mut r, chrome::MORE);
    tap(&mut r, chrome::NEW_GAME);
    assert_eq!(r.app().view, View::New);
    assert!(texts(&shown(&r)).contains(&"Choose a difficulty.".to_owned()));
    r.action(ActionId::BACK);
    let c = blank(&r.app().game, &r.app().puzzles);
    tap(&mut r, &cell_name(c));
    tap(&mut r, "digit-5");
    tap(&mut r, chrome::MORE);
    tap(&mut r, chrome::NEW_GAME);
    assert!(texts(&shown(&r)).contains(&"Replaces this game and its history.".to_owned()));
    tap(&mut r, "new-easy");
    assert_eq!(r.app().view, View::Play);
    assert_eq!(r.app().title(), "Sudoku · Easy 2");
}
fn texts(screen: &Screen) -> Vec<String> {
    screen
        .nodes
        .iter()
        .filter_map(|node| match node {
            kobo_ui::Node::Text { text, .. } | kobo_ui::Node::Secondary { text, .. } => {
                Some(text.clone())
            }
            _ => None,
        })
        .collect()
}

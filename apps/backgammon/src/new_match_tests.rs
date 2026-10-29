use super::*;
use kobo_ui::{Chrome, CLARA_BW_METRICS};

fn running_match() -> Game {
    let mut game = Game {
        score: [2, 1],
        mode: Mode::PassAndPlay,
        dice_source: Dice::scripted([(6, 3)]),
        ..Game::default()
    };
    game.roll();
    game.played.push("White 13/7 8/5".into());
    game.turn_plays.push(Move {
        from: Some(5),
        to: Some(2),
        die: 3,
    });
    game
}

/// New game from the menu, the way a finger reaches it: the dots, then the
/// first entry of the menu they open.
fn choose_new_game(game: &mut Game) -> Option<bool> {
    let _runner = kobo_sdk::AppRunner::new(Game::default());
    assert_eq!(game.apply_action(action_id(game::MORE)), Some(false));
    let screen = screen(game, None);
    let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::measuring(true));
    let rect = layout
        .rect_of_action(action_id(game::NEW_GAME))
        .expect("New game is in the open menu");
    let hit = layout
        .hit_test(rect.x + rect.width / 2, rect.y + rect.height / 2)
        .unwrap();
    game.apply_action(hit)
}

#[test]
fn new_game_during_a_match_asks_first_without_changing_progress() {
    let mut game = running_match();
    let before = game.encode();
    assert_eq!(choose_new_game(&mut game), Some(false));
    assert_eq!(game.view, View::NewMatch);
    assert_eq!(game.encode(), before);
    assert!(screen(&game, None).owns_back);
}

#[test]
fn new_game_before_anything_is_played_starts_at_once() {
    let mut game = Game::default();
    assert_eq!(choose_new_game(&mut game), Some(false));
    assert_eq!(
        game.view,
        View::Board,
        "nothing would be lost, so nothing is asked"
    );
}

#[test]
fn back_or_keep_playing_preserves_the_exact_game_and_score() {
    for cancel in [ActionId::BACK, action_id("cancel-new-match")] {
        let mut game = running_match();
        let before = game.encode();
        choose_new_game(&mut game);
        assert_eq!(game.apply_action(action_id("roll")), None);
        assert_eq!(game.encode(), before);
        assert_eq!(game.apply_action(cancel), Some(false));
        assert_eq!(game.view, View::Board);
        assert_eq!(game.encode(), before);
    }
}

#[test]
fn confirmation_clears_the_match_once_and_keeps_the_selected_rules() {
    let mut game = running_match();
    game.mode = Mode::Solo;
    game.turn = Player::White;
    choose_new_game(&mut game);
    assert_eq!(
        game.apply_action(action_id("confirm-new-match")),
        Some(true)
    );
    assert_eq!(game.view, View::Board);
    assert_eq!(game.phase, Phase::Playing);
    assert_eq!(game.score, [0; 2]);
    assert_eq!(game.mode, Mode::Solo);
    assert_eq!(game.match_to, 5);
    assert!(game.opening);
    assert!(game.dice.is_empty());
    assert!(game.history.is_empty());
    assert!(game.played.is_empty());
    assert!(game.turn_plays.is_empty());
    let after = game.encode();
    assert_eq!(game.apply_action(action_id("confirm-new-match")), None);
    assert_eq!(game.encode(), after);
    assert!(Game::decode(&after).is_some());
}

#[test]
fn confirmation_controls_fit_and_are_tappable_at_every_text_size_and_pose() {
    let mut game = running_match();
    choose_new_game(&mut game);
    for pose in [(1072, 1448), (1448, 1072)] {
        for text_scale in kobo_ui::TextScale::STEPS {
            let metrics = kobo_ui::DisplayMetrics {
                width: pose.0,
                height: pose.1,
                text_scale,
                ..CLARA_BW_METRICS
            };
            let screen = screen(&game, None);
            let diagnostics = screen.diagnostics(&metrics, &Chrome::measuring(true));
            assert!(
                diagnostics.issues.is_empty(),
                "{metrics:?}: {:?}",
                diagnostics.issues
            );
            let layout = screen.layout_with(&metrics, &Chrome::measuring(true));
            for action in [
                ActionId::BACK,
                action_id("confirm-new-match"),
                action_id("cancel-new-match"),
            ] {
                let rect = layout.rect_of_action(action).unwrap();
                assert_eq!(
                    layout.hit_test(rect.x + rect.width / 2, rect.y + rect.height / 2),
                    Some(action)
                );
            }
        }
    }
}

/// Offering the cube, answering an offer and dropping it are the same kind
/// of question as New match, so they are drawn the same way: the question as
/// the screen's heading, what follows under it, and the two answers, each a
/// whole-width button under a finger at every text size.
#[test]
fn the_cube_questions_are_asked_like_new_match_at_every_text_size() {
    let questions = [
        (
            Phase::ConfirmDouble,
            "Offer the cube at 2?",
            ["confirm-double", "cancel-double"],
        ),
        (
            Phase::Offered(Player::Black),
            "Black offers the cube at 2",
            ["take", "drop"],
        ),
        (
            Phase::ConfirmDrop(Player::Black),
            "Drop Black's double?",
            ["confirm-drop", "cancel-drop"],
        ),
    ];
    for (phase, question, answers) in questions {
        let game = Game {
            phase,
            mode: Mode::PassAndPlay,
            ..Game::default()
        };
        for text_scale in kobo_ui::TextScale::STEPS {
            let metrics = kobo_ui::DisplayMetrics {
                text_scale,
                ..CLARA_BW_METRICS
            };
            let runner = kobo_sdk::AppRunner::with_metrics(Game::default(), metrics);
            let screen = screen_for(&game, None, &runner.context());
            let diagnostics = screen.diagnostics(&metrics, &Chrome::measuring(true));
            assert!(
                diagnostics.issues.is_empty(),
                "{phase:?} {text_scale:?}: {:?}",
                diagnostics.issues
            );
            let layout = screen.layout_with(&metrics, &Chrome::measuring(true));
            let heading = layout
                .nodes
                .iter()
                .find(|node| matches!(node.kind, kobo_ui::LayoutKind::Heading(_)))
                .map(|node| node.text_lines.join(" "));
            assert_eq!(heading.as_deref(), Some(question), "{text_scale:?}");
            for answer in answers {
                let action = action_id(answer);
                let rect = layout.rect_of_action(action).expect("an answer");
                assert!(
                    rect.height >= metrics.touch_target_minimum(),
                    "{answer} {text_scale:?}"
                );
                assert_eq!(
                    layout.hit_test(rect.x + rect.width / 2, rect.y + rect.height / 2),
                    Some(action),
                    "{answer} {text_scale:?}"
                );
            }
        }
    }
}

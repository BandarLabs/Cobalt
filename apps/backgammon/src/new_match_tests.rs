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
    game.turn_moves.push("6/3".into());
    game.view = View::Match;
    game
}

#[test]
fn the_visible_new_match_action_opens_a_confirmation_without_changing_progress() {
    let _runner = kobo_sdk::AppRunner::new(Game::default());
    let mut game = running_match();
    let before = game.encode();
    let layout = screen(&game, None).layout_with(&CLARA_BW_METRICS, &Chrome::measuring(true));
    let rect = layout
        .rect_of_action(action_id("new-match"))
        .expect("visible New match");
    let hit = layout
        .hit_test(rect.x + rect.width / 2, rect.y + rect.height / 2)
        .unwrap();
    assert_eq!(game.apply_action(hit), Some(false));
    assert_eq!(game.view, View::NewMatch);
    assert_eq!(game.encode(), before);
    assert!(screen(&game, None).owns_back);
}

#[test]
fn back_or_keep_match_preserves_the_exact_game_and_score() {
    for cancel in [ActionId::BACK, action_id("cancel-new-match")] {
        let mut game = running_match();
        let before = game.encode();
        game.apply_action(action_id("new-match"));
        assert_eq!(game.apply_action(action_id("roll")), None);
        assert_eq!(game.encode(), before);
        assert_eq!(game.apply_action(cancel), Some(false));
        assert_eq!(game.view, View::Match);
        assert_eq!(game.encode(), before);
    }
}

#[test]
fn confirmation_clears_the_match_once_and_keeps_the_selected_rules() {
    let mut game = running_match();
    game.mode = Mode::Solo;
    game.turn = Player::Black;
    game.apply_action(action_id("new-match"));
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
    assert!(game.turn_moves.is_empty());
    let after = game.encode();
    assert_eq!(game.apply_action(action_id("confirm-new-match")), None);
    assert_eq!(game.encode(), after);
    assert!(Game::decode(&after).is_some());
}

#[test]
fn confirmation_controls_fit_and_are_tappable_at_every_text_size_and_pose() {
    let _runner = kobo_sdk::AppRunner::new(Game::default());
    let mut game = running_match();
    game.apply_action(action_id("new-match"));
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

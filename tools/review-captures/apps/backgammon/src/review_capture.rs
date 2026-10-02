//! Real renderer snapshots, not interactive simulator captures.
use super::*;
#[path = "../screenshots/ui-review/render.rs"]
mod render_capture;

#[test]
#[ignore]
fn capture_review_screens() {
    let _runner = kobo_sdk::AppRunner::new(Game::default());
    let mut game = Game {
        view: View::Match,
        dice_source: Dice::scripted([(6, 3)]),
        ..Game::default()
    };
    render_capture::capture("backgammon", "match", &screen(&game, None));
    game.apply_action(action_id("new-match"));
    render_capture::capture("backgammon", "new-match-tapped", &screen(&game, None));
    game.apply_action(ActionId::BACK);
    render_capture::capture("backgammon", "new-match-cancelled", &screen(&game, None));
    game.apply_action(action_id("new-match"));
    game.apply_action(action_id("confirm-new-match"));
    game.apply_action(action_id("match"));
    render_capture::capture("backgammon", "new-match-started", &screen(&game, None));
}

#[test]
#[ignore]
fn capture_help_screens() {
    let _runner = kobo_sdk::AppRunner::new(Game::default());
    assert!(kobo_ui::has_typesetter());
    let game = Game {
        view: View::Help,
        ..Game::default()
    };
    for (name, width, height) in [
        ("help-170-portrait", 1072, 1448),
        ("help-170-landscape", 1448, 1072),
    ] {
        let metrics = kobo_ui::DisplayMetrics {
            width,
            height,
            text_scale: kobo_ui::TextScale::Largest,
            ..kobo_ui::CLARA_BW_METRICS
        };
        render_capture::capture_at("backgammon", name, &screen(&game, None), metrics, &());
    }
}

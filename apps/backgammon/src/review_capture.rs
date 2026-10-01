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

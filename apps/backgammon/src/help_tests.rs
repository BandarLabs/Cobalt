use super::*;
use kobo_ui::{Chrome, DisplayMetrics, TextScale, CLARA_BW_METRICS};

#[test]
fn complete_help_and_exit_controls_fit_with_real_fonts_in_every_size_and_pose() {
    let _runner = kobo_sdk::AppRunner::new(Game::default());
    assert!(kobo_ui::has_typesetter());
    for (width, height) in [(1072, 1448), (1448, 1072)] {
        for text_scale in TextScale::STEPS {
            let metrics = DisplayMetrics {
                width,
                height,
                text_scale,
                ..CLARA_BW_METRICS
            };
            let mut game = Game {
                mode: Mode::PassAndPlay,
                dice_source: Dice::scripted([(5, 3)]),
                ..Game::default()
            };
            game.roll();
            let saved = game.encode();
            for close in [ActionId::BACK, action_id("close-help")] {
                assert_eq!(game.apply_action(action_id("how-to-play")), Some(false));
                let screen = screen(&game, None);
                let chrome = Chrome::for_screen(&screen, false, Chrome::measuring(true).status);
                let diagnostics = screen.diagnostics(&metrics, &chrome);
                assert!(
                    diagnostics.issues.is_empty(),
                    "{metrics:?}: {:?}",
                    diagnostics.issues
                );
                let layout = screen.layout_with(&metrics, &chrome);
                let drawn = layout
                    .nodes
                    .iter()
                    .flat_map(|node| node.text_lines.iter().map(String::as_str))
                    .collect::<Vec<_>>()
                    .join(" ");
                for rule in [
                    "marked legal destination",
                    "bar checkers first",
                    "home before bearing",
                    "clear all 15 wins",
                    "take or drop",
                ] {
                    assert!(drawn.contains(rule), "{metrics:?}: {drawn}");
                }
                let rect = layout.rect_of_action(close).expect("visible exit control");
                let hit = layout.hit_test(rect.x + rect.width / 2, rect.y + rect.height / 2);
                assert_eq!(hit, Some(close));
                assert_eq!(game.apply_action(action_id("roll")), None);
                assert_eq!(game.apply_action(hit.unwrap()), Some(false));
                assert_eq!(game.view, View::Board);
                assert_eq!(game.encode(), saved);
            }
        }
    }
}

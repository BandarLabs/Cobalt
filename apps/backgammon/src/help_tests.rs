use super::*;
use kobo_ui::{Chrome, DisplayMetrics, TextScale, CLARA_BW_METRICS};

#[test]
fn the_rules_and_their_way_back_fit_with_real_fonts_in_every_size_and_pose() {
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
            let runner = kobo_sdk::AppRunner::with_metrics(Game::default(), metrics);
            let context = runner.context();
            let pages = rules_pages(&context).len();
            for close in [ActionId::BACK, action_id("close-help")] {
                assert_eq!(game.apply_action(action_id(game::RULES)), Some(false));
                assert_eq!(game.view, View::Help);
                assert_eq!(game.rules_page, 0, "the rules open at their start");
                let mut every_page = String::new();
                for page in 0..pages {
                    game.rules_page = page;
                    let screen = screen_for(&game, None, &context);
                    let chrome = Chrome::for_screen(&screen, false, Chrome::measuring(true).status);
                    let diagnostics = screen.diagnostics(&metrics, &chrome);
                    assert!(
                        diagnostics.issues.is_empty(),
                        "{metrics:?} page {page}: {:?}",
                        diagnostics.issues
                    );
                    let layout = screen.layout_with(&metrics, &chrome);
                    for line in layout.nodes.iter().flat_map(|node| node.text_lines.iter()) {
                        every_page.push_str(line);
                        every_page.push(' ');
                    }
                    let rect = layout
                        .rect_of_action(close)
                        .expect("a way back to the board on every page");
                    let hit = layout.hit_test(rect.x + rect.width / 2, rect.y + rect.height / 2);
                    assert_eq!(hit, Some(close));
                }
                for rule in [
                    "tap a marked point to move it there",
                    "have to come back in first",
                    "into your home board",
                    "bear off all 15 wins",
                    "may take or drop",
                    "computer plays Black",
                ] {
                    assert!(every_page.contains(rule), "{metrics:?}: {every_page}");
                }
                assert_eq!(game.apply_action(action_id("roll")), None);
                assert_eq!(game.apply_action(close), Some(false));
                assert_eq!(game.view, View::Board);
                assert_eq!(game.encode(), saved);
            }
        }
    }
}

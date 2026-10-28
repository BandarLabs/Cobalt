use super::*;
use kobo_ui::{Chrome, DisplayMetrics, TextScale, CLARA_BW_METRICS};

#[test]
fn match_controls_and_complete_history_fit_every_text_size_and_pose() {
    for (width, height) in [(1072, 1448), (1448, 1072)] {
        for text_scale in TextScale::STEPS {
            let metrics = DisplayMetrics {
                width,
                height,
                text_scale,
                ..CLARA_BW_METRICS
            };
            let mut runner = kobo_sdk::AppRunner::with_metrics(
                Game {
                    score: [2, 1],
                    mode: Mode::PassAndPlay,
                    dice_source: Dice::scripted([(6, 3)]),
                    played: (0..40)
                        .map(|n| format!("Turn {n}: White 13/7 8/5"))
                        .collect(),
                    view: View::Match,
                    ..Game::default()
                },
                metrics,
            );
            let context = runner.context();
            let before = runner.app().encode();
            let screen = screen_for(runner.app(), None, &context);
            let chrome = Chrome::for_screen(&screen, false, Chrome::measuring(true).status);
            assert!(
                screen.diagnostics(&metrics, &chrome).issues.is_empty(),
                "{metrics:?}"
            );
            let layout = screen.layout_with(&metrics, &chrome);
            for name in ["how-to-play", "new-match", "turn-history", "close-match"] {
                let action = action_id(name);
                let rect = layout
                    .rect_of_action(action)
                    .expect("match control visible");
                assert_eq!(
                    layout.hit_test(rect.x + rect.width / 2, rect.y + rect.height / 2),
                    Some(action)
                );
            }
            runner.action(action_id("turn-history"));
            assert_eq!(runner.app().view, View::History);
            let pages = history_pages(runner.app(), &context);
            let text = pages
                .iter()
                .flatten()
                .cloned()
                .collect::<Vec<_>>()
                .join(" ");
            for n in 0..40 {
                assert!(text.contains(&format!("Turn {n}:")));
            }
            for page in 0..pages.len() {
                assert_eq!(runner.app().history_page, page);
                let screen = screen_for(runner.app(), None, &context);
                let chrome = Chrome::for_screen(&screen, false, Chrome::measuring(true).status);
                assert!(
                    screen.diagnostics(&metrics, &chrome).issues.is_empty(),
                    "{metrics:?} page {page}"
                );
                if page + 1 < pages.len() {
                    let layout = screen.layout_with(&metrics, &chrome);
                    let action = action_id("history-next");
                    let rect = layout.rect_of_action(action).expect("next page visible");
                    let hit = layout
                        .hit_test(rect.x + rect.width / 2, rect.y + rect.height / 2)
                        .unwrap();
                    assert_eq!(hit, action);
                    let commands = runner.action(hit);
                    assert!(!commands
                        .iter()
                        .any(|command| matches!(command, kobo_sdk::Command::Store(_))));
                }
            }
            runner.action(action_id("history-next"));
            assert_eq!(runner.app().history_page, pages.len() - 1);
            while runner.app().history_page > 0 {
                runner.page_turn(false);
            }
            runner.page_turn(false);
            assert_eq!(runner.app().history_page, 0);
            runner.action(action_id("roll"));
            assert_eq!(runner.app().encode(), before);
            runner.action(ActionId::BACK);
            assert_eq!(runner.app().view, View::Match);
            assert_eq!(runner.app().encode(), before);
        }
    }
}

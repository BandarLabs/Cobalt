use super::*;
use kobo_sdk::AppRunner;
use kobo_ui::{TextScale, CLARA_BW_METRICS};

fn ready_story() -> Parser {
    let mut parser = Parser {
        view: View::Play,
        stories: vec![("story-lamplight.z3".into(), 4096)],
        open_blob: Some("story-lamplight.z3".into()),
        machine: Some(
            Machine::new(
                include_bytes!("../fixtures/lamplight.z3").to_vec(),
                "story-lamplight.z3",
            )
            .expect("fixture opens"),
        ),
        ..Parser::default()
    };
    parser.advance_story(&mut Context::default());
    parser
}

#[test]
fn runtime_back_closes_keys_then_returns_to_the_library_without_restarting() {
    let _runner = AppRunner::new(Parser::default());
    let mut parser = ready_story();
    let mut context = Context::default();
    parser.keyboard = Keyboard::with_text("examine lamp");
    parser.keyboard_open = true;
    let transcript = parser.transcript.clone();
    assert!(parser.play_screen().owns_back);
    parser.on_action(&mut context, ActionId::BACK);
    assert_eq!(parser.view, View::Play);
    assert!(!parser.keyboard_open);
    assert_eq!(parser.keyboard.text(), "examine lamp");
    parser.on_action(&mut context, ActionId::BACK);
    assert_eq!(parser.view, View::Library);
    assert!(!parser.library_screen(&context).owns_back);
    parser.on_action(&mut context, action_id("story-0"));
    assert_eq!(parser.view, View::Play);
    assert_eq!(parser.transcript, transcript);
    assert_eq!(parser.keyboard.text(), "examine lamp");
    assert!(
        parser.loading.is_none(),
        "resuming must not load an older autosave"
    );
}

#[test]
fn runtime_back_cancels_a_story_restore_and_leaves_one_way_out() {
    let _runner = AppRunner::new(Parser::default());
    let mut parser = ready_story();
    let mut context = Context::default();
    parser.command(&mut context, "restore");
    assert_eq!(parser.view, View::Slots);
    assert!(parser.machine.as_ref().unwrap().awaiting_restore());
    let screen = parser.slots_screen();
    assert!(screen.owns_back);
    let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::measuring(true));
    assert!(layout.rect_of_action(action_id("play")).is_none());
    assert!(layout.rect_of_action(ActionId::BACK).is_some());
    parser.on_action(&mut context, ActionId::BACK);
    assert_eq!(parser.view, View::Play);
    assert!(!parser.machine.as_ref().unwrap().awaiting_restore());
    assert!(
        parser.message.is_none(),
        "obsolete restore instructions must be cleared"
    );
    parser.command(&mut context, "look");
    assert_eq!(parser.view, View::Play);
    assert!(parser.transcript.contains("pool of light."));
}

#[test]
fn every_library_story_is_reachable_at_each_text_size_and_pose() {
    for pose in [(1072, 1448), (1448, 1072)] {
        for scale in TextScale::STEPS {
            let metrics = DisplayMetrics {
                width: pose.0,
                height: pose.1,
                text_scale: scale,
                ..CLARA_BW_METRICS
            };
            let runner = AppRunner::with_metrics(Parser::default(), metrics);
            kobo_text::install(metrics).expect("bundled font installs");
            let context = runner.context();
            let mut parser = Parser {
                stories: (0..16)
                    .map(|index| (format!("story-adventure-{index:02}.z3"), 4096))
                    .collect(),
                ..Parser::default()
            };
            for message in [
                None,
                Some("That story could not be read from storage.".into()),
            ] {
                parser.message = message;
                let pages = parser.library_pages(&context);
                assert!(pages.len() > 1);
                assert_eq!(
                    pages.iter().flatten().copied().collect::<Vec<_>>(),
                    (0..16).collect::<Vec<_>>()
                );
                for (page, indices) in pages.iter().enumerate() {
                    parser.library_page = page;
                    let screen = parser.library_screen(&context);
                    let chrome = Chrome::measuring(true);
                    let diagnostics = screen.diagnostics(&metrics, &chrome);
                    assert!(
                        diagnostics
                            .issues
                            .iter()
                            .all(|issue| issue.severity != kobo_ui::DiagnosticSeverity::Error),
                        "{metrics:?} page {page}: {:?}",
                        diagnostics.issues
                    );
                    let layout = screen.layout_with(&metrics, &chrome);
                    for &index in indices {
                        let action = action_id(&format!("story-{index}"));
                        let rect = layout.rect_of_action(action).expect("story on page");
                        assert_eq!(
                            layout.hit_test(rect.x + rect.width / 2, rect.y + rect.height / 2),
                            Some(action)
                        );
                    }
                    assert!(layout.rect_of_action(action_id("refresh")).is_some());
                }
            }
        }
    }
}

#[test]
fn library_page_turns_clamp_and_preserve_the_active_story() {
    let _runner = AppRunner::new(Parser::default());
    let mut parser = ready_story();
    parser.view = View::Library;
    parser.stories = (0..16)
        .map(|i| (format!("story-adventure-{i:02}.z3"), 4096))
        .collect();
    let mut context = Context::default();
    let transcript = parser.transcript.clone();
    parser.on_page_turn(&mut context, false);
    assert_eq!(parser.library_page, 0);
    for _ in 0..20 {
        parser.on_action(&mut context, action_id("library-next"));
    }
    assert_eq!(
        parser.library_page,
        parser.library_pages(&context).len() - 1
    );
    assert_eq!(parser.transcript, transcript);
    parser.stories.truncate(1);
    parser.on_page_turn(&mut context, false);
    assert_eq!(parser.library_page, 0);
    let screen = parser.library_screen(&context);
    assert!(screen
        .diagnostics(&CLARA_BW_METRICS, &Chrome::measuring(true))
        .issues
        .is_empty());
    assert!(screen
        .layout_with(&CLARA_BW_METRICS, &Chrome::measuring(true))
        .rect_of_action(action_id("story-0"))
        .is_some());
}

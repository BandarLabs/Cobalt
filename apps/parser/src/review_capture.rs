//! Opt-in renderer review fixtures. Not an interactive simulator capture.
use super::*;
#[path = "../screenshots/ui-review/render.rs"]
mod render_capture;

#[test]
#[ignore]
fn capture_review_screens() {
    let _runner = kobo_sdk::AppRunner::new(Parser::default());
    let mut parser = Parser {
        stories: vec![("story-lamplight.z3".into(), 4096)],
        ..Parser::default()
    };
    render_capture::capture(
        "parser",
        "library",
        &parser.library_screen(&Context::default()),
    );
    let mut context = Context::default();
    parser.machine = Some(
        Machine::new(
            include_bytes!("../fixtures/lamplight.z3").to_vec(),
            "story-lamplight.z3",
        )
        .unwrap(),
    );
    parser.open_blob = Some("story-lamplight.z3".into());
    parser.view = View::Play;
    parser.advance_story(&mut context);
    render_capture::capture("parser", "story", &parser.play_screen());
    parser.on_action(&mut context, action_id("keyboard-toggle"));
    render_capture::capture("parser", "keyboard", &parser.play_screen());
    parser.on_action(&mut context, action_id("restore"));
    render_capture::capture("parser", "restore", &parser.slots_screen());
    parser.on_action(&mut context, action_id("play"));
    parser.command(&mut context, "restore");
    render_capture::capture("parser", "story-restore", &parser.slots_screen());
    parser.on_action(&mut context, action_id("play"));
    render_capture::capture("parser", "restore-cancelled", &parser.play_screen());
    parser.view = View::Library;
    parser.message = None;
    parser.stories = (0..16)
        .map(|i| (format!("story-adventure-{i:02}.z3"), 4096))
        .collect();
    render_capture::capture(
        "parser",
        "library-sixteen",
        &parser.library_screen(&Context::default()),
    );
    parser.on_page_turn(&mut context, true);
    parser.on_page_turn(&mut context, true);
    render_capture::capture(
        "parser",
        "library-last",
        &parser.library_screen(&Context::default()),
    );
}

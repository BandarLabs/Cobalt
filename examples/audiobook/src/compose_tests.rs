use super::*;
use kobo_sdk::{AppRunner, Command};
use kobo_sdk::{Chrome, DisplayMetrics, CLARA_BW_METRICS};

fn interface_sizes() -> Vec<DisplayMetrics> {
    let mut metrics = CLARA_BW_METRICS;
    while let Some(smaller) = metrics.text_scale.smaller() {
        metrics.text_scale = smaller;
    }
    let mut sizes = vec![metrics];
    while let Some(larger) = metrics.text_scale.larger() {
        metrics.text_scale = larger;
        sizes.push(metrics);
    }
    assert_eq!(sizes.len(), 9);
    sizes
}

fn screen(runner: &AppRunner<Audiobook>) -> Screen {
    runner.app().screen(&runner.context())
}
fn tap(runner: &mut AppRunner<Audiobook>, name: &str) {
    let drawn = screen(runner);
    let metrics = runner.context().metrics();
    let chrome = Chrome::for_screen(&drawn, false, Chrome::measuring(true).status);
    let layout = drawn.layout_with(&metrics, &chrome);
    let action = action_id(name);
    let rect = layout
        .rect_of_action(action)
        .unwrap_or_else(|| panic!("missing {name}"));
    let (x, y) = (rect.x + rect.width / 2, rect.y + rect.height / 2);
    assert!(x >= 0 && x < metrics.width && y >= 0 && y < metrics.height);
    assert_eq!(layout.hit_test(x, y), Some(action));
    runner.action(action);
}

#[test]
fn language_resume_hint_topic_and_submit_fit_all_clara_text_sizes() {
    for metrics in interface_sizes() {
        let text_scale = metrics.text_scale;
        for checkpoint in [false, true] {
            for hint in [false, true] {
                let mut runner = AppRunner::with_metrics(
                    Audiobook {
                        stage: Stage::Compose,
                        library: Some(vec![Saved {
                            name: "old.mp3z".into(),
                            title: "An old book".into(),
                            bytes: 1000,
                        }]),
                        topic: Keyboard::with_text("The Moon and its future"),
                        hint: hint.then_some("That is too short. Add a few words."),
                        checkpoint: checkpoint.then(|| Checkpoint {
                            title: "A researched history of the Moon and the people who learned to understand it".into(),
                            parts: vec!["Part".into(); 8],
                            next_part: 2,
                            ..Checkpoint::default()
                        }),
                        ..Audiobook::default()
                    },
                    metrics,
                );
                let drawn = screen(&runner);
                let chrome = Chrome::for_screen(&drawn, false, Chrome::measuring(true).status);
                let diagnostics = drawn.diagnostics(&metrics, &chrome);
                assert!(
                    diagnostics.issues.is_empty(),
                    "{text_scale:?} checkpoint={checkpoint} hint={hint}: {:?}",
                    diagnostics.issues
                );
                assert!(drawn.validate(&metrics).is_empty());
                assert!(drawn.owns_back);
                for language in pipeline::LANGUAGES {
                    tap(&mut runner, &language_action(language));
                    assert_eq!(runner.app().language, language);
                }
                assert!(screen(&runner)
                    .layout_with(&metrics, &chrome)
                    .rect_of_action(action_id("kb.enter"))
                    .is_some());
                assert_eq!(
                    screen(&runner)
                        .layout_with(&metrics, &chrome)
                        .rect_of_action(action_id(RESUME))
                        .is_some(),
                    checkpoint
                );
            }
        }
    }
}

#[test]
fn back_and_shelf_keep_an_unsubmitted_topic_and_language() {
    let mut runner = AppRunner::new(Audiobook {
        library: Some(vec![Saved {
            name: "old.mp3z".into(),
            title: "An old book".into(),
            bytes: 1000,
        }]),
        ..Audiobook::default()
    });
    runner.app_mut().pages = vec![vec![0]];
    tap(&mut runner, NEW);
    runner.app_mut().topic = Keyboard::with_text("The Moon and its future");
    tap(&mut runner, &language_action(pipeline::LANGUAGES[1]));
    let language = runner.app().language;
    runner.action(kobo_sdk::ActionId::BACK);
    assert_eq!(runner.app().stage, Stage::Library);
    tap(&mut runner, NEW);
    assert_eq!(runner.app().topic.text(), "The Moon and its future");
    assert_eq!(runner.app().language, language);
    tap(&mut runner, SHELF);
    assert_eq!(runner.app().stage, Stage::Library);
    tap(&mut runner, NEW);
    assert_eq!(runner.app().topic.text(), "The Moon and its future");
}

#[test]
fn correcting_a_short_topic_clears_the_old_hint_without_starting_work() {
    let mut runner = AppRunner::new(Audiobook {
        stage: Stage::Compose,
        library: Some(Vec::new()),
        ..Audiobook::default()
    });
    tap(&mut runner, "kb.enter");
    assert!(runner.app().hint.is_some());
    assert_eq!(runner.app().stage, Stage::Compose);
    let commands = runner.action(action_id("kb.r0c0"));
    assert_eq!(runner.app().topic.text(), "q");
    assert!(runner.app().hint.is_none());
    assert!(!commands
        .iter()
        .any(|command| matches!(command, Command::Spawn { .. })));
}

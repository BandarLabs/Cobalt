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
                        topic: "The Moon and its future".into(),
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
                for control in [TOPIC, CREATE] {
                    assert!(screen(&runner)
                        .layout_with(&metrics, &chrome)
                        .rect_of_action(action_id(control))
                        .is_some());
                }
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
    runner.app_mut().topic = "The Moon and its future".into();
    tap(&mut runner, &language_action(pipeline::LANGUAGES[1]));
    let language = runner.app().language;
    runner.action(kobo_sdk::ActionId::BACK);
    assert_eq!(runner.app().stage, Stage::Library);
    tap(&mut runner, NEW);
    assert_eq!(runner.app().topic, "The Moon and its future");
    assert_eq!(runner.app().language, language);
    tap(&mut runner, SHELF);
    assert_eq!(runner.app().stage, Stage::Library);
    tap(&mut runner, NEW);
    assert_eq!(runner.app().topic, "The Moon and its future");
}

#[test]
fn a_short_topic_is_refused_under_the_field_and_a_corrected_one_checks_the_keys_first() {
    let mut runner = AppRunner::new(Audiobook {
        stage: Stage::Compose,
        library: Some(Vec::new()),
        ..Audiobook::default()
    });
    tap(&mut runner, CREATE);
    assert!(runner.app().hint.is_some());
    assert_eq!(runner.app().stage, Stage::Compose);
    tap(&mut runner, TOPIC);
    assert!(runner.app().entry.is_open(), "the field raised no keyboard");
    let commands = runner.type_text("the moon");
    assert_eq!(runner.app().topic, "the moon");
    assert!(runner.app().hint.is_none());
    assert_eq!(runner.app().stage, Stage::Setup);
    assert!(
        !commands
            .iter()
            .any(|command| matches!(command, Command::Spawn { .. })),
        "work started before the keys were checked"
    );
}

#[test]
fn backing_out_of_the_keyboard_keeps_the_composer_and_what_was_there() {
    let mut runner = AppRunner::new(Audiobook {
        stage: Stage::Compose,
        library: Some(Vec::new()),
        topic: "the moon".into(),
        ..Audiobook::default()
    });
    tap(&mut runner, TOPIC);
    runner.cancel_text();
    assert_eq!(runner.app().stage, Stage::Compose);
    assert!(!runner.app().entry.is_open());
    assert_eq!(runner.app().topic, "the moon");
}

#[test]
fn the_keys_and_estimate_screens_fit_and_keep_their_controls_at_every_text_size() {
    for metrics in interface_sizes() {
        let text_scale = metrics.text_scale;
        for (stage, controls) in [
            (Stage::Keys, [CHECK, EDIT]),
            (Stage::Confirm, [START, EDIT]),
        ] {
            let mut runner = AppRunner::with_metrics(
                Audiobook {
                    stage,
                    missing: SECRETS.to_vec(),
                    topic:
                        "A researched history of the Moon and the people who learned to understand it"
                            .into(),
                    ..Audiobook::default()
                },
                metrics,
            );
            let drawn = screen(&runner);
            let chrome = Chrome::for_screen(&drawn, false, Chrome::measuring(true).status);
            let diagnostics = drawn.diagnostics(&metrics, &chrome);
            assert!(
                diagnostics.issues.is_empty(),
                "{stage:?} at {text_scale:?}: {:?}",
                diagnostics.issues
            );
            assert!(drawn.owns_back, "{stage:?}");
            for control in controls {
                let layout = drawn.layout_with(&metrics, &chrome);
                assert!(
                    layout.rect_of_action(action_id(control)).is_some(),
                    "{stage:?} at {text_scale:?} lost {control}"
                );
            }
            tap(&mut runner, EDIT);
            assert_eq!(runner.app().stage, Stage::Compose);
            assert!(runner.app().topic.starts_with("A researched history"));
        }
    }
}

#[test]
fn every_failure_screen_fits_with_its_controls_at_every_text_size() {
    let shelf = || {
        Some(vec![Saved {
            name: "old.mp3z".into(),
            title: "An old book".into(),
            bytes: 1000,
        }])
    };
    let long_title = "A researched history of the Moon and the people who learned to understand it";
    let cases = [
        // Something to resume, with a shelf.
        Audiobook {
            stage: Stage::Failed,
            title: long_title.into(),
            parts: vec!["Part".into(); 9],
            next_part: 4,
            library: shelf(),
            trouble: Some((
                StandardState::Error,
                "ElevenLabs did not answer, or its certificate is not one this reader trusts. Try again later.".into(),
            )),
            ..Audiobook::default()
        },
        // Nothing to resume, worth trying again, with a shelf.
        Audiobook {
            stage: Stage::Failed,
            retryable: true,
            library: shelf(),
            trouble: Some((
                StandardState::Offline,
                "This reader is not on Wi-Fi. Join Wi-Fi and try again.".into(),
            )),
            ..Audiobook::default()
        },
        // Nothing to resume, not worth retrying, no shelf.
        Audiobook {
            stage: Stage::Failed,
            trouble: Some((
                StandardState::Error,
                "OpenAI turned the request down. The account may be out of credit, or OpenAI may have retired what this app asks for. Check the account, or check Store for an update.".into(),
            )),
            ..Audiobook::default()
        },
    ];
    for (index, case) in cases.into_iter().enumerate() {
        for metrics in interface_sizes() {
            let runner = AppRunner::with_metrics(
                Audiobook {
                    library: case.library.clone(),
                    title: case.title.clone(),
                    parts: case.parts.clone(),
                    trouble: case.trouble.clone(),
                    stage: case.stage,
                    next_part: case.next_part,
                    retryable: case.retryable,
                    ..Audiobook::default()
                },
                metrics,
            );
            let drawn = screen(&runner);
            let chrome = Chrome::for_screen(&drawn, false, Chrome::measuring(true).status);
            let diagnostics = drawn.diagnostics(&metrics, &chrome);
            assert!(
                diagnostics.issues.is_empty(),
                "case {index} at {:?}: {:?}",
                metrics.text_scale,
                diagnostics.issues
            );
        }
    }
}

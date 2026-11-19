//! In-process app/renderer tests; no gateway or interactive simulator is used.
use super::*;
use kobo_sdk::{AppRunner, DiagnosticSeverity};
use kobo_ui::{Chrome, DisplayMetrics, TextScale, CLARA_BW_METRICS};

fn post() -> Post {
    Post {
        view: View::Letter,
        gateway: "https://gateway.example".into(),
        letters: vec![Letter {
            id: "long".into(),
            title: "A long letter".into(),
            body: format!(
                "{}\n\nThe final sentence is here.",
                "Tea and toast are ready. ".repeat(120)
            ),
        }],
        ..Post::default()
    }
}

fn assert_fits(screen: &Screen, metrics: DisplayMetrics) {
    let chrome = Chrome::for_screen(screen, false, Chrome::measuring(true).status);
    let errors: Vec<_> = screen
        .diagnostics(&metrics, &chrome)
        .issues
        .into_iter()
        .filter(|issue| issue.severity == DiagnosticSeverity::Error)
        .collect();
    assert!(errors.is_empty(), "{errors:#?}");
    let target = screen
        .layout_with(&metrics, &chrome)
        .rect_of_action(action_id(REPLY))
        .expect("reply action");
    assert!(target.height >= metrics.touch_target_minimum());
}

#[test]
fn long_letters_keep_every_word_and_the_reply_action_at_every_text_size() {
    for text_scale in TextScale::STEPS {
        let metrics = DisplayMetrics {
            text_scale,
            ..CLARA_BW_METRICS
        };
        let runner = AppRunner::with_metrics(Post::default(), metrics);
        let mut context = runner.context();
        for notice in [false, true] {
            let mut app = post();
            if notice {
                app.notice = Some((BannerLevel::Attention, "The Hermes gateway is offline. Your reply is still queued and will be sent when the connection returns.".into()));
                app.outbox
                    .push(Reply::new(&app.gateway, "long", "Thank you for the letter.").unwrap());
            }
            let pages = app.letter_pages(&context, &app.letters[0]);
            assert!(pages.len() > 1);
            let displayed = pages
                .iter()
                .flatten()
                .flat_map(|paragraph| paragraph.split_whitespace())
                .collect::<Vec<_>>();
            assert_eq!(
                displayed,
                app.letters[0].body.split_whitespace().collect::<Vec<_>>()
            );
            let mut offset = 0;
            for page in &pages {
                app.set_place("long", offset);
                assert_fits(&app.letter(&mut context), metrics);
                offset += page_words(page);
            }
        }
    }
}

#[test]
fn repeated_page_turns_stop_at_the_ends_and_a_saved_position_resumes() {
    let mut runner = AppRunner::new(post());
    for _ in 0..60 {
        runner.action(action_id("next-page"));
    }
    let pages = runner
        .app()
        .letter_pages(&runner.context(), &runner.app().letters[0]);
    let final_offset: usize = pages
        .iter()
        .take(pages.len() - 1)
        .map(|page| page_words(page))
        .sum();
    assert_eq!(runner.app().place("long"), final_offset);
    let bytes = protocol::encode_cache(&runner.app().letters, &runner.app().places);
    let (letters, places) = protocol::decode_cache(&bytes).unwrap();
    let mut resumed = AppRunner::new(Post {
        view: View::Letter,
        letters,
        places,
        ..Post::default()
    });
    assert_eq!(resumed.app().place("long"), final_offset);
    resumed.action(action_id(REPLY));
    assert!(resumed.app().view == View::Compose);
    resumed.action(ActionId::BACK);
    assert!(resumed.app().view == View::Inbox);
    resumed.action(action_id("letter.0"));
    assert_eq!(resumed.app().place("long"), final_offset);
    for _ in 0..60 {
        resumed.action(action_id("previous-page"));
    }
    assert_eq!(resumed.app().place("long"), 0);
}

#[test]
fn a_gateway_signed_in_from_the_computer_opens_the_inbox_on_check_again() {
    let mut runner = kobo_sdk::AppRunner::new(Post::default());
    runner.start();
    for key in [GATEWAY, CACHE, DRAFTS, OUTBOX] {
        runner.store_result(kobo_sdk::StoreResult::Loaded {
            key: key.into(),
            value: None,
        });
    }
    assert!(runner.app().view == View::Setup);
    runner.action(kobo_sdk::action_id("check-again"));
    runner.store_result(kobo_sdk::StoreResult::Loaded {
        key: GATEWAY.into(),
        value: None,
    });
    assert!(runner
        .app()
        .notice
        .as_ref()
        .unwrap()
        .1
        .contains("No gateway has arrived"));
    runner.lifecycle(kobo_sdk::Lifecycle::Foreground);
    runner.store_result(kobo_sdk::StoreResult::Loaded {
        key: GATEWAY.into(),
        value: Some(b"https://letters.example".to_vec()),
    });
    assert!(runner.app().view == View::Inbox);
}

#[test]
fn a_failed_check_names_its_cause_without_an_em_dash() {
    let mut app = Post {
        gateway: "https://letters.example".into(),
        ..Post::default()
    };
    let mut context = kobo_sdk::Context::default();
    app.fetch_outcome(
        &mut context,
        kobo_sdk::TaskOutcome::Failed(kobo_sdk::TaskError::Unauthorized),
    );
    let (level, notice) = app.notice.clone().unwrap();
    // A refused account is not mended by waiting, so it is drawn as an error.
    assert_eq!(level, kobo_sdk::BannerLevel::Error);
    assert!(notice.contains("did not accept your account"), "{notice}");
    assert!(notice.contains("kobo connect post"), "{notice}");
    assert!(!notice.contains('\u{2014}'), "{notice}");
}

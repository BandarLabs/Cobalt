use super::*;
use kobo_sdk::AppRunner;
use kobo_ui::{Chrome, DisplayMetrics, TextScale, CLARA_BW_METRICS};

#[test]
fn every_visible_category_selects_itself_at_every_text_size_and_pose() {
    for pose in [(1072, 1448), (1448, 1072)] {
        for text_scale in TextScale::STEPS {
            let metrics = DisplayMetrics {
                width: pose.0,
                height: pose.1,
                text_scale,
                ..CLARA_BW_METRICS
            };
            let runner = AppRunner::with_metrics(Quiz::default(), metrics);
            let context = runner.context();
            let mut quiz = Quiz {
                view: View::Setup,
                ..Quiz::default()
            };
            let categories = setup_categories(&quiz);
            let pages = setup_pages(&quiz, &context);
            assert_eq!(
                pages.iter().flatten().copied().collect::<Vec<_>>(),
                (0..=categories.len()).collect::<Vec<_>>()
            );
            for (page, rows) in pages.iter().enumerate() {
                quiz.setup_page = page;
                for &row in rows {
                    let screen = setup_screen(&quiz, &context);
                    let chrome = Chrome::measuring(true);
                    let diagnostics = screen.diagnostics(&metrics, &chrome);
                    assert!(
                        diagnostics.issues.is_empty(),
                        "{metrics:?} page {page}: {:?}",
                        diagnostics.issues
                    );
                    let action = if row == 0 {
                        action_id("cat-any")
                    } else {
                        action_id(&format!("cat-{}", row - 1))
                    };
                    let layout = screen.layout_with(&metrics, &chrome);
                    let rect = layout.rect_of_action(action).expect("category is visible");
                    let hit = layout
                        .hit_test(rect.x + rect.width / 2, rect.y + rect.height / 2)
                        .expect("category is tappable");
                    assert_eq!(hit, action);
                    assert!(quiz.setup_action(hit, &context));
                    let expected = row.checked_sub(1).map(|index| categories[index].as_str());
                    assert_eq!(
                        quiz.setup_category.as_deref(),
                        expected,
                        "{metrics:?} page {page} row {row}"
                    );
                    assert!(
                        quiz.setup_action(hit, &context),
                        "a repeated selection stays valid"
                    );
                    assert_eq!(quiz.setup_category.as_deref(), expected);
                    assert!(layout.rect_of_action(action_id("continue-setup")).is_some());
                }
            }
        }
    }
}

#[test]
fn long_category_names_are_measured_and_keep_their_exact_selection() {
    let mut runner = AppRunner::new(Quiz {
        view: View::Setup,
        questions: (0..3)
            .map(|index| {
                question(
                    "Entertainment: Science Fiction and Fantasy",
                    Difficulty::Easy,
                    &format!("Fixture question {index}?"),
                    ["A", "B", "C", "D"],
                    0,
                )
            })
            .collect(),
        ..Quiz::default()
    });
    let context = runner.context();
    let pages = setup_pages(runner.app(), &context);
    for page in 0..pages.len() {
        runner.app_mut().setup_page = page;
        let screen = setup_screen(runner.app(), &context);
        assert!(screen
            .diagnostics(&CLARA_BW_METRICS, &Chrome::measuring(true))
            .issues
            .is_empty());
    }
    runner.action(action_id("cat-0"));
    assert_eq!(
        runner.app().setup_category.as_deref(),
        Some("Entertainment: Science Fiction and Fantasy")
    );
    runner.app_mut().setup_party = false;
    runner.action(action_id("continue-setup"));
    assert_eq!(runner.app().view, View::Question);
    assert_eq!(runner.app().round_questions.len(), 3);
}

#[test]
fn setup_back_and_name_cancel_preserve_the_readers_choices() {
    let mut runner = AppRunner::new(Quiz::default());
    runner.start();
    runner.action(action_id("party"));
    assert!(setup_screen(runner.app(), &runner.context()).owns_back);
    runner.action(action_id("cat-2"));
    let category = runner.app().setup_category.clone();
    runner.action(action_id("continue-setup"));
    assert!(players_screen(runner.app()).owns_back);
    runner.action(action_id("rename-0"));
    assert!(screen_with(runner.app(), &runner.context()).owns_back);
    runner.action(ActionId::BACK);
    assert!(!runner.app().entry.is_open());
    assert_eq!(runner.app().view, View::Players);
    assert_eq!(runner.app().names[0], "Ada");
    runner.action(ActionId::BACK);
    assert_eq!(runner.app().view, View::Setup);
    assert_eq!(runner.app().setup_category, category);
    runner.action(ActionId::BACK);
    assert_eq!(runner.app().view, View::Home);
    assert!(!screen_with(runner.app(), &runner.context()).owns_back);
}

#[test]
fn setup_page_edges_and_empty_question_sets_remain_safe() {
    let runner = AppRunner::new(Quiz::default());
    let context = runner.context();
    let mut quiz = Quiz {
        view: View::Setup,
        ..Quiz::default()
    };
    for _ in 0..20 {
        quiz.setup_action(action_id("next-page"), &context);
    }
    assert_eq!(quiz.setup_page, setup_pages(&quiz, &context).len() - 1);
    for _ in 0..20 {
        quiz.setup_action(action_id("previous-page"), &context);
    }
    assert_eq!(quiz.setup_page, 0);
    quiz.questions.clear();
    assert_eq!(setup_pages(&quiz, &context), vec![vec![0]]);
    let screen = setup_screen(&quiz, &context);
    assert!(screen
        .diagnostics(&CLARA_BW_METRICS, &Chrome::measuring(true))
        .issues
        .is_empty());
    quiz.setup_party = false;
    quiz.setup_action(action_id("continue-setup"), &context);
    assert_eq!(quiz.view, View::Home);
    assert!(quiz.note.as_deref().unwrap().contains("No questions match"));
}

#[test]
fn physical_page_turns_preserve_selection_and_the_round_uses_that_category() {
    let mut runner = AppRunner::new(Quiz::default());
    runner.start();
    runner.action(action_id("party"));
    runner.action(action_id("cat-2"));
    assert_eq!(runner.app().setup_category.as_deref(), Some("History"));
    runner.page_turn(true);
    assert_eq!(runner.app().setup_page, 1);
    runner.page_turn(false);
    assert_eq!(runner.app().setup_page, 0);
    runner.action(action_id("continue-setup"));
    runner.action(action_id("start"));
    assert_eq!(runner.app().view, View::Question);
    assert!(!runner.app().round_questions.is_empty());
    assert!(runner
        .app()
        .round_questions
        .iter()
        .all(|question| question.category == "History"));
}

#[test]
fn all_players_and_start_fit_every_text_size_and_pose() {
    for (width, height) in [(1072, 1448), (1448, 1072)] {
        for text_scale in TextScale::STEPS {
            let metrics = DisplayMetrics {
                width,
                height,
                text_scale,
                ..CLARA_BW_METRICS
            };
            let mut runner = AppRunner::with_metrics(
                Quiz {
                    view: View::Players,
                    names: [
                        "Alexanderthe1".into(),
                        "Bartholomew2".into(),
                        "Christopher3".into(),
                        "Desdemona444".into(),
                    ],
                    ..Quiz::default()
                },
                metrics,
            );
            for count in 2..=4 {
                runner.app_mut().players = count;
                let screen = players_screen(runner.app());
                let chrome = Chrome::for_screen(&screen, false, Chrome::measuring(true).status);
                assert!(
                    screen.diagnostics(&metrics, &chrome).issues.is_empty(),
                    "{metrics:?}: {:?}",
                    screen.diagnostics(&metrics, &chrome).issues
                );
                let layout = screen.layout_with(&metrics, &chrome);
                for name in (0..count).map(|i| format!("rename-{i}")).chain([
                    "start".into(),
                    "count-2".into(),
                    "count-3".into(),
                    "count-4".into(),
                ]) {
                    let action = action_id(&name);
                    let rect = layout
                        .rect_of_action(action)
                        .expect("player control visible");
                    assert_eq!(
                        layout.hit_test(rect.x + rect.width / 2, rect.y + rect.height / 2),
                        Some(action)
                    );
                }
            }
            runner.action(action_id("start"));
            assert_eq!(runner.app().view, View::Question);
        }
    }
}

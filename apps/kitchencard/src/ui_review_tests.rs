//! In-process callback and renderer regression coverage, not a simulator session.
use super::*;
use kobo_sdk::{AppRunner, Command, DiagnosticSeverity};
use kobo_ui::{Chrome, DisplayMetrics, TextScale, CLARA_BW_METRICS};

fn kitchen() -> Kitchen {
    Kitchen {
        server: "https://mealie.example".into(),
        credential: "mealie".into(),
        recipes: (0..12)
            .map(|n| Recipe {
                slug: format!("recipe-{n}"),
                name: format!("Recipe {n:02} with lemon and chickpeas"),
                category: if n < 10 { "Dinner" } else { "Lunch" }.into(),
                servings: 2,
                description: "A quick, comforting dinner.".into(),
                ingredients: (0..18)
                    .map(|i| mealie::Ingredient {
                        quantity: Some(1.0),
                        unit: "cup".into(),
                        food: format!("Ingredient {i:02}"),
                        note: String::new(),
                        display: format!("1 cup ingredient {i:02}"),
                    })
                    .collect(),
                steps: vec![
                    "Mix the ingredients.".into(),
                    "Simmer gently for 10 minutes.".into(),
                ],
            })
            .collect(),
        tonight: Some("recipe-0".into()),
        servings: 2,
        ..Kitchen::default()
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
}

#[test]
fn every_recipe_and_ingredient_fits_at_every_text_size() {
    for text_scale in TextScale::STEPS {
        let metrics = DisplayMetrics {
            text_scale,
            ..CLARA_BW_METRICS
        };
        let runner = AppRunner::with_metrics(Kitchen::default(), metrics);
        let context = runner.context();
        let mut app = kitchen();
        app.note = Some((
            true,
            "Mealie did not answer. Saved recipes are still available.".into(),
        ));
        app.view = Some(View::Browse);
        let pages = app.browse_pages(&context);
        assert_eq!(
            pages.iter().flatten().copied().collect::<Vec<_>>(),
            (0..12).collect::<Vec<_>>()
        );
        for (page, indices) in pages.iter().enumerate() {
            app.browse_page = page;
            let screen = app.screen(&context);
            assert_fits(&screen, metrics);
            let layout = screen.layout_with(&metrics, &Chrome::measuring(true));
            for &index in indices {
                let recipe = index;
                assert!(layout
                    .rect_of_action(action_id(&format!("recipe-{recipe}")))
                    .is_some());
            }
        }
        app.view = Some(View::Ingredients);
        let pages = app.ingredients_pages(&context);
        assert_eq!(
            pages.iter().flatten().copied().collect::<Vec<_>>(),
            (0..18).collect::<Vec<_>>()
        );
        for (page, indices) in pages.iter().enumerate() {
            app.ingredients_page = page;
            let screen = app.screen(&context);
            assert_fits(&screen, metrics);
            let layout = screen.layout_with(&metrics, &Chrome::measuring(true));
            for &index in indices {
                assert!(layout
                    .rect_of_action(action_id(&format!("ingredient-{index}")))
                    .is_some());
            }
        }
    }
}

#[test]
fn ingredient_detours_keep_the_cooking_step_and_checked_items() {
    let mut app = kitchen();
    app.view = Some(View::Cook);
    app.step = 1;
    let mut runner = AppRunner::new(app);
    runner.action(action_id("ingredients"));
    for _ in 0..50 {
        runner.action(action_id("list-next"));
    }
    let last = runner.app().ingredients_pages(&runner.context()).len() - 1;
    assert_eq!(runner.app().ingredients_page, last);
    runner.action(action_id("ingredient-17"));
    assert_eq!(runner.app().checked, [17]);
    runner.action(action_id("cook"));
    assert_eq!(runner.app().step, 1);
    assert_eq!(runner.app().view, Some(View::Cook));
    runner.action(action_id("ingredients"));
    assert_eq!(runner.app().ingredients_page, last);
    let commands = runner.action(ActionId::BACK);
    assert_eq!(runner.app().view, Some(View::Cook));
    assert_eq!(runner.app().step, 1);
    assert!(commands
        .iter()
        .any(|command| matches!(command, Command::SetScreen(screen) if screen.owns_back)));
    runner.action(ActionId::BACK);
    assert_eq!(runner.app().view, Some(View::Tonight));
    runner.action(action_id("cook"));
    assert_eq!(
        runner.app().step,
        0,
        "starting a fresh cook still begins at step one"
    );
}

#[test]
fn recipe_pages_clamp_and_selection_resets_ingredients() {
    let mut runner = AppRunner::new(kitchen());
    runner.action(action_id("browse"));
    for _ in 0..50 {
        runner.action(action_id("list-next"));
    }
    assert_eq!(
        runner.app().browse_page,
        runner.app().browse_pages(&runner.context()).len() - 1
    );
    runner.app_mut().ingredients_page = 99;
    runner.action(action_id("recipe-11"));
    assert_eq!(runner.app().tonight.as_deref(), Some("recipe-11"));
    assert_eq!(runner.app().ingredients_page, 0);
    assert!(runner.app().checked.is_empty());
    runner.action(action_id("browse"));
    assert_eq!(runner.app().browse_page, 0);
    for _ in 0..50 {
        runner.action(action_id("list-previous"));
    }
    assert_eq!(runner.app().browse_page, 0);
    runner.action(ActionId::BACK);
    assert_eq!(runner.app().view, Some(View::Tonight));
}

#[test]
fn supporting_views_have_one_back_destination() {
    for text_scale in [TextScale::Default, TextScale::Largest] {
        let metrics = DisplayMetrics {
            text_scale,
            ..CLARA_BW_METRICS
        };
        for view in [View::Settings, View::Finished] {
            let runner = AppRunner::with_metrics(kitchen(), metrics);
            let mut context = runner.context();
            let mut app = kitchen();
            app.view = Some(view);
            app.show(&mut context);
            let screen = context
                .commands()
                .iter()
                .rev()
                .find_map(|command| {
                    if let Command::SetScreen(screen) = command {
                        Some(screen)
                    } else {
                        None
                    }
                })
                .unwrap();
            let diagnostics = screen.diagnostics(&metrics, &Chrome::measuring(true));
            assert!(!diagnostics
                .issues
                .iter()
                .any(|issue| issue.kind == kobo_ui::LayoutIssueKind::AmbiguousBack));
            app.on_action(&mut context, ActionId::BACK);
            assert_eq!(app.view, Some(View::Tonight));
        }
    }
}

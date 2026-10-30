//! In-process callback and renderer regression coverage, not a simulator session.
use super::*;
use kobo_sdk::{AppRunner, Command, DiagnosticSeverity};
use kobo_ui::{Chrome, DisplayMetrics, Orientation, TextScale, CLARA_BW_METRICS};

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

#[test]
fn a_refused_token_is_blamed_on_the_token_and_never_on_the_address() {
    let mut app = kitchen();
    app.fail(kobo_sdk::TaskError::Unauthorized);
    let (attention, said) = app.note.clone().unwrap();
    assert!(attention);
    assert!(said.contains("did not accept your account"), "{said}");
    assert!(said.contains("kobo connect kitchencard"), "{said}");
    assert!(!said.contains("Check the address"), "{said}");
    assert!(said.contains("Saved recipes still cook"), "{said}");
    let mut home = Kitchen {
        server: "https://192.168.1.30:9925".into(),
        ..kitchen()
    };
    home.fail(kobo_sdk::TaskError::Unreachable);
    assert!(home.note.unwrap().1.contains("home network"));
}

#[test]
fn the_cooking_step_is_set_large_and_still_fits_at_every_text_size() {
    let mut app = kitchen();
    app.view = Some(View::Cook);
    let screen = app.screen(&Context::default());
    assert!(
        screen.nodes.iter().any(|node| matches!(node, kobo_sdk::Node::Heading { text, level: 2, .. } if text == "Mix the ingredients.")),
        "{screen:?}"
    );
    let long = "Bring a large pot of salted water to the boil, add the pasta and cook it until it is just tender, stirring now and then so it does not stick, then scoop out a mug of the cooking water before you drain it and set it aside for the sauce.".to_owned();
    for text_scale in TextScale::STEPS {
        let metrics = DisplayMetrics {
            text_scale,
            ..CLARA_BW_METRICS
        };
        let runner = AppRunner::with_metrics(Kitchen::default(), metrics);
        let mut app = kitchen();
        app.view = Some(View::Cook);
        app.recipes[0].steps[0] = long.clone();
        let screen = app.screen(&runner.context());
        assert_fits(&screen, metrics);
        if text_scale == TextScale::Default {
            assert!(
                screen
                    .nodes
                    .iter()
                    .any(|node| matches!(node, kobo_sdk::Node::Heading { level: 2, .. })),
                "a step of a few sentences is still large at the default size"
            );
        }
    }
}

/// The first-run screen is the only way in, so the command, Check again and
/// typing the address have to be on the panel at every size, held either way,
/// before a check and after one that found nothing, whose banner is the
/// tallest thing on the screen.
#[test]
fn the_connect_screen_keeps_every_control_on_a_clara_at_every_size_held_either_way() {
    for text_scale in TextScale::STEPS {
        for orientation in [Orientation::Portrait, Orientation::Landscape] {
            let metrics = DisplayMetrics {
                text_scale,
                ..CLARA_BW_METRICS
            }
            .oriented(orientation);
            let runner = AppRunner::with_metrics(Kitchen::default(), metrics);
            for checked in [false, true] {
                let app = Kitchen {
                    note: checked.then(|| (true, NO_ACCOUNT_YET.to_owned())),
                    ..Kitchen::default()
                };
                let at = format!("{text_scale:?} {orientation:?} checked {checked}");
                let screen = app.connect_screen(&runner.context());
                let diagnostics = screen.diagnostics(&metrics, &Chrome::measuring(true));
                assert!(
                    diagnostics.issues.is_empty(),
                    "{at}: {:?}",
                    diagnostics.issues
                );
                let drawn = format!("{screen:?}");
                assert!(drawn.contains("kobo connect kitchen"), "{at}");
                assert!(drawn.contains("Mealie address and API token"), "{at}");
                for action in ["check-again", "settings"] {
                    assert!(
                        diagnostics
                            .layout
                            .rect_of_action(action_id(action))
                            .is_some(),
                        "{at}: {action}"
                    );
                }
                let offered = diagnostics
                    .layout
                    .nodes
                    .iter()
                    .filter(|node| node.kind.acts_on() == Some(action_id("check-again")))
                    .count();
                assert_eq!(offered, 1, "{at}");
                let on_banner = screen.nodes.iter().any(|node| {
                    matches!(
                        node,
                        kobo_ui::Node::Banner { action: Some(action), .. }
                            if action.action == action_id("check-again")
                    )
                });
                assert_eq!(on_banner, checked, "{at}");
            }
        }
    }
}

#[test]
fn a_connection_made_on_the_computer_is_found_by_check_again() {
    let mut runner = AppRunner::new(Kitchen::default());
    runner.start();
    for key in [CONFIG, TONIGHT] {
        runner.store_result(StoreResult::Loaded {
            key: key.into(),
            value: None,
        });
    }
    let commands = runner.action(action_id("check-again"));
    assert!(commands.iter().any(
        |c| matches!(c, Command::Store(kobo_sdk::StoreRequest::Load { key }) if key == CONFIG)
    ));
    runner.store_result(StoreResult::Loaded {
        key: CONFIG.into(),
        value: None,
    });
    assert!(runner
        .app()
        .note
        .as_ref()
        .unwrap()
        .1
        .contains("No account has arrived"));
    runner.lifecycle(kobo_sdk::Lifecycle::Foreground);
    runner.store_result(StoreResult::Loaded {
        key: CONFIG.into(),
        value: Some(b"https://mealie.example\nmealie".to_vec()),
    });
    assert!(runner.app().ready());
}

/// Tonight's recipe is the screen the app opens on, so its controls and
/// everything it says about the recipe must be on the panel at every size,
/// held either way, under the note a sync leaves. A description of a few
/// sentences is used because Mealie's own are about that long.
#[test]
fn tonights_recipe_keeps_every_control_and_fact_at_every_size_held_either_way() {
    const DESCRIPTION: &str = "A weeknight roast chicken with lemon, garlic, and thyme. Roast it on a bed of onions so the juices make a sauce while it rests.";
    for text_scale in TextScale::STEPS {
        for orientation in [Orientation::Portrait, Orientation::Landscape] {
            let metrics = DisplayMetrics {
                text_scale,
                ..CLARA_BW_METRICS
            }
            .oriented(orientation);
            let mut runner = AppRunner::with_metrics(kitchen(), metrics);
            let app = runner.app_mut();
            app.recipes[0].name = "Lemon Herb Roasted Chicken".into();
            app.recipes[0].description = DESCRIPTION.into();
            app.note = Some((false, "Tonight's card is saved for offline cooking.".into()));
            app.view = Some(View::Tonight);
            let at = format!("{text_scale:?} {orientation:?}");
            let mut reached = std::collections::BTreeSet::new();
            let mut said = String::new();
            for page in 0..2 {
                runner.app_mut().tonight_page = page;
                let screen = runner.app().screen(&runner.context());
                let diagnostics = screen.diagnostics(&metrics, &Chrome::measuring(true));
                assert!(
                    diagnostics.issues.is_empty(),
                    "{at} page {page}: {:?}",
                    diagnostics.issues
                );
                for action in ["cook", "browse", "less", "more", "sync", "settings"] {
                    if diagnostics
                        .layout
                        .rect_of_action(action_id(action))
                        .is_some()
                    {
                        reached.insert(action);
                    }
                }
                said.push_str(&format!("{screen:?}"));
            }
            assert_eq!(reached.len(), 6, "{at}: only {reached:?}");
            for fact in ["Serves 2", "Steps", "Ingredients", DESCRIPTION] {
                assert!(said.contains(fact), "{at}: {fact}");
            }
        }
    }
}

#[test]
fn the_servings_step_one_at_a_time_between_one_and_twelve() {
    let mut runner = AppRunner::new(Kitchen {
        view: Some(View::Tonight),
        ..kitchen()
    });
    runner.action(action_id("less"));
    assert_eq!(runner.app().servings, 1);
    runner.action(action_id("less"));
    assert_eq!(runner.app().servings, 1);
    let screen = runner.app().screen(&runner.context());
    assert!(screen.nodes.iter().any(|node| matches!(
        node,
        kobo_sdk::Node::Stepper { label, less_state: kobo_sdk::ControlState::Disabled, more_state: kobo_sdk::ControlState::Enabled, .. }
            if label == "Serves 1"
    )), "{screen:?}");
    for _ in 0..20 {
        runner.action(action_id("more"));
    }
    assert_eq!(runner.app().servings, MAX_SERVINGS);
}

/// Ticking an ingredient off is state the runtime draws as a box, not a
/// check mark the app picks for the row's lead, so a ticked line reads as
/// ticked and is the same size as an open one. There is no selection bar,
/// because nothing is done to the ticked lines all at once. The list still
/// pages whole at every size, held either way, and ticking a line moves
/// nothing on the page.
#[test]
fn ticked_ingredients_are_checked_rows_that_move_nothing_at_every_size_held_either_way() {
    for text_scale in TextScale::STEPS {
        for orientation in [Orientation::Portrait, Orientation::Landscape] {
            let metrics = DisplayMetrics {
                text_scale,
                ..CLARA_BW_METRICS
            }
            .oriented(orientation);
            let at = format!("{text_scale:?} {orientation:?}");
            let mut runner = AppRunner::with_metrics(
                Kitchen {
                    view: Some(View::Ingredients),
                    ..kitchen()
                },
                metrics,
            );
            let pages = runner.app().ingredients_pages(&runner.context());
            assert_eq!(
                pages.iter().flatten().copied().collect::<Vec<_>>(),
                (0..18).collect::<Vec<_>>(),
                "{at}"
            );
            let before = runner.app().screen(&runner.context());
            assert_fits(&before, metrics);
            let first = pages[0][0];
            runner.action(action_id(&format!("ingredient-{first}")));
            assert_eq!(runner.app().checked, [first], "{at}");
            let after = runner.app().screen(&runner.context());
            assert_fits(&after, metrics);
            let states = after
                .nodes
                .iter()
                .find_map(|node| match node {
                    kobo_sdk::Node::Rows { rows, .. } => {
                        Some(rows.iter().map(|row| row.state).collect::<Vec<_>>())
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("{at}: no ingredient list"));
            assert_eq!(states[0], RowState::Checked, "{at}");
            assert!(
                states[1..]
                    .iter()
                    .all(|state| *state == RowState::Unchecked),
                "{at}: {states:?}"
            );
            assert!(
                !after
                    .nodes
                    .iter()
                    .any(|node| matches!(node, kobo_sdk::Node::SelectionBar { .. })),
                "{at}: a selection bar"
            );
            let chrome = Chrome::measuring(true);
            let (was, now) = (
                before.layout_with(&metrics, &chrome),
                after.layout_with(&metrics, &chrome),
            );
            for &index in &pages[0] {
                let action = action_id(&format!("ingredient-{index}"));
                assert_eq!(
                    was.rect_of_action(action),
                    now.rect_of_action(action),
                    "{at}: ingredient {index} moved"
                );
            }
        }
    }
}

use super::*;
use kobo_ui::{Chrome, CLARA_BW_METRICS};

#[test]
fn back_from_the_record_returns_to_the_unchanged_game() {
    let _runner = kobo_sdk::AppRunner::new(Parlor::default());
    for title in GAMES {
        let mut app = Parlor::default();
        app.start(title);
        let before = encode(&app, app.position.as_ref().unwrap());
        app.handle_action(action_id("record"));
        assert_eq!(app.view, View::Record);
        let screen = app.screen();
        assert!(screen.owns_back);
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::measuring(true));
        assert!(
            layout.rect_of_action(action_id("board")).is_none(),
            "use the standard way back"
        );
        let rect = layout
            .rect_of_action(ActionId::BACK)
            .expect("runtime Back is visible");
        let action = layout
            .hit_test(rect.x + rect.width / 2, rect.y + rect.height / 2)
            .unwrap();
        app.handle_action(action);
        assert_eq!(app.view, View::Board);
        assert_eq!(encode(&app, app.position.as_ref().unwrap()), before);
        app.handle_action(action_id("record"));
        app.handle_action(ActionId::BACK);
        assert_eq!(app.view, View::Board);
        app.handle_action(ActionId::BACK);
        assert_eq!(app.view, View::Menu, "a second Back still leaves the board");
        app.handle_action(action_id("resume"));
        assert_eq!(encode(&app, app.position.as_ref().unwrap()), before);
    }
}

#[test]
fn the_record_and_its_runtime_back_fit_all_interface_sizes() {
    let _runner = kobo_sdk::AppRunner::new(Parlor::default());
    let mut app = Parlor::default();
    app.start(Title::Reversi);
    app.record.push("Black d3".into());
    app.view = View::Record;
    for text_scale in kobo_ui::TextScale::STEPS {
        let metrics = kobo_ui::DisplayMetrics {
            text_scale,
            ..CLARA_BW_METRICS
        };
        let screen = app.screen();
        assert!(screen
            .diagnostics(&metrics, &Chrome::measuring(true))
            .issues
            .is_empty());
        assert!(screen
            .layout_with(&metrics, &Chrome::measuring(true))
            .rect_of_action(ActionId::BACK)
            .is_some());
    }
}

#[test]
fn long_records_keep_every_move_reachable_at_every_size_and_pose() {
    for pose in [(1072, 1448), (1448, 1072)] {
        for text_scale in kobo_ui::TextScale::STEPS {
            let metrics = kobo_ui::DisplayMetrics {
                width: pose.0,
                height: pose.1,
                text_scale,
                ..CLARA_BW_METRICS
            };
            let runner = kobo_sdk::AppRunner::with_metrics(Parlor::default(), metrics);
            let context = runner.context();
            let mut app = Parlor::default();
            app.start(Title::Reversi);
            app.record = (0..40)
                .map(|index| {
                    format!(
                        "{} d3 flips 1",
                        if index % 2 == 0 { "Black" } else { "White" }
                    )
                })
                .collect();
            app.view = View::Record;
            let pages = app.record_pages(&context);
            assert!(!pages.is_empty());
            let text = pages
                .iter()
                .flatten()
                .cloned()
                .collect::<Vec<_>>()
                .join("\n");
            for number in 1..=40 {
                assert_eq!(
                    text.lines()
                        .filter(|line| line.starts_with(&format!("{number}. ")))
                        .count(),
                    1,
                    "move {number} at {metrics:?}"
                );
            }
            for page in 0..pages.len() {
                app.record_page = page;
                let screen = app.screen_for(&context);
                let diagnostics = screen.diagnostics(&metrics, &Chrome::measuring(true));
                assert!(
                    diagnostics.issues.is_empty(),
                    "{metrics:?} page {page}: {:?}",
                    diagnostics.issues
                );
                let layout = screen.layout_with(&metrics, &Chrome::measuring(true));
                for (name, visible) in [
                    ("record-previous", page > 0),
                    ("record-next", page + 1 < pages.len()),
                ] {
                    assert_eq!(layout.rect_of_action(action_id(name)).is_some(), visible);
                }
            }
        }
    }
}

#[test]
fn record_page_turns_clamp_and_do_not_resave_or_change_the_game() {
    let mut runner = kobo_sdk::AppRunner::new(Parlor::default());
    runner.app_mut().start(Title::Kalah);
    runner.app_mut().record = (0..40).map(|index| format!("Move {index}")).collect();
    runner.app_mut().view = View::Record;
    let before = encode(runner.app(), runner.app().position.as_ref().unwrap());
    for _ in 0..20 {
        let commands = runner.action(action_id("record-next"));
        assert!(!commands
            .iter()
            .any(|command| matches!(command, kobo_sdk::Command::Store(_))));
    }
    assert_eq!(
        runner.app().record_page,
        runner.app().record_pages(&runner.context()).len() - 1
    );
    for _ in 0..20 {
        runner.page_turn(false);
    }
    assert_eq!(runner.app().record_page, 0);
    assert_eq!(
        encode(runner.app(), runner.app().position.as_ref().unwrap()),
        before
    );
    runner.action(ActionId::BACK);
    assert_eq!(runner.app().view, View::Board);
}

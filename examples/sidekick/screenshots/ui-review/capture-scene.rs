#[test]
fn capture() {
    for scale in [kobo_ui::TextScale::Default, kobo_ui::TextScale::Largest] {
        let metrics = DisplayMetrics {
            text_scale: scale,
            ..CLARA_BW_METRICS
        };
        let context = kobo_sdk::AppRunner::with_metrics(Sidekick::default(), metrics).context();
        let prefix = scale.percent();
        let mut app = Sidekick {
            view: View::Asking,
            address: "192.168.1.20:9331".into(),
            ..Sidekick::default()
        };
        app.ask=Some(Ask{id:1,source:"codex".into(),session:"cobalt ab12".into(),tool:"shell".into(),detail:"Review the accessibility of all application screens and preserve every existing behavior. Include error recovery, keyboard input, navigation, content pagination, and regression tests. ".repeat(4),choices:Vec::new(),permission:true,multi:false});
        save_capture(
            &format!("{prefix}-long-permission"),
            &APP_SCREEN.with_own_back(true),
            metrics,
            false,
        );
        app.ask.as_mut().unwrap().detail = "Which changes should I include in this release?".into();
        app.ask.as_mut().unwrap().permission = false;
        app.ask.as_mut().unwrap().multi = true;
        app.ask.as_mut().unwrap().choices =
            (0..5)
                .map(|i| Choice {
                    label: format!("Section {}", i + 1),
                    description:
                        "Keep the original behavior and make each important action easy to find."
                            .into(),
                })
                .collect();
        app.ticked = vec![false; 5];
        save_capture(
            &format!("{prefix}-multiple-choice"),
            &APP_SCREEN.with_own_back(true),
            metrics,
            false,
        );
        app.view = View::Board;
        app.board = (0..10)
            .map(|i| Ask {
                id: i,
                source: "codex".into(),
                session: format!("cobalt terminal {i}"),
                tool: "shell".into(),
                detail: "cargo test --workspace --locked".into(),
                choices: Vec::new(),
                permission: true,
                multi: false,
            })
            .collect();
        save_capture(
            &format!("{prefix}-waiting-board"),
            &APP_SCREEN,
            metrics,
            false,
        );
    }
}

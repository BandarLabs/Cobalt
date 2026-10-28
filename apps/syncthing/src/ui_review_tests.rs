//! In-process tests of the runtime Back-ownership contract and existing handler.
use super::*;
use kobo_sdk::{AppRunner, Command};
use kobo_ui::{Chrome, CLARA_BW_METRICS};

fn screen(commands: &[Command]) -> &Screen {
    commands
        .iter()
        .find_map(|command| match command {
            Command::SetScreen(screen) => Some(screen),
            _ => None,
        })
        .expect("screen update")
}

fn active_sync() -> Sync {
    Sync {
        config: Config {
            enabled: true,
            cadence: Cadence::Hourly,
        },
        status: WindowStatus {
            state: "running".into(),
            bytes: 524_288,
            message: "Receiving notes".into(),
            peers: Some(1),
            last_success: 1_790_841_600,
            conflicts: Some(0),
        },
        imports: vec![Import {
            folder: "vault".into(),
            files: 4,
            epoch: 1_790_841_600,
        }],
        scheduled_at: 1_790_845_200,
        first_sync_seen: true,
        ..Sync::default()
    }
}

#[test]
fn runtime_back_returns_from_every_supporting_page_without_changing_sync() {
    let mut runner = AppRunner::new(active_sync());
    let config = runner.app().config.clone();
    let status = format!("{:?}", runner.app().status);
    let imports = format!("{:?}", runner.app().imports);
    let scheduled = runner.app().scheduled_at;
    for _ in 0..5 {
        for action in ["setup", "folders", "about"] {
            let opened = runner.action(action_id(action));
            assert!(
                screen(&opened).owns_back,
                "{action} must receive runtime Back"
            );
            assert!(
                screen(&opened)
                    .diagnostics(&CLARA_BW_METRICS, &Chrome::measuring(true))
                    .issues
                    .is_empty(),
                "{action} must have one unambiguous way back"
            );
            let returned = runner.action(ActionId::BACK);
            assert_eq!(runner.app().view, View::Status);
            assert!(
                !screen(&returned).owns_back,
                "root Back is free to leave Sync"
            );
            assert!(
                returned
                    .iter()
                    .all(|command| matches!(command, Command::SetScreen(_))),
                "Back must not write settings, reschedule, or spawn work"
            );
            assert_eq!(runner.app().config, config);
            assert_eq!(format!("{:?}", runner.app().status), status);
            assert_eq!(format!("{:?}", runner.app().imports), imports);
            assert_eq!(runner.app().scheduled_at, scheduled);
        }
    }
}

#[test]
fn backing_out_of_setup_does_not_enable_paused_sync() {
    let mut runner = AppRunner::new(Sync::default());
    let opened = runner.action(action_id("setup"));
    assert!(screen(&opened).owns_back);
    runner.action(ActionId::BACK);
    assert_eq!(runner.app().view, View::Status);
    assert!(!runner.app().config.enabled);
    assert_eq!(runner.app().config.cadence, Cadence::Manual);
    assert_eq!(runner.app().scheduled_at, 0);
}

#[test]
fn setup_instructions_fit_at_every_portrait_text_size() {
    for text_scale in kobo_ui::TextScale::STEPS {
        let metrics = kobo_ui::DisplayMetrics {
            text_scale,
            ..CLARA_BW_METRICS
        };
        let screen = guide_screen().with_own_back(true);
        let chrome = Chrome::for_screen(&screen, false, Chrome::measuring(true).status);
        let diagnostics = screen.diagnostics(&metrics, &chrome);
        assert!(
            !diagnostics.has_errors(),
            "{text_scale:?}: {:?}",
            diagnostics.issues
        );
    }
}

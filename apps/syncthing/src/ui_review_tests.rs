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
        last_attempt: 1_790_841_600,
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
    let attempted = runner.app().last_attempt;
    for _ in 0..5 {
        for action in ["setup", "folders", "about", "cadence"] {
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
            assert_eq!(runner.app().last_attempt, attempted);
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
    assert_eq!(runner.app().asked, Asked::No);
}

#[test]
fn setup_instructions_fit_at_every_portrait_text_size() {
    for text_scale in kobo_ui::TextScale::STEPS {
        let metrics = kobo_ui::DisplayMetrics {
            text_scale,
            ..CLARA_BW_METRICS
        };
        // A runner installs the face the runtime measures with. Without one,
        // this test measured in the built-in fallback type whenever it ran
        // before any other test had made one.
        let _runner = AppRunner::with_metrics(Sync::default(), metrics);
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

#[test]
fn the_busiest_status_screen_fits_at_every_portrait_text_size() {
    let mut busiest = active_sync();
    busiest.status = WindowStatus::parse(
        "conflict\n0\n2 file(s) could not sync. Sync tries them again next time.\n1\n1790841600\n2",
    );
    busiest.imports.push(Import {
        folder: "frame".into(),
        files: 12,
        epoch: 1_790_841_600,
    });
    // Every schedule, because each one words the schedule row differently and
    // a row that wraps takes the room the status facts need.
    for cadence in [
        Cadence::Manual,
        Cadence::Hourly,
        Cadence::FourHourly,
        Cadence::Daily,
    ] {
        busiest.config.cadence = cadence;
        for text_scale in kobo_ui::TextScale::STEPS {
            let metrics = kobo_ui::DisplayMetrics {
                text_scale,
                ..CLARA_BW_METRICS
            };
            // A runner installs the face the runtime measures with. Without
            // one, this test measured in the built-in fallback type whenever
            // it ran before any other test had made one.
            let _runner = AppRunner::with_metrics(Sync::default(), metrics);
            let screen = busiest.status_screen();
            let chrome = Chrome::for_screen(&screen, true, Chrome::measuring(true).status);
            let diagnostics = screen.diagnostics(&metrics, &chrome);
            assert!(
                !diagnostics.has_errors(),
                "{cadence:?} {text_scale:?}: {:?}",
                diagnostics.issues
            );
        }
    }
}

#[test]
fn the_schedule_is_chosen_from_a_list_that_marks_the_current_one_and_closes_on_a_choice() {
    let mut runner = AppRunner::new(active_sync());
    let opened = runner.action(action_id("cadence"));
    let Some(kobo_ui::Node::Options { selected, .. }) = screen(&opened)
        .nodes
        .iter()
        .find(|node| matches!(node, kobo_ui::Node::Options { .. }))
    else {
        panic!("the schedule is not offered as a list")
    };
    assert_eq!(*selected, Some(1), "every hour is not marked as the answer");
    runner.action(action_id(&schedule_name(3)));
    assert_eq!(runner.app().view, View::Status);
    assert_eq!(runner.app().config.cadence, Cadence::Daily);
}

#[test]
fn the_schedule_list_and_a_stalled_sync_fit_at_every_portrait_text_size() {
    let mut stalled = active_sync();
    stalled.status = WindowStatus::default();
    stalled.asked = Asked::Stalled;
    for text_scale in kobo_ui::TextScale::STEPS {
        let metrics = kobo_ui::DisplayMetrics {
            text_scale,
            ..CLARA_BW_METRICS
        };
        let _runner = AppRunner::with_metrics(Sync::default(), metrics);
        for (name, screen) in [
            ("schedule", stalled.schedule_screen().with_own_back(true)),
            ("stalled", stalled.status_screen()),
        ] {
            let chrome = Chrome::for_screen(&screen, true, Chrome::measuring(true).status);
            let diagnostics = screen.diagnostics(&metrics, &chrome);
            assert!(
                !diagnostics.has_errors(),
                "{name} {text_scale:?}: {:?}",
                diagnostics.issues
            );
        }
    }
}

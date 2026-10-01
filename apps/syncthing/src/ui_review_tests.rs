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
fn capture_review_pages_when_requested() {
    let Ok(output) = std::env::var("COBALT_REVIEW_OUT") else {
        return;
    };
    let output = std::path::PathBuf::from(output);
    std::fs::create_dir_all(&output).unwrap();
    let mut runner = AppRunner::new(Sync::default());
    let opened = runner.action(action_id("setup"));
    let guide = screen(&opened).clone();
    let returned = runner.action(ActionId::BACK);
    let status = screen(&returned).clone();
    let opened = runner.action(action_id("folders"));
    let folders = screen(&opened).clone();
    let opened = runner.action(action_id("about"));
    let about = screen(&opened).clone();
    std::fs::write(
        output.join("back-ownership.json"),
        format!(
            "{{\"guide\":{},\"status_after_back\":{},\"sync_enabled\":{}}}\n",
            guide.owns_back,
            status.owns_back,
            runner.app().config.enabled
        ),
    )
    .unwrap();
    for (name, screen) in [
        ("guide", guide),
        ("folders", folders),
        ("about", about),
        ("status-after-back", status),
    ] {
        let chrome = Chrome::for_screen(&screen, false, Chrome::measuring(true).status);
        let screen = kobo_ui::ensure_way_back(screen, &chrome, "Sync");
        let issues = screen.diagnostics(&CLARA_BW_METRICS, &chrome).issues;
        assert!(issues.is_empty(), "{name}: {issues:#?}");
        let mut surface = kobo_ui::Surface::new(1072, 1448);
        kobo_ui::render_all(
            &screen,
            &CLARA_BW_METRICS,
            &chrome,
            &kobo_ui::PictureCache::default(),
            &mut surface,
            None,
        );
        let png = kobo_image::encode_png_grey(1072, 1448, &surface.pixels).unwrap();
        std::fs::write(output.join(format!("{name}.png")), png).unwrap();
        std::fs::write(
            output.join(format!("{name}.diagnostics.txt")),
            format!("{:#?}", screen.diagnostics(&CLARA_BW_METRICS, &chrome)),
        )
        .unwrap();
    }
}

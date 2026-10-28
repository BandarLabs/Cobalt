use super::coldest;
use std::time::{Duration, Instant};

/// Reads better than a bare bool at every call site below.
const BUSY: bool = true;
const IDLE: bool = false;

fn ago(seconds: u64) -> Instant {
    Instant::now()
        .checked_sub(Duration::from_secs(seconds))
        .unwrap_or_else(Instant::now)
}

#[test]
fn the_application_left_alone_longest_is_the_one_that_goes() {
    let seen = [(1, ago(30), IDLE), (2, ago(300), IDLE), (3, ago(5), IDLE)];
    assert_eq!(coldest(&seen, 3), Some(1));
}

#[test]
fn the_one_on_the_panel_is_never_stopped_even_when_it_is_the_oldest() {
    // The front application is the oldest here by a wide margin, because
    // `used` records when it was last brought forward rather than when it
    // was last touched. Stopping it would close what the reader is looking
    // at in order to open something else.
    let seen = [(1, ago(900), IDLE), (2, ago(10), IDLE)];
    assert_eq!(coldest(&seen, 1), Some(1));
}

#[test]
fn nothing_is_stopped_when_the_only_application_is_the_front_one() {
    let seen = [(7, ago(60), IDLE)];
    assert_eq!(coldest(&seen, 7), None);
}

#[test]
fn an_application_still_working_is_passed_over_for_a_newer_idle_one() {
    // The shape this exists for: an audiobook was started, took minutes to
    // write, and the owner read the news while it did. It is the oldest by
    // a long way and the only one with anything to lose, so the idle
    // application touched moments ago goes instead.
    let seen = [(1, ago(600), BUSY), (2, ago(20), IDLE), (3, ago(2), IDLE)];
    assert_eq!(coldest(&seen, 3), Some(1));
}

#[test]
fn work_in_flight_outweighs_any_amount_of_idleness() {
    let seen = [(1, ago(86_400), BUSY), (2, ago(1), IDLE)];
    assert_eq!(coldest(&seen, 9), Some(1));
}

#[test]
fn a_working_application_is_stopped_when_every_candidate_is_working() {
    // Refusing to open anything is worse than stopping something, so when
    // there is no idle candidate the oldest busy one still goes.
    let seen = [(1, ago(30), BUSY), (2, ago(300), BUSY), (3, ago(5), IDLE)];
    assert_eq!(coldest(&seen, 3), Some(1));
}

#[test]
fn platform_and_app_updates_have_separate_builtin_authorities() {
    use kobo_protocol::DeviceRequest;

    let platform = DeviceRequest::Update {
        url: "https://example.test/Cobalt.tgz".to_owned(),
        sha256: "a".repeat(64),
    };
    assert!(super::system_request_allowed("settings", &platform));
    assert!(!super::system_request_allowed("store", &platform));
    assert!(!super::system_request_allowed("todo", &platform));

    for request in [
        DeviceRequest::ReadAutoUpdate,
        DeviceRequest::SetAutoUpdate {
            cobalt: true,
            apps: false,
        },
        DeviceRequest::ReadUpdateChannel,
        DeviceRequest::SetUpdateChannel {
            channel: kobo_protocol::UpdateChannel::Beta,
        },
    ] {
        assert!(super::system_request_allowed("settings", &request));
        assert!(!super::system_request_allowed("store", &request));
        assert!(!super::system_request_allowed("todo", &request));
    }

    let install = DeviceRequest::InstallApp {
        id: "word-count".to_owned(),
    };
    assert!(super::system_request_allowed("store", &install));
    assert!(!super::system_request_allowed("settings", &install));
    assert!(!super::system_request_allowed("todo", &install));

    assert!(super::system_request_allowed(
        "launcher",
        &DeviceRequest::ListInstalledApps
    ));
    assert!(!super::system_request_allowed(
        "launcher",
        &DeviceRequest::RefreshAppCatalog
    ));
    for request in [
        DeviceRequest::ReadAppLink,
        DeviceRequest::BeginAppLink,
        DeviceRequest::PollAppLink,
        DeviceRequest::DisconnectAppLink,
    ] {
        assert!(super::system_request_allowed("store", &request));
        assert!(!super::system_request_allowed("settings", &request));
        assert!(!super::system_request_allowed("todo", &request));
    }
}

#[test]
fn keyboard_release_gets_one_short_app_response_window() {
    assert_eq!(
        super::release_grace(super::FeedbackKind::Control),
        super::CONTROL_RELEASE_GRACE
    );
    assert_eq!(
        super::release_grace(super::FeedbackKind::KeyboardKey),
        super::KEYBOARD_RELEASE_GRACE
    );
    assert!(super::KEYBOARD_RELEASE_GRACE > super::CONTROL_RELEASE_GRACE);
    assert!(super::KEYBOARD_RELEASE_GRACE < std::time::Duration::from_millis(50));
}

#[test]
fn keyboard_cells_use_the_keyboard_feedback_window() {
    let screen = kobo_ui::Screen::new(
        1,
        vec![kobo_ui::Node::Grid {
            id: kobo_ui::NodeId(1),
            columns: 2,
            square: false,
            cells: vec![
                kobo_ui::Cell::new(kobo_ui::ActionId(1), "A"),
                kobo_ui::Cell::new(kobo_ui::ActionId(2), "B"),
            ],
        }],
    );
    let layout = screen.layout();
    let key = layout
        .nodes
        .iter()
        .find(|node| {
            matches!(
                node.kind,
                kobo_ui::LayoutKind::Cell(_, kobo_ui::CellStyle::Key, _)
            )
        })
        .expect("keyboard key")
        .rect;
    assert_eq!(
        super::feedback_kind(&layout, key),
        super::FeedbackKind::KeyboardKey
    );
}
#[test]
fn app_secret_installation_is_scoped_private_and_replaceable() {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    let directory = std::env::temp_dir().join(format!("kobo-app-secret-{}", std::process::id()));
    let _ignored = std::fs::remove_dir_all(&directory);
    kobo_policy::credentials::install_app_secret(&directory, "zotero-reader", "zotero", "first")
        .expect("install credential");
    assert_eq!(
        std::fs::read(directory.join("apps/zotero-reader/zotero")).expect("read credential"),
        b"first"
    );
    // Windows has no mode bits to assert; the profile ACL is the boundary.
    #[cfg(unix)]
    {
        assert_eq!(
            std::fs::metadata(&directory)
                .expect("secret directory")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(directory.join("apps/zotero-reader/zotero"))
                .expect("secret file")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    kobo_policy::credentials::install_app_secret(&directory, "zotero-reader", "zotero", "second")
        .expect("replace credential");
    assert_eq!(
        std::fs::read(directory.join("apps/zotero-reader/zotero")).expect("read replacement"),
        b"second"
    );
    assert!(kobo_policy::credentials::install_app_secret(
        &directory,
        "zotero-reader",
        "openai",
        "not-authorized"
    )
    .is_err());
    assert!(!directory.join("apps/zotero-reader/openai").exists());
    let _ignored = std::fs::remove_dir_all(directory);
}

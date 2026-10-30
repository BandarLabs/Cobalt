//! The device host's half of a runtime text field: taps resolved against the
//! field it drew, and exactly one answer on the application's socket.

use super::{open_text_field, type_into, visible, Hosted, Tap, TextField};
use kobo_hal::touch::TouchEvent;
use kobo_policy::TaskRunner;
use kobo_protocol::{DenyReason, DeviceRequest, DeviceResult, Message, SecretTarget, TextOutcome};
use kobo_ui::{ActionId, Chrome, FieldKind, LayoutKind};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

static FIXTURE: AtomicUsize = AtomicUsize::new(0);

fn hosted(name: &str) -> (Hosted, std::os::unix::net::UnixStream, PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "cobalt-text-field-{}-{}",
        std::process::id(),
        FIXTURE.fetch_add(1, Ordering::SeqCst)
    ));
    let _ignored = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("scratch device");
    let (runtime, client) = std::os::unix::net::UnixStream::pair().expect("socket pair");
    client
        .set_read_timeout(Some(Duration::from_secs(1)))
        .expect("read timeout");
    let child = std::process::Command::new("/usr/bin/true")
        .spawn()
        .expect("a stand-in process");
    let hosted = Hosted {
        top_bar: kobo_ui::TopBarState::Hidden,
        id: 1,
        protocol: kobo_protocol::TEXT_ENTRY_VERSION,
        name: name.into(),
        path: root.join(name),
        jail: None,
        identity_slot: None,
        child: super::ApplicationChild::ordinary(child),
        stream: runtime,
        store: kobo_policy::store::Store::new(root.join("state")),
        shelf: kobo_policy::shelf::Shelf::new(root.join("shelf")),
        tasks: TaskRunner::simulated(root.join("data")),
        declared: kobo_policy::Declared::all(),
        shells: kobo_shell::Shells::new(&[]),
        screen: None,
        pictures: super::PictureCache::default(),
        fonts: kobod::fonts::FontOwner::default(),
        orientation: kobo_ui::Orientation::Portrait,
        landscape_turn: kobo_ui::LandscapeTurn::Clockwise,
        painted: 0,
        started: std::time::Instant::now(),
        used: std::time::Instant::now(),
        typing: None,
    };
    (hosted, client, root)
}

fn ask(kind: FieldKind) -> DeviceRequest {
    DeviceRequest::TypeText {
        prompt: "New item".into(),
        hint: String::new(),
        initial: String::new(),
        submit: "Add".into(),
        kind,
        save_as: (kind == FieldKind::Secret).then(|| SecretTarget::named("lichess")),
    }
}

/// Taps the centre of whatever on the panel sends `action`, the way a finger
/// would, and says what the runtime made of it.
fn tap(app: &mut Hosted, action: ActionId) -> Tap {
    let screen = visible(app).expect("a field is drawn");
    let chrome = Chrome::for_screen(&screen, false, None);
    let layout = screen.layout_with(&super::metrics_for(&screen), &chrome);
    let rect = layout
        .nodes
        .iter()
        .find(|node| match node.kind {
            LayoutKind::Cell(hit, ..) => hit == action,
            // The way out is the shell's Back in the top bar.
            LayoutKind::Back => action == ActionId::BACK,
            _ => false,
        })
        .map(|node| node.rect)
        .expect("the key is drawn");
    let event = TouchEvent::Up {
        x: u32::try_from(rect.x + rect.width / 2).expect("on the panel"),
        y: u32::try_from(rect.y + rect.height / 2).expect("on the panel"),
    };
    type_into(
        app,
        event,
        Some(&screen),
        &chrome,
        kobo_ui::LandscapeTurn::Clockwise,
    )
    .expect("the tap is routed")
}

fn answer(client: &mut std::os::unix::net::UnixStream, request_id: u32) -> TextOutcome {
    let frame = kobo_protocol::read_from(client).expect("an answer");
    assert_eq!(
        frame.request_id, request_id,
        "answers the request it was asked"
    );
    assert_eq!(frame.version, kobo_protocol::TEXT_ENTRY_VERSION);
    let Message::DeviceResult(DeviceResult::Text(outcome)) = frame.message else {
        panic!("a text answer: {:?}", frame.message)
    };
    outcome
}

#[test]
fn the_runtime_types_into_its_own_field_and_answers_the_application_once() {
    let (mut app, mut client, root) = hosted("todo");
    open_text_field(&mut app, 9, &ask(FieldKind::Text)).unwrap();
    for key in ["kb.shift", "kb.r0c0", "kb.r0c1", "kb.space", "kb.r1c0"] {
        assert_eq!(
            tap(&mut app, ActionId::from_name(key)),
            Tap::Redraw,
            "{key}"
        );
    }
    assert!(
        format!("{:?}", visible(&app)).contains("Qw a"),
        "the field shows what was typed"
    );
    assert_eq!(tap(&mut app, ActionId::from_name("kb.enter")), Tap::Handled);
    assert_eq!(answer(&mut client, 9), TextOutcome::Typed("Qw a".into()));
    assert!(
        matches!(app.typing, Some(TextField::Answered(_))),
        "the field stays drawn until the application draws what comes next"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn back_cancels_a_field_and_a_newer_request_replaces_an_open_one() {
    let (mut app, mut client, root) = hosted("todo");
    open_text_field(&mut app, 3, &ask(FieldKind::Number)).unwrap();
    assert!(
        format!("{:?}", visible(&app)).contains("label: \"abc\""),
        "a number opens on the digits"
    );
    open_text_field(&mut app, 4, &ask(FieldKind::Address)).unwrap();
    assert_eq!(answer(&mut client, 3), TextOutcome::Cancelled);
    tap(&mut app, ActionId::from_name("kb.at"));
    assert_eq!(tap(&mut app, ActionId::BACK), Tap::Handled);
    assert_eq!(answer(&mut client, 4), TextOutcome::Cancelled);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn a_secret_an_application_may_not_keep_is_refused_without_drawing_a_keyboard() {
    let (mut app, mut client, root) = hosted("todo");
    open_text_field(&mut app, 5, &ask(FieldKind::Secret)).unwrap();
    assert!(app.typing.is_none(), "nothing was opened to type into");
    assert_eq!(
        answer(&mut client, 5),
        TextOutcome::Denied(DenyReason::NotDeclared)
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn an_application_never_shares_an_identity_slot_with_one_still_alive() {
    use kobo_abi::sandbox::IDENTITY_SLOTS;
    let pool = u64::from(IDENTITY_SLOTS);
    assert_eq!(super::identity_slot_for([].into_iter(), 5), 5);
    assert_eq!(super::identity_slot_for([].into_iter(), pool + 3), 3);
    // The preferred slot belongs to a launch one pool earlier that is
    // still running, and the lowest free slot is taken instead.
    assert_eq!(super::identity_slot_for([3, 0].into_iter(), pool + 3), 1);
    // Nothing free is a state the host never reaches; it is answered with
    // the preferred slot rather than a refusal to launch anything.
    assert_eq!(super::identity_slot_for(0..IDENTITY_SLOTS, 7), 7);
}

use kobo_policy::{Capability, TaskRunner};
use kobo_protocol::{
    Credential, CredentialUse, DeviceResult, Frame, Header, Message, Task, TaskId, TaskOutcome,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::{ServerConfig, ServerConnection, StreamOwned};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Once};
use std::thread;
use std::time::Duration;

const MOCK_CA: &[u8] = include_bytes!("../../../kobo-net/tests/fixtures/localhost-ca.der");
const MOCK_CERTIFICATE: &[u8] =
    include_bytes!("../../../kobo-net/tests/fixtures/localhost-cert.der");
const MOCK_PRIVATE_KEY: &[u8] =
    include_bytes!("../../../kobo-net/tests/fixtures/localhost-key.der");
const SEEK_BODY: &str = "rated=true&time=10&increment=0&variant=standard&color=random";
const FORM: &str = "application/x-www-form-urlencoded";
static HOSTED_PEER: AtomicUsize = AtomicUsize::new(0);

#[test]
fn launch_splash_covers_the_panel_and_centres_the_mark_without_chrome() {
    for (width, height) in [(600_i32, 800_i32), (1072, 1448), (1440, 1920), (1872, 1404)] {
        let surface = super::launch_surface(
            kobo_hal::Rect {
                x: 0,
                y: 0,
                width: u32::try_from(width).unwrap(),
                height: u32::try_from(height).unwrap(),
            },
            false,
        );
        assert_eq!(surface.width, usize::try_from(width).unwrap());
        assert_eq!(surface.height, usize::try_from(height).unwrap());
        let mark_width = width.min(height) * 3 / 5;
        let mark_height = mark_width * super::LOGO_HEIGHT / super::LOGO_WIDTH;
        for (index, pixel) in surface.pixels.iter().enumerate() {
            if *pixel == kobo_ui::tone::PAPER {
                continue;
            }
            let x = i32::try_from(index % surface.width).unwrap();
            let y = i32::try_from(index / surface.width).unwrap();
            assert!(x >= (width - mark_width) / 2 && x < (width + mark_width) / 2);
            assert!(y >= (height - mark_height) / 2 && y < (height + mark_height) / 2);
        }
        assert!(surface
            .pixels
            .iter()
            .any(|pixel| *pixel != kobo_ui::tone::PAPER));
    }
}

#[test]
fn launch_logo_keeps_its_blue_and_a_legible_greyscale_plane() {
    let mut surface = kobo_ui::Surface::new(264, 111);
    super::draw_cobalt_logo(
        &mut surface,
        kobo_ui::Rect {
            x: 0,
            y: 0,
            width: 264,
            height: 111,
        },
        true,
    );
    let at = |x: usize, y: usize| y * surface.width + x;
    assert_eq!(surface.rgb_at(at(120, 10)), Some(super::COBALT_BLUE));
    assert_eq!(surface.rgb_at(at(8, 8)), Some([255; 3]));
    assert_eq!(surface.rgb_at(at(17, 17)), Some(super::COBALT_BLUE));
    assert_eq!(surface.rgb_at(at(129, 40)), Some([255; 3]));
    assert!(surface.has_colour());
    assert!(surface.pixels[at(120, 10)] > 0);
    assert!(surface.pixels[at(120, 10)] < 255);

    let mut greyscale = kobo_ui::Surface::new(264, 111);
    super::draw_cobalt_logo(
        &mut greyscale,
        kobo_ui::Rect {
            x: 0,
            y: 0,
            width: 264,
            height: 111,
        },
        false,
    );
    assert!(!greyscale.has_colour());
    assert_eq!(greyscale.pixels[at(120, 10)], kobo_ui::tone::INK);

    let source = include_str!("../../../../assets/cobalt-logo.svg");
    assert!(source.contains("viewBox=\"0 0 264 111\""));
    assert!(source.contains("#6E93D6"));
}

#[test]
#[ignore = "writes an explicit launch preview for visual review"]
fn export_launch_splash_preview() {
    let path = std::env::var("COBALT_LAUNCH_PREVIEW").expect("preview output path");
    let surface = super::launch_surface(
        kobo_hal::Rect {
            x: 0,
            y: 0,
            width: 1072,
            height: 1448,
        },
        false,
    );
    let mut bytes = format!("P5\n{} {}\n255\n", surface.width, surface.height).into_bytes();
    bytes.extend_from_slice(&surface.pixels);
    std::fs::write(path, bytes).expect("write launch preview");
}

fn trust_mock_root() {
    static TRUST: Once = Once::new();
    TRUST.call_once(|| {
        kobo_net::trust_owner_root(MOCK_CA.to_vec()).expect("install mock root");
    });
}

fn mock_server_config() -> Arc<ServerConfig> {
    let certificate = CertificateDer::from(MOCK_CERTIFICATE.to_vec());
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(MOCK_PRIVATE_KEY.to_vec()));
    Arc::new(
        ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .expect("protocol versions")
            .with_no_client_auth()
            .with_single_cert(vec![certificate], key)
            .expect("mock certificate"),
    )
}

fn accept_mock(
    listener: &TcpListener,
    config: Arc<ServerConfig>,
) -> (StreamOwned<ServerConnection, TcpStream>, Vec<u8>) {
    let (socket, _) = listener.accept().expect("accept mock client");
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout");
    let connection = ServerConnection::new(config).expect("server connection");
    let mut stream = StreamOwned::new(connection, socket);
    let mut request = Vec::new();
    let mut buffer = [0_u8; 2048];
    loop {
        let read = stream.read(&mut buffer).expect("read request");
        assert!(read > 0, "client closed before request");
        request.extend_from_slice(&buffer[..read]);
        assert!(request.len() < 32 * 1024, "oversized test request");
        let Some(head_end) = request
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map(|position| position + 4)
        else {
            continue;
        };
        let head = std::str::from_utf8(&request[..head_end]).expect("request head");
        let content_length = head
            .split("\r\n")
            .find_map(|line| {
                line.strip_prefix("Content-Length: ")
                    .and_then(|value| value.parse::<usize>().ok())
            })
            .unwrap_or(0);
        if request.len() >= head_end + content_length {
            return (stream, request);
        }
    }
}

fn has_header(request: &[u8], prefix: &str) -> bool {
    std::str::from_utf8(request)
        .is_ok_and(|request| request.split("\r\n").any(|line| line.starts_with(prefix)))
}

fn request_body(request: &[u8]) -> &[u8] {
    let start = request
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("request separator")
        + 4;
    &request[start..]
}

fn test_root(name: &str) -> PathBuf {
    let target = std::env::var_os("CARGO_TARGET_DIR").map_or_else(
        || {
            std::env::current_dir()
                .expect("current directory")
                .join("target")
        },
        PathBuf::from,
    );
    target
        .join("kobod-test-state")
        .join(format!("{name}-{}", std::process::id()))
}

fn through_protocol_11(task: TaskId, work: Task) -> (TaskId, Task) {
    let mut encoded = Vec::new();
    kobo_protocol::write_to(
        &mut encoded,
        &Frame {
            version: kobo_protocol::LEGACY_VERSION,
            request_id: task.0,
            message: Message::Spawn { task, work },
        },
    )
    .expect("encode protocol-11 task");
    let decoded =
        kobo_protocol::read_from(&mut std::io::Cursor::new(encoded)).expect("decode task");
    assert_eq!(decoded.version, kobo_protocol::LEGACY_VERSION);
    assert_eq!(decoded.request_id, task.0);
    match decoded.message {
        Message::Spawn { task, work } => (task, work),
        other => panic!("decoded the wrong message: {other:?}"),
    }
}

fn stream_work(url: &str, action: &str) -> Task {
    Task::Fetch {
        url: url.to_owned(),
        offset: 0,
        max_bytes: 128 * 1024,
        credential: Some(Credential::bearer("lichess")),
        headers: vec![
            Header::new("Accept", "application/x-ndjson"),
            Header::new("X-Cobalt-Line-Stream", action),
            Header::new("X-Cobalt-Rate-Limit", "1"),
        ],
    }
}

struct EventMock {
    url: String,
    release: mpsc::Sender<()>,
    server: thread::JoinHandle<()>,
}

fn start_event_mock(config: Arc<ServerConfig>) -> EventMock {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind event mock");
    let port = listener.local_addr().expect("event address").port();
    let (release, wait) = mpsc::channel();
    let server = thread::spawn(move || {
        let (mut stream, request) = accept_mock(&listener, config);
        assert!(request.starts_with(b"GET /api/stream/event HTTP/1.1\r\n"));
        assert!(has_header(&request, "Authorization: Bearer "));
        assert!(has_header(&request, "Accept: application/x-ndjson"));
        assert!(!has_header(&request, "X-Cobalt-"));
        stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nTransfer-Encoding: chunked\r\nConnection: keep-alive\r\n\r\n",
                )
                .expect("event response head");
        stream.write_all(b"1\r\n\n\r\n").expect("event keepalive");
        let event = br#"{"type":"gameStart","game":{"id":"abcdEF12"}}"#;
        write!(stream, "{:x}\r\n", event.len() + 1).expect("event chunk size");
        stream.write_all(event).expect("event record");
        stream.write_all(b"\n\r\n").expect("event frame");
        stream.flush().expect("flush event");
        let _ = wait.recv_timeout(Duration::from_secs(5));
    });
    EventMock {
        url: format!("https://localhost:{port}/api/stream/event"),
        release,
        server,
    }
}

struct SeekMock {
    url: String,
    requests: Arc<AtomicUsize>,
    started: mpsc::Receiver<()>,
    release: mpsc::Sender<()>,
    server: thread::JoinHandle<()>,
}

fn start_seek_mock(config: Arc<ServerConfig>) -> SeekMock {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind seek mock");
    let port = listener.local_addr().expect("seek address").port();
    let requests = Arc::new(AtomicUsize::new(0));
    let observed_requests = Arc::clone(&requests);
    let (report_start, started) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let server = thread::spawn(move || {
        let (mut stream, request) = accept_mock(&listener, config);
        observed_requests.fetch_add(1, Ordering::SeqCst);
        assert!(request.starts_with(b"POST /api/board/seek HTTP/1.1\r\n"));
        assert!(has_header(
            &request,
            "Content-Type: application/x-www-form-urlencoded"
        ));
        assert!(has_header(&request, "Authorization: Bearer "));
        assert!(!has_header(&request, "X-Cobalt-"));
        assert_eq!(request_body(&request), SEEK_BODY.as_bytes());
        stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nTransfer-Encoding: chunked\r\nConnection: keep-alive\r\n\r\n1\r\n\n\r\n",
                )
                .expect("seek response");
        stream.flush().expect("flush seek response");
        report_start.send(()).expect("report seek");
        let _ = wait.recv_timeout(Duration::from_secs(5));
    });
    SeekMock {
        url: format!("https://localhost:{port}/api/board/seek"),
        requests,
        started,
        release,
        server,
    }
}

fn assert_outcome(runner: &mut TaskRunner, task: TaskId, expected: &TaskOutcome) {
    let finished = runner
        .wait(Duration::from_secs(5))
        .unwrap_or_else(|| panic!("task {} did not finish", task.0));
    assert_eq!(finished.task, task);
    assert_eq!(&finished.outcome, expected);
}

#[test]
fn native_power_delivery_pauses_tasks_and_resumes_each_host_once() {
    use super::{ApplicationChild, Hosted};
    use kobod::power::{Effect, WakeReason};
    let root =
        std::env::temp_dir().join(format!("cobalt-native-power-host-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let (runtime, mut client) = std::os::unix::net::UnixStream::pair().unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    let child = std::process::Command::new("/usr/bin/true").spawn().unwrap();
    let mut apps = vec![Hosted {
        top_bar: kobo_ui::TopBarState::Hidden,
        id: 1,
        protocol: kobo_protocol::VERSION,
        name: "fixture".into(),
        path: root.join("fixture"),
        jail: None,
        identity_slot: None,
        child: ApplicationChild::ordinary(child),
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
    }];
    let mut power = kobod::power::Power::default();
    apps[0].protocol = kobod::power::MIN_APP_PROTOCOL - 1;
    assert_eq!(
        super::begin_power(
            &mut power,
            &apps,
            0,
            kobod::power::SleepReason::Owner,
            kobod::power::Conditions {
                charging: false,
                usb_attached: false,
                keep_awake_until: 0,
                terminal_open: false,
                input_quiet: true,
                panel_idle: true,
                tasks_idle: true
            }
        ),
        Err(kobod::power::Refusal::UnsupportedApp)
    );
    assert_eq!(power.state(), kobod::power::State::Awake);
    assert!(
        !apps[0].tasks.is_quiescent(),
        "an old app must not have its task admission paused"
    );
    apps[0].protocol = kobo_protocol::VERSION;
    assert!(!super::apply_power_effect(&mut apps, Effect::Prepare { generation: 4 }).unwrap());
    assert!(apps[0].tasks.is_quiescent());
    assert_eq!(
        kobo_protocol::read_from(&mut client).unwrap().message,
        Message::PrepareSuspend { generation: 4 }
    );
    assert!(!super::apply_power_effect(
        &mut apps,
        Effect::Resume {
            generation: 4,
            reason: WakeReason::Touch
        }
    )
    .unwrap());
    assert!(!apps[0].tasks.is_quiescent());
    assert_eq!(
        kobo_protocol::read_from(&mut client).unwrap().message,
        Message::Resume {
            generation: 4,
            reason: WakeReason::Touch
        }
    );
    assert!(super::apply_power_effect(&mut apps, Effect::Enter { generation: 4 }).is_err());
    assert!(super::apply_power_effect(&mut apps, Effect::Handback { generation: 4 }).unwrap());
    apps[0].child.process.wait().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn cancelled_touch_cannot_activate_an_app_control() {
    let (mut writer, mut reader) = std::os::unix::net::UnixStream::pair().unwrap();
    reader.set_nonblocking(true).unwrap();
    let result = super::deliver_touch(
        &mut writer,
        kobo_hal::touch::TouchEvent::Cancel,
        None,
        &kobo_ui::Chrome::default(),
        false,
        kobo_ui::Orientation::Portrait,
        kobo_ui::LandscapeTurn::Clockwise,
        kobo_protocol::VERSION,
        kobo_ui::TopBarState::Hidden,
    )
    .unwrap();
    assert!(matches!(result, super::Tap::Handled));
    let mut bytes = [0_u8; 1];
    assert_eq!(
        std::io::Read::read(&mut reader, &mut bytes)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[test]
fn an_application_cannot_change_protocol_version_after_greeting() {
    let (runtime, mut application) = std::os::unix::net::UnixStream::pair().expect("socket pair");
    let (sender, receiver) = std::sync::mpsc::channel();
    super::pump_application(&runtime, &sender, 42, kobo_protocol::VERSION)
        .expect("start application pump");
    kobo_protocol::write_to(
        &mut application,
        &kobo_protocol::Frame {
            version: kobo_protocol::LEGACY_VERSION,
            request_id: 1,
            message: kobo_protocol::Message::Exit,
        },
    )
    .expect("send mixed-version frame");
    assert!(matches!(
        receiver
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("application rejection"),
        super::Event::AppGone(42)
    ));
}

fn hosted_peer(protocol: u8) -> (super::Hosted, std::os::unix::net::UnixStream, PathBuf) {
    use super::{ApplicationChild, Hosted};
    let root = std::env::temp_dir().join(format!(
        "cobalt-protocol-echo-{}-{}-{}",
        protocol,
        std::process::id(),
        HOSTED_PEER.fetch_add(1, Ordering::SeqCst)
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
        protocol,
        name: "todo".into(),
        path: root.join("todo"),
        jail: None,
        identity_slot: None,
        child: ApplicationChild::ordinary(child),
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
    };
    (hosted, client, root)
}

fn assert_runtime_frame(
    client: &mut std::os::unix::net::UnixStream,
    protocol: u8,
    request_id: u32,
    expected: &Message,
) {
    let frame = kobo_protocol::read_from(client).expect("runtime frame");
    assert_eq!(frame.version, protocol, "wire version");
    assert_eq!(frame.request_id, request_id, "request id");
    assert_eq!(&frame.message, expected);
}

fn reading_page() -> kobo_ui::Screen {
    kobo_ui::Screen::new(
        1,
        vec![kobo_ui::Node::Text {
            id: kobo_ui::NodeId(1),
            text: "A page of a book.".to_owned(),
            links: Vec::new(),
        }],
    )
    .with_page_turns(kobo_ui::ActionId(11), kobo_ui::ActionId(12))
    .with_hold(kobo_ui::ActionId(13))
}

fn content_tap(screen: &kobo_ui::Screen) -> kobo_hal::touch::TouchEvent {
    let metrics = super::metrics_for(screen);
    let content = screen
        .layout_with(&metrics, &kobo_ui::Chrome::default())
        .content;
    kobo_hal::touch::TouchEvent::Up {
        x: u32::try_from(metrics.width / 2).expect("inside the panel"),
        y: u32::try_from(content.y + content.height / 2).expect("inside the panel"),
    }
}

fn selectable_page() -> kobo_ui::Screen {
    kobo_ui::Screen::new(
        1,
        vec![kobo_ui::Node::RichText {
            id: kobo_ui::NodeId(1),
            text: "naïve café".into(),
            spans: Vec::new(),
            links: Vec::new(),
            presentation: kobo_ui::ParagraphPresentation::default(),
            selection: Some(kobo_ui::TextSelection {
                context: 19,
                offset: 100,
            }),
            formulae: Vec::new(),
        }],
    )
    .with_reading(true)
    .with_hold(kobo_ui::ActionId(13))
}

fn text_tap(screen: &kobo_ui::Screen) -> kobo_hal::touch::TouchEvent {
    let metrics = super::metrics_for(screen);
    let layout = screen.layout_with(&metrics, &kobo_ui::Chrome::default());
    let (rect, _) = layout.text_hits.first().expect("selectable word");
    kobo_hal::touch::TouchEvent::Up {
        x: u32::try_from(rect.x + rect.width / 2).expect("inside the word"),
        y: u32::try_from(rect.y + rect.height / 2).expect("inside the word"),
    }
}

fn runtime_keeps_protocol_on_send_and_reply(protocol: u8) {
    let (mut hosted, mut client, root) = hosted_peer(protocol);
    hosted
        .send(Message::Lifecycle(kobo_protocol::Lifecycle::Foreground))
        .expect("send");
    assert_runtime_frame(
        &mut client,
        protocol,
        0,
        &Message::Lifecycle(kobo_protocol::Lifecycle::Foreground),
    );
    super::reply(&mut hosted, 7, Message::DeviceResult(DeviceResult::Done)).expect("reply");
    assert_runtime_frame(
        &mut client,
        protocol,
        7,
        &Message::DeviceResult(DeviceResult::Done),
    );
    hosted.child.process.wait().ok();
    let _ignored = std::fs::remove_dir_all(root);
}

fn runtime_keeps_protocol_on_tap_and_hold(protocol: u8) {
    let screen = reading_page();
    let chrome = kobo_ui::Chrome::default();
    let tap = content_tap(&screen);
    let words = selectable_page();
    let word_tap = text_tap(&words);
    let (mut runtime, mut app) = std::os::unix::net::UnixStream::pair().expect("a pair of sockets");
    app.set_read_timeout(Some(Duration::from_secs(1)))
        .expect("read timeout");
    assert_eq!(
        super::deliver_touch(
            &mut runtime,
            tap,
            Some(&screen),
            &chrome,
            false,
            kobo_ui::Orientation::Portrait,
            kobo_ui::LandscapeTurn::Clockwise,
            protocol,
            kobo_ui::TopBarState::Hidden,
        )
        .expect("page turn"),
        super::Tap::Handled
    );
    let turned = kobo_protocol::read_from(&mut app).expect("action");
    assert_eq!(turned.version, protocol);
    assert_eq!(turned.request_id, 0);
    assert_eq!(
        turned.message,
        Message::Action {
            action: kobo_ui::ActionId(12)
        }
    );

    assert_eq!(
        super::deliver_touch(
            &mut runtime,
            word_tap,
            Some(&words),
            &chrome,
            true,
            kobo_ui::Orientation::Portrait,
            kobo_ui::LandscapeTurn::Clockwise,
            protocol,
            kobo_ui::TopBarState::Hidden,
        )
        .expect("text hold"),
        super::Tap::Handled
    );
    let hold_frame = kobo_protocol::read_from(&mut app).expect("hold");
    assert_eq!(hold_frame.version, protocol);
    assert_eq!(hold_frame.request_id, 0);
    assert!(
        matches!(hold_frame.message, Message::TextHold { action, .. } if action == kobo_ui::ActionId(13)),
        "{:?}",
        hold_frame.message
    );
}

#[test]
fn a_protocol_10_app_keeps_receiving_protocol_10() {
    // The oldest protocol in the release registry. Refusing it left Store
    // applications built for it unable to open after a platform update.
    runtime_keeps_protocol_on_send_and_reply(kobo_protocol::DICTIONARY_VERSION);
    runtime_keeps_protocol_on_tap_and_hold(kobo_protocol::DICTIONARY_VERSION);
}

#[test]
fn a_protocol_11_app_keeps_receiving_protocol_11() {
    runtime_keeps_protocol_on_send_and_reply(kobo_protocol::LEGACY_VERSION);
    runtime_keeps_protocol_on_tap_and_hold(kobo_protocol::LEGACY_VERSION);
}

#[test]
fn a_protocol_12_app_keeps_receiving_protocol_12() {
    runtime_keeps_protocol_on_send_and_reply(kobo_protocol::FOLIO_VERSION);
    runtime_keeps_protocol_on_tap_and_hold(kobo_protocol::FOLIO_VERSION);
}

#[test]
fn a_protocol_13_app_keeps_receiving_protocol_13() {
    // #191: Todo compiled for 13 died with UnsupportedVersion(14) on the
    // first runtime-originated frame. Elipsa rotation session-rotation1.log
    // is the same shape with 13: tictactoe ended after a tap.
    runtime_keeps_protocol_on_send_and_reply(kobo_protocol::SELECTED_GRID_VERSION);
    runtime_keeps_protocol_on_tap_and_hold(kobo_protocol::SELECTED_GRID_VERSION);
}

#[test]
fn a_protocol_14_app_keeps_receiving_protocol_14() {
    // Named after the version it means, not after whichever version is
    // newest: written as VERSION this stopped covering protocol 14 the
    // moment the current version moved, and every published application
    // is protocol 14.
    runtime_keeps_protocol_on_send_and_reply(kobo_protocol::SUSPEND_VERSION);
    runtime_keeps_protocol_on_tap_and_hold(kobo_protocol::SUSPEND_VERSION);
}

#[test]
fn an_application_built_today_keeps_receiving_the_current_protocol() {
    runtime_keeps_protocol_on_send_and_reply(kobo_protocol::VERSION);
    runtime_keeps_protocol_on_tap_and_hold(kobo_protocol::VERSION);
}

#[test]
fn a_greeted_protocol_13_app_survives_the_first_runtime_frame() {
    let directory =
        std::env::temp_dir().join(format!("cobalt-protocol-greet-{}", std::process::id()));
    let _ignored = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("scratch");
    let socket = directory.join("runtime.sock");
    let listener = std::os::unix::net::UnixListener::bind(&socket).expect("listen");
    let client_socket = socket.clone();
    let client = thread::spawn(move || {
        let mut stream = std::os::unix::net::UnixStream::connect(client_socket).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("timeout");
        kobo_protocol::write_to(
            &mut stream,
            &Frame {
                version: kobo_protocol::SELECTED_GRID_VERSION,
                request_id: 1,
                message: Message::Hello {
                    name: "todo".to_owned(),
                },
            },
        )
        .expect("hello");
        let welcome = kobo_protocol::read_from(&mut stream).expect("welcome");
        assert_eq!(welcome.version, kobo_protocol::SELECTED_GRID_VERSION);
        assert!(matches!(welcome.message, Message::Welcome { .. }));
        let first = kobo_protocol::read_from(&mut stream).expect("first runtime frame");
        assert_eq!(first.version, kobo_protocol::SELECTED_GRID_VERSION);
        assert_eq!(first.request_id, 0);
        assert_eq!(
            first.message,
            Message::Lifecycle(kobo_protocol::Lifecycle::Foreground)
        );
    });
    let panel = kobo_hal::Rect {
        x: 0,
        y: 0,
        width: 1072,
        height: 1448,
    };
    let (stream, name, version) = super::greet(&listener, panel, "todo").expect("greet");
    assert_eq!(name, "todo");
    assert_eq!(version, kobo_protocol::SELECTED_GRID_VERSION);
    let (mut hosted, _unused, root) = hosted_peer(version);
    hosted.stream = stream;
    hosted
        .send(Message::Lifecycle(kobo_protocol::Lifecycle::Foreground))
        .expect("foreground");
    client.join().expect("client");
    hosted.child.process.wait().ok();
    let _ignored = std::fs::remove_dir_all(root);
    let _ignored = std::fs::remove_dir_all(directory);
}

#[test]
fn protocol_11_lichess_consumes_host_secret_for_stream_and_cancellable_seek() {
    trust_mock_root();
    let config = mock_server_config();
    let event = start_event_mock(Arc::clone(&config));
    let seek_mock = start_seek_mock(config);

    let root = test_root("protocol-11-lichess");
    let _ignored = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("test secret root");
    std::fs::write(root.join("lichess"), "generated-test-credential").expect("test credential");
    assert!(!root.join("apps/lichess/lichess").exists());
    let allowed_event = event.url.clone();
    let allowed_seek = seek_mock.url.clone();
    let mut runner = TaskRunner::simulated(root.join("data"))
        .with_fetch(Arc::new(kobo_net::fetch_from_controlled))
        .with_post(Arc::new(kobo_net::post_controlled))
        .with_updates(Arc::new(kobo_net::write_controlled))
        .with_line_streams(Arc::new(kobo_net::LineStreams::default()))
        .with_app_secrets(&root, "lichess")
        .with_credential_policy(Arc::new(
            move |credential, url, usage, body, content_type, _server| {
                credential == &Credential::bearer("lichess")
                    && match usage {
                        CredentialUse::Fetch => {
                            url == allowed_event && body.is_none() && content_type.is_none()
                        }
                        CredentialUse::Post => {
                            url == allowed_seek
                                && body == Some(SEEK_BODY)
                                && content_type == Some(FORM)
                        }
                        CredentialUse::Put | CredentialUse::Patch => false,
                    }
            },
        ))
        .with_capabilities([Capability::Network]);

    for (id, action, expected) in [
        (TaskId(1), "open", TaskOutcome::Completed(Vec::new())),
        (
            TaskId(2),
            "next",
            TaskOutcome::Completed(br#"{"type":"gameStart","game":{"id":"abcdEF12"}}"#.to_vec()),
        ),
    ] {
        let (task, work) = through_protocol_11(id, stream_work(&event.url, action));
        runner.submit(task, work).expect("submit stream task");
        assert_outcome(&mut runner, task, &expected);
    }

    let seek = TaskId(3);
    let (seek, work) = through_protocol_11(
        seek,
        Task::Post {
            url: seek_mock.url.clone(),
            body: SEEK_BODY.to_owned(),
            content_type: FORM.to_owned(),
            credential: Some(Credential::bearer("lichess")),
            headers: vec![
                Header::new("X-Cobalt-Wait-Until-Cancelled", "1"),
                Header::new("X-Cobalt-Rate-Limit", "1"),
            ],
            max_bytes: 4096,
        },
    );
    runner.submit(seek, work).expect("submit seek");
    seek_mock
        .started
        .recv_timeout(Duration::from_secs(5))
        .expect("seek reached mock");
    runner.cancel(seek);
    assert_outcome(&mut runner, seek, &TaskOutcome::Cancelled);
    assert_eq!(
        seek_mock.requests.load(Ordering::SeqCst),
        1,
        "the non-idempotent seek was replayed"
    );

    let close = TaskId(4);
    let (close, work) = through_protocol_11(close, stream_work(&event.url, "close"));
    runner.submit(close, work).expect("submit stream close");
    assert_outcome(&mut runner, close, &TaskOutcome::Completed(Vec::new()));

    seek_mock.release.send(()).expect("release seek mock");
    event.release.send(()).expect("release event mock");
    seek_mock.server.join().expect("seek mock");
    event.server.join().expect("event mock");
    drop(runner);
    std::fs::remove_dir_all(root).expect("remove test state");
}

#[test]
fn app_file_roots_are_private_per_app() {
    let nonograms = super::app_data_root("nonograms");
    let panels = super::app_data_root("panels");
    assert_ne!(nonograms, panels);
    assert_eq!(
        nonograms,
        std::path::Path::new("/mnt/onboard/.adds/cobalt/data/nonograms")
    );
}

/// A zone file as the C library sees it: a version 2 header, a table this
/// code never reads, and the POSIX rule on the last line.
fn zone(rule: &str) -> Vec<u8> {
    let mut bytes = b"TZif2\0\0\0binary transition table\n".to_vec();
    bytes.extend_from_slice(format!("\n{rule}\n").as_bytes());
    bytes
}

#[test]
fn the_clock_reads_the_offset_posix_spells_backwards() {
    // POSIX inverts the sign everyone expects: EST5 is five hours *behind*
    // UTC, not ahead. Getting this backwards puts the band ten hours out.
    let cases = [
        ("UTC0", 0),
        ("EST5", -5 * 60),
        ("IST-5:30", 5 * 60 + 30),
        ("CET-1", 60),
        ("NZST-12:45", 12 * 60 + 45),
        // An empty TZ is UTC to the C library, and so is anything not
        // understood rather than a guess.
        ("", 0),
        ("Etc/Unknown", 0),
    ];
    for (tz, want) in cases {
        assert_eq!(
            super::offset_minutes(Some(tz), |_| None),
            want,
            "TZ={tz} was read as the wrong offset"
        );
    }
    assert_eq!(
        super::offset_minutes(None, |_| None),
        0,
        "a device with no TZ and no zone file did not fall back to UTC"
    );
}

#[test]
fn without_tz_the_offset_comes_from_the_zone_file_the_c_library_reads() {
    let localtime = |path: &std::path::Path| {
        (path == std::path::Path::new("/etc/localtime")).then(|| zone("IST-5:30"))
    };
    assert_eq!(super::offset_minutes(None, localtime), 330);
    // A TZ that says something wins over the file, as it does for the C
    // library.
    assert_eq!(super::offset_minutes(Some("EST5"), localtime), -300);
    // A version 1 file has no rule to read, and an unreadable one is not a
    // zone at all. Both are UTC rather than a guess.
    assert_eq!(
        super::offset_minutes(None, |_| Some(b"TZif\0\0\0\0table\nIST-5:30\n".to_vec())),
        0
    );
    assert_eq!(
        super::offset_minutes(None, |_| Some(b"not a zone".to_vec())),
        0
    );
}

#[test]
fn a_named_zone_is_read_from_the_zone_directory_and_cannot_climb_out_of_it() {
    let zoneinfo = |path: &std::path::Path| match path.to_str() {
        Some("/usr/share/zoneinfo/America/New_York") => Some(zone("EST5EDT,M3.2.0,M11.1.0")),
        Some("/usr/share/zoneinfo/Asia/Kathmandu") => Some(zone("<+0545>-5:45")),
        Some("/etc/zones/Here") => Some(zone("<+03>-3")),
        _ => None,
    };
    assert_eq!(
        super::offset_minutes(Some("America/New_York"), zoneinfo),
        -300
    );
    assert_eq!(
        super::offset_minutes(Some(":Asia/Kathmandu"), zoneinfo),
        345
    );
    assert_eq!(
        super::offset_minutes(Some(":/etc/zones/Here"), zoneinfo),
        180
    );
    for escape in ["../../etc/zones/Here", "Asia//Kathmandu", ":"] {
        assert_eq!(super::zone_file(escape), None, "{escape}");
    }
}

#[test]
fn every_application_is_started_with_the_readers_offset_and_nothing_else_of_ours() {
    let socket = std::path::Path::new("/tmp/kobo-session-test.sock");
    let mut command = super::application_command(std::path::Path::new("/usr/bin/env"), socket, 330);
    command.stdout(std::process::Stdio::piped());
    let output = command.output().expect("run env as an application");
    assert!(output.status.success());
    let mut seen = String::from_utf8(output.stdout)
        .expect("env prints text")
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    seen.sort();
    assert_eq!(
        seen,
        [
            format!("KOBO_SOCKET={}", socket.display()),
            format!("{}=330", kobo_policy::clock::READER_OFFSET_VARIABLE),
        ]
    );
}

#[test]
fn the_clock_is_two_fields_and_never_a_twenty_fifth_hour() {
    let now = super::clock();
    assert_eq!(now.len(), 5, "the clock was not HH:MM: {now}");
    let (hours, minutes) = now.split_once(':').expect("a separator");
    assert!(hours.parse::<u32>().expect("hours") < 24, "{now}");
    assert!(minutes.parse::<u32>().expect("minutes") < 60, "{now}");
}

fn sink(observation: u64, connected: bool) -> kobo_hal::audio::SinkEvidence {
    kobo_hal::audio::SinkEvidence {
        observation,
        connected,
    }
}

#[test]
fn a_repeated_sink_failure_after_a_bluetooth_reply_still_clears_the_mark() {
    let mut status = super::StatusSource::new();
    assert!(!status.observe_audio_sink(Some(sink(1, false))));
    assert!(
        status.observe_bluetooth(true),
        "the reply puts the mark back"
    );
    assert!(
        status.observe_audio_sink(Some(sink(2, false))),
        "a second failure is news even though it says the same thing"
    );
    assert!(!status.bluetooth);
}

#[test]
fn a_repeated_sink_success_after_a_bluetooth_reply_still_sets_the_mark() {
    let mut status = super::StatusSource::new();
    assert!(status.observe_audio_sink(Some(sink(1, true))));
    assert!(status.observe_bluetooth(false));
    assert!(status.observe_audio_sink(Some(sink(2, true))));
    assert!(status.bluetooth);
}

#[test]
fn an_old_sink_observation_never_overrides_a_newer_bluetooth_reply() {
    let mut status = super::StatusSource::new();
    assert!(!status.observe_audio_sink(Some(sink(1, false))));
    assert!(status.observe_bluetooth(true));
    assert!(
        !status.observe_audio_sink(Some(sink(1, false))),
        "polled again, nothing new"
    );
    assert!(status.bluetooth);
}

#[test]
fn a_book_is_drawn_without_the_band_and_everything_else_with_it() {
    let mut status = super::StatusSource::new();
    let reading = Screen::new(1, Vec::new()).with_reading(true);
    assert!(
        super::chrome_for(&reading, false, &mut status)
            .status
            .is_none(),
        "a clock was put above a book"
    );
    let listing = Screen::new(1, Vec::new());
    assert!(
        super::chrome_for(&listing, false, &mut status)
            .status
            .is_some(),
        "an ordinary screen lost its status band"
    );
    // Home has no way back, and the band is independent of that.
    let home = super::chrome_for(&listing, true, &mut status);
    assert!(!home.back, "the launcher was given a way out of itself");
    assert!(home.status.is_some(), "the launcher lost its status band");
}

/// Both answers on the consent notice can actually be tapped.
///
/// The notice is the one screen an owner cannot get past by any other
/// route, so a layout that placed a button where no touch resolves to it
/// would leave them with a stopped reader and nothing to press. Checked by
/// sweeping the panel rather than by reading the button's own rectangle,
/// because what matters is that a tap lands on it, not that it was laid
/// out somewhere.
#[test]
fn both_answers_on_the_consent_notice_can_be_reached_by_a_tap() {
    for touch_may_be_wrong in [false, true] {
        let notice = crate::consent::Notice {
            title: "Untested device".to_owned(),
            body: vec![
                "Cobalt has not been tested on this Kobo.".to_owned(),
                "Cobalt does not change how your Kobo starts up.".to_owned(),
                "Provided without warranty, at your own risk.".to_owned(),
            ],
            touch_may_be_wrong,
        };
        let screen = super::consent_screen(&notice);
        let metrics = super::metrics_for(&screen);
        let layout = screen.layout_with(&metrics, &kobo_ui::Chrome::with_back(false));
        let mut reachable = Vec::new();
        let mut y = 0;
        while y < metrics.height {
            let mut x = 0;
            while x < metrics.width {
                if let Some(action) = layout.hit_test(x, y) {
                    if !reachable.contains(&action) {
                        reachable.push(action);
                    }
                }
                x += 4;
            }
            y += 4;
        }
        assert!(
            reachable.contains(&super::CONSENT_ACCEPT),
            "nothing on the panel accepts (footnote: {touch_may_be_wrong})"
        );
        assert!(
            reachable.contains(&super::CONSENT_CLOSE),
            "nothing on the panel closes (footnote: {touch_may_be_wrong})"
        );
    }
}

/// A rooted application's sub-screen still gets a way back.
///
/// `kobo present` runs one application as home, and every screen it opened
/// from there was drawn with no back control, because back was decided
/// purely by whether the application was the launcher. Settings could
/// reach its Bluetooth pane and then had nothing to return with, on a
/// device whose only other way out is the power button.
#[test]
fn a_rooted_applications_sub_screen_is_still_drawn_a_way_back() {
    let mut status = StatusSource::new();
    let pane = Screen::new(1, Vec::new())
        .with_top_bar(kobo_ui::TopBar::new(kobo_ui::NodeId(1), "Bluetooth"))
        .with_own_back(true);
    let chrome = super::chrome_for(&pane, true, &mut status);
    assert!(
        chrome.back,
        "a screen that owns back was left with no way to use it"
    );

    let rooted = Screen::new(1, Vec::new())
        .with_top_bar(kobo_ui::TopBar::new(kobo_ui::NodeId(1), "Settings"));
    assert!(
        !super::chrome_for(&rooted, true, &mut status).back,
        "an application's own root still has nowhere to go back to"
    );
}

use super::*;

fn hello() -> Screen {
    Screen::new(1, Vec::new()).with_top_bar(kobo_ui::TopBar::new(kobo_ui::NodeId(1), "Hello"))
}

#[test]
fn a_session_is_refused_before_the_reader_is_stopped_when_there_is_nothing_to_run() {
    let missing = std::env::temp_dir().join("kobo-does-not-exist");
    let _ignored = fs::remove_file(&missing);
    let error = preflight(&missing).expect_err("a missing application is refused");
    assert!(error.contains("nothing to run"), "{error}");
    // The message has to say the device is untouched, because the whole
    // point of checking here is that nothing has happened yet.
    assert!(error.contains("Nothing was changed"), "{error}");

    let directory = std::env::temp_dir().join(format!("kobo-preflight-{}", std::process::id()));
    fs::create_dir_all(&directory).expect("make a directory");
    assert!(preflight(&directory)
        .expect_err("a directory is refused")
        .contains("not a file"));

    let unreadable = directory.join("not-executable");
    fs::write(&unreadable, b"#!/bin/sh\n").expect("write a file");
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o644))
        .expect("clear the executable bits");
    assert!(preflight(&unreadable)
        .expect_err("a file that cannot be executed is refused")
        .contains("not executable"));

    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o755))
        .expect("set the executable bits");
    assert!(preflight(&unreadable).is_ok());
    let _ignored = fs::remove_dir_all(&directory);
}

#[test]
fn touches_follow_the_application_that_is_on_the_panel_now() {
    // The defect this covers: the touch receiver can only be taken once,
    // so a pump started per application left the second one with no input
    // at all. Every application after the first ignored every tap.
    let sink = TouchSink::default();
    let (first, launcher) = mpsc::channel();
    sink.set(Some(first));
    sink.send(TouchEvent::Up { x: 1, y: 1 });
    assert!(matches!(
        launcher.try_recv(),
        Ok(Event::Touch(TouchEvent::Up { x: 1, y: 1 }))
    ));

    let (second, application) = mpsc::channel();
    sink.set(Some(second));
    sink.send(TouchEvent::Up { x: 2, y: 2 });
    assert!(matches!(
        application.try_recv(),
        Ok(Event::Touch(TouchEvent::Up { x: 2, y: 2 }))
    ));
    // and not to the application that just closed.
    assert!(launcher.try_recv().is_err());

    // Between applications a tap is dropped rather than queued, so it
    // cannot act on whatever opens next.
    sink.set(None);
    sink.send(TouchEvent::Up { x: 3, y: 3 });
    assert!(application.try_recv().is_err());
}

#[test]
fn the_runtime_draws_a_way_back_only_for_a_launched_application() {
    let home = PathBuf::from("/tmp/kobo-launcher");
    assert!(!&Chrome::with_back(*home.as_path() != *home.as_path()).back);
    assert!(&Chrome::with_back(Path::new("/tmp/kobo-hello") != home).back);
}

#[test]
fn an_application_that_forgot_a_top_bar_still_gets_a_way_back() {
    let bare = Screen::new(1, Vec::new());
    let fixed = kobo_ui::ensure_way_back(bare, &Chrome::with_back(true), "Hello");
    assert_eq!(
        fixed.top_bar.as_ref().map(|bar| bar.title.as_str()),
        Some("Hello")
    );
    // The launcher itself is not given one it did not ask for.
    assert!(
        kobo_ui::ensure_way_back(Screen::new(1, Vec::new()), &Chrome::default(), "Launcher")
            .top_bar
            .is_none()
    );
}

#[test]
fn a_held_finger_reaches_the_hold_and_a_quick_one_turns_the_page() {
    // The two intents land on the same pixels, so the only thing telling
    // them apart is how long the finger stayed down. A reading page is
    // page turns from edge to edge: without this, marking a paragraph has
    // nowhere to be asked for, and with it done wrong every page turn
    // becomes a mark.
    let screen = Screen::new(
        1,
        vec![kobo_ui::Node::Text {
            id: kobo_ui::NodeId(1),
            text: "A page of a book.".to_owned(),
            links: Vec::new(),
        }],
    )
    .with_page_turns(ActionId(11), ActionId(12))
    .with_hold(ActionId(13));
    let chrome = Chrome::default();
    let metrics = metrics_for(&screen);
    let content = screen.layout_with(&metrics, &chrome).content;
    let touch = TouchEvent::Up {
        x: u32::try_from(metrics.width / 2).expect("inside the panel"),
        y: u32::try_from(content.y + content.height / 2).expect("inside the panel"),
    };
    assert_eq!(
        action_for(touch, Some(&screen), &chrome, true),
        Some(ActionId(13))
    );
    assert_eq!(
        action_for(touch, Some(&screen), &chrome, false),
        Some(ActionId(12))
    );
    // A screen that asked for no hold must still turn its pages, however
    // long the finger rested.
    let no_hold = Screen::new(1, Vec::new()).with_page_turns(ActionId(11), ActionId(12));
    assert_eq!(
        action_for(touch, Some(&no_hold), &chrome, true),
        Some(ActionId(12))
    );

    let covered = screen.clone().with_overlay(kobo_ui::Overlay::modal(
        kobo_ui::NodeId(9),
        "Question",
        Vec::new(),
    ));
    assert_eq!(
        text_hold_for_oriented(
            touch,
            Some(&covered),
            &chrome,
            true,
            kobo_ui::Orientation::Portrait,
        ),
        None,
        "covered text received a hold through the modal"
    );
    assert_ne!(
        action_for(touch, Some(&covered), &chrome, true),
        Some(ActionId(13)),
        "the modal leaked the page hold"
    );
}

#[test]
fn landscape_taps_reach_the_same_control_from_either_physical_side() {
    let action = ActionId(77);
    let screen = Screen::new(
        1,
        vec![kobo_ui::Node::Button {
            id: kobo_ui::NodeId(1),
            action,
            label: "Open".to_owned(),
            state: kobo_ui::ControlState::Enabled,
            emphasis: kobo_ui::Emphasis::Primary,
        }],
    );
    let chrome = kobo_ui::Chrome::default();
    let physical = crate::device_metrics();
    let logical_metrics = physical.oriented(kobo_ui::Orientation::Landscape);
    let rect = screen
        .layout_with(&logical_metrics, &chrome)
        .rect_of_action(action)
        .expect("button");
    let logical_x = rect.x + rect.width / 2;
    let logical_y = rect.y + rect.height / 2;

    let clockwise = TouchEvent::Up {
        x: u32::try_from(physical.width - 1 - logical_y).expect("physical x"),
        y: u32::try_from(logical_x).expect("physical y"),
    };
    assert_eq!(
        super::action_for_oriented(
            clockwise,
            Some(&screen),
            &chrome,
            false,
            kobo_ui::Orientation::Landscape,
            kobo_ui::LandscapeTurn::Clockwise,
        ),
        Some(action)
    );

    let counter_clockwise = TouchEvent::Up {
        x: u32::try_from(logical_y).expect("physical x"),
        y: u32::try_from(physical.height - 1 - logical_x).expect("physical y"),
    };
    assert_eq!(
        super::action_for_oriented(
            counter_clockwise,
            Some(&screen),
            &chrome,
            false,
            kobo_ui::Orientation::Landscape,
            kobo_ui::LandscapeTurn::CounterClockwise,
        ),
        Some(action)
    );
}

#[test]
fn landscape_feedback_rect_matches_the_rotated_control() {
    let physical = crate::device_metrics();
    let logical = kobo_ui::Rect {
        x: 10,
        y: 20,
        width: 30,
        height: 40,
    };
    assert_eq!(
        super::physical_feedback_rect(
            logical,
            kobo_ui::Orientation::Landscape,
            kobo_ui::LandscapeTurn::Clockwise,
            &physical,
        ),
        kobo_ui::Rect {
            x: physical.width - 60,
            y: 10,
            width: 40,
            height: 30,
        }
    );
    assert_eq!(
        super::physical_feedback_rect(
            logical,
            kobo_ui::Orientation::Landscape,
            kobo_ui::LandscapeTurn::CounterClockwise,
            &physical,
        ),
        kobo_ui::Rect {
            x: 20,
            y: physical.height - 40,
            width: 40,
            height: 30,
        }
    );
}

/// The three states a page key can land in, and the one that used to be
/// wrong: a press while a dialog is up is dropped, not passed on raw.
#[test]
fn a_page_key_is_dropped_while_an_overlay_is_up() {
    let chrome = Chrome::default();
    let page = || kobo_ui::Node::Text {
        id: kobo_ui::NodeId(1),
        text: "A page of a book.".to_owned(),
        links: Vec::new(),
    };

    // Declares turns: the press becomes the declared action, exactly as a
    // tap on the side zone would have.
    let declared = Screen::new(1, vec![page()]).with_page_turns(ActionId(11), ActionId(12));
    assert!(matches!(
        page_key_message(&declared, &chrome, true),
        Some(kobo_protocol::Message::Action {
            action: ActionId(12)
        })
    ));
    assert!(matches!(
        page_key_message(&declared, &chrome, false),
        Some(kobo_protocol::Message::Action {
            action: ActionId(11)
        })
    ));

    // Declares nothing: the application may make its own sense of the
    // press, so it hears the raw intent.
    let undeclared = Screen::new(1, vec![page()]);
    assert!(matches!(
        page_key_message(&undeclared, &chrome, true),
        Some(kobo_protocol::Message::PageTurn { forward: true })
    ));
    assert!(matches!(
        page_key_message(&undeclared, &chrome, false),
        Some(kobo_protocol::Message::PageTurn { forward: false })
    ));

    // Covered by a dialog: nothing is sent. Not the declared action, and
    // not the raw intent either -- an application that handled the raw
    // intent would have paged the content underneath the dialog, which is
    // the bug this state exists to make unrepresentable.
    let modal = kobo_ui::Overlay::modal(
        kobo_ui::NodeId(40),
        "Leave?",
        vec![kobo_ui::Node::Button {
            id: kobo_ui::NodeId(41),
            action: ActionId(6),
            label: "Leave".to_owned(),
            state: kobo_ui::ControlState::Enabled,
            emphasis: kobo_ui::Emphasis::Primary,
        }],
    );
    assert_eq!(
        page_key_message(&declared.clone().with_overlay(modal.clone()), &chrome, true),
        None
    );
    // Including when the screen never declared turns: the dialog is what
    // the reader is answering either way.
    assert_eq!(
        page_key_message(&undeclared.with_overlay(modal), &chrome, true),
        None
    );
}

#[test]
fn back_is_reported_from_the_chrome_the_frame_was_drawn_with() {
    let screen = hello();
    let chrome = Chrome::with_back(true);
    let back = screen
        .layout_with(&crate::device_metrics(), &chrome)
        .nodes
        .iter()
        .find(|node| node.kind == kobo_ui::LayoutKind::Back)
        .map(|node| node.rect)
        .expect("a back affordance");
    let hit = action_for(
        TouchEvent::Up {
            x: u32::try_from(back.x + back.width / 2).expect("inside the panel"),
            y: u32::try_from(back.y + back.height / 2).expect("inside the panel"),
        },
        Some(&screen),
        &chrome,
        false,
    );
    assert_eq!(hit, Some(ActionId::BACK));
    // Laid out without the affordance, the same tap must not invent one.
    assert_ne!(
        action_for(
            TouchEvent::Up {
                x: u32::try_from(back.x + back.width / 2).expect("inside the panel"),
                y: u32::try_from(back.y + back.height / 2).expect("inside the panel"),
            },
            Some(&screen),
            &Chrome::default(),
            false,
        ),
        Some(ActionId::BACK)
    );
}

#[test]
fn a_screen_that_asked_for_back_is_given_it_and_one_that_did_not_is_left() {
    // The defect this covers: the runtime swallowed Back entirely, so an
    // application with screens of its own could not return to the one the
    // reader came from. Tapping out of a book dropped them at the launcher
    // and reopening the application showed the book again, because its
    // retained screen had never changed.
    let chrome = Chrome::with_back(true);
    let screen = hello();
    let back = screen
        .layout_with(&crate::device_metrics(), &chrome)
        .nodes
        .iter()
        .find(|node| node.kind == kobo_ui::LayoutKind::Back)
        .map(|node| node.rect)
        .expect("a back affordance");
    let tap = TouchEvent::Up {
        x: u32::try_from(back.x + back.width / 2).expect("inside the panel"),
        y: u32::try_from(back.y + back.height / 2).expect("inside the panel"),
    };

    let (mut runtime, mut app) = std::os::unix::net::UnixStream::pair().expect("a pair of sockets");
    assert_eq!(
        deliver_touch(
            &mut runtime,
            tap,
            Some(&screen),
            &chrome,
            false,
            kobo_ui::Orientation::Portrait,
            kobo_ui::LandscapeTurn::Clockwise,
            kobo_protocol::VERSION,
            kobo_ui::TopBarState::Hidden,
        )
        .expect("route the tap"),
        Tap::Leave,
        "a screen that did not ask keeps the old behaviour"
    );

    let owning = screen.clone().with_own_back(true);
    assert_eq!(
        deliver_touch(
            &mut runtime,
            tap,
            Some(&owning),
            &chrome,
            false,
            kobo_ui::Orientation::Portrait,
            kobo_ui::LandscapeTurn::Clockwise,
            kobo_protocol::VERSION,
            kobo_ui::TopBarState::Hidden,
        )
        .expect("route the tap"),
        Tap::OfferedBack
    );
    let frame = kobo_protocol::read_from(&mut app).expect("the application is told");
    assert!(matches!(
        frame.message,
        Message::Action {
            action: ActionId::BACK
        }
    ));
}

#[test]
fn overlay_close_is_delivered_before_a_root_back_leaves() {
    let chrome = Chrome::with_back(true);
    let root = hello();
    let covered = root.clone().with_overlay(kobo_ui::Overlay::modal(
        kobo_ui::NodeId(2),
        "Settings",
        vec![],
    ));
    let (mut runtime, mut app) = std::os::unix::net::UnixStream::pair().unwrap();
    app.set_read_timeout(Some(Duration::from_millis(20)))
        .unwrap();
    for (screen, kind, expected) in [
        (
            &covered,
            kobo_ui::LayoutKind::OverlayClose,
            Tap::OfferedBack,
        ),
        (&root, kobo_ui::LayoutKind::Back, Tap::Leave),
    ] {
        let layout = screen.layout_with(&crate::device_metrics(), &chrome);
        let rect = layout
            .nodes
            .iter()
            .find(|node| node.kind == kind)
            .unwrap()
            .rect;
        let tap = TouchEvent::Up {
            x: u32::try_from(rect.x + rect.width / 2).unwrap(),
            y: u32::try_from(rect.y + rect.height / 2).unwrap(),
        };
        assert_eq!(
            deliver_touch(
                &mut runtime,
                tap,
                Some(screen),
                &chrome,
                false,
                kobo_ui::Orientation::Portrait,
                kobo_ui::LandscapeTurn::Clockwise,
                kobo_protocol::VERSION,
                kobo_ui::TopBarState::Hidden,
            )
            .unwrap(),
            expected,
        );
        if expected == Tap::OfferedBack {
            assert!(matches!(
                kobo_protocol::read_from(&mut app).unwrap().message,
                Message::Action {
                    action: ActionId::BACK
                }
            ));
        } else {
            assert!(kobo_protocol::read_from(&mut app).is_err());
        }
    }
}

fn catalogue() -> PathBuf {
    let directory = std::env::temp_dir().join(format!("kobo-catalogue-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("make a catalogue");
    std::fs::write(directory.join("kobo-hello"), b"#!/bin/sh\n").expect("install an app");
    directory
}

#[test]
fn an_installed_name_resolves() {
    let directory = catalogue();
    assert_eq!(
        resolve(&directory, "hello").expect("hello is installed"),
        directory.join("kobo-hello")
    );
}

#[test]
fn a_binary_name_is_the_identity_the_handshake_must_use() {
    let path = catalogue().join("kobo-hello");
    assert_eq!(super::installed_name(&path).as_deref(), Ok("hello"));
    for path in ["hello", "kobo-Terminal", "kobo-../terminal", "kobo-"] {
        assert!(super::installed_name(std::path::Path::new(path)).is_err());
    }
}

#[test]
fn a_name_that_is_not_installed_is_refused() {
    assert!(resolve(&catalogue(), "nothing-here").is_err());
}

#[test]
fn a_name_may_not_escape_the_catalogue() {
    // An application that could name a path could start anything on the
    // device, so traversal has to fail on the name and not on the lookup.
    let directory = catalogue();
    for attempt in [
        "../../bin/sh",
        "..",
        "/bin/sh",
        "hello/../../../bin/sh",
        "hello;reboot",
        "hello sh",
        "",
    ] {
        assert!(
            resolve(&directory, attempt).is_err(),
            "{attempt:?} was accepted"
        );
    }
}

#[test]
fn a_name_is_bounded_in_length() {
    assert!(resolve(&catalogue(), &"a".repeat(33)).is_err());
}

/// The update an application receives has to be the one that starts.
///
/// A reader had arXiv left beside the runtime by a package old enough to
/// predate the current wire protocol. The store updated the application,
/// said so, and the reader went on starting the leftover, which the
/// runtime then refused for speaking a protocol it no longer answers. The
/// application was pinned to a binary no update could reach, and the only
/// visible symptom was a screen that said Starting and stayed there.
#[test]
fn the_installed_copy_is_started_even_when_a_binary_sits_beside_the_runtime() {
    let directory = catalogue();
    let installed = directory.join("installed-elsewhere/kobo-hello");
    assert_eq!(
        super::prefer_installed(&directory, "hello", || Ok(installed.clone()))
            .expect("the installed copy answers"),
        installed,
        "the leftover beside the runtime shadowed the installed copy"
    );
}

#[test]
fn a_binary_beside_the_runtime_answers_only_when_nothing_is_installed() {
    let directory = catalogue();
    assert_eq!(
        super::prefer_installed(&directory, "hello", || Err("nothing installed".to_owned()))
            .expect("the binary beside the runtime answers"),
        directory.join("kobo-hello")
    );
    assert!(super::prefer_installed(&directory, "nothing-here", || Err(
        "nothing installed".to_owned()
    ))
    .is_err());
}

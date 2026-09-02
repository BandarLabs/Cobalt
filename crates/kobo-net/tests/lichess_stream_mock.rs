use kobo_net::{
    fetch_from_controlled, post_controlled, trust_owner_root, LineStreamAction, LineStreams,
    RequestOptions,
};
use kobo_protocol::TaskError;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::{ServerConfig, ServerConnection, StreamOwned};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Once};
use std::thread;
use std::time::{Duration, Instant};

const CA_CERTIFICATE: &[u8] = include_bytes!("fixtures/localhost-ca.der");
const CERTIFICATE: &[u8] = include_bytes!("fixtures/localhost-cert.der");
const PRIVATE_KEY: &[u8] = include_bytes!("fixtures/localhost-key.der");

fn trust_fixture() {
    static TRUST: Once = Once::new();
    TRUST.call_once(|| {
        trust_owner_root(CA_CERTIFICATE.to_vec()).expect("install local mock root");
    });
}

fn server_config() -> Arc<ServerConfig> {
    let certificate = CertificateDer::from(CERTIFICATE.to_vec());
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(PRIVATE_KEY.to_vec()));
    Arc::new(
        ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .expect("protocol versions")
            .with_no_client_auth()
            .with_single_cert(vec![certificate], key)
            .expect("mock certificate"),
    )
}

fn accept(
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
    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = stream.read(&mut buffer).expect("read request");
        assert!(read > 0, "client closed before request head");
        request.extend_from_slice(&buffer[..read]);
        assert!(request.len() < 32 * 1024, "oversized test request");
    }
    (stream, request)
}

fn contains_header(request: &[u8], prefix: &str) -> bool {
    std::str::from_utf8(request)
        .is_ok_and(|request| request.split("\r\n").any(|line| line.starts_with(prefix)))
}

fn one_response(response: &'static [u8]) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind mock");
    let address = listener.local_addr().expect("mock address");
    let config = server_config();
    let server = thread::spawn(move || {
        let (mut stream, _) = accept(&listener, config);
        stream.write_all(response).expect("write response");
        stream.flush().expect("flush response");
    });
    (
        format!("https://localhost:{}/api/account", address.port()),
        server,
    )
}

#[test]
fn local_https_ndjson_mock_streams_game_start_without_exposing_controls() {
    trust_fixture();
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind mock");
    let address = listener.local_addr().expect("mock address");
    let config = server_config();
    let (release_tx, release_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        let (mut stream, request) = accept(&listener, config);
        assert!(request.starts_with(b"GET /api/stream/event HTTP/1.1\r\n"));
        assert!(contains_header(&request, "Authorization: Bearer "));
        assert!(contains_header(&request, "Accept: application/x-ndjson"));
        assert!(!contains_header(&request, "X-Cobalt-"));
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nTransfer-Encoding: chunked\r\nConnection: keep-alive\r\n\r\n",
            )
            .expect("response head");
        stream.write_all(b"1\r\n\n\r\n").expect("blank keepalive");
        let event = br#"{"type":"gameStart","game":{"id":"abcdEF12"}}"#;
        write!(stream, "{:x}\r\n", event.len() + 1).expect("chunk size");
        stream.write_all(event).expect("event");
        stream.write_all(b"\n\r\n").expect("event frame");
        stream.flush().expect("flush event");
        let _ = release_rx.recv_timeout(Duration::from_secs(5));
    });

    let streams = LineStreams::default();
    let url = format!("https://localhost:{}/api/stream/event", address.port());
    let headers = [("Accept", "application/x-ndjson")];
    let cancel = AtomicBool::new(false);
    assert_eq!(
        streams.request(
            LineStreamAction::Open,
            &url,
            4096,
            Some(("Authorization", "Bearer mock-only-token")),
            &headers,
            RequestOptions {
                report_rate_limit: true,
                wait_until_cancelled: false,
            },
            &cancel,
        ),
        Ok(Vec::new())
    );
    let record = streams
        .request(
            LineStreamAction::Next,
            &url,
            4096,
            Some(("Authorization", "Bearer mock-only-token")),
            &headers,
            RequestOptions::default(),
            &cancel,
        )
        .expect("next event");
    assert_eq!(record, br#"{"type":"gameStart","game":{"id":"abcdEF12"}}"#);
    assert_eq!(
        streams.request(
            LineStreamAction::Next,
            &url,
            2048,
            Some(("Authorization", "Bearer mock-only-token")),
            &headers,
            RequestOptions::default(),
            &cancel,
        ),
        Err(TaskError::Denied),
        "a later task cannot silently lower the retained record ceiling"
    );
    streams
        .request(
            LineStreamAction::Close,
            &url,
            4096,
            None,
            &headers,
            RequestOptions::default(),
            &cancel,
        )
        .expect("close");
    release_tx.send(()).expect("release server");
    server.join().expect("mock server");
}

#[test]
fn credentialed_stream_redirects_are_denied_without_contacting_the_target() {
    trust_fixture();
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind mock");
    let address = listener.local_addr().expect("mock address");
    let config = server_config();
    let server = thread::spawn(move || {
        let (mut stream, _) = accept(&listener, config);
        stream
            .write_all(
                b"HTTP/1.1 302 Found\r\nLocation: https://attacker.invalid/collect\r\nContent-Length: 0\r\n\r\n",
            )
            .expect("redirect");
        stream.flush().expect("flush redirect");
    });
    let streams = LineStreams::default();
    let url = format!("https://localhost:{}/api/stream/event", address.port());
    let cancel = AtomicBool::new(false);
    assert_eq!(
        streams.request(
            LineStreamAction::Open,
            &url,
            4096,
            Some(("Authorization", "Bearer mock-only-token")),
            &[("Accept", "application/x-ndjson")],
            RequestOptions::default(),
            &cancel,
        ),
        Err(TaskError::Denied)
    );
    server.join().expect("mock server");
}

#[test]
fn long_lived_seek_cancels_promptly_and_is_sent_exactly_once() {
    trust_fixture();
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind mock");
    let address = listener.local_addr().expect("mock address");
    let config = server_config();
    let requests = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&requests);
    let server = thread::spawn(move || {
        let (mut stream, request) = accept(&listener, config);
        observed.fetch_add(1, Ordering::SeqCst);
        assert!(request.starts_with(b"POST /api/board/seek HTTP/1.1\r\n"));
        assert!(!contains_header(&request, "X-Cobalt-"));
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nTransfer-Encoding: chunked\r\n\r\n1\r\n\n\r\n",
            )
            .expect("seek response");
        stream.flush().expect("flush seek response");
        thread::sleep(Duration::from_secs(2));
    });

    let url = format!("https://localhost:{}/api/board/seek", address.port());
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = Arc::clone(&cancel);
    let started = Instant::now();
    let worker = thread::spawn(move || {
        post_controlled(
            &url,
            b"rated=true&time=10&increment=0&variant=standard&color=random",
            "application/x-www-form-urlencoded",
            Some(("Authorization", "Bearer mock-only-token")),
            &[],
            4096,
            RequestOptions {
                report_rate_limit: true,
                wait_until_cancelled: true,
            },
            &worker_cancel,
        )
    });
    thread::sleep(Duration::from_millis(100));
    cancel.store(true, Ordering::SeqCst);
    assert_eq!(
        worker.join().expect("seek worker"),
        Err(TaskError::TimedOut)
    );
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "cancellation waited for the server"
    );
    assert_eq!(requests.load(Ordering::SeqCst), 1);
    server.join().expect("mock server");
}

#[test]
fn local_https_mock_preserves_auth_and_retry_after_errors() {
    trust_fixture();
    for status in [401, 403] {
        let response = if status == 401 {
            &b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n"[..]
        } else {
            &b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n"[..]
        };
        let (url, server) = one_response(response);
        let cancel = AtomicBool::new(false);
        assert_eq!(
            fetch_from_controlled(
                &url,
                0,
                4096,
                Some(("Authorization", "Bearer mock-only-token")),
                &[],
                RequestOptions {
                    report_rate_limit: true,
                    wait_until_cancelled: false,
                },
                &cancel,
            ),
            Err(TaskError::Unauthorized)
        );
        server.join().expect("auth mock");
    }

    let (url, server) = one_response(
        b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 23\r\nContent-Length: 0\r\n\r\n",
    );
    let cancel = AtomicBool::new(false);
    assert_eq!(
        fetch_from_controlled(
            &url,
            0,
            4096,
            Some(("Authorization", "Bearer mock-only-token")),
            &[],
            RequestOptions {
                report_rate_limit: true,
                wait_until_cancelled: false,
            },
            &cancel,
        )
        .expect("rate envelope"),
        b"COBALT-HTTP/1 429\nRetry-After: 23\n\n"
    );
    server.join().expect("rate mock");
}

#[test]
fn truncated_chunked_stream_is_rejected_instead_of_returned_as_an_event() {
    trust_fixture();
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind mock");
    let address = listener.local_addr().expect("mock address");
    let config = server_config();
    let server = thread::spawn(move || {
        let (mut stream, _) = accept(&listener, config);
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nTransfer-Encoding: chunked\r\n\r\n20\r\n{\"type\":\"gameStart\"",
            )
            .expect("truncated response");
        stream.flush().expect("flush truncated response");
    });
    let streams = LineStreams::default();
    let url = format!("https://localhost:{}/api/stream/event", address.port());
    let headers = [("Accept", "application/x-ndjson")];
    let cancel = AtomicBool::new(false);
    streams
        .request(
            LineStreamAction::Open,
            &url,
            4096,
            Some(("Authorization", "Bearer mock-only-token")),
            &headers,
            RequestOptions::default(),
            &cancel,
        )
        .expect("open");
    assert_eq!(
        streams.request(
            LineStreamAction::Next,
            &url,
            4096,
            Some(("Authorization", "Bearer mock-only-token")),
            &headers,
            RequestOptions::default(),
            &cancel,
        ),
        Err(TaskError::Unreachable)
    );
    server.join().expect("truncated mock");
}

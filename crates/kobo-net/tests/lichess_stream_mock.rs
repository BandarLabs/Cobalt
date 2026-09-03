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
            break;
        }
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

fn close_tls(stream: &mut StreamOwned<ServerConnection, TcpStream>) {
    stream.conn.send_close_notify();
    let _ = stream.flush();
}

struct HeldStream {
    url: String,
    release: mpsc::Sender<()>,
    server: thread::JoinHandle<()>,
}

fn held_stream(path: &str) -> HeldStream {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind held stream");
    let address = listener.local_addr().expect("held stream address");
    let config = server_config();
    let expected = format!("GET {path} HTTP/1.1\r\n");
    let (release, wait) = mpsc::channel();
    let server = thread::spawn(move || {
        let (mut stream, request) = accept(&listener, config);
        assert!(request.starts_with(expected.as_bytes()));
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nTransfer-Encoding: chunked\r\nConnection: keep-alive\r\n\r\n1\r\n\n\r\n",
            )
            .expect("held response");
        stream.flush().expect("flush held response");
        let _ = wait.recv_timeout(Duration::from_secs(5));
        close_tls(&mut stream);
    });
    HeldStream {
        url: format!("https://localhost:{}{path}", address.port()),
        release,
        server,
    }
}

fn open_ndjson(streams: &LineStreams, url: &str) -> Result<Vec<u8>, TaskError> {
    streams.request(
        LineStreamAction::Open,
        url,
        4096,
        Some(("Authorization", "******")),
        &[("Accept", "application/x-ndjson")],
        RequestOptions::default(),
        &AtomicBool::new(false),
    )
}

fn assert_reopens_after_clean_end(path: &str, first_response: &'static [u8]) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind reconnect mock");
    let address = listener.local_addr().expect("reconnect address");
    let config = server_config();
    let expected = format!("GET {path} HTTP/1.1\r\n");
    let server = thread::spawn(move || {
        let (mut first, request) = accept(&listener, Arc::clone(&config));
        assert!(request.starts_with(expected.as_bytes()));
        first.write_all(first_response).expect("first response");
        first.flush().expect("flush first response");
        close_tls(&mut first);

        let (mut second, request) = accept(&listener, config);
        assert!(request.starts_with(expected.as_bytes()));
        let event = b"{\"type\":\"reconnected\"}\n";
        write!(
            second,
            "HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            event.len()
        )
        .expect("second response head");
        second.write_all(event).expect("second event");
        second.flush().expect("flush second response");
        close_tls(&mut second);
    });

    let streams = LineStreams::default();
    let companion = held_stream("/api/stream/companion");
    assert_eq!(open_ndjson(&streams, &companion.url), Ok(Vec::new()));
    let url = format!("https://localhost:{}{path}", address.port());
    assert_eq!(open_ndjson(&streams, &url), Ok(Vec::new()));
    let ended = streams.request(
        LineStreamAction::Next,
        &url,
        4096,
        Some(("Authorization", "******")),
        &[("Accept", "application/x-ndjson")],
        RequestOptions::default(),
        &AtomicBool::new(false),
    );
    assert_eq!(ended, Err(TaskError::Unreachable));
    assert!(ended.unwrap_err().worth_retrying());

    assert_eq!(open_ndjson(&streams, &url), Ok(Vec::new()));
    assert_eq!(
        streams.request(
            LineStreamAction::Next,
            &url,
            4096,
            Some(("Authorization", "******")),
            &[("Accept", "application/x-ndjson")],
            RequestOptions::default(),
            &AtomicBool::new(false),
        ),
        Ok(br#"{"type":"reconnected"}"#.to_vec())
    );
    streams.close_all();
    companion.release.send(()).expect("release companion");
    companion.server.join().expect("companion mock");
    server.join().expect("reconnect mock");
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
fn accepted_seek_clean_close_completes_without_a_retryable_failure_or_replay() {
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
        stream
            .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .expect("seek completion");
        stream.flush().expect("flush completion");
        close_tls(&mut stream);
    });

    let url = format!("https://localhost:{}/api/board/seek", address.port());
    assert_eq!(
        post_controlled(
            &url,
            b"rated=true&time=10&increment=0&variant=standard&color=random",
            "application/x-www-form-urlencoded",
            Some(("Authorization", "******")),
            &[],
            4096,
            RequestOptions {
                report_rate_limit: true,
                wait_until_cancelled: true,
            },
            &AtomicBool::new(false),
        ),
        Ok(Vec::new())
    );
    assert_eq!(requests.load(Ordering::SeqCst), 1);
    server.join().expect("seek completion mock");
}

#[test]
fn clean_event_and_game_stream_endings_are_retryable_and_can_reopen() {
    trust_fixture();
    assert_reopens_after_clean_end(
        "/api/stream/event",
        b"HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    );
    assert_reopens_after_clean_end(
        "/api/board/game/stream/abcdEF12",
        b"HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n0\r\n\r\n",
    );
}

#[test]
fn terminal_framing_drains_every_buffered_record_before_releasing_the_stream() {
    trust_fixture();
    let body = b"{\"sequence\":1}\n{\"sequence\":2}\n";
    let mut content_length = Vec::new();
    write!(
        content_length,
        "HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .expect("content-length response head");
    content_length.extend_from_slice(body);

    let mut chunked = Vec::new();
    write!(
        chunked,
        "HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n",
        body.len()
    )
    .expect("chunked response head");
    chunked.extend_from_slice(body);
    chunked.extend_from_slice(b"\r\n0\r\n\r\n");

    for (framing, response) in [("content-length", content_length), ("chunked", chunked)] {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind terminal stream");
        let address = listener.local_addr().expect("terminal stream address");
        let config = server_config();
        let server = thread::spawn(move || {
            let (mut stream, _) = accept(&listener, config);
            stream.write_all(&response).expect("terminal response");
            stream.flush().expect("flush terminal response");
            close_tls(&mut stream);
        });
        let streams = LineStreams::default();
        let url = format!("https://localhost:{}/api/stream/{framing}", address.port());
        let headers = [("Accept", "application/x-ndjson")];
        let cancel = AtomicBool::new(false);

        assert_eq!(open_ndjson(&streams, &url), Ok(Vec::new()), "{framing}");
        for expected in [br#"{"sequence":1}"#, br#"{"sequence":2}"#] {
            assert_eq!(
                streams.request(
                    LineStreamAction::Next,
                    &url,
                    4096,
                    Some(("Authorization", "******")),
                    &headers,
                    RequestOptions::default(),
                    &cancel,
                ),
                Ok(expected.to_vec()),
                "{framing}"
            );
        }
        assert_eq!(
            streams.request(
                LineStreamAction::Next,
                &url,
                4096,
                Some(("Authorization", "******")),
                &headers,
                RequestOptions::default(),
                &cancel,
            ),
            Err(TaskError::Unreachable),
            "{framing}"
        );
        server.join().expect("terminal stream server");
    }
}

#[test]
fn retained_stream_budget_releases_on_error_close_and_shutdown() {
    trust_fixture();
    assert_eq!(kobo_net::MAX_RETAINED_STREAMS, 2);
    let streams = LineStreams::default();

    let (bad_url, bad_server) =
        one_response(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 0\r\n\r\n");
    assert_eq!(open_ndjson(&streams, &bad_url), Err(TaskError::Unreachable));
    bad_server.join().expect("bad stream mock");

    let event = held_stream("/api/stream/event");
    let first_game = held_stream("/api/board/game/stream/abcdEF12");
    assert_eq!(open_ndjson(&streams, &event.url), Ok(Vec::new()));
    assert_eq!(open_ndjson(&streams, &first_game.url), Ok(Vec::new()));
    assert_eq!(
        open_ndjson(
            &streams,
            "https://localhost:1/api/board/game/stream/ijklMN34"
        ),
        Err(TaskError::Denied),
        "a third retained connection reached the network instead of the budget gate"
    );

    streams.close(&first_game.url);
    let second_game = held_stream("/api/board/game/stream/ijklMN34");
    assert_eq!(open_ndjson(&streams, &second_game.url), Ok(Vec::new()));

    streams.close_all();
    let replacement_event = held_stream("/api/stream/event");
    let replacement_game = held_stream("/api/board/game/stream/qrstUV56");
    assert_eq!(
        open_ndjson(&streams, &replacement_event.url),
        Ok(Vec::new())
    );
    assert_eq!(open_ndjson(&streams, &replacement_game.url), Ok(Vec::new()));
    streams.close_all();

    for held in [
        event,
        first_game,
        second_game,
        replacement_event,
        replacement_game,
    ] {
        held.release.send(()).expect("release held stream");
        held.server.join().expect("held stream mock");
    }
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

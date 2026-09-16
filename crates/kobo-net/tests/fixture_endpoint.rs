#![cfg(debug_assertions)]

use kobo_net::{LineStreamAction, LineStreams, RequestOptions};
use kobo_protocol::TaskError;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::{ServerConfig, ServerConnection, StreamOwned};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

#[test]
fn loopback_route_preserves_tls_http_and_streams_and_denies_other_destinations() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    kobo_net::fixture::install(&format!("localhost={}", listener.local_addr().unwrap())).unwrap();
    kobo_net::trust_owner_root(include_bytes!("fixtures/localhost-ca.der").to_vec()).unwrap();
    let config = Arc::new(
        ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![CertificateDer::from(
                    include_bytes!("fixtures/localhost-cert.der").to_vec(),
                )],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
                    include_bytes!("fixtures/localhost-key.der").to_vec(),
                )),
            )
            .unwrap(),
    );
    let server = std::thread::spawn(move || {
        for (method, path, body) in [
            ("GET", "/api/account", "{\"id\":\"fixture\"}"),
            ("POST", "/api/action", "{\"ok\":true}"),
            ("GET", "/api/stream/event", "{\"type\":\"gameStart\"}\n"),
        ] {
            let (socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut stream =
                StreamOwned::new(ServerConnection::new(Arc::clone(&config)).unwrap(), socket);
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
                assert!(request.len() < 4096);
            }
            let request = String::from_utf8(request).unwrap();
            assert!(request.starts_with(&format!("{method} {path} HTTP/1.1\r\n")));
            assert!(request.contains("\r\nHost: localhost\r\n"));
            assert_eq!(stream.conn.server_name(), Some("localhost"));
            if method == "POST" {
                assert!(request.contains("\r\nAuthorization: Bearer fixture-only\r\n"));
                let mut body = [0; 3];
                stream.read_exact(&mut body).unwrap();
                assert_eq!(&body, b"x=1");
            }
            write!(stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            stream.flush().unwrap();
            stream.conn.send_close_notify();
            stream.flush().unwrap();
        }
    });
    assert_eq!(
        kobo_net::fetch("https://localhost/api/account", 1024).unwrap(),
        br#"{"id":"fixture"}"#
    );
    assert_eq!(
        kobo_net::post(
            "https://localhost/api/action",
            b"x=1",
            "application/x-www-form-urlencoded",
            Some(("Authorization", "Bearer fixture-only")),
            &[],
            1024
        )
        .unwrap(),
        br#"{"ok":true}"#
    );
    let streams = LineStreams::default();
    let cancel = AtomicBool::new(false);
    let request = |action| {
        streams.request(
            action,
            "https://localhost/api/stream/event",
            1024,
            None,
            &[("Accept", "application/x-ndjson")],
            RequestOptions::default(),
            &cancel,
        )
    };
    request(LineStreamAction::Open).unwrap();
    assert_eq!(
        request(LineStreamAction::Next).unwrap(),
        br#"{"type":"gameStart"}"#
    );
    request(LineStreamAction::Close).unwrap();
    server.join().unwrap();
    assert_eq!(
        kobo_net::fetch("https://example.org/api/account", 1024),
        Err(TaskError::Denied)
    );
    assert_eq!(
        kobo_net::fetch("https://localhost:8443/api/account", 1024),
        Err(TaskError::Denied)
    );
    assert!(kobo_net::fixture::install("localhost=127.0.0.1:9999").is_err());
}

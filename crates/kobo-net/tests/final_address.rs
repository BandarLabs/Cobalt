#![cfg(debug_assertions)]
//! The address a body came from, once redirects are followed, is the one a
//! caller resolves the body's own relative addresses against.

use kobo_net::RequestOptions;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::{ServerConfig, ServerConnection, StreamOwned};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

#[test]
fn a_redirected_fetch_answers_the_address_the_body_was_served_from() {
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
        for (path, answer) in [
            (
                "/go",
                "HTTP/1.1 302 Found\r\nLocation: /articles/story/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            ),
            (
                "/articles/story/",
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 5\r\nConnection: close\r\n\r\nstory",
            ),
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
            assert!(request.starts_with(&format!("GET {path} HTTP/1.1\r\n")), "{request}");
            stream.write_all(answer.as_bytes()).unwrap();
            stream.flush().unwrap();
            stream.conn.send_close_notify();
            stream.flush().unwrap();
        }
    });
    let cancel = AtomicBool::new(false);
    let fetched = kobo_net::fetch_from_controlled_at(
        "https://localhost/go",
        0,
        1024,
        None,
        &[],
        RequestOptions::default(),
        &cancel,
    )
    .unwrap();
    assert_eq!(fetched.body, b"story");
    assert_eq!(fetched.address, "https://localhost/articles/story/");
    server.join().unwrap();
}

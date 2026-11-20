//! Each rule seen refusing, on a real port, in the bytes a peer would send.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpStream};
use std::time::{Duration, Instant};

use super::super::api::ROUTES;
use super::super::jobs::{Finish, Kind};
use super::super::testing::{make_desk, query_value, quick, Desk, Page, Reply, Running, CLARA_BW};
use super::*;

/// A body every `POST` route can read: a JSON object naming a reader, a
/// browser, an app and an address, so that a request which gets past the
/// rules goes on to do what its route does.
fn body_for(volume: &std::path::Path) -> String {
    serde_json::json!({
        "volume": volume.to_string_lossy(),
        "ids": ["almanac"],
        "id": "almanac",
        "browser": "chrome",
        "url": "https://example.org/article",
        "upload": 1,
    })
    .to_string()
}

/// Whether anything a request could have caused has happened.
fn untouched(desk: &Desk, running: &Running, before: &[String]) -> bool {
    desk.commands().is_empty()
        && desk.ejected.lock().expect("ejected").is_empty()
        && desk.fetched.lock().expect("fetched").is_empty()
        && running.shared.jobs.summaries().is_empty()
        && Desk::listing(&desk.root) == before
}

fn started(name: &str) -> (Running, Desk, std::path::PathBuf) {
    let (machine, desk) = make_desk(name);
    let volume = desk.plug("reader", CLARA_BW, Some("0.3.30"));
    let running = Running::start(machine, Limits::default(), quick());
    (running, desk, volume)
}

fn post(running: &Running, path: &str, headers: &[(&str, &str)], body: &str) -> Reply {
    use std::fmt::Write as _;
    let mut request = format!("POST {path} HTTP/1.1\r\n");
    for (name, value) in headers {
        let _ = write!(request, "{name}: {value}\r\n");
    }
    let _ = write!(request, "Content-Length: {}\r\n\r\n{body}", body.len());
    running.send(&request)
}

#[test]
fn the_server_listens_on_loopback_and_on_no_other_address() {
    let (machine, _desk) = make_desk("loopback");
    let server = Server::bind(machine, Limits::default(), quick()).expect("bind");
    let address = server.listener.local_addr().expect("address");
    assert_eq!(address.ip(), Ipv4Addr::LOCALHOST);
    assert_ne!(address.port(), 0);
    assert_eq!(address.port(), server.shared.session.port);
    // Two servers never share a port, because neither chose one.
    let (machine, _other) = make_desk("loopback-second");
    let second = Server::bind(machine, Limits::default(), quick()).expect("bind");
    assert_ne!(second.shared.session.port, address.port());
}

#[test]
fn a_request_that_does_not_name_this_server_exactly_is_refused_whatever_else_it_carries() {
    let (running, desk, volume) = started("host");
    let page = running.open();
    let before = Desk::listing(&desk.root);
    let port = running.port;
    let everything_else_right = |host: Option<&str>, method: &str, path: &str| {
        let host = host.map_or_else(String::new, |host| format!("Host: {host}\r\n"));
        let body = body_for(&volume);
        running.send(&format!(
            "{method} {path} HTTP/1.1\r\n{host}Cookie: {}\r\nOrigin: {}\r\n\
             X-Kobo-Token: {}\r\nContent-Length: {}\r\n\r\n{body}",
            page.cookie,
            running.origin(),
            page.token,
            body.len()
        ))
    };
    for host in [
        Some("attacker.example".to_owned()),
        Some(format!("attacker.example:{port}")),
        Some(format!("127.0.0.1.attacker.example:{port}")),
        Some(format!("localhost.attacker.example:{port}")),
        Some("127.0.0.1".to_owned()),
        Some("localhost".to_owned()),
        Some(format!("127.0.0.1:{}", port.wrapping_add(1))),
        Some(format!("0.0.0.0:{port}")),
        Some(format!("[::1]:{port}")),
        Some(format!("LOCALHOST:{port}")),
        None,
    ] {
        for (method, path) in [
            ("GET", "/"),
            ("GET", "/api/state"),
            ("GET", "/app.js"),
            ("POST", "/api/reader/install"),
            ("POST", "/api/reader/eject"),
        ] {
            let reply = everything_else_right(host.as_deref(), method, path);
            assert_eq!(reply.status, 403, "{host:?} {method} {path}");
            assert!(!reply.body.contains(&page.token));
        }
    }
    assert!(untouched(&desk, &running, &before));

    for host in [format!("127.0.0.1:{port}"), format!("localhost:{port}")] {
        assert_eq!(
            everything_else_right(Some(&host), "GET", "/api/state").status,
            200,
            "{host}"
        );
    }
}

#[test]
fn a_host_said_twice_is_refused_rather_than_one_of_them_believed() {
    let (running, _desk, _volume) = started("host-twice");
    let page = running.open();
    let reply = running.send(&format!(
        "GET /api/state HTTP/1.1\r\nHost: {}\r\nHost: attacker.example\r\nCookie: {}\r\n\r\n",
        running.host(),
        page.cookie
    ));
    assert_eq!(reply.status, 400);
}

#[test]
fn the_launch_address_buys_a_cookie_once_and_the_browser_is_sent_on_without_the_token() {
    let (machine, _desk) = make_desk("exchange");
    let running = Running::start(machine, Limits::default(), quick());
    let address = running.shared.session.launch_address().expect("address");
    let path = address.strip_prefix(&running.origin()).expect("path");
    let token = path.strip_prefix("/?token=").expect("token");
    let get = |path: &str, cookie: Option<&str>| {
        let cookie = cookie.map_or_else(String::new, |cookie| format!("Cookie: {cookie}\r\n"));
        running.send(&format!(
            "GET {path} HTTP/1.1\r\nHost: {}\r\n{cookie}\r\n",
            running.host()
        ))
    };

    // A wrong token buys nothing and does not spend the right one.
    let wrong = get("/?token=0000", None);
    assert_eq!(wrong.status, 401);
    assert_eq!(wrong.header("set-cookie"), None);
    assert!(wrong.body.contains("Run kobo ui"));

    let first = get(path, None);
    assert_eq!(first.status, 303);
    assert_eq!(first.header("location"), Some("/"));
    let set = first.header("set-cookie").expect("a cookie");
    assert!(set.starts_with("kobo_ui="));
    assert!(set.contains("; HttpOnly"));
    assert!(set.contains("; SameSite=Strict"));
    assert!(set.contains("; Path=/"));
    assert!(!set.contains(token), "the cookie is not the launch token");
    let cookie = set.split(';').next().expect("value");

    // The same address a second time, from a browser that was not given
    // the cookie, opens nothing.
    let second = get(path, None);
    assert_eq!(second.status, 401);
    assert_eq!(second.header("set-cookie"), None);

    // The browser that was given it is sent on to the page, clean, with no
    // second cookie; this is the preload-then-open case.
    let again = get(path, Some(cookie));
    assert_eq!(again.status, 303);
    assert_eq!(again.header("location"), Some("/"));
    assert_eq!(again.header("set-cookie"), None);

    let page = get("/", Some(cookie));
    assert_eq!(page.status, 200);
    assert!(page.body.contains("name=\"kobo-token\""));
    assert!(!page.body.contains(token));
    assert!(!page.body.contains(cookie.trim_start_matches("kobo_ui=")));

    // The token is good on the page's own address only, and only by GET.
    let (machine, _desk) = make_desk("exchange-elsewhere");
    let other = Running::start(machine, Limits::default(), quick());
    let address = other.shared.session.launch_address().expect("address");
    let query = address.split('?').nth(1).expect("query");
    for target in [format!("/api/state?{query}"), format!("/app.js?{query}")] {
        let reply = other.send(&format!(
            "GET {target} HTTP/1.1\r\nHost: {}\r\n\r\n",
            other.host()
        ));
        assert_eq!(reply.status, 401, "{target}");
        assert_eq!(reply.header("set-cookie"), None);
    }
    let posted = other.send(&format!(
        "POST /?{query} HTTP/1.1\r\nHost: {}\r\nContent-Length: 0\r\n\r\n",
        other.host()
    ));
    assert_eq!(posted.status, 401);
    assert!(other.shared.session.launch_address().is_some());
}

#[test]
fn nothing_is_served_without_the_cookie_and_a_stranger_is_not_told_what_exists() {
    let (running, desk, volume) = started("cookie");
    let page = running.open();
    let before = Desk::listing(&desk.root);
    let body = body_for(&volume);
    for route in ROUTES {
        for cookie in [
            None,
            Some("kobo_ui=".to_owned()),
            Some("kobo_ui=0000".to_owned()),
            // The request token is not the cookie.
            Some(format!("kobo_ui={}", page.token)),
            Some(format!(
                "other={}",
                page.cookie.trim_start_matches("kobo_ui=")
            )),
        ] {
            let cookie = cookie.map_or_else(String::new, |cookie| format!("Cookie: {cookie}\r\n"));
            let reply = running.send(&format!(
                "{} {} HTTP/1.1\r\nHost: {}\r\n{cookie}Origin: {}\r\nX-Kobo-Token: {}\r\n\
                 Content-Length: {}\r\n\r\n{body}",
                route.method,
                route.path,
                running.host(),
                running.origin(),
                page.token,
                body.len()
            ));
            assert_eq!(
                reply.status, 401,
                "{} {} {cookie:?}",
                route.method, route.path
            );
            assert!(!reply.body.contains(&page.token));
        }
    }
    // A path that does not exist is answered exactly as one that does.
    let missing = running.send(&format!(
        "GET /api/no-such-thing HTTP/1.1\r\nHost: {}\r\n\r\n",
        running.host()
    ));
    let real = running.send(&format!(
        "GET /api/state HTTP/1.1\r\nHost: {}\r\n\r\n",
        running.host()
    ));
    assert_eq!((missing.status, &missing.body), (real.status, &real.body));
    assert!(untouched(&desk, &running, &before));
    assert_eq!(page.get("/api/no-such-thing").status, 404);
}

#[test]
fn a_request_that_changes_something_must_come_from_this_page_and_carry_its_token() {
    let (running, desk, volume) = started("origin");
    let page = running.open();
    let before = Desk::listing(&desk.root);
    let body = body_for(&volume);
    let host = running.host();
    let origin = running.origin();
    let port = running.port;
    let cookie_value = page.cookie.trim_start_matches("kobo_ui=").to_owned();
    let other_port = format!("http://127.0.0.1:{}", port.wrapping_add(1));
    let https = format!("https://127.0.0.1:{port}");
    let lookalike = format!("http://127.0.0.1:{port}.attacker.example");
    // Each case is everything a real request carries with one thing wrong.
    let cases: Vec<(&str, Vec<(&str, &str)>)> = vec![
        ("no Origin", vec![("X-Kobo-Token", &page.token)]),
        (
            "another site's Origin",
            vec![
                ("Origin", "https://attacker.example"),
                ("X-Kobo-Token", &page.token),
            ],
        ),
        (
            "a null Origin",
            vec![("Origin", "null"), ("X-Kobo-Token", &page.token)],
        ),
        (
            "this address on another port",
            vec![("Origin", &other_port), ("X-Kobo-Token", &page.token)],
        ),
        (
            "this address over https",
            vec![("Origin", &https), ("X-Kobo-Token", &page.token)],
        ),
        (
            "a name that begins with this address",
            vec![("Origin", &lookalike), ("X-Kobo-Token", &page.token)],
        ),
        (
            "a browser that says it is cross-site",
            vec![
                ("Origin", &origin),
                ("Sec-Fetch-Site", "cross-site"),
                ("X-Kobo-Token", &page.token),
            ],
        ),
        (
            "a browser that says it is same-site but not same-origin",
            vec![
                ("Origin", &origin),
                ("Sec-Fetch-Site", "same-site"),
                ("X-Kobo-Token", &page.token),
            ],
        ),
        ("no token", vec![("Origin", &origin)]),
        (
            "a wrong token",
            vec![("Origin", &origin), ("X-Kobo-Token", "0000")],
        ),
        (
            "the cookie's value offered as the token",
            vec![("Origin", &origin), ("X-Kobo-Token", &cookie_value)],
        ),
        (
            "an empty token",
            vec![("Origin", &origin), ("X-Kobo-Token", "")],
        ),
    ];
    for route in ROUTES.iter().filter(|route| route.method == "POST") {
        for (what, extra) in &cases {
            let mut headers = vec![("Host", host.as_str()), ("Cookie", page.cookie.as_str())];
            headers.extend(extra.iter().copied());
            let reply = post(&running, route.path, &headers, &body);
            assert_eq!(reply.status, 403, "{} with {what}", route.path);
        }
    }
    assert!(untouched(&desk, &running, &before));

    // With nothing wrong, the same request is let through to its route.
    let reply = page.post("/api/reader/eject", &body);
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert_eq!(*desk.ejected.lock().expect("ejected"), [volume]);
    // And from the page's other name for the same address.
    let local = format!("localhost:{port}");
    let local_origin = format!("http://localhost:{port}");
    let reply = post(
        &running,
        "/api/reader/eject",
        &[
            ("Host", &local),
            ("Cookie", &page.cookie),
            ("Origin", &local_origin),
            ("X-Kobo-Token", &page.token),
        ],
        &body,
    );
    assert_eq!(reply.status, 200, "{}", reply.body);
}

#[test]
fn asking_never_changes_anything_and_changing_is_never_done_by_asking() {
    let (running, desk, volume) = started("methods");
    let page = running.open();
    let before = Desk::listing(&desk.root);
    let query = format!(
        "?volume={}&id=1&undo=1&name=x.cbz&from=0",
        query_value(&volume.to_string_lossy())
    );

    // Every route that changes something refuses to be asked with GET,
    // query and all, and says which method it takes.
    for route in ROUTES.iter().filter(|route| route.method == "POST") {
        let reply = page.get(&format!("{}{query}", route.path));
        assert_eq!(reply.status, 405, "GET {}", route.path);
        assert_eq!(reply.header("allow"), Some("POST"));
    }
    assert!(untouched(&desk, &running, &before));

    // Every question, asked properly, leaves the computer and the reader
    // exactly as they were.
    for route in ROUTES.iter().filter(|route| route.method == "GET") {
        let reply = page.get(&format!("{}{query}", route.path));
        assert!(
            matches!(reply.status, 200 | 404),
            "GET {} answered {}",
            route.path,
            reply.status
        );
    }
    assert!(untouched(&desk, &running, &before));

    // And a question cannot be turned into a change by posting to it.
    for route in ROUTES.iter().filter(|route| route.method == "GET") {
        let reply = page.post(route.path, "{}");
        assert_eq!(reply.status, 405, "POST {}", route.path);
        assert_eq!(reply.header("allow"), Some("GET"));
    }
    assert!(untouched(&desk, &running, &before));
}

#[test]
fn only_get_and_post_exist_and_no_answer_ever_invites_another_origin() {
    let (running, _desk, volume) = started("cors");
    let page = running.open();
    let mut replies = Vec::new();
    // The preflight a browser sends before another origin's script may
    // post with a custom header. Refusing it is what stops that script.
    replies.push(running.send(&format!(
        "OPTIONS /api/reader/install HTTP/1.1\r\nHost: {}\r\n\
         Origin: https://attacker.example\r\nAccess-Control-Request-Method: POST\r\n\
         Access-Control-Request-Headers: x-kobo-token\r\n\r\n",
        running.host()
    )));
    assert_eq!(replies[0].status, 405);
    for method in ["PUT", "DELETE", "PATCH", "HEAD", "TRACE", "CONNECT"] {
        let reply = running.send(&format!(
            "{method} /api/state HTTP/1.1\r\nHost: {}\r\nCookie: {}\r\n\r\n",
            running.host(),
            page.cookie
        ));
        assert_eq!(reply.status, 405, "{method}");
        replies.push(reply);
    }
    replies.push(page.get("/"));
    replies.push(page.get("/app.css"));
    replies.push(page.get("/api/state"));
    replies.push(page.get("/api/nothing"));
    replies.push(page.get("/api/reader/install"));
    replies.push(page.post("/api/reader/eject", &body_for(&volume)));
    replies.push(running.send(&format!(
        "GET /api/state HTTP/1.1\r\nHost: {}\r\nOrigin: https://attacker.example\r\n\r\n",
        running.host()
    )));
    replies.push(running.send("GET / HTTP/1.1\r\nHost: attacker.example\r\n\r\n"));
    replies.push(running.send("nonsense\r\n\r\n"));
    for reply in &replies {
        assert_ne!(reply.status, 0);
        for (name, _) in &reply.headers {
            assert!(
                !name.starts_with("access-control-"),
                "{} carried {name}",
                reply.status
            );
        }
        assert_eq!(reply.header("cache-control"), Some("no-store"));
        assert_eq!(reply.header("x-frame-options"), Some("DENY"));
        assert_eq!(reply.header("x-content-type-options"), Some("nosniff"));
        let policy = reply.header("content-security-policy").expect("a policy");
        assert!(policy.contains("default-src 'none'"));
        assert!(policy.contains("connect-src 'self'"));
        assert!(policy.contains("frame-ancestors 'none'"));
        assert!(!policy.contains("unsafe"));
        assert!(!policy.contains("http"));
    }
}

#[test]
fn a_request_larger_than_its_bound_is_refused_before_it_is_read() {
    let (machine, desk) = make_desk("bounds");
    let volume = desk.plug("reader", CLARA_BW, None);
    let limits = Limits {
        head: 2048,
        json: 256,
        upload: 4096,
    };
    let running = Running::start(machine, limits, quick());
    let page = running.open();

    // A head that never ends, and one that ends too late.
    let endless = format!(
        "GET /api/state HTTP/1.1\r\nHost: {}\r\nX-Pad: {}",
        running.host(),
        "a".repeat(4096)
    );
    assert_eq!(running.send(&endless).status, 431);
    assert_eq!(running.send(&format!("{endless}\r\n\r\n")).status, 431);

    // A JSON body over its bound is refused from its stated length: the
    // request below never sends the body it claims, and is answered at once
    // instead of being waited on.
    let asked = Instant::now();
    let claimed = running.send(&format!(
        "POST /api/reader/eject HTTP/1.1\r\nHost: {}\r\nCookie: {}\r\nOrigin: {}\r\n\
         X-Kobo-Token: {}\r\nContent-Length: 257\r\n\r\n",
        running.host(),
        page.cookie,
        running.origin(),
        page.token
    ));
    assert_eq!(claimed.status, 413);
    assert!(asked.elapsed() < Duration::from_secs(3));
    let padded = format!(
        "{{\"volume\":\"{}\",\"pad\":\"{}\"}}",
        volume.to_string_lossy(),
        "a".repeat(300)
    );
    assert_eq!(page.post("/api/reader/eject", &padded).status, 413);
    assert!(desk.ejected.lock().expect("ejected").is_empty());

    // A file over the upload bound is refused the same way and leaves
    // nothing behind in the folder files are received into.
    let reply = page.post_bytes("/api/send/file?name=big.cbz", &vec![b'x'; 4097]);
    assert_eq!(reply.status, 413);
    // One at the bound is taken.
    let reply = page.post_bytes("/api/send/file?name=fits.cbz", &vec![b'x'; 4096]);
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert_eq!(reply.json()["bytes"], 4096);
    let received: Vec<String> = Desk::listing(&desk.root.join("scratch"))
        .into_iter()
        .filter(|entry| entry.contains("cbz"))
        .collect();
    assert_eq!(received.len(), 1, "{received:?}");
    assert!(received[0].ends_with("fits.cbz 4096"));

    // A body with no stated length cannot be bounded, so it is not taken.
    let chunked = running.send(&format!(
        "POST /api/reader/eject HTTP/1.1\r\nHost: {}\r\nCookie: {}\r\nOrigin: {}\r\n\
         X-Kobo-Token: {}\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n",
        running.host(),
        page.cookie,
        running.origin(),
        page.token
    ));
    assert_eq!(chunked.status, 411);
    // A body that is not a JSON object is refused as malformed.
    for body in ["", "[]", "\"text\"", "{not json"] {
        assert_eq!(page.post("/api/reader/eject", body).status, 400, "{body:?}");
    }
    assert!(desk.ejected.lock().expect("ejected").is_empty());
    assert!(desk.commands().is_empty());
}

#[test]
fn a_request_refused_by_a_rule_is_answered_without_waiting_for_its_body() {
    let (running, _desk, _volume) = started("unread-body");
    let page = running.open();
    // The read patience is five seconds. A server that tried to read this
    // body, which is promised and never sent, would sit for all of it.
    let asked = Instant::now();
    let reply = running.send(&format!(
        "POST /api/send/file?name=a.cbz HTTP/1.1\r\nHost: {}\r\nCookie: {}\r\n\
         Origin: https://attacker.example\r\nX-Kobo-Token: {}\r\nContent-Length: 400000000\r\n\r\n",
        running.host(),
        page.cookie,
        page.token
    ));
    assert_eq!(reply.status, 403);
    assert!(
        asked.elapsed() < Duration::from_secs(3),
        "{:?}",
        asked.elapsed()
    );
}

#[test]
fn a_peer_that_goes_quiet_mid_request_is_dropped_when_patience_runs_out() {
    let (machine, _desk) = make_desk("quiet");
    let patience = Patience {
        read: Duration::from_millis(300),
        ..quick()
    };
    let running = Running::start(machine, Limits::default(), patience);
    let mut stream = TcpStream::connect(("127.0.0.1", running.port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timeout");
    stream
        .write_all(b"GET /api/state HTTP/1.1\r\nHost: ")
        .expect("half a request");
    let asked = Instant::now();
    let mut answer = String::new();
    let _ = stream.read_to_string(&mut answer);
    assert!(answer.starts_with("HTTP/1.1 408 "), "{answer:?}");
    assert!(asked.elapsed() >= Duration::from_millis(250));
    assert!(asked.elapsed() < Duration::from_secs(5));
}

#[test]
fn connections_past_the_cap_are_turned_away_and_the_page_gets_in_again_after() {
    let (machine, _desk) = make_desk("crowd");
    let running = Running::start(machine, Limits::default(), quick());
    let page = running.open();
    // Connections that say nothing each hold a seat until the read patience
    // ends, which is longer than this test.
    let mut idle = Vec::new();
    for _ in 0..MOST_CONNECTIONS {
        idle.push(TcpStream::connect(("127.0.0.1", running.port)).expect("connect"));
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while running.shared.connections.load(Ordering::SeqCst) < MOST_CONNECTIONS {
        assert!(Instant::now() < deadline, "the seats were never all taken");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(page.get("/api/state").status, 0, "closed without an answer");
    assert_eq!(
        running.shared.connections.load(Ordering::SeqCst),
        MOST_CONNECTIONS
    );
    drop(idle);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if page.get("/api/state").status == 200 {
            break;
        }
        assert!(Instant::now() < deadline, "the seats were never given back");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn brief(linger: u64, idle: u64) -> Patience {
    Patience {
        linger: Duration::from_millis(linger),
        idle: Duration::from_millis(idle),
        ..quick()
    }
}

fn say_goodbye(page: &Page<'_>) {
    assert_eq!(page.post("/api/goodbye", "{}").status, 200);
}

#[test]
fn the_server_stops_a_little_after_the_page_says_it_is_closing() {
    let (machine, _desk) = make_desk("goodbye");
    let running = Running::start(machine, Limits::default(), brief(200, 60_000));
    let page = running.open();
    assert_eq!(page.get("/api/state").status, 200);
    assert_eq!(running.stopped(), None);
    say_goodbye(&page);
    let said = Instant::now();
    assert_eq!(
        running.stops_within(Duration::from_secs(5)),
        Some(Stopped::WindowClosed)
    );
    assert!(
        said.elapsed() >= Duration::from_millis(150),
        "it waited first"
    );
}

#[test]
fn a_page_that_comes_back_within_the_wait_keeps_its_server() {
    let (machine, _desk) = make_desk("reload");
    let running = Running::start(machine, Limits::default(), brief(700, 60_000));
    let page = running.open();
    // A reload: the old page says goodbye, the new one asks for the page.
    say_goodbye(&page);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(page.get("/").status, 200);
    assert_eq!(running.stops_within(Duration::from_millis(1500)), None);
    say_goodbye(&page);
    assert_eq!(
        running.stops_within(Duration::from_secs(5)),
        Some(Stopped::WindowClosed)
    );
}

#[test]
fn a_server_nobody_asks_anything_of_stops_at_the_idle_limit() {
    let (machine, _desk) = make_desk("idle");
    let running = Running::start(machine, Limits::default(), brief(60_000, 400));
    let page = running.open();
    // Asking keeps it alive past the limit.
    for _ in 0..6 {
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(page.get("/api/state").status, 200);
    }
    assert_eq!(running.stopped(), None);
    assert_eq!(
        running.stops_within(Duration::from_secs(5)),
        Some(Stopped::Idle)
    );
}

#[test]
fn a_stranger_cannot_keep_the_server_alive_or_close_it() {
    let (machine, _desk) = make_desk("stranger-presence");
    let running = Running::start(machine, Limits::default(), brief(200, 600));
    let page = running.open();
    let started = Instant::now();
    // Requests without the cookie, one of them a goodbye, arriving the
    // whole time. None counts as the page, so the server neither stops
    // early on the goodbye nor outlives its idle limit.
    let mut stopped = None;
    while started.elapsed() < Duration::from_secs(5) && stopped.is_none() {
        let _ = running.send(&format!(
            "GET /api/state HTTP/1.1\r\nHost: {}\r\n\r\n",
            running.host()
        ));
        let _ = post(
            &running,
            "/api/goodbye",
            &[
                ("Host", &running.host()),
                ("Origin", &running.origin()),
                ("X-Kobo-Token", &page.token),
            ],
            "{}",
        );
        stopped = running.stopped();
    }
    assert_eq!(stopped, Some(Stopped::Idle));
    assert!(
        started.elapsed() >= Duration::from_millis(550),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn the_server_stops_when_the_program_showing_the_page_exits() {
    let (machine, _desk) = make_desk("shell");
    let closed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let asked = std::sync::Arc::clone(&closed);
    let running = Running::start_with(machine, Limits::default(), quick(), move || {
        asked.load(Ordering::SeqCst)
    });
    let page = running.open();
    assert_eq!(page.get("/api/state").status, 200);
    assert_eq!(running.stops_within(Duration::from_millis(400)), None);
    closed.store(true, Ordering::SeqCst);
    assert_eq!(
        running.stops_within(Duration::from_secs(5)),
        Some(Stopped::ShellClosed)
    );
}

#[test]
fn nothing_stops_the_server_while_a_job_is_still_running() {
    let (machine, _desk) = make_desk("job-holds");
    let closed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let asked = std::sync::Arc::clone(&closed);
    let running = Running::start_with(machine, Limits::default(), brief(100, 300), move || {
        asked.load(Ordering::SeqCst)
    });
    let page = running.open();
    let (release, held) = std::sync::mpsc::channel::<()>();
    running
        .shared
        .jobs
        .start(
            Kind::Reader,
            "Installing Cobalt",
            move |_| {
                let _ = held.recv();
                Finish {
                    ok: true,
                    code: None,
                }
            },
            || {},
        )
        .expect("started");
    // The window closes, its program exits, and far longer than both the
    // wait and the idle limit goes by.
    say_goodbye(&page);
    closed.store(true, Ordering::SeqCst);
    assert_eq!(running.stops_within(Duration::from_millis(1200)), None);
    release.send(()).expect("let the job end");
    assert!(running.stops_within(Duration::from_secs(5)).is_some());
}

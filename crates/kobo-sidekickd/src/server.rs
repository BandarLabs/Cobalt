//! The two listeners, and the four routes between them.
//!
//! Loopback, plaintext, for the hooks: `POST /ask` blocks until the person
//! decides or patience runs out. LAN, TLS, for the reader: `GET /pending`
//! long-polls for a question, `POST /answer` delivers the tap, and both
//! demand the pairing code so a stranger on the network can watch nothing
//! and answer for nobody.
//!
//! Every connection is one request on one thread. The traffic is a person
//! pressing buttons; there is nothing here worth an event loop.

use crate::board::{Ask, Asking, Board, Choice, Decision};
use crate::http::{read_request, respond_json, Request};
use crate::state;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// The longest `/pending` will hold the line. The runtime allows a request
/// three minutes; staying well inside means the reader's poll always ends
/// with an answer rather than a timeout it has to explain.
const LONGEST_POLL: Duration = Duration::from_secs(25);

/// The most connections each listener serves at once. Admission is decided
/// before a thread is spawned and before any TLS work, so a flood of
/// connections -- paired or not -- costs the flooder a handshake apiece and
/// this process a bounded number of threads. Hooks legitimately sit in
/// `/ask` for minutes each, so they get the deeper bench.
const MOST_HOOKS: usize = 64;
const MOST_READERS: usize = 16;

/// How long a connection gets to deliver its request. A request here is a
/// few hundred bytes from the same machine or the same room; a sender that
/// needs longer than this is not one worth holding a thread for.
const READ_PATIENCE: Duration = Duration::from_secs(10);

/// How long a response write may sit unaccepted before the thread is taken
/// back.
const WRITE_PATIENCE: Duration = Duration::from_secs(30);

/// Loads the identity and serves until killed.
///
/// # Errors
///
/// Fails at startup for a missing identity or an unbindable port; after
/// that, individual connections fail individually.
pub fn run() -> Result<(), String> {
    let identity = state::load()?;
    let tls = kobo_net::serve::TlsServer::from_pem(&identity.certificate, &identity.key)?;
    let board = Arc::new(Board::new());
    let hooks = TcpListener::bind(("127.0.0.1", state::HOOK_PORT))
        .map_err(|error| format!("bind 127.0.0.1:{}: {error}", state::HOOK_PORT))?;
    let reader = TcpListener::bind(("0.0.0.0", state::READER_PORT))
        .map_err(|error| format!("bind 0.0.0.0:{}: {error}", state::READER_PORT))?;
    println!(
        "sidekick: hooks on 127.0.0.1:{}, reader on 0.0.0.0:{} (TLS)",
        state::HOOK_PORT,
        state::READER_PORT
    );
    let hook_board = Arc::clone(&board);
    std::thread::spawn(move || {
        let crowd = Crowd::new(MOST_HOOKS);
        for stream in hooks.incoming().flatten() {
            // Refusal is closing the connection: the hook errors out, prints
            // nothing, and its agent falls back to the terminal prompt.
            let Some(seat) = crowd.admit() else { continue };
            let board = Arc::clone(&hook_board);
            std::thread::spawn(move || {
                let _seat = seat;
                let mut stream = stream;
                let _ = stream.set_read_timeout(Some(READ_PATIENCE));
                let _ = stream.set_write_timeout(Some(WRITE_PATIENCE));
                if let Ok(request) = read_request(&mut stream) {
                    hook_route(&board, &request, &mut stream);
                }
            });
        }
    });
    let pairing = Arc::new(identity.pairing);
    let tls = Arc::new(tls);
    let crowd = Crowd::new(MOST_READERS);
    for stream in reader.incoming().flatten() {
        let Some(seat) = crowd.admit() else { continue };
        let board = Arc::clone(&board);
        let pairing = Arc::clone(&pairing);
        let tls = Arc::clone(&tls);
        std::thread::spawn(move || {
            let _seat = seat;
            let _ = stream.set_read_timeout(Some(READ_PATIENCE));
            let _ = stream.set_write_timeout(Some(WRITE_PATIENCE));
            if let Ok(mut stream) = tls.accept(stream) {
                if let Ok(request) = read_request(&mut stream) {
                    reader_route(&board, &pairing, &request, &mut stream);
                }
            }
        });
    }
    Ok(())
}

/// Admission to one listener: a count of live connection threads, bounded.
struct Crowd {
    live: Arc<AtomicUsize>,
    most: usize,
}

impl Crowd {
    fn new(most: usize) -> Self {
        Self {
            live: Arc::new(AtomicUsize::new(0)),
            most,
        }
    }

    /// A seat if the room is not full, `None` to say "hang up". The seat
    /// frees itself when dropped, however its connection thread ends.
    fn admit(&self) -> Option<Seat> {
        self.live
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |live| {
                (live < self.most).then_some(live + 1)
            })
            .ok()
            .map(|_| Seat(Arc::clone(&self.live)))
    }
}

struct Seat(Arc<AtomicUsize>);

impl Drop for Seat {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// `POST /ask` from a hook: put the question up, wait for the person.
fn hook_route(board: &Board, request: &Request, stream: &mut (impl Read + Write)) {
    if request.method != "POST" || request.path() != "/ask" {
        respond_json(stream, 404, "Not Found", "{}");
        return;
    }
    let body = String::from_utf8_lossy(&request.body);
    let Ok(ask) = kobo_json::parse(&body) else {
        respond_json(stream, 400, "Bad Request", "{}");
        return;
    };
    let field = |name: &str| {
        ask.get(name)
            .and_then(kobo_json::Value::as_str)
            .unwrap_or("")
            .to_owned()
    };
    let mut asking = Asking::new(&field("source"), &field("tool"), &field("detail"))
        .in_session(&field("session"))
        .offering(read_choices(ask.get("choices")));
    // Absent means a permission, which is what almost every ask is.
    if ask.get("permission").and_then(kobo_json::Value::as_bool) == Some(false) {
        let multi = ask.get("multi").and_then(kobo_json::Value::as_bool) == Some(true);
        asking = asking.multiple_choice(multi);
    }
    let reply = match board.submit(asking, state::ASK_PATIENCE) {
        Decision::Allow => plain("allow"),
        Decision::Deny => plain("deny"),
        Decision::Pass => plain("pass"),
        // The labels go back whole. A hook that offered choices knows what
        // it offered, and nothing in between needs to understand them.
        Decision::Chose(labels) => kobo_json::ObjectBuilder::new()
            .set("decision", "chose")
            .set(
                "labels",
                kobo_json::Value::Array(labels.into_iter().map(kobo_json::Value::String).collect()),
            )
            .build(),
    };
    respond_json(stream, 200, "OK", &reply.to_json());
}

/// A decision with nothing to say beyond its own name.
fn plain(word: &str) -> kobo_json::Value {
    kobo_json::ObjectBuilder::new()
        .set("decision", word)
        .build()
}

/// The choices in an `/ask` body, if it offered any.
fn read_choices(value: Option<&kobo_json::Value>) -> Vec<Choice> {
    let Some(items) = value.and_then(kobo_json::Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let text = |name: &str| {
                item.get(name)
                    .and_then(kobo_json::Value::as_str)
                    .unwrap_or("")
                    .to_owned()
            };
            let label = text("label");
            // A button with nothing written on it cannot be pressed for a
            // reason, so it is dropped rather than drawn blank.
            (!label.is_empty()).then(|| Choice {
                label,
                description: text("description"),
            })
        })
        .collect()
}

/// `GET /pending` and `POST /answer` from the reader, behind the code.
fn reader_route(board: &Board, pairing: &str, request: &Request, stream: &mut (impl Read + Write)) {
    match (request.method.as_str(), request.path()) {
        ("GET", "/pending") => {
            if request.query("token") != Some(pairing) {
                respond_json(stream, 403, "Forbidden", "{}");
                return;
            }
            if request.query("all") == Some("true") {
                let (version, asks) = board.snapshot();
                let reply = kobo_json::ObjectBuilder::new()
                    .set("version", version.to_string())
                    .set(
                        "asks",
                        kobo_json::Value::Array(asks.iter().map(ask_json).collect()),
                    )
                    .build();
                respond_json(stream, 200, "OK", &reply.to_json());
                return;
            }
            let wait = request
                .query("wait")
                .and_then(|value| value.parse::<u64>().ok())
                .map_or(LONGEST_POLL, Duration::from_secs)
                .min(LONGEST_POLL);
            let reply = board.next(wait).map_or_else(
                || "{}".to_owned(),
                |ask| {
                    kobo_json::ObjectBuilder::new()
                        .set("ask", ask_json(&ask))
                        .build()
                        .to_json()
                },
            );
            respond_json(stream, 200, "OK", &reply);
        }
        ("POST", "/answer") => {
            let body = String::from_utf8_lossy(&request.body);
            let Ok(answer) = kobo_json::parse(&body) else {
                respond_json(stream, 400, "Bad Request", "{}");
                return;
            };
            if answer.get("token").and_then(kobo_json::Value::as_str) != Some(pairing) {
                respond_json(stream, 403, "Forbidden", "{}");
                return;
            }
            let id = answer.get("id").and_then(kobo_json::Value::as_i64);
            // Chosen options arrive under their own name, so a question
            // offering a choice called "allow" still means the choice.
            let chosen: Vec<String> = answer
                .get("labels")
                .and_then(kobo_json::Value::as_array)
                .map(<[kobo_json::Value]>::to_vec)
                .unwrap_or_default()
                .iter()
                .filter_map(|label| label.as_str().map(str::to_owned))
                .filter(|label| !label.is_empty())
                .collect();
            let decision = if chosen.is_empty() {
                match answer.get("choice").and_then(kobo_json::Value::as_str) {
                    Some("allow") => Some(Decision::Allow),
                    Some("deny") => Some(Decision::Deny),
                    Some("pass") => Some(Decision::Pass),
                    _ => None,
                }
            } else {
                Some(Decision::Chose(chosen))
            };
            let landed = match (id, decision) {
                (Some(id), Some(decision)) => {
                    u32::try_from(id).is_ok_and(|id| board.answer(id, decision))
                }
                _ => false,
            };
            let reply = kobo_json::ObjectBuilder::new().set("ok", landed).build();
            respond_json(stream, 200, "OK", &reply.to_json());
        }
        _ => respond_json(stream, 404, "Not Found", "{}"),
    }
}

/// The question as the reader sees it.
fn ask_json(ask: &Ask) -> kobo_json::Value {
    let choices = ask
        .choices
        .iter()
        .map(|choice| {
            kobo_json::ObjectBuilder::new()
                .set("label", choice.label.as_str())
                .set("description", choice.description.as_str())
                .build()
        })
        .collect();
    kobo_json::ObjectBuilder::new()
        .set("id", ask.id)
        .set("source", ask.source.as_str())
        .set("session", ask.session.as_str())
        .set("tool", ask.tool.as_str())
        .set("detail", ask.detail.as_str())
        .set("choices", kobo_json::Value::Array(choices))
        .set("permission", ask.permission)
        .set("multi", ask.multi)
        .build()
}

#[cfg(test)]
mod tests {
    use super::{hook_route, reader_route};
    use crate::board::{Board, Decision};
    use crate::http::Request;
    use std::io::Cursor;
    use std::sync::Arc;
    use std::time::Duration;

    fn get(target: &str) -> Request {
        Request {
            method: "GET".to_owned(),
            target: target.to_owned(),
            body: Vec::new(),
        }
    }

    fn post(target: &str, body: &str) -> Request {
        Request {
            method: "POST".to_owned(),
            target: target.to_owned(),
            body: body.as_bytes().to_vec(),
        }
    }

    fn body_of(written: &[u8]) -> String {
        let text = String::from_utf8_lossy(written);
        let (_, body) = text.split_once("\r\n\r\n").expect("a response head");
        body.to_owned()
    }

    /// The id inside a `/pending` body, which no test may guess: ids start
    /// somewhere random precisely so that nothing can.
    fn id_of(pending: &str) -> u32 {
        let body = kobo_json::parse(pending).expect("a JSON body");
        let id = body
            .get("ask")
            .and_then(|ask| ask.get("id"))
            .and_then(kobo_json::Value::as_i64)
            .expect("an ask with an id");
        u32::try_from(id).expect("an id that fits")
    }

    #[test]
    fn a_question_travels_from_hook_to_reader_and_the_tap_travels_back() {
        let board = Arc::new(Board::new());
        let hook_board = Arc::clone(&board);
        let hook = std::thread::spawn(move || {
            let mut wire = Cursor::new(Vec::new());
            hook_route(
                &hook_board,
                &post(
                    "/ask",
                    r#"{"source":"codex","tool":"shell","detail":"cargo test"}"#,
                ),
                &mut wire,
            );
            body_of(&wire.into_inner())
        });
        let mut wire = Cursor::new(Vec::new());
        reader_route(
            &board,
            "code",
            &get("/pending?token=code&wait=5"),
            &mut wire,
        );
        let pending = body_of(&wire.into_inner());
        assert!(pending.contains("\"detail\":\"cargo test\""), "{pending}");
        let id = id_of(&pending);
        let mut wire = Cursor::new(Vec::new());
        reader_route(
            &board,
            "code",
            &post(
                "/answer",
                &format!(r#"{{"token":"code","id":{id},"choice":"allow"}}"#),
            ),
            &mut wire,
        );
        assert!(body_of(&wire.into_inner()).contains("\"ok\":true"));
        assert!(hook
            .join()
            .expect("hook")
            .contains("\"decision\":\"allow\""));
    }

    #[test]
    fn the_wrong_pairing_code_sees_nothing_and_answers_nothing() {
        let board = Board::new();
        let mut wire = Cursor::new(Vec::new());
        reader_route(
            &board,
            "code",
            &get("/pending?token=wrong&wait=0"),
            &mut wire,
        );
        let text = String::from_utf8_lossy(wire.get_ref()).into_owned();
        assert!(text.starts_with("HTTP/1.1 403"), "{text}");
        let mut wire = Cursor::new(Vec::new());
        reader_route(
            &board,
            "code",
            &post("/answer", r#"{"token":"wrong","id":1,"choice":"allow"}"#),
            &mut wire,
        );
        let text = String::from_utf8_lossy(wire.get_ref()).into_owned();
        assert!(text.starts_with("HTTP/1.1 403"), "{text}");
    }

    #[test]
    fn an_empty_board_long_polls_into_an_empty_object() {
        let board = Board::new();
        let mut wire = Cursor::new(Vec::new());
        reader_route(
            &board,
            "code",
            &get("/pending?token=code&wait=0"),
            &mut wire,
        );
        assert_eq!(body_of(&wire.into_inner()), "{}");
    }

    #[test]
    fn all_pending_is_a_versioned_snapshot_with_session_identity() {
        let board = Arc::new(Board::new());
        let hook_board = Arc::clone(&board);
        let hook = std::thread::spawn(move || {
            let mut wire = Cursor::new(Vec::new());
            hook_route(
                &hook_board,
                &post(
                    "/ask",
                    r#"{"source":"claude","session":"cobalt · ab12","tool":"Bash","detail":"cargo test"}"#,
                ),
                &mut wire,
            );
        });
        while board.snapshot().1.is_empty() {}
        let mut wire = Cursor::new(Vec::new());
        reader_route(&board, "code", &get("/pending?token=code&all=true"), &mut wire);
        let reply = body_of(&wire.into_inner());
        assert!(reply.contains("\"asks\"") && reply.contains("cobalt · ab12"), "{reply}");
        let ask = board.snapshot().1.pop().expect("pending ask");
        assert!(board.answer(ask.id, Decision::Pass));
        hook.join().expect("hook");
    }

    #[test]
    fn answering_a_question_that_already_left_reports_ok_false() {
        let board = Board::new();
        let mut wire = Cursor::new(Vec::new());
        reader_route(
            &board,
            "code",
            &post("/answer", r#"{"token":"code","id":9,"choice":"deny"}"#),
            &mut wire,
        );
        assert!(body_of(&wire.into_inner()).contains("\"ok\":false"));
    }

    #[test]
    fn an_ignored_question_lets_the_terminal_prompt_have_it() {
        let board = Arc::new(Board::new());
        let hook_board = Arc::clone(&board);
        let hook = std::thread::spawn(move || {
            let mut wire = Cursor::new(Vec::new());
            hook_route(
                &hook_board,
                &post("/ask", r#"{"source":"claude","tool":"Bash","detail":"ls"}"#),
                &mut wire,
            );
            body_of(&wire.into_inner())
        });
        while board.next(Duration::from_millis(10)).is_none() {}
        let ask = board.next(Duration::from_millis(10)).expect("the question");
        let mut wire = Cursor::new(Vec::new());
        reader_route(
            &board,
            "code",
            &post(
                "/answer",
                &format!(r#"{{"token":"code","id":{},"choice":"pass"}}"#, ask.id),
            ),
            &mut wire,
        );
        assert!(hook.join().expect("hook").contains("\"decision\":\"pass\""));
    }

    #[test]
    fn a_full_room_turns_connections_away_until_a_seat_frees() {
        let crowd = super::Crowd::new(2);
        let first = crowd.admit().expect("an empty room has a seat");
        let second = crowd.admit().expect("a second seat");
        assert!(crowd.admit().is_none(), "a full room admitted a third");
        drop(first);
        let third = crowd.admit().expect("a freed seat can be taken");
        drop(second);
        drop(third);
    }
}

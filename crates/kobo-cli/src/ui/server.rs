//! The listener, the order the rules are applied in, and when to stop.
//!
//! Every connection is one request on one thread. The traffic is one person
//! clicking in one window, with a status poll every couple of seconds behind
//! them; there is nothing here worth an event loop.
//!
//! The order in [`Shared::answer`] is the point of this file. A request is
//! refused at the first rule it fails, the rules that cost nothing come
//! first, and nothing a request asks for is read, looked up or run until
//! every rule that applies to it has passed. In particular the body is never
//! touched before then, so a stranger costs this process one bounded head.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::api::{self, Machine};
use super::guard::{self, Exchange, Session};
use super::http::{Head, Malformed, Response};
use super::jobs::Jobs;

/// How much a request may make this process read.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// The request line and headers. A browser's own headers for this page
    /// come to about a kilobyte; sixteen leaves room for any browser's and
    /// still ends a peer that sends headers without end.
    pub head: usize,
    /// A JSON body. The largest the page sends is a list of app ids, a few
    /// hundred bytes, so this bounds a mistake rather than a use.
    pub json: u64,
    /// A file to send to the reader. A whole comic volume is a few hundred
    /// megabytes and nothing an owner sends is larger, so past this a
    /// request is a mistake that would only fill the temporary folder. The
    /// bytes go to disk as they arrive and are never held in memory.
    pub upload: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            head: 16 * 1024,
            json: 64 * 1024,
            upload: 512 * 1024 * 1024,
        }
    }
}

/// How long this server waits for things.
#[derive(Clone, Copy, Debug)]
pub struct Patience {
    /// One read from a connection. The peer is a browser on this computer,
    /// so a read that takes this long is a connection nobody is behind, and
    /// the thread is taken back.
    pub read: Duration,
    /// One write to a connection, for the same reason.
    pub write: Duration,
    /// How long after the page says it is closing the server waits before
    /// believing it. Reloading a page closes it and opens it again within a
    /// second; this covers a slow reload without leaving a server behind a
    /// closed window for long enough to matter.
    pub linger: Duration,
    /// How long with no request at all before the server gives up on a
    /// window that never said it was closing, because the browser was
    /// killed or the computer slept. Browsers slow a hidden tab's timers to
    /// about one a minute, so this has to be several times that or a tab
    /// left in the background would lose its server.
    pub idle: Duration,
}

impl Default for Patience {
    fn default() -> Self {
        Self {
            read: Duration::from_secs(15),
            write: Duration::from_secs(30),
            linger: Duration::from_secs(5),
            idle: Duration::from_secs(600),
        }
    }
}

/// The most connections served at once.
///
/// The page holds one or two; an upload holds one for as long as the file
/// takes. Admission is decided before a thread is spawned, so a local
/// program that opens connections in a loop costs itself the sockets and
/// this process a fixed number of threads.
const MOST_CONNECTIONS: usize = 32;

/// How often the reasons to stop are looked at.
const TICK: Duration = Duration::from_millis(100);

/// Why the server stopped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stopped {
    /// The page said it was closing and did not come back.
    WindowClosed,
    /// The program showing the page exited.
    ShellClosed,
    /// Nothing asked for anything for [`Patience::idle`].
    Idle,
}

/// When the page was last heard from.
#[derive(Debug)]
struct Presence {
    last: Instant,
    /// When the page last said it was closing, unless it has asked for
    /// something since.
    goodbye: Option<Instant>,
}

/// Everything a request is answered from.
pub struct Shared {
    pub session: Session,
    pub limits: Limits,
    pub patience: Patience,
    pub jobs: Arc<Jobs>,
    pub machine: Machine,
    presence: Mutex<Presence>,
    connections: AtomicUsize,
    /// Set when a program this server can watch is showing the companion.
    tied: AtomicBool,
}

/// A request that has passed every rule, and the connection its body is
/// still waiting on.
pub struct Request<'a> {
    pub head: Head,
    stream: &'a mut dyn Read,
    limits: Limits,
}

impl Request<'_> {
    /// Reads the body as a JSON object.
    ///
    /// # Errors
    ///
    /// The response to send instead, when the body is too large, cut short,
    /// or not an object.
    pub fn json(&mut self) -> Result<Value, Response> {
        let body = self
            .head
            .body(&mut self.stream, self.limits.json)
            .map_err(Response::malformed)?;
        serde_json::from_slice::<Value>(&body)
            .ok()
            .filter(Value::is_object)
            .ok_or_else(|| Response::malformed(Malformed::Bad))
    }

    /// Copies the body to `sink`, up to the upload limit.
    ///
    /// # Errors
    ///
    /// Why the body was refused or cut short, for the caller to answer once
    /// it has put away whatever was already written.
    pub fn save(&mut self, sink: &mut impl Write) -> Result<u64, Malformed> {
        self.head
            .body_into(&mut self.stream, self.limits.upload, sink)
    }
}

/// What answers a route.
pub type Handler = fn(&Arc<Shared>, &mut Request<'_>) -> Response;

/// One thing the page can ask for.
pub struct Route {
    /// `GET` for a question, `POST` for anything that changes something on
    /// the reader, on this computer or in this server. There is no third
    /// kind, and the method is the whole of how the two are told apart.
    pub method: &'static str,
    pub path: &'static str,
    pub handle: Handler,
}

impl Shared {
    /// Applies every rule to a request and answers it.
    fn answer(self: &Arc<Self>, head: Head, stream: &mut dyn Read) -> Response {
        let port = self.session.port;
        // First, and for everything: a request that did not name this
        // server by its own address was sent here by a name somebody else
        // controls.
        if !guard::host_is_ours(head.header("host"), port) {
            return Response::refusal(
                403,
                "Forbidden",
                "session",
                "This server answers only to its own address on this computer.",
            );
        }
        let method = head.method.clone();
        let method = method.as_str();
        // `OPTIONS` lands here too. It is how a browser asks whether another
        // origin may send a request, and the answer is that none may.
        if method != "GET" && method != "POST" {
            return not_allowed("GET, POST");
        }
        let path = head.path().to_owned();
        // The native app's way in: its own token on every request, in place
        // of the cookie and the page's token. Decided before anything a
        // browser could have sent is looked at, and a request that offers a
        // token here is judged on that alone, so a wrong one is refused even
        // if a good cookie rides along with it.
        if let Some(authorization) = head.header("authorization") {
            if !self.session.holds_app_token(Some(authorization)) {
                return stranger(&path);
            }
            // The app is not a page and sends no `Origin`. A request that
            // carries both the app's token and an origin was made by a
            // browser, where that token has no business being, so it is
            // refused however right the token is.
            if head.header("origin").is_some() {
                return not_this_page();
            }
            return self.route(method, &path, head, stream);
        }
        let known = self.session.holds_cookie(head.cookie(guard::COOKIE));
        if method == "GET" && path == "/" {
            if let Some(token) = head.query("token") {
                // A browser that preloads an address as it is typed spends
                // the token before the owner presses Return. The cookie it
                // was given is the proof that this is the same browser, so
                // it is let through to the page rather than turned away
                // with a token that was good a moment ago.
                if known {
                    self.heard(false);
                    return to_the_page();
                }
                return match self.session.exchange(&token) {
                    Exchange::Granted => {
                        self.heard(false);
                        to_the_page().with("Set-Cookie", self.session.set_cookie())
                    }
                    Exchange::Refused => stranger(&path),
                };
            }
        }
        if !known {
            return stranger(&path);
        }
        if method == "POST" {
            // Sent by every current browser and impossible for a page to
            // forge. `Sec-Fetch-Site` is checked only when present, because
            // its absence is an older browser, not an attack; `Origin` is
            // the rule.
            if !guard::origin_is_ours(head.header("origin"), port)
                || head
                    .header("sec-fetch-site")
                    .is_some_and(|site| site != "same-origin")
            {
                return not_this_page();
            }
            if !self.session.holds_token(head.header(guard::TOKEN_HEADER)) {
                return Response::refusal(
                    403,
                    "Forbidden",
                    "session",
                    "This request did not carry this session's token. Reload the page.",
                );
            }
        }
        self.route(method, &path, head, stream)
    }

    /// Hands a request that has passed every rule to what it asked for.
    fn route(
        self: &Arc<Self>,
        method: &str,
        path: &str,
        head: Head,
        stream: &mut dyn Read,
    ) -> Response {
        let Some(route) = api::ROUTES.iter().find(|route| route.path == path) else {
            return Response::refusal(404, "Not Found", "not_found", "Not Found");
        };
        if route.method != method {
            return not_allowed(route.method);
        }
        self.heard(path == api::GOODBYE);
        (route.handle)(
            self,
            &mut Request {
                head,
                stream,
                limits: self.limits,
            },
        )
    }

    /// Says that the companion is being shown by a program whose ending
    /// this server will be told of, so that it stops when that program
    /// does and for no other reason.
    pub fn tie(&self) {
        self.tied.store(true, Ordering::SeqCst);
    }

    /// Notes that the page asked for something, or that it said goodbye.
    fn heard(&self, goodbye: bool) {
        if let Ok(mut presence) = self.presence.lock() {
            let now = Instant::now();
            presence.last = now;
            presence.goodbye = goodbye.then_some(now);
        }
    }

    /// Whether it is time to stop, and why.
    fn finished(&self, shell_closed: bool) -> Option<Stopped> {
        let mut presence = self.presence.lock().ok()?;
        // A job is never abandoned. Most are a child whose output this
        // process is reading, and a child whose reader goes away dies at its
        // next line, which for an install is in the middle of the write.
        // While one runs the idle clock is held at now, so the window that
        // started it still gets the full wait once it ends.
        if self.jobs.any_running() {
            presence.last = Instant::now();
            return None;
        }
        if shell_closed {
            return Some(Stopped::ShellClosed);
        }
        // With a program to watch, its ending is the only reason to stop.
        // Silence means nothing then: the system slows an app that is
        // hidden or minimised until it asks for almost nothing, and a
        // server that took that for an empty room would be gone when the
        // owner came back to the window.
        if self.tied.load(Ordering::SeqCst) {
            return None;
        }
        if presence
            .goodbye
            .is_some_and(|at| at.elapsed() >= self.patience.linger)
        {
            return Some(Stopped::WindowClosed);
        }
        (presence.last.elapsed() >= self.patience.idle).then_some(Stopped::Idle)
    }
}

fn not_allowed(allowed: &str) -> Response {
    Response::refusal(
        405,
        "Method Not Allowed",
        "bad_request",
        "Method Not Allowed",
    )
    .with("Allow", allowed.to_owned())
}

fn not_this_page() -> Response {
    Response::refusal(
        403,
        "Forbidden",
        "session",
        "This request did not come from this page.",
    )
}

/// Sends the browser to the page's plain address.
///
/// This is what takes the launch token out of the address bar, the history
/// and anything the page might later be asked to share. `303` so that the
/// follow-up is a `GET` whatever came before.
fn to_the_page() -> Response {
    Response {
        status: 303,
        reason: "See Other",
        ..Response::file("text/plain; charset=utf-8", b"")
    }
    .with("Location", "/".to_owned())
}

/// The answer to anybody who does not hold the session.
///
/// A person can land here honestly, by opening an address from an earlier
/// run or in a second browser, so the page's own address gets a sentence
/// that says what to do. Everything else gets the bare refusal, and neither
/// says whether the path asked for exists.
fn stranger(path: &str) -> Response {
    if path == "/" {
        Response {
            status: 401,
            reason: "Unauthorized",
            ..Response::file(
                "text/plain; charset=utf-8",
                b"This address opened Cobalt once and cannot open it again.\n\n\
                  Run kobo ui in a terminal to open a new window.\n",
            )
        }
    } else {
        Response::refusal(401, "Unauthorized", "session", "Unauthorized")
    }
}

/// A bound server that has not started answering yet.
pub struct Server {
    listener: TcpListener,
    pub shared: Arc<Shared>,
}

impl Server {
    /// Binds a port on loopback and draws a session for it.
    ///
    /// The address is `127.0.0.1` and never anything wider: not `0.0.0.0`,
    /// which would offer the page to the whole network, and not `localhost`,
    /// which is a name and can be made to mean something else. The port is
    /// whichever the system has free, so two windows never collide and no
    /// page elsewhere can know in advance where to aim.
    ///
    /// # Errors
    ///
    /// When no port can be bound or no random bytes can be drawn.
    pub fn bind(machine: Machine, limits: Limits, patience: Patience) -> Result<Self, String> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .map_err(|error| format!("could not listen on this computer: {error}"))?;
        let port = listener
            .local_addr()
            .map_err(|error| format!("could not read the port that was bound: {error}"))?
            .port();
        Ok(Self {
            listener,
            shared: Arc::new(Shared {
                session: Session::new(port)?,
                limits,
                patience,
                jobs: Arc::new(Jobs::default()),
                machine,
                presence: Mutex::new(Presence {
                    last: Instant::now(),
                    goodbye: None,
                }),
                connections: AtomicUsize::new(0),
                tied: AtomicBool::new(false),
            }),
        })
    }

    /// Answers requests until there is a reason to stop, and says which.
    ///
    /// `shell_closed` is asked on every tick whether the program showing the
    /// page has exited. A browser is not this server's child and never
    /// answers yes; for those the page's own goodbye and the idle limit are
    /// the reasons.
    pub fn serve(self, mut shell_closed: impl FnMut() -> bool) -> Stopped {
        let Self { listener, shared } = self;
        let answering = Arc::clone(&shared);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let Some(seat) = Seat::take(&answering) else {
                    // Over the cap, the connection is closed unanswered. The
                    // page's next poll finds a free seat; a flood finds the
                    // same closed door every time.
                    continue;
                };
                let shared = Arc::clone(&answering);
                std::thread::spawn(move || {
                    let _seat = seat;
                    converse(stream, &shared);
                });
            }
        });
        let tick = TICK
            .min(shared.patience.linger / 4)
            .max(Duration::from_millis(1));
        loop {
            std::thread::sleep(tick);
            if let Some(why) = shared.finished(shell_closed()) {
                return why;
            }
        }
    }
}

/// One admitted connection, given back when it is dropped.
struct Seat(Arc<Shared>);

impl Seat {
    fn take(shared: &Arc<Shared>) -> Option<Self> {
        let taken = shared.connections.fetch_add(1, Ordering::SeqCst);
        if taken >= MOST_CONNECTIONS {
            shared.connections.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(Self(Arc::clone(shared)))
    }
}

impl Drop for Seat {
    fn drop(&mut self) {
        self.0.connections.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Reads one request, answers it, and closes.
fn converse(mut stream: TcpStream, shared: &Arc<Shared>) {
    let _ = stream.set_read_timeout(Some(shared.patience.read));
    let _ = stream.set_write_timeout(Some(shared.patience.write));
    let response = match Head::read(&mut stream, shared.limits.head) {
        Ok(head) => shared.answer(head, &mut stream),
        Err(why) => Response::malformed(why),
    };
    response.write(&mut stream);
    // A refusal is written while the peer may still be sending the body it
    // was refused for. Closing with that unread makes the system reset the
    // connection, and a browser shows a reset as a network failure instead
    // of the answer it was just sent. So the sending side is closed first
    // and what is still arriving is let drain for a moment, bounded in both
    // time and bytes, since a refused peer is owed no more than that.
    let _ = stream.shutdown(Shutdown::Write);
    let _ = stream.set_read_timeout(Some(Duration::from_millis(250)));
    let mut spare = [0_u8; 8192];
    let mut drained = 0;
    while drained < 256 * 1024 {
        match stream.read(&mut spare) {
            Ok(0) | Err(_) => break,
            Ok(read) => drained += read,
        }
    }
}

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;

#![forbid(unsafe_code)]

//! Localhost-only browser simulator for Kobo grayscale screens.

use std::fs;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;

use kobo_policy::{store::Store, DeviceServices, TaskRunner};
use kobo_protocol::{read_from, write_to, Frame, Message};
use kobo_ui::{render, ActionId, Node, NodeId, Screen, Surface, DISPLAY_HEIGHT, DISPLAY_WIDTH};

const MAX_HTTP_HEADER: usize = 8 * 1024;
const PROTOCOL_WIDTH: u16 = 1072;
const PROTOCOL_HEIGHT: u16 = 1448;
const PROTOCOL_PPI: u16 = 300;

/// A deterministic interactive counter used to exercise rendering and hit testing.
#[derive(Debug)]
pub struct Simulator {
    counter: u32,
    screen: Screen,
}

impl Default for Simulator {
    fn default() -> Self {
        Self::new()
    }
}

impl Simulator {
    #[must_use]
    pub fn new() -> Self {
        let mut simulator = Self {
            counter: 0,
            screen: Screen::new(1, Vec::new()),
        };
        simulator.rebuild_screen();
        simulator
    }

    #[must_use]
    pub const fn counter(&self) -> u32 {
        self.counter
    }

    #[must_use]
    pub fn screen(&self) -> &Screen {
        &self.screen
    }

    #[must_use]
    pub fn frame(&self) -> Vec<u8> {
        let mut surface = Surface::new(DISPLAY_WIDTH as usize, DISPLAY_HEIGHT as usize);
        render(&self.screen, &mut surface, None);
        surface.pixels
    }

    pub fn touch(&mut self, x: i32, y: i32) -> Option<ActionId> {
        let action = self.screen.hit_test(x, y)?;
        if action == ActionId(1) {
            self.counter = self.counter.saturating_add(1);
            self.rebuild_screen();
        }
        Some(action)
    }

    fn rebuild_screen(&mut self) {
        self.screen = Screen::new(
            1,
            vec![
                Node::Heading {
                    id: NodeId(1),
                    text: "Counter".into(),
                },
                Node::Text {
                    id: NodeId(2),
                    text: format!("Value: {}", self.counter),
                },
                Node::Button {
                    id: NodeId(3),
                    action: ActionId(1),
                    label: "Increment".into(),
                },
            ],
        );
    }
}

/// A localhost listener and its in-memory simulator state.
#[derive(Debug)]
pub struct Server {
    listener: TcpListener,
    simulator: Simulator,
}

impl Server {
    /// # Errors
    ///
    /// Returns an error if the loopback listener cannot be bound.
    pub fn bind_localhost(port: u16) -> io::Result<Self> {
        Self::bind_address(&format!("127.0.0.1:{port}"))
    }

    /// Binds only an IPv4 loopback address. Hostnames other than `localhost`
    /// and all non-loopback addresses are rejected before binding.
    ///
    /// # Errors
    ///
    /// Returns an error for a non-loopback address, invalid port, or bind failure.
    pub fn bind_address(address: &str) -> io::Result<Self> {
        install_typeface();
        let listener = TcpListener::bind(parse_local_address(address)?)?;
        Ok(Self {
            listener,
            simulator: Simulator::new(),
        })
    }

    /// # Errors
    ///
    /// Returns an error if the listener address cannot be queried.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// Serves one request, useful when an embedding event loop owns the listener.
    ///
    /// # Errors
    ///
    /// Returns an error if accepting, reading, or writing the request fails.
    pub fn serve_one(&mut self) -> io::Result<()> {
        let (stream, _) = self.listener.accept()?;
        self.handle(stream)
    }

    /// Serves requests indefinitely. The listener is bound only to IPv4 localhost.
    ///
    /// # Errors
    ///
    /// Returns an error if accepting, reading, or writing a request fails.
    pub fn serve(&mut self) -> io::Result<()> {
        loop {
            self.serve_one()?;
        }
    }

    fn handle(&mut self, mut stream: TcpStream) -> io::Result<()> {
        let request = read_request(&mut stream)?;
        match (request.method.as_str(), request.path.as_str()) {
            ("GET", "/") => write_response(
                &mut stream,
                200,
                "text/html; charset=utf-8",
                SHELL.as_bytes(),
            ),
            ("GET", "/frame") => {
                let frame = self.simulator.frame();
                write_response(&mut stream, 200, "application/octet-stream", &frame)
            }
            ("GET", "/diagnostics") => {
                let body =
                    diagnostics_json(self.simulator.screen(), &kobo_ui::PictureCache::default());
                write_response(
                    &mut stream,
                    200,
                    "application/json; charset=utf-8",
                    body.as_bytes(),
                )
            }
            ("POST", "/touch") => {
                if let Some((x, y)) = parse_touch(&request.body) {
                    self.simulator.touch(x, y);
                    write_response(&mut stream, 204, "text/plain; charset=utf-8", b"")
                } else {
                    write_response(
                        &mut stream,
                        400,
                        "text/plain; charset=utf-8",
                        b"invalid touch",
                    )
                }
            }
            _ => write_response(&mut stream, 404, "text/plain; charset=utf-8", b"not found"),
        }
    }
}

/// Browser simulator host for a real Kobo SDK application.
///
/// The HTTP shell is always bound to IPv4 loopback. The SDK process connects
/// over the caller-selected Unix socket and owns the screen state.
#[derive(Debug)]
pub struct AppServer {
    http: TcpListener,
    app: UnixListener,
    socket_path: PathBuf,
    socket_identity: (u64, u64),
}

impl AppServer {
    /// Creates a localhost HTTP listener and a new Unix socket listener.
    ///
    /// # Errors
    ///
    /// Returns an error for a non-loopback HTTP address, an existing socket
    /// path, an unsafe socket parent, or a listener binding failure.
    pub fn bind(address: &str, socket_path: impl AsRef<Path>) -> io::Result<Self> {
        install_typeface();
        let socket_path = socket_path.as_ref().to_path_buf();
        validate_socket_parent(&socket_path)?;
        match fs::symlink_metadata(&socket_path) {
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "refusing to replace an existing SDK socket",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let http = TcpListener::bind(parse_local_address(address)?)?;
        let app = UnixListener::bind(&socket_path)?;
        let metadata = match fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))
            .and_then(|()| fs::symlink_metadata(&socket_path))
        {
            Ok(metadata) => metadata,
            Err(error) => {
                drop(app);
                let _ = fs::remove_file(&socket_path);
                return Err(error);
            }
        };
        Ok(Self {
            http,
            app,
            socket_path,
            socket_identity: (metadata.dev(), metadata.ino()),
        })
    }

    /// Returns the validated loopback HTTP address.
    ///
    /// # Errors
    ///
    /// Returns an error if the listener address cannot be queried.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.http.local_addr()
    }

    /// Configures both listeners for polling by an embedding event loop.
    ///
    /// # Errors
    ///
    /// Returns an error if either listener cannot change its blocking mode.
    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.http.set_nonblocking(nonblocking)?;
        self.app.set_nonblocking(nonblocking)
    }

    /// Waits for the one SDK connection, validates its Hello, and starts its
    /// protocol reader.
    ///
    /// # Errors
    ///
    /// Returns an error when accepting, handshaking, or creating the protocol
    /// reader fails.
    pub fn accept_app(&self) -> io::Result<AppSession> {
        let (mut stream, _) = self.app.accept()?;
        Self::start_session(&mut stream)
    }

    /// Accepts a pending SDK connection without blocking.
    ///
    /// Call [`Self::set_nonblocking`] with `true` first. Returns `None` when no
    /// SDK connection is currently pending.
    ///
    /// # Errors
    ///
    /// Returns an error when accepting, handshaking, or creating the protocol
    /// reader fails.
    pub fn try_accept_app(&self) -> io::Result<Option<AppSession>> {
        match self.app.accept() {
            Ok((mut stream, _)) => {
                stream.set_nonblocking(false)?;
                Self::start_session(&mut stream).map(Some)
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn start_session(stream: &mut UnixStream) -> io::Result<AppSession> {
        let hello = read_protocol_frame(stream)?;
        if !matches!(hello.message, Message::Hello { .. }) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "SDK must send Hello before other messages",
            ));
        }
        write_protocol_frame(
            stream,
            &Frame {
                request_id: hello.request_id,
                message: Message::Welcome {
                    width: PROTOCOL_WIDTH,
                    height: PROTOCOL_HEIGHT,
                    pixels_per_inch: PROTOCOL_PPI,
                    text_scale: kobo_ui::display_metrics_from_env().text_scale,
                },
            },
        )?;
        let reader = stream.try_clone()?;
        let state = Arc::new(Mutex::new(AppState::default()));
        let reader_state = Arc::clone(&state);
        // One writer for the whole session, shared by every thread that has
        // something to say to the application: taps from the browser, replies
        // to requests, and terminal output arriving on its own. Frames are
        // length-prefixed, so two of them written at once do not make two
        // frames, they make one unreadable stream.
        let writer = Arc::new(Mutex::new(stream.try_clone()?));
        let reader_writer = Arc::clone(&writer);
        thread::spawn(move || {
            // A malformed frame ends the session rather than being skipped,
            // and the developer is told which one: a reader that dies quietly
            // leaves an application talking to nobody, with a panel that keeps
            // showing the last good screen and ignores every tap.
            if let Err(error) = read_app_messages(reader, &reader_writer, &reader_state) {
                eprintln!("the application's connection ended: {error}");
            }
        });
        Ok(AppSession { state, writer })
    }

    /// Accepts the SDK app and serves browser requests until an I/O error.
    ///
    /// # Errors
    ///
    /// Returns an error when accepting the app or a browser request fails.
    pub fn serve(&self) -> io::Result<()> {
        let session = self.accept_app()?;
        loop {
            self.serve_one(&session)?;
        }
    }

    /// Serves one browser HTTP request after an SDK app has connected.
    ///
    /// # Errors
    ///
    /// Returns an error when accepting, reading, or writing the request fails.
    pub fn serve_one(&self, session: &AppSession) -> io::Result<()> {
        let (stream, _) = self.http.accept()?;
        session.handle_http(stream)
    }

    /// Serves one pending browser request without blocking.
    ///
    /// Call [`Self::set_nonblocking`] with `true` first. Returns `false` when
    /// no browser request is currently pending.
    ///
    /// # Errors
    ///
    /// Returns an error when accepting, reading, or writing the request fails.
    pub fn try_serve_one(&self, session: &AppSession) -> io::Result<bool> {
        match self.http.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false)?;
                session.handle_http(stream)?;
                Ok(true)
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(false),
            Err(error) => Err(error),
        }
    }
}

impl Drop for AppServer {
    fn drop(&mut self) {
        if fs::symlink_metadata(&self.socket_path)
            .is_ok_and(|metadata| (metadata.dev(), metadata.ino()) == self.socket_identity)
        {
            let _ = fs::remove_file(&self.socket_path);
        }
    }
}

fn validate_socket_parent(socket_path: &Path) -> io::Result<()> {
    let parent = socket_path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "SDK socket path must have a parent directory",
        )
    })?;
    let metadata = fs::symlink_metadata(parent)?;
    if !metadata.file_type().is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "SDK socket parent must be a directory",
        ));
    }
    if metadata.uid() != current_user_id()? {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "SDK socket parent must be owned by the current user",
        ));
    }
    if metadata.mode() & 0o7777 != 0o700 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "SDK socket parent must have mode 0700",
        ));
    }
    Ok(())
}

fn current_user_id() -> io::Result<u32> {
    let output = Command::new("/usr/bin/id").arg("-u").output()?;
    if !output.status.success() {
        return Err(io::Error::other("could not determine current user ID"));
    }
    std::str::from_utf8(&output.stdout)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "current user ID is not UTF-8"))?
        .trim()
        .parse()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "current user ID is invalid"))
}

/// Keeps or releases one picture on the application's behalf.
///
/// Split out only so the message loop stays readable; the cache is the same
/// one the device runtime uses.
fn hold(state: &Arc<Mutex<AppState>>, message: Message) -> io::Result<()> {
    let diagnostic = {
        let mut held = state
            .lock()
            .map_err(|_| io::Error::other("app state lock poisoned"))?;
        match message {
            Message::PutPicture {
                handle,
                width,
                height,
                grey,
            } => picture_result(
                handle,
                held.pictures.put_report(handle, width, height, grey),
            ),
            Message::BeginPicture {
                handle,
                width,
                height,
            } => (!held.pictures.begin_upload(handle, width, height))
                .then(|| format!("picture {} upload refused", handle.0)),
            Message::PictureChunk {
                handle,
                offset,
                grey,
            } => (!held.pictures.upload_chunk(
                handle,
                usize::try_from(offset).unwrap_or(usize::MAX),
                &grey,
            ))
            .then(|| format!("picture {} chunk refused", handle.0)),
            Message::CommitPicture { handle } => {
                let result = held.pictures.commit_upload(handle);
                picture_result(handle, result)
            }
            Message::DropPicture { handle } => {
                held.pictures.remove(handle);
                None
            }
            _ => None,
        }
    };
    match diagnostic {
        Some(message) => note(state, &message),
        None => Ok(()),
    }
}

fn picture_result(
    handle: kobo_ui::PictureHandle,
    result: Option<Vec<kobo_ui::PictureHandle>>,
) -> Option<String> {
    match result {
        None => Some(format!("picture {} refused", handle.0)),
        Some(evicted) if !evicted.is_empty() => Some(format!(
            "picture {} stored; evicted {}",
            handle.0,
            evicted
                .iter()
                .map(|picture| picture.0.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )),
        Some(_) => None,
    }
}

fn is_picture_message(message: &Message) -> bool {
    matches!(
        message,
        Message::PutPicture { .. }
            | Message::BeginPicture { .. }
            | Message::PictureChunk { .. }
            | Message::CommitPicture { .. }
            | Message::DropPicture { .. }
    )
}

#[derive(Debug)]
struct AppState {
    screen: Screen,
    logs: Vec<String>,
    /// The same bounded cache the device runtime uses, so a preview that shows
    /// a cover and a panel that does not would be a real difference rather than
    /// a simulator shortcut.
    pictures: kobo_ui::PictureCache,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            screen: Screen::new(0, Vec::new()),
            logs: Vec::new(),
            pictures: kobo_ui::PictureCache::default(),
        }
    }
}

/// Connected SDK app state and the serialized action writer.
#[derive(Clone, Debug)]
pub struct AppSession {
    state: Arc<Mutex<AppState>>,
    writer: Arc<Mutex<UnixStream>>,
}

impl AppSession {
    /// Returns the most recently received SDK screen.
    #[must_use]
    pub fn screen(&self) -> Screen {
        self.state.lock().map_or_else(
            |poisoned| poisoned.into_inner().screen.clone(),
            |state| state.screen.clone(),
        )
    }

    /// Sends an action to the SDK app. Writes are serialized with a mutex so
    /// complete protocol frames cannot interleave.
    ///
    /// # Errors
    ///
    /// Returns an error if the SDK connection is closed or its writer is poisoned.
    pub fn send_action(&self, action: ActionId) -> io::Result<()> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| io::Error::other("SDK writer lock poisoned"))?;
        write_protocol_frame(
            &mut writer,
            &Frame {
                request_id: 0,
                message: Message::Action { action },
            },
        )
    }

    fn handle_http(&self, mut stream: TcpStream) -> io::Result<()> {
        let request = read_request(&mut stream)?;
        match (request.method.as_str(), request.path.as_str()) {
            ("GET", "/") => write_response(
                &mut stream,
                200,
                "text/html; charset=utf-8",
                SHELL.as_bytes(),
            ),
            ("GET", "/frame") => {
                let mut surface = Surface::new(DISPLAY_WIDTH as usize, DISPLAY_HEIGHT as usize);
                {
                    let state = self
                        .state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    kobo_ui::render_all(
                        &state.screen,
                        &kobo_ui::display_metrics_from_env(),
                        kobo_ui::Chrome::default(),
                        &state.pictures,
                        &mut surface,
                        None,
                    );
                }
                write_response(
                    &mut stream,
                    200,
                    "application/octet-stream",
                    &surface.pixels,
                )
            }
            ("GET", "/diagnostics") => {
                let body = {
                    let state = self
                        .state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    diagnostics_json(&state.screen, &state.pictures)
                };
                write_response(
                    &mut stream,
                    200,
                    "application/json; charset=utf-8",
                    body.as_bytes(),
                )
            }
            ("POST", "/touch") => {
                let response = parse_touch(&request.body)
                    .and_then(|(x, y)| self.screen().hit_test(x, y))
                    .map_or(Ok(()), |action| self.send_action(action));
                match response {
                    Ok(()) => write_response(&mut stream, 204, "text/plain; charset=utf-8", b""),
                    Err(_) => write_response(
                        &mut stream,
                        503,
                        "text/plain; charset=utf-8",
                        b"SDK unavailable",
                    ),
                }
            }
            _ => write_response(&mut stream, 404, "text/plain; charset=utf-8", b"not found"),
        }
    }
}

fn diagnostics_json(screen: &Screen, pictures: &kobo_ui::PictureCache) -> String {
    let diagnostics = screen.diagnostics_with_pictures(
        &kobo_ui::display_metrics_from_env(),
        kobo_ui::Chrome::default(),
        pictures,
    );
    let mut json = String::from("{\"issues\":[");
    for (index, issue) in diagnostics.issues.iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        let severity = match issue.severity {
            kobo_ui::DiagnosticSeverity::Warning => "warning",
            kobo_ui::DiagnosticSeverity::Error => "error",
        };
        let node = issue
            .node
            .map_or_else(|| "null".to_owned(), |node| node.0.to_string());
        let rect = issue.rect.map_or_else(
            || "null".to_owned(),
            |rect| {
                format!(
                    "{{\"x\":{},\"y\":{},\"width\":{},\"height\":{}}}",
                    rect.x, rect.y, rect.width, rect.height
                )
            },
        );
        let _ = std::fmt::Write::write_fmt(
            &mut json,
            format_args!(
                "{{\"severity\":\"{severity}\",\"node\":{node},\"message\":{},\"rect\":{rect}}}",
                json_string(&issue.to_string())
            ),
        );
    }
    json.push_str("]}");
    json
}

fn json_string(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len() + 2);
    encoded.push('"');
    for character in value.chars() {
        match character {
            '"' => encoded.push_str("\\\""),
            '\\' => encoded.push_str("\\\\"),
            '\n' => encoded.push_str("\\n"),
            '\r' => encoded.push_str("\\r"),
            '\t' => encoded.push_str("\\t"),
            character if character.is_control() => {
                let _ = std::fmt::Write::write_fmt(
                    &mut encoded,
                    format_args!("\\u{:04x}", character as u32),
                );
            }
            character => encoded.push(character),
        }
    }
    encoded.push('"');
    encoded
}

/// Records one line in the simulator's log, keeping only the recent tail.
fn note(state: &Arc<Mutex<AppState>>, line: &str) -> io::Result<()> {
    let mut state = state
        .lock()
        .map_err(|_| io::Error::other("app state lock poisoned"))?;
    state.logs.push(line.to_owned());
    if state.logs.len() > 64 {
        state.logs.remove(0);
    }
    Ok(())
}

fn answer_store(
    writer: &Arc<Mutex<UnixStream>>,
    request_id: u32,
    store: &Store,
    request: &kobo_protocol::StoreRequest,
    state: &Arc<Mutex<AppState>>,
) -> io::Result<()> {
    let result = store.handle(request);
    note(state, &format!("store: {request:?} -> {result:?}"))?;
    write_shared(
        writer,
        &Frame {
            request_id,
            message: Message::StoreResult(result),
        },
    )
}

/// Writes one frame while holding the shared write lock.
///
/// Two threads write to this socket: the message loop, and the terminal drain
/// that has to deliver output nobody asked for. A frame is length-prefixed, so
/// two interleaved writes do not produce two frames, they produce one
/// unreadable stream.
fn write_shared(writer: &Arc<Mutex<UnixStream>>, frame: &Frame) -> io::Result<()> {
    let mut stream = writer
        .lock()
        .map_err(|_| io::Error::other("simulator write lock poisoned"))?;
    write_protocol_frame(&mut stream, frame)
}

/// Delivers terminal output as it arrives, rather than when the next message
/// happens to come in.
///
/// Without this the simulator would only show what a program printed after the
/// developer pressed another key, so anything that prints on its own would look
/// like it had hung.
fn drain_shell(
    shells: &Arc<Mutex<kobo_shell::Shells>>,
    writer: &Arc<Mutex<UnixStream>>,
) -> io::Result<()> {
    loop {
        let events = {
            let Ok(mut shells) = shells.lock() else {
                return Ok(());
            };
            shells.drain()
        };
        for event in events {
            write_shared(
                writer,
                &Frame {
                    request_id: 0,
                    message: Message::ShellEvent(event),
                },
            )?;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// Applies one terminal request and reports anything it has to say.
///
/// A refusal is always written back, because an application that asked for a
/// shell and heard nothing cannot tell a denial from a slow start.
fn answer_shell(
    writer: &Arc<Mutex<UnixStream>>,
    request_id: u32,
    shells: &Arc<Mutex<kobo_shell::Shells>>,
    request: kobo_protocol::ShellRequest,
) -> io::Result<()> {
    let answer = shells
        .lock()
        .map_err(|_| io::Error::other("simulator shell lock poisoned"))?
        .handle(request);
    if let Some(event) = answer {
        write_shared(
            writer,
            &Frame {
                request_id,
                message: Message::ShellEvent(event),
            },
        )?;
    }
    Ok(())
}

/// A terminal on the developer's own machine, running their own shell.
///
/// The point of the simulator is that an application behaves the same here as
/// on the panel, and an application that could not open a terminal in
/// development would have to be tested on the device to be tested at all.
fn simulated_shells(writer: &Arc<Mutex<UnixStream>>) -> Arc<Mutex<kobo_shell::Shells>> {
    let shells = Arc::new(Mutex::new(kobo_shell::Shells::new(&[
        kobo_policy::Capability::Shell,
    ])));
    let draining = Arc::clone(&shells);
    let writer = Arc::clone(writer);
    std::thread::spawn(move || drain_shell(&draining, &writer));
    shells
}

fn read_app_messages(
    mut stream: UnixStream,
    writer: &Arc<Mutex<UnixStream>>,
    state: &Arc<Mutex<AppState>>,
) -> io::Result<()> {
    // The simulator owns no hardware, so it answers state queries from a
    // believable model and refuses everything that would change a real device.
    let mut services = DeviceServices::simulated();
    let tasks = Arc::new(Mutex::new(simulated_tasks()));
    // Drained on its own thread for the same reason terminal output is. The
    // message loop below blocks on the application's socket, so an outcome
    // that arrived while nothing was being typed used to sit in the channel
    // until the developer happened to tap something. A refusal is instant and
    // was therefore delivered immediately, which is exactly why the gap
    // survived: the only tasks the simulator ever completed were the ones it
    // refused.
    {
        let draining = Arc::clone(&tasks);
        let writer = Arc::clone(writer);
        let state = Arc::clone(state);
        std::thread::spawn(move || drain_tasks(&draining, &writer, &state));
    }
    // Kept outside the process so state survives a reload, which is the whole
    // point of a store: a developer restarting the application should see what
    // the owner would see after closing and reopening it.
    let store = Store::new(std::env::temp_dir().join("cobalt-sim-state"));
    let shells = simulated_shells(writer);
    loop {
        let frame = read_protocol_frame(&mut stream)?;
        let request_id = frame.request_id;
        match frame.message {
            Message::SetScreen(screen) => {
                let mut state = state
                    .lock()
                    .map_err(|_| io::Error::other("app state lock poisoned"))?;
                state.screen = screen;
            }
            // The simulator hosts exactly one application, so a launch is
            // reported rather than performed. Pretending it worked would hide
            // the handover from the developer, which is the interesting part.
            Message::Launch { name } => {
                let mut state = state
                    .lock()
                    .map_err(|_| io::Error::other("app state lock poisoned"))?;
                state.logs.push(format!(
                    "Info: asked to launch {name}; the simulator hosts one application"
                ));
            }
            message if is_picture_message(&message) => hold(state, message)?,
            Message::Log { level, message } => note(state, &format!("{level:?}: {message}"))?,
            Message::DeviceRequest(request) => {
                let result = services.handle(request);
                {
                    let mut state = state
                        .lock()
                        .map_err(|_| io::Error::other("app state lock poisoned"))?;
                    state
                        .logs
                        .push(format!("device: {request:?} -> {result:?}"));
                    if state.logs.len() > 64 {
                        state.logs.remove(0);
                    }
                }
                write_shared(
                    writer,
                    &Frame {
                        request_id,
                        message: Message::DeviceResult(result),
                    },
                )?;
            }
            Message::Spawn { task, work } => {
                let rejected = tasks
                    .lock()
                    .map_err(|_| io::Error::other("simulator task lock poisoned"))?
                    .submit(task, work)
                    .err();
                if let Some(reason) = rejected {
                    let mut state = state
                        .lock()
                        .map_err(|_| io::Error::other("app state lock poisoned"))?;
                    state
                        .logs
                        .push(format!("task {} refused: {reason:?}", task.0));
                }
            }
            Message::StoreRequest(request) => {
                answer_store(writer, request_id, &store, &request, state)?;
            }
            Message::ShellRequest(request) => {
                answer_shell(writer, request_id, &shells, request)?;
            }
            Message::Cancel { task } => tasks
                .lock()
                .map_err(|_| io::Error::other("simulator task lock poisoned"))?
                .cancel(task),
            Message::Exit => {
                tasks
                    .lock()
                    .map_err(|_| io::Error::other("simulator task lock poisoned"))?
                    .shutdown();
                return Ok(());
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "unexpected SDK protocol message",
                ));
            }
        }
        deliver_task_outcomes(&tasks, writer, state)?;
    }
}

/// Delivers a finished task as soon as it finishes.
///
/// A fetch takes seconds, and nothing arrives from the application while it is
/// waiting for one, so without this thread the answer would only reach the
/// screen when the developer next tapped something.
fn drain_tasks(
    tasks: &Arc<Mutex<TaskRunner>>,
    writer: &Arc<Mutex<UnixStream>>,
    state: &Arc<Mutex<AppState>>,
) -> io::Result<()> {
    loop {
        deliver_task_outcomes(tasks, writer, state)?;
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// Reports every task that finished, to the log and to the application.
fn deliver_task_outcomes(
    tasks: &Arc<Mutex<TaskRunner>>,
    writer: &Arc<Mutex<UnixStream>>,
    state: &Arc<Mutex<AppState>>,
) -> io::Result<()> {
    let finished = tasks
        .lock()
        .map_err(|_| io::Error::other("simulator task lock poisoned"))?
        .drain();
    for finished in finished {
        {
            let mut state = state
                .lock()
                .map_err(|_| io::Error::other("app state lock poisoned"))?;
            state.logs.push(format!(
                "task {} -> {:?}",
                finished.task.0, finished.outcome
            ));
            if state.logs.len() > 64 {
                state.logs.remove(0);
            }
        }
        write_shared(
            writer,
            &Frame {
                request_id: 0,
                message: Message::TaskOutcome {
                    task: finished.task,
                    outcome: finished.outcome,
                },
            },
        )?;
    }
    Ok(())
}

fn read_protocol_frame(stream: &mut UnixStream) -> io::Result<Frame> {
    read_from(stream).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn write_protocol_frame(stream: &mut UnixStream, frame: &Frame) -> io::Result<()> {
    write_to(stream, frame).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// Starts the counter simulator at a requested loopback address or port.
///
/// # Errors
///
/// Returns an error if the localhost listener cannot be bound or served.
pub fn run_server(address: &str) -> io::Result<()> {
    run_server_at(address)
}

/// Starts the counter simulator at a requested IPv4 loopback address or port.
///
/// Accepted forms are a decimal port (`"3000"`), `"127.0.0.1:3000"`, and
/// `"localhost:3000"`. All other addresses are rejected.
///
/// # Errors
///
/// Returns an error for an invalid/non-loopback address or listener failure.
pub fn run_server_at(address: &str) -> io::Result<()> {
    Server::bind_address(address)?.serve()
}

/// Where the simulator looks for the named credentials an application asks
/// for, mirroring the device directory without ever handing one to the
/// application.
const SIM_SECRETS: &str = "cobalt-sim-secrets";

/// Set this to any value to make every network task fail.
///
/// Failure handling is code, and code nobody has run does not work. This is
/// how a developer runs it deliberately, instead of the simulator refusing
/// everything all the time and teaching nothing.
pub const OFFLINE: &str = "KOBO_SIM_OFFLINE";

/// The task runner the browser simulator gives an application.
///
/// It performs real requests, for the same reason the simulator runs a real
/// shell: an application that could only reach the network on the device could
/// only be developed on the device, which is the one thing this project is
/// arranged to avoid. Network is granted here as the placeholder for a
/// manifest, exactly as the device runtime grants it, so the two cannot drift.
fn simulated_tasks() -> TaskRunner {
    let runner = TaskRunner::simulated(std::env::temp_dir())
        .with_secrets(std::env::temp_dir().join(SIM_SECRETS));
    if std::env::var_os(OFFLINE).is_some() {
        return runner;
    }
    runner
        .with_fetch(Arc::new(kobo_net::fetch_from))
        .with_post(Arc::new(kobo_net::post))
        .with_capabilities([kobo_policy::Capability::Network])
}

/// Gives the simulator the same type the panel gets.
///
/// This was missing, and it was not cosmetic. Without it every preview was
/// drawn in the built-in bitmap fallback, which is uppercase-only and fixed
/// width, so line breaks, wrapping, page counts and the height of every block
/// of text in the browser were nothing like the device's. The whole claim that
/// a screen which fits in the simulator fits on the panel rested on this call.
///
/// A failure is not fatal: `kobo-ui` keeps its bitmap, so the worst case is a
/// preview that looks like the old one.
fn install_typeface() {
    let _ = kobo_text::install(kobo_ui::display_metrics_from_env());
}

fn parse_local_address(address: &str) -> io::Result<SocketAddr> {
    if let Ok(port) = address.parse::<u16>() {
        return Ok(SocketAddr::from(([127, 0, 0, 1], port)));
    }
    let (host, port) = address
        .rsplit_once(':')
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "expected localhost address"))?;
    if !matches!(host, "127.0.0.1" | "localhost") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "simulator may only bind 127.0.0.1 or localhost",
        ));
    }
    let port = port
        .parse::<u16>()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid localhost port"))?;
    Ok(SocketAddr::from(([127, 0, 0, 1], port)))
}

#[derive(Debug, Eq, PartialEq)]
struct HttpRequest {
    method: String,
    path: String,
    body: Vec<u8>,
}

fn read_request(stream: &mut TcpStream) -> io::Result<HttpRequest> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 1024];
    let header_end = loop {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "request ended early",
            ));
        }
        bytes.extend_from_slice(&buffer[..read]);
        if bytes.len() > MAX_HTTP_HEADER {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "HTTP header too large",
            ));
        }
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let header = std::str::from_utf8(&bytes[..header_end])
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "non-UTF-8 header"))?;
    let content_length = content_length(header)?;
    if content_length > MAX_HTTP_HEADER {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "HTTP body too large",
        ));
    }
    while bytes.len() < header_end + content_length {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "body ended early",
            ));
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
    parse_request(&bytes[..header_end + content_length])
        .map_err(|message| io::Error::new(io::ErrorKind::InvalidData, message))
}

fn content_length(header: &str) -> io::Result<usize> {
    for line in header.lines().skip(1) {
        if let Some((name, value)) = line.split_once(':') {
            if !name.eq_ignore_ascii_case("Content-Length") {
                continue;
            }
            return value
                .trim()
                .parse()
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad Content-Length"));
        }
    }
    Ok(0)
}

fn parse_request(bytes: &[u8]) -> Result<HttpRequest, &'static str> {
    let split = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or("missing HTTP header terminator")?;
    let header = std::str::from_utf8(&bytes[..split]).map_err(|_| "non-UTF-8 HTTP header")?;
    let mut lines = header.lines();
    let request_line = lines.next().ok_or("missing request line")?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().ok_or("missing method")?;
    let path = parts.next().ok_or("missing path")?;
    let version = parts.next().ok_or("missing version")?;
    if parts.next().is_some() || !version.starts_with("HTTP/") {
        return Err("invalid request line");
    }
    if !matches!(method, "GET" | "POST") || !path.starts_with('/') || path.contains('?') {
        return Err("unsupported request");
    }
    Ok(HttpRequest {
        method: method.to_owned(),
        path: path.to_owned(),
        body: bytes[split + 4..].to_vec(),
    })
}

fn parse_touch(body: &[u8]) -> Option<(i32, i32)> {
    let body = std::str::from_utf8(body).ok()?;
    let mut x = None;
    let mut y = None;
    for part in body.split('&') {
        let (key, value) = part.split_once('=')?;
        match key {
            "x" => x = value.parse().ok(),
            "y" => y = value.parse().ok(),
            _ => return None,
        }
    }
    Some((x?, y?))
}

fn write_response(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
) -> io::Result<()> {
    let phrase = match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        404 => "Not Found",
        _ => "Internal Server Error",
    };
    write!(
        stream,
        "HTTP/1.1 {status} {phrase}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)
}

const SHELL: &str = r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Kobo simulator</title>
<style>
:root { color-scheme:dark; --workspace:#171717; --panel:#242424; --border:#5f5f5f; --text:#f5f5f5; --muted:#c7c7c7; --paper:#f8f8f8; --paper-ink:#111; --focus:#fff; --space:16px; }
* { box-sizing:border-box; }
body { margin:0; min-height:100vh; background:var(--workspace); color:var(--text); font:16px/1.5 system-ui,sans-serif; }
button,canvas { font:inherit; }
.toolbar { min-height:64px; display:flex; align-items:center; gap:var(--space); padding:12px max(var(--space), calc((100vw - 1440px)/2)); border-bottom:1px solid var(--border); background:var(--panel); }
.toolbar h1 { margin:0; font-size:1rem; letter-spacing:.02em; }
.toolbar p { margin:0 auto 0 0; color:var(--muted); font-size:.875rem; }
.toolbar button { min-height:44px; padding:0 16px; border:1px solid var(--text); background:var(--text); color:var(--workspace); font-weight:700; cursor:pointer; }
button:focus-visible,canvas:focus-visible { outline:3px solid var(--focus); outline-offset:3px; }
main { max-width:1440px; margin:auto; padding:clamp(16px, 3vw, 40px); }
.workspace { display:grid; grid-template-columns:minmax(0, 1fr) 240px; gap:24px; align-items:start; }
.device { margin:0; overflow:auto; padding:16px; border:1px solid var(--border); background:var(--panel); }
.device canvas { display:block; width:min(100%, 1072px); height:auto; margin:auto; background:var(--paper); image-rendering:pixelated; touch-action:manipulation; }
figcaption { margin-top:12px; color:var(--muted); font-size:.875rem; }
.status-panel { padding:16px; border:1px solid var(--border); background:var(--panel); }
.status-panel h2 { margin:0 0 8px; font-size:1rem; }
.status { min-height:1.5em; margin:0; color:var(--muted); }
.diagnostic-toggle { display:flex; align-items:center; gap:8px; min-height:44px; margin-top:12px; }
.diagnostics { max-height:52vh; overflow:auto; margin:8px 0 0; padding-left:20px; color:var(--muted); font-size:.8125rem; }
.diagnostics li + li { margin-top:8px; }
.diagnostics .error { color:#ffb4ab; }
.diagnostics .warning { color:#ffd18b; }
.key { margin-top:16px; color:var(--muted); font-size:.875rem; }
@media (max-width:760px) { .toolbar { align-items:flex-start; flex-wrap:wrap; } .toolbar p { order:3; width:100%; } .workspace { grid-template-columns:1fr; } .device { padding:8px; } }
@media (prefers-contrast:more) { :root { --workspace:#000; --panel:#000; --border:#fff; --text:#fff; --muted:#fff; --paper:#fff; --paper-ink:#000; } }
</style>
</head>
<body>
<header class="toolbar">
  <h1>Kobo simulator</h1>
  <p>Local grayscale display inspector</p>
  <button type="button" id="refresh">Refresh frame</button>
</header>
<main>
<div class="workspace">
  <figure class="device">
    <canvas id="display" width="1072" height="1448" tabindex="0" role="application" aria-label="Kobo grayscale display" aria-describedby="instructions"></canvas>
    <figcaption id="instructions">Raw 1072 × 1448 grayscale frame. Click or tap the display to send touch coordinates.</figcaption>
  </figure>
  <aside class="status-panel" aria-label="Simulator status">
    <h2>Connection</h2>
    <p class="status" id="status" aria-live="polite">Loading frame.</p>
    <label class="diagnostic-toggle"><input type="checkbox" id="overlay" checked> Show diagnostic outlines</label>
    <h2>Layout diagnostics</h2>
    <ul class="diagnostics" id="diagnostics"><li>Checking screen…</li></ul>
    <p class="key">Keyboard: focus the display, then press Enter or Space to repeat the last touch.</p>
  </aside>
</div>
</main>
<script>
const canvas=document.getElementById("display"), ctx=canvas.getContext("2d",{alpha:false});
const status=document.getElementById("status"), list=document.getElementById("diagnostics"), overlay=document.getElementById("overlay"); let point={x:536,y:177}, issues=[];
function showDiagnostics(){list.replaceChildren();if(!issues.length){const item=document.createElement("li");item.textContent="No layout issues.";list.append(item);return;}
 for(const issue of issues){const item=document.createElement("li");item.className=issue.severity;item.textContent=issue.message;list.append(item);}}
function drawDiagnostics(){if(!overlay.checked)return;ctx.save();ctx.lineWidth=5;for(const issue of issues){if(!issue.rect)continue;ctx.strokeStyle=issue.severity==="error"?"#d00000":"#b56a00";const r=issue.rect;ctx.strokeRect(r.x+2,r.y+2,Math.max(0,r.width-4),Math.max(0,r.height-4));}ctx.restore();}
async function frame(){const [r,d]=await Promise.all([fetch("/frame",{cache:"no-store"}),fetch("/diagnostics",{cache:"no-store"})]);const raw=new Uint8Array(await r.arrayBuffer());issues=(await d.json()).issues;
 if(raw.length!==1072*1448)throw Error("Invalid frame");const image=ctx.createImageData(1072,1448);
 for(let i=0;i<raw.length;i++){const p=i*4;image.data[p]=image.data[p+1]=image.data[p+2]=raw[i];image.data[p+3]=255;}ctx.putImageData(image,0,0);showDiagnostics();drawDiagnostics();status.textContent=issues.length?`Frame loaded with ${issues.length} diagnostic${issues.length===1?"":"s"}.`:"Frame loaded; layout clean.";}
function touchLocation(event){const r=canvas.getBoundingClientRect();return {x:Math.floor((event.clientX-r.left)*1072/r.width),y:Math.floor((event.clientY-r.top)*1448/r.height)};}
async function touch(next){point=next;await fetch("/touch",{method:"POST",headers:{"Content-Type":"text/plain"},body:`x=${point.x}&y=${point.y}`});await frame();status.textContent="Display updated.";}
canvas.addEventListener("pointerup",event=>{event.preventDefault();touch(touchLocation(event)).catch(error=>status.textContent=error.message);});
canvas.addEventListener("keydown",event=>{if(event.key==="Enter"||event.key===" "){event.preventDefault();touch(point).catch(error=>status.textContent=error.message);}});
document.getElementById("refresh").addEventListener("click",()=>frame().catch(error=>status.textContent=error.message));
overlay.addEventListener("change",()=>frame().catch(error=>status.textContent=error.message));
frame().catch(error=>status.textContent=error.message);
</script>
</body></html>"##;

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;

    static NEXT_PRIVATE_DIR: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn a_task_that_finishes_while_nothing_is_typed_still_reaches_the_application() {
        // The message loop blocks on the application's socket, so a fetch
        // taking seconds used to be delivered only when the developer next
        // tapped something. Refusals arrived instantly, which is why nothing
        // noticed: the only tasks the simulator completed were refused ones.
        let (client, server) = UnixStream::pair().expect("a socket pair");
        let writer = Arc::new(Mutex::new(server));
        let state = Arc::new(Mutex::new(AppState::default()));
        let tasks = Arc::new(Mutex::new(
            TaskRunner::simulated(private_temp_dir())
                .with_capabilities([kobo_policy::Capability::Network]),
        ));
        {
            let draining = Arc::clone(&tasks);
            let writer = Arc::clone(&writer);
            let state = Arc::clone(&state);
            std::thread::spawn(move || drain_tasks(&draining, &writer, &state));
        }
        tasks
            .lock()
            .expect("the task lock")
            .submit(
                kobo_protocol::TaskId(1),
                kobo_protocol::Task::Sleep { seconds: 0 },
            )
            .expect("the task was accepted");

        let mut client = client;
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("a read timeout");
        let frame = read_from(&mut client).expect("an outcome arrived unprompted");
        assert!(
            matches!(
                frame.message,
                Message::TaskOutcome {
                    task: kobo_protocol::TaskId(1),
                    ..
                }
            ),
            "expected the outcome, got {:?}",
            frame.message
        );
    }

    #[test]
    fn the_simulator_reaches_the_network_unless_it_is_told_not_to() {
        // Refusing every request taught a developer nothing except that the
        // simulator refuses requests, and an application that can only reach
        // the network on the device can only be built on the device. Failure
        // handling is still reachable, deliberately, through one variable.
        let mut online = simulated_tasks();
        assert!(
            online
                .submit(
                    kobo_protocol::TaskId(1),
                    kobo_protocol::Task::Fetch {
                        url: "https://example.invalid/x".into(),
                        offset: 0,
                        max_bytes: 16,
                    },
                )
                .is_ok(),
            "the simulator refused a fetch outright"
        );
        let denied = online.drain().into_iter().any(|finished| {
            matches!(
                finished.outcome,
                kobo_protocol::TaskOutcome::Failed(kobo_protocol::TaskError::Denied)
            )
        });
        assert!(!denied, "the simulator denied a fetch on capability alone");
        online.shutdown();
    }

    fn private_temp_dir() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "ks-{}-{}",
            std::process::id(),
            NEXT_PRIVATE_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).expect("create private directory");
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
            .expect("protect private directory");
        root
    }

    #[test]
    fn parses_bounded_http_request() {
        let request = parse_request(b"POST /touch HTTP/1.1\r\nHost: localhost\r\n\r\nx=12&y=34")
            .expect("valid request");
        assert_eq!(request.method, "POST");
        assert_eq!(parse_touch(&request.body), Some((12, 34)));
    }

    #[test]
    fn frame_and_touch_use_ui_hit_testing() {
        let mut simulator = Simulator::new();
        assert_eq!(
            simulator.frame().len(),
            (DISPLAY_WIDTH * DISPLAY_HEIGHT) as usize
        );
        let button = simulator.screen().layout().nodes[2].rect;
        assert_eq!(
            simulator.touch(button.x + button.width / 2, button.y + button.height / 2),
            Some(ActionId(1))
        );
        assert_eq!(simulator.counter(), 1);
    }

    #[test]
    fn diagnostics_endpoint_payload_names_layout_failures() {
        let screen = Screen::new(
            1,
            (0..80)
                .map(|index| Node::Text {
                    id: NodeId(index + 1),
                    text: "One visible line".into(),
                })
                .collect(),
        );
        let payload = diagnostics_json(&screen, &kobo_ui::PictureCache::default());
        assert!(payload.starts_with("{\"issues\":["));
        assert!(payload.contains("below the content area"));
        assert!(payload.contains("\"severity\":\"error\""));
    }

    #[test]
    fn accepts_only_requested_loopback_addresses() {
        assert_eq!(
            parse_local_address("3000").expect("port"),
            "127.0.0.1:3000".parse().expect("address")
        );
        assert_eq!(
            parse_local_address("localhost:0").expect("localhost"),
            "127.0.0.1:0".parse().expect("address")
        );
        assert!(parse_local_address("0.0.0.0:3000").is_err());
        assert!(parse_local_address("192.0.2.1:3000").is_err());
        assert!(parse_local_address("[::1]:3000").is_err());
    }

    #[test]
    fn app_server_polling_reports_no_pending_connection() {
        let root = private_temp_dir();
        let socket_path = root.join("app.sock");
        let server = AppServer::bind("127.0.0.1:0", &socket_path).expect("bind app server");
        server.set_nonblocking(true).expect("enable polling");
        assert!(server.try_accept_app().expect("poll app").is_none());
        drop(server);
        assert!(!socket_path.exists());
        fs::remove_dir(root).expect("remove private directory");
    }

    #[test]
    fn app_server_rejects_non_private_socket_parent() {
        let root = private_temp_dir();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).expect("make parent unsafe");
        assert!(AppServer::bind("127.0.0.1:0", root.join("app.sock")).is_err());
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
            .expect("restore private permissions");
        fs::remove_dir(root).expect("remove private directory");
    }

    #[test]
    fn app_server_handshakes_renders_and_returns_actions() {
        let root = private_temp_dir();
        let socket_path = root.join("app.sock");
        let server = AppServer::bind("127.0.0.1:0", &socket_path).expect("bind app server");
        assert_eq!(
            fs::symlink_metadata(&socket_path)
                .expect("socket metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let address = server.local_addr().expect("HTTP address");
        let (ready_sender, ready_receiver) = mpsc::channel();
        let app_socket_path = socket_path.clone();
        let app = thread::spawn(move || -> io::Result<ActionId> {
            let mut stream = UnixStream::connect(&app_socket_path)?;
            write_protocol_frame(
                &mut stream,
                &Frame {
                    request_id: 7,
                    message: Message::Hello {
                        name: "test app".into(),
                    },
                },
            )?;
            let welcome = read_protocol_frame(&mut stream)?;
            assert_eq!(welcome.request_id, 7);
            assert_eq!(
                welcome.message,
                Message::Welcome {
                    width: PROTOCOL_WIDTH,
                    height: PROTOCOL_HEIGHT,
                    pixels_per_inch: PROTOCOL_PPI,
                    text_scale: kobo_ui::TextScale::Default,
                }
            );
            write_protocol_frame(
                &mut stream,
                &Frame {
                    request_id: 8,
                    message: Message::SetScreen(Screen::new(
                        1,
                        vec![Node::Button {
                            id: NodeId(1),
                            action: ActionId(9),
                            label: "Tap".into(),
                        }],
                    )),
                },
            )?;
            ready_sender.send(()).expect("test receiver");
            match read_protocol_frame(&mut stream)?.message {
                Message::Action { action } => Ok(action),
                _ => Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "expected action",
                )),
            }
        });
        let session = server.accept_app().expect("accept app");
        ready_receiver.recv().expect("screen sent");
        for _ in 0..100 {
            if !session.screen().nodes.is_empty() {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        assert!(!session.screen().nodes.is_empty());

        let browser = thread::spawn(move || -> io::Result<()> {
            let mut stream = TcpStream::connect(address)?;
            stream.write_all(
                b"POST /touch HTTP/1.1\r\nHost: localhost\r\nContent-Length: 9\r\n\r\nx=60&y=60",
            )?;
            let mut response = String::new();
            stream.read_to_string(&mut response)?;
            assert!(response.starts_with("HTTP/1.1 204"));
            Ok(())
        });
        server.serve_one(&session).expect("serve touch");
        browser.join().expect("browser thread").expect("browser IO");
        assert_eq!(
            app.join().expect("app thread").expect("app IO"),
            ActionId(9)
        );
        drop(session);
        drop(server);
        assert!(!socket_path.exists());
        fs::remove_dir(root).expect("remove private directory");
    }

    #[test]
    fn app_server_does_not_remove_replacement_path() {
        let root = private_temp_dir();
        let socket_path = root.join("app.sock");
        let server = AppServer::bind("127.0.0.1:0", &socket_path).expect("bind app server");
        fs::remove_file(&socket_path).expect("unlink server socket");
        fs::write(&socket_path, b"replacement").expect("write replacement");

        drop(server);
        assert_eq!(
            fs::read(&socket_path).expect("replacement remains"),
            b"replacement"
        );
        fs::remove_file(socket_path).expect("remove replacement");
        fs::remove_dir(root).expect("remove private directory");
    }
}

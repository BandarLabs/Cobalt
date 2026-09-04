#![forbid(unsafe_code)]

//! Localhost-only browser simulator for Kobo grayscale screens.

use std::fs;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, LazyLock, Mutex};
use std::thread;

// Panel policy belongs to the runtime, but the simulator compiles the same
// source so region and waveform decisions cannot drift.
#[path = "../../kobod/src/frame.rs"]
mod frame;
use frame::{FramePlanner, FrameTransition, PanelWaveform};
use kobo_policy::{shelf::Shelf, store::Store, DeviceServices, TaskRunner};
use kobo_profile::{DeviceProfile, PanelPose, CLARA_BW_391, SUPPORTED_PROFILES};
use kobo_protocol::{read_from, write_to, Frame, Lifecycle, Message};
use kobo_ui::{ActionId, DisplayMetrics, Node, NodeId, Screen, Surface};

const MAX_HTTP_HEADER: usize = 8 * 1024;
static PROFILE: LazyLock<&'static DeviceProfile> = LazyLock::new(|| {
    let requested = std::env::var("KOBO_SIM_PROFILE").ok();
    requested
        .as_deref()
        .and_then(|id| {
            SUPPORTED_PROFILES
                .iter()
                .copied()
                .find(|profile| profile.id == id)
        })
        .unwrap_or(&CLARA_BW_391)
});
/// The simulator asserts an orientation rather than observing one, because it
/// has no device. That is legitimate here and nowhere near a real framebuffer:
/// it means the browser exercises exactly the transform the selected profile
/// was measured at.
static POSE: LazyLock<PanelPose<'static>> = LazyLock::new(|| PanelPose::reference(*PROFILE));

#[must_use]
pub fn selected_profile() -> &'static DeviceProfile {
    *PROFILE
}

fn profile_metrics() -> DisplayMetrics {
    DisplayMetrics {
        width: i32::try_from(PROFILE.width).unwrap_or(i32::MAX),
        height: i32::try_from(PROFILE.height).unwrap_or(i32::MAX),
        pixels_per_inch: i32::from(PROFILE.pixels_per_inch),
        text_scale: kobo_ui::display_metrics_from_env().text_scale,
    }
}

/// A deterministic failure mode selected from the simulator controls.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Scenario {
    #[default]
    Normal,
    /// The reader has no network at all.
    Offline,
    /// The reader has a network, but the host it wants does not answer.
    ///
    /// Separate from `Offline` because an app should say different things
    /// about the two, and a simulator that could only produce one of them
    /// would let an app ship having only been shown half the problem.
    HostDown,
    LowBattery,
    PermissionDenied,
    MissingSecret,
    NetworkTimeout,
    StorageFull,
    CachePressure,
}

impl Scenario {
    const ALL: [Self; 9] = [
        Self::Normal,
        Self::Offline,
        Self::HostDown,
        Self::LowBattery,
        Self::PermissionDenied,
        Self::MissingSecret,
        Self::NetworkTimeout,
        Self::StorageFull,
        Self::CachePressure,
    ];

    const fn name(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Offline => "offline",
            Self::HostDown => "host-down",
            Self::LowBattery => "low-battery",
            Self::PermissionDenied => "permission-denied",
            Self::MissingSecret => "missing-secret",
            Self::NetworkTimeout => "network-timeout",
            Self::StorageFull => "storage-full",
            Self::CachePressure => "cache-pressure",
        }
    }

    fn parse(bytes: &[u8]) -> Option<Self> {
        let name = std::str::from_utf8(bytes).ok()?.trim();
        Self::ALL
            .into_iter()
            .find(|scenario| scenario.name() == name)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SimulatedTouch {
    display: (u32, u32),
    raw: (i32, i32),
}

/// The panel's visible state, including a deliberately labelled approximation
/// of residue left by non-cleaning updates.
#[derive(Debug)]
struct PanelPreview {
    planner: FramePlanner,
    ideal: Vec<u8>,
    visible: Vec<u8>,
    last: Option<FrameTransition>,
}

impl PanelPreview {
    fn new() -> Self {
        let width = PROFILE.width as usize;
        let height = PROFILE.height as usize;
        let pixels = width.saturating_mul(height);
        Self {
            planner: FramePlanner::new(width, height),
            ideal: vec![kobo_ui::tone::PAPER; pixels],
            visible: vec![kobo_ui::tone::INK; pixels],
            last: None,
        }
    }

    fn update(&mut self, surface: &Surface) {
        self.ideal.clear();
        self.ideal.extend_from_slice(&surface.pixels);
        let Some(transition) = self.planner.plan(surface) else {
            self.last = None;
            return;
        };
        if transition.full {
            self.visible.copy_from_slice(&surface.pixels);
        } else {
            self.apply_partial(surface, &transition);
        }
        if self.planner.commit(surface, &transition) {
            self.last = Some(transition);
        }
    }

    fn apply_partial(&mut self, surface: &Surface, transition: &FrameTransition) {
        for update in &transition.regions {
            let (Ok(left), Ok(top), Ok(width), Ok(height)) = (
                usize::try_from(update.region.x),
                usize::try_from(update.region.y),
                usize::try_from(update.region.width),
                usize::try_from(update.region.height),
            ) else {
                continue;
            };
            for y in top..top.saturating_add(height) {
                let row = y.saturating_mul(surface.width);
                for x in left..left.saturating_add(width) {
                    let index = row.saturating_add(x);
                    let Some(target) = surface.pixels.get(index).copied() else {
                        continue;
                    };
                    let Some(visible) = self.visible.get_mut(index) else {
                        continue;
                    };
                    let target = match update.waveform {
                        PanelWaveform::Du => {
                            if target < 128 {
                                kobo_ui::tone::INK
                            } else {
                                kobo_ui::tone::PAPER
                            }
                        }
                        PanelWaveform::Gl16 | PanelWaveform::Gc16 => target,
                    };
                    // An LCD cannot reproduce electrophoretic residue. Retaining
                    // one sixteenth of the previous displayed value makes stale
                    // edges visible without claiming hardware-measured physics.
                    *visible = u8::try_from((u16::from(target) * 15 + u16::from(*visible)) / 16)
                        .unwrap_or(target);
                }
            }
        }
    }

    fn frame(&self, ideal: bool) -> &[u8] {
        if ideal {
            &self.ideal
        } else {
            &self.visible
        }
    }
}

/// A deterministic interactive counter used to exercise rendering and hit testing.
#[derive(Debug)]
pub struct Simulator {
    counter: u32,
    screen: Screen,
    panel: PanelPreview,
    scenario: Scenario,
    lifecycle: Lifecycle,
    last_touch: Option<SimulatedTouch>,
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
            panel: PanelPreview::new(),
            scenario: Scenario::Normal,
            lifecycle: Lifecycle::Foreground,
            last_touch: None,
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
    pub fn frame(&mut self) -> Vec<u8> {
        self.render_frame(false)
    }

    #[must_use]
    pub fn ideal_frame(&mut self) -> Vec<u8> {
        self.render_frame(true)
    }

    fn render_frame(&mut self, ideal: bool) -> Vec<u8> {
        let mut surface = Surface::new(PROFILE.width as usize, PROFILE.height as usize);
        kobo_ui::render_with(
            &self.screen,
            &profile_metrics(),
            &kobo_ui::Chrome::default(),
            &mut surface,
            None,
        );
        self.panel.update(&surface);
        self.panel.frame(ideal).to_vec()
    }

    pub fn touch(&mut self, x: i32, y: i32) -> Option<ActionId> {
        let (display_x, display_y) = (u32::try_from(x).ok()?, u32::try_from(y).ok()?);
        let raw = POSE.display_to_touch(display_x, display_y)?;
        let display = POSE.touch_to_display(raw.0, raw.1)?;
        self.last_touch = Some(SimulatedTouch { display, raw });
        let action = self.screen.hit_test(
            i32::try_from(display.0).ok()?,
            i32::try_from(display.1).ok()?,
        )?;
        if action == ActionId(1) {
            self.counter = self.counter.saturating_add(1);
            self.rebuild_screen();
        }
        Some(action)
    }

    fn simulation_json(&self) -> String {
        simulation_json(&self.panel, self.scenario, self.lifecycle, self.last_touch)
    }

    fn rebuild_screen(&mut self) {
        self.screen = Screen::new(
            1,
            vec![
                Node::Heading {
                    id: NodeId(1),
                    text: "Counter".into(),
                    level: 1,
                },
                Node::Text {
                    id: NodeId(2),
                    text: format!("Value: {}", self.counter),
                    links: Vec::new(),
                },
                Node::Button {
                    id: NodeId(3),
                    action: ActionId(1),
                    label: "Increment".into(),
                    state: kobo_ui::ControlState::Enabled,
                    emphasis: kobo_ui::Emphasis::Primary,
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
    /// One connection that goes wrong is reported and stepped over rather than
    /// ending the session. A browser opening a speculative connection and
    /// closing it unused, a port knock, or a reload abandoned halfway all
    /// arrive here as a read error on one stream, and none of them is a reason
    /// to take the panel away from somebody who is working.
    ///
    /// # Errors
    ///
    /// Returns an error if the listener itself stops accepting, which is not
    /// something the next connection would recover from.
    pub fn serve(&mut self) -> io::Result<()> {
        loop {
            let (stream, _) = self.listener.accept()?;
            if let Err(error) = self.handle(stream) {
                report_dropped_request(&error);
            }
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
            ("GET", "/ideal-frame") => {
                let frame = self.simulator.ideal_frame();
                write_response(&mut stream, 200, "application/octet-stream", &frame)
            }
            ("GET", "/simulation") => {
                let body = self.simulator.simulation_json();
                write_response(
                    &mut stream,
                    200,
                    "application/json; charset=utf-8",
                    body.as_bytes(),
                )
            }
            ("GET", "/layout") => {
                let body = layout_json(self.simulator.screen(), 0);
                write_response(
                    &mut stream,
                    200,
                    "application/json; charset=utf-8",
                    body.as_bytes(),
                )
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
            ("POST", "/scenario") => match Scenario::parse(&request.body) {
                Some(scenario) => {
                    self.simulator.scenario = scenario;
                    write_response(&mut stream, 204, "text/plain; charset=utf-8", b"")
                }
                None => write_response(
                    &mut stream,
                    400,
                    "text/plain; charset=utf-8",
                    b"invalid scenario",
                ),
            },
            ("POST", "/lifecycle") => match parse_lifecycle(&request.body) {
                Some(lifecycle) => {
                    self.simulator.lifecycle = lifecycle;
                    write_response(&mut stream, 204, "text/plain; charset=utf-8", b"")
                }
                None => write_response(
                    &mut stream,
                    400,
                    "text/plain; charset=utf-8",
                    b"invalid lifecycle",
                ),
            },
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
    apps: Arc<Mutex<SimulatedApps>>,
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
            apps: Arc::new(Mutex::new(SimulatedApps::default())),
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
        self.start_session(&mut stream)
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
                self.start_session(&mut stream).map(Some)
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn start_session(&self, stream: &mut UnixStream) -> io::Result<AppSession> {
        let hello = read_protocol_frame(stream)?;
        // Kept, not just checked. The name is the identity credential policy
        // is written against, so a simulator that threw it away could not
        // apply the same policy the device applies.
        let Message::Hello { name } = hello.message.clone() else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "SDK must send Hello before other messages",
            ));
        };
        if !valid_app_name(&name) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "SDK application name is not a safe data directory name",
            ));
        }
        write_protocol_frame(
            stream,
            &Frame {
                request_id: hello.request_id,
                message: Message::Welcome {
                    width: u16::try_from(PROFILE.width).unwrap_or(u16::MAX),
                    height: u16::try_from(PROFILE.height).unwrap_or(u16::MAX),
                    pixels_per_inch: PROFILE.pixels_per_inch,
                    text_scale: profile_metrics().text_scale,
                },
            },
        )?;
        let reader = stream.try_clone()?;
        let state = Arc::new(Mutex::new(AppState::with_apps(Arc::clone(&self.apps))));
        let reader_state = Arc::clone(&state);
        // One writer for the whole session, shared by every thread that has
        // something to say to the application: taps from the browser, replies
        // to requests, and terminal output arriving on its own. Frames are
        // length-prefixed, so two of them written at once do not make two
        // frames, they make one unreadable stream.
        let writer = AppWriter::spawn(stream.try_clone()?);
        let reader_writer = Arc::clone(&writer);
        thread::spawn(move || {
            // A malformed frame ends the session rather than being skipped,
            // and the developer is told which one: a reader that dies quietly
            // leaves an application talking to nobody, with a panel that keeps
            // showing the last good screen and ignores every tap.
            if let Err(error) = read_app_messages(reader, &name, &reader_writer, &reader_state) {
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
            let (stream, _) = self.http.accept()?;
            if let Err(error) = session.handle_http(stream) {
                report_dropped_request(&error);
            }
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
    /// A connection that fails to produce a request is reported and counted as
    /// served, for the reason given on [`Server::serve`]: the developer whose
    /// application is running behind this is not the one who opened it.
    ///
    /// # Errors
    ///
    /// Returns an error when the listener itself stops accepting.
    pub fn try_serve_one(&self, session: &AppSession) -> io::Result<bool> {
        match self.http.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false)?;
                if let Err(error) = session.handle_http(stream) {
                    report_dropped_request(&error);
                }
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
    match message {
        Message::PutFont {
            handle,
            name,
            bytes,
        } => match kobo_text::BookFont::from_bytes(&bytes, &name, profile_metrics()) {
            Ok(font) => {
                kobo_ui::put_book_typesetter(handle, Box::new(font));
                Ok(())
            }
            Err(error) => note(state, &format!("font {} refused: {error}", handle.0)),
        },
        Message::DropFont { handle } => {
            kobo_ui::drop_book_typesetter(handle);
            Ok(())
        }
        other => hold_picture(state, other),
    }
}

fn hold_picture(state: &Arc<Mutex<AppState>>, message: Message) -> io::Result<()> {
    let diagnostic = {
        let mut held = state
            .lock()
            .map_err(|_| io::Error::other("app state lock poisoned"))?;
        let pictures = held.active_pictures_mut();
        match message {
            Message::PutPicture {
                handle,
                width,
                height,
                grey,
            } => picture_result(handle, pictures.put_report(handle, width, height, grey)),
            Message::BeginPicture {
                handle,
                width,
                height,
            } => (!pictures.begin_upload(handle, width, height))
                .then(|| format!("picture {} upload refused", handle.0)),
            Message::PictureChunk {
                handle,
                offset,
                grey,
            } => (!pictures.upload_chunk(
                handle,
                usize::try_from(offset).unwrap_or(usize::MAX),
                &grey,
            ))
            .then(|| format!("picture {} chunk refused", handle.0)),
            Message::CommitPicture { handle } => {
                let result = pictures.commit_upload(handle);
                picture_result(handle, result)
            }
            Message::DropPicture { handle } => {
                pictures.remove(handle);
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
            | Message::PutFont { .. }
            | Message::DropFont { .. }
    )
}

#[derive(Debug)]
struct AppState {
    screen: Screen,
    orientation: kobo_ui::Orientation,
    /// How many screens the application has painted since it started.
    ///
    /// A driver posts a tap to this process and the application answers it in
    /// another, so a driver that read the layout straight back read the screen
    /// it had just tapped and concluded the tap did nothing. Counting paints
    /// gives it something to wait on. Not the screen's own id, which is the
    /// application's name and never changes.
    paints: u64,
    logs: Vec<String>,
    /// The same bounded cache the device runtime uses, so a preview that shows
    /// a cover and a panel that does not would be a real difference rather than
    /// a simulator shortcut.
    pictures: kobo_ui::PictureCache,
    /// A disposable low-budget cache used only while the pressure scenario is
    /// active. Keeping it separate makes leaving the scenario restore the
    /// normal preview instead of permanently deleting pictures the app sent.
    pressure_pictures: kobo_ui::PictureCache,
    panel: PanelPreview,
    scenario: Scenario,
    lifecycle: Lifecycle,
    last_touch: Option<SimulatedTouch>,
    apps: Arc<Mutex<SimulatedApps>>,
}

impl Default for AppState {
    fn default() -> Self {
        Self::with_apps(Arc::new(Mutex::new(SimulatedApps::default())))
    }
}

impl AppState {
    fn with_apps(apps: Arc<Mutex<SimulatedApps>>) -> Self {
        Self {
            screen: Screen::new(0, Vec::new()),
            orientation: kobo_ui::Orientation::Portrait,
            paints: 0,
            logs: Vec::new(),
            pictures: kobo_ui::PictureCache::default(),
            pressure_pictures: kobo_ui::PictureCache::new(256 * 1024),
            panel: PanelPreview::new(),
            scenario: Scenario::Normal,
            lifecycle: Lifecycle::Foreground,
            last_touch: None,
            apps,
        }
    }
}

#[derive(Debug)]
struct SimulatedApps {
    catalog: Vec<kobo_protocol::AppInfo>,
}

impl Default for SimulatedApps {
    #[allow(
        clippy::too_many_lines,
        reason = "the simulator catalog mirrors the complete first-party Store listing"
    )]
    fn default() -> Self {
        Self {
            catalog: vec![
                simulated_app(
                    "audiobook",
                    "Audiobook Studio",
                    "Audiobooks",
                    "Research, narrate and play an original audiobook about any topic.",
                    kobo_ui::Glyph::Headphones,
                    &["network", "audio", "bluetooth-audio", "bluetooth-control"],
                    true,
                ),
                simulated_app(
                    "brief",
                    "Daily Brief",
                    "Daily Brief",
                    "Collects the day's stories while you read something else.",
                    kobo_ui::Glyph::Clock,
                    &["network"],
                    true,
                ),
                simulated_app(
                    "chat",
                    "AI Command Center",
                    "AI Chat",
                    "Ask a question and tap the answer, rather than typing one.",
                    kobo_ui::Glyph::Chat,
                    &["network"],
                    true,
                ),
                simulated_app(
                    "gallery",
                    "Components",
                    "Components",
                    "Every UI primitive on real hardware, for checking by eye.",
                    kobo_ui::Glyph::Chart,
                    &["network"],
                    true,
                ),
                simulated_app(
                    "gutenbird",
                    "Gutenbird",
                    "Gutenbird",
                    "Sixty thousand free books from Project Gutenberg.",
                    kobo_ui::Glyph::Book,
                    &["network", "frontlight-control"],
                    true,
                ),
                simulated_app(
                    "hn",
                    "Hacker News",
                    "Hacker News",
                    "Top, New, Ask and Show, with whole comment threads.",
                    kobo_ui::Glyph::News,
                    &["network"],
                    true,
                ),
                simulated_app(
                    "magnet",
                    "Magnet",
                    "Magnet",
                    "Find the hall sensor behind the bezel and watch it answer.",
                    kobo_ui::Glyph::Magnet,
                    &["cover-sensor"],
                    true,
                ),
                simulated_app(
                    "rss",
                    "Feeds",
                    "Feeds",
                    "Follow a site by name and read its articles, not its layout.",
                    kobo_ui::Glyph::Rss,
                    &["network"],
                    true,
                ),
                simulated_app(
                    "settings",
                    "Settings",
                    "Settings",
                    "Connect Wi-Fi, manage hardware and update the Cobalt platform.",
                    kobo_ui::Glyph::Settings,
                    &[
                        "network",
                        "battery-read",
                        "bluetooth-control",
                        "wifi-control",
                    ],
                    true,
                ),
                simulated_app(
                    "sidekick",
                    "Sidekick",
                    "Sidekick",
                    "Approve or deny what your coding agents ask to run, from here.",
                    kobo_ui::Glyph::Key,
                    &["network"],
                    true,
                ),
                simulated_app(
                    "sudoku",
                    "Sudoku",
                    "Sudoku",
                    "A crisp touch-first Sudoku built for the e-ink panel.",
                    kobo_ui::Glyph::Grid,
                    &[],
                    false,
                ),
                simulated_app(
                    "terminal",
                    "Terminal",
                    "Terminal",
                    "A shell on the panel, with keys that send rather than collect.",
                    kobo_ui::Glyph::Terminal,
                    &["shell"],
                    true,
                ),
                simulated_app(
                    "tictactoe",
                    "Tic-tac-toe",
                    "Tic-tac-toe",
                    "Two players, one panel. Nought goes first.",
                    kobo_ui::Glyph::Grid,
                    &[],
                    true,
                ),
                simulated_app(
                    "todo",
                    "Todo",
                    "Todo",
                    "A list that remembers itself. Tap an item to finish it.",
                    kobo_ui::Glyph::Check,
                    &[],
                    true,
                ),
            ],
        }
    }
}

fn simulated_app(
    id: &str,
    title: &str,
    label: &str,
    summary: &str,
    glyph: kobo_ui::Glyph,
    capabilities: &[&str],
    installed: bool,
) -> kobo_protocol::AppInfo {
    kobo_protocol::AppInfo {
        id: id.to_owned(),
        title: title.to_owned(),
        label: label.to_owned(),
        summary: summary.to_owned(),
        version: "1.0.0".to_owned(),
        minimum_cobalt_version: env!("CARGO_PKG_VERSION").to_owned(),
        glyph,
        capabilities: capabilities
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
        installed_version: installed.then(|| "1.0.0".to_owned()),
    }
}

impl AppState {
    fn active_pictures(&self) -> &kobo_ui::PictureCache {
        if self.scenario == Scenario::CachePressure {
            &self.pressure_pictures
        } else {
            &self.pictures
        }
    }

    fn active_pictures_mut(&mut self) -> &mut kobo_ui::PictureCache {
        if self.scenario == Scenario::CachePressure {
            &mut self.pressure_pictures
        } else {
            &mut self.pictures
        }
    }
}

/// Connected SDK app state and the serialized action writer.
#[derive(Clone, Debug)]
pub struct AppSession {
    state: Arc<Mutex<AppState>>,
    writer: Arc<AppWriter>,
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
        write_shared(
            &self.writer,
            &Frame {
                request_id: 0,
                message: Message::Action { action },
            },
        )
    }

    fn send_lifecycle(&self, lifecycle: Lifecycle) -> io::Result<()> {
        write_shared(
            &self.writer,
            &Frame {
                request_id: 0,
                message: Message::Lifecycle(lifecycle),
            },
        )?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("app state lock poisoned"))?;
        state.lifecycle = lifecycle;
        state.logs.push(format!("lifecycle: {lifecycle:?}"));
        Ok(())
    }

    fn render_frame(&self, ideal: bool) -> Vec<u8> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut surface = Surface::new(PROFILE.width as usize, PROFILE.height as usize);
        kobo_ui::render_oriented(
            &state.screen,
            &profile_metrics(),
            &kobo_ui::Chrome::default(),
            state.active_pictures(),
            &mut surface,
            None,
            state.orientation,
        );
        state.panel.update(&surface);
        state.panel.frame(ideal).to_vec()
    }

    fn set_scenario(&self, scenario: Scenario) -> io::Result<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("app state lock poisoned"))?;
        if state.scenario == scenario {
            return Ok(());
        }
        state.scenario = scenario;
        if scenario == Scenario::CachePressure {
            state.pressure_pictures = kobo_ui::PictureCache::new(256 * 1024);
        }
        state.logs.push(format!("scenario: {}", scenario.name()));
        Ok(())
    }

    fn touch_action(&self, x: i32, y: i32) -> Option<ActionId> {
        let display = (u32::try_from(x).ok()?, u32::try_from(y).ok()?);
        let raw = POSE.display_to_touch(display.0, display.1)?;
        let mapped = POSE.touch_to_display(raw.0, raw.1)?;
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.last_touch = Some(SimulatedTouch {
            display: mapped,
            raw,
        });
        let (x, y) = kobo_ui::logical_touch(
            state.orientation,
            i32::try_from(PROFILE.width).ok()?,
            i32::try_from(mapped.0).ok()?,
            i32::try_from(mapped.1).ok()?,
        );
        state
            .screen
            .layout_with(
                &profile_metrics().for_orientation(state.orientation),
                &kobo_ui::Chrome::default(),
            )
            .hit_test(x, y)
    }

    #[allow(clippy::too_many_lines, reason = "one explicit route table")]
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
                let frame = self.render_frame(false);
                write_response(&mut stream, 200, "application/octet-stream", &frame)
            }
            ("GET", "/ideal-frame") => {
                let frame = self.render_frame(true);
                write_response(&mut stream, 200, "application/octet-stream", &frame)
            }
            ("GET", "/simulation") => {
                let body = {
                    let state = self
                        .state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    simulation_json(
                        &state.panel,
                        state.scenario,
                        state.lifecycle,
                        state.last_touch,
                    )
                };
                write_response(
                    &mut stream,
                    200,
                    "application/json; charset=utf-8",
                    body.as_bytes(),
                )
            }
            ("GET", "/layout") => {
                let body = {
                    let state = self
                        .state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    layout_json(&state.screen, state.paints)
                };
                write_response(
                    &mut stream,
                    200,
                    "application/json; charset=utf-8",
                    body.as_bytes(),
                )
            }
            ("GET", "/diagnostics") => {
                let body = {
                    let state = self
                        .state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    diagnostics_json(&state.screen, state.active_pictures())
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
                    .and_then(|(x, y)| self.touch_action(x, y))
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
            ("POST", "/scenario") => match Scenario::parse(&request.body) {
                Some(scenario) => match self.set_scenario(scenario) {
                    Ok(()) => write_response(&mut stream, 204, "text/plain; charset=utf-8", b""),
                    Err(_) => write_response(
                        &mut stream,
                        503,
                        "text/plain; charset=utf-8",
                        b"simulator state unavailable",
                    ),
                },
                None => write_response(
                    &mut stream,
                    400,
                    "text/plain; charset=utf-8",
                    b"invalid scenario",
                ),
            },
            ("POST", "/lifecycle") => match parse_lifecycle(&request.body) {
                Some(lifecycle) => match self.send_lifecycle(lifecycle) {
                    Ok(()) => write_response(&mut stream, 204, "text/plain; charset=utf-8", b""),
                    Err(_) => write_response(
                        &mut stream,
                        503,
                        "text/plain; charset=utf-8",
                        b"SDK unavailable",
                    ),
                },
                None => write_response(
                    &mut stream,
                    400,
                    "text/plain; charset=utf-8",
                    b"invalid lifecycle",
                ),
            },
            _ => write_response(&mut stream, 404, "text/plain; charset=utf-8", b"not found"),
        }
    }
}

fn diagnostics_json(screen: &Screen, pictures: &kobo_ui::PictureCache) -> String {
    let diagnostics =
        screen.diagnostics_with_pictures(&profile_metrics(), &kobo_ui::Chrome::default(), pictures);
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

fn simulation_json(
    panel: &PanelPreview,
    scenario: Scenario,
    lifecycle: Lifecycle,
    touch: Option<SimulatedTouch>,
) -> String {
    let transition = panel.last.as_ref().map_or_else(
        || "null".to_owned(),
        |transition| {
            let regions = transition
                .regions
                .iter()
                .map(|update| {
                    format!(
                        "{{\"waveform\":\"{}\",\"region\":{{\"x\":{},\"y\":{},\"width\":{},\"height\":{}}}}}",
                        update.waveform.name(),
                        update.region.x,
                        update.region.y,
                        update.region.width,
                        update.region.height,
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            format!(
                concat!(
                    "{{\"waveform\":\"{}\",\"full\":{},\"refresh\":{},",
                    "\"dirty\":{},\"regions\":[{}],\"region\":",
                    "{{\"x\":{},\"y\":{},\"width\":{},\"height\":{}}}}}"
                ),
                transition.waveform.name(),
                transition.full,
                transition.refresh,
                transition.dirty,
                regions,
                transition.region.x,
                transition.region.y,
                transition.region.width,
                transition.region.height,
            )
        },
    );
    let touch = touch.map_or_else(
        || "null".to_owned(),
        |touch| {
            format!(
                concat!(
                    "{{\"display\":{{\"x\":{},\"y\":{}}},",
                    "\"raw\":{{\"x\":{},\"y\":{}}}}}"
                ),
                touch.display.0, touch.display.1, touch.raw.0, touch.raw.1,
            )
        },
    );
    let lifecycle = match lifecycle {
        Lifecycle::Foreground => "foreground",
        Lifecycle::Background => "background",
    };
    format!(
        concat!(
            "{{\"profile\":{{\"id\":{},\"model\":{},\"width\":{},",
            "\"height\":{},\"pixelsPerInch\":{},\"rotation\":{},",
            "\"touch\":{{\"name\":{},\"xMin\":{},\"xMax\":{},",
            "\"yMin\":{},\"yMax\":{}}}}},\"scenario\":{},",
            "\"lifecycle\":{},\"transition\":{},\"refreshCount\":{},",
            "\"partialsSinceClean\":{},\"touch\":{},",
            "\"panelApproximation\":true}}"
        ),
        json_string(PROFILE.id),
        json_string(PROFILE.model),
        PROFILE.width,
        PROFILE.height,
        PROFILE.pixels_per_inch,
        PROFILE.rotation,
        json_string(PROFILE.touch_name),
        PROFILE.touch_x_min,
        PROFILE.touch_x_max,
        PROFILE.touch_y_min,
        PROFILE.touch_y_max,
        json_string(scenario.name()),
        json_string(lifecycle),
        transition,
        panel.planner.refreshes(),
        panel.planner.dirty(),
        touch,
    )
}

fn parse_lifecycle(bytes: &[u8]) -> Option<Lifecycle> {
    match std::str::from_utf8(bytes).ok()?.trim() {
        "foreground" => Some(Lifecycle::Foreground),
        "background" => Some(Lifecycle::Background),
        _ => None,
    }
}

/// Everything on the panel, named, measured and addressable.
///
/// The endpoint that makes the simulator drivable by something that is not a
/// pair of eyes. A test, a script or an agent can ask what is on screen, find
/// the control called "Search" by its label, and tap the middle of it -- and
/// the tap then goes through `POST /touch` like any other, so it is put
/// through the panel's own coordinate transform and the renderer's own
/// hit-testing rather than shortcutting to an action.
///
/// That last point is the whole design. An automation surface that dispatched
/// actions directly would pass happily on a screen whose button had been laid
/// out three millimetres off the bottom of the panel, which is exactly the
/// class of fault worth catching.
fn layout_json(screen: &Screen, paints: u64) -> String {
    let metrics = profile_metrics();
    let layout = screen.layout_with(&metrics, &kobo_ui::Chrome::default());
    let mut json = format!("{{\"paints\":{paints},\"nodes\":[");
    for (index, node) in layout.nodes.iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        let lines = node
            .text_lines
            .iter()
            .map(|line| json_string(line))
            .collect::<Vec<_>>()
            .join(",");
        let centre = (
            node.rect.x + node.rect.width / 2,
            node.rect.y + node.rect.height / 2,
        );
        let action = layout
            .hit_test(centre.0, centre.1)
            .map_or_else(|| "null".to_owned(), |action| action.0.to_string());
        let _ = std::fmt::Write::write_fmt(
            &mut json,
            format_args!(
                "{{\"kind\":{},\"x\":{},\"y\":{},\"width\":{},\"height\":{},\
                 \"centre\":{{\"x\":{},\"y\":{}}},\"action\":{action},\"lines\":[{lines}]}}",
                json_string(&format!("{:?}", node.kind)),
                node.rect.x,
                node.rect.y,
                node.rect.width,
                node.rect.height,
                centre.0,
                centre.1,
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

/// Set this to any value to print the simulator's log as it happens.
///
/// Off by default because a shelf upload alone is hundreds of lines. On when a
/// developer is trying to find out why something failed, which is the only
/// time any of it is worth reading.
pub const VERBOSE: &str = "KOBO_SIM_LOG";

/// Records one line in the simulator's log, keeping only the recent tail.
///
/// The tail used to be the whole story: it was collected, capped at sixty four
/// lines, and then read by nothing at all. A developer watching an application
/// fail in the simulator could see the failed screen and had no way to find
/// out which request produced it. Now the interesting lines reach the terminal
/// the simulator is running in.
fn note(state: &Arc<Mutex<AppState>>, line: &str) -> io::Result<()> {
    if std::env::var_os(VERBOSE).is_some() {
        eprintln!("{line}");
    }
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
    writer: &Arc<AppWriter>,
    request_id: u32,
    store: &Store,
    shelf: &Shelf,
    request: &kobo_protocol::StoreRequest,
    state: &Arc<Mutex<AppState>>,
) -> io::Result<()> {
    // The shelf answers first, and answers `None` to everything that is not
    // its own, which is what keeps the two from having to know about each
    // other's request tags.
    let result = shelf
        .handle(request)
        .unwrap_or_else(|| store.handle(request));
    note(state, &format!("store: {request:?} -> {result:?}"))?;
    write_shared(
        writer,
        &Frame {
            request_id,
            message: Message::StoreResult(result),
        },
    )
}

/// The one thing allowed to write to the application's socket.
///
/// Four threads have something to say to the application: the message loop
/// answering requests, the browser thread delivering taps, the task drain, and
/// the terminal drain. A frame is length-prefixed, so two of them written at
/// once do not make two frames, they make one unreadable stream, and the
/// obvious answer is a mutex around the socket.
///
/// The obvious answer deadlocks. A socket write blocks once the kernel buffer
/// is full, and it is full exactly when the application is busy -- which, for
/// an application waiting on a synchronous store request, is until it gets its
/// answer, which is a frame nobody can write because the task drain is holding
/// the lock waiting for the buffer to drain. The whole simulator stopped
/// answering, including the screen, so it looked precisely like the
/// application had hung.
///
/// So the queue is the lock. Everyone hands a frame to this and returns
/// immediately; one thread does the blocking write, and when it blocks it
/// blocks alone.
#[cfg(test)]
mod writer_tests {
    use super::{write_shared, AppWriter};
    use kobo_protocol::{Frame, Message};

    /// The deadlock this file used to have, reproduced in the small.
    ///
    /// With a mutex around the socket, the fortieth or so of these blocked
    /// forever, because nothing is reading the other end and the kernel buffer
    /// is finite. Everything else that wanted to write to the application then
    /// queued behind it, including the answer the application was waiting for,
    /// and the simulator stopped answering its own HTTP port.
    #[test]
    fn writing_to_an_application_that_is_not_reading_does_not_block_the_writer() {
        let (ours, theirs) = std::os::unix::net::UnixStream::pair().expect("socket pair");
        // Deliberately never read from.
        let writer = AppWriter::spawn(ours);
        let frame = Frame {
            request_id: 0,
            message: Message::Log {
                level: kobo_protocol::LogLevel::Info,
                message: "x".repeat(4096),
            },
        };
        let (done, waiting) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for _ in 0..512 {
                if write_shared(&writer, &frame).is_err() {
                    break;
                }
            }
            let _ = done.send(());
        });
        waiting
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("512 frames queued without blocking on the socket");
        drop(theirs);
    }
}

impl std::fmt::Debug for AppWriter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AppWriter")
    }
}

struct AppWriter {
    /// `Sender` is `Send` but not `Sync`, and this is held by four threads.
    /// The lock is only ever held across a queue push, which cannot block on
    /// anything, which is the entire point.
    sender: Mutex<std::sync::mpsc::Sender<Frame>>,
}

impl AppWriter {
    /// Takes the socket, and returns the only handle anything else may use.
    fn spawn(mut stream: UnixStream) -> Arc<Self> {
        let (sender, receiver) = std::sync::mpsc::channel::<Frame>();
        std::thread::spawn(move || {
            for frame in receiver {
                if write_protocol_frame(&mut stream, &frame).is_err() {
                    break;
                }
            }
        });
        Arc::new(Self {
            sender: Mutex::new(sender),
        })
    }
}

/// Queues one frame for the application. Never blocks on the socket.
fn write_shared(writer: &Arc<AppWriter>, frame: &Frame) -> io::Result<()> {
    writer
        .sender
        .lock()
        .map_err(|_| io::Error::other("simulator write lock poisoned"))?
        .send(frame.clone())
        .map_err(|_| io::Error::other("the application is no longer listening"))
}

/// Delivers terminal output as it arrives, rather than when the next message
/// happens to come in.
///
/// Without this the simulator would only show what a program printed after the
/// developer pressed another key, so anything that prints on its own would look
/// like it had hung.
fn drain_shell(shells: &Arc<Mutex<kobo_shell::Shells>>, writer: &Arc<AppWriter>) -> io::Result<()> {
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
    writer: &Arc<AppWriter>,
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
fn simulated_shells(writer: &Arc<AppWriter>) -> Arc<Mutex<kobo_shell::Shells>> {
    let shells = Arc::new(Mutex::new(kobo_shell::Shells::new(&[
        kobo_policy::Capability::Shell,
    ])));
    let draining = Arc::clone(&shells);
    let writer = Arc::clone(writer);
    std::thread::spawn(move || drain_shell(&draining, &writer));
    shells
}

fn current_scenario(state: &Arc<Mutex<AppState>>) -> Scenario {
    state.lock().map_or_else(
        |poisoned| poisoned.into_inner().scenario,
        |state| state.scenario,
    )
}

fn scenario_task_error(
    scenario: Scenario,
    task: &kobo_protocol::Task,
) -> Option<kobo_protocol::TaskError> {
    let network = matches!(
        task,
        kobo_protocol::Task::Fetch { .. } | kobo_protocol::Task::Post { .. }
    );
    match scenario {
        Scenario::Offline if network => Some(kobo_protocol::TaskError::Offline),
        Scenario::HostDown if network => Some(kobo_protocol::TaskError::Unreachable),
        Scenario::LowBattery | Scenario::PermissionDenied if network => {
            Some(kobo_protocol::TaskError::Denied)
        }
        Scenario::MissingSecret
            if matches!(
                task,
                kobo_protocol::Task::Post {
                    credential: Some(_),
                    ..
                }
            ) =>
        {
            Some(kobo_protocol::TaskError::NotFound)
        }
        Scenario::NetworkTimeout if network => Some(kobo_protocol::TaskError::TimedOut),
        _ => None,
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "one exhaustive protocol message dispatcher"
)]
fn read_app_messages(
    mut stream: UnixStream,
    name: &str,
    writer: &Arc<AppWriter>,
    state: &Arc<Mutex<AppState>>,
) -> io::Result<()> {
    // The simulator owns no hardware, so it answers state queries from a
    // believable model and refuses everything that would change a real device.
    let mut services = DeviceServices::simulated();
    // There is no bezel here to hold a magnet against, so the state the hall
    // sensor reports is set on the way in. Without this the second half of
    // every cover-aware screen is unreachable off hardware.
    services.set_magnet(matches!(
        std::env::var("KOBO_MAGNET").as_deref(),
        Ok("1" | "present")
    ));
    let tasks = Arc::new(Mutex::new(simulated_tasks(name)));
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
    let store = Store::new(std::env::temp_dir().join("cobalt-sim-state").join(name));
    // The shelf is where an application keeps what will not fit in a message:
    // an audiobook, a downloaded book. Without one here every shelf request
    // came back `Unwritable`, so the one class of application that most needs
    // to be developed off the device -- the ones that take four minutes and a
    // dozen network calls to produce a file -- was the one class that could
    // not be run in the simulator at all.
    let shelf = Shelf::new(simulated_data_root(name));
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
                state.paints = state.paints.saturating_add(1);
            }
            Message::SetOrientation(orientation) => {
                let mut state = state
                    .lock()
                    .map_err(|_| io::Error::other("app state lock poisoned"))?;
                state.orientation = orientation;
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
                let scenario = current_scenario(state);
                services.observe_battery(
                    if scenario == Scenario::LowBattery {
                        5
                    } else {
                        72
                    },
                    false,
                );
                let result =
                    if let Some(result) = simulated_app_request(state, name, scenario, &request)? {
                        result
                    } else if !simulated_platform_request_allowed(name, &request) {
                        kobo_protocol::DeviceResult::Denied(kobo_protocol::DenyReason::NotDeclared)
                    } else {
                        match scenario {
                            Scenario::PermissionDenied => kobo_protocol::DeviceResult::Denied(
                                kobo_protocol::DenyReason::NotDeclared,
                            ),
                            _ => services.handle(request.clone()),
                        }
                    };
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
                if let Some(error) = scenario_task_error(current_scenario(state), &work) {
                    note(
                        state,
                        &format!("task {} injected failure: {error:?}", task.0),
                    )?;
                    write_shared(
                        writer,
                        &Frame {
                            request_id,
                            message: Message::TaskOutcome {
                                task,
                                outcome: kobo_protocol::TaskOutcome::Failed(error),
                            },
                        },
                    )?;
                    continue;
                }
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
                if current_scenario(state) == Scenario::StorageFull
                    && matches!(request, kobo_protocol::StoreRequest::Save { .. })
                {
                    let result =
                        kobo_protocol::StoreResult::Denied(kobo_protocol::StoreError::TooFull);
                    note(state, &format!("store: injected {result:?}"))?;
                    write_shared(
                        writer,
                        &Frame {
                            request_id,
                            message: Message::StoreResult(result),
                        },
                    )?;
                } else {
                    answer_store(writer, request_id, &store, &shelf, &request, state)?;
                }
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

fn simulated_platform_request_allowed(
    caller: &str,
    request: &kobo_protocol::DeviceRequest,
) -> bool {
    !matches!(request, kobo_protocol::DeviceRequest::Update { .. }) || caller == "settings"
}

fn simulated_app_request(
    state: &Arc<Mutex<AppState>>,
    caller: &str,
    scenario: Scenario,
    request: &kobo_protocol::DeviceRequest,
) -> io::Result<Option<kobo_protocol::DeviceResult>> {
    use kobo_protocol::{DenyReason, DeviceError, DeviceRequest, DeviceResult};

    let authorized = match request {
        DeviceRequest::ListInstalledApps => matches!(caller, "launcher" | "store"),
        DeviceRequest::ReadAppCatalog
        | DeviceRequest::RefreshAppCatalog
        | DeviceRequest::InstallApp { .. }
        | DeviceRequest::UninstallApp { .. } => caller == "store",
        _ => return Ok(None),
    };
    if !authorized || scenario == Scenario::PermissionDenied {
        return Ok(Some(DeviceResult::Denied(DenyReason::NotDeclared)));
    }
    let apps = state
        .lock()
        .map_err(|_| io::Error::other("app state lock poisoned"))?
        .apps
        .clone();
    let mut apps = apps
        .lock()
        .map_err(|_| io::Error::other("simulated apps lock poisoned"))?;
    let result = match request {
        DeviceRequest::ListInstalledApps => DeviceResult::Apps {
            entries: apps
                .catalog
                .iter()
                .filter(|entry| {
                    entry.is_installed() && !matches!(entry.id.as_str(), "settings" | "terminal")
                })
                .cloned()
                .collect(),
        },
        DeviceRequest::ReadAppCatalog => DeviceResult::Apps {
            entries: apps.catalog.clone(),
        },
        DeviceRequest::RefreshAppCatalog => match scenario {
            Scenario::Offline | Scenario::HostDown => {
                DeviceResult::Failed(DeviceError::Unreachable)
            }
            Scenario::NetworkTimeout => DeviceResult::Failed(DeviceError::TimedOut),
            _ => DeviceResult::Apps {
                entries: apps.catalog.clone(),
            },
        },
        DeviceRequest::InstallApp { id } => match scenario {
            Scenario::Offline | Scenario::HostDown => {
                DeviceResult::Failed(DeviceError::Unreachable)
            }
            Scenario::NetworkTimeout => DeviceResult::Failed(DeviceError::TimedOut),
            Scenario::StorageFull => DeviceResult::Failed(DeviceError::Backend),
            _ => {
                if let Some(entry) = apps.catalog.iter_mut().find(|entry| entry.id == *id) {
                    entry.installed_version = Some(entry.version.clone());
                    DeviceResult::Done
                } else {
                    DeviceResult::Failed(DeviceError::NotFound)
                }
            }
        },
        DeviceRequest::UninstallApp { id } => {
            if matches!(id.as_str(), "settings" | "terminal") {
                return Ok(Some(DeviceResult::Failed(DeviceError::InvalidInput)));
            }
            if let Some(entry) = apps.catalog.iter_mut().find(|entry| entry.id == *id) {
                entry.installed_version = None;
                DeviceResult::Done
            } else {
                DeviceResult::Failed(DeviceError::NotFound)
            }
        }
        _ => return Ok(None),
    };
    Ok(Some(result))
}

/// Delivers a finished task as soon as it finishes.
///
/// A fetch takes seconds, and nothing arrives from the application while it is
/// waiting for one, so without this thread the answer would only reach the
/// screen when the developer next tapped something.
fn drain_tasks(
    tasks: &Arc<Mutex<TaskRunner>>,
    writer: &Arc<AppWriter>,
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
    writer: &Arc<AppWriter>,
    state: &Arc<Mutex<AppState>>,
) -> io::Result<()> {
    let finished = tasks
        .lock()
        .map_err(|_| io::Error::other("simulator task lock poisoned"))?
        .drain();
    for finished in finished {
        // A failure is always printed, whatever the log setting. It is the one
        // line that explains a screen the developer is looking at, and the
        // alternative is guessing which of a dozen requests went wrong.
        if let kobo_protocol::TaskOutcome::Failed(error) = &finished.outcome {
            eprintln!("task {} failed: {error:?}", finished.task.0);
        }
        note(
            state,
            &format!("task {} -> {:?}", finished.task.0, finished.outcome),
        )?;
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
fn simulated_tasks(name: &str) -> TaskRunner {
    // The same owner trust roots the device loads, from the host's own
    // directory. Once per process: roots are process-wide and the TLS
    // configuration refuses additions after it is first used.
    static TRUST: std::sync::Once = std::sync::Once::new();
    TRUST.call_once(|| {
        let directory = std::env::var_os("HOME").map_or_else(
            || std::path::PathBuf::from(".kobo-trust"),
            |home| {
                std::path::PathBuf::from(home)
                    .join(".config")
                    .join("kobo")
                    .join("trust")
            },
        );
        let _ = kobo_net::trust_owner_roots_from_dir(&directory);
    });
    let runner = TaskRunner::simulated(simulated_data_root(name))
        .with_secrets(std::env::temp_dir().join(SIM_SECRETS));
    if std::env::var_os(OFFLINE).is_some() {
        return runner;
    }
    // The same policy the device applies, from the same function, because a
    // runner with no policy at all refuses every credentialed request. That
    // is what it did: an application that needs an API key reported
    // "Permission needed" in the simulator no matter which key was installed,
    // and could only ever be run on hardware.
    let app = name.to_owned();
    runner
        .with_fetch(Arc::new(kobo_net::fetch_from))
        .with_post(Arc::new(kobo_net::post))
        .with_credential_policy(Arc::new(move |credential, url, usage| {
            kobo_policy::credentials::allowed(&app, credential, url, usage)
        }))
        .with_capabilities([kobo_policy::Capability::Network])
}

/// Directory the host simulator uses for one application's shelf.
///
/// Companion commands such as `kobo frame push --sim` write here so a `kobo
/// dev` session sees the same files the reader would after an SSH push.
#[must_use]
pub fn simulated_data_root(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join("cobalt-sim-data").join(name)
}

fn valid_app_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
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

/// Says that one browser connection came to nothing, and carries on.
///
/// On stderr rather than stdout, so it cannot be mistaken for the address line
/// the simulator prints for somebody to paste into a browser.
fn report_dropped_request(error: &io::Error) {
    eprintln!("kobo: dropped one browser connection: {error}");
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
<title>Kobo Clara BW simulator</title>
<style>
:root { color-scheme:dark; --workspace:#151515; --panel:#222; --raised:#2b2b2b; --border:#5c5c5c; --text:#f7f7f7; --muted:#c6c6c6; --paper:#f8f8f8; --focus:#fff; --accent:#9fd4ff; --warning:#ffd18b; --error:#ffb4ab; --space-1:8px; --space-2:16px; --space-3:24px; }
* { box-sizing:border-box; }
body { margin:0; min-height:100vh; background:var(--workspace); color:var(--text); font:16px/1.5 system-ui,sans-serif; }
button,select,canvas { font:inherit; }
button,select { min-height:44px; border:1px solid var(--border); border-radius:4px; }
button { padding:0 14px; background:var(--raised); color:var(--text); font-weight:700; cursor:pointer; }
button:hover { border-color:var(--text); }
button:focus-visible,select:focus-visible,canvas:focus-visible,input:focus-visible { outline:3px solid var(--focus); outline-offset:3px; }
.toolbar { min-height:68px; display:flex; align-items:center; gap:var(--space-2); padding:12px max(var(--space-2), calc((100vw - 1480px)/2)); border-bottom:1px solid var(--border); background:var(--panel); }
.toolbar h1 { margin:0; font-size:1.05rem; letter-spacing:.01em; }
.toolbar p { margin:0 auto 0 0; color:var(--muted); font-size:.875rem; }
.badge { padding:4px 9px; border:1px solid var(--border); border-radius:999px; color:var(--accent); font:700 .75rem/1.4 ui-monospace,monospace; }
.primary { border-color:var(--text); background:var(--text); color:var(--workspace); }
main { max-width:1480px; margin:auto; padding:clamp(16px,3vw,40px); }
.workspace { display:grid; grid-template-columns:minmax(0,1fr) 320px; gap:var(--space-3); align-items:start; }
.device { margin:0; overflow:auto; padding:16px; border:1px solid var(--border); background:var(--panel); }
.screen { position:relative; width:min(100%,1072px); margin:auto; overflow:hidden; background:#000; }
.device canvas { display:block; width:100%; height:auto; background:var(--paper); image-rendering:pixelated; touch-action:manipulation; }
.device canvas.clean-flash { animation:clean-flash 460ms steps(1,end); }
@keyframes clean-flash { 0%,100% { filter:none; } 22% { filter:brightness(0); } 52% { filter:brightness(4); } }
figcaption { margin-top:12px; color:var(--muted); font-size:.875rem; }
.inspector { display:grid; gap:var(--space-2); }
.card { padding:16px; border:1px solid var(--border); background:var(--panel); }
.card h2 { margin:0 0 10px; font-size:.95rem; }
.status { min-height:1.5em; margin:0; color:var(--muted); }
.facts { display:grid; grid-template-columns:1fr auto; gap:6px 12px; margin:0; font-size:.8125rem; }
.facts dt { color:var(--muted); }
.facts dd { margin:0; text-align:right; font-family:ui-monospace,monospace; }
.control { display:grid; gap:6px; margin-top:12px; color:var(--muted); font-size:.875rem; }
.control select { width:100%; padding:0 10px; background:var(--raised); color:var(--text); }
.check { display:flex; align-items:center; gap:9px; min-height:44px; color:var(--muted); font-size:.875rem; }
.check input { width:18px; height:18px; }
.buttons { display:grid; grid-template-columns:1fr 1fr; gap:8px; margin-top:10px; }
.diagnostics { max-height:30vh; overflow:auto; margin:8px 0 0; padding-left:20px; color:var(--muted); font-size:.8125rem; }
.diagnostics li + li { margin-top:8px; }
.diagnostics .error { color:var(--error); }
.diagnostics .warning { color:var(--warning); }
.note { margin:10px 0 0; color:var(--muted); font-size:.75rem; }
@media (max-width:900px) { .toolbar { align-items:flex-start; flex-wrap:wrap; } .toolbar p { order:4; width:100%; } .workspace { grid-template-columns:1fr; } .inspector { grid-template-columns:repeat(2,minmax(0,1fr)); } }
@media (max-width:620px) { .inspector { grid-template-columns:1fr; } .device { padding:8px; } .badge { display:none; } }
@media (prefers-reduced-motion:reduce) { .device canvas.clean-flash { animation:none; } }
@media (prefers-contrast:more) { :root { --workspace:#000; --panel:#000; --raised:#000; --border:#fff; --text:#fff; --muted:#fff; --accent:#fff; --warning:#fff; --error:#fff; } }
</style>
</head>
<body>
<header class="toolbar">
  <h1>Kobo simulator</h1>
  <span class="badge" id="profile-badge">clara-bw-391</span>
  <p>Shared renderer, touch transform and refresh planner</p>
  <button class="primary" type="button" id="refresh">Refresh frame</button>
</header>
<main>
<div class="workspace">
  <figure class="device">
    <div class="screen"><canvas id="display" width="1072" height="1448" tabindex="0" role="application" aria-label="Kobo grayscale display" aria-describedby="instructions"></canvas></div>
    <figcaption id="instructions">Kobo panel preview. Click or tap to exercise the selected profile's measured controller transform and SDK hit testing.</figcaption>
  </figure>
  <aside class="inspector" aria-label="Simulator inspector">
    <section class="card">
      <h2>Session</h2>
      <p class="status" id="status" aria-live="polite">Loading frame.</p>
      <label class="control" for="scenario">Deterministic scenario
        <select id="scenario">
          <option value="normal">Normal</option>
          <option value="offline">Offline</option>
          <option value="low-battery">Low battery</option>
          <option value="permission-denied">Permission denied</option>
          <option value="missing-secret">Missing secret</option>
          <option value="network-timeout">Network timeout</option>
          <option value="storage-full">Storage full</option>
          <option value="cache-pressure">Image cache pressure</option>
        </select>
      </label>
      <div class="buttons">
        <button type="button" data-lifecycle="background">Background</button>
        <button type="button" data-lifecycle="foreground">Foreground</button>
      </div>
    </section>
    <section class="card">
      <h2>Panel transition</h2>
      <dl class="facts">
        <dt>Waveform</dt><dd id="waveform">—</dd>
        <dt>Update</dt><dd id="update-kind">—</dd>
        <dt>Changed region</dt><dd id="region">—</dd>
        <dt>Refresh count</dt><dd id="refresh-count">0</dd>
        <dt>Since clean</dt><dd id="partial-count">0 / 8</dd>
      </dl>
      <label class="check"><input type="checkbox" id="ideal"> Show ideal pixels</label>
      <label class="check"><input type="checkbox" id="refresh-region" checked> Outline refresh region</label>
      <p class="note">Residue is an explicit visual approximation. Pixel output and refresh selection are exact.</p>
    </section>
    <section class="card">
      <h2>Clara BW profile</h2>
      <dl class="facts">
        <dt>Panel</dt><dd id="geometry">1072 × 1448</dd>
        <dt>Density</dt><dd id="density">300 PPI</dd>
        <dt>Framebuffer rotation</dt><dd id="rotation">3</dd>
        <dt>Lifecycle</dt><dd id="lifecycle">foreground</dd>
        <dt>Display touch</dt><dd id="display-touch">—</dd>
        <dt>Raw touch</dt><dd id="raw-touch">—</dd>
      </dl>
    </section>
    <section class="card">
      <h2>Layout diagnostics</h2>
      <label class="check"><input type="checkbox" id="overlay" checked> Show diagnostic outlines</label>
      <ul class="diagnostics" id="diagnostics"><li>Checking screen…</li></ul>
    </section>
  </aside>
</div>
</main>
<script>
const canvas=document.getElementById("display"), ctx=canvas.getContext("2d",{alpha:false});
const status=document.getElementById("status"),list=document.getElementById("diagnostics"),overlay=document.getElementById("overlay");
const ideal=document.getElementById("ideal"),refreshRegion=document.getElementById("refresh-region"),scenario=document.getElementById("scenario");
let point={x:536,y:177},issues=[],profile={width:1072,height:1448},transition=null,lastFlash=0;
function checked(response){if(!response.ok)throw Error("Simulator request failed ("+response.status+")");return response;}
function showDiagnostics(){list.replaceChildren();if(!issues.length){const item=document.createElement("li");item.textContent="No layout issues.";list.append(item);return;}for(const issue of issues){const item=document.createElement("li");item.className=issue.severity;item.textContent=issue.message;list.append(item);}}
function outline(rect,color,width){if(!rect)return;ctx.save();ctx.lineWidth=width;ctx.strokeStyle=color;ctx.strokeRect(rect.x+width/2,rect.y+width/2,Math.max(0,rect.width-width),Math.max(0,rect.height-width));ctx.restore();}
function drawOverlays(){if(refreshRegion.checked&&transition)for(const update of transition.regions)outline(update.region,"#006fbb",6);if(!overlay.checked)return;for(const issue of issues){outline(issue.rect,issue.severity==="error"?"#d00000":"#b56a00",5);}}
function showSimulation(sim){profile=sim.profile;transition=sim.transition;scenario.value=sim.scenario;document.getElementById("profile-badge").textContent=profile.id;document.getElementById("geometry").textContent=profile.width+" × "+profile.height;document.getElementById("density").textContent=profile.pixelsPerInch+" PPI";document.getElementById("rotation").textContent=profile.rotation;document.getElementById("lifecycle").textContent=sim.lifecycle;const touch=sim.touch;document.getElementById("display-touch").textContent=touch?touch.display.x+", "+touch.display.y:"—";document.getElementById("raw-touch").textContent=touch?touch.raw.x+", "+touch.raw.y:"—";document.getElementById("waveform").textContent=transition?transition.waveform:"—";document.getElementById("update-kind").textContent=transition?(transition.full?"full / cleaning":"partial"):"unchanged";document.getElementById("region").textContent=transition?(transition.regions.length===1?transition.region.width+"×"+transition.region.height+" @ "+transition.region.x+","+transition.region.y:transition.regions.length+" regions"):"—";document.getElementById("refresh-count").textContent=sim.refreshCount;document.getElementById("partial-count").textContent=sim.partialsSinceClean+" / 8";}
async function frame(){const path=ideal.checked?"/ideal-frame":"/frame";const response=checked(await fetch(path,{cache:"no-store"}));const raw=new Uint8Array(await response.arrayBuffer());const [diagnostics,simulation]=await Promise.all([fetch("/diagnostics",{cache:"no-store"}).then(checked).then(r=>r.json()),fetch("/simulation",{cache:"no-store"}).then(checked).then(r=>r.json())]);issues=diagnostics.issues;showSimulation(simulation);if(raw.length!==profile.width*profile.height)throw Error("Invalid "+profile.id+" frame");if(canvas.width!==profile.width||canvas.height!==profile.height){canvas.width=profile.width;canvas.height=profile.height;}const image=ctx.createImageData(profile.width,profile.height);for(let i=0;i<raw.length;i++){const p=i*4;image.data[p]=image.data[p+1]=image.data[p+2]=raw[i];image.data[p+3]=255;}ctx.putImageData(image,0,0);showDiagnostics();drawOverlays();if(!ideal.checked&&transition&&transition.full&&transition.refresh!==lastFlash){lastFlash=transition.refresh;canvas.classList.remove("clean-flash");void canvas.offsetWidth;canvas.classList.add("clean-flash");}status.textContent=issues.length?"Frame loaded with "+issues.length+" diagnostic"+(issues.length===1?"":"s")+".":"Frame loaded; layout clean.";}
function touchLocation(event){const rect=canvas.getBoundingClientRect();return{x:Math.floor((event.clientX-rect.left)*profile.width/rect.width),y:Math.floor((event.clientY-rect.top)*profile.height/rect.height)};}
async function touch(next){point=next;checked(await fetch("/touch",{method:"POST",headers:{"Content-Type":"text/plain"},body:"x="+point.x+"&y="+point.y}));await frame();status.textContent="Touch delivered through the selected profile transform.";}
async function post(path,body){checked(await fetch(path,{method:"POST",headers:{"Content-Type":"text/plain"},body}));await frame();}
canvas.addEventListener("pointerup",event=>{event.preventDefault();touch(touchLocation(event)).catch(error=>status.textContent=error.message);});
canvas.addEventListener("keydown",event=>{if(event.key==="Enter"||event.key===" "){event.preventDefault();touch(point).catch(error=>status.textContent=error.message);}});
document.getElementById("refresh").addEventListener("click",()=>frame().catch(error=>status.textContent=error.message));
for(const control of [overlay,ideal,refreshRegion])control.addEventListener("change",()=>frame().catch(error=>status.textContent=error.message));
scenario.addEventListener("change",()=>post("/scenario",scenario.value).catch(error=>status.textContent=error.message));
for(const button of document.querySelectorAll("[data-lifecycle]"))button.addEventListener("click",()=>post("/lifecycle",button.dataset.lifecycle).catch(error=>status.textContent=error.message));
frame().catch(error=>status.textContent=error.message);
</script>
</body></html>"##;

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn simulated_app_data_is_private_to_each_app() {
        let nonograms = simulated_data_root("nonograms");
        let panels = simulated_data_root("panels");
        assert_ne!(nonograms, panels);
        assert!(nonograms.ends_with("cobalt-sim-data/nonograms"));
        assert!(panels.ends_with("cobalt-sim-data/panels"));
    }
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;

    static NEXT_PRIVATE_DIR: AtomicUsize = AtomicUsize::new(0);

    fn app_result(
        state: &Arc<Mutex<AppState>>,
        caller: &str,
        scenario: Scenario,
        request: &kobo_protocol::DeviceRequest,
    ) -> kobo_protocol::DeviceResult {
        simulated_app_request(state, caller, scenario, request)
            .expect("simulate request")
            .expect("app-store request")
    }

    #[test]
    fn simulated_app_names_cannot_escape_their_data_root() {
        for name in ["../outside", "/absolute", "two/parts", "", "UPPER"] {
            assert!(!valid_app_name(name), "{name:?} was accepted");
        }
        assert!(valid_app_name("nonograms"));
    }

    #[test]
    fn simulated_store_updates_and_reinstalls_in_one_session() {
        use kobo_protocol::{DeviceRequest, DeviceResult};

        let apps = Arc::new(Mutex::new(SimulatedApps::default()));
        {
            let mut state = apps.lock().expect("simulated apps");
            let todo = state
                .catalog
                .iter_mut()
                .find(|entry| entry.id == "todo")
                .expect("Todo entry");
            todo.version = "1.1.0".to_owned();
        }
        let store = Arc::new(Mutex::new(AppState::with_apps(Arc::clone(&apps))));
        assert_eq!(
            app_result(
                &store,
                "store",
                Scenario::Normal,
                &DeviceRequest::InstallApp {
                    id: "todo".to_owned(),
                },
            ),
            DeviceResult::Done
        );
        assert_eq!(
            app_result(
                &store,
                "store",
                Scenario::Normal,
                &DeviceRequest::InstallApp {
                    id: "sudoku".to_owned(),
                },
            ),
            DeviceResult::Done
        );
        let launcher = Arc::new(Mutex::new(AppState::with_apps(apps)));
        let DeviceResult::Apps { entries } = app_result(
            &launcher,
            "launcher",
            Scenario::Normal,
            &DeviceRequest::ListInstalledApps,
        ) else {
            panic!("installed list");
        };
        assert_eq!(entries.len(), 12);
        assert_eq!(
            entries
                .iter()
                .find(|entry| entry.id == "todo")
                .and_then(|entry| entry.installed_version.as_deref()),
            Some("1.1.0")
        );
        assert!(entries.iter().any(|entry| entry.id == "sudoku"));
        assert_eq!(
            app_result(
                &store,
                "store",
                Scenario::Normal,
                &DeviceRequest::UninstallApp {
                    id: "sudoku".to_owned(),
                },
            ),
            DeviceResult::Done
        );
        let DeviceResult::Apps { entries } = app_result(
            &launcher,
            "launcher",
            Scenario::Normal,
            &DeviceRequest::ListInstalledApps,
        ) else {
            panic!("installed list");
        };
        assert_eq!(entries.len(), 11);
        assert!(!entries.iter().any(|entry| entry.id == "sudoku"));
        assert_eq!(
            app_result(
                &store,
                "store",
                Scenario::Normal,
                &DeviceRequest::InstallApp {
                    id: "sudoku".to_owned(),
                },
            ),
            DeviceResult::Done
        );
        let DeviceResult::Apps { entries } = app_result(
            &launcher,
            "launcher",
            Scenario::Normal,
            &DeviceRequest::ListInstalledApps,
        ) else {
            panic!("installed list");
        };
        assert_eq!(entries.len(), 12);
        assert!(entries.iter().any(|entry| entry.id == "sudoku"));
    }

    #[test]
    fn simulated_store_models_authorization_and_network_failures() {
        use kobo_protocol::{DenyReason, DeviceError, DeviceRequest, DeviceResult};

        let state = Arc::new(Mutex::new(AppState::default()));
        assert_eq!(
            app_result(
                &state,
                "hello",
                Scenario::Normal,
                &DeviceRequest::ReadAppCatalog,
            ),
            DeviceResult::Denied(DenyReason::NotDeclared)
        );
        assert_eq!(
            app_result(
                &state,
                "store",
                Scenario::Offline,
                &DeviceRequest::RefreshAppCatalog,
            ),
            DeviceResult::Failed(DeviceError::Unreachable)
        );
        assert_eq!(
            app_result(
                &state,
                "store",
                Scenario::NetworkTimeout,
                &DeviceRequest::RefreshAppCatalog,
            ),
            DeviceResult::Failed(DeviceError::TimedOut)
        );
        assert_eq!(
            app_result(
                &state,
                "store",
                Scenario::StorageFull,
                &DeviceRequest::InstallApp {
                    id: "sudoku".to_owned(),
                },
            ),
            DeviceResult::Failed(DeviceError::Backend)
        );
        assert_eq!(
            app_result(
                &state,
                "store",
                Scenario::Offline,
                &DeviceRequest::InstallApp {
                    id: "sudoku".to_owned(),
                },
            ),
            DeviceResult::Failed(DeviceError::Unreachable)
        );
        assert_eq!(
            app_result(
                &state,
                "store",
                Scenario::NetworkTimeout,
                &DeviceRequest::InstallApp {
                    id: "sudoku".to_owned(),
                },
            ),
            DeviceResult::Failed(DeviceError::TimedOut)
        );
    }

    #[test]
    fn simulated_catalog_contains_store_only_sudoku() {
        use kobo_protocol::{DeviceRequest, DeviceResult};

        let state = Arc::new(Mutex::new(AppState::default()));
        let DeviceResult::Apps { entries } = app_result(
            &state,
            "store",
            Scenario::Normal,
            &DeviceRequest::ReadAppCatalog,
        ) else {
            panic!("catalog");
        };
        let sudoku = entries
            .iter()
            .find(|entry| entry.id == "sudoku")
            .expect("Sudoku catalog entry");
        assert_eq!(sudoku.version, "1.0.0");
        assert!(!sudoku.is_installed());
    }

    #[test]
    fn simulated_platform_updates_remain_settings_only() {
        let update = kobo_protocol::DeviceRequest::Update {
            url: "https://example.test/Cobalt.tgz".to_owned(),
            sha256: "a".repeat(64),
        };
        assert!(simulated_platform_request_allowed("settings", &update));
        assert!(!simulated_platform_request_allowed("store", &update));
        assert!(simulated_platform_request_allowed(
            "store",
            &kobo_protocol::DeviceRequest::RefreshAppCatalog
        ));
    }

    #[test]
    fn a_task_that_finishes_while_nothing_is_typed_still_reaches_the_application() {
        // The message loop blocks on the application's socket, so a fetch
        // taking seconds used to be delivered only when the developer next
        // tapped something. Refusals arrived instantly, which is why nothing
        // noticed: the only tasks the simulator completed were refused ones.
        let (client, server) = UnixStream::pair().expect("a socket pair");
        let writer = AppWriter::spawn(server);
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
        let mut online = simulated_tasks("gallery");
        assert!(
            online
                .submit(
                    kobo_protocol::TaskId(1),
                    kobo_protocol::Task::Fetch {
                        url: "https://example.invalid/x".into(),
                        offset: 0,
                        max_bytes: 16,
                        credential: None,
                        headers: Vec::new(),
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
            (PROFILE.width * PROFILE.height) as usize
        );
        let button = simulator.screen().layout().nodes[2].rect;
        assert_eq!(
            simulator.touch(button.x + button.width / 2, button.y + button.height / 2),
            Some(ActionId(1))
        );
        assert_eq!(simulator.counter(), 1);
    }

    #[test]
    fn simulation_reports_the_clara_profile_panel_update_and_raw_touch() {
        let mut simulator = Simulator::new();
        let _ = simulator.frame();
        let button = simulator.screen().layout().nodes[2].rect;
        let x = button.x + button.width / 2;
        let y = button.y + button.height / 2;
        simulator.touch(x, y).expect("button is touchable");

        let payload = simulator.simulation_json();
        let raw = POSE
            .display_to_touch(
                u32::try_from(x).expect("positive display x"),
                u32::try_from(y).expect("positive display y"),
            )
            .expect("display point maps to the touch controller");
        assert!(payload.contains("\"id\":\"clara-bw-391\""));
        assert!(payload.contains("\"width\":1072"));
        assert!(payload.contains("\"height\":1448"));
        assert!(payload.contains("\"pixelsPerInch\":300"));
        assert!(payload.contains("\"waveform\":\"GC16\""));
        assert!(payload.contains(&format!("\"raw\":{{\"x\":{},\"y\":{}}}", raw.0, raw.1)));
        assert!(payload.contains("\"panelApproximation\":true"));
    }

    #[test]
    fn unchanged_frames_do_not_replay_the_previous_transition() {
        let mut simulator = Simulator::new();
        let _ = simulator.frame();
        let _ = simulator.frame();

        let payload = simulator.simulation_json();
        assert!(payload.contains("\"transition\":null"));
        assert!(payload.contains("\"refreshCount\":1"));
        assert!(payload.contains("\"partialsSinceClean\":0"));
    }

    #[test]
    fn scenarios_inject_only_the_failures_they_name() {
        let fetch = kobo_protocol::Task::Fetch {
            url: "https://example.invalid/data".into(),
            offset: 0,
            max_bytes: 32,
            credential: None,
            headers: Vec::new(),
        };
        let local = kobo_protocol::Task::ReadFile {
            path: "notes.txt".into(),
        };
        let credentialed_post = kobo_protocol::Task::Post {
            url: "https://example.invalid/data".into(),
            body: "{}".into(),
            content_type: "application/json".into(),
            credential: Some(kobo_protocol::Credential::bearer("api-key")),
            headers: Vec::new(),
            max_bytes: 32,
        };

        assert_eq!(
            scenario_task_error(Scenario::Offline, &fetch),
            Some(kobo_protocol::TaskError::Offline)
        );
        assert_eq!(scenario_task_error(Scenario::Offline, &local), None);
        // The other half of the same problem, which an app has to answer
        // differently: the reader is fine, the host is not.
        assert_eq!(
            scenario_task_error(Scenario::HostDown, &fetch),
            Some(kobo_protocol::TaskError::Unreachable)
        );
        assert_eq!(scenario_task_error(Scenario::HostDown, &local), None);
        assert_eq!(
            scenario_task_error(Scenario::PermissionDenied, &fetch),
            Some(kobo_protocol::TaskError::Denied)
        );
        assert_eq!(
            scenario_task_error(Scenario::NetworkTimeout, &fetch),
            Some(kobo_protocol::TaskError::TimedOut)
        );
        assert_eq!(
            scenario_task_error(Scenario::MissingSecret, &credentialed_post),
            Some(kobo_protocol::TaskError::NotFound)
        );
        assert_eq!(scenario_task_error(Scenario::Normal, &fetch), None);
    }

    #[test]
    fn scenario_and_lifecycle_inputs_are_closed_sets() {
        for scenario in Scenario::ALL {
            assert_eq!(Scenario::parse(scenario.name().as_bytes()), Some(scenario));
        }
        assert_eq!(Scenario::parse(b"surprise"), None);
        assert_eq!(parse_lifecycle(b"foreground"), Some(Lifecycle::Foreground));
        assert_eq!(parse_lifecycle(b"background"), Some(Lifecycle::Background));
        assert_eq!(parse_lifecycle(b"suspended"), None);
    }

    #[test]
    fn leaving_cache_pressure_restores_the_normal_picture_cache() {
        let handle = kobo_ui::PictureHandle(7);
        let mut state = AppState::default();
        assert!(state.pictures.put(handle, 1, 1, vec![kobo_ui::tone::INK]));

        state.scenario = Scenario::CachePressure;
        assert!(!kobo_ui::Pictures::contains(
            state.active_pictures(),
            handle
        ));
        assert!(state
            .pressure_pictures
            .put(handle, 1, 1, vec![kobo_ui::tone::PAPER]));

        state.scenario = Scenario::Normal;
        let restored = kobo_ui::Pictures::get(state.active_pictures(), handle)
            .expect("normal cache survived the scenario");
        assert_eq!(restored.grey, &[kobo_ui::tone::INK]);
    }

    #[test]
    fn lifecycle_control_sends_the_real_sdk_event() {
        let (mut client, server) = UnixStream::pair().expect("socket pair");
        let session = AppSession {
            state: Arc::new(Mutex::new(AppState::default())),
            writer: AppWriter::spawn(server),
        };

        session
            .send_lifecycle(Lifecycle::Background)
            .expect("send lifecycle");
        let received = read_protocol_frame(&mut client).expect("read lifecycle");
        assert_eq!(received.message, Message::Lifecycle(Lifecycle::Background));
        assert_eq!(
            session.state.lock().expect("state lock").lifecycle,
            Lifecycle::Background
        );
    }

    #[test]
    fn diagnostics_endpoint_payload_names_layout_failures() {
        let screen = Screen::new(
            1,
            (0..80)
                .map(|index| Node::Text {
                    id: NodeId(index + 1),
                    text: "One visible line".into(),
                    links: Vec::new(),
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

    /// A dropped connection is the commonest thing a listener on a developer's
    /// own machine sees, and it used to end the session: the browser preconnect
    /// that is never used, whatever is watching for open ports, a reload
    /// abandoned before the request went out. Each one arrived as an
    /// `UnexpectedEof` from the first read and took the simulator, and the
    /// application running behind it, down with it.
    #[test]
    fn a_connection_that_sends_no_request_does_not_end_the_session() {
        let mut server = Server::bind_address("127.0.0.1:0").expect("bind simulator");
        let address = server.local_addr().expect("simulator address");
        thread::spawn(move || server.serve());

        TcpStream::connect(address)
            .expect("open a connection")
            .shutdown(std::net::Shutdown::Both)
            .expect("close it again unused");

        let mut stream = TcpStream::connect(address).expect("ask for the page afterwards");
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .expect("send the request");
        let mut answer = [0_u8; 15];
        stream.read_exact(&mut answer).expect("read the answer");
        assert_eq!(&answer, b"HTTP/1.1 200 OK");
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
                        name: "test-app".into(),
                    },
                },
            )?;
            let welcome = read_protocol_frame(&mut stream)?;
            assert_eq!(welcome.request_id, 7);
            assert_eq!(
                welcome.message,
                Message::Welcome {
                    width: u16::try_from(PROFILE.width).expect("profile width"),
                    height: u16::try_from(PROFILE.height).expect("profile height"),
                    pixels_per_inch: PROFILE.pixels_per_inch,
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
                            state: kobo_ui::ControlState::Enabled,
                            emphasis: kobo_ui::Emphasis::Normal,
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

    #[test]
    fn each_simulated_app_session_starts_in_portrait() {
        assert_eq!(
            AppState::default().orientation,
            kobo_ui::Orientation::Portrait
        );
    }
}

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
mod activity;
mod capture;
mod clock;
mod hardware;
mod input;
mod panel;
pub use capture::CaptureSource;
use panel::PanelPreview;
#[path = "../../kobod/src/frame.rs"]
mod frame;

use kobo_policy::{shelf::Shelf, store::Store, DeviceServices, TaskRunner};
use kobo_profile::{DeviceProfile, PanelPose, CLARA_BW_391, SUPPORTED_PROFILES};
use kobo_protocol::{read_from, write_to, Frame, Lifecycle, Message};
use kobo_ui::{ActionId, DisplayMetrics, Node, NodeId, Screen, Surface};

const MAX_HTTP_HEADER: usize = 8 * 1024;

static OBSERVATION: LazyLock<Result<Option<kobo_profile::observation::Observation>, String>> =
    LazyLock::new(|| {
        let Some(path) = std::env::var_os("KOBO_SIM_OBSERVATION") else {
            return Ok(None);
        };
        let mut source = String::new();
        fs::File::open(path)
            .and_then(|file| {
                file.take(kobo_profile::observation::MAX_BYTES as u64 + 1)
                    .read_to_string(&mut source)
            })
            .map_err(|error| format!("read hardware observation: {error}"))?;
        kobo_profile::observation::Observation::parse(&source).map(Some)
    });

fn configured_profile() -> io::Result<&'static DeviceProfile> {
    let requested = match std::env::var("KOBO_SIM_PROFILE") {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(error) => return Err(io::Error::new(io::ErrorKind::InvalidInput, error)),
    };
    profile_from_observation(requested.as_deref(), observation()?)
}

fn observation() -> io::Result<Option<&'static kobo_profile::observation::Observation>> {
    OBSERVATION
        .as_ref()
        .map(Option::as_ref)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.clone()))
}

fn profile_from_observation(
    requested: Option<&str>,
    observation: Option<&kobo_profile::observation::Observation>,
) -> io::Result<&'static DeviceProfile> {
    let Some(observation) = observation else {
        return parse_profile(requested);
    };
    let profile = kobo_profile::identify_profile(&observation.snapshot).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "observation does not match a supported Cobalt profile",
        )
    })?;
    if requested.is_some_and(|id| id != profile.id) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "requested profile does not match the hardware observation",
        ));
    }
    if profile.validate(&observation.snapshot).readiness == kobo_profile::Readiness::Rejected {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "hardware observation geometry does not match its profile",
        ));
    }
    let framebuffer = observation.snapshot.framebuffer.as_ref().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "observation has no framebuffer",
        )
    })?;
    PanelPose::resolve(profile, framebuffer)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;
    Ok(profile)
}
fn parse_profile(requested: Option<&str>) -> io::Result<&'static DeviceProfile> {
    match requested {
        None => Ok(&CLARA_BW_391),
        Some(id) => {
            SUPPORTED_PROFILES
                .iter()
                .copied()
                .find(|profile| profile.id == id)
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput,
                format!("Unknown simulator profile {id:?}. Use a profile ID from kobo profiles."))
                })
        }
    }
}

fn validate_configuration() -> io::Result<()> {
    configured_profile()?;
    match std::env::var("KOBO_TEXT_SCALE") {
        Ok(value) => validate_scale(&value)?,
        Err(std::env::VarError::NotPresent) => {}
        Err(error) => return Err(io::Error::new(io::ErrorKind::InvalidInput, error)),
    }
    configured_backends()?;
    Ok(())
}

fn parse_backends(value: Option<&str>) -> io::Result<kobo_policy::Declared> {
    // These are the modeled services from Cobalt's existing runtime. Optional
    // audio, Bluetooth and power ownership need an explicit fixture. This is
    // a conservative development model, not a hardware observation.
    let value = value
        .unwrap_or("network,battery-read,frontlight-control,wifi-control,cover-sensor,library");
    kobo_policy::Declared::parse(value.split(',').filter(|name| !name.is_empty()))
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))
}

fn configured_backends() -> io::Result<kobo_policy::Declared> {
    match std::env::var("KOBO_SIM_BACKENDS") {
        Ok(value) => parse_backends(Some(&value)),
        Err(std::env::VarError::NotPresent) => match observation()? {
            Some(observation) => parse_backends(Some(&observation.available_backends.join(","))),
            None => parse_backends(None),
        },
        Err(error) => Err(io::Error::new(io::ErrorKind::InvalidInput, error)),
    }
}

fn validate_scale(value: &str) -> io::Result<()> {
    kobo_ui::TextScale::from_name(value).map(|_| ()).ok_or_else(||
        io::Error::new(io::ErrorKind::InvalidInput,
            format!("Unknown text size {value:?}. Use default, large, extra-large, or a supported percentage.")))
}

static PROFILE: LazyLock<&'static DeviceProfile> =
    LazyLock::new(|| configured_profile().expect("validate simulator profile before starting"));
/// The simulator asserts an orientation rather than observing one, because it
/// has no device. That is legitimate here and nowhere near a real framebuffer:
/// it means the browser exercises exactly the transform the selected profile
/// was measured at.
static POSE: LazyLock<PanelPose<'static>> = LazyLock::new(|| {
    observation().expect("validated observation").map_or_else(
        || PanelPose::reference(*PROFILE),
        |value| {
            PanelPose::resolve(
                *PROFILE,
                value
                    .snapshot
                    .framebuffer
                    .as_ref()
                    .expect("validated framebuffer"),
            )
            .expect("validated observation pose")
        },
    )
});

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

fn physical_rect(orientation: kobo_ui::Orientation, rect: kobo_ui::Rect) -> kobo_ui::Rect {
    match orientation {
        kobo_ui::Orientation::Portrait => rect,
        kobo_ui::Orientation::Landscape => kobo_ui::Rect {
            x: profile_metrics()
                .width
                .saturating_sub(rect.y)
                .saturating_sub(rect.height),
            y: rect.x,
            width: rect.height,
            height: rect.width,
        },
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
        self.panel.frame(ideal).to_vec()
    }

    fn capture(&self, ideal: bool, rgb: bool) -> io::Result<Vec<u8>> {
        let colour = rgb.then(|| self.panel.ideal_rgb(PROFILE.colour_panel));
        capture::View {
            app: "counter",
            mode: "counter-demo",
            screen: self.screen(),
            source: &CaptureSource::default(),
            paints: u64::from(self.counter),
            orientation: kobo_ui::Orientation::Portrait,
            simulation: self.simulation_json(),
            frame: colour.as_deref().unwrap_or_else(|| self.panel.frame(ideal)),
            ideal,
            rgb,
        }
        .pack()
    }

    fn commit_frame(&mut self) {
        let mut surface = Surface::new(PROFILE.width as usize, PROFILE.height as usize);
        kobo_ui::render_with(
            &self.screen,
            &profile_metrics(),
            &kobo_ui::Chrome::default(),
            &mut surface,
            None,
        );
        self.panel.update(&surface);
    }

    pub fn touch(&mut self, x: i32, y: i32) -> Option<ActionId> {
        let (display_x, display_y) = (u32::try_from(x).ok()?, u32::try_from(y).ok()?);
        let raw = POSE.display_to_touch(display_x, display_y)?;
        let display = POSE.touch_to_display(raw.0, raw.1)?;
        self.last_touch = Some(SimulatedTouch { display, raw });
        let action = self
            .screen
            .layout_with(&profile_metrics(), &kobo_ui::Chrome::default())
            .hit_test(
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
        self.commit_frame();
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
        validate_configuration()?;
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
            ("GET", "/capture" | "/ideal-capture" | "/colour-capture") => {
                let ideal = request.path != "/capture";
                let body = self
                    .simulator
                    .capture(ideal, request.path == "/colour-capture")?;
                write_response(&mut stream, 200, "application/octet-stream", &body)
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
                let body = layout_json(self.simulator.screen(), 0, kobo_ui::Orientation::Portrait);
                write_response(
                    &mut stream,
                    200,
                    "application/json; charset=utf-8",
                    body.as_bytes(),
                )
            }
            ("GET", "/diagnostics") => {
                let body = diagnostics_json(
                    self.simulator.screen(),
                    &kobo_ui::PictureCache::default(),
                    kobo_ui::Orientation::Portrait,
                );
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
    manifest: Option<kobo_catalog::App>,
    capture_source: CaptureSource,
    time: clock::Time,
}

impl AppServer {
    /// Creates a localhost HTTP listener and a new Unix socket listener.
    ///
    /// # Errors
    ///
    /// Returns an error for a non-loopback HTTP address, an existing socket
    /// path, an unsafe socket parent, or a listener binding failure.
    pub fn bind(address: &str, socket_path: impl AsRef<Path>) -> io::Result<Self> {
        validate_configuration()?;
        let time = clock::Time::configured()?;
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
            manifest: None,
            capture_source: CaptureSource::default(),
            time,
        })
    }

    /// Uses the local publishing manifest, including its capability declaration.
    /// The SDK Hello must match its identity; an unrelated bundled app cannot
    /// supply permissions for this process.
    ///
    /// # Errors
    /// Returns invalid-data for malformed or oversized metadata.
    pub fn with_manifest(mut self, source: &str) -> io::Result<Self> {
        if source.len() > 64 * 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "app manifest exceeds 64 KiB",
            ));
        }
        self.manifest = Some(
            kobo_catalog::App::parse(source)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?,
        );
        Ok(self)
    }

    /// Attach build provenance to captures without changing the app's grants.
    ///
    /// # Errors
    /// Refuses malformed hashes and oversized or control-bearing labels.
    pub fn with_capture_source(mut self, source: CaptureSource) -> io::Result<Self> {
        if !source.valid() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid capture source",
            ));
        }
        self.capture_source = source;
        Ok(self)
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
        let declared = session_declaration(&name, self.manifest.as_ref())?;
        write_protocol_frame(
            stream,
            &Frame {
                version: hello.version,
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
        let mut initial = AppState::with_apps(Arc::clone(&self.apps));
        initial.services = DeviceServices::new(
            declared.clone(),
            kobo_policy::PowerPolicy::DEFAULT,
            kobo_policy::Backends::with(configured_backends()?.iter()),
        );
        initial.hardware.magnet_present =
            matches!(std::env::var("KOBO_MAGNET").as_deref(), Ok("1" | "present"));
        initial.observe_hardware();
        initial.app_name.clone_from(&name);
        initial.capture_source = self.capture_source.clone();
        initial.time = self.time.clone();
        initial.clock_snapshot = initial.time.now()?;
        let mut runner = simulated_tasks(&name, &declared);
        if let Some(clock) = initial.time.manual() {
            runner = runner.with_manual_clock(clock);
        }
        let tasks = Arc::new(Mutex::new(runner));
        initial.tasks = Some(Arc::clone(&tasks));
        let state = Arc::new(Mutex::new(initial));
        let reader_state = Arc::clone(&state);
        // One writer for the whole session, shared by every thread that has
        // something to say to the application: taps from the browser, replies
        // to requests, and terminal output arriving on its own. Frames are
        // length-prefixed, so two of them written at once do not make two
        // frames, they make one unreadable stream.
        let writer = AppWriter::spawn_for(stream.try_clone()?, hello.version);
        let reader_writer = Arc::clone(&writer);
        thread::spawn(move || {
            // A malformed frame ends the session rather than being skipped,
            // and the developer is told which one: a reader that dies quietly
            // leaves an application talking to nobody, with a panel that keeps
            // showing the last good screen and ignores every tap.
            if let Err(error) = read_app_messages(
                reader,
                &name,
                &declared,
                &reader_writer,
                &reader_state,
                &tasks,
            ) {
                eprintln!("the application's connection ended: {error}");
            }
            reader_writer.close();
            if let Ok(mut tasks) = tasks.lock() {
                tasks.shutdown();
            }
            drop(tasks);
            if let Ok(mut activity) = reader_writer.activity.lock() {
                activity.finish_disconnect();
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
                format,
                pixels,
            } => picture_result(
                handle,
                pictures.put_report_with(handle, width, height, format, pixels),
            ),
            Message::BeginPicture {
                handle,
                width,
                height,
                format,
            } => (!pictures.begin_upload_with(handle, width, height, format))
                .then(|| format!("picture {} upload refused", handle.0)),
            Message::PictureChunk {
                handle,
                offset,
                pixels,
            } => (!pictures.upload_chunk(
                handle,
                usize::try_from(offset).unwrap_or(usize::MAX),
                &pixels,
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
    app_name: String,
    capture_source: CaptureSource,
    time: clock::Time,
    hardware: kobo_policy::DeviceState,
    input: input::Replay,
    services: DeviceServices,
    clock_snapshot: kobo_policy::clock::Snapshot,
    tasks: Option<Arc<Mutex<TaskRunner>>>,
    secret_directory: PathBuf,
    chrome: kobo_ui::Chrome,
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
        let time = clock::Time::default();
        let clock_snapshot = time.now().expect("host clock must be in 1970..9999");
        Self {
            screen: Screen::new(0, Vec::new()),
            app_name: "app".into(),
            capture_source: CaptureSource::default(),
            time,
            hardware: kobo_policy::DeviceState::default(),
            input: input::Replay::new(&POSE),
            services: DeviceServices::new(
                kobo_policy::Declared::parse([]).expect("no grants"),
                kobo_policy::PowerPolicy::DEFAULT,
                kobo_policy::Backends::none(),
            ),
            clock_snapshot,
            tasks: None,
            secret_directory: std::env::temp_dir().join(SIM_SECRETS),
            chrome: kobo_ui::Chrome::default(),
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
    fn default() -> Self {
        let mut catalog = kobo_catalog::bundled()
            .expect("validated bundled app metadata")
            .into_iter()
            .map(|entry| {
                let wanted = entry.glyph.replace('-', "");
                let glyph = kobo_ui::Glyph::ALL
                    .into_iter()
                    .find(|glyph| format!("{glyph:?}").eq_ignore_ascii_case(&wanted))
                    .unwrap_or(kobo_ui::Glyph::App);
                // Keep one catalog app available for an install/reinstall journey.
                let installed_version = (entry.id != "sudoku").then(|| entry.version.clone());
                kobo_protocol::AppInfo {
                    id: entry.id,
                    title: entry.title,
                    label: entry.label,
                    summary: entry.summary,
                    version: entry.version.clone(),
                    minimum_cobalt_version: env!("CARGO_PKG_VERSION").into(),
                    glyph,
                    capabilities: entry.capabilities,
                    installed_version,
                }
            })
            .collect::<Vec<_>>();
        catalog.extend([
            simulated_app(
                "settings",
                "Settings",
                "Settings",
                "Reader settings",
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
                "terminal",
                "Terminal",
                "Terminal",
                "Reader terminal",
                kobo_ui::Glyph::Terminal,
                &["shell"],
                true,
            ),
        ]);
        Self { catalog }
    }
}

fn app_declaration(name: &str) -> kobo_policy::Declared {
    let catalog = SimulatedApps::default();
    let names = catalog
        .catalog
        .iter()
        .find(|app| app.id == name)
        .map(|app| {
            app.capabilities
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    kobo_policy::Declared::parse(names).expect("validated capabilities")
}

fn session_declaration(
    name: &str,
    manifest: Option<&kobo_catalog::App>,
) -> io::Result<kobo_policy::Declared> {
    let Some(manifest) = manifest else {
        return Ok(app_declaration(name));
    };
    if manifest.id != name {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "SDK identity {name:?} does not match manifest {:?}",
                manifest.id
            ),
        ));
    }
    kobo_policy::Declared::parse(manifest.capabilities.iter().map(String::as_str))
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
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
    fn simulation_json(&self) -> String {
        let base = simulation_json(&self.panel, self.scenario, self.lifecycle, self.last_touch);
        let mut value = kobo_json::parse(&base).expect("simulator JSON");
        if let kobo_json::Value::Object(fields) = &mut value {
            fields.push(("clock".into(), self.time.json(self.clock_snapshot)));
            fields.push(("input".into(), self.input.json()));
            fields.push((
                "hardware".into(),
                hardware::json(self.effective_hardware(), self.orientation),
            ));
        }
        value.to_json()
    }

    fn input_event(&mut self, event: kobo_hal::TouchEvent, held: bool) -> Option<Message> {
        let kobo_hal::TouchEvent::Up { x, y } = event else {
            return None;
        };
        let raw = POSE.display_to_touch(x, y)?;
        self.last_touch = Some(SimulatedTouch {
            display: (x, y),
            raw,
        });
        let physical = profile_metrics();
        let (x, y) = kobo_ui::logical_point(
            self.orientation,
            physical.width,
            i32::try_from(x).ok()?,
            i32::try_from(y).ok()?,
        );
        let layout = self
            .screen
            .layout_with(&physical.oriented(self.orientation), &self.chrome);
        if held {
            if let Some((action, hit)) = layout.hold.zip(layout.hit_text(x, y)) {
                return Some(Message::TextHold {
                    action,
                    context: hit.context,
                    start: hit.start,
                    end: hit.end,
                });
            }
        }
        let action = if held {
            layout.hit_hold(x, y).or_else(|| layout.hit_test(x, y))
        } else {
            layout.hit_test(x, y)
        }?;
        Some(Message::Action { action })
    }

    fn effective_hardware(&self) -> kobo_policy::DeviceState {
        kobo_policy::DeviceState {
            battery_percent: if self.scenario == Scenario::LowBattery {
                5
            } else {
                self.hardware.battery_percent
            },
            ..self.hardware
        }
    }

    fn observe_hardware(&mut self) {
        let observed = self.effective_hardware();
        self.services
            .observe_battery(observed.battery_percent, observed.charging);
        self.services
            .observe_frontlight(observed.frontlight_percent);
        self.services.set_magnet(observed.magnet_present);
    }

    fn update_chrome(&mut self) {
        if let Ok(snapshot) = self.time.now() {
            self.clock_snapshot = snapshot;
        }
        let status = kobo_ui::Status {
            clock: self.clock_snapshot.hour_minute().map_or_else(
                || "--:--".into(),
                |(hour, minute)| format!("{hour:02}:{minute:02}"),
            ),
            signal: if self.scenario == Scenario::Offline {
                kobo_ui::Signal::Off
            } else {
                kobo_ui::Signal::Strong
            },
            battery: Some(kobo_ui::Percent::new(
                self.effective_hardware().battery_percent,
            )),
            charging: self.hardware.charging,
            bluetooth: true,
        };
        self.chrome =
            kobo_ui::Chrome::for_screen(&self.screen, self.app_name == "launcher", Some(status));
    }

    fn record(&mut self, mut message: String) {
        if let Some((end, _)) = message.char_indices().nth(4096) {
            message.truncate(end);
        }
        self.logs.push(message);
        if self.logs.len() > 64 {
            self.logs.remove(0);
        }
    }

    fn set_screen(&mut self, screen: Screen) {
        self.screen = screen;
        self.update_chrome();
        self.screen = kobo_ui::ensure_way_back(self.screen.clone(), &self.chrome, &self.app_name);
        self.paints = self.paints.saturating_add(1);
        self.record(format!("screen: {} paint: {}", self.screen.id, self.paints));
        self.commit_frame();
    }

    fn commit_frame(&mut self) {
        let mut surface = Surface::new(PROFILE.width as usize, PROFILE.height as usize);
        kobo_ui::render_oriented(
            &self.screen,
            &profile_metrics(),
            &self.chrome,
            self.active_pictures(),
            &mut surface,
            None,
            self.orientation,
        );
        let before = self.panel.planner.refreshes();
        self.panel.update(&surface);
        if let Some(transition) = self
            .panel
            .last
            .as_ref()
            .filter(|transition| transition.refresh != before)
        {
            self.record(format!(
                "refresh: {} waveform: {} full: {}",
                transition.refresh,
                transition.waveform.name(),
                transition.full
            ));
        }
    }

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
        self.state
            .lock()
            .map_err(|_| io::Error::other("app state lock poisoned"))?
            .record(format!("action: {}", action.0));
        write_shared(
            &self.writer,
            &Frame {
                version: kobo_protocol::VERSION,
                request_id: 0,
                message: Message::Action { action },
            },
        )
    }

    fn cancel_tasks(&self) -> io::Result<()> {
        if !self.writer.connected() {
            return Err(io::Error::other("the application is disconnected"));
        }
        let tasks = self
            .state
            .lock()
            .map_err(|_| io::Error::other("app state lock poisoned"))?
            .tasks
            .clone()
            .ok_or_else(|| io::Error::other("no task runner in this session"))?;
        tasks
            .lock()
            .map_err(|_| io::Error::other("task lock poisoned"))?
            .cancel_all();
        deliver_task_outcomes(&tasks, &self.writer, &self.state)
    }

    fn replay_input(&self, command: &str) -> io::Result<()> {
        if !self.writer.connected() {
            return Err(io::Error::other("the application is disconnected"));
        }
        let (kind, source) = command
            .split_once(char::is_whitespace)
            .unwrap_or((command, ""));
        let messages = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| io::Error::other("app state lock poisoned"))?;
            if !state.panel.accepts_input() {
                return Err(io::Error::other(
                    "panel must be ready before replaying input",
                ));
            }
            let millis = state.time.now()?.monotonic_millis;
            let mut messages = Vec::new();
            match kind {
                "touch" => {
                    let events = state.input.touch(source, &POSE, millis)?;
                    for (event, held) in events {
                        if let Some(message) = state.input_event(event, held) {
                            messages.push(message);
                        }
                    }
                }
                "tap" => {
                    let (x, y) = parse_touch(source.as_bytes())
                        .ok_or_else(|| io::Error::other("invalid tap coordinates"))?;
                    let events = state.input.tap(
                        u32::try_from(x).map_err(io::Error::other)?,
                        u32::try_from(y).map_err(io::Error::other)?,
                        &POSE,
                        millis,
                    )?;
                    for (event, held) in events {
                        if let Some(message) = state.input_event(event, held) {
                            messages.push(message);
                        }
                    }
                }
                "gpio" => {
                    if let Some(forward) = state.input.gpio(source)? {
                        let layout = state.screen.layout_with(
                            &profile_metrics().oriented(state.orientation),
                            &state.chrome,
                        );
                        match layout.page_turns {
                            kobo_ui::PagingState::Declared(turns) => {
                                messages.push(Message::Action {
                                    action: if forward { turns.next } else { turns.previous },
                                });
                            }
                            kobo_ui::PagingState::None => {
                                messages.push(Message::PageTurn { forward });
                            }
                            kobo_ui::PagingState::SuppressedByOverlay => {}
                        }
                    }
                }
                "resync" => state.input.resynchronize(source)?,
                _ => return Err(io::Error::other("input expects touch, gpio or resync")),
            }
            state.record(format!("input: {command}"));
            if state.lifecycle == Lifecycle::Background {
                messages.clear();
            }
            messages
        };
        for message in messages {
            write_shared(
                &self.writer,
                &Frame {
                    version: kobo_protocol::VERSION,
                    request_id: 0,
                    message,
                },
            )?;
        }
        Ok(())
    }

    fn change_hardware(&self, command: &str) -> io::Result<()> {
        let change = hardware::Change::parse(command).ok_or_else(|| io::Error::other(
            "device expects battery PERCENT charging|unplugged, frontlight PERCENT, cover open|closed, or orientation portrait|landscape"))?;
        let event = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| io::Error::other("app state lock poisoned"))?;
            let before = state.hardware;
            match change {
                hardware::Change::Battery { percent, charging } => {
                    state.hardware.battery_percent = percent;
                    state.hardware.charging = charging;
                }
                hardware::Change::Frontlight(percent) => {
                    state.hardware.frontlight_percent = percent;
                }
                hardware::Change::Cover(present) => state.hardware.magnet_present = present,
                hardware::Change::Orientation(orientation) => state.orientation = orientation,
            }
            state.observe_hardware();
            state.update_chrome();
            state.commit_frame();
            state.record(format!("device observation: {command}"));
            (before.magnet_present != state.hardware.magnet_present
                && state.lifecycle == Lifecycle::Foreground
                && configured_backends()?.holds(kobo_policy::Capability::CoverSensor))
            .then_some(Message::CoverChanged {
                magnet_present: state.hardware.magnet_present,
            })
        };
        if let Some(message) = event {
            write_shared(
                &self.writer,
                &Frame {
                    version: kobo_protocol::VERSION,
                    request_id: 0,
                    message,
                },
            )?;
        }
        Ok(())
    }

    fn change_clock(&self, command: &str) -> io::Result<()> {
        let tasks = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| io::Error::other("app state lock poisoned"))?;
            state.time.change(command)?;
            state.record(format!("clock: {command}"));
            state.update_chrome();
            state.commit_frame();
            state.tasks.clone()
        };
        if let Some(tasks) = tasks {
            deliver_task_outcomes(&tasks, &self.writer, &self.state)?;
        }
        Ok(())
    }

    fn send_lifecycle(&self, lifecycle: Lifecycle) -> io::Result<()> {
        write_shared(
            &self.writer,
            &Frame {
                version: kobo_protocol::VERSION,
                request_id: 0,
                message: Message::Lifecycle(lifecycle),
            },
        )?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("app state lock poisoned"))?;
        state.lifecycle = lifecycle;
        state.record(format!("lifecycle: {lifecycle:?}"));
        Ok(())
    }

    fn render_frame(&self, ideal: bool) -> Vec<u8> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .panel
            .frame(ideal)
            .to_vec()
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
        state.observe_hardware();
        if scenario == Scenario::CachePressure {
            state.pressure_pictures = kobo_ui::PictureCache::new(256 * 1024);
        }
        state.record(format!("scenario: {}", scenario.name()));
        state.update_chrome();
        state.commit_frame();
        Ok(())
    }

    #[cfg(test)]
    fn touch_action(&self, x: i32, y: i32) -> Option<ActionId> {
        let display = (u32::try_from(x).ok()?, u32::try_from(y).ok()?);
        let raw = POSE.display_to_touch(display.0, display.1)?;
        let mapped = POSE.touch_to_display(raw.0, raw.1)?;
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.panel.accepts_input() {
            return None;
        }
        state.last_touch = Some(SimulatedTouch {
            display: mapped,
            raw,
        });
        let physical = profile_metrics();
        let (x, y) = kobo_ui::logical_point(
            state.orientation,
            physical.width,
            i32::try_from(mapped.0).ok()?,
            i32::try_from(mapped.1).ok()?,
        );
        state
            .screen
            .layout_with(&physical.oriented(state.orientation), &state.chrome)
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
            ("GET", "/capture" | "/ideal-capture" | "/colour-capture") => {
                let body = {
                    let state = self
                        .state
                        .lock()
                        .map_err(|_| io::Error::other("app state lock poisoned"))?;
                    let ideal = request.path != "/capture";
                    let rgb = request.path == "/colour-capture";
                    let colour = rgb.then(|| state.panel.ideal_rgb(PROFILE.colour_panel));
                    capture::View {
                        app: &state.app_name,
                        mode: "single-app",
                        screen: &state.screen,
                        source: &state.capture_source,
                        paints: state.paints,
                        orientation: state.orientation,
                        simulation: state.simulation_json(),
                        frame: colour
                            .as_deref()
                            .unwrap_or_else(|| state.panel.frame(ideal)),
                        ideal,
                        rgb,
                    }
                    .pack()?
                };
                write_response(&mut stream, 200, "application/octet-stream", &body)
            }
            ("GET", "/activity") => {
                let body = self
                    .writer
                    .activity
                    .lock()
                    .map_err(|_| io::Error::other("simulator activity lock poisoned"))?
                    .json();
                write_response(
                    &mut stream,
                    200,
                    "application/json; charset=utf-8",
                    body.as_bytes(),
                )
            }
            ("GET", "/simulation") => {
                let body = {
                    let state = self
                        .state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    state.simulation_json()
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
                    layout_json_with_chrome(
                        &state.screen,
                        state.paints,
                        state.orientation,
                        &state.chrome,
                    )
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
                    diagnostics_json_with_chrome(
                        &state.screen,
                        state.active_pictures(),
                        state.orientation,
                        &state.chrome,
                    )
                };
                write_response(
                    &mut stream,
                    200,
                    "application/json; charset=utf-8",
                    body.as_bytes(),
                )
            }
            ("POST", "/tasks") if request.body == b"cancel" => match self.cancel_tasks() {
                Ok(()) => write_response(&mut stream, 200, "text/plain", b"cancellation requested"),
                Err(error) => {
                    write_response(&mut stream, 400, "text/plain", error.to_string().as_bytes())
                }
            },
            ("POST", "/session") if request.body == b"disconnect" => {
                self.writer.close();
                write_response(
                    &mut stream,
                    200,
                    "text/plain",
                    b"application connection closed",
                )
            }
            ("GET", "/input") => {
                let state = self
                    .state
                    .lock()
                    .map_err(|_| io::Error::other("app state lock poisoned"))?;
                write_response(
                    &mut stream,
                    200,
                    "application/json",
                    state.input.json().to_json().as_bytes(),
                )
            }
            ("POST", "/input") => {
                match self.replay_input(std::str::from_utf8(&request.body).unwrap_or("")) {
                    Ok(()) => write_response(&mut stream, 200, "text/plain", b"ok"),
                    Err(error) => {
                        write_response(&mut stream, 400, "text/plain", error.to_string().as_bytes())
                    }
                }
            }
            ("GET", "/panel") => {
                let state = self
                    .state
                    .lock()
                    .map_err(|_| io::Error::other("app state lock poisoned"))?;
                write_response(
                    &mut stream,
                    200,
                    "application/json",
                    state.panel.state_json().to_json().as_bytes(),
                )
            }
            ("POST", "/panel") => {
                let command = std::str::from_utf8(&request.body).unwrap_or("");
                let mut state = self
                    .state
                    .lock()
                    .map_err(|_| io::Error::other("app state lock poisoned"))?;
                match state.panel.control(command) {
                    Ok(()) => {
                        state.record(format!("panel: {command}"));
                        write_response(&mut stream, 200, "text/plain", b"ok")
                    }
                    Err(error) => {
                        write_response(&mut stream, 400, "text/plain", error.to_string().as_bytes())
                    }
                }
            }
            ("GET", "/device") => {
                let state = self
                    .state
                    .lock()
                    .map_err(|_| io::Error::other("app state lock poisoned"))?;
                let body = hardware::json(state.effective_hardware(), state.orientation).to_json();
                write_response(&mut stream, 200, "application/json", body.as_bytes())
            }
            ("POST", "/device") => {
                match self.change_hardware(std::str::from_utf8(&request.body).unwrap_or("")) {
                    Ok(()) => write_response(&mut stream, 200, "text/plain", b"ok"),
                    Err(error) => {
                        write_response(&mut stream, 400, "text/plain", error.to_string().as_bytes())
                    }
                }
            }
            ("GET", "/clock") => {
                let state = self
                    .state
                    .lock()
                    .map_err(|_| io::Error::other("app state lock poisoned"))?;
                let body = state.time.json(state.time.now()?).to_json();
                write_response(&mut stream, 200, "application/json", body.as_bytes())
            }
            ("POST", "/clock") => {
                let command = std::str::from_utf8(&request.body).unwrap_or("");
                match self.change_clock(command) {
                    Ok(()) => write_response(&mut stream, 200, "text/plain", b"ok"),
                    Err(error) => {
                        write_response(&mut stream, 400, "text/plain", error.to_string().as_bytes())
                    }
                }
            }
            ("POST", "/touch") => {
                if !self
                    .state
                    .lock()
                    .map_err(|_| io::Error::other("app state lock poisoned"))?
                    .panel
                    .accepts_input()
                {
                    return write_response(&mut stream, 409, "text/plain", b"Panel is busy or its contents are uncertain. Complete the update or retry the failed refresh before tapping.");
                }

                let response = std::str::from_utf8(&request.body)
                    .map_err(io::Error::other)
                    .and_then(|source| self.replay_input(&format!("tap {source}")));
                match response {
                    Ok(()) => write_response(&mut stream, 204, "text/plain; charset=utf-8", b""),
                    Err(error) => {
                        write_response(&mut stream, 400, "text/plain", error.to_string().as_bytes())
                    }
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

fn diagnostics_json(
    screen: &Screen,
    pictures: &kobo_ui::PictureCache,
    orientation: kobo_ui::Orientation,
) -> String {
    diagnostics_json_with_chrome(screen, pictures, orientation, &kobo_ui::Chrome::default())
}

fn diagnostics_json_with_chrome(
    screen: &Screen,
    pictures: &kobo_ui::PictureCache,
    orientation: kobo_ui::Orientation,
    chrome: &kobo_ui::Chrome,
) -> String {
    let diagnostics = screen.diagnostics_with_pictures(
        &profile_metrics().oriented(orientation),
        chrome,
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
        let rect = issue
            .rect
            .map(|rect| physical_rect(orientation, rect))
            .map_or_else(
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
            "\"height\":{},\"pixelsPerInch\":{},\"rotation\":{},\"colourPanel\":{},",
            "\"touch\":{{\"name\":{},\"xMin\":{},\"xMax\":{},",
            "\"yMin\":{},\"yMax\":{}}}}},\"scenario\":{},",
            "\"lifecycle\":{},\"transition\":{},\"refreshCount\":{},",
            "\"dirtyPixelsSinceClean\":{},\"touch\":{},",
            "\"panelApproximation\":true,\"panel\":{},\"observation\":{},\"backends\":{}}}"
        ),
        json_string(PROFILE.id),
        json_string(PROFILE.model),
        PROFILE.width,
        PROFILE.height,
        PROFILE.pixels_per_inch,
        POSE.rotation(),
        PROFILE.colour_panel,
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
        panel.state_json().to_json(),
        observation().ok().flatten().map_or_else(
            || "null".into(),
            |value| value.to_json().unwrap_or_else(|_| "null".into())
        ),
        format!(
            "[{}]",
            configured_backends()
                .map(|value| value
                    .iter()
                    .map(|c| json_string(c.manifest_name()))
                    .collect::<Vec<_>>()
                    .join(","))
                .unwrap_or_default()
        ),
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
fn layout_json(screen: &Screen, paints: u64, orientation: kobo_ui::Orientation) -> String {
    layout_json_with_chrome(screen, paints, orientation, &kobo_ui::Chrome::default())
}

fn layout_json_with_chrome(
    screen: &Screen,
    paints: u64,
    orientation: kobo_ui::Orientation,
    chrome: &kobo_ui::Chrome,
) -> String {
    let metrics = profile_metrics().oriented(orientation);
    let layout = screen.layout_with(&metrics, chrome);
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
        let logical_centre = (
            node.rect.x + node.rect.width / 2,
            node.rect.y + node.rect.height / 2,
        );
        let action = layout
            .hit_test(logical_centre.0, logical_centre.1)
            .map_or_else(|| "null".to_owned(), |action| action.0.to_string());
        let rect = physical_rect(orientation, node.rect);
        let centre = (rect.x + rect.width / 2, rect.y + rect.height / 2);
        let _ = std::fmt::Write::write_fmt(
            &mut json,
            format_args!(
                "{{\"kind\":{},\"x\":{},\"y\":{},\"width\":{},\"height\":{},\
                 \"centre\":{{\"x\":{},\"y\":{}}},\"action\":{action},\"lines\":[{lines}]}}",
                json_string(&format!("{:?}", node.kind)),
                rect.x,
                rect.y,
                rect.width,
                rect.height,
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
    state.record(line.to_owned());
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
            version: kobo_protocol::VERSION,
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
        let writer = AppWriter::spawn_for(ours, kobo_protocol::VERSION);
        let frame = Frame {
            version: kobo_protocol::VERSION,
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

    #[test]
    fn a_legacy_session_receives_legacy_runtime_events() {
        let (ours, mut theirs) = std::os::unix::net::UnixStream::pair().expect("socket pair");
        let writer = AppWriter::spawn_for(ours, kobo_protocol::LEGACY_VERSION);
        let frame = Frame {
            version: kobo_protocol::VERSION,
            request_id: 4,
            message: Message::Lifecycle(kobo_protocol::Lifecycle::Foreground),
        };
        write_shared(&writer, &frame).expect("queued");
        let delivered = super::read_protocol_frame(&mut theirs).expect("delivered");
        assert_eq!(delivered.version, kobo_protocol::LEGACY_VERSION);
        assert_eq!(delivered.message, frame.message);
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
    sender: Mutex<std::sync::mpsc::Sender<Option<Frame>>>,
    socket: Arc<UnixStream>,
    /// Fixed by Hello for the lifetime of this one-app session.
    version: u8,
    activity: Arc<Mutex<activity::Activity>>,
}

impl AppWriter {
    fn spawn_for(stream: UnixStream, version: u8) -> Arc<Self> {
        let (sender, receiver) = std::sync::mpsc::channel::<Option<Frame>>();
        let activity = Arc::new(Mutex::new(activity::Activity::default()));
        let thread_activity = Arc::clone(&activity);
        let socket = Arc::new(stream);
        let thread_socket = Arc::clone(&socket);
        std::thread::spawn(move || {
            let mut output = &*thread_socket;
            for frame in receiver {
                let Some(frame) = frame else {
                    break;
                };
                if write_to(&mut output, &frame).is_err() {
                    break;
                }
            }
            let _ = thread_socket.shutdown(std::net::Shutdown::Both);
            if let Ok(mut activity) = thread_activity.lock() {
                activity.disconnected();
            }
        });
        Arc::new(Self {
            sender: Mutex::new(sender),
            socket,
            version,
            activity,
        })
    }
    fn connected(&self) -> bool {
        self.activity
            .lock()
            .is_ok_and(|activity| activity.connected())
    }
    fn close(&self) {
        if let Ok(mut activity) = self.activity.lock() {
            activity.disconnected();
        }
        // Shutdown unblocks both a writer stalled on a full socket and the
        // session reader. The sentinel also wakes a writer waiting for work.
        let _ = self.socket.shutdown(std::net::Shutdown::Both);
        if let Ok(sender) = self.sender.lock() {
            let _ = sender.send(None);
        }
    }
}

/// Queues one frame for the application. Never blocks on the socket.
fn write_shared(writer: &Arc<AppWriter>, frame: &Frame) -> io::Result<()> {
    let mut frame = frame.clone();
    frame.version = writer.version;
    {
        let mut activity = writer
            .activity
            .lock()
            .map_err(|_| io::Error::other("simulator activity lock poisoned"))?;
        if !activity.connected() {
            return Err(io::Error::other("the application is disconnected"));
        }
        activity.sent(&frame.message);
    }
    writer
        .sender
        .lock()
        .map_err(|_| io::Error::other("simulator write lock poisoned"))?
        .send(Some(frame))
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
        if !writer.connected() {
            return Ok(());
        }
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
                    version: kobo_protocol::VERSION,
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
                version: kobo_protocol::VERSION,
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
fn simulated_shells(
    writer: &Arc<AppWriter>,
    declared: &kobo_policy::Declared,
) -> (
    Arc<Mutex<kobo_shell::Shells>>,
    thread::JoinHandle<io::Result<()>>,
) {
    let shells = Arc::new(Mutex::new(kobo_shell::Shells::new(
        &declared.iter().collect::<Vec<_>>(),
    )));
    let draining = Arc::clone(&shells);
    let writer = Arc::clone(writer);
    let worker = std::thread::spawn(move || drain_shell(&draining, &writer));
    (shells, worker)
}

/// Join the output pumps before reporting that a disconnected session has
/// released its tasks and terminal. Closing the socket also wakes the reader.
struct SessionWorkers {
    writer: Arc<AppWriter>,
    workers: Vec<thread::JoinHandle<io::Result<()>>>,
}
impl Drop for SessionWorkers {
    fn drop(&mut self) {
        self.writer.close();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
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
        Scenario::PermissionDenied if network => Some(kobo_protocol::TaskError::Denied),
        Scenario::MissingSecret
            if matches!(
                task,
                kobo_protocol::Task::Post {
                    credential: Some(_),
                    ..
                } | kobo_protocol::Task::Fetch {
                    credential: Some(_),
                    ..
                }
            ) =>
        {
            Some(kobo_protocol::TaskError::NoCredential)
        }
        Scenario::NetworkTimeout if network => Some(kobo_protocol::TaskError::TimedOut),
        _ => None,
    }
}

fn simulated_task_error(
    scenario: Scenario,
    task: &kobo_protocol::Task,
    declared: &kobo_policy::Declared,
    backends: &kobo_policy::Declared,
) -> Option<kobo_protocol::TaskError> {
    if matches!(
        task,
        kobo_protocol::Task::Fetch { .. } | kobo_protocol::Task::Post { .. }
    ) {
        if !declared.holds(kobo_policy::Capability::Network) {
            return Some(kobo_protocol::TaskError::Denied);
        }
        if !backends.holds(kobo_policy::Capability::Network) {
            return Some(kobo_protocol::TaskError::Offline);
        }
    }
    scenario_task_error(scenario, task)
}

#[allow(
    clippy::too_many_lines,
    reason = "one exhaustive protocol message dispatcher"
)]
fn read_app_messages(
    mut stream: UnixStream,
    name: &str,
    declared: &kobo_policy::Declared,
    writer: &Arc<AppWriter>,
    state: &Arc<Mutex<AppState>>,
    tasks: &Arc<Mutex<TaskRunner>>,
) -> io::Result<()> {
    let backends = configured_backends()?;
    // Drained on its own thread for the same reason terminal output is. The
    // message loop below blocks on the application's socket, so an outcome
    // that arrived while nothing was being typed used to sit in the channel
    // until the developer happened to tap something. A refusal is instant and
    // was therefore delivered immediately, which is exactly why the gap
    // survived: the only tasks the simulator ever completed were the ones it
    // refused.
    let mut workers = SessionWorkers {
        writer: Arc::clone(writer),
        workers: Vec::with_capacity(2),
    };
    {
        let draining = Arc::clone(tasks);
        let writer = Arc::clone(writer);
        let state = Arc::clone(state);
        workers.workers.push(std::thread::spawn(move || {
            drain_tasks(&draining, &writer, &state)
        }));
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
    let (shells, shell_worker) = simulated_shells(writer, declared);
    workers.workers.push(shell_worker);
    loop {
        let frame = read_protocol_frame(&mut stream)?;
        let request_id = frame.request_id;
        match frame.message {
            Message::SetScreen(screen) => {
                let mut state = state
                    .lock()
                    .map_err(|_| io::Error::other("app state lock poisoned"))?;
                state.set_screen(screen);
            }
            Message::SetOrientation(orientation) => {
                let mut state = state
                    .lock()
                    .map_err(|_| io::Error::other("app state lock poisoned"))?;
                state.orientation = orientation;
                state.commit_frame();
            }
            // The simulator hosts exactly one application, so a launch is
            // reported rather than performed. Pretending it worked would hide
            // the handover from the developer, which is the interesting part.
            Message::Launch { name } => {
                let mut state = state
                    .lock()
                    .map_err(|_| io::Error::other("app state lock poisoned"))?;
                state.record(format!(
                    "Info: asked to launch {name}; the simulator hosts one application"
                ));
            }
            message if is_picture_message(&message) => hold(state, message)?,
            Message::Log {
                level: kobo_protocol::LogLevel::Debug,
                message,
            } if message == kobo_protocol::SIM_CALLBACK_COMPLETE => {
                writer
                    .activity
                    .lock()
                    .map_err(|_| io::Error::other("simulator activity lock poisoned"))?
                    .callback_complete();
            }
            Message::Log { level, message } => note(state, &format!("{level:?}: {message}"))?,
            Message::DeviceRequest(request) => {
                let scenario = current_scenario(state);
                let result =
                    if let Some(result) = simulated_app_request(state, name, scenario, &request)? {
                        result
                    } else if !simulated_platform_request_allowed(name, &request)
                        || scenario == Scenario::PermissionDenied
                    {
                        kobo_protocol::DeviceResult::Denied(kobo_protocol::DenyReason::NotDeclared)
                    } else {
                        let mut state = state
                            .lock()
                            .map_err(|_| io::Error::other("app state lock poisoned"))?;
                        state.observe_hardware();
                        let mut result = state.services.handle(request.clone());
                        if let kobo_protocol::DeviceResult::Identity(identity) = &mut result {
                            identity.profile_id = format!("SIMULATOR:{}", PROFILE.id);
                            identity.model = format!("Simulated {}", PROFILE.model);
                            identity.panel_width = PROFILE.width;
                            identity.panel_height = PROFILE.height;
                        }
                        if let kobo_protocol::DeviceResult::Frontlight { percent } = &result {
                            state.hardware.frontlight_percent = *percent;
                        }
                        result
                    };
                {
                    let mut state = state
                        .lock()
                        .map_err(|_| io::Error::other("app state lock poisoned"))?;
                    state.record(format!("device: {request:?} -> {result:?}"));
                }
                write_shared(
                    writer,
                    &Frame {
                        version: kobo_protocol::VERSION,
                        request_id,
                        message: Message::DeviceResult(result),
                    },
                )?;
            }
            Message::Spawn { task, work } => {
                let fault =
                    simulated_task_error(current_scenario(state), &work, declared, &backends);
                submit_simulated_task(tasks, writer, state, task, work, fault)?;
            }
            Message::StoreRequest(request) => {
                if scenario_refuses_store(current_scenario(state), &request) {
                    let result =
                        kobo_protocol::StoreResult::Denied(kobo_protocol::StoreError::TooFull);
                    note(state, &format!("store: injected {result:?}"))?;
                    write_shared(
                        writer,
                        &Frame {
                            version: kobo_protocol::VERSION,
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
        deliver_task_outcomes(tasks, writer, state)?;
    }
}

/// Hold runner admission through activity registration and immediate reply so
/// the drain worker cannot deliver a completion before its metadata exists.
fn submit_simulated_task(
    tasks: &Arc<Mutex<TaskRunner>>,
    writer: &Arc<AppWriter>,
    state: &Arc<Mutex<AppState>>,
    task: kobo_protocol::TaskId,
    work: kobo_protocol::Task,
    fault: Option<kobo_protocol::TaskError>,
) -> io::Result<()> {
    let mut tasks = tasks
        .lock()
        .map_err(|_| io::Error::other("simulator task lock poisoned"))?;
    let kind = activity::Kind::from(&work);
    let admitted = tasks.submit_with_fault(task, work, fault);
    {
        let mut activity = writer
            .activity
            .lock()
            .map_err(|_| io::Error::other("simulator activity lock poisoned"))?;
        if admitted == Err(kobo_policy::RejectReason::DuplicateId) {
            activity.attempted(kind);
        } else {
            activity.started(task, kind);
        }
    }
    if let Err(reason) = admitted {
        note(state, &format!("task {} refused: {reason:?}", task.0))?;
        if let Some(outcome) = reason.outcome() {
            write_shared(
                writer,
                &Frame {
                    version: kobo_protocol::VERSION,
                    request_id: 0,
                    message: Message::TaskOutcome { task, outcome },
                },
            )?;
        }
    }
    Ok(())
}

fn scenario_refuses_store(scenario: Scenario, request: &kobo_protocol::StoreRequest) -> bool {
    scenario == Scenario::StorageFull
        && matches!(
            request,
            kobo_protocol::StoreRequest::Save { .. }
                | kobo_protocol::StoreRequest::ShelfWrite { .. }
        )
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

    if let DeviceRequest::SetSecret { name, value } = request {
        if scenario == Scenario::PermissionDenied
            || !kobo_policy::credentials::may_set(caller, name)
        {
            return Ok(Some(DeviceResult::Denied(DenyReason::NotDeclared)));
        }
        let directory = state
            .lock()
            .map_err(|_| io::Error::other("app state lock poisoned"))?
            .secret_directory
            .clone();
        let result =
            kobo_policy::credentials::install_app_secret(&directory, caller, name, value.as_str())
                .map_or_else(DeviceResult::Failed, |()| DeviceResult::Done);
        return Ok(Some(result));
    }

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
        if !writer.connected() {
            return Ok(());
        }
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
    // Serialize draining through queue delivery. A clock advance must not
    // return while another drain has removed due timers but not queued their
    // callbacks yet.
    let mut tasks = tasks
        .lock()
        .map_err(|_| io::Error::other("simulator task lock poisoned"))?;
    let finished = tasks.drain();
    for finished in finished {
        // A failure is always printed, whatever the log setting. It is the one
        // line that explains a screen the developer is looking at, and the
        // alternative is guessing which of a dozen requests went wrong.
        if let kobo_protocol::TaskOutcome::Failed(error) = &finished.outcome {
            eprintln!("task {} failed: {error:?}", finished.task.0);
        }
        let summary = match &finished.outcome {
            kobo_protocol::TaskOutcome::Completed(bytes) => {
                format!("completed {} bytes", bytes.len())
            }
            kobo_protocol::TaskOutcome::Failed(error) => format!("failed: {error:?}"),
            kobo_protocol::TaskOutcome::Cancelled => "cancelled".into(),
        };
        note(state, &format!("task {} -> {summary}", finished.task.0))?;
        write_shared(
            writer,
            &Frame {
                version: kobo_protocol::VERSION,
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
/// arranged to avoid. Capabilities come from the app's publishing manifest;
/// an unknown app has no implicit network or shell permission.
fn simulated_tasks(name: &str, declared: &kobo_policy::Declared) -> TaskRunner {
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
        .with_app_secrets(std::env::temp_dir().join(SIM_SECRETS), name);
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
        .with_fetch(Arc::new(kobo_net::fetch_from_controlled))
        .with_post(Arc::new(kobo_net::post_controlled))
        .with_line_streams(Arc::new(kobo_net::LineStreams::default()))
        .with_credential_policy(Arc::new(
            move |credential, url, usage, body, content_type| {
                kobo_policy::credentials::allowed_request(
                    &app,
                    credential,
                    url,
                    usage,
                    body,
                    content_type,
                )
            },
        ))
        .with_capabilities(declared.iter())
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
        409 => "Conflict",
        _ => "Internal Server Error",
    };
    write!(
        stream,
        "HTTP/1.1 {status} {phrase}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)
}

const SHELL: &str = include_str!("shell.html");

#[cfg(test)]
mod tests {
    #[test]
    fn observations_cannot_silently_select_an_unrelated_profile_or_pose() {
        let mut observation = kobo_profile::observation::Observation::parse(include_str!(
            "../../../docs/quality/fixtures/clara-bw-synthetic.json"
        ))
        .unwrap();
        assert_eq!(
            super::profile_from_observation(None, Some(&observation))
                .unwrap()
                .id,
            "clara-bw-391"
        );
        assert!(
            super::profile_from_observation(Some("libra-colour-390"), Some(&observation)).is_err()
        );
        observation.snapshot.framebuffer.as_mut().unwrap().rotation = 2;
        assert!(super::profile_from_observation(None, Some(&observation)).is_err());
        observation.snapshot.framebuffer.as_mut().unwrap().rotation = 3;
        observation.snapshot.framebuffer.as_mut().unwrap().stride += 1;
        assert!(super::profile_from_observation(None, Some(&observation)).is_err());
    }
    #[test]
    fn local_manifest_is_authoritative_and_cannot_borrow_a_catalog_identity() {
        let mut app = kobo_catalog::bundled().unwrap().remove(0);
        app.id = "local-reader".into();
        app.capabilities = vec!["network".into()];
        let declared = super::session_declaration("local-reader", Some(&app)).unwrap();
        assert_eq!(
            declared.iter().collect::<Vec<_>>(),
            vec![kobo_policy::Capability::Network]
        );
        assert!(super::session_declaration("terminal", Some(&app)).is_err());
        app.capabilities.clear();
        assert_eq!(
            super::session_declaration("local-reader", Some(&app))
                .unwrap()
                .iter()
                .count(),
            0
        );
        assert_eq!(
            super::session_declaration("unknown-app", None)
                .unwrap()
                .iter()
                .count(),
            0
        );
    }

    #[test]
    fn unavailable_backend_and_undeclared_service_match_runtime_refusals() {
        use kobo_policy::{Backends, Declared, DeviceServices, PowerPolicy};
        use kobo_protocol::{DenyReason, DeviceRequest, DeviceResult};
        let available = super::parse_backends(Some("battery-read")).unwrap();
        let mut services = DeviceServices::new(
            Declared::all(),
            PowerPolicy::DEFAULT,
            Backends::with(available.iter()),
        );
        assert_eq!(
            services.handle(DeviceRequest::ReadCover),
            DeviceResult::Denied(DenyReason::Unsupported)
        );
        assert!(matches!(
            services.handle(DeviceRequest::ReadBattery),
            DeviceResult::Battery { .. }
        ));
        let mut undeclared = DeviceServices::new(
            Declared::parse([]).unwrap(),
            PowerPolicy::DEFAULT,
            Backends::with(available.iter()),
        );
        assert_eq!(
            undeclared.handle(DeviceRequest::ReadBattery),
            DeviceResult::Denied(DenyReason::NotDeclared)
        );
        assert!(super::parse_backends(Some("typo")).is_err());
        assert_eq!(super::parse_backends(Some("")).unwrap().iter().count(), 0);
    }

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
    fn simulated_secret_install_is_authorized_and_visible_to_tasks() {
        use kobo_protocol::{DeviceRequest, DeviceResult, SecretValue};

        let directory = private_temp_dir();
        let path = directory.join("apps/zotero-reader/zotero");
        let state = Arc::new(Mutex::new(AppState::with_apps(Arc::new(Mutex::new(
            SimulatedApps::default(),
        )))));
        state
            .lock()
            .unwrap()
            .secret_directory
            .clone_from(&directory);
        let request = DeviceRequest::SetSecret {
            name: "zotero".to_owned(),
            value: SecretValue::new("private-token"),
        };
        assert_eq!(
            app_result(&state, "zotero-reader", Scenario::Normal, &request),
            DeviceResult::Done
        );
        assert_eq!(
            fs::read(&path).expect("stored simulator secret"),
            b"private-token"
        );
        assert!(matches!(
            app_result(&state, "todo", Scenario::Normal, &request),
            DeviceResult::Denied(_)
        ));
        fs::remove_dir_all(directory).expect("remove owned test directory");
    }

    #[test]
    fn simulated_store_updates_and_reinstalls_in_one_session() {
        use kobo_protocol::{DeviceRequest, DeviceResult};

        let catalog_len = kobo_catalog::bundled().expect("catalog").len();
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
        assert_eq!(entries.len(), catalog_len);
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
        assert_eq!(entries.len(), catalog_len - 1);
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
        assert_eq!(entries.len(), catalog_len);
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
    fn injected_failures_and_capacity_use_runner_delivery_without_duplicate_callbacks() {
        use kobo_protocol::{Task, TaskError, TaskId, TaskOutcome};
        let (mut peer, socket) = UnixStream::pair().unwrap();
        peer.set_read_timeout(Some(Duration::from_millis(250)))
            .unwrap();
        let writer = AppWriter::spawn_for(socket, kobo_protocol::VERSION);
        let state = Arc::new(Mutex::new(AppState::default()));
        let clock = Arc::new(
            kobo_policy::clock::ManualClock::new(kobo_policy::clock::Snapshot {
                unix_millis: 0,
                monotonic_millis: 0,
                utc_offset_minutes: 0,
            })
            .unwrap(),
        );
        let tasks = Arc::new(Mutex::new(
            TaskRunner::simulated(private_temp_dir()).with_manual_clock(clock),
        ));
        writer.activity.lock().unwrap().callback_complete();
        for id in 1..=4 {
            submit_simulated_task(
                &tasks,
                &writer,
                &state,
                TaskId(id),
                Task::Sleep { seconds: 300 },
                None,
            )
            .unwrap();
        }
        let fetch = Task::Fetch {
            url: "https://fixture.invalid".into(),
            offset: 0,
            max_bytes: 32,
            credential: None,
            headers: vec![],
        };
        submit_simulated_task(
            &tasks,
            &writer,
            &state,
            TaskId(1),
            fetch.clone(),
            Some(TaskError::Offline),
        )
        .unwrap();
        submit_simulated_task(
            &tasks,
            &writer,
            &state,
            TaskId(5),
            fetch,
            Some(TaskError::Offline),
        )
        .unwrap();
        assert!(matches!(
            read_protocol_frame(&mut peer).unwrap().message,
            Message::TaskOutcome {
                task: TaskId(5),
                outcome: TaskOutcome::Failed(TaskError::Denied),
            }
        ));
        let activity = writer.activity.lock().unwrap().json();
        assert!(activity.contains("\"sleepingTasks\":4"));
        assert!(activity.contains("\"connected\":true"));
        assert!(activity.contains("\"activeWork\":0"));
        writer.activity.lock().unwrap().callback_complete();
        tasks.lock().unwrap().cancel_all();
        deliver_task_outcomes(&tasks, &writer, &state).unwrap();
        let mut ids = std::collections::BTreeSet::new();
        for _ in 0..4 {
            match read_protocol_frame(&mut peer).unwrap().message {
                Message::TaskOutcome {
                    task,
                    outcome: TaskOutcome::Cancelled,
                } => {
                    assert!(ids.insert(task.0));
                }
                other => panic!("unexpected outcome: {other:?}"),
            }
            writer.activity.lock().unwrap().callback_complete();
        }
        assert_eq!(ids, [1, 2, 3, 4].into());
        assert!(
            read_protocol_frame(&mut peer).is_err(),
            "no duplicate completion for task 1"
        );
        assert!(writer
            .activity
            .lock()
            .unwrap()
            .json()
            .contains("\"idle\":true"));
        writer.close();
    }

    #[test]
    fn disconnect_reaps_session_workers_and_preserves_last_screen() {
        let root = private_temp_dir();
        let server = AppServer::bind("127.0.0.1:0", root.join("app.sock")).unwrap();
        let mut peer = UnixStream::connect(root.join("app.sock")).unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        write_protocol_frame(
            &mut peer,
            &Frame {
                version: kobo_protocol::VERSION,
                request_id: 1,
                message: Message::Hello {
                    name: "test-app".into(),
                },
            },
        )
        .unwrap();
        let session = server.accept_app().unwrap();
        assert!(matches!(
            read_protocol_frame(&mut peer).unwrap().message,
            Message::Welcome { .. }
        ));
        session.state.lock().unwrap().set_screen(Screen::new(
            1,
            vec![Node::Button {
                id: NodeId(1),
                action: ActionId(9),
                label: "Last screen".into(),
                state: kobo_ui::ControlState::Enabled,
                emphasis: kobo_ui::Emphasis::Normal,
            }],
        ));
        let tasks = session.state.lock().unwrap().tasks.clone().unwrap();
        tasks
            .lock()
            .unwrap()
            .submit(
                kobo_protocol::TaskId(99),
                kobo_protocol::Task::Sleep { seconds: 300 },
            )
            .unwrap();
        session.writer.activity.lock().unwrap().started(
            kobo_protocol::TaskId(99),
            &kobo_protocol::Task::Sleep { seconds: 300 },
        );
        let before = session.screen();
        drop(peer);
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < deadline
            && !session
                .writer
                .activity
                .lock()
                .unwrap()
                .json()
                .contains("\"cleanupComplete\":true")
        {
            thread::sleep(Duration::from_millis(5));
        }
        let activity = session.writer.activity.lock().unwrap().json();
        assert!(activity.contains("\"cleanupComplete\":true"), "{activity}");
        assert!(activity.contains("\"connected\":false"));
        assert!(activity.contains("\"abandoned\":1"));
        assert!(activity.contains("\"idle\":false"));
        assert_eq!(tasks.lock().unwrap().in_flight(), 0);
        // Only the retained AppState owns a task runner once the reader and
        // output pump have exited; this local reference is the second owner.
        assert_eq!(Arc::strong_count(&tasks), 2);
        assert_eq!(session.screen(), before);
        assert!(session.cancel_tasks().is_err());
        assert!(session.replay_input("gpio 1 193 1").is_err());
        drop(session);
        drop(server);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn transfer_cancellation_reaches_the_app_once_and_new_work_remains_possible() {
        use kobo_protocol::{Task, TaskId, TaskOutcome};
        use std::sync::atomic::Ordering;
        let (mut peer, socket) = UnixStream::pair().unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let tasks = Arc::new(Mutex::new(
            TaskRunner::simulated(".")
                .with_capabilities([kobo_policy::Capability::Network])
                .with_fetch(Arc::new(move |_, _, _, _, _, _, cancelled| {
                    started_tx.send(()).unwrap();
                    let deadline = std::time::Instant::now() + Duration::from_secs(2);
                    while !cancelled.load(Ordering::SeqCst) && std::time::Instant::now() < deadline
                    {
                        thread::sleep(Duration::from_millis(1));
                    }
                    Ok(Vec::new())
                })),
        ));
        let session = AppSession {
            state: Arc::new(Mutex::new(AppState {
                tasks: Some(Arc::clone(&tasks)),
                ..AppState::default()
            })),
            writer: AppWriter::spawn_for(socket, kobo_protocol::VERSION),
        };
        let work = Task::Fetch {
            url: "https://fixture.invalid/file".into(),
            offset: 0,
            max_bytes: 32,
            credential: None,
            headers: vec![],
        };
        session
            .writer
            .activity
            .lock()
            .unwrap()
            .started(TaskId(7), &work);
        tasks.lock().unwrap().submit(TaskId(7), work).unwrap();
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        session.cancel_tasks().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while tasks.lock().unwrap().in_flight() > 0 && std::time::Instant::now() < deadline {
            deliver_task_outcomes(&tasks, &session.writer, &session.state).unwrap();
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(
            read_protocol_frame(&mut peer).unwrap().message,
            Message::TaskOutcome {
                task: TaskId(7),
                outcome: TaskOutcome::Cancelled
            }
        );
        session.cancel_tasks().unwrap();
        assert!(tasks.lock().unwrap().drain().is_empty());
        tasks
            .lock()
            .unwrap()
            .submit(TaskId(8), Task::Sleep { seconds: 0 })
            .unwrap();
        let outcome = tasks.lock().unwrap().wait(Duration::from_secs(2)).unwrap();
        assert_eq!(outcome.task, TaskId(8));
        assert_eq!(outcome.outcome, TaskOutcome::Completed(Vec::new()));
        session.writer.close();
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
        let manifest = kobo_catalog::bundled()
            .expect("catalog")
            .into_iter()
            .find(|entry| entry.id == "sudoku")
            .expect("Sudoku manifest");
        assert_eq!(sudoku.version, manifest.version);
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
        let writer = AppWriter::spawn_for(server, kobo_protocol::VERSION);
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
        let mut online = simulated_tasks("gallery", &app_declaration("gallery"));
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
        assert!(simulator
            .simulation_json()
            .contains("\"waveform\":\"GC16\""));
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
        assert!(payload.contains("\"refreshCount\":2"));
        assert!(payload.contains(&format!("\"raw\":{{\"x\":{},\"y\":{}}}", raw.0, raw.1)));
        assert!(payload.contains("\"panelApproximation\":true"));
    }

    #[test]
    fn observing_frames_does_not_change_panel_history() {
        let mut simulator = Simulator::new();
        let before = simulator.simulation_json();
        for _ in 0..10 {
            let _ = simulator.frame();
            let _ = simulator.ideal_frame();
        }
        let payload = simulator.simulation_json();
        assert_eq!(before, payload);
        assert!(payload.contains("\"refreshCount\":1"));
        assert!(payload.contains("\"dirtyPixelsSinceClean\":0"));
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
            Some(kobo_protocol::TaskError::NoCredential)
        );
        let mut credentialed_get = fetch.clone();
        if let kobo_protocol::Task::Fetch { credential, .. } = &mut credentialed_get {
            *credential = Some(kobo_protocol::Credential::bearer("api-key"));
        }
        assert_eq!(
            scenario_task_error(Scenario::MissingSecret, &credentialed_get),
            Some(kobo_protocol::TaskError::NoCredential)
        );
        assert_eq!(scenario_task_error(Scenario::MissingSecret, &fetch), None);
        assert_eq!(scenario_task_error(Scenario::Normal, &fetch), None);
        assert_eq!(scenario_task_error(Scenario::LowBattery, &fetch), None);
        let none = kobo_policy::Declared::parse([]).unwrap();
        let network = kobo_policy::Declared::parse(["network"]).unwrap();
        assert_eq!(
            simulated_task_error(Scenario::MissingSecret, &credentialed_get, &none, &network),
            Some(kobo_protocol::TaskError::Denied)
        );
        assert_eq!(
            simulated_task_error(Scenario::Normal, &fetch, &network, &none),
            Some(kobo_protocol::TaskError::Offline)
        );
    }

    #[test]
    fn configuration_errors_do_not_silently_choose_a_different_device_or_size() {
        assert_eq!(parse_profile(None).unwrap().id, CLARA_BW_391.id);
        for profile in SUPPORTED_PROFILES {
            assert_eq!(parse_profile(Some(profile.id)).unwrap().id, profile.id);
        }
        assert!(parse_profile(Some("misspelled-reader")).is_err());
        assert!(parse_profile(Some("")).is_err());
        assert!(validate_scale("extra-large").is_ok());
        assert!(validate_scale("140%").is_ok());
        assert!(validate_scale("extra-lagre").is_err());
        assert!(validate_scale("0").is_err());
    }

    #[test]
    fn stream_open_and_next_use_the_same_network_failure_injection() {
        for operation in ["open", "next", "close"] {
            let task = kobo_protocol::Task::Fetch {
                url: "https://example.invalid/events".into(),
                offset: 0,
                max_bytes: 128,
                credential: None,
                headers: vec![kobo_protocol::Header::new(
                    "X-Cobalt-Line-Stream",
                    operation,
                )],
            };
            assert_eq!(
                scenario_task_error(Scenario::Offline, &task),
                Some(kobo_protocol::TaskError::Offline)
            );
            assert_eq!(
                scenario_task_error(Scenario::NetworkTimeout, &task),
                Some(kobo_protocol::TaskError::TimedOut)
            );
        }
    }

    #[test]
    fn storage_full_refuses_shelf_chunks_but_allows_read_and_cleanup() {
        use kobo_protocol::StoreRequest;
        let write = StoreRequest::ShelfWrite {
            name: "comic".into(),
            offset: 0,
            bytes: vec![1],
            last: true,
        };
        assert!(scenario_refuses_store(Scenario::StorageFull, &write));
        assert!(!scenario_refuses_store(Scenario::Normal, &write));
        assert!(!scenario_refuses_store(
            Scenario::StorageFull,
            &StoreRequest::ShelfRemove {
                name: "comic".into()
            }
        ));
        assert!(!scenario_refuses_store(
            Scenario::StorageFull,
            &StoreRequest::Load {
                key: "comic".into()
            }
        ));
    }

    #[test]
    fn unknown_and_offline_apps_do_not_inherit_network_or_shell() {
        use kobo_policy::Capability;
        for name in ["sudoku", "unregistered-application"] {
            assert!(!app_declaration(name).holds(Capability::Network));
            assert!(!app_declaration(name).holds(Capability::Shell));
        }
        assert!(app_declaration("gallery").holds(Capability::Network));
        assert!(app_declaration("terminal").holds(Capability::Shell));
    }

    #[test]
    fn panel_history_tracks_every_screen_even_without_a_screenshot() {
        let mut state = AppState::default();
        for number in 1..=3 {
            state.set_screen(Screen::new(
                number,
                vec![Node::Text {
                    id: NodeId(1),
                    text: format!("Count {number}"),
                    links: vec![],
                }],
            ));
        }
        assert_eq!(state.paints, 3);
        assert_eq!(state.panel.planner.refreshes(), 3);
        let before = state.panel.planner.refreshes();
        for _ in 0..10 {
            let _ = state.panel.frame(false);
            let _ = state.panel.frame(true);
        }
        assert_eq!(state.panel.planner.refreshes(), before);
        for _ in 0..100 {
            state.record("x".repeat(5000));
        }
        assert_eq!(state.logs.len(), 64);
        assert!(state.logs.iter().all(|line| line.len() == 4096));
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
    fn manual_clock_delivers_due_callbacks_before_return_and_capture_reads_do_not_tick() {
        let clock = Arc::new(
            kobo_policy::clock::ManualClock::new(kobo_policy::clock::Snapshot {
                unix_millis: 1_704_067_200_000,
                monotonic_millis: 0,
                utc_offset_minutes: 0,
            })
            .unwrap(),
        );
        let tasks = Arc::new(Mutex::new(
            TaskRunner::simulated(".").with_manual_clock(Arc::clone(&clock)),
        ));
        tasks
            .lock()
            .unwrap()
            .submit(
                kobo_protocol::TaskId(7),
                kobo_protocol::Task::Sleep { seconds: 30 },
            )
            .unwrap();
        let (mut client, server) = UnixStream::pair().unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let session = AppSession {
            state: Arc::new(Mutex::new(AppState {
                time: clock::Time::Manual(clock),
                tasks: Some(tasks),
                ..AppState::default()
            })),
            writer: AppWriter::spawn_for(server, kobo_protocol::VERSION),
        };
        session.change_clock("set 1704153600000 330").unwrap();
        assert_eq!(
            session
                .state
                .lock()
                .unwrap()
                .clock_snapshot
                .monotonic_millis,
            0
        );
        let before = session.state.lock().unwrap().simulation_json();
        for _ in 0..5 {
            session.render_frame(false);
            assert_eq!(session.state.lock().unwrap().simulation_json(), before);
        }
        session.change_clock("advance 30000").unwrap();
        let received = read_protocol_frame(&mut client).unwrap();
        assert_eq!(
            received.message,
            Message::TaskOutcome {
                task: kobo_protocol::TaskId(7),
                outcome: kobo_protocol::TaskOutcome::Completed(Vec::new())
            }
        );
        assert!(session
            .writer
            .activity
            .lock()
            .unwrap()
            .json()
            .contains("pendingCallbacks"));
        assert_eq!(
            session
                .state
                .lock()
                .unwrap()
                .clock_snapshot
                .monotonic_millis,
            30_000
        );
        let before = session.state.lock().unwrap().simulation_json();
        assert!(session.change_clock("advance -1").is_err());
        assert!(session.change_clock("advance 999999999999").is_err());
        assert_eq!(session.state.lock().unwrap().simulation_json(), before);
    }

    #[test]
    fn hardware_controls_update_service_reads_and_only_emit_foreground_cover_edges() {
        use kobo_protocol::{DeviceRequest, DeviceResult};
        let (mut client, server) = UnixStream::pair().unwrap();
        client
            .set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        let session = AppSession {
            state: Arc::new(Mutex::new(AppState {
                services: DeviceServices::simulated(),
                ..AppState::default()
            })),
            writer: AppWriter::spawn_for(server, kobo_protocol::VERSION),
        };
        session.change_hardware("battery 18 charging").unwrap();
        session.change_hardware("frontlight 39").unwrap();
        {
            let mut state = session.state.lock().unwrap();
            assert_eq!(
                state.services.handle(DeviceRequest::ReadBattery),
                DeviceResult::Battery {
                    percent: 18,
                    charging: true
                }
            );
            assert_eq!(
                state.services.handle(DeviceRequest::ReadFrontlight),
                DeviceResult::Frontlight { percent: 39 }
            );
        }
        session.set_scenario(Scenario::LowBattery).unwrap();
        assert_eq!(
            session
                .state
                .lock()
                .unwrap()
                .services
                .handle(DeviceRequest::ReadBattery),
            DeviceResult::Battery {
                percent: 5,
                charging: true
            }
        );
        session.set_scenario(Scenario::Normal).unwrap();
        assert_eq!(
            session
                .state
                .lock()
                .unwrap()
                .services
                .handle(DeviceRequest::ReadBattery),
            DeviceResult::Battery {
                percent: 18,
                charging: true
            }
        );
        session.change_hardware("cover closed").unwrap();
        assert_eq!(
            read_protocol_frame(&mut client).unwrap().message,
            Message::CoverChanged {
                magnet_present: true
            }
        );
        session.change_hardware("cover closed").unwrap();
        assert!(read_protocol_frame(&mut client).is_err());
        session.state.lock().unwrap().lifecycle = Lifecycle::Background;
        session.change_hardware("cover open").unwrap();
        assert!(read_protocol_frame(&mut client).is_err());
        assert_eq!(
            session
                .state
                .lock()
                .unwrap()
                .services
                .handle(DeviceRequest::ReadCover),
            DeviceResult::Cover {
                available: true,
                magnet_present: false
            }
        );
        let before = session.state.lock().unwrap().hardware;
        assert!(session.change_hardware("battery 101 charging").is_err());
        assert_eq!(session.state.lock().unwrap().hardware, before);
    }

    #[test]
    fn raw_hold_and_page_press_deliver_real_messages_and_background_input_is_quiet() {
        let clock = Arc::new(
            kobo_policy::clock::ManualClock::new(kobo_policy::clock::Snapshot {
                unix_millis: 1_704_067_200_000,
                monotonic_millis: 0,
                utc_offset_minutes: 0,
            })
            .unwrap(),
        );
        let (mut client, server) = UnixStream::pair().unwrap();
        client
            .set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        let mut state = AppState {
            time: clock::Time::Manual(Arc::clone(&clock)),
            ..AppState::default()
        };
        state.set_screen(Screen::new(4, Vec::new()).with_hold(ActionId(13)));
        let session = AppSession {
            state: Arc::new(Mutex::new(state)),
            writer: AppWriter::spawn_for(server, kobo_protocol::VERSION),
        };
        let (x, y) = POSE.display_to_touch(500, 500).unwrap();
        session
            .replay_input(&format!("touch 3 47 0;3 57 9;3 53 {x};3 54 {y};0 0 0"))
            .unwrap();
        clock.advance(Duration::from_millis(500)).unwrap();
        session.replay_input("touch 3 57 -1;0 0 0").unwrap();
        assert_eq!(
            read_protocol_frame(&mut client).unwrap().message,
            Message::Action {
                action: ActionId(13)
            }
        );
        session.replay_input("gpio 4 3 24").unwrap();
        session.replay_input("gpio 1 194 1").unwrap();
        assert_eq!(
            read_protocol_frame(&mut client).unwrap().message,
            Message::PageTurn { forward: true }
        );
        session.replay_input("gpio 1 194 0").unwrap();
        assert!(read_protocol_frame(&mut client).is_err());
        session.state.lock().unwrap().lifecycle = Lifecycle::Background;
        session.replay_input("gpio 1 194 1").unwrap();
        assert!(read_protocol_frame(&mut client).is_err());
    }

    #[test]
    fn lifecycle_control_sends_the_real_sdk_event() {
        let (mut client, server) = UnixStream::pair().expect("socket pair");
        let session = AppSession {
            state: Arc::new(Mutex::new(AppState::default())),
            writer: AppWriter::spawn_for(server, kobo_protocol::VERSION),
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
    fn rendered_landscape_control_accepts_a_tap_at_its_physical_centre() {
        let action = ActionId(77);
        let screen = Screen::new(
            1,
            vec![Node::Button {
                id: NodeId(1),
                action,
                label: "Open".to_owned(),
                state: kobo_ui::ControlState::Enabled,
                emphasis: kobo_ui::Emphasis::Primary,
            }],
        );
        let orientation = kobo_ui::Orientation::Landscape;
        let logical = screen
            .layout_with(
                &profile_metrics().oriented(orientation),
                &kobo_ui::Chrome::default(),
            )
            .rect_of_action(action)
            .expect("button");
        let physical = physical_rect(orientation, logical);
        let (client, server) = UnixStream::pair().expect("socket pair");
        drop(client);
        let session = AppSession {
            state: Arc::new(Mutex::new(AppState {
                screen: screen.clone(),
                orientation,
                ..AppState::default()
            })),
            writer: AppWriter::spawn_for(server, kobo_protocol::VERSION),
        };

        session.state.lock().unwrap().commit_frame();
        let frame = session.render_frame(false);
        let width = usize::try_from(profile_metrics().width).expect("panel width");
        let inked = (physical.y..physical.y + physical.height)
            .flat_map(|y| (physical.x..physical.x + physical.width).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                frame[usize::try_from(y).expect("positive y") * width
                    + usize::try_from(x).expect("positive x")]
                    != kobo_ui::tone::PAPER
            })
            .count();
        assert!(inked > 0, "the transformed control was not rendered");
        assert_eq!(
            session.touch_action(
                physical.x + physical.width / 2,
                physical.y + physical.height / 2,
            ),
            Some(action)
        );

        let layout = layout_json(&screen, 1, orientation);
        assert!(layout.contains(&format!("\"x\":{}", physical.x)));
        assert!(layout.contains(&format!("\"y\":{}", physical.y)));
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
        let payload = diagnostics_json(
            &screen,
            &kobo_ui::PictureCache::default(),
            kobo_ui::Orientation::Portrait,
        );
        assert!(payload.starts_with("{\"issues\":["));
        assert!(payload.contains("below the content area"));
        assert!(payload.contains("\"severity\":\"error\""));
    }

    #[test]
    fn diagnostics_fail_when_an_interactive_control_is_offscreen() {
        let screen = Screen::new(
            1,
            vec![Node::Grid {
                id: NodeId(100),
                columns: 1,
                square: false,
                cells: (0..81)
                    .map(|index| kobo_ui::Cell::new(ActionId(index + 1), index.to_string()))
                    .collect(),
            }],
        );
        let payload = diagnostics_json(
            &screen,
            &kobo_ui::PictureCache::default(),
            kobo_ui::Orientation::Portrait,
        );
        assert!(
            payload.contains("interactive control is outside the visible panel"),
            "{payload}"
        );
        assert!(payload.contains("\"severity\":\"error\""));
    }

    #[test]
    fn landscape_diagnostics_report_physical_framebuffer_rectangles() {
        let screen = Screen::new(
            1,
            vec![Node::Grid {
                id: NodeId(100),
                columns: 1,
                square: false,
                cells: (0..81)
                    .map(|index| kobo_ui::Cell::new(ActionId(index + 1), index.to_string()))
                    .collect(),
            }],
        );
        let orientation = kobo_ui::Orientation::Landscape;
        let diagnostics = screen.diagnostics_with_pictures(
            &profile_metrics().oriented(orientation),
            &kobo_ui::Chrome::default(),
            &kobo_ui::PictureCache::default(),
        );
        let logical = diagnostics
            .issues
            .iter()
            .find_map(|issue| issue.rect)
            .expect("diagnostic rectangle");
        let physical = physical_rect(orientation, logical);
        let payload = diagnostics_json(&screen, &kobo_ui::PictureCache::default(), orientation);
        assert!(payload.contains(&format!(
            "\"rect\":{{\"x\":{},\"y\":{},\"width\":{},\"height\":{}}}",
            physical.x, physical.y, physical.width, physical.height
        )));
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
    #[allow(clippy::too_many_lines)]
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
                    version: kobo_protocol::VERSION,
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
                    version: kobo_protocol::VERSION,
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
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while std::time::Instant::now() < deadline {
            if !session.screen().nodes.is_empty() {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        let screen = session.screen();
        assert!(
            !screen.nodes.is_empty(),
            "screen was not rendered within one second"
        );
        let button = screen
            .layout()
            .nodes
            .iter()
            .find(|node| matches!(node.kind, kobo_ui::LayoutKind::Button(..)))
            .expect("rendered button")
            .rect;
        let (x, y) = (button.x + button.width / 2, button.y + button.height / 2);

        let browser = thread::spawn(move || -> io::Result<()> {
            let mut stream = TcpStream::connect(address)?;
            let body = format!("x={x}&y={y}");
            stream.write_all(
                format!(
                    "POST /touch HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
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

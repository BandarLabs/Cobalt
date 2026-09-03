//! The host half of Paperterm: a bounded terminal model and authenticated TLS
//! transport.  Terminal bytes never cross a disk boundary.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::missing_errors_doc,
    clippy::needless_pass_by_value,
    clippy::too_many_arguments,
    clippy::too_many_lines
)]
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// TCP port reserved for Paperterm's TLS listener.
pub const DEFAULT_PORT: u16 = 9332;
/// The maximum time one `/screen` request may wait for a changed row.
pub const LONGEST_POLL: Duration = Duration::from_secs(25);
/// The maximum number of decoded input bytes accepted in one request.
pub const MAX_KEY_BYTES: usize = 64;
/// Snapshot cadence: two settled views per second.
pub const SNAPSHOT_INTERVAL: Duration = Duration::from_millis(500);
const MAX_REQUEST: usize = 64 * 1024;
const MAX_READERS: usize = 16;
const FINAL_SCREEN_FOR: Duration = Duration::from_secs(60);

/// The negotiated immutable terminal geometry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Grid {
    pub columns: u16,
    pub rows: u16,
}

impl Grid {
    #[must_use]
    pub const fn fallback() -> Self {
        Self {
            columns: 100,
            rows: 35,
        }
    }

    /// Parses `COLSxROWS`, with sensible non-zero terminal bounds.
    pub fn parse(text: &str) -> Result<Self, String> {
        let (columns, rows) = text.split_once('x').ok_or("grid must be COLSxROWS")?;
        let columns = columns
            .parse::<u16>()
            .map_err(|_| "grid columns must be a number")?;
        let rows = rows
            .parse::<u16>()
            .map_err(|_| "grid rows must be a number")?;
        if columns == 0 || rows == 0 || columns > 300 || rows > 120 {
            return Err("grid is outside 1x1 through 300x120".to_owned());
        }
        Ok(Self { columns, rows })
    }

    #[must_use]
    pub fn words(self) -> String {
        format!("{}x{}", self.columns, self.rows)
    }
}

/// Device input policy advertised during `/hello`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputMode {
    None,
    Controls,
    Full,
}

impl InputMode {
    #[must_use]
    pub const fn wire(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Controls => "controls",
            Self::Full => "full",
        }
    }
}

/// A terminal row and its one supported attribute: the cursor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Row {
    pub y: u16,
    pub cells: String,
    pub cursor: Option<u16>,
}

/// A bounded screen reply. `rows` is full only for sequence zero.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Screen {
    pub seq: u64,
    pub rows: Vec<Row>,
    pub ended: bool,
    pub exit: Option<i32>,
}

/// Control-strip bytes. This is deliberately shared by request validation and
/// the device documentation; controls mode accepts no other terminal input.
pub const CONTROL_BYTES: [&[u8]; 9] = [
    b"\x1b[A", b"\x1b[B", b"\x1b[D", b"\x1b[C", b"\r", b"\x1b", b"y", b"n", b"\x03",
];

/// Whether this exact input is permitted by the advertised mode.
#[must_use]
pub fn permits_input(mode: InputMode, bytes: &[u8]) -> bool {
    bytes.len() <= MAX_KEY_BYTES
        && match mode {
            InputMode::None => false,
            InputMode::Controls => {
                let mut remaining = bytes;
                while !remaining.is_empty() {
                    let Some(control) = CONTROL_BYTES
                        .iter()
                        .find(|control| remaining.starts_with(control))
                    else {
                        return false;
                    };
                    remaining = &remaining[control.len()..];
                }
                true
            }
            InputMode::Full => true,
        }
}

struct Terminal {
    parser: vt100::Parser,
    grid: Grid,
    next_lease: u64,
    lease: u64,
    generation: u64,
    previous: Vec<String>,
    previous_cursor: Option<(u16, u16)>,
    seq: u64,
    ended: Option<i32>,
    last_snapshot: Instant,
    dirty: bool,
}

impl Terminal {
    fn new(grid: Grid) -> Self {
        Self {
            parser: vt100::Parser::new(grid.rows, grid.columns, 0),
            grid,
            next_lease: 0,
            lease: 0,
            generation: 0,
            previous: Vec::new(),
            previous_cursor: None,
            seq: 0,
            ended: None,
            last_snapshot: Instant::now()
                .checked_sub(SNAPSHOT_INTERVAL)
                .unwrap_or_else(Instant::now),
            dirty: true,
        }
    }

    fn feed(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
        self.dirty = true;
    }

    fn finish(&mut self, status: i32) {
        self.ended = Some(status);
        self.last_snapshot = Instant::now()
            .checked_sub(SNAPSHOT_INTERVAL)
            .unwrap_or_else(Instant::now);
        self.dirty = true;
    }

    fn resize(&mut self, grid: Grid) {
        if self.grid == grid {
            return;
        }
        self.parser.screen_mut().set_size(grid.rows, grid.columns);
        self.grid = grid;
        self.previous.clear();
        self.previous_cursor = None;
        self.last_snapshot = Instant::now()
            .checked_sub(SNAPSHOT_INTERVAL)
            .unwrap_or_else(Instant::now);
        self.dirty = true;
    }

    fn issue_lease(&mut self) -> u64 {
        self.next_lease = self.next_lease.saturating_add(1).max(1);
        self.next_lease
    }

    fn resize_for(&mut self, grid: Grid, lease: u64, generation: u64) -> bool {
        let lease_is_current = lease == self.lease;
        let lease_is_new = lease > self.lease && lease <= self.next_lease;
        if (!lease_is_current && !lease_is_new)
            || (lease_is_current && generation < self.generation)
        {
            return false;
        }
        if lease_is_new {
            self.lease = lease;
            self.generation = 0;
        }
        self.generation = generation;
        self.resize(grid);
        true
    }

    fn screen_for(&mut self, since: u64, lease: u64, generation: u64) -> Option<Screen> {
        (lease == self.lease && generation == self.generation).then(|| self.screen(since))
    }

    fn screen(&mut self, since: u64) -> Screen {
        if since != 0 && (!self.dirty || self.last_snapshot.elapsed() < SNAPSHOT_INTERVAL) {
            return Screen {
                seq: self.seq,
                rows: Vec::new(),
                ended: self.ended.is_some(),
                exit: self.ended,
            };
        }
        let screen = self.parser.screen();
        let visible: Vec<String> = (0..self.grid.rows)
            .map(|row| visible_row(screen, row, self.grid.columns))
            .collect();
        let (cursor_y, cursor_x) = screen.cursor_position();
        let cursor =
            (!screen.hide_cursor() && cursor_y < self.grid.rows && cursor_x < self.grid.columns)
                .then_some((cursor_y, cursor_x));
        let cursor_changed = cursor != self.previous_cursor;
        let changed = visible != self.previous || cursor_changed;
        if changed {
            self.seq = self.seq.saturating_add(1);
        }
        let full = since == 0;
        let rows = visible
            .iter()
            .enumerate()
            .filter(|(index, row)| {
                let index = *index as u16;
                full || self.previous.get(usize::from(index)) != Some(*row)
                    || (cursor_changed
                        && (self.previous_cursor.is_some_and(|(y, _)| y == index)
                            || cursor.is_some_and(|(y, _)| y == index)))
            })
            .map(|(index, cells)| Row {
                y: index as u16,
                cells: cells.clone(),
                cursor: cursor
                    .filter(|(row, _)| usize::from(*row) == index)
                    .map(|(_, column)| column),
            })
            .collect();
        self.previous = visible;
        self.previous_cursor = cursor;
        self.dirty = false;
        self.last_snapshot = Instant::now();
        Screen {
            seq: self.seq,
            rows,
            ended: self.ended.is_some(),
            exit: self.ended,
        }
    }
}

/// Host-owned live screen. It is synchronised so a long poll never races a
/// terminal reader or sees half an escape sequence.
pub struct Session {
    terminal: Mutex<Terminal>,
    changed: Condvar,
}

impl Session {
    #[must_use]
    pub fn new(grid: Grid) -> Self {
        Self {
            terminal: Mutex::new(Terminal::new(grid)),
            changed: Condvar::new(),
        }
    }

    pub fn feed(&self, bytes: &[u8]) {
        if let Ok(mut terminal) = self.terminal.lock() {
            terminal.feed(bytes);
            self.changed.notify_all();
        }
    }

    pub fn finish(&self, status: i32) {
        if let Ok(mut terminal) = self.terminal.lock() {
            terminal.finish(status);
            self.changed.notify_all();
        }
    }

    pub fn resize(&self, grid: Grid) {
        if let Ok(mut terminal) = self.terminal.lock() {
            terminal.resize(grid);
            self.changed.notify_all();
        }
    }

    fn issue_lease(&self) -> Option<u64> {
        self.terminal
            .lock()
            .ok()
            .map(|mut terminal| terminal.issue_lease())
    }

    fn resize_for(
        &self,
        grid: Grid,
        lease: u64,
        generation: u64,
        resize_pty: impl FnOnce() -> Result<(), String>,
    ) -> Result<bool, String> {
        let mut terminal = self
            .terminal
            .lock()
            .map_err(|_| "terminal screen lock failed")?;
        let lease_is_current = lease == terminal.lease;
        let lease_is_new = lease > terminal.lease && lease <= terminal.next_lease;
        if (!lease_is_current && !lease_is_new)
            || (lease_is_current && generation < terminal.generation)
        {
            return Ok(false);
        }
        resize_pty()?;
        let accepted = terminal.resize_for(grid, lease, generation);
        self.changed.notify_all();
        Ok(accepted)
    }

    fn screen_for(&self, since: u64, lease: u64, generation: u64) -> Option<Screen> {
        self.terminal
            .lock()
            .ok()
            .and_then(|mut terminal| terminal.screen_for(since, lease, generation))
    }

    fn accepts_lease(&self, lease: u64) -> bool {
        self.terminal
            .lock()
            .is_ok_and(|terminal| terminal.lease == lease)
    }

    #[must_use]
    pub fn screen(&self, since: u64) -> Screen {
        self.terminal.lock().map_or_else(
            |_| Screen {
                seq: 0,
                rows: Vec::new(),
                ended: true,
                exit: Some(1),
            },
            |mut terminal| terminal.screen(since),
        )
    }
}

/// Arguments for one non-interactive `kobo stream` invocation.
#[derive(Clone, Debug)]
pub struct Options {
    pub grid: Grid,
    pub controls: bool,
    pub interactive: bool,
    pub port: u16,
    pub command: Vec<String>,
}

impl Options {
    #[must_use]
    pub const fn input_mode(&self) -> InputMode {
        if self.interactive {
            InputMode::Full
        } else if self.controls {
            InputMode::Controls
        } else {
            InputMode::None
        }
    }
}

/// Runs the command and serves the current terminal screen over TLS.
///
/// The host and reader both attach to the one PTY, so full-screen programs
/// receive a real controlling terminal, grid, job control, and input bytes.
pub fn run(options: Options) -> Result<i32, String> {
    if options.command.is_empty() {
        return Err("kobo stream needs a command after --".to_owned());
    }
    let identity = Identity::load()?;
    let tls = kobo_net::serve::TlsServer::from_pem(&identity.certificate, &identity.key)?;
    let listener = TcpListener::bind(("0.0.0.0", options.port))
        .map_err(|error| format!("bind 0.0.0.0:{}: {error}", options.port))?;
    listener
        .set_nonblocking(true)
        .map_err(|error| format!("configure listener: {error}"))?;
    let arguments: Vec<&str> = options.command[1..].iter().map(String::as_str).collect();
    let environment = host_environment();
    let environment_refs = environment
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect::<Vec<_>>();
    let pty = kobo_abi::pty::Pty::spawn(
        &options.command[0],
        &arguments,
        &environment_refs,
        options.grid.columns,
        options.grid.rows,
    )
    .map_err(|error| format!("start {} in a terminal: {error}", options.command[0]))?;
    let input = Arc::new(Mutex::new(pty));
    let session = Arc::new(Session::new(options.grid));
    let mut raw_stdin = RawStdin::enable();
    forward_stdin(Arc::clone(&input));
    let session_id = random_session()?;
    let title = options.command[0].clone();
    let mode = options.input_mode();
    let mut exit_code = None;
    let mut ended_at = None;
    let readers = Arc::new(AtomicUsize::new(0));
    loop {
        if ended_at.is_none() {
            let output_eof = drain_pty(&input, &session);
            if exit_code.is_none() {
                exit_code = input
                    .lock()
                    .map_err(|_| "terminal lock failed")?
                    .finished()
                    .map_err(|error| format!("wait for command: {error}"))?;
                if exit_code.is_some() {
                    drop(raw_stdin.take());
                }
            }
            if let Some(code) = exit_code.filter(|_| output_eof) {
                session.finish(code);
                ended_at = Some(Instant::now());
            }
        }
        if ended_at.is_some_and(|when| when.elapsed() >= FINAL_SCREEN_FOR) {
            break;
        }
        match listener.accept() {
            Ok((socket, _)) => {
                if readers
                    .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                        (count < MAX_READERS).then_some(count + 1)
                    })
                    .is_err()
                {
                    continue;
                }
                let slot = ReaderSlot(Arc::clone(&readers));
                let session = Arc::clone(&session);
                let code = identity.pairing.clone();
                let input = Arc::clone(&input);
                let title = title.clone();
                socket
                    .set_nonblocking(false)
                    .map_err(|error| format!("configure Paperterm reader: {error}"))?;
                let _ = socket.set_read_timeout(Some(Duration::from_secs(10)));
                let _ = socket.set_write_timeout(Some(Duration::from_secs(30)));
                let tls = tls.accept(socket)?;
                std::thread::spawn(move || {
                    let _slot = slot;
                    let mut stream = tls;
                    if let Err(error) = route(
                        &mut stream,
                        &code,
                        session_id,
                        &title,
                        mode,
                        &input,
                        &session,
                    ) {
                        eprintln!("Paperterm connection ended: {error}");
                    }
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => return Err(format!("accept Paperterm reader: {error}")),
        }
    }
    Ok(session.screen(0).exit.unwrap_or(0))
}

/// The deliberately small part of the host environment a terminal program
/// needs to behave like it was launched from the owner's shell.
///
/// `Pty` clears its environment because its other caller runs untrusted Kobo
/// applications. A host command is different: without `PATH` a command such as
/// `claude` cannot be found, and without `HOME` its own configuration cannot be
/// found. Credentials and unrelated application variables are not copied.
fn host_environment() -> Vec<(String, String)> {
    host_environment_with(|name| std::env::var(name).ok())
}

fn host_environment_with(mut read: impl FnMut(&str) -> Option<String>) -> Vec<(String, String)> {
    const INHERITED: &[&str] = &[
        "HOME",
        "PATH",
        "USER",
        "LOGNAME",
        "SHELL",
        "LANG",
        "LC_ALL",
        "LC_CTYPE",
        "TMPDIR",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_CACHE_HOME",
    ];
    let mut environment = vec![("TERM".to_owned(), "xterm-256color".to_owned())];
    environment.extend(
        INHERITED
            .iter()
            .filter_map(|name| read(name).map(|value| ((*name).to_owned(), value))),
    );
    environment
}

struct ReaderSlot(Arc<AtomicUsize>);

impl Drop for ReaderSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Restores the caller's terminal even when the child exits unexpectedly.
struct RawStdin(String);

impl RawStdin {
    fn enable() -> Option<Self> {
        let saved = Command::new("stty").arg("-g").output().ok()?;
        if !saved.status.success() {
            return None;
        }
        let state = String::from_utf8(saved.stdout).ok()?.trim().to_owned();
        (!state.is_empty()
            && Command::new("stty")
                .args(["raw", "-echo"])
                .status()
                .is_ok_and(|status| status.success()))
        .then_some(Self(state))
    }
}

impl Drop for RawStdin {
    fn drop(&mut self) {
        let _ignored = Command::new("stty").arg(&self.0).status();
    }
}

fn forward_stdin(pty: Arc<Mutex<kobo_abi::pty::Pty>>) {
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin().lock();
        let mut bytes = [0_u8; 1024];
        while let Ok(read) = stdin.read(&mut bytes) {
            if read == 0 {
                break;
            }
            if pty
                .lock()
                .is_ok_and(|mut terminal| terminal.write(&bytes[..read]).is_ok())
            {
                continue;
            }
            break;
        }
    });
}

/// Drains whatever the PTY reader has already delivered without holding the
/// input lock while waiting. Device key writes therefore never wait behind
/// output from a noisy full-screen application.
fn drain_pty(pty: &Mutex<kobo_abi::pty::Pty>, session: &Session) -> bool {
    let mut drained = Vec::new();
    let disconnected = {
        let Ok(terminal) = pty.lock() else {
            return false;
        };
        loop {
            match terminal.output().try_recv() {
                Ok(bytes) => drained.push(bytes),
                Err(std::sync::mpsc::TryRecvError::Empty) => break false,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break true,
            }
        }
    };
    for bytes in drained {
        let _ = std::io::stdout().write_all(&bytes);
        session.feed(&bytes);
    }
    disconnected
}

/// A stream identity, intentionally separate from Sidekick's identity.
pub struct Identity {
    certificate: String,
    key: String,
    pairing: String,
}
impl Identity {
    pub fn load() -> Result<Self, String> {
        let directory = identity_dir()?;
        let read = |name| {
            std::fs::read_to_string(directory.join(name))
                .map_err(|_| format!("no {name} in {}; run kobo stream init", directory.display()))
        };
        Ok(Self {
            certificate: read("cert.pem")?,
            key: read("key.pem")?,
            pairing: read("pairing")?.trim().to_owned(),
        })
    }
}

/// Initialises a CA, leaf certificate, and pairing code for the stream host.
pub fn init(hosts: &[String]) -> Result<(), String> {
    let mut requested_hosts = Vec::new();
    let mut arguments = hosts.iter();
    while let Some(argument) = arguments.next() {
        if argument != "--host" {
            return Err(format!(
                "unknown argument '{argument}'; expected --host ADDRESS"
            ));
        }
        requested_hosts.push(
            arguments
                .next()
                .ok_or("--host needs an address")?
                .to_owned(),
        );
    }
    let directory = identity_dir()?;
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("create {}: {error}", directory.display()))?;
    let ca = directory.join("ca-cert.pem");
    let key = directory.join("ca-key.pem");
    if !ca.exists() || !key.exists() {
        let output = Command::new("openssl")
            .args([
                "req", "-x509", "-newkey", "rsa:2048", "-sha256", "-days", "3650", "-nodes",
                "-keyout",
            ])
            .arg(&key)
            .arg("-out")
            .arg(&ca)
            .args([
                "-subj",
                "/CN=kobo-stream authority",
                "-addext",
                "basicConstraints=critical,CA:TRUE",
            ])
            .output()
            .map_err(|error| format!("run openssl: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "openssl refused the stream authority: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
    }
    let leaf = directory.join("cert.pem");
    let leaf_key = directory.join("key.pem");
    if !leaf.exists() || !leaf_key.exists() || !requested_hosts.is_empty() {
        let request = directory.join("leaf.csr");
        let extensions = directory.join("leaf.ext");
        let serial = directory.join("ca-cert.srl");
        let mut names = vec!["IP:127.0.0.1".to_owned()];
        names.extend(requested_hosts.iter().map(|host| {
            if host.parse::<std::net::IpAddr>().is_ok() {
                format!("IP:{host}")
            } else {
                format!("DNS:{host}")
            }
        }));
        std::fs::write(
            &extensions,
            format!(
                "subjectAltName={}\nbasicConstraints=CA:FALSE\nkeyUsage=digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\n",
                names.join(",")
            ),
        )
        .map_err(|error| format!("write leaf extensions: {error}"))?;
        let request_output = Command::new("openssl")
            .args(["req", "-newkey", "rsa:2048", "-nodes", "-keyout"])
            .arg(&leaf_key)
            .arg("-out")
            .arg(&request)
            .args(["-subj", "/CN=kobo-stream"])
            .output()
            .map_err(|error| format!("run openssl: {error}"))?;
        if !request_output.status.success() {
            return Err(format!(
                "openssl refused the stream leaf: {}",
                String::from_utf8_lossy(&request_output.stderr)
            ));
        }
        let signed = Command::new("openssl")
            .args([
                "x509",
                "-req",
                "-sha256",
                "-days",
                "3650",
                "-CAcreateserial",
            ])
            .arg("-in")
            .arg(&request)
            .arg("-CA")
            .arg(&ca)
            .arg("-CAkey")
            .arg(&key)
            .arg("-CAserial")
            .arg(&serial)
            .arg("-extfile")
            .arg(&extensions)
            .arg("-out")
            .arg(&leaf)
            .output()
            .map_err(|error| format!("run openssl: {error}"))?;
        let _ = std::fs::remove_file(request);
        let _ = std::fs::remove_file(extensions);
        if !signed.status.success() {
            return Err(format!(
                "openssl refused to sign the stream leaf: {}",
                String::from_utf8_lossy(&signed.stderr)
            ));
        }
        let authority =
            std::fs::read_to_string(&ca).map_err(|error| format!("read authority: {error}"))?;
        let certificate =
            std::fs::read_to_string(&leaf).map_err(|error| format!("read stream leaf: {error}"))?;
        std::fs::write(&leaf, format!("{certificate}{authority}"))
            .map_err(|error| format!("write stream chain: {error}"))?;
    }
    let pairing_path = directory.join("pairing");
    let pairing = std::fs::read_to_string(&pairing_path)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or(pairing_code()?);
    std::fs::write(&pairing_path, &pairing).map_err(|error| format!("write pairing: {error}"))?;
    let trust = config_dir()?.join("trust");
    std::fs::create_dir_all(&trust)
        .map_err(|error| format!("create {}: {error}", trust.display()))?;
    std::fs::copy(&ca, trust.join("stream.pem"))
        .map_err(|error| format!("install stream trust root: {error}"))?;
    let address = requested_hosts
        .first()
        .cloned()
        .unwrap_or_else(|| "your-computer".to_owned());
    println!("Paperterm is initialised.\n\n  address       {address}:{DEFAULT_PORT}\n  pairing code  {}\n\nNext: kobo trust set stream --device READER_IP", std::fs::read_to_string(pairing_path).unwrap_or_default().trim());
    Ok(())
}

fn config_dir() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or("no HOME in the environment")?;
    Ok(PathBuf::from(home).join(".config").join("kobo"))
}
fn identity_dir() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("stream"))
}
fn pairing_code() -> Result<String, String> {
    const ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
    let mut bytes = [0_u8; 6];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .map_err(|error| format!("read randomness: {error}"))?;
    Ok(bytes
        .iter()
        .map(|byte| ALPHABET[usize::from(*byte) % ALPHABET.len()] as char)
        .collect())
}
fn route(
    stream: &mut kobo_net::serve::TlsStream,
    pairing: &str,
    session_id: u64,
    title: &str,
    mode: InputMode,
    input: &Mutex<kobo_abi::pty::Pty>,
    session: &Session,
) -> Result<(), String> {
    let request = read_request(stream)?;
    let token = query(&request.target, "token");
    if token.as_deref() != Some(pairing) {
        return respond(stream, 403, "{}");
    }
    match (request.method.as_str(), request.path()) {
        ("GET", "/lease") => {
            let lease = session.issue_lease().ok_or("terminal screen lock failed")?;
            respond(stream, 200, &format!(r#"{{"lease":{lease}}}"#))
        }
        ("GET", "/hello") => {
            let grid = query(&request.target, "grid")
                .ok_or_else(|| "hello omitted the terminal grid".to_owned())
                .and_then(|value| Grid::parse(&value))?;
            let generation = query(&request.target, "generation")
                .and_then(|value| value.parse().ok())
                .unwrap_or(0);
            let lease = query(&request.target, "lease")
                .and_then(|value| value.parse().ok())
                .unwrap_or(0);
            let accepted = session.resize_for(grid, lease, generation, || {
                input
                    .lock()
                    .map_err(|_| "terminal input lock failed")?
                    .resize(grid.columns, grid.rows)
                    .map_err(|error| format!("resize terminal: {error}"))
            })?;
            if !accepted {
                return respond(stream, 409, r#"{"stale":true}"#);
            }
            respond(
                stream,
                200,
                &format!(
                    r#"{{"session":{session_id},"grid":"{}","title":{},"input":"{}"}}"#,
                    grid.words(),
                    json(title),
                    mode.wire()
                ),
            )
        }
        ("GET", "/screen") => {
            let received = query(&request.target, "session").and_then(|value| value.parse().ok());
            if received != Some(session_id) {
                return respond(stream, 409, r#"{"restart":true}"#);
            }
            let since = query(&request.target, "seq")
                .and_then(|value| value.parse().ok())
                .unwrap_or(0);
            let generation = query(&request.target, "generation")
                .and_then(|value| value.parse().ok())
                .unwrap_or(0);
            let lease = query(&request.target, "lease")
                .and_then(|value| value.parse().ok())
                .unwrap_or(0);
            let started = Instant::now();
            let Some(mut screen) = session.screen_for(since, lease, generation) else {
                return respond(stream, 409, r#"{"stale":true}"#);
            };
            while screen.rows.is_empty() && !screen.ended && started.elapsed() < LONGEST_POLL {
                std::thread::sleep(Duration::from_millis(100));
                let Some(current) = session.screen_for(since, lease, generation) else {
                    return respond(stream, 409, r#"{"stale":true}"#);
                };
                screen = current;
            }
            respond(stream, 200, &screen_json(&screen))
        }
        ("POST", "/keys") => {
            let lease = query(&request.target, "lease")
                .and_then(|value| value.parse().ok())
                .unwrap_or(0);
            if !session.accepts_lease(lease) {
                return respond(stream, 409, r#"{"stale":true}"#);
            }
            let body = String::from_utf8_lossy(&request.body);
            let parsed = kobo_json::parse(&body).map_err(|_| "invalid keys JSON")?;
            if parsed.get("session").and_then(kobo_json::Value::as_i64) != Some(session_id as i64) {
                return respond(stream, 409, "{}");
            }
            let bytes = parsed
                .get("bytes_b64")
                .and_then(kobo_json::Value::as_str)
                .and_then(decode_base64)
                .ok_or("invalid key bytes")?;
            if !permits_input(mode, &bytes) {
                return respond(stream, 403, r#"{"accepted":false}"#);
            }
            let mut terminal = input.lock().map_err(|_| "terminal input lock failed")?;
            terminal
                .write(&bytes)
                .map_err(|error| format!("write terminal input: {error}"))?;
            respond(stream, 200, r#"{"accepted":true}"#)
        }
        _ => respond(stream, 404, "{}"),
    }
}

struct Request {
    method: String,
    target: String,
    body: Vec<u8>,
}
impl Request {
    fn path(&self) -> &str {
        self.target
            .split_once('?')
            .map_or(&self.target, |(path, _)| path)
    }
}
fn read_request(stream: &mut impl Read) -> Result<Request, String> {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 1024];
    let end = loop {
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break index;
        }
        if bytes.len() > MAX_REQUEST {
            return Err("request too large".to_owned());
        }
        let count = stream.read(&mut chunk).map_err(|error| error.to_string())?;
        if count == 0 {
            return Err("closed request".to_owned());
        }
        bytes.extend_from_slice(&chunk[..count]);
    };
    let head = String::from_utf8_lossy(&bytes[..end]);
    let mut lines = head.lines();
    let mut words = lines.next().ok_or("empty request")?.split_whitespace();
    let method = words.next().ok_or("no method")?.to_owned();
    let target = words.next().ok_or("no target")?.to_owned();
    let length = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    if length > MAX_REQUEST {
        return Err("request body too large".to_owned());
    }
    let mut body = bytes[end + 4..].to_vec();
    while body.len() < length {
        let count = stream.read(&mut chunk).map_err(|error| error.to_string())?;
        if count == 0 {
            return Err("closed request body".to_owned());
        }
        body.extend_from_slice(&chunk[..count]);
    }
    body.truncate(length);
    Ok(Request {
        method,
        target,
        body,
    })
}
fn query(target: &str, wanted: &str) -> Option<String> {
    target
        .split_once('?')?
        .1
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(name, _)| *name == wanted)
        .map(|(_, value)| value.to_owned())
}
fn respond(stream: &mut impl Write, status: u16, body: &str) -> Result<(), String> {
    write!(stream, "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).map_err(|error| error.to_string())?;
    stream.flush().map_err(|error| error.to_string())
}
fn json(text: &str) -> String {
    format!("{text:?}")
}
fn screen_json(screen: &Screen) -> String {
    let rows = screen
        .rows
        .iter()
        .map(|row| {
            format!(
                r#"{{"y":{},"cells":{},"cursor":{}}}"#,
                row.y,
                json(&row.cells),
                row.cursor
                    .map_or_else(|| "null".to_owned(), |column| column.to_string())
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!(
        r#"{{"seq":{},"rows":[{rows}],"ended":{},"exit":{}}}"#,
        screen.seq,
        screen.ended,
        screen
            .exit
            .map_or_else(|| "null".to_owned(), |exit| exit.to_string())
    )
}
fn visible_row(screen: &vt100::Screen, row: u16, columns: u16) -> String {
    let mut text = String::with_capacity(usize::from(columns));
    for column in 0..columns {
        let Some(cell) = screen.cell(row, column) else {
            break;
        };
        if cell.is_wide_continuation() {
            continue;
        }
        if !cell.has_contents() {
            text.push(' ');
            continue;
        }
        text.push(substitute_cell(cell.contents()));
        if cell.is_wide() {
            // Paperterm deliberately renders a one-cell replacement glyph.
            // Keep its terminal continuation cell so following ASCII stays in
            // the column the host TUI assigned it.
            text.push(' ');
        }
    }
    text.truncate(text.trim_end().len());
    text
}

fn substitute_cell(text: &str) -> char {
    let character = text.chars().next().unwrap_or(' ');
    match character {
        '\u{2580}'..='\u{259f}' => '#',
        '\u{2800}'..='\u{28ff}' => '·',
        character if character > '\u{7f}' && !('\u{2500}'..='\u{257f}').contains(&character) => '·',
        character => character,
    }
}
fn random_session() -> Result<u64, String> {
    // The device's intentionally small JSON parser stores numbers as f64.
    // Keep opaque identifiers inside the largest exactly representable integer
    // so the value sent back on /screen and /keys is byte-for-byte equivalent.
    const MAX_EXACT_JSON_INTEGER: u64 = (1_u64 << 53) - 1;
    let mut bytes = [0_u8; 8];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .map_err(|error| format!("read randomness: {error}"))?;
    Ok((u64::from_le_bytes(bytes) & MAX_EXACT_JSON_INTEGER).max(1))
}
fn decode_base64(text: &str) -> Option<Vec<u8>> {
    const A: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    if text.len() % 4 != 0 {
        return None;
    }
    let mut out = Vec::new();
    for group in text.as_bytes().chunks(4) {
        let value = |byte| {
            A.bytes()
                .position(|candidate| candidate == byte)
                .map(|value| value as u32)
        };
        let a = value(group[0])?;
        let b = value(group[1])?;
        let c = if group[2] == b'=' {
            0
        } else {
            value(group[2])?
        };
        let d = if group[3] == b'=' {
            0
        } else {
            value(group[3])?
        };
        out.push((a << 2 | b >> 4) as u8);
        if group[2] != b'=' {
            out.push((b << 4 | c >> 2) as u8);
        }
        if group[3] != b'=' {
            out.push((c << 6 | d) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reader_slot_is_released_when_a_connection_thread_ends() {
        let readers = Arc::new(AtomicUsize::new(1));
        {
            let _slot = ReaderSlot(Arc::clone(&readers));
            assert_eq!(readers.load(Ordering::Acquire), 1);
        }
        assert_eq!(readers.load(Ordering::Acquire), 0);
    }

    #[test]
    fn controls_are_a_closed_whitelist() {
        assert!(permits_input(InputMode::Controls, b"\x1b[A"));
        assert!(permits_input(InputMode::Controls, b"\x03"));
        assert!(permits_input(InputMode::Controls, b"\x1b[A\r\x03yn"));
        assert!(!permits_input(InputMode::Controls, b"rm -rf /"));
        assert!(!permits_input(InputMode::None, b"y"));
    }

    #[test]
    fn host_terminal_gets_shell_paths_but_not_ambient_credentials() {
        let environment = host_environment_with(|name| match name {
            "HOME" => Some("/home/reader".to_owned()),
            "PATH" => Some("/opt/tools:/usr/bin".to_owned()),
            "ANTHROPIC_API_KEY" => Some("must-not-leak".to_owned()),
            _ => None,
        });
        assert!(environment.contains(&("TERM".to_owned(), "xterm-256color".to_owned())));
        assert!(environment.contains(&("HOME".to_owned(), "/home/reader".to_owned())));
        assert!(environment.contains(&("PATH".to_owned(), "/opt/tools:/usr/bin".to_owned())));
        assert!(environment
            .iter()
            .all(|(name, _)| name != "ANTHROPIC_API_KEY"));
    }

    #[test]
    fn rows_diff_after_terminal_bytes_and_substitute_braille() {
        let session = Session::new(Grid {
            columns: 12,
            rows: 2,
        });
        assert_eq!(session.screen(0).rows.len(), 2);
        session.feed(b"ok \xe2\xa0\x80");
        assert!(
            session.screen(1).rows.is_empty(),
            "a new frame cannot arrive before the 2 Hz snapshot"
        );
        std::thread::sleep(SNAPSHOT_INTERVAL);
        let changed = session.screen(1);
        assert_eq!(changed.rows.len(), 1);
        assert_eq!(changed.rows[0].cells.trim(), "ok ·");
        assert!(session.screen(changed.seq).rows.is_empty());
    }

    #[test]
    fn substituted_wide_cells_keep_following_columns_aligned() {
        let session = Session::new(Grid {
            columns: 8,
            rows: 1,
        });
        session.feed("好X".as_bytes());
        std::thread::sleep(SNAPSHOT_INTERVAL);
        let screen = session.screen(0);
        assert_eq!(screen.rows[0].cells, "· X");
        assert_eq!(screen.rows[0].cursor, Some(3));
    }

    #[test]
    fn combining_marks_do_not_create_phantom_terminal_cells() {
        let session = Session::new(Grid {
            columns: 8,
            rows: 1,
        });
        session.feed("e\u{301}X".as_bytes());
        std::thread::sleep(SNAPSHOT_INTERVAL);
        let screen = session.screen(0);
        assert_eq!(screen.rows[0].cells, "eX");
        assert_eq!(screen.rows[0].cursor, Some(2));
    }

    #[test]
    fn cursor_only_moves_dirty_the_old_and_new_cursor_rows() {
        let session = Session::new(Grid {
            columns: 8,
            rows: 2,
        });
        session.feed(b"abc");
        std::thread::sleep(SNAPSHOT_INTERVAL);
        let before = session.screen(0);
        session.feed(b"\x1b[D");
        std::thread::sleep(SNAPSHOT_INTERVAL);
        let moved = session.screen(before.seq);
        assert!(moved.seq > before.seq);
        assert_eq!(moved.rows.len(), 1);
        assert_eq!(moved.rows[0].cells, "abc");
        assert_eq!(moved.rows[0].cursor, Some(2));

        session.feed(b"\r\n");
        std::thread::sleep(SNAPSHOT_INTERVAL);
        let next_row = session.screen(moved.seq);
        assert_eq!(
            next_row.rows.iter().map(|row| row.y).collect::<Vec<_>>(),
            vec![0, 1]
        );
        assert_eq!(next_row.rows[0].cursor, None);
        assert_eq!(next_row.rows[1].cursor, Some(0));
    }

    #[test]
    fn alternate_screen_returns_to_the_primary_screen_cleanly() {
        let session = Session::new(Grid {
            columns: 12,
            rows: 2,
        });
        session.feed(b"primary");
        std::thread::sleep(SNAPSHOT_INTERVAL);
        let primary = session.screen(0);
        session.feed(b"\x1b[?1049hsecondary");
        std::thread::sleep(SNAPSHOT_INTERVAL);
        let alternate = session.screen(primary.seq);
        assert_eq!(alternate.rows[0].cells, "secondary");
        session.feed(b"\x1b[?1049l");
        std::thread::sleep(SNAPSHOT_INTERVAL);
        let restored = session.screen(alternate.seq);
        assert_eq!(restored.rows[0].cells, "primary");
    }

    #[test]
    fn multiple_terminal_updates_share_one_snapshot() {
        let session = Session::new(Grid {
            columns: 12,
            rows: 2,
        });
        let first = session.screen(0).seq;
        session.feed(b"first");
        session.feed(b"\rsecond");
        std::thread::sleep(SNAPSHOT_INTERVAL);
        let frame = session.screen(first);
        assert_eq!(frame.rows.len(), 1);
        assert_eq!(frame.rows[0].cells.trim(), "second");
    }

    #[test]
    fn resizing_changes_the_terminal_viewport_and_forces_a_full_frame() {
        let session = Session::new(Grid {
            columns: 12,
            rows: 2,
        });
        session.feed(b"before");
        std::thread::sleep(SNAPSHOT_INTERVAL);
        let before = session.screen(0);
        session.resize(Grid {
            columns: 20,
            rows: 3,
        });
        let resized = session.screen(before.seq);
        assert_eq!(resized.rows.len(), 3);
        assert_eq!(resized.rows[0].cells.trim(), "before");
    }

    #[test]
    fn obsolete_poll_generation_cannot_consume_the_post_resize_snapshot() {
        let session = Session::new(Grid {
            columns: 12,
            rows: 2,
        });
        session.feed(b"before");
        let before = session.screen(0);
        let resized = session
            .resize_for(
                Grid {
                    columns: 20,
                    rows: 3,
                },
                0,
                1,
                || Ok(()),
            )
            .expect("resize");
        assert!(resized);
        assert!(session.screen_for(before.seq, 0, 0).is_none());

        let current = session
            .screen_for(before.seq, 0, 1)
            .expect("current generation snapshot");
        assert_eq!(current.rows.len(), 3);
        assert_eq!(current.rows[0].cells.trim(), "before");
        assert!(!session
            .resize_for(
                Grid {
                    columns: 12,
                    rows: 2,
                },
                0,
                0,
                || panic!("stale resize reached the PTY"),
            )
            .expect("stale resize verdict"));
    }

    #[test]
    fn fresh_lease_resets_generations_and_rejects_delayed_old_epoch_resize() {
        let session = Session::new(Grid {
            columns: 12,
            rows: 2,
        });
        let first = session.issue_lease().expect("first lease");
        assert!(session
            .resize_for(
                Grid {
                    columns: 20,
                    rows: 3,
                },
                first,
                9,
                || Ok(()),
            )
            .expect("first resize"));
        let relaunched = session.issue_lease().expect("relaunch lease");
        assert!(relaunched > first);
        assert!(session
            .resize_for(
                Grid {
                    columns: 30,
                    rows: 4,
                },
                relaunched,
                0,
                || Ok(()),
            )
            .expect("relaunch resize"));
        assert!(!session
            .resize_for(
                Grid {
                    columns: 10,
                    rows: 1,
                },
                first,
                u64::MAX,
                || panic!("old epoch reached the PTY"),
            )
            .expect("old epoch verdict"));
        assert!(session.screen_for(0, first, u64::MAX).is_none());
        let current = session
            .screen_for(0, relaunched, 0)
            .expect("relaunched screen");
        assert_eq!(current.rows.len(), 4);
    }

    #[test]
    fn hidden_open_closed_resize_emits_one_coherent_full_tui_frame_each_time() {
        let hidden = Grid {
            columns: 40,
            rows: 10,
        };
        let open = Grid {
            columns: 40,
            rows: 6,
        };
        let session = Session::new(hidden);
        session.feed(
            b"\x1b[2J\x1b[HQuick safety check\r\n\
              Security guide\r\n\
              1. Yes, I trust this folder\r\n\
              2. No, exit\r\n\
              Enter to confirm",
        );
        let first = session.screen(0);
        assert_eq!(
            first
                .rows
                .iter()
                .filter(|row| row.cells.contains("Quick safety check"))
                .count(),
            1
        );

        session.resize(open);
        let smaller = session.screen(first.seq);
        assert_eq!(smaller.rows.len(), usize::from(open.rows));
        assert_eq!(
            smaller
                .rows
                .iter()
                .filter(|row| row.cells.contains("Quick safety check"))
                .count(),
            1
        );

        session.resize(hidden);
        let expanded = session.screen(smaller.seq);
        assert_eq!(expanded.rows.len(), usize::from(hidden.rows));
        assert_eq!(
            expanded
                .rows
                .iter()
                .filter(|row| row.cells.contains("Quick safety check"))
                .count(),
            1
        );
        assert!(expanded
            .rows
            .iter()
            .skip(usize::from(open.rows))
            .all(|row| row.cells.trim().is_empty()));
    }

    #[test]
    fn pty_output_keeps_terminal_escape_sequences_for_the_screen_model() {
        let mut pty = kobo_abi::pty::Pty::spawn(
            "/bin/sh",
            &["-c", "printf '\\033[2Jready'"],
            &[("TERM", "xterm-256color")],
            12,
            2,
        )
        .expect("start a PTY command");
        let bytes = pty
            .output()
            .recv_timeout(Duration::from_secs(2))
            .expect("PTY output");
        let session = Session::new(Grid {
            columns: 12,
            rows: 2,
        });
        session.feed(&bytes);
        std::thread::sleep(SNAPSHOT_INTERVAL);
        assert_eq!(session.screen(0).rows[0].cells.trim(), "ready");
        let _ = pty.finished().expect("reap PTY command");
    }

    #[test]
    fn child_exit_drains_pty_eof_and_exposes_uncapped_final_output() {
        let pty = kobo_abi::pty::Pty::spawn(
            "/bin/sh",
            &["-c", "printf first; sleep 1; printf final"],
            &[("TERM", "xterm-256color")],
            20,
            2,
        )
        .expect("start final-output command");
        let input = Mutex::new(pty);
        let session = Session::new(Grid {
            columns: 20,
            rows: 2,
        });
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut before = None;
        let mut exit = None;
        loop {
            let eof = drain_pty(&input, &session);
            if before.is_none() {
                let screen = session.screen(0);
                if screen.rows.iter().any(|row| row.cells.contains("first")) {
                    before = Some(screen.seq);
                }
            }
            if exit.is_none() {
                exit = input
                    .lock()
                    .expect("terminal lock")
                    .finished()
                    .expect("wait for command");
            }
            if let Some(code) = exit.filter(|_| eof) {
                session.finish(code);
                break;
            }
            assert!(Instant::now() < deadline, "PTY did not reach EOF");
            std::thread::sleep(Duration::from_millis(10));
        }

        let final_screen = session.screen(before.expect("initial output snapshot"));
        assert!(final_screen.ended);
        assert_eq!(final_screen.exit, Some(0));
        assert!(final_screen
            .rows
            .iter()
            .any(|row| row.cells.contains("firstfinal")));
    }

    #[test]
    fn pty_accepts_control_input_without_waiting_for_a_snapshot() {
        let mut pty = kobo_abi::pty::Pty::spawn(
            "/bin/sh",
            &["-c", "read answer; printf 'answer:%s' \"$answer\""],
            &[("TERM", "xterm-256color")],
            24,
            2,
        )
        .expect("start PTY command");
        pty.write(b"yes\r").expect("write terminal input");
        let output = pty
            .output()
            .recv_timeout(Duration::from_secs(2))
            .expect("PTY echoed the answer");
        assert!(String::from_utf8_lossy(&output).contains("yes"));
        let _ = pty.finished().expect("reap PTY command");
    }
    #[test]
    fn grid_rejects_ambiguous_or_unusable_values() {
        assert_eq!(
            Grid::parse("100x35"),
            Ok(Grid {
                columns: 100,
                rows: 35
            })
        );
        assert!(Grid::parse("100").is_err());
        assert!(Grid::parse("0x35").is_err());
    }
    #[test]
    fn base64_is_bounded_before_input_is_accepted() {
        assert_eq!(decode_base64("Gw=="), Some(vec![27]));
        assert_eq!(decode_base64("bad"), None);
    }

    #[test]
    fn session_identifiers_survive_the_device_json_number_model() {
        const MAX_EXACT_JSON_INTEGER: u64 = (1_u64 << 53) - 1;
        let body = format!(r#"{{"session":{MAX_EXACT_JSON_INTEGER}}}"#);
        let parsed = kobo_json::parse(&body).expect("session JSON");
        assert_eq!(
            parsed.get("session").and_then(kobo_json::Value::as_i64),
            Some(MAX_EXACT_JSON_INTEGER as i64)
        );
    }
}

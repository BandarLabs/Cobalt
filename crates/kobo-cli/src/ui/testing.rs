//! A whole server on a real port, a computer made of temporary folders, and
//! a client that speaks to it in raw bytes.
//!
//! The rules under test are about exactly what is on the wire: a header
//! that is missing, said twice, or one character off. A client library
//! would tidy those away before they were sent, so the tests write every
//! request by hand and read every response as text.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use kobo_app_store::{
    build_bundle, derive_public_key, sign, Catalog, CatalogEntry, CatalogEntryInput, Manifest,
    ManifestInput,
};
use kobo_protocol::DeviceError;

use super::api::Machine;
use super::server::{Limits, Patience, Server, Shared, Stopped};
use super::shopfront::{Fetch, Pictures, Shopfront};
use super::surroundings::Room;
use crate::setup;

const SEED: [u8; 32] = [7; 32];

/// A Clara BW on the firmware Cobalt is tested on.
pub const CLARA_BW: &str =
    "N365410043013,4.9.77,4.45.23697,4.9.77,4.9.77,00000000-0000-0000-0000-000000000391";

/// A Clara HD on a firmware branch nobody measured it on.
pub const CLARA_HD_NEWER: &str =
    "N249123456789,4.9.77,4.45.23640,4.9.77,4.9.77,00000000-0000-0000-0000-000000000376";

/// A reader on the firmware generation Cobalt cannot be started from.
pub const FIRMWARE_FIVE: &str =
    "N365410043013,5.1.0,5.2.190625,5.1.0,5.1.0,00000000-0000-0000-0000-000000000391";

/// The folders a test computer is made of, and what was done to it.
pub struct Desk {
    pub root: PathBuf,
    /// Volumes the test has plugged in.
    pub plugged: Arc<Mutex<Vec<PathBuf>>>,
    /// Volumes that were ejected, in order.
    pub ejected: Arc<Mutex<Vec<PathBuf>>>,
    /// Addresses the catalog transport was asked for.
    pub fetched: Arc<Mutex<Vec<String>>>,
    /// Set to take this computer off the network.
    pub offline: Arc<AtomicBool>,
    /// Set to have the catalog arrive with one byte changed.
    pub tampered: Arc<AtomicBool>,
    /// What every volume answers when asked how much room it has.
    pub room: Arc<Mutex<Room>>,
    /// Drives the system is refusing to let this process read.
    pub blocked: Arc<Mutex<Vec<PathBuf>>>,
    /// How many Kobos the USB bus reports, when it reports.
    pub attached: Arc<Mutex<Option<usize>>>,
    /// The pictures the site publishes, by address, to add to or take from.
    pub pictures: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
}

impl Drop for Desk {
    /// Takes the folders away with the test. A server started on them may
    /// still be answering on another thread, which costs it nothing: every
    /// test has made its assertions by the time its desk is dropped.
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

impl Desk {
    /// Makes a reader's book partition under this desk and plugs it in.
    /// `cobalt` is the release installed on it, when one is.
    pub fn plug(&self, name: &str, version_line: &str, cobalt: Option<&str>) -> PathBuf {
        let volume = self.root.join(name);
        fs::create_dir_all(volume.join(".kobo")).expect("system folder");
        fs::write(volume.join(".kobo/version"), format!("{version_line}\n")).expect("version");
        if let Some(version) = cobalt {
            let root = volume.join(setup::INSTALL_FOLDER);
            fs::create_dir_all(&root).expect("install folder");
            fs::write(root.join("VERSION"), format!("{version}\n")).expect("VERSION");
        }
        self.plugged.lock().expect("plugged").push(volume.clone());
        volume
    }

    /// What the stand-in for the kobo command was run with, one line per
    /// run, or nothing when it never ran.
    pub fn commands(&self) -> Vec<String> {
        fs::read_to_string(self.root.join("kobo.log"))
            .map(|log| log.lines().map(str::to_owned).collect())
            .unwrap_or_default()
    }

    /// Every file and folder under `folder`, sorted, for telling whether
    /// anything at all was changed.
    pub fn listing(folder: &Path) -> Vec<String> {
        let mut found = Vec::new();
        let mut waiting = vec![folder.to_owned()];
        while let Some(next) = waiting.pop() {
            for entry in fs::read_dir(&next).into_iter().flatten().flatten() {
                let path = entry.path();
                let size = entry.metadata().map_or(0, |metadata| metadata.len());
                if path.is_dir() {
                    found.push(path.display().to_string());
                    waiting.push(path);
                } else {
                    found.push(format!("{} {size}", path.display()));
                }
            }
        }
        found.sort();
        found
    }
}

/// Where the site publishes the picture the store lists as `name`.
pub fn picture_address(name: &str) -> String {
    format!("https://bandarlabs.github.io/Cobalt/media/site/apps/{name}")
}

/// The start of a PNG that says it is `width` by `height`, which is as far
/// into a picture as this server ever reads. `mark` makes one differ from
/// another.
pub fn picture(width: u32, height: u32, mark: u8) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
    bytes.extend_from_slice(&width.to_be_bytes());
    bytes.extend_from_slice(&height.to_be_bytes());
    bytes.extend_from_slice(&[8, 0, 0, 0, 0, mark, mark, mark, mark]);
    bytes
}

/// The manifests the fixture store is made from. Almanac says everything a
/// manifest can; Tide Table says the least it may.
pub const MANIFESTS: &[&str] = &[
    r#"{
        "id": "almanac", "display_name": "Almanac", "short_label": "Almanac",
        "summary": "The year at a glance.",
        "page_description": "Sunrise, sunset and the phase of the moon for every day of the year.",
        "version": "1.0.0", "release_notes": "The moon is drawn the right way up.",
        "glyph": "note", "capabilities": ["network", "keep-awake"],
        "setup": { "steps": [
            { "text": "Find your town.", "link": { "label": "Towns", "url": "https://example.org/towns" } },
            { "text": "Save it.", "command": "kobo almanac town NAME" }
        ] }
    }"#,
    r#"{
        "id": "tide-table", "display_name": "Tide Table", "short_label": "Tides",
        "summary": "High and low water.", "version": "3.1.0",
        "glyph": "note", "capabilities": []
    }"#,
];

/// The listing that goes with [`MANIFESTS`]: two pictures of Almanac and
/// none of Tide Table.
pub const LISTING: &str = r#"{
    "groups": [
        { "id": "reading", "title": "Reading and listening", "apps": ["almanac"] },
        { "id": "everyday", "title": "Everyday tools", "apps": ["tide-table"] }
    ],
    "shots": {
        "almanac": [
            { "name": "almanac.png", "caption": "The year on one screen", "width": 1072, "height": 1448 },
            { "name": "almanac/moon.png", "caption": "Tonight's moon", "width": 1072, "height": 1448 }
        ],
        "tide-table": []
    },
    "release_note_overrides": {}
}"#;

/// Writes the stand-in for the kobo command under `root`: a script that
/// says how it was run, where, and what was beside it, then exits with the
/// status the file beside it asks for.
fn stand_in_command(root: &Path) -> PathBuf {
    let program = root.join("kobo");
    fs::write(
        &program,
        "#!/bin/sh\n\
         echo \"$*\" >> \"$0.log\"\n\
         echo \"ran: $*\"\n\
         echo \"beside: $(ls | tr '\\n' ' ')\"\n\
         echo 'kobo: said on the error stream' >&2\n\
         exit \"$(cat \"$0.status\" 2>/dev/null || echo 0)\"\n",
    )
    .expect("stand-in command");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).expect("executable");
    }
    program
}

/// The site the pictures are published on: it answers with what `pictures`
/// holds, notes each address in `asked`, and cannot be reached while
/// `offline` is set.
fn picture_site(
    pictures: &Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
    asked: &Arc<Mutex<Vec<String>>>,
    offline: &Arc<AtomicBool>,
) -> Box<Fetch> {
    let (pictures, asked, offline) = (Arc::clone(pictures), Arc::clone(asked), Arc::clone(offline));
    Box::new(move |url, _| {
        asked.lock().expect("fetched").push(url.to_owned());
        if offline.load(Ordering::SeqCst) {
            return Err(DeviceError::Unreachable);
        }
        let published = pictures.lock().expect("pictures");
        published.get(url).cloned().ok_or(DeviceError::NotFound)
    })
}

/// A computer made of folders: no readers until one is plugged in, a
/// catalog that publishes Almanac and Tide Table, a store that shows the
/// same two with pictures of the first, a release to install, and a
/// stand-in for the kobo command that records how it was run.
pub fn make_desk(name: &str) -> (Machine, Desk) {
    let root = std::env::temp_dir().join(format!("kobo-ui-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    for folder in ["scratch", "home", "config"] {
        fs::create_dir_all(root.join(folder)).expect("folder");
    }
    let program = stand_in_command(&root);
    let plugged: Arc<Mutex<Vec<PathBuf>>> = Arc::default();
    let ejected: Arc<Mutex<Vec<PathBuf>>> = Arc::default();
    let fetched: Arc<Mutex<Vec<String>>> = Arc::default();
    let answers = published(&[
        ("almanac", "Almanac", "1.0.0", "0.3.0"),
        ("tide-table", "Tide Table", "3.1.0", "0.3.0"),
        ("orrery", "Orrery", "2.0.0", "999.0.0"),
    ]);
    let (seen, asked, gone) = (
        Arc::clone(&plugged),
        Arc::clone(&fetched),
        Arc::clone(&ejected),
    );
    let offline = Arc::new(AtomicBool::new(false));
    let tampered = Arc::new(AtomicBool::new(false));
    let space = Arc::new(Mutex::new(Room {
        free: Some(1_000_000_000),
        writable: Some(true),
    }));
    let blocked: Arc<Mutex<Vec<PathBuf>>> = Arc::default();
    let attached: Arc<Mutex<Option<usize>>> = Arc::default();
    let (down, bent) = (Arc::clone(&offline), Arc::clone(&tampered));
    let (roomy, kept_out, on_the_bus) = (
        Arc::clone(&space),
        Arc::clone(&blocked),
        Arc::clone(&attached),
    );
    let pictures = Arc::new(Mutex::new(BTreeMap::from([
        (picture_address("almanac.png"), picture(1072, 1448, 1)),
        (picture_address("almanac/moon.png"), picture(1072, 1448, 2)),
    ])));
    let shopfront = Shopfront::from(MANIFESTS, LISTING).expect("fixture store");
    let kept = Pictures::open(root.join("cache"), shopfront.pictures());
    let machine = Machine {
        program,
        readers: Box::new(move || {
            seen.lock()
                .expect("plugged")
                .iter()
                .filter_map(|volume| setup::read_reader(volume))
                .collect()
        }),
        release: Ok("0.3.37".to_owned()),
        release_dir: None,
        scratch: root.join("scratch"),
        fetch: Box::new(move |url, _| {
            asked.lock().expect("fetched").push(url.to_owned());
            if down.load(Ordering::SeqCst) {
                return Err(DeviceError::Unreachable);
            }
            let mut answer = answers.get(url).cloned().ok_or(DeviceError::NotFound)?;
            if bent.load(Ordering::SeqCst) && url == kobod::app_store::CATALOG_URL {
                answer[10] ^= 1;
            }
            Ok(answer)
        }),
        shopfront,
        pictures: kept,
        picture: picture_site(&pictures, &fetched, &offline),
        key: derive_public_key(&SEED).expect("fixture key"),
        eject: Box::new(move |volume| {
            gone.lock().expect("ejected").push(volume.to_owned());
            Ok(())
        }),
        room: Box::new(move |_| *roomy.lock().expect("room")),
        blocked: Box::new(move || kept_out.lock().expect("blocked").clone()),
        attached: Box::new(move || *on_the_bus.lock().expect("attached")),
        network: Box::new(|| true),
        home: root.join("home"),
        config: root.join("config"),
        uploads: Mutex::default(),
        receiving: Mutex::default(),
        store: Mutex::default(),
    };
    (
        machine,
        Desk {
            root,
            plugged,
            ejected,
            fetched,
            offline,
            tampered,
            room: space,
            blocked,
            attached,
            pictures,
        },
    )
}

/// A stable catalog signed with the fixture key, as the addresses it is
/// published at and the bytes each answers with.
fn published(offers: &[(&str, &str, &str, &str)]) -> BTreeMap<String, Vec<u8>> {
    let mut answers = BTreeMap::new();
    let mut entries = Vec::new();
    for (id, title, version, minimum) in offers {
        let binary = format!("{id} {version} binary").into_bytes();
        let manifest = Manifest::new(
            ManifestInput {
                id: (*id).to_owned(),
                display_name: (*title).to_owned(),
                short_label: (*title).to_owned(),
                summary: format!("The {title} fixture."),
                version: (*version).to_owned(),
                minimum_cobalt_version: (*minimum).to_owned(),
                glyph: "note".to_owned(),
                capabilities: Vec::new(),
                binary_sha256: crate::sha256::hex_digest(&binary),
                binary_bytes: binary.len() as u64,
            },
            kobo_app_store::public_reserved_app_ids(),
        )
        .expect("manifest");
        let package = build_bundle(&manifest, &binary, &SEED).expect("bundle");
        let url = format!("https://fixtures.invalid/{id}.cobalt-app");
        entries.push(
            CatalogEntry::new(CatalogEntryInput {
                manifest,
                package_url: url.clone(),
                package_sha256: crate::sha256::hex_digest(&package),
                package_bytes: package.len() as u64,
            })
            .expect("entry"),
        );
        answers.insert(url, package);
    }
    let catalog = Catalog::new(entries).expect("catalog").to_canonical_bytes();
    let signature = format!("{}\n", sign(&catalog, &SEED).expect("signature")).into_bytes();
    answers.insert(kobod::app_store::CATALOG_URL.to_owned(), catalog);
    answers.insert(
        kobod::app_store::CATALOG_SIGNATURE_URL.to_owned(),
        signature,
    );
    answers
}

/// A value as it is written in a query, so that a path with a character the
/// query gives meaning to still arrives as the same path.
pub fn query_value(text: &str) -> String {
    use std::fmt::Write as _;
    let mut written = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
            written.push(char::from(byte));
        } else {
            let _ = write!(written, "%{byte:02X}");
        }
    }
    written
}

/// Waits that suit a test: long enough that a loaded machine does not trip
/// them, short enough that a test of the wait itself finishes.
pub fn quick() -> Patience {
    Patience {
        read: Duration::from_secs(5),
        write: Duration::from_secs(5),
        linger: Duration::from_secs(30),
        idle: Duration::from_secs(60),
    }
}

/// A server that is answering.
pub struct Running {
    pub port: u16,
    pub shared: Arc<Shared>,
    stopped: Receiver<Stopped>,
}

impl Running {
    /// Starts a server for `machine` and leaves it answering.
    pub fn start(machine: Machine, limits: Limits, patience: Patience) -> Self {
        Self::start_with(machine, limits, patience, || false)
    }

    /// The same, with something that says when the window's program exits.
    pub fn start_with(
        machine: Machine,
        limits: Limits,
        patience: Patience,
        shell_closed: impl FnMut() -> bool + Send + 'static,
    ) -> Self {
        let server = Server::bind(machine, limits, patience).expect("bind");
        let shared = Arc::clone(&server.shared);
        let port = shared.session.port;
        let (said, stopped) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = said.send(server.serve(shell_closed));
        });
        Self {
            port,
            shared,
            stopped,
        }
    }

    /// Why the server stopped, if it has.
    pub fn stopped(&self) -> Option<Stopped> {
        match self.stopped.try_recv() {
            Ok(why) => Some(why),
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => None,
        }
    }

    /// Waits up to `patience` for the server to stop.
    pub fn stops_within(&self, patience: Duration) -> Option<Stopped> {
        self.stopped.recv_timeout(patience).ok()
    }

    /// The host this server answers to.
    pub fn host(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }

    /// The origin of this server's own page.
    pub fn origin(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Opens the page the way a browser does: follows the launch address,
    /// keeps the cookie, and reads the request token out of the page.
    pub fn open(&self) -> Page<'_> {
        let address = self
            .shared
            .session
            .launch_address()
            .expect("an unspent launch address");
        let path = address
            .strip_prefix(&self.origin())
            .expect("the address is this server's");
        let exchange = self.send(&format!(
            "GET {path} HTTP/1.1\r\nHost: {}\r\n\r\n",
            self.host()
        ));
        assert_eq!(exchange.status, 303, "{}", exchange.body);
        let cookie = exchange
            .header("set-cookie")
            .and_then(|value| value.split(';').next())
            .expect("a cookie")
            .to_owned();
        let page = self.send(&format!(
            "GET / HTTP/1.1\r\nHost: {}\r\nCookie: {cookie}\r\n\r\n",
            self.host()
        ));
        assert_eq!(page.status, 200, "{}", page.body);
        let token = page
            .body
            .split("name=\"kobo-token\" content=\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .expect("the page carries its token")
            .to_owned();
        Page {
            running: self,
            cookie,
            token,
        }
    }

    /// Asks as the Mac app asks: its own token, and nothing a browser sends.
    pub fn app(&self, method: &str, path: &str, body: &str) -> Reply {
        self.send(&format!(
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\n\
             Content-Length: {}\r\n\r\n{body}",
            self.host(),
            self.shared.session.app_token(),
            body.len()
        ))
    }

    /// Sends exactly these bytes and reads the whole answer.
    pub fn send(&self, request: &str) -> Reply {
        self.send_bytes(request.as_bytes())
    }

    /// The same for a request whose body is not text.
    pub fn send_bytes(&self, request: &[u8]) -> Reply {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(20)))
            .expect("timeout");
        // A refusal can arrive, and the connection close, while the body is
        // still being written. That is the server working, not the test
        // failing, so a failed write goes on to read what was said.
        let _ = stream.write_all(request);
        let mut answer = Vec::new();
        let _ = stream.read_to_end(&mut answer);
        Reply::parse(&answer)
    }
}

/// One response, as text.
#[derive(Debug)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Reply {
    fn parse(answer: &[u8]) -> Self {
        let text = String::from_utf8_lossy(answer);
        let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
        let mut lines = head.split("\r\n");
        let status = lines
            .next()
            .and_then(|line| line.split(' ').nth(1))
            .and_then(|code| code.parse().ok())
            .unwrap_or(0);
        Self {
            status,
            headers: lines
                .filter_map(|line| line.split_once(": "))
                .map(|(name, value)| (name.to_ascii_lowercase(), value.to_owned()))
                .collect(),
            body: body.to_owned(),
        }
    }

    /// One header's value, by lowercase name.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(known, _)| known == name)
            .map(|(_, value)| value.as_str())
    }

    /// The body as JSON.
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body)
            .unwrap_or_else(|error| panic!("not JSON ({error}): {:?}", self.body))
    }

    /// The kind a refusal said it was.
    pub fn kind(&self) -> String {
        self.json()["kind"].as_str().unwrap_or_default().to_owned()
    }

    /// The reason a refusal gave.
    pub fn error(&self) -> String {
        self.json()["error"].as_str().unwrap_or_default().to_owned()
    }
}

/// The page, open in a browser that holds the session.
pub struct Page<'a> {
    pub running: &'a Running,
    /// `kobo_ui=<value>`, as a `Cookie` header carries it.
    pub cookie: String,
    pub token: String,
}

impl Page<'_> {
    /// Asks a question.
    pub fn get(&self, path: &str) -> Reply {
        self.running.send(&format!(
            "GET {path} HTTP/1.1\r\nHost: {}\r\nCookie: {}\r\n\r\n",
            self.running.host(),
            self.cookie
        ))
    }

    /// Asks for a change, with everything such a request must carry.
    pub fn post(&self, path: &str, body: &str) -> Reply {
        self.post_bytes(path, body.as_bytes())
    }

    /// The same for a body that is a file.
    pub fn post_bytes(&self, path: &str, body: &[u8]) -> Reply {
        let mut request = format!(
            "POST {path} HTTP/1.1\r\nHost: {}\r\nCookie: {}\r\nOrigin: {}\r\n\
             Sec-Fetch-Site: same-origin\r\nX-Kobo-Token: {}\r\nContent-Length: {}\r\n\r\n",
            self.running.host(),
            self.cookie,
            self.running.origin(),
            self.token,
            body.len()
        )
        .into_bytes();
        request.extend_from_slice(body);
        self.running.send_bytes(&request)
    }

    /// Waits for a job to end and returns its last report.
    pub fn finished(&self, id: u64) -> serde_json::Value {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let report = self.get(&format!("/api/job?id={id}&from=0")).json();
            if report["running"] == false {
                return report;
            }
            assert!(
                Instant::now() < deadline,
                "the job never finished: {report}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

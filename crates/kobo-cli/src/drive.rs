//! Driving a running application and getting the panel back as a picture.
//!
//! # Why this exists
//!
//! Everything in this workspace could be tested except the one thing it is
//! for: what a reader actually sees. A layout assertion proves a button was
//! placed; it does not prove the screen reads as a product. The only way to
//! close that loop -- for a person, or for something automating on their
//! behalf -- is to drive the application the way a finger does and look at the
//! result.
//!
//! Both halves of that already existed and neither was reachable. The
//! simulator has run the device's own renderer and refresh planner since the
//! beginning, and it has served the frame over HTTP the whole time; what was
//! missing was a way to say "tap Search" rather than "tap 536, 912", and a way
//! to get 1.5 megabytes of raw grey out as something openable.
//!
//! # Why taps go through coordinates
//!
//! A driver that dispatched actions straight into the application would be
//! simpler and would be worthless. It would pass on a screen whose only button
//! had been laid out four millimetres below the bottom edge of the panel,
//! which is precisely the fault this is for catching. So a step names a
//! control, this resolves the name against the layout the renderer produced,
//! and then taps the middle of the rectangle it actually occupies -- through
//! the same touch transform and the same hit-testing a finger goes through.
//! If the control is not reachable, the tap misses, and the script fails.

use std::fmt::Write as _;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long to wait for the simulator to answer one request.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// How long `wait-for` will keep looking for something to appear.
const APPEAR_TIMEOUT: Duration = Duration::from_secs(10);

/// The panel every simulated frame comes back as.
/// How long to wait for the application to answer a tap, and how often to look.
///
/// Half a second in total. Long enough for a screen that has to be encoded,
/// sent over a socket, decoded and laid out; short enough that a script full of
/// deliberately inert taps does not become a script that takes a minute.
const SETTLE_POLLS: u32 = 25;
const SETTLE_INTERVAL: std::time::Duration = std::time::Duration::from_millis(20);

pub const SIMULATED_PANEL: (u32, u32) = (1072, 1448);
const FRAME_WIDTH: u32 = SIMULATED_PANEL.0;
const FRAME_HEIGHT: u32 = SIMULATED_PANEL.1;

/// How the device announces a screenshot inside its ordinary report.
const CAPTURE_HEADER: &str = "capture-begin";
const CAPTURE_FOOTER: &str = "capture-end";

/// One node of the layout the renderer produced.
#[derive(Clone, Debug)]
pub struct Control {
    pub kind: String,
    pub centre: (i32, i32),
    pub lines: Vec<String>,
    pub action: Option<u32>,
}

impl Control {
    /// Whether this node carries `needle` in any of its lines.
    ///
    /// Case-insensitive and by substring, because a label is routinely
    /// shortened to fit -- "Return to Kobo reader" becomes "Return to Kobo…"
    /// on a narrow panel, and a script that had to know which panel it was
    /// running on would be a script nobody kept up to date.
    fn says(&self, needle: &str) -> bool {
        let needle = needle.to_lowercase();
        self.lines
            .iter()
            .any(|line| line.to_lowercase().contains(&needle))
    }

    /// Whether one of this node's lines is exactly `needle`.
    fn says_exactly(&self, needle: &str) -> bool {
        self.lines
            .iter()
            .any(|line| line.trim().eq_ignore_ascii_case(needle.trim()))
    }

    /// Whether a finger on the middle of this node would do anything.
    const fn tappable(&self) -> bool {
        self.action.is_some()
    }
}

/// A connection to a running simulator.
pub struct Driver {
    address: String,
    shots: PathBuf,
    taken: usize,
    /// Take screenshots without the e-ink residue of earlier frames.
    ///
    /// The panel really does keep a ghost of what it last drew, and `/frame`
    /// is honest about it -- which is what you want when the question is
    /// whether a screen refreshes cleanly. It is precisely what you do not
    /// want when the question is whether a screen *reads* well, because two
    /// screens overlaid are unreadable to a person and worse to a model.
    ideal: bool,
}

impl Driver {
    pub fn new(address: &str, shots: &Path) -> Self {
        Self {
            address: address.to_owned(),
            shots: shots.to_owned(),
            taken: 0,
            ideal: false,
        }
    }

    /// Takes screenshots from the residue-free frame.
    #[must_use]
    pub const fn ideal(mut self, ideal: bool) -> Self {
        self.ideal = ideal;
        self
    }

    /// Runs one script, stopping at the first step that fails.
    ///
    /// # Errors
    ///
    /// Returns the failing step, its line number and why it failed. A failed
    /// step always takes a screenshot first, because the question that
    /// immediately follows "tap Search failed" is "what was on the screen".
    pub fn run_script(&mut self, script: &str) -> Result<(), String> {
        for (number, line) in script.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Err(error) = self.step(line) {
                // A screenshot of the failure, when one can still be had. If
                // the simulator is what failed, the attempt fails too, and
                // repeating its complaint inside the first one helps nobody.
                let where_to_look = self
                    .shot(&format!("failed-line-{}", number + 1))
                    .map_or_else(
                        |_| String::new(),
                        |path| format!(" (screen at {})", path.display()),
                    );
                return Err(format!(
                    "line {}: {line}: {error}{where_to_look}",
                    number + 1
                ));
            }
        }
        Ok(())
    }

    /// Runs one step.
    pub fn step(&mut self, line: &str) -> Result<(), String> {
        let (verb, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let rest = rest.trim();
        match verb {
            "tap" => self.tap(rest),
            "tap-at" => {
                let (x, y) = parse_point(rest)?;
                self.touch(x, y)
            }
            "type" => self.type_text(rest),
            "shot" => self.shot(rest).map(|path| {
                println!("shot {}", path.display());
            }),
            "expect" => self.expect(rest),
            "expect-missing" => {
                if self.find(rest)?.is_some() {
                    return Err(format!("{rest:?} is on the screen and should not be"));
                }
                Ok(())
            }
            "wait-for" => self.wait_for(rest),
            "clean" => self.clean(),
            "lifecycle" => self.post("/lifecycle", rest),
            "scenario" => self.post("/scenario", rest),
            "wait" => {
                let milliseconds: u64 = rest
                    .parse()
                    .map_err(|_| "wait takes a number of milliseconds".to_owned())?;
                std::thread::sleep(Duration::from_millis(milliseconds));
                Ok(())
            }
            "dump" => {
                for control in self.layout()? {
                    if !control.lines.is_empty() {
                        println!("{:>24}  {:?}", control.kind, control.lines);
                    }
                }
                Ok(())
            }
            other => Err(format!("unknown step {other:?}")),
        }
    }

    /// Taps the control whose label carries `label`.
    ///
    /// Four passes, narrowest first: a tappable node whose whole label is the
    /// word, then a tappable node that merely contains it, then the same two
    /// again for nodes that cannot be tapped at all.
    ///
    /// The order matters more than it looks. A plain substring search over
    /// every node finds prose before it finds controls, and prose is drawn
    /// first: a script that said `tap Close` on a screen whose body text
    /// contained the word "closes" tapped the paragraph, which does nothing,
    /// and then reported that the button underneath was broken. The last two
    /// passes are kept so that tapping a row or a tile by its title still
    /// works -- the tappable node there is the row, whose own lines are the
    /// title, but a picture or a label inside it may be what matched.
    fn tap(&mut self, label: &str) -> Result<(), String> {
        let controls = self.layout()?;
        let control = controls
            .iter()
            .find(|control| control.tappable() && control.says_exactly(label))
            .or_else(|| {
                controls
                    .iter()
                    .find(|control| control.tappable() && control.says(label))
            })
            .or_else(|| controls.iter().find(|control| control.says_exactly(label)))
            .or_else(|| controls.iter().find(|control| control.says(label)))
            .ok_or_else(|| format!("nothing on the screen says {label:?}"))?;
        let (x, y) = control.centre;
        self.touch(x, y)
    }

    /// Types by tapping the keys of the on-screen keyboard.
    ///
    /// There is no other way in, and that is correct: this device has no
    /// hardware keyboard, so a driver that injected text into the application
    /// would be exercising a path a reader can never take. A key is any
    /// tappable node whose whole label is the character -- the SDK has no
    /// keyboard node, because a keyboard, a calculator and a colour picker are
    /// all the same grid of one-word buttons, so there is nothing more
    /// specific to match on. If the character has no key on the layer showing,
    /// that is a finding, not an inconvenience, and it is reported as one.
    fn type_text(&mut self, text: &str) -> Result<(), String> {
        for character in text.chars() {
            let label = match character {
                ' ' => "Space".to_owned(),
                character => character.to_string(),
            };
            let key = self
                .layout()?
                .into_iter()
                .find(|control| {
                    control.action.is_some()
                        && control.lines.len() == 1
                        && control.lines[0].trim().eq_ignore_ascii_case(&label)
                })
                .ok_or_else(|| {
                    format!("no key for {label:?}; is the right keyboard layer showing?")
                })?;
            self.touch(key.centre.0, key.centre.1)?;
        }
        Ok(())
    }

    /// Asserts that something on the screen says this.
    fn expect(&mut self, text: &str) -> Result<(), String> {
        if self.find(text)?.is_some() {
            return Ok(());
        }
        Err(format!("nothing on the screen says {text:?}"))
    }

    /// The same, but allowing for work that is still in flight.
    fn wait_for(&mut self, text: &str) -> Result<(), String> {
        let deadline = Instant::now() + APPEAR_TIMEOUT;
        loop {
            if self.find(text)?.is_some() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "waited {}s and nothing on the screen said {text:?}",
                    APPEAR_TIMEOUT.as_secs()
                ));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Asserts the renderer raised no errors about this screen.
    ///
    /// Warnings are printed and not fatal. An error means something was
    /// clipped, unreachable, or drawn in a character the panel's face cannot
    /// set, and none of those are visible in a screenshot until somebody
    /// notices they are missing.
    fn clean(&mut self) -> Result<(), String> {
        let body = self.get("/diagnostics")?;
        let body = String::from_utf8_lossy(&body).into_owned();
        let errors = json_objects(&body)
            .into_iter()
            .filter(|issue| json_field(issue, "severity").as_deref() == Some("error"))
            .filter_map(|issue| json_field(&issue, "message"))
            .collect::<Vec<_>>();
        if errors.is_empty() {
            return Ok(());
        }
        Err(format!(
            "the renderer refused this screen: {}",
            errors.join("; ")
        ))
    }

    /// Writes the panel out as a PNG and returns where it went.
    fn shot(&mut self, name: &str) -> Result<PathBuf, String> {
        let frame = self.color_frame()?;
        let png = kobo_image::encode_png_rgb(FRAME_WIDTH, FRAME_HEIGHT, &frame)
            .map_err(|error| format!("encode the frame: {error}"))?;
        self.taken += 1;
        let name = if name.is_empty() {
            format!("{:03}", self.taken)
        } else {
            name.trim_end_matches(".png").replace(['/', ' '], "-")
        };
        std::fs::create_dir_all(&self.shots)
            .map_err(|error| format!("create {}: {error}", self.shots.display()))?;
        let path = self.shots.join(format!("{name}.png"));
        std::fs::write(&path, png).map_err(|error| format!("write {}: {error}", path.display()))?;
        Ok(path)
    }

    /// The first control saying `label`, if any.
    fn find(&self, label: &str) -> Result<Option<Control>, String> {
        Ok(self
            .layout()?
            .into_iter()
            .find(|control| control.says(label)))
    }

    /// The raw RGB bytes of the panel as the simulator has it.
    pub fn color_frame(&self) -> Result<Vec<u8>, String> {
        let frame = self.get(if self.ideal {
            "/ideal-frame-rgb"
        } else {
            "/frame-rgb"
        })?;
        let expected = (FRAME_WIDTH as usize)
            .saturating_mul(FRAME_HEIGHT as usize)
            .saturating_mul(3);
        if frame.len() != expected {
            return Err(format!(
                "the simulator sent {} bytes for a {FRAME_WIDTH}x{FRAME_HEIGHT} RGB panel, not {expected}",
                frame.len()
            ));
        }
        Ok(frame)
    }

    /// The raw grayscale bytes of the panel as the simulator has it.
    ///
    /// Kept for existing driver clients. The RGB endpoint is used for PNG
    /// captures so verified colour profiles retain their channels.
    #[allow(
        dead_code,
        reason = "a public compatibility method is not called by the CLI itself"
    )]
    pub fn frame(&self) -> Result<Vec<u8>, String> {
        let frame = self.get(if self.ideal { "/ideal-frame" } else { "/frame" })?;
        let expected = (FRAME_WIDTH as usize).saturating_mul(FRAME_HEIGHT as usize);
        if frame.len() != expected {
            return Err(format!(
                "the simulator sent {} bytes for a {FRAME_WIDTH}x{FRAME_HEIGHT} panel, not {expected}",
                frame.len()
            ));
        }
        Ok(frame)
    }

    /// Everything the renderer put on the panel.
    pub fn layout(&self) -> Result<Vec<Control>, String> {
        let body = self.get("/layout")?;
        let body = String::from_utf8_lossy(&body).into_owned();
        Ok(json_objects(&body)
            .into_iter()
            .map(|node| Control {
                kind: json_field(&node, "kind").unwrap_or_default(),
                centre: json_point(&node, "centre"),
                lines: json_array(&node, "lines"),
                action: json_number(&node, "\"action\"")
                    .and_then(|value| u32::try_from(value).ok()),
            })
            .collect())
    }

    /// Presses a point and waits for the application to answer.
    ///
    /// The wait is the important half. The tap is posted to the simulator and
    /// answered by the application in another process, so a script that read
    /// the layout back immediately read the screen it had just tapped on --
    /// and then reported that the button did nothing, which is a race that
    /// passes four runs in five and looks exactly like a real defect.
    ///
    /// A tap that draws nothing new is normal and not an error: the runtime
    /// drops a repaint identical to what is already displayed, and plenty of
    /// taps are meant to be inert. So this waits for a paint, and gives up
    /// quietly.
    fn touch(&self, x: i32, y: i32) -> Result<(), String> {
        let before = self.paints()?;
        self.post("/touch", &format!("x={x}&y={y}"))?;
        for _ in 0..SETTLE_POLLS {
            if self.paints()? != before {
                return Ok(());
            }
            std::thread::sleep(SETTLE_INTERVAL);
        }
        Ok(())
    }

    /// How many screens the application has painted so far.
    fn paints(&self) -> Result<u64, String> {
        let body = self.get("/layout")?;
        let body = String::from_utf8_lossy(&body).into_owned();
        Ok(json_number(&body, "\"paints\"")
            .and_then(|value| u64::try_from(value).ok())
            .unwrap_or_default())
    }

    fn get(&self, path: &str) -> Result<Vec<u8>, String> {
        self.request("GET", path, "")
    }

    fn post(&self, path: &str, body: &str) -> Result<(), String> {
        self.request("POST", path, body).map(|_| ())
    }

    /// One HTTP request, spoken by hand.
    ///
    /// No client library, deliberately. This CLI has exactly one dependency
    /// and the whole conversation is four verbs against a server in this same
    /// workspace; pulling in an async runtime to say `GET /frame` would be a
    /// worse trade than eighty lines of `write!`.
    fn request(&self, method: &str, path: &str, body: &str) -> Result<Vec<u8>, String> {
        let mut stream = TcpStream::connect(&self.address).map_err(|error| {
            format!(
                "connect to the simulator at {}: {error}\n\
                 start one with: kobo dev --builtin",
                self.address
            )
        })?;
        stream
            .set_read_timeout(Some(REQUEST_TIMEOUT))
            .and_then(|()| stream.set_write_timeout(Some(REQUEST_TIMEOUT)))
            .map_err(|error| format!("configure the connection: {error}"))?;
        let mut request = String::new();
        let _ = write!(
            &mut request,
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n",
            self.address
        );
        if method == "POST" {
            let _ = write!(
                &mut request,
                "Content-Type: text/plain\r\nContent-Length: {}\r\n",
                body.len()
            );
        }
        request.push_str("\r\n");
        request.push_str(body);
        stream
            .write_all(request.as_bytes())
            .map_err(|error| format!("send {method} {path}: {error}"))?;
        let mut answer = Vec::new();
        stream
            .read_to_end(&mut answer)
            .map_err(|error| format!("read the answer to {method} {path}: {error}"))?;
        split_response(&answer, path)
    }
}

/// Pulls the picture out of a device transcript.
///
/// The doctor prints its whole read-only report and then the panel, so this
/// has to find the picture inside prose rather than parse a file. The header
/// carries the width, the height and the byte count the device measured, so a
/// truncated transfer -- an SSH connection dropped halfway through two
/// megabytes is not a rare event -- is caught here rather than becoming a PNG
/// that is half a screenshot and half black.
///
/// # Errors
///
/// Returns an error when there is no capture in the transcript, when its
/// header is malformed, or when fewer bytes arrived than were announced.
#[allow(
    dead_code,
    reason = "a public compatibility function is not called by the CLI itself"
)]
pub fn decode_capture(transcript: &str) -> Result<(u32, u32, Vec<u8>), String> {
    let capture = decode_capture_frame(transcript)?;
    Ok((capture.width, capture.height, capture.pixels))
}

/// A framebuffer capture decoded from a `kobo-doctor` transcript.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Capture {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
    pub color: bool,
}

/// Decodes either a legacy grayscale capture or an RGB capture from a
/// verified Kobo Colour profile.
pub fn decode_capture_frame(transcript: &str) -> Result<Capture, String> {
    let mut lines = transcript.lines();
    let header = lines
        .find(|line| line.starts_with(CAPTURE_HEADER))
        .ok_or_else(|| {
            "the device sent no capture; is this build of kobo-doctor current?".to_owned()
        })?;
    let fields: Vec<&str> = header.split_whitespace().collect();
    let (width, height, length, color) = match fields.as_slice() {
        [_, width, height, length] | [_, width, height, length, "grey"] => {
            (*width, *height, *length, false)
        }
        [_, width, height, length, "rgb"] => (*width, *height, *length, true),
        _ => {
            return Err(format!(
                "the device sent a capture header we cannot read: {header:?}"
            ));
        }
    };
    let parse = |value: &str, what: &str| -> Result<u32, String> {
        value
            .parse::<u32>()
            .map_err(|_| format!("the device reported {what} as {value:?}"))
    };
    let width = parse(width, "the panel width")?;
    let height = parse(height, "the panel height")?;
    let announced = parse(length, "the capture length")? as usize;
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(if color { 3 } else { 1 }))
        .ok_or_else(|| "the device reported a capture too large to index".to_owned())?;
    if announced != expected {
        return Err(format!(
            "the device announced {announced} bytes for a {width}x{height} {} capture, not {expected}",
            if color { "RGB" } else { "grayscale" }
        ));
    }
    let mut pixels = Vec::with_capacity(announced);
    for line in lines {
        if line.starts_with(CAPTURE_FOOTER) {
            break;
        }
        pixels.extend(base64_decode(line.trim())?);
    }
    if pixels.len() != announced {
        return Err(format!(
            "the device announced {announced} bytes of screen and {} arrived; \
             the transfer was cut short",
            pixels.len()
        ));
    }
    Ok(Capture {
        width,
        height,
        pixels,
        color,
    })
}

/// Standard base64, padded.
#[allow(clippy::naive_bytecount)]
fn base64_decode(line: &str) -> Result<Vec<u8>, String> {
    let value_of = |character: u8| -> Option<u32> {
        match character {
            b'A'..=b'Z' => Some(u32::from(character - b'A')),
            b'a'..=b'z' => Some(u32::from(character - b'a') + 26),
            b'0'..=b'9' => Some(u32::from(character - b'0') + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    };
    let bytes = line.as_bytes();
    if bytes.len() % 4 != 0 {
        return Err("a line of the capture is not a whole number of base64 groups".to_owned());
    }
    let mut decoded = Vec::with_capacity(bytes.len() / 4 * 3);
    for group in bytes.chunks(4) {
        let padding = group.iter().filter(|byte| **byte == b'=').count();
        let mut packed = 0_u32;
        for byte in group {
            let sextet = if *byte == b'=' {
                0
            } else {
                value_of(*byte).ok_or_else(|| {
                    format!(
                        "the capture contains {:?}, which is not base64",
                        *byte as char
                    )
                })?
            };
            packed = (packed << 6) | sextet;
        }
        let triple = [
            u8::try_from((packed >> 16) & 0xff).unwrap_or(0),
            u8::try_from((packed >> 8) & 0xff).unwrap_or(0),
            u8::try_from(packed & 0xff).unwrap_or(0),
        ];
        decoded.extend_from_slice(&triple[..3 - padding]);
    }
    Ok(decoded)
}

/// The body of an HTTP response, with the status checked.
fn split_response(answer: &[u8], path: &str) -> Result<Vec<u8>, String> {
    let header_end = answer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| format!("{path}: the simulator sent no complete response"))?;
    let head = String::from_utf8_lossy(&answer[..header_end]);
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| format!("{path}: the simulator sent no status"))?;
    if !(200..300).contains(&status) {
        return Err(format!("{path}: the simulator answered {status}"));
    }
    Ok(answer[header_end + 4..].to_vec())
}

/// The objects directly inside the document's array, as their own strings.
///
/// A parser rather than a dependency, and a deliberately shallow one: the two
/// documents this reads are produced a few hundred lines away in this same
/// workspace, so the shapes are known -- an object holding one array of
/// objects. It counts braces and respects strings, which is all that is needed
/// and all that is claimed. It does not descend into a node's own nested
/// objects, so a node comes back whole.
fn json_objects(body: &str) -> Vec<String> {
    let mut objects = Vec::new();
    let mut inside_array = false;
    let mut depth = 0;
    let mut start = 0;
    let mut in_string = false;
    let mut escaped = false;
    for (index, character) in body.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }
        match character {
            '"' => in_string = true,
            '[' if !inside_array => inside_array = true,
            ']' if inside_array && depth == 0 => break,
            '{' if inside_array => {
                if depth == 0 {
                    start = index;
                }
                depth += 1;
            }
            '}' if inside_array && depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    objects.push(body[start..=index].to_owned());
                }
            }
            _ => {}
        }
    }
    objects
}

/// The string value of `field`, unescaped.
fn json_field(object: &str, field: &str) -> Option<String> {
    let key = format!("\"{field}\":\"");
    let start = object.find(&key)? + key.len();
    let mut value = String::new();
    let mut escaped = false;
    for character in object[start..].chars() {
        if escaped {
            value.push(match character {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                other => other,
            });
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == '"' {
            return Some(value);
        } else {
            value.push(character);
        }
    }
    None
}

/// The strings of a `"field":[...]` array.
fn json_array(object: &str, field: &str) -> Vec<String> {
    let key = format!("\"{field}\":[");
    let Some(start) = object.find(&key).map(|at| at + key.len()) else {
        return Vec::new();
    };
    let Some(end) = object[start..].find(']').map(|at| at + start) else {
        return Vec::new();
    };
    let mut lines = Vec::new();
    let mut rest = &object[start..end];
    while let Some(open) = rest.find('"') {
        let mut value = String::new();
        let mut escaped = false;
        let mut consumed = open + 1;
        for character in rest[open + 1..].chars() {
            consumed += character.len_utf8();
            if escaped {
                value.push(match character {
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    other => other,
                });
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                break;
            } else {
                value.push(character);
            }
        }
        lines.push(value);
        rest = &rest[consumed..];
    }
    lines
}

/// The `x` and `y` of a nested `"field":{"x":..,"y":..}`.
///
/// Scoped to the nested object rather than searched for across the whole node,
/// because a node carries its own top-level `x` and `y` as well and a search
/// for `"y"` finds the wrong one -- which is a tap that lands on the right
/// column and the wrong row, and passes for as long as the two happen to agree.
fn json_point(object: &str, field: &str) -> (i32, i32) {
    let key = format!("\"{field}\":{{");
    let Some(start) = object.find(&key).map(|at| at + key.len()) else {
        return (0, 0);
    };
    let Some(end) = object[start..].find('}').map(|at| at + start) else {
        return (0, 0);
    };
    let inner = &object[start..end];
    (
        json_number(inner, "\"x\"")
            .and_then(|x| i32::try_from(x).ok())
            .unwrap_or(0),
        json_number(inner, "\"y\"")
            .and_then(|y| i32::try_from(y).ok())
            .unwrap_or(0),
    )
}

/// The number following `key`, which is given with its quotes already on so a
/// nested path can be matched without a real parser.
/// Reads a whole number, wide enough for every number the layout carries.
///
/// `i64` rather than `i32` because an action is a `u32` hash and rather more
/// than half of them are larger than `i32::MAX`. Parsing those into an `i32`
/// failed, the control came back with no action, and the driver reported the
/// key as missing from the keyboard: `q` and `o` could not be typed while `h`
/// could, which reads as a layout bug and is arithmetic.
fn json_number(object: &str, key: &str) -> Option<i64> {
    let start = object.find(key)? + key.len();
    let rest = object[start..].trim_start_matches([':', '"']);
    let digits: String = rest
        .chars()
        .take_while(|character| character.is_ascii_digit() || *character == '-')
        .collect();
    digits.parse().ok()
}

fn parse_point(text: &str) -> Result<(i32, i32), String> {
    let (x, y) = text
        .split_once([',', ' '])
        .ok_or_else(|| "tap-at takes 'x,y'".to_owned())?;
    let x = x
        .trim()
        .parse()
        .map_err(|_| "tap-at takes whole numbers".to_owned())?;
    let y = y
        .trim()
        .parse()
        .map_err(|_| "tap-at takes whole numbers".to_owned())?;
    Ok((x, y))
}

#[cfg(test)]
mod tests {
    use super::{
        base64_decode, decode_capture, decode_capture_frame, json_array, json_field, json_number,
        json_objects, json_point, split_response,
    };

    const BODY: &str = r#"{"nodes":[{"kind":"Button","x":10,"y":20,"width":30,"height":40,"centre":{"x":25,"y":40},"action":77,"lines":["Search","for a \"book\""]},{"kind":"Divider","x":0,"y":1,"width":2,"height":3,"centre":{"x":1,"y":2},"action":null,"lines":[]}]}"#;

    #[test]
    fn the_layout_reader_finds_every_node_and_its_words() {
        let nodes = json_objects(BODY);
        assert_eq!(nodes.len(), 2);
        assert_eq!(json_field(&nodes[0], "kind").as_deref(), Some("Button"));
        assert_eq!(
            json_array(&nodes[0], "lines"),
            vec!["Search".to_owned(), "for a \"book\"".to_owned()],
            "an escaped quote inside a label must not end the label"
        );
        assert_eq!(
            json_point(&nodes[0], "centre"),
            (25, 40),
            "the centre must come from the centre, not from the node's own x and y"
        );
        assert_eq!(json_number(&nodes[0], "\"action\""), Some(77));
        assert!(json_array(&nodes[1], "lines").is_empty());
    }

    /// An action is a `u32` hash, so most of them do not fit an `i32`. Reading
    /// one into an `i32` silently produced a control with no action, and the
    /// driver then reported the key as absent from the keyboard.
    #[test]
    fn an_action_larger_than_an_i32_is_still_an_action() {
        let body = r#"{"nodes":[{"kind":"CellLabel","centre":{"x":882,"y":527},"action":2633322323,"lines":["o"]}]}"#;
        let node = &json_objects(body)[0];
        assert_eq!(json_number(node, "\"action\""), Some(2_633_322_323));
        assert_eq!(
            json_number(node, "\"action\"").and_then(|value| u32::try_from(value).ok()),
            Some(2_633_322_323),
            "the key would be untypable"
        );
    }

    #[test]
    fn a_brace_inside_a_label_does_not_end_the_node() {
        let body =
            r#"{"nodes":[{"kind":"Text","lines":["a } brace"]},{"kind":"Rule","lines":[]}]}"#;
        assert_eq!(
            json_objects(body).len(),
            2,
            "a closing brace inside a string is a character, not structure"
        );
    }

    #[test]
    fn a_refused_request_is_reported_rather_than_parsed() {
        let answer = b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 3\r\n\r\nno!";
        assert!(split_response(answer, "/touch").is_err());
        let answer = b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n";
        assert_eq!(split_response(answer, "/touch"), Ok(Vec::new()));
    }
    #[test]
    fn the_decoder_agrees_with_the_standard_at_every_remainder() {
        assert_eq!(base64_decode("Zg==").as_deref(), Ok(&b"f"[..]));
        assert_eq!(base64_decode("Zm8=").as_deref(), Ok(&b"fo"[..]));
        assert_eq!(base64_decode("Zm9v").as_deref(), Ok(&b"foo"[..]));
        assert_eq!(
            base64_decode("AP+A").as_deref(),
            Ok(&[0x00, 0xff, 0x80][..])
        );
        assert!(
            base64_decode("Zm9").is_err(),
            "a partial group is a truncated transfer"
        );
        assert!(base64_decode("Zm9!").is_err());
    }

    #[test]
    fn a_capture_is_found_inside_the_doctors_ordinary_report() {
        let transcript = concat!(
            "Kobo doctor 0.1.0\n",
            "framebuffer: id=mxc 1072x1448\n",
            "capture-begin 2 3 6\n",
            "AAECAwQF\n",
            "capture-end\n"
        );
        assert_eq!(
            decode_capture(transcript),
            Ok((2, 3, vec![0, 1, 2, 3, 4, 5]))
        );
    }

    #[test]
    fn a_colour_capture_retains_its_format_and_channels() {
        let transcript = "capture-begin 2 1 6 rgb\n/wAAgDJA\ncapture-end\n";
        assert_eq!(
            decode_capture_frame(transcript),
            Ok(super::Capture {
                width: 2,
                height: 1,
                pixels: vec![255, 0, 0, 128, 50, 64],
                color: true,
            })
        );
        assert_eq!(
            decode_capture(transcript),
            Ok((2, 1, vec![255, 0, 0, 128, 50, 64]))
        );
    }

    #[test]
    fn a_transfer_cut_short_is_refused_rather_than_saved_as_half_a_picture() {
        let transcript = "capture-begin 2 3 6\nAAEC\ncapture-end\n";
        let error = decode_capture(transcript).expect_err("three bytes is not six");
        assert!(error.contains("cut short"), "{error}");
    }

    #[test]
    fn a_report_with_no_capture_says_so_rather_than_producing_an_empty_screen() {
        assert!(decode_capture("Kobo doctor 0.1.0\n").is_err());
    }
}

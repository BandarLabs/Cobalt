#![forbid(unsafe_code)]

//! Versioned, bounded wire format used between Kobo applications and hosts.

use std::fmt;
use std::io::{self, Read, Write};

use kobo_ui::{
    ActionId, BannerLevel, BarAction, Cell, Freeform, Glyph, NavBar, Node, NodeId, PageTurns,
    Percent, Row, Screen, Space, Tile, TopBar, MIN_NAV_DESTINATIONS,
};
use std::cmp::min;

pub const MAGIC: [u8; 4] = *b"KOBO";
pub const VERSION: u8 = 1;
pub const HEADER_LEN: usize = 14;
pub const MAX_FRAME_LEN: usize = 1_048_576;
pub const MAX_STRING_LEN: usize = 16_384;
pub const MAX_NODES: usize = 512;
const MAX_DEPTH: usize = 16;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Frame {
    pub request_id: u32,
    pub message: Message,
}

/// The longest a URL may be, so a catalog entry cannot be used to blow the
/// frame budget on its own.
pub const MAX_URL_LEN: usize = 2048;

/// The most a single task may hand back in one frame.
///
/// A task that needs more than this is downloading a file, which belongs on
/// disk rather than in memory on a device with this much of it.
pub const MAX_TASK_BYTES_U32: u32 = 512 * 1024;

/// The same limit as [`MAX_TASK_BYTES_U32`], in the width Rust indexes with.
/// Declaring the wire width first means the conversion only ever widens.
pub const MAX_TASK_BYTES: usize = MAX_TASK_BYTES_U32 as usize;

/// A handle to work the runtime is carrying out on an application's behalf.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TaskId(pub u32);

/// Work an application can ask the runtime to perform off the event loop.
///
/// Deliberately a closed set rather than a closure. An application does not get
/// to run arbitrary code on a background thread, because a thread it owns is a
/// thread that can outlive the screen, hold the radio open, or keep the device
/// awake after the reader has walked away.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Task {
    /// Fetches a URL. The application never opens a socket; the runtime
    /// resolves, connects, enforces TLS, applies the byte ceiling and decides
    /// whether the permission was granted in the first place.
    Fetch {
        url: String,
        /// Where to start reading, as a byte offset.
        ///
        /// A long document is read in pieces rather than refused: the largest
        /// response this transport carries is smaller than a great many of the
        /// books these applications exist to read.
        offset: u32,
        max_bytes: u32,
    },
    /// Sends a body to a URL. The application supplies the body and, when the
    /// request needs a credential, the *name* of one — never its value. The
    /// runtime looks the named secret up and attaches it, so an API key is
    /// never in an application's memory, its logs or its crash dump.
    Post {
        url: String,
        body: String,
        content_type: String,
        /// The name of a secret the runtime holds, or `None`.
        secret: Option<String>,
        max_bytes: u32,
    },
    /// Reads a file from the application's own directory.
    ReadFile { path: String },
    /// Waits, without holding a wake lock.
    Sleep { seconds: u32 },
}

/// How a task ended.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TaskOutcome {
    Completed(Vec<u8>),
    Failed(TaskError),
    /// The application asked for this through [`Context::cancel`].
    Cancelled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskError {
    /// The application does not hold the capability the task requires.
    Denied,
    Unreachable,
    /// The response exceeded the ceiling the task itself declared.
    TooLarge,
    TimedOut,
    NotFound,
}

impl fmt::Display for TaskError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Denied => "the application does not hold this permission",
            Self::Unreachable => "the network could not be reached",
            Self::TooLarge => "the response was larger than the limit the task declared",
            Self::TimedOut => "the task ran out of time",
            Self::NotFound => "not found",
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Message {
    Hello {
        name: String,
    },
    Welcome {
        width: u16,
        height: u16,
        /// The panel's density, so an application can measure text exactly as
        /// the runtime will lay it out.
        ///
        /// Without this an application knows how many pixels it has but not
        /// how large they are, and every size in the UI layer is derived from
        /// a physical measurement. A reader deciding where to break a page
        /// would have to assume a panel, which is the one thing this platform
        /// does not do.
        pixels_per_inch: u16,
    },
    SetScreen(Screen),
    Action {
        action: ActionId,
    },
    Log {
        level: LogLevel,
        message: String,
    },
    Exit,
    /// An application asking the runtime to hand the panel to another one.
    ///
    /// The name is an identity, not a path. Resolving it is the runtime's job,
    /// because an application that could name a path could start anything on
    /// the device; naming an entry in a catalogue it does not control is the
    /// whole of the privilege.
    Launch {
        name: String,
    },
    /// An application asking the runtime to do something with the hardware.
    DeviceRequest(DeviceRequest),
    /// The runtime's answer to exactly one device request.
    DeviceResult(DeviceResult),
    /// An application handing work to the runtime so its event loop keeps
    /// running.
    Spawn {
        task: TaskId,
        work: Task,
    },
    Cancel {
        task: TaskId,
    },
    /// The runtime reporting how exactly one task ended.
    TaskOutcome {
        task: TaskId,
        outcome: TaskOutcome,
    },
}

/// Every hardware operation an application can ask for.
///
/// Applications never open a device node. They describe an intent, the runtime
/// decides whether to honour it, and the answer is always explicit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceRequest {
    /// Report battery percentage and whether the device is charging.
    ReadBattery,
    /// Keep Wi-Fi associated for at most this many seconds.
    HoldWifi { seconds: u32 },
    /// Release a Wi-Fi hold early.
    ReleaseWifi,
    /// Keep the device out of suspend for at most this many seconds.
    KeepAwake { seconds: u32 },
    /// Release a wake hold early.
    AllowSleep,
    /// Ask to be woken again after this many seconds.
    ScheduleWake { seconds: u32 },
    /// Cancel a pending scheduled wake.
    CancelWake,
    /// Set the front light to a percentage.
    SetFrontlight { percent: u8 },
    /// Report the current front light percentage.
    ReadFrontlight,
}

/// The runtime's answer to a device request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceResult {
    /// The request was carried out and needs no value.
    Done,
    /// A time-bounded request was granted, possibly for less than was asked.
    Granted { seconds: u32 },
    /// Battery state.
    Battery { percent: u8, charging: bool },
    /// Front light state.
    Frontlight { percent: u8 },
    /// The request was refused, with the exact reason.
    Denied(DenyReason),
}

/// Why a device request was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum DenyReason {
    /// The application did not declare the capability in its manifest.
    NotDeclared = 1,
    /// The capability is declared but withheld because the battery is low.
    WithheldForBattery = 2,
    /// This runtime cannot do it on this hardware yet.
    Unsupported = 3,
    /// The request was well formed but outside what policy allows at all.
    PolicyRejected = 4,
    /// Another application currently owns this exclusive resource.
    Busy = 5,
}

impl DenyReason {
    #[must_use]
    pub fn describe(self) -> &'static str {
        match self {
            Self::NotDeclared => "the application did not declare this capability",
            Self::WithheldForBattery => "withheld because the battery is low",
            Self::Unsupported => "not supported by this runtime on this hardware",
            Self::PolicyRejected => "refused by system policy",
            Self::Busy => "another application holds this resource",
        }
    }
}

impl fmt::Display for DenyReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.describe())
    }
}

impl TryFrom<u8> for DenyReason {
    type Error = ProtocolError;

    fn try_from(value: u8) -> Result<Self, ProtocolError> {
        match value {
            1 => Ok(Self::NotDeclared),
            2 => Ok(Self::WithheldForBattery),
            3 => Ok(Self::Unsupported),
            4 => Ok(Self::PolicyRejected),
            5 => Ok(Self::Busy),
            _ => Err(ProtocolError::InvalidValue("deny reason")),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum LogLevel {
    Debug = 1,
    Info = 2,
    Warn = 3,
    Error = 4,
}

impl TryFrom<u8> for LogLevel {
    type Error = ProtocolError;

    fn try_from(value: u8) -> Result<Self, ProtocolError> {
        match value {
            1 => Ok(Self::Debug),
            2 => Ok(Self::Info),
            3 => Ok(Self::Warn),
            4 => Ok(Self::Error),
            _ => Err(ProtocolError::InvalidValue("log level")),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProtocolError {
    Truncated,
    BadMagic,
    UnsupportedVersion(u8),
    UnknownMessageType(u8),
    FrameTooLarge,
    LengthMismatch,
    InvalidUtf8,
    StringTooLarge,
    TooManyNodes,
    TooDeep,
    InvalidValue(&'static str),
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for ProtocolError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StreamError {
    Io(io::ErrorKind),
    Protocol(ProtocolError),
}

impl fmt::Display for StreamError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(kind) => write!(formatter, "stream I/O error: {kind}"),
            Self::Protocol(error) => write!(formatter, "protocol error: {error}"),
        }
    }
}

impl std::error::Error for StreamError {}

impl From<ProtocolError> for StreamError {
    fn from(error: ProtocolError) -> Self {
        Self::Protocol(error)
    }
}

impl From<io::Error> for StreamError {
    fn from(error: io::Error) -> Self {
        Self::Io(error.kind())
    }
}

/// # Errors
///
/// Returns an error when a message exceeds protocol limits.
pub fn encode(frame: &Frame) -> Result<Vec<u8>, ProtocolError> {
    let (kind, payload_len) = encoded_message_layout(&frame.message)?;
    let mut payload = Vec::with_capacity(payload_len);
    match &frame.message {
        Message::Hello { name } => {
            push_string(&mut payload, name)?;
        }
        Message::Welcome {
            width,
            height,
            pixels_per_inch,
        } => {
            push_u16(&mut payload, *width);
            push_u16(&mut payload, *height);
            push_u16(&mut payload, *pixels_per_inch);
        }
        Message::SetScreen(screen) => {
            let mut count = 0;
            encode_screen(&mut payload, screen, 0, &mut count)?;
        }
        Message::Action { action } => {
            push_u32(&mut payload, action.0);
        }
        Message::Log { level, message } => {
            payload.push(*level as u8);
            push_string(&mut payload, message)?;
        }
        Message::Exit => {}
        Message::Launch { name } => push_string(&mut payload, name)?,
        Message::DeviceRequest(request) => encode_device_request(&mut payload, *request),
        Message::DeviceResult(result) => encode_device_result(&mut payload, *result),
        Message::Spawn { task, work } => {
            push_u32(&mut payload, task.0);
            match work {
                Task::Fetch {
                    url,
                    offset,
                    max_bytes,
                } => {
                    payload.push(0);
                    push_string(&mut payload, url)?;
                    push_u32(&mut payload, *offset);
                    push_u32(&mut payload, *max_bytes);
                }
                Task::ReadFile { path } => {
                    payload.push(1);
                    push_string(&mut payload, path)?;
                }
                Task::Sleep { seconds } => {
                    payload.push(2);
                    push_u32(&mut payload, *seconds);
                }
                Task::Post {
                    url,
                    body,
                    content_type,
                    secret,
                    max_bytes,
                } => {
                    payload.push(3);
                    push_string(&mut payload, url)?;
                    push_string(&mut payload, body)?;
                    push_string(&mut payload, content_type)?;
                    push_string(&mut payload, secret.as_deref().unwrap_or(""))?;
                    push_u32(&mut payload, *max_bytes);
                }
            }
        }
        Message::Cancel { task } => push_u32(&mut payload, task.0),
        Message::TaskOutcome { task, outcome } => {
            push_u32(&mut payload, task.0);
            match outcome {
                TaskOutcome::Completed(bytes) => {
                    payload.push(0);
                    push_u32(
                        &mut payload,
                        u32::try_from(bytes.len()).map_err(|_| ProtocolError::FrameTooLarge)?,
                    );
                    payload.extend_from_slice(bytes);
                }
                TaskOutcome::Failed(error) => {
                    payload.push(1);
                    payload.push(encode_task_error(*error));
                }
                TaskOutcome::Cancelled => payload.push(2),
            }
        }
    }
    debug_assert_eq!(payload.len(), payload_len);
    let mut bytes = Vec::with_capacity(HEADER_LEN + payload.len());
    bytes.extend_from_slice(&MAGIC);
    bytes.push(VERSION);
    bytes.push(kind);
    let payload_len = u32::try_from(payload.len()).map_err(|_| ProtocolError::FrameTooLarge)?;
    bytes.extend_from_slice(&payload_len.to_be_bytes());
    bytes.extend_from_slice(&frame.request_id.to_be_bytes());
    bytes.extend_from_slice(&payload);
    Ok(bytes)
}

fn encoded_message_layout(message: &Message) -> Result<(u8, usize), ProtocolError> {
    match message {
        Message::Hello { name } => Ok((1, encoded_string_len(name)?)),
        Message::Welcome { .. } => Ok((2, 6)),
        Message::SetScreen(screen) => {
            let mut count = 0;
            Ok((3, encoded_screen_len(screen, 0, &mut count)?))
        }
        Message::Action { .. } => Ok((4, 4)),
        Message::Log { message, .. } => {
            let mut length = 1;
            add_encoded_len(&mut length, encoded_string_len(message)?)?;
            Ok((5, length))
        }
        Message::Exit => Ok((6, 0)),
        Message::Launch { name } => {
            let mut length = 0;
            add_encoded_len(&mut length, encoded_string_len(name)?)?;
            Ok((12, length))
        }
        Message::DeviceRequest(_) => Ok((7, 5)),
        Message::DeviceResult(result) => Ok((8, 1 + device_result_value_len(*result))),
        Message::Spawn { work, .. } => {
            let mut length = 6;
            match work {
                Task::Fetch { url, .. } => {
                    add_encoded_len(&mut length, 8)?;
                    add_encoded_len(&mut length, encoded_string_len(url)?)?;
                }
                Task::ReadFile { path } => {
                    add_encoded_len(&mut length, encoded_string_len(path)?)?;
                }
                Task::Sleep { .. } => add_encoded_len(&mut length, 4)?,
                Task::Post {
                    url,
                    body,
                    content_type,
                    secret,
                    ..
                } => {
                    add_encoded_len(&mut length, 4)?;
                    add_encoded_len(&mut length, encoded_string_len(url)?)?;
                    add_encoded_len(&mut length, encoded_string_len(body)?)?;
                    add_encoded_len(&mut length, encoded_string_len(content_type)?)?;
                    add_encoded_len(
                        &mut length,
                        encoded_string_len(secret.as_deref().unwrap_or(""))?,
                    )?;
                }
            }
            Ok((9, length))
        }
        Message::Cancel { .. } => Ok((10, 4)),
        Message::TaskOutcome { outcome, .. } => {
            let mut length = 5;
            match outcome {
                TaskOutcome::Completed(bytes) => {
                    if bytes.len() > MAX_TASK_BYTES {
                        return Err(ProtocolError::FrameTooLarge);
                    }
                    add_encoded_len(&mut length, 4)?;
                    add_encoded_len(&mut length, bytes.len())?;
                }
                TaskOutcome::Failed(_) => add_encoded_len(&mut length, 1)?,
                TaskOutcome::Cancelled => {}
            }
            Ok((11, length))
        }
    }
}

/// Every device request encodes as one tag byte and one 32-bit argument, so a
/// malformed request can never change the frame length.
fn encode_device_request(output: &mut Vec<u8>, request: DeviceRequest) {
    let (tag, argument) = match request {
        DeviceRequest::ReadBattery => (1_u8, 0_u32),
        DeviceRequest::HoldWifi { seconds } => (2, seconds),
        DeviceRequest::ReleaseWifi => (3, 0),
        DeviceRequest::KeepAwake { seconds } => (4, seconds),
        DeviceRequest::AllowSleep => (5, 0),
        DeviceRequest::ScheduleWake { seconds } => (6, seconds),
        DeviceRequest::CancelWake => (7, 0),
        DeviceRequest::SetFrontlight { percent } => (8, u32::from(percent)),
        DeviceRequest::ReadFrontlight => (9, 0),
    };
    output.push(tag);
    push_u32(output, argument);
}

fn decode_device_request(reader: &mut Reader<'_>) -> Result<DeviceRequest, ProtocolError> {
    let tag = reader.u8()?;
    let argument = reader.u32()?;
    match tag {
        1 => Ok(DeviceRequest::ReadBattery),
        2 => Ok(DeviceRequest::HoldWifi { seconds: argument }),
        3 => Ok(DeviceRequest::ReleaseWifi),
        4 => Ok(DeviceRequest::KeepAwake { seconds: argument }),
        5 => Ok(DeviceRequest::AllowSleep),
        6 => Ok(DeviceRequest::ScheduleWake { seconds: argument }),
        7 => Ok(DeviceRequest::CancelWake),
        8 => {
            let percent = u8::try_from(argument)
                .ok()
                .filter(|percent| *percent <= 100)
                .ok_or(ProtocolError::InvalidValue("frontlight percent"))?;
            Ok(DeviceRequest::SetFrontlight { percent })
        }
        9 => Ok(DeviceRequest::ReadFrontlight),
        _ => Err(ProtocolError::InvalidValue("device request")),
    }
}

const fn device_result_value_len(result: DeviceResult) -> usize {
    match result {
        DeviceResult::Done => 0,
        DeviceResult::Granted { .. } => 4,
        DeviceResult::Battery { .. } => 2,
        DeviceResult::Frontlight { .. } | DeviceResult::Denied(_) => 1,
    }
}

fn encode_device_result(output: &mut Vec<u8>, result: DeviceResult) {
    match result {
        DeviceResult::Done => output.push(1),
        DeviceResult::Granted { seconds } => {
            output.push(2);
            push_u32(output, seconds);
        }
        DeviceResult::Battery { percent, charging } => {
            output.push(3);
            output.push(percent);
            output.push(u8::from(charging));
        }
        DeviceResult::Frontlight { percent } => {
            output.push(4);
            output.push(percent);
        }
        DeviceResult::Denied(reason) => {
            output.push(5);
            output.push(reason as u8);
        }
    }
}

fn decode_device_result(reader: &mut Reader<'_>) -> Result<DeviceResult, ProtocolError> {
    match reader.u8()? {
        1 => Ok(DeviceResult::Done),
        2 => Ok(DeviceResult::Granted {
            seconds: reader.u32()?,
        }),
        3 => {
            let percent = reader.u8()?;
            if percent > 100 {
                return Err(ProtocolError::InvalidValue("battery percent"));
            }
            let charging = match reader.u8()? {
                0 => false,
                1 => true,
                _ => return Err(ProtocolError::InvalidValue("charging flag")),
            };
            Ok(DeviceResult::Battery { percent, charging })
        }
        4 => {
            let percent = reader.u8()?;
            if percent > 100 {
                return Err(ProtocolError::InvalidValue("frontlight percent"));
            }
            Ok(DeviceResult::Frontlight { percent })
        }
        5 => Ok(DeviceResult::Denied(DenyReason::try_from(reader.u8()?)?)),
        _ => Err(ProtocolError::InvalidValue("device result")),
    }
}

fn encoded_screen_len(
    screen: &Screen,
    depth: usize,
    count: &mut usize,
) -> Result<usize, ProtocolError> {
    if screen.nodes.len() > MAX_NODES {
        return Err(ProtocolError::TooManyNodes);
    }
    let mut length = 8;
    if let Some(top_bar) = &screen.top_bar {
        add_encoded_len(&mut length, 5)?;
        add_encoded_len(&mut length, encoded_string_len(&top_bar.title)?)?;
        if let Some(action) = &top_bar.action {
            add_encoded_len(&mut length, 4)?;
            add_encoded_len(&mut length, encoded_string_len(&action.label)?)?;
        }
    }
    // One flag byte, plus two action identifiers when the screen asked for
    // tap-to-turn.
    add_encoded_len(&mut length, 1)?;
    if screen.page_turns.is_some() {
        add_encoded_len(&mut length, 8)?;
    }
    if let Some(nav_bar) = &screen.nav_bar {
        if nav_bar.destinations.len() > u8::MAX as usize {
            return Err(ProtocolError::TooManyNodes);
        }
        add_encoded_len(&mut length, 6)?;
        for destination in &nav_bar.destinations {
            add_encoded_len(&mut length, 4)?;
            add_encoded_len(&mut length, encoded_string_len(&destination.label)?)?;
        }
    }
    for node in &screen.nodes {
        add_encoded_len(&mut length, encoded_node_len(node, depth, count)?)?;
    }
    Ok(length)
}

// One exhaustive match over every node kind. Splitting it would only move
// arms out of reach of the compiler's exhaustiveness check, which is the one
// thing making it impossible to add a node and forget the wire format. The
// arms stay in enum order and are never merged by coincidentally equal sizes,
// because reading this beside the enum is how the two are kept in step.
#[allow(clippy::too_many_lines, clippy::match_same_arms)]
fn encoded_node_len(node: &Node, depth: usize, count: &mut usize) -> Result<usize, ProtocolError> {
    if depth > MAX_DEPTH {
        return Err(ProtocolError::TooDeep);
    }
    *count = count.checked_add(1).ok_or(ProtocolError::TooManyNodes)?;
    if *count > MAX_NODES {
        return Err(ProtocolError::TooManyNodes);
    }

    let length = match node {
        Node::Heading { text, .. } | Node::Text { text, .. } => {
            let mut length = 5;
            add_encoded_len(&mut length, encoded_string_len(text)?)?;
            length
        }
        Node::Button { label, .. } => {
            let mut length = 9;
            add_encoded_len(&mut length, encoded_string_len(label)?)?;
            length
        }
        Node::Card { children, .. } => {
            if children.len() > MAX_NODES {
                return Err(ProtocolError::TooManyNodes);
            }
            let mut length = 7;
            for child in children {
                add_encoded_len(&mut length, encoded_node_len(child, depth + 1, count)?)?;
            }
            length
        }
        Node::Divider { .. } => 5,
        // One tag byte, not a length. This was 9 while the node still carried
        // a raw i32, which over-reserved the frame by three bytes and tripped
        // the encoder's own length assertion in debug builds.
        Node::Spacer { .. } => 6,
        Node::Progress { .. } => 6,
        Node::PagedList { items, .. } => {
            if items.len() > MAX_NODES {
                return Err(ProtocolError::TooManyNodes);
            }
            let mut length = 9;
            for item in items {
                add_encoded_len(&mut length, encoded_string_len(item)?)?;
            }
            length
        }
        Node::Grid { cells, .. } => {
            if cells.len() > u8::MAX as usize {
                return Err(ProtocolError::TooManyNodes);
            }
            // Tag, id, columns, square flag and count.
            let mut length = 8;
            for cell in cells {
                add_encoded_len(&mut length, 4)?;
                add_encoded_len(&mut length, encoded_string_len(&cell.label)?)?;
            }
            length
        }
        Node::Rows { rows, .. } => {
            if rows.len() > u8::MAX as usize {
                return Err(ProtocolError::TooManyNodes);
            }
            let mut length = 6;
            for row in rows {
                // Four bytes of action and one of glyph, then both strings.
                add_encoded_len(&mut length, 5)?;
                add_encoded_len(&mut length, encoded_string_len(&row.title)?)?;
                add_encoded_len(&mut length, encoded_string_len(&row.summary)?)?;
            }
            length
        }
        Node::TileGrid { tiles, .. } => {
            if tiles.len() > u8::MAX as usize {
                return Err(ProtocolError::TooManyNodes);
            }
            let mut length = 6;
            for tile in tiles {
                add_encoded_len(&mut length, 5)?;
                add_encoded_len(&mut length, encoded_string_len(&tile.label)?)?;
            }
            length
        }
        Node::Choice {
            prompt,
            options,
            freeform,
            ..
        } => {
            if options.len() > u8::MAX as usize {
                return Err(ProtocolError::TooManyNodes);
            }
            let mut length = 7;
            add_encoded_len(&mut length, encoded_string_len(prompt)?)?;
            for option in options {
                add_encoded_len(&mut length, 4)?;
                add_encoded_len(&mut length, encoded_string_len(&option.label)?)?;
            }
            if let Some(freeform) = freeform {
                add_encoded_len(&mut length, 4)?;
                add_encoded_len(&mut length, encoded_string_len(&freeform.placeholder)?)?;
            }
            length
        }
        Node::Banner { text, .. } => {
            let mut length = 6;
            add_encoded_len(&mut length, encoded_string_len(text)?)?;
            length
        }
        Node::Skeleton { .. } => 6,
        Node::Activity {
            label,
            progress,
            cancel,
            ..
        } => {
            let mut length = 7;
            add_encoded_len(&mut length, encoded_string_len(label)?)?;
            if progress.is_some() {
                add_encoded_len(&mut length, 1)?;
            }
            if let Some(cancel) = cancel {
                add_encoded_len(&mut length, 4)?;
                add_encoded_len(&mut length, encoded_string_len(&cancel.label)?)?;
            }
            length
        }
    };
    if length > MAX_FRAME_LEN {
        return Err(ProtocolError::FrameTooLarge);
    }
    Ok(length)
}

fn encoded_string_len(text: &str) -> Result<usize, ProtocolError> {
    if text.len() > MAX_STRING_LEN || u16::try_from(text.len()).is_err() {
        return Err(ProtocolError::StringTooLarge);
    }
    Ok(2 + text.len())
}

fn add_encoded_len(total: &mut usize, additional: usize) -> Result<(), ProtocolError> {
    *total = total
        .checked_add(additional)
        .filter(|length| *length <= MAX_FRAME_LEN)
        .ok_or(ProtocolError::FrameTooLarge)?;
    Ok(())
}

/// # Errors
///
/// Returns an error for an unsupported, malformed, or oversized frame.
// One arm per message type. Splitting the table would put the wire tags in a
// different place from the lengths they have to agree with, which is the one
// thing this function exists to keep together.
#[allow(clippy::too_many_lines)]
pub fn decode(bytes: &[u8]) -> Result<Frame, ProtocolError> {
    if bytes.len() < HEADER_LEN {
        return Err(ProtocolError::Truncated);
    }

    if bytes[..4] != MAGIC {
        return Err(ProtocolError::BadMagic);
    }
    if bytes[4] != VERSION {
        return Err(ProtocolError::UnsupportedVersion(bytes[4]));
    }
    let payload_len = u32::from_be_bytes([bytes[6], bytes[7], bytes[8], bytes[9]]) as usize;
    if payload_len > MAX_FRAME_LEN {
        return Err(ProtocolError::FrameTooLarge);
    }
    if bytes.len() != HEADER_LEN + payload_len {
        return Err(ProtocolError::LengthMismatch);
    }
    let request_id = u32::from_be_bytes([bytes[10], bytes[11], bytes[12], bytes[13]]);
    let mut reader = Reader::new(&bytes[HEADER_LEN..]);
    let message = match bytes[5] {
        1 => Message::Hello {
            name: reader.string()?,
        },
        2 => Message::Welcome {
            width: reader.u16()?,
            height: reader.u16()?,
            pixels_per_inch: reader.u16()?,
        },
        3 => {
            let mut count = 0;
            Message::SetScreen(decode_screen(&mut reader, 0, &mut count)?)
        }
        4 => Message::Action {
            action: ActionId(reader.u32()?),
        },
        5 => Message::Log {
            level: LogLevel::try_from(reader.u8()?)?,
            message: reader.string()?,
        },
        6 => Message::Exit,
        12 => Message::Launch {
            name: reader.string()?,
        },
        7 => Message::DeviceRequest(decode_device_request(&mut reader)?),
        8 => Message::DeviceResult(decode_device_result(&mut reader)?),
        9 => {
            let task = TaskId(reader.u32()?);
            let work = match reader.u8()? {
                0 => {
                    let url = reader.string()?;
                    if url.len() > MAX_URL_LEN {
                        return Err(ProtocolError::StringTooLarge);
                    }
                    Task::Fetch {
                        url,
                        offset: reader.u32()?,
                        // Clamped here rather than trusted, so a task cannot
                        // declare a ceiling larger than the transport can carry
                        // and then be surprised when the answer will not fit.
                        max_bytes: min(reader.u32()?, MAX_TASK_BYTES_U32),
                    }
                }
                1 => Task::ReadFile {
                    path: reader.string()?,
                },
                2 => Task::Sleep {
                    seconds: reader.u32()?,
                },
                3 => {
                    let url = reader.string()?;
                    if url.len() > MAX_URL_LEN {
                        return Err(ProtocolError::StringTooLarge);
                    }
                    let body = reader.string()?;
                    let content_type = reader.string()?;
                    let secret = reader.string()?;
                    Task::Post {
                        url,
                        body,
                        content_type,
                        // An empty name means no credential. Encoding it as an
                        // empty string rather than an option keeps the wire
                        // format free of a flag byte whose two values would
                        // otherwise both have to be exercised.
                        secret: if secret.is_empty() {
                            None
                        } else {
                            Some(secret)
                        },
                        max_bytes: min(reader.u32()?, MAX_TASK_BYTES_U32),
                    }
                }
                _ => return Err(ProtocolError::InvalidValue("task kind")),
            };
            Message::Spawn { task, work }
        }
        10 => Message::Cancel {
            task: TaskId(reader.u32()?),
        },
        11 => {
            let task = TaskId(reader.u32()?);
            let outcome = match reader.u8()? {
                0 => {
                    let length = reader.u32()? as usize;
                    if length > MAX_TASK_BYTES {
                        return Err(ProtocolError::FrameTooLarge);
                    }
                    TaskOutcome::Completed(reader.take(length)?.to_vec())
                }
                1 => TaskOutcome::Failed(decode_task_error(reader.u8()?)?),
                2 => TaskOutcome::Cancelled,
                _ => return Err(ProtocolError::InvalidValue("task outcome")),
            };
            Message::TaskOutcome { task, outcome }
        }
        value => return Err(ProtocolError::UnknownMessageType(value)),
    };
    if !reader.is_finished() {
        return Err(ProtocolError::LengthMismatch);
    }
    Ok(Frame {
        request_id,
        message,
    })
}

/// Writes one complete frame to a reliable byte stream.
///
/// # Errors
///
/// Returns a protocol error for an invalid frame or an I/O error from the
/// destination.
pub fn write_to<W: Write>(writer: &mut W, frame: &Frame) -> Result<(), StreamError> {
    let bytes = encode(frame)?;
    writer.write_all(&bytes)?;
    writer.flush()?;
    Ok(())
}

/// Reads one bounded frame from a reliable byte stream.
///
/// # Errors
///
/// Returns an I/O error when the frame is truncated and a protocol error when
/// its header or payload is invalid.
pub fn read_from<R: Read>(reader: &mut R) -> Result<Frame, StreamError> {
    let mut header = [0_u8; HEADER_LEN];
    reader.read_exact(&mut header)?;
    if header[..4] != MAGIC {
        return Err(ProtocolError::BadMagic.into());
    }
    if header[4] != VERSION {
        return Err(ProtocolError::UnsupportedVersion(header[4]).into());
    }
    let payload_len = u32::from_be_bytes([header[6], header[7], header[8], header[9]]) as usize;
    if payload_len > MAX_FRAME_LEN {
        return Err(ProtocolError::FrameTooLarge.into());
    }
    let mut bytes = Vec::with_capacity(HEADER_LEN + payload_len);
    bytes.extend_from_slice(&header);
    bytes.resize(HEADER_LEN + payload_len, 0);
    reader.read_exact(&mut bytes[HEADER_LEN..])?;
    Ok(decode(&bytes)?)
}

fn encode_screen(
    output: &mut Vec<u8>,
    screen: &Screen,
    depth: usize,
    count: &mut usize,
) -> Result<(), ProtocolError> {
    push_u32(output, screen.id);
    // Bars are encoded as presence flags outside the node list, mirroring the
    // in-memory shape. A screen with two nav bars is not a frame this format
    // can express, so no validation is needed to reject one.
    match &screen.top_bar {
        None => output.push(0),
        Some(top_bar) => {
            output.push(1);
            push_u32(output, top_bar.id.0);
            push_string(output, &top_bar.title)?;
            match &top_bar.action {
                None => output.push(0),
                Some(action) => {
                    output.push(1);
                    encode_bar_action(output, action)?;
                }
            }
        }
    }
    match &screen.nav_bar {
        None => output.push(0),
        Some(nav_bar) => {
            output.push(1);
            push_u32(output, nav_bar.id.0);
            let len = u8::try_from(nav_bar.destinations.len())
                .map_err(|_| ProtocolError::TooManyNodes)?;
            output.push(len);
            output.push(u8::try_from(nav_bar.selected).unwrap_or(u8::MAX));
            for destination in &nav_bar.destinations {
                encode_bar_action(output, destination)?;
            }
        }
    }
    match &screen.page_turns {
        None => output.push(0),
        Some(turns) => {
            output.push(1);
            push_u32(output, turns.previous.0);
            push_u32(output, turns.next.0);
        }
    }
    push_u16(
        output,
        u16::try_from(screen.nodes.len()).map_err(|_| ProtocolError::TooManyNodes)?,
    );
    for node in &screen.nodes {
        encode_node(output, node, depth, count)?;
    }
    Ok(())
}

fn encode_bar_action(output: &mut Vec<u8>, action: &BarAction) -> Result<(), ProtocolError> {
    push_u32(output, action.action.0);
    push_string(output, &action.label)
}

fn decode_bar_action(reader: &mut Reader<'_>) -> Result<BarAction, ProtocolError> {
    let action = ActionId(reader.u32()?);
    // The runtime owns going back, so an application is not allowed to name
    // that identifier. Rejecting it here means a hostile frame cannot forge a
    // control the reader is entitled to trust.
    if action.is_reserved() {
        return Err(ProtocolError::InvalidValue("reserved action id"));
    }
    Ok(BarAction {
        action,
        label: reader.string()?,
    })
}

// One exhaustive match over every node kind. Splitting it would only move
// arms out of reach of the compiler's exhaustiveness check, which is the one
// thing making it impossible to add a node and forget the wire format.
#[allow(clippy::too_many_lines)]
fn encode_node(
    output: &mut Vec<u8>,
    node: &Node,
    depth: usize,
    count: &mut usize,
) -> Result<(), ProtocolError> {
    if depth > MAX_DEPTH {
        return Err(ProtocolError::TooDeep);
    }
    *count += 1;
    if *count > MAX_NODES {
        return Err(ProtocolError::TooManyNodes);
    }
    match node {
        Node::Heading { id, text } => {
            output.push(1);
            push_u32(output, id.0);
            push_string(output, text)?;
        }
        Node::Text { id, text } => {
            output.push(2);
            push_u32(output, id.0);
            push_string(output, text)?;
        }
        Node::Button { id, action, label } => {
            output.push(3);
            push_u32(output, id.0);
            push_u32(output, action.0);
            push_string(output, label)?;
        }
        Node::Card { id, children } => {
            output.push(4);
            push_u32(output, id.0);
            push_u16(
                output,
                u16::try_from(children.len()).map_err(|_| ProtocolError::TooManyNodes)?,
            );
            for child in children {
                encode_node(output, child, depth + 1, count)?;
            }
        }
        Node::Divider { id } => {
            output.push(5);
            push_u32(output, id.0);
        }
        Node::Spacer { id, space } => {
            output.push(6);
            push_u32(output, id.0);
            // A tag rather than a length, so the wire format cannot carry a
            // spacing that is off the scale or negative.
            output.push(match space {
                Space::Tight => 0,
                Space::Small => 1,
                Space::Medium => 2,
                Space::Large => 3,
            });
        }
        Node::Progress { id, value } => {
            output.push(7);
            push_u32(output, id.0);
            output.push(value.get());
        }
        Node::PagedList { id, page, items } => {
            if items.len() > MAX_NODES {
                return Err(ProtocolError::TooManyNodes);
            }
            output.push(8);
            push_u32(output, id.0);
            push_u16(output, *page);
            push_u16(
                output,
                u16::try_from(items.len()).map_err(|_| ProtocolError::TooManyNodes)?,
            );
            for item in items {
                push_string(output, item)?;
            }
        }
        Node::Grid {
            id,
            columns,
            square,
            cells,
        } => {
            output.push(15);
            push_u32(output, id.0);
            output.push(*columns);
            output.push(u8::from(*square));
            output.push(u8::try_from(cells.len()).map_err(|_| ProtocolError::TooManyNodes)?);
            for cell in cells {
                push_u32(output, cell.action.0);
                push_string(output, &cell.label)?;
            }
        }
        Node::Rows { id, rows } => {
            output.push(14);
            push_u32(output, id.0);
            output.push(u8::try_from(rows.len()).map_err(|_| ProtocolError::TooManyNodes)?);
            for row in rows {
                push_u32(output, row.action.0);
                push_string(output, &row.title)?;
                push_string(output, &row.summary)?;
                output.push(encode_glyph(row.glyph));
            }
        }
        Node::TileGrid { id, tiles } => {
            output.push(9);
            push_u32(output, id.0);
            output.push(u8::try_from(tiles.len()).map_err(|_| ProtocolError::TooManyNodes)?);
            for tile in tiles {
                push_u32(output, tile.action.0);
                push_string(output, &tile.label)?;
                output.push(encode_glyph(tile.glyph));
            }
        }
        Node::Choice {
            id,
            prompt,
            options,
            freeform,
        } => {
            output.push(10);
            push_u32(output, id.0);
            push_string(output, prompt)?;
            output.push(u8::try_from(options.len()).map_err(|_| ProtocolError::TooManyNodes)?);
            for option in options {
                encode_bar_action(output, option)?;
            }
            match freeform {
                None => output.push(0),
                Some(freeform) => {
                    output.push(1);
                    push_u32(output, freeform.action.0);
                    push_string(output, &freeform.placeholder)?;
                }
            }
        }
        Node::Banner { id, level, text } => {
            output.push(11);
            push_u32(output, id.0);
            output.push(match level {
                BannerLevel::Info => 0,
                BannerLevel::Attention => 1,
            });
            push_string(output, text)?;
        }
        Node::Skeleton { id, lines } => {
            output.push(12);
            push_u32(output, id.0);
            output.push(*lines);
        }
        Node::Activity {
            id,
            label,
            progress,
            cancel,
        } => {
            output.push(13);
            push_u32(output, id.0);
            push_string(output, label)?;
            match progress {
                None => output.push(0),
                Some(progress) => {
                    output.push(1);
                    output.push(progress.get());
                }
            }
            match cancel {
                None => output.push(0),
                Some(cancel) => {
                    output.push(1);
                    encode_bar_action(output, cancel)?;
                }
            }
        }
    }
    Ok(())
}

const fn encode_glyph(glyph: Glyph) -> u8 {
    match glyph {
        Glyph::App => 0,
        Glyph::Book => 1,
        Glyph::Note => 2,
        Glyph::Clock => 3,
        Glyph::Settings => 4,
        Glyph::Folder => 5,
        Glyph::Chart => 6,
        Glyph::Search => 7,
        Glyph::Wifi => 8,
        Glyph::Battery => 9,
        Glyph::Reader => 10,
        Glyph::Power => 11,
        Glyph::Grid => 12,
    }
}

const fn decode_glyph(tag: u8) -> Option<Glyph> {
    Some(match tag {
        0 => Glyph::App,
        1 => Glyph::Book,
        2 => Glyph::Note,
        3 => Glyph::Clock,
        4 => Glyph::Settings,
        5 => Glyph::Folder,
        6 => Glyph::Chart,
        7 => Glyph::Search,
        8 => Glyph::Wifi,
        9 => Glyph::Battery,
        10 => Glyph::Reader,
        11 => Glyph::Power,
        12 => Glyph::Grid,
        _ => return None,
    })
}

fn decode_screen(
    reader: &mut Reader<'_>,
    depth: usize,
    count: &mut usize,
) -> Result<Screen, ProtocolError> {
    let id = reader.u32()?;
    let top_bar = match reader.u8()? {
        0 => None,
        1 => {
            let bar_id = NodeId(reader.u32()?);
            let title = reader.string()?;
            let action = match reader.u8()? {
                0 => None,
                1 => Some(decode_bar_action(reader)?),
                _ => return Err(ProtocolError::InvalidValue("top bar action flag")),
            };
            Some(TopBar {
                id: bar_id,
                title,
                action,
            })
        }
        _ => return Err(ProtocolError::InvalidValue("top bar flag")),
    };
    let nav_bar = match reader.u8()? {
        0 => None,
        1 => {
            let bar_id = NodeId(reader.u32()?);
            let len = usize::from(reader.u8()?);
            let selected = usize::from(reader.u8()?);
            let mut destinations = Vec::with_capacity(len);
            for _ in 0..len {
                destinations.push(decode_bar_action(reader)?);
            }
            if destinations.len() < MIN_NAV_DESTINATIONS {
                return Err(ProtocolError::InvalidValue("nav bar destinations"));
            }
            Some(NavBar {
                id: bar_id,
                // Clamped rather than rejected: an out of range selection is a
                // caller mistake, and refusing the frame would leave the reader
                // with no navigation at all.
                selected: min(selected, destinations.len() - 1),
                destinations,
            })
        }
        _ => return Err(ProtocolError::InvalidValue("nav bar flag")),
    };
    let page_turns = match reader.u8()? {
        0 => None,
        1 => Some(PageTurns::new(
            ActionId(reader.u32()?),
            ActionId(reader.u32()?),
        )),
        _ => return Err(ProtocolError::InvalidValue("page turn flag")),
    };
    let count_nodes = usize::from(reader.u16()?);
    if count_nodes > MAX_NODES {
        return Err(ProtocolError::TooManyNodes);
    }
    let mut nodes = Vec::with_capacity(count_nodes);
    for _ in 0..count_nodes {
        nodes.push(decode_node(reader, depth, count)?);
    }
    let mut screen = Screen::new(id, nodes);
    screen.top_bar = top_bar;
    screen.nav_bar = nav_bar;
    screen.page_turns = page_turns;
    Ok(screen)
}

// One exhaustive match over every node kind. Splitting it would only move
// arms out of reach of the compiler's exhaustiveness check, which is the one
// thing making it impossible to add a node and forget the wire format.
#[allow(clippy::too_many_lines)]
fn decode_node(
    reader: &mut Reader<'_>,
    depth: usize,
    count: &mut usize,
) -> Result<Node, ProtocolError> {
    if depth > MAX_DEPTH {
        return Err(ProtocolError::TooDeep);
    }
    *count += 1;
    if *count > MAX_NODES {
        return Err(ProtocolError::TooManyNodes);
    }
    let tag = reader.u8()?;
    let id = NodeId(reader.u32()?);
    match tag {
        1 => Ok(Node::Heading {
            id,
            text: reader.string()?,
        }),
        2 => Ok(Node::Text {
            id,
            text: reader.string()?,
        }),
        3 => Ok(Node::Button {
            id,
            action: ActionId(reader.u32()?),
            label: reader.string()?,
        }),
        4 => {
            let child_count = usize::from(reader.u16()?);
            if child_count > MAX_NODES {
                return Err(ProtocolError::TooManyNodes);
            }
            let mut children = Vec::with_capacity(child_count);
            for _ in 0..child_count {
                children.push(decode_node(reader, depth + 1, count)?);
            }
            Ok(Node::Card { id, children })
        }
        5 => Ok(Node::Divider { id }),
        6 => Ok(Node::Spacer {
            id,
            space: match reader.u8()? {
                0 => Space::Tight,
                1 => Space::Small,
                2 => Space::Medium,
                3 => Space::Large,
                _ => return Err(ProtocolError::InvalidValue("spacer scale")),
            },
        }),
        7 => Ok(Node::Progress {
            id,
            // Clamped rather than rejected, because a percentage over a
            // hundred is a caller mistake rather than a malformed frame.
            value: Percent::new(reader.u8()?),
        }),
        8 => {
            let page = reader.u16()?;
            let item_count = usize::from(reader.u16()?);
            if item_count > MAX_NODES {
                return Err(ProtocolError::TooManyNodes);
            }
            let mut items = Vec::with_capacity(item_count);
            for _ in 0..item_count {
                items.push(reader.string()?);
            }
            Ok(Node::PagedList { id, page, items })
        }
        9 => {
            let len = usize::from(reader.u8()?);
            let mut tiles = Vec::with_capacity(len);
            for _ in 0..len {
                let action = ActionId(reader.u32()?);
                if action.is_reserved() {
                    return Err(ProtocolError::InvalidValue("reserved action id"));
                }
                let label = reader.string()?;
                let glyph =
                    decode_glyph(reader.u8()?).ok_or(ProtocolError::InvalidValue("tile glyph"))?;
                tiles.push(Tile {
                    action,
                    label,
                    glyph,
                });
            }
            Ok(Node::TileGrid { id, tiles })
        }
        10 => {
            let prompt = reader.string()?;
            let len = usize::from(reader.u8()?);
            let mut options = Vec::with_capacity(len);
            for _ in 0..len {
                options.push(decode_bar_action(reader)?);
            }
            let freeform = match reader.u8()? {
                0 => None,
                1 => {
                    let action = ActionId(reader.u32()?);
                    if action.is_reserved() {
                        return Err(ProtocolError::InvalidValue("reserved action id"));
                    }
                    Some(Freeform {
                        action,
                        placeholder: reader.string()?,
                    })
                }
                _ => return Err(ProtocolError::InvalidValue("freeform flag")),
            };
            if options.is_empty() && freeform.is_none() {
                return Err(ProtocolError::InvalidValue("choice with no answers"));
            }
            Ok(Node::Choice {
                id,
                prompt,
                options,
                freeform,
            })
        }
        11 => {
            let level = match reader.u8()? {
                0 => BannerLevel::Info,
                1 => BannerLevel::Attention,
                _ => return Err(ProtocolError::InvalidValue("banner level")),
            };
            Ok(Node::Banner {
                id,
                level,
                text: reader.string()?,
            })
        }
        12 => Ok(Node::Skeleton {
            id,
            lines: reader.u8()?,
        }),
        13 => {
            let label = reader.string()?;
            let progress = match reader.u8()? {
                0 => None,
                1 => Some(Percent::new(reader.u8()?)),
                _ => return Err(ProtocolError::InvalidValue("activity progress flag")),
            };
            let cancel = match reader.u8()? {
                0 => None,
                1 => Some(decode_bar_action(reader)?),
                _ => return Err(ProtocolError::InvalidValue("activity cancel flag")),
            };
            Ok(Node::Activity {
                id,
                label,
                progress,
                cancel,
            })
        }
        15 => {
            let columns = reader.u8()?;
            if columns == 0 || columns > kobo_ui::MAX_COLUMNS {
                return Err(ProtocolError::InvalidValue("grid columns"));
            }
            let square = match reader.u8()? {
                0 => false,
                1 => true,
                _ => return Err(ProtocolError::InvalidValue("grid square flag")),
            };
            let len = usize::from(reader.u8()?);
            if len > kobo_ui::MAX_CELLS {
                return Err(ProtocolError::TooManyNodes);
            }
            let mut cells = Vec::with_capacity(len);
            for _ in 0..len {
                let action = ActionId(reader.u32()?);
                if action.is_reserved() {
                    return Err(ProtocolError::InvalidValue("reserved action id"));
                }
                cells.push(Cell::new(action, reader.string()?));
            }
            Ok(Node::Grid {
                id,
                columns,
                square,
                cells,
            })
        }
        14 => {
            let len = usize::from(reader.u8()?);
            let mut rows = Vec::with_capacity(len);
            for _ in 0..len {
                let action = ActionId(reader.u32()?);
                if action.is_reserved() {
                    return Err(ProtocolError::InvalidValue("reserved action id"));
                }
                let title = reader.string()?;
                let summary = reader.string()?;
                let glyph =
                    decode_glyph(reader.u8()?).ok_or(ProtocolError::InvalidValue("row glyph"))?;
                rows.push(Row {
                    action,
                    title,
                    summary,
                    glyph,
                });
            }
            Ok(Node::Rows { id, rows })
        }
        _ => Err(ProtocolError::InvalidValue("node tag")),
    }
}

fn push_string(output: &mut Vec<u8>, text: &str) -> Result<(), ProtocolError> {
    if text.len() > MAX_STRING_LEN {
        return Err(ProtocolError::StringTooLarge);
    }
    push_u16(
        output,
        u16::try_from(text.len()).map_err(|_| ProtocolError::StringTooLarge)?,
    );
    output.extend_from_slice(text.as_bytes());
    Ok(())
}

fn push_u16(output: &mut Vec<u8>, value: u16) {
    output.extend_from_slice(&value.to_be_bytes());
}

fn push_u32(output: &mut Vec<u8>, value: u32) {
    output.extend_from_slice(&value.to_be_bytes());
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], ProtocolError> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or(ProtocolError::Truncated)?;
        let slice = self
            .bytes
            .get(self.offset..end)
            .ok_or(ProtocolError::Truncated)?;
        self.offset = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, ProtocolError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, ProtocolError> {
        let bytes = self.take(2)?;
        Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    fn u32(&mut self) -> Result<u32, ProtocolError> {
        let bytes = self.take(4)?;
        Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn string(&mut self) -> Result<String, ProtocolError> {
        let length = usize::from(self.u16()?);
        if length > MAX_STRING_LEN {
            return Err(ProtocolError::StringTooLarge);
        }
        let bytes = self.take(length)?;
        let text = std::str::from_utf8(bytes).map_err(|_| ProtocolError::InvalidUtf8)?;
        Ok(text.to_owned())
    }

    const fn is_finished(&self) -> bool {
        self.offset == self.bytes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn every_device_request_round_trips() {
        let requests = [
            DeviceRequest::ReadBattery,
            DeviceRequest::HoldWifi { seconds: 600 },
            DeviceRequest::ReleaseWifi,
            DeviceRequest::KeepAwake { seconds: u32::MAX },
            DeviceRequest::AllowSleep,
            DeviceRequest::ScheduleWake { seconds: 900 },
            DeviceRequest::CancelWake,
            DeviceRequest::SetFrontlight { percent: 100 },
            DeviceRequest::SetFrontlight { percent: 0 },
            DeviceRequest::ReadFrontlight,
        ];
        for request in requests {
            let frame = Frame {
                request_id: 9,
                message: Message::DeviceRequest(request),
            };
            let bytes = encode(&frame).expect("encode");
            assert_eq!(bytes.len(), HEADER_LEN + 5, "requests are fixed width");
            assert_eq!(decode(&bytes).expect("decode"), frame);
        }
    }

    #[test]
    fn every_device_result_round_trips() {
        let results = [
            DeviceResult::Done,
            DeviceResult::Granted { seconds: 300 },
            DeviceResult::Battery {
                percent: 100,
                charging: true,
            },
            DeviceResult::Battery {
                percent: 0,
                charging: false,
            },
            DeviceResult::Frontlight { percent: 42 },
            DeviceResult::Denied(DenyReason::NotDeclared),
            DeviceResult::Denied(DenyReason::WithheldForBattery),
            DeviceResult::Denied(DenyReason::Unsupported),
            DeviceResult::Denied(DenyReason::PolicyRejected),
            DeviceResult::Denied(DenyReason::Busy),
        ];
        for result in results {
            let frame = Frame {
                request_id: 11,
                message: Message::DeviceResult(result),
            };
            let bytes = encode(&frame).expect("encode");
            assert_eq!(decode(&bytes).expect("decode"), frame);
        }
    }

    #[test]
    fn malformed_device_payloads_are_rejected_without_panic() {
        let template = encode(&Frame {
            request_id: 1,
            message: Message::DeviceRequest(DeviceRequest::ReadBattery),
        })
        .expect("encode");

        // An unknown request tag.
        let mut unknown = template.clone();
        unknown[HEADER_LEN] = 200;
        assert_eq!(
            decode(&unknown),
            Err(ProtocolError::InvalidValue("device request"))
        );

        // A percentage that cannot exist.
        let mut absurd = template.clone();
        absurd[HEADER_LEN] = 8;
        absurd[HEADER_LEN + 4] = 250;
        assert_eq!(
            decode(&absurd),
            Err(ProtocolError::InvalidValue("frontlight percent"))
        );

        // A truncated payload must not be read past its end.
        let mut truncated = template.clone();
        truncated.truncate(HEADER_LEN + 3);
        assert_eq!(decode(&truncated), Err(ProtocolError::LengthMismatch));

        let result = encode(&Frame {
            request_id: 1,
            message: Message::DeviceResult(DeviceResult::Denied(DenyReason::Busy)),
        })
        .expect("encode");
        let mut bad_reason = result;
        let last = bad_reason.len() - 1;
        bad_reason[last] = 99;
        assert_eq!(
            decode(&bad_reason),
            Err(ProtocolError::InvalidValue("deny reason"))
        );
    }

    #[test]
    fn screen_round_trip_is_deterministic() {
        let frame = Frame {
            request_id: 12,
            message: Message::SetScreen(Screen::new(
                7,
                vec![Node::Card {
                    id: NodeId(1),
                    children: vec![Node::Button {
                        id: NodeId(2),
                        action: ActionId(3),
                        label: "Go".into(),
                    }],
                }],
            )),
        };
        let encoded = encode(&frame).expect("valid screen");
        assert_eq!(encoded, encode(&frame).expect("stable encoding"));
        assert_eq!(decode(&encoded), Ok(frame));
    }

    #[test]
    fn malformed_frames_are_rejected_before_allocation() {
        assert_eq!(decode(b"short"), Err(ProtocolError::Truncated));
        let mut header = [0_u8; HEADER_LEN];
        header[..4].copy_from_slice(&MAGIC);
        header[4] = VERSION;
        header[5] = 6;
        header[6..10].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(decode(&header), Err(ProtocolError::FrameTooLarge));
    }

    #[test]
    fn invalid_utf8_is_rejected() {
        let mut bytes = encode(&Frame {
            request_id: 1,
            message: Message::Hello { name: "x".into() },
        })
        .expect("valid hello");
        *bytes.last_mut().expect("payload") = 0xff;
        assert_eq!(decode(&bytes), Err(ProtocolError::InvalidUtf8));
    }

    #[test]
    fn stream_round_trip_reads_exactly_one_frame() {
        let frame = Frame {
            request_id: 42,
            message: Message::Hello {
                name: "counter".into(),
            },
        };
        let mut bytes = Vec::new();
        write_to(&mut bytes, &frame).expect("write frame");
        bytes.extend_from_slice(b"remaining");
        let mut cursor = Cursor::new(bytes);
        assert_eq!(read_from(&mut cursor).expect("read frame"), frame);
        assert_eq!(
            usize::try_from(cursor.position()).expect("fixture position fits"),
            encode(&frame).unwrap().len()
        );
    }

    #[test]
    fn stream_rejects_oversized_length_before_allocation() {
        let mut header = [0_u8; HEADER_LEN];
        header[..4].copy_from_slice(&MAGIC);
        header[4] = VERSION;
        header[5] = 1;
        header[6..10].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(
            read_from(&mut Cursor::new(header)),
            Err(StreamError::Protocol(ProtocolError::FrameTooLarge))
        );
    }

    #[test]
    fn encoder_rejects_node_and_list_counts_decoder_would_reject() {
        let nodes = (0..=MAX_NODES)
            .map(|id| Node::Divider {
                id: NodeId(u32::try_from(id).expect("fixture ID")),
            })
            .collect();
        assert_eq!(
            encode(&Frame {
                request_id: 1,
                message: Message::SetScreen(Screen::new(1, nodes)),
            }),
            Err(ProtocolError::TooManyNodes)
        );

        assert_eq!(
            encode(&Frame {
                request_id: 2,
                message: Message::SetScreen(Screen::new(
                    2,
                    vec![Node::PagedList {
                        id: NodeId(1),
                        page: 0,
                        items: vec![String::new(); MAX_NODES + 1],
                    }],
                )),
            }),
            Err(ProtocolError::TooManyNodes)
        );
    }

    #[test]
    fn encoder_preflights_payload_limit() {
        let nodes = (0..65)
            .map(|id| Node::Text {
                id: NodeId(id),
                text: "x".repeat(MAX_STRING_LEN),
            })
            .collect();
        assert_eq!(
            encode(&Frame {
                request_id: 3,
                message: Message::SetScreen(Screen::new(3, nodes)),
            }),
            Err(ProtocolError::FrameTooLarge)
        );
    }
}

#[cfg(test)]
mod node_coverage_tests {
    use super::*;

    /// Every node kind, so the precomputed frame layout is checked against the
    /// real encoder for all of them.
    ///
    /// This exists because `Spacer` carried a stale length of nine bytes for
    /// as long as it encoded an `i32`, and nothing noticed when it became a
    /// single tag byte: no test had ever put a spacer through `encode`. The
    /// encoder asserts its own predicted length in debug builds, so the bug was
    /// one call away from being loud, and was silently over-reserving instead.
    fn one_of_every_node() -> Vec<Node> {
        vec![
            Node::Heading {
                id: NodeId(1),
                text: "Heading".into(),
            },
            Node::Text {
                id: NodeId(2),
                text: "Body".into(),
            },
            Node::Button {
                id: NodeId(3),
                action: ActionId(1),
                label: "Press".into(),
            },
            Node::Card {
                id: NodeId(4),
                children: vec![Node::Text {
                    id: NodeId(5),
                    text: "Nested".into(),
                }],
            },
            Node::Divider { id: NodeId(6) },
            Node::Spacer {
                id: NodeId(7),
                space: Space::Medium,
            },
            Node::Progress {
                id: NodeId(8),
                value: Percent::new(40),
            },
            Node::PagedList {
                id: NodeId(9),
                page: 0,
                items: vec!["one".into(), "two".into()],
            },
            Node::TileGrid {
                id: NodeId(10),
                tiles: vec![
                    Tile::new(ActionId(2), "Reader", Glyph::Reader),
                    Tile::new(ActionId(3), "Notes", Glyph::Note),
                ],
            },
            Node::Rows {
                id: NodeId(20),
                rows: vec![
                    Row::new(
                        ActionId(7),
                        "Hello",
                        "The smallest application.",
                        Glyph::App,
                    ),
                    // An empty summary is legal and must survive the wire.
                    Row::new(ActionId(8), "Counter", "", Glyph::Note),
                ],
            },
            Node::Choice {
                id: NodeId(11),
                prompt: "Pick one".into(),
                options: vec![
                    BarAction::new(ActionId(4), "First"),
                    BarAction::new(ActionId(5), "Second"),
                ],
                freeform: Some(Freeform::new(ActionId(6), "Something else")),
            },
            Node::Banner {
                id: NodeId(12),
                level: BannerLevel::Attention,
                text: "Battery low".into(),
            },
            Node::Skeleton {
                id: NodeId(13),
                lines: 3,
            },
            Node::Activity {
                id: NodeId(14),
                label: "Fetching articles".into(),
                progress: Some(Percent::new(45)),
                cancel: Some(BarAction::new(ActionId(7), "Cancel")),
            },
            Node::Activity {
                id: NodeId(15),
                label: "Connecting".into(),
                progress: None,
                cancel: None,
            },
        ]
    }

    fn round_trip(screen: Screen) -> Screen {
        let frame = Frame {
            request_id: 7,
            message: Message::SetScreen(screen),
        };
        let bytes = encode(&frame).expect("encode");
        match decode(&bytes).expect("decode").message {
            Message::SetScreen(screen) => screen,
            other => panic!("expected a screen, got {other:?}"),
        }
    }

    #[test]
    fn every_node_kind_round_trips_byte_for_byte() {
        for node in one_of_every_node() {
            let screen = Screen::new(1, vec![node.clone()]);
            assert_eq!(
                round_trip(screen).nodes,
                vec![node.clone()],
                "node did not survive the wire: {node:?}"
            );
        }
    }

    #[test]
    fn a_screen_holding_every_node_round_trips() {
        let nodes = one_of_every_node();
        let screen = Screen::new(9, nodes.clone())
            .with_top_bar(TopBar::new(NodeId(100), "Gallery").action(ActionId(50), "Done"))
            .with_nav_bar(NavBar::new(
                NodeId(101),
                vec![
                    BarAction::new(ActionId(60), "Home"),
                    BarAction::new(ActionId(61), "Books"),
                    BarAction::new(ActionId(62), "More"),
                ],
                1,
            ));
        let decoded = round_trip(screen.clone());
        assert_eq!(decoded, screen);
    }

    #[test]
    fn bars_survive_independently_of_each_other() {
        let only_top = Screen::new(1, Vec::new()).with_top_bar(TopBar::new(NodeId(1), "Title"));
        assert_eq!(round_trip(only_top.clone()), only_top);

        let only_nav = Screen::new(2, Vec::new()).with_nav_bar(NavBar::new(
            NodeId(2),
            vec![
                BarAction::new(ActionId(1), "A"),
                BarAction::new(ActionId(2), "B"),
            ],
            0,
        ));
        assert_eq!(round_trip(only_nav.clone()), only_nav);

        let neither = Screen::new(3, Vec::new());
        assert_eq!(round_trip(neither.clone()), neither);
    }

    #[test]
    fn the_reserved_back_action_cannot_arrive_from_an_application() {
        // Going back belongs to the runtime's navigation stack. If an app could
        // name that identifier it could draw a control the reader is entitled
        // to trust and then handle it however it liked.
        let screen = Screen::new(1, Vec::new())
            .with_top_bar(TopBar::new(NodeId(1), "Trap").action(ActionId::BACK, "Back"));
        let frame = Frame {
            request_id: 1,
            message: Message::SetScreen(screen),
        };
        let bytes = encode(&frame).expect("encode");
        assert!(matches!(
            decode(&bytes),
            Err(ProtocolError::InvalidValue("reserved action id"))
        ));
    }

    #[test]
    fn a_nav_bar_with_one_destination_is_rejected() {
        let screen = Screen::new(1, Vec::new()).with_nav_bar(NavBar::new(
            NodeId(1),
            vec![BarAction::new(ActionId(1), "Only")],
            0,
        ));
        let frame = Frame {
            request_id: 1,
            message: Message::SetScreen(screen),
        };
        let bytes = encode(&frame).expect("encode");
        assert!(matches!(
            decode(&bytes),
            Err(ProtocolError::InvalidValue("nav bar destinations"))
        ));
    }

    #[test]
    fn an_out_of_range_selection_clamps_rather_than_losing_navigation() {
        let screen = Screen::new(1, Vec::new()).with_nav_bar(NavBar::new(
            NodeId(1),
            vec![
                BarAction::new(ActionId(1), "A"),
                BarAction::new(ActionId(2), "B"),
            ],
            250,
        ));
        let decoded = round_trip(screen);
        assert_eq!(decoded.nav_bar.expect("nav bar").selected, 1);
    }

    #[test]
    fn a_choice_offering_no_answers_is_rejected() {
        let screen = Screen::new(
            1,
            vec![Node::Choice {
                id: NodeId(1),
                prompt: "Dead end".into(),
                options: Vec::new(),
                freeform: None,
            }],
        );
        let frame = Frame {
            request_id: 1,
            message: Message::SetScreen(screen),
        };
        let bytes = encode(&frame).expect("encode");
        assert!(matches!(
            decode(&bytes),
            Err(ProtocolError::InvalidValue("choice with no answers"))
        ));
    }

    #[test]
    fn unknown_tags_are_rejected_rather_than_guessed() {
        for (label, bytes) in [
            ("glyph", {
                let mut screen = Vec::new();
                let mut count = 0;
                encode_screen(
                    &mut screen,
                    &Screen::new(
                        1,
                        vec![Node::TileGrid {
                            id: NodeId(1),
                            tiles: vec![Tile::new(ActionId(1), "x", Glyph::App)],
                        }],
                    ),
                    0,
                    &mut count,
                )
                .expect("encode");
                let last = screen.len() - 1;
                screen[last] = 200;
                screen
            }),
            ("banner level", {
                let mut screen = Vec::new();
                let mut count = 0;
                encode_screen(
                    &mut screen,
                    &Screen::new(
                        1,
                        vec![Node::Banner {
                            id: NodeId(1),
                            level: BannerLevel::Info,
                            text: String::new(),
                        }],
                    ),
                    0,
                    &mut count,
                )
                .expect("encode");
                // The level byte sits after the screen header, node tag and id.
                let position = screen.len() - 3;
                screen[position] = 9;
                screen
            }),
        ] {
            let mut reader = Reader::new(&bytes);
            let mut count = 0;
            assert!(
                decode_screen(&mut reader, 0, &mut count).is_err(),
                "an unknown {label} tag was accepted"
            );
        }
    }
}

const fn encode_task_error(error: TaskError) -> u8 {
    match error {
        TaskError::Denied => 0,
        TaskError::Unreachable => 1,
        TaskError::TooLarge => 2,
        TaskError::TimedOut => 3,
        TaskError::NotFound => 4,
    }
}

const fn decode_task_error(tag: u8) -> Result<TaskError, ProtocolError> {
    Ok(match tag {
        0 => TaskError::Denied,
        1 => TaskError::Unreachable,
        2 => TaskError::TooLarge,
        3 => TaskError::TimedOut,
        4 => TaskError::NotFound,
        _ => return Err(ProtocolError::InvalidValue("task error")),
    })
}

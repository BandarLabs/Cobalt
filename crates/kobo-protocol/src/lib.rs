#![forbid(unsafe_code)]

//! Versioned, bounded wire format used between Kobo applications and hosts.

use std::fmt;
use std::io::{self, Read, Write};

use kobo_ui::{
    ActionId, BannerLevel, BarAction, BottomAction, Caret, Cell, ControlState, Freeform, Glyph,
    NavBar, Node, NodeId, PageTurns, Percent, PictureHandle, Row, RowLead, RowState, Screen, Space,
    TextScale, Tile, TilePicture, TileShape, TopBar, MAX_TERMINAL_COLUMNS, MAX_TERMINAL_ROWS,
    MIN_NAV_DESTINATIONS,
};
use std::cmp::min;

pub const MAGIC: [u8; 4] = *b"KOBO";
pub const VERSION: u8 = 2;
pub const HEADER_LEN: usize = 14;
pub const MAX_FRAME_LEN: usize = 1_048_576;
/// The largest decoded picture accepted from one application.
///
/// Four Clara panels is the same bound used by `kobo-image`: enough headroom
/// for a high-resolution source while remaining below the per-app cache.
pub const MAX_PICTURE_BYTES: usize = 4 * 1072 * 1448;
/// Largest picture sent as one legacy `PutPicture` frame.
pub const MAX_INLINE_PICTURE_BYTES: usize = 768 * 1024;
/// Largest piece of a chunked upload. Small enough to bound transient copies
/// while still moving a full panel in a handful of local-socket writes.
pub const MAX_PICTURE_CHUNK_BYTES: usize = 256 * 1024;
pub const MAX_STRING_LEN: usize = 16_384;
pub const MAX_NODES: usize = 512;
/// The byte a nav bar sends when no destination is the current one.
///
/// Out of band by construction: the destination count travels in a byte of its
/// own, so a bar with 255 destinations could not name this index anyway, and
/// the panel shows a handful at most.
pub const NAV_SELECTION_NONE: u8 = u8::MAX;
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

/// The longest a stored key may be.
pub const MAX_STORE_KEY_LEN: usize = 64;

/// The largest value an application may keep under one key.
///
/// Generous for state and far too small for content. That asymmetry is the
/// point: this is where an application keeps what it needs to open in the same
/// place it closed, not where it keeps a library.
pub const MAX_STORE_VALUE: usize = 256 * 1024;

/// The most keys one application may hold.
pub const MAX_STORE_KEYS: usize = 256;

/// The prefix that marks a key as one the runtime may throw away.
///
/// Spelled in the key rather than carried beside it so that it survives every
/// path a key takes: the wire, the filename, and a listing. There is no way to
/// write a cache key and forget it was one.
pub const CACHE_PREFIX: &str = "cache.";

/// How many cache keys one application may hold.
///
/// Counted apart from [`MAX_STORE_KEYS`] and capped apart from it, so that
/// artwork a shelf is holding can never crowd out a reading position. A shelf
/// page of covers is six, so this is twenty pages of catalogue.
pub const MAX_CACHE_KEYS: usize = 64;

/// The most keys one listing may name: every durable key and every cache key.
pub const MAX_LISTED_KEYS: usize = MAX_STORE_KEYS + MAX_CACHE_KEYS;

/// The most bytes one shelf write or read may carry.
///
/// A book is megabytes and a frame is one, so a blob moves in pieces. This is
/// the piece: large enough that a ten-megabyte book is forty round trips
/// rather than four hundred, and small enough that neither side is ever
/// holding a frame near the limit while it also holds the thing being built.
pub const MAX_SHELF_CHUNK: usize = 256 * 1024;

/// The most bytes one application may keep on the shelf.
pub const MAX_SHELF_BYTES: u64 = 256 * 1024 * 1024;

/// The most blobs one application may keep.
pub const MAX_SHELF_BLOBS: usize = 4_096;

/// How much of the card must stay free whatever an application asks for.
///
/// `KoboReader.sqlite` shares this partition, and it is the stock reader's
/// entire library. A database with nowhere to write is a library that comes
/// back empty, and nothing about that failure points at us. Sixty-four
/// megabytes is far more than the database needs to grow into and small enough
/// not to matter on a card measured in gigabytes.
pub const SHELF_RESERVE: u64 = 64 * 1024 * 1024;

/// A handle to work the runtime is carrying out on an application's behalf.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TaskId(pub u32);

/// The most headers one request may carry, beyond the ones the runtime sets.
pub const MAX_HEADERS: usize = 8;
/// The longest a header name may be.
pub const MAX_HEADER_NAME: usize = 64;
/// The longest a header value may be.
pub const MAX_HEADER_VALUE: usize = 256;

/// The characters RFC 9110 allows in a header name.
const fn is_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

/// One header an application supplies.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Header {
    pub name: String,
    pub value: String,
}

impl Header {
    #[must_use]
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }

    /// Whether this is something an application may legitimately send.
    ///
    /// Names are checked against the token characters HTTP allows and values
    /// against visible ASCII, because a newline in either would let an
    /// application append headers of its own, including the credential header
    /// it is not allowed to see.
    #[must_use]
    pub fn is_well_formed(&self) -> bool {
        !self.name.is_empty()
            && self.name.len() <= MAX_HEADER_NAME
            && self.value.len() <= MAX_HEADER_VALUE
            && self.name.bytes().all(is_token_byte)
            && self
                .value
                .bytes()
                .all(|byte| (0x20..0x7f).contains(&byte) || byte == b'\t')
    }
}

/// Where a credential goes in the request.
///
/// Bearer is not the only convention, and treating it as if it were means every
/// service that uses another one has to be reached through a proxy that does.
/// Anthropic wants `x-api-key` and Google wants `x-goog-api-key`; naming the
/// header is what lets an application talk to either directly.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SecretHeader {
    /// `Authorization: Bearer <value>`.
    Bearer,
    /// A header carrying the value alone, such as `x-api-key`.
    Named(String),
}

/// A credential an application may use and never see.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Credential {
    /// The name of a secret the runtime holds.
    pub secret: String,
    pub header: SecretHeader,
}

impl Credential {
    /// The usual convention: `Authorization: Bearer <value>`.
    #[must_use]
    pub fn bearer(secret: impl Into<String>) -> Self {
        Self {
            secret: secret.into(),
            header: SecretHeader::Bearer,
        }
    }

    /// A named header, such as `x-api-key` or `x-goog-api-key`.
    #[must_use]
    pub fn in_header(secret: impl Into<String>, header: impl Into<String>) -> Self {
        Self {
            secret: secret.into(),
            header: SecretHeader::Named(header.into()),
        }
    }

    /// The header name this credential will be sent under.
    #[must_use]
    pub fn header_name(&self) -> &str {
        match &self.header {
            SecretHeader::Bearer => "Authorization",
            SecretHeader::Named(name) => name,
        }
    }

    #[must_use]
    pub fn is_well_formed(&self) -> bool {
        if self.secret.is_empty() || self.secret.len() > MAX_HEADER_NAME {
            return false;
        }
        match &self.header {
            SecretHeader::Bearer => true,
            SecretHeader::Named(name) => {
                !name.is_empty() && name.len() <= MAX_HEADER_NAME && name.bytes().all(is_token_byte)
            }
        }
    }
}

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
    /// request needs a credential, the *name* of one, never its value. The
    /// runtime looks the named secret up and attaches it, so an API key is
    /// never in an application's memory, its logs or its crash dump.
    Post {
        url: String,
        body: String,
        content_type: String,
        /// The credential to attach, or `None`.
        credential: Option<Credential>,
        /// Headers the request needs that are not secret.
        ///
        /// Some APIs are unusable without one: Anthropic refuses any request
        /// that does not carry `anthropic-version`. Bounded and validated, and
        /// the headers the runtime owns cannot be set here.
        headers: Vec<Header>,
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
        /// The reader's accessibility preference. Applications receive this
        /// before laying out or paginating any content.
        text_scale: TextScale,
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
    /// An application reading or writing its own small state.
    StoreRequest(StoreRequest),
    /// Sent by the runtime when an application gains or loses the panel.
    Lifecycle(Lifecycle),
    /// The runtime's answer to exactly one store request.
    StoreResult(StoreResult),
    /// An application driving a terminal the runtime owns.
    ShellRequest(ShellRequest),
    /// The runtime reporting what the program on that terminal did.
    ShellEvent(ShellEvent),
    /// Hands a decoded picture to the runtime, to be referred to afterwards by
    /// `handle`.
    ///
    /// Pictures travel once and out of band because a screen is re-sent whole
    /// on every change. Sending one again on each repaint would put a cover on
    /// the wire for every tap, and a shelf of them would exceed a frame.
    ///
    /// Replacing a live handle is allowed and is how an application updates a
    /// picture in place.
    PutPicture {
        handle: PictureHandle,
        width: u32,
        height: u32,
        /// Eight-bit grey, row major, exactly `width * height` bytes.
        grey: Vec<u8>,
    },
    /// Starts an atomic picture upload larger than one protocol frame.
    BeginPicture {
        handle: PictureHandle,
        width: u32,
        height: u32,
    },
    /// One in-order span of a picture started by [`Message::BeginPicture`].
    PictureChunk {
        handle: PictureHandle,
        offset: u32,
        grey: Vec<u8>,
    },
    /// Makes a completely received upload visible to screens.
    CommitPicture {
        handle: PictureHandle,
    },
    /// Releases a picture. The runtime also drops every picture an application
    /// holds when it exits, so this is for applications that outlive their own
    /// pictures rather than a requirement.
    DropPicture {
        handle: PictureHandle,
    },
}

/// The most bytes carried in one direction of a terminal in a single message.
///
/// Output is chunked at this size and input is refused above it. A program
/// printing a large file must not be able to build a frame larger than the
/// panel could ever show, and a bound the sender and receiver both know is the
/// only way a stream stays bounded without either side trusting the other.
pub const MAX_SHELL_CHUNK: usize = 4096;

/// Everything an application can ask of its terminal.
///
/// The application never holds the descriptor. It says what it wants typed and
/// what size the grid is; the runtime owns the pseudo-terminal, the child
/// process and the decision about whether this application may have one at
/// all. That is the same rule as the framebuffer and the network: the
/// dangerous object stays behind the daemon.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ShellRequest {
    /// Starts a terminal of the given grid. One per application.
    Open { columns: u16, rows: u16 },
    /// Keystrokes, already encoded as the bytes a terminal expects.
    Input(Vec<u8>),
    /// The grid changed.
    Resize { columns: u16, rows: u16 },
    /// Ends the program and releases the terminal.
    Close,
}

/// What the runtime reports back about a terminal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ShellEvent {
    /// The terminal exists and the program is running.
    Opened,
    /// Bytes the program printed, in the order it printed them.
    Output(Vec<u8>),
    /// The program finished. A terminal is never reopened implicitly.
    Closed { status: i32 },
    /// The request was refused, and why.
    Refused(ShellError),
}

/// Why a terminal request was refused.
///
/// Distinct reasons rather than one failure, because they call for different
/// answers: a missing permission is a manifest problem the developer fixes, a
/// build without a terminal backend is a platform limit nobody can fix from an
/// application, and asking twice is a bug in the application itself.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum ShellError {
    /// The application did not declare the capability.
    NotPermitted = 0,
    /// This build has no terminal to give.
    Unavailable = 1,
    /// A terminal is already open for this application.
    AlreadyOpen = 2,
    /// There is no terminal to act on.
    NotOpen = 3,
    /// The program could not be started.
    Failed = 4,
}

impl TryFrom<u8> for ShellError {
    type Error = ProtocolError;

    fn try_from(tag: u8) -> Result<Self, ProtocolError> {
        Ok(match tag {
            0 => Self::NotPermitted,
            1 => Self::Unavailable,
            2 => Self::AlreadyOpen,
            3 => Self::NotOpen,
            4 => Self::Failed,
            _ => return Err(ProtocolError::InvalidValue("shell error")),
        })
    }
}

/// Everything an application can ask of its own store.
///
/// # Why keys and not paths
///
/// An application that can name a path can name `../../../etc/init.d/rcS`, and
/// then every caller for the rest of time has to remember to sanitise it. A key
/// namespace deletes the entire class of mistake instead of defending against
/// it: there is no syntax here that can express somewhere else.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StoreRequest {
    /// Writes a value, replacing whatever was there.
    Save { key: String, value: Vec<u8> },
    /// Reads a value back. A key that was never written is not an error.
    Load { key: String },
    /// Removes a key.
    Forget { key: String },
    /// Lists the keys this application has written.
    List,
    /// Writes part of a blob, at a byte offset within it.
    ///
    /// Offsets rather than an append cursor: a write that is retried after a
    /// disconnection must land in the same place, and a cursor the two sides
    /// disagree about is a file with a hole or a repeat in the middle of it.
    /// `last` finishes the blob, which is when it becomes readable under its
    /// name, until then a half-written book is not something that can be
    /// opened and found wanting.
    ShelfWrite {
        name: String,
        offset: u32,
        bytes: Vec<u8>,
        last: bool,
    },
    /// Reads part of a blob.
    ShelfRead {
        name: String,
        offset: u32,
        length: u32,
    },
    /// Removes a blob, and any half-written copy of it.
    ShelfRemove { name: String },
    /// Lists the blobs this application has finished writing, with their sizes.
    ShelfList,
}

/// Where an application stands relative to the panel.
///
/// An application is not stopped when the reader leaves it. It keeps its
/// process, its memory and its work in flight, and is told it is no longer
/// being looked at. Coming back is then instant and shows exactly what was
/// left, which on a device where a restart costs a full refresh and a reload
/// is the difference between switching and starting over.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Lifecycle {
    /// This application owns the panel. Draw.
    Foreground,
    /// Something else owns the panel. Keep working, but nothing drawn now will
    /// be seen until this comes back, so this is the moment to save.
    Background,
}

/// The runtime's answer to exactly one [`StoreRequest`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StoreResult {
    Saved {
        key: String,
    },
    /// `None` means the key has never been written, which is the ordinary
    /// first-run answer rather than a failure.
    Loaded {
        key: String,
        value: Option<Vec<u8>>,
    },
    Forgotten {
        key: String,
    },
    Keys(Vec<String>),
    Denied(StoreError),
    /// A piece of a blob landed. `size` is how much of it exists so far.
    ShelfWritten {
        name: String,
        size: u32,
    },
    /// A piece of a blob. `size` is the whole blob's length, so a reader knows
    /// when to stop asking without a separate round trip to find out.
    ShelfRead {
        name: String,
        offset: u32,
        bytes: Vec<u8>,
        size: u32,
    },
    ShelfRemoved {
        name: String,
    },
    Shelf(Vec<(String, u32)>),
}

/// Why a store request was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum StoreError {
    /// The key was empty, too long, or used a character outside the allowed
    /// set. Refused rather than rewritten, so two keys can never collide into
    /// one after sanitising.
    BadKey = 1,
    /// The value was larger than [`MAX_STORE_VALUE`], or the application
    /// already holds [`MAX_STORE_KEYS`] keys.
    TooFull = 2,
    /// The store itself could not be written, or this session has none. The
    /// previous value survives.
    ///
    /// There is deliberately no "not permitted" here. An application's own
    /// state is not a privilege it has to ask for, any more than a phone asks
    /// permission to remember which tab you were on.
    Unwritable = 3,
    /// The card itself is too near full. Distinct from [`StoreError::TooFull`],
    /// which is about this application's own allowance: this one means the
    /// write was refused to leave the stock reader's library room to breathe,
    /// and deleting something of this application's own may not help.
    NoRoom = 4,
    /// No blob of that name, or the offset does not line up with what is
    /// already there. Writes are appends: a piece that would leave a hole in
    /// the middle of a book is refused rather than padded, because a book with
    /// a hole in it opens and is wrong, which is harder to notice than a book
    /// that does not open at all.
    Missing = 5,
}

impl TryFrom<u8> for StoreError {
    type Error = ProtocolError;

    fn try_from(value: u8) -> Result<Self, ProtocolError> {
        match value {
            1 => Ok(Self::BadKey),
            2 => Ok(Self::TooFull),
            3 => Ok(Self::Unwritable),
            4 => Ok(Self::NoRoom),
            5 => Ok(Self::Missing),
            _ => Err(ProtocolError::InvalidValue("store error")),
        }
    }
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::BadKey => "that is not a usable key",
            Self::TooFull => "there is no room for that",
            Self::Unwritable => "the store could not be written",
            Self::NoRoom => "the card is too nearly full to write that",
            Self::Missing => "there is nothing there to read",
        })
    }
}

/// Whether a key is one the store will accept.
///
/// Lowercase letters, digits, `.`, `-` and `_`. A leading dot is refused so a
/// key can never become a hidden file, and `..` cannot be spelled at all.
#[must_use]
pub fn is_valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= MAX_STORE_KEY_LEN
        && !key.starts_with('.')
        && key.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b".-_".contains(&byte)
        })
}

/// Whether a key is one the runtime may evict to make room for another.
///
/// A cache key holds something that came from somewhere else and can come from
/// there again. Anything that cannot be fetched a second time -- a place in a
/// book, a list of subscriptions -- must not be written under one.
#[must_use]
pub fn is_cache_key(key: &str) -> bool {
    key.starts_with(CACHE_PREFIX)
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
            text_scale,
        } => {
            push_u16(&mut payload, *width);
            push_u16(&mut payload, *height);
            push_u16(&mut payload, *pixels_per_inch);
            payload.push(text_scale.wire_value());
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
        Message::Spawn { .. } | Message::Cancel { .. } | Message::TaskOutcome { .. } => {
            encode_task_message(&mut payload, &frame.message)?;
        }
        Message::StoreRequest(request) => encode_store_request(&mut payload, request)?,
        Message::StoreResult(result) => encode_store_result(&mut payload, result)?,
        Message::ShellRequest(request) => encode_shell_request(&mut payload, request)?,
        Message::ShellEvent(event) => encode_shell_event(&mut payload, event)?,
        Message::PutPicture {
            handle,
            width,
            height,
            grey,
        } => {
            push_u32(&mut payload, handle.0);
            push_u32(&mut payload, *width);
            push_u32(&mut payload, *height);
            payload.extend_from_slice(grey);
        }
        Message::BeginPicture {
            handle,
            width,
            height,
        } => {
            push_u32(&mut payload, handle.0);
            push_u32(&mut payload, *width);
            push_u32(&mut payload, *height);
        }
        Message::PictureChunk {
            handle,
            offset,
            grey,
        } => {
            push_u32(&mut payload, handle.0);
            push_u32(&mut payload, *offset);
            payload.extend_from_slice(grey);
        }
        Message::CommitPicture { handle } | Message::DropPicture { handle } => {
            push_u32(&mut payload, handle.0);
        }
        Message::Lifecycle(state) => payload.push(match state {
            Lifecycle::Foreground => 0,
            Lifecycle::Background => 1,
        }),
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

fn encode_task_message(payload: &mut Vec<u8>, message: &Message) -> Result<(), ProtocolError> {
    match message {
        Message::Spawn { task, work } => {
            push_u32(payload, task.0);
            match work {
                Task::Fetch {
                    url,
                    offset,
                    max_bytes,
                } => {
                    payload.push(0);
                    push_string(payload, url)?;
                    push_u32(payload, *offset);
                    push_u32(payload, *max_bytes);
                }
                Task::ReadFile { path } => {
                    payload.push(1);
                    push_string(payload, path)?;
                }
                Task::Sleep { seconds } => {
                    payload.push(2);
                    push_u32(payload, *seconds);
                }
                Task::Post {
                    url,
                    body,
                    content_type,
                    credential,
                    headers,
                    max_bytes,
                } => {
                    payload.push(3);
                    push_string(payload, url)?;
                    push_string(payload, body)?;
                    push_string(payload, content_type)?;
                    match credential {
                        None => payload.push(0),
                        Some(credential) => match &credential.header {
                            SecretHeader::Bearer => {
                                payload.push(1);
                                push_string(payload, &credential.secret)?;
                            }
                            SecretHeader::Named(name) => {
                                payload.push(2);
                                push_string(payload, name)?;
                                push_string(payload, &credential.secret)?;
                            }
                        },
                    }
                    payload.push(
                        u8::try_from(headers.len())
                            .map_err(|_| ProtocolError::InvalidValue("too many headers"))?,
                    );
                    for header in headers {
                        push_string(payload, &header.name)?;
                        push_string(payload, &header.value)?;
                    }
                    push_u32(payload, *max_bytes);
                }
            }
        }
        Message::Cancel { task } => push_u32(payload, task.0),
        Message::TaskOutcome { task, outcome } => {
            push_u32(payload, task.0);
            match outcome {
                TaskOutcome::Completed(bytes) => {
                    payload.push(0);
                    push_u32(
                        payload,
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
        _ => unreachable!("only task messages reach here"),
    }
    Ok(())
}

fn encode_store_request(
    payload: &mut Vec<u8>,
    request: &StoreRequest,
) -> Result<(), ProtocolError> {
    match request {
        StoreRequest::Save { key, value } => {
            payload.push(0);
            push_string(payload, key)?;
            push_u32(
                payload,
                u32::try_from(value.len()).map_err(|_| ProtocolError::FrameTooLarge)?,
            );
            payload.extend_from_slice(value);
        }
        StoreRequest::Load { key } => {
            payload.push(1);
            push_string(payload, key)?;
        }
        StoreRequest::Forget { key } => {
            payload.push(2);
            push_string(payload, key)?;
        }
        StoreRequest::List => payload.push(3),
        StoreRequest::ShelfWrite {
            name,
            offset,
            bytes,
            last,
        } => {
            payload.push(4);
            push_string(payload, name)?;
            push_u32(payload, *offset);
            payload.push(u8::from(*last));
            push_u32(
                payload,
                u32::try_from(bytes.len()).map_err(|_| ProtocolError::FrameTooLarge)?,
            );
            payload.extend_from_slice(bytes);
        }
        StoreRequest::ShelfRead {
            name,
            offset,
            length,
        } => {
            payload.push(5);
            push_string(payload, name)?;
            push_u32(payload, *offset);
            push_u32(payload, *length);
        }
        StoreRequest::ShelfRemove { name } => {
            payload.push(6);
            push_string(payload, name)?;
        }
        StoreRequest::ShelfList => payload.push(7),
    }
    Ok(())
}

fn encode_store_result(payload: &mut Vec<u8>, result: &StoreResult) -> Result<(), ProtocolError> {
    match result {
        StoreResult::Saved { key } => {
            payload.push(0);
            push_string(payload, key)?;
        }
        StoreResult::Loaded { key, value } => {
            payload.push(1);
            push_string(payload, key)?;
            match value {
                Some(value) => {
                    payload.push(1);
                    push_u32(
                        payload,
                        u32::try_from(value.len()).map_err(|_| ProtocolError::FrameTooLarge)?,
                    );
                    payload.extend_from_slice(value);
                }
                None => payload.push(0),
            }
        }
        StoreResult::Forgotten { key } => {
            payload.push(2);
            push_string(payload, key)?;
        }
        StoreResult::Keys(keys) => {
            payload.push(3);
            push_u16(
                payload,
                u16::try_from(keys.len()).map_err(|_| ProtocolError::FrameTooLarge)?,
            );
            for key in keys {
                push_string(payload, key)?;
            }
        }
        StoreResult::Denied(error) => {
            payload.push(4);
            payload.push(*error as u8);
        }
        StoreResult::ShelfWritten { name, size } => {
            payload.push(5);
            push_string(payload, name)?;
            push_u32(payload, *size);
        }
        StoreResult::ShelfRead {
            name,
            offset,
            bytes,
            size,
        } => {
            payload.push(6);
            push_string(payload, name)?;
            push_u32(payload, *offset);
            push_u32(payload, *size);
            push_u32(
                payload,
                u32::try_from(bytes.len()).map_err(|_| ProtocolError::FrameTooLarge)?,
            );
            payload.extend_from_slice(bytes);
        }
        StoreResult::ShelfRemoved { name } => {
            payload.push(7);
            push_string(payload, name)?;
        }
        StoreResult::Shelf(blobs) => {
            payload.push(8);
            push_u16(
                payload,
                u16::try_from(blobs.len()).map_err(|_| ProtocolError::FrameTooLarge)?,
            );
            for (name, size) in blobs {
                push_string(payload, name)?;
                push_u32(payload, *size);
            }
        }
    }
    Ok(())
}

fn encode_shell_request(
    payload: &mut Vec<u8>,
    request: &ShellRequest,
) -> Result<(), ProtocolError> {
    match request {
        ShellRequest::Open { columns, rows } => {
            payload.push(0);
            push_u16(payload, *columns);
            push_u16(payload, *rows);
        }
        ShellRequest::Input(bytes) => {
            payload.push(1);
            push_shell_bytes(payload, bytes)?;
        }
        ShellRequest::Resize { columns, rows } => {
            payload.push(2);
            push_u16(payload, *columns);
            push_u16(payload, *rows);
        }
        ShellRequest::Close => payload.push(3),
    }
    Ok(())
}

fn encode_shell_event(payload: &mut Vec<u8>, event: &ShellEvent) -> Result<(), ProtocolError> {
    match event {
        ShellEvent::Opened => payload.push(0),
        ShellEvent::Output(bytes) => {
            payload.push(1);
            push_shell_bytes(payload, bytes)?;
        }
        ShellEvent::Closed { status } => {
            payload.push(2);
            // Two's complement, both ways, rather than a cast: an exit status
            // is signed and a cast that clips it would report a killed program
            // as a successful one.
            push_u32(payload, u32::from_ne_bytes(status.to_ne_bytes()));
        }
        ShellEvent::Refused(error) => {
            payload.push(3);
            payload.push(*error as u8);
        }
    }
    Ok(())
}

fn push_shell_bytes(payload: &mut Vec<u8>, bytes: &[u8]) -> Result<(), ProtocolError> {
    if bytes.len() > MAX_SHELL_CHUNK {
        return Err(ProtocolError::FrameTooLarge);
    }
    push_u16(
        payload,
        u16::try_from(bytes.len()).map_err(|_| ProtocolError::FrameTooLarge)?,
    );
    payload.extend_from_slice(bytes);
    Ok(())
}

fn read_shell_bytes(reader: &mut Reader) -> Result<Vec<u8>, ProtocolError> {
    let length = usize::from(reader.u16()?);
    if length > MAX_SHELL_CHUNK {
        return Err(ProtocolError::FrameTooLarge);
    }
    Ok(reader.take(length)?.to_vec())
}

fn shell_request_len(request: &ShellRequest) -> Result<usize, ProtocolError> {
    Ok(match request {
        ShellRequest::Open { .. } | ShellRequest::Resize { .. } => 5,
        ShellRequest::Input(bytes) => shell_chunk_len(bytes)?,
        ShellRequest::Close => 1,
    })
}

fn shell_event_len(event: &ShellEvent) -> Result<usize, ProtocolError> {
    Ok(match event {
        ShellEvent::Opened => 1,
        ShellEvent::Output(bytes) => shell_chunk_len(bytes)?,
        ShellEvent::Closed { .. } => 5,
        ShellEvent::Refused(_) => 2,
    })
}

fn shell_chunk_len(bytes: &[u8]) -> Result<usize, ProtocolError> {
    if bytes.len() > MAX_SHELL_CHUNK {
        return Err(ProtocolError::FrameTooLarge);
    }
    Ok(3 + bytes.len())
}

/// How many bytes one [`Task`] encodes to, identifier and tag included.
fn encoded_task_len(work: &Task) -> Result<usize, ProtocolError> {
    // Four bytes of task identifier and one tag byte. This was six, which made
    // every spawned task claim one byte more than it encodes to, and the debug
    // assertion at the end of `encode` turned that into a panic the moment an
    // application asked for a download, so no application that fetches
    // anything could be opened in the simulator at all.
    let mut length = 5;
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
            credential,
            headers,
            ..
        } => {
            // Refused here rather than stripped silently. A request that
            // quietly loses the header an API requires fails at the far end
            // with an error the author cannot connect to anything they wrote.
            if headers.len() > MAX_HEADERS || headers.iter().any(|header| !header.is_well_formed())
            {
                return Err(ProtocolError::InvalidValue("request header"));
            }
            if credential
                .as_ref()
                .is_some_and(|credential| !credential.is_well_formed())
            {
                return Err(ProtocolError::InvalidValue("credential"));
            }
            add_encoded_len(&mut length, 6)?;
            add_encoded_len(&mut length, encoded_string_len(url)?)?;
            add_encoded_len(&mut length, encoded_string_len(body)?)?;
            add_encoded_len(&mut length, encoded_string_len(content_type)?)?;
            if let Some(credential) = credential {
                add_encoded_len(&mut length, encoded_string_len(&credential.secret)?)?;
                if let SecretHeader::Named(name) = &credential.header {
                    add_encoded_len(&mut length, encoded_string_len(name)?)?;
                }
            }
            for header in headers {
                add_encoded_len(&mut length, encoded_string_len(&header.name)?)?;
                add_encoded_len(&mut length, encoded_string_len(&header.value)?)?;
            }
        }
    }
    Ok(length)
}

fn encoded_message_layout(message: &Message) -> Result<(u8, usize), ProtocolError> {
    match message {
        Message::Hello { name } => Ok((1, encoded_string_len(name)?)),
        Message::Welcome { .. } => Ok((2, 7)),
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
        Message::Spawn { work, .. } => Ok((9, encoded_task_len(work)?)),
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
        Message::StoreRequest(request) => Ok((13, store_request_len(request)?)),
        Message::StoreResult(result) => Ok((14, store_result_len(result)?)),
        Message::Lifecycle(_) => Ok((15, 1)),
        Message::ShellRequest(request) => Ok((16, shell_request_len(request)?)),
        Message::ShellEvent(event) => Ok((17, shell_event_len(event)?)),
        Message::PutPicture {
            width,
            height,
            grey,
            ..
        } => {
            // The declared size and the bytes must agree before anything is
            // allocated on the strength of either, or a decoder reading by
            // dimension would run off the end of a short payload.
            let expected = picture_len(*width, *height)?;
            if expected != grey.len() {
                return Err(ProtocolError::InvalidValue("picture size"));
            }
            if grey.len() > MAX_INLINE_PICTURE_BYTES {
                return Err(ProtocolError::FrameTooLarge);
            }
            Ok((18, 12 + grey.len()))
        }
        Message::DropPicture { .. } => Ok((19, 4)),
        Message::BeginPicture { width, height, .. } => {
            let expected = picture_len(*width, *height)?;
            if expected == 0 || expected > MAX_PICTURE_BYTES {
                return Err(ProtocolError::FrameTooLarge);
            }
            Ok((20, 12))
        }
        Message::PictureChunk { offset, grey, .. } => {
            if grey.is_empty()
                || grey.len() > MAX_PICTURE_CHUNK_BYTES
                || usize::try_from(*offset)
                    .ok()
                    .and_then(|offset| offset.checked_add(grey.len()))
                    .is_none_or(|end| end > MAX_PICTURE_BYTES)
            {
                return Err(ProtocolError::FrameTooLarge);
            }
            Ok((21, 8 + grey.len()))
        }
        Message::CommitPicture { .. } => Ok((22, 4)),
    }
}

fn picture_len(width: u32, height: u32) -> Result<usize, ProtocolError> {
    usize::try_from(width)
        .ok()
        .and_then(|width| width.checked_mul(usize::try_from(height).ok()?))
        .ok_or(ProtocolError::FrameTooLarge)
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
    // One flag byte for first refusal on the runtime's Back control, one for a
    // text size this screen asks for in place of the reader's own, and one for
    // whether its text is a book rather than an interface.
    add_encoded_len(&mut length, 3)?;
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
    // One flag byte for the pinned control, plus its node, action and label
    // when there is one and no bar has already claimed the band.
    add_encoded_len(&mut length, 1)?;
    if let Some(bottom) = &screen.bottom_action {
        if screen.nav_bar.is_none() {
            add_encoded_len(&mut length, 8)?;
            add_encoded_len(&mut length, encoded_string_len(&bottom.action.label)?)?;
        }
    }
    for node in &screen.nodes {
        add_encoded_len(&mut length, encoded_node_len(node, depth, count)?)?;
    }
    // The presence flag, and when there is one: the id, the kind, the anchor a
    // popover names, the title and the count of its nodes.
    add_encoded_len(&mut length, 1)?;
    if let Some(overlay) = &screen.overlay {
        add_encoded_len(&mut length, 7)?;
        if matches!(overlay.kind, kobo_ui::OverlayKind::Popover { .. }) {
            add_encoded_len(&mut length, 4)?;
        }
        add_encoded_len(&mut length, encoded_string_len(&overlay.title)?)?;
        for node in &overlay.nodes {
            add_encoded_len(&mut length, encoded_node_len(node, depth, count)?)?;
        }
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
        Node::Heading { text, .. } | Node::Text { text, .. } | Node::Secondary { text, .. } => {
            let mut length = 5;
            add_encoded_len(&mut length, encoded_string_len(text)?)?;
            length
        }
        Node::Quote { text, fold, .. } => {
            // id, depth, role, whether it folds, then the text. A fold costs
            // seven more: the action, whether it is shut, and the count.
            let mut length = 8 + if fold.is_some() { 7 } else { 0 };
            add_encoded_len(&mut length, encoded_string_len(text)?)?;
            length
        }
        Node::Button { label, .. } => {
            // tag, id, action, state, emphasis, then the label.
            let mut length = 11;
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
                // Four bytes of action, three of lead and one of state, then
                // both strings.
                add_encoded_len(&mut length, 8)?;
                add_encoded_len(&mut length, encoded_string_len(&row.title)?)?;
                add_encoded_len(&mut length, encoded_string_len(&row.summary)?)?;
            }
            length
        }
        Node::TileGrid { tiles, .. } => {
            if tiles.len() > u8::MAX as usize {
                return Err(ProtocolError::TooManyNodes);
            }
            let mut length = 7;
            for tile in tiles {
                add_encoded_len(&mut length, 6)?;
                add_encoded_len(&mut length, encoded_string_len(&tile.label)?)?;
                if tile.picture.is_some() {
                    add_encoded_len(&mut length, 12)?;
                }
            }
            length
        }
        Node::Picture { .. } => 19,
        Node::Choice {
            prompt,
            options,
            freeform,
            ..
        } => {
            if options.len() > u8::MAX as usize {
                return Err(ProtocolError::TooManyNodes);
            }
            let mut length = 8;
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
        Node::Terminal { rows, cursor, .. } => {
            if rows.len() > MAX_TERMINAL_ROWS {
                return Err(ProtocolError::TooManyNodes);
            }
            let mut length = 6;
            for row in rows {
                if row.chars().count() > MAX_TERMINAL_COLUMNS {
                    return Err(ProtocolError::FrameTooLarge);
                }
                add_encoded_len(&mut length, encoded_string_len(row)?)?;
            }
            add_encoded_len(&mut length, 1)?;
            if cursor.is_some() {
                add_encoded_len(&mut length, 4)?;
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
            text_scale: TextScale::from_wire(reader.u8()?)
                .ok_or(ProtocolError::InvalidValue("text scale"))?,
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
                    let credential = match reader.u8()? {
                        0 => None,
                        1 => Some(Credential::bearer(reader.string()?)),
                        2 => {
                            let header = reader.string()?;
                            Some(Credential::in_header(reader.string()?, header))
                        }
                        _ => return Err(ProtocolError::InvalidValue("credential")),
                    };
                    if credential
                        .as_ref()
                        .is_some_and(|credential| !credential.is_well_formed())
                    {
                        return Err(ProtocolError::InvalidValue("credential"));
                    }
                    let count = usize::from(reader.u8()?);
                    if count > MAX_HEADERS {
                        return Err(ProtocolError::InvalidValue("too many headers"));
                    }
                    let mut headers = Vec::with_capacity(count);
                    for _ in 0..count {
                        let header = Header::new(reader.string()?, reader.string()?);
                        if !header.is_well_formed() {
                            return Err(ProtocolError::InvalidValue("request header"));
                        }
                        headers.push(header);
                    }
                    Task::Post {
                        url,
                        body,
                        content_type,
                        credential,
                        headers,
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
        13 => Message::StoreRequest(match reader.u8()? {
            0 => {
                let key = reader.string()?;
                let length = reader.u32()? as usize;
                if length > MAX_STORE_VALUE {
                    return Err(ProtocolError::FrameTooLarge);
                }
                StoreRequest::Save {
                    key,
                    value: reader.take(length)?.to_vec(),
                }
            }
            1 => StoreRequest::Load {
                key: reader.string()?,
            },
            2 => StoreRequest::Forget {
                key: reader.string()?,
            },
            3 => StoreRequest::List,
            4 => {
                let name = reader.string()?;
                let offset = reader.u32()?;
                let last = match reader.u8()? {
                    0 => false,
                    1 => true,
                    _ => return Err(ProtocolError::InvalidValue("shelf finished flag")),
                };
                let length = reader.u32()? as usize;
                if length > MAX_SHELF_CHUNK {
                    return Err(ProtocolError::FrameTooLarge);
                }
                StoreRequest::ShelfWrite {
                    name,
                    offset,
                    bytes: reader.take(length)?.to_vec(),
                    last,
                }
            }
            5 => StoreRequest::ShelfRead {
                name: reader.string()?,
                offset: reader.u32()?,
                length: reader.u32()?,
            },
            6 => StoreRequest::ShelfRemove {
                name: reader.string()?,
            },
            7 => StoreRequest::ShelfList,
            _ => return Err(ProtocolError::InvalidValue("store request")),
        }),
        14 => Message::StoreResult(match reader.u8()? {
            0 => StoreResult::Saved {
                key: reader.string()?,
            },
            1 => {
                let key = reader.string()?;
                let value = match reader.u8()? {
                    0 => None,
                    1 => {
                        let length = reader.u32()? as usize;
                        if length > MAX_STORE_VALUE {
                            return Err(ProtocolError::FrameTooLarge);
                        }
                        Some(reader.take(length)?.to_vec())
                    }
                    _ => return Err(ProtocolError::InvalidValue("stored value")),
                };
                StoreResult::Loaded { key, value }
            }
            2 => StoreResult::Forgotten {
                key: reader.string()?,
            },
            3 => {
                let count = reader.u16()? as usize;
                // Both namespaces, because a listing names every key an
                // application holds and a cache key is one of those.
                if count > MAX_LISTED_KEYS {
                    return Err(ProtocolError::FrameTooLarge);
                }
                let mut keys = Vec::with_capacity(count);
                for _ in 0..count {
                    keys.push(reader.string()?);
                }
                StoreResult::Keys(keys)
            }
            4 => StoreResult::Denied(StoreError::try_from(reader.u8()?)?),
            5 => StoreResult::ShelfWritten {
                name: reader.string()?,
                size: reader.u32()?,
            },
            6 => {
                let name = reader.string()?;
                let offset = reader.u32()?;
                let size = reader.u32()?;
                let length = reader.u32()? as usize;
                if length > MAX_SHELF_CHUNK {
                    return Err(ProtocolError::FrameTooLarge);
                }
                StoreResult::ShelfRead {
                    name,
                    offset,
                    bytes: reader.take(length)?.to_vec(),
                    size,
                }
            }
            7 => StoreResult::ShelfRemoved {
                name: reader.string()?,
            },
            8 => {
                let count = reader.u16()? as usize;
                if count > MAX_STORE_KEYS {
                    return Err(ProtocolError::FrameTooLarge);
                }
                let mut blobs = Vec::with_capacity(count);
                for _ in 0..count {
                    let name = reader.string()?;
                    blobs.push((name, reader.u32()?));
                }
                StoreResult::Shelf(blobs)
            }
            _ => return Err(ProtocolError::InvalidValue("store result")),
        }),
        15 => Message::Lifecycle(match reader.u8()? {
            0 => Lifecycle::Foreground,
            1 => Lifecycle::Background,
            _ => return Err(ProtocolError::InvalidValue("lifecycle state")),
        }),
        16 => Message::ShellRequest(match reader.u8()? {
            0 => ShellRequest::Open {
                columns: reader.u16()?,
                rows: reader.u16()?,
            },
            1 => ShellRequest::Input(read_shell_bytes(&mut reader)?),
            2 => ShellRequest::Resize {
                columns: reader.u16()?,
                rows: reader.u16()?,
            },
            3 => ShellRequest::Close,
            _ => return Err(ProtocolError::InvalidValue("shell request")),
        }),
        17 => Message::ShellEvent(match reader.u8()? {
            0 => ShellEvent::Opened,
            1 => ShellEvent::Output(read_shell_bytes(&mut reader)?),
            2 => ShellEvent::Closed {
                status: i32::from_ne_bytes(reader.u32()?.to_ne_bytes()),
            },
            3 => ShellEvent::Refused(ShellError::try_from(reader.u8()?)?),
            _ => return Err(ProtocolError::InvalidValue("shell event")),
        }),
        18 => {
            let handle = PictureHandle(reader.u32()?);
            let width = reader.u32()?;
            let height = reader.u32()?;
            let expected = picture_len(width, height)?;
            if expected > MAX_INLINE_PICTURE_BYTES {
                return Err(ProtocolError::FrameTooLarge);
            }
            let grey = reader.take(expected)?.to_vec();
            Message::PutPicture {
                handle,
                width,
                height,
                grey,
            }
        }
        19 => Message::DropPicture {
            handle: PictureHandle(reader.u32()?),
        },
        20 => {
            let handle = PictureHandle(reader.u32()?);
            let width = reader.u32()?;
            let height = reader.u32()?;
            let expected = picture_len(width, height)?;
            if expected == 0 || expected > MAX_PICTURE_BYTES {
                return Err(ProtocolError::FrameTooLarge);
            }
            Message::BeginPicture {
                handle,
                width,
                height,
            }
        }
        21 => {
            let handle = PictureHandle(reader.u32()?);
            let offset = reader.u32()?;
            let length = reader.remaining();
            if length == 0 || length > MAX_PICTURE_CHUNK_BYTES {
                return Err(ProtocolError::FrameTooLarge);
            }
            let end = usize::try_from(offset)
                .ok()
                .and_then(|offset| offset.checked_add(length))
                .ok_or(ProtocolError::FrameTooLarge)?;
            if end > MAX_PICTURE_BYTES {
                return Err(ProtocolError::FrameTooLarge);
            }
            Message::PictureChunk {
                handle,
                offset,
                grey: reader.take(length)?.to_vec(),
            }
        }
        22 => Message::CommitPicture {
            handle: PictureHandle(reader.u32()?),
        },
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
            // Refused here as well as on the way in. A bar of one destination
            // is a bar that says nothing about where else the reader could go,
            // and the decoder has always rejected it, so an application that
            // built one encoded happily, killed the runtime's reader thread on
            // arrival, and then sat waiting forever for an event from a
            // connection nobody was reading any more. Failing at the encoder
            // turns that into an error the application sees immediately.
            if nav_bar.destinations.len() < MIN_NAV_DESTINATIONS {
                return Err(ProtocolError::InvalidValue("nav bar destinations"));
            }
            output.push(1);
            push_u32(output, nav_bar.id.0);
            let len = u8::try_from(nav_bar.destinations.len())
                .map_err(|_| ProtocolError::TooManyNodes)?;
            output.push(len);
            // 255 is the "no destination is current" sentinel. A bar can never
            // have that many destinations (the length above is a byte and the
            // panel clamps to a handful) so the value is safely out of band,
            // and it has to be expressible: without it a bar of actions is
            // forced to claim one of them is where the reader is.
            output.push(match nav_bar.selected {
                Some(selected) => u8::try_from(selected)
                    .map_err(|_| ProtocolError::InvalidValue("nav bar selection"))?
                    .min(NAV_SELECTION_NONE - 1),
                None => NAV_SELECTION_NONE,
            });
            for destination in &nav_bar.destinations {
                encode_bar_action(output, destination)?;
            }
        }
    }
    // Written after the bar and never instead of it, so a peer decoding an
    // older screen reads a zero here and carries on. The two are mutually
    // exclusive by construction rather than by agreement: a screen that
    // somehow carries both sends only the bar, because that is what the
    // reserved band was measured for.
    match &screen.bottom_action {
        Some(bottom) if screen.nav_bar.is_none() => {
            output.push(1);
            push_u32(output, bottom.id.0);
            encode_bar_action(output, &bottom.action)?;
        }
        _ => output.push(0),
    }
    match &screen.page_turns {
        None => output.push(0),
        Some(turns) => {
            output.push(1);
            push_u32(output, turns.previous.0);
            push_u32(output, turns.next.0);
        }
    }
    output.push(u8::from(screen.owns_back));
    // Zero means inherit, so a screen that says nothing keeps the reader's own
    // setting and the byte costs nothing to leave alone.
    output.push(screen.text_scale.map_or(0, |scale| scale.wire_value() + 1));
    output.push(u8::from(screen.reading));
    push_u16(
        output,
        u16::try_from(screen.nodes.len()).map_err(|_| ProtocolError::TooManyNodes)?,
    );
    for node in &screen.nodes {
        encode_node(output, node, depth, count)?;
    }
    // Last, and after the node count, so the nodes are read the same way with
    // or without one. An overlay's nodes are counted against the same budget
    // as the screen's: a dialogue is not a way to smuggle another screen's
    // worth of nodes past the limit.
    match &screen.overlay {
        None => output.push(0),
        Some(overlay) => {
            output.push(1);
            push_u32(output, overlay.id.0);
            match overlay.kind {
                kobo_ui::OverlayKind::Modal => output.push(0),
                kobo_ui::OverlayKind::Popover { anchor } => {
                    output.push(1);
                    push_u32(output, anchor.0);
                }
            }
            push_string(output, &overlay.title)?;
            push_u16(
                output,
                u16::try_from(overlay.nodes.len()).map_err(|_| ProtocolError::TooManyNodes)?,
            );
            for node in &overlay.nodes {
                encode_node(output, node, depth, count)?;
            }
        }
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
        Node::Secondary { id, text } => {
            output.push(19);
            push_u32(output, id.0);
            push_string(output, text)?;
        }
        Node::Quote {
            id,
            depth,
            role,
            text,
            fold,
        } => {
            output.push(18);
            push_u32(output, id.0);
            output.push((*depth).min(kobo_ui::MAX_QUOTE_DEPTH));
            output.push(match role {
                kobo_ui::QuoteRole::Body => 0,
                kobo_ui::QuoteRole::Byline => 1,
            });
            // A flag rather than a reserved action id, because zero is a
            // perfectly ordinary action and there is no value to spare.
            if let Some(fold) = fold {
                output.push(1);
                push_u32(output, fold.action.0);
                output.push(u8::from(fold.collapsed));
                push_u16(output, fold.hidden);
            } else {
                output.push(0);
            }
            push_string(output, text)?;
        }
        Node::Button {
            id,
            action,
            label,
            state,
            emphasis,
        } => {
            output.push(3);
            push_u32(output, id.0);
            push_u32(output, action.0);
            output.push(match state {
                ControlState::Enabled => 0,
                ControlState::Disabled => 1,
            });
            output.push(match emphasis {
                kobo_ui::Emphasis::Normal => 0,
                kobo_ui::Emphasis::Primary => 1,
            });
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
                push_row_lead(output, row.lead);
                output.push(encode_row_state(row.state));
            }
        }
        Node::TileGrid { id, tiles, shape } => {
            output.push(9);
            push_u32(output, id.0);
            output.push(match shape {
                TileShape::Square => 0,
                TileShape::Portrait => 1,
            });
            output.push(u8::try_from(tiles.len()).map_err(|_| ProtocolError::TooManyNodes)?);
            for tile in tiles {
                push_u32(output, tile.action.0);
                push_string(output, &tile.label)?;
                output.push(encode_glyph(tile.glyph));
                match tile.picture {
                    Some(picture) => {
                        output.push(1);
                        push_u32(output, picture.handle.0);
                        push_u32(output, picture.source.0);
                        push_u32(output, picture.source.1);
                    }
                    None => output.push(0),
                }
            }
        }
        Node::Picture {
            id,
            handle,
            source,
            max_height_tenths_mm,
        } => {
            output.push(17);
            push_u32(output, id.0);
            push_u32(output, handle.0);
            push_u32(output, source.0);
            push_u32(output, source.1);
            push_u16(output, *max_height_tenths_mm);
        }
        Node::Choice {
            id,
            prompt,
            options,
            selected,
            freeform,
        } => {
            output.push(10);
            push_u32(output, id.0);
            push_string(output, prompt)?;
            output.push(u8::try_from(options.len()).map_err(|_| ProtocolError::TooManyNodes)?);
            for option in options {
                encode_bar_action(output, option)?;
            }
            // Sent as one past the index so that "no answer yet" is zero, which
            // is what a peer that never sets it produces.
            output.push(match selected {
                Some(index) if usize::from(*index) < options.len() => index.saturating_add(1),
                _ => 0,
            });
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
        Node::Terminal { id, rows, cursor } => {
            if rows.len() > MAX_TERMINAL_ROWS {
                return Err(ProtocolError::TooManyNodes);
            }
            output.push(16);
            push_u32(output, id.0);
            output.push(u8::try_from(rows.len()).map_err(|_| ProtocolError::TooManyNodes)?);
            for row in rows {
                if row.chars().count() > MAX_TERMINAL_COLUMNS {
                    return Err(ProtocolError::FrameTooLarge);
                }
                push_string(output, row)?;
            }
            match cursor {
                None => output.push(0),
                Some(caret) => {
                    output.push(1);
                    push_u16(output, caret.row);
                    push_u16(output, caret.column);
                }
            }
        }
    }
    Ok(())
}

const fn encode_row_state(state: RowState) -> u8 {
    match state {
        RowState::Open => 0,
        RowState::Done => 1,
    }
}

// Rejected rather than defaulted. A state nobody defined is a sender this
// receiver does not understand, and guessing "not done" for it would quietly
// show a finished task as outstanding.
const fn decode_row_state(tag: u8) -> Option<RowState> {
    Some(match tag {
        0 => RowState::Open,
        1 => RowState::Done,
        _ => return None,
    })
}

fn store_request_len(request: &StoreRequest) -> Result<usize, ProtocolError> {
    let mut length = 1;
    match request {
        StoreRequest::Save { key, value } => {
            if value.len() > MAX_STORE_VALUE {
                return Err(ProtocolError::FrameTooLarge);
            }
            add_encoded_len(&mut length, encoded_string_len(key)?)?;
            add_encoded_len(&mut length, 4)?;
            add_encoded_len(&mut length, value.len())?;
        }
        StoreRequest::Load { key } | StoreRequest::Forget { key } => {
            add_encoded_len(&mut length, encoded_string_len(key)?)?;
        }
        StoreRequest::List | StoreRequest::ShelfList => {}
        StoreRequest::ShelfWrite { name, bytes, .. } => {
            if bytes.len() > MAX_SHELF_CHUNK {
                return Err(ProtocolError::FrameTooLarge);
            }
            add_encoded_len(&mut length, encoded_string_len(name)?)?;
            // Offset, the finished flag, and the length that precedes the
            // bytes themselves.
            add_encoded_len(&mut length, 4 + 1 + 4)?;
            add_encoded_len(&mut length, bytes.len())?;
        }
        StoreRequest::ShelfRead { name, .. } => {
            add_encoded_len(&mut length, encoded_string_len(name)?)?;
            add_encoded_len(&mut length, 4 + 4)?;
        }
        StoreRequest::ShelfRemove { name } => {
            add_encoded_len(&mut length, encoded_string_len(name)?)?;
        }
    }
    Ok(length)
}

fn store_result_len(result: &StoreResult) -> Result<usize, ProtocolError> {
    let mut length = 1;
    match result {
        StoreResult::Saved { key } | StoreResult::Forgotten { key } => {
            add_encoded_len(&mut length, encoded_string_len(key)?)?;
        }
        StoreResult::Loaded { key, value } => {
            add_encoded_len(&mut length, encoded_string_len(key)?)?;
            add_encoded_len(&mut length, 1)?;
            if let Some(value) = value {
                if value.len() > MAX_STORE_VALUE {
                    return Err(ProtocolError::FrameTooLarge);
                }
                add_encoded_len(&mut length, 4)?;
                add_encoded_len(&mut length, value.len())?;
            }
        }
        StoreResult::Keys(keys) => {
            if keys.len() > MAX_STORE_KEYS {
                return Err(ProtocolError::FrameTooLarge);
            }
            add_encoded_len(&mut length, 2)?;
            for key in keys {
                add_encoded_len(&mut length, encoded_string_len(key)?)?;
            }
        }
        StoreResult::Denied(_) => add_encoded_len(&mut length, 1)?,
        StoreResult::ShelfWritten { name, .. } => {
            add_encoded_len(&mut length, encoded_string_len(name)?)?;
            add_encoded_len(&mut length, 4)?;
        }
        StoreResult::ShelfRemoved { name } => {
            add_encoded_len(&mut length, encoded_string_len(name)?)?;
        }
        StoreResult::ShelfRead { name, bytes, .. } => {
            if bytes.len() > MAX_SHELF_CHUNK {
                return Err(ProtocolError::FrameTooLarge);
            }
            add_encoded_len(&mut length, encoded_string_len(name)?)?;
            // Offset, whole size, and the length that precedes the bytes.
            add_encoded_len(&mut length, 4 + 4 + 4)?;
            add_encoded_len(&mut length, bytes.len())?;
        }
        StoreResult::Shelf(blobs) => {
            if blobs.len() > MAX_STORE_KEYS {
                return Err(ProtocolError::FrameTooLarge);
            }
            add_encoded_len(&mut length, 2)?;
            for (name, _) in blobs {
                add_encoded_len(&mut length, encoded_string_len(name)?)?;
                add_encoded_len(&mut length, 4)?;
            }
        }
    }
    Ok(length)
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
        Glyph::Circle => 13,
        Glyph::Check => 14,
        Glyph::Terminal => 15,
        Glyph::Chat => 16,
        Glyph::News => 17,
        Glyph::Rss => 18,
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
        13 => Glyph::Circle,
        14 => Glyph::Check,
        15 => Glyph::Terminal,
        16 => Glyph::Chat,
        17 => Glyph::News,
        18 => Glyph::Rss,
        _ => return None,
    })
}

#[allow(clippy::too_many_lines)]
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
                // with no navigation at all. `None` is not a mistake, though,
                // so it is passed through rather than clamped onto the last
                // destination, which is what used to happen.
                selected: if selected == usize::from(NAV_SELECTION_NONE) {
                    None
                } else {
                    Some(min(selected, destinations.len() - 1))
                },
                destinations,
            })
        }
        _ => return Err(ProtocolError::InvalidValue("nav bar flag")),
    };
    let bottom_action = match reader.u8()? {
        0 => None,
        1 => Some(BottomAction::new(
            NodeId(reader.u32()?),
            decode_bar_action(reader)?,
        )),
        _ => return Err(ProtocolError::InvalidValue("bottom action flag")),
    };
    let page_turns = match reader.u8()? {
        0 => None,
        1 => Some(PageTurns::new(
            ActionId(reader.u32()?),
            ActionId(reader.u32()?),
        )),
        _ => return Err(ProtocolError::InvalidValue("page turn flag")),
    };
    let owns_back = match reader.u8()? {
        0 => false,
        1 => true,
        _ => return Err(ProtocolError::InvalidValue("own back flag")),
    };
    let text_scale = match reader.u8()? {
        0 => None,
        value => {
            Some(TextScale::from_wire(value - 1).ok_or(ProtocolError::InvalidValue("text scale"))?)
        }
    };
    let reading = match reader.u8()? {
        0 => false,
        1 => true,
        _ => return Err(ProtocolError::InvalidValue("reading flag")),
    };
    let count_nodes = usize::from(reader.u16()?);
    if count_nodes > MAX_NODES {
        return Err(ProtocolError::TooManyNodes);
    }
    let mut nodes = Vec::with_capacity(count_nodes);
    for _ in 0..count_nodes {
        nodes.push(decode_node(reader, depth, count)?);
    }
    let overlay = match reader.u8()? {
        0 => None,
        1 => {
            let id = NodeId(reader.u32()?);
            let kind = match reader.u8()? {
                0 => kobo_ui::OverlayKind::Modal,
                1 => kobo_ui::OverlayKind::Popover {
                    anchor: ActionId(reader.u32()?),
                },
                _ => return Err(ProtocolError::InvalidValue("overlay kind")),
            };
            let title = reader.string()?;
            let count_overlay = usize::from(reader.u16()?);
            if count_overlay > MAX_NODES {
                return Err(ProtocolError::TooManyNodes);
            }
            let mut overlay_nodes = Vec::with_capacity(count_overlay);
            for _ in 0..count_overlay {
                overlay_nodes.push(decode_node(reader, depth, count)?);
            }
            Some(Box::new(kobo_ui::Overlay {
                id,
                kind,
                title,
                nodes: overlay_nodes,
            }))
        }
        _ => return Err(ProtocolError::InvalidValue("overlay flag")),
    };
    let mut screen = Screen::new(id, nodes);
    screen.overlay = overlay;
    screen.top_bar = top_bar;
    screen.nav_bar = nav_bar;
    // Only when there is no bar. A frame carrying both is a peer that built
    // something this layer refuses to draw, and the bar is the one the content
    // above it was laid out against.
    if screen.nav_bar.is_none() {
        screen.bottom_action = bottom_action;
    }
    screen.page_turns = page_turns;
    screen.owns_back = owns_back;
    screen.text_scale = text_scale;
    screen.reading = reading;
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
        18 => {
            let depth = reader.u8()?;
            let role = reader.u8()?;
            // Anything other than the flag we write means no fold, on the same
            // principle as the role: a frame from a newer application should
            // still be readable as the comment it is.
            let fold = if reader.u8()? == 1 {
                Some(kobo_ui::Fold {
                    action: ActionId(reader.u32()?),
                    collapsed: reader.u8()? == 1,
                    hidden: reader.u16()?,
                })
            } else {
                None
            };
            Ok(Node::Quote {
                id,
                // Clamped rather than rejected: a depth past the cap is a
                // deeper reply, not a malformed frame, and the renderer was
                // always going to draw it at the cap anyway.
                depth: depth.min(kobo_ui::MAX_QUOTE_DEPTH),
                // An unknown role is prose. A frame from a newer application
                // that has invented a third kind of line should still be
                // readable, and the thing it certainly is not is a byline.
                role: match role {
                    1 => kobo_ui::QuoteRole::Byline,
                    _ => kobo_ui::QuoteRole::Body,
                },
                fold,
                text: reader.string()?,
            })
        }
        3 => Ok(Node::Button {
            id,
            action: ActionId(reader.u32()?),
            state: match reader.u8()? {
                0 => ControlState::Enabled,
                1 => ControlState::Disabled,
                _ => return Err(ProtocolError::InvalidValue("control state")),
            },
            // An unrecognised emphasis is the quiet one. Guessing "primary"
            // for a value we do not understand would let a future application
            // fill every control on a screen by accident.
            emphasis: match reader.u8()? {
                1 => kobo_ui::Emphasis::Primary,
                _ => kobo_ui::Emphasis::Normal,
            },
            label: reader.string()?,
        }),
        19 => Ok(Node::Secondary {
            id,
            text: reader.string()?,
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
            let shape = match reader.u8()? {
                0 => TileShape::Square,
                1 => TileShape::Portrait,
                _ => return Err(ProtocolError::InvalidValue("tile shape")),
            };
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
                let picture = match reader.u8()? {
                    0 => None,
                    1 => Some(TilePicture {
                        handle: PictureHandle(reader.u32()?),
                        source: (reader.u32()?, reader.u32()?),
                    }),
                    _ => return Err(ProtocolError::InvalidValue("tile picture flag")),
                };
                tiles.push(Tile {
                    action,
                    label,
                    glyph,
                    picture,
                });
            }
            Ok(Node::TileGrid { id, tiles, shape })
        }
        17 => Ok(Node::Picture {
            id,
            handle: PictureHandle(reader.u32()?),
            source: (reader.u32()?, reader.u32()?),
            max_height_tenths_mm: reader.u16()?,
        }),
        10 => {
            let prompt = reader.string()?;
            let len = usize::from(reader.u8()?);
            let mut options = Vec::with_capacity(len);
            for _ in 0..len {
                options.push(decode_bar_action(reader)?);
            }
            // Clamped rather than refused: an answer that does not name one of
            // the options is a caller mistake, and refusing the frame would
            // cost the whole screen over a marker.
            let selected = match reader.u8()? {
                0 => None,
                marked if usize::from(marked) <= options.len() => Some(marked - 1),
                _ => None,
            };
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
                selected,
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
        16 => {
            let count = usize::from(reader.u8()?);
            if count > MAX_TERMINAL_ROWS {
                return Err(ProtocolError::TooManyNodes);
            }
            let mut rows = Vec::with_capacity(count);
            for _ in 0..count {
                let row = reader.string()?;
                if row.chars().count() > MAX_TERMINAL_COLUMNS {
                    return Err(ProtocolError::InvalidValue("terminal row too wide"));
                }
                rows.push(row);
            }
            let cursor = match reader.u8()? {
                0 => None,
                1 => Some(Caret::new(reader.u16()?, reader.u16()?)),
                _ => return Err(ProtocolError::InvalidValue("terminal cursor flag")),
            };
            Ok(Node::Terminal { id, rows, cursor })
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
                let lead = read_row_lead(reader)?;
                let state = decode_row_state(reader.u8()?)
                    .ok_or(ProtocolError::InvalidValue("row state"))?;
                rows.push(Row {
                    action,
                    title,
                    summary,
                    lead,
                    state,
                });
            }
            Ok(Node::Rows { id, rows })
        }
        _ => Err(ProtocolError::InvalidValue("node tag")),
    }
}

/// A row's lead is always three bytes: a tag and a sixteen-bit value.
///
/// Fixed width rather than variable, because `encoded_screen_len` has to
/// predict the size of every screen before a byte is written, and a length
/// that depends on which variant a row happens to carry is exactly the kind of
/// arithmetic that has already produced one `debug_assert` panic in this file.
fn push_row_lead(output: &mut Vec<u8>, lead: RowLead) {
    match lead {
        RowLead::Icon(glyph) => {
            output.push(0);
            push_u16(output, u16::from(encode_glyph(glyph)));
        }
        RowLead::Number(number) => {
            output.push(1);
            push_u16(output, number);
        }
    }
}

fn read_row_lead(reader: &mut Reader<'_>) -> Result<RowLead, ProtocolError> {
    let tag = reader.u8()?;
    let value = reader.u16()?;
    match tag {
        0 => {
            let glyph = u8::try_from(value)
                .ok()
                .and_then(decode_glyph)
                .ok_or(ProtocolError::InvalidValue("row glyph"))?;
            Ok(RowLead::Icon(glyph))
        }
        1 => Ok(RowLead::Number(value)),
        _ => Err(ProtocolError::InvalidValue("row lead")),
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

    const fn remaining(&self) -> usize {
        self.bytes.len() - self.offset
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
                        state: ControlState::Enabled,
                        emphasis: kobo_ui::Emphasis::Normal,
                    }],
                }],
            )),
        };
        let encoded = encode(&frame).expect("valid screen");
        assert_eq!(encoded, encode(&frame).expect("stable encoding"));
        assert_eq!(decode(&encoded), Ok(frame));
    }

    #[test]
    fn a_screen_can_ask_for_a_text_size_and_most_do_not() {
        let screen = Screen::new(1, Vec::new());
        assert!(
            screen.text_scale.is_none(),
            "inheriting the reader's own setting is the default"
        );
        for scale in [None, Some(TextScale::Large), Some(TextScale::ExtraLarge)] {
            let frame = Frame {
                request_id: 9,
                message: Message::SetScreen(screen.clone().with_text_scale(scale)),
            };
            let bytes = encode(&frame).expect("encodes");
            let back = decode(&bytes).expect("decodes");
            let Message::SetScreen(out) = back.message else {
                panic!("wrong message");
            };
            assert_eq!(out.text_scale, scale);
        }
    }

    #[test]
    fn a_request_for_first_refusal_on_back_survives_the_wire() {
        // A screen that asked to answer Back itself has to arrive that way,
        // because the runtime decides where the reader's tap goes from this
        // flag alone. Lost in transit it would silently mean the opposite.
        let screen = Screen::new(
            7,
            vec![Node::Text {
                id: NodeId(1),
                text: "Chapter one".into(),
            }],
        );
        assert!(!screen.owns_back, "not asking for it is the default");
        for owns_back in [false, true] {
            let frame = Frame {
                request_id: 12,
                message: Message::SetScreen(screen.clone().with_own_back(owns_back)),
            };
            let encoded = encode(&frame).expect("valid screen");
            assert_eq!(decode(&encoded), Ok(frame));
        }
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
    #[allow(clippy::too_many_lines, reason = "one literal per node variant")]
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
            Node::Quote {
                id: NodeId(30),
                depth: 2,
                role: kobo_ui::QuoteRole::Body,
                fold: None,
                text: "A reply".into(),
            },
            Node::Button {
                id: NodeId(3),
                action: ActionId(1),
                label: "Press".into(),
                state: ControlState::Disabled,
                emphasis: kobo_ui::Emphasis::Normal,
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
                shape: TileShape::Square,
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
                selected: Some(1),
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
            Node::Terminal {
                id: NodeId(16),
                rows: vec!["~ # uname -a".into(), "Linux kobo 4.9.77".into()],
                cursor: Some(Caret::new(1, 17)),
            },
            Node::Terminal {
                id: NodeId(17),
                rows: Vec::new(),
                cursor: None,
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
    fn a_folding_byline_survives_the_wire_with_its_count() {
        // The count is what the renderer draws beside the mark, so losing it
        // would show a bare plus with no idea how much is behind it -- and the
        // size table has to agree with the payload or every frame carrying one
        // fails the length check rather than the assertion here.
        for fold in [
            None,
            Some(kobo_ui::Fold {
                action: ActionId(4321),
                collapsed: true,
                hidden: 4095,
            }),
            Some(kobo_ui::Fold {
                action: ActionId(0),
                collapsed: false,
                hidden: 0,
            }),
        ] {
            let node = Node::Quote {
                id: NodeId(3),
                depth: 2,
                role: kobo_ui::QuoteRole::Byline,
                fold,
                text: "someone 3 hours ago".to_owned(),
            };
            assert_eq!(
                round_trip(Screen::new(1, vec![node.clone()])).nodes,
                vec![node.clone()],
                "a fold did not survive the wire: {fold:?}"
            );
        }
    }

    #[test]
    fn an_overlay_survives_the_wire() {
        let screen = Screen::new(
            1,
            vec![Node::Text {
                id: NodeId(1),
                text: "Underneath".to_owned(),
            }],
        )
        .with_overlay(kobo_ui::Overlay::modal(
            NodeId(9),
            "Delete this?",
            vec![Node::Button {
                id: NodeId(10),
                action: ActionId(6),
                label: "Delete".to_owned(),
                state: ControlState::Enabled,
                emphasis: kobo_ui::Emphasis::Primary,
            }],
        ));
        assert_eq!(round_trip(screen.clone()).overlay, screen.overlay);
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
    fn a_reply_deeper_than_the_cap_arrives_at_the_cap_rather_than_being_refused() {
        // Real discussion threads nest far past anything this panel can draw,
        // and forty levels is a deeper reply rather than a malformed frame. It
        // is clamped on the way out and again on the way in, so a peer that
        // never clamped cannot make the renderer indent off the panel.
        let screen = Screen::new(
            1,
            vec![Node::Quote {
                id: NodeId(1),
                depth: 40,
                role: kobo_ui::QuoteRole::Body,
                fold: None,
                text: "Deep in an argument".into(),
            }],
        );
        assert_eq!(
            round_trip(screen).nodes,
            vec![Node::Quote {
                id: NodeId(1),
                depth: kobo_ui::MAX_QUOTE_DEPTH,
                role: kobo_ui::QuoteRole::Body,
                fold: None,
                text: "Deep in an argument".into(),
            }]
        );
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
                Some(1),
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
            Some(0),
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

    /// A one-destination bar is refused by both halves, and the encoder
    /// matters more than the decoder.
    ///
    /// It used to encode cleanly and fail only on arrival. The runtime's
    /// reader thread died on the malformed frame, the application never heard
    /// about it, and it then waited forever on a socket nobody was reading:
    /// the panel kept showing the previous screen and every later tap did
    /// nothing at all. A Hacker News thread opening with a single "Stories"
    /// destination is exactly how it was found.
    #[test]
    fn a_nav_bar_with_one_destination_is_rejected() {
        let screen = Screen::new(1, Vec::new()).with_nav_bar(NavBar::new(
            NodeId(1),
            vec![BarAction::new(ActionId(1), "Only")],
            Some(0),
        ));
        let frame = Frame {
            request_id: 1,
            message: Message::SetScreen(screen),
        };
        assert!(matches!(
            encode(&frame),
            Err(ProtocolError::InvalidValue("nav bar destinations"))
        ));
    }

    /// The launcher and the library both meant "none of these is where you
    /// are" and both said `usize::MAX`. The byte saturated to 255 and the
    /// decoder clamped it onto the last destination, so both shipped with the
    /// rightmost entry underlined on the panel, "More apps" on a launcher
    /// showing page one, "Next" on a library showing the first page.
    #[test]
    fn no_destination_being_current_survives_the_wire() {
        let screen = Screen::new(1, Vec::new()).with_nav_bar(NavBar::new(
            NodeId(1),
            vec![
                BarAction::new(ActionId(1), "Back"),
                BarAction::new(ActionId(2), "Library"),
                BarAction::new(ActionId(3), "Next"),
            ],
            None,
        ));
        let decoded = round_trip(screen);
        assert_eq!(
            decoded.nav_bar.expect("nav bar").selected,
            None,
            "a bar of actions must not claim the reader is standing on one of them"
        );
    }

    #[test]
    fn an_out_of_range_selection_clamps_rather_than_losing_navigation() {
        let screen = Screen::new(1, Vec::new()).with_nav_bar(NavBar::new(
            NodeId(1),
            vec![
                BarAction::new(ActionId(1), "A"),
                BarAction::new(ActionId(2), "B"),
            ],
            Some(250),
        ));
        let decoded = round_trip(screen);
        assert_eq!(decoded.nav_bar.expect("nav bar").selected, Some(1));
    }

    #[test]
    fn an_answer_naming_no_option_arrives_unmarked_rather_than_refused() {
        let screen = Screen::new(
            1,
            vec![Node::Choice {
                id: NodeId(1),
                prompt: "Pick one".into(),
                options: vec![BarAction::new(ActionId(4), "First")],
                selected: Some(9),
                freeform: None,
            }],
        );
        let bytes = encode(&Frame {
            request_id: 1,
            message: Message::SetScreen(screen),
        })
        .expect("encode");
        let Message::SetScreen(screen) = decode(&bytes).expect("decode").message else {
            unreachable!("a set screen frame decodes as one")
        };
        let [Node::Choice { selected, .. }] = &screen.nodes[..] else {
            unreachable!("the screen is one choice")
        };
        assert_eq!(*selected, None);
    }

    #[test]
    fn a_choice_offering_no_answers_is_rejected() {
        let screen = Screen::new(
            1,
            vec![Node::Choice {
                id: NodeId(1),
                prompt: "Dead end".into(),
                options: Vec::new(),
                selected: None,
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
                            shape: TileShape::Square,
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

#[cfg(test)]
mod store_tests {
    use super::*;

    fn message_round_trip(message: Message) -> Message {
        let frame = Frame {
            request_id: 11,
            message,
        };
        let bytes = encode(&frame).expect("encode");
        decode(&bytes).expect("decode").message
    }

    #[test]
    fn a_terminal_chunk_larger_than_the_bound_is_refused_rather_than_sent() {
        // The ceiling has to be enforced by the sender as well as the reader,
        // or a program printing without pause builds a frame the other end
        // will only reject after it has already been allocated.
        let frame = Frame {
            request_id: 1,
            message: Message::ShellEvent(ShellEvent::Output(vec![b'x'; MAX_SHELL_CHUNK + 1])),
        };
        assert!(matches!(encode(&frame), Err(ProtocolError::FrameTooLarge)));
    }

    #[test]
    fn a_terminal_chunk_exactly_at_the_bound_is_carried() {
        let message = Message::ShellRequest(ShellRequest::Input(vec![b'x'; MAX_SHELL_CHUNK]));
        assert_eq!(message_round_trip(message.clone()), message);
    }

    #[test]
    fn every_task_message_encodes_to_exactly_the_length_it_claims() {
        // `encode` predicts the payload length before it writes anything, and
        // a wrong prediction used to be a debug assertion that only fired when
        // an application actually spawned something, which meant every task
        // message crashed the simulator and no test noticed, because none of
        // them encoded one.
        for message in [
            Message::Spawn {
                task: TaskId(7),
                work: Task::Fetch {
                    url: "https://example.invalid/book.txt".into(),
                    offset: 0,
                    max_bytes: 1024,
                },
            },
            Message::Spawn {
                task: TaskId(8),
                work: Task::ReadFile {
                    path: "/mnt/onboard/book.txt".into(),
                },
            },
            Message::Spawn {
                task: TaskId(9),
                work: Task::Sleep { seconds: 30 },
            },
            Message::Spawn {
                task: TaskId(10),
                work: Task::Post {
                    url: "https://example.invalid/v1".into(),
                    body: "{}".into(),
                    content_type: "application/json".into(),
                    credential: Some(Credential::bearer("openai")),
                    headers: Vec::new(),
                    max_bytes: 4096,
                },
            },
            Message::Spawn {
                task: TaskId(11),
                work: Task::Post {
                    url: "https://example.invalid/v1".into(),
                    body: String::new(),
                    content_type: "application/json".into(),
                    credential: None,
                    headers: Vec::new(),
                    max_bytes: 4096,
                },
            },
            Message::Cancel { task: TaskId(12) },
            Message::TaskOutcome {
                task: TaskId(13),
                outcome: TaskOutcome::Completed(b"hello".to_vec()),
            },
            Message::TaskOutcome {
                task: TaskId(14),
                outcome: TaskOutcome::Failed(TaskError::TooLarge),
            },
            Message::TaskOutcome {
                task: TaskId(15),
                outcome: TaskOutcome::Cancelled,
            },
        ] {
            let (_, predicted) =
                encoded_message_layout(&message).expect("the message is within the limits");
            let frame = Frame {
                request_id: 3,
                message: message.clone(),
            };
            let encoded = encode(&frame).expect("encode");
            assert_eq!(
                encoded.len() - HEADER_LEN,
                predicted,
                "{message:?} encodes to a different length than it predicted"
            );
            assert_eq!(message_round_trip(message.clone()), message);
        }
    }

    #[test]
    fn every_store_message_survives_the_wire() {
        for message in [
            Message::StoreRequest(StoreRequest::Save {
                key: "tasks".into(),
                value: vec![0, 1, 2, 255],
            }),
            Message::StoreRequest(StoreRequest::Load {
                key: "tasks".into(),
            }),
            Message::StoreRequest(StoreRequest::Forget {
                key: "tasks".into(),
            }),
            Message::StoreRequest(StoreRequest::List),
            Message::StoreResult(StoreResult::Saved {
                key: "tasks".into(),
            }),
            Message::StoreResult(StoreResult::Loaded {
                key: "tasks".into(),
                value: Some(b"[]".to_vec()),
            }),
            Message::StoreResult(StoreResult::Loaded {
                key: "tasks".into(),
                value: None,
            }),
            Message::StoreResult(StoreResult::Forgotten {
                key: "tasks".into(),
            }),
            Message::StoreResult(StoreResult::Keys(vec!["a".into(), "b".into()])),
            Message::StoreResult(StoreResult::Denied(StoreError::BadKey)),
            Message::Lifecycle(Lifecycle::Foreground),
            Message::Lifecycle(Lifecycle::Background),
            Message::ShellRequest(ShellRequest::Open {
                columns: 53,
                rows: 20,
            }),
            Message::ShellRequest(ShellRequest::Input(vec![0x03])),
            Message::ShellRequest(ShellRequest::Input(Vec::new())),
            Message::ShellRequest(ShellRequest::Resize {
                columns: 40,
                rows: 10,
            }),
            Message::ShellRequest(ShellRequest::Close),
            Message::ShellEvent(ShellEvent::Opened),
            Message::ShellEvent(ShellEvent::Output(b"~ # \x1b[K".to_vec())),
            Message::ShellEvent(ShellEvent::Closed { status: 0 }),
            // Negative, because a program stopped by a signal is reported as
            // one and a status that came back wrong would look like success.
            Message::ShellEvent(ShellEvent::Closed { status: -1 }),
            Message::ShellEvent(ShellEvent::Refused(ShellError::NotPermitted)),
            Message::ShellEvent(ShellEvent::Refused(ShellError::Failed)),
        ] {
            assert_eq!(message_round_trip(message.clone()), message);
        }
    }

    #[test]
    fn every_shelf_message_survives_the_wire_at_the_length_it_predicted() {
        // The length functions are maintained by hand beside the encoders, so
        // this asserts the two agree as well as that the bytes decode back.
        for message in [
            Message::StoreRequest(StoreRequest::ShelfWrite {
                name: "pg1342.epub".into(),
                offset: 0,
                bytes: vec![0, 1, 2, 255],
                last: false,
            }),
            Message::StoreRequest(StoreRequest::ShelfWrite {
                name: "pg1342.epub".into(),
                offset: 262_144,
                bytes: Vec::new(),
                last: true,
            }),
            Message::StoreRequest(StoreRequest::ShelfRead {
                name: "pg1342.epub".into(),
                offset: 4096,
                length: 65536,
            }),
            Message::StoreRequest(StoreRequest::ShelfRemove {
                name: "pg1342.epub".into(),
            }),
            Message::StoreRequest(StoreRequest::ShelfList),
            Message::StoreResult(StoreResult::ShelfWritten {
                name: "pg1342.epub".into(),
                size: 262_144,
            }),
            Message::StoreResult(StoreResult::ShelfRead {
                name: "pg1342.epub".into(),
                offset: 4096,
                bytes: b"It is a truth universally acknowledged".to_vec(),
                size: 700_000,
            }),
            Message::StoreResult(StoreResult::ShelfRemoved {
                name: "pg1342.epub".into(),
            }),
            Message::StoreResult(StoreResult::Shelf(vec![
                ("pg1342.epub".into(), 700_000),
                ("pg84.txt".into(), 442_000),
            ])),
            Message::StoreResult(StoreResult::Shelf(Vec::new())),
            Message::StoreResult(StoreResult::Denied(StoreError::NoRoom)),
            Message::StoreResult(StoreResult::Denied(StoreError::Missing)),
        ] {
            let frame = Frame {
                request_id: 1,
                message: message.clone(),
            };
            let (_, predicted) = encoded_message_layout(&frame.message).expect("a valid message");
            let encoded = encode(&frame).expect("a valid message");
            assert_eq!(
                encoded.len() - HEADER_LEN,
                predicted,
                "{message:?} encodes to a different length than it predicted"
            );
            assert_eq!(message_round_trip(message.clone()), message);
        }
    }

    #[test]
    fn a_chunk_over_the_ceiling_is_refused_by_both_ends() {
        // Refused at encode, so an application cannot build a frame the
        // runtime would only drop, and refused at decode, so a peer that
        // ignored the first rule cannot make us allocate on its say-so.
        let message = Message::StoreRequest(StoreRequest::ShelfWrite {
            name: "big".into(),
            offset: 0,
            bytes: vec![0; MAX_SHELF_CHUNK + 1],
            last: false,
        });
        assert!(matches!(
            encode(&Frame {
                request_id: 1,
                message,
            }),
            Err(ProtocolError::FrameTooLarge)
        ));
    }

    #[test]
    fn a_finished_flag_that_is_neither_yes_nor_no_is_refused() {
        let mut encoded = encode(&Frame {
            request_id: 1,
            message: Message::StoreRequest(StoreRequest::ShelfWrite {
                name: "a".into(),
                offset: 0,
                bytes: Vec::new(),
                last: true,
            }),
        })
        .expect("a valid message");
        let flag = encoded.len() - 5;
        assert_eq!(encoded[flag], 1, "the finished flag moved");
        encoded[flag] = 2;
        assert!(matches!(
            decode(&encoded),
            Err(ProtocolError::InvalidValue("shelf finished flag"))
        ));
    }

    #[test]
    fn a_never_written_key_is_distinct_from_an_empty_one() {
        // These encode to different bytes on purpose. An application that
        // cannot tell "nothing saved yet" from "saved nothing" cannot tell a
        // first run from a cleared list.
        let missing = Message::StoreResult(StoreResult::Loaded {
            key: "k".into(),
            value: None,
        });
        let empty = Message::StoreResult(StoreResult::Loaded {
            key: "k".into(),
            value: Some(Vec::new()),
        });
        assert_ne!(
            encode(&Frame {
                request_id: 0,
                message: missing.clone()
            })
            .unwrap(),
            encode(&Frame {
                request_id: 0,
                message: empty.clone()
            })
            .unwrap()
        );
        assert_eq!(message_round_trip(missing.clone()), missing);
        assert_eq!(message_round_trip(empty.clone()), empty);
    }

    #[test]
    fn an_oversized_value_is_refused_before_it_is_encoded() {
        let frame = Frame {
            request_id: 0,
            message: Message::StoreRequest(StoreRequest::Save {
                key: "big".into(),
                value: vec![0; MAX_STORE_VALUE + 1],
            }),
        };
        assert!(matches!(encode(&frame), Err(ProtocolError::FrameTooLarge)));
    }

    #[test]
    fn keys_that_could_name_somewhere_else_are_refused() {
        for bad in [
            "",
            "..",
            "../../etc/passwd",
            ".hidden",
            "has/slash",
            "Upper",
            "has space",
            "has\\backslash",
            "nul\0byte",
        ] {
            assert!(!is_valid_key(bad), "{bad:?} was accepted as a key");
        }
        for good in ["tasks", "book.position", "a-b_c", "v2.state"] {
            assert!(is_valid_key(good), "{good:?} was refused as a key");
        }
        assert!(!is_valid_key(&"a".repeat(MAX_STORE_KEY_LEN + 1)));
        assert!(is_valid_key(&"a".repeat(MAX_STORE_KEY_LEN)));
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

#[cfg(test)]
mod picture_tests {
    use super::*;

    #[test]
    fn a_picture_on_a_shelf_survives_the_wire() {
        let screen = Screen::new(
            9,
            vec![
                Node::TileGrid {
                    id: NodeId(1),
                    tiles: vec![
                        Tile::new(ActionId(11), "Waiting", Glyph::Book),
                        Tile::new(ActionId(12), "Moby Dick", Glyph::Book)
                            .with_picture(TilePicture::new(PictureHandle(7), 190, 300)),
                    ],
                    shape: TileShape::Portrait,
                },
                Node::Picture {
                    id: NodeId(2),
                    handle: PictureHandle(7),
                    source: (190, 300),
                    max_height_tenths_mm: 600,
                },
            ],
        );
        let frame = Frame {
            request_id: 3,
            message: Message::SetScreen(screen),
        };
        let bytes = encode(&frame).expect("encode");
        assert_eq!(decode(&bytes).expect("decode"), frame);
    }

    #[test]
    fn handing_over_a_picture_survives_the_wire() {
        let frame = Frame {
            request_id: 1,
            message: Message::PutPicture {
                handle: PictureHandle(4),
                width: 3,
                height: 2,
                grey: vec![0, 32, 64, 96, 128, 160],
            },
        };
        let bytes = encode(&frame).expect("encode");
        assert_eq!(decode(&bytes).expect("decode"), frame);
    }

    #[test]
    fn a_picture_whose_size_disagrees_with_its_bytes_is_refused() {
        // The decoder allocates on the strength of the declared size, so the
        // two have to be checked against each other before anything is read.
        let refused = encode(&Frame {
            request_id: 1,
            message: Message::PutPicture {
                handle: PictureHandle(4),
                width: 100,
                height: 100,
                grey: vec![0; 99],
            },
        });
        assert!(matches!(refused, Err(ProtocolError::InvalidValue(_))));
    }

    #[test]
    fn a_picture_larger_than_a_frame_is_refused() {
        let refused = encode(&Frame {
            request_id: 1,
            message: Message::PutPicture {
                handle: PictureHandle(4),
                width: u32::try_from(MAX_INLINE_PICTURE_BYTES + 1).expect("fits"),
                height: 1,
                grey: vec![0; MAX_INLINE_PICTURE_BYTES + 1],
            },
        });
        assert!(matches!(refused, Err(ProtocolError::FrameTooLarge)));
    }

    #[test]
    fn every_phase_of_a_large_picture_upload_survives_the_wire() {
        let messages = [
            Message::BeginPicture {
                handle: PictureHandle(4),
                width: 1072,
                height: 1448,
            },
            Message::PictureChunk {
                handle: PictureHandle(4),
                offset: 0,
                grey: vec![17; 4096],
            },
            Message::CommitPicture {
                handle: PictureHandle(4),
            },
        ];
        for message in messages {
            let frame = Frame {
                request_id: 1,
                message,
            };
            let bytes = encode(&frame).expect("encode");
            assert_eq!(decode(&bytes).expect("decode"), frame);
        }
    }

    #[test]
    fn a_picture_chunk_is_independently_bounded() {
        let refused = encode(&Frame {
            request_id: 1,
            message: Message::PictureChunk {
                handle: PictureHandle(4),
                offset: 0,
                grey: vec![0; MAX_PICTURE_CHUNK_BYTES + 1],
            },
        });
        assert!(matches!(refused, Err(ProtocolError::FrameTooLarge)));
    }

    #[test]
    fn releasing_a_picture_survives_the_wire() {
        let frame = Frame {
            request_id: 8,
            message: Message::DropPicture {
                handle: PictureHandle(4),
            },
        };
        let bytes = encode(&frame).expect("encode");
        assert_eq!(decode(&bytes).expect("decode"), frame);
    }

    #[test]
    fn an_unknown_tile_shape_is_refused_rather_than_guessed() {
        let frame = Frame {
            request_id: 1,
            message: Message::SetScreen(Screen::new(
                1,
                vec![Node::TileGrid {
                    id: NodeId(1),
                    tiles: Vec::new(),
                    shape: TileShape::Square,
                }],
            )),
        };
        let mut bytes = encode(&frame).expect("encode");
        // An empty grid ends with its shape and then its tile count, and a
        // screen ends with whether it carries an overlay, so the shape is the
        // third byte from the end.
        let shape = bytes.len() - 3;
        assert_eq!(bytes[shape], 0, "square");
        bytes[shape] = 9;
        assert!(matches!(
            decode(&bytes),
            Err(ProtocolError::InvalidValue("tile shape"))
        ));
    }
}

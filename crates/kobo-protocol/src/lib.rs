#![forbid(unsafe_code)]

//! Versioned, bounded wire format used between Kobo applications and hosts.

use std::fmt;
use std::io::{self, Read, Write};
use std::time::Duration;

use kobo_ui::{
    ActionId, BannerLevel, BarAction, BarStyle, BottomAction, Caret, Cell, ControlState,
    FontHandle, Freeform, Glyph, NavBar, Node, NodeId, Orientation, PageTurns, Percent,
    PictureHandle, Row, RowLead, RowState, Screen, Space, TextScale, Tile, TilePicture, TileShape,
    TileState, TopBar, TransferFailure, MAX_BAR_ACTIONS, MAX_TERMINAL_COLUMNS, MAX_TERMINAL_ROWS,
    MIN_NAV_DESTINATIONS,
};
use std::cmp::min;

pub const MAGIC: [u8; 4] = *b"KOBO";
/// The wire version, refused rather than reinterpreted on a mismatch.
///
/// Went to 3 when a grid cell gained an optional glyph. That is a change to
/// the payload of an existing tag rather than a new tag, so an old runtime
/// would have read the flag byte as the next cell's action. The version byte
/// exists precisely so it declines the frame instead.
///
/// Went to 4 when the Bluetooth result gained `restart_on_exit`, for the same
/// reason: a trailing byte on an existing tag, which an old application would
/// have left in the buffer.
///
/// Went to 5 when a row gained an optional overflow action. Same shape again:
/// a flag byte inside the repeated part of tag 14, which an old runtime would
/// have read as the next row's action.
///
/// Went to 6 for the runtime-owned app catalog and app transaction requests,
/// whose new result variant carries a bounded list of application metadata.
///
/// Went to 7 when `Fetch` gained `headers`, the same trailing count-and-pairs
/// shape `Post` already carried. An old runtime reading a new frame would have
/// read the header count as the first byte of the next message.
///
/// Went to 8 when `Fetch` gained a credential, and when a refusal to
/// authenticate became an answer of its own rather than being reported as a
/// missing page. Both are tags an older runtime has no reading for, and the
/// credential sits ahead of the header count, so a frame it did not expect
/// would have been misread from that point on rather than refused. Version 9
/// adds bounded rich EPUB text and runtime-held publisher-font handles.
/// Version 10 adds exact text-hold coordinates and typed offline dictionary
/// requests/results. Version 11 adds the identity request and its result,
/// both tags an older side has no reading for. Update-channel requests were
/// later added on new tags without changing the shapes of existing frames. Idle-sleep requests use later unused tags for the same reason.
pub const VERSION: u8 = 11;
pub const HEADER_LEN: usize = 14;
/// The largest single frame either side will read.
///
/// Was one megabyte until narrated audio began arriving as task replies: a
/// minute of 128 kbps MP3 is about a megabyte on its own, and a frame budget
/// the same size as the payload it carries leaves no room for the envelope.
/// Eight megabytes bounds a runaway peer just as well and costs nothing when
/// frames stay small, which every frame except an audio reply does.
pub const MAX_FRAME_LEN: usize = 8 * 1_048_576;
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
/// Largest embedded outline font one application may hand to the runtime.
pub const MAX_FONT_BYTES: usize = 2 * 1024 * 1024;
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
/// Half a megabyte, until it met a minute of narrated speech: at 128 kbps
/// that is a megabyte of MP3, delivered as one task reply, and the reply was
/// refused as too large on the very device it was made for. Four megabytes
/// carries the longest part a narration is split into with room to spare,
/// while still refusing a provider that will not stop talking. Anything
/// genuinely bigger than this is a file download, which belongs on disk
/// rather than in memory on a device with this much of it.
pub const MAX_TASK_BYTES_U32: u32 = 4 * 1024 * 1024;

/// The same limit as [`MAX_TASK_BYTES_U32`], in the width Rust indexes with.
/// Declaring the wire width first means the conversion only ever widens.
pub const MAX_TASK_BYTES: usize = MAX_TASK_BYTES_U32 as usize;

/// The most an application may send in one request body.
///
/// A body is not a label. It carries a document, a research digest, a batch of
/// items, and it outgrows [`MAX_STRING_LEN`] the first time an application asks
/// a model to work on something it fetched. A task may already hand back
/// [`MAX_TASK_BYTES`], so sending is now bounded the same way receiving is
/// rather than at a fortieth of it.
pub const MAX_POST_BODY_LEN: usize = MAX_TASK_BYTES;

/// The most radios a device scan can report in one answer.
pub const MAX_RADIO_DEVICES: usize = 32;

/// The most applications a catalog may expose in one bounded reply.
pub const MAX_APP_CATALOG_ENTRIES: usize = 128;
/// Stable application identities are deliberately short enough to become
/// filenames without truncation or platform-specific path behavior.
pub const MAX_APP_ID_LEN: usize = 32;
/// Application versions share the signed manifest's bounded version field.
pub const MAX_APP_VERSION_LEN: usize = 64;
/// Capability declarations are drawn as a short list and are also bounded by
/// the complete capability vocabulary.
pub const MAX_APP_CAPABILITIES: usize = 16;
const MAX_APP_LINK_EXPIRES_IN: u32 = 10 * 60;
const MAX_APP_LINK_BROWSERS: u8 = 8;

/// Human-readable radio identifiers are deliberately shorter than an ordinary
/// protocol string. They are drawn on one row and are also accepted from local
/// system tools, so bounding them prevents a broken backend from manufacturing
/// a very large frame.
pub const MAX_RADIO_NAME: usize = 96;

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
pub const MAX_LOOKUP_WORD_BYTES: usize = 128;
pub const MAX_DICTIONARY_ENTRIES: usize = 8;
pub const MAX_DICTIONARY_DEFINITION_BYTES: usize = 4_096;

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
    /// `Authorization: Basic <base64 of the stored value>`.
    ///
    /// The stored value is the whole `user:password` pair, encoded by the
    /// runtime rather than the application, so an application asking for a
    /// gated feed never holds either half. Standard Ebooks wants a donor's
    /// email address as the user and nothing as the password, which is a
    /// perfectly ordinary Basic credential and an unusual-looking secret.
    Basic,
}

/// A credential an application may use and never see.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Credential {
    /// The name of a secret the runtime holds.
    pub secret: String,
    pub header: SecretHeader,
}

/// The network operation for which the runtime is about to resolve a named
/// credential.
///
/// Destination policy needs the method as well as the URL: a read-only client
/// may legitimately GET an item while a POST to that route changes data.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CredentialUse {
    Fetch,
    Post,
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

    /// The credential a site asks for with `WWW-Authenticate: Basic`.
    #[must_use]
    pub fn basic(secret: impl Into<String>) -> Self {
        Self {
            secret: secret.into(),
            header: SecretHeader::Basic,
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
            SecretHeader::Bearer | SecretHeader::Basic => "Authorization",
            SecretHeader::Named(name) => name,
        }
    }

    #[must_use]
    pub fn is_well_formed(&self) -> bool {
        if self.secret.is_empty() || self.secret.len() > MAX_HEADER_NAME {
            return false;
        }
        match &self.header {
            SecretHeader::Bearer | SecretHeader::Basic => true,
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
        /// The credential to attach, or `None`.
        ///
        /// Named, never held: a catalogue behind a subscription is reached by
        /// asking the runtime for "the Standard Ebooks credential", and the
        /// runtime is what knows the address and the password. A feed is a
        /// GET, so a fetch that could not carry one meant a gated catalogue
        /// could be recognised and never opened.
        credential: Option<Credential>,
        /// Headers the request needs that are not secret.
        ///
        /// An OPDS catalogue is JSON to one server and Atom to another at the
        /// same path, and the only way to ask for one over the other is
        /// `Accept`. Bounded and validated exactly like `Post`'s headers, and
        /// for the same reason the runtime, not the application, turns
        /// `offset` into the `Range` header: an application that could set
        /// `Range` itself could read past the piece the byte ceiling allowed.
        headers: Vec<Header>,
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

impl Task {
    /// Whether this task fits on the wire.
    ///
    /// Asked before a task is handed over rather than discovered by the
    /// encoder, so an application that assembles an over-large request can be
    /// told about it on screen instead of ending mid-frame.
    #[must_use]
    pub fn is_sendable(&self) -> bool {
        match self {
            Self::Fetch {
                url,
                credential,
                headers,
                ..
            } => {
                url.len() <= MAX_URL_LEN
                    && headers.len() <= MAX_HEADERS
                    && headers.iter().all(Header::is_well_formed)
                    && credential.as_ref().is_none_or(Credential::is_well_formed)
            }
            Self::Post {
                url,
                body,
                content_type,
                credential,
                headers,
                ..
            } => {
                url.len() <= MAX_URL_LEN
                    && body.len() <= MAX_POST_BODY_LEN
                    && content_type.len() <= MAX_STRING_LEN
                    && headers.len() <= MAX_HEADERS
                    && headers.iter().all(Header::is_well_formed)
                    && credential.as_ref().is_none_or(Credential::is_well_formed)
            }
            Self::ReadFile { path } => path.len() <= MAX_STRING_LEN,
            Self::Sleep { .. } => true,
        }
    }
}

/// How a task ended.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TaskOutcome {
    Completed(Vec<u8>),
    Failed(TaskError),
    /// The application asked for this through `Context::cancel`.
    Cancelled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskError {
    /// The application does not hold the capability the task requires.
    Denied,
    /// The task named a credential this reader has no key for.
    ///
    /// Separate from [`TaskError::Denied`] because the application is not at
    /// fault and nothing it can do will fix it. It asked for the right key by
    /// the right name and was allowed to; the key is simply not on the device
    /// yet. The answer is to install one, which is a thing the person holding
    /// the reader does once, not a thing the code retries.
    ///
    /// Kept apart from the capability refusal so that the two do not share a
    /// sentence. "The application does not hold this permission" is actively
    /// misleading when the truth is that a file is missing.
    NoCredential,
    /// This reader has no network at all.
    ///
    /// Separate from [`TaskError::Unreachable`] because the two need opposite
    /// things from the person holding the reader. This one is answered by
    /// joining Wi-Fi, and no amount of retrying will help until they do.
    /// Deciding between them is the daemon's job: it is the only layer that
    /// runs outside the sandbox and can see whether the device has a route.
    Offline,
    /// The reader has a network, but this host did not answer usefully.
    ///
    /// A refused connection, a name that does not resolve while other names
    /// do, a handshake that fails, or a reply that is not HTTP. Worth
    /// retrying, and worth reporting as the host's problem rather than the
    /// reader's.
    Unreachable,
    /// The response exceeded the ceiling the task itself declared.
    TooLarge,
    TimedOut,
    NotFound,
    /// The host will not answer without a credential, or not with the one it
    /// was given.
    ///
    /// Kept apart from [`TaskError::NotFound`], which is where every refusal
    /// used to land, because the two ask opposite things of the person holding
    /// the reader. A book that is not there is the end of it; a feed that
    /// wants a subscription is something they can go and get. Standard Ebooks
    /// answers its catalogue this way for anyone who has not donated, and
    /// "not found" was both wrong and unactionable.
    ///
    /// Kept apart from [`TaskError::NoCredential`] as well: that one is the
    /// device having no key, this one is the host refusing the request.
    Unauthorized,
}

impl TaskError {
    /// Whether trying the same thing again could reasonably succeed.
    ///
    /// The radio on this reader powers down when idle and wakes on demand, so
    /// the first request after a quiet spell regularly fails while it comes
    /// back. That is the case this exists for.
    ///
    /// [`TaskError::Denied`] is not here because a permission does not appear
    /// on the second ask, and neither is [`TaskError::TooLarge`], because the
    /// response will be the same size next time. [`TaskError::NotFound`] is a
    /// real answer from a host that is working. [`TaskError::NoCredential`] is
    /// not here for the same reason as `Denied`: a key does not install itself
    /// between two attempts three seconds apart.
    #[must_use]
    pub const fn worth_retrying(self) -> bool {
        matches!(self, Self::Offline | Self::Unreachable | Self::TimedOut)
    }
}

impl fmt::Display for TaskError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Denied => "the application does not hold this permission",
            Self::NoCredential => "this reader has no key for that service",
            Self::Offline => "this reader is not on a network",
            Self::Unreachable => "the host did not answer",
            Self::TooLarge => "the response was larger than the limit the task declared",
            Self::TimedOut => "the task ran out of time",
            Self::NotFound => "not found",
            Self::Unauthorized => "the host will not answer without a credential",
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
    /// Changes the logical direction for this application session. Omission
    /// keeps the portrait-compatible default.
    SetOrientation(Orientation),
    Action {
        action: ActionId,
    },
    /// A held word in application-defined logical text coordinates.
    TextHold {
        action: ActionId,
        context: u64,
        start: u32,
        end: u32,
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
    /// The hall sensor changed: a magnet arrived, or left.
    ///
    /// Unsolicited, and therefore not a [`DeviceResult`]. Device answers are
    /// matched to the request that produced them and one with nothing
    /// outstanding is dropped, which is right for answers and wrong for
    /// something the world did on its own. This is the same shape as
    /// [`Message::Lifecycle`]: the runtime saying what changed, unprompted.
    ///
    /// Only changes are sent. The sensor bounces while a magnet is moved past
    /// it slowly, and an application acting on every edge would act several
    /// times for one deliberate gesture.
    CoverChanged {
        magnet_present: bool,
    },
    /// A physical page-turn key was pressed, already resolved to intent.
    ///
    /// Unsolicited, the same shape as [`Message::CoverChanged`]. The runtime
    /// owns the raw keycodes and how the reader is held; an application only
    /// ever hears which way the reader wants to go. Sent on the press, not
    /// the release, because a page turn should not wait for a finger to lift.
    ///
    /// An application that does nothing with this does nothing — there is no
    /// runtime fallback, deliberately.
    PageTurn {
        forward: bool,
    },
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
    /// Hands one bounded TrueType/OpenType publisher face to the runtime.
    PutFont {
        handle: FontHandle,
        name: String,
        bytes: Vec<u8>,
    },
    /// Releases a publisher face and its glyph cache.
    DropFont {
        handle: FontHandle,
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

/// How long Cobalt waits with nothing happening before it sleeps in session.
///
/// Chosen in Settings. Five minutes is the default. [`Self::Never`] leaves the
/// panel on until the power button, the cover, or the session ceiling.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum IdleSleep {
    OneMinute,
    #[default]
    FiveMinutes,
    TenMinutes,
    ThirtyMinutes,
    Never,
}

impl IdleSleep {
    /// The choices Settings offers, in the order they appear.
    pub const ALL: [Self; 5] = [
        Self::OneMinute,
        Self::FiveMinutes,
        Self::TenMinutes,
        Self::ThirtyMinutes,
        Self::Never,
    ];

    /// Whole minutes, or `None` when the panel should not sleep from idleness.
    #[must_use]
    pub const fn minutes(self) -> Option<u8> {
        match self {
            Self::OneMinute => Some(1),
            Self::FiveMinutes => Some(5),
            Self::TenMinutes => Some(10),
            Self::ThirtyMinutes => Some(30),
            Self::Never => None,
        }
    }

    /// How long the session loop waits, or `None` for [`Self::Never`].
    #[must_use]
    pub fn duration(self) -> Option<Duration> {
        self.minutes()
            .map(|minutes| Duration::from_secs(u64::from(minutes) * 60))
    }

    /// The phrase Settings shows for this choice.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::OneMinute => "1 minute",
            Self::FiveMinutes => "5 minutes",
            Self::TenMinutes => "10 minutes",
            Self::ThirtyMinutes => "30 minutes",
            Self::Never => "Never",
        }
    }

    /// The token written to the on-disk preference.
    #[must_use]
    pub const fn as_stored(self) -> &'static str {
        match self {
            Self::OneMinute => "1",
            Self::FiveMinutes => "5",
            Self::TenMinutes => "10",
            Self::ThirtyMinutes => "30",
            Self::Never => "never",
        }
    }

    /// Reads a stored token. Anything unknown becomes the five-minute default.
    #[must_use]
    pub fn from_stored(text: &str) -> Self {
        match text.trim() {
            "1" => Self::OneMinute,
            "5" => Self::FiveMinutes,
            "10" => Self::TenMinutes,
            "30" => Self::ThirtyMinutes,
            "never" => Self::Never,
            _ => Self::default(),
        }
    }

    const fn to_wire(self) -> u8 {
        match self {
            Self::Never => 0,
            Self::OneMinute => 1,
            Self::FiveMinutes => 2,
            Self::TenMinutes => 3,
            Self::ThirtyMinutes => 4,
        }
    }

    const fn from_wire(tag: u8) -> Option<Self> {
        match tag {
            0 => Some(Self::Never),
            1 => Some(Self::OneMinute),
            2 => Some(Self::FiveMinutes),
            3 => Some(Self::TenMinutes),
            4 => Some(Self::ThirtyMinutes),
            _ => None,
        }
    }
}

/// Every hardware operation an application can ask for.
///
/// Applications never open a device node. They describe an intent, the runtime
/// decides whether to honour it, and the answer is always explicit.
/// Which published update stream the runtime follows.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum UpdateChannel {
    /// Normal releases. This remains the default for existing readers.
    #[default]
    Stable,
    /// Prerelease platform and application catalog releases.
    Beta,
}

impl UpdateChannel {
    const fn wire(self) -> u8 {
        match self {
            Self::Stable => 0,
            Self::Beta => 1,
        }
    }

    fn from_wire(value: u8) -> Result<Self, ProtocolError> {
        match value {
            0 => Ok(Self::Stable),
            1 => Ok(Self::Beta),
            _ => Err(ProtocolError::InvalidValue("update channel")),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeviceRequest {
    /// Report battery percentage and whether the device is charging.
    ReadBattery,
    /// Everything the gauge publishes, for a screen that shows it.
    ///
    /// Separate from [`DeviceRequest::ReadBattery`], which every session makes
    /// for the mark in the status band and which policy consults before
    /// granting expensive work. That one has to stay two numbers the kernel
    /// answers instantly. This one reads ten files and is asked only when
    /// somebody has opened a battery screen and is looking at it.
    ReadBatteryDetail,
    /// Asks where the magnet is now.
    ///
    /// An application is told about changes without asking, but a change is
    /// only useful to something that knows the state it changed from. This is
    /// how a screen that has just opened finds that out.
    ReadCover,
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
    /// Report whether the Bluetooth controller is available and powered.
    ReadBluetooth,
    /// Power the Bluetooth controller on or off.
    SetBluetooth { enabled: bool },
    /// Discover nearby and remembered Bluetooth devices.
    ScanBluetooth,
    /// Pair with a Bluetooth device by its canonical address.
    PairBluetooth { address: String },
    /// Connect a paired Bluetooth device.
    ConnectBluetooth { address: String },
    /// Disconnect a Bluetooth device without forgetting it.
    DisconnectBluetooth { address: String },
    /// Remove a remembered Bluetooth pairing.
    ForgetBluetooth { address: String },
    /// Report Wi-Fi power and association state.
    ReadWifi,
    /// Power the Wi-Fi interface on or off.
    SetWifi { enabled: bool },
    /// Discover nearby Wi-Fi networks.
    ScanWifi,
    /// Join a Wi-Fi network. An empty password means an open network.
    JoinWifi { ssid: String, password: String },
    /// Leave the current Wi-Fi network without powering the radio off.
    DisconnectWifi,
    /// Report the active audio source and transport state.
    ReadAudio,
    /// Prepare a shelf file or HTTPS stream for playback.
    LoadAudio { source: AudioSource },
    /// Start or resume the prepared source.
    PlayAudio,
    /// Pause without discarding the prepared source or position.
    PauseAudio,
    /// Seek to an absolute position in the prepared source.
    SeekAudio { position_ms: u32 },
    /// Stop playback and return to the beginning of the prepared source.
    StopAudio,
    /// Set software playback volume as a percentage.
    SetAudioVolume { percent: u8 },
    /// Replace the installed Cobalt with a downloaded release archive.
    ///
    /// The runtime fetches `url`, refuses the bytes unless their SHA-256
    /// digest is exactly `sha256`, unpacks them beside the install and swaps
    /// the folders, keeping the old install for one step of rollback. The
    /// root filesystem is never written, so the worst a bad archive can do
    /// is fail to start; the reader itself cannot be harmed.
    Update { url: String, sha256: String },
    /// Enumerate app-store applications currently installed on this reader.
    ListInstalledApps,
    /// Read the last verified app catalog without using the network.
    ReadAppCatalog,
    /// Fetch and verify the current app catalog from Cobalt's fixed source.
    RefreshAppCatalog,
    /// Install or update one catalog application by stable identity.
    InstallApp { id: String },
    /// Remove one app-store application by stable identity.
    UninstallApp { id: String },
    /// Look up one selected word using only runtime-installed dictionaries.
    LookupWord {
        word: String,
        language: Option<String>,
    },
    /// Report the current browser-link state.
    ReadAppLink,
    /// Begin a new browser-link attempt.
    BeginAppLink,
    /// Poll for pairing and remote installation progress.
    PollAppLink,
    /// Disconnect every paired browser.
    DisconnectAppLink,
    /// Report which automatic updates the runtime performs on its own.
    ReadAutoUpdate,
    /// Choose which automatic updates the runtime performs on its own. Both
    /// switches travel together so two writes cannot interleave and leave a
    /// mixture neither screen chose.
    SetAutoUpdate { cobalt: bool, apps: bool },
    /// Report what this runtime is and what it is running on.
    ///
    /// Asked when somebody opens a screen that shows it, in the same spirit
    /// as [`DeviceRequest::ReadBatteryDetail`]: the session already proved
    /// this identity at startup, so answering is a read, not a probe.
    ReadIdentity,
    /// Report which published update stream the runtime follows.
    ReadUpdateChannel,
    /// Select the published update stream used for platform and app updates.
    SetUpdateChannel { channel: UpdateChannel },
    /// Report how long the session waits idle before sleeping.
    ReadIdleSleep,
    /// Set how long the session waits idle before sleeping.
    SetIdleSleep(IdleSleep),
}

/// Current state of the runtime-owned App Store browser link.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AppLinkState {
    Unpaired,
    Pairing {
        code: String,
        url: String,
        expires_in: u32,
    },
    Paired {
        browsers: u8,
    },
}

/// Result of processing one remotely requested application installation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RemoteInstallOutcome {
    None,
    Installed {
        id: String,
    },
    Updated {
        id: String,
    },
    AlreadyInstalled {
        id: String,
    },
    Included {
        id: String,
    },
    Unavailable {
        id: String,
    },
    RequiresCobalt {
        id: String,
        minimum_cobalt_version: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DictionaryEntry {
    pub dictionary: String,
    pub language: String,
    pub headword: String,
    pub definition: String,
}

/// Everything the gauge publishes that is worth putting in front of a reader.
///
/// Separate from the two fields in [`DeviceResult::Battery`] because the two
/// have different jobs. That one is read on every session for the mark in the
/// status band and for the policy rule about expensive work on a low battery,
/// so it stays what the kernel can answer instantly. This is read only when
/// somebody has opened a battery screen and asked.
///
/// Every field is optional. These readings are a vendor driver's choice, not a
/// kernel guarantee: the Clara BW publishes all of them and another reader may
/// publish half. A field that is missing is left out of the screen rather than
/// shown as zero, for the same reason an unreadable gauge draws no mark.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BatteryDetail {
    /// Charge remaining, 0 to 100.
    pub percent: Option<u8>,
    /// `Charging`, `Discharging`, `Full`, `Not charging`.
    pub status: Option<String>,
    /// The driver's own verdict: `Good`, `Overheat`, `Cold`, and so on.
    pub health: Option<String>,
    /// `Li-ion` and friends.
    pub technology: Option<String>,
    /// Tenths of a degree Celsius, as the kernel reports it.
    pub decidegrees: Option<i32>,
    /// Microvolts across the pack.
    pub microvolts: Option<i32>,
    /// Microamps. Negative while discharging, positive while charging.
    pub microamps: Option<i32>,
    /// Microamp-hours currently held.
    pub charge_now: Option<i32>,
    /// Microamp-hours the pack holds today when full.
    pub charge_full: Option<i32>,
    /// Microamp-hours it held when new. Together with `charge_full` this is
    /// the only honest measure of how worn the battery is.
    pub charge_full_design: Option<i32>,
}

impl BatteryDetail {
    /// How much of the original capacity the pack still holds, 0 to 100.
    ///
    /// `None` when either figure is missing or the design capacity is zero,
    /// rather than a percentage computed from a divisor nobody supplied.
    #[must_use]
    pub fn health_percent(&self) -> Option<u8> {
        let (full, design) = (self.charge_full?, self.charge_full_design?);
        if design <= 0 || full <= 0 {
            return None;
        }
        let percent = i64::from(full) * 100 / i64::from(design);
        Some(u8::try_from(percent.clamp(0, 100)).unwrap_or(0))
    }

    /// Whole minutes until empty at the current draw, or until full.
    ///
    /// `None` when the pack is idle or the current is not published, because
    /// dividing by a current of zero gives an estimate of forever and showing
    /// that is worse than showing nothing.
    #[must_use]
    pub fn minutes_remaining(&self) -> Option<u32> {
        let current = self.microamps?;
        if current == 0 {
            return None;
        }
        let held = self.charge_now?;
        let charge = if current > 0 {
            i64::from(self.charge_full?.saturating_sub(held))
        } else {
            i64::from(held)
        };
        if charge <= 0 {
            return None;
        }
        let minutes = charge * 60 / i64::from(current.abs());
        u32::try_from(minutes).ok()
    }
}

/// The longest text any one identity field may carry.
///
/// Firmware versions, kernel releases, profile names and model names are all
/// short. A field longer than this is not a longer truth, it is a corrupt or
/// hostile frame, and it is refused on both sides.
pub const MAX_IDENTITY_FIELD_LEN: usize = 64;

/// What this runtime is and what it is running on.
///
/// Everything here was already proved once: the session refuses to start on
/// hardware whose identity does not match a profile exactly, so these are the
/// matched values rather than fresh guesses. A screen showing them, on the
/// panel of the reader they describe, is evidence that this build actually
/// ran there — which is what a reviewer wants a photograph of.
///
/// Empty strings mean the value has no source on this host, as firmware and
/// kernel do in the simulator. The serial number is deliberately absent: it
/// identifies the owner's unit, and a screen made to be photographed and
/// shared must not carry it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DeviceIdentity {
    /// The matched profile's stable name, `CLARA_BW_391` for example.
    pub profile_id: String,
    /// The human name of the reader, `Kobo Clara BW` for example.
    pub model: String,
    /// The vendor's device code, `391` for example. Zero in the simulator.
    pub device_code: u16,
    /// The firmware version the reader is running.
    pub firmware: String,
    /// The kernel release the reader is running.
    pub kernel: String,
    /// The version this runtime was compiled as.
    pub runtime_version: String,
    /// Panel width in pixels, from the matched profile.
    pub panel_width: u32,
    /// Panel height in pixels, from the matched profile.
    pub panel_height: u32,
}

fn validate_identity(identity: &DeviceIdentity) -> Result<(), ProtocolError> {
    for text in [
        &identity.profile_id,
        &identity.model,
        &identity.firmware,
        &identity.kernel,
        &identity.runtime_version,
    ] {
        if text.len() > MAX_IDENTITY_FIELD_LEN {
            return Err(ProtocolError::InvalidValue("identity field"));
        }
    }
    Ok(())
}

/// The runtime's answer to a device request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeviceResult {
    /// The request was carried out and needs no value.
    Done,
    /// A time-bounded request was granted, possibly for less than was asked.
    Granted { seconds: u32 },
    /// Battery state.
    Battery { percent: u8, charging: bool },
    /// Everything the gauge publishes. See [`BatteryDetail`].
    BatteryDetail(BatteryDetail),
    /// Where the magnet is, and whether this reader can tell.
    ///
    /// `available` is false on a reader with no hall sensor, which is a
    /// different answer from a sensor that reports no magnet.
    Cover {
        available: bool,
        magnet_present: bool,
    },
    /// Front light state.
    Frontlight { percent: u8 },
    /// Bluetooth controller state and the bounded set currently known.
    Bluetooth {
        available: bool,
        enabled: bool,
        devices: Vec<BluetoothDevice>,
        /// Whether leaving this session will reboot the reader rather than
        /// hand the panel back to it.
        ///
        /// The Clara BW's `MediaTek` radio driver cannot be initialised twice in
        /// one boot, so once Cobalt has touched that stack the only proven
        /// hand-back is a clean reboot. That is not a failure, but it is
        /// indistinguishable from one unless the application says so first,
        /// which is why the runtime reports it instead of keeping it private.
        restart_on_exit: bool,
    },
    /// Wi-Fi controller state and the bounded set currently known.
    Wifi {
        available: bool,
        enabled: bool,
        connected_ssid: Option<String>,
        networks: Vec<WifiNetwork>,
    },
    /// Audio backend and transport state.
    Audio {
        available: bool,
        state: AudioPlaybackState,
        position_ms: u32,
        duration_ms: u32,
        volume: u8,
    },
    /// A bounded app-store or installed-app listing.
    Apps { entries: Vec<AppInfo> },
    /// Bounded offline definitions in installed dictionary order. An empty
    /// list is an explicit no-result answer, not a transport failure.
    Dictionary {
        word: String,
        entries: Vec<DictionaryEntry>,
    },
    /// Current state of the App Store browser link.
    AppLink(AppLinkState),
    /// Outcome of the latest remote installation request.
    RemoteInstall(RemoteInstallOutcome),
    /// Which automatic updates the runtime performs on its own.
    AutoUpdate { cobalt: bool, apps: bool },
    /// What this runtime is and what it is running on. See [`DeviceIdentity`].
    Identity(DeviceIdentity),
    /// How long the session waits idle before sleeping.
    IdleSleep(IdleSleep),
    /// The backend exists, but the requested operation failed.
    Failed(DeviceError),
    /// The request was refused, with the exact reason.
    Denied(DenyReason),
    /// Which published update stream the runtime follows.
    UpdateChannel(UpdateChannel),
}

/// Application metadata safe to show to an unprivileged launcher or Store UI.
///
/// Download URLs, signatures and filesystem locations deliberately remain
/// runtime-owned. An application chooses an identity; it never chooses bytes
/// or a destination.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppInfo {
    pub id: String,
    pub title: String,
    pub label: String,
    pub summary: String,
    pub version: String,
    /// Oldest Cobalt platform release that can run this app package.
    pub minimum_cobalt_version: String,
    pub glyph: Glyph,
    pub capabilities: Vec<String>,
    /// The installed version when present. A different `version` means an
    /// update is available.
    pub installed_version: Option<String>,
}

impl AppInfo {
    #[must_use]
    pub const fn is_installed(&self) -> bool {
        self.installed_version.is_some()
    }

    #[must_use]
    pub fn has_update(&self) -> bool {
        self.installed_version
            .as_ref()
            .is_some_and(|installed| numeric_version_is_newer(&self.version, installed))
    }

    #[must_use]
    pub fn is_compatible_with(&self, cobalt_version: &str) -> bool {
        numeric_version_at_least(cobalt_version, &self.minimum_cobalt_version)
    }
}

fn numeric_version_at_least(current: &str, minimum: &str) -> bool {
    let (Some(mut current), Some(mut minimum)) =
        (numeric_version(current), numeric_version(minimum))
    else {
        return false;
    };
    let width = current.len().max(minimum.len());
    current.resize(width, 0);
    minimum.resize(width, 0);
    current >= minimum
}

fn numeric_version_is_newer(candidate: &str, installed: &str) -> bool {
    let (Some(mut candidate), Some(mut installed)) =
        (numeric_version(candidate), numeric_version(installed))
    else {
        return false;
    };
    let width = candidate.len().max(installed.len());
    candidate.resize(width, 0);
    installed.resize(width, 0);
    candidate > installed
}

fn numeric_version(version: &str) -> Option<Vec<u64>> {
    version
        .split('.')
        .map(|part| {
            if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
                None
            } else {
                part.parse().ok()
            }
        })
        .collect()
}

/// A source accepted by the runtime-owned audio player.
///
/// Shelf names are resolved inside the calling application's own shelf. A
/// stream is always HTTPS and carries no credentials, so an application can
/// neither escape its data root nor turn the player into a secret-bearing
/// request primitive.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AudioSource {
    Shelf(String),
    Stream(String),
}

/// Observable state of the runtime-owned audio transport.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum AudioPlaybackState {
    Idle = 1,
    Loading = 2,
    Ready = 3,
    Playing = 4,
    Paused = 5,
    Finished = 6,
}

impl TryFrom<u8> for AudioPlaybackState {
    type Error = ProtocolError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Idle),
            2 => Ok(Self::Loading),
            3 => Ok(Self::Ready),
            4 => Ok(Self::Playing),
            5 => Ok(Self::Paused),
            6 => Ok(Self::Finished),
            _ => Err(ProtocolError::InvalidValue("audio playback state")),
        }
    }
}

/// A Bluetooth device discovered or remembered by the system controller.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BluetoothDevice {
    pub address: String,
    pub name: String,
    pub kind: BluetoothDeviceKind,
    pub paired: bool,
    pub connected: bool,
}

/// The device classes the settings UI can meaningfully distinguish.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum BluetoothDeviceKind {
    Audio = 1,
    Keyboard = 2,
    Input = 3,
    Other = 4,
}

impl TryFrom<u8> for BluetoothDeviceKind {
    type Error = ProtocolError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Audio),
            2 => Ok(Self::Keyboard),
            3 => Ok(Self::Input),
            4 => Ok(Self::Other),
            _ => Err(ProtocolError::InvalidValue("Bluetooth device kind")),
        }
    }
}

/// A Wi-Fi network returned by a scan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WifiNetwork {
    pub ssid: String,
    pub signal_dbm: i16,
    pub secured: bool,
    pub connected: bool,
}

/// Failures from an available radio backend.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum DeviceError {
    NotFound = 1,
    Authentication = 2,
    TimedOut = 3,
    Unreachable = 4,
    InvalidInput = 5,
    Backend = 6,
    /// Downloaded bytes did not match the digest they were promised under.
    Integrity = 7,
}

impl DeviceError {
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::NotFound => "the device or network was not found",
            Self::Authentication => "authentication failed",
            Self::TimedOut => "the radio operation timed out",
            Self::Unreachable => "the device or network is unreachable",
            Self::InvalidInput => "the address or credentials are invalid",
            Self::Backend => "the system radio service failed",
            Self::Integrity => "the download did not match its published digest",
        }
    }
}

impl fmt::Display for DeviceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.describe())
    }
}

impl TryFrom<u8> for DeviceError {
    type Error = ProtocolError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::NotFound),
            2 => Ok(Self::Authentication),
            3 => Ok(Self::TimedOut),
            4 => Ok(Self::Unreachable),
            5 => Ok(Self::InvalidInput),
            6 => Ok(Self::Backend),
            7 => Ok(Self::Integrity),
            _ => Err(ProtocolError::InvalidValue("device error")),
        }
    }
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
#[allow(clippy::too_many_lines)]
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
        Message::SetOrientation(orientation) => payload.push(*orientation as u8),
        Message::Action { action } => {
            push_u32(&mut payload, action.0);
        }
        Message::TextHold {
            action,
            context,
            start,
            end,
        } => {
            push_u32(&mut payload, action.0);
            push_u64(&mut payload, *context);
            push_u32(&mut payload, *start);
            push_u32(&mut payload, *end);
        }
        Message::Log { level, message } => {
            payload.push(*level as u8);
            push_string(&mut payload, message)?;
        }
        Message::Exit => {}
        Message::Launch { name } => push_string(&mut payload, name)?,
        Message::DeviceRequest(request) => encode_device_request(&mut payload, request)?,
        Message::DeviceResult(result) => encode_device_result(&mut payload, result)?,
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
        Message::PutFont {
            handle,
            name,
            bytes,
        } => {
            push_u32(&mut payload, handle.0);
            push_string(&mut payload, name)?;
            payload.extend_from_slice(bytes);
        }
        Message::DropFont { handle } => push_u32(&mut payload, handle.0),
        Message::Lifecycle(state) => payload.push(match state {
            Lifecycle::Foreground => 0,
            Lifecycle::Background => 1,
        }),
        Message::CoverChanged { magnet_present } => payload.push(u8::from(*magnet_present)),
        Message::PageTurn { forward } => payload.push(u8::from(*forward)),
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

/// Writes a credential, or the absence of one.
///
/// Shared by fetch and post so the two cannot drift: a tag written by one and
/// read by the other is how a credential turns into a header nobody meant.
/// Tags are appended rather than inserted, so an older runtime meeting a newer
/// one refuses what it does not know instead of guessing.
fn push_credential(
    payload: &mut Vec<u8>,
    credential: Option<&Credential>,
) -> Result<(), ProtocolError> {
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
            SecretHeader::Basic => {
                payload.push(3);
                push_string(payload, &credential.secret)?;
            }
        },
    }
    Ok(())
}

/// Reads a credential back, refusing a tag it does not know.
fn take_credential(reader: &mut Reader<'_>) -> Result<Option<Credential>, ProtocolError> {
    let credential = match reader.u8()? {
        0 => None,
        1 => Some(Credential::bearer(reader.string()?)),
        2 => {
            let header = reader.string()?;
            Some(Credential::in_header(reader.string()?, header))
        }
        3 => Some(Credential::basic(reader.string()?)),
        _ => return Err(ProtocolError::InvalidValue("credential")),
    };
    if credential
        .as_ref()
        .is_some_and(|credential| !credential.is_well_formed())
    {
        return Err(ProtocolError::InvalidValue("credential"));
    }
    Ok(credential)
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
                    credential,
                    headers,
                } => {
                    payload.push(0);
                    push_string(payload, url)?;
                    push_u32(payload, *offset);
                    push_u32(payload, *max_bytes);
                    push_credential(payload, credential.as_ref())?;
                    payload.push(
                        u8::try_from(headers.len())
                            .map_err(|_| ProtocolError::InvalidValue("too many headers"))?,
                    );
                    for header in headers {
                        push_string(payload, &header.name)?;
                        push_string(payload, &header.value)?;
                    }
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
                    push_long_string(payload, body)?;
                    push_string(payload, content_type)?;
                    push_credential(payload, credential.as_ref())?;
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
        Task::Fetch {
            url,
            credential,
            headers,
            ..
        } => {
            // Refused here rather than stripped silently, for the same reason
            // as `Post`: a request that quietly loses the header an API
            // requires (`Accept`, say) fails at the far end with an error the
            // author cannot connect to anything they wrote.
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
            // Nine for the offset, the ceiling and the header count, and one
            // more for the credential's own tag.
            add_encoded_len(&mut length, 10)?;
            add_encoded_len(&mut length, encoded_string_len(url)?)?;
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
            add_encoded_len(&mut length, encoded_body_len(body)?)?;
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
        Message::SetOrientation(_) => Ok((36, 1)),
        Message::Action { .. } => Ok((4, 4)),
        Message::TextHold { start, end, .. } => {
            if start >= end {
                return Err(ProtocolError::InvalidValue("text hold range"));
            }
            Ok((26, 20))
        }
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
        Message::DeviceRequest(request) => Ok((7, device_request_len(request)?)),
        Message::DeviceResult(result) => Ok((8, device_result_len(result)?)),
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
        Message::CoverChanged { .. } => Ok((23, 1)),
        Message::PageTurn { .. } => Ok((27, 1)),
        Message::PutFont { name, bytes, .. } => {
            if bytes.is_empty() || bytes.len() > MAX_FONT_BYTES {
                return Err(ProtocolError::FrameTooLarge);
            }
            let mut length = 4;
            add_encoded_len(&mut length, encoded_string_len(name)?)?;
            add_encoded_len(&mut length, bytes.len())?;
            Ok((24, length))
        }
        Message::DropFont { .. } => Ok((25, 4)),
    }
}

fn picture_len(width: u32, height: u32) -> Result<usize, ProtocolError> {
    usize::try_from(width)
        .ok()
        .and_then(|width| width.checked_mul(usize::try_from(height).ok()?))
        .ok_or(ProtocolError::FrameTooLarge)
}

#[allow(
    clippy::too_many_lines,
    reason = "one explicit bounded request tag table"
)]
fn encode_device_request(
    output: &mut Vec<u8>,
    request: &DeviceRequest,
) -> Result<(), ProtocolError> {
    match request {
        DeviceRequest::ReadBattery => fixed_device_request(output, 1, 0),
        DeviceRequest::ReadBatteryDetail => output.push(29),
        DeviceRequest::ReadCover => output.push(30),
        DeviceRequest::HoldWifi { seconds } => fixed_device_request(output, 2, *seconds),
        DeviceRequest::ReleaseWifi => fixed_device_request(output, 3, 0),
        DeviceRequest::KeepAwake { seconds } => fixed_device_request(output, 4, *seconds),
        DeviceRequest::AllowSleep => fixed_device_request(output, 5, 0),
        DeviceRequest::ScheduleWake { seconds } => fixed_device_request(output, 6, *seconds),
        DeviceRequest::CancelWake => fixed_device_request(output, 7, 0),
        DeviceRequest::SetFrontlight { percent } => {
            fixed_device_request(output, 8, u32::from(*percent));
        }
        DeviceRequest::ReadFrontlight => fixed_device_request(output, 9, 0),
        DeviceRequest::ReadBluetooth => output.push(10),
        DeviceRequest::SetBluetooth { enabled } => {
            output.extend_from_slice(&[11, u8::from(*enabled)]);
        }
        DeviceRequest::ScanBluetooth => output.push(12),
        DeviceRequest::PairBluetooth { address } => {
            output.push(13);
            push_radio_string(output, address)?;
        }
        DeviceRequest::ConnectBluetooth { address } => {
            output.push(14);
            push_radio_string(output, address)?;
        }
        DeviceRequest::DisconnectBluetooth { address } => {
            output.push(15);
            push_radio_string(output, address)?;
        }
        DeviceRequest::ForgetBluetooth { address } => {
            output.push(16);
            push_radio_string(output, address)?;
        }
        DeviceRequest::ReadWifi => output.push(17),
        DeviceRequest::SetWifi { enabled } => {
            output.extend_from_slice(&[18, u8::from(*enabled)]);
        }
        DeviceRequest::ScanWifi => output.push(19),
        DeviceRequest::JoinWifi { ssid, password } => {
            if ssid.is_empty()
                || ssid.len() > 32
                || !(password.is_empty() || (8..=63).contains(&password.len()))
            {
                return Err(ProtocolError::InvalidValue("Wi-Fi credentials"));
            }
            output.push(20);
            push_radio_string(output, ssid)?;
            push_radio_string(output, password)?;
        }
        DeviceRequest::DisconnectWifi => output.push(21),
        DeviceRequest::ReadAudio => output.push(22),
        DeviceRequest::LoadAudio { source } => {
            output.push(23);
            match source {
                AudioSource::Shelf(name) if is_valid_key(name) => {
                    output.push(1);
                    push_string(output, name)?;
                }
                AudioSource::Stream(url)
                    if url.starts_with("https://") && url.len() <= MAX_URL_LEN =>
                {
                    output.push(2);
                    push_string(output, url)?;
                }
                AudioSource::Shelf(_) | AudioSource::Stream(_) => {
                    return Err(ProtocolError::InvalidValue("audio source"));
                }
            }
        }
        DeviceRequest::PlayAudio => output.push(24),
        DeviceRequest::PauseAudio => output.push(25),
        DeviceRequest::SeekAudio { position_ms } => {
            fixed_device_request(output, 26, *position_ms);
        }
        DeviceRequest::StopAudio => output.push(27),
        DeviceRequest::SetAudioVolume { percent } if *percent <= 100 => {
            output.extend_from_slice(&[28, *percent]);
        }
        DeviceRequest::SetAudioVolume { .. } => {
            return Err(ProtocolError::InvalidValue("audio volume"));
        }
        DeviceRequest::Update { url, sha256 } => {
            if !url.starts_with("https://") || url.len() > MAX_URL_LEN {
                return Err(ProtocolError::InvalidValue("update url"));
            }
            if !is_hex_digest(sha256) {
                return Err(ProtocolError::InvalidValue("update digest"));
            }
            output.push(31);
            push_string(output, url)?;
            push_string(output, sha256)?;
        }
        DeviceRequest::ListInstalledApps => output.push(32),
        DeviceRequest::ReadAppCatalog => output.push(33),
        DeviceRequest::RefreshAppCatalog => output.push(34),
        DeviceRequest::InstallApp { id } if valid_app_id(id) => {
            output.push(35);
            push_string(output, id)?;
        }
        DeviceRequest::UninstallApp { id } if valid_app_id(id) => {
            output.push(36);
            push_string(output, id)?;
        }
        DeviceRequest::InstallApp { .. } | DeviceRequest::UninstallApp { .. } => {
            return Err(ProtocolError::InvalidValue("application id"));
        }
        DeviceRequest::LookupWord { word, language } => {
            validate_lookup(word, language.as_deref())?;
            output.push(37);
            push_string(output, word)?;
            push_optional_string(output, language.as_deref())?;
        }
        DeviceRequest::ReadAppLink => output.push(38),
        DeviceRequest::BeginAppLink => output.push(39),
        DeviceRequest::PollAppLink => output.push(40),
        DeviceRequest::DisconnectAppLink => output.push(41),
        DeviceRequest::ReadIdentity => output.push(42),
        DeviceRequest::ReadAutoUpdate => output.push(43),
        DeviceRequest::SetAutoUpdate { cobalt, apps } => {
            output.extend_from_slice(&[44, radio_flags(*cobalt, *apps)]);
        }
        DeviceRequest::ReadUpdateChannel => output.push(45),
        DeviceRequest::SetUpdateChannel { channel } => {
            output.extend_from_slice(&[46, channel.wire()]);
        }
        DeviceRequest::ReadIdleSleep => output.push(47),
        DeviceRequest::SetIdleSleep(timeout) => {
            output.extend_from_slice(&[48, timeout.to_wire()]);
        }
    }
    Ok(())
}

/// Sixty-four lowercase hex characters: the only shape a SHA-256 hex digest
/// has. Checked at both ends of the wire, so a digest that cannot possibly
/// match anything is refused before a download is spent on it.
fn is_hex_digest(digest: &str) -> bool {
    digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Whether an identity can select only one application-owned directory.
#[must_use]
pub fn valid_app_id(id: &str) -> bool {
    if id.is_empty() || id.len() > MAX_APP_ID_LEN {
        return false;
    }
    let bytes = id.as_bytes();
    bytes[0].is_ascii_lowercase()
        && bytes.last() != Some(&b'-')
        && !bytes.windows(2).any(|pair| pair == b"--")
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

fn fixed_device_request(output: &mut Vec<u8>, tag: u8, argument: u32) {
    output.push(tag);
    push_u32(output, argument);
}

fn device_request_len(request: &DeviceRequest) -> Result<usize, ProtocolError> {
    let mut encoded = Vec::new();
    encode_device_request(&mut encoded, request)?;
    Ok(encoded.len())
}

fn device_result_len(result: &DeviceResult) -> Result<usize, ProtocolError> {
    let mut encoded = Vec::new();
    encode_device_result(&mut encoded, result)?;
    Ok(encoded.len())
}

/// The gauge readings, written as a run of optional fields in a fixed order.
/// Encoder and decoder sit next to each other so the order stays one fact.
fn push_battery_detail(output: &mut Vec<u8>, detail: &BatteryDetail) -> Result<(), ProtocolError> {
    push_optional_u8(output, detail.percent);
    for text in [&detail.status, &detail.health, &detail.technology] {
        push_optional_string(output, text.as_deref())?;
    }
    for value in [
        detail.decidegrees,
        detail.microvolts,
        detail.microamps,
        detail.charge_now,
        detail.charge_full,
        detail.charge_full_design,
    ] {
        push_optional_i32(output, value);
    }
    Ok(())
}

fn battery_detail(reader: &mut Reader<'_>) -> Result<BatteryDetail, ProtocolError> {
    let percent = reader.optional_u8()?;
    if percent.is_some_and(|percent| percent > 100) {
        return Err(ProtocolError::InvalidValue("battery percent"));
    }
    Ok(BatteryDetail {
        percent,
        status: reader.optional_string()?,
        health: reader.optional_string()?,
        technology: reader.optional_string()?,
        decidegrees: reader.optional_i32()?,
        microvolts: reader.optional_i32()?,
        microamps: reader.optional_i32()?,
        charge_now: reader.optional_i32()?,
        charge_full: reader.optional_i32()?,
        charge_full_design: reader.optional_i32()?,
    })
}

fn push_identity(output: &mut Vec<u8>, identity: &DeviceIdentity) -> Result<(), ProtocolError> {
    validate_identity(identity)?;
    for text in [
        &identity.profile_id,
        &identity.model,
        &identity.firmware,
        &identity.kernel,
        &identity.runtime_version,
    ] {
        push_string(output, text)?;
    }
    push_u16(output, identity.device_code);
    push_u32(output, identity.panel_width);
    push_u32(output, identity.panel_height);
    Ok(())
}

fn identity(reader: &mut Reader<'_>) -> Result<DeviceIdentity, ProtocolError> {
    let identity = DeviceIdentity {
        profile_id: reader.string()?,
        model: reader.string()?,
        firmware: reader.string()?,
        kernel: reader.string()?,
        runtime_version: reader.string()?,
        device_code: reader.u16()?,
        panel_width: reader.u32()?,
        panel_height: reader.u32()?,
    };
    validate_identity(&identity)?;
    Ok(identity)
}

fn push_radio_string(output: &mut Vec<u8>, value: &str) -> Result<(), ProtocolError> {
    if value.len() > MAX_RADIO_NAME {
        return Err(ProtocolError::InvalidValue("radio string"));
    }
    push_string(output, value)
}

fn radio_string(reader: &mut Reader<'_>) -> Result<String, ProtocolError> {
    let value = reader.string()?;
    if value.len() > MAX_RADIO_NAME {
        Err(ProtocolError::InvalidValue("radio string"))
    } else {
        Ok(value)
    }
}

fn wifi_ssid(reader: &mut Reader<'_>) -> Result<String, ProtocolError> {
    let ssid = radio_string(reader)?;
    if ssid.is_empty() || ssid.len() > 32 {
        Err(ProtocolError::InvalidValue("Wi-Fi SSID"))
    } else {
        Ok(ssid)
    }
}

fn read_boolean(reader: &mut Reader<'_>, field: &'static str) -> Result<bool, ProtocolError> {
    match reader.u8()? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(ProtocolError::InvalidValue(field)),
    }
}

fn radio_flags(first: bool, second: bool) -> u8 {
    u8::from(first) | (u8::from(second) << 1)
}

const fn flags_first(flags: u8) -> bool {
    flags & 1 != 0
}

const fn flags_second(flags: u8) -> bool {
    flags & 2 != 0
}

fn valid_radio_flags(flags: u8, field: &'static str) -> Result<u8, ProtocolError> {
    if flags & !3 == 0 {
        Ok(flags)
    } else {
        Err(ProtocolError::InvalidValue(field))
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "one explicit bounded request tag table"
)]
fn decode_device_request(reader: &mut Reader<'_>) -> Result<DeviceRequest, ProtocolError> {
    let tag = reader.u8()?;
    match tag {
        1 => fixed_argument(reader, 0).map(|()| DeviceRequest::ReadBattery),
        29 => Ok(DeviceRequest::ReadBatteryDetail),
        30 => Ok(DeviceRequest::ReadCover),
        2 => Ok(DeviceRequest::HoldWifi {
            seconds: reader.u32()?,
        }),
        3 => fixed_argument(reader, 0).map(|()| DeviceRequest::ReleaseWifi),
        4 => Ok(DeviceRequest::KeepAwake {
            seconds: reader.u32()?,
        }),
        5 => fixed_argument(reader, 0).map(|()| DeviceRequest::AllowSleep),
        6 => Ok(DeviceRequest::ScheduleWake {
            seconds: reader.u32()?,
        }),
        7 => fixed_argument(reader, 0).map(|()| DeviceRequest::CancelWake),
        8 => {
            let argument = reader.u32()?;
            let percent = u8::try_from(argument)
                .ok()
                .filter(|percent| *percent <= 100)
                .ok_or(ProtocolError::InvalidValue("frontlight percent"))?;
            Ok(DeviceRequest::SetFrontlight { percent })
        }
        9 => fixed_argument(reader, 0).map(|()| DeviceRequest::ReadFrontlight),
        10 => Ok(DeviceRequest::ReadBluetooth),
        11 => Ok(DeviceRequest::SetBluetooth {
            enabled: read_boolean(reader, "Bluetooth enabled")?,
        }),
        12 => Ok(DeviceRequest::ScanBluetooth),
        13 => Ok(DeviceRequest::PairBluetooth {
            address: radio_string(reader)?,
        }),
        14 => Ok(DeviceRequest::ConnectBluetooth {
            address: radio_string(reader)?,
        }),
        15 => Ok(DeviceRequest::DisconnectBluetooth {
            address: radio_string(reader)?,
        }),
        16 => Ok(DeviceRequest::ForgetBluetooth {
            address: radio_string(reader)?,
        }),
        17 => Ok(DeviceRequest::ReadWifi),
        18 => Ok(DeviceRequest::SetWifi {
            enabled: read_boolean(reader, "Wi-Fi enabled")?,
        }),
        19 => Ok(DeviceRequest::ScanWifi),
        20 => {
            let ssid = wifi_ssid(reader)?;
            let password = radio_string(reader)?;
            if !(password.is_empty() || (8..=63).contains(&password.len())) {
                return Err(ProtocolError::InvalidValue("Wi-Fi credentials"));
            }
            Ok(DeviceRequest::JoinWifi { ssid, password })
        }
        21 => Ok(DeviceRequest::DisconnectWifi),
        22 => Ok(DeviceRequest::ReadAudio),
        23 => match reader.u8()? {
            1 => {
                let name = reader.string()?;
                if is_valid_key(&name) {
                    Ok(DeviceRequest::LoadAudio {
                        source: AudioSource::Shelf(name),
                    })
                } else {
                    Err(ProtocolError::InvalidValue("audio source"))
                }
            }
            2 => {
                let url = reader.string()?;
                if url.starts_with("https://") && url.len() <= MAX_URL_LEN {
                    Ok(DeviceRequest::LoadAudio {
                        source: AudioSource::Stream(url),
                    })
                } else {
                    Err(ProtocolError::InvalidValue("audio source"))
                }
            }
            _ => Err(ProtocolError::InvalidValue("audio source")),
        },
        24 => Ok(DeviceRequest::PlayAudio),
        25 => Ok(DeviceRequest::PauseAudio),
        26 => Ok(DeviceRequest::SeekAudio {
            position_ms: reader.u32()?,
        }),
        27 => Ok(DeviceRequest::StopAudio),
        28 => {
            let percent = reader.u8()?;
            if percent <= 100 {
                Ok(DeviceRequest::SetAudioVolume { percent })
            } else {
                Err(ProtocolError::InvalidValue("audio volume"))
            }
        }
        31 => decode_update(reader),
        32 => Ok(DeviceRequest::ListInstalledApps),
        33 => Ok(DeviceRequest::ReadAppCatalog),
        34 => Ok(DeviceRequest::RefreshAppCatalog),
        35 => decode_app_request(reader, true),
        36 => decode_app_request(reader, false),
        37 => {
            let word = reader.string()?;
            let language = reader.optional_string()?;
            validate_lookup(&word, language.as_deref())?;
            Ok(DeviceRequest::LookupWord { word, language })
        }
        38 => Ok(DeviceRequest::ReadAppLink),
        39 => Ok(DeviceRequest::BeginAppLink),
        40 => Ok(DeviceRequest::PollAppLink),
        41 => Ok(DeviceRequest::DisconnectAppLink),
        42 => Ok(DeviceRequest::ReadIdentity),
        43 => Ok(DeviceRequest::ReadAutoUpdate),
        44 => {
            let flags = valid_radio_flags(reader.u8()?, "auto-update flags")?;
            Ok(DeviceRequest::SetAutoUpdate {
                cobalt: flags_first(flags),
                apps: flags_second(flags),
            })
        }
        45 => Ok(DeviceRequest::ReadUpdateChannel),
        46 => Ok(DeviceRequest::SetUpdateChannel {
            channel: UpdateChannel::from_wire(reader.u8()?)?,
        }),
        47 => Ok(DeviceRequest::ReadIdleSleep),
        48 => IdleSleep::from_wire(reader.u8()?)
            .map(DeviceRequest::SetIdleSleep)
            .ok_or(ProtocolError::InvalidValue("idle sleep")),
        _ => Err(ProtocolError::InvalidValue("device request")),
    }
}

/// Both fields are checked here as well as at the encoder, so a peer that
/// skipped the encoder cannot hand the runtime an unverifiable job.
fn decode_update(reader: &mut Reader<'_>) -> Result<DeviceRequest, ProtocolError> {
    let url = reader.string()?;
    if !url.starts_with("https://") || url.len() > MAX_URL_LEN {
        return Err(ProtocolError::InvalidValue("update url"));
    }
    let sha256 = reader.string()?;
    if !is_hex_digest(&sha256) {
        return Err(ProtocolError::InvalidValue("update digest"));
    }
    Ok(DeviceRequest::Update { url, sha256 })
}

fn decode_app_request(
    reader: &mut Reader<'_>,
    install: bool,
) -> Result<DeviceRequest, ProtocolError> {
    let id = reader.string()?;
    if !valid_app_id(&id) {
        return Err(ProtocolError::InvalidValue("application id"));
    }
    Ok(if install {
        DeviceRequest::InstallApp { id }
    } else {
        DeviceRequest::UninstallApp { id }
    })
}

fn fixed_argument(reader: &mut Reader<'_>, expected: u32) -> Result<(), ProtocolError> {
    if reader.u32()? == expected {
        Ok(())
    } else {
        Err(ProtocolError::InvalidValue("device request argument"))
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "one explicit bounded result tag table"
)]
fn encode_device_result(output: &mut Vec<u8>, result: &DeviceResult) -> Result<(), ProtocolError> {
    match result {
        DeviceResult::Done => output.push(1),
        DeviceResult::Granted { seconds } => {
            output.push(2);
            push_u32(output, *seconds);
        }
        DeviceResult::Battery { percent, charging } => {
            output.push(3);
            output.push(*percent);
            output.push(u8::from(*charging));
        }
        DeviceResult::BatteryDetail(detail) => {
            output.push(10);
            push_battery_detail(output, detail)?;
        }
        DeviceResult::Cover {
            available,
            magnet_present,
        } => output.extend_from_slice(&[11, u8::from(*available), u8::from(*magnet_present)]),
        DeviceResult::Frontlight { percent } => {
            output.push(4);
            output.push(*percent);
        }
        DeviceResult::Denied(reason) => {
            output.push(5);
            output.push(*reason as u8);
        }
        DeviceResult::Bluetooth {
            available,
            enabled,
            devices,
            restart_on_exit,
        } => {
            if devices.len() > MAX_RADIO_DEVICES {
                return Err(ProtocolError::InvalidValue("too many Bluetooth devices"));
            }
            output.extend_from_slice(&[6, radio_flags(*available, *enabled)]);
            output.push(u8::try_from(devices.len()).map_err(|_| ProtocolError::FrameTooLarge)?);
            for device in devices {
                push_radio_string(output, &device.address)?;
                push_radio_string(output, &device.name)?;
                output.push(device.kind as u8);
                output.push(radio_flags(device.paired, device.connected));
            }
            // Trailing, so the shape the device list already had is untouched
            // and only the tail is new.
            output.push(u8::from(*restart_on_exit));
        }
        DeviceResult::Wifi {
            available,
            enabled,
            connected_ssid,
            networks,
        } => {
            if networks.len() > MAX_RADIO_DEVICES {
                return Err(ProtocolError::InvalidValue("too many Wi-Fi networks"));
            }
            output.extend_from_slice(&[7, radio_flags(*available, *enabled)]);
            match connected_ssid {
                Some(ssid) => {
                    if ssid.is_empty() || ssid.len() > 32 {
                        return Err(ProtocolError::InvalidValue("Wi-Fi SSID"));
                    }
                    output.push(1);
                    push_radio_string(output, ssid)?;
                }
                None => output.push(0),
            }
            output.push(u8::try_from(networks.len()).map_err(|_| ProtocolError::FrameTooLarge)?);
            for network in networks {
                if network.ssid.is_empty() || network.ssid.len() > 32 {
                    return Err(ProtocolError::InvalidValue("Wi-Fi SSID"));
                }
                push_radio_string(output, &network.ssid)?;
                push_u16(output, u16::from_be_bytes(network.signal_dbm.to_be_bytes()));
                output.push(radio_flags(network.secured, network.connected));
            }
        }
        DeviceResult::Failed(error) => {
            output.extend_from_slice(&[8, *error as u8]);
        }
        DeviceResult::Audio {
            available,
            state,
            position_ms,
            duration_ms,
            volume,
        } => {
            if *volume > 100 {
                return Err(ProtocolError::InvalidValue("audio volume"));
            }
            output.extend_from_slice(&[9, u8::from(*available), *state as u8]);
            push_u32(output, *position_ms);
            push_u32(output, *duration_ms);
            output.push(*volume);
        }
        DeviceResult::Apps { entries } => {
            if entries.len() > MAX_APP_CATALOG_ENTRIES {
                return Err(ProtocolError::InvalidValue("too many applications"));
            }
            output.push(12);
            push_u16(
                output,
                u16::try_from(entries.len()).map_err(|_| ProtocolError::FrameTooLarge)?,
            );
            for entry in entries {
                encode_app_info(output, entry)?;
            }
        }
        DeviceResult::Dictionary { word, entries } => {
            validate_lookup(word, None)?;
            if entries.len() > MAX_DICTIONARY_ENTRIES {
                return Err(ProtocolError::InvalidValue("too many dictionary entries"));
            }
            output.push(13);
            push_string(output, word)?;
            output.push(u8::try_from(entries.len()).map_err(|_| ProtocolError::FrameTooLarge)?);
            for entry in entries {
                validate_dictionary_entry(entry)?;
                push_string(output, &entry.dictionary)?;
                push_string(output, &entry.language)?;
                push_string(output, &entry.headword)?;
                push_string(output, &entry.definition)?;
            }
        }
        DeviceResult::AppLink(state) => {
            output.push(14);
            encode_app_link(output, state)?;
        }
        DeviceResult::RemoteInstall(outcome) => {
            output.push(15);
            encode_remote_install(output, outcome)?;
        }
        DeviceResult::Identity(identity) => {
            output.push(16);
            push_identity(output, identity)?;
        }
        DeviceResult::AutoUpdate { cobalt, apps } => {
            output.extend_from_slice(&[17, radio_flags(*cobalt, *apps)]);
        }
        DeviceResult::UpdateChannel(channel) => output.extend_from_slice(&[18, channel.wire()]),
        DeviceResult::IdleSleep(timeout) => {
            output.extend_from_slice(&[19, timeout.to_wire()]);
        }
    }
    Ok(())
}

fn valid_app_link_code(code: &str) -> bool {
    code.len() == 8
        && code
            .bytes()
            .all(|byte| b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ".contains(&byte))
}

fn encode_app_link(output: &mut Vec<u8>, state: &AppLinkState) -> Result<(), ProtocolError> {
    match state {
        AppLinkState::Unpaired => output.push(1),
        AppLinkState::Pairing {
            code,
            url,
            expires_in,
        } => {
            if !valid_app_link_code(code)
                || !url.starts_with("https://")
                || url.len() > MAX_URL_LEN
                || *expires_in > MAX_APP_LINK_EXPIRES_IN
            {
                return Err(ProtocolError::InvalidValue("application link"));
            }
            output.push(2);
            push_string(output, code)?;
            push_string(output, url)?;
            push_u32(output, *expires_in);
        }
        AppLinkState::Paired { browsers } if *browsers <= MAX_APP_LINK_BROWSERS => {
            output.extend_from_slice(&[3, *browsers]);
        }
        AppLinkState::Paired { .. } => {
            return Err(ProtocolError::InvalidValue("application link"));
        }
    }
    Ok(())
}

fn encode_remote_install(
    output: &mut Vec<u8>,
    outcome: &RemoteInstallOutcome,
) -> Result<(), ProtocolError> {
    let (tag, id, minimum_cobalt_version) = match outcome {
        RemoteInstallOutcome::None => {
            output.push(1);
            return Ok(());
        }
        RemoteInstallOutcome::Installed { id } => (2, id, None),
        RemoteInstallOutcome::Updated { id } => (3, id, None),
        RemoteInstallOutcome::AlreadyInstalled { id } => (4, id, None),
        RemoteInstallOutcome::Included { id } => (5, id, None),
        RemoteInstallOutcome::Unavailable { id } => (6, id, None),
        RemoteInstallOutcome::RequiresCobalt {
            id,
            minimum_cobalt_version,
        } => (7, id, Some(minimum_cobalt_version)),
    };
    if !valid_app_id(id) {
        return Err(ProtocolError::InvalidValue("application id"));
    }
    if minimum_cobalt_version.is_some_and(|version| !valid_cobalt_version(version)) {
        return Err(ProtocolError::InvalidValue("Cobalt version"));
    }
    output.push(tag);
    push_string(output, id)?;
    if let Some(version) = minimum_cobalt_version {
        push_string(output, version)?;
    }
    Ok(())
}

fn encode_app_info(output: &mut Vec<u8>, entry: &AppInfo) -> Result<(), ProtocolError> {
    validate_app_info(entry)?;
    for text in [
        &entry.id,
        &entry.title,
        &entry.label,
        &entry.summary,
        &entry.version,
        &entry.minimum_cobalt_version,
    ] {
        push_string(output, text)?;
    }
    output.push(encode_glyph(entry.glyph));
    output.push(u8::try_from(entry.capabilities.len()).map_err(|_| ProtocolError::FrameTooLarge)?);
    for capability in &entry.capabilities {
        push_string(output, capability)?;
    }
    match &entry.installed_version {
        Some(version) => {
            output.push(1);
            push_string(output, version)?;
        }
        None => output.push(0),
    }
    Ok(())
}

fn validate_app_info(entry: &AppInfo) -> Result<(), ProtocolError> {
    if !valid_app_id(&entry.id)
        || entry.title.is_empty()
        || entry.title.len() > 96
        || entry.label.is_empty()
        || entry.label.len() > 32
        || entry.summary.is_empty()
        || entry.summary.len() > 512
        || !valid_version(&entry.version)
        || !valid_cobalt_version(&entry.minimum_cobalt_version)
        || entry
            .installed_version
            .as_deref()
            .is_some_and(|version| !valid_version(version))
        || entry.capabilities.len() > MAX_APP_CAPABILITIES
        || entry
            .capabilities
            .iter()
            .any(|capability| capability.is_empty() || capability.len() > 32)
    {
        return Err(ProtocolError::InvalidValue("application metadata"));
    }
    Ok(())
}

fn valid_version(version: &str) -> bool {
    !version.is_empty()
        && version.len() <= MAX_APP_VERSION_LEN
        && version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+'))
}

fn valid_cobalt_version(version: &str) -> bool {
    !version.is_empty()
        && version.len() <= MAX_APP_VERSION_LEN
        && version
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

#[allow(
    clippy::too_many_lines,
    reason = "one explicit bounded result tag table"
)]
fn decode_device_result(reader: &mut Reader<'_>) -> Result<DeviceResult, ProtocolError> {
    match reader.u8()? {
        1 => Ok(DeviceResult::Done),
        2 => Ok(DeviceResult::Granted {
            seconds: reader.u32()?,
        }),
        3 => decode_battery_result(reader),
        10 => battery_detail(reader).map(DeviceResult::BatteryDetail),
        11 => Ok(DeviceResult::Cover {
            available: read_boolean(reader, "cover sensor available")?,
            magnet_present: read_boolean(reader, "cover magnet present")?,
        }),
        4 => {
            let percent = reader.u8()?;
            if percent > 100 {
                return Err(ProtocolError::InvalidValue("frontlight percent"));
            }
            Ok(DeviceResult::Frontlight { percent })
        }
        5 => Ok(DeviceResult::Denied(DenyReason::try_from(reader.u8()?)?)),
        6 => decode_bluetooth_result(reader),
        7 => {
            let flags = valid_radio_flags(reader.u8()?, "Wi-Fi state flags")?;
            let connected_ssid = match reader.u8()? {
                0 => None,
                1 => Some(wifi_ssid(reader)?),
                _ => return Err(ProtocolError::InvalidValue("Wi-Fi connection flag")),
            };
            let count = usize::from(reader.u8()?);
            if count > MAX_RADIO_DEVICES {
                return Err(ProtocolError::InvalidValue("too many Wi-Fi networks"));
            }
            let mut networks = Vec::with_capacity(count);
            for _ in 0..count {
                let ssid = wifi_ssid(reader)?;
                let signal_dbm = i16::from_be_bytes(reader.u16()?.to_be_bytes());
                let network_flags = valid_radio_flags(reader.u8()?, "Wi-Fi network flags")?;
                networks.push(WifiNetwork {
                    ssid,
                    signal_dbm,
                    secured: flags_first(network_flags),
                    connected: flags_second(network_flags),
                });
            }
            Ok(DeviceResult::Wifi {
                available: flags_first(flags),
                enabled: flags_second(flags),
                connected_ssid,
                networks,
            })
        }
        8 => Ok(DeviceResult::Failed(DeviceError::try_from(reader.u8()?)?)),
        9 => decode_audio_result(reader),
        12 => decode_apps_result(reader),
        13 => decode_dictionary_result(reader),
        14 => decode_app_link(reader),
        15 => decode_remote_install(reader),
        16 => identity(reader).map(DeviceResult::Identity),
        17 => decode_auto_update(reader),
        18 => UpdateChannel::from_wire(reader.u8()?).map(DeviceResult::UpdateChannel),
        19 => IdleSleep::from_wire(reader.u8()?)
            .map(DeviceResult::IdleSleep)
            .ok_or(ProtocolError::InvalidValue("idle sleep")),
        _ => Err(ProtocolError::InvalidValue("device result")),
    }
}

fn decode_auto_update(reader: &mut Reader<'_>) -> Result<DeviceResult, ProtocolError> {
    let flags = valid_radio_flags(reader.u8()?, "auto-update flags")?;
    Ok(DeviceResult::AutoUpdate {
        cobalt: flags_first(flags),
        apps: flags_second(flags),
    })
}

fn decode_battery_result(reader: &mut Reader<'_>) -> Result<DeviceResult, ProtocolError> {
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

fn decode_bluetooth_result(reader: &mut Reader<'_>) -> Result<DeviceResult, ProtocolError> {
    let flags = valid_radio_flags(reader.u8()?, "Bluetooth state flags")?;
    let count = usize::from(reader.u8()?);
    if count > MAX_RADIO_DEVICES {
        return Err(ProtocolError::InvalidValue("too many Bluetooth devices"));
    }
    let mut devices = Vec::with_capacity(count);
    for _ in 0..count {
        let address = radio_string(reader)?;
        let name = radio_string(reader)?;
        let kind = BluetoothDeviceKind::try_from(reader.u8()?)?;
        let device_flags = valid_radio_flags(reader.u8()?, "Bluetooth device flags")?;
        devices.push(BluetoothDevice {
            address,
            name,
            kind,
            paired: flags_first(device_flags),
            connected: flags_second(device_flags),
        });
    }
    let restart_on_exit = match reader.u8()? {
        0 => false,
        1 => true,
        _ => return Err(ProtocolError::InvalidValue("Bluetooth restart flag")),
    };
    Ok(DeviceResult::Bluetooth {
        available: flags_first(flags),
        enabled: flags_second(flags),
        devices,
        restart_on_exit,
    })
}

fn decode_app_link(reader: &mut Reader<'_>) -> Result<DeviceResult, ProtocolError> {
    let state = match reader.u8()? {
        1 => AppLinkState::Unpaired,
        2 => {
            let code = reader.string()?;
            let url = reader.string()?;
            let expires_in = reader.u32()?;
            if !valid_app_link_code(&code)
                || !url.starts_with("https://")
                || url.len() > MAX_URL_LEN
                || expires_in > MAX_APP_LINK_EXPIRES_IN
            {
                return Err(ProtocolError::InvalidValue("application link"));
            }
            AppLinkState::Pairing {
                code,
                url,
                expires_in,
            }
        }
        3 => {
            let browsers = reader.u8()?;
            if browsers > MAX_APP_LINK_BROWSERS {
                return Err(ProtocolError::InvalidValue("application link"));
            }
            AppLinkState::Paired { browsers }
        }
        _ => return Err(ProtocolError::InvalidValue("application link")),
    };
    Ok(DeviceResult::AppLink(state))
}

fn decode_remote_install(reader: &mut Reader<'_>) -> Result<DeviceResult, ProtocolError> {
    let tag = reader.u8()?;
    if tag == 1 {
        return Ok(DeviceResult::RemoteInstall(RemoteInstallOutcome::None));
    }
    if !(2..=7).contains(&tag) {
        return Err(ProtocolError::InvalidValue("remote install outcome"));
    }
    let id = reader.string()?;
    if !valid_app_id(&id) {
        return Err(ProtocolError::InvalidValue("application id"));
    }
    let outcome = match tag {
        2 => RemoteInstallOutcome::Installed { id },
        3 => RemoteInstallOutcome::Updated { id },
        4 => RemoteInstallOutcome::AlreadyInstalled { id },
        5 => RemoteInstallOutcome::Included { id },
        6 => RemoteInstallOutcome::Unavailable { id },
        7 => {
            let minimum_cobalt_version = reader.string()?;
            if !valid_cobalt_version(&minimum_cobalt_version) {
                return Err(ProtocolError::InvalidValue("Cobalt version"));
            }
            RemoteInstallOutcome::RequiresCobalt {
                id,
                minimum_cobalt_version,
            }
        }
        _ => unreachable!("tag range checked above"),
    };
    Ok(DeviceResult::RemoteInstall(outcome))
}

fn validate_lookup(word: &str, language: Option<&str>) -> Result<(), ProtocolError> {
    if word.trim().is_empty()
        || word.len() > MAX_LOOKUP_WORD_BYTES
        || language.is_some_and(|language| {
            language.is_empty()
                || language.len() > 16
                || !language
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
    {
        return Err(ProtocolError::InvalidValue("dictionary lookup"));
    }
    Ok(())
}

fn validate_dictionary_entry(entry: &DictionaryEntry) -> Result<(), ProtocolError> {
    if entry.dictionary.is_empty()
        || entry.dictionary.len() > 96
        || entry.language.is_empty()
        || entry.language.len() > 16
        || entry.headword.is_empty()
        || entry.headword.len() > MAX_LOOKUP_WORD_BYTES
        || entry.definition.is_empty()
        || entry.definition.len() > MAX_DICTIONARY_DEFINITION_BYTES
    {
        return Err(ProtocolError::InvalidValue("dictionary entry"));
    }
    Ok(())
}

fn decode_dictionary_result(reader: &mut Reader<'_>) -> Result<DeviceResult, ProtocolError> {
    let word = reader.string()?;
    validate_lookup(&word, None)?;
    let count = usize::from(reader.u8()?);
    if count > MAX_DICTIONARY_ENTRIES {
        return Err(ProtocolError::InvalidValue("too many dictionary entries"));
    }
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let entry = DictionaryEntry {
            dictionary: reader.string()?,
            language: reader.string()?,
            headword: reader.string()?,
            definition: reader.string()?,
        };
        validate_dictionary_entry(&entry)?;
        entries.push(entry);
    }
    Ok(DeviceResult::Dictionary { word, entries })
}

fn decode_apps_result(reader: &mut Reader<'_>) -> Result<DeviceResult, ProtocolError> {
    let count = usize::from(reader.u16()?);
    if count > MAX_APP_CATALOG_ENTRIES {
        return Err(ProtocolError::InvalidValue("too many applications"));
    }
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let id = reader.string()?;
        let title = reader.string()?;
        let label = reader.string()?;
        let summary = reader.string()?;
        let version = reader.string()?;
        let minimum_cobalt_version = reader.string()?;
        let glyph =
            decode_glyph(reader.u8()?).ok_or(ProtocolError::InvalidValue("application glyph"))?;
        let capability_count = usize::from(reader.u8()?);
        if capability_count > MAX_APP_CAPABILITIES {
            return Err(ProtocolError::InvalidValue(
                "too many application capabilities",
            ));
        }
        let mut capabilities = Vec::with_capacity(capability_count);
        for _ in 0..capability_count {
            capabilities.push(reader.string()?);
        }
        let installed_version = match reader.u8()? {
            0 => None,
            1 => Some(reader.string()?),
            _ => return Err(ProtocolError::InvalidValue("installed application flag")),
        };
        let entry = AppInfo {
            id,
            title,
            label,
            summary,
            version,
            minimum_cobalt_version,
            glyph,
            capabilities,
            installed_version,
        };
        validate_app_info(&entry)?;
        entries.push(entry);
    }
    Ok(DeviceResult::Apps { entries })
}

fn decode_audio_result(reader: &mut Reader<'_>) -> Result<DeviceResult, ProtocolError> {
    let available = read_boolean(reader, "audio available")?;
    let state = AudioPlaybackState::try_from(reader.u8()?)?;
    let position_ms = reader.u32()?;
    let duration_ms = reader.u32()?;
    let volume = reader.u8()?;
    if volume > 100 || position_ms > duration_ms && duration_ms != 0 {
        return Err(ProtocolError::InvalidValue("audio state"));
    }
    Ok(DeviceResult::Audio {
        available,
        state,
        position_ms,
        duration_ms,
        volume,
    })
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
        // Four for the identifier and one for how many controls the bar
        // carries. The flag byte saying there is a bar at all is in the base
        // length above, which is paid whether there is one or not.
        add_encoded_len(&mut length, 5)?;
        add_encoded_len(&mut length, encoded_string_len(&top_bar.title)?)?;
        for action in &top_bar.actions {
            add_encoded_len(&mut length, encoded_bar_action_len(action)?)?;
        }
    }
    // One flag byte, plus two action identifiers when the screen asked for
    // tap-to-turn and a third when it also asked for a middle column.
    add_encoded_len(&mut length, 1)?;
    if let Some(turns) = &screen.page_turns {
        add_encoded_len(&mut length, 8)?;
        if turns.menu.is_some() {
            add_encoded_len(&mut length, 4)?;
        }
        // A flag, plus the page and the total when there is one.
        add_encoded_len(&mut length, 1)?;
        if turns.position.is_some() {
            add_encoded_len(&mut length, 4)?;
        }
    }
    // One flag byte, plus an action identifier when the screen asked to hear
    // about a finger held still on it.
    add_encoded_len(&mut length, 1)?;
    if screen.hold.is_some() {
        add_encoded_len(&mut length, 4)?;
    }
    // One flag byte for first refusal on the runtime's Back control, one for a
    // text size this screen asks for in place of the reader's own, and one for
    // whether its text is a book rather than an interface, and one for an
    // optional runtime-held publisher font.
    add_encoded_len(&mut length, 4)?;
    if screen.reading_font.is_some() {
        add_encoded_len(&mut length, 4)?;
    }
    if let Some(nav_bar) = &screen.nav_bar {
        if nav_bar.destinations.len() > u8::MAX as usize {
            return Err(ProtocolError::TooManyNodes);
        }
        add_encoded_len(&mut length, 6)?;
        for destination in &nav_bar.destinations {
            add_encoded_len(&mut length, encoded_bar_action_len(destination)?)?;
        }
    }
    // One flag byte for the pinned control, plus its node, action and label
    // when there is one and no bar has already claimed the band.
    add_encoded_len(&mut length, 1)?;
    if let Some(bottom) = &screen.bottom_action {
        if screen.nav_bar.is_none() {
            add_encoded_len(&mut length, 4)?;
            add_encoded_len(&mut length, encoded_bar_action_len(&bottom.action)?)?;
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
        // The tag, the id, the text, and one byte for the heading's level.
        Node::Heading { text, .. } => {
            let mut length = 6;
            add_encoded_len(&mut length, encoded_string_len(text)?)?;
            length
        }
        Node::Secondary { text, .. } => {
            let mut length = 5;
            add_encoded_len(&mut length, encoded_string_len(text)?)?;
            length
        }
        Node::Text { text, links, .. } => {
            // id, the text, the count, then an action and two offsets each.
            let mut length = 6;
            add_encoded_len(&mut length, encoded_string_len(text)?)?;
            for _ in links.iter().take(kobo_ui::MAX_TEXT_LINKS) {
                add_encoded_len(&mut length, 12)?;
            }
            length
        }
        Node::RichText {
            text,
            spans,
            links,
            selection,
            formulae,
            ..
        } => {
            if spans.len() > kobo_ui::MAX_RICH_TEXT_SPANS {
                return Err(ProtocolError::TooManyNodes);
            }
            let mut length = 5;
            add_encoded_len(&mut length, encoded_string_len(text)?)?;
            add_encoded_len(&mut length, 2 + spans.len() * 9 + 9 + 1 + 1)?;
            for _ in links.iter().take(kobo_ui::MAX_TEXT_LINKS) {
                add_encoded_len(&mut length, 12)?;
            }
            if selection.is_some() {
                add_encoded_len(&mut length, 12)?;
            }
            add_encoded_len(&mut length, 1)?;
            for _ in formulae.iter().take(kobo_ui::MAX_INLINE_FORMULAE) {
                add_encoded_len(&mut length, 20)?;
            }
            length
        }
        Node::Section { title, value, .. } => {
            // id, then the title, then a byte saying whether a value follows.
            let mut length = 6;
            add_encoded_len(&mut length, encoded_string_len(title)?)?;
            if let Some(value) = value {
                add_encoded_len(&mut length, encoded_string_len(value)?)?;
            }
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
        Node::Field {
            value,
            placeholder,
            clear,
            ..
        } => {
            // id, the action, the clear flag, then the two strings. A clear
            // action costs four more.
            let mut length = 10;
            add_encoded_len(&mut length, encoded_string_len(value)?)?;
            add_encoded_len(&mut length, encoded_string_len(placeholder)?)?;
            if clear.is_some() {
                add_encoded_len(&mut length, 4)?;
            }
            length
        }
        Node::Chips { chips, .. } => {
            // id, the count, then an action, a selected flag and a label each.
            let mut length = 6;
            for chip in chips.iter().take(kobo_ui::MAX_CHIPS) {
                add_encoded_len(&mut length, 5)?;
                add_encoded_len(&mut length, encoded_string_len(&chip.label)?)?;
            }
            length
        }
        Node::Tabs { tabs, .. } => {
            // id, the count, the selection, then an action and a label each.
            let mut length = 7;
            for tab in tabs.iter().take(kobo_ui::MAX_TABS) {
                add_encoded_len(&mut length, 4)?;
                add_encoded_len(&mut length, encoded_string_len(&tab.label)?)?;
            }
            length
        }
        Node::Facts { entries, .. } => {
            // id, the count, then a label and a value for each pair.
            let mut length = 7;
            for (label, value) in entries.iter().take(kobo_ui::MAX_FACTS) {
                add_encoded_len(&mut length, encoded_string_len(label)?)?;
                add_encoded_len(&mut length, encoded_string_len(value)?)?;
            }
            length
        }
        Node::Band { slots, .. } => {
            // id, the alignment, the slot count, then each slot's width token
            // and the node inside it. A fixed width costs two more bytes.
            let mut length = 7;
            for slot in slots.iter().take(kobo_ui::MAX_BAND_SLOTS) {
                add_encoded_len(&mut length, 1)?;
                if matches!(slot.width, kobo_ui::SlotWidth::Fixed(_)) {
                    add_encoded_len(&mut length, 2)?;
                }
                add_encoded_len(&mut length, 2)?;
                for node in &slot.nodes {
                    add_encoded_len(&mut length, encoded_node_len(node, depth + 1, count)?)?;
                }
            }
            length
        }
        Node::Divider { .. } => 5,
        // One tag byte, not a length. This was 9 while the node still carried
        // a raw i32, which over-reserved the frame by three bytes and tripped
        // the encoder's own length assertion in debug builds.
        Node::Spacer { .. } => 6,
        // Tag and identifier and nothing else: a flex carries no value.
        Node::Flex { .. } => 5,
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
                add_encoded_len(&mut length, if cell.glyph.is_some() { 2 } else { 1 })?;
            }
            length
        }
        Node::Rows { rows, .. } => {
            if rows.len() > u8::MAX as usize {
                return Err(ProtocolError::TooManyNodes);
            }
            let mut length = 6;
            for row in rows {
                // Four bytes of action, the fixed-width lead, one of state,
                // one saying whether a trailing value follows and one saying
                // whether an overflow action does, then the strings.
                add_encoded_len(&mut length, 7 + ROW_LEAD_LEN)?;
                add_encoded_len(&mut length, encoded_string_len(&row.title)?)?;
                add_encoded_len(&mut length, encoded_string_len(&row.summary)?)?;
                if let Some(trailing) = &row.trailing {
                    add_encoded_len(&mut length, encoded_string_len(trailing)?)?;
                }
                if row.menu.is_some() {
                    add_encoded_len(&mut length, 4)?;
                }
            }
            length
        }
        Node::TileGrid { tiles, .. } => {
            if tiles.len() > u8::MAX as usize {
                return Err(ProtocolError::TooManyNodes);
            }
            let mut length = 7;
            for tile in tiles {
                add_encoded_len(&mut length, 7)?;
                add_encoded_len(&mut length, encoded_string_len(&tile.label)?)?;
                add_encoded_len(&mut length, encoded_string_len(&tile.badge)?)?;
                add_encoded_len(&mut length, encoded_string_len(&tile.subtitle)?)?;
                if tile.picture.is_some() {
                    add_encoded_len(&mut length, 12)?;
                }
            }
            length
        }
        Node::Picture { .. } => 20,
        Node::Table { rows, weights, .. } => {
            if rows.len() > u8::MAX as usize || weights.len() > u8::MAX as usize {
                return Err(ProtocolError::TooManyNodes);
            }
            let mut length = 7;
            add_encoded_len(&mut length, weights.len().saturating_mul(2))?;
            for row in rows {
                if row.cells.len() > u8::MAX as usize {
                    return Err(ProtocolError::TooManyNodes);
                }
                add_encoded_len(&mut length, 2)?;
                for cell in &row.cells {
                    add_encoded_len(&mut length, encoded_string_len(cell)?)?;
                }
            }
            length
        }
        Node::Stepper {
            label, less, more, ..
        } => {
            let mut length = 8;
            add_encoded_len(&mut length, encoded_string_len(label)?)?;
            add_encoded_len(&mut length, encoded_bar_action_len(less)?)?;
            add_encoded_len(&mut length, encoded_bar_action_len(more)?)?;
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
            let mut length = 8;
            add_encoded_len(&mut length, encoded_string_len(prompt)?)?;
            for option in options {
                add_encoded_len(&mut length, encoded_bar_action_len(option)?)?;
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
        Node::Splash {
            glyph,
            title,
            summary,
            ..
        } => {
            let mut length = 6;
            if glyph.is_some() {
                add_encoded_len(&mut length, 1)?;
            }
            add_encoded_len(&mut length, encoded_string_len(title)?)?;
            add_encoded_len(&mut length, encoded_string_len(summary)?)?;
            length
        }
        Node::Activity {
            label,
            progress,
            cancel,
            transferred,
            failure,
            ..
        } => {
            let mut length = 7;
            add_encoded_len(&mut length, encoded_string_len(label)?)?;
            if progress.is_some() {
                add_encoded_len(&mut length, 1)?;
            }
            // A flag, plus the received count and a flagged total.
            add_encoded_len(&mut length, 1)?;
            if let Some((_, total)) = transferred {
                add_encoded_len(&mut length, 9)?;
                if total.is_some() {
                    add_encoded_len(&mut length, 8)?;
                }
            }
            add_encoded_len(&mut length, 1)?;
            if let Some(failure) = failure {
                add_encoded_len(&mut length, 1)?;
                add_encoded_len(&mut length, encoded_string_len(&failure.reason)?)?;
            }
            if let Some(cancel) = cancel {
                add_encoded_len(&mut length, encoded_bar_action_len(cancel)?)?;
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

fn encoded_body_len(text: &str) -> Result<usize, ProtocolError> {
    if text.len() > MAX_POST_BODY_LEN {
        return Err(ProtocolError::StringTooLarge);
    }
    Ok(4 + text.len())
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
        36 => Message::SetOrientation(
            Orientation::from_wire(reader.u8()?)
                .ok_or(ProtocolError::InvalidValue("orientation"))?,
        ),
        4 => Message::Action {
            action: ActionId(reader.u32()?),
        },
        26 => {
            let action = ActionId(reader.u32()?);
            let context = reader.u64()?;
            let start = reader.u32()?;
            let end = reader.u32()?;
            if start >= end {
                return Err(ProtocolError::InvalidValue("text hold range"));
            }
            Message::TextHold {
                action,
                context,
                start,
                end,
            }
        }
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
                    let offset = reader.u32()?;
                    // Clamped here rather than trusted, so a task cannot
                    // declare a ceiling larger than the transport can carry
                    // and then be surprised when the answer will not fit.
                    let max_bytes = min(reader.u32()?, MAX_TASK_BYTES_U32);
                    let credential = take_credential(&mut reader)?;
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
                    Task::Fetch {
                        url,
                        offset,
                        max_bytes,
                        credential,
                        headers,
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
                    let body = reader.long_string()?;
                    let content_type = reader.string()?;
                    let credential = take_credential(&mut reader)?;
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
        23 => Message::CoverChanged {
            magnet_present: read_boolean(&mut reader, "cover magnet present")?,
        },
        27 => Message::PageTurn {
            forward: read_boolean(&mut reader, "page turn direction")?,
        },
        24 => {
            let handle = FontHandle(reader.u32()?);
            let name = reader.string()?;
            let length = reader.remaining();
            if length == 0 || length > MAX_FONT_BYTES {
                return Err(ProtocolError::FrameTooLarge);
            }
            Message::PutFont {
                handle,
                name,
                bytes: reader.take(length)?.to_vec(),
            }
        }
        25 => Message::DropFont {
            handle: FontHandle(reader.u32()?),
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

fn encode_top_bar(output: &mut Vec<u8>, top_bar: Option<&TopBar>) -> Result<(), ProtocolError> {
    let Some(top_bar) = top_bar else {
        output.push(0);
        return Ok(());
    };
    output.push(1);
    push_u32(output, top_bar.id.0);
    push_string(output, &top_bar.title)?;
    // A count rather than a flag. One control was the whole shape of this bar
    // until a reading screen needed the type size and the front light kept
    // apart, and a flag byte cannot grow into two without every peer
    // disagreeing about what the next byte is.
    let actions = &top_bar.actions[..min(top_bar.actions.len(), MAX_BAR_ACTIONS)];
    output.push(u8::try_from(actions.len()).unwrap_or(0));
    for action in actions {
        encode_bar_action(output, action)?;
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
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
    encode_top_bar(output, screen.top_bar.as_ref())?;
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
            // The presence flag doubles as the style, rather than costing a
            // byte of its own: 1 is a bar of destinations, 2 a bar of verbs.
            output.push(match nav_bar.style {
                BarStyle::Navigation => 1,
                BarStyle::Actions => 2,
            });
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
            // Two shapes rather than an action identifier of zero for "no
            // menu": zero is a legitimate hash, and a screen whose middle
            // column silently did nothing would be indistinguishable from one
            // whose middle column asked for a control the application forgot
            // to answer.
            output.push(if turns.menu.is_some() { 2 } else { 1 });
            push_u32(output, turns.previous.0);
            push_u32(output, turns.next.0);
            if let Some(menu) = turns.menu {
                push_u32(output, menu.0);
            }
            match turns.position {
                Some((page, of)) => {
                    output.push(1);
                    push_u16(output, page);
                    push_u16(output, of);
                }
                None => output.push(0),
            }
        }
    }
    match screen.hold {
        None => output.push(0),
        Some(hold) => {
            output.push(1);
            push_u32(output, hold.0);
        }
    }
    output.push(u8::from(screen.owns_back));
    // Zero means inherit, so a screen that says nothing keeps the reader's own
    // setting and the byte costs nothing to leave alone.
    output.push(screen.text_scale.map_or(0, |scale| scale.wire_value() + 1));
    output.push(u8::from(screen.reading));
    match screen.reading_font {
        None => output.push(0),
        Some(handle) => {
            output.push(1);
            push_u32(output, handle.0);
        }
    }
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
    // The mark travels with every bar action rather than only the top bar's.
    // It used to be encoded beside the top bar alone, on the reasoning that a
    // word is what a control is everywhere else; the bottom band disagreed as
    // soon as it had to say "Return to Kobo reader" in a slot a third of a
    // panel wide.
    match action.glyph {
        None => output.push(0),
        Some(glyph) => {
            output.push(1);
            output.push(encode_glyph(glyph));
        }
    }
    push_string(output, &action.label)
}

/// Four for the identifier, one flag for whether it carries a mark, one more
/// naming the mark, and the label.
fn encoded_bar_action_len(action: &BarAction) -> Result<usize, ProtocolError> {
    let mut length = 5;
    if action.glyph.is_some() {
        add_encoded_len(&mut length, 1)?;
    }
    add_encoded_len(&mut length, encoded_string_len(&action.label)?)?;
    Ok(length)
}

fn decode_bar_action(reader: &mut Reader<'_>) -> Result<BarAction, ProtocolError> {
    let action = ActionId(reader.u32()?);
    // The runtime owns going back, so an application is not allowed to name
    // that identifier. Rejecting it here means a hostile frame cannot forge a
    // control the reader is entitled to trust.
    if action.is_reserved() {
        return Err(ProtocolError::InvalidValue("reserved action id"));
    }
    let glyph = match reader.u8()? {
        0 => None,
        1 => Some(decode_glyph(reader.u8()?).ok_or(ProtocolError::InvalidValue("bar glyph"))?),
        _ => return Err(ProtocolError::InvalidValue("bar glyph flag")),
    };
    Ok(BarAction {
        action,
        label: reader.string()?,
        glyph,
    })
}

// One exhaustive match over every node kind. Splitting it would only move
// arms out of reach of the compiler's exhaustiveness check, which is the one
// thing making it impossible to add a node and forget the wire format.
fn text_presentation_byte(style: kobo_ui::TextPresentation) -> u8 {
    u8::from(style.strong)
        | (u8::from(style.emphasis) << 1)
        | (u8::from(style.underline) << 2)
        | (u8::from(style.superscript) << 3)
        | (u8::from(style.subscript) << 4)
        | (u8::from(style.highlighted) << 5)
}

fn text_presentation_from_byte(value: u8) -> Result<kobo_ui::TextPresentation, ProtocolError> {
    if value & !0x3f != 0 {
        return Err(ProtocolError::InvalidValue("rich text style"));
    }
    Ok(kobo_ui::TextPresentation {
        strong: value & 1 != 0,
        emphasis: value & 2 != 0,
        underline: value & 4 != 0,
        superscript: value & 8 != 0,
        subscript: value & 16 != 0,
        highlighted: value & 32 != 0,
    })
}

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
        Node::Heading { id, text, level } => {
            output.push(1);
            push_u32(output, id.0);
            push_string(output, text)?;
            output.push(*level);
        }
        Node::Text { id, text, links } => {
            let links = &links[..links.len().min(kobo_ui::MAX_TEXT_LINKS)];
            output.push(2);
            push_u32(output, id.0);
            push_string(output, text)?;
            output.push(u8::try_from(links.len()).map_err(|_| ProtocolError::TooManyNodes)?);
            for link in links {
                push_u32(output, link.action.0);
                push_u32(output, u32::try_from(link.start).unwrap_or(u32::MAX));
                push_u32(output, u32::try_from(link.end).unwrap_or(u32::MAX));
            }
        }
        Node::RichText {
            id,
            text,
            spans,
            links,
            presentation,
            selection,
            formulae,
        } => {
            if spans.len() > kobo_ui::MAX_RICH_TEXT_SPANS {
                return Err(ProtocolError::TooManyNodes);
            }
            output.push(28);
            push_u32(output, id.0);
            push_string(output, text)?;
            push_u16(
                output,
                u16::try_from(spans.len()).map_err(|_| ProtocolError::TooManyNodes)?,
            );
            for span in spans {
                if span.start >= span.end
                    || span.end > text.len()
                    || !text.is_char_boundary(span.start)
                    || !text.is_char_boundary(span.end)
                {
                    return Err(ProtocolError::InvalidValue("rich text span"));
                }
                push_u32(
                    output,
                    u32::try_from(span.start).map_err(|_| ProtocolError::FrameTooLarge)?,
                );
                push_u32(
                    output,
                    u32::try_from(span.end).map_err(|_| ProtocolError::FrameTooLarge)?,
                );
                output.push(text_presentation_byte(span.presentation));
            }
            output.push(match presentation.alignment {
                kobo_ui::ParagraphAlignment::Start => 0,
                kobo_ui::ParagraphAlignment::Center => 1,
                kobo_ui::ParagraphAlignment::End => 2,
                kobo_ui::ParagraphAlignment::Justify => 3,
            });
            push_u16(output, presentation.line_height_percent);
            push_u16(output, presentation.margin_before_em);
            push_u16(output, presentation.margin_after_em);
            push_u16(
                output,
                u16::from_ne_bytes(presentation.first_line_indent_em.to_ne_bytes()),
            );
            let links = &links[..links.len().min(kobo_ui::MAX_TEXT_LINKS)];
            output.push(u8::try_from(links.len()).map_err(|_| ProtocolError::TooManyNodes)?);
            for link in links {
                push_u32(output, link.action.0);
                push_u32(output, u32::try_from(link.start).unwrap_or(u32::MAX));
                push_u32(output, u32::try_from(link.end).unwrap_or(u32::MAX));
            }
            match selection {
                Some(selection) => {
                    output.push(1);
                    push_u64(output, selection.context);
                    push_u32(output, selection.offset);
                }
                None => output.push(0),
            }
            let formulae = &formulae[..formulae.len().min(kobo_ui::MAX_INLINE_FORMULAE)];
            output.push(u8::try_from(formulae.len()).map_err(|_| ProtocolError::TooManyNodes)?);
            for formula in formulae {
                if formula.start >= formula.end
                    || formula.end > text.len()
                    || !text.is_char_boundary(formula.start)
                    || !text.is_char_boundary(formula.end)
                {
                    return Err(ProtocolError::InvalidValue("inline formula"));
                }
                push_u32(output, formula.handle.0);
                push_u32(
                    output,
                    u32::try_from(formula.start).map_err(|_| ProtocolError::FrameTooLarge)?,
                );
                push_u32(
                    output,
                    u32::try_from(formula.end).map_err(|_| ProtocolError::FrameTooLarge)?,
                );
                push_u32(output, formula.source.0);
                push_u32(output, formula.source.1);
            }
        }
        Node::Secondary { id, text } => {
            output.push(19);
            push_u32(output, id.0);
            push_string(output, text)?;
        }
        Node::Section { id, title, value } => {
            output.push(21);
            push_u32(output, id.0);
            push_string(output, title)?;
            output.push(u8::from(value.is_some()));
            if let Some(value) = value {
                push_string(output, value)?;
            }
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
        Node::Field {
            id,
            action,
            value,
            placeholder,
            clear,
        } => {
            output.push(24);
            push_u32(output, id.0);
            push_u32(output, action.0);
            push_string(output, value)?;
            push_string(output, placeholder)?;
            match clear {
                Some(clear) => {
                    output.push(1);
                    push_u32(output, clear.0);
                }
                None => output.push(0),
            }
        }
        Node::Chips { id, chips } => {
            let chips = &chips[..chips.len().min(kobo_ui::MAX_CHIPS)];
            output.push(25);
            push_u32(output, id.0);
            output.push(u8::try_from(chips.len()).map_err(|_| ProtocolError::TooManyNodes)?);
            for chip in chips {
                push_u32(output, chip.action.0);
                output.push(u8::from(chip.selected));
                push_string(output, &chip.label)?;
            }
        }
        Node::Tabs { id, tabs, selected } => {
            let tabs = &tabs[..tabs.len().min(kobo_ui::MAX_TABS)];
            output.push(26);
            push_u32(output, id.0);
            output.push(u8::try_from(tabs.len()).map_err(|_| ProtocolError::TooManyNodes)?);
            // Clamped rather than refused, for the reason the choice marker
            // gives: a selection nobody named is a caller mistake, and losing
            // the whole screen over it is worse than showing the first tab.
            output.push(u8::try_from(*selected).unwrap_or(0));
            for tab in tabs {
                push_u32(output, tab.action.0);
                push_string(output, &tab.label)?;
            }
        }
        Node::Facts { id, entries } => {
            let entries = &entries[..entries.len().min(kobo_ui::MAX_FACTS)];
            output.push(23);
            push_u32(output, id.0);
            push_u16(
                output,
                u16::try_from(entries.len()).map_err(|_| ProtocolError::TooManyNodes)?,
            );
            for (label, value) in entries {
                push_string(output, label)?;
                push_string(output, value)?;
            }
        }
        Node::Band { id, align, slots } => {
            let slots = &slots[..slots.len().min(kobo_ui::MAX_BAND_SLOTS)];
            output.push(22);
            push_u32(output, id.0);
            output.push(match align {
                kobo_ui::BandAlign::Top => 0,
                kobo_ui::BandAlign::Middle => 1,
                kobo_ui::BandAlign::Bottom => 2,
            });
            output.push(u8::try_from(slots.len()).map_err(|_| ProtocolError::TooManyNodes)?);
            for slot in slots {
                match slot.width {
                    kobo_ui::SlotWidth::Natural => output.push(0),
                    kobo_ui::SlotWidth::Fill => output.push(1),
                    kobo_ui::SlotWidth::Fixed(tenths) => {
                        output.push(2);
                        push_u16(output, tenths);
                    }
                }
                push_u16(
                    output,
                    u16::try_from(slot.nodes.len()).map_err(|_| ProtocolError::TooManyNodes)?,
                );
                for node in &slot.nodes {
                    encode_node(output, node, depth + 1, count)?;
                }
            }
        }
        Node::Divider { id } => {
            output.push(5);
            push_u32(output, id.0);
        }
        Node::Flex { id } => {
            output.push(27);
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
                match cell.glyph {
                    None => output.push(0),
                    Some(glyph) => {
                        output.push(1);
                        output.push(encode_glyph(glyph));
                    }
                }
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
                output.push(u8::from(row.trailing.is_some()));
                if let Some(trailing) = &row.trailing {
                    push_string(output, trailing)?;
                }
                output.push(u8::from(row.menu.is_some()));
                if let Some(menu) = row.menu {
                    push_u32(output, menu.0);
                }
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
                output.push(match tile.state {
                    TileState::Normal => 0,
                    TileState::Held => 1,
                    TileState::Unavailable => 2,
                    TileState::Busy => 3,
                });
                push_string(output, &tile.badge)?;
                push_string(output, &tile.subtitle)?;
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
            framed,
        } => {
            output.push(17);
            push_u32(output, id.0);
            push_u32(output, handle.0);
            push_u32(output, source.0);
            push_u32(output, source.1);
            push_u16(output, *max_height_tenths_mm);
            output.push(u8::from(*framed));
        }
        Node::Table { id, rows, weights } => {
            output.push(30);
            push_u32(output, id.0);
            output.push(u8::try_from(weights.len()).map_err(|_| ProtocolError::TooManyNodes)?);
            for weight in weights {
                push_u16(output, *weight);
            }
            output.push(u8::try_from(rows.len()).map_err(|_| ProtocolError::TooManyNodes)?);
            for row in rows {
                output.push(u8::from(row.header));
                output
                    .push(u8::try_from(row.cells.len()).map_err(|_| ProtocolError::TooManyNodes)?);
                for cell in &row.cells {
                    push_string(output, cell)?;
                }
            }
        }
        Node::Stepper {
            id,
            label,
            less,
            more,
            less_state,
            more_state,
            fill,
        } => {
            output.push(29);
            push_u32(output, id.0);
            push_string(output, label)?;
            encode_bar_action(output, less)?;
            encode_bar_action(output, more)?;
            for state in [less_state, more_state] {
                output.push(match state {
                    ControlState::Enabled => 0,
                    ControlState::Disabled => 1,
                });
            }
            // Sent as one past the reading so that "no track" is zero, which is
            // what a peer that never asks for one produces.
            output.push(match fill {
                None => 0,
                Some(fill) => fill.min(&100).saturating_add(1),
            });
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
        Node::Splash {
            id,
            glyph,
            title,
            summary,
        } => {
            output.push(20);
            push_u32(output, id.0);
            match glyph {
                None => output.push(0),
                Some(glyph) => {
                    output.push(1);
                    output.push(encode_glyph(*glyph));
                }
            }
            push_string(output, title)?;
            push_string(output, summary)?;
        }
        Node::Activity {
            id,
            label,
            progress,
            cancel,
            transferred,
            failure,
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
            match transferred {
                None => output.push(0),
                Some((received, total)) => {
                    output.push(1);
                    push_u64(output, *received);
                    match total {
                        None => output.push(0),
                        Some(total) => {
                            output.push(1);
                            push_u64(output, *total);
                        }
                    }
                }
            }
            match failure {
                None => output.push(0),
                Some(failure) => {
                    output.push(1);
                    output.push(u8::from(failure.resumable));
                    push_string(output, &failure.reason)?;
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
        Glyph::Light => 19,
        Glyph::Close => 20,
        Glyph::Download => 21,
        Glyph::Bookmark => 22,
        Glyph::Filter => 23,
        Glyph::Person => 24,
        Glyph::Tag => 25,
        Glyph::Globe => 26,
        Glyph::Refresh => 27,
        Glyph::More => 28,
        Glyph::Bluetooth => 29,
        Glyph::Key => 30,
        Glyph::Magnet => 31,
        Glyph::Play => 32,
        Glyph::Pause => 33,
        Glyph::Rewind30 => 34,
        Glyph::Forward30 => 35,
        Glyph::VolumeDown => 36,
        Glyph::VolumeUp => 37,
        Glyph::MoreVertical => 38,
        Glyph::Trash => 39,
        Glyph::Previous => 40,
        Glyph::Next => 41,
        Glyph::Plus => 42,
        Glyph::Headphones => 43,
        Glyph::Minus => 44,
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
        19 => Glyph::Light,
        20 => Glyph::Close,
        21 => Glyph::Download,
        22 => Glyph::Bookmark,
        23 => Glyph::Filter,
        24 => Glyph::Person,
        25 => Glyph::Tag,
        26 => Glyph::Globe,
        27 => Glyph::Refresh,
        28 => Glyph::More,
        29 => Glyph::Bluetooth,
        30 => Glyph::Key,
        31 => Glyph::Magnet,
        32 => Glyph::Play,
        33 => Glyph::Pause,
        34 => Glyph::Rewind30,
        35 => Glyph::Forward30,
        36 => Glyph::VolumeDown,
        37 => Glyph::VolumeUp,
        38 => Glyph::MoreVertical,
        39 => Glyph::Trash,
        40 => Glyph::Previous,
        41 => Glyph::Next,
        42 => Glyph::Plus,
        43 => Glyph::Headphones,
        44 => Glyph::Minus,

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
            let count = usize::from(reader.u8()?);
            if count > MAX_BAR_ACTIONS {
                return Err(ProtocolError::InvalidValue("top bar action count"));
            }
            let mut actions = Vec::with_capacity(count);
            for _ in 0..count {
                actions.push(decode_bar_action(reader)?);
            }
            Some(TopBar {
                id: bar_id,
                title,
                actions,
            })
        }
        _ => return Err(ProtocolError::InvalidValue("top bar flag")),
    };
    let nav_bar = match reader.u8()? {
        0 => None,
        style @ (1 | 2) => {
            let style = if style == 1 {
                BarStyle::Navigation
            } else {
                BarStyle::Actions
            };
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
                style,
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
        2 => Some(
            PageTurns::new(ActionId(reader.u32()?), ActionId(reader.u32()?))
                .with_menu(ActionId(reader.u32()?)),
        ),
        _ => return Err(ProtocolError::InvalidValue("page turn flag")),
    };
    let page_turns = match page_turns {
        None => None,
        Some(turns) => Some(match reader.u8()? {
            0 => turns,
            1 => turns.with_position(reader.u16()?, reader.u16()?),
            _ => return Err(ProtocolError::InvalidValue("page position flag")),
        }),
    };
    let hold = match reader.u8()? {
        0 => None,
        1 => Some(ActionId(reader.u32()?)),
        _ => return Err(ProtocolError::InvalidValue("hold flag")),
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
    let reading_font = match reader.u8()? {
        0 => None,
        1 => Some(FontHandle(reader.u32()?)),
        _ => return Err(ProtocolError::InvalidValue("reading font flag")),
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
    screen.hold = hold;
    screen.owns_back = owns_back;
    screen.text_scale = text_scale;
    screen.reading = reading;
    screen.reading_font = reading_font;
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
            level: reader.u8()?,
        }),
        2 => {
            let text = reader.string()?;
            let count = usize::from(reader.u8()?);
            let mut links = Vec::with_capacity(count.min(kobo_ui::MAX_TEXT_LINKS));
            for _ in 0..count {
                let action = ActionId(reader.u32()?);
                let start = reader.u32()? as usize;
                let end = reader.u32()? as usize;
                // Checked here rather than trusted, because these index a
                // string that came off the same wire and a bad pair would
                // panic the renderer rather than draw something wrong.
                if start < end
                    && end <= text.len()
                    && text.is_char_boundary(start)
                    && text.is_char_boundary(end)
                {
                    links.push(kobo_ui::TextLink { action, start, end });
                }
            }
            Ok(Node::Text { id, text, links })
        }
        28 => {
            let text = reader.string()?;
            let span_count = usize::from(reader.u16()?);
            if span_count > kobo_ui::MAX_RICH_TEXT_SPANS {
                return Err(ProtocolError::TooManyNodes);
            }
            let mut spans = Vec::with_capacity(span_count);
            for _ in 0..span_count {
                let start = reader.u32()? as usize;
                let end = reader.u32()? as usize;
                let style = text_presentation_from_byte(reader.u8()?)?;
                if start >= end
                    || end > text.len()
                    || !text.is_char_boundary(start)
                    || !text.is_char_boundary(end)
                {
                    return Err(ProtocolError::InvalidValue("rich text span"));
                }
                spans.push(kobo_ui::RichTextSpan {
                    start,
                    end,
                    presentation: style,
                });
            }
            let alignment = match reader.u8()? {
                0 => kobo_ui::ParagraphAlignment::Start,
                1 => kobo_ui::ParagraphAlignment::Center,
                2 => kobo_ui::ParagraphAlignment::End,
                3 => kobo_ui::ParagraphAlignment::Justify,
                _ => return Err(ProtocolError::InvalidValue("paragraph alignment")),
            };
            let presentation = kobo_ui::ParagraphPresentation {
                alignment,
                line_height_percent: reader.u16()?.clamp(80, 250),
                margin_before_em: reader.u16()?.min(2_000),
                margin_after_em: reader.u16()?.min(2_000),
                first_line_indent_em: i16::from_ne_bytes(reader.u16()?.to_ne_bytes())
                    .clamp(-2_000, 2_000),
            };
            let link_count = usize::from(reader.u8()?);
            let mut links = Vec::with_capacity(link_count.min(kobo_ui::MAX_TEXT_LINKS));
            for _ in 0..link_count {
                let action = ActionId(reader.u32()?);
                let start = reader.u32()? as usize;
                let end = reader.u32()? as usize;
                if start < end
                    && end <= text.len()
                    && text.is_char_boundary(start)
                    && text.is_char_boundary(end)
                    && links.len() < kobo_ui::MAX_TEXT_LINKS
                {
                    links.push(kobo_ui::TextLink { action, start, end });
                }
            }
            let selection = match reader.u8()? {
                0 => None,
                1 => Some(kobo_ui::TextSelection {
                    context: reader.u64()?,
                    offset: reader.u32()?,
                }),
                _ => return Err(ProtocolError::InvalidValue("text selection flag")),
            };
            let formula_count = usize::from(reader.u8()?);
            let mut formulae = Vec::with_capacity(formula_count.min(kobo_ui::MAX_INLINE_FORMULAE));
            for _ in 0..formula_count {
                let handle = PictureHandle(reader.u32()?);
                let start = reader.u32()? as usize;
                let end = reader.u32()? as usize;
                let source = (reader.u32()?, reader.u32()?);
                // A formula that does not land on the text it stands in for
                // is dropped rather than drawn: the words are still there,
                // and a picture at the wrong offset would cover the wrong
                // ones. Formulas are kept in order and never overlapping so
                // that layout can walk them alongside the string.
                if start < end
                    && end <= text.len()
                    && text.is_char_boundary(start)
                    && text.is_char_boundary(end)
                    && formulae.len() < kobo_ui::MAX_INLINE_FORMULAE
                    && formulae
                        .last()
                        .is_none_or(|last: &kobo_ui::InlineFormula| last.end <= start)
                {
                    formulae.push(kobo_ui::InlineFormula {
                        start,
                        end,
                        handle,
                        source,
                    });
                }
            }
            Ok(Node::RichText {
                id,
                text,
                spans,
                links,
                presentation,
                selection,
                formulae,
            })
        }
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
        21 => {
            let title = reader.string()?;
            let value = if reader.u8()? == 0 {
                None
            } else {
                Some(reader.string()?)
            };
            Ok(Node::Section { id, title, value })
        }
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
        24 => {
            let action = ActionId(reader.u32()?);
            if action.is_reserved() {
                return Err(ProtocolError::InvalidValue("reserved action id"));
            }
            let value = reader.string()?;
            let placeholder = reader.string()?;
            let clear = match reader.u8()? {
                0 => None,
                1 => {
                    let clear = ActionId(reader.u32()?);
                    if clear.is_reserved() {
                        return Err(ProtocolError::InvalidValue("reserved action id"));
                    }
                    Some(clear)
                }
                _ => return Err(ProtocolError::InvalidValue("field clear flag")),
            };
            Ok(Node::Field {
                id,
                action,
                value,
                placeholder,
                clear,
            })
        }
        25 => {
            let count_of_chips = usize::from(reader.u8()?);
            if count_of_chips > kobo_ui::MAX_CHIPS {
                return Err(ProtocolError::TooManyNodes);
            }
            let mut chips = Vec::with_capacity(count_of_chips);
            for _ in 0..count_of_chips {
                let action = ActionId(reader.u32()?);
                if action.is_reserved() {
                    return Err(ProtocolError::InvalidValue("reserved action id"));
                }
                let selected = match reader.u8()? {
                    0 => false,
                    1 => true,
                    _ => return Err(ProtocolError::InvalidValue("chip selected flag")),
                };
                chips.push(kobo_ui::Chip {
                    action,
                    label: reader.string()?,
                    selected,
                });
            }
            Ok(Node::Chips { id, chips })
        }
        26 => {
            let count_of_tabs = usize::from(reader.u8()?);
            if count_of_tabs > kobo_ui::MAX_TABS {
                return Err(ProtocolError::TooManyNodes);
            }
            let selected = usize::from(reader.u8()?);
            let mut tabs = Vec::with_capacity(count_of_tabs);
            for _ in 0..count_of_tabs {
                let action = ActionId(reader.u32()?);
                if action.is_reserved() {
                    return Err(ProtocolError::InvalidValue("reserved action id"));
                }
                tabs.push(kobo_ui::Chip {
                    action,
                    label: reader.string()?,
                    selected: false,
                });
            }
            let selected = if selected < tabs.len() { selected } else { 0 };
            Ok(Node::Tabs { id, tabs, selected })
        }
        23 => {
            let count_of_entries = usize::from(reader.u16()?);
            if count_of_entries > kobo_ui::MAX_FACTS {
                return Err(ProtocolError::TooManyNodes);
            }
            let mut entries = Vec::with_capacity(count_of_entries);
            for _ in 0..count_of_entries {
                entries.push((reader.string()?, reader.string()?));
            }
            Ok(Node::Facts { id, entries })
        }
        22 => {
            let align = match reader.u8()? {
                1 => kobo_ui::BandAlign::Middle,
                2 => kobo_ui::BandAlign::Bottom,
                _ => kobo_ui::BandAlign::Top,
            };
            let count_of_slots = usize::from(reader.u8()?);
            if count_of_slots > kobo_ui::MAX_BAND_SLOTS {
                return Err(ProtocolError::TooManyNodes);
            }
            let mut slots = Vec::with_capacity(count_of_slots);
            for _ in 0..count_of_slots {
                let width = match reader.u8()? {
                    0 => kobo_ui::SlotWidth::Natural,
                    2 => kobo_ui::SlotWidth::Fixed(reader.u16()?),
                    _ => kobo_ui::SlotWidth::Fill,
                };
                let inside = usize::from(reader.u16()?);
                if inside > MAX_NODES {
                    return Err(ProtocolError::TooManyNodes);
                }
                let mut nodes = Vec::with_capacity(inside);
                for _ in 0..inside {
                    nodes.push(decode_node(reader, depth + 1, count)?);
                }
                slots.push(kobo_ui::BandSlot::new(width, nodes));
            }
            Ok(Node::Band { id, align, slots })
        }
        5 => Ok(Node::Divider { id }),
        27 => Ok(Node::Flex { id }),
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
                let state = match reader.u8()? {
                    0 => TileState::Normal,
                    1 => TileState::Held,
                    2 => TileState::Unavailable,
                    3 => TileState::Busy,
                    _ => return Err(ProtocolError::InvalidValue("tile state")),
                };
                let badge = reader.string()?;
                let subtitle = reader.string()?;
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
                    state,
                    badge,
                    subtitle,
                });
            }
            Ok(Node::TileGrid { id, tiles, shape })
        }
        17 => Ok(Node::Picture {
            id,
            handle: PictureHandle(reader.u32()?),
            source: (reader.u32()?, reader.u32()?),
            max_height_tenths_mm: reader.u16()?,
            framed: reader.u8()? != 0,
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
        30 => {
            let count = reader.u8()? as usize;
            let mut weights = Vec::with_capacity(count.min(MAX_NODES));
            for _ in 0..count {
                weights.push(reader.u16()?);
            }
            let count = reader.u8()? as usize;
            let mut rows = Vec::with_capacity(count.min(MAX_NODES));
            for _ in 0..count {
                let header = match reader.u8()? {
                    0 => false,
                    1 => true,
                    _ => return Err(ProtocolError::InvalidValue("table heading flag")),
                };
                let cells = reader.u8()? as usize;
                let mut row = Vec::with_capacity(cells.min(MAX_NODES));
                for _ in 0..cells {
                    row.push(reader.string()?);
                }
                rows.push(kobo_ui::TableRow { header, cells: row });
            }
            Ok(Node::Table { id, rows, weights })
        }
        29 => {
            let label = reader.string()?;
            let less = decode_bar_action(reader)?;
            let more = decode_bar_action(reader)?;
            let mut states = [ControlState::Enabled; 2];
            for state in &mut states {
                *state = match reader.u8()? {
                    0 => ControlState::Enabled,
                    1 => ControlState::Disabled,
                    _ => return Err(ProtocolError::InvalidValue("stepper control state")),
                };
            }
            let [less_state, more_state] = states;
            let fill = match reader.u8()? {
                0 => None,
                marked if marked <= 101 => Some(marked - 1),
                _ => return Err(ProtocolError::InvalidValue("stepper fill")),
            };
            Ok(Node::Stepper {
                id,
                label,
                less,
                more,
                less_state,
                more_state,
                fill,
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
        20 => {
            let glyph = match reader.u8()? {
                0 => None,
                1 => Some(
                    decode_glyph(reader.u8()?)
                        .ok_or(ProtocolError::InvalidValue("splash glyph"))?,
                ),
                _ => return Err(ProtocolError::InvalidValue("splash glyph flag")),
            };
            Ok(Node::Splash {
                id,
                glyph,
                title: reader.string()?,
                summary: reader.string()?,
            })
        }
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
            let transferred = match reader.u8()? {
                0 => None,
                1 => {
                    let received = reader.u64()?;
                    let total = match reader.u8()? {
                        0 => None,
                        1 => Some(reader.u64()?),
                        _ => return Err(ProtocolError::InvalidValue("activity total flag")),
                    };
                    Some((received, total))
                }
                _ => return Err(ProtocolError::InvalidValue("activity transfer flag")),
            };
            let failure = match reader.u8()? {
                0 => None,
                1 => {
                    let resumable = match reader.u8()? {
                        0 => false,
                        1 => true,
                        _ => return Err(ProtocolError::InvalidValue("activity resumable flag")),
                    };
                    Some(TransferFailure {
                        resumable,
                        reason: reader.string()?,
                    })
                }
                _ => return Err(ProtocolError::InvalidValue("activity failure flag")),
            };
            Ok(Node::Activity {
                id,
                label,
                progress,
                cancel,
                transferred,
                failure,
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
                let label = reader.string()?;
                let cell = Cell::new(action, label);
                cells.push(match reader.u8()? {
                    0 => cell,
                    1 => cell.with_glyph(
                        decode_glyph(reader.u8()?).ok_or(ProtocolError::InvalidValue("glyph"))?,
                    ),
                    _ => return Err(ProtocolError::InvalidValue("cell glyph flag")),
                });
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
                let trailing = if reader.u8()? == 0 {
                    None
                } else {
                    Some(reader.string()?)
                };
                let menu = if reader.u8()? == 0 {
                    None
                } else {
                    let menu = ActionId(reader.u32()?);
                    if menu.is_reserved() {
                        return Err(ProtocolError::InvalidValue("reserved action id"));
                    }
                    Some(menu)
                };
                rows.push(Row {
                    action,
                    title,
                    summary,
                    lead,
                    state,
                    trailing,
                    menu,
                });
            }
            Ok(Node::Rows { id, rows })
        }
        _ => Err(ProtocolError::InvalidValue("node tag")),
    }
}

/// A row's lead is always [`ROW_LEAD_LEN`] bytes: a tag, a sixteen-bit value,
/// and a picture payload that is written as zeroes when the lead is not one.
///
/// Fixed width rather than variable, because `encoded_screen_len` has to
/// predict the size of every screen before a byte is written, and a length
/// that depends on which variant a row happens to carry is exactly the kind of
/// arithmetic that has already produced one `debug_assert` panic in this file.
/// Thirteen wasted bytes per row is the price of keeping that prediction a
/// constant, and it is a price worth paying twice.
fn push_row_lead(output: &mut Vec<u8>, lead: RowLead) {
    let before = output.len();
    match lead {
        RowLead::Icon(glyph) => {
            output.push(0);
            push_u16(output, u16::from(encode_glyph(glyph)));
        }
        RowLead::Number(number) => {
            output.push(1);
            push_u16(output, number);
        }
        RowLead::Picture(picture, glyph) => {
            output.push(2);
            push_u16(output, u16::from(encode_glyph(glyph)));
            push_u32(output, picture.handle.0);
            push_u32(output, picture.source.0);
            push_u32(output, picture.source.1);
        }
    }
    output.resize(before + ROW_LEAD_LEN, 0);
    debug_assert_eq!(output.len(), before + ROW_LEAD_LEN);
}

/// The fixed width of an encoded row lead.
const ROW_LEAD_LEN: usize = 15;

fn read_row_lead(reader: &mut Reader<'_>) -> Result<RowLead, ProtocolError> {
    let tag = reader.u8()?;
    let value = reader.u16()?;
    let glyph = || {
        u8::try_from(value)
            .ok()
            .and_then(decode_glyph)
            .ok_or(ProtocolError::InvalidValue("row glyph"))
    };
    let lead = match tag {
        0 => RowLead::Icon(glyph()?),
        1 => RowLead::Number(value),
        2 => {
            let handle = PictureHandle(reader.u32()?);
            let source = (reader.u32()?, reader.u32()?);
            RowLead::Picture(TilePicture { handle, source }, glyph()?)
        }
        _ => return Err(ProtocolError::InvalidValue("row lead")),
    };
    // The padding the encoder wrote, so every lead costs the same however it
    // was built.
    let written = if tag == 2 { 15 } else { 3 };
    for _ in written..ROW_LEAD_LEN {
        reader.u8()?;
    }
    Ok(lead)
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

/// Writes a request body, which is length-prefixed with four bytes rather than
/// two because a body is allowed to be far larger than a label.
fn push_long_string(output: &mut Vec<u8>, text: &str) -> Result<(), ProtocolError> {
    if text.len() > MAX_POST_BODY_LEN {
        return Err(ProtocolError::StringTooLarge);
    }
    push_u32(
        output,
        u32::try_from(text.len()).map_err(|_| ProtocolError::StringTooLarge)?,
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

/// A present flag then the value, which is how every optional field on the
/// wire is written. Three small helpers rather than one generic one, because
/// the alternative is a trait bound to read for the sake of nine call sites.
fn push_optional_u8(output: &mut Vec<u8>, value: Option<u8>) {
    output.push(u8::from(value.is_some()));
    output.push(value.unwrap_or(0));
}

fn push_optional_i32(output: &mut Vec<u8>, value: Option<i32>) {
    output.push(u8::from(value.is_some()));
    output.extend_from_slice(&value.unwrap_or(0).to_be_bytes());
}

fn push_optional_string(output: &mut Vec<u8>, text: Option<&str>) -> Result<(), ProtocolError> {
    match text {
        None => {
            output.push(0);
            Ok(())
        }
        Some(text) => {
            output.push(1);
            push_string(output, text)
        }
    }
}

fn push_u64(output: &mut Vec<u8>, value: u64) {
    output.extend_from_slice(&value.to_be_bytes());
}

/// What a present flag is called when it decodes to something other than 0 or 1.
const PRESENT: &str = "optional field flag";

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

    fn optional_u8(&mut self) -> Result<Option<u8>, ProtocolError> {
        let present = read_boolean(self, PRESENT)?;
        let value = self.u8()?;
        Ok(present.then_some(value))
    }

    fn optional_i32(&mut self) -> Result<Option<i32>, ProtocolError> {
        let present = read_boolean(self, PRESENT)?;
        let bytes = self.take(4)?;
        let value = i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        Ok(present.then_some(value))
    }

    fn optional_string(&mut self) -> Result<Option<String>, ProtocolError> {
        if read_boolean(self, PRESENT)? {
            self.string().map(Some)
        } else {
            Ok(None)
        }
    }

    fn u64(&mut self) -> Result<u64, ProtocolError> {
        let bytes = self.take(8)?;
        let mut octets = [0u8; 8];
        octets.copy_from_slice(bytes);
        Ok(u64::from_be_bytes(octets))
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

    fn long_string(&mut self) -> Result<String, ProtocolError> {
        let length = usize::try_from(self.u32()?).map_err(|_| ProtocolError::StringTooLarge)?;
        if length > MAX_POST_BODY_LEN {
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
    fn every_glyph_has_a_wire_tag_and_gets_it_back() {
        // A glyph added to the enum without a tag here encodes as whatever the
        // match arm above it chose, so a book would arrive as a battery. The
        // encoder is exhaustive so that half is caught by the compiler; the
        // decoder is a numeric table and is not, which is the half this covers.
        for glyph in Glyph::ALL {
            let tag = encode_glyph(glyph);
            assert_eq!(
                decode_glyph(tag),
                Some(glyph),
                "{glyph:?} encoded as {tag} and came back as something else"
            );
        }
        let tags: Vec<u8> = Glyph::ALL.iter().copied().map(encode_glyph).collect();
        let mut unique = tags.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), tags.len(), "two glyphs share one wire tag");
        assert_eq!(
            decode_glyph(u8::try_from(Glyph::ALL.len()).expect("small set")),
            None,
            "a tag one past the end decoded as a glyph"
        );
    }

    #[test]
    fn every_device_request_round_trips() {
        let requests = vec![
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
            DeviceRequest::ReadBluetooth,
            DeviceRequest::SetBluetooth { enabled: true },
            DeviceRequest::ScanBluetooth,
            DeviceRequest::PairBluetooth {
                address: "AA:BB:CC:DD:EE:FF".to_owned(),
            },
            DeviceRequest::ConnectBluetooth {
                address: "AA:BB:CC:DD:EE:FF".to_owned(),
            },
            DeviceRequest::DisconnectBluetooth {
                address: "AA:BB:CC:DD:EE:FF".to_owned(),
            },
            DeviceRequest::ForgetBluetooth {
                address: "AA:BB:CC:DD:EE:FF".to_owned(),
            },
            DeviceRequest::ReadWifi,
            DeviceRequest::SetWifi { enabled: false },
            DeviceRequest::ScanWifi,
            DeviceRequest::JoinWifi {
                ssid: "Library".to_owned(),
                password: "readmore".to_owned(),
            },
            DeviceRequest::DisconnectWifi,
            DeviceRequest::ReadAudio,
            DeviceRequest::LoadAudio {
                source: AudioSource::Shelf("audiobook.mp3z".to_owned()),
            },
            DeviceRequest::LoadAudio {
                source: AudioSource::Stream("https://example.com/book.mp3".to_owned()),
            },
            DeviceRequest::PlayAudio,
            DeviceRequest::PauseAudio,
            DeviceRequest::SeekAudio {
                position_ms: 120_000,
            },
            DeviceRequest::StopAudio,
            DeviceRequest::SetAudioVolume { percent: 65 },
            DeviceRequest::ReadBatteryDetail,
            DeviceRequest::ReadCover,
            DeviceRequest::Update {
                url: "https://github.com/o/r/releases/download/v0.1.1/KoboRoot.tgz".to_owned(),
                sha256: "a".repeat(64),
            },
            DeviceRequest::ListInstalledApps,
            DeviceRequest::ReadAppCatalog,
            DeviceRequest::RefreshAppCatalog,
            DeviceRequest::InstallApp {
                id: "word-count".to_owned(),
            },
            DeviceRequest::UninstallApp {
                id: "word-count".to_owned(),
            },
            DeviceRequest::ReadAppLink,
            DeviceRequest::BeginAppLink,
            DeviceRequest::PollAppLink,
            DeviceRequest::DisconnectAppLink,
            DeviceRequest::ReadIdentity,
            DeviceRequest::ReadAutoUpdate,
            DeviceRequest::SetAutoUpdate {
                cobalt: true,
                apps: false,
            },
            DeviceRequest::SetAutoUpdate {
                cobalt: false,
                apps: true,
            },
            DeviceRequest::ReadUpdateChannel,
            DeviceRequest::SetUpdateChannel {
                channel: UpdateChannel::Stable,
            },
            DeviceRequest::SetUpdateChannel {
                channel: UpdateChannel::Beta,
            },
            DeviceRequest::ReadIdleSleep,
            DeviceRequest::SetIdleSleep(IdleSleep::FiveMinutes),
            DeviceRequest::SetIdleSleep(IdleSleep::Never),
        ];
        for request in requests {
            let frame = Frame {
                request_id: 9,
                message: Message::DeviceRequest(request),
            };
            let bytes = encode(&frame).expect("encode");
            assert_eq!(decode(&bytes).expect("decode"), frame);
        }
    }

    #[test]
    fn idle_sleep_tokens_that_are_not_a_choice_become_five_minutes() {
        assert_eq!(IdleSleep::from_stored("5"), IdleSleep::FiveMinutes);
        assert_eq!(IdleSleep::from_stored(" 10 \n"), IdleSleep::TenMinutes);
        assert_eq!(IdleSleep::from_stored("never"), IdleSleep::Never);
        assert_eq!(IdleSleep::from_stored("1"), IdleSleep::OneMinute);
        assert_eq!(IdleSleep::from_stored("30"), IdleSleep::ThirtyMinutes);
        assert_eq!(IdleSleep::from_stored(""), IdleSleep::FiveMinutes);
        assert_eq!(IdleSleep::from_stored("15"), IdleSleep::FiveMinutes);
        assert_eq!(IdleSleep::Never.minutes(), None);
        assert_eq!(IdleSleep::FiveMinutes.minutes(), Some(5));
        assert_eq!(IdleSleep::default(), IdleSleep::FiveMinutes);
    }

    #[test]
    fn an_update_that_could_not_be_verified_is_refused_at_the_encoder() {
        let cases = [
            // Plaintext could be rewritten in flight, digest or no digest.
            ("http://github.com/a.tgz".to_owned(), "a".repeat(64)),
            // A digest of the wrong length matches nothing.
            ("https://github.com/a.tgz".to_owned(), "a".repeat(63)),
            // Uppercase is somebody's formatting, not this wire's.
            (
                "https://github.com/a.tgz".to_owned(),
                format!("{}F", "a".repeat(63)),
            ),
        ];
        for (url, sha256) in cases {
            let frame = Frame {
                request_id: 9,
                message: Message::DeviceRequest(DeviceRequest::Update {
                    url: url.clone(),
                    sha256,
                }),
            };
            assert!(encode(&frame).is_err(), "{url} was encoded");
        }
    }

    #[test]
    fn every_device_result_round_trips() {
        let results = vec![
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
            DeviceResult::Bluetooth {
                available: true,
                enabled: true,
                devices: vec![BluetoothDevice {
                    address: "AA:BB:CC:DD:EE:FF".to_owned(),
                    name: "Headphones".to_owned(),
                    kind: BluetoothDeviceKind::Audio,
                    paired: true,
                    connected: true,
                }],
                restart_on_exit: false,
            },
            DeviceResult::Bluetooth {
                available: true,
                enabled: true,
                devices: Vec::new(),
                restart_on_exit: true,
            },
            DeviceResult::Wifi {
                available: true,
                enabled: true,
                connected_ssid: Some("Library".to_owned()),
                networks: vec![WifiNetwork {
                    ssid: "Library".to_owned(),
                    signal_dbm: -48,
                    secured: true,
                    connected: true,
                }],
            },
            DeviceResult::Audio {
                available: true,
                state: AudioPlaybackState::Playing,
                position_ms: 75_000,
                duration_ms: 600_000,
                volume: 70,
            },
            DeviceResult::Failed(DeviceError::Authentication),
            DeviceResult::Failed(DeviceError::Integrity),
            DeviceResult::BatteryDetail(BatteryDetail {
                percent: Some(28),
                status: Some("Discharging".to_owned()),
                health: Some("Good".to_owned()),
                technology: Some("Li-ion".to_owned()),
                decidegrees: Some(290),
                microvolts: Some(3_720_000),
                microamps: Some(-180_000),
                charge_now: Some(420_000),
                charge_full: Some(1_480_000),
                charge_full_design: Some(1_500_000),
            }),
            DeviceResult::BatteryDetail(BatteryDetail::default()),
            DeviceResult::Cover {
                available: true,
                magnet_present: true,
            },
            DeviceResult::Cover {
                available: false,
                magnet_present: false,
            },
            DeviceResult::IdleSleep(IdleSleep::FiveMinutes),
            DeviceResult::IdleSleep(IdleSleep::Never),
            DeviceResult::Apps {
                entries: vec![AppInfo {
                    id: "word-count".to_owned(),
                    title: "Word Count".to_owned(),
                    label: "Words".to_owned(),
                    summary: "Counts words in a note.".to_owned(),
                    version: "1.2.0".to_owned(),
                    minimum_cobalt_version: "0.3.0".to_owned(),
                    glyph: Glyph::Note,
                    capabilities: vec!["shared-files".to_owned()],
                    installed_version: Some("1.1.0".to_owned()),
                }],
            },
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
    fn every_auto_update_result_round_trips() {
        for cobalt in [false, true] {
            for apps in [false, true] {
                let frame = Frame {
                    request_id: 11,
                    message: Message::DeviceResult(DeviceResult::AutoUpdate { cobalt, apps }),
                };
                let bytes = encode(&frame).expect("encode");
                assert_eq!(decode(&bytes).expect("decode"), frame);
            }
        }
    }

    #[test]
    fn protocol_11_auto_update_frames_remain_byte_compatible() {
        let read = Frame {
            request_id: 8,
            message: Message::DeviceRequest(DeviceRequest::ReadAutoUpdate),
        };
        let read_bytes = encode(&read).expect("encode read");
        assert_eq!(
            read_bytes,
            [b'K', b'O', b'B', b'O', 11, 7, 0, 0, 0, 1, 0, 0, 0, 8, 43,]
        );
        assert_eq!(decode(&read_bytes).expect("decode read"), read);

        let request = Frame {
            request_id: 9,
            message: Message::DeviceRequest(DeviceRequest::SetAutoUpdate {
                cobalt: true,
                apps: false,
            }),
        };
        let request_bytes = encode(&request).expect("encode request");
        assert_eq!(
            request_bytes,
            [b'K', b'O', b'B', b'O', 11, 7, 0, 0, 0, 2, 0, 0, 0, 9, 44, 1,]
        );
        assert_eq!(decode(&request_bytes).expect("decode request"), request);

        let result = Frame {
            request_id: 11,
            message: Message::DeviceResult(DeviceResult::AutoUpdate {
                cobalt: false,
                apps: true,
            }),
        };
        let result_bytes = encode(&result).expect("encode result");
        assert_eq!(
            result_bytes,
            [b'K', b'O', b'B', b'O', 11, 8, 0, 0, 0, 2, 0, 0, 0, 11, 17, 2,]
        );
        assert_eq!(decode(&result_bytes).expect("decode result"), result);
    }

    #[test]
    fn update_channel_variants_round_trip() {
        for channel in [UpdateChannel::Stable, UpdateChannel::Beta] {
            for message in [
                Message::DeviceRequest(DeviceRequest::SetUpdateChannel { channel }),
                Message::DeviceResult(DeviceResult::UpdateChannel(channel)),
            ] {
                let frame = Frame {
                    request_id: 9,
                    message,
                };
                let bytes = encode(&frame).expect("encode");
                assert_eq!(decode(&bytes).expect("decode"), frame);
            }
        }
        let read = Frame {
            request_id: 9,
            message: Message::DeviceRequest(DeviceRequest::ReadUpdateChannel),
        };
        let bytes = encode(&read).expect("encode");
        assert_eq!(decode(&bytes).expect("decode"), read);
    }

    #[test]
    fn unknown_update_channels_are_refused() {
        let request = Frame {
            request_id: 9,
            message: Message::DeviceRequest(DeviceRequest::SetUpdateChannel {
                channel: UpdateChannel::Stable,
            }),
        };
        let mut bytes = encode(&request).expect("encode request");
        *bytes.last_mut().expect("channel byte") = 2;
        assert_eq!(
            decode(&bytes),
            Err(ProtocolError::InvalidValue("update channel"))
        );

        let result = Frame {
            request_id: 9,
            message: Message::DeviceResult(DeviceResult::UpdateChannel(UpdateChannel::Stable)),
        };
        let mut bytes = encode(&result).expect("encode result");
        *bytes.last_mut().expect("channel byte") = 2;
        assert_eq!(
            decode(&bytes),
            Err(ProtocolError::InvalidValue("update channel"))
        );
    }

    #[test]
    fn identity_results_round_trip() {
        let results = [
            DeviceResult::Identity(DeviceIdentity {
                profile_id: "CLARA_BW_391".to_owned(),
                model: "Kobo Clara BW".to_owned(),
                device_code: 391,
                firmware: "4.41.23145".to_owned(),
                kernel: "5.10.117".to_owned(),
                runtime_version: "0.1.0".to_owned(),
                panel_width: 1072,
                panel_height: 1448,
            }),
            DeviceResult::Identity(DeviceIdentity::default()),
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
    fn oversized_identity_field_is_refused() {
        let identity = DeviceIdentity {
            model: "x".repeat(MAX_IDENTITY_FIELD_LEN + 1),
            ..DeviceIdentity::default()
        };
        let frame = Frame {
            request_id: 11,
            message: Message::DeviceResult(DeviceResult::Identity(identity)),
        };
        assert!(encode(&frame).is_err());
    }

    #[test]
    fn every_app_link_result_round_trips() {
        let results = [
            DeviceResult::AppLink(AppLinkState::Unpaired),
            DeviceResult::AppLink(AppLinkState::Pairing {
                code: "2345ABCD".to_owned(),
                url: "https://store.example/link".to_owned(),
                expires_in: 600,
            }),
            DeviceResult::AppLink(AppLinkState::Paired { browsers: 8 }),
            DeviceResult::RemoteInstall(RemoteInstallOutcome::None),
            DeviceResult::RemoteInstall(RemoteInstallOutcome::Installed {
                id: "word-count".to_owned(),
            }),
            DeviceResult::RemoteInstall(RemoteInstallOutcome::Updated {
                id: "word-count".to_owned(),
            }),
            DeviceResult::RemoteInstall(RemoteInstallOutcome::AlreadyInstalled {
                id: "word-count".to_owned(),
            }),
            DeviceResult::RemoteInstall(RemoteInstallOutcome::Included {
                id: "word-count".to_owned(),
            }),
            DeviceResult::RemoteInstall(RemoteInstallOutcome::Unavailable {
                id: "word-count".to_owned(),
            }),
            DeviceResult::RemoteInstall(RemoteInstallOutcome::RequiresCobalt {
                id: "word-count".to_owned(),
                minimum_cobalt_version: "0.4.0".to_owned(),
            }),
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
    fn app_requests_and_results_are_bounded_and_validated() {
        let ids = vec![
            String::new(),
            "../todo".to_owned(),
            "Todo".to_owned(),
            "a/b".to_owned(),
            "1todo".to_owned(),
            "-todo".to_owned(),
            "todo-".to_owned(),
            "todo--list".to_owned(),
            "a".repeat(MAX_APP_ID_LEN + 1),
        ];
        for id in ids {
            let frame = Frame {
                request_id: 1,
                message: Message::DeviceRequest(DeviceRequest::InstallApp { id: id.clone() }),
            };
            assert!(encode(&frame).is_err(), "{id:?} was accepted as an app id");
        }
        for id in ["a", "todo", "todo-2", "a1-b2"] {
            assert!(valid_app_id(id), "{id:?} was rejected as an app id");
        }

        let too_many = DeviceResult::Apps {
            entries: (0..=MAX_APP_CATALOG_ENTRIES)
                .map(|index| AppInfo {
                    id: format!("app-{index}"),
                    title: "App".to_owned(),
                    label: "App".to_owned(),
                    summary: "One application.".to_owned(),
                    version: "1.0.0".to_owned(),
                    minimum_cobalt_version: "0.3.0".to_owned(),
                    glyph: Glyph::App,
                    capabilities: Vec::new(),
                    installed_version: None,
                })
                .collect(),
        };
        assert!(encode(&Frame {
            request_id: 1,
            message: Message::DeviceResult(too_many),
        })
        .is_err());

        let app = |version: String| AppInfo {
            id: "version-test".to_owned(),
            title: "Version Test".to_owned(),
            label: "Version".to_owned(),
            summary: "Checks the application version wire bound.".to_owned(),
            version,
            minimum_cobalt_version: "0.3.0".to_owned(),
            glyph: Glyph::App,
            capabilities: Vec::new(),
            installed_version: None,
        };
        assert!(encode(&Frame {
            request_id: 1,
            message: Message::DeviceResult(DeviceResult::Apps {
                entries: vec![app("a".repeat(MAX_APP_VERSION_LEN))],
            }),
        })
        .is_ok());
        assert!(encode(&Frame {
            request_id: 1,
            message: Message::DeviceResult(DeviceResult::Apps {
                entries: vec![app("a".repeat(MAX_APP_VERSION_LEN + 1))],
            }),
        })
        .is_err());
    }

    #[test]
    fn app_compatibility_uses_numeric_cobalt_versions() {
        let app = AppInfo {
            id: "version-test".to_owned(),
            title: "Version Test".to_owned(),
            label: "Version".to_owned(),
            summary: "Checks minimum Cobalt versions.".to_owned(),
            version: "1.0.0".to_owned(),
            minimum_cobalt_version: "0.2.4".to_owned(),
            glyph: Glyph::App,
            capabilities: Vec::new(),
            installed_version: None,
        };
        assert!(app.is_compatible_with("0.3.0"));
        assert!(app.is_compatible_with("0.3"));
        assert!(!app.is_compatible_with("0.2.3"));
        assert!(!app.is_compatible_with("nightly"));

        let newer = AppInfo {
            minimum_cobalt_version: "0.4.0".to_owned(),
            ..app
        };
        assert!(!newer.is_compatible_with("0.3.0"));
    }

    #[test]
    fn app_updates_are_only_strictly_newer_numeric_versions() {
        let app = |published: &str, installed: &str| AppInfo {
            id: "version-test".to_owned(),
            title: "Version Test".to_owned(),
            label: "Version".to_owned(),
            summary: "Checks Store update ordering.".to_owned(),
            version: published.to_owned(),
            minimum_cobalt_version: "0.3.2".to_owned(),
            glyph: Glyph::App,
            capabilities: Vec::new(),
            installed_version: Some(installed.to_owned()),
        };
        assert!(app("1.2.0", "1.1.9").has_update());
        assert!(!app("1.2.0", "1.2.0").has_update());
        assert!(!app("1.1.9", "1.2.0").has_update());
        assert!(!app("stable", "1.2.0").has_update());
        assert!(!app("1.2.0", "beta").has_update());
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
    fn app_link_and_remote_install_payloads_are_bounded_and_validated() {
        let pairing = |code: String, url: String, expires_in| Frame {
            request_id: 1,
            message: Message::DeviceResult(DeviceResult::AppLink(AppLinkState::Pairing {
                code,
                url,
                expires_in,
            })),
        };
        for code in [
            "2345ABC".to_owned(),
            "2345ABCDE".to_owned(),
            "2345ABCI".to_owned(),
            "2345abcD".to_owned(),
        ] {
            assert!(encode(&pairing(code, "https://store.example/link".to_owned(), 60)).is_err());
        }
        assert!(encode(&pairing(
            "2345ABCD".to_owned(),
            "http://store.example/link".to_owned(),
            60
        ))
        .is_err());
        assert!(encode(&pairing(
            "2345ABCD".to_owned(),
            format!("https://{}", "a".repeat(MAX_URL_LEN)),
            60
        ))
        .is_err());
        assert!(encode(&pairing(
            "2345ABCD".to_owned(),
            "https://store.example/link".to_owned(),
            601
        ))
        .is_err());
        assert!(encode(&Frame {
            request_id: 1,
            message: Message::DeviceResult(DeviceResult::AppLink(AppLinkState::Paired {
                browsers: 9,
            })),
        })
        .is_err());
        assert!(encode(&Frame {
            request_id: 1,
            message: Message::DeviceResult(DeviceResult::RemoteInstall(
                RemoteInstallOutcome::Installed {
                    id: "../store".to_owned(),
                },
            )),
        })
        .is_err());
    }

    #[test]
    fn malformed_app_link_payloads_are_rejected() {
        let pairing = |code: String, url: String, expires_in| Frame {
            request_id: 1,
            message: Message::DeviceResult(DeviceResult::AppLink(AppLinkState::Pairing {
                code,
                url,
                expires_in,
            })),
        };
        let valid_pairing = encode(&pairing(
            "2345ABCD".to_owned(),
            "https://store.example/link".to_owned(),
            600,
        ))
        .expect("valid pairing");
        let mut bad_code = valid_pairing.clone();
        bad_code[HEADER_LEN + 6] = b'I';
        assert_eq!(
            decode(&bad_code),
            Err(ProtocolError::InvalidValue("application link"))
        );
        let mut bad_expiry = valid_pairing;
        let last = bad_expiry.len() - 1;
        bad_expiry[last] = 0x59;
        assert_eq!(
            decode(&bad_expiry),
            Err(ProtocolError::InvalidValue("application link"))
        );

        let mut bad_url = encode(&pairing(
            "2345ABCD".to_owned(),
            "https://store.example/link".to_owned(),
            600,
        ))
        .expect("valid pairing");
        bad_url[HEADER_LEN + 18] = b'x';
        assert_eq!(
            decode(&bad_url),
            Err(ProtocolError::InvalidValue("application link"))
        );

        let mut bad_browsers = encode(&Frame {
            request_id: 1,
            message: Message::DeviceResult(DeviceResult::AppLink(AppLinkState::Paired {
                browsers: 8,
            })),
        })
        .expect("valid paired state");
        let last = bad_browsers.len() - 1;
        bad_browsers[last] = 9;
        assert_eq!(
            decode(&bad_browsers),
            Err(ProtocolError::InvalidValue("application link"))
        );

        let mut bad_id = encode(&Frame {
            request_id: 1,
            message: Message::DeviceResult(DeviceResult::RemoteInstall(
                RemoteInstallOutcome::Installed {
                    id: "word-count".to_owned(),
                },
            )),
        })
        .expect("valid outcome");
        bad_id[HEADER_LEN + 6] = b'W';
        assert_eq!(
            decode(&bad_id),
            Err(ProtocolError::InvalidValue("application id"))
        );

        let mut bad_outcome = encode(&Frame {
            request_id: 1,
            message: Message::DeviceResult(DeviceResult::RemoteInstall(RemoteInstallOutcome::None)),
        })
        .expect("valid empty outcome");
        let last = bad_outcome.len() - 1;
        bad_outcome[last] = 8;
        assert_eq!(
            decode(&bad_outcome),
            Err(ProtocolError::InvalidValue("remote install outcome"))
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
    fn a_reading_screens_middle_column_survives_the_wire() {
        // The length calculation is separate from the encoder, so a field
        // added to one and not the other produces a frame that encodes and
        // then refuses to decode. Both shapes are round-tripped for that
        // reason rather than only the new one.
        let plain = Screen::new(1, Vec::new()).with_page_turns(ActionId(11), ActionId(12));
        let with_menu = {
            let mut screen = plain.clone();
            screen.page_turns = screen
                .page_turns
                .map(|turns| turns.with_menu(ActionId(13)).with_position(4, 12));
            screen
        };
        for screen in [plain, with_menu] {
            let expected = screen.page_turns;
            let frame = Frame {
                request_id: 4,
                message: Message::SetScreen(screen),
            };
            let bytes = encode(&frame).expect("encodes");
            let back = decode(&bytes).expect("decodes");
            let Message::SetScreen(out) = back.message else {
                panic!("wrong message");
            };
            assert_eq!(out.page_turns, expected);
        }
    }

    #[test]
    fn a_held_finger_survives_the_wire() {
        // Same trap as the middle column: encoded_len is computed apart from
        // the encoder, so both shapes are round-tripped rather than only the
        // one carrying the new field.
        let plain = Screen::new(1, Vec::new());
        let holding = Screen::new(1, Vec::new()).with_hold(ActionId(21));
        for screen in [plain, holding] {
            let expected = screen.hold;
            let frame = Frame {
                request_id: 4,
                message: Message::SetScreen(screen),
            };
            let bytes = encode(&frame).expect("encodes");
            let back = decode(&bytes).expect("decodes");
            let Message::SetScreen(out) = back.message else {
                panic!("wrong message");
            };
            assert_eq!(out.hold, expected);
        }
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
                links: Vec::new(),
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
        // The full node budget with the longest string each node may carry:
        // 512 × 16 KiB is the frame limit exactly, and the envelope around
        // the strings takes it over.
        let nodes = (0..512)
            .map(|id| Node::Text {
                id: NodeId(id),
                text: "x".repeat(MAX_STRING_LEN),
                links: Vec::new(),
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
    use kobo_ui::{BandAlign, BandSlot, Chip};

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
            Node::Table {
                id: NodeId(70),
                rows: vec![
                    kobo_ui::TableRow {
                        header: true,
                        cells: vec!["Model".into(), "Top-1".into()],
                    },
                    kobo_ui::TableRow {
                        header: false,
                        cells: vec!["ResNet-50".into(), "76.1".into()],
                    },
                ],
                weights: vec![240, 90],
            },
            Node::Heading {
                id: NodeId(1),
                text: "Heading".into(),
                level: 1,
            },
            Node::Text {
                id: NodeId(2),
                text: "Body".into(),
                links: Vec::new(),
            },
            Node::RichText {
                id: NodeId(52),
                text: "Styled body".into(),
                spans: vec![kobo_ui::RichTextSpan {
                    start: 0,
                    end: 6,
                    presentation: kobo_ui::TextPresentation {
                        strong: true,
                        ..kobo_ui::TextPresentation::default()
                    },
                }],
                links: Vec::new(),
                presentation: kobo_ui::ParagraphPresentation {
                    alignment: kobo_ui::ParagraphAlignment::Center,
                    line_height_percent: 130,
                    ..kobo_ui::ParagraphPresentation::default()
                },
                selection: Some(kobo_ui::TextSelection {
                    context: 3,
                    offset: 11,
                }),
                formulae: vec![kobo_ui::InlineFormula {
                    start: 7,
                    end: 11,
                    handle: PictureHandle(9),
                    source: (40, 20),
                }],
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
                    links: Vec::new(),
                }],
            },
            Node::Divider { id: NodeId(6) },
            Node::Rows {
                id: NodeId(45),
                rows: vec![Row::new(
                    ActionId(67),
                    "Bleak House",
                    "Charles Dickens",
                    RowLead::Picture(
                        TilePicture {
                            handle: PictureHandle(9),
                            source: (190, 300),
                        },
                        Glyph::Book,
                    ),
                )],
            },
            Node::Field {
                id: NodeId(40),
                action: ActionId(60),
                value: "dickens".into(),
                placeholder: "Search the library".into(),
                clear: Some(ActionId(61)),
            },
            // Empty and with nothing to clear: both optional halves absent is
            // where a length mismatch hides.
            Node::Field {
                id: NodeId(41),
                action: ActionId(62),
                value: String::new(),
                placeholder: String::new(),
                clear: None,
            },
            Node::Chips {
                id: NodeId(42),
                chips: vec![
                    Chip::new(ActionId(63), "Fiction").selected(true),
                    Chip::new(ActionId(64), "History"),
                ],
            },
            Node::Chips {
                id: NodeId(43),
                chips: Vec::new(),
            },
            Node::Tabs {
                id: NodeId(44),
                tabs: vec![
                    Chip::new(ActionId(65), "Discover"),
                    Chip::new(ActionId(66), "Popular"),
                ],
                selected: 1,
            },
            Node::Facts {
                id: NodeId(31),
                entries: vec![
                    ("Downloads".into(), "94,206".into()),
                    ("Rights".into(), "Public domain in the USA".into()),
                    // An empty value is legal and must survive the wire.
                    ("Series".into(), String::new()),
                ],
            },
            // Every slot shape, because the width token is the part that
            // changes length on the wire and a fixed width is the only one
            // carrying a number after it.
            Node::Band {
                id: NodeId(25),
                align: BandAlign::Middle,
                slots: vec![
                    BandSlot::fixed(
                        300,
                        vec![Node::Text {
                            id: NodeId(26),
                            text: "Cover".into(),
                            links: Vec::new(),
                        }],
                    ),
                    BandSlot::fill(vec![
                        Node::Heading {
                            id: NodeId(27),
                            text: "Moby Dick".into(),
                            level: 1,
                        },
                        Node::Secondary {
                            id: NodeId(28),
                            text: "Herman Melville".into(),
                        },
                    ]),
                    BandSlot::natural(vec![Node::Secondary {
                        id: NodeId(29),
                        text: "32".into(),
                    }]),
                ],
            },
            // A slot holding nothing at all: the empty collection is where a
            // length mismatch hides.
            Node::Band {
                id: NodeId(30),
                align: BandAlign::Bottom,
                slots: vec![BandSlot::fill(Vec::new())],
            },
            // Both shapes of section: the value is the optional half, and an
            // optional half is exactly what a length mismatch hides in.
            Node::Section {
                id: NodeId(23),
                title: "Details".into(),
                value: None,
            },
            Node::Section {
                id: NodeId(24),
                title: "Popular".into(),
                value: Some("32".into()),
            },
            Node::Flex { id: NodeId(90) },
            Node::Spacer {
                id: NodeId(7),
                space: Space::Medium,
            },
            Node::Progress {
                id: NodeId(8),
                value: Percent::new(40),
            },
            Node::Stepper {
                id: NodeId(96),
                label: "120%".into(),
                less: BarAction::new(ActionId(30), String::new()).with_glyph(Glyph::Minus),
                more: BarAction::new(ActionId(31), String::new()).with_glyph(Glyph::Plus),
                less_state: ControlState::Enabled,
                more_state: ControlState::Disabled,
                fill: Some(75),
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
                    // A tile wearing every optional part at once, because the
                    // empty ones are exactly where a length mismatch hides.
                    Tile::new(ActionId(4), "Bleak House", Glyph::Book)
                        .with_state(TileState::Held)
                        .with_badge("12")
                        .with_subtitle("Charles Dickens"),
                ],
            },
            Node::Splash {
                id: NodeId(21),
                glyph: Some(Glyph::Book),
                title: "Gutenbird".into(),
                summary: "Sixty thousand free books.".into(),
            },
            // No mark, and nothing to say. Both halves are optional in
            // practice and the empty ones are what a length mismatch hides in.
            Node::Splash {
                id: NodeId(22),
                glyph: None,
                title: "Starting".into(),
                summary: String::new(),
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
                    // A trailing value is the optional half of a row, and an
                    // optional half is where a length mismatch hides.
                    Row::new(ActionId(9), "Great Expectations", "Dickens", Glyph::Book)
                        .with_trailing("18,204"),
                    // An overflow action is the other optional half, and the
                    // two together are where an ordering mistake hides.
                    Row::new(ActionId(10), "Ars Technica", "arstechnica.com", Glyph::Rss)
                        .with_menu(ActionId(11)),
                    Row::new(
                        ActionId(12),
                        "Hacker News",
                        "news.ycombinator.com",
                        Glyph::News,
                    )
                    .with_trailing("30")
                    .with_menu(ActionId(13)),
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
                transferred: Some((4_404_019, Some(11_534_336))),
                failure: Some(TransferFailure {
                    reason: "The connection was reset".into(),
                    resumable: true,
                }),
            },
            Node::Activity {
                id: NodeId(15),
                label: "Connecting".into(),
                progress: None,
                cancel: None,
                transferred: None,
                failure: None,
            },
            Node::Activity {
                id: NodeId(56),
                label: "Downloading".into(),
                progress: None,
                cancel: None,
                transferred: Some((512, None)),
                failure: Some(TransferFailure {
                    reason: "This edition is no longer published".into(),
                    resumable: false,
                }),
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
    fn a_grid_cell_carries_its_glyph_across_the_wire() {
        // The cell payload gained a flag byte after its label. An encoder that
        // wrote it and a decoder that did not would read the flag as the top
        // byte of the next cell's action, so the second button of a transport
        // row would fire something nobody named. VERSION went to 3 for this.
        let cells = vec![
            kobo_ui::Cell::new(ActionId(11), "Back 30 sec").with_glyph(Glyph::Rewind30),
            kobo_ui::Cell::new(ActionId(12), "Play"),
            kobo_ui::Cell::new(ActionId(13), "Louder").with_glyph(Glyph::VolumeUp),
        ];
        let screen = Screen::new(
            1,
            vec![Node::Grid {
                id: NodeId(1),
                columns: 3,
                square: false,
                cells: cells.clone(),
            }],
        );
        match round_trip(screen).nodes.first() {
            Some(Node::Grid { cells: back, .. }) => assert_eq!(back, &cells),
            other => panic!("expected a grid, got {other:?}"),
        }
    }

    #[test]
    fn a_reading_screen_keeps_its_publisher_font_handle() {
        let screen = Screen::new(
            1,
            vec![Node::Text {
                id: NodeId(1),
                text: "Publisher prose".into(),
                links: Vec::new(),
            }],
        )
        .with_reading(true)
        .with_reading_font(Some(FontHandle(42)));
        assert_eq!(round_trip(screen.clone()), screen);
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
                links: Vec::new(),
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

    /// A mark on a bar entry used to be a top bar privilege.
    ///
    /// The flag was encoded beside the top bar rather than inside the shared
    /// bar action, so a bottom bar could hold a glyph in memory, encode it,
    /// and arrive on the panel as a word. The launcher is where it showed:
    /// "Return to Kobo reader" in a slot a third of a panel wide.
    #[test]
    fn a_mark_on_any_bar_entry_survives_the_wire() {
        let screen = Screen::new(1, Vec::new())
            .with_nav_bar(NavBar::actions(
                NodeId(1),
                vec![
                    BarAction::new(ActionId(1), "Previous").with_glyph(Glyph::Previous),
                    BarAction::new(ActionId(2), "Reader"),
                    BarAction::new(ActionId(3), "More apps").with_glyph(Glyph::Next),
                ],
            ))
            .with_top_bar(TopBar::new(NodeId(2), "Cobalt"));
        let frame = Frame {
            request_id: 1,
            message: Message::SetScreen(screen.clone()),
        };
        let bytes = encode(&frame).expect("encode");
        // The reserved length and the encoder have to agree, and a mark that
        // is written but not counted is exactly how they stop agreeing.
        let counted = encoded_screen_len(&screen, 0, &mut 0).expect("length");
        assert_eq!(bytes.len(), HEADER_LEN + counted);
        let Message::SetScreen(back) = decode(&bytes).expect("decode").message else {
            panic!("expected a screen");
        };
        let marks = back
            .nav_bar
            .expect("nav bar")
            .destinations
            .iter()
            .map(|destination| destination.glyph)
            .collect::<Vec<_>>();
        assert_eq!(
            marks,
            vec![Some(Glyph::Previous), None, Some(Glyph::Next)],
            "a bar entry came back without the mark it was given"
        );
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
    fn a_text_hold_survives_the_wire_without_losing_its_range() {
        let message = Message::TextHold {
            action: ActionId(9),
            context: u64::MAX - 1,
            start: 41,
            end: 48,
        };
        assert_eq!(message_round_trip(message.clone()), message);
    }

    #[test]
    fn offline_dictionary_requests_and_entries_are_bounded_and_typed() {
        let request = Message::DeviceRequest(DeviceRequest::LookupWord {
            word: "café".into(),
            language: Some("fr".into()),
        });
        assert_eq!(message_round_trip(request.clone()), request);

        let result = Message::DeviceResult(DeviceResult::Dictionary {
            word: "café".into(),
            entries: vec![DictionaryEntry {
                dictionary: "Pocket French".into(),
                language: "fr".into(),
                headword: "café".into(),
                definition: "Établissement où l'on sert des boissons.".into(),
            }],
        });
        assert_eq!(message_round_trip(result.clone()), result);
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
    fn a_request_body_may_be_far_larger_than_a_label() {
        // The body used to be encoded as a label, so an application that
        // handed a model the research it had just fetched could not spawn the
        // request at all: encoding failed with `StringTooLarge` and the
        // application ended, having said nothing about why.
        let body = "x".repeat(64 * 1024);
        let message = Message::Spawn {
            task: TaskId(3),
            work: Task::Post {
                url: "https://example.invalid/v1".into(),
                body: body.clone(),
                content_type: "application/json".into(),
                credential: Some(Credential::bearer("openai")),
                headers: Vec::new(),
                max_bytes: 4096,
            },
        };
        let frame = Frame {
            request_id: 1,
            message,
        };
        let encoded = encode(&frame).expect("a 64 KiB body encodes");
        let decoded = decode(&encoded).expect("a 64 KiB body decodes");
        assert_eq!(decoded, frame);

        let oversized = Frame {
            request_id: 1,
            message: Message::Spawn {
                task: TaskId(3),
                work: Task::Post {
                    url: "https://example.invalid/v1".into(),
                    body: "x".repeat(MAX_POST_BODY_LEN + 1),
                    content_type: "application/json".into(),
                    credential: None,
                    headers: Vec::new(),
                    max_bytes: 4096,
                },
            },
        };
        assert_eq!(encode(&oversized), Err(ProtocolError::StringTooLarge));
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
                    credential: None,
                    headers: Vec::new(),
                },
            },
            Message::Spawn {
                task: TaskId(16),
                work: Task::Fetch {
                    url: "https://example.invalid/catalog".into(),
                    offset: 0,
                    max_bytes: 1024,
                    credential: None,
                    headers: vec![Header::new("Accept", "application/opds+json")],
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
    fn a_fetch_that_carries_headers_survives_an_encode_decode_round_trip() {
        let message = Message::Spawn {
            task: TaskId(20),
            work: Task::Fetch {
                url: "https://example.invalid/catalog".into(),
                offset: 0,
                max_bytes: 4096,
                credential: None,
                headers: vec![
                    Header::new("Accept", "application/opds+json"),
                    Header::new("If-None-Match", "\"abc123\""),
                ],
            },
        };
        assert_eq!(message_round_trip(message.clone()), message);
    }

    #[test]
    fn a_fetch_whose_header_count_exceeds_the_bound_is_refused_rather_than_truncated() {
        let headers = (0..=MAX_HEADERS)
            .map(|index| Header::new(format!("X-{index}"), "value"))
            .collect();
        let frame = Frame {
            request_id: 1,
            message: Message::Spawn {
                task: TaskId(21),
                work: Task::Fetch {
                    url: "https://example.invalid/catalog".into(),
                    offset: 0,
                    max_bytes: 4096,
                    credential: None,
                    headers,
                },
            },
        };
        assert_eq!(
            encode(&frame),
            Err(ProtocolError::InvalidValue("request header"))
        );
    }

    #[test]
    fn a_fetch_header_carrying_a_newline_in_its_name_or_value_is_refused() {
        // A newline in either would let an application append headers of its
        // own onto the wire, including ones the runtime never agreed to send.
        for header in [
            Header::new("X-Evil\r\nX-Injected", "value"),
            Header::new("X-Evil", "value\r\nX-Injected: 1"),
        ] {
            let frame = Frame {
                request_id: 1,
                message: Message::Spawn {
                    task: TaskId(22),
                    work: Task::Fetch {
                        url: "https://example.invalid/catalog".into(),
                        offset: 0,
                        max_bytes: 4096,
                        credential: None,
                        headers: vec![header],
                    },
                },
            };
            assert_eq!(
                encode(&frame),
                Err(ProtocolError::InvalidValue("request header"))
            );
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
            Message::CoverChanged {
                magnet_present: true,
            },
            Message::CoverChanged {
                magnet_present: false,
            },
            Message::PageTurn { forward: true },
            Message::PageTurn { forward: false },
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
        // Appended rather than inserted. The tags are the wire, and renumbering
        // them would make a new daemon and an older app disagree about what
        // went wrong without either of them noticing.
        TaskError::Offline => 5,
        TaskError::NoCredential => 6,
        TaskError::Unauthorized => 7,
    }
}

const fn decode_task_error(tag: u8) -> Result<TaskError, ProtocolError> {
    Ok(match tag {
        0 => TaskError::Denied,
        1 => TaskError::Unreachable,
        2 => TaskError::TooLarge,
        3 => TaskError::TimedOut,
        4 => TaskError::NotFound,
        5 => TaskError::Offline,
        6 => TaskError::NoCredential,
        7 => TaskError::Unauthorized,
        _ => return Err(ProtocolError::InvalidValue("task error")),
    })
}

#[cfg(test)]
mod task_error_tests {
    use super::{decode_task_error, encode_task_error, ProtocolError, TaskError};

    /// Every variant, so that adding one without a tag fails here.
    const EVERY: &[TaskError] = &[
        TaskError::Denied,
        TaskError::NoCredential,
        TaskError::Offline,
        TaskError::Unreachable,
        TaskError::TooLarge,
        TaskError::TimedOut,
        TaskError::NotFound,
    ];

    #[test]
    fn every_task_error_survives_the_wire() {
        for error in EVERY {
            assert_eq!(decode_task_error(encode_task_error(*error)), Ok(*error));
        }
    }

    #[test]
    fn no_two_task_errors_share_a_tag() {
        let mut tags: Vec<u8> = EVERY
            .iter()
            .map(|error| encode_task_error(*error))
            .collect();
        tags.sort_unstable();
        let count = tags.len();
        tags.dedup();
        assert_eq!(tags.len(), count, "two errors encode to the same tag");
    }

    #[test]
    fn the_tags_that_were_already_on_the_wire_did_not_move() {
        // An app built before Offline existed is still talking to this daemon
        // over these numbers. Renumbering would make the two disagree about
        // what went wrong without either of them noticing.
        assert_eq!(encode_task_error(TaskError::Denied), 0);
        assert_eq!(encode_task_error(TaskError::Unreachable), 1);
        assert_eq!(encode_task_error(TaskError::TooLarge), 2);
        assert_eq!(encode_task_error(TaskError::TimedOut), 3);
        assert_eq!(encode_task_error(TaskError::NotFound), 4);
        assert_eq!(encode_task_error(TaskError::Offline), 5);
        assert_eq!(encode_task_error(TaskError::NoCredential), 6);
    }

    /// The two refusals have to stay distinguishable in words as well as on
    /// the wire. A missing key blamed on the application sends whoever is
    /// holding the reader looking in entirely the wrong place.
    #[test]
    fn a_missing_key_and_a_refused_permission_do_not_share_a_sentence() {
        let denied = TaskError::Denied.to_string();
        let absent = TaskError::NoCredential.to_string();
        assert_ne!(denied, absent);
        assert!(absent.contains("key"), "{absent}");
        assert!(!TaskError::NoCredential.worth_retrying());
    }

    #[test]
    fn a_tag_from_the_future_is_refused_rather_than_guessed() {
        assert_eq!(
            decode_task_error(8),
            Err(ProtocolError::InvalidValue("task error"))
        );
        assert_eq!(
            decode_task_error(255),
            Err(ProtocolError::InvalidValue("task error"))
        );
    }

    #[test]
    fn only_the_failures_that_could_pass_next_time_are_worth_retrying() {
        assert!(TaskError::Offline.worth_retrying());
        assert!(TaskError::Unreachable.worth_retrying());
        assert!(TaskError::TimedOut.worth_retrying());
        // A permission does not appear on the second ask, a response does not
        // shrink, and a 404 is a working host giving a real answer.
        assert!(!TaskError::Denied.worth_retrying());
        assert!(!TaskError::TooLarge.worth_retrying());
        assert!(!TaskError::NotFound.worth_retrying());
    }

    #[test]
    fn the_two_network_failures_do_not_read_the_same() {
        // They ask opposite things of the person holding the reader: one is
        // answered by joining Wi-Fi, the other by waiting for a host.
        let offline = TaskError::Offline.to_string();
        let unreachable = TaskError::Unreachable.to_string();
        assert_ne!(offline, unreachable);
        assert!(offline.contains("not on a network"), "{offline}");
        assert!(unreachable.contains("host"), "{unreachable}");
    }
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
                    framed: true,
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
    fn a_publisher_font_travels_once_and_is_bounded() {
        let message = Message::PutFont {
            handle: FontHandle(7),
            name: "Book.otf".into(),
            bytes: b"OTTOfixture".to_vec(),
        };
        let frame = Frame {
            request_id: 1,
            message: message.clone(),
        };
        let bytes = encode(&frame).expect("encode font");
        assert_eq!(decode(&bytes).expect("decode font").message, message);

        let oversized = Frame {
            request_id: 1,
            message: Message::PutFont {
                handle: FontHandle(8),
                name: "huge.ttf".into(),
                bytes: vec![0; MAX_FONT_BYTES + 1],
            },
        };
        assert!(matches!(
            encode(&oversized),
            Err(ProtocolError::FrameTooLarge)
        ));
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

    #[test]
    fn orientation_request_round_trips_and_keeps_portrait_as_the_default() {
        let frame = Frame {
            request_id: 4,
            message: Message::SetOrientation(Orientation::Landscape),
        };
        assert_eq!(
            decode(&encode(&frame).expect("encode")).expect("decode"),
            frame
        );
        assert_eq!(Orientation::default(), Orientation::Portrait);
        assert_eq!(Orientation::Portrait as u8, 0);
    }

    #[test]
    fn a_basic_credential_survives_an_encode_decode_round_trip() {
        // A gated catalogue is reached by name, and the name is all an
        // application ever holds: the pair behind it is the runtime's.
        let work = Task::Post {
            url: "https://standardebooks.example/feeds/opds/all".to_owned(),
            body: String::new(),
            content_type: "application/x-www-form-urlencoded".to_owned(),
            credential: Some(Credential::basic("standardebooks")),
            headers: Vec::new(),
            max_bytes: 1024,
        };
        let frame = Frame {
            request_id: 1,
            message: Message::Spawn {
                task: TaskId(1),
                work: work.clone(),
            },
        };
        let bytes = encode(&frame).expect("encodes");
        let Message::Spawn { work: back, .. } = decode(&bytes).expect("decodes").message else {
            panic!("not a spawn");
        };
        assert_eq!(back, work);
    }
}

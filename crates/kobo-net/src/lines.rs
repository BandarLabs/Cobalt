//! Bounded, pull-driven HTTP line streams.
//!
//! The application protocol already carries `Fetch` requests and task
//! outcomes. A line stream therefore stays on protocol 11 by keeping the
//! socket in the runtime and letting the application ask for one complete
//! record at a time. Tokens never cross that boundary, blank SSE keepalives
//! never wake the panel, and an application exit drops every retained socket.

use super::{
    connect, content_length, head, header, transfer_is_chunked, Address, Held, Method,
    RequestOptions, MAX_HEADER_BYTES, MAX_RESPONSE_HEADERS, RESPONSE_TIMEOUT,
};
use kobo_protocol::TaskError;
use std::collections::HashMap;
use std::io::{ErrorKind, Read};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// One global event stream plus one active game stream.
pub const MAX_RETAINED_STREAMS: usize = 2;

/// One operation on a runtime-owned line stream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LineStreamAction {
    Open,
    Next,
    Close,
}

/// Open streams belonging to one application task runner.
///
/// Debug output deliberately reports only a count. URLs can contain private
/// game identifiers, and credentials are represented only by a digest.
pub struct LineStreams {
    state: Mutex<StreamState>,
    budget: Arc<StreamBudget>,
    connector: Arc<Connector>,
}

type Connector = dyn Fn(&Address, &dyn Fn() -> bool) -> Result<Held, TaskError> + Send + Sync;

impl Default for LineStreams {
    fn default() -> Self {
        Self {
            state: Mutex::new(StreamState::default()),
            budget: Arc::new(StreamBudget::default()),
            connector: Arc::new(connect),
        }
    }
}

impl std::fmt::Debug for LineStreams {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LineStreams")
            .field("open", &self.budget.in_use())
            .finish_non_exhaustive()
    }
}

#[derive(Default)]
struct StreamBudget {
    in_use: AtomicUsize,
}

impl StreamBudget {
    fn reserve(self: &Arc<Self>) -> Result<StreamLease, TaskError> {
        let mut current = self.in_use.load(Ordering::SeqCst);
        loop {
            if current >= MAX_RETAINED_STREAMS {
                return Err(TaskError::Denied);
            }
            match self.in_use.compare_exchange_weak(
                current,
                current + 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => {
                    return Ok(StreamLease {
                        budget: Arc::clone(self),
                    });
                }
                Err(observed) => current = observed,
            }
        }
    }

    fn in_use(&self) -> usize {
        self.in_use.load(Ordering::SeqCst)
    }
}

struct StreamLease {
    budget: Arc<StreamBudget>,
}

impl Drop for StreamLease {
    fn drop(&mut self) {
        self.budget.in_use.fetch_sub(1, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct StreamState {
    // Active I/O keeps an addressable generation here while the socket itself
    // is outside the mutex, so Close can invalidate it without blocking.
    streams: HashMap<String, StreamSlot>,
    next_generation: u64,
}

impl StreamState {
    fn generation(&mut self) -> u64 {
        self.next_generation = self.next_generation.wrapping_add(1);
        if self.next_generation == 0 {
            self.next_generation = 1;
        }
        self.next_generation
    }
}

enum StreamSlot {
    Opening {
        generation: u64,
        close: Arc<AtomicBool>,
    },
    Ready {
        generation: u64,
        stream: Box<LineStream>,
    },
    Reading {
        generation: u64,
        close: Arc<AtomicBool>,
    },
    Closed {
        generation: u64,
    },
}

impl StreamSlot {
    fn generation(&self) -> u64 {
        match self {
            Self::Opening { generation, .. }
            | Self::Ready { generation, .. }
            | Self::Reading { generation, .. }
            | Self::Closed { generation } => *generation,
        }
    }
}

#[derive(Clone, Copy)]
struct Cancellation<'a> {
    task: &'a AtomicBool,
    stream: &'a AtomicBool,
}

impl Cancellation<'_> {
    fn requested(self) -> bool {
        self.task.load(Ordering::SeqCst) || self.stream.load(Ordering::SeqCst)
    }
}

enum OpenResult {
    Retained(Box<LineStream>),
    Immediate(Vec<u8>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct StreamIdentity {
    credential: String,
    headers: String,
    format: Format,
    maximum: usize,
}

impl LineStreams {
    #[cfg(test)]
    fn with_connector(connector: Arc<Connector>) -> Self {
        Self {
            connector,
            ..Self::default()
        }
    }

    /// Performs one pull operation.
    ///
    /// `Open` completes after the response head has been authenticated and
    /// parsed, before waiting for an event. `Next` returns exactly one
    /// non-blank SSE event or NDJSON line. `Close` never needs the secret
    /// value and is safe after credential removal.
    ///
    /// # Errors
    ///
    /// Returns a bounded [`TaskError`] for policy-invalid headers, TLS or HTTP
    /// failures, malformed framing, cancellation, and oversized records.
    #[allow(
        clippy::too_many_arguments,
        reason = "the transport boundary keeps credential, headers, limits, options, and cancellation explicit"
    )]
    pub fn request(
        &self,
        action: LineStreamAction,
        url: &str,
        max_bytes: u32,
        credential: Option<(&str, &str)>,
        headers: &[(&str, &str)],
        options: RequestOptions,
        cancel: &AtomicBool,
    ) -> Result<Vec<u8>, TaskError> {
        match action {
            LineStreamAction::Open => {
                self.open(url, max_bytes, credential, headers, options, cancel)
            }
            LineStreamAction::Next => self.next(url, max_bytes, credential, headers, cancel),
            LineStreamAction::Close => self.close_controlled(url, cancel),
        }
    }

    /// Closes one retained stream, if present.
    pub fn close(&self, url: &str) {
        if let Ok(mut state) = self.state.lock() {
            Self::mark_closed(&mut state, url);
        }
    }

    fn close_controlled(&self, url: &str, cancel: &AtomicBool) -> Result<Vec<u8>, TaskError> {
        let mut state = self.state.lock().map_err(|_| TaskError::Unreachable)?;
        if cancel.load(Ordering::SeqCst) {
            return Err(TaskError::TimedOut);
        }
        Self::mark_closed(&mut state, url);
        Ok(Vec::new())
    }

    /// Drops every retained socket owned by this application runner.
    pub fn close_all(&self) {
        if let Ok(mut state) = self.state.lock() {
            for (_, slot) in state.streams.drain() {
                Self::interrupt(slot);
            }
        }
    }

    fn mark_closed(state: &mut StreamState, url: &str) {
        let Some(slot) = state.streams.remove(url) else {
            return;
        };
        match slot {
            StreamSlot::Opening { generation, close }
            | StreamSlot::Reading { generation, close } => {
                close.store(true, Ordering::SeqCst);
                state
                    .streams
                    .insert(url.to_owned(), StreamSlot::Closed { generation });
            }
            StreamSlot::Ready { .. } => {}
            closed @ StreamSlot::Closed { .. } => {
                state.streams.insert(url.to_owned(), closed);
            }
        }
    }

    fn interrupt(slot: StreamSlot) {
        match slot {
            StreamSlot::Opening { close, .. } | StreamSlot::Reading { close, .. } => {
                close.store(true, Ordering::SeqCst);
            }
            StreamSlot::Ready { .. } | StreamSlot::Closed { .. } => {}
        }
    }

    fn remove_generation(&self, url: &str, generation: u64) {
        if let Ok(mut state) = self.state.lock() {
            if state
                .streams
                .get(url)
                .is_some_and(|slot| slot.generation() == generation)
            {
                state.streams.remove(url);
            }
        }
    }

    fn open(
        &self,
        url: &str,
        max_bytes: u32,
        credential: Option<(&str, &str)>,
        headers: &[(&str, &str)],
        options: RequestOptions,
        cancel: &AtomicBool,
    ) -> Result<Vec<u8>, TaskError> {
        if max_bytes == 0 {
            return Err(TaskError::TooLarge);
        }
        if cancel.load(Ordering::SeqCst) {
            return Err(TaskError::TimedOut);
        }
        let identity = StreamIdentity {
            credential: credential_identity(credential),
            headers: headers_identity(headers),
            format: Format::from_headers(headers)?,
            maximum: max_bytes as usize,
        };
        let Some((generation, close, lease)) = self.reserve_open(url, &identity)? else {
            return Ok(Vec::new());
        };
        let cancellation = Cancellation {
            task: cancel,
            stream: &close,
        };
        let opened = self.establish(
            url,
            credential,
            headers,
            options,
            identity,
            lease,
            cancellation,
        );
        match opened {
            Err(error) => {
                self.remove_generation(url, generation);
                Err(error)
            }
            Ok(OpenResult::Immediate(bytes)) => {
                self.remove_generation(url, generation);
                Ok(bytes)
            }
            Ok(OpenResult::Retained(stream)) => {
                self.install_open(url, generation, stream, cancellation)
            }
        }
    }

    fn reserve_open(
        &self,
        url: &str,
        identity: &StreamIdentity,
    ) -> Result<Option<(u64, Arc<AtomicBool>, StreamLease)>, TaskError> {
        let mut state = self.state.lock().map_err(|_| TaskError::Unreachable)?;
        if state.streams.get(url).is_some_and(
            |slot| matches!(slot, StreamSlot::Ready { stream, .. } if stream.identity == *identity),
        ) {
            return Ok(None);
        }
        if let Some(slot) = state.streams.remove(url) {
            Self::interrupt(slot);
        }
        let lease = self.budget.reserve()?;
        let generation = state.generation();
        let close = Arc::new(AtomicBool::new(false));
        state.streams.insert(
            url.to_owned(),
            StreamSlot::Opening {
                generation,
                close: Arc::clone(&close),
            },
        );
        Ok(Some((generation, close, lease)))
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "opening retains the complete authenticated request identity and its one resource lease"
    )]
    fn establish(
        &self,
        url: &str,
        credential: Option<(&str, &str)>,
        headers: &[(&str, &str)],
        options: RequestOptions,
        identity: StreamIdentity,
        lease: StreamLease,
        cancellation: Cancellation<'_>,
    ) -> Result<OpenResult, TaskError> {
        if cancellation.requested() {
            return Err(TaskError::TimedOut);
        }
        let address = super::parse(url)?;
        let cancelled = || cancellation.requested();
        let mut held = (self.connector)(&address, &cancelled)?;
        if cancellation.requested() {
            return Err(TaskError::TimedOut);
        }
        let method = Method::Get {
            offset: None,
            credential,
            headers,
            streaming: true,
        };
        let response = response_head(&mut held, &address, &method, cancellation)?;
        let status = response.status;
        if status == 429 && options.report_rate_limit {
            return Ok(OpenResult::Immediate(super::rate_limit_envelope(
                response.retry_after,
            )));
        }
        match status {
            200..=299 => {}
            301..=303 | 307 | 308 => {
                return Err(if credential.is_some() {
                    TaskError::Denied
                } else {
                    TaskError::NotFound
                });
            }
            401 | 403 => return Err(TaskError::Unauthorized),
            400..=599 => return Err(TaskError::NotFound),
            _ => return Err(TaskError::Unreachable),
        }
        if response
            .encoding
            .is_some_and(|encoding| !encoding.eq_ignore_ascii_case("identity"))
        {
            return Err(TaskError::Unreachable);
        }
        if !identity.format.accepts(response.content_type.as_deref()) {
            return Err(TaskError::Unreachable);
        }
        let framing = match (response.chunked, response.length) {
            (true, Some(_)) => return Err(TaskError::Unreachable),
            (true, None) => Framing::Chunked(ChunkPhase::Size),
            (false, Some(length)) => Framing::Length(length),
            (false, None) => Framing::Close,
        };
        let mut stream = LineStream {
            held,
            identity,
            framing,
            wire: response.body,
            decoded: Vec::new(),
            partial: Vec::new(),
            ended: false,
            last_progress: Instant::now(),
            _lease: lease,
        };
        stream.decode_available()?;
        Ok(OpenResult::Retained(Box::new(stream)))
    }

    fn install_open(
        &self,
        url: &str,
        generation: u64,
        stream: Box<LineStream>,
        cancellation: Cancellation<'_>,
    ) -> Result<Vec<u8>, TaskError> {
        let mut state = self.state.lock().map_err(|_| TaskError::Unreachable)?;
        let current = state.streams.get(url);
        let may_install = matches!(
            current,
            Some(StreamSlot::Opening {
                generation: current,
                ..
            }) if *current == generation
        ) && !cancellation.requested();
        if !may_install {
            if current.is_some_and(|slot| slot.generation() == generation) {
                state.streams.remove(url);
            }
            return Err(TaskError::TimedOut);
        }
        state
            .streams
            .insert(url.to_owned(), StreamSlot::Ready { generation, stream });
        if cancellation.requested() {
            state.streams.remove(url);
            return Err(TaskError::TimedOut);
        }
        Ok(Vec::new())
    }

    fn next(
        &self,
        url: &str,
        max_bytes: u32,
        credential: Option<(&str, &str)>,
        headers: &[(&str, &str)],
        cancel: &AtomicBool,
    ) -> Result<Vec<u8>, TaskError> {
        let format = Format::from_headers(headers)?;
        let expected = StreamIdentity {
            credential: credential_identity(credential),
            headers: headers_identity(headers),
            format,
            maximum: max_bytes as usize,
        };
        let (generation, close, mut stream) = {
            let mut state = self.state.lock().map_err(|_| TaskError::Unreachable)?;
            let slot = state.streams.remove(url).ok_or(TaskError::Unreachable)?;
            let StreamSlot::Ready { generation, stream } = slot else {
                state.streams.insert(url.to_owned(), slot);
                return Err(TaskError::Unreachable);
            };
            if stream.identity.credential != expected.credential {
                return Err(TaskError::Unauthorized);
            }
            if stream.identity != expected {
                return Err(TaskError::Denied);
            }
            let close = Arc::new(AtomicBool::new(false));
            state.streams.insert(
                url.to_owned(),
                StreamSlot::Reading {
                    generation,
                    close: Arc::clone(&close),
                },
            );
            (generation, close, stream)
        };
        let cancellation = Cancellation {
            task: cancel,
            stream: &close,
        };
        let result = stream.next(cancellation);
        let mut state = self.state.lock().map_err(|_| TaskError::Unreachable)?;
        let current = state.streams.get(url);
        let may_retain = result.is_ok()
            && !stream.ended
            && !cancellation.requested()
            && matches!(
                current,
                Some(StreamSlot::Reading {
                    generation: current,
                    ..
                }) if *current == generation
            );
        if may_retain {
            state
                .streams
                .insert(url.to_owned(), StreamSlot::Ready { generation, stream });
        } else if current.is_some_and(|slot| slot.generation() == generation) {
            state.streams.remove(url);
        }
        result
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Format {
    EventStream,
    Ndjson,
}

impl Format {
    fn from_headers(headers: &[(&str, &str)]) -> Result<Self, TaskError> {
        let mut accepted = None;
        for (name, value) in headers {
            if name.eq_ignore_ascii_case("accept") {
                if accepted.is_some() {
                    return Err(TaskError::Denied);
                }
                accepted = match value.trim() {
                    "text/event-stream" => Some(Self::EventStream),
                    "application/x-ndjson" => Some(Self::Ndjson),
                    _ => return Err(TaskError::Denied),
                };
            }
        }
        accepted.ok_or(TaskError::Denied)
    }

    fn accepts(self, content_type: Option<&str>) -> bool {
        let Some(content_type) = content_type else {
            return false;
        };
        let media_type = content_type
            .split_once(';')
            .map_or(content_type, |(media_type, _)| media_type)
            .trim();
        match self {
            Self::EventStream => media_type.eq_ignore_ascii_case("text/event-stream"),
            Self::Ndjson => {
                media_type.eq_ignore_ascii_case("application/x-ndjson")
                    || media_type.eq_ignore_ascii_case("application/ndjson")
            }
        }
    }
}

fn credential_identity(credential: Option<(&str, &str)>) -> String {
    let mut material = Vec::new();
    if let Some((name, value)) = credential {
        material.extend_from_slice(name.as_bytes());
        material.push(0);
        material.extend_from_slice(value.as_bytes());
    }
    super::sha256::hex_digest(&material)
}

fn headers_identity(headers: &[(&str, &str)]) -> String {
    let mut material = Vec::new();
    for (name, value) in headers {
        material.extend_from_slice(name.to_ascii_lowercase().as_bytes());
        material.push(0);
        material.extend_from_slice(value.as_bytes());
        material.push(0xff);
    }
    super::sha256::hex_digest(&material)
}

struct ResponseHead {
    status: u16,
    chunked: bool,
    length: Option<usize>,
    content_type: Option<String>,
    encoding: Option<String>,
    retry_after: Option<u32>,
    body: Vec<u8>,
}

fn response_head(
    held: &mut Held,
    address: &Address,
    method: &Method<'_>,
    cancellation: Cancellation<'_>,
) -> Result<ResponseHead, TaskError> {
    let mut tls = rustls::Stream::new(&mut held.connection, &mut held.socket);
    let cancelled = || cancellation.requested();
    super::write_request_head(&mut tls, &head(address, method, 1), &cancelled)
        .map_err(super::WriteFailure::task_error)?;

    let mut response = Vec::new();
    let mut buffer = [0_u8; 4096];
    let started = Instant::now();
    loop {
        if cancellation.requested() {
            return Err(TaskError::TimedOut);
        }
        let mut headers = [httparse::EMPTY_HEADER; MAX_RESPONSE_HEADERS];
        let mut parsed = httparse::Response::new(&mut headers);
        match parsed.parse(&response) {
            Ok(httparse::Status::Complete(end)) if end <= MAX_HEADER_BYTES => {
                let status = parsed.code.ok_or(TaskError::Unreachable)?;
                let chunked = transfer_is_chunked(parsed.headers)?;
                let length = content_length(parsed.headers)?;
                let content_type = header(parsed.headers, "content-type").map(str::to_owned);
                let encoding = header(parsed.headers, "content-encoding").map(str::to_owned);
                let retry_after = header(parsed.headers, "retry-after")
                    .and_then(|value| value.parse::<u32>().ok())
                    .map(|seconds| seconds.min(super::MAX_RETRY_AFTER_SECONDS));
                return Ok(ResponseHead {
                    status,
                    chunked,
                    length,
                    content_type,
                    encoding,
                    retry_after,
                    body: response[end..].to_vec(),
                });
            }
            Ok(httparse::Status::Complete(_)) | Err(_) => return Err(TaskError::Unreachable),
            Ok(httparse::Status::Partial) => {}
        }
        match tls.read(&mut buffer) {
            Ok(0) => return Err(TaskError::Unreachable),
            Ok(read) => {
                response.extend_from_slice(&buffer[..read]);
                if response.len() > MAX_HEADER_BYTES {
                    return Err(TaskError::Unreachable);
                }
            }
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                if started.elapsed() >= RESPONSE_TIMEOUT {
                    return Err(TaskError::TimedOut);
                }
            }
            Err(_) => return Err(TaskError::Unreachable),
        }
    }
}

enum Framing {
    Chunked(ChunkPhase),
    Length(usize),
    Close,
}

enum ChunkPhase {
    Size,
    Data(usize),
    DataCrLf,
    Trailers,
    End,
}

struct LineStream {
    held: Held,
    identity: StreamIdentity,
    framing: Framing,
    wire: Vec<u8>,
    decoded: Vec<u8>,
    partial: Vec<u8>,
    ended: bool,
    last_progress: Instant,
    _lease: StreamLease,
}

impl LineStream {
    fn next(&mut self, cancellation: Cancellation<'_>) -> Result<Vec<u8>, TaskError> {
        loop {
            if let Some(record) = self.take_record()? {
                return Ok(record);
            }
            if self.ended {
                return Err(TaskError::Unreachable);
            }
            if cancellation.requested() {
                return Err(TaskError::TimedOut);
            }
            self.read_more(cancellation)?;
            self.decode_available()?;
        }
    }

    fn read_more(&mut self, cancellation: Cancellation<'_>) -> Result<(), TaskError> {
        let mut buffer = [0_u8; 4096];
        loop {
            if cancellation.requested() {
                return Err(TaskError::TimedOut);
            }
            let mut tls = rustls::Stream::new(&mut self.held.connection, &mut self.held.socket);
            match tls.read(&mut buffer) {
                Ok(0) => {
                    self.ended = true;
                    return Ok(());
                }
                Ok(read) => {
                    self.wire.extend_from_slice(&buffer[..read]);
                    self.last_progress = Instant::now();
                    if self
                        .wire
                        .len()
                        .saturating_add(self.decoded.len())
                        .saturating_add(self.partial.len())
                        > self.identity.maximum.saturating_add(MAX_HEADER_BYTES)
                    {
                        return Err(TaskError::TooLarge);
                    }
                    return Ok(());
                }
                Err(error)
                    if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) =>
                {
                    if self.last_progress.elapsed() >= RESPONSE_TIMEOUT {
                        return Err(TaskError::TimedOut);
                    }
                }
                Err(_) => return Err(TaskError::Unreachable),
            }
        }
    }

    fn decode_available(&mut self) -> Result<(), TaskError> {
        decode_wire(
            &mut self.framing,
            &mut self.wire,
            &mut self.decoded,
            &mut self.ended,
        )?;
        if self.decoded.len().saturating_add(self.partial.len()) > self.identity.maximum {
            return Err(TaskError::TooLarge);
        }
        Ok(())
    }

    fn take_record(&mut self) -> Result<Option<Vec<u8>>, TaskError> {
        take_record(
            self.identity.format,
            &mut self.decoded,
            &mut self.partial,
            self.identity.maximum,
        )
    }
}

fn decode_wire(
    framing: &mut Framing,
    wire: &mut Vec<u8>,
    decoded: &mut Vec<u8>,
    ended: &mut bool,
) -> Result<(), TaskError> {
    match framing {
        Framing::Close => decoded.append(wire),
        Framing::Length(remaining) => {
            let take = (*remaining).min(wire.len());
            decoded.extend_from_slice(&wire[..take]);
            wire.drain(..take);
            *remaining -= take;
            if *remaining == 0 {
                *ended = true;
            }
        }
        Framing::Chunked(phase) => loop {
            match phase {
                ChunkPhase::Size => {
                    let Some(end) = find(wire, b"\r\n") else {
                        break;
                    };
                    let line =
                        std::str::from_utf8(&wire[..end]).map_err(|_| TaskError::Unreachable)?;
                    let size = line.split_once(';').map_or(line, |(size, _)| size).trim();
                    let size =
                        usize::from_str_radix(size, 16).map_err(|_| TaskError::Unreachable)?;
                    wire.drain(..end + 2);
                    *phase = if size == 0 {
                        ChunkPhase::Trailers
                    } else {
                        ChunkPhase::Data(size)
                    };
                }
                ChunkPhase::Data(remaining) => {
                    if wire.len() < *remaining {
                        break;
                    }
                    decoded.extend_from_slice(&wire[..*remaining]);
                    wire.drain(..*remaining);
                    *phase = ChunkPhase::DataCrLf;
                }
                ChunkPhase::DataCrLf => {
                    if wire.len() < 2 {
                        break;
                    }
                    if &wire[..2] != b"\r\n" {
                        return Err(TaskError::Unreachable);
                    }
                    wire.drain(..2);
                    *phase = ChunkPhase::Size;
                }
                ChunkPhase::Trailers => {
                    let end = if wire.starts_with(b"\r\n") {
                        Some(2)
                    } else {
                        find(wire, b"\r\n\r\n").map(|end| end + 4)
                    };
                    let Some(end) = end else {
                        break;
                    };
                    wire.drain(..end);
                    *phase = ChunkPhase::End;
                    *ended = true;
                }
                ChunkPhase::End => {
                    if !wire.is_empty() {
                        return Err(TaskError::Unreachable);
                    }
                    break;
                }
            }
        },
    }
    Ok(())
}

fn take_record(
    format: Format,
    decoded: &mut Vec<u8>,
    partial: &mut Vec<u8>,
    maximum: usize,
) -> Result<Option<Vec<u8>>, TaskError> {
    while let Some(end) = decoded.iter().position(|byte| *byte == b'\n') {
        let mut line = decoded.drain(..=end).collect::<Vec<_>>();
        line.pop();
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        match format {
            Format::Ndjson => {
                if line.is_empty() {
                    continue;
                }
                if line.len() > maximum {
                    return Err(TaskError::TooLarge);
                }
                return Ok(Some(line));
            }
            Format::EventStream => {
                if line.is_empty() {
                    if partial.is_empty() {
                        continue;
                    }
                    return Ok(Some(std::mem::take(partial)));
                }
                if line.starts_with(b":") {
                    continue;
                }
                if !partial.is_empty() {
                    partial.push(b'\n');
                }
                partial.extend_from_slice(&line);
                if partial.len() > maximum {
                    return Err(TaskError::TooLarge);
                }
            }
        }
    }
    Ok(None)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::{
        decode_wire, take_record, ChunkPhase, Format, Framing, LineStreamAction, LineStreams,
        RequestOptions, StreamSlot, MAX_RETAINED_STREAMS,
    };
    use kobo_protocol::TaskError;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
    use rustls::{ServerConfig, ServerConnection, StreamOwned};
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::{mpsc, Arc};
    use std::thread;
    use std::time::{Duration, Instant};

    const CERTIFICATE: &[u8] = include_bytes!("../tests/fixtures/localhost-cert.der");
    const PRIVATE_KEY: &[u8] = include_bytes!("../tests/fixtures/localhost-key.der");

    fn trust_fixture() {
        crate::tls_config().expect("test TLS configuration");
    }

    fn server_config() -> Arc<ServerConfig> {
        let certificate = CertificateDer::from(CERTIFICATE.to_vec());
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(PRIVATE_KEY.to_vec()));
        Arc::new(
            ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .expect("protocol versions")
                .with_no_client_auth()
                .with_single_cert(vec![certificate], key)
                .expect("mock certificate"),
        )
    }

    fn accept(
        listener: &TcpListener,
        config: Arc<ServerConfig>,
    ) -> StreamOwned<ServerConnection, TcpStream> {
        let (socket, _) = listener.accept().expect("accept mock client");
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("read timeout");
        let connection = ServerConnection::new(config).expect("server connection");
        let mut stream = StreamOwned::new(connection, socket);
        let mut request = Vec::new();
        let mut buffer = [0_u8; 2048];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let read = stream.read(&mut buffer).expect("read request");
            assert!(read > 0, "client closed before request");
            request.extend_from_slice(&buffer[..read]);
        }
        stream
    }

    struct BlockingServer {
        url: String,
        accepted: mpsc::Receiver<()>,
        release: mpsc::Sender<()>,
        handle: thread::JoinHandle<()>,
    }

    fn blocking_stream_server(path: &str) -> BlockingServer {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind mock");
        let address = listener.local_addr().expect("mock address");
        let config = server_config();
        let (report_accepted, accepted) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let handle = thread::spawn(move || {
            let mut stream = accept(&listener, config);
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nTransfer-Encoding: chunked\r\nConnection: keep-alive\r\n\r\n",
                )
                .expect("response head");
            stream.flush().expect("flush response head");
            report_accepted.send(()).expect("report accepted");
            let _ = wait.recv_timeout(Duration::from_secs(5));
            stream.conn.send_close_notify();
            let _ = stream.flush();
        });
        BlockingServer {
            url: format!("https://localhost:{}{path}", address.port()),
            accepted,
            release,
            handle,
        }
    }

    fn raw_blocking_server(path: &str) -> BlockingServer {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind raw mock");
        let address = listener.local_addr().expect("raw mock address");
        let (report_accepted, accepted) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let handle = thread::spawn(move || {
            let (_socket, _) = listener.accept().expect("accept raw client");
            report_accepted.send(()).expect("report raw connection");
            let _ = wait.recv_timeout(Duration::from_secs(5));
        });
        BlockingServer {
            url: format!("https://localhost:{}{path}", address.port()),
            accepted,
            release,
            handle,
        }
    }

    fn request(
        streams: &LineStreams,
        action: LineStreamAction,
        url: &str,
    ) -> Result<Vec<u8>, TaskError> {
        streams.request(
            action,
            url,
            4096,
            (action != LineStreamAction::Close).then_some(("Authorization", "******")),
            &[("Accept", "application/x-ndjson")],
            RequestOptions::default(),
            &std::sync::atomic::AtomicBool::new(false),
        )
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum SlotKind {
        Opening,
        Ready,
        Reading,
        Closed,
    }

    fn slot_kind(streams: &LineStreams, url: &str) -> Option<SlotKind> {
        streams
            .state
            .lock()
            .expect("stream state")
            .streams
            .get(url)
            .map(|slot| match slot {
                StreamSlot::Opening { .. } => SlotKind::Opening,
                StreamSlot::Ready { .. } => SlotKind::Ready,
                StreamSlot::Reading { .. } => SlotKind::Reading,
                StreamSlot::Closed { .. } => SlotKind::Closed,
            })
    }

    fn wait_for_slot(streams: &LineStreams, url: &str, expected: SlotKind) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if slot_kind(streams, url) == Some(expected) {
                return;
            }
            thread::yield_now();
        }
        panic!(
            "stream {url} did not become {expected:?}; current state is {:?}",
            slot_kind(streams, url)
        );
    }

    #[test]
    fn close_and_shutdown_interrupt_resolve_or_connect_before_a_socket_exists() {
        let (report_entered, entered) = mpsc::channel();
        let connector = Arc::new(
            move |_: &super::Address,
                  cancelled: &dyn Fn() -> bool|
                  -> Result<super::Held, TaskError> {
                report_entered.send(()).expect("report connection phase");
                let deadline = Instant::now() + Duration::from_secs(2);
                while !cancelled() {
                    assert!(
                        Instant::now() < deadline,
                        "the simulated resolver was never cancelled"
                    );
                    thread::sleep(Duration::from_millis(1));
                }
                Err(TaskError::TimedOut)
            },
        );
        let streams = Arc::new(LineStreams::with_connector(connector));

        for (index, shutdown) in [(1, false), (2, true)] {
            let url = format!("https://resolver.invalid/api/stream/{index}");
            let worker_streams = Arc::clone(&streams);
            let worker_url = url.clone();
            let (report_result, result) = mpsc::channel();
            let worker = thread::spawn(move || {
                report_result
                    .send(request(
                        &worker_streams,
                        LineStreamAction::Open,
                        &worker_url,
                    ))
                    .expect("report open");
            });
            entered
                .recv_timeout(Duration::from_secs(2))
                .expect("open entered connection phase");
            wait_for_slot(&streams, &url, SlotKind::Opening);
            let started = Instant::now();
            if shutdown {
                streams.close_all();
            } else {
                assert_eq!(
                    request(&streams, LineStreamAction::Close, &url),
                    Ok(Vec::new())
                );
            }
            assert_eq!(
                result
                    .recv_timeout(Duration::from_secs(1))
                    .expect("cancelled connection"),
                Err(TaskError::TimedOut)
            );
            assert!(started.elapsed() < Duration::from_secs(1));
            worker.join().expect("connection worker");
            assert_eq!(slot_kind(&streams, &url), None);
            assert_eq!(streams.budget.in_use(), 0);
        }
    }

    #[test]
    fn close_during_request_head_write_prevents_open_reinsertion() {
        trust_fixture();
        let server = raw_blocking_server("/api/stream/event");
        let streams = Arc::new(LineStreams::default());
        let worker_streams = Arc::clone(&streams);
        let worker_url = server.url.clone();
        let (report_result, result) = mpsc::channel();
        let worker = thread::spawn(move || {
            report_result
                .send(request(
                    &worker_streams,
                    LineStreamAction::Open,
                    &worker_url,
                ))
                .expect("report open");
        });

        server
            .accepted
            .recv_timeout(Duration::from_secs(2))
            .expect("TCP connection reached server");
        wait_for_slot(&streams, &server.url, SlotKind::Opening);
        let started = Instant::now();
        assert_eq!(
            request(&streams, LineStreamAction::Close, &server.url),
            Ok(Vec::new())
        );
        assert_eq!(
            result
                .recv_timeout(Duration::from_secs(2))
                .expect("cancelled open"),
            Err(TaskError::TimedOut)
        );
        assert!(started.elapsed() < Duration::from_secs(1));
        worker.join().expect("open worker");
        assert_eq!(slot_kind(&streams, &server.url), None);
        assert_eq!(streams.budget.in_use(), 0);
        assert_eq!(
            request(&streams, LineStreamAction::Next, &server.url),
            Err(TaskError::Unreachable)
        );
        server.release.send(()).expect("release server");
        server.handle.join().expect("open server");
    }

    #[test]
    fn close_during_next_and_shutdown_do_not_restore_an_old_generation() {
        trust_fixture();
        let server = blocking_stream_server("/api/stream/event");
        let streams = Arc::new(LineStreams::default());
        assert_eq!(
            request(&streams, LineStreamAction::Open, &server.url),
            Ok(Vec::new())
        );
        server
            .accepted
            .recv_timeout(Duration::from_secs(2))
            .expect("stream response");

        let worker_streams = Arc::clone(&streams);
        let worker_url = server.url.clone();
        let (report_result, result) = mpsc::channel();
        let worker = thread::spawn(move || {
            report_result
                .send(request(
                    &worker_streams,
                    LineStreamAction::Next,
                    &worker_url,
                ))
                .expect("report next");
        });
        wait_for_slot(&streams, &server.url, SlotKind::Reading);
        assert_eq!(
            request(&streams, LineStreamAction::Close, &server.url),
            Ok(Vec::new())
        );
        assert_eq!(
            result
                .recv_timeout(Duration::from_secs(2))
                .expect("cancelled next"),
            Err(TaskError::TimedOut)
        );
        worker.join().expect("next worker");
        assert_eq!(slot_kind(&streams, &server.url), None);
        assert_eq!(streams.budget.in_use(), 0);
        server.release.send(()).expect("release first server");
        server.handle.join().expect("first server");

        let event = blocking_stream_server("/api/stream/event");
        let game = blocking_stream_server("/api/board/game/stream/abcdEF12");
        for open in [&event, &game] {
            assert_eq!(
                request(&streams, LineStreamAction::Open, &open.url),
                Ok(Vec::new())
            );
            open.accepted
                .recv_timeout(Duration::from_secs(2))
                .expect("stream response");
        }
        assert_eq!(streams.budget.in_use(), MAX_RETAINED_STREAMS);
        assert_eq!(
            request(
                &streams,
                LineStreamAction::Open,
                "https://localhost:1/api/board/game/stream/ijklMN34",
            ),
            Err(TaskError::Denied)
        );

        let worker_streams = Arc::clone(&streams);
        let worker_url = event.url.clone();
        let (report_result, result) = mpsc::channel();
        let worker = thread::spawn(move || {
            report_result
                .send(request(
                    &worker_streams,
                    LineStreamAction::Next,
                    &worker_url,
                ))
                .expect("report shutdown next");
        });
        wait_for_slot(&streams, &event.url, SlotKind::Reading);
        streams.close_all();
        assert_eq!(
            result
                .recv_timeout(Duration::from_secs(2))
                .expect("shutdown next"),
            Err(TaskError::TimedOut)
        );
        worker.join().expect("shutdown worker");
        assert_eq!(streams.budget.in_use(), 0);
        assert_eq!(slot_kind(&streams, &event.url), None);
        assert_eq!(slot_kind(&streams, &game.url), None);

        for open in [event, game] {
            open.release.send(()).expect("release server");
            open.handle.join().expect("stream server");
        }
    }

    #[test]
    fn chunked_ndjson_survives_arbitrary_network_boundaries() {
        let mut framing = Framing::Chunked(ChunkPhase::Size);
        let mut wire = b"4\r\n{\"a\"".to_vec();
        let mut decoded = Vec::new();
        let mut ended = false;
        decode_wire(&mut framing, &mut wire, &mut decoded, &mut ended).expect("first piece");
        assert_eq!(decoded, b"{\"a\"");
        wire.extend_from_slice(b"\r\n4\r\n:1}\n\r\n0\r\n\r\n");
        decode_wire(&mut framing, &mut wire, &mut decoded, &mut ended).expect("last pieces");
        assert!(ended);
        let mut partial = Vec::new();
        assert_eq!(
            take_record(Format::Ndjson, &mut decoded, &mut partial, 1024)
                .expect("record")
                .as_deref(),
            Some(&b"{\"a\":1}"[..])
        );
    }

    #[test]
    fn sse_keepalives_are_ignored_and_multiline_events_stay_whole() {
        let mut decoded =
            b": keepalive\r\n\r\nevent: game\ndata: {\"a\":1}\ndata: {\"b\":2}\r\n\r\n".to_vec();
        let mut partial = Vec::new();
        assert_eq!(
            take_record(Format::EventStream, &mut decoded, &mut partial, 1024)
                .expect("event")
                .as_deref(),
            Some(&b"event: game\ndata: {\"a\":1}\ndata: {\"b\":2}"[..])
        );
    }

    #[test]
    fn malformed_chunking_and_oversized_records_fail_closed() {
        let mut framing = Framing::Chunked(ChunkPhase::Size);
        let mut wire = b"zz\r\n".to_vec();
        let mut decoded = Vec::new();
        let mut ended = false;
        assert_eq!(
            decode_wire(&mut framing, &mut wire, &mut decoded, &mut ended),
            Err(TaskError::Unreachable)
        );

        let mut decoded = b"12345\n".to_vec();
        let mut partial = Vec::new();
        assert_eq!(
            take_record(Format::Ndjson, &mut decoded, &mut partial, 4),
            Err(TaskError::TooLarge)
        );
    }

    #[test]
    fn a_truncated_sse_event_is_not_returned_as_complete() {
        let mut decoded = b"event: game\ndata: {\"a\":1}".to_vec();
        let mut partial = Vec::new();
        assert_eq!(
            take_record(Format::EventStream, &mut decoded, &mut partial, 1024).expect("partial"),
            None
        );
        assert!(
            !decoded.is_empty() || !partial.is_empty(),
            "end-of-stream logic must reject this truncated record"
        );
    }
}

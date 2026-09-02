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
use std::io::{ErrorKind, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Instant;

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
#[derive(Default)]
pub struct LineStreams {
    streams: Mutex<HashMap<String, LineStream>>,
}

impl std::fmt::Debug for LineStreams {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let count = self.streams.lock().map_or(0, |streams| streams.len());
        formatter
            .debug_struct("LineStreams")
            .field("open", &count)
            .finish()
    }
}

impl LineStreams {
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
            LineStreamAction::Close => {
                self.close(url);
                Ok(Vec::new())
            }
        }
    }

    /// Closes one retained stream, if present.
    pub fn close(&self, url: &str) {
        if let Ok(mut streams) = self.streams.lock() {
            streams.remove(url);
        }
    }

    /// Drops every retained socket owned by this application runner.
    pub fn close_all(&self) {
        if let Ok(mut streams) = self.streams.lock() {
            streams.clear();
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
        let format = Format::from_headers(headers)?;
        let identity = credential_identity(credential);
        let headers_identity = headers_identity(headers);
        {
            let mut streams = self.streams.lock().map_err(|_| TaskError::Unreachable)?;
            if streams.get(url).is_some_and(|stream| {
                stream.identity == identity
                    && stream.headers_identity == headers_identity
                    && stream.format == format
                    && stream.maximum == max_bytes as usize
            }) {
                return Ok(Vec::new());
            }
            streams.remove(url);
        }

        let address = super::parse(url)?;
        let mut held = connect(&address)?;
        let method = Method::Get {
            offset: None,
            credential,
            headers,
            streaming: true,
        };
        let response = response_head(&mut held, &address, &method, cancel)?;
        let status = response.status;
        if status == 429 && options.report_rate_limit {
            return Ok(super::rate_limit_envelope(response.retry_after));
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
        if !format.accepts(response.content_type.as_deref()) {
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
            format,
            identity,
            headers_identity,
            framing,
            wire: response.body,
            decoded: Vec::new(),
            partial: Vec::new(),
            maximum: max_bytes as usize,
            ended: false,
            last_progress: Instant::now(),
        };
        stream.decode_available()?;
        let mut streams = self.streams.lock().map_err(|_| TaskError::Unreachable)?;
        if cancel.load(Ordering::SeqCst) {
            return Err(TaskError::TimedOut);
        }
        streams.insert(url.to_owned(), stream);
        if cancel.load(Ordering::SeqCst) {
            streams.remove(url);
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
        let mut stream = self
            .streams
            .lock()
            .map_err(|_| TaskError::Unreachable)?
            .remove(url)
            .ok_or(TaskError::NotFound)?;
        if stream.identity != credential_identity(credential) {
            return Err(TaskError::Unauthorized);
        }
        if stream.format != format
            || stream.headers_identity != headers_identity(headers)
            || stream.maximum != max_bytes as usize
        {
            return Err(TaskError::Denied);
        }
        let result = stream.next(cancel);
        if result.is_ok() && !cancel.load(Ordering::SeqCst) && !stream.ended {
            self.streams
                .lock()
                .map_err(|_| TaskError::Unreachable)?
                .insert(url.to_owned(), stream);
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
    cancel: &AtomicBool,
) -> Result<ResponseHead, TaskError> {
    let mut tls = rustls::Stream::new(&mut held.connection, &mut held.socket);
    tls.write_all(head(address, method, 1).as_bytes())
        .map_err(|_| TaskError::Unreachable)?;
    tls.flush().map_err(|_| TaskError::Unreachable)?;

    let mut response = Vec::new();
    let mut buffer = [0_u8; 4096];
    let started = Instant::now();
    loop {
        if cancel.load(Ordering::SeqCst) {
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
    format: Format,
    identity: String,
    headers_identity: String,
    framing: Framing,
    wire: Vec<u8>,
    decoded: Vec<u8>,
    partial: Vec<u8>,
    maximum: usize,
    ended: bool,
    last_progress: Instant,
}

impl LineStream {
    fn next(&mut self, cancel: &AtomicBool) -> Result<Vec<u8>, TaskError> {
        loop {
            if let Some(record) = self.take_record()? {
                return Ok(record);
            }
            if self.ended {
                return if self.partial.is_empty() && self.decoded.is_empty() {
                    Err(TaskError::NotFound)
                } else {
                    Err(TaskError::Unreachable)
                };
            }
            if cancel.load(Ordering::SeqCst) {
                return Err(TaskError::TimedOut);
            }
            self.read_more(cancel)?;
            self.decode_available()?;
        }
    }

    fn read_more(&mut self, cancel: &AtomicBool) -> Result<(), TaskError> {
        let mut buffer = [0_u8; 4096];
        loop {
            if cancel.load(Ordering::SeqCst) {
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
                        > self.maximum.saturating_add(MAX_HEADER_BYTES)
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
        if self.decoded.len().saturating_add(self.partial.len()) > self.maximum {
            return Err(TaskError::TooLarge);
        }
        Ok(())
    }

    fn take_record(&mut self) -> Result<Option<Vec<u8>>, TaskError> {
        take_record(
            self.format,
            &mut self.decoded,
            &mut self.partial,
            self.maximum,
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
    use super::{decode_wire, take_record, ChunkPhase, Format, Framing};
    use kobo_protocol::TaskError;

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

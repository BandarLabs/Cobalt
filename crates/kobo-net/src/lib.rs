//! The one place in this workspace that talks to the internet.
//!
//! Applications never open a socket. They submit a `Fetch` task, the runtime
//! decides whether the capability was granted, and this module performs the
//! request under a byte ceiling and a deadline. Keeping it in a crate of its
//! own means the other packages stay free of external dependencies, and it
//! means the cryptography can be replaced without any application changing.
//!
//! ## Why the runtime carries its own TLS
//!
//! Measured on a Clara BW: the newest OpenSSL present is 1.0.1j from 2014,
//! there is no CA bundle anywhere on the filesystem, and `s_client` fails with
//! `sslv3 alert handshake failure` against a large share of modern hosts while
//! succeeding against others. A platform whose network calls work for some
//! addresses and silently fail for others is not one anybody can build on, so
//! the runtime links its own verifier and its own roots and ignores the
//! device's libraries entirely.
//!
//! ## Parsing and cryptography
//!
//! URL syntax is parsed by [`http::Uri`], and response heads and chunk sizes
//! by `httparse`, rather than by request-line and header string splitting.
//! The transport remains deliberately small so its byte ceilings, range
//! requests and Kobo-specific TLS roots stay visible, while Rustls uses its
//! maintained ring provider instead of an experimental provider.

pub mod gzip;
mod lines;
pub mod pem;
pub mod serve;
pub mod sha256;

pub use lines::{LineStreamAction, LineStreams};

use kobo_protocol::TaskError;
use std::borrow::Cow;
use std::fmt::Write as _;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How long opening a connection may take before the host is called
/// unreachable.
///
/// Short on purpose. A reader whose radio is off, or which is associated with
/// a network that goes nowhere, should be told so in seconds rather than
/// minutes, and nothing has been said yet that would be worth waiting for.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a server that has accepted the connection may stay silent.
///
/// Deliberately much longer than [`CONNECT_TIMEOUT`], because the two are not
/// the same failure. A host that will not answer at all is broken; a host that
/// has taken the request and is working on it is not. A reasoning model
/// answering a request for a four chapter script takes a measured hundred
/// seconds to send its first byte, and at thirty seconds every audiobook this
/// project generates failed as "the network was too slow to answer" -- which
/// was the runtime giving up, not the network.
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(180);

/// How often a blocking socket wakes to notice task cancellation.
const IO_POLL: Duration = Duration::from_millis(250);

/// The largest response header block accepted before the body is refused.
const MAX_HEADER_BYTES: usize = 32 * 1024;

/// A malicious server cannot ask an e-reader UI to suppress retry forever.
const MAX_RETRY_AFTER_SECONDS: u32 = 60 * 60;

/// Runtime-only behavior layered on the existing protocol-11 request shape.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RequestOptions {
    /// Return a small status envelope for HTTP 429 instead of flattening it
    /// into `NotFound`, so an opted-in application can honor `Retry-After`.
    pub report_rate_limit: bool,
    /// Keep a successful POST open until its task is cancelled, discarding
    /// blank keepalives. This is for APIs whose open request is the resource.
    pub wait_until_cancelled: bool,
}

/// Where Linux lists the routes it holds, and so whether there is a network.
const ROUTE_TABLE: &str = "/proc/net/route";

/// Decides whether a failure to connect was this reader's fault or the host's.
///
/// Only ever called *after* something has already failed, and that ordering is
/// the whole design. The radio on this reader powers down when idle and wakes
/// when a socket asks for it, so a check made before a request would report a
/// reader offline that was about to succeed. Asked afterwards it costs nothing
/// on the path where everything worked, and it can only sharpen a failure that
/// has already happened.
///
/// A default route is the question, not a ping: a ping needs a host to answer
/// and would fail for the same reasons the request just did.
///
/// Anywhere without a Linux route table, which is every host this is tested
/// on, is treated as having a network. A simulator that claimed to be offline
/// because it is not a Kobo would teach the apps the wrong lesson.
fn no_route_to_anywhere() -> bool {
    let Ok(table) = std::fs::read_to_string(ROUTE_TABLE) else {
        return false;
    };
    !has_default_route(&table)
}

/// True when the route table holds a usable default route.
///
/// The columns are iface, destination, gateway, flags, and the rest. A default
/// route is the one whose destination is all zeroes. Loopback is skipped
/// because a reader with nothing but `lo` is a reader with no network, and it
/// is exactly the state a powered-down radio leaves behind.
fn has_default_route(table: &str) -> bool {
    table
        .lines()
        .skip(1)
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let interface = fields.next()?;
            let destination = fields.next()?;
            let flags = fields.nth(1)?;
            Some((interface, destination, flags))
        })
        .any(|(interface, destination, flags)| {
            let up = u32::from_str_radix(flags, 16).is_ok_and(|flags| flags & 1 != 0);
            interface != "lo" && destination.trim_start_matches('0').is_empty() && up
        })
}

/// The failure to report when a socket could not be opened.
fn could_not_connect() -> TaskError {
    if no_route_to_anywhere() {
        TaskError::Offline
    } else {
        TaskError::Unreachable
    }
}

/// The most response headers retained while parsing.
const MAX_RESPONSE_HEADERS: usize = 64;

/// How many redirects to follow before giving up.
///
/// Download links are almost always redirects: the one measured here,
/// Gutenberg's `.epub` URL, answers 302 and sends the caller elsewhere. Not
/// following them would make the runtime useless for the thing it exists for.
/// The chain is bounded so a server that loops cannot hold a task open.
const MAX_REDIRECTS: usize = 5;

/// A URL split into the parts a request needs.
#[derive(Debug, Eq, PartialEq)]
pub struct Address {
    pub host: String,
    pub port: u16,
    pub path: String,
    authority: String,
}

/// Splits an `https` URL, refusing anything this runtime will not fetch.
///
/// Plain `http` is rejected rather than upgraded. An application asking for an
/// unencrypted URL has made a mistake worth reporting, and silently rewriting
/// it would hide that the request it believed it made was not the one sent.
///
/// # Errors
///
/// Returns [`TaskError::NotFound`] for anything that is not a well formed
/// `https` URL with a host.
pub fn parse(url: &str) -> Result<Address, TaskError> {
    let target = url.parse::<http::Uri>().map_err(|_| TaskError::NotFound)?;
    if target.scheme_str() != Some("https") {
        return Err(TaskError::NotFound);
    }
    let authority = target.authority().ok_or(TaskError::NotFound)?;
    // Credentials in a URL would be sent to the host and written into any log
    // that records the request, so they are refused rather than stripped.
    if authority.as_str().contains('@') {
        return Err(TaskError::NotFound);
    }
    let host = authority.host();
    if host.is_empty() {
        return Err(TaskError::NotFound);
    }
    let port = authority.port_u16().unwrap_or(443);
    let path = target
        .path_and_query()
        .map_or("/", http::uri::PathAndQuery::as_str);
    Ok(Address {
        host: host.to_string(),
        port,
        path: path.to_string(),
        authority: authority.as_str().to_string(),
    })
}

/// Whether `url` has exactly the supplied HTTPS host and port.
///
/// Credential policy uses this instead of string prefixes, for which
/// `api.example.com.attacker.invalid` would look like the intended host.
#[must_use]
pub fn has_origin(url: &str, host: &str, port: u16) -> bool {
    parse(url).is_ok_and(|address| address.host.eq_ignore_ascii_case(host) && address.port == port)
}

/// What a server said, once the status line has been understood.
#[derive(Debug, Eq, PartialEq)]
pub enum Response<'a> {
    /// Borrowed when the server framed the body with a length, owned when it
    /// arrived chunked and had to be reassembled.
    Body(Cow<'a, [u8]>),
    /// The value of the `Location` header, which may be relative.
    Redirect(String),
}

/// A protocol-neutral body used only by requests that explicitly opt in to
/// HTTP rate-limit metadata.
///
/// It travels as ordinary task bytes, so the protocol remains version 11.
/// The upstream response body is deliberately omitted: rate-limit pages often
/// contain request identifiers and are not needed to decide when to retry.
fn rate_limit_envelope(seconds: Option<u32>) -> Vec<u8> {
    format!(
        "COBALT-HTTP/1 429\nRetry-After: {}\n\n",
        seconds.map_or_else(|| "-".to_owned(), |seconds| seconds.to_string())
    )
    .into_bytes()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RateLimit {
    No,
    Limited(Option<u32>),
}

/// Reads bounded rate-limit metadata without retaining the response body.
fn rate_limit(response: &[u8]) -> Result<RateLimit, TaskError> {
    let mut headers = [httparse::EMPTY_HEADER; MAX_RESPONSE_HEADERS];
    let mut parsed = httparse::Response::new(&mut headers);
    match parsed.parse(response) {
        Ok(httparse::Status::Complete(end)) if end <= MAX_HEADER_BYTES => {}
        Ok(httparse::Status::Complete(_) | httparse::Status::Partial) | Err(_) => {
            return Err(TaskError::Unreachable);
        }
    }
    if parsed.code != Some(429) {
        return Ok(RateLimit::No);
    }
    Ok(RateLimit::Limited(
        header(parsed.headers, "retry-after")
            .and_then(|value| value.parse::<u32>().ok())
            .map(|seconds| seconds.min(MAX_RETRY_AFTER_SECONDS)),
    ))
}

/// Separates a response into its status code and its body.
///
/// # Errors
///
/// Returns [`TaskError::Unreachable`] if the response is not recognisable
/// HTTP, and [`TaskError::NotFound`] for a 4xx or 5xx status.
pub fn split_response(response: &[u8], max_bytes: u32) -> Result<Response<'_>, TaskError> {
    let mut headers = [httparse::EMPTY_HEADER; MAX_RESPONSE_HEADERS];
    let mut parsed = httparse::Response::new(&mut headers);
    let head_end = match parsed.parse(response) {
        Ok(httparse::Status::Complete(end)) if end <= MAX_HEADER_BYTES => end,
        Ok(httparse::Status::Complete(_) | httparse::Status::Partial) | Err(_) => {
            return Err(TaskError::Unreachable);
        }
    };
    let status = parsed.code.ok_or(TaskError::Unreachable)?;
    let headers = parsed.headers;
    match status {
        200..=299 => {
            let body = &response[head_end..];
            // Chunked is not an exotic case to handle later: every large CDN
            // answers HTTP/1.1 with it, and api.openai.com behind Cloudflare
            // always does. Handing the framing back as if it were the body
            // means the caller sees `1f4\r\n{"id":...` and reports the reply as
            // unreadable, which is exactly how this was found.
            let chunked = transfer_is_chunked(headers)?;
            let length = content_length(headers)?;
            if chunked && length.is_some() {
                return Err(TaskError::Unreachable);
            }
            let framed = if chunked {
                Cow::Owned(decode_chunked(body)?)
            } else if let Some(length) = length {
                if body.len() != length {
                    return Err(TaskError::Unreachable);
                }
                Cow::Borrowed(body)
            } else {
                Cow::Borrowed(body)
            };
            // Framing first, then encoding. A chunked gzip reply is two
            // separate wrappers in a fixed order, and every large CDN sends
            // exactly that: the chunks are how the body was sent, the gzip is
            // what the body is.
            Ok(Response::Body(expanded(headers, framed, max_bytes)?))
        }
        // The range started past the end of the document, which is what asking
        // for the next piece of a book that has just ended looks like. An
        // empty body says "nothing further" to a caller reading in pieces; the
        // alternative is reporting the last page of every book as a failure.
        416 => Ok(Response::Body(Cow::Borrowed(&[]))),
        // 304 carries no body and no Location, so it is not a redirect here.
        301..=303 | 307 | 308 => header(headers, "location")
            .map(|target| Response::Redirect(target.to_string()))
            .ok_or(TaskError::Unreachable),
        // Told apart from every other refusal because it is the one a reader
        // can do something about. A catalogue behind a subscription answers
        // exactly this, and reporting it as "not found" sent them looking for
        // a book rather than for a login.
        401 | 403 => Err(TaskError::Unauthorized),
        400..=599 => Err(TaskError::NotFound),
        _ => Err(TaskError::Unreachable),
    }
}

/// Undoes the content coding the server applied, if it applied one.
///
/// A body with no `Content-Encoding`, or one that says `identity`, is handed
/// straight back and never copied. Anything other than gzip is a coding the
/// runtime never offered to read: rather than hand a caller bytes that look
/// like a truncated document, it is refused the way any unreadable reply is.
fn expanded<'a>(
    headers: &[httparse::Header<'_>],
    body: Cow<'a, [u8]>,
    max_bytes: u32,
) -> Result<Cow<'a, [u8]>, TaskError> {
    let Some(encoding) = header(headers, "content-encoding") else {
        return Ok(body);
    };
    if gzip::is_identity(encoding) {
        return Ok(body);
    }
    if !gzip::is_gzip(encoding) {
        return Err(TaskError::Unreachable);
    }
    gzip::expand(&body, max_bytes).map(Cow::Owned)
}

/// Reads one ASCII header value, matching the name case insensitively.
fn header<'a>(headers: &[httparse::Header<'a>], wanted: &str) -> Option<&'a str> {
    headers.iter().find_map(|header| {
        if header.name.eq_ignore_ascii_case(wanted) {
            std::str::from_utf8(header.value).ok().map(str::trim)
        } else {
            None
        }
    })
}

/// Accepts the one transfer coding this transport implements.
fn transfer_is_chunked(headers: &[httparse::Header<'_>]) -> Result<bool, TaskError> {
    let mut codings = 0_u8;
    for header in headers
        .iter()
        .filter(|header| header.name.eq_ignore_ascii_case("transfer-encoding"))
    {
        let value = std::str::from_utf8(header.value).map_err(|_| TaskError::Unreachable)?;
        for coding in value.split(',') {
            codings = codings.checked_add(1).ok_or(TaskError::Unreachable)?;
            if !coding.trim().eq_ignore_ascii_case("chunked") {
                return Err(TaskError::Unreachable);
            }
        }
    }
    match codings {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(TaskError::Unreachable),
    }
}

/// Reads a unique, decimal content length.
fn content_length(headers: &[httparse::Header<'_>]) -> Result<Option<usize>, TaskError> {
    let mut length = None;
    for header in headers
        .iter()
        .filter(|header| header.name.eq_ignore_ascii_case("content-length"))
    {
        let parsed = std::str::from_utf8(header.value)
            .ok()
            .and_then(|value| value.trim().parse::<usize>().ok())
            .ok_or(TaskError::Unreachable)?;
        if length.is_some_and(|previous| previous != parsed) {
            return Err(TaskError::Unreachable);
        }
        length = Some(parsed);
    }
    Ok(length)
}

/// Resolves a `Location` value against the request it answered.
///
/// Servers are entitled to send a relative target, and a runtime that only
/// understood absolute ones would fail on a large share of real download
/// links. Anything that resolves to something other than `https` is refused,
/// so a redirect cannot quietly downgrade a request to plaintext.
///
/// # Errors
///
/// Returns [`TaskError::NotFound`] for a target this runtime will not follow.
pub fn resolve_redirect(from: &Address, location: &str) -> Result<String, TaskError> {
    if location.starts_with("https://") {
        return Ok(location.to_string());
    }
    // An `http` target is upgraded rather than followed. This is not a
    // relaxation of the rule that a redirect may never downgrade a request:
    // the request still goes over TLS, and a host that does not serve the
    // target over TLS fails rather than falling back.
    //
    // It exists because real servers do this. Project Gutenberg answers
    // `https://www.gutenberg.org/ebooks/2641.txt.utf-8` with a redirect to
    // `http://www.gutenberg.org/cache/epub/2641/pg2641.txt`, and the same file
    // is served perfectly well over TLS. Refusing outright made a large part
    // of the catalogue undownloadable, which is how this was found.
    if let Some(rest) = location.strip_prefix("http://") {
        return Ok(format!("https://{rest}"));
    }
    // Scheme-relative and every other scheme stay refused: the first inherits
    // a scheme rather than stating one, and the rest are not fetches.
    if location.contains("://") || location.starts_with("//") {
        return Err(TaskError::NotFound);
    }
    let base = if from.port == 443 {
        format!("https://{}", from.host)
    } else {
        format!("https://{}:{}", from.host, from.port)
    };
    if location.starts_with('/') {
        return Ok(format!("{base}{location}"));
    }
    let parent = from.path.rsplit_once('/').map_or("/", |(head, _)| head);
    Ok(format!("{base}{parent}/{location}"))
}

/// Reassembles a `Transfer-Encoding: chunked` body.
///
/// The format is a hexadecimal length, an optional `;extension`, CRLF, that
/// many bytes, CRLF, repeated until a zero length. Trailers may follow and are
/// discarded: nothing this runtime does acts on one.
///
/// # Errors
///
/// Returns [`TaskError::Unreachable`] for framing that does not parse or a body
/// that ends mid-chunk. A partial body is not returned as if it were complete,
/// because a caller cannot tell truncated JSON from malformed JSON and would
/// report the wrong thing to the reader.
fn decode_chunked(mut body: &[u8]) -> Result<Vec<u8>, TaskError> {
    let mut out = Vec::with_capacity(body.len());
    loop {
        let (line_end, size) = match httparse::parse_chunk_size(body) {
            Ok(httparse::Status::Complete(parsed)) => parsed,
            Ok(httparse::Status::Partial) | Err(_) => return Err(TaskError::Unreachable),
        };
        let size = usize::try_from(size).map_err(|_| TaskError::Unreachable)?;
        body = &body[line_end..];
        if size == 0 {
            let mut trailers = [httparse::EMPTY_HEADER; MAX_RESPONSE_HEADERS];
            let end = match httparse::parse_headers(body, &mut trailers) {
                Ok(httparse::Status::Complete((end, _))) => end,
                Ok(httparse::Status::Partial) | Err(_) => return Err(TaskError::Unreachable),
            };
            return if end == body.len() {
                Ok(out)
            } else {
                Err(TaskError::Unreachable)
            };
        }
        let framed = size.checked_add(2).ok_or(TaskError::Unreachable)?;
        if body.len() < framed {
            return Err(TaskError::Unreachable);
        }
        out.extend_from_slice(&body[..size]);
        if &body[size..size + 2] != b"\r\n" {
            return Err(TaskError::Unreachable);
        }
        body = &body[framed..];
    }
}

/// Fetches `url`, returning at most `max_bytes` of body.
///
/// # Errors
///
/// Distinguishes the failures an application can act on: a name that does not
/// resolve or a host that refuses is [`TaskError::Unreachable`], a response
/// past the ceiling is [`TaskError::TooLarge`], and a refusal by the server is
/// [`TaskError::NotFound`].
pub fn fetch(url: &str, max_bytes: u32) -> Result<Vec<u8>, TaskError> {
    let cancel = AtomicBool::new(false);
    get(
        url,
        None,
        max_bytes,
        None,
        &[],
        RequestOptions::default(),
        &cancel,
    )
}

/// Fetches `url` starting `offset` bytes in, returning at most `max_bytes`.
///
/// This is what makes a long book readable on a device with a small transport
/// ceiling. A plain-text Gutenberg novel is regularly several times the
/// largest response this runtime will carry, and without a way to ask for the
/// next part the only options are refusing the book or truncating it silently.
///
/// The range is sent for every piece, **including the first**. Asking for the
/// first 256 KB of a book without one means the server sends all 738 KB of it
/// and the ceiling then rejects the answer, so the opening page of any book
/// larger than one chunk could never be read at all.
///
/// A server that ignores the range answers `200` with the whole document, and
/// the ceiling then reports it as too large rather than handing back the
/// beginning of the book labelled as the middle.
///
/// `credential`, when present, is the runtime-owned secret header, kept apart
/// so redirects cannot mistake it for an ordinary application header.
///
/// `headers` are the non-secret headers the application asked for, already
/// checked by the runtime against the ones it owns itself: an application
/// cannot be one of these headers, because `offset` is the only thing
/// permitted to become `Range`, so a `Fetch` could never ask for a piece
/// other than the one the byte ceiling agreed to.
///
/// # Errors
///
/// The same distinctions [`fetch`] makes.
pub fn fetch_from(
    url: &str,
    offset: u32,
    max_bytes: u32,
    credential: Option<(&str, &str)>,
    headers: &[(&str, &str)],
) -> Result<Vec<u8>, TaskError> {
    let cancel = AtomicBool::new(false);
    fetch_from_controlled(
        url,
        offset,
        max_bytes,
        credential,
        headers,
        RequestOptions::default(),
        &cancel,
    )
}

/// The cancellable runtime form of [`fetch_from`].
///
/// Applications cannot call this function: the policy runner supplies both
/// the resolved credential and the runtime-only options.
///
/// # Errors
///
/// Returns the same bounded transport failures as [`fetch_from`], plus
/// cancellation as [`TaskError::TimedOut`] for the policy runner to translate.
pub fn fetch_from_controlled(
    url: &str,
    offset: u32,
    max_bytes: u32,
    credential: Option<(&str, &str)>,
    headers: &[(&str, &str)],
    options: RequestOptions,
    cancel: &AtomicBool,
) -> Result<Vec<u8>, TaskError> {
    // The last gate before the socket, and the same one `post` applies to its
    // own headers: both names and values may ultimately originate outside the
    // runtime, so grammar is checked here rather than trusted from upstream.
    let valid_header = |name: &str, value: &str| {
        name.parse::<http::HeaderName>().is_ok() && value.parse::<http::HeaderValue>().is_ok()
    };
    if credential.is_some_and(|(name, value)| !valid_header(name, value))
        || headers
            .iter()
            .any(|(name, value)| !valid_header(name, value))
    {
        return Err(TaskError::Denied);
    }
    if options.wait_until_cancelled {
        return Err(TaskError::Denied);
    }
    get(
        url,
        Some(offset),
        max_bytes,
        credential,
        headers,
        options,
        cancel,
    )
}

/// The one implementation behind [`fetch`] and [`fetch_from`].
fn get(
    url: &str,
    offset: Option<u32>,
    max_bytes: u32,
    credential: Option<(&str, &str)>,
    headers: &[(&str, &str)],
    options: RequestOptions,
    cancel: &AtomicBool,
) -> Result<Vec<u8>, TaskError> {
    let mut target = url.to_string();
    let mut forwarded = headers;
    for _ in 0..=MAX_REDIRECTS {
        let address = parse(&target)?;
        let response = request(
            &address,
            &Method::Get {
                offset,
                credential,
                headers: forwarded,
                streaming: false,
            },
            max_bytes,
            cancel,
        )?;
        if options.report_rate_limit {
            if let RateLimit::Limited(delay) = rate_limit(&response)? {
                return Ok(rate_limit_envelope(delay));
            }
        }
        match split_response(&response, max_bytes)? {
            Response::Body(body) => {
                return if body.len() > max_bytes as usize {
                    Err(TaskError::TooLarge)
                } else {
                    Ok(body.to_vec())
                };
            }
            Response::Redirect(location) => {
                let next = resolve_redirect(&address, &location)?;
                let next_address = parse(&next)?;
                forwarded = redirect_headers(&address, &next_address, credential, forwarded)?;
                target = next;
            }
        }
    }
    Err(TaskError::Unreachable)
}

/// Keeps ordinary application headers on same-origin redirects. Any redirect
/// carrying a runtime credential is denied: the target cannot be re-authorized
/// after the server chooses it, even when it has the same origin.
fn redirect_headers<'a>(
    current: &Address,
    next: &Address,
    credential: Option<(&str, &str)>,
    headers: &'a [(&'a str, &'a str)],
) -> Result<&'a [(&'a str, &'a str)], TaskError> {
    if credential.is_some() {
        return Err(TaskError::Denied);
    }
    if next.host.eq_ignore_ascii_case(&current.host) && next.port == current.port {
        return Ok(headers);
    }
    Ok(&[])
}

/// What is sent, beyond the address.
enum Method<'a> {
    Get {
        /// Where to start reading, as a byte offset, or `None` to ask for the
        /// whole document with no range header at all.
        offset: Option<u32>,
        /// The runtime-owned credential, kept separate from application
        /// headers so redirect handling retains its provenance.
        credential: Option<(&'a str, &'a str)>,
        /// Further headers the request needs, none of them secret and none of
        /// them `Range`: that one is derived from `offset` alone, in [`head`],
        /// so an application cannot widen or move the piece it was granted.
        headers: &'a [(&'a str, &'a str)],
        /// A retained line stream asks for identity encoding and is never put
        /// in the ordinary idle-connection cache.
        streaming: bool,
    },
    Post {
        body: &'a [u8],
        content_type: &'a str,
        /// The credential header, already assembled as a name and its value.
        credential: Option<(&'a str, &'a str)>,
        /// Further headers the request needs, none of them secret.
        headers: &'a [(&'a str, &'a str)],
    },
}

impl Method<'_> {
    fn verb(&self) -> &'static str {
        match self {
            Self::Get { .. } => "GET",
            Self::Post { .. } => "POST",
        }
    }
}

/// Sends `body` to `url` and returns at most `max_bytes` of the answer.
///
/// `credential`, when present, is a header name and its complete value, the
/// caller decides whether that is `Authorization: Bearer …`, `x-api-key: …` or
/// something else, because the convention differs by service and choosing one
/// here would mean every other service needs a proxy in front of it.
///
/// It is taken as a parameter rather than read from anywhere here because the
/// only caller that has one is the runtime: an application names a secret and
/// never sees its value, so a credential cannot leak through an application's
/// own memory, logs or crash dump.
///
/// Redirects are deliberately **not** followed. Replaying a body at whatever
/// address a server names is how a request meant for one host ends up, headers
/// and credential included, at another.
///
/// # Errors
///
/// The same distinctions [`fetch`] makes: [`TaskError::Unreachable`] for a
/// host that cannot be reached, [`TaskError::TooLarge`] past the ceiling, and
/// [`TaskError::NotFound`] for a refusal by the server.
pub fn post(
    url: &str,
    body: &[u8],
    content_type: &str,
    credential: Option<(&str, &str)>,
    headers: &[(&str, &str)],
    max_bytes: u32,
) -> Result<Vec<u8>, TaskError> {
    let cancel = AtomicBool::new(false);
    post_controlled(
        url,
        body,
        content_type,
        credential,
        headers,
        max_bytes,
        RequestOptions::default(),
        &cancel,
    )
}

/// The cancellable runtime form of [`post`].
///
/// A wait-until-cancelled POST is sent exactly once. Once the server accepts
/// it, body bytes are discarded until the task is cancelled; neither stale
/// connections nor transport failures ever replay the request.
///
/// # Errors
///
/// Returns the same bounded transport failures as [`post`]. Cancellation is
/// returned as [`TaskError::TimedOut`] so the policy runner can report the
/// task's explicit cancelled outcome.
#[allow(
    clippy::too_many_arguments,
    reason = "the runtime transport boundary keeps body, credential, headers, limits, options, and cancellation explicit"
)]
pub fn post_controlled(
    url: &str,
    body: &[u8],
    content_type: &str,
    credential: Option<(&str, &str)>,
    headers: &[(&str, &str)],
    max_bytes: u32,
    options: RequestOptions,
    cancel: &AtomicBool,
) -> Result<Vec<u8>, TaskError> {
    // Header grammar is checked by the same maintained types used for URI
    // syntax. This is the last gate before the socket, and both names and
    // values may ultimately originate outside the runtime.
    let valid_header = |name: &str, value: &str| {
        name.parse::<http::HeaderName>().is_ok() && value.parse::<http::HeaderValue>().is_ok()
    };
    if let Some((name, value)) = credential {
        if !valid_header(name, value) {
            return Err(TaskError::Denied);
        }
    }
    if headers
        .iter()
        .any(|(name, value)| !valid_header(name, value))
    {
        return Err(TaskError::Denied);
    }
    if content_type.parse::<http::HeaderValue>().is_err() {
        return Err(TaskError::Denied);
    }
    let address = parse(url)?;
    if options.wait_until_cancelled {
        return post_until_cancelled(
            &address,
            body,
            content_type,
            credential,
            headers,
            options,
            cancel,
        );
    }
    let response = request(
        &address,
        &Method::Post {
            body,
            content_type,
            credential,
            headers,
        },
        max_bytes,
        cancel,
    )?;
    if options.report_rate_limit {
        if let RateLimit::Limited(delay) = rate_limit(&response)? {
            return Ok(rate_limit_envelope(delay));
        }
    }
    match split_response(&response, max_bytes)? {
        Response::Body(body) => {
            if body.len() > max_bytes as usize {
                Err(TaskError::TooLarge)
            } else {
                Ok(body.to_vec())
            }
        }
        Response::Redirect(_) => Err(TaskError::NotFound),
    }
}

fn post_until_cancelled(
    address: &Address,
    body: &[u8],
    content_type: &str,
    credential: Option<(&str, &str)>,
    headers: &[(&str, &str)],
    options: RequestOptions,
    cancel: &AtomicBool,
) -> Result<Vec<u8>, TaskError> {
    let method = Method::Post {
        body,
        content_type,
        credential,
        headers,
    };
    let mut held = connect(address)?;
    let mut tls = rustls::Stream::new(&mut held.connection, &mut held.socket);
    tls.write_all(head(address, &method, 1).as_bytes())
        .and_then(|()| tls.write_all(body))
        .and_then(|()| tls.flush())
        .map_err(|_| TaskError::Unreachable)?;

    let mut response = Vec::new();
    let mut buffer = [0_u8; 4096];
    let mut last_progress = Instant::now();
    let status = loop {
        if cancel.load(Ordering::SeqCst) {
            return Err(TaskError::TimedOut);
        }
        let mut parsed_headers = [httparse::EMPTY_HEADER; MAX_RESPONSE_HEADERS];
        let mut parsed = httparse::Response::new(&mut parsed_headers);
        match parsed.parse(&response) {
            Ok(httparse::Status::Complete(end)) if end <= MAX_HEADER_BYTES => {
                let status = parsed.code.ok_or(TaskError::Unreachable)?;
                let delay = (status == 429).then(|| {
                    header(parsed.headers, "retry-after")
                        .and_then(|value| value.parse::<u32>().ok())
                        .map(|seconds| seconds.min(MAX_RETRY_AFTER_SECONDS))
                });
                break (status, delay);
            }
            Ok(httparse::Status::Complete(_)) | Err(_) => return Err(TaskError::Unreachable),
            Ok(httparse::Status::Partial) => {}
        }
        match tls.read(&mut buffer) {
            Ok(0) => return Err(TaskError::Unreachable),
            Ok(read) => {
                response.extend_from_slice(&buffer[..read]);
                last_progress = Instant::now();
                if response.len() > MAX_HEADER_BYTES {
                    return Err(TaskError::Unreachable);
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                if last_progress.elapsed() >= RESPONSE_TIMEOUT {
                    return Err(TaskError::TimedOut);
                }
            }
            Err(_) => return Err(TaskError::Unreachable),
        }
    };

    match status {
        (200..=299, _) => {}
        (401 | 403, _) => return Err(TaskError::Unauthorized),
        (429, Some(delay)) if options.report_rate_limit => {
            return Ok(rate_limit_envelope(delay));
        }
        (301..=303 | 307 | 308 | 400..=599, _) => return Err(TaskError::NotFound),
        _ => return Err(TaskError::Unreachable),
    }

    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err(TaskError::TimedOut);
        }
        match tls.read(&mut buffer) {
            Ok(0) => return Err(TaskError::Unreachable),
            Ok(_) => last_progress = Instant::now(),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                if last_progress.elapsed() >= RESPONSE_TIMEOUT {
                    return Err(TaskError::TimedOut);
                }
            }
            Err(_) => return Err(TaskError::Unreachable),
        }
    }
}

/// Builds the request line and headers.
///
/// Separated from the socket so it can be tested. The bug this exists to stop
/// recurring was invisible from the outside: a missing range header on the
/// first piece of a document, which no test could see because it only showed
/// up as a server sending more than the ceiling allowed.
fn head(address: &Address, method: &Method<'_>, max_bytes: u32) -> String {
    let (verb, path, host) = (
        method.verb(),
        address.path.as_str(),
        address.authority.as_str(),
    );
    // A range is a range of the bytes the server sends, so a ranged request
    // that is answered compressed asks for a window into a deflate stream and
    // gets a fragment that cannot be expanded on its own. Every caller reading
    // a document in pieces is counting the bytes it asked for, too. So the
    // reader that pages through a book keeps asking for the bytes as written,
    // and everything else, which is every JSON API here, takes the fifth of
    // the bytes that gzip costs instead.
    let encoding = match method {
        Method::Get {
            offset: Some(_), ..
        }
        | Method::Get {
            streaming: true, ..
        } => "identity",
        Method::Get {
            offset: None,
            streaming: false,
            ..
        }
        | Method::Post { .. } => "gzip",
    };
    // A POST hangs up after its answer; a GET does not. HTTP/1.1 is
    // persistent by default, so the difference is whether `close` is said at
    // all. Only a GET is ever replayed on a connection the far end had quietly
    // dropped, so only a GET is allowed to hold one open.
    let connection = match method {
        Method::Get { .. } => "keep-alive",
        Method::Post { .. } => "close",
    };
    let mut head = format!(
        "{verb} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: {connection}\r\nAccept-Encoding: {encoding}\r\nUser-Agent: kobo-runtime\r\n"
    );
    match method {
        Method::Get {
            offset,
            credential,
            headers,
            ..
        } => {
            // Closed at both ends, because an open-ended range invites the
            // server to send the rest of a book that does not fit. Sent for
            // the first piece as well as later ones: a request for the opening
            // 256 KB of a 738 KB novel without a range is answered with the
            // whole novel, and then rejected by the ceiling.
            if let Some(start) = offset {
                let last = u64::from(*start) + u64::from(max_bytes.saturating_sub(1));
                write!(head, "Range: bytes={start}-{last}\r\n")
                    .expect("writing to a String cannot fail");
            }
            if let Some((name, value)) = credential {
                write!(head, "{name}: {value}\r\n").expect("writing to a String cannot fail");
            }
            // Written after `Range`, and never able to replace it: `headers`
            // here has already been through the same reserved-name gate
            // `Post`'s headers pass through, so `Range` can never be one of
            // these pairs in the first place.
            for (name, value) in *headers {
                write!(head, "{name}: {value}\r\n").expect("writing to a String cannot fail");
            }
        }
        Method::Post {
            body,
            content_type,
            credential,
            headers,
        } => {
            if let Some((name, value)) = credential {
                write!(head, "{name}: {value}\r\n").expect("writing to a String cannot fail");
            }
            for (name, value) in *headers {
                write!(head, "{name}: {value}\r\n").expect("writing to a String cannot fail");
            }
            write!(
                head,
                "Content-Type: {content_type}\r\nContent-Length: {}\r\n",
                body.len()
            )
            .expect("writing to a String cannot fail");
        }
    }
    head.push_str("\r\n");
    head
}

/// Performs one request and returns the whole response, headers included.
/// The TLS configuration, built once for the life of the process.
///
/// Not for the reason it looks like. Building one costs about five
/// microseconds, so copying the webpki root store per request was never the
/// problem, and it is worth saying so rather than leaving a plausible wrong
/// answer lying next to a right one.
///
/// The cost was that rustls keeps its TLS session store *inside* the config.
/// A config discarded after one request discards the resumption tickets with
/// it, so every request paid a full handshake -- an extra round trip and the
/// asymmetric signature verification -- even when it was the sixth cover from
/// the host we had been talking to a second earlier. That verification is the
/// expensive half on a 1 GHz ARM core.
///
/// Measured over four runs of five sequential requests to gutenberg.org:
/// 7.7s with a config per request against 6.1s with one shared, consistently
/// around a fifth faster, on a machine far quicker than the reader.
static TLS_CONFIG: OnceLock<Arc<rustls::ClientConfig>> = OnceLock::new();

/// Roots the owner has installed beside the ones every browser carries.
///
/// Empty on almost every reader. It exists for the one case public roots
/// cannot serve: a daemon on the owner's own machine, on the owner's own
/// network, holding a certificate no public authority would sign for a
/// private address. The owner installs that certificate once, over the same
/// attended channel that installs a credential, and the runtime then verifies
/// the daemon exactly as it verifies any public host -- rather than being
/// handed a switch that turns verification off.
///
/// Held apart from [`TLS_CONFIG`] because installation happens once at
/// runtime start, before any request; after the shared configuration is
/// built, further installs are refused rather than silently ignored.
static OWNER_ROOTS: Mutex<Vec<rustls::pki_types::CertificateDer<'static>>> = Mutex::new(Vec::new());

/// Installs one DER certificate as a trust root for this process.
///
/// # Errors
///
/// Refuses a certificate the verifier cannot use as an anchor, and refuses
/// every certificate once the TLS configuration has been built, because a
/// root added after that point would appear installed while never being
/// consulted.
pub fn trust_owner_root(certificate: Vec<u8>) -> Result<(), TaskError> {
    if TLS_CONFIG.get().is_some() {
        return Err(TaskError::Denied);
    }
    let der = rustls::pki_types::CertificateDer::from(certificate);
    // Proven usable as an anchor now, so a corrupt file fails at install
    // time with a message, rather than at request time as `Unreachable`.
    let mut probe = rustls::RootCertStore::empty();
    probe.add(der.clone()).map_err(|_| TaskError::Denied)?;
    let mut roots = OWNER_ROOTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    roots.push(der);
    Ok(())
}

/// Installs every certificate found in a directory of `.pem` or `.der` files.
///
/// Returns how many roots were installed. A missing or empty directory is
/// zero rather than an error: almost no reader has one, and the runtime
/// calls this unconditionally at start.
#[must_use]
pub fn trust_owner_roots_from_dir(directory: &std::path::Path) -> usize {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return 0;
    };
    let mut installed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let ders = match path.extension().and_then(|extension| extension.to_str()) {
            Some("pem") => pem::certificates(&String::from_utf8_lossy(&bytes)),
            Some("der") => vec![bytes],
            _ => continue,
        };
        for der in ders {
            if trust_owner_root(der).is_ok() {
                installed += 1;
            }
        }
    }
    installed
}

fn tls_config() -> Result<Arc<rustls::ClientConfig>, TaskError> {
    if let Some(config) = TLS_CONFIG.get() {
        return Ok(Arc::clone(config));
    }
    let mut roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    {
        let owner = OWNER_ROOTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for der in owner.iter() {
            // Already proven addable at install time; a failure here would
            // mean the certificate changed while held, which it cannot.
            let _ = roots.add(der.clone());
        }
    }
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|_| TaskError::Unreachable)?
    .with_root_certificates(roots)
    .with_no_client_auth();
    // Whichever call built it first wins; the loser drops its own copy. Both
    // are the same configuration, so it does not matter which.
    Ok(Arc::clone(TLS_CONFIG.get_or_init(|| Arc::new(config))))
}

/// One open connection to one origin, TLS session and socket together.
///
/// The two cannot be separated: a rustls session is the encryption state of
/// exactly one socket, and either one alone is useless.
struct Held {
    connection: rustls::ClientConnection,
    socket: TcpStream,
}

/// Connections that finished a request cleanly and could carry another.
///
/// # Why this exists
///
/// Every request used to open a socket, complete a TLS handshake, ask one
/// question and hang up. Hacker News is read one item at a time because that
/// is the only shape its API has, so a screen of comments was two dozen
/// handshakes; a shelf of book covers and a round of feeds are the same. On
/// this reader a handshake is the expensive part by a wide margin: the radio
/// has to be awake for a round trip it did not need, and the signature
/// arithmetic runs on a processor from 2013.
///
/// # Why it is this small
///
/// One connection per origin, not a pool of them. These applications ask one
/// question at a time, so a second connection to the same host would sit idle;
/// what matters is that the second question does not pay for a second
/// handshake, and one is enough for that.
static IDLE: Mutex<Option<Kept>> = Mutex::new(None);

/// The one held connection, and what it would have to match to be used.
struct Kept {
    host: String,
    port: u16,
    held: Held,
    since: Instant,
}

/// How long a connection nobody used is still worth trying.
///
/// Servers close idle connections on their own schedule and are under no
/// obligation to say so, so this is a guess either way and a stale one costs
/// only the reconnection it was trying to avoid. Kept well under the shortest
/// keep-alive any large host advertises.
const IDLE_LIMIT: Duration = Duration::from_secs(20);

/// Takes the kept connection if it is to this origin and still fresh.
fn take_idle(address: &Address) -> Option<Held> {
    let mut idle = IDLE.lock().ok()?;
    let kept = idle.as_ref()?;
    if kept.host != address.host || kept.port != address.port || kept.since.elapsed() > IDLE_LIMIT {
        return None;
    }
    idle.take().map(|kept| kept.held)
}

/// Keeps a connection for the next request, displacing any other.
fn keep_idle(address: &Address, held: Held) {
    if let Ok(mut idle) = IDLE.lock() {
        *idle = Some(Kept {
            host: address.host.clone(),
            port: address.port,
            held,
            since: Instant::now(),
        });
    }
}

/// Why an exchange did not produce a response.
enum Failed {
    /// The far end had already closed a connection this borrowed from the
    /// pool. Nothing was said, so it can be said again on a new socket.
    Stale,
    /// Something that would have happened on a new connection too.
    Real(TaskError),
}

fn request(
    address: &Address,
    method: &Method<'_>,
    max_bytes: u32,
    cancel: &AtomicBool,
) -> Result<Vec<u8>, TaskError> {
    // Only a GET is replayed. A POST that failed after leaving this machine
    // may have been acted on by the far end, and asking a model to answer
    // twice or a daemon to run a command twice is a worse failure than the one
    // being recovered from.
    let reusable = matches!(method, Method::Get { .. });
    if reusable {
        if let Some(mut held) = take_idle(address) {
            match exchange(&mut held, address, method, max_bytes, cancel) {
                Ok((response, again)) => {
                    if again {
                        keep_idle(address, held);
                    }
                    return Ok(response);
                }
                Err(Failed::Stale) => {}
                Err(Failed::Real(error)) => return Err(error),
            }
        }
    }
    let mut held = connect(address)?;
    match exchange(&mut held, address, method, max_bytes, cancel) {
        Ok((response, again)) => {
            if reusable && again {
                keep_idle(address, held);
            }
            Ok(response)
        }
        // A fresh connection that ended before it said anything is a host
        // that will not talk, not a connection to try again.
        Err(Failed::Stale) => Err(TaskError::Unreachable),
        Err(Failed::Real(error)) => Err(error),
    }
}

/// Opens a socket to `address` and completes a TLS handshake on it.
fn connect(address: &Address) -> Result<Held, TaskError> {
    let config = tls_config()?;
    let name = address
        .host
        .clone()
        .try_into()
        .map_err(|_| TaskError::NotFound)?;
    // The session cache lives on the shared configuration, so a second
    // connection to a host this reader has already met resumes rather than
    // doing the full handshake again. That is why the configuration is built
    // once and cloned rather than built per request.
    let connection =
        rustls::ClientConnection::new(config, name).map_err(|_| TaskError::Unreachable)?;
    // The two places a request fails before anything has been said. Both are
    // asked which kind of failure it was, because a name that will not resolve
    // and a socket that will not open are the same event on a reader whose
    // radio is off, and different events on one that is on a network.
    let mut addresses = (address.host.as_str(), address.port)
        .to_socket_addrs()
        .map_err(|_| could_not_connect())?;
    let socket = addresses
        .find_map(|address| TcpStream::connect_timeout(&address, CONNECT_TIMEOUT).ok())
        .ok_or_else(could_not_connect)?;
    socket
        .set_read_timeout(Some(IO_POLL))
        .and_then(|()| socket.set_write_timeout(Some(RESPONSE_TIMEOUT)))
        .map_err(|_| TaskError::Unreachable)?;
    // Nagle's algorithm holds a small write back waiting for company. A
    // request head is exactly that write, and there is no company coming.
    let _ = socket.set_nodelay(true);
    Ok(Held { connection, socket })
}

/// Sends one request on `held` and reads exactly one response back.
///
/// Returns the response bytes and whether the connection may carry another.
fn exchange(
    held: &mut Held,
    address: &Address,
    method: &Method<'_>,
    max_bytes: u32,
    cancel: &AtomicBool,
) -> Result<(Vec<u8>, bool), Failed> {
    let mut tls = rustls::Stream::new(&mut held.connection, &mut held.socket);
    // A write failure here is the ordinary way a pooled connection reports
    // that the far end closed it while it was idle, so it is not fatal on its
    // own; the caller decides, knowing whether this socket was reused.
    tls.write_all(head(address, method, max_bytes).as_bytes())
        .map_err(|_| Failed::Stale)?;
    if let Method::Post { body, .. } = method {
        tls.write_all(body).map_err(|_| Failed::Stale)?;
    }
    tls.flush().map_err(|_| Failed::Stale)?;

    // The ceiling is applied to the whole response as it arrives, so a server
    // that never stops sending cannot fill memory before the body is examined.
    let ceiling = (max_bytes as usize).saturating_add(MAX_HEADER_BYTES);
    let mut response = Vec::new();
    let mut buffer = [0_u8; 8192];
    let mut last_progress = Instant::now();
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err(Failed::Real(TaskError::TimedOut));
        }
        // Asked before each read rather than after, so a response that is
        // already complete never waits on a socket that has nothing more to
        // send. Reading to the end of the socket is what made every request
        // need its own connection.
        match message_end(&response) {
            Ok(Some(end)) => {
                response.truncate(end);
                let again = stays_open(&response);
                return Ok((response, again));
            }
            Ok(None) => {}
            Err(error) => return Err(Failed::Real(error)),
        }
        match tls.read(&mut buffer) {
            // The far end hung up. If it had already sent a whole response
            // the loop above would have returned, so this is either a
            // response framed by the close itself or nothing at all.
            Ok(0) => {
                return if response.is_empty() {
                    Err(Failed::Stale)
                } else {
                    Ok((response, false))
                }
            }
            Ok(read) => {
                response.extend_from_slice(&buffer[..read]);
                last_progress = Instant::now();
                if response.len() > ceiling {
                    return Err(Failed::Real(TaskError::TooLarge));
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                if last_progress.elapsed() >= RESPONSE_TIMEOUT {
                    return Err(Failed::Real(TaskError::TimedOut));
                }
            }
            Err(_) if response.is_empty() => return Err(Failed::Stale),
            Err(_) => return Ok((response, false)),
        }
    }
}

/// Where this response ends, if enough of it has arrived to say.
///
/// `Ok(None)` means the answer is not yet knowable and more bytes are needed.
/// A response the server frames by closing the connection is never knowable,
/// so it stays `Ok(None)` until the socket ends, which is the old behaviour
/// kept for the servers that still do that.
fn message_end(response: &[u8]) -> Result<Option<usize>, TaskError> {
    let mut headers = [httparse::EMPTY_HEADER; MAX_RESPONSE_HEADERS];
    let mut parsed = httparse::Response::new(&mut headers);
    let head_end = match parsed.parse(response) {
        Ok(httparse::Status::Complete(end)) if end <= MAX_HEADER_BYTES => end,
        Ok(httparse::Status::Partial) => return Ok(None),
        Ok(httparse::Status::Complete(_)) | Err(_) => return Err(TaskError::Unreachable),
    };
    let status = parsed.code.ok_or(TaskError::Unreachable)?;
    // Two statuses are defined to carry no body at all, whatever their headers
    // say. Waiting for one is waiting for something nobody is sending.
    if matches!(status, 204 | 304) {
        return Ok(Some(head_end));
    }
    let body = &response[head_end..];
    if transfer_is_chunked(parsed.headers)? {
        return Ok(chunked_end(body)?.map(|end| head_end + end));
    }
    match content_length(parsed.headers)? {
        Some(length) => Ok(head_end
            .checked_add(length)
            .filter(|end| *end <= response.len())),
        None => Ok(None),
    }
}

/// Where a chunked body ends, if its last chunk has arrived.
///
/// This walks the same framing [`decode_chunked`] walks, without copying any
/// of it. Anything incomplete is `None` rather than an error: a body arrives a
/// packet at a time, and every one of those packets ends mid-chunk.
fn chunked_end(body: &[u8]) -> Result<Option<usize>, TaskError> {
    let mut at = 0_usize;
    loop {
        let rest = body.get(at..).ok_or(TaskError::Unreachable)?;
        let (line_end, size) = match httparse::parse_chunk_size(rest) {
            Ok(httparse::Status::Complete(parsed)) => parsed,
            Ok(httparse::Status::Partial) => return Ok(None),
            Err(_) => return Err(TaskError::Unreachable),
        };
        let size = usize::try_from(size).map_err(|_| TaskError::Unreachable)?;
        at = at.checked_add(line_end).ok_or(TaskError::Unreachable)?;
        if size == 0 {
            let rest = body.get(at..).ok_or(TaskError::Unreachable)?;
            let mut trailers = [httparse::EMPTY_HEADER; MAX_RESPONSE_HEADERS];
            return match httparse::parse_headers(rest, &mut trailers) {
                Ok(httparse::Status::Complete((end, _))) => Ok(Some(at + end)),
                Ok(httparse::Status::Partial) => Ok(None),
                Err(_) => Err(TaskError::Unreachable),
            };
        }
        at = at
            .checked_add(size)
            .and_then(|at| at.checked_add(2))
            .ok_or(TaskError::Unreachable)?;
        if at > body.len() {
            return Ok(None);
        }
    }
}

/// Whether the server is willing to answer again on this connection.
///
/// HTTP/1.1 is persistent unless somebody says otherwise, and HTTP/1.0 is the
/// reverse. Both are honoured, because a connection kept against a server that
/// has already decided to close it is a guaranteed retry on the next request.
fn stays_open(response: &[u8]) -> bool {
    let mut headers = [httparse::EMPTY_HEADER; MAX_RESPONSE_HEADERS];
    let mut parsed = httparse::Response::new(&mut headers);
    if !matches!(parsed.parse(response), Ok(httparse::Status::Complete(_))) {
        return false;
    }
    let said = header(parsed.headers, "connection").unwrap_or_default();
    if said
        .split(',')
        .any(|token| token.trim().eq_ignore_ascii_case("close"))
    {
        return false;
    }
    match parsed.version {
        Some(0) => said
            .split(',')
            .any(|token| token.trim().eq_ignore_ascii_case("keep-alive")),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    /// A ceiling for tests that are about framing rather than size. Large
    /// enough that nothing in this module ever reaches it.
    const CEILING: u32 = 64 * 1024;

    #[test]
    fn runtime_credentials_are_refused_on_any_redirect() {
        let zotero_current =
            super::parse("https://api.zotero.org/users/123/items/ABCD2345?format=json")
                .expect("Zotero item route");
        let zotero_next =
            super::parse("https://api.zotero.org/users/123/items/EFGH6789?format=json")
                .expect("Zotero full-text route");
        let other_current =
            super::parse("https://api.openai.com/v1/chat/completions").expect("OpenAI route");
        let other_next =
            super::parse("https://api.openai.com/v1/responses").expect("same-origin OpenAI route");
        let other_origin = super::parse("https://attacker.invalid/collect").expect("other origin");
        for credential in [
            ("Authorization", "Bearer private"),
            ("x-api-key", "private"),
            ("x-goog-api-key", "private"),
            ("xi-api-key", "private"),
        ] {
            for (current, next) in [
                (&zotero_current, &zotero_next),
                (&other_current, &other_next),
                (&other_current, &other_origin),
            ] {
                assert_eq!(
                    super::redirect_headers(current, next, Some(credential), &[]),
                    Err(TaskError::Denied)
                );
            }
        }
    }

    #[test]
    fn ordinary_headers_survive_same_origin_redirects() {
        let current =
            super::parse("https://api.openai.com/v1/chat/completions").expect("OpenAI route");
        let same_origin =
            super::parse("https://api.openai.com/v1/responses").expect("same-origin route");
        let other_origin = super::parse("https://attacker.invalid/collect").expect("other origin");
        let ordinary = [("Accept", "application/json")];

        assert_eq!(
            super::redirect_headers(&current, &same_origin, None, &ordinary),
            Ok(&ordinary[..])
        );
        assert_eq!(
            super::redirect_headers(&current, &other_origin, None, &ordinary),
            Ok(&[][..])
        );
    }

    use super::has_default_route;

    /// Read off a Clara BW on Wi-Fi, header and spacing untouched.
    const READER_ONLINE: &str = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\nwlan0\t00000000\t0101A8C0\t0003\t0\t0\t307\t00000000\t0\t0\t0\nwlan0\t0001A8C0\t00000000\t0001\t0\t0\t307\t00FFFFFF\t0\t0\t0\n";

    #[test]
    fn a_reader_on_wifi_has_a_route() {
        assert!(has_default_route(READER_ONLINE));
    }

    #[test]
    fn a_reader_with_only_a_subnet_route_has_no_way_out() {
        // The second line of the real table on its own. A reader that can
        // reach its own subnet and nothing else cannot fetch anything, and
        // calling that reachable would send the app looking for a host to
        // blame.
        let table = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\nwlan0\t0001A8C0\t00000000\t0001\t0\t0\t307\t00FFFFFF\t0\t0\t0\n";
        assert!(!has_default_route(table));
    }

    #[test]
    fn loopback_is_not_a_network() {
        // What a reader with its radio powered down is left holding.
        let table = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\nlo\t00000000\t00000000\t0003\t0\t0\t0\t00000000\t0\t0\t0\n";
        assert!(!has_default_route(table));
    }

    #[test]
    fn a_route_that_is_not_up_does_not_count() {
        // Flags without bit one set: the entry is in the table but the kernel
        // will not send anything down it.
        let table = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\nwlan0\t00000000\t0101A8C0\t0002\t0\t0\t307\t00000000\t0\t0\t0\n";
        assert!(!has_default_route(table));
    }

    #[test]
    fn an_empty_or_broken_table_is_not_read_as_a_network() {
        assert!(!has_default_route(""));
        assert!(!has_default_route("Iface\tDestination\tGateway\tFlags\n"));
        assert!(!has_default_route("nonsense"));
        assert!(!has_default_route("Iface\nwlan0\t00000000\n"));
    }

    #[test]
    fn a_second_interface_keeps_its_own_route() {
        // A reader on both Wi-Fi and a USB network. Only one needs a way out.
        let table = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\nusb0\t0002A8C0\t00000000\t0001\t0\t0\t0\t00FFFFFF\t0\t0\t0\nwlan0\t00000000\t0101A8C0\t0003\t0\t0\t307\t00000000\t0\t0\t0\n";
        assert!(has_default_route(table));
    }

    /// Sharing the configuration is the whole optimisation, so it is worth a
    /// test that fails if someone moves it back inside `request`.
    ///
    /// Pointer equality is the assertion rather than anything about the
    /// contents, because what matters is that it is the *same* config: that is
    /// what carries the TLS session store, and therefore what lets the second
    /// request to a host resume instead of handshaking from nothing.
    #[test]
    fn every_request_shares_one_tls_configuration() {
        let first = super::tls_config().expect("a usable TLS configuration");
        let second = super::tls_config().expect("a usable TLS configuration");
        assert!(
            std::sync::Arc::ptr_eq(&first, &second),
            "the TLS config must be built once and shared, or session resumption is impossible"
        );
    }

    #[test]
    fn a_reply_is_not_finished_until_its_content_length_has_arrived() {
        let head = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n";
        assert_eq!(message_end(b"HTTP/1.1 200 OK\r\nContent-Len"), Ok(None));
        assert_eq!(message_end(head), Ok(None));
        assert_eq!(
            message_end(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhel"),
            Ok(None)
        );
        let whole = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello";
        assert_eq!(message_end(whole), Ok(Some(whole.len())));
    }

    #[test]
    fn a_reply_ends_where_it_said_it_would_and_not_where_the_socket_does() {
        // The bug this stops: a kept connection carrying the front of the
        // next answer, or a server that pads, silently becoming part of a
        // body an application then tries to parse.
        let response = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhelloHTTP/1.1 200 OK\r\n";
        let end = message_end(response)
            .expect("a framed reply")
            .expect("a complete reply");
        assert_eq!(
            &response[..end],
            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello"
        );
    }

    #[test]
    fn the_two_statuses_that_carry_no_body_are_not_waited_on() {
        for response in [
            &b"HTTP/1.1 204 No Content\r\nContent-Length: 9\r\n\r\n"[..],
            &b"HTTP/1.1 304 Not Modified\r\nContent-Length: 9\r\n\r\n"[..],
        ] {
            assert_eq!(
                message_end(response),
                Ok(Some(response.len())),
                "a bodiless status must not be read as owing a body"
            );
        }
    }

    #[test]
    fn a_reply_with_no_framing_at_all_ends_only_when_the_socket_does() {
        let response = b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\nhello";
        assert_eq!(
            message_end(response),
            Ok(None),
            "without a length or chunking the close is the framing, so the read must go on"
        );
    }

    #[test]
    fn a_chunked_reply_is_finished_by_its_terminator_and_not_before() {
        let head = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n";
        let mut response = head.to_vec();
        assert_eq!(message_end(&response), Ok(None));
        response.extend_from_slice(b"5\r\nhello\r\n");
        assert_eq!(message_end(&response), Ok(None));
        response.extend_from_slice(b"0\r\n");
        assert_eq!(
            message_end(&response),
            Ok(None),
            "a last chunk still owes its trailer terminator"
        );
        response.extend_from_slice(b"\r\n");
        assert_eq!(message_end(&response), Ok(Some(response.len())));
    }

    #[test]
    fn a_chunked_reply_carries_its_trailers_to_the_end() {
        let response =
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\nDigest: x\r\n\r\n";
        assert_eq!(message_end(response), Ok(Some(response.len())));
    }

    #[test]
    fn a_reply_that_cannot_be_parsed_is_refused_rather_than_waited_on() {
        assert_eq!(
            message_end(b"nonsense\r\n\r\n"),
            Err(TaskError::Unreachable)
        );
        assert_eq!(
            message_end(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n"),
            Err(TaskError::Unreachable)
        );
    }

    #[test]
    fn a_connection_is_kept_only_where_the_server_agrees_to_it() {
        // HTTP/1.1 is persistent unless the server says otherwise; 1.0 is the
        // reverse. Keeping one the server has already closed costs a whole
        // extra round trip on the next request, so both are honoured.
        assert!(stays_open(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n"));
        assert!(!stays_open(
            b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
        ));
        assert!(!stays_open(
            b"HTTP/1.1 200 OK\r\nConnection: keep-alive, close\r\nContent-Length: 0\r\n\r\n"
        ));
        assert!(!stays_open(b"HTTP/1.0 200 OK\r\nContent-Length: 0\r\n\r\n"));
        assert!(stays_open(
            b"HTTP/1.0 200 OK\r\nConnection: Keep-Alive\r\nContent-Length: 0\r\n\r\n"
        ));
    }

    #[test]
    fn only_a_get_offers_to_hold_its_connection_open() {
        // A POST replayed on a connection the far end had dropped could ask a
        // model to answer twice or a daemon to run a command twice, so it
        // hangs up rather than risk being the request that gets repeated.
        let address = book();
        assert!(head(
            &address,
            &Method::Get {
                offset: None,
                credential: None,
                headers: &[],
                streaming: false,
            },
            1024
        )
        .contains("Connection: keep-alive"));
        assert!(head(
            &address,
            &Method::Get {
                offset: Some(1024),
                credential: None,
                headers: &[],
                streaming: false,
            },
            1024
        )
        .contains("Connection: keep-alive"));
        assert!(head(
            &address,
            &Method::Post {
                body: b"{}",
                content_type: "application/json",
                credential: None,
                headers: &[],
            },
            1024
        )
        .contains("Connection: close"));
    }

    use super::{
        fetch, head, message_end, parse, post, resolve_redirect, split_response, stays_open,
        Address, Cow, Method, Response,
    };
    use kobo_protocol::TaskError;

    fn book() -> Address {
        Address {
            host: "www.gutenberg.org".into(),
            port: 443,
            path: "/files/1342/1342-0.txt".into(),
            authority: "www.gutenberg.org".into(),
        }
    }

    #[test]
    fn the_first_piece_of_a_document_is_asked_for_by_range_like_every_other_piece() {
        // This is the whole reason a long book can be opened at all. Without a
        // range on the first request the server sends all 738 KB of Pride and
        // Prejudice, the 256 KB ceiling rejects it, and the opening page of
        // every book larger than one piece is unreachable. The symptom on the
        // device was a download that appeared to hang.
        let request = head(
            &book(),
            &Method::Get {
                offset: Some(0),
                credential: None,
                headers: &[],
                streaming: false,
            },
            262_144,
        );
        assert!(
            request.contains("\r\nRange: bytes=0-262143\r\n"),
            "no range on the first piece: {request}"
        );
    }

    #[test]
    fn a_later_piece_starts_where_the_last_one_ended() {
        let request = head(
            &book(),
            &Method::Get {
                offset: Some(262_144),
                credential: None,
                headers: &[],
                streaming: false,
            },
            262_144,
        );
        assert!(
            request.contains("\r\nRange: bytes=262144-524287\r\n"),
            "{request}"
        );
    }

    #[test]
    fn asking_for_a_whole_document_sends_no_range_at_all() {
        // A catalogue response is meant to be complete or nothing; a partial
        // one is not shorter JSON, it is broken JSON.
        let request = head(
            &book(),
            &Method::Get {
                offset: None,
                credential: None,
                headers: &[],
                streaming: false,
            },
            262_144,
        );
        assert!(!request.contains("Range:"), "{request}");
    }

    #[test]
    fn a_range_past_the_end_of_a_document_is_the_end_of_the_book_rather_than_a_failure() {
        // Every book ends, and the last top-up asks for a piece that is not
        // there. Reported as an error it would put a warning on the panel at
        // the end of every book that happens to divide evenly.
        let response = b"HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: 0\r\n\r\n";
        assert_eq!(
            split_response(response, CEILING),
            Ok(Response::Body(Cow::Borrowed(&[])))
        );
    }

    #[test]
    fn a_partial_answer_is_a_success_rather_than_something_to_retry() {
        let response = b"HTTP/1.1 206 Partial Content\r\nContent-Length: 5\r\n\r\nCHAP1";
        assert_eq!(
            split_response(response, CEILING),
            Ok(Response::Body(Cow::Borrowed(b"CHAP1")))
        );
    }

    #[test]
    fn a_plain_url_uses_the_default_port_and_root_path() {
        assert_eq!(
            parse("https://example.com"),
            Ok(Address {
                host: "example.com".into(),
                port: 443,
                path: "/".into(),
                authority: "example.com".into(),
            })
        );
    }

    #[test]
    fn a_port_and_path_are_both_kept() {
        assert_eq!(
            parse("https://example.com:8443/feed.xml?since=1"),
            Ok(Address {
                host: "example.com".into(),
                port: 8443,
                path: "/feed.xml?since=1".into(),
                authority: "example.com:8443".into(),
            })
        );
    }

    /// Unencrypted requests are refused rather than quietly upgraded.
    #[test]
    fn plain_http_is_refused() {
        assert_eq!(parse("http://example.com"), Err(TaskError::NotFound));
    }

    /// Credentials in a URL would reach the host and any log of the request.
    #[test]
    fn credentials_in_the_url_are_refused() {
        assert_eq!(
            parse("https://user:secret@example.com/"),
            Err(TaskError::NotFound)
        );
    }

    #[test]
    fn nonsense_is_refused_rather_than_guessed_at() {
        for url in [
            "",
            "example.com",
            "ftp://example.com",
            "https://",
            "https://:443/",
        ] {
            assert_eq!(parse(url), Err(TaskError::NotFound), "accepted {url}");
        }
    }

    #[test]
    fn request_line_control_characters_are_refused() {
        for url in [
            "https://example.com/x\r\nX-Stolen: yes",
            "https://example.com/x\nGET /second",
            "https://example.com/a b",
            "https://example.com/\u{7f}",
        ] {
            assert_eq!(parse(url), Err(TaskError::NotFound), "accepted {url:?}");
        }
    }

    #[test]
    fn a_non_default_port_is_present_in_the_host_header() {
        let address = parse("https://example.com:8443/path").expect("a URL");
        let request = head(
            &address,
            &Method::Get {
                offset: None,
                credential: None,
                headers: &[],
                streaming: false,
            },
            1024,
        );
        assert!(request.contains("Host: example.com:8443\r\n"), "{request}");
    }

    #[test]
    fn a_body_is_separated_from_its_headers() {
        let response = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello";
        assert_eq!(
            split_response(response, CEILING),
            Ok(Response::Body(Cow::Borrowed(&b"hello"[..])))
        );
    }

    #[test]
    fn a_server_refusal_is_reported_as_such() {
        let response = b"HTTP/1.1 404 Not Found\r\n\r\nmissing";
        assert_eq!(split_response(response, CEILING), Err(TaskError::NotFound));
    }

    #[test]
    fn rate_limit_metadata_is_bounded_and_contains_no_server_body() {
        let response =
            b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 17\r\nContent-Length: 18\r\n\r\nprivate request id";
        assert_eq!(
            super::rate_limit(response),
            Ok(super::RateLimit::Limited(Some(17)))
        );
        assert_eq!(
            String::from_utf8(super::rate_limit_envelope(Some(17))).expect("envelope"),
            "COBALT-HTTP/1 429\nRetry-After: 17\n\n"
        );
        let huge =
            b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 999999\r\nContent-Length: 0\r\n\r\n";
        assert_eq!(
            super::rate_limit(huge),
            Ok(super::RateLimit::Limited(Some(
                super::MAX_RETRY_AFTER_SECONDS
            )))
        );
    }

    #[test]
    fn a_reply_that_is_not_http_is_unreachable_rather_than_parsed() {
        assert_eq!(
            split_response(b"garbage", CEILING),
            Err(TaskError::Unreachable)
        );
    }

    #[test]
    fn a_redirect_is_reported_with_its_target() {
        let response = b"HTTP/1.1 302 Found\r\nLocation: https://elsewhere.test/book.epub\r\n\r\n";
        assert_eq!(
            split_response(response, CEILING),
            Ok(Response::Redirect(
                "https://elsewhere.test/book.epub".into()
            ))
        );
    }

    /// Header names are not case sensitive and real servers vary.
    #[test]
    fn the_location_header_is_matched_whatever_its_case() {
        let response = b"HTTP/1.1 301 Moved\r\nlocation: https://a.test/x\r\n\r\n";
        assert_eq!(
            split_response(response, CEILING),
            Ok(Response::Redirect("https://a.test/x".into()))
        );
    }

    /// A redirect with nowhere to go must not be mistaken for a body.
    #[test]
    fn a_redirect_without_a_location_is_an_error() {
        assert_eq!(
            split_response(b"HTTP/1.1 302 Found\r\nX: y\r\n\r\n", CEILING),
            Err(TaskError::Unreachable)
        );
    }

    fn address(host: &str, path: &str) -> Address {
        Address {
            host: host.into(),
            port: 443,
            path: path.into(),
            authority: host.into(),
        }
    }

    #[test]
    fn an_absolute_redirect_is_taken_as_given() {
        assert_eq!(
            resolve_redirect(&address("a.test", "/one"), "https://b.test/two"),
            Ok("https://b.test/two".into())
        );
    }

    #[test]
    fn a_rooted_redirect_keeps_the_original_host() {
        assert_eq!(
            resolve_redirect(&address("a.test", "/one/two"), "/three"),
            Ok("https://a.test/three".into())
        );
    }

    #[test]
    fn a_relative_redirect_resolves_against_the_current_directory() {
        assert_eq!(
            resolve_redirect(&address("a.test", "/books/index.html"), "1342.epub"),
            Ok("https://a.test/books/1342.epub".into())
        );
    }

    /// A redirect must never be able to quietly downgrade to plaintext.
    #[test]
    fn a_plaintext_redirect_is_upgraded_rather_than_followed() {
        // Project Gutenberg really does answer an https request with an http
        // Location, for the same file it also serves over TLS.
        let from = parse("https://www.gutenberg.org/ebooks/2641.txt.utf-8").expect("an address");
        assert_eq!(
            resolve_redirect(&from, "http://www.gutenberg.org/cache/epub/2641/pg2641.txt"),
            Ok("https://www.gutenberg.org/cache/epub/2641/pg2641.txt".to_string())
        );
    }

    #[test]
    fn a_redirect_cannot_downgrade_the_connection() {
        // `http` is absent deliberately: it is upgraded to TLS rather than
        // followed, which is covered by its own test. Nothing here may result
        // in a plaintext request either way.
        for target in ["//a.test/x", "ftp://a.test/x", "file:///etc/passwd"] {
            assert_eq!(
                resolve_redirect(&address("a.test", "/one"), target),
                Err(TaskError::NotFound),
                "followed {target}"
            );
        }
    }

    /// The ceiling is checked before any socket is opened.
    #[test]
    fn a_refused_url_never_reaches_the_network() {
        assert_eq!(fetch("http://example.com", 10), Err(TaskError::NotFound));
    }

    /// A gzip member carrying `content`, as a server would send one.
    fn gzipped(content: &[u8]) -> Vec<u8> {
        let mut out = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 0xff];
        out.extend_from_slice(&miniz_oxide::deflate::compress_to_vec(content, 6));
        out.extend_from_slice(&[0; 8]);
        out
    }

    #[test]
    fn a_json_api_is_asked_for_its_reply_compressed() {
        let address = parse("https://feedsearch.dev/api/v1/search?url=nytimes.com").expect("a url");
        let request = head(
            &address,
            &Method::Get {
                offset: None,
                credential: None,
                headers: &[],
                streaming: false,
            },
            512 * 1024,
        );
        assert!(
            request.contains("\r\nAccept-Encoding: gzip\r\n"),
            "a plain fetch did not ask for gzip: {request}"
        );

        let posted = head(
            &address,
            &Method::Post {
                body: b"{}",
                content_type: "application/json",
                credential: None,
                headers: &[],
            },
            512 * 1024,
        );
        assert!(
            posted.contains("\r\nAccept-Encoding: gzip\r\n"),
            "a post did not ask for gzip: {posted}"
        );
    }

    #[test]
    fn a_piece_of_a_document_is_still_asked_for_as_it_was_written() {
        let address = parse("https://gutenberg.org/files/2701/2701-0.txt").expect("a url");
        let request = head(
            &address,
            &Method::Get {
                offset: Some(262_144),
                credential: None,
                headers: &[],
                streaming: false,
            },
            262_144,
        );
        // A range names bytes the server sends. Compressed, those bytes are a
        // window into a deflate stream, which is not a document and cannot be
        // expanded without the ones before it.
        assert!(
            request.contains("\r\nAccept-Encoding: identity\r\n"),
            "a ranged request asked for gzip: {request}"
        );
    }

    #[test]
    fn a_compressed_reply_reaches_the_caller_as_the_document_it_is() {
        let content = b"{\"feeds\":[{\"url\":\"https://rss.nytimes.com/HomePage.xml\"}]}";
        let body = gzipped(content);
        let mut response = format!(
            "HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        response.extend_from_slice(&body);
        assert_eq!(
            split_response(&response, CEILING),
            Ok(Response::Body(Cow::Owned(content.to_vec())))
        );
    }

    #[test]
    fn a_reply_that_is_both_chunked_and_compressed_is_unwrapped_in_that_order() {
        // What every large CDN actually sends. The chunks are how it was sent
        // and the gzip is what it is, so the framing has to come off first.
        let content = b"a body that arrived in pieces and compressed";
        let body = gzipped(content);
        let mut response =
            b"HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nTransfer-Encoding: chunked\r\n\r\n"
                .to_vec();
        for piece in body.chunks(7) {
            response.extend_from_slice(format!("{:x}\r\n", piece.len()).as_bytes());
            response.extend_from_slice(piece);
            response.extend_from_slice(b"\r\n");
        }
        response.extend_from_slice(b"0\r\n\r\n");
        assert_eq!(
            split_response(&response, CEILING),
            Ok(Response::Body(Cow::Owned(content.to_vec())))
        );
    }

    #[test]
    fn a_reply_in_a_coding_that_was_never_asked_for_is_not_handed_on_as_a_body() {
        // Brotli, which some servers send whatever they were offered. Half a
        // brotli stream read as text is worse than an honest failure.
        let response = b"HTTP/1.1 200 OK\r\nContent-Encoding: br\r\nContent-Length: 3\r\n\r\nabc";
        assert_eq!(
            split_response(response, CEILING),
            Err(TaskError::Unreachable)
        );
    }

    #[test]
    fn a_reply_that_says_identity_is_the_body_itself() {
        let response =
            b"HTTP/1.1 200 OK\r\nContent-Encoding: identity\r\nContent-Length: 3\r\n\r\nabc";
        assert_eq!(
            split_response(response, CEILING),
            Ok(Response::Body(Cow::Borrowed(b"abc")))
        );
    }

    #[test]
    fn a_compressed_reply_is_measured_after_it_expands_and_not_before() {
        // Two hundred kilobytes of one letter is a few hundred bytes on the
        // wire. The ceiling the task declared is about what the reader holds.
        let body = gzipped(&vec![b'a'; 200_000]);
        let mut response = format!(
            "HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        response.extend_from_slice(&body);
        assert!(body.len() < CEILING as usize, "the wire body was not small");
        assert_eq!(split_response(&response, CEILING), Err(TaskError::TooLarge));
    }

    #[test]
    fn a_chunked_body_is_reassembled() {
        // Exactly the framing api.openai.com uses over HTTP/1.1. Without this
        // the caller is handed `1a\r\n{"choices"...` and reports a perfectly
        // good reply as unreadable.
        let response = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n\
            17\r\n{\"choices\":[{\"message\":\r\n\
            12\r\n{\"content\":\"h\"}}]}\r\n\
            0\r\n\r\n";
        assert_eq!(
            split_response(response, CEILING),
            Ok(Response::Body(Cow::Owned(
                br#"{"choices":[{"message":{"content":"h"}}]}"#.to_vec()
            )))
        );
    }

    #[test]
    fn a_chunk_extension_is_not_part_of_the_length() {
        let response =
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5;a=b\r\nhello\r\n0\r\n\r\n";
        assert_eq!(
            split_response(response, CEILING),
            Ok(Response::Body(Cow::Owned(b"hello".to_vec())))
        );
    }

    #[test]
    fn a_body_that_ends_mid_chunk_is_a_failure_rather_than_a_short_answer() {
        // Returning what arrived would present half a book, or half a reply,
        // as the whole of it.
        let response = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n10\r\nshort";
        assert_eq!(
            split_response(response, CEILING),
            Err(TaskError::Unreachable)
        );
    }

    #[test]
    fn a_chunked_body_requires_its_final_header_terminator() {
        let response = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n";
        assert_eq!(
            split_response(response, CEILING),
            Err(TaskError::Unreachable)
        );
    }

    #[test]
    fn conflicting_lengths_are_refused() {
        let response = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nContent-Length: 4\r\n\r\nhello";
        assert_eq!(
            split_response(response, CEILING),
            Err(TaskError::Unreachable)
        );
    }

    #[test]
    fn transfer_encoding_and_content_length_cannot_disagree() {
        let response = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Length: 5\r\n\r\n5\r\nhello\r\n0\r\n\r\n";
        assert_eq!(
            split_response(response, CEILING),
            Err(TaskError::Unreachable)
        );
    }

    #[test]
    fn a_length_framed_body_must_arrive_whole() {
        let response = b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\n\r\nhello";
        assert_eq!(
            split_response(response, CEILING),
            Err(TaskError::Unreachable)
        );
    }

    #[test]
    fn a_length_framed_body_is_still_borrowed_rather_than_copied() {
        let response = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello";
        assert!(matches!(
            split_response(response, CEILING),
            Ok(Response::Body(Cow::Borrowed(b"hello")))
        ));
    }

    #[test]
    fn a_credential_actually_reaches_the_request() {
        // This existed as `Authorization: ******`, the redaction meant for a
        // log, written into the real request. Every authenticated POST was
        // therefore sent with a placeholder where the key should have been,
        // and nothing on this side could see it: the failure appears as a 401
        // from the far end.
        let address = parse("https://api.anthropic.com/v1/messages").expect("a URL");
        let post_head = head(
            &address,
            &Method::Post {
                body: b"{}",
                content_type: "application/json",
                credential: Some(("x-api-key", "not-a-real-key")),
                headers: &[("anthropic-version", "2023-06-01")],
            },
            1024,
        );
        assert!(
            post_head.contains("x-api-key: not-a-real-key\r\n"),
            "{post_head}"
        );
        assert!(
            post_head.contains("anthropic-version: 2023-06-01\r\n"),
            "{post_head}"
        );
        assert!(!post_head.contains('*'), "{post_head}");
        assert!(post_head.contains("Content-Length: 2\r\n"), "{post_head}");

        let fetch_head = head(
            &address,
            &Method::Get {
                offset: None,
                credential: Some(("x-api-key", "not-a-real-key")),
                headers: &[("anthropic-version", "2023-06-01")],
                streaming: false,
            },
            1024,
        );
        assert!(
            fetch_head.contains("x-api-key: not-a-real-key\r\n"),
            "{fetch_head}"
        );
        assert!(
            fetch_head.contains("anthropic-version: 2023-06-01\r\n"),
            "{fetch_head}"
        );
    }

    #[test]
    fn a_bearer_credential_is_spelled_the_usual_way() {
        let address = parse("https://openrouter.ai/api/v1/chat").expect("a URL");
        let head = head(
            &address,
            &Method::Post {
                body: b"{}",
                content_type: "application/json",
                credential: Some(("Authorization", "Bearer sk-or-secret")),
                headers: &[],
            },
            1024,
        );
        assert!(
            head.contains("Authorization: Bearer sk-or-secret\r\n"),
            "{head}"
        );
    }

    #[test]
    fn a_header_that_could_forge_another_one_is_refused() {
        for headers in [
            &[("x-note", "one\r\nAuthorization: Bearer stolen")][..],
            &[("Host: attacker.invalid", "anything")][..],
            &[(" Host", "attacker.invalid")][..],
        ] {
            let refused = post(
                "https://example.invalid/",
                b"{}",
                "application/json",
                None,
                headers,
                1024,
            );
            assert_eq!(refused, Err(TaskError::Denied));
        }
    }
}

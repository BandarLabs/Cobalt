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
//! ## Why a pure-Rust provider
//!
//! The usual providers compile C and assembly, which would require a cross
//! toolchain and end this project's property that a plain
//! `rustup target add armv7-unknown-linux-musleabihf` is enough to build a
//! device binary. The provider used here is pure Rust and cross-compiles with
//! no system packages. It is also young and unaudited, which is the reason all
//! of this sits behind one function: changing it is a change to this file.

use kobo_protocol::TaskError;
use std::borrow::Cow;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// How long a single request may spend connected before it is abandoned.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// The largest response header block accepted before the body is refused.
const MAX_HEADER_BYTES: usize = 32 * 1024;

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
    let rest = url.strip_prefix("https://").ok_or(TaskError::NotFound)?;
    let (authority, path) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, "/"),
    };
    // Credentials in a URL would be sent to the host and written into any log
    // that records the request, so they are refused rather than stripped.
    if authority.contains('@') || authority.is_empty() {
        return Err(TaskError::NotFound);
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (host, port.parse::<u16>().map_err(|_| TaskError::NotFound)?),
        None => (authority, 443),
    };
    if host.is_empty() || host.contains('/') {
        return Err(TaskError::NotFound);
    }
    Ok(Address {
        host: host.to_string(),
        port,
        path: path.to_string(),
    })
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

/// Separates a response into its status code and its body.
///
/// # Errors
///
/// Returns [`TaskError::Unreachable`] if the response is not recognisable
/// HTTP, and [`TaskError::NotFound`] for a 4xx or 5xx status.
pub fn split_response(response: &[u8]) -> Result<Response<'_>, TaskError> {
    let head_end = find_header_end(response).ok_or(TaskError::Unreachable)?;
    let head = std::str::from_utf8(&response[..head_end]).map_err(|_| TaskError::Unreachable)?;
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or(TaskError::Unreachable)?;
    match status {
        200..=299 => {
            let body = &response[head_end + 4..];
            // Chunked is not an exotic case to handle later: every large CDN
            // answers HTTP/1.1 with it, and api.openai.com behind Cloudflare
            // always does. Handing the framing back as if it were the body
            // means the caller sees `1f4\r\n{"id":...` and reports the reply as
            // unreadable, which is exactly how this was found.
            if header(head, "transfer-encoding")
                .is_some_and(|value| value.to_ascii_lowercase().contains("chunked"))
            {
                Ok(Response::Body(Cow::Owned(decode_chunked(body)?)))
            } else {
                Ok(Response::Body(Cow::Borrowed(body)))
            }
        }
        // The range started past the end of the document, which is what asking
        // for the next piece of a book that has just ended looks like. An
        // empty body says "nothing further" to a caller reading in pieces; the
        // alternative is reporting the last page of every book as a failure.
        416 => Ok(Response::Body(Cow::Borrowed(&[]))),
        // 304 carries no body and no Location, so it is not a redirect here.
        301..=303 | 307 | 308 => header(head, "location")
            .map(|target| Response::Redirect(target.to_string()))
            .ok_or(TaskError::Unreachable),
        400..=599 => Err(TaskError::NotFound),
        _ => Err(TaskError::Unreachable),
    }
}

/// Reads one header value, matching the name case insensitively.
fn header<'a>(head: &'a str, wanted: &str) -> Option<&'a str> {
    head.lines().skip(1).find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case(wanted)
            .then(|| value.trim())
    })
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
        let line_end = find(body, b"\r\n").ok_or(TaskError::Unreachable)?;
        let header = std::str::from_utf8(&body[..line_end]).map_err(|_| TaskError::Unreachable)?;
        // The extension after a semicolon is metadata about the chunk, never
        // part of its length.
        let size = header.split(';').next().unwrap_or(header).trim();
        let size = usize::from_str_radix(size, 16).map_err(|_| TaskError::Unreachable)?;
        body = &body[line_end + 2..];
        if size == 0 {
            return Ok(out);
        }
        if body.len() < size + 2 {
            return Err(TaskError::Unreachable);
        }
        out.extend_from_slice(&body[..size]);
        if &body[size..size + 2] != b"\r\n" {
            return Err(TaskError::Unreachable);
        }
        body = &body[size + 2..];
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn find_header_end(response: &[u8]) -> Option<usize> {
    response
        .windows(4)
        .take(MAX_HEADER_BYTES)
        .position(|window| window == b"\r\n\r\n")
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
    get(url, None, max_bytes)
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
/// # Errors
///
/// The same distinctions [`fetch`] makes.
pub fn fetch_from(url: &str, offset: u32, max_bytes: u32) -> Result<Vec<u8>, TaskError> {
    get(url, Some(offset), max_bytes)
}

/// The one implementation behind [`fetch`] and [`fetch_from`].
fn get(url: &str, offset: Option<u32>, max_bytes: u32) -> Result<Vec<u8>, TaskError> {
    let mut target = url.to_string();
    for _ in 0..=MAX_REDIRECTS {
        let address = parse(&target)?;
        let response = request(&address, &Method::Get { offset }, max_bytes)?;
        match split_response(&response)? {
            Response::Body(body) => {
                return if body.len() > max_bytes as usize {
                    Err(TaskError::TooLarge)
                } else {
                    Ok(body.to_vec())
                };
            }
            Response::Redirect(location) => target = resolve_redirect(&address, &location)?,
        }
    }
    Err(TaskError::Unreachable)
}

/// What is sent, beyond the address.
enum Method<'a> {
    Get {
        /// Where to start reading, as a byte offset, or `None` to ask for the
        /// whole document with no range header at all.
        offset: Option<u32>,
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
/// `credential`, when present, is a header name and its complete value — the
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
    // A header containing a newline would let a caller append headers of its
    // own to the request. The runtime reads the credential off a file whose
    // contents it does not control, so this is checked rather than assumed,
    // and it is checked here because this is the last gate before the socket.
    let clean = |text: &str| !text.bytes().any(|byte| byte < 0x20 || byte == 0x7f);
    if let Some((name, value)) = credential {
        if name.is_empty() || !clean(name) || !clean(value) {
            return Err(TaskError::Denied);
        }
    }
    if headers
        .iter()
        .any(|(name, value)| name.is_empty() || !clean(name) || !clean(value))
    {
        return Err(TaskError::Denied);
    }
    if !clean(content_type) {
        return Err(TaskError::Denied);
    }
    let address = parse(url)?;
    let response = request(
        &address,
        &Method::Post {
            body,
            content_type,
            credential,
            headers,
        },
        max_bytes,
    )?;
    match split_response(&response)? {
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

/// Builds the request line and headers.
///
/// Separated from the socket so it can be tested. The bug this exists to stop
/// recurring was invisible from the outside: a missing range header on the
/// first piece of a document, which no test could see because it only showed
/// up as a server sending more than the ceiling allowed.
fn head(address: &Address, method: &Method<'_>, max_bytes: u32) -> String {
    let (verb, path, host) = (method.verb(), address.path.as_str(), address.host.as_str());
    let mut head = format!(
        "{verb} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nAccept-Encoding: identity\r\nUser-Agent: kobo-runtime\r\n"
    );
    match method {
        Method::Get { offset: None } => {}
        Method::Get {
            offset: Some(start),
        } => {
            // Closed at both ends, because an open-ended range invites the
            // server to send the rest of a book that does not fit. Sent for
            // the first piece as well as later ones: a request for the opening
            // 256 KB of a 738 KB novel without a range is answered with the
            // whole novel, and then rejected by the ceiling.
            let last = u64::from(*start) + u64::from(max_bytes) - 1;
            head.push_str(&format!("Range: bytes={start}-{last}\r\n"));
        }
        Method::Post {
            body,
            content_type,
            credential,
            headers,
        } => {
            if let Some((name, value)) = credential {
                head.push_str(&format!("{name}: {value}\r\n"));
            }
            for (name, value) in *headers {
                head.push_str(&format!("{name}: {value}\r\n"));
            }
            head.push_str(&format!(
                "Content-Type: {content_type}\r\nContent-Length: {}\r\n",
                body.len()
            ));
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
/// expensive half on a 1 GHz ARM core driving a pure-Rust crypto provider with
/// no AES instructions behind it.
///
/// Measured over four runs of five sequential requests to gutenberg.org:
/// 7.7s with a config per request against 6.1s with one shared, consistently
/// around a fifth faster, on a machine far quicker than the reader.
static TLS_CONFIG: OnceLock<Arc<rustls::ClientConfig>> = OnceLock::new();

fn tls_config() -> Result<Arc<rustls::ClientConfig>, TaskError> {
    if let Some(config) = TLS_CONFIG.get() {
        return Ok(Arc::clone(config));
    }
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let config =
        rustls::ClientConfig::builder_with_provider(Arc::new(rustls_rustcrypto::provider()))
            .with_safe_default_protocol_versions()
            .map_err(|_| TaskError::Unreachable)?
            .with_root_certificates(roots)
            .with_no_client_auth();
    // Whichever call built it first wins; the loser drops its own copy. Both
    // are the same configuration, so it does not matter which.
    Ok(Arc::clone(TLS_CONFIG.get_or_init(|| Arc::new(config))))
}

fn request(address: &Address, method: &Method<'_>, max_bytes: u32) -> Result<Vec<u8>, TaskError> {
    let config = tls_config()?;
    let name = address
        .host
        .clone()
        .try_into()
        .map_err(|_| TaskError::NotFound)?;
    let mut connection =
        rustls::ClientConnection::new(config, name).map_err(|_| TaskError::Unreachable)?;
    let mut socket = TcpStream::connect((address.host.as_str(), address.port))
        .map_err(|_| TaskError::Unreachable)?;
    socket
        .set_read_timeout(Some(REQUEST_TIMEOUT))
        .and_then(|()| socket.set_write_timeout(Some(REQUEST_TIMEOUT)))
        .map_err(|_| TaskError::Unreachable)?;
    let mut tls = rustls::Stream::new(&mut connection, &mut socket);
    tls.write_all(head(address, method, max_bytes).as_bytes())
        .map_err(|_| TaskError::Unreachable)?;
    if let Method::Post { body, .. } = method {
        tls.write_all(body).map_err(|_| TaskError::Unreachable)?;
    }
    tls.flush().map_err(|_| TaskError::Unreachable)?;

    // The ceiling is applied to the whole response as it arrives, so a server
    // that never stops sending cannot fill memory before the body is examined.
    let ceiling = max_bytes as usize;
    let mut response = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        match tls.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                response.extend_from_slice(&buffer[..read]);
                if response.len() > ceiling.saturating_add(MAX_HEADER_BYTES) {
                    return Err(TaskError::TooLarge);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                return Err(TaskError::TimedOut)
            }
            Err(_) => break,
        }
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
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

    use super::{
        fetch, head, parse, post, resolve_redirect, split_response, Address, Cow, Method, Response,
    };
    use kobo_protocol::TaskError;

    fn book() -> Address {
        Address {
            host: "www.gutenberg.org".into(),
            port: 443,
            path: "/files/1342/1342-0.txt".into(),
        }
    }

    #[test]
    fn the_first_piece_of_a_document_is_asked_for_by_range_like_every_other_piece() {
        // This is the whole reason a long book can be opened at all. Without a
        // range on the first request the server sends all 738 KB of Pride and
        // Prejudice, the 256 KB ceiling rejects it, and the opening page of
        // every book larger than one piece is unreachable. The symptom on the
        // device was a download that appeared to hang.
        let request = head(&book(), &Method::Get { offset: Some(0) }, 262_144);
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
        let request = head(&book(), &Method::Get { offset: None }, 262_144);
        assert!(!request.contains("Range:"), "{request}");
    }

    #[test]
    fn a_range_past_the_end_of_a_document_is_the_end_of_the_book_rather_than_a_failure() {
        // Every book ends, and the last top-up asks for a piece that is not
        // there. Reported as an error it would put a warning on the panel at
        // the end of every book that happens to divide evenly.
        let response = b"HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: 0\r\n\r\n";
        assert_eq!(
            split_response(response),
            Ok(Response::Body(Cow::Borrowed(&[])))
        );
    }

    #[test]
    fn a_partial_answer_is_a_success_rather_than_something_to_retry() {
        let response = b"HTTP/1.1 206 Partial Content\r\nContent-Length: 5\r\n\r\nCHAP1";
        assert_eq!(
            split_response(response),
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
                path: "/".into()
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
                path: "/feed.xml?since=1".into()
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
    fn a_body_is_separated_from_its_headers() {
        let response = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello";
        assert_eq!(
            split_response(response),
            Ok(Response::Body(Cow::Borrowed(&b"hello"[..])))
        );
    }

    #[test]
    fn a_server_refusal_is_reported_as_such() {
        let response = b"HTTP/1.1 404 Not Found\r\n\r\nmissing";
        assert_eq!(split_response(response), Err(TaskError::NotFound));
    }

    #[test]
    fn a_reply_that_is_not_http_is_unreachable_rather_than_parsed() {
        assert_eq!(split_response(b"garbage"), Err(TaskError::Unreachable));
    }

    #[test]
    fn a_redirect_is_reported_with_its_target() {
        let response = b"HTTP/1.1 302 Found\r\nLocation: https://elsewhere.test/book.epub\r\n\r\n";
        assert_eq!(
            split_response(response),
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
            split_response(response),
            Ok(Response::Redirect("https://a.test/x".into()))
        );
    }

    /// A redirect with nowhere to go must not be mistaken for a body.
    #[test]
    fn a_redirect_without_a_location_is_an_error() {
        assert_eq!(
            split_response(b"HTTP/1.1 302 Found\r\nX: y\r\n\r\n"),
            Err(TaskError::Unreachable)
        );
    }

    fn address(host: &str, path: &str) -> Address {
        Address {
            host: host.into(),
            port: 443,
            path: path.into(),
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
            split_response(response),
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
            split_response(response),
            Ok(Response::Body(Cow::Owned(b"hello".to_vec())))
        );
    }

    #[test]
    fn a_body_that_ends_mid_chunk_is_a_failure_rather_than_a_short_answer() {
        // Returning what arrived would present half a book, or half a reply,
        // as the whole of it.
        let response = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n10\r\nshort";
        assert_eq!(split_response(response), Err(TaskError::Unreachable));
    }

    #[test]
    fn a_length_framed_body_is_still_borrowed_rather_than_copied() {
        let response = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello";
        assert!(matches!(
            split_response(response),
            Ok(Response::Body(Cow::Borrowed(b"hello")))
        ));
    }

    #[test]
    fn a_credential_actually_reaches_the_request() {
        // This existed as `Authorization: ******` — the redaction meant for a
        // log, written into the real request. Every authenticated POST was
        // therefore sent with a placeholder where the key should have been,
        // and nothing on this side could see it: the failure appears as a 401
        // from the far end.
        let address = parse("https://api.anthropic.com/v1/messages").expect("a URL");
        let head = head(
            &address,
            &Method::Post {
                body: b"{}",
                content_type: "application/json",
                credential: Some(("x-api-key", "sk-ant-secret")),
                headers: &[("anthropic-version", "2023-06-01")],
            },
            1024,
        );
        assert!(head.contains("x-api-key: sk-ant-secret\r\n"), "{head}");
        assert!(head.contains("anthropic-version: 2023-06-01\r\n"), "{head}");
        assert!(!head.contains('*'), "{head}");
        assert!(head.contains("Content-Length: 2\r\n"), "{head}");
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
        let refused = post(
            "https://example.invalid/",
            b"{}",
            "application/json",
            None,
            &[("x-note", "one\r\nAuthorization: Bearer stolen")],
            1024,
        );
        assert_eq!(refused, Err(TaskError::Denied));
    }
}

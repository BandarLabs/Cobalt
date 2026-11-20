//! Just enough HTTP for one page and the requests it makes.
//!
//! The only client is a browser on this computer showing this program's own
//! page, so this is not a web server. One request per connection, then the
//! connection is closed: no keep-alive, no chunked bodies, no pipelining.
//! What it does take seriously is the head, because the head is where every
//! rule in [`super::guard`] is decided, and it is read and bounded before a
//! single byte of a body is.
//!
//! The head and the body are read separately on purpose. A caller checks who
//! is asking from the head alone, and only then asks for the body with the
//! limit that route allows, so a stranger can make this program read a few
//! kilobytes and nothing more.

use std::io::{Read, Write};

/// What reading a request can refuse, each with the status that says so.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Malformed {
    /// The head or the body is larger than its limit.
    TooLarge,
    /// The head went past its limit before it ended.
    HeadTooLarge,
    /// The request cannot be read as HTTP/1.1, or says the same thing twice
    /// in a header that may only be said once.
    Bad,
    /// A body with no stated length. A length is the only way to refuse a
    /// body before reading it.
    LengthRequired,
    /// The connection ended or went quiet before the request did.
    Incomplete,
}

impl Malformed {
    /// The status line this refusal is answered with.
    #[must_use]
    pub const fn status(self) -> (u16, &'static str) {
        match self {
            Self::TooLarge => (413, "Content Too Large"),
            Self::HeadTooLarge => (431, "Request Header Fields Too Large"),
            Self::Bad => (400, "Bad Request"),
            Self::LengthRequired => (411, "Length Required"),
            Self::Incomplete => (408, "Request Timeout"),
        }
    }
}

/// Headers a request may carry once. Two `Host` lines, or two lengths, are
/// how one reader of a request is made to disagree with another about what
/// it says, and every rule here is decided on exactly these.
const SAID_ONCE: &[&str] = &[
    "host",
    "origin",
    "cookie",
    "content-length",
    "transfer-encoding",
    "x-kobo-token",
    "sec-fetch-site",
    "authorization",
];

/// A request's first line and headers, and whatever of the body arrived in
/// the same reads.
#[derive(Debug)]
pub struct Head {
    pub method: String,
    /// The path with its query still attached.
    pub target: String,
    headers: Vec<(String, String)>,
    /// Body bytes that came in with the head.
    early: Vec<u8>,
}

impl Head {
    /// Reads a request up to the blank line that ends its head.
    ///
    /// # Errors
    ///
    /// A head past `limit`, one that is not HTTP/1.1, one that repeats a
    /// header in [`SAID_ONCE`], or a connection that ends first.
    pub fn read(stream: &mut impl Read, limit: usize) -> Result<Self, Malformed> {
        let mut buffer = Vec::new();
        let mut chunk = [0_u8; 2048];
        let end = loop {
            if let Some(position) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
                break position;
            }
            if buffer.len() > limit {
                return Err(Malformed::HeadTooLarge);
            }
            match stream.read(&mut chunk) {
                Ok(0) | Err(_) => return Err(Malformed::Incomplete),
                Ok(read) => buffer.extend_from_slice(&chunk[..read]),
            }
        };
        if end > limit {
            return Err(Malformed::HeadTooLarge);
        }
        let text = std::str::from_utf8(&buffer[..end]).map_err(|_| Malformed::Bad)?;
        let mut lines = text.split("\r\n");
        let mut first = lines.next().unwrap_or_default().split(' ');
        let (Some(method), Some(target), Some(version), None) =
            (first.next(), first.next(), first.next(), first.next())
        else {
            return Err(Malformed::Bad);
        };
        if !version.starts_with("HTTP/1.") || !target.starts_with('/') {
            return Err(Malformed::Bad);
        }
        let mut headers: Vec<(String, String)> = Vec::new();
        for line in lines {
            let (name, value) = line.split_once(':').ok_or(Malformed::Bad)?;
            // A name with a space in it, or a line that begins with one, is
            // the old folded form. Nothing a browser sends uses it, and
            // accepting it is how a header is hidden from a check.
            if name.is_empty() || name.contains(char::is_whitespace) {
                return Err(Malformed::Bad);
            }
            let name = name.to_ascii_lowercase();
            if SAID_ONCE.contains(&name.as_str()) && headers.iter().any(|(seen, _)| *seen == name) {
                return Err(Malformed::Bad);
            }
            headers.push((name, value.trim().to_owned()));
        }
        Ok(Self {
            method: method.to_owned(),
            target: target.to_owned(),
            headers,
            early: buffer[end + 4..].to_vec(),
        })
    }

    /// One header's value, by its lowercase name.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(known, _)| known == name)
            .map(|(_, value)| value.as_str())
    }

    /// The path without its query.
    #[must_use]
    pub fn path(&self) -> &str {
        self.target
            .split_once('?')
            .map_or(self.target.as_str(), |(path, _)| path)
    }

    /// One query value, percent-decoded. `None` when it is absent or does
    /// not decode to text.
    #[must_use]
    pub fn query(&self, key: &str) -> Option<String> {
        let (_, query) = self.target.split_once('?')?;
        query
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .find(|(name, _)| *name == key)
            .and_then(|(_, value)| percent_decoded(value))
    }

    /// One cookie's value.
    #[must_use]
    pub fn cookie(&self, name: &str) -> Option<&str> {
        self.header("cookie")?
            .split(';')
            .filter_map(|pair| pair.trim().split_once('='))
            .find(|(known, _)| *known == name)
            .map(|(_, value)| value)
    }

    /// The length the request states for its body.
    ///
    /// # Errors
    ///
    /// A chunked body, which has no stated length, or a length that is not a
    /// number. A request with neither header has no body.
    pub fn length(&self) -> Result<u64, Malformed> {
        if self.header("transfer-encoding").is_some() {
            return Err(Malformed::LengthRequired);
        }
        match self.header("content-length") {
            None => Ok(0),
            Some(text) if !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()) => {
                text.parse().map_err(|_| Malformed::TooLarge)
            }
            Some(_) => Err(Malformed::Bad),
        }
    }

    /// Reads the whole body into memory, for the small JSON bodies.
    ///
    /// # Errors
    ///
    /// A stated length past `limit`, refused before anything more is read,
    /// or a connection that ends before the body does.
    pub fn body(&mut self, stream: &mut impl Read, limit: u64) -> Result<Vec<u8>, Malformed> {
        let mut body = Vec::new();
        self.body_into(stream, limit, &mut body)?;
        Ok(body)
    }

    /// Copies the body to `sink` as it arrives, for an upload that is too
    /// large to hold. Returns the number of bytes written.
    ///
    /// # Errors
    ///
    /// As [`Self::body`], and when `sink` cannot be written.
    pub fn body_into(
        &mut self,
        stream: &mut impl Read,
        limit: u64,
        sink: &mut impl Write,
    ) -> Result<u64, Malformed> {
        let length = self.length()?;
        if length > limit {
            return Err(Malformed::TooLarge);
        }
        let early = std::mem::take(&mut self.early);
        let from_head = early
            .len()
            .min(usize::try_from(length).unwrap_or(usize::MAX));
        sink.write_all(&early[..from_head])
            .map_err(|_| Malformed::Incomplete)?;
        let mut left = length - from_head as u64;
        let mut chunk = vec![0_u8; 64 * 1024];
        while left > 0 {
            let want = chunk.len().min(usize::try_from(left).unwrap_or(usize::MAX));
            match stream.read(&mut chunk[..want]) {
                Ok(0) | Err(_) => return Err(Malformed::Incomplete),
                Ok(read) => {
                    sink.write_all(&chunk[..read])
                        .map_err(|_| Malformed::Incomplete)?;
                    left -= read as u64;
                }
            }
        }
        Ok(length)
    }
}

/// Decodes `%XX` escapes and `+`, as a form or a `URLSearchParams` writes
/// them.
fn percent_decoded(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' => {
                let pair = value.get(index + 1..index + 3)?;
                decoded.push(u8::from_str_radix(pair, 16).ok()?);
                index += 3;
            }
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(decoded).ok()
}

/// What goes on every response, whatever it carries.
///
/// The policy allows this page its own scripts, styles and requests and
/// nothing from anywhere else, which is what makes "the page talks to no
/// other origin" a property the browser enforces rather than one a reviewer
/// has to keep checking. It may not be framed, so nothing can be laid over
/// it to borrow a click. Nothing is cached, because every answer is about a
/// reader that may have been unplugged since. There is deliberately no
/// `Access-Control-Allow-*` line here or anywhere: no other origin has any
/// business reading these answers.
const FIXED_HEADERS: &str = "\
Cache-Control: no-store\r\n\
Content-Security-Policy: default-src 'none'; script-src 'self'; style-src 'self'; \
img-src 'self' data:; connect-src 'self'; base-uri 'none'; form-action 'none'; \
frame-ancestors 'none'\r\n\
X-Content-Type-Options: nosniff\r\n\
X-Frame-Options: DENY\r\n\
Referrer-Policy: no-referrer\r\n\
Cross-Origin-Opener-Policy: same-origin\r\n\
Cross-Origin-Resource-Policy: same-origin\r\n\
Connection: close\r\n";

/// One response.
#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub reason: &'static str,
    pub content_type: &'static str,
    pub body: Vec<u8>,
    /// Lines beyond the fixed ones, such as a redirect's `Location`.
    pub extra: Vec<(&'static str, String)>,
}

impl Response {
    /// A JSON answer.
    #[must_use]
    pub fn json(status: u16, reason: &'static str, value: &serde_json::Value) -> Self {
        Self {
            status,
            reason,
            content_type: "application/json; charset=utf-8",
            body: value.to_string().into_bytes(),
            extra: Vec::new(),
        }
    }

    /// A successful JSON answer.
    #[must_use]
    pub fn ok(value: &serde_json::Value) -> Self {
        Self::json(200, "OK", value)
    }

    /// A refusal, as JSON: the reason in `error`, which is the one field a
    /// front end shows to the owner as it is, and what sort of refusal it
    /// is in `kind`, which is the one a front end decides on. The sentence
    /// may be reworded freely; the kinds are the contract.
    #[must_use]
    pub fn refusal(status: u16, reason: &'static str, kind: &str, error: &str) -> Self {
        Self::json(
            status,
            reason,
            &serde_json::json!({ "error": error, "kind": kind }),
        )
    }

    /// The answer to a request that could not be read.
    #[must_use]
    pub fn malformed(why: Malformed) -> Self {
        let (status, reason) = why.status();
        let kind = if why == Malformed::TooLarge {
            "too_large"
        } else {
            "bad_request"
        };
        Self::refusal(status, reason, kind, reason)
    }

    /// One of the page's own files.
    #[must_use]
    pub fn file(content_type: &'static str, body: &[u8]) -> Self {
        Self {
            status: 200,
            reason: "OK",
            content_type,
            body: body.to_vec(),
            extra: Vec::new(),
        }
    }

    /// Adds a header line.
    #[must_use]
    pub fn with(mut self, name: &'static str, value: String) -> Self {
        self.extra.push((name, value));
        self
    }

    /// Writes the response. A peer that has gone away is not an error worth
    /// reporting to anybody.
    pub fn write(&self, stream: &mut impl Write) {
        let mut head = format!(
            "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\n{FIXED_HEADERS}",
            self.status,
            self.reason,
            self.content_type,
            self.body.len()
        );
        for (name, value) in &self.extra {
            head.push_str(name);
            head.push_str(": ");
            head.push_str(value);
            head.push_str("\r\n");
        }
        head.push_str("\r\n");
        let _ = stream.write_all(head.as_bytes());
        let _ = stream.write_all(&self.body);
        let _ = stream.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(raw: &str) -> Result<Head, Malformed> {
        Head::read(&mut raw.as_bytes(), 4096)
    }

    #[test]
    fn a_request_parses_into_its_method_path_query_headers_and_cookies() {
        let head = head(
            "POST /api/send/file?name=My%20Comic+1.cbz&x=1 HTTP/1.1\r\nHost: 127.0.0.1:9\r\n\
             Cookie: other=1; kobo_ui=abc\r\nX-Kobo-Token: t\r\n\r\n",
        )
        .expect("parses");
        assert_eq!(head.method, "POST");
        assert_eq!(head.path(), "/api/send/file");
        assert_eq!(head.query("name").as_deref(), Some("My Comic 1.cbz"));
        assert_eq!(head.query("missing"), None);
        assert_eq!(head.header("host"), Some("127.0.0.1:9"));
        assert_eq!(head.header("x-kobo-token"), Some("t"));
        assert_eq!(head.cookie("kobo_ui"), Some("abc"));
        assert_eq!(head.cookie("absent"), None);
    }

    #[test]
    fn a_head_past_its_limit_is_refused_whether_or_not_it_ever_ends() {
        let long = format!("GET / HTTP/1.1\r\nX-Pad: {}\r\n\r\n", "a".repeat(5000));
        assert_eq!(head(&long).unwrap_err(), Malformed::HeadTooLarge);
        let endless = format!("GET / HTTP/1.1\r\nX-Pad: {}", "a".repeat(5000));
        assert_eq!(head(&endless).unwrap_err(), Malformed::HeadTooLarge);
    }

    #[test]
    fn a_request_that_stops_before_its_blank_line_is_incomplete() {
        assert_eq!(
            head("GET / HTTP/1.1\r\nHost: x").unwrap_err(),
            Malformed::Incomplete
        );
    }

    #[test]
    fn a_header_the_rules_are_decided_on_may_not_be_said_twice() {
        for name in ["Host", "Origin", "Cookie", "Content-Length", "X-Kobo-Token"] {
            let raw = format!(
                "GET / HTTP/1.1\r\n{name}: a\r\n{}: b\r\n\r\n",
                name.to_uppercase()
            );
            assert_eq!(head(&raw).unwrap_err(), Malformed::Bad, "{name}");
        }
        assert!(head("GET / HTTP/1.1\r\nAccept: a\r\nAccept: b\r\n\r\n").is_ok());
    }

    #[test]
    fn a_folded_or_nameless_header_and_a_strange_first_line_are_refused() {
        for raw in [
            "GET / HTTP/1.1\r\nHost: a\r\n Host: b\r\n\r\n",
            "GET / HTTP/1.1\r\nHost : a\r\n\r\n",
            "GET / HTTP/1.1\r\nno colon here\r\n\r\n",
            "GET /\r\n\r\n",
            "GET http://127.0.0.1/ HTTP/1.1\r\n\r\n",
            "GET / HTTP/2\r\n\r\n",
            "GET  / HTTP/1.1\r\n\r\n",
        ] {
            assert_eq!(head(raw).unwrap_err(), Malformed::Bad, "{raw:?}");
        }
    }

    #[test]
    fn a_body_is_read_to_its_stated_length_across_the_head_and_later_reads() {
        let raw = b"POST /x HTTP/1.1\r\nContent-Length: 11\r\n\r\nhello";
        let mut stream = &raw[..];
        let mut head = Head::read(&mut stream, 4096).expect("head");
        let mut rest = &b" world and more"[..];
        let mut chained = (&mut stream).chain(&mut rest);
        assert_eq!(head.body(&mut chained, 64).expect("body"), b"hello world");
    }

    #[test]
    fn a_body_longer_than_its_limit_is_refused_from_its_length_alone() {
        let raw = b"POST /x HTTP/1.1\r\nContent-Length: 65\r\n\r\n";
        let mut stream = &raw[..];
        let mut head = Head::read(&mut stream, 4096).expect("head");
        // The stream has nothing left; a refusal that tried to read would
        // report an incomplete request instead.
        assert_eq!(head.body(&mut stream, 64).unwrap_err(), Malformed::TooLarge);
    }

    #[test]
    fn a_chunked_body_and_a_length_that_is_not_a_number_are_refused() {
        let chunked = head("POST /x HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n").expect("head");
        assert_eq!(chunked.length().unwrap_err(), Malformed::LengthRequired);
        for length in ["-1", "1e3", "", "0x10", "+5"] {
            let odd = head(&format!(
                "POST /x HTTP/1.1\r\nContent-Length: {length}\r\n\r\n"
            ))
            .expect("head");
            assert_eq!(odd.length().unwrap_err(), Malformed::Bad, "{length:?}");
        }
        let huge = head("POST /x HTTP/1.1\r\nContent-Length: 99999999999999999999999\r\n\r\n")
            .expect("head");
        assert_eq!(huge.length().unwrap_err(), Malformed::TooLarge);
    }

    #[test]
    fn a_body_that_ends_early_is_incomplete() {
        let raw = b"POST /x HTTP/1.1\r\nContent-Length: 10\r\n\r\nshort";
        let mut stream = &raw[..];
        let mut head = Head::read(&mut stream, 4096).expect("head");
        assert_eq!(
            head.body(&mut stream, 64).unwrap_err(),
            Malformed::Incomplete
        );
    }

    #[test]
    fn every_response_carries_the_fixed_headers_and_never_a_cors_one() {
        let mut written = Vec::new();
        Response::ok(&serde_json::json!({ "a": 1 }))
            .with("Location", "/".to_owned())
            .write(&mut written);
        let text = String::from_utf8(written).expect("text");
        assert!(text.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(text.contains("Cache-Control: no-store\r\n"));
        assert!(text.contains("default-src 'none'"));
        assert!(text.contains("frame-ancestors 'none'"));
        assert!(text.contains("X-Frame-Options: DENY\r\n"));
        assert!(text.contains("Location: /\r\n"));
        assert!(text.contains("Connection: close\r\n"));
        assert!(!text.to_ascii_lowercase().contains("access-control"));
        assert!(text.ends_with("\r\n\r\n{\"a\":1}"));
    }

    #[test]
    fn a_query_value_that_does_not_decode_is_absent_rather_than_guessed() {
        let head = head("GET /x?a=%zz&b=%ff&c=%4 HTTP/1.1\r\n\r\n").expect("head");
        assert_eq!(head.query("a"), None);
        assert_eq!(head.query("b"), None);
        assert_eq!(head.query("c"), None);
    }
}

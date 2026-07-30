//! Just enough HTTP for four routes on a trusted network.
//!
//! The daemon speaks to two clients it fully describes -- its own hook
//! binary over loopback and the reader's runtime over TLS -- so this is not
//! a web server. It reads one request, answers it, and closes. No keep-alive,
//! no chunking, no percent-decoding: the query values are a pairing code and
//! a number of seconds, and the bodies are small JSON. The caps exist so a
//! confused or hostile peer costs a socket, not memory.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

/// The most bytes a request may spend on head plus body. Questions are a
/// command line and change, so anything larger is not one of ours.
const MAX_REQUEST: usize = 64 * 1024;

/// One parsed request: the line that names it and the body that fills it in.
pub struct Request {
    pub method: String,
    /// Path with the query still attached, e.g. `/pending?token=x&wait=25`.
    pub target: String,
    pub body: Vec<u8>,
}

impl Request {
    /// The path without its query.
    #[must_use]
    pub fn path(&self) -> &str {
        self.target
            .split_once('?')
            .map_or(self.target.as_str(), |(path, _)| path)
    }

    /// One query value, verbatim. The protocol's values are codes and
    /// numbers, so there is deliberately no percent-decoding to get wrong.
    #[must_use]
    pub fn query(&self, key: &str) -> Option<&str> {
        let (_, query) = self.target.split_once('?')?;
        query
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .find(|(name, _)| *name == key)
            .map(|(_, value)| value)
    }
}

/// Reads one request off the stream: request line, headers for the length,
/// then exactly that many body bytes.
///
/// # Errors
///
/// A short read, a malformed head or a request past the cap all fail with a
/// phrase for the log; the connection is answered with a 400 by the caller
/// or simply dropped.
pub fn read_request(stream: &mut impl Read) -> Result<Request, String> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 1024];
    let head_end = loop {
        if let Some(position) = find_head_end(&buffer) {
            break position;
        }
        if buffer.len() > MAX_REQUEST {
            return Err("request head too large".to_owned());
        }
        let read = stream.read(&mut chunk).map_err(|error| error.to_string())?;
        if read == 0 {
            return Err("connection closed mid-request".to_owned());
        }
        buffer.extend_from_slice(&chunk[..read]);
    };
    let head = String::from_utf8_lossy(&buffer[..head_end]).into_owned();
    let mut lines = head.lines();
    let request_line = lines.next().ok_or("empty request")?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().ok_or("no method")?.to_owned();
    let target = parts.next().ok_or("no target")?.to_owned();
    let length = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    if length > MAX_REQUEST {
        return Err("request body too large".to_owned());
    }
    let mut body = buffer[head_end + 4..].to_vec();
    while body.len() < length {
        let read = stream.read(&mut chunk).map_err(|error| error.to_string())?;
        if read == 0 {
            return Err("connection closed mid-body".to_owned());
        }
        body.extend_from_slice(&chunk[..read]);
    }
    body.truncate(length);
    Ok(Request {
        method,
        target,
        body,
    })
}

fn find_head_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

/// Writes one JSON response and is done with the connection.
pub fn respond_json(stream: &mut impl Write, status: u16, reason: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

/// POSTs JSON to the daemon's loopback listener and returns the response
/// body. This is the hook's whole client: connect, ask, wait for the person.
///
/// # Errors
///
/// Connection failures and timeouts come back as text; the hook treats every
/// one of them as "no decision" so the agent falls back to its own prompt.
pub fn post_local(port: u16, path: &str, body: &str, patience: Duration) -> Result<String, String> {
    let stream =
        TcpStream::connect(("127.0.0.1", port)).map_err(|error| format!("connect: {error}"))?;
    stream
        .set_read_timeout(Some(patience))
        .map_err(|error| format!("timeout: {error}"))?;
    let mut stream = stream;
    let request = format!(
        "POST {path} HTTP/1.1\r\n\
         Host: 127.0.0.1:{port}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|error| format!("send: {error}"))?;
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .map_err(|error| format!("receive: {error}"))?;
    let head_end = find_head_end(&response).ok_or("malformed response")?;
    Ok(String::from_utf8_lossy(&response[head_end + 4..]).into_owned())
}

#[cfg(test)]
mod tests {
    use super::{read_request, Request};

    #[test]
    fn a_post_with_a_body_parses_into_method_target_and_body() {
        let raw = b"POST /answer HTTP/1.1\r\nHost: x\r\nContent-Length: 4\r\n\r\nwait";
        let request = read_request(&mut raw.as_slice()).expect("parses");
        assert_eq!(request.method, "POST");
        assert_eq!(request.target, "/answer");
        assert_eq!(request.body, b"wait");
    }

    #[test]
    fn query_values_come_out_by_name_and_the_path_sheds_them() {
        let request = Request {
            method: "GET".to_owned(),
            target: "/pending?token=abc123&wait=25".to_owned(),
            body: Vec::new(),
        };
        assert_eq!(request.path(), "/pending");
        assert_eq!(request.query("token"), Some("abc123"));
        assert_eq!(request.query("wait"), Some("25"));
        assert_eq!(request.query("missing"), None);
    }

    #[test]
    fn a_request_that_never_finishes_its_head_is_refused() {
        let raw = b"GET /pending HTTP/1.1\r\nHost: x";
        assert!(read_request(&mut raw.as_slice()).is_err());
    }

    #[test]
    fn a_body_larger_than_the_cap_is_refused_before_it_is_read() {
        let raw = b"POST /ask HTTP/1.1\r\nContent-Length: 999999999\r\n\r\n";
        assert!(read_request(&mut raw.as_slice()).is_err());
    }
}

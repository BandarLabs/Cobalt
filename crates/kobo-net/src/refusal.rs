//! A 4xx answer kept with the start of what the server said.
//!
//! The transport used to keep only the status of a refusal and flatten it into
//! a [`TaskError`], which is enough to tell "your key was refused" from "there
//! is nothing here" but not enough to tell "slow down" from "your credit has
//! run out". `OpenAI` answers 429 for both, Anthropic answers 400 for a credit
//! balance that is too low, and the difference is only in the body. An owner
//! told to wait a minute when the real answer is "add credit" waits forever.
//!
//! A caller that asks for refusals gets them back as a status envelope in the
//! task's ordinary bytes, so nothing about the protocol changes:
//!
//! ```text
//! COBALT-HTTP/1 429
//! Retry-After: 20
//!
//! {"error":{"code":"insufficient_quota"}}
//! ```
//!
//! The excerpt is bounded, flattened to one line of printable text, and has
//! the request's own credential cut out of it, because a server that echoes
//! the header it rejected is not unheard of and the excerpt may end up on a
//! panel or in a log.

use kobo_protocol::TaskError;

/// The most of a refused response's body that is kept, in bytes.
pub const MAX_EXCERPT: usize = 512;

/// How much of a body is decoded before it is cut to [`MAX_EXCERPT`]. A
/// gzip refusal page can expand a long way; only its start is ever wanted.
const DECODE_CEILING: u32 = 16 * 1024;

const PREFIX: &str = "COBALT-HTTP/1 ";

/// What stands in for a credential cut out of an excerpt.
const REDACTED: &str = "[credential]";

/// A refused HTTP answer: its status, any `Retry-After`, and the start of
/// its body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Refusal {
    pub status: u16,
    pub retry_after: Option<u32>,
    pub excerpt: String,
}

impl Refusal {
    /// Reads a raw HTTP response, or `Ok(None)` when it is not a 4xx.
    ///
    /// `secrets` are values that must never appear in the excerpt: the
    /// credential header value the request carried, and the token inside it.
    ///
    /// # Errors
    /// Returns [`TaskError::Unreachable`] when the response is not readable
    /// HTTP, the same as every other path through the transport.
    pub fn from_response(response: &[u8], secrets: &[&str]) -> Result<Option<Self>, TaskError> {
        let mut headers = [httparse::EMPTY_HEADER; super::MAX_RESPONSE_HEADERS];
        let mut parsed = httparse::Response::new(&mut headers);
        let head_end = match parsed.parse(response) {
            Ok(httparse::Status::Complete(end)) if end <= super::MAX_HEADER_BYTES => end,
            Ok(httparse::Status::Complete(_) | httparse::Status::Partial) | Err(_) => {
                return Err(TaskError::Unreachable);
            }
        };
        let status = parsed.code.ok_or(TaskError::Unreachable)?;
        if !(400..500).contains(&status) {
            return Ok(None);
        }
        let headers = parsed.headers;
        let retry_after = super::header(headers, "retry-after")
            .and_then(|value| value.parse::<u32>().ok())
            .map(|seconds| seconds.min(super::MAX_RETRY_AFTER_SECONDS));
        let raw = &response[head_end..];
        // Framing and coding are undone when they can be, and the raw bytes
        // kept when they cannot. A refusal page is read for a handful of
        // words; a body that will not decode cleanly is still worth a look.
        let framed = if super::transfer_is_chunked(headers).unwrap_or(false) {
            super::decode_chunked(raw).unwrap_or_else(|_| raw.to_vec())
        } else {
            raw.to_vec()
        };
        let body = super::expanded(
            headers,
            std::borrow::Cow::Owned(framed.clone()),
            DECODE_CEILING,
        )
        .map_or(framed, std::borrow::Cow::into_owned);
        Ok(Some(Self {
            status,
            retry_after,
            excerpt: excerpt(&body, secrets),
        }))
    }

    /// The status envelope that carries this refusal as task bytes.
    #[must_use]
    pub fn envelope(&self) -> Vec<u8> {
        format!(
            "{PREFIX}{}\nRetry-After: {}\n\n{}",
            self.status,
            self.retry_after
                .map_or_else(|| "-".to_owned(), |seconds| seconds.to_string()),
            self.excerpt
        )
        .into_bytes()
    }

    /// Reads a status envelope, or `None` for an ordinary body.
    #[must_use]
    pub fn parse_envelope(bytes: &[u8]) -> Option<Self> {
        if !bytes.starts_with(PREFIX.as_bytes()) {
            return None;
        }
        let rest = std::str::from_utf8(bytes).ok()?.strip_prefix(PREFIX)?;
        let (status, rest) = rest.split_once('\n')?;
        let status = status
            .parse::<u16>()
            .ok()
            .filter(|status| (400..500).contains(status))?;
        let (retry, excerpt) = rest.strip_prefix("Retry-After: ")?.split_once("\n\n")?;
        let retry_after = match retry {
            "-" => None,
            seconds => Some(seconds.parse::<u32>().ok()?),
        };
        (excerpt.len() <= MAX_EXCERPT).then(|| Self {
            status,
            retry_after,
            excerpt: excerpt.to_owned(),
        })
    }

    /// The error this refusal flattens to for a caller that did not ask to
    /// see refusals.
    #[must_use]
    pub fn task_error(&self) -> TaskError {
        match self.status {
            401 | 403 => TaskError::Unauthorized,
            429 => TaskError::RateLimited(self.retry_after.unwrap_or(60).max(1)),
            _ => TaskError::NotFound,
        }
    }
}

/// One line of printable text from the start of `body`, at most
/// [`MAX_EXCERPT`] bytes, with every secret cut out.
fn excerpt(body: &[u8], secrets: &[&str]) -> String {
    let start = &body[..body.len().min(MAX_EXCERPT * 2)];
    let text = String::from_utf8_lossy(start);
    let mut flat = String::with_capacity(text.len());
    let mut space = false;
    for character in text.chars() {
        if character.is_whitespace() || character.is_control() {
            space = !flat.is_empty();
        } else {
            if space {
                flat.push(' ');
                space = false;
            }
            flat.push(character);
        }
    }
    // The longest secret first, so a header value is removed whole before its
    // token is looked for inside what is left.
    let mut secrets: Vec<&str> = secrets
        .iter()
        .copied()
        .filter(|secret| secret.len() >= 8)
        .collect();
    secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
    for secret in secrets {
        flat = flat.replace(secret, REDACTED);
    }
    let mut end = flat.len().min(MAX_EXCERPT);
    while !flat.is_char_boundary(end) {
        end -= 1;
    }
    flat.truncate(end);
    flat
}

/// What must never appear in an excerpt for a request carrying `credential`.
pub(crate) fn secrets_of<'a>(credential: Option<(&'a str, &'a str)>) -> Vec<&'a str> {
    credential
        .map(|(_, value)| {
            let mut secrets = vec![value];
            if let Some((_, token)) = value.split_once(' ') {
                secrets.push(token.trim());
            }
            secrets
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{secrets_of, Refusal, MAX_EXCERPT};
    use kobo_protocol::TaskError;

    #[test]
    fn a_quota_refusal_keeps_the_words_that_say_the_credit_ran_out() {
        let response = b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 20\r\nContent-Length: 46\r\n\r\n{\"error\":{\"code\":\"insufficient_quota\"}}\n\n\n\n\n\n";
        let refusal = Refusal::from_response(response, &[])
            .expect("readable")
            .expect("a refusal");
        assert_eq!(refusal.status, 429);
        assert_eq!(refusal.retry_after, Some(20));
        assert_eq!(
            refusal.excerpt,
            "{\"error\":{\"code\":\"insufficient_quota\"}}"
        );
        let envelope = refusal.envelope();
        assert_eq!(
            envelope,
            b"COBALT-HTTP/1 429\nRetry-After: 20\n\n{\"error\":{\"code\":\"insufficient_quota\"}}"
        );
        assert_eq!(Refusal::parse_envelope(&envelope), Some(refusal));
    }

    #[test]
    fn a_success_or_a_broken_server_is_not_a_refusal() {
        for response in [
            &b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}"[..],
            b"HTTP/1.1 503 Service Unavailable\r\n\r\ndown",
            b"HTTP/1.1 302 Found\r\nLocation: /x\r\n\r\n",
        ] {
            assert_eq!(Refusal::from_response(response, &[]), Ok(None));
        }
        assert_eq!(
            Refusal::from_response(b"not http", &[]),
            Err(TaskError::Unreachable)
        );
    }

    #[test]
    fn the_excerpt_is_one_bounded_line_with_the_credential_cut_out() {
        let token = "fake-token-abcdefghijklm";
        let header = format!("Bearer {token}");
        let body = format!(
            "<html>\r\n<p>Token\t{token} was revoked.</p>\n{}",
            "é".repeat(600)
        );
        let response = format!(
            "HTTP/1.1 401 Unauthorized\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let refusal = Refusal::from_response(
            response.as_bytes(),
            &secrets_of(Some(("Authorization", &header))),
        )
        .unwrap()
        .unwrap();
        assert!(!refusal.excerpt.contains(token), "{}", refusal.excerpt);
        assert!(refusal
            .excerpt
            .starts_with("<html> <p>Token [credential] was revoked.</p>"));
        assert!(refusal.excerpt.len() <= MAX_EXCERPT);
        assert!(!refusal.excerpt.contains(['\n', '\r', '\t']));
        assert_eq!(refusal.task_error(), TaskError::Unauthorized);
        assert!(Refusal::parse_envelope(&refusal.envelope()).is_some());
    }

    #[test]
    fn a_chunked_refusal_is_read_without_its_framing() {
        let response = b"HTTP/1.1 400 Bad Request\r\nTransfer-Encoding: chunked\r\n\r\n1e\r\nYour credit balance is too low\r\n0\r\n\r\n";
        let refusal = Refusal::from_response(response, &[]).unwrap().unwrap();
        assert_eq!(refusal.excerpt, "Your credit balance is too low");
        assert_eq!(refusal.task_error(), TaskError::NotFound);
    }

    #[test]
    fn only_a_well_formed_envelope_parses() {
        assert!(Refusal::parse_envelope(b"[]").is_none());
        assert!(Refusal::parse_envelope(b"COBALT-HTTP/1 500\nRetry-After: -\n\n").is_none());
        assert!(Refusal::parse_envelope(b"COBALT-HTTP/1 404\nRetry-After: x\n\n").is_none());
        assert_eq!(
            Refusal::parse_envelope(b"COBALT-HTTP/1 404\nRetry-After: -\n\n"),
            Some(Refusal {
                status: 404,
                retry_after: None,
                excerpt: String::new()
            })
        );
    }
}

//! Reading a body the server compressed.
//!
//! # Why this is here at all
//!
//! The runtime asked every host for `Accept-Encoding: identity` until now, so
//! every JSON API answered in full. That is a fifth of the bytes these
//! applications need, sent over a radio that is the slowest and most expensive
//! part of this device. Feedsearch answers for a national newspaper in 150 KB
//! of JSON that compresses to about 12 KB; a Hacker News page, a Gutenberg
//! catalogue and an `OpenAI` reply are all the same shape. Text over the wire
//! uncompressed is time the reader spends with its radio lit.
//!
//! # Why the container is parsed here rather than by a library
//!
//! `miniz_oxide` is already in this workspace, decodes raw deflate, and is the
//! part that is genuinely hard to get right. What it does not do is the gzip
//! wrapper, which is a fixed ten-byte header, a handful of optional
//! variable-length fields, and a trailer. That is small enough to write out in
//! full, and writing it out in full is how the length arithmetic stays
//! checked: every step below is a `checked_add` against the real length,
//! because every one of those optional fields is a length taken from the far
//! end of a socket.
//!
//! # What is deliberately not done
//!
//! Only the first member of a multi-member stream is read. Concatenated gzip
//! members are legal and are what `zcat a.gz b.gz` makes, but no HTTP server
//! sends a response that way, and supporting it would mean looping over
//! attacker-framed lengths for no reader-visible gain.
//!
//! Neither the CRC nor the length in the trailer is checked. Both sit under
//! TLS, which already authenticates every byte with a MAC that is far stronger
//! than a CRC32, and a truncated stream is caught by the decompressor itself.

use kobo_protocol::TaskError;
use miniz_oxide::inflate::{decompress_to_vec_with_limit, TINFLStatus};

/// The two bytes every gzip member begins with.
const MAGIC: [u8; 2] = [0x1f, 0x8b];

/// Deflate, the only compression method gzip has ever defined.
const DEFLATE: u8 = 8;

/// Magic, method, flags, modification time, extra flags, operating system.
const FIXED_HEADER: usize = 10;

/// The byte the flags live in.
const FLAGS: usize = 3;

/// A CRC over the header follows the other optional fields.
const FHCRC: u8 = 0b0000_0010;
/// A length-prefixed block of extra fields follows the fixed header.
const FEXTRA: u8 = 0b0000_0100;
/// A zero-terminated original file name follows.
const FNAME: u8 = 0b0000_1000;
/// A zero-terminated comment follows.
const FCOMMENT: u8 = 0b0001_0000;

/// Whether a header names an encoding this can read.
///
/// `x-gzip` is the name gzip was registered under first and is still sent by a
/// few hosts. An absent header, or `identity`, means the body arrived as it
/// was written and needs nothing done to it.
#[must_use]
pub fn is_gzip(encoding: &str) -> bool {
    let encoding = encoding.trim();
    encoding.eq_ignore_ascii_case("gzip") || encoding.eq_ignore_ascii_case("x-gzip")
}

/// Whether a header names no encoding at all.
///
/// Anything that is neither this nor [`is_gzip`] is something the runtime
/// never asked for and cannot read, which is a failure rather than a body.
#[must_use]
pub fn is_identity(encoding: &str) -> bool {
    let encoding = encoding.trim();
    encoding.is_empty() || encoding.eq_ignore_ascii_case("identity")
}

/// Expands one gzip member, refusing to produce more than `limit` bytes.
///
/// The limit is the same ceiling the task declared, applied to the expanded
/// size rather than the compressed one. That is the number that matters: a
/// megabyte of zeroes is a kilobyte on the wire, and a reader that agreed to
/// hold half a megabyte should not be handed fifty.
///
/// # Errors
///
/// [`TaskError::TooLarge`] when the body expands past `limit`, and
/// [`TaskError::Unreachable`] when it is not a gzip stream this can read,
/// which is the same answer the transport gives for any other reply it cannot
/// make sense of.
pub fn expand(body: &[u8], limit: u32) -> Result<Vec<u8>, TaskError> {
    let deflate = body
        .get(header_length(body)?..)
        .ok_or(TaskError::Unreachable)?;
    decompress_to_vec_with_limit(deflate, limit as usize).map_err(|error| match error.status {
        // The one failure that is not the server's fault, and the one the
        // caller has a different sentence for.
        TINFLStatus::HasMoreOutput => TaskError::TooLarge,
        _ => TaskError::Unreachable,
    })
}

/// How many bytes of `body` are header, and so where the deflate stream starts.
///
/// Every length below comes off the wire, so every step is checked against the
/// real length rather than trusted and indexed with.
fn header_length(body: &[u8]) -> Result<usize, TaskError> {
    let fixed = body.get(..FIXED_HEADER).ok_or(TaskError::Unreachable)?;
    if fixed[..2] != MAGIC || fixed[2] != DEFLATE {
        return Err(TaskError::Unreachable);
    }
    let flags = fixed[FLAGS];
    let mut at = FIXED_HEADER;

    if flags & FEXTRA != 0 {
        let length = body
            .get(at..at.checked_add(2).ok_or(TaskError::Unreachable)?)
            .ok_or(TaskError::Unreachable)?;
        // Little endian, like every other number in this container.
        let length = usize::from(u16::from_le_bytes([length[0], length[1]]));
        at = at
            .checked_add(2)
            .and_then(|at| at.checked_add(length))
            .ok_or(TaskError::Unreachable)?;
    }
    if flags & FNAME != 0 {
        at = after_terminator(body, at)?;
    }
    if flags & FCOMMENT != 0 {
        at = after_terminator(body, at)?;
    }
    if flags & FHCRC != 0 {
        at = at.checked_add(2).ok_or(TaskError::Unreachable)?;
    }
    if at > body.len() {
        return Err(TaskError::Unreachable);
    }
    Ok(at)
}

/// Steps past a zero-terminated field, or fails if it is never terminated.
fn after_terminator(body: &[u8], from: usize) -> Result<usize, TaskError> {
    let rest = body.get(from..).ok_or(TaskError::Unreachable)?;
    let end = rest
        .iter()
        .position(|byte| *byte == 0)
        .ok_or(TaskError::Unreachable)?;
    from.checked_add(end)
        .and_then(|at| at.checked_add(1))
        .ok_or(TaskError::Unreachable)
}

#[cfg(test)]
mod prose_tests {
    use super::{expand, is_gzip, is_identity, DEFLATE, FCOMMENT, FEXTRA, FHCRC, FNAME, MAGIC};
    use kobo_protocol::TaskError;
    use miniz_oxide::deflate::compress_to_vec;

    /// A gzip member carrying `content`, with the given optional fields.
    fn member(content: &[u8], flags: u8, optional: &[u8]) -> Vec<u8> {
        let mut out = vec![MAGIC[0], MAGIC[1], DEFLATE, flags, 0, 0, 0, 0, 0, 0xff];
        out.extend_from_slice(optional);
        out.extend_from_slice(&compress_to_vec(content, 6));
        out.extend_from_slice(&[0; 8]);
        out
    }

    #[test]
    fn a_body_the_server_compressed_comes_back_as_it_was_written() {
        let content = b"{\"feeds\":[\"one\",\"two\"]}";
        let expanded = expand(&member(content, 0, &[]), 1024).expect("a gzip body");
        assert_eq!(expanded, content);
    }

    #[test]
    fn the_optional_fields_are_stepped_over_rather_than_read_as_data() {
        let content = b"a reply";
        // Extra, then a name, then a comment, then a header CRC: every
        // optional field at once, which no server sends and all of which have
        // to be skipped correctly anyway.
        let mut optional = vec![3, 0, b'x', b'y', b'z'];
        optional.extend_from_slice(b"index.json\0");
        optional.extend_from_slice(b"written by a server\0");
        optional.extend_from_slice(&[0, 0]);
        let flags = FEXTRA | FNAME | FCOMMENT | FHCRC;
        let expanded = expand(&member(content, flags, &optional), 1024).expect("a gzip body");
        assert_eq!(expanded, content);
    }

    #[test]
    fn a_body_that_expands_past_the_ceiling_is_too_large_rather_than_unreadable() {
        // Compresses to almost nothing and expands to far more than the
        // ceiling, which is the shape of the attack this limit exists for.
        let content = vec![b'a'; 200_000];
        assert_eq!(
            expand(&member(&content, 0, &[]), 1024),
            Err(TaskError::TooLarge)
        );
    }

    #[test]
    fn a_body_that_is_not_gzip_is_reported_the_way_any_unreadable_reply_is() {
        for body in [
            b"not gzip at all".as_slice(),
            &[0x1f, 0x8b],
            // The right magic and a compression method that has never existed.
            &[0x1f, 0x8b, 9, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3],
        ] {
            assert_eq!(expand(body, 1024), Err(TaskError::Unreachable));
        }
    }

    #[test]
    fn a_header_that_promises_a_field_it_never_ends_is_refused() {
        // A name flag with no terminating zero anywhere: the field runs off
        // the end of the body, and reading it as a length would walk past it.
        let mut body = vec![MAGIC[0], MAGIC[1], DEFLATE, FNAME, 0, 0, 0, 0, 0, 0xff];
        body.extend_from_slice(b"a name that never ends");
        assert_eq!(expand(&body, 1024), Err(TaskError::Unreachable));
    }

    #[test]
    fn an_extra_field_longer_than_the_body_is_refused() {
        let mut body = vec![MAGIC[0], MAGIC[1], DEFLATE, FEXTRA, 0, 0, 0, 0, 0, 0xff];
        // Says sixty thousand bytes of extra follow, and sends two.
        body.extend_from_slice(&[0xff, 0xef, 1, 2]);
        assert_eq!(expand(&body, 1024), Err(TaskError::Unreachable));
    }

    #[test]
    fn the_names_a_server_may_give_this_encoding_are_all_recognised() {
        assert!(is_gzip("gzip"));
        assert!(is_gzip("x-gzip"));
        assert!(is_gzip(" GZIP "));
        assert!(!is_gzip("br"));
        assert!(!is_gzip("deflate"));

        assert!(is_identity(""));
        assert!(is_identity("identity"));
        assert!(!is_identity("gzip"));
    }
}

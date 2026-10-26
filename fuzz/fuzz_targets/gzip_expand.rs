#![no_main]
//! A gzip body from the far end of a socket expands to no more than the
//! limit the caller named, or is refused, and the frame around the deflate
//! stream is read without a panic however its optional fields are cut.
//!
//! The first four bytes of the input choose the limit, so the fuzzer also
//! explores bodies that are exactly at and just over it.

use libfuzzer_sys::fuzz_target;

const MOST: u32 = 4 * 1024 * 1024;

fuzz_target!(|data: &[u8]| {
    let (limit, body) = match data {
        [a, b, c, d, rest @ ..] => (u32::from_le_bytes([*a, *b, *c, *d]) % MOST, rest),
        _ => return,
    };
    if let Ok(expanded) = kobo_net::gzip::expand(body, limit) {
        assert!(
            expanded.len() <= limit as usize,
            "expansion must stay within the limit the caller named"
        );
    }
    let _ = kobo_net::gzip::is_gzip(&String::from_utf8_lossy(body));
});

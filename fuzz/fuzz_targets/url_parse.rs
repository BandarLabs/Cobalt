#![no_main]
//! A URL from a feed, a catalog or a Location header parses or is refused,
//! and a redirect resolved against it is never allowed to leave HTTPS.
//!
//! The input is split at its first newline: the part before is the address
//! and the part after is the Location a server answered with.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let (url, location) = text.split_once('\n').unwrap_or((text, ""));
    let Ok(address) = kobo_net::parse(url) else {
        return;
    };
    if let Ok(next) = kobo_net::resolve_redirect(&address, location) {
        assert!(
            next.starts_with("https://"),
            "a redirect must never downgrade a request to plaintext"
        );
        let _ = kobo_net::parse(&next);
    }
});

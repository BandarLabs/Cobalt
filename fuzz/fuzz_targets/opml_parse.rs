#![no_main]
//! An OPML export from any feed reader imports some number of feeds, possibly
//! none, or is refused; it never panics.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = kobo_opml::parse(data);
});

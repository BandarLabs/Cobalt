#![no_main]
//! An OPDS page in either of its shapes, Atom or JSON, parses to a feed or a
//! fault, and a malformed entry inside an otherwise valid page costs only that
//! entry.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = kobo_opds::parse(data, "https://catalog.example.invalid/opds");
});

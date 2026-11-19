#![no_main]
//! The XML scanner walks any string to its end or stops early, and never
//! panics or recurses. Entity decoding and attribute lookup take the same
//! bytes without complaint.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let mut decoded = Vec::new();
    let mut events = 0_usize;
    kobo_xml::scan(text, &mut decoded, |_| events += 1);
    let _ = kobo_xml::decode_entities(text);
    let _ = kobo_xml::attribute(text, "href");
    let _ = kobo_xml::split_name(text);
});

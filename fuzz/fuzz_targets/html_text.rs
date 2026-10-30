#![no_main]
//! A fragment of anyone's HTML becomes text within the ceiling, whatever its
//! shape, and the helpers around it refuse nothing by panicking.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(html) = std::str::from_utf8(data) else {
        return;
    };
    let _ = kobo_html::to_text(html);
    let _ = kobo_html::to_text_within(html, 64);
    let _ = kobo_html::decode_entities(html);
    let _ = kobo_html::attribute(html, "href");
    let _ = kobo_html::element_name(html);
    let _ = kobo_html::skip_element(html, "script");
});

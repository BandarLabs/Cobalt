#![no_main]
//! A file handed to the reader opens as a document or is refused, under every
//! name the format sniffer distinguishes, and the zip reader underneath the
//! EPUB path lists and extracts whatever directory it can find without a
//! panic.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(archive) = kobo_doc::zip::Archive::open(data) {
        let names: Vec<String> = archive.names().map(str::to_owned).collect();
        for name in names {
            let _ = archive.read(&name);
        }
    }
    for name in ["book.epub", "page.html", "notes.md", "plain.txt", "unknown"] {
        if let Ok(document) = kobo_doc::read(name, data) {
            assert!(
                document.blocks.len() <= kobo_doc::MAX_BLOCKS,
                "a document never holds more blocks than its ceiling"
            );
        }
    }
});

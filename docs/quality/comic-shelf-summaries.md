# Comic shelf summaries

`Memory::saved_page` shares the reader's bounded record parser. It returns no
position for an absent record, a zero-based page for a valid record, and an error
for damaged/newer state. It reads the recorded index without archive decoding;
use it only with unchanged page order. Opening the reader still resolves its
filename anchor. `ComicView::cover_preview` returns the shared bounded cover
thumbnail without changing the current page.

The shared comic/reader tests and strict all-target Clippy pass on Rust 1.85.1.
Added coverage checks summary/reader agreement for malformed records, missing
state, legacy positions, bounds and anchor behavior after page reordering.
Panels' shelf adoption and screenshots belong to the catalog PR.

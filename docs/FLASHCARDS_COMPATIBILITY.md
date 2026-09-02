# Flashcards package compatibility

Flashcards is unofficial, offline Anki-package compatibility software. It is
not affiliated with, endorsed by, or sponsored by Ankitects, Anki, AnkiDroid,
or AnkiWeb. It includes no upstream logo or artwork. Exact source pins, full
licence texts, dependency notices, and distribution requirements are in
[`licenses/NOTICE-Flashcards-Anki.md`](../licenses/NOTICE-Flashcards-Anki.md).

## Exact supported package boundary

The host accepts a bounded ZIP `.apkg` in merge mode or legacy `.colpkg` in
replacement mode when all of the following are true:

- there is exactly one SQLite `collection.anki2` or `collection.anki21`;
- `media` is the legacy JSON filename map and media members have numeric names;
- collection schema is **11 or 14 through 18**; and
- every used card template can be completed by pinned Anki rslib without
  JavaScript or an external/add-on filter.

The importer opens the copied collection with Anki rslib pinned at
`9e32ad8849068510a82273889c21b22e1acf0949`. That executes upstream integrity
checking, schema migration to 18, metadata normalization, existing-card
rendering, media-reference extraction, and scheduler queue construction.
There is no handwritten collection migration, scheduler, cloze renderer, or
template-filter substitute.

Schemas 12 and 13 are explicitly refused because this pinned rslib requires
those transient schemas to be cleanly downgraded first. Schemas below 11 and
above 18 are refused. Modern `meta`/`collection.anki21b` packages use the
protobuf media map and zstd collection path and are refused rather than
silently treated as legacy SQLite packages. Encrypted ZIP entries and
compression methods other than stored/deflate are also refused.

## Scheduling and rendering

The bundle preserves every note, card, normalized notetype, deck, deck
configuration, collection configuration/tag record, grave, scheduling field,
and revlog row. The review queue is separate from the complete card inventory.
For each top-level deck root, the host asks pinned rslib for its queue and
stores the returned order and new/learning/review counts. Deck limits, child
deck semantics, due cutoffs, learning-ahead behavior, and day-rollover
unburying therefore come from upstream. Suspended, still-buried, and non-due
cards do not enter the queue. Existing rslib card ordinals drive cloze
rendering, including multi-cloze notes.

Both sides are rendered with rslib's existing-card service. A partial upstream
render is first used to detect filters that rslib reserves for external code;
such add-on filters reject the package. Source templates containing script,
active embedded documents, JavaScript URLs, or event handlers are rejected.
Built-in Anki filters such as cloze, furigana, kanji, kana, hint, text, TTS,
and type-answer pass through rslib. Type-answer markers become explicitly
retained marker text followed by a visible
`[Type answer is unavailable on Kobo]` notice, with diagnostics; marker text is
never silently deleted. HTML and CSS never execute on the Kobo:
rendered HTML is reduced to bounded plain text and source styling remains inert
metadata.

## Media and image safety

Media names are NFC-normalized and path components, controls, duplicate
normalized names, duplicate JSON keys, and content/name conflicts are refused.
Image bytes must match an explicit supported filename extension; disguised or
extensionless image data is rejected rather than silently treated as a generic
attachment. Media references on each card side come from rslib's media
extractor. The app uses the question-side list before reveal and prefers
answer-only media after reveal. Because the current screen has one image slot,
a side that renders more than one image occurrence is rejected instead of
dropping or lexicographically reordering images. Any rendered media reference
missing from the package rejects import; diagnostics never stand in for bytes.
Referenced formats outside supported image, retained audio, and retained video
types also reject import instead of disappearing as generic attachments.

PNG and JPEG are decoded on the host through Cobalt's bounded image decoder
before publication and again from digest-verified bundle bytes during
verification. GIF and WebP are **not advertised or decoded** and are rejected;
this avoids claiming bounded animation/frame support that the device build
does not contain. Audio/video are retained as non-playing attachments.
Executable add-on media (`.js`, `.mjs`, `.cjs`, `.html`, `.wasm`) is rejected.

Accepted SVG is UTF-8 XML with at most 20,000 nodes and bounded intrinsic
dimensions/pixels. Entity declarations, active/animated/foreign elements,
event handlers, external/relative/absolute/data image references, external
CSS/font references, and non-fragment `url()` values are refused. resvg/usvg's
default file resolver is replaced with resolvers that always return `None`.
Only the bundled Atkinson Hyperlegible and DejaVu faces are available, and SVG
text needing another glyph is rejected. A harmless legacy doctype may be
stripped only when there are no entity declarations. The host creates a
deterministic greyscale PNG named from the source digest; the original SVG is
retained for reconciliation. Host verification/staging check every SVG, and
device admission checks every SVG referenced by the bounded due queue: both
re-parse/re-rasterize and require byte-for-byte equality with the PNG. The
review screen then decodes only that bounded PNG, so a crafted redirect cannot
detach displayed bytes from the retained source.

## Merge, replacement, and review records

An APKG merge keeps both source records and merges notes, notetypes, decks,
deck configurations, revlog, graves, diagnostics, queues, and media.
Identifier-equal metadata/revlog/grave records are deduplicated only when their
entire normalized content is equal. A differing identifier/content pair is
rejected. Card identifiers are never overwritten or deduplicated: any card-ID
collision rejects the merge. Equal same-named media is deduplicated by bytes;
differing bytes reject the merge. These rules are deterministic and do not
invent replacement IDs or filenames.

A legacy COLPKG replacement atomically replaces the Flashcards collection
bundle and does not merge the previous Anki metadata. The separately named,
owner-local `cobalt-review-log.ndjson` is the only documented Cobalt state
preserved by collection replacement/staging. Format-2 records bind card ID,
grade, imported scheduling snapshot, and the exact bundle SHA-256. Export
requires exact fields, a supported grade, a valid digest, bounded
newline-terminated records, and byte-for-byte verification after atomic write.
The log is not presented as an Anki scheduler round trip.

## Bounds and deterministic publication

The reader rejects path traversal, absolute paths, backslashes, controls,
directories, symlinks, duplicate members, suspicious expansion ratios, more
than 8,192 archive entries, a compressed package over 32 MiB, an expanded
archive over 116 MiB, a collection over 64 MiB, a media file over 4 MiB, a
512 KiB SVG source, or a bundle over 32 MiB. The decoded manifest and media
payload are capped at 16 MiB and 48 MiB respectively, and every payload byte
must belong to one sorted, contiguous, SHA-256-checked media record. Manifests
enforce sorted unique identifiers, bounded record counts, valid references,
exact queue counts, and canonical side-media unions. The due queue is capped at
512 cards so device-side SVG binding checks and a review session remain
bounded. Publication and Kobo staging use synced partial files, digest checks,
and atomic rename.

The private owner-deck equivalence test is opt-in so the package and its media
never enter source control:

```sh
COBALT_ANKI_EQUIVALENCE_APKG=/path/to/private.apkg \
  cargo test -p kobo-flashcards-import \
  private_owner_deck_matches_pinned_rslib_aggregates_when_available
```

It compares only aggregate hashes/counts for rendered question/answer identity,
side media references, notes/cards, and upstream scheduler queues. It does not
print card text, media filenames, SSIDs, credentials, or package contents.

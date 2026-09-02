# Flashcards package compatibility

Flashcards is an unofficial, offline Anki-package compatibility feature. It is
not affiliated with, endorsed by, or sponsored by Ankitects, Anki, AnkiDroid,
or AnkiWeb, and it uses no upstream logos or trademarks. Licensing and pinned
upstream source details are in
[`licenses/NOTICE-Flashcards-Anki.md`](../licenses/NOTICE-Flashcards-Anki.md).
This is engineering guidance, not legal advice.

## Import and transfer

`flashcards-import` reads only a bounded `.apkg` in merge mode or a `.colpkg`
in replacement mode and creates one deterministic, compressed
`collection.cobfc` bundle. `flashcards-import stage BUNDLE --kobo-root MOUNT`
copies only to the fixed Flashcards shelf entry on a mounted USB volume.
`flashcards-import verify BUNDLE` additionally decodes every referenced image
through the same Kobo image primitive (including safe SVG rasterization) before
reporting that image references resolve to digest-verified bytes.
An APKG can merge with an existing Flashcards bundle only when card identifiers
do not collide and same-named media have the same digest. Differing media is
refused rather than renamed or overwritten. A COLPKG always replaces the
target atomically.

The archive reader rejects absolute paths, `..`, backslashes, controls,
directories, symlinks, duplicate members, duplicate NFC-normalized media
names, corrupt data, suspicious decompression ratios, more than 8,192
members, a 32 MiB compressed bundle, a 64 MiB SQLite collection or decoded
manifest/media payload, or a media file over 4 MiB. It validates known image signatures, normalizes media names to NFC,
records SHA-256 content digests, sorts media deterministically, and never
extracts an archive pathname. A bundle is parsed and digest-verified before
the final atomic rename. An interruption leaves the prior final bundle
unchanged.

Cobalt's existing shelf transfer is the device-side transaction: it sends
256 KiB chunks to a hidden partial file, accepts only contiguous offsets,
syncs before publication, and atomically renames only the last chunk. A
restart begins at zero safely; a partial collection cannot be opened.

## Rendered data

The host retains original notetype/model, deck, deck-configuration, and
template JSON alongside card ordinal, tags, hierarchy (`::`), scheduling
queue/type/due/interval/ease/repetition/lapse/learning fields, and complete
legacy revlog records. The Kobo renderer renders plain text only. It supports
ordinary field substitution, `FrontSide`, `Tags`, `text`, `hint`, basic
conditionals, and cloze deletion rendering. It retains card-level diagnostics
for an unsupported filter rather than manufacturing a semantically different
card. HTML is reduced to text, CSS is retained as inert source metadata, and
JavaScript is never executed. Referenced PNG/JPEG/GIF/WebP image bytes are
bundled and rendered only after Cobalt's bounded image decoder accepts them;
a safe SVG source is retained and rasterized in the device image path.
Image-occlusion media is retained; interactive occlusion drawing is not implemented. Sound, video,
and TTS references are explicit non-playing attachments.

“Full Anki package compatibility” in this project means using upstream package
and render semantics where they can be represented safely, with explicit
unsupported interactive and add-on execution boundaries. It never means that
add-on JavaScript, arbitrary HTML, sockets, local paths, or filesystem access
will run on a Kobo.

## Present scope and reconciliation boundary

The current host implementation accepts legacy SQLite `collection.anki2`
schemas 11 through 18. Modern protobuf collection members (`collection.anki21`
and `collection.anki21b`) are intentionally refused pending a narrowly scoped
host helper built from Anki rslib at the pinned revision. They are not
silently downgraded. FSRS parameters and imported scheduling fields remain
verbatim in the bundle; the Kobo does not rewrite them. A review action creates
a Cobalt-local event only after its shelf transaction replies. Events are
newline-delimited and can be copied byte-for-byte from a mounted Kobo with
`flashcards-import export-review-log --kobo-root MOUNT OUTPUT.ndjson`; this is
the lossless Cobalt review-log export for future host reconciliation. It does
not claim to round-trip an Anki scheduler or write an APKG.

These explicit boundaries are preferable to a compatibility claim that would
corrupt a collection. The self-generated fixture tests basic, reversed,
cloze, Unicode, CSS retention, image and sound references, malformed SQLite,
path traversal, duplicate media, unsupported filters, interrupted publication,
and an oversized compressed archive.

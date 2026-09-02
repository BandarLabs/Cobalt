# Flashcards

Flashcards reviews an offline `collection.cobfc` bundle in its private Kobo
shelf. It is unofficial Anki-package compatibility software and is not
affiliated with Ankitects or AnkiDroid, and it uses no upstream logo. See
[`licenses/NOTICE-Flashcards-Anki.md`](../../licenses/NOTICE-Flashcards-Anki.md)
for exact source pins, licences, dependency notices, and distribution terms.

Prepare and stage a collection on the host, with the Kobo USB volume mounted
at `MOUNT` and Flashcards closed:

```sh
cargo run -p kobo-flashcards-import -- \
  import deck.apkg --merge collection.cobfc
cargo run -p kobo-flashcards-import -- \
  stage collection.cobfc --kobo-root MOUNT
```

The staging command copies only to the fixed private shelf entry
`.adds/cobalt/data/flashcards/collection.cobfc`. It writes 256 KiB durable
chunks with a digest-checked resume record and atomically replaces the final
entry only after fully validating the bundle. The application reads that one
validated name and refuses corrupt, unbounded, or path-addressable content.

The host derives the due queue, deck order/limits, cloze ordinals, both rendered
sides, and side-specific media references from pinned Anki rslib. The device
reviews that finite queue once per launch; non-due, suspended, and buried cards
are absent. Review grades append only to the separately preserved,
bundle-digest-bound Cobalt owner log and do not claim to update Anki scheduling.

The device draws bounded PNG/JPEG only. Accepted SVG is parsed with no
file/data/network resolver and controlled bundled fonts on the host, then
stored as a digest-addressed greyscale PNG. At admission the Kobo re-rasterizes
SVG sources referenced by the bounded due queue and requires exact PNG
equality, then displays only the PNG. GIF and WebP are explicitly unsupported.
Audio and video remain visible as non-playing attachments and cannot cause
playback or network activity. Answer-only media is selected only after reveal.
A card side with more than one rendered image is rejected on the host rather
than silently dropping or reordering images for the app's single image slot.

Choose **Notices** on the question screen to read the non-affiliation notice,
exact source pins, full Anki/AnkiDroid/resvg terms, font terms, and resolved
device dependency notices embedded in every `.cobalt-app` executable.

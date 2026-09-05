# Flashcards

Flashcards reviews a Cobalt-owned neutral `collection.cobfc` bundle in its
private Kobo shelf. The device application contains no linked study-engine
code, collection migration logic, upstream logo, or remote-network capability.
It uses only Cobalt's required local Unix-domain runtime IPC.

The separate host converter accepts only the legacy package subset documented
in [`docs/FLASHCARDS_COMPATIBILITY.md`](../../docs/FLASHCARDS_COMPATIBILITY.md)
and uses pinned Anki rslib there. Its exact source and AGPL obligations are
host-artifact notices, not device-package notices.

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
Pre-neutral version-3 bundles are intentionally rejected: rerun the current
host import and stage commands to create version 4. The separately stored local
review log is not replaced.

The host derives the due queue, deck order/limits, cloze ordinals, both rendered
sides, and side-specific media references from pinned Anki rslib. The device
reviews that finite queue once per launch; non-due, suspended, and buried cards
are absent. Review grades append only to the separately preserved,
bundle-digest-bound Cobalt owner log and do not claim to update Anki scheduling.

The device UI is a sparse portrait review surface rather than a desktop-Anki
clone: choose a due deck, read one dominant card, reveal it with one primary
action, then grade with stable Again/Hard/Good/Easy positions. Long cards turn
as measured pages without moving or clipping the controls. Secondary
information, settings, and licences stay behind the top-bar menu. Semantic
emphasis survives host conversion, while arbitrary template CSS remains inert.

The device draws bounded PNG/JPEG only. Accepted SVG is parsed with no
file/data/network resolver and controlled bundled fonts on the host, then
stored as a digest-addressed greyscale PNG. At admission the Kobo re-rasterizes
SVG sources referenced by the bounded due queue and requires exact PNG
equality, and decodes every due-card PNG/JPEG before accepting the bundle. It
then displays only checked raster bytes. GIF and WebP are explicitly
unsupported. Audio and video remain visible as non-playing attachments and
cannot cause playback or network activity. Answer-only media is selected only
after reveal. A card side with more than one rendered image is rejected on the
host rather than silently dropping or reordering images for the app's single
image slot.

Atkinson Hyperlegible remains the interface face. Japanese card and SVG text
uses the deterministically derived Cobalt Japanese subset; unsupported glyphs
fail closed instead of drawing empty boxes. Its SIL OFL text, exact Noto CJK
source pin, and reproducible subset instructions ship in both applicable
artifacts.

Choose **Licences & about** from the top-bar menu to read the device notice,
resvg/font terms, source pins, and resolved device dependency notices embedded
in every `.cobalt-app` executable. Anki source and licence notices are
intentionally absent because the device binary does not link Anki code.

State-by-state 1072×1448 golden captures are under `screenshots/states/`.

| Question | Answer |
| --- | --- |
| ![Japanese question with bounded SVG media and one Reveal answer action](screenshots/states/question-japanese-svg.png) | ![Revealed answer with stable Again, Hard, Good and Easy controls](screenshots/states/answer-reveal.png) |

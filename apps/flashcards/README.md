# Flashcards

Flashcards reviews an offline `collection.cobfc` bundle in its private Kobo
shelf. It is unofficial Anki-package compatibility software and is not
affiliated with Ankitects or AnkiDroid. See
[`licenses/NOTICE-Flashcards-Anki.md`](../../licenses/NOTICE-Flashcards-Anki.md)
for the pinned upstream sources, licences, and distribution notice.

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

The device runs cards as plain text. Images referenced by cards are preserved
and decoded through Cobalt's bounded image primitive. Audio and video remain
visible as non-playing attachments; they do not disappear and cannot cause
playback or network activity.

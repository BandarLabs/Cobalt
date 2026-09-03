# Flashcards — compatibility gate

Flashcards is an offline review surface. It is **not device-ready for Anki
collections**: APKG and COLPKG import is deliberately disabled until the host
uses Anki's own collection, rendering, and media implementation. This prevents
silently changing cards or discarding media.

![Flashcards deck list on the Kobo panel](screenshots/decks.png)

## Import and library

`kobo flashcards import deck.apkg --out deck.flashcards` currently refuses the
input and writes nothing. The device's **Transfer** screen reports the same
gate. Existing device libraries can still be reviewed, and their reviews and
one-level Undo are saved transactionally, but no claim of Anki compatibility is
made for them.

The previous host importer was removed because it incorrectly stripped HTML,
used only two note fields, guessed basic/cloze rendering, dropped every media
file, and did not preserve Anki scheduling, deck options, or revlog. Refusal is
atomic: it leaves both a collection and an output path untouched.

## Exact current compatibility

| Feature | Status |
| --- | --- |
| Basic and cloze templates; field substitutions | Not imported |
| HTML/CSS rendering | Not imported |
| Deck hierarchy and options | Not imported |
| Current scheduling metadata and revlog | Not imported |
| Images, audio tags, and media map | Not imported |
| Unicode | Not imported from Anki packages |
| Existing device-only review state | Supported, local FSRS only |

Anki's upstream Rust library is AGPL-3.0-or-later, compatible with this
AGPL-3.0-only workspace, and can remain host-only even though it pulls network
dependencies. The evaluated upstream is Anki
`9e32ad8849068510a82273889c21b22e1acf0949`. However, it is `publish = false`,
has no supported crates.io embedding package, and its import/render APIs are
tied to a pinned upstream workspace. A correct integration must pin and test
that workspace before this gate can be removed.

## Scheduling gate

`fsrs` 6.6.2 (BSD-3-Clause) remains device-only and socket-free. It schedules
only an existing device library; it is not a substitute for Anki's scheduler,
deck options, or revlog. The device application still has no SQLite, ZIP, HTTP,
or socket-capable dependency.

## Dependencies

- `fsrs` 6.6.2 — BSD-3-Clause; local FSRS scheduling.
- No SQLite, ZIP, HTTP, or socket-capable dependency is linked into the device
  application. Package extraction is host-only.

## Repaint policy

A card repaints once to reveal its answer and once to show the next question.
Idle review screens do not poll or repaint. Transfer progress changes only when
a shelf chunk arrives.

## Simulator

```sh
cargo run -p kobo-cli -- run --sim --app flashcards
cargo run -p kobo-cli -- drive --ideal --script apps/flashcards/drive.kobo --shots apps/flashcards/screenshots
```

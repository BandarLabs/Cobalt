# PARSER-01 · An original tutorial story

The catalog already promises "an original interactive story" (apps/parser/cobalt-app.json
summary), so this is truth-in-copy, not a nice-to-have. No Z-machine compiler exists in
the repo; the only authoring path is the hand-encoding test machinery in
apps/parser/src/zvm/mod.rs (`story()`, `code_story()`).

## Shape

A small Rust story-builder module in the app (`apps/parser/src/story.rs`) that emits a
**v3** story file from a compact room/object/verb description. v3 because the object
table is 9-byte entries with 4-byte property headers, the dictionary is 4-byte words,
and the compiled Lamplight fixture already proves the interpreter's v3 paths end to end.

The builder owns the layout the test helpers currently hand-fix: header, dictionary,
object table, property lists, and routines. It grows only what the tutorial needs:

- rooms: name, description, exits
- objects: name, adjectives, parent room, description, portable flag
- verbs: LOOK, EXAMINE x, TAKE x, DROP x, INVENTORY, OPEN x, N/S/E/W/UP/DOWN,
  SAVE/RESTORE (real opcodes, free), QUIT, plus an unknown-command line
- one tiny goal so the story can end: take the lamp, light it, carry it to the last
  room -> closing paragraph and quit-to-library

## Story

Working title "First Light". Three rooms (a gatehouse, a storeroom, a stair top), three
objects (a note that teaches EXAMINE, an unlit lamp, a match). The note's text names
every verb the reader needs, so the tutorial teaches inside the fiction. Original prose,
written for this app, AGPL-3.0-only like the rest of the tree; no borrowed text.

## Delivery

`build_tutorial_story()` is unit-tested (header facts, dictionary lookups, a scripted
walkthrough that completes the goal, a save/restore round trip mid-story), the emitted
bytes are committed as `apps/parser/story/first-light.z3`, and `main.rs` seeds it onto
the shelf with `include_bytes!` when the library is empty, so a first run shows one
story instead of transfer instructions. Library empty-state copy stays for a shelf the
reader later empties.

## Proof before board close

- unit walkthrough + round trip green
- drive route extended: first run shows First Light, play the goal to the end
- 27-cell matrix at the head, picker/playing shots inspected at default + extra-large
- version bump + regenerated catalog page; README game ledger row (bundled, AGPL)

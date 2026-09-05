# Parser

Parser turns a Kobo into an offline interactive-fiction reader. It executes
text-only Z-machine v3, v5 and v8 story files, typesets the transcript as prose,
and moves through long sessions with ordinary page turns.

## Transfer a story

Parser never downloads games or sends play data over the network. Transfer a
story you already own over Cobalt's authenticated owner connection:

```sh
kobo parser push game.z5 --device 192.168.1.23
```

Open Parser and tap **Refresh library**. Invalid versions are rejected before
transfer; Glulx is named explicitly and politely refused. Story files are kept
in Parser's private shelf and transferred atomically, so interruption cannot
publish a partial game.

## Playing

- Type any command with the platform keyboard.
- Tap common commands: LOOK, INVENTORY, EXAMINE, TAKE, compass directions,
  UNDO, SAVE, RESTORE and AGAIN.
- EXAMINE and TAKE leave the input line open for a noun.
- Tap a word in the transcript to append it to the command line.
- Turn pages at either side of the transcript or with physical page buttons.
- SAVE opens ten per-story Quetzal slots.
- Parser writes a separate Quetzal autosave after every accepted turn and on
  suspend, background and exit. Reopening a story silently restores it.

Timed input in v5 is deliberately treated as ordinary turn-based input.
Graphics, sound, v6, Glulx, TADS and Ink are not supported.

## Interpreter and test status

The interpreter in `src/zvm/` is original AGPL-3.0-only code following
[Z-Machine Standards 1.1][standard]. It implements instruction decoding,
routine frames, variables and stacks, objects/properties, dictionaries and
tokenisation, Z-strings/abbreviations, arithmetic/branches, deterministic
randomness, text/keyboard I/O, status lines, undo, and Quetzal IFZS
save/restore for v3, v5 and v8.

Generated legal fixtures exercise deterministic startup and output on all
three supported versions, format refusal, UTF-8-safe transcript pagination,
word taps, Clara layout diagnostics, and Quetzal round trips.

The upstream **czech** and **praxix** suites and attended full-game runs have
not yet been executed on Kobo hardware. They are not bundled because this
repository has not completed the per-work redistribution audit. Until those
runs are recorded, Parser should be treated as an implementation preview
rather than a claim of full Standards conformance.

## Game and license ledger

| Work | Bundled | License |
| --- | --- | --- |
| Generated interpreter fixtures | Tests only | AGPL-3.0-only |
| Advent 350 | No | Not audited for this distribution |
| Modern v8 story | No | Not selected or audited |
| Infocom commercial stories (including Zork) | **Never** | Proprietary; readers must sideload copies they are entitled to use |

No third-party story is distributed. `encrusted` (MIT) was consulted only as
the request's named reference implementation; no source was copied and no
runtime dependency was added.

[standard]: https://inform-fiction.org/zmachine/standards/z1point1/index.html

# Flashcards compatibility and distribution notice

`flashcards-import` is an unofficial, offline host converter for a deliberately
limited set of Anki package files. It is not affiliated with, endorsed by, or
sponsored by Ankitects, Anki, or AnkiWeb. The names identify an input format
and upstream library only. No upstream logo or artwork is included.

## Exact upstream source pins

| Project | Exact source | Use in Flashcards | Licence |
| --- | --- | --- | --- |
| Anki | <https://github.com/ankitects/anki/tree/9e32ad8849068510a82273889c21b22e1acf0949> | The host helper links the pinned `anki` (rslib), `anki_i18n`, `anki_io`, and `anki_proto` crates for collection migration/normalization, scheduler queues, media-reference extraction, card rendering, localization, I/O support, and generated service types. | AGPL-3.0-or-later; see `LICENSE-Anki.txt` |
| resvg/usvg 0.45.1 | <https://github.com/linebender/resvg/tree/1b6c2fddbcbeffa8135df4323b02aaae84890907> | The host helper rasterizes accepted SVG media with all file/data/network image resolvers disabled and only bundled fonts. At admission the Kobo app uses the same bounded path only to verify due-card source/raster equality, then displays the PNG. | Apache-2.0 OR MIT; see `LICENSE-resvg.txt` |
| Cobalt Japanese font subset | Noto CJK revision `165c01b46ea533872e002e0785ff17e44f6d97d8` | Bounded Japanese card/SVG text coverage. The derived font is renamed and deterministically reproducible; unsupported glyphs reject import. | SIL Open Font License 1.1; see `LICENSE-Cobalt-Japanese-font.txt` and `SOURCE-Cobalt-Japanese-font.md` |

Corresponding-source retrieval and build instructions are in
`SOURCE-Flashcards-Anki.md`. Non-Anki Rust dependency notices for the host
helper are in `LICENSE-Flashcards-host-dependencies.txt`; the linked Anki
packages are covered separately by this notice, `LICENSE-Anki.txt`, and the
source instructions. The device application's resolved dependency notices are
in `LICENSE-Flashcards-device-dependencies.txt`. Atkinson Hyperlegible and
DejaVu font terms are in the `kobo-text/fonts` licence files. Cobalt Japanese
font terms and the exact source/subsetting recipe are in the files named
above.

`LICENSE-Anki.txt` reproduces Anki's upstream notice verbatim before the AGPL
text. References to other projects inside that upstream notice are retained
only to keep Anki's notice complete; they are not implementation dependencies.

## Distribution

Keep this notice, `LICENSE-Anki.txt`, `SOURCE-Flashcards-Anki.md`,
`LICENSE-Cobalt-Japanese-font.txt`, `SOURCE-Cobalt-Japanese-font.md`, the host
dependency notice bundle, and exact source pins with every distributed
`flashcards-import` helper. The host command embeds and prints them with
`flashcards-import --licenses`; `--notice` prints the short non-affiliation
notice.

The Kobo `.cobalt-app` contains no linked Anki code, so it intentionally does
not carry the Anki licence/source notice. Its Notices screen exposes only the
terms for code and assets actually present in the device binary.

Cobalt is AGPL-3.0-only. This engineering record is not legal advice.

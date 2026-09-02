# Flashcards compatibility and distribution notice

Flashcards is unofficial, offline compatibility software for Anki package
files. It is not affiliated with, endorsed by, or sponsored by Ankitects,
Anki, AnkiDroid, or AnkiWeb. The names identify compatible file formats only.
No upstream logo or artwork is included.

## Exact upstream source pins

| Project | Exact source | Use in Flashcards | Licence |
| --- | --- | --- | --- |
| Anki | <https://github.com/ankitects/anki/tree/9e32ad8849068510a82273889c21b22e1acf0949> | The host helper links the pinned `rslib`, `anki_proto`, and `anki_i18n` code for collection migration/normalization, scheduler queues, media-reference extraction, and card rendering. | AGPL-3.0-or-later; see `LICENSE-Anki.txt` |
| AnkiDroid | <https://github.com/ankidroid/Anki-Android/tree/20107044ee1934ffa7479ef969e453eb51f436f0> | Compatibility research only; no AnkiDroid code or artwork is linked or copied. Its exact provenance and terms remain shipped to avoid implying affiliation or hidden reuse. | GPL-3.0-or-later; see `LICENSE-AnkiDroid.txt` |
| resvg/usvg 0.45.1 | <https://github.com/linebender/resvg/tree/1b6c2fddbcbeffa8135df4323b02aaae84890907> | The host helper rasterizes accepted SVG media with all file/data/network image resolvers disabled and only bundled fonts. At admission the Kobo app uses the same bounded path only to verify due-card source/raster equality, then displays the PNG. | Apache-2.0 OR MIT; see `LICENSE-resvg.txt` |

The host helper's complete resolved dependency notices are in
`LICENSE-Flashcards-host-dependencies.txt`. The device application's resolved
dependency notices are in `LICENSE-Flashcards-device-dependencies.txt`.
Atkinson Hyperlegible and DejaVu font terms are in the `kobo-text/fonts`
licence files.

## Distribution

Keep this notice, the applicable full licence texts, dependency notice bundle,
and exact source pins with every host helper or device package containing the
corresponding code. The pathless `.cobalt-app` package carries them inside the
Flashcards executable and exposes paged notices from the review screen. The
host command carries the same texts and prints them with
`flashcards-import --licenses`; `--notice` prints the short non-affiliation
notice.

Cobalt is AGPL-3.0-only. This engineering record is not legal advice.

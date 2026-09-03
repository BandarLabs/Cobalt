# Third-party licences

Cobalt is licensed under the GNU Affero General Public License, version 3. It
links code and embeds fonts written by other people, all under permissive
licences that impose no copyleft of their own, so a binary built from this tree
can be redistributed under the AGPL as long as the notices below travel with
it.

## Rust dependencies

Every crate in the dependency graph, grouped by the licence selected for the
binary distribution. Regenerate the source inventory with:

```
cargo metadata --format-version 1 --all-features
```

| Selected licence | Crates |
| --- | --- |
| Apache-2.0 | dependencies that offer Apache-2.0 as an alternative, including `image`, `http`, `rustls`, `libc`, `png`, `flate2`, `ttf-parser`, and their transitive dependencies |
| MIT | `bytes`, `byteorder-lite`, `memchr`, `minimp3`, `minimp3-sys`, `pulldown-cmark`, `simd-adler32`, `slice-ring-buffer`, `vt100` |
| Apache-2.0 and ISC | `ring` |
| ISC | `rustls-webpki`, `untrusted` |
| BSD-3-Clause | `subtle` |
| CDLA-Permissive-2.0 | `webpki-roots` |
| Zlib | `foldhash` |

Proc-macro and build-script crates (`unicode-ident` and its dependents, `cc`,
and similar) run at compile time only; no code from them ships in a binary, so
their terms do not attach to the distribution.

The release archive contains the complete selected terms and package-specific
notices in `licenses/LICENSE-Rust-dependencies.txt`. That includes `ring`'s
Apache-2.0 and ISC terms and the CDLA-Permissive-2.0 agreement for the Mozilla
CA data bundled by `webpki-roots`.

## Icons

The icon geometry every application draws comes from a published set rather
than being drawn here.

| Set | Licence | File |
| --- | --- | --- |
| Tabler Icons 3.46.0 | MIT | `licenses/LICENSE-Tabler.txt` |

The artwork is converted once, offline, into checked-in Rust at
`crates/kobo-ui/src/vector/tabler.rs`, so nothing at build time or run time
reads an SVG or reaches the network. `scripts/import-icons.sh` reproduces that
file from the upstream tag, and `tools/icon-import/icons.txt` records which
icon stands behind which `Glyph`.

MIT imposes no copyleft, so this geometry travels inside an AGPL binary
without changing anything about it. What it does ask is that the notice
travels too, which is what the file above is for.

## Fonts

Two typefaces are embedded in the `kobo-text` crate and end up inside every
binary. Atkinson Hyperlegible is embedded in two weights, which the one licence
below covers. Flashcards additionally embeds the bounded Cobalt Japanese font
subset in its own host/device dependency closure. Their licences ship beside
the artifacts that contain them.

| Font | Licence | File |
| --- | --- | --- |
| Atkinson Hyperlegible (Regular and Bold) | SIL Open Font License 1.1 | `crates/kobo-text/fonts/LICENSE-AtkinsonHyperlegible.txt` |
| DejaVu Sans | Bitstream Vera and Arev fonts licence | `crates/kobo-text/fonts/LICENSE-DejaVu.txt` |
| Cobalt Japanese (derived Noto Sans CJK JP subset) | SIL Open Font License 1.1 | `licenses/LICENSE-Cobalt-Japanese-font.txt`; source and deterministic subset recipe in `licenses/SOURCE-Cobalt-Japanese-font.md` |

Both permit embedding and redistribution. The OFL forbids selling the font on
its own and requires the reserved name to be kept, which embedding does not
touch.

## Flashcards host conversion

The host-only converter is unofficial and is not affiliated with Ankitects,
Anki, or AnkiWeb. It uses no upstream logo or artwork. It links Anki rslib and
its i18n/proto support at one exact pinned revision. The Anki AGPL terms, resvg
source pin, exact revision, and host distribution requirements are in
[`licenses/NOTICE-Flashcards-Anki.md`](licenses/NOTICE-Flashcards-Anki.md).
Full Anki terms and corresponding-source instructions are in
`licenses/LICENSE-Anki.txt` and `licenses/SOURCE-Flashcards-Anki.md`.

The Kobo app consumes a Cobalt-owned neutral bundle and links no Anki code.
Consequently its package intentionally omits the Anki source/licence notice.
Resolved non-Anki dependency notices for the host helper and complete resolved
dependency notices for the device app are in
`licenses/LICENSE-Flashcards-host-dependencies.txt` and
`licenses/LICENSE-Flashcards-device-dependencies.txt`. The host's linked Anki
packages are noticed separately in the Anki notice/licence/source files. These
texts are embedded only in the corresponding artifacts; the app exposes its
device notices as paged text and the host helper prints its host notices with
`--licenses`.
Regenerate both deterministically with
`scripts/generate-flashcards-licenses.py`; its accepted SPDX policy is scoped
in `licenses/flashcards-about.toml`.

## Flashcards SVG rendering

The host Flashcards importer links `resvg`/`usvg` 0.45.1 to rasterize accepted
SVG image media before publication. The device application links the same
bounded path to verify due-card source/raster equality at admission, then
displays only the digest-addressed PNG. resvg is dual-licensed Apache-2.0 or
MIT; both selected terms travel in
[`licenses/LICENSE-resvg.txt`](licenses/LICENSE-resvg.txt).

## Services

The example applications talk to services this project does not own.

| Application | Service | Terms |
| --- | --- | --- |
| `hn` | Hacker News Firebase API | Public, unauthenticated, no key |
| `gutenbird` | Gutendex and Project Gutenberg | Public; Gutenberg texts are public domain in the US |
| `rss` | Feedsearch | Public; the search screen carries the attribution its terms ask for |

None of them is paid for or rate-limit-exempt. An application that hammers
them is your responsibility, not theirs.

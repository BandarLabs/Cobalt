# Third-party licences

Cobalt is MIT licensed. It links code and embeds fonts written by other
people, all under permissive licences. Nothing here is copyleft, so a binary
built from this tree can be redistributed under the MIT terms as long as the
notices below travel with it.

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

The release archive contains the complete selected terms and package-specific
notices in `licenses/LICENSE-Rust-dependencies.txt`. That includes `ring`'s
Apache-2.0 and ISC terms and the CDLA-Permissive-2.0 agreement for the Mozilla
CA data bundled by `webpki-roots`.

## Fonts

Two typefaces are embedded in the `kobo-text` crate and end up inside every
binary. Their licences ship beside them.

| Font | Licence | File |
| --- | --- | --- |
| Atkinson Hyperlegible | SIL Open Font License 1.1 | `crates/kobo-text/fonts/LICENSE-AtkinsonHyperlegible.txt` |
| DejaVu Sans | Bitstream Vera and Arev fonts licence | `crates/kobo-text/fonts/LICENSE-DejaVu.txt` |

Both permit embedding and redistribution. The OFL forbids selling the font on
its own and requires the reserved name to be kept, which embedding does not
touch.

## Services

The example applications talk to services this project does not own.

| Application | Service | Terms |
| --- | --- | --- |
| `hn` | Hacker News Firebase API | Public, unauthenticated, no key |
| `gutenbird` | Gutendex and Project Gutenberg | Public; Gutenberg texts are public domain in the US |
| `rss` | Feedsearch | Public; the search screen carries the attribution its terms ask for |

None of them is paid for or rate-limit-exempt. An application that hammers
them is your responsibility, not theirs.

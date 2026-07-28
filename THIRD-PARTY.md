# Third-party licences

Cobalt is MIT licensed. It links code and embeds fonts written by other
people, all under permissive licences. Nothing here is copyleft, so a binary
built from this tree can be redistributed under the MIT terms as long as the
notices below travel with it.

## Rust dependencies

Every crate in the dependency graph, grouped by the licence its author
declared. Regenerate this list with:

```
cargo metadata --format-version 1 --all-features
```

| Licence | Crates |
| --- | --- |
| MIT or Apache-2.0 | the majority of the graph, including `serde`, `libc`, `image`, `png`, `flate2`, `rsa`, `sha2`, `rand`, `rustls-rustcrypto`, `rustls-pki-types`, `ttf-parser` |
| MIT | `pulldown-cmark`, `generic-array`, `libm`, `simd-adler32`, `spin`, `vt100` |
| Apache-2.0 or ISC or MIT | `rustls` |
| Apache-2.0 and ISC | `ring` |
| ISC | `rustls-webpki`, `untrusted` |
| BSD-3-Clause | `curve25519-dalek`, `ed25519-dalek`, `x25519-dalek`, `subtle` |
| BSD-3-Clause or Apache-2.0 | `moxcms`, `pxfm` |
| CDLA-Permissive-2.0 | `webpki-roots` |
| Zlib | `foldhash` |
| MIT or Apache-2.0 or Zlib | `fontdue`, `miniz_oxide`, `zune-core`, `zune-jpeg` |
| Unlicense or MIT | `memchr`, `byteorder-lite` |
| Apache-2.0 | `unicode-linebreak` |

`ring` is the one that carries an obligation beyond attribution: its Apache-2.0
half requires that its `LICENSE` file be included with any redistribution of a
binary. `webpki-roots` bundles Mozilla's CA set under CDLA-Permissive-2.0,
which also requires the notice to be kept.

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

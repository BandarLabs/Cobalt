# Atkinson Hyperlegible, with a plain zero

`crates/kobo-text/fonts/AtkinsonHyperlegible-Regular.ttf` and
`AtkinsonHyperlegible-Bold.ttf` are the Braille Institute's Atkinson
Hyperlegible, licensed under the SIL Open Font License 1.1 with no Reserved
Font Name, modified in one glyph.

The zero is drawn as a plain oval instead of the original slashed zero. Set in
times, dates, scores and counts across the interface, the slash read as a code
font. The outer contour is kept; the two counters the slash split are replaced
by one, the outer contour scaled about its centre so the stroke matches the
slashed zero's own at its sides, top and bottom. Advance widths, side bearings
and every other glyph are unchanged, so nothing measured in the face moves.
The zero remains distinct from the capital O, which is a fifth wider.

The modification is reproducible from the files it replaced (git blobs
`b3cc4200fcf09d600eac74002ab4ea65b404a536` and `ca3f613847de62b8da83d2dab500ec391c8fe477`) with fontTools:

```sh
python tools/fonts/plain-zero.py AtkinsonHyperlegible-Regular.ttf AtkinsonHyperlegible-Regular.ttf
python tools/fonts/plain-zero.py AtkinsonHyperlegible-Bold.ttf AtkinsonHyperlegible-Bold.ttf
```

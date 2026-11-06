# Literata subset

`crates/kobo-text/fonts/Literata-Regular.ttf`, `Literata-SemiBold.ttf` and
`Literata-Italic.ttf` are subsets of the static TTFs in the Literata 3.103 release
(https://github.com/googlefonts/literata/releases/tag/3.103, `fonts/ttf/`),
licensed under the SIL Open Font License 1.1 with no Reserved Font Name.

They were made with fontTools' `pyftsubset`:

```sh
pyftsubset Literata-Regular.ttf \
  --unicodes='U+0020-007E,U+00A0-017F,U+2010-2027,U+2030-203A,U+2044,U+20AC,U+2122,U+2212,U+FB01-FB02' \
  --layout-features='kern' --no-hinting --desubroutinize \
  --name-IDs='0,1,2,3,4,5,6,13,14' \
  --output-file=Literata-Regular.ttf
```

and the same for `Literata-SemiBold.ttf` and `Literata-Italic.ttf`. The
`--name-IDs` list keeps the licence description and URL (name records 13 and
14) beside the copyright notice, so every copy compiled into a binary carries
its licence as the Open Font License asks; the subsetter drops them by
default. Keeping them changes no glyph or metric table.

Characters outside the subset fall back to the bundled monospace face, as they
do for Atkinson Hyperlegible.

# Flashcards device notice

The Flashcards Kobo application consumes only Cobalt-owned `CBFLASH` bundles
created and verified on a host computer. The device binary contains no linked
third-party study-engine code, performs no collection migration, and requests
no remote-network capability. Like every Cobalt app, it uses the runtime's
local Unix-domain IPC transport; that transport is not internet access.
Review-facing card content is already reduced to bounded text, semantic
emphasis, and validated media before the bundle reaches the reader. Latin
interface text uses Cobalt's Atkinson Hyperlegible stack. Japanese card and
SVG text uses the bounded **Cobalt Japanese** subset documented in
`SOURCE-Cobalt-Japanese-font.md`; text outside those shipped glyph sets is
rejected instead of becoming empty boxes. The bundle may also retain original
collection/template metadata for host reconciliation; the device treats that
metadata as inert data and never executes or resolves it.

The device links resvg/usvg only to verify that SVG sources referenced by the
bounded due queue reproduce their digest-addressed PNG bytes. It displays the
PNG rather than executing or resolving SVG content. Applicable resvg,
Atkinson, DejaVu, Cobalt Japanese, and Rust dependency terms and source pins
are included in the adjacent device licence documents and exposed by the
application's **Licences & about** screen.

Flashcards and the `CBFLASH` format are Cobalt components licensed under
AGPL-3.0-only.

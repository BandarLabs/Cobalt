# Flashcards device notice

The Flashcards Kobo application consumes only Cobalt-owned `CBFLASH` bundles
created and verified on a host computer. The device binary contains no linked
third-party study-engine code, performs no collection migration, and requests
no remote-network capability. Like every Cobalt app, it uses the runtime's
local Unix-domain IPC transport; that transport is not internet access.
Review-facing card content is already reduced to bounded text and validated
media before the bundle reaches the reader. The bundle may also retain
original collection/template metadata for host reconciliation; the device
treats that metadata as inert data and never executes or resolves it.

The device links resvg/usvg only to verify that SVG sources referenced by the
bounded due queue reproduce their digest-addressed PNG bytes. It displays the
PNG rather than executing or resolving SVG content. Applicable resvg, font,
and Rust dependency terms are included in the adjacent device licence
documents and exposed by the application's Notices screen.

Flashcards and the `CBFLASH` format are Cobalt components licensed under
AGPL-3.0-only.

# Needles

Needles is an unofficial companion for your Ravelry library. It reads only
your account through Ravelry's official API: Library, Queue, and Favorites.
Install the HTTP Basic credential under its exact runtime name with
`kobo secret set ravelry --device <address>`. The secret is named in runtime
tasks, constrained to those official read-only endpoints, and never available
to the application or its logs.

Each section has its own durable row and repeat counter. The large `+1 row`
control autosaves on every tap; `Undo -1 row` reverses the most recent count
without underflowing. Following a project keeps the reader awake in stand
mode, and all counters, Ravelry metadata, and transferred text remain usable
offline after sleep or reboot.

## Preparing a pattern you own

Needles uses the shared `kobo-bookview`/`kobo-doc` reading pipeline for
reflowable Markdown and plain text. On the host, `kobo needles push` calls
Poppler's `pdftotext` (a separately installed, GPL-licensed tool) for a
user-owned PDF, rejects unsafe/oversized/unenriched input, and atomically
places the bounded Markdown result in Needles' private shelf:

```sh
kobo needles prepare PATTERN.pdf --out PATTERN.md
kobo needles push PATTERN.pdf --device <address>
# or, after review/editing:
kobo needles push PATTERN.md --device <address>
```

This is intentionally text-first. A scanned, chart-only, encrypted, or
malformed PDF is refused with an explanation rather than being claimed to be
readable. Chart/SVG/raster conversion is not available in v1. The host-side
ownership and atomic-transfer shape follows Music Stand's score-transfer
pipeline; MuPDF remains credited there as its AGPL-3.0 chart renderer, but
Needles does not bundle or invoke it.

Ravelry project-note postback is deliberately unavailable in v1: the exact
write field and endpoint are not verified here, so Needles never pretends to
have updated ravelry.com. Use Ravelry on the web to edit project notes.

“Ravelry” is used nominatively. This app is not affiliated with Ravelry.
Respect Ravelry's API terms and attribution requirements; Needles only reads
metadata from the signed-in owner's account and does not redistribute patterns.

```sh
cargo test -p kobo-needles
kobo run --sim --app needles
```

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
reflowable Markdown and plain text. On the host, `kobo needles` turns a
pattern you own into that Markdown and puts it on Needles' private shelf:

```sh
kobo needles preview PATTERN.pdf                       # outline, charts, first lines
kobo needles prepare PATTERN.pdf --out PATTERN.md      # keep the Markdown for review
kobo needles push PATTERN.pdf --device <address>       # straight to the reader
kobo needles push PATTERN.md --sim                     # or into the simulator
```

PDFs go through Poppler's `pdftotext` (a separately installed, GPL-licensed
tool); `kobo needles setup` installs it with the platform package manager. A
scanned, encrypted, or malformed PDF is refused with an explanation, and a
page that holds only a chart or a photo is named in the report, never
silently dropped.

Charts travel as PNGs beside the pattern file: a pattern that refers to
`chart-lace.png` picks up that file on push, and the reader draws it inline.
Chart/SVG conversion out of the PDF itself is not available in v1; export
charts as PNGs and keep them next to the pattern. The host-side ownership
and atomic-transfer shape follows Music Stand's score-transfer pipeline;
MuPDF remains credited there as its AGPL-3.0 chart renderer, but Needles
does not bundle or invoke it.

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

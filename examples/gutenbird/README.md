# Gutenbird

Browse free book libraries and read their books on your Kobo.

<table>
<tr>
<td width="50%" valign="top"><img width="300" src="screenshots/shelf.png" alt="A shelf of covers from a library"><br>A shelf of covers from a library</td>
<td width="50%" valign="top"><img width="300" src="screenshots/book.png" alt="A book, ready to read"><br>A book, ready to read</td>
</tr>
</table>

## Features

- Works with Project Gutenberg, Standard Ebooks, Open Library and any other
  OPDS catalog you add by its address.
- Search and browse catalogs, with covers.
- Downloads EPUB when available, with progress on screen, and falls back to
  plain text. **Choose download** picks the format when a book offers several.
- Books open in the shared reader, with text size, front light, bookmarks and
  highlights.
- OPDS 1.2 and 2.0 catalogs look and behave the same.

See [docs/OPDS.md](../../docs/OPDS.md) for how the OPDS client handles real
catalogs.

## Permissions

- `network`: reads catalogs and downloads books.
- `frontlight-control`: adjusts the front light while reading.

## Development

```sh
cargo test -p kobo-gutenbird
kobo run --sim --app gutenbird      # in the browser simulator
python3 scripts/check-apps-sim.py gutenbird
kobo deploy --device <ip>       # onto a reader over Wi-Fi
```

---

Part of [Cobalt](../../README.md). See [all apps](../../README.md#apps).

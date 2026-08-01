# Gutenbird

The Project Gutenberg library, on the device.

Search sixty thousand public domain books, and read one without leaving the
application.

| The shelf | A book |
| --- | --- |
| ![Six real Gutenberg covers in a three by two grid, with "1 of 6" beneath](screenshots/shelf.png) | ![A cover beside the title and author, a Read button, and a paged summary](screenshots/book.png) |

*Captured from a Kobo Clara BW over Wi-Fi with `kobo shot --device`. Those are
the real covers, fetched over the reader's own radio and decoded on the
device.*

## Why plain text rather than EPUB

Gutenberg publishes every book in several formats, and this reads the plain
text one. `kobo-doc` can read an EPUB now, so this is no longer a matter of
what can be parsed: it is that an EPUB is only useful whole, and a
half-downloaded zip is not a half-downloaded book. The plain text can be read
from the first byte, which is what lets the first page appear in about a second
on a radio this slow. What is lost is italics and a table of contents.

## Why the book arrives in pieces

The transport carries half a megabyte at most, and a Victorian novel is more
than that. Gutenberg honours `Range`, so the book is asked for in chunks: the
first arrives in about a second and the reader starts reading, and the rest is
topped up a few pages ahead of where they are. A page turn never waits for the
radio unless the reader is genuinely at the end of what has arrived, and the
foot of the page says so when they are.

## Why the reading screen is not built here

Type size, front light, bookmarks and marked passages are not Gutenbird's to
invent. Every
application that shows a book wants the same ones, and a reader who learns them
in one should find them in the next. They live in `kobo-read`.

## Running it

```sh
kobo run --sim --app gutenbird          # in the browser simulator
kobo deploy --device <ip>               # onto a reader over Wi-Fi
```

---

Built with the [Cobalt SDK](../../README.md). The other apps:
[Launcher](../launcher/README.md) ·
[Audiobook Studio](../audiobook/README.md) ·
[Hacker News](../hn/README.md) ·
[RSS Reader](../rss/README.md) ·
[Daily Brief](../brief/README.md) ·
[AI Chat](../chat/README.md) ·
[Coding Agents Sidekick](../sidekick/README.md) ·
[Terminal](../terminal/README.md) ·
[UI Components Showcase](../gallery/README.md) ·
[Settings](../settings/README.md) ·
[Todo](../todo/README.md) ·
[Tic-tac-toe](../tictactoe/README.md) ·
[Magnet Sensor](../magnet/README.md)

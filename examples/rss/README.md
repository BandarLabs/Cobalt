# RSS Reader

The sites you read, on the device.

Type an address, pick the feed it finds, and read the articles without leaving
the application.

| The articles | Finding a feed |
| --- | --- |
| ![A list of articles with glyph leads and clamped titles](screenshots/articles.png) | ![The search screen, with the Feedsearch attribution](screenshots/search.png) |

*Captured from a Kobo Clara BW over Wi-Fi with `kobo shot --device`.*

## Why a search service rather than guessing the address

Almost nobody knows the address of a site's feed. They know the address of the
site. Turning one into the other means fetching the page, parsing its HTML,
reading `<link rel="alternate">`, then trying `/feed`, `/rss.xml`, `/atom.xml`
and a dozen more: several round trips over a radio that costs battery, and an
HTML parser aimed at whole pages rather than fragments.

[Feedsearch](https://feedsearch.dev) does that work once, server-side, and has
done it before for most sites anybody types. One request returns every feed a
domain has, already ranked. That is the whole reason this application can be a
few hundred lines rather than a browser.

Their terms ask for a visible attribution wherever their results are shown,
which is why it is on both the search screen and the results screen.

## Why the articles are read from the feed and not from the site

Because the feed is the readable copy. Most publishers put the whole post in
`content:encoded`, and the ones that do not put a summary there. Either way it
is prose with a little markup, which is exactly what an E Ink panel wants.
Following the link instead would mean fetching a modern web page: a megabyte of
layout, script and advertising wrapped around the same words.

## Running it

```sh
kobo run --sim --app rss                # in the browser simulator
kobo deploy --device <ip>               # onto a reader over Wi-Fi
```

---

Built with the [Cobalt SDK](../../README.md), which
[installs on a Kobo](../../README.md#install-it-on-your-kobo) with one
command over USB. The other apps:
[Launcher](../launcher/README.md) ·
[Audiobook Studio](../audiobook/README.md) ·
[Gutenbird](../gutenbird/README.md) ·
[Hacker News](../hn/README.md) ·
[Daily Brief](../brief/README.md) ·
[AI Chat](../chat/README.md) ·
[Coding Agents Sidekick](../sidekick/README.md) ·
[Terminal](../terminal/README.md) ·
[UI Components Showcase](../gallery/README.md) ·
[Settings](../settings/README.md) ·
[Todo](../todo/README.md) ·
[Tic-tac-toe](../tictactoe/README.md) ·
[Magnet Sensor](../magnet/README.md)

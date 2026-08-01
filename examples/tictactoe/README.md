# Tic-tac-toe

Two players, one panel, three in a row.

This exists to prove a point about the SDK as much as to be a game: it is
written entirely against the public builders, and the board is not a board
primitive. It is a `grid`, which is the same thing a keypad or an on-screen
keyboard is. If a game needs the framework to grow a new node type, the
framework is not general enough yet.

![A finished game, with O winning down the third column](screenshots/game.png)

*Captured from a Kobo Clara BW over Wi-Fi with `kobo shot --device`.*

## The rules

The ones people actually play at a table: whoever is holding the device taps,
and the mark alternates. Nought goes first.

## Why it is the floor

Tic-tac-toe gets nothing. When the SDK's vocabulary was expanded from a handful
of nodes to nearly thirty, this application was deliberately left untouched: if
a proposed component turns out to be needed *here*, in a game that is a
three-by-three grid and a line of text, then the component is wrong and the
primitive it should have been built from is missing.

That makes it a useful canary. A change to the layout engine that this cannot
survive is a change that has broken something fundamental.

## Running it

```sh
kobo run --sim --app tictactoe          # in the browser simulator
kobo deploy --device <ip>               # onto a reader over Wi-Fi
```

---

Built with the [Cobalt SDK](../../README.md). The other apps:
[Launcher](../launcher/README.md) ·
[Audiobook Studio](../audiobook/README.md) ·
[Gutenbird](../gutenbird/README.md) ·
[Hacker News](../hn/README.md) ·
[RSS Reader](../rss/README.md) ·
[Daily Brief](../brief/README.md) ·
[AI Chat](../chat/README.md) ·
[Coding Agents Sidekick](../sidekick/README.md) ·
[Terminal](../terminal/README.md) ·
[UI Components Showcase](../gallery/README.md) ·
[Settings](../settings/README.md) ·
[Todo](../todo/README.md) ·
[Magnet Sensor](../magnet/README.md)

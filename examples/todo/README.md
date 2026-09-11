# Todo

A list of things to do, which is where a platform's state model shows.

It exercises the four things an application on this device actually has to get
right.

| The list | Adding an item |
| --- | --- |
| ![The list, with two items and the Add button](screenshots/list.png) | ![The keyboard, raised for exactly one field](screenshots/compose.png) |

*Captured from a Kobo Clara BW over Wi-Fi with `kobo shot --device`.*

## What it demonstrates

- **State that outlives the process.** The list is written through
  `kobo_sdk::AppStore`, so closing the application and opening it again shows
  the same list. Nothing here knows where that is stored, and there is no path
  it could name.
- **Actions that change one thing.** Tapping a row completes it. Only that row
  changes, so the runtime repaints that row rather than the screen, which on
  this panel is the difference between a flicker and a flash.
- **A state, drawn as the renderer sees fit.** A finished item is struck
  through and muted. The application never asks for a line through text; it
  says the item is done and the renderer decides what that looks like.
- **Typing, only where it is unavoidable.** Adding an item needs words, so the
  keyboard is raised for exactly that and put away again afterwards.

## What it does

Write something down, tick it off, and clear the finished ones away when they
get in the way. Clearing can be undone once, until the next change: the
alternative is asking "are you sure" before a button that is pressed once a
week, which trains everybody to answer without reading it.

Anything with a `#tag` in it can be narrowed to with the chips above the list.
The tags are the ones the owner typed; nothing here invents a category.

Changing the list is a different job from working through it, so it has its own
screen. Edit opens the list as it is kept rather than grouped by what is
finished, and tapping an item there opens that item: rename it, move it, give
it a date, or take it off. A date is said against today ("due tomorrow", "3
days late") because a list of things to do is read against today and nothing
else.

Save a copy offers the whole list to a paired computer as plain text, through
the same export the rest of the system uses. The original stays on the reader.

## Why the list is saved on every change

There is no save button and no "are you sure". E Ink devices are closed by
shutting a cover and are forgotten until the battery is flat, so any design
that relies on a clean exit loses data. Each write is atomic, so the worst a
power loss can cost is the change that was in flight.

## Running it

```sh
kobo run --sim --app todo               # in the browser simulator
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
[RSS Reader](../rss/README.md) ·
[Daily Brief](../brief/README.md) ·
[AI Chat](../chat/README.md) ·
[Coding Agents Sidekick](../sidekick/README.md) ·
[Terminal](../terminal/README.md) ·
[UI Components Showcase](../gallery/README.md) ·
[Settings](../settings/README.md) ·
[Tic-tac-toe](../tictactoe/README.md) ·
[Magnet Sensor](../magnet/README.md)

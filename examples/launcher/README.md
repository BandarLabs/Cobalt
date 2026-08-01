# Launcher

The framework launcher.

This is deliberately an ordinary application written against `kobo-sdk`. It
gets no privileged drawing path, no private widgets and no hardware access the
counter example could not also ask for. The only thing that will eventually
distinguish it is a permission to enumerate and start other applications. If
the launcher cannot be expressed with the public SDK, the SDK is not good
enough yet, so keeping it honest here is the point.

| Page one | Page two |
| --- | --- |
| ![Nine application tiles in a three by three grid, headed "Cobalt 1 of 2", over a bar offering "Return to Kobo reader" and "More apps"](screenshots/home.png) | ![The remaining three tiles, over a bar that now offers "Previous" and "Return to Kobo reader" and no longer offers more](screenshots/more-apps.png) |

*Captured from a Kobo Clara BW over Wi-Fi with `kobo shot --device`.*

## Why "Return to Kobo reader" is the largest thing on the screen

Returning to the stock reader is a first-class, always-visible destination
rather than something hidden in a menu. The reader is not an application and
cannot be one: it owns the framebuffer, input, power and Wi-Fi while it runs,
and its lifecycle belongs to vendor init. Showing it again means ending this
session and restarting it.

Making that the most obvious control on the screen also makes it the most
exercised path in the system, which is exactly where the reliability is wanted.

## Why the applications are paged rather than scrolled

Nothing scrolls on this panel. A grid that runs off the bottom loses its last
row without a word, so the tiles are paginated against the room actually
measured for them, and the page position under the grid is what tells a page
turn from a tap that did nothing.

A direction is only offered when there is a page on that side of this one, so
"Previous" and "More apps" never name the same destination and the last page
never promises applications that are not there.

## Running it

```sh
kobo run --sim --app launcher           # in the browser simulator
kobo deploy --device <ip>               # onto a reader over Wi-Fi
```

On a reader, `/mnt/onboard/.adds/cobalt/start.sh` starts the session and this
is the first screen it shows.

---

Built with the [Cobalt SDK](../../README.md). The other apps:
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
[Tic-tac-toe](../tictactoe/README.md) ·
[Magnet Sensor](../magnet/README.md)

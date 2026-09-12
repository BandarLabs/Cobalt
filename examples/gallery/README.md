# UI Components Showcase

Every UI primitive, on one device, so each can be checked by eye.

This is a test instrument as much as a demonstration. If a primitive looks
wrong here it looks wrong everywhere, and the layout tests only prove that
sizes are right, not that the result is worth reading.

| Type and structure | A standard state |
| --- | --- |
| ![Headings, body, secondary text, section rules and a facts block](screenshots/text.png) | ![A centred empty state with a recovery button beneath it](screenshots/controls.png) |

*Captured from a Kobo Clara BW over Wi-Fi with `kobo shot --device`.*

The **Panel** tab is the Folio acceptance surface: masthead, featured card tiles
(including caption and live-value variants), a trailing section link, page
rail, active nav notch, and a deliberately alternating ghosting comparison.
Its ordinary action is a centered content-width button; only primary actions
expand to the full content measure.
It is captured by `scripts/shoot-apps.sh` with the normal gallery pass.

## Pages that fit the device in your hand

No page here decides in advance how many panels it takes. Each one is a list of
parts, and the parts are dealt out into panels by measurement: parts are added
until the runtime's diagnostics say something has been pushed off the panel,
and then a new panel starts, reachable with the toolkit's own page turns. The
same reference is thirty-one panels on a Clara BW at the default text size and
thirty-seven at 170%, and `every_page_fits_every_supported_panel_at_every_text_size`
lays every screen out on all eight supported panels at all nine text sizes.

## One job, from end to end

The App Store card on the Panel tab starts the worked example: choose a book,
confirm it, watch it arrive, be told it is there, open it, and come back out
again a step at a time. Every step is drawn with the controls the other pages
show. A reference made only of pages of controls teaches how each one is drawn
and nothing about how they follow one another.

## The two warnings this application provokes on purpose

The tone budget, because a shelf of tiles is a surface, the marked navigation
bar under it is inverted, and with a rule, a subtitle and a word of prose that
is all five inks on one panel. That is what a launcher home screen costs, and
it is worth seeing. And two primary actions on the buttons page, which is the
same dominant verb drawn twice with one of them refused. Any other warning
fails `every_warning_this_reference_provokes_is_one_it_means_to`.

## Why this is the conformance screen

Every node in `kobo-ui` has an instance here. Adding a node to the vocabulary
without adding it to this gallery is how a primitive ends up shipped and never
looked at: it passes its own unit test, it never appears on a panel, and the
first person to reach for it finds out that it is nine pixels too tall.

The conformance test measures every page against the status band the runtime
draws above it. Without that band the content starts sixty pixels higher than
it does on the device, and the slack is enough to hide a page that overflows.

The gallery is also the fastest way to see a rendering change. `kobo drive`
taps through its tabs and captures each one, so a change to the layout engine
can be diffed as pictures rather than as numbers.

## Running it

```sh
kobo run --sim --app gallery            # in the browser simulator
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
[Settings](../settings/README.md) ·
[Todo](../todo/README.md) ·
[Tic-tac-toe](../tictactoe/README.md) ·
[Magnet Sensor](../magnet/README.md)

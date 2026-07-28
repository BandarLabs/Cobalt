# Components

Every UI primitive, on one device, so each can be checked by eye.

This is a test instrument as much as a demonstration. If a primitive looks
wrong here it looks wrong everywhere, and the layout tests only prove that
sizes are right, not that the result is worth reading.

| Type and structure | A standard state |
| --- | --- |
| ![Headings, body, secondary text, section rules and a facts block](screenshots/text.png) | ![A centred empty state with a recovery button beneath it](screenshots/controls.png) |

*Captured from a Kobo Clara BW over Wi-Fi with `kobo shot --device`.*

## Why this is the conformance screen

Every node in `kobo-ui` has an instance here. Adding a node to the vocabulary
without adding it to this gallery is how a primitive ends up shipped and never
looked at: it passes its own unit test, it never appears on a panel, and the
first person to reach for it finds out that it is nine pixels too tall.

The conformance test measures every page against the status band the runtime
draws above it. Without that band the content starts sixty pixels higher than
it does on the device, and the slack is enough to hide a page that overflows.
Two pages here were doing exactly that.

The gallery is also the fastest way to see a rendering change. `kobo drive`
taps through its tabs and captures each one, so a change to the layout engine
can be diffed as pictures rather than as numbers.

## What it found

The sub-tab strip used to drop every tab past a fixed maximum without saying
so, which meant the gallery itself was hiding some of the components it exists
to show. The empty states were drawn at the top of a thousand pixels of white.
That is what made `Splash` worth having as a node rather than a heading and a
paragraph: it centres itself in the band it is given, and stops short of
whatever follows it.

## Running it

```sh
kobo run --sim --app gallery            # in the browser simulator
kobo deploy --device <ip>               # onto a reader over Wi-Fi
```

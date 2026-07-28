# Terminal

A shell on the panel.

This replaced the hello example, which showed that a button could be tapped and
nothing else. A terminal is the opposite: it is the hardest thing this platform
can host, and every part of it is a claim that has to be true.

![A real /bin/sh listing the Kobo root filesystem in three columns, above a
keyboard with esc, tab, ctrl and arrow keys](screenshots/shell.png)

*Captured from a Kobo Clara BW over Wi-Fi with `kobo shot --device`. That is
the reader's own root filesystem, listed by a real `/bin/sh`.*

## What it demonstrates

- **A capability the application does not hold.** There is no pseudo-terminal
  here, no fork, no file descriptor and no path to a program. The application
  says what was typed and the runtime decides whether there is a shell at all.
  An application without `kobo_sdk::Capability::Shell` running this same code
  is refused and shown saying so.
- **A grid measured rather than assumed.** `kobo_sdk::terminal_grid_for` lays
  this screen out with an empty terminal and measures what is left, so the
  shell wraps its lines exactly where the reader sees them wrap.
- **Keys that send rather than collect.** The text keyboard gathers a string
  and hands it over on submit, which is right for a search box and useless for
  a shell: `Ctrl-C` has to arrive while the program is still running.
- **Background life.** Leaving the terminal does not end it. A build started
  here keeps running while the reader is elsewhere, and coming back shows what
  it printed.

## Why the specials row is the same size as the letters

An `esc` that is half the height of a `q` is a key that gets missed, and on a
panel with no haptics a missed key is indistinguishable from a key that did
nothing. Every cap in every row is one touch target.

## Running it

```sh
kobo run --sim --app terminal           # in the browser simulator
kobo deploy --device <ip>               # onto a reader over Wi-Fi
```

The simulator runs the same host the daemon runs and starts a real `/bin/sh`,
so the loop can be exercised without a reader present.

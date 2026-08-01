# Daily brief

A brief that is ready before you open it.

This exists to demonstrate the one lifecycle e-readers actually need, and the
one a mobile framework would call backgrounding. It is not a feed reader with
extra steps: the whole point is what happens when you *leave*.

![The brief: stories and sources side by side, then the top stories
numbered](screenshots/brief.png)

*Captured from a Kobo Clara BW over Wi-Fi with `kobo shot --device`.*

## What it demonstrates

Tap Refresh and it starts fetching. Go back to the launcher and open something
else. The fetch keeps running, because leaving an application no longer stops
it: the runtime keeps the process, the work in flight and the memory, and tells
the application it is no longer being looked at. Come back and the brief is
finished and drawn, with no reload and no second fetch.

## Why it saves the moment it goes to the background

`KoboApp::on_background` is the last certain moment. A reader closes an
e-reader by shutting a cover and may not open it for a week, and the device may
run its battery flat in between. So the brief is written then, and on every
arrival, rather than on the way out.

## Why the two counts sit side by side

How many headlines, and how many places they came from. A brief drawn from one
site is a different thing from one drawn from six, and that was invisible when
the sites were only a line under each title. Given a full line each they read
as a list of findings rather than the one-line summary they are, so they go in
a `band`. That is the SDK's two-or-three column escape from the downward flow,
and it stacks itself back up if the panel is ever too narrow to give both slots
a readable width.

## Running it

```sh
kobo run --sim --app brief              # in the browser simulator
kobo deploy --device <ip>               # onto a reader over Wi-Fi
```

---

Built with the [Cobalt SDK](../../README.md). The other apps:
[Launcher](../launcher/README.md) ·
[Audiobook Studio](../audiobook/README.md) ·
[Gutenbird](../gutenbird/README.md) ·
[Hacker News](../hn/README.md) ·
[RSS Reader](../rss/README.md) ·
[AI Chat](../chat/README.md) ·
[Coding Agents Sidekick](../sidekick/README.md) ·
[Terminal](../terminal/README.md) ·
[UI Components Showcase](../gallery/README.md) ·
[Settings](../settings/README.md) ·
[Todo](../todo/README.md) ·
[Tic-tac-toe](../tictactoe/README.md) ·
[Magnet Sensor](../magnet/README.md)

# Deck

A text-only command deck for the existing `kobo-sidekickd` pairing. It polls
`/deck`, posts named key presses to `/deck/press`, and keeps the last good grid
visible when the computer is off the air.

<img width="300" src="screenshots/deck.png" alt="Deck on a Clara BW showing its first-run pairing screen">

## Security

Deck turns the reader into a remote command runner for the paired computer. The
daemon must use Sidekick TLS and require its six-character pairing code on every
request. Commands remain exclusively in `~/.config/kobo/sidekick/deck.toml`,
which the reader never writes. Use a confirmed key for externally visible or
destructive commands. The expected daemon protocol caps retained output at 2 KB.

The client is prepared for the existing Sidekick listener on port 9331. This
app-only change cannot add the required daemon routes; until those routes ship,
it reports the daemon as off the air rather than executing anything locally.

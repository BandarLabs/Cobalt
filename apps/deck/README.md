# Deck

A text-only command deck for the existing `kobo-sidekickd` pairing. It polls
`/deck`, posts named key presses to `/deck/press`, and keeps the last good grid
visible when the computer is off the air.

<img width="300" src="screenshots/deck.png" alt="Deck on a Clara BW showing six command controls in a two-column grid">

## Set up

Initialize Sidekick once, then create `~/.config/kobo/sidekick/deck.toml`:

```toml
[[page]]
name = "Build"

[[page.key]]
label = "Test"
detail = "cargo test"
run = "cd ~/src/project && cargo test"
confirm = false

[[page.key]]
label = "Deploy"
run = "~/bin/deploy-staging.sh"
confirm = true
```

Run `kobo-sidekickd run`, open Deck on the reader, and enter the same address
and six-character pairing code used by Sidekick. Editing the file refreshes the
open grid; a malformed edit leaves the last working grid in place and shows the
problem.

## Security

Deck turns the reader into a remote command runner for the paired computer.
Every request uses Sidekick TLS and its pairing code. Commands come only from
`deck.toml`, which the reader cannot write, and run as the computer user from
their home directory. Mark externally visible or destructive commands with
`confirm = true`. Only four commands may run at once, each is stopped after ten
minutes, and only the final 2 KB of cleaned output is retained.

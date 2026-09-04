# Deck

A text-only command deck for the existing `kobo-sidekickd` pairing. It polls
`/deck`, posts named key presses to `/deck/press`, and keeps the last good grid
visible when the computer is off the air.

<img width="300" src="screenshots/deck.png" alt="Deck on a Clara BW showing a 3 by 5 grid of square command pads">

## Set up

Initialize Sidekick once, then assign pads with the host CLI. Those commands
write `~/.config/kobo/sidekick/deck.toml` and can push the same layout into the
simulator or reader store so Deck opens on the grid, not the pairing splash:

```sh
kobo deck init
kobo deck set 1 --launch todo
kobo deck set 2 --url https://example.com
kobo deck set 3 --label Test --detail "cargo test" --run "cd ~/src/project && cargo test"
kobo deck ls
kobo deck push --sim          # or: kobo deck push --device IP
```

`--launch APP` becomes a platform open command (`open -a` on macOS, `gtk-launch`
elsewhere). `--url` becomes `open` / `xdg-open`. `--run` is a raw shell command
Sidekick executes from the owner's home directory.

The same file can still be edited by hand:

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
and six-character pairing code used by Sidekick. A layout pushed with
`kobo deck push` skips that splash and shows the assigned pads immediately.
Editing the file refreshes a live paired grid; a malformed edit leaves the last
working grid in place and shows the problem.

## Security

Deck turns the reader into a remote command runner for the paired computer.
Every request uses Sidekick TLS and its pairing code. Commands come only from
`deck.toml`, which the reader cannot write, and run as the computer user from
their home directory. Mark externally visible or destructive commands with
`confirm = true`. Only four commands may run at once, each is stopped after ten
minutes, and only the final 2 KB of cleaned output is retained.

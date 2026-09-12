# Deck

A text-only command deck for the existing `kobo-sidekickd` pairing. It polls
`/deck`, posts named key presses to `/deck/press`, and keeps the last good grid
visible when the computer is off the air.

<img width="300" src="screenshots/deck.png" alt="Deck on a Clara BW showing a 3 by 5 grid of square command pads">

## What the panel says

The bar carries the name of the page showing, because a deck with a Build page
and a Home page is two decks and the reader has to know which one is under
their thumb. Under the tabs, one line says which computer this is talking to
and whether it answered the last thing it was asked: a key that does nothing
because the computer is asleep looks exactly like a key that does nothing
because it has not been assigned.

Fifteen places are drawn, three rows of five, because that is the shape of the
panel. The ones nobody has assigned are drawn in a hairline rather than the
bezel a key gets, so a deck with three things on it does not read as twelve
controls that do nothing.

A key that has run opens what it said, whether it worked or not, and the line
under the deck says what the last one did.

## Set up

Start from a preset, or assign pads one at a time with the host CLI:

```sh
kobo deck init --preset build   # test, format, lint, build, status, pull, deploy
kobo deck init --preset home    # music, lights, a timer, lock the screen
```

Either way the pads are yours to change. Those commands
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


## Preview and pairing

`kobo deck push --sim` stages a static preview when the simulator is unpaired.
Preview pads do not execute commands. Choose **Pair** to enter your computer's
address and pairing code. If the simulator already has a pairing, a layout
push preserves it.

`kobo deck push --device ADDRESS` updates the cached layout only and preserves
reader pairing. An unpaired reader still needs to pair with the computer;
transferring a layout does not establish an executable connection.

![Static Deck preview](../../docs/quality/evidence/deck-preview/default/preview.png)
![Preview pad feedback at larger text size](../../docs/quality/evidence/deck-preview/170/preview-tapped.png)


Each page supports 1–15 pads. The CLI and computer helper both reject pages
outside that range, including hand-edited configuration. Use another page
for additional actions, up to six pages.

![All 15 pads at larger text size](../../docs/quality/evidence/deck-fifteen-pads/preview.png)

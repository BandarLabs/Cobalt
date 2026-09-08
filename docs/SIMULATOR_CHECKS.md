# Repeatable catalog simulator checks

From the repository root, run:

```sh
python3 scripts/check-apps-sim.py --out target/sim-check
```

The runner builds the CLI, launches every catalog app with a fresh temporary
store, and runs each app's committed drive route when one exists. It fixes the
Inkling date and supplies local fixtures for apps that need them. A failed
launch or route returns a nonzero exit code. The runner terminates its own
simulator processes and removes temporary stores, including after interruption.

To check selected apps:

```sh
python3 scripts/check-apps-sim.py inkling pubquiz --out target/sim-check-games
```

`results.json` records the source commit, whether the checkout was dirty, and
each app's launch and route result. The output directory also contains logs
and route artifacts. CI runs the complete sweep and uploads the evidence.

The current catalog contains 44 apps and 36 committed routes. Audiobook,
Brief, Chat, Gutenbird, HN, Magnet, RSS, and Zotero Reader have launch coverage
only. A successful sweep verifies the committed simulator scenarios; it does
not certify every app feature, external account integration, or physical Kobo
behavior.

The runner requires Python 3.9 or newer, Node.js, and the repository's Rust
toolchain. `CARGO_TARGET_DIR` is supported for sharing an existing build cache.


## Live Paperterm session

After building `kobo-cli`, run:

```sh
python3 scripts/quality/check-paperterm-live.py --scale default --output /tmp/paperterm-live
```

This launches the actual SDK app and a real host PTY with an original synthetic
command. It types from the laptop and reader into the same session, checks
wide output, keyboard resizing, Ctrl-C and laptop terminal restoration, and
captures the portrait UI. It seeds pairing in a temporary store; this does not
validate manual onboarding. Use `--profile` and `--scale` for other displays.

The fixture sets `KOBO_STREAM_CONFIG_DIR` and `KOBO_SIM_TRUST_DIR` to private
temporary directories. The latter replaces the simulator's default owner-root
directory (`~/.config/kobo/trust`); ordinary certificate verification remains
active. No owner identity or trust roots are changed. Live long polls remain
outstanding by design, so this route waits for visible content instead of
waiting for all tasks to become idle. See the [portrait session](../apps/paperterm/screenshots/terminal.png).


Add `--pair-on-reader` to enter the private address and code through the on-screen
keyboard instead of seeding the pairing record. This verifies the reader-side
form and its successful save; the fixture still installs trust locally, so it
does not claim hardware trust transfer. The live route also checks uncertain
input, explicit resume, offline reconnect with the keyboard open and closed,
and restored two-way input. The committed `apps/paperterm/drive.kobo` route
separately covers the welcome, offline preview, setup pages and corrected form
errors, asserting zero fetch/post effects throughout.

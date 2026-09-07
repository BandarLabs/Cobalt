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

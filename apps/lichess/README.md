# Lichess

An unofficial Lichess client for offline puzzle sessions and polling
correspondence chess. Lichess supports third-party clients through its public
API. Install a Personal Access Token with `kobo secret set lichess` for
personalized puzzles and play; the application only names that secret in
runtime tasks and never reads it.

Without a key, a 1500-level puzzle session still works. After its batch is
downloaded, solving is offline and checked by `shakmaty`. These solves and the
device's streak are local: the public Lichess API has no puzzle-solve
submission endpoint, so they do not update lichess.org ratings.

`shakmaty` 0.27.3 is GPL-3.0-or-later, compatible with Cobalt's AGPL-3.0-only
distribution.

Pieces use compact `wK`/`bQ` labels. This deliberate fallback remained clearer
than chess Unicode glyphs in the Clara BW simulator.

Real-time games are not included: Board API move streams need a long-lived
transport; this v1 polls correspondence and slow games instead.

## Run

```sh
cargo test -p kobo-lichess
kobo run --sim --app lichess
```

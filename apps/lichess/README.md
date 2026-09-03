# Lichess

An unofficial, touch-first Lichess Board API client for Cobalt. It supports
responsive Folio time-control presets, incoming standard-clock challenges,
live games, two-tap legal move entry, vector pieces, promotion, clocks,
reconnect, draw actions, resign, conservative abort, and opponent-gone
victory claims.

| Responsive presets | Live board | Pairing |
| --- | --- | --- |
| ![Responsive time controls and puzzles](screenshots/home.png) | ![Live Lichess board](screenshots/game.png) | ![Rated pairing](screenshots/pairing.png) |

## Requirements

- Cobalt **0.3.5 or newer**, speaking protocol **12** for responsive Folio
  card tiles and layout metrics.
- A Lichess Personal Access Token with the `board:play` scope.
- Install the token under the exact secret name `lichess`:

  ```sh
  kobo secret set lichess --from <token-file> --device <address>
  ```

The application sends only the secret name. Cobalt resolves the value inside
the runtime and binds it to the exact HTTPS `lichess.org` Board API routes used
by the app. The token is never returned to the process, shown in UI, written to
state, or included in logs. Redirects carrying the token are denied, and POST
requests are never replayed.

Only the official Lichess origin is supported. This release deliberately does
not accept a custom server URL because Cobalt has no owner-scoped,
app-specific origin policy that could broaden the destination without also
broadening credential authority.

## Supported

- Account validation and live detection of CLI-installed token changes
- Rated random-color 10+0, 10+5, 15+10, 30+0, and 30+20 seeks
- One selected seek at a time, opened only after the event stream and
  current-game snapshot are ready
- A uniquely matching new game opens immediately
- Ended seeks reconcile once and are never replayed automatically
- `gameStart`, `gameFinish`, and incoming challenge events
- Board stream reconstruction from server-acknowledged UCI moves
- White/black vector pieces, player clocks above and below the board, and a
  portrait board that follows the player's side
- Two-tap source/destination moves with a brief invalid-square mark
- Castling, en passant, and four promotion choices
- Last move, check, result, turn, server clocks, and opponent-gone countdown
- Move, resign, abort during Lichess's first-two-ply window, draw offer/accept/decline, and
  claim-victory requests
- Restart/reconnect using only game ID, color, opponent label, rated flag, and
  a bounded server retry deadline
- Anonymous offline puzzle batches; local solves do not affect Lichess rating

## Deliberate boundaries

- No chat is requested, rendered, or persisted.
- Takeback controls are not offered. A takeback made elsewhere causes an
  authoritative stream reopen instead of guessing at local history.
- Only standard chess clock challenges are accepted.
- Abort is shown only before both players have moved; Lichess still makes the
  final API decision.
- Draw acceptance and decline remain pending until the authoritative board
  stream reports the resulting state.
- No custom Lichess-compatible base URL is accepted.

## Validate

```sh
cargo test -p kobo-lichess
cargo test -p kobo-net --test lichess_stream_mock -- --test-threads=1
cargo run --locked -p kobo-cli -- app-check --registry apps/catalog.json \
  --package kobo-lichess
```

The HTTPS mock uses a generated test-only CA/key pair under
`crates/kobo-net/tests/fixtures`; it carries no owner credential and never
contacts Lichess.

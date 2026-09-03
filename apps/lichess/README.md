# Lichess

An unofficial, touch-first Lichess Board API client for Cobalt. It supports
responsive Folio time-control presets, incoming standard-clock challenges,
live games, legal move entry, promotion, clocks, reconnect, draw actions,
resign, conservative abort, and opponent-gone victory claims.

| Responsive presets | Live board | Correct seek lifecycle |
| --- | --- | --- |
| ![The responsive Folio home showing rated time-control and puzzle tiles](screenshots/home.png) | ![A black-oriented live Lichess board with two clocks, last move, and draw control](screenshots/game.png) | ![A rated pairing screen explaining the event-stream and reconciliation lifecycle](screenshots/pairing.png) |

Account-global starts are confirmed explicitly before opening:
![A confirmation screen for a selected-preset game recovered from the global account state](screenshots/candidate.png)

An ended seek enters an explicit current-game reconciliation state:
![The selected preset waiting while current games are checked after the seek transport ended](screenshots/reconciling.png)

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

- Account validation and clear missing/invalid/expired-token guidance
- Rated, random-color 3+0, 3+2, 5+0, 5+3, 10+0, 10+5, 15+10, 30+0, and
  30+20 standard seeks; bullet presets are intentionally omitted for e-ink
- One selected seek at a time, opened only after the event stream and
  current-game snapshot are ready
- Account-global game starts require on-device confirmation before the seek is
  closed; an ended seek is reconciled against a fresh current-game snapshot
  and is never replayed automatically
- `gameStart`, `gameFinish`, and incoming challenge events
- Board stream reconstruction from server-acknowledged UCI moves
- White/black orientation, castling, en passant, and four promotion choices
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

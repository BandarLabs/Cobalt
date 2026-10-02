# Birds repeated Refresh recovery

The matched 140% frames show a valid collage update interrupted only by repeated
Refresh actions. Before: Birds reports that the collage is missing, although its
valid PNG chunks are still arriving. After: the same update completes and the
false error disappears. The retained previous collage stays visible throughout.

## Reproduction and fix

Before production source is beta `97153048`. The fixture is the repository's
`scripts/fixtures/birds/current.png`, decoded through the actual app and installed
into the renderer's PictureCache through its actual PutPicture commands.
The callback driver answers shelf requests strictly in FIFO order:

1. Load a complete snapshot and PNG, then start Refresh
2. After image chunk 0, tap Refresh again; its snapshot read queues behind chunk 1
3. Chunk 1 queues chunk 2 behind that snapshot read
4. Originally, the new snapshot reply replaced the image buffer before chunk 2
   arrived. Its valid offset was rejected, producing the false missing error

`reload` now coalesces Refresh while either snapshot or image transfer exists.
It does not discard bytes, restart offsets, or change stored data. Actual refused,
checksum-failed, and corrupt updates keep the prior complete image/metadata;
cleared transfer state still allows an explicit retry. Existing automatic poll
and wake safeguards remain intact.

This leaves PR #224's menu presentation work and PR #237's runtime Back routing
alone. The manifest version is 0.1.5, after the 0.1.4 proposed by PR #224.

## Capture provenance and limits

These are **genuine app-renderer and SDK-callback captures, not live simulator or
physical-device screenshots**. Metrics are Clara BW 1072 × 1448 at 300 PPI and
140% text. Captures were also inspected/generated at 100% and 170%. Fonts come
from `kobo_text::install`; runtime header/Back composition uses a synthetic
measuring status, which the reading chrome suppresses here. No image editing,
network requests, user data or live companion service is involved.

The local host refused AF_UNIX socket creation, including a reviewed elevated
launch. The independent baseline CI Birds menu was also inspected from run
36893171875, clean source `b9f6f20efa8012724d2865d15216086a34c24c51`, but that
simple catalog route does not exercise the repeated-refresh race.

## Validation

11 ordinary app tests pass, including three new regressions using the actual
multi-chunk PNG. They cover coalesced repeated Refresh during both transfer
phases, request-order correctness, no duplicate store commands, exact image
bytes, actual read refusal and checksum-failure preservation, successful retry,
and app-callback menu Back versus exit. Runtime Back interception remains the
separate PR #237 issue. Two opt-in capture tests pass.

```sh
COBALT_REVIEW_CAPTURE_DIR=/tmp/birds-before \
  python3 apps/birds/screenshots/ui-review/capture.py --baseline
COBALT_REVIEW_CAPTURE_DIR=/tmp/birds-after \
  python3 apps/birds/screenshots/ui-review/capture.py
cargo test -p kobo-birds --all-targets --all-features --locked
cargo clippy -p kobo-birds --all-targets --all-features -- -D warnings
node --test tools/*.test.mjs
```

All 105 Node tests, format, strict clippy, and the documented contributor dry-run
pass, including static ARM verification and deterministic fixture-only Beta
preview creation. App Cargo.toml and Cargo.lock remain byte-identical to beta;
extra capture dependencies live only in a temporary package with incremental
compilation disabled. The public beta-store-smoke seed (`2a` × 32) was supplied
locally at mode 0600, not committed. Physical Kobo, live BirdNET-Go/Fugleramme and
interactive updated-simulator behavior remain unverified.

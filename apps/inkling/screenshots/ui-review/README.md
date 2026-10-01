# Inkling interaction and feedback review

These are genuine retained-screen captures from Inkling's screen builder, SDK
callbacks and the Cobalt renderer. They are **not interactive simulator or
physical-device screenshots**. The cloud host refused AF_UNIX socket creation,
including a reviewed elevated launch, so `kobo dev` could not start. The
runtime's back/header composition and a clearly synthetic `00:00`, 50% status
strip are included in every capture. The screenshots use the Clara BW metrics
(1072 × 1448, 300 PPI) at 140% text size and the installed Cobalt font set.

## Before and after

- `before-export.png`: storage acknowledged the export, but Statistics showed
  neither the saved result nor any failure information
- `after-export-saved.png`: the acknowledged file is named on Statistics
- `after-export-failed.png`: a rejected save explains what happened and offers
  an explicit retry; repeated Export actions cannot queue duplicate writes
- `before-typing-back.png`: a Back callback left guess entry open; the screen
  also did not own Back, so the runtime would leave the app instead
- `after-typing-back.png`: Back returns to the board without spending a guess;
  reopening entry keeps the unsubmitted draft

Before captures use production source from beta `97153048`, with test-only
capture instrumentation. The accompanying provenance lists source and image
SHA-256 values. `review_capture.rs` is the reproducible capture harness.

## Checks

The focused regression tests cover successful/failed exports, explicit retry,
repeated actions, leaving and reopening Statistics while a write is pending,
keyed acknowledgement routing, Back/Cancel with an unfinished guess, and dense
statistics with all six distribution counts at every supported Clara text size
(80–170%). Existing daily/archive/statistics and persistence tests also pass.

To reproduce the current captures:

```sh
COBALT_REVIEW_CAPTURE_DIR=/tmp/inkling-captures \
  python3 apps/inkling/screenshots/ui-review/capture.py
cargo test -p kobo-inkling --all-targets --all-features
cargo clippy -p kobo-inkling --all-targets --all-features -- -D warnings
```

Remaining validation: the committed drive journey in a host that supports Unix
sockets, and attended physical-device rendering/touch/Back evidence. No runtime,
SDK, network or vocabulary behavior is changed here.

The documented `node tools/app-contribute.mjs --manifest
apps/inkling/cobalt-app.json --dry-run` also passed: version/provenance gates,
formatting, tests, strict clippy, static ARM build verification, and deterministic
Beta-shaped package/catalog generation. The checkout omitted the public test
seed; the repository's beta-store-smoke fixture value (`2a` repeated 32 times)
was materialized locally with mode 0600 for that check and was not committed.
Only the public fixture key and `example.invalid` preview destinations were used.

## Dependency-isolated revalidation

The final normal app suite passes 25 tests; the opt-in capture suite passes 1. The complete `node --test tools/*.test.mjs` suite passes all 105 tests, and app tests (locked), strict clippy, format and contributor/static-ARM checks pass again. App Cargo.toml and the workspace Cargo.lock are byte-identical to beta: capture-only renderer/font dependencies live only in a temporary package created by `capture.py`. Every committed after PNG is byte-for-byte identical to its original reviewed capture (3 images); 30 of 30 full capture-set PNGs also match. The script copies the current app source/assets and the workspace lock into a temporary mirror, adds only the capture module, and never modifies the release dependency graph.

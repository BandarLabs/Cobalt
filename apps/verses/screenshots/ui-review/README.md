# Verses reading and navigation review

These PNGs are genuine app-renderer/SDK-callback captures, not live simulator
or physical-device screenshots. This cloud host refused AF_UNIX socket
creation, including a reviewed elevated launch, so `kobo dev` could not start.
They use Clara BW metrics (1072 × 1448, 300 PPI), installed Cobalt fonts and
140% text. Runtime back/header composition is applied. The non-reading frames
use a synthetic `00:00`, 50% status strip; reading screens correctly hide it.
All online poems in these fixtures are synthetic, and no network request runs.

## Before and after

- The beta online view emitted an entire poem as one text node. The before
  image shows the first sixteen of fifty lines, with no way to read the rest
- Online poems now retain verse lines and stanza gaps across measured pages;
  the first and final pages are captured. Exceptional long lines split at the
  renderer's own Unicode/grapheme-safe boundaries. Layouts are cached for page
  turns and invalidated when content, notices or metrics change
- Favorites and search results now page, including entries after the former
  arbitrary forty-result cutoff. Before/after favorites pages are included
- More by this poet previously cleared the results while retaining an online
  index, causing an index-out-of-bounds panic. It now opens an explicit search
  activity. Back and New search cancel pending work, and late answers cannot
  move the reader back to a screen they left
- Returning from a poem or quote card restores its source view. A single-page
  bundled poem uses a valid single bottom action for Quote card, rather than
  a one-entry action bar rejected by the protocol

Before captures use beta `97153048` production code with test-only capture
instrumentation. The source/image hashes are in `provenance.json`.

## Verification

24 app tests cover every result in a 48-result list, mixed local/online
favorites, every line of a fifty-line poem with stanza gaps, long unbroken
combining sequences, callback cancellation/stale results, repeat page turns,
quote-card Back and protocol validity of every bundled poem. Every supported
Clara text size (80–170%) is exercised. Existing poem/card, persistence,
metadata and service-error tests also pass.

```sh
COBALT_REVIEW_CAPTURE_DIR=/tmp/verses-captures \
  python3 apps/verses/screenshots/ui-review/capture.py
cargo test -p kobo-verses --all-targets --all-features
cargo clippy -p kobo-verses --all-targets --all-features -- -D warnings
```

Format and strict clippy pass. The documented app-contribute dry-run also
passed static ARM verification and deterministic fixture-only Beta packaging.
The repository's missing public beta-store-smoke seed (`2a` × 32) was created
locally at mode 0600, never committed; preview destinations are example.invalid.
Interactive simulator, live PoetryDB behavior, device latency and attended
Kobo touch/rendering remain unverified.

## Dependency-isolated revalidation

The final normal app suite passes 23 tests; the opt-in capture suite passes 1. The complete `node --test tools/*.test.mjs` suite passes all 105 tests, and app tests (locked), strict clippy, format and contributor/static-ARM checks pass again. App Cargo.toml and the workspace Cargo.lock are byte-identical to beta: capture-only renderer/font dependencies live only in a temporary package created by `capture.py`. Every committed after PNG is byte-for-byte identical to its original reviewed capture (5 images); 33 of 33 full capture-set PNGs also match. The script copies the current app source/assets and the workspace lock into a temporary mirror, adds only the capture module, and never modifies the release dependency graph.

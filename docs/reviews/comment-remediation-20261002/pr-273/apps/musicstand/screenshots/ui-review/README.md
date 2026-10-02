# Music Stand collection review

These PNGs are genuine Music Stand retained screens rendered by Cobalt with
Clara BW metrics (1072 × 1448, 300 PPI), real Cobalt fonts, and 140% text.
They are **not interactive simulator or device screenshots**. Unix socket
creation was refused by this cloud host, including a reviewed elevated launch,
so `kobo dev` could not start. The runtime's back/header composition is applied
with a synthetic `00:00`, 50% status strip.

## Reproduced problem

Twelve scores or setlists were drawn in a single unscrollable flow. Lower rows
and the Setlists, Add scores, New setlist, Library, and Remove last score
controls could fall below the panel. `before-library.png` and
`before-setlists.png` use beta `97153048` production builders with test-only
capture instrumentation and twelve synthetic entries.

## Change

All three collections use measured pagination. Add scores remains in the fixed
header and each list keeps one relevant bottom action. Standard Back returns
through the collection views, keeping navigation separate from setlist edits. Long score names are bounded to two measured lines;
page arrows and physical page-turn callbacks reach every row. Creating a
setlist reveals its page. Back from a setlist returns to all setlists, and all
child collection/help screens correctly claim Back from the runtime.

The after images show the first and final library pages, the final setlist
collection page, and the last entries of a twelve-score setlist.

## Verification

`collection_tests.rs` hit-tests the centers of actual layout targets, verifies
all 24 long-titled scores and all 12 setlists, opens the last score, keeps page
positions when returning, bounds repeated final-page actions, and removes every
setlist entry without losing the remaining control. These checks cover every
supported Clara text scale (80–170%), including a notice and marked score.
Existing score crop, zoom, shelf parsing, and persistence tests also pass.

```sh
COBALT_REVIEW_CAPTURE_DIR=/tmp/musicstand-captures \
  python3 apps/musicstand/screenshots/ui-review/capture.py
cargo test -p kobo-musicstand --all-targets --all-features
cargo clippy -p kobo-musicstand --all-targets --all-features -- -D warnings
```

The companion simulator route uses the new concise `New setlist` label, but
that route and physical-device touch/Back/rendering remain unverified here.
The empty-shelf copy being revised separately in PR #224 is preserved.

The documented app-contribute dry-run passed version/provenance gates,
formatting, tests, strict clippy, static ARM verification and deterministic
Beta-shaped package/catalog generation. The repository's public `2a` × 32
beta-store-smoke seed was materialized locally with mode 0600 because the
checkout omitted that fixture file; it was not committed. Preview destinations
are `example.invalid`, and no production signing material was used.

## Dependency-isolated revalidation

The final normal app suite passes 12 tests; the opt-in capture suite passes 1. The complete `node --test tools/*.test.mjs` suite passes all 105 tests, and app tests (locked), strict clippy, format and contributor/static-ARM checks pass again. App Cargo.toml and the workspace Cargo.lock are byte-identical to beta: capture-only renderer/font dependencies live only in a temporary package created by `capture.py`. Every committed after PNG is byte-for-byte identical to its original reviewed capture (4 images); 27 of 27 full capture-set PNGs also match. The script copies the current app source/assets and the workspace lock into a temporary mirror, adds only the capture module, and never modifies the release dependency graph.

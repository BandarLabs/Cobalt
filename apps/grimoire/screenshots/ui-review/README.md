# Grimoire bookmark pagination

These are genuine Grimoire app-builder/SDK-callback captures using Cobalt's
installed fonts at Clara BW metrics (1072 × 1448, 300 PPI), with runtime header
and Back composition and a synthetic 00:00/50% status strip. They are **not live
simulator or physical-device screenshots**: this host denied AF_UNIX socket
creation even for a reviewed elevated launch.

## Matched evidence

`before-bookmarks.png` uses beta `97153048` production source, with test-only
capture instrumentation, and 25 saved references at 140% text. The seventh row
runs through the fixed Back button and the remaining references cannot be
reached. `after-bookmarks.png` uses the same input: five measured rows, a page
indicator, and reachable page/Back controls. `after-last-page-170.png` shows the
last page at the largest text size. Repeated names in this fixture belong to
separate corpus entries/editions and retain their original indices.

Bookmarks now paginate using measured SDK rows, independently of the compendium
page. Opening and closing a reference returns to the same bookmark page. Invalid
stored references are ignored for display without rewriting stored data.

## Validation

26 app tests passed, including all 25 bookmarks across every Clara text size
(80–170%), actual layout hit-testing, final-item opening, Back restoration,
independent compendium position, repeated boundary turns, a shortened collection,
and invalid stored indices. Existing combat, party, reference, and persistence
tests continue to pass.

```sh
COBALT_REVIEW_CAPTURE_DIR=/tmp/grimoire-captures \
  python3 apps/grimoire/screenshots/ui-review/capture.py
cargo test -p kobo-grimoire --all-targets --all-features
cargo clippy -p kobo-grimoire --all-targets --all-features -- -D warnings
```

Workspace format check and strict app clippy passed. The documented contributor
dry-run passed, including static ARM verification and deterministic fixture-only
Beta package/catalog generation. The public beta-store-smoke seed (`2a` × 32)
was supplied locally at mode 0600 and is not committed. No SDK/runtime or
capability changes. Interactive simulator and physical Kobo checks remain open.

## Dependency-isolated revalidation

The final normal app suite passes 24 tests; the opt-in capture suite passes 2. The complete `node --test tools/*.test.mjs` suite passes all 105 tests, and app tests (locked), strict clippy, format and contributor/static-ARM checks pass again. App Cargo.toml and the workspace Cargo.lock are byte-identical to beta: capture-only renderer/font dependencies live only in a temporary package created by `capture.py`. Every committed after PNG is byte-for-byte identical to its original reviewed capture (2 images); 30 of 30 full capture-set PNGs also match. The script copies the current app source/assets and the workspace lock into a temporary mirror, adds only the capture module, and never modifies the release dependency graph.

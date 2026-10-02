# Fieldbook lists and sighting safety

These are genuine Fieldbook retained screens and SDK-callback captures, rendered
with Cobalt's installed fonts and Clara BW metrics (1072 × 1448, 300 PPI) at
140% text. They are **not live simulator or device screenshots**: this cloud
host refused AF_UNIX socket creation even for a reviewed elevated launch.
Runtime header/back composition and a synthetic `00:00`, 50% status strip are
included. All outing/species data here is synthetic or from committed fixtures.

## Findings and changes

Before frames use beta `97153048` production builders with test instrumentation.

- History clipped into the navigation bar. Packs, sightings and the life list
  each stopped after six entries, with no route to the rest. All four lists now
  use measured SDK pagination, including navigation after empty/failure states
- Tapping a sighting immediately deleted it. A standard Delete/Keep confirmation
  now precedes removal, Back dismisses it, repeated confirmation is harmless,
  and Undo remains pinned and restores the exact entry
- Two manually entered names shared an empty banding code and were merged into
  one record. The matched before/after manual frames show Robin and Crow. Empty
  codes now use the species name as identity; coded species retain their code
  identity, and all existing record fields/serialization stay unchanged
- Life-list entries now open their species details rather than accepting taps
  without a result

Previously merged records cannot be reconstructed automatically: only the
first name and combined count remain in those saved records. This change does
not guess, split historical counts or attempt a migration. A regression proves
old records round-trip unchanged and new distinct names no longer merge.

## Verification

13 tests pass. New checks hit-test all 24 outings, 12 packs and 18 sightings/
life-list entries at every Clara text size (80–170%). They cover opening final
entries, bounded repeated page turns, delete/keep/Back/Undo and repeated
confirmation, failed-save/empty states, manual-name identity, export separation
and exact preservation of existing storage bytes. Existing pack/photo metadata
parsing tests also pass.

```sh
COBALT_REVIEW_CAPTURE_DIR=/tmp/fieldbook-captures \
  python3 apps/fieldbook/screenshots/ui-review/capture.py
cargo test -p kobo-fieldbook --all-targets --all-features
cargo clippy -p kobo-fieldbook --all-targets --all-features -- -D warnings
```

Format and strict clippy pass. The documented contributor dry-run passed static
ARM verification and deterministic fixture-only Beta package/catalog creation.
The omitted public beta-store-smoke seed (`2a` × 32) was materialized locally at
mode 0600, not committed. No production signing keys, remote service or physical
device storage was used. Interactive simulator and attended Kobo touch/Back,
photo loading and companion transfer validation remain unverified here.

The existing Fieldbook simulator journey now explicitly checks Keep sighting, confirmed Delete, and Undo. Its Python syntax compiles; the journey was not run locally because Unix sockets are unavailable.

## Dependency-isolated revalidation

The final normal app suite passes 10 tests; the opt-in capture suite passes 3. The complete `node --test tools/*.test.mjs` suite passes all 105 tests, and app tests (locked), strict clippy, format and contributor/static-ARM checks pass again. App Cargo.toml and the workspace Cargo.lock are byte-identical to beta: capture-only renderer/font dependencies live only in a temporary package created by `capture.py`. Every committed after PNG is byte-for-byte identical to its original reviewed capture (6 images); 45 of 45 full capture-set PNGs also match. The script copies the current app source/assets and the workspace lock into a temporary mirror, adds only the capture module, and never modifies the release dependency graph.

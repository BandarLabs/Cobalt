# Needles collection review

These PNGs are genuine Needles retained screens rendered by Cobalt with Clara
BW metrics (1072 × 1448, 300 PPI), installed Cobalt fonts, and 140% text. They
are **not live simulator or physical-device screenshots**: the cloud host
refused Unix socket creation even for a reviewed elevated launch. The renderer
uses runtime back/header composition and a synthetic `00:00`, 50% status strip.

The before frames use beta `97153048` production builders with test-only
instrumentation. Twelve projects and twenty patterns overflowed the panel;
lower entries and the Sync button were unreachable. The after frames show
measured pagination, final pages, and a pinned Sync control that remains
reachable after an account refusal. The redundant Library section label was
removed because the selected tab already supplies the same label.

The regression suite hit-tests every item in a sixty-pattern collection and all
twelve projects at all supported Clara text scales (80–170%). It verifies the
last item opens, Back from a pattern returns to the same collection/page, tabs
retain independent pages, project counters remain independent, and repeated
final-page actions stop at the boundary. Additional callback tests prove that
repeated Sync actions retain one request, a refreshed collection resets only
its own page, and a failed sync leaves an explicit retry reachable.

```sh
COBALT_REVIEW_CAPTURE_DIR=/tmp/needles-captures \
  python3 apps/needles/screenshots/ui-review/capture.py
cargo test -p kobo-needles --all-targets --all-features
cargo clippy -p kobo-needles --all-targets --all-features -- -D warnings
```

No live Ravelry account, remote request or device storage is used by these
fixtures. Interactive simulator, real network/authentication, and physical
Kobo touch/Back validation remain to be performed on a suitable host/device.

The documented app-contribute dry-run also passed version/provenance gates,
formatting, tests, strict clippy, static ARM verification and deterministic
Beta-shaped package/catalog generation. Its missing public beta-store-smoke
seed (`2a` repeated 32 times) was created locally at mode 0600, never committed;
only fixture signing material and `example.invalid` destinations were used.

## Dependency-isolated revalidation

The final normal app suite passes 17 tests; the opt-in capture suite passes 1. The complete `node --test tools/*.test.mjs` suite passes all 105 tests, and app tests (locked), strict clippy, format and contributor/static-ARM checks pass again. App Cargo.toml and the workspace Cargo.lock are byte-identical to beta: capture-only renderer/font dependencies live only in a temporary package created by `capture.py`. Every committed after PNG is byte-for-byte identical to its original reviewed capture (5 images); 30 of 30 full capture-set PNGs also match. The script copies the current app source/assets and the workspace lock into a temporary mirror, adds only the capture module, and never modifies the release dependency graph.

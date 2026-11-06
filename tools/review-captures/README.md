# Review capture drivers

Capture-only Python and Rust fixtures live here, outside the production apps, so ordinary workspace tests never compile them. App regression tests stay in their own source modules. Output images, large diagnostics and historical manifests live on the archived evidence branch, linked from each packet's README. `relocation-*.json` maps each original driver path to its location here.

Run from the repository root with Python 3.11+, Cargo and the repository's Rust toolchain. Builds run with `--offline`, so the dependency cache must already be filled. Native captures exercise the real app screen builders and the renderer and font code in a temporary Cargo package. They do not prove interactive navigation in the runtime or behaviour on hardware.

For an app with `src/review_capture.rs` under this directory, `run.py` copies the current app, overlays the capture files in the temporary package, and runs the ignored capture tests:

```sh
python3 tools/review-captures/run.py apps/parser --out /tmp/parser-captures
python3 tools/review-captures/run.py apps/backgammon --out /tmp/backgammon-captures
```

`--filter` narrows the tests run (default `capture_`). App dependencies and the workspace lockfile are left unchanged, and app and shared test fixtures are available in the temporary copy.

The other drivers keep their original command lines, with `tools/review-captures/` in front of the script and scenario paths. For example:

```sh
COBALT_REVIEW_CAPTURE_DIR=/tmp/birds-captures \
  python3 tools/review-captures/apps/birds/screenshots/ui-review/capture.py
python3 tools/review-captures/examples/chat/screenshots/ui-review/capture.py \
  --source-root "$PWD" --out /tmp/chat-captures
python3 tools/review-captures/docs/reviews/audit-five-fixes/capture.py --help
```

A `--baseline` option builds the historical source pinned in its driver, not today's beta. The archived packet notes give each scenario's original source hashes and output variables.

CI checks the harness structure, relocation and injection through `node --test tools/*.test.mjs`. To run that check alone:

```sh
python3 -B tools/review-captures/test_relocation.py
```

These checks do not look at pixels. When you change a scenario or a production screen builder, run a native capture and inspect the images. Full nine-profile and interactive-runtime evidence stays in the archived packets.

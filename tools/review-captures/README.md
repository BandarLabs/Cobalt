# Review capture drivers

Capture-only Python and Rust fixtures live here, outside the production apps. Normal app regression tests stay in their original source modules. Output images, large diagnostics, and historical manifests live on the immutable evidence branch, linked from each original packet README. `relocation-*.json` maps original driver paths to their new locations.

Run from the repository root with Python 3.11+, Cargo, and the repository's Rust toolchain/dependency cache. Native captures exercise real app screen builders and renderer/font code in an isolated temporary Cargo package. They do not prove interactive runtime navigation or physical hardware behavior.

For an app with `src/review_capture.rs` under this directory, the common runner copies the current app and overlays the relocated capture files only in a temporary package:

```sh
python3 tools/review-captures/run.py apps/parser --out /tmp/parser-captures
python3 tools/review-captures/run.py apps/backgammon --out /tmp/backgammon-captures
```

Use whichever app exists in the checked-out PR. This runner includes ignored capture tests; ordinary workspace tests no longer compile these capture-only modules. App dependencies and the workspace lockfile remain unchanged. App fixtures and shared test fixtures remain available in the temporary mirror.

Other existing drivers retain their command-line interface. Prefix the old script and scenario paths with `tools/review-captures/`. For example:

```sh
COBALT_REVIEW_CAPTURE_DIR=/tmp/birds-captures \
  python3 tools/review-captures/apps/birds/screenshots/ui-review/capture.py
python3 tools/review-captures/examples/chat/screenshots/ui-review/capture.py \
  --source-root "$PWD" --out /tmp/chat-captures
python3 tools/review-captures/docs/reviews/audit-five-fixes/capture.py --help
```

Only use drivers present in the current checkout. `--baseline` options retain their explicitly pinned historical source; they are not aliases for today's beta. Archived packet notes give each scenario's original source hashes and output environment variables.

Lightweight syntax, relocation, and injection checks run through `node --test tools/*.test.mjs` in existing CI. Run them alone with:

```sh
python3 -B tools/review-captures/test_relocation.py
```

These checks validate the harness structure, not pixels. Run a native capture and inspect its images when changing a scenario or production screen builder. Full nine-profile and interactive-runtime evidence remains separately identified in the archived packets.

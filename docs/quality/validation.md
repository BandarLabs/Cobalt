# Foundation validation log

This records an implementation checkpoint, not completion of PR 1 or the three-PR program. The checklist retains open work. Hardware execution remains scheduled after all three PRs are ready.

## 8 September 2026: typography, simulator and CBZ

- `cargo test -p kobo-ui --lib`: **258 passed, 2 existing ignored**. Scoped scale tests cover render/measurement agreement and restoration after unwind.
- `cargo test -p kobo-catalog --lib`: **1 passed**, all 44 bundled publishing manifests valid and unique.
- `cargo test -p kobo-sim --lib`: **48 passed**. Includes screenshot-cadence invariance, unseen screen commits, capability declarations, credentialed GET/POST failures, streaming headers, full shelf writes, strict profile/scale parsing and bounded history.
- `cargo +1.85.1 test -p kobo-comic -p kobo-panels`: **9 comic + 7 Panels tests passed**, plus doc tests. Subsequent Panels error/Back changes passed its 7 tests on the current toolchain.
- `cargo +1.85.1 check -p kobo-comic --target armv7-unknown-linux-musleabihf`: **passed**. This is a cross-target compile check, not a linked device build or hardware execution.
- `cargo clippy -p kobo-catalog -p kobo-comic -p kobo-sim -p kobo-panels --all-targets -- -D warnings`: **passed**.
- `cargo build -p kobo-cli -p kobo-panels`: **passed**; driven simulator runs rebuild the current Panels source.

Commands use `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0` to conserve disk space.

## Shared collections and document reflow

- `cargo test -p kobo-sdk -p kobo-bookview -p kobo-read --lib`: **108 SDK, 14 BookView and 79 Reader tests passed**. Four SDK tests exercise collection selection/reflow/deletion and invalid-input atomicity.
- After adding `BookView::reflow`, its suite passed **15 tests**, including plain text, Markdown and HTML anchors across portrait/landscape changes.
- `cargo clippy -p kobo-sdk -p kobo-bookview --all-targets -- -D warnings`: **passed**.
- Combined checkpoint run for UI, simulator, catalog, comic, SDK, BookView and Reader: **518 passed, 2 existing ignored**. Panels' **7 binary tests** passed separately; `--lib` does not select those.

The [shared app contracts](shared-ui-contracts.md) document reuse of the existing reader, collection navigation and control hierarchy. Catalog adoption remains app-specific open work.

## Actual simulator journeys

`scripts/quality/check-comics-sim.py` creates original geometric PNG pages and a CBZ in private temporary storage. It opens the comic, turns forward/back, returns to the library through the actual Back hit target, checks serious diagnostics and verifies repeated screenshot sampling leaves complete simulation metadata unchanged. Waits use expected screen content with a deadline.

Passed runs on Clara BW:

- Default interface size: [opened page](evidence/comics/default-02-opened.png), [state and diagnostics](evidence/comics/default-02-opened.json).
- Extra-large interface size: [opened page](evidence/comics/extra-large-02-opened.png), [state and diagnostics](evidence/comics/extra-large-02-opened.json).
- RAR content behind a `.cbz` name: [explicit CBR guidance](evidence/comics/cbr-02-opened.png), [state and diagnostics](evidence/comics/cbr-02-opened.json).

The first draft of the driver incorrectly quoted a multiword label, used a fixed delay before decode finished, and looked for Back as visible text. Those driver mistakes were corrected to match the CLI grammar, wait for semantic state and locate the actual Back node. The resulting runs passed. No external comic artwork or upstream test archive was used.

## License and advisory evidence

The selected ZIP normal dependency closure is recorded in `THIRD-PARTY.md`. Published package manifests and license files offer MIT or Apache-2.0 throughout this closure; ZIP's MIT copyright/terms were added to the shipped Rust dependency notices. No RAR decoder is present.

`cargo audit --json` could not complete because its refreshed advisory database contains duplicate ID `RUSTSEC-2026-0244`. Do not treat this as a clean full-workspace audit. Direct inspection found the database's ZIP advisory RUSTSEC-2025-0168 fixed from 2.3.0 (selected 4.6.1), and hashbrown advisory RUSTSEC-2024-0402 fixed from 0.15.1 (selected 0.17.1). There were no other entries for the selected ZIP normal closure in that local database. Re-run the complete audit when the upstream database is corrected.

## Still open

The foundation is incomplete. This checkpoint does not claim comic zoom/pan, metadata preferences, thumbnails, durable sideload-library registration, whole-volume streaming, full-runtime app switching, simulator hardware observations, all SDK contracts, catalog polish or companion work is done. The existing Panels transfer cap is still lower than the shared archive admission cap; reconcile it in the app/import work. Physical display, suspend/wake and memory/latency measurements require the owner's Clara BW.

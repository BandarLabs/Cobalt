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

The foundation is incomplete. This checkpoint does not claim durable sideload-library registration, whole-volume streaming, full-runtime app switching, all SDK contracts, catalog polish or companion work is done. The shared reader and observation work below supersedes the earlier comic controls and simulator observation gaps. Physical display, suspend/wake and memory/latency measurements require the owner's Clara BW.

## Save acknowledgements, mutation outbox and driver checks

- `cargo test -p kobo-state`: **11 unit tests and 1 actual-store integration test passed**. Covers save-before-send, stale acknowledgements, edits during a write, failed storage, byte/count limits, corrupt/future snapshots, restart after a provider acknowledgement and retry/conflict retention.
- `cargo clippy -p kobo-state --all-targets -- -D warnings`: passed, including the actual-store integration test.
- `cargo test -p kobo-cli drive::tests`: **15 passed**. Captures/recordings now follow the selected profile rather than assuming Clara dimensions. A new regression checks every supported profile and rejects missing/oversized/invalid dimensions. Transition commands automatically reject serious layout diagnostics, and Back has a semantic name even though its glyph has no painted text. `tap-id` uses the reachable control's action ID and still sends a real coordinate tap.
- Driven Panels CBZ flow on `libra-colour-390`: **passed**, including actual CLI PNG dimensions at every capture and serious-diagnostic checks. This verifies geometry in the existing grayscale simulation; it is not color calibration.

## Lost input recovery and local manifests

The HAL now cancels incomplete gestures on `SYN_DROPPED`, ignores events through the next report boundary, and queries current contact state before accepting a new gesture. Unsupported or failed queries keep input blocked rather than inventing a release. Quiescence checks no longer treat a disconnected or unresolved stream as idle. The runtime clears press feedback on cancellation and cannot turn it into an app action. This follows the [Linux input event contract](https://docs.kernel.org/input/event-codes.html#ev-syn); no local OSS reference implementation was copied.

- HAL with `device-write`: **153 tests passed**.
- Runtime with `device-write`: **148 tests passed**.
- ABI/HAL/runtime Clippy with `-D warnings`: **passed**.
- Rust 1.85.1 ARMv7 musl runtime check with `device-write`: **passed** using the installed `armv7-unknown-linux-musleabihf-gcc` and archiver. The first attempt could not find the compiler under Cargo's default name. This is compilation, not physical execution.
- Simulator: **50 tests passed**, including local manifest identity/capability isolation and production `Unsupported` versus `NotDeclared` service refusal.
- Simulator and CLI Clippy with `-D warnings`: **passed**.

`kobo dev` reads the working app's bounded `cobalt-app.json`; its identity must match the SDK Hello. Unknown apps without metadata get no implicit capabilities. The default simulated backends are a conservative development model (network, battery, frontlight, Wi-Fi, cover and library), not a hardware measurement. `KOBO_SIM_BACKENDS` selects an explicit comma-separated set; an empty value simulates no services, and invalid names fail startup. The observation-derived backend configuration described below supersedes this checkpoint.

Physical lost-input recovery and settings restoration are still part of the combined Clara BW run after all three PRs.

## Shared comic controls, durable positions and hardware observations

- Combined test run: **268 passed** (BookView 16, comic core 17, doctor 4, image 23, Panels 8, profile 40, SDK 109, simulator 51); two existing SDK documentation examples remain ignored.
- Clippy for those packages and CLI, all targets with `-D warnings`: **passed**.
- Rust 1.85.1 ARMv7 musl check for comic, BookView, Panels and doctor: **passed**, using the installed cross compiler. This does not establish linked device memory or physical performance.
- All supported profiles, three text sizes, portrait and landscape reader routes check both serious diagnostics and reachable hit targets before each action, including save-failure and details surfaces.
- Shared reading memory retains filename anchors, direction, spreads, fit, zoom and normalized pan across restart. A correlated SDK `on_save` callback associates even keyless storage refusals with the requested save; older apps retain their existing `on_store` behavior. Regression tests interleave library writes with failed position saves and newer page changes.
- `check-comics-sim.py --reader-tools` passed at default and extra-large sizes. The expanded extra-large run injects storage-full, checks a visible failure, retries successfully, then checks RTL/spreads/zoom after restarting the process with the same private storage. Evidence: [failed position](evidence/comics/shared-xl-15-position-save-failed.png), [successful retry](evidence/comics/shared-xl-16-position-retry-saved.png), [controls](evidence/comics/shared-xl-05-reading-controls.png), [pan](evidence/comics/shared-xl-07-pan.png), [RTL spread](evidence/comics/shared-xl-10-rtl-spread.png), [reopened page](evidence/comics/shared-xl-12-reopened.png), [result](evidence/comics/shared-xl-result.json). Each matching JSON records layout, diagnostics and simulation state; sampling frame endpoints remains read-only.
- `doctor --json` produces the versioned bounded observation format. `KOBO_SIM_OBSERVATION` accepts it, derives the observed display pose independently of the digitizer mapping, and rejects profile/geometry mismatch. Service availability remains an explicit observation, never inferred from an empty list. The included Clara fixture is **synthetic**, not an owner's hardware capture. An observation-driven simulator comic journey passed.

Metadata and two-page covers have bounded regression tests. Page decode is lazy with a two-page LRU cache; the archive remains in memory, capped at 32 MiB consistently across the reader and Panels transfer. The maximum archive is an admission limit, not measured safe memory headroom on a Clara BW. Library registration acknowledgement, import receipts, color calibration and physical acceptance remain open.


## Provider setup, records and verified imports

- SDK: **118 tests passed**, including provider header conventions, bounded address validation, cancelled/stale response isolation, genuine provider-validation gating, receipt failures, read-back integrity and rechecking missing files. Provider and import layouts pass all supported profiles and three text sizes with Cobalt's real font installed. Two existing SDK documentation examples remain ignored.
- State: **14 unit tests + 1 actual-store integration test passed**. New record cases cover incremental migrations, wrong/future/empty/corrupt schemas, duplicate JSON fields, excessive intermediate records and preservation of source bytes.
- Policy: **108 tests passed**, including bounded reads, expected missing versus unreadable/oversized records, failed removal honesty, cache pruning and durable-key isolation.
- Comic: **17 passed**; Panels: **9 passed**. The additional Panels regression ensures an unrelated record-save refusal cannot fail a comic transfer. Named shelf callbacks correlate keyless refusals with their actual transfer.
- Clippy for state, policy, SDK, Panels and comic, all targets with `-D warnings`: **passed**.
- Rust 1.85.1 ARMv7 musl check for state, SDK and Panels: **passed**.
- The full extra-large comic simulator journey passed again after the shelf correlation and transfer validation changes, including storage-full retry and process restart.

The SDK shelf download has a 32 MiB in-memory ceiling. Comic admission was reduced from 64 MiB to **32 MiB** to match it, with a compile-time Panels assertion preventing the limits from drifting. The archive still resides in memory; this does not claim whole-volume streaming or measured physical headroom.

Import verification checks transfer identity, not document parsing. Apps must validate their actual formats and adopt the receipt/library flow. These shared contracts do not complete all app-specific imports, migrations, cache freshness, index recovery or companion workflows. They add only Cobalt workspace dependencies; no new external codec or third-party runtime dependency was introduced.


## Semantic simulator assertions and callback completion

- SDK **118 tests**, simulator **53 tests**, and CLI driver **16 tests** passed. Clippy for UI/protocol/SDK/simulator/CLI, all targets with `-D warnings`, passed.
- `kobo dev` opts its child into a callback-completion marker. The SDK emits it after all commands from a callback, including callbacks with no screen change. It uses an existing debug-log frame; no wire version, runtime authority or production device operation changed. Older/custom clients without markers are never asserted idle.
- `/activity` records pending callbacks, active tasks, sleeping timers and bounded aggregate attempt/outcome counts. Task completion atomically removes work and adds its pending callback, so the interval before the app handles a response cannot appear idle. A disconnected app cannot appear idle. Future sleeping timers are reported separately and do not prevent current quiescence.
- `wait-idle [MS]` waits for this actual completion boundary, with a maximum 60-second deadline. `tap-id` and `wait-for-id` accept stable action names via the same hash used by SDK builders. `expect-state ENDPOINT#JSON_POINTER JSON_VALUE` compares typed values, refusing missing paths and string/number mismatches.
- The extra-large shared comic flow passed using stable action names, explicit idle waits, storage-full recovery, process restart and assertions of zero fetch/post attempts. It no longer uses a fixed delay before captures. [Result](evidence/comics/semantic-idle-result.json).

Activity metadata counts app request attempts, including locally refused requests. It does not claim a remote mutation succeeded. It contains no URL, credential, request body or returned document. Simulator task logs now summarize returned byte counts instead of retaining response bytes. Virtual clock, raw HAL replay and full-runtime switching remain open.

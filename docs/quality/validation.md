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


## Atomic simulator capture provenance

Screenshots and retained recording frames now receive a JSON sidecar from the same locked frame snapshot as their pixels. The envelope records app identity, single-app versus counter mode, runtime version, source revision and dirty state, app binary SHA-256, profile and pose, installed font source filenames, interface and reading sizes, fixture label and requested seed. Unknown source fields remain null. A seed label does not establish that an arbitrary app consumes deterministic entropy, and font filenames are not font-content hashes. Capture endpoints remain read-only.

- Simulator: **53 tests passed**. CLI driver: **17 tests passed**, including rejecting truncated or changed capture bytes and matching each retained recording frame to its sidecar digest.
- Simulator, CLI and text Clippy, all targets with `-D warnings`: **passed**.
- Full extra-large comic route: **passed**, including atomic captures, navigation, rotation, process restart, storage failure and retry. Each capture checks source and font provenance, dimensions and serious diagnostics. [Result](evidence/comics/atomic-capture-result.json), [failure screen](evidence/comics/atomic-capture-save-failed.png), [matching provenance](evidence/comics/atomic-capture-save-failed.json).

The capture identifies this verification build as a dirty working tree on its parent revision; it does not mislabel uncommitted changes as that commit. Physical panel calibration remains pending.


## Controllable time and fixture entropy

- Policy: **111 tests passed**; SDK: **120 passed**; simulator: **54 passed**. Cases cover Gregorian leap centuries, UTC-offset date boundaries, wall corrections without changing monotonic time, atomic rejection of overflow, sleep admission limits, ordering, cancellation, clamping and exactly-once completion. Simulator callback delivery is serialized with clock advancement. Two existing SDK documentation examples remain ignored.
- Clippy for policy, SDK, simulator and CLI, all targets with `-D warnings`: **passed**.
- Rust 1.85.1 ARMv7 musl SDK check: **passed**, including installed-font provenance and clock/entropy APIs.
- Full extra-large comic simulator journey with manual clock: **passed**, including driver advancement, explicit offset change and typed clock assertions. [Result](evidence/comics/manual-clock-result.json), [capture provenance](evidence/comics/manual-clock-library.json).

The simulator clock controls sleep callbacks and its status clock; HTTP transport deadlines stay real. Existing apps reading their own operating-system clock are not automatically rewritten. Catalog adoption of the injectable SDK clock/date and entropy contracts remains in PR 2. A fixed UTC offset is explicit and does not claim automatic time-zone or daylight-saving rules. Capture metadata retains the clock snapshot used for the committed screen, so screenshot sampling does not alter it.

Removed approximately 7.2 GiB of three inactive `/tmp/cobalt-...-target` Cargo caches after checking their cache markers, contents and absence of running users. Source files, original checkout edits and review evidence were preserved.


## Simulator device observations and browser controls

- Simulator: **56 tests passed**; simulator and CLI Clippy, all targets with `-D warnings`: **passed**. Controls reject invalid/ambiguous values without mutation. Battery reads, charging and frontlight use the same observed values as the simulator. Low-battery injection preserves the owner's modeled charging state and restores the prior battery observation when removed. Cover events are edge-only and foreground-only, using the production SDK event.
- Full extra-large comic route: **passed** with driver battery/charging/frontlight/cover commands and state assertions. [Result](evidence/comics/device-controls-result.json).
- In-app browser controls: battery 18% charging, light 39%, closed cover and 60-second clock advancement verified against service state. Landscape/portrait composition had no layout errors for the tested empty Panels library. Browser console had no errors. At viewport width 390, document scroll width was also 390. [Recorded state](evidence/simulator/device-browser-result.json).

The browser uses the atomic frame envelope and waits for app callbacks after simulator controls. Inspector markup/styles/scripts now live in `shell.html`. Its refresh-debt label correctly reports repainted pixels; the former “partials / 8” label attached a count to a pixel total. The JSON field is now `dirtyPixelsSinceClean`.

Frontlight controls update service values, not calibrated visual illumination. Display orientation composes the existing screen; the app's own rotation action is required to exercise application reflow. Digitizer mapping remains separate. These are explicit inspector limits, not claims of hardware accuracy. A first browser fixture launch hit the Unix socket path limit under macOS's long default temporary path; rerunning in a short private `/tmp` directory succeeded. CLI handling of that path remains open.


## Panel submission, completion and recovery

The simulator can hold updates in progress, retain only the newest queued frame, complete or fail a submission, and retry. The planner commits only confirmed completions. Screenshots cannot complete work. Visible pixels while busy or failed represent the last confirmed frame, and metadata marks current contents uncertain. Input is refused until the panel is ready. Retrying a failed update requires whole-panel cleaning while retaining the refresh sequence. The runtime also invalidates its planner on an actual region-submission failure, so any retry cannot rely on partly updated content.

- Simulator: **58 tests passed**. Runtime with `device-write`: **149 binary + 16 library tests passed**. Cases cover queue coalescing, no sampling-induced completion, failed/cancelled control refusal, uncertain input, full-clean recovery and sequence preservation.
- Simulator, CLI and runtime Clippy, all targets with `device-write` and `-D warnings`: **passed**.
- Rust 1.85.1 ARMv7 musl runtime check with `device-write`: **passed**.
- Full extra-large comic journey: **passed**, including eight unchanged captures while busy, a second queued frame, failed completion, refused tap, full-clean retry and the existing reader/save/restart checks. [Result](evidence/comics/panel-recovery-result.json), [last-confirmed screen after failure](evidence/simulator/panel-failed.png), [uncertain-state metadata](evidence/simulator/panel-failed.json).
- Browser hold → advance clock → fail → retry → complete → automatic: **passed**, ending in an acknowledged full refresh with no layout or console errors. [State](evidence/simulator/panel-browser-result.json).

Pending and latest surfaces are bounded to the selected profile; this is a controlled simulator queue model, not a measured hardware pipeline. Submission timestamps record host control observations. The model does not claim electrophoretic timing, intermediate pixels, device busy-ioctl behavior or physical waveform calibration. Input replay and physical timings remain open.


## Raw input replay through HAL

SDK simulator taps now produce evdev contact reports through `TouchDecoder`. Driver `input touch` accepts bounded raw down/move/up reports, `input gpio` uses the actual GPIO decoder, and `input resync` supplies explicit synthetic query outcomes after `SYN_DROPPED`. The device runtime and simulator share `HoldTracker` with the existing 500 ms / 40 pixel policy. A backward clock, movement past the threshold, cancellation or unmatched release cannot manufacture a hold. Text holds and page events use existing SDK messages and layout hit testing. Background input produces no app messages.

- HAL with `device-write`: **154 tests passed**; simulator: **61 passed**; runtime: **149 binary + 16 library passed**. Cases cover real decoder output, shared hold classification, key press versus release/repeat, portrait key mapping, atomic bad-batch refusal, explicit unknown/active/released resynchronization and real app-facing hold/page messages.
- HAL, simulator, CLI and runtime Clippy, all targets with `device-write` and `-D warnings`: **passed**.
- Rust 1.85.1 ARMv7 musl runtime check with `device-write`: **passed**.
- Full extra-large comic route: **passed** through the new HAL tap path. It injects lost input and unknown-then-released resynchronization, turns forward/back using raw page keys, checks that release does not turn again, and repeats the panel/storage/restart routes. [Result](evidence/comics/raw-input-result.json), [page-key capture provenance](evidence/comics/raw-page-key-next.json).

Replay is an explicit synthetic input channel, including when a profile has no physical page buttons. It does not claim evdev grabs, hardware sampling rates, GPIO availability or accelerometer/landscape-turn calibration. Browser clicks remain synthesized taps; use raw reports and clock advancement for holds and movement. Press-feedback timing and full-runtime Back/launcher behavior remain separate open fidelity work.

## Interrupted tasks and disconnected apps

- **112 policy and 63 simulator tests passed**. A controlled transfer is cancelled through the session API and reports one `Cancelled` outcome; later work is still accepted. A real socket disconnect with a five-minute timer releases its task runner and output workers, records abandoned work separately from successful/cancelled callbacks, refuses further input and retains the last screen.
- **17 driver tests passed**; policy/simulator/CLI Clippy with `-D warnings` passed.
- The extra-large Panels route passed background/foreground events, suppressed background page keys, task cancellation, and a forced `SIGKILL` of its independently created fixture process group. Restarting against the same private storage restores page, zoom, direction and spreads. [Result](evidence/simulator/forced-exit-result.json). This is host process recovery, not a device watchdog or suspend test.

`drive --step 'tasks cancel'` cancels current app tasks and awaits callbacks. The browser exposes the same control. `drive --step 'session disconnect'` closes SDK IPC; it does not claim to send a process signal. `/activity` reports `connected`, `abandoned` and `cleanupComplete`; disconnected sessions never report idle. Cleanup waits for task backends to honor their cancellation contract. Hardware forced-exit and owner-setting restoration remain in the combined Clara BW acceptance run.

## Color inspection and comic rendering

- **259 UI, 18 comic, 17 BookView and 9 Panels tests passed**; two existing UI tests remain ignored. Tests cover both landscape turns, clearing stale color when returning to grayscale, opt-in decode, mixed color/grayscale spreads, cache invalidation without position loss, and downsizing RGB to fit the existing wire limit.
- **64 simulator and 19 driver tests passed**. RGB captures validate channel length and digest and retain exact PNG channels. Inspecting ideal color does not complete a held update or conceal failure. The driver now awaits callback completion after a tap, including slow image processing, without waiting for an active download to end. The built-in counter and older simulators keep their bounded paint wait.
- All changed packages pass Clippy with `-D warnings`. Rust 1.85.1 ARMv7 musl Panels check passed; this includes the shared reader/UI changes, not physical execution.
- The original RGB comic journey passed on [Libra Colour](evidence/comics/colour-profile-result.json), including the full tools/save-failure/SIGKILL/reopen route, and on [Clara BW](evidence/comics/colour-grayscale-profile-result.json), which correctly outputs luminance. Browser color inspection and task cancellation were exercised. [Portrait RGB](evidence/comics/colour-portrait.png) and [landscape spread RGB](evidence/comics/colour-landscape-spread.png) have verified digest-matching JSON sidecars.

`GET /colour-capture` and `drive --step 'shot-colour NAME'` provide ideal `rgb24` output with atomic provenance. The browser's **Show ideal color** appears only for a color profile and states that color-filter resolution, contrast and saturation are uncalibrated. Existing capture and recording endpoints remain `grey8`; approximate residue remains a luminance-only model. Simulator identity now names its configured profile and dimensions while retaining an explicit `SIMULATOR:` prefix, simulated model, zero device code and empty firmware/kernel.

Comic color is enabled only after an identity response reports color support; the default remains grayscale. The decode cache retains at most two pages and 24,836,096 bytes of decoded planes; a large spread can temporarily need additional decode/crop buffers. Physical peak memory and latency remain unmeasured. Existing 32 MiB archive, 4 MiB encoded page and bounded image/wire limits still apply. No codec or external dependency was added, and CBR remains deferred.

## Shared feedback, samples and metric validation

- **126 SDK unit tests passed**; two existing documentation examples remain ignored. The suite includes all supported profiles, portrait/landscape and three interface sizes. Feedback states preserve the exact rectangles of content and pinned controls. A failed acknowledged write retains the draft and cannot display `Saved`.
- New metric-aware construction rejects a screen with an overflowing paragraph and hidden action even though it passes collection-only validation. Collection truncation also remains an error.
- Sample selection provides 12 original notes, 8 original short readings and 12 arithmetic cards without network, store or account-verification side effects. Sample identities are unique and exports explicitly mark their provenance. App-specific adoption remains in PR 2.
- ScreenBuilder implementation was moved unchanged into a dedicated module; public types and shared UI contracts remain stable. SDK Clippy (`-D warnings`), strict rustdoc (`RUSTDOCFLAGS=-D warnings`), text-disabled compilation and Rust 1.85.1 ARMv7 musl SDK compilation passed.


## Clara BW combined validation preparation

The [physical protocol](clara-bw-validation.md) and `scripts/quality/clara-bw-check.py` prepare the combined owner-assisted run after all three PRs are ready. The default creates a private plan with no device commands. Actual execution first requires a real Clara BW 391 observation; existing HAL firmware/write gates remain in force. Evidence records source/CLI digests and keeps physical assessment pending.

- **7 harness tests passed**: synthetic/wrong-device refusal, stop before panel operations, effect-free planning, named physical touch points, strict setting comparisons, boot/suspend evidence and timeout cleanup of the owned host process group.
- **19 existing CLI developer-session tests passed** after adding a read-only kernel boot identity to status. An unchanged uptime value cannot hide a different boot.
- Baseline, display, touch and recovery plans generated successfully. [Example display plan](evidence/hardware/clara-planned-display.json) is explicitly unexecuted. No reader was contacted, no physical timing was measured and no calibration coefficient changed.

Sleep/wake, interrupted work, guardian recovery and setting/frontlight restoration have explicit observations and fail conditions in the protocol. Remaining runtime power implementation and the final 43-app automation must be completed before physical acceptance.


## Board transactions and controls

- **134 SDK tests passed** (two existing doc examples ignored), including whole-run undo, undoable reset/clear, protected givens, atomic invalid restoration/edits, redo preservation and both move/count memory bounds.
- Undo/Redo/Clear keep identical rectangles across empty, played and undone states at every supported profile, three interface sizes and both orientations. Only available operations are actionable.
- SDK Clippy with `-D warnings` and Rust 1.85.1 ARMv7 musl compilation passed.

The board model bounds dimensions to 64 × 64 and history to 64 moves/8,192 cell changes. It does not persist by itself. Larger-board viewport, clue surface and game-specific catalog adoption remain open.

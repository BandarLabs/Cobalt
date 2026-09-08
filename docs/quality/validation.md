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


## Failure injection through production task policy

- **116 policy and 65 simulator tests passed**. Injected offline/timeout/missing-secret failures retain header and credential authority checks, task capacity/ID ownership, normal callback delivery and local file/timer work. Stream close remains local cleanup even while offline.
- Runtime suites passed: **149 binary + 16 library tests**. Device and host runtime now share capacity/duplicate refusal semantics with the simulator. Duplicate requests do not manufacture another completion for a live ID; queued immediate refusals retain ownership until drained.
- Strict Clippy for policy, simulator and runtime with `device-write` passed. ARMv7 musl Rust 1.85.1 runtime compilation passed (two subsequently removed unused imports were reported in that compile).
- Full extra-large [Panels simulator journey](evidence/simulator/task-fault-reader-result.json) passed, including raw input, held/failed panel refreshes, task cancellation, failed position saves, retry and forced process exit/reopen.

SIM-09 retains its open status while the remaining full-runtime/app-install failure paths are completed. Injecting a transport fault does not fabricate valid credentials or replace earlier validation errors. No task protocol variant or dependency was added.


## Physical refresh observations and retained completion ownership

The display HAL records `cobalt.refresh-observation` v1 for actual submit/wait calls: per-session sequence/monotonic time, marker, backend, requested/applied intent, region/full flag, submitted and driver-returned waveform, operation duration and failure errno. Completion records link to the submitted marker. Failed submits do not invent translation or completion. Failed waits retain pending ownership for recovery; the synchronous refresh path now retains that ownership too.

- **158 HAL tests passed** with `device-write`. New cases cover bounded observation retention, read-only snapshots, failed submit against `/dev/null`, failed waits/retry ordering and no duplicate completion. Existing backend/intent mappings, conservative color flags/downgrade, profile/firmware gates and grayscale/inversion behavior remain covered.
- **202 synthetic test log records** parsed as JSON across two independently correlated sessions. This validates the serialization and session boundaries, not any physical latency.
- HAL/runtime Clippy with `-D warnings` and Rust 1.85.1 ARMv7 musl runtime compilation passed.

`KOBO_FRAME_TIMING=1` enables JSON on stderr before runtime launch. Timing smoke rows also include markers, and runtime frame lines identify backend, requested/applied intents and translated waveform. The ring retains 128 observations and explicitly counts dropped records. No new waveform, inversion flag, color coefficient or device support was enabled. Completion ioctl success is distinct from measured visible ink settling; Clara BW measurements remain pending.


## Durable storage acknowledgements and failure parity

Records and final shelf chunks now require the file flush, rename and parent-directory flush to succeed before reporting success. Creating app directories confirms their parent entries before creating children. Removal reports unlink and directory-flush errors; retry still flushes when the name was already removed. A failed flush can leave the new value visible, so it reports uncertainty rather than promising rollback. Nonfinal shelf chunks acknowledge upload progress, not a completed durable file.

- **124 policy and 66 simulator tests passed**. Fault cases preserve published and partial files, enforce key/offset/size validation before disk-full injection, permit reads/removal, and prevent failed cache eviction from exceeding its allowance. A real IPC test checks correlated policy errors and confirms refused writes create no directories.
- Policy/simulator strict Clippy and Rust 1.85.1 ARMv7 musl policy compilation passed. The full extra-large [Panels recovery journey](evidence/simulator/durable-storage-reader-result.json) passed, including failed saves, retry and forced exit/reopen. The subsequent cache-unlink guard is covered by policy tests.
- Simulator storage logs retain request IDs and outcomes without record values or shelf bytes.

SIM-09 and HW-11 remain open: app-install failure parity and a runtime suspend barrier still require implementation. Host filesystem tests do not establish durability on a physical reader; that remains part of the combined Clara BW run.


## Explicit board geometry, clues and accessible large-board navigation

- **262 UI, 92 protocol and 137 SDK tests passed** (two pre-existing UI tests and two doc examples ignored). This includes one separately run three-number clue regression after the full suite. The board fixture checks every supported profile, three interface sizes and both orientations. Every square of a 64 × 64 board is reachable without renumbering or modifying it. Refused refits preserve the prior window; resizing/rotation reveal the selected square.
- Pixel checks distinguish all six marks with selection/given states and keep oversized content inside its square. Native square grids also keep their original column count on narrow panels. Wire tests cover every truncated prefix, invalid dimensions/counts/flags/marks, duplicate actions and version gating. Existing version-13 grid payload bytes are unchanged.
- **66 simulator + 16 runtime library + 149 runtime binary tests passed**. UI/protocol/SDK/simulator strict Clippy and Rust 1.85.1 ARMv7 musl SDK/runtime compilation passed.
- The original extra-large Clara BW [SDK IPC journey](evidence/boards/result.json) passed selection, horizontal/vertical panning, resizing, complete clue inspection and return. Captures have atomic provenance; [selected mark](evidence/boards/selected.png), [complete clue](evidence/boards/clue.png) and [larger squares](evidence/boards/larger.png) were visually inspected. These captures use ideal pixels and do not claim measured panel residue.

SDK-14/15 are complete as shared contracts. Catalog adoption remains in PR 2. Candidate editing and puzzle rules remain app responsibilities; this fixture is not a playable shipped puzzle. The SDK/runtime wire version is 14, with 11–13 decoder compatibility; install/version coordination remains in the foundation runtime work. No external dependency was added: the simulator example uses the existing local SDK as a development dependency.


## Exact light restoration and crash handback

Front-light restoration now writes and verifies the captured raw brightness and warmth, avoiding loss from percentage rounding. Driver ranges must still match. A failed warmth write does not prevent the brightness attempt, and guardian screen restoration still runs if light restoration fails. Runtime creates a private session directory and flushes a bounded light record before arming its watchdog and stopping the reader. Recovery validates that record and the normal hardware write gate before restoring the light and restarting the stock reader.

- **18 front-light/recovery and 10 guardian tests passed**, including low raw values, changed ranges, malformed/path-escaping records, a deliberately killed fixture child, and independent screen/light failures.
- **150 runtime binary and 16 runtime library tests passed**. Failed light recovery retains its session record while allowing the reader to restart; a successful recovery clears it. Strict Clippy and Rust 1.85.1 ARMv7 musl runtime/guardian compilation passed.

The guardian can restore after its child exits; this does not claim recovery if the guardian itself is killed. Runtime crash recovery uses its independent watchdog. A retained failure record is diagnostic evidence, not an automatic instruction to overwrite settings once the reader is running. Owner-setting/suspend integration and physical Clara BW validation remain open under HW-12.


## Signed local Store transactions and publishing compatibility

The simulator's explicit `KOBO_SIM_APP_STORE` mode loads a private test trust key and local transport files, then calls `kobod::app_store` for catalog verification and install/update/remove. It never fetches a fixture URL from the network or replaces device trust. Invalid signatures/hashes and incompatible runtime requirements fail before injected storage failures; failure preserves the current verified installation. A simulated full disk uses the same bounded error as an actual transaction I/O failure. The normal catalog preview remains available and is explicitly marked `catalog-preview`, with `signedTransactions: false`, in simulation/capture metadata.

- **70 simulator, 16 runtime library, 150 runtime binary, 18 Store and 92 protocol tests passed** on Cobalt 0.3.12. New cases cover signature/hash corruption, version gates, authority before faults, failed refresh cache retention, fixture-key/path restrictions, restart, removal and owner-data preservation.
- **20 driver tests passed**, including text assertions across visible line wraps without inventing clipped words. Store tests require a fresh installed-state read after an uncertain write, with no false rollback/success claim.
- The real Store [SDK IPC journey](evidence/store/result.json) passed install, failed update, retry, process restart, removal/data retention and reinstall at extra-large size on Clara BW simulation. [Available](evidence/store/available.png), [failed update](evidence/store/failed.png), [updated](evidence/store/updated.png) and [removed](evidence/store/removed.png) have atomic JSON provenance. The fixture payload is intentionally inert; these are transaction checks, not app launch or physical hardware validation.
- Strict Clippy for simulator/runtime/Store/CLI with all targets and runtime device-write passed. Rust 1.85.1 ARMv7 musl runtime/Store compilation passed.
- **64 publishing-policy tests passed**. Protocol 14 maps to Cobalt 0.3.12, the prepared workspace version. Existing protocol 11–13 decoders remain available. Historical release exemptions are tested only when their protocol is active, matching production selection; no changed SDK blob received a new exemption. Catalog app version/release-note changes remain part of PR 2.

Create an original fixture with `cargo build -p kobo-sim --example signed-store`, then `target/debug/examples/signed-store init /tmp/NEW-PRIVATE-FIXTURE 1.0.0`. The root must be new. Set `KOBO_SIM_APP_STORE` to that directory when running `kobo dev` from `examples/store`. The example's `publish` command changes the local catalog for update tests; it does not publish a release. `python3 scripts/quality/check-store-sim.py --output /tmp/NEW-EVIDENCE` creates and cleans its own fixture and processes.


## Real launcher transitions, font ownership and callback measurements

`kobo dev --runtime [address] [--apps todo,store,magnet,tictactoe]` builds and hosts the selected local SDK binaries with the real launcher. The device runtime and simulator share Back ownership, the two-second app-owned Back deadline, and eviction selection. Repeated Back taps cannot extend the first deadline. The launcher and foreground process remain protected, and the host retains at most four app processes. Background screens are retained without submitting a panel refresh; the one panel history moves with the foreground app.

- **138 SDK, 263 UI, 72 simulator, 19 Store, 20 runtime library and 150 runtime binary tests passed** (two existing UI tests and two SDK doc examples ignored). Strict Clippy passed for CLI/simulator/runtime/SDK/UI/Store with all targets and device-write. Rust 1.85.1 ARMv7 musl runtime/Store compilation passed.
- The extra-large Clara BW [real-process journey](evidence/runtime/result.json) passed shell Back, Store-owned Back, retained process/state, eviction, shared panel history, app death, launcher return and durable state after relaunch. Only processes created by the fixture were killed. [Game controls](evidence/runtime/fourth-app.png), [Store detail](evidence/runtime/store-detail.png) and [returned state](evidence/runtime/after-relaunch.png) have atomic capture provenance. Game and Store captures were visually inspected.
- SDK callbacks now measure at the scale supplied by the runtime and restore their caller's environment. The actual catalog is paginated with the same measured section/banner spacing that is rendered. The complete catalog, including result notices, fits Clara BW and Elipsa at default, large and extra-large sizes. Square grids can use the available height while keeping every original column, square cell, minimum physical target and trailing control; the regression covers every supported profile and both poses.
- Fonts now have bounded app-local ownership in both hosts. The same local handle in two apps resolves to separate renderer handles. Invalid fonts consume no slot, a refused replacement retains the previous face, and release/eviction frees the owner's fonts. Each app is limited to 16 valid faces.

Run `python3 scripts/quality/check-runtime-sim.py --output /tmp/NEW-EVIDENCE` for the isolated journey. This is real host SDK IPC with shared runtime policies, not Linux sandboxing, kernel suspend or measured hardware timing. Signed Store transactions remain in the separate single-app fixture: combining that inert payload fixture with local runtime builds is explicitly refused, rather than presenting local binaries as signed installed packages. Uninstalled catalog apps cannot be reopened from a retained process. A background notification remains asynchronous and is not a durable-save barrier.


## Save barriers and simulated power entry

The shared power coordinator has explicit awake, preparing, ready, suspended and handback states. A nonzero generation identifies one attempt and its exact hosted app set. Only that set's matching acknowledgements can satisfy it. Worker cancellation/drain and confirmed panel completion remain separate requirements. Save refusal, a five-second monotonic preparation deadline, USB/charging changes and new input abort an attempt; a wake invalidates an outstanding entry decision. An unsupported backend selects reader handback rather than claiming kernel sleep.

Protocol 14 adds bounded, version-gated prepare, readiness, resume and targeted scheduled-occurrence messages. The SDK's default suspend callback invokes the existing background-save hook; apps with additional drafts override `on_suspend`/`can_suspend`. Readiness waits for all device/store replies and task outcomes, including chained saves. A storage denial refuses the attempt. Repeated/stale preparation and resume messages cannot repeat callbacks. Scheduled work has its own occurrence ID and is delivered only to the requesting app; a general resume does not schedule every hosted app. `TaskRunner::pause` cancels current workers, reports their normal outcomes once and prevents new workers; resume opens admission without replaying cancelled requests.

- **125 policy, 93 protocol, 141 SDK, 72 simulator, 24 runtime library and 150 runtime binary tests passed**; two existing SDK doc examples ignored. Strict Clippy passed for those packages with all targets/device-write. Rust 1.85.1 ARMv7 musl SDK/runtime compilation passed.
- The original [real SDK power journey](evidence/power/result.json) passed a full-disk save refusal, two ordered durable writes, cancellation exactly once, duplicate scheduled wake, percentage-light restoration, panel-busy deadline and USB entry/wake rules at extra-large Clara BW size. [Failed save](evidence/power/failed-save.png), [completed barrier](evidence/power/asleep.png), [resumed](evidence/power/resumed.png) and [USB wake](evidence/power/usb-wake.png) have capture provenance. The resumed fixture was visually inspected. The actual launcher/app eviction/crash journey passed again after these changes.

Build with `cargo build -p kobo-cli -p kobo-launcher` and `cargo build -p kobo-sim --example power`; run `python3 scripts/quality/check-power-sim.py --output /tmp/NEW-EVIDENCE`. In multi-app mode, `GET /power` exposes state and `POST /power` accepts `sleep`, `sleep cover`, `sleep idle`, `wake`, `wake cover`, `wake touch`, `wake scheduled`, `usb attach` and `usb detach`. These are explicit fixtures, not physical measurements. Interface input wakes without activating the underlying control. Signed transaction fixtures remain separate.

This is partial progress on HW-09/11/13/14/15. The device host does not yet initiate these barriers or enter kernel suspend. Native power/cover routing, radio/watchdog ownership across suspend, scheduled hardware wake and physical calibration remain open. The simulator currently models entry and frontlight state; it does not freeze host processes or reproduce a kernel/radio power transition. No device command, kernel power write or third-party source was used for this change.

## Full CI integration and measured catalog layouts

- Rust **1.85.1** full workspace/all-target/all-feature run: **3,108 passed, 2 existing ignored** across 105 test binaries. Strict workspace Clippy and formatting pass with the same pinned toolchain. The full run precedes the final mechanical package-version metadata increments; those are separately checked against the published catalog below.
- Direct `Context` pagination, line clamping, row and tile measurement now scope the reader's interface size themselves. Detached context tests preserve every word and verify rendered prose pages fit at every text scale. Pub Quiz's existing large-question/answer accessibility regression passes.
- `Layout::content_used` records the actual flow height, including spacing after section labels. `Context::paginate_rows_under` measures a built heading/filter/notice prefix before placing rows. Fieldbook's sighting picker reserves notices, page controls and the fixed tally/count actions; every species remains reachable across all text scales on Clara portrait/landscape and a smaller 212-PPI panel. Nonograms similarly pages its complete puzzle list and pins help/photo controls.
- Pictures preserve aspect ratio within the available height and reserve trailing controls. An empty game grid has no expected painted rectangle, so it no longer causes a false hidden-content diagnostic. Nonempty clipped controls remain errors. UI suite: **265 passed, 2 existing ignored**.
- Grimoire uses paged, named filter choices and separate result pages. `Any` is always the first choice, class tags split into individual names, and changing edition/category resets incompatible filter indices. Filter options are computed once per result scan. **13 Grimoire tests** cover exact selection, Back/Clear, complete result indices, and reachable choices across every text scale. **24 Nonograms and 4 Fieldbook tests** pass.
- Actual simulator journeys pass for Backgammon, Fieldbook, Frame, Grimoire and Nonograms. Grimoire's revised route explicitly turns the class-choice page before selecting Wizard, then selects Abjuration and opens the filtered results. The committed-source sweep at `532684c` passes **43/43 in-scope apps: 36 committed interaction routes and 7 launch-only checks**. [Results](evidence/layout-integration/results.json) and selected captures are retained alongside them. Capture provenance marks the tree dirty because of generated runtime test fixtures under an untracked target directory; the source revision and binary hashes are recorded.
- ARM Rust 1.85.1 cross-target checks for the runtime, Fieldbook, Grimoire and Nonograms pass. The existing platform-conditional unused-code warnings remain; this was a compile check, not physical hardware execution.
- **104 publishing-tool tests passed.** The package version gate passes against the downloaded beta catalog after verifying its provenance and SHA-256: publication source `a3a96768d83ff8f93ea98c3ee807d96d0b3282e8`, catalog hash `77f34eaaed84ff24ee709931540d998a1b01cdb21ab424cc3f8ae00d8c623610`. All affected packages receive a new version for the changed shared SDK. Zotero Reader receives only mechanical package metadata; its app code remains outside the quality review. No compatibility exemption was introduced.

This checkpoint completes BACK-06, GRIM-01 and GRIM-02 as foundation integration fixes. The other catalog tasks, remaining native power integration and companion workflows are still tracked as open. CBR remains deferred.

## Correlated record-load failures

`KoboApp::on_load` receives the requested record key even when storage refuses a load without returning a key on the wire. Existing apps retain their `on_store` behavior by default. A failed library load can retry while another record and a list request remain outstanding without consuming either answer. Rust 1.85.1 SDK tests: **143 passed**, two existing doc examples ignored; strict SDK Clippy and formatting pass. Panels adopts this callback in the catalog work so a failed library read cannot appear as an empty shelf.

## Repeated imports preserve existing files

The shared import flow checks the content-addressed shelf before writing. A complete, verified identical file is reused, including when the owner cancels or the receipt save fails. Only a missing file or a completely read, mismatching partial copy starts a transfer; storage refusals and invalid read responses do not become permission to overwrite. Retrying an interrupted write repeats this check. Post-write verification still rejects corruption, and reopening a missing receipt target remains unavailable. The upload buffer is released before post-write readback.

Rust 1.85.1 SDK tests: **145 passed**, two existing doc examples ignored. Tests use the actual policy shelf with a multi-chunk original and cover duplicate cancellation, receipt failure, read-only probe refusal, partial repair, corruption and missing-file reopening. Strict SDK Clippy passes. No dependency or wire-format change.

## Catalog checkpoint: acknowledged Panels imports and comic shelf

Panels now adopts the shared import preview, verified content-addressed shelf copy and saved receipt. The success screen also waits for its comic-list save; leaving the success screen keeps the imported comic available on the shelf. Failures retain the pending record and offer a retry. Cancellation drains an outstanding response before admitting another import, and repeated selection cannot relabel an earlier comic's incoming bytes.

The bounded 64-comic library uses a strict versioned record with acknowledged migration of valid legacy rows. Corrupt, unreadable and future records do not become empty libraries or get overwritten. List pages reserve the actual notice and navigation heights at every text scale. Reading-position keys remain valid for full content-addressed names and restore existing legacy positions when available. Completed remote downloads retain their recovery data until the library write succeeds; broader remote interruption coverage is still open.

Validation with Rust 1.85.1: **19 Panels tests pass**, strict app Clippy and formatting pass, ARM cross-target check passes, and the publishing version gate passes against the previously verified beta catalog. App/manifest versions are 0.1.2 and the generated app page is current. The actual Clara BW simulator journey at extra-large text passes preview, receipt, reading controls, page keys, storage-full failure/retry and forced process restart with position, zoom and direction restored. A separate CBR fixture is refused with visible CBZ recovery guidance. The script compares rendered text across wrapped lines. [Captured results and screens](evidence/panels-import/) record the tested binary hashes; captures include uncommitted catalog changes and are marked dirty. Preview, receipt, shelf and CBR refusal images were visually inspected.

This completes COMIC-18 through catalog adoption. Remaining Panels tasks include editable server setup, owner-facing samples, shelf thumbnails/progress and full interrupted-download verification. The wider app and companion program remains in progress; no physical-device validation has been performed.
## Catalog connection response admission

The shared OPDS entry point now rejects HTML sign-in pages, XML error roots, incomplete/mismatched XML roots and JSON without catalog fields. Valid empty catalogs remain valid. Atom namespace prefixes no longer hide a catalog title. This prevents a successful HTTP response from being reported as a successful library connection merely because it begins with `<` or `{`.

Rust 1.85.1: **58 OPDS unit tests and one Atom/JSON parity test pass**; strict OPDS Clippy passes. Tests include login/error documents, truncated roots, a second document root, legitimate empty catalogs and an alternate Atom prefix. Panels' catalog connection test exercises this distinction in the app integration. No dependency or public wire change.

## Separate account fields and readable keyboard controls

HTTP Basic setup now collects username and password separately and stores the combined value only through the runtime's private-secret request. Password entry preserves leading/trailing spaces, including a whitespace-only password; ordinary text entry retains its existing trimming behavior. Account keys also use verbatim private entry. Cancellation clears the pending username and input, and neither password contents nor pending credentials appear in the screen or setup debug representation.

Account entry uses a fixed Back control and places the keyboard beneath the entered value. The existing Shift and delete actions now use original vector symbols, retaining semantic labels and stable action IDs. Their glyph tags are appended as 66/67 to the prepared protocol-14/Cobalt-0.3.12 runtime; they do not renumber earlier glyphs or require another dependency. The bundled body font lacks these Unicode symbols, so rendering does not depend on a host-font fallback.

Rust 1.85.1: **148 SDK, 265 UI and 93 protocol tests pass**, with two existing UI and two existing SDK doc examples ignored. Strict Clippy passes for all three crates. Account prompt, username, password, key and saving screens fit every text-size step on all declared portrait profiles plus a 758×1024/212-PPI test geometry. Password-space preservation, cancellation, invalid Basic usernames and redaction are tested. The initially chosen 600×800/212-PPI stress geometry was not a declared reader profile; portrait account tests now use declared profiles and the stated additional geometry. Landscape account-entry validation remains separate work.

## Native credential-save parity

The native app host now handles credential saves through the same private, durable writer as the simulator and file-rendering host. Previously, the native path could fall through to a generic service success without installing the secret. The generic service now refuses an unhandled credential save instead of reporting success. The shared handler preserves the exact app/name allowlist and returns failure when the private directory cannot be written; an unrelated app cannot install a credential.

App-entered credentials also retain exact whitespace when the task runner reads them. Legacy owner-managed credential files keep their existing whitespace/newline trimming. Rust 1.85.1: **127 policy and 72 simulator tests pass**, strict policy/simulator/runtime Clippy passes, and the ARM runtime check passes with its existing platform warnings. The regression checks acknowledged bytes, refused app identity, unwritable storage and preserved owner-file contents. This is host testing and cross-compilation, not physical reader execution.

Panels' new configurable-server flow remains uncommitted catalog work: its attempted account save was correctly refused by the current allowlist, and its network policy still admits only the historical fixed root. Binding a saved credential to the selected server is required before that flow is complete. No general destination allowance was added.


### Server-bound account storage (2026-09-08)

Protocol 14 adds a server-account request. The runtime writes the selected HTTPS
server and exact account value in one private, durably acknowledged record.
Panels/Komga is the only enabled provider. Requests must remain within that
origin, port and base path, use the approved Basic header and fetch method, and
pass host authorization. Corrupt records fail closed without falling back to a
legacy account. Authenticated redirects remain refused by the transport.

The SDK collects separate username and password fields, refuses oversized combined
values before sending them, waits for the matching save acknowledgement, and
keeps the old computer instructions hidden for scoped accounts until a supported
companion flow exists. Native and simulated hosts use the same installer.

Validation: 130 policy, 94 protocol, 149 SDK and 72 simulator tests passed (445
total; two existing SDK doctests ignored). Strict Clippy passed for these crates
and kobod with all targets/features. ARMv7 musl kobod check passed with the 118
existing platform warnings. No hardware commands were run. Logs:
`/tmp/cobalt-bound-account-tests.log`, `/tmp/cobalt-bound-account-clippy.log`,
`/tmp/cobalt-bound-account-arm.log`.


### Restore original USB setup preferences (2026-09-08)

USB setup now durably records the owner's original Wi-Fi and sleep values before
changing the reader configuration. Repeated setup preserves the first record.
Undo restores only values that still match Cobalt's applied setting, retaining
later owner edits and unrelated preferences. Undo runs restoration before payload
removal. Older installations without a record keep their current settings; the
original values cannot be recovered by guessing. Corrupt/future records and
failed record writes refuse changes. Both records and configuration updates use
file flush, atomic replacement and directory flush.

All 55 setup tests passed, including original-value restoration, repeated setup,
owner changes, missing legacy records and failed/corrupt backups. Strict CLI
Clippy passed with all targets/features. Logs: `/tmp/cobalt-owner-settings-tests.log`
and `/tmp/cobalt-owner-settings-clippy.log`. HW-12 still awaits native power
integration and the planned physical validation.


### Native save barrier and power-button edges (2026-09-08)

Native power-button release and idle expiry now enter the shared generation-scoped
power coordinator. Hosted task admission pauses, SDK save barriers run, and reader
handback waits for all app acknowledgements, drained workers and the panel fence.
Save refusal, touch, cover changes and hosted-set changes cancel preparation.
The native path explicitly selects reader handback; no kernel suspend backend is
enabled before physical profile/firmware validation. The existing guardian,
frontlight restoration, reader restart and watchdog recovery remain the owners of
teardown. Native USB/scheduled-wake/kernel integration remains unfinished.

App repaints no longer renew the owner-activity idle timer. An open terminal and
charging block automatic preparation. A failed attempt waits for a later idle
period instead of spinning. Physical power-button edge handling is shared with
runtime simulation: duplicate presses/releases and releasing a wake press cannot
start a second attempt. The native input decoder exposes read-only quiescence and
marks it unsafe if its reader thread ends.

Validation: 163 HAL tests, 25 runtime-library tests and 151 runtime-binary tests
passed. Strict HAL/runtime/simulator Clippy and the device-write ARMv7 musl runtime
check passed. The extended multi-app simulator journey passed save failure,
chained saves, cancellation, scheduled wake, panel deadlines, USB wake, light
restoration and duplicate button edges. Its evidence explicitly says
hardware_validation=false. Logs: `/tmp/cobalt-native-power-final-tests.log`,
`/tmp/cobalt-native-power-final-clippy.log`, `/tmp/cobalt-native-power-arm.log`.
Evidence: `evidence/native-power/`. Remaining HW tasks are not marked complete
from these partial native integrations.


### Shared export receiver and UI copy (2026-09-08)

SDK-19 and SDK-23 are implemented. `exports::Export` prepares owner-selected text,
Markdown, PNG or JPEG content with a verified copy and acknowledged offer.
`kobo export --app APP (--device ADDRESS | --sim) --out FOLDER` uses the existing
SSH identity/host verification or isolated simulator storage. It bounds reads,
verifies size/hash, flushes completed files and publishes without replacement;
repeated identical receiving also retries file/directory durability checks.
Conflicting names get a suffix. Computer failures never delete reader content.
The reader reports only local readiness, not remote receipt. No new service,
network listener, dependency licence or pairing scheme was introduced.

Shared account/error text no longer assumes computer-only setup, guesses an
outage, or calls a missing requested item an empty library. Three compatibility
assertions were updated for changed shared wording: Audiobook, RSS, and one
Zotero Reader account-button assertion. Zotero Reader's implementation and review
scope remain untouched. The export/copy contract is in `sdk-export-and-copy.md`;
app-specific adoption remains in the catalog and companion checklists.

Validation: the full workspace all-feature run had 3,135 passing tests and one
stale account-button assertion. After updating that assertion, its 25-test target
passed; all **3,136 distinct workspace checks** therefore pass, with four existing
ignored tests/doc examples. Strict workspace Clippy passes for all targets and
features. The actual SDK/launcher/simulator-to-CLI journeys pass for original
text and PNG fixtures at Clara BW extra-large text: no availability before owner
confirmation, full-storage failure/retry, exact receiving, duplicate reuse and
corruption preserving the existing computer file. Updated ready/preview/failure
screens were inspected. No hardware command or SSH transfer was executed.

Logs: `/tmp/cobalt-foundation-export-workspace-tests.log`,
`/tmp/cobalt-sdk-account-compatibility.log`,
`/tmp/cobalt-foundation-export-workspace-clippy.log`.
Evidence: `evidence/exports/result.json` and paired captures. Beta was fetched
again and remains `7f1a543aa432248f45a69db186e1d6b85c888b17`.


### Foundation completion: validated handback and failure parity (2026-09-08)

SIM-09 and HW-09–16 now have implementation and repeatable host evidence. Earlier
entries describe intermediate work; the final native route is a save barrier
followed by normal stock-reader handback. It deliberately does not enter kernel
suspend or advertise RTC wake. Automatic sleep-cover polarity is not inferred
from a raw magnet event. Radio and watchdog ownership use existing teardown;
physical behavior and calibration remain acceptance work after all three PRs.

Native power observation discovers supply types and reads bounded status/online
values, retaining unknown state instead of guessing from a driver name. A cable
power observation is not a claim about USB mass-storage ownership. The host
cancels preparation with USB/charging wake reasons. These fields follow the
[Linux power-supply ABI](https://www.kernel.org/doc/Documentation/ABI/testing/sysfs-class-power).

Every CLI wake acquisition now carries a two-minute expiry; a running hold
renews it every thirty seconds even when already held. Computer loss therefore
does not depend on a later SSH release or reboot. This uses the documented
[nanosecond wake-lock timeout](https://www.kernel.org/doc/Documentation/ABI/testing/sysfs-power),
with no copied reference implementation. A shell fixture verifies the emitted
write for both absent and already-held locks. Actual kernel expiry is still a
Clara BW acceptance measurement.

Validation: 130 policy and 73 simulator tests pass for account fault ordering;
310 CLI, 164 HAL, 25 runtime library and 151 runtime binary tests pass for the
power/lease changes. Strict all-target/all-feature Clippy for these five crates
passes, as does Rust 1.85.1 ARMv7 musl runtime compilation with device-write.
The [real SDK journey](evidence/power-completion/result.json) additionally passes
cover bounce during panel-held preparation, charging refusal/wake and USB
reconnection. Captures include source/fixture provenance and remain explicitly
simulated. The charging-wake screen was visually inspected at extra-large size.
No physical reader or third-party source code was used.


The native barrier also checks every hosted app's negotiated protocol before
pausing any runner. An older app is refused without sending it an unsupported
save request; the simulator uses the same fixed protocol floor. This check is
covered by the native Unix-socket fixture. A fresh catalog sweep at `3b0dc79`
passes **43/43 apps** (36 committed interaction routes, seven launch-only checks),
excluding Zotero Reader. This sweep predates only the native barrier-version guard.


### Panels: server setup, import guide and original sample (2026-09-08)

Panels now guides a Komga connection on the reader. The HTTPS address is stored
through acknowledged, versioned settings; account entry uses the shared
server-bound credential contract. A failed address save blocks connection
checking until retry succeeds. Only a valid OPDS result opens browsing; login
pages and generic error responses stay in setup. The manifest and generated app
page now describe these implemented steps instead of a fixed-server CLI secret.

A missing local file opens four short USB steps. They explain the actual folder,
filename, size limit and safe ejection. The bundled four-page *A small garden*
uses original drawing geometry and captions, with a deterministic generator and
the repository's existing font. It enters the same preview, verified-copy,
receipt and library-save flow as an owner file. No external comic/artwork or
reference-project implementation is included.

**24 Panels tests pass**, including all portrait text scales on Clara BW and
758×1024 at 212 ppi; the guide was shortened after a largest-scale overflow was
caught. Strict all-target Clippy, ARMv7 musl compilation and the published-catalog
version gate pass. The [full extra-large simulator journey](evidence/panels-onboarding/result.json)
passes account/address restart, USB steps, full-storage sample retry, reading and
forced-exit restore, plus the existing zoom/pan/RTL/spread and durable-position
journey. [Sample page](evidence/panels-onboarding/35-sample-first-page.png),
[USB folder step](evidence/panels-onboarding/31-import-guide-folder.png) and
[restored server](evidence/panels-onboarding/25-server-reopened.png) were visually
inspected. Captures include binary/source provenance. No live Komga service or
physical reader was contacted; network admission is covered by policy/app tests.

PANELS-02 and PANELS-03 are complete. Shelf thumbnails/progress and validated
interrupted-download recovery remain open (PANELS-04 and PANELS-07); the current
remote partial-file flow still needs stronger persistence and identity checks.


### Panels: reachable server catalogs and Back restoration (2026-09-08)

Large Komga responses now use measured local pages, with bounded title/summary
previews and reachable server pagination links. Search preserves each original
publication/section action. Nested navigation retains the local page and query;
Back cancels outstanding work so a late server response cannot replace the
restored page. History is bounded to 16 responses. The Search glyph keeps its
accessible label and fits where the text action overflowed at maximum scale.

**26 Panels tests and strict Clippy pass.** A 128-book original OPDS fixture checks
all 129 actions, including the server-next link, across every text scale on
Clara BW portrait/landscape and 758×1024/212 ppi. It checks both ends of the local
pages, filtered original IDs and late-response refusal after Back. This addition
uses app/policy tests rather than a live network library. No extra todo is marked
complete: PANELS-04 and PANELS-07 remain open.
## Public documentation follow-through

README, SDK.md and the public SDK page now document the export receiver, verified
copy lifecycle, acknowledged drafts, server-bound accounts and shared CBZ reader.
The public SDK page uses the actual committed export fixture screenshot with its
simulator limitation stated. Device session instructions now explain the timed
two-minute wake-lock lease, renewal and stock-reader suspend limitation. Simulator
residue is described as a model awaiting physical calibration. API names and CLI
lease behavior were checked against source; local image paths and diff whitespace
were checked. No code or physical-reader behavior changed in this documentation
update. Beta was fetched again and remains 7f1a543.

## Panels download recovery and public screenshots

PANELS-07 is complete with fixture-based validation. Recovery now saves a
versioned size/SHA-256 checkpoint only after its bounded shelf upload succeeds.
Two alternating blobs keep the previous checkpoint intact across an interrupted
metadata save. An initial failed metadata save prevents fetching. Retry writes
pending storage before requesting more bytes. Restart verifies the exact saved
blob and compares its prefix against the server before appending. Changed server
content, corrupt/newer/legacy records and missing bytes stay explicit recovery
states; none becomes a silently empty download. A complete checkpoint can be
imported offline through verified copy, receipt and library acknowledgements.
Content-derived copies preserve older comics at the same URL and reuse identical
copies. Removal drains outstanding callbacks, acknowledges clearing metadata,
then removes only owned recovery blobs; cleanup failures remain retryable.
Suspension pauses fetching and retains pending checkpoints until storage settles.

Validation on the catalog worktree:

- 36 Panels tests pass, including nine SDK/policy filesystem recovery journeys,
  bounded transfer/checkpoint cases, and recovery controls at every portrait text
  size on Clara BW and 758 × 1024 panels. Original fixtures replace network
  responses; this is not a live-server interoperability test.
- Strict all-target Clippy and ARMv7 musl compilation pass with Rust 1.85.1.
- Published-catalog version gate passes for Panels 0.1.2.
- Actual simulator recovery at extra-large Clara BW size passes after the final
  duplicate-copy cleanup fix. It checks completed offline import, full-storage
  failure/retry, receipts, removal of temporary files and forced-restart reading.
  The capture asserts zero fetch/post effects and read-only screenshot sampling.
  Evidence: [recovery result](evidence/panels-recovery/result.json).
- The full actual SDK comic journey also passes with server/account restart,
  USB guide, sample import, storage retry, reading positions, zoom/pan, RTL and
  spreads. That run predates only the final identical-copy cleanup guard, covered
  by the subsequent recovery tests and simulator run.
  Evidence: [journey result](evidence/panels-docs/result.json).

The public app screenshot, canonical app screenshots, app README, generated app
page and SDK comic example now show actual current interface captures. Each
image links to its source/binary/profile/font/scale provenance. These captures
use original artwork and ideal simulator frames, not calibrated physical e-ink
appearance. Public SDK/CLI documentation also now explains verified exports,
server-bound accounts, durable state and timed session leases in PR 1.

Limits remain explicit: there is no HTTP version-token API, so resuming re-reads
the saved prefix; no personal Komga service or physical reader was used. Shelf
thumbnails/progress (PANELS-04), other catalog tasks and companion work remain
open. Combined checklist: 494 tasks, 102 complete, 391 open, CBR deferred.

## Panels covers and saved shelf positions

PANELS-04 is complete. The shelf now shows bounded cover thumbnails and the last
acknowledged page. Positions are read through the shared comic parser, with
missing state distinct from damaged/newer records. A per-write snapshot prevents
an earlier save acknowledgement from displaying a newer unsaved page. The final
page is still a page, not an assertion that the owner finished reading.

Covers are generated on import/open and kept in the SDK's evictable namespace.
Only visible cached covers and small position records load on the shelf; there
is no sweep decoding every archive. Invalid or evicted covers fall back to a book
icon in the same column. Covers are at most 160 × 240 grayscale pixels, with 32
live display handles; leaving a shelf page releases its pictures. Opening a comic
regenerates a missing cover. Existing uncached books get covers when opened.

The new shared cover-row clamping/pagination helpers reserve the renderer's wider
picture column. Two-line shelf titles, cover/fallback geometry, summaries and
notices are checked together, including long RTL rows on small panels.

Validation: 41 Panels tests, strict all-target Clippy and ARMv7 musl compilation
pass. Shared runs pass 20 comic, 17 bookview, 151 SDK and 265 UI tests; the existing
two ignored UI tests and two ignored SDK doc examples remain unchanged. The real
SDK simulator journey at extra-large Clara BW scale passes sample import,
acknowledged progress after forced restart, cache eviction without position loss,
cover regeneration, reader controls, save failure/retry, RTL and spreads. Every
capture checks diagnostics, provenance and zero network effects. The current
public app image, app README/screenshots and public SDK page now show the actual
updated shelf. Evidence: [simulator result](evidence/panels-previews/result.json).
Physical Clara BW acceptance still follows all three PRs.

All seven Panels checklist items are now complete at the fixture/simulator level.
The broader catalog and companion work remain open: 494 total tasks, 103 complete,
390 open, and CBR deferred. PR 2 has 11 complete and 257 open items.

## Sudoku: original puzzles, saved games and complete play

All seven Sudoku tasks are implemented on the catalog branch. The quality checklist now contains **110 done, 383 open and one owner-deferred CBR task**. PR 2 has **18 done and 250 open**; the companion group still has 133 open. This does not complete the wider catalog program.

Sudoku 1.0.11 replaces the single digit-shifted puzzle with 36 original puzzles, 12 per measured difficulty. The reproducible generator uses a unique-solution check, then classifies whether solving needs single candidates, single locations in units, or techniques beyond those two. A separately written Rust solver checks uniqueness and all rows, columns and boxes of every bundled solution. The Python classifier was also rerun against all 36 committed puzzles. No external puzzle corpus or reference-project source was used.

One bounded record preserves answers, pencil notes, selected square, orientation, checking preference, hint count and up to 64 undo steps. A Draft acknowledges only its released revision, holds newer edits through a failure and requires explicit retry. Invalid/future saves stay untouched. Checking is opt-in and never rejects an answer; reveals and restarts ask first and are undoable. A completed solution and its save status remain distinct.

The shared SDK/UI changes add selected keypad outlines, fitted short board marks, 3×3 spacing in six-row views and orientation-aware help pagination. Sudoku exposes rotation through View. Portrait keeps all 81 squares; landscape uses overlapping rows 1–6 and 4–9 with stable action IDs and retained focus.

Validation:

- **10 Sudoku tests pass**, including whole-pack uniqueness, immutable clues, persistent bounded undo, exact acknowledgement, failed-save retry, invalid records, completion/reopen and confirmation cancellation.
- Layout checks cover all **nine interface sizes** at **1072×1448 / 300 ppi** and **758×1024 / 212 ppi**, each in portrait and landscape. They include both landscape windows, the longest puzzle titles, all nine pencil notes, checking warnings, completion, save failure, confirmation screens and every help page. Unreadable-save screens are also checked in their startup portrait orientation.
- **152 SDK and 267 UI tests pass**, with two existing UI ignores. The keypad rendering test verifies selection changes only pixels inside its unchanged hit rectangle. Strict all-target SDK/UI and Sudoku Clippy pass.
- ARMv7 musl compilation and the published-catalog version gate pass. Beta was fetched again and remains `7f1a543`.
- The actual **Clara BW SDK simulator journey at extra-large text** passes note toggles, exact forced-restart restoration, persistent undo, storage-full preservation, explicit retry, unassisted wrong entries, optional checking, reveal confirmation, full puzzle completion, completion restart, new difficulty, portrait help, both landscape views, landscape restart and landscape help. It also runs the updated committed `apps/sudoku/drive.kobo` route. All captures assert zero fetch/post effects.

See [the simulator result](evidence/sudoku/result.json), [pencil notes](evidence/sudoku/02-pencil-notes.png), [save recovery](evidence/sudoku/05-save-recovery.png), [completion after restart](evidence/sudoku/10-completion-restored.png) and [landscape restoration](evidence/sudoku/16-landscape-restored.png). Captures include actual source/binary/font/profile provenance and accurately record dirty source during implementation. The app README, public app page, SDK page and canonical screenshots are updated. Physical Clara BW acceptance remains scheduled after all three PRs; these checks do not certify panel behavior on hardware.

### Follow-up board-app scale sweep

The shared board changes prompted a targeted simulator sweep. Crossword, Logic Pack and Nonograms pass their existing routes at default text size. At extra-large size, Crossword fails to find a letter key after a coordinate-based tap, and Logic Pack and Nonograms encounter renderer text-fit refusals. Parlor and Tic-tac-toe pass at extra-large size. These unresolved issues are recorded against CROSS-02, LOGIC-04 and NONO-01, without marking additional tasks done or attributing the failures to a particular commit. See [results and failure captures](evidence/board-regression/README.md). PR 2 remains a draft.

## Nonograms: attached clues, full-size boards and durable undo

Nonograms 0.1.5 completes its six original app checklist items. The review also exposed repetitive legacy stroke patterns, now tracked separately as NONO-07: replace the pack with varied original picture puzzles while preserving existing saved games. **495 tasks: 116 done, 378 open, one CBR deferral. PR 2: 24 done, 245 open. PR 3: 133 open.** This does not complete the broader catalog program.

The app uses the shared BoardViewport for attached row/column clues, complete clue inspection, overlapping panning and square-size controls. It supports all bundled sizes through 25×25 with absolute cell identities. Selected squares and their two matching clue gutters have an ink outline and shaded field. Portrait uses three control rows; wide displays use two. Controls are measured before allocating the remaining area to the board. The initial extra-large refusal recorded in `evidence/board-regression` is now resolved for Nonograms; Crossword and Logic Pack remain open.

Per-puzzle versioned records retain marks, preferences, recorded selection and 64 undo snapshots within 64 KiB. They validate the full answer digest. Single marks, whole runs and confirmed restarts undo atomically. Shipped mode-plus-marks records still open and migrate on the next edit. Corrupt, oversized and future records remain untouched. Progress and the solved index each wait for exact storage acknowledgements; failure preserves the latest in-memory edits and offers retry before leaving. Completion displays the actual solved grid, without the previous decorative stripe effect, and can be reopened through Undo.

Photo generation still refuses answers not fully determined by row/column deductions. Options display a documented pass-count difficulty guide; it is not a human-tested difficulty scale. Current companion imports remain 5, 7 or 9 squares, accurately stated in the app. Companion preview, named imports and simulator targeting remain in PR 3. The legacy bundled study pack is retained until NONO-07 provides a migration-safe replacement.

Validation:

- **30 app tests pass**, including every cell of all five sizes across **nine text scales**, **1072×1448 / 300 ppi** and **758×1024 / 212 ppi**, in both orientations. Geometry checks align each gutter to its row/column and verify physical touch minima. Options, clue details, both kinds of help, the size gate, completion and save-recovery layouts are covered.
- State tests cover persistent atomic undo, bounded largest-board history, legacy restore, corrupt/future/oversized/changed-identity refusal, exact progress acknowledgement, retry, and a separately failed solved-index write.
- **268 shared UI tests pass**, with two existing ignores; the new clue-selection regression checks every square of a panned 3×2 window and hit-tests both highlighted clue targets. Strict all-target UI and app Clippy pass. ARMv7 musl compilation and the published-catalog version gate pass.
- The final **actual SDK simulator** run at extra-large Clara BW size passes clue inspection, selected marks, forced restart, persistent run undo, storage-full preservation, retry, confirmed restart undo, full completion, completion restart/undo, 25×25 panning, square 624 restoration, all playing-help pages and the committed demonstration route. All captures assert zero fetch/post effects. The demonstration route uses a fresh disposable game after the persistence scenarios; only this script’s private fixture is reset.

See [the result](evidence/nonograms/result.json), [selected square and clues](evidence/nonograms/02-marked-square.png), [save retry](evidence/nonograms/06-save-recovery.png), [last square](evidence/nonograms/09-last-square.png), and [completed grid](evidence/nonograms/12-completed-puzzle.png). PNG/JSON/layout artifacts preserve actual source, binary, font and profile provenance, including dirty-source status. App/SDK guides, public app page and canonical screenshots are updated.

PR #167 was merged into beta as `c22c946`; its tree matches the previously verified foundation head `4611d20`. PR #168 now targets beta directly. Its merge reconciliation preserves the subsequent catalog changes; the additional shared clue renderer change is included in PR #168. No fourth PR, physical-reader operation, RAR dependency or reference-project source was introduced. Clara BW hardware acceptance remains scheduled after all three PRs.


## Nonograms picture collection and earlier-save preservation

NONO-07 is complete in Nonograms 0.1.6. **495 tasks: 117 done, 377 open and one CBR deferral. PR 2 has 25 done and 244 open; PR 3 still has 133 open.**

The default collection now contains 18 distinct original picture drawings, from a house and heart to a castle and bridge, covering all five supported sizes. Their original pixel masters and explicit larger-grid construction are in `scripts/quality/make-nonogram-pictures.py`; the generated `pictures.txt` SHA-256 is `b207347b75d886f8bc58bad5d8cfdad1835e515246b686119a132df8cf69c926`. No external corpus, artwork or reference-project source was used. The generator’s independent Python line solver checks each final image; Rust rechecks the shipped answers and distinctness. Larger grids retain the simple block drawing style.

The Earlier collection retains the previous 60 IDs, answers and progress keys. New pictures use separate `picture-NAME-v1` identities. Switching collections changes only the browser filter/page; it does not rewrite an earlier game. Imported photos return to Pictures. The browser, app instructions and canonical screenshots now show the new collection.

**32 app tests pass**, including the existing layout/recovery suite plus new-picture solvability/distinctness and separate save destinations. Strict Clippy, ARMv7 musl compilation and the published-catalog version gate pass. The final actual extra-large Clara BW SDK simulator repeats the prior clue, panning, undo, completion and recovery journey against Earlier, then saves/restarts/completes the new House picture and verifies the earlier save bytes remain unchanged. It runs the new committed demonstration route. No personal storage or physical reader was used.

See [the full result](evidence/nonogram-pictures/result.json), [House selection](evidence/nonogram-pictures/14-picture-selection.png), [restored House](evidence/nonogram-pictures/15-picture-restored.png) and [completed House](evidence/nonogram-pictures/16-picture-completed.png). Capture metadata records actual binary/source/font/profile provenance and dirty source accurately. The app README, public page and screenshots are updated. Physical acceptance remains scheduled after the three PRs; the catalog and companion program is still in progress.


## Crossword: numbered grids, complete play and acknowledged saves

CROSS-01–05 are complete in Crossword 0.1.3. **495 tasks: 122 done, 372 open,
and one CBR deferral. PR 2 has 30 done and 239 open; PR 3 has 133 open.**
Paperterm's live two-way laptop terminal session is the owner's next priority.

Crossword uses a conventional black-and-white grid with joined black rules,
small upper-left clue numbers, centered letters and solid noninteractive blocks.
The active word is shaded lightly. The first puzzle, Odds and ends, has ten
unique crossing answers of three or more letters; the other three are explicitly
word squares. Starter/Easy/Medium are editorial guides. The previous 5×5 answer
and its saved progress remain intact. This is a bundled-corpus edition: `.puz`,
`.ipuz`, rebuses and large Sunday imports are not advertised or counted as shipped.
The obsolete test-only header parser was removed.

The app deliberately requests portrait so the full clue, word and keyboard stay
together at the largest text setting. Entry accepts one letter or a whole word,
limits input to A–Z and the current word length, and preserves invalid partial
entry for correction. Checking never changes the letters. Reveal and Restart
ask first; undo survives reopening. A revealed correct guess still counts as
assistance. Empty Undo and Clear controls are disabled. Separate progress,
completion history and assistance counts for all four puzzles fit a bounded
24 KiB schema with 32 undo steps each. Exact write acknowledgements govern
saved state. Full storage retains the latest draft and exposes Retry save;
legacy records migrate only after editing, and invalid/future records stay intact.

The SDK adds a numbered-grid node on beta protocol-14 tag 33; ordinary tag-15
grid bytes are unchanged. The matching beta runtime is required. Shared key
labels fit their physical rectangles at large text sizes, and top-bar action
measurement/drawing agree when body type is too tall. A real simulator run
exposed `kobo drive type` selecting an existing crossword letter instead of a
keyboard key. The driver now prefers SDK keyboard actions, still through actual
touch coordinates; custom keyboards keep their existing fallback.

**11 app tests pass**, covering corpus validity, all nine sizes on two portrait
profiles, every clue/error entry at Largest, noninteractive blocks, bounded
history, save acknowledgements, corrupt/future records and legacy migration.
Shared validation passes **152 SDK, 268 UI and 95 protocol tests**; the UI has two
existing ignores. **21 driver tests pass serially.** The broad CLI run had one
local socket WouldBlock timeout under concurrent test load (301 passed); its
callback timing test passes in the serial driver run. Strict all-target Clippy,
ARMv7 musl checking with Rust 1.85.1 and the published-catalog version gate pass.

The [actual extra-large Clara BW simulator result](evidence/crossword/result.json)
passes 17 checks, including full completion, forced reopening, undo, full-storage
preservation/retry, all help pages, both clue directions, the committed drive
route, actual legacy migration and unreadable-record preservation. Captures
assert zero fetch/post effects and retain source/binary/font/profile provenance.
See the [numbered grid](evidence/crossword/02-numbered-grid.png),
[complete crossword](evidence/crossword/08-completed-crossword.png),
[save recovery](evidence/crossword/05-save-recovery.png) and
[preserved unreadable record](evidence/crossword/15-unreadable-preserved.png).
The app and SDK guides, public app page and screenshots are updated. Physical
Clara BW acceptance remains after all three PRs.


## Paperterm portrait and a real shared laptop terminal · 9 September 2026

PAPER-05 and PAPER-06 are complete for local/simulator validation. Paperterm
0.1.3 requests portrait before its first screen. Terminal text now has a
separate 1.8 mm monospace em, while interface labels keep their normal sizes.
The owner's text scale still applies. Rendering, cursor cells and PTY sizing
share the same metrics; no 80-column promise overrides the measured width.

| Actual simulator profile | Text setting | Keyboard hidden | Keyboard open |
| --- | --- | --- | --- |
| Clara BW 391 | Default | 75 × 49 | 75 × 27 |
| Clara BW 391 | Extra-large | 54 × 35 | 54 × 19 |
| Elipsa 2E 389 | Default | 133 × 64 | 133 × 64 |

The Elipsa reaches the existing 64-row bound in both states. App tests use the
shipped fonts across all eight supported profiles and nine text settings,
checking host-valid grids and stable columns when the keyboard opens.

The live fixture connects the actual SDK app to a real host PTY over private,
verified TLS. A second PTY represents the laptop terminal. Thirteen checks
pass in each of the [Clara Default](evidence/paperterm/clara-default/result.json),
[Clara Extra-large](evidence/paperterm/clara-large/result.json) and
[Elipsa Default](evidence/paperterm/elipsa-default/result.json) runs. They cover
portrait captures, negotiated grids, a prompt without a newline, laptop raw
mode, reader input, laptop input before Enter, shared output, wide rows,
reader Ctrl-C, the retained final screen and restored laptop terminal settings.
Each capture includes its source/binary/font/profile metadata and layout.

This exposed and fixed a host bug: `stty -g` was receiving null stdin, so raw
mode never started. It now inherits the laptop TTY. Output flushes without
waiting for a newline, and bounded draining yields the PTY lock to input.
The fixture's config/trust overrides keep the owner's identity untouched;
certificate verification remains active. Pairing is seeded for this live test,
so it does not claim manual onboarding is complete. The fresh-app committed
[pairing route](evidence/paperterm/pairing-route.json) also passes.

Validation passes 28 Paperterm, 20 stream and 268 UI tests (two existing UI
ignores), strict all-target Clippy for the changed app/UI/stream/simulator,
Rust 1.85.1 ARMv7 musl checking, formatting and the published-catalog version
gate. App, SDK, simulator and host guides, public app page and actual screenshots
are updated. No external source code or new dependencies were added. Physical
Clara BW readability, latency and refresh acceptance remain scheduled after
all three PRs. Paperterm onboarding, preview and connection/recovery polish
remain open; this does not complete PR 2 or the companion PR.


## Paperterm input recovery · 9 September 2026

Paperterm 0.1.4 accepts a key request only after the host returns an explicit
`accepted: true`. A timeout or malformed reply cannot prove whether the input
arrived, so the app discards unsent keys and pauses until **Resume typing**.
The queue is bounded at 256 bytes, with each host request still capped at 64;
overflow also pauses rather than silently dropping part of a command and
continuing. Read-only mode cannot send keys. Reconnecting while input is pending
retains the pause, and successful screen polling cannot clear it. Recovery
messages participate in the measured grid; resuming restores the keyboard's
grid. The keyboard toggle is hidden while input is paused.

Screen deltas are validated before any retained rows change. Row counts and
indices are bounded at 64, oversized text is rejected, and stale sequences
cannot replace newer output. Malformed deltas trigger reconnect instead of
allocating from an unchecked remote row index.

All **31 app tests pass**, including a stalled queue, uncertain/rejected
acknowledgements, explicit resume, read-only enforcement, malformed delta
preservation and recovery layout at all nine text sizes. Strict all-target
Clippy and Rust 1.85.1 ARMv7 musl checking pass. The [actual live result](evidence/paperterm/input-recovery/result.json)
passes 15 checks, adding a simulator-injected input timeout, no failed-key replay
and explicit resume before successful reader/laptop input. The
[paused screen](evidence/paperterm/input-recovery/02a-input-paused.png) and its
metadata/layout are retained. Relevant app docs and screenshots are updated.
PAPER-04 has partial evidence; full onboarding, preview, mode visibility and
connection polish remain open. The overall checklist stays at 124 done,
370 open and one deferred CBR task.


## Paperterm preview, pairing forms and connection state · 9 September 2026

Paperterm 0.1.5 adds a first-run choice between connecting a computer and a
read-only offline preview. The preview uses original sample output, makes no
network requests and saves no changes. Setup is split into three short pages:
prepare the host, install trust and start the session. The guide uses the
existing `kobo devices` command to find the reader's address. Help and an Address
shortcut preserve unfinished form values.

Addresses are validated before URL construction: names, IPv4 and bracketed IPv6
are accepted, with 9332 as the default port; URL credentials, paths, queries,
invalid ports and invalid numeric IPv4 are rejected. Six-character alphanumeric
codes normalize to the lowercase printed by the host. Invalid entry remains in
the field. Unreadable saved pairing remains untouched without starting network
work. The actual [offline route](evidence/paperterm/onboarding/result.json) passes
welcome, preview, each setup page, corrected address/code errors and code-draft
preservation, with zero fetch/post effects. Screenshots and metadata are included.

A status line distinguishes Connecting, Connected, Reconnecting, Input paused
and Session ended, and reports Read only, Controls or Keyboard when known.
Notices sit above the retained terminal output; the keyboard toggle remains
usable during an ordinary disconnect. The SDK now recognizes a terminal with
zero rows as a valid waiting state instead of falsely reporting hidden content.
The initial on-screen pairing run exposed that diagnostic, and the offline
reconnect run exposed the previous below-terminal warning placement; both are
fixed and the final routes pass.

The [Extra-large live journey](evidence/paperterm/paired-live/result.json) enters
the actual address and private code through the on-screen keyboard, verifies the
saved pairing, and passes 17 live checks, including explicit input recovery,
offline keyboard toggling and restored two-way input. The
[Default live journey](evidence/paperterm/status-default/result.json) also passes
17 checks. Normal grids with the status line are **75 × 47 / 75 × 25** on Clara
BW Default and **54 × 33 / 54 × 18** at Extra-large, for hidden/open keyboards.
The temporary TLS roots are installed locally by the fixture; hardware trust
transfer is not claimed.

Validation passes **38 app and 269 UI tests** (two existing UI ignores), strict
all-target Clippy, Rust 1.85.1 ARMv7 musl checking and the published-catalog
version gate. Entry, help, preview, error and empty-terminal layouts cover all
eight supported profiles at all nine text sizes. App, SDK and simulator docs
and actual screenshots are updated.

PAPER-02, PAPER-03 and PAPER-04 are complete for local/simulator validation.
PAPER-01 stays open: pairing-store load/save failures still need explicit
recovery, since `on_store` currently handles only `Loaded`. The overall checklist
is **127 done, 367 open and one deferred CBR task**; PR 2 has 35/269 complete.
Physical Clara BW acceptance and the companion PR remain pending.

## Paperterm pairing storage recovery — 9 September 2026

Paperterm 0.1.6 completes PAPER-01. A failed pairing read offers Retry reading
or Continue without saving; temporary sessions never write pairing data. New
credentials are saved only after the computer confirms a valid handshake, so
an unreachable or rejected connection cannot replace a saved connection.
Single-flight writes track the exact acknowledged snapshot. A failed save
leaves the terminal usable, displays Not saved, and offers Retry saving from
Pairing. Polls and resizing do not silently retry the write. The connection
menu retains the negotiated terminal dimensions while output continues.

The [storage recovery journey](evidence/paperterm/storage-recovery/result.json)
and [temporary connection journey](evidence/paperterm/temporary-pairing/result.json)
each pass **19 checks** on portrait Clara BW at Extra-large. Both use the actual
SDK app, a real laptop/host PTY and private trusted TLS. The former retries a
failed read and explicitly retries a failed save after successful reader input;
the latter proves the original unreadable store path remains untouched. Both
also verify reconnect, two-way input, uncertain-input recovery, wide output,
Ctrl-C and laptop terminal restoration. Actual PNG captures and layout metadata
are included; the source was the pre-commit working tree based on c612cb1.

**42 app tests**, strict all-target Clippy, Rust 1.85.1 ARMv7 musl checking and
the published-catalog version gate pass. Recovery layouts cover all nine text
sizes; existing grid and entry coverage retains all eight supported profiles.
The app README, simulator instructions, release notes, generated app page and
actual screenshots are updated. No physical reader execution is claimed.

All six Paperterm checklist tasks are now complete for local/simulator validation.
The program totals **128 done, 366 open and one deferred CBR task**; PR 2 has
**36/269 complete**. Physical Clara BW acceptance remains scheduled after all
three PRs, and all 133 companion tasks remain open.

## Logic Pack progress and undo — 9 September 2026

Logic Pack 0.1.3 keeps separate progress for its four current games. Switching
games no longer resets them. Each game retains up to 32 undo snapshots,
including mine relocation, flood reveals, flags, checks and completion.
Restart asks first, preserves the other games and can itself be undone.
Completed fields show appropriate actions; Minesweeper uses blank zero-neighbour
squares and flags the remaining mines on completion.

The bounded 24 KiB versioned record verifies each original puzzle identity,
position and undo history before restoring anything. Legacy single-game records
remain unchanged until the next edit. Invalid and future records stay untouched.
The shared draft state releases one write at a time, acknowledges the exact
snapshot and offers explicit retry after failure without discarding newer moves.
The app prevents suspension with unsaved edits. It opens in portrait and uses
two-column controls with Help in the top bar.

**13 app tests pass**, including shipped-font layouts at all nine text scales
on Clara BW and 758 × 1024 displays. Strict all-target Clippy, Rust 1.85.1 ARMv7
musl checking and the published-catalog version gate pass. The
[actual Extra-large simulator route](evidence/logicpack/progress-recovery/result.json)
passes **16 checks**: independent progress, forced restart, persistent undo,
full-store preservation/retry, confirmed restart/undo, all four completions and
reopenings, first-mine relocation/loss undo, help, the committed drive route,
legacy migration and future-record preservation. Every capture checks zero
fetch/post effects and records binary/source/font provenance. Captures were
made from the working tree based on 87ea6ce. App and simulator docs, release
notes, public page and actual screenshots are updated.

LOGIC-02, LOGIC-03 and LOGIC-05 are complete for the current four games.
LOGIC-01 remains open for a varied validated collection with difficulty guides;
LOGIC-04 remains open for proper line, bridge and cross-sum board rendering.
The Minesweeper control overflow is resolved, but generic board cells are not
claimed as the finished design. Physical Clara BW acceptance remains pending.
The program totals **131 done, 363 open and one deferred CBR task**; PR 2 has
**39/269 complete**. All 133 companion tasks remain open.

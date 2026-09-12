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

## Logic Pack pencil boards — 9 September 2026

Logic Pack 0.1.4 replaces separate text buttons with shared pencil-board
geometry. Slitherlink draws continuous lines between dots with fixed interior
clues; Hashi draws circular numbered islands and single or double bridges;
Kakuro joins white cells to black diagonal sum cells. Across sums occupy the
upper-right triangle and down sums the lower-left. Fixed clues and given digits
carry no action. The original puzzle identities, progress and undo are retained.

The SDK adds `pencil_board` with integer board positions, a physical cell size,
bounded marks and orthogonal edges. Runtime and simulator share layout, ink,
number fitting and touch targets. Validation rejects duplicate coordinates,
edges/actions, reserved actions, invalid numeric values, missing endpoints and
excessive counts. Layout expansion is also bounded. Protocol-14 beta tag 34
carries the new node; previous tags and older installed grid encodings remain
unchanged. Earlier wire versions refuse the new node.

Validation passes **13 app, 152 SDK, 97 protocol and 272 UI tests**, with two
existing UI test ignores and two SDK documentation ignores. New checks cover
protocol round-trips/truncation/refusal, fixed clues, physical edge targets,
continuous strokes, double-bridge ink and dirty clipping. App layouts with
shipped fonts cover all nine text scales on Clara BW and 758 × 1024. The broader
SDK run found an old terminal test still expecting caption metrics; its
assertions now use the dedicated terminal font introduced for portrait Paperterm.
The corrected focused test and full SDK suite pass.

Strict all-target Clippy and Rust 1.85.1 ARMv7 musl checking of both the app and
reader runtime pass; the runtime retains its existing platform warnings.
The [actual Extra-large simulator journey](evidence/logicpack/pencil-boards/result.json)
passes **17 checks**, retaining all four completions/reopenings, undo, failed-save
retry and record-preservation checks while verifying the new shared geometry.
Additional captures show double bridges and excluded loop edges. Every capture
checks zero network effects and records binary/source/font provenance, from
the working tree based on 7be7ebe. App, SDK and simulator docs and actual
screenshots are updated, and the published-catalog version gate passes.

LOGIC-04 is complete for local/simulator validation. LOGIC-01 remains open for
a varied validated collection with difficulty guides. Physical Clara BW
acceptance remains pending. The program totals **132 done, 362 open and one
deferred CBR task**; PR 2 has **40/269 complete**. All 133 companion tasks
remain open.


## Logic Pack original collection and direct digit entry

LOGIC-01 is complete for implementation and local/simulator validation.
Version 0.1.5 offers 20 original puzzles: five each of Slitherlink, Hashi,
Kakuro and Minesweeper. The four previous identities remain intact. The picker
shows each puzzle's title, editorial difficulty and saved progress. The original
Python generator verifies exactly one rule-valid solution for every loop,
bridge and cross-sum puzzle; an independent Rust checker validates completed
boards against the rules. Mines difficulty describes size and density and does
not promise guess-free play. No reference-project code or puzzle corpus is used.

Kakuro opens a direct digit picker with the selected square's row, column and
both sums, plus clear and cancel. Board controls use one row; More contains undo,
checking, confirmed restart and rules. Each puzzle retains 32 persistent undo
steps. The bounded 128 KiB record migrates both original single-game saves and
four-game version-1 records, including undo, without writing until an edit.
Unreadable and future records remain preserved.

The [actual simulator journey](evidence/logicpack/collection/result.json)
completes and forcibly reopens all 20 puzzles, with exact saved-state comparisons.
Its 16 check groups include collections, rules/difficulty, direct entry/clear,
restart/undo, full-store recovery, both migrations, special marks and zero network
effects. Captures include actual binary/source/font provenance from the working
tree based on e40dcd4. Representative picker, entry and all four larger boards
were visually inspected. App and public documentation use actual new captures.

All 16 app tests pass, including independent solution checking, full 20-puzzle
undo capacity, every collection screen and every Kakuro entry position with
shipped fonts at all nine text scales on 1072×1448/300 ppi and 758×1024/212 ppi.
Strict all-target Clippy, Rust 1.85.1 ARMv7 musl checking, formatting, diff checks,
original generator reproduction and the published-catalog version gate pass.
The reproduction check was corrected to compare JSON-normalized tuples/lists;
it does not change the generated puzzles. Physical acceptance remains pending.

Program totals: **133 done, 361 open, one deferred CBR task**. PR 2 has
**41/269 complete**; all 133 companion tasks remain open.


## calibre-web catalog, account and offline reading acceptance

calibre-web 0.1.3 replaces the static root categories with parsed OPDS 1.2/2.0
navigation, book details, catalog paging and parent-page retention. Setup accepts
an HTTPS OPDS endpoint, checks its response and acknowledges the saved address.
Unreadable settings remain untouched. Private libraries use the shared username/
password flow and the existing atomic server-bound account record. The policy
now explicitly permits calibre-web's `calibre` Basic account for reads within
its saved HTTPS server scope. Legacy unbound accounts retain their root-only
policy; owners sign in on the reader to enable private navigation and downloads.

EPUB/text downloads use bounded chunks and acknowledged shelf writes before
library publication. Shelf filenames satisfy the platform's 64-character limit;
the complete SHA-256 digest validates every local opening. BookView provides the
shared reader. A bounded 64-book index retains reading state with serialized,
exactly acknowledged writes and explicit retry. Damaged files offer replacement
without prematurely removing the prior index entry. The top-bar Back action
closes the book after honoring any reader-internal navigation.

The [actual HTTPS simulator journey](evidence/calibre-web/result.json) passes
**16 checks** on Clara BW at Extra-large. It follows author/shelf links, downloads
the original EPUB, saves its position, kills the app and reopens offline with no
network request. It exercises failed reading/setup writes, explicit retries,
file corruption and repair, HTTP authentication refusal, malformed catalogs and
unreadable settings with continued offline access. Its final stage enters an
account through the on-screen keyboard; the server verifies Basic authentication
on the root, author catalog and EPUB download. The fixture stores no owner data
and records no authentication headers. Captures include binary/source/font
provenance from the working tree based on 05a0012. Representative catalog,
reading and recovery captures were visually reviewed and published in app docs.

The real-storage journey exposed an overlong shelf key and a missing top-bar
Back handler that the initial mocked tests had missed; both are fixed, covered
by regressions and included in the final successful journey. All **18 app tests**
and **131 policy tests** pass, including all-scale setup/catalog/recovery layouts,
local EPUB position restoration, publication ordering and server-scope refusal.
Strict all-target Clippy, Rust 1.85.1 ARMv7 musl checking, formatting, diff checks,
the CLI build and published-catalog version gate pass. App, SDK policy and
simulator documentation and actual screenshots are updated.

CALIBRE-01 through CALIBRE-06 are complete for implementation/local validation.
Physical Clara BW acceptance remains pending. Program totals: **139 done,
355 open, one deferred CBR task**. PR 2 has **47/269 complete**; all 133
companion tasks remain open.


## Miniflux article retention work in progress

The current working tree validates Miniflux response shape, bounds, positive
unique IDs and article fields before replacing the current inbox. HTML bodies,
source URLs and read/star state are retained for the shared reader. Malformed
responses preserve the current articles and show an error instead of appearing
as a successful empty inbox. Five tests and strict Clippy pass. No MINI or LATER
task is complete yet; disk persistence and actual reading/sync routes remain open.

The [official Miniflux API](https://miniflux.app/docs/api.html#update-entries)
requires `PUT /v1/entries` for read-state updates. Desired starred state is
supported there since 2.3.2; the older bookmark endpoint toggles state and cannot
be blindly retried. The unused test-only POST helper was removed because it
asserted an incorrect request shape. The current task transport exposes only
GET/POST, so correct native PUT support and method-aware credential checks are
needed before connecting the durable mutation queue. Wallabag's mutation method
must also be verified when Read Later is integrated. Remaining work includes
acknowledged body/index storage, reader pagination, pending/retry UI, fixture
servers, updated docs/screenshots and release checks.


Miniflux platform follow-up: the [server-bound token policy](miniflux-account-policy.md)
now supports reviewed reads, explicit entry PUTs and constrained feed creation.
The runtime applies the saved-account gate before resolving the token. Article
parsing additionally refuses duplicate top-level/entry fields, malformed status
and URL types, and IDs outside the parser's exact integer range. App integration,
acknowledged offline storage and end-to-end sync remain unfinished.

### RSS search and offline reading — in progress

The RSS discovery parser now distinguishes malformed, truncated, incorrectly
shaped and invalid UTF-8 responses from a successful empty result. The failure
screen offers retrying the same address or changing it. Refreshing the current
feed retains its open articles on failure; changing feeds clears those articles
so one publisher's content cannot appear under another publisher's name.
These are in-memory recovery improvements, not durable offline storage.

The remaining RSS/Miniflux work must cover direct feed URL preview and adding,
searchable saved article bodies, reading position after restart, and account-bound
read/star changes that remain visibly pending until the server acknowledges them.
Miniflux integration must use the reviewed server-bound credential policy and
reconcile uncertain writes before repeating them. No RSS or Miniflux checklist
item is marked complete by these partial changes. Actual simulator screenshots
and restart/reconnect fixture evidence are still required before release.

RSS snapshot storage is now connected to feed opening and successful refresh.
A two-slot file scheme keeps the published snapshot intact until the replacement
file and its store pointer are acknowledged. Opening a saved feed reads its
local snapshot without starting a fetch; Refresh is explicit. SHA-256 verifies
reopened bytes. An uncertain pointer acknowledgement prevents additional writes
until state is reloaded, avoiding accidental overwrite of a possibly published
slot. Unit tests cover publication ordering, reopening, corruption and uncertain
commit behavior. Cache cleanup, in-app recovery, reading position, image storage
and actual process-restart simulator evidence remain unfinished.

Image support is now explicit in FEEDS-06 and MINI-03: inline images,
captions/alt text, e-ink scaling, offline restoration and missing-image recovery.
The existing shared BookView has image decoding/rendering and a same-origin
fetch pipeline; RSS currently converts markup to text and has not yet adopted
that pipeline or durable image storage. Neither image requirement is complete.

Article parsing now retains publisher HTML alongside plain text: RSS encoded
content, Atom XHTML (including image attributes and captions), and JSON Feed
HTML survive parsing. Nested article markup cannot replace entry titles, and
plain Atom text keeps literal angle brackets. HTML web pages with titles are
rejected as direct feeds. Shared-reader display and offline image binaries are
still pending; these parser tests are not image-rendering evidence.

Release follow-up requested on 2026-09-09: publish from beta after current work.
Remote inspection found beta at c22c9462f6b191999529d683276505079e171cc9,
already tagged beta-v0.3.12. Requested beta-v0.3.10 already exists at
cf7eb33b9ca394160646e33398b48045e3774217. Version clarification is pending;
no existing tag is to be moved. Recheck beta, unused version, CI and release
artifacts when the work is ready. No release has been dispatched for this request.

HTML RSS articles are now connected to BookView for measured pagination, image
loading and reading controls. Image task completions are routed to the reader;
leaving the article closes it and cancels its pending request. App tests verify
an image fetch with no credential, cancellation and ignored late replies.
Plain-text entries retain the existing reader for now. Full reading-state
persistence, durable image binaries and actual simulator visual evidence remain
open; no image checklist item is closed on the basis of these tests.

BookView now resolves root-relative image paths (such as /images/photo.png)
against the document's HTTPS host. It does not treat them as filesystem paths.
Protocol-relative addresses, traversal and off-host requests retain their
existing restrictions.

Actual RSS simulator evidence now covers six checks using the original Field
Journal fixture: local feed opening with zero network requests, HTML reading and
page turns, offline refresh retaining the open articles, reopening after killing
and restarting the app, unchanged saved feed bytes, and zero POST/PUT/PATCH
requests. The SDK makes two failed fetch attempts during explicit offline
refresh. All six captures pass simulator diagnostics and record the actual
source/binary/font/profile provenance. See `evidence/rss-offline/result.json` and
`scripts/quality/check-rss-sim.py`. Canonical app screenshots now show this run.
The fixture deliberately contains no image binaries and does not assert reading
position persistence. Those requirements remain open, as does hardware testing.

HTML reading state now persists through the acknowledged `reading-v1` store.
Writes serialize, retaining newer positions while an earlier write is pending;
failed acknowledgements retain unsaved state for explicit retry. Strict decoding
preserves unreadable/future records. Limits are 1,000 article versions and
192 KiB, with visible refusal rather than eviction of bookmarks/annotations.
Article identity includes the body, so changed content cannot reuse a stale
locator. Plain-text reading-state migration remains open.

75 RSS tests and strict Clippy pass. The updated actual simulator journey
verifies page 2 restoration after process restart: all layout fields match the
pre-restart page except the expected paint counter. Stored progress bytes remain
unchanged on reopening. Evidence and app screenshots are refreshed under
`evidence/rss-offline`. Unit tests cover pending-save ordering, retry and corrupt
records; simulator storage-failure recovery and image persistence remain open.

Plain-text entries now use the same BookView and reading-state persistence as
HTML; the old separate page model is removed. A regression test verifies literal
angle brackets remain text, multi-page pagination, saved position and reopening.
The RSS suite remains at 75 passing tests; strict Clippy passes.

The actual simulator fixture now also exercises storage-full during a page turn:
the previous progress bytes remain unchanged, the reader displays an unsaved
warning, and an explicit retry from the article list persists the newer position.
`07-progress-save-failure` and `08-progress-save-recovered` screenshots and layout
provenance are included with the updated eight-check result. These checks do not
claim image-cache or Miniflux completion.

RSS now manages image retrieval itself around BookView: verified local snapshot
first, then one bounded credential-free network request if no snapshot exists.
Downloaded bytes must decode before being submitted for two-slot persistence.
Leaving the article cancels its image request; already-issued store writes can
finish. Current bounds are 16 images/article, 512 KiB/image and 64 image records
per running session. Cache cleanup and durable retry for image-save failures are
not finished and the overall image requirement remains open.

76 RSS tests and strict Clippy pass. The actual simulator fixture now includes
an original PNG ridge drawing and caption, opened from a seeded verified local
snapshot with zero fetch effects. The image, caption, article page turns,
restart position and progress-save recovery pass together. Evidence and the
canonical reading screenshot are refreshed. This proves rendering/reopening;
it does not prove network acquisition or image-save recovery.

Image-store retry now retains the candidate bytes on a failed write, reloads
the published reference, then selects the inactive slot. A unit regression
models an acknowledgement failure after the new reference was actually committed:
retry writes the other slot and preserves the published file. Corrupt references
still block replacement. Image storage can be retried from the article list;
background write failures remain visible even after leaving the article.
Live HTTPS acquisition/save-failure simulator coverage is still pending.

BookView image resolution now uses the existing HTTPS URL parser and accepts
explicit ports while requiring the document's effective port for absolute image
URLs. This supports self-hosted readers without allowing a document to change
ports. Root-directory relative URLs and documents with no path also resolve
correctly. 77 RSS tests, 17 BookView tests and strict Clippy pass.

Live image acquisition/recovery is now exercised by
`scripts/quality/check-rss-image-save-sim.py` against a private HTTPS server on
an explicit port. The app downloads one original PNG without Authorization or
X-Auth-Token headers. Full storage prevents publication; retry persists the exact
image bytes without another download. After process restart in the offline
scenario, the saved image renders with zero fetch effects. All captures pass
diagnostics and preserve source/binary/font/profile provenance. See
`evidence/rss-image-save/result.json` and its four screenshots.

Save failures now share one notice and fixed-bottom “Retry saving” action. The
new SDK `paginate_rows_below_notice` reserves the measured banner height and
bottom action space; a 50-article list regression verifies recovery still fits.
78 RSS tests and strict RSS/SDK Clippy pass. Cache cleanup, remaining RSS
features and Miniflux remain open; these results do not close the app checklist.

RSS now searches the open feed's saved article titles, authors and plain text.
All query words must match, ignoring case; result actions retain original item
indices. The query survives opening/back navigation, and Clear restores the full
list. The actual simulator verifies search entry, body matches, no-match feedback
and clearing with zero fetch effects, alongside the existing restart/save checks.
Search screenshots and provenance are refreshed in `evidence/rss-offline` and
the app README. Cross-feed search remains open. 79 RSS tests and strict Clippy
pass; the new regression verifies selection of an original nonzero article index.

OPML import now reads staged app-shelf files, parses nested outlines and decoded
attributes, previews new/skipped counts and commits subscriptions only after the
store acknowledgement. Duplicate/unsupported URLs are counted, HTTP and embedded
credentials are refused, and oversized/damaged input or excess subscription
capacity cannot partially import. Bounds are 256 KiB and 2,000 outline elements;
the existing 40-feed limit is enforced before saving.

82 RSS tests and strict Clippy pass. The actual simulator fixture now stages an
original OPML file, verifies the preview, forces a failed save with unchanged
subscription bytes, then retries and verifies exactly one new HTTPS feed. The
three import captures and updated result are in `evidence/rss-offline`; the app
README has the current preview screenshot. Computer-side transfer and a fuller
per-feed preview remain open, so FEEDS-03 is not yet marked complete.


### RSS selection and large-text corrections — 9 September 2026

OPML preview now lists each feed’s title and HTTPS address, supports including or
excluding individual feeds, and permits a selection from a list larger than the
remaining subscription capacity. Empty or oversized selections cannot write.
The source indices are preserved through pagination and confirmation. Subscriptions
still change only after a successful store acknowledgement.

Actual Clara BW simulator journeys pass at default and 170% interface scale,
including selection toggles, failed subscription save and retry, image-backed
offline reading, process restart, article search and progress save recovery.
Evidence is in `evidence/rss-offline` and `evidence/rss-offline-large`.
The OPML screenshot and app README are updated. Computer-side transfer remains
open; FEEDS-03 is not complete.

The 170% run exposed shared-reader quote rendering using interface typography
against reading-face measurements; drawing and validation now use the same
reading face. It also exposed a clipped refresh skeleton and a reading save
warning overlapping the footer. Feeds now keeps saved rows visible during
refresh; loading an empty feed uses the activity label without a fixed skeleton.
Reader reports reserve their measured banner height before pagination, preserving
the document location and end-of-book navigation when dismissed.

Validation: 84 RSS tests, 79 reader tests and 273 UI tests pass (two existing UI tests are ignored); strict Rust
1.85.1 Clippy passes for these three crates. Further RSS work and release gates
remain open; these changes are local work in progress.


### RSS subscription write acknowledgements — 9 September 2026

Replaced overlapping subscription writes with one active write and a coalesced
latest pending snapshot. An acknowledgement for an earlier write cannot complete
an OPML import. Save failures keep the current in-memory subscription list and
expose Retry saving on the shelf; failure state remains visible after navigation.
Shelf pagination reserves the warning area. Imports remain unpublished until
the exact candidate snapshot is acknowledged.

86 RSS tests and strict Rust 1.85.1 Clippy pass. Actual default/170% Clara BW
simulator journeys now include failed unfollow, unchanged disk state, explicit
retry and restart verification; captures 15–17 and updated result files are in
both RSS evidence directories. App docs and the new subscription-save screenshot
are updated. Broader feed management, cache cleanup and companion transfer remain
open. These RSS changes are still local work in progress.

The earlier focused CI repair, commit 3b0c92d, passed both GitHub runs
34319399506 (push) and 34319404819 (pull request), including host, device and
simulator jobs. This validates that committed repair, not the subsequent local
RSS work.


### RSS unreadable subscription recovery — 9 September 2026

Subscription decoding now rejects malformed UTF-8, malformed rows and records
over the 40-feed limit instead of silently discarding rows and later overwriting
the original. Valid three-field records and blank titles retain their existing
behavior. A denied load or unreadable record blocks subscription mutations and
shows a retry screen. A successful reload restores normal operation.

87 RSS tests and strict Rust 1.85.1 Clippy pass. The simulator fixture seeds a
damaged file, retries and verifies exact byte preservation, then supplies a
repaired fixture and verifies reload. Captures 18–19 extend the existing offline
journey. This does not implement automatic file repair or the companion transfer
flow; both remain outside the work completed here.

Both default and 170% interface-size simulator journeys pass; their evidence
directories and the app recovery screenshot are updated.


### Public RSS starter feeds — 9 September 2026

Add a feed now offers Browse with BBC Science & Environment and NASA Science.
Both use the existing direct HTTPS preview and explicit subscribe flow; browsing
itself spawns no request. Unit coverage verifies each target URL, absence of
credentials and the separate preview/subscribe steps. Offline simulator coverage
verifies that a failed preview leaves saved subscriptions unchanged.

Live validation on this date returned HTTP 200 without redirection for both
publisher URLs: BBC supplied 42 RSS entries in 31,146 bytes; NASA supplied 10 in
277,699 bytes. No publisher article bodies were added to repository fixtures.
URLs: https://feeds.bbci.co.uk/news/science_and_environment/rss.xml and
https://science.nasa.gov/feed/. These are availability observations, not a
promise about future publisher uptime.

88 RSS tests and strict Rust 1.85.1 Clippy pass. App documentation now explains
the Browse flow. This remains part of the local RSS work pending the app release
checks and the remaining RSS functionality.

Default and 170% simulator journeys pass, with captures 20–21 showing the
starter list and failed offline preview in both RSS evidence directories.


### RSS article unread state — 9 September 2026

The article list now displays an unread count and labels unread rows. Read state
uses the same stable content identity as the saved reading position, avoiding a
second ledger that could disagree with it. Opening a readable article records
its initial position immediately. Restoring the acknowledged reading record
restores read status; revised content has a new identity and is unread. Unknown
reading status is not presented as a count. This records opening an article,
not a claim that the reader finished it.

89 RSS tests and strict Rust 1.85.1 Clippy pass. The simulator journey asserts one
unread article before opening and zero after returning, alongside its existing
restart and failed-save checks. Per-feed refresh timestamps, durable shelf
summaries and failure status remain open, so FEEDS-05 is still incomplete.

Default and 170% actual simulator runs pass. Both RSS evidence directories and
the article-list screenshot now show unread status.


### RSS per-feed refresh history — 9 September 2026

Added a bounded acknowledged history record for each feed (40 feeds, 32 KiB).
Successful parsed refreshes record a UTC timestamp; failures retain the previous
success and record the reason separately. The shelf shows the timestamp and
failure marker, while the article list restores the failure detail. Cached
opening does not advance history. History writes serialize, retry the latest
state after failure and refuse to overwrite unreadable data. Removing a feed
prunes its history only after the subscription save is acknowledged. A failed
subscription load does not prune history.

95 RSS tests and strict Rust 1.85.1 Clippy pass. Added coverage for event ordering
before initial load, successful-refresh/failure transitions, failed-save retry,
reopening history, malformed records and pruning only removed feeds. Actual
simulator journeys at default and 170% seed a known successful timestamp, force
an offline refresh and verify that the failure persists without changing that
timestamp. Both RSS evidence directories and the feed-status screenshot are
updated. Successful live publisher fetching was validated separately in the
starter-feed work; the timestamp here is an explicitly seeded fixture.

FEEDS-05 still needs the combined shelf unread summary and remaining integration
review. RSS release checks and the broader app work remain open.


### RSS shelf unread summaries — 9 September 2026

Feed history now carries an optional bounded unread/total summary for its saved
snapshot. Updates use acknowledged reading-record identities rather than pending
in-memory edits. A reading save failure cannot advance the durable shelf count.
Legacy refresh-history rows remain readable; malformed/out-of-range counts are
refused. Removed subscriptions are excluded from subsequent count updates.
A history load cannot prune records while subscription removal is unacknowledged.

Added SDK helpers for clamping multi-line overflow-menu rows and paginating
those rows below a measured notice. RSS uses the actual menu column when laying
out the combined timestamp, unread count and failure summary. SDK.md and the app
README describe the behavior.

97 RSS tests, 153 SDK tests and strict Rust 1.85.1 Clippy pass. Two existing SDK
doc tests remain ignored. Default and 170% simulator journeys verify a zero-unread
shelf summary after process restart. Both evidence directories and feed-status.png
are updated. The broader RSS release/integration pass is still pending.

### RSS refresh-save recovery and integration — 9 September 2026

The workspace all-target/all-feature test run completed successfully on the
snapshot before the final RSS retry changes. The final RSS suite passes all 99
tests; strict Rust 1.85.1 Clippy passes for RSS, SDK, reader, UI and BookView.
The final RSS ARMv7 musl cross-check, formatting check and whitespace check pass.

A failed feed snapshot now offers Retry saving. Retry rereads the published
pointer before choosing a slot. A newer refresh replaces the unsaved candidate
without writing over the previous published copy. The HTTPS simulator fixture
now exercises two successive refreshes during full storage, recovery without
another download, and an offline restart that opens the newest article and image.
All requests are credential-free; no POST, PUT or PATCH is issued.

Both default and 170% text-size runs pass with no error diagnostics. Their seven
captures and request assertions are in `evidence/rss-image-save/` and
`evidence/rss-image-save-large/`. Visual inspection caught a duplicated save
warning; the final captures show one warning and a punctuated unread count.
The app README and save-articles screenshot document recovery. The public SDK
guide now describes the measured notice/menu pagination helpers.

FEEDS-01, FEEDS-02, FEEDS-04 and FEEDS-05 are locally implemented and validated.
FEEDS-03 remains open for the computer-to-reader import path. FEEDS-06 remains
open for the remaining image-recovery/cache review. RSS release/version checks,
publication and Miniflux work remain outstanding; these changes are not yet pushed.

### RSS missing and damaged image recovery — 9 September 2026

Cache reads distinguish unavailable content from uncertain writes. Missing,
oversized or digest-invalid content can be downloaded again after its pointer
has been read successfully. Replacement writes use the other slot; failed write
acknowledgements still require rereading the pointer before any retry. Neither
the damaged file nor its pointer is changed merely by opening an article offline.

The RSS suite passes 100 tests, including missing/corrupt snapshot replacement
and the existing uncertain-commit cases. Strict Rust 1.85.1 Clippy and the ARMv7
musl cross-check pass. The HTTPS fixture passes at default and 170% text size,
with 12 captures in each rss-image-save evidence directory. It removes an image,
then damages a saved replacement, verifies readable offline fallback, reopens
online to fetch exactly one credential-free replacement each time, and finally
restarts offline. Recorded layouts confirm image absence during fallback and
image presence after repair and restart. Those assertions are also now part of
the fixture. Default-size fallback and recovery screenshots were visually
inspected and added to the README.

The changes remain local while RSS integration, cache lifecycle review, OPML
transfer and Miniflux continue. No additional checklist task is closed here.

### RSS long-session image capacity — 9 September 2026

The 64-record in-memory image limit no longer permanently blocks new images
after a long session. When full, the manager releases an idle record outside
the current article. Saved files and pointers remain untouched, so revisiting
an image checks its local copy again. Active reads/writes, failed saves and
current-article records cannot be released.

A runner test opens 128 distinct illustrated articles, acknowledges task
cancellation on leaving each one, then revisits the first image. Every image
checks its saved copy and can request a missing image. A separate capacity
test verifies that active and failed writes survive while only idle records
can be released. All 102 RSS tests, strict Rust 1.85.1 Clippy, formatting and
the ARMv7 musl cross-check pass. The README documents the memory lifecycle.
This changes no visible layout; existing recovery screenshots remain relevant.
Disk cache cleanup and final RSS integration remain open.

### Shared SDK snapshot storage — 9 September 2026

The verified Feeds cache implementation now lives in `kobo_sdk::snapshot` as
`Snapshot` and `SnapshotEvent`. Feeds re-exports these types instead of carrying
a private duplicate. The configurable bound defaults to 512 KiB and supports
the 768 KiB Miniflux response limit, capped by the SDK shelf download maximum.
SDK.md and the public SDK guide document callback routing, acknowledged
publication, recovery and lifecycle ownership.

The extracted implementation passes 95 Feeds tests and 162 SDK tests (seven
storage tests moved from Feeds; two new bound tests were added). Two existing
SDK doctests remain ignored. Strict Clippy passes. The default-size HTTPS
simulator fixture passes all image/feed persistence and repair scenarios using
the shared implementation; `evidence/sdk-snapshot/` records its result and final
offline restart capture. No visible interface changed in this extraction.
Miniflux offline integration and the shared-change publication checks remain
open; these new changes are local.

### Miniflux offline response persistence — 9 September 2026

Miniflux now uses the shared SDK snapshot with its 768 KiB response bound. The
snapshot identity includes the server address and credential name. Startup
loads the saved response without a network request; validated sync responses
retain complete supplied HTML bodies and article metadata. File and pointer
acknowledgements are both required before publication. Failed saves leave
articles in memory and expose explicit retry without another fetch. Switching
settings is blocked while storage or queued mutations remain unfinished, and
changing settings cancels an outstanding fetch before opening the new scope.

Eight Miniflux tests pass, including acknowledged publication, complete-body
restore after restart with zero spawned tasks, failed-save retry and malformed
response preservation. Strict Clippy passes. The README distinguishes this
local storage work from the unfinished reader, image, subscription and mutation
flows. No Miniflux checklist task is closed yet: simulator verification, account
setup and the complete reading/sync journey remain outstanding.


### Miniflux reading, tabs and acknowledged changes (11 September 2026)

The Miniflux application is now the complete account-backed reader the
checklist asks for, and MINI-01 to MINI-06 are closed on this evidence.

**Shared before written twice.** The article-image loader and the acknowledged
reading-position record left Feeds and became `kobo_bookview::illustrations`
and `kobo_bookview::positions`; Feeds re-exports both, so the two readers of web
articles share one implementation of the parts that are easy to get wrong.
Saved image identities and the position file keep the names Feeds published
under, so an upgrade opens what a reader already has. The SDK gained
`Context::paginate_rows_with_menu_under`, because a list measured without the
overflow mark's column comes back with rows that wrap when drawn.

**Reading.** Articles open in the shared document reader through
`kobo_doc::html`, with headings, quotes, figures and captions. A picture the
feed named but did not carry is fetched once and saved; the saved copy is used
afterwards, including offline after a restart. Reading positions are kept per
article and returned to.

**Three tabs, one download.** A sync collects unread, starred and read as three
requests, because a Miniflux query carries one status, and merges them into one
saved batch of at most 100 articles. All three tabs then work with the radio
off. A part that fails ends the sync rather than writing half a batch over a
whole one.

**Changes.** Every change is sent as the state the article should end in, never
as a toggle: the runtime sends an update once and never replays it, so a queue
of assignments is the only kind that can be resumed safely after a lost reply.
The queue is written down before it is sent, survives restarts, is shown on
screen with its count, and is flushed before a sync downloads anything. The
reviewed credential surface allows exactly these routes, which is why the
bookmark toggle endpoint is not used.

**Verification.** Twenty-six application tests pass, including reaching every
article of a 100-entry batch at all nine interface scales with no error
diagnostics, the merged three-part sync, an acknowledged save reopening after a
restart with no request, a failed save keeping its articles, a lost reply going
out again unchanged, and a refused account change while work is outstanding.
Twenty-two `kobo-bookview` and 162 SDK tests pass, 90 in Feeds. Strict Clippy
and `cargo fmt` pass.

`scripts/quality/check-miniflux-sim.py` drives the actual simulator against an
original HTTPS fixture account with real taps: first sync, reading with its
picture, a page turn and a resumed position, the read mark reaching the server,
starring from the row menu, a change made offline, a process restart with no
network, catching up on reconnection, an applied change whose reply the fixture
drops, a full article fetched and reopened offline after another restart, and a
suggested feed filed under a real category. Every capture passes runtime layout
diagnostics; the token is attached to every request by the runtime and never
appears in the application. The journey passes at default text size and at
170%; captures and results are in `evidence/miniflux-account/` and
`evidence/miniflux-account-large/`, and the screenshots were visually inspected.

Superseded: the earlier `evidence/miniflux-pages/` captures and their entry
described list reachability against a hand-written saved file, which this
journey covers end to end from a real account.

### Grimoire references, initiative and a table of six (11 September 2026)

GRIM-03 to GRIM-06 are closed on this evidence, which leaves the Grimoire
group complete.

**What the bundle actually held.** The index ships 624 magic items that had no
way in from anywhere in the application, fifteen 2024 conditions whose bodies
were empty because the generator read only the 2014 spelling of the field, and
every rule section with a literal backslash-n where its paragraph breaks should
have been, because each field was escaped twice on the way into the file. The
generator now reads both spellings, escapes once, and converts the snapshots'
Markdown into the reader's markup; Magic items is a category on the home
screen. A table in the source is written out one labelled row at a time rather
than as columns, because a column that fits at the default text size is clipped
at the larger ones and the panel has nothing smaller to fall back on.

**Reading.** References open in the shared document reader. The longest rule
section is twenty pages at the default size and can be read to its last word at
every one of the nine interface sizes, with headings as headings and no markup
on the panel. Bookmarking and sending a monster to the initiative order moved
to the row, since the reading screen belongs to the reader.

**The table.** Initiative states the round and which turn of how many it is.
Previous turn takes the round back with it, which is what a turn passed by
accident costs at a table. A combatant has a screen of their own for taking the
turn, correcting a roll or being removed, in the same shape the party already
used for a member; ending combat sits in the top bar away from the control
tapped every turn. Six combatants and six party members are measured for the
panel: before this the sixth member was drawn under the Add member button and
clipped by the foot of the screen at 170%, and a member's slots and buttons ran
off the bottom entirely. A member is now two pages, health and slots.

**Emptiness explained.** A category an edition does not hold says so and names
the edition that does, instead of asking the reader to change a filter they
never set. About counts what the build holds per edition rather than claiming
it, and says plainly that the System Reference Documents cover no classes,
subclasses, backgrounds or feats.

**A platform fix.** The tail of a paragraph split across a page break was drawn
as a plain text node measured with the interface's line spacing while the page
had been measured with the book's. Where a reader's type size differs from the
interface size the last line of such a paragraph was dropped, and the runtime
refused the screen: the simulator reported it at 110% on the twentieth page of
a rule section. Every paragraph now goes through one node in `kobo-read`. Feeds
and the Miniflux reader are on the same path and had the same exposure.

**Verification.** Twenty-two Grimoire tests pass, including the longest
reference read page by page at all nine text sizes with no markup left in it,
six combatants and six party members reachable at every size, every table
screen free of layout issues at every size, a turn taken back across a round
boundary, and a six-person party with its initiative restored from what was
written down. Seventy-nine `kobo-read` and twenty-two `kobo-bookview` tests
pass, with Feeds at ninety and Miniflux at twenty-six. The full workspace
suite, strict Clippy and `cargo fmt` pass.

`scripts/quality/check-grimoire-sim.py` drives the actual simulator through a
table session: open Magic items, read a reference, search the rules and read a
twenty-page section to its last page, find an edition with no spells and be
told why, open a 2024 condition and find text under it, add a sixth combatant
by typing, pass a turn and take it back, hand the turn to another combatant
from their own screen, damage a party member and spend a slot, then restart the
process and find all of it. It passes at the default text size, at 110% and at
170%; captures and results are in `evidence/grimoire/` and
`evidence/grimoire-large/`, and the screenshots were visually inspected.

### Paperterm and calibre-web re-verified after the reader change (11 September 2026)

Both groups were already complete. Both read through the same path the
`kobo-read` paragraph fix touched, so both were driven again rather than
assumed.

`scripts/quality/check-calibre-sim.py` passes all sixteen of its checks against
the private HTTPS OPDS fixture, including the shared reader, a saved position
reopened offline after a forced restart, a failed position save retried, a
damaged download repaired, and an authenticated catalog behind a server-bound
Basic account.

`scripts/quality/check-paperterm-live.py` passes all seventeen of its checks
with a real host PTY shared between a laptop terminal and the simulator over
trusted TLS: negotiated grids of 54 columns in portrait, both input directions,
reconnection with retained output, a paused input path that does not replay,
Ctrl-C from the reader, and the laptop terminal restored afterwards.

The remaining Paperterm work is the companion side, STREAMCLI-01 to
STREAMCLI-05, which belongs to PR 3.

### Feeds subscription transfer and the other half of what publishers serve (11 September 2026)

FEEDS-03 and FEEDS-06 are closed, which completes the Feeds group.

**Carrying a subscription list across.** The reader could already read an OPML
file waiting on its shelf; nothing could put one there. `kobo feeds check FILE`
and `kobo feeds push FILE (--device IP | --sim)` do, and they read the list with
the same parser the reader uses: the OPML reader moved out of the application
into `kobo-opml`, so a file the Kobo would refuse is refused on the computer,
where there is a keyboard and a full screen to say why. A list with nothing
usable in it, one that ends mid-document, one that is not OPML and one naming
only plain HTTP feeds are all refused before anything is transferred. The name
the file takes on the shelf is reduced to what a shelf name may hold.

**RSS and Atom, full articles and summaries.** The image journey now serves an
Atom feed beside the RSS one. Its first entry carries real markup and reads with
its figure, caption and alt text; its second offers a summary and nothing else
and reads as that summary with no picture and no claim of more. The picture that
entry names had already been saved for the other feed, and is not downloaded
again: the fixture records one request for the Atom document and none for the
image. After a restart with no network, both the article and its picture are
still there.

**Verification.** Eighty-eight Feeds tests and four `kobo-opml` tests pass, with
306 in the CLI including the new refusals and shelf-name rules. The full
workspace suite, strict Clippy and `cargo fmt` pass.

`scripts/quality/check-rss-sim.py` now stages its OPML file by running the CLI
against the simulator rather than writing the file itself, so the journey covers
the transfer as well as the import, and asserts the summary the CLI prints and
the name it wrote. `scripts/quality/check-rss-image-save-sim.py` carries the
four new Atom checks. Both pass at the default text size and at 170%; results
and captures are refreshed in `evidence/rss-offline{,-large}` and
`evidence/rss-image-save{,-large}`.

### Backgammon dice, board and match (11 September 2026)

BACK-01 to BACK-05 are closed, which completes the Backgammon group.

**The dice were not dice.** Both numbers came off a counter that advanced by
one each roll, so every game dealt the same sequence in the same order. They
come from the operating system's entropy source now. `KOBO_BACKGAMMON_SEED`
plays a fixed sequence for fixtures and captures, and the Match screen says so
whenever a seed is in use, because a recorded game that reads as chance and is
not would be a lie about the dice. A source that cannot be opened is reported
on the board and rolls nothing rather than falling back to anything.

**The board says what is happening.** Whose move it is is stated above the
board in words and drawn in the centre bar as that side's own checker. The
checker in hand is marked solid and the places it may go are ringed, which used
to be the same mark for both. The centre bar is wider so the dice and the cube
are drawn large enough to read, and both are written out under the board as
well: the board is 52 mm across, and a die drawn there is two millimetres.

**Setup left the board.** Who is playing, how long the match runs, the rules
and a new match are on a Match screen of their own. The board keeps Roll,
Double and Undo.

**What has happened is written down.** Each finished turn is recorded the way a
board writes it, "White 24/23 23/21" or "bar/20", the last one is shown under
the board, and the last eight are on the Match screen and survive a restart. A
match saved by the previous build is carried forward rather than discarded.

**Verification.** Forty-two Backgammon tests pass, including a seeded sequence
that replays and differs from another seed, a broken source that stops the turn
and says why, the record surviving a save and reload, every screen free of
layout issues at all nine text sizes, and the previous save format being
carried forward. Strict Clippy and `cargo fmt` pass.

`scripts/quality/check-backgammon-sim.py` drives the actual simulator with a
seeded run: it reads the opening board, changes the players on the Match screen,
rolls, takes a checker in hand, plays the turn out, reads the record of it, finds
the seed and the history on the Match screen, and finds the record again after a
restart. It passes at the default text size and at 170%; captures and results are
in `evidence/backgammon/` and `evidence/backgammon-large/`.

### Daily Brief source, time, offline state and reading (11 September 2026)

BRIEF-01 to BRIEF-04 are closed, which completes the Daily Brief group.

**A brief that says what it is.** The line above the stories names the list it
was drawn from and when it was fetched, read off the device clock at the time.
A brief with no time on it cannot be told from this morning's.

**Four lists, chosen rather than given.** The Source control offers the front
page, the best of the week, the questions and the things people built, all
public Hacker News indexes. Choosing another clears the stories that came from
the old one rather than leaving them under a new heading, says so, and writes
the choice down. Nothing is fetched until a refresh.

**A refresh that fails keeps the brief.** The stories on the panel stay exactly
as they were; the screen says the refresh did not happen, and the one control
on it offers another attempt. With no network at all, the saved brief and any
story already read both open with no request.

**Stories are read, not just listed.** A story opens in the shared document
reader with its figures, captions and type controls, and is saved as it is read,
so the second opening costs nothing and works with the radio off. A question or
a show-and-tell has no address of its own, so what the poster wrote is what is
read and nothing is requested.

Six headlines are one panel at most text sizes and two at the largest. The list
is measured under everything above it with `Context::paginate_ranked_rows_under`,
added for the purpose, because a ranked list measured as a marked one comes back
a row short: before this the sixth story was drawn under the Refresh button and
off the panel at 110%.

**Verification.** Fourteen Daily Brief tests pass, including every screen free
of layout issues at all nine text sizes, a failed refresh keeping the brief and
offering another attempt, a source change clearing what came from the old list,
a linked story fetched once and saved, a question read with no request, and a
brief written by the previous version still opening. Strict Clippy and
`cargo fmt` pass.

`scripts/quality/check-brief-sim.py` drives the actual simulator against a
private HTTPS fixture index: an empty brief that asks for nothing, a fetch, a
story read with its picture, a restart with no network where both the brief and
the story are still there, a refresh that cannot happen, a change of source, and
a question read from what the poster wrote. `KOBO_BRIEF_ORIGIN` points the
application at that fixture; unset, it is Hacker News. It passes at the default
text size and at 170%; captures are in `evidence/brief/` and
`evidence/brief-large/`.

### Tic-tac-toe session, win state and one-player mode (11 September 2026)

TIC-01 to TIC-04 are closed, which completes the Tic-tac-toe group.

**The session is kept.** The line under the heading says how many games each
side has won and how many were tied. Next game clears the board and keeps that
count; Clear score starts the afternoon again. Both survive closing the
application, and a finished board is counted once however many times it is
tapped afterwards. The board itself is deliberately not saved: nobody comes
back to a half-played game of noughts and crosses.

**The end of a game is visible.** The three squares that won it are marked on
the board, and the heading names the winner in the words of whoever is playing:
"Your turn, playing O" and "The Kobo wins" in one-player mode, "O to play" and
"X wins" in two.

**One player.** Against the Kobo hands the crosses to the device. The opponent
takes a win when it has one, blocks a loss when it must, and otherwise plays the
middle, a corner, a side. It can be beaten, which is the point of playing it,
and one tap leaves the board with its answer already on it.

**Still the floor.** The application remains a grid and a few lines of text
against the public builders, and its committed route now plays a game out to a
win, checks the score, starts the next, hands over the crosses and clears the
score. The route taps squares by name rather than by coordinate, so a line of
text added above the board no longer re-breaks it.

**A platform fix it caught.** Marking the winning line showed that the glyph in
a chosen cell was drawn in paper whatever the cell was filled with, so three
noughts on a shaded row came out as three empty squares. `kobo-ui` now inverts a
cell mark only where the cell is drawn on ink, with a rendering test that counts
the ink inside a chosen square. Any board or pad using a glyph with selection
was affected.

**Verification.** Fifteen Tic-tac-toe tests pass, including the winning line
marked at all nine text sizes with no layout issues, a game counted once, a
rematch that keeps the score, the session written and read back, the opponent's
priorities, and a solo tap answered before the panel is drawn again. The 274
`kobo-ui` tests pass with the new one. `scripts/check-apps-sim.py tictactoe`
passes against the committed route; captures are in `evidence/tictactoe/`.

### Magnet sweep guidance and controlled sensor events (11 September 2026)

MAGNET-01 to MAGNET-04 are closed, which completes the Magnet group.

**Guidance that points somewhere.** The reader is drawn with one edge marked as
the one to sweep, and Sweep the next edge walks round the four. Nothing claims
to know where the sensor is: the profiles describe the panel, not the magnet,
and the bezel says nothing. What the application does is help somebody find it,
credit a change to the edge that was being swept when it happened, ring that
edge on the diagram, and write the finding down so the next opening starts
there and says "Found on the right edge".

**Told apart.** A reader with no hall sensor, a build that cannot read it, an
application that did not ask for it and a read that failed each say their own
sentence, and none of them is "No magnet": a screen that says there is no
magnet before it has asked is guessing. Nothing is claimed before the first
answer arrives.

**The count.** Still resettable, and now kept per edge, because how many times
it moved matters less than where it was when it did. A restated state is not
movement, and the first answer is not a change.

**Controlled events.** `examples/magnet/drive.txt` drives the simulator's own
hall-sensor controls: sweep the top edge, close the cover, see the magnet and
the ring, open it, move to the right edge, close and open again, check the
count, and clear it. `scripts/check-apps-sim.py magnet` passes against it.

**Verification.** Eleven Magnet tests pass, including a change credited to the
swept edge and written down, the finding read back so a calibrated reader
starts at the answer, the sweep walking the four edges, the diagram marking the
swept edge and ringing the one that answered, and every state free of layout
issues at all nine text sizes. Strict Clippy and `cargo fmt` pass. Captures are
in `evidence/magnet/`.

### A components reference that fits every panel it ships on (11 September 2026)

GALLERY-01 to GALLERY-05 are closed, which completes the Components reference
group.

**Every control, and the proof that it is here.** `every_control_the_toolkit_draws_is_somewhere_in_this_gallery`
reads the builder's own source and fails if a method that draws something is
not called by the gallery. Eight are excused by name, each with a reason: they
start a screen, end one, or set a property of one rather than drawing a
control. The vocabulary test beside it does the same job from the other end,
by node kind. A control added to the toolkit and left out of this application
now fails a test instead of shipping unseen.

**Pages that fit the device, not the desk.** The reference no longer decides in
advance how many panels a page takes. Each page is a list of parts, and the
parts are dealt out into panels by measuring: parts are added until the
runtime's own diagnostics say something has been pushed off the panel, and then
a new panel starts. `every_page_fits_every_supported_panel_at_every_text_size`
lays out all forty-odd screens on all eight supported panels at all nine text
sizes, and the pages turn where they have to: the same reference is thirty-one
panels on a Clara BW at the default size and thirty-seven at 170%.

**A job from end to end.** Choosing a book, being asked to confirm it, watching
it arrive, being told it is there and opening it. Each step is drawn with the
controls the rest of the reference shows, and each has a way back out, which
the journey walks in both directions. A reference made only of pages of
controls says nothing about how they follow one another.

**Labelled diagnostics.** Two warnings are provoked on purpose: the tone budget,
because a shelf of tiles under a marked navigation bar spends all five inks and
that is worth seeing, and the two primary actions on the buttons page, which is
the same verb drawn twice with one of them refused.
`every_warning_this_reference_provokes_is_one_it_means_to` fails on any other.

**Platform fixes found by this work.** Three, all in the toolkit rather than in
the application. A menu was measured with a narrower padding than the buttons
inside it are drawn with, so at the largest text setting "Rename" was broken
across two lines in the middle of the word. A dialogue's title was cut to one
line whatever it said, so "Download Mrs Dalloway?" reached the reader as
"Download Mrs"; a modal now takes a second line for its question, and a
popover's title, which is a label over a short menu, still does not. A selected
pencil mark with no action of its own is rejected by the protocol, which the
reference was doing and nothing had caught.

**Two doc comments corrected.** `table` did not say what its `weights` are:
they are the widths in pixels that named columns ask for, not proportions, and
the reference had passed `vec![1, 1]` meaning "two equal columns" and drawn two
one-pixel ones. And `controls` claimed the label always stays, while the
renderer draws the picture alone by a deliberate rule stated where it is drawn.
The stepper's own explanation had come adrift onto `table` and is back.

**Verification.** Thirteen gallery tests pass, including the sweep across every
panel and text size. The 278 `kobo-ui` tests pass with the two new ones.
`scripts/check-apps-sim.py gallery` passes against the committed route, and
`scripts/quality/check-gallery-sim.py` walks every page of all five
destinations, every panel they turn onto, all thirteen sheets of icons and the
whole job, at the default size and at 170%. Captures are in `evidence/gallery/`
and `evidence/gallery-large/`.

### A story, its discussion and what the device remembers (12 September 2026)

HN-01 to HN-06 are closed, which completes the Hacker News group.

**The headline, once.** It was in the top bar and in the first paragraph of the
discussion, which is the same words twice on one panel with one of them cut
short, and it cost the first page a headline's worth of comments. It stays in
the bar, because that is the part of the screen that survives a page turn: a
reader four pages into a thread should not have to page back to find out whose
story they are arguing about.

**The story and the discussion are two places.** A row opens the discussion,
which is what this application is for. The article behind the story is offered
on the story's own screen, beside Save, and opens in the shared reader with the
same type sizes and the same front light control as every other book on the
device. A story that is its own text says so instead of offering to fetch an
article that does not exist, and a site that will not answer says which failure
it was and leaves the discussion where it is.

**What the device remembers.** Which stories have been opened and which were
put aside, in one small state file. Both are said at the front of the row's
second line, where an eye running down the left edge of a list finds them, and
only on the rows that have them. A saved story is written down whole, with its
article beside it, so the Saved list draws with no radio at all and the article
opens from the copy on the device. Coming back from a story lands on the page
of the list it was opened from.

**Deep replies and folding.** A reply deeper than the gutter can show says how
deep it is in its byline, which costs no width, and the drawn indent stops
moving at the cap rather than pretending to keep going. Any comment folds away
with its replies and opens again with them.

**Two defects found by this work.** Tapping Saved while the front page was
still arriving showed the saved list and then the front page a second later,
because a ranking that lands after the reader has moved on put its own tab back
on the panel. And the discussion's messages were drawn as a banner the
paginator had never measured: on a full page at 170% the renderer refused the
whole screen rather than draw through the panel edge, so a reader who turned
the type up got nothing at all. Everything the screen has to say is now said
inside the flow that was measured, and the end of a thread is written at the
end of the thread rather than raised over the top of it.

**Platform.** A new `paginate_tagged_under`, for a screen that draws a block of
its own above the first page of a long piece of prose and nothing above the
rest. The application had been reserving paragraphs of roughly the right height
and swapping them for the real block while drawing, which is a dozen pixels out
over four facts, which is one line too many at the foot of the first page. The
same measurement the other `_under` paginators already use now serves this one,
and the reservation and the tag that went with it are gone. The loading
placeholder is measured too: six rows fit a Clara BW at the default text size
and run off the bottom of it at 170%, where the renderer refuses the screen.

**Verification.** Sixty-three Hacker News tests pass, including the round trip
of a fold, a reply past the indent cap saying how deep it is, the list position
kept across a story, the saved list asking the network for nothing, and a story
with no link of its own. `scripts/check-apps-sim.py hn` passes against a
committed route that is deliberately offline, and
`scripts/quality/check-hn-sim.py` reads a private HTTPS fixture end to end at
the default size and at 170%: the front page, a discussion six replies deep,
the article behind it, saving, the marks on the list, and the saved story and
its article reopening with the radio off. Captures are in `evidence/hn/` and
`evidence/hn-large/`.

### A list that can be changed, dated and taken off the reader (12 September 2026)

TODO-01 to TODO-06 are closed, which completes the Todo group.

**The empty list offers the one thing to do about it.** A plain button beside
nothing at all reads as a footnote, so on an empty list Add is the dominant
verb and says what it adds.

**Changing the list is its own screen.** Edit opens the list in the order it is
kept rather than grouped by what is finished, because that is the order this
screen can move. Tapping an item there opens that item: rename it, move it up
or down, give it a date, or take it off. The list itself stays a list, where
every row is one tap and that tap means the one thing a list of things to do is
for. Renaming opens the field on the words already written, so fixing a typo is
two taps rather than typing it again, and it does not move the item.

**Dates, said against today.** Today, tomorrow, in a week, or none at all, as
four chips with the current one drawn as chosen by the renderer. A row says
"due tomorrow" or "3 days late" rather than a calendar date, because a list of
things to do is read against today and nothing else. A finished item keeps its
date to itself: what is due is what is still to do.

**Undo.** Clear finished keeps what it took until the next change and offers it
back. One undo, in memory only. An undo that outlives what it was undoing is a
trap, and a confirmation before a button pressed once a week trains everybody
to answer without reading it.

**A copy for the computer.** Save a copy offers the whole list as plain text
through `kobo_sdk::exports`, which is the platform's own owner-initiated
export: the copy is verified and acknowledged before it is offered, and the
original stays on the reader. This is the first application to use it.

**Measured capacity.** How many rows a page holds was a constant. Six fits a
Clara BW at the default text size and four fit it at 170%, so a list of long
items drew its last rows through the buttons under them and the renderer
refused the whole screen: a reader who turned the type up got a blank panel.
The list is now measured under everything it is drawn around, the chips above
it and the buttons below, and it says which page it is on rather than paging
silently.

**Three defects found by this work.** The keyboard had no `#` on either layer,
so the tags this application groups by could only be typed by somebody who had
written their list somewhere else and imported it. A button was drawn one line
tall whatever its label said, so "Clear finished" beside another control at
170% was two lines of text in a one line box, which the renderer refuses
outright. And every store answer was read as though it were the list, so
saving a copy emptied one: the key is checked now, and anything else is
offered to the export that asked for it.

**Verification.** Fifteen Todo tests pass, including every screen laid out at
all nine text sizes with no layout errors, the round trip of a date through the
state file, a list written by the version before dates still reading, the undo
and its expiry, and an item renamed, moved and dated from its own screen. The
277 `kobo-ui` tests pass with the new one, and the keyboard's own test now
asks for every character an application makes somebody type. `scripts/check-apps-sim.py todo`
passes against the committed route, and `scripts/quality/check-todo-sim.py`
writes three items with tags, ticks one off, undoes a clear, edits, dates,
moves and renames an item, offers the list to a computer and finds everything
after a restart, at the default size and at 170%. Captures are in
`evidence/todo/` and `evidence/todo-large/`.

### A deck that says which computer it is talking to (12 September 2026)

DECK-01 to DECK-06 are closed, which completes the Deck group.

**Places, and keys.** Fifteen places are drawn because that is the shape of the
panel. The ones nobody has assigned are now drawn in a hairline rather than the
bezel a key gets: a deck with three things on it used to read as twelve
controls that do nothing. The renderer decides that from the cell itself, so
any application whose deck is half empty gets the same treatment.

**Which deck, and whether anyone is listening.** The bar carries the name of
the page showing rather than the word Deck, and one line under the tabs says
which computer this is paired with and whether it answered. A key that does
nothing because the computer is asleep looked exactly like a key that does
nothing because it has not been assigned.

**What a key did.** A key that has run opens what it said, whether it worked or
not; it used to do that only when the command had failed, so everything that
worked was silent. The line under the deck says what the last one did. The
status character that used to be appended to the key's own label is gone: a pad
carries a single line, so the newline in front of it was drawn as a box with a
cross in it on every key that had ever been pressed.

**Asking first is kept.** A key marked confirm on the computer raises the
question here and runs only when it is answered, and the computer can refuse a
press that did not carry an answer. Both paths are tested.

**Presets.** `kobo deck init --preset build` and `--preset home` write a deck
that does something on the first press, which is the alternative to twenty
minutes of `kobo deck set` before anything works. Neither writes over a deck
that already exists.

**One number for the pads.** The companion allowed twelve pads a page and the
reader's deck draws fifteen places, so three of them could never be filled and
nothing said so. The companion allows fifteen now, which is what the panel
shows.

**The defect this found.** The deck asked the computer for its keys again the
moment an answer landed: three hundred and seventy requests went out in the ten
seconds it took the harness to notice, on a device whose radio is the largest
single draw on its battery. It waits five seconds between looks now, and the
same journey costs nine requests.

**Verification.** Fourteen Deck tests pass across the application, its model
and the companion, including the paced polling, a confirmed key that does not
run until it is answered, a finished key opening its output, and every pad the
reader can see being assignable from the computer. The 278 `kobo-ui` tests pass
with the new one. `scripts/check-apps-sim.py deck` passes against the committed
route, and `scripts/quality/check-deck-sim.py` pairs with a private HTTPS
fixture, presses a key, answers the one that asks, reads what it said and finds
the deck again after a restart, at the default size and at 170%. Captures are
in `evidence/deck/` and `evidence/deck-large/`.

### Answering an agent from the armchair, with its name on it (12 September 2026)

SIDE-01 to SIDE-05 are closed, which completes the Sidekick group.

**What to run, by name.** The first screen said "Open Sidekick in the Cobalt
desktop app", which is true and useless: what has to happen is that a daemon is
running on the computer, so the screen names `kobo-sidekickd init`, which is
the command that starts it and prints the two things the screen then asks for.

**Who is asking.** A question now says which terminal, on which computer, as
well as which tool. With one agent the tool was enough; with three of them on
two machines, "shell asks" is not a question anybody can answer. The board
already carried the session on every row, and the question itself did not.

**Two at once.** Two terminals asking at the same time are both on the board,
each named, and answering one answers that one: the other is still on the
daemon and comes up next. The journey drives exactly that, and a test holds the
same shape.

**An answer that arrived too late.** The daemon acknowledges an answer only
when it still had the question. Without that acknowledgement the panel says the
question was gone before the answer arrived rather than claiming a decision
nobody received, and the journey provokes it by taking the question away
between the tap and the post.

**Two defects found by this work.** The poll loop had no floor: against a
daemon that answers immediately rather than holding the request open, and while
a board of questions was showing, the reader asked again the instant each
answer landed. Three hundred and sixty requests went out in ten seconds with
the panel sitting still. There is a two second floor now, which costs nothing
against a daemon that long-polls properly. And at 170% the watching screen was
refused outright by the renderer once it had an answer to report, because a
four line splash subtitle plus two sections plus a button is more than a six
inch panel holds: the splash keeps its sentence only while it is the whole
screen, and the last answer is cut to what the panel will actually hold rather
than to a character count.

**Verification.** Thirty Sidekick tests pass, including a new sweep that lays
every screen out at all nine text sizes, the question naming its terminal and
computer, two terminals answered independently, and the empty poll taking a
breath before asking again. `scripts/check-apps-sim.py sidekick` passes against
the committed route, and `scripts/quality/check-sidekick-sim.py` pairs with a
private HTTPS fixture daemon, answers a question, takes two at once, leaves one
for the terminal and finds that an answer the daemon no longer holds is not
reported as a decision, at the default size and at 170%. Captures are in
`evidence/sidekick/` and `evidence/sidekick-large/`.

## Parlor: four games on one panel

**A shelf rather than a list.** The table used to be four rows, each carrying a
sentence about its game, and at the larger text settings the fourth row was
drawn through the bottom edge and the renderer refused the screen. Four games
are now four cards on one shelf, each with its own mark and three words saying
what the game is: bracket and flip, captures are forced, mills then flying, sow
and capture. Nothing pages, because nothing has to: a shelf of four fits at
every text size the reader offers, and what a game is in full belongs on the
screen the card opens.

**A mark for each game.** Reversi wears a disc, draughts a crowned man, Kalah a
grid. Nine Men's Morris wore a single board point, which is a full stop at card
size, so the renderer now draws a mill: three points on a rule, the one shape
the game is played for. It is a platform glyph rather than a picture in one
application, because the next board game to want it should not draw its own.

**Whose move it is.** The top bar carried the game and the turn together, and
on a six inch panel at 170% it cut the turn off mid-word: two people passing a
reader between them lost the one line they have to read. The bar carries the
game; the turn is the first line under it, with the match score at the end of
the same rule, and what to do next sits under that.

**A board this panel cannot draw.** International draughts needs a hundred
touch cells and this panel holds eighty-one. Choosing it used to raise the full
error panel, cross and "Something went wrong" and all, which reads as a broken
application rather than an option that is not on offer, and which was itself
drawn through the bottom edge at 170%. The row says it plainly, a line under it
says why, and no button offers to start what cannot be drawn.

**Leaving a game.** The board offered Undo, Moves, New game and Games, and the
fourth wrapped onto a second row that was drawn off the panel at the larger
sizes. Leaving is the chevron in the bar, as everywhere else on the reader.

**Two defects found by this work.** A capture chain in Anglo-American draughts
was rejected by the position's own validity check, so the game could be played
but never saved: a reader who put the panel down mid-chain lost the game.
And Nine Men's Morris had no way to end: two players shuffling between the same
points could slide forever. There is a fifty-move quiet rule now, and the draw
it produces is a result the match score records.

**Verification.** Twenty-two Parlor tests pass, including a sweep that lays the
shelf out in both its states, every setup screen including the refused ruleset,
every board and the move record, at all nine text sizes on the smallest panel.
`scripts/check-apps-sim.py parlor` passes against the committed route, and
`scripts/quality/check-parlor-sim.py` opens all four games, reads what the panel
says about each, takes a real turn found from the marked legal squares rather
than from a fixed coordinate, closes the application and picks the game up where
it was left, at the default size and at 170%. Captures are in `evidence/parlor/`
and `evidence/parlor-large/`.


## Lichess: recover a match while the seek is still open · 12 September

The previous fallback read current games only after the seek request ended.
A missed account-stream start event could therefore leave the panel waiting
while the seek connection stayed open. Pairing now schedules a current-games
read every ten seconds. Empty results preserve an active seek; a unique new
match opens its board and cancels the seek. Ambiguous matches cancel the seek
and return to the game list. Cancellation stops the checks, and an ended seek
still reconciles before reporting no match. The seek POST is never replayed.

**Validation:** all 105 Lichess tests pass on Rust 1.85.1; strict Clippy for all
app targets passes. New SDK-runner regressions cover recovery before the seek
ends, one POST only, an empty active check, and cancellation. An additional
regression covers ambiguous matches. The offline checking screen was driven
and visually inspected on Clara BW metrics at default and 170% text size;
both diagnostics contain no errors. Reproduce those captures with
`scripts/quality/check-lichess-pairing-sim.py --output /tmp/lichess-pairing`.
Evidence is under `evidence/lichess-recovery/{default,170}`. These are demo
captures, not live-service or physical-device proof. LICHESS-06 remains open.
The existing uncommitted tile-label patch was present during host checks and
is deliberately excluded from this change.


## Lichess board and offline journey — 2026-09-12

The board now uses joined alternating squares, equal-sized legal-move hints,
and a visible selection border. Only the active clock is filled. Player rows
fit with the board at enlarged text sizes. Leaving an unfinished computer
game and choosing Computer again preserves its position.

The complete simulator drive passed on Clara BW metrics at normal and 170%
text size: select e2, play e4, receive a computer reply, leave and resume,
inspect the menu, cancel resignation, then resign and dismiss the result.
Both runs have no diagnostic errors. Evidence: `evidence/lichess-computer`.
The latest combined unit suite passed 107 Lichess and 279 UI tests (two UI
tests ignored). All nine text sizes exercise board geometry in runner tests.

One authorized live seek reached a real board, with zero moves sent. The
application subsequently showed Game aborted; the test did not initiate
that abort. The server then reported no active games. The original harness
misidentified the overflow control, so this is not proof of UI abort handling.
Credential-free results are in `evidence/lichess-live/result.json`. Physical
Clara BW and further recovery acceptance remain open.

The pre-existing tile-title patch is now completed as part of this renderer
change: the label node records its actual wrapped line count, so short titles
are not mistakenly painted in a larger font. Its real-title regression now
passes. This supersedes the earlier note excluding that unfinished patch.

Strict Clippy for all Lichess and UI targets also passes on Rust 1.85.1.

Lichess checklist reconciliation: 107 app tests pass on e33fe681. Normal and
170% screenshots show legible selected-square borders and legal-move dots.
Authoritative acknowledgement and stale-state/reconnect fixture tests pass.
LICHESS-03/04 are complete; remaining Lichess items stay open. No live API calls
or moves were made in this check. PR 3 evidence:
https://github.com/BandarLabs/Cobalt/tree/beta-quality-apps-2/docs/quality/evidence/lichess-todo-review
Cross-PR tracker totals reconciled: 229 done, 266 open, one deferred.

Lichess 1.0.7 fixes Resume current when only a stored session exists. The fresh
app now opens the board stream rather than merely switching screens. A new
regression loads actual SDK store-save bytes, restores the authoritative
position and verifies cleanup on confirmed completion. All 108 app tests and
strict all-target Clippy pass. Full restarted simulator transport acceptance
remains open under LICHESS-05; no live requests or moves were made.

Crossword completion: clear active-word shading once solved and restore it
when edited. All 12 app tests and strict Clippy pass. The blocked 5x5 puzzle
was solved by typed answers in the offline simulator at normal and 170% text
size; screenshots and diagnostics are in `evidence/crossword-completed`.
The capture harness builds the CLI from the app checkout to avoid sibling
branch renderer mismatches. Failed preliminary captures are not acceptance
evidence. Lichess restart transport acceptance remains open: existing static
demos and error scenarios do not supply an offline HTTP line-stream fixture.

Simulator capture provenance: Crossword completion, Lichess computer play and
Lichess pairing recovery harnesses now build the CLI from the current checkout
and use Cargo's reported executable. Each result records the source revision,
whether tracked changes were present, Rust toolchain and executable SHA-256.
A post-capture fingerprint check rejects replacement of a shared target binary;
use a dedicated CARGO_TARGET_DIR when captures run beside other builds.
All three harnesses pass at normal and 170% text size. A deliberate executable
replacement was correctly rejected. Refreshed screenshots and result metadata
are in the corresponding evidence directories. These remain offline presentation
checks, not live matchmaking, session-restart transport or hardware acceptance.

Offline stream fixture transport: debug simulator builds can route one exact
HTTPS origin to a numeric loopback endpoint while preserving TLS verification,
HTTP framing and runtime credential policy. Other destinations fail closed.
The new integration test passes real GET, authenticated POST and retained
NDJSON requests and checks Host/SNI preservation and destination refusal.
All 104 kobo-net tests pass (91 unit, 13 integration); strict all-target
Clippy and release-profile checks pass for kobo-net and kobo-sim.
LICHESS-05 stays open until the actual
app completes and resumes a fixture game through this transport.

Lichess session acceptance is complete for LICHESS-05. The actual SDK app and
simulator pair against a private TLS fixture, send exactly one move, retain the
initial board after POST success, render the acknowledged e4/e5 position,
restart with the same private store, restore that position from a fresh board
stream, accept a draw, remove the saved session and resume account polling.
Normal and 170% runs pass; screenshots and request receipts are in
`evidence/lichess-session`. All 109 app tests and strict Clippy pass.
The fixture exposed and verified fixes for account polling occupying the move
task slot and duplicate confirmation copy clipping the clock at 170%.
No public Lichess requests or moves were made. Physical acceptance remains
separate. Tracker totals: 230 complete, 265 open, one deferred.

LICHESS-06 is complete: local TLS fixtures omit all gameStart events and still
open the matched board at normal and 170% text sizes without a duplicate seek
or account recheck. Full move acknowledgement, restart, draw and post-game
polling checks pass. Evidence: `evidence/lichess-missed-start`. The fixture
exposed background account polling starving the ten-second recovery timer;
Lichess 1.0.9 pauses that polling during pairing and schedules the recovery
check after cancellation releases capacity. All 110 app tests and strict
Clippy pass. Tracker: 231 complete, 264 open, one deferred.

PR #181 host job 103551595707 failed because generated Crossword and Lichess
pages contained old app versions. Regenerated `docs/apps` from the current
manifests and verified a second generation makes no changes. This repairs the
observed generated-page failure; the new CI run must still complete.

LICHESS-02 is complete. The real TLS fixture now checks active-side clock
selection after each acknowledged move, disconnects the board stream, verifies
retained piece positions and unconfirmed clocks, attempts moves while offline,
and verifies restored clocks and cleared guidance after reconnect. Normal and
170% runs pass. A regression verifies finished boards do not say “Paused” or
offer Reconnect; all 111 app tests and strict Clippy pass. Evidence and captured
layouts: `evidence/lichess-connection`. Lichess 1.0.10 uses short reconnect copy
and full-width clock placeholders to preserve touch targets and large-text
layout. Generated app pages were updated. No live service was used.
Tracker: 232 complete, 263 open, one deferred.

Lichess 1.0.11 simplifies pairing and recovery guidance, removes the duplicate
checking banner and separates clock settings from the game type to avoid an
awkward enlarged heading wrap. Rate-limit guidance no longer incorrectly says
pairing was cancelled. Normal and 170% checking-screen captures pass; all 111
app tests and strict Clippy pass. Updated screenshot evidence is in
`evidence/lichess-recovery`. LICHESS-01 remains open for the rest of its guidance
review; counts remain 232 complete, 263 open, one deferred.

LICHESS-01 is complete. Reviewed pairing, challenge, saved-game and connection
recovery guidance; removed protocol vocabulary and clarified ambiguous matches.
The failed-check screen and explicit cancellation pass at normal and 170% text
sizes with clean rendering/touch diagnostics. Evidence:
`evidence/lichess-pairing-guidance`, alongside the prior checking-screen and
real TLS recovery evidence. All 111 app tests and strict Clippy pass.
All six Lichess app-quality tasks are now complete; companion work and physical
acceptance retain their own scope. Tracker: 233 complete, 262 open, one deferred.


### Companion checklist reconciliation

Synced PR 4 task records through companion commit `f91c56af`: app setup cards,
provider help, Sidekick selection/help/backups/sample, and Deck pairing,
preview, limits and confirmation preferences. Ten additional tasks are done;
credential transfer recovery remains partial CLI-18. Completion evidence and
implementation live on `beta-quality-companion` in PR #182. This metadata
update does not import companion changes into PR #181 or certify hardware
acceptance. Global counts: 243 done, 252 open, one deferred.

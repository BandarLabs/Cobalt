# Shared app contracts

Use these contracts for the catalog work in PR 2. App-specific content, routes and provider behavior stay in the app. Layout measurements, reading, page navigation and result-state conventions come from shared components.

## Reading and collections

- Use `kobo_bookview::BookView` for text, Markdown, HTML and EPUB documents. Its `open_bytes` delegates format handling to `kobo-doc`; `open_html` also coordinates remote figures. Do not put an unbounded article into a column of text nodes.
- Persist `BookView::memory()` after reading actions that return `Outcome::Save`. Restore that memory when reopening. `BookView::reflow` remeasures the open document for changed panel/orientation metrics while preserving the block anchor, book text size and annotations.
- Use `Context::paginate_rows` or its trailing-control variant with the actual row titles/subtitles. Feed its index pages and stable provider/local record IDs into `collections::PagedCollection::reflow`. Render only `visible()` indexes. Select the stable record ID before opening details; retain `position()` so returning and refreshing keep the owner's item in view.
- Empty collections have zero pages. Show a useful empty state and its next action; never show “Page 1 of 0” or an error for a library that has never been populated.
- A removed record clears selection and clamps the remaining page. A new sort order or larger text size follows the record anchor. Reject duplicate provider IDs and incomplete pagination instead of quietly reassigning actions to another row.
- Comic archive handling lives in `kobo-comic`; Panels remains the comic app. CBZ uses the dependency and bounds described in the [comic decision](comic-reader-decision.md). Use `kobo_bookview::comic::ComicView` for shared reading controls. Local import adoption remains app-specific work.

## Layout and controls

`DisplayMetrics`, physical spacing/hit-target tokens and semantic font roles in `kobo-ui` are authoritative. Use the same panel metrics and chrome when measuring, diagnosing and rendering. A scoped typography environment now applies the interface and reading sizes at all three entry points, and restores the caller's environment afterwards.

Use one primary action when a screen has a clear next step. Keep routine secondary actions outlined, grouped by purpose and in a stable order. Preserve navigation while content loads; changing content height must not move a destructive action under the owner's finger. Keep confirmation close to the affected action and name what will be removed. Reserve trailing controls during pagination instead of placing them after an unbounded list.

Set `owns_back(true)` on app routes with an internal return destination. The shared `Chrome::for_screen` composes the same visible Back control in runtime and simulator; the app must handle the resulting `ActionId::BACK`. A root screen returns to the launcher. The full runtime still owns escape/watchdog behavior; single-app simulator parity for every escape case remains open.

## Results and copy

Use an activity state while work is in flight, a receipt after acknowledged completion and a recoverable explanation after failure. “Saved” requires a successful store response. “Synced” requires a validated provider response and locally acknowledged state. An offline queue is “Waiting to sync”; it is not a completed sync.

An expected empty library, an unsupported format, a corrupt record, a missing sign-in and an unreachable provider need separate outcomes. Keep the current content available when a refresh fails. Explain the next useful action in the same screen: add a file, finish sign-in, retry, export unsaved work or open a supported copy.

Use ordinary task language in owner screens and commands. Name the document, destination and result. Keep internal handles, transport names, package manifests and decoder details in diagnostics. Avoid celebratory status messages, generic feature claims and claims of completion before the work is verified.

## Validation

Exercise each adopted contract with enough records/text to require several pages, at the owner's default and extra-large interface sizes. Test deletion of the selected record, refresh reordering, return from details and reopen from saved position. Drive the actual hit target and assert the resulting content. Query serious layout diagnostics at each transition; a screenshot that merely looks plausible is insufficient.

The shared text/Markdown/HTML reading pipeline already existed on beta. Foundation work reuses it and adds an explicit orientation reflow entry point plus regression coverage. This is not a second document renderer. See [the validation log](validation.md) for executed checks and remaining hardware work.

## Saving drafts and queued provider changes

`kobo-state::draft::Draft` tracks the latest owner edits separately from the one write in flight. Give it the app store's byte limit. `begin()` returns a revision and bytes; retain that revision alongside the specific store request. Feed only that request's result to `finish`. An older successful write leaves subsequent edits unsaved. On failure, keep the draft on screen, offer retry, and export `bytes()` if requested. An export is not a save acknowledgement.

`kobo-state::outbox::Outbox` provides bounded, versioned snapshots for idempotent provider changes. Queue record intent without credentials. Save the bytes returned by `checkpoint()` under one app-owned key and serialize writes to that key. Call `saved(checkpoint.revision)` only after the matching successful store response. Until then, `begin()` releases no remote work. Call `finish(sequence, result)` after checking the provider's status/body; acknowledge a successful mutation only when the provider accepted it. Save the resulting queue again before releasing more work.

A crash between provider acknowledgement and local removal can replay a mutation. The app must use an idempotent state-setting API or a provider idempotency key. “Archive this record” is suitable; “create another message” without provider deduplication is not. A newer value cannot overwrite an in-flight request. Retry and conflict states persist, and conflicts require reconciliation before an explicit retry. Corrupt or newer-version snapshots return an error; keep their bytes for recovery instead of silently creating an empty queue. Existing app-specific legacy queues still need migration during adoption.

The SDK now composes its provider and import modules from Cobalt's own network, JSON and state crates. No additional third-party runtime dependency is introduced by these helpers.


## Provider setup and connection checks

`kobo_sdk::provider::ProviderSetup` composes the shared text entry, runtime-owned credential entry and bounded network tasks. Supply the service name, authorized credential name and a fixed account-probe path; apps must use the provider's correct authentication contract. Use `with_authentication` for Basic authentication or a named API-key header; bearer is the default. The runtime still enforces destination and credential authorization.

`AddressChanged` asks the app to save a validated HTTPS base address through its normal acknowledged configuration. Query tokens and embedded credentials are refused without echoing them in error text. `Response` is a completed HTTP request, **not** a successful connection: parse the expected provider response, then call `verified()` or `invalid_response()`. Account details saved by the credential flow still need a separate connection check. Cancel/Back stops the outstanding task; late responses cannot change the next address's status. Missing credentials and rejected accounts point to the reachable account control. Offline state offers the existing Wi-Fi route.

## Versioned records and caches

`kobo_state::record::Schema` bounds a JSON envelope, requires its identity and positive version, and rejects ambiguous duplicate keys. `restore(None, ...)` is expected first launch; empty, corrupt, oversized, wrong-schema and newer-version bytes remain distinct errors. Migrations explicitly convert version N to N+1 with bounded intermediate records. The original bytes stay with the caller until the replacement is saved and acknowledged. Do not automatically reset unreadable records.

Use the existing `AppStore::cache` namespace only for refetchable articles and assets up to the 256 KiB encoded value limit. It holds at most 64 keys (at most 16 MiB of values); oldest-written cache entries are pruned at the key cap while durable reading positions retain their separate allowance. Failed reads no longer pretend to be an empty library, and failed removals no longer report success. A cache can still be absent or fail to write. Use the shelf for larger assets and explicitly retained offline documents; app-specific cache freshness/pinning and content migration remain part of adoption.

## Verified local imports

`kobo_sdk::imports::Import` accepts app-validated document bytes up to the shared 32 MiB in-memory shelf limit. It supplies preview, copying, checking, failed and available screens. Preview does not write. On confirmation, it moves the byte buffer into a chunked upload, releases that buffer, then reads the shelf back and compares a SHA-256 digest using Cobalt's existing digest implementation. It saves a versioned receipt only after the bytes match. `is_available()` becomes true only after the receipt save is acknowledged.

Route `KoboApp::on_shelf(context, name, result)` into the matching import, and its `on_save(context, key, result)` into the receipt. Both callbacks retain the requested name even for keyless wire refusals. Existing apps receive `on_store` by default. Retrying a failed receipt save writes only that receipt. A hash mismatch offers selecting the original file again; it does not retry the same damaged bytes. Cancellation stops further chunks and ignores late responses. It does not erase an earlier identical import or automatically delete an unreferenced blob.

Persist/restore receipt keys as part of the app's library. `Receipt::restore` validates the record but does not assert that its file still exists; `Import::verify_existing` rechecks the actual file after reopen. The app remains responsible for parsing the document, its library index and removal workflow. A transfer receipt never claims that an unsupported document format was successfully decoded.


## Time and entropy

Depend on `kobo_sdk::clock::Clock` for date and elapsed-time decisions. `SystemClock::new(offset_minutes)` reads the operating-system clock and process monotonic time; use the owner's configured UTC offset explicitly. `Snapshot::date()` returns a Gregorian date rather than a hardcoded daily-content date. `ManualClock` is an injectable test source: reads never tick it, `advance` updates wall and monotonic time atomically, and `set_wall` corrects the calendar/offset without changing a countdown. The supported civil range is 1970–9999. Persist wall-clock deadlines for work that must survive process restart; process monotonic values are not durable timestamps. Time-zone/DST policy belongs to the host, not guessed by the SDK.

Use `entropy::SystemEntropy` for game choices and propagate source failures. `Entropy::below` uses bounded rejection sampling rather than biased modulo-only selection. `FixtureEntropy::new(seed)` is explicitly for deterministic tests and samples; it must never generate credentials, pairing secrets or security identifiers. Apps must select fixture entropy explicitly. Merely recording a requested seed in capture metadata does not mean an app has adopted it.

For simulator sleep tests, start `kobo dev` with `KOBO_SIM_CLOCK_MILLIS=<Unix milliseconds>` and optional `KOBO_SIM_UTC_OFFSET_MINUTES`. Drive `clock advance 60000` or `clock set 1704153600000 330`, then assert `/clock` fields with `expect-state`. One advancement completes sleeps due at that point; a callback that schedules another sleep starts its next deadline at the new time. The driver waits for resulting callbacks. Individual advances are capped at seven days. Real network deadlines are unchanged; existing apps' direct OS clock reads do not become virtual automatically.


## Simulator device controls

`kobo drive` accepts `device battery 18 charging`, `device battery 72 unplugged`, `device frontlight 39`, `device cover closed|open` and `device orientation portrait|landscape`. Percentages outside 0–100 are refused. Assert the effective observations using `/device` in `expect-state`; captures include them under `simulation.hardware`. The low-battery scenario overlays 5% without destroying the configured battery value. App service requests continue through declared-capability/backend/power policy. Cover events are sent on actual edges to the foreground app when a cover backend is modeled.

Browser inspector controls use the same endpoints. Frontlight values do not model LCD illumination. Display orientation changes composition and hit testing of the current screen; use the app's own rotation control when verifying its measured reflow. Device controls do not add a physical backend or establish measured calibration.


## Panel failure tests

Use `panel hold` before an app transition or clock/device observation that repaints. Assert `/panel#/status "busy"`; frame reads retain confirmed output while ideal pixels show requested content. A newer requested screen replaces the one queued behind the pending frame. `panel complete` confirms one pending update and submits the latest queued frame, if any. `panel fail` marks contents uncertain without committing the planner. `panel retry` submits a whole-panel cleaning refresh; complete it before returning to `panel auto`. Invalid commands or commands in the wrong state fail explicitly. The browser exposes the same controls and disables unavailable actions.

`/panel` reports submission/completion/failure counts, a current marker, host timestamps, whether a latest frame is queued and whether current contents are known. Taps return HTTP 409 while busy or uncertain. `wait-idle` concerns application callbacks and tasks; it does not implicitly complete a held panel. Use explicit panel assertions/completion in such tests. Visible failed output is the last confirmed image, not a simulated claim about partially driven physical pixels.


## Raw input and holds

`input touch TYPE CODE VALUE; ...` submits at most 64 evdev events / 4096 text bytes through the HAL. Malformed batches do not change decoder state. Keep separate gestures in separate driver steps so callback settlement can occur between them. For a hold, send contact down/position/report, advance a manual clock by 500 ms, then send release/report. Movement and cancel events use the same hold policy as runtime. `input gpio 1 194 1` injects a page-key press; value 0 releases and value 2 auto-repeat is ignored by the existing HAL. Portrait GPIO observations update key direction; this does not assert that the selected physical model has those buttons.

After `input touch 0 3 0;0 0 0` (lost input and report boundary), assert `/input#/needsResynchronization true`. `input resync unknown` or `active` keeps input blocked; send another report boundary before the next query outcome. `input resync released` restores quiescence. Metadata records the synthetic source, raw-event count, quiescence, query requirement and page-key mapping. Browser/driver ordinary taps are also decoded via HAL rather than directly invoking the hit-test result. Direct physical I/O ownership is reserved for the combined Clara BW run.

## Inline feedback, samples and construction checks

Keep `ScreenBuilder::inline_status(feedback::Status)` in the same place across data-loading and recovery transitions. Its short, shared labels preserve the next content/control position at all supported profile sizes, text sizes and orientations. Keep the current items visible; put a specific failure explanation and recovery action alongside the affected operation. `Status::from(draft.status())` follows real save acknowledgements. The status line does not perform a save or invent a sync result.

`ProviderSetup::with_samples(samples::Collection::Notes)` attaches original offline content to the existing sample choice; `Reading` and `Cards` are also available. On `Event::Sample`, open a separate sample session using `setup.samples()`. Sample exports carry an explicit schema, `sample: true` and stable `sample.*` identities. Never merge them into an account's data or use the sample event as a connection check. App adoption is part of the catalog PR.

Before publishing a screen, use `build_checked_with(&logical_metrics, &chrome)` to catch text overflow, hidden content and unreachable controls in addition to builder truncation. Use the same logical landscape metrics and chrome as rendering. The existing `build_checked()` keeps its collection-only behavior for compatibility. Rendering, metrics and diagnostics remain in `kobo-ui`; `kobo-sdk/src/builder.rs` owns only construction of that shared tree.


## Board edits and undo

Use `board::Board` for bounded mark edits, with immutable puzzle givens in `Field::given` and editable squares in `Field::editable`. Games own their rules and meaning of values/candidate bits. A run entry, candidate removal or reset is one `apply` transaction and one Undo. Duplicate/out-of-range cells, changed givens and invalid marks refuse the entire move. Unchanged or rejected moves preserve Redo; a new accepted move after Undo abandons that future.

Keep `board_history_controls(&board)` in the same layout slot. Its Undo, Redo and Clear buttons retain their positions and use semantic disabled state. Dispatch the `board::UNDO`, `REDO` and `CLEAR` names to the model; call `viewport.reveal` for Undo/Redo's selected square. Ask before resetting a played puzzle, then keep that reset undoable.

Save changed marks through acknowledged durable state. `restore` rejects changed givens and never invents history; the app must first validate puzzle identity, version and game-specific mark meanings. The model is in memory and does not claim to save anything. It retains at most 64 moves and 8,192 cell changes across Undo and Redo for a board up to 64 × 64. Oldest moves are evicted when either bound is reached; app copy must not promise unlimited history. Catalog adoption remains in PR 2.


## Task failure ordering

Network scenario failures enter `TaskRunner::submit_with_fault` through normal admission. Missing credentials hide the secret store for that request; they never bypass destination authorization. Transport faults occur after header/credential checks, so an invalid request still reports its real earlier refusal. Stream close performs local cleanup while offline. Network faults leave local reads and timers usable.

Both runtime hosts and the simulator use the same admission outcome: a full task budget returns the existing `Denied` wire result; reusing an active task ID produces no extra completion for it. Apps should use the SDK task allocator and keep at most four tasks in flight. Queued refusals hold their ID until drained. The simulator counts duplicate attempts without replacing active-task metadata or pretending the connection ended.


## Board surface and viewport

Use `BoardViewport::new` with the board, complete `BoardClues`, logical display metrics and the area reserved after chrome, status and controls. It fits whole squares at a comfortable physical size, grows them for two lines of readable text when needed, and refuses an area too small for one square. It never silently changes board coordinates. A viewport sends at most 81 cells, with at most 12 rows/columns, from a board up to 64 × 64. Gutter clues contain at most 32 positive values per line. Invalid shapes, repeated/reserved actions and invalid marks refuse on the wire.

`board_viewport` registers `board.cell.INDEX`, `board.row.INDEX` and `board.column.INDEX` with absolute zero-based indices. `cell_action` resolves visible cells only. `inspect_clue` returns the complete title/text for a visible gutter action; show it with ordinary Back/Close. A short column clue displays up to three values, a longer clue ends visibly in an ellipsis. Row clues use a compact prefix with an ellipsis when shortened. Do not treat the visible prefix as the complete puzzle.

Selection is an outline independent of filled, crossed, dotted, numeric and candidate marks. Givens have an additional underline. Candidate sets show their count; use the selected cell's complete `Mark::description()` and the game's candidate editor for inspection/editing. Very long numeric values abbreviate visibly rather than painting outside a square. The game still validates mark meaning and prevents changes to givens.

`board_viewport_controls` keeps Left/Up/Right and Smaller/Down/Larger in fixed slots. Route these through `navigate`; unavailable directions and sizes are disabled. Panning overlaps one row/column where possible. `reflow` atomically refits on size/orientation changes and reveals the selected square; refusal leaves the prior view intact. Undo/Redo should likewise reveal their selected square. Neither operation edits or saves the board. Use a distinct screen identity for clue details and major viewport geometry changes so the renderer treats them as screen transitions.

The new node requires protocol 14. Runtime decoders retain versions 11, 12 and 13, and version-13 grid payloads remain byte-compatible. The updated runtime must precede apps built with this SDK; app installation/version gating is part of the remaining runtime work. `crates/kobo-sim/examples/board.rs` is an original SDK fixture, not an added store app. `scripts/quality/check-board-sim.py` exercises it through actual IPC and the ordinary driver.

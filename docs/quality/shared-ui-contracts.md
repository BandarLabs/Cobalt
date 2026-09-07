# Shared app contracts

Use these contracts for the catalog work in PR 2. App-specific content, routes and provider behavior stay in the app. Layout measurements, reading, page navigation and result-state conventions come from shared components.

## Reading and collections

- Use `kobo_bookview::BookView` for text, Markdown, HTML and EPUB documents. Its `open_bytes` delegates format handling to `kobo-doc`; `open_html` also coordinates remote figures. Do not put an unbounded article into a column of text nodes.
- Persist `BookView::memory()` after reading actions that return `Outcome::Save`. Restore that memory when reopening. `BookView::reflow` remeasures the open document for changed panel/orientation metrics while preserving the block anchor, book text size and annotations.
- Use `Context::paginate_rows` or its trailing-control variant with the actual row titles/subtitles. Feed its index pages and stable provider/local record IDs into `collections::PagedCollection::reflow`. Render only `visible()` indexes. Select the stable record ID before opening details; retain `position()` so returning and refreshing keep the owner's item in view.
- Empty collections have zero pages. Show a useful empty state and its next action; never show “Page 1 of 0” or an error for a library that has never been populated.
- A removed record clears selection and clamps the remaining page. A new sort order or larger text size follows the record anchor. Reject duplicate provider IDs and incomplete pagination instead of quietly reassigning actions to another row.
- Comic archive handling lives in `kobo-comic`; Panels remains the comic app. CBZ uses the dependency and bounds described in the [comic decision](comic-reader-decision.md). Comic viewing controls and import adoption are still open work.

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

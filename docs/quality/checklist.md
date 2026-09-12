# Beta quality implementation checklist

Source: the 8 September 2026 app/SDK/simulator/companion review. The ten new app ideas and their product pilots are excluded. CBZ/CBR requirements are added from the implementation request. Repeated recommendations are consolidated into their owning task; app adoption remains explicit.

## Delivery plan

Four PRs rooted in `beta`. The initial base was `7f1a543`; PR 1 is merged as `c22c946` and PR 2 targets beta directly. PR 2 grew large enough that its remaining app groups are better validated on hardware than stacked on top of it, so it ships the twenty app groups it has finished and the rest move to PR 3; the companion work it would have shared a branch with moves to PR 4:

1. **Foundation:** simulator/runtime parity, shared SDK/document/board contracts, platform test interfaces and comic decoding/reading. Includes catalog integration fixes exposed by the stricter shared layout checks. Branch `beta-quality-foundation`, base `beta`.
2. **Catalog:** complete and polish the 43 existing app journeys using those contracts. Branch `beta-quality-apps`, initially based on PR 1.
3. **Remaining catalog:** the app groups PR 2 did not reach, reading applications first, each pulling real data wherever the source permits it. Branch `beta-quality-apps-2`, base `beta`.
4. **Companion:** owner onboarding, imports, credentials, recovery and CLI consistency, plus integrated acceptance scripts. Branch `beta-quality-companion`, base `beta`.

After predecessors merge, later PRs can target beta without duplicating earlier diffs. No merge or deployment is implied by creating the PRs.

A checked task requires implementation plus recorded validation. Physical measurements and user studies remain open until performed; the owner asked for Clara BW hardware validation once PR 2 is on a reader rather than after every PR is written. Scripts/protocols can be completed independently of their physical execution. Existing documented stock-menu dependency consumption is allowed; copying its or other local reference source is not.

## Status

The PR 1 implementation checklist is complete; CBR is deferred and COMIC-18 belongs to catalog integration in PR 2. This does not certify hardware accuracy: native sleep hands ownership back to the stock reader, and no generic kernel suspend, RTC wake or automatic cover-sleep backend is enabled. Physical acceptance moves ahead of PR 3: twenty app groups are validated as far as a simulator can speak for them, and panel latency, ghosting, touch accuracy and the live services each application talks to are not among those things.

496 tracked tasks: 217 completed, 1 deferred by the owner, 278 open. Implementation in progress. Evidence is recorded per task in `tasks.json` and in [the validation log](validation.md).

Design direction: the bar is a well-made iPad application, built for a panel that cannot animate. Familiar visual conventions per app, restrained controls, plain copy, full repaints and page turns rather than scrolling, and controls that do not move under a finger. Crossword follows printed crossword typography and grids. PR 3 takes the reading applications first, in the owner's order: Gutenbird, arXiv, Verses, Read Later, Frame, Sync, Lichess, Music Stand. Each pulls real data wherever the source permits it, and Lichess is the application the catalog is shown with, so it has to work against the live service rather than a fixture. Paperterm defaults to portrait with a physically scaled, denser terminal font; the measured grid takes precedence over forcing 80 columns.

## PR 1 · Simulator fidelity (SIM-1–10)

- [x] **SIM-01** Apply interface and reading scales consistently to rendering.
- [x] **SIM-02** Apply the same scale context to measurement, diagnostics and hit testing.
- [x] **SIM-03** Restore ambient scales after rendering to prevent cross-request leakage.
- [x] **SIM-04** Share runtime Back/chrome composition with browser simulation.
- [x] **SIM-05** Route shell Back and app-owned Back consistently.
- [x] **SIM-06** Use declared app capabilities instead of granting every capability.
- [x] **SIM-07** Respect device-supported service backends in simulation.
- [x] **SIM-08** Inject missing credentials for all credentialed network tasks.
- [x] **SIM-09** Return production-equivalent failure reasons.
- [x] **SIM-10** Cover streaming requests with offline and timeout injection.
- [x] **SIM-11** Cover shelf/chunked writes with storage-full injection.
- [x] **SIM-12** Commit panel transitions when screens arrive rather than when screenshots are requested.
- [x] **SIM-13** Make screenshot endpoints read-only.
- [x] **SIM-14** Record bounded screen, input and refresh event histories.
- [x] **SIM-15** Generate simulator Store fixtures from the catalog registry.
- [x] **SIM-16** Exercise actual launcher/app transitions in full-runtime mode.
- [x] **SIM-17** Exercise signed install, update and remove against local fixture packages.
- [x] **SIM-18** Add deterministic virtual clock controls.
- [x] **SIM-19** Replay raw touch down, move, up, hold and page-button events through the HAL.
- [x] **SIM-20** Model panel submission, busy state, completion and failure.
- [x] **SIM-21** Expose battery, charging, frontlight, cover and orientation controls.
- [x] **SIM-22** Inject app kill, lifecycle change and transfer interruption.
- [x] **SIM-23** Keep ideal and approximate panel output distinct.
- [x] **SIM-24** Add color output and label uncalibrated appearance clearly.
- [x] **SIM-25** Reject invalid profile and scale arguments.
- [x] **SIM-26** Stamp captures with source, app, mode, profile, pose, fonts, scales, fixture and seed.
- [x] **SIM-27** Assert serious diagnostics at every drive transition.
- [x] **SIM-28** Add semantic state/network-effect assertions to drive.
- [x] **SIM-29** Use stable control IDs and bounded wait-for-idle in routes.
- [x] **SIM-30** Keep refresh results invariant under screenshot sampling cadence.

## PR 1 · Shared SDK and design contracts

- [x] **SDK-01** Provide measured pagination for plain text, Markdown and HTML.
- [x] **SDK-02** Provide bounded paged collections with stable selection and return position.
- [x] **SDK-03** Preserve reading anchors across scale and orientation changes.
- [x] **SDK-04** Compose provider setup from existing credential and URL controls.
- [x] **SDK-05** Provide reusable connection test and production error mapping.
- [x] **SDK-06** Provide durable content records and migrations.
- [x] **SDK-07** Provide bounded article/asset cache and pruning.
- [x] **SDK-08** Provide persistent mutation outbox with deduplication and acknowledgement.
- [x] **SDK-09** Expose retry and conflict states without false success.
- [x] **SDK-10** Provide save acknowledgement and retry/export for unsaved edits.
- [x] **SDK-11** Provide import receipts, progress and verified availability.
- [x] **SDK-12** Provide expected-empty, corrupt and unsupported library states.
- [x] **SDK-13** Add original sample collections to reusable setup flows.
- [x] **SDK-14** Extend shared board surface with clue gutters, selection and distinct marks.
- [x] **SDK-15** Support larger boards with accessible paging/pan/zoom.
- [x] **SDK-16** Provide board undo/control conventions.
- [x] **SDK-17** Provide injectable calendar date, monotonic time and test entropy.
- [x] **SDK-18** Provide stable inline loading, offline, stale and result surfaces.
- [x] **SDK-19** Provide owner-initiated text/image export and paired handoff.
- [x] **SDK-20** Add metric-aware validation beyond builder collection limits.
- [x] **SDK-21** Keep physical hit targets, margins and semantic type tokens authoritative.
- [x] **SDK-22** Document primary/secondary button hierarchy and stable action placement.
- [x] **SDK-23** Keep copy concrete and remove unexplained implementation language from primary flows.
- [x] **SDK-24** Separate UI/SDK responsibilities into maintainable modules without duplicating contracts.

## PR 1 · Platform requirements from read-only hardware research

- [x] **HW-01** Export versioned observations from Cobalt device probes.
- [x] **HW-02** Record measured, inferred and unverified profile fields separately.
- [x] **HW-03** Load observation fixtures into the simulator without importing third-party device tables.
- [x] **HW-04** Keep digitizer mapping separate from display pose.
- [x] **HW-05** Handle lost input and resynchronization explicitly.
- [x] **HW-06** Preserve backend-specific refresh intents and waveform capability checks.
- [x] **HW-07** Record real refresh submission/completion markers and timing.
- [x] **HW-08** Preserve conservative color/inversion behavior until calibrated.
- [x] **HW-09** Implement platform power state machine with explicit ownership.
- [x] **HW-10** Handle power button, cover, charging and wake reasons through shared state.
- [x] **HW-11** Flush durable work and pause/cancel tasks before suspend.
- [x] **HW-12** Restore frontlight and owner settings after exit or crash.
- [x] **HW-13** Test wake-during-suspend, cover bounce and repeated wake.
- [x] **HW-14** Test scheduled wake, USB attach/detach and reconnect.
- [x] **HW-15** Avoid duplicate task resumption or indefinite unintended wake.
- [x] **HW-16** Keep watchdog and reader handback guarantees intact.
- [x] **HW-17** Create firmware/profile compatibility and recovery evidence format.
- [x] **HW-18** Prepare Clara BW automation for physical corner/touch/refresh checks.
- [x] **HW-19** Prepare Clara BW sleep/wake, forced-exit and setting-restoration checks.
- [x] **HW-20** Prepare latency, ghosting, resume and power calibration protocol.

## PR 1 · CBZ/CBR architecture and shared reader

- [x] **COMIC-01** Document shared reader versus separate-app decision.
- [x] **COMIC-02** Evaluate Rust ZIP/RAR libraries, MSRV, maintenance, resource bounds and full licenses.
- [x] **COMIC-03** Use dependencies without copying third-party source into Cobalt.
- [x] **COMIC-04** Move comic archive metadata and natural page ordering into shared code.
- [x] **COMIC-05** Support CBZ using bounded archive reads and validated images.
- [ ] **COMIC-06 — deferred by owner** CBR support is out of scope for now; no RAR decoder or restricted UnRAR dependency will be added.
- [x] **COMIC-07** Identify format by content and report extension mismatch helpfully.
- [x] **COMIC-08** Reject corrupt, encrypted or unsupported archives with a specific recovery action.
- [x] **COMIC-09** Bound entries, archive bytes, expanded bytes, image pixels and decode resources.
- [x] **COMIC-10** Reject traversal, duplicate names, symlinks and unsafe extraction targets.
- [x] **COMIC-11** Handle numeric filenames, nested folders and non-page metadata.
- [x] **COMIC-12** Preserve title, page order, reading direction and cover metadata.
- [x] **COMIC-13** Provide shared comic page fit, width, zoom and pan controls.
- [x] **COMIC-14** Provide page jump, thumbnails and stable Back/return position.
- [x] **COMIC-15** Persist page, direction and viewport per book.
- [x] **COMIC-16** Support RTL and two-page spreads with a reachable single-page fallback.
- [x] **COMIC-17** Keep page decode lazy and prefetch/cache bounded.
- [x] **COMIC-18** Route local comics through the same import preview and receipt as other documents. (Delivered with PR 2.)
- [x] **COMIC-19** Test original CBZ fixtures and hostile/corrupt archives; test clear refusal of CBR.
- [x] **COMIC-20** Verify Clara-sized comic reading, scaling, navigation and reopen in simulator.

## PR 3 · arXiv

- [ ] **ARXIV-01** Separate title, authors and metadata visually.
- [ ] **ARXIV-02** Add saved searches and followed subjects.
- [ ] **ARXIV-03** Show downloading, available offline and reading progress.
- [ ] **ARXIV-04** Validate long HTML, formulas, figures and tables.
- [ ] **ARXIV-05** Retain saved reading through reopen and failed figure fetch.

## PR 3 · Audiobook Studio

- [ ] **AUDIO-01** Preflight required provider configuration.
- [ ] **AUDIO-02** Show truthful generation stages and cancellation.
- [ ] **AUDIO-03** Checkpoint partial generation and support explicit retry.
- [ ] **AUDIO-04** Provide a free original playable sample.
- [ ] **AUDIO-05** Unify library and now-playing controls.
- [ ] **AUDIO-06** Map missing-secret and partial failures to useful recovery.

## PR 2 · Backgammon

- [x] **BACK-01** Clarify active player and legal source/destination on board.
- [x] **BACK-02** Make dice and doubling cube legible.
- [x] **BACK-03** Separate match setup from moves.
- [x] **BACK-04** Show concise turn history.
- [x] **BACK-05** Use real entropy with explicit deterministic fixture seeds.
- [x] **BACK-06** Resolve opening layout diagnostic.

## PR 2 · Daily Brief

- [x] **BRIEF-01** Show fetched time and source.
- [x] **BRIEF-02** Keep cached headlines with offline/retry state.
- [x] **BRIEF-03** Allow source selection.
- [x] **BRIEF-04** Open and save stories through the shared reader.

## PR 2 · calibre-web (APP-1)

- [x] **CALIBRE-01** Replace ignored response/static categories with real OPDS navigation.
- [x] **CALIBRE-02** Configure and test provider endpoint/authentication.
- [x] **CALIBRE-03** Handle books, authors and shelves through live parsed catalog links.
- [x] **CALIBRE-04** Download and read a fixture book through BookView.
- [x] **CALIBRE-05** Retain downloaded book and progress offline.
- [x] **CALIBRE-06** Distinguish authentication, transport, HTTP and parsing failures.

## PR 3 · AI Command Center

- [ ] **CHAT-01** Guide provider setup and explain provider/model choice.
- [ ] **CHAT-02** Persist and manage conversations.
- [ ] **CHAT-03** Export conversations.
- [ ] **CHAT-04** Support explicit retry and cancel.
- [ ] **CHAT-05** Paginate long replies and test provider-specific failures with fixtures.

## PR 2 · Crossword

- [x] **CROSS-01** Render numbered cells and active word distinctly.
- [x] **CROSS-02** Keep clue and entry together.
- [x] **CROSS-03** Add puzzle corpus/import and difficulty.
- [x] **CROSS-04** Add check/reveal choices.
- [x] **CROSS-05** Persist completion and statistics.

## PR 2 · Deck app

- [x] **DECK-01** Reduce visual weight of unused pads.
- [x] **DECK-02** Show layout title and connection state.
- [x] **DECK-03** Show per-action busy state and last result.
- [x] **DECK-04** Provide useful original preset layouts.
- [x] **DECK-05** Preserve explicit confirmation for chosen commands.
- [x] **DECK-06** Unify visible pad count with companion configuration.

## PR 3 · Fanshelf

- [ ] **FANS-01** Show download and reading progress consistently.
- [ ] **FANS-02** Improve fandom/filter organization.
- [ ] **FANS-03** Provide bulk shelf management.
- [ ] **FANS-04** Distinguish WIP, unread updates and manual update state.
- [ ] **FANS-05** Explain locked or unavailable work.
- [ ] **FANS-06** Validate EPUB reading with synthetic owned content.

## PR 3 · Fieldbook (APP-3)

- [ ] **FIELD-01** Label starter data truthfully.
- [ ] **FIELD-02** Import actual field/species packs and search them.
- [ ] **FIELD-03** Record outing, location, date and time with sightings.
- [ ] **FIELD-04** Edit, delete and undo sightings.
- [ ] **FIELD-05** Show per-outing totals.
- [ ] **FIELD-06** Produce a real downloadable checklist.
- [ ] **FIELD-07** Report save/sync failure instead of fictitious success.
- [ ] **FIELD-08** Keep local logging independent of service availability.

## PR 3 · Flashcards app

- [ ] **CARDS-01** Replace expected missing collection error with first-use setup.
- [ ] **CARDS-02** Include an original ready-to-review sample.
- [ ] **CARDS-03** Show import status and guided companion handoff.
- [ ] **CARDS-04** Explain local grading and review-log reconciliation accurately.
- [ ] **CARDS-05** Test reveal, grade and restart with imported fixtures.
- [ ] **CARDS-06** Test Japanese, media and long cards.

## PR 3 · Frame app

- [ ] **FRAME-01** Expose slideshow mode, interval and ordering controls.
- [ ] **FRAME-02** Show album, date and count clearly.
- [ ] **FRAME-03** Provide an original multi-photo demo.
- [ ] **FRAME-04** Show verified transfer status.
- [ ] **FRAME-05** Integrate shared fit/crop behavior.
- [ ] **FRAME-06** Prepare sleep ownership and energy validation.

## PR 2 · Components reference

- [x] **GALLERY-01** Document every control and loading/disabled/error variant.
- [x] **GALLERY-02** Exercise supported scales, profiles and long labels.
- [x] **GALLERY-03** Label intentional developer diagnostics.
- [x] **GALLERY-04** Replace version-like section labels with meaningful categories.
- [x] **GALLERY-05** Include complete task-flow examples.

## PR 2 · Grimoire (APP-9)

- [x] **GRIM-01** Page spell results using remaining height after filters.
- [x] **GRIM-02** Replace cycling filters with labeled selection.
- [x] **GRIM-03** Paginate long stat blocks.
- [x] **GRIM-04** Improve combat/initiative controls.
- [x] **GRIM-05** Explain unavailable source categories.
- [x] **GRIM-06** Test realistic six-person party and initiative persistence.

## PR 3 · Gutenbird

- [ ] **GUTEN-01** Remove catalog boilerplate from summaries.
- [ ] **GUTEN-02** Clarify edition and language choices.
- [ ] **GUTEN-03** Improve cover fallbacks.
- [ ] **GUTEN-04** Persist shelf reading progress.
- [ ] **GUTEN-05** Reuse shared provider setup.
- [ ] **GUTEN-06** Verify fixture download, reading and offline reopen.

## PR 3 · Habits

- [ ] **HABIT-01** Make Add habit prominent on empty Today.
- [ ] **HABIT-02** Clarify check and skip hierarchy.
- [ ] **HABIT-03** Provide easy habit editing.
- [ ] **HABIT-04** Explain missed and skipped day rules.
- [ ] **HABIT-05** Add useful weekly summary.
- [ ] **HABIT-06** Provide owner export and backup.

## PR 2 · Hacker News

- [x] **HN-01** Reduce duplicate title and metadata noise.
- [x] **HN-02** Clarify saved and read states.
- [x] **HN-03** Preserve list position after reading.
- [x] **HN-04** Test deep comments and collapse/expand.
- [x] **HN-05** Handle unavailable links and long titles.
- [x] **HN-06** Keep story and discussion navigation distinct.

## PR 3 · Home Panel

- [ ] **HOME-01** Guide server discovery and connection setup.
- [ ] **HOME-02** Show online/stale/last-updated states.
- [ ] **HOME-03** Acknowledge each tile action and failure.
- [ ] **HOME-04** Provide compact tile editing.
- [ ] **HOME-05** Add usable climate controls.
- [ ] **HOME-06** Provide an optional wall-panel mode.
- [ ] **HOME-07** Validate against local service fixtures.

## PR 3 · Inkling

- [ ] **INK-01** Expand audited answer and guess vocabulary.
- [ ] **INK-02** Show uppercase letters with redundant state patterns.
- [ ] **INK-03** Keep keyboard knowledge visible.
- [ ] **INK-04** Use real date instead of technical identifiers.
- [ ] **INK-05** Add result export and distribution statistics.
- [ ] **INK-06** Provide archive play.
- [ ] **INK-07** Verify daily persistence and deterministic fixtures.

## PR 3 · Kitchen Card (APP-2)

- [ ] **KITCHEN-01** Configure Mealie endpoint and credentials.
- [ ] **KITCHEN-02** Parse real recipe list and detail responses.
- [ ] **KITCHEN-03** Preserve ingredient quantities and cooking steps.
- [ ] **KITCHEN-04** Persist imported recipes through restart.
- [ ] **KITCHEN-05** Handle save and connection failures honestly.
- [ ] **KITCHEN-06** Add ingredient check-off.
- [ ] **KITCHEN-07** Add recipe scaling with readable fractions.
- [ ] **KITCHEN-08** Add cooking timers and Finished state.

## PR 3 · Lichess

- [ ] **LICHESS-01** Polish pairing and reconnection guidance.
- [ ] **LICHESS-02** Show active side, clock and connection status clearly.
- [ ] **LICHESS-03** Keep legal moves and selected squares legible.
- [ ] **LICHESS-04** Validate move acknowledgement, stale connection and reconnect.
- [ ] **LICHESS-05** Test complete fixture match and retained session state.
- [ ] **LICHESS-06** Notice on the panel when a seek has already been matched.

## PR 2 · Logic Pack

- [x] **LOGIC-01** Add varied validated puzzles and difficulty.
- [x] **LOGIC-02** Persist progress.
- [x] **LOGIC-03** Provide undo and contextual controls.
- [x] **LOGIC-04** Render appropriate line, bridge and cross-sum boards with attached clues.
- [x] **LOGIC-05** Test completion and reopen for each game.

## PR 2 · Magnet

- [x] **MAGNET-01** Show annotated sensor-location guidance.
- [x] **MAGNET-02** Distinguish unsupported sensor from no magnet.
- [x] **MAGNET-03** Provide resettable observation count.
- [x] **MAGNET-04** Test controlled hall-sensor events.

## PR 3 · Morse

- [ ] **MORSE-01** Expose speed, duration and repeat clearly.
- [ ] **MORSE-02** Keep Stop reachable.
- [ ] **MORSE-03** Explain unsupported characters.
- [ ] **MORSE-04** Restore previous light setting.
- [ ] **MORSE-05** Add visible learning mode and letter reference.
- [ ] **MORSE-06** Prepare hardware timing checks.

## PR 3 · Music Stand (APP-4)

- [ ] **MUSIC-01** Replace text placeholders with actual score pages.
- [ ] **MUSIC-02** Render overlapping half-page crops correctly.
- [ ] **MUSIC-03** Create and edit setlists and rehearsal order.
- [ ] **MUSIC-04** Persist per-score page, crop and marks.
- [ ] **MUSIC-05** Provide usable zoom and full-screen controls.
- [ ] **MUSIC-06** Support reachable physical/page-button turns.
- [ ] **MUSIC-07** Read host-prepared scores and report import failures.

## PR 3 · Needles

- [ ] **NEEDLES-01** Provide projects and named sections.
- [ ] **NEEDLES-02** Emphasize current row count.
- [ ] **NEEDLES-03** Keep Undo adjacent to increment.
- [ ] **NEEDLES-04** Show pattern location and repeat progress.
- [ ] **NEEDLES-05** Read actual imported pattern sections.
- [ ] **NEEDLES-06** Support charts through shared image/document reading.

## PR 2 · Nonograms

- [x] **NONO-01** Attach clues to matching rows and columns.
- [x] **NONO-02** Show selected line and marks distinctly.
- [x] **NONO-03** Provide undo.
- [x] **NONO-04** Make supported larger grids navigable.
- [x] **NONO-05** Distinguish unsupported sizes before play.
- [x] **NONO-06** Validate imported puzzle solvability and difficulty.
- [x] **NONO-07** Replace repetitive bundled stroke patterns with varied original picture puzzles while preserving existing saved games. (Added during implementation review.)

## PR 2 · Panels

- [x] **PANELS-01** Use shared comic archive/reader components.
- [x] **PANELS-02** Guide server or local-comic import.
- [x] **PANELS-03** Provide original sample comic pages.
- [x] **PANELS-04** Show thumbnails and reading progress.
- [x] **PANELS-05** Expose page zoom, fit and spread controls.
- [x] **PANELS-06** Verify RTL reading.
- [x] **PANELS-07** Verify interrupted download and offline resume.

## PR 2 · Paperterm

- [x] **PAPER-01** Guide host setup and pairing.
- [x] **PAPER-02** Provide sample/read-only preview.
- [x] **PAPER-03** Keep session mode and connection visible.
- [x] **PAPER-04** Explain reconnect and retain a usable keyboard toggle.
- [x] **PAPER-05** Validate wide rows, cursor and keyboard input.
- [x] **PAPER-06** Validate orientation on supported profiles.

## PR 2 · Parlor

- [x] **PARLOR-01** Distinguish boards and pieces.
- [x] **PARLOR-02** Show current player and last move.
- [x] **PARLOR-03** Improve legal-target contrast.
- [x] **PARLOR-04** Complete game/resume checks for all four rulesets.
- [x] **PARLOR-05** Keep unavailable board sizes honest until navigable.

## PR 3 · Parser

- [ ] **PARSER-01** Include an original tutorial story.
- [ ] **PARSER-02** Offer useful command suggestions.
- [ ] **PARSER-03** Clarify import and save slots.
- [ ] **PARSER-04** Complete interpreter conformance fixtures.
- [ ] **PARSER-05** Run representative story fixtures.
- [ ] **PARSER-06** Paginate long transcript and restore saved play.

## PR 3 · Post

- [ ] **POST-01** Paginate inbox and letters.
- [ ] **POST-02** Persist interrupted drafts.
- [ ] **POST-03** Distinguish queued, sending, delivered and rejected.
- [ ] **POST-04** Explain gateway/companion setup.
- [ ] **POST-05** Retry with duplicate protection.
- [ ] **POST-06** Verify delivery/retry against a local mock.

## PR 3 · Pub Quiz

- [ ] **QUIZ-01** Allow player names and count.
- [ ] **QUIZ-02** Provide categories and difficulty.
- [ ] **QUIZ-03** Show pack source and freshness.
- [ ] **QUIZ-04** Clarify pass-device and answer-reveal screens.
- [ ] **QUIZ-05** Export a scorecard.
- [ ] **QUIZ-06** Test full rounds, repeats and offline refreshed packs.

## PR 3 · Read Later (APP-5/6)

- [ ] **LATER-01** Persist fetched full article bodies.
- [ ] **LATER-02** Implement acknowledged durable archive/star/read outbox.
- [ ] **LATER-03** Wire all visible tabs and action controls.
- [ ] **LATER-04** Paginate and retain reading position.
- [ ] **LATER-05** Show pending/retry/offline state.
- [ ] **LATER-06** Verify reconnect, server effects and process restart.

## PR 2 · Feeds

- [x] **FEEDS-01** Distinguish failed discovery from no feed found.
- [x] **FEEDS-02** Accept direct feed URLs.
- [x] **FEEDS-03** Support OPML import.
- [x] **FEEDS-04** Provide working original or public starter feeds.
- [x] **FEEDS-05** Show last refresh, unread counts and per-feed failure.
- [x] **FEEDS-06** Test RSS, Atom, full-content and summary entries with inline images, captions/alt text, e-ink scaling, offline image restoration and missing-image recovery.

## PR 2 · Miniflux (APP-5)

- [x] **MINI-01** Persist full article bodies offline.
- [x] **MINI-02** Flush acknowledged read/star/archive mutations correctly.
- [x] **MINI-03** Parse HTML into the shared document reader with inline images, captions/alt text, e-ink scaling, offline image restoration and missing-image recovery.
- [x] **MINI-04** Wire tabs, star, full-text and suggested-feed controls.
- [x] **MINI-05** Keep pending/retry state visible.
- [x] **MINI-06** Test fetch/read/mutate/reconnect/restart against a fixture server.

## PR 2 · Sidekick app

- [x] **SIDE-01** Guide desktop connection setup.
- [x] **SIDE-02** Show paired host and session identity.
- [x] **SIDE-03** Handle reconnect and stale requests.
- [x] **SIDE-04** Test simultaneous fixture prompts.
- [x] **SIDE-05** Require response acknowledgement before completion.

## PR 2 · Sudoku (APP-7)

- [x] **SUDOKU-01** Persist exact game state after each move.
- [x] **SUDOKU-02** Provide varied valid puzzles with difficulty.
- [x] **SUDOKU-03** Add pencil marks.
- [x] **SUDOKU-04** Add undo.
- [x] **SUDOKU-05** Render stronger 3x3 boundaries and row/column selection.
- [x] **SUDOKU-06** Make assisted checking optional.
- [x] **SUDOKU-07** Show a clear completion state.

## PR 3 · Sync app

- [ ] **SYNCAPP-01** Guide folder choice through pairing and first verified sync.
- [ ] **SYNCAPP-02** Show last success, bytes remaining and peer availability.
- [ ] **SYNCAPP-03** Show next scheduled window.
- [ ] **SYNCAPP-04** Distinguish raw transfer from app ingestion.
- [ ] **SYNCAPP-05** Integrate Vault and Frame import/index publication.
- [ ] **SYNCAPP-06** Provide pause/resume and conflict/retry states.

## PR 2 · Tic-tac-toe

- [x] **TIC-01** Add session score and rematch.
- [x] **TIC-02** Clarify turn and win state.
- [x] **TIC-03** Provide optional small solo mode.
- [x] **TIC-04** Keep it as a simple regression fixture.

## PR 2 · Todo

- [x] **TODO-01** Make Add task prominent in empty state.
- [x] **TODO-02** Edit and reorder tasks.
- [x] **TODO-03** Undo completion.
- [x] **TODO-04** Provide optional dated tasks.
- [x] **TODO-05** Export tasks.
- [x] **TODO-06** Test long text, tags and measured capacity.

## PR 3 · Vault (APP-6)

- [ ] **VAULT-01** Page library rows without clipped actions.
- [ ] **VAULT-02** Paginate long notes through final sentence.
- [ ] **VAULT-03** Wire tag filtering and deduplicate tags.
- [ ] **VAULT-04** Build navigable folder hierarchy.
- [ ] **VAULT-05** Replace single packed-index bottleneck with scalable indexed shelf.
- [ ] **VAULT-06** Integrate Sync ingestion.
- [ ] **VAULT-07** Preserve Back, list and reading positions.

## PR 3 · Verses (APP-8)

- [ ] **VERSE-01** Use real injectable calendar date and advance across midnight.
- [ ] **VERSE-02** Provide complete permitted poems or label excerpts explicitly.
- [ ] **VERSE-03** Paginate long poems.
- [ ] **VERSE-04** Show author and source context.
- [ ] **VERSE-05** Make favorites usable.
- [ ] **VERSE-06** Export attributed quote cards.

## PR 3 · Catalog-wide release gates

- [ ] **APPQA-01** Use one dominant task action with stable secondary controls.
- [ ] **APPQA-02** Differentiate loading, empty, offline, expired credentials and malformed content.
- [ ] **APPQA-03** Keep cached content usable with honest freshness.
- [ ] **APPQA-04** Show unsaved state before success.
- [ ] **APPQA-05** Audit every visible action for a handler.
- [ ] **APPQA-06** Supply a committed meaningful route for each of the 43 apps.
- [ ] **APPQA-07** Exercise install/sample/own-data/task/offline-reopen for each app.
- [ ] **APPQA-08** Test 0, 1, 20 and 100 items with long Unicode titles.
- [ ] **APPQA-09** Test long bodies, supported fonts and CJK where applicable.
- [ ] **APPQA-10** Test delayed/failed downloads and storage interruption.
- [ ] **APPQA-11** Check every intermediate state under supported profile/scale/orientation.
- [ ] **APPQA-12** Regenerate screenshots only from the shipped build.
- [ ] **APPQA-13** Keep Zotero Reader outside this catalog review.

## PR 4 · Main CLI and companion operation engine

- [ ] **CLI-01** Provide guided interactive entry point for bare kobo.
- [ ] **CLI-02** Keep plain compact help for noninteractive use.
- [ ] **CLI-03** Separate owner tasks from developer/release commands.
- [ ] **CLI-04** Provide a desktop/local companion surface using the same operations.
- [ ] **CLI-05** Name readers by stable identity and owner nickname.
- [ ] **CLI-06** Select among multiple USB/Wi-Fi readers without first-device fallback.
- [ ] **CLI-07** Remember pairing and reconnect after address changes.
- [ ] **CLI-08** Preserve verified USB setup, eject, reboot and reconnect steps.
- [ ] **CLI-09** Send and open an original sample for first success.
- [ ] **CLI-10** Describe installation changes before approval.
- [ ] **CLI-11** Provide one app setup card generated from capabilities.
- [ ] **CLI-12** Persist completed setup steps.
- [ ] **CLI-13** Choose files/folders through picker or explicit CLI path.
- [ ] **CLI-14** Detect suitable target apps and resolve ambiguity.
- [ ] **CLI-15** Preview content for the selected reader before sending.
- [ ] **CLI-16** Report real preparing/sending/checking/ready stages.
- [ ] **CLI-17** Persist receipts and resume approved interrupted transfers.
- [ ] **CLI-18** Publish atomically and retain prior valid content.
- [ ] **CLI-19** Retain selection and preparation on retry.
- [ ] **CLI-20** Report useful owner errors with optional technical details.
- [ ] **CLI-21** Export diagnostic reports without secrets or content by default.
- [ ] **CLI-22** Provide understandable background status, pause and quit.
- [ ] **CLI-23** Notify only useful completion or required action.
- [ ] **CLI-24** Explain wake/network effects of continuous sync.
- [ ] **CLI-25** Use consistent verbs and reader/simulator/output target resolution.
- [ ] **CLI-26** Create required storage within validated import.
- [ ] **CLI-27** Support offline preparation and bundled local help.
- [ ] **CLI-28** Keep prepared, sent and available-offline states distinct.
- [ ] **CLI-29** Provide keyboard and screen-reader-friendly forms.
- [ ] **CLI-30** Provide numbered plain terminal alternative.
- [ ] **CLI-31** Respect narrow widths, resize, NO_COLOR, non-TTY and reduced motion.
- [ ] **CLI-32** Keep progress on stderr with versioned JSON on stdout.
- [ ] **CLI-33** Use stable exit categories and bounded noninteractive behavior.
- [ ] **CLI-34** Keep typed operations shared across UI, CLI and agent callers.
- [ ] **CLI-35** Show host/reader/helper compatibility and signed update status.
- [ ] **CLI-36** Preserve content, preferences and pairing through updates.
- [ ] **CLI-37** Keep arbitrary commands in explicit advanced controls.
- [ ] **CLI-38** Avoid required AI/chat, vague slogans and decorative dashboard clutter.

## PR 4 · Deck companion

- [ ] **DECKCLI-01** Preserve real pairing when pushing layouts.
- [ ] **DECKCLI-02** Separate static simulator preview from executable pairing.
- [ ] **DECKCLI-03** Unify 15-pad configuration and UI limits.
- [ ] **DECKCLI-04** Provide app/URL/action picker and grid preview.
- [ ] **DECKCLI-05** Offer practical original presets.
- [ ] **DECKCLI-06** Test a harmless action and show its acknowledgement.
- [ ] **DECKCLI-07** Keep per-action confirmation configurable.

## PR 4 · Flashcards companion

- [x] **FLASHCLI-01** Replace main CLI refusal stub with the supported helper entry point.
- [x] **FLASHCLI-02** Make helper install/version status discoverable.
- [ ] **FLASHCLI-03** Distribute verified host helper without requiring a toolchain.
- [ ] **FLASHCLI-04** Keep existing helper license/distribution boundary intact.
- [x] **FLASHCLI-05** Explain actual supported formats and unsupported features.
- [x] **FLASHCLI-06** Preview front/back, media and card counts.
- [x] **FLASHCLI-07** Explain and implement add/merge versus replace.
- [x] **FLASHCLI-08** Expose verify, stage and review-log export consistently.
- [x] **FLASHCLI-09** Validate imported content and preserve previous collection on failure.

## PR 4 · Frame companion

- [x] **FRAMECLI-01** Preview multiple photos and crop/pad choices.
- [x] **FRAMECLI-02** Perform bounded downsize with visual quality preview.
- [x] **FRAMECLI-03** Show storage estimate and album naming.
- [x] **FRAMECLI-04** Deduplicate repeated import clearly.
- [x] **FRAMECLI-05** Show concrete deletion/replacement list.
- [ ] **FRAMECLI-06** Retain recoverable previous album.
- [ ] **FRAMECLI-07** Verify reader availability before saying photos are ready.

## PR 4 · Vault companion

- [ ] **VAULTCLI-01** Pick folder and preview included/excluded notes.
- [ ] **VAULTCLI-02** Preview a long note at reader dimensions.
- [ ] **VAULTCLI-03** Package incrementally with scalable index.
- [ ] **VAULTCLI-04** Separate transfer acknowledgement from indexing completion.
- [ ] **VAULTCLI-05** Handle rename without duplicating the whole library.
- [ ] **VAULTCLI-06** Explain direction and supported reader-edit export honestly.
- [ ] **VAULTCLI-07** Enable optional ongoing sync after a successful import.

## PR 4 · Sync companion

- [ ] **SYNCCLI-01** Preserve isolated private daemon configuration.
- [ ] **SYNCCLI-02** Provide explicit test/config root.
- [ ] **SYNCCLI-03** Provide structured plan, status and diagnostics.
- [ ] **SYNCCLI-04** Unify raw synced files with actual app ingestion.
- [ ] **SYNCCLI-05** Expose last successful sync, pause/resume and conflicts.
- [ ] **SYNCCLI-06** Keep owner originals protected and directions explicit.

## PR 4 · Needles companion

- [ ] **NEEDLECLI-01** Manage required converter instead of demanding manual toolchain setup.
- [ ] **NEEDLECLI-02** Preview extracted instructions against source.
- [ ] **NEEDLECLI-03** Flag image-only pages and missing charts.
- [ ] **NEEDLECLI-04** Choose title and preview section/row parsing.
- [ ] **NEEDLECLI-05** Use the same prepare/preview/send flow for PDF, Markdown and text.
- [ ] **NEEDLECLI-06** Support simulator and output-only targets.

## PR 4 · Nonograms companion

- [ ] **NONOCLI-01** Preview resulting puzzle at each supported size.
- [ ] **NONOCLI-02** Validate solvability and difficulty before transfer.
- [ ] **NONOCLI-03** Support named imports and multiple puzzles.
- [ ] **NONOCLI-04** Support simulator and reader targets consistently.

## PR 4 · Parser companion

- [ ] **PARSERCLI-01** Share structural validation with interpreter.
- [ ] **PARSERCLI-02** Distinguish recognized format from playable validated story.
- [ ] **PARSERCLI-03** Show title, format and compatibility.
- [ ] **PARSERCLI-04** Support shelf choice, duplicates and simulator transfer.

## PR 4 · Paperterm companion

- [ ] **STREAMCLI-01** Guide pairing through named reader choice.
- [ ] **STREAMCLI-02** Offer known terminal/task presets and connection test.
- [ ] **STREAMCLI-03** Show stopped, waiting, connected and reconnecting states.
- [ ] **STREAMCLI-04** Provide obvious Stop and computer-awake explanation.
- [ ] **STREAMCLI-05** Keep arbitrary terminal command entry advanced.

## PR 4 · Sidekick companion

- [ ] **SIDECLI-01** Unify helper install/start/status/stop in companion.
- [ ] **SIDECLI-02** Show and select agent integrations before configuration.
- [ ] **SIDECLI-03** Preserve dry-run, printed configuration and backups.
- [ ] **SIDECLI-04** Verify a harmless sample event on the reader.
- [ ] **SIDECLI-05** Return success from help.
- [ ] **SIDECLI-06** Provide self-contained sample mode.

## PR 4 · Provider connections

- [ ] **SERVICECLI-01** Fix secret/trust help exit status.
- [ ] **SERVICECLI-02** Provide per-app Connect service form.
- [ ] **SERVICECLI-03** Use discovery and browser authentication where supported.
- [ ] **SERVICECLI-04** Use labeled masked tokens with direct provider guidance otherwise.
- [ ] **SERVICECLI-05** Test endpoint and required capabilities.
- [ ] **SERVICECLI-06** Show account/server identity.
- [ ] **SERVICECLI-07** Explain certificate errors and explicit trust installation.
- [ ] **SERVICECLI-08** Avoid secret input in shell history.

## PR 4 · Missing companion workflows

- [ ] **MISSINGCLI-01** Implement scores import/push with real score conversion.
- [ ] **MISSINGCLI-02** Implement Fieldbook pack import and checklist export.
- [ ] **MISSINGCLI-03** Provide Panels CBZ import with page/cover preview and clear CBR guidance.
- [ ] **MISSINGCLI-04** Generate setup instructions and command references from shared capabilities.
- [ ] **MISSINGCLI-05** Remove obsolete commands and false availability claims.

## PR 4 · Owner experience and final validation

- [ ] **OWNERQA-01** Test photo first-use flow without command typing.
- [ ] **OWNERQA-02** Test card preview/import/review-log flow.
- [ ] **OWNERQA-03** Test notes import and optional sync flow.
- [ ] **OWNERQA-04** Test disconnected reader with retained prepared files.
- [ ] **OWNERQA-05** Test multiple-reader selection and identity revalidation.
- [ ] **OWNERQA-06** Test partial transfer with truthful acknowledged counts.
- [ ] **OWNERQA-07** Test unsupported/oversized/corrupt input recovery.
- [ ] **OWNERQA-08** Test rejected/missing token versus host/certificate errors.
- [ ] **OWNERQA-09** Test expected empty app sample flow.
- [ ] **OWNERQA-10** Test missing/incompatible helper recovery.
- [ ] **OWNERQA-11** Prepare 6–8-person nontechnical usability protocol including assistive technology.
- [ ] **OWNERQA-12** Measure completion, assistance, wrong-target attempts and error comprehension.
- [ ] **OWNERQA-13** Record first-success timing separately from firmware reboot.
- [ ] **OWNERQA-14** Validate recovery without repeated file selection.
- [ ] **OWNERQA-15** Validate understanding of prepared/sent/offline distinctions.
- [ ] **OWNERQA-16** Prepare one-week repeated-use follow-up protocol.
- [ ] **OWNERQA-17** Run all automated simulator and integration gates.
- [ ] **OWNERQA-18** Prepare one combined Clara BW hardware validation script for after all three PRs.
- [ ] **OWNERQA-19** Record unperformed physical/user-study checks honestly.
- [ ] **OWNERQA-20** Verify licenses and absence of copied local-reference source.
- [ ] **OWNERQA-21** Open no more than three logically grouped PRs with tests and remaining validation stated.

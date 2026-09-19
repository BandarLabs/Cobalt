# Proposed upstream PR

## Title

`cli: ship owner-ready companion workflows and real-workload verification`

## Body

### What changed

- adds guided companion commands for photos, notes, feeds, puzzles, stories, comics, scores, field packs, provider connections, sync and reader targeting
- makes transfer state explicit: planned, prepared, sent, acknowledged, offline and recoverable are reported separately
- adds bounded previews, input validation, atomic publication, readback, recovery and secret-file handling
- adds real host-to-simulator journeys using attributable public artifacts, with source hashes and app-side assertions
- packages the Flashcards helper and keeps its license/source boundary visible

### Tests

- real-workload harness: 5/5 flows pass for Vault, Nonograms, Parser, Panels and RSS
- focused Frame, Feeds and Sync recovery acceptances pass without repeated file selection
- 363 of 365 `kobo-cli` tests pass in the validation container; the remaining two packaging tests require the unavailable ARM C cross-compiler
- `cargo fmt --all --check` passes
- strict Clippy reaches an existing `kobo-sim` `format_in_format_args` lint before checking the CLI on Rust 1.98
- locked dependency metadata reports no third-party package without declared license information; changed source contains no developer-local/reference-source markers

### Remaining validation

- no physical reader, Wi-Fi transfer, USB mount/eject, sleep/wake or e-ink panel behavior was certified here
- run the prepared combined Clara BW hardware script before stable promotion
- rerun the two packaging tests with the documented ARM cross-compiler and strict Clippy with the shared simulator lint fixed or the repository's pinned toolchain
- the prepared nontechnical usability and one-week follow-up protocols remain unperformed

This is one companion-CLI PR against `beta`. The app-polish work remains in its separate app PR, so the program stays within the requested two-PR shape.

# PR 168 CI repair — 9 September 2026

The pull-request run 34313647803 and push run 34313643168 failed on the same two checks at a693082. Device compilation succeeded; the device job failed during app version validation.

The crossword legacy-save decoder now destructures exactly three fields instead of indexing the vector in a match expression. It still rejects malformed records and preserves the same migration behavior. This resolves Rust 1.85.1 Clippy’s `match_on_vec_items` diagnostic without suppressing the lint.

The new protocol-14 compatibility record pins the exact published and current Cargo.lock blobs. Its reviewed changes are local versions and dependencies for the seven improved apps, and the runtime policy’s dependency on the existing workspace JSON parser. There are no external dependency changes. The existing release checker continues to enforce individual app changes and versions; published binaries remain compatible with the runtime. The exception stops applying if either lockfile blob changes, so further dependency work requires a new review.

Validation on the isolated PR head plus this repair:

- Rust 1.85.1 formatting check and strict workspace Clippy, all targets and features: passed.
- Crossword tests: 11 passed, including legacy-save migration, corrupt records, failed writes and layout checks.
- App-version checker tests: 54 passed.
- Publication check against the downloaded app-catalog-beta, source c22c9462f6b191999529d683276505079e171cc9: passed.
- Selected packages: Calibre-Web, Crossword, Logic Pack, Nonograms, Panels, Paperterm and Sudoku. Unchanged apps, including Zotero Reader, are not selected.

No visible UI changes were made by this repair. Existing crossword screenshots remain representative. Hosted CI still needs to validate the pushed commit.

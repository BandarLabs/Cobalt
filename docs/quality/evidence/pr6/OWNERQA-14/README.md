# Recovery without repeated file selection

Three real CLI acceptance programs pass against an isolated host/simulator state:

- Frame preserves two bounded recovery slots, restores replacement/removal, refuses incomplete backup state, and leaves the prepared image bytes reusable after failed publication.
- Feeds preserves both the selected OPML bytes and installed list across malformed input, oversized input, occupied staging and repeated publication.
- Sync publishes the already selected notes folder and Frame album idempotently, and pause/resume/run do not ask the owner to select those files again.

These are host and simulator checks, not physical-reader validation. The JSON files are the scripts' direct result records.

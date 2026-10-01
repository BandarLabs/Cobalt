# Audiobook composer accessibility

These are genuine app-builder and SDK-callback captures rendered with installed
Cobalt fonts at Clara BW metrics (1072 × 1448, 300 PPI). They include runtime
header/Back composition and a synthetic 00:00/50% status strip. They are **not live
simulator or physical-device screenshots**: this host denied AF_UNIX socket
creation even for a reviewed elevated launch. Topics and checkpoints are synthetic.

## Matched evidence

Before frames use beta `97153048` production source with test-only capture code.
The normal composer pair uses 140% text: the original long introduction is
suppressed by layout pressure, whereas the shortened heading and supporting copy
remain visible. The resume-plus-validation pair uses 170% text: originally the
long resume button and hint consume the panel, pushing the keyboard and Create
out of view. The revised composer keeps all six languages, topic entry, Create,
and Resume reachable. `after-resume-170.png` additionally shows checkpoint part
progress and a width-clamped title when no validation hint is active.

Resume is a fixed header action. Back now returns to the shelf while preserving
an unsubmitted topic and selected language; changing the topic clears its stale
validation hint. The paid-generation preflight and checkpoint pipeline are not
changed. Existing empty-library content is deliberately left to PR #224.

## Validation

36 app tests pass. Added regression coverage exercises all nine Clara text sizes
(80–170%) with checkpoint and validation states, including simultaneous Shelf and
Resume header actions, actual language target hit-testing, protocol/layout
validation, keyboard submit reachability, Back/Shelf draft preservation, and
short-topic correction without launching generation. Existing preflight,
checkpoint, sample, library and player tests remain passing.

```sh
COBALT_REVIEW_CAPTURE_DIR=/tmp/audiobook-captures \
  python3 examples/audiobook/screenshots/ui-review/capture.py
cargo test -p kobo-audiobook --all-targets --all-features
cargo clippy -p kobo-audiobook --all-targets --all-features -- -D warnings
```

Format and strict clippy pass. The documented contributor dry-run passed static
ARM verification and deterministic fixture-only Beta preview generation. Its
missing public beta-store-smoke seed (`2a` × 32) was supplied locally at mode
0600, not committed. The specialized simulator journey's heading expectations
were updated and Python syntax checked, but it was not run locally.

Version 1.0.16 avoids the 1.0.15 bump already proposed by PR #224. No SDK/runtime
or capability changes are included. Live narration providers, audio output,
Bluetooth, interactive simulator and physical Kobo validation remain open.

## Dependency-isolated revalidation

The final normal app suite passes 34 tests; the opt-in capture suite passes 2. The complete `node --test tools/*.test.mjs` suite passes all 105 tests, and app tests (locked), strict clippy, format and contributor/static-ARM checks pass again. App Cargo.toml and the workspace Cargo.lock are byte-identical to beta: capture-only renderer/font dependencies live only in a temporary package created by `capture.py`. Every committed after PNG is byte-for-byte identical to its original reviewed capture (3 images); 21 of 21 full capture-set PNGs also match. The script copies the current app source/assets and the workspace lock into a temporary mirror, adds only the capture module, and never modifies the release dependency graph.

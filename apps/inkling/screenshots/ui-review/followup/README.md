# Inkling PR259 follow-up evidence

The baseline app is PR head `5037033f45fe6d1f4672bb6878c6a1f44bb5d3dd`.
The final app and receiver are code commit `07a6dc49` (receiver `bbe18c0d`).
The earlier screenshots in the parent directory are preserved.

## Reproduction and change

On Rust 1.85.1, the unchanged head reproduced the reported CI failure exactly:
24 app tests passed and Help failed at 758 × 1024, 212 PPI, Huge (155%) text,
with `TextOverflow` at node 5. See [the original test log](reproduced-before-test.log).
Help now combines its redundant heading and opening rule and uses a concise
shape legend. The same test passes at all text steps, with additional Libra
and Elipsa portrait/landscape geometries.

The overflowing case is the unit test's fallback font measurement. The actual
bundled-font renderer does **not** show that overflow in the baseline PNGs;
these pictures must not be described as a visually clipped baseline. The new
layout has more room with both fallback and installed fonts.

## Full simulator: all nine profiles

These captures run the app in a separate process through real SDK IPC, use the
actual native renderer and simulator shell, and drive actual controls. Each
profile uses 155% text, a pinned synthetic 2026-09-01 puzzle, synthetic clock,
and isolated temporary storage. Each phase has board, Help, typing, typing
Back, Statistics, and acknowledged export PNGs, layout/diagnostic JSON, source
and font sidecars, and a log. No physical Kobo was used.

| Profile | Before Help | After Help |
| --- | --- | --- |
| clara-bw-391 | [Before](before/clara-bw-391/help.png) | [After](after/clara-bw-391/help.png) |
| clara-bw-395 | [Before](before/clara-bw-395/help.png) | [After](after/clara-bw-395/help.png) |
| clara-hd-376 | [Before](before/clara-hd-376/help.png) | [After](after/clara-hd-376/help.png) |
| clara-colour-393 | [Before](before/clara-colour-393/help.png) | [After](after/clara-colour-393/help.png) |
| elipsa-2e-389 | [Before](before/elipsa-2e-389/help.png) | [After](after/elipsa-2e-389/help.png) |
| libra-2-388 | [Before](before/libra-2-388/help.png) | [After](after/libra-2-388/help.png) |
| libra-colour-390 | [Before](before/libra-colour-390/help.png) | [After](after/libra-colour-390/help.png) |
| libra-colour-390-4.46.23836 | [Before](before/libra-colour-390-4.46.23836/help.png) | [After](after/libra-colour-390-4.46.23836/help.png) |
| libra-h2o-384 | [Before](before/libra-h2o-384/help.png) | [After](after/libra-h2o-384/help.png) |

The before/after `provenance.json` records the actual app-source and CLI hashes.
The baseline capture finished after the independent receiver commit, so its
recorded HEAD is `bbe18c0d`; its app SHA-256 matches `5037033f` exactly. After
captures used the final app source byte-for-byte, subsequently committed in
`07a6dc49`. A source dirty flag in the image sidecars reflects uncommitted work
at capture time, not a different final app implementation.

## Supplemental native renderer: regression geometry

[Before at 155%](renderer-before/help-155.png) ·
[After at 155%](renderer-after/help-155.png) ·
[Before at 170%](renderer-before/help-170.png) ·
[After at 170%](renderer-after/help-170.png)

These 758 × 1024, 212 PPI captures use the app builders, SDK callbacks, bundled
fonts and native renderer with synthetic measuring chrome. They are **not**
full simulator or hardware screenshots. Their text sidecars record geometry,
font source, layout, diagnostics and app-source SHA-256.

## Reproduction commands

```sh
cargo +1.85.1 build --locked -p kobo-cli
python3 apps/inkling/screenshots/ui-review/capture-sim.py /tmp/inkling-final
python3 scripts/quality/check-inkling-sim.py --profile clara-bw-391 --scale 155 --output /tmp/inkling-journey
COBALT_REVIEW_SOURCE_REV=5037033f COBALT_REVIEW_METRICS=758,1024,212 \
  COBALT_REVIEW_CAPTURE_DIR=/tmp/inkling-native-before \
  python3 apps/inkling/screenshots/ui-review/capture.py
COBALT_REVIEW_METRICS=758,1024,212 \
  COBALT_REVIEW_CAPTURE_DIR=/tmp/inkling-native-after \
  python3 apps/inkling/screenshots/ui-review/capture.py
```

For baseline full-simulator captures, run the matrix harness against an
isolated checkout of `5037033f` with that checkout's CLI. All fixture data is
synthetic. No real account, paired reader, merge, or deployment was used.

## Validation

- All **nine full simulator journeys passed** at 155% text; see
  [results](journeys/results.json). Each checks daily solve and shape feedback,
  letter hints, Statistics, on-disk export, CLI reception with byte equality,
  preserving an existing local file via a numbered copy, archive isolation,
  and daily/statistics persistence across restart.
- **25/25 app tests**, **432/432 CLI tests**, including **6/6 receiver fixture
  tests**, passed on Rust 1.85.1.
- Workspace strict clippy (`--all-targets --all-features -- -D warnings`),
  workspace format, **105 Node tests**, POSIX installer tests and both simulator
  runner unit tests passed.
- The documented contributor dry-run passed: published-version gate, tests,
  strict lint, static ARM verification, and deterministic fixture-only
  Beta-shaped package/catalog. The public `2a` × 32 fixture seed was used only
  locally and removed afterward. No release credentials were used.
- The standalone ARM binary also passed `kobo verify`: static ARM EABI5
  hard-float. The build used Rust 1.85.1 and Debian ARM GCC 14.2.

Logs are under [checks](checks/), with only trailing blank lines normalized. Full workspace test status is recorded in
`checks/workspace-summary.json`; do not infer a green workspace test run from
the scoped successes above. An initial fast-exit PTY test encountered a
permission error and passed on focused rerun. Initial CLI packaging tests
could not find a relocated cross-archiver; after configuring its library
path, all 432 CLI tests passed.

Physical touch targets, display waveforms, hardware Back, suspend/resume and
real SSH export remain untested. The simulator export uses synthetic local
files only.

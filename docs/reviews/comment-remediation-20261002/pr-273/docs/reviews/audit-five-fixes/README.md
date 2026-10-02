# Five independent-audit repairs

These fixes address all five findings from the second review of PRs #272–274. They are additive to PR #273. Original evidence and prior source commits remain intact.

| Finding | Fix | Regression evidence |
| --- | --- | --- |
| Parser pending B replaces resumed A (introduced, P2) | Cancel the pending load when resuming the already-open story | Actual shelf-read callback test; pending B completion cannot replace A |
| Kitchen duplicate Back (introduced polish, P3) | Remove body Back on Settings/Finished; retain runtime ownership and Cook again | Emitted-screen diagnostics and callback destination assertions at normal/largest scale |
| Gutenbird capture module injection (introduced tooling, P3) | Insert explicitly into the unique original tests module | Two Python scope tests and the documented Rust capture command, producing ten images |
| Read Later positional selection (pre-existing, P2) | Track open article by stable ID; reconcile queue/cache replacement; return to queue if removed | Queue refresh, cache reorder/removal, archive and star identity assertions |
| Chat one long turn clips (pre-existing, P2) | Measure display fragments, split oversized turns at grapheme boundaries, retain complete original conversation | Every page fits, fragments concatenate exactly, combining/ZWJ graphemes remain intact, latest controls stay latest |

## Source provenance

- Before: `822d01840d24f3826cf4fcfb82172d95a51e720f` (pre-repair PR #273, not original beta).
- App fixes / native AFTER captures: `6271b1debe4311e1a2bcde84a17ef252933e1761`.
- Lock review metadata: `51ee406a3b916ef8826d853e81d75f6db1f1a318`. Chat gains a direct edge to the already-resolved unicode-segmentation package; no third-party package version changes. The protocol-15 compatibility record was reviewed and updated for that Chat-only edge.
- Combined full workspace tests and CLI build: local merge `dcf80eeb920d8cf9d9b8e118ea94082be4fbf9cd` combines app fixes, #272 `7312dedb`, #274 `9efb2545`, beta `880c2192` (including #237).
- Runtime result records name `5643712166a10ef66ff0e7bffac149b07d0beb7c`, which differs from the CLI build source only by lock-review metadata. The binary SHA-256 is retained in each result.
- Beta advanced during testing to `7c17078f069569e6d5b41d6f70f4716456cea55e`; it merges cleanly into local combined `ec0a97e3f97defcffa79775ba0f453e1c32eff6d`. Its sole tree change is Bluetooth HAL code. All 186 HAL tests and strict HAL Clippy pass on that updated union.

## Capture coverage

[Chat](chat.md), [Kitchen](kitchencard.md), [Read Later](readlater.md), [Parser](parser.md), [Gutenbird](gutenbird.md) link every retained full-resolution image. Native manifests record source/app object hashes and output SHA-256 values. Nine profile identities × normal/170% × five apps; 144 BEFORE and 219 AFTER images. Every AFTER scene has zero error-level renderer diagnostics. All images were visually reviewed in contact sheets, with representative full-resolution inspections. Chat shows every fragment page; Read Later shows selection before refresh, after refresh, and archive; Kitchen shows both affected views. Parser library and Gutenbird catalog are visual smokes; the race and capture compilation defects are established by separate callback/compiler tests, not those screenshots.

Runtime navigation is separately exercised with `kobo dev --runtime` on Clara BW391 at 100% and170%. Both five-app journeys pass: Chat long-turn page traversal/overlay/service/root Back; Kitchen and Read Later settings/root Back; Parser tutorial/resume/library/root Back; Gutenbird detour/root Back. Each stay assertion waits beyond the runtime Back grace interval. Result JSON, logs and screenshots are in runtime-100 and runtime-170. Early harness attempts incorrectly counted invisible PageRail actions as visible page buttons; the retained script matches PagePrevious/PageNext nodes and the final runs pass.

## Checks and reproduction

Pinned Rust 1.85.1. Focused app tests pass (Chat60, Gutenbird103, Kitchen19, Parser41 plus1ignored, Read Later34). The earlier focused log has Chat59/Read Later33; the final extra-case logs and combined log cover the added cases. Strict workspace Clippy, cargo fmt, generated-page freshness and 105 Node tests pass. Two capture Python tests pass. The documented Gutenbird capture command passes and produces ten images. Failed-before logs are retained separately.

```sh
cargo test --workspace --all-targets --all-features --no-fail-fast
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all --check
node --test tools/*.test.mjs
node tools/generate-app-pages.mjs
git diff --exit-code
python -m unittest discover -s examples/gutenbird/screenshots/ui-review -p test_capture.py
python docs/reviews/audit-five-fixes/capture.py --root /path/to/source --output /path/to/output --phase after
python docs/reviews/audit-five-fixes/runtime.py --root /path/to/combined-source --cli /path/to/built/kobo --output /path/to/output --scale 170
```

Use an isolated checkout for the BEFORE source and `--phase before`. Capture scripts append test-only scenarios to temporary copies, preserving original source and artifacts.

## Limits

No physical reader, full VM, real provider account, radio, suspend/rotation or device driver validation. Unsupported emoji remain explicit font diagnostics; they no longer cause pagination to explode into one-grapheme pages. Native profile identities share three modeled geometry/PPI groups; this is not nine physical firmware validations. The historical 279+279 routes and72 journeys used ordinary app dev mode, so they do not prove runtime navigation, nor are those old counts new-head results. Existing Settings/Stacks launch-only matrix limits remain; the independent audit had separate Settings and Stacks deeper checks.

A concurrent reviewer requested real-reader Settings testing, evidence relocation and resolving overlap with #236. Those requests are outside these five repairs and remain open; nothing was merged or deployed, and original evidence was not deleted.

Full combined workspace run: **4079 passed, 4 failed, 8 ignored**, across 112 test binaries. Four failures are environment limits: one kobo-abi PTY test receives EPERM, and three kobo-cli packaging command tests require an absent ARM C cross-compiler. These are retained as failures, not reported as a clean full-suite pass. See logs/integration-tests.log.

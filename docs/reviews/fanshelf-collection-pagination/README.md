# Fanshelf collection layout review

## Reproduced on the original source

Source base: `97153048` (Beta). Before changing production code, the review driver
created 18 synthetic, long-title works, fandoms, followed tags, and feed entries.
The unmodified application emitted its screens through `AppRunner`; the genuine
Cobalt text/layout/rendering engine produced the PNGs. These are **renderer and
callback captures, not live simulator or physical-device screenshots**.

The original fixed six-row pages placed the page label and Previous/Next buttons
after the list. On Clara BW, the shelf's Next control was already offscreen at
80% text. At 90%, all five collections lost Next. At larger scales, rows were
also clipped or lost their touch targets. At 170%, the shelf's three navigation
buttons split their labels inside words.

- [Shelf before, 170%](shelf-before-170.png): clipped third work, missing paging controls, split navigation labels
- [Shelf after, 170%](shelf-after-170.png): whole rows, visible page count and pinned paging controls, complete navigation labels
- [Followed tags before, 140%](follow-before-140.png): clipped list and inaccessible Next
- [Followed tags after, 140%](follow-after-140.png): measured list and reachable navigation

Each pair uses the same synthetic data, panel, font, scale, and runtime chrome.
Images are original full-resolution 1072 × 1448 grayscale PNGs, without editing.
The smaller three-work demo did not reveal this collection-size-dependent fault.

## Narrow fix

- Measure all five collection types with `Context::paginate_rows_under`, using
  the exact displayed row strings and the actual prefix, including banners
- Clamp external row titles to two measured lines, following the SDK convention
- Pin Previous/Next in the reserved action bar, with the SDK page-position strip
- Clamp turns against the page actually displayed, including repeated boundary
  taps and after a collection shrinks
- Keep one-page and empty lists free of unnecessary paging controls
- Measure the shelf button labels; if the compact three-column arrangement wraps,
  use a standalone Followed tags row above Updates and Manage
- Keep AO3 parsing, chapter labels, downloads, and shared SDK/runtime code unchanged

## Provenance

- Original `apps/fanshelf/src/main.rs` SHA-256:
  `785ca25eaa53fcbcb19b621a3434c1e81b538856a4fa31fed63a219a898c39a4`
- `CLARA_BW_METRICS`: 1072 × 1448 pixels, 300 ppi
- All nine `TextScale::STEPS`: 80, 90, 100, 110, 120, 130, 140, 155, 170%
- Fonts installed through `kobo_text::install`: Atkinson Hyperlegible Regular
  (interface/reading), Atkinson Hyperlegible Bold (display), DejaVu Sans Mono
- Chrome: `Chrome::for_screen(screen, false, Chrome::measuring(true).status)`,
  plus runtime `ensure_way_back`. The 00:00/battery/radio status is synthetic
- No AO3 requests or user accounts/data were used. Network actions were inspected
  as `AppRunner` commands without executing them
- The supported local simulator could not bind AF_UNIX in the review sandbox;
  elevation was also denied. Its transport was not modified or bypassed
- Hardware touch, e-ink refresh/ghosting, network operation, and attended device
  Settings/About proof remain untested

The before-only driver is preserved as [baseline-capture.rs](baseline-capture.rs).
[The capture script](capture.py) (Python 3.11+) copies app source into a temporary
Cargo package, adds the appropriate review driver and renderer-only dependencies,
and builds offline with a temporary copy of the workspace lockfile. It never
changes the app's Cargo.toml or workspace dependency graph. The `--baseline` mode
reads the original app source directly from Git at `97153048`; the normal mode
copies current app source verbatim. Both use the same SDK, fonts and renderer.
Run a normal workspace Cargo build first to populate its offline dependency cache.

```sh
COBALT_REVIEW_CAPTURE_DIR=/tmp/fanshelf-before \
  python3 docs/reviews/fanshelf-collection-pagination/capture.py --baseline
COBALT_REVIEW_CAPTURE_DIR=/tmp/fanshelf-after \
  python3 docs/reviews/fanshelf-collection-pagination/capture.py
cargo test -p kobo-fanshelf --all-targets --all-features --locked
```

Normal app tests write no screenshots and use the app's existing SDK/UI test
dependencies. The app Cargo.toml and Cargo.lock are byte-identical to the baseline.
The isolated driver reproduced all 90 before PNGs and all 90 after PNGs byte-for-byte.
The four selected PNGs have checksums in [SHA256SUMS](SHA256SUMS).

## Automated verification

The app regression module checks all five collection types at every scale:

- Every page has zero diagnostics and each row/control center hit-tests correctly
- All 18 stable action IDs are reached in order, exactly once
- Forward/reverse paging and repeated first/last-page taps stay bounded
- Normal and notice-bearing lists reserve their actual measured prefix
- Empty, one-item, and shrinking lists recover from out-of-range page indexes
- Opening a work and Back preserves the shelf page; opening a tag and Back
  preserves the followed-tags page
- A fandom selected on a later page filters the correct work; All and repeated
  Updates actions reset the appropriate page
- Shelf navigation labels remain single-line at every scale

Final host checks (Rust 1.85.1):

- `node --test tools/*.test.mjs`: 105 passed
- `cargo fmt --all --check`: passed
- `cargo test -p kobo-fanshelf --all-targets --all-features`: 23 passed
- `cargo test -p kobo-fanshelf --all-targets --all-features --locked --no-run`: passed
- `cargo clippy -p kobo-fanshelf --all-targets --all-features -- -D warnings`: passed
- Isolated before/after capture drivers: passed, all 180 PNGs byte-identical
- App `Cargo.toml`, workspace lockfile and `library.rs` unchanged from `97153048`

- `node tools/app-contribute.mjs --manifest apps/fanshelf/cobalt-app.json --dry-run`:
  passed, including static ARMv7 hard-float verification, package/catalog preview,
  and generated install-page checks
- Verified ARM executable SHA-256:
  `ec057e8fe0023038b93386783bea13b2828906f20f70ad4976dc6ba5c8277061`
- Derived protocol 15 and minimum Cobalt 0.3.18

Full workspace Rust tests and a live simulator/device run were not performed
as part of this app-scoped review.

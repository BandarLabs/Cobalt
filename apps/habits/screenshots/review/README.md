# Habits Stats tab review

The original Stats screen highlighted Today because its selected index was 4 while
the four tabs have indices 0 through 3. The renderer clamps an invalid index to 0.
Stats now uses index 3. The change is limited to selection state.

These before/after images are in-process app/renderer captures, not interactive
simulator screenshots. Both use three daily habits without completions, Clara BW
metrics, the actual interface face, and synthetic status/Back chrome.

```sh
COBALT_REVIEW_OUT=/tmp/habits-review cargo test -p kobo-habits capture_review_page_when_requested
```

Tests select every tab repeatedly, return from Settings to Stats and then Today,
and verify Stats selection, diagnostics and tab targets at all nine text sizes.

Open PR #224 changes skip rows and copy. This branch does not duplicate those source
changes, but both touch the app manifest/generated page; version and release-note
conflicts should be reconciled when rebasing either contribution.

The executor rejects the interactive simulator's Unix socket bind with EPERM.
No interactive simulator or physical-device validation is claimed.

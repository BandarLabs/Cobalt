# Vault list and import-report review

These are in-process app/renderer captures, not interactive-simulator screenshots.
The original implementation produced the before images. Captures use Clara BW
metrics, the installed interface face, and synthetic runtime status/Back chrome.

The original import report clipped a 12-file failure list. Measured pages now keep
all failures and the About guidance reachable. A failure row opens its full filename
and reason, which also page when long. Back returns to the same report page.

A callback journey also reproduced a list-navigation defect: 50 Next actions at a
three-page list left the internal cursor at 50. One Previous changed it to 49 while
the screen remained on page 3. The stored cursor now clamps with the displayed page,
so one Previous immediately returns to page 2. List arrows and position occupy the
reserved page strip rather than unconditionally active in-flow buttons.

```sh
COBALT_REVIEW_OUT=/tmp/vault-review cargo test -p kobo-vault capture_review_pages_when_requested
```

Regression tests cover Browse, Tags, tagged notes, Recent, Search, Backlinks and the
import report at all nine text sizes, every row target, every word of long failure
explanations, repeated page turns, opening a failure and returning with Back.

Interactive simulation is blocked here by Unix socket bind EPERM. No simulator,
live shelf transfer or physical-device success is claimed.

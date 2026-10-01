# Sync Back-navigation review

The existing Back handler already returned to Sync status. However, supporting
screens emitted `owns_back = false`, so the runtime left the app before that handler
could receive a reader Back event. The emitted-screen observations are recorded in
`back-ownership-before.json` and `back-ownership-after.json`; the policy is implemented
in `crates/kobod/src/navigation.rs`.

Setup, Folders and About now claim Back. Status remains the root exit. Enabling that
contract exposed duplicate Back controls through the SDK's `AmbiguousBack` diagnostic,
so the redundant in-content controls are removed and About moves to the folder header.

The before/after guide images and post-Back status image are in-process app/renderer
captures at Clara BW metrics with synthetic runtime status/Back chrome. They are not
interactive simulator screenshots. The status capture shows that cancelling setup
leaves Sync paused.

```sh
COBALT_REVIEW_OUT=/tmp/sync-review python3 apps/syncthing/screenshots/review/capture.py
```

Tests repeatedly enter and leave each supporting view, verify the Back contract and
clean layout, and prove that configuration, cadence, scheduling time, transfer status,
bytes and import reports do not change. A separate paused-setup case stays disabled.

Open PR #224 changes the refresh description. This branch leaves that copy alone,
but its manifest/generated-page edits will need reconciliation if both are rebased.
The executor rejects Unix socket bind with EPERM, so interactive simulation and
physical-device validation remain unverified. No Syncthing engine ran for these tests.

The opt-in capture script copies the app source and the workspace lockfile into a
temporary Cargo package. PNG encoding adds a dependency only to that temporary
package, preserving the app’s reviewed release dependency graph. The normal app
regression suite retains every behavior and layout assertion.

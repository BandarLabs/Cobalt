# Final combined verification

The final combined source is local merge63387b9a9dbd5708552154568bf263602983557f, tree948f54a0cc3a066beab31762e95cf2f274637493. It combines final PR272 b1d15610, PR273 e2a05090, PR274 c641e812, beta7c17078f and PR236 e3d173e8. All full hashes are in source.json. PR273 retains272 and236 as real prerequisite ancestry; require those PRs land first. Their original branches were not modified by this work.

The tree is independently reproducible from the published code heads, without relying on the local merge commit:

```sh
git merge-tree --write-tree e2a05090c1cdd41ca38f25a52dea0263e6fa3403 c641e812cecbac5b12564db7b8894bd613e94f91
# 948f54a0cc3a066beab31762e95cf2f274637493
```

No CLI/sidekickd files differ between the272 head and273 head. No protocol/SDK/policy/Wi-Fi HAL files differ between236 and the integrated273 head; the Settings conflict resolution and bounded trust presentation are the integration changes.

## Test accounting

All nine current-head CI jobs passed for each PR: [272 / 37040204464](https://github.com/BandarLabs/Cobalt/actions/runs/37040204464), [273 / 37041905747](https://github.com/BandarLabs/Cobalt/actions/runs/37041905747), and [274 / 37040872747](https://github.com/BandarLabs/Cobalt/actions/runs/37040872747). Each includes host workspace tests and strict Clippy, device build/packaging, simulator, importer, advisories and four host release builds. All three release-policy checks also passed. These are checks of the respective published heads, separate from the local combined-tree verification below.

- Full workspace all-target/all-feature/no-fail-fast run on combined638ae3d9:4,111 passed,3 failed,3 ignored across113 test binaries. The three failures are the CLI packaging-command tests requiring an absent ARM C cross-compiler. They remain explicitly failed in the retained log. Remote exact-head device/packaging checks are separate.
- The final63387b9a tree differs from638ae3d9 in only examples/settings/src/main.rs and list_tests.rs (the final bounded server-name rendering and expanded real-font regression). All49 Settings tests were rerun successfully on the final combined tree; do not double-count them as additional unique tests.
- Final combined strict workspace Clippy, formatting,106 Node tests, generated pages and clean-tree checks pass. These are exact-final-tree checks; the large full-suite log retains its earlier precise source pin.
- The final real-font regression covers nine modeled profiles at normal/170% with long/extreme certificate names, waiting screen, full fingerprint and decision hit targets. Runtime fixture journeys and their exact source/binary/adapter bindings are documented in ../remediation-validation/settings/RUNTIME-README.md.

No physical reader, real enterprise network/account, suspend/rotation/device driver validation was performed. The production simulator has no discovered Wi-Fi networks; enterprise journeys use a clearly preserved validation-only synthetic fixture. Long server names can be visibly ellipsized; the full fingerprint remains visible. Deck processes deliberately escaping the managed process group remain outside its group-lifetime guarantee.

## Evidence cleanup

Code diff against current beta: PR27210 paths/zero PNGs; PR273212 paths/zero PNGs (previously1,329 paths/556 PNGs/42,305,927 bytes); PR27482 paths/zero PNGs (previously805 paths/236 PNGs/11,397,186 bytes). Original images and source/provenance remain in the immutable archive with2,003 verified file bindings. Reusable capture-only sources moved under tools/review-captures; substantive regression tests remain in apps. Node CI runs lightweight harness checks; six representative capture executions produced81 frames (52PNG/29PGM).

Historical32 large-text failures (24 in273,8 in274) were resolved by37008549549. That historical app-dev matrix is limited action/render coverage, not proof of every runtime navigation path. Settings/Stacks launch-only limits from that run remain explicit. Earlier five-fix CI37032152348 passed at649c3dd8; its runtime provenance correction remains in the archive and current PR description.

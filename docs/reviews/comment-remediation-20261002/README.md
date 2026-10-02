# Preserved evidence for PR272–274 comment remediation

These are byte-identical snapshots of the evidence removed from the code PR diffs. The manifest records original source commits, paths, Git blobs, byte counts and SHA-256 checksums. Original evidence at da90eb60 remains unchanged elsewhere on this branch. No physical-device validation is claimed.

- [PR273 evidence and capture sources](pr-273/): source649c3dd8808ae519271e42fc8afc186b5f0aede1
- [PR274 evidence and capture sources](pr-274/): source9efb2545eb8cf44cde6e9255d3c27d52318d1e28
- PR272 contains no bulk evidence files in its diff.
- [Five-fix audit report](pr-273/docs/reviews/audit-five-fixes/README.md)
- [All-file checksum manifest](manifest.json)

Reproduction: archived scripts retain their original paths and source expectations. Check out the exact source commit listed in the manifest to reproduce that historical capture. Updated lightweight harnesses in the code PRs live under tools/review-captures. Historical screenshots are observations of their pinned sources, not certification of later code.

Runtime provenance correction for the five-fix report: runtime100 records ec0a97e3f97defcffa79775ba0f453e1c32eff6d; runtime170 records5643712166a10ef66ff0e7bffac149b07d0beb7c. Both record the same CLI hash. That CLI was built from dcf80eeb; subsequent source differences were lock-review metadata and beta's Bluetooth-only HAL advance, not a rebuilt runtime binary. Original report and JSON are preserved as received.

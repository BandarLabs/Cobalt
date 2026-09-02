# Release train

Cobalt uses `beta` as its integration and physical-acceptance branch. Stable
artifacts are promoted from beta rather than rebuilt.

## Application changes

1. Open each application pull request against `beta`.
2. Merge only after CI succeeds. The merge publishes a signed beta Store
   catalog; it does not modify the stable catalog.
3. Install or update the application from the beta Store on a physical Kobo.
   Record the beta catalog source commit, catalog SHA-256, package SHA-256, and
   acceptance evidence. Follow
   [Beta Store acceptance](BETA-STORE-ACCEPTANCE.md): run the complete local
   fixture matrix first, then the bounded owner-attended command against the
   exact published beta catalog.
4. Merge `beta` into `main` without squashing away the tested beta commit.
5. Run **Promote tested beta apps** from `main` with the recorded commit and
   catalog digest. The workflow verifies provenance and copies the exact tested
   `.cobalt-app` bytes before replacing the signed stable catalog. Readers
   reject a transient catalog/signature mismatch and retain their last verified
   catalog until the next check.

## Platform changes

1. Merge platform pull requests into `beta`.
2. Bump the workspace version once when cutting a beta candidate. The first
   push of that version publishes immutable `beta-vX.Y.Z` device and host
   assets, their signed manifest, and the bootstrap installer. Later app-only
   pushes at the same version leave that platform release unchanged.
3. Install the beta release through Software Update and record the tested
   commit and archive SHA-256.
4. Merge `beta` into `main` without squashing away the tested commit.
5. Run **Promote tested beta** from `main`. The workflow tags the tested commit
   and publishes the already-tested device package, four host packages,
   installer, manifest, and signatures as `vX.Y.Z` without rebuilding.
   The `docs/install.sh` merged to main is then available from the canonical
   GitHub Pages stable discovery URL. Beta itself never changes the live Pages
   source or publishes a public host bootstrap.

Direct stable publication from `main` is intentionally disabled for Store
applications. A successful build is necessary but does not replace physical
acceptance or artifact promotion. Stable `vX.Y.Z` tags and releases are created
only by **Promote tested beta**; pushing a tag does not start a second build.
Protect the `v*` tag namespace with a repository ruleset that allows tag
creation only by the GitHub Actions promotion workflow.

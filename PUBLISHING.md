# Publishing the SDK crates

The public Rust API is split into small crates. Their local `path` dependencies
also carry exact `0.1.0` registry versions, so a workspace checkout uses local
source while a published crate resolves the same version from crates.io.

Publish a release in dependency order:

1. `kobo-ui` and `kobo-abi`
2. `kobo-protocol` and `kobo-text`
3. `kobo-policy`
4. `kobo-sdk`

For each layer, run `cargo publish --dry-run -p <crate>` and publish it before
checking the next layer. Cargo deliberately resolves registry dependencies when
packaging, so `kobo-sdk` cannot complete its dry run until the preceding crates
exist in the registry at the same version.

Runtime binaries, CLI tools and examples are distributed from this repository,
not as library crates. Pushing a tag `vX.Y.Z` from a clean commit runs the
[release workflow](.github/workflows/release.yml), which checks the tag against
the workspace version, builds the device package per profile, and publishes a
GitHub release with assets named `cobalt-X.Y.Z-<device>-KoboRoot.tgz` alongside
their checksums and third-party notices.

## Stable and beta platform channels

Stable remains the default. Stable readers use GitHub's latest non-prerelease
release and Stable Store catalog. Owners can enable **Beta updates** in
Settings to use prerelease platform releases and the separately signed
`app-catalog-beta` Store catalog. Turning Beta updates off changes future
checks to Stable; it does not delete apps or install older versions.

The long-lived `beta` branch is the only source for beta publishing:

- `.github/workflows/beta-release.yml` builds the workspace version once and
  creates immutable `beta-vX.Y.Z` as a non-latest prerelease. The beta branch
  must advance the workspace version for every published test build; an
  existing beta tag or release is never moved or replaced. Assets retain the
  stable deterministic names, including `cobalt-X.Y.Z-KoboRoot.tgz` and
  `cobalt-X.Y.Z.sha256`.
- `.github/workflows/apps.yml` publishes beta branch apps only to
  `app-catalog-beta`; main continues to publish only to `app-catalog`.
- `.github/workflows/promote-beta.yml` is a manual, guarded promotion. Supply
  `X.Y.Z`, the full tested beta commit SHA, the expected primary archive
  SHA-256, and the exact confirmation `promote-beta-vX.Y.Z`. It requires the
  prerelease tag to target that commit, requires the commit to be in current
  main, enforces the workspace version, verifies the downloaded checksum
  files and expected archive digest, and publishes those exact bytes as
  `vX.Y.Z` targeting the tested commit. It never rebuilds or replaces a tag or
  release.

Promotion is never automatic. Merge the tested beta commit to main first so
the main workspace version matches the artifact being promoted.

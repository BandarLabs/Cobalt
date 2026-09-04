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
not as library crates. The beta workflow builds the device package and the
`kobo` host command for macOS x86_64/arm64 and Linux x86_64/arm64. Host archives
carry the command, the verified updater used by `kobo update`, the project
license, dependency terms, third-party notices, and exact source commit.

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
  `cobalt-X.Y.Z.sha256`. It also publishes four
  `kobo-X.Y.Z-<platform>.tar.gz` archives and a versioned host manifest.
  The manifest fixes every host/device asset name, size, SHA-256, version, and
  source commit. The protected `COBALT_APP_SIGNING_SEED` signs it twice with
  the repository's existing Ed25519 release key: raw detached form for
  in-process verification and standard OpenSSH SSHSIG form for the POSIX
  bootstrap. Pull-request tests use disposable fixture keys and never require
  the protected seed. Cobalt Settings and background platform updates verify
  the raw signature for both Stable and Beta before accepting the device
  archive digest. The same host packages support `kobo update`; its default is
  Stable and its Beta host channel is explicit and independent from the
  in-product platform channel.
- `.github/workflows/apps.yml` publishes beta branch apps only to
  `app-catalog-beta`. Contributors supply source plus one concise manifest;
  Cobalt derives compatibility, builds ARM binaries, signs packages/catalogs,
  and archives each publication transaction.
- `.github/workflows/promote-beta-apps.yml` promotes an archived, tested Beta
  app transaction after the commit is merged to main. The
  `app-store-stable` environment supplies human release approval. Stable
  packages are copied byte-for-byte and never rebuilt; the Stable catalog
  pointer is signed only after exact package size/SHA verification.
- `.github/workflows/promote-beta.yml` is a manual, guarded promotion. Supply
  `X.Y.Z`, the full tested beta commit SHA, the expected primary archive
  SHA-256, and the exact confirmation `promote-beta-vX.Y.Z`. It requires the
  prerelease tag to target that commit, requires the commit to be in current
  main, enforces the workspace version, verifies the downloaded checksum
  files, both manifest signatures, all host/device manifest entries, and the
  expected archive digest. It takes `install.sh` from the exact tested commit,
  verifies it against the signed bootstrap manifest entry, and publishes the
  stable release without rebuilding any binary. It never replaces a tag or
  release.

Promotion is never automatic. Merge the tested beta commit to main first so
the main workspace version matches the artifact being promoted. GitHub Pages
continues to publish `main:/docs`; after that merge, `docs/install.sh` becomes
the canonical stable discovery URL at
`https://bandarlabs.github.io/Cobalt/install.sh`. Beta does not publish a
public host bootstrap; owners opt into beta later through Cobalt Settings.

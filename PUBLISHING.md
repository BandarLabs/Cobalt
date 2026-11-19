# Publishing the SDK crates

Every SDK crate is marked `publish = false` and none is on crates.io today;
applications are built from a checkout of this repository. This is the order
to follow if that changes.

The public Rust API is split into small crates. Most of their local `path`
dependencies also carry a `0.3.2` registry version, so a workspace checkout
uses local source while a published crate would resolve from crates.io. A few
carry no version yet (`kobo-state`, `kobo-doc`, `kobo-html`, and `kobo-net`'s
dependency on `kobo-protocol`), and each needs one before anything that
depends on it can be published.

Publish a release in dependency order:

1. `kobo-ui`, `kobo-abi`, `kobo-json`, `kobo-dict` and `kobo-html`
2. `kobo-protocol`, `kobo-text`, `kobo-state` and `kobo-doc`
3. `kobo-net`
4. `kobo-policy`
5. `kobo-sdk`

For each layer, run `cargo publish --dry-run -p <crate>` and publish it before
checking the next layer. Cargo resolves registry dependencies when packaging,
so `kobo-sdk` cannot complete its dry run until the crates before it exist in
the registry.

Runtime binaries, CLI tools and examples are distributed from this repository,
not as library crates. The beta workflow builds the device package and the
`kobo` host command for macOS x86_64/arm64 and Linux x86_64/arm64. Host archives
carry the command, the verified updater used by `kobo update`, the project
license, dependency terms, third-party notices, and exact source commit.

## Stable and beta platform channels

Stable remains the default. Stable readers use GitHub's latest non-prerelease
release and Stable Store catalog. Owners can choose **Beta** under
**Settings ▸ Software update** to use prerelease platform releases and the
separately signed `app-catalog-beta` Store catalog. Switching back to Stable
changes future checks; it does not delete apps or install older versions.

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
- `.github/workflows/apps.yml` publishes apps from the beta branch only, to
  `app-catalog-beta`. The Stable `app-catalog` changes only through
  `.github/workflows/promote-beta-apps.yml`, which copies the tested Beta
  packages byte for byte (see
  [docs/CONTRIBUTING_APPS.md](docs/CONTRIBUTING_APPS.md#stable-promotion)).
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
the main workspace version matches the artifact being promoted.
`.github/workflows/pages.yml` then builds the site from `docs/` on main and
serves `docs/install.sh` at `https://bandarlabs.github.io/Cobalt/install.sh`.
Beta has no public install script. Owners switch to Beta in Cobalt Settings.

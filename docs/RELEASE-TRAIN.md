# Release train

Cobalt uses `beta` as its integration and physical-acceptance branch. Stable
artifacts are promoted from beta rather than rebuilt.

The beta **platform** (`beta-vX.Y.Z`) and the beta **Store catalog**
(`app-catalog-beta`) are separate publications. A Store-only merge updates the
catalog without a workspace version bump. A platform-only merge publishes a new
`beta-vX.Y.Z` without bumping every Store app. Promoting one to stable does not
promote the other.

On every push to `beta`, two workflows start independently. Neither waits on
the other:

| Workflow | File | Concurrency | Publishes |
|---|---|---|---|
| **Publish apps** | `.github/workflows/apps.yml` | `cobalt-app-catalog-beta` | `app-catalog-beta` |
| **Publish beta platform** | `.github/workflows/beta-release.yml` | `cobalt-beta-platform` | `beta-vX.Y.Z` |

A Store-only change (`apps/`, or a registered Store package that still lives
under `examples/`) leaves the platform run quiet and green. A platform-only
change (`crates/`, device-only `examples/`, `Cargo.toml`, `Cargo.lock`, the
toolchain pin) leaves the catalog run quiet and green after it re-verifies the
already-published `app-catalog-beta`. Either run fails, rather than skipping,
when the inputs it owns moved and it decided not to publish.

Re-run a quiet channel from the Actions tab with the `beta` branch selected, or:

```
gh workflow run "Publish apps" --ref beta
gh workflow run "Publish beta platform" --ref beta
```

## Application changes

1. Open each application pull request against `beta`.
2. Merge only after CI succeeds. The merge publishes a signed beta Store
   catalog; it does not modify the stable catalog and does not require a
   workspace version bump.
3. Install or update the application from the beta Store on a physical Kobo.
   Record the beta catalog source commit, catalog SHA-256, package SHA-256, and
   acceptance evidence.
4. Merge `beta` into `main` without squashing away the tested beta commit.
   This merge does not publish a stable catalog and does not create `vX.Y.Z`.
5. Run **Promote tested beta apps** from `main` with the recorded commit and
   catalog digest. The workflow verifies provenance and copies the exact tested
   `.cobalt-app` bytes before replacing the signed stable catalog. Readers
   reject a transient catalog/signature mismatch and retain their last verified
   catalog until the next check.

   From the Actions tab, select **Promote tested beta apps**, branch `main`:

   - `tested_commit`: the 40-character beta commit recorded in step 3
   - `expected_catalog_sha256`: SHA-256 of that commit's `cobalt-app-catalog.json`
   - `confirmation`: `promote-apps-` plus the first 12 characters of the commit

   ```
   gh workflow run "Promote tested beta apps" --ref main \
     -f tested_commit=COMMIT \
     -f expected_catalog_sha256=DIGEST \
     -f confirmation=promote-apps-SHORTSHA
   ```

   Do not run **Promote tested beta**. That workflow publishes `vX.Y.Z` and
   does not touch the Store catalog.

## Platform changes

1. Merge platform pull requests into `beta`.
2. Bump the workspace version once when cutting a beta candidate. The first
   push of that version publishes immutable `beta-vX.Y.Z` device and host
   assets, their signed manifest, and the bootstrap installer. Later app-only
   pushes at the same version leave that platform release unchanged, and a
   platform bump does not republish `app-catalog-beta`.
3. Install the beta release through Software Update and record the tested
   commit and archive SHA-256.
   For a protocol transition, OTA over an existing protocol-11 installation
   before installing protocol-12 apps. Confirm old apps retain their state,
   secrets and preferences, keep their legacy layout, can return to Nickel,
   and can roll back. Confirm protocol-12 Folio, protocol-13 selected-cell,
   and protocol-14 PUT Store apps are beta-gated with their
   `minimum_cobalt_version` before promotion.
4. Merge `beta` into `main` without squashing away the tested commit.
   This merge does not create `vX.Y.Z` and does not replace the stable catalog.
5. Run **Promote tested beta** from `main`. The workflow tags the tested commit
   and publishes the already-tested device package, four host packages,
   installer, manifest, and signatures as `vX.Y.Z` without rebuilding.
   The `docs/install.sh` merged to main is then available from the canonical
   GitHub Pages stable discovery URL. Beta itself never changes the live Pages
   source or publishes a public host bootstrap.

   From the Actions tab, select **Promote tested beta**, branch `main`:

   - `version`: the exact workspace version, for example `0.3.5`
   - `tested_commit`: the 40-character commit recorded in step 3
   - `expected_archive_sha256`: SHA-256 of `cobalt-VERSION-KoboRoot.tgz`
   - `confirmation`: `promote-beta-v` plus the same version

   ```
   gh workflow run "Promote tested beta" --ref main \
     -f version=X.Y.Z \
     -f tested_commit=COMMIT \
     -f expected_archive_sha256=DIGEST \
     -f confirmation=promote-beta-vX.Y.Z
   ```

   Do not run **Promote tested beta apps**. That workflow replaces the stable
   Store catalog and does not create `vX.Y.Z`.

Direct stable publication from `main` is intentionally disabled for Store
applications. A successful build is necessary but does not replace physical
acceptance or artifact promotion. Stable `vX.Y.Z` tags and releases are created
only by **Promote tested beta**; pushing a tag does not start a second build.
Protect the `v*` tag namespace with a repository ruleset that allows tag
creation only by the GitHub Actions promotion workflow.

Beta is opt-in: Stable readers do not move channels just because a beta exists.
Returning to Stable changes only future checks; it never forces a beta
downgrade, and Stable resumes once GA catches or exceeds that beta. OTA is a
verified staged swap over Wi-Fi, not a USB requirement. Acceptance evidence
must include rollback after the staged swap and a return through the normal
Nickel handoff, as well as confirmation that owner secrets, trust, state, data,
apps, and Store data all survived.

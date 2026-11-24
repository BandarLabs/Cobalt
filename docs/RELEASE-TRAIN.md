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
   - `expected_catalog_sha256`: SHA-256 of that commit's `cobalt-app-catalog-v2.json`
     (its `cobalt-app-catalog.json` if the Beta release has no v2 file yet).
     The workflow promotes both catalogs.
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
   In the same commit, rewrite `examples/launcher/whats-new.txt` for the new
   version; see [What's new](#whats-new).
3. Install the beta release through **Settings ▸ Software update** and record
   the tested commit and archive SHA-256.
   From 0.3.38 an update is staged beside the installation and takes over the
   next time Cobalt opens. Before closing Cobalt after an update, open two
   apps and confirm they still run on the old release; after reopening,
   confirm Settings names the new version.
   For a protocol bump, update over the air from an installation on the
   previous protocol before installing apps built for the new one. Confirm old
   apps retain their state, secrets and preferences, keep their layout, can
   return to the stock reader, and can roll back. Confirm a Store app on the
   new protocol is beta-gated by its `minimum_cobalt_version` before promotion.
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

### What's new

The first time the launcher opens after an update it shows a short sheet,
once: `What's new in Cobalt X.Y.Z` and the changes listed in
`examples/launcher/whats-new.txt`. The file is compiled in and is shown only
under the version on its first line, so a release cut without rewriting it
tells owners nothing.

In the commit that bumps the workspace version:

1. Put the new version on the file's first line.
2. Replace every change with this release's: two to five lines, each
   `Lead words: one sentence.`, the one most worth knowing first. A large
   text size drops changes from the end.
3. Put the same version and leads in `examples/launcher/drive-whats-new.kobo`.
4. Run `cargo test -p kobo-launcher`. It checks the format, that the list
   fits every supported panel at the default text size, and that the file is
   for this version or the next patch. That last check is the one that fails
   when a bump leaves the file behind.

### Release notes

After a beta is published, write its notes onto the GitHub release in place
of the line the workflow leaves there. Settings reads them on readers that
are offered this version, before the owner installs it, so they are written
for that page as much as for the browser:

- Each change is one list item, `- **Lead.** First sentence. More.` The lead
  is bold and ends in a full stop.
- The reader shows the lead and the first sentence only, and at most five
  changes, so the first sentence must say what changed on its own, in under
  about twenty words. Anything after it stays on the release page.
- Paragraphs that are not list items, such as what has not been seen on a
  reader and the commit it was built from, are not shown on the reader.

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
must include rollback after the staged swap and a return to the stock reader
through the normal handoff, as well as confirmation that owner secrets, trust, state, data,
apps, and Store data all survived.

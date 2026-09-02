# Contributing applications

A Store contribution has only two hand-authored parts: app source and one
concise manifest. The contributor command generates website files as ordinary
reviewable outputs. Contributors do not hand-edit release catalogs,
minimum-platform tables, package lists, signing configuration, GitHub
releases, or Stable promotion inputs.

## Add an app

Copy `templates/app/` to `apps/<app-id>/`, rename the Cargo package to
`kobo-<app-id>`, and make the runtime identity passed to `kobo_sdk::run` equal
`<app-id>`.

The only Store metadata file is `apps/<app-id>/cobalt-app.json`:

```json
{
  "id": "example",
  "display_name": "Example",
  "short_label": "Example",
  "summary": "Describe what the app lets a Kobo owner do.",
  "version": "1.0.0",
  "glyph": "app",
  "capabilities": []
}
```

`package` is derived as `kobo-<id>`. `minimum_cobalt_version` is derived from
the current SDK protocol and capability policy in
`tools/protocol-minimums.json`. Supplying either field is an error: release
plumbing belongs to Cobalt.

Optional `setup.steps` may describe account or self-hosting prerequisites.
Use one to six short steps. Links must be absolute HTTPS URLs without embedded
credentials. Setup text is website-only and never enters the signed package.

New directories under `apps/` are workspace members automatically. The
effective registry is assembled from Cobalt's built-in base entries and every
`apps/*/cobalt-app.json`; duplicate IDs or packages fail closed.

## Run the one contributor check

From the repository root:

```sh
node tools/app-contribute.mjs \
  --manifest apps/<app-id>/cobalt-app.json \
  --dry-run
```

That one command:

1. validates the directory, Cargo package, manifest, capabilities, and
   generated registry;
2. derives the protocol and minimum compatible Cobalt release;
3. runs workspace formatting plus the app's tests and strict clippy;
4. cross-builds and verifies the static ARMv7 hard-float executable;
5. builds a deterministic pathless `.cobalt-app` and signed Beta-shaped
   catalog using the public test-only fixture key; and
6. generates the install page/sitemap and writes hashes and derived values
   under `target/app-contribute/<app-id>/`.

Commit generated `docs/apps/<app-id>/` and `docs/sitemap.xml` changes produced
by the command. They are automation output, not additional metadata to design
or maintain.

The preview exercises the same bundle/catalog commands as publishing, but its
key and `example.invalid` URL make it impossible to mistake for a release.
There is intentionally no local publish mode.

If the ARM compiler is missing, install `gcc-arm-linux-gnueabihf` on Debian or
an equivalent `armv7-unknown-linux-musleabihf` compiler on macOS. Every failure
names the failed command and the next corrective action.

For interactive layout work:

```sh
cd apps/<app-id>
cargo run --manifest-path ../../crates/kobo-cli/Cargo.toml -- dev
```

Tests should validate screens with `CLARA_BW_METRICS`, including tappability
and stable layout after state changes.

## Pull request and Beta publication

Open the app pull-request template against `beta`. Human review is for product
policy: purpose, licensing, capabilities, setup requirements, and whether the
public/demo evidence is appropriate. Mechanical release work is automated.

Pull-request CI has read-only repository permissions and runs:

- formatting, Node policy tests, full Rust tests, and strict clippy;
- generated registry/page freshness;
- ARM cross-checks and static executable verification; and
- published-version/protocol compatibility gates.

After merge to `beta`, `Publish apps`:

1. selects only affected apps and reuses unchanged verified binaries;
2. builds each selected ARM app on an isolated runner;
3. downloads immutable binaries onto a separate signing runner;
4. creates deterministic signed packages and the complete signed Beta catalog;
5. records source commit, workflow run/attempt, and catalog SHA-256;
6. archives versioned catalog/signature/provenance transaction assets for
   rollback; and
7. moves the fixed Beta catalog pointer only after every referenced package is
   present.

The signing seed exists only in the publish job. Fork pull requests, build
jobs, tests, and contributors never receive it. Build jobs have read-only
permissions; only the final publish job receives `contents: write`.

Run the documented `kobo beta-store-smoke` local fixture and attended device
proof after publication. Automation retains exact catalog/package identities,
functional evidence, and mandatory public-route marketing artifacts. Physical
hardware is never assumed by ordinary CI.

## Stable promotion

Merge the tested Beta commit to `main`, then dispatch **Promote tested beta
apps** with that commit. The `app-store-stable` GitHub environment supplies
the human release approval. No digest, signature, seed, release upload, or
confirmation phrase is entered manually.

Promotion finds the archived Beta transaction whose provenance names the
tested commit, verifies its catalog digest, downloads every package named by
that catalog, and compares exact size/SHA-256. Stable package assets are copied
byte-for-byte and never rebuilt. Only the signed Stable catalog pointer is
regenerated to replace Beta asset URLs with Stable URLs; its provenance binds
the tested Beta commit and source catalog digest.

Stable catalog, signature, and provenance are archived per workflow
run/attempt before the fixed pointer changes. Those immutable transaction
assets preserve audit history and provide a known-good rollback source.

Repository/environment protection should require approval only for merging
policy-sensitive app changes and for the Stable environment. Formatting,
tests, builds, signing, publication, provenance checks, evidence collection,
and byte copying are automation decisions.

# Contributing applications

Store applications live under `apps/` and are published independently from
the Cobalt platform.

## Add an app

1. Create `apps/<app-id>/Cargo.toml` and `apps/<app-id>/src/main.rs`.
2. Name the Cargo package `kobo-<app-id>`.
3. Add the package to the workspace members in the root `Cargo.toml`.
4. Add one entry to `apps/catalog.json`.
5. Add the package to `STORE_PACKAGES` in `crates/kobo-cli/src/main.rs`.

Store apps are downloaded only when an owner installs them. Do not add a
contributed app to `INSTALLED_PACKAGES` or `MANAGED_BUILTINS`; those lists put
the binary in the Cobalt platform package and are reserved for system apps.

Registry fields:

| Field | Meaning |
|---|---|
| `package` | Workspace Cargo package, such as `kobo-sudoku` |
| `id` | Stable lowercase Store and launcher identifier |
| `display_name` | Full Store title |
| `short_label` | Compact launcher label |
| `summary` | Short Store description |
| `version` | App release version, independent from Cobalt. Bump it whenever the published binary or metadata changes |
| `minimum_cobalt_version` | Oldest platform release that supports the SDK protocol and every runtime service the app uses |
| `glyph` | A built-in Cobalt glyph name |
| `capabilities` | Runtime services the app needs |
| `setup` | Optional website-only prerequisites shown before the install controls |

Public apps cannot use a platform-reserved ID or request the `shell`
capability. Request only capabilities the app actually uses.

If an app needs an account, API key, named secret, self-hosted service, or
other preparation outside Cobalt, add a `setup` object to its registry entry.
The generated install page puts these steps in a **Before you install** panel,
so owners see the requirements before installing rather than discovering them
on first launch. Each step has plain `text` and may add one HTTPS `link` and
one shell `command`:

```json
"setup": {
  "steps": [
    {
      "text": "Create a dedicated read-only key.",
      "link": {
        "label": "Service key settings",
        "url": "https://example.com/settings/keys"
      }
    },
    {
      "text": "Install the key under the exact secret name used by the app.",
      "command": "kobo secret set service --device <address>"
    },
    {
      "text": "Launch the app and finish its on-device setup."
    }
  ]
}
```

Use one to six short steps. Links must be absolute HTTPS URLs without embedded
credentials. Do not put HTML or Markdown in these fields; the generator
escapes all catalog text. The setup block is website metadata and does not
enter the signed device catalog, so correcting these instructions does not by
itself require an app version bump.

Apps built from the current SDK require Cobalt 0.2.4 or newer because that is
the first release supporting the current wire protocol. A newer runtime
service can require a higher minimum. CI rejects a registry entry below the
SDK protocol floor.

## Test the app

Run unit and workspace checks:

```sh
cargo test -p kobo-<app-id>
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all --check
```

Run the complete host runtime:

```sh
cargo run -p kobo-cli -- run --sim --app <app-id>
```

Run the browser simulator from the app directory:

```sh
cd apps/<app-id>
cargo run --manifest-path ../../crates/kobo-cli/Cargo.toml -- dev
```

Layout tests should use `CLARA_BW_METRICS` and verify that controls fit, remain
tappable, and do not move when app state changes.

## Show the app running

Every new app pull request needs two different kinds of visual evidence:

1. Attach a GIF, video, or photos showing the app running on a physical,
   fully supported Kobo. This is review evidence: it proves the real panel,
   touch controls, page buttons where applicable, and return to the launcher.
2. Check in one clean 1072×1448 panel screenshot for the app README and its
   generated website install page. This is the product image: use a direct
   panel capture without a bezel, hand, camera perspective, or e-ink residue.

Put the clean image under `apps/<app-id>/screenshots/`, show it from
`apps/<app-id>/README.md`, then choose a site filename and copy the same bytes
to `docs/media/site/apps/<site-filename>.png`. Register that exact filename and
useful alt text in the `screenshots` map in `tools/generate-app-pages.mjs`. A
photograph attached to the pull request does not replace this clean checked-in
image.

Add the app's card and `apps/<app-id>/` link to `docs/index.html`, then generate
and commit the install page and sitemap:

```sh
node tools/generate-app-pages.mjs
git diff --check
```

The publish workflow runs the generator again and refuses stale generated
files. It does not create or commit missing screenshots, homepage cards,
install pages, or sitemap entries after merge.

## Publish or update an app

Open a pull request containing the app source, tests, workspace and Store
entries, registry metadata, README, clean screenshot, generated website files,
and physical-device evidence. Increment the app's `version` whenever its code,
local dependencies, release inputs, or public metadata change. Shared SDK and
protocol changes count because they produce a different binary. CI compares
each package with the last published catalog and rejects a reused version.

Do not change the Cobalt platform version for an ordinary app update. Raise
`minimum_cobalt_version` when the app starts using a protocol or runtime
service absent from older platform releases.

After merge to `main`, `.github/workflows/apps.yml`:

1. Builds every registered app as static ARMv7 hard-float on its own runner.
2. Verifies and uploads exactly that app's executable.
3. Downloads the immutable artifacts on a fresh signing runner.
4. Creates signed `.cobalt-app` packages.
5. Creates and signs the complete catalog.
6. Updates the fixed `app-catalog` GitHub release.

Installed readers fetch:

- `https://github.com/BandarLabs/Cobalt/releases/download/app-catalog/cobalt-app-catalog.json`
- `https://github.com/BandarLabs/Cobalt/releases/download/app-catalog/cobalt-app-catalog.json.sig`

The signing seed is available only to the protected repository workflow. Pull
requests never need access to it.

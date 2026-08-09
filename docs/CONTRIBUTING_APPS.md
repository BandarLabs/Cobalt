# Contributing applications

Store applications live under `apps/` and are published independently from
the Cobalt platform.

## Add an app

1. Create `apps/<app-id>/Cargo.toml` and `apps/<app-id>/src/main.rs`.
2. Name the Cargo package `kobo-<app-id>`.
3. Add the package to the workspace members in the root `Cargo.toml`.
4. Add one entry to `apps/catalog.json`.

Registry fields:

| Field | Meaning |
|---|---|
| `package` | Workspace Cargo package, such as `kobo-sudoku` |
| `id` | Stable lowercase Store and launcher identifier |
| `display_name` | Full Store title |
| `short_label` | Compact launcher label |
| `summary` | Short Store description |
| `version` | App version, independent from Cobalt |
| `minimum_cobalt_version` | Oldest compatible platform version |
| `glyph` | A built-in Cobalt glyph name |
| `capabilities` | Runtime services the app needs |

Public apps cannot use a platform-reserved ID or request the `shell`
capability. Request only capabilities the app actually uses.

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

## Publish or update an app

Open a pull request containing the app source, tests, workspace entry, and
registry metadata. Increment only the app's `version` when updating that app.
A Cobalt version change is needed only when the app requires a new platform or
SDK capability.

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

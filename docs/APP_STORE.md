# App Store publishing

Cobalt platform releases and Store app releases are separate:

- Tagged `v*` releases publish the USB-installable Cobalt platform package.
- Every accepted merge to `main` runs the app publishing workflow.
- App-only changes do not require a Cobalt version bump or platform update.

Installed readers use the fixed app channel:

- `https://github.com/BandarLabs/Cobalt/releases/download/app-catalog/cobalt-app-catalog.json`
- `https://github.com/BandarLabs/Cobalt/releases/download/app-catalog/cobalt-app-catalog.json.sig`

## Registry

Store apps are workspace packages under `apps/` and are declared in
`apps/catalog.json`. The registry supplies public metadata; binary size and
SHA-256 are calculated from the exact ARM release binary during publishing.

Sudoku is the first registered app. It is not in the platform package's
`INSTALLED_PACKAGES`, so installing it proves that Wi-Fi delivery works.

See [CONTRIBUTING_APPS.md](CONTRIBUTING_APPS.md) for the contribution format.

## Publishing workflow

`.github/workflows/apps.yml` runs on every push to `main`. It:

1. Builds each registered Cargo package for
   `armv7-unknown-linux-musleabihf`.
2. Rejects binaries that are not static ARM hard-float executables with a real
   executable load segment.
3. Builds a canonical manifest from registry metadata and the binary digest.
4. Signs a pathless single-binary `.cobalt-app` package.
5. Builds and signs the complete catalog.
6. Replaces the assets on the fixed `app-catalog` GitHub release.

The workflow uses the protected `COBALT_APP_SIGNING_SEED` secret. Publishing
fails if the seed does not derive the public key pinned in released runtimes.

For local release validation:

```sh
kobo app-release \
  --registry apps/catalog.json \
  --seed /secure/cobalt-app-signing-seed \
  --out dist/apps \
  --base-url https://github.com/BandarLabs/Cobalt/releases/download/app-catalog
```

## Runtime verification

The catalog signature covers canonical catalog JSON. Each entry fixes the
package HTTPS URL, size, and SHA-256. Each package contains:

- Format magic and version
- Canonical manifest length
- Detached Ed25519 manifest signature
- Canonical manifest
- One executable byte string

The format contains no archive paths, links, scripts, or root filesystem
members.

Catalog JSON and signature are cached as one directory transaction. Installed
apps retain `manifest.json.sig`. Every capability lookup and launch re-verifies
the signed manifest and installed binary.

## Paid delivery later

Public GitHub assets cannot enforce payment. A future paid service can keep the
same signed package format while QR activation and Stripe checkout grant a
device entitlement and short-lived package URL.

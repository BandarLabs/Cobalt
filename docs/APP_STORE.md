# App Store publishing

Cobalt platform releases and Store app releases are separate:

- Tagged `v*` releases publish the USB-installable Cobalt platform package.
- Every accepted merge to `main` runs the app publishing workflow.
- App-only changes do not require a Cobalt version bump or platform update.

Installed readers use the fixed app channel:

- `https://github.com/BandarLabs/Cobalt/releases/download/app-catalog/cobalt-app-catalog.json`
- `https://github.com/BandarLabs/Cobalt/releases/download/app-catalog/cobalt-app-catalog.json.sig`

## Install links

Each catalog app also has a shareable page at
`https://bandarlabs.github.io/Cobalt/apps/<app-id>/`. A reader can link a
browser without creating an account:

1. Open **App Store** on the Kobo and select **Install links**.
2. Scan the QR code, or open the displayed address and enter its eight-character
   pairing code.
3. Choose **Install** on an app page.

The browser encrypts the app identity for that Kobo before sending it. The
relay cannot read the request, and it never receives package URLs or signing
keys. `kobod` resolves the identity through the signed catalog and performs the
same package verification as an install started on the device.

Install requests wait for up to 24 hours. If the Kobo is offline, open Cobalt
App Store after reconnecting to process the queue. The result distinguishes a
new install, an update, an app already at the current version, an app included
with Cobalt, and an app unavailable from the current catalog.

Use **Disconnect all** on the Kobo to revoke every linked browser. With SSH
enabled, the host maintenance command can do the same:

```sh
kobo app-link unpair --device <reader-address>
```

## Registry

Store apps are workspace packages declared in `apps/catalog.json`. The
registry supplies public metadata; binary size and SHA-256 are calculated from
the exact ARM release binary during publishing.

The initial Cobalt applications are registered too. Their `0.2.0` copies are
bundled for a useful first boot, appear as installed in Store, and can later be
updated, removed, or reinstalled through the same signed channel. Sudoku is
not in the platform package, so installing it proves that Wi-Fi delivery works
for an app absent from the reader.

See [CONTRIBUTING_APPS.md](CONTRIBUTING_APPS.md) for the contribution format.

## Publishing workflow

`.github/workflows/apps.yml` runs on every push to `main`. It:

1. Validates the registry and creates a package matrix.
2. Builds each registered Cargo package on a separate runner.
3. Rejects binaries that are not static ARM hard-float executables with a real
   executable load segment.
4. Uploads exactly one immutable artifact from each app runner.
5. Downloads those artifacts on a fresh runner that has not executed app code.
6. Builds and signs the packages and catalog only after that isolation.
7. Replaces the assets on the fixed `app-catalog` GitHub release.

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

## Test before a version release

A local Clara BW can run the complete flow without a tagged Cobalt release:

1. Install or deploy the current development platform build with `kobo setup`
   over USB or `kobo deploy --device <address>` over SSH.
2. Build signed app assets with `kobo app-release`.
3. Upload those assets to the fixed `app-catalog` release with
   `gh release upload app-catalog dist/apps/* --clobber`.
4. On the reader, refresh Store and test install, update, uninstall, and
   reinstall.

This does not create a `v*` platform tag. Updating `app-catalog` affects every
reader already running a Store-capable development build, so use it only with
reviewed assets signed by the production app key.

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

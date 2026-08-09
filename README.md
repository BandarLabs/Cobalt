<p align="center">
  <img src="docs/logo.svg" width="220" alt="Cobalt">
</p>

<p align="center"><strong>Apps and an SDK for Kobo e-readers.</strong></p>

Cobalt is an open-source application platform for Kobo. It provides a launcher,
an App Store, a Rust SDK, a runtime with capability isolation, and a Clara BW
simulator.

After one USB installation, users can install, update, and remove signed apps
over Wi-Fi. App releases are independent from Cobalt platform releases, so a
new app can appear in Store without reinstalling or updating Cobalt.

> [!IMPORTANT]
> Cobalt currently supports only the **Kobo Clara BW N365 (device code 391)**.
> It is an independent project and is not affiliated with Rakuten Kobo.

## Features

- Signed Wi-Fi app installation, updates, and removal
- Separate Settings-based updates for the Cobalt platform
- Apps run as separate unprivileged processes
- Per-app capability checks for network, storage, audio, frontlight, and other
  device services
- Declarative e-ink UI toolkit and browser simulator
- Full and partial refresh planning for the 1072 x 1448 Clara BW panel
- Static ARMv7 binaries with no device-side package manager
- Recovery-safe app and catalog transactions

## Install

Install Rust, add the ARM target and connect a charged Clara BW over USB:

```sh
git clone https://github.com/BandarLabs/Cobalt.git
cd Cobalt
rustup target add armv7-unknown-linux-musleabihf
cargo run -p kobo-cli -- setup
```

Restart the reader and open **Cobalt** from Kobo's menu. Future applications
are installed from **Store** over Wi-Fi. Full Cobalt updates remain under
**Settings**.

See [docs/INSTALL.md](docs/INSTALL.md) for the complete walkthrough and
recovery steps.

## App Store

Store reads a signed catalog from the fixed `app-catalog` GitHub release. Each
package contains one ARM executable and a signed canonical manifest. The
runtime verifies the catalog, package, installed manifest, and binary before
launch.

Apps are published automatically when an app PR is merged into `main`.
Publishing an app does **not** require changing the Cobalt version or creating
a platform release.

Sudoku is the first Store-only application and is intentionally absent from
the USB platform package.

## Build an app

```sh
cargo install --path crates/kobo-cli
kobo new my-app
cd my-app
kobo dev
```

`kobo dev` runs the app in the Clara BW browser simulator. The SDK guide is in
[SDK.md](SDK.md).

## Contributing apps

App contributions are regular pull requests:

1. Add the app as a workspace package under `apps/<app-id>/`.
2. Add its release metadata to `apps/catalog.json`.
3. Add unit and Clara BW layout tests.
4. Run the app in the browser and runtime simulators.
5. Open a pull request.

After the PR is reviewed and merged, the `Publish apps` workflow builds every
registered app for ARM, signs the packages and catalog, and updates the fixed
Store channel. App versions are independent from the Cobalt platform version.

See [docs/CONTRIBUTING_APPS.md](docs/CONTRIBUTING_APPS.md) for metadata,
capabilities, testing, and release details.

## Repository layout

| Path | Purpose |
|---|---|
| `apps/` | Store applications and release registry |
| `examples/` | Built-in applications and SDK examples |
| `crates/kobo-sdk` | Public application SDK |
| `crates/kobod` | Device runtime |
| `crates/kobo-ui` | Layout and e-ink renderer |
| `crates/kobo-sim` | Clara BW browser/runtime simulator |
| `crates/kobo-app-store` | Signed package and catalog formats |
| `crates/kobo-cli` | Setup, build, simulation, packaging, and release tools |
| `docs/` | Installation, device, app publishing, and development guides |

## Development

```sh
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all --check
cargo run -p kobo-cli -- run --sim --app sudoku
```

Additional guides:

- [Developing Cobalt](docs/DEVELOPING.md)
- [Working with devices](docs/DEVICES.md)
- [Publishing apps](docs/APP_STORE.md)
- [Porting to another Kobo](docs/PORTING.md)
- [Security policy](SECURITY.md)

## Safety and support

Cobalt does not replace Kobo's boot chain. Device support is explicitly gated
by hardware and firmware identity, and a reboot returns to the stock reader.
The first installation still modifies files on the user storage partition and
is provided without warranty.

Only the Clara BW profile has been tested. Do not install Cobalt on another
model until that model has a reviewed and hardware-tested profile.

## License

GNU Affero General Public License v3.0. See [LICENSE](LICENSE) and
[THIRD-PARTY.md](THIRD-PARTY.md).

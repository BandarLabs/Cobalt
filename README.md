<p align="center">
  <img src="docs/logo.svg" width="220" alt="Cobalt">
</p>

<p align="center"><strong>Apps and an SDK for Kobo e-readers.</strong></p>

<p align="center">
  <a href="https://github.com/BandarLabs/Cobalt/actions/workflows/ci.yml"><img src="https://github.com/BandarLabs/Cobalt/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI"></a>
  <a href="https://github.com/BandarLabs/Cobalt/actions/workflows/apps.yml"><img src="https://github.com/BandarLabs/Cobalt/actions/workflows/apps.yml/badge.svg?branch=main" alt="Publish apps"></a>
  <a href="https://github.com/BandarLabs/Cobalt/releases/latest"><img src="https://img.shields.io/github/v/release/BandarLabs/Cobalt?color=brightgreen" alt="Latest release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/BandarLabs/Cobalt?color=brightgreen" alt="License"></a>
</p>

Cobalt is an open-source application platform for Kobo. It provides a launcher,
an App Store, a Rust SDK, a runtime with capability isolation, and a Clara BW
simulator.

See the [public roadmap](ROADMAP.md) for the product outcomes Cobalt is working
toward and the principles used to choose them.

After one USB installation, users can install, update, and remove signed apps
over Wi-Fi. App releases are independent from Cobalt platform releases, so a
new app can appear in Store without reinstalling or updating Cobalt.

<p align="center">
  <a href="docs/cobalt-tour.mp4">
    <img src="docs/tour.gif" height="600" alt="A real Kobo Clara BW running Audiobook Studio, Gutenbird, Terminal, Components, Hacker News, Sidekick, AI Command Center, Feeds, Tic-tac-toe and audio playback, followed by the full App Store catalog and Sudoku being installed, played, removed and reinstalled over Wi-Fi">
  </a><br>
  <sub>Recorded on a Kobo Clara BW at 3× speed: apps, Store discovery, and the complete Sudoku install lifecycle.</sub>
</p>

> [!IMPORTANT]
> The **Kobo Clara BW N365 (device code 391)**, **Kobo Elipsa 2E N605 (device
> code 389)**, **Kobo Clara HD N249 (device code 376)**, **Kobo Libra 2 N418
> (device code 388)**, **Kobo Clara Colour N367 (device code 393)**, and
> **Kobo Libra Colour N428 (device code 390)** are
> fully hardware-tested on the exact firmware and kernel versions in the
> support matrix. See the
> [device support matrix](docs/DEVICES.md#device-support-matrix) before
> installing.
> The 2025 **Kobo Clara BW P365 (device code 395)** hardware refresh is also
> supported: its measured panel, touch, firmware, and kernel facts match the
> attended-tested N365 Clara BW.
> It is an independent project and is not affiliated with Rakuten Kobo.

> [!TIP]
> **Own an unsupported Kobo? Help test its port.** No coding is required.
> [Join an existing device thread or create a new one](https://github.com/BandarLabs/Cobalt/issues)
> with your exact model, firmware, and whether you can run attended tests.
> Start with read-only checks; run panel tests only against the commit named
> by a maintainer. See
> [Contributing](CONTRIBUTING.md#device-testing).

## Features

- Signed Wi-Fi app installation, updates, and removal
- Shareable app pages with encrypted QR or pairing-code installation
- Separate Settings-based updates for the Cobalt platform
- Apps run as separate unprivileged processes
- Per-app capability checks for network, storage, audio, frontlight, and other
  device services
- Declarative e-ink UI toolkit and browser simulator
- Profile-driven full and partial refresh planning for supported panels
- Static ARMv7 binaries with no device-side package manager
- Recovery-safe app and catalog transactions

## How it differs

[NickelMenu](https://pgaskin.net/NickelMenu/) adds actions to Kobo's stock
menu. [KOReader](https://koreader.rocks/) and
[Plato](https://github.com/baskerville/plato) are reading apps. Cobalt is a
platform for building and installing apps.

Cobalt handles the common parts: screens, app lifecycle, drawing to the e-ink
display, partial refreshes, touch input, device access, process isolation,
testing, and signed installs. App authors can focus on their app instead of
building those parts again.
See the [FAQ](https://bandarlabs.github.io/Cobalt/faq.html) for a fuller
comparison.

## Apps

Every screenshot below is a real capture from a Kobo Clara BW. Store manages
the installable applications; Settings and Terminal remain protected system
utilities.

<table>
<tr>
<td width="33%" valign="top"><a href="examples/launcher/README.md"><img width="230" src="examples/launcher/screenshots/home.png" alt="Cobalt launcher showing a grid of applications"></a><br><b><a href="examples/launcher/README.md">Launcher</a></b><br>Opens installed apps and always keeps a route back to the Kobo reader.</td>
<td width="33%" valign="top"><a href="docs/APP_STORE.md"><img width="230" src="examples/store/screenshots/catalog.png" alt="Cobalt App Store listing installed and available applications"></a><br><b><a href="docs/APP_STORE.md">App Store</a></b><br>Browses signed apps and installs, updates, removes, and reinstalls them over Wi-Fi.</td>
<td width="33%" valign="top"><a href="apps/sudoku/"><img width="230" src="apps/sudoku/screenshots/game.png" alt="A complete 81-cell Sudoku game on a Kobo Clara BW"></a><br><b><a href="apps/sudoku/">Sudoku</a></b><br>A Store-only touch game that also proves delivery of an app absent from the platform package.</td>
</tr>
<tr>
<td valign="top"><a href="examples/audiobook/README.md"><img width="230" src="examples/audiobook/screenshots/player.png" alt="An audiobook player with cover art, position and transport controls"></a><br><b><a href="examples/audiobook/README.md">Audiobook Studio</a></b><br>Researches, writes, narrates, and plays an original audiobook.</td>
<td valign="top"><a href="examples/gutenbird/README.md"><img width="230" src="examples/gutenbird/screenshots/shelf.png" alt="A shelf of book covers from an OPDS catalogue"></a><br><b><a href="examples/gutenbird/README.md">Gutenbird</a></b><br>Reads any OPDS library: Project Gutenberg, Standard Ebooks, Open Library, or one you add.</td>
<td valign="top"><a href="examples/hn/README.md"><img width="230" src="examples/hn/screenshots/stories.png" alt="A ranked list of Hacker News stories"></a><br><b><a href="examples/hn/README.md">Hacker News</a></b><br>Top, New, Ask, and Show stories with complete comment threads.</td>
</tr>
<tr>
<td valign="top"><a href="examples/rss/README.md"><img width="230" src="examples/rss/screenshots/articles.png" alt="A list of subscribed feeds and articles"></a><br><b><a href="examples/rss/README.md">Feeds</a></b><br>Discovers a site's feed and presents its articles without the browser layout.</td>
<td valign="top"><a href="examples/brief/README.md"><img width="230" src="examples/brief/screenshots/brief.png" alt="A numbered daily news brief"></a><br><b><a href="examples/brief/README.md">Daily Brief</a></b><br>Collects the day's stories while the reader is using another app.</td>
<td valign="top"><a href="examples/chat/README.md"><img width="230" src="examples/chat/screenshots/answer.png" alt="An AI answer displayed as readable text on the panel"></a><br><b><a href="examples/chat/README.md">AI Command Center</a></b><br>Asks a question and turns the answer into touch-friendly reading.</td>
</tr>
<tr>
<td valign="top"><a href="examples/sidekick/README.md"><img width="230" src="examples/sidekick/screenshots/question.png" alt="A coding agent request with tappable responses"></a><br><b><a href="examples/sidekick/README.md">Sidekick</a></b><br>Lets a reader approve or deny requests from coding agents.</td>
<td valign="top"><a href="examples/terminal/README.md"><img width="230" src="examples/terminal/screenshots/shell.png" alt="A shell and touch keyboard on the Kobo display"></a><br><b><a href="examples/terminal/README.md">Terminal</a></b><br>A panel-native shell with keys that send input immediately.</td>
<td valign="top"><a href="examples/gallery/README.md"><img width="230" src="examples/gallery/screenshots/text.png" alt="Cobalt typography and UI components"></a><br><b><a href="examples/gallery/README.md">Components</a></b><br>Shows the UI toolkit's controls, layouts, typography, and states.</td>
</tr>
<tr>
<td valign="top"><a href="examples/settings/README.md"><img width="230" src="examples/settings/screenshots/battery.png" alt="Battery status and hardware facts"></a><br><b><a href="examples/settings/README.md">Settings</a></b><br>Manages connectivity and hardware, and keeps platform updates separate from Store.</td>
<td valign="top"><a href="examples/todo/README.md"><img width="230" src="examples/todo/screenshots/list.png" alt="A persistent to-do list with completed items"></a><br><b><a href="examples/todo/README.md">Todo</a></b><br>A persistent list with touch entry and completed-item states.</td>
<td valign="top"><a href="examples/tictactoe/README.md"><img width="230" src="examples/tictactoe/screenshots/game.png" alt="A completed game of tic-tac-toe"></a><br><b><a href="examples/tictactoe/README.md">Tic-tac-toe</a></b><br>A two-player touch game using partial refreshes for individual cells.</td>
</tr>
<tr>
<td valign="top"><a href="apps/arxiv/README.md"><img width="230" src="apps/arxiv/screenshots/listing.png" alt="The newest machine learning preprints on a Kobo Clara BW, newest first"></a><br><b><a href="apps/arxiv/README.md">arXiv Reader</a></b><br>Browses and searches preprints, keeps them for offline reading, and sets their mathematics as type.</td>
<td valign="top"><a href="apps/morse/README.md"><img width="230" src="apps/morse/screenshots/sending.png" alt="The letter S filling the panel while the beacon sends it"></a><br><b><a href="apps/morse/README.md">Morse</a></b><br>Sends a typed message on the front light, a letter at a time, drawn across the panel as it goes.</td>
</tr>
<tr>
<td valign="top"><a href="examples/magnet/README.md"><img width="230" src="examples/magnet/screenshots/counting.png" alt="The Kobo hall sensor responding to a magnet"></a><br><b><a href="examples/magnet/README.md">Magnet</a></b><br>Locates the hall sensor behind the bezel and reports its changes.</td>
<td></td>
<td></td>
</tr>
</table>

### Suggest an app

What would you use on your Kobo? It could be a game, a reading tool, a
home-automation control, or a replacement for an Android or iOS app. Add it to
the [app request thread](https://github.com/BandarLabs/Cobalt/issues/41).

## Install

On macOS or Linux, install the stable release:

```sh
curl -fsSL https://bandarlabs.github.io/Cobalt/install.sh | sh
```

This canonical discovery URL is published by GitHub Pages from stable
`main:/docs`; it becomes available with the first stable promotion containing
the installer. It trusts GitHub Pages HTTPS for the bootstrap script itself.
After it starts, every downloaded executable and device package is covered by
the signed release manifest and SHA-256 checks. For pre-execution verification
of `install.sh`, use the recommended
[signed-bootstrap procedure](docs/INSTALL.md#high-assurance-signed-bootstrap).

The installer supports macOS Intel and Apple Silicon, and Linux x86_64 and
arm64. It installs the stable Kobo platform only. The platform Beta channel is
enabled exclusively in Cobalt Settings after first launch, or through the
developer/source workflow. Settings shows the installed version and current
channel, confirms channel changes explicitly, and verifies signed Beta
platform metadata. Returning to Stable needs no USB connection and preserves
installed apps, state, and secrets.

Update the installed host command independently:

```sh
kobo update
kobo update --channel beta   # explicit host CLI beta; never writes to a reader
```

The host CLI channel does not select the Kobo platform channel. Device Beta
updates remain an explicit choice in Cobalt Settings.

Rerun the same command to update. Restart the reader, wait one minute for
NickelMenu's failsafe, then open **Cobalt** from Kobo's menu. Future
applications are installed from **Store** over Wi-Fi.

If you already use NickelMenu, Cobalt is added to it; existing entries are
left alone.

See [docs/INSTALL.md](docs/INSTALL.md) for the complete walkthrough and
recovery, uninstall, and source-build instructions.

## App Store

Store reads a signed catalog from the Stable `app-catalog` GitHub release, or
the isolated `app-catalog-beta` release when the owner enables Beta updates.
Each package contains one ARM executable and a signed canonical manifest. The
runtime verifies the catalog, package, installed manifest, and binary before
launch.

Store is the only app-management surface. The applications bundled with the
first `0.2.0` platform install appear as installed, can be removed and
reinstalled in the same session, and can be updated in place without creating
a second launcher entry. Platform utilities such as Settings and Terminal are
shown as installed system apps and cannot be removed.

Open **Install links** in Store to link a phone or computer without an
account. The **Install** button on any
[Cobalt app page](https://bandarlabs.github.io/Cobalt/#apps) then sends an
encrypted request to that Kobo. If the reader is offline, reconnect it and
open Store within 72 hours to continue.

Apps are published automatically when an app PR is merged into `main`.
Publishing an app does **not** require changing the Cobalt version or creating
a platform release.

Sudoku remains Store-only and is intentionally absent from the USB platform
package, so installing it verifies delivery of an app that was not already on
the reader.

## Build an app

```sh
cargo install --path crates/kobo-cli
kobo new my-app
cd my-app
kobo dev
```

`kobo dev` runs the app in the Clara BW browser simulator. Start with the
[SDK documentation](https://bandarlabs.github.io/Cobalt/sdk.html); the
repository also keeps the [deep implementation guide](SDK.md).

### What the SDK provides

| Area | Application-facing support |
|---|---|
| App model | Ordinary Rust binaries with declarative screens, named actions, lifecycle callbacks, and runtime-managed Back navigation |
| E-ink UI | Measured text, rows, tiles, pictures, dialogs, keyboards, terminal views, pagination, and full or partial refresh planning |
| Network and credentials | Asynchronous HTTPS fetches and posts, ranged downloads, bounded responses, and named secrets whose values never enter the app |
| State and background work | Atomic per-app keyed storage, cancellable tasks, foreground/background lifecycle events, and scheduled wake capabilities |
| Device and media | Capability-gated battery, cover, frontlight, Wi-Fi, Bluetooth, and audio requests |
| Tooling | App scaffolding, the browser and native runtime simulators, layout diagnostics, deterministic failure scenarios, packaging, and device deployment |

Apps request services through the SDK instead of opening device resources
directly. The runtime can deny a request because it was not declared, is too
expensive for the current battery state, or is unsupported, and each refusal
is returned to the app as a value it can present or recover from.

See the SDK docs for the
[application model](https://bandarlabs.github.io/Cobalt/sdk.html#application-model),
[UI components](https://bandarlabs.github.io/Cobalt/sdk.html#ui),
[runtime services](https://bandarlabs.github.io/Cobalt/sdk.html#services),
[capabilities](https://bandarlabs.github.io/Cobalt/sdk.html#capabilities),
[developer-facing crates](https://bandarlabs.github.io/Cobalt/sdk.html#crates),
and the [CLI command reference](https://bandarlabs.github.io/Cobalt/sdk.html#cli).

## Contributing apps

App contributions are regular pull requests:

1. Add the app as a workspace package under `apps/<app-id>/`.
2. Add its release metadata to `apps/catalog.json`.
3. Add unit tests and layout checks for every affected supported profile.
4. Run the app in the browser and runtime simulators.
5. Run it on a physical, fully supported Kobo and attach a GIF, video, or
   photos to the pull request.
6. Include one clean panel screenshot for the app README and generated website
   install page.
7. Open a pull request.

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

- [Roadmap](ROADMAP.md)
- [Contributing](CONTRIBUTING.md)
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

Normal panel-write entry points require one of the exact hardware and firmware
combinations in the
[device support matrix](docs/DEVICES.md#device-support-matrix). Do not treat a
read-only profile match as permission to install: normal use requires the
profile's owner-attended display, touch, exit, and recovery evidence to be
complete.

## License

GNU Affero General Public License v3.0. See [LICENSE](LICENSE) and
[THIRD-PARTY.md](THIRD-PARTY.md).

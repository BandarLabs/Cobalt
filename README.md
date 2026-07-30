# Cobalt

**Real applications on a Kobo E Ink reader, without ever owning the boot.**

An SDK, a declarative UI layer, a runtime that takes the hardware for the
length of a session and always gives it back, a browser simulator, and a CLI.

This is an independent, unofficial project. It is not affiliated with,
endorsed by, or sponsored by Rakuten Kobo Inc. “Kobo” and related product
names are trademarks of their respective owners.

Applications are ordinary Rust binaries. They describe whole screens and
receive named actions. They never open the framebuffer, the touch device, a
network socket or a credential; everything else is a request the runtime may
refuse, and a refusal is a value rather than a crash.

**[SDK.md](SDK.md) is the developer guide.**

## Start here

Three questions cover almost everyone who arrives. Pick yours.

| You have | You want | Go to |
| --- | --- | --- |
| A Kobo Clara BW | To run Cobalt on it | [Install it on your Kobo](#install-it-on-your-kobo) |
| A Kobo Clara BW | To write your own application for it | [Build your first application](#build-your-first-application) |
| A different Kobo | To find out what it would take | [Porting to another Kobo](docs/PORTING.md) |

Nothing in the second path needs a device: the browser simulator runs the same
layout engine, typeface and refresh planner the panel does. You can write and
finish an application before you ever plug a reader in.

## Contents

**Using it**

- [Before you install](#before-you-install)
- [Install it on your Kobo](#install-it-on-your-kobo)
- [What is here, and what is not](#what-is-here-and-what-is-not)
- [What runs on the panel](#what-runs-on-the-panel)

**How it stays safe**

- [The governing rule](#the-governing-rule)
- [Why Cobalt itself is not a `KoboRoot.tgz`](docs/DEVICES.md#why-cobalt-itself-is-not-a-koboroottgz)
- [Credentials](#credentials)
- [Verified on the hardware](docs/PORTING.md#how-to-get-the-numbers)

**Building on it**

- [Build your first application](#build-your-first-application)
- [What the UI layer draws](#what-the-ui-layer-draws)
- [Development](docs/DEVELOPING.md)
- [Layout](#layout)

**Working against a real reader**

- [Porting to another Kobo](docs/PORTING.md)
- [Connecting and talking to a device](docs/DEVICES.md)
- [Keeping a reader awake while you work](docs/DEVICES.md#keeping-a-device-reachable-while-developing)
- [Attended display smoke tests](docs/DEVICES.md#attended-display-smoke-tests)

**The project**

- [Contributing](#contributing)
- [Licence](#licence)

## Before you install

**Tested on one device: the Kobo Clara BW (N365, device code 391), firmware
4.45.23697.** Nothing here has been run on any other model. Every device write
is gated on an exact match of framebuffer identity, geometry, device code,
serial model prefix, firmware version and kernel release, so a different reader
is refused rather than guessed at. That refusal is the safety mechanism, not a
limitation to work around. On any other Kobo, Cobalt will decline to draw.

**You run this at your own risk.** It is AGPL-3.0 licensed, which means it
comes with no warranty of any kind. The design rule is that nothing survives a
reboot, and it is followed carefully (see [The governing
rule](#the-governing-rule)), but nobody here can promise your reader will be
fine. If you brick a device, that is your device and your decision. Do not run
this on a reader you cannot afford to lose.

**Other devices: pull requests welcome.** Support for another Kobo means a new
profile with its own geometry, waveforms, touch transform and identity gate.
If you have a Libra, a Sage, a Clara 2E or anything else and you are willing to
test on it, that contribution would be genuinely valuable. Open an issue first
so the profile shape can be agreed before you write it.

## Install it on your Kobo

This is the whole of it: one command over USB, one restart, and a **Cobalt**
entry appears on the reader's own menu. No SSH, no IP address, no terminal on
the device, and nothing written outside the partition your books live on.

```sh
git clone https://github.com/BandarLabs/Cobalt
cd Cobalt
rustup target add armv7-unknown-linux-musleabihf
cargo run -p kobo-cli -- setup
```

You need a charged Kobo Clara BW, a USB cable that carries data, an internet
connection, [Rust](https://rustup.rs) and an ARM cross-compiler. Setup builds
before it writes anything, so a failed build leaves the reader untouched, and
`--dry-run` shows every step without touching anything. Restart the reader
once setup finishes, wait a minute, then find **Cobalt** in the menu at the
bottom right of the home screen.

Removing it is deleting a folder: `cargo run -p kobo-cli -- setup --undo`, or
plug the reader in and delete `.adds/cobalt` from it directly.

**The full walkthrough, what to do if a step doesn't go as described, and
deploying over Wi-Fi instead of USB are in
[docs/INSTALL.md](docs/INSTALL.md).**

## Build your first application

An application is an ordinary Rust binary. It describes whole screens and
receives named actions back. It never opens the framebuffer, the touch device,
a socket or a credential, so there is nothing you can do here that damages a
reader, and **you do not need a device for any of this**: the simulator runs
the same layout engine, typeface and refresh planner the panel uses.

```sh
cargo install --path crates/kobo-cli   # once, so `kobo` is on your PATH
kobo new my-app && cd my-app
kobo dev                               # opens the browser simulator
```

That writes a working application, the same file as `examples/hello`, which
the workspace compiles and tests so it is never stale: a screen, two buttons,
and a battery reading that shows how hardware is asked for and how every
answer, including a refusal, comes back.

**[SDK.md](SDK.md) walks the whole thing end to end**, from editing
`src/main.rs` through to a tile on the reader's own launcher, over Wi-Fi or
USB.

## Porting to another Kobo

**Cobalt has only ever run on a Kobo Clara BW (N365, device code 391).** Every
device write is gated on an exact hardware match, so a different reader is
refused rather than guessed at. What a new profile has to supply, how to
measure it, and what refuses to work until it is right are in
**[docs/PORTING.md](docs/PORTING.md)**.

## What is here, and what is not

Verified on the physical Clara BW unless stated otherwise.

**Working on the device**

| | |
|---|---|
| Runtime | Sessions with guaranteed teardown, screen snapshot and restore, watchdog, exclusive touch grab, idle and ceiling limits |
| Display | Full and partial refresh, GC16 and DU waveforms, measured layout at 1072×1448 |
| Input | Touch with the panel's own transform, verified against a physical tap |
| UI | Bars, tiles, picture tiles, grids, rows, checklists, keyboard, terminal, prose pagination, skeletons, banners, dialogs |
| Pictures | Chunked upload, an LRU cache, greyscale conversion, glyph fallback |
| Network | HTTPS `Fetch` and `Post`, ranged downloads, a 24 MB transfer, named credentials the application never sees |
| Audio | Bounded MP3/MP3Z decode, A2DP playback, shared album-art player, Bluetooth output handoff |
| Storage | Per-application keyed state under its own directory |
| Navigation | A runtime-owned Back the application may answer first (see below) |
| Tooling | `devices`, `doctor`, `package`, `deploy`, `inspect`, `verify`, `session`, `wait`, `logs`, `touch-probe`, `record`, and a Clara BW simulator in the browser |
| Applications | `launcher`, `audiobook`, `settings`, `hn`, `rss`, `gutenbird`, `chat`, `todo`, `terminal`, `tictactoe`, `magnet`, `gallery`, `brief` |

**Not here yet, stated plainly**

- **`schedule_wake` has no device backend.** The runtime does not own suspend
  or the RTC alarm, so a scheduled wake is refused rather than silently
  dropped. This costs the entire ambient genre, which is arguably E Ink's
  native one: `brief` is the shape of it and on a device it only collects while
  it is in the foreground. Making it real means `kobod` owning suspend on the
  only device there is.
- **One device.** Clara BW N365, device code 391. Everything else is refused
  rather than mapped to a similar model, so there is no second profile to test
  against and no evidence any of this holds elsewhere.
- **No install without SSH.** Deploying over Wi-Fi needs an SSH server the
  platform does not ship. The USB route (`kobo package`, copy to
  `.kobo/KoboRoot.tgz`) always works and needs nothing.
- **The simulator draws the chrome but not the reader's hands.** It attaches
  the same status band and back bar the runtime does, which is how the band
  overlapping the top bar was found. It still cannot exercise the grace period
  behind Back, or a finger arriving mid-refresh.
- **Nothing is signed or verified at rest.** `kobo deploy` checksums what it
  uploads end to end, but a package already on the drive is trusted.
- **No power budget.** Nothing measures or bounds what a session costs the
  battery beyond the idle and ceiling timers.

## The governing rule

**Nothing that cannot be undone by a reboot.**

The platform never owns boot, so a power cycle always lands in the stock
reader. Everything else follows from that:

- The screen is snapshotted before a session and restored on every exit path,
  including every error path.
- An exclusive touch grab belongs to an open file description, so the kernel
  drops it even on `SIGKILL`.
- Stopping the stock reader arms a detached watchdog first, which restarts it
  unconditionally after a deadline even if this process is killed outright.
- Nothing is written to the rootfs, the bootloader, the kernel, a partition,
  or any startup script.
- Every device write is gated on an exact match of framebuffer identity,
  geometry, device code, serial model prefix, firmware version and kernel
  release.

## Layout

```
crates/    kobo-sdk       what an application imports
           kobo-ui        layout, rendering, pagination, vector icons
           kobo-protocol  the bounded wire format between the two
           kobo-policy    capabilities, task runner, device services, storage
           kobo-net       HTTPS; carries TLS and nothing else does
           kobo-json      a small JSON reader and object builder
           kobo-image     JPEG and PNG decoding, scaling and halftoning
           kobo-text      typeface loading and measurement
           kobo-shell     one terminal per application, hosted by the runtime
           kobo-term      the vt100 screen a terminal's output is parsed into
           kobo-hal       display, touch, battery, reader handoff
           kobo-abi       the only unsafe in the workspace
           kobo-profile   exact hardware identity
           kobod          the runtime
           kobo-sim       the browser simulator, same renderer
           kobo-cli       scaffolding, simulation, building, diagnostics
           kobo-doctor    read-only device probe
           kobo-smoke     owner-attended display writes
           kobo-handoff   stopping and restarting the stock reader
           kobo-guard     screen capture and restore around a session
examples/  launcher, audiobook, settings, terminal, todo, brief, chat,
           gutenbird, gallery, tictactoe, hn, rss
```

External dependencies are kept behind narrow crates: `kobo-net` (HTTP and
TLS), `kobo-text` (glyph rasterisation), `kobo-term` (a vt100 parser),
`kobo-image` (JPEG and PNG decoding), `kobo-doc` (document parsing) and
`kobo-abi` (libc/kernel calls). Their interfaces keep applications independent
of the particular implementations.
Device binaries are statically linked ARMv7 and need nothing installed on the
device.

## Development

```sh
cargo test --workspace --all-features
cargo run -p kobo-cli -- dev --builtin      # browser simulator
cargo run -p kobo-cli -- run --sim          # the real runtime, host socket
```

The simulator runs the same renderer, layout engine, policy, typeface and
refresh planner as the panel, so an application can be written and finished
before a reader is ever plugged in. The rest, including the loop that drives an
application and photographs what it drew, is in
**[docs/DEVELOPING.md](docs/DEVELOPING.md)**.

## Working against a real reader

Connecting over Wi-Fi, the two ways to install, what the runtime does with the
radio and the three watchdogs, keeping a reader awake while you work, and the
attended display tests are in **[docs/DEVICES.md](docs/DEVICES.md)**.

## Verified on the hardware

The read-only doctor matches the physical N365:

- Device tree `mediatek,mt8110`, `mediatek,mt8512`
- Framebuffer `hwtcon`, 1072×1448, 32-bit RGBA, 4288-byte stride,
  6,243,328-byte map, rotation 3
- Touch `cyttsp5_mt` on `/dev/input/event1`, X 0–1447, Y 0–1071
- `identity: model=N365 firmware=4.45.23697 kernel=4.9.77 device-code=391`

The full serial number is deliberately never read past its four-character
model prefix.

Proven on the device, in order: a GC16 refresh that writes no pixel; a
reversible pixel write restored and verified byte for byte; a whole-screen
snapshot and restore; the DU waveform; the touch transform, against a physical
touch; guardian restoration after a failed child; stopping and restarting the
stock reader; an application rendered on the panel and taps reaching it; and
HTTPS, including a 24 MB download.

Update markers are random and at least `0x40000000`, because markers are a
global namespace shared with the stock reader and a low fixed marker could be
matched against another process's update.

## What the UI layer draws

A closed set of nodes, no free-form drawing, no colour, no font choice and no
pixel positioning: headings and paragraphs, buttons, rows and checklists, tile
grids with icons or pictures, a picture on its own, a tap-first `choose` with
an optional freeform row, threaded `quote` paragraphs for replies, banners,
progress and activity, skeletons, dividers and spacers, a paged list, an
on-screen keyboard, and a terminal grid.

Everything that varies is *state* rather than styling. A finished row is
finished, a chosen answer is chosen, a reply has a depth; the renderer decides
what each looks like. That is what makes a badly proportioned screen
unexpressible rather than merely discouraged, and it is also what stops an
application marking its own state with a character the installed face has no
glyph for. In debug builds `set_screen` refuses a screen carrying one, so an
application's own tests fail instead of the panel showing an empty box.

### Back belongs to the reader, and can be lent

The Back control in the top bar is drawn by the runtime, on top of whatever the
application asked for. It cannot be removed, cannot be forged (`ActionId::BACK`
is refused if an application tries to bind it) and always ends at the launcher.
That is what makes it the reliable way out of anything.

It used to end there *immediately*, which was wrong in a way only the device
showed: tapping Back inside a book left the whole application, and reopening it
came back to the book rather than the shelf, because its retained screen had
never changed. An application had no way to have any history at all.

A screen may now ask for first refusal with `owns_back(true)`. The runtime
delivers `ActionId::BACK` as an ordinary action instead of leaving, and starts a
two second clock. If a screen arrives, the application went back inside itself.
If none does, the launcher appears anyway. So an application can have history,
and still cannot trap a reader: the guarantee is a deadline rather than a
promise, which is the only kind an application cannot break.

Pictures are decoded by `kobo-image`, halftoned to the sixteen greys this panel
resolves, and scaled to the cell they will occupy, including *up*, bounded, so
a book cover published at 190 by 300 fills a tile on a 300 pixel-per-inch panel
instead of sitting in the middle of it like a stamp.

## Writing an application

[SDK.md](SDK.md) is the full guide. In short: an application is a plain Rust
struct. It owns its state, describes a screen,
and reacts to events. It never opens a device, chooses a refresh waveform,
writes a sysfs file, or talks to a radio.

```rust
use kobo_sdk::prelude::*;

#[derive(Default)]
struct Dashboard {
    battery: Option<String>,
}

impl KoboApp for Dashboard {
    fn on_start(&mut self, context: &mut Context) {
        context.device().read_battery();
        self.show(context);
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if action == action_id("refresh") {
            context.device().read_battery();
        }
    }

    fn on_device_result(
        &mut self,
        context: &mut Context,
        request: DeviceRequest,
        result: DeviceResult,
    ) {
        if let DeviceResult::Battery { percent, .. } = result {
            self.battery = Some(format!("{percent}%"));
            self.show(context);
        }
        let _ = request;
    }
}
```

`kobo new` generates exactly this shape, and `kobo dev` runs it in the browser
simulator against the same renderer and the same policy the device applies.

### The hardware API

`context.device()` is the whole hardware surface:

| Call | Meaning |
| --- | --- |
| `read_battery()` | Percentage and charging state |
| `hold_wifi(duration)` | Keep Wi-Fi associated, for an always-on view |
| `release_wifi()` | Give it back early |
| `keep_awake(duration)` | Stay out of suspend while in the foreground |
| `allow_sleep()` | Give that back early |
| `schedule_wake(delay)` | Be woken to refresh content |
| `cancel_wake()` | Drop a pending wake |
| `set_frontlight(percent)` / `read_frontlight()` | Front light |
| `read_bluetooth()` / `set_bluetooth(on)` | Bluetooth availability and power |
| `scan_bluetooth()` | Discover nearby devices |
| `pair_bluetooth(address)` / `connect_bluetooth(address)` | Pair and connect headphones, speakers, keyboards, remotes and other input devices |
| `disconnect_bluetooth(address)` / `forget_bluetooth(address)` | Disconnect or remove a pairing |
| `read_wifi()` / `set_wifi(on)` / `scan_wifi()` | Wi-Fi state, power and nearby networks |
| `join_wifi(ssid, password)` / `disconnect_wifi()` | Join a WPA personal or open network, or disconnect |
| `load_shelf_audio(name)` / `load_audio_stream(url)` | Prepare a bounded MP3 or Kobo MP3Z source |
| `play_audio()` / `pause_audio()` / `stop_audio()` | Control the runtime-owned audio transport |
| `seek_audio(position)` / `set_audio_volume(percent)` | Seek and adjust software playback volume |

Every call produces exactly one `on_device_result`, so an application always
learns what happened. A request can come back `Granted` for **less** time than
was asked for, or `Denied` with the exact reason: the capability was not
declared, it was withheld because the battery is low, system policy refused it,
another application holds it, or this runtime cannot do it on this hardware.

That last reason is the safety rule made visible. A build only performs what it
has a proven backend for; anything else is refused rather than pretended. The
device runtime uses the existing firmware `wpa_supplicant` for Wi-Fi and the
firmware's BlueZ-compatible D-Bus service for Bluetooth. It never starts a
second supplicant, attaches HCI itself, or unloads the shared radio modules.
Audio uses the firmware-owned AOSP A2DP HAL: the runtime decodes MP3, paces
44.1 kHz stereo PCM into `btservice`, and keeps file paths and HTTPS transport
inside the runtime. `kobo_sdk::audio::AudioPlayer` composes album art, position,
seek, play/pause, volume and an audio-only Bluetooth picker. If Play has no
connected output, the picker powers Bluetooth, scans, pairs and connects, then
continues playback automatically.
On Clara BW firmware 4.45.23697 the stable capability marker is
`/usr/lib/libaudio.a2dp.default.so`; `btservice` owns
`/tmp/audio.a2dp_ctrl` and creates `/tmp/audio.a2dp_data` after START. The
runtime therefore detects the HAL before a headset connects and opens the live
sockets only when playback begins.
On MediaTek Clara devices, using Bluetooth requests a clean reboot when leaving
Cobalt because restarting Nickel into an already-initialised driver can panic
the vendor Wi-Fi module.

### Refusing rather than inventing

`schedule_wake` is the largest refusal and is listed under
[what is not here](#what-is-here-and-what-is-not). The principle behind it
applies to every backend:

**An invented reading is worse than a refusal**, because an application cannot
tell one from the other and will act on it. The battery backend finds the supply
by reading each `type` file for `Battery` rather than hardcoding a device name,
and an unparseable capacity returns nothing rather than zero: rounding towards
"flat" is the dangerous direction.

## Credentials

An application never holds a key. It names one:

```rust
Task::Post { credential: Some(Credential::bearer("openrouter")), .. }
Task::Post { credential: Some(Credential::in_header("anthropic", "x-api-key")), .. }
```

The runtime reads `/mnt/onboard/.adds/cobalt/secrets/<name>` and attaches it,
either as a bearer token or under the header the service expects, so a request
goes straight to Anthropic or Gemini rather than through a proxy that would
have to be trusted with the key. The value never enters the application's
memory, its logs or its crash dump, and it is not replayed across a redirect.

Getting a key onto the reader is a command, not an errand:

```sh
kobo secret set openai --from ~/.openai --device 192.168.1.5
kobo secret set exa --from ~/.exa --device 192.168.1.5
kobo secret set elevenlabs --from ~/.elevenlabs --device 192.168.1.5
kobo secret list --device 192.168.1.5      # names only, never values
kobo secret remove openai --device 192.168.1.5
```

With no `--from`, the key is looked for in `$KOBO_SECRETS_DIR/<name>`,
`~/.config/cobalt/secrets/<name>` and `~/.<name>`, in that order. The value is
read on this machine and written straight to the reader: it is never passed as
an argument, so it does not reach a process table or a shell history, and it is
never printed. A one-line `NAME=value` file is accepted as well as a raw key;
only the value is installed. `--volume` does the same thing over USB for a
reader that is not yet on Wi-Fi.

A key must never reach a commit. `tools/pre-commit` refuses one, and is enabled
per clone with:

```sh
git config core.hooksPath tools
```

It scans staged lines for published credential shapes (OpenAI, Anthropic,
GitHub, AWS, Google, Slack), for a PEM private key header, and for a shell
assignment of something named like a key. It reports the shape it matched and
never the match, because printing the key to a terminal or a CI log is the
thing being prevented.

## What applications may ask for

Applications never touch hardware. They declare capabilities and the runtime
grants a clamped subset:

| Capability | Purpose |
| --- | --- |
| `network` | Reach the network in the foreground |
| `background-network` | Reach the network from a scheduled wake |
| `hold-wifi` | Keep Wi-Fi associated, for always-on dashboards |
| `keep-awake` | Stay out of suspend in the foreground |
| `scheduled-wake` | Be woken to refresh content |
| `battery-read` | Read battery percentage and charging state |
| `frontlight-control` | Change front light brightness |
| `audio`, `bluetooth-audio` | Play audio, including to headphones |
| `bluetooth-control` | Power, scan, pair and connect Bluetooth devices |
| `wifi-control` | Power, scan, join and disconnect Wi-Fi |
| `sleep-screen` | Draw the sleep screen |
| `notifications` | Post notifications |
| `shared-files` | Use a user-visible folder |
| `shell` | Run a terminal, hosted by the runtime |

Unknown names are rejected rather than ignored, dependencies are enforced
(`hold-wifi` requires `network`, `background-network` requires
`scheduled-wake`, `bluetooth-audio` requires `audio`), and a system
`PowerPolicy` the application cannot raise imposes a minimum wake interval, a
maximum Wi-Fi hold, and withdrawal of the expensive capabilities below fifteen
percent battery unless the device is charging.

`shell` is the one that is different in kind. Every other capability is undone
by a reboot; a shell on this device is root on a writable root filesystem, so
it is the first thing the platform hosts that a power cycle cannot repair. It
is never implied by another capability, it is granted today only to the
application named `terminal`, and the application never holds the
pseudo-terminal: it sends what was typed and receives what was printed, so the
runtime is the only thing that can start, bound, or stop a program.

## What runs on the panel

| Application | What it is for |
| --- | --- |
| `launcher` | The home screen, and an ordinary SDK application like the rest |
| `audiobook` | Research any topic with Exa, write it with OpenAI, narrate it with ElevenLabs, then show album art and play it over Bluetooth while also saving it to My Books |
| `settings` | Toggle and join Wi-Fi; scan, pair and connect Bluetooth devices |
| `terminal` | A shell, with keys that send a byte rather than collect a word |
| `todo` | State that survives a restart, and a row that can be struck through |
| `brief` | Background work: stories collected while the reader is elsewhere |
| `chat` | An answer that can be tapped rather than typed |
| `gutenbird` | Sixty thousand free books, downloaded and read on the panel |
| `gallery` | Every UI primitive at once, for checking by eye on real hardware |
| `tictactoe` | Two players, one panel, and partial repaints of single cells |
| `magnet` | The hall sensor behind the bezel, and where to find it |
| `hn` | Hacker News, with whole threads laid out by reply depth |
| `rss` | Any site's feed, found by typing its address, read without a browser |

Leaving an application does not end it. It is put behind the launcher rather
than stopped, so a download or a build that was running keeps running and
coming back is a repaint rather than a restart.

### Feeds and Feedsearch

`rss` finds feeds with [Feedsearch](https://feedsearch.dev), which takes a site
address and answers with the feeds it has: you type `arstechnica.com` rather
than hunting for a link with `rss` in it. Their terms ask for an attribution
visible to the reader on the search and results screens, so both carry one and
a test asserts it on both. That is not decoration: it has been lost once
already, silently, to a full page of results pushing it off the bottom of the
panel, which is why it now lives in the results screen's top bar where the
layout cannot discard it.

Feeds arrive as RSS 2.0, Atom or JSON Feed and are read into one shape. An
answer that stops at the fetch budget is reported as too large rather than as
not a feed: a cut XML feed keeps every item that arrived whole, but half a JSON
document is not a document at all and yields nothing, and sending somebody to
look for a different address does not help when the address was right.

## Contributing

The most valuable contribution is a second device. Everything here is measured
against one panel, and there is no evidence any of it holds elsewhere. Adding a
reader means a profile with its own geometry, waveform table, touch transform
and identity gate, and somebody willing to run it on hardware they own. Open an
issue before writing one so the profile shape can be agreed.

Beyond that:

- Every change is expected to keep `cargo test --workspace`,
  `cargo clippy --workspace --all-targets`,
  `cargo clippy -p kobod --features device-write --all-targets` and
  `cargo fmt --all --check` clean.
- `unsafe` is forbidden outside `kobo-abi`, which is where the kernel structs
  live.
- Anything that touches the device has to keep the governing rule: nothing a
  reboot cannot undo.
- A test that asserts intention rather than a measured result is not worth
  much here. Most of the defects in this project were found by rendering
  something and looking at it, against real captured data, and the tests that
  survived are the ones written that way.

## Licence

GNU Affero General Public License, version 3. See [LICENSE](LICENSE). What
ships inside the binary, and what its authors ask for in return, is in
[THIRD-PARTY.md](THIRD-PARTY.md).

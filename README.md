# Kobo

A Rust application platform for Kobo E Ink readers: an SDK, a declarative UI
layer, a runtime that owns the hardware, a browser simulator, and a CLI.

Applications are ordinary Rust binaries. They describe whole screens and
receive named actions. They never open the framebuffer, the touch device, a
network socket or a credential; everything else is a request the runtime may
refuse, and a refusal is a value rather than a crash.

**[SDK.md](SDK.md) is the developer guide.** [BUILD_IN_PUBLIC.md](BUILD_IN_PUBLIC.md)
is the running account of what was built, what broke, and what the device
taught us.

The hardware target is the Kobo Clara BW N365, device code 391. Unknown
hardware is rejected rather than mapped to a similar model.

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
| Storage | Per-application keyed state under its own directory |
| Navigation | A runtime-owned Back the application may answer first (see below) |
| Tooling | `devices`, `doctor`, `package`, `deploy`, `inspect`, `verify`, `session`, `wait`, `logs`, `touch-probe`, and a Clara BW simulator in the browser |
| Applications | `launcher`, `hn`, `gutenshelf`, `chat`, `todo`, `terminal`, `tictactoe`, `gallery`, `brief` |

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
- **The simulator does not draw the runtime chrome.** It lays screens out with
  no back bar, so Back and the grace period that backs it can only be exercised
  on hardware or in `kobod`'s tests.
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
examples/  launcher, terminal, todo, brief, chat, gutenshelf, gallery,
           tictactoe, hn
```

Outside dependencies are quarantined in exactly four crates, each behind one
interface: `kobo-net` (TLS), `kobo-text` (glyph rasterisation), `kobo-term`
(a vt100 parser) and `kobo-image` (JPEG and PNG decoding). Everything an
application touches is dependency free, and each of those four can be replaced
without a single application changing.
Device binaries are statically linked ARMv7 and need nothing installed on the
device.

## Development

```sh
cargo test --workspace --all-features
cargo run -p kobo-cli -- dev --builtin      # browser simulator
cargo run -p kobo-cli -- run --sim          # the real runtime, host socket
```

The simulator binds only to `127.0.0.1` and currently targets the measured Kobo
Clara BW 391 profile: 1072 × 1448 at 300 PPI, including its rotated raw touch
coordinates. It uses the same renderer, layout engine, policy, typeface and
panel refresh planner as the device, so a screen that fits in the browser fits
on the panel and the reported changed rectangle, waveform and clean-refresh
cadence cannot drift from the runtime. The inspector can compare ideal pixels
with a clearly labelled approximation of E Ink residue and outline the next
refresh region.

Network requests and terminals are real. The inspector's deterministic
scenarios exercise offline, low-battery, denied-permission, missing-secret,
timeout, full-storage and image-cache-pressure paths; it can also deliver
foreground and background lifecycle events. Its layout panel reports text,
touch-target and picture diagnostics with optional outlines over the exact
failing rectangles. Run with `KOBO_TEXT_SCALE=large` or
`KOBO_TEXT_SCALE=extra-large` to verify the 120% and 140% accessibility settings
with the same metrics used for pagination.

Create and run a new application:

```sh
cargo build -p kobo-cli
target/debug/kobo new weather
cd weather && ../target/debug/kobo dev
```

Build every device-side program:

```sh
rustup target add armv7-unknown-linux-musleabihf
cargo run -p kobo-cli -- build --device
```

That `rustup target add` is the entire cross-build setup. The linker is pinned
in `.cargo/config.toml` to `rust-lld`, which ships with the toolchain, so a
fresh checkout builds device binaries with no system packages.

## Connecting a device

The reader has to be on the same wireless network as the machine you work from.
Join it on the device the ordinary way — the top bar, the Wi-Fi icon, then the
network — and know that the radio goes down every time the reader sleeps.
Nothing on a stock Kobo keeps Wi-Fi up through a suspend, so this is not a
setting somebody forgot to turn on.

Two things follow from that, and between them they account for very nearly
every occasion this project has failed to reach a device.

The first is that the address changes. It comes from DHCP on every
reconnection, so the one that worked this morning is somebody's laptop by the
afternoon, and the reader never mentions it. `kobo devices` is the answer:

```sh
kobo devices                       # this machine's own /24
kobo devices --subnet 192.168.1    # when this machine has more than one route
```

It completes a TCP handshake on port 22 across the subnet, opens a shell on
whatever answered, and reads four files. Everything it does is read-only.

```
192.168.1.15  N365 · firmware 4.45.23697 · Cobalt 0.1.0
2 other host(s) answered on port 22
```

Hosts that turn out not to be readers are counted rather than listed. A tool
asked where an e-reader went should not reply with an inventory of the network
it was asked on.

The second is that a device stops answering a few minutes after anyone stops
touching it. Two reversible settings hold it open while you work:

```sh
kobo session --device <address> --wifi-always-on on
kobo session --device <address> --keep-awake on
```

`--wifi-always-on` writes the reader's own developer setting, which is read at
startup, so it applies from the next reader restart. `--keep-awake` takes a
kernel wake lock that lives in RAM. Both clear on a reboot, and neither is
sufficient on its own on this firmware — *Keeping a device reachable while
developing*, below, explains what actually stops the suspend and how it was
measured.

### The two ways to install

Over USB, which needs no SSH and is what an owner does:

```sh
cargo run -p kobo-cli -- package     # target/KoboRoot.tgz
```

Charge the device, copy the file to `.kobo/KoboRoot.tgz`, eject, and the reader
installs it at the next boot with its own installer. Charging first is not
politeness: that installer is gated on battery level and fails silently, so an
install that appears to do nothing usually means a flat battery. This path is
described in full under *Installing on a device*.

Over Wi-Fi, which needs SSH already working on the device and installs with no
reboot at all:

```sh
kobo deploy --device <address>
kobo deploy --device <address> --package target/KoboRoot.tgz
```

There is no reboot because there is nothing to reboot for. `/mnt/onboard` is
mounted without `noexec`, so an install is a folder of files arriving on the
book partition, and the vendor installer — the part that needs a reboot and a
charged battery — is not involved. `deploy` builds the same archive `package`
builds, sends it through the stdin-only shell channel as base64, and the device
compares the SHA-256 of what arrived against the SHA-256 of what was sent
before it extracts anything.

It refuses more than it does. An archive containing any path outside
`.adds/cobalt` is refused here before it is sent and again on the device from
the bytes that actually arrived, because that half runs as root. A package
given with `--package` is read back and checked exactly as `kobo inspect` reads
it, so an archive nobody has looked inside is never uploaded. And a running
Cobalt session is refused rather than worked around, since the files being
replaced are the ones it is executing.

Neither path starts anything. Run `.adds/cobalt/start.sh` on the reader, or add
the single NickelMenu line the packaged `README.txt` gives you.

### When it will not answer

Every command that fails to reach a device prints the same four causes, in the
order they actually happen: the reader is asleep and its radio is off; Wi-Fi is
off while it is awake; its address has changed; or nothing is listening on port
22. The first is more common than the other three together, and the fix is the
power button.

The last one is worth stating plainly. **Cobalt does not install an SSH server
and does not need one to run.** SSH is only how a developer's machine reaches
the device; nothing the platform does on the reader involves it. Somebody who
has never set one up installs over USB and never encounters any of this.

## Talking to a device

```sh
kobo doctor  --device <address>              # read-only identity probe
kobo session --device <address> --status     # power and network state
kobo logs    --device <address>              # follow the runtime trace
kobo logs    --device <address> --dump -t 50 # the last 50 lines, then exit
kobo logs    --device <address> --clear      # empty it before a test run
```

`kobo logs` reads `/mnt/onboard/.kobo-blackbox.log`, which is where the runtime
writes every tap, every screen and every task result. It is the only view into
what a session is actually doing. The runtime writes it only when started with
`KOBO_BLACKBOX=1`, because a synchronous write per event is not something to
impose on a session nobody is debugging; `kobo logs` says so rather than
showing an empty file when the trace is not there.

### Wi-Fi across a session

Stopping and restarting the stock reader reliably drops the Wi-Fi connection.
The reader owns the radio and drives it inside `libnickel`, and the restarted
one begins from its own "not connected" state; there is no D-Bus service, no
script and no supported way to ask it to reconnect. So every session costs the
connection, and the reader picks it up again by itself.

The runtime can put the link back by restarting the supplicant and DHCP client
it recorded, but it does **not** do so by default, and that is deliberate. Those
daemons attach to `wlan0`, the restarted reader drives the same radio and cannot
be told what we started behind it, and two owners of one radio leaves the
reader's own network panel unable to scan at all — not merely disconnected, but
unable to see a network it has known for months. A reboot clears it, as a reboot
clears everything here, but that is a poor thing to owe someone who only opened
an application.

Restoring it is therefore a convenience for one case only: working on a device
over Wi-Fi, where losing the link loses the session driving it. Ask for it
explicitly:

```sh
KOBO_KEEP_NETWORK=1 KOBO_PRESENT_UNLOCK=OWNER_ATTENDED_PANEL_SESSION kobod --present ...
```

`start.sh` in the installed package does not set it, so an owner launching from
NickelMenu never gets it. If a session does leave the radio confused, a reboot
always returns the stock reader with its network intact.

### If you have shipped for Android or iOS

The concepts are the same and only the spelling differs, so the spellings you
already know work:

| You may type | It runs |
| --- | --- |
| `kobo logcat` | `kobo logs` |
| `kobo install` | `kobo deploy` |
| `kobo wait-for-device` | `kobo wait` |
| `kobo sim`, `kobo simulator` | `kobo dev` |
| `kobo init`, `kobo create` | `kobo new` |

`kobo logs` takes `adb logcat`'s flags too — `-f` follow, `-d` dump, `-t N`
lines, `-c` clear — and every command that takes `--device` also takes `-s`.
These are aliases onto one implementation rather than second commands, so there
is nothing extra to keep in step.

`scp` cannot be used with this device: its SSH server ignores remote arguments,
so the `scp -t` helper never runs and the transfer hangs. Files go through the
stdin-only shell channel as base64, verified by comparing SHA-256 on both ends.

Every binary sent to a device is rebuilt first from this workspace's pinned
manifest with `--locked`, and the checksum the device verifies is taken over
exactly the bytes that were uploaded, so a stale or foreign artifact cannot be
run by accident.

## Installing on a device

```sh
cargo run -p kobo-cli -- package                 # target/KoboRoot.tgz
cargo run -p kobo-cli -- inspect target/KoboRoot.tgz
```

Charge the device, copy the file to `.kobo/KoboRoot.tgz` over USB, and eject.
The reader installs it at the next boot with its own installer, which writes
the boot environment to recovery first and puts it back afterwards, so an
interrupted install lands somewhere designed for it. No terminal, no SSH, no
IP address.

Everything lands in `.adds/cobalt` on the same partition as the books. That
partition is vfat mounted without `noexec`, so the binaries run from where they
land, which is why no rootfs file and no boot script is needed. Uninstall is
deleting the folder, over USB, from any computer.

The archive is incapable of writing anywhere else. Members are checked before
they are written and then read back out of the finished bytes, so `kobo inspect`
reports what the package can do rather than what it was asked to do; absolute
paths, `..`, symbolic links, device nodes and anything outside the install root
are refused. Output is byte-for-byte reproducible — `gzip -n -9`, mtime 0,
uid/gid 0 — so the printed SHA-256 is worth comparing.

Two things are worth knowing before blaming the package. The reader's installer
is gated on battery level and fails silently, so an install that appears to do
nothing usually means charge it first. And nothing yet starts Cobalt at boot:
run `.adds/cobalt/start.sh`, or add the single NickelMenu line the packaged
`README.txt` gives you. Boot takeover is permanently out of scope, so a reboot
always returns to the stock reader.

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

## Attended display smoke tests

`kobo smoke-display` is not compiled into a default build; the CLI must be
rebuilt with `--features device-write`. Before it changes anything it requires,
in order: the exact confirmation phrase on the command line, the exact unlock
phrase in the device process environment, an exact match of every probed
hardware value against the profile, and an exact match of the device code,
serial model prefix, firmware version and kernel release.

```
kobo smoke-display --device <address> --confirm DISPLAY_ONLY_GC16
kobo smoke-display --device <address> --confirm REVERSIBLE_PIXELS_GC16
kobo smoke-display --device <address> --confirm SCREEN_SNAPSHOT_RESTORE
kobo smoke-display --device <address> --confirm REVERSIBLE_PIXELS_DU
```

`SCREEN_SNAPSHOT_RESTORE` proves the guarantee everything else rests on:
whatever the runtime draws, the reader's own screen can always be put back
exactly. Even a whole-screen update is submitted in partial mode, because full
mode is an untested code path on this controller.

## Keeping a device reachable while developing

A device drops off Wi-Fi within a few minutes of inactivity, which makes
unattended testing impractical. `kobo session` exposes the two reversible
mechanisms that fix this:

```
kobo session --device <address> --status
kobo session --device <address> --keep-awake on
kobo session --device <address> --wifi-always-on on
kobo session --device <address> --restore-reader-config
```

`--keep-awake` holds a named kernel wake lock. It lives in RAM only and always
clears on reboot, so it cannot leave a device permanently unable to sleep.

`--wifi-always-on` sets the reader's own `ForceWifiOn` developer setting. A
pristine backup is taken before the first change, the file is rewritten through
a temporary file in the same directory, and the change is rejected unless it
changes only the intended line and produces exactly the intended value.
`--restore-reader-config` puts the original file back.

A settings file is only advice: the reader silently ignores keys it does not
implement, so writing one would look like a success and do nothing. Enabling is
therefore refused unless the running firmware is shown to contain the setting.
Removing it never consults that check, so recovery works on any firmware.

The reader reads its settings file only at startup, so `ForceWifiOn` takes
effect after the next reader restart or a normal reboot, not immediately.

### Why a device stops answering

An earlier version of this document blamed the reader's Wi-Fi inactivity timer,
on the evidence that `/proc/uptime` kept increasing while `wlan0` came and went.
That reasoning was wrong, and the mistake is worth recording: `/proc/uptime` is
taken from a clock that keeps counting while the system is suspended, so it can
never show that a device suspended.

Kernel log timestamps do not count suspended time, so comparing the two is what
actually settles it. On this device the newest kernel timestamp was 342 seconds
while `/proc/uptime` read 760 seconds: 418 of those seconds were spent
suspended. The kernel log says so directly:

```
PM: suspend entry 14:25:07 ... PM: suspend exit 14:26:08
PM: suspend entry 14:26:12 ... PM: suspend exit 14:31:57
```

So the device suspends after a few minutes of inactivity, and that is what takes
Wi-Fi down. `--status` now reports the evidence rather than the guess:

```
suspend_events: 12                 suspends since boot
uptime_seconds: 760                counts suspended time
kernel_awake_seconds: 342          does not
```

A large gap between the last two means the device has spent most of its time
asleep.

### What actually stops the suspend

The suspend is requested by the reader process itself:

```
[  338.010942] .(0)[360:nickel]PM: suspend entry
                    ^^^^^^^^^^ the reader, not the kernel
```

That matters, because a kernel wake lock only blocks the kernel's own autosleep.
It cannot block a userspace process writing to `/sys/power/state`. Measured on
the device, a continuously held wake lock did not prevent a single suspend.

The lever is the reader's own sleep delay, `AutoSleepMinutes` in `[PowerOptions]`:

```
kobo session --device <address> --sleep-after 90
kobo session --device <address> --sleep-after default
```

`default` removes the key so the reader returns to its own behaviour. The value
is bounded, because a device that never sleeps flattens its battery. Like every
settings change this takes effect at the next reader start.

Verified on the device. Before, over 9839 seconds of uptime the device had been
awake for 682 of them and stopped answering every three minutes. After setting
the delay and restarting the reader:

```
suspend_events: 0
uptime_seconds: 2307
kernel_awake_seconds: 2305
```

Thirty eight minutes of continuous reachability with nobody touching it, and no
suspends at all.

`kobo session --hold [minutes]` still exists and renews the wake lock, but it is
not sufficient on its own on this firmware and is documented as such.

### One audited path for every settings change

Settings are described rather than hand-written, so there is a single reviewed
rewrite path instead of one per setting:

```rust
Setting { section: "PowerOptions", key: "AutoSleepMinutes", value: 90 }
```

Each change is refused unless the running firmware contains the key, takes a
pristine backup before the first write, goes through a temporary file in the same
directory, must produce exactly the intended value, and must change no more lines
than that specific edit can account for. Removal never consults the firmware
check, so recovery works on any firmware.

The change bound is counted with `diff -U 0`, and the script refuses outright if
the files differ but no change can be counted. That guard exists because of a
real bug: the original code counted lines matching `^[<>]`, which is the classic
diff format. BusyBox `diff` on the device writes unified output, so the count was
always zero and the bound silently never applied. Host tests passed throughout,
because the host's `diff` does emit the classic format. The tests now assert the
reported count against an independently computed difference, and reintroducing
the old counter makes nine of them fail.

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
glyph for — in debug builds `set_screen` refuses a screen carrying one, so an
application's own tests fail instead of the panel showing an empty box.

### Back belongs to the reader, and can be lent

The Back control in the top bar is drawn by the runtime, on top of whatever the
application asked for. It cannot be removed, cannot be forged — `ActionId::BACK`
is refused if an application tries to bind it — and always ends at the launcher.
That is what makes it the reliable way out of anything.

It used to end there *immediately*, which was wrong in a way only the device
showed: tapping Back inside a book left the whole application, and reopening it
came back to the book rather than the shelf, because its retained screen had
never changed. An application had no way to have any history at all.

A screen may now ask for first refusal with `owns_back(true)`. The runtime
delivers `ActionId::BACK` as an ordinary action instead of leaving, and starts a
two second clock. If a screen arrives, the application went back inside itself.
If none does, the launcher appears anyway. So an application can have history,
and still cannot trap a reader — the guarantee is a deadline rather than a
promise, which is the only kind an application cannot break.

Pictures are decoded by `kobo-image`, halftoned to the sixteen greys this panel
resolves, and scaled to the cell they will occupy — including *up*, bounded, so
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

Every call produces exactly one `on_device_result`, so an application always
learns what happened. A request can come back `Granted` for **less** time than
was asked for, or `Denied` with the exact reason: the capability was not
declared, it was withheld because the battery is low, system policy refused it,
another application holds it, or this runtime cannot do it on this hardware.

That last reason is the safety rule made visible. A build only performs what it
has a proven backend for; anything else is refused rather than pretended. On a
real device today that means the battery gauge, which is read-only, and nothing
else: every hardware-changing request is honestly refused. The simulator
implements all of them, so application logic can be written and tested now and
will behave identically when a backend is turned on.

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
| `terminal` | A shell, with keys that send a byte rather than collect a word |
| `todo` | State that survives a restart, and a row that can be struck through |
| `brief` | Background work: stories collected while the reader is elsewhere |
| `chat` | An answer that can be tapped rather than typed |
| `gutenshelf` | Sixty thousand free books, downloaded and read on the panel |
| `gallery` | Every UI primitive at once, for checking by eye on real hardware |
| `tictactoe` | Two players, one panel, and partial repaints of single cells |
| `hn` | Hacker News, with whole threads laid out by reply depth |

Leaving an application does not end it. It is put behind the launcher rather
than stopped, so a download or a build that was running keeps running and
coming back is a repaint rather than a restart.

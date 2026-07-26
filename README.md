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
           tictactoe
```

Outside dependencies are quarantined in exactly three crates, each behind one
interface: `kobo-net` (TLS), `kobo-text` (glyph rasterisation) and `kobo-term`
(a vt100 parser). Everything an application touches is dependency free, and
each of those three can be replaced without a single application changing.
Device binaries are statically linked ARMv7 and need nothing installed on the
device.

## Development

```sh
cargo test --workspace --all-features
cargo run -p kobo-cli -- dev --builtin      # browser simulator
cargo run -p kobo-cli -- run --sim          # the real runtime, host socket
```

The simulator binds only to `127.0.0.1` and uses the same renderer, the same
layout engine and the same policy as the device, so a screen that fits in the
browser fits on the panel.

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

## Talking to a device

```sh
kobo doctor  --device <address>              # read-only identity probe
kobo session --device <address> --status     # power and network state
```

`scp` cannot be used with this device: its SSH server ignores remote arguments,
so the `scp -t` helper never runs and the transfer hangs. Files go through the
stdin-only shell channel as base64, verified by comparing SHA-256 on both ends.

Every binary sent to a device is rebuilt first from this workspace's pinned
manifest with `--locked`, and the checksum the device verifies is taken over
exactly the bytes that were uploaded, so a stale or foreign artifact cannot be
run by accident.

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

**An invented reading is worse than a refusal**, because an application cannot
tell one from the other and will act on it. The battery backend finds the supply
by reading each `type` file for `Battery` rather than hardcoding a device name,
and an unparseable capacity returns nothing rather than zero: rounding towards
"flat" is the dangerous direction.

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

Leaving an application does not end it. It is put behind the launcher rather
than stopped, so a download or a build that was running keeps running and
coming back is a repaint rather than a restart.

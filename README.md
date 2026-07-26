# Kobo

A dependency-free, add-on-first application SDK for Kobo E Ink readers.

The initial hardware target is the Kobo Clara BW N365, device code 391. The
workspace currently provides:

- A retained grayscale UI and shared browser simulator
- A Rust application SDK and bounded local protocol
- Strict Clara BW hardware profiles
- A read-only ARM device doctor
- Conformant HWTCON ABI fixtures and a slot-aware touch decoder
- A capability-gated hardware API that applications reach through the runtime
- Host CLI commands for scaffolding, simulation, building, and diagnostics

## Safety

The doctor opens hardware read-only and uses query ioctls only. An explicitly
feature-gated, owner-attended `smoke-display` command is the sole display write
path: it submits one fixed 32×32 GC16 partial refresh of unchanged framebuffer
contents, after an exact profile probe and two fixed confirmations. It is not
safe for unattended use. `kobo-guard` is disabled and excluded from device
builds. No component modifies partitions, the bootloader, kernel, firmware,
startup scripts, rootfs, power controls, or network controls.

## Development

```sh
cargo test --workspace
cargo run -p kobo-cli -- dev --builtin
```

The simulator binds only to `127.0.0.1` and uses the same grayscale renderer as
device builds. The host runtime path exercises the SDK, bounded Unix protocol,
daemon, application lifecycle, and renderer:

```sh
cargo run -p kobo-cli -- run --sim
```

Create and run a live SDK application:

```sh
cargo build -p kobo-cli
target/debug/kobo new weather
cd weather
../target/debug/kobo dev
```

The CLI builds the application, starts a private Unix-socket session, and prints
the loopback browser URL. Browser touches are returned to the Rust event loop.

Build all device-side programs as static ARMv7 hard-float binaries:

```sh
rustup target add armv7-unknown-linux-musleabihf
cargo run -p kobo-cli -- build --device
```

`kobo doctor --device <address>` is the read-only remote diagnostic command. It
streams the fixed workspace doctor into a unique mode-0700 directory in `/tmp`,
compares SHA-256 checksums, performs read-only device queries, and removes the
binary and directory. Every remote operation has a timeout. Interactive device
execution is intentionally rejected until physical recovery and guarded Nickel
handoff tests have passed.

For an owner physically attending the verified Clara BW only, the separate
display-only smoke path builds its fixed feature-gated artifact, verifies it,
and uses the same stdin-only SSH transport:

```sh
kobo smoke-display --device <address> --confirm DISPLAY_ONLY_GC16
```

It does not write framebuffer pixels and runs through the verified BusyBox
`/usr/bin/timeout 15` lease. Do not use it unattended.

The project intentionally has no crates.io dependencies in its MVP workspace.

## Verified Clara BW profile

The read-only doctor has matched the physical N365 device:

- Device tree: `mediatek,mt8110`, `mediatek,mt8512`
- Framebuffer: `hwtcon`, 1072×1448, 32-bit RGBA, 4288-byte stride,
  6,243,328-byte map, rotation 3
- Touch: `cyttsp5_mt` on `/dev/input/event1`, X 0–1447, Y 0–1071

The profile deliberately remains `read-only matched`, so the default build still
has no callable write path.

## Attended display smoke test

`kobo smoke-display` is the only command that writes to hardware. It is not
compiled into a default build at all; the CLI must be rebuilt with
`--features device-write`. Before it changes anything it requires, in order:

1. the exact confirmation phrase on the command line,
2. the exact unlock phrase in the device process environment,
3. an exact match of every probed hardware value against the profile, and
4. an exact match of the device code, serial model prefix, firmware version,
   and kernel release.

Three stages exist:

```
kobo smoke-display --device <address> --confirm DISPLAY_ONLY_GC16
kobo smoke-display --device <address> --confirm REVERSIBLE_PIXELS_GC16
kobo smoke-display --device <address> --confirm SCREEN_SNAPSHOT_RESTORE
```

`DISPLAY_ONLY_GC16` asks the controller to re-render a fixed 32×32 region
without writing a single pixel byte. `REVERSIBLE_PIXELS_GC16` captures that
region, inverts it, shows it briefly, restores the exact original bytes, and
then verifies the restoration byte for byte.

`SCREEN_SNAPSHOT_RESTORE` proves the guarantee everything else rests on: it
snapshots the entire visible framebuffer, changes a 256×256 region, and then
puts the whole screen back from that snapshot and verifies the change is gone.
Whatever a future runtime draws, the reader's own screen can always be restored
exactly. The snapshot is taken first and rewritten on every path, including
every error path. Even a whole-screen update is submitted in partial mode,
because full mode is an untested code path on this controller.

The first two stages have been run on the physical Clara BW and both passed:

```
display-only GC16 refresh completed; no pixel byte was written
reversible GC16 pixel test completed; 4096 bytes restored and verified
```

`SCREEN_SNAPSHOT_RESTORE` is implemented and unit tested but has not yet been
run on hardware.

After each run Nickel was still alive, the device had not rebooted, `dmesg`
contained no controller errors, and no temporary files were left behind. Both
`HWTCON_SEND_UPDATE` (`0x4024462e`) and `HWTCON_WAIT_FOR_UPDATE_COMPLETE`
(`0xc008462f`) are therefore confirmed correct against the real kernel, and a
framebuffer region can be captured, changed, restored, and proven identical.

Every binary that is sent to a device is rebuilt first from this workspace's own
pinned manifest with `--locked`, and the checksum the device verifies is taken
over exactly the bytes that were uploaded. A stale or foreign artifact cannot be
run on a device by accident.

Reading is bounded the same way. `kobo doctor` reports the identity it gates on:

```
identity: model=N365 firmware=4.45.23697 kernel=4.9.77 device-code=391
```

The full serial number is deliberately never read past its four-character model
prefix.

Update markers are random and at least `0x40000000`, because markers are a
global namespace shared with the stock reader and a low fixed marker could be
matched against another process's update.

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

## Writing an application## Writing an application

An application is a plain Rust struct. It owns its state, describes a screen,
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
real device today that means every hardware-changing request is refused, because
no backend is enabled yet. The simulator implements all of them, so application
logic can be written and tested now and will behave identically when a backend
is turned on.

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

Unknown names are rejected rather than ignored, dependencies are enforced
(`hold-wifi` requires `network`, `background-network` requires
`scheduled-wake`, `bluetooth-audio` requires `audio`), and a system
`PowerPolicy` the application cannot raise imposes a minimum wake interval, a
maximum Wi-Fi hold, and withdrawal of the expensive capabilities below fifteen
percent battery unless the device is charging.

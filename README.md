# Kobo

A Rust application platform for Kobo E Ink readers: an SDK, a declarative UI
layer, a runtime that owns the hardware, a browser simulator, and a CLI.

This is an independent, unofficial project. It is not affiliated with,
endorsed by, or sponsored by Rakuten Kobo Inc. “Kobo” and related product
names are trademarks of their respective owners.

Applications are ordinary Rust binaries. They describe whole screens and
receive named actions. They never open the framebuffer, the touch device, a
network socket or a credential; everything else is a request the runtime may
refuse, and a refusal is a value rather than a crash.

**[SDK.md](SDK.md) is the developer guide.** [BUILD_IN_PUBLIC.md](BUILD_IN_PUBLIC.md)
is the running account of what was built, what broke, and what the device
taught us.

## Read this before you run it on a reader

**Tested on one device: the Kobo Clara BW (N365, device code 391).** Nothing
here has been run on any other model. Every device write is gated on an exact
match of framebuffer identity, geometry, device code, serial model prefix,
firmware version and kernel release, so a different reader is refused rather
than guessed at. That refusal is the safety mechanism, not a limitation to
work around.

**You run this at your own risk.** It is MIT licensed, which means it comes
with no warranty of any kind. The design rule is that nothing survives a
reboot, and it is followed carefully (see below), but nobody here can promise
your reader will be fine. If you brick a device, that is your device and your
decision. Do not run this on a reader you cannot afford to lose.

**Other devices: pull requests welcome.** Support for another Kobo means a new
profile with its own geometry, waveforms, touch transform and identity gate.
If you have a Libra, a Sage, a Clara 2E or anything else and you are willing to
test on it, that contribution would be genuinely valuable. Open an issue first
so the profile shape can be agreed before you write it.

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
| Tooling | `devices`, `doctor`, `package`, `deploy`, `inspect`, `verify`, `session`, `wait`, `logs`, `touch-probe`, and a Clara BW simulator in the browser |
| Applications | `launcher`, `audiobook`, `settings`, `hn`, `rss`, `gutenbird`, `chat`, `todo`, `terminal`, `tictactoe`, `gallery`, `brief` |

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
cargo run -p kobo-cli -- run --sim --app rss   # ... pointed at one application
```

`run --sim` starts the real `kobod`, runs one application against it over a
host socket and saves what it drew to `target/kobo-sim-last.raw`: 1072 × 1448
bytes of eight-bit grey, one per pixel, which `P5` PGM will open directly.
`--app` takes any shipped application by the name the launcher uses or the name
cargo uses. It is the shortest way to see what a screen really looks like
without a reader in front of you.

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

### Driving it, and photographing the result

A layout assertion proves a button was placed. It does not prove the screen
reads as a product, and it does not prove the button is reachable. Closing that
loop, for a person or for something automating on their behalf, means
driving the application the way a finger does and then looking at the result.

```sh
cargo run -p kobo-cli -- dev 127.0.0.1:8787          # in one terminal
cargo run -p kobo-cli -- drive --script tour.kobo --shots target/shots
cargo run -p kobo-cli -- drive --step "tap Search" --step "expect Results"
```

A script is one step per line: `tap LABEL`, `tap-at X,Y`, `type TEXT`,
`expect TEXT`, `expect-missing TEXT`, `wait-for TEXT`, `clean`, `shot NAME`,
`dump`, `scenario NAME`, `lifecycle background`, `wait MS`. A failing step
reports the line and the reason and screenshots the panel first. Add `--ideal`
to take screenshots without the panel's e-ink residue, which is what you want
when a person or a model is going to read them.

`tap` resolves the label against the layout the renderer produced and then taps
the coordinate, through the panel's own touch transform and the renderer's own
hit-testing. Dispatching the action directly would have been simpler and
worthless: it passes on a screen whose only button has been laid out below the
bottom edge, which is the fault worth catching.

For the real panel:

```sh
cargo run -p kobo-cli -- shot --device <address> --out screen.png
cargo run -p kobo-cli --features device-write -- tap --device <address> 536,900
```

`shot --device` is read-only. It opens the framebuffer for reading and never
grabs, refreshes or writes, so it is safe against a device with the stock
reader in the foreground. `tap --device` writes real evdev records to the real
touch node, so the digitiser, the transform, the multitouch decoder and the
hit-testing all run as they do under a finger; it is behind `device-write` and
an unlock phrase, and it always lifts.

Create and run a new application:

```sh
cargo build -p kobo-cli
target/debug/kobo new weather
cd weather && ../target/debug/kobo dev
```

Build every device-side program:

```sh
rustup target add armv7-unknown-linux-musleabihf
# Also install an ARM hard-float compiler (Debian: gcc-arm-linux-gnueabihf).
CC_armv7_unknown_linux_musleabihf=arm-linux-gnueabihf-gcc \
  cargo run -p kobo-cli -- build --device
```

Rust code is linked by `rust-lld`, which ships with the toolchain. Rustls uses
the maintained `ring` cryptography provider, whose small C/assembly core also
needs an ARM hard-float compiler at build time. The resulting binaries remain
statically linked and need no library installed on the reader.

## Connecting a device

The reader has to be on the same wireless network as the machine you work from.
Join it on the device the ordinary way (the top bar, the Wi-Fi icon, then the
network) and know that the radio goes down every time the reader sleeps.
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
sufficient on its own on this firmware. *Keeping a device reachable while
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
book partition, and the vendor installer, the part that needs a reboot and a
charged battery, is not involved. `deploy` builds the same archive `package`
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

**A stock device cannot launch either of those, and this is the prerequisite
rather than a footnote.** Running `start.sh` needs a shell, and the NickelMenu
line needs NickelMenu; Cobalt deliberately installs neither, because writing to
the root filesystem is the one thing the packaging promises never to do.

`kobo setup` is the answer, and it needs nothing on the device beforehand:

```
kobo setup            # with the reader connected by USB and showing 'Connected'
```

It finds the mounted reader, copies Cobalt into `.adds/cobalt`, reads every
file back to prove it arrived intact, sets
`DeveloperSettings/ForceWifiOn` and `PowerOptions/AutoSleepMinutes=90`, adds a
**Cobalt** entry to the reader's own menu, and ejects. It leaves the firmware's
root SSH server disabled.

Developers who need Wi-Fi deployment may opt in explicitly:

```
kobo setup --enable-ssh
```

That enables the firmware's **own root SSH server** and makes the setup
reproducible. It creates or reuses the dedicated `~/.ssh/kobo_cobalt` key and
stages only its public half inside Cobalt. After the restart, open Cobalt once
from the reader menu: its root-owned start script appends the key to
`/root/.ssh/authorized_keys` exactly once and deletes the staged copy. The
private key never leaves the computer and no password is created or weakened.
`kobo setup --undo` disables the server again. A plain `kobo setup` remains the
recommended owner-facing installation.

With `--enable-ssh`, it then waits. The restart is the one step that has to happen on the reader,
its SSH server only starts at boot, and nothing on this side can press the
power button, so the command asks for it and then watches the network for the
reader to come back. Open Cobalt once after the reboot to install the staged
public key; the command then prints the address and exact `kobo deploy` line.
It identifies the reader first by *change*: it records which addresses answer
on port 22 before the wait, and only ones that were not answering and now are
candidates. It then authenticates with the new dedicated key and asks the same
read-only identity script every other device command uses.

Change alone was not enough. A laptop waking from sleep mid-wait was reported
as the reader, and a confident wrong address is worse than none. The obvious
second test was the SSH banner, and it was wrong: this firmware runs **OpenSSH**,
not Dropbear as this file claimed for months, so a banner check rejected the
very device it was written to find. What each newcomer gets asked instead is
who it is, over the same identity script every other command uses. A reader
says so. Anything else is passed over and named at the end. An address that
accepts a connection but answers neither way is asked again next round rather
than written off, because a booting reader does exactly that.

`--no-wait` skips that SSH wait and `--no-menu` skips the menu entry.
`kobo setup --undo` puts every part of the setup back,
and `kobo setup --dry-run` prints what it would do without touching anything.
including for `--undo`, which is what `--undo --dry-run` means.

Both settings are the reader's own, applied by the reader's own code, so
nothing here becomes a second owner of the radio or of power. The sleep timer
is the same key [`kobo session --sleep-after`](#what-actually-stops-the-suspend)
uses, and for the same reason: the suspend is requested by nickel itself, so
nickel's timer is the only thing that can prevent it. It costs battery, and the
reader's Energy saving screen overrides it at any time.

The optional SSH server is the part worth explaining, because it is not ours. Firmware
4.42 and later ship one, switched off, gated on the name of a file on the book
partition: `.kobo/ssh-disabled`. Renaming it to `ssh-enabled` is the firmware's
documented mechanism, and the file says so in its own text. Renaming it back
is the whole of the uninstall. This was found on a factory-reset Clara BW
running 4.45.23697, and it replaces the worse answer that came before it:
`EnableDebugServices=true`, which brings up telnet and FTP as root **with no
password at all** and still does not give you `kobo deploy`.

### Why Cobalt itself is not a `KoboRoot.tgz`

The ordinary way to install anything on a Kobo is to drop a `KoboRoot.tgz` into
`.kobo/`, which the firmware unpacks **as root, at `/`, at the next boot**. It
is also the one mechanism on the device that can leave it unbootable, because
nothing checks the paths inside before extracting them over the running system.

So Cobalt is not shipped that way. `kobo setup` copies the same files straight
into `.adds/cobalt` as a plain folder, which the reader never elevates. The cost
is that a folder copy does not trigger the firmware's update-and-restart, so the
reader has to be restarted by hand once for the SSH server to start. That is one
button held down, in exchange for never handing the boot script an archive. The
worst outcome of a setup that goes wrong is a folder to delete.

`kobo package` still builds a `KoboRoot.tgz` for owners who want the usual
route, and `kobo inspect` proves before it is copied that every path in it falls
under `.adds/cobalt`.

### The one archive setup does stage, and what is checked first

There is exactly one exception, and it is the menu entry. A way into Cobalt from
the reader's own home screen means running code inside `nickel`, and nothing on
the book partition can do that. `kobo setup` therefore stages
[NickelMenu](https://pgaskin.net/NickelMenu), pinned to one release, downloaded
over HTTPS and checked against a recorded SHA-256, so the transport does not have
to be trusted, and writes a single entry beside it:

```
menu_item :main :Cobalt :cmd_spawn :quiet:/mnt/onboard/.adds/cobalt/start.sh
```

`--no-menu` skips all of it.

Two things make this acceptable rather than a hole in the rule above.

The first is NickelMenu's own failsafe, which is the reason it is worth using at
all rather than reimplementing. It moves its plugin aside *before* it hooks
anything and only puts it back some seconds after a successful start, so a
reader that crashes while hooking comes up at the next boot with nothing to
load. It cannot boot-loop, which is the failure that makes `KoboRoot.tgz`
frightening in the first place.

The second is ours. The firmware extracts the archive as root without looking
inside it, so `kobo setup` looks: it lists the members and **refuses to write
any archive** that is not exactly NickelMenu's two paths,
`./usr/local/Kobo/imageformats/libnm.so` and `./mnt/onboard/.adds/nm/doc`. An
archive naming `./etc/init.d/rcS` is the one that ends a device, and it is
refused by name. It also refuses to overwrite an archive some other mod has
already staged, since `.kobo/KoboRoot.tgz` is a single shared slot.

`kobo setup --undo` takes the entry away. If the reader has not restarted yet it
simply takes the staged archive back, and nothing was ever installed. If it has,
it writes NickelMenu's own uninstall flag, unless another mod still has a
configuration file beside ours, in which case the plugin stays and only the
Cobalt entry goes, because it is shared.

The entry starts Cobalt **on demand**, and deliberately not at boot. `kobod` has
one mode and it is to stop `nickel` and take the panel, so starting it at boot
would leave a device with no stock reader on it, and would spend the safety net
every risky thing in this project leans on, which is that restarting always
comes back to stock.

#### Why not implement the menu ourselves

The home screen is Qt, drawn by a stripped 24 MB `libnickel.so.1.0.0`. A menu
entry means a shared library that Qt will load into that process, resolving
mangled C++ symbols out of a proprietary binary and rewriting the GOT entry
behind one of them under `mprotect`. None of that can be Rust in any useful
sense, and all of it is `unsafe`, which this workspace confines to `kobo-abi`.
It would be NickelMenu again, without NickelMenu's failsafe. The four symbols it
depends on were checked against this device's firmware (4.45.23697) and are all
present.

### When it will not answer

Every command that fails to reach a device prints the same four causes, in the
order they actually happen: the reader is asleep and its radio is off; Wi-Fi is
off while it is awake; its address has changed; or nothing is listening on port
22. The first is more common than the other three together, and the fix is the
power button.

The last one is worth stating plainly. **Cobalt does not install an SSH server.**
It enables the one the firmware already ships, and only when you ask it to with
`kobo setup`. Nothing the platform does on the reader involves SSH; it is only
how a developer's machine reaches the device.

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

The runtime does **not** put the link back, and there is no option to make it.
It used to be able to, by restarting the supplicant and DHCP client it had
recorded. Those daemons attach to `wlan0`; the restarted reader drives the same
radio from inside libnickel and cannot be told what we started behind it; and
two owners of one radio leaves the reader's own network panel unable to scan at
all: not merely disconnected, but unable to see a network it has known for
months.

That was known and the restore was kept anyway, behind an environment variable,
as a convenience for working on a device over Wi-Fi where losing the link costs
a reboot. It was removed after it erased a device. The reader came up owning a
radio it had not configured, never reached its first watchdog ping, and the
freeze watchdog was armed against it regardless, which is an SoC reset every
ten seconds with nothing synced, until one landed inside a write to the library
database and the device came up asking for a language.

Both links in that chain are now cut. The watchdog is armed only on evidence
that something is feeding it (see `resume_once_fed`), and the network is simply
never restored. A developer working over Wi-Fi loses the link when the session
ends and reconnects the reader's own way, or reboots. That is a worse afternoon
and a better trade.

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

`kobo logs` takes `adb logcat`'s flags too (`-f` follow, `-d` dump, `-t N`
lines, `-c` clear) and every command that takes `--device` also takes `-s`.
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
are refused. Output is byte-for-byte reproducible (`gzip -n -9`, mtime 0,
uid/gid 0) so the printed SHA-256 is worth comparing.

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
  much here. Most of the defects in `BUILD_IN_PUBLIC.md` were found by
  rendering something and looking at it, against real captured data, and the
  tests that survived are the ones written that way.

## Licence

MIT. See [LICENSE](LICENSE). What ships inside the binary, and what its authors
ask for in return, is in [THIRD-PARTY.md](THIRD-PARTY.md).

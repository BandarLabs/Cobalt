# Working against a real reader

Getting Cobalt onto a device, talking to it over Wi-Fi, keeping it awake long
enough to work, and the attended tests that are allowed to write to the panel.
Part of [Cobalt](../README.md).

## Device support matrix

Support is tied to an exact model, device code, firmware, kernel, framebuffer,
and touch profile. A matching model name alone is not sufficient.

| Device | Exact tested identity | Evidence status | Installation status |
|---|---|---|---|
| Kobo Clara BW | N365, code 391, firmware 4.45.23697, kernel 4.9.77 | Read-only probe and owner-attended display, touch, exit, and recovery tests complete | Fully tested |
| Kobo Clara HD | N249, code 376, firmware 4.38.23684 or 4.38.23697, kernel 4.1.15-00136-g12655eaaef89 | Read-only probe and owner-attended display, touch, exit, sandbox, and recovery tests complete | Fully tested |
| Kobo Elipsa 2E | N605, code 389, firmware 4.38.23697, kernel 4.9.77 | Read-only probe and owner-attended display, touch, exit, suspend/resume, and recovery tests complete | Fully tested |
| Kobo Libra 2 | N418, code 388, firmware 4.38.23697, kernel 4.1.15-00868-g58a2758be07 | Read-only probe and owner-attended display, touch, page-button, exit, and recovery tests complete | Fully tested |
| Kobo Libra Colour | N428, code 390, firmware 4.45.23697, kernel 4.9.77 | Read-only probe, measured touch transform, all four bounded display stages, wait timing, guardian restoration, an end-to-end tap, page-button presses (upper 193 pages back, lower 194 forward, the Libra 2's mapping), and a clean stock-reader restart with no watchdog reset all complete on the owner's device. Evidence on [issue #28](https://github.com/BandarLabs/Cobalt/issues/28) and [PR #49](https://github.com/BandarLabs/Cobalt/pull/49). Colour path and suspend/resume not exercised | Fully tested |

`Read-only doctor match complete` means the profile describes the observed
identity, framebuffer, and touch ranges. It does not prove the physical touch
direction, panel refresh behavior, or recovery after Cobalt takes ownership of
the device. Those claims require owner-attended hardware runs; simulator and
fixture results are not substitutes.

Firmware versions not listed here are unsupported even on the same model until
a new read-only probe and the applicable attended evidence have been reviewed.

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
second test was the SSH banner, and it was wrong: this firmware runs
**OpenSSH** rather than Dropbear, so a banner check rejected the very device it
was written to find. What each newcomer gets asked instead is
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

> [!NOTE]
> The firmware version this toggle first shipped in is not 4.42, and may be model-dependent: a Libra 2 on firmware 4.38.23697 ships `.kobo/ssh-disabled`, and renaming it brings up a working OpenSSH. If your firmware really lacks the toggle, install an SSH server manually (like Dropbear via NickelMenu) and copy your public key (from `~/.ssh/kobo_cobalt.pub`) into `authorized_keys` under root's home directory yourself — which is `/.ssh/authorized_keys` on readers whose `/etc/passwd` gives root a home of `/`, such as the i.MX6 models, not `/root/.ssh/`.

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
kobo shell   --device <address> dmesg | tail # run one command and read it back
kobo shell   --device <address>              # or open a session on the reader
```

`kobo shell` exists because the obvious spelling does not work. Running
`ssh root@<address> 'uname -a'` returns nothing at all on this firmware: the
login shell ignores the command it was handed, so the command has to arrive on
standard input with the terminal turned off instead. Every other verb here has
always done that internally, and there was simply no way to ask for it, so
everybody who tried the obvious thing concluded the reader was broken.

The words after the address are joined with spaces and sent as one line of
shell, the way `ssh` and `adb shell` both do, so a pipeline or a redirection
needs quoting for the local shell as well as the device. Given no command at
all it opens an ordinary session with a terminal, line editing and a prompt.
Either way it exits with whatever the reader exited with, so it can be tested
for in a script. It retries a connection the same way the rest of the CLI does,
because a reader that has been idle for a minute refuses the first knock while
its radio wakes up.

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

### The three watchdogs

Three separate things on this device will reset it, and for a long time they
were mistaken for each other. It is worth naming them apart.

| | What it is | How it is handled |
| --- | --- | --- |
| Recovery watchdog | Ours, a shell loop in `/tmp` | Restarts the reader if the runtime dies |
| Freeze watchdog | Kobo's `sickel`, on the session bus | `Suspend` for the session, `Resume` on evidence it is being fed |
| SoC watchdog | A counter inside the MediaTek chip | Given slack for the session, armed again afterwards |

The third one is the one that reset a device every time a session ended, and it
took days to find because it leaves no trace at all. There is no kernel message,
nothing is synced, and the next line in the log is a cold boot.

The numbers are the whole explanation:

```text
mtk-wdt 10007000.toprgu: Watchdog enabled (timeout=31 sec, nowayout=0)
[112:feeding_thread] watchdog feeding_interval = 28000 ms
```

A kernel thread feeds a thirty-one second counter every twenty-eight seconds.
Three seconds of margin, and stopping and restarting the reader is the heaviest
thing this device ever does.

Almost everything about the symptom pointed elsewhere. The reset landed about
ten seconds after `kobo present` handed the panel back, so `present` looked
guilty; it is not, and restarting the reader with no display session, no touch
session and no panel involvement at all resets the device just the same.
Scanning `/proc/*/fd` for a process holding `/dev/watchdog` finds nobody, which
reads as "never armed" and is wrong, because the feeder is a kernel thread and
kernel threads have no descriptor table. Reading that as innocence sent the
search after the Bluetooth chip, the freeze watchdog and a phantom second reader
in turn. What settled it was `/proc/wdk`, which is writable, and an A/B:

| `/proc/wdk` | uptime across a session | outcome |
| --- | --- | --- |
| `0` (slack) | 247s to 389s | survived, and kept going |
| `1` (armed) | 420s to 446s | reset, cold boot |

So `crates/kobo-hal/src/soc_watchdog.rs` gives the counter slack for exactly as
long as the runtime stands between the reader and the hardware, on the same
lifetime as the freeze watchdog suspension, and arms it again once the reader is
demonstrably back. The window is bounded by a guard that restores the previous
value on every exit path including a panic, the recovery path arms it explicitly
because a killed session leaves nobody to do so, and the kernel arms it anyway
on the next boot. A device that resets every time a developer looks at it has no
working safety net either.

### Reading the black box

The watchdogs above are only nameable because there is a record of what the
session was doing when one of them fired. Standard output is not that record.
It lives on a tmpfs a reboot empties, and the copy on `/mnt/onboard` is VFAT
with buffered writes, so the last several seconds -- exactly the seconds that
matter -- are never on the card when the device comes back.

So `crates/kobod/src/blackbox.rs` writes to the book partition and calls
`fsync` after every single line. That is deliberately expensive, and it is the
only way to learn *when* the device died and *what it was doing* at the time.

| | |
| --- | --- |
| Where | `/mnt/onboard/.kobo-blackbox.log` |
| Switch | `KOBO_BLACKBOX=1`, off otherwise |
| Set for you by | `kobo present`, for the whole session |
| On the device | `kobo shell --device IP "tail -c 2000 /mnt/onboard/.kobo-blackbox.log"` |

It appends and never truncates, so the record of a session that ended in a
reset survives the run investigating it. It is off by default because a
synchronous write per event on somebody's only reader is a cost that should be
paid on purpose; a session driven from a development machine is the opposite
case, which is why `present` turns it on without being asked.

Every line is two clocks and an event:

```text
   3209.84  1501.07 session finished, handing the panel back
   3210.21  1501.43 panel and touch released, restarting the reader
   3210.69  1501.91 alive
```

The first column is the kernel's own clock from `/proc/uptime`, the second is
seconds since this session started. Read the first column when you want to know
whether the device rebooted and the second when you want to know how far into a
session something happened.

The kernel clock is the one that answers the question that matters most, and it
answers it by itself. On this device `/proc/uptime` counts suspended time and
never resets on its own, so a reading that suddenly drops to single digits is a
boot and nothing else. A wall clock cannot make that claim, because NTP can step
it. This is also why a trace that simply stops, with no `session finished` line
before it, is the signature of a hardware reset rather than an ordinary exit:
nothing got the chance to write a last line.

Anything an application logs goes to the trace too, prefixed `app`, so a hang
inside an application can be read back the same way. This is the reason to
reach for `context.log(...)` around work you suspect: an application logs to
explain itself, and the times it most needs to be believed are the times it
took the reader down with it.

### Per-frame timing

One line per painted frame on standard error, off unless `KOBO_FRAME_TIMING=1`:

```text
frame 1264x1680 wf=2 convert=41ms write=18ms submit=214us wait=401ms
```

It separates the grayscale conversion, the framebuffer write and the two
ioctls, and names the waveform the panel was actually given. This is what found
the Libra 2 tap delay, and it is the fastest way to tell a slow conversion apart
from a slow panel. It goes to standard error rather than the black box because
the black box costs an `fsync` per line, which would swamp the thing being
measured; `start.sh` captures standard error already.

The excerpt above is the whole method in three lines. A reader that had returned
to the stock interface looks identical whether the session ended cleanly or the
`SoC` watchdog reset the device, and the two have completely different causes.
Here the session announced its own end at its lease expiry and the next line is
`alive`, so it was a timed handback and there is nothing to fix. Had it been a
reset there would be no `session finished` at all, and the following line would
carry a kernel clock of a few seconds.

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

## Attended display smoke tests

`kobo smoke-display` is not compiled into a default build; the CLI must be
rebuilt with `--features device-write`. Before it changes anything it requires,
in order: the exact confirmation phrase on the command line, the exact unlock
phrase in the device process environment, an exact match of every probed
hardware value against the profile, and an exact match of the device code,
serial model prefix, firmware version and kernel release.

Ordinary display, guard, synthetic-touch, and exclusive touch-grab paths also
require the profile's reviewed `write_ready` flag. The HAL owns the smoke
operation's fixed regions, waveform choices, restoration, and verification, so
the caller never receives a general candidate-capable display session. It may
ignore only the evidence-pending blocker. Any geometry, framebuffer, or
identity blocker still refuses the smoke run before the framebuffer is opened.

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

Proven on the physical N365, in order: a GC16 refresh that writes no pixel; a
reversible pixel write restored and verified byte for byte; a whole-screen
snapshot and restore; the DU waveform; the touch transform, against a physical
touch; guardian restoration after a failed child; stopping and restarting the
stock reader; an application rendered on the panel and taps reaching it; and
HTTPS, including a 24 MB download.

Proven on the physical N605: the same four bounded GC16, reversible-pixel,
whole-screen restore, and DU stages; the Elan touch transform against a
physical top-left touch; guardian restoration after a deliberate child
failure; a Todo session rendered at 1404×1872 with physical taps reaching UI
actions; release of panel and touch followed by a successful stock-reader
restart; and suspend/resume with monotonic device uptime and no Cobalt process
left running.

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

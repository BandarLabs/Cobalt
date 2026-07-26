# Building an application platform for a Kobo, in public

This is a running, factual log of building a Rust SDK and runtime for an E Ink
reader — a Kobo Clara BW — starting from nothing but a USB cable.

It is written as it happened, including the things that went wrong, because the
wrong turns are where all the real information about this device is.

One rule governs everything below: **nothing we cannot revert with a reboot.**
There is exactly one device. It is not a development board, it is somebody's
e-reader. Every decision in this project is downstream of that.

---

## 1. Getting in

The Clara BW mounts as USB mass storage. That gets you `/mnt/onboard`, the FAT
partition the reader uses for books — and nothing else. No shell, no processes,
no `/proc`.

The way in is the stock firmware's own developer switch: a `.kobo` config
setting that starts an SSH server (Dropbear) on the device. Nothing was
patched, no partition was written, no bootloader was touched. The device does
this itself; it just does not advertise it.

Two immediate surprises shaped every later tool:

- **`scp` does not exist.** BusyBox 1.31.1, no SFTP subsystem.
- **Remote `argv` is ignored.** `ssh root@device 'echo hi'` connects, exits 0,
  and prints nothing. Commands only run if piped to *stdin*.

So the file-transfer tool became: `gzip -9 | base64`, sent inside a heredoc on
stdin, decoded on the device, and always verified with `md5sum` on both ends. A
crude method, but every byte that has ever reached this device has been
checksummed, which has already caught one truncated upload.

---

## 2. What the hardware actually is

Read-only exploration first — `/proc`, `/sys`, `dmesg`, and the vendor binaries.

| | |
|---|---|
| SoC | MediaTek MT8512 ("MT8110 board" in the device tree) |
| Kernel | Linux 4.9.77, ARMv7, SMP PREEMPT |
| RAM | ~456 MB usable |
| Panel | 1072×1448, 32 bpp, 4288-byte stride, ~300 DPI |
| Display controller | **HWTCON** (MediaTek), *not* MXCFB |
| Touch | Cypress `cyttsp5_mt` on `/dev/input/event1` |
| PMIC / EPD power | ROHM BD71828, SY7636 |
| Frontlight | TI LM3630A |
| Hardware watchdog | `mtk-wdt`, **31 s**, fed by a kernel thread every 28 s |

The display controller matters more than anything else on this list. Most Kobo
tooling in the wild targets **MXCFB** (the i.MX6 controller in older models).
The ioctl numbers, structures, and waveform constants are *different* on
HWTCON, and getting them wrong means writing an arbitrary struct into a display
driver. So the ABI was never guessed.

### The vendor header oracle

Instead of reverse-engineering the ioctls by trial, we took the vendor's own
kernel header and generated the constants from it, then wrote fixtures that
assert our Rust structs match it byte for byte — size, alignment, and every
field offset. If a struct ever drifts, the test fails on the host, long before
anything is sent to a driver.

Separately, `kobo-abi` keeps HWTCON, MXCFB, and sunxi definitions **strictly
apart**. There is no shared "refresh" struct. They are different hardware and
merging them would only ever produce a plausible-looking wrong number.

---

## 3. The first pixel

The display is not a framebuffer you memcpy into and forget. E Ink needs an
explicit *update* with a **waveform** — a per-pixel drive sequence that says how
to get from the old state to the new one. Choose the wrong one and the panel
looks broken even though the pixels are correct.

The ones that matter here:

| Waveform | Levels | Behaviour |
|---|---|---|
| `INIT` (0) | — | clears the panel |
| `DU` (1) | **2** | fast, no flash, **black and white only** |
| `GC16` (2) | 16 | full quality, visible flash |
| `GL16` (3) | 16 | greyscale, no flash |

The first on-device write was deliberately the smallest provable thing: a single
32×32 `GC16` refresh of **unchanged** framebuffer contents, behind a compile-time
feature, an environment unlock, an exact hardware-profile probe, and two typed
confirmations. It could not change what was on screen even if every other check
failed. It worked, and that one square was the entire result of a day.

---

## 4. Touch

Touch is evdev, but the raw stream is not screen coordinates: it is multi-touch
slots (`ABS_MT_SLOT`, `ABS_MT_POSITION_X/Y`, `ABS_MT_TRACKING_ID`) in panel
orientation, which does not match display orientation.

The decoder is slot-aware and the axis transform comes from the device profile
rather than constants, because the whole point is to support more than one
Kobo later. Grabbing the touch device is `EVIOCGRAB` — and, importantly, the
kernel drops the grab automatically when the holder dies. That is a safety
property we rely on: if our process is killed, the reader gets touch back with
no cleanup code needing to run.

---

## 5. Taking the screen from the reader — and the reboot mystery

To draw a full-screen application you have to stop Nickel, the stock reader,
because it owns the framebuffer, input, power and Wi-Fi.

A five-second handoff worked perfectly. A ninety-second handoff **rebooted the
device.**

The culprit was `sickel`, a small Kobo daemon nobody documents. It registers
`com.kobo.watchdog.Sickel` on the session bus and expects the reader to call
`Ping`. When the pings stop, it concludes the reader has hung — and reboots the
device. From its point of view, a reader we stopped on purpose and a reader that
crashed are identical.

We did not kill it. It exposes exactly three methods — `Suspend`, `Ping`,
`Resume` — and suspending is plainly the supported way to hold it off. Killing
it would leave the device with no freeze protection and no way back short of a
reboot; suspending is reversible with one call.

That fix held for weeks of short sessions. It was not the end of the story
(see §9).

### The safety net

Owning the screen is wrapped in layers, all of which fail *towards* the stock
reader:

1. The reader's exact command line and environment are saved before it is
   stopped.
2. A detached watchdog process is armed to restore the reader even if our
   process is killed outright.
3. The framebuffer is captured before, and restored after.
4. Teardown runs in reverse order on **every** exit path, including errors.
5. A hard session limit. If a session outlives it, the reader wins.

And underneath all of it: a reboot always boots the stock reader. That is the
floor, and it is why the project is safe to work on at all.

---

## 6. The UI layer, and a font that could not type

The UI is declarative: applications describe a `Screen` from a small set of
primitives and never see a pixel, a font, or an event loop.

Then we built a headless preview renderer, rendered the launcher to a PNG, and
**looked at it**. Two things were badly wrong.

First, the text. The built-in 5×7 bitmap font's `draw_text` called
`to_ascii_uppercase()` on every string — meaning every label in the entire
system was *physically incapable* of rendering a lowercase letter. It had been
that way for as long as there had been text.

The device turned out to be carrying 37 TrueType fonts of its own, including
**Atkinson Hyperlegible** — a typeface designed by the Braille Institute for
low-vision readers. Reading a font already on the device is not redistribution,
so there is no licensing question, and it is a far better choice than anything
we would have shipped. `kobo-ui` stays dependency-free by *defining* a
`Typesetter` trait; a separate crate implements it. Sizes are physical (body =
3.6 mm) rather than pixel counts, so they stay correct across 212–300 DPI
panels.

Fixing the font immediately exposed a second bug: `line_height()` was a `const
fn` returning *bitmap* metrics, used in twenty layout and render sites, while
the renderer was now drawing real type. Lines overlapped. Layout has to ask the
typesetter, not a constant.

### And then the ink got worse

With real, antialiased type on screen, the panel looked *terrible* — smeared,
crushed, ghosting.

The cause was in the table above: every screen after the first used `DU`, which
is a **two-level** waveform. It cannot represent grey. It had been fine for
blocky bitmap text; the moment glyphs had soft edges, every edge pixel was
slammed to pure black or white and the residue accumulated.

Improving the type is what made the drawing bad. The renderer now diffs frames,
refreshes only the changed rectangle, picks `GL16` when grey is present, `DU`
only when the change is strictly two-level, and a full `GC16` flash
periodically to clear ghosting.

---

## 7. Applications that actually open

Until recently the launcher could describe an application but not start one.
Launching is now a runtime concern: an application asks the runtime by *name*,
the runtime resolves that name against a catalogue directory it chooses, and
the panel and touch device are held across the swap so opening something does
not flash the reader back for a moment.

Names are validated, never trusted as paths — an application that could name a
path could start anything on the device.

**Back is the runtime's, not the application's.** The runtime draws it, the
application cannot remove it, and the application never sees the tap. That is
what makes it a reliable way out of anything, and it is the same reason iOS owns
its own navigation chrome.

---

## 8. Four bugs found by actually using it

Nothing here was found by reading code. All four came from putting it on the
panel and tapping.

**1. Only the first application received touch.** The touch pump was started
per application, and the underlying receiver can only be *taken once* — so
every application launched after the first got no input at all. Tapping Hello
worked; nothing inside Hello did. There is now exactly one reader thread on the
touch descriptor for the whole session, and the destination is swapped as
applications change. Taps that arrive *between* applications are deliberately
dropped rather than queued, so a tap meant for a closing application cannot act
on the next one.

**2. A missing file cost the user the reader.** A reboot cleared `/tmp`, which
wiped the staged binaries. The runtime stopped Nickel, discovered the
application did not exist, and handed back — costing half a minute and the
network connection for a file that could have been checked first. There is now
a preflight check before the point of no return, and its error message says
explicitly that nothing on the device was changed.

**3. A missing catalogue entry killed the whole session.** Tapping an entry
whose binary was absent ended the session and restarted the reader. It now
returns to the launcher: one missing entry should not take everything else down
with it.

**4. The disarm marker was deleted immediately after being written.** The
recovery watchdog is cancelled by writing a file — into the state directory
that the very next line deletes. So the watchdog fired after *every* session.
It happened to be harmless, because it checks whether the reader is already
running, but it was firing every single time.

---

## 9. The reboot, properly this time

Then the device started rebooting again, roughly thirty seconds after a session
ended. `sickel` was already suspended, so the earlier explanation no longer
applied. Rather than guess, we measured.

**First, we read the watchdog.** `sickel` is a 21 KB binary. Pulled it off the
device and disassembled it — as Thumb-2, after ARM mode produced nonsense — and
resolved its PLT entries against the relocation table:

- `Suspend()` → `QTimer::stop()` — **indefinite**, cannot fire.
- `Resume()` → `QTimer::start(10000)` — a **ten second** fuse.
- `Ping()` → restarts that same ten second timer.
- The timer's expiry calls `reboot()`.

**Second, we watched the bus.** `dbus-monitor` on the session bus showed the
reader pings **every five seconds**, against that ten-second fuse. Two missed
pings is a reboot. It also showed something undocumented: on startup Nickel
issues about nineteen rapid `Suspend`/`Resume` pairs before settling into its
ping cadence — and since `Suspend` is not reference-counted, that is worth
knowing.

This immediately exposed a real ordering bug: teardown resumed the watchdog as
soon as the reader *process existed*, which lights a ten-second fuse that a
still-booting reader cannot feed. The runtime now watches the bus and hands the
watchdog back only once it has *seen* the reader feed it. Waiting is the safe
direction: a suspended watchdog has a stopped timer and cannot fire at all, so
dying while waiting leaves the device unprotected until its next reboot — a
degradation — rather than rebooting it, which is a failure.

But the thirty-second reboots were something else, and the clue was in what
survived: the log lost every line after the first. Those writes were sitting in
the page cache of a FAT partition, so this was an **unclean** reset — not a
`reboot()` call.

Thirty-one seconds is the `mtk-wdt` hardware watchdog.

The cause was ordering again: teardown restored the screen but kept the display
and touch descriptors **open while the reader restarted**, so two owners were
bringing up the same EPD controller. Lengthening the teardown (to wait for that
ping) turned an occasional race into a reliable one. The panel and touch device
are now released *before* the reader is started.

The lesson we keep relearning: on this device, **ordering is the whole game.**

---

## 10. Sessions that end when you say so

Until now a panel session ended on a timer, because a timer was the safest
thing to write while the exit path was still unproven. That is a strange thing
to hand somebody: the device would take the screen back mid-sentence.

Removing it meant separating two ideas that had been the same variable. A
*session* now ends when the reader taps `Return to Kobo reader`, when the
application asks to leave, after fifteen minutes with no tap and no repaint, or
at a two-hour backstop. *Recovery* — the detached watchdog that restarts the
reader if the runtime is `SIGKILL`ed — used to sleep for the session length and
then act, which tied how fast the device recovers to how long a session is
allowed to be. Unusable once sessions can run for hours.

So recovery became a heartbeat. The watchdog script loops: read a counter,
sleep sixty seconds, read it again, and act only when two consecutive reads
agree. Counters rather than timestamps, because BusyBox spells date arithmetic
differently on every device. The runtime beats **from inside its event loop**,
so a beat proves the loop is running rather than merely that the process
exists.

While rewriting it we found a bug that had been there the whole time: the
cancel marker lived *inside* the directory teardown deletes at the end of a
session. Every clean exit deleted the marker it had just written, so the
watchdog fired every single time — masked only by the fact that it restarts a
reader that is already running, which looks like nothing happening. Markers now
live outside that directory.

---

## 11. Credentials the application never sees

Two of the new applications talk to the internet, and one of them needs an API
key. The obvious design — hand the application its key — is the one that means
every application, and every crash dump, and every log line, is now a place a
key can leak from.

Instead, `Task::Post` carries the *name* of a secret. The runtime reads
`/mnt/onboard/.adds/cobalt/secrets/<name>` and attaches the `Authorization`
header itself. The application composes the request and never holds the
credential; a test asserts the outgoing body contains no `sk-`, no `bearer`,
and no authorization header at all.

Two details matter more than they look:

- **Secret names are `[A-Za-z0-9_-]{1,64}`.** The application also chooses the
  destination URL, so a name that could escape that directory would be a way to
  post any readable file to any server.
- **`post` does not follow redirects,** although `fetch` does. Following one
  means replaying a body *and a credential* to an address the server picked.

A named secret the runtime does not hold is a refusal, not an unauthenticated
request — a 401 from OpenAI and a missing key are different problems and should
not arrive looking the same.

---

## 12. Three applications, and the one that broke the UI layer

**Tic-tac-toe** and **Chat** were built on primitives that already existed. The
game is a `Grid`; the chat keyboard is also a `Grid`, because an on-screen
keyboard is a grid of keys and adding a `Keyboard` node would have meant
teaching the layout engine, the renderer, the hit test and the wire format
about something that is already expressible.

**Gutenshelf** — search and read sixty thousand Project Gutenberg books — did
not fit, and finding out why was the most useful thing in this stretch.

It reads plain text rather than EPUB. An EPUB is a zip of XHTML that would need
an unpacker, an XML parser and a CSS subset before one word reached the panel.
The plain text is the same book, minus italics, in a few hundred lines that
cannot be attacked by a malformed archive. Books arrive in 256 KB pieces
because the IPC transport carries half a megabyte and a Victorian novel is
several times that, so `Task::Fetch` grew a byte offset and sends a `Range`
header. The next piece is fetched while there are still pages left to read.

Then: **where does a page end?**

The first answer was a character budget, tuned by eye. A test was written to
prove it fitted the panel. The test passed. It also passed at *three times* the
budget, which is how we learned it was proving nothing.

The reason is a line in the layout engine: it stops laying out nodes once the
cursor passes the bottom of the content area. That is right for a screen that
is slightly too long — better than drawing off the edge — and it is silent.
A page that measured wrongly loses its last paragraph on the device and
**nowhere else**. Worse, the page-turn controls were the last thing in the
flow, so the first thing a long page dropped was the way to leave it.

Two fixes, one structural and one architectural.

The controls moved into the pinned bottom bar, which the layout *reserves*
before any content is placed. Content now stops above them by construction.

And pagination stopped guessing. It measures with the runtime's own wrapping
and line height — but an application cannot do that, because it never knew what
panel it was on. The handshake reported width and height in pixels, and every
size in this UI layer is derived from a *physical* measurement; pixels alone do
not say how big a pixel is. So `Welcome` now carries the panel's density, the
SDK exposes it as `Context::metrics()`, and `kobo-sdk` installs the same
typeface the runtime lays out with, so both sides measure identically.

The pagination itself lives in `kobo-ui` where every application can reach it,
and it is tested against five real Kobo panels by laying every page out with
the production layout engine and asserting that every paragraph it claimed
would fit was actually drawn.

That test also settled the original question. A page of description holds
noticeably more text than a page of dialogue, because dialogue is mostly short
paragraphs and the gaps between them. No character budget can serve both. It
was never a matter of picking a better number.

---

## 13. Four bugs that only a real device and a real network could find

The three applications all passed their tests. All three were broken on the
panel. Every one of these was invisible in the simulator, and every one of them
presented to the owner as the same symptom: *nothing is happening*.

### The runtime only noticed finished work when something else happened

Ask the chat application a question and it sat there. The reply had arrived —
the log proved it — and the screen did not change until the next touch, at which
point the answer appeared instantly.

The event loop looked correct:

```rust
match events.recv_timeout(wait) {
    Err(RecvTimeoutError::Timeout) => continue,   // ← the bug
    Ok(Event::Touch(event)) => { ... }
    ...
}
for finished in tasks.drain() { ... }             // ← never reached
```

`continue` skips the rest of the loop body, and the rest of the loop body was
the only place a completed task was ever delivered. A background task therefore
finished into a channel nobody read until an unrelated event arrived.

The lazy fix is to fall through. That alone still leaves an answer waiting up to
one heartbeat — ten seconds — which on a device that spends its life asleep is
not a fix, it is a shorter bug.

So the task runner now takes a wake callback and calls it from the finishing
task's own thread, after the result is in the channel:

```rust
let _ = sender.send(Finished { task, outcome });
if let Some(wake) = wake { wake(); }
```

The runtime passes a closure that pushes `Event::TaskReady` onto the same
channel touches arrive on. No polling, no second delivery path — the wake
carries nothing, so `drain` remains the only way a result reaches an
application. Two tests, one of which covers refusals, because a denial is
delivered without a thread and so has its own way of being missed.

### Every chat reply was unreadable, and it was not the parser

The next symptom was "The reply could not be read." The obvious suspect was the
hand-written JSON parser. It was innocent: fed the exact bytes from the real
endpoint it parsed them perfectly.

`api.openai.com` over HTTP/1.1 answers `Transfer-Encoding: chunked` — always,
because it is behind Cloudflare. The HTTP client handed the body back with the
chunk framing still in it, so the application was asked to parse:

```
17\r\n{"choices":[{"message":\r\n12\r\n{"content":"h"}}]}\r\n0\r\n\r\n
```

The reason this had never been caught is that the other two network users —
gutendex and gutenberg.org — both send `Content-Length`. One endpoint's framing
choice was load-bearing.

Worth noticing: `curl` hides this. `curl` negotiates HTTP/2, where framing is a
transport concern and chunked does not exist. Reproducing the client's actual
behaviour needed `curl --http1.1`, and until that flag was added the response
looked nothing like what the device received.

### Most of Project Gutenberg could not be downloaded

"A Room with a View" failed with *the network could not be reached*. The
network was fine. Gutenberg answers

```
GET https://www.gutenberg.org/ebooks/2641.txt.utf-8
302 → http://www.gutenberg.org/cache/epub/2641/pg2641.txt
```

— a redirect from TLS to plaintext, for a file it also serves perfectly well
over TLS. The client refused, correctly: a redirect must never be able to
quietly downgrade a request to plaintext.

Refusing outright, though, made a large part of the catalogue undownloadable in
service of a threat that was not present. The rule is now sharper: a plaintext
target is **upgraded** and retried over TLS rather than followed.

```rust
if let Some(rest) = location.strip_prefix("http://") {
    return Ok(format!("https://{rest}"));
}
```

The request still goes over TLS. A host that will not serve it over TLS fails.
Scheme-relative targets and every other scheme stay refused. This is not a
relaxation — nothing that was previously refused now happens in plaintext.

### The battery was a fiction

A sharp-eyed owner noticed the counter application reported 72%, and kept
reporting 72%. That is `DeviceState::default()`: the device session had been
built with `DeviceServices::simulated()`, so every hardware read was answered
with a believable invention.

An invented reading is worse than a refusal, because an application cannot tell
one from the other and will act on it. The device build now performs exactly
what it has a proven backend for, which today is a read-only battery gauge:

```rust
match kobo_hal::battery::read() {
    Some(_) => Backends::with([Capability::BatteryRead]),
    None    => Backends::none(),
}
```

The gauge is found by reading each supply's `type` file and taking the one that
says `Battery`, not by hard-coding `bd71827_bat`. Naming the part would work on
this Kobo and quietly read the wrong supply on the next one. Unparseable
capacity returns `None` rather than zero — telling an application the battery is
flat when the file is merely garbled is the dangerous direction to round.

The reading is taken on demand and rate limited to thirty seconds, so a session
where nobody asks does no file work at all and an application polling in a loop
cannot turn a read into a busy one.

### What these have in common

None of the four is a hard bug. Each is a few lines. All four were invisible
until the software met a real panel, a real CDN and a real battery, and all four
presented identically to the person holding the device: *it is stuck*.

The tests were not weak — the chat application has 31 of them. They were testing
the application against the runtime's *model* of the world. The parts that
broke were all at the seams where that model meets something it does not own: a
scheduler's wakeup, a server's framing, a redirect's scheme, a gauge's file.

---

## 14. Where it stands

Working: hardware identification, HWTCON display with a real refresh policy,
touch, reversible reader handoff, panel sessions, real typography, sixteen UI
primitives, a browser simulator sharing the same renderer, a bounded protocol,
application launching with runtime-owned Back, and a CLI. 286 tests, no clippy
warnings, statically linked ARMv7 binaries with no device-side dependencies —
`rustup target add` is the entire setup.

Confirmed on the panel: applications launch, their controls work, Back returns
to the launcher.

Not done, and not pretended otherwise:

- **Isolation.** Applications currently run as root. Per-application UID,
  capability dropping, and the tests that prove an application *cannot* reach
  the framebuffer are the single biggest gap to production.
- **Persistence.** No packaging, manifest, install or rollback. Everything
  lives in `/tmp` and dies on reboot.
- **Signing.** None.
- **Power and hardware APIs** are honestly *refused* by the device build and
  exist only in the simulator.
- **Wi-Fi after a handoff** still fights the reader for the radio.
- **The UI is not general enough.** It is a fixed set of primitives, so every
  new idea wants a new one. The next major piece of work is a real
  flexbox-style layout model plus crisp vector drawing, so components become
  compositions instead of enum variants. The back arrow is still a scaled
  bitmap, and it looks like one.

---

## Method notes

Things that have repeatedly paid for themselves:

- **Read the vendor's own header. Don't guess an ABI.**
- **Disassemble the daemon rather than theorise about it.** Ten minutes with
  `objdump` replaced a week of plausible hypotheses about the reboots.
- **Measure the live system.** `dbus-monitor` answered in thirty seconds a
  question that had been open for days.
- **Look at the rendering.** The uppercase-only font survived because nobody had
  actually looked at a picture of the output.
- **Log somewhere that survives a reboot**, and remember that a FAT partition
  buffers — a lost log line is itself evidence of an unclean reset.
- **Checksum every transfer.**
- **Write the test so that it fails first.** A pagination test that passed at
  three times the budget it was checking had been decoration from the day it
  was written.
- **Make failure land on the stock reader.** Every layer fails towards the thing
  the owner actually bought.
- **`curl` is not your client.** It negotiates HTTP/2, where chunked framing does
  not exist. `curl --http1.1` is the only version of the question worth asking.
- **Never invent a hardware reading.** A refusal an application can see beats a
  plausible number it cannot question.

*Updated as work continues.*

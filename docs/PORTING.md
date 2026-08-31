# Porting Cobalt to another Kobo

Part of [Cobalt](../README.md).

Cobalt selects a device profile at runtime. The Clara BW, Clara Colour,
Elipsa 2E, Clara HD, Libra 2, and Libra Colour profiles are fully
hardware-tested at their recorded identity and firmware boundaries. The
current status and exact boundaries are recorded in the
[device support matrix](DEVICES.md#device-support-matrix).

Display and synthetic-touch write entry points demand an exact hardware and
firmware match, so an unknown reader is refused rather than guessed at. A
read-only match is only a porting milestone, not permission to ship or install
the profile. Review every other path that takes device ownership against the
same identity boundary.

Open or join a device issue before writing code. Check for an existing pull
request and agree on the profile or backend shape there to avoid duplicate
ports.

## Help test a device

Testing does not require a code contribution. Comment on the device's porting
issue with its exact model and firmware, and say whether you can run attended
tests. Mention any known hardware revision and whether the device has buttons
or a stylus. Do not post the full serial number.

Start with the read-only doctor report below. Panel and touch tests come later,
after a maintainer has reviewed a candidate profile and named the exact commit
to test. Keep the output and observations on the porting issue so the
implementation PR and support matrix can link to the evidence.

Do not infer support from a similar model. Normal writes remain blocked until
the attended display, touch, exit, and recovery results have been reviewed.
Additional testers are most useful on another firmware build or hardware
revision.

## Port stages

1. **Identify:** post the complete read-only doctor report.
2. **Implement:** add the narrow profile or backend with host tests. A profile
   may remain registered with `write_ready: false` while evidence is pending.
3. **Review:** a maintainer reviews the write boundary and names the exact
   commit for attended testing.
4. **Validate:** the device owner runs the bounded display stages, physical
   touch check, recovery check, and clean exit.
5. **Enable:** set `write_ready: true` only for the exact identity that passed.

## What is device-specific

Less than you would expect. The SDK, the UI layer, the renderer, the protocol
and every application are device-independent. What is measured is:

1. **A `DeviceProfile`**, in `crates/kobo-profile/src/lib.rs`. Panel size and
   stride, framebuffer identity, the touch device and its axis ranges and
   rotation, refresh controller, physical density, and the identity fields
   (device code, serial prefix, firmware, kernel). Its `write_ready` flag
   remains false until the attended evidence in the device support matrix has
   been reviewed. `CLARA_BW_391`, `ELIPSA_2E_389`, and `CLARA_HD_376` are the
   current examples.
2. **A controller backend**, when the device does not use one already present.
   Cobalt currently supports MediaTek HWTCON and Mark 7 MXCFB v2. Their ioctl
   structures and waveform numbers are deliberately separate.
3. **`DisplayMetrics`**, in `crates/kobo-ui/src/lib.rs`. Size and DPI, which is
   what the layout engine reasons about.
4. **Runtime profile registration.** `SUPPORTED_PROFILES` in `kobo-profile`
   contains the profiles the runtime may identify. Device tools probe first and
   select a matching hardware profile; panel-write entry points then apply the
   shared `write_ready_profile` gate, which requires exact identity and reviewed
   evidence.

The framebuffer, touch decoder, and refresh planner consume the selected
`DeviceProfile` rather than assuming one model. The display backend dispatches
on the profile's controller, so a new panel family needs its own backend plus
tests proving that every write path uses the profile selected from the same
probe.

## How to get the numbers

`kobo doctor` is read-only. It opens nothing for writing, grabs no input
device and refreshes no pixel, so it is safe to run against a reader running
its stock software.

It reaches the device over SSH, which a Kobo does not have switched on. Turn
it on first. This step is not gated on the model: it recognises any Kobo
serial, and it only writes files to the USB storage partition, which is the
same partition your books are on.

```sh
cargo run -p kobo-cli -- setup --enable-ssh --no-menu   # over USB, then eject
cargo run -p kobo-cli -- devices                        # find the address
cargo run -p kobo-cli -- doctor --device <address>
cargo run -p kobo-cli -- touch-probe --device <address> --seconds 30
```

`--no-menu` leaves the reader's own menus alone, which is what you want here:
Cobalt itself will refuse to run on an unrecognised device, so a launcher
entry for it would only be a button that declines. `--dry-run` shows what
would be written without writing it, and `setup --undo` switches SSH back off
and removes everything it wrote.

The doctor cross-compiles its own ARM binary, copies it over, runs it and
brings the report back. On the Clara BW that report reads:

```
profile: clara-bw-391 (Kobo Clara BW)
device-tree compatible: mediatek,mt8110, mediatek,mt8512
framebuffer: id=hwtcon 1072x1448 virtual=1072x1448 offset=0,0 bpp=32 ...
identity: model=N365 firmware=4.45.23697 kernel=4.9.77 device-code=391
touch: cyttsp5_mt at /dev/input/event1 X=0..1447 Y=0..1071
result: write ready
```

The hardware-verified Elipsa 2E probe is:

```
profile: elipsa-2e-389 (Kobo Elipsa 2E)
framebuffer: id=hwtcon 1404x1872 virtual=1404x1872 offset=0,0 bpp=32 ...
identity: model=N605 firmware=4.38.23697 kernel=4.9.77 device-code=389
touch: Elan Touchscreen at /dev/input/event2 X=0..1872 Y=0..1404
result: write ready
```

The hardware-verified Clara HD probe is:

```
profile: clara-hd-376 (Kobo Clara HD)
device-tree compatible: fsl,imx6sll-lpddr3-arm2, fsl,imx6sll
framebuffer: id=mxc_epdc_fb 1072x1448 virtual=1088x1536 offset=0,0 bpp=32 ...
identity: model=N249 firmware=4.38.23697 kernel=4.1.15-00136-g12655eaaef89 device-code=376
touch: cyttsp5_mt X=0..1447 Y=0..1071
result: write ready
```

The Libra Colour probe, from a device whose profile is registered but whose
attended evidence is still pending, reads:

```
profile: libra-colour-390 (Kobo Libra Colour)
device-tree compatible: mediatek,mt8110, mediatek,mt8512
framebuffer: id=hwtcon 1264x1680 virtual=1264x1680 offset=0,0 bpp=32 ...
identity: model=N428 firmware=4.45.23697 kernel=4.9.77 device-code=390
touch: Elan Touchscreen at /dev/input/event1 X=0..1680 Y=0..1264
result: read-only matched
write blocker: owner-attended display, touch, exit, and recovery evidence is incomplete
```

The 4.46.23836 Libra Colour candidate probe reads:

```
profile: libra-colour-390-4.46.23836 (Kobo Libra Colour)
device-tree compatible: mediatek,mt8110, mediatek,mt8512
device-tree model: MediaTek MT8110 board
framebuffer: id=hwtcon 1264x1680 virtual=1264x1680 offset=0,0 bpp=32 grayscale=0 stride=5056 map=8494080 type=0 visual=2 rotation=1
identity: model=N428 firmware=4.46.23836 kernel=4.9.77 device-code=390
touch: Elan Touchscreen at /dev/input/event1 X=0..1680 Y=0..1264
result: read-only matched
write blocker: owner-attended display, touch, exit, and recovery evidence is incomplete
```

Every field a profile needs is on that page. The doctor compares the probe with
the registered profiles. A known device is named; an unknown device is reported
as unsupported after its raw fields are printed, making that report the
starting evidence for a new profile. The touch probe then checks a physical
touch at a known corner against the proposed rotated transform.

The full serial number is deliberately never read past its four-character
model prefix.

## What will refuse to work until the profile is right

By design, all of it. `validate` returns `Rejected` on any mismatch, and every
ordinary write or exclusive-ownership path uses `write_ready_profile`, which
also demands an exact device code, serial prefix, firmware version, kernel
release, and completed attended evidence. A profile that is merely close is
treated as a different device. That is the whole point: geometry alone is not
proof of identity, and the failure mode of guessing is somebody else's reader.

The bounded `kobo smoke-display` operation is the only exception while evidence
is being gathered. The HAL owns its fixed regions, waveform choices,
restoration, and verification and never returns a candidate-capable display
session to the caller. It may ignore only the evidence-pending blocker;
geometry, framebuffer safety, and exact identity remain mandatory. Normal
runtime display, guard, synthetic-tap, and exclusive touch-grab paths stay
blocked until `write_ready` is reviewed and enabled.

## Evidence for write-ready support

Post one evidence block on the porting issue and link it from the pull request:

- tested commit, model prefix, device code, firmware, and kernel;
- complete doctor output;
- results for all four
  [attended display stages](DEVICES.md#attended-display-smoke-tests);
- a physical touch sample and an end-to-end tap;
- guardian restoration and a clean return to the stock reader;
- suspend/resume results when the port changes session or power behavior;
- sandbox results when the kernel needs a different isolation path;
- known gaps, including buttons, stylus support, driver quirks, firmware
  coverage, or hardware revisions.

Photos or recordings help review orientation and interaction, but they do not
replace command output or restoration checks.

For a new framebuffer controller, cite the vendor kernel or header used and add
conformance tests for the ABI. Third-party projects can corroborate behavior,
but should not be treated as proof that the target device passed the tests.

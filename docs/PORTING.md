# Porting Cobalt to another Kobo

Part of [Cobalt](../README.md).

**Cobalt has hardware-tested profiles for the Kobo Clara BW (N365, device code
391) and Kobo Clara HD (N249, device code 376).** Every device write is gated
on an exact hardware match, so a different reader is refused rather than
guessed at. Nothing here bricks a device by trying; it simply declines.

This is a genuinely welcome pull request. Open an issue first so the profile
shape can be agreed.

## What is device-specific

Less than you would expect. The SDK, the UI layer, the renderer, the protocol
and every application are device-independent. What is measured is:

1. **A `DeviceProfile`**, in `crates/kobo-profile/src/lib.rs`. Panel size and
   stride, framebuffer identity, the touch device and its axis ranges and
   rotation, refresh controller, physical density, and the identity fields
   (device code, serial prefix, firmware, kernel). `CLARA_BW_391` and
   `CLARA_HD_376` are the worked examples.
2. **A controller backend**, when the device does not use one already present.
   Cobalt currently supports MediaTek HWTCON and Mark 7 MXCFB v2. Their ioctl
   structures and waveform numbers are deliberately separate.

The runtime selects a matching profile after probing and derives its
`DisplayMetrics` from that profile. The touch decoder and layout engine take
the selected profile rather than reaching for a model-specific global. The
browser simulator still presents the Clara BW geometry by default.

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
result: read-only matched
```

Every field a profile needs is on that page. It then validates what it found
against the profiles compiled into it and lists each mismatch, so on an
unsupported device the report *is* your starting profile: each mismatch names
a field and the value your device actually has. The touch probe then checks a
physical touch at a known corner against the proposed rotated transform.

The full serial number is deliberately never read past its four-character
model prefix.

## What will refuse to work until the profile is right

By design, all of it. `validate` returns `Rejected` on any mismatch, and every
write path also demands an exact device code, serial prefix, firmware
version and kernel release. A profile that is merely close is treated as a
different device. That is the whole point: geometry alone is not proof of
identity, and the failure mode of guessing is somebody else's reader.

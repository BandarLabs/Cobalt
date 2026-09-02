# Installing Cobalt

The full owner-facing walkthrough for getting Cobalt onto a supported Kobo over
USB, what to do if a step does not go as described, and how to take it back
off. Part of [Cobalt](../README.md).

The procedure below is fully hardware-tested on the **Kobo Clara BW N365
(device code 391), firmware 4.45.23697**, the **Kobo Elipsa 2E N605 (device
code 389), firmware 4.38.23697**, the **Kobo Clara HD N249 (device code
376), firmware 4.38.23684 or 4.38.23697**, the **Kobo Libra 2 N418 (device
code 388), firmware 4.38.23697**, the **Kobo Clara Colour N367 (device code
393), firmware 4.45.23697**, and the **Kobo Libra Colour N428 (device code
390), firmware 4.45.23697**. Support remains tied to the exact
firmware, kernel, framebuffer, touch, and identity combination in the
[device support matrix](DEVICES.md#device-support-matrix).

Display and synthetic-touch writes require an exact match of framebuffer
identity, geometry, device code, serial model prefix, firmware version, and
kernel release. A different reader or firmware is refused rather than guessed
at.

## What you need

- A charged reader whose entry in the
  [device support matrix](DEVICES.md#device-support-matrix) is fully tested.
  The reader's own installer is gated on battery level and fails silently, so
  charge it first.
- A **USB cable** that carries data. Charge-only cables are common and they
  look identical; if the reader charges but never offers to connect, suspect
  the cable before anything else.
- An internet connection, `curl` or `wget`, `tar`, and OpenSSH `ssh-keygen`.
  These are included with current macOS and mainstream Linux distributions.

No Rust toolchain, Git checkout, or ARM cross-compiler is needed for the
prebuilt installer.

## 1. Install the host command

Stable is the default:

```sh
curl -fsSL https://github.com/BandarLabs/Cobalt/releases/latest/download/install.sh | sh
```

The script selects macOS Intel/Apple Silicon or Linux x86_64/arm64, verifies an
OpenSSH Ed25519 signature over the versioned release manifest, verifies the
host and device package sizes and SHA-256 digests, and installs `kobo` under
`~/.local/bin` without sudo. It prompts through `/dev/tty`, so piping the
script does not consume its answers.

For the current beta candidate:

```sh
curl -fsSL https://github.com/BandarLabs/Cobalt/releases/latest/download/install.sh |
  sh -s -- --beta
```

To install an exact immutable release, add `--version X.Y.Z`. For CI, use
`--non-interactive --yes`; add `--no-setup` when no physical reader is
attached. Beta is never selected implicitly.

## 2. Connect and confirm the reader

This is the step people get stuck on, so in full:

1. Plug the reader into your computer.
2. **The reader asks. Answer it.** A prompt appears on the reader's own screen
   offering to connect to the computer. Tap **Connect**. Until you do, the
   reader charges and nothing is mounted, and setup will report that it cannot
   find a volume.
3. Wait for a volume named **`KOBOeReader`** to appear on your computer. That
   volume is the reader's book partition, and it is the only thing Cobalt
   writes to.

The installer waits for the volume, resolves its friendly model and support
status through Cobalt's device profiles, then shows the model, device code,
profile, firmware, mount point, release version/channel, and intended changes.
It does not print the full serial. The write requires an explicit default-no
confirmation. Unsupported or changed devices, unsupported firmware, and
multiple mounted readers are refused.

Setup copies the verified prebuilt package directly into `.adds/cobalt`, reads
every file back byte for byte, preserves installed apps, app state, secrets,
owner files, and unrelated NickelMenu entries, then ejects where supported.
Under WSL it tells you to eject from Windows and does not claim WSL ejected it.
The firmware's root SSH server remains disabled unless `--enable-ssh` is
explicitly supplied.

The binaries are statically linked, so nothing has to be installed on the
reader to support them.

Not sure? Run `kobo setup --dry-run`. If installation was deferred with
`--no-setup`, run `kobo setup`; the installed command automatically reuses its
signed prebuilt package.

## 4. Restart the reader

Hold the power button until it powers off, then turn it back on. This is the
one step that has to happen on the device, and it is needed because the menu
entry is loaded at startup.

**Then leave it alone for a minute.** NickelMenu moves its own plugin aside
before it hooks anything and only puts it back once it has started cleanly, so
a reader restarted again immediately comes up with the menu entry gone. This is
its failsafe working as designed, and it is the reason the entry cannot leave
you with an unbootable reader.

## 5. Open Cobalt

On this firmware the entry is in the menu at the **bottom right** of the home
screen. (NickelMenu puts its items in the top-left menu on old firmware and in
the bottom-right one from 4.23.15505 onward, and all five tested profiles are well
past that.) Tap it and choose **Cobalt**.

The launcher appears with Cobalt's built-in applications. Store-only
applications are deliberately absent until they are installed over Wi-Fi.

To leave, use **Return to Kobo reader** at the bottom of the launcher. The
stock reader comes back. So does a reboot, always, from anywhere.

## Installing apps after setup

Open **App Store** in the launcher. It immediately shows the last verified
catalog saved on the reader, then checks Cobalt's fixed app release channel
over Wi-Fi.
Each app can be installed, updated or removed independently. Installed apps
appear in the launcher without rebooting.

For app links shared on the web, open **Install links** in App Store and scan
the QR code, or enter its pairing code and verification key in the browser.
This links the browser without an account. Future app-page installs use Wi-Fi
and do not need the USB cable. If the Kobo is offline, reconnect it and open
App Store within 72 hours to continue.

For the `0.2.0` release, **Sudoku** is the end-to-end Store test: it is not in
the USB package. Seeing it in Store, installing it, and then seeing it appear
in the launcher proves that catalog refresh, package verification, installation
and launcher rediscovery all worked.

The Store never replaces Cobalt itself. Full platform updates remain in
**Settings**, use a separate request and preserve installed apps and the
verified catalog. If refresh fails, the last verified catalog remains usable;
if an install fails, the previous installed copy remains in place.

## If something goes wrong

- **Setup says it cannot find a reader.** The reader is plugged in but not
  connected. Look at the reader's screen and tap **Connect**, or try a
  different cable.
- **There is no Cobalt entry after the restart, the first time.** The menu
  entry is the one piece that arrives through the reader's own installer, and
  that installer is gated on battery level and fails silently. Charge the
  reader properly and restart it again. Cobalt itself is already on the device
  either way.
- **There is no Cobalt entry, and a firmware update happened since NickelMenu
  was installed.** A firmware update removes the plugin but leaves its files
  on the book partition, so setup believes NickelMenu is still there and
  stages nothing. Setup says so when it notices the dates disagree. Run
  `kobo setup --menu` to stage NickelMenu again; that keeps every menu entry
  already on the reader.
- **The Cobalt entry was there and then vanished.** That is NickelMenu's
  failsafe, which reads any unexpected restart of the reader software as a
  crash and disables itself rather than risk a boot loop. You can confirm it:
  the plugin is left beside itself as `libnm.so.failsafe`. Run
  `kobo setup` again and restart, and this time let the reader sit on its home
  screen for a minute before touching it.
- **Setup refused to do something.** Read what it printed. It refuses rather
  than guesses, and it names the reason: an unrecognised volume, a menu slot
  another mod is already using, or a file that did not read back byte for byte.
- **The screen looks wrong, or nothing draws.** Cobalt declines to write to a
  panel it does not recognise exactly. Hold the power button to reboot, and you
  are back in the stock reader with nothing to undo.

## Deploying over Wi-Fi instead

If you are developing rather than reading, `kobo setup --enable-ssh` turns on
the firmware's own SSH server so that `kobo deploy` can install without a
reboot. That is a developer path with its own trade-offs, and it is described
under [Connecting a device](DEVICES.md#connecting-a-device).

## Updating or building from source

Rerun the same stable or beta installer command to update. The host binary and
verified release directory are replaced atomically; an interrupted download is
never activated. Running it again at the same version is safe.

Developers can keep the original source-build path:

```sh
rustup toolchain install stable
git clone https://github.com/BandarLabs/Cobalt.git
cd Cobalt
rustup override set stable
rustup target add armv7-unknown-linux-musleabihf
# macOS: brew install messense/macos-cross-toolchains/armv7-unknown-linux-musleabihf
# Debian/Ubuntu: sudo apt-get install gcc-arm-linux-gnueabihf
cargo run -p kobo-cli -- setup --source
```

`--source` builds the device package before writing. The direct-folder writer,
profile checks, confirmation, preservation rules, and byte-for-byte readback
are the same as the prebuilt path.

## Removing it

Cobalt never writes to the root filesystem, the bootloader, the kernel, a
partition table or any startup script, so removing it is deleting a folder:

```sh
kobo setup --undo
```

Or, with no tooling at all, plug the reader in and delete `.adds/cobalt` from
it. That is the entire uninstall. If you used `--enable-ssh`, `--undo` also
switches the SSH server back off.

To remove the host command, read `~/.local/share/kobo/install-state`, remove
only the `binary` path named there and `~/.local/share/kobo`, then remove the
clearly delimited `Cobalt kobo installer` block from the shell startup file the
installer reported. Do not delete another `kobo` command at a different path.

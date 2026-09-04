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
390), firmware 4.45.23697 or 4.46.23836**. Support remains tied to the exact
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
curl -fsSL https://bandarlabs.github.io/Cobalt/install.sh | sh
```

This canonical stable discovery path is served by GitHub Pages from
`main:/docs` after stable promotion. It fixes discovery and avoids depending on
a particular stable release asset name; it does not solve self-verification.
The one-line route trusts GitHub Pages HTTPS for the bootstrap because a script
cannot verify itself before the shell executes it. Once running, the small
Pages bootstrap verifies the signed release manifest and the full release
installer before executing it. The release installer then verifies every host
and device artifact.

To install an exact immutable release, add `--version X.Y.Z`. For CI, use
`--non-interactive --yes`; add `--no-setup` when no physical reader is
attached. The public bootstrap and prebuilt `kobo setup` install the stable
platform only. Enable **Beta updates** exclusively in Cobalt Settings after a
normal stable installation, or use the source workflow for development.
Settings shows the installed version and channel, requires a separate
confirmation before changing it, persists the choice, and verifies the signed
platform manifest. Returning to Stable changes future platform and Store
checks without USB, downgrading, or deleting apps, state, or secrets.

### High-assurance signed bootstrap

The recommended route when GitHub HTTPS alone is not sufficient verifies
`install.sh` before execution. Choose an exact immutable release, download the
manifest, SSHSIG, and script as data, verify the manifest with the pinned
release key below, then verify the script against the signed `bootstrap`
entry. Obtain this signer line from a separately trusted copy of this guide or
repository; its SHA-256 fingerprint is
`SHA256:ufJnWeLeZxeWlrY7KXb1MadhxMHYZdHSmk21Nmovgbo`.

```sh
version=0.3.4
tag=v$version
base=https://github.com/BandarLabs/Cobalt/releases/download/$tag
dir=cobalt-installer-$version
(umask 077 && mkdir "$dir") || exit
cd "$dir"
curl -fsSLO "$base/cobalt-host-manifest.txt"
curl -fsSLO "$base/cobalt-host-manifest.txt.sshsig"
curl -fsSLO "$base/install.sh"
printf '%s\n' \
  'cobalt-release ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIL7XUR3p+tvPgftO/kRbigc8gagzP2RBDG3tWIu/1KXe' \
  > allowed_signers
ssh-keygen -Y verify -q -f allowed_signers -I cobalt-release \
  -n cobalt-host-release -s cobalt-host-manifest.txt.sshsig \
  < cobalt-host-manifest.txt
set -- $(awk '$1 == "bootstrap" && $2 == "install.sh" && NF == 4 {
  print $3, $4
}' cobalt-host-manifest.txt)
test "$#" -eq 2
test "$(wc -c < install.sh | tr -d ' ')" = "$1"
if command -v sha256sum >/dev/null 2>&1; then
  actual=$(sha256sum install.sh | awk '{print $1}')
else
  actual=$(shasum -a 256 install.sh | awk '{print $1}')
fi
test "$actual" = "$2"
sh ./install.sh --version "$version"
```

Keep the pinned signer line from this repository or another out-of-band trusted
copy, not from the release being checked.

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

Setup writes the complete verified managed payload into `.adds/cobalt.next`
and reads every file back byte for byte before activation. It temporarily
holds the known mutable owner folders, retires the complete old payload as
`.adds/cobalt.prev`, activates the complete new directory, and restores owner
data. A failed swap rolls back; an interrupted rollback is recovered on the
next run without mixing managed versions. Installed apps, app state, secrets,
owner data, and unrelated NickelMenu entries are preserved. Under WSL setup
instructs Windows eject and does not claim WSL ejected the reader.

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

Rerun the stable installer command to update. The host binary and verified
release directory are replaced atomically; an interrupted download is never
activated. Running it again at the same version is safe. Beta platform updates
remain inside Cobalt Settings and do not require USB.

After the first installation, update only the host command with:

```sh
kobo update
kobo update --channel beta
```

Stable is the default. Host Beta requires the explicit selector and changes
only the installed `kobo` executable; it never scans, mounts, ejects, or writes
an attached reader. Returning from a Beta host CLI to the latest Stable CLI is
`kobo update`. An already-current command reports that result without replacing
the binary. The stable setup package and its signed metadata remain
byte-for-byte unchanged, so `kobo setup` never turns into a Beta USB
installation. Each host release keeps `kobo` and its next updater together in
an immutable directory; one atomic selector changes the live pair while the
public command link stays fixed.

The installer lock fails closed. If an interrupted process leaves
`~/.local/share/kobo/install.lock`, first verify that no installer is still
running, then remove that exact directory manually and rerun. The script never
guesses that a lock is stale or races another process to reclaim it.

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

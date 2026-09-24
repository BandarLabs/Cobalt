# Installing Cobalt

How to install Cobalt on a Kobo, fix common problems, update it and remove it.

Cobalt is tested on the models and firmware in the
[device support matrix](DEVICES.md#device-support-matrix). On another model or
firmware, Cobalt shows what has not been tested and asks before it starts.
Kobo firmware 5.x is not supported.

## What you need

- A charged Kobo. The Kobo's own installer does nothing if the battery is low,
  and it does not say so.
- A USB cable that carries data. Charge-only cables look the same. If the Kobo
  charges but never offers to connect, try another cable.
- For the terminal install: macOS or Linux with `curl` or `wget`, `tar` and
  `ssh-keygen`. These come with macOS and most Linux distributions.

You do not need Rust or a copy of the source code.

## Option 1: install from the browser

Open the [browser installer](https://bandarlabs.github.io/Cobalt/install/) in
Chrome, Edge or Opera, plug in your Kobo and follow the steps. It writes
Cobalt and NickelMenu to the Kobo and can add apps at the same time. Then
continue from [Restart the reader](#3-restart-the-reader).

## Option 2: install from a terminal

### 1. Install the `kobo` command

```sh
curl -fsSL https://bandarlabs.github.io/Cobalt/install.sh | sh
```

This installs the stable release. The script checks everything it downloads
against the signed release manifest. It cannot check itself before your shell
runs it, so it relies on GitHub's HTTPS for that first step. To verify it
first, use the [signed bootstrap](#signed-bootstrap) instead.

Options:

- `--version X.Y.Z` installs a specific release.
- `--non-interactive --yes` runs without prompts, for CI.
- `--no-setup` installs the command without looking for a Kobo. Run
  `kobo setup` later.

### 2. Connect the Kobo

1. Plug the Kobo into your computer.
2. **Tap Connect on the Kobo's screen.** Until you do, it only charges and the
   installer cannot find it.
3. Wait for a drive called **KOBOeReader** to appear. This is the Kobo's book
   storage, and the only place Cobalt writes to.

The installer shows the model, firmware and what it will change, and asks
before writing. It does not show the full serial number. It will not continue
with more than one Kobo connected.

To see what would happen without writing anything, run `kobo setup --dry-run`.

The installer writes the new version beside the old one and checks every file
before switching over. If it is interrupted, the next run finishes or rolls
back cleanly. Your apps, their data, your secrets and any other NickelMenu
entries are kept.

On WSL, eject the drive from Windows when setup asks.

## 3. Restart the reader

Hold the power button until the Kobo turns off, then turn it on again. The
menu entry is loaded at startup, so this step is needed.

**Then leave it alone for a minute.** NickelMenu turns itself off if the reader
restarts again straight away, as a guard against boot loops. Waiting a minute
lets it confirm a clean start.

## 4. Open Cobalt

Open the menu at the **bottom right** of the Kobo home screen and choose
**Cobalt**. The launcher opens with the built-in apps.

To leave Cobalt, choose **Kobo reader** in the launcher. Restarting the Kobo
also always returns to the stock reader.

## Installing apps

Open **Store** in the launcher. It shows the last catalog it downloaded, then
checks for updates over Wi-Fi. Apps install, update and uninstall one at a
time, and appear in the launcher straight away.

To install from the website, open **Install links** in Store and scan the QR
code, or enter the pairing code and verification key in your browser. After
that, the **Install** button on any
[app page](https://bandarlabs.github.io/Cobalt/#apps) sends the app to your
Kobo over Wi-Fi. If the Kobo is offline, connect it and open Store within 72
hours.

If a catalog refresh fails, the last catalog stays usable. If an install
fails, the previous version stays in place.

## If something goes wrong

- **Setup cannot find the Kobo.** Look at the Kobo's screen and tap
  **Connect**, or try another cable.
- **No Cobalt entry after the first restart.** The Kobo's installer skips its
  work when the battery is low. Charge the Kobo and restart it again. Cobalt
  is already on the device.
- **No Cobalt entry after a Kobo firmware update.** Firmware updates remove
  NickelMenu. Run `kobo setup --menu` to add it back. Your menu entries are
  kept.
- **The Cobalt entry appeared, then disappeared.** NickelMenu turned itself
  off after an unexpected restart. You can confirm this: its file is renamed to
  `libnm.so.failsafe`. Run `kobo setup` again, restart, and leave the Kobo on
  its home screen for a minute.
- **Setup refused to continue.** It prints the reason, such as an unrecognised
  drive, a menu entry used by another mod, or a file that did not read back
  correctly.
- **The screen looks wrong or stays blank.** Hold the power button to restart.
  You are back in the stock reader with nothing to undo.
- **Software update says "the address or credentials are invalid" on 0.3.1.**
  The 0.3.1 updater cannot install current releases. Reinstall once over USB
  with the steps above. See [issue #154](https://github.com/BandarLabs/Cobalt/issues/154).

## Updating

- **Cobalt**: use **Settings → Software update** on the Kobo. Settings also
  switches between Stable and Beta, keeping your apps and data.
- **Apps**: use **Store**.
- **The `kobo` command**: run `kobo update`, or `kobo update --channel beta`
  for the beta command. This never changes the Kobo.

Rerunning the install script also updates the `kobo` command, and is safe at
the same version.

Kobos installed by an early release, whose menu entry starts
`.adds/cobalt/start.sh`, must be reinstalled over USB once before Settings can
update them.

If an interrupted install leaves `~/.local/share/kobo/install.lock`, make sure
no installer is running, then delete that directory and try again.

## Signed bootstrap

For higher assurance, verify `install.sh` against the signed release manifest
before running it. Get the signer line below from a copy of this guide you
already trust, not from the release you are checking. Its SHA-256 fingerprint
is `SHA256:ufJnWeLeZxeWlrY7KXb1MadhxMHYZdHSmk21Nmovgbo`.

```sh
version=0.3.22   # the release to install
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

## Building from source

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

`--source` builds the package before writing. Everything else works the same
as the prebuilt install.

For development, `kobo setup --enable-ssh` turns on the Kobo's SSH server so
`kobo deploy` can install over Wi-Fi without a restart. See
[Connecting a device](DEVICES.md#connecting-a-device).

## Uninstalling

Cobalt never changes the Kobo's system files, bootloader, kernel or
partitions. To remove it:

```sh
kobo setup --undo
```

This removes Cobalt, its launcher script and its menu entry. Before deleting
anything, it moves your `secrets`, `trust`, `state`, `data`, `apps` and
`store` folders to `.adds/cobalt.recovery.N` on the Kobo, and it reports any
`.adds/cobalt.unusable` folders. Check those before deleting them. If you used
`--enable-ssh`, SSH is turned off again.

To remove the `kobo` command, delete the `binary` path listed in
`~/.local/share/kobo/install-state`, then `~/.local/share/kobo`, then the
`Cobalt kobo installer` block the installer added to your shell startup file.

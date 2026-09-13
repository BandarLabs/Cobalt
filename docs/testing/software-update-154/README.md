# Manual recovery from Cobalt 0.3.1 — issue #154

On 2026-09-07, the maintainer requested a manual installation of 0.3.9 after
Settings > Software update on an Elipsa 2E reported “the address or credentials
are invalid”. The device was N605/code 389, firmware 4.38.23697, kernel 4.9.77.

The GitHub Pages universal installer was downloaded, inspected and run on
macOS arm64 with `--version 0.3.9 --yes --non-interactive --no-setup --no-path`
and a temporary `--install-dir`. It verified the signed release metadata and
installed the host CLI and cached device package successfully. The release
metadata and device UI label 0.3.9 as Stable. USB setup, shell profile changes,
Linux and browser installation were not exercised.

With Cobalt stopped, the old installation was backed up. The published package
was inspected and deployed over SSH using the released CLI. That developer
command omits the standalone launcher, so the reviewed launcher from the same
verified package was installed separately and the exact managed NickelMenu
entry migrated to it. All 27 packaged files then matched their SHA-256 digests.
Owner data and installed store applications were retained.

Cobalt launched, Settings showed 0.3.9, and Check for updates completed over
Wi-Fi. [The device framebuffer capture](stable-039-check.png) shows Stable and
“0.3.9 is the newest published release, and it is what this reader is running.”
This is a successful manual migration and update-discovery check, not a test
of a subsequent OTA installation or Beta channel switching.

The downloaded 0.3.9 archive starts with the plain file
`mnt/onboard/.adds/cobalt-launch.sh`. The 0.3.1 updater permits files only below
`mnt/onboard/.adds/cobalt/` and returns `DeviceError::InvalidInput` for that
member. Current source deliberately tests this refusal before installation
swap (`f49b32c_updater_rejects_bootstrap_release_before_swap`). This is a concrete
package incompatibility consistent with the original screen error; the old
session log did not identify its internal failure stage.

The recovery guidance uses the supported USB setup path. The manual SSH steps
above describe the attended developer test, not a complete first-migration
capability of `kobo deploy`.

# Elipsa 2E portrait rotation acceptance — issue #89

Tested on 2026-09-07 with source commit `633f1f8b97368492594dfbaee8a821977036b602`
(beta base `cf7eb33`). Later evidence-only commits do not change the tested code.

Device: Kobo Elipsa 2E, N605/code 389, firmware 4.38.23697, kernel 4.9.77,
HWTCON 1404×1872 framebuffer, 32 bpp, stride 5616, Elan touch on event2.

## Results

The unpatched doctor rejected the device at rotation 3 with otherwise matching
identity and framebuffer fields. The candidate reports `write ready` in both
portrait poses; see [rotation 1 probe](doctor-rotation1.txt) and
[rotation 3 probe](doctor-rotation3.txt).

- Rotation 3: launcher, Tic-tac-toe and Settings > About rendered. Injected
  touches selected top-left O, bottom-right X and centre O on the game board;
  navigation through Settings/About and back to Nickel also worked.
- Rotation 1: launcher and Tic-tac-toe rendered. Injected touches selected
  top-right O, bottom-left X and centre O; navigation back to Nickel worked.
- The attending device owner explicitly confirmed that Cobalt looked upright
  and finger taps hit the intended controls in both physical portrait poses.
- Both session logs record launcher exit status 0, panel release and reader
  restart. Nickel was running and kobod absent at final restoration. The logs
  end at the freeze-watchdog wait; they do not establish watchdog recovery.

The screenshots below are device framebuffer captures, not camera photographs.
Synthetic input exercises the real daemon's evdev/dispatch/render path, but
uses the profile's inverse transform and is not an independent digitizer
calibration. Physical orientation and finger-touch acceptance are owner-reported.

| Evidence | Capture |
| --- | --- |
| Rotation 3 About/model information | [About](about-rotation3.png) |
| Rotation 3 three-cell selection | [Board](board-rotation3.png) |
| Rotation 1 three-cell selection | [Board](board-rotation1.png) |

Session logs: [rotation 3](session-rotation3.log), [rotation 1](session-rotation1.log).
The earlier rotation-3 game session log was overwritten; its board capture was retained.

## Test installation and restoration

The candidate daemon SHA-256 was
`5987cb11b5feef7c180d4cd81c01190994332c32a6ff5421e48f4aff6b689cf2`.
Matching beta launcher, Settings, Todo and Tic-tac-toe binaries were temporarily
installed because the existing applications used an incompatible protocol.
The separately installed store game also used the old protocol; replacing it
was correctly rejected by the signature/integrity check. Its original files
were restored, and its directory temporarily moved outside the app discovery
path so the matching bundled test game could run. No integrity check was disabled.
These test-environment errors are retained in the rotation-1 log.

After testing, the daemon and all four bundled applications were restored and
byte-compared against their backups. The store game directory was returned to
its original location; its binary and manifest also matched their backups.
The restored daemon SHA-256 is
`8524c3c248a167678e14e6614cdb9b30f836e0f21d43cbd0757c2c28751f5c84`.

## Scope and automated checks

This validates the rotation-3 launch fix and rotation-1 regression on the tested
firmware. Landscape and unverified firmware remain refused by regression tests.
Changing pose requires returning to Nickel and relaunching; live autorotation
is not added. Wi-Fi issues #90/#91 are outside this PR and are not declared fixed.

At the tested code commit, GitHub CI passed host tests, device build, four host
release builds, advisories and release routing. Local focused HAL/profile/daemon
checks passed 351 tests; Clippy, formatting and 100 Node tests passed. Local full
workspace runs encountered Flashcards and Habits failures also reproduced in
full package suites on unchanged beta; no unrelated application changes were made.

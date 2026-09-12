# Elipsa 2E Wi-Fi #90/#91 current-beta acceptance

This record covers the implementation submitted in PR #164, based on beta
commit `a409e7e8`.

## Implementation

- The N605 Elipsa 2E profile enables `reap_nickel_supplicant`. During hand-back,
  Cobalt removes only the detached supplicant it captured and lets Nickel
  recreate its normal owner.
- When the known MediaTek firmware tool exists but `wlan0` is absent, the Wi-Fi
  backend returns `DenyReason::WifiNeedsNickel`. The user-facing text is
  `Enable Wi-Fi in the Kobo reader, then start Cobalt again.` Cobalt never
  initializes the WMT/MediaTek radio directly.
- Protocol `VERSION` is now 15. The new refusal encodes as `Unsupported` for
  older protocol versions, and responses retain the negotiated app protocol
  version. All accepted historical versions, including update-task version 14,
  remain covered by encode/decode/read paths.

## Build and deployment

The final rebuilt package (after the protocol-publication correction) was
produced with:

```text
cargo run --locked -p kobo-cli -- package --out /tmp/cobalt-wifi-work/results/cobalt-current-beta-0.3.15-KoboRoot.tgz
```

Package result: 27 files, 20,883,843 bytes,
SHA-256 `fbae403e775050621a02ba904289c4df35d78e02623db562debbe36d3e61ab2d`.

Deployment to `root@192.168.1.206` completed with the Kobo CLI. The deployment
archive was 20,882,634 bytes, SHA-256
`4c8a4a7768c80c698ec02f5893bb248e237b12dff07ac75cae9bd1a4c51ad0ee`.
The device reported Cobalt `0.3.15` and 20 installed binaries. The installed
`/mnt/onboard/.adds/cobalt/bin/kobod` SHA-256 was
`d51aca2fc6020b77c4bfa61b03643074ff58453d942693abe25a77adb72effb0`, matching
the local ARMv7 build.

## Current-beta hand-back and soak

The installed build was launched through the existing `start.sh` with the
owner-attended trace and panel-session unlock environment. Nickel stopped while
`kobod --present` owned the panel. Sending SIGTERM exercised normal daemon
cleanup; the device log recorded `session finished, handing the panel back`,
`panel released, restarting the reader`, and `waiting for the reader to feed the
freeze watchdog`.

The final-package trace is
`/mnt/onboard/.adds/cobalt/diagnostics/wifi-handoff-v1-381615430-7511.jsonl`
A [checked-in projection](final-0.3.15-handoff.jsonl) retains lifecycle markers,
timestamps, process counts, association, interface and route status. It omits
network identifiers, process arguments, and unrelated diagnostics.
Its `kobod_exit` marker is at monotonic 49,660 ms. From 49,680 ms through the
trace tail at 648,810 ms (599,150 ms), every one of 581 snapshots reported:

- `nickel_count=1`, `supplicant_count=1`, `dhcp_count=1`, and
  `dbus_process_count=1`;
- `associated=true`, `default_route=true`, `wlan_present=true`, and
  `operstate=up`.

The trace stopped just short of its configured ten-minute post-exit window
because of its bounded capture/file limit. Timestamped passive polls then
covered 18:24:40, 18:25:41, 18:26:42, and 18:27:44 WEST. Every final-package
poll showed Nickel PID 7810, one `wpa_supplicant` PID 7962, one `dhcpcd` PID
1022, `wlan0=up`, and a default route. Together with the trace, this is more
than eleven minutes of source-specific post-hand-back observation.

The trace tuple is a generation tuple, so `N2;S3;D1;B1` means owner generations,
not two Nickel or three supplicant processes. The earlier candidate trace's
`N2;S2` observation therefore showed one process of each role at that sample;
it did not prove exclusive ownership or end-to-end usability. That earlier
candidate evidence is kept separate from this current-beta acceptance.

The user also reported that, after the earlier 0.3.14 hand-back, cycling Wi-Fi
off, waiting five seconds, and on again restored the network list and
reconnection without a reboot, and that Kobo Store worked. This is
user-reported physical usability evidence from the preceding deployment; it is
not a claim that the final 0.3.15 Store screen was remotely observed.

The hardware test used SIGTERM to exercise normal cleanup rather than a physical
launcher UI exit. No direct WMT initialization was attempted. Deployment did
not alter saved Wi-Fi settings; the existing backups under
`/mnt/onboard/.adds/cobalt-test-wifi-90-91` were preserved.

The missing-`wlan0` Settings path in issue #90 was classified by the tested
availability logic but was not physically reproduced. No forced radio
deinitialization or direct WMT initialization was attempted. The installed final
state is stock Nickel with the normal start script,
the original `ForceWifiOn=true` preference retained, no temporary tracer in the
launcher script, and the pre-existing absent auto-update marker preserved.

## Verification

Passing checks included `cargo fmt --all --check`, focused tests for
`kobo-profile` (56), `kobo-protocol` (103), `kobo-hal` (165), `kobod` (169
passed; 1 ignored), settings (28), and store (19), plus warning-denied Clippy for the changed
hal/protocol/store crates. `kobod` Clippy passed with `-A dead_code -D warnings`.
The stricter existing `kobod` `-D warnings` invocation still reports its
unrelated baseline dead-code inventory (127 errors); this is recorded rather
than presented as a clean repository-wide lint.

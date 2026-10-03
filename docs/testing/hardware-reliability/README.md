# First hardware reliability tranche

Based on beta `9715304831eae95566758fd0aa6b8e6fc87ee3ee`. This is a draft
reliability change, not device qualification. All implementations and test
fixtures added here are original; no external implementation was imported.

## Boundaries

- Provisional display selection requires exact qualified device-tree tokens
  (`fsl,imx6sll` / its existing board alias, `fsl,imx6ull`, `mediatek,mt8110`,
  or `mediatek,mt8512`) and the matching framebuffer ID. Generic vendor names,
  legacy i.MX50/i.MX6SL, marketing-only MT8113, contradictory families, and
  framebuffer mismatches are refused. The i.MX6ULL Nia keeps the provisional
  path beta already gave it, because its 4.1.15 EPDC driver takes the same
  72-byte request as the i.MX6SLL readers. Existing measured profiles are
  unchanged. Provisional profiles remain not write-ready.
- Passive Wi-Fi tracing retains the `wakeup_count` field as `not-sampled` and
  never opens the sysfs node. A numeric parsing limit was not a read deadline.
  A FIFO fixture with no writer verifies that sampling does not open it. The
  sample runs in a subprocess that is killed and reaped after five seconds if
  a blocking read regresses, so the regression fails rather than hanging CI.
- Wi-Fi subprocesses have a three-second deadline, nonblocking input/output
  pipes, a 64 KiB output ceiling, and owned-child kill/reap cleanup. Inherited
  output pipes cannot keep the caller waiting indefinitely. Credentials stay
  on stdin and are never logged or placed in process arguments.
- A scan sends one bounded `scan` command and returns the supplicant's current
  results without waiting for the new scan to finish. Requests are answered on
  the session loop, and Settings rescans every five seconds while the Wi-Fi list
  is shown, so waiting would freeze touch and drawing for most of that time. The
  next request reports what this one started. `FAIL-BUSY` means a scan is
  already running and fills the same results, so it is accepted; any other
  reply fails. A disconnected status does not label a remembered SSID
  connected. Disabled interfaces are not raised by scanning.
- The Wi-Fi backend resolves the unique current sysfs wireless marker instead
  of using the hand-back code's cached fallback. Missing, changed, or ambiguous
  interfaces are refused; `mlan0` and other kernel names need no whitelist.
  `network::wireless_link`, used by status and hand-back checks, now detects
  the interface on every call instead of caching its first answer, so a radio
  that appears late as `eth0` is no longer misnamed for the whole session.
- Join configuration is acknowledged one step at a time before selection and
  saving. Failures remove only the newly allocated network. After a selection
  attempt, restoration of the previous current/enabled networks is best-effort;
  a failed/unavailable supplicant can also refuse cleanup. The previous list is
  capped at 32 enabled networks. Each exchange is bounded, but a join plus
  rollback can take longer than a single command deadline. This is not an
  atomic transaction with the firmware's persistent configuration.
- The doctor reports all valid event nodes, EV/KEY/ABS/SW capability bitmaps,
  and available ABS_X/Y and ABS_MT_POSITION_X/Y ranges through read-only query
  ioctls. Separate power/cover nodes and unknown touch names remain evidence,
  not runtime routing choices. No events are consumed and no device is grabbed.
  Missing inventory/open errors are retained; serial/unique IDs are excluded.
  The optional `input_devices` observation field preserves version-1 parsing
  and roundtrips; it does not select profiles or authorize writes. At most 16
  nodes and 128 characters per name or capability line are kept, so the
  largest inventory still fits the 32 KiB observation limit.

No legacy display backend, suspend path, new decoder, profile, authentication
mode, or sandbox fallback is introduced. Read-only discovery on an older kernel
does not establish that its application isolation primitives are available.

## Research and overlapping work

The input and display gaps are consistent with the owner-led
[Mini #180](https://github.com/BandarLabs/Cobalt/issues/180) and
[Sage #204](https://github.com/BandarLabs/Cobalt/issues/204) reports. Their ports
remain with their contributors. The supplied research identifies incompatible
68-byte legacy and 72-byte v2 update requests in Kobo's published kernel
archives at [Kobo-Reader revision
7a762964](https://github.com/kobolabs/Kobo-Reader/tree/7a762964e7fa71ad8e8df50dd1b6eee721a43217/hw).
This change uses that evidence to refuse ambiguous selection; it adds no old ABI.
The repository's existing v2 struct-size/ioctl tests continue to pass.

Inspected open work includes #164 (Elipsa guidance/ownership), #236 (enterprise
Wi-Fi), #65/#118 (power/suspend), and #45 (Nia port). This change leaves their
features alone. #236 edits the same Wi-Fi command/script code and introduces its
own monitor, so merging both will need deliberate reconciliation of deadlines
and acknowledgment handling. Application UI PRs #238–268 are untouched.

## Validation

Rust 1.85.1, host Linux, no physical reader:

- Focused all-feature tests: doctor 4, HAL 189, profile 63, Wi-Fi trace 21 plus
  its binary test 1; all passed.
- ABI tests: 19 passed in the filtered run; the remaining existing PTY test
  initially returned EPERM, then passed when rerun alone. No ABI test was edited
  or weakened to hide that transient failure.
- Device-enabled daemon tests: 25 library and 175 binary tests passed;
  one existing test ignored.
- Strict all-target/all-feature Clippy passed for ABI, HAL, profile, doctor,
  and Wi-Fi trace, with warnings denied. Workspace formatting and diff whitespace
  checks passed. This is not a full-workspace test/lint claim.
- Static ARMv7 musl build passed for doctor, Wi-Fi trace, and device-enabled HAL.
  ELF inspection confirms ARM EABI5 hard-float executables with no interpreter
  or dynamic dependency section. C dependencies used Zig 0.16.0 with musl headers.

Tests cover conservative ABI refusal, observation compatibility, unknown and
separate input nodes, omitted wakeup handshake, accepted and busy scan replies,
stalled stdin, inherited stdout, excessive output, nonzero exit, child reaping, join
failure ordering, disconnected SSIDs, and changing/ambiguous wireless links.

No attended display, input, radio, hand-back, suspend, or Settings/About photo
was obtained. Real hardware timing remains unverified. Keep this PR draft pending review and attended qualification;
nothing was merged, deployed, or installed on a reader.

## Review follow-up

The FIFO regression now samples in a child process with a five-second deadline;
its parent kills/reaps the child and removes the fixture before asserting. A
local mutation temporarily restored the old blocking `wakeup_count` read. The
test failed at its deadline as intended (8.11 seconds including compilation),
and passed again after restoring production code. No mutation was committed.
HAL's 189 tests and trace's 21 library plus one binary test passed, as did strict
all-target/all-feature Clippy for HAL/trace, formatting, and whitespace checks.
Wi-Fi module documentation now describes unique sysfs discovery and per-exchange
revalidation rather than the hand-back module's cached fallback. These changes
add no hardware qualification or runtime behavior.

## Merge-readiness fixes

- Scan no longer waits up to twelve seconds for a completion event on the
  session loop, and no longer reports `FAIL-BUSY` as "another application
  holds this resource". This also removes the dependency on the firmware's
  interactive `wpa_cli` printing scan events, which had not been seen on a
  reader. The interactive monitor in `wifi_process` is gone; the tool runner
  now returns the reply together with its exit status, because `wpa_cli`
  builds differ on whether `FAIL-BUSY` exits non-zero.
- `network::wireless_link` re-detects on every call.
- `fsl,imx6ull` is accepted with `mxc_epdc_fb`, restoring the Nia's beta
  behaviour without readmitting i.MX6SL or i.MX50.
- Input inventory is bounded so a full doctor observation always parses.

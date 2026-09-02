# N365 same-process Nickel pause/resume candidate

This is an attended candidate, not a proven fix. Do not run it on a device
that still has an extra or uninterruptible WMT launcher. A clean reboot is a
precondition, and the runtime refuses the candidate unless it sees exactly one
healthy Nickel, supplicant, DHCP client, dhcpcd D-Bus adapter, and WMT launcher.

The candidate is restricted to the exact Clara BW N365/code 391 profile on
firmware 4.45.23697 and kernel 4.9.77. Every other profile, and every session
without the explicit unlock, retains the current stop/restart behavior.

## Physical evidence

The completed official-0.3.3 baseline trace has SHA-256
`2d31a74f3d7ee65c47e6273163da1a9d0a742eeb1294e43a45a16f52eed838d2`.
It ended cleanly at 703.2 seconds with 798 valid lines.

Privacy-safe timeline:

- At 0.01s the device had one Nickel N1, supplicant S1, DHCP D1,
  dhcpcd-dbus B1 and WMT launcher W1. Association, private-prefix address,
  default route, resolver and control-socket inode 5159 were present.
- Nickel N1 was stopped at 0.93s and absent by 1.04s. S1/D1/B1/W1, the route,
  resolver, DHCP artifact identities and probes remained healthy throughout
  the 90-second session.
- Restart was requested at 90.6s. Nickel N2 appeared at 90.7s and remained
  through the trace.
- A child Nickel N3, parented by N2, existed from 93.69s through 94.75s and was
  gone by 94.98s.
- WMT launcher W2 appeared at 95.98s and remained beside boot-original W1
  through 703.07s. The post-run process observation found W2 permanently
  uninterruptible and repeatedly attempting WMT-open SDIO recovery.
- Boot supplicant S1 remained through 97.84s, when carrier and operstate fell.
  S2 appeared at 98.05s, S1 was gone, and the control socket changed from inode
  5159 to 9543.
- The default route and resolver disappeared at 98.05s. Association reported
  scanning at 98.65s. Carrier returned at 99.54s, association and route at
  99.74s, resolver at 99.96s, and the private-prefix IPv4 category at 100.78s.
- DHCP D1, dhcpcd-dbus B1, its D-Bus owner identity, and all observed DHCP
  pid/socket/lease metadata remained unchanged through 703.07s. This is a
  mixed S2/D1/B1 generation, not a complete firmware-network restart.
- Every sampled gateway, DNS and HTTPS probe succeeded: gateway 9–22ms, DNS
  12–225ms, HTTPS 139–712ms. Their 30-second cadence did not directly sample
  the approximately 2.7-second hand-back outage.
- A new sanitized WMT timeout event was observed across 101.73–101.83s, after
  route recovery. Kernel evidence is semantically deduplicated, so the trace
  proves occurrence, not repetition count.
- `kobod` exited at 103.2s. N2/S2/D1/B1/W1+W2 and socket inode 9543 persisted
  for the remaining ten-minute soak. There was no new panic/reset marker,
  pstore or last-kmsg evidence; initial watchdog/reset categories came from
  the pre-existing kernel ring. The clean deadline rules out tracer failure.

The first causal divergence is therefore process creation by restarted
Nickel, not the later route outage: N2 creates transient N3, permanent W2, and
replacement S2. The route recovers while competing WMT generations remain.

## Candidate ownership model

With the unlock below, Cobalt:

1. exact-matches the live N365 profile and verifies a clean single-generation
   network/WMT preflight;
2. records Nickel PID and `/proc` starttime in the recovery state;
3. suspends the freeze watchdog and gives the SoC watchdog slack;
4. sends `SIGSTOP` only to that exact Nickel process;
5. leaves wpa_supplicant, dhcpcd, dhcpcd-dbus, WMT, wlan0, address, route,
   resolver and leases untouched;
   Wi-Fi control, Bluetooth control and audio backends are withheld for the
   whole pause session, while ordinary HTTPS networking remains available;
6. releases touch and framebuffer ownership before sending `SIGCONT`;
7. resumes only if PID, executable, starttime and stopped state still match;
8. waits for the continued reader's sustained watchdog heartbeat before
   restoring freeze and SoC watchdog protection.

The detached recovery watchdog uses the saved PID/starttime. It sends
`SIGCONT` only to the exact paused process. Missing, reused, uninterruptible or
otherwise ambiguous state is retained for inspection and requires a manual
reboot instead of starting another Nickel. Neither the ordinary timed reboot
guard nor a remote forced reboot has been physically proven reliable on this
reader, so the pause/resume path never treats a scripted reboot as recovery.
Other profiles continue using the existing recovery path.

## Build

```sh
export CARGO_TARGET_DIR="/Volumes/Untitled 1/cobalt-targets/wifi-n365-pause-resume"
cargo run -p kobo-cli --features device-write -- \
  package --out "$CARGO_TARGET_DIR/KoboRoot-wifi-n365-pause-resume.tgz"
```

## Attended physical acceptance

Do not perform this procedure until review is complete.

1. Keep the rollback package on the host.
2. Manually reboot the reader. Do not start Cobalt if more than one WMT
   launcher, supplicant, DHCP client or Nickel process remains.
3. Install the candidate package through USB. If the normal installation flow
   does not restart the reader, perform another manual reboot.
4. Confirm stock Nickel has working Wi-Fi and allow it to become idle.
5. Open an interactive shell:

   ```sh
   "$CARGO_TARGET_DIR/debug/kobo" shell --device READER_IP
   ```

6. At the reader prompt, run one 90-second network-only session:

   ```sh
   KOBO_N365_PAUSE_RESUME=OWNER_ATTENDED_N365_NICKEL_PAUSE_RESUME \
   KOBO_WIFI_HANDOFF_TRACE=OWNER_ATTENDED_N365_WIFI_HANDOFF_TRACE \
   KOBO_WIFI_HANDOFF_ACTIVE_PROBES=OWNER_ATTENDED_BOUNDED_WIFI_PROBES \
   KOBO_SESSION_SECONDS=90 \
     /mnt/onboard/.adds/cobalt/start.sh
   ```

7. Do not scan, toggle Wi-Fi, suspend, open the network screen, or start
   another session during the passive ten-minute soak.
8. Copy the exact trace basename printed when the session starts, then retrieve
   that session only:

   ```sh
   "$CARGO_TARGET_DIR/debug/kobo" wifi-trace retrieve --device READER_IP \
     --trace wifi-handoff-v1-MONOTONIC-PID.jsonl \
     --out wifi-n365-pause-resume.jsonl
   ```

   Retrieval deliberately has no “latest file” mode because an earlier
   attended helper may still be finishing its soak.

Acceptance requires one unchanged Nickel PID/starttime/generation, with state
`stopped` only between the pause checkpoints; unchanged S/D/B/W generations,
control-socket inode, DHCP identities, route and resolver; no carrier loss;
successful probes; `nickel_resumed`; sustained watchdog feeding; and a clean
trace deadline. Any extra WMT/supplicant, identity change, reboot, missing end
marker, failed probe, route loss or recovery fallback rejects the candidate.

To roll back, first manually reboot, then install the separately built
unmodified `origin/beta` rollback archive through USB. Do not use an in-session
or remotely forced rollback.

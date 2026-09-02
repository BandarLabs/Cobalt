# Official 0.3.3 baseline Wi-Fi handoff trace

This attended build adds only passive diagnostics to the official Cobalt
0.3.3 beta source at commit
`2b08e793e13d36547fb72841df846774ca69798d`. The official release archive at
tag `beta-v0.3.3` contains a `kobod` whose SHA-256 is
`945b60b6999cafacbbbb7684868969a10291b40f155f50750276cd57178e421c`.

This branch deliberately retains that commit's hand-back behavior. In
particular, it does not add the later automatic reconnect, captured daemon
restart, association recovery, or healthy-route gate from PR #94.

## Build and install

```sh
export CARGO_TARGET_DIR="/Volumes/Untitled 1/cobalt-targets/wifi-handoff-baseline-trace"
cargo run -p kobo-cli --features device-write -- \
  package --out "$CARGO_TARGET_DIR/KoboRoot-wifi-handoff-baseline-trace.tgz"
```

Install through the ordinary USB `KoboRoot.tgz` flow, or deploy to an
authorized development reader:

```sh
"$CARGO_TARGET_DIR/debug/kobo" deploy --device READER_IP \
  --package "$CARGO_TARGET_DIR/KoboRoot-wifi-handoff-baseline-trace.tgz"
```

Normal `start.sh` sessions do not create a trace file or helper process.

## Run

Start from a clean reboot, verify Nickel is online, and open an interactive
shell:

```sh
"$CARGO_TARGET_DIR/debug/kobo" shell --device READER_IP
```

At the device prompt:

```sh
KOBO_WIFI_HANDOFF_BASELINE_TRACE=OWNER_ATTENDED_OFFICIAL_0_3_3_WIFI_HANDOFF_TRACE \
KOBO_WIFI_HANDOFF_ACTIVE_PROBES=OWNER_ATTENDED_BOUNDED_WIFI_PROBES \
  /mnt/onboard/.adds/cobalt/start.sh
```

Exit Cobalt within two minutes. Do not toggle Wi-Fi, open Nickel's network
screen, deliberately suspend, or start another Cobalt session during the
ten-minute passive observation. If connectivity disappears, note the time and
leave the device untouched until the trace ends.

The trace uses the official baseline's successful Nickel process start as its
hand-back anchor; this is not a claim that association or routing is healthy.
An absent route, duplicate WMT launchers, or another unhealthy first snapshot
is reported as an immediate baseline hand-back divergence.

## Retrieve

```sh
"$CARGO_TARGET_DIR/debug/kobo" wifi-trace retrieve --device READER_IP \
  --out wifi-handoff-baseline-v1.jsonl
```

If the reader is unreachable, wait for the bounded trace to finish, then copy
the newest file over USB from:

```text
.adds/cobalt/diagnostics/wifi-handoff-baseline-v1-*.jsonl
```

Summarize a copied trace:

```sh
"$CARGO_TARGET_DIR/debug/kobo" wifi-trace summarize \
  wifi-handoff-baseline-v1.jsonl
```

The versioned header identifies this as
`official_beta_0.3.3_pre_pr94`. Process and file equality identities use a
random per-trace HMAC key that is never serialized. Slow read-only firmware
commands cannot delay 200 ms core process/link/route snapshots. The Linux
boot-time ceiling is exactly 14 minutes 30 seconds, includes suspend, reserves
space for a clean end marker, and retains at most four baseline traces.

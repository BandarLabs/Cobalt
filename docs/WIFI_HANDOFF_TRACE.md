# Attended Wi-Fi handoff trace

This is a diagnostic harness for the existing automatic reconnect path. It is
not a Wi-Fi fix. The tracer is passive with respect to network ownership: it
does not change an interface, send a signal, call WMT, control `dhcpcd`, or use
a mutating `wpa_cli` command.

The first attended target is the Clara BW N365 (device code 391), firmware
4.45.23697, kernel 4.9.77, with the MT8110 radio. Do not use the procedure as an
unattended or routine installed-device mode.

## Build and install the attended branch

Keep all Cargo output on the external volume:

```sh
export CARGO_TARGET_DIR="/Volumes/Untitled 1/cobalt-targets/wifi-handoff-phase0"
cargo run -p kobo-cli --features device-write -- \
  package --out "$CARGO_TARGET_DIR/KoboRoot-wifi-handoff-phase0.tgz"
```

Install that archive through the usual USB `KoboRoot.tgz` flow, or deploy it to
an already-authorized development reader:

```sh
"$CARGO_TARGET_DIR/debug/kobo" deploy --device READER_IP \
  --package "$CARGO_TARGET_DIR/KoboRoot-wifi-handoff-phase0.tgz"
```

Normal `start.sh` launches do not trace and retain their existing behavior.

## Run one attended trace

Start from a clean reboot with stock Nickel, `wpa_supplicant`, `dhcpcd`, and
`dhcpcd-dbus` running. Confirm the reader is online, open an interactive shell
(the command form of `kobo shell` has a two-minute ceiling), then run the
attended command at the device prompt:

```sh
"$CARGO_TARGET_DIR/debug/kobo" shell --device READER_IP
```

```sh
KOBO_WIFI_HANDOFF_TRACE=OWNER_ATTENDED_N365_WIFI_HANDOFF_TRACE \
KOBO_WIFI_HANDOFF_ACTIVE_PROBES=OWNER_ATTENDED_BOUNDED_WIFI_PROBES \
  /mnt/onboard/.adds/cobalt/start.sh
```

The second unlock enables bounded gateway, DNS, and direct HTTPS reachability
checks. Omit it for a wholly passive trace; all ownership and state sampling
still runs.

Exit Cobalt within two minutes so the 15-minute hard runtime can include the
full ten-minute post-Nickel soak. After Nickel reappears, leave the device
alone for at least ten minutes.

**Do not toggle Wi-Fi during that ten-minute passive soak.** Do not open
Nickel's Wi-Fi screen, suspend the device deliberately, run another Cobalt
session, or start/stop networking daemons. If connectivity disappears, record
the wall-clock time and continue waiting. The trace ends by itself.

## Retrieve and summarize

After the helper has stopped, retrieve the latest trace over SSH:

```sh
"$CARGO_TARGET_DIR/debug/kobo" wifi-trace retrieve --device READER_IP \
  --out wifi-handoff-v1.jsonl
```

If the device is unreachable, wait until the passive window has completed,
then connect USB and copy the newest file from:

```text
.adds/cobalt/diagnostics/wifi-handoff-v1-*.jsonl
```

Summarize a USB-retrieved file without printing owner data:

```sh
"$CARGO_TARGET_DIR/debug/kobo" wifi-trace summarize wifi-handoff-v1.jsonl
```

The summary lists generation-tuple transitions and the first divergence after
the current reconnect gate was accepted. Keep the original JSONL file: an
unexpected reboot may leave a valid final synced line followed by one
interrupted line, which the parser deliberately ignores.

## Trace lifetime and contents

The helper starts before Nickel is stopped, writes a durable baseline, samples
at 200 ms through transition windows, then every five seconds during the soak.
It exits ten minutes after the later of observing the restarted Nickel PID or
`kobod` exiting, bounded by 15 minutes total and a 4 MiB file ceiling. At most
four traces are retained.

Records carry trace version 1 and monotonic milliseconds. They contain process
PID, PPID, executable identity, command-line SHA-256, and `/proc` starttime;
supplicant/DHCP/D-Bus generations; socket/file inode metadata; association,
address, route, resolver, driver, watchdog, power, and sanitized kernel
evidence. Writes are flushed immediately and synced at bounded intervals and
at lifecycle checkpoints.

SSID, BSSID, MAC addresses, serial numbers, credentials, private addresses and
gateways, DNS names from the device, lease contents, Bluetooth names, and
owner-supplied domains are never written. DHCP path and content identity is
represented by checksums rather than contents.

The harness cannot prove the cause until it is run on physical hardware. A
trace without a clean end marker means it was still running, was interrupted,
or the device rebooted.

# Combined Clara BW validation

Run this after the foundation, catalog and companion PRs are ready. The owner has a Clara BW, hardware code 391. Nothing in this document records new hardware results. Current implementation evidence is from host tests and simulation.

The harness uses Cobalt's existing doctor, session, touch-probe, bounded display smoke, guard, logs and screenshot commands. It does not install a package, change sleep settings, set a wake lock, copy a reference project's implementation or bypass the HAL's exact profile/firmware gates. Prepare the final reviewed device build and application fixtures through the existing package/install facilities first. Record all three PR heads and the installed package digest with the run.

## Prepare and run

Build the CLI with the device operations enabled:

```sh
cargo build -p kobo-cli --features device-write
```

Discover the reader using the existing `kobo devices` command. Use its current address; do not reuse an old DHCP address. A plan requires no connected reader:

```sh
python3 scripts/quality/clara-bw-check.py --device reader.local \
  --phase display --out /tmp/clara-display-plan
```

When the reader and reviewed build are ready, execute into a **new** evidence directory:

```sh
python3 scripts/quality/clara-bw-check.py --device reader.local \
  --phase baseline --out /tmp/clara-baseline --execute
python3 scripts/quality/clara-bw-check.py --device reader.local \
  --phase display --out /tmp/clara-display --execute
python3 scripts/quality/clara-bw-check.py --device reader.local \
  --phase recovery --out /tmp/clara-recovery --execute
```

`display` runs the existing reversible GC16 patch, whole-screen restore, DU patch and submit/wait timing checks. `recovery` runs the existing deliberate failed-child guardian test. The exact confirmation arguments belong to those fixed operations; the device still performs its existing hardware/firmware checks. Keep the reader attended during these phases. A failed command stops the phase and records its transcript; it is never retried as if nothing happened.

Every executed phase first requires a real doctor observation with hardware code 391, 1072 × 1448 geometry, firmware and kernel. The harness's coarse identity check is not a replacement for the HAL's complete write gate. Synthetic observations are rejected before display or settings operations. All records are local, in a private directory; review screenshots and transcripts before publishing evidence.

## Physical touch

For each point, run the touch phase with `--corner top-left`, `top-right`, `bottom-left`, `bottom-right` or `centre`, using a separate output directory. Place the finger approximately 10 mm in from each edge; use the same marked physical positions in baseline and final runs. For the centre, use the intersection of the screen's midlines. A ruler or a removable paper guide can establish the target without changing firmware.

```sh
python3 scripts/quality/clara-bw-check.py --device reader.local \
  --phase touch --corner top-left --out /tmp/clara-touch-top-left --execute
```

Tap repeatedly during the 30-second device observation window. Allow for the CLI's build/upload delay. The reader continues receiving these real touches because the probe does not grab the input node. Use a harmless stable screen and describe exactly where it was touched. Record the number of intentional taps; compare raw events, mapped coordinates, release/cancel events and missing/duplicate contacts. Repeat a short drag and a hold at the centre in separately labelled runs. Repeat the app's own landscape route after confirming its reported pose.

No synthetic `kobo tap` command is used for calibration. Synthetic taps are useful for the later 43-app route checks, but they cannot verify the physical controller mapping. A successful probe with zero events is not a passed touch test. Preserve the raw trace and target placement photograph; record observed spread and offset in pixels and millimetres before proposing a transform change.

## Sleep, wake and recovery matrix

Run the catalog's combined fixture routes before and after each cycle. Verify the same selected record, reading position and pending save state, not just that an app launches. Use the existing automation facility to drive app controls; physical cover, power-button and USB transitions remain explicit operator events until an actuator is available.

| Cycle | Required evidence |
| --- | --- |
| Power-button sleep and wake | Suspend counter increases, uptime does not reset, app position returns, input and frontlight respond |
| Cover close/open, including two quick closes | One final foreground transition; no duplicate request or save; final cover state matches the sensor |
| Wake while suspend is entering | Recorded event ordering, no deadlock, one resumed task generation |
| Ten repeated sleep/wake cycles | No growing worker count, lost input, stuck wake ownership or position regression |
| Sleep with pending save/download | Save acknowledgement or explicit retained failure; cancelled/paused transfer resumes once |
| Scheduled wake | Recorded scheduled reason, bounded useful work, return to sleep; no repeated background fetch |
| USB attached/detached, charging and unplugged | Correct attachment/charging state; no unattended repeated wake or missing reader handback |
| Ordinary app exit and supervised failed child | Stock reader display, touch and settings restored; guardian verification and trace retained |
| Runtime forced exit | Existing watchdog/handback facility restores stock reader; compare frontlight/warmth and settings against baseline |

Some cycles depend on the remaining platform power work. Do not mark them passed using the single-app simulator or a guardian-only check.

Capture `baseline` immediately before the sleep experiment. After physically waking the same reader, run:

```sh
python3 scripts/quality/clara-bw-check.py --device reader.local \
  --phase after-wake --baseline /tmp/clara-baseline/session-before.stdout \
  --out /tmp/clara-after-wake --execute
```

The harness compares wake-lock and owner configuration values, observed frontlight/warmth controls, device boot identity, suspend count and uptime. Missing values, changed settings, no observed suspend, a changed boot identity or an uptime reset fail the comparison. It never calls a returning Wi-Fi connection proof of resume, and it does not treat uptime alone as awake time. Preserve differences for diagnosis rather than writing guessed defaults back to the reader. If the operator intentionally changes a preference, start a new baseline.

For unexpected interruption, use the existing `kobo stop --device ADDRESS` handback command when reachable. If the reader is unresponsive, use the documented physical restart path in `docs/INSTALL.md`; do not disable profile checks or add a boot takeover. Record recovery duration and whether owner intervention was needed. Do not restore someone else's prior development configuration backup without first establishing ownership of that change.

## Latency, residue and power protocol

Keep firmware, profile, app fixture, text sizes, orientation, battery range, charger state, lighting, frontlight/warmth and room temperature consistent between the beta baseline and final build. Record differences when that is impossible. Separate a cold launch from a warm page turn. Keep originals and raw observations, not just an average.

1. **Driver time:** Retain host command duration, but label it as including build, upload and network time. It is not device touch latency.
2. **Submit/wait:** Use the device's existing timing smoke output. Retain requested intent, submitted/translated waveform, submission and completion wait durations and restoration result. Report sample count, median, p95 and maximum separately for GC16 and DU. Do not replace these with browser timestamps.
3. **Visible feedback:** Film the physical target at a known frame rate, with the tap/trigger visible. Count frames from contact to first feedback and to settled content. Report the frame-rate uncertainty. A framebuffer recording verifies desired pixels, not the time ink physically moves.
4. **Residue:** Photograph the same page sequence before and after a cleaning refresh with exposure, focus, distance and lighting fixed. Include a white reference and repeat text-to-image and image-to-text transitions. Retain crops and the full frame. Assess old-edge contrast against the clean reference. The simulator's retained one-sixteenth residue is not a measured coefficient.
5. **Resume:** Record power/cover/USB trigger, device suspend/wake evidence, first readable frame and first successful input. Record failed or duplicate network work separately from paint latency.
6. **Power:** Use an external current logger or a verified hardware charge counter when available. Record the measurement method and sampling interval. Compare idle, reading, download, sleep and scheduled-wake windows long enough to include complete cycles. Battery percentage alone is too coarse to claim a power saving.

Use at least 30 repeatable interaction samples per comparable case and report every failed run. Treat this count as a test protocol, not a statistical guarantee. Define acceptable regression bounds from the measured baseline before accepting the final run. Never tune the simulator from a single sample, an unlabelled screenshot or a different reader's measurements. Keep color/inversion calibration unverified on this grayscale Clara BW.

## Evidence format and acceptance

Each `run.json` uses `cobalt.hardware-quality-run`, version 1. It records source revision/dirty state (including untracked files), CLI digest, intended profile, phase and physical point, exact argv, step state, exit code, host duration, transcript paths/lengths/digests, raw compatibility observations, before/after settings and screenshot digest. Each raw doctor observation retains its own origin and version. Missing phases and observations remain pending.

`command-checks-completed` means only that the selected commands and automatic comparisons succeeded. `physicalAssessment` remains `pending` and `calibrationApplied` remains `false`. Attach operator observations, photographs, frame-rate metadata, timing samples and current traces to assess the physical criteria. Do not edit these fields to imply that merely creating a plan performed a measurement.

Use a comparison row for each firmware/profile/package combination: three PR heads, package digest, CLI digest, doctor observation digest, phase reports, app-route results, recovery result, physical assessment and unresolved differences. Unknown firmware and hardware remain unsupported unless the existing complete Cobalt gates accept them. This report does not grant new device support.

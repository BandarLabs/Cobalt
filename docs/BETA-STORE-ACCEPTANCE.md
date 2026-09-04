# Beta Store acceptance

`kobo beta-store-smoke` is the release acceptance harness for Store
applications. It uses the runtime's catalog verification, bundle parser,
install planner, atomic package transaction, removal recovery, and application
resolver. It does not implement a second signature or installation path.

The command has only two modes:

- `--fixture` creates an isolated mock device under the evidence directory.
- `--beta-catalog` accepts only Cobalt's fixed `app-catalog-beta` URL and also
  requires an explicitly identified, owner-attended device.

Stable and Main catalog URLs are refused. Neither mode changes the configured
device update channel.

## Local acceptance

Run this before involving a reader:

```sh
cargo run --locked -p kobo-cli -- beta-store-smoke \
  --app beta-smoke-fixture \
  --fixture tools/fixtures/beta-store-smoke \
  --out target/beta-store-smoke-evidence
```

The deterministic public fixture key is test-only. The harness uses it with
the production catalog, signature, manifest, bundle, install, recovery, and
remove implementations. It covers:

- clean install and an already-installed no-op;
- baseline-to-target update and downgrade refusal;
- remove confirmation and state-preserving reinstall;
- interrupted package bytes, checksum failure, signature failure, malformed
  signed catalog, and malformed catalog-bound package;
- conflicting harness commands;
- interrupted activation recovery from the verified previous directory;
- launch failure followed by known-good recovery; and
- exact preservation digests for unrelated apps, all state, target app state,
  secrets, owner TLS trust roots, data, and a network-ownership sentinel.

Use `--dry-run` to verify and export the fixture artifacts without creating
mock device state.

The evidence directory contains `report.json`, exact catalog/signature/package
bytes, PNG screenshots, sanitized logs, the mock device filesystem, and the
mandatory `marketing/` capture. Report digests reveal no state or secret
values.

## Marketing capture

Marketing capture runs only after functional acceptance. Its route is a
strict, non-shell file with exactly one public title, one bounded interaction,
and one public result:

```text
cobalt-beta-marketing-route 1
privacy public-demo-no-owner-data
title Public app title
interaction Open sample|536|724
result Public sample result
timing 1200|2200
```

Labels are bounded and reject credential, account, notification, network, and
private-data terms. Coordinates must be inside the confirmed panel. Use only a
reviewed public/demo route that does not open owner content or require a
credential. The explicit `privacy` line is mandatory. The recorder starts
after the app owns the panel and stops before Nickel returns, so Nickel and its
notifications are never marketing frames.

On a device, the harness invokes the existing read-only equivalent of:

```sh
kobo record --device DEVICE_IP --seconds N --fps 4 --out EVIDENCE/marketing
```

It retains consecutive `frame-NNNN.png` files and their real timestamps,
validates dimensions, strict timestamp order, title/interaction/result frame
coverage, duration, and size, then uses local `ffmpeg`/`ffprobe` to produce and
validate:

- `marketing.mp4` (H.264);
- `marketing.webm` (VP9); and
- `marketing.gif` (48-colour differential palette).

Every frame, route, timing/concat file, command, video, and GIF is recorded in
`report.json` with byte size and SHA-256. `marketing/ffmpeg-command.sh` is the
exact reproducible encoding command.

If `ffmpeg`, `ffprobe`, or a required codec is unavailable, the numbered PNGs,
timings, route, concat input, and command remain. `marketing.complete` is
`false`, the reason is recorded, and the smoke command fails instead of
silently accepting incomplete marketing evidence.

## PR to physical proof

1. Open the application PR against `beta`; its app version must increase when
   release inputs change.
2. Merge after CI. `Publish apps` builds isolated ARM artifacts, signs the
   canonical catalog and packages, and publishes only `app-catalog-beta`.
3. Wait for that workflow to finish. Read
   `cobalt-app-catalog-provenance.json` from the beta release and confirm its
   `source_sha` is the merged beta commit.
4. Run the local fixture command above and retain its report.
5. Update the attended reader to the matching Beta Cobalt platform containing
   this harness and select **Beta updates** in Cobalt Settings before the
   acceptance window. The harness verifies that existing choice and never
   changes it. Do not change Wi-Fi or sleep settings for the run.
6. Read the exact values from the device before typing the confirmation:

   ```sh
   kobo shell --device DEVICE_IP \
     "/mnt/onboard/.adds/cobalt/bin/kobod --beta-store identity"
   ```

7. While the owner is present, run:

   ```sh
   cargo run --locked -p kobo-cli --features device-write -- beta-store-smoke \
     --app APP_ID \
     --beta-catalog \
       https://github.com/BandarLabs/Cobalt/releases/download/app-catalog-beta/cobalt-app-catalog.json \
     --device DEVICE_IP \
     --expected-profile PROFILE_ID \
     --expected-cobalt COBALT_VERSION \
     --expected-firmware FIRMWARE_VERSION \
     --marketing-route routes/APP_ID-marketing.txt \
     --confirm PROFILE_ID/Cobalt-COBALT_VERSION/FIRMWARE_VERSION \
     --out evidence/APP_ID-BETA_COMMIT
   ```

The attended run verifies the production release key, Beta update-channel
selection, catalog, package, manifest, installed binary, runtime protocol,
exact device profile, firmware, and Cobalt version. It captures Nickel before
the run, the launched app, Nickel after hand-back, sanitized runtime logs,
remove/reinstall results, and before/after preservation digests. Target-owned
state/data is compared immediately before removal and after reinstall;
unrelated state/data is held constant across the whole run. The panel session
is limited to 45 seconds. A second, bounded session records only the reviewed
public marketing route after every functional check passes.
After refresh, the device reports the SHA-256 of its verified cached catalog
and signature; both must match the exact bytes archived by the host. Runtime
logs are read only from the byte offset recorded immediately before this
acceptance panel session, so earlier owner sessions cannot enter the evidence.
Every failure first asks the existing bounded panel controller to restore
Nickel; if that cannot complete, it requests a clean reboot. The command never
toggles Wi-Fi and never writes network ownership configuration.

The device must already be awake, connected, and explicitly confirmed. An
offline or incompatible reader is refused rather than reconfigured. A
pre-existing Cobalt panel session is also refused; the harness never stops a
session it did not start.

Signed Store binaries are canonical at
`.adds/cobalt/apps/APP_ID/bin/kobo-APP_ID`; `.adds/cobalt/bin/` contains
platform-owned built-ins. The matching Beta host command asks `kobod` to
resolve that verified app-scoped path. An older `kobo present` that checks only
the platform `bin/` directory will incorrectly report a Store app as not
installed and must be updated with the Beta platform before acceptance.

An app directory containing `manifest.json` but missing its app-scoped binary,
signature, or other verified package component is a corrupt installation, not
an absent app. Status, Store refresh/install planning, and Beta smoke fail
closed on that condition. The attended run must stop before mutation; remove
the corrupt package explicitly and reinstall it through the signed Beta
catalog before collecting acceptance evidence.

## Promotion and retention

Review `report.json` and screenshots, then record the beta source commit and
catalog SHA-256 in the application PR or release record. Retain both local and
attended evidence at least until the exact beta catalog is promoted to Stable
and for the project's normal release-audit retention period. If CI storage is
used, upload the evidence directory as an ordinary artifact; do not add device
credentials, public secrets, hardware assumptions, or an unattended physical
job to a workflow.

After acceptance, merge `beta` into `main` without rebuilding or squashing
away the tested commit. Run **Promote tested beta apps** with the recorded full
beta commit. The protected Stable environment supplies approval; the workflow
finds the archived catalog digest from provenance and copies the exact tested
package bytes before switching Stable.

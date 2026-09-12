# Elipsa 2E Wi-Fi implementation evidence

The implementation is limited to the following source areas:

- `crates/kobo-profile/src/lib.rs`: measured N605 profile ownership cleanup.
- `crates/kobo-hal/src/wifi.rs`: passive availability classification with no
  radio initialization.
- `crates/kobo-protocol/src/lib.rs`: actionable refusal reason, protocol 15,
  backward refusal encoding, and negotiated-version responses.
- `crates/kobod/src/device.rs`: Wi-Fi refusal mapping and per-session protocol
  response version, including unsolicited touch frames.
- `examples/store/src/main.rs`: exhaustive rendering of the new refusal reason.
- `docs/DEVICES.md`: N605 operational guidance and safety boundary.

The tested package is Cobalt `0.3.15`; package SHA-256 is
`fbae403e775050621a02ba904289c4df35d78e02623db562debbe36d3e61ab2d`.
`tools/protocol-minimums.json` maps protocol 15 to Cobalt `0.3.15`, so Store
publication cannot advertise a protocol-15 app against the older 0.3.14 floor.
The current-beta acceptance trace and its interpretation are recorded in
`current-beta-acceptance.md` in this directory.

An external Copilot read-only review identified that unsolicited touch frames still used the newest protocol rather
than the negotiated app version. A focused external correction threaded the
session version through `deliver_touch`, added legacy TextHold and Action
coverage, and passed the final kobod tests/build and app-version guard.

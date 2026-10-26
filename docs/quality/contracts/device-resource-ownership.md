# Contract: device resource ownership

Every hardware resource has exactly one owner at any moment, and ownership
transfer is measured, not assumed (issues #47, #91, #189, #190).

## Per-resource record

For panel, touch, Wi-Fi, Bluetooth, audio, USB, power/watchdog and PTY,
each device/firmware profile records:

- owner before Cobalt entry;
- process and device nodes that establish ownership;
- whether Cobalt borrows, stops, reaps or initializes the resource;
- prerequisites and safe idempotent setup;
- bounded hand-back steps with a timeout each;
- the observation that proves Nickel resumed successfully;
- firmware-specific exceptions;
- failure rollback and diagnostic capture.

## Rules

- Probe, never infer: `/dev/pts`, fonts, interface names, radio backends
  and adapters are probed at startup; absence reports
  `temporarily-unavailable` or `unsupported` with evidence
  (see [capability availability](capability-availability.md)).
- After Wi-Fi hand-back there is exactly one supplicant owning the
  interface; uptime does not reset; the NickelMenu failsafe is not
  triggered.
- Ownership state transitions are a simulator-testable state machine in
  `crates/kobo-handoff`; physical evidence is attached per admitted
  device/firmware and stays a promotion gate.

## Tests

- Simulator: owner transition state machine, including interrupted
  hand-back and double-owner detection.
- Physical (promotion gate, simulator cannot prove): process/interface/
  route/control-socket snapshots before entry, during Cobalt, after
  Nickel recovery; one-supplicant assertion; uptime continuity.
  UNVERIFIED without device evidence.
- Interaction with the in-flight Wi-Fi reliability work: this contract
  consumes its measured logic; it does not reimplement it.

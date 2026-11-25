# Contract: capability availability

Replaces boolean capability checks with a reasoned state, so a device that
needs owner setup is never reported as unsupported hardware (issue #90) and
a missing prerequisite is never inferred from profile identity (issue #55).

## Enum

Every capability report is one of:

- `available` - usable now.
- `owner-setup-required` - usable after a named owner action (enable
  Wi-Fi in Nickel, pair headphones); the report names the action.
- `temporarily-unavailable` - transient state (radio off, resource owned
  by Nickel right now); the report says what would change it.
- `denied` - the user or policy refused it; the report names the
  permission and where to change it.
- `unsupported` - the hardware/firmware genuinely lacks it; the report
  carries the probe evidence, not an assumption.

Availability is established by probing (device nodes, interfaces, adapters,
fonts, `/dev/pts`), never inferred from the device profile alone. Profiles
declare expectations; probes decide.

## SDK/runtime rules

- Apps ask for a capability and receive the enum plus a human-readable
  reason; boolean helpers are derived views, not the source of truth.
- Undeclared requests are denied at the runtime boundary.
- Revocation takes effect on the next request without reinstall.
- Optional-capability denial leaves the rest of the app usable; required
  denial blocks only the dependent feature and provides recovery guidance.
- Secret values never enter app memory, logs, screenshots or exported
  state; apps hold named-secret references only.

## Tests

- Each availability source has unit tests per enum state.
- Simulator scenarios: unsupported, denied, revoked, owner-setup-required.
- Existing binary states (e.g. audio `Availability`) migrate to the enum.

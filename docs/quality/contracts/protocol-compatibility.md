# Contract: protocol compatibility

Base: `crates/kobo-protocol/src/lib.rs` at beta head `4bb0d0e`.

## Versions

- Current protocol: **14** (`VERSION`).
- Oldest supported: **11** (`LEGACY_VERSION`).
- Introduction markers: 12 Folio (`FOLIO_VERSION`), 13 selected grid
  (`SELECTED_GRID_VERSION`), 14 server account / update task
  (`SERVER_ACCOUNT_VERSION`, `UPDATE_TASK_VERSION`).
- Anything below 11 and anything above 14 is refused with
  `UnsupportedVersion` before the app is shown as launchable.

## Session state rule

The version negotiated at Welcome is stored once, immutably, for the life
of the session. Every later encoder - service replies, cancellation, Back,
power and error frames - reads the session version. Neither side can change
protocol after Welcome. Beta head `4bb0d0e` lands the runtime half of this
rule ("Keep talking to an app on the protocol it greeted with", #192); the
contract freezes it and the matrix below keeps it executable.

## Compatibility matrix (required release tests)

| Runtime | App protocol | Expected result |
| --- | --- | --- |
| current | current (14) | full behavior |
| current | 13 | negotiated legacy behavior; no v14 frame emitted |
| current | 12 | negotiated legacy behavior; no v13+ frame emitted |
| current | 11 (oldest) | launch, first screen, Back, state save, clean exit |
| current | 15+ / unknown | refuse before app UI with actionable update message |
| current | 10- | refuse with `UnsupportedVersion` |
| previous Stable | app requiring newer runtime | Store refuses install/update and keeps old app |

Fixtures must be immutable: golden wire transcripts per supported version
under `scripts/fixtures/protocol/`, or real previously released app
binaries. Recompiling an old app with the current SDK does not produce a
valid fixture.

## Frame introduction audit

Every frame declares the version that introduced it, its fallback when the
peer is older, and whether omitting it changes safety. Unknown optional
fields are ignored where the format permits; unknown required variants fail
before launch. Removing support for a protocol version requires a
deprecation release, a measured installed-base decision and an explicit
migration path.

## Tests

- `crates/kobo-protocol`: unit tests already refuse `LEGACY_VERSION - 1`
  and `VERSION + 1`; extend to per-version transcript replay.
- `crates/kobod/src/app_link.rs`: session version is carried through every
  encoder (audit test walks the encoder list).
- Release CI lane `contract`: current runtime x every supported protocol
  version against immutable fixtures.

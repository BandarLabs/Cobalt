# Independently authored simulator fixtures

These are software state-machine fixtures, not device captures or hardware
validation. No vendor firmware, DTB, executable, trace, or package is included.
The board identity SHA-256 pins a newline-delimited tuple of existing Cobalt
profile fields: profile ID, serial prefix, numeric device code, one existing
firmware-version string, and kernel-release string. This digest is not a
firmware checksum. Changing any profile field requires reviewing the pin.
The fixture table deliberately covers three profiles with different dimensions
and panel/touch poses. Other supported profiles still run through their
normal simulator path and have no pinned board identity.

`POST /input` with `fixture-tap` replays a synthetic tap at one quarter panel
width and one third panel height. It transforms that coordinate through the
selected profile's pose and Cobalt's real input decoder. No captured event
trace is replayed. The simulator's selected `KOBO_SIM_PROFILE`, not the table,
controls which profile is active.

`POST /panel` accepts `delay 0..5000` milliseconds while idle. This selects a
synthetic panel latency, *not a measured refresh time*. `advance 0..60000`
advances its independent fixture clock without sleeping; while a refresh is
pending input is blocked. `fail` during a pending refresh and then `retry`
exercise invalidation and full-refresh recovery.

In runtime mode, `POST /power-fault` queues a one-shot `permission-veto`,
`alarm-absent`, `alarm-failed`, `immediate-wake`, or `duplicate-wake` before a
sleep request. Duplicate or stale fault admission is refused without ending
the runtime loop. These simulate backend branches only; no actual kernel suspend
or RTC alarm is executed. `POST /wifi-fixture` selects `normal`,
`interface-missing`, or `interface-down`, keeping device permission checks in
front of the synthetic result. The interface names in `boardFixture` are
fixture choices, not measured claims. `POST /shell-fault` queues a one-shot
`helper-missing` or `helper-start-failed` refusal on the next shell open. It
does not launch or stop a real host helper. Do not treat any success here as
proof that a physical reader will behave the same way.

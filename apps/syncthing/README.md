# Sync

Sync is the Settings surface and deterministic configuration model for Cobalt's
planned kobod-owned Syncthing service. It persists the on/off state and
cadence, shows the fixed receive-only `vault`, `frame`, and `books` folders
plus send-only `out`, and makes the zero-battery default visible.

![Sync folders on the Clara BW simulator](screenshots/syncthing-folders.png)

The supervisor model generates a loopback-only REST configuration and refuses
to open a window while disabled. The actual unmodified ARMv7 Syncthing binary,
daemon spawn/stop, API-key storage, scheduled wakes, and platform-update
delivery require kobod/rootfs changes, deliberately outside this Store-only
commit.

## Third-party attribution

[Syncthing](https://github.com/syncthing/syncthing) is MPL-2.0. The shipped
binary's exact upstream version and commit must be recorded in the platform's
`THIRD-PARTY.md` when that runtime integration lands.

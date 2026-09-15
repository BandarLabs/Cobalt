# Windows support and cross-platform verification

This document records what "Windows support" means for Cobalt today: which
paths are verified and by what, and which are explicitly unverified. A path
that is not listed as verified is not claimed to work.

## Verification tiers

- **Verified in CI**: compiled and exercised by an automated job on every
  push to this branch.
- **Verified by emulation**: executed under qemu-arm user-mode emulation.
  This proves logic, syscalls and musl/ARM codegen. It does not prove real
  device kernel behavior.
- **UNVERIFIED**: compiles, but no automated runtime verification exists.
  Use on a real machine may surface defects. This label is removed only by
  running the path on a real target, not by time passing.
- **Unsupported**: no honest implementation exists; the code returns an
  explicit error instead of pretending.

## Windows host (x86_64-pc-windows-msvc, windows-2022 runner)

Verified in CI (native `cargo test --workspace --all-targets`, plus a
built `kobo.exe` artifact that is executed for `--version`):

- The `kobo` CLI build, package, and deploy flows, and every workspace unit
  test that is not Unix-specific.
- The simulator app channel between `kobo-sim`, `kobo-sdk` and `kobod` runs
  over loopback TCP on Windows (see "Known boundary" below) and its session
  tests execute in CI.
- `kobo-cli --features device-write` compiles for a Windows host: the gated
  hardware-verification verbs (`tap`, `present`, `stop`, `smoke-display`,
  `guard-test`) are host-side ssh orchestration, so they are expected to
  work from Windows exactly as from macOS/Linux. The compile check is in
  CI; the verbs' runtime behavior against a real device is UNVERIFIED from
  any host OS until run against hardware.

- Interactive terminal attachment (`kobo shell`, `kobo stream`): the PTY
  layer is a real ConPTY implementation (`CreatePseudoConsole` + extended
  startup info), runtime-verified on the windows-2022 runner by the
  `conpty_carries_output_status_and_shutdown` test, which spawns cmd.exe,
  reads its output over the channel, checks its exit status and closes the
  console. Interactive use against a real device is UNVERIFIED until run
  from a physical Windows machine.

UNVERIFIED on Windows (compile clean, unit-tested where noted, but not yet
run end-to-end on a real Windows machine):

- `kobo sync` (dedicated Syncthing orchestration). Unit tests pass in CI;
  a full sync against a device has not been run from Windows.
- Device communication (ssh/scp to a reader) from Windows: expected to
  work with an installed OpenSSH client, not yet exercised.

Unsupported on Windows by design:

- Kernel ioctls, framebuffer and input device access (there is no Kobo
  panel behind a Windows host); these return explicit Unsupported errors.
- Device-side `device-write` crates (`kobo-hal` with the feature, the
  `kobo-tap`/`kobo-guard`/`kobo-handoff`/`kobo-smoke` binaries) are ARM
  Linux device software; they are not built for Windows at all.

### Known boundary: the simulator channel on Windows

On Unix the local app channel is a Unix-domain socket with owner-only mode
bits: only the owner's processes can connect. On Windows it is a loopback
TCP listener whose port is written to an exclusive address file at the
socket path. Any process running as any local user can in principle connect
to a loopback port. The address file inherits the user-profile ACL, which
limits casual discovery, but this is a genuinely weaker boundary than the
Unix socket and is recorded here rather than smoothed over. Hardening
options (per-connection token handshake) are under consideration.

## Device-side binaries (ARM Linux, musl)

Verified by emulation (qemu-arm on every push, job `device-emulated`):

- `cargo test` suites for kobo-abi, kobo-hal, kobod, kobo-tap, kobo-guard,
  kobo-handoff and kobo-smoke, built for armv7-unknown-linux-musleabihf.
- Execution smoke: each built device binary is invoked under qemu-arm, so
  artifacts are proven to run, not just to link.

Still UNVERIFIED on real hardware even where emulation passes:

- Framebuffer, panel refresh and touch input ioctls against a real kernel.
- Nickel/Qt integration, suspend/resume timing, Wi-Fi firmware behavior.
- Anything timing- or signal-sensitive can behave differently under
  emulation; emulation-green is not device-green.

## Keeping this honest

- Every Windows or emulated path above either runs in CI or is named here.
- A test that cannot run on Windows or under qemu is skipped with a `cfg`
  attribute and a comment giving the reason; the skip is visible in source.
- When a path is verified on a real Windows machine or a real device, this
  document is updated in the same commit that records the evidence.

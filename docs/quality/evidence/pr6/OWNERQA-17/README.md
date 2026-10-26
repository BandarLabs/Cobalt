# Automated gates at the pr6 head

All automated host gates run green at the pr6 companion-CLI head:

- `cargo test -p kobo-cli`: 406/406 pass (kobo-cli-tests-final.log), including
  the two packaging tests that need an ARM C cross-compiler; run with the same
  gcc-arm-linux-gnueabihf-class toolchain ci.yml installs.
- `cargo fmt --all -- --check`: clean.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` on the
  CI-pinned 1.85.1 toolchain: clean (clippy-ci-gate.log). Newer local toolchains
  report lints CI does not run; the pinned gate is the one that matters.
- The simulator, device-build and release-build workflows run in CI; the fork
  queue had not started them at this writing.

These are host and simulator gates, not physical-reader validation.

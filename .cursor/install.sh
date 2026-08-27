#!/usr/bin/env bash
# Idempotent Cloud Agent bootstrap for the Cobalt Rust workspace.
# Mirrors the toolchain the CI `host` and `device-build` jobs use so a fresh
# agent can build, test, lint, run the browser simulator, and cross-compile
# device binaries.
set -euo pipefail

TOOLCHAIN="1.85.1"
ARM_TARGET="armv7-unknown-linux-musleabihf"

# Pin the workspace toolchain (Cargo.toml rust-version) with the components the
# `cargo fmt`/`cargo clippy` checks require.
rustup toolchain install "$TOOLCHAIN" --profile minimal --component clippy,rustfmt
rustup default "$TOOLCHAIN"

# ARM hard-float musl target used for the on-device (Kobo) binaries.
rustup target add "$ARM_TARGET"

# The `ring` TLS crate and the packaging tests need an ARM cross C compiler.
if ! command -v arm-linux-gnueabihf-gcc >/dev/null 2>&1; then
  sudo apt-get update
  sudo apt-get install --yes --no-install-recommends gcc-arm-linux-gnueabihf
fi

# Warm the host build cache so the first agent build and the simulator start
# quickly. Kept out of `start` so it is not re-run on every boot.
cargo build --workspace --locked

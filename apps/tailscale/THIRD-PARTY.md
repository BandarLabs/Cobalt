# Tailscale

The Tailscale runtime launches the unmodified [Tailscale](https://github.com/tailscale/tailscale)
programs `tailscaled` and `tailscale`. Tailscale is licensed under the BSD
3-Clause License, reproduced in
[`licenses/LICENSE-Tailscale.txt`](../../licenses/LICENSE-Tailscale.txt),
which is installed on the reader with Cobalt.

The programs are not in this repository or in the platform package. The
reader downloads the official archive the first time Tailscale is switched on
and checks it, and each program in it, against digests built into Cobalt.

- Upstream version: Tailscale 1.104.7
- Source: https://github.com/tailscale/tailscale/tree/v1.104.7
- Download: https://pkgs.tailscale.com/stable/tailscale_1.104.7_arm.tgz
- Archive SHA-256: `1c753fef45d497491081db37552b7a2a42c7932292743bf6936cb1a42b6bceb0`
- `tailscaled` SHA-256: `7b6e407353575cb4dcd1d95da7a1b197c9e6c0597b9570007d7564d2929a7ae4`
- `tailscale` SHA-256: `1c14fb51b91c9d70e24643a905f6046675a3f76d8bea89c9eb11e8e601c2adbb`

To check the pin yourself, download the archive, compare its digest with the
one Tailscale publishes beside it at the same address with `.sha256` added,
and compare both with the constants in `crates/kobod/src/tailscale.rs`.

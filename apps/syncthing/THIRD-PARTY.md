# Syncthing

The Sync runtime launches an unmodified [Syncthing](https://github.com/syncthing/syncthing)
binary. Syncthing is licensed under MPL-2.0.

The binary is not in the platform package. The reader downloads it the first
time a computer is set up and checks it against a digest built into Cobalt.

- Upstream version: Syncthing v2.0.9
- Commit: `3382ccc3f16536b5a7b6df7c8212951f7d4d3a9f`
- Source: https://github.com/syncthing/syncthing
- Build: `GOARM=7 CGO_ENABLED=0 go run build.go -goos linux -goarch arm build`
  with Go 1.24.13, as in `build-armv7.sh` and
  `.github/workflows/syncthing-engine.yml`
- SHA-256: `e7e0523d8db0328b22ebff5c98bd721c94e295122771c0538414898a06ef8ebf`
- Download: `syncthing-armv7` on this repository's `syncthing-v2.0.9` release

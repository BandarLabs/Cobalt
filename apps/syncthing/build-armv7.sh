#!/bin/sh
# Build in a pre-verified, pinned upstream Syncthing checkout. This script
# neither downloads source nor installs the result on a device.
set -eu

: "${SYNCTHING_SOURCE:?set SYNCTHING_SOURCE to a pinned upstream checkout}"
: "${SYNCTHING_OUTPUT:?set SYNCTHING_OUTPUT outside this repository}"

EXPECTED_COMMIT=3382ccc3f16536b5a7b6df7c8212951f7d4d3a9f
EXPECTED_GO=go1.24.13
EXPECTED_SHA256=e7e0523d8db0328b22ebff5c98bd721c94e295122771c0538414898a06ef8ebf

test "$(git -C "$SYNCTHING_SOURCE" rev-parse HEAD)" = "$EXPECTED_COMMIT"
test "$(go env GOVERSION)" = "$EXPECTED_GO"
test -f "$SYNCTHING_SOURCE/LICENSE"
test -d "$SYNCTHING_OUTPUT"

(
  cd "$SYNCTHING_SOURCE"
  go mod verify
  # build.go embeds the build user and hostname, so an unpinned build is a
  # different binary on every machine. Pinning them, and the Go release above,
  # is what makes the digest below something anyone can reproduce.
  BUILD_USER=cobalt BUILD_HOST=cobalt GOARM=7 CGO_ENABLED=0 \
    go run build.go -goos linux -goarch arm build
)

install -m 0755 "$SYNCTHING_SOURCE/syncthing" "$SYNCTHING_OUTPUT/syncthing"
ACTUAL_SHA256=$(shasum -a 256 "$SYNCTHING_OUTPUT/syncthing" | awk '{print $1}')
test "$ACTUAL_SHA256" = "$EXPECTED_SHA256"
printf '%s\n' "$ACTUAL_SHA256" > "$SYNCTHING_OUTPUT/syncthing.sha256"
echo "built $(wc -c < "$SYNCTHING_OUTPUT/syncthing") bytes"
echo "export COBALT_SYNCTHING_ARTIFACT='$SYNCTHING_OUTPUT/syncthing' before packaging"

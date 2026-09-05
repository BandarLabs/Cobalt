#!/bin/sh
# Build in a pre-verified, pinned upstream Syncthing checkout. This script
# neither downloads source nor installs the result on a device.
set -eu

: "${SYNCTHING_SOURCE:?set SYNCTHING_SOURCE to a pinned upstream checkout}"
: "${SYNCTHING_OUTPUT:?set SYNCTHING_OUTPUT outside this repository}"

EXPECTED_COMMIT=3382ccc3f16536b5a7b6df7c8212951f7d4d3a9f
EXPECTED_SHA256=845336fa67494f38ecb69dfaa0a81de6e33e9b5427bd707385d85051596641a1

test "$(git -C "$SYNCTHING_SOURCE" rev-parse HEAD)" = "$EXPECTED_COMMIT"
test -f "$SYNCTHING_SOURCE/LICENSE"
test -d "$SYNCTHING_OUTPUT"

(
  cd "$SYNCTHING_SOURCE"
  go mod verify
  GOARM=7 CGO_ENABLED=0 go run build.go -goos linux -goarch arm build
)

install -m 0755 "$SYNCTHING_SOURCE/syncthing" "$SYNCTHING_OUTPUT/syncthing"
ACTUAL_SHA256=$(shasum -a 256 "$SYNCTHING_OUTPUT/syncthing" | awk '{print $1}')
test "$ACTUAL_SHA256" = "$EXPECTED_SHA256"
printf '%s\n' "$ACTUAL_SHA256" > "$SYNCTHING_OUTPUT/syncthing.sha256"
echo "built $(wc -c < "$SYNCTHING_OUTPUT/syncthing") bytes"
echo "export COBALT_SYNCTHING_ARTIFACT='$SYNCTHING_OUTPUT/syncthing' before packaging"

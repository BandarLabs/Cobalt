#!/usr/bin/env bash
# Device census matrix: every catalog app in a fresh simulator at every
# supported device profile x every text scale. 27 cells = 9 profiles
# (kobo-profile SUPPORTED_PROFILES) x 3 scales (default, large, extra-large).
#
# Results land OUT_ROOT/<profile>-<scale>/out/results.json and the aggregate
# census JSON+MD are written by census-summarize.py.
#
# Recipe notes (learned 2026-09-18):
# - Profile comes from KOBO_SIM_PROFILE, scale from KOBO_TEXT_SCALE; the sim
#   validator lives in the freshly built kobo-cli, so build it first.
# - Run with the RELEASE binary for speed: cargo build --release -p kobo-cli,
#   then keep target/debug/kobo as a copy of it (check-apps-sim.py drives
#   target/debug/kobo).
# - Portrait cells only. Landscape is an app-owned SetOrientation choice, not a
#   device-level re-render (kobod/src/device.rs); apps with their own rotation
#   control get measured-reflow checks through that control instead.
# - 758x1024 is NOT a supported profile; do not add it.
# - No source edits while the matrix runs: every cell must measure the same
#   tree. 2-way parallel maximum on small sandboxes.
set -u
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT_ROOT="${1:-/tmp/census/matrix}"
PROFILES="clara-bw-391 clara-bw-395 clara-hd-376 clara-colour-393 elipsa-2e-389 libra-2-388 libra-colour-390 libra-colour-390-4.46.23836 libra-h2o-384"
SCALES="default large extra-large"
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0
run_cell() {
  local profile="$1" scale="$2"
  local cell="$OUT_ROOT/$profile-$scale"
  if [ -s "$cell/out/results.json" ]; then
    echo "SKIP $profile-$scale (already complete)"
    return 0
  fi
  mkdir -p "$cell"
  echo "CELL $profile-$scale $(date -Is)"
  KOBO_SIM_PROFILE="$profile" KOBO_TEXT_SCALE="$scale" \
    python3 "$ROOT/scripts/check-apps-sim.py" --out "$cell/out" \
    > "$cell/cell.log" 2>&1
  echo "DONE $profile-$scale rc=$? $(date -Is)"
}
running=0
for profile in $PROFILES; do
  for scale in $SCALES; do
    run_cell "$profile" "$scale" &
    running=$((running + 1))
    if [ "$running" -ge 2 ]; then wait -n; running=$((running - 1)); fi
  done
done
wait
echo "MATRIX-COMPLETE $(date -Is)"

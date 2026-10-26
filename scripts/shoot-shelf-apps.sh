#!/bin/sh
# Drive catalog apps in isolated simulators and propagate every failure.
set -eu
ROOT="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
exec python3 "$ROOT/scripts/check-apps-sim.py" --out "${SHOTS:-${TMPDIR:-/tmp}/cobalt-app-shots}" "$@"

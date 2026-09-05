#!/bin/sh
# Drive every catalog app's drive.kobo in the host simulator with --ideal.
set -eu

ROOT="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
TMP="${TMPDIR:-/tmp}"
KOBO="${CARGO_TARGET_DIR:-$TMP/cobalt-beta-shelf-target}/debug/kobo"
SHOTS="${SHOTS:-$TMP/cobalt-app-shots}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$TMP/cobalt-beta-shelf-target}"

if [ ! -x "$KOBO" ]; then
  echo "missing $KOBO; build kobo-cli first" >&2
  exit 1
fi

stop_sim() {
  if [ -n "${DEV_PID:-}" ]; then
    kill "$DEV_PID" 2>/dev/null || true
    wait "$DEV_PID" 2>/dev/null || true
    DEV_PID=
  fi
  # Leave nothing listening on the simulator port.
  pids="$(lsof -tiTCP:8787 -sTCP:LISTEN 2>/dev/null || true)"
  if [ -n "$pids" ]; then
    kill $pids 2>/dev/null || true
    sleep 0.3
  fi
}

wait_for_sim() {
  i=0
  while [ "$i" -lt 90 ]; do
    if "$KOBO" drive --step dump >/tmp/cobalt-drive-dump.txt 2>/tmp/cobalt-drive-dump.err; then
      # Dump prints `kind  ["label"]`. Ignore the trailing success line.
      if grep -v '^drive:' /tmp/cobalt-drive-dump.txt | grep -q '\['; then
        return 0
      fi
    fi
    i=$((i + 1))
    sleep 1
  done
  return 1
}

run_one() {
  app="$1"
  script="$ROOT/apps/$app/drive.kobo"
  if [ ! -f "$script" ]; then
    echo "SKIP $app (no drive.kobo)"
    return 0
  fi
  mkdir -p "$SHOTS/$app"
  stop_sim
  echo "==== $app ===="
  (
    cd "$ROOT/apps/$app"
    if [ "$app" = fanshelf ]; then
      FANSHELF_DEMO=1 "$KOBO" dev
    else
      "$KOBO" dev
    fi
  ) >/tmp/cobalt-dev-"$app".log 2>&1 &
  DEV_PID=$!
  if ! wait_for_sim; then
    echo "FAIL $app: simulator did not answer" | tee -a /tmp/cobalt-shelf-shots.log
    tail -20 /tmp/cobalt-dev-"$app".log || true
    stop_sim
    return 1
  fi
  if "$KOBO" drive --ideal --script "$script" --shots "$SHOTS/$app"; then
    echo "OK $app" | tee -a /tmp/cobalt-shelf-shots.log
  else
    echo "FAIL $app: drive failed" | tee -a /tmp/cobalt-shelf-shots.log
  fi
  stop_sim
}

APPS="${*:-}"
if [ -z "$APPS" ]; then
  APPS="backgammon calibre-web crossword deck fanshelf fieldbook flashcards frame grimoire habits homepanel inkling kitchencard lichess logicpack musicstand needles nonograms panels paperterm parlor parser post pubquiz readlater rss-miniflux syncthing vault verses"
fi

: >/tmp/cobalt-shelf-shots.log
trap stop_sim EXIT INT TERM
for app in $APPS; do
  run_one "$app" || true
done
echo "done; log /tmp/cobalt-shelf-shots.log"

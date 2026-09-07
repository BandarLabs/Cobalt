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

catalog_apps() {
  (
    cd "$ROOT"
    node --input-type=module -e '
      import { collectRegistry } from "./tools/app-registry.mjs";
      for (const app of collectRegistry().apps) console.log(app.id);
    '
  )
}

source_of() {
  for root in apps examples; do
    if [ -d "$ROOT/$root/$1" ]; then
      echo "$ROOT/$root/$1"
      return 0
    fi
  done
  return 1
}

script_of() {
  for name in drive.kobo drive.txt; do
    if [ -f "$1/$name" ]; then
      echo "$1/$name"
      return 0
    fi
  done
  return 1
}

run_one() {
  app="$1"
  directory="$(source_of "$app")" || {
    echo "SKIP $app (no source directory)"
    return 0
  }
  script="$(script_of "$directory")" || {
    echo "SKIP $app (no drive script)"
    return 0
  }
  mkdir -p "$SHOTS/$app"
  store="$(mktemp -d "/tmp/cb-$app.XXXXXX")"
  case "$app" in
    deck)
      TMPDIR="$store" "$KOBO" deck init --home "$store/deck-config" >/dev/null
      TMPDIR="$store" "$KOBO" deck set 1 --label Todo --launch todo --home "$store/deck-config" >/dev/null
      TMPDIR="$store" "$KOBO" deck set 2 --label Example --url https://example.com --home "$store/deck-config" >/dev/null
      TMPDIR="$store" "$KOBO" deck push --sim --home "$store/deck-config" >/dev/null
      ;;
    frame)
      TMPDIR="$store" "$KOBO" frame init --sim >/dev/null
      TMPDIR="$store" "$KOBO" frame push "$ROOT/apps/frame/screenshots/frame.png" --sim --fit pad >/dev/null
      ;;
    vault)
      TMPDIR="$store" "$KOBO" vault init --sim >/dev/null
      TMPDIR="$store" "$KOBO" vault push "$ROOT/apps/vault/tests/fixtures" --sim >/dev/null
      ;;
  esac
  stop_sim
  echo "==== $app ===="
  (
    cd "$directory"
    case "$app" in
      fanshelf) TMPDIR="$store" FANSHELF_DEMO=1 "$KOBO" dev ;;
      inkling) TMPDIR="$store" KOBO_INKLING_DAY=2026-09-01 "$KOBO" dev ;;
      *) TMPDIR="$store" "$KOBO" dev ;;
    esac
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
  APPS="$(catalog_apps)"
fi

: >/tmp/cobalt-shelf-shots.log
trap stop_sim EXIT INT TERM
for app in $APPS; do
  run_one "$app" || true
done
echo "done; log /tmp/cobalt-shelf-shots.log"

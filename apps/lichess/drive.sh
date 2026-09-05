#!/bin/sh
set -eu

here="$(CDPATH= cd -- "$(dirname "$0")" && pwd)"
root="$(CDPATH= cd -- "$here/../.." && pwd)"
scenario="${1:-game}"
address="${2:-127.0.0.1:8787}"
script="$here/drive/$scenario.kobo"
shots="${CARGO_TARGET_DIR:-$root/target}/lichess-drive/$scenario"

test -f "$script"
mkdir -p "$shots"

cd "$here"
KOBO_LICHESS_DEMO="$scenario" \
  cargo run --quiet --manifest-path ../../crates/kobo-cli/Cargo.toml -- \
  dev "$address" &
server=$!
trap 'kill "$server" 2>/dev/null || true; wait "$server" 2>/dev/null || true' EXIT INT TERM

tries=0
until curl -fsS "http://$address/layout" >/dev/null 2>&1; do
  tries=$((tries + 1))
  if ! kill -0 "$server" 2>/dev/null || [ "$tries" -ge 80 ]; then
    echo "Lichess simulator did not become ready" >&2
    exit 1
  fi
  sleep 0.25
done

cd "$root"
cargo run --quiet -p kobo-cli -- drive \
  --address "$address" \
  --script "$script" \
  --shots "$shots" \
  --ideal

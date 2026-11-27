#!/bin/sh
# The computer's half of the apps that work with one: run from `host` steps in
# tools/device-shots.moments, so Sidekick, Deck and Paperterm show real work
# when they are photographed.
#
#   device-shots-companions.sh ask        three agents waiting on the reader
#   device-shots-companions.sh stream CMD a terminal for Paperterm, running CMD
#   device-shots-companions.sh stop       end every companion this started
#
# Run `kobo sidekick init --device IP` once first, so the reader knows this
# computer, and `kobo sidekick run` so the helper is up. The questions go
# through `kobo-sidekickd hook claude`, the command a coding agent's
# permission hook runs, so the reader is asked exactly as it would be by an
# agent at work. Nothing is run when one is answered: the hooks print the
# decision and end.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
kobo=${KOBO:-kobo}
state="${TMPDIR:-/tmp}/cobalt-device-shots"
mkdir -p "$state"

helper() {
  beside="$(dirname "$(command -v "$kobo")")/kobo-sidekickd"
  if [ -x "$beside" ]; then echo "$beside"; else command -v kobo-sidekickd; fi
}

# One agent's request for permission, as Claude Code hands it to its hook.
ask() {
  printf '{"hook_event_name":"PermissionRequest","session_id":"%s","cwd":"%s","tool_name":"%s","tool_input":%s,"permission_suggestions":[{"type":"addRules","rules":[{"toolName":"%s"}],"behavior":"allow","destination":"session"}]}' \
    "$1" "$2" "$3" "$4" "$3" |
    "$(helper)" hook claude > /dev/null 2>&1 &
  echo $! >> "$state/pids"
}

case "${1:-}" in
  ask)
    ask 01JD4Q8ZK3T7 "$HOME/src/cobalt" Bash '{"command":"cargo test -p kobo-ui --lib"}'
    sleep 1
    ask 01JD4QB2M9WX "$HOME/src/site" Edit '{"file_path":"docs/index.html","old_string":"<h2>","new_string":"<h2 id=\"mac\">"}'
    sleep 1
    ask 01JD4QC7P1RN "$HOME/src/field-notes" WebFetch '{"url":"https://www.gutenberg.org/ebooks/1342"}'
    ;;
  stream)
    shift
    "$here/device-shots-companions.sh" stop-stream
    "$kobo" stream -- "$@" > "$state/stream.log" 2>&1 &
    echo $! > "$state/stream.pid"
    sleep 3
    ;;
  stop-stream)
    if [ -f "$state/stream.pid" ]; then
      kill "$(cat "$state/stream.pid")" 2>/dev/null || true
      rm -f "$state/stream.pid"
      sleep 1
    fi
    ;;
  stop)
    "$here/device-shots-companions.sh" stop-stream
    if [ -f "$state/pids" ]; then
      while read -r pid; do kill "$pid" 2>/dev/null || true; done < "$state/pids"
      rm -f "$state/pids"
    fi
    ;;
  *)
    sed -n '2,16p' "$0" | sed 's/^# \{0,1\}//' >&2
    exit 2
    ;;
esac

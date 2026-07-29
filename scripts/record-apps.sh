#!/bin/sh
#
# Records every example application while driving it on a real reader.
#
# This is the moving-picture half of the screenshot loop. A still says what a
# screen looked like once; this says what it did, which is the only way to see
# a tap landing in the wrong place, a screen flashing through a wrong state
# before it settles, or ink left behind by a refresh.
#
# It is a script rather than a list of commands in a README because it is meant
# to be run again. The interesting failures are the ones that appear between
# two runs, and comparing runs is only possible if both were driven the same
# way.
#
# Read-only on the device. `kobo record` opens the framebuffer for reading and
# never grabs, refreshes or writes it. The taps are real, so the applications
# do move; nothing else on the reader is touched.
#
# usage: scripts/record-apps.sh --device IP [--out DIR] [--seconds N]
#                               [--fps F] [--apps "hn rss todo"]

set -eu

DEVICE=""
OUT=""
SECONDS_EACH=24
FPS=2
APPS="hn rss brief gutenbird todo tictactoe gallery terminal chat launcher"

usage() {
    sed -n '2,22p' "$0" | sed 's/^# \{0,1\}//'
    exit 2
}

while [ $# -gt 0 ]; do
    case "$1" in
        --device) DEVICE="${2:?--device needs an address}"; shift 2 ;;
        --out) OUT="${2:?--out needs a directory}"; shift 2 ;;
        --seconds) SECONDS_EACH="${2:?--seconds needs a count}"; shift 2 ;;
        --fps) FPS="${2:?--fps needs a rate}"; shift 2 ;;
        --apps) APPS="${2:?--apps needs a list}"; shift 2 ;;
        -h|--help) usage ;;
        *) echo "unknown option '$1'" >&2; usage ;;
    esac
done

[ -n "$DEVICE" ] || { echo "this needs a reader: --device IP" >&2; usage; }

cd "$(dirname "$0")/.."

# Dated, because the point of a recording is comparing today against last time.
[ -n "$OUT" ] || OUT="target/device-test/$(date +%Y-%m-%d-%H%M%S)"

# The panel is 1072x1448. These are the three places worth touching in almost
# every one of these applications: something in the list, something in the bar
# along the bottom, and the way back at the top left. Applications that want
# their own route through are listed in `steps_for` below.
TAP_CONTENT="536,400"
TAP_BAR="400,1380"
TAP_BACK="80,80"

# The tap sequence for an application, as "delay:x,y" pairs. The delay is
# seconds to wait *before* the tap, counted from the previous one, which leaves
# room for a fetch to come back before the next tap lands on a screen that has
# moved underneath it.
steps_for() {
    case "$1" in
        # A list, a story, and back to the list.
        hn|rss|brief|gutenbird)
            echo "7:$TAP_CONTENT 6:$TAP_BACK 4:$TAP_BAR" ;;
        # A board takes taps anywhere and has no back.
        tictactoe)
            echo "5:$TAP_CONTENT 3:400,700 3:670,700 3:536,900" ;;
        # A keyboard, so the taps go to where the keys are.
        chat|terminal)
            echo "6:$TAP_BAR 4:300,1100 3:500,1100 3:700,1100" ;;
        *)
            echo "6:$TAP_CONTENT 5:$TAP_BACK 4:$TAP_BAR" ;;
    esac
}

# Built once rather than per application, so a compile error stops this before
# it has taken the panel away from anybody.
echo "building the CLI with device-write, for taps"
cargo build --release -p kobo-cli --features device-write

KOBO="target/release/kobo"

# Held awake for the whole run. Without this the reader sleeps partway through
# and the rest of the recordings are of a blank panel, which is the single most
# common way this loop wastes ten minutes.
echo "holding $DEVICE awake"
"$KOBO" session --device "$DEVICE" --keep-awake on
"$KOBO" session --device "$DEVICE" --wifi-always-on on || true

mkdir -p "$OUT"
FAILED=""

for app in $APPS; do
    echo
    echo "=== $app ==="
    # The application is given longer than the recording, so it is still on the
    # panel when the last frame is taken rather than handing it back early and
    # recording the home screen.
    if ! "$KOBO" present "$app" --device "$DEVICE" --seconds $((SECONDS_EACH + 20)); then
        echo "could not start $app; moving on" >&2
        FAILED="$FAILED $app"
        continue
    fi
    sleep 3

    "$KOBO" record --device "$DEVICE" --seconds "$SECONDS_EACH" --fps "$FPS" \
        --out "$OUT/$app" &
    recorder=$!

    for step in $(steps_for "$app"); do
        delay="${step%%:*}"
        point="${step#*:}"
        sleep "$delay"
        "$KOBO" tap --device "$DEVICE" "$point" || true
    done

    if ! wait "$recorder"; then
        echo "recording $app failed" >&2
        FAILED="$FAILED $app"
    fi
    "$KOBO" stop --device "$DEVICE" || true
done

# Handed back deliberately. A reader left with a wake lock does not sleep, and
# the owner finds a flat battery in the morning.
echo
echo "releasing the wake lock"
"$KOBO" session --device "$DEVICE" --keep-awake off || true

echo
echo "recordings are in $OUT"
if [ -n "$FAILED" ]; then
    echo "these did not record:$FAILED" >&2
    exit 1
fi

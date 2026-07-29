#!/bin/sh
#
# Records a walk through every application, driven from the launcher the way
# somebody holding the reader would drive it.
#
# The companion to `record-apps.sh`, which starts each application on its own.
# This one never does that. It presents the launcher once and then taps its way
# in and out of every tile, so what comes out is the thing that actually has to
# work: the launcher, the application, the way back, and the launcher again.
# A tap that lands on the wrong tile, a back control that is missing, or a
# screen that flashes through a wrong state on the way home all show up here
# and in none of the still screenshots.
#
# Two kinds of output, because both are wanted. Each application gets its own
# clip under its own name, and they are concatenated into one tour at the end.
#
# Read-only on the device. `kobo record` opens the framebuffer for reading and
# never grabs, refreshes or writes it. The taps are real, so the applications
# do move; nothing else on the reader is touched.
#
# usage: scripts/record-tour.sh --device IP [--out DIR] [--fps F]

set -eu

DEVICE=""
OUT=""
FPS=2

usage() {
    sed -n '2,21p' "$0" | sed 's/^# \{0,1\}//'
    exit 2
}

while [ $# -gt 0 ]; do
    case "$1" in
        --device) DEVICE="${2:?--device needs an address}"; shift 2 ;;
        --out) OUT="${2:?--out needs a directory}"; shift 2 ;;
        --fps) FPS="${2:?--fps needs a rate}"; shift 2 ;;
        -h|--help) usage ;;
        *) echo "unknown option '$1'" >&2; usage ;;
    esac
done

[ -n "$DEVICE" ] || { echo "this needs a reader: --device IP" >&2; usage; }

cd "$(dirname "$0")/.."
[ -n "$OUT" ] || OUT="target/tour/$(date +%Y-%m-%d-%H%M%S)"

# The panel is 1072x1448. Tiles sit on a three by two grid and the launcher
# pages between two screens of them.
BACK="96,110"
NEXT_PAGE="890,1380"
PREV_PAGE="177,1380"

# Page one, then page two.
T_AUDIOBOOK="202,510"; T_SETTINGS="536,510"; T_GUTENBIRD="869,510"
T_TERMINAL="202,880";  T_GALLERY="536,880";  T_MAGNET="869,880"
T_TICTACTOE="202,510"; T_BRIEF="536,510";    T_TODO="869,510"
T_CHAT="202,880";      T_HN="536,880";       T_RSS="869,880"

# What to do once inside, as "delay:x,y" pairs. The delay is seconds to wait
# before the tap, counted from the previous one, which leaves room for a fetch
# to come back before the next tap lands on a screen that has moved underneath.
# Every sequence ends back at the launcher so the next one can start there.
steps_for() {
    case "$1" in
        settings)   echo "4:536,770 5:$BACK 3:536,525 5:$BACK 3:$BACK" ;;
        audiobook)  echo "4:530,250 7:$BACK 3:$BACK" ;;
        magnet)     echo "12:$BACK" ;;
        gallery)    echo "4:536,400 4:536,1380 4:$BACK 3:$BACK" ;;
        terminal)   echo "5:400,1380 3:300,1100 3:500,1100 4:$BACK 3:$BACK" ;;
        chat)       echo "5:400,1380 3:300,1100 3:500,1100 4:$BACK 3:$BACK" ;;
        tictactoe)  echo "4:536,400 3:400,700 3:670,700 3:536,900 4:$BACK" ;;
        hn|rss|brief|gutenbird|todo)
                    echo "7:536,400 7:$BACK 4:$BACK" ;;
        *)          echo "6:536,400 5:$BACK 3:$BACK" ;;
    esac
}

# How long each clip runs. Long enough to cover its own taps plus the fetch
# they are waiting on, and no longer, because a tour nobody watches to the end
# proves nothing.
seconds_for() {
    case "$1" in
        hn|rss|brief|gutenbird) echo 26 ;;
        settings|terminal|chat) echo 24 ;;
        audiobook|gallery)      echo 20 ;;
        magnet)                 echo 16 ;;
        *)                      echo 18 ;;
    esac
}

tile_for() {
    case "$1" in
        audiobook) echo "$T_AUDIOBOOK" ;; settings) echo "$T_SETTINGS" ;;
        gutenbird) echo "$T_GUTENBIRD" ;; terminal) echo "$T_TERMINAL" ;;
        gallery)   echo "$T_GALLERY" ;;   magnet)   echo "$T_MAGNET" ;;
        tictactoe) echo "$T_TICTACTOE" ;; brief)    echo "$T_BRIEF" ;;
        todo)      echo "$T_TODO" ;;      chat)     echo "$T_CHAT" ;;
        hn)        echo "$T_HN" ;;        rss)      echo "$T_RSS" ;;
    esac
}

PAGE_ONE="settings gutenbird terminal gallery magnet audiobook"
PAGE_TWO="tictactoe brief todo chat hn rss"

echo "building the CLI with device-write, for taps"
cargo build --release -q -p kobo-cli --features device-write
KOBO="target/release/kobo"

# Held awake for the whole run. Without this the reader sleeps partway through
# and the rest of the tour is of a blank panel.
echo "holding $DEVICE awake"
"$KOBO" session --device "$DEVICE" --keep-awake on >/dev/null
"$KOBO" session --device "$DEVICE" --wifi-always-on on >/dev/null 2>&1 || true

mkdir -p "$OUT"

# One launcher for the whole tour. Everything below is a tap inside it.
echo "presenting the launcher"
"$KOBO" present launcher --device "$DEVICE" --seconds 1800 >/dev/null
sleep 4

CLIPS=""
FAILED=""

record_one() {
    name="$1"; tile="$2"; secs="$3"
    echo
    echo "=== $name ==="
    "$KOBO" record --device "$DEVICE" --seconds "$secs" --fps "$FPS" \
        --out "$OUT/$name" >/dev/null 2>&1 &
    recorder=$!
    sleep 2
    "$KOBO" tap --device "$DEVICE" "$tile" >/dev/null 2>&1 || true
    for step in $(steps_for "$name"); do
        sleep "${step%%:*}"
        "$KOBO" tap --device "$DEVICE" "${step#*:}" >/dev/null 2>&1 || true
    done
    if wait "$recorder" && [ -f "$OUT/$name/recording.mp4" ]; then
        CLIPS="$CLIPS $name"
    else
        echo "recording $name failed" >&2
        FAILED="$FAILED $name"
    fi
}

# The launcher itself first, paging between its two screens, so the tour opens
# on the thing every other clip starts from.
record_one launcher "$NEXT_PAGE" 14
"$KOBO" tap --device "$DEVICE" "$PREV_PAGE" >/dev/null 2>&1 || true
sleep 3

for app in $PAGE_ONE; do
    record_one "$app" "$(tile_for "$app")" "$(seconds_for "$app")"
    sleep 2
done

echo
echo "turning to the second page of tiles"
"$KOBO" tap --device "$DEVICE" "$NEXT_PAGE" >/dev/null 2>&1 || true
sleep 3

for app in $PAGE_TWO; do
    record_one "$app" "$(tile_for "$app")" "$(seconds_for "$app")"
    "$KOBO" tap --device "$DEVICE" "$NEXT_PAGE" >/dev/null 2>&1 || true
    sleep 2
done

# Handed back deliberately. A reader left with a wake lock does not sleep, and
# the owner finds a flat battery in the morning.
echo
echo "releasing the wake lock"
"$KOBO" stop --device "$DEVICE" >/dev/null 2>&1 || true
"$KOBO" session --device "$DEVICE" --keep-awake off >/dev/null 2>&1 || true

# The combined cut. Concatenating the encoded clips rather than re-encoding
# from frames keeps this fast and lossless, and every clip came out of the same
# encoder at the same size so the streams are compatible.
if command -v ffmpeg >/dev/null 2>&1 && [ -n "$CLIPS" ]; then
    echo "assembling the combined tour"
    : > "$OUT/clips.txt"
    for name in $CLIPS; do
        echo "file '$(cd "$OUT/$name" && pwd)/recording.mp4'" >> "$OUT/clips.txt"
    done
    ffmpeg -nostdin -y -loglevel error -f concat -safe 0 -i "$OUT/clips.txt" \
        -c copy "$OUT/cobalt-tour.mp4" && echo "tour: $OUT/cobalt-tour.mp4"
fi

echo
echo "clips are in $OUT"
if [ -n "$FAILED" ]; then
    echo "these did not record:$FAILED" >&2
    exit 1
fi

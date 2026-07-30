#!/bin/sh
#
# Records one continuous tour of Cobalt, driven from the launcher the way
# somebody holding the reader would drive it.
#
# usage: scripts/record-tour.sh --device IP [--out DIR] [--seconds N]
#                               [--fps F] [--speed X]
#
# # Why one recording rather than one clip per application
#
# This used to take the panel for each application in turn. That meant a reader
# restart between every one of them, and the reader spent longer restarting
# than running anything. It also cut out the part worth showing: applications
# are started from the launcher and come back to it, and a set of clips with
# the launcher removed shows a pile of unrelated programs rather than a system.
#
# The launcher is the title card, too. A tour that announces each application
# on a slide before showing it is a slide deck. A tour that shows a finger
# landing on a tile and the application opening is the thing itself.
#
# # Why the order is what it is
#
# Impressive first, because this is watched by somebody deciding whether to
# keep watching. A reader that researches a subject, writes a book about it and
# reads it aloud is the strongest thing here, so it opens. Then sixty thousand
# books with their covers, then the news. The applications that exist to prove
# one narrow point are at the end, where they belong.
#
# # Why the taps are one invocation
#
# `kobo tap` takes a whole sequence and times it on the device. Sent one at a
# time, each tap costs a cross-compile, an upload of the tap binary and a
# checksum on the reader's own processor, so every wait was the wait asked for
# plus however long that took, and a tap meant to land while a screen was up
# landed after it had gone.
#
# Read-only apart from the taps themselves. `kobo record` opens the framebuffer
# for reading and never grabs, refreshes or writes it.

set -eu

DEVICE=""
OUT=""
SECONDS_TOTAL=300
FPS=2
SPEED=3

usage() { sed -n '2,38p' "$0" | sed 's/^# \{0,1\}//'; exit 2; }

while [ $# -gt 0 ]; do
    case "$1" in
        --device) DEVICE="${2:?--device needs an address}"; shift 2 ;;
        --out) OUT="${2:?--out needs a directory}"; shift 2 ;;
        --seconds) SECONDS_TOTAL="${2:?--seconds needs a count}"; shift 2 ;;
        --fps) FPS="${2:?--fps needs a rate}"; shift 2 ;;
        --speed) SPEED="${2:?--speed needs a multiplier}"; shift 2 ;;
        -h|--help) usage ;;
        *) echo "unknown option '$1'" >&2; usage ;;
    esac
done

[ -n "$DEVICE" ] || { echo "this needs a reader: --device IP" >&2; usage; }

cd "$(dirname "$0")/.."
[ -n "$OUT" ] || OUT="target/tour/$(date +%Y-%m-%d-%H%M%S)"

# Every point below was read off a screenshot of this panel at 1072x1448.
# The launcher pages nine tiles then three, and offers a direction only where
# there is a page in it, so the bottom bar has two controls on each page and
# not three. All of these moved when the "Continue reading" band came out.
L_AUDIOBOOKS="201,352";  L_SETTINGS="536,352";   L_GUTENBIRD="869,352"
L_TERMINAL="201,727";    L_COMPONENTS="536,727"; L_MAGNET="869,727"
L_TICTACTOE="201,1102";  L_BRIEF="536,1102";     L_TODO="869,1102"
L_MORE="800,1376";       L_PREVIOUS="267,1376"
L_CHAT="201,352";        L_HN="536,352";         L_FEEDS="869,352"

# The way out of an application, which is the way back into the launcher.
BACK="95,110"

# Waits are milliseconds before each tap. An e-ink refresh is most of a second,
# so these are tight where the reader is only drawing and generous at the two
# places where it talks to the internet.
TOUR="
2500:$L_AUDIOBOOKS 3000:530,250 4000:536,884 7000:866,884 3000:536,884
3000:$BACK 2500:$BACK

3000:$L_GUTENBIRD 7000:201,400 6000:536,1376 4000:$BACK 2500:$BACK

2500:$L_MORE

2500:$L_HN 8000:536,300 7000:$BACK 3000:$BACK

2500:$L_FEEDS 5000:536,300 6000:536,300 5000:$BACK 3000:$BACK 2500:$BACK

2500:$L_PREVIOUS

2500:$L_SETTINGS 3000:536,770 4000:$BACK 3000:536,525 4000:$BACK 2500:$BACK

2500:$L_TERMINAL 3000:400,1380 3000:300,1100 2000:500,1100 3000:$BACK

2500:$L_COMPONENTS 3000:536,300 3000:800,300 3000:$BACK

2500:$L_BRIEF 8000:$BACK

2500:$L_TODO 3000:536,755 3000:$BACK 2500:$BACK

2500:$L_TICTACTOE 2000:400,700 2000:670,700 2000:536,900 2500:$BACK
"

echo "building the CLI with device-write, for taps"
cargo build --release -q -p kobo-cli --features device-write
KOBO="target/release/kobo"

# Held awake for the whole run. Without this the reader sleeps partway through
# and the rest of the tour is of a blank panel.
echo "holding $DEVICE awake"
"$KOBO" session --device "$DEVICE" --keep-awake on >/dev/null
"$KOBO" session --device "$DEVICE" --wifi-always-on on >/dev/null 2>&1 || true

mkdir -p "$OUT"

# One launcher for the whole tour, given longer than the recording so it is
# still what is on the panel when the last frame is taken.
echo "presenting the launcher"
if ! "$KOBO" present launcher --device "$DEVICE" --seconds $((SECONDS_TOTAL + 60)) >/dev/null; then
    echo "the reader is probably still restarting; trying once more" >&2
    sleep 15
    "$KOBO" present launcher --device "$DEVICE" --seconds $((SECONDS_TOTAL + 60)) >/dev/null
fi
sleep 4

echo "recording ${SECONDS_TOTAL}s at ${FPS}fps while the tour runs"
"$KOBO" record --device "$DEVICE" --seconds "$SECONDS_TOTAL" --fps "$FPS" --out "$OUT/tour" &
recorder=$!

# One upload for the whole tour. The waits are honoured on the device, so this
# returns only once the last tap has been made.
# shellcheck disable=SC2086
"$KOBO" tap --device "$DEVICE" $TOUR ||
    echo "the tour stopped early; what was recorded is still worth looking at" >&2

wait "$recorder" || { echo "the recording failed" >&2; exit 1; }
"$KOBO" stop --device "$DEVICE" >/dev/null 2>&1 || true

# Handed back deliberately. A reader left with a wake lock does not sleep, and
# the owner finds a flat battery in the morning.
echo "releasing the wake lock"
"$KOBO" session --device "$DEVICE" --keep-awake off >/dev/null 2>&1 || true

echo
echo "the tour is in $OUT/tour"
if [ "$SPEED" != "1" ] && command -v ffmpeg >/dev/null 2>&1 && [ -f "$OUT/tour/recording.mp4" ]; then
    # E-ink is honestly slow and honest footage of it is slow to watch. Every
    # frame and its order are real; only the clock is compressed.
    ffmpeg -nostdin -y -loglevel error -i "$OUT/tour/recording.mp4" \
        -filter:v "setpts=PTS/$SPEED" -an "$OUT/cobalt-tour.mp4"
    echo "and at ${SPEED}x in $OUT/cobalt-tour.mp4"
    ffprobe -v error -show_entries format=duration -of default=nw=1:nk=1 "$OUT/cobalt-tour.mp4" |
        awk '{printf "it runs for %d:%02d\n", $1/60, $1%60}'
fi

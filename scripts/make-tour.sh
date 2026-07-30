#!/bin/sh
#
# Joins the per-application recordings into one tour of the whole system.
#
# `scripts/record-apps.sh` leaves a directory per application, each with its
# own mp4. That is the right shape for looking at one application closely and
# the wrong shape for showing somebody what Cobalt is, which needs one file
# they can press play on.
#
# Each clip is introduced by a title card naming the application, so the tour
# is legible without a commentary track. The cards are drawn at the panel's own
# size in the panel's own colours, because a tour of an e-ink reader that
# flashes white boxes between clips looks like a fault.
#
# usage: scripts/make-tour.sh DIR [--out FILE] [--seconds N]
#
#   DIR   a directory left by record-apps.sh
#
# Needs ffmpeg, which record-apps.sh already needs to make the clips at all.

set -eu

DIR=""
OUT=""
CARD=1.2

while [ $# -gt 0 ]; do
    case "$1" in
        --out) OUT="${2:?--out needs a file}"; shift 2 ;;
        --seconds) CARD="${2:?--seconds needs a number}"; shift 2 ;;
        -h|--help) sed -n '2,18p' "$0" | sed 's/^# \{0,1\}//'; exit 2 ;;
        *) DIR="$1"; shift ;;
    esac
done

[ -n "$DIR" ] || { echo "this needs a recording directory: scripts/make-tour.sh DIR" >&2; exit 2; }
[ -d "$DIR" ] || { echo "no such directory: $DIR" >&2; exit 1; }
command -v ffmpeg >/dev/null || { echo "this needs ffmpeg on the path" >&2; exit 1; }

[ -n "$OUT" ] || OUT="$DIR/cobalt-tour.mp4"

# The order is a walk through the system rather than the alphabet: the way in
# first, then the applications that show most of what the SDK can do, then the
# ones that exist to prove a single point.
ORDER="launcher gutenbird audiobook hn rss brief chat terminal settings todo magnet tictactoe gallery"

title_for() {
    case "$1" in
        launcher)  echo "Launcher" ;;
        gutenbird) echo "Gutenbird" ;;
        audiobook) echo "Audiobooks" ;;
        hn)        echo "Hacker News" ;;
        rss)       echo "Feeds" ;;
        brief)     echo "Daily Brief" ;;
        chat)      echo "AI Chat" ;;
        terminal)  echo "Terminal" ;;
        settings)  echo "Settings" ;;
        todo)      echo "Todo" ;;
        magnet)    echo "Magnet" ;;
        tictactoe) echo "Tic-tac-toe" ;;
        gallery)   echo "Components" ;;
        *)         echo "$1" ;;
    esac
}

WORK="$DIR/.tour"
rm -rf "$WORK"
mkdir -p "$WORK"
LIST="$WORK/parts.txt"
: > "$LIST"

# The panel is 1072x1448 and h264 wants even numbers, which both of those are.
WIDTH=1072
HEIGHT=1448

found=0
for app in $ORDER; do
    clip="$DIR/$app/recording.mp4"
    [ -f "$clip" ] || { echo "no clip for $app, skipping"; continue; }
    found=$((found + 1))
    title=$(title_for "$app")

    # A card is a still, encoded to the same shape and rate as the clips so
    # the concat demuxer can take both without re-encoding either.
    card="$WORK/$app-card.mp4"
    ffmpeg -v error -y -f lavfi -i "color=c=white:s=${WIDTH}x${HEIGHT}:r=10:d=$CARD" \
        -vf "drawtext=text='$title':fontcolor=black:fontsize=64:x=(w-text_w)/2:y=(h-text_h)/2" \
        -c:v libx264 -pix_fmt yuv420p "$card"

    # Re-encoded to identical parameters for the same reason. The clips come
    # out of `kobo record` with per-frame durations, and a concat that trusts
    # them to already match produces a file that plays for one clip and then
    # stops.
    part="$WORK/$app-clip.mp4"
    ffmpeg -v error -y -i "$clip" -r 10 -s "${WIDTH}x${HEIGHT}" \
        -c:v libx264 -pix_fmt yuv420p "$part"

    echo "file '$(basename "$card")'" >> "$LIST"
    echo "file '$(basename "$part")'" >> "$LIST"
done

[ "$found" -gt 0 ] || { echo "there were no clips in $DIR" >&2; exit 1; }

ffmpeg -v error -y -f concat -safe 0 -i "$LIST" -c copy "$OUT"
rm -rf "$WORK"

echo "toured $found applications into $OUT"
ffprobe -v error -show_entries format=duration -of default=nw=1:nk=1 "$OUT" |
    awk '{printf "it runs for %d:%02d\n", $1/60, $1%60}'

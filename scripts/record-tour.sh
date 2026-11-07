#!/bin/sh
#
# Records the tour on the front page: one continuous take of a Kobo Clara BW,
# driven from the launcher the way somebody holding it would.
#
# usage: scripts/record-tour.sh --device IP [--out DIR] [--seconds N] [--fps F]
#
# # What the tour shows, and why
#
# The launcher first, paged through, because a shelf of apps that each look
# like what they do is the first thing that says this is not a toy. Then a game
# of chess against the computer, because it is the most complete thing a reader
# can do here that its own software cannot: a real board, legal moves shown as
# dots, a computer that answers, and the move line saying what just happened.
# Then a crossword, to end on something calm and readable.
#
# Settings is left out on purpose: every screen in it names a Wi-Fi network,
# the reader's own and the neighbours', which is fine on a desk and not on the
# front page of a public repository. So are Books and Daily Brief: one shows
# whatever files are on this reader, the other whatever the news is that day.
#
# # Before running it
#
# - The reader is in Cobalt on the launcher, charging, with Lichess not in the
#   middle of a game (finish or abort it), so Play the computer starts afresh.
# - The opening moves are fixed: e4, Nf3, Bc4. They stay legal whatever the
#   computer answers in its first two moves: no black piece can reach f3, e2
#   or d3 that soon, a black pawn on c4 is simply taken, and the pawns on d2
#   and f2 block every check it could give.
#
# # Afterwards
#
# Look at the frames and cut them with scripts/cut-tour.py, naming the runs to
# keep. A take always has a frame or two caught mid-refresh, and those are left
# out by reading them, not by guessing here.
#
# Read-only apart from the taps. `kobo record` opens the framebuffer for
# reading and never grabs, refreshes or writes it.

set -eu

DEVICE=""
OUT=""
SECONDS_TOTAL=170
FPS=2

usage() { sed -n '2,36p' "$0" | sed 's/^# \{0,1\}//'; exit 2; }

while [ $# -gt 0 ]; do
    case "$1" in
        --device) DEVICE="${2:?--device needs an address}"; shift 2 ;;
        --out) OUT="${2:?--out needs a directory}"; shift 2 ;;
        --seconds) SECONDS_TOTAL="${2:?--seconds needs a count}"; shift 2 ;;
        --fps) FPS="${2:?--fps needs a rate}"; shift 2 ;;
        -h|--help) usage ;;
        *) echo "unknown option '$1'" >&2; usage ;;
    esac
done

[ -n "$DEVICE" ] || { echo "this needs a reader: --device IP" >&2; usage; }

cd "$(dirname "$0")/.."
[ -n "$OUT" ] || OUT="target/tour/$(date +%Y-%m-%d-%H%M%S)"

cargo build --release -q -p kobo-cli --features device-write
KOBO="target/release/kobo"

# Every point was read off a screenshot of a Clara BW's 1072x1448 panel on
# 0.3.35. The launcher shows nine tiles a page, alphabetically, so Lichess is
# the top right of page 3 and Crossword the middle of the bottom row of page 1.
APPS_TAB="134,1380"
NEXT_PAGE="965,1265"
BACK="97,109"
LICHESS="870,325"
CROSSWORD="536,1015"
PLAY_COMPUTER="536,454"
START_GAME="536,759"
# Board squares are 122 px from a1 at the bottom left (109,1207).
E2="597,1085"; E4="597,841"; G1="841,1207"; F3="719,963"; F1="719,1207"; C4="353,841"

tap() { "$KOBO" tap --device "$DEVICE" "$@" | grep -v '^tapped' || true; }

# The reader would sleep halfway through a take otherwise.
"$KOBO" session --device "$DEVICE" --hold 10 > /dev/null 2>&1 &
HOLD=$!
trap 'kill "$HOLD" 2>/dev/null || true' EXIT INT TERM

tap "$APPS_TAB"
sleep 3
mkdir -p "$OUT"
"$KOBO" record --device "$DEVICE" --seconds "$SECONDS_TOTAL" --fps "$FPS" --out "$OUT" > "$OUT.log" 2>&1 &
RECORDER=$!
sleep 6

tap "$NEXT_PAGE"; sleep 2
tap "$NEXT_PAGE"; sleep 2
tap "$LICHESS"; sleep 3
tap "$PLAY_COMPUTER"; sleep 2
tap "$START_GAME"; sleep 3
# A move is two taps: the piece, then where it goes. The pause after each
# move is the computer's answer, which takes a few seconds on this reader.
tap "$E2" "1200:$E4"; sleep 7
tap "$G1" "1200:$F3"; sleep 7
tap "$F1" "1200:$C4"; sleep 8
tap "$BACK"; sleep 2
tap "$BACK"; sleep 2
tap "$APPS_TAB"; sleep 2
tap "$CROSSWORD"; sleep 6

wait "$RECORDER"
echo "recorded into $OUT"
echo "cut it with: scripts/cut-tour.py $OUT --keep FIRST-LAST,..."

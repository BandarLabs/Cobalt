#!/bin/sh
#
# Records one continuous tour of Cobalt, driven from the launcher the way
# somebody holding the reader would drive it.
#
# usage: scripts/record-tour.sh --device IP [--out DIR] [--seconds N]
#                               [--fps F] [--speed X] [--lead S]
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
# The tour is built around one job that takes minutes. An audiobook is
# researched, written, narrated and packaged in about three minutes, so the
# first thing the tour does is start one, and then it leaves. Everything else
# happens while that book is being written, and the last thing the tour does is
# come back to the shelf and open it.
#
# That shape was chosen because it is the only one that shows what this system
# actually is. A tour that starts a book and waits for it is four minutes of a
# progress bar. A tour that shows each application in turn is a list. Starting
# something real, going away, and finding it done is the difference between a
# device that runs one program at a time and a device that runs a system, and
# it is a claim the README makes that nothing else here demonstrates.
#
# Within that, impressive first, because this is watched by somebody deciding
# whether to keep watching. Sixty thousand books with their covers, an agent
# stopping to ask permission, the news. The applications that exist to prove
# one narrow point sit in the middle, where they belong, and the two that are
# worth watching to a conclusion get one: the board is played until somebody
# wins, and the question put to the chat service is one this device can answer
# by sitting there.
#
# # Where the coordinates come from
#
# Out of the layout engine, not off a screenshot. A test lays each screen out
# at 1072x1448 against `Chrome::measuring`, which is the chrome the device
# draws including the status band, and prints the centre of every control. The
# band is 59 pixels tall and measuring without it puts every top anchored
# control that much too high, which is enough to miss a tab strip entirely.
# Guessed coordinates cost a seven minute recording each time they are wrong.
#
# # Why the taps are one invocation
#
# `kobo tap` takes a whole sequence and times it on the device. Sent one at a
# time, each tap costs a cross-compile, an upload of the tap binary and a
# checksum on the reader's own processor, so every wait was the wait asked for
# plus however long that took, and a tap meant to land while a screen was up
# landed after it had gone.
#
# # Why some of it is faster than it happened
#
# E-ink is slow and honest footage of it is slow to watch, but not evenly so.
# Watching somebody type an address one key at a time is not watching the
# system do anything; watching it find the feeds behind that address is. So
# every moment of the tour carries its own speed, the frames are retimed
# against the clock the recorder wrote down beside each one, and the parts that
# matter run at the speed they really ran at. No frame is invented and none is
# dropped: only how long each one is held changes.
#
# Read-only apart from the taps themselves. `kobo record` opens the framebuffer
# for reading and never grabs, refreshes or writes it.

set -eu

DEVICE=""
OUT=""
SECONDS_TOTAL=420
FPS=2
SPEED=1
LEAD=""

usage() { sed -n '2,48p' "$0" | sed 's/^# \{0,1\}//'; exit 2; }

while [ $# -gt 0 ]; do
    case "$1" in
        --device) DEVICE="${2:?--device needs an address}"; shift 2 ;;
        --out) OUT="${2:?--out needs a directory}"; shift 2 ;;
        --seconds) SECONDS_TOTAL="${2:?--seconds needs a count}"; shift 2 ;;
        --fps) FPS="${2:?--fps needs a rate}"; shift 2 ;;
        --speed) SPEED="${2:?--speed needs a multiplier}"; shift 2 ;;
        --lead) LEAD="${2:?--lead needs seconds}"; shift 2 ;;
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
# The second page gained Sidekick in the middle of its top row, which pushed
# Hacker News along and Feeds down onto a row of its own. The tour tapped
# Sidekick believing it was opening Hacker News for a while after that.
L_CHAT="201,352";        L_SIDEKICK="536,352";   L_HN="869,352"
L_FEEDS="201,727"

# The way out of an application, which is the way back into the launcher. Its
# mirror on the right is where a top bar's one verb goes: both controls are the
# same square inset by the same margin, so 95 from the left is 977 from it.
BACK="95,110"
AB_CREATE="977,110"
# The first book on the shelf, and the louder of the two volume keys under the
# transport. Play is deliberately not among these: with no headphones connected
# it goes to the Bluetooth pane, and that pane says plainly that leaving it
# restarts the reader, because Bluetooth and Wi-Fi share one radio here and it
# starts once per boot. A restart mid-tour ends the recording.
AB_SHELF_ONE="536,250"
AB_LOUDER="783,968"

# The one thing a feed shelf has to offer, centred along the bottom.
RSS_ADD="536,1380"

# A reading page carries no clock, so everything on it sits a status band
# higher than it does anywhere else and its way out is at 50 rather than 110.
# Tapping 110 there hits the first line of the article, which is why the tour
# used to stop dead on a newspaper story and make the rest of its taps into
# nothing: a page turn here, a page turn there, on a screen it never left.
READING_BACK="95,50"

# Allow, under a one-line command. A longer command pushes the buttons down,
# so the payload below is kept to one line to hold this still.
SK_ALLOW="536,477"

# The panel's own keyboard: three rows of letters, then a row of controls. Only
# the keys the tour actually presses are named, because a table of thirty
# points nobody reads is a table nobody notices is wrong.
K_E="288,940";   K_T="486,940";   K_Y="585,940";   K_I="783,940"
K_O="880,940"
K_S="205,1070";  K_K="865,1070";  K_L="975,1070"
K_C="425,1200";  K_N="755,1200";  K_M="865,1200"
# The full stop lives on the punctuation page, so an address costs a trip to
# ?123 and back to abc. This is the part of the tour nobody needs to watch at
# the speed it happened.
K_PUNCT="205,1330"; K_DOT="205,1200"; K_ABC="205,1330"
# The rightmost key of the bottom row, which is whatever verb the screen wants:
# Search on a feed search, Create on a composer, Enter in a terminal.
K_GO="865,1330"

# The keys the two new segments need, on the same three rows as the table
# above. Every point here was read out of the layout engine rather than off a
# screenshot: a test lays the screen out at 1072x1448 against the chrome the
# device actually draws and prints the centre of each control. That matters
# more than it sounds. Measuring without the status band puts every top
# anchored control 59 pixels high, and the tab strip below is exactly the kind
# of thing that would then miss by a whole row.
K_C_LOW="426,1200"; K_O_TOP="882,940"; K_M_LOW="866,1200"

# Components. Five destinations along the bottom, and a strip of tabs inside
# each one: three under Type and Work, four under the rest. Eighteen pages in
# all, which is the whole of what the SDK can draw.
G_TYPE="107,1377";  G_LISTS="321,1377"; G_INPUT="535,1377"
G_STATES="749,1377"; G_WORK="964,1377"
# Type is never tapped, because the gallery opens on it. It is named anyway so
# the five destinations read as five rather than as four and a gap.
G_THREE_TWO="536,234"; G_THREE_THREE="862,234"
G_FOUR_TWO="413,234";  G_FOUR_THREE="657,234"; G_FOUR_FOUR="902,234"

# The board, at the centre of each of its nine squares, and the three
# destinations of the chat panel.
T_R0C0="206,391";  T_R0C2="866,391"
T_R1C0="206,721";  T_R1C2="866,721"
T_R2C2="866,1051"

# Chat carries a navigation bar, and a bar along the bottom lifts the keyboard
# above it. Every key is 95 pixels higher here than on a screen without one,
# which is why this table exists rather than reusing the one above.
C_TALK="178,1377"; C_TYPE="535,1377"
C_W="189,845";  C_E="288,845";  C_I="783,845";  C_O="882,845"
C_A="96,975";   C_S="206,975";  C_D="316,975";  C_G="536,975"
C_H="646,975";  C_K="866,975";  C_L="976,975"
C_N="756,1105"; C_M="866,1105"
C_SPACE="536,1235"; C_SEND="866,1235"

# A real permission, in the shape Claude Code's PermissionRequest hook sends.
# Nothing runs as a result: the daemon holds the question, the reader answers
# it, and the answer goes back to a hook with no agent behind it.
SK_ASK='{"session_id":"tour","cwd":"/tmp","hook_event_name":"PermissionRequest","tool_name":"Bash","tool_input":{"command":"cargo test --workspace"}}'

# Each line of the tour is "speed | taps", and each tap is "millis:x,y" where
# the millis are waited before the tap lands. The speed is how fast that
# stretch is worth watching, and it holds from the line's first tap until the
# next line's first tap:
#
#   1  the payoff. Real time, because the claim is that this really happened.
#   3  getting somewhere. Tiles opening, screens closing, backing out.
#   8  typing. Nobody needs to watch a keyboard at reading speed.
#
# Comment and blank lines are stripped, so the reasoning can sit beside the
# coordinates instead of in a paragraph above them.
#
# The tour is in two halves, because the middle of it needs something to
# happen off the reader. Sidekick has nothing to show until an agent stops to
# ask, so the question in the recording is a real one, put through the same
# hook Claude Code calls, while the panel is watching for it.
tour_one() {
    cat <<TOUR
# An audiobook, started first and left to write itself.
#
# This is the spine of the whole recording. Research, a script, a narration and
# a package take about three minutes, and the tour does not sit and watch them:
# it types a subject, presses Create, backs out to the launcher and goes and
# does everything else. The book is being written on the reader throughout
# every segment that follows, and the last segment comes back to it finished.
# That is a claim this system makes and it is worth showing rather than saying.
3 | 2500:$L_AUDIOBOOKS
1 | 6000:$AB_CREATE
3 | 3000:$K_C_LOW
8 | 700:$K_O_TOP 700:$K_M_LOW 700:$K_E 700:$K_T 700:$K_S
1 | 1200:$K_GO 8000:$BACK

# Sixty thousand books, with their covers, opened on the glass.
3 | 2500:$L_GUTENBIRD
1 | 7000:201,400 6000:536,1376
3 | 4000:$BACK 2500:$BACK

# Settings: the battery, read off the gauge rather than guessed.
#
# The Wi-Fi screens are deliberately not here. A scan names the network this
# reader is on and every network within range of it, which on a desk is the
# whole point and in a published recording is the owner's address and their
# neighbours'. The connections screen names the joined one too, so the visit
# stops at the battery.
3 | 2500:$L_SETTINGS 3000:536,700 4000:$BACK 2500:$BACK

# A real /bin/sh, on the reader, hosted by the runtime. It types "ls", because
# a listing of a real root filesystem is the claim.
#
# The first key of every typed word waits with the navigation rather than with
# the typing. A keyboard that has only just been asked for is not yet listening,
# and a tap 700ms behind the screen that opened it lands on nothing.
3 | 2500:$L_TERMINAL 3000:400,1380 2500:$K_L
8 | 900:$K_S
1 | 3000:$K_GO
3 | 5000:$BACK

# Every primitive the SDK draws: five destinations, eighteen pages, no repeats.
#
# This used to be three taps and a back, which showed perhaps a fifth of what
# is there and made the richest screen in the tree look like the thinnest. Each
# page gets three seconds and the stretch runs at half the speed of the rest of
# the navigation, because the point of this segment is to be looked at rather
# than got through. Type and Work carry three tabs, the other three carry four.
3 | 2500:$L_COMPONENTS
2 | 3000:$G_THREE_TWO 3000:$G_THREE_THREE
2 | 3000:$G_LISTS 3000:$G_FOUR_TWO 3000:$G_FOUR_THREE 3000:$G_FOUR_FOUR
2 | 3000:$G_INPUT 3000:$G_FOUR_TWO 3000:$G_FOUR_THREE 3000:$G_FOUR_FOUR
2 | 3000:$G_STATES 3000:$G_FOUR_TWO 3000:$G_FOUR_THREE 3000:$G_FOUR_FOUR
2 | 3000:$G_WORK 3000:$G_THREE_TWO 3000:$G_THREE_THREE
3 | 3000:$BACK

# The front page, and a story off it. The launcher stays on whichever page it
# was left on, so the second page is asked for once and everything below is
# already there.
3 | 2500:$L_MORE 2500:$L_HN
1 | 8000:536,300
3 | 7000:$BACK 3000:$BACK

# And the agent panel, which is where the recording pauses for a real question.
3 | 2500:$L_SIDEKICK
TOUR
}

# The question arrives between the two halves, and this one answers it. Allow
# is the first button under a one-line command; the payload above keeps the
# command to one line so it stays there.
tour_two() {
    cat <<TOUR
# An agent stopped to ask, and the reader is where the answer comes from.
1 | 6000:$SK_ALLOW
3 | 4000:$BACK

# A question worth asking, put to a service from the panel.
#
# It asks how E Ink holds an image, on an E Ink screen, which is the one
# question this device can answer by sitting there. The keyboard here is 95
# pixels above the usual one because a navigation bar is under it.
#
# Sending goes straight back to the transcript and the answer arrives into it.
# The taps on Conversation while it waits are deliberate no-ops: the tour is
# driven by taps, so waiting for something to arrive means tapping where the
# reader already is.
3 | 2500:$L_CHAT 3000:$C_TYPE 2500:$C_H
8 | 700:$C_O 700:$C_W 700:$C_SPACE 700:$C_D 700:$C_O 700:$C_E 700:$C_S
8 | 700:$C_SPACE 700:$C_E 700:$C_SPACE 700:$C_I 700:$C_N 700:$C_K
8 | 700:$C_SPACE 700:$C_H 700:$C_O 700:$C_L 700:$C_D 700:$C_SPACE
8 | 700:$C_A 700:$C_N 700:$C_SPACE 700:$C_I 700:$C_M 700:$C_A
8 | 700:$C_G 700:$C_E
1 | 1500:$C_SEND 14000:$C_TALK
2 | 12000:$C_TALK
3 | 4000:$BACK

# Feeds. Not one that was already there: the address of a newspaper, typed on
# the panel, searched for, followed and read. This is the one that shows the
# reader doing something for somebody rather than showing itself off.
#
# A reading page carries no clock, so its way out is 60 pixels higher than
# everywhere else, which is what READING_BACK is for.
3 | 2500:$L_FEEDS 5000:$RSS_ADD 3000:$K_N
8 | 700:$K_Y 700:$K_T 700:$K_I 700:$K_M 700:$K_E 700:$K_S
8 | 700:$K_PUNCT 700:$K_DOT 700:$K_ABC 700:$K_C 700:$K_O 700:$K_M
1 | 1200:$K_GO 15000:536,300 9000:536,300
3 | 8000:$READING_BACK 2500:$BACK 2500:$BACK 2500:$BACK 2500:$BACK 2000:$BACK

# Back to the first page for the three small ones.
3 | 2500:$L_PREVIOUS 2500:$L_BRIEF
1 | 8000:$BACK

# State that survives being closed.
3 | 2500:$L_TODO 3000:536,755 3000:$BACK 2500:$BACK

# A game played to a result, rather than three taps into the middle of a board.
#
# Nought moves first, so noughts take the right hand column and crosses answer
# down the left. Five taps and the board says who won. The old three taps left
# it unfinished, and two of them landed in the same square because they were
# guessed off a screenshot rather than measured.
3 | 2500:$L_TICTACTOE
1 | 2000:$T_R0C2 2000:$T_R0C0 2000:$T_R1C2 2000:$T_R1C0 2500:$T_R2C2
1 | 6000:$BACK

# And the end of it: the book that was started in the first minute, finished.
#
# Nothing here is staged. The shelf is read off the reader's own filesystem,
# the book on it was written during the preceding segments, and the player is
# the one that plays it. This runs at real speed to the last frame, because
# the whole recording exists to say that this happened.
#
# Volume rather than play. Play with no headphones connected goes to the
# Bluetooth pane, and that pane says why it must not: the radio is shared with
# Wi-Fi, it starts once per boot, and the reader restarts itself on the way
# out. Two taps take it from 70% to 90%, visibly and without rebooting.
3 | 2500:$L_AUDIOBOOKS
1 | 8000:$AB_SHELF_ONE 8000:$AB_LOUDER 4000:$AB_LOUDER 8000:$AB_LOUDER
TOUR
}

# The taps, with the speeds and the commentary taken back out.
taps_of() {
    sed 's/#.*//' | sed 's/^[^|]*|//' | tr '\n' ' '
}

# The plan, as one "millis speed" line per tap, in the order they land.
plan_of() {
    sed 's/#.*//' | grep '|' | while IFS='|' read -r speed taps; do
        for tap in $taps; do
            printf '%s %s\n' "${tap%%:*}" $speed
        done
    done
}

TAPS_ONE=$(tour_one | taps_of)
TAPS_TWO=$(tour_two | taps_of)
PLAN_ONE="${TMPDIR:-/tmp}/kobo-tour-one.$$"
PLAN_TWO="${TMPDIR:-/tmp}/kobo-tour-two.$$"
trap 'rm -f "$PLAN_ONE" "$PLAN_TWO"' EXIT
tour_one | plan_of > "$PLAN_ONE"
tour_two | plan_of > "$PLAN_TWO"

SUM_ONE=$(awk '{total += $1} END {print total + 0}' "$PLAN_ONE")
SUM_TWO=$(awk '{total += $1} END {print total + 0}' "$PLAN_TWO")
PLANNED=$(( (SUM_ONE + SUM_TWO) / 1000 ))
echo "the tour is ${PLANNED}s of taps, inside a ${SECONDS_TOTAL}s recording"
[ "$PLANNED" -lt "$SECONDS_TOTAL" ] ||
    { echo "the tour is longer than the recording; raise --seconds" >&2; exit 2; }

echo "building the CLI with device-write, for taps"
cargo build --release -q -p kobo-cli --features device-write
# The daemon too, because the tour asks Sidekick a question partway through.
cargo build --release -q -p kobo-sidekickd
KOBO="target/release/kobo"
HOOK="target/release/kobo-sidekickd"

# Sidekick's segment is the one that fails silently. The panel says "Watching"
# whether the daemon is down, the pairing is stale or the question simply has
# not been asked yet, so none of it shows up until the recording is reviewed.
# Both halves are therefore checked here, where there is somewhere to print.
#
# A pairing goes stale on its own: the reader remembers an address, and DHCP
# hands this machine a different one every few days. Re-pair by writing the
# daemon's address and code into the store the app reads, which is the same
# two lines the pairing screens write, and note that the code has no trailing
# newline because the app takes everything after the first one as the code.
PAIRED_FILE="/mnt/onboard/.adds/cobalt/state/sidekick/paired"
if ! nc -z 127.0.0.1 9330 2>/dev/null; then
    echo "no sidekick daemon is listening; start one with 'kobo-sidekickd run'" >&2
    echo "the tour will run, but its Sidekick segment will stay on Watching" >&2
elif LAN=$(ipconfig getifaddr en0 2>/dev/null || hostname -I 2>/dev/null | awk '{print $1}') &&
    [ -n "$LAN" ]; then
    REMEMBERED=$("$KOBO" shell --device "$DEVICE" "cat $PAIRED_FILE" 2>/dev/null | head -1)
    case "$REMEMBERED" in
    "$LAN":*) ;;
    *)
        echo "the reader is paired with '${REMEMBERED:-nothing}', but this machine is $LAN" >&2
        echo "re-pair it, or the Sidekick segment stays on Watching:" >&2
        echo "  kobo-sidekickd init && kobo trust set sidekick --device $DEVICE" >&2
        printf '  kobo shell --device %s "printf %s%s:9331\\nCODE%s > %s"\n' \
            "$DEVICE" "'" "$LAN" "'" "$PAIRED_FILE" >&2
        ;;
    esac
fi

# Held awake for the whole run. Without this the reader sleeps partway through
# and the rest of the tour is of a blank panel.
echo "holding $DEVICE awake"
"$KOBO" session --device "$DEVICE" --keep-awake on >/dev/null
"$KOBO" session --device "$DEVICE" --wifi-always-on on >/dev/null 2>&1 || true

mkdir -p "$OUT"

# One launcher for the whole tour, given longer than the recording so it is
# still what is on the panel when the last frame is taken.
echo "presenting the launcher"
if ! "$KOBO" present launcher --device "$DEVICE" --seconds $((SECONDS_TOTAL + 90)) >/dev/null; then
    echo "the reader is probably still restarting; trying once more" >&2
    sleep 15
    "$KOBO" present launcher --device "$DEVICE" --seconds $((SECONDS_TOTAL + 90)) >/dev/null
fi
sleep 4

echo "recording ${SECONDS_TOTAL}s at ${FPS}fps while the tour runs"
REC_LAUNCH=$(date +%s)
"$KOBO" record --device "$DEVICE" --seconds "$SECONDS_TOTAL" --fps "$FPS" --out "$OUT/tour" &
recorder=$!

# One upload per half. The waits are honoured on the device, so each of these
# returns only once its last tap has been made. The clock is read around each
# one, because the frames are timed from when the reader started filming and
# the taps are timed from when their upload finished, and the retiming at the
# end has to lay those two clocks on top of each other.
ONE_START=$(date +%s)
# shellcheck disable=SC2086
"$KOBO" tap --device "$DEVICE" $TAPS_ONE ||
    echo "the tour stopped early; what was recorded is still worth looking at" >&2
ONE_END=$(date +%s)

# Sidekick is on the panel now and has nothing to show, because nothing has
# asked it anything. So something asks. This is the daemon's own hook, given
# the payload Claude Code would have given it, and it blocks until the reader
# answers exactly as it does for a real agent. Backgrounded so the taps that
# answer it can run.
#
# If there is no daemon the hook fails immediately, the panel stays on
# Watching, and the tap meant for Allow lands on nothing. The tour carries on
# either way; a recording missing one screen is worth more than no recording.
if [ -x "$HOOK" ]; then
    printf '%s' "$SK_ASK" | "$HOOK" hook claude >/dev/null 2>&1 &
else
    echo "no sidekick daemon built, so the tour has nothing to ask it" >&2
fi

TWO_START=$(date +%s)
# shellcheck disable=SC2086
"$KOBO" tap --device "$DEVICE" $TAPS_TWO ||
    echo "the tour stopped early; what was recorded is still worth looking at" >&2
TWO_END=$(date +%s)

# A failure here is almost never a failure to record. `kobo record` captures
# into /mnt/onboard/.kobo-record.bin and only deletes it once it has been
# pulled home, so a broken pull leaves a complete recording on the reader and
# reports the same non-zero status as a capture that never happened. That
# distinction cost hours once: the tour had run correctly on the panel, three
# further attempts wrote empty directories, and the 397MB original was sitting
# on the device the whole time. Say so, rather than let the next person read
# this as "start again".
wait "$recorder" || {
    echo "the pull or the capture failed" >&2
    echo "check the reader before re-running: if /mnt/onboard/.kobo-record.bin" >&2
    echo "is still there it is the whole recording, and it comes home in 16MB" >&2
    echo "chunks with dd and gzip -1. One long stream over this radio dies to" >&2
    echo "any hiccup and starts again from nothing." >&2
    exit 1
}
"$KOBO" stop --device "$DEVICE" >/dev/null 2>&1 || true

# Handed back deliberately. A reader left with a wake lock does not sleep, and
# the owner finds a flat battery in the morning.
echo "releasing the wake lock"
"$KOBO" session --device "$DEVICE" --keep-awake off >/dev/null 2>&1 || true

echo
echo "the tour is in $OUT/tour"

TIMINGS="$OUT/tour/timings.txt"
if ! command -v ffmpeg >/dev/null 2>&1 || [ ! -f "$TIMINGS" ]; then
    echo "no ffmpeg or no timings, so the frames are all there is"
    exit 0
fi

# How long each half spent uploading and starting a tap binary rather than
# tapping. Measured rather than guessed, because it depends on the link, and
# everything downstream is placed relative to it.
OVERHEAD_ONE=$(awk -v elapsed=$((ONE_END - ONE_START)) -v planned="$SUM_ONE" \
    'BEGIN {print elapsed - planned / 1000}')
OVERHEAD_TWO=$(awk -v elapsed=$((TWO_END - TWO_START)) -v planned="$SUM_TWO" \
    'BEGIN {print elapsed - planned / 1000}')
# And how long the *recorder* spent doing the same before its first frame,
# which is the one number nothing reports. A recording and a tap are the same
# errand over the same link with a binary of about the same size, so the tap
# upload that was measured stands in for the one that was not. --lead overrides
# it when a run comes back visibly out of step.
[ -n "$LEAD" ] || LEAD="$OVERHEAD_ONE"
echo "tap uploads took ${OVERHEAD_ONE}s and ${OVERHEAD_TWO}s; assuming ${LEAD}s before the first frame"

BASE_ONE=$(awk -v start=$((ONE_START - REC_LAUNCH)) -v overhead="$OVERHEAD_ONE" -v lead="$LEAD" \
    'BEGIN {print start + overhead - lead}')
BASE_TWO=$(awk -v start=$((TWO_START - REC_LAUNCH)) -v overhead="$OVERHEAD_TWO" -v lead="$LEAD" \
    'BEGIN {print start + overhead - lead}')

# Where each stretch of the tour begins, in the recorder's own clock.
{
    # Before the first tap there is only the launcher, sitting still.
    echo "0 3"
    awk -v base="$BASE_ONE" '{at += $1 / 1000; print base + at, $2}' "$PLAN_ONE"
    awk -v base="$BASE_TWO" '{at += $1 / 1000; print base + at, $2}' "$PLAN_TWO"
} > "$OUT/segments.txt"

# Retimed frame by frame against the clock written down beside each one. A
# frame is held for as long as it really was on the panel, divided by how fast
# that moment is worth watching, and never for less than a fortieth of a
# second, because a keystroke shown for no time is a keystroke that was cut.
awk -v speed="$SPEED" '
    NR == FNR { start[++segments] = $1; rate[segments] = $2; next }
    { name[++frames] = $1; at[frames] = $2 / 1000 }
    END {
        for (frame = 1; frame <= frames; frame++) {
            held = (frame < frames ? at[frame + 1] - at[frame] : 1)
            fast = rate[1]
            for (segment = 1; segment <= segments; segment++)
                if (start[segment] <= at[frame]) fast = rate[segment]
            shown = held / (fast * speed)
            if (shown < 0.025) shown = 0.025
            printf "file '\''%s'\''\nduration %.3f\n", name[frame], shown
        }
        printf "file '\''%s'\''\n", name[frames]
    }
' "$OUT/segments.txt" "$TIMINGS" > "$OUT/tour/edit.txt"

ffmpeg -nostdin -y -loglevel error -f concat -safe 0 -i "$OUT/tour/edit.txt" \
    -vf "pad=ceil(iw/2)*2:ceil(ih/2)*2,fps=30" -pix_fmt yuv420p \
    "$OUT/cobalt-tour.mp4"
echo "and edited, at the speed each part deserves, in $OUT/cobalt-tour.mp4"
ffprobe -v error -show_entries format=duration -of default=nw=1:nk=1 "$OUT/cobalt-tour.mp4" |
    awk '{printf "it runs for %d:%02d\n", $1/60, $1%60}'

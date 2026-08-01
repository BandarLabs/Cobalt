#!/bin/sh
#
# Retakes the screenshots in every example's README, on a real reader.
#
# The third script in the loop. `record-apps.sh` says what an application did,
# `record-tour.sh` says what the whole system feels like, and this says what
# each screen looks like right now.
#
# It exists because the stills went stale and nobody noticed. A typographic
# change lands, every README still shows the old weight, and the drift is
# invisible until somebody opens two of them side by side. A screenshot taken
# by hand is a screenshot taken once; this one is meant to be run again after
# anything that changes how a screen is drawn.
#
# Files are written back over the committed ones, under the names the READMEs
# already point at, so a run shows up as a diff on images rather than as a pile
# of new files somebody has to wire in.
#
# Read-only on the device apart from the taps, which are real: the applications
# genuinely move. Nothing else on the reader is touched.
#
# Some shots cannot be taken this way and are left alone deliberately. They are
# listed under BY_HAND below, with the reason.
#
# usage: scripts/shoot-apps.sh --device IP [--apps "settings todo"] [--dry-run]

set -eu

DEVICE=""
DRY=""
APPS="launcher settings todo tictactoe gallery terminal magnet hn rss gutenbird brief"

usage() {
    sed -n '2,25p' "$0" | sed 's/^# \{0,1\}//'
    exit 2
}

while [ $# -gt 0 ]; do
    case "$1" in
        --device) DEVICE="${2:?--device needs an address}"; shift 2 ;;
        --apps) APPS="${2:?--apps needs a list}"; shift 2 ;;
        --dry-run) DRY=yes; shift ;;
        -h|--help) usage ;;
        *) echo "unknown option '$1'" >&2; usage ;;
    esac
done

[ -n "$DEVICE" ] || { echo "this needs a reader: --device IP" >&2; usage; }

cd "$(dirname "$0")/.."

# Shots that need something this script cannot arrange:
#
#   magnet/counting      a magnet, walked along the edge of the panel by hand
#   settings/bluetooth   headphones that are paired, on, and in range
#   chat/*               a conversation with a real API key behind it
#   audiobook/*          a book that takes minutes to write and read aloud
#   sidekick/*           an agent on the desk actually stopping to ask
#
# All of them are real captures of things that really happened, which is why
# they are kept rather than regenerated as mock-ups.
BY_HAND="magnet/counting settings/bluetooth chat audiobook sidekick"

# Every point was read off a 1072x1448 panel. Back is the arrow in the top
# left, which is the way out of any screen an application opened.
BACK="95,110"

# The route through an application, as "name:settle:point" steps run in order.
# The tap lands first, then the settle in seconds, then the shot. A name of "-"
# takes no picture, which is how a step walks back out of a screen it has just
# photographed. A point of "-" taps nothing, which is how the screen an
# application opens on is photographed before anything is touched.
shots_for() {
    case "$1" in
        launcher)
            echo "home:2:- more-apps:3:800,1376" ;;
        # The root list, then each pane it opens and the way back out.
        settings)
            echo "connections:2:- wifi:6:536,475 -:3:$BACK battery:4:536,700" ;;
        # The list, then the keyboard the Add button raises. Add is the left
        # of two buttons, so the tap goes to its half of the row.
        todo)
            echo "list:2:- compose:4:285,730" ;;
        # Played to a finish. O takes the third column while X answers in the
        # first, which is the game the README describes.
        tictactoe)
            echo "-:2:869,400 -:2:201,400 -:2:869,700 -:2:201,700 game:4:869,1000" ;;
        # Two pages of the gallery: the type specimen, then the controls.
        gallery)
            echo "text:3:- controls:4:800,1380" ;;
        # A shell that has actually run something. The taps are the l and s
        # keys and then enter, so the listing on the panel is a real one made
        # by the reader's own /bin/sh rather than a picture of one.
        terminal)
            echo "-:3:975,1070 -:1:205,1070 shell:6:865,1330" ;;
        # Nothing to tap, and the empty state is the whole point of it.
        magnet)
            echo "no-magnet:3:-" ;;
        # A list that has to arrive over the network, then a story from it.
        hn)
            echo "stories:9:- thread:9:536,300" ;;
        rss)
            echo "articles:9:- search:5:400,1380" ;;
        gutenbird)
            echo "shelf:10:- book:8:201,400" ;;
        brief)
            echo "brief:12:-" ;;
        *)
            echo "" ;;
    esac
}

echo "building the CLI with device-write, for taps"
cargo build --release -q -p kobo-cli --features device-write
KOBO="target/release/kobo"

if [ -n "$DRY" ]; then
    for app in $APPS; do
        for step in $(shots_for "$app"); do
            name="${step%%:*}"; rest="${step#*:}"
            settle="${rest%%:*}"; point="${rest#*:}"
            [ "$point" = "-" ] || echo "$app: tap $point"
            [ "$name" = "-" ] ||
                echo "$app: after ${settle}s, examples/$app/screenshots/$name.png"
        done
    done
    echo
    echo "taken by hand and left alone: $BY_HAND"
    exit 0
fi

# Held awake for the whole run, or the reader sleeps partway through and the
# rest of the shots are of a blank panel.
echo "holding $DEVICE awake"
"$KOBO" session --device "$DEVICE" --keep-awake on >/dev/null
"$KOBO" session --device "$DEVICE" --wifi-always-on on >/dev/null 2>&1 || true

TAKEN=0
FAILED=""

for app in $APPS; do
    steps=$(shots_for "$app")
    [ -n "$steps" ] || { echo "no route through $app; skipping" >&2; continue; }

    echo
    echo "=== $app ==="
    # Given longer than the route needs, so the application is still on the
    # panel when the last shot is taken rather than handing it back early.
    if ! "$KOBO" present "$app" --device "$DEVICE" --seconds 240 >/dev/null; then
        echo "could not start $app; waiting for the reader and trying once more" >&2
        sleep 15
        if ! "$KOBO" present "$app" --device "$DEVICE" --seconds 240 >/dev/null; then
            echo "could not start $app; moving on" >&2
            FAILED="$FAILED $app"
            continue
        fi
    fi
    # An application is not on the panel the moment `present` returns; it is
    # still starting. Shooting into that gap photographs the screen before it.
    sleep 20

    mkdir -p "examples/$app/screenshots"
    for step in $steps; do
        name="${step%%:*}"; rest="${step#*:}"
        settle="${rest%%:*}"; point="${rest#*:}"

        [ "$point" = "-" ] || "$KOBO" tap --device "$DEVICE" "$point" >/dev/null
        sleep "$settle"
        [ "$name" != "-" ] || continue

        out="examples/$app/screenshots/$name.png"
        if "$KOBO" shot --device "$DEVICE" --out "$out" >/dev/null; then
            echo "  $out"
            TAKEN=$((TAKEN + 1))
            # Two of the settings screens are a list of the names of the
            # networks around whoever ran this, which is fine on a desk and
            # not fine in a public repository. Painted out here rather than
            # afterwards, so a re-shoot cannot quietly put them back: this
            # script is the only thing that writes these files, and the last
            # sweep did put them back.
            case "$app/$name" in
            settings/connections)
                python3 scripts/redact-ssids.py "$out" --line 499 ||
                    FAILED="$FAILED $app/$name(unredacted)" ;;
            settings/wifi)
                python3 scripts/redact-ssids.py "$out" \
                    --line 410 --line 588 --line 740 --line 894 --line 1047 ||
                    FAILED="$FAILED $app/$name(unredacted)" ;;
            esac
        else
            echo "could not shoot $out" >&2
            FAILED="$FAILED $app/$name"
        fi
    done

    "$KOBO" stop --device "$DEVICE" >/dev/null 2>&1 || true
    # The reader restarts its own software after the panel goes back, and a
    # `present` that arrives during that restart is refused.
    sleep 8
done
# Handed back deliberately. A reader left with a wake lock does not sleep, and
# the owner finds a flat battery in the morning.
echo
echo "releasing the wake lock"
"$KOBO" session --device "$DEVICE" --keep-awake off >/dev/null 2>&1 || true

echo
echo "took $TAKEN screenshots"
echo "left alone, taken by hand: $BY_HAND"
if [ -n "$FAILED" ]; then
    echo "these did not come out:$FAILED" >&2
    exit 1
fi

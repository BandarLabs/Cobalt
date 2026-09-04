#!/bin/sh
#
# Records every application in the simulator, with no reader involved.
#
# The companion to `record-apps.sh`, which does the same thing on real glass.
# That one needs a reader that is awake, on the network, and not being used by
# anybody else, and it takes six minutes to find out that it was none of those.
# This one needs a laptop. Every frame comes from the same renderer, the same
# layout engine, the same hit-testing and the same refresh planner the device
# runs, so the recordings are of the real thing; what is missing is the panel,
# the radio and the reader's own software.
#
# That trade is the whole point. A demo for a new application can be made on
# the day the application is written, by whoever wrote it, rather than queued
# behind the one reader on the desk.
#
# Each application is started in its own simulator, driven by its committed
# drive script, and filmed. An application with no drive script is recorded on
# its opening screen and reported, because a route through an application is
# something only that application's author can write.
#
# Nothing committed is written. Recordings go to a dated directory under
# `target/sim-recordings/`, and an --out that names a directory holding
# committed files is refused unless --overwrite says otherwise.
#
# --fresh gives every application a store of its own, so a run starts from the
# state the application ships with rather than from whatever the last run left
# behind. The simulator keeps an application's store in the host's temporary
# directory, which means a second run of a drive script resumes a saved game,
# opens a list that already has things on it, and fails an `expect` that passed
# the first time. The cost is that credentials installed with `kobo secret`
# live in the ordinary temporary directory and are not visible from the new
# one, so an application that needs an API key should be recorded without it.
#
# usage: scripts/record-apps-sim.sh [--out DIR] [--apps "todo gallery"]
#                                   [--fps N] [--ghosting] [--overwrite]
#                                   [--address host:port] [--fresh] [--list]

set -eu

OUT=""
APPS=""
FPS=4
GHOSTING=""
OVERWRITE=""
ADDRESS="127.0.0.1:8787"
FRESH=""
LIST=""

# How long an application is given to compile and reach its first screen. A
# cold workspace is minutes, not seconds, and a timeout that assumed otherwise
# would report every application as broken on the one run that matters.
START_TIMEOUT=300

usage() {
    sed -n '2,39p' "$0" | sed 's/^# \{0,1\}//'
    exit 2
}

while [ $# -gt 0 ]; do
    case "$1" in
        --out) OUT="${2:?--out needs a directory}"; shift 2 ;;
        --apps) APPS="${2:?--apps needs a list}"; shift 2 ;;
        --fps) FPS="${2:?--fps needs a rate}"; shift 2 ;;
        --address) ADDRESS="${2:?--address needs host:port}"; shift 2 ;;
        --ghosting) GHOSTING="--ghosting"; shift ;;
        --overwrite) OVERWRITE=yes; shift ;;
        --fresh) FRESH=yes; shift ;;
        --list) LIST=yes; shift ;;
        -h|--help) usage ;;
        *) echo "unknown option '$1'" >&2; usage ;;
    esac
done

cd "$(dirname "$0")/.."

command -v ffmpeg >/dev/null 2>&1 || {
    echo "ffmpeg is not on the path, and a recording is assembled with it." >&2
    echo "install it with: brew install ffmpeg (macOS) or apt install ffmpeg (Debian)" >&2
    exit 1
}

# Every application the catalog ships, in the order it lists them, rather than
# a list kept here. A list kept here is a list that is right on the day it is
# written: the applications that landed afterwards are the ones with no demo,
# which is exactly the ones a demo is for.
catalog_apps() {
    sed -n 's/^ *"id": *"\([a-z0-9-]*\)".*/\1/p' apps/catalog.json
}

# Where an application's source lives. The catalog does not say, because the
# split between an example and a shipped application is a repository layout
# question and not something the reader has any business knowing.
source_of() {
    for root in apps examples; do
        if [ -d "$root/$1" ]; then
            echo "$root/$1"
            return 0
        fi
    done
    return 1
}

# The route through an application, as a drive script. `.kobo` is the
# extension the SDK documents; `.txt` is what the applications in the tree
# happen to use. Both are the same format, so both are accepted rather than
# making somebody rename a file to get a demo.
script_of() {
    for name in drive.kobo drive.txt; do
        if [ -f "$1/$name" ]; then
            echo "$1/$name"
            return 0
        fi
    done
    return 1
}

[ -n "$APPS" ] || APPS=$(catalog_apps)

if [ -n "$LIST" ]; then
    for app in $APPS; do
        directory=$(source_of "$app") || { echo "$app	no source in this tree"; continue; }
        script=$(script_of "$directory") ||
            { echo "$app	$directory	opening screen only"; continue; }
        echo "$app	$directory	$script"
    done
    exit 0
fi

# Dated, because the point of a recording is comparing today against last time,
# and because a run that overwrote the last one would make that impossible.
[ -n "$OUT" ] || OUT="target/sim-recordings/$(date +%Y-%m-%d-%H%M%S)"

# Committed screenshots and the media on the website are the only copy of
# themselves. A run pointed at one of those directories by accident -- a stale
# --out in somebody's shell history is all it takes -- would replace them with
# whatever the simulator drew this morning.
if [ -z "$OVERWRITE" ] && [ -d "$OUT" ] &&
   [ -n "$(git ls-files -- "$OUT" 2>/dev/null | head -n 1)" ]; then
    echo "$OUT holds committed files; pass --overwrite if that is really meant" >&2
    exit 1
fi

echo "building the CLI"
cargo build --release -q -p kobo-cli
KOBO="$PWD/target/release/kobo"

mkdir -p "$OUT"
RECORDED=0
FAILED=""
NO_SCRIPT=""

for app in $APPS; do
    directory=$(source_of "$app") || {
        echo "no source for $app in this tree; skipping" >&2
        continue
    }
    echo
    echo "=== $app ($directory) ==="

    log="$OUT/$app.log"
    # Started from the application's own directory, which is how `kobo dev`
    # decides what to build and run.
    if [ -n "$FRESH" ]; then
        mkdir -p "$OUT/$app-store"
        store=$(cd "$OUT/$app-store" && pwd)
        (cd "$directory" && TMPDIR="$store" exec "$KOBO" dev "$ADDRESS") > "$log" 2>&1 &
    else
        (cd "$directory" && exec "$KOBO" dev "$ADDRESS") > "$log" 2>&1 &
    fi
    simulator=$!

    # The address line is printed once the application has compiled, started
    # and connected, so it is the only honest signal that there is a screen to
    # film. Polling the port would connect to a simulator with nothing in it.
    waited=0
    while ! grep -q "Kobo app simulator:" "$log" 2>/dev/null; do
        if ! kill -0 "$simulator" 2>/dev/null; then
            echo "$app did not start; see $log" >&2
            FAILED="$FAILED $app"
            break
        fi
        if [ "$waited" -ge "$START_TIMEOUT" ]; then
            echo "$app did not reach a screen in ${START_TIMEOUT}s; see $log" >&2
            FAILED="$FAILED $app"
            break
        fi
        sleep 1
        waited=$((waited + 1))
    done

    # Connected is not the same as drawn. The address line goes out when the
    # application opens its socket, and its first screen arrives some
    # milliseconds later; filming from the earlier moment opens every recording
    # on a blank panel and fails the first `expect` about one run in three.
    # `dump` prints one line per node that has words on it, so a line with a
    # bracket in it is a screen.
    if grep -q "Kobo app simulator:" "$log" 2>/dev/null; then
        painted=0
        while [ "$painted" -lt 30 ]; do
            "$KOBO" drive --address "$ADDRESS" --step dump 2>/dev/null |
                grep -q '\["' && break
            sleep 1
            painted=$((painted + 1))
        done
    fi

    if grep -q "Kobo app simulator:" "$log" 2>/dev/null; then
        if script=$(script_of "$directory"); then
            set -- --script "$script"
        else
            # No route through it, so this is the opening screen and a note.
            # Writing a route is a job for whoever knows what the application
            # is meant to do, and guessing at coordinates produces a demo of
            # taps landing on nothing.
            set -- --step "wait 2000"
            NO_SCRIPT="$NO_SCRIPT $app"
        fi
        # GHOSTING is deliberately unquoted: empty means the flag is absent.
        # shellcheck disable=SC2086
        if "$KOBO" drive --address "$ADDRESS" --record "$OUT/$app" --fps "$FPS" \
                $GHOSTING --shots "$OUT/$app-shots" "$@"; then
            RECORDED=$((RECORDED + 1))
        else
            echo "driving $app failed; the recording up to the failure is in $OUT/$app" >&2
            FAILED="$FAILED $app"
        fi
    fi

    # The application is a child of the simulator, and a simulator taken down
    # by a signal does not get to run the code that reaps it.
    children=$(pgrep -P "$simulator" 2>/dev/null || true)
    kill "$simulator" 2>/dev/null || true
    wait "$simulator" 2>/dev/null || true
    for child in $children; do
        kill "$child" 2>/dev/null || true
    done
done

echo
echo "recorded $RECORDED applications into $OUT"
[ -z "$NO_SCRIPT" ] ||
    echo "opening screen only, no drive script committed:$NO_SCRIPT"
if [ -n "$FAILED" ]; then
    echo "these did not record:$FAILED" >&2
    exit 1
fi

#!/bin/sh
# Drives a real reader through the moments in tools/device-shots.moments and
# saves a picture of each, one folder per app, for the website and for
# marketing. Run it again after a release and the same screens come back.
#
#   tools/device-shots.sh --device IP [--out DIR] [--only ID[,ID...]]
#                         [--moments FILE] [--step STEP ...]
#
# --device   the reader's address on this network (or KOBO_DEVICE)
# --out      where the pictures go (default ~/Downloads/cobalt-marketing/device)
# --only     run just these apps' blocks from the moments file
# --step     run steps given here instead of the file, for finding a moment:
#            --step 'open lichess' --step 'shot look'
#
# The reader has to be awake, on Wi-Fi, running Cobalt, and reachable over
# SSH with the key `kobo setup` installed. Taps go to the real panel through
# `kobo tap`, so the reader's screen changes while this runs. The moments
# are measured on a Kobo Clara BW (1072 x 1448) at the default text size.
#
# Environment:
#   KOBO           the kobo command to use (default: kobo on PATH); it needs
#                  `kobo tap`, which a build with --features device-write has
#   KOBO_SSH_ALIAS the address the reader's host key is saved under, when the
#                  reader has since moved to another address
#
# Steps, one per line in the moments file:
#   app ID         starts a block; pictures go to OUT/ID/
#   open ID        go home, open the app from the launcher, check it opened
#   tap X,Y        tap the panel
#   wait SECONDS   let the screen settle
#   shot NAME      save OUT/<block>/NN-NAME.png
#   home           close the app in front and return to the launcher
#   tab NAME       on the launcher, tap the Apps, Books or Settings tab
#   host COMMAND   run COMMAND on this computer (a companion, say)
#   # ...          a comment
set -eu
# Steps are split on spaces; nothing in them is a file pattern.
set -f

here=$(cd "$(dirname "$0")" && pwd)
device=${KOBO_DEVICE:-}
out="$HOME/Downloads/cobalt-marketing/device"
moments="$here/device-shots.moments"
only=""
steps=""
nl='
'
while [ $# -gt 0 ]; do
  case "$1" in
    --device) device=$2; shift 2 ;;
    --out) out=$2; shift 2 ;;
    --only) only=",$2,"; shift 2 ;;
    --moments) moments=$2; shift 2 ;;
    --step) steps="$steps$2$nl"; shift 2 ;;
    -h|--help) sed -n '2,38p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
[ -n "$device" ] || { echo "give the reader's address with --device IP" >&2; exit 2; }
kobo=${KOBO:-kobo}
"$kobo" tap --help >/dev/null 2>&1 || {
  echo "$kobo has no 'tap'; build it with: cargo build -p kobo-cli --features device-write" >&2
  exit 2
}

# kobo shot and kobo tap call ssh themselves. A reader whose address moved is
# still the same reader, so its saved host key is checked under the old name.
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
if [ -n "${KOBO_SSH_ALIAS:-}" ]; then
  mkdir -p "$work/bin"
  real_ssh=$(command -v ssh)
  printf '#!/bin/sh\nexec %s -o HostKeyAlias=%s "$@"\n' "$real_ssh" "$KOBO_SSH_ALIAS" > "$work/bin/ssh"
  chmod +x "$work/bin/ssh"
  PATH="$work/bin:$PATH"
  export PATH
fi

# The reader's login shell ignores a command given as an argument, so every
# remote script goes in on stdin.
# It is the key `kobo setup` installed, as the kobo command uses.
on_reader() {
# A reader that falls asleep or off Wi-Fi mid-run would otherwise hold the
# connection open for good; these give up within half a minute.
  ssh -T -o BatchMode=yes -o ConnectTimeout=8 -o IdentitiesOnly=yes \
    -o ServerAliveInterval=5 -o ServerAliveCountMax=4 \
    -i "$HOME/.ssh/kobo_cobalt" "root@$device" 2>/dev/null
}

# Clara BW at the default text size: the launcher's 3 x 3 grid, its page
# arrows and the tab bar.
tile_x="200 535 870"
tile_y="340 690 1035"
next_page="965,1265"
apps_tab="134,1380"
books_tab="402,1380"
settings_tab="670,1380"
per_page=9

block=""
count=0

tap() {
  "$kobo" tap --device "$device" "$1" >/dev/null
}

pause() {
  perl -e "select(undef, undef, undef, $1)"
}

shot() {
  [ -n "$block" ] || { echo "shot before any 'app' line" >&2; exit 1; }
  count=$((count + 1))
  mkdir -p "$out/$block"
  name=$(printf '%02d-%s.png' "$count" "$1")
  rm -f "$out/$block/$name"
  "$kobo" shot --device "$device" --out "$out/$block/$name" >/dev/null
  [ -s "$out/$block/$name" ] || { echo "no picture for $block/$name" >&2; exit 1; }
  echo "  $block/$name"
}

# The app in front, by the binary it runs: a jailed app is a copy under /tmp,
# so it is matched to an installed app by checksum.
front_app() {
  on_reader <<'EOF'
C=/mnt/onboard/.adds/cobalt
newest=0; pid=
for p in $(ls /proc | grep '^[0-9]*$'); do
  case "$(readlink /proc/$p/exe 2>/dev/null)" in /tmp/kobo-app-*)
    start=$(cut -d' ' -f22 /proc/$p/stat 2>/dev/null)
    [ "${start:-0}" -gt "$newest" ] && { newest=$start; pid=$p; } ;;
  esac
done
[ -n "$pid" ] || exit 0
sum=$(sha256sum /proc/$pid/exe | cut -d' ' -f1)
for b in $C/apps/*/bin/* $C/bin/kobo-*; do
  case "$b" in *.prev/*) continue ;; esac
  if [ "$(sha256sum "$b" | cut -d' ' -f1)" = "$sum" ]; then
    case "$b" in $C/apps/*) b=${b#$C/apps/}; echo "${b%%/*} $pid" ;;
      *) echo "$(basename "$b" | sed 's/^kobo-//') $pid" ;; esac
    exit 0
  fi
done
echo "unknown $pid"
EOF
}

# Ending the app in front hands the panel back to the launcher; the
# runtime does this for any app that exits. Every app shown is ended, so
# none is left running with state from this run.
home() {
  on_reader <<'EOF'
launcher=$(sha256sum /mnt/onboard/.adds/cobalt/bin/kobo-launcher | cut -d' ' -f1)
for p in $(ls /proc | grep '^[0-9]*$'); do
  case "$(readlink /proc/$p/exe 2>/dev/null)" in /tmp/kobo-app-*)
    [ "$(sha256sum /proc/$p/exe | cut -d' ' -f1)" = "$launcher" ] || kill -TERM $p 2>/dev/null ;;
  esac
done
EOF
  pause 4
  tap "$apps_tab"
  pause 2
  # Apps tapped while on Apps goes to its first page.
  tap "$apps_tab"
  pause 2
}

# Where an app's tile is: App Store and Terminal first, then every installed
# app by display name, as the launcher sorts them.
tile_of() {
  want=$1
  on_reader <<'EOF' > "$work/titles"
C=/mnt/onboard/.adds/cobalt/apps
for d in $C/*; do
  case $d in *.prev) continue ;; esac
  [ -f $d/manifest.json ] || continue
  printf '%s\t%s\n' "$(sed 's/.*"display_name":"\([^"]*\)".*/\1/' $d/manifest.json)" "$(basename $d)"
done
EOF
  index=$(
    { printf 'App Store\tstore\nTerminal\tterminal\n'
      LC_ALL=C sort -f -t "$(printf '\t')" -k1,1 -k2,2 "$work/titles"; } |
      awk -F'\t' -v want="$want" '{ n++ } $2 == want { print n - 1; exit }'
  )
  [ -n "$index" ] || { echo "$want is not installed on the reader" >&2; exit 1; }
  echo "$index"
}

open_app() {
  id=$1
  home
  index=$(tile_of "$id")
  page=$((index / per_page))
  place=$((index % per_page))
  while [ "$page" -gt 0 ]; do tap "$next_page"; pause 2; page=$((page - 1)); done
  x=$(echo "$tile_x" | cut -d' ' -f$((place % 3 + 1)))
  y=$(echo "$tile_y" | cut -d' ' -f$((place / 3 + 1)))
  tap "$x,$y"
  pause 6
  case "$id" in store|terminal) return ;; esac
  opened=$(front_app | cut -d' ' -f1)
  if [ "$opened" != "$id" ]; then
    echo "opening $id from tile $index started '${opened:-nothing}' instead" >&2
    exit 1
  fi
}

run_step() {
  line=$1
  # shellcheck disable=SC2086 # a step is its words
  set -- $line
  [ $# -gt 0 ] || return 0
  verb=$1; shift
  case "$verb" in
    \#*) ;;
    app) block=$1; count=0; echo "$block" ;;
    open) open_app "$1" ;;
    tap) tap "$1" ;;
    wait) pause "$1" ;;
    shot) shot "$1" ;;
    home) home ;;
    tab)
      case "$1" in
        apps) tap "$apps_tab" ;;
        books) tap "$books_tab" ;;
        settings) tap "$settings_tab" ;;
        *) echo "no tab called $1" >&2; exit 1 ;;
      esac
      pause 3 ;;
    # From the repository root, where the companion script and any file a
    # companion shows are found.
    host) (cd "$here/.." && sh -c "$*") ;;
    *) echo "unknown step: $line" >&2; exit 1 ;;
  esac
}

if [ -n "$steps" ]; then
  block=${block:-try}
  printf '%s' "$steps" | while IFS= read -r line; do run_step "$line"; done
  exit 0
fi

# Blocks run in file order; --only skips the others.
skipping=false
while IFS= read -r line || [ -n "$line" ]; do
  case "$line" in
    app\ *)
      id=${line#app }
      if [ -n "$only" ] && ! printf '%s' "$only" | grep -q ",$id,"; then
        skipping=true; continue
      fi
      skipping=false ;;
  esac
  $skipping && continue
  run_step "$line"
done < "$moments"
home

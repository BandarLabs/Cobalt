#!/bin/sh
# Removes Cobalt from a Kobo connected by USB, and leaves everything the owner
# put there.
#
# `kobo setup --undo` already does this for anyone who installed from a
# terminal. This exists for everyone else: an install from the website needs no
# host command, so an uninstall must not require one either.
#
# What it removes is the payload a release installs -- the programs, the
# licences, the launch script -- and the Cobalt line in NickelMenu. What it
# keeps is everything a release never carried: books, settings, credentials,
# trust roots, and the applications and their data.
#
# It does not remove NickelMenu. Other mods use it, and taking it away because
# Cobalt was removed would break them.
set -eu

say() { printf '%s\n' "$*"; }
fail() { printf 'kobo uninstall: %s\n' "$*" >&2; exit 1; }

VOLUME=${1:-}
YES=false
for argument in "$@"; do
    case "$argument" in
        --yes) YES=true ;;
        --help|-h)
            cat <<'EOF'
usage: uninstall.sh [VOLUME] [--yes]

Removes Cobalt from a Kobo mounted at VOLUME, keeping books, settings,
credentials, applications and their data. Finds the reader itself when no
volume is given.
EOF
            exit 0
            ;;
    esac
done
case "$VOLUME" in --*) VOLUME= ;; esac

# The reader mounts under a different place on each system, and guessing wrong
# means writing to somebody's backup drive.
if [ -z "$VOLUME" ]; then
    for candidate in \
        /Volumes/KOBOeReader \
        "/media/$(id -un 2>/dev/null || echo root)/KOBOeReader" \
        /media/KOBOeReader \
        /mnt/KOBOeReader
    do
        if [ -d "$candidate" ]; then VOLUME=$candidate; break; fi
    done
fi
[ -n "$VOLUME" ] || fail "no Kobo found. Connect it, tap Connect on the reader, and pass the drive: uninstall.sh /Volumes/KOBOeReader"
[ -d "$VOLUME" ] || fail "$VOLUME is not there"

# A Kobo has this, and a memory stick does not. Everything below deletes files,
# so the drive is identified before anything is touched rather than after.
[ -f "$VOLUME/.kobo/version" ] || fail "$VOLUME has no .kobo/version, so it is not a Kobo"

INSTALL=$VOLUME/.adds/cobalt
[ -d "$INSTALL" ] || fail "there is no Cobalt on $VOLUME"

# Named rather than globbed. `rm -rf .adds/cobalt` would take the owner's data
# with it, which is the one thing this must not do.
PAYLOAD="bin licenses start.sh README.txt LICENSE THIRD-PARTY.md VERSION"
KEPT="secrets trust state data apps store"

say "Reader:  $VOLUME"
say "Removing Cobalt's own files, and keeping:"
for folder in $KEPT; do
    [ -e "$INSTALL/$folder" ] || continue
    say "  $folder"
done
if [ "$YES" != true ]; then
    printf 'Continue? [y/N] '
    read -r answer </dev/tty || answer=n
    case "$answer" in y|Y|yes|YES) ;; *) fail "nothing was removed" ;; esac
fi

for entry in $PAYLOAD; do
    rm -rf "$INSTALL/$entry"
done
rm -f "$VOLUME/.adds/cobalt-launch.sh"

# Written by the installer and by `kobo setup`, and named for Cobalt precisely
# so it can be removed without reading anybody else's menu. A config somebody
# wrote by hand lives in .adds/nm/menu and is not touched.
rm -f "$VOLUME/.adds/nm/cobalt"

# Rollback and quarantine copies are Cobalt's own, and carry no owner data:
# the updater moves that into the installation it promotes before it leaves
# them behind.
for stale in "$VOLUME/.adds"/cobalt.prev "$VOLUME/.adds"/cobalt.next \
             "$VOLUME/.adds"/cobalt.previous "$VOLUME/.adds"/cobalt.unusable.*
do
    [ -e "$stale" ] || continue
    rm -rf "$stale"
done

# An archive still waiting in the firmware's slot would reinstall Cobalt at the
# next start, which is not what somebody uninstalling it expects. Only removed
# when it is Cobalt's: another mod may be waiting there.
SLOT=$VOLUME/.kobo/KoboRoot.tgz
if [ -f "$SLOT" ] && tar tzf "$SLOT" 2>/dev/null | grep -q '^\.\?/*mnt/onboard/\.adds/cobalt'; then
    rm -f "$SLOT"
    say "Removed the Cobalt archive that was waiting to install."
fi

say ""
say "Cobalt is removed. Your books, settings, credentials, applications and"
say "their data are still on the reader; installing Cobalt again picks them up"
say "where they are."
say ""
say "NickelMenu is left installed, because other mods use it. Its Cobalt entry"
say "is gone."
say ""
say "Eject the drive and restart the reader."

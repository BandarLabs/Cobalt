#!/bin/sh
# Stable book-partition entrypoint. Versioned cobalt trees may be renamed by
# OTA recovery, so NickelMenu must never launch a script from inside one.
set -eu

adds=${COBALT_ADDS:-/mnt/onboard/.adds}
current="$adds/cobalt"
previous="$adds/cobalt.prev"
staging="$adds/cobalt.next"
holder="$adds/cobalt.owner"
owner_folders="secrets trust state data apps store"

regular_file() {
    [ -f "$1" ] && [ ! -L "$1" ]
}

complete_release() {
    [ -d "$1" ] && [ ! -L "$1" ] &&
        regular_file "$1/start.sh" &&
        regular_file "$1/bin/kobod" && [ -x "$1/bin/kobod" ] &&
        regular_file "$1/bin/kobo-launcher" && [ -x "$1/bin/kobo-launcher" ]
}

ensure_holder() {
    if [ -e "$holder" ] || [ -L "$holder" ]; then
        [ -d "$holder" ] && [ ! -L "$holder" ] || {
            echo "Cobalt owner-data holder is unsafe" >&2
            exit 1
        }
    else
        mkdir "$holder"
        sync
    fi
}

hold_current_owner_data() {
    [ -d "$current" ] && [ ! -L "$current" ] || return 0
    for folder in $owner_folders; do
        source="$current/$folder"
        destination="$holder/$folder"
        if [ -e "$source" ] || [ -L "$source" ]; then
            [ -d "$source" ] && [ ! -L "$source" ] || {
                echo "Cobalt owner folder $folder is unsafe" >&2
                exit 1
            }
            ensure_holder
            if [ -e "$destination" ] || [ -L "$destination" ]; then
                echo "Cobalt owner folder $folder has two recoverable copies" >&2
                exit 1
            fi
            mv "$source" "$destination"
            sync
        fi
    done
}

restore_owner_data() {
    if ! [ -e "$holder" ] && ! [ -L "$holder" ]; then
        return 0
    fi
    [ -d "$holder" ] && [ ! -L "$holder" ] || {
        echo "Cobalt owner-data holder is unsafe" >&2
        exit 1
    }
    for folder in $owner_folders; do
        source="$holder/$folder"
        destination="$current/$folder"
        if [ -e "$source" ] || [ -L "$source" ]; then
            [ -d "$source" ] && [ ! -L "$source" ] || {
                echo "Cobalt held owner folder $folder is unsafe" >&2
                exit 1
            }
            if [ -e "$destination" ] || [ -L "$destination" ]; then
                echo "Cobalt owner folder $folder cannot be restored without overwrite" >&2
                exit 1
            fi
            mv "$source" "$destination"
            sync
        fi
    done
    if rmdir "$holder" 2>/dev/null; then
        sync
    fi
}

if ! complete_release "$current"; then
    candidate=
    if complete_release "$previous"; then
        candidate=$previous
    elif complete_release "$staging"; then
        candidate=$staging
    else
        echo "Cobalt has no complete launchable release" >&2
        exit 1
    fi

    hold_current_owner_data
    if [ -e "$current" ] || [ -L "$current" ]; then
        quarantine=
        for suffix in 0 1 2 3 4 5 6 7; do
            slot="$adds/cobalt.unusable.$suffix"
            if ! [ -e "$slot" ] && ! [ -L "$slot" ]; then
                quarantine=$slot
                break
            fi
        done
        if [ -n "$quarantine" ]; then
            mv "$current" "$quarantine"
        else
            # Every bounded diagnostic slot is occupied. Owner data is already
            # durable in the holder, so stale managed files may be discarded
            # rather than letting old quarantines block a verified candidate.
            rm -rf "$current"
        fi
        sync
    fi
    mv "$candidate" "$current"
    sync
fi

restore_owner_data
exec /bin/sh "$current/start.sh"

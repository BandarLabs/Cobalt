#!/bin/sh
set -eu

if [ "$#" -ne 1 ]; then
    echo "usage: $0 /path/to/hwtcon_ioctl_cmd.h" >&2
    exit 2
fi

header_dir=$(dirname "$1")
output=$(mktemp /tmp/kobo-hwtcon-conformance.XXXXXX)
trap 'rm -f "$output"' EXIT HUP INT TERM

cc -std=c11 -Wall -Wextra -Werror \
    -I tools/abi/include \
    -I "$header_dir" \
    tools/abi/hwtcon_conformance.c \
    -o "$output"
"$output"


#!/bin/sh
#
# Regenerates the icon geometry from Tabler Icons.
#
# The artwork this project draws is checked in, as Rust, in
# `crates/kobo-ui/src/vector/tabler.rs`. Nothing at build time or run time
# reads an SVG, downloads anything or depends on this script: the workspace has
# no dependencies and keeps none. This exists so that the checked-in file can
# be reproduced and reviewed rather than being a blob nobody can account for.
#
# Run it when `tools/icon-import/icons.txt` changes, which is when a Glyph is
# added or when one is decided to be the wrong picture for its name.
#
# usage: scripts/import-icons.sh [--version vX.Y.Z] [--keep]
#
set -eu

version=v3.46.0
keep=no

while [ $# -gt 0 ]; do
  case "$1" in
    --version) version=${2:?--version needs a tag}; shift 2 ;;
    --keep) keep=yes; shift ;;
    -h|--help) sed -n '2,17p' "$0" | cut -c 3-; exit 0 ;;
    *) echo "unknown option '$1'" >&2; exit 2 ;;
  esac
done

root=$(cd "$(dirname "$0")/.." && pwd)
checkout=${TMPDIR:-/tmp}/tabler-icons-$version

if [ ! -d "$checkout" ]; then
  echo "fetching Tabler Icons $version"
  git clone --quiet --depth 1 --branch "$version" \
    https://github.com/tabler/tabler-icons.git "$checkout"
fi

# The licence travels with the artwork, which is the whole of what the MIT
# licence asks for in exchange.
cp "$checkout/LICENSE" "$root/licenses/LICENSE-Tabler.txt"

cargo run --quiet --manifest-path "$root/Cargo.toml" -p icon-import -- \
  "$checkout" "$root/crates/kobo-ui/src/vector/tabler.rs"

cargo fmt --manifest-path "$root/Cargo.toml" -p kobo-ui

if [ "$keep" = no ]; then
  rm -rf "$checkout"
fi

cat <<'DONE'

Look at the result before trusting it. Icon artwork is judged by eye:

    cargo test -p kobo-ui contact_sheet -- --ignored --nocapture

draws every glyph onto one sheet and prints where it put it.
DONE

#!/bin/sh
# Builds the Cobalt window, the small Mac program `kobo ui` shows its page
# in, for the Mac this runs on.
#
#   tools/build-macos-shell.sh [OUT_DIR]
#
# Leaves OUT_DIR/Cobalt.app, by default under target/macos-shell, and prints
# its path. `kobo ui` uses the window when Cobalt.app sits in the same folder
# as the kobo binary, and the browser otherwise.
set -eu

if [ "$#" -gt 1 ]; then
    echo "usage: build-macos-shell.sh [OUT_DIR]" >&2
    exit 2
fi
root=$(cd "$(dirname "$0")/.." && pwd)
out=${1:-"$root/target/macos-shell"}

[ "$(uname -s)" = Darwin ] || {
    echo "the Cobalt window is a Mac program and is built on a Mac" >&2
    exit 1
}
# One architecture, the host's. The release archives are per architecture,
# so each carries the slice its own runner built and nothing is merged.
# macOS 13 is the oldest system the window is built for: below it the
# compiler links a Swift runtime that would have to travel inside the
# bundle, and the bundle is meant to hold one executable and one list.
case "$(uname -m)" in
    arm64) triple=arm64-apple-macos13.0 ;;
    x86_64) triple=x86_64-apple-macos13.0 ;;
    *)
        echo "unsupported Mac architecture: $(uname -m)" >&2
        exit 1
        ;;
esac

# The workspace version, so the window's About panel names the release it
# shipped with.
version=$(sed -n 's/^version = "\([0-9][0-9.]*\)"$/\1/p' "$root/Cargo.toml" | head -n 1)
printf '%s\n' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' || {
    echo "could not read the workspace version from Cargo.toml" >&2
    exit 1
}

app="$out/Cobalt.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
xcrun swiftc -O -target "$triple" \
    -o "$app/Contents/MacOS/Cobalt" "$root/desktop/macos/Cobalt.swift"
sed "s/@VERSION@/$version/g" "$root/desktop/macos/Info.plist" > "$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist" > /dev/null

# Signed ad hoc, and only once the bundle is whole. Apple silicon refuses to
# run code with no signature at all, and the Intel slice leaves the linker
# with none. Signing the assembled bundle rather than the bare executable
# seals Info.plist into the signature and makes the bundle identifier the
# code's identity. It is not a Developer ID signature and does not make the
# bundle pass Gatekeeper; it never needs to, because the installer fetches
# it with curl and unpacks it with tar, neither of which marks a file as
# downloaded.
codesign --force --sign - "$app"
codesign --verify --strict "$app"

echo "$app"

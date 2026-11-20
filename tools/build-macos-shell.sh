#!/bin/sh
# Builds Cobalt.app, the Mac app for a Kobo running Cobalt, for the Mac this
# runs on.
#
#   tools/build-macos-shell.sh [OUT_DIR]
#
# Leaves OUT_DIR/Cobalt.app, by default under target/macos-shell, and prints
# its path. The app talks to `kobo ui`: either that command starts the app
# when Cobalt.app sits in the same folder as the kobo binary, or the app,
# opened by itself, starts the kobo binary it finds in that folder.
set -eu

if [ "$#" -gt 1 ]; then
    echo "usage: build-macos-shell.sh [OUT_DIR]" >&2
    exit 2
fi
root=$(cd "$(dirname "$0")/.." && pwd)
out=${1:-"$root/target/macos-shell"}
package="$root/desktop/macos"

[ "$(uname -s)" = Darwin ] || {
    echo "Cobalt.app is a Mac program and is built on a Mac" >&2
    exit 1
}

# The workspace version, so the app's About panel names the release it
# shipped with and the app can tell when the kobo command beside it is from
# a different one.
version=$(sed -n 's/^version = "\([0-9][0-9.]*\)"$/\1/p' "$root/Cargo.toml" | head -n 1)
printf '%s\n' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' || {
    echo "could not read the workspace version from Cargo.toml" >&2
    exit 1
}

# One architecture, the host's, which is what swift build makes when it is
# not told otherwise. The release archives are per architecture, so each
# carries the slice its own runner built and nothing is merged. The package
# asks for macOS 13 and links only system frameworks, so there is no Swift
# runtime or anything else to carry inside the bundle.
swift build --package-path "$package" -c release --product Cobalt
built=$(swift build --package-path "$package" -c release --show-bin-path)/Cobalt
[ -x "$built" ] || {
    echo "swift build did not leave $built" >&2
    exit 1
}

app="$out/Cobalt.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$built" "$app/Contents/MacOS/Cobalt"
sed "s/@VERSION@/$version/g" "$package/Info.plist" > "$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist" > /dev/null

# The icon is drawn by a program in the package and packed by the system's
# own tool, so there is no picture in the repository to fall out of step
# with the mark. Without one the Dock, and every question macOS asks on the
# app's behalf, would show the blank sheet it gives a program with no icon.
iconset=$(mktemp -d "${TMPDIR:-/tmp}/cobalt-icon.XXXXXX")
trap 'rm -rf "$iconset"' EXIT HUP INT TERM
swift "$package/Icon.swift" "$iconset/Cobalt.iconset"
iconutil --convert icns --output "$app/Contents/Resources/Cobalt.icns" "$iconset/Cobalt.iconset"
[ -s "$app/Contents/Resources/Cobalt.icns" ] || {
    echo "iconutil did not leave an icon" >&2
    exit 1
}

# Signed ad hoc, and only once the bundle is whole. Apple silicon refuses to
# run code with no signature at all. Signing the assembled bundle rather
# than the bare executable seals Info.plist into the signature and makes the
# bundle identifier the code's identity, which is the name macOS files its
# privacy decisions under. It is not a Developer ID signature and does not
# make the bundle pass Gatekeeper; it never needs to, because the installer
# fetches it with curl and unpacks it with tar, neither of which marks a
# file as downloaded.
codesign --force --sign - "$app"
codesign --verify --strict "$app"

# The bundle is an executable, a list, an icon and the signature over them.
# Anything else in it arrived by accident and would ship.
extra=$(cd "$app" && find . ! -type d \
    ! -path ./Contents/MacOS/Cobalt \
    ! -path ./Contents/Info.plist \
    ! -path ./Contents/Resources/Cobalt.icns \
    ! -path ./Contents/_CodeSignature/CodeResources)
[ -z "$extra" ] || {
    echo "unexpected files in $app:" >&2
    echo "$extra" >&2
    exit 1
}

echo "$app"

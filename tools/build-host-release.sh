#!/bin/sh
set -eu

if [ "$#" -ne 4 ]; then
    echo "usage: build-host-release.sh VERSION CHANNEL SOURCE_SHA DIST" >&2
    exit 2
fi
VERSION=$1
CHANNEL=$2
SOURCE_SHA=$3
DIST=$4

printf '%s\n' "$VERSION" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$'
case "$CHANNEL" in stable|beta) ;; *) exit 2 ;; esac
printf '%s\n' "$SOURCE_SHA" | grep -Eq '^[0-9a-f]{40}$'
[ -d "$DIST" ]

sha256_file() {
    sha256sum "$1" | awk '{print $1}'
}

size_file() {
    wc -c < "$1" | tr -d ' '
}

device="cobalt-$VERSION-KoboRoot.tgz"
[ -f "$DIST/$device" ]
cp install.sh "$DIST/install.sh"
chmod 755 "$DIST/install.sh"

build_root="$DIST/.host-release-build"
rm -rf "$build_root"
mkdir -p "$build_root"
trap 'rm -rf "$build_root"' EXIT HUP INT TERM

for platform in macos-x86_64 macos-arm64 linux-x86_64 linux-arm64; do
    binary="$DIST/host-binaries/$platform/kobo"
    [ -f "$binary" ] || {
        echo "missing host binary $binary" >&2
        exit 1
    }
    package="$build_root/$platform"
    mkdir -p "$package/licenses"
    cp "$binary" "$package/kobo"
    chmod 755 "$package/kobo"
    cp LICENSE "$package/LICENSE"
    cp THIRD-PARTY.md "$package/THIRD-PARTY.md"
    cp licenses/LICENSE-Rust-dependencies.txt \
        "$package/licenses/LICENSE-Rust-dependencies.txt"
    {
        printf 'Cobalt %s\n' "$VERSION"
        printf 'source https://github.com/BandarLabs/Cobalt/commit/%s\n' "$SOURCE_SHA"
        printf 'release train immutable beta candidate, promotable unchanged to stable\n'
        printf 'host platform %s\n' "$platform"
        printf 'command kobo\n'
    } > "$package/SOURCE.txt"
    asset="$DIST/kobo-$VERSION-$platform.tar.gz"
    tar --sort=name --mtime='@0' --owner=0 --group=0 --numeric-owner \
        -cf - -C "$package" . | gzip -n -9 > "$asset"
done

manifest="$DIST/cobalt-host-manifest.txt"
{
    printf 'cobalt-host-release 1\n'
    printf 'version %s\n' "$VERSION"
    printf 'channels stable,beta\n'
    printf 'source %s\n' "$SOURCE_SHA"
    printf 'device %s %s %s\n' \
        "$device" "$(size_file "$DIST/$device")" "$(sha256_file "$DIST/$device")"
    printf 'bootstrap install.sh %s %s\n' \
        "$(size_file "$DIST/install.sh")" "$(sha256_file "$DIST/install.sh")"
    for platform in macos-x86_64 macos-arm64 linux-x86_64 linux-arm64; do
        asset="kobo-$VERSION-$platform.tar.gz"
        printf 'host %s %s %s %s\n' \
            "$platform" "$asset" "$(size_file "$DIST/$asset")" \
            "$(sha256_file "$DIST/$asset")"
    done
} > "$manifest"

#!/bin/sh
# Stable discovery bootstrap served by GitHub Pages after stable promotion.
#
# This file is trusted through GitHub Pages HTTPS. It verifies the signed
# release manifest and the release's full installer before executing that
# installer; it cannot cryptographically verify its own already-running bytes.
set -eu

REPOSITORY=${KOBO_INSTALLER_REPOSITORY:-BandarLabs/Cobalt}
CHANNEL=stable
VERSION=
VERSION_EXPLICIT=false
EXPECT_VERSION=false
STAGE=

fail() {
    printf 'kobo bootstrap: %s\n' "$*" >&2
    exit 1
}

cleanup() {
    if [ -n "$STAGE" ] && [ -d "$STAGE" ]; then
        rm -rf "$STAGE"
    fi
}
trap cleanup EXIT HUP INT TERM

for argument in "$@"; do
    if [ "$EXPECT_VERSION" = true ]; then
        VERSION=$argument
        VERSION_EXPLICIT=true
        EXPECT_VERSION=false
        continue
    fi
    case "$argument" in
        --stable) CHANNEL=stable ;;
        --beta) CHANNEL=beta ;;
        --version) EXPECT_VERSION=true ;;
    esac
done
[ "$EXPECT_VERSION" = false ] || fail "--version needs X.Y.Z"
case "$VERSION" in
    '') ;;
    *[!0-9.]*|.*|*..*|*.) fail "version must be X.Y.Z" ;;
esac
if [ -n "$VERSION" ] &&
    [ "$(printf '%s' "$VERSION" | awk -F. '{print NF}')" -ne 3 ]; then
    fail "version must be X.Y.Z"
fi

CACHE_HOME=${XDG_CACHE_HOME:-"$HOME/.cache"}
case "$CACHE_HOME" in
    /*) ;;
    *) fail "XDG_CACHE_HOME must be an absolute path" ;;
esac
case "$CACHE_HOME" in
    *'
'*) fail "cache path must not contain newlines" ;;
esac
mkdir -p "$CACHE_HOME/kobo"
chmod 700 "$CACHE_HOME/kobo" 2>/dev/null || true
STAGE=$CACHE_HOME/kobo/pages-bootstrap.$$
rm -rf "$STAGE"
(umask 077 && mkdir "$STAGE") || fail "cannot create bootstrap staging directory"

download() {
    url=$1
    output=$2
    case "$url" in
        https://*) ;;
        *) fail "bootstrap downloads must use HTTPS" ;;
    esac
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL --retry 3 --retry-delay 1 --connect-timeout 10 --max-time 300 \
            -o "$output" -- "$url"
    elif command -v wget >/dev/null 2>&1; then
        wget -q --https-only --timeout=20 --tries=3 -O "$output" -- "$url"
    else
        fail "curl or wget is required"
    fi
}

sha256_file() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | awk '{print $1}'
    elif command -v openssl >/dev/null 2>&1; then
        openssl dgst -sha256 "$1" | awk '{print $NF}'
    else
        fail "sha256sum, shasum, or openssl is required"
    fi
}

file_size() {
    wc -c < "$1" | tr -d ' '
}

if [ "$CHANNEL" = beta ] && [ -z "$VERSION" ]; then
    beta_manifest=$STAGE/Cargo.toml
    download "https://raw.githubusercontent.com/$REPOSITORY/beta/Cargo.toml" \
        "$beta_manifest"
    VERSION=$(sed -n 's/^version = "\([0-9][0-9.]*\)"$/\1/p' "$beta_manifest")
    printf '%s\n' "$VERSION" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' ||
        fail "beta branch does not declare one workspace version"
fi

if [ -n "$VERSION" ]; then
    if [ "$CHANNEL" = beta ]; then
        tag=beta-v$VERSION
    else
        tag=v$VERSION
    fi
    BASE_URL=https://github.com/$REPOSITORY/releases/download/$tag
else
    BASE_URL=https://github.com/$REPOSITORY/releases/latest/download
fi

MANIFEST=$STAGE/cobalt-host-manifest.txt
SIGNATURE=$STAGE/cobalt-host-manifest.txt.sshsig
INSTALLER=$STAGE/install.sh
download "$BASE_URL/cobalt-host-manifest.txt" "$MANIFEST"
download "$BASE_URL/cobalt-host-manifest.txt.sshsig" "$SIGNATURE"

command -v ssh-keygen >/dev/null 2>&1 ||
    fail "OpenSSH ssh-keygen with -Y signature verification is required"
printf '%s\n' \
    'cobalt-release ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIL7XUR3p+tvPgftO/kRbigc8gagzP2RBDG3tWIu/1KXe' \
    > "$STAGE/allowed_signers"
ssh-keygen -Y verify -q \
    -f "$STAGE/allowed_signers" \
    -I cobalt-release \
    -n cobalt-host-release \
    -s "$SIGNATURE" < "$MANIFEST" >/dev/null 2>&1 ||
    fail "release manifest signature verification failed"

manifest_field() {
    field=$1
    count=$(awk -v field="$field" '$1 == field && NF == 2 {count++} END {print count + 0}' \
        "$MANIFEST")
    [ "$count" -eq 1 ] ||
        fail "signed manifest must contain exactly one $field field"
    awk -v field="$field" '$1 == field && NF == 2 {print $2}' "$MANIFEST"
}
manifest_version=$(manifest_field version)
manifest_channels=$(manifest_field channels)
case ",$manifest_channels," in
    *,"$CHANNEL",*) ;;
    *) fail "signed manifest does not allow the requested $CHANNEL channel" ;;
esac
if [ "$VERSION_EXPLICIT" = true ] && [ "$manifest_version" != "$VERSION" ]; then
    fail "requested version $VERSION, but the signed manifest is $manifest_version"
fi
VERSION=$manifest_version

bootstrap_line=$(awk \
    '$1 == "bootstrap" && $2 == "install.sh" && NF == 4 {print $3 " " $4}' \
    "$MANIFEST")
[ "$(printf '%s\n' "$bootstrap_line" | wc -l | tr -d ' ')" -eq 1 ] &&
    [ -n "$bootstrap_line" ] ||
    fail "signed manifest does not contain exactly one install.sh bootstrap"
IFS=' ' read -r expected_bytes expected_sha256 <<EOF
$bootstrap_line
EOF

download "$BASE_URL/install.sh" "$INSTALLER"
[ "$(file_size "$INSTALLER")" = "$expected_bytes" ] ||
    fail "release install.sh length does not match the signed manifest"
[ "$(sha256_file "$INSTALLER")" = "$expected_sha256" ] ||
    fail "release install.sh checksum does not match the signed manifest"
chmod 700 "$INSTALLER"

printf '%s\n' \
    "GitHub Pages supplied discovery; signed release metadata verified install.sh $VERSION."
if [ "$VERSION_EXPLICIT" = true ]; then
    sh "$INSTALLER" "$@"
else
    sh "$INSTALLER" "$@" --version "$VERSION"
fi

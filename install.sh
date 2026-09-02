#!/bin/sh
# Installs the prebuilt kobo host CLI and its verified Cobalt device package.
set -eu

REPOSITORY=${KOBO_INSTALLER_REPOSITORY:-BandarLabs/Cobalt}
CHANNEL=stable
VERSION=
VERSION_EXPLICIT=false
INSTALL_DIR=${KOBO_INSTALL_DIR:-"$HOME/.local/bin"}
NO_SETUP=false
NO_PATH=false
YES=false
NONINTERACTIVE=false
FORCE_CONFLICT=false
BASE_URL=${KOBO_INSTALLER_BASE_URL:-}
PLATFORM=
LOCK=
LOCK_HELD=false
STAGE=

fail() {
    printf 'kobo installer: %s\n' "$*" >&2
    exit 1
}

say() {
    printf '%s\n' "$*"
}

cleanup() {
    if [ -n "$STAGE" ] && [ -d "$STAGE" ]; then
        rm -rf "$STAGE"
    fi
    if [ "$LOCK_HELD" = true ] && [ -n "$LOCK" ] && [ -d "$LOCK" ]; then
        rm -rf "$LOCK"
    fi
}
trap cleanup EXIT HUP INT TERM

usage() {
    cat <<'EOF'
usage: install.sh [--version X.Y.Z] [--yes]
                  [--non-interactive] [--no-setup] [--no-path]
                  [--install-dir PATH] [--force-conflict]

This installs stable Cobalt. Beta is selected later in Cobalt Settings.
EOF
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --stable) ;;
        --beta)
            fail "the host bootstrap installs stable only; enable Beta updates in Cobalt Settings after installation"
            ;;
        --version)
            [ "$#" -ge 2 ] || fail "--version needs X.Y.Z"
            VERSION=$2
            VERSION_EXPLICIT=true
            shift
            ;;
        --install-dir)
            [ "$#" -ge 2 ] || fail "--install-dir needs a path"
            INSTALL_DIR=$2
            shift
            ;;
        --yes) YES=true ;;
        --non-interactive) NONINTERACTIVE=true ;;
        --no-setup) NO_SETUP=true ;;
        --no-path) NO_PATH=true ;;
        --force-conflict) FORCE_CONFLICT=true ;;
        --help|-h)
            usage
            exit 0
            ;;
        --platform)
            [ "${KOBO_INSTALLER_TESTING:-0}" = 1 ] ||
                fail "--platform is reserved for installer tests"
            [ "$#" -ge 2 ] || fail "--platform needs a value"
            PLATFORM=$2
            shift
            ;;
        *) fail "unknown option $1" ;;
    esac
    shift
done

case "$VERSION" in
    '') ;;
    *[!0-9.]*|.*|*..*|*.) fail "version must be X.Y.Z" ;;
esac
if [ -n "$VERSION" ] && [ "$(printf '%s' "$VERSION" | awk -F. '{print NF}')" -ne 3 ]; then
    fail "version must be X.Y.Z"
fi
if [ "$NONINTERACTIVE" = true ] && [ "$YES" != true ]; then
    fail "--non-interactive requires --yes"
fi

uname_s=${KOBO_TEST_UNAME_S:-$(uname -s)}
uname_m=${KOBO_TEST_UNAME_M:-$(uname -m)}
WSL=false
case "$uname_s" in
    Darwin)
        rosetta=${KOBO_TEST_ROSETTA:-0}
        if [ "$rosetta" = 0 ] && command -v sysctl >/dev/null 2>&1; then
            rosetta=$(sysctl -in sysctl.proc_translated 2>/dev/null || printf 0)
        fi
        if [ "$rosetta" = 1 ]; then
            uname_m=arm64
            say "Rosetta detected; selecting the native Apple Silicon package."
        fi
        case "$uname_m" in
            x86_64) detected=macos-x86_64 ;;
            arm64|aarch64) detected=macos-arm64 ;;
            *) fail "unsupported macOS architecture: $uname_m" ;;
        esac
        ;;
    Linux)
        if [ "${KOBO_TEST_WSL:-0}" = 1 ] ||
            [ -n "${WSL_INTEROP:-}" ] ||
            { [ -r /proc/sys/kernel/osrelease ] &&
              grep -qi microsoft /proc/sys/kernel/osrelease; }; then
            WSL=true
        fi
        case "$uname_m" in
            x86_64|amd64) detected=linux-x86_64 ;;
            aarch64|arm64) detected=linux-arm64 ;;
            *) fail "unsupported Linux architecture: $uname_m" ;;
        esac
        ;;
    *) fail "unsupported operating system: $uname_s" ;;
esac
[ -n "$PLATFORM" ] || PLATFORM=$detected
case "$PLATFORM" in
    macos-x86_64|macos-arm64|linux-x86_64|linux-arm64) ;;
    *) fail "unsupported platform selection: $PLATFORM" ;;
esac

DATA_HOME=${XDG_DATA_HOME:-"$HOME/.local/share"}
CACHE_HOME=${XDG_CACHE_HOME:-"$HOME/.cache"}
case "$INSTALL_DIR:$DATA_HOME:$CACHE_HOME" in
    *'
'*) fail "installation paths must not contain newlines" ;;
esac
for directory in "$INSTALL_DIR" "$DATA_HOME" "$CACHE_HOME"; do
    case "$directory" in
        /*) ;;
        *) fail "installation paths must be absolute: $directory" ;;
    esac
done
ROOT=$DATA_HOME/kobo
mkdir -p "$ROOT" "$CACHE_HOME/kobo" "$INSTALL_DIR"
chmod 700 "$ROOT" "$CACHE_HOME/kobo" 2>/dev/null || true

LOCK=$ROOT/install.lock
if ! mkdir "$LOCK" 2>/dev/null; then
    lock_pid=$(cat "$LOCK/pid" 2>/dev/null || true)
    fail "installation lock already exists${lock_pid:+ (recorded pid $lock_pid)}. \
Do not remove $LOCK while an installer may be running; if no installer is active, \
remove that exact directory manually and rerun"
fi
LOCK_HELD=true
printf '%s\n' "$$" > "$LOCK/pid"
rm -rf "$CACHE_HOME/kobo"/install.* "$ROOT/releases"/.new-*
rm -f "$INSTALL_DIR"/.kobo.new.*

STAGE=$CACHE_HOME/kobo/install.$$
rm -rf "$STAGE"
(umask 077 && mkdir "$STAGE") || fail "cannot create staging directory"

download() {
    url=$1
    output=$2
    case "$url" in
        https://*) ;;
        file://*)
            [ "${KOBO_INSTALLER_TESTING:-0}" = 1 ] ||
                fail "release downloads must use HTTPS"
            ;;
        *) fail "release downloads must use HTTPS" ;;
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

if [ -z "$BASE_URL" ]; then
    if [ -n "$VERSION" ]; then
        tag=v$VERSION
        BASE_URL=https://github.com/$REPOSITORY/releases/download/$tag
    else
        BASE_URL=https://github.com/$REPOSITORY/releases/latest/download
    fi
fi

MANIFEST=$STAGE/cobalt-host-manifest.txt
SSH_SIGNATURE=$STAGE/cobalt-host-manifest.txt.sshsig
RAW_SIGNATURE=$STAGE/cobalt-host-manifest.txt.sig
download "$BASE_URL/cobalt-host-manifest.txt" "$MANIFEST" ||
    fail "cannot download the release manifest"
download "$BASE_URL/cobalt-host-manifest.txt.sshsig" "$SSH_SIGNATURE" ||
    fail "cannot download the release signature"

command -v ssh-keygen >/dev/null 2>&1 ||
    fail "OpenSSH ssh-keygen with -Y signature verification is required"
ALLOWED_SIGNER="cobalt-release ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIL7XUR3p+tvPgftO/kRbigc8gagzP2RBDG3tWIu/1KXe"
printf '%s\n' "$ALLOWED_SIGNER" > "$STAGE/allowed_signers"
if ! ssh-keygen -Y verify -q \
    -f "$STAGE/allowed_signers" \
    -I cobalt-release \
    -n cobalt-host-release \
    -s "$SSH_SIGNATURE" < "$MANIFEST" >/dev/null 2>&1; then
    fail "release manifest signature verification failed"
fi

[ "$(sed -n '1p' "$MANIFEST")" = "cobalt-host-release 1" ] ||
    fail "unsupported release manifest format"
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
manifest_source=$(manifest_field source)
case ",$manifest_channels," in
    *,stable,*) ;;
    *) fail "signed manifest does not allow stable installation" ;;
esac
if [ -n "$VERSION" ] && [ "$manifest_version" != "$VERSION" ]; then
    fail "requested version $VERSION, but the signed manifest is $manifest_version"
fi
VERSION=$manifest_version
printf '%s\n' "$VERSION" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' ||
    fail "signed manifest has an invalid version"
printf '%s\n' "$manifest_source" | grep -Eq '^[0-9a-f]{40}$' ||
    fail "signed manifest has an invalid source commit"
installed_version=$(sed -n 's/^version //p' "$ROOT/install-state" 2>/dev/null || true)
if [ -n "$installed_version" ] && [ "$VERSION_EXPLICIT" != true ]; then
    if ! awk -v old="$installed_version" -v new="$VERSION" 'BEGIN {
        split(old, a, "."); split(new, b, ".");
        for (i = 1; i <= 3; i++) {
            if ((b[i] + 0) > (a[i] + 0)) exit 0;
            if ((b[i] + 0) < (a[i] + 0)) exit 1;
        }
        exit 0;
    }'; then
        fail "refusing automatic downgrade from $installed_version to $VERSION; use --version $VERSION explicitly"
    fi
fi

host_line=$(awk -v platform="$PLATFORM" \
    '$1 == "host" && $2 == platform && NF == 5 {print $3 " " $4 " " $5}' \
    "$MANIFEST")
device_line=$(awk '$1 == "device" && NF == 4 {print $2 " " $3 " " $4}' "$MANIFEST")
bootstrap_line=$(awk \
    '$1 == "bootstrap" && $2 == "install.sh" && NF == 4 {print $2 " " $3 " " $4}' \
    "$MANIFEST")
[ "$(printf '%s\n' "$host_line" | wc -l | tr -d ' ')" -eq 1 ] && [ -n "$host_line" ] ||
    fail "signed manifest does not contain exactly one $PLATFORM host package"
[ "$(printf '%s\n' "$device_line" | wc -l | tr -d ' ')" -eq 1 ] && [ -n "$device_line" ] ||
    fail "signed manifest does not contain exactly one device package"
[ "$(printf '%s\n' "$bootstrap_line" | wc -l | tr -d ' ')" -eq 1 ] &&
    [ -n "$bootstrap_line" ] ||
    fail "signed manifest does not contain exactly one install.sh bootstrap"
IFS=' ' read -r HOST_ASSET HOST_BYTES HOST_SHA <<EOF
$host_line
EOF
IFS=' ' read -r DEVICE_ASSET DEVICE_BYTES DEVICE_SHA <<EOF
$device_line
EOF
IFS=' ' read -r BOOTSTRAP_ASSET BOOTSTRAP_BYTES BOOTSTRAP_SHA <<EOF
$bootstrap_line
EOF
[ "$BOOTSTRAP_ASSET" = install.sh ] || fail "signed bootstrap asset has an invalid name"
case "$HOST_ASSET:$DEVICE_ASSET" in
    *[!A-Za-z0-9._:-]*) fail "signed manifest contains an unsafe asset name" ;;
esac

case "$0" in
    sh|-sh|bash|-bash|dash|-dash|*/sh|*/bash|*/dash)
        say "Bootstrap note: piped shell input is trusted through HTTPS; downloaded artifacts remain signature-verified."
        ;;
    *)
        [ -f "$0" ] || fail "cannot inspect bootstrap path $0"
        [ "$(file_size "$0")" = "$BOOTSTRAP_BYTES" ] ||
            fail "this install.sh does not match the signed bootstrap length"
        [ "$(sha256_file "$0")" = "$BOOTSTRAP_SHA" ] ||
            fail "this install.sh does not match the signed bootstrap checksum"
        ;;
esac

HOST_ARCHIVE=$STAGE/$HOST_ASSET
DEVICE_ARCHIVE=$STAGE/$DEVICE_ASSET
download "$BASE_URL/$HOST_ASSET" "$HOST_ARCHIVE" ||
    fail "cannot download $HOST_ASSET"
download "$BASE_URL/$DEVICE_ASSET" "$DEVICE_ARCHIVE" ||
    fail "cannot download $DEVICE_ASSET"
download "$BASE_URL/cobalt-host-manifest.txt.sig" "$RAW_SIGNATURE" ||
    fail "cannot download the raw release-manifest signature"

verify_asset() {
    path=$1
    expected_bytes=$2
    expected_sha=$3
    actual_bytes=$(file_size "$path")
    [ "$actual_bytes" = "$expected_bytes" ] ||
        fail "$(basename "$path") is truncated: expected $expected_bytes bytes, found $actual_bytes"
    actual_sha=$(sha256_file "$path")
    [ "$actual_sha" = "$expected_sha" ] ||
        fail "$(basename "$path") checksum failed"
}
verify_asset "$HOST_ARCHIVE" "$HOST_BYTES" "$HOST_SHA"
verify_asset "$DEVICE_ARCHIVE" "$DEVICE_BYTES" "$DEVICE_SHA"

PACKAGE=$STAGE/host
mkdir "$PACKAGE"
tar -tzf "$HOST_ARCHIVE" > "$STAGE/host-files" ||
    fail "$HOST_ASSET is not a readable host package"
LC_ALL=C sort "$STAGE/host-files" > "$STAGE/host-files.sorted"
cat > "$STAGE/host-files.expected" <<'EOF'
./
./LICENSE
./SOURCE.txt
./THIRD-PARTY.md
./kobo
./licenses/
./licenses/LICENSE-Rust-dependencies.txt
EOF
if ! cmp "$STAGE/host-files.expected" "$STAGE/host-files.sorted" >/dev/null 2>&1; then
    fail "$HOST_ASSET contains an unexpected path"
fi
tar -xzf "$HOST_ARCHIVE" -C "$PACKAGE" ||
    fail "$HOST_ASSET is not a readable host package"
for required in kobo LICENSE THIRD-PARTY.md licenses/LICENSE-Rust-dependencies.txt SOURCE.txt; do
    [ -f "$PACKAGE/$required" ] || fail "$HOST_ASSET is missing $required"
    [ ! -L "$PACKAGE/$required" ] || fail "$HOST_ASSET contains a symbolic link"
done
[ -x "$PACKAGE/kobo" ] || chmod 755 "$PACKAGE/kobo"

TARGET=$INSTALL_DIR/kobo
STATE=$ROOT/install-state
managed_binary=$(sed -n 's/^binary //p' "$STATE" 2>/dev/null || true)
if [ -e "$TARGET" ] && [ "$managed_binary" != "$TARGET" ] && [ "$FORCE_CONFLICT" != true ]; then
    fail "$TARGET already exists and is not managed by this installer; use --force-conflict to replace it"
fi
existing=$(command -v kobo 2>/dev/null || true)
if [ -n "$existing" ] && [ "$existing" != "$TARGET" ] &&
    [ "$managed_binary" != "$existing" ] && [ "$FORCE_CONFLICT" != true ]; then
    fail "another kobo command is already on PATH at $existing; refusing to shadow it"
fi

say ""
say "Install Cobalt $VERSION ($CHANNEL)"
say "  Platform: $PLATFORM"
say "  Host command: $TARGET"
say "  Device package: $DEVICE_ASSET"
say "  Source commit: $manifest_source"
if [ "$YES" != true ]; then
    [ "$NONINTERACTIVE" != true ] || fail "noninteractive install was not confirmed"
    [ -r /dev/tty ] || fail "no terminal is available for confirmation; use --yes after review"
    printf 'Continue? [y/N] ' > /dev/tty
    IFS= read -r answer < /dev/tty || answer=
    case "$answer" in y|Y|yes|YES|Yes) ;; *) say "Declined; nothing was installed."; exit 0 ;; esac
fi

RELEASE=$ROOT/releases/$VERSION-stable
RELEASE_NEW=$ROOT/releases/.new-$VERSION-stable-$$
mkdir -p "$ROOT/releases"
rm -rf "$RELEASE_NEW"
mkdir "$RELEASE_NEW"
cp "$MANIFEST" "$RELEASE_NEW/cobalt-host-manifest.txt"
cp "$RAW_SIGNATURE" "$RELEASE_NEW/cobalt-host-manifest.txt.sig"
cp "$SSH_SIGNATURE" "$RELEASE_NEW/cobalt-host-manifest.txt.sshsig"
cp "$DEVICE_ARCHIVE" "$RELEASE_NEW/$DEVICE_ASSET"
printf '%s\n' "$CHANNEL" > "$RELEASE_NEW/channel"
if [ -d "$RELEASE" ]; then
    for file in cobalt-host-manifest.txt cobalt-host-manifest.txt.sig \
        cobalt-host-manifest.txt.sshsig "$DEVICE_ASSET" channel; do
        cmp "$RELEASE/$file" "$RELEASE_NEW/$file" >/dev/null 2>&1 ||
            fail "installed immutable release $VERSION-$CHANNEL differs from the signed release"
    done
    rm -rf "$RELEASE_NEW"
else
    mv "$RELEASE_NEW" "$RELEASE" ||
        fail "cannot activate the verified release package"
fi

TARGET_NEW=$INSTALL_DIR/.kobo.new.$$
cp "$PACKAGE/kobo" "$TARGET_NEW"
chmod 755 "$TARGET_NEW"
[ "$(sha256_file "$TARGET_NEW")" = "$(sha256_file "$PACKAGE/kobo")" ] ||
    fail "staged kobo binary changed while copying"
mv -f "$TARGET_NEW" "$TARGET" || fail "cannot replace $TARGET"

STATE_NEW=$ROOT/install-state.new.$$
{
    printf 'cobalt-kobo-install 1\n'
    printf 'binary %s\n' "$TARGET"
    printf 'release %s\n' "$RELEASE"
    printf 'version %s\n' "$VERSION"
    printf 'channel %s\n' "$CHANNEL"
    printf 'platform %s\n' "$PLATFORM"
    printf 'source %s\n' "$manifest_source"
} > "$STATE_NEW"
mv "$STATE_NEW" "$STATE"

if [ "$NO_PATH" != true ]; then
    case ":${PATH:-}:" in
        *:"$INSTALL_DIR":*) ;;
        *)
            shell_name=$(basename "${SHELL:-sh}")
            case "$shell_name" in
                zsh) rc=$HOME/.zshrc ;;
                bash)
                    if [ "$uname_s" = Darwin ]; then rc=$HOME/.bash_profile; else rc=$HOME/.bashrc; fi
                    ;;
                *) rc=$HOME/.profile ;;
            esac
            begin='# >>> Cobalt kobo installer >>>'
            if ! grep -F "$begin" "$rc" >/dev/null 2>&1; then
                {
                    printf '\n%s\n' "$begin"
                    printf '%s\n' "export PATH=\"$INSTALL_DIR:\$PATH\""
                    printf '%s\n' '# <<< Cobalt kobo installer <<<'
                } >> "$rc"
            fi
            say "Added $INSTALL_DIR to PATH in $rc (idempotent marked block)."
            say "Open a new shell, or run: export PATH=\"$INSTALL_DIR:\$PATH\""
            ;;
    esac
fi

say "Installed kobo $VERSION at $TARGET."
if [ "$WSL" = true ]; then
    say "WSL detected. Kobo drives normally appear below /mnt; eject them from Windows, not WSL."
fi

if [ "$NO_SETUP" = false ]; then
    if [ "$NONINTERACTIVE" = true ]; then
        "$TARGET" setup --release-dir "$RELEASE" --non-interactive --yes
    else
        "$TARGET" setup --release-dir "$RELEASE" --wait-for-reader
    fi
else
    say "Run 'kobo setup' later with the charged reader connected by USB."
fi

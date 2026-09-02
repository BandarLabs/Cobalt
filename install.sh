#!/bin/sh
# Installs the prebuilt kobo host CLI and its verified Cobalt device package.
set -eu

REPOSITORY=${KOBO_INSTALLER_REPOSITORY:-BandarLabs/Cobalt}
CHANNEL=stable
HOST_UPDATE=false
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
HOST_NEW=
CURRENT_NEW=
COMMAND_NEW=

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
    [ -z "$HOST_NEW" ] || rm -rf "$HOST_NEW"
    [ -z "$CURRENT_NEW" ] || rm -f "$CURRENT_NEW"
    [ -z "$COMMAND_NEW" ] || rm -f "$COMMAND_NEW"
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
        --beta)
            fail "the host bootstrap installs stable only; enable Beta updates in Cobalt Settings after installation"
            ;;
        --host-update)
            HOST_UPDATE=true
            NO_SETUP=true
            NO_PATH=true
            YES=true
            NONINTERACTIVE=true
            ;;
        --channel)
            [ "$#" -ge 2 ] || fail "--channel needs stable or beta"
            CHANNEL=$2
            shift
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

if [ "$HOST_UPDATE" != true ] && [ "$CHANNEL" != stable ]; then
    fail "channel selection is available only through 'kobo update'"
fi
case "$CHANNEL" in
    stable|beta) ;;
    *) fail "update channel must be stable or beta" ;;
esac

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
ROOT=$DATA_HOME/kobo
STATE=$ROOT/install-state
if [ "$HOST_UPDATE" = true ]; then
    [ -f "$STATE" ] || fail "kobo update requires an installation managed by this installer"
    managed_binary=$(sed -n 's/^binary //p' "$STATE")
    [ -n "$managed_binary" ] || fail "managed installation state has no binary path"
    INSTALL_DIR=$(dirname "$managed_binary")
fi
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
rm -rf "$CACHE_HOME/kobo"/install.* "$ROOT/releases"/.new-* "$ROOT/hosts"/.new-*
rm -f "$ROOT"/.current.new.* "$INSTALL_DIR"/.kobo.new.*

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

fail_point() {
    if [ "${KOBO_INSTALLER_TESTING:-0}" = 1 ] &&
        [ "${KOBO_TEST_FAIL_AT:-}" = "$1" ]; then
        fail "injected failure after $1"
    fi
}

if [ -z "$BASE_URL" ]; then
    if [ "$HOST_UPDATE" = true ] && [ "$CHANNEL" = beta ] && [ -z "$VERSION" ]; then
        beta_manifest=$STAGE/Cargo.toml
        download "https://raw.githubusercontent.com/$REPOSITORY/beta/Cargo.toml" "$beta_manifest" ||
            fail "cannot discover the current beta version"
        VERSION=$(sed -n 's/^version = "\([0-9][0-9.]*\)"$/\1/p' "$beta_manifest")
        [ -n "$VERSION" ] || fail "beta branch does not declare one workspace version"
    fi
    if [ -n "$VERSION" ]; then
        if [ "$CHANNEL" = beta ]; then tag=beta-v$VERSION; else tag=v$VERSION; fi
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
    *,"$CHANNEL",*) ;;
    *) fail "signed manifest does not allow $CHANNEL installation" ;;
esac
if [ -n "$VERSION" ] && [ "$manifest_version" != "$VERSION" ]; then
    fail "requested version $VERSION, but the signed manifest is $manifest_version"
fi
VERSION=$manifest_version
printf '%s\n' "$VERSION" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' ||
    fail "signed manifest has an invalid version"
printf '%s\n' "$manifest_source" | grep -Eq '^[0-9a-f]{40}$' ||
    fail "signed manifest has an invalid source commit"
if [ "$HOST_UPDATE" = true ] && [ -f "$ROOT/current" ]; then
    current_host=$(cat "$ROOT/current")
    case "$current_host" in
        ''|*[!A-Za-z0-9._-]*) fail "managed host selector is invalid" ;;
    esac
    installed_version=$(cat "$ROOT/hosts/$current_host/VERSION" 2>/dev/null || true)
    installed_host_channel=$(cat "$ROOT/hosts/$current_host/CHANNEL" 2>/dev/null || true)
else
    installed_version=$(sed -n 's/^version //p' "$ROOT/install-state" 2>/dev/null || true)
    installed_host_channel=$(sed -n 's/^host-channel //p' "$ROOT/install-state" 2>/dev/null || true)
fi
[ -n "$installed_host_channel" ] || installed_host_channel=stable
if [ "$HOST_UPDATE" = true ] &&
    [ "$installed_version" = "$VERSION" ] &&
    [ "$installed_host_channel" = "$CHANNEL" ]; then
    say "kobo $VERSION is already current on the $CHANNEL channel."
    exit 0
fi
if [ -n "$installed_version" ] &&
    [ "$VERSION_EXPLICIT" != true ] &&
    { [ "$HOST_UPDATE" != true ] || [ "$installed_host_channel" = "$CHANNEL" ]; }; then
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

BOOTSTRAP_SOURCE=
if [ "$HOST_UPDATE" != true ]; then
    BOOTSTRAP_SOURCE=$STAGE/verified-install.sh
    case "$0" in
        sh|-sh|bash|-bash|dash|-dash|*/sh|*/bash|*/dash)
            say "Bootstrap note: piped shell input is trusted through HTTPS; downloaded artifacts remain signature-verified."
            download "$BASE_URL/install.sh" "$BOOTSTRAP_SOURCE" ||
                fail "cannot download the stable updater"
            ;;
        *)
            [ -f "$0" ] || fail "cannot inspect bootstrap path $0"
            cp "$0" "$BOOTSTRAP_SOURCE"
            ;;
    esac
fi
if [ -n "$BOOTSTRAP_SOURCE" ]; then
    [ "$(file_size "$BOOTSTRAP_SOURCE")" = "$BOOTSTRAP_BYTES" ] ||
        fail "install.sh does not match the signed bootstrap length"
    [ "$(sha256_file "$BOOTSTRAP_SOURCE")" = "$BOOTSTRAP_SHA" ] ||
        fail "install.sh does not match the signed bootstrap checksum"
fi

HOST_ARCHIVE=$STAGE/$HOST_ASSET
DEVICE_ARCHIVE=$STAGE/$DEVICE_ASSET
CACHE_SETUP=true
if [ "$HOST_UPDATE" = true ]; then
    CACHE_SETUP=false
fi
download "$BASE_URL/$HOST_ASSET" "$HOST_ARCHIVE" ||
    fail "cannot download $HOST_ASSET"
if [ "$CACHE_SETUP" = true ]; then
    download "$BASE_URL/$DEVICE_ASSET" "$DEVICE_ARCHIVE" ||
        fail "cannot download $DEVICE_ASSET"
    download "$BASE_URL/cobalt-host-manifest.txt.sig" "$RAW_SIGNATURE" ||
        fail "cannot download the raw release-manifest signature"
fi

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
if [ "$CACHE_SETUP" = true ]; then
    verify_asset "$DEVICE_ARCHIVE" "$DEVICE_BYTES" "$DEVICE_SHA"
fi

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
./updater.sh
EOF
if ! cmp "$STAGE/host-files.expected" "$STAGE/host-files.sorted" >/dev/null 2>&1; then
    fail "$HOST_ASSET contains an unexpected path"
fi
tar -xzf "$HOST_ARCHIVE" -C "$PACKAGE" ||
    fail "$HOST_ASSET is not a readable host package"
for required in kobo updater.sh LICENSE THIRD-PARTY.md licenses/LICENSE-Rust-dependencies.txt SOURCE.txt; do
    [ -f "$PACKAGE/$required" ] || fail "$HOST_ASSET is missing $required"
    [ ! -L "$PACKAGE/$required" ] || fail "$HOST_ASSET contains a symbolic link"
done
[ -x "$PACKAGE/kobo" ] || chmod 755 "$PACKAGE/kobo"
[ "$(file_size "$PACKAGE/updater.sh")" = "$BOOTSTRAP_BYTES" ] ||
    fail "packaged updater length does not match the signed manifest"
[ "$(sha256_file "$PACKAGE/updater.sh")" = "$BOOTSTRAP_SHA" ] ||
    fail "packaged updater checksum does not match the signed manifest"

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
COMMAND_DESTINATION=$ROOT/launcher/kobo
if [ "$HOST_UPDATE" = true ]; then
    [ -L "$TARGET" ] && [ "$(readlink "$TARGET")" = "$COMMAND_DESTINATION" ] ||
        fail "managed kobo command link changed; refusing to replace it"
fi

say ""
if [ "$HOST_UPDATE" = true ]; then
    say "Update kobo to $VERSION ($CHANNEL)"
else
    say "Install Cobalt $VERSION ($CHANNEL)"
fi
say "  Platform: $PLATFORM"
say "  Host command: $TARGET"
if [ "$CACHE_SETUP" = true ]; then
    say "  Stable setup package: $DEVICE_ASSET"
fi
say "  Source commit: $manifest_source"
if [ "$YES" != true ]; then
    [ "$NONINTERACTIVE" != true ] || fail "noninteractive install was not confirmed"
    [ -r /dev/tty ] || fail "no terminal is available for confirmation; use --yes after review"
    printf 'Continue? [y/N] ' > /dev/tty
    IFS= read -r answer < /dev/tty || answer=
    case "$answer" in y|Y|yes|YES|Yes) ;; *) say "Declined; nothing was installed."; exit 0 ;; esac
fi

if [ "$CACHE_SETUP" = true ]; then
    RELEASE=$ROOT/releases/$VERSION-stable
    RELEASE_NEW=$ROOT/releases/.new-$VERSION-stable-$$
    mkdir -p "$ROOT/releases"
    rm -rf "$RELEASE_NEW"
    mkdir "$RELEASE_NEW"
    cp "$MANIFEST" "$RELEASE_NEW/cobalt-host-manifest.txt"
    cp "$RAW_SIGNATURE" "$RELEASE_NEW/cobalt-host-manifest.txt.sig"
    cp "$SSH_SIGNATURE" "$RELEASE_NEW/cobalt-host-manifest.txt.sshsig"
    cp "$DEVICE_ARCHIVE" "$RELEASE_NEW/$DEVICE_ASSET"
    printf '%s\n' stable > "$RELEASE_NEW/channel"
    if [ -d "$RELEASE" ]; then
        for file in cobalt-host-manifest.txt cobalt-host-manifest.txt.sig \
            cobalt-host-manifest.txt.sshsig "$DEVICE_ASSET" channel; do
            cmp "$RELEASE/$file" "$RELEASE_NEW/$file" >/dev/null 2>&1 ||
                fail "installed immutable release $VERSION-stable differs from the signed release"
        done
        rm -rf "$RELEASE_NEW"
    else
        mv "$RELEASE_NEW" "$RELEASE" ||
            fail "cannot activate the verified release package"
    fi
    SETUP_CHANNEL=stable
else
    RELEASE=$(sed -n 's/^release //p' "$STATE")
    SETUP_CHANNEL=$(sed -n 's/^channel //p' "$STATE")
    [ -n "$RELEASE" ] && [ "$SETUP_CHANNEL" = stable ] ||
        fail "managed installation has no stable setup package"
fi

HOST_ID=$VERSION-$CHANNEL-$PLATFORM-$HOST_SHA
HOST_DIR=$ROOT/hosts/$HOST_ID
HOST_NEW=$ROOT/hosts/.new-$HOST_ID-$$
mkdir -p "$ROOT/hosts"
rm -rf "$HOST_NEW"
mkdir "$HOST_NEW"
cp -R "$PACKAGE/." "$HOST_NEW/"
printf '%s\n' "$VERSION" > "$HOST_NEW/VERSION"
printf '%s\n' "$CHANNEL" > "$HOST_NEW/CHANNEL"
printf '%s\n' "$PLATFORM" > "$HOST_NEW/PLATFORM"
printf '%s\n' "$manifest_source" > "$HOST_NEW/SOURCE_COMMIT"
printf '%s\n' "$HOST_SHA" > "$HOST_NEW/HOST_ARCHIVE_SHA256"
fail_point host-directory
if [ -d "$HOST_DIR" ]; then
    for file in kobo updater.sh VERSION CHANNEL PLATFORM SOURCE_COMMIT HOST_ARCHIVE_SHA256; do
        cmp "$HOST_DIR/$file" "$HOST_NEW/$file" >/dev/null 2>&1 ||
            fail "immutable host release $HOST_ID differs from its verified copy"
    done
    rm -rf "$HOST_NEW"
else
    mv "$HOST_NEW" "$HOST_DIR" || fail "cannot activate immutable host release $HOST_ID"
fi
HOST_NEW=

if [ "$HOST_UPDATE" != true ]; then
    LAUNCHER_NEW=$STAGE/launcher
    mkdir "$LAUNCHER_NEW"
    printf '%s\n' "$ROOT" > "$LAUNCHER_NEW/root"
    cat > "$LAUNCHER_NEW/kobo" <<'EOF'
#!/bin/sh
set -eu
target=$0
while [ -L "$target" ]; do
    link=$(readlink "$target") || {
        echo "kobo: managed command link is invalid" >&2
        exit 1
    }
    case "$link" in
        /*) target=$link ;;
        *) target=$(dirname "$target")/$link ;;
    esac
done
launcher=$(dirname "$target")
root=$(cat "$launcher/root")
selected=$(cat "$root/current")
case "$selected" in
    ''|*[!A-Za-z0-9._-]*)
        echo "kobo: managed host selector is invalid" >&2
        exit 1
        ;;
esac
exec "$root/hosts/$selected/kobo" "$@"
EOF
    chmod 700 "$LAUNCHER_NEW/kobo"
    if [ -d "$ROOT/launcher" ]; then
        if ! cmp "$ROOT/launcher/root" "$LAUNCHER_NEW/root" >/dev/null 2>&1 ||
            ! cmp "$ROOT/launcher/kobo" "$LAUNCHER_NEW/kobo" >/dev/null 2>&1; then
            fail "managed kobo launcher changed; refusing to replace it"
        fi
    else
        mv "$LAUNCHER_NEW" "$ROOT/launcher" ||
            fail "cannot activate the stable kobo launcher"
    fi
fi

CURRENT_NEW=$ROOT/.current.new.$$
rm -f "$CURRENT_NEW"
printf '%s\n' "$HOST_ID" > "$CURRENT_NEW"
fail_point before-selector
mv -f "$CURRENT_NEW" "$ROOT/current" ||
    fail "cannot atomically select host release $HOST_ID"
CURRENT_NEW=
fail_point after-selector

if [ "$HOST_UPDATE" != true ]; then
    COMMAND_NEW=$INSTALL_DIR/.kobo.new.$$
    rm -f "$COMMAND_NEW"
    ln -s "$COMMAND_DESTINATION" "$COMMAND_NEW" ||
        fail "cannot stage the kobo command link"
    mv -f "$COMMAND_NEW" "$TARGET" || fail "cannot activate the kobo command link"
    COMMAND_NEW=

    STATE_NEW=$ROOT/install-state.new.$$
    {
        printf 'cobalt-kobo-install 1\n'
        printf 'binary %s\n' "$TARGET"
        printf 'release %s\n' "$RELEASE"
        printf 'version %s\n' "$VERSION"
        printf 'channel %s\n' "$SETUP_CHANNEL"
        printf 'host-channel %s\n' "$CHANNEL"
        printf 'platform %s\n' "$PLATFORM"
        printf 'source %s\n' "$manifest_source"
    } > "$STATE_NEW"
    mv "$STATE_NEW" "$STATE"
fi

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

if [ "$HOST_UPDATE" = true ]; then
    say "Updated kobo to $VERSION ($CHANNEL) at $TARGET."
    exit 0
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

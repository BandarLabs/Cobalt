#!/bin/sh
set -eu

ROOT=$(CDPATH='' cd -- "$(dirname "$0")/.." && pwd)
WORK=$ROOT/target/installer-tests
rm -rf "$WORK"
mkdir -p "$WORK"
trap 'rm -rf "$WORK"' EXIT HUP INT TERM

sha256_file() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

size_file() {
    wc -c < "$1" | tr -d ' '
}

file_url() {
    printf 'file://%s\n' "$1" | sed 's/ /%20/g'
}

ssh-keygen -q -t ed25519 -N '' -f "$WORK/signing" >/dev/null
SIGNER="cobalt-release $(cat "$WORK/signing.pub")"
SIGNER=$(printf '%s\n' "$SIGNER" | awk '{print $1 " " $2 " " $3}')
INSTALLER=$WORK/install.sh
sed "s|cobalt-release ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIL7XUR3p+tvPgftO/kRbigc8gagzP2RBDG3tWIu/1KXe|$SIGNER|" \
    "$ROOT/install.sh" > "$INSTALLER"
PAGES_INSTALLER=$WORK/pages-install.sh
sed "s|cobalt-release ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIL7XUR3p+tvPgftO/kRbigc8gagzP2RBDG3tWIu/1KXe|$SIGNER|" \
    "$ROOT/docs/install.sh" > "$PAGES_INSTALLER"

make_release() {
    directory=$1
    version=$2
    channel=$3
    marker=$4
    case "$channel" in stable|beta) ;; *) exit 2 ;; esac
    mkdir -p "$directory/package/licenses"
    cat > "$directory/package/kobo" <<EOF
#!/bin/sh
printf '%s\n' '$marker'
EOF
    chmod 755 "$directory/package/kobo"
    cp "$INSTALLER" "$directory/package/updater.sh"
    chmod 700 "$directory/package/updater.sh"
    printf 'license\n' > "$directory/package/LICENSE"
    printf 'notices\n' > "$directory/package/THIRD-PARTY.md"
    printf 'dependency terms\n' > "$directory/package/licenses/LICENSE-Rust-dependencies.txt"
    printf 'source 0123456789abcdef0123456789abcdef01234567\n' > "$directory/package/SOURCE.txt"
    device="cobalt-$version-KoboRoot.tgz"
    printf 'device package %s\n' "$marker" > "$directory/$device"
    cp "$INSTALLER" "$directory/install.sh"
    for platform in macos-x86_64 macos-arm64 linux-x86_64 linux-arm64; do
        asset="kobo-$version-$platform.tar.gz"
        tar -czf "$directory/$asset" -C "$directory/package" .
    done
    {
        printf 'cobalt-host-release 1\n'
        printf 'version %s\n' "$version"
        printf 'channels stable,beta\n'
        printf 'source 0123456789abcdef0123456789abcdef01234567\n'
        printf 'device %s %s %s\n' \
            "$device" "$(size_file "$directory/$device")" "$(sha256_file "$directory/$device")"
        printf 'bootstrap install.sh %s %s\n' \
            "$(size_file "$directory/install.sh")" "$(sha256_file "$directory/install.sh")"
        for platform in macos-x86_64 macos-arm64 linux-x86_64 linux-arm64; do
            asset="kobo-$version-$platform.tar.gz"
            printf 'host %s %s %s %s\n' \
                "$platform" "$asset" "$(size_file "$directory/$asset")" \
                "$(sha256_file "$directory/$asset")"
        done
    } > "$directory/cobalt-host-manifest.txt"
    ssh-keygen -q -Y sign -f "$WORK/signing" -n cobalt-host-release \
        "$directory/cobalt-host-manifest.txt" >/dev/null
    mv "$directory/cobalt-host-manifest.txt.sig" \
        "$directory/cobalt-host-manifest.txt.sshsig"
    printf '%0128d\n' 0 > "$directory/cobalt-host-manifest.txt.sig"
    rm -rf "$directory/package"
}

run_install() {
    home=$1
    release=$2
    shift 2
    mkdir -p "$home"
    HOME=$home \
    XDG_DATA_HOME=$home/data \
    XDG_CACHE_HOME=$home/cache \
    PATH=/usr/bin:/bin:/usr/sbin:/sbin \
    KOBO_INSTALLER_TESTING=1 \
    KOBO_INSTALLER_BASE_URL="$(file_url "$release")" \
    sh "$INSTALLER" --yes --no-setup --no-path "$@"
}

run_host_update() {
    home=$1
    release=$2
    channel=$3
    shift 3
    selected=$(cat "$home/data/kobo/current")
    HOME=$home \
    XDG_DATA_HOME=$home/data \
    XDG_CACHE_HOME=$home/cache \
    PATH=/usr/bin:/bin:/usr/sbin:/sbin \
    KOBO_INSTALLER_TESTING=1 \
    KOBO_INSTALLER_BASE_URL="$(file_url "$release")" \
    "$@" sh "$home/data/kobo/hosts/$selected/updater.sh" \
        --host-update --channel "$channel" --platform linux-x86_64
}

expect_failure() {
    label=$1
    shift
    if "$@" > "$WORK/failure.out" 2>&1; then
        printf 'expected failure: %s\n' "$label" >&2
        exit 1
    fi
}

stable=$WORK/stable
beta=$WORK/beta
make_release "$stable" 0.3.3 stable stable-one
make_release "$beta" 0.3.4 beta beta-one
grep -F "https://bandarlabs.github.io/Cobalt/install.sh" "$ROOT/README.md" >/dev/null
if grep -Eq 'beta-v[0-9.]+/install\.sh|sh -s -- --beta' "$ROOT/README.md"; then
    printf 'README exposes a beta bootstrap route\n' >&2
    exit 1
fi
expect_failure "release installer beta route" \
    env HOME="$WORK/home-reject-beta" KOBO_INSTALLER_TESTING=1 \
    sh "$INSTALLER" --beta
expect_failure "Pages installer beta route" \
    env HOME="$WORK/home-pages-reject-beta" sh "$PAGES_INSTALLER" --beta

# The separately verified route checks the stable release installer before
# execution with the out-of-band signer.
printf '%s\n' "$SIGNER" > "$WORK/high-assurance-signers"
ssh-keygen -Y verify -q -f "$WORK/high-assurance-signers" \
    -I cobalt-release -n cobalt-host-release \
    -s "$stable/cobalt-host-manifest.txt.sshsig" \
    < "$stable/cobalt-host-manifest.txt"
bootstrap_line=$(awk \
    '$1 == "bootstrap" && $2 == "install.sh" && NF == 4 {print $3 " " $4}' \
    "$stable/cobalt-host-manifest.txt")
IFS=' ' read -r bootstrap_bytes bootstrap_sha <<EOF
$bootstrap_line
EOF
[ "$(size_file "$INSTALLER")" = "$bootstrap_bytes" ]
[ "$(sha256_file "$INSTALLER")" = "$bootstrap_sha" ]

mock_bin=$WORK/mock-bin
mkdir "$mock_bin"
cat > "$mock_bin/curl" <<'EOF'
#!/bin/sh
set -eu
output=
url=
while [ "$#" -gt 0 ]; do
    case "$1" in
        -o)
            output=$2
            shift
            ;;
        https://*) url=$1 ;;
    esac
    shift
done
printf '%s\n' "$url" >> "$KOBO_TEST_URL_LOG"
case "$url" in
    https://github.com/BandarLabs/Cobalt/releases/latest/download/*)
        cp "$KOBO_TEST_RELEASE/${url#https://github.com/BandarLabs/Cobalt/releases/latest/download/}" \
            "$output"
        ;;
    https://github.com/BandarLabs/Cobalt/releases/download/v0.3.3/*)
        cp "$KOBO_TEST_RELEASE/${url#https://github.com/BandarLabs/Cobalt/releases/download/v0.3.3/}" \
            "$output"
        ;;
    *) exit 22 ;;
esac
EOF
chmod 755 "$mock_bin/curl"
# Once main:/docs is promoted, Pages discovers stable and verifies the signed
# release installer before running it.
pages_home=$WORK/home-pages
: > "$WORK/pages-urls"
HOME=$pages_home XDG_DATA_HOME=$pages_home/data \
XDG_CACHE_HOME=$pages_home/cache \
PATH="$mock_bin:/usr/bin:/bin:/usr/sbin:/sbin" \
KOBO_INSTALLER_TESTING=1 KOBO_TEST_RELEASE=$stable \
KOBO_TEST_URL_LOG=$WORK/pages-urls \
sh "$PAGES_INSTALLER" --yes --no-setup --no-path --platform linux-x86_64 \
    >/dev/null
grep -F "/releases/latest/download/cobalt-host-manifest.txt" \
    "$WORK/pages-urls" >/dev/null
grep -F "/releases/latest/download/install.sh" "$WORK/pages-urls" >/dev/null
grep -F "/releases/download/v0.3.3/cobalt-host-manifest.txt" \
    "$WORK/pages-urls" >/dev/null

# Host-only update uses the installed verified updater. Stable is the normal
# path, Beta is explicit, and neither path invokes setup or touches a volume.
host_update_home=$WORK/home-host-update
run_install "$host_update_home" "$stable" --platform linux-x86_64 >/dev/null
setup_release=$host_update_home/data/kobo/releases/0.3.3-stable
setup_state_sha=$(sha256_file "$host_update_home/data/kobo/install-state")
setup_manifest_sha=$(sha256_file "$setup_release/cobalt-host-manifest.txt")
setup_signature_sha=$(sha256_file "$setup_release/cobalt-host-manifest.txt.sig")
setup_device_sha=$(sha256_file "$setup_release/cobalt-0.3.3-KoboRoot.tgz")
command_destination=$(readlink "$host_update_home/.local/bin/kobo")
ln -s "$host_update_home/.local/bin/kobo" "$host_update_home/.local/bin/kobo-compat"
assert_setup_cache_unchanged() {
    [ "$(sha256_file "$host_update_home/data/kobo/install-state")" = "$setup_state_sha" ]
    [ "$(sha256_file "$setup_release/cobalt-host-manifest.txt")" = "$setup_manifest_sha" ]
    [ "$(sha256_file "$setup_release/cobalt-host-manifest.txt.sig")" = "$setup_signature_sha" ]
    [ "$(sha256_file "$setup_release/cobalt-0.3.3-KoboRoot.tgz")" = "$setup_device_sha" ]
    grep -Fx "release $setup_release" "$host_update_home/data/kobo/install-state" >/dev/null
    grep -Fx "channel stable" "$host_update_home/data/kobo/install-state" >/dev/null
    grep -Fx "version 0.3.3" "$host_update_home/data/kobo/install-state" >/dev/null
    grep -Fx "version 0.3.3" "$setup_release/cobalt-host-manifest.txt" >/dev/null
    [ "$(readlink "$host_update_home/.local/bin/kobo")" = "$command_destination" ]
}
attached=$host_update_home/mounted-reader
mkdir -p "$attached/.kobo"
printf 'N365000000000,4.9.77,4.45.23697\n' > "$attached/.kobo/version"
printf 'owner bytes\n' > "$attached/untouched"
run_host_update "$host_update_home" "$stable" stable > "$WORK/already-current.out"
grep -F "already current on the stable channel" "$WORK/already-current.out" >/dev/null
run_host_update "$host_update_home" "$beta" beta >/dev/null
[ "$("$host_update_home/.local/bin/kobo")" = beta-one ]
[ "$("$host_update_home/.local/bin/kobo-compat")" = beta-one ]
selected=$(cat "$host_update_home/data/kobo/current")
grep -Fx beta "$host_update_home/data/kobo/hosts/$selected/CHANNEL" >/dev/null
assert_setup_cache_unchanged
run_host_update "$host_update_home" "$stable" stable >/dev/null
[ "$("$host_update_home/.local/bin/kobo")" = stable-one ]
[ "$("$host_update_home/.local/bin/kobo-compat")" = stable-one ]
selected=$(cat "$host_update_home/data/kobo/current")
grep -Fx stable "$host_update_home/data/kobo/hosts/$selected/CHANNEL" >/dev/null
assert_setup_cache_unchanged
[ "$(cat "$attached/untouched")" = "owner bytes" ]

host_updated=$WORK/host-updated
make_release "$host_updated" 0.3.5 stable stable-two
rm "$host_updated/cobalt-0.3.5-KoboRoot.tgz" \
    "$host_updated/cobalt-host-manifest.txt.sig" \
    "$host_updated/install.sh"
run_host_update "$host_update_home" "$host_updated" stable >/dev/null
[ "$("$host_update_home/.local/bin/kobo")" = stable-two ]
assert_setup_cache_unchanged

activation_failure() {
    point=$1
    version=$2
    marker=$3
    release=$WORK/activation-$version
    make_release "$release" "$version" stable "$marker"
    previous=$(cat "$host_update_home/data/kobo/current")
    previous_output=$("$host_update_home/.local/bin/kobo")
    updater=$host_update_home/data/kobo/hosts/$previous/updater.sh
    set +e
    HOME=$host_update_home XDG_DATA_HOME=$host_update_home/data \
    XDG_CACHE_HOME=$host_update_home/cache \
    PATH=/usr/bin:/bin:/usr/sbin:/sbin \
    KOBO_INSTALLER_TESTING=1 KOBO_TEST_FAIL_AT=$point \
    KOBO_INSTALLER_BASE_URL="$(file_url "$release")" \
    sh "$updater" \
        --host-update --channel stable --platform linux-x86_64 \
        >"$WORK/activation-$point.out" 2>&1
    status=$?
    set -e
    [ "$status" -ne 0 ]
    [ ! -d "$host_update_home/data/kobo/install.lock" ]
    test -z "$(find "$host_update_home/data/kobo" -maxdepth 1 -name '.current.new.*' -print)"
    test -z "$(find "$host_update_home/data/kobo/hosts" -maxdepth 1 -name '.new-*' -print)"
    [ "$(readlink "$host_update_home/.local/bin/kobo")" = "$command_destination" ]
    assert_setup_cache_unchanged
    if [ "$point" = after-selector ]; then
        [ "$("$host_update_home/.local/bin/kobo")" = "$marker" ]
        [ "$("$host_update_home/.local/bin/kobo-compat")" = "$marker" ]
    else
        [ "$(cat "$host_update_home/data/kobo/current")" = "$previous" ]
        [ "$("$host_update_home/.local/bin/kobo")" = "$previous_output" ]
        [ "$("$host_update_home/.local/bin/kobo-compat")" = "$previous_output" ]
    fi
    run_host_update "$host_update_home" "$release" stable >/dev/null
    [ "$("$host_update_home/.local/bin/kobo")" = "$marker" ]
    [ "$("$host_update_home/.local/bin/kobo-compat")" = "$marker" ]
    assert_setup_cache_unchanged
}
activation_failure host-directory 0.3.6 stable-three
activation_failure before-selector 0.3.7 stable-four
activation_failure after-selector 0.3.8 stable-five

host_truncated=$WORK/host-truncated
make_release "$host_truncated" 0.3.9 stable stable-six
printf x >> "$host_truncated/kobo-0.3.9-linux-x86_64.tar.gz"
expect_failure "host update truncated download" \
    run_host_update "$host_update_home" "$host_truncated" stable
host_checksum=$WORK/host-checksum
make_release "$host_checksum" 0.3.9 stable stable-six
printf X | dd of="$host_checksum/kobo-0.3.9-linux-x86_64.tar.gz" \
    bs=1 seek=8 conv=notrunc 2>/dev/null
expect_failure "host update checksum failure" \
    run_host_update "$host_update_home" "$host_checksum" stable
host_signature=$WORK/host-signature
make_release "$host_signature" 0.3.9 stable stable-six
printf x >> "$host_signature/cobalt-host-manifest.txt"
expect_failure "host update signature failure" \
    run_host_update "$host_update_home" "$host_signature" stable

mkdir "$host_update_home/data/kobo/install.lock"
printf '%s\n' "$$" > "$host_update_home/data/kobo/install.lock/pid"
expect_failure "host update lock contention" \
    run_host_update "$host_update_home" "$host_updated" stable
printf '%s\n' 999999 > "$host_update_home/data/kobo/install.lock/pid"
expect_failure "host update stale-looking lock" \
    run_host_update "$host_update_home" "$host_updated" stable
rm -rf "$host_update_home/data/kobo/install.lock"
expect_failure "host update unsupported platform" \
    run_host_update "$host_update_home" "$host_updated" stable \
    env KOBO_TEST_UNAME_S=FreeBSD
conflicting_path=$WORK/host-update-conflict
mkdir "$conflicting_path"
printf '#!/bin/sh\nexit 0\n' > "$conflicting_path/kobo"
chmod 755 "$conflicting_path/kobo"
selected=$(cat "$host_update_home/data/kobo/current")
expect_failure "host update command conflict" env \
    PATH="$conflicting_path:/usr/bin:/bin:/usr/sbin:/sbin" \
    HOME="$host_update_home" XDG_DATA_HOME="$host_update_home/data" \
    XDG_CACHE_HOME="$host_update_home/cache" KOBO_INSTALLER_TESTING=1 \
    KOBO_INSTALLER_BASE_URL="$(file_url "$beta")" \
    sh "$host_update_home/data/kobo/hosts/$selected/updater.sh" \
    --host-update --channel beta

# Clean install, update, and idempotent rerun.
home=$WORK/home-clean
run_install "$home" "$stable" --platform linux-x86_64
[ "$("$home/.local/bin/kobo")" = stable-one ]
run_install "$home" "$stable" --platform linux-x86_64
[ "$("$home/.local/bin/kobo")" = stable-one ]
updated=$WORK/updated
make_release "$updated" 0.3.5 stable stable-two
run_install "$home" "$updated" --platform linux-x86_64
[ "$("$home/.local/bin/kobo")" = stable-two ]
grep -Fx "version 0.3.5" "$home/data/kobo/install-state" >/dev/null
expect_failure "automatic downgrade" \
    run_install "$home" "$stable" --platform linux-x86_64
run_install "$home" "$stable" --version 0.3.3 --platform linux-x86_64 >/dev/null

# PATH configuration is marked and idempotent.
path_home=$WORK/home-path
HOME=$path_home XDG_DATA_HOME=$path_home/data XDG_CACHE_HOME=$path_home/cache \
PATH=/usr/bin:/bin:/usr/sbin:/sbin SHELL=/bin/sh \
KOBO_INSTALLER_TESTING=1 \
KOBO_INSTALLER_BASE_URL="$(file_url "$stable")" \
sh "$INSTALLER" --yes --no-setup --platform linux-x86_64 >/dev/null
HOME=$path_home XDG_DATA_HOME=$path_home/data XDG_CACHE_HOME=$path_home/cache \
PATH=/usr/bin:/bin:/usr/sbin:/sbin SHELL=/bin/sh \
KOBO_INSTALLER_TESTING=1 \
KOBO_INSTALLER_BASE_URL="$(file_url "$stable")" \
sh "$INSTALLER" --yes --no-setup --platform linux-x86_64 >/dev/null
[ "$(grep -Fc '# >>> Cobalt kobo installer >>>' "$path_home/.profile")" -eq 1 ]

# A live or stale-looking lock fails closed. Concurrent attempts cannot delete
# and replace one another's lock; reclamation is an explicit manual action.
lock_home=$WORK/home-lock
mkdir -p "$lock_home/data/kobo/install.lock"
printf '%s\n' "$$" > "$lock_home/data/kobo/install.lock/pid"
expect_failure "active install lock" \
    run_install "$lock_home" "$stable" --platform linux-x86_64
printf '%s\n' 999999 > "$lock_home/data/kobo/install.lock/pid"
set +e
run_install "$lock_home" "$stable" --platform linux-x86_64 \
    >"$WORK/lock-one.out" 2>&1 &
lock_one=$!
run_install "$lock_home" "$stable" --platform linux-x86_64 \
    >"$WORK/lock-two.out" 2>&1 &
lock_two=$!
wait "$lock_one"
lock_one_status=$?
wait "$lock_two"
lock_two_status=$?
set -e
[ "$lock_one_status" -ne 0 ]
[ "$lock_two_status" -ne 0 ]
[ "$(cat "$lock_home/data/kobo/install.lock/pid")" = 999999 ]
rm -rf "$lock_home/data/kobo/install.lock"
run_install "$lock_home" "$stable" --platform linux-x86_64 >/dev/null

# Stable and explicit version enforcement.
run_install "$WORK/home-version" "$stable" --version 0.3.3 --platform macos-arm64
expect_failure "wrong explicit version" \
    run_install "$WORK/home-wrong-version" "$stable" --version 9.9.9 --platform linux-x86_64

# Every supported platform, including an Apple Silicon selection under Rosetta.
for platform in macos-x86_64 macos-arm64 linux-x86_64 linux-arm64; do
    run_install "$WORK/home-$platform" "$stable" --platform "$platform"
    grep -Fx "platform $platform" \
        "$WORK/home-$platform/data/kobo/install-state" >/dev/null
done
HOME=$WORK/home-rosetta \
XDG_DATA_HOME=$WORK/home-rosetta/data \
XDG_CACHE_HOME=$WORK/home-rosetta/cache \
PATH=/usr/bin:/bin:/usr/sbin:/sbin \
KOBO_INSTALLER_TESTING=1 \
KOBO_INSTALLER_BASE_URL="$(file_url "$stable")" \
KOBO_TEST_UNAME_S=Darwin KOBO_TEST_UNAME_M=x86_64 KOBO_TEST_ROSETTA=1 \
sh "$INSTALLER" --yes --no-setup --no-path > "$WORK/rosetta.out"
grep -F "native Apple Silicon" "$WORK/rosetta.out" >/dev/null
grep -Fx "platform macos-arm64" \
    "$WORK/home-rosetta/data/kobo/install-state" >/dev/null

# WSL selects Linux and never claims to eject a drive.
HOME=$WORK/home-wsl \
XDG_DATA_HOME=$WORK/home-wsl/data \
XDG_CACHE_HOME=$WORK/home-wsl/cache \
PATH=/usr/bin:/bin:/usr/sbin:/sbin \
KOBO_INSTALLER_TESTING=1 \
KOBO_INSTALLER_BASE_URL="$(file_url "$stable")" \
KOBO_TEST_UNAME_S=Linux KOBO_TEST_UNAME_M=x86_64 KOBO_TEST_WSL=1 \
sh "$INSTALLER" --yes --no-setup --no-path > "$WORK/wsl.out"
grep -F "eject them from Windows, not WSL" "$WORK/wsl.out" >/dev/null
if grep -F "volume ejected" "$WORK/wsl.out" >/dev/null; then
    printf 'WSL output falsely claimed an eject\n' >&2
    exit 1
fi

# Existing unrelated command and target conflicts fail closed.
conflict=$WORK/home-conflict
mkdir -p "$conflict/.local/bin"
printf '#!/bin/sh\nexit 0\n' > "$conflict/.local/bin/kobo"
chmod 755 "$conflict/.local/bin/kobo"
expect_failure "unmanaged target conflict" \
    run_install "$conflict" "$stable" --platform linux-x86_64
path_conflict=$WORK/path-conflict
mkdir -p "$path_conflict/bin"
printf '#!/bin/sh\nexit 0\n' > "$path_conflict/bin/kobo"
chmod 755 "$path_conflict/bin/kobo"
expect_failure "PATH conflict" env PATH="$path_conflict/bin:$PATH" \
    HOME="$path_conflict/home" XDG_DATA_HOME="$path_conflict/home/data" \
    XDG_CACHE_HOME="$path_conflict/home/cache" KOBO_INSTALLER_TESTING=1 \
    KOBO_INSTALLER_BASE_URL="$(file_url "$stable")" \
    sh "$INSTALLER" --yes --no-setup --no-path --platform linux-x86_64

# Truncation, checksum damage, and signature damage are independently refused.
truncated=$WORK/truncated
cp -R "$stable" "$truncated"
printf x >> "$truncated/kobo-0.3.3-linux-x86_64.tar.gz"
expect_failure "truncated download" \
    run_install "$WORK/home-truncated" "$truncated" --platform linux-x86_64
checksum=$WORK/checksum
cp -R "$stable" "$checksum"
printf X | dd of="$checksum/kobo-0.3.3-linux-x86_64.tar.gz" bs=1 seek=8 conv=notrunc 2>/dev/null
expect_failure "checksum failure" \
    run_install "$WORK/home-checksum" "$checksum" --platform linux-x86_64
bad_signature=$WORK/bad-signature
cp -R "$stable" "$bad_signature"
printf x >> "$bad_signature/cobalt-host-manifest.txt"
expect_failure "signature failure" \
    run_install "$WORK/home-signature" "$bad_signature" --platform linux-x86_64
duplicate=$WORK/duplicate-field
cp -R "$stable" "$duplicate"
printf 'version 9.9.9\n' >> "$duplicate/cobalt-host-manifest.txt"
ssh-keygen -q -Y sign -f "$WORK/signing" -n cobalt-host-release \
    "$duplicate/cobalt-host-manifest.txt" >/dev/null
mv "$duplicate/cobalt-host-manifest.txt.sig" \
    "$duplicate/cobalt-host-manifest.txt.sshsig"
printf '%0128d\n' 0 > "$duplicate/cobalt-host-manifest.txt.sig"
expect_failure "duplicate signed manifest field" \
    run_install "$WORK/home-duplicate" "$duplicate" --platform linux-x86_64

printf 'installer tests passed\n'

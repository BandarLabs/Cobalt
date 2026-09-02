#!/bin/sh
set -eu

ROOT=$(CDPATH='' cd -- "$(dirname "$0")/.." && pwd)
WORK=$ROOT/target/installer-tests
WORKSPACE_VERSION=$(sed -n 's/^version = "\([0-9][0-9.]*\)"$/\1/p' "$ROOT/Cargo.toml")
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

ssh-keygen -q -t ed25519 -N '' -f "$WORK/signing" >/dev/null
SIGNER="cobalt-release $(cat "$WORK/signing.pub")"
SIGNER=$(printf '%s\n' "$SIGNER" | awk '{print $1 " " $2 " " $3}')
INSTALLER=$WORK/install.sh
sed "s|cobalt-release ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIL7XUR3p+tvPgftO/kRbigc8gagzP2RBDG3tWIu/1KXe|$SIGNER|" \
    "$ROOT/install.sh" > "$INSTALLER"

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
    KOBO_INSTALLER_BASE_URL="file://$release" \
    sh "$INSTALLER" --yes --no-setup --no-path "$@"
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
make_release "$beta" "$WORKSPACE_VERSION" beta beta-one
grep -F \
    "https://github.com/BandarLabs/Cobalt/releases/download/beta-v${WORKSPACE_VERSION}/install.sh" \
    "$ROOT/README.md" >/dev/null
grep -F "sh -s -- --beta --version ${WORKSPACE_VERSION}" "$ROOT/README.md" >/dev/null

# The first beta does not depend on a stable installer asset. An explicit beta
# version resolves every download against its immutable beta-vX.Y.Z release.
printf '%s\n' "$SIGNER" > "$WORK/high-assurance-signers"
ssh-keygen -Y verify -q -f "$WORK/high-assurance-signers" \
    -I cobalt-release -n cobalt-host-release \
    -s "$beta/cobalt-host-manifest.txt.sshsig" \
    < "$beta/cobalt-host-manifest.txt"
bootstrap_line=$(awk \
    '$1 == "bootstrap" && $2 == "install.sh" && NF == 4 {print $3 " " $4}' \
    "$beta/cobalt-host-manifest.txt")
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
prefix=https://github.com/BandarLabs/Cobalt/releases/download/beta-v$KOBO_TEST_VERSION/
case "$url" in
    "$prefix"*) cp "$KOBO_TEST_RELEASE/${url#"$prefix"}" "$output" ;;
    *) exit 22 ;;
esac
EOF
chmod 755 "$mock_bin/curl"
first_beta_home=$WORK/home-first-beta
HOME=$first_beta_home XDG_DATA_HOME=$first_beta_home/data \
XDG_CACHE_HOME=$first_beta_home/cache \
PATH="$mock_bin:/usr/bin:/bin:/usr/sbin:/sbin" \
KOBO_INSTALLER_TESTING=1 KOBO_TEST_RELEASE=$beta \
KOBO_TEST_VERSION=$WORKSPACE_VERSION \
KOBO_TEST_URL_LOG=$WORK/first-beta-urls \
sh "$INSTALLER" --yes --no-setup --no-path --platform linux-x86_64 \
    --beta --version "$WORKSPACE_VERSION" >/dev/null
if grep -F '/releases/latest/download/' "$WORK/first-beta-urls" >/dev/null; then
    printf 'first beta unexpectedly depended on a stable latest asset\n' >&2
    exit 1
fi
grep -F "/releases/download/beta-v${WORKSPACE_VERSION}/cobalt-host-manifest.txt" \
    "$WORK/first-beta-urls" >/dev/null

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
KOBO_INSTALLER_BASE_URL="file://$stable" \
sh "$INSTALLER" --yes --no-setup --platform linux-x86_64 >/dev/null
HOME=$path_home XDG_DATA_HOME=$path_home/data XDG_CACHE_HOME=$path_home/cache \
PATH=/usr/bin:/bin:/usr/sbin:/sbin SHELL=/bin/sh \
KOBO_INSTALLER_TESTING=1 \
KOBO_INSTALLER_BASE_URL="file://$stable" \
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

# Stable/beta and explicit version enforcement.
run_install "$WORK/home-beta" "$beta" --beta --platform linux-arm64
grep -Fx "channel beta" "$WORK/home-beta/data/kobo/install-state" >/dev/null
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
KOBO_INSTALLER_BASE_URL="file://$stable" \
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
KOBO_INSTALLER_BASE_URL="file://$stable" \
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
    KOBO_INSTALLER_BASE_URL="file://$stable" \
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

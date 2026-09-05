#!/bin/sh
set -eu

bootstrap_python=
for candidate in /usr/bin/python3 /opt/homebrew/bin/python3; do
  if [ -x "$candidate" ]; then
    bootstrap_python=$candidate
    break
  fi
done
if [ -z "$bootstrap_python" ]; then
  echo "a trusted system Python 3 is required" >&2
  exit 1
fi

script_dir=${0%/*}
if [ "$script_dir" = "$0" ]; then
  script_dir=.
fi
repo=$(CDPATH= cd -- "$script_dir/.." && /bin/pwd -P)
account_home=$("$bootstrap_python" -I -c \
  'import os, pwd; print(pwd.getpwuid(os.getuid()).pw_dir)')
account_user=$("$bootstrap_python" -I -c \
  'import os, pwd; print(pwd.getpwuid(os.getuid()).pw_name)')
case $account_home in
  /*) ;;
  *) echo "could not determine an absolute account home" >&2; exit 1 ;;
esac
case $account_home in
  *:*) echo "account home cannot be represented safely in PATH" >&2; exit 1 ;;
esac
trusted_path="/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin:/opt/homebrew/opt/llvm/bin:$account_home/.cargo/bin"
exec /usr/bin/env -i \
  HOME="$account_home" \
  USER="$account_user" \
  LOGNAME="$account_user" \
  PATH="$trusted_path" \
  LANG=C \
  LC_ALL=C \
  TZ=UTC \
  GIT_CONFIG_NOSYSTEM=1 \
  GIT_CONFIG_GLOBAL=/dev/null \
  CARGO_TERM_COLOR=never \
  /bin/sh -s -- "$repo" "$bootstrap_python" "$@" <<'COBALT_FLASHCARDS_CLEAN_SCRIPT'
set -eu

repo=$1
PYTHON3=$2
shift 2

ancestor=${repo%/*}
while :; do
  for config in \
    "$ancestor/.cargo/config.toml" \
    "$ancestor/.cargo/config" \
    "$ancestor/rust-toolchain.toml" \
    "$ancestor/rust-toolchain"; do
    if [ -e "$config" ]; then
      echo "refusing untracked parent toolchain configuration: $config" >&2
      exit 1
    fi
  done
  if [ "$ancestor" = / ]; then
    break
  fi
  ancestor=${ancestor%/*}
  if [ -z "$ancestor" ]; then
    ancestor=/
  fi
done

if [ "$#" -ne 1 ]; then
  echo "usage: scripts/audit-flashcards-artifacts.sh TARGET_ROOT" >&2
  exit 2
fi

if [ -n "$(git -C "$repo" status --porcelain --untracked-files=normal)" ]; then
  echo "artifact audits require a clean committed source tree" >&2
  exit 1
fi

find_arm_tool() {
  for candidate in "$@"; do
    for directory in /usr/bin /bin /opt/homebrew/bin /opt/homebrew/opt/llvm/bin; do
      candidate_path=$directory/$candidate
      if [ -x "$candidate_path" ] &&
        "$candidate_path" --version >/dev/null 2>&1; then
        printf '%s\n' "$candidate_path"
        return 0
      fi
    done
  done
  return 1
}

find_rust_lld() {
  for candidate in "$rust_sysroot"/lib/rustlib/*/bin/rust-lld; do
    if [ -x "$candidate" ]; then
      printf '%s\n' "$candidate"
      return 0
    fi
  done
  return 1
}

if [ -x "$HOME/.cargo/bin/rustup" ]; then
  RUSTUP="$HOME/.cargo/bin/rustup"
  default_toolchain=$("$RUSTUP" default)
  active_toolchain=$(CDPATH= cd -- "$repo" && "$RUSTUP" show active-toolchain)
  if [ "$active_toolchain" != "$default_toolchain" ]; then
    echo "repository inherits a non-default rustup toolchain override" >&2
    exit 1
  fi
  toolchain_name=${default_toolchain%% *}
  rust_sysroot=$("$RUSTUP" run "$toolchain_name" rustc --print sysroot)
else
  rust_driver=
  for candidate in /usr/bin/rustc /opt/homebrew/bin/rustc; do
    if [ -x "$candidate" ] && "$candidate" --version >/dev/null 2>&1; then
      rust_driver=$candidate
      break
    fi
  done
  if [ -z "$rust_driver" ]; then
    echo "no trusted Rust toolchain driver was found" >&2
    exit 1
  fi
  rust_sysroot=$(CDPATH= cd -- "$repo" && "$rust_driver" --print sysroot)
fi
RUSTC="$rust_sysroot/bin/rustc"
CARGO="$rust_sysroot/bin/cargo"
if [ ! -x "$RUSTC" ] || [ ! -x "$CARGO" ]; then
  echo "the active Rust sysroot lacks rustc or cargo" >&2
  exit 1
fi
PATH="$rust_sysroot/bin:$PATH"
export PATH RUSTC
CARGO_ABOUT="$HOME/.cargo/bin/cargo-about"
if [ ! -x "$CARGO_ABOUT" ] ||
  [ "$("$CARGO_ABOUT" --version)" != "cargo-about 0.6.4" ]; then
  echo "cargo-about 0.6.4 was not found at the fixed Cargo tool path" >&2
  exit 1
fi
export CARGO_ABOUT

case $1 in
  /*) target_root=$1 ;;
  *) target_root=$(pwd)/$1 ;;
esac
target_root=$(CDPATH= cd -- "$target_root" && pwd)
device="$target_root/armv7-unknown-linux-musleabihf/release/kobo-flashcards"
audit_device="$target_root/audit-unstripped/armv7-unknown-linux-musleabihf/release/kobo-flashcards"
package="$target_root/artifacts/flashcards-validation.cobalt-app"
manifest="$target_root/artifacts/flashcards.manifest.json"
public_key="$target_root/artifacts/validation-public-key.txt"
catalog="$target_root/artifacts/catalog/catalog.json"
catalog_signature="$target_root/artifacts/catalog/catalog.sig"
host="$target_root/host-target/release/flashcards-import"
source_commit_file="$target_root/artifacts/flashcards-import.source-commit.txt"
host_notice_file="$target_root/artifacts/flashcards-import.notice.txt"
host_licenses_file="$target_root/artifacts/flashcards-import.licenses.txt"
audit_report="$target_root/artifacts/ARTIFACT-AUDIT.txt"
sentinel="$target_root/.cobalt-flashcards-validation-root"
japanese_font="$repo/crates/kobo-flashcards-format/fonts/CobaltJapanese-Regular.otf"
readelf=$(find_arm_tool \
  armv7-unknown-linux-musleabihf-readelf \
  armv7-linux-musleabihf-readelf \
  arm-linux-musleabihf-readelf \
  arm-linux-gnueabihf-readelf) || {
    echo "no supported ARM readelf tool was found" >&2
    exit 1
  }
CC_armv7_unknown_linux_musleabihf=$(find_arm_tool \
  armv7-unknown-linux-musleabihf-gcc \
  armv7-linux-musleabihf-gcc \
  arm-linux-musleabihf-gcc \
  arm-linux-gnueabihf-gcc) || {
    echo "no supported ARM C compiler was found" >&2
    exit 1
  }
AR_armv7_unknown_linux_musleabihf=$(find_arm_tool \
  armv7-unknown-linux-musleabihf-ar \
  armv7-linux-musleabihf-ar \
  arm-linux-musleabihf-ar \
  arm-linux-gnueabihf-ar) || {
    echo "no supported ARM archiver was found" >&2
    exit 1
  }
CARGO_TARGET_ARMV7_UNKNOWN_LINUX_MUSLEABIHF_LINKER=$(find_rust_lld) || {
  echo "rust-lld was not found in the active Rust toolchain" >&2
  exit 1
}
export CC_armv7_unknown_linux_musleabihf
export AR_armv7_unknown_linux_musleabihf
export CARGO_TARGET_ARMV7_UNKNOWN_LINUX_MUSLEABIHF_LINKER
unset RUSTFLAGS CARGO_BUILD_RUSTFLAGS
CARGO_ENCODED_RUSTFLAGS=
RUSTC_WRAPPER=
RUSTC_WORKSPACE_WRAPPER=
CARGO_BUILD_RUSTC_WRAPPER=
CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER=
export \
  CARGO_ENCODED_RUSTFLAGS \
  RUSTC_WRAPPER \
  RUSTC_WORKSPACE_WRAPPER \
  CARGO_BUILD_RUSTC_WRAPPER \
  CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER
expected_validation_key=d759793bbc13a2819a827c76adb6fba8a49aee007f49f2d0992d99b825ad2c48
expected_japanese_font_hash=150c82a7b6a4e39645099b3d27c96a00a148a1f57faf523027559910059c2dc0
rm -f "$audit_report"
cargo_home="$target_root/cargo-home"
rm -rf "$cargo_home"
mkdir -p "$cargo_home/registry" "$cargo_home/git"
for cache in registry/index registry/cache git/db; do
  if [ -d "$HOME/.cargo/$cache" ]; then
    mkdir -p "$cargo_home/${cache%/*}"
    ln -s "$HOME/.cargo/$cache" "$cargo_home/$cache"
  fi
done
export CARGO_HOME="$cargo_home"

for path in "$device" "$audit_device" "$package" "$manifest" "$public_key" "$catalog" "$catalog_signature" "$source_commit_file" "$sentinel"; do
  if [ ! -f "$path" ]; then
    echo "missing artifact: $path" >&2
    exit 1
  fi
done
if [ ! -f "$japanese_font" ] ||
  [ "$(shasum -a 256 "$japanese_font" | awk '{print $1}')" != "$expected_japanese_font_hash" ]; then
  echo "bounded Japanese font does not match its documented deterministic source" >&2
  exit 1
fi
if [ -L "$sentinel" ] ||
  [ "$(cat "$sentinel")" != "Cobalt Flashcards validation root v1" ]; then
  echo "validation target sentinel is invalid" >&2
  exit 1
fi

source_commit=$(tr -d '\n' < "$source_commit_file")
if [ "$source_commit" != "$(git -C "$repo" rev-parse HEAD)" ]; then
  echo "artifact source commit does not match this checkout" >&2
  exit 1
fi

assert_source_unchanged() {
  if [ -n "$(git -C "$repo" status --porcelain --untracked-files=normal)" ] ||
    [ "$source_commit" != "$(git -C "$repo" rev-parse HEAD)" ]; then
    echo "source tree changed during artifact audit" >&2
    exit 1
  fi
}

if [ "$(tr -d '\n' < "$public_key")" != "$expected_validation_key" ]; then
  echo "validation package public key is not the fixed audit key" >&2
  exit 1
fi

fresh_device_root="$target_root/audit-device-fresh"
rm -rf "$fresh_device_root"
mkdir -p "$fresh_device_root/production" "$fresh_device_root/unstripped" "$target_root/build-tmp"
(
  cd "$repo"
  export TMPDIR="$target_root/build-tmp"
  CARGO_TARGET_DIR="$fresh_device_root/production" \
    "$CARGO" build --quiet --locked --release \
    --target armv7-unknown-linux-musleabihf -p kobo-flashcards
  CARGO_TARGET_DIR="$fresh_device_root/unstripped" \
  CARGO_PROFILE_RELEASE_STRIP=none \
    "$CARGO" build --quiet --locked --release \
    --target armv7-unknown-linux-musleabihf -p kobo-flashcards
)
assert_source_unchanged
fresh_device="$fresh_device_root/production/armv7-unknown-linux-musleabihf/release/kobo-flashcards"
fresh_audit_device="$fresh_device_root/unstripped/armv7-unknown-linux-musleabihf/release/kobo-flashcards"
if ! cmp "$fresh_device" "$device" >/dev/null; then
  echo "production device ELF differs from the fresh audited-source build" >&2
  exit 1
fi
if ! cmp "$fresh_audit_device" "$audit_device" >/dev/null; then
  echo "unstripped device ELF differs from the fresh audited-source build" >&2
  exit 1
fi
rm -rf "$fresh_device_root" "$target_root/host-target"

audit_tools="$target_root/audit-tools"
rm -rf "$audit_tools"
mkdir -p "$audit_tools"
(
  cd "$repo"
  TMPDIR="$target_root/build-tmp" \
  COBALT_SOURCE_COMMIT="$source_commit" \
  CARGO_TARGET_DIR="$audit_tools" \
    "$CARGO" build --quiet --locked --release -p kobo-cli -p kobo-flashcards-import
)
assert_source_unchanged
trusted_cli="$audit_tools/release/kobo"
trusted_host="$audit_tools/release/flashcards-import"
audited_host="$target_root/artifacts/.flashcards-import.audited"
cp "$trusted_host" "$audited_host"
mkdir -p "$(dirname "$host")"
mv "$audited_host" "$host"
if ! cmp "$trusted_host" "$host" >/dev/null; then
  echo "finalized host helper differs from the fresh reference build" >&2
  exit 1
fi
"$host" --notice > "$host_notice_file"
"$host" --licenses > "$host_licenses_file"

device_tree=$(
  cd "$repo"
  "$CARGO" tree --locked --offline -p kobo-flashcards --edges normal --prefix none
)
if printf '%s\n' "$device_tree" |
  grep -E '^(anki|anki_i18n|anki_io|anki_proto) v' >/dev/null; then
  echo "device dependency tree contains Anki packages" >&2
  exit 1
fi

host_tree=$(
  cd "$repo"
  "$CARGO" tree --locked --offline -p kobo-flashcards-import --edges normal --prefix none
)
for package_name in anki anki_i18n anki_io anki_proto; do
  if ! printf '%s\n' "$host_tree" |
    grep -E "^${package_name} v" >/dev/null; then
    echo "host dependency tree is missing $package_name" >&2
    exit 1
  fi
done

"$PYTHON3" -I - "$repo" <<'PY'
import json
import subprocess
import sys

revision = "9e32ad8849068510a82273889c21b22e1acf0949"
metadata = json.loads(
    subprocess.run(
        [
            "cargo",
            "metadata",
            "--format-version",
            "1",
            "--locked",
            "--offline",
            "--manifest-path",
            "crates/kobo-flashcards-import/Cargo.toml",
        ],
        cwd=sys.argv[1],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
)
expected = {"anki", "anki_i18n", "anki_io", "anki_proto"}
sources = {
    package["name"]: package.get("source") or ""
    for package in metadata["packages"]
    if package["name"] in expected
}
if set(sources) != expected:
    raise SystemExit("host Anki package inventory is incomplete")
for name, source in sources.items():
    if f"rev={revision}" not in source or not source.endswith(f"#{revision}"):
        raise SystemExit(f"{name} is not pinned to the required Anki revision")
PY

TMPDIR="$target_root/build-tmp" \
  "$PYTHON3" -I "$repo/scripts/generate-flashcards-licenses.py" --check
assert_source_unchanged

if ! device_headers=$("$readelf" -lW "$device"); then
  echo "production device artifact is not a readable ELF" >&2
  exit 1
fi
if ! audit_headers=$("$readelf" -lW "$audit_device"); then
  echo "unstripped device artifact is not a readable ELF" >&2
  exit 1
fi
if ! audit_symbols=$("$readelf" -Ws "$audit_device"); then
  echo "unstripped device symbol table could not be read" >&2
  exit 1
fi

if printf '%s\n' "$device_headers" | grep -E 'INTERP|DYNAMIC' >/dev/null; then
  echo "device binary is not static" >&2
  exit 1
fi
if printf '%s\n' "$audit_headers" | grep -E 'INTERP|DYNAMIC' >/dev/null; then
  echo "unstripped audit binary is not static" >&2
  exit 1
fi
if ! printf '%s\n' "$audit_symbols" | grep -E 'FUNC|OBJECT' >/dev/null; then
  echo "unstripped audit binary has no inspectable symbol table" >&2
  exit 1
fi
if printf '%s\n' "$audit_symbols" |
  grep -E 'FUNC|OBJECT' |
  grep -E '(^|[^[:alnum:]_])anki(_|::|$)|[0-9]anki|ankitects|rslib|anki_i18n' >/dev/null; then
  echo "device binary exposes Anki-linked symbols" >&2
  exit 1
fi
if printf '%s\n' "$audit_symbols" |
  grep -E 'FUNC|OBJECT' |
  grep -Ei 'kobo_net|reqwest|rustls|TcpStream|UdpSocket|getaddrinfo|gethostbyname|getnameinfo|freeaddrinfo|inet_(addr|aton|ntoa|ntop|pton)|res_query' >/dev/null; then
  echo "device binary exposes remote-network implementation symbols" >&2
  exit 1
fi

"$PYTHON3" -I - "$repo/apps/catalog.json" "$device" "$manifest" "$catalog" "$package" <<'PY'
import hashlib
import json
import sys
from pathlib import Path

catalog = json.loads(Path(sys.argv[1]).read_text())
app = next(app for app in catalog["apps"] if app["id"] == "flashcards")
device = Path(sys.argv[2]).read_bytes()
actual = Path(sys.argv[3]).read_bytes()
validation_catalog = json.loads(Path(sys.argv[4]).read_text())
package = Path(sys.argv[5]).read_bytes()
manifest = {
    "format_version": 1,
    "id": app["id"],
    "display_name": app["display_name"],
    "short_label": app["short_label"],
    "summary": app["summary"],
    "version": app["version"],
    "minimum_cobalt_version": app["minimum_cobalt_version"],
    "glyph": app["glyph"],
    "capabilities": app["capabilities"],
    "binary_sha256": hashlib.sha256(device).hexdigest(),
    "binary_bytes": len(device),
}
order = [
    "format_version",
    "id",
    "display_name",
    "short_label",
    "summary",
    "version",
    "minimum_cobalt_version",
    "glyph",
    "capabilities",
    "binary_sha256",
    "binary_bytes",
]
expected = (
    "{"
    + ",".join(
        json.dumps(key, separators=(",", ":"))
        + ":"
        + json.dumps(manifest[key], ensure_ascii=False, separators=(",", ":"))
        for key in order
    )
    + "}"
).encode()
if actual != expected:
    raise SystemExit("artifact manifest differs from apps/catalog.json and device ELF")
entries = validation_catalog.get("entries", [])
if len(entries) != 1:
    raise SystemExit("validation catalog must contain exactly one entry")
entry = entries[0]
if entry.get("manifest") != manifest:
    raise SystemExit("validation catalog manifest differs from source metadata")
if entry.get("package_sha256") != hashlib.sha256(package).hexdigest():
    raise SystemExit("validation catalog package digest differs")
if entry.get("package_bytes") != len(package):
    raise SystemExit("validation catalog package length differs")
if entry.get("package_url") != "https://example.invalid/flashcards-validation.cobalt-app":
    raise SystemExit("validation catalog package URL differs")
PY

"$trusted_cli" app-verify \
  --package "$package" \
  --public-key "$public_key" \
  --manifest "$manifest" \
  --binary "$device" >/dev/null
"$trusted_cli" app-catalog-verify \
  --catalog "$catalog" \
  --signature "$catalog_signature" \
  --public-key "$public_key" \
  --package "$package" >/dev/null

for path in "$device" "$package"; do
  if strings "$path" |
    grep -E 'Anki|AnkiDroid|ankitects|anki_i18n|rslib|9e32ad8849068510a82273889c21b22e1acf0949' >/dev/null; then
    echo "device artifact contains host-only Anki branding or source material: $path" >&2
    exit 1
  fi
done

for required in \
  'Flashcards device notice' \
  'Cobalt Japanese font source' \
  '165c01b46ea533872e002e0785ff17e44f6d97d8' \
  "$expected_japanese_font_hash" \
  'cobalt-flashcards-converter-v1' \
  '"capabilities":[]'; do
  if ! strings "$package" | grep -F "$required" >/dev/null; then
    echo "device package is missing its neutral Cobalt notice/format marker" >&2
    exit 1
  fi
done

"$PYTHON3" -I - "$japanese_font" "$device" "$host" <<'PY'
import sys
from pathlib import Path

font = Path(sys.argv[1]).read_bytes()
for artifact in map(Path, sys.argv[2:]):
    if font not in artifact.read_bytes():
        raise SystemExit(f"{artifact} does not contain the audited Japanese font bytes")
PY

if [ -e "$repo/licenses/LICENSE-AnkiDroid.txt" ]; then
  echo "standalone AnkiDroid notice remains in the current source tree" >&2
  exit 1
fi

for required in \
  '9e32ad8849068510a82273889c21b22e1acf0949' \
  "$source_commit" \
  'GNU AFFERO GENERAL PUBLIC LICENSE' \
  'Corresponding source for the Flashcards host converter' \
  'Corresponding source for the Cobalt Japanese font subset' \
  '165c01b46ea533872e002e0785ff17e44f6d97d8' \
  "$expected_japanese_font_hash" \
  'Flashcards host helper non-Anki dependency notices'; do
  if ! strings "$host" | grep -F "$required" >/dev/null; then
    echo "host helper is missing required Anki source/licence material" >&2
    exit 1
  fi
done

if ! "$host" --licenses |
  grep -F 'Corresponding source for the Flashcards host converter' >/dev/null; then
  echo "host helper does not expose corresponding-source instructions" >&2
  exit 1
fi
if ! "$host" --licenses | grep -F "$source_commit" >/dev/null; then
  echo "host helper does not expose its exact Cobalt source commit" >&2
  exit 1
fi
notice_hash=$("$host" --notice | shasum -a 256 | awk '{print $1}')
notice_file_hash=$(shasum -a 256 "$host_notice_file" | awk '{print $1}')
if [ "$notice_hash" != "$notice_file_hash" ]; then
  echo "host notice sidecar differs from helper output" >&2
  exit 1
fi
licenses_hash=$("$host" --licenses | shasum -a 256 | awk '{print $1}')
licenses_file_hash=$(shasum -a 256 "$host_licenses_file" | awk '{print $1}')
if [ "$licenses_hash" != "$licenses_file_hash" ]; then
  echo "host licence/source sidecar differs from helper output" >&2
  exit 1
fi

rm -rf \
  "$audit_tools" \
  "$fresh_device_root" \
  "$target_root/build-tmp" \
  "$target_root/cargo-home"
assert_source_unchanged

"$PYTHON3" -I - "$target_root" <<'PY'
import sys
from pathlib import Path

root = Path(sys.argv[1])
expected = {
    ".cobalt-flashcards-validation-root",
    "armv7-unknown-linux-musleabihf",
    "armv7-unknown-linux-musleabihf/release",
    "armv7-unknown-linux-musleabihf/release/kobo-flashcards",
    "audit-unstripped",
    "audit-unstripped/armv7-unknown-linux-musleabihf",
    "audit-unstripped/armv7-unknown-linux-musleabihf/release",
    "audit-unstripped/armv7-unknown-linux-musleabihf/release/kobo-flashcards",
    "host-target",
    "host-target/release",
    "host-target/release/flashcards-import",
    "artifacts",
    "artifacts/catalog",
    "artifacts/catalog/catalog.json",
    "artifacts/catalog/catalog.sig",
    "artifacts/flashcards-validation.cobalt-app",
    "artifacts/flashcards.manifest.json",
    "artifacts/validation-public-key.txt",
    "artifacts/flashcards-import.source-commit.txt",
    "artifacts/flashcards-import.notice.txt",
    "artifacts/flashcards-import.licenses.txt",
}
for path in root.rglob("*"):
    relative = path.relative_to(root).as_posix()
    if path.is_symlink():
        raise SystemExit(f"symlink remains in audited target root: {relative!r}")
    if relative not in expected:
        raise SystemExit(f"unexpected unaudited path remains in target root: {relative!r}")
if {path.relative_to(root).as_posix() for path in root.rglob("*")} != expected:
    raise SystemExit("audited target root is missing an expected path")
PY

{
  echo "device dependency tree: no Anki packages"
  echo "device ELF/package strings and unstripped symbols: no Anki or AnkiDroid implementation material"
  echo "device production/audit ELFs: static, with no declared remote-network capability"
  echo "device symbols: no known high-level remote-network implementation"
  echo "device local transport: generic socket primitives remain for required Cobalt Unix-domain IPC"
  echo "device package: signature/canonical manifest verified against catalog and standalone ELF"
  echo "host/device font: exact bounded Cobalt Japanese bytes, SIL OFL notice, source pin, and deterministic hash present"
  echo "validation catalog: signature and sole package entry verified"
  echo "device ELFs: byte-identical to fresh audited-source builds"
  echo "host verifier/reference helper: rebuilt from audited source in a fresh target directory"
  echo "host helper artifact: finalized from the fresh audited-source build"
  echo "host helper: pinned Anki rslib/i18n/io/proto, AGPL notice, source pin, and source instructions present"
  echo "host notice sidecars: exact copies of helper notice/licence output"
  (
    cd "$target_root"
    shasum -a 256 \
      armv7-unknown-linux-musleabihf/release/kobo-flashcards \
      audit-unstripped/armv7-unknown-linux-musleabihf/release/kobo-flashcards \
      artifacts/flashcards-validation.cobalt-app \
      artifacts/flashcards.manifest.json \
      artifacts/validation-public-key.txt \
      artifacts/catalog/catalog.json \
      artifacts/catalog/catalog.sig \
      artifacts/flashcards-import.source-commit.txt \
      artifacts/flashcards-import.notice.txt \
      artifacts/flashcards-import.licenses.txt \
      host-target/release/flashcards-import
  )
} > "$audit_report"
cat "$audit_report"
cat "$audit_report"

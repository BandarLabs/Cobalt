#!/bin/sh
set -eu

if [ "$#" -ne 1 ]; then
  echo "usage: scripts/build-flashcards-validation-artifacts.sh TARGET_ROOT" >&2
  exit 2
fi

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
if [ -n "$(git -C "$repo" status --porcelain --untracked-files=normal)" ]; then
  echo "artifact builds require a clean committed source tree" >&2
  exit 1
fi
source_commit=$(git -C "$repo" rev-parse HEAD)

find_arm_tool() {
  for candidate in "$@"; do
    candidate_path=$(command -v "$candidate" 2>/dev/null || true)
    if [ -n "$candidate_path" ] &&
      "$candidate_path" --version >/dev/null 2>&1; then
      printf '%s\n' "$candidate_path"
      return 0
    fi
  done
  return 1
}

find_rust_lld() {
  rust_sysroot=$(rustc --print sysroot) || return 1
  for candidate in "$rust_sysroot"/lib/rustlib/*/bin/rust-lld; do
    if [ -x "$candidate" ]; then
      printf '%s\n' "$candidate"
      return 0
    fi
  done
  return 1
}

case $1 in
  /*) target_root=$1 ;;
  *) target_root=$(pwd)/$1 ;;
esac
target_root=$(python3 - "$target_root" "$repo" <<'PY'
import sys
from pathlib import Path

root = Path(sys.argv[1]).expanduser()
root.mkdir(parents=True, exist_ok=True)
root = root.resolve()
repo = Path(sys.argv[2]).resolve()
home = Path.home().resolve()
if root == Path("/") or root == home or root == repo:
    raise SystemExit("refusing dangerous validation target root")
if root in repo.parents or repo in root.parents:
    raise SystemExit("validation target root must not contain or sit inside the repository")
sentinel = root / ".cobalt-flashcards-validation-root"
entries = list(root.iterdir())
if entries and not sentinel.is_file():
    raise SystemExit("non-empty validation target root lacks the Cobalt sentinel")
expected = "Cobalt Flashcards validation root v1\n"
if sentinel.exists():
    if sentinel.is_symlink() or sentinel.read_text() != expected:
        raise SystemExit("validation target sentinel is invalid")
else:
    sentinel.write_text(expected)
print(root)
PY
)
artifacts="$target_root/artifacts"
device="$target_root/armv7-unknown-linux-musleabihf/release/kobo-flashcards"
audit_device="$target_root/audit-unstripped/armv7-unknown-linux-musleabihf/release/kobo-flashcards"
host="$target_root/host-target/release/flashcards-import"
cli="$target_root/host-tools/release/kobo"

find "$target_root" -mindepth 1 -maxdepth 1 \
  ! -name '.cobalt-flashcards-validation-root' \
  -exec rm -rf -- {} +
mkdir -p "$artifacts/catalog" "$target_root/build-tmp"
export TMPDIR="$target_root/build-tmp"
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
unset RUSTFLAGS CARGO_ENCODED_RUSTFLAGS CARGO_BUILD_RUSTFLAGS

(
  cd "$repo"
  CARGO_TARGET_DIR="$target_root" \
    cargo build --locked --release \
    --target armv7-unknown-linux-musleabihf -p kobo-flashcards
  CARGO_TARGET_DIR="$target_root/audit-unstripped" \
  CARGO_PROFILE_RELEASE_STRIP=none \
    cargo build --locked --release \
    --target armv7-unknown-linux-musleabihf -p kobo-flashcards
  COBALT_SOURCE_COMMIT="$source_commit" \
  CARGO_TARGET_DIR="$target_root/host-target" \
    cargo build --locked --release -p kobo-flashcards-import
  CARGO_TARGET_DIR="$target_root/host-tools" \
    cargo build --locked --release -p kobo-cli
)

if [ -n "$(git -C "$repo" status --porcelain --untracked-files=normal)" ] ||
  [ "$source_commit" != "$(git -C "$repo" rev-parse HEAD)" ]; then
  echo "source tree changed during artifact build" >&2
  exit 1
fi

python3 - "$repo/apps/catalog.json" "$device" "$artifacts/flashcards.manifest.json" <<'PY'
import hashlib
import json
import sys
from pathlib import Path

catalog = json.loads(Path(sys.argv[1]).read_text())
app = next(app for app in catalog["apps"] if app["id"] == "flashcards")
binary = Path(sys.argv[2]).read_bytes()
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
    "binary_sha256": hashlib.sha256(binary).hexdigest(),
    "binary_bytes": len(binary),
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
text = "{" + ",".join(
    json.dumps(key, separators=(",", ":"))
    + ":"
    + json.dumps(manifest[key], ensure_ascii=False, separators=(",", ":"))
    for key in order
) + "}"
Path(sys.argv[3]).write_text(text)
PY

# Public, fixed validation material only. Production runtimes do not trust it.
seed="$artifacts/.validation-seed.hex"
trap 'rm -f "$seed" "$artifacts/.flashcards-validation-second.cobalt-app"' EXIT
printf '%064d\n' 0 | tr '0' '4' > "$seed"
chmod 600 "$seed"

"$cli" app-bundle \
  --manifest "$artifacts/flashcards.manifest.json" \
  --binary "$device" \
  --seed "$seed" \
  --out "$artifacts/flashcards-validation.cobalt-app"
"$cli" app-bundle \
  --manifest "$artifacts/flashcards.manifest.json" \
  --binary "$device" \
  --seed "$seed" \
  --out "$artifacts/.flashcards-validation-second.cobalt-app"
cmp "$artifacts/flashcards-validation.cobalt-app" \
  "$artifacts/.flashcards-validation-second.cobalt-app"
"$cli" app-key --seed "$seed" > "$artifacts/validation-public-key.txt"
if [ "$(tr -d '\n' < "$artifacts/validation-public-key.txt")" != \
  "d759793bbc13a2819a827c76adb6fba8a49aee007f49f2d0992d99b825ad2c48" ]; then
  echo "validation seed derived an unexpected public key" >&2
  exit 1
fi
"$cli" app-catalog \
  --seed "$seed" \
  --out "$artifacts/catalog/catalog.json" \
  --signature "$artifacts/catalog/catalog.sig" \
  --entry "$artifacts/flashcards-validation.cobalt-app" \
  "https://example.invalid/flashcards-validation.cobalt-app"

"$host" --notice > "$artifacts/flashcards-import.notice.txt"
"$host" --licenses > "$artifacts/flashcards-import.licenses.txt"
printf '%s\n' "$source_commit" > "$artifacts/flashcards-import.source-commit.txt"

production_copy="$artifacts/.kobo-flashcards.production"
audit_copy="$artifacts/.kobo-flashcards.unstripped"
cp "$device" "$production_copy"
cp "$audit_device" "$audit_copy"
rm -rf \
  "$target_root/armv7-unknown-linux-musleabihf" \
  "$target_root/audit-unstripped"
mkdir -p \
  "$(dirname "$device")" \
  "$(dirname "$audit_device")"
mv "$production_copy" "$device"
mv "$audit_copy" "$audit_device"

rm -f "$seed" "$artifacts/.flashcards-validation-second.cobalt-app"
trap - EXIT
rm -rf "$target_root/host-tools" "$target_root/build-tmp" "$target_root/release"
rm -f "$target_root/.rustc_info.json" "$target_root/CACHEDIR.TAG"

echo "built validation-only Flashcards artifacts under $target_root"

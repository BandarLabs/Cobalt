#!/bin/sh
set -eu

if [ "$#" -ne 1 ]; then
  echo "usage: scripts/audit-flashcards-artifacts.sh TARGET_ROOT" >&2
  exit 2
fi

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
if [ -n "$(git -C "$repo" status --porcelain --untracked-files=normal)" ]; then
  echo "artifact audits require a clean committed source tree" >&2
  exit 1
fi
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
readelf=${READELF:-armv7-unknown-linux-musleabihf-readelf}
expected_validation_key=d759793bbc13a2819a827c76adb6fba8a49aee007f49f2d0992d99b825ad2c48

for path in "$device" "$audit_device" "$package" "$manifest" "$public_key" "$catalog" "$catalog_signature" "$host" "$source_commit_file" "$host_notice_file" "$host_licenses_file"; do
  if [ ! -f "$path" ]; then
    echo "missing artifact: $path" >&2
    exit 1
  fi
done

source_commit=$(tr -d '\n' < "$source_commit_file")
if [ "$source_commit" != "$(git -C "$repo" rev-parse HEAD)" ]; then
  echo "artifact source commit does not match this checkout" >&2
  exit 1
fi

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
  export CC_armv7_unknown_linux_musleabihf="${CC_armv7_unknown_linux_musleabihf:-armv7-unknown-linux-musleabihf-gcc}"
  export AR_armv7_unknown_linux_musleabihf="${AR_armv7_unknown_linux_musleabihf:-armv7-unknown-linux-musleabihf-ar}"
  CARGO_TARGET_DIR="$fresh_device_root/production" \
    cargo build --quiet --locked --release \
    --target armv7-unknown-linux-musleabihf -p kobo-flashcards
  CARGO_TARGET_DIR="$fresh_device_root/unstripped" \
  CARGO_PROFILE_RELEASE_STRIP=none \
    cargo build --quiet --locked --release \
    --target armv7-unknown-linux-musleabihf -p kobo-flashcards
)
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

audit_tools="$target_root/audit-tools"
rm -rf "$audit_tools"
mkdir -p "$audit_tools"
(
  cd "$repo"
  TMPDIR="$target_root/build-tmp" \
  COBALT_SOURCE_COMMIT="$source_commit" \
  CARGO_TARGET_DIR="$audit_tools" \
    cargo build --quiet --locked --release -p kobo-cli -p kobo-flashcards-import
)
trusted_cli="$audit_tools/release/kobo"
trusted_host="$audit_tools/release/flashcards-import"

device_tree=$(
  cd "$repo"
  cargo tree --locked --offline -p kobo-flashcards --edges normal --prefix none
)
if printf '%s\n' "$device_tree" |
  grep -E '^(anki|anki_i18n|anki_io|anki_proto) v' >/dev/null; then
  echo "device dependency tree contains Anki packages" >&2
  exit 1
fi

host_tree=$(
  cd "$repo"
  cargo tree --locked --offline -p kobo-flashcards-import --edges normal --prefix none
)
for package_name in anki anki_i18n anki_io anki_proto; do
  if ! printf '%s\n' "$host_tree" |
    grep -E "^${package_name} v" >/dev/null; then
    echo "host dependency tree is missing $package_name" >&2
    exit 1
  fi
done

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

python3 - "$repo/apps/catalog.json" "$device" "$manifest" "$catalog" "$package" <<'PY'
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
  'cobalt-flashcards-converter-v1' \
  '"capabilities":[]'; do
  if ! strings "$package" | grep -F "$required" >/dev/null; then
    echo "device package is missing its neutral Cobalt notice/format marker" >&2
    exit 1
  fi
done

if [ -e "$repo/licenses/LICENSE-AnkiDroid.txt" ]; then
  echo "standalone AnkiDroid notice remains in the current source tree" >&2
  exit 1
fi

for required in \
  '9e32ad8849068510a82273889c21b22e1acf0949' \
  "$source_commit" \
  'GNU AFFERO GENERAL PUBLIC LICENSE' \
  'Corresponding source for the Flashcards host converter' \
  'Flashcards host helper non-Anki dependency notices'; do
  if ! strings "$host" | grep -F "$required" >/dev/null; then
    echo "host helper is missing required Anki source/licence material" >&2
    exit 1
  fi
done

if ! "$trusted_host" --licenses |
  grep -F 'Corresponding source for the Flashcards host converter' >/dev/null; then
  echo "host helper does not expose corresponding-source instructions" >&2
  exit 1
fi
if ! "$trusted_host" --licenses | grep -F "$source_commit" >/dev/null; then
  echo "host helper does not expose its exact Cobalt source commit" >&2
  exit 1
fi
notice_hash=$("$trusted_host" --notice | shasum -a 256 | awk '{print $1}')
notice_file_hash=$(shasum -a 256 "$host_notice_file" | awk '{print $1}')
if [ "$notice_hash" != "$notice_file_hash" ]; then
  echo "host notice sidecar differs from helper output" >&2
  exit 1
fi
if [ "$notice_hash" != "$("$host" --notice | shasum -a 256 | awk '{print $1}')" ]; then
  echo "submitted host helper notice output differs from the fresh reference build" >&2
  exit 1
fi
licenses_hash=$("$trusted_host" --licenses | shasum -a 256 | awk '{print $1}')
licenses_file_hash=$(shasum -a 256 "$host_licenses_file" | awk '{print $1}')
if [ "$licenses_hash" != "$licenses_file_hash" ]; then
  echo "host licence/source sidecar differs from helper output" >&2
  exit 1
fi
if [ "$licenses_hash" != "$("$host" --licenses | shasum -a 256 | awk '{print $1}')" ]; then
  echo "submitted host helper licence output differs from the fresh reference build" >&2
  exit 1
fi

echo "device dependency tree: no Anki packages"
echo "device ELF/package strings and unstripped symbols: no Anki or AnkiDroid implementation material"
echo "device production/audit ELFs: static, with no declared remote-network capability"
echo "device symbols: no known high-level remote-network implementation"
echo "device local transport: generic socket primitives remain for required Cobalt Unix-domain IPC"
echo "device package: signature/canonical manifest verified against catalog and standalone ELF"
echo "validation catalog: signature and sole package entry verified"
echo "device ELFs: byte-identical to fresh audited-source builds"
echo "host verifier/reference helper: rebuilt from audited source in a fresh target directory"
echo "submitted host helper: notice/source output matches the fresh reference build"
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

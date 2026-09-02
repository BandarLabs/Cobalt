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
host="$target_root/host-target/release/flashcards-import"
source_commit_file="$target_root/artifacts/flashcards-import.source-commit.txt"
readelf=${READELF:-armv7-unknown-linux-musleabihf-readelf}

for path in "$device" "$audit_device" "$package" "$manifest" "$host" "$source_commit_file"; do
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
for package_name in anki anki_i18n; do
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

python3 - "$package" "$device" "$manifest" <<'PY'
import hashlib
import json
import sys
from pathlib import Path

package = Path(sys.argv[1]).read_bytes()
device = Path(sys.argv[2]).read_bytes()
external_manifest = Path(sys.argv[3]).read_bytes()
header = 8 + 2 + 4 + 64
if len(package) < header or package[:8] != b"COBALTAP":
    raise SystemExit("package has an invalid header")
if int.from_bytes(package[8:10], "big") != 1:
    raise SystemExit("package has an unsupported version")
manifest_length = int.from_bytes(package[10:14], "big")
manifest_end = header + manifest_length
if manifest_end > len(package):
    raise SystemExit("package manifest is truncated")
embedded_manifest = package[header:manifest_end]
packaged_binary = package[manifest_end:]
if embedded_manifest != external_manifest:
    raise SystemExit("external and packaged manifests differ")
manifest = json.loads(embedded_manifest)
if manifest["binary_bytes"] != len(device):
    raise SystemExit("manifest binary length differs from device ELF")
if manifest["binary_sha256"] != hashlib.sha256(device).hexdigest():
    raise SystemExit("manifest binary digest differs from device ELF")
if packaged_binary != device:
    raise SystemExit("packaged binary differs from standalone device ELF")
PY

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
  'Flashcards host helper dependency notices'; do
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

echo "device dependency tree: no Anki packages"
echo "device ELF/package strings and unstripped symbols: no Anki or AnkiDroid implementation material"
echo "device production/audit ELFs: static, with no declared remote-network capability"
echo "device symbols: no known high-level remote-network implementation"
echo "device local transport: generic socket primitives remain for required Cobalt Unix-domain IPC"
echo "device package: embedded manifest and binary exactly match the standalone ELF"
echo "host helper: pinned Anki rslib/i18n, AGPL notice, source pin, and source instructions present"
(
  cd "$target_root"
  shasum -a 256 \
    armv7-unknown-linux-musleabihf/release/kobo-flashcards \
    audit-unstripped/armv7-unknown-linux-musleabihf/release/kobo-flashcards \
    artifacts/flashcards-validation.cobalt-app \
    artifacts/flashcards-import.source-commit.txt \
    host-target/release/flashcards-import
)

#!/bin/sh
set -eu
cd "$(dirname "$0")/../.."
cargo run -q -p kobo-cli -- run --sim --app lichess
python3 - <<'PY'
from pathlib import Path
import struct, zlib
raw = Path("target/kobo-sim-last.raw").read_bytes()
w, h = 1072, 1448
assert len(raw) == w * h
def chunk(kind, data): return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xffffffff)
png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 0, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(b"".join(b"\0" + raw[y*w:(y+1)*w] for y in range(h)))) + chunk(b"IEND", b"")
Path("apps/lichess/screenshots/home.png").parent.mkdir(parents=True, exist_ok=True)
Path("apps/lichess/screenshots/home.png").write_bytes(png)
PY

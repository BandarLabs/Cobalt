#!/bin/sh
set -eu
cd "$(dirname "$0")/../.."
cargo run -q -p kobo-cli -- run --sim --app needles
python3 - <<'PY'
import os
from pathlib import Path
import struct, zlib
raw = (Path(os.environ["CARGO_TARGET_DIR"]) / "kobo-sim-last.raw").read_bytes(); w, h = 1072, 1448
assert len(raw) == w*h
def c(k, d): return struct.pack(">I",len(d))+k+d+struct.pack(">I",zlib.crc32(k+d)&0xffffffff)
png=b"\x89PNG\r\n\x1a\n"+c(b"IHDR",struct.pack(">IIBBBBB",w,h,8,0,0,0,0))+c(b"IDAT",zlib.compress(b"".join(b"\0"+raw[y*w:(y+1)*w] for y in range(h))))+c(b"IEND",b"")
out=Path("apps/needles/screenshots/project.png"); out.parent.mkdir(parents=True,exist_ok=True); out.write_bytes(png)
PY

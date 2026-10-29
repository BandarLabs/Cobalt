#!/usr/bin/env python3
"""Draw a two-page score of empty staves for Music Stand's routes.

A folder of page images is pushed as one score named after the folder, and
that needs no Poppler, so a route can put a score on the stand anywhere the
simulator runs.
"""
import struct
import zlib
from pathlib import Path

OUT = Path(__file__).resolve().parent / "Minuet"


def staff_page(path, seed, width=1072, height=1448):
    rows = []
    for y in range(height):
        on_staff = (y - 120 - seed) % 160 < 80 and (y - 120 - seed) % 20 == 0
        rows.append(b"\0" + (b"\x20" if on_staff else b"\xff") * width)

    def chunk(kind, data):
        return (struct.pack(">I", len(data)) + kind + data
                + struct.pack(">I", zlib.crc32(kind + data)))

    path.write_bytes(b"\x89PNG\r\n\x1a\n"
                     + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 0, 0, 0, 0))
                     + chunk(b"IDAT", zlib.compress(b"".join(rows), 9))
                     + chunk(b"IEND", b""))


if __name__ == "__main__":
    OUT.mkdir(exist_ok=True)
    for index in range(2):
        staff_page(OUT / f"page-{index + 1}.png", seed=index)

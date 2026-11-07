#!/usr/bin/env python3
"""Write a two-page CBZ for Panels' routes.

The sample built into Panels is already on the shelf after a route adds it,
so a comic sent from the computer has to be a different one for the shelf to
show it waiting. Plain drawn pages keep the file small and need nothing but
the standard library.
"""
import struct
import zipfile
import zlib
from pathlib import Path

OUT = Path(__file__).resolve().parent / "night-shift.cbz"


def page(seed, width=600, height=900):
    rows = []
    for y in range(height):
        row = bytearray(b"\xff" * width)
        if 40 <= y < height - 40:
            for x in (40, width - 41):
                row[x] = 0
        if y in (40, height - 41) or (y - seed) % 300 == 0:
            row[40:width - 40] = b"\x00" * (width - 80)
        rows.append(b"\0" + bytes(row))

    def chunk(kind, data):
        return (struct.pack(">I", len(data)) + kind + data
                + struct.pack(">I", zlib.crc32(kind + data)))

    return (b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 0, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(b"".join(rows), 9))
            + chunk(b"IEND", b""))


if __name__ == "__main__":
    info = ('<?xml version="1.0"?><ComicInfo><Title>Night shift</Title>'
            '<Series>Night shift</Series><PageCount>2</PageCount></ComicInfo>')
    with zipfile.ZipFile(OUT, "w", zipfile.ZIP_STORED) as cbz:
        cbz.writestr(zipfile.ZipInfo("ComicInfo.xml", (2026, 1, 1, 0, 0, 0)), info)
        for index in range(2):
            cbz.writestr(zipfile.ZipInfo(f"page-{index + 1:02}.png", (2026, 1, 1, 0, 0, 0)),
                         page(100 + index * 120))

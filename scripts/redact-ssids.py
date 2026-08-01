#!/usr/bin/env python3
"""Paints out the network names in a screenshot of the settings app.

Two of the settings screens are, by their nature, a list of the names of the
networks around whoever took the picture. That is exactly right on a desk and
wrong on the front page of a public repository, where it says which street the
reader is on and what the neighbours call their routers.

Nothing else in the tree needs this. Bluetooth device names are a person's own
name for their own headphones, and are left alone deliberately; a screenshot
with a name in it is a decision for whoever is in it.

## How a name is found

Not by reading it -- there is no OCR here -- but by where it sits. Every line
this touches has the same shape: some label or icon, a clear gap, then the
name, which runs to the end of the line. So the band of ink containing the
given row is split into runs at any gap of twelve pixels or more, and the last
run is the name. That survives a name of a different length, a different
typeface and a different panel width, which a hard-coded rectangle would not.

## How it fails

Loudly. A row with no ink on it means the screen has been redesigned and the
name has moved, so the image is left exactly as it was and the exit status
says so. A redaction that silently does nothing is worse than no redaction,
because the whole point is that nobody looks at these again.

Running it twice is safe. A bar is itself ink, and it sits close enough to the
label before it that a second pass would read the two as one word and paint
out the label as well, so a line that already carries a bar is left alone.

usage: scripts/redact-ssids.py IMAGE --line Y [--line Y ...]

Y is any row inside the line to paint out. The band around it is measured, so
the number only has to land somewhere in the text rather than on its baseline.
"""

import sys
from pathlib import Path

from PIL import Image, ImageDraw

# Anything darker than this counts as ink. The panel is greyscale and the type
# is drawn near black, so this sits well clear of the anti-aliased edges.
INK = 128

# A gap this wide separates a label from a value. Comfortably wider than the
# space inside "Connected to" and narrower than the gap that follows it.
WORD_GAP = 12

# Painted a little larger than the ink, so no anti-aliased edge survives.
PAD_X = 4
PAD_Y = 3

# How many columns at the end of a line are examined to tell a bar from type.
BAR_PROBE = 16


def band_around(pixels, width, height, row):
    """The run of rows of ink containing `row`, or None if that row is blank."""
    def has_ink(y):
        return any(pixels[x, y] < INK for x in range(width))

    if not 0 <= row < height or not has_ink(row):
        return None
    top = row
    while top > 0 and has_ink(top - 1):
        top -= 1
    bottom = row
    while bottom + 1 < height and has_ink(bottom + 1):
        bottom += 1
    return top, bottom


def last_run(pixels, width, top, bottom):
    """The final run of ink across `top`..`bottom`, split at WORD_GAP."""
    columns = [
        any(pixels[x, y] < INK for y in range(top, bottom + 1)) for x in range(width)
    ]
    runs, start, blank = [], None, 0
    for x, inked in enumerate(columns):
        if inked:
            if start is None:
                start = x
            blank = 0
        elif start is not None:
            blank += 1
            if blank >= WORD_GAP:
                runs.append((start, x - blank))
                start = None
    if start is not None:
        runs.append((start, width - 1))
    return runs[-1] if runs else None


def already_barred(pixels, width, band):
    """Whether the line already ends in a solid bar rather than in type.

    A bar runs to the end of the line's ink and fills its box; the last
    letter of a name never does. Sixteen columns is wider than any stroke
    and narrower than the shortest name worth hiding.
    """
    top, bottom = band
    edge = max(
        (x for x in range(width) if any(pixels[x, y] < INK for y in range(top, bottom + 1))),
        default=None,
    )
    if edge is None or edge < BAR_PROBE:
        return False
    return all(
        pixels[x, y] < INK
        for x in range(edge - BAR_PROBE + 1, edge + 1)
        for y in range(top, bottom + 1)
    )


def redact(path, rows):
    image = Image.open(path).convert("L")
    pixels = image.load()
    width, height = image.size
    draw = ImageDraw.Draw(image)
    painted = 0
    for row in rows:
        band = band_around(pixels, width, height, row)
        if band is None:
            print(f"{path}: no ink on row {row}; the screen has moved", file=sys.stderr)
            return None
        if already_barred(pixels, width, band):
            continue
        run = last_run(pixels, width, *band)
        if run is None:
            print(f"{path}: nothing to paint on row {row}", file=sys.stderr)
            return None
        draw.rectangle(
            [run[0] - PAD_X, band[0] - PAD_Y, run[1] + PAD_X, band[1] + PAD_Y],
            fill=0,
        )
        painted += 1
    image.save(path)
    return painted


def main(argv):
    paths, rows = [], []
    argument = iter(argv)
    for flag in argument:
        if flag == "--line":
            rows.append(int(next(argument)))
        elif flag.startswith("-"):
            print(__doc__.strip(), file=sys.stderr)
            return 2
        else:
            paths.append(Path(flag))
    if not paths or not rows:
        print(__doc__.strip(), file=sys.stderr)
        return 2
    for path in paths:
        painted = redact(path, rows)
        if painted is None:
            return 1
        print(f"{path}: painted out {painted} name(s)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))

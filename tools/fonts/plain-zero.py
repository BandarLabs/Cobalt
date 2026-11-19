"""Replace Atkinson Hyperlegible's slashed zero with a plain oval zero.

The outer contour is kept; the two counters the slash splits are replaced by
one counter, the outer contour scaled about its centre so the stroke is as
thick as the slashed zero's own at its sides, top and bottom. Advance widths,
side bearings and every other glyph are untouched.

usage: python tools/fonts/plain-zero.py IN.ttf OUT.ttf

The reason, and the files it was run on, are in
licenses/SOURCE-AtkinsonHyperlegible.md.
"""
import sys
from fontTools.pens.ttGlyphPen import TTGlyphPointPen
from fontTools.ttLib import TTFont

font = TTFont(sys.argv[1])
glyf = font['glyf']
zero = glyf['zero']
coords, ends, flags = zero.getCoordinates(glyf)
coords = list(coords)
assert len(ends) == 3, 'expected the slashed zero: an outer contour and two counters'
outer = [(coords[i], flags[i] & 1) for i in range(ends[0] + 1)]
counters = coords[ends[0] + 1:]
xs = [p[0] for p, _ in outer]
ys = [p[1] for p, _ in outer]
x0, x1, y0, y1 = min(xs), max(xs), min(ys), max(ys)
side = min(p[0] for p in counters) - x0
cap = y1 - max(p[1] for p in counters)
cx, cy = (x0 + x1) / 2, (y0 + y1) / 2
kx = ((x1 - x0) - 2 * side) / (x1 - x0)
ky = ((y1 - y0) - 2 * cap) / (y1 - y0)
inner = [((round(cx + (x - cx) * kx), round(cy + (y - cy) * ky)), on) for (x, y), on in outer][::-1]


def contour(pen, points):
    pen.beginPath()
    for index, (point, on) in enumerate(points):
        if not on:
            kind = None
        else:
            before = points[index - 1][1]
            kind = 'line' if before else 'qcurve'
        pen.addPoint(point, segmentType=kind)
    pen.endPath()


pen = TTGlyphPointPen(None)
contour(pen, outer)
contour(pen, inner)
glyph = pen.glyph()
glyph.recalcBounds(glyf)
glyf['zero'] = glyph
font.save(sys.argv[2])
print(sys.argv[2], 'stroke', side, cap)

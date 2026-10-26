#!/usr/bin/env python3
"""Draw Frame's demo album: original scenes with e-ink-friendly contrast."""
import math
import random
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter

OUT = Path(__file__).resolve().parent


def gradient(draw, box, top, bottom, horizontal=False):
    x0, y0, x1, y1 = box
    steps = (x1 - x0) if horizontal else (y1 - y0)
    for i in range(steps):
        t = i / max(steps - 1, 1)
        c = tuple(int(a + (b - a) * t) for a, b in zip(top, bottom))
        if horizontal:
            draw.line([(x0 + i, y0), (x0 + i, y1)], fill=c)
        else:
            draw.line([(x0, y0 + i), (x1, y0 + i)], fill=c)


def grain(img, amount=6):
    rnd = random.Random(7)
    px = img.load()
    w, h = img.size
    for _ in range(w * h // 18):
        x, y = rnd.randrange(w), rnd.randrange(h)
        delta = rnd.randrange(-amount, amount + 1)
        px[x, y] = tuple(max(0, min(255, c + delta)) for c in px[x, y])
    return img


def hills():
    img = Image.new("RGB", (1600, 1200))
    d = ImageDraw.Draw(img)
    gradient(d, (0, 0, 1600, 1200), (248, 226, 178), (112, 128, 144))
    d.ellipse([1050, 130, 1260, 340], fill=(252, 240, 200))
    for i, (base, tone) in enumerate([(720, (96, 110, 112)), (850, (74, 88, 92)), (990, (52, 62, 68))]):
        pts = [(x, base - 90 * math.sin(x / 310 + i * 1.7) - 40 * math.sin(x / 97 + i)) for x in range(0, 1700, 40)]
        d.polygon(pts + [(1600, 1200), (0, 1200)], fill=tone)
    return grain(img)


def lighthouse():
    img = Image.new("RGB", (1500, 1100))
    d = ImageDraw.Draw(img)
    gradient(d, (0, 0, 1500, 700), (236, 222, 200), (150, 168, 178))
    gradient(d, (0, 700, 1500, 1100), (70, 96, 110), (34, 48, 60))
    for y in range(720, 1080, 46):
        d.arc([0, y, 1500, y + 60], 0, 180, fill=(210, 222, 226), width=3)
    d.polygon([(660, 700), (700, 260), (800, 260), (840, 700)], fill=(226, 224, 214))
    for y in range(320, 700, 90):
        d.polygon([(672 - (y - 260) * 0.06, y), (828 + (y - 260) * 0.06, y),
                   (840, y + 34), (660, y + 34)], fill=(178, 62, 52))
    d.rectangle([712, 200, 788, 262], fill=(58, 62, 66))
    d.polygon([(700, 200), (750, 150), (800, 200)], fill=(178, 62, 52))
    d.polygon([(788, 220), (1420, 130), (1420, 220)], fill=(252, 246, 214))
    d.ellipse([60, 640, 340, 760], fill=(44, 52, 58))
    return grain(img)


def leaf():
    img = Image.new("RGB", (900, 1350), (240, 238, 230))
    d = ImageDraw.Draw(img)
    d.polygon([(450, 90), (820, 620), (450, 1290), (80, 620)], fill=(96, 128, 92))
    d.line([(450, 110), (450, 1270)], fill=(214, 224, 200), width=10)
    for i in range(1, 9):
        y = 110 + i * 135
        spread = 60 + i * 26
        d.line([(450, y), (450 + spread, y + 120)], fill=(198, 210, 188), width=6)
        d.line([(450, y), (450 - spread, y + 120)], fill=(198, 210, 188), width=6)
    return grain(img, 4)


def skyline():
    img = Image.new("RGB", (1700, 1000), (16, 20, 34))
    d = ImageDraw.Draw(img)
    d.ellipse([1330, 90, 1440, 200], fill=(232, 228, 208))
    rnd = random.Random(11)
    x = 0
    while x < 1700:
        w = rnd.randrange(90, 190)
        h = rnd.randrange(280, 700)
        d.rectangle([x, 1000 - h, x + w, 1000], fill=(28, 34, 48))
        for wy in range(1000 - h + 24, 980, 42):
            for wx in range(x + 14, x + w - 18, 30):
                if rnd.random() < 0.42:
                    d.rectangle([wx, wy, wx + 12, wy + 16], fill=(238, 208, 130))
        x += w + rnd.randrange(6, 26)
    d.rectangle([0, 960, 1700, 1000], fill=(10, 12, 20))
    return img


def birds_on_wire():
    img = Image.new("RGB", (2000, 800), (222, 226, 228))
    d = ImageDraw.Draw(img)
    gradient(d, (0, 0, 2000, 800), (238, 240, 240), (188, 198, 204))
    for offset, weight in [(300, 5), (340, 4)]:
        pts = [(x, offset + int(46 * math.sin(x / 2000 * math.pi))) for x in range(0, 2001, 25)]
        d.line(pts, fill=(40, 44, 48), width=weight)
    rnd = random.Random(3)
    for cx in (420, 760, 1180, 1460, 1720):
        cy = 300 + int(46 * math.sin(cx / 2000 * math.pi)) - 26
        body = rnd.randrange(26, 40)
        d.ellipse([cx - body, cy - body, cx + body, cy + body], fill=(36, 40, 44))
        d.ellipse([cx + body - 12, cy - body - 16, cx + body + 16, cy - body + 12], fill=(36, 40, 44))
        d.polygon([(cx + body + 14, cy - body - 8), (cx + body + 34, cy - body - 2), (cx + body + 14, cy + 2)], fill=(36, 40, 44))
    return grain(img, 4)


def mountains():
    img = Image.new("RGB", (1200, 1200))
    d = ImageDraw.Draw(img)
    gradient(d, (0, 0, 1200, 620), (206, 216, 226), (240, 236, 224))
    for peak, tone in [([(120, 620), (480, 180), (820, 620)], (88, 100, 116)),
                       ([(520, 620), (900, 260), (1240, 620)], (64, 76, 92))]:
        d.polygon(peak, fill=tone)
    d.polygon([(400, 300), (480, 180), (560, 300), (520, 330), (480, 300), (440, 330)], fill=(246, 246, 240))
    gradient(d, (0, 620, 1200, 1200), (120, 138, 152), (58, 70, 84))
    d.polygon([(120, 620), (480, 1000), (820, 620)], fill=(104, 118, 132))
    d.polygon([(520, 620), (900, 950), (1200, 620)], fill=(82, 94, 108))
    for y in range(660, 1180, 60):
        d.arc([100, y, 1100, y + 50], 10, 170, fill=(200, 212, 220), width=3)
    return grain(img, 4)


SCENES = {
    "hills-at-dawn.jpg": hills,
    "lighthouse.jpg": lighthouse,
    "monstera-leaf.jpg": leaf,
    "city-skyline.jpg": skyline,
    "birds-on-a-wire.jpg": birds_on_wire,
    "mountain-lake.jpg": mountains,
}

if __name__ == "__main__":
    for name, draw in SCENES.items():
        img = draw()
        img = img.filter(ImageFilter.UnsharpMask(radius=2, percent=60))
        img.save(OUT / name, quality=90)
        print(name, img.size)

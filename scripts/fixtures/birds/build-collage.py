#!/usr/bin/env python3
"""Rebuild the Birds fixture collage from public-domain plates.

Downloads the Wikimedia Commons plates listed in THIRD-PARTY.md, crops each to
its content, and composites a labelled 1072x1448 collage as
scripts/fixtures/birds/current.png. Requires Pillow. Every source plate is
marked public domain on its Commons file page.
"""
import io
import json
import urllib.parse
import urllib.request
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parent
USER_AGENT = {'User-Agent': 'CobaltBirdsFixtureBuilder/1.0 (fixture rebuild)'}

PLATES = [
    ('Tawny Owl', 'File:Waldkauz strix aluco 750pix.jpg'),
    ('Eurasian Kestrel', 'File:Falco tinnunculus 1873.jpg'),
    ('Hawfinch', 'File:Coccothraustes coccothraustes 1873.jpg'),
    ('Fieldfare', 'File:Turdus pilaris 1873.jpg'),
    ('Red Crossbill', 'File:Loxia curvirostra 1873.jpg'),
    ('Lesser Whitethroat', 'File:Sylvia curruca 1869.jpg'),
    ('Canada Goose', 'File:Ornithologia Neerlandica (Branta canadensis).png'),
    ('Merlin', 'File:Falco Aesalon.tif'),
    ('Bluethroat', 'File:Bluethroat Keulemans.jpg'),
    ('Razorbill', 'File:Alca torda Keulemans.jpg'),
]

WIDTH, HEIGHT = 1072, 1448
PAPER = (244, 238, 226)
INK = (56, 48, 40)


def fetch_plate(title, width=900):
    url = ('https://commons.wikimedia.org/w/api.php?action=query&format=json&titles='
           + urllib.parse.quote(title)
           + '&prop=imageinfo&iiprop=url|extmetadata&iiurlwidth=' + str(width))
    with urllib.request.urlopen(urllib.request.Request(url, headers=USER_AGENT), timeout=30) as r:
        page = next(iter(json.load(r)['query']['pages'].values()))
    info = page['imageinfo'][0]
    license_name = info.get('extmetadata', {}).get('LicenseShortName', {}).get('value', '')
    if 'public domain' not in license_name.lower():
        raise SystemExit(f'{title}: expected a public-domain plate, found {license_name!r}')
    thumb = info.get('thumburl') or info['url']
    with urllib.request.urlopen(urllib.request.Request(thumb, headers=USER_AGENT), timeout=60) as r:
        return Image.open(io.BytesIO(r.read())).convert('RGB')


def crop_content(image, threshold=235):
    mask = image.convert('L').point(lambda p: 255 if p < threshold else 0)
    bbox = mask.getbbox()
    if bbox is None:
        return image
    x0, y0, x1, y1 = bbox
    pad = 6
    return image.crop((max(0, x0 - pad), max(0, y0 - pad),
                       min(image.width, x1 + pad), min(image.height, y1 + pad)))


def main():
    font_dir = Path('/usr/share/fonts/truetype/dejavu')
    title_font = ImageFont.truetype(str(font_dir / 'DejaVuSerif-Italic.ttf'), 44)
    note_font = ImageFont.truetype(str(font_dir / 'DejaVuSerif.ttf'), 22)
    label_font = ImageFont.truetype(str(font_dir / 'DejaVuSerif-Italic.ttf'), 30)

    canvas = Image.new('RGB', (WIDTH, HEIGHT), PAPER)
    draw = ImageDraw.Draw(canvas)
    draw.text((60, 44), 'Birds of the Field Guide', font=title_font, fill=INK)
    draw.text((60, 104), 'a collage of public-domain ornithological plates', font=note_font, fill=(110, 100, 88))
    draw.line([(60, 142), (WIDTH - 60, 142)], fill=(120, 110, 95), width=2)

    tiles = [(label, crop_content(fetch_plate(title))) for label, title in PLATES]
    cols, margin, gutter, max_tile = 3, 48, 30, 225
    colw = (WIDTH - 2 * margin - (cols - 1) * gutter) // cols
    rows = [tiles[i:i + cols] for i in range(0, len(tiles), cols)]
    y = 178
    for row in rows:
        scaled = []
        row_height = 0
        for label, image in row:
            scale = min(colw / image.width, max_tile / image.height)
            tile = image.resize((max(1, int(image.width * scale)),
                                 max(1, int(image.height * scale))), Image.LANCZOS)
            scaled.append((label, tile))
            row_height = max(row_height, tile.height)
        x = margin
        for label, tile in scaled:
            canvas.paste(tile, (x + (colw - tile.width) // 2, y + (row_height - tile.height) // 2))
            text_width = draw.textlength(label, font=label_font)
            draw.text((x + (colw - text_width) / 2, y + row_height + 6), label, font=label_font, fill=INK)
            x += colw + gutter
        y += row_height + 40 + gutter
    if y > HEIGHT:
        raise SystemExit(f'collage overflows the panel: bottom at {y} > {HEIGHT}')
    canvas.save(ROOT / 'current.png')
    print(f'wrote {ROOT / "current.png"} from {len(tiles)} public-domain plates')


if __name__ == '__main__':
    main()

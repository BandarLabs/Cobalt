#!/usr/bin/env python3
"""Build Cobalt's original, deterministic sample comic; no downloaded artwork."""
import io
from pathlib import Path
import zipfile
from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parents[2]
FONT = ROOT / 'crates/kobo-text/fonts/AtkinsonHyperlegible-Regular.ttf'
BOLD = ROOT / 'crates/kobo-text/fonts/AtkinsonHyperlegible-Bold.ttf'


def page(number, caption, stage):
    image = Image.new('L', (800, 1100), 255)
    draw = ImageDraw.Draw(image)
    draw.text((52, 35), 'A small garden', font=ImageFont.truetype(str(BOLD), 48), fill=0)
    draw.text((54, 105), caption, font=ImageFont.truetype(str(FONT), 28), fill=0)
    draw.rectangle((50, 175, 750, 910), outline=0, width=4)
    # Window, sill and clay pot are drawn directly from original geometry.
    draw.rectangle((110, 215, 690, 675), outline=48, width=4)
    draw.line((400, 215, 400, 675), fill=96, width=3)
    draw.line((110, 445, 690, 445), fill=96, width=3)
    draw.ellipse((560, 250, 620, 310), outline=0, width=3)
    draw.line((75, 790, 725, 790), fill=0, width=4)
    draw.polygon(((265, 660), (535, 660), (500, 840), (300, 840)), fill=224, outline=0, width=4)
    draw.ellipse((263, 638, 537, 682), fill=255, outline=0, width=4)
    draw.ellipse((280, 648, 520, 674), fill=96)
    if stage == 0:
        draw.ellipse((385, 647, 413, 660), fill=0)
    else:
        top = 630 - stage * 72
        draw.line((400, 656, 400, top), fill=0, width=8)
        for leaf in range(stage):
            y = 616 - leaf * 64
            draw.ellipse((320, y - 42, 400, y), fill=48)
            draw.ellipse((400, y - 72, 480, y - 30), fill=96)
    draw.text((53, 948), f'{number} / 4', font=ImageFont.truetype(str(FONT), 25), fill=0)
    if number == 1:
        draw.text((53, 1000), 'Turn the page to watch it grow.', font=ImageFont.truetype(str(FONT), 28), fill=0)
    elif number == 4:
        draw.text((53, 1000), 'A little water. A little time.', font=ImageFont.truetype(str(FONT), 28), fill=0)
    output = io.BytesIO()
    image.save(output, format='PNG', optimize=True)
    return output.getvalue()


def main():
    output = ROOT / 'apps/panels/assets/a-small-garden.cbz'
    with zipfile.ZipFile(output, 'w', compression=zipfile.ZIP_DEFLATED) as archive:
        files = [('ComicInfo.xml', b'<ComicInfo><Title>A small garden</Title><Summary>An original four-page comic for trying Panels.</Summary></ComicInfo>')]
        captions = ['One seed, beside the window.', 'The first green shoot.', 'Two more leaves.', 'Room for one more pot.']
        files += [(f'{i + 1:02}.png', page(i + 1, caption, i)) for i, caption in enumerate(captions)]
        for name, data in files:
            entry = zipfile.ZipInfo(name, date_time=(2026, 9, 8, 0, 0, 0))
            entry.compress_type = zipfile.ZIP_DEFLATED
            entry.external_attr = 0o100644 << 16
            archive.writestr(entry, data)
    print(output)


if __name__ == '__main__':
    main()

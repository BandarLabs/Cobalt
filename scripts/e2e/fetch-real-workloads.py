#!/usr/bin/env python3
"""Acquire public, attributable workloads for the companion-to-simulator journey."""
import argparse
import hashlib
import json
from pathlib import Path
import urllib.request
import zipfile

SOURCES = {
    "moon.jpg": "https://images-assets.nasa.gov/image/PIA13227/PIA13227~small.jpg",
    "zork1.z3": "https://raw.githubusercontent.com/historicalsource/zork1/97b7b3d68c075dd9af7da499c3e9690ada3471fd/COMPILED/zork1.z3",
    "comic-source.cbz": "https://dodoledev.gitlab.io/pepperandcarrot-cbz/library/Pepper%20and%20Carrot/ep01_Potion-of-Flight.cbz",
}

def fetch(url, path):
    request = urllib.request.Request(url, headers={"User-Agent": "Cobalt-e2e/1"})
    with urllib.request.urlopen(request, timeout=60) as reply:
        path.write_bytes(reply.read())

def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    out = args.out.resolve(); out.mkdir(parents=True, exist_ok=True)
    records = []
    for name, url in SOURCES.items():
        path = out / name; fetch(url, path)
        records.append({"file": name, "url": url, "sha256": digest(path), "bytes": path.stat().st_size})
    normalized = out / "comic.cbz"
    with zipfile.ZipFile(out / "comic-source.cbz") as source, zipfile.ZipFile(normalized, "w", zipfile.ZIP_DEFLATED) as target:
        for name in source.namelist():
            if name.lower().endswith((".jpg", ".jpeg", ".png")):
                target.writestr(Path(name).name, source.read(name))
    records.append({"file": normalized.name, "derived_from": "comic-source.cbz", "sha256": digest(normalized), "bytes": normalized.stat().st_size})
    notes = out / "notes"; notes.mkdir(exist_ok=True)
    (notes / "00-field-notes.md").write_text("# Field notes\n\nMoon study: compare the bright limb with the shadow line.\n\n- [ ] Sketch the terminator\n- [ ] Record the observation time\n")
    opml = out / "subscriptions.opml"
    opml.write_text('<?xml version="1.0"?><opml version="2.0"><head><title>Cobalt real feeds</title></head><body><outline text="NASA Breaking News" xmlUrl="https://www.nasa.gov/news-release/feed/" htmlUrl="https://www.nasa.gov/news/"/></body></opml>\n')
    (out / "sources.json").write_text(json.dumps(records, indent=2) + "\n")
    print(out)
if __name__ == "__main__": raise SystemExit(main())

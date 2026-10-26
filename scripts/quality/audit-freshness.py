"""Extract cache/freshness surfaces for APPQA-03 (cached content usable
with honest freshness).

Per app, collect the strings that (a) mark content as cached/kept/downloaded
and (b) say how fresh it is ('since HH:MM', 'as of', 'updated', 'stale',
'retry'). Extraction only; verdicts are human.
"""
import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
MARKERS = re.compile(
    r'"([^"]*(?:[Ss]aved|[Kk]ept|[Cc]ached|[Dd]ownloaded|stale|[Oo]ffline|'
    r'as of|updated|[Ss]ync|refreshed|fetched)[^"]{2,120})"')

out = {}
for parent in ('apps', 'examples'):
    for d in sorted((ROOT / parent).iterdir()):
        if not (d / 'cobalt-app.json').exists() or d.name == 'zotero-reader':
            continue
        hits = []
        for rs in sorted((d / 'src').rglob('*.rs')):
            src = rs.read_text()
            if '#[cfg(test)]' in src:
                src = src.split('#[cfg(test)]')[0]
            for m in MARKERS.finditer(src):
                hits.append(m.group(1))
        if hits:
            out[d.name] = sorted(set(hits))
dest = ROOT / 'docs/quality/evidence/appqa03/freshness.json'
dest.parent.mkdir(parents=True, exist_ok=True)
dest.write_text(json.dumps(out, indent=1, ensure_ascii=False) + '\n')
print(len(out), 'apps ->', dest)

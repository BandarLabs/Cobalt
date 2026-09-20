"""Extract save-flow surfaces for APPQA-04 (show unsaved state before success).

Per app, collect the strings that (a) claim a save/transfer/connection
SUCCEEDED ('Saved', 'Imported', 'Connected', 'Sent', 'Synced', 'Added',
'Published', 'Transferred', 'Exported', 'Downloaded', 'Created', 'Updated',
'Done') and (b) mark work IN PROGRESS before success ('Saving', 'Importing',
'Connecting', 'Sending', 'Syncing', 'Working', 'Transferring', 'Adding',
'Creating', 'Checking', 'Preparing', 'Waiting'). Also count persistence call
sites (storage set/save, transfer, export, publish) so apps that persist
without any in-progress state are visible. Extraction only; verdicts are human.
"""
import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
SUCCESS = re.compile(
    r'"([^"]{2,90}(?:[Ss]aved|[Ii]mported|[Cc]onnected|[Ss]ent|[Ss]ynced|'
    r'[Aa]dded|[Pp]ublished|[Tt]ransferred|[Ee]xported|[Dd]ownloaded|'
    r'[Cc]reated|[Uu]pdated|on (?:the|your) reader|is ready)[^"]{0,90})"')
PENDING = re.compile(
    r'"([^"]{2,90}(?:[Ss]aving|[Ii]mporting|[Cc]onnecting|[Ss]ending|'
    r'[Ss]yncing|[Ww]orking|[Tt]ransferring|[Aa]dding|[Cc]reating|'
    r'[Cc]hecking|[Pp]reparing|[Ww]aiting|[Dd]ownloading|[Ee]xporting|'
    r'[Pp]ublishing|almost there|hold on|one moment|in progress)[^"]{0,90})"')
PERSIST = re.compile(
    r'(?:storage\.(?:set|save|put)|persist|save_to|transfer|export_|publish|'
    r'send_request|upload)')

out = {}
for parent in ('apps', 'examples'):
    for d in sorted((ROOT / parent).iterdir()):
        if not (d / 'cobalt-app.json').exists() or d.name == 'zotero-reader':
            continue
        succ, pend, pers = set(), set(), 0
        for rs in sorted((d / 'src').rglob('*.rs')):
            src = rs.read_text()
            if '#[cfg(test)]' in src:
                src = src.split('#[cfg(test)]')[0]
            succ.update(m.group(1) for m in SUCCESS.finditer(src))
            pend.update(m.group(1) for m in PENDING.finditer(src))
            pers += len(PERSIST.findall(src))
        if succ or pend or pers:
            out[d.name] = {
                'success_strings': sorted(succ),
                'pending_strings': sorted(pend),
                'persist_callsites': pers,
            }
dest = ROOT / 'docs/quality/evidence/appqa04/unsaved.json'
dest.parent.mkdir(parents=True, exist_ok=True)
dest.write_text(json.dumps(out, indent=1, ensure_ascii=False) + '\n')
print(len(out), 'apps ->', dest)

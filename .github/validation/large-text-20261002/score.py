import hashlib, io, json, re, subprocess, urllib.request, zipfile
from pathlib import Path
SCORE_URL = 'https://www.mutopiaproject.org/ftp/BachJS/BWV1007/bwv1007/bwv1007-a4-pdfs.zip'
SCORE_SHA256 = 'c82a7977944dfed870a1c14736ae502c83e82758b4350a984df253aed94cddc3'
SCORE_TITLE = 'Cello Suite No. 1 - Prelude.pdf'

def music_score(root, out, download):
    # No PDF is committed. The provenance below is committed at UNION_SHA:
    # docs/quality/evidence/musicstand-companion-real-score/README.md
    if not download:
        return None, 'No committed PDF score exists; optional provenance-pinned download was not enabled.'
    try:
        provenance = (root / 'docs/quality/evidence/musicstand-companion-real-score/README.md').read_text()
        if SCORE_URL not in provenance or SCORE_SHA256 not in provenance:
            raise RuntimeError('Committed provenance no longer matches the pinned score identity')
        with urllib.request.urlopen(SCORE_URL, timeout=60) as response:
            archive = response.read(32 * 1024 * 1024 + 1)
        if len(archive) > 32 * 1024 * 1024:
            raise RuntimeError('Score archive exceeds 32 MiB bound')
        matches = []
        with zipfile.ZipFile(io.BytesIO(archive)) as bundle:
            for entry in bundle.infolist():
                if entry.filename.lower().endswith('.pdf') and entry.file_size <= 16 * 1024 * 1024:
                    data = bundle.read(entry)
                    if hashlib.sha256(data).hexdigest() == SCORE_SHA256:
                        matches.append(data)
        if len(matches) != 1 or not matches[0].startswith(b'%PDF-'):
            raise RuntimeError('Source archive did not contain exactly the recorded genuine PDF')
        path = out / SCORE_TITLE
        path.write_bytes(matches[0])
        info = subprocess.check_output(['pdfinfo', str(path)], text=True, timeout=30)
        if not re.search(r'^Pages:\s+6\s*$', info, re.MULTILINE):
            raise RuntimeError('Recorded score no longer reports six pages')
        print(json.dumps(dict(musicstand_fixture='genuine source PDF', source=SCORE_URL,
                              sha256=SCORE_SHA256, pages=6, renamed_for_existing_test=SCORE_TITLE)), flush=True)
        return path, None
    except Exception as error:
        return None, 'Genuine score fixture unavailable: ' + str(error)


if __name__ == "__main__":
    import sys
    out=Path(sys.argv[1]); out.mkdir(parents=True, exist_ok=True)
    score,error=music_score(Path.cwd(),out,True)
    if error: raise SystemExit(error)

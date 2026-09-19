#!/usr/bin/env python3
"""Every committed journey under the supported profiles, scales and orientation (APPQA-11).

Nine device profiles collapse to three distinct panel geometries, so the
portrait matrix is 45 apps x 3 geometries x 3 text scales with each app's own
committed drive route and seed. Landscape is a boot smoke per app at the
default scale: rotate, render the first screen, shot. Results accumulate in
matrix.json so the run resumes where it stopped.
"""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent.parent
EVIDENCE = ROOT / 'docs/quality/evidence/appqa11'

GEOMETRIES = {
    '1072x1448': 'clara-bw-391',
    '1264x1680': 'libra-2-388',
    '1404x1872': 'elipsa-2e-389',
}
SCALES = ['default', 'large', 'extra-large']

APPS = sorted(
    d.name for group in ('apps', 'examples') for d in (ROOT / group).iterdir()
    if d.is_dir() and (d / 'cobalt-app.json').is_file()
    and any((d / name).is_file() for name in ('drive.kobo', 'drive.txt'))
)


def run_one(app, geometry, scale, orientation='portrait'):
    out = EVIDENCE / orientation / geometry / scale / app
    shutil.rmtree(out, ignore_errors=True)
    out.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ, KOBO_SIM_PROFILE=GEOMETRIES[geometry])
    if scale != 'default':
        env['KOBO_TEXT_SCALE'] = scale
    cmd = ['python3', str(ROOT / 'scripts/check-apps-sim.py'), app, '--out', str(out)]
    if orientation == 'landscape':
        directory = next(ROOT / group / app for group in ('apps', 'examples')
                         if (ROOT / group / app).is_dir())
        committed = next(directory / name for name in ('drive.kobo', 'drive.txt')
                         if (directory / name).is_file())
        route = Path(f'/tmp/appqa11-{app}-landscape.kobo')
        route.write_text('device orientation landscape\n' + committed.read_text())
        cmd += ['--route', str(route)]
    subprocess.run(cmd, cwd=ROOT, env=env, capture_output=True, text=True)
    report = out / 'results.json'
    if not report.is_file():
        return {'status': 'fail', 'error': 'no report'}
    return json.loads(report.read_text())['results'][0]


def main():
    only = sys.argv[1:]
    path = EVIDENCE / 'matrix.json'
    matrix = json.loads(path.read_text()) if path.is_file() else {}
    for app in APPS:
        if only and app not in only:
            continue
        entry = matrix.setdefault(app, {})
        for geometry in GEOMETRIES:
            for scale in SCALES:
                key = f'{geometry}/{scale}'
                if entry.get(key, {}).get('status') == 'pass':
                    continue
                result = run_one(app, geometry, scale)
                entry[key] = result
                path.write_text(json.dumps(matrix, indent=1) + '\n')
                print(app, key, result['status'], flush=True)
        key = 'landscape'
        if entry.get(key, {}).get('status') != 'pass':
            result = run_one(app, '1072x1448', 'default', 'landscape')
            entry[key] = result
            path.write_text(json.dumps(matrix, indent=1) + '\n')
            print(app, key, result['status'], flush=True)


if __name__ == '__main__':
    main()

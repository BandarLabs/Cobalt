#!/usr/bin/env python3
"""Exercise install/sample/own-data/task/offline-reopen for each catalog app.

Three simulator runs per app at the caller's profile and text scale:

  journey  fresh simulator, default seed, the app's committed route
           (install + sample content + own data + the app's task)
  reopen   a second simulator launched from the first run's kept state,
           expecting the app's home marker (state survives a restart)
  offline  the same, under `scenario offline` (state is usable without
           a network)

Writes docs/quality/evidence/appqa07/lifecycles.json; per-run reports and
logs land under docs/quality/evidence/appqa07/runs/<app>/<stage>/.
"""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent.parent
EVIDENCE = ROOT / 'docs/quality/evidence/appqa07'
STATE = Path('/tmp/appqa07-state')
ROUTES = Path('/tmp/appqa07-routes')


def catalog():
    out = subprocess.check_output([
        'node', '--input-type=module', '-e',
        'import { collectRegistry } from "./tools/app-registry.mjs"; '
        'console.log(JSON.stringify(collectRegistry()));'
    ], cwd=ROOT, text=True)
    return {app['id']: app for app in json.loads(out)['apps']}


def home_marker(app):
    """The first positive expectation of the committed route."""
    route = ROOT / 'apps' / app / 'drive.kobo'
    if route.is_file():
        for line in route.read_text().splitlines():
            if line.startswith('expect '):
                return line[len('expect '):].strip()
    return None


def route_verbs(app):
    route = ROOT / 'apps' / app / 'drive.kobo'
    if not route.is_file():
        return []
    return sorted({line.split(' ', 1)[0] for line in route.read_text().splitlines()
                   if line and not line.startswith('#')})


def run(app, stage, extra, route_text=None):
    out = EVIDENCE / 'runs' / app / stage
    shutil.rmtree(out, ignore_errors=True)
    out.mkdir(parents=True, exist_ok=True)
    cmd = ['python3', str(ROOT / 'scripts/check-apps-sim.py'), app,
           '--out', str(out)] + extra
    if route_text is not None:
        ROUTES.mkdir(parents=True, exist_ok=True)
        route = ROUTES / f'{app}-{stage}.kobo'
        route.write_text(route_text)
        cmd += ['--route', str(route)]
    subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True)
    report = out / 'results.json'
    if not report.is_file():
        return {'app': app, 'launched': False, 'status': 'fail', 'error': 'no report'}
    return json.loads(report.read_text())['results'][0]


def main():
    apps = sys.argv[1:] or sorted(catalog())
    lifecycles_path = EVIDENCE / 'lifecycles.json'
    lifecycles = json.loads(lifecycles_path.read_text()) if lifecycles_path.is_file() else {}
    for app in apps:
        entry = lifecycles.get(app, {})
        if entry.get('journey', {}).get('status') == 'pass' \
                and entry.get('reopen', {}).get('status') == 'pass' \
                and entry.get('offline', {}).get('status') == 'pass':
            continue
        marker = home_marker(app)
        state = STATE / app
        shutil.rmtree(state, ignore_errors=True)
        journey = run(app, 'journey', ['--state-dir', str(state)])
        own_files = sorted(str(path.relative_to(state))
                           for path in state.rglob('*') if path.is_file()) if state.is_dir() else []
        reopen = {'status': 'skip', 'reason': 'no home marker in committed route'}
        offline = {'status': 'skip', 'reason': 'no home marker in committed route'}
        if marker:
            reopen = run(app, 'reopen',
                         ['--seed-root', str(state), '--bare'],
                         route_text=f'expect {marker}\nclean\nshot {app}-reopen\n')
            offline = run(app, 'offline',
                          ['--seed-root', str(state), '--bare'],
                          route_text=f'scenario offline\nexpect {marker}\nclean\nshot {app}-offline\nscenario normal\n')
        lifecycles[app] = {
            'marker': marker,
            'route_verbs': route_verbs(app),
            'journey': journey,
            'state_files': own_files,
            'reopen': reopen,
            'offline': offline,
        }
        lifecycles_path.write_text(json.dumps(lifecycles, indent=1) + '\n')
        print(app, journey['status'], reopen['status'], offline['status'], flush=True)
    failed = [app for app, entry in lifecycles.items()
              if 'fail' in (entry.get('journey', {}).get('status'),
                            entry.get('reopen', {}).get('status'),
                            entry.get('offline', {}).get('status'))]
    print('failures:', failed if failed else 'none')


if __name__ == '__main__':
    main()

#!/usr/bin/env python3
"""Interrupt downloads and storage mid-flow (APPQA-10).

Each case boots an app, switches the simulator scenario mid-session, then
drives the flow that must fail gracefully: a refused save, a dead host, a
full store. Routes avoid wait-for after the switch; the dump and shot are
the evidence. The sim has no delayed-transfer scenario, so progress UI is
covered by the journey evidence instead.
"""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent.parent
EVIDENCE = ROOT / 'docs/quality/evidence/appqa10'
STATE = Path('/tmp/appqa10-state')


def state_dir(name):
    return STATE / name


def seed_rss(root):
    store = root / 'cobalt-sim-state' / 'rss'
    store.mkdir(parents=True)
    store.joinpath('feeds').write_text(
        'https://feeds.example.com/f0.xml\tInterrompu Weekly\tExample Site 0\n')


def seed_panels(root):
    store = root / 'cobalt-sim-state' / 'panels'
    store.mkdir(parents=True)
    record = {'schema': 'panels.library', 'version': 1, 'payload': [
        {'key': 'comic-000', 'title': 'Interrupted', 'pages': '12', 'rtl': False}]}
    store.joinpath('library').write_text(json.dumps(record, separators=(',', ':')))


def seed_parser(root):
    shelf = root / 'cobalt-sim-data' / 'parser'
    shelf.mkdir(parents=True)
    shutil.copyfile(ROOT / 'apps/parser/fixtures/zork1.z3', shelf / 'story-zork1.z3')


def seed_none(root):
    root.mkdir(parents=True, exist_ok=True)


CASES = {
    'rss-host-down': dict(
        app='rss', seed=seed_rss,
        route=('wait-for Feeds\nexpect Interrompu Weekly\nscenario host-down\n'
               'tap Interrompu Weekly\nwait 4000\ndump\nshot rss-host-down\n')),
    'panels-storage-full': dict(
        app='panels', seed=seed_panels,
        route=('wait-for Panels\nexpect Interrupted\nscenario storage-full\n'
               'tap Interrupted\nwait 4000\ndump\nshot panels-storage-full\n')),
    'parser-storage-full': dict(
        app='parser', seed=seed_parser,
        route=('wait-for Interactive fiction\ntap Zork1\nwait-for small mailbox\n'
               'scenario storage-full\ntap Keyboard\ntype save\ntap Run\n'
               'wait 3000\ndump\nshot parser-storage-full\n')),
    'todo-storage-full': dict(
        app='todo', seed=seed_none,
        route=('wait-for Todo\nscenario storage-full\ntap Add a task\n'
               'type water the fern\ntap Add\nwait 2000\ndump\nshot todo-storage-full\n')),
}


def run_case(name):
    case = CASES[name]
    out = EVIDENCE / name
    shutil.rmtree(out, ignore_errors=True)
    out.mkdir(parents=True, exist_ok=True)
    shutil.rmtree(state_dir(name), ignore_errors=True)
    case['seed'](state_dir(name))
    route_path = Path(f'/tmp/appqa10-{name}.kobo')
    route_path.write_text(case['route'])
    cmd = ['python3', str(ROOT / 'scripts/check-apps-sim.py'), case['app'],
           '--seed-root', str(state_dir(name)), '--bare',
           '--route', str(route_path), '--out', str(out)]
    env = dict(os.environ)
    subprocess.run(cmd, cwd=ROOT, env=env, capture_output=True, text=True)
    report = out / 'results.json'
    if not report.is_file():
        return {'status': 'fail', 'error': 'no report'}
    return json.loads(report.read_text())['results'][0]


def main():
    names = sys.argv[1:] or list(CASES)
    path = EVIDENCE / 'cases.json'
    verdicts = json.loads(path.read_text()) if path.is_file() else {}
    for name in names:
        result = run_case(name)
        verdicts[name] = result
        path.write_text(json.dumps(verdicts, indent=1) + '\n')
        print(name, result['status'], flush=True)


if __name__ == '__main__':
    main()

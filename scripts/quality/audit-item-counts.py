#!/usr/bin/env python3
"""Exercise 0, 1, 20 and 100 items with long Unicode titles per list app.

Each seeder writes the app's store or shelf directly into a kept simulator
state (or drives the companion CLI where one exists for the sim), then a
one-shot route renders the list screen. Output lands under
docs/quality/evidence/appqa08/<app>/<n>/ with shots; lifecycles accumulate in
counts.json for the verdicts.
"""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent.parent
EVIDENCE = ROOT / 'docs/quality/evidence/appqa08'
STATE = Path('/tmp/appqa08-state')
KOBO = ROOT / 'target/debug/kobo'

COUNTS = [0, 1, 20, 100]
# rss holds at most 40 feeds (MAX_FEEDS); panels holds 64 books (MAX_BOOKS).
# Their extra count probes the over-cap path rather than a full load.
APP_COUNTS = {
    'rss': [0, 1, 20, 40, 100],
    'flashcards': [0, 1],
    'panels': [0, 1, 20, 64, 100],
}


def title(i):
    # Accented Latin stays inside the simulator's drawable set; CJK and emoji
    # make the installed face panic, so they cannot seed a sim run.
    return f"Très lông titre {i:02} — accénts ñüé and a long trail of words that keeps going well past sixty"


def state_dir(app):
    return STATE / app


def data(app):
    return state_dir(app) / 'cobalt-sim-data' / app


def store(app):
    return state_dir(app) / 'cobalt-sim-state' / app


def seed_vault(n, env, log):
    fixture = STATE / 'vault-fixture'
    shutil.rmtree(fixture, ignore_errors=True)
    fixture.mkdir(parents=True)
    for i in range(n):
        (fixture / f'{title(i)}.md').write_text(
            f'# {title(i)}\n\nA note body with a [[Welcome]] link.\n')
    if n == 0:
        fixture.mkdir(exist_ok=True)
    subprocess.run([str(KOBO), 'vault', 'init', '--sim'], cwd=ROOT, env=env,
                   stdout=log, stderr=log, timeout=30, check=True)
    if n:
        subprocess.run([str(KOBO), 'vault', 'push', str(fixture), '--sim'], cwd=ROOT,
                       env=env, stdout=log, stderr=log, timeout=60, check=True)


def seed_todo(n, env, log):
    store('todo').mkdir(parents=True, exist_ok=True)
    store('todo').joinpath('items').write_text(
        ''.join(f'- {title(i)}\n' for i in range(n)))


def seed_habits(n, env, log):
    store('habits').mkdir(parents=True, exist_ok=True)
    # Rows: done-count, key, name, then two empty fields (see the fixture row
    # "0\td\ttea"): one habit per line, untouched.
    store('habits').joinpath('habits-v1').write_text(
        ''.join(f'0\th{i}\t{title(i)}\t\t\n' for i in range(n)))


def seed_chat(n, env, log):
    store('chat').mkdir(parents=True, exist_ok=True)
    turns = [
        {'role': 'user' if i % 2 == 0 else 'assistant', 'text': title(i)}
        for i in range(n)
    ]
    store('chat').joinpath('conversation').write_text(
        json.dumps({'v': '1', 'turns': turns}))


def seed_needles(n, env, log):
    shelf = data('needles')
    shutil.rmtree(shelf, ignore_errors=True)
    shelf.mkdir(parents=True, exist_ok=True)
    pattern = (ROOT / 'apps/needles/fixtures/pattern.md').read_text()
    for i in range(n):
        shelf.joinpath(f'{title(i)}.md').write_text(pattern)
    if n:
        shutil.copyfile(ROOT / 'apps/needles/fixtures/chart-main.png',
                        shelf / 'chart-main.png')


def seed_fieldbook(n, env, log):
    shelf = data('fieldbook')
    shutil.rmtree(shelf, ignore_errors=True)
    shelf.mkdir(parents=True, exist_ok=True)
    packs = [
        {
            'id': f'pack-{i}',
            'title': title(i),
            'region': 'US-CA-SF',
            'issued': '2026-09-01',
            'species': [{'code': 'AMRO', 'common': 'American Robin',
                         'scientific': 'Turdus migratorius', 'family': 'Thrushes'}],
        }
        for i in range(n)
    ]
    shelf.joinpath('packs.v1').write_text(json.dumps(
        {'format': 'fieldbook-shelf', 'version': '1', 'packs': packs}))


def seed_parser(n, env, log):
    shelf = data('parser')
    shutil.rmtree(shelf, ignore_errors=True)
    shelf.mkdir(parents=True, exist_ok=True)
    for i in range(n):
        shutil.copyfile(ROOT / 'apps/parser/fixtures/zork1.z3',
                        shelf / f'story-{title(i)}.z3')


def seed_birds(n, env, log):
    shelf = data('birds')
    shutil.rmtree(shelf, ignore_errors=True)
    shelf.mkdir(parents=True, exist_ok=True)
    if n:
        shutil.copyfile(ROOT / 'scripts/fixtures/birds/current.json', shelf / 'current.json')
        shutil.copyfile(ROOT / 'scripts/fixtures/birds/current.png', shelf / 'current.png')


def seed_rss(n, env, log):
    store('rss').mkdir(parents=True, exist_ok=True)
    store('rss').joinpath('feeds').write_text(
        ''.join(f'https://feeds.example.com/f{i}.xml\t{title(i)}\tExample Site {i}\n'
                for i in range(n)))


def seed_audiobook(n, env, log):
    shelf = data('audiobook')
    shutil.rmtree(shelf, ignore_errors=True)
    shelf.mkdir(parents=True, exist_ok=True)
    store('audiobook').mkdir(parents=True, exist_ok=True)
    lines = []
    source = STATE / 'audiobook-src' / 'the-quiet-shelf.mp3z'
    if not source.is_file():
        source = Path('/tmp/appqa07-state/audiobook/cobalt-sim-data/audiobook/the-quiet-shelf.mp3z')
    for i in range(n):
        name = f'book-{i:03}.mp3z'
        shutil.copyfile(source, shelf / name)
        lines.append(f'{name}\t{title(i)}\n')
    store('audiobook').joinpath('library').write_text(''.join(lines))


def seed_frame(n, env, log):
    subprocess.run([str(KOBO), 'frame', 'init', '--sim'], cwd=ROOT, env=env,
                   stdout=log, stderr=log, timeout=30, check=True)
    for i in range(n):
        subprocess.run([str(KOBO), 'frame', 'push', str(FRAME_PHOTO), '--sim'],
                       cwd=ROOT, env=env, stdout=log, stderr=log, timeout=30,
                       check=True)


def seed_panels(n, env, log):
    # The library index is a kobo_state record: plain JSON with a schema
    # envelope. Over 64 entries (MAX_BOOKS) decode must reject the file
    # cleanly, which is what the 100 count probes.
    store('panels').mkdir(parents=True, exist_ok=True)
    entries = [
        {'key': f'comic-{i:03}', 'title': title(i), 'pages': '12', 'rtl': False}
        for i in range(n)
    ]
    record = {'schema': 'panels.library', 'version': 1, 'payload': entries}
    store('panels').joinpath('library').write_text(
        json.dumps(record, separators=(',', ':'), ensure_ascii=False))


def seed_flashcards(n, env, log):
    if n == 0:
        return
    shelf = data('flashcards')
    shelf.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(Path('/tmp/appqa07-state/flashcards/cobalt-sim-data/flashcards/collection.cobfc'),
                    shelf / 'collection.cobfc')


FRAME_PHOTO = Path('/tmp/e2e-workloads/moon.jpg')
PANELS_COMIC = Path('/tmp/e2e-workloads/comic.cbz')

SEEDERS = {
    'vault': seed_vault,
    'todo': seed_todo,
    'habits': seed_habits,
    'chat': seed_chat,
    'needles': seed_needles,
    'fieldbook': seed_fieldbook,
    'parser': seed_parser,
    'birds': seed_birds,
    'rss': seed_rss,
    'audiobook': seed_audiobook,
    'frame': seed_frame,
    'panels': seed_panels,
    'flashcards': seed_flashcards,
}

# The route that puts each app's list on the panel.
ROUTES = {
    'vault': 'wait-idle\n',
    'todo': 'wait-idle\n',
    'habits': 'wait-idle\n',
    'chat': 'wait-idle\n',
    'needles': 'wait-idle\n',
    'fieldbook': 'wait-idle\n',
    'parser': 'wait-idle\n',
    'birds': 'wait-idle\n',
    'rss': 'wait-idle\n',
    'audiobook': 'wait-idle\n',
    'frame': 'wait-idle\n',
    'panels': 'wait-idle\n',
    'flashcards': 'wait-idle\n',
}


def run_count(app, n):
    out = EVIDENCE / app / str(n)
    shutil.rmtree(out, ignore_errors=True)
    out.mkdir(parents=True, exist_ok=True)
    shutil.rmtree(state_dir(app), ignore_errors=True)
    log_path = out / 'seed.log'
    with log_path.open('w') as log:
        env = dict(os.environ, TMPDIR=str(state_dir(app)))
        state_dir(app).mkdir(parents=True, exist_ok=True)
        try:
            SEEDERS[app](n, env, log)
        except subprocess.CalledProcessError as error:
            return {'app': app, 'count': n, 'status': 'fail', 'error': f'seed: {error}'}
    route = ROUTES[app] + f'clean\nshot {app}-{n}\n'
    route_path = Path(f'/tmp/appqa08-{app}-{n}.kobo')
    route_path.write_text(route)
    cmd = ['python3', str(ROOT / 'scripts/check-apps-sim.py'), app,
           '--seed-root', str(state_dir(app)), '--bare',
           '--route', str(route_path), '--out', str(out)]
    subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True)
    report = out / 'results.json'
    if not report.is_file():
        return {'app': app, 'count': n, 'status': 'fail', 'error': 'no report'}
    result = json.loads(report.read_text())['results'][0]
    result['count'] = n
    return result


def main():
    apps = sys.argv[1:] or sorted(SEEDERS)
    counts_path = EVIDENCE / 'counts.json'
    counts = json.loads(counts_path.read_text()) if counts_path.is_file() else {}
    for app in apps:
        entry = counts.setdefault(app, {})
        for n in APP_COUNTS.get(app, COUNTS):
            if entry.get(str(n), {}).get('status') == 'pass':
                continue
            result = run_count(app, n)
            entry[str(n)] = result
            counts_path.write_text(json.dumps(counts, indent=1) + '\n')
            print(app, n, result['status'], flush=True)


if __name__ == '__main__':
    main()

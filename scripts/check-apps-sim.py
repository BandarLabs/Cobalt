#!/usr/bin/env python3
"""Build and drive catalog apps in fresh simulators; fail if any app fails."""
import argparse
import json
import os
from pathlib import Path
import re
import shlex
import signal
import sqlite3
import contextlib
import shutil
import subprocess
import tempfile
import time
import zipfile

ROOT = Path(__file__).resolve().parent.parent


def stop_group(process):
    if process is None:
        return
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        process.wait()


def seed(app, state, kobo, env, log):
    def run(*args):
        subprocess.run([str(kobo), *args], cwd=ROOT, env=env,
                       stdout=log, stderr=log, timeout=30, check=True)
    if app == 'deck':
        home = str(Path(state) / 'deck-config')
        run('deck', 'init', '--home', home)
        run('deck', 'set', '1', '--label', 'Todo', '--launch', 'todo', '--home', home)
        run('deck', 'set', '2', '--label', 'Example', '--url', 'https://example.com', '--home', home)
        run('deck', 'push', '--sim', '--home', home)
    # Seeding goes through the owner's own commands wherever one exists, so
    # the sweep checks the companion and the app agree on what a push leaves
    # on the reader, not just that the app can read a hand-placed file.
    elif app == 'frame':
        run('frame', 'push', str(ROOT / 'apps/frame/screenshots/frame.png'), '--sim', '--fit', 'pad')
    elif app == 'needles':
        run('needles', 'push', str(ROOT / 'apps/needles/fixtures/pattern.md'), '--sim')
    elif app == 'birds':
        shelf = Path(state) / 'cobalt-sim-data' / 'birds'
        shelf.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(ROOT / 'scripts/fixtures/birds/current.json', shelf / 'current.json')
        shutil.copyfile(ROOT / 'scripts/fixtures/birds/current.png', shelf / 'current.png')
    elif app == 'fieldbook':
        run('fieldbook', 'push', str(ROOT / 'scripts/fixtures/fieldbook/packs.v1'), '--sim')
    elif app == 'vault':
        run('vault', 'push', str(ROOT / 'scripts/fixtures/vault'), '--sim')
    elif app == 'chat':
        store = Path(state) / 'cobalt-sim-state' / 'chat'
        store.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(ROOT / 'scripts/fixtures/chat/conversation', store / 'conversation')
    elif app == 'books':
        # The simulator's library folder stands in for the reader's storage.
        # A route for the empty shelf runs with --bare instead.
        library = Path(state) / 'cobalt-sim-data' / 'library'
        library.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(ROOT / 'apps/calibre-web/fixtures/river.epub', library / 'River.epub')
        seed_stock_library(library)
    elif app == 'parser':
        run('parser', 'push', str(ROOT / 'apps/parser/fixtures/lamplight.z3'), '--sim')


def write_epub(path, title, author, words):
    """The smallest EPUB a reader accepts: a stored mimetype, a container,
    a package and one chapter."""
    package = (
        '<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="3.0">'
        '<metadata xmlns:dc="http://purl.org/dc/elements/1.1/">'
        f'<dc:title>{title}</dc:title><dc:creator>{author}</dc:creator></metadata>'
        '<manifest><item id="one" href="one.xhtml" media-type="application/xhtml+xml"/></manifest>'
        '<spine><itemref idref="one"/></spine></package>')
    container = (
        '<?xml version="1.0"?><container version="1.0" '
        'xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles>'
        '<rootfile full-path="OEBPS/book.opf" media-type="application/oebps-package+xml"/>'
        '</rootfiles></container>')
    with zipfile.ZipFile(path, 'w') as book:
        book.writestr(zipfile.ZipInfo('mimetype'), 'application/epub+zip',
                      compress_type=zipfile.ZIP_STORED)
        book.writestr('META-INF/container.xml', container, compress_type=zipfile.ZIP_DEFLATED)
        book.writestr('OEBPS/book.opf', package, compress_type=zipfile.ZIP_DEFLATED)
        book.writestr('OEBPS/one.xhtml',
                      f'<html><body><h1>{title}</h1><p>{words}</p></body></html>',
                      compress_type=zipfile.ZIP_DEFLATED)


def seed_stock_library(card):
    """A stock reader's database on the simulated card, as the reader keeps one.

    Books lists Kobo Store titles from it on a shelf of their own: one whose
    file is on the card, kept where the stock reader keeps downloads, and one
    that is in the account but not downloaded.
    """
    kobo = card / '.kobo'
    kepubs = kobo / 'kepub'
    kepubs.mkdir(parents=True, exist_ok=True)
    kepub = kepubs / '5f1c7e2a-0d3b-4a8e-9c61-7b2d4e8f9a10'
    write_epub(kepub, 'The Lighthouse Keeper', 'A. Keeper',
               'The lamp was lit at dusk, as it had been every night for forty years.')
    with sqlite3.connect(kobo / 'KoboReader.sqlite') as database:
        database.execute(
            'CREATE TABLE content (ContentID TEXT, Title TEXT, MimeType TEXT, '
            'IsDownloaded TEXT, ___FileSize INTEGER, ContentType TEXT)')
        database.executemany('INSERT INTO content VALUES (?, ?, ?, ?, ?, ?)', [
            (f'file://{kepub.resolve()}', 'The Lighthouse Keeper',
             'application/x-kobo-epub+zip', 'true', kepub.stat().st_size, '6'),
            ('7a866127-8d49-4541-ac08-4a279e0f07cd', 'A Winter Crossing',
             'application/x-kobo-epub+zip', 'false', 0, '6'),
        ])


def route_directive(route):
    """How an extra route asks to be run, from a `# sweep:` first line.

    A route such as an empty first run or a conversation cut short needs a
    different starting state from the app's main route. Saying so in the
    route itself keeps the instruction next to the steps that depend on it,
    where a table in this script would drift from them.
    """
    first = route.read_text().splitlines()[:1]
    match = re.match(r'#\s*sweep:\s*(.*)$', first[0]) if first else None
    options = dict(bare=False, seed_root=None, after_main=False, skip=False)
    if match is None:
        return options
    words = shlex.split(match.group(1))
    index = 0
    while index < len(words):
        word = words[index]
        if word == '--bare':
            options['bare'] = True
        elif word == '--seed-root' and index + 1 < len(words):
            options['seed_root'] = ROOT / words[index + 1]
            index += 1
        elif word == 'after-main':
            options['after_main'] = True
        elif word == 'skip':
            options['skip'] = True
        else:
            raise ValueError(f'{route}: unknown sweep directive {word!r}')
        index += 1
    return options


def app_directory(app):
    return next((ROOT / group / app for group in ('apps', 'examples')
                 if (ROOT / group / app).is_dir()), None)


def run_routes(app, kobo, out, environment, timeout):
    """The main route, then every `drive-*.kobo` beside it as it asks."""
    results = [run_app(app, kobo, out, environment, timeout)]
    directory = app_directory(app)
    for route in sorted(directory.glob('drive-*.kobo')) if directory else []:
        label = f'{app}:{route.stem}'
        try:
            options = route_directive(route)
        except ValueError as error:
            results.append(dict(app=label, launched=False, status='fail', error=str(error)))
            continue
        if options['skip']:
            results.append(dict(app=label, launched=False, status='skipped',
                                route=str(route.relative_to(ROOT))))
            continue
        if options['after_main']:
            with tempfile.TemporaryDirectory(prefix='cb-', dir='/tmp') as state:
                run_app(app, kobo, out, environment, timeout, state_dir=state,
                        label=f'{label}-main')
                results.append(run_app(app, kobo, out, environment, timeout, bare=True,
                                       route_override=route, state_dir=state, label=label))
            continue
        results.append(run_app(app, kobo, out, environment, timeout, bare=options['bare'],
                               route_override=route, seed_root=options['seed_root'],
                               label=label))
    return results


def run_app(app, kobo, out, environment, timeout, bare=False, route_override=None,
            seed_root=None, state_dir=None, label=None):
    label = label or app
    directory = app_directory(app)
    result = dict(app=label, launched=False, status='fail')
    if directory is None:
        return dict(result, error='catalog app has no source directory')
    route = route_override or next((directory / name for name in ('drive.kobo', 'drive.txt')
                                    if (directory / name).is_file()), None)
    result['route'] = (str(route.relative_to(ROOT)) if route and route.is_relative_to(ROOT)
                       else str(route) if route else None)
    process = None
    log_path = out / (label.replace(':', '-') + '.log')
    # Short paths are required by Unix sockets, independently of --out length.
    if state_dir is not None:
        kept = Path(state_dir)
        kept.mkdir(parents=True, exist_ok=True)
        context = contextlib.nullcontext(str(kept))
    else:
        context = tempfile.TemporaryDirectory(prefix='cb-', dir='/tmp')
    with context as state, log_path.open('w') as log:
        # The simulator otherwise hands apps the computer's own UTC offset, and
        # a route that names a day would pass in one zone and fail in another.
        env = dict(environment, TMPDIR=state, KOBO_INKLING_DAY='2026-09-01',
                   KOBO_UTC_OFFSET_MINUTES=environment.get('KOBO_UTC_OFFSET_MINUTES', '0'))
        env.pop('KOBO_SIM_DATA', None)
        if seed_root is not None and Path(seed_root).is_dir():
            shutil.copytree(seed_root, state, dirs_exist_ok=True,
                            ignore=shutil.ignore_patterns('*.sock'))
        if app == 'fanshelf':
            env['FANSHELF_DEMO'] = '1'
        if app == 'backgammon':
            # Real dice come from the operating system, so a route that says
            # what was rolled needs the fixture source rather than chance.
            env['KOBO_BACKGAMMON_SEED'] = '7'
        try:
            if not bare:
                seed(app, state, kobo, env, log)
            process = subprocess.Popen([str(kobo), 'dev', '127.0.0.1:0'], cwd=directory,
                                       env=env, stdout=log, stderr=log, start_new_session=True)
            deadline = time.monotonic() + timeout
            address = None
            while time.monotonic() < deadline:
                if process.poll() is not None:
                    raise RuntimeError(f'simulator exited with {process.returncode}; see {log_path}')
                ready = re.search(r'Kobo app simulator: http://(127\.0\.0\.1:\d+)', log_path.read_text())
                if ready:
                    address = ready.group(1)
                    probe = subprocess.run([str(kobo), 'drive', '--address', address, '--step', 'dump'],
                                           env=env, capture_output=True, text=True, timeout=10)
                    if probe.returncode == 0 and '["' in probe.stdout:
                        result['launched'] = True
                        break
                time.sleep(0.2)
            if not result['launched']:
                raise RuntimeError(f'no first screen within {timeout}s; see {log_path}')
            command = [str(kobo), 'drive', '--address', address, '--ideal', '--shots',
                       str(out / (label.replace(':', '-') + '-shots'))]
            command += ['--script', str(route)] if route else ['--step', 'dump']
            driven = subprocess.run(command, cwd=ROOT, env=env, stdout=log, stderr=log, timeout=timeout)
            result['exit_code'] = driven.returncode
            result['status'] = 'pass' if driven.returncode == 0 else 'fail'
        except (OSError, RuntimeError, subprocess.SubprocessError) as error:
            result['error'] = str(error)
        finally:
            stop_group(process)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('apps', nargs='*', help='catalog IDs; default: all apps')
    parser.add_argument('--out', type=Path, default=ROOT / 'target/sim-check')
    parser.add_argument('--timeout', type=int, default=300)
    # scripts/e2e/run.py seeds through the real companion commands and then
    # asks for no default seed under this longer name.
    parser.add_argument('--bare', '--skip-default-seed', action='store_true',
                        help='skip seeding, for first-run scenarios')
    parser.add_argument('--seed-root', type=Path,
                        help='copy this prepared simulator state into the fresh simulator')
    parser.add_argument('--state-dir', type=Path,
                        help='keep simulator state here instead of deleting it after the run')
    parser.add_argument('--route', type=Path,
                        help='drive script to run instead of the app default')
    parser.add_argument('--all-routes', action='store_true',
                        help='also run every drive-*.kobo, as its "# sweep:" first line asks')
    args = parser.parse_args()
    if args.route is not None:
        args.route = args.route.resolve()
    if args.timeout < 1:
        parser.error('--timeout must be positive')
    registry = json.loads(subprocess.check_output([
        'node', '--input-type=module', '-e',
        'import { collectRegistry } from "./tools/app-registry.mjs"; console.log(JSON.stringify(collectRegistry()));'
    ], cwd=ROOT, text=True))
    catalog = [app['id'] for app in registry['apps']]
    # Books, Store, Launcher, Settings and the other built-ins ship with the
    # runtime rather than the Store, so the registry does not list them. They
    # are checked when named, which is how their committed routes get run.
    built_ins = sorted(path.name for path in (ROOT / 'examples').iterdir()
                       if (path / 'Cargo.toml').is_file() and path.name not in catalog)
    unknown = set(args.apps) - set(catalog) - set(built_ins)
    if unknown:
        parser.error('unknown apps: ' + ', '.join(sorted(unknown)))
    apps = list(dict.fromkeys(args.apps or catalog))
    out = args.out.resolve()
    if out in ROOT.parents:
        parser.error('--out must not contain the repository')
    tracked = subprocess.check_output(['git', 'ls-files', '--', str(out)], cwd=ROOT, text=True) if out.is_relative_to(ROOT) else ''
    if tracked.strip():
        parser.error('--out must not contain tracked files')
    out.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ)
    target = Path(env.get('CARGO_TARGET_DIR', str(ROOT / 'target'))).resolve()
    env['CARGO_TARGET_DIR'] = str(target)
    subprocess.run(['cargo', 'build', '--locked', '-p', 'kobo-cli'], cwd=ROOT, env=env, check=True)
    kobo = target / 'debug/kobo'
    report = {
        'source_sha': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
        'dirty': bool(subprocess.check_output(['git', 'status', '--porcelain'], cwd=ROOT, text=True).strip()),
        'results': [],
    }
    if args.all_routes and (args.route or args.bare or args.seed_root or args.state_dir):
        parser.error('--all-routes chooses each route\'s state itself')
    for app in apps:
        if args.all_routes:
            results = run_routes(app, kobo, out, env, args.timeout)
        else:
            results = [run_app(app, kobo, out, env, args.timeout,
                               bare=args.bare, route_override=args.route,
                               seed_root=args.seed_root, state_dir=args.state_dir)]
        for result in results:
            report['results'].append(result)
            (out / 'results.json').write_text(json.dumps(report, indent=2) + '\n')
            print(json.dumps(result), flush=True)
    runs = [r for r in report['results'] if r['status'] != 'skipped']
    failed = [r['app'] for r in runs if r['status'] != 'pass']
    routes = sum(r.get('route') is not None for r in runs)
    print(f"{len(runs) - len(failed)}/{len(runs)} runs passed across {len(apps)} apps; "
          f"{routes} committed routes; report: {out / 'results.json'}")
    return 1 if failed else 0


if __name__ == '__main__':
    def interrupted(_signum, _frame):
        raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, interrupted)
    raise SystemExit(main())

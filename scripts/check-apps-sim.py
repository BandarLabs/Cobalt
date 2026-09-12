#!/usr/bin/env python3
"""Build and drive catalog apps in fresh simulators; fail if any app fails."""
import argparse
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import tempfile
import time

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
    elif app == 'frame':
        run('frame', 'init', '--sim')
        run('frame', 'push', str(ROOT / 'apps/frame/screenshots/frame.png'), '--sim', '--fit', 'pad')
    elif app == 'vault':
        run('vault', 'init', '--sim')
        run('vault', 'push', str(ROOT / 'scripts/fixtures/vault'), '--sim')


def run_app(app, kobo, out, environment, timeout):
    directory = next((ROOT / group / app for group in ('apps', 'examples')
                      if (ROOT / group / app).is_dir()), None)
    result = dict(app=app, launched=False, status='fail')
    if directory is None:
        return dict(result, error='catalog app has no source directory')
    route = next((directory / name for name in ('drive.kobo', 'drive.txt')
                  if (directory / name).is_file()), None)
    result['route'] = str(route.relative_to(ROOT)) if route else None
    process = None
    log_path = out / (app + '.log')
    # Short paths are required by Unix sockets, independently of --out length.
    with tempfile.TemporaryDirectory(prefix='cb-', dir='/tmp') as state, log_path.open('w') as log:
        env = dict(environment, TMPDIR=state, KOBO_INKLING_DAY='2026-09-01')
        if app == 'fanshelf':
            env['FANSHELF_DEMO'] = '1'
        if app == 'backgammon':
            # Real dice come from the operating system, so a route that says
            # what was rolled needs the fixture source rather than chance.
            env['KOBO_BACKGAMMON_SEED'] = '7'
        try:
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
            command = [str(kobo), 'drive', '--address', address, '--ideal', '--shots', str(out / (app + '-shots'))]
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
    args = parser.parse_args()
    if args.timeout < 1:
        parser.error('--timeout must be positive')
    registry = json.loads(subprocess.check_output([
        'node', '--input-type=module', '-e',
        'import { collectRegistry } from "./tools/app-registry.mjs"; console.log(JSON.stringify(collectRegistry()));'
    ], cwd=ROOT, text=True))
    catalog = [app['id'] for app in registry['apps']]
    if set(args.apps) - set(catalog):
        parser.error('unknown apps: ' + ', '.join(sorted(set(args.apps) - set(catalog))))
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
    for app in apps:
        result = run_app(app, kobo, out, env, args.timeout)
        report['results'].append(result)
        (out / 'results.json').write_text(json.dumps(report, indent=2) + '\n')
        print(json.dumps(result), flush=True)
    failed = [r['app'] for r in report['results'] if r['status'] != 'pass']
    routes = sum(r.get('route') is not None for r in report['results'])
    print(f"{len(apps) - len(failed)}/{len(apps)} apps passed; {routes} committed routes; report: {out / 'results.json'}")
    return 1 if failed else 0


if __name__ == '__main__':
    def interrupted(_signum, _frame):
        raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, interrupted)
    raise SystemExit(main())

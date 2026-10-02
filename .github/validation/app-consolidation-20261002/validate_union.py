#!/usr/bin/env python3
"""Validate the pinned combined production checkout without editing or uploading it."""
import argparse
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import time
import tomllib
import urllib.request
import zipfile

sys.dont_write_bytecode = True
UNION_SHA = '218b5172262767be2fd25265d2b35f7f9337b8c6'
APPS = ('launcher sidekick readlater chat pubquiz parser backgammon settings parlor '
        'homepanel kitchencard deck post habits vault syncthing zotero-reader '
        'rss-miniflux gutenbird rss hn inkling arxiv needles verses fanshelf '
        'musicstand grimoire birds audiobook fieldbook').split()
PROFILES = ('clara-bw-391 clara-bw-395 clara-hd-376 clara-colour-393 elipsa-2e-389 '
            'libra-2-388 libra-colour-390 libra-colour-390-4.46.23836 libra-h2o-384').split()
SPECIALIZED = {
    'post': 'check-post-sim.py', 'deck': 'check-deck-sim.py',
    'inkling': 'check-inkling-sim.py', 'musicstand': 'check-musicstand-shelf-sim.py',
    'audiobook': 'check-audiobook-sim.py', 'fieldbook': 'check-fieldbook-sim.py',
}
SCORE_URL = 'https://www.mutopiaproject.org/ftp/BachJS/BWV1007/bwv1007/bwv1007-a4-pdfs.zip'
SCORE_SHA256 = 'c82a7977944dfed870a1c14736ae502c83e82758b4350a984df253aed94cddc3'
SCORE_TITLE = 'Cello Suite No. 1 - Prelude.pdf'


def git(root, *args):
    return subprocess.check_output(['git', '-C', str(root), *args], text=True).strip()


def assert_production_tree(root):
    actual = git(root, 'rev-parse', 'HEAD')
    if actual != UNION_SHA:
        raise RuntimeError(f'Expected exact union {UNION_SHA}, found {actual}')
    status = git(root, 'status', '--porcelain', '--untracked-files=all')
    if status:
        raise RuntimeError(f'Tested checkout was modified: {status}')


def directory(root, app):
    return next((root / group / app for group in ('apps', 'examples')
                 if (root / group / app).is_dir()), None)


def load_helper(root):
    spec = importlib.util.spec_from_file_location('committed_sim_runner', root / 'scripts/check-apps-sim.py')
    helper = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(helper)
    return helper


def write_summary(report, path):
    if not path:
        return
    lines = ['## Consolidated app simulator validation', '',
             f'Exact production commit: `{UNION_SHA}`',
             'No screenshots or artifacts are uploaded by this workflow.', '',
             '| App | Profile | Text | Coverage | Result |', '| --- | --- | --- | --- | --- |']
    for r in report['results']:
        lines.append('| ' + ' | '.join(str(r.get(k, '')).replace('|', '/') for k in
                     ('app', 'profile', 'scale', 'coverage', 'status')) + ' |')
    counts = {s: sum(r['status'] == s for r in report['results']) for s in ('pass', 'fail', 'unavailable')}
    lines += ['', f"{counts['pass']} passed; {counts['fail']} failed; {counts['unavailable']} unavailable."]
    for r in report['results']:
        if r.get('error'):
            lines.append(f"- {r['app']} ({r.get('profile', '')}, {r.get('scale', '')}): {r['error']}")
    if report.get('fatal_error'):
        lines.append('Fatal validation error: ' + report['fatal_error'])
    with open(path, 'a') as stream:
        stream.write('\n'.join(lines) + '\n')


def save_report(out, report):
    (out / 'validation-results.json').write_text(json.dumps(report, indent=2) + '\n')


def print_failure_logs(out):
    for path in sorted(out.rglob('*.log')):
        # Logs contain synthetic fixtures only. Never print images or binary sidecars.
        text = path.read_text(errors='replace')[-16000:]
        print(f'--- final log excerpt: {path.name} ---\n{text}', flush=True)


def build(root, env, apps):
    packages = ['kobo-cli']
    for app in apps:
        source = directory(root, app)
        if source is None:
            raise RuntimeError(f'Missing source directory: {app}')
        package = tomllib.loads((source / 'Cargo.toml').read_text())['package']['name']
        if package not in packages:
            packages.append(package)
    command = ['cargo', '+1.85.1', 'build', '--locked', '--bins']
    for package in packages:
        command.extend(['-p', package])
    print('BUILD ' + ' '.join(command), flush=True)
    subprocess.run(command, cwd=root, env=env, check=True, timeout=1500)


def run_profiles(root, out, env, profiles, report, timeout):
    helper = load_helper(root)
    launcher_route = Path(__file__).with_name('launcher-review-route.kobo')
    for profile, scale in ((p, s) for p in profiles for s in ('default', '170')):
        profile_env = dict(env, KOBO_SIM_PROFILE=profile, KOBO_TEXT_SCALE=scale)
        for app in APPS:
            dest = out / profile / scale / app
            dest.mkdir(parents=True)
            source = directory(root, app)
            committed = next((source / n for n in ('drive.kobo', 'drive.txt') if (source / n).is_file()), None)
            override = launcher_route if app == 'launcher' else None
            started = time.monotonic()
            result = helper.run_app(app, Path(env['CARGO_TARGET_DIR']) / 'debug/kobo',
                                    dest, profile_env, timeout, route_override=override)
            result.update(profile=profile, scale=scale, source_sha=UNION_SHA,
                          elapsed_seconds=round(time.monotonic() - started, 2),
                          coverage='existing launcher review route' if override else
                          'committed route' if committed else 'first-screen launch only',
                          committed_route=str(committed.relative_to(root)) if committed else None)
            used_route = override or committed
            if used_route:
                result['route_sha256'] = hashlib.sha256(used_route.read_bytes()).hexdigest()
            report['results'].append(result)
            save_report(out, report)
            print(json.dumps(result), flush=True)
            if result['status'] != 'pass':
                print_failure_logs(dest)


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


def run_specialized(root, out, env, scale, report, download_score):
    score, score_error = music_score(root, out, download_score)
    for app, script_name in SPECIALIZED.items():
        dest = out / app
        dest.mkdir()
        result = dict(app=app, profile='clara-bw-391', scale=scale, source_sha=UNION_SHA,
                      coverage='committed specialized simulator journey')
        if app == 'musicstand' and score is None:
            result.update(status='unavailable', error=score_error,
                          coverage='specialized score journey unavailable; committed route runs in profile sweep')
        else:
            command = [sys.executable, str(Path(__file__).with_name('specialized_adapter.py')),
                       str(root / 'scripts/quality' / script_name),
                       '--output', str(dest), '--scale', scale]
            if app == 'musicstand':
                command.extend(['--score', str(score)])
            print('RUN ' + ' '.join(command), flush=True)
            started = time.monotonic()
            with (dest / 'harness.log').open('w') as log:
                process = subprocess.Popen(command, cwd=root, env=env, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
                try:
                    code = process.wait(timeout=1200)
                except subprocess.TimeoutExpired:
                    # Interrupt Python so the unchanged harness runs its finally
                    # cleanup for simulator sessions before forcing termination.
                    process.send_signal(signal.SIGINT)
                    try:
                        process.wait(timeout=30)
                    except subprocess.TimeoutExpired:
                        os.killpg(process.pid, signal.SIGKILL)
                        process.wait()
                    code = 124
                    result['error'] = 'Specialized journey exceeded its 20-minute outer deadline'
            result.update(status='pass' if code == 0 else 'fail',
                          exit_code=code, elapsed_seconds=round(time.monotonic()-started, 2))
            if code:
                print_failure_logs(dest)
        report['results'].append(result)
        save_report(out, report)
        print(json.dumps(result), flush=True)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    modes = parser.add_mutually_exclusive_group(required=True)
    modes.add_argument('--profiles', nargs='+', choices=PROFILES)
    modes.add_argument('--specialized-scale', choices=['default', '170'])
    parser.add_argument('--download-music-score', action='store_true')
    parser.add_argument('--timeout', type=int, default=240)
    args = parser.parse_args(argv)
    if args.timeout < 1:
        parser.error('--timeout must be positive')
    root, out = args.root.resolve(), args.out.resolve()
    if out == root or out.is_relative_to(root):
        parser.error('--out must be outside the tested checkout')
    out.mkdir(parents=True, exist_ok=True)
    report = dict(source_sha=UNION_SHA, workflow_sha=os.environ.get('GITHUB_SHA'), results=[])
    env = dict(os.environ, CARGO_TARGET_DIR=str(Path(os.environ.get('CARGO_TARGET_DIR', out / 'cargo-target')).resolve()),
               CARGO_PROFILE_DEV_DEBUG='0', CARGO_INCREMENTAL='0', RUSTUP_TOOLCHAIN='1.85.1',
               CARGO_BUILD_JOBS='2', PYTHONDONTWRITEBYTECODE='1')
    if Path(env['CARGO_TARGET_DIR']).is_relative_to(root):
        parser.error('CARGO_TARGET_DIR must be outside the tested checkout')
    try:
        assert_production_tree(root)
        build(root, env, APPS if args.profiles else list(SPECIALIZED))
        assert_production_tree(root)
        if args.profiles:
            run_profiles(root, out, env, args.profiles, report, args.timeout)
        else:
            run_specialized(root, out, env, args.specialized_scale, report, args.download_music_score)
        assert_production_tree(root)
    except Exception as error:
        report['fatal_error'] = str(error)
        print('VALIDATION ERROR: ' + str(error), flush=True)
    finally:
        save_report(out, report)
        write_summary(report, os.environ.get('GITHUB_STEP_SUMMARY'))
    unavailable = [r['app'] for r in report['results'] if r['status'] == 'unavailable']
    if unavailable:
        print('UNAVAILABLE (not passed): ' + ', '.join(unavailable), flush=True)
    return int(bool(report.get('fatal_error')) or any(r['status'] == 'fail' for r in report['results']))


if __name__ == '__main__':
    raise SystemExit(main())

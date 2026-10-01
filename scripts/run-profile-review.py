#!/usr/bin/env python3
"""Drive exact app sources through their committed simulator helper, including built-ins."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.dont_write_bytecode = True


def select_apps(manifest, args, parser):
    requested = args.apps
    if args.repair_only:
        if args.phase != 'after' or not manifest.get('repair_after_apps'):
            parser.error('--repair-only requires phase after and a nonempty repair_after_apps list')
        requested = list(manifest['repair_after_apps'])
        requested += manifest.get('repair_after_by_profile', {}).get(args.profile, [])
    selected = manifest['apps']
    if requested:
        unknown = set(requested) - {item['app'] for item in selected}
        if unknown:
            parser.error('apps absent from exact-head manifest: ' + ', '.join(sorted(unknown)))
        selected = [item for item in selected if item['app'] in requested]
    return selected


def source_directory(work, app):
    return next((work / group / app for group in ('apps', 'examples')
                 if (work / group / app).is_dir()), None)


def source_tree(work, path):
    return subprocess.check_output(['git', '-C', str(work), 'rev-parse',
                                    'HEAD:' + str(path.relative_to(work))], text=True).strip()


def load_helper(work):
    spec = importlib.util.spec_from_file_location('profile_sim_helper', work / 'scripts/check-apps-sim.py')
    helper = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(helper)
    return helper


def write_report(out, report):
    (out / 'profile-results.json').write_text(json.dumps(report, indent=2) + '\n')


def review_app(work, item, source_sha, helper, env, out, args):
    app = item['app']
    dest = out / app
    dest.mkdir(parents=True, exist_ok=True)
    directory = source_directory(work, app)
    if directory is None:
        return dict(app=app, status='fail', launched=False, source_sha=source_sha,
                    error='app has no source directory', failure_stage='source-discovery')
    committed_route = next((directory / name for name in ('drive.kobo', 'drive.txt')
                            if (directory / name).is_file()), None)
    # Preserve committed interactions and append a screenshot outside source.
    content = committed_route.read_text() if committed_route else 'dump\n'
    route = dest / 'profile-route.kobo'
    route.write_text(content.rstrip() + '\nclean\nshot profile-final\n')
    result = helper.run_app(app, Path(env['CARGO_TARGET_DIR']) / 'debug/kobo', dest,
                            env, args.timeout, route_override=route)
    result.update(source_sha=source_sha, pr=item['number'], profile=args.profile,
                  phase=args.phase, app_source_tree=source_tree(work, directory),
                  production_source_tree=source_tree(work, directory / 'src'),
                  route=str(committed_route.relative_to(work)) if committed_route else None,
                  route_sha256=hashlib.sha256(content.encode()).hexdigest(),
                  capture_route=str(route.relative_to(out)),
                  capture_scope='committed route plus final screenshot' if committed_route
                                else 'first-screen smoke and screenshot')
    (dest / 'results.json').write_text(json.dumps({'source_sha': source_sha, 'results': [result]}, indent=2) + '\n')
    return result


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--profile', required=True)
    parser.add_argument('--phase', choices=['before', 'after'], required=True)
    selection = parser.add_mutually_exclusive_group()
    selection.add_argument('--apps', nargs='+', help='only named manifest apps; default: all')
    selection.add_argument('--repair-only', action='store_true', help='after: use manifest repair scope')
    parser.add_argument('--timeout', type=int, default=300)
    args = parser.parse_args(argv)
    if args.timeout < 1:
        parser.error('--timeout must be positive')
    manifest = json.loads((ROOT / 'scripts/profile-review-heads.json').read_text())
    selected = select_apps(manifest, args, parser)
    out = ROOT / 'target/profile-review' / args.phase / args.profile
    out.mkdir(parents=True, exist_ok=True)
    report = dict(kind='full simulator through exact-source run_app helper',
                  workflow_head=os.environ.get('GITHUB_SHA'), profile=args.profile,
                  phase=args.phase, baseline=manifest['baseline'],
                  requested_apps=[item['app'] for item in selected], results=[])
    env = dict(os.environ, KOBO_SIM_PROFILE=args.profile, CARGO_TARGET_DIR=str(ROOT / 'target'))
    groups = [(manifest['baseline'], selected)] if args.phase == 'before' else [
        (item['head'], [item]) for item in selected]
    write_report(out, report)
    for source_sha, items in groups:
        work = ROOT / 'target' / ('profile-source-' + args.phase + '-' + items[0]['app'])
        created = False
        try:
            subprocess.run(['git', 'worktree', 'add', '--detach', str(work), source_sha], cwd=ROOT, env=env, check=True)
            created = True
            subprocess.run(['cargo', 'build', '--locked', '-p', 'kobo-cli'], cwd=work, env=env, check=True)
            helper = load_helper(work)
            for item in items:
                result = review_app(work, item, source_sha, helper, env, out, args)
                report['results'].append(result)
                write_report(out, report)
                print(json.dumps(result), flush=True)
        except (OSError, subprocess.SubprocessError) as error:
            completed = {r['app'] for r in report['results']}
            for item in items:
                if item['app'] not in completed:
                    report['results'].append(dict(app=item['app'], status='fail', launched=False,
                                                   source_sha=source_sha, error=str(error),
                                                   failure_stage='checkout-or-build'))
            write_report(out, report)
        finally:
            if created:
                cleanup = subprocess.run(['git', 'worktree', 'remove', str(work)], cwd=ROOT, env=env)
                if cleanup.returncode:
                    report.setdefault('cleanup_errors', []).append(str(work))
                    write_report(out, report)
    return int(any(r['status'] != 'pass' for r in report['results']) or bool(report.get('cleanup_errors')))


if __name__ == '__main__':
    raise SystemExit(main())

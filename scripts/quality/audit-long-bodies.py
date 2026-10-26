#!/usr/bin/env python3
"""Exercise long bodies, reading faces and CJK where applicable (APPQA-09).

Each case seeds one app, renders the body screen, and screenshots it. CJK
probes run twice: once against the default face set (the system face cannot
draw CJK, so the debug simulator halts - that halt is the documented
contract) and once with KOBO_READING_FONT pointing at the bundled
CobaltJapanese face, which is the path that makes CJK drawable.
"""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent.parent
EVIDENCE = ROOT / 'docs/quality/evidence/appqa09'
STATE = Path('/tmp/appqa09-state')
JAPANESE = ROOT / 'crates/kobo-flashcards-format/fonts/CobaltJapanese-Regular.otf'

LONG_BODY = ("Le corps du texte répète cette phrase accentuée pour remplir la page. " * 120).strip()
CJK_BODY = "これは長い本文です。日本語の文章が読書面で正しく表示されるかを確かめるためのものです。" * 40


def state_dir(name):
    return STATE / name


def run_case(name, app, seed, route, scales=('default',), extra_env=None, expect_halt=False):
    results = {}
    for scale in scales:
        out = EVIDENCE / name / scale
        shutil.rmtree(out, ignore_errors=True)
        out.mkdir(parents=True, exist_ok=True)
        shutil.rmtree(state_dir(name), ignore_errors=True)
        seed(state_dir(name))
        route_path = Path(f'/tmp/appqa09-{name}.kobo')
        route_path.write_text(route)
        env = dict(os.environ)
        if scale != 'default':
            env['KOBO_TEXT_SCALE'] = scale
        env.update(extra_env or {})
        cmd = ['python3', str(ROOT / 'scripts/check-apps-sim.py'), app,
               '--seed-root', str(state_dir(name)), '--bare',
               '--route', str(route_path), '--out', str(out)]
        subprocess.run(cmd, cwd=ROOT, env=env, capture_output=True, text=True)
        report = out / 'results.json'
        if not report.is_file():
            results[scale] = {'status': 'fail', 'error': 'no report'}
            continue
        result = json.loads(report.read_text())['results'][0]
        halted = result['status'] == 'fail'
        result['contract'] = ('halt matches the undrawable-character contract'
                              if expect_halt and halted else
                              'unexpected halt' if halted else 'rendered')
        results[scale] = result
    return results


def seed_vault_long(root):
    fixture = root / 'fixture'
    fixture.mkdir(parents=True)
    fixture.joinpath('Longbody.md').write_text(f'# Longbody\n\n{LONG_BODY}\n')
    log = open(root / 'seed.log', 'w')
    env = dict(os.environ, TMPDIR=str(root))
    subprocess.run([str(ROOT / 'target/debug/kobo'), 'vault', 'init', '--sim'],
                   cwd=ROOT, env=env, stdout=log, stderr=log, check=True)
    subprocess.run([str(ROOT / 'target/debug/kobo'), 'vault', 'push', str(fixture), '--sim'],
                   cwd=ROOT, env=env, stdout=log, stderr=log, check=True)


def seed_vault_cjk(root):
    fixture = root / 'fixture'
    fixture.mkdir(parents=True)
    fixture.joinpath('Japanese.md').write_text(f'# Japanese\n\n{CJK_BODY}\n')
    log = open(root / 'seed.log', 'w')
    env = dict(os.environ, TMPDIR=str(root))
    subprocess.run([str(ROOT / 'target/debug/kobo'), 'vault', 'init', '--sim'],
                   cwd=ROOT, env=env, stdout=log, stderr=log, check=True)
    subprocess.run([str(ROOT / 'target/debug/kobo'), 'vault', 'push', str(fixture), '--sim'],
                   cwd=ROOT, env=env, stdout=log, stderr=log, check=True)


def seed_needles_long(root):
    shelf = root / 'cobalt-sim-data' / 'needles'
    shelf.mkdir(parents=True)
    row = "k2, p2, k2tog, yo, p1, k3, p2, k1"
    lines = [f"Row {i}: {row}" for i in range(1, 3001)]
    shelf.joinpath('pattern.md').write_text('# Long pattern\n\n' + '\n'.join(lines) + '\n')
    shutil.copyfile(ROOT / 'apps/needles/fixtures/chart-main.png', shelf / 'chart-main.png')


VAULT_ROUTE = ('wait 500\nexpect Vault\ntap Browse\nexpect Longbody\ntap Longbody\n'
               'clean\nshot vault-long-body\n')
VAULT_CJK_ROUTE = ('wait 500\nexpect Vault\ntap Browse\nexpect Japanese\ntap Japanese\n'
                   'clean\nshot vault-cjk-body\n')
NEEDLES_ROUTE = 'wait 1500\nclean\nshot needles-long\n'

CASES = {
    'vault-long-body': dict(app='vault', seed=seed_vault_long, route=VAULT_ROUTE,
                            scales=('default', 'large', 'extra-large')),
    'needles-long-pattern': dict(app='needles', seed=seed_needles_long, route=NEEDLES_ROUTE,
                                 scales=('default', 'extra-large')),
    'vault-cjk-default': dict(app='vault', seed=seed_vault_cjk, route=VAULT_CJK_ROUTE,
                              expect_halt=True),
    'vault-cjk-japanese-face': dict(app='vault', seed=seed_vault_cjk, route=VAULT_CJK_ROUTE,
                                    extra_env={'KOBO_READING_FONT': str(JAPANESE)}),
}


def main():
    names = sys.argv[1:] or list(CASES)
    verdicts_path = EVIDENCE / 'cases.json'
    verdicts = json.loads(verdicts_path.read_text()) if verdicts_path.is_file() else {}
    for name in names:
        case = CASES[name]
        results = run_case(name, **case)
        verdicts[name] = results
        verdicts_path.write_text(json.dumps(verdicts, indent=1) + '\n')
        for scale, result in results.items():
            print(name, scale, result['status'], result.get('contract', ''), flush=True)


if __name__ == '__main__':
    main()

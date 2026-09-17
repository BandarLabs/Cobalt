#!/usr/bin/env python3
"""Verify the Sync companion CLI end to end against stub services.

A stub `syncthing` binary (identity generation, device ID, and a loopback
REST API that records configuration writes) and a stub `ssh` (answering the
kobod pairing calls) let the real `kobo sync` binary run setup, plan, run,
status, pause/resume, publish and stop exactly as an owner would, with every
write captured for assertion. Run:
python3 scripts/quality/check-sync-cli.py --output target/sim-check/sync-cli
"""
import argparse
import json
import os
import shutil
import stat
import subprocess
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
STUBBIN = Path(__file__).resolve().parent / 'sync-cli-stub'
HOST_ID = "AAAAAAA-AAAAAAA-AAAAAAA-AAAAAAA-AAAAAAA-AAAAAAA-AAAAAAA-AAAAAAA"
KOBO_ID = "KKKKKKK-KKKKKKK-KKKKKKK-KKKKKKK-KKKKKKK-KKKKKKK-KKKKKKK-KKKKKKK"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    target = Path(os.environ.get('CARGO_TARGET_DIR', str(ROOT / 'target'))).resolve()
    cli = target / 'debug/kobo'
    result = {'server': 'stub syncthing + stub ssh', 'checks': []}
    checks = result['checks']

    # The stub binds one fixed loopback port; never start against a leftover.
    subprocess.run(['pkill', '-f', 'sync-cli-stub/syncthing'], capture_output=True)
    time.sleep(.5)
    with tempfile.TemporaryDirectory(prefix='cobalt-sync-cli-', dir='/tmp') as temporary:
        base = Path(temporary)
        home = base / 'home'
        sync_home = base / 'sync-home'
        notes = base / 'notes'
        home.mkdir()
        notes.mkdir()
        (notes / 'Alpha.md').write_text('# Alpha\nsee [[Beta]]\n')
        (notes / 'Beta.md').write_text('Beta body\n')
        photos = base / 'photos'
        photos.mkdir()
        # A valid 1x1 PNG.
        (photos / 'photo-one.png').write_bytes(bytes([
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9c, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00,
            0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
        ]))
        env = dict(os.environ,
                   PATH=f"{STUBBIN}:{os.environ['PATH']}",
                   HOME=str(home), KOBO_SYNC_HOME=str(sync_home))

        def run_cli(*cli_args, expect=0, extra_env=None):
            merged = dict(env)
            if extra_env:
                merged.update(extra_env)
            completed = subprocess.run([str(cli), 'sync', *cli_args], capture_output=True,
                                       text=True, timeout=120, env=merged)
            if completed.returncode != expect:
                raise RuntimeError(
                    f"kobo sync {' '.join(cli_args)} exited {completed.returncode} "
                    f"(wanted {expect}): {completed.stdout}\n{completed.stderr}")
            return completed

        def mode(path):
            return stat.S_IMODE(path.stat().st_mode)

        def record():
            log = sync_home / 'stub-record.jsonl'
            if not log.is_file():
                return []
            return [json.loads(line) for line in log.read_text().splitlines()]

        # 1. setup pairs through the stub Kobo and writes a private home.
        completed = run_cli('setup', str(notes), '--folder', 'vault', '--device', '192.168.7.1')
        assert 'Sync mapping ready' in completed.stdout, completed.stdout
        assert (sync_home / 'kobo-host.json').is_file()
        for name in ('kobo-host.json', 'cert.pem', 'key.pem', 'config.xml'):
            assert mode(sync_home / name) == 0o600, name
        assert mode(sync_home) == 0o700
        put = next(entry['put-config'] for entry in record() if 'put-config' in entry)
        folder = put['folders'][0]
        assert folder['id'] == 'kobo-vault' and folder['type'] == 'sendonly'
        assert folder['path'] == str(notes)
        assert {device['deviceID'] for device in folder['devices']} == {HOST_ID, KOBO_ID}
        assert put['gui']['address'] == '127.0.0.1:8385'
        # Isolation: nothing landed in the default location under HOME.
        assert not (home / '.config/kobo/syncthing').exists()
        checks.append({'name': 'setup writes an isolated 0700 home, 0600 secrets, '
                               'loopback API and a send-only vault mapping'})

        # 2. A relative explicit root is refused.
        refused = run_cli('status', expect=1, extra_env={'KOBO_SYNC_HOME': 'relative'})
        assert 'absolute' in refused.stderr + refused.stdout
        checks.append({'name': 'a relative KOBO_SYNC_HOME is rejected'})

        # 3. plan states the fixed directions and the ingest contract.
        plan = json.loads(run_cli('plan', '--json').stdout)
        assert plan['format'] == 'kobo-sync-plan'
        assert plan['folders'][0]['id'] == 'kobo-vault'
        assert plan['folders'][0]['direction'] == 'sendonly'
        assert 'synced.v1' in plan['folders'][0]['ingest']
        assert plan['gui'] == '127.0.0.1:8385'
        checks.append({'name': 'plan --json states direction, ingest contract and loopback API'})

        # 4. run + status surface last change and errors, then pause/resume.
        run_cli('run')
        for _ in range(50):
            probe = json.loads(run_cli('status', '--json').stdout)
            if probe['running']:
                break
            time.sleep(.2)
        assert probe['running'] is True
        assert probe['folders'][0]['state'] == 'idle'
        assert probe['folders'][0]['last_change'] == 1789617600  # 2026-09-17T04:00:00Z
        assert probe['folders'][0]['errors'] == 0
        text = run_cli('status').stdout
        assert 'running' in text and 'sendonly' in text
        run_cli('pause')
        run_cli('resume')
        actions = [key for entry in record() for key in entry if key in ('pause', 'resume')]
        assert actions == ['pause', 'resume'], actions
        checks.append({'name': 'run, structured status with last change and errors, '
                               'pause and resume against the paired reader'})

        # 5. publish packs raw notes into the synced.v1 ingest package.
        published = run_cli('publish', '--folder', 'vault').stdout
        assert 'Packed 2 note(s)' in published, published
        manifest = json.loads((notes / 'synced.v1').read_text())
        assert manifest['format'] == 'vault-shelf'
        ids = [note['id'] for note in manifest['notes']]
        assert len(ids) == 2 and all(note_id.startswith('synced-note-') for note_id in ids)
        for note_id in ids:
            assert (notes / f'{note_id}.md').is_file()
        before = (notes / 'synced.v1').read_text()
        run_cli('publish', '--folder', 'vault')
        assert (notes / 'synced.v1').read_text() == before
        checks.append({'name': 'publish packs raw notes into the exact synced.v1 package '
                               'the reader ingests, idempotently'})

        # 6. stop leaves a clean stopped state.
        run_cli('stop')
        probe = json.loads(run_cli('status', '--json').stdout)
        assert probe['running'] is False
        checks.append({'name': 'stop leaves the dedicated peer stopped'})


        # 7. frame publish packs a manifest.v1 album without re-ingesting itself.
        run_cli('setup', str(photos), '--folder', 'frame', '--device', '192.168.7.1')
        packed = run_cli('publish', '--folder', 'frame').stdout
        assert 'Packed 1 photo(s) into manifest.v1' in packed, packed
        lines = (photos / 'manifest.v1').read_text().splitlines()
        assert lines[0] == 'cobalt-frame-v1' and len(lines) == 2
        shelf_name = f"{lines[1].split(chr(9))[0]}.png"
        assert (photos / shelf_name).is_file()
        again = run_cli('publish', '--folder', 'frame').stdout
        assert 'Packed 1 photo(s)' in again
        assert (photos / 'manifest.v1').read_text().splitlines()[1].split(chr(9))[0] == lines[1].split(chr(9))[0]
        checks.append({'name': 'frame publish packs a manifest.v1 album, idempotently'})

    (output / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
    for check in checks:
        print(f"PASS {check['name']}")
    print(f"ALL {len(checks)} CHECKS PASSED")


if __name__ == '__main__':
    main()

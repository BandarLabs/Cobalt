#!/usr/bin/env python3
"""Drive the real Store app against private local signed package transactions."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import tempfile
import time
import urllib.request

ROOT = Path(__file__).resolve().parents[2]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--scale', default='extra-large')
    args = parser.parse_args()
    cli = ROOT / 'target/debug/kobo'
    builder = ROOT / 'target/debug/examples/signed-store'
    if not cli.is_file() or not builder.is_file():
        parser.error('Build kobo-cli and the kobo-sim signed-store example first.')
    args.output.mkdir(parents=True, mode=0o700)
    process = None
    with tempfile.TemporaryDirectory(prefix='cobalt-store-', dir='/tmp') as private:
        fixture = Path(private) / 'fixture'
        env = dict(os.environ, TMPDIR=private, CARGO_TARGET_DIR=str(ROOT / 'target'),
                   CARGO_PROFILE_DEV_DEBUG='0', CARGO_INCREMENTAL='0',
                   KOBO_SIM_PROFILE='clara-bw-391', KOBO_TEXT_SCALE=args.scale,
                   KOBO_SIM_APP_STORE=str(fixture), KOBO_SIM_FIXTURE='original-signed-store',
                   KOBO_SIM_SEED='0', KOBO_SIM_CLOCK_MILLIS='1788850860000')
        with (args.output / 'simulator.log').open('w') as log:
            def publish(version, mode='publish'):
                subprocess.run([str(builder), mode, str(fixture), version], check=True,
                               stdout=log, stderr=log, timeout=15)

            def stop():
                if process is not None and process.poll() is None:
                    os.killpg(process.pid, signal.SIGTERM)
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        os.killpg(process.pid, signal.SIGKILL)
                        process.wait(timeout=5)

            try:
                publish('1.0.0', 'init')

                def start():
                    nonlocal process
                    start_offset = log.tell()
                    process = subprocess.Popen([str(cli), 'dev', '127.0.0.1:0'],
                                               cwd=ROOT / 'examples/store', env=env,
                                               stdout=log, stderr=log, start_new_session=True)
                    deadline = time.monotonic() + 120
                    while time.monotonic() < deadline:
                        text = (args.output / 'simulator.log').read_text()[start_offset:]
                        if process.poll() is not None:
                            raise RuntimeError('Store simulator stopped: ' + text[-2000:])
                        match = re.search(r'Kobo app simulator: http://(127\.0\.0\.1:\d+)', text)
                        if match:
                            return match.group(1)
                        time.sleep(.1)
                    raise RuntimeError('Store simulator did not start')

                address = start()

                def drive(*steps):
                    command = [str(cli), 'drive', '--address', address, '--ideal', '--shots', str(args.output)]
                    for step in steps:
                        command.extend(['--step', step])
                    subprocess.run(command, cwd=ROOT, env=env, check=True, timeout=30,
                                   stdout=log, stderr=log)

                def installed():
                    return json.loads((fixture / 'installed/apps/quality-fixture/manifest.json').read_text())['version']

                drive('wait-idle', 'expect-state /simulation#/appStore/mode "signed-local"',
                      'tap-id app-quality-fixture', 'shot available',
                      'tap-id install-quality-fixture', 'expect installed successfully', 'shot installed')
                assert installed() == '1.0.0'
                notes = fixture / 'installed/data/quality-fixture/notes'
                notes.parent.mkdir(parents=True)
                notes.write_text('Original owner fixture note\n')
                publish('1.1.0')
                drive('tap-id refresh', 'tap-id app-quality-fixture', 'shot update',
                      'scenario storage-full', 'tap-id install-quality-fixture', 'expect Check free space', 'shot failed')
                assert installed() == '1.0.0'
                drive('scenario normal', 'tap-id app-quality-fixture',
                      'tap-id install-quality-fixture', 'expect updated successfully', 'shot updated')
                assert installed() == '1.1.0'
                stop()
                address = start()
                drive('wait-idle', 'expect Installed · 1.1.0', 'shot reopened',
                      'tap-id app-quality-fixture', 'tap-id remove-quality-fixture',
                      'expect removed successfully', 'shot removed')
                assert not (fixture / 'installed/apps/quality-fixture').exists()
                assert notes.read_text() == 'Original owner fixture note\n'
                drive('tap-id app-quality-fixture', 'tap-id install-quality-fixture',
                      'expect installed successfully', 'shot reinstalled')
                assert installed() == '1.1.0'
                with urllib.request.urlopen(f'http://{address}/simulation', timeout=5) as response:
                    simulation = json.load(response)
                result = dict(status='passed', basis='store-app-sdk-ipc-and-runtime-transactions',
                              original_fixture=True, fixture_payload='inert; not a launch validation',
                              scale=args.scale, app_store=simulation['appStore'],
                              checks=['install', 'disk-full preserves version', 'update', 'process restart',
                                      'remove preserves data', 'reinstall'],
                              cli_sha256=hashlib.sha256(cli.read_bytes()).hexdigest(),
                              builder_sha256=hashlib.sha256(builder.read_bytes()).hexdigest(),
                              source_head=subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
                              source_dirty=bool(subprocess.check_output(['git', 'status', '--porcelain'], cwd=ROOT)))
                (args.output / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
            finally:
                stop()


if __name__ == '__main__':
    main()

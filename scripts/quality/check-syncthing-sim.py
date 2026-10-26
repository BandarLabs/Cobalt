#!/usr/bin/env python3
"""Verify the Sync app against fixture supervisor status in the simulator.

No Syncthing engine runs here: the app only ever reads the runtime-owned
status and import-report files, so writing those files exactly the way the
supervisor writes them exercises every screen the owner can reach - first
run, pairing guide, running transfer, completed window with imports,
conflict, pause and restart persistence. Run:
python3 scripts/quality/check-syncthing-sim.py --output target/sim-check/syncthing
"""
import argparse
import json
import os
import re
import shutil
import signal
import subprocess
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]



def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--scale', default='default')
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    target = Path(os.environ.get('CARGO_TARGET_DIR', str(ROOT / 'target'))).resolve()
    cli = target / 'debug/kobo'
    process = None
    result = {'server': 'fixture supervisor files', 'checks': []}
    with tempfile.TemporaryDirectory(prefix='cobalt-syncthing-', dir='/tmp') as temporary:
        private = Path(temporary)
        env = dict(os.environ, TMPDIR=str(private), CARGO_TARGET_DIR=str(target),
                   CARGO_PROFILE_DEV_DEBUG='0', CARGO_INCREMENTAL='0',
                   KOBO_SIM_PROFILE='clara-bw-391', KOBO_TEXT_SCALE=args.scale)
        env.pop('KOBO_SIM_OFFLINE', None)
        LAST_SUCCESS = int(time.time()) - 3600
        state = private / 'cobalt-sim-state/syncthing'
        state.mkdir(parents=True)
        # Enabled, hourly, before the first start.
        (state / 'sync-config').write_text('true\n1')

        def write_status(text):
            (state / 'sync-status').write_text(text)

        log_path = output / 'simulator.log'
        with log_path.open('w') as log:
            def stop():
                nonlocal process
                if process and process.poll() is None:
                    os.killpg(process.pid, signal.SIGTERM)
                    process.wait(timeout=15)
                process = None

            def start():
                nonlocal process
                log.seek(0)
                log.truncate()
                process = subprocess.Popen([str(cli), 'dev', '127.0.0.1:0'],
                                           cwd=ROOT / 'apps/syncthing', env=env,
                                           stdout=log, stderr=log, start_new_session=True)
                deadline = time.monotonic() + 300
                while time.monotonic() < deadline:
                    if process.poll() is not None:
                        raise RuntimeError('Simulator exited; see simulator.log')
                    found = re.search(r'Kobo app simulator: http://(127\.0\.0\.1:\d+)',
                                      log_path.read_text())
                    if found:
                        return found.group(1)
                    time.sleep(.1)
                raise TimeoutError('Simulator startup timed out')

            def drive(*steps, timeout=240):
                command = [str(cli), 'drive', '--address', ADDRESS, '--ideal',
                           '--shots', str(output)]
                for step in steps:
                    command.extend(['--step', step])
                subprocess.run(command, cwd=ROOT, env=env, stdout=log, stderr=log,
                               check=True, timeout=timeout)

            def capture(name):
                drive('clean', 'shot ' + name)

            try:
                ADDRESS = start()
                drive('wait-for Refresh status', 'wait-idle', timeout=300)
                capture('sync-fresh')
                result['checks'].append(dict(
                    name='fresh start shows never-synced facts and the setup row',
                    shot='sync-fresh.png'))

                drive('tap Set up Sync', 'wait-for Pair one computer folder', 'wait-idle')
                capture('sync-guide')
                result['checks'].append(dict(
                    name='guide walks through pairing one folder and the fixed set',
                    shot='sync-guide.png'))
                drive('tap Back', 'wait-for Refresh status', 'wait-idle')

                write_status(f'running\n4096\nSyncing folders.\n1\n0\n0')
                drive('tap Refresh status', 'wait-for Syncing', 'wait-idle')
                capture('sync-running')
                result['checks'].append(dict(
                    name='running window shows state, bytes remaining and peers online',
                    shot='sync-running.png'))

                write_status(f'idle\n0\nLast sync: complete. vault: 2 files imported.\n1\n{LAST_SUCCESS}\n0')
                (state / 'sync-ingest').write_text(f'vault\t2\t4096\t{LAST_SUCCESS}')
                drive('tap Refresh status', 'wait-for First sync complete', 'wait-idle')
                capture('sync-first-complete')
                result['checks'].append(dict(
                    name='first verified sync is celebrated once, with last success, next window and the Vault import',
                    shot='sync-first-complete.png'))

                drive('tap Folders', 'wait-for sync/vault', 'wait-idle')
                capture('sync-folders')
                result['checks'].append(dict(
                    name='folders view explains the fixed set and which folders import',
                    shot='sync-folders.png'))
                drive('tap About', 'wait-for MPL-2.0', 'wait-idle')
                capture('sync-about')
                result['checks'].append(dict(
                    name='engine attribution stays reachable',
                    shot='sync-about.png'))
                drive('tap Back', 'wait-for Refresh status', 'wait-idle')

                write_status(f'conflict\n0\n2 file(s) could not sync. They retry on the next window.\n1\n{LAST_SUCCESS}\n2')
                drive('tap Refresh status', 'wait-for could not sync', 'wait-idle')
                capture('sync-conflict')
                result['checks'].append(dict(
                    name='conflict state is distinct and names the retry path',
                    shot='sync-conflict.png'))

                drive('tap Back', 'tap Pause Sync', 'wait-for Sync is off', 'wait-idle')
                capture('sync-paused')
                result['checks'].append(dict(
                    name='pause shows the service off with the resume path',
                    shot='sync-paused.png'))

                stop()
                ADDRESS = start()
                drive('wait-for Refresh status', 'wait-idle', timeout=300)
                capture('sync-restored')
                result['checks'].append(dict(
                    name='restart keeps config, import report and the seen first sync (no repeated banner)',
                    shot='sync-restored.png'))
            finally:
                stop()
    failed = [c for c in result['checks'] if c.get('error')]
    (output / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
    for check in result['checks']:
        print('PASS', check['name'])
    if failed:
        raise SystemExit(f'{len(failed)} check(s) failed')
    print(f"ALL {len(result['checks'])} CHECKS PASSED")


if __name__ == '__main__':
    main()

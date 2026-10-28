#!/usr/bin/env python3
"""Verify the Sync app against fixture supervisor status in the simulator.

No Syncthing engine runs here: the app only ever reads the runtime-owned
status and import-report files and saves the request the runtime answers, so
writing those files exactly the way the supervisor writes them exercises every
screen the owner can reach: first run, pairing guide, Sync now, a running sync
followed without a refresh, a finished sync with imports, a conflict, a
computer that was not found, a request nothing answers, turning Sync off and
restart persistence. Run:
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
                drive('wait-for Sync now', 'wait-idle', timeout=300)
                drive('expect Not run yet', 'expect No sync has finished yet',
                      'expect When on Wi-Fi', 'expect-missing Refresh status',
                      'expect-missing window', 'expect-missing Peers')
                capture('sync-fresh')
                result['checks'].append(dict(
                    name='fresh start says nothing has run, offers Sync now and the setup row, '
                         'with no Syncthing words and no refresh button',
                    shot='sync-fresh.png'))

                drive('tap Set up Sync', 'wait-for Pair one computer folder',
                      'expect turn on Sync here', 'expect-missing receive-only', 'wait-idle')
                capture('sync-guide')
                result['checks'].append(dict(
                    name='guide walks through pairing one folder and turning Sync on',
                    shot='sync-guide.png'))
                drive('tap Back', 'wait-for Sync now', 'wait-idle')

                # Sync now saves the request the runtime answers. Nothing else
                # can start a sync, so the file is the whole contract.
                drive('tap Sync now', 'wait-for Starting', 'wait-idle')
                assert (state / 'sync-request').is_file(), 'Sync now did not ask the runtime'
                result['checks'].append(dict(
                    name='Sync now saves the request the runtime starts a sync from'))

                # What the runtime does as the sync starts. The app follows it
                # without anybody tapping anything.
                (state / 'sync-request').unlink()
                (state / 'sync-last-attempt').write_text(str(int(time.time())))
                write_status('running\n4096\nSyncing folders.\n1\n0\n0')
                drive('wait-for Left to move', 'expect Syncing. This takes up to five minutes.',
                      'wait-idle')
                capture('sync-running')
                result['checks'].append(dict(
                    name='a running sync is followed on its own, with bytes left and computers found',
                    shot='sync-running.png'))

                write_status(f'idle\n0\nSync finished. Vault: 2 files imported.\n1\n{LAST_SUCCESS}\n0')
                (state / 'sync-ingest').write_text(f'vault\t2\t4096\t{LAST_SUCCESS}')
                drive('wait-for First sync finished', 'expect Added to Vault', 'wait-idle')
                capture('sync-first-complete')
                result['checks'].append(dict(
                    name='first finished sync is announced once, with last sync, next sync and the Vault import',
                    shot='sync-first-complete.png'))

                drive('tap Folders', 'wait-for sync/vault',
                      'expect Notes, from the computer to Vault', 'expect-missing Receive only',
                      'wait-idle')
                capture('sync-folders')
                result['checks'].append(dict(
                    name='folders view says in plain words which way each folder goes',
                    shot='sync-folders.png'))
                drive('tap About', 'wait-for MPL-2.0', 'wait-idle')
                capture('sync-about')
                result['checks'].append(dict(
                    name='engine attribution stays reachable',
                    shot='sync-about.png'))
                drive('tap Back', 'tap Back', 'wait-for Sync now', 'wait-idle')

                # Coming back to the app reads what the runtime wrote meanwhile.
                write_status(f'conflict\n0\n2 file(s) could not sync. Sync tries them again next time.\n1\n{LAST_SUCCESS}\n2')
                drive('lifecycle background', 'lifecycle foreground',
                      'wait-for could not sync', 'expect Files not synced', 'wait-idle')
                capture('sync-conflict')
                result['checks'].append(dict(
                    name='conflict state is distinct, counted and names the retry',
                    shot='sync-conflict.png'))

                write_status(f'timed-out\n0\nThe computer was not found. Run kobo sync run on it, keep both on the same network, then tap Sync now.\n0\n{LAST_SUCCESS}\n0')
                drive('lifecycle background', 'lifecycle foreground',
                      'wait-for The computer was not found', 'expect Did not finish', 'wait-idle')
                capture('sync-not-found')
                result['checks'].append(dict(
                    name='a sync that found no computer says so and names the fix',
                    shot='sync-not-found.png'))

                # A runtime too old to answer: the request is never picked up.
                drive('tap Sync now', 'wait-for Starting', 'wait 32000',
                      'wait-for Sync has not started', 'expect Not started', 'wait-idle',
                      timeout=120)
                capture('sync-not-started')
                result['checks'].append(dict(
                    name='a request nothing answers is reported rather than shown as starting forever',
                    shot='sync-not-started.png'))

                drive('tap-id toggle', 'wait-for Sync is off',
                      'expect Syncs once now, then on your schedule.', 'wait-idle')
                capture('sync-paused')
                result['checks'].append(dict(
                    name='turning Sync off shows it off with the way back on',
                    shot='sync-paused.png'))

                stop()
                ADDRESS = start()
                drive('wait-for Syncs once now, then on your schedule.',
                      'expect-missing First sync finished',
                      'expect Added to Vault', 'wait-idle', timeout=300)
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

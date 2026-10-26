#!/usr/bin/env python3
"""Check real launcher/app handoffs, retained processes, eviction and app death."""
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


def action_id(name):
    value = 2166136261
    for byte in name.encode():
        value = ((value ^ byte) * 16777619) & 0xffffffff
    return value


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--scale', default='extra-large')
    args = parser.parse_args()
    args.output.mkdir(parents=True, mode=0o700)
    cli = ROOT / 'target/debug/kobo'
    process = None
    with tempfile.TemporaryDirectory(prefix='cobalt-runtime-', dir='/tmp') as private:
        items = Path(private) / 'cobalt-sim-state/todo/items'
        items.parent.mkdir(parents=True)
        items.write_text('- Read the garden notes\n')
        env = dict(os.environ, TMPDIR=private, CARGO_TARGET_DIR=str(ROOT / 'target'),
                   CARGO_PROFILE_DEV_DEBUG='0', CARGO_INCREMENTAL='0',
                   KOBO_SIM_PROFILE='clara-bw-391', KOBO_TEXT_SCALE=args.scale,
                   KOBO_SIM_FIXTURE='original-runtime-journey', KOBO_SIM_SEED='0',
                   KOBO_SIM_CLOCK_MILLIS='1788850860000')
        # The selected mode belongs to this fixture, regardless of the caller's shell.
        env.pop('KOBO_SIM_APP_STORE', None)
        with (args.output / 'simulator.log').open('w') as log:
            try:
                process = subprocess.Popen([str(cli), 'dev', '--runtime', '127.0.0.1:0',
                                            '--apps', 'todo,store,magnet,tictactoe'],
                                           cwd=ROOT, env=env, stdout=log, stderr=log,
                                           start_new_session=True)
                deadline = time.monotonic() + 120
                while time.monotonic() < deadline:
                    text = (args.output / 'simulator.log').read_text()
                    if process.poll() is not None:
                        raise RuntimeError('Runtime simulator stopped: ' + text[-2000:])
                    match = re.search(r'Kobo runtime simulator: http://(127\.0\.0\.1:\d+)', text)
                    if match:
                        address = match.group(1)
                        break
                    time.sleep(.1)
                else:
                    raise RuntimeError('Runtime simulator did not start')

                def get(endpoint):
                    with urllib.request.urlopen(f'http://{address}/{endpoint}', timeout=5) as response:
                        return json.load(response)

                def drive(*steps):
                    command = [str(cli), 'drive', '--address', address, '--ideal', '--shots', str(args.output)]
                    for step in steps:
                        command.extend(['--step', step])
                    subprocess.run(command, cwd=ROOT, env=env, check=True, timeout=30, stdout=log, stderr=log)

                panel_submitted = 0

                def wait_app(name):
                    nonlocal panel_submitted
                    deadline = time.monotonic() + 10
                    while time.monotonic() < deadline:
                        nav = get('simulation')['navigation']
                        if nav['app'] == name:
                            drive('wait-idle')
                            submitted = int(get('panel')['submitted'])
                            assert submitted >= panel_submitted, 'Panel history reset on app switch'
                            panel_submitted = submitted
                            return nav
                        time.sleep(.05)
                    raise RuntimeError(f'Expected {name}, found {nav}')

                def find_action(name):
                    wanted = action_id(name)
                    # Returning to an app preserves its page. Search from the start.
                    for _ in range(20):
                        layout = get('layout')
                        if any(node.get('action') == wanted for node in layout['nodes']):
                            return
                        if not any(node.get('action') == action_id('previous') for node in layout['nodes']):
                            break
                        drive('tap-id previous')
                        if get('layout') == layout:
                            break
                    for _ in range(20):
                        layout = get('layout')
                        if any(node.get('action') == wanted for node in layout['nodes']):
                            return
                        if any(node.get('action') == action_id('next') for node in layout['nodes']):
                            drive('tap-id next')
                            if get('layout') == layout:
                                break
                        else:
                            break
                    raise RuntimeError('No reachable action for ' + name)

                def open_app(name):
                    find_action('open-' + name)
                    drive('tap-id open-' + name)
                    return wait_app(name)

                launcher = wait_app('launcher')
                drive('shot launcher')
                todo = open_app('todo')
                drive('tap-id item.0', 'expect Done', 'shot todo')
                assert items.read_text().startswith('x ')
                drive('tap Back')
                assert wait_app('launcher')['processId'] == launcher['processId']
                assert open_app('todo')['processId'] == todo['processId']
                drive('expect Done', 'shot returned', 'tap Back')
                wait_app('launcher')
                open_app('store')
                find_action('app-todo')
                drive('tap-id app-todo', 'shot store-detail', 'tap Back')
                assert get('simulation')['navigation']['app'] == 'store'
                drive('tap Back')
                wait_app('launcher')
                open_app('magnet')
                drive('tap Back')
                wait_app('launcher')
                open_app('tictactoe')
                drive('shot fourth-app', 'tap Back')
                nav = wait_app('launcher')
                assert len(nav['hosted']) <= 4 and 'launcher' in nav['hosted']
                assert 'todo' not in nav['hosted'], nav
                reopened = open_app('todo')
                assert reopened['processId'] != todo['processId']
                drive('expect Done', 'shot after-eviction')
                os.kill(reopened['processId'], signal.SIGKILL)
                assert wait_app('launcher')['processId'] == launcher['processId']
                drive('shot after-kill')
                again = open_app('todo')
                assert again['processId'] != reopened['processId']
                drive('expect Done', 'shot after-relaunch')
                result = dict(status='passed', basis='real-host-sdk-processes', scale=args.scale,
                              hardware_validation=False,
                              checks=['shell Back', 'app-owned Back', 'retained process and state',
                                      'four-process bound', 'shared panel history', 'launcher retained during eviction',
                                      'killed app returns to launcher', 'durable state after relaunch'],
                              cli_sha256=hashlib.sha256(cli.read_bytes()).hexdigest(),
                              source_head=subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
                              source_dirty=bool(subprocess.check_output(['git', 'status', '--porcelain'], cwd=ROOT)))
                (args.output / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
            finally:
                if process is not None and process.poll() is None:
                    os.killpg(process.pid, signal.SIGTERM)
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        os.killpg(process.pid, signal.SIGKILL)
                        process.wait(timeout=5)


if __name__ == '__main__':
    main()

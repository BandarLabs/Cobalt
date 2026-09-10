#!/usr/bin/env python3
"""Exercise the original board example through SDK IPC and the simulator driver."""
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
    parser.add_argument('--profile', default='clara-bw-391')
    parser.add_argument('--scale', default='extra-large')
    args = parser.parse_args()
    fixture = ROOT / 'target/debug/examples/board'
    cli = ROOT / 'target/debug/kobo'
    if not fixture.is_file() or not cli.is_file():
        parser.error('Build kobo-cli and the kobo-sim board example first.')
    args.output.mkdir(parents=True, mode=0o700)
    process = None
    with tempfile.TemporaryDirectory(prefix='cobalt-board-', dir='/tmp') as private:
        env = dict(os.environ, TMPDIR=private, KOBO_SIM_PROFILE=args.profile,
                   KOBO_TEXT_SCALE=args.scale, KOBO_SIM_CALLBACKS='1',
                   KOBO_SIM_FIXTURE='original-board', KOBO_SIM_SEED='0',
                   KOBO_SIM_CLOCK_MILLIS='1788850860000', KOBO_SIM_UTC_OFFSET_MINUTES='0')
        log_path = args.output / 'simulator.log'
        with log_path.open('w') as log:
            try:
                process = subprocess.Popen([str(fixture), str(Path(private) / 'board.sock')],
                                           cwd=ROOT, env=env, stdout=log, stderr=log,
                                           start_new_session=True)
                deadline = time.monotonic() + 30
                address = None
                while time.monotonic() < deadline:
                    if process.poll() is not None:
                        raise RuntimeError('Board fixture stopped: ' + log_path.read_text()[-2000:])
                    match = re.search(r'Board simulator: http://(127\.0\.0\.1:\d+)', log_path.read_text())
                    if match:
                        address = match.group(1)
                        break
                    time.sleep(.05)
                if address is None:
                    raise RuntimeError('Board fixture did not start.')

                def drive(*steps):
                    command = [str(cli), 'drive', '--address', address, '--ideal',
                               '--shots', str(args.output)]
                    for step in steps:
                        command.extend(['--step', step])
                    subprocess.run(command, cwd=ROOT, env=env, stdout=log, stderr=log,
                                   check=True, timeout=30)

                def layout():
                    with urllib.request.urlopen(f'http://{address}/layout', timeout=5) as response:
                        return json.load(response)

                def position():
                    return next(node['lines'] for node in layout()['nodes']
                                if node['kind'] == 'Secondary')

                drive('wait-idle', 'shot initial', 'tap-id board.cell.1', 'shot selected')
                assert any(node['kind'].endswith('Board, true)') and
                           node['lines'] == ['Row 1, column 2, filled'] for node in layout()['nodes'])
                drive('tap-id board.row.0', 'expect Row 1', 'expect 1 · 2 · 3 · 4 · 5', 'shot clue')
                drive('tap-id close')
                before = position()
                drive('tap-id board.right', 'shot panned')
                assert position() != before
                drive('tap-id board.larger', 'shot larger')
                assert any(node['kind'].endswith('Board, true)') and
                           node['lines'] == ['Row 1, column 2, filled'] for node in layout()['nodes'])
                drive('tap-id board.down', 'shot down', 'tap-id board.smaller', 'shot smaller')
                assert any(node['kind'].endswith('Board, true)') and
                           node['lines'] == ['Row 1, column 2, filled'] for node in layout()['nodes'])
                result = dict(status='passed', profile=args.profile, scale=args.scale,
                              original_fixture=True, basis='simulator-sdk-ipc',
                              fixture_sha256=hashlib.sha256(fixture.read_bytes()).hexdigest(),
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

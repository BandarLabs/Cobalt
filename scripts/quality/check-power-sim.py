#!/usr/bin/env python3
"""Exercise the real SDK save barrier against simulated power transitions."""
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
    args = parser.parse_args()
    args.output.mkdir(parents=True, mode=0o700)
    cli = ROOT / 'target/debug/kobo'
    fixture = ROOT / 'target/debug/examples/power'
    revision = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
    with tempfile.TemporaryDirectory(prefix='cobalt-power-', dir='/tmp') as private:
        env = dict(os.environ, TMPDIR=private, KOBO_SIM_PROFILE='clara-bw-391',
                   KOBO_TEXT_SCALE='extra-large', KOBO_SIM_CLOCK_MILLIS='1788850860000',
                   KOBO_SIM_SEED='0', KOBO_FIXTURE_REVISION=revision)
        env.pop('KOBO_SOCKET', None)
        env.pop('KOBO_SIM_APP_STORE', None)
        with (args.output / 'simulator.log').open('w') as log:
            process = subprocess.Popen([str(fixture), '127.0.0.1:0', str(Path(private) / 'sdk.sock'),
                                        str(ROOT / 'target/debug/kobo-launcher')], cwd=ROOT,
                                       env=env, stdout=log, stderr=log, start_new_session=True)
            try:
                deadline = time.monotonic() + 20
                while time.monotonic() < deadline:
                    text = (args.output / 'simulator.log').read_text()
                    if process.poll() is not None:
                        raise RuntimeError(text[-2000:])
                    match = re.search(r'Kobo runtime simulator: http://(127\.0\.0\.1:\d+)', text)
                    if match:
                        address = match.group(1)
                        break
                    time.sleep(.05)
                else:
                    raise RuntimeError('Power fixture did not start')

                def get(endpoint):
                    with urllib.request.urlopen(f'http://{address}/{endpoint}', timeout=5) as response:
                        return json.load(response)

                def post(endpoint, command):
                    request = urllib.request.Request(f'http://{address}/{endpoint}', data=command.encode())
                    with urllib.request.urlopen(request, timeout=5) as response:
                        return response.read()

                def drive(*steps):
                    command = [str(cli), 'drive', '--address', address, '--ideal', '--shots', str(args.output)]
                    for step in steps:
                        command.extend(['--step', step])
                    subprocess.run(command, cwd=ROOT, env=env, check=True, timeout=30, stdout=log, stderr=log)

                def wait_for(predicate):
                    deadline = time.monotonic() + 10
                    while time.monotonic() < deadline:
                        state = get('power')
                        if isinstance(state, dict) and predicate(state):
                            return state
                        time.sleep(.02)
                    raise RuntimeError('Power did not settle: ' + json.dumps(state))

                drive('wait-idle', 'tap-id open-todo', 'wait-idle', 'tap-id edit', 'shot edited')
                before = get('device')
                drive('scenario storage-full')
                post('power', 'sleep')
                wait_for(lambda s: s['state'] == 'awake' and s['lastRefusal'] == 'Some(Save)')
                drive('wait-idle', 'expect Note not saved', 'shot failed-save')
                assert not (Path(private) / 'cobalt-sim-state/todo/note').exists()
                drive('scenario normal')
                post('power', 'sleep')
                wait_for(lambda s: s['state'] == 'suspended')
                assert (Path(private) / 'cobalt-sim-state/todo/note').read_text() == 'Water the mint after sunset.'
                assert (Path(private) / 'cobalt-sim-state/todo/receipt').read_text() == 'note saved'
                drive('expect Note and receipt saved', 'expect Cancelled tasks: 1', 'shot asleep')
                asleep_panel = get('panel')
                post('power', 'wake scheduled')
                wait_for(lambda s: s['state'] == 'awake')
                drive('wait-idle', 'expect Resumes: 2', 'expect Scheduled: 1', 'shot resumed')
                post('power', 'wake scheduled')
                drive('wait-idle', 'expect Resumes: 2', 'expect Scheduled: 1')
                assert get('device') == before
                assert int(get('panel')['submitted']) >= int(asleep_panel['submitted'])

                # A held panel keeps the barrier in preparation until its deadline.
                post('panel', 'hold')
                post('power', 'sleep')
                wait_for(lambda s: s['state'] == 'preparing')
                post('clock', 'advance 6000')
                wait_for(lambda s: s['state'] == 'awake' and s['lastRefusal'] == 'Some(Deadline)')
                for _ in range(8):
                    if get('panel')['status'] != 'busy':
                        break
                    post('panel', 'complete')
                post('panel', 'auto')
                drive('wait-idle')

                post('power', 'usb attach')
                wait_for(lambda s: s['usbAttached'])
                post('power', 'sleep')
                wait_for(lambda s: s['state'] == 'awake' and s['lastRefusal'] == 'Some(Usb)')
                post('power', 'usb detach')
                wait_for(lambda s: not s['usbAttached'])
                post('power', 'sleep')
                wait_for(lambda s: s['state'] == 'suspended')
                post('power', 'usb attach')
                wait_for(lambda s: s['state'] == 'awake' and s['lastWake'] == 'Some(Usb)')
                drive('wait-idle', 'shot usb-wake')
                post('power', 'usb detach')
                drive('wait-idle')
                post('power', 'button down')
                post('power', 'button down')
                post('power', 'button up')
                asleep = wait_for(lambda s: s['state'] == 'suspended')
                post('power', 'button down')
                awake = wait_for(lambda s: s['state'] == 'awake')
                post('power', 'button down')
                post('power', 'button up')
                drive('wait-idle', 'shot button-wake')
                assert get('power')['state'] == 'awake', 'Wake release started another sleep attempt'
                result = dict(status='passed', hardware_validation=False, source_head=revision,
                              source_dirty=True, fixture_sha256=hashlib.sha256(fixture.read_bytes()).hexdigest(),
                              checks=['failed save blocks sleep', 'chained durable save acknowledgements',
                                      'task cancellation exactly once', 'duplicate scheduled wake ignored',
                                      'frontlight restored', 'panel completion timeout', 'USB blocks entry and wakes', 'duplicate power-button edges and wake release'])
                (args.output / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
            finally:
                try:
                    os.killpg(process.pid, signal.SIGTERM)
                    process.wait(timeout=5)
                except ProcessLookupError:
                    pass
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait(timeout=5)


if __name__ == '__main__':
    main()

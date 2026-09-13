#!/usr/bin/env python3
"""Exercise the sample helper over verified local TLS using isolated pairing."""
import argparse
import hashlib
import json
import os
import re
from pathlib import Path
import signal
import socket
import ssl
import subprocess
import tempfile
import time
import urllib.error
import urllib.request

def drive_sample(cli, environment, root, code, output, scale):
    repo = Path(__file__).resolve().parents[2]
    env = dict(environment, TMPDIR=str(root), KOBO_SIM_TRUST_DIR=str(root/'trust'),
               KOBO_SIM_PROFILE='clara-bw-391', KOBO_TEXT_SCALE=scale,
               CARGO_PROFILE_DEV_DEBUG='0', CARGO_INCREMENTAL='0')
    for key in ('KOBO_SIM_OFFLINE', 'KOBO_SIM_DEMO', 'KOBO_SIM_HTTP_FIXTURE'):
        env.pop(key, None)
    log_path = root/'simulator.log'
    with log_path.open('w') as log:
        process = subprocess.Popen([str(cli), 'dev', '127.0.0.1:0'], cwd=repo/'examples/sidekick',
                                   env=env, stdout=log, stderr=log, start_new_session=True)
        try:
            deadline = time.monotonic()+180
            while True:
                assert process.poll() is None, log_path.read_text()[-3000:]
                found = re.search(r'Kobo app simulator: http://(127\.0\.0\.1:\d+)', log_path.read_text())
                if found:
                    panel = found.group(1)
                    break
                assert time.monotonic()<deadline
                time.sleep(.1)
            def drive(*steps):
                command = [str(cli), 'drive', '--address', panel, '--ideal', '--shots', str(output)]
                for step in steps:
                    command += ['--step', step]
                subprocess.run(command, env=env, cwd=repo, stdout=log, stderr=log, check=True, timeout=60)
            def get(endpoint):
                with urllib.request.urlopen('http://'+panel+'/'+endpoint, timeout=5) as response:
                    return json.load(response)
            def words():
                return ' '.join(str(line) for node in get('layout')['nodes'] for line in node['lines'])
            def type_text(text):
                layer = 'letters' if '?123' in words() else 'symbols'
                for character in text:
                    wanted = 'letters' if character.isalpha() else 'symbols'
                    if wanted != layer:
                        drive('tap ?123' if wanted == 'symbols' else 'tap abc', 'wait-idle')
                        layer = wanted
                    drive('type '+character, 'wait-idle')
            def capture(name):
                drive('wait-idle', 'shot '+name)
                diagnostics = get('diagnostics')
                assert not [issue for issue in diagnostics['issues'] if issue['severity']=='error'], diagnostics
                (output/(name+'.layout.json')).write_text(json.dumps(get('layout'), indent=2)+'\n')
            drive('wait-for Pair with your computer')
            type_text('127.0.0.1:9331')
            drive('tap Next', 'wait-idle')
            type_text(code)
            drive('tap Pair', 'wait-for Received', 'wait-idle')
            capture('sample-question')
            drive('tap Received', 'wait-for Answered', 'wait-idle')
            assert 'Left at the terminal' not in words()
            capture('sample-answered')
        finally:
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGTERM)
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait(timeout=5)


parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--helper', type=Path, required=True)
parser.add_argument('--output', type=Path, required=True)
parser.add_argument('--cli', type=Path)
parser.add_argument('--scale', default='default')
args = parser.parse_args()
helper = str(args.helper.resolve())
args.output = args.output.resolve()
args.output.mkdir(parents=True, exist_ok=True)
# Hold the hook port throughout: sample must neither need nor serve that port.
hook_guard = socket.socket()
hook_guard.bind(('127.0.0.1', 9330))
hook_guard.listen(1)
with tempfile.TemporaryDirectory(prefix='sidekick-sample-', dir='/tmp') as temp:
    root = Path(temp)
    env = dict(os.environ, KOBO_SIDEKICK_CONFIG_DIR=str(root))
    subprocess.run([helper, 'init', '--host', '127.0.0.1'], env=env,
                   capture_output=True, check=True, timeout=30)
    code = (root/'sidekick/pairing').read_text().strip()
    context = ssl.create_default_context(cafile=str(root/'trust/sidekick.pem'))
    # An existing Deck file must still not be enabled in sample mode.
    (root/'sidekick/deck.toml').write_text('intentionally invalid; sample must ignore this file')
    log_path = root/'sample.log'
    with log_path.open('w') as log:
        child = subprocess.Popen([helper, 'sample'], env=env, stdout=log, stderr=log,
                                 start_new_session=True)
        def request(path, body=None):
            data = None if body is None else json.dumps(body).encode()
            req = urllib.request.Request('https://127.0.0.1:9331'+path, data=data)
            try:
                with urllib.request.urlopen(req, context=context, timeout=5) as response:
                    return response.status, json.load(response)
            except urllib.error.HTTPError as error:
                return error.code, json.load(error)
        try:
            deadline = time.monotonic()+10
            while 'Sidekick sample is ready.' not in log_path.read_text():
                assert child.poll() is None, log_path.read_text()
                assert time.monotonic()<deadline
                time.sleep(.05)
            assert request('/pending?token=wrong&wait=0')[0] == 403
            status, pending = request('/pending?token='+code+'&wait=1')
            assert status == 200
            ask = pending['ask']
            assert ask['source'] == 'Sidekick sample' and ask['permission'] is False
            assert request('/deck?token='+code)[0] == 404
            if args.cli:
                drive_sample(args.cli.resolve(), env, root, code, args.output, args.scale)
            else:
                assert request('/answer', {'token': code, 'id': ask['id'], 'labels': ['Received']})[1]['ok'] is True
            deadline = time.monotonic()+10
            while 'Received on your reader.' not in log_path.read_text():
                assert time.monotonic()<deadline
                time.sleep(.05)
            assert request('/pending?token='+code+'&wait=0')[1] == {}
            transcript = log_path.read_text()
            assert code not in transcript
        finally:
            if child.poll() is None:
                os.killpg(child.pid, signal.SIGTERM)
                child.wait(timeout=5)
hook_guard.close()
args.output.mkdir(parents=True, exist_ok=True)
(args.output/'terminal.txt').write_text(transcript)
(args.output/'result.json').write_text(json.dumps({'status': 'passed',
    'helper_sha256': hashlib.sha256(Path(helper).read_bytes()).hexdigest(),
    'checks': ['private pairing and verified local TLS', 'wrong pairing rejected',
               'sample starts while hook port is occupied', 'Deck unavailable',
               'sample question acknowledged and removed', 'terminal reports receipt'],
    'physical_reader_tested': False, 'simulator_ui_tested': bool(args.cli), 'scale': args.scale}, indent=2)+'\n')

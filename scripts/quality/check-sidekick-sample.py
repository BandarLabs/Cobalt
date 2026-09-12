#!/usr/bin/env python3
"""Exercise the sample helper over verified local TLS using isolated pairing."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import signal
import socket
import ssl
import subprocess
import tempfile
import time
import urllib.error
import urllib.request

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--helper', type=Path, required=True)
parser.add_argument('--output', type=Path, required=True)
args = parser.parse_args()
helper = str(args.helper.resolve())
# Hold the hook port throughout: sample must neither need nor serve that port.
hook_guard = socket.socket()
hook_guard.bind(('127.0.0.1', 9330))
hook_guard.listen(1)
with tempfile.TemporaryDirectory(prefix='sidekick-sample-') as temp:
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
            assert request('/answer', {'token': code, 'id': ask['id'], 'labels': ['Received']})[1]['ok'] is True
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
    'physical_reader_tested': False, 'simulator_ui_tested': False}, indent=2)+'\n')

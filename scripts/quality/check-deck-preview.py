#!/usr/bin/env python3
"""Stage a static Deck preview without commands, then check its reader UI."""
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

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--cli', type=Path, required=True)
parser.add_argument('--output', type=Path, required=True)
parser.add_argument('--scale', default='default')
args = parser.parse_args()
repo = Path(__file__).resolve().parents[2]
cli = str(args.cli.resolve())
output = args.output.resolve()
output.mkdir(parents=True, exist_ok=True)
with tempfile.TemporaryDirectory(prefix='deck-preview-', dir='/tmp') as temp:
    root = Path(temp)
    env = dict(os.environ, TMPDIR=temp, KOBO_SIM_PROFILE='clara-bw-391',
               KOBO_TEXT_SCALE=args.scale, CARGO_PROFILE_DEV_DEBUG='0', CARGO_INCREMENTAL='0')
    def command(*parts):
        return subprocess.run([cli, *parts], env=env, cwd=repo,
                              capture_output=True, text=True, check=True, timeout=30)
    owner = str(root/'owner')
    command('deck', 'set', '1', '--label', 'Sample', '--run', 'printf sample', '--home', owner)
    command('deck', 'push', '--sim', '--home', owner)
    store = root/'cobalt-sim-state/deck'
    assert (store/'paired').read_text() == 'local|assigned'
    log_path = root/'simulator.log'
    with log_path.open('w') as log:
        child = subprocess.Popen([cli, 'dev', '127.0.0.1:0'], cwd=repo/'apps/deck', env=env,
                                 stdout=log, stderr=log, start_new_session=True)
        try:
            deadline = time.monotonic()+120
            while True:
                assert child.poll() is None, log_path.read_text()[-2000:]
                match = re.search(r'Kobo app simulator: http://(127\.0\.0\.1:\d+)', log_path.read_text())
                if match:
                    panel = match.group(1)
                    break
                assert time.monotonic()<deadline
                time.sleep(.1)
            def drive(*steps):
                parts = ['drive', '--address', panel, '--ideal', '--shots', str(output)]
                for step in steps:
                    parts += ['--step', step]
                command(*parts)
            def get(endpoint):
                with urllib.request.urlopen('http://'+panel+'/'+endpoint, timeout=5) as response:
                    return json.load(response)
            drive('wait-for Preview only', 'wait-idle', 'shot preview')
            drive('tap Sample', 'wait-idle', 'shot preview-tapped')
            diagnostics = get('diagnostics')
            assert not [issue for issue in diagnostics['issues'] if issue['severity']=='error'], diagnostics
            layout = get('layout')
            words = ' '.join(str(line) for node in layout['nodes'] for line in node['lines'])
            assert 'Preview only' in words and 'Running' not in words
            (output/'preview-tapped.layout.json').write_text(json.dumps(layout, indent=2)+'\n')
            drive('tap Pair', 'wait-for Pair with your computer', 'wait-idle')
        finally:
            if child.poll() is None:
                os.killpg(child.pid, signal.SIGTERM)
                try:
                    child.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(child.pid, signal.SIGKILL)
                    child.wait(timeout=5)
    # A later transfer must preserve a real pairing, byte for byte.
    paired = b'192.0.2.10:9331|fixture-only-pairing'
    (store/'paired').write_bytes(paired)
    command('deck', 'push', '--sim', '--home', owner)
    assert (store/'paired').read_bytes() == paired
(output/'result.json').write_text(json.dumps({'status':'passed', 'scale':args.scale,
    'cli_sha256':hashlib.sha256(Path(cli).read_bytes()).hexdigest(),
    'checks':['fresh simulator opens static preview', 'preview tap does not claim execution',
              'no layout errors after tap', 'subsequent push preserves real pairing bytes'],
    'physical_reader_tested':False}, indent=2)+'\n')

#!/usr/bin/env python3
"""Pair a deck with a fixture computer, press a key, confirm the one that asks,
watch it run and read what it said, then find the deck again after a restart."""
import argparse
import http.server
import json
import os
from pathlib import Path
import re
import signal
import ssl
import subprocess
import tempfile
import threading
import time
import urllib.request

ROOT = Path(__file__).resolve().parents[2]

# Two pages, one key that asks before it runs, and fewer keys than the deck
# has places: an unassigned key is a real state, not a corner case.
DECK = {
    'version': '4',
    'pages': [
        {
            'name': 'Build',
            'keys': [
                {'id': 'test', 'label': 'Test', 'detail': 'cargo test', 'confirm': False,
                 'state': 'idle'},
                {'id': 'fmt', 'label': 'Format', 'detail': 'cargo fmt', 'confirm': False,
                 'state': 'idle'},
                {'id': 'deploy', 'label': 'Deploy', 'detail': 'ship it', 'confirm': True,
                 'state': 'idle'},
            ],
        },
        {
            'name': 'Home',
            'keys': [
                {'id': 'lights', 'label': 'Lights', 'detail': 'downstairs', 'confirm': False,
                 'state': 'idle'},
            ],
        },
    ],
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--scale', default='default')
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    target = Path(os.environ.get('CARGO_TARGET_DIR', str(ROOT/'target'))).resolve()
    cli = target/'debug/kobo'
    process = None
    requests = []
    pressed = {}
    with tempfile.TemporaryDirectory(prefix='cobalt-deck-', dir='/tmp') as temporary:
        private = Path(temporary)
        env = dict(os.environ, TMPDIR=str(private), CARGO_TARGET_DIR=str(target),
                   CARGO_PROFILE_DEV_DEBUG='0', CARGO_INCREMENTAL='0',
                   KOBO_SIM_PROFILE='clara-bw-391', KOBO_TEXT_SCALE=args.scale,
                   KOBO_SIM_FIXTURE='a-paired-command-deck', KOBO_SIM_SEED='0')
        config = private/'config'
        env.update(KOBO_STREAM_CONFIG_DIR=str(config), KOBO_SIM_TRUST_DIR=str(config/'trust'))
        subprocess.run([str(cli), 'stream', 'init', '--host', '127.0.0.1'], cwd=ROOT,
                       env=env, check=True, capture_output=True, timeout=60)

        class Handler(http.server.BaseHTTPRequestHandler):
            protocol_version = 'HTTP/1.1'

            def log_message(self, *_):
                pass

            def answer(self, body):
                self.send_response(200)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Content-Length', str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def do_GET(self):
                requests.append(self.path)
                path = self.path.split('?')[0]
                if path == '/deck':
                    deck = json.loads(json.dumps(DECK))
                    for page in deck['pages']:
                        for key in page['keys']:
                            key['state'] = pressed.get(key['id'], 'idle')
                    self.answer(json.dumps(deck).encode())
                elif path == '/deck/result':
                    key = self.path.split('key=')[1].split('&')[0]
                    self.answer(json.dumps({
                        'status': 'ok', 'exit': 0,
                        'tail': f'{key} finished on the computer',
                    }).encode())
                else:
                    self.send_error(404)

            def do_POST(self):
                requests.append(self.path)
                length = int(self.headers.get('Content-Length', '0'))
                asked = json.loads(self.rfile.read(length) or b'{}')
                key = asked.get('key', '')
                # The computer decides whether a command needs answering for,
                # which is what stops a reader's deck from being the only
                # thing standing between a stray tap and a deployment.
                wants = any(k['id'] == key and k['confirm']
                            for page in DECK['pages'] for k in page['keys'])
                if wants and not asked.get('confirmed'):
                    self.answer(b'{"outcome":"needs-confirm"}')
                    return
                pressed[key] = 'ok'
                self.answer(b'{"outcome":"started"}')

        server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        tls.load_cert_chain(config/'stream/cert.pem', config/'stream/key.pem')
        server.socket = tls.wrap_socket(server.socket, server_side=True)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        address = f'127.0.0.1:{server.server_port}'
        (private/'cobalt-sim-state/deck').mkdir(parents=True)
        log_path = output/'simulator.log'
        checks = []
        with log_path.open('w') as log:
            def stop():
                nonlocal process
                if process and process.poll() is None:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait(timeout=5)
                process = None

            def start():
                nonlocal process
                log.seek(0)
                log.truncate()
                process = subprocess.Popen([str(cli), 'dev', '127.0.0.1:0'],
                                           cwd=ROOT/'apps/deck', env=env, stdout=log,
                                           stderr=log, start_new_session=True)
                deadline = time.monotonic()+120
                while time.monotonic() < deadline:
                    if process.poll() is not None:
                        raise RuntimeError(log_path.read_text()[-3000:])
                    found = re.search(r'Kobo app simulator: http://(127\.0\.0\.1:\d+)',
                                      log_path.read_text())
                    if found:
                        return found.group(1)
                    time.sleep(.1)
                raise RuntimeError('Simulator did not start')

            def drive(*steps):
                command = [str(cli), 'drive', '--address', panel, '--ideal',
                           '--shots', str(output)]
                for step in steps:
                    command.extend(['--step', step])
                subprocess.run(command, cwd=ROOT, env=env, stdout=log, stderr=log,
                               check=True, timeout=90)

            def get(endpoint):
                with urllib.request.urlopen(f'http://{panel}/{endpoint}', timeout=5) as answer:
                    return json.load(answer)

            def capture(name):
                drive('wait-idle', 'shot '+name)
                diagnostics = get('diagnostics')
                errors = [i for i in diagnostics['issues'] if i['severity'] == 'error']
                assert not errors, f'{name}: {errors}'
                layout = get('layout')
                (output/(name+'.layout.json')).write_text(json.dumps(layout, indent=2)+'\n')
                metadata = json.loads((output/(name+'.json')).read_text())
                assert metadata['app'] == 'deck', metadata
                return layout

            def words(layout):
                drawn = ' '.join(str(line) for node in layout['nodes'] for line in node['lines'])
                return ' '.join(drawn.split())

            LETTERS = set('abcdefghijklmnopqrstuvwxyz')

            def type_text(text):
                """Types the way a thumb does, including the layer switch.

                Digits and punctuation are on the second layer, so an address
                like 127.0.0.1:8080 is one tap on ?123 and then the whole of
                it. The switch is sticky, which is why this only taps it when
                the layer actually has to change.
                """
                # Which layer is showing is read off the panel rather than
                # remembered: the switch is sticky, so a second field opens on
                # whichever layer the first one was left on.
                layer = 'letters' if '?123' in words(get('layout')) else 'symbols'
                for character in text:
                    wanted = 'letters' if character.lower() in LETTERS else 'symbols'
                    if character == ' ':
                        drive('tap space', 'wait-idle')
                        continue
                    if wanted != layer:
                        drive('tap ?123' if wanted == 'symbols' else 'tap abc', 'wait-idle')
                        layer = wanted
                    drive(f'type {character}', 'wait-idle')

            try:
                panel = start()
                drive('wait-for Pair with your computer')
                pairing = capture('01-pairing')
                assert 'Sidekick' in words(pairing), words(pairing)
                checks.append('an unpaired deck says what to do about it before asking for it')

                drive('tap Enter the address', 'wait-idle')
                type_text(address)
                drive('tap Next', 'wait-idle')
                code = capture('02-code')
                assert 'pairing code' in words(code).lower(), words(code)
                drive('tap Enter the code', 'wait-idle')
                type_text('abc123')
                drive('tap Pair', 'wait-idle')
                grid = capture('03-the-deck')
                said = words(grid)
                assert 'Test' in said and 'Deploy' in said, said
                checks.append('the deck the computer sent is drawn, keys and pages')

                drive('tap Test', 'wait-idle')
                ran = capture('04-pressed')
                checks.append('a key without a confirmation runs on the first tap')

                # The computer's answer comes back on the next look rather
                # than instantly, which is the whole reason the deck says what
                # it is doing rather than leaving the key looking untouched.
                drive('wait 7000', 'wait-idle')
                finished = capture('05-finished')
                assert 'Test' in words(finished), words(finished)
                drive('tap Test', 'wait-idle')
                said = capture('06-what-it-said')
                assert 'finished on the computer' in words(said), words(said)
                checks.append('a key that has run opens what it said, not only when it failed')

                drive('tap Back to controls', 'wait-idle')
                back = capture('07-back-with-the-last-result')
                assert 'Test finished' in words(back), words(back)
                checks.append('the deck says what the last key it ran did')

                drive('tap Deploy', 'wait-idle')
                asked = capture('08-asked-first')
                assert 'Run this on the paired computer?' in words(asked), words(asked)
                drive('tap Run', 'wait-idle')
                capture('09-confirmed')
                assert any('/deck/press' in path for path in requests), requests
                checks.append('a key that asks first only runs after it is answered')

                # Closed and reopened: the deck the computer sent is on the
                # device, so the panel is not blank while the radio wakes up.
                stop()
                panel = start()
                drive('wait-for Build')
                kept = capture('10-reopened')
                assert 'Test' in words(kept), words(kept)
                checks.append('the deck is on the device, so reopening it draws it at once')

                (output/'result.json').write_text(json.dumps({
                    'status': 'passed', 'scale': args.scale,
                    'fixture': 'a-paired-command-deck',
                    'requests': requests, 'checks': checks}, indent=2)+'\n')
            finally:
                stop()
                server.shutdown()


if __name__ == '__main__':
    main()

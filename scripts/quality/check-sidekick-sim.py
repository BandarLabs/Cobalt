#!/usr/bin/env python3
"""Answer a coding agent from the armchair: pair with a fixture daemon, take a
question, answer it, take two at once and answer one of them, and find that an
answer the daemon has already lost is not reported as a decision."""
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

CODE = 'abc123'


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
    answered = []
    # What the daemon is holding, set by the journey as it goes.
    waiting = {'asks': []}
    with tempfile.TemporaryDirectory(prefix='cobalt-sidekick-', dir='/tmp') as temporary:
        private = Path(temporary)
        env = dict(os.environ, TMPDIR=str(private), CARGO_TARGET_DIR=str(target),
                   CARGO_PROFILE_DEV_DEBUG='0', CARGO_INCREMENTAL='0',
                   KOBO_SIM_PROFILE='clara-bw-391', KOBO_TEXT_SCALE=args.scale,
                   KOBO_SIM_FIXTURE='two-agents-asking', KOBO_SIM_SEED='0')
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
                assert f'token={CODE}' in self.path, self.path
                # A real daemon holds the request open until something is
                # waiting, which is what makes one poll per question rather
                # than one poll per moment.
                deadline = time.monotonic() + 3
                while not waiting['asks'] and time.monotonic() < deadline:
                    time.sleep(.05)
                asks = waiting['asks']
                if len(asks) == 1:
                    self.answer(json.dumps({'ask': asks[0]}).encode())
                else:
                    self.answer(json.dumps({'version': '4', 'asks': asks}).encode())

            def do_POST(self):
                requests.append(self.path)
                length = int(self.headers.get('Content-Length', '0'))
                sent = json.loads(self.rfile.read(length) or b'{}')
                answered.append(sent)
                # A question the daemon no longer holds is refused, which is
                # the case the panel must not report as a decision.
                held = any(ask['id'] == sent.get('id') for ask in waiting['asks'])
                if held:
                    waiting['asks'] = [ask for ask in waiting['asks']
                                       if ask['id'] != sent.get('id')]
                # The daemon's own acknowledgement: "ok" means the answer
                # reached the question. Anything else, and the panel must not
                # claim a decision nobody received.
                self.answer(json.dumps({'ok': held}).encode())

        server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        tls.load_cert_chain(config/'stream/cert.pem', config/'stream/key.pem')
        server.socket = tls.wrap_socket(server.socket, server_side=True)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        address = f'127.0.0.1:{server.server_port}'
        (private/'cobalt-sim-state/sidekick').mkdir(parents=True)
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
                                           cwd=ROOT/'examples/sidekick', env=env, stdout=log,
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
                assert metadata['app'] == 'sidekick', metadata
                return layout

            def words(layout):
                drawn = ' '.join(str(line) for node in layout['nodes'] for line in node['lines'])
                return ' '.join(drawn.split())

            LETTERS = set('abcdefghijklmnopqrstuvwxyz')

            def type_text(text):
                """Types the way a thumb does, taking the layer switch."""
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
                # The command may be wrapped across two lines at the larger
                # text sizes, which is what a long word does on a six inch
                # panel; what matters is that the screen names it.
                assert 'sidekickd init' in words(pairing).replace('- ', ''), words(pairing)
                checks.append('an unpaired reader is told what to run on the computer')

                type_text(address)
                drive('tap Next', 'wait-idle')
                code = capture('02-code')
                assert 'pairing code' in words(code).lower(), words(code)
                type_text(CODE)
                drive('tap Pair', 'wait-idle')
                watching = capture('03-watching')
                assert 'Watching' in words(watching), words(watching)
                assert address in words(watching), words(watching)
                checks.append('a paired reader says which computer it is watching')

                # One agent asks. The panel takes it from the long poll.
                waiting['asks'] = [{
                    'id': 1, 'source': 'claude', 'session': 'cobalt ab12',
                    'tool': 'Bash', 'detail': 'rm -rf target && cargo build --release',
                }]
                drive('wait-for asks', 'wait-idle')
                asked = capture('04-a-question')
                said = words(asked)
                # The command may be wrapped at the larger text sizes, which
                # is what a long command does on a six inch panel.
                assert 'cargo build' in said, said
                assert 'cobalt ab12' in said, said
                assert address in said, said
                checks.append('a question says what is being run, by which terminal, on which '
                              'computer')

                drive('tap Allow', 'wait-idle')
                allowed = capture('05-allowed')
                assert answered and answered[-1]['choice'] == 'allow', answered
                assert 'Allowed' in words(allowed), words(allowed)
                checks.append('an answer is sent and the panel says what was decided')

                # Two terminals ask at once.
                waiting['asks'] = [
                    {'id': 2, 'source': 'claude', 'session': 'cobalt ab12', 'tool': 'Bash',
                     'detail': 'cargo test --workspace'},
                    {'id': 3, 'source': 'codex', 'session': 'notes cd34', 'tool': 'shell',
                     'detail': 'git push --force-with-lease'},
                ]
                drive('wait-for Waiting questions', 'wait-idle')
                board = capture('06-two-at-once')
                said = words(board)
                assert 'cobalt ab12' in said and 'notes cd34' in said, said
                checks.append('two terminals asking at once are both on the board, each named')

                drive('tap notes', 'wait-idle')
                second = capture('07-the-second-one')
                assert 'git push --force-with-lease' in words(second), words(second)
                drive('tap Deny', 'wait-idle')
                denied = capture('08-denied')
                assert answered[-1]['id'] == 3 and answered[-1]['choice'] == 'deny', answered
                # The other terminal is still waiting, so the panel goes
                # straight to its question rather than back to a quiet screen.
                assert 'cargo test --workspace' in words(denied), words(denied)
                checks.append('answering one of them answers that one and brings up the other')

                drive('tap Leave it for the terminal', 'wait-idle')
                left = capture('09-left-for-the-terminal')
                assert 'Left' in words(left) or 'terminal' in words(left), words(left)
                checks.append('a question can be left for the terminal without deciding it')

                # A question the daemon has already lost: the panel must not
                # claim a decision nobody received.
                waiting['asks'] = [{
                    'id': 9, 'source': 'codex', 'session': 'notes cd34', 'tool': 'shell',
                    'detail': 'ship the release',
                }]
                drive('wait-for asks', 'wait-idle')
                capture('10-a-question-that-will-be-gone')
                waiting['asks'] = []
                drive('tap Allow', 'wait-idle')
                gone = capture('11-gone-before-the-answer')
                assert 'gone before the answer' in words(gone), words(gone)
                checks.append('an answer the daemon no longer holds is not reported as a decision')

                (output/'result.json').write_text(json.dumps({
                    'status': 'passed', 'scale': args.scale,
                    'fixture': 'two-agents-asking',
                    'answered': answered, 'checks': checks}, indent=2)+'\n')
            finally:
                stop()
                server.shutdown()


if __name__ == '__main__':
    main()

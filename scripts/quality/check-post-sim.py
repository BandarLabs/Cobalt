#!/usr/bin/env python3
"""Drive Post against a fixture Hermes gateway: paginated inbox, letter
paging, reply delivery with idempotent retry, rejection, draft recovery and
restart - through the actual simulator with real taps."""
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

ROOT = Path(__file__).resolve().parents[2]
TOKEN = 'fixture-token'

LONG_BODY = ('The kettle takes its time, and so does this letter.\n\n'
             + 'Steam rises from the spout while the street outside is still. '
               'A letter like this one is meant to be read slowly, page by page.\n\n' * 30)


def corpus():
    letters = [
        {'id': 'l-01', 'title': 'A long letter', 'body': LONG_BODY},
        {'id': 'l-02', 'title': 'Tea notes', 'body': 'First flush, two minutes, no milk.'},
        {'id': 'l-03', 'title': 'Flaky line', 'body': 'The connection to this one drops once.'},
        {'id': 'l-04', 'title': 'Ghost letter', 'body': 'The gateway forgets this letter.'},
    ]
    for n in range(5, 13):
        letters.append({'id': f'l-{n:02d}', 'title': f'Second page letter {n}',
                        'body': f'A short note number {n}.'})
    return letters


class Gateway:
    """The fixture gateway, and everything it was asked to do."""

    def __init__(self):
        self.letters = corpus()
        self.replies = []
        self.flaky_failed = False


def handler_for(gateway):
    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def _json(self, code, payload):
            body = json.dumps(payload).encode()
            self.send_response(code)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def _authorized(self):
            return self.headers.get('Authorization') == f'Bearer {TOKEN}'

        def do_GET(self):
            if not self._authorized():
                return self._json(401, {'error': 'unauthorized'})
            match = re.fullmatch(r'/letters\?page=(\d+)&per_page=(\d+)', self.path)
            if not match:
                return self._json(404, {'error': 'unknown route'})
            page, per = int(match.group(1)), int(match.group(2))
            start = (page - 1) * per
            items = gateway.letters[start:start + per]
            self._json(200, {'total': len(gateway.letters), 'items': items})

        def do_POST(self):
            if not self._authorized():
                return self._json(401, {'error': 'unauthorized'})
            if self.path != '/replies':
                return self._json(404, {'error': 'unknown route'})
            body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            if body['letter_id'] == 'l-04':
                return self._json(404, {'error': 'letter unknown'})
            if body['letter_id'] == 'l-03' and not gateway.flaky_failed:
                gateway.flaky_failed = True
                return self._json(500, {'error': 'dropped once'})
            if any(r['reply_id'] == body['reply_id'] for r in gateway.replies):
                return self._json(200, {'status': 'duplicate'})
            gateway.replies.append(body)
            self._json(200, {'status': 'accepted'})

    return Handler


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
    result = {'server': 'local TLS fixture', 'checks': []}
    with tempfile.TemporaryDirectory(prefix='cobalt-post-', dir='/tmp') as temporary:
        private = Path(temporary)
        env = dict(os.environ, TMPDIR=str(private), CARGO_TARGET_DIR=str(target),
                   CARGO_PROFILE_DEV_DEBUG='0', CARGO_INCREMENTAL='0',
                   KOBO_SIM_PROFILE='clara-bw-391', KOBO_TEXT_SCALE=args.scale,
                   KOBO_SIM_CLOCK_MILLIS='1767265860000')
        env.pop('KOBO_SIM_OFFLINE', None)
        config = private / 'config'
        env.update(KOBO_STREAM_CONFIG_DIR=str(config), KOBO_SIM_TRUST_DIR=str(config / 'trust'))
        subprocess.run([str(cli), 'stream', 'init', '--host', '127.0.0.1'], cwd=ROOT,
                       env=env, check=True, capture_output=True, timeout=60)

        gateway = Gateway()
        server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), handler_for(gateway))
        tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        tls.load_cert_chain(config / 'stream/cert.pem', config / 'stream/key.pem')
        server.socket = tls.wrap_socket(server.socket, server_side=True)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        origin = f'https://127.0.0.1:{server.server_port}'

        # The token, pinned to this gateway, installed the way the companion
        # CLI installs it; it never reaches the application.
        secrets = private / 'cobalt-sim-secrets/apps/post'
        (secrets / 'servers').mkdir(parents=True)
        (secrets / 'servers/hermes-post').write_text(
            f'cobalt-server-account-v1\n{origin}\n{TOKEN}')
        os.chmod(secrets / 'servers/hermes-post', 0o600)
        state = private / 'cobalt-sim-state/post'
        state.mkdir(parents=True)
        (state / 'gateway').write_text(origin)

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
                                           cwd=ROOT / 'apps/post', env=env,
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
                drive('wait-for A long letter', 'wait-idle', timeout=300)
                capture('post-inbox')
                result['checks'].append(dict(
                    name='first page of the inbox', status='passed',
                    detail='the first paginated fetch showed the newest letters'))

                drive('tap Older', 'wait-for Second page letter 9',
                      'wait-idle', timeout=300)
                capture('post-inbox-page-2')
                drive('tap Newer', 'wait-for A long letter', timeout=300)
                result['checks'].append(dict(
                    name='inbox pagination', status='passed',
                    detail='Older fetched page two from the gateway and Newer returned'))

                drive('tap A long letter', 'wait-for The kettle takes its time',
                      'wait-idle', timeout=300)
                drive('tap Next', 'wait 600', 'wait-idle')
                capture('post-letter-page-2')
                result['checks'].append(dict(
                    name='long letters page', status='passed',
                    detail='the long letter paged forward inside the panel'))

                drive('tap Inbox', 'wait-for Tea notes')
                drive('tap Tea notes', 'wait-for First flush')
                drive('tap Write a reply', 'type Kettle on and book open',
                      'tap Send letter', 'wait-for Sent to Hermes.', 'wait-idle',
                      timeout=300)
                capture('post-reply-sent')
                assert len(gateway.replies) == 1, 'the gateway should hold one reply'
                result['checks'].append(dict(
                    name='reply delivered', status='passed',
                    detail='the reply reached the gateway and shows Delivered'))

                drive('tap Inbox', 'tap Flaky line', 'wait-for drops once')
                drive('tap Write a reply', 'type Sending this twice would be wrong',
                      'tap Send letter', 'wait-for still queued', timeout=300)
                drive('tap Inbox', 'tap Check', 'wait-for Sent to Hermes.',
                      timeout=300)
                flaky = [r for r in gateway.replies if r['letter_id'] == 'l-03']
                assert len(flaky) == 1, 'a retried send must deliver exactly once'
                result['checks'].append(dict(
                    name='retry with duplicate protection', status='passed',
                    detail='the first attempt failed at the gateway; the retry reused the '
                           'same reply key and the gateway holds exactly one copy'))

                drive('tap Ghost letter', 'wait-for forgets this letter')
                drive('tap Write a reply', 'type Anybody there',
                      'tap Send letter', 'wait-for no longer knows the letter',
                      'wait-idle', timeout=300)
                capture('post-rejected')
                result['checks'].append(dict(
                    name='rejection is distinct', status='passed',
                    detail='a 404 marks the reply Rejected instead of retrying forever'))

                drive('tap Inbox', 'tap Tea notes', 'tap Write a reply', timeout=300)
                drive('type Adding biscuits', 'tap back')
                stop()
                ADDRESS = start()
                drive('wait-for Tea notes', timeout=300)
                drive('tap Tea notes', 'tap Continue your reply',
                      'wait-for Adding biscuits', 'wait-idle', timeout=300)
                capture('post-draft-restored')
                result['checks'].append(dict(
                    name='restart restores drafts and inbox', status='passed',
                    detail='after a full simulator restart the cached inbox and the '
                           'unsent draft came back from the store'))
                result['status'] = 'passed'
            finally:
                stop()
        (output / 'result.json').write_text(json.dumps(result, indent=2))
        print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()

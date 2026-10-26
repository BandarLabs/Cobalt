#!/usr/bin/env python3
"""Fetch a brief from a private HTTPS fixture, read a story from it, change the
source, and find yesterday's brief still there with no network."""
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

ARTICLE = ('<h1>A morning by the river</h1>'
           '<figure><img src="/ridge.png" alt="A mountain ridge"/>'
           '<figcaption>The hills beyond the river.</figcaption></figure>'
           '<p>The first light reaches the bridge before the town wakes.</p>'
           + '<p>Reeds bend towards the water, and the allotments are quiet.</p>' * 8).encode()


def picture():
    import struct
    import zlib
    width, height = 240, 90

    def chunk(kind, data):
        return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind+data))

    rows = bytearray()
    for y in range(height):
        rows.append(0)
        for x in range(width):
            ridge = 18 + abs(x-96)//3
            rows.append(56 if ridge <= y < ridge+3 else 255)
    return (b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', width, height, 8, 0, 0, 0, 0))
            + chunk(b'IDAT', zlib.compress(bytes(rows))) + chunk(b'IEND', b''))


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
    with tempfile.TemporaryDirectory(prefix='cobalt-brief-', dir='/tmp') as temporary:
        private = Path(temporary)
        env = dict(os.environ, TMPDIR=str(private), CARGO_TARGET_DIR=str(target),
                   CARGO_PROFILE_DEV_DEBUG='0', CARGO_INCREMENTAL='0',
                   KOBO_SIM_PROFILE='clara-bw-391', KOBO_TEXT_SCALE=args.scale,
                   KOBO_SIM_FIXTURE='original-brief-index', KOBO_SIM_SEED='0')
        config = private/'config'
        env.update(KOBO_STREAM_CONFIG_DIR=str(config), KOBO_SIM_TRUST_DIR=str(config/'trust'))
        subprocess.run([str(cli), 'stream', 'init', '--host', '127.0.0.1'], cwd=ROOT,
                       env=env, check=True, capture_output=True, timeout=60)
        drawing = picture()

        class Handler(http.server.BaseHTTPRequestHandler):
            protocol_version = 'HTTP/1.1'

            def log_message(self, *_):
                pass

            def do_GET(self):
                requests.append(self.path)
                path = self.path
                if path.endswith('stories.json'):
                    listed = [11, 12, 13] if 'askstories' in path else [1, 2, 3, 4, 5, 6]
                    body = json.dumps(listed).encode()
                    kind = 'application/json'
                elif path.startswith('/v0/item/'):
                    number = int(path.split('/')[3].split('.')[0])
                    if number >= 11:
                        story = {'title': f'Ask HN: question {number}',
                                 'text': f'<p>The {number}th question, with no link of its own.</p>'}
                    else:
                        story = {'title': f'Story {number}: a morning by the river',
                                 'url': f'https://127.0.0.1:{server.server_port}/article-{number}.html'}
                    body = json.dumps(story).encode()
                    kind = 'application/json'
                elif path.startswith('/article'):
                    body = ARTICLE
                    kind = 'text/html'
                elif path == '/ridge.png':
                    body = drawing
                    kind = 'image/png'
                else:
                    self.send_error(404)
                    return
                self.send_response(200)
                self.send_header('Content-Type', kind)
                self.send_header('Content-Length', str(len(body)))
                self.end_headers()
                self.wfile.write(body)

        server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        tls.load_cert_chain(config/'stream/cert.pem', config/'stream/key.pem')
        server.socket = tls.wrap_socket(server.socket, server_side=True)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        env['KOBO_BRIEF_ORIGIN'] = f'https://127.0.0.1:{server.server_port}'
        (private/'cobalt-sim-state/brief').mkdir(parents=True)
        log_path = output/'simulator.log'
        checks = []
        with log_path.open('w') as log:
            def stop():
                nonlocal process
                if process and process.poll() is None:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait(timeout=5)
                process = None

            def start(offline=False):
                nonlocal process
                log.seek(0)
                log.truncate()
                started = dict(env, KOBO_SIM_OFFLINE='1') if offline else env
                process = subprocess.Popen([str(cli), 'dev', '127.0.0.1:0'],
                                           cwd=ROOT/'examples/brief', env=started, stdout=log,
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
                command = [str(cli), 'drive', '--address', address, '--ideal', '--shots', str(output)]
                for step in steps:
                    command.extend(['--step', step])
                subprocess.run(command, cwd=ROOT, env=env, stdout=log, stderr=log,
                               check=True, timeout=90)

            def get(endpoint):
                with urllib.request.urlopen(f'http://{address}/{endpoint}', timeout=5) as answer:
                    return json.load(answer)

            def capture(name, picture=None):
                drive('wait-idle', 'shot '+name)
                diagnostics = get('diagnostics')
                assert not [i for i in diagnostics['issues'] if i['severity'] == 'error'], diagnostics
                layout = get('layout')
                (output/(name+'.layout.json')).write_text(json.dumps(layout, indent=2)+'\n')
                if picture is not None:
                    drawn = any('Picture(' in node['kind'] for node in layout['nodes'])
                    assert drawn == picture, f'{name}: picture drawn={drawn}'
                metadata = json.loads((output/(name+'.json')).read_text())
                assert metadata['app'] == 'brief', metadata
                return layout

            def words(layout):
                drawn = ' '.join(str(line) for node in layout['nodes'] for line in node['lines'])
                return ' '.join(drawn.split())

            try:
                address = start()
                drive('wait-for-id refresh')
                empty = capture('01-nothing-yet')
                assert 'not fetched yet' in words(empty), words(empty)
                assert not requests, 'the brief called the index unasked'
                checks.append('a brief with nothing in it says so and asks for nothing')

                drive('tap-id refresh')
                fetched = capture('02-fetched')
                assert re.search(r'as of \d{4}-\d{2}-\d{2} \d{2}:\d{2} UTC', words(fetched)), words(fetched)
                assert 'Top stories' in words(fetched), words(fetched)
                assert requests.count('/v0/topstories.json') == 1, requests
                checks.append('the brief says which list it is and when it was fetched')

                drive('tap Story 1')
                story = capture('03-reading-a-story', picture=True)
                assert 'the bridge before the town wakes' in words(story), words(story)
                assert any(path.startswith('/article-1') for path in requests), requests
                checks.append('a story opens in the shared reader with its picture')

                # Closed and reopened with no network at all: the brief and the
                # story that was read are both still here.
                stop()
                before = len(requests)
                address = start(offline=True)
                drive('wait-for-id refresh')
                offline = capture('04-offline-brief')
                assert 'as of' in words(offline), words(offline)
                drive('tap Story 1')
                saved = capture('05-offline-story', picture=True)
                assert 'the bridge before the town wakes' in words(saved), words(saved)
                assert len(requests) == before, requests[before:]
                checks.append('the brief and a read story reopen with no network')

                # A refresh that cannot happen keeps what is on the panel and
                # offers another attempt.
                drive('tap Back', 'tap-id refresh')
                failed = capture('06-refresh-failed')
                assert 'already saved' in words(failed), words(failed)
                assert 'Try again' in words(failed), words(failed)
                assert len(requests) == before, requests[before:]
                checks.append('a refresh with no network keeps the brief and offers another')

                stop()
                address = start()
                drive('wait-for-id refresh', 'tap-id sources')
                sources = capture('07-sources')
                assert 'Showing this now' in words(sources), words(sources)
                drive('tap Ask Hacker News')
                chosen = capture('08-source-chosen')
                assert 'Ask Hacker News · not fetched yet' in words(chosen), words(chosen)
                drive('tap-id refresh')
                asked = capture('09-questions')
                assert 'Ask HN: question 11' in words(asked), words(asked)
                assert requests.count('/v0/askstories.json') == 1, requests
                checks.append('another list is chosen, fetched and labelled as itself')

                drive('tap Ask HN: question 11')
                question = capture('10-question', picture=False)
                assert 'no link of its own' in words(question), words(question)
                checks.append('a question is read from what the poster wrote')

                (output/'result.json').write_text(json.dumps({
                    'status': 'passed', 'scale': args.scale,
                    'fixture': 'original-brief-index',
                    'requests': requests, 'checks': checks}, indent=2)+'\n')
            finally:
                stop()
                server.shutdown()


if __name__ == '__main__':
    main()

#!/usr/bin/env python3
"""Fetch a brief from a private HTTPS fixture, find every story behind it kept
on the device, read one with the network gone, watch the brief refresh itself
when it is stale and when the morning wake comes, and change the source."""
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


# 18:30 on 1 November 2026 in Sydney, ten hours ahead of UTC. The simulator
# starts its clock here and passes the offset to the app, so the "as of" line
# has one right answer and it is not the UTC one.
START_MILLIS = 1_793_521_800_000
OFFSET_MINUTES = 600
SEVEN_HOURS = 7 * 3600 * 1000


def certificates(config):
    """A private certificate authority and a server certificate for 127.0.0.1.

    Made here rather than with `kobo stream init`, which now looks for a reader
    on the local network to install its trust root on, and a check that runs on
    a developer's desk must never reach the reader sitting next to it.
    """
    trust = config/'trust'
    stream = config/'stream'
    trust.mkdir(parents=True)
    stream.mkdir(parents=True)

    def openssl(*arguments):
        subprocess.run(['openssl', *arguments], check=True, capture_output=True, timeout=60)

    extensions = config/'server.ext'
    extensions.write_text('basicConstraints=CA:FALSE\n'
                          'subjectAltName=IP:127.0.0.1\n'
                          'extendedKeyUsage=serverAuth\n')
    openssl('req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '2', '-subj',
            '/CN=Brief fixture authority', '-keyout', str(config/'root.key'),
            '-out', str(trust/'root.pem'), '-addext', 'basicConstraints=critical,CA:TRUE',
            '-addext', 'keyUsage=critical,keyCertSign')
    openssl('req', '-newkey', 'rsa:2048', '-nodes', '-subj', '/CN=127.0.0.1',
            '-keyout', str(stream/'key.pem'), '-out', str(config/'server.csr'))
    openssl('x509', '-req', '-in', str(config/'server.csr'), '-CA', str(trust/'root.pem'),
            '-CAkey', str(config/'root.key'), '-CAcreateserial', '-days', '2',
            '-extfile', str(extensions), '-out', str(stream/'cert.pem'))


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
                   KOBO_SIM_FIXTURE='original-brief-index', KOBO_SIM_SEED='0',
                   KOBO_SIM_CLOCK_MILLIS=str(START_MILLIS),
                   KOBO_UTC_OFFSET_MINUTES=str(OFFSET_MINUTES),
                   KOBO_SIM_UTC_OFFSET_MINUTES=str(OFFSET_MINUTES),
                   # The default model has no power ownership, so a scheduled
                   # wake would be refused as unsupported. A reader has it.
                   KOBO_SIM_BACKENDS='network,battery-read,frontlight-control,wifi-control,'
                                     'cover-sensor,library,scheduled-wake')
        config = private/'config'
        env.update(KOBO_SIM_TRUST_DIR=str(config/'trust'))
        certificates(config)
        drawing = picture()

        class Handler(http.server.BaseHTTPRequestHandler):
            protocol_version = 'HTTP/1.1'

            def log_message(self, *_):
                pass

            def do_GET(self):
                requests.append(self.path)
                path = self.path
                if path.endswith('stories.json'):
                    listed = [11, 12, 13] if 'askstories' in path else list(range(1, 15))
                    body = json.dumps(listed).encode()
                    kind = 'application/json'
                elif path.startswith('/v0/item/'):
                    number = int(path.split('/')[3].split('.')[0])
                    if 11 <= number <= 13 and 'askstories' in ' '.join(requests):
                        story = {'id': number, 'title': f'Ask HN: question {number}',
                                 'text': f'<p>The {number}th question, with no link of its own.</p>'}
                    else:
                        story = {'id': number,
                                 'title': f'Story {number}: a morning by the river',
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

            def start(offline=False, later=0):
                nonlocal process
                log.seek(0)
                log.truncate()
                started = dict(env, KOBO_SIM_OFFLINE='1') if offline else dict(env)
                started['KOBO_SIM_CLOCK_MILLIS'] = str(START_MILLIS + later)
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
                assert 'as of 2026-11-01 18:30' in words(fetched), words(fetched)
                assert 'UTC' not in words(fetched), words(fetched)
                assert 'Top stories' in words(fetched), words(fetched)
                assert requests.count('/v0/topstories.json') == 1, requests
                checks.append('the brief says which list it is and when, on the reader\'s clock')
                items = [path for path in requests if path.startswith('/v0/item/')]
                assert len(items) == 12, items
                assert re.search(r'\b1 of [23]\b', words(fetched)), words(fetched)
                checks.append('a brief holds twelve stories, paged')

                # Every story behind a headline is fetched and kept as the brief
                # lands, without anybody opening it.
                drive('wait-for Ready offline: 12 stories')
                kept = capture('03-kept-offline')
                assert 'Ready offline: 12 stories' in words(kept), words(kept)
                fetched_articles = {path for path in requests if path.startswith('/article')}
                assert len(fetched_articles) == 12, sorted(fetched_articles)
                checks.append('every story in the brief is kept on the device with its picture')

                before = len(requests)
                drive('tap Story 1')
                story = capture('04-reading-a-story', picture=True)
                assert 'the bridge before the town wakes' in words(story), words(story)
                assert len(requests) == before, requests[before:]
                checks.append('a story opens from the device, with its picture, asking for nothing')

                # Closed and reopened with no network at all: the brief and a
                # story nobody opened yet are both still here.
                stop()
                before = len(requests)
                address = start(offline=True)
                drive('wait-for-id refresh')
                offline = capture('05-offline-brief')
                assert 'as of 2026-11-01 18:30' in words(offline), words(offline)
                drive('tap Story 2')
                saved = capture('06-offline-story', picture=True)
                assert 'the bridge before the town wakes' in words(saved), words(saved)
                assert len(requests) == before, requests[before:]
                checks.append('a story nobody opened reads with no network')

                # A refresh that cannot happen keeps what is on the panel and
                # offers another attempt.
                drive('tap Back', 'tap-id refresh')
                failed = capture('07-refresh-failed')
                assert 'already saved' in words(failed), words(failed)
                assert 'not on a network' in words(failed), words(failed)
                assert 'brought back nothing' not in words(failed), words(failed)
                assert 'Try again' in words(failed), words(failed)
                assert len(requests) == before, requests[before:]
                checks.append('a refresh with no network keeps the brief and offers another')

                # Seven hours later the brief is stale, and opening the app is
                # enough to fetch a new one.
                stop()
                address = start(later=SEVEN_HOURS)
                drive('wait-for-id refresh', 'wait-for as of 2026-11-02 01:30')
                stale = capture('08-refreshed-when-stale')
                assert requests.count('/v0/topstories.json') == 2, requests
                assert 'as of 2026-11-02 01:30' in words(stale), words(stale)
                checks.append('a brief more than six hours old refreshes itself when opened')

                # The morning wake the brief asked for refreshes it with nobody
                # touching anything.
                drive('clock advance 86400000', 'wait-for-id refresh')
                capture('09-after-morning-wake')
                assert requests.count('/v0/topstories.json') == 3, requests
                checks.append('the morning wake refreshes the brief on its own')

                # Restarted a little after that refresh, so the brief is fresh
                # and nothing is fetched until a new source is chosen.
                stop()
                address = start(later=SEVEN_HOURS + 120_000)
                drive('wait-for-id refresh', 'tap-id sources')
                sources = capture('09b-sources')
                # The list showing now carries the runtime's own mark in the
                # option list, and it is the only one that does.
                marked = [node['action'] for node in sources['nodes']
                          if node['kind'].startswith('OptionRow(') and node['kind'].endswith(', true)')]
                named = {node['action']: ' '.join(node['lines']) for node in sources['nodes']
                         if node['kind'] == 'OptionLabel'}
                assert [named.get(action) for action in marked] == ['Top stories'], marked
                drive('tap Ask Hacker News')
                chosen = capture('10-source-chosen')
                assert 'Ask Hacker News · not fetched yet' in words(chosen), words(chosen)
                drive('tap-id refresh')
                asked = capture('11-questions')
                assert 'Ask HN: question 11' in words(asked), words(asked)
                assert requests.count('/v0/askstories.json') == 1, requests
                checks.append('another list is chosen, fetched and labelled as itself')

                drive('tap Ask HN: question 11')
                question = capture('12-question', picture=False)
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

#!/usr/bin/env python3
"""Read Hacker News from a private HTTPS fixture: open a discussion, read the
article behind it, put the story aside, and find it on the saved list after a
restart with the radio off."""
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

# Deep on purpose: the reply at depth six is past the indent cap, which is
# where a thread either says how deep it is in words or silently pretends the
# conversation is flatter than it was.
THREAD = [
    (2, 1, 'alice', 'The measurement in the third table is the whole result.', [3]),
    (3, 2, 'bob', 'It is, and the error bars are wider than the effect.', [4]),
    (4, 3, 'carol', 'Wider in the preprint. The published version narrows them.', [5]),
    (5, 4, 'dan', 'Which is the version everybody is arguing about.', [6]),
    (6, 5, 'erin', 'Only because the preprint was the one on the front page.', [7]),
    (7, 6, 'frank', 'Six deep and still about the error bars.', []),
]

ARTICLE = (b'<h1>A quieter way to measure the sky</h1>'
           b'<p>The array was rebuilt over a winter, one dish at a time.</p>'
           + b'<p>Each dish is aligned against the same star, which takes a night.</p>' * 6)


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
    with tempfile.TemporaryDirectory(prefix='cobalt-hn-', dir='/tmp') as temporary:
        private = Path(temporary)
        env = dict(os.environ, TMPDIR=str(private), CARGO_TARGET_DIR=str(target),
                   CARGO_PROFILE_DEV_DEBUG='0', CARGO_INCREMENTAL='0',
                   KOBO_SIM_PROFILE='clara-bw-391', KOBO_TEXT_SCALE=args.scale,
                   KOBO_SIM_FIXTURE='hacker-news-front-page', KOBO_SIM_SEED='0')
        config = private/'config'
        env.update(KOBO_STREAM_CONFIG_DIR=str(config), KOBO_SIM_TRUST_DIR=str(config/'trust'))
        subprocess.run([str(cli), 'stream', 'init', '--host', '127.0.0.1'], cwd=ROOT,
                       env=env, check=True, capture_output=True, timeout=60)

        def item(number):
            """One item, in the shape the site's own API answers with."""
            if number == 1:
                return {'id': 1, 'type': 'story', 'by': 'submitter', 'time': 1_760_000_000,
                        'title': 'A quieter way to measure the sky, rebuilt over one winter '
                                 'by a team of four',
                        'url': f'https://127.0.0.1:{server.server_port}/article.html',
                        'score': 214, 'descendants': len(THREAD), 'kids': [2]}
            if number == 90:
                return {'id': 90, 'type': 'story', 'by': 'asker', 'time': 1_760_000_100,
                        'title': 'Ask HN: what do you read on an e-reader?',
                        'text': 'Papers, mostly. Anything with tables in it.',
                        'score': 12, 'descendants': 0, 'kids': []}
            for identifier, _, who, text, kids in THREAD:
                if identifier == number:
                    return {'id': identifier, 'type': 'comment', 'by': who,
                            'time': 1_760_000_200, 'text': text, 'kids': kids}
            return {'id': number, 'type': 'story', 'by': 'someone', 'time': 1_760_000_300,
                    'title': f'Story number {number}', 'url': 'https://example.com/x',
                    'score': 10, 'descendants': 0, 'kids': []}

        class Handler(http.server.BaseHTTPRequestHandler):
            protocol_version = 'HTTP/1.1'

            def log_message(self, *_):
                pass

            def do_GET(self):
                requests.append(self.path)
                path = self.path.split('?')[0]
                if path.endswith('stories.json'):
                    body = json.dumps([1, 90, 50, 51, 52, 53, 54, 55]).encode()
                    kind = 'application/json'
                elif path.startswith('/v0/item/'):
                    number = int(path.split('/')[3].split('.')[0])
                    body = json.dumps(item(number)).encode()
                    kind = 'application/json'
                elif path.startswith('/article'):
                    body = ARTICLE
                    kind = 'text/html'
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
        env['KOBO_HN_ORIGIN'] = f'https://127.0.0.1:{server.server_port}'
        (private/'cobalt-sim-state/hn').mkdir(parents=True)
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
                # Emptied first: the address is found by reading this log, and
                # a second simulator would otherwise be driven at the port the
                # first one had.
                log.seek(0)
                log.truncate()
                started = dict(env, KOBO_SIM_OFFLINE='1') if offline else env
                process = subprocess.Popen([str(cli), 'dev', '127.0.0.1:0'],
                                           cwd=ROOT/'examples/hn', env=started, stdout=log,
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
                command = [str(cli), 'drive', '--address', address, '--ideal',
                           '--shots', str(output)]
                for step in steps:
                    command.extend(['--step', step])
                subprocess.run(command, cwd=ROOT, env=env, stdout=log, stderr=log,
                               check=True, timeout=90)

            def get(endpoint):
                with urllib.request.urlopen(f'http://{address}/{endpoint}', timeout=5) as answer:
                    return json.load(answer)

            def capture(name):
                drive('wait-idle', 'shot '+name)
                diagnostics = get('diagnostics')
                errors = [i for i in diagnostics['issues'] if i['severity'] == 'error']
                assert not errors, f'{name}: {errors}'
                layout = get('layout')
                (output/(name+'.layout.json')).write_text(json.dumps(layout, indent=2)+'\n')
                metadata = json.loads((output/(name+'.json')).read_text())
                assert metadata['app'] == 'hn', metadata
                return layout

            def words(layout):
                drawn = ' '.join(str(line) for node in layout['nodes'] for line in node['lines'])
                return ' '.join(drawn.split())

            try:
                address = start()
                drive('wait-for A quieter way')
                listed = capture('01-front-page')
                assert '214 points' in words(listed), words(listed)
                checks.append('the front page arrives in the order the site gave it')

                drive('tap A quieter way')
                thread = capture('02-discussion')
                said = words(thread)
                # The headline is in the bar and nowhere else on the panel.
                assert said.count('A quieter way') == 1, said
                assert 'Domain' in said and '214 points' in said, said
                assert 'the third table' in said, said
                checks.append('the discussion says the headline once, in the bar')

                deep = said
                page = 0
                while 'Six deep' not in deep and page < 6:
                    drive('tap-id thread-next')
                    deep = words(capture(f'03-thread-page-{page + 2}'))
                    page += 1
                assert 'Six deep' in deep, deep
                checks.append('a reply six levels down is reachable and says how deep it is')

                drive('tap-id read-article')
                article = capture('04-article')
                assert 'rebuilt over a winter' in words(article), words(article)
                assert any(path.startswith('/article') for path in requests), requests
                checks.append('the article behind a story is read on the device')

                drive('tap back', 'wait-idle')
                back = capture('05-back-to-the-discussion')
                assert 'Six deep' in words(back) or 'the third table' in words(back), words(back)
                checks.append('leaving the article lands back in the discussion')

                drive('tap-id save')
                saved = capture('06-saved')
                assert 'Saved' in words(saved), words(saved)
                checks.append('a story is put aside from its own screen')

                drive('tap back', 'wait-idle')
                marked = capture('07-list-marks-what-was-read')
                assert 'Read, saved' in words(marked), words(marked)
                checks.append('the list says which story was read and which was put aside')

                # Closed, reopened with no network at all: the saved list is
                # the one that still works, and the article with it.
                stop()
                before = len(requests)
                address = start(offline=True)
                drive('wait-for Top', 'tap Saved', 'wait-idle')
                offline = capture('08-saved-offline')
                assert 'A quieter way' in words(offline), words(offline)
                drive('tap A quieter way', 'wait-idle', 'tap-id read-article')
                kept = capture('09-article-offline')
                assert 'rebuilt over a winter' in words(kept), words(kept)
                assert len(requests) == before, requests[before:]
                checks.append('the saved story and its article reopen with no network')

                drive('tap back', 'wait-idle', 'tap back', 'wait-idle')
                drive('tap-id saved-menu-0')
                menu = capture('10-saved-menu')
                assert 'Take off this list' in words(menu), words(menu)
                drive('tap Take off this list', 'wait-idle')
                emptied = capture('11-nothing-saved')
                assert 'Nothing saved yet' in words(emptied), words(emptied)
                checks.append('a saved story can be taken off the list, and the empty list says how to fill it')

                (output/'result.json').write_text(json.dumps({
                    'status': 'passed', 'scale': args.scale,
                    'fixture': 'hacker-news-front-page',
                    'requests': requests, 'checks': checks}, indent=2)+'\n')
            finally:
                stop()
                server.shutdown()


if __name__ == '__main__':
    main()

#!/usr/bin/env python3
"""Drive the Miniflux reader against a fixture account: fetch, read, mutate,
reconnect and restart, through the actual simulator with real taps."""
import argparse
import hashlib
import http.server
import json
import os
from pathlib import Path
import re
import signal
import ssl
import struct
import subprocess
import tempfile
import threading
import time
import urllib.parse
import urllib.request
import zlib

ROOT = Path(__file__).resolve().parents[2]
TOKEN = 'fixture-token'


def ridge_picture():
    """An original test picture: a ridge line under an empty sky."""
    width, height = 320, 120

    def chunk(kind, data):
        return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind+data))

    rows = bytearray()
    for y in range(height):
        rows.append(0)
        for x in range(width):
            ridge = 22 + abs(x-130)//3
            rows.append(48 if ridge <= y < ridge+3 else 255)
    return (b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', width, height, 8, 0, 0, 0, 0))
            + chunk(b'IDAT', zlib.compress(bytes(rows))) + chunk(b'IEND', b''))


def article_body(origin):
    return ('<h2>Along the river</h2>'
            f'<figure><img src="{origin}/ridge.png" alt="A mountain ridge"/>'
            '<figcaption>The hills beyond the river.</figcaption></figure>'
            '<p>The first light reaches the bridge before the town wakes.</p>'
            + '<p>Reeds bend towards the water. Beyond the footpath the allotments are quiet, '
              'and the old stone steps are cold enough to notice.</p>' * 6)


class Account:
    """The fixture Miniflux account, and everything it was asked to do."""

    def __init__(self, origin):
        self.origin = origin
        self.requests = []
        self.drop_next_update = False
        self.entries = {
            7: {'id': 7, 'title': 'A morning by the river', 'content': article_body(origin),
                'starred': False, 'status': 'unread', 'url': f'{origin}/articles/7',
                'feed': {'title': 'The Field Journal'}, 'published_at': 3},
            8: {'id': 8, 'title': 'A note from the allotments', 'content': '<p>Only the first line.</p>',
                'starred': False, 'status': 'unread', 'url': f'{origin}/articles/8',
                'feed': {'title': 'The Field Journal'}, 'published_at': 2},
            9: {'id': 9, 'title': 'An evening walk, read last week', 'content': '<p>Read on another device.</p>',
                'starred': True, 'status': 'read', 'url': f'{origin}/articles/9',
                'feed': {'title': 'The Field Journal'}, 'published_at': 1},
        }
        self.feeds = []

    def listing(self, query):
        wanted = urllib.parse.parse_qs(query)
        limit = int(wanted['limit'][0])
        entries = sorted(self.entries.values(), key=lambda e: -e['published_at'])
        if 'status' in wanted:
            entries = [e for e in entries if e['status'] == wanted['status'][0]]
        if 'starred' in wanted:
            entries = [e for e in entries if e['starred'] == (wanted['starred'][0] == 'true')]
        return {'total': len(entries), 'entries': [
            {k: v for k, v in e.items() if k != 'published_at'} for e in entries[:limit]]}

    def updates(self, body):
        for id in body['entry_ids']:
            entry = self.entries[id]
            if 'status' in body:
                entry['status'] = body['status']
            if 'starred' in body:
                entry['starred'] = body['starred']


def handler_for(account):
    class Handler(http.server.BaseHTTPRequestHandler):
        protocol_version = 'HTTP/1.1'

        def log_message(self, *_):
            pass

        def record(self, method, body=None):
            account.requests.append({'method': method, 'path': self.path,
                                     'token': self.headers.get('X-Auth-Token'),
                                     'authorization': self.headers.get('Authorization'),
                                     'body': body})

        def reply(self, payload, kind='application/json'):
            body = payload if isinstance(payload, bytes) else json.dumps(payload).encode()
            self.send_response(200)
            self.send_header('Content-Type', kind)
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):
            self.record('GET')
            path, _, query = self.path.partition('?')
            if path == '/ridge.png':
                self.reply(PICTURE, 'image/png')
            elif path == '/v1/entries':
                self.reply(account.listing(query))
            elif path == '/v1/categories':
                self.reply([{'id': 3, 'title': 'Reading'}])
            elif path.endswith('/fetch-content'):
                id = int(path.split('/')[3])
                self.reply({'content': '<h2>All of it</h2><p>Every paragraph of the page, '
                                       'fetched from the site rather than the feed.</p>'})
            else:
                self.send_error(404)

        def do_PUT(self):
            body = json.loads(self.rfile.read(int(self.headers['Content-Length'])) or b'{}')
            self.record('PUT', body)
            if self.path != '/v1/entries':
                self.send_error(404)
                return
            account.updates(body)
            if account.drop_next_update:
                # Applied, and then the reply never arrives. This is the case
                # the queue exists for: the Kobo cannot tell it from a change
                # that never landed at all.
                account.drop_next_update = False
                self.close_connection = True
                return
            self.reply({'ok': True})

        def do_POST(self):
            body = json.loads(self.rfile.read(int(self.headers['Content-Length'])) or b'{}')
            self.record('POST', body)
            if self.path != '/v1/feeds':
                self.send_error(404)
                return
            account.feeds.append(body['feed_url'])
            self.reply({'feed_id': 12})
    return Handler


PICTURE = ridge_picture()


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
    with tempfile.TemporaryDirectory(prefix='cobalt-miniflux-', dir='/tmp') as temporary:
        private = Path(temporary)
        env = dict(os.environ, TMPDIR=str(private), CARGO_TARGET_DIR=str(target),
                   CARGO_PROFILE_DEV_DEBUG='0', CARGO_INCREMENTAL='0',
                   KOBO_SIM_PROFILE='clara-bw-391', KOBO_TEXT_SCALE=args.scale,
                   KOBO_SIM_FIXTURE='original-miniflux-account', KOBO_SIM_SEED='0')
        config = private/'config'
        env.update(KOBO_STREAM_CONFIG_DIR=str(config), KOBO_SIM_TRUST_DIR=str(config/'trust'))
        subprocess.run([str(cli), 'stream', 'init', '--host', '127.0.0.1'], cwd=ROOT,
                       env=env, check=True, capture_output=True, timeout=60)

        account = Account('https://127.0.0.1:0')
        server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), handler_for(account))
        tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        tls.load_cert_chain(config/'stream/cert.pem', config/'stream/key.pem')
        server.socket = tls.wrap_socket(server.socket, server_side=True)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        origin = f'https://127.0.0.1:{server.server_port}'
        account.__init__(origin)

        # The token and the one server it may be sent to are installed the way
        # the computer-side CLI installs them, and never reach the application.
        secrets = private/'cobalt-sim-secrets/apps/rss-miniflux'
        (secrets/'servers').mkdir(parents=True)
        (secrets/'servers/miniflux').write_text(f'cobalt-server-account-v1\n{origin}\n{TOKEN}')
        os.chmod(secrets/'servers/miniflux', 0o600)

        state = private/'cobalt-sim-state/rss-miniflux'
        shelf = private/'cobalt-sim-data/rss-miniflux'
        state.mkdir(parents=True)
        shelf.mkdir(parents=True)
        (state/'config').write_text(f'{origin}\nminiflux')
        batch_key = hashlib.sha256(f'miniflux-articles-v1:{origin}\nminiflux'.encode()).hexdigest()
        image_key = hashlib.sha256(f'rss-image:{origin}/ridge.png'.encode()).hexdigest()
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
                # A reader who opens the application away from Wi-Fi is offline
                # before the first line of it runs, which a scenario applied
                # after startup cannot reproduce: the saved batch and the
                # unsent change are read in the first milliseconds.
                nonlocal process
                log.seek(0)
                log.truncate()
                started = dict(env, KOBO_SIM_OFFLINE='1') if offline else env
                process = subprocess.Popen([str(cli), 'dev', '127.0.0.1:0'],
                                           cwd=ROOT/'apps/rss-miniflux', env=started, stdout=log,
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
                assert metadata['app'] == 'rss-miniflux', metadata
                assert metadata['fonts'] and len(metadata['source']['binarySha256']) == 64
                return layout

            def open_menu(title):
                # By what the row says rather than by its place in the batch:
                # the mark beside an article is what a reader taps, and its
                # identifier moves as articles are read, starred and synced.
                layout = get('layout')
                rows = [n for n in layout['nodes'] if n['kind'] == 'RowTitle'
                        and any(title in str(line) for line in n['lines'])]
                assert rows, f'no row says {title!r}: {layout}'
                band = rows[0]['centre']['y']
                marks = [n for n in layout['nodes'] if n['kind'].startswith('RowMenu')
                         and n['y'] <= band < n['y']+n['height']]
                assert marks, f'no menu mark beside {title!r}: {layout}'
                drive(f"tap-at {marks[0]['centre']['x']},{marks[0]['centre']['y']}")

            def says(layout, text):
                # Joined, because a sentence on the panel is wrapped into as
                # many lines as it needs and no assertion should know how many.
                drawn = ' '.join(str(line) for node in layout['nodes'] for line in node['lines'])
                return text in ' '.join(drawn.split())

            def paths(method=None):
                return [r['path'] for r in account.requests if method in (None, r['method'])]

            try:
                address = start()

                # A reader who has installed the token and typed the address in,
                # opening the application for the first time.
                drive('wait-for-id sync')
                first = capture('01-before-the-first-sync')
                assert says(first, 'No unread articles'), first
                assert not account.requests, 'the application called Miniflux unasked'

                drive('tap-id sync')
                listed = capture('02-unread-after-sync')
                assert says(listed, 'A morning by the river'), listed
                assert not says(listed, 'An evening walk'), 'a read article was listed as unread'
                queries = paths('GET')
                assert len(queries) == 3, queries
                assert 'status=unread' in queries[0] and 'starred=true' in queries[1] \
                    and 'status=read' in queries[2], queries
                assert all(r['token'] == TOKEN for r in account.requests), 'the token was not attached'
                assert all(r['authorization'] is None for r in account.requests)
                pointer = (state/batch_key).read_text()
                slot, digest = pointer.split(':')
                saved = (shelf/(batch_key[:60]+'.'+slot)).read_bytes()
                assert hashlib.sha256(saved).hexdigest() == digest
                assert b'An evening walk' in saved, 'the merged batch lost the History tab'
                checks.append('three lists merged into one saved batch')

                # Reading one, with the picture the feed only named.
                drive('tap-id article-0')
                reading = capture('03-reading-with-its-picture', picture=True)
                assert says(reading, 'the bridge before the town wakes'), reading
                assert '/ridge.png' in paths('GET'), paths('GET')
                assert (state/image_key).exists(), 'the picture was not saved'
                images = paths('GET').count('/ridge.png')
                checks.append('article picture fetched once and saved')

                # Where the reader left off, kept and returned to.
                drive('tap-at 900,700')
                second = capture('04-second-page')
                assert says(second, '2 of 2'), second
                assert (state/'reading-v1').exists(), 'the reading position was not saved'
                drive('tap Back', 'tap-id history', 'tap A morning by the river')
                resumed = capture('05-resumed-where-it-was')
                assert says(resumed, '2 of 2'), 'the article did not reopen where it was left'
                checks.append('reading position kept and returned to')

                drive('tap Back', 'tap-id unread')
                after = capture('06-read-and-queued')
                assert not says(after, 'A morning by the river'), 'a read article stayed in Unread'
                assert account.entries[7]['status'] == 'read', account.entries[7]
                assert {'entry_ids': [7], 'status': 'read'} in [
                    r['body'] for r in account.requests if r['method'] == 'PUT']
                checks.append('opening an article marks it read on the server')

                drive('tap-id history')
                history = capture('07-history')
                assert says(history, 'A morning by the river'), history

                # Starring from the row menu, and seeing it in its tab.
                open_menu('A morning by the river')
                capture('08-row-menu')
                drive('tap-id star')
                capture('09-starred-locally')
                assert account.entries[7]['starred'] is True, account.entries[7]
                drive('tap-id starred')
                starred = capture('10-starred-tab')
                assert says(starred, 'A morning by the river'), starred
                checks.append('star reaches the account and shows in its tab')

                # Away from Wi-Fi: the change is kept, and says so.
                sent_before = len(paths('PUT'))
                drive('scenario offline')
                open_menu('A morning by the river')
                drive('tap-id unstar')
                offline = capture('11-change-waiting-offline')
                assert says(offline, 'is kept and goes out on the next sync'), offline
                assert len(paths('PUT')) == sent_before, 'a change reached the server while offline'
                assert account.entries[7]['starred'] is True, 'the server was changed while offline'
                checks.append('an offline change is kept and reported')

                # Closed and reopened, still offline: everything is still here.
                stop()
                requests_before = len(account.requests)
                address = start(offline=True)
                drive('wait-for-id sync', 'tap-id starred')
                restarted = capture('12-reopened-offline')
                assert says(restarted, 'is kept and goes out on the next sync'), restarted
                assert not says(restarted, 'A morning by the river'), \
                    'the unsent unstar was forgotten across a restart'
                assert says(restarted, 'An evening walk, read last week'), \
                    'the saved batch did not reopen offline'
                assert len(account.requests) == requests_before, 'a restart reached Miniflux'
                assert account.entries[7]['starred'] is True, 'the server changed while offline'
                checks.append('saved batch and unsent change survive a restart offline')

                # Back on Wi-Fi: the change goes out before the sync.
                stop()
                address = start()
                drive('wait-for-id sync', 'tap-id sync')
                caught_up = capture('13-caught-up')
                assert account.entries[7]['starred'] is False, account.entries[7]
                assert not says(caught_up, 'waiting for Miniflux'), caught_up
                assert paths()[requests_before] == '/v1/entries', paths()[requests_before:]
                checks.append('unsent changes are flushed before the next sync')

                # A change that was applied and whose reply never arrived.
                account.drop_next_update = True
                drive('tap-id unread')
                open_menu('A note from the allotments')
                drive('tap-id star')
                unanswered = capture('14-answer-never-came')
                assert says(unanswered, 'Miniflux did not answer'), unanswered
                assert says(unanswered, 'is kept and goes out on the next sync'), unanswered
                assert account.entries[8]['starred'] is True, 'the fixture did not apply the change'
                drive('tap-id sync')
                recovered = capture('15-sent-again-safely')
                assert not says(recovered, 'Miniflux did not answer'), recovered
                assert account.entries[8]['starred'] is True, 'sending again undid the change'
                checks.append('a lost reply is re-sent as the same end state')

                # The article the feed only summarised.
                drive('tap-id starred')
                open_menu('A note from the allotments')
                drive('tap-id full-article')
                full = capture('16-full-article-fetched')
                assert says(full, 'The full article is here'), full
                assert any('fetch-content' in path for path in paths('GET')), paths('GET')
                pointer = (state/batch_key).read_text()
                saved = (shelf/(batch_key[:60]+'.'+pointer.split(':')[0])).read_bytes()
                assert b'Every paragraph of the page' in saved, 'the full article was not saved'
                checks.append('a fetched article replaces its summary and is saved')

                stop()
                requests_before = len(account.requests)
                address = start(offline=True)
                drive('wait-for-id sync', 'tap-id starred', 'tap A note from the allotments')
                offline_full = capture('17-full-article-offline')
                assert says(offline_full, 'Every paragraph of the page'), offline_full
                assert len(account.requests) == requests_before, 'reading offline called Miniflux'
                checks.append('the fetched article reopens offline after a restart')

                # Following a suggested feed, filed under a real category.
                stop()
                address = start()
                drive('wait-for-id sync', 'tap-id settings', 'tap-id directory')
                capture('18-suggested-feeds')
                drive('tap-id starter-0')
                added = capture('19-feed-added')
                assert says(added, 'was added to your Miniflux account'), added
                assert '/v1/categories' in paths('GET'), paths('GET')
                assert account.feeds == [
                    'https://feeds.bbci.co.uk/news/science_and_environment/rss.xml'], account.feeds
                assert [r['body'] for r in account.requests if r['method'] == 'POST'] == [
                    {'feed_url': account.feeds[0], 'category_id': 3}]
                checks.append('a chosen feed is filed under an existing category')

                assert paths('GET').count('/ridge.png') == images, \
                    'the saved picture was downloaded again'
                (output/'result.json').write_text(json.dumps({
                    'status': 'passed', 'scale': args.scale,
                    'fixture': 'original-miniflux-account',
                    'requests': account.requests, 'checks': checks}, indent=2)+'\n')
            finally:
                stop()
                server.shutdown()


if __name__ == '__main__':
    main()

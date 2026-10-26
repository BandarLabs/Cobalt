#!/usr/bin/env python3
"""Exercise HTTPS feed/image persistence, failed saves, retry and offline restart."""
import argparse
import hashlib
import http.server
import ssl
import threading
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import tempfile
import time
import urllib.request
import struct
import zlib

ROOT = Path(__file__).resolve().parents[2]
URL = 'https://example.com/journal.xml'
FEED = ('<rss><channel><title>The Field Journal</title><item><title>A morning by the river</title>'
        '<link>https://example.com/river</link><content:encoded><![CDATA['
        '<h2>Along the river</h2><figure><img src="/ridge.png" alt="A mountain ridge"/>'
        '<figcaption>The hills beyond the river.</figcaption></figure><p>The first light reaches the bridge before the town wakes.</p>'
        '<blockquote>A good walk leaves room to notice small things.</blockquote>'
        + '<p>Reeds bend towards the water. Beyond the footpath, the allotments are quiet. '
          'We stop at the old stone steps and listen to the birds across the bank.</p>' * 18
        + ']]></content:encoded></item></channel></rss>').encode()


# The other half of what publishers actually serve: Atom, with one entry whose
# content is real markup and one that offers a summary and nothing else.
ATOM = ('<?xml version="1.0" encoding="utf-8"?>'
        '<feed xmlns="http://www.w3.org/2005/Atom"><title>The Ridge Letter</title>'
        '<entry><title>An afternoon on the ridge</title>'
        '<link href="https://example.com/ridge"/>'
        '<content type="html">&lt;figure&gt;&lt;img src="/ridge.png" alt="A mountain ridge"/&gt;'
        '&lt;figcaption&gt;The ridge from the north path.&lt;/figcaption&gt;&lt;/figure&gt;'
        '&lt;p&gt;The path climbs through bracken and then gives out onto open ground.&lt;/p&gt;'
        '</content></entry>'
        '<entry><title>A note about the weather</title>'
        '<link href="https://example.com/weather"/>'
        '<summary>Rain until Thursday, then a cold clear spell.</summary>'
        '</entry></feed>').encode()


def original_picture():
    width, height = 320, 120
    def chunk(kind, data):
        return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind+data))
    rows = bytearray()
    for y in range(height):
        rows.append(0)
        for x in range(width):
            ridge = 22 + abs(x-130)//3
            rows.append(48 if ridge <= y < ridge+3 else 255)
    return (b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', width,height,8,0,0,0,0))
            + chunk(b'IDAT', zlib.compress(bytes(rows))) + chunk(b'IEND', b''))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--scale', default='default')
    args = parser.parse_args()
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    target = Path(os.environ.get('CARGO_TARGET_DIR', str(ROOT/'target'))).resolve()
    cli = target/'debug/kobo'
    process = None
    requests = []
    with tempfile.TemporaryDirectory(prefix='cobalt-rss-', dir='/tmp') as private:
        private = Path(private)
        env = dict(os.environ, TMPDIR=str(private), CARGO_TARGET_DIR=str(target),
                   CARGO_PROFILE_DEV_DEBUG='0', CARGO_INCREMENTAL='0',
                   KOBO_SIM_PROFILE='clara-bw-391', KOBO_TEXT_SCALE=args.scale,
                   KOBO_SIM_FIXTURE='original-rss-journal', KOBO_SIM_SEED='0')
        config = private/'config'
        env.update(KOBO_STREAM_CONFIG_DIR=str(config), KOBO_SIM_TRUST_DIR=str(config/'trust'))
        subprocess.run([str(cli), 'stream', 'init', '--host', '127.0.0.1'], cwd=ROOT,
                       env=env, check=True, capture_output=True, timeout=30)
        picture = original_picture()
        latest_feed = [b'']
        atom_feed = [b'']
        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_): pass
            def do_GET(self):
                requests.append({'path': self.path, 'authorization': self.headers.get('Authorization'),
                                 'token': self.headers.get('X-Auth-Token')})
                if self.path not in ('/ridge.png', '/journal.xml', '/atom.xml'):
                    self.send_error(404); return
                body = {'/ridge.png': picture, '/atom.xml': atom_feed[0]}.get(self.path, latest_feed[0])
                self.send_response(200)
                self.send_header('Content-Type', {'/ridge.png': 'image/png',
                                                  '/atom.xml': 'application/atom+xml'}
                                 .get(self.path, 'application/rss+xml'))
                self.send_header('Content-Length', str(len(body)))
                self.end_headers()
                self.wfile.write(body)
        server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        tls.load_cert_chain(config/'stream/cert.pem', config/'stream/key.pem')
        server.socket = tls.wrap_socket(server.socket, server_side=True)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        origin = f'https://127.0.0.1:{server.server_port}'
        feed_url = origin+'/journal.xml'
        fixture = FEED.replace(b'https://example.com', origin.encode())
        latest_feed[0] = fixture
        state = private/'cobalt-sim-state/rss'
        shelf = private/'cobalt-sim-data/rss'
        state.mkdir(parents=True)
        shelf.mkdir(parents=True)
        atom_url = origin+'/atom.xml'
        atom_feed[0] = ATOM.replace(b'https://example.com', origin.encode())
        digest = hashlib.sha256(feed_url.encode()).hexdigest()
        (state/'feeds').write_text(feed_url+'\tThe Field Journal\t'+origin+'/\n'
                                   + atom_url+'\tThe Ridge Letter\t'+origin+'/\n')
        (state/digest).write_text('1:'+hashlib.sha256(fixture).hexdigest())
        snapshot = shelf/(digest[:60]+'.1')
        snapshot.write_bytes(fixture)
        image_key = hashlib.sha256(('rss-image:'+origin+'/ridge.png').encode()).hexdigest()
        log_path = args.output/'simulator.log'
        with log_path.open('w') as log:
            def stop():
                if process and process.poll() is None:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait(timeout=5)

            def start():
                nonlocal process
                log.seek(0)
                log.truncate()
                process = subprocess.Popen([str(cli), 'dev', '127.0.0.1:0'],
                    cwd=ROOT/'examples/rss', env=env, stdout=log, stderr=log, start_new_session=True)
                deadline = time.monotonic()+120
                while time.monotonic()<deadline:
                    if process.poll() is not None:
                        raise RuntimeError(log_path.read_text()[-3000:])
                    match = re.search(r'Kobo app simulator: http://(127\.0\.0\.1:\d+)', log_path.read_text())
                    if match:
                        return match.group(1)
                    time.sleep(.1)
                raise RuntimeError('Simulator did not start')

            def drive(*steps):
                command = [str(cli), 'drive', '--address', address, '--ideal', '--shots', str(args.output)]
                for step in steps:
                    command.extend(['--step', step])
                subprocess.run(command, cwd=ROOT, env=env, stdout=log, stderr=log, check=True, timeout=45)

            def get(endpoint):
                with urllib.request.urlopen(f'http://{address}/{endpoint}', timeout=5) as response:
                    return json.load(response)

            def paths():
                return [request['path'] for request in requests]

            def capture(name, fetches=0, picture=None):
                # A fetch count of None means the phase asserts on the requests
                # the fixture actually received instead, which says more.
                counted = [] if fetches is None else [
                    f'expect-state /activity#/effects/fetch {fetches}']
                drive('wait-idle', *counted,
                      'expect-state /activity#/effects/post 0', 'expect-state /activity#/effects/put 0',
                      'expect-state /activity#/effects/patch 0', 'shot '+name)
                diagnostics = get('diagnostics')
                assert not [i for i in diagnostics['issues'] if i['severity']=='error'], diagnostics
                layout = get('layout')
                (args.output/(name+'.layout.json')).write_text(json.dumps(layout, indent=2)+'\n')
                if picture is not None:
                    assert any('Picture(' in node['kind'] for node in layout['nodes']) == picture, layout
                metadata = json.loads((args.output/(name+'.json')).read_text())
                assert metadata['app']=='rss' and metadata['source']['fixture']=='original-rss-journal'
                assert metadata['fonts'] and len(metadata['source']['binarySha256'])==64
                return layout

            try:
                address = start()
                drive('wait-for-id feed-0', 'tap-id feed-0', 'wait-for-id item-0',
                      'scenario storage-full', 'tap-id item-0', 'wait-idle')
                capture('01-downloaded-image-unsaved', fetches=1)
                assert requests == [{'path': '/ridge.png', 'authorization': None, 'token': None}], requests
                assert not (state/image_key).exists(), 'A failed image save was marked published'
                drive('tap Back', 'wait-for-id retry-save')
                capture('02-image-save-retry', fetches=1)
                drive('scenario normal', 'tap-id retry-save', 'wait-idle')
                pointer = (state/image_key).read_text()
                slot, digest = pointer.split(':')
                assert digest==hashlib.sha256(picture).hexdigest()
                stored = shelf/(image_key[:60]+'.'+slot)
                assert stored.read_bytes()==picture
                assert len(requests)==1, 'Retry downloaded the image again'
                drive('tap-id item-0', 'wait-idle')
                capture('03-image-save-recovered', fetches=1)
                stop()
                address = start()
                drive('wait-for-id feed-0', 'scenario offline', 'tap-id feed-0',
                      'wait-for-id item-0', 'tap-id item-0', 'wait-idle')
                capture('04-offline-image-after-restart')
                assert len(requests)==1
                assert stored.read_bytes()==picture
                assert (state/image_key).read_text()==pointer
                feed_key = hashlib.sha256(feed_url.encode()).hexdigest()
                old_pointer = (state/feed_key).read_text()
                latest_feed[0] = fixture.replace(b'A morning by the river', b'An afternoon by the river')
                drive('tap Back', 'scenario storage-full', 'tap-id refresh',
                      'wait-for An afternoon by the river', 'wait-for-id retry-save')
                capture('05-feed-save-failed', fetches=1)
                assert (state/feed_key).read_text()==old_pointer
                assert snapshot.read_bytes()==fixture
                latest_feed[0] = fixture.replace(b'A morning by the river', b'An evening by the river')
                drive('tap-id refresh', 'wait-for An evening by the river', 'wait-idle')
                assert (state/feed_key).read_text()==old_pointer
                drive('scenario normal', 'tap-id retry-save', 'wait-idle')
                capture('06-latest-feed-saved', fetches=2)
                new_pointer = (state/feed_key).read_text()
                feed_slot, feed_digest = new_pointer.split(':')
                assert feed_digest==hashlib.sha256(latest_feed[0]).hexdigest()
                assert (shelf/(feed_key[:60]+'.'+feed_slot)).read_bytes()==latest_feed[0]
                assert snapshot.read_bytes()==fixture, 'Retry overwrote the previously published slot'
                assert len(requests)==3, 'Saving a pending feed downloaded it again'
                assert all(r['authorization'] is None and r['token'] is None for r in requests)
                stop()
                address = start()
                drive('wait-for-id feed-0', 'scenario offline', 'tap-id feed-0',
                      'wait-for An evening by the river', 'tap-id item-0', 'wait-idle')
                capture('07-latest-feed-offline')
                assert len(requests)==3
                assert (state/feed_key).read_text()==new_pointer
                for mode, number in [('missing', 8), ('damaged', 10)]:
                    stop()
                    image_pointer = (state/image_key).read_text()
                    image_slot = image_pointer.split(':')[0]
                    cached_image = shelf/(image_key[:60]+'.'+image_slot)
                    if mode == 'missing':
                        cached_image.unlink()
                    else:
                        cached_image.write_bytes(b'damaged fixture image')
                    before = len(requests)
                    address = start()
                    drive('wait-for-id feed-0', 'scenario offline', 'tap-id feed-0',
                          'wait-for-id item-0', 'tap-id item-0', 'wait-idle')
                    capture(f'{number:02}-{mode}-image-offline', fetches=1, picture=False)
                    assert len(requests)==before
                    assert (state/image_key).read_text()==image_pointer
                    drive('tap Back', 'scenario normal', 'tap-id item-0', 'wait-idle')
                    capture(f'{number+1:02}-{mode}-image-recovered', fetches=2, picture=True)
                    assert len(requests)==before+1
                    assert requests[-1]=={'path': '/ridge.png', 'authorization': None, 'token': None}
                    restored_pointer = (state/image_key).read_text()
                    restored_slot, restored_digest = restored_pointer.split(':')
                    assert restored_slot!=image_slot
                    assert restored_digest==hashlib.sha256(picture).hexdigest()
                    assert (shelf/(image_key[:60]+'.'+restored_slot)).read_bytes()==picture
                stop()
                address = start()
                drive('wait-for-id feed-0', 'scenario offline', 'tap-id feed-0',
                      'wait-for-id item-0', 'tap-id item-0', 'wait-idle')
                capture('12-repaired-image-offline', picture=True)
                assert len(requests)==5

                # The other half of what publishers serve. An Atom feed with
                # one entry of real markup and one that offers a summary only,
                # read on the same panel and against the same saved picture.
                drive('tap Back', 'tap Back', 'scenario normal', 'wait-for-id feed-1')
                before_atom = len(requests)
                drive('tap-id feed-1', 'wait-for-id item-0', 'tap-id item-0', 'wait-idle')
                atom_article = capture('13-atom-article', fetches=None, picture=True)
                assert any('The ridge from the north path.' in str(line)
                           for node in atom_article['nodes'] for line in node['lines']), atom_article
                assert paths()[before_atom:].count('/atom.xml') == 1, paths()[before_atom:]
                assert '/ridge.png' not in paths()[before_atom:], \
                    'a picture already saved was downloaded again for another feed'
                drive('tap Back', 'tap-id item-1', 'wait-idle')
                summary = capture('14-atom-summary-entry', fetches=None, picture=False)
                assert any('Rain until Thursday' in str(line)
                           for node in summary['nodes'] for line in node['lines']), summary
                stop()
                address = start()
                drive('wait-for-id feed-1', 'scenario offline', 'tap-id feed-1',
                      'wait-for-id item-0', 'tap-id item-0', 'wait-idle')
                capture('15-atom-article-offline', fetches=None, picture=True)
                assert len(requests)==before_atom+1, paths()[before_atom:]
                (args.output/'result.json').write_text(json.dumps({
                    'status': 'passed', 'checks': ['credential-free HTTPS image acquisition',
                    'failed image write does not publish a pointer', 'visible image-save retry',
                    'retry persists exact bytes without downloading again',
                    'offline process restart renders saved image without a fetch',
                    'failed feed save preserves the published snapshot',
                    'retry publishes the newest refresh without another download',
                    'latest feed and image reopen offline after restart',
                    'missing and damaged images preserve the pointer while offline',
                    'reopening online restores missing and damaged images into the other slot',
                    'repaired image opens offline after another restart',
                    'an Atom entry reads with its figure, caption and alt text',
                    'a saved picture is reused across feeds without another download',
                    'a summary-only entry reads as its summary with no picture',
                    'the Atom article and its picture reopen offline after a restart',
                    'zero POST/PUT/PATCH requests'], 'requests': requests,
                    'fixture': 'original-rss-journal',
                }, indent=2)+'\n')
            finally:
                stop()
                server.shutdown()
                server.server_close()


if __name__ == '__main__':
    main()

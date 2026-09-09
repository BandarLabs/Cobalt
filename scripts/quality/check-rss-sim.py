#!/usr/bin/env python3
"""Exercise saved RSS HTML through the actual simulator, without a live publisher."""
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
    with tempfile.TemporaryDirectory(prefix='cobalt-rss-', dir='/tmp') as private:
        private = Path(private)
        env = dict(os.environ, TMPDIR=str(private), CARGO_TARGET_DIR=str(target),
                   CARGO_PROFILE_DEV_DEBUG='0', CARGO_INCREMENTAL='0',
                   KOBO_SIM_PROFILE='clara-bw-391', KOBO_TEXT_SCALE=args.scale,
                   KOBO_SIM_FIXTURE='original-rss-journal', KOBO_SIM_SEED='0')
        state = private/'cobalt-sim-state/rss'
        shelf = private/'cobalt-sim-data/rss'
        state.mkdir(parents=True)
        shelf.mkdir(parents=True)
        digest = hashlib.sha256(URL.encode()).hexdigest()
        (state/'feeds').write_text(URL+'\tThe Field Journal\thttps://example.com/\n')
        (state/digest).write_text('1:'+hashlib.sha256(FEED).hexdigest())
        (state/'feed-status-v1').write_text('rss-status-v1\n'+digest+'\t"2026-09-09 08:30 UTC"\t""\n')
        snapshot = shelf/(digest[:60]+'.1')
        snapshot.write_bytes(FEED)
        (shelf/'subscriptions.opml').write_text('<opml><body><outline text="Existing" xmlUrl="'+URL+'"/><outline text="Second journal" xmlUrl="https://example.com/second.xml"/><outline xmlUrl="http://example.com/insecure"/></body></opml>')
        picture = original_picture()
        image_key = hashlib.sha256(b'rss-image:https://example.com/ridge.png').hexdigest()
        (state/image_key).write_text('1:'+hashlib.sha256(picture).hexdigest())
        (shelf/(image_key[:60]+'.1')).write_bytes(picture)
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

            def capture(name, fetches=0):
                drive('wait-idle', f'expect-state /activity#/effects/fetch {fetches}',
                      'expect-state /activity#/effects/post 0', 'expect-state /activity#/effects/put 0',
                      'expect-state /activity#/effects/patch 0', 'shot '+name)
                diagnostics = get('diagnostics')
                assert not [i for i in diagnostics['issues'] if i['severity']=='error'], diagnostics
                (args.output/(name+'.layout.json')).write_text(json.dumps(get('layout'), indent=2)+'\n')
                metadata = json.loads((args.output/(name+'.json')).read_text())
                assert metadata['app']=='rss' and metadata['source']['fixture']=='original-rss-journal'
                assert metadata['fonts'] and len(metadata['source']['binarySha256'])==64

            try:
                address = start()
                drive('wait-for-id feed-0')
                capture('01-saved-feeds')
                drive('tap-id feed-0', 'wait-for-id item-0', 'expect A morning by the river')
                drive('expect 1 unread of 1 article')
                capture('02-offline-articles')
                drive('tap-id search-articles')
                capture('09-search-entry')
                drive('type quartz', 'tap-id kb.enter', 'expect No saved articles match')
                capture('11-no-matches')
                drive('tap-id search-articles', 'tap-id clear-search', 'tap-id search-articles',
                      'type reeds', 'tap-id kb.enter', 'wait-for-id item-0')
                capture('10-search-results')
                drive('tap-id search-articles', 'tap-id clear-search')
                drive('tap-id item-0', 'wait-for-id reader-forward')
                capture('03-html-article')
                drive('tap-id reader-forward')
                capture('04-next-page')
                position = (state/'reading-v1').read_bytes()
                assert b'at 0\\n' not in position
                page_layout = get('layout')
                drive('tap Back', 'scenario offline', 'tap-id refresh', 'wait-for This reader is not on a network',
                      'wait-for-id item-0', 'expect A morning by the river')
                drive('expect 0 unread of 1 article')
                capture('05-refresh-failure', fetches=2)
                history = (state/'feed-status-v1').read_text()
                assert '2026-09-09 08:30 UTC' in history and 'This reader is not on a network' in history
                assert snapshot.read_bytes()==FEED
                stop()
                address = start()
                drive('wait-for-id feed-0', 'tap-id feed-0', 'wait-for-id item-0', 'tap-id item-0')
                capture('06-reopened-after-restart')
                assert (state/'reading-v1').read_bytes()==position
                restored_layout = get('layout')
                assert {k:v for k,v in restored_layout.items() if k!='paints'} == {k:v for k,v in page_layout.items() if k!='paints'}, 'Article did not resume at the saved page'
                drive('scenario storage-full', 'tap-id reader-forward', 'wait-idle',
                      'expect Reading progress is not saved')
                assert (state/'reading-v1').read_bytes()==position
                capture('07-progress-save-failure')
                drive('tap Back', 'scenario normal', 'tap-id retry-save', 'wait-idle')
                assert (state/'reading-v1').read_bytes()!=position
                drive('tap-id item-0')
                capture('08-progress-save-recovered')
                drive('tap Back', 'tap Back', 'tap-id add', 'tap-id import-opml',
                      'wait-for-id import-file-0', 'tap-id import-file-0', 'wait-for-id import-confirm',
                      'expect 1 new feed')
                drive('expect Second journal', 'expect https://example.com/', 'expect second.xml',
                      'tap-id import-toggle-1', 'expect Not selected',
                      'tap-id import-toggle-1', 'expect Selected', 'wait-for-id import-confirm')
                capture('12-opml-preview')
                before_import = (state/'feeds').read_bytes()
                drive('scenario storage-full', 'tap-id import-confirm', 'wait-idle',
                      'expect Subscriptions were not saved')
                assert (state/'feeds').read_bytes()==before_import
                capture('13-opml-save-failure')
                drive('scenario normal', 'tap-id import-confirm', 'wait-for-id feed-1')
                assert b'https://example.com/second.xml' in (state/'feeds').read_bytes()
                assert (state/'feeds').read_bytes().count(URL.encode())==1
                capture('14-opml-imported')
                imported = (state/'feeds').read_bytes()
                drive('scenario storage-full', 'tap-id feed-menu-1', 'tap-id feed-forget',
                      'wait-for-id retry-subscriptions', 'expect Subscriptions were not saved')
                assert (state/'feeds').read_bytes()==imported
                capture('15-subscriptions-save-failure')
                drive('scenario normal', 'tap-id retry-subscriptions', 'wait-idle')
                assert (state/'feeds').read_bytes()==before_import
                capture('16-subscriptions-save-recovered')
                stop()
                address = start()
                drive('wait-for-id feed-0')
                assert (state/'feeds').read_bytes()==before_import
                drive('expect 0 unread')
                capture('17-subscriptions-reopened')
                stop()
                damaged = b'\xfforiginal subscriptions must survive'
                (state/'feeds').write_bytes(damaged)
                address = start()
                drive('wait-for-id retry-load-subscriptions', 'expect The saved file has been left unchanged',
                      'tap-id retry-load-subscriptions', 'wait-idle')
                assert (state/'feeds').read_bytes()==damaged
                capture('18-subscriptions-unreadable')
                (state/'feeds').write_bytes(before_import)
                drive('tap-id retry-load-subscriptions', 'wait-for-id feed-0')
                capture('19-subscriptions-reloaded')
                drive('tap-id add', 'tap-id browse-feeds', 'expect BBC Science', 'expect NASA Science')
                capture('20-browse-feeds')
                drive('scenario offline', 'tap-id starter-0', 'wait-for This reader is not on a network')
                assert (state/'feeds').read_bytes()==before_import
                capture('21-starter-offline', fetches=2)
                assert snapshot.read_bytes()==FEED
                (args.output/'result.json').write_text(json.dumps({
                    'status': 'passed', 'checks': ['saved feed opens without network', 'saved body search, no matches and clearing without network',
                    'HTML reading and page turn with a verified offline image', 'offline refresh retains articles',
                    'process restart reopens saved body at saved page', 'original snapshot unchanged',
                    'no server mutations', 'failed progress write preserves prior state', 'explicit save retry persists latest position', 'OPML preview supports selection and skips duplicate and HTTP entries', 'OPML save failure preserves subscriptions and retry adds only new feed', 'failed unfollow preserves disk state and retry survives restart', 'unreadable subscriptions survive retry and a repaired file can reload', 'starter browsing sends no requests and offline selection does not subscribe'], 'fixture': 'original-rss-journal',
                    'limits': ['Image snapshot is seeded locally; network download and image-save failure need separate coverage'],
                }, indent=2)+'\n')
            finally:
                stop()


if __name__ == '__main__':
    main()

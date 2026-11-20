#!/usr/bin/env python3
"""Follow a newsletter from its site's address and read a summary-only feed as
whole articles, against private HTTPS fixtures: the page behind a teaser
fetched and kept, a page with no article in it, a site that does not answer
and then does, a site that refuses, and all of it again with the radio off."""
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

POST = 'harbour-post.example'
LETTER = 'tide-letter.example'
SEARCH = 'feedsearch.dev'

# A news site of the common kind: its feed gives a sentence and a link.
STORIES = [
    ('wall', 'The harbour wall is to be rebuilt',
     'The council voted on Tuesday to rebuild the harbour wall.'),
    ('ferry', 'Ferry times change from Monday',
     'The first sailing moves to half past six.'),
    ('light', 'The lighthouse opens to visitors',
     'Tours of the lamp room begin in the spring.'),
    ('budget', 'What the budget means for the quay',
     'Members can read the full breakdown.'),
    ('market', 'The fish market moves indoors',
     'Stalls will trade under the old customs shed this winter.'),
]


def summary_feed():
    items = ''.join(
        f'<item><title>{title}</title><link>https://{POST}/news/{slug}</link>'
        f'<description>{summary}</description>'
        f'<pubDate>Thu, 19 Nov 2026 0{9 - index}:00:00 GMT</pubDate></item>'
        for index, (slug, title, summary) in enumerate(STORIES))
    return (f'<rss version="2.0"><channel><title>The Harbour Post</title>'
            f'<link>https://{POST}/</link>{items}</channel></rss>').encode()


def article(title, distinctive):
    """A page as a site serves one: the story inside its menus and footer."""
    paragraph = ('The work will take two winters and close the east steps while it is done. '
                 'Engineers found the footing sound but the facing stone loose along most '
                 'of its length, and the council has set the money aside in full. ')
    body = ''.join(f'<p>{paragraph}</p>' for _ in range(6))
    return (f'<html><head><title>{title} | The Harbour Post</title></head><body>'
            f'<nav><a href="/">Home</a><a href="/news">News</a><a href="/sport">Sport</a>'
            f'<a href="/weather">Weather</a></nav>'
            f'<article><h1>{title}</h1>{body}<p>{distinctive}</p></article>'
            f'<footer><p>Contact the newsroom</p><p>Terms of use</p></footer>'
            f'</body></html>').encode()


PAGES = {
    'wall': article('The harbour wall is to be rebuilt',
                    'The quartz facing from the old wall will be kept and set back in place.'),
    'light': article('The lighthouse opens to visitors',
                     'The lamp room has not been open to the public since the keeper left.'),
    'market': article('The fish market moves indoors',
                      'The customs shed has stood empty since the bonded store closed.'),
    # A page that will not give its story: a headline and an invitation.
    'ferry': (b'<html><body><nav><a href="/">Home</a></nav><h1>Ferry times change from Monday</h1>'
              b'<p>Subscribe to keep reading.</p></body></html>'),
}

LETTER_HOME = (f'<html><head><title>The Tide Letter</title>'
               f'<link rel="stylesheet" href="/style.css">'
               f'<link rel="alternate" type="application/rss+xml" href="/feed" '
               f'title="The Tide Letter"/></head><body><h1>The Tide Letter</h1>'
               f'<p>A letter about the sea, most Sundays.</p>'
               f'<form><input type="email"><button>Subscribe</button></form>'
               f'</body></html>').encode()

ISSUE = ('<p>This week the spring tides came a day early, and the harbour master moved the '
         'small boats up the slip before breakfast.</p>'
         + '<p>A reader wrote to ask why the tables in the almanac differ from the ones the '
           'pilots use. The short answer is that the almanac is computed for the bar and the '
           'pilots correct for the river, and the long answer is the rest of this letter.</p>' * 7)

LETTER_FEED = (f'<rss version="2.0" xmlns:content="http://purl.org/rss/1.0/modules/content/">'
               f'<channel><title>The Tide Letter</title><link>https://{LETTER}/</link>'
               f'<item><title>Why the tables disagree</title>'
               f'<link>https://{LETTER}/p/tables</link>'
               f'<content:encoded><![CDATA[{ISSUE}]]></content:encoded></item>'
               f'<item><title>A slack-water walk</title><link>https://{LETTER}/p/walk</link>'
               f'<content:encoded><![CDATA[{ISSUE}]]></content:encoded></item>'
               f'</channel></rss>').encode()


def certificates(config, hosts):
    """A private certificate authority and one server certificate naming every
    fixture host, so one listener can stand in for all of them."""
    trust = config/'trust'
    trust.mkdir(parents=True)

    def openssl(*arguments):
        subprocess.run(['openssl', *arguments], check=True, capture_output=True, timeout=60)

    names = ','.join(f'DNS:{host}' for host in hosts)
    extensions = config/'server.ext'
    extensions.write_text(f'basicConstraints=CA:FALSE\nsubjectAltName={names}\n'
                          'extendedKeyUsage=serverAuth\n')
    openssl('req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '2', '-subj',
            '/CN=Fixture authority', '-keyout', str(config/'root.key'),
            '-out', str(trust/'root.pem'), '-addext', 'basicConstraints=critical,CA:TRUE',
            '-addext', 'keyUsage=critical,keyCertSign')
    openssl('req', '-newkey', 'rsa:2048', '-nodes', '-subj', f'/CN={hosts[0]}',
            '-keyout', str(config/'key.pem'), '-out', str(config/'server.csr'))
    openssl('x509', '-req', '-in', str(config/'server.csr'), '-CA', str(trust/'root.pem'),
            '-CAkey', str(config/'root.key'), '-CAcreateserial', '-days', '2',
            '-extfile', str(extensions), '-out', str(config/'cert.pem'))
    return trust


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
    # What the fixture site is doing wrong at the moment.
    down = {'light'}
    slow = {'wall'}
    with tempfile.TemporaryDirectory(prefix='cobalt-rss-whole-', dir='/tmp') as temporary:
        private = Path(temporary)
        config = private/'config'
        trust = certificates(config, [POST, LETTER, SEARCH])

        class Handler(http.server.BaseHTTPRequestHandler):
            protocol_version = 'HTTP/1.1'

            def log_message(self, *_):
                pass

            def answer(self, status, kind, body):
                self.send_response(status)
                self.send_header('Content-Type', kind)
                self.send_header('Content-Length', str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def do_GET(self):
                host = self.headers.get('Host', '').split(':')[0]
                path = self.path.split('?')[0]
                requests.append((host, path))
                if host == SEARCH:
                    return self.answer(200, 'application/json', b'[]')
                if host == LETTER and path == '/':
                    return self.answer(200, 'text/html', LETTER_HOME)
                if host == LETTER and path == '/feed':
                    return self.answer(200, 'application/rss+xml', LETTER_FEED)
                if host == POST and path == '/feed.xml':
                    return self.answer(200, 'application/rss+xml', summary_feed())
                if host == POST and path.startswith('/news/'):
                    slug = path.rsplit('/', 1)[1]
                    if slug == 'budget':
                        return self.answer(403, 'text/html', b'<p>Members only.</p>')
                    if slug in down:
                        return self.answer(503, 'text/html', b'<p>Back soon.</p>')
                    if slug in slow:
                        time.sleep(4)
                    if slug in PAGES:
                        return self.answer(200, 'text/html', PAGES[slug])
                self.answer(404, 'text/html', b'<p>Not here.</p>')

        server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        server.daemon_threads = True
        tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        tls.load_cert_chain(config/'cert.pem', config/'key.pem')
        server.socket = tls.wrap_socket(server.socket, server_side=True)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        socket = f'127.0.0.1:{server.server_port}'
        env = dict(os.environ, TMPDIR=str(private), CARGO_TARGET_DIR=str(target),
                   CARGO_PROFILE_DEV_DEBUG='0', CARGO_INCREMENTAL='0',
                   KOBO_SIM_PROFILE='clara-bw-391', KOBO_TEXT_SCALE=args.scale,
                   KOBO_SIM_SEED='0', KOBO_SIM_TRUST_DIR=str(trust),
                   KOBO_SIM_HTTP_FIXTURE=','.join(f'{host}={socket}'
                                                  for host in (POST, LETTER, SEARCH)))
        env.pop('KOBO_SIM_OFFLINE', None)
        (private/'cobalt-sim-state/rss').mkdir(parents=True)
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
                """A fresh simulator over the same saved state, at the shelf."""
                nonlocal process
                stop()
                log.seek(0)
                log.truncate()
                started = dict(env, KOBO_SIM_OFFLINE='1') if offline else env
                process = subprocess.Popen([str(cli), 'dev', '127.0.0.1:0'],
                                           cwd=ROOT/'examples/rss', env=started, stdout=log,
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
                               check=True, timeout=120)

            def get(endpoint):
                with urllib.request.urlopen(f'http://{address}/{endpoint}', timeout=5) as answer:
                    return json.load(answer)

            def capture(name):
                drive('wait-idle 30000', 'shot '+name)
                diagnostics = get('diagnostics')
                errors = [i for i in diagnostics['issues'] if i['severity'] == 'error']
                assert not errors, f'{name}: {errors}'
                layout = get('layout')
                (output/(name+'.layout.json')).write_text(json.dumps(layout, indent=2)+'\n')
                drawn = ' '.join(str(line) for node in layout['nodes'] for line in node['lines'])
                return ' '.join(drawn.split())

            def asked(host):
                return [path for seen, path in requests if seen == host]

            def add(typed):
                drive('wait-for Add a feed', 'tap Add a feed', 'wait-for Import OPML',
                      'tap-id feed-address', 'wait-for-id kb.enter', 'type '+typed,
                      'tap-id kb.enter', 'wait-idle 30000', 'wait-for Select to add')

            def open_story(index):
                drive('wait-for The Harbour Post', 'tap The Harbour Post',
                      'wait-for-id item-0', f'tap-id item-{index}')

            try:
                address = start()
                drive('wait-for No feeds yet', 'tap Add a feed', 'wait-for Import OPML')
                said = capture('01-add-a-feed')
                assert 'Most newsletters can be followed here too.' in said, said
                checks.append('the screen that adds a feed says newsletters can be followed')
                drive('tap Back', 'wait-for No feeds yet')

                # A newsletter, by the address of its site.
                add(f'https://{LETTER}')
                said = capture('02-newsletter-found')
                assert 'The Tide Letter' in said and 'Feed preview' in said, said
                assert asked(LETTER) == ['/', '/feed'], asked(LETTER)
                assert not asked(SEARCH), 'a pasted address reached the search service'
                checks.append('a pasted site address is followed to the feed its page names')
                checks.append('a pasted address is never sent to the search service')
                drive('tap-id found-0', 'wait-for unread of 2 articles')
                capture('03-newsletter-issues')
                drive('tap-id item-0')
                said = capture('04-newsletter-issue')
                assert 'spring tides came a day early' in said, said
                assert asked(LETTER) == ['/', '/feed', '/feed'], asked(LETTER)
                checks.append('an issue the feed carries whole opens without asking for its page')

                # A summary-only feed: the page behind the teaser, slowly.
                address = start()
                add(f'https://{POST}/feed.xml')
                drive('tap-id found-0', 'wait-for unread of 5 articles')
                capture('05-teasers')
                drive('tap-id item-0', 'wait-for Getting the article', 'shot 06-getting-the-article')
                said = capture('07-whole-article')
                assert 'The work will take two winters' in said, said
                assert 'Weather' not in said and 'Terms of use' not in said, said
                assert asked(POST).count('/news/wall') == 1, asked(POST)
                checks.append('a teaser says the article is on its way, then opens whole')

                # A page with no article in it leaves the teaser and says so.
                address = start()
                open_story(1)
                said = capture('08-no-more-on-the-page')
                assert 'The first sailing moves to half past six.' in said, said
                assert 'had no more to read than the feed gave' in said, said
                assert 'Subscribe' not in said, said
                address = start()
                open_story(1)
                said = capture('08b-no-more-said-once')
                assert 'The first sailing moves' in said and 'no more to read' not in said, said
                assert asked(POST).count('/news/ferry') == 1, asked(POST)
                checks.append('a page with no article leaves the teaser, says so once, and is not asked for again')

                # A site that does not answer, and then does.
                address = start()
                open_story(2)
                said = capture('09-site-did-not-answer')
                assert 'Tours of the lamp room begin in the spring.' in said, said
                assert 'Open it again to try once more.' in said, said
                down.clear()
                address = start()
                open_story(2)
                said = capture('10-opened-again')
                assert 'The work will take two winters' in said, said
                checks.append('a page that did not come shows the teaser, and opening it again fetches it')

                # A site that refuses.
                address = start()
                open_story(3)
                said = capture('11-site-refused')
                assert 'Members can read the full breakdown.' in said, said
                assert 'would not give this reader the whole article' in said, said
                checks.append('a refused page leaves the teaser and says the site refused')

                # What the shelf says, and a search for a word only the
                # fetched article holds.
                address = start()
                said = capture('12-shelf')
                # Two issues the letter carries whole, two articles kept, and
                # two teasers whose pages are known to hold no more.
                assert 'Ready offline: 6 articles' in said, said
                drive('tap Search saved', 'wait-for-id kb.enter', 'type quartz',
                      'tap-id kb.enter', 'wait-for Results for quartz')
                said = capture('13-search-finds-fetched-words')
                assert 'The harbour wall is to be rebuilt' in said, said
                checks.append('searching saved articles finds a word only the fetched article holds')

                # Refresh all fetches the one teaser nobody has opened, and
                # none of the pages already known about.
                address = start()
                before = len(requests)
                drive('wait-for Ready offline', 'tap Refresh all', 'wait 1000',
                      'wait-idle 60000', 'wait 2000', 'wait-idle 60000')
                said = capture('14-after-refresh-all')
                fetched = [path for _, path in requests[before:] if path.startswith('/news/')]
                assert fetched == ['/news/market'], fetched
                assert 'Ready offline: 7 articles' in said, said
                checks.append('Refresh all fetches the teasers not yet read and nothing already known')

                # With the radio off: the kept article opens whole, and
                # nothing is asked of anybody.
                address = start(offline=True)
                before = len(requests)
                open_story(4)
                said = capture('15-kept-article-offline')
                assert 'The fish market moves indoors' in said, said
                assert 'The work will take two winters' in said, said
                assert len(requests) == before, requests[before:]
                checks.append('an article kept by Refresh all opens whole with no network')

                (output/'result.json').write_text(json.dumps({
                    'status': 'passed', 'scale': args.scale, 'checks': checks,
                    'requests': requests}, indent=2)+'\n')
            finally:
                stop()
                server.shutdown()


if __name__ == '__main__':
    main()

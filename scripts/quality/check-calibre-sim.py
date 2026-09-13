#!/usr/bin/env python3
"""Exercise calibre-web against an original, private HTTPS OPDS/EPUB fixture."""
import argparse
import base64
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

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--scale', default='extra-large')
    parser.add_argument('--profile', default='clara-bw-391')
    args = parser.parse_args()
    args.output = args.output.resolve(); args.output.mkdir(parents=True, exist_ok=True)
    target = Path(os.environ.get('CARGO_TARGET_DIR', ROOT/'target')).resolve()
    cli = target/'debug/kobo'
    requests = []
    authentication = {"required": False, "accepted": []}
    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_): pass
        def do_GET(self):
            requests.append(self.path)
            if authentication['required']:
                expected = 'Basic '+base64.b64encode(b'reader:sample').decode()
                if self.headers.get('Authorization') != expected:
                    self.send_error(401); return
                authentication['accepted'].append(self.path)
            if self.path == '/denied':
                self.send_error(401); return
            if self.path == '/invalid':
                body = b'<html><body>Sign in</body></html>'
            elif self.path == '/opds':
                body = (ROOT/'apps/calibre-web/fixtures/root.xml').read_bytes()
            elif self.path in ('/opds/authors', '/opds/shelves', '/opds?page=2'):
                title = 'Authors' if self.path.endswith('authors') else 'My shelves'
                body = f'<feed xmlns="http://www.w3.org/2005/Atom"><title>{title}</title></feed>'.encode()
            elif self.path == '/books/river.epub':
                body = (ROOT/'apps/calibre-web/fixtures/river.epub').read_bytes()
            else:
                self.send_error(404); return
            header = self.headers.get('Range')
            if header:
                start, end = header.removeprefix('bytes=').split('-')
                full = len(body); start = int(start); end = min(int(end) if end else full-1, full-1)
                body = body[start:end+1]
                self.send_response(206)
                self.send_header('Content-Range', f'bytes {start}-{end}/{full}')
            else: self.send_response(200)
            self.send_header('Content-Length', str(len(body)))
            self.send_header('Content-Type', 'application/epub+zip' if self.path.endswith('.epub') else 'application/atom+xml')
            self.end_headers(); self.wfile.write(body)
    process = None
    with tempfile.TemporaryDirectory(prefix='cobalt-calibre-', dir='/tmp') as directory:
        private = Path(directory); config = private/'config'
        env = dict(os.environ, TMPDIR=directory, CARGO_TARGET_DIR=str(target),
                   CARGO_PROFILE_DEV_DEBUG='0', CARGO_INCREMENTAL='0',
                   KOBO_STREAM_CONFIG_DIR=str(config), KOBO_SIM_TRUST_DIR=str(config/'trust'),
                   KOBO_SIM_PROFILE=args.profile, KOBO_TEXT_SCALE=args.scale,
                   KOBO_SIM_FIXTURE='original-calibre-library', KOBO_SIM_SEED='0',
                   KOBO_SIM_CLOCK_MILLIS='1788850860000', KOBO_SIM_UTC_OFFSET_MINUTES='0')
        subprocess.run([str(cli), 'stream', 'init', '--host', '127.0.0.1'], cwd=ROOT, env=env, check=True, capture_output=True, timeout=30)
        server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        tls.load_cert_chain(config/'stream/cert.pem', config/'stream/key.pem')
        server.socket = tls.wrap_socket(server.socket, server_side=True)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        root = f'https://127.0.0.1:{server.server_port}/opds'
        state = private/'cobalt-sim-state/calibre-web'; state.mkdir(parents=True)
        (state/'catalog').write_text(root+'|')
        log_path = args.output/'simulator.log'
        with log_path.open('w') as log:
            try:
                def start():
                    nonlocal process
                    log.seek(0); log.truncate()
                    process = subprocess.Popen([str(cli), 'dev', '127.0.0.1:0'], cwd=ROOT/'apps/calibre-web', env=env, stdout=log, stderr=log, start_new_session=True)
                    deadline = time.monotonic()+120
                    while time.monotonic()<deadline:
                        if process.poll() is not None: raise RuntimeError(log_path.read_text()[-3000:])
                        match = re.search(r'Kobo app simulator: http://(127\.0\.0\.1:\d+)', log_path.read_text())
                        if match: return match.group(1)
                        time.sleep(.1)
                    raise RuntimeError('Simulator did not start')
                address = start()
                def drive(*steps):
                    command = [str(cli), 'drive', '--address', address, '--ideal', '--shots', str(args.output)]
                    for step in steps: command += ['--step', step]
                    subprocess.run(command, cwd=ROOT, env=env, stdout=log, stderr=log, check=True, timeout=45)
                def get(path):
                    with urllib.request.urlopen(f'http://{address}/{path}', timeout=5) as response: return response.read()
                def capture(name):
                    drive('wait-idle', 'shot '+name)
                    diagnostics = json.loads(get('diagnostics'))
                    assert not [i for i in diagnostics['issues'] if i['severity']=='error'], diagnostics
                    (args.output/(name+'.layout.json')).write_text(json.dumps(json.loads(get('layout')),indent=2)+'\n')
                    provenance = json.loads((args.output/(name+'.json')).read_text())
                    assert provenance['app']=='calibre-web' and provenance['fonts']
                    assert provenance['source']['fixture']=='original-calibre-library'
                    assert len(provenance['source']['binarySha256'])==64
                def restart(expected='library'):
                    nonlocal address
                    os.killpg(process.pid, signal.SIGKILL); process.wait(timeout=5)
                    address = start(); drive('wait-for-id '+expected, 'wait-idle')
                drive('wait-for-id library', 'wait-idle')
                capture('01-library')
                drive('tap-id library', 'wait-for-id entry-0', 'wait-idle')
                capture('02-catalog')
                drive('tap-id entry-0', 'wait-idle', 'expect Authors', 'expect No books or sections', 'tap Back', 'wait-for-id entry-0')
                drive('tap-id entry-1', 'wait-idle', 'expect My shelves', 'tap Back', 'wait-for-id entry-0')
                drive('tap-id page-next', 'tap-id entry-2', 'wait-for-id download-book')
                capture('03-book-details')
                drive('tap-id download-book', 'wait-for-id offline-0', 'wait-idle')
                assert '/books/river.epub' in requests
                capture('04-downloaded')
                drive('tap-id offline-0', 'wait-for-id reader-forward', 'wait-idle')
                capture('05-reading')
                drive('tap-id reader-forward', 'wait-idle', 'tap-id reader-forward', 'wait-idle')
                capture('06-reading-position')
                before = (state/'books-v1').read_bytes()
                kept = json.loads(before)['payload']['books'][0]
                assert int(re.search(r'^at (\d+)', kept['memory'], re.M)[1])>0
                count = len(requests)
                restart()
                drive('scenario offline', 'tap-id offline', 'wait-for-id offline-0', 'tap-id offline-0', 'wait-for-id reader-forward', 'wait-idle')
                assert len(requests)==count, 'Offline reopening contacted the server'
                assert (state/'books-v1').read_bytes()==before, 'Reopening changed saved position'
                capture('07-offline-reopened')
                # A failed position save must preserve the acknowledged record.
                drive('scenario storage-full', 'tap-id reader-forward', 'wait-idle')
                assert (state/'books-v1').read_bytes()==before
                capture('08-position-save-failed')
                drive('tap Back', 'wait-for-id offline-save', 'scenario normal', 'tap-id offline-save', 'wait-idle')
                assert (state/'books-v1').read_bytes()!=before
                capture('09-position-save-retried')
                # Damage only this private fixture's shelf file, then replace it through the app.
                saved_book = json.loads((state/'books-v1').read_text())['payload']['books'][0]
                local_file = private/'cobalt-sim-data/calibre-web'/saved_book['file']
                assert local_file.is_file(), local_file
                local_file.write_bytes(b'Incomplete fixture file')
                drive('tap-id offline-0', 'wait-for-id offline-repair', 'wait-idle')
                capture('10-damaged-download')
                drive('tap-id offline-repair', 'wait-for-id offline-0', 'wait-idle')
                assert local_file.read_bytes()==(ROOT/'apps/calibre-web/fixtures/river.epub').read_bytes()
                capture('11-repaired-download')
                # Reconfigure through the actual keyboard; checked setup survives a failed write.
                old_settings = (state/'catalog').read_bytes()
                drive('tap Back', 'tap-id add', 'tap-id kb.layer', 'type '+root.removeprefix('https://').removesuffix('opds'), 'tap-id kb.layer', 'type opds', 'tap Continue')
                drive('scenario storage-full', 'tap-id public-library', 'wait-for-id settings-save', 'wait-idle')
                assert (state/'catalog').read_bytes()==old_settings
                capture('12-setup-save-failed')
                drive('scenario normal', 'tap-id settings-save', 'wait-idle')
                assert (state/'catalog').read_text()==root+'|'
                capture('13-setup-saved')
                # The same real HTTP path distinguishes unauthorized and non-catalog responses.
                for endpoint, expected, name in [('/denied','refused','14-account-refused'),('/invalid','invalid catalog','15-invalid-catalog')]:
                    (state/'catalog').write_text(root.rsplit('/',1)[0]+endpoint+'|')
                    restart()
                    drive('tap-id library', 'wait-for-id retry', 'expect '+expected, 'wait-idle')
                    capture(name)
                (state/'catalog').write_bytes(b'future unreadable settings')
                restart('settings-load')
                capture('16-settings-preserved')
                assert (state/'catalog').read_bytes()==b'future unreadable settings'
                drive('tap-id offline', 'wait-for-id offline-0')
                capture('17-offline-library-with-unreadable-settings')
                # Exercise the runtime-held, server-bound account using the actual reader keyboard.
                (state/'catalog').write_text(root+'|')
                (state/'books-v1').unlink()
                restart()
                drive('tap-id add', 'tap-id kb.layer', 'type '+root.removeprefix('https://').removesuffix('opds'), 'tap-id kb.layer', 'type opds', 'tap Continue')
                authentication['required'] = True
                drive('tap-id sign-in', 'tap-id credential.enter', 'type reader', 'tap Next', 'type sample', 'tap Save', 'wait-for-id entry-0', 'wait-idle')
                assert (state/'catalog').read_text()==root+'|calibre'
                capture('18-private-catalog')
                drive('tap-id entry-0', 'wait-idle', 'expect Authors', 'tap Back', 'tap-id page-next', 'tap-id entry-2', 'tap-id download-book', 'wait-for-id offline-0', 'wait-idle')
                for path in ('/opds','/opds/authors','/books/river.epub'):
                    assert path in authentication['accepted'], (path, authentication)
                capture('19-private-download')
                result = dict(status='passed', profile=args.profile, scale=args.scale,
                              basis='actual-sdk-simulator-with-private-https-opds-and-original-epub',
                              checks=['parsed catalog', 'authors and shelves navigation', 'book details', 'EPUB download', 'verified shelf publication', 'shared reader', 'saved position', 'forced restart and offline reopening', 'position save failure and retry', 'damaged download repair', 'on-screen setup with failed save and retry', 'HTTP authentication refusal', 'malformed catalog response', 'unreadable settings preserved with offline access', 'on-reader server-bound Basic account', 'authenticated catalog navigation and EPUB download'],
                              requests=requests, authenticated_paths=authentication['accepted'])
                (args.output/'result.json').write_text(json.dumps(result,indent=2)+'\n')
            finally:
                if process and process.poll() is None:
                    os.killpg(process.pid,signal.SIGTERM)
                    try: process.wait(timeout=5)
                    except subprocess.TimeoutExpired: os.killpg(process.pid,signal.SIGKILL);process.wait(timeout=5)
                server.shutdown(); server.server_close()

if __name__ == '__main__': main()

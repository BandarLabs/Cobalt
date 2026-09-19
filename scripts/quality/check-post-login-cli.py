#!/usr/bin/env python3
"""Verify `kobo post login` against a local TLS fixture gateway.

A wrong token must be refused before anything is installed; a good one is
checked against the gateway, pinned to it in cobalt-server-account-v1 form,
and installed where the simulator's Post reads it, together with the gateway
address in the application's state. Run: python3 scripts/quality/check-post-login-cli.py
"""
import http.server
import json
import os
import re
import ssl
import subprocess
import sys
import tempfile
import threading
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TOKEN = 'fixture-token-post'
LETTERS = [{'id': f'l-{i:02d}', 'title': f'Fixture letter {i}', 'body': f'Body {i}.'}
           for i in range(1, 4)]


class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.headers.get('Authorization') != f'Bearer {TOKEN}':
            self.send_response(401)
            self.end_headers()
            return
        match = re.match(r'/letters\?page=(\d+)&per_page=(\d+)', self.path)
        if not match:
            self.send_response(404)
            self.end_headers()
            return
        page, per = int(match.group(1)), int(match.group(2))
        body = json.dumps({'total': len(LETTERS),
                           'items': LETTERS[(page - 1) * per: page * per]}).encode()
        self.send_response(200)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *arguments):
        pass


def main():
    target = Path(os.environ.get('CARGO_TARGET_DIR', str(ROOT / 'target'))).resolve()
    cli = target / 'debug/kobo'
    checks = []
    with tempfile.TemporaryDirectory(prefix='cobalt-post-login-', dir='/tmp') as temporary:
        private = Path(temporary)
        env = dict(os.environ, TMPDIR=str(private), CARGO_TARGET_DIR=str(target),
                   CARGO_PROFILE_DEV_DEBUG='0', CARGO_INCREMENTAL='0')
        config = private / 'config'
        env.update(KOBO_STREAM_CONFIG_DIR=str(config),
                   KOBO_SIM_TRUST_DIR=str(config / 'trust'))
        subprocess.run([str(cli), 'stream', 'init', '--host', '127.0.0.1'], cwd=ROOT,
                       env=env, check=True, capture_output=True, timeout=60)
        server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        tls.load_cert_chain(config / 'stream/cert.pem', config / 'stream/key.pem')
        server.socket = tls.wrap_socket(server.socket, server_side=True)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        origin = f'https://127.0.0.1:{server.server_port}'

        def login(token):
            token_file = private / 'token'
            token_file.write_text(token + '\n')
            return subprocess.run([str(cli), 'post', 'login', '--gateway', origin,
                                   '--token-file', str(token_file), '--sim'],
                                  env=env, capture_output=True, text=True, timeout=120)

        secret = private / 'cobalt-sim-secrets/apps/post/servers/hermes-post'
        state = private / 'cobalt-sim-state/post/gateway'

        refused = login('wrong-token')
        checks.append(('a token the gateway refuses stops the login', refused.returncode != 0))
        checks.append(('a refused login installs nothing', not secret.exists() and not state.exists()))

        accepted = login(TOKEN)
        checks.append(('a good token logs in', accepted.returncode == 0))
        checks.append(('the credential is pinned to the gateway', secret.is_file() and
                       secret.read_text() == f'cobalt-server-account-v1\n{origin}\n{TOKEN}'))
        checks.append(('the gateway address reaches the application state',
                       state.is_file() and state.read_text() == origin))
        server.shutdown()

    failed = [name for name, ok in checks if not ok]
    for name, ok in checks:
        print(('PASS ' if ok else 'FAIL ') + name)
    if failed:
        sys.exit(f'{len(failed)} check(s) failed')
    print(f'ALL {len(checks)} CHECKS PASSED')


if __name__ == '__main__':
    main()

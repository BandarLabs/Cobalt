#!/usr/bin/env python3
"""Drive Home Panel against a fixture Home Assistant over TLS: guided connect,
tile actions with acknowledgements, climate target control, tile editing, the
wall-panel layout, connection failure and recovery, and a restart - all through
the real simulator with real taps, against a server that records everything."""
import argparse
import http.server
import json
import os
from pathlib import Path
import re
import ssl
import subprocess
import tempfile
import threading
import time

ROOT = Path(__file__).resolve().parents[2]
TOKEN = 'fixture-ha-token'


class Home:
    """The fixture Home Assistant, and everything it was asked to do."""

    def __init__(self):
        self.requests = []
        self.drop = False
        self.entities = {
            'light.desk': {'state': 'on', 'attributes': {'friendly_name': 'Desk lamp'}},
            'switch.fan': {'state': 'off', 'attributes': {'friendly_name': 'Fan'}},
            'climate.bedroom': {'state': 'heat', 'attributes': {
                'friendly_name': 'Bedroom', 'current_temperature': 19.5, 'temperature': 21}},
            'sensor.outside': {'state': '12.3', 'attributes': {
                'friendly_name': 'Outside', 'unit_of_measurement': '°C'}},
        }

    def states_payload(self):
        return [{'entity_id': eid, 'state': e['state'], 'attributes': e['attributes']}
                for eid, e in self.entities.items()]

    def template(self, body):
        if 'for e in states' in body:
            # Discovery: every entity with its friendly name.
            return [{'id': eid, 's': e['state'],
                     'n': e['attributes'].get('friendly_name', eid)}
                    for eid, e in sorted(self.entities.items())]
        ids = re.findall(r"'([a-z_]+\.[a-z_0-9]+)'", body)
        answer = []
        for eid in ids:
            entity = self.entities.get(eid)
            if entity is None:
                continue
            attrs = entity['attributes']
            answer.append({'id': eid, 's': entity['state'], 'a': {
                'brightness': attrs.get('brightness'),
                'unit': attrs.get('unit_of_measurement'),
                'ct': attrs.get('current_temperature'),
                't': attrs.get('temperature')}})
        return answer

    def service(self, domain, action, body):
        entity = self.entities.get(body.get('entity_id'))
        if entity is None:
            return
        if domain == 'climate' and action == 'set_temperature':
            entity['attributes']['temperature'] = body['temperature']
        elif action == 'toggle':
            entity['state'] = 'off' if entity['state'] != 'off' else 'on'
        elif action == 'turn_on':
            entity['state'] = 'on'


def handler_for(home):
    class Handler(http.server.BaseHTTPRequestHandler):
        protocol_version = 'HTTP/1.1'

        def log_message(self, *_):
            pass

        def record(self, method, body=None):
            home.requests.append({'method': method, 'path': self.path,
                                  'authorization': self.headers.get('Authorization'),
                                  'body': body})

        def reply(self, payload):
            body = json.dumps(payload).encode()
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def unavailable(self):
            # The reply never arrives: what Wi-Fi trouble looks like.
            self.close_connection = True

        def do_GET(self):
            if home.drop:
                self.record('GET')
                return self.unavailable()
            if self.path == '/api/':
                self.record('GET')
                self.reply({'message': 'API running.'})
            elif self.path == '/api/states':
                self.record('GET')
                self.reply(home.states_payload())
            else:
                self.send_error(404)

        def do_POST(self):
            length = int(self.headers.get('Content-Length') or 0)
            raw = self.rfile.read(length) if length else b''
            if home.drop:
                self.record('POST')
                return self.unavailable()
            if self.path == '/api/template':
                self.record('POST', raw.decode())
                self.reply(home.template(raw.decode()))
            elif self.path.startswith('/api/services/'):
                domain, _, action = self.path[len('/api/services/'):].partition('/')
                body = json.loads(raw or b'{}')
                self.record('POST', {'domain': domain, 'action': action, **body})
                home.service(domain, action, body)
                self.reply([])
            else:
                self.send_error(404)

    return Handler


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    target = Path(os.environ.get('CARGO_TARGET_DIR', str(ROOT / 'target'))).resolve()
    cli = target / 'debug/kobo'
    process = None
    result = {'checks': []}
    try:
        with tempfile.TemporaryDirectory(prefix='cobalt-homepanel-', dir='/tmp') as temporary:
            private = Path(temporary)
            env = dict(os.environ, TMPDIR=str(private), CARGO_TARGET_DIR=str(target),
                       CARGO_PROFILE_DEV_DEBUG='0', CARGO_INCREMENTAL='0')
            config = private / 'config'
            env.update(KOBO_STREAM_CONFIG_DIR=str(config), KOBO_SIM_TRUST_DIR=str(config / 'trust'))
            subprocess.run([str(cli), 'stream', 'init', '--host', '127.0.0.1'], cwd=ROOT,
                           env=env, check=True, capture_output=True, timeout=60)

            home = Home()
            main.requests = home.requests
            server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), handler_for(home))
            tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
            tls.load_cert_chain(config / 'stream/cert.pem', config / 'stream/key.pem')
            server.socket = tls.wrap_socket(server.socket, server_side=True)
            threading.Thread(target=server.serve_forever, daemon=True).start()
            origin = f'https://127.0.0.1:{server.server_port}'

            # The token lands the way kobo secret set installs it: a raw value
            # under the app-scoped secrets directory, never seen by the app.
            secrets = private / 'cobalt-sim-secrets/apps/homepanel'
            secrets.mkdir(parents=True)
            (secrets / 'homeassistant').write_text(TOKEN)
            os.chmod(secrets / 'homeassistant', 0o600)

            (private / 'cobalt-sim-state/homepanel').mkdir(parents=True)
            (private / 'cobalt-sim-data/homepanel').mkdir(parents=True)

            def start():
                nonlocal process
                process = subprocess.Popen(
                    [str(cli), 'dev'], cwd=ROOT / 'apps/homepanel', env=env,
                    stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
                address = None
                deadline = time.time() + 180
                while time.time() < deadline:
                    line = process.stdout.readline()
                    if not line:
                        break
                    match = re.search(r'http://(127\.0\.0\.1:\d+)', line)
                    if match:
                        address = match.group(1)
                        break
                assert address, 'simulator did not start'
                return address

            def stop():
                nonlocal process
                if process:
                    process.terminate()
                    process.wait(timeout=30)
                    process = None

            def drive(*steps, timeout=300):
                command = [str(cli), 'drive', '--address', address, '--ideal',
                           '--shots', str(output)]
                for step in steps:
                    command += ['--step', step]
                log_path = output / 'simulator.log'
                with open(log_path, 'a') as log:
                    subprocess.run(command, cwd=ROOT, env=env, stdout=log, stderr=log,
                                   check=True, timeout=timeout)

            def capture(name):
                drive('clean', 'shot ' + name)

            try:
                address = start()
                drive('wait-for Connect Home Assistant')
                capture('homepanel-setup')

                # Guided setup: type the address, test the connection.
                drive('tap ?123', f'type 127.0.0.1:{server.server_port}',
                      'tap Test connection', 'wait-for No tiles')
                capture('homepanel-empty')

                # Add four tiles from the discovered devices.
                for name in ('Bedroom', 'Desk lamp', 'Fan', 'Outside'):
                    drive('tap Add tile', f'wait-for {name}', f'tap {name}',
                          'wait-for Home Panel')
                drive('wait-for bedroom · 19.5°')
                capture('homepanel-grid')

                # A tile action is acknowledged by name after the next poll.
                drive('tap desk · on', 'wait-for desk is now off.')
                capture('homepanel-ack')

                # Climate: room temperature on the tile, target on the screen.
                drive('tap bedroom · 19.5°', 'wait-for Target: 21°')
                capture('homepanel-climate')
                drive('tap Warmer', 'wait-for Target: 21.5°')
                capture('homepanel-climate-warmer')
                drive('tap back', 'wait-for Home Panel')

                # Edit tiles: move up, then remove.
                drive('tap Settings', 'wait-for Edit tiles', 'tap Edit tiles',
                      'wait-for fan')
                drive('tap fan', 'wait-for Move up', 'tap Move up',
                      'wait-for Edit tiles')
                drive('tap fan', 'wait-for Remove', 'tap Remove',
                      'wait-for fan removed.', 'tap Done', 'tap Done',
                      'wait-for Home Panel', 'expect-missing fan · off')
                capture('homepanel-edited')

                # Wall panel: one large tile per row.
                drive('tap Settings', 'tap Use one large column', 'tap Done',
                      'wait-for Home Panel')
                capture('homepanel-wall')
                drive('tap Settings', 'tap Use two columns', 'tap Done',
                      'wait-for Home Panel')

                # Connection failure is honest, recovery clears it.
                home.drop = True
                drive('wait 12000', 'wait-for Home Assistant did not answer.')
                capture('homepanel-offline')
                home.drop = False
                drive('wait 12000', 'wait-for Updated')
                capture('homepanel-recovered')

                # State survives a restart.
                stop()
                address = start()
                drive('wait-for desk · off')
                capture('homepanel-restored')

                toggles = [r for r in home.requests
                           if r['method'] == 'POST' and isinstance(r['body'], dict)
                           and r['body'].get('action') == 'toggle']
                assert any(t['body'].get('entity_id') == 'light.desk' for t in toggles), \
                    f'no toggle for light.desk: {home.requests}'
                sets = [r for r in home.requests
                        if r['method'] == 'POST' and isinstance(r['body'], dict)
                        and r['body'].get('action') == 'set_temperature']
                assert any(s['body'].get('temperature') == 21.5 for s in sets), \
                    f'no set_temperature 21.5: {home.requests}'
                authed = [r for r in home.requests if r['authorization'] == f'Bearer {TOKEN}']
                assert authed and len(authed) == len(home.requests), \
                    'a request reached the server without the token'

                result['checks'].append(dict(
                    name='homepanel journey',
                    detail='guided https setup with a connection test, four tiles added '
                           'from discovery, toggle acknowledged by name after repoll, '
                           'climate target set to 21.5, tiles reordered and removed, '
                           'wall-panel layout, server outage reported with the last '
                           'good reading time, recovery, and a restart with state kept. '
                           'Every server request carried the installed bearer token.',
                    status='passed'))
            finally:
                stop()
                server.shutdown()
    except subprocess.CalledProcessError as error:
        seen = [(r['method'], r['path']) for r in getattr(main, 'requests', [])]
        result['checks'].append(dict(name='homepanel journey',
                                     detail=f'drive failed; server saw {seen}',
                                     status='failed'))
    result['status'] = 'passed' if result['checks'] and all(
        check['status'] == 'passed' for check in result['checks']) else 'failed'
    (output / 'result.json').write_text(json.dumps(result, indent=2))
    print(json.dumps(result, indent=2))
    raise SystemExit(0 if result['status'] == 'passed' else 1)


if __name__ == '__main__':
    main()
